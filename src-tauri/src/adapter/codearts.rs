//! 华为云 CodeArts（`codearts`）内置适配器 —— 额度监控型。
//!
//! ## 更正：CodeArts **确实有「每日签到」**（2026-11-27 重新逆向 `app.asar` 证实）
//!
//! 早期版本本模块记载「全文检索 `每日`/`签到`/`打卡`/`daily` 均 0 命中」，
//! 该结论**已过时/错误**。当前 CodeArts 客户端（`活动福利` 面板）真实存在：
//!
//! * 每日签到领 {n} 积分（真机 `type=USER_LOGIN` / `triggerEvent=user.login`，
//!   早期 asar 误读为 `daily_claim`；按钮「每日领取」）——**这就是每日签到**；
//! * 学生认证领积分（`type=STUDENT_CERTIFIED`，每月 5000，有效期至 2026-12-31）；
//! * 邀请好友得积分（`type=invite`）。
//!
//! ### 真实数据面（`BenefitService`，全部走华为云 SDK-HMAC-SHA256 签名，签名算法与本适配器
//! `sign_headers_with_body` 完全一致，故实现无新增密码学成本）：
//!
//! ```text
//! GET  {policyCenterDomain}/v1/ops/delivery?channel=DESKTOP      // 拉活动列表 → items[]
//! POST {policyCenterDomain}/v1/ops/claim    {campaignId, idempotentKey, channel:"DESKTOP"}  // 领取
//! POST {policyCenterDomain}/v1/ops/confirm  {userBenefitId}      // 领取后确认（两步）
//! ```
//!
//! 头部固定：`X-Security-Token`(=STS)、`Agent-Type: PromptCenter`、`Content-Type: application/json`、
//! `X-Language: zh-cn`（delivery 另加 `x-snap-traceid`）。
//!
//! `items[]` 关键字段：`campaignId`、`type`(DAILY_CLAIM/STUDENT_CERTIFY/INVITE_USER/…)、
//! `status`(CLAIMED/CONFIRMED/CONSUMED=已领)、`claimable`、`benefitAmount`、`displayConfig`。
//!
//! 旧 `BenefitService.claimBenefit`（`/api/v1/benefit/claim`）是另一套旧接口，仅做查询/一次性领取；
//! 当前福利主路径是 `/v1/ops/*`。因此本适配器现状：
//!
//! * `supports_quota_query = true`
//! * `supports_daily_claim = false`（**待改造为 true**，见 docs/DOUBAO_CODEARTS_CLAIM_FEASIBILITY_2026-11-27.md）
//!
//! ## 凭据形态
//!
//! 客户端用 Electron `safeStorage` 把 `{session, loginContext}` 加密后存到
//! `<userData>/secretStorage/<sha256(key)>.enc`，key 为 `authSession:<region>`。
//! macOS 上 safeStorage 等价于 Chromium `v10` 信封（AES-128-CBC，密钥由
//! Keychain 条目 `<appName> Safe Storage` 经 PBKDF2 派生），因此本适配器直接
//! 读该文件并用 Keychain 密码解密，不依赖客户端运行时。
//!
//! session 内含华为云临时凭据：`accessKey` / `secretKey` / `securityToken`。
//! 三者都是请求签名必需项。
//!
//! ## 认证方式
//!
//! 不是 Bearer Token，而是华为云 **SDK-HMAC-SHA256** 请求签名：
//!
//! ```text
//! CanonicalRequest = METHOD\nCanonicalURI\nCanonicalQuery\nCanonicalHeaders\nSignedHeaders\nSHA256(body)
//! StringToSign     = "SDK-HMAC-SHA256"\nX-Sdk-Date\nSHA256(CanonicalRequest)
//! Authorization    = SDK-HMAC-SHA256 Access=<ak>, SignedHeaders=<h>, Signature=<hmac>
//! ```
//!
//! 签名错误时服务端返回 `401 APIG.0301`，可据此区分凭据失效与业务错误。
//!
//! ## 额度接口
//!
//! `GET {snapEngineUrl}/snap-manager/v1/package/overview`，返回三段积分：
//! `all_credit_package`（合计）/ `basic_package`（基础）/ `bonus_credit_package`（赠送）。

use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL},
    Engine as _,
};
use chrono::Utc;
use reqwest::header::{ACCEPT, USER_AGENT};
use ring::rand::{SecureRandom as _, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair as _, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::adapter::common::{
    hash_fallback_uid, json_uid_to_string, map_http_status, shared_http_client,
};
use crate::adapter::safe_storage;
use crate::adapter::{
    AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult, SessionBackup, ToolAdapter,
};
use crate::domain::{
    AuthMethod, ClaimStatus, CredentialEnvelope, CredentialPayload, QuotaBucket, QuotaSnapshot,
    UnifiedErrorCode,
};
use crate::platform::PlatformHost;

const DEFAULT_SNAP_ENGINE: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";
const DEFAULT_IAM_STS: &str = "https://sts.cn-north-4.myhuaweicloud.com";
const DEFAULT_REGION: &str = "cn-north-4";
const USER_AGENT_VALUE: &str = "CodeArts/0.2.3";
const SIGN_ALGORITHM: &str = "SDK-HMAC-SHA256";

// extra 字段键名（凭据信封内传递华为云临时凭据）
const EXTRA_ACCESS_KEY: &str = "access_key";
const EXTRA_SECRET_KEY: &str = "secret_key";
const EXTRA_SECURITY_TOKEN: &str = "security_token";
const EXTRA_REGION: &str = "region";
const EXTRA_DOMAIN_ID: &str = "domain_id";
const EXTRA_CODEARTS_MODE: &str = "codearts_mode";
const EXTRA_CODE_VERIFIER: &str = "code_verifier";
const EXTRA_DPOP_PKCS8_B64: &str = "dpop_pkcs8_b64";
/// 本机导入时保存的完整客户端 session 快照（base64 JSON），切号时原样写回
const EXTRA_SESSION_SNAPSHOT: &str = "session_snapshot_b64";

const CODEARTS_PORTAL_URL: &str = "https://codearts.huaweicloud.com/portal";
const CODEARTS_OAUTH_CLIENT_ID: &str = "codearts-agent";
const CODEARTS_OAUTH_REDIRECT_PATH: &str = "/oauth/callback";
const CODEARTS_LOGIN_PLUGIN_NAME: &str = "snap_AIIDE";
const CODEARTS_LOGIN_PLUGIN_VERSION: &str = "5.2.0";
const CODEARTS_REFRESH_LEAD_MS: i64 = 30 * 60 * 1000;

/// 凭据**硬过期**兜底阈值：只有当本地记录的有效期已经越过「当前时间 + 该容差」时才在
/// 本地直接拦截（避免发出注定 401 的请求）。
///
/// **为什么不再用 5 分钟窗口硬拦**：华为云 STS 的过期以服务端为准，本地 `expiresAt`
/// 只是客户端某次写入的快照，可能早于服务端真实失效时间（时钟漂移、客户端未及时回写）。
/// 过去「剩余 < 5 分钟即判 AuthExpired」会把**服务端其实仍可用**的 STS 误标为过期，
/// 从而逼用户「打开码道客户端重新关联本机」——这正是「频繁关联本机」痛感的来源之一。
/// 现在改为：只要**尚未真正越过有效期**，就放行请求、由服务端 401 来确证失效；
/// 仅当已明显越界（含容差）时才本地快速失败。真正过期的凭据仍会被拦截（见单测）。
/// 本地「硬过期」判定容差：有效期越过 `now + 该值` 才本地拦截。
/// 取 60 秒，足以吸收时钟抖动又不放行真正过期的凭据。
const EXPIRY_HARD_TOLERANCE_MS: i64 = 60 * 1000;

/// 凭据过期时给用户的行动指引。
const EXPIRED_HINT: &str =
    "CodeArts 凭据已过期。请打开「码道 CodeArts」客户端，它会自动刷新登录态；\
     刷新完成后回到 Qwitch 重新导入该账号即可";

/// 把外部时间戳统一成**毫秒**。
///
/// 码道客户端不同版本对 `expiresAt` 的单位并不统一（秒 / 毫秒 / ISO 字符串），
/// 历史实现直接 `as_i64()` 当毫秒用：秒级时间戳会被判成 1970 年，从而在导入时
/// 误报「凭据已过期」，即使客户端明明处于登录状态。
fn normalize_epoch_ms(v: &Value) -> Option<i64> {
    const SECONDS_CUTOFF: i64 = 10_000_000_000;
    let from_num = |n: i64| {
        Some(if n > 0 && n < SECONDS_CUTOFF {
            n * 1000
        } else {
            n
        })
    };
    match v {
        Value::Number(n) => n.as_i64().and_then(from_num),
        Value::String(raw) => {
            let t = raw.trim();
            if t.is_empty() {
                return None;
            }
            if let Ok(n) = t.parse::<i64>() {
                return from_num(n);
            }
            chrono::DateTime::parse_from_rfc3339(t)
                .ok()
                .map(|dt| dt.timestamp_millis())
        }
        _ => None,
    }
}

/// 华为云临时凭据（AK/SK/STS）。
#[derive(Debug, Clone)]
struct HuaweiCreds {
    access_key: String,
    secret_key: String,
    security_token: String,
    /// 区域标识（`cn-north-4`）。当前仅用于凭据往返，
    /// 签名用的 host 直接取自目标 URL，故不参与计算。
    #[allow(dead_code)]
    region: String,
}

impl HuaweiCreds {
    fn from_envelope(cred: &CredentialEnvelope) -> Result<Self, AdapterError> {
        let CredentialPayload::Token { extra, .. } = &cred.payload else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts 凭据缺少 AK/SK，请重新导入".into(),
            });
        };
        let get = |k: &str| {
            extra
                .get(k)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let access_key = get(EXTRA_ACCESS_KEY).ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "CodeArts 凭据缺少 access_key".into(),
        })?;
        let secret_key = get(EXTRA_SECRET_KEY).ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "CodeArts 凭据缺少 secret_key".into(),
        })?;
        let security_token = get(EXTRA_SECURITY_TOKEN).ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "CodeArts 凭据缺少 security_token".into(),
        })?;
        Ok(Self {
            access_key,
            secret_key,
            security_token,
            region: get(EXTRA_REGION).unwrap_or_else(|| DEFAULT_REGION.to_string()),
        })
    }
}

/// 华为云 CodeArts 适配器（额度监控 + 每日签到型）。
pub struct CodeArtsAdapter {
    manifest: AdapterManifest,
    http: reqwest::Client,
    snap_engine: String,
    iam_sts: String,
    /// 保留宿主引用：`import_from_local` 需要读客户端配置目录。
    /// `import_from_local` 走的是入参 `host`，故此字段仅为与其它内置适配器
    /// 保持一致的构造签名而保留。
    #[allow(dead_code)]
    host: Option<std::sync::Arc<dyn PlatformHost>>,
}

impl CodeArtsAdapter {
    pub fn new() -> Self {
        Self::with_bases(DEFAULT_SNAP_ENGINE, DEFAULT_IAM_STS, None)
    }

    pub fn with_host(host: std::sync::Arc<dyn PlatformHost>) -> Self {
        Self::with_bases(DEFAULT_SNAP_ENGINE, DEFAULT_IAM_STS, Some(host))
    }

    #[cfg(test)]
    pub(crate) fn with_base_urls(
        snap_engine: impl Into<String>,
        iam_sts: impl Into<String>,
    ) -> Self {
        Self::with_bases(snap_engine, iam_sts, None)
    }

    #[cfg(test)]
    pub(crate) fn with_base_urls_and_host(
        snap_engine: impl Into<String>,
        iam_sts: impl Into<String>,
        host: std::sync::Arc<dyn PlatformHost>,
    ) -> Self {
        Self::with_bases(snap_engine, iam_sts, Some(host))
    }

