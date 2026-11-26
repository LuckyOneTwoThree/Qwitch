pub mod model_meta;
pub mod pools;
pub use pools::*;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    WebviewOauth,
    LocalImport,
    TokenPaste,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Active,
    Expired,
    RiskPaused,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Unknown,
    Available,
    Claimed,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UnifiedErrorCode {
    AuthExpired,
    RiskTriggered,
    RateLimited,
    DeviceDailyLimitReached,
    NetworkError,
    ClientRunning,
    ClientNotFound,
    UnsupportedCapability,
    InvalidCredential,
    InternalError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucket {
    pub key: String,
    pub label: String,
    pub used: f64,
    pub total: f64,
    pub remaining: f64,
    pub unit: String,
    pub reset_at: Option<String>,
    /// 服务端是否给出了可信的「总量」。
    ///
    /// LobsterAI 的积分是跨池余额（免费池 + 签到 + 邀请），服务端只报剩余值，
    /// 已发放总量不可观测。旧实现用「免费池已用」反推总量，会得出「361 / 661
    /// 积分 · 55%」这种用户一对客户端就发现对不上的读数。这类桶把 `has_total`
    /// 置为 false，前端只展示剩余，不再画百分比进度条。
    #[serde(default = "default_has_total")]
    pub has_total: bool,
}

fn default_has_total() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub plan_name: Option<String>,
    pub claim_status: ClaimStatus,
    pub buckets: Vec<QuotaBucket>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: String,
    pub tool_id: String,
    pub label: String,
    pub account_name: Option<String>,
    pub uid_masked: String,
    pub email_masked: Option<String>,
    pub status: AccountStatus,
    pub is_active_on_client: bool,
    pub has_local_snapshot: bool,
    pub has_api_token: bool,
    pub device_id_masked: Option<String>,
    pub device_type: Option<String>,
    pub latest_quota: Option<QuotaSnapshot>,
    pub last_claimed_at: Option<String>,
    pub credential_expires_at: Option<String>,
    pub renewal_mode: Option<String>,
    pub updated_at: String,
}

fn default_true() -> bool {
    true
}

