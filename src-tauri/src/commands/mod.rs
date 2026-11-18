use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::adapter::{
    qoder::QoderAdapter, workbuddy::WorkBuddyAdapter, AdapterError, AdapterManifest, ToolAdapter,
};
use crate::domain::{
    AccountStatus, AccountSummary, ActionLogCleanupRequest, ActionLogItem, ActionLogStats,
    AppSettingsDto, ExportLogsResult, LogScope, QuotaSnapshot, UnifiedErrorCode,
};
use crate::platform::PlatformHost;
use crate::scheduler::{AntiRiskScheduler, JitterConfig};
use crate::storage::QwitchDatabase;

const SETTINGS_KEY: &str = "app.settings";

pub fn load_app_settings(db: &QwitchDatabase) -> AppSettingsDto {
    db.get_setting(SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<AppSettingsDto>(&json).ok())
        .unwrap_or_default()
}

pub fn apply_runtime_app_policy(app: &AppHandle, settings: &AppSettingsDto) {
    #[cfg(target_os = "macos")]
    {
        let policy = if settings.hide_dock_icon {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        };
        let _ = app.set_activation_policy(policy);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, settings);
    }
}

fn resolve_client_executable(
    host: &dyn PlatformHost,
    settings: &AppSettingsDto,
    tool_id: &str,
) -> Option<std::path::PathBuf> {
    if let Some(custom) = settings
        .custom_client_paths
        .get(tool_id)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let p = std::path::PathBuf::from(custom);
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
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientHostStatus {
    pub tool_id: String,
    pub executable_path: Option<String>,
    pub config_dir: Option<String>,
    pub is_running: bool,
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    if let Some(tray) = app.get_webview_window("tray") {
        let _ = tray.hide();
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[tauri::command]
pub fn list_adapters(state: State<'_, AppState>) -> Vec<AdapterManifest> {
    let mut list: Vec<AdapterManifest> = state
        .adapters
        .values()
        .map(|a| a.manifest().clone())
        .collect();
    list.sort_by(|a, b| b.is_builtin.cmp(&a.is_builtin).then(a.id.cmp(&b.id)));
    list
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAdapterInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub platforms: Vec<String>,
    pub trusted: bool,
    pub reserved: bool,
}

#[tauri::command]
pub fn list_external_adapters(state: State<'_, AppState>) -> Result<Vec<ExternalAdapterInfo>, String> {
    let data_dir = state.host.qwitch_data_dir().map_err(|e| e.to_string())?;
    let settings = load_app_settings(&state.db);
    let mut list: Vec<ExternalAdapterInfo> =
        crate::adapter::external::scan_external_manifests(&data_dir)
            .into_iter()
            .map(|(_, m)| {
                let reserved = crate::adapter::RESERVED_ADAPTER_IDS.contains(&m.id.as_str());
                ExternalAdapterInfo {
                    trusted: !reserved && settings.trusted_adapters.iter().any(|t| t == &m.id),
                    reserved,
                    id: m.id,
                    name: m.name,
                    version: m.version,
                    platforms: m.platforms,
                }
            })
            .collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(list)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchProgressEvent {
    pub account_id: String,
    pub phase: String,
}

#[tauri::command]
pub fn get_app_settings(state: State<'_, AppState>) -> Result<AppSettingsDto, String> {
    Ok(load_app_settings(&state.db))
}

#[tauri::command]
pub fn save_app_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: AppSettingsDto,
) -> Result<AppSettingsDto, String> {
    // 防风控抖动下限：不允许 0ms 连发
    settings.jitter_min_ms = settings.jitter_min_ms.max(500);
    settings.jitter_max_ms = settings.jitter_max_ms.max(settings.jitter_min_ms);
    for (tool, path) in &settings.custom_client_paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        let p = std::path::Path::new(trimmed);
        if !p.is_absolute() || !p.exists() {
            return Err(format!(
                "工具 {tool} 的自定义客户端路径不存在或不是绝对路径：{trimmed}"
            ));
        }
    }
    state.host.set_launch_at_login(settings.launch_at_login)?;
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    state
        .db
        .set_setting(SETTINGS_KEY, &json)
        .map_err(|e| e.to_string())?;
    let _ = state
        .db
        .apply_log_retention(settings.log_archive_after_days, settings.log_max_active_count);
    apply_runtime_app_policy(&app, &settings);
    let _ = app.emit("qwitch://settings-updated", &settings);
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(settings)
}

#[tauri::command]
pub fn detect_client_status(
    state: State<'_, AppState>,
    tool_id: String,
) -> Result<ClientHostStatus, String> {
    let settings = load_app_settings(&state.db);
    let executable_path = resolve_client_executable(state.host.as_ref(), &settings, &tool_id)
        .map(|p| p.to_string_lossy().to_string());
    let config_dir = state
        .host
        .client_config_dir(&tool_id)
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    let is_running = state
        .host
        .is_client_running(&tool_id)
        .map_err(|e| e.to_string())?;

    Ok(ClientHostStatus {
        tool_id,
        executable_path,
        config_dir,
        is_running,
    })
}

#[tauri::command]
pub fn list_accounts(
    state: State<'_, AppState>,
    tool_id: Option<String>,
) -> Result<Vec<AccountSummary>, String> {
    state
        .db
        .list_accounts(tool_id.as_deref())
        .map_err(|e| e.to_string())
}

fn is_valid_oauth_uid(uid: &str) -> bool {
    let s = uid.trim();
    if s.len() < 2 || s.len() > 128 {
        return false;
    }
    if s
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, ',' | '[' | ']' | '{' | '}' | '"' | '\''))
    {
        return false;
    }
    !matches!(
        s.to_ascii_lowercase().as_str(),
        "0" | "-1" | "null" | "undefined" | "none" | "guest" | "anonymous"
    )
}