    fn with_bases(
        snap_engine: impl Into<String>,
        iam_sts: impl Into<String>,
        host: Option<std::sync::Arc<dyn PlatformHost>>,
    ) -> Self {
        Self {
            manifest: AdapterManifest {
                id: "codearts".to_string(),
                name: "码道 CodeArts".to_string(),
                version: "0.1.0".to_string(),
                is_builtin: true,
                capabilities: AdapterCapabilities {
                    auth_methods: vec![
                        AuthMethod::LocalImport,
                        AuthMethod::WebviewOauth,
                        AuthMethod::TokenPaste,
                    ],
                    supports_quota_query: true,
                    // CodeArts「活动福利」确有每日签到（真机 type=USER_LOGIN），走 /v1/ops/* 纯服务端接口
                    supports_daily_claim: true,
                    // 切号 = safeStorage 重加密写回，仅 macOS 可复现（Windows 走 DPAPI）
                    supports_client_switch: cfg!(target_os = "macos"),
                    claim_requires_client_switch: false,
                    supports_multi_instance: false,
                },
            },
            http: shared_http_client().clone(),
            snap_engine: snap_engine.into().trim_end_matches('/').to_string(),
            iam_sts: iam_sts.into().trim_end_matches('/').to_string(),
            host,
        }
    }

