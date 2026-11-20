use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL},
    Engine as _,
};
use chrono::{Local, TimeZone, Utc};
use rand::RngCore;
use reqwest::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

const AGENT_ORIGIN: &str = "https://agent.minimax.cn";
const CREDIT_DETAILS_PATH: &str = "/minimax-cloud/api/v1/credit/details";
const SIGNIN_STATUS_PATH: &str = "/minimax-cloud/api/v1/signin/status";
const SIGNIN_CLAIM_PATH: &str = "/minimax-cloud/api/v1/signin/claim";
const USER_PROFILE_PATH: &str = "/matrix/api/v1/user/profile";
const ACCOUNT_TOKEN_URL: &str = "https://account.minimax.cn/oauth2/token";
const DEVICE_CODE_URL: &str = "https://account.minimax.cn/oauth2/device/code";
const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const CLIENT_ID: &str = "mcode-public";
const OAUTH_SCOPE: &str = "agent.default";
const OAUTH_AUDIENCE: &str = "agent-backend";
const CREDENTIAL_SERVICE: &str = "com.minimax.mcode.oauth.prod.cn";
const USER_AGENT_VALUE: &str = "MiniMax-Code/3.1";

const AUTH_JSON: &str = "auth.json";
const AUTH_STATE_JSON: &str = "auth-state.json";
const APP_CONFIG_JSON: &str = "minimax-agent-cn-config.json";
const SNAPSHOT_TOKEN_FILE: &str = "decrypted_token.txt";
const SNAPSHOT_REFRESH_FILE: &str = "decrypted_refresh_token.txt";

/// accessToken 过期前 5 分钟主动刷新，避免签到/查额遇到 401 后账号被判过期
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;

fn int_field(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

fn amount_field(v: Option<&Value>) -> f64 {
    match v {
        Some(v) => v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
            .unwrap_or(0.0),
        None => 0.0,
    }
}

fn ms_to_rfc3339(ms: i64) -> Option<String> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.to_rfc3339())
}

fn tz_offset_seconds() -> i64 {
    Local::now().offset().local_minus_utc() as i64
}

fn non_empty_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone)]
struct MiniMaxTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at_ms: Option<i64>,
}

/// 设备码授权会话（OAuth 2.0 Device Authorization Grant + PKCE）。
#[derive(Debug, Clone)]
pub struct MiniMaxDeviceAuth {
    login_url: String,
    device_code: Option<String>,
    user_code: Option<String>,
    code_verifier: String,
    interval_ms: u64,
    expires_at_ms: i64,
}

impl MiniMaxDeviceAuth {
    pub fn login_url(&self) -> &str {
        &self.login_url
    }

    pub fn poll_interval_ms(&self) -> u64 {
        self.interval_ms
    }

    pub fn is_expired(&self) -> bool {
        Utc::now().timestamp_millis() >= self.expires_at_ms
    }
}

#[derive(Debug)]
pub enum DeviceAuthPoll {
    Pending,
    SlowDown,
    Ready(Box<CredentialEnvelope>),
}

pub struct MiniMaxAdapter {
    manifest: AdapterManifest,
    http: reqwest::Client,
}

impl MiniMaxAdapter {
    pub fn new() -> Self {
        Self {
            manifest: AdapterManifest {
                id: "minimax_code_cn".to_string(),
                name: "MiniMax Code CN".to_string(),
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
            http: shared_http_client().clone(),
        }
    }

    fn home_dir() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        #[cfg(not(target_os = "windows"))]
        let home = std::env::var_os("HOME");

        home.map(PathBuf::from)
    }

    fn auth_file_paths() -> Option<(PathBuf, PathBuf)> {
        Self::home_dir().map(|home| {
            let dir = home.join(".minimax/auth/prod/cn/mcode-public");
            (dir.join(AUTH_JSON), dir.join(AUTH_STATE_JSON))
        })
    }

