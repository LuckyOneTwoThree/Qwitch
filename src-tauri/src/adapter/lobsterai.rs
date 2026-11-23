use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL},
    Engine as _,
};
use chrono::{TimeZone, Utc};
use reqwest::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

use crate::adapter::common::{
    hash_fallback_uid, is_idempotent_claimed_phrase, is_valid_path_segment, json_uid_to_string,
    map_business_code, map_http_status, merge_post_claim_quota, shared_http_client,
};
use crate::adapter::{
    AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult, SessionBackup, ToolAdapter,
};
use crate::domain::{
    AuthMethod, ClaimStatus, CredentialEnvelope, CredentialPayload, QuotaBucket, QuotaSnapshot,
    UnifiedErrorCode,
};
use crate::platform::PlatformHost;

const DEFAULT_SERVER_BASE: &str = "https://lobsterai-server.youdao.com";
const DEFAULT_PORTAL_LOGIN_URL: &str = "https://lobsterai.youdao.com/portal#/login";
const CLIENT_VERSION: &str = "2026.11.20";
const USER_AGENT_VALUE: &str = "LobsterAI/2026.11.20";
const LOBSTERAI_DB_FILE: &str = "lobsterai.sqlite";
const SNAPSHOT_TOKEN_FILE: &str = "decrypted_token.txt";
const SNAPSHOT_REFRESH_FILE: &str = "decrypted_refresh_token.txt";

const KV_AUTH_TOKENS: &str = "auth_tokens";
const KV_AUTH_USER: &str = "auth_user";
const KV_ENTERPRISE_CONTEXT: &str = "enterprise_account_context";
const EXTRA_AUTH_USER_JSON: &str = "auth_user_json";
const EXTRA_YID: &str = "yid";

/// accessToken 过期前 5 分钟主动刷新，避免查额/签到遇到 401。
const REFRESH_SKEW_SECS: i64 = 5 * 60;

#[derive(Debug, Clone)]
struct LobsterTokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
    extra: HashMap<String, String>,
}

/// LobsterAI（网易有道桌面 AI Agent）内置适配器。
pub struct LobsterAiAdapter {
    manifest: AdapterManifest,
    http: reqwest::Client,
    server_base: String,
}