async fn record_import_quota_check(
    db: &Arc<QwitchDatabase>,
    adapter: &Arc<dyn ToolAdapter>,
    summary: &AccountSummary,
    cred: &crate::domain::CredentialEnvelope,
) {
    match adapter.inspect_quota(cred).await {
        Ok(snapshot) => {
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

#[tauri::command]
pub async fn import_local_account(
    app: AppHandle,
    state: State<'_, AppState>,
    tool_id: String,
    label: String,
    target_account_id: Option<String>,
) -> Result<AccountSummary, String> {
    let adapter = state
        .adapters
        .get(&tool_id)
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;

    let envelope = adapter
        .import_from_local(state.host.as_ref())
        .await
        .map_err(|e| e.to_string())?;

    let target_id = target_account_id.filter(|s| !s.trim().is_empty());
    let summary = if let Some(ref acc_id) = target_id {
        let label_override = (!label.trim().is_empty()).then_some(label.as_str());
        state
            .db
            .bind_account_credential(&state.master_key, acc_id, label_override, &envelope)
            .map_err(|e| e.to_string())?
    } else {
        let label_override = (!label.trim().is_empty()).then_some(label.trim());
        state
            .db
            .upsert_account(&state.master_key, label_override, &envelope)
            .map_err(|e| e.to_string())?
    };

    let cred_for_quota = state
        .db
        .get_account_credential(&state.master_key, &summary.id)
        .map(|(_, env)| env)
        .unwrap_or(envelope);
    record_import_quota_check(&state.db, &adapter, &summary, &cred_for_quota).await;

    let _ = state.db.set_active_on_client(&tool_id, &summary.id);

    let log_msg = if target_id.is_some() {
        format!("补充/更新本机快照 {} ({})", summary.label, summary.uid_masked)
    } else {
        format!("从本机导入 {} ({})", summary.label, summary.uid_masked)
    };
    let _ = state.db.append_action_log(
        Some(&summary.id),
        &tool_id,
        "import",
        "success",
        None,
        &log_msg,
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    let _ = app.emit("qwitch://logs-updated", ());
    state
        .db
        .get_account_summary(&summary.id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_account_by_token(
    app: AppHandle,
    state: State<'_, AppState>,
    tool_id: String,
    label: String,
    raw_input: String,
    target_account_id: Option<String>,
) -> Result<AccountSummary, String> {
    let adapter = state
        .adapters
        .get(&tool_id)
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;

    let envelope = adapter
        .parse_manual_token(&raw_input)
        .await
        .map_err(|e| e.to_string())?;

    let target_id = target_account_id.filter(|s| !s.trim().is_empty());
    let summary = if let Some(ref acc_id) = target_id {
        let label_override = (!label.trim().is_empty()).then_some(label.as_str());
        state
            .db
            .bind_account_credential(&state.master_key, acc_id, label_override, &envelope)
            .map_err(|e| e.to_string())?
    } else {
        let label_override = (!label.trim().is_empty()).then_some(label.trim());
        state
            .db
            .upsert_account(&state.master_key, label_override, &envelope)
            .map_err(|e| e.to_string())?
    };

    let cred_for_quota = state
        .db
        .get_account_credential(&state.master_key, &summary.id)
        .map(|(_, env)| env)
        .unwrap_or(envelope);
    record_import_quota_check(&state.db, &adapter, &summary, &cred_for_quota).await;

    let log_msg = if target_id.is_some() {
        format!("补充/更新凭据 {} ({})", summary.label, summary.uid_masked)
    } else {
        format!("通过手动凭据添加 {} ({})", summary.label, summary.uid_masked)
    };
    let _ = state.db.append_action_log(
        Some(&summary.id),
        &tool_id,
        "import",
        "success",
        None,
        &log_msg,
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    let _ = app.emit("qwitch://logs-updated", ());
    state
        .db
        .get_account_summary(&summary.id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn refresh_account_quota(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<QuotaSnapshot, String> {
    let (summary, cred) = state
        .db
        .get_account_credential(&state.master_key, &account_id)
        .map_err(|e| e.to_string())?;

    let adapter = state
        .adapters
        .get(&summary.tool_id)
        .ok_or_else(|| format!("适配器 {} 尚未注册", summary.tool_id))?;

    // 单账号查额同样进入串行队列，批量运行中排队等待而非并发请求平台
    let _queue_guard = state.scheduler.queue_lock().lock_owned().await;

    match adapter.inspect_quota(&cred).await {
        Ok(snapshot) => {
            let _ = state.db.save_quota_snapshot(&summary.id, &snapshot);
            let _ = state.db.append_action_log(
                Some(&summary.id),
                &summary.tool_id,
                "query",
                "success",
                None,
                &format!("已刷新 {} ({}) 的最新额度", summary.label, summary.uid_masked),
            );
            let _ = app.emit("qwitch://account-updated", &summary.id);
            let _ = app.emit("qwitch://logs-updated", ());
            Ok(snapshot)
        }
        Err(AdapterError::Unified { code, message }) => {
            if matches!(code, UnifiedErrorCode::AuthExpired) {
                let _ = state
                    .db
                    .update_account_status(&summary.id, AccountStatus::Expired);
            } else if matches!(
                code,
                UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited
            ) {
                let _ = state
                    .db
                    .update_account_status(&summary.id, AccountStatus::RiskPaused);
            }
            let _ = state.db.append_action_log(
                Some(&summary.id),
                &summary.tool_id,
                "query",
                "failed",
                Some(&code),
                &format!("{} 查询失败：{message}", summary.label),
            );
            let _ = app.emit("qwitch://account-updated", &summary.id);
            let _ = app.emit("qwitch://logs-updated", ());
            Err(message)
        }
    }
}

struct SwitchFlagGuard {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl SwitchFlagGuard {
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SwitchFlagGuard {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(false, Ordering::SeqCst);
        }
    }
}

#[tauri::command]
pub async fn switch_desktop_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    force_kill: bool,
) -> Result<(), String> {
    let mut switch_guard = SwitchFlagGuard::new(Arc::clone(&state.switching_in_progress));

    let (summary, cred) = state
        .db
        .get_account_credential(&state.master_key, &account_id)
        .map_err(|e| e.to_string())?;

    let adapter = state
        .adapters
        .get(&summary.tool_id)
        .ok_or_else(|| format!("适配器 {} 尚未注册", summary.tool_id))?;

    let settings = load_app_settings(&state.db);
    let exe = resolve_client_executable(state.host.as_ref(), &settings, &summary.tool_id)
        .ok_or_else(|| {
            format!(
                "未检测到 {} 客户端安装路径，请在设置中配置自定义路径后再切换",
                summary.tool_id
            )
        })?;

    let running = state
        .host
        .is_client_running(&summary.tool_id)
        .map_err(|e| e.to_string())?;

    if running && !force_kill {
        return Err("CLIENT_RUNNING".into());
    }

    if running {
        let _ = app.emit(
            "qwitch://switch-progress",
            SwitchProgressEvent {
                account_id: summary.id.clone(),
                phase: "closing".into(),
            },
        );
        state
            .host
            .terminate_client_gracefully(&summary.tool_id, 5000)
            .await
            .map_err(|e| e.to_string())?;
    }

    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "writing".into(),
        },
    );

    adapter
        .apply_session(state.host.as_ref(), &cred)
        .await
        .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "launching".into(),
        },
    );

    state
        .host
        .launch_client(&exe, &[])
        .map_err(|e| format!("写入快照成功但拉起客户端失败: {e}"))?;

    state
        .db
        .set_active_on_client(&summary.tool_id, &summary.id)
        .map_err(|e| e.to_string())?;

    let _ = state.db.append_action_log(
        Some(&summary.id),
        &summary.tool_id,
        "switch",
        "success",
        None,
        &format!("已将本机客户端切换至 {} ({})", summary.label, summary.uid_masked),
    );
    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "done".into(),
        },
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    let _ = app.emit("qwitch://logs-updated", ());

    switch_guard.disarm();
    let flag_clone = Arc::clone(&state.switching_in_progress);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
        flag_clone.store(false, Ordering::SeqCst);
    });

    Ok(())
}

#[tauri::command]
pub fn delete_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<(), String> {
    let summary = state
        .db
        .get_account_summary(&account_id)
        .map_err(|e| e.to_string())?;
    state
        .db
        .delete_account(&account_id)
        .map_err(|e| e.to_string())?;
    let _ = state.db.append_action_log(
        None,
        &summary.tool_id,
        "import",
        "success",
        None,
        &format!("已删除账号 {} ({})", summary.label, summary.uid_masked),
    );
    let _ = app.emit("qwitch://account-updated", &account_id);
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(())
}

#[tauri::command]
pub fn update_account_status(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    status: AccountStatus,
) -> Result<(), String> {
    state
        .db
        .update_account_status(&account_id, status.clone())
        .map_err(|e| e.to_string())?;
    if let Ok(summary) = state.db.get_account_summary(&account_id) {
        let status_label = match status {
            AccountStatus::Active => "正常",
            AccountStatus::Expired => "凭据过期",
            AccountStatus::RiskPaused => "风控暂停",
            AccountStatus::Disabled => "已停用",
        };
        let _ = state.db.append_action_log(
            Some(&summary.id),
            &summary.tool_id,
            "import",
            "success",
            None,
            &format!(
                "已将账号 {} ({}) 状态切换为{status_label}",
                summary.label, summary.uid_masked
            ),
        );
    }
    let _ = app.emit("qwitch://account-updated", &account_id);
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(())
}

#[tauri::command]
pub fn rename_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    label: String,
) -> Result<AccountSummary, String> {
    let summary = state
        .db
        .update_account_label(&account_id, &label)
        .map_err(|e| e.to_string())?;
    let _ = state.db.append_action_log(
        Some(&summary.id),
        &summary.tool_id,
        "import",
        "success",
        None,
        &format!("已将账号 ({}) 备注重命名为 {}", summary.uid_masked, summary.label),
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(summary)
}

#[tauri::command]
pub async fn refresh_quota_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    account_ids: Vec<String>,
) -> Result<(), String> {
    if account_ids.len() > 200 {
        return Err("单次批量最多 200 个账号".into());
    }
    let settings = load_app_settings(&state.db);
    let jitter = JitterConfig {
        min_ms: settings.jitter_min_ms,
        max_ms: settings.jitter_max_ms.max(settings.jitter_min_ms),
    };
    let db = Arc::clone(&state.db);
    let master_key = state.master_key;
    let adapters = Arc::clone(&state.adapters);
    let scheduler = Arc::clone(&state.scheduler);
    // 重入即拒绝：快速双击/双窗口并发不再排队覆盖进度
    let guard = scheduler
        .queue_lock()
        .try_lock_owned()
        .map_err(|_| "已有批量任务正在进行，请等待完成后再试".to_string())?;

    tokio::spawn(async move {
        let _guard = guard;
        let _ = scheduler
            .run_quota_refresh_batch(
                &app,
                &db,
                &master_key,
                &adapters,
                account_ids,
                jitter,
            )
            .await;
    });

    Ok(())
}

#[tauri::command]
pub async fn claim_daily_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    account_ids: Vec<String>,
) -> Result<(), String> {
    if account_ids.len() > 200 {
        return Err("单次批量最多 200 个账号".into());
    }
    let settings = load_app_settings(&state.db);
    let jitter = JitterConfig {
        min_ms: settings.jitter_min_ms,
        max_ms: settings.jitter_max_ms.max(settings.jitter_min_ms),
    };
    let db = Arc::clone(&state.db);
    let master_key = state.master_key;
    let adapters = Arc::clone(&state.adapters);
    let scheduler = Arc::clone(&state.scheduler);
    let guard = scheduler
        .queue_lock()
        .try_lock_owned()
        .map_err(|_| "已有批量任务正在进行，请等待完成后再试".to_string())?;

    tokio::spawn(async move {
        let _guard = guard;
        let _ = scheduler
            .run_daily_claim_batch(
                &app,
                &db,
                &master_key,
                &adapters,
                account_ids,
                jitter,
            )
            .await;
    });

    Ok(())
}

