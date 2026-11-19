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
    pub uid_masked: String,
    pub email_masked: Option<String>,
    pub status: AccountStatus,
    pub is_active_on_client: bool,
    pub has_local_snapshot: bool,
    pub has_api_token: bool,
    pub latest_quota: Option<QuotaSnapshot>,
    pub last_claimed_at: Option<String>,
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
            CredentialPayload::Token { .. } => {
                let can_switch = matches!(self.tool_id.as_str(), "trae" | "trae_cn" | "workbuddy");
                (can_switch, true)
            }
            CredentialPayload::LocalSnapshot { files } => {
                let has_local_snapshot = true;
                let has_api_token = match self.tool_id.as_str() {
                    // 仅存在已解密 token 缓存才算就绪；纯加密快照需本机客户端解密，
                    // 不再无条件虚报"查额与签到就绪"
                    "workbuddy" => files.contains_key("decrypted_token.txt"),
                    "qoder" | "qoder_cn" => files.contains_key("decrypted_token.txt"),
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
    }
}