impl LobsterAiAdapter {
    pub fn new() -> Self {
        Self::with_server_base(DEFAULT_SERVER_BASE)
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(server_base: impl Into<String>) -> Self {
        Self::with_server_base(server_base)
    }

    fn with_server_base(server_base: impl Into<String>) -> Self {
        Self {
            manifest: AdapterManifest {
                id: "lobsterai".to_string(),
                name: "LobsterAI".to_string(),
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
            server_base: server_base.into().trim_end_matches('/').to_string(),
        }
    }

    /// 构造官方桌面授权登录入口 URL（经由 Qwitch 内置 WebView loop-back 拦截回调）。
    /// 对带 Hash 路由的门户地址（如 `.../portal#/login`），将参数写入 fragment 内查询串。
    pub fn build_oauth_login_url(redirect_uri: &str, state: &str) -> Result<String, AdapterError> {
        append_login_params(
            DEFAULT_PORTAL_LOGIN_URL,
            &[
                ("source", "electron"),
                ("redirect_uri", redirect_uri),
                ("state", state),
            ],
        )
    }

    /// 通过授权回调一次性 `authCode` 换取双令牌并构建可落盘凭据。
    pub async fn exchange_auth_code(code: &str) -> Result<CredentialEnvelope, AdapterError> {
        Self::new().exchange_auth_code_with_base(code).await
    }

    pub(crate) async fn exchange_auth_code_with_base(
        &self,
        code: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let clean = code.trim();
        if clean.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "LobsterAI 授权回调缺少 authCode".into(),
            });
        }
        let body = Self::keyfrom_body(serde_json::json!({ "authCode": clean }));
        let resp = self
            .post_json("/api/auth/exchange", None, &body, "交换 LobsterAI 登录令牌")
            .await?;
        let data = Self::check_api_envelope(&resp, "交换 LobsterAI 登录令牌")?;
        let access_token = data
            .get("accessToken")
            .and_then(non_empty_str)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "LobsterAI 授权响应缺少 accessToken".into(),
            })?;
        let refresh_token = data.get("refreshToken").and_then(non_empty_str);
        let user_from_exchange = data.get("user").filter(|v| v.is_object()).cloned();
        let user = match user_from_exchange {
            Some(u) => Some(u),
            None => self.fetch_user_profile(&access_token).await.ok(),
        };
        Self::build_envelope(access_token, refresh_token, user.as_ref())
    }

    fn db_path(host: &dyn PlatformHost) -> Result<PathBuf, AdapterError> {
        let dir = host
            .client_config_dir("lobsterai")
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: e.to_string(),
            })?;
        Ok(dir.join(LOBSTERAI_DB_FILE))
    }

    fn sidecar_paths(db_path: &Path) -> [PathBuf; 3] {
        let raw = db_path.as_os_str().to_string_lossy();
        [
            db_path.to_path_buf(),
            PathBuf::from(format!("{raw}-wal")),
            PathBuf::from(format!("{raw}-shm")),
        ]
    }

    fn read_kv_ro(db_path: &Path, key: &str) -> Result<Option<String>, AdapterError> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("打开 LobsterAI 数据库失败: {e}"),
        })?;
        let _ = conn.busy_timeout(Duration::from_millis(2000));
        conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("读取 LobsterAI 配置项 {key} 失败: {e}"),
        })
    }

    /// 仅解包 JWT payload（不校验服务端签名），用于读取标准 `exp` / `sub` / `yid` / `type`。
    fn decode_jwt_claims(token: &str) -> Option<Value> {
        let payload = token.split('.').nth(1)?;
        let bytes = BASE64_URL
            .decode(payload.trim_end_matches('=').as_bytes())
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn jwt_exp_secs(token: &str) -> Option<i64> {
        Self::decode_jwt_claims(token)
            .and_then(|v| v.get("exp").and_then(int_field))
            .map(normalize_epoch_secs)
    }

    fn extract_tokens(cred: &CredentialEnvelope) -> Result<LobsterTokens, AdapterError> {
        match &cred.payload {
            CredentialPayload::Token {
                access_token,
                refresh_token,
                expires_at,
                extra,
            } => {
                let at = access_token.trim();
                if at.is_empty() {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "该账号缺少有效的 LobsterAI accessToken，请重新登录或导入".into(),
                    });
                }
                Ok(LobsterTokens {
                    access_token: at.to_string(),
                    refresh_token: refresh_token
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    expires_at: expires_at.map(normalize_epoch_secs),
                    extra: extra.clone(),
                })
            }
            CredentialPayload::LocalSnapshot { files } => {
                let at = files
                    .get(SNAPSHOT_TOKEN_FILE)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "该账号快照缺少 LobsterAI 访问令牌，请重新登录或导入".into(),
                    })?;
                let rt = files
                    .get(SNAPSHOT_REFRESH_FILE)
                    .and_then(|b64| BASE64.decode(b64).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                Ok(LobsterTokens {
                    access_token: at,
                    refresh_token: rt,
                    expires_at: None,
                    extra: HashMap::new(),
                })
            }
            CredentialPayload::Cookies { .. } => Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "LobsterAI 不使用 Cookie 凭据，请通过本机导入、内置授权或粘贴 Token 添加"
                    .into(),
            }),
        }
    }

    fn keyfrom_body(mut extra: Value) -> Value {
        let obj = extra.as_object_mut();
        if let Some(map) = obj {
            map.insert(
                "firstKeyfrom".to_string(),
                Value::String("official".to_string()),
            );
            map.insert(
                "latestKeyfrom".to_string(),
                Value::String("official".to_string()),
            );
            map.insert(
                "version".to_string(),
                Value::String(CLIENT_VERSION.to_string()),
            );
        }
        extra
    }

    fn check_api_envelope<'a>(body: &'a Value, context: &str) -> Result<&'a Value, AdapterError> {
        let code = body.get("code").and_then(int_field);
        if code != Some(0) {
            if let Some(c) = code {
                return Err(Self::map_lobster_error_code(c, body, context));
            }
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("{context}：响应缺少业务状态码 code"),
            });
        }
        body.get("data").ok_or_else(|| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("{context}：响应缺少 data 字段"),
        })
    }

    fn map_lobster_error_code(code: i64, body: &Value, context: &str) -> AdapterError {
        if matches!(code, 401 | 40100 | 40101 | 51102) {
            return AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                message: format!("{context}：登录态已过期（业务码 {code}）"),
            };
        }
        if let Some(err) = map_business_code(code, context) {
            return err;
        }
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("未知业务错误");
        AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("{context}：业务码 {code}（{msg}）"),
        }
    }

    fn current_activity_platform() -> &'static str {
        #[cfg(target_os = "windows")]
        {
            "win32"
        }
        #[cfg(target_os = "macos")]
        {
            "darwin"
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            "linux"
        }
    }

    fn build_envelope(
        access_token: String,
        refresh_token: Option<String>,
        user_json: Option<&Value>,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let clean_access = access_token.trim().to_string();
        if clean_access.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "LobsterAI accessToken 不能为空".into(),
            });
        }
        let clean_refresh = refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let claims = Self::decode_jwt_claims(&clean_access);
        let uid = user_json
            .and_then(|u| u.get("id"))
            .and_then(json_uid_to_string)
            .or_else(|| {
                user_json
                    .and_then(|u| u.get("yid"))
                    .and_then(json_uid_to_string)
            })
            .or_else(|| {
                claims
                    .as_ref()
                    .and_then(|c| c.get("sub").or_else(|| c.get("yid")))
                    .and_then(json_uid_to_string)
            })
            .unwrap_or_else(|| hash_fallback_uid("lobsterai", clean_access.as_bytes()));

        let display_name = user_json
            .and_then(|u| {
                u.get("nickname")
                    .or_else(|| u.get("name"))
                    .or_else(|| u.get("username"))
            })
            .and_then(non_empty_str);

        let display_email = user_json
            .and_then(|u| u.get("email"))
            .and_then(non_empty_str)
            .filter(|e| e.contains('@'));

        let expires_at = Self::jwt_exp_secs(&clean_access);

        let mut extra = HashMap::new();
        if let Some(user_val) = user_json.filter(|v| v.is_object()) {
            if let Ok(serialized) = serde_json::to_string(user_val) {
                extra.insert(EXTRA_AUTH_USER_JSON.to_string(), serialized);
            }
        }
        if let Some(yid) = user_json
            .and_then(|u| u.get("yid"))
            .and_then(json_uid_to_string)
            .or_else(|| {
                claims
                    .as_ref()
                    .and_then(|c| c.get("yid"))
                    .and_then(json_uid_to_string)
            })
        {
            extra.insert(EXTRA_YID.to_string(), yid);
        }

        Ok(CredentialEnvelope {
            tool_id: "lobsterai".to_string(),
            uid,
            display_name,
            display_email,
            payload: CredentialPayload::Token {
                access_token: clean_access,
                refresh_token: clean_refresh,
                expires_at,
                extra,
            },
        })
    }

    async fn send_json_request(
        &self,
        method: reqwest::Method,
        path_and_query: &str,
        bearer: Option<&str>,
        json_body: Option<&Value>,
        context: &str,
    ) -> Result<Value, AdapterError> {
        let url = format!("{}{path_and_query}", self.server_base);
        let mut req = self
            .http
            .request(method, &url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header("Cache-Control", "no-store");
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        if let Some(body) = json_body {
            req = req.header(CONTENT_TYPE, "application/json").json(body);
        }

        let resp = req.send().await.map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::NetworkError,
            message: format!("{context}失败: {}", e.without_url()),
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(map_http_status(status, context));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: format!("解析{context}响应失败: {}", e.without_url()),
            })
    }

    async fn authed_get(
        &self,
        path_and_query: &str,
        token: &str,
        context: &str,
    ) -> Result<Value, AdapterError> {
        self.send_json_request(
            reqwest::Method::GET,
            path_and_query,
            Some(token),
            None,
            context,
        )
        .await
    }

    async fn post_json(
        &self,
        path_and_query: &str,
        token: Option<&str>,
        body: &Value,
        context: &str,
    ) -> Result<Value, AdapterError> {
        self.send_json_request(
            reqwest::Method::POST,
            path_and_query,
            token,
            Some(body),
            context,
        )
        .await
    }

    async fn fetch_user_profile(&self, token: &str) -> Result<Value, AdapterError> {
        let raw = self
            .authed_get("/api/user/profile", token, "查询 LobsterAI 用户信息")
            .await?;
        let data = Self::check_api_envelope(&raw, "查询 LobsterAI 用户信息")?;
        Ok(data.clone())
    }

    async fn request_token_refresh(
        &self,
        refresh_token: &str,
    ) -> Result<(String, Option<String>), AdapterError> {
        let body = Self::keyfrom_body(serde_json::json!({ "refreshToken": refresh_token }));
        let resp = self
            .post_json("/api/auth/refresh", None, &body, "刷新 LobsterAI 令牌")
            .await?;
        let data = Self::check_api_envelope(&resp, "刷新 LobsterAI 令牌")?;
        let next_access = data
            .get("accessToken")
            .and_then(non_empty_str)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "刷新 LobsterAI 令牌响应缺少 accessToken".into(),
            })?;
        let next_refresh = data.get("refreshToken").and_then(non_empty_str);
        Ok((next_access, next_refresh))
    }

    async fn fetch_quota_data(&self, token: &str) -> Result<Value, AdapterError> {
        let raw = self
            .authed_get("/api/user/quota", token, "查询 LobsterAI 额度")
            .await?;
        Ok(Self::check_api_envelope(&raw, "查询 LobsterAI 额度")?.clone())
    }

    async fn fetch_daily_checkin_slot(
        &self,
        token: &str,
    ) -> Result<Option<(String, i64)>, AdapterError> {
        let platform = Self::current_activity_platform();
        let query_url = reqwest::Url::parse_with_params(
            &format!("{}/api/client-activities/slot", self.server_base),
            &[
                ("placement", "desktop_sidebar"),
                ("clientVersion", CLIENT_VERSION),
                ("containerApiVersion", "2"),
                ("platform", platform),
            ],
        )
        .map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("构造活动槽位查询参数失败: {e}"),
        })?;
        let path_q = match query_url.query() {
            Some(q) => format!("{}?{q}", query_url.path()),
            None => query_url.path().to_string(),
        };
        let raw = self
            .authed_get(&path_q, token, "查询 LobsterAI 签到活动")
            .await?;
        let data = Self::check_api_envelope(&raw, "查询 LobsterAI 签到活动")?;
        if data.get("slotState").and_then(|v| v.as_str()) != Some("available") {
            return Ok(None);
        }
        let Some(activity) = data.get("activity").filter(|v| v.is_object()) else {
            return Ok(None);
        };
        if activity.get("activityType").and_then(|v| v.as_str()) != Some("daily_check_in") {
            return Ok(None);
        }
        let Some(code) = activity.get("activityCode").and_then(non_empty_str) else {
            return Ok(None);
        };
        if !is_valid_path_segment(&code) {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "LobsterAI 返回了非法的活动标识，已中止请求".into(),
            });
        }
        let Some(revision) = activity
            .get("configRevision")
            .and_then(int_field)
            .filter(|r| *r > 0)
        else {
            return Ok(None);
        };
        Ok(Some((code, revision)))
    }

    async fn fetch_daily_checkin_context(
        &self,
        token: &str,
        activity_code: &str,
        config_revision: i64,
    ) -> Result<Value, AdapterError> {
        let path = format!(
            "/api/client-activities/{activity_code}/context?configRevision={config_revision}"
        );
        let raw = self
            .authed_get(&path, token, "查询 LobsterAI 签到上下文")
            .await?;
        Ok(Self::check_api_envelope(&raw, "查询 LobsterAI 签到上下文")?.clone())
    }

    async fn probe_daily_checkin_status(&self, token: &str) -> Result<ClaimStatus, AdapterError> {
        let Some((activity_code, revision)) = self.fetch_daily_checkin_slot(token).await? else {
            return Ok(ClaimStatus::Unsupported);
        };
        let context = self
            .fetch_daily_checkin_context(token, &activity_code, revision)
            .await?;
        Ok(Self::claim_status_from_context(&context))
    }

    fn claim_status_from_context(context: &Value) -> ClaimStatus {
        let Some(state) = context.get("state").filter(|v| v.is_object()) else {
            return ClaimStatus::Unknown;
        };
        if state
            .get("claimedToday")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return ClaimStatus::Claimed;
        }
        let lifecycle = context.get("lifecycleState").and_then(|v| v.as_str());
        let completed = state
            .get("completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if lifecycle != Some("active") || completed {
            return ClaimStatus::Unsupported;
        }
        let can_check_in = context
            .get("actions")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| arr.iter().any(|item| item.as_str() == Some("check_in")));
        if can_check_in {
            ClaimStatus::Available
        } else {
            ClaimStatus::Unknown
        }
    }

    fn build_quota_snapshot(raw_quota: &Value, claim_status: ClaimStatus) -> QuotaSnapshot {
        let mut total = 0.0;
        let mut used = 0.0;
        let mut explicit_remaining: Option<f64> = None;
        let mut plan_name = String::from("免费版");

        let sub_status = raw_quota
            .get("subscriptionStatus")
            .and_then(|v| v.as_str())
            .map(str::to_ascii_lowercase);
        let default_status_plan = match sub_status.as_deref() {
            Some("enterprise") => "团队版",
            Some("active") => "订阅版",
            _ => "免费版",
        };

        if let Some(limit) = opt_amount_field(raw_quota.get("limit")) {
            total = limit;
            used = amount_field(raw_quota.get("used"));
            plan_name = raw_quota
                .get("planName")
                .and_then(non_empty_str)
                .unwrap_or_else(|| "团队版".to_string());
        } else if let Some(free_total) =
            opt_amount_field(raw_quota.get("freeCreditsTotal")).filter(|&v| v > 0.0)
        {
            total = free_total;
            used = amount_field(raw_quota.get("freeCreditsUsed"));
            plan_name = raw_quota
                .get("planName")
                .and_then(non_empty_str)
                .unwrap_or_else(|| "免费版".to_string());
        } else if let Some(monthly_limit) = opt_amount_field(raw_quota.get("monthlyCreditsLimit")) {
            total = monthly_limit;
            used = amount_field(raw_quota.get("monthlyCreditsUsed"));
            plan_name = raw_quota
                .get("planName")
                .and_then(non_empty_str)
                .unwrap_or_else(|| "订阅版".to_string());
        } else if let Some(daily_limit) = opt_amount_field(raw_quota.get("dailyCreditsLimit")) {
            total = daily_limit;
            used = amount_field(raw_quota.get("dailyCreditsUsed"));
            plan_name = raw_quota
                .get("planName")
                .and_then(non_empty_str)
                .unwrap_or_else(|| "免费版".to_string());
        } else if let Some(credits_limit) = opt_amount_field(raw_quota.get("creditsLimit")) {
            total = credits_limit;
            used = amount_field(raw_quota.get("creditsUsed"));
            explicit_remaining = opt_amount_field(raw_quota.get("creditsRemaining"));
            plan_name = raw_quota
                .get("planName")
                .and_then(non_empty_str)
                .unwrap_or_else(|| default_status_plan.to_string());
        } else if let Some(raw_plan) = raw_quota.get("planName").and_then(non_empty_str) {
            plan_name = raw_plan;
        }

        let remaining = explicit_remaining
            .unwrap_or_else(|| (total - used).max(0.0))
            .max(0.0);

        let reset_at = [
            "expiresAt",
            "resetAt",
            "periodEnd",
            "freeCreditsExpiresAt",
            "nextResetAt",
        ]
        .iter()
        .find_map(|key| parse_timestamp_rfc3339(raw_quota.get(*key)));

        QuotaSnapshot {
            plan_name: Some(plan_name),
            claim_status,
            buckets: vec![QuotaBucket {
                key: "credits".to_string(),
                label: "可用积分".to_string(),
                used,
                total,
                remaining,
                unit: "积分".to_string(),
                reset_at,
            }],
            fetched_at: Utc::now().to_rfc3339(),
        }
    }

    async fn post_checkin_action(
        &self,
        token: &str,
        activity_code: &str,
        config_revision: i64,
    ) -> Result<Value, AdapterError> {
        let path = format!("/api/client-activities/{activity_code}/actions/check_in");
        let body = serde_json::json!({
            "configRevision": config_revision,
            "idempotencyKey": Uuid::new_v4().to_string(),
            "payload": {}
        });
        let raw = self
            .post_json(&path, Some(token), &body, "领取 LobsterAI 每日签到积分")
            .await?;
        Ok(raw)
    }

    fn format_claim_reward_summary(data: &Value) -> String {
        let credits = opt_amount_field(
            data.pointer("/result/creditsGranted")
                .or_else(|| data.pointer("/context/state/rewardCredits")),
        )
        .filter(|c| *c > 0.0);
        let claimed_days = data
            .pointer("/context/state/claimedDays")
            .and_then(int_field);
        let total_days = data.pointer("/context/state/totalDays").and_then(int_field);

        let base = match credits {
            Some(c) if (c.fract()).abs() < f64::EPSILON => {
                format!("每日签到成功 +{} 积分", c as i64)
            }
            Some(c) => format!("每日签到成功 +{c:.2} 积分"),
            None => "每日签到已完成".to_string(),
        };
        match (claimed_days, total_days) {
            (Some(d), Some(t)) if t > 0 => format!("{base}（第 {d}/{t} 天）"),
            _ => base,
        }
    }
}

