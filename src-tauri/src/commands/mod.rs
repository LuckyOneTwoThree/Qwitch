mod accounts;
mod api_providers;
mod logs;
mod oauth;
mod plugins;
mod pools;
mod profile;
mod settings;
mod switch;

pub use accounts::*;
pub use api_providers::*;
pub use logs::*;
pub use oauth::*;
pub use plugins::*;
pub use pools::*;
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
use crate::gateway::GatewayRuntime;
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

/// 解析客户端可执行文件路径（自定义路径优先，其次自动探测）。
///
/// **安全**：无论来自设置还是自动探测，返回前都经
/// `platform::validate_client_executable` 复核（名称白名单 + bundle 结构 +
/// 受信任安装目录）。校验不通过的自定义路径会被忽略并回退到自动探测，
/// 避免「设置里写入任意二进制 → 启动时执行」的路径。
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
        if p.exists() && crate::platform::validate_client_executable(tool_id, &p).is_ok() {
            return Some(p);
        }
    }
    let detected = host.detect_client_executable(tool_id)?;
    if crate::platform::validate_client_executable(tool_id, &detected).is_ok() {
        Some(detected)
    } else {
        // 自动探测结果同样不可信（例如被同名程序污染的 PATH）
        None
    }
}

/// 外部插件信任的一次性挑战票据存储类型：`adapter_id -> (ticket, content_hash, issued_at_ms)`。
pub type PluginTrustTicketMap = HashMap<String, (String, String, u64)>;

pub struct AppState {
    pub db: Arc<QwitchDatabase>,
    pub master_key: [u8; 32],
    pub host: Arc<dyn PlatformHost>,
    pub adapters: Arc<HashMap<String, Arc<dyn ToolAdapter>>>,
    pub plugins: Arc<std::sync::RwLock<crate::plugins::PluginRegistry>>,
    pub scheduler: Arc<AntiRiskScheduler>,
    pub gateway: Arc<GatewayRuntime>,
    pub oauth_flow_seq: Arc<AtomicU64>,
    /// 已被消费的 OAuth 回调 nonce 集合。
    ///
    /// `oauth_flow_seq` 只能判定「是否为最新流」，无法阻止同一个 nonce 被重复投递：
    /// 授权页内任意脚本都能读到 nonce 并反复构造回调，从而向凭据库注入不同 token
    /// （账号投毒）。此集合记录已消费的 nonce，命中即拒绝，实现一次性语义。
    pub consumed_oauth_nonces: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    pub switching_in_progress: Arc<AtomicBool>,
    pub last_reconcile_ms: Arc<AtomicU64>,
    pub tray_blur_suppress_until_ms: Arc<AtomicU64>,
    /// 外部插件信任的一次性挑战票据：`adapter_id -> (challenge, 审批时 content_hash, 签发时间 ms)`。
    ///
    /// 信任决策不能只凭渲染进程的入参，否则一次前端污染即可自我批准插件加载。
    /// 流程：前端请求 → 后端弹原生确认框 → 用户确认后签发与当时哈希绑定的短时效票据 →
    /// 前端携带票据保存设置。票据一次性消费且校验哈希未被偷换。
    pub plugin_trust_tickets: Arc<std::sync::Mutex<PluginTrustTicketMap>>,
}

/// 信任票据有效期（毫秒）。用户从「点击启用」到「保存设置」通常数秒内完成。
pub const PLUGIN_TRUST_TICKET_TTL_MS: u64 = 120_000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 签发一次性信任票据（绑定当时磁盘 content_hash，并覆盖此前未消费票据）。
pub fn issue_plugin_trust_ticket(
    store: &std::sync::Mutex<PluginTrustTicketMap>,
    adapter_id: &str,
    content_hash: &str,
) -> String {
    let ticket = uuid::Uuid::new_v4().to_string();
    if let Ok(mut map) = store.lock() {
        map.insert(
            adapter_id.to_string(),
            (ticket.clone(), content_hash.to_string(), now_ms()),
        );
    }
    ticket
}

/// 校验并消费信任票据。票据必须匹配、哈希与审批时一致、未过期且仅能使用一次。
pub fn consume_plugin_trust_ticket(
    store: &std::sync::Mutex<PluginTrustTicketMap>,
    adapter_id: &str,
    ticket: &str,
    expected_hash: &str,
) -> Result<(), String> {
    let mut map = store
        .lock()
        .map_err(|_| "信任票据存储不可用，请重试".to_string())?;
    let Some((expected, approved_hash, issued_at)) = map.get(adapter_id).cloned() else {
        return Err(format!(
            "插件 {adapter_id} 缺少信任确认记录，请重新点击启用并在系统弹窗中确认"
        ));
    };
    if expected != ticket.trim() {
        return Err(format!("插件 {adapter_id} 的信任确认票据无效，请重新确认"));
    }
    if !approved_hash.eq_ignore_ascii_case(expected_hash.trim()) {
        map.remove(adapter_id);
        return Err(format!(
            "插件 {adapter_id} 内容在确认后发生变化，请重新确认信任"
        ));
    }
    if now_ms().saturating_sub(issued_at) > PLUGIN_TRUST_TICKET_TTL_MS {
        map.remove(adapter_id);
        return Err(format!(
            "插件 {adapter_id} 的信任确认已超时，请重新点击启用并在系统弹窗中确认"
        ));
    }
    // 一次性消费
    map.remove(adapter_id);
    Ok(())
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
    let account_id_bg = summary.id.clone();
    let scheduler_bg = Arc::clone(scheduler);
    tokio::spawn(async move {
        let _guard = scheduler_bg.queue_lock().lock_owned().await;
        let Ok((latest_summary, latest_cred)) =
            db_bg.get_account_credential(&key_bg, &account_id_bg)
        else {
            return;
        };
        record_import_quota_check(&db_bg, &key_bg, &adapter_bg, &latest_summary, &latest_cred)
            .await;
        let _ = app_bg.emit("qwitch://account-updated", &latest_summary.id);
        let _ = app_bg.emit("qwitch://logs-updated", ());
    });
}