fn default_auto_refresh_interval_minutes() -> u32 {
    0
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_log_archive_after_days() -> u32 {
    14
}

fn default_log_max_active_count() -> u32 {
    200
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingsDto {
    pub jitter_min_ms: u64,
    pub jitter_max_ms: u64,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub custom_client_paths: HashMap<String, String>,
    #[serde(default = "default_log_archive_after_days")]
    pub log_archive_after_days: u32,
    #[serde(default = "default_log_max_active_count")]
    pub log_max_active_count: u32,
    #[serde(default = "default_true")]
    pub close_to_tray: bool,
    #[serde(default)]
    pub launch_at_login: bool,
    #[serde(default)]
    pub start_minimized: bool,
    #[serde(default)]
    pub auto_refresh_on_startup: bool,
    #[serde(default = "default_auto_refresh_interval_minutes")]
    pub auto_refresh_interval_minutes: u32,
    #[serde(default)]
    pub auto_claim_daily: bool,
    #[serde(default)]
    pub trusted_adapters: Vec<String>,
    #[serde(default)]
    pub trusted_adapter_hashes: HashMap<String, String>,
    #[serde(default = "default_gateway_port")]
    pub gateway_port: u16,
}

fn default_gateway_port() -> u16 {
    8187
}

impl Default for AppSettingsDto {
    fn default() -> Self {
        Self {
            jitter_min_ms: 1500,
            jitter_max_ms: 3500,
            theme: "system".to_string(),
            custom_client_paths: HashMap::new(),
            log_archive_after_days: default_log_archive_after_days(),
            log_max_active_count: default_log_max_active_count(),
            close_to_tray: true,
            launch_at_login: false,
            start_minimized: false,
            auto_refresh_on_startup: false,
            auto_refresh_interval_minutes: default_auto_refresh_interval_minutes(),
            auto_claim_daily: false,
            trusted_adapters: Vec::new(),
            trusted_adapter_hashes: HashMap::new(),
            gateway_port: default_gateway_port(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogScope {
    Active,
    Archived,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionLogItem {
    pub id: i64,
    pub account_id: Option<String>,
    pub tool_id: String,
    pub action_type: String,
    pub result_status: String,
    pub error_code: Option<UnifiedErrorCode>,
    pub message_sanitized: String,
    pub created_at: String,
    pub archived_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActionLogStats {
    pub active_count: u64,
    pub archived_count: u64,
    pub failed_active_count: u64,
    pub noise_active_count: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActionLogCleanupRequest {
    pub scope: Option<LogScope>,
    pub ids: Option<Vec<i64>>,
    pub older_than_days: Option<u32>,
    pub action_type: Option<String>,
    pub result_status: Option<String>,
    #[serde(default)]
    pub redundant_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportLogsResult {
    pub file_path: String,
    pub count: usize,
    pub archived_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialPayload {
    Token {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<i64>,
        extra: HashMap<String, String>,
    },
    Cookies {
        cookies: HashMap<String, String>,
        user_agent: Option<String>,
    },
    LocalSnapshot {
        files: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialEnvelope {
    pub tool_id: String,
    pub uid: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub display_email: Option<String>,
    pub payload: CredentialPayload,
}

impl CredentialEnvelope {
    pub fn capability_flags(&self) -> (bool, bool) {
        match &self.payload {
            CredentialPayload::Token { extra, .. } => {
                let can_switch = match self.tool_id.as_str() {
                    "trae" | "trae_cn" | "workbuddy" | "lobsterai" => true,
                    // 码道：只有本机导入留下的完整 session 快照才能重加密写回
                    "codearts" => extra.contains_key("session_snapshot_b64"),
                    _ => false,
                };
                (can_switch, true)
            }
            CredentialPayload::LocalSnapshot { files } => {
                let has_local_snapshot = true;
                let has_api_token = match self.tool_id.as_str() {
                    // 仅存在已解密 token 缓存才算就绪；纯加密快照需本机客户端解密，
                    // 不再无条件虚报"查额与签到就绪"
                    "workbuddy" => files.contains_key("decrypted_token.txt"),
                    "qoder" | "qoder_cn" => files.contains_key("decrypted_token.txt"),
                    "doubaowork" => files.contains_key("decrypted_token.txt"),
                    "minimax_code_cn" => {
                        files.contains_key("decrypted_token.txt") || files.contains_key("auth.json")
                    }
                    "trae" | "trae_cn" => {
                        files.contains_key("decrypted_token.txt")
                            || Self::snapshot_has_trae_token(files)
                    }
                    _ => true,
                };
                (has_local_snapshot, has_api_token)
            }
            CredentialPayload::Cookies { .. } => (false, true),
        }
    }

    /// 返回 `(credential_expires_at_rfc3339, renewal_mode)` 用于前端安全展示续期方式与有效期。
    /// 不包含任何明文令牌。
    pub fn renewal_metadata(&self) -> (Option<String>, String) {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;

        fn jwt_exp_ms(tok: &str) -> Option<i64> {
            let seg = tok.split('.').nth(1)?;
            let bytes = BASE64_URL.decode(seg.as_bytes()).ok()?;
            let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            let exp = v.get("exp").and_then(|x| x.as_i64())?;
            Some(if exp < 10_000_000_000 {
                exp.saturating_mul(1000)
            } else {
                exp
            })
        }

        let (has_snap, has_tok) = self.capability_flags();
        let exp_ms = match &self.payload {
            CredentialPayload::Token {
                access_token,
                expires_at,
                ..
            } => expires_at
                .map(|e| {
                    if e < 10_000_000_000 {
                        e.saturating_mul(1000)
                    } else {
                        e
                    }
                })
                .or_else(|| jwt_exp_ms(access_token)),
            CredentialPayload::LocalSnapshot { files } => files
                .get("decrypted_token.txt")
                .and_then(|b64| BASE64.decode(b64).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|tok| jwt_exp_ms(tok.trim())),
            CredentialPayload::Cookies { .. } => None,
        };
        let exp_rfc3339 = exp_ms
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|dt| dt.to_rfc3339());

        let has_rt = match &self.payload {
            CredentialPayload::Token { refresh_token, .. } => refresh_token
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            CredentialPayload::LocalSnapshot { files } => {
                files.contains_key("decrypted_refresh_token.txt")
            }
            CredentialPayload::Cookies { .. } => false,
        };

        let mode = if self.tool_id == "doubaowork" && has_tok {
            "cookie_keepalive"
        } else if self.tool_id == "codearts" {
            let is_isolated = matches!(
                &self.payload,
                CredentialPayload::Token { extra, .. }
                    if extra.get("codearts_mode").map(String::as_str) == Some("isolated_oauth")
            );
            if is_isolated && has_rt {
                "auto_refresh"
            } else {
                "client_sync"
            }
        } else if matches!(
            self.tool_id.as_str(),
            "minimax_code_cn" | "lobsterai" | "workbuddy" | "qoder_cn" | "trae_cn" | "trae"
        ) && has_rt
        {
            "auto_refresh"
        } else if has_snap {
            "client_sync"
        } else {
            "manual"
        };

        (exp_rfc3339, mode.to_string())
    }

    /// 从快照提取账号绑定设备身份（仅多设备适配器写入 `trae_device.json`）。
    /// 返回 (脱敏 device_id, 设备类型)，用于 UI 展示；无则 None。
    pub fn device_identity(&self) -> Option<(String, Option<String>)> {
        let CredentialPayload::LocalSnapshot { files } = &self.payload else {
            return None;
        };
        let b64 = files.get("trae_device.json")?;
        let bytes = BASE64.decode(b64).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let id = value.get("device_id")?.as_str()?.trim();
        if id.is_empty() {
            return None;
        }
        let chars: Vec<char> = id.chars().collect();
        let masked = if chars.len() <= 8 {
            format!("{}****", chars.iter().take(4).collect::<String>())
        } else {
            let prefix: String = chars.iter().take(4).collect();
            let suffix: String = chars[chars.len() - 4..].iter().collect();
            format!("{prefix}****{suffix}")
        };
        let dtype = value
            .get("device_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Some((masked, dtype))
    }

    /// 跨 OS 导入时覆盖快照中的设备平台，使 `x-device-type` 与设备来源一致。
    /// `platform`: "mac" | "win" | "linux"；`drop_host_metadata` 为真时清空宿主机的
    /// 设备品牌与系统版本（避免把导入机的机型与 OS 版本错配到别的平台设备上）。返回是否发生修改。
    pub fn override_device_platform(&mut self, platform: &str, drop_host_metadata: bool) -> bool {
        let CredentialPayload::LocalSnapshot { files } = &mut self.payload else {
            return false;
        };
        let Some(b64) = files.get("trae_device.json").cloned() else {
            return false;
        };
        let Ok(bytes) = BASE64.decode(b64) else {
            return false;
        };
        let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return false;
        };
        let Some(obj) = value.as_object_mut() else {
            return false;
        };
        obj.insert(
            "device_type".to_string(),
            serde_json::Value::String(platform.to_string()),
        );
        if drop_host_metadata {
            obj.remove("device_brand");
            obj.remove("os_version");
        }
        match serde_json::to_vec(&value) {
            Ok(new_bytes) => {
                files.insert("trae_device.json".to_string(), BASE64.encode(new_bytes));
                true
            }
            Err(_) => false,
        }
    }

    fn snapshot_has_trae_token(files: &HashMap<String, String>) -> bool {
        let Some(b64) = files.get("icube_auth_entries.json") else {
            return false;
        };
        let Ok(bytes) = BASE64.decode(b64) else {
            return false;
        };
        let Ok(entries) =
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)
        else {
            return false;
        };
        let primary = entries
            .get("iCubeAuthInfo://icube.cloudide")
            .or_else(|| entries.values().next());
        let Some(raw_str) = primary.and_then(|v| v.as_str()) else {
            return false;
        };
        if !raw_str.trim_start().starts_with('{') {
            return false;
        }
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw_str) else {
            return false;
        };
        parsed
            .get("token")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    }

    pub(crate) fn is_fallback_uid(uid: &str) -> bool {
        uid.starts_with("trae_cn_")
            || uid.starts_with("trae_")
            || uid.starts_with("qoder_cn_")
            || uid.starts_with("wb_manual_")
            || uid.starts_with("wb_oauth_")
            || uid.starts_with("minimax_cn_")
            || uid.starts_with("lobsterai_")
            || uid.starts_with("doubaowork_")
            || uid.starts_with("codearts_")
    }

    pub fn merge_from(&self, incoming: &CredentialEnvelope) -> CredentialEnvelope {
        let display_name = incoming
            .display_name
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| self.display_name.clone());

        let display_email = incoming
            .display_email
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| self.display_email.clone());

        let uid = if Self::is_fallback_uid(&self.uid)
            && !incoming.uid.trim().is_empty()
            && !Self::is_fallback_uid(&incoming.uid)
        {
            incoming.uid.clone()
        } else if !self.uid.trim().is_empty() {
            self.uid.clone()
        } else {
            incoming.uid.clone()
        };

        let payload = match (&self.payload, &incoming.payload) {
            (
                CredentialPayload::LocalSnapshot { files: old_files },
                CredentialPayload::Token {
                    access_token,
                    refresh_token,
                    ..
                },
            ) => {
                let mut files = old_files.clone();
                files.insert(
                    "decrypted_token.txt".to_string(),
                    BASE64.encode(access_token.as_bytes()),
                );
                if let Some(rt) = refresh_token.as_ref().filter(|s| !s.trim().is_empty()) {
                    files.insert(
                        "decrypted_refresh_token.txt".to_string(),
                        BASE64.encode(rt.as_bytes()),
                    );
                }
                CredentialPayload::LocalSnapshot { files }
            }
            (
                CredentialPayload::Token {
                    access_token,
                    refresh_token,
                    ..
                },
                CredentialPayload::LocalSnapshot { files: new_files },
            ) => {
                let mut files = new_files.clone();
                if !files.contains_key("decrypted_token.txt") {
                    files.insert(
                        "decrypted_token.txt".to_string(),
                        BASE64.encode(access_token.as_bytes()),
                    );
                }
                if !files.contains_key("decrypted_refresh_token.txt") {
                    if let Some(rt) = refresh_token.as_ref().filter(|s| !s.trim().is_empty()) {
                        files.insert(
                            "decrypted_refresh_token.txt".to_string(),
                            BASE64.encode(rt.as_bytes()),
                        );
                    }
                }
                CredentialPayload::LocalSnapshot { files }
            }
            (
                CredentialPayload::LocalSnapshot { files: old_files },
                CredentialPayload::LocalSnapshot { files: new_files },
            ) => {
                let mut files = old_files.clone();
                for (k, v) in new_files {
                    files.insert(k.clone(), v.clone());
                }
                if !new_files.contains_key("decrypted_token.txt") {
                    if let Some(old_tok) = old_files.get("decrypted_token.txt") {
                        files.insert("decrypted_token.txt".to_string(), old_tok.clone());
                    }
                }
                if !new_files.contains_key("decrypted_refresh_token.txt") {
                    if let Some(old_rt) = old_files.get("decrypted_refresh_token.txt") {
                        files.insert("decrypted_refresh_token.txt".to_string(), old_rt.clone());
                    }
                }
                CredentialPayload::LocalSnapshot { files }
            }
            _ => incoming.payload.clone(),
        };

        CredentialEnvelope {
            tool_id: self.tool_id.clone(),
            uid,
            display_name,
            display_email,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v0.6.3 及更早的历史额度快照没有 `hasTotal` 字段，反序列化必须按 true 处理，
    /// 否则老快照会被当成「余额型」，UI 直接不画进度条。
    #[test]
    fn legacy_quota_bucket_json_defaults_has_total_true() {
        let legacy = r#"{"key":"window_4_0_0","label":"每月额度","used":19.0,
            "total":2142.0,"remaining":2123.0,"unit":"额度","resetAt":null}"#;
        let bucket: QuotaBucket = serde_json::from_str(legacy).expect("deserialize");
        assert!(bucket.has_total, "旧快照缺少 hasTotal 时必须默认 true");
        assert_eq!(bucket.total, 2142.0);

        // 新写入的余额型桶要能往返
        let balance = QuotaBucket {
            key: "credits".into(),
            label: "可用积分".into(),
            used: 0.0,
            total: 361.5,
            remaining: 361.5,
            unit: "积分".into(),
            reset_at: None,
            has_total: false,
        };
        let json = serde_json::to_string(&balance).unwrap();
        let back: QuotaBucket = serde_json::from_str(&json).unwrap();
        assert!(!back.has_total);
    }

    #[test]
    fn bidirectional_merge_preserves_snapshot_and_token_and_flags() {
        let mut snap_files = HashMap::new();
        snap_files.insert("auth.v1.dat".to_string(), BASE64.encode(b"v10dat"));

        let snap_env = CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "qoder_cn_112233445566".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot {
                files: snap_files.clone(),
            },
        };
        assert_eq!(snap_env.capability_flags(), (true, false));

        let tok_env = CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "real_qoder_uid_888".to_string(),
            display_name: Some("Dev".to_string()),
            display_email: Some("dev@example.com".to_string()),
            payload: CredentialPayload::Token {
                access_token: "access-token-xyz".to_string(),
                refresh_token: Some("refresh-token-uvw".to_string()),
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        assert_eq!(tok_env.capability_flags(), (false, true));

        // 1. LocalSnapshot + Token
        let merged_a = snap_env.merge_from(&tok_env);
        assert_eq!(merged_a.uid, "real_qoder_uid_888");
        assert_eq!(merged_a.capability_flags(), (true, true));
        if let CredentialPayload::LocalSnapshot { files } = &merged_a.payload {
            assert!(files.contains_key("auth.v1.dat"));
            assert_eq!(
                files.get("decrypted_token.txt").map(String::as_str),
                Some(BASE64.encode(b"access-token-xyz").as_str())
            );
            assert!(files.contains_key("decrypted_refresh_token.txt"));
        } else {
            panic!("expected LocalSnapshot after merge");
        }

        // 2. Token + LocalSnapshot
        let merged_b = tok_env.merge_from(&snap_env);
        assert_eq!(merged_b.uid, "real_qoder_uid_888");
        assert_eq!(merged_b.capability_flags(), (true, true));
        if let CredentialPayload::LocalSnapshot { files } = &merged_b.payload {
            assert!(files.contains_key("auth.v1.dat"));
            assert_eq!(
                files.get("decrypted_token.txt").map(String::as_str),
                Some(BASE64.encode(b"access-token-xyz").as_str())
            );
        } else {
            panic!("expected LocalSnapshot after reverse merge");
        }
    }

    #[test]
    fn override_device_platform_sets_type_and_drops_brand() {
        let mut files = HashMap::new();
        files.insert(
            "trae_device.json".to_string(),
            BASE64.encode(
                serde_json::to_vec(&serde_json::json!({
                    "device_id": "93900972767536",
                    "device_type": "mac",
                    "device_brand": "MacBookPro18,3",
                    "os_version": "macOS 15.1"
                }))
                .unwrap(),
            ),
        );
        let mut env = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "u".to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        };
        assert!(env.override_device_platform("win", true));
        let (masked, dtype) = env.device_identity().expect("device");
        assert_eq!(masked, "9390****7536");
        assert_eq!(dtype.as_deref(), Some("win"));
        if let CredentialPayload::LocalSnapshot { files } = &env.payload {
            let bytes = BASE64
                .decode(files.get("trae_device.json").unwrap())
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(value.get("device_brand").is_none());
            assert!(value.get("os_version").is_none());
            assert_eq!(
                value.get("device_id").and_then(|v| v.as_str()),
                Some("93900972767536")
            );
        } else {
            panic!("expected snapshot payload");
        }
    }

    #[test]
    fn app_settings_legacy_json_deserializes_with_safe_defaults() {
        let legacy_json =
            r#"{"jitterMinMs":1500,"jitterMaxMs":3500,"theme":"dark","hideDockIcon":true}"#;
        let parsed: AppSettingsDto = serde_json::from_str(legacy_json).expect("deserialize legacy");
        assert!(parsed.close_to_tray);
        assert!(!parsed.launch_at_login);
        assert!(!parsed.start_minimized);
        // 默认不静默后台刷新（Human-in-the-Loop 产品边界）
        assert!(!parsed.auto_refresh_on_startup);
        assert_eq!(parsed.auto_refresh_interval_minutes, 0);
        assert!(!parsed.auto_claim_daily);
        assert!(parsed.trusted_adapters.is_empty());
        assert!(parsed.trusted_adapter_hashes.is_empty());
    }
}
