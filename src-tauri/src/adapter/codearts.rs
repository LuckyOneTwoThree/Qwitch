//! 华为云 CodeArts（`codearts`）内置适配器 —— 额度监控型。
//!
//! 与其余内置适配器的关键差异：**CodeArts 没有「每日签到」机制**。
//! 对 `app.asar` 主进程代码全文检索 `每日` / `签到` / `打卡` / `daily` 均为 0 命中，
//! 仅存在一次性的权益领取（`BenefitService.claimBenefit`）。因此本适配器：
//!
//! * `supports_quota_query = true`
//! * `supports_daily_claim = false`
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
use base64::Engine as _;
use chrono::Utc;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use crate::adapter::common::{
    hash_fallback_uid, json_uid_to_string, map_http_status, shared_http_client,
};
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

/// Chromium safeStorage 在 macOS 上的固定参数（与 Electron 实现一致）。
const SAFE_STORAGE_SALT: &[u8] = b"saltysalt";
const SAFE_STORAGE_ITERATIONS: u32 = 1003;
const SAFE_STORAGE_KEY_LEN: usize = 16;
const SAFE_STORAGE_IV: [u8; 16] = [b' '; 16];

// extra 字段键名（凭据信封内传递华为云临时凭据）
const EXTRA_ACCESS_KEY: &str = "access_key";
const EXTRA_SECRET_KEY: &str = "secret_key";
const EXTRA_SECURITY_TOKEN: &str = "security_token";
const EXTRA_REGION: &str = "region";
const EXTRA_DOMAIN_ID: &str = "domain_id";

/// 凭据剩余有效期低于该阈值即视为「需要刷新」，提前拦截避免发出注定 401 的请求。
///
/// 客户端在剩余 1 小时时就会自行刷新（`startRefreshTimer` 里
/// `expiresAt - TOKEN_REFRESH_INTERVAL`），因此这里只留 5 分钟余量：
/// 真正的刷新责任交给客户端，Qwitch 只做只读监控。
const EXPIRY_SKEW_MS: i64 = 5 * 60 * 1000;

/// 凭据过期时给用户的行动指引。
const EXPIRED_HINT: &str =
    "CodeArts 凭据已过期。请打开「码道 CodeArts」客户端，它会自动刷新登录态；\
     刷新完成后回到 Qwitch 重新导入该账号即可";

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

/// 华为云 CodeArts 适配器（额度监控型，无每日签到）。
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
                    auth_methods: vec![AuthMethod::LocalImport, AuthMethod::TokenPaste],
                    supports_quota_query: true,
                    // CodeArts 无每日签到机制，见模块文档
                    supports_daily_claim: false,
                    supports_client_switch: true,
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
        let service = format!("{app_name} Safe Storage");
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                &service,
                "-a",
                app_name,
                "-w",
            ])
            .output()
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("调用 security 读取钥匙串失败: {e}"),
            })?;
        if !output.status.success() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!(
                    "未能从钥匙串读取「{service}」，请在系统「钥匙串访问」中允许 Qwitch 访问该条目"
                ),
            });
        }
        let pw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if pw.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("钥匙串条目「{service}」为空"),
            });
        }
        Ok(pw)
    }

    /// 解密 Chromium `v10` safeStorage 信封（AES-128-CBC）。
    ///
    /// 密钥派生与 Electron 的 `OSCrypt` 实现一致：
    /// `PBKDF2-HMAC-SHA1(password, "saltysalt", 1003, 16)`，IV 为 16 个空格。
    fn decrypt_safe_storage(payload: &[u8], password: &str) -> Result<Vec<u8>, AdapterError> {
        use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

        let body = if payload.len() > 3 && &payload[..3] == b"v10" {
            &payload[3..]
        } else {
            payload
        };
        if body.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts 凭据文件为空".into(),
            });
        }

        let mut key = [0u8; SAFE_STORAGE_KEY_LEN];
        pbkdf2_hmac_sha1(
            password.as_bytes(),
            SAFE_STORAGE_SALT,
            SAFE_STORAGE_ITERATIONS,
            &mut key,
        );

        type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
        let mut buf = body.to_vec();
        let plain = Aes128CbcDec::new(&key.into(), &SAFE_STORAGE_IV.into())
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|_| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "CodeArts 凭据解密失败（钥匙串密码不匹配）".into(),
            })?;
        Ok(plain.to_vec())
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
        let expires_at = s.get("expiresAt").and_then(|v| v.as_i64());
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

    /// 构造华为云 SDK-HMAC-SHA256 签名头。
    fn sign_headers(
        method: &str,
        url: &reqwest::Url,
        creds: &HuaweiCreds,
        extra_headers: &[(&str, &str)],
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

        let body_hash = hex::encode(Sha256::digest(b""));
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

        let resp = req.send().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("{context}失败: {}", e.without_url()),
        })?;
        let status = resp.status();
        if !status.is_success() {
            // 401/APIG.0301 表示 AK/SK 签名校验失败 → 凭据失效而非网络问题
            if status.as_u16() == 401 {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: format!("{context}: 华为云签名校验失败 (HTTP 401)，凭据可能已过期"),
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
    fn ensure_not_expired(cred: &CredentialEnvelope) -> Result<(), AdapterError> {
        let CredentialPayload::Token { expires_at, .. } = &cred.payload else {
            // 非 Token 形态不在此处拦截，交给后续流程处理
            return Ok(());
        };
        let Some(expires_at) = expires_at else {
            // 未记录有效期（如手工粘贴的 AK/SK）→ 不拦截，让服务端判定
            return Ok(());
        };
        let now = Utc::now().timestamp_millis();
        if *expires_at - now <= EXPIRY_SKEW_MS {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                message: EXPIRED_HINT.to_string(),
            });
        }
        Ok(())
    }
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

