mod accounts;
mod logs;
mod oauth;
mod profile;
mod settings;
mod switch;

pub use accounts::*;
pub use logs::*;
pub use oauth::*;
pub use profile::*;
pub use settings::*;
pub use switch::*;

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

use crate::adapter::{AdapterError, ToolAdapter};
use crate::domain::{AccountStatus, AccountSummary, AppSettingsDto, UnifiedErrorCode};
use crate::platform::PlatformHost;
use crate::scheduler::AntiRiskScheduler;
use crate::storage::QwitchDatabase;

pub(crate) const SETTINGS_KEY: &str = "app.settings";

pub fn load_app_settings(db: &QwitchDatabase) -> AppSettingsDto {
    db.get_setting(SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<AppSettingsDto>(&json).ok())
        .unwrap_or_default()
}

pub fn set_macos_dock_visible(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if visible {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        };
        let _ = app.set_activation_policy(policy);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, visible);
    }
}

pub fn present_main_window(app: &AppHandle) {
    if let Some(tray) = app.get_webview_window("tray") {
        let _ = tray.hide();
    }
    set_macos_dock_visible(app, true);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

pub fn hide_main_window_to_tray(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    let has_other_visible = app
        .webview_windows()
        .iter()
        .any(|(label, w)| label != "main" && label != "tray" && w.is_visible().unwrap_or(false));
    if !has_other_visible {
        set_macos_dock_visible(app, false);
    }
}

pub(crate) fn resolve_client_executable(
    host: &dyn PlatformHost,
    settings: &AppSettingsDto,
    tool_id: &str,
) -> Option<PathBuf> {
    if let Some(custom) = settings
        .custom_client_paths
        .get(tool_id)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let p = PathBuf::from(custom);
        if p.exists() {
            return Some(p);
        }
    }
    host.detect_client_executable(tool_id)
}

pub struct AppState {
    pub db: Arc<QwitchDatabase>,
    pub master_key: [u8; 32],
    pub host: Arc<dyn PlatformHost>,
    pub adapters: Arc<HashMap<String, Arc<dyn ToolAdapter>>>,
    pub scheduler: Arc<AntiRiskScheduler>,
    pub oauth_flow_seq: Arc<AtomicU64>,
    pub switching_in_progress: Arc<AtomicBool>,
    pub last_reconcile_ms: Arc<AtomicU64>,
    pub tray_blur_suppress_until_ms: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchProgressEvent {
    pub account_id: String,
    pub phase: String,
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    present_main_window(&app);
}

pub(crate) fn is_valid_oauth_uid(uid: &str) -> bool {
    let s = uid.trim();
    if s.len() < 2 || s.len() > 128 {
        return false;
    }
    if s.chars()
        .any(|c| c.is_whitespace() || matches!(c, ',' | '[' | ']' | '{' | '}' | '"' | '\''))
    {
        return false;
    }
    !matches!(
        s.to_ascii_lowercase().as_str(),
        "0" | "-1" | "null" | "undefined" | "none" | "guest" | "anonymous"
    )
}

pub(crate) async fn record_import_quota_check(
    db: &Arc<QwitchDatabase>,
    master_key: &[u8; 32],
    adapter: &Arc<dyn ToolAdapter>,
    summary: &AccountSummary,
    cred: &crate::domain::CredentialEnvelope,
) {
    if !summary.has_api_token || !adapter.manifest().capabilities.supports_quota_query {
        return;
    }
    let effective_cred = match adapter.refresh_credential(cred).await {
        Ok(Some(updated)) => {
            let _ = db.bind_account_credential(master_key, &summary.id, None, &updated);
            updated
        }
        Ok(None) => cred.clone(),
        Err(AdapterError::Unified { code, message }) => {
            let status = match code {
                UnifiedErrorCode::AuthExpired => Some(AccountStatus::Expired),
                UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited => {
                    Some(AccountStatus::RiskPaused)
                }
                _ => None,
            };
            if let Some(status) = status {
                let _ = db.update_account_status(&summary.id, status);
            }
            let _ = db.append_action_log(
                Some(&summary.id),
                &summary.tool_id,
                "query",
                "failed",
                Some(&code),
                &format!("导入后查额前刷新凭据失败：{message}"),
            );
            return;
        }
    };
    match adapter.inspect_quota(&effective_cred).await {
        Ok(snapshot) => {
            if matches!(
                summary.status,
                AccountStatus::Expired | AccountStatus::RiskPaused
            ) {
                let _ = db.update_account_status(&summary.id, AccountStatus::Active);
            }
            let _ = db.save_quota_snapshot(&summary.id, &snapshot);
        }
        Err(AdapterError::Unified { code, message }) => {
            let status = match code {
                UnifiedErrorCode::AuthExpired => Some(AccountStatus::Expired),
                UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited => {
                    Some(AccountStatus::RiskPaused)
                }
                _ => None,
            };
            if let Some(status) = status {
                let _ = db.update_account_status(&summary.id, status);
            }
            let _ = db.append_action_log(
                Some(&summary.id),
                &summary.tool_id,
                "query",
                "failed",
                Some(&code),
                &format!("导入后查额失败：{message}"),
            );
        }
    }
}

pub(crate) async fn schedule_import_quota_check(
    app: &AppHandle,
    db: &Arc<QwitchDatabase>,
    master_key: &[u8; 32],
    adapter: &Arc<dyn ToolAdapter>,
    summary: &AccountSummary,
    cred: &crate::domain::CredentialEnvelope,
    scheduler: &Arc<AntiRiskScheduler>,
) {
    if !summary.has_api_token || !adapter.manifest().capabilities.supports_quota_query {
        return;
    }
    if let Ok(guard) = scheduler.queue_lock().try_lock_owned() {
        record_import_quota_check(db, master_key, adapter, summary, cred).await;
        drop(guard);
        return;
    }

    let app_bg = app.clone();
    let db_bg = Arc::clone(db);
    let key_bg = *master_key;
    let adapter_bg = Arc::clone(adapter);
    let summary_bg = summary.clone();
    let cred_bg = cred.clone();
    let scheduler_bg = Arc::clone(scheduler);
    tokio::spawn(async move {
        let _guard = scheduler_bg.queue_lock().lock_owned().await;
        record_import_quota_check(&db_bg, &key_bg, &adapter_bg, &summary_bg, &cred_bg).await;
        let _ = app_bg.emit("qwitch://account-updated", &summary_bg.id);
        let _ = app_bg.emit("qwitch://logs-updated", ());
    });
}