impl Default for LobsterAiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_epoch_secs(ts: i64) -> i64 {
    if ts >= 1_000_000_000_000 {
        ts / 1000
    } else {
        ts
    }
}

fn int_field(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

fn opt_amount_field(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    let val = v
        .as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))?;
    val.is_finite().then_some(val)
}

fn amount_field(v: Option<&Value>) -> f64 {
    opt_amount_field(v).unwrap_or(0.0)
}

fn non_empty_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_timestamp_rfc3339(v: Option<&Value>) -> Option<String> {
    let val = v?;
    if let Some(s) = val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc).to_rfc3339());
        }
        if let Ok(n) = s.parse::<i64>() {
            return epoch_num_to_rfc3339(n);
        }
        return None;
    }
    int_field(val).and_then(epoch_num_to_rfc3339)
}

fn epoch_num_to_rfc3339(n: i64) -> Option<String> {
    if n <= 0 {
        return None;
    }
    if n >= 1_000_000_000_000 {
        Utc.timestamp_millis_opt(n)
            .single()
            .map(|dt| dt.to_rfc3339())
    } else {
        Utc.timestamp_opt(n, 0).single().map(|dt| dt.to_rfc3339())
    }
}

fn strip_bearer_prefix(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("bearer ") {
        trimmed[7..].trim()
    } else {
        trimmed
    }
}