    fn read_device_id() -> Option<String> {
        Self::home_dir().and_then(|home| {
            std::fs::read_to_string(home.join(".minimax/v2/session-handoff/device-id"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    }

    /// 客户端 FileStore 的凭据键：`service\0account`，
    /// account = base64url(sha256(`<home>/.minimax/auth\0mcode-public`))。
    fn credential_record_key() -> Option<String> {
        let home = Self::home_dir()?;
        let auth_home = home.join(".minimax").join("auth");
        let seed = format!("{}\u{0}{CLIENT_ID}", auth_home.to_string_lossy());
        let account = BASE64_URL.encode(Sha256::digest(seed.as_bytes()));
        Some(format!("{CREDENTIAL_SERVICE}\u{0}{account}"))
    }

    fn app_config_path(host: &dyn PlatformHost) -> Option<PathBuf> {
        host.client_config_dir("minimax_code_cn")
            .ok()
            .map(|dir| dir.join(APP_CONFIG_JSON))
    }

    /// 单账号登录态下 records 只有一条；多记录时选过期时间最晚的一条，避免读到残留旧记录。
    fn pick_auth_record(auth: &Value) -> Option<&serde_json::Map<String, Value>> {
        auth.get("records")?
            .as_object()?
            .values()
            .filter(|v| v.is_object())
            .max_by_key(|v| v.get("expiresAtMs").and_then(int_field).unwrap_or(0))
            .and_then(|v| v.as_object())
    }

    fn tokens_from_record(record: &serde_json::Map<String, Value>) -> Option<MiniMaxTokens> {
        let access_token = record
            .get("accessToken")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?
            .to_string();
        let refresh_token = record
            .get("refreshToken")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let expires_at_ms = record.get("expiresAtMs").and_then(int_field);
        Some(MiniMaxTokens {
            access_token,
            refresh_token,
            expires_at_ms,
        })
    }

    fn tokens_from_snapshot(files: &HashMap<String, String>) -> Option<MiniMaxTokens> {
        if let Some(auth_b64) = files.get(AUTH_JSON) {
            if let Ok(bytes) = BASE64.decode(auth_b64) {
                if let Ok(auth) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(tokens) =
                        Self::pick_auth_record(&auth).and_then(Self::tokens_from_record)
                    {
                        return Some(tokens);
                    }
                }
            }
        }

        let access_token = files
            .get(SNAPSHOT_TOKEN_FILE)
            .and_then(|b64| BASE64.decode(b64).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let refresh_token = files
            .get(SNAPSHOT_REFRESH_FILE)
            .and_then(|b64| BASE64.decode(b64).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some(MiniMaxTokens {
            access_token,
            refresh_token,
            expires_at_ms: None,
        })
    }

    fn tokens_from_cred(cred: &CredentialEnvelope) -> Option<MiniMaxTokens> {
        match &cred.payload {
            CredentialPayload::Token {
                access_token,
                refresh_token,
                expires_at,
                ..
            } => {
                if access_token.trim().is_empty() {
                    return None;
                }
                let expires_at_ms = expires_at.map(|ms| {
                    if ms > 0 && ms < 1_000_000_000_000 {
                        ms.saturating_mul(1000)
                    } else {
                        ms
                    }
                });
                Some(MiniMaxTokens {
                    access_token: access_token.trim().to_string(),
                    refresh_token: refresh_token
                        .as_ref()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                    expires_at_ms,
                })
            }
            CredentialPayload::LocalSnapshot { files } => Self::tokens_from_snapshot(files),
            CredentialPayload::Cookies { .. } => None,
        }
    }

    fn extract_access_token(cred: &CredentialEnvelope) -> Result<String, AdapterError> {
        Self::tokens_from_cred(cred)
            .map(|t| t.access_token)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "该账号缺少可用的 MiniMax Code CN 访问令牌，请重新导入本机登录态".into(),
            })
    }

    fn ensure_base_resp(body: &Value, context: &str) -> Result<(), AdapterError> {
        let Some(base_resp) = body.get("base_resp") else {
            return Ok(());
        };
        let code = base_resp
            .get("status_code")
            .and_then(int_field)
            .unwrap_or(0);
        if code == 0 {
            return Ok(());
        }
        if let Some(err) = crate::adapter::common::map_business_code(code, context) {
            return Err(err);
        }
        let msg = base_resp
            .get("status_msg")
            .and_then(|v| v.as_str())
            .unwrap_or("未知业务错误");
        Err(AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("{context}：业务码 {code}（{msg}）"),
        })
    }

    async fn authed_get(
        &self,
        token: &str,
        url: &str,
        context: &str,
    ) -> Result<Value, AdapterError> {
        let resp = self
            .http
            .get(url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("{context}失败: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(map_http_status(status, context));
        }
        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析{context}响应失败: {e}"),
        })?;
        Self::ensure_base_resp(&body, context)?;
        Ok(body)
    }

    async fn fetch_credit_details(&self, token: &str) -> Result<Value, AdapterError> {
        let url = format!("{AGENT_ORIGIN}{CREDIT_DETAILS_PATH}");
        self.authed_get(token, &url, "查询 MiniMax 积分").await
    }

    async fn fetch_signin_status(&self, token: &str) -> Result<Value, AdapterError> {
        let url = format!(
            "{AGENT_ORIGIN}{SIGNIN_STATUS_PATH}?timezone_offset={}",
            tz_offset_seconds()
        );
        self.authed_get(token, &url, "查询 MiniMax 签到状态").await
    }

    async fn post_signin_claim(&self, token: &str) -> Result<Value, AdapterError> {
        let url = format!(
            "{AGENT_ORIGIN}{SIGNIN_CLAIM_PATH}?timezone_offset={}",
            tz_offset_seconds()
        );
        let resp = self
            .http
            .post(url)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .bearer_auth(token)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("领取 MiniMax 签到积分失败: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            if status.as_u16() == 400 || status.as_u16() == 409 {
                let err_text = resp.text().await.unwrap_or_default();
                if crate::adapter::common::is_idempotent_claimed_phrase(&err_text) {
                    return Ok(serde_json::json!({
                        "base_resp": { "status_code": 0 },
                        "data": { "claim_result": 2 }
                    }));
                }
            }
            return Err(map_http_status(status, "领取 MiniMax 签到积分"));
        }
        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 MiniMax 领取响应失败: {e}"),
        })?;
        Self::ensure_base_resp(&body, "领取 MiniMax 签到积分")?;
        Ok(body)
    }

    /// 发起设备码授权：POST /oauth2/device/code（PKCE S256）。
    pub async fn create_device_authorization() -> Result<MiniMaxDeviceAuth, AdapterError> {
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        let code_verifier = BASE64_URL.encode(raw);
        let code_challenge = BASE64_URL.encode(Sha256::digest(code_verifier.as_bytes()));

        let resp = shared_http_client()
            .post(DEVICE_CODE_URL)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .form(&[
                ("client_id", CLIENT_ID),
                ("scope", OAUTH_SCOPE),
                ("audience", OAUTH_AUDIENCE),
                ("code_challenge", code_challenge.as_str()),
                ("code_challenge_method", "S256"),
            ])
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("发起 MiniMax 设备授权失败: {e}"),
            })?;

        let http_status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !http_status.is_success() || body.get("error").is_some() {
            let detail = body
                .get("error_description")
                .or_else(|| body.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or(text.as_str());
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("MiniMax 设备授权被拒绝: {detail}"),
            });
        }

        let device_code = body.get("device_code").and_then(non_empty_str);
        let user_code = body.get("user_code").and_then(non_empty_str);
        if device_code.is_none() && user_code.is_none() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "MiniMax 设备授权响应缺少 device_code/user_code".into(),
            });
        }
        let login_url = body
            .get("verification_uri_complete")
            .and_then(non_empty_str)
            .or_else(|| body.get("verification_uri").and_then(non_empty_str))
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "MiniMax 设备授权响应缺少 verification_uri".into(),
            })?;
        let expires_in = body
            .get("expires_in")
            .and_then(int_field)
            .or_else(|| body.get("expired_in").and_then(int_field))
            .filter(|s| *s > 0)
            .unwrap_or(300);
        let interval = body
            .get("interval")
            .and_then(int_field)
            .filter(|s| *s > 0)
            .unwrap_or(3)
            .clamp(1, 60);

        Ok(MiniMaxDeviceAuth {
            login_url,
            device_code,
            user_code,
            code_verifier,
            interval_ms: (interval as u64).saturating_mul(1000),
            expires_at_ms: Utc::now()
                .timestamp_millis()
                .saturating_add(expires_in.saturating_mul(1000)),
        })
    }

    /// 单次轮询设备码令牌；Ready 时已完成 profile 查询并构造完整切号快照。
    pub async fn poll_device_authorization(
        auth: &MiniMaxDeviceAuth,
        host: &dyn PlatformHost,
    ) -> Result<DeviceAuthPoll, AdapterError> {
        if auth.is_expired() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                message: "MiniMax 设备授权已过期，请重新发起登录".into(),
            });
        }
        let (poll_field, poll_value) = if let Some(device_code) = auth.device_code.as_deref() {
            ("device_code", device_code)
        } else if let Some(user_code) = auth.user_code.as_deref() {
            ("user_code", user_code)
        } else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "MiniMax 设备授权缺少轮询凭据".into(),
            });
        };

        let resp = shared_http_client()
            .post(ACCOUNT_TOKEN_URL)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .form(&[
                ("grant_type", DEVICE_GRANT_TYPE),
                (poll_field, poll_value),
                ("client_id", CLIENT_ID),
                ("code_verifier", auth.code_verifier.as_str()),
            ])
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("轮询 MiniMax 设备授权失败: {}", e.without_url()),
            })?;

        let http_status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

        let poll_status = body
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.as_str()));
        match poll_status {
            Some("pending") | Some("authorization_pending") => return Ok(DeviceAuthPoll::Pending),
            Some("slow_down") => return Ok(DeviceAuthPoll::SlowDown),
            Some("denied") | Some("access_denied") => {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "用户在 MiniMax 授权页拒绝了本次登录".into(),
                })
            }
            Some("expired") | Some("expired_token") => {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: "MiniMax 设备授权已过期，请重新发起登录".into(),
                })
            }
            _ => {}
        }

        if !http_status.is_success() {
            return Err(map_http_status(http_status, "轮询 MiniMax 设备授权"));
        }

        let access_token = body
            .get("access_token")
            .and_then(non_empty_str)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "MiniMax 授权响应缺少 access_token".into(),
            })?;
        let refresh_token = body
            .get("refresh_token")
            .and_then(non_empty_str)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "MiniMax 授权响应缺少 refresh_token".into(),
            })?;
        let expires_in = body
            .get("expires_in")
            .and_then(int_field)
            .filter(|s| *s > 0)
            .unwrap_or(3600);
        let expires_at_ms = Utc::now()
            .timestamp_millis()
            .saturating_add(expires_in.saturating_mul(1000));

        let (uid, name, email) = Self::fetch_user_profile(&access_token).await?;
        let base_config = Self::app_config_path(host).and_then(|p| std::fs::read(p).ok());
        let files = Self::build_session_snapshot(
            &access_token,
            &refresh_token,
            expires_at_ms,
            &uid,
            name.as_deref(),
            base_config.as_deref(),
            Self::read_device_id().as_deref(),
        )?;

        Ok(DeviceAuthPoll::Ready(Box::new(CredentialEnvelope {
            tool_id: "minimax_code_cn".to_string(),
            uid,
            display_name: name,
            display_email: email,
            payload: CredentialPayload::LocalSnapshot { files },
        })))
    }

    async fn fetch_user_profile(
        token: &str,
    ) -> Result<(String, Option<String>, Option<String>), AdapterError> {
        let url = format!("{AGENT_ORIGIN}{USER_PROFILE_PATH}");
        let resp = shared_http_client()
            .get(url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("获取 MiniMax 账号信息失败: {e}"),
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(map_http_status(status, "获取 MiniMax 账号信息"));
        }
        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 MiniMax 账号信息失败: {e}"),
        })?;
        Self::ensure_base_resp(&body, "获取 MiniMax 账号信息")?;

        let uid = body
            .get("user_id")
            .and_then(crate::adapter::common::json_uid_to_string)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "MiniMax 账号信息缺少 user_id，无法建立账号".into(),
            })?;
        let name = body.get("user_name").and_then(non_empty_str);
        let email = body.get("email").and_then(non_empty_str);
        Ok((uid, name, email))
    }

    /// 用设备授权返回的令牌构造与客户端 FileStore 完全兼容的本地会话快照，
    /// 使内置授权得到的账号可直接用于冷切换。
    fn build_session_snapshot(
        access_token: &str,
        refresh_token: &str,
        expires_at_ms: i64,
        uid: &str,
        display_name: Option<&str>,
        base_config: Option<&[u8]>,
        device_id: Option<&str>,
    ) -> Result<HashMap<String, String>, AdapterError> {
        let record_key = Self::credential_record_key().ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: "无法解析 HOME 目录，不能生成 MiniMax 登录快照".into(),
        })?;
        let generation = 1;
        let login_epoch = Uuid::new_v4().to_string();

        let mut records = serde_json::Map::new();
        records.insert(
            record_key,
            serde_json::json!({
                "schemaVersion": 1,
                "accessToken": access_token,
                "refreshToken": refresh_token,
                "tokenType": "Bearer",
                "clientId": CLIENT_ID,
                "scopes": [OAUTH_SCOPE],
                "audience": OAUTH_AUDIENCE,
                "expiresAtMs": expires_at_ms,
                "generation": generation,
                "loginEpoch": login_epoch
            }),
        );
        let auth = serde_json::json!({
            "schemaVersion": 1,
            "records": Value::Object(records)
        });
        let state = serde_json::json!({
            "schemaVersion": 2,
            "status": "authenticated",
            "storeKind": "file",
            "clientId": CLIENT_ID,
            "scopes": [OAUTH_SCOPE],
            "audience": OAUTH_AUDIENCE,
            "buildEnv": "prod",
            "region": "cn",
            "generation": generation,
            "expiresAtMs": expires_at_ms
        });

        let mut config: Value = base_config
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if !config.is_object() {
            config = serde_json::json!({});
        }
        let obj = config.as_object_mut().expect("config object");
        let previous_device = obj
            .get("sharedUser")
            .and_then(|v| v.get("deviceID"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let device = device_id
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .or(previous_device)
            .unwrap_or_default();
        let name = display_name.unwrap_or(uid);
        obj.insert(
            "sharedUser".to_string(),
            serde_json::json!({
                "userID": uid,
                "realUserID": uid,
                "userName": name,
                "deviceID": device,
                "subUserName": name
            }),
        );
        // 清空上一账号在模板配置里遗留的令牌与用户信息，防止跨账号会话污染
        obj.insert("tokens".to_string(), serde_json::json!({}));
        obj.insert("user".to_string(), serde_json::json!({}));

        let auth_bytes = serde_json::to_vec(&auth).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("序列化 MiniMax auth.json 失败: {e}"),
        })?;
        let state_bytes = serde_json::to_vec(&state).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("序列化 MiniMax auth-state.json 失败: {e}"),
        })?;
        let config_bytes = serde_json::to_vec(&config).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("序列化 MiniMax 配置快照失败: {e}"),
        })?;

        let mut files = HashMap::new();
        files.insert(AUTH_JSON.to_string(), BASE64.encode(auth_bytes));
        files.insert(AUTH_STATE_JSON.to_string(), BASE64.encode(state_bytes));
        files.insert(APP_CONFIG_JSON.to_string(), BASE64.encode(config_bytes));
        files.insert(
            SNAPSHOT_TOKEN_FILE.to_string(),
            BASE64.encode(access_token.as_bytes()),
        );
        files.insert(
            SNAPSHOT_REFRESH_FILE.to_string(),
            BASE64.encode(refresh_token.as_bytes()),
        );
        Ok(files)
    }

    fn claim_status_from_signin(signin: &Value) -> ClaimStatus {
        let Some(days) = signin.pointer("/data/days").and_then(|v| v.as_array()) else {
            return ClaimStatus::Unknown;
        };
        let Some(today) = days
            .iter()
            .find(|d| d.get("is_today").and_then(|v| v.as_bool()).unwrap_or(false))
        else {
            return ClaimStatus::Unknown;
        };
        match today.get("status").and_then(int_field) {
            // 客户端 SigninDayStatus：1 Upcoming / 2 Claimable / 3 Claimed / 4 Disabled
            Some(2) => ClaimStatus::Available,
            Some(3) => ClaimStatus::Claimed,
            Some(4) => ClaimStatus::Unsupported,
            // Upcoming / 未知状态保持 fail-closed，避免误发领取请求
            _ => ClaimStatus::Unknown,
        }
    }

    fn signin_panel_today_claimed(claim: &Value) -> bool {
        claim
            .pointer("/data/panel/days")
            .and_then(|v| v.as_array())
            .is_some_and(|days| {
                days.iter().any(|d| {
                    d.get("is_today").and_then(|v| v.as_bool()).unwrap_or(false)
                        && d.get("status").and_then(int_field) == Some(3)
                })
            })
    }

    fn build_quota_snapshot(credit: &Value, signin: Option<&Value>) -> QuotaSnapshot {
        let details = credit.get("details").and_then(|v| v.as_array());
        let mut total = 0.0;
        let mut used = 0.0;
        let mut remaining = 0.0;
        let mut next_expiry: Option<i64> = None;

        if let Some(rows) = details {
            for row in rows {
                total += amount_field(row.get("granted_amount"));
                used += amount_field(row.get("consumed_amount"));
                remaining += amount_field(row.get("remaining_amount"));
                if let Some(expire) = row.get("expire_at_ms").and_then(int_field) {
                    if expire > 0 {
                        next_expiry = Some(next_expiry.map_or(expire, |cur| cur.min(expire)));
                    }
                }
            }
        }

        let buckets = if details.is_some_and(|rows| !rows.is_empty()) {
            vec![QuotaBucket {
                key: "credits".into(),
                label: "可用积分".into(),
                used,
                total,
                remaining,
                unit: "积分".into(),
                reset_at: next_expiry.and_then(ms_to_rfc3339),
            }]
        } else {
            Vec::new()
        };

        let claim_status = signin
            .map(Self::claim_status_from_signin)
            .unwrap_or(ClaimStatus::Unknown);

        QuotaSnapshot {
            plan_name: None,
            claim_status,
            buckets,
            fetched_at: Utc::now().to_rfc3339(),
        }
    }

    fn merge_post_claim_quota(
        inspected: Option<QuotaSnapshot>,
        precheck: Option<&QuotaSnapshot>,
        confirmed_claimed: bool,
    ) -> Option<QuotaSnapshot> {
        match inspected {
            Some(mut q) => {
                if confirmed_claimed
                    && matches!(
                        q.claim_status,
                        ClaimStatus::Unknown | ClaimStatus::Unsupported
                    )
                {
                    q.claim_status = ClaimStatus::Claimed;
                }
                Some(q)
            }
            None if confirmed_claimed => Some(QuotaSnapshot {
                plan_name: precheck.and_then(|p| p.plan_name.clone()),
                claim_status: ClaimStatus::Claimed,
                buckets: precheck.map(|p| p.buckets.clone()).unwrap_or_default(),
                fetched_at: Utc::now().to_rfc3339(),
            }),
            None => None,
        }
    }

    fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AdapterError> {
        crate::adapter::write_private_file_atomic(path, bytes)
    }

    fn decode_snapshot_file(
        files: &HashMap<String, String>,
        key: &str,
    ) -> Result<Vec<u8>, AdapterError> {
        let b64 = files.get(key).ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("账号快照缺少 {key}，无法写入客户端会话"),
        })?;
        BASE64.decode(b64).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: format!("{key} 解码失败: {e}"),
        })
    }

    /// 刷新成功后同步回本地快照中的 auth.json / auth-state.json，
    /// 保证下一次切号写入客户端的是轮换后的新令牌。
    fn rewrite_auth_files(
        files: &mut HashMap<String, String>,
        access_token: &str,
        refresh_token: &str,
        expires_at_ms: i64,
    ) -> Result<(), AdapterError> {
        let Some(auth_b64) = files.get(AUTH_JSON).cloned() else {
            return Ok(());
        };
        let mut auth: Value = BASE64
            .decode(&auth_b64)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "本地 auth.json 快照已损坏，无法写入刷新后的令牌".into(),
            })?;

        let mut generation = 0i64;
        if let Some(records) = auth.get_mut("records").and_then(|v| v.as_object_mut()) {
            let key = records
                .iter()
                .filter(|(_, v)| v.is_object())
                .max_by_key(|(_, v)| v.get("expiresAtMs").and_then(int_field).unwrap_or(0))
                .map(|(k, _)| k.clone());
            if let Some(key) = key {
                if let Some(record) = records.get_mut(&key).and_then(|v| v.as_object_mut()) {
                    generation = record.get("generation").and_then(int_field).unwrap_or(0) + 1;
                    record.insert("accessToken".into(), Value::String(access_token.into()));
                    record.insert("refreshToken".into(), Value::String(refresh_token.into()));
                    record.insert("expiresAtMs".into(), Value::from(expires_at_ms));
                    record.insert("generation".into(), Value::from(generation));
                }
            }
        }

        let auth_bytes = serde_json::to_vec(&auth).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("序列化刷新后的 auth.json 失败: {e}"),
        })?;
        files.insert(AUTH_JSON.into(), BASE64.encode(auth_bytes));

        if let Some(state_b64) = files.get(AUTH_STATE_JSON).cloned() {
            if let Ok(Ok(mut state)) = BASE64
                .decode(&state_b64)
                .map(|bytes| serde_json::from_slice::<Value>(&bytes))
            {
                if let Some(obj) = state.as_object_mut() {
                    obj.insert("expiresAtMs".into(), Value::from(expires_at_ms));
                    if generation > 0 {
                        obj.insert("generation".into(), Value::from(generation));
                    }
                }
                if let Ok(bytes) = serde_json::to_vec(&state) {
                    files.insert(AUTH_STATE_JSON.into(), BASE64.encode(bytes));
                }
            }
        }

        Ok(())
    }
}