#[tauri::command]
pub fn get_action_logs(
    state: State<'_, AppState>,
    scope: Option<LogScope>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<ActionLogItem>, String> {
    state
        .db
        .list_action_logs(
            scope.unwrap_or(LogScope::Active),
            limit.unwrap_or(100),
            offset.unwrap_or(0),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_action_log_stats(state: State<'_, AppState>) -> Result<ActionLogStats, String> {
    state.db.get_action_log_stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn archive_action_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ActionLogCleanupRequest,
) -> Result<usize, String> {
    let count = state
        .db
        .archive_action_logs(&request)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(count)
}

#[tauri::command]
pub fn unarchive_action_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ActionLogCleanupRequest,
) -> Result<usize, String> {
    let count = state
        .db
        .unarchive_action_logs(&request)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(count)
}

#[tauri::command]
pub fn delete_action_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ActionLogCleanupRequest,
) -> Result<usize, String> {
    let count = state
        .db
        .delete_action_logs(&request)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(count)
}

#[tauri::command]
pub fn export_action_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    scope: Option<LogScope>,
    archive_after_export: Option<bool>,
) -> Result<ExportLogsResult, String> {
    let target_scope = scope.unwrap_or(LogScope::Archived);
    let logs = state
        .db
        .list_action_logs(target_scope, 10_000, 0)
        .map_err(|e| e.to_string())?;
    if logs.is_empty() {
        return Err("当前视图没有可导出的操作日志".into());
    }

    let data_dir = state
        .host
        .qwitch_data_dir()
        .map_err(|e| e.to_string())?;
    let archive_dir = data_dir.join("archives");
    std::fs::create_dir_all(&archive_dir).map_err(|e| e.to_string())?;

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let scope_tag = match target_scope {
        LogScope::Active => "active",
        LogScope::Archived => "archived",
        LogScope::All => "all",
    };
    let file_path = archive_dir.join(format!("action-logs-{scope_tag}-{stamp}.json"));
    let payload = serde_json::json!({
        "app": "Qwitch",
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "scope": scope_tag,
        "count": logs.len(),
        "logs": logs,
    });
    let pretty = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    std::fs::write(&file_path, pretty).map_err(|e| e.to_string())?;

    let mut archived_count = 0usize;
    if archive_after_export.unwrap_or(false) && target_scope == LogScope::Active {
        let ids: Vec<i64> = payload["logs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("id").and_then(|v| v.as_i64()))
            .collect();
        archived_count = state
            .db
            .archive_action_logs(&ActionLogCleanupRequest {
                scope: Some(LogScope::Active),
                ids: Some(ids),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        let _ = app.emit("qwitch://logs-updated", ());
    }

    Ok(ExportLogsResult {
        file_path: file_path.to_string_lossy().to_string(),
        count: payload["count"].as_u64().unwrap_or(0) as usize,
        archived_count,
    })
}

const OAUTH_CAPTURE_JS: &str = r#"
(function() {
  if (window.__qwitchOAuthInjected) return;
  window.__qwitchOAuthInjected = true;
  let captured = false;

  const ACC_KEY = '__qwitch_oauth_acc_v2';
  function loadAcc() {
    try {
      const raw = window.sessionStorage.getItem(ACC_KEY);
      return raw ? JSON.parse(raw) : {};
    } catch (_) {
      return {};
    }
  }
  function saveAcc(acc) {
    try {
      window.sessionStorage.setItem(ACC_KEY, JSON.stringify(acc));
    } catch (_) {}
  }

  function decodeJwtPayload(token) {
    if (typeof token !== 'string') return null;
    const parts = token.split('.');
    if (parts.length !== 3) return null;
    try {
      const b64 = parts[1].replace(/-/g, '+').replace(/_/g, '/');
      const json = decodeURIComponent(atob(b64).split('').map(c => '%' + ('00' + c.charCodeAt(0).toString(16)).slice(-2)).join(''));
      return JSON.parse(json);
    } catch (_) {
      return null;
    }
  }

  function isValidUid(val) {
    if (typeof val !== 'string' && typeof val !== 'number') return false;
    const s = String(val).trim();
    if (!s || s.length < 2 || s.length > 128) return false;
    if (/[\s,\[\]{}"']/.test(s)) return false;
    if (/^(0|-1|null|undefined|none|guest|anonymous)$/i.test(s)) return false;
    return true;
  }

  function isLikelyAuthToken(val) {
    if (typeof val !== 'string') return false;
    const s = val.trim();
    if (s.length < 24 || /\s/.test(s)) return false;
    if (/^(qr_|qrcode_|csrf_|anon_|guest_)/i.test(s)) return false;
    return true;
  }

  function isOnLoginPage() {
    const h = window.location.hostname || '';
    const p = window.location.pathname || '';
    if (/douyin\.com|doubao\.com|feishu\.cn|larkoffice\.com|weixin\.qq\.com|aliyun\.com|apple\.com|github\.com|google\.com/i.test(h)) return true;
    if (/\/login|\/sign-in|\/signin|\/oauth/i.test(p)) return true;
    return false;
  }

  function isValidDisplayName(val) {
    if (typeof val !== 'string') return false;
    const s = val.trim();
    if (!s || s.length > 64) return false;
    if (/^(success|ok|okay|error|fail|failed|true|false|null|undefined|none|unknown|trae|douyin|doubao|qoder|workbuddy|guest|anonymous|resource|data|result|message|status|code|user|account|info|default|normal)$/i.test(s)) {
      return false;
    }
    return true;
  }

  function collectFields(obj, depth, acc) {
    if (!obj || typeof obj !== 'object' || (depth || 0) > 5) return;
    const d = depth || 0;

    for (const key of ['accessToken', 'access_token', 'jwt', 'idToken', 'id_token', 'Token', 'token']) {
      if (isLikelyAuthToken(obj[key])) {
        acc.token = obj[key].trim();
        break;
      }
    }

    for (const key of ['userId', 'uid', 'user_id', 'UserId', 'uin', 'sub', 'accountId', 'memberId']) {
      if (isValidUid(obj[key])) {
        acc.userId = String(obj[key]).trim();
        break;
      }
    }

    for (const key of ['email', 'Email', 'mail']) {
      if (typeof obj[key] === 'string' && obj[key].includes('@')) {
        acc.email = obj[key].trim();
        break;
      }
    }

    for (const key of ['ScreenName', 'screenName', 'NickName', 'nickName', 'nickname', 'username', 'userName', 'displayName']) {
      if (isValidDisplayName(obj[key])) {
        acc.name = obj[key].trim();
        break;
      }
    }

    for (const key of ['refreshToken', 'refresh_token']) {
      if (isLikelyAuthToken(obj[key])) {
        acc.refreshToken = obj[key].trim();
        break;
      }
    }

    if (Array.isArray(obj)) {
      for (let i = 0; i < Math.min(obj.length, 5); i++) {
        collectFields(obj[i], d + 1, acc);
      }
      return;
    }

    for (const k of Object.keys(obj)) {
      if (obj[k] && typeof obj[k] === 'object') {
        collectFields(obj[k], d + 1, acc);
      }
    }
  }

  function tryEmit(sourceObj, fromTrustedEndpoint) {
    if (captured) return;
    // 仍在登录页或第三方 OAuth 页时，仅放行明确已登录的 GetUserToken / iCubeAuthInfo 凭据
    if (isOnLoginPage() && !fromTrustedEndpoint) return;

    const acc = loadAcc();
    if (sourceObj && typeof sourceObj === 'object') {
      collectFields(sourceObj, 0, acc);
    }

    if (acc.token && !acc.userId) {
      const claims = decodeJwtPayload(acc.token);
      if (claims && typeof claims === 'object') {
        const claimUid = claims.userId || claims.uid || claims.user_id || claims.sub || claims.uin;
        if (isValidUid(claimUid)) acc.userId = String(claimUid).trim();
        if (!acc.email && typeof claims.email === 'string' && claims.email.includes('@')) {
          acc.email = claims.email.trim();
        }
        if (!acc.name && typeof (claims.name || claims.nickname) === 'string') {
          acc.name = (claims.name || claims.nickname).trim();
        }
      }
    }

    saveAcc(acc);

    // 必须同时具备有效 token 与真实用户身份 (userId 或 email)
    if (acc.token && (acc.userId || acc.email)) {
      captured = true;
      try { window.sessionStorage.removeItem(ACC_KEY); } catch (_) {}
      const payloadStr = JSON.stringify({
        token: acc.token,
        userId: acc.userId || undefined,
        email: acc.email || undefined,
        name: acc.name || undefined,
        refreshToken: acc.refreshToken || undefined,
      });
      window.location.href = 'https://qwitch.local/oauth-callback?nonce=' + encodeURIComponent('__QWITCH_NONCE__') + '&payload=' + encodeURIComponent(payloadStr);
    }
  }

  function isIgnoredUrl(url) {
    return /qrcode|qr_code|get_qr|check_qr|qrconnect|csrf|captcha|config|anon|guest|metrics|telemetry|report|tea\/v|slardar|passport\/web\/get/i.test(url);
  }

  function isTrustedAuthUrl(url) {
    return /GetUserToken|iCubeAuthInfo|\/v2\/billing\/meter\/|\/sash\/api\/v1\/me\//i.test(url);
  }

  function inspectResponseText(url, text) {
    if (captured || !text || isIgnoredUrl(url)) return;
    const trimmed = text.trim();
    if (trimmed[0] !== '{' && trimmed[0] !== '[') return;
    try {
      const json = JSON.parse(trimmed);
      tryEmit(json, isTrustedAuthUrl(url));
    } catch (_) {}
  }

  const origFetch = window.fetch;
  window.fetch = async function(...args) {
    const resp = await origFetch.apply(this, args);
    try {
      const url = String(args[0] && args[0].url ? args[0].url : args[0] || '');
      if (!isIgnoredUrl(url) && /GetUserToken|GetUserInfo|userinfo|user\/info|account|session|campaigns|checkin|billing/i.test(url)) {
        resp.clone().text().then(txt => inspectResponseText(url, txt)).catch(() => {});
      }
    } catch (_) {}
    return resp;
  };

  const origOpen = XMLHttpRequest.prototype.open;
  const origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function(method, url) {
    this.__qwitchUrl = String(url || '');
    return origOpen.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function() {
    this.addEventListener('load', function() {
      try {
        const url = this.__qwitchUrl || '';
        if (!isIgnoredUrl(url) && /GetUserToken|GetUserInfo|userinfo|user\/info|account|session|campaigns|checkin|billing/i.test(url)) {
          inspectResponseText(url, this.responseText);
        }
      } catch (_) {}
    });
    return origSend.apply(this, arguments);
  };

  function scanStorage() {
    if (captured) return;
    for (const store of [window.localStorage, window.sessionStorage]) {
      try {
        if (!store) continue;
        for (let i = 0; i < store.length; i++) {
          const k = store.key(i);
          if (!k || k === ACC_KEY) continue;
          // 仅扫描明确的工具登录态键名，绝不扫描埋点/统计键 (__tea_cache 等)
          if (!/iCubeAuthInfo|workbuddy|qoder|auth_info|user_info/i.test(k)) continue;
          const v = store.getItem(k);
          if (!v) continue;
          const trimmed = v.trim();
          if (trimmed[0] === '{') {
            try {
              tryEmit(JSON.parse(trimmed), k.startsWith('iCubeAuthInfo'));
              if (captured) return;
            } catch (_) {}
          }
        }
      } catch (_) {}
    }
  }

  // 将第三方登录（如抖音/微信/阿里云）通过 window.open 或 target="_blank" 打开的页面直接引导到当前窗口显示
  window.open = function(url) {
    if (typeof url === 'string' && url && url !== 'about:blank') {
      window.location.href = url;
      return window;
    }
    return {
      closed: false,
      close: function() {},
      focus: function() {},
      location: {
        set href(u) { if (typeof u === 'string' && u) window.location.href = u; },
        get href() { return window.location.href; },
        replace: function(u) { if (typeof u === 'string' && u) window.location.replace(u); },
        assign: function(u) { if (typeof u === 'string' && u) window.location.assign(u); }
      }
    };
  };

  document.addEventListener('click', function(e) {
    const a = e.target && e.target.closest ? e.target.closest('a[target="_blank"]') : null;
    if (a && a.href && /^https?:\/\//i.test(a.href)) {
      e.preventDefault();
      window.location.href = a.href;
    }
  }, true);

  setInterval(scanStorage, 1000);
})();
"#;

const POPUP_REDIRECT_JS: &str = r#"
(function() {
  if (window.__qwitchPopupInjected) return;
  window.__qwitchPopupInjected = true;
  window.open = function(url) {
    if (typeof url === 'string' && url && url !== 'about:blank') {
      window.location.href = url;
      return window;
    }
    return {
      closed: false,
      close: function() {},
      focus: function() {},
      location: {
        set href(u) { if (typeof u === 'string' && u) window.location.href = u; },
        get href() { return window.location.href; },
        replace: function(u) { if (typeof u === 'string' && u) window.location.replace(u); },
        assign: function(u) { if (typeof u === 'string' && u) window.location.assign(u); }
      }
    };
  };
  document.addEventListener('click', function(e) {
    const a = e.target && e.target.closest ? e.target.closest('a[target="_blank"]') : null;
    if (a && a.href && /^https?:\/\//i.test(a.href)) {
      e.preventDefault();
      window.location.href = a.href;
    }
  }, true);
})();
"#;

async fn persist_oauth_envelope(
    app: &AppHandle,
    db: &Arc<QwitchDatabase>,
    host: &Arc<dyn PlatformHost>,
    master_key: &[u8; 32],
    adapter: &Arc<dyn ToolAdapter>,
    flow_seq: &Arc<AtomicU64>,
    flow_id: u64,
    tool_id: &str,
    label: &str,
    target_account_id: Option<&str>,
    envelope: crate::domain::CredentialEnvelope,
) {
    if flow_seq.load(Ordering::SeqCst) != flow_id {
        return;
    }

    if !is_valid_oauth_uid(&envelope.uid) {
        let _ = app.emit(
            "qwitch://oauth-error",
            "授权返回的账号 UID 无效（疑似非字符串身份字段），已拒绝导入",
        );
        return;
    }

    let mut matched_local = false;
    let envelope_to_save = match adapter.import_from_local(host.as_ref()).await {
        Ok(local_env)
            if !local_env.uid.trim().is_empty() && local_env.uid == envelope.uid =>
        {
            matched_local = true;
            local_env.merge_from(&envelope)
        }
        _ => envelope,
    };

    if flow_seq.load(Ordering::SeqCst) != flow_id {
        return;
    }

    let save_res = if let Some(acc_id) = target_account_id {
        let label_override = (!label.is_empty()).then_some(label);
        db.bind_account_credential(master_key, acc_id, label_override, &envelope_to_save)
    } else {
        db.upsert_account(
            master_key,
            (!label.is_empty()).then_some(label),
            &envelope_to_save,
        )
    };
    match save_res {
        Ok(summary) => {
            if matched_local {
                let _ = db.set_active_on_client(tool_id, &summary.id);
            }
            let cred_for_quota = db
                .get_account_credential(master_key, &summary.id)
                .map(|(_, env)| env)
                .unwrap_or(envelope_to_save);
            record_import_quota_check(db, adapter, &summary, &cred_for_quota).await;
            let log_msg = if target_account_id.is_some() {
                format!(
                    "通过内置授权补充/更新 {} ({})",
                    summary.label, summary.uid_masked
                )
            } else {
                format!(
                    "通过内置授权导入 {} ({})",
                    summary.label, summary.uid_masked
                )
            };
            let _ = db.append_action_log(
                Some(&summary.id),
                tool_id,
                "import",
                "success",
                None,
                &log_msg,
            );
            let _ = app.emit("qwitch://account-updated", &summary.id);
            let _ = app.emit("qwitch://logs-updated", ());
        }
        Err(e) => {
            let _ = app.emit("qwitch://oauth-error", format!("保存账号失败: {e}"));
        }
    }
    if flow_seq.load(Ordering::SeqCst) == flow_id {
        if let Some(win) = app.get_webview_window("oauth-login") {
            let _ = win.close();
        }
    }
}

#[tauri::command]
pub async fn start_webview_oauth(
    app: AppHandle,
    state: State<'_, AppState>,
    tool_id: String,
    label: String,
    target_account_id: Option<String>,
) -> Result<(), String> {
    let flow_id = state.oauth_flow_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let flow_nonce = uuid::Uuid::new_v4().to_string();

    if let Some(existing) = app.get_webview_window("oauth-login") {
        let _ = existing.close();
    }

    enum OAuthMode {
        TraeCapture,
        WorkBuddyPoll { state_token: String },
        QoderPkcePoll {
            nonce: String,
            verifier: String,
            machine_id: String,
        },
    }

    let (login_url, mode) = match tool_id.as_str() {
        "trae_cn" => ("https://www.trae.cn/login".to_string(), OAuthMode::TraeCapture),
        "workbuddy" => {
            let (state_token, auth_url) = WorkBuddyAdapter::create_oauth_session().await?;
            (auth_url, OAuthMode::WorkBuddyPoll { state_token })
        }
        "qoder_cn" => {
            let (auth_url, nonce, verifier, machine_id) =
                QoderAdapter::create_pkce_device_flow(state.host.as_ref());
            (
                auth_url,
                OAuthMode::QoderPkcePoll {
                    nonce,
                    verifier,
                    machine_id,
                },
            )
        }
        _ => return Err(format!("工具 {tool_id} 暂不支持内置 WebView 授权")),
    };

    // 导航白名单：授权窗口只允许停留在对应工具的官方域名，
    // 防止被诱导跳转到第三方页面后注入脚本收集凭据
    let allowed_auth_hosts: Vec<String> = match tool_id.as_str() {
        "trae_cn" => vec!["trae.cn".to_string()],
        "workbuddy" => vec![
            "copilot.tencent.com".to_string(),
            "workbuddy.cn".to_string(),
        ],
        "qoder_cn" => vec!["qoder.cn".to_string(), "qoder.com.cn".to_string()],
        _ => vec![],
    };

    let parsed_url: tauri::Url = login_url.parse().map_err(|e| format!("URL 解析失败: {e}"))?;
    let adapter = state
        .adapters
        .get(&tool_id)
        .cloned()
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;

    let db = Arc::clone(&state.db);
    let host_arc = Arc::clone(&state.host);
    let master_key = state.master_key;
    let flow_seq = Arc::clone(&state.oauth_flow_seq);
    let app_handle = app.clone();
    let tool_id_owned = tool_id.clone();
    let label_owned = label.trim().to_string();
    let target_id_owned = target_account_id.filter(|s| !s.trim().is_empty());

    let app_for_popup = app.clone();
    let allowed_for_popup = allowed_auth_hosts.clone();
    let init_js: String = match mode {
        OAuthMode::TraeCapture => OAUTH_CAPTURE_JS.replace("__QWITCH_NONCE__", &flow_nonce),
        OAuthMode::WorkBuddyPoll { .. } | OAuthMode::QoderPkcePoll { .. } => {
            POPUP_REDIRECT_JS.to_string()
        }
    };

    let mut builder = WebviewWindowBuilder::new(&app, "oauth-login", WebviewUrl::External(parsed_url))
        .title(format!("Qwitch 内置授权登录 - {}", adapter.manifest().name))
        .inner_size(960.0, 720.0)
        .center()
        .incognito(true)
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .initialization_script(&init_js)
        .on_new_window(move |url, _features| {
            if url.as_str() != "about:blank" {
                let host = url.host_str().unwrap_or("");
                let allowed = allowed_for_popup
                    .iter()
                    .any(|base| host == base || host.ends_with(&format!(".{base}")));
                if allowed {
                    if let Some(win) = app_for_popup.get_webview_window("oauth-login") {
                        let _ = win.navigate(url);
                    }
                }
            }
            tauri::webview::NewWindowResponse::Deny
        });

    if matches!(mode, OAuthMode::TraeCapture) {
        let app_nav = app_handle.clone();
        let db_nav = Arc::clone(&db);
        let host_nav = Arc::clone(&host_arc);
        let adapter_nav = Arc::clone(&adapter);
        let flow_seq_nav = Arc::clone(&flow_seq);
        let expected_nonce = flow_nonce.clone();
        let tid_nav = tool_id_owned.clone();
        let lbl_nav = label_owned.clone();
        let target_nav = target_id_owned.clone();
        let allowed_nav = allowed_auth_hosts.clone();

        builder = builder.on_navigation(move |nav_url| {
            if nav_url.host_str() == Some("qwitch.local") {
                if nav_url.path() != "/oauth-callback"
                    || flow_seq_nav.load(Ordering::SeqCst) != flow_id
                {
                    return false;
                }
                let callback_nonce = nav_url
                    .query_pairs()
                    .find(|(k, _)| k == "nonce")
                    .map(|(_, v)| v.to_string());
                if callback_nonce.as_deref() != Some(expected_nonce.as_str()) {
                    let _ = app_nav.emit(
                        "qwitch://oauth-error",
                        "内置授权回调校验失败（会话状态不匹配），已拒绝该请求",
                    );
                    return false;
                }

                let raw_payload = nav_url
                    .query_pairs()
                    .find(|(k, _)| k == "payload")
                    .or_else(|| nav_url.query_pairs().find(|(k, _)| k == "token"))
                    .map(|(_, v)| v.to_string());

                if let Some(payload_str) = raw_payload {
                    let app_clone = app_nav.clone();
                    let db_clone = Arc::clone(&db_nav);
                    let host_clone = Arc::clone(&host_nav);
                    let adapter_clone = Arc::clone(&adapter_nav);
                    let flow_seq_clone = Arc::clone(&flow_seq_nav);
                    let tid = tid_nav.clone();
                    let lbl = lbl_nav.clone();
                    let target_id = target_nav.clone();

                    tauri::async_runtime::spawn(async move {
                        match adapter_clone.parse_manual_token(&payload_str).await {
                            Ok(envelope) => {
                                persist_oauth_envelope(
                                    &app_clone,
                                    &db_clone,
                                    &host_clone,
                                    &master_key,
                                    &adapter_clone,
                                    &flow_seq_clone,
                                    flow_id,
                                    &tid,
                                    &lbl,
                                    target_id.as_deref(),
                                    envelope,
                                )
                                .await;
                            }
                            Err(e) => {
                                let _ = app_clone.emit(
                                    "qwitch://oauth-error",
                                    format!("解析授权凭据失败: {e}"),
                                );
                                if flow_seq_clone.load(Ordering::SeqCst) == flow_id {
                                    if let Some(win) = app_clone.get_webview_window("oauth-login") {
                                        let _ = win.close();
                                    }
                                }
                            }
                        }
                    });
                }
                return false;
            }
            let host = nav_url.host_str().unwrap_or("");
            allowed_nav
                .iter()
                .any(|base| host == base || host.ends_with(&format!(".{base}")))
        });
    }

    if !matches!(mode, OAuthMode::TraeCapture) {
        let allowed_nav = allowed_auth_hosts.clone();
        builder = builder.on_navigation(move |nav_url| {
            let host = nav_url.host_str().unwrap_or("");
            allowed_nav
                .iter()
                .any(|base| host == base || host.ends_with(&format!(".{base}")))
        });
    }

    builder
        .build()
        .map_err(|e| format!("打开内置授权窗口失败: {e}"))?;

    match mode {
        OAuthMode::TraeCapture => {}
        OAuthMode::WorkBuddyPoll { state_token } => {
            tauri::async_runtime::spawn(async move {
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if flow_seq.load(Ordering::SeqCst) != flow_id
                        || app_handle.get_webview_window("oauth-login").is_none()
                    {
                        break;
                    }
                    match WorkBuddyAdapter::poll_oauth_result(&state_token).await {
                        Ok(Some(envelope)) => {
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &flow_seq,
                                flow_id,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                envelope,
                            )
                            .await;
                            break;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            if flow_seq.load(Ordering::SeqCst) == flow_id {
                                let _ = app_handle.emit("qwitch://oauth-error", err);
                                if let Some(win) = app_handle.get_webview_window("oauth-login") {
                                    let _ = win.close();
                                }
                            }
                            break;
                        }
                    }
                }
            });
        }
        OAuthMode::QoderPkcePoll {
            nonce,
            verifier,
            machine_id,
        } => {
            tauri::async_runtime::spawn(async move {
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if flow_seq.load(Ordering::SeqCst) != flow_id
                        || app_handle.get_webview_window("oauth-login").is_none()
                    {
                        break;
                    }
                    match QoderAdapter::poll_pkce_device_token(&nonce, &verifier, &machine_id).await {
                        Ok(Some(envelope)) => {
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &flow_seq,
                                flow_id,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                envelope,
                            )
                            .await;
                            break;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            if flow_seq.load(Ordering::SeqCst) == flow_id {
                                let _ = app_handle.emit("qwitch://oauth-error", err);
                                if let Some(win) = app_handle.get_webview_window("oauth-login") {
                                    let _ = win.close();
                                }
                            }
                            break;
                        }
                    }
                }
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_uid_validation_rejects_objects_and_blacklist() {
        assert!(is_valid_oauth_uid("u_123456"));
        assert!(is_valid_oauth_uid("1234567890"));
        assert!(!is_valid_oauth_uid("[object Object]"));
        assert!(!is_valid_oauth_uid("a,b"));
        assert!(!is_valid_oauth_uid("has space"));
        assert!(!is_valid_oauth_uid("{\"id\":1}"));
        assert!(!is_valid_oauth_uid(""));
        assert!(!is_valid_oauth_uid("x"));
        assert!(!is_valid_oauth_uid("null"));
        assert!(!is_valid_oauth_uid("0"));
        assert!(!is_valid_oauth_uid("guest"));
    }
}