fn finalize_claimed_quota(
    inspected: Option<QuotaSnapshot>,
    precheck: Option<&QuotaSnapshot>,
) -> Option<QuotaSnapshot> {
    let mut quota = merge_post_claim_quota(inspected, precheck, true);
    if let Some(ref mut q) = quota {
        // LobsterAI 每日签到每人每天仅一次：确认领取或已签后即使后端活动上下文缓存延迟，也强制收口为 Claimed
        q.claim_status = ClaimStatus::Claimed;
    }
    quota
}

#[async_trait]
impl ToolAdapter for LobsterAiAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    fn detect_local_active_uid(&self, host: &dyn PlatformHost) -> Option<String> {
        let db_path = Self::db_path(host).ok()?;
        if !db_path.is_file() {
            return None;
        }
        if let Ok(Some(raw_user)) = Self::read_kv_ro(&db_path, KV_AUTH_USER) {
            if let Ok(user_val) = serde_json::from_str::<Value>(&raw_user) {
                if let Some(uid) = user_val
                    .get("id")
                    .and_then(json_uid_to_string)
                    .or_else(|| user_val.get("yid").and_then(json_uid_to_string))
                {
                    if !CredentialEnvelope::is_fallback_uid(&uid) {
                        return Some(uid);
                    }
                }
            }
        }
        if let Ok(Some(raw_tokens)) = Self::read_kv_ro(&db_path, KV_AUTH_TOKENS) {
            if let Ok(tokens_val) = serde_json::from_str::<Value>(&raw_tokens) {
                if let Some(at) = tokens_val.get("accessToken").and_then(non_empty_str) {
                    if let Some(claims) = Self::decode_jwt_claims(&at) {
                        if let Some(uid) = claims
                            .get("sub")
                            .or_else(|| claims.get("yid"))
                            .and_then(json_uid_to_string)
                        {
                            if !CredentialEnvelope::is_fallback_uid(&uid) {
                                return Some(uid);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let db_path = Self::db_path(host)?;
        if !db_path.is_file() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: "未检测到 LobsterAI 本地数据库，请先安装并登录一次客户端".into(),
            });
        }

        let raw_tokens =
            Self::read_kv_ro(&db_path, KV_AUTH_TOKENS)?.ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "LobsterAI 客户端尚未登录，请先在客户端登录后再导入".into(),
            })?;
        let tokens_val: Value =
            serde_json::from_str(&raw_tokens).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("LobsterAI 本机凭据格式无效: {e}"),
            })?;
        let access_token = tokens_val
            .get("accessToken")
            .and_then(non_empty_str)
            .ok_or_else(|| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "LobsterAI 本机凭据缺少 accessToken，请先在客户端重新登录".into(),
            })?;
        let refresh_token = tokens_val.get("refreshToken").and_then(non_empty_str);

        let local_user = Self::read_kv_ro(&db_path, KV_AUTH_USER)?
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(|v| v.is_object());

        let user_json = match local_user {
            Some(u) => Some(u),
            None => self.fetch_user_profile(&access_token).await.ok(),
        };

        Self::build_envelope(access_token, refresh_token, user_json.as_ref())
    }

    async fn parse_manual_token(
        &self,
        raw_input: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let trimmed = raw_input.trim();
        if trimmed.is_empty() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: "LobsterAI Token 或 JSON 不能为空".into(),
            });
        }

        let (mut access_token, mut refresh_token, local_user) = if trimmed.starts_with('{') {
            let val: Value = serde_json::from_str(trimmed).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("JSON 格式无效: {e}"),
            })?;
            let access = val
                .get("accessToken")
                .or_else(|| val.get("access_token"))
                .or_else(|| val.get("token"))
                .and_then(non_empty_str);
            let refresh = val
                .get("refreshToken")
                .or_else(|| val.get("refresh_token"))
                .and_then(non_empty_str);
            let user = val.get("user").filter(|v| v.is_object()).cloned();
            let (at, rt) = match (access, refresh) {
                (Some(at), rt) => (at, rt),
                (None, Some(rt)) => {
                    let (new_at, rotated_rt) = self.request_token_refresh(&rt).await?;
                    (new_at, Some(rotated_rt.unwrap_or(rt)))
                }
                (None, None) => {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "JSON 中缺少 accessToken 或 refreshToken 字段".into(),
                    });
                }
            };
            (at, rt, user)
        } else {
            let clean = strip_bearer_prefix(trimmed);
            if clean.is_empty() {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InvalidCredential,
                    message: "LobsterAI Token 不能为空".into(),
                });
            }
            let is_refresh_jwt = Self::decode_jwt_claims(clean)
                .and_then(|c| c.get("type").and_then(|v| v.as_str()).map(str::to_string))
                .is_some_and(|t| t == "refresh");
            if is_refresh_jwt {
                let (new_at, rotated_rt) = self.request_token_refresh(clean).await?;
                (
                    new_at,
                    Some(rotated_rt.unwrap_or_else(|| clean.to_string())),
                    None,
                )
            } else {
                (clean.to_string(), None, None)
            }
        };

        // 用服务端 profile 校验凭据并补全权威用户信息
        let fetched_user = match self.fetch_user_profile(&access_token).await {
            Ok(u) => Some(u),
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }) => {
                // 若同时提供了 refreshToken，则尝试续期后再校验一次
                if let Some(rt) = refresh_token.clone() {
                    let (new_at, rotated_rt) =
                        self.request_token_refresh(&rt).await.map_err(|e| match e {
                            AdapterError::Unified {
                                code: UnifiedErrorCode::AuthExpired,
                                ..
                            } => AdapterError::Unified {
                                code: UnifiedErrorCode::InvalidCredential,
                                message: "LobsterAI Token 无效或已过期，请检查后重试".into(),
                            },
                            other => other,
                        })?;
                    access_token = new_at;
                    refresh_token = Some(rotated_rt.unwrap_or(rt));
                    Some(
                        self.fetch_user_profile(&access_token)
                            .await
                            .map_err(|e| match e {
                                AdapterError::Unified {
                                    code: UnifiedErrorCode::AuthExpired,
                                    ..
                                } => AdapterError::Unified {
                                    code: UnifiedErrorCode::InvalidCredential,
                                    message: "LobsterAI Token 无效或已过期，请检查后重试".into(),
                                },
                                other => other,
                            })?,
                    )
                } else {
                    return Err(AdapterError::Unified {
                        code: UnifiedErrorCode::InvalidCredential,
                        message: "LobsterAI Token 无效或已过期，请检查后重试".into(),
                    });
                }
            }
            Err(err) => return Err(err),
        };

        let user = fetched_user.or(local_user);
        Self::build_envelope(access_token, refresh_token, user.as_ref())
    }

    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError> {
        let tokens = Self::extract_tokens(cred)?;
        let raw_quota = self.fetch_quota_data(&tokens.access_token).await?;

        // 活动槽位探测属于附加状态：认证/风控/限流错误向上熔断，普通网络波动退化为 Unknown
        let claim_status = match self.probe_daily_checkin_status(&tokens.access_token).await {
            Ok(status) => status,
            Err(
                e @ AdapterError::Unified {
                    code:
                        UnifiedErrorCode::AuthExpired
                        | UnifiedErrorCode::RiskTriggered
                        | UnifiedErrorCode::RateLimited,
                    ..
                },
            ) => return Err(e),
            Err(_) => ClaimStatus::Unknown,
        };

        Ok(Self::build_quota_snapshot(&raw_quota, claim_status))
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        let tokens = Self::extract_tokens(cred)?;

        // 前置已完成核实且今日已签/无活动时直接幂等返回
        if let Some(q) = precheck {
            match q.claim_status {
                ClaimStatus::Claimed => {
                    return Ok(ClaimResult {
                        claimed: false,
                        reward_summary: "今日已签过".into(),
                        updated_quota: Some(q.clone()),
                    });
                }
                ClaimStatus::Unsupported => {
                    return Ok(ClaimResult {
                        claimed: false,
                        reward_summary: "当前账号暂无可用的每日签到活动".into(),
                        updated_quota: None,
                    });
                }
                ClaimStatus::Available | ClaimStatus::Unknown => {}
            }
        }

        let Some((mut activity_code, mut config_revision)) =
            self.fetch_daily_checkin_slot(&tokens.access_token).await?
        else {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "当前账号暂无可用的每日签到活动".into(),
                updated_quota: self.inspect_quota(cred).await.ok(),
            });
        };

        let context = self
            .fetch_daily_checkin_context(&tokens.access_token, &activity_code, config_revision)
            .await?;
        let state = context.get("state").filter(|v| v.is_object());
        if state
            .and_then(|s| s.get("claimedToday"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "今日已签过".into(),
                updated_quota: finalize_claimed_quota(
                    self.inspect_quota(cred).await.ok(),
                    precheck,
                ),
            });
        }
        if state
            .and_then(|s| s.get("completed"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || context.get("lifecycleState").and_then(|v| v.as_str()) != Some("active")
        {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "本期签到天数已全部领完或活动未开启".into(),
                updated_quota: self.inspect_quota(cred).await.ok(),
            });
        }

        let mut raw_claim = self
            .post_checkin_action(&tokens.access_token, &activity_code, config_revision)
            .await?;
        let mut code = raw_claim.get("code").and_then(int_field);

        // 51106 RevisionMismatch：重拉一次 slot 后重试一次
        if code == Some(51106) {
            if let Some((fresh_code, fresh_rev)) =
                self.fetch_daily_checkin_slot(&tokens.access_token).await?
            {
                activity_code = fresh_code;
                config_revision = fresh_rev;
                raw_claim = self
                    .post_checkin_action(&tokens.access_token, &activity_code, config_revision)
                    .await?;
                code = raw_claim.get("code").and_then(int_field);
            }
        }

        if code == Some(0) {
            let data = raw_claim.get("data").unwrap_or(&Value::Null);
            let reward_summary = Self::format_claim_reward_summary(data);
            let refreshed = finalize_claimed_quota(self.inspect_quota(cred).await.ok(), precheck);
            return Ok(ClaimResult {
                claimed: true,
                reward_summary,
                updated_quota: refreshed,
            });
        }

        let msg = raw_claim
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if code == Some(51104) || is_idempotent_claimed_phrase(msg) {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "今日已签过".into(),
                updated_quota: finalize_claimed_quota(
                    self.inspect_quota(cred).await.ok(),
                    precheck,
                ),
            });
        }

        if matches!(code, Some(51100 | 51101)) {
            return Ok(ClaimResult {
                claimed: false,
                reward_summary: "当前账号暂无可用的每日签到活动".into(),
                updated_quota: self.inspect_quota(cred).await.ok(),
            });
        }

        match code {
            Some(c) => Err(Self::map_lobster_error_code(
                c,
                &raw_claim,
                "领取 LobsterAI 每日签到积分",
            )),
            None => Err(AdapterError::Unified {
                code: UnifiedErrorCode::NetworkError,
                message: "领取 LobsterAI 每日签到响应缺少业务状态码".into(),
            }),
        }
    }

    async fn refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        let tokens = match Self::extract_tokens(cred) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        let Some(rt) = tokens.refresh_token.clone().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };

        let exp_secs = tokens
            .expires_at
            .or_else(|| Self::jwt_exp_secs(&tokens.access_token));
        let now_secs = Utc::now().timestamp();
        if let Some(exp) = exp_secs {
            if exp - now_secs > REFRESH_SKEW_SECS {
                return Ok(None);
            }
        } else {
            // 无 exp 时不盲目提前刷新
            return Ok(None);
        }

        let (next_access, rotated_refresh) = self.request_token_refresh(&rt).await?;
        let next_refresh = rotated_refresh.unwrap_or(rt);
        let next_expires_at = Self::jwt_exp_secs(&next_access);

        Ok(Some(CredentialEnvelope {
            tool_id: "lobsterai".to_string(),
            uid: cred.uid.clone(),
            display_name: cred.display_name.clone(),
            display_email: cred.display_email.clone(),
            payload: CredentialPayload::Token {
                access_token: next_access,
                refresh_token: Some(next_refresh),
                expires_at: next_expires_at,
                extra: tokens.extra,
            },
        }))
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError> {
        let tokens = Self::extract_tokens(cred)?;
        let db_path = Self::db_path(host)?;
        if !db_path.is_file() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::ClientNotFound,
                message: "未检测到 LobsterAI 本地数据库，请先启动一次客户端".into(),
            });
        }

        let auth_tokens_json = serde_json::json!({
            "accessToken": tokens.access_token,
            "refreshToken": tokens.refresh_token.unwrap_or_default(),
        })
        .to_string();

        let auth_user_val = tokens
            .extra
            .get(EXTRA_AUTH_USER_JSON)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .filter(|v| v.is_object())
            .unwrap_or_else(|| {
                let id_val = cred
                    .uid
                    .parse::<i64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::String(cred.uid.clone()));
                let mut map = Map::new();
                map.insert("id".to_string(), id_val);
                map.insert(
                    "nickname".to_string(),
                    Value::String(
                        cred.display_name
                            .clone()
                            .unwrap_or_else(|| cred.uid.clone()),
                    ),
                );
                map.insert("status".to_string(), Value::String("active".to_string()));
                if let Some(yid) = tokens.extra.get(EXTRA_YID) {
                    map.insert("yid".to_string(), Value::String(yid.clone()));
                }
                Value::Object(map)
            });
        let auth_user_json = auth_user_val.to_string();

        let mut conn = Connection::open(&db_path).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("打开 LobsterAI 数据库失败: {e}"),
        })?;
        let _ = conn.busy_timeout(Duration::from_millis(3000));

        let now_ms = Utc::now().timestamp_millis();
        let tx = conn.transaction().map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("开启 LobsterAI 数据库事务失败: {e}"),
        })?;
        for (key, value) in [
            (KV_AUTH_TOKENS, auth_tokens_json),
            (KV_AUTH_USER, auth_user_json),
        ] {
            tx.execute(
                "INSERT INTO kv (key, value, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET
                   value = excluded.value,
                   updated_at = excluded.updated_at",
                rusqlite::params![key, value, now_ms],
            )
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("写入 LobsterAI 会话项 {key} 失败: {e}"),
            })?;
        }
        tx.execute("DELETE FROM kv WHERE key = ?1", [KV_ENTERPRISE_CONTEXT])
            .map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("清理 LobsterAI 企业上下文失败: {e}"),
            })?;
        tx.commit().map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("提交 LobsterAI 会话事务失败: {e}"),
        })?;

        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        Ok(())
    }

    async fn backup_session(&self, host: &dyn PlatformHost) -> Result<SessionBackup, AdapterError> {
        let db_path = Self::db_path(host)?;
        let paths = Self::sidecar_paths(&db_path);
        crate::adapter::backup_files(&paths)
    }
}

