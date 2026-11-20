use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL},
    Engine as _,
};
use chrono::Utc;
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::adapter::common::{map_http_status, shared_http_client};
use crate::adapter::{
    AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult, ToolAdapter,
};
use crate::domain::{
    AuthMethod, ClaimStatus, CredentialEnvelope, CredentialPayload, QuotaBucket, QuotaSnapshot,
    UnifiedErrorCode,
};
use crate::platform::PlatformHost;

const DEFAULT_BASE_URL: &str = "https://openapi.qoder.com.cn";
const QODER_WEB_BASE: &str = "https://qoder.cn";
const QODER_CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";
const AUTH_DAT_FILE: &str = "auth.v1.dat";
const MACHINE_ID_FILE: &str = "auth.machine-id";
const APP_STATUS_SNAPSHOT_KEY: &str = "qoder_app_status.json";

#[cfg(target_os = "windows")]
const LOCAL_STATE_FILE: &str = "Local State";

pub struct QoderAdapter {
    manifest: AdapterManifest,
    http: reqwest::Client,
    /// 注入的平台宿主。为 None 时回退到 `NativePlatformHost`。
    ///
    /// 历史实现在 `read_local_machine_id_fallback` 内自造 `NativePlatformHost`，
    /// 使多实例 profile 隔离场景读不到目标 profile 的 machine-id。
    host: Option<Arc<dyn PlatformHost>>,
}

impl QoderAdapter {
    /// 注入平台宿主（用于多实例 profile 等隔离场景）。
    pub fn with_host(host: Arc<dyn PlatformHost>) -> Self {
        let mut adapter = Self::new();
        adapter.host = Some(host);
        adapter
    }