/// PBKDF2-HMAC-SHA1（Chromium safeStorage 的密钥派生）。
fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;

    let mut block_index: u32 = 1;
    let mut filled = 0usize;
    while filled < out.len() {
        let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(password).expect("HMAC 接受任意长度密钥");
        mac.update(salt);
        mac.update(&block_index.to_be_bytes());
        let mut u = mac.finalize().into_bytes();
        let mut acc = u;
        for _ in 1..iterations {
            let mut mac =
                <Hmac<Sha1> as Mac>::new_from_slice(password).expect("HMAC 接受任意长度密钥");
            mac.update(&u);
            u = mac.finalize().into_bytes();
            for (a, b) in acc.iter_mut().zip(u.iter()) {
                *a ^= b;
            }
        }
        let take = (out.len() - filled).min(acc.len());
        out[filled..filled + take].copy_from_slice(&acc[..take]);
        filled += take;
        block_index += 1;
    }
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
        Ok(QuotaSnapshot {
            plan_name: Some(plan_name.to_string()),
            // 无签到机制，状态恒为不支持；调度器据此跳过领取
            claim_status: ClaimStatus::Unsupported,
            buckets,
            fetched_at: Utc::now().to_rfc3339(),
        })
    }

    async fn claim_daily(
        &self,
        _cred: &CredentialEnvelope,
        _precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        // CodeArts 没有每日签到机制，不做任何网络请求。
        // 注意：这里**不**做过期检查——调度器对无签到能力的工具本就不该调它，
        // 若在此报 AuthExpired 会让账号状态被误标为过期，干扰额度监控。
        Ok(ClaimResult {
            claimed: false,
            reward_summary: "码道 CodeArts 无每日签到机制，仅监控额度".into(),
            updated_quota: None,
        })
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError> {
        // 切号 = 把目标账号的 session 重新加密写回 secretStorage。
        // 但 safeStorage 加密需要 Electron 运行时（Keychain 主密码可读，
        // 加密本身可复现），这里采用「备份原文件 → 用新 session 覆盖」策略：
        // 由于无法在本进程内生成合法 safeStorage 密文，切号需借助客户端。
        let _ = (host, cred);
        Err(AdapterError::Unified {
            code: UnifiedErrorCode::UnsupportedCapability,
            message: "码道 CodeArts 暂不支持本地切号（凭据由客户端 safeStorage 加密托管）".into(),
        })
    }

    async fn backup_session(&self, host: &dyn PlatformHost) -> Result<SessionBackup, AdapterError> {
        let dir = Self::resolve_secret_dir(host)?;
        let file = dir.join(Self::secret_file_name(&format!(
            "authSession:{DEFAULT_REGION}"
        )));
        let content = std::fs::read(&file).ok();
        Ok(SessionBackup {
            files: vec![(file, content)],
        })
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
    fn manifest_declares_no_daily_claim() {
        let a = CodeArtsAdapter::new();
        let m = a.manifest();
        assert_eq!(m.id, "codearts");
        assert!(m.capabilities.supports_quota_query);
        assert!(
            !m.capabilities.supports_daily_claim,
            "CodeArts 无每日签到机制，capability 必须为 false"
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

    #[test]
    fn pbkdf2_matches_known_vector() {
        // PBKDF2-HMAC-SHA1("password", "salt", 1, 20) 的公开测试向量
        let mut out = [0u8; 20];
        pbkdf2_hmac_sha1(b"password", b"salt", 1, &mut out);
        assert_eq!(hex::encode(out), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
    }

    #[tokio::test]
    async fn inspect_quota_returns_unsupported_claim_status() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
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
        assert_eq!(q.claim_status, ClaimStatus::Unsupported);
        assert_eq!(q.buckets.len(), 3);
        assert_eq!(q.plan_name.as_deref(), Some("基础套餐"));

        // 必须带上签名头
        let reqs = server.received_requests().await.unwrap();
        let auth = reqs[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(auth.starts_with("SDK-HMAC-SHA256 Access=AK"), "auth={auth}");
        assert!(auth.contains("SignedHeaders="));
        assert!(auth.contains("Signature="));
        assert_eq!(reqs[0].headers.get("x-security-token").unwrap(), "STS");
        assert!(reqs[0].headers.contains_key("x-sdk-date"));
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
    async fn claim_daily_is_noop_without_network() {
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        // 不挂任何 mock：若有请求就会 404 报错
        let res = adapter
            .claim_daily(&cred_with("AK", "SK", "STS"), None)
            .await
            .expect("noop");
        assert!(!res.claimed);
        assert!(res.reward_summary.contains("无每日签到"));
        assert!(server.received_requests().await.unwrap().is_empty());
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
    fn expiry_check_blocks_expired_and_near_expiry() {
        let now = chrono::Utc::now().timestamp_millis();

        // 已过期
        let err =
            CodeArtsAdapter::ensure_not_expired(&cred_with_expiry(Some(now - 1000))).unwrap_err();
        match err {
            AdapterError::Unified { code, message } => {
                assert_eq!(code, UnifiedErrorCode::AuthExpired);
                assert!(message.contains("码道 CodeArts"), "{message}");
                assert!(message.contains("重新导入"), "{message}");
            }
        }

        // 剩余 2 分钟 < 5 分钟余量 → 视为需要刷新
        let err = CodeArtsAdapter::ensure_not_expired(&cred_with_expiry(Some(now + 2 * 60 * 1000)))
            .unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));

        // 剩余 30 分钟 → 放行
        assert!(
            CodeArtsAdapter::ensure_not_expired(&cred_with_expiry(Some(now + 30 * 60 * 1000)))
                .is_ok()
        );

        // 未记录有效期（手工粘贴 AK/SK）→ 放行，交服务端判定
        assert!(CodeArtsAdapter::ensure_not_expired(&cred_with_expiry(None)).is_ok());
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
    async fn claim_daily_ignores_expiry_to_avoid_mislabeling_account() {
        // claim 是 no-op，即使凭据过期也不应报 AuthExpired，
        // 否则会把账号状态误标为过期、打断额度监控。
        let server = MockServer::start().await;
        let adapter = CodeArtsAdapter::with_base_urls(server.uri(), server.uri());
        let expired = cred_with_expiry(Some(chrono::Utc::now().timestamp_millis() - 60_000));
        let res = adapter.claim_daily(&expired, None).await.expect("noop");
        assert!(!res.claimed);
        assert!(server.received_requests().await.unwrap().is_empty());
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
}