fn set_url_query_params(url: &mut reqwest::Url, params: &[(&str, &str)]) {
    let mut merged: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !params.iter().any(|(pk, _)| *pk == k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    for (k, v) in params {
        merged.push(((*k).to_string(), (*v).to_string()));
    }
    url.set_query(None);
    if !merged.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in &merged {
            pairs.append_pair(k, v);
        }
    }
}

fn append_login_params(base_url: &str, params: &[(&str, &str)]) -> Result<String, AdapterError> {
    let mut parsed = reqwest::Url::parse(base_url).map_err(|e| AdapterError::Unified {
        code: UnifiedErrorCode::InternalError,
        message: format!("构造 LobsterAI 授权链接失败: {e}"),
    })?;
    if let Some(frag) = parsed
        .fragment()
        .filter(|f| !f.is_empty())
        .map(str::to_string)
    {
        let (hash_path, hash_query) = frag.split_once('?').unwrap_or((&frag, ""));
        let mut dummy = reqwest::Url::parse("https://placeholder.local/").map_err(|e| {
            AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("构造 LobsterAI 授权链接参数失败: {e}"),
            }
        })?;
        if !hash_query.is_empty() {
            dummy.set_query(Some(hash_query));
        }
        set_url_query_params(&mut dummy, params);
        let next_fragment = match dummy.query().filter(|q| !q.is_empty()) {
            Some(q) => format!("{hash_path}?{q}"),
            None => hash_path.to_string(),
        };
        parsed.set_fragment(Some(&next_fragment));
    } else {
        set_url_query_params(&mut parsed, params);
    }
    Ok(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{KeyExpectation, ScopedPlatformHost};
    use async_trait::async_trait;
    use std::sync::Arc;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct DummyHost {
        root: PathBuf,
    }

    #[async_trait]
    impl PlatformHost for DummyHost {
        fn qwitch_data_dir(&self) -> Result<PathBuf, crate::platform::HostError> {
            Ok(self.root.join("qwitch"))
        }

        fn client_config_dir(&self, tool_id: &str) -> Result<PathBuf, crate::platform::HostError> {
            Ok(self.root.join(tool_id))
        }

        fn detect_client_executable(&self, _tool_id: &str) -> Option<PathBuf> {
            None
        }

        fn is_client_running(&self, _tool_id: &str) -> Result<bool, crate::platform::HostError> {
            Ok(false)
        }

        async fn terminate_client_gracefully(
            &self,
            _tool_id: &str,
            _timeout_ms: u64,
        ) -> Result<(), crate::platform::HostError> {
            Ok(())
        }

        fn launch_client(
            &self,
            _executable: &Path,
            _args: &[String],
        ) -> Result<(), crate::platform::HostError> {
            Ok(())
        }

        fn get_or_create_master_key(
            &self,
            _expectation: &KeyExpectation,
        ) -> Result<[u8; 32], crate::platform::HostError> {
            Ok([0u8; 32])
        }

        fn set_launch_at_login(&self, _enabled: bool) -> Result<(), String> {
            Ok(())
        }
    }

    fn make_jwt(claims: Value) -> String {
        let hdr = BASE64_URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let body = BASE64_URL.encode(serde_json::to_vec(&claims).unwrap());
        format!("{hdr}.{body}.sig")
    }

    fn seed_lobster_sqlite(dir: &Path, tokens: Option<&Value>, user: Option<&Value>) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let db_path = dir.join(LOBSTERAI_DB_FILE);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE kv (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE TABLE cowork_sessions (id TEXT PRIMARY KEY, title TEXT);
             INSERT INTO cowork_sessions (id, title) VALUES ('sess-1', '保留聊天记录');",
        )
        .unwrap();
        if let Some(t) = tokens {
            conn.execute(
                "INSERT INTO kv (key, value, updated_at) VALUES (?1, ?2, 1000)",
                rusqlite::params![KV_AUTH_TOKENS, t.to_string()],
            )
            .unwrap();
        }
        if let Some(u) = user {
            conn.execute(
                "INSERT INTO kv (key, value, updated_at) VALUES (?1, ?2, 1000)",
                rusqlite::params![KV_AUTH_USER, u.to_string()],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO kv (key, value, updated_at) VALUES (?1, '{\"orgId\":9}', 1000)",
            rusqlite::params![KV_ENTERPRISE_CONTEXT],
        )
        .unwrap();
        db_path
    }

    #[tokio::test]
    async fn sqlite_detect_import_backup_apply_and_restore_roundtrip() {
        let root = std::env::temp_dir().join(format!("qwitch_lobster_sqlite_{}", Uuid::new_v4()));
        let host: Arc<dyn PlatformHost> = Arc::new(DummyHost { root: root.clone() });
        let scoped = ScopedPlatformHost::new(Arc::clone(&host), "lobsterai", root.join("client"));

        let exp = Utc::now().timestamp() + 3600;
        let at_a = make_jwt(
            serde_json::json!({ "sub": "10001", "yid": "y_10001", "type": "access", "exp": exp }),
        );
        let rt_a =
            make_jwt(serde_json::json!({ "sub": "10001", "type": "refresh", "exp": exp + 86400 }));
        let user_a = serde_json::json!({
            "id": 10001,
            "yid": "y_10001",
            "nickname": "龙虾一号",
            "avatarUrl": "https://example.com/a.png",
            "status": "active"
        });
        let db_path = seed_lobster_sqlite(
            &root.join("client"),
            Some(&serde_json::json!({ "accessToken": at_a, "refreshToken": rt_a })),
            Some(&user_a),
        );

        let adapter = LobsterAiAdapter::new();
        assert_eq!(
            adapter.detect_local_active_uid(&scoped).as_deref(),
            Some("10001")
        );

        let imported = adapter.import_from_local(&scoped).await.expect("import");
        assert_eq!(imported.tool_id, "lobsterai");
        assert_eq!(imported.uid, "10001");
        assert_eq!(imported.display_name.as_deref(), Some("龙虾一号"));
        assert_eq!(imported.capability_flags(), (true, true));

        let backup = adapter.backup_session(&scoped).await.expect("backup");
        assert_eq!(backup.files.len(), 3);

        // 切换到账号 B
        let at_b = make_jwt(
            serde_json::json!({ "sub": "20002", "yid": "y_20002", "type": "access", "exp": exp }),
        );
        let rt_b =
            make_jwt(serde_json::json!({ "sub": "20002", "type": "refresh", "exp": exp + 86400 }));
        let user_b = serde_json::json!({ "id": 20002, "yid": "y_20002", "nickname": "龙虾二号", "status": "active" });
        let cred_b =
            LobsterAiAdapter::build_envelope(at_b.clone(), Some(rt_b.clone()), Some(&user_b))
                .expect("cred_b");

        adapter
            .apply_session(&scoped, &cred_b)
            .await
            .expect("apply_session");
        assert_eq!(
            adapter.detect_local_active_uid(&scoped).as_deref(),
            Some("20002")
        );

        // 验证其它业务表未被破坏且 enterprise_account_context 已清理
        let conn = Connection::open(&db_path).unwrap();
        let session_title: String = conn
            .query_row(
                "SELECT title FROM cowork_sessions WHERE id = 'sess-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(session_title, "保留聊天记录");
        let ent_ctx: Option<String> = conn
            .query_row(
                "SELECT value FROM kv WHERE key = ?1",
                [KV_ENTERPRISE_CONTEXT],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert!(ent_ctx.is_none());
        drop(conn);

        // 回滚恢复到账号 A
        adapter
            .restore_session(&scoped, &backup)
            .await
            .expect("restore_session");
        assert_eq!(
            adapter.detect_local_active_uid(&scoped).as_deref(),
            Some("10001")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn parse_manual_token_supports_json_access_jwt_refresh_jwt_and_rejects_expired() {
        let server = MockServer::start().await;
        let adapter = LobsterAiAdapter::with_base_url(server.uri());
        let exp = Utc::now().timestamp() + 3600;
        let access_jwt =
            make_jwt(serde_json::json!({ "sub": "30003", "type": "access", "exp": exp }));
        let refresh_jwt =
            make_jwt(serde_json::json!({ "sub": "30003", "type": "refresh", "exp": exp + 86400 }));

        Mock::given(method("GET"))
            .and(path("/api/user/profile"))
            .and(header(
                "authorization",
                format!("Bearer {access_jwt}").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "id": 30003, "yid": "y_30003", "nickname": "测试虾" }
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/auth/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "accessToken": access_jwt,
                    "refreshToken": "rt-rotated"
                }
            })))
            .mount(&server)
            .await;

        // 1. JSON auth_tokens
        let json_env = adapter
            .parse_manual_token(
                &serde_json::json!({ "accessToken": access_jwt, "refreshToken": refresh_jwt })
                    .to_string(),
            )
            .await
            .expect("json paste");
        assert_eq!(json_env.uid, "30003");
        assert_eq!(json_env.display_name.as_deref(), Some("测试虾"));

        // 2. 裸 accessToken
        let bearer_env = adapter
            .parse_manual_token(&format!("Bearer {access_jwt}"))
            .await
            .expect("bearer paste");
        assert_eq!(bearer_env.uid, "30003");

        // 3. 裸 refreshToken 自动先换取 accessToken
        let rt_env = adapter
            .parse_manual_token(&refresh_jwt)
            .await
            .expect("refresh jwt paste");
        assert_eq!(rt_env.uid, "30003");
        if let CredentialPayload::Token { refresh_token, .. } = rt_env.payload {
            assert_eq!(refresh_token.as_deref(), Some("rt-rotated"));
        } else {
            panic!("expected Token payload");
        }

        // 4. 失效 Token 返回 InvalidCredential
        Mock::given(method("GET"))
            .and(path("/api/user/profile"))
            .and(header("authorization", "Bearer bad-token"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let err = adapter.parse_manual_token("bad-token").await.unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn inspect_quota_handles_all_quota_shapes_and_activity_states() {
        let server = MockServer::start().await;
        let adapter = LobsterAiAdapter::with_base_url(server.uri());
        let exp = Utc::now().timestamp() + 3600;
        let cred = LobsterAiAdapter::build_envelope(
            make_jwt(serde_json::json!({ "sub": "5001", "exp": exp })),
            Some("rt-5001".into()),
            None,
        )
        .unwrap();

        // Case 1: Free credits + available daily check-in
        Mock::given(method("GET"))
            .and(path("/api/user/quota"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "freeCreditsTotal": 300,
                    "freeCreditsUsed": 120,
                    "subscriptionStatus": "free",
                    "freeCreditsExpiresAt": "2026-12-06T00:00:00Z"
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/client-activities/slot"))
            .and(query_param("placement", "desktop_sidebar"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "slotState": "available",
                    "activity": {
                        "placement": "desktop_sidebar",
                        "activityType": "daily_check_in",
                        "activityCode": "daily_checkin_2026",
                        "configRevision": 3
                    }
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/client-activities/daily_checkin_2026/context"))
            .and(query_param("configRevision", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "activityCode": "daily_checkin_2026",
                    "configRevision": 3,
                    "lifecycleState": "active",
                    "state": {
                        "totalDays": 30,
                        "claimedDays": 4,
                        "remainingDays": 26,
                        "claimedToday": false,
                        "completed": false,
                        "rewardCredits": 100
                    },
                    "actions": ["check_in"]
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let q1 = adapter.inspect_quota(&cred).await.expect("quota free");
        assert_eq!(q1.plan_name.as_deref(), Some("免费版"));
        assert_eq!(q1.claim_status, ClaimStatus::Available);
        assert_eq!(q1.buckets[0].total, 300.0);
        assert_eq!(q1.buckets[0].used, 120.0);
        assert_eq!(q1.buckets[0].remaining, 180.0);
        assert!(q1.buckets[0].reset_at.is_some());

        // Case 2: Monthly subscription + no active slot -> Unsupported
        Mock::given(method("GET"))
            .and(path("/api/user/quota"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "monthlyCreditsLimit": 3000,
                    "monthlyCreditsUsed": 500,
                    "planName": "Pro 月卡",
                    "subscriptionStatus": "active"
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/client-activities/slot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "slotState": "empty" }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let q2 = adapter.inspect_quota(&cred).await.expect("quota monthly");
        assert_eq!(q2.plan_name.as_deref(), Some("Pro 月卡"));
        assert_eq!(q2.claim_status, ClaimStatus::Unsupported);
        assert_eq!(q2.buckets[0].remaining, 2500.0);

        // Case 3: Generic creditsLimit / Team limit
        let team_snapshot = LobsterAiAdapter::build_quota_snapshot(
            &serde_json::json!({ "limit": 10000, "used": 250 }),
            ClaimStatus::Claimed,
        );
        assert_eq!(team_snapshot.plan_name.as_deref(), Some("团队版"));
        assert_eq!(team_snapshot.buckets[0].remaining, 9750.0);

        let generic_snapshot = LobsterAiAdapter::build_quota_snapshot(
            &serde_json::json!({
                "creditsLimit": 500,
                "creditsUsed": 100,
                "creditsRemaining": 400,
                "subscriptionStatus": "active"
            }),
            ClaimStatus::Available,
        );
        assert_eq!(generic_snapshot.plan_name.as_deref(), Some("订阅版"));
        assert_eq!(generic_snapshot.buckets[0].remaining, 400.0);
    }

    #[tokio::test]
    async fn claim_daily_handles_revision_retry_idempotent_already_claimed_and_login_required() {
        let server = MockServer::start().await;
        let adapter = LobsterAiAdapter::with_base_url(server.uri());
        let exp = Utc::now().timestamp() + 3600;
        let cred = LobsterAiAdapter::build_envelope(
            make_jwt(serde_json::json!({ "sub": "6001", "exp": exp })),
            Some("rt-6001".into()),
            None,
        )
        .unwrap();

        // Revision mismatch (51106) -> reload slot -> success
        Mock::given(method("GET"))
            .and(path("/api/client-activities/slot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "slotState": "available",
                    "activity": {
                        "activityType": "daily_check_in",
                        "activityCode": "daily_v1",
                        "configRevision": 1
                    }
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/client-activities/daily_v1/context"))
            .and(query_param("configRevision", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "activityCode": "daily_v1",
                    "configRevision": 1,
                    "lifecycleState": "active",
                    "state": { "claimedToday": false, "completed": false, "totalDays": 30, "claimedDays": 0 },
                    "actions": ["check_in"]
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/client-activities/daily_v1/actions/check_in"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 51106,
                "message": "revision mismatch"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second slot + action (with revision 2)
        Mock::given(method("GET"))
            .and(path("/api/client-activities/slot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "slotState": "available",
                    "activity": {
                        "activityType": "daily_check_in",
                        "activityCode": "daily_v1",
                        "configRevision": 2
                    }
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/client-activities/daily_v1/actions/check_in"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "context": {
                        "state": { "totalDays": 30, "claimedDays": 1, "claimedToday": true, "rewardCredits": 100 }
                    },
                    "result": { "activityCode": "daily_v1", "actionId": "check_in", "creditsGranted": 100 }
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/user/quota"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": { "monthlyCreditsLimit": 3000, "monthlyCreditsUsed": 0 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/client-activities/daily_v1/context"))
            .and(query_param("configRevision", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "lifecycleState": "active",
                    "state": { "claimedToday": false, "completed": false, "totalDays": 30, "claimedDays": 1 },
                    "actions": ["check_in"]
                }
            })))
            .mount(&server)
            .await;

        let res = adapter
            .claim_daily(&cred, None)
            .await
            .expect("claim after revision retry");
        assert!(res.claimed);
        assert!(res.reward_summary.contains("+100 积分"));
        assert_eq!(
            res.updated_quota.as_ref().map(|q| &q.claim_status),
            Some(&ClaimStatus::Claimed)
        );

        // Next call: 51104 AlreadyClaimed -> idempotent claimed: false + Claimed status
        Mock::given(method("POST"))
            .and(path("/api/client-activities/daily_v1/actions/check_in"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 51104,
                "message": "already claimed"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let idem = adapter
            .claim_daily(&cred, None)
            .await
            .expect("51104 idempotent");
        assert!(!idem.claimed);
        assert_eq!(
            idem.updated_quota.as_ref().map(|q| &q.claim_status),
            Some(&ClaimStatus::Claimed)
        );

        // Next call: 51102 LoginRequired -> AuthExpired
        Mock::given(method("POST"))
            .and(path("/api/client-activities/daily_v1/actions/check_in"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 51102,
                "message": "login required"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let auth_err = adapter.claim_daily(&cred, None).await.unwrap_err();
        assert!(matches!(
            auth_err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn refresh_credential_rotates_near_expiry_skips_fresh_and_errors_on_40101() {
        let server = MockServer::start().await;
        let adapter = LobsterAiAdapter::with_base_url(server.uri());

        // 1. Fresh token -> Ok(None)
        let fresh_exp = Utc::now().timestamp() + 3600;
        let fresh_cred = LobsterAiAdapter::build_envelope(
            make_jwt(serde_json::json!({ "sub": "7001", "exp": fresh_exp })),
            Some("rt-keep".into()),
            Some(&serde_json::json!({ "id": 7001, "nickname": "新鲜虾" })),
        )
        .unwrap();
        assert!(adapter
            .refresh_credential(&fresh_cred)
            .await
            .expect("fresh")
            .is_none());

        // 2. Near-expiry token -> refreshes and preserves auth_user_json
        let soon_exp = Utc::now().timestamp() + 60;
        let next_exp = Utc::now().timestamp() + 7200;
        let next_at = make_jwt(serde_json::json!({ "sub": "7001", "exp": next_exp }));
        let near_cred = LobsterAiAdapter::build_envelope(
            make_jwt(serde_json::json!({ "sub": "7001", "exp": soon_exp })),
            Some("rt-old".into()),
            Some(&serde_json::json!({ "id": 7001, "yid": "y_7001", "nickname": "快过期虾" })),
        )
        .unwrap();

        Mock::given(method("POST"))
            .and(path("/api/auth/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "accessToken": next_at,
                    "refreshToken": "rt-new"
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let updated = adapter
            .refresh_credential(&near_cred)
            .await
            .expect("refresh ok")
            .expect("should return updated envelope");
        match &updated.payload {
            CredentialPayload::Token {
                access_token,
                refresh_token,
                expires_at,
                extra,
            } => {
                assert_eq!(access_token, &next_at);
                assert_eq!(refresh_token.as_deref(), Some("rt-new"));
                assert_eq!(*expires_at, Some(next_exp));
                assert!(extra.contains_key(EXTRA_AUTH_USER_JSON));
            }
            _ => panic!("expected Token"),
        }

        // 3. Terminal 40101 -> AuthExpired
        Mock::given(method("POST"))
            .and(path("/api/auth/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 40101,
                "message": "refresh token expired"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let err = adapter.refresh_credential(&near_cred).await.unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Unified {
                code: UnifiedErrorCode::AuthExpired,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn exchange_auth_code_returns_complete_envelope() {
        let server = MockServer::start().await;
        let adapter = LobsterAiAdapter::with_base_url(server.uri());
        let exp = Utc::now().timestamp() + 3600;
        let access = make_jwt(serde_json::json!({ "sub": "8001", "yid": "y_8001", "exp": exp }));

        Mock::given(method("POST"))
            .and(path("/api/auth/exchange"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0,
                "data": {
                    "accessToken": access,
                    "refreshToken": "rt-8001",
                    "user": {
                        "id": 8001,
                        "yid": "y_8001",
                        "nickname": "OAuth龙虾",
                        "status": "active"
                    }
                }
            })))
            .mount(&server)
            .await;

        let env = adapter
            .exchange_auth_code_with_base("auth-code-xyz")
            .await
            .expect("exchange");
        assert_eq!(env.tool_id, "lobsterai");
        assert_eq!(env.uid, "8001");
        assert_eq!(env.display_name.as_deref(), Some("OAuth龙虾"));
        assert_eq!(env.capability_flags(), (true, true));
    }

    #[test]
    fn build_oauth_login_url_targets_portal_hash_route() {
        let redirect_uri = "http://127.0.0.1:47821/auth/callback";
        let state = "nonce-abc-123";
        let raw = LobsterAiAdapter::build_oauth_login_url(redirect_uri, state)
            .expect("build oauth login url");
        let parsed = reqwest::Url::parse(&raw).expect("valid url");

        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("lobsterai.youdao.com"));
        assert_eq!(parsed.path(), "/portal");
        assert!(parsed.query().is_none(), "portal query 应放在 Hash 路由内");

        let frag = parsed.fragment().expect("portal hash fragment");
        let (hash_path, hash_query) = frag.split_once('?').expect("fragment query");
        assert_eq!(hash_path, "/login");

        let dummy = reqwest::Url::parse(&format!("https://placeholder.local/?{hash_query}"))
            .expect("parse fragment query");
        let params: HashMap<String, String> = dummy
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(params.get("source").map(String::as_str), Some("electron"));
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some(redirect_uri)
        );
        assert_eq!(params.get("state").map(String::as_str), Some(state));
    }
}