    /// 采集「当前本机客户端登录账号」的会话快照，用于后续冷切换。
    ///
    /// 背景：Qoder 的 `auth.v1.dat` 是 Chromium Safe Storage 加密（`v10` 前缀），
    /// 本项目不实现该加密链路，因此无法为「在线授权得到的 Token 账号」生成可写入
    /// 的快照。但 `apply_session` 的实现是**原样写回字节**，并不需要解密——
    /// 于是可行的路径是：让用户在 Qoder 客户端里手动登录目标账号，再由本方法
    /// 采集当时的 `auth.v1.dat` 字节，存为该账号的 LocalSnapshot。
    ///
    /// **安全前提**：必须确认本机当前登录的正是目标账号，否则会把 A 账号的快照
    /// 错误绑定到 B 账号上（切号时会把 B 登成 A）。因此这里比对
    /// `detect_local_active_uid` 与 `expected_uid`，不一致即拒绝采集。
    pub fn capture_session_snapshot(
        &self,
        host: &dyn PlatformHost,
        expected_uid: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let expected = expected_uid.trim();
        if expected.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "缺少目标账号 UID，无法校验本机登录态".into(),
            });
        }

        // 1. 身份校验：本机当前登录的必须是目标账号
        let local_uid =
            self.detect_local_active_uid(host)
                .ok_or_else(|| AdapterError::Unified {
                    code: UnifiedErrorCode::ClientNotFound,
                    message: "无法识别本机 Qoder 当前登录账号，请确认客户端已登录后再试".into(),
                })?;

        if local_uid != expected {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!(
                    "本机当前登录的不是目标账号（本机 {local_uid}，目标 {expected}）。\
                     请先在 Qoder 客户端中切换到该账号，再执行采集。"
                ),
            });
        }

        // 2. 采集原始字节（不解密，直接作为快照保存）
        let config_dir = host
            .client_config_dir("qoder_cn")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        let auth_path = config_dir.join(AUTH_DAT_FILE);
        if !auth_path.is_file() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: format!("未找到 Qoder CN 登录态文件: {}", auth_path.display()),
            });
        }

        let raw_dat = std::fs::read(&auth_path).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("读取 Qoder CN auth.v1.dat 失败: {e}"),
        })?;

        let mut files = HashMap::new();
        files.insert(AUTH_DAT_FILE.to_string(), BASE64.encode(&raw_dat));

        if let Ok(mid_bytes) = std::fs::read(config_dir.join(MACHINE_ID_FILE)) {
            files.insert(MACHINE_ID_FILE.to_string(), BASE64.encode(mid_bytes));
        }

        // 状态快照（含头像 URL 等）在切号时一并写回，保证 UI 一致
        if let Some(dot_dir) = Self::qoder_dot_dir() {
            if let Ok(status_bytes) = std::fs::read(dot_dir.join(".qoder-app-status.json")) {
                files.insert(
                    APP_STATUS_SNAPSHOT_KEY.to_string(),
                    BASE64.encode(&status_bytes),
                );
            }
        }

        Ok(CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: expected.to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        })
    }

    /// 优先使用注入的宿主，否则回退到本机实现。
    fn host_or_native(&self) -> Arc<dyn PlatformHost> {
        self.host
            .clone()
            .unwrap_or_else(|| Arc::new(crate::platform::NativePlatformHost))
    }

    pub fn new() -> Self {
        let http = shared_http_client().clone();

        Self {
            host: None,
            manifest: AdapterManifest {
                id: "qoder_cn".to_string(),
                name: "Qoder CN".to_string(),
                version: "0.1.0".to_string(),
                is_builtin: true,
                capabilities: AdapterCapabilities {
                    auth_methods: vec![
                        AuthMethod::LocalImport,
                        AuthMethod::WebviewOauth,
                        AuthMethod::TokenPaste,
                    ],
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: true,
                    supports_multi_instance: false,
                },
            },
            http,
        }
    }

    fn hash_uid(prefix: &str, data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hex_digest = hex::encode(hasher.finalize());
        format!("{prefix}_{}", &hex_digest[..12])
    }

    #[cfg(target_os = "windows")]
    async fn try_decrypt_windows_auth_dat(
        config_dir: &std::path::Path,
        raw_dat: &[u8],
    ) -> Option<Value> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };

        if raw_dat.len() < 3 + 12 + 16 + 2 || &raw_dat[..3] != b"v10" {
            return None;
        }
        let local_state_bytes = std::fs::read(config_dir.join(LOCAL_STATE_FILE)).ok()?;
        let local_state: Value = serde_json::from_slice(&local_state_bytes).ok()?;
        let enc_key_b64 = local_state.pointer("/os_crypt/encrypted_key")?.as_str()?;
        let enc_key_blob = BASE64.decode(enc_key_b64).ok()?;
        if enc_key_blob.len() <= 5 || &enc_key_blob[..5] != b"DPAPI" {
            return None;
        }
        let dpapi_b64 = BASE64.encode(&enc_key_blob[5..]);

        let ps_script = format!(
            "Add-Type -AssemblyName System.Security; \
             $b=[Convert]::FromBase64String('{dpapi_b64}'); \
             $u=[System.Security.Cryptography.ProtectedData]::Unprotect($b,$null,'CurrentUser'); \
             [Convert]::ToBase64String($u)"
        );
        let ps_bin = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .map(|r| {
                r.join("System32")
                    .join("WindowsPowerShell")
                    .join("v1.0")
                    .join("powershell.exe")
            })
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("powershell"));
        let mut cmd = tokio::process::Command::new(ps_bin);
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps_script]);
        cmd.creation_flags(0x0800_0000);
        cmd.kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(10), cmd.output())
            .await
            .ok()?
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let aes_key_b64 = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let aes_key = BASE64.decode(aes_key_b64).ok()?;
        let cipher = Aes256Gcm::new_from_slice(&aes_key).ok()?;
        let nonce = Nonce::from_slice(&raw_dat[3..15]);
        let plaintext = cipher.decrypt(nonce, &raw_dat[15..]).ok()?;
        serde_json::from_slice::<Value>(&plaintext).ok()
    }

    /// macOS 上解析 Qoder 的 `auth.v1.dat`。
    ///
    /// 真实 Qoder macOS 客户端用 Chromium Safe Storage 加密（`v10` 前缀 +
    /// Keychain 派生的 AES-128-CBC），本适配器**尚未实现**该解密链路：
    /// 这里只处理未加密的明文 JSON（部分开发版/降级版本会落明文）。
    ///
    /// 因此当返回 `None` 且原始数据以 `v10` 开头时，调用方必须明确告知用户
    /// 「本平台本地导入不可用，请改用在线授权」，而不是笼统提示「未完成授权」——
    /// 后者会让用户反复重试一个不可能成功的操作。
    #[cfg(target_os = "macos")]
    fn try_decrypt_macos_auth_dat(raw_dat: &[u8]) -> Option<Value> {
        if raw_dat.len() > 3 && &raw_dat[..3] == b"v10" {
            // 加密载荷：当前未实现 Safe Storage 解密
            if let Ok(val) = serde_json::from_slice::<Value>(&raw_dat[3..]) {
                return Some(val);
            }
            return None;
        }
        serde_json::from_slice::<Value>(raw_dat).ok()
    }

    /// 判断 macOS 上的 `auth.v1.dat` 是否为「已加密但本平台无法解密」的形态。
    #[cfg(target_os = "macos")]
    fn macos_auth_is_encrypted_but_unsupported(raw_dat: &[u8]) -> bool {
        raw_dat.len() > 3
            && &raw_dat[..3] == b"v10"
            && serde_json::from_slice::<Value>(&raw_dat[3..]).is_err()
    }

    fn build_headers(token: &str, machine_id: Option<&str>) -> Result<HeaderMap, AdapterError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static("Qoder/claim"));
        headers.insert("Cosy-ClientType", HeaderValue::from_static("10"));
        let auth = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "Qoder Token 包含非法请求头字符".into(),
            }
        })?;
        headers.insert(AUTHORIZATION, auth);
        if let Some(mid) = machine_id.filter(|s| !s.trim().is_empty()) {
            if let Ok(v) = HeaderValue::from_str(mid.trim()) {
                headers.insert("Cosy-MachineId", v);
            }
        }
        Ok(headers)
    }

    fn read_local_machine_id_fallback(host: &dyn PlatformHost) -> Option<String> {
        let config_dir = host.client_config_dir("qoder_cn").ok()?;
        for name in [MACHINE_ID_FILE, "machineid"] {
            if let Ok(raw) = std::fs::read_to_string(config_dir.join(name)) {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    /// 转发到共享实现（见 `adapter::common::merge_post_claim_quota`）。
    /// 保留此薄封装以免改动调用点与既有测试。
    fn merge_post_claim_quota(
        inspected: Option<QuotaSnapshot>,
        precheck: Option<&QuotaSnapshot>,
        confirmed_claimed: bool,
    ) -> Option<QuotaSnapshot> {
        crate::adapter::common::merge_post_claim_quota(inspected, precheck, confirmed_claimed)
    }

    fn extract_token_and_machine_id(
        cred: &CredentialEnvelope,
        host: &dyn PlatformHost,
    ) -> Result<(String, Option<String>), AdapterError> {
        match &cred.payload {
            CredentialPayload::Token {
                access_token,
                extra,
                ..
            } => {
                let machine_id = extra
                    .get("machine_id")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| Self::read_local_machine_id_fallback(host));
                Ok((access_token.clone(), machine_id))
            }
            CredentialPayload::LocalSnapshot { files } => {
                let machine_id = files
                    .get(MACHINE_ID_FILE)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok());

                if let Some(tok_b64) = files.get("decrypted_token.txt") {
                    if let Ok(bytes) = BASE64.decode(tok_b64) {
                        if let Ok(tok) = String::from_utf8(bytes) {
                            if !tok.trim().is_empty() {
                                return Ok((tok.trim().to_string(), machine_id));
                            }
                        }
                    }
                }

                Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "该账号尚未完成在线授权，请点击卡片上的「在线授权」完成一次登录以启用查额与签到".into(),
                })
            }
            CredentialPayload::Cookies { .. } => Err(AdapterError::Unified {
                code: UnifiedErrorCode::UnsupportedCapability,
                message: "Qoder CN 不支持 Cookie 凭据模式".into(),
            }),
        }
    }

    async fn fetch_campaigns(&self, headers: &HeaderMap) -> Result<Value, AdapterError> {
        let url = format!("{DEFAULT_BASE_URL}/sash/api/v1/me/campaigns");
        let resp = self
            .http
            .get(&url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("查询 Qoder CN 活动失败: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(map_http_status(status, "查询 Qoder CN 活动"));
        }

        resp.json::<Value>()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("解析 Qoder CN 活动响应失败: {e}"),
            })
            .and_then(|body| {
                if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
                    if code != 0 {
                        if let Some(err) =
                            crate::adapter::common::map_business_code(code, "查询 Qoder CN 活动")
                        {
                            return Err(err);
                        }
                        return Err(AdapterError::Unified {
                            code: UnifiedErrorCode::NetworkError,
                            message: format!("Qoder CN 活动接口返回异常业务码 {code}"),
                        });
                    }
                }
                Ok(body)
            })
    }

    fn analyze_campaigns(payload: &Value) -> (Vec<Value>, Vec<Value>, f64, f64) {
        let now = Utc::now().timestamp() as f64;
        let rows = payload
            .get("campaigns")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut claimable = Vec::new();
        let mut claimed = Vec::new();
        let mut claimable_credits = 0.0;
        let mut claimed_credits = 0.0;

        for row in rows {
            if row.get("actionType").and_then(|v| v.as_str()) != Some("CLAIM_BENEFIT") {
                continue;
            }
            let normalize_ts = |ts: f64| {
                if ts > 1_000_000_000_000.0 {
                    ts / 1000.0
                } else {
                    ts
                }
            };
            if let Some(start) = row
                .get("startAt")
                .and_then(|v| v.as_f64())
                .map(normalize_ts)
            {
                if now < start {
                    continue;
                }
            }
            if let Some(end) = row.get("endAt").and_then(|v| v.as_f64()).map(normalize_ts) {
                if now >= end {
                    continue;
                }
            }

            let amount = row
                .pointer("/benefit/amount")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            match row.get("claimStatus").and_then(|v| v.as_str()) {
                Some("CLAIMABLE") => {
                    claimable_credits += amount;
                    claimable.push(row);
                }
                Some("CLAIMED") => {
                    claimed_credits += amount;
                    claimed.push(row);
                }
                _ => {}
            }
        }

        (claimable, claimed, claimable_credits, claimed_credits)
    }

    fn qoder_dot_dir() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        #[cfg(not(target_os = "windows"))]
        let home = std::env::var_os("HOME");
        home.map(|h| PathBuf::from(h).join(".qoder-cn"))
    }

    fn extract_uid_from_avatar_url(avatar_url: &str) -> Option<String> {
        let marker = "/users/";
        let idx = avatar_url.find(marker)?;
        let rest = &avatar_url[idx + marker.len()..];
        let seg = rest.split('/').next()?.trim();
        if seg.len() >= 8 {
            Some(seg.to_string())
        } else {
            None
        }
    }

    fn read_uid_from_main_sqlite(config_dir: &std::path::Path) -> Option<String> {
        let db_path = config_dir.join("main.sqlite");
        if !db_path.is_file() {
            return None;
        }
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()?;
        let _ = conn.busy_timeout(std::time::Duration::from_millis(2000));
        for query in [
            "SELECT account_id FROM plugin_inventory_meta WHERE account_id IS NOT NULL AND account_id != '' ORDER BY rowid DESC LIMIT 1",
            "SELECT account_id FROM agent_profiles WHERE account_id IS NOT NULL AND account_id != '' ORDER BY rowid DESC LIMIT 1",
        ] {
            if let Ok(uid) = conn.query_row(query, [], |row| row.get::<_, String>(0)) {
                let trimmed = uid.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    pub fn create_pkce_device_flow(host: &dyn PlatformHost) -> (String, String, String, String) {
        const CHARSET: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
        let verifier: String = {
            let mut rng = rand::thread_rng();
            (0..64)
                .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
                .collect()
        };
        let challenge = BASE64_URL.encode(Sha256::digest(verifier.as_bytes()));
        let nonce = Uuid::new_v4().to_string();

        let machine_id = host
            .client_config_dir("qoder_cn")
            .ok()
            .and_then(|d| std::fs::read_to_string(d.join(MACHINE_ID_FILE)).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                Self::qoder_dot_dir()
                    .and_then(|d| std::fs::read_to_string(d.join(".auth").join("machine_id")).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let machine_token = Self::qoder_dot_dir()
            .and_then(|d| std::fs::read(d.join("umid-cache.json")).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|v| {
                v.pointer("/info/machineToken")
                    .and_then(|t| t.as_str())
                    .map(|s| s.trim().to_string())
            })
            .filter(|s| !s.is_empty());

        let mut params = vec![
            ("challenge", challenge),
            ("challenge_method", "S256".to_string()),
            ("nonce", nonce.clone()),
            ("machine_id", machine_id.clone()),
            ("client_id", QODER_CLIENT_ID.to_string()),
        ];
        if let Some(mt) = machine_token {
            params.push(("machine_token", mt));
        }

        let auth_url = reqwest::Url::parse_with_params(
            &format!("{QODER_WEB_BASE}/device/selectAccounts"),
            &params,
        )
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("{QODER_WEB_BASE}/users/sign-in"));

        (auth_url, nonce, verifier, machine_id)
    }

    pub async fn poll_pkce_device_token(
        nonce: &str,
        verifier: &str,
        machine_id: &str,
    ) -> Result<Option<CredentialEnvelope>, String> {
        let http = shared_http_client();

        let poll_url = reqwest::Url::parse_with_params(
            &format!("{DEFAULT_BASE_URL}/api/v1/deviceToken/poll"),
            &[
                ("nonce", nonce),
                ("verifier", verifier),
                ("challenge_method", "S256"),
            ],
        )
        .map_err(|e| format!("构造 Qoder 轮询 URL 失败: {e}"))?;

        let resp = http
            .get(poll_url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "qoder/0.5.14")
            .send()
            .await
            .map_err(|e| format!("轮询 Qoder 登录态失败: {}", e.without_url()))?;

        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(format!(
                "Qoder 授权轮询返回 HTTP {}",
                resp.status().as_u16()
            ));
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("解析 Qoder 授权响应失败: {e}"))?;
        let Some(token) = body
            .get("token")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };

        let refresh_token = body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mut uid_opt = body
            .get("user_id")
            .and_then(crate::adapter::common::json_uid_to_string);
        let mut name_opt = body
            .get("user_name")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mut email_opt: Option<String> = None;

        if let Ok(info_resp) = http
            .get(format!("{DEFAULT_BASE_URL}/api/v1/userinfo"))
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "qoder/0.5.14")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await
        {
            if info_resp.status().is_success() {
                if let Ok(info) = info_resp.json::<Value>().await {
                    if let Some(u) = info
                        .get("id")
                        .or_else(|| info.get("user_id"))
                        .or_else(|| info.get("uid"))
                        .and_then(crate::adapter::common::json_uid_to_string)
                    {
                        uid_opt = Some(u);
                    }
                    if let Some(n) = info
                        .get("name")
                        .or_else(|| info.get("username"))
                        .or_else(|| info.get("user_name"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        name_opt = Some(n.to_string());
                    }
                    email_opt = info
                        .get("email")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                }
            }
        }

        let uid = uid_opt.unwrap_or_else(|| Self::hash_uid("qoder_cn", token.as_bytes()));
        let mut extra = HashMap::new();
        if !machine_id.trim().is_empty() {
            extra.insert("machine_id".to_string(), machine_id.trim().to_string());
        }

        Ok(Some(CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid,
            display_name: name_opt,
            display_email: email_opt,
            payload: CredentialPayload::Token {
                access_token: token.to_string(),
                refresh_token,
                expires_at: None,
                extra,
            },
        }))
    }

    fn build_quota_snapshot(payload: &Value) -> QuotaSnapshot {
        let has_campaigns = payload
            .get("campaigns")
            .and_then(|v| v.as_array())
            .is_some();
        let (claimable, claimed, claimable_credits, claimed_credits) =
            Self::analyze_campaigns(payload);

        let claim_status = if !has_campaigns {
            ClaimStatus::Unknown
        } else if !claimable.is_empty() {
            ClaimStatus::Available
        } else if !claimed.is_empty() {
            ClaimStatus::Claimed
        } else {
            ClaimStatus::Unsupported
        };

        let total_window = claimable_credits + claimed_credits;
        // 如实反映活动窗口额度，不做 100 地板虚报；无额度数据时不造 bucket
        let buckets = if total_window <= 0.0 {
            Vec::new()
        } else {
            match claim_status {
                ClaimStatus::Claimed => vec![QuotaBucket {
                    key: "daily_credits".into(),
                    label: "本周期已领福利".into(),
                    used: 0.0,
                    total: total_window,
                    remaining: claimed_credits,
                    unit: "Credits".into(),
                    reset_at: None,
                }],
                ClaimStatus::Available => vec![QuotaBucket {
                    key: "daily_credits".into(),
                    label: "本周期可领福利".into(),
                    used: 0.0,
                    total: total_window,
                    remaining: claimable_credits,
                    unit: "Credits".into(),
                    reset_at: None,
                }],
                ClaimStatus::Unsupported | ClaimStatus::Unknown => Vec::new(),
            }
        };

        QuotaSnapshot {
            plan_name: None,
            claim_status,
            buckets,
            fetched_at: Utc::now().to_rfc3339(),
        }
    }
}