    /// 定位 CodeArts 的 secretStorage 目录。
    fn resolve_secret_dir(host: &dyn PlatformHost) -> Result<PathBuf, AdapterError> {
        let root = host
            .client_config_dir("codearts")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: format!("未找到 CodeArts 配置目录: {e}"),
            })?;
        let dir = root.join("secretStorage");
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: "未找到 CodeArts 凭据目录，请先安装并登录客户端".into(),
            })
        }
    }

    /// secretStorage 的文件名 = `sha256(key)` 的十六进制 + `.enc`。
    fn secret_file_name(key: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        format!("{}.enc", hex::encode(hasher.finalize()))
    }

    /// 在 macOS 上从 Keychain 读取 safeStorage 主密码。
    ///
    /// 服务名形如 `<AppName> Safe Storage`，账号名与 app 名一致。
    /// 该命令可能触发系统授权弹窗，因此只在真正需要解密时调用一次。
    fn macos_safe_storage_password(app_name: &str) -> Result<String, AdapterError> {
        safe_storage::macos_safe_storage_password(app_name)
    }

    /// 解密 Chromium `v10` safeStorage 信封（AES-128-CBC）。
    fn decrypt_safe_storage(payload: &[u8], password: &str) -> Result<Vec<u8>, AdapterError> {
        safe_storage::decrypt_safe_storage(payload, password, "CodeArts")
    }

    /// 从 secretStorage 读取并解析 `{session, loginContext}`。
    fn load_session(host: &dyn PlatformHost) -> Result<Value, AdapterError> {
        let dir = Self::resolve_secret_dir(host)?;
        let file = dir.join(Self::secret_file_name(&format!(
            "authSession:{DEFAULT_REGION}"
        )));
        let raw_b64 = std::fs::read_to_string(&file).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::ClientNotFound,
            message: format!("读取 CodeArts 凭据文件失败: {e}"),
        })?;
        let encrypted = base64::engine::general_purpose::STANDARD
            .decode(raw_b64.trim())
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("CodeArts 凭据文件不是合法 base64: {e}"),
            })?;

        let password = Self::macos_safe_storage_password("codearts")?;
        let plain = Self::decrypt_safe_storage(&encrypted, &password)?;
        serde_json::from_slice(&plain).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("CodeArts 凭据结构解析失败: {e}"),
        })
    }

    /// 把已解析的 session 转成凭据信封。
    fn envelope_from_session(session: &Value) -> Result<CredentialEnvelope, AdapterError> {
        let s = session
            .get("session")
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts 凭据缺少 session 字段".into(),
            })?;
        let access_key = non_empty(s, "accessKey").ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "CodeArts 会话缺少 accessKey".into(),
        })?;
        let secret_key = non_empty(s, "secretKey").ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "CodeArts 会话缺少 secretKey".into(),
        })?;
        let security_token =
            non_empty(s, "securityToken").ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts 会话缺少 securityToken".into(),
            })?;
        let uid = non_empty(s, "userId")
            .or_else(|| non_empty(s, "userName"))
            .unwrap_or_else(|| hash_fallback_uid("codearts", access_key.as_bytes()));
        let display_name = non_empty(s, "userName");
        let expires_at = s
            .get("expiresAt")
            .or_else(|| s.get("expires_at"))
            .and_then(normalize_epoch_ms);
        let refresh_token = non_empty(s, "refreshToken");
        let domain_id = non_empty(s, "domainId");

        let mut extra = HashMap::new();
        extra.insert(EXTRA_ACCESS_KEY.to_string(), access_key);
        extra.insert(EXTRA_SECRET_KEY.to_string(), secret_key);
        extra.insert(EXTRA_SECURITY_TOKEN.to_string(), security_token);
        extra.insert(EXTRA_REGION.to_string(), DEFAULT_REGION.to_string());
        if let Some(d) = domain_id {
            extra.insert(EXTRA_DOMAIN_ID.to_string(), d);
        }
        // 存一份完整 `{session, loginContext}` 快照：切号要写回 secretStorage，
        // 而 AK/SK/STS 三个字段并不足以还原客户端认可的会话文件。
        // 只从本机导入时才拿得到原文；独立 OAuth 授权的账号没有这份快照。
        if let Ok(raw) = serde_json::to_vec(session) {
            extra.insert(
                EXTRA_SESSION_SNAPSHOT.to_string(),
                BASE64.encode(raw.as_slice()),
            );
        }

        Ok(CredentialEnvelope {
            tool_id: "codearts".to_string(),
            uid,
            display_name,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: String::new(),
                refresh_token,
                expires_at,
                extra,
            },
        })
    }

    /// 码道数据面（snap-access）的候选 host 列表，按凭据里的 region 选择国内/国际站。
    /// 国内站主域名 `.myhuaweicloud.com` 在部分代理/TUN 环境下 TLS 握手会 reset，
    /// 因此附上码道官方内置的 `.myhuaweicloud.cn` 备用域名按序回退。
    pub(crate) fn snap_base_urls(cred: &CredentialEnvelope) -> Vec<String> {
        let region = match &cred.payload {
            CredentialPayload::Token { extra, .. } => extra
                .get(EXTRA_REGION)
                .map(String::as_str)
                .unwrap_or(DEFAULT_REGION),
            _ => DEFAULT_REGION,
        };
        if region.starts_with("ap-") {
            vec!["https://snap-access.ap-southeast-1.myhuaweicloud.com".to_string()]
        } else {
            vec![
                "https://snap-access.cn-north-4.myhuaweicloud.com".to_string(),
                "https://snap-access.cn-north-4.myhuaweicloud.cn".to_string(),
            ]
        }
    }

    /// 码道数据面（snap-access）的首选 host。
    #[cfg(test)]
    pub(crate) fn snap_base_url(cred: &CredentialEnvelope) -> String {
        Self::snap_base_urls(cred)
            .into_iter()
            .next()
            .unwrap_or_else(|| DEFAULT_SNAP_ENGINE.to_string())
    }

    /// 福利/活动接口（`/v1/ops/*`）的 host。
    ///
    /// 客户端 `policyCenterDomain` 与 `snapEngineUrl` 指向同一数据面
    /// （`snap-access.cn-north-4...`），故直接复用适配器已配置的 `snap_engine`。
    fn policy_center_base(&self) -> String {
        self.snap_engine.trim_end_matches('/').to_string()
    }

    /// 为一次具体请求生成完整的签名头（含 `X-Security-Token`、`Content-Type`）。
    ///
    /// 供网关的对话上游复用：码道不接受 bearer，只认这套签名。
    pub(crate) fn signed_request_headers(
        cred: &CredentialEnvelope,
        method: &str,
        url: &str,
        body: &[u8],
    ) -> Result<Vec<(String, String)>, String> {
        let creds = HuaweiCreds::from_envelope(cred).map_err(|e| e.to_string())?;
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("码道上游地址非法: {e}"))?;
        let mut headers = Self::sign_headers_with_body(
            method,
            &parsed,
            &creds,
            &[
                ("Content-Type", "application/json"),
                ("X-Security-Token", creds.security_token.as_str()),
            ],
            body,
        );
        // 小写化会破坏 reqwest 的规范化；这里恢复成常见写法，值保持不变
        for (k, _) in headers.iter_mut() {
            if k.eq_ignore_ascii_case("authorization") {
                *k = "Authorization".to_string();
            } else if k.eq_ignore_ascii_case("x-sdk-date") {
                *k = "X-Sdk-Date".to_string();
            } else if k.eq_ignore_ascii_case("x-security-token") {
                *k = "X-Security-Token".to_string();
            } else if k.eq_ignore_ascii_case("content-type") {
                *k = "Content-Type".to_string();
            } else if k.eq_ignore_ascii_case("host") {
                *k = "Host".to_string();
            }
        }
        Ok(headers)
    }

    /// 构造华为云 SDK-HMAC-SHA256 签名头。
    fn sign_headers(
        method: &str,
        url: &reqwest::Url,
        creds: &HuaweiCreds,
        extra_headers: &[(&str, &str)],
    ) -> Vec<(String, String)> {
        Self::sign_headers_with_body(method, url, creds, extra_headers, b"")
    }

    /// 同 [`sign_headers`]，但把**请求体哈希**算进签名串。
    ///
    /// 历史实现把 payload 哈希写死成 `sha256("")`，所以只能签 GET（额度查询）；
    /// 对话接口是 POST，body 的哈希必须进 canonical request，否则服务端校验签名失败。
    fn sign_headers_with_body(
        method: &str,
        url: &reqwest::Url,
        creds: &HuaweiCreds,
        extra_headers: &[(&str, &str)],
        body: &[u8],
    ) -> Vec<(String, String)> {
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};

        let canonical_uri = {
            let mut out = String::new();
            for (i, seg) in url
                .path_segments()
                .map(|s| s.collect::<Vec<_>>())
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                if i > 0 {
                    out.push('/');
                }
                out.push_str(&percent_encode(seg));
            }
            if !out.starts_with('/') {
                out.insert(0, '/');
            }
            if !out.ends_with('/') {
                out.push('/');
            }
            out
        };

        let mut query_pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (percent_encode(&k), percent_encode(&v)))
            .collect();
        query_pairs.sort();
        let canonical_query = query_pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let host = url
            .host_str()
            .map(|h| match url.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_string(),
            })
            .unwrap_or_default();
        let now = Utc::now();
        let sdk_date = now.format("%Y%m%dT%H%M%SZ").to_string();

        let mut headers: Vec<(String, String)> = extra_headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        headers.push(("host".to_string(), host));
        headers.push(("X-Sdk-Date".to_string(), sdk_date.clone()));
        headers.sort_by_key(|(k, _)| k.to_lowercase());

        let canonical_headers = headers
            .iter()
            .map(|(k, v)| format!("{}:{}\n", k.to_lowercase(), v.trim()))
            .collect::<String>();
        let signed_headers = headers
            .iter()
            .map(|(k, _)| k.to_lowercase())
            .collect::<Vec<_>>()
            .join(";");

        let body_hash = hex::encode(Sha256::digest(body));
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.to_uppercase(),
            canonical_uri,
            canonical_query,
            canonical_headers,
            signed_headers,
            body_hash
        );
        let canonical_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let string_to_sign = format!("{SIGN_ALGORITHM}\n{sdk_date}\n{canonical_hash}");
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(creds.secret_key.as_bytes())
            .expect("HMAC 接受任意长度密钥");
        mac.update(string_to_sign.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        headers.push((
            "Authorization".to_string(),
            format!(
                "{SIGN_ALGORITHM} Access={}, SignedHeaders={}, Signature={}",
                creds.access_key, signed_headers, signature
            ),
        ));
        headers
    }

    /// 发起一次带签名的 GET 请求。
    async fn signed_get(
        &self,
        url: &str,
        creds: &HuaweiCreds,
        extra_headers: &[(&str, &str)],
        context: &str,
    ) -> Result<Value, AdapterError> {
        let parsed = reqwest::Url::parse(url).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("{context}: URL 非法 ({e})"),
        })?;
        let headers = Self::sign_headers("GET", &parsed, creds, extra_headers);

        let mut req = self
            .http
            .get(parsed)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE);
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(first_err) => {
                if url.contains(".myhuaweicloud.com") {
                    let cn_url = url.replace(".myhuaweicloud.com", ".myhuaweicloud.cn");
                    if let Ok(cn_parsed) = reqwest::Url::parse(&cn_url) {
                        let cn_headers =
                            Self::sign_headers("GET", &cn_parsed, creds, extra_headers);
                        let mut cn_req = self
                            .http
                            .get(cn_parsed)
                            .header(ACCEPT, "application/json")
                            .header(USER_AGENT, USER_AGENT_VALUE);
                        for (k, v) in &cn_headers {
                            cn_req = cn_req.header(k.as_str(), v.as_str());
                        }
                        cn_req.send().await.map_err(|e| AdapterError::Unified {
                            code: UnifiedErrorCode::NetworkError,
                            message: format!("{context}失败: {}", e.without_url()),
                        })?
                    } else {
                        return Err(AdapterError::Unified {
                            code: UnifiedErrorCode::NetworkError,
                            message: format!("{context}失败: {}", first_err.without_url()),
                        });
                    }
                } else {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::NetworkError,
                        message: format!("{context}失败: {}", first_err.without_url()),
                    });
                }
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            let lower = body_text.to_ascii_lowercase();
            // 401 或 APIG.0301 / APIG.0602 表示 STS/AK/SK 签名或令牌失效 → 凭据失效而非网络问题
            if status.as_u16() == 401
                || lower.contains("apig.0602")
                || lower.contains("apig.0301")
                || lower.contains("security token has expired")
            {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: format!(
                        "{context}: 华为云鉴权/签名失效 (HTTP {})，凭据可能已过期",
                        status.as_u16()
                    ),
                });
            }
            return Err(map_http_status(status, context));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("解析{context}响应失败: {}", e.without_url()),
            })
    }

    /// 发起一次带签名（含 body 哈希）的 POST JSON 请求。
    ///
    /// 福利领取链路（`/v1/ops/claim`、`/v1/ops/confirm`）都是 POST + JSON body，
    /// 必须把 body 哈希算进 canonical request，故复用 `sign_headers_with_body`。
    async fn signed_post_json(
        &self,
        url: &str,
        creds: &HuaweiCreds,
        extra_headers: &[(&str, &str)],
        body: &Value,
        context: &str,
    ) -> Result<Value, AdapterError> {
        let parsed = reqwest::Url::parse(url).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("{context}: URL 非法 ({e})"),
        })?;
        let body_bytes = serde_json::to_vec(body).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("{context}: 序列化请求体失败 ({e})"),
        })?;
        let headers =
            Self::sign_headers_with_body("POST", &parsed, creds, extra_headers, &body_bytes);

        let mut req = self
            .http
            .post(parsed)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .body(body_bytes.clone());
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(first_err) => {
                if url.contains(".myhuaweicloud.com") {
                    let cn_url = url.replace(".myhuaweicloud.com", ".myhuaweicloud.cn");
                    if let Ok(cn_parsed) = reqwest::Url::parse(&cn_url) {
                        let cn_headers = Self::sign_headers_with_body(
                            "POST",
                            &cn_parsed,
                            creds,
                            extra_headers,
                            &body_bytes,
                        );
                        let mut cn_req = self
                            .http
                            .post(cn_parsed)
                            .header(ACCEPT, "application/json")
                            .header(USER_AGENT, USER_AGENT_VALUE)
                            .body(body_bytes);
                        for (k, v) in &cn_headers {
                            cn_req = cn_req.header(k.as_str(), v.as_str());
                        }
                        cn_req.send().await.map_err(|e| AdapterError::Unified {
                            code: UnifiedErrorCode::NetworkError,
                            message: format!("{context}失败: {}", e.without_url()),
                        })?
                    } else {
                        return Err(AdapterError::Unified {
                            code: UnifiedErrorCode::NetworkError,
                            message: format!("{context}失败: {}", first_err.without_url()),
                        });
                    }
                } else {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::NetworkError,
                        message: format!("{context}失败: {}", first_err.without_url()),
                    });
                }
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            let lower = body_text.to_ascii_lowercase();
            if status.as_u16() == 401
                || lower.contains("apig.0602")
                || lower.contains("apig.0301")
                || lower.contains("security token has expired")
            {
                let detail = if std::env::var("CODEARTS_PROBE_DUMP_401").as_deref() == Ok("1") {
                    body_text
                } else {
                    String::new()
                };
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: format!(
                        "{context}: 华为云鉴权/签名失效 (HTTP {})，凭据可能已过期 {detail}",
                        status.as_u16()
                    ),
                });
            }
            return Err(map_http_status(status, context));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("解析{context}响应失败: {}", e.without_url()),
            })
    }

    /// 福利/活动接口的公共签名头（`Agent-Type: PromptCenter` + `X-Language: zh-cn`）。
    fn benefit_extra_headers(creds: &HuaweiCreds) -> Vec<(&str, &str)> {
        vec![
            ("Content-Type", "application/json"),
            ("X-Security-Token", creds.security_token.as_str()),
            ("Agent-Type", "PromptCenter"),
            ("X-Language", "zh-cn"),
        ]
    }

    /// 拉取活动福利列表（`GET /v1/ops/delivery?channel=DESKTOP`），返回 `data.items`。
    async fn fetch_delivery_items(&self, creds: &HuaweiCreds) -> Result<Vec<Value>, AdapterError> {
        let base = self.policy_center_base();
        let url = format!("{base}/v1/ops/delivery?channel=DESKTOP");
        let trace_id = uuid_like();
        let mut headers = Self::benefit_extra_headers(creds);
        headers.push(("x-snap-traceid", trace_id.as_str()));
        let delivery = self
            .signed_get(&url, creds, &headers, "查询码道活动福利")
            .await?;
        if std::env::var("CODEARTS_PROBE_DUMP_DELIVERY").as_deref() == Ok("1") {
            eprintln!(
                "[dump] delivery raw = {}",
                serde_json::to_string_pretty(&delivery).unwrap_or_default()
            );
        }
        let items = delivery
            .get("data")
            .and_then(|d| d.get("items"))
            .and_then(|v| v.as_array())
            .or_else(|| delivery.get("items").and_then(|v| v.as_array()))
            .cloned()
            .unwrap_or_default();
        Ok(items)
    }

    async fn fetch_package_overview(&self, creds: &HuaweiCreds) -> Result<Value, AdapterError> {
        let url = format!("{}/snap-manager/v1/package/overview", self.snap_engine);
        let raw = self
            .signed_get(
                &url,
                creds,
                &[
                    ("X-Security-Token", &creds.security_token),
                    ("Content-Type", "application/json"),
                ],
                "查询 CodeArts 额度",
            )
            .await?;
        if let Some(err) = raw.get("error_code").and_then(|v| v.as_str()) {
            if err != "0000" {
                let msg = raw
                    .get("error_msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知错误");
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: format!("查询 CodeArts 额度: error_code={err}（{msg}）"),
                });
            }
        }
        Ok(raw)
    }

    /// 从 `package/overview` 中解析出三个积分桶。
    fn parse_overview(data: &Value) -> Vec<QuotaBucket> {
        let mut buckets = Vec::new();
        // (JSON 键, 展示名, bucket key)
        let sections = [
            ("all_credit_package", "总积分", "total"),
            ("basic_package", "基础套餐积分", "basic"),
            ("bonus_credit_package", "赠送积分", "bonus"),
            ("ondemand_credit_package", "按需积分", "ondemand"),
        ];
        for (key, label, bucket_key) in sections {
            let Some(section) = data.get(key).filter(|v| !v.is_null()) else {
                continue;
            };
            let amount = num_field(section, "package_credit_amount");
            let used = num_field(section, "package_credit_used");
            let remain = num_field(section, "package_credit_remain");
            let (Some(total), Some(remaining)) = (amount, remain) else {
                continue;
            };
            if total <= 0.0 && remaining <= 0.0 {
                continue;
            }
            buckets.push(QuotaBucket {
                key: bucket_key.to_string(),
                label: label.to_string(),
                used: used.unwrap_or((total - remaining).max(0.0)),
                total,
                remaining,
                unit: "积分".to_string(),
                reset_at: None,
                has_total: true,
            });
        }
        buckets
    }

    /// 调 `caller-identity` 校验签名凭据是否仍然有效。
    async fn verify_identity(&self, creds: &HuaweiCreds) -> Result<Value, AdapterError> {
        let url = format!("{}/v5/caller-identity", self.iam_sts);
        self.signed_get(
            &url,
            creds,
            &[
                ("X-Security-Token", &creds.security_token),
                ("Content-Type", "application/json"),
            ],
            "校验 CodeArts 凭据",
        )
        .await
    }

    /// 只读式过期检查：不发任何网络请求，仅比对凭据自带的 `expiresAt`。
    ///
    /// **为什么 Qwitch 不主动刷新**：华为云 STS 的 refresh token 是一次性轮换的，
    /// 谁消费谁必须立刻持有新的 token。若 Qwitch 与客户端都尝试刷新，必然有一方
    /// 把对方的 token 作废，结果是用户被迫重新登录——代价远大于「点一下打开客户端」。
    /// 官方客户端在剩余 1 小时时就会自行刷新，因此这里只做检测与引导。
    ///
    /// **软过期（P2 优化）**：仅当本地 `expiresAt` 已真正越过（含 `EXPIRY_HARD_TOLERANCE_MS`
    /// 容差）时才本地拦截；处于「临期但未过期」窗口时不再硬拦，放行请求由服务端
    /// 401 确证，避免把服务端仍可用的 STS 误标为过期、减少无谓的「重新关联本机」。
    fn ensure_not_expired(cred: &CredentialEnvelope) -> Result<(), AdapterError> {
        // 真机调试旁路：设置 CODEARTS_PROBE_IGNORE_EXPIRY=1 时跳过本地过期拦截，
        // 直接让服务端判定 STS 是否仍有效（用于验证领取链路/排查误标过期）。
        if std::env::var("CODEARTS_PROBE_IGNORE_EXPIRY").as_deref() == Ok("1") {
            return Ok(());
        }
        let CredentialPayload::Token { expires_at, .. } = &cred.payload else {
            // 非 Token 形态不在此处拦截，交给后续流程处理
            return Ok(());
        };
        let Some(expires_at) = expires_at else {
            // 未记录有效期（如手工粘贴的 AK/SK）→ 不拦截，让服务端判定
            return Ok(());
        };
        let now = Utc::now().timestamp_millis();
        // 防御：即便上层漏了归一化，秒级时间戳也不能直接当毫秒比较
        let expires_at_ms = if *expires_at > 0 && *expires_at < 10_000_000_000 {
            expires_at.saturating_mul(1000)
        } else {
            *expires_at
        };
        if expires_at_ms - now <= EXPIRY_HARD_TOLERANCE_MS {
            let fmt = |ms: i64| {
                chrono::DateTime::from_timestamp_millis(ms)
                    .map(|dt| {
                        dt.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_else(|| ms.to_string())
            };
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                message: format!(
                    "{EXPIRED_HINT}（本地记录有效期 {}，当前 {}）",
                    fmt(expires_at_ms),
                    fmt(now)
                ),
            });
        }
        Ok(())
    }

    pub fn create_oauth_flow() -> Result<CodeArtsOAuthFlow, AdapterError> {
        let rng = SystemRandom::new();
        let mut verifier_raw = [0u8; 32];
        rng.fill(&mut verifier_raw)
            .map_err(|_| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "生成 CodeArts PKCE 随机串失败".into(),
            })?;
        let code_verifier = BASE64_URL.encode(verifier_raw);
        let code_challenge = BASE64_URL.encode(Sha256::digest(code_verifier.as_bytes()));

        let mut port_raw = [0u8; 2];
        rng.fill(&mut port_raw).map_err(|_| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: "生成 CodeArts 回调端口失败".into(),
        })?;
        let port = 10_000 + (u16::from_be_bytes(port_raw) % 50_000);

        let dpop_doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "生成 CodeArts DPoP 密钥对失败".into(),
            })?;
        let ticket_id = uuid::Uuid::new_v4().to_string();

        let auth_url = reqwest::Url::parse_with_params(
            &format!("{CODEARTS_PORTAL_URL}/authorize"),
            &[
                ("theme", "2"),
                ("locale", "zh-cn"),
                ("uri_scheme", CODEARTS_OAUTH_CLIENT_ID),
                ("client_id", CODEARTS_OAUTH_CLIENT_ID),
                ("port", &port.to_string()),
                ("code_challenge", &code_challenge),
                ("code_challenge_method", "SHA-256"),
                ("ticket_id", &ticket_id),
                ("plugin-name", CODEARTS_LOGIN_PLUGIN_NAME),
                ("plugin-version", CODEARTS_LOGIN_PLUGIN_VERSION),
            ],
        )
        .map(|u| u.to_string())
        .map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("构造 CodeArts 授权地址失败: {e}"),
        })?;

        Ok(CodeArtsOAuthFlow {
            auth_url,
            port,
            code_verifier,
            dpop_pkcs8: dpop_doc.as_ref().to_vec(),
        })
    }

    pub(crate) fn sign_dpop_jws(
        pkcs8: &[u8],
        htm: &str,
        htu: &str,
    ) -> Result<String, AdapterError> {
        let rng = SystemRandom::new();
        let key_pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &rng)
            .map_err(|_| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts DPoP 私钥格式无效".into(),
            })?;
        let pub_bytes = key_pair.public_key().as_ref();
        if pub_bytes.len() != 65 || pub_bytes[0] != 0x04 {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "CodeArts DPoP 公钥格式异常".into(),
            });
        }
        let x_b64 = BASE64_URL.encode(&pub_bytes[1..33]);
        let y_b64 = BASE64_URL.encode(&pub_bytes[33..65]);

        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "dpop+jwt",
            "jwk": {
                "crv": "P-256",
                "kty": "EC",
                "x": x_b64,
                "y": y_b64,
            }
        });
        let mut jti_raw = [0u8; 16];
        rng.fill(&mut jti_raw).map_err(|_| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: "生成 DPoP jti 失败".into(),
        })?;
        let payload = serde_json::json!({
            "htm": htm,
            "htu": htu,
            "iat": Utc::now().timestamp(),
            "jti": hex::encode(jti_raw),
        });

        let header_b64 =
            BASE64_URL.encode(
                serde_json::to_vec(&header).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: e.to_string(),
                })?,
            );
        let payload_b64 =
            BASE64_URL.encode(
                serde_json::to_vec(&payload).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: e.to_string(),
                })?,
            );
        let signing_input = format!("{header_b64}.{payload_b64}");
        let sig =
            key_pair
                .sign(&rng, signing_input.as_bytes())
                .map_err(|_| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: "CodeArts DPoP 签名失败".into(),
                })?;
        Ok(format!(
            "{signing_input}.{}",
            BASE64_URL.encode(sig.as_ref())
        ))
    }

    async fn exchange_sts_token(
        &self,
        form: &[(&str, &str)],
        dpop_pkcs8: &[u8],
    ) -> Result<Value, AdapterError> {
        let url = format!("{}/v1/oauth2/tokens", self.iam_sts);
        let dpop = Self::sign_dpop_jws(dpop_pkcs8, "POST", &url)?;

        let resp = match self
            .http
            .post(&url)
            .header(ACCEPT, "application/json")
            .header("DPoP", dpop)
            .form(form)
            .send()
            .await
        {
            Ok(r) => r,
            Err(first_err) => {
                if url.contains(".myhuaweicloud.com") {
                    let cn_url = url.replace(".myhuaweicloud.com", ".myhuaweicloud.cn");
                    let cn_dpop = Self::sign_dpop_jws(dpop_pkcs8, "POST", &cn_url)?;
                    self.http
                        .post(&cn_url)
                        .header(ACCEPT, "application/json")
                        .header("DPoP", cn_dpop)
                        .form(form)
                        .send()
                        .await
                        .map_err(|e| AdapterError::Unified {
                            code: UnifiedErrorCode::NetworkError,
                            message: format!("请求 CodeArts STS 令牌接口失败: {}", e.without_url()),
                        })?
                } else {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::NetworkError,
                        message: format!(
                            "请求 CodeArts STS 令牌接口失败: {}",
                            first_err.without_url()
                        ),
                    });
                }
            }
        };

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let has_creds = body.get("credentials").is_some_and(Value::is_object);

        if !status.is_success() || !has_creds {
            let err_code = body
                .get("error")
                .or_else(|| body.get("error_code"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_dead = err_code.eq_ignore_ascii_case("invalid_grant")
                || text.contains("ExpiredRefreshToken")
                || text.contains("STS5.1806")
                || text.to_ascii_lowercase().contains("invalid refresh token")
                || text
                    .to_ascii_lowercase()
                    .contains("refresh token has been used");
            if is_dead {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: "CodeArts 独立 OAuth 刷新令牌已失效，请重新授权".into(),
                });
            }
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!(
                    "CodeArts STS 令牌交换失败 (HTTP {}): {}",
                    status.as_u16(),
                    text.chars().take(160).collect::<String>()
                ),
            });
        }
        Ok(body)
    }

    /// 用 `caller-identity` 的返回补齐真实身份。
    ///
    /// 华为云 STS 的授权码换 token 响应**通常不含用户身份**，历史实现直接退化成
    /// `codearts_<hash(一次性 accessKey)>` 作为 uid：既与本机导入的 `userId` 对不上，
    /// 又因为 accessKey 每次授权都变，同一账号会被反复新建（用户看到「同一个账号两条」）。
    /// 这里用稳定身份（IAM user id）覆盖 uid，并补上显示名与 domain id。
    fn apply_identity(env: &mut CredentialEnvelope, identity: &Value) {
        let user = identity.get("user");
        let pick = |v: Option<&Value>| {
            v.and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let uid = pick(user.and_then(|u| u.get("id")))
            .or_else(|| pick(identity.get("principal_id")))
            .or_else(|| pick(identity.get("account_id")));
        if let Some(uid) = uid {
            env.uid = uid;
        }
        let name = pick(user.and_then(|u| u.get("name")));
        if env
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
        {
            if let Some(name) = name {
                env.display_name = Some(name);
            }
        }
        if let Some(domain_id) = pick(user.and_then(|u| u.get("domain_id"))) {
            if let CredentialPayload::Token { extra, .. } = &mut env.payload {
                extra.insert(EXTRA_DOMAIN_ID.to_string(), domain_id);
            }
        }
    }

    fn envelope_from_sts_response(
        body: &Value,
        code_verifier: &str,
        dpop_pkcs8_b64: &str,
        fallback_refresh_token: Option<&str>,
        prev_uid: Option<&str>,
        prev_name: Option<&str>,
        prev_domain_id: Option<&str>,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let creds = body
            .get("credentials")
            .and_then(|v| v.as_object())
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts STS 响应缺少 credentials".into(),
            })?;
        let get_str = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|k| {
                creds
                    .get(*k)
                    .or_else(|| body.get(*k))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
        };
        let access_key = get_str(&["access_key_id", "accessKey", "access"]).ok_or_else(|| {
            AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts STS 响应缺少 access_key_id".into(),
            }
        })?;
        let secret_key =
            get_str(&["secret_access_key", "secretKey", "secret"]).ok_or_else(|| {
                AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "CodeArts STS 响应缺少 secret_access_key".into(),
                }
            })?;
        let security_token =
            get_str(&["security_token", "securityToken"]).ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts STS 响应缺少 security_token".into(),
            })?;
        let refresh_token = get_str(&["refresh_token", "refreshToken"])
            .or_else(|| fallback_refresh_token.map(str::to_string));

        let expires_at = creds
            .get("expires_at")
            .or_else(|| creds.get("expiresAt"))
            .or_else(|| body.get("expires_at"))
            .and_then(|v| {
                v.as_i64()
                    .map(|n| {
                        if n < 10_000_000_000 {
                            n.saturating_mul(1000)
                        } else {
                            n
                        }
                    })
                    .or_else(|| {
                        v.as_str()
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok())
                            .map(|dt| dt.timestamp_millis())
                    })
            });

        let uid = get_str(&["principal_id", "user_id", "userId"])
            .or_else(|| prev_uid.map(str::to_string))
            .unwrap_or_else(|| hash_fallback_uid("codearts", access_key.as_bytes()));
        let display_name = get_str(&["user_name", "userName", "principal_name"])
            .or_else(|| prev_name.map(str::to_string));
        let domain_id =
            get_str(&["domain_id", "domainId"]).or_else(|| prev_domain_id.map(str::to_string));

        let mut extra = HashMap::new();
        extra.insert(EXTRA_ACCESS_KEY.to_string(), access_key);
        extra.insert(EXTRA_SECRET_KEY.to_string(), secret_key);
        extra.insert(EXTRA_SECURITY_TOKEN.to_string(), security_token);
        extra.insert(EXTRA_REGION.to_string(), DEFAULT_REGION.to_string());
        extra.insert(
            EXTRA_CODEARTS_MODE.to_string(),
            "isolated_oauth".to_string(),
        );
        extra.insert(EXTRA_CODE_VERIFIER.to_string(), code_verifier.to_string());
        extra.insert(EXTRA_DPOP_PKCS8_B64.to_string(), dpop_pkcs8_b64.to_string());
        if let Some(d) = domain_id {
            extra.insert(EXTRA_DOMAIN_ID.to_string(), d);
        }

        Ok(CredentialEnvelope {
            tool_id: "codearts".to_string(),
            uid,
            display_name,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: String::new(),
                refresh_token,
                expires_at,
                extra,
            },
        })
    }

    pub async fn exchange_authorization_code(
        &self,
        code: &str,
        code_verifier: &str,
        port: u16,
        dpop_pkcs8: &[u8],
    ) -> Result<CredentialEnvelope, AdapterError> {
        let redirect_uri = format!("http://127.0.0.1:{port}{CODEARTS_OAUTH_REDIRECT_PATH}");
        let body = self
            .exchange_sts_token(
                &[
                    ("client_id", CODEARTS_OAUTH_CLIENT_ID),
                    ("code", code),
                    ("code_verifier", code_verifier),
                    ("grant_type", "authorization_code"),
                    ("redirect_uri", &redirect_uri),
                ],
                dpop_pkcs8,
            )
            .await?;
        let dpop_b64 = BASE64.encode(dpop_pkcs8);
        let mut env = Self::envelope_from_sts_response(
            &body,
            code_verifier,
            &dpop_b64,
            None,
            None,
            None,
            None,
        )?;
        // STS 响应没有用户身份时，uid 会退化成基于一次性 accessKey 的哈希 →
        // 与本机导入对不上、重复授权还会再建一条。这里换成 IAM 的稳定身份。
        if CredentialEnvelope::is_fallback_uid(&env.uid) || env.uid.trim().is_empty() {
            if let Ok(creds) = HuaweiCreds::from_envelope(&env) {
                if let Ok(identity) = self.verify_identity(&creds).await {
                    Self::apply_identity(&mut env, &identity);
                }
            }
        }
        Ok(env)
    }

    async fn refresh_credential_inner(
        &self,
        cred: &CredentialEnvelope,
        force: bool,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        let CredentialPayload::Token {
            refresh_token,
            expires_at,
            extra,
            ..
        } = &cred.payload
        else {
            return Ok(None);
        };
        // 防互踢保护：仅独立 OAuth（isolated_oauth）且自带 PKCE verifier + DPoP 私钥的账号允许走网络刷新；
        // 本机客户端导入的账号绝不消费客户端一次性 refresh token，只做只读回吸本机客户端磁盘中已更新的 STS 凭据。
        if extra.get(EXTRA_CODEARTS_MODE).map(String::as_str) != Some("isolated_oauth") {
            let now_ms = Utc::now().timestamp_millis();
            let expired_or_near =
                expires_at.is_none_or(|exp| exp - now_ms < CODEARTS_REFRESH_LEAD_MS);
            if !force && !expired_or_near {
                return Ok(None);
            }
            let Some(host) = self.host.as_deref() else {
                return Ok(None);
            };
            let Ok(incoming) = self.import_from_local(host).await else {
                return Ok(None);
            };
            let stored_real =
                !cred.uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&cred.uid);
            let incoming_real = !incoming.uid.trim().is_empty()
                && !CredentialEnvelope::is_fallback_uid(&incoming.uid);
            if stored_real && incoming_real && cred.uid != incoming.uid {
                return Ok(None);
            }
            let (old_tok, old_exp) = match &cred.payload {
                CredentialPayload::Token {
                    expires_at, extra, ..
                } => (extra.get(EXTRA_SECURITY_TOKEN).cloned(), *expires_at),
                _ => (None, None),
            };
            let (new_tok, new_exp) = match &incoming.payload {
                CredentialPayload::Token {
                    expires_at, extra, ..
                } => (extra.get(EXTRA_SECURITY_TOKEN).cloned(), *expires_at),
                _ => (None, None),
            };
            let fresher_exp = match (old_exp, new_exp) {
                (Some(o), Some(n)) => n > o + 60_000,
                (None, Some(_)) => true,
                _ => false,
            };
            let tok_changed = new_tok.is_some() && new_tok != old_tok;
            if fresher_exp || tok_changed {
                return Ok(Some(incoming));
            }
            return Ok(None);
        }
        let Some(rt) = refresh_token
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        let Some(code_verifier) = extra
            .get(EXTRA_CODE_VERIFIER)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        let Some(dpop_b64) = extra
            .get(EXTRA_DPOP_PKCS8_B64)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        let Ok(dpop_pkcs8) = BASE64.decode(dpop_b64) else {
            return Ok(None);
        };

        let now_ms = Utc::now().timestamp_millis();
        if !force {
            if let Some(exp) = expires_at {
                if exp - now_ms >= CODEARTS_REFRESH_LEAD_MS {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }

        let body = self
            .exchange_sts_token(
                &[
                    ("client_id", CODEARTS_OAUTH_CLIENT_ID),
                    ("code_verifier", code_verifier),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", rt.as_str()),
                ],
                &dpop_pkcs8,
            )
            .await?;

        let updated = Self::envelope_from_sts_response(
            &body,
            code_verifier,
            dpop_b64,
            Some(&rt),
            Some(&cred.uid),
            cred.display_name.as_deref(),
            extra.get(EXTRA_DOMAIN_ID).map(String::as_str),
        )?;
        Ok(Some(updated))
    }
}