impl Default for MiniMaxAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolAdapter for MiniMaxAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn detect_local_active_uid(&self, host: &dyn PlatformHost) -> Option<String> {
        let (auth_path, _) = Self::auth_file_paths()?;
        let auth_bytes = std::fs::read(&auth_path).ok()?;
        let auth_json: Value = serde_json::from_slice(&auth_bytes).ok()?;
        let record = Self::pick_auth_record(&auth_json)?;
        let _tokens = Self::tokens_from_record(record)?;
        let config_path = Self::app_config_path(host)?;
        let config_bytes = std::fs::read(&config_path).ok()?;
        let config: Value = serde_json::from_slice(&config_bytes).ok()?;
        let shared = config.get("sharedUser")?;
        let uid = shared
            .get("realUserID")
            .and_then(crate::adapter::common::json_uid_to_string)
            .or_else(|| {
                shared
                    .get("userID")
                    .and_then(crate::adapter::common::json_uid_to_string)
            })?;
        if CredentialEnvelope::is_fallback_uid(&uid) {
            None
        } else {
            Some(uid)
        }
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let Some((auth_path, state_path)) = Self::auth_file_paths() else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "无法读取 HOME 目录，不能定位 MiniMax Code CN 登录态".into(),
            });
        };
        if !auth_path.is_file() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: format!("未找到 MiniMax Code CN 本机登录态: {}", auth_path.display()),
            });
        }

        let auth_bytes = std::fs::read(&auth_path).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("读取 MiniMax auth.json 失败: {e}"),
        })?;
        let auth_json: Value =
            serde_json::from_slice(&auth_bytes).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("MiniMax auth.json 格式无效: {e}"),
            })?;
        let record = Self::pick_auth_record(&auth_json).ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "MiniMax auth.json 中没有有效的登录记录，请先在客户端完成登录".into(),
        })?;
        let tokens = Self::tokens_from_record(record).ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::InvalidCredential,
            message: "MiniMax auth.json 登录记录缺少 accessToken，请重新登录".into(),
        })?;

        let mut files = HashMap::new();
        files.insert(AUTH_JSON.to_string(), BASE64.encode(&auth_bytes));
        if let Ok(state_bytes) = std::fs::read(&state_path) {
            files.insert(AUTH_STATE_JSON.to_string(), BASE64.encode(state_bytes));
        }

        let mut uid_from_config: Option<String> = None;
        let mut display_name: Option<String> = None;
        if let Some(config_path) = Self::app_config_path(host) {
            if let Ok(config_bytes) = std::fs::read(&config_path) {
                files.insert(APP_CONFIG_JSON.to_string(), BASE64.encode(&config_bytes));
                if let Ok(config) = serde_json::from_slice::<Value>(&config_bytes) {
                    if let Some(shared) = config.get("sharedUser") {
                        uid_from_config = shared
                            .get("realUserID")
                            .and_then(crate::adapter::common::json_uid_to_string)
                            .or_else(|| {
                                shared
                                    .get("userID")
                                    .and_then(crate::adapter::common::json_uid_to_string)
                            });
                        display_name = shared
                            .get("userName")
                            .or_else(|| shared.get("subUserName"))
                            .and_then(non_empty_str);
                    }
                }
            }
        }

        // 用服务端 profile 的 user_name 作为权威昵称；若本机 config 未提供真实 userID，
        // 也优先采用服务端 profile 的 realUserID，确保与 OAuth 导入稳定去重同一账号。
        let mut uid_from_profile: Option<String> = None;
        let mut display_email: Option<String> = None;
        if let Ok((p_uid, p_name, p_email)) = Self::fetch_user_profile(&tokens.access_token).await {
            if !p_uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&p_uid) {
                uid_from_profile = Some(p_uid);
            }
            if let Some(n) = p_name.filter(|s| !s.trim().is_empty()) {
                display_name = Some(n);
            }
            display_email = p_email.filter(|s| !s.trim().is_empty());
        }

        let uid = uid_from_config
            .filter(|s| !s.is_empty())
            .or(uid_from_profile)
            .unwrap_or_else(|| {
                crate::adapter::common::hash_fallback_uid(
                    "minimax_cn",
                    tokens.access_token.as_bytes(),
                )
            });

        files.insert(
            SNAPSHOT_TOKEN_FILE.to_string(),
            BASE64.encode(tokens.access_token.as_bytes()),
        );
        if let Some(refresh) = tokens.refresh_token.as_ref() {
            files.insert(
                SNAPSHOT_REFRESH_FILE.to_string(),
                BASE64.encode(refresh.as_bytes()),
            );
        }

        Ok(CredentialEnvelope {
            tool_id: "minimax_code_cn".to_string(),
            uid,
            display_name,
            display_email,
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
                message: "MiniMax Token 不能为空".into(),
            });
        }

        let (access_token, refresh_token, expires_at, uid, display_name) = if trimmed
            .starts_with('{')
        {
            let val: Value = serde_json::from_str(trimmed).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("JSON 格式无效: {e}"),
            })?;

            // 兼容直接粘贴整个 auth.json（records.*）或常见的扁平字段格式
            let record = val
                .get("accessToken")
                .is_none()
                .then(|| Self::pick_auth_record(&val))
                .flatten();
            let source = record.map(|r| Value::Object(r.clone())).unwrap_or(val);

            let access = source
                .get("accessToken")
                .or_else(|| source.get("access_token"))
                .or_else(|| source.get("token"))
                .and_then(non_empty_str)
                .ok_or_else(|| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "JSON 中缺少 accessToken 字段".into(),
                })?;
            let refresh = source
                .get("refreshToken")
                .or_else(|| source.get("refresh_token"))
                .and_then(non_empty_str);
            let expires_at = source
                .get("expiresAtMs")
                .or_else(|| source.get("expires_at"))
                .and_then(int_field);
            let uid = source
                .get("userId")
                .or_else(|| source.get("userID"))
                .or_else(|| source.get("uid"))
                .and_then(crate::adapter::common::json_uid_to_string);
            let name = source
                .get("userName")
                .or_else(|| source.get("realUserName"))
                .or_else(|| source.get("nickName"))
                .or_else(|| source.get("name"))
                .and_then(non_empty_str);
            (access, refresh, expires_at, uid, name)
        } else {
            let clean = trimmed
                .strip_prefix("Bearer ")
                .unwrap_or(trimmed)
                .trim()
                .to_string();
            if clean.is_empty() {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "MiniMax Token 不能为空".into(),
                });
            }
            (clean, None, None, None, None)
        };

        let uid = uid.unwrap_or_else(|| {
            crate::adapter::common::hash_fallback_uid("minimax_cn", access_token.as_bytes())
        });

        Ok(CredentialEnvelope {
            tool_id: "minimax_code_cn".to_string(),
            uid,
            display_name,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token,
                refresh_token,
                expires_at,
                extra: HashMap::new(),
            },
        })
    }

    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError> {
        let token = Self::extract_access_token(cred)?;
        let credit = self.fetch_credit_details(&token).await?;
        // 签到状态属于附加信息：网络/解析失败时保留积分水位，但认证/风控/限流错误必须向上熔断
        let signin = match self.fetch_signin_status(&token).await {
            Ok(v) => Some(v),
            Err(
                e @ AdapterError::Unified {
                    code:
                        UnifiedErrorCode::AuthExpired
                        | UnifiedErrorCode::RiskTriggered
                        | UnifiedErrorCode::RateLimited,
                    ..
                },
            ) => return Err(e),
            Err(_) => None,
        };
        Ok(Self::build_quota_snapshot(&credit, signin.as_ref()))
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        let token = Self::extract_access_token(cred)?;
        let current_status = match precheck {
            Some(q) => q.claim_status.clone(),
            None => {
                let status = self.fetch_signin_status(&token).await?;
                Self::claim_status_from_signin(&status)
            }
        };
        match current_status {
            ClaimStatus::Claimed => {
                return Ok(ClaimResult {
                    claimed: false,
                    reward_summary: "今日已领取签到积分".into(),
                    updated_quota: match precheck {
                        Some(q) => Some(q.clone()),
                        None => self.inspect_quota(cred).await.ok(),
                    },
                });
            }
            ClaimStatus::Unsupported => {
                return Ok(ClaimResult {
                    claimed: false,
                    reward_summary: "当前账号暂不支持每日签到".into(),
                    updated_quota: None,
                });
            }
            ClaimStatus::Unknown => {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: "MiniMax 签到状态未知，为避免重复请求已中止领取".into(),
                });
            }
            ClaimStatus::Available => {}
        }

        let claim = self.post_signin_claim(&token).await?;
        let claim_result = claim.pointer("/data/claim_result").and_then(int_field);
        let points = claim
            .pointer("/data/points")
            .and_then(int_field)
            .unwrap_or(0);
        let already_claimed = claim_result == Some(2);
        let confirmed_claimed = !already_claimed
            && (claim_result == Some(1)
                || (Self::signin_panel_today_claimed(&claim) && points > 0));

        if !confirmed_claimed && !already_claimed {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "MiniMax 领取响应缺少确认状态，请刷新核实".into(),
            });
        }

        let refreshed = Self::merge_post_claim_quota(
            self.inspect_quota(cred).await.ok(),
            precheck,
            confirmed_claimed || already_claimed,
        );

        Ok(ClaimResult {
            claimed: confirmed_claimed,
            reward_summary: if confirmed_claimed && points > 0 {
                format!("成功领取 {points} 积分")
            } else if confirmed_claimed {
                "每日签到已完成".into()
            } else {
                "今日已领取签到积分".into()
            },
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
                message: "该账号缺少 MiniMax 本机登录快照，无法写入客户端会话".into(),
            });
        };

        let auth_bytes = Self::decode_snapshot_file(files, AUTH_JSON)?;
        let state_bytes_opt = if files.contains_key(AUTH_STATE_JSON) {
            Some(Self::decode_snapshot_file(files, AUTH_STATE_JSON)?)
        } else {
            None
        };
        let Some((auth_path, state_path)) = Self::auth_file_paths() else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "无法读取 HOME 目录，不能定位 MiniMax 登录态写入路径".into(),
            });
        };

        Self::write_atomic(&auth_path, &auth_bytes)?;
        if let Some(state_bytes) = state_bytes_opt {
            Self::write_atomic(&state_path, &state_bytes)?;
        } else if let Err(e) = std::fs::remove_file(&state_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("清理旧 MiniMax auth-state.json 失败: {e}"),
                });
            }
        }

        if let Some(config_b64) = files.get(APP_CONFIG_JSON) {
            let config_bytes = BASE64
                .decode(config_b64)
                .map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: format!("{APP_CONFIG_JSON} 解码失败: {e}"),
                })?;
            let config_dir =
                host.client_config_dir("minimax_code_cn")
                    .map_err(|e| AdapterError::Unified {
                        code: UnifiedErrorCode::InternalError,
                        message: e.to_string(),
                    })?;
            Self::write_atomic(&config_dir.join(APP_CONFIG_JSON), &config_bytes)?;
        }

        Ok(())
    }

    async fn backup_session(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<crate::adapter::SessionBackup, AdapterError> {
        let Some((auth_path, state_path)) = Self::auth_file_paths() else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: "无法读取 HOME 目录，不能定位 MiniMax 会话文件".into(),
            });
        };
        let mut paths = vec![auth_path, state_path];
        if let Some(config_path) = Self::app_config_path(host) {
            paths.push(config_path);
        }
        crate::adapter::backup_files(&paths)
    }

    async fn refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        let Some(tokens) = Self::tokens_from_cred(cred) else {
            return Ok(None);
        };
        let now_ms = Utc::now().timestamp_millis();
        let Some(expires_at_ms) = tokens.expires_at_ms else {
            // 无过期时间（纯手工粘贴）无法判断，避免盲目刷新
            return Ok(None);
        };
        if expires_at_ms - now_ms >= REFRESH_SKEW_MS {
            return Ok(None);
        }

        let Some(refresh_token) = tokens.refresh_token.filter(|s| !s.is_empty()) else {
            if now_ms >= expires_at_ms {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: "MiniMax 登录态已过期且缺少刷新令牌，请重新导入本机登录态".into(),
                });
            }
            return Ok(None);
        };

        let resp = self
            .http
            .post(ACCOUNT_TOKEN_URL)
            .header(ACCEPT, "application/json")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", CLIENT_ID),
            ])
            .send()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("刷新 MiniMax 登录态失败: {}", e.without_url()),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 400
                || status.as_u16() == 401
                || err_text.contains("invalid_grant")
            {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: "MiniMax 刷新令牌已失效，请重新导入本机登录态".into(),
                });
            }
            return Err(map_http_status(status, "刷新 MiniMax 登录态"));
        }

        let body: Value = resp.json().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("解析 MiniMax 刷新响应失败: {e}"),
        })?;
        let access_token = body
            .get("access_token")
            .and_then(non_empty_str)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "MiniMax 刷新响应缺少 access_token".into(),
            })?;
        let new_refresh = body
            .get("refresh_token")
            .and_then(non_empty_str)
            .unwrap_or(refresh_token);
        let expires_in = body
            .get("expires_in")
            .and_then(int_field)
            .filter(|s| *s > 0)
            .unwrap_or(3600);
        let new_expires_at_ms = now_ms.saturating_add(expires_in.saturating_mul(1000));

        let mut updated = cred.clone();
        match &mut updated.payload {
            CredentialPayload::Token {
                access_token: at,
                refresh_token: rt,
                expires_at,
                ..
            } => {
                *at = access_token.clone();
                *rt = Some(new_refresh.clone());
                *expires_at = Some(new_expires_at_ms);
            }
            CredentialPayload::LocalSnapshot { files } => {
                Self::rewrite_auth_files(files, &access_token, &new_refresh, new_expires_at_ms)?;
                files.insert(
                    SNAPSHOT_TOKEN_FILE.to_string(),
                    BASE64.encode(access_token.as_bytes()),
                );
                files.insert(
                    SNAPSHOT_REFRESH_FILE.to_string(),
                    BASE64.encode(new_refresh.as_bytes()),
                );
            }
            CredentialPayload::Cookies { .. } => return Ok(None),
        }

        Ok(Some(updated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as B64;

    #[test]
    fn quota_aggregates_credit_details_and_reads_signin() {
        let credit = serde_json::json!({
            "details": [
                {"granted_amount": 1000, "remaining_amount": 600, "consumed_amount": 400, "expire_at_ms": 1_800_000_000_000i64},
                {"granted_amount": 500, "remaining_amount": 500, "consumed_amount": 0, "expire_at_ms": 1_700_000_000_000i64}
            ],
            "total_count": 2,
            "base_resp": {"status_code": 0}
        });
        let signin = serde_json::json!({
            "data": {
                "days": [
                    {"day_no": 1, "points": 10, "status": 3, "is_today": false},
                    {"day_no": 2, "points": 20, "status": 2, "is_today": true}
                ]
            }
        });

        let snap = MiniMaxAdapter::build_quota_snapshot(&credit, Some(&signin));
        assert_eq!(snap.claim_status, ClaimStatus::Available);
        assert_eq!(snap.buckets.len(), 1);
        let bucket = &snap.buckets[0];
        assert_eq!(bucket.total, 1500.0);
        assert_eq!(bucket.used, 400.0);
        assert_eq!(bucket.remaining, 1100.0);
        assert_eq!(bucket.label, "可用积分");
        let expected = ms_to_rfc3339(1_700_000_000_000).unwrap();
        assert_eq!(bucket.reset_at.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn signin_status_maps_today_and_missing_days() {
        let claimed = serde_json::json!({
            "data": {"days": [{"status": 3, "is_today": true}]}
        });
        assert_eq!(
            MiniMaxAdapter::claim_status_from_signin(&claimed),
            ClaimStatus::Claimed
        );

        let available = serde_json::json!({
            "data": {"days": [{"status": 2, "is_today": true}]}
        });
        assert_eq!(
            MiniMaxAdapter::claim_status_from_signin(&available),
            ClaimStatus::Available
        );

        // 1 = Upcoming / 0 或缺失：fail-closed，避免误发领取请求
        for status in [0, 1] {
            let unknown = serde_json::json!({
                "data": {"days": [{"status": status, "is_today": true}]}
            });
            assert_eq!(
                MiniMaxAdapter::claim_status_from_signin(&unknown),
                ClaimStatus::Unknown,
                "status={status}"
            );
        }

        // 4 = Disabled：明确无可用签到
        let disabled = serde_json::json!({
            "data": {"days": [{"status": 4, "is_today": true}]}
        });
        assert_eq!(
            MiniMaxAdapter::claim_status_from_signin(&disabled),
            ClaimStatus::Unsupported
        );

        let missing_today = serde_json::json!({
            "data": {"days": [{"status": 1, "is_today": false}]}
        });
        assert_eq!(
            MiniMaxAdapter::claim_status_from_signin(&missing_today),
            ClaimStatus::Unknown
        );
        assert_eq!(
            MiniMaxAdapter::claim_status_from_signin(&serde_json::json!({})),
            ClaimStatus::Unknown
        );
    }

    #[tokio::test]
    async fn token_paste_accepts_flat_json_raw_bearer_and_auth_record() {
        let adapter = MiniMaxAdapter::new();
        let env = adapter
            .parse_manual_token(
                r#"{"accessToken":"mm-access","refreshToken":"mm-refresh","expiresAtMs":1800000000000,"userId":579123}"#,
            )
            .await
            .expect("flat json");
        assert_eq!(env.tool_id, "minimax_code_cn");
        assert_eq!(env.uid, "579123");
        if let CredentialPayload::Token {
            access_token,
            refresh_token,
            expires_at,
            ..
        } = &env.payload
        {
            assert_eq!(access_token, "mm-access");
            assert_eq!(refresh_token.as_deref(), Some("mm-refresh"));
            assert_eq!(*expires_at, Some(1_800_000_000_000));
        } else {
            panic!("expected Token payload");
        }

        let raw = adapter
            .parse_manual_token("Bearer mm-raw-token")
            .await
            .expect("raw bearer");
        assert!(raw.uid.starts_with("minimax_cn_"));

        // 直接粘贴客户端 auth.json 也能识别 records 内的登录记录
        let auth_json = serde_json::json!({
            "schemaVersion": 1,
            "records": {
                "k1": {
                    "accessToken": "from-records",
                    "refreshToken": "rt-from-records",
                    "expiresAtMs": 1900000000000i64,
                    "generation": 3
                }
            }
        });
        let nested = adapter
            .parse_manual_token(&auth_json.to_string())
            .await
            .expect("auth.json");
        if let CredentialPayload::Token { access_token, .. } = &nested.payload {
            assert_eq!(access_token, "from-records");
        } else {
            panic!("expected Token payload");
        }
    }

    #[test]
    fn refresh_rewrites_auth_json_and_state_snapshot() {
        let auth = serde_json::json!({
            "schemaVersion": 1,
            "records": {
                "k1": {
                    "accessToken": "old-access",
                    "refreshToken": "old-refresh",
                    "tokenType": "Bearer",
                    "expiresAtMs": 1_000i64,
                    "generation": 7,
                    "loginEpoch": 0
                }
            }
        });
        let state = serde_json::json!({
            "schemaVersion": 2,
            "status": "authenticated",
            "generation": 7,
            "expiresAtMs": 1_000i64
        });
        let mut files = HashMap::new();
        files.insert(
            AUTH_JSON.to_string(),
            B64.encode(serde_json::to_vec(&auth).unwrap()),
        );
        files.insert(
            AUTH_STATE_JSON.to_string(),
            B64.encode(serde_json::to_vec(&state).unwrap()),
        );

        MiniMaxAdapter::rewrite_auth_files(&mut files, "new-access", "new-refresh", 2_000)
            .expect("rewrite");

        let auth_out: Value =
            serde_json::from_slice(&B64.decode(files.get(AUTH_JSON).unwrap()).unwrap()).unwrap();
        let record = auth_out.pointer("/records/k1").unwrap();
        assert_eq!(
            record.get("accessToken").unwrap().as_str(),
            Some("new-access")
        );
        assert_eq!(
            record.get("refreshToken").unwrap().as_str(),
            Some("new-refresh")
        );
        assert_eq!(record.get("expiresAtMs").unwrap().as_i64(), Some(2_000));
        assert_eq!(record.get("generation").unwrap().as_i64(), Some(8));

        let state_out: Value =
            serde_json::from_slice(&B64.decode(files.get(AUTH_STATE_JSON).unwrap()).unwrap())
                .unwrap();
        assert_eq!(state_out.get("expiresAtMs").unwrap().as_i64(), Some(2_000));
        assert_eq!(state_out.get("generation").unwrap().as_i64(), Some(8));
    }

    #[test]
    fn snapshot_tokens_prefer_auth_json_over_decrypted_cache() {
        let auth = serde_json::json!({
            "records": {"k1": {"accessToken": "auth-json-token", "expiresAtMs": 123}}
        });
        let mut files = HashMap::new();
        files.insert(
            AUTH_JSON.to_string(),
            B64.encode(serde_json::to_vec(&auth).unwrap()),
        );
        files.insert(SNAPSHOT_TOKEN_FILE.to_string(), B64.encode(b"cached-token"));
        let tokens = MiniMaxAdapter::tokens_from_snapshot(&files).expect("tokens");
        assert_eq!(tokens.access_token, "auth-json-token");
        assert_eq!(tokens.expires_at_ms, Some(123));
    }

    #[test]
    fn device_auth_snapshot_is_client_file_store_compatible() {
        let base_config = br#"{"sharedUser":{"deviceID":"prev-device"},"config":{"x":1}}"#;
        let files = MiniMaxAdapter::build_session_snapshot(
            "acc-token",
            "ref-token",
            1_800_000_000_000,
            "579123",
            Some("MiniMax252465"),
            Some(base_config),
            Some("device-file"),
        )
        .expect("snapshot");

        let key = MiniMaxAdapter::credential_record_key().expect("key");
        assert!(key.starts_with(&format!("{CREDENTIAL_SERVICE}\u{0}")));

        let auth: Value =
            serde_json::from_slice(&B64.decode(files.get(AUTH_JSON).unwrap()).unwrap()).unwrap();
        let record = auth
            .get("records")
            .and_then(|v| v.as_object())
            .and_then(|records| records.get(&key))
            .expect("record under deterministic key");
        assert_eq!(
            record.get("accessToken").unwrap().as_str(),
            Some("acc-token")
        );
        assert_eq!(
            record.get("refreshToken").unwrap().as_str(),
            Some("ref-token")
        );
        assert_eq!(record.get("tokenType").unwrap().as_str(), Some("Bearer"));
        assert_eq!(
            record.get("clientId").unwrap().as_str(),
            Some("mcode-public")
        );
        assert_eq!(
            record.get("expiresAtMs").unwrap().as_i64(),
            Some(1_800_000_000_000)
        );
        assert!(record
            .get("loginEpoch")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()));

        let state: Value =
            serde_json::from_slice(&B64.decode(files.get(AUTH_STATE_JSON).unwrap()).unwrap())
                .unwrap();
        assert_eq!(state.get("status").unwrap().as_str(), Some("authenticated"));
        assert_eq!(state.get("storeKind").unwrap().as_str(), Some("file"));
        assert_eq!(state.get("generation").unwrap().as_i64(), Some(1));
        assert_eq!(state.get("region").unwrap().as_str(), Some("cn"));

        let config: Value =
            serde_json::from_slice(&B64.decode(files.get(APP_CONFIG_JSON).unwrap()).unwrap())
                .unwrap();
        assert_eq!(
            config.pointer("/sharedUser/realUserID").unwrap().as_str(),
            Some("579123")
        );
        assert_eq!(
            config.pointer("/sharedUser/userName").unwrap().as_str(),
            Some("MiniMax252465")
        );
        assert_eq!(
            config.pointer("/sharedUser/deviceID").unwrap().as_str(),
            Some("device-file")
        );
        assert_eq!(config.pointer("/config/x").unwrap().as_i64(), Some(1));

        // 生成的快照必须能被同一适配器再次解析，供刷新与查额复用
        let tokens = MiniMaxAdapter::tokens_from_snapshot(&files).expect("tokens");
        assert_eq!(tokens.access_token, "acc-token");
        assert_eq!(tokens.refresh_token.as_deref(), Some("ref-token"));
    }

    #[test]
    fn build_session_snapshot_wipes_inherited_tokens_and_user() {
        let base_config = br#"{
            "sharedUser": {"deviceID": "keep-dev", "userID": "old-user"},
            "tokens": {"accessToken": "leaked-token-a"},
            "user": {"id": "old-user", "email": "old@example.com"},
            "theme": "dark"
        }"#;
        let files = MiniMaxAdapter::build_session_snapshot(
            "new-acc-token",
            "new-ref-token",
            1_800_000_000_000,
            "new-uid",
            Some("NewUser"),
            Some(base_config),
            None,
        )
        .expect("snapshot");
        let config: Value =
            serde_json::from_slice(&B64.decode(files.get(APP_CONFIG_JSON).unwrap()).unwrap())
                .unwrap();
        assert_eq!(
            config
                .pointer("/sharedUser/deviceID")
                .and_then(|v| v.as_str()),
            Some("keep-dev")
        );
        assert_eq!(config.get("tokens"), Some(&serde_json::json!({})));
        assert_eq!(config.get("user"), Some(&serde_json::json!({})));
        assert_eq!(config.get("theme").and_then(|v| v.as_str()), Some("dark"));
    }

    #[tokio::test]
    async fn claim_daily_fail_closed_on_unknown_and_unsupported_precheck() {
        let adapter = MiniMaxAdapter::new();
        let cred = CredentialEnvelope {
            tool_id: "minimax_code_cn".to_string(),
            uid: "u_mm_1".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "mm-tok".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };

        let unknown_snap = QuotaSnapshot {
            plan_name: None,
            claim_status: ClaimStatus::Unknown,
            buckets: vec![],
            fetched_at: "2026-11-20T00:00:00Z".into(),
        };
        assert!(adapter
            .claim_daily(&cred, Some(&unknown_snap))
            .await
            .is_err());

        let unsup_snap = QuotaSnapshot {
            plan_name: None,
            claim_status: ClaimStatus::Unsupported,
            buckets: vec![],
            fetched_at: "2026-11-20T00:00:00Z".into(),
        };
        let res = adapter
            .claim_daily(&cred, Some(&unsup_snap))
            .await
            .expect("unsupported skips cleanly");
        assert!(!res.claimed);
    }

    #[test]
    fn merge_post_claim_quota_synthesizes_claimed_on_already_claimed() {
        let pre = QuotaSnapshot {
            plan_name: Some("Plus".into()),
            claim_status: ClaimStatus::Available,
            buckets: vec![],
            fetched_at: "2026-11-20T00:00:00Z".into(),
        };
        let syn = MiniMaxAdapter::merge_post_claim_quota(None, Some(&pre), true).expect("merged");
        assert_eq!(syn.claim_status, ClaimStatus::Claimed);
        assert_eq!(syn.plan_name.as_deref(), Some("Plus"));

        let unk = QuotaSnapshot {
            plan_name: None,
            claim_status: ClaimStatus::Unknown,
            buckets: vec![],
            fetched_at: "2026-11-20T00:01:00Z".into(),
        };
        let upgraded =
            MiniMaxAdapter::merge_post_claim_quota(Some(unk), Some(&pre), true).expect("upgraded");
        assert_eq!(upgraded.claim_status, ClaimStatus::Claimed);
    }
}