impl Default for QoderAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolAdapter for QoderAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn detect_local_active_uid(&self, host: &dyn PlatformHost) -> Option<String> {
        let config_dir = host.client_config_dir("qoder_cn").ok()?;
        if !config_dir.join(AUTH_DAT_FILE).is_file() {
            return None;
        }
        if let Some(dot_dir) = Self::qoder_dot_dir() {
            if let Ok(status_bytes) = std::fs::read(dot_dir.join(".qoder-app-status.json")) {
                if let Ok(status_json) = serde_json::from_slice::<Value>(&status_bytes) {
                    if let Some(avatar_uid) = status_json
                        .get("avatar_url")
                        .and_then(|v| v.as_str())
                        .and_then(Self::extract_uid_from_avatar_url)
                    {
                        if !CredentialEnvelope::is_fallback_uid(&avatar_uid) {
                            return Some(avatar_uid);
                        }
                    }
                }
            }
        }
        Self::read_uid_from_main_sqlite(&config_dir)
            .filter(|uid| !CredentialEnvelope::is_fallback_uid(uid))
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let config_dir = host
            .client_config_dir("qoder_cn")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        let auth_path = config_dir.join(AUTH_DAT_FILE);
        if !auth_path.is_file() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: format!("未找到 Qoder CN 本机登录态: {}", auth_path.display()),
            });
        }

        let raw_dat = std::fs::read(&auth_path).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("读取 Qoder CN auth.v1.dat 失败: {e}"),
        })?;

        let mut files = HashMap::new();
        files.insert(AUTH_DAT_FILE.to_string(), BASE64.encode(&raw_dat));

        if let Ok(mid_bytes) = std::fs::read(config_dir.join(MACHINE_ID_FILE)) {
            files.insert(MACHINE_ID_FILE.to_string(), BASE64.encode(mid_bytes));
        }

        #[allow(unused_mut)]
        let mut uid = Self::hash_uid("qoder_cn", &raw_dat);
        let mut display_name: Option<String> = None;

        if let Some(dot_dir) = Self::qoder_dot_dir() {
            let status_path = dot_dir.join(".qoder-app-status.json");
            if let Ok(status_bytes) = std::fs::read(&status_path) {
                files.insert(
                    APP_STATUS_SNAPSHOT_KEY.to_string(),
                    BASE64.encode(&status_bytes),
                );
                if let Ok(status_json) = serde_json::from_slice::<Value>(&status_bytes) {
                    if let Some(n) = status_json
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                    {
                        display_name = Some(n);
                    }
                    if let Some(avatar_uid) = status_json
                        .get("avatar_url")
                        .and_then(|v| v.as_str())
                        .and_then(Self::extract_uid_from_avatar_url)
                    {
                        uid = avatar_uid;
                    }
                }
            }
        }

        if uid.starts_with("qoder_cn_") {
            if let Some(sql_uid) = Self::read_uid_from_main_sqlite(&config_dir) {
                uid = sql_uid;
            }
        }

        #[cfg(target_os = "windows")]
        {
            if let Some(sess) = Self::try_decrypt_windows_auth_dat(&config_dir, &raw_dat).await {
                if let Some(tok) = sess.get("token").and_then(|v| v.as_str()) {
                    files.insert(
                        "decrypted_token.txt".to_string(),
                        BASE64.encode(tok.as_bytes()),
                    );
                    if uid.starts_with("qoder_cn_") {
                        uid = Self::hash_uid("qoder_cn", tok.as_bytes());
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            match Self::try_decrypt_macos_auth_dat(&raw_dat) {
                Some(sess) => {
                    if let Some(tok) = sess.get("token").and_then(|v| v.as_str()) {
                        files.insert(
                            "decrypted_token.txt".to_string(),
                            BASE64.encode(tok.as_bytes()),
                        );
                        if uid.starts_with("qoder_cn_") {
                            uid = Self::hash_uid("qoder_cn", tok.as_bytes());
                        }
                    }
                }
                None => {
                    // 已加密但本平台无解密实现：明确告知用户改走在线授权，
                    // 避免其反复尝试一个注定失败的本地导入。
                    if Self::macos_auth_is_encrypted_but_unsupported(&raw_dat) {
                        return Err(AdapterError::Unified {
                            code: UnifiedErrorCode::UnsupportedCapability,
                            message: "macOS 版 Qoder 的本地凭据为 Chromium Safe Storage 加密，\
                                      当前版本尚未实现该平台的本地解密。请改用「在线授权」导入 Qoder 账号。"
                                .into(),
                        });
                    }
                }
            }
        }

        Ok(CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid,
            display_name,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        })
    }

    async fn parse_manual_token(
        &self,
        raw_input: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let trimmed = raw_input.trim();
        if trimmed.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "Qoder Token 不能为空".into(),
            });
        }

        let (token, uid_opt, email_opt, name_opt, machine_id_opt) = if trimmed.starts_with('{') {
            let val: Value = serde_json::from_str(trimmed).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("JSON 格式无效: {e}"),
            })?;
            let tok = val
                .get("token")
                .or_else(|| val.get("accessToken"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "JSON 中缺少 token 字段".into(),
                })?
                .to_string();
            let uid = val
                .get("userId")
                .or_else(|| val.get("uid"))
                .or_else(|| val.get("user_id"))
                .and_then(crate::adapter::common::json_uid_to_string);
            let email = val
                .get("email")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let name = val
                .get("name")
                .or_else(|| val.get("nickName"))
                .or_else(|| val.get("nickname"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let machine_id = val
                .get("machineId")
                .or_else(|| val.get("machine_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (tok, uid, email, name, machine_id)
        } else {
            let clean = trimmed
                .strip_prefix("Bearer ")
                .unwrap_or(trimmed)
                .trim()
                .to_string();
            if clean.is_empty() {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "Qoder Token 不能为空".into(),
                });
            }
            (clean, None, None, None, None)
        };

        let uid = uid_opt.unwrap_or_else(|| Self::hash_uid("qoder_cn", token.as_bytes()));
        let mut extra = HashMap::new();
        if let Some(mid) = machine_id_opt {
            extra.insert("machine_id".to_string(), mid);
        }

        Ok(CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid,
            display_name: name_opt,
            display_email: email_opt,
            payload: CredentialPayload::Token {
                access_token: token,
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
        let host = self.host_or_native();
        let (token, machine_id) = Self::extract_token_and_machine_id(cred, &*host)?;
        let headers = Self::build_headers(&token, machine_id.as_deref())?;
        let payload = self.fetch_campaigns(&headers).await?;
        Ok(Self::build_quota_snapshot(&payload))
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        let host = self.host_or_native();
        let (token, machine_id) = Self::extract_token_and_machine_id(cred, &*host)?;
        let headers = Self::build_headers(&token, machine_id.as_deref())?;
        let payload = self.fetch_campaigns(&headers).await?;
        let has_campaigns = payload
            .get("campaigns")
            .and_then(|v| v.as_array())
            .is_some();
        if !has_campaigns {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "Qoder CN 活动列表格式异常，已中止领取".into(),
            });
        }
        let (claimable, claimed, _, _) = Self::analyze_campaigns(&payload);

        if claimable.is_empty() {
            let msg = if !claimed.is_empty() {
                "本周期已领取 Credits".to_string()
            } else {
                "当前暂无可领取的 Credits 活动".to_string()
            };
            // 直接基于本次已拉取的最新活动列表构建快照，避免复用过时 precheck 或重复查额
            let refreshed = Some(Self::build_quota_snapshot(&payload));
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: msg,
                updated_quota: refreshed,
            });
        }

        let mut total_gained = 0.0;
        let mut claimed_any = false;
        let mut saw_already_claimed = false;
        let mut skipped_invalid_cid = false;
        let mut skipped_missing_cid = false;
        let mut partial_failure_note: Option<String> = None;
        for (idx, campaign) in claimable.into_iter().enumerate() {
            let Some(cid) = campaign
                .get("campaignId")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                skipped_missing_cid = true;
                continue;
            };
            if !crate::adapter::common::is_valid_path_segment(cid) {
                skipped_invalid_cid = true;
                continue;
            }
            if idx > 0 {
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            let url = format!("{DEFAULT_BASE_URL}/sash/api/v1/me/campaigns/{cid}/claim");
            let resp = match self.http.post(&url).headers(headers.clone()).send().await {
                Ok(r) => r,
                Err(e) => {
                    let err = AdapterError::Unified {
                        code: UnifiedErrorCode::NetworkError,
                        message: format!("领取 Qoder CN Credits 失败: {e}"),
                    };
                    if claimed_any || saw_already_claimed {
                        partial_failure_note = Some("后续活动请求失败，已保留已领取进度".into());
                        break;
                    }
                    return Err(err);
                }
            };

            let status = resp.status();
            if !status.is_success() {
                if status.as_u16() == 400 || status.as_u16() == 409 {
                    let err_text = resp.text().await.unwrap_or_default();
                    if crate::adapter::common::is_idempotent_claimed_phrase(&err_text) {
                        saw_already_claimed = true;
                        continue;
                    }
                }
                let err = map_http_status(status, "领取 Qoder CN Credits");
                if claimed_any || saw_already_claimed {
                    partial_failure_note = Some(format!(
                        "后续活动返回异常（HTTP {}），已保留已领取进度",
                        status.as_u16()
                    ));
                    break;
                }
                return Err(err);
            }

            let body: Value = match resp.json().await {
                Ok(b) => b,
                Err(e) => {
                    let err = AdapterError::Unified {
                        code: UnifiedErrorCode::NetworkError,
                        message: format!("解析 Qoder CN 领取响应失败: {e}"),
                    };
                    if claimed_any || saw_already_claimed {
                        partial_failure_note =
                            Some("后续活动响应解析失败，已保留已领取进度".into());
                        break;
                    }
                    return Err(err);
                }
            };
            let data = body.get("data").filter(|v| v.is_object()).unwrap_or(&body);
            let status_str = data
                .get("status")
                .or_else(|| data.get("claimStatus"))
                .or_else(|| body.get("status"))
                .or_else(|| body.get("claimStatus"))
                .and_then(|v| v.as_str());
            let api_code = body.get("code").and_then(|v| v.as_i64());
            if let Some(c) = api_code {
                if let Some(err) =
                    crate::adapter::common::map_business_code(c, "领取 Qoder CN Credits")
                {
                    if claimed_any || saw_already_claimed {
                        partial_failure_note =
                            Some(format!("后续活动返回业务码 {c}，已保留已领取进度"));
                        break;
                    }
                    return Err(err);
                }
            }
            if status_str == Some("CLAIMED") || (status_str.is_none() && api_code == Some(0)) {
                claimed_any = true;
                if let Some(amt) = data
                    .pointer("/benefit/amount")
                    .or_else(|| campaign.pointer("/benefit/amount"))
                    .and_then(|v| v.as_f64())
                {
                    total_gained += amt;
                }
            } else if claimed_any || saw_already_claimed {
                partial_failure_note = Some("后续活动缺少确认状态，已保留已领取进度".into());
                break;
            } else {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: "Qoder CN 领取响应缺少确认状态，请刷新核实".into(),
                });
            }
        }

        if (skipped_invalid_cid || skipped_missing_cid) && !claimed_any && !saw_already_claimed {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: if skipped_missing_cid {
                    "Qoder CN 活动缺少有效 campaignId，无法完成领取".into()
                } else {
                    "Qoder CN 活动 ID 包含非法字符，无法完成领取".into()
                },
            });
        }

        // 领取成功或确认已领但领后查额失败/返回 Unknown/Unsupported 时合成 Claimed；
        // 若领后查额返回 Available（多活动/新活动仍可领），必须保留 Available，不得隐藏剩余活动
        let confirmed_claimed = claimed_any || saw_already_claimed;
        let refreshed = Self::merge_post_claim_quota(
            self.inspect_quota(cred).await.ok(),
            precheck,
            confirmed_claimed,
        );
        let base_summary = if total_gained > 0.0 {
            format!("成功领取 {} Credits", total_gained as i64)
        } else if claimed_any {
            "每日 Credits 领取已完成".into()
        } else {
            "本周期已领取".into()
        };
        let reward_summary = match partial_failure_note {
            Some(note) => format!("{base_summary}（{note}）"),
            None => base_summary,
        };
        Ok(ClaimResult {
            claimed: claimed_any,
            reward_summary,
            updated_quota: refreshed,
        })
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError> {
        let CredentialPayload::LocalSnapshot { files } = &cred.payload else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::UnsupportedCapability,
                message: "该账号缺少本机 auth.v1.dat 快照，无法写入客户端会话".into(),
            });
        };

        let dat_b64 = files
            .get(AUTH_DAT_FILE)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "快照中缺少 auth.v1.dat".into(),
            })?;
        let dat_bytes = BASE64.decode(dat_b64).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("auth.v1.dat 解码失败: {e}"),
        })?;

        let config_dir = host
            .client_config_dir("qoder_cn")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        std::fs::create_dir_all(&config_dir).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("创建 Qoder CN 配置目录失败: {e}"),
        })?;

        let target = config_dir.join(AUTH_DAT_FILE);
        crate::adapter::write_private_file_atomic(&target, &dat_bytes)?;

        if let Some(status_b64) = files.get(APP_STATUS_SNAPSHOT_KEY) {
            if let (Ok(status_bytes), Some(dot_dir)) =
                (BASE64.decode(status_b64), Self::qoder_dot_dir())
            {
                let status_target = dot_dir.join(".qoder-app-status.json");
                let _ = crate::adapter::write_private_file_atomic(&status_target, &status_bytes);
            }
        } else if let Some(dot_dir) = Self::qoder_dot_dir() {
            let _ = std::fs::remove_file(dot_dir.join(".qoder-app-status.json"));
        }

        Ok(())
    }

    async fn backup_session(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<crate::adapter::SessionBackup, AdapterError> {
        let config_dir = host
            .client_config_dir("qoder_cn")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        let mut paths = vec![config_dir.join(AUTH_DAT_FILE)];
        if let Some(dot_dir) = Self::qoder_dot_dir() {
            paths.push(dot_dir.join(".qoder-app-status.json"));
        }
        crate::adapter::backup_files(&paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qoder_snapshot_claimed_stays_healthy_and_missing_campaigns_is_unknown() {
        let missing = serde_json::json!({});
        let snap_missing = QoderAdapter::build_quota_snapshot(&missing);
        assert_eq!(snap_missing.claim_status, ClaimStatus::Unknown);
        assert!(snap_missing.buckets.is_empty());

        let claimed_payload = serde_json::json!({
            "campaigns": [
                {
                    "campaignId": "c1",
                    "actionType": "CLAIM_BENEFIT",
                    "claimStatus": "CLAIMED",
                    "benefit": { "amount": 100.0 }
                }
            ]
        });
        let snap_claimed = QoderAdapter::build_quota_snapshot(&claimed_payload);
        assert_eq!(snap_claimed.claim_status, ClaimStatus::Claimed);
        assert_eq!(snap_claimed.buckets.len(), 1);
        assert_eq!(snap_claimed.buckets[0].remaining, 100.0);
        assert_eq!(snap_claimed.buckets[0].total, 100.0);
        assert_eq!(snap_claimed.buckets[0].label, "本周期已领福利");

        // 非整百额度如实展示，不做 100 地板虚报
        let partial = serde_json::json!({
            "campaigns": [
                {
                    "campaignId": "c2",
                    "actionType": "CLAIM_BENEFIT",
                    "claimStatus": "CLAIMED",
                    "benefit": { "amount": 50.0 }
                }
            ]
        });
        let snap_partial = QoderAdapter::build_quota_snapshot(&partial);
        assert_eq!(snap_partial.buckets[0].remaining, 50.0);
        assert_eq!(snap_partial.buckets[0].total, 50.0);

        // amount 缺失不伪造额度桶
        let no_amount = serde_json::json!({
            "campaigns": [
                {
                    "campaignId": "c3",
                    "actionType": "CLAIM_BENEFIT",
                    "claimStatus": "CLAIMED"
                }
            ]
        });
        let snap_no_amount = QoderAdapter::build_quota_snapshot(&no_amount);
        assert_eq!(snap_no_amount.claim_status, ClaimStatus::Claimed);
        assert!(snap_no_amount.buckets.is_empty());
    }

    #[tokio::test]
    async fn qoder_numeric_uid_and_token_paste_capability() {
        let adapter = QoderAdapter::new();
        assert!(adapter
            .manifest()
            .capabilities
            .auth_methods
            .contains(&AuthMethod::TokenPaste));
        let env = adapter
            .parse_manual_token(r#"{"token":"qd-tok-123","userId":998877,"machineId":"mid-abc"}"#)
            .await
            .expect("numeric uid");
        assert_eq!(env.uid, "998877");
        if let CredentialPayload::Token { extra, .. } = &env.payload {
            assert_eq!(extra.get("machine_id").map(String::as_str), Some("mid-abc"));
        } else {
            panic!("expected Token payload");
        }
    }

    #[test]
    fn qoder_post_claim_quota_preserves_available_and_upgrades_unknown() {
        let pre = QuotaSnapshot {
            plan_name: Some("Pro".into()),
            claim_status: ClaimStatus::Available,
            buckets: vec![],
            fetched_at: "2026-11-18T00:00:00Z".into(),
        };

        // 领后查额仍返回 Available（多活动/新活动可领）：必须保留 Available
        let inspected_avail = QuotaSnapshot {
            plan_name: None,
            claim_status: ClaimStatus::Available,
            buckets: vec![],
            fetched_at: "2026-11-18T00:01:00Z".into(),
        };
        assert_eq!(
            QoderAdapter::merge_post_claim_quota(Some(inspected_avail), Some(&pre), true)
                .unwrap()
                .claim_status,
            ClaimStatus::Available
        );

        // 领后查额返回 Unknown / Unsupported 且已确认领取过：升格为 Claimed
        let inspected_unk = QuotaSnapshot {
            plan_name: None,
            claim_status: ClaimStatus::Unknown,
            buckets: vec![],
            fetched_at: "2026-11-18T00:01:00Z".into(),
        };
        assert_eq!(
            QoderAdapter::merge_post_claim_quota(Some(inspected_unk), Some(&pre), true)
                .unwrap()
                .claim_status,
            ClaimStatus::Claimed
        );

        // 领后查额失败（None）且已确认领取：合成 Claimed 快照并保留套餐名
        let synthetic = QoderAdapter::merge_post_claim_quota(None, Some(&pre), true).unwrap();
        assert_eq!(synthetic.claim_status, ClaimStatus::Claimed);
        assert_eq!(synthetic.plan_name.as_deref(), Some("Pro"));

        // 空可领活动列表 + 已领活动列表应产出 Claimed（不滞留 Available）
        let fresh_payload = serde_json::json!({
            "campaigns": [
                {
                    "campaignId": "c1",
                    "actionType": "CLAIM_BENEFIT",
                    "claimStatus": "CLAIMED",
                    "benefit": { "amount": 50 }
                }
            ]
        });
        let fresh_snap = QoderAdapter::build_quota_snapshot(&fresh_payload);
        assert_eq!(fresh_snap.claim_status, ClaimStatus::Claimed);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_auth_tests {
    use super::QoderAdapter;

    #[test]
    fn plaintext_auth_dat_still_parses() {
        let raw = br#"{"token":"plain-token-abc"}"#.to_vec();
        let parsed = QoderAdapter::try_decrypt_macos_auth_dat(&raw).expect("plaintext json");
        assert_eq!(
            parsed.get("token").and_then(|v| v.as_str()),
            Some("plain-token-abc")
        );
        assert!(!QoderAdapter::macos_auth_is_encrypted_but_unsupported(&raw));
    }

    #[test]
    fn v10_encrypted_payload_is_detected_as_unsupported() {
        // v10 前缀 + 非 JSON 的二进制密文（真实 Chromium Safe Storage 形态）
        let mut raw = b"v10".to_vec();
        raw.extend_from_slice(&[0x9f, 0x2a, 0x00, 0xff, 0x13, 0x88, 0x44, 0x01]);
        assert!(QoderAdapter::try_decrypt_macos_auth_dat(&raw).is_none());
        assert!(
            QoderAdapter::macos_auth_is_encrypted_but_unsupported(&raw),
            "v10 + 非 JSON 必须被识别为「加密但本平台不支持」"
        );
    }
}

#[cfg(test)]
mod capture_snapshot_tests {
    use super::QoderAdapter;
    use crate::adapter::AdapterError;
    use crate::domain::UnifiedErrorCode;

    /// 空 UID 必须在读取任何文件前就被拒绝（否则会把任意快照绑到空账号）。
    #[test]
    fn empty_expected_uid_is_rejected() {
        let host = crate::platform::NativePlatformHost;
        let adapter = QoderAdapter::new();
        let err = adapter
            .capture_session_snapshot(&host, "   ")
            .expect_err("空 UID 必须拒绝");
        match err {
            AdapterError::Unified { code, .. } => {
                assert_eq!(code, UnifiedErrorCode::InvalidCredential);
            }
        }
    }

    /// UID 不匹配时必须拒绝采集 —— 这是防止「把 A 的快照绑到 B 上」的核心保护。
    /// 用一个几乎不可能与本机真实账号相同的 UID 来触发该分支。
    #[test]
    fn mismatched_uid_is_rejected_before_writing() {
        let host = crate::platform::NativePlatformHost;
        let adapter = QoderAdapter::new();
        let result =
            adapter.capture_session_snapshot(&host, "00000000-0000-0000-0000-000000000000");
        // 本机未登录时返回 ClientNotFound；已登录时返回 InvalidCredential（不匹配）。
        // 两种情况都必须失败，绝不能成功。
        match result {
            Err(AdapterError::Unified { code, .. }) => {
                assert!(
                    matches!(
                        code,
                        UnifiedErrorCode::InvalidCredential | UnifiedErrorCode::ClientNotFound
                    ),
                    "不匹配的 UID 必须以凭据/客户端错误拒绝，实际 {code:?}"
                );
            }
            Ok(_) => panic!("不匹配的 UID 绝不能采集成功"),
        }
    }
}