pub struct CodeArtsOAuthFlow {
    pub auth_url: String,
    pub port: u16,
    pub code_verifier: String,
    pub dpop_pkcs8: Vec<u8>,
}

impl Default for CodeArtsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// RFC 3986 百分号编码（华为云规范化要求：仅保留 `A-Za-z0-9-_.~`）。
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn non_empty(obj: &Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn num_field(obj: &Value, key: &str) -> Option<f64> {
    let v = obj.get(key)?;
    let n = v.as_f64().or_else(|| {
        v.as_str()
            .and_then(|s| s.trim().replace(',', "").parse().ok())
    })?;
    n.is_finite().then_some(n)
}

// ---------------------------------------------------------------------------
// 活动福利（/v1/ops/*）字段解析辅助
// ---------------------------------------------------------------------------

/// 归一化活动类型：兼容 `DAILY_CLAIM`（大写）与 `daily_claim`（小写）。
fn activity_type(item: &Value) -> String {
    item.get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 是否为「每日签到」活动。
///
/// 真机 delivery 实测：每日签到活动的 `type == "USER_LOGIN"`（触发事件
/// `extra.triggerEvent == "user.login"`），如「每日签到领100积分」。历史/其它
/// 区域也可能用 `daily_claim`。这里两者都认，并以 triggerEvent 兜底。
fn is_daily_checkin(item: &Value) -> bool {
    let ty = activity_type(item);
    if ty == "user_login" || ty == "daily_claim" || ty == "daily_signin" {
        return true;
    }
    item.get("extra")
        .and_then(|e| e.get("triggerEvent"))
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("user.login"))
        .unwrap_or(false)
}

/// 活动是否已领：`claimed==true` 或 `status ∈ {CLAIMED,CONFIRMED,CONSUMED}`。
fn activity_claimed(item: &Value) -> bool {
    if matches!(item.get("claimed").and_then(|v| v.as_bool()), Some(true)) {
        return true;
    }
    match item.get("status").and_then(|v| v.as_str()) {
        Some(s) => matches!(
            s.to_ascii_uppercase().as_str(),
            "CLAIMED" | "CONFIRMED" | "CONSUMED"
        ),
        None => false,
    }
}

/// 取活动的 campaignId（客户端用 `campaignId` 或 `campaign_id`）。
fn activity_campaign_id(item: &Value) -> String {
    // campaignId 真机为数字（如 34），也兼容字符串形态。
    for key in ["campaignId", "campaign_id", "id"] {
        if let Some(v) = item.get(key) {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
            if let Some(n) = v.as_i64() {
                return n.to_string();
            }
            if let Some(n) = v.as_u64() {
                return n.to_string();
            }
        }
    }
    String::new()
}

/// 从领取响应里提取 `userBenefitId`（客户端取 `data.id ?? data.userBenefitId`）。
fn extract_user_benefit_id(resp: &Value) -> Option<String> {
    let data = resp.get("data").unwrap_or(resp);
    non_empty(data, "userBenefitId")
        .or_else(|| non_empty(data, "user_benefit_id"))
        .or_else(|| non_empty(data, "id"))
}

/// 毫秒时间戳（用于幂等键）。
fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// 生成一个 UUID v4 形式字符串（用于 `x-snap-traceid`），避免引入新依赖。
fn uuid_like() -> String {
    let mut b = [0u8; 16];
    SystemRandom::new().fill(&mut b).expect("系统随机源不可用");
    // 版本位/变体位，凑成合法 v4 形态
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14],
        b[15]
    )
}

#[async_trait]
impl ToolAdapter for CodeArtsAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let session = Self::load_session(host)?;
        let envelope = Self::envelope_from_session(&session)?;
        // 导入时立刻验一次签名凭据，避免存入已失效的 AK/SK。
        // 若本地 session 已过期，先给出可执行的指引而不是让请求打空。
        Self::ensure_not_expired(&envelope)?;
        let creds = HuaweiCreds::from_envelope(&envelope)?;
        self.verify_identity(&creds).await?;
        Ok(envelope)
    }

    async fn parse_manual_token(
        &self,
        raw_input: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        // 手工粘贴：支持 `AK:SK:STS` 或 JSON 两种形态
        let trimmed = raw_input.trim();
        let mut extra = HashMap::new();
        let (access_key, secret_key, security_token) = if trimmed.starts_with('{') {
            let v: Value = serde_json::from_str(trimmed).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("CodeArts 凭据 JSON 解析失败: {e}"),
            })?;
            (
                non_empty(&v, "accessKey").or_else(|| non_empty(&v, "access_key")),
                non_empty(&v, "secretKey").or_else(|| non_empty(&v, "secret_key")),
                non_empty(&v, "securityToken").or_else(|| non_empty(&v, "security_token")),
            )
        } else {
            let parts: Vec<&str> = trimmed.split(':').map(str::trim).collect();
            if parts.len() < 3 {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "CodeArts 凭据格式应为 AK:SK:STS 或 JSON".into(),
                });
            }
            (
                Some(parts[0].to_string()),
                Some(parts[1].to_string()),
                Some(parts[2].to_string()),
            )
        };

        let access_key = access_key.ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "缺少 accessKey".into(),
        })?;
        let secret_key = secret_key.ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "缺少 secretKey".into(),
        })?;
        let security_token = security_token.ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "缺少 securityToken".into(),
        })?;

        extra.insert(EXTRA_ACCESS_KEY.to_string(), access_key.clone());
        extra.insert(EXTRA_SECRET_KEY.to_string(), secret_key);
        extra.insert(EXTRA_SECURITY_TOKEN.to_string(), security_token);
        extra.insert(EXTRA_REGION.to_string(), DEFAULT_REGION.to_string());

        let probe = CredentialEnvelope {
            tool_id: "codearts".to_string(),
            uid: hash_fallback_uid("codearts", access_key.as_bytes()),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: String::new(),
                refresh_token: None,
                expires_at: None,
                extra: extra.clone(),
            },
        };
        let creds = HuaweiCreds::from_envelope(&probe)?;
        let identity = self.verify_identity(&creds).await?;

        let uid = identity
            .get("principal_id")
            .and_then(json_uid_to_string)
            .unwrap_or(probe.uid);
        let display_name = identity
            .get("principal_urn")
            .and_then(|v| v.as_str())
            .and_then(|urn| urn.rsplit(':').next())
            .map(str::to_string)
            .filter(|s| !s.is_empty());
        if let Some(d) = non_empty(&identity, "account_id") {
            extra.insert(EXTRA_DOMAIN_ID.to_string(), d);
        }

        Ok(CredentialEnvelope {
            tool_id: "codearts".to_string(),
            uid,
            display_name,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: String::new(),
                refresh_token: None,
                expires_at: None,
                extra,
            },
        })
    }

    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError> {
        // 过期即拦截，避免发出注定 401 的请求并污染日志
        Self::ensure_not_expired(cred)?;
        let creds = HuaweiCreds::from_envelope(cred)?;
        let overview = self.fetch_package_overview(&creds).await?;
        let buckets = Self::parse_overview(&overview);
        let plan_name = if buckets.iter().any(|b| b.key == "basic") {
            "基础套餐"
        } else {
            "免费"
        };
        // 从活动列表推断每日签到状态，供调度器决定是否领取：
        //   有未领的每日签到 → Available；有但已领 → Claimed；无该活动 → Unsupported。
        // 活动接口失败不应拖垮额度查询：降级为 Unknown（调度器会跳过，防误发请求）。
        let claim_status = match self.fetch_delivery_items(&creds).await {
            Ok(items) => {
                let daily: Vec<&Value> = items.iter().filter(|it| is_daily_checkin(it)).collect();
                if daily.is_empty() {
                    ClaimStatus::Unsupported
                } else if daily.iter().any(|it| {
                    !activity_claimed(it)
                        && !matches!(it.get("claimable").and_then(|v| v.as_bool()), Some(false))
                }) {
                    ClaimStatus::Available
                } else {
                    ClaimStatus::Claimed
                }
            }
            Err(_) => ClaimStatus::Unknown,
        };
        Ok(QuotaSnapshot {
            plan_name: Some(plan_name.to_string()),
            claim_status,
            buckets,
            fetched_at: Utc::now().to_rfc3339(),
        })
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        _precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        // 码道「活动福利」体系确有每日签到（真机 type=USER_LOGIN / triggerEvent=user.login），
        // 走纯服务端接口：
        //   GET  {policyCenterDomain}/v1/ops/delivery?channel=DESKTOP      // 拉活动列表
        //   POST {policyCenterDomain}/v1/ops/claim   {campaignId,idempotentKey,channel}  // 领取
        //   POST {policyCenterDomain}/v1/ops/confirm {userBenefitId}       // 领取后确认
        // 与豆包不同：不需要桌面端在登、不需要切号，只需 STS 签名即可。
        // 过期即拦截，避免发出注定 401 的请求（与 inspect_quota 一致）。
        Self::ensure_not_expired(cred)?;
        let creds = HuaweiCreds::from_envelope(cred)?;
        let base = self.policy_center_base();

        // 1) 拉活动列表
        let items = self.fetch_delivery_items(&creds).await?;

        // 2) 找未领取的每日签到活动
        let daily = items.iter().find(|it| {
            is_daily_checkin(it)
                && !activity_claimed(it)
                && !matches!(it.get("claimable").and_then(|v| v.as_bool()), Some(false))
        });

        let Some(activity) = daily else {
            // 没有可领的每日签到：可能今日已领，或该账号无此活动
            let any_daily = items.iter().any(is_daily_checkin);
            let msg = if any_daily {
                "码道每日签到：今日已领取"
            } else {
                "码道当前无可领取的每日签到活动"
            };
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: msg.into(),
                updated_quota: None,
            });
        };

        let campaign_id = activity_campaign_id(activity);
        if campaign_id.is_empty() {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "码道每日签到活动缺少 campaignId，已跳过".into(),
                updated_quota: None,
            });
        }
        let reward = activity
            .get("benefitAmount")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // 3) 领取（带幂等键，安全重试不重复发放）
        let idem = format!("claim_{}_{}", campaign_id, now_ms());
        let claim_body = serde_json::json!({
            "campaignId": campaign_id,
            "idempotentKey": idem,
            "channel": "DESKTOP",
        });
        let claim_headers = Self::benefit_extra_headers(&creds);
        let claim = self
            .signed_post_json(
                &format!("{base}/v1/ops/claim"),
                &creds,
                &claim_headers,
                &claim_body,
                "领取码道每日签到",
            )
            .await?;

        // 4) 领取成功后确认（部分活动返回 userBenefitId 需二次确认）
        let user_benefit_id = extract_user_benefit_id(&claim);
        let mut confirm_note = String::new();
        if let Some(ubid) = user_benefit_id.as_deref().filter(|s| !s.is_empty()) {
            let confirm_body = serde_json::json!({ "userBenefitId": ubid });
            let confirm_headers = Self::benefit_extra_headers(&creds);
            match self
                .signed_post_json(
                    &format!("{base}/v1/ops/confirm"),
                    &creds,
                    &confirm_headers,
                    &confirm_body,
                    "确认码道每日签到",
                )
                .await
            {
                Ok(_) => {}
                Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    ..
                }) => {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::AuthExpired,
                        message: "码道每日签到领取成功但确认时凭据过期，请稍后重试".into(),
                    })
                }
                Err(e) => confirm_note = format!("（确认阶段异常，可稍后重试：{e}）"),
            }
        }

        let summary = if reward > 0 {
            format!("码道每日签到成功，领取 {reward} 积分{confirm_note}")
        } else {
            format!("码道每日签到成功{confirm_note}")
        };
        Ok(ClaimResult {
            claimed: true,
            reward_summary: summary,
            updated_quota: None,
        })
    }

    async fn refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        self.refresh_credential_inner(cred, false).await
    }

    async fn force_refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        self.refresh_credential_inner(cred, true).await
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError> {
        // 切号 = 把目标账号的 `{session, loginContext}` 用 safeStorage 重新加密写回
        // secretStorage。字符串层面的加解密与客户端等价（同为 AES-128-CBC +
        // Keychain 派生的同一把密钥），因此不再需要借助客户端登录流程。
        //
        // 前提：必须持有**该账号在本机登录时**留下的完整会话快照 —— 独立 OAuth
        // 授权只有 AK/SK/STS，拼不出客户端认可的 loginContext。
        let CredentialPayload::Token { extra, .. } = &cred.payload else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::UnsupportedCapability,
                message: "码道 CodeArts 暂不支持该凭据形态的本地切号".into(),
            });
        };
        let Some(snapshot_b64) = extra.get(EXTRA_SESSION_SNAPSHOT) else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::UnsupportedCapability,
                message: "该账号没有本机会话快照（独立 OAuth 授权导入），无法写回客户端。\
                          请先在码道客户端登录该账号，再用「本机导入」补一次快照"
                    .into(),
            });
        };
        Self::ensure_not_expired(cred)?;

        let raw = BASE64
            .decode(snapshot_b64.trim())
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("码道会话快照不是合法 base64: {e}"),
            })?;
        let mut snapshot: Value =
            serde_json::from_slice(&raw).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("码道会话快照结构解析失败: {e}"),
            })?;

        // 用当前凭据里的临时凭据覆盖快照内的同名字段：快照是导入当时的，
        // STS 会轮换；不覆盖的话写回去的会话可能已经过期。
        if let Some(session) = snapshot.get_mut("session").and_then(|v| v.as_object_mut()) {
            for (key, extra_key) in [
                ("accessKey", EXTRA_ACCESS_KEY),
                ("secretKey", EXTRA_SECRET_KEY),
                ("securityToken", EXTRA_SECURITY_TOKEN),
            ] {
                if let Some(value) = extra.get(extra_key) {
                    session.insert(key.to_string(), Value::String(value.clone()));
                }
            }
        }

        let patched = serde_json::to_vec(&snapshot).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("码道会话快照序列化失败: {e}"),
        })?;

        let password = Self::macos_safe_storage_password("codearts")?;
        let envelope = safe_storage::encrypt_safe_storage(&patched, &password, "CodeArts")?;
        let encoded = BASE64.encode(&envelope);

        let dir = Self::resolve_secret_dir(host)?;
        let file = dir.join(Self::secret_file_name(&format!(
            "authSession:{DEFAULT_REGION}"
        )));
        crate::adapter::write_private_file_atomic(&file, encoded.as_bytes())
    }

    async fn backup_session(&self, host: &dyn PlatformHost) -> Result<SessionBackup, AdapterError> {
        let dir = Self::resolve_secret_dir(host)?;
        let file = dir.join(Self::secret_file_name(&format!(
            "authSession:{DEFAULT_REGION}"
        )));
        crate::adapter::backup_files(&[file])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{KeyExpectation, ScopedPlatformHost};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use std::path::Path;

    struct DummyHost {
        root: PathBuf,
    }

    #[async_trait]
    impl PlatformHost for DummyHost {
        fn qwitch_data_dir(&self) -> Result<PathBuf, crate::platform::HostError> {
            Ok(self.root.clone())
        }
        fn client_config_dir(&self, _t: &str) -> Result<PathBuf, crate::platform::HostError> {
            Ok(self.root.clone())
        }
        fn detect_client_executable(&self, _t: &str) -> Option<PathBuf> {
            None
        }
        fn is_client_running(&self, _t: &str) -> Result<bool, crate::platform::HostError> {
            Ok(false)
        }
        async fn terminate_client_gracefully(
            &self,
            _t: &str,
            _ms: u64,
        ) -> Result<(), crate::platform::HostError> {
            Ok(())
        }
        fn launch_client(
            &self,
            _e: &Path,
            _a: &[String],
        ) -> Result<(), crate::platform::HostError> {
            Ok(())
        }
        fn get_or_create_master_key(
            &self,
            _e: &KeyExpectation,
        ) -> Result<[u8; 32], crate::platform::HostError> {
            Ok([0u8; 32])
        }
        fn set_launch_at_login(&self, _e: bool) -> Result<(), String> {
            Ok(())
        }
    }

    fn cred_with(ak: &str, sk: &str, sts: &str) -> CredentialEnvelope {
        let mut extra = HashMap::new();
        extra.insert(EXTRA_ACCESS_KEY.to_string(), ak.to_string());
        extra.insert(EXTRA_SECRET_KEY.to_string(), sk.to_string());
        extra.insert(EXTRA_SECURITY_TOKEN.to_string(), sts.to_string());
        extra.insert(EXTRA_REGION.to_string(), DEFAULT_REGION.to_string());
        CredentialEnvelope {
            tool_id: "codearts".to_string(),
            uid: "u1".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: String::new(),
                refresh_token: None,
                expires_at: None,
                extra,
            },
        }
    }

    #[test]
    fn manifest_declares_daily_claim() {
        let a = CodeArtsAdapter::new();
        let m = a.manifest();
        assert_eq!(m.id, "codearts");
        assert!(m.capabilities.supports_quota_query);
        assert!(
            m.capabilities.supports_daily_claim,
            "CodeArts「活动福利」确有每日签到（/v1/ops/*），capability 必须为 true"
        );
        assert_eq!(
            m.capabilities.supports_client_switch,
            cfg!(target_os = "macos"),
            "码道切号依赖 safeStorage 重加密，仅 macOS 可复现"
        );
    }

    #[test]
    fn secret_file_name_is_sha256_of_key() {
        use sha2::{Digest, Sha256};
        let key = "authSession:cn-north-4";
        let expected = format!("{}.enc", hex::encode(Sha256::digest(key.as_bytes())));
        assert_eq!(CodeArtsAdapter::secret_file_name(key), expected);
    }

    #[test]
    fn parse_overview_extracts_three_credit_buckets() {
        let data = serde_json::json!({
            "all_credit_package": {
                "package_credit_amount": 4500.0,
                "package_credit_used": 100.0,
                "package_credit_remain": 4400.0
            },
            "basic_package": {
                "package_credit_amount": 500.0,
                "package_credit_used": 100.0,
                "package_credit_remain": 400.0
            },
            "bonus_credit_package": {
                "package_credit_amount": 4000.0,
                "package_credit_used": 0.0,
                "package_credit_remain": 4000.0
            },
            "ondemand_credit_package": null
        });
        let buckets = CodeArtsAdapter::parse_overview(&data);
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[0].key, "total");
        assert_eq!(buckets[0].remaining, 4400.0);
        assert_eq!(buckets[1].key, "basic");
        assert_eq!(buckets[2].key, "bonus");
        assert_eq!(buckets[2].total, 4000.0);
        assert!(buckets.iter().all(|b| b.unit == "积分"));
    }

    #[test]
    fn parse_overview_skips_zero_and_missing_sections() {
        let data = serde_json::json!({
            "all_credit_package": {
                "package_credit_amount": 0.0,
                "package_credit_remain": 0.0
            },
            "basic_package": null
        });
        assert!(CodeArtsAdapter::parse_overview(&data).is_empty());
    }

    #[test]
    fn percent_encode_matches_rfc3986() {
        assert_eq!(percent_encode("abc-_.~"), "abc-_.~");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("a/b"), "a%2Fb");
        assert_eq!(percent_encode("中"), "%E4%B8%AD");
    }

    #[tokio::test]
    async fn inspect_quota_derives_claim_status_from_delivery() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        // 活动列表：每日签到今日已领
        Mock::given(method("GET"))
            .and(path("/v1/ops/delivery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "items": [
                    { "campaignId": "cmp_daily", "type": "daily_claim", "status": "CLAIMED" }
                ]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/snap-manager/v1/package/overview"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "all_credit_package": {
                    "package_credit_amount": 4500.0,
                    "package_credit_used": 0.0,
                    "package_credit_remain": 4500.0
                },
                "basic_package": {
                    "package_credit_amount": 500.0,
                    "package_credit_used": 0.0,
                    "package_credit_remain": 500.0
                },
                "bonus_credit_package": {
                    "package_credit_amount": 4000.0,
                    "package_credit_used": 0.0,
                    "package_credit_remain": 4000.0
                }
            })))
            .mount(&server)
            .await;

        let q = adapter
            .inspect_quota(&cred_with("AK", "SK", "STS"))
            .await
            .expect("inspect");
        assert_eq!(q.claim_status, ClaimStatus::Claimed);
        assert_eq!(q.buckets.len(), 3);
        assert_eq!(q.plan_name.as_deref(), Some("基础套餐"));

        // 必须带上签名头（额度请求）
        let reqs = server.received_requests().await.unwrap();
        let overview_req = reqs
            .iter()
            .find(|r| r.url.path() == "/snap-manager/v1/package/overview")
            .expect("应有额度请求");
        let auth = overview_req
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(auth.starts_with("SDK-HMAC-SHA256 Access=AK"), "auth={auth}");
        assert!(auth.contains("SignedHeaders="));
        assert!(auth.contains("Signature="));
        assert_eq!(overview_req.headers.get("x-security-token").unwrap(), "STS");
        assert!(overview_req.headers.contains_key("x-sdk-date"));
    }

    #[tokio::test]
    async fn signature_failure_maps_to_auth_expired() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        Mock::given(method("GET"))
            .and(path("/snap-manager/v1/package/overview"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error_code": "APIG.0301",
                "error_msg": "Incorrect IAM authentication information: verify ak sk signature fail"
            })))
            .mount(&server)
            .await;

        let err = adapter
            .inspect_quota(&cred_with("AK", "SK", "STS"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn claim_daily_claims_unclaimed_daily_and_confirms() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        // 1) 活动列表：一条未领的每日签到
        Mock::given(method("GET"))
            .and(path("/v1/ops/delivery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "items": [
                    { "campaignId": "cmp_daily_1", "type": "DAILY_CLAIM",
                      "status": "ENTRY", "claimable": true, "benefitAmount": 100 }
                ]}
            })))
            .mount(&server)
            .await;
        // 2) 领取：返回 userBenefitId
        Mock::given(method("POST"))
            .and(path("/v1/ops/claim"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "userBenefitId": "ub_123" }
            })))
            .mount(&server)
            .await;
        // 3) 确认
        Mock::given(method("POST"))
            .and(path("/v1/ops/confirm"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": {} })),
            )
            .mount(&server)
            .await;

        let res = adapter
            .claim_daily(&cred_with("AK", "SK", "STS"), None)
            .await
            .expect("claim ok");
        assert!(res.claimed);
        assert!(res.reward_summary.contains("100"), "{}", res.reward_summary);

        // 三个接口都被访问
        let reqs = server.received_requests().await.unwrap();
        let paths: Vec<String> = reqs.iter().map(|r| r.url.path().to_string()).collect();
        assert!(paths.iter().any(|p| p == "/v1/ops/delivery"));
        assert!(paths.iter().any(|p| p == "/v1/ops/claim"));
        assert!(paths.iter().any(|p| p == "/v1/ops/confirm"));
        // 签名头存在
        let claim_req = reqs
            .iter()
            .find(|r| r.url.path() == "/v1/ops/claim")
            .unwrap();
        assert!(claim_req.headers.get("authorization").is_some());
        assert_eq!(claim_req.headers.get("x-security-token").unwrap(), "STS");
        assert_eq!(claim_req.headers.get("agent-type").unwrap(), "PromptCenter");
    }

    #[tokio::test]
    async fn claim_daily_claims_user_login_campaign_with_numeric_id() {
        // 真机 delivery 实测结构：每日签到 type=USER_LOGIN、campaignId 为数字、
        // status=ELIGIBLE、claimable=true、extra.triggerEvent=user.login。
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        Mock::given(method("GET"))
            .and(path("/v1/ops/delivery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "channel": "DESKTOP", "items": [
                    { "campaignId": 39, "type": "STUDENT_CERTIFIED",
                      "status": null, "claimable": false, "benefitAmount": 5000,
                      "extra": { "triggerEvent": "student.certified" } },
                    { "campaignId": 34, "type": "USER_LOGIN",
                      "status": "ELIGIBLE", "claimable": true, "benefitAmount": 100,
                      "pendingCount": 1, "title": "每日签到领100积分",
                      "extra": { "triggerEvent": "user.login" } }
                ]}
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/ops/claim"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "userBenefitId": "ub_34" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/ops/confirm"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": {} })),
            )
            .mount(&server)
            .await;

        let res = adapter
            .claim_daily(&cred_with("AK", "SK", "STS"), None)
            .await
            .expect("claim ok");
        assert!(res.claimed);
        assert!(res.reward_summary.contains("100"), "{}", res.reward_summary);

        // claim 请求体应带数字 campaignId 的字符串化 "34"
        let reqs = server.received_requests().await.unwrap();
        let claim_req = reqs
            .iter()
            .find(|r| r.url.path() == "/v1/ops/claim")
            .expect("claim called");
        let body: Value = serde_json::from_slice(claim_req.body.as_ref()).unwrap();
        assert_eq!(body.get("campaignId").and_then(|v| v.as_str()), Some("34"));
        assert_eq!(
            body.get("channel").and_then(|v| v.as_str()),
            Some("DESKTOP")
        );
    }

    #[tokio::test]
    async fn claim_daily_reports_already_claimed_without_claim_call() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        Mock::given(method("GET"))
            .and(path("/v1/ops/delivery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "items": [
                    { "campaignId": "cmp_daily_1", "type": "DAILY_CLAIM", "status": "CLAIMED" }
                ]}
            })))
            .mount(&server)
            .await;

        let res = adapter
            .claim_daily(&cred_with("AK", "SK", "STS"), None)
            .await
            .expect("ok");
        assert!(!res.claimed);
        assert!(
            res.reward_summary.contains("已领取"),
            "{}",
            res.reward_summary
        );

        let reqs = server.received_requests().await.unwrap();
        assert!(
            !reqs.iter().any(|r| r.url.path() == "/v1/ops/claim"),
            "已领时不应发起 claim 请求"
        );
    }

    #[tokio::test]
    async fn claim_daily_skips_when_no_daily_activity() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        Mock::given(method("GET"))
            .and(path("/v1/ops/delivery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "items": [
                    { "campaignId": "cmp_stu", "type": "student_certify", "status": "ENTRY" }
                ]}
            })))
            .mount(&server)
            .await;

        let res = adapter
            .claim_daily(&cred_with("AK", "SK", "STS"), None)
            .await
            .expect("ok");
        assert!(!res.claimed);
    }

    #[tokio::test]
    async fn business_error_code_surfaces_as_network_error() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        Mock::given(method("GET"))
            .and(path("/snap-manager/v1/package/overview"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error_code": "4000",
                "error_msg": "invalid channel, group not found",
                "result": null
            })))
            .mount(&server)
            .await;

        let err = adapter
            .inspect_quota(&cred_with("AK", "SK", "STS"))
            .await
            .unwrap_err();
        match err {
            AdapterError::Unified { code, message } => {
                assert_eq!(code, UnifiedErrorCode::NetworkError);
                assert!(message.contains("4000"), "{message}");
            }
        }
    }

    #[tokio::test]
    async fn apply_session_reports_unsupported() {
        let root = std::env::temp_dir().join(format!("ca_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("secretStorage")).unwrap();
        let host: Arc<dyn PlatformHost> = Arc::new(DummyHost { root: root.clone() });
        let adapter = CodeArtsAdapter::with_base_urls_and_host("http://x", "http://y", host);

        let err = adapter
            .apply_session(
                &DummyHost { root: root.clone() } as &dyn PlatformHost,
                &cred_with("AK", "SK", "STS"),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::UnsupportedCapability,
                ..
            }
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn cred_with_expiry(expires_at: Option<i64>) -> CredentialEnvelope {
        let mut c = cred_with("AK", "SK", "STS");
        if let CredentialPayload::Token { expires_at: e, .. } = &mut c.payload {
            *e = expires_at;
        }
        c
    }

    #[test]
    fn expiry_check_blocks_only_truly_expired_and_allows_soft_window() {
        let now = chrono::Utc::now().timestamp_millis();

        // 已过期（越过有效期 1 秒）→ 本地拦截
        let err =
            CodeArtsAdapter::ensure_not_expired(&cred_with_expiry(Some(now - 1000))).unwrap_err();
        match err {
            AdapterError::Unified { code, message } => {
                assert_eq!(code, UnifiedErrorCode::AuthExpired);
                assert!(message.contains("码道 CodeArts"), "{message}");
                assert!(message.contains("重新导入"), "{message}");
            }
        }

        // 临期但未过期（剩余 2 分钟）→ 软过期，**放行**交服务端判定（P2：减少误标过期）
        assert!(
            CodeArtsAdapter::ensure_not_expired(&cred_with_expiry(Some(now + 2 * 60 * 1000)))
                .is_ok(),
            "临期但未过期的凭据不得被本地硬拦，应由服务端 401 确证"
        );

        // 剩余 30 分钟 → 放行
        assert!(
            CodeArtsAdapter::ensure_not_expired(&cred_with_expiry(Some(now + 30 * 60 * 1000)))
                .is_ok()
        );

        // 未记录有效期（手工粘贴 AK/SK）→ 放行，交服务端判定
        assert!(CodeArtsAdapter::ensure_not_expired(&cred_with_expiry(None)).is_ok());
    }

    /// 回归：授权码换 token 的响应不含用户身份时，uid 会退化成基于一次性 accessKey
    /// 的哈希（`codearts_<hash>`）。这既与本机导入的 userId 对不上，重复授权还会再建
    /// 一条账号——用户看到「同一个账号两条」。修复方式是换完 token 再用
    /// caller-identity 拿稳定身份。
    #[tokio::test]
    async fn authorization_import_uses_stable_identity_instead_of_ephemeral_access_key() {
        use ring::rand::SystemRandom;
        use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth2/tokens"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "credentials": {
                    "access_key_id": "AK-EPHEMERAL",
                    "secret_access_key": "SK",
                    "security_token": "ST",
                    "expires_at": 4102444800000i64
                },
                "refresh_token": "RT"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v5/caller-identity"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user": {
                    "id": "c02b6539a92f4027bc7b95945aba38f7",
                    "name": "GT-HeQiLucky",
                    "domain_id": "132872383cea43eeb25fbcec37b4c07c"
                }
            })))
            .mount(&server)
            .await;

        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .expect("generate dpop key");

        let env = adapter
            .exchange_authorization_code("code", "verifier", 4321, pkcs8.as_ref())
            .await
            .expect("exchange authorization code");

        assert_eq!(
            env.uid, "c02b6539a92f4027bc7b95945aba38f7",
            "OAuth 导入必须落在稳定身份上，否则与本机导入无法合并"
        );
        assert!(!CredentialEnvelope::is_fallback_uid(&env.uid));
        assert_eq!(env.display_name.as_deref(), Some("GT-HeQiLucky"));
        if let CredentialPayload::Token { extra, .. } = &env.payload {
            assert_eq!(
                extra.get(EXTRA_DOMAIN_ID).map(String::as_str),
                Some("132872383cea43eeb25fbcec37b4c07c")
            );
        } else {
            panic!("expected token payload");
        }
    }

    #[test]
    fn sts_response_without_identity_falls_back_to_ephemeral_uid() {
        // 记录历史行为：没有身份字段时只能退化，因此上层必须补 identity
        let body = serde_json::json!({
            "credentials": {
                "access_key_id": "AK1",
                "secret_access_key": "SK1",
                "security_token": "ST1"
            }
        });
        let env = CodeArtsAdapter::envelope_from_sts_response(
            &body, "verifier", "dpop", None, None, None, None,
        )
        .expect("envelope");
        assert!(
            CredentialEnvelope::is_fallback_uid(&env.uid),
            "uid 应为 codearts_ 前缀的兜底值"
        );
    }

    #[test]
    fn apply_identity_prefers_iam_user_id_and_keeps_existing_name() {
        let mut env = CredentialEnvelope {
            tool_id: "codearts".into(),
            uid: "codearts_abc123def456".into(),
            display_name: Some("已有昵称".into()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: String::new(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        CodeArtsAdapter::apply_identity(
            &mut env,
            &serde_json::json!({
                "principal_id": "should-not-win",
                "user": {"id": "u-1", "name": "新名字", "domain_id": "d-1"}
            }),
        );
        assert_eq!(env.uid, "u-1", "优先取 IAM user.id");
        assert_eq!(
            env.display_name.as_deref(),
            Some("已有昵称"),
            "不覆盖已有显示名"
        );

        // 没有 user 字段时退回 principal_id
        let mut env2 = env.clone();
        env2.uid = "codearts_x".into();
        CodeArtsAdapter::apply_identity(&mut env2, &serde_json::json!({"principal_id": "p-9"}));
        assert_eq!(env2.uid, "p-9");
    }

    fn token_cred_with_extra(extra: HashMap<String, String>) -> CredentialEnvelope {
        CredentialEnvelope {
            tool_id: "codearts".into(),
            uid: "u-1".into(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: String::new(),
                refresh_token: None,
                expires_at: None,
                extra,
            },
        }
    }

    #[test]
    fn snap_base_url_follows_region() {
        let mut extra = HashMap::new();
        extra.insert(EXTRA_REGION.to_string(), "cn-north-4".to_string());
        assert_eq!(
            CodeArtsAdapter::snap_base_url(&token_cred_with_extra(extra)),
            "https://snap-access.cn-north-4.myhuaweicloud.com"
        );

        let mut intl = HashMap::new();
        intl.insert(EXTRA_REGION.to_string(), "ap-southeast-1".to_string());
        assert_eq!(
            CodeArtsAdapter::snap_base_url(&token_cred_with_extra(intl)),
            "https://snap-access.ap-southeast-1.myhuaweicloud.com"
        );
        let cn_urls = CodeArtsAdapter::snap_base_urls(&token_cred_with_extra(HashMap::new()));
        assert_eq!(
            cn_urls,
            vec![
                "https://snap-access.cn-north-4.myhuaweicloud.com".to_string(),
                "https://snap-access.cn-north-4.myhuaweicloud.cn".to_string(),
            ]
        );
    }

    /// 网关复用这套签名头：必须是 SDK-HMAC-SHA256，且把 security token 与
    /// content-type 纳入 SignedHeaders（否则服务端校验会失败）。
    #[test]
    fn signed_request_headers_cover_token_and_content_type() {
        let mut extra = HashMap::new();
        extra.insert(EXTRA_ACCESS_KEY.to_string(), "AK-TEST".to_string());
        extra.insert(EXTRA_SECRET_KEY.to_string(), "SK-TEST".to_string());
        extra.insert(EXTRA_SECURITY_TOKEN.to_string(), "STS-TEST".to_string());
        let cred = token_cred_with_extra(extra);
        let body = br#"{"model":"deepseek-v4.1-flash","messages":[]}"#;
        let headers = CodeArtsAdapter::signed_request_headers(
            &cred,
            "POST",
            "https://snap-access.cn-north-4.myhuaweicloud.com/api/v2/chat/completions",
            body,
        )
        .expect("signed headers");

        let auth = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.clone())
            .expect("Authorization header");
        assert!(auth.starts_with("SDK-HMAC-SHA256 Access=AK-TEST"));
        let signed = auth
            .split("SignedHeaders=")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .unwrap_or_default();
        for required in ["content-type", "x-security-token", "x-sdk-date", "host"] {
            assert!(
                signed.contains(required),
                "SignedHeaders 缺少 {required}: {signed}"
            );
        }
        assert!(headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-security-token") && v == "STS-TEST"));
    }

    #[test]
    fn epoch_normalization_accepts_seconds_millis_and_iso() {
        let future_secs = (Utc::now() + chrono::Duration::hours(1)).timestamp();
        let future_ms = future_secs * 1000;
        assert_eq!(
            normalize_epoch_ms(&serde_json::json!(future_secs)),
            Some(future_ms),
            "秒级时间戳必须换算成毫秒"
        );
        assert_eq!(
            normalize_epoch_ms(&serde_json::json!(future_ms)),
            Some(future_ms),
            "毫秒级时间戳保持不变"
        );
        let iso = "2026-11-25T07:00:00Z";
        assert_eq!(
            normalize_epoch_ms(&serde_json::json!(iso)),
            Some(
                chrono::DateTime::parse_from_rfc3339(iso)
                    .unwrap()
                    .timestamp_millis()
            )
        );
        assert_eq!(normalize_epoch_ms(&serde_json::json!("")), None);
        assert_eq!(normalize_epoch_ms(&serde_json::json!(null)), None);
    }

    /// 回归：客户端把 `expiresAt` 写成秒时，历史实现会算成 1970 年，
    /// 于是在「明明处于登录状态」时误报「凭据已过期」。
    #[test]
    fn local_session_expiry_in_seconds_is_not_reported_as_expired() {
        let future_secs = (Utc::now() + chrono::Duration::hours(2)).timestamp();
        let session = serde_json::json!({
            "session": {
                "accessKey": "AK",
                "secretKey": "SK",
                "securityToken": "ST",
                "userId": "u-1",
                "userName": "GT-HeQiLucky",
                "expiresAt": future_secs
            }
        });
        let env = CodeArtsAdapter::envelope_from_session(&session).expect("envelope");
        let expires_at = match &env.payload {
            CredentialPayload::Token { expires_at, .. } => *expires_at,
            _ => panic!("expected token payload"),
        };
        assert!(
            expires_at.expect("expires_at") > 1_000_000_000_000,
            "秒级时间戳必须归一化为毫秒后再存储"
        );
        assert!(
            CodeArtsAdapter::ensure_not_expired(&env).is_ok(),
            "有效期内的凭据不得被判过期"
        );
    }

    #[test]
    fn local_session_really_expired_is_still_rejected_with_timestamp_hint() {
        let past_secs = (Utc::now() - chrono::Duration::hours(3)).timestamp();
        let session = serde_json::json!({
            "session": {
                "accessKey": "AK",
                "secretKey": "SK",
                "securityToken": "ST",
                "userId": "u-1",
                "expiresAt": past_secs
            }
        });
        let env = CodeArtsAdapter::envelope_from_session(&session).expect("envelope");
        let err = CodeArtsAdapter::ensure_not_expired(&env).expect_err("should be expired");
        let message = format!("{err}");
        assert!(
            message.contains("本地记录有效期"),
            "报错需带真实有效期: {message}"
        );
    }

    #[tokio::test]
    async fn inspect_quota_short_circuits_on_expired_credential() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        // 不挂 mock：若发请求就会 404
        let expired = cred_with_expiry(Some(chrono::Utc::now().timestamp_millis() - 60_000));
        let err = adapter.inspect_quota(&expired).await.unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "过期凭据不应发出任何网络请求"
        );
    }

    #[tokio::test]
    async fn claim_daily_short_circuits_on_expired_credential() {
        // claim 现在会真实联网；凭据过期时应直接拦截报 AuthExpired，
        // 避免发出注定 401 的请求（与 inspect_quota 行为一致）。
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        let expired = cred_with_expiry(Some(chrono::Utc::now().timestamp_millis() - 60_000));
        let err = adapter.claim_daily(&expired, None).await.unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// P2：软过期（临期但未真正过期）的 STS **不应**在本地被硬拦，
    /// 必须放行到服务端判定——这是「减少频繁关联本机」的核心行为。
    #[tokio::test]
    async fn soft_expiry_window_still_reaches_server_instead_of_forced_relink() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        // 额度接口响应（证明软过期凭据确实发出了请求，而非被本地拦截）
        Mock::given(method("GET"))
            .and(path("/v1/ops/delivery"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "data": { "items": [] } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/snap-manager/v1/package/overview"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "all_credit_package": {
                    "package_credit_amount": 100.0,
                    "package_credit_used": 0.0,
                    "package_credit_remain": 100.0
                }
            })))
            .mount(&server)
            .await;

        // 剩余 2 分钟：过去会被本地判 AuthExpired，现在应放行并真实联网。
        let mut soft = cred_with("AK", "SK", "STS");
        if let CredentialPayload::Token { expires_at, .. } = &mut soft.payload {
            *expires_at = Some(chrono::Utc::now().timestamp_millis() + 2 * 60 * 1000);
        }
        let q = adapter
            .inspect_quota(&soft)
            .await
            .expect("soft expiry 应放行联网");
        assert!(!q.buckets.is_empty(), "软过期凭据应拿到额度结果");
        assert!(
            !server.received_requests().await.unwrap().is_empty(),
            "软过期凭据必须实际发出请求，交服务端判定而非本地硬拦"
        );
    }

    #[test]
    fn decrypt_rejects_short_and_empty_payload() {
        let err = CodeArtsAdapter::decrypt_safe_storage(b"v10", "pw").unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                ..
            }
        ));
    }

    // 保证 ScopedPlatformHost 在本模块可解析（与其它适配器测试保持一致）
    #[allow(dead_code)]
    fn _assert_scoped_host_available(h: Arc<dyn PlatformHost>) -> ScopedPlatformHost {
        ScopedPlatformHost::new(h, "codearts", PathBuf::from("/tmp"))
    }

    #[test]
    fn dpop_jws_has_valid_es256_header_payload_and_64_byte_signature() {
        let flow = CodeArtsAdapter::create_oauth_flow().expect("oauth flow");
        assert!(flow.port >= 10_000);
        assert!(flow.auth_url.contains("client_id=codearts-agent"));
        assert!(flow.auth_url.contains("code_challenge_method=SHA-256"));

        let jws = CodeArtsAdapter::sign_dpop_jws(
            &flow.dpop_pkcs8,
            "POST",
            "https://sts.cn-north-4.myhuaweicloud.com/v1/oauth2/tokens",
        )
        .expect("dpop jws");
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(parts.len(), 3);
        let hdr: Value = serde_json::from_slice(&BASE64_URL.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(hdr["alg"], "ES256");
        assert_eq!(hdr["typ"], "dpop+jwt");
        assert_eq!(hdr["jwk"]["crv"], "P-256");
        assert_eq!(hdr["jwk"]["kty"], "EC");
        let pay: Value = serde_json::from_slice(&BASE64_URL.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(pay["htm"], "POST");
        assert_eq!(
            pay["htu"],
            "https://sts.cn-north-4.myhuaweicloud.com/v1/oauth2/tokens"
        );
        let sig_bytes = BASE64_URL.decode(parts[2]).unwrap();
        assert_eq!(sig_bytes.len(), 64, "ES256 JWS signature must be raw r||s");
    }

    #[tokio::test]
    async fn client_readonly_account_never_refreshes_to_avoid_kicking_desktop_client() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        let expired = cred_with_expiry(Some(chrono::Utc::now().timestamp_millis() - 60_000));
        assert!(adapter
            .refresh_credential(&expired)
            .await
            .expect("skip readonly")
            .is_none());
        assert!(adapter
            .force_refresh_credential(&expired)
            .await
            .expect("skip readonly force")
            .is_none());
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn isolated_oauth_codearts_refreshes_with_dpop_and_maps_sts5_1806_to_expired() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        let flow = CodeArtsAdapter::create_oauth_flow().expect("oauth flow");

        let next_exp = (chrono::Utc::now() + chrono::Duration::hours(6)).to_rfc3339();
        Mock::given(method("POST"))
            .and(path("/v1/oauth2/tokens"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "credentials": {
                    "access_key_id": "AK_NEW",
                    "secret_access_key": "SK_NEW",
                    "security_token": "STS_NEW",
                    "refresh_token": "RT_NEW",
                    "expires_at": next_exp,
                    "principal_id": "uid-ca-1",
                    "user_name": "CodeArtsDev"
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let mut env = adapter
            .exchange_authorization_code(
                "auth-code-1",
                &flow.code_verifier,
                flow.port,
                &flow.dpop_pkcs8,
            )
            .await
            .expect("exchange code");
        assert_eq!(env.uid, "uid-ca-1");
        // 将 expires_at 调近以触发刷新
        if let CredentialPayload::Token {
            ref mut expires_at, ..
        } = env.payload
        {
            *expires_at = Some(chrono::Utc::now().timestamp_millis() + 60_000);
        }

        let rotated_exp = (chrono::Utc::now() + chrono::Duration::hours(8)).to_rfc3339();
        Mock::given(method("POST"))
            .and(path("/v1/oauth2/tokens"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "credentials": {
                    "access_key_id": "AK_2",
                    "secret_access_key": "SK_2",
                    "security_token": "STS_2",
                    "refresh_token": "RT_2",
                    "expires_at": rotated_exp
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let refreshed = adapter
            .refresh_credential(&env)
            .await
            .expect("refresh ok")
            .expect("updated envelope");
        match &refreshed.payload {
            CredentialPayload::Token {
                refresh_token,
                extra,
                ..
            } => {
                assert_eq!(refresh_token.as_deref(), Some("RT_2"));
                assert_eq!(extra.get("access_key").map(String::as_str), Some("AK_2"));
                assert_eq!(
                    extra.get("codearts_mode").map(String::as_str),
                    Some("isolated_oauth")
                );
            }
            _ => panic!("expected Token"),
        }

        // STS5.1806 / invalid_grant -> AuthExpired
        Mock::given(method("POST"))
            .and(path("/v1/oauth2/tokens"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "STS5.1806: refresh token has been used"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let err = adapter
            .force_refresh_credential(&refreshed)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));
    }
}
