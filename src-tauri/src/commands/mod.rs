use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::adapter::{
    minimax::MiniMaxAdapter, qoder::QoderAdapter, workbuddy::WorkBuddyAdapter, AdapterError,
    AdapterManifest, ToolAdapter,
};
use crate::domain::{
    AccountStatus, AccountSummary, ActionLogCleanupRequest, ActionLogItem, ActionLogStats,
    AppSettingsDto, AuthMethod, ExportLogsResult, LogScope, QuotaSnapshot, UnifiedErrorCode,
};
use crate::platform::{PlatformHost, ScopedPlatformHost};
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

/// 解析导入来源宿主：指定 profile 时用 ScopedPlatformHost 把 config_dir 指向该隔离 profile，
/// 从而采集该 profile 独立注册的设备身份（多设备方案 A）。
fn import_host(
    host: &Arc<dyn PlatformHost>,
    tool_id: &str,
    profile_dir: Option<&str>,
) -> Arc<dyn PlatformHost> {
    match profile_dir.map(str::trim).filter(|s| !s.is_empty()) {
        Some(dir) => {
            let config_dir = PathBuf::from(dir).join("User").join("globalStorage");
            Arc::new(ScopedPlatformHost::new(
                Arc::clone(host),
                tool_id,
                config_dir,
            ))
        }
        None => Arc::clone(host),
    }
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

fn read_device_id_from_storage(storage_path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read(storage_path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value
        .as_object()?
        .keys()
        .find_map(|key| key.strip_prefix("iCubeAuthInfo://icube-dc:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn mask_device_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 8 {
        format!("{}****", chars.iter().take(4).collect::<String>())
    } else {
        let prefix: String = chars.iter().take(4).collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{prefix}****{suffix}")
    }
}

/// 只读预检一个“设备 profile 目录”是否可用于导入：
/// 是否含 storage.json、其设备 ID（脱敏）、是否与当前本机同一设备。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInspectResult {
    pub has_storage: bool,
    pub storage_path: String,
    pub device_id_masked: Option<String>,
    pub same_as_local: Option<bool>,
}

#[tauri::command]
pub fn inspect_profile_dir(
    state: State<'_, AppState>,
    tool_id: String,
    profile_dir: String,
) -> Result<ProfileInspectResult, String> {
    let dir = profile_dir.trim();
    if dir.is_empty() {
        return Err("设备 profile 目录为空".into());
    }
    let storage = PathBuf::from(dir)
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let has_storage = storage.is_file();
    let device_id = if has_storage {
        read_device_id_from_storage(&storage)
    } else {
        None
    };
    let local_id = state
        .host
        .client_config_dir(&tool_id)
        .ok()
        .map(|d| d.join("storage.json"))
        .and_then(|p| read_device_id_from_storage(&p));
    let same_as_local = match (&device_id, &local_id) {
        (Some(a), Some(b)) => Some(a == b),
        _ => None,
    };
    Ok(ProfileInspectResult {
        has_storage,
        storage_path: storage.to_string_lossy().to_string(),
        device_id_masked: device_id.as_deref().map(mask_device_id),
        same_as_local,
    })
}

/// 原生“选择文件夹”对话框（macOS 需在主线程执行 rfd 弹窗）。
#[tauri::command]
pub async fn pick_folder(app: AppHandle, default_path: Option<String>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let default = default_path.filter(|p| !p.trim().is_empty());
    let spawn = app.run_on_main_thread(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(p) = default.as_deref() {
            if std::path::Path::new(p).exists() {
                dialog = dialog.set_directory(p);
            }
        }
        let picked = dialog
            .pick_folder()
            .map(|p| p.to_string_lossy().to_string());
        let _ = tx.send(picked);
    });
    if spawn.is_err() {
        return None;
    }
    rx.await.ok().flatten()
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
    present_main_window(&app);
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
pub fn list_external_adapters(
    state: State<'_, AppState>,
) -> Result<Vec<ExternalAdapterInfo>, String> {
    let data_dir = state.host.qwitch_data_dir().map_err(|e| e.to_string())?;
    let settings = load_app_settings(&state.db);
    let mut list: Vec<ExternalAdapterInfo> =
        crate::adapter::external::scan_external_manifests(&data_dir)
            .into_iter()
            .filter(|(_, m)| {
                crate::adapter::is_valid_adapter_id(&m.id)
                    && (m.platforms.is_empty()
                        || m.platforms.iter().any(|p| {
                            p == crate::adapter::external::ExternalAdapter::current_platform_tag()
                        }))
            })
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
    let mut seen = std::collections::HashSet::with_capacity(list.len());
    list.retain(|item| seen.insert(item.id.clone()));
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
    let previous = load_app_settings(&state.db);
    for (tool, path) in &settings.custom_client_paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        // 仅校验本次新增/修改的路径；历史失效路径不阻塞无关设置保存
        if previous.custom_client_paths.get(tool).map(|p| p.trim()) == Some(trimmed) {
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
    let _ = state.db.apply_log_retention(
        settings.log_archive_after_days,
        settings.log_max_active_count,
    );
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

/// 校正“本机使用中”标记：读取各工具本机客户端当前真实登录账号，
/// 与本库账号比对后更新 is_active_on_client（读不到真实 uid 的工具跳过，绝不误清）。
#[tauri::command]
pub async fn reconcile_active_accounts(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let adapters: Vec<(String, Arc<dyn ToolAdapter>)> = state
        .adapters
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (tool_id, adapter) in adapters {
        if !adapter
            .manifest()
            .capabilities
            .auth_methods
            .contains(&AuthMethod::LocalImport)
        {
            continue;
        }
        let env = match adapter.import_from_local(state.host.as_ref()).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        let uid = env.uid.trim();
        if uid.is_empty() || crate::domain::CredentialEnvelope::is_fallback_uid(uid) {
            // 未能可靠解密出真实 uid：跳过，避免把正确的标记误清
            continue;
        }
        match state.db.find_account_id_by_uid(&tool_id, uid) {
            Ok(Some((id, _))) => {
                let _ = state.db.set_active_on_client(&tool_id, &id);
                let _ = app.emit("qwitch://account-updated", &id);
            }
            Ok(None) => {
                // 本机当前账号不在库中 → 清掉该工具下所有“使用中”标记
                let _ = state.db.clear_active_on_client(&tool_id);
            }
            Err(_) => {}
        }
    }
    Ok(())
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

async fn restore_session_note(
    adapter: &Arc<dyn ToolAdapter>,
    host: &dyn PlatformHost,
    backup: &crate::adapter::SessionBackup,
) -> String {
    match adapter.restore_session(host, backup).await {
        Ok(()) => "已恢复原会话".to_string(),
        Err(e) => format!("原会话恢复失败（{e}），请手动重新登录客户端"),
    }
}

async fn record_import_quota_check(
    db: &Arc<QwitchDatabase>,
    adapter: &Arc<dyn ToolAdapter>,
    summary: &AccountSummary,
    cred: &crate::domain::CredentialEnvelope,
) {
    if !summary.has_api_token || !adapter.manifest().capabilities.supports_quota_query {
        return;
    }
    match adapter.inspect_quota(cred).await {
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

async fn schedule_import_quota_check(
    app: &AppHandle,
    db: &Arc<QwitchDatabase>,
    adapter: &Arc<dyn ToolAdapter>,
    summary: &AccountSummary,
    cred: &crate::domain::CredentialEnvelope,
    scheduler: &Arc<AntiRiskScheduler>,
) {
    if !summary.has_api_token || !adapter.manifest().capabilities.supports_quota_query {
        return;
    }
    if let Ok(guard) = scheduler.queue_lock().try_lock_owned() {
        record_import_quota_check(db, adapter, summary, cred).await;
        drop(guard);
        return;
    }

    let app_bg = app.clone();
    let db_bg = Arc::clone(db);
    let adapter_bg = Arc::clone(adapter);
    let summary_bg = summary.clone();
    let cred_bg = cred.clone();
    let scheduler_bg = Arc::clone(scheduler);
    tokio::spawn(async move {
        let _guard = scheduler_bg.queue_lock().lock_owned().await;
        record_import_quota_check(&db_bg, &adapter_bg, &summary_bg, &cred_bg).await;
        let _ = app_bg.emit("qwitch://account-updated", &summary_bg.id);
        let _ = app_bg.emit("qwitch://logs-updated", ());
    });
}

#[tauri::command]
pub async fn import_local_account(
    app: AppHandle,
    state: State<'_, AppState>,
    tool_id: String,
    label: String,
    target_account_id: Option<String>,
    profile_dir: Option<String>,
    device_platform: Option<String>,
) -> Result<AccountSummary, String> {
    let adapter = state
        .adapters
        .get(&tool_id)
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;
    if !adapter
        .manifest()
        .capabilities
        .auth_methods
        .contains(&AuthMethod::LocalImport)
    {
        return Err(format!("适配器 {tool_id} 不支持本机导入"));
    }

    let host = import_host(&state.host, &tool_id, profile_dir.as_deref());
    let mut envelope = adapter
        .import_from_local(host.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    if envelope.tool_id != tool_id {
        return Err(format!(
            "适配器返回工具标识不匹配：期望 {tool_id}，实际 {}",
            envelope.tool_id
        ));
    }

    // 跨 OS 导入：按来源设备覆盖设备平台，避免 x-device-type 与本机不符
    if let Some(plat) = device_platform
        .as_deref()
        .map(str::trim)
        .filter(|p| matches!(*p, "mac" | "win" | "linux"))
    {
        let current = if cfg!(target_os = "macos") {
            "mac"
        } else if cfg!(target_os = "windows") {
            "win"
        } else {
            "linux"
        };
        envelope.override_device_platform(plat, plat != current);
    }

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
    schedule_import_quota_check(
        &app,
        &state.db,
        adapter,
        &summary,
        &cred_for_quota,
        &state.scheduler,
    )
    .await;

    let is_external_profile = profile_dir
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    // 仅“真·本机导入”才把该账号标记为本机使用中；外部设备 profile 只是采集设备身份，
    // 并不代表本机客户端当前登录的是它，否则会出现“状态错 / 切号按钮被误禁”。
    if !is_external_profile {
        let _ = state.db.set_active_on_client(&tool_id, &summary.id);
    }

    let log_msg = if target_id.is_some() {
        format!(
            "补充/更新账号快照 {} ({})",
            summary.label, summary.uid_masked
        )
    } else if is_external_profile {
        format!(
            "从外部设备 profile 导入 {} ({})",
            summary.label, summary.uid_masked
        )
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountPreview {
    pub tool_id: String,
    pub uid: String,
    pub uid_masked: String,
    pub display_name: Option<String>,
    pub is_fallback_uid: bool,
    pub matches_target: bool,
    pub matched_account_id: Option<String>,
    pub matched_account_label: Option<String>,
}

/// 只读预检本机客户端当前登录账号，用于「关联本机」前判断归属，
/// 避免用户在错误的卡片上反复触发绑定失败。
#[tauri::command]
pub async fn preview_local_account(
    state: State<'_, AppState>,
    tool_id: String,
    account_id: Option<String>,
    profile_dir: Option<String>,
) -> Result<LocalAccountPreview, String> {
    let adapter = state
        .adapters
        .get(&tool_id)
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;
    if !adapter
        .manifest()
        .capabilities
        .auth_methods
        .contains(&AuthMethod::LocalImport)
    {
        return Err(format!("适配器 {tool_id} 不支持本机导入"));
    }

    let host = import_host(&state.host, &tool_id, profile_dir.as_deref());
    let envelope = adapter
        .import_from_local(host.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    let matched = state
        .db
        .find_account_id_by_uid(&tool_id, &envelope.uid)
        .map_err(|e| e.to_string())?;
    let matches_target = account_id.as_deref().is_some_and(|id| {
        state
            .db
            .get_account_credential(&state.master_key, id)
            .map(|(_, target)| target.uid == envelope.uid)
            .unwrap_or(false)
    });

    Ok(LocalAccountPreview {
        uid_masked: crate::storage::crypto::mask_uid(&envelope.uid),
        is_fallback_uid: crate::domain::CredentialEnvelope::is_fallback_uid(&envelope.uid),
        matches_target,
        matched_account_id: matched.as_ref().map(|(id, _)| id.clone()),
        matched_account_label: matched.map(|(_, label)| label),
        tool_id: envelope.tool_id,
        uid: envelope.uid,
        display_name: envelope.display_name,
    })
}

/// 拉起某工具的隔离 profile（`--user-data-dir`），用于首次在该 profile 登录以注册独立设备。
/// 返回 profile 根目录（即 `--user-data-dir` 指向目录），供后续按 profile 导入采集设备身份。
#[tauri::command]
pub fn launch_tool_profile(
    state: State<'_, AppState>,
    tool_id: String,
    profile_dir: String,
) -> Result<String, String> {
    let dir = profile_dir.trim();
    if dir.is_empty() {
        return Err("profile_dir 不能为空".into());
    }
    if crate::platform::client_spec(&tool_id).is_none() {
        return Err(format!("未知或不支持多实例的工具: {tool_id}"));
    }
    let settings = load_app_settings(&state.db);
    let exe = resolve_client_executable(state.host.as_ref(), &settings, &tool_id)
        .ok_or_else(|| format!("未检测到 {tool_id} 客户端安装路径，请先在设置中配置"))?;
    let profile = PathBuf::from(dir);
    std::fs::create_dir_all(&profile).map_err(|e| format!("创建 profile 目录失败: {e}"))?;
    let extensions_dir = profile.join("extensions");
    std::fs::create_dir_all(&extensions_dir).ok();
    let args = vec![
        "--user-data-dir".to_string(),
        profile.to_string_lossy().to_string(),
        "--extensions-dir".to_string(),
        extensions_dir.to_string_lossy().to_string(),
    ];
    state
        .host
        .launch_client(&exe, &args)
        .map_err(|e| e.to_string())?;
    Ok(profile.to_string_lossy().to_string())
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
    if !adapter
        .manifest()
        .capabilities
        .auth_methods
        .contains(&AuthMethod::TokenPaste)
    {
        return Err(format!("适配器 {tool_id} 不支持凭据粘贴导入"));
    }

    let envelope = adapter
        .parse_manual_token(&raw_input)
        .await
        .map_err(|e| e.to_string())?;
    if envelope.tool_id != tool_id {
        return Err(format!(
            "适配器返回工具标识不匹配：期望 {tool_id}，实际 {}",
            envelope.tool_id
        ));
    }

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
    schedule_import_quota_check(
        &app,
        &state.db,
        adapter,
        &summary,
        &cred_for_quota,
        &state.scheduler,
    )
    .await;

    let log_msg = if target_id.is_some() {
        format!("补充/更新凭据 {} ({})", summary.label, summary.uid_masked)
    } else {
        format!(
            "通过手动凭据添加 {} ({})",
            summary.label, summary.uid_masked
        )
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
    if !adapter.manifest().capabilities.supports_quota_query {
        return Err(format!("适配器 {} 不支持额度查询", summary.tool_id));
    }

    // 单账号查额同样进入串行队列，批量运行中排队等待而非并发请求平台
    let _queue_guard = state.scheduler.queue_lock().lock_owned().await;

    // 请求前按需刷新 OAuth 凭据（如 MiniMax accessToken），刷新结果立即加密落盘
    let cred = match adapter.refresh_credential(&cred).await {
        Ok(Some(updated)) => {
            if let Err(e) =
                state
                    .db
                    .bind_account_credential(&state.master_key, &summary.id, None, &updated)
            {
                let _ = state.db.append_action_log(
                    Some(&summary.id),
                    &summary.tool_id,
                    "query",
                    "failed",
                    Some(&UnifiedErrorCode::InternalError),
                    &format!("{} 刷新后的凭据落盘失败：{e}", summary.label),
                );
            }
            updated
        }
        Ok(None) => cred,
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
                &format!("{} 刷新凭据失败：{message}", summary.label),
            );
            let _ = app.emit("qwitch://account-updated", &summary.id);
            let _ = app.emit("qwitch://logs-updated", ());
            return Err(message);
        }
    };

    match adapter.inspect_quota(&cred).await {
        Ok(snapshot) => {
            state
                .db
                .save_quota_snapshot(&summary.id, &snapshot)
                .map_err(|e| format!("额度已查询但落盘失败: {e}"))?;
            if matches!(
                summary.status,
                AccountStatus::Expired | AccountStatus::RiskPaused
            ) {
                let _ = state
                    .db
                    .update_account_status(&summary.id, AccountStatus::Active);
            }
            let _ = state.db.append_action_log(
                Some(&summary.id),
                &summary.tool_id,
                "query",
                "success",
                None,
                &format!(
                    "已刷新 {} ({}) 的最新额度",
                    summary.label, summary.uid_masked
                ),
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
    fn try_acquire(flag: Arc<AtomicBool>) -> Result<Self, String> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "切号进行中，请等待当前切换完成后再试".to_string())?;
        Ok(Self { flag, armed: true })
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

fn log_switch_failure(
    db: &QwitchDatabase,
    app: &AppHandle,
    summary: &AccountSummary,
    code: UnifiedErrorCode,
    message: &str,
) {
    let _ = db.append_action_log(
        Some(&summary.id),
        &summary.tool_id,
        "switch",
        "failed",
        Some(&code),
        &format!(
            "{} ({}) 切号失败：{message}",
            summary.label, summary.uid_masked
        ),
    );
    let _ = app.emit("qwitch://logs-updated", ());
}

async fn check_client_running_async(
    host: &Arc<dyn PlatformHost>,
    tool_id: &str,
) -> Result<bool, String> {
    let h = Arc::clone(host);
    let tid = tool_id.to_string();
    tokio::task::spawn_blocking(move || h.is_client_running(&tid))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn switch_desktop_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    force_kill: bool,
) -> Result<(), String> {
    let mut switch_guard = SwitchFlagGuard::try_acquire(Arc::clone(&state.switching_in_progress))?;

    let (summary, cred) = state
        .db
        .get_account_credential(&state.master_key, &account_id)
        .map_err(|e| e.to_string())?;

    let adapter = state
        .adapters
        .get(&summary.tool_id)
        .ok_or_else(|| format!("适配器 {} 尚未注册", summary.tool_id))?;
    if !adapter.manifest().capabilities.supports_client_switch {
        let msg = format!("适配器 {} 不支持桌面端切号", summary.tool_id);
        log_switch_failure(
            &state.db,
            &app,
            &summary,
            UnifiedErrorCode::UnsupportedCapability,
            &msg,
        );
        return Err(msg);
    }

    let settings = load_app_settings(&state.db);
    let Some(exe) = resolve_client_executable(state.host.as_ref(), &settings, &summary.tool_id)
    else {
        let msg = format!(
            "未检测到 {} 客户端安装路径，请在设置中配置自定义路径后再切换",
            summary.tool_id
        );
        log_switch_failure(
            &state.db,
            &app,
            &summary,
            UnifiedErrorCode::ClientNotFound,
            &msg,
        );
        return Err(msg);
    };

    let running = match check_client_running_async(&state.host, &summary.tool_id).await {
        Ok(r) => r,
        Err(e) => {
            log_switch_failure(
                &state.db,
                &app,
                &summary,
                UnifiedErrorCode::InternalError,
                &e,
            );
            return Err(e);
        }
    };

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
        if let Err(e) = state
            .host
            .terminate_client_gracefully(&summary.tool_id, 5000)
            .await
        {
            let msg = e.to_string();
            log_switch_failure(
                &state.db,
                &app,
                &summary,
                UnifiedErrorCode::ClientRunning,
                &msg,
            );
            return Err(msg);
        }
    }

    // 终止后备份权威会话（避免读到运行中半写文件）；备份失败则重拉原客户端
    let backup = match adapter.backup_session(state.host.as_ref()).await {
        Ok(b) => b,
        Err(e) => {
            if running {
                let _ = state.host.launch_client(&exe, &[]);
            }
            let msg = format!(
                "备份客户端会话失败{}: {e}",
                if running {
                    "，已尝试重新拉起原客户端"
                } else {
                    "（客户端未做任何改动）"
                }
            );
            log_switch_failure(
                &state.db,
                &app,
                &summary,
                UnifiedErrorCode::InternalError,
                &msg,
            );
            return Err(msg);
        }
    };

    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "writing".into(),
        },
    );

    if let Err(e) = adapter.apply_session(state.host.as_ref(), &cred).await {
        let note = restore_session_note(adapter, state.host.as_ref(), &backup).await;
        if running {
            let _ = state.host.launch_client(&exe, &[]);
        }
        let msg = format!("写入新会话失败，{note}: {e}");
        log_switch_failure(
            &state.db,
            &app,
            &summary,
            UnifiedErrorCode::InternalError,
            &msg,
        );
        return Err(msg);
    }

    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "launching".into(),
        },
    );

    if let Err(e) = state.host.launch_client(&exe, &[]) {
        let note = restore_session_note(adapter, state.host.as_ref(), &backup).await;
        let msg = format!("拉起客户端失败，{note}（客户端当前处于关闭状态，请手动启动）: {e}");
        log_switch_failure(
            &state.db,
            &app,
            &summary,
            UnifiedErrorCode::InternalError,
            &msg,
        );
        return Err(msg);
    }

    // 启动存活轮询：700ms 间隔、总超时 45s、连续两次探测存活才确认，
    // 给慢启动客户端（大量扩展/大工作区/冷启动）更充裕的时间，避免误判回滚
    let mut alive = false;
    for _ in 0..64 {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        if check_client_running_async(&state.host, &summary.tool_id)
            .await
            .unwrap_or(false)
        {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if check_client_running_async(&state.host, &summary.tool_id)
                .await
                .unwrap_or(false)
            {
                alive = true;
                break;
            }
        }
    }
    if !alive {
        // 迟到进程二次确认：超时后才出现的进程视为启动成功，
        // 不回滚正在读取新会话的客户端
        alive = check_client_running_async(&state.host, &summary.tool_id)
            .await
            .unwrap_or(false);
    }
    if !alive {
        let note = restore_session_note(adapter, state.host.as_ref(), &backup).await;
        // 失败后尽力把原客户端重新拉起，避免用户被留在“客户端已关闭”状态
        let relaunch_note = match state.host.launch_client(&exe, &[]) {
            Ok(()) => "，已尝试重新拉起客户端".to_string(),
            Err(e) => format!("，重新拉起客户端失败（{e}）"),
        };
        let msg = format!(
            "客户端启动后 45 秒内未确认存活，{note}{relaunch_note}；若仍无法启动请手动检查或重启系统"
        );
        log_switch_failure(
            &state.db,
            &app,
            &summary,
            UnifiedErrorCode::InternalError,
            &msg,
        );
        return Err(msg);
    }

    let mut set_active_err = state
        .db
        .set_active_on_client(&summary.tool_id, &summary.id)
        .err();
    if set_active_err.is_some() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        set_active_err = state
            .db
            .set_active_on_client(&summary.tool_id, &summary.id)
            .err();
    }
    if let Some(e) = set_active_err {
        let msg = format!(
            "客户端已完成切号并重启，但本地活跃状态落盘失败（{e}）；无需重复切号，请刷新界面后重试"
        );
        log_switch_failure(
            &state.db,
            &app,
            &summary,
            UnifiedErrorCode::InternalError,
            &msg,
        );
        return Err(msg);
    }

    let _ = state.db.append_action_log(
        Some(&summary.id),
        &summary.tool_id,
        "switch",
        "success",
        None,
        &format!(
            "已将本机客户端切换至 {} ({})",
            summary.label, summary.uid_masked
        ),
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
    let app_for_tray = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
        flag_clone.store(false, Ordering::SeqCst);
        if let Some(tray) = app_for_tray.get_webview_window("tray") {
            if tray.is_visible().unwrap_or(false) && !tray.is_focused().unwrap_or(false) {
                let _ = tray.hide();
            }
        }
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
        &format!(
            "已将账号 ({}) 备注重命名为 {}",
            summary.uid_masked, summary.label
        ),
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(summary)
}

fn dedup_account_ids(ids: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::with_capacity(ids.len());
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// Trae CN 按设备限制每日签到次数；只在 Trae CN 账号内部把当前客户端账号排在前面，
/// 并保持其他工具账号在原队列位置，避免混合批次被意外重排。
fn prioritize_active_trae_account_on_client(db: &QwitchDatabase, ids: Vec<String>) -> Vec<String> {
    let mut result = ids.clone();
    let mut trae_positions = Vec::new();
    let mut active_trae = Vec::new();
    let mut remaining_trae = Vec::new();

    for (index, id) in ids.into_iter().enumerate() {
        if let Ok(summary) = db.get_account_summary(&id) {
            if summary.tool_id == "trae_cn" {
                trae_positions.push(index);
                if summary.is_active_on_client {
                    active_trae.push(id);
                } else {
                    remaining_trae.push(id);
                }
            }
        }
    }

    active_trae.extend(remaining_trae);
    for (index, id) in trae_positions.into_iter().zip(active_trae) {
        result[index] = id;
    }
    result
}

#[tauri::command]
pub async fn refresh_quota_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    account_ids: Vec<String>,
) -> Result<(), String> {
    let account_ids = dedup_account_ids(account_ids);
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
            .run_quota_refresh_batch(&app, &db, &master_key, &adapters, account_ids, jitter)
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
    let account_ids = dedup_account_ids(account_ids);
    if account_ids.len() > 200 {
        return Err("单次批量最多 200 个账号".into());
    }
    // Trae CN 设备级每日签到仅一次：当前客户端活跃账号优先领取。
    let account_ids = prioritize_active_trae_account_on_client(state.db.as_ref(), account_ids);
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
            .run_daily_claim_batch(&app, &db, &master_key, &adapters, account_ids, jitter)
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
            limit.unwrap_or(100).clamp(1, 500),
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

    let data_dir = state.host.qwitch_data_dir().map_err(|e| e.to_string())?;
    let archive_dir = data_dir.join("archives");
    std::fs::create_dir_all(&archive_dir).map_err(|e| e.to_string())?;

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f");
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600));
    }

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

  function isTraeCaptureOrigin() {
    const h = (window.location.hostname || '').toLowerCase();
    return h === 'trae.cn' || h.endsWith('.trae.cn');
  }

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
    if (captured || !isTraeCaptureOrigin()) return;
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
      window.location.href = 'https://qwitch.local/oauth-callback#nonce=' + encodeURIComponent('__QWITCH_NONCE__') + '&payload=' + encodeURIComponent(payloadStr);
    }
  }

  function isIgnoredUrl(url) {
    return /qrcode|qr_code|get_qr|check_qr|qrconnect|csrf|captcha|config|anon|guest|metrics|telemetry|report|tea\/v|slardar|passport\/web\/get/i.test(url);
  }

  function isTrustedAuthUrl(url) {
    return /GetUserToken|iCubeAuthInfo|\/v2\/billing\/meter\/|\/sash\/api\/v1\/me\//i.test(url);
  }

  function inspectResponseText(url, text) {
    if (captured || !text || !isTraeCaptureOrigin() || isIgnoredUrl(url)) return;
    const trimmed = text.trim();
    if (trimmed[0] !== '{' && trimmed[0] !== '[') return;
    try {
      const json = JSON.parse(trimmed);
      tryEmit(json, isTrustedAuthUrl(url));
    } catch (_) {}
  }

  if (isTraeCaptureOrigin()) {
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
  }

  function scanStorage() {
    if (captured || !isTraeCaptureOrigin()) return;
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

  if (isTraeCaptureOrigin()) {
    setInterval(scanStorage, 1000);
  }
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

#[cfg(target_os = "windows")]
const OAUTH_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
#[cfg(not(target_os = "windows"))]
const OAUTH_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn allowed_oauth_hosts(tool_id: &str) -> &'static [&'static str] {
    match tool_id {
        "trae_cn" => &[
            "trae.cn",
            "douyin.com",
            "bytedance.com",
            "snssdk.com",
            "volcengine.com",
            "bytegoofy.com",
            "doubao.com",
            "feishu.cn",
            "larkoffice.com",
            "github.com",
            "apple.com",
            "microsoftonline.com",
            "okta.com",
        ],
        "workbuddy" => &[
            "copilot.tencent.com",
            "workbuddy.cn",
            "tencent.com",
            "qcloud.com",
            "qq.com",
            "weixin.qq.com",
            "wx.qq.com",
            "work.weixin.qq.com",
            "wechat.com",
            "microsoftonline.com",
            "okta.com",
        ],
        "qoder_cn" => &[
            "qoder.cn",
            "qoder.com.cn",
            "aliyun.com",
            "alibabacloud.com",
            "alipay.com",
            "dingtalk.com",
            "taobao.com",
            "alibaba.com",
            "github.com",
            "google.com",
            "microsoftonline.com",
            "okta.com",
        ],
        "minimax_code_cn" => &[
            "minimax.cn",
            "minimaxi.com",
            "xaminim.com",
            // 登录页资源与滑块验证码 iframe（腾讯 TCaptcha 及其 CDN）
            "hailuoai.com",
            "hailuo.ai",
            "qcloud.com",
            "gtimg.com",
            "tencentcs.com",
            "tencentrio.com",
            "qq.com",
            "github.com",
            "google.com",
            "microsoftonline.com",
            "apple.com",
            "weixin.qq.com",
        ],
        _ => &[],
    }
}

fn is_allowed_oauth_url(url: &tauri::Url, allowed_hosts: &[&str]) -> bool {
    if url.as_str() == "about:blank" {
        return true;
    }
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str().map(|h| h.to_ascii_lowercase()) else {
        return false;
    };
    allowed_hosts
        .iter()
        .any(|base| host == *base || host.ends_with(&format!(".{base}")))
}

#[allow(clippy::too_many_arguments)]
async fn persist_oauth_envelope(
    app: &AppHandle,
    db: &Arc<QwitchDatabase>,
    host: &Arc<dyn PlatformHost>,
    master_key: &[u8; 32],
    adapter: &Arc<dyn ToolAdapter>,
    scheduler: &Arc<AntiRiskScheduler>,
    flow_seq: &Arc<AtomicU64>,
    flow_id: u64,
    win_label: &str,
    tool_id: &str,
    label: &str,
    target_account_id: Option<&str>,
    envelope: crate::domain::CredentialEnvelope,
) {
    let close_window = || {
        if flow_seq.load(Ordering::SeqCst) == flow_id {
            if let Some(win) = app.get_webview_window(win_label) {
                let _ = win.close();
            }
        }
    };

    if flow_seq.load(Ordering::SeqCst) != flow_id {
        return;
    }

    if envelope.tool_id != tool_id {
        let _ = app.emit(
            "qwitch://oauth-error",
            format!(
                "授权返回的工具类型不匹配：期望 {tool_id}，实际 {}",
                envelope.tool_id
            ),
        );
        close_window();
        return;
    }

    if !is_valid_oauth_uid(&envelope.uid) {
        let _ = app.emit(
            "qwitch://oauth-error",
            "授权返回的账号 UID 无效（疑似非字符串身份字段），已拒绝导入",
        );
        close_window();
        return;
    }

    let mut matched_local = false;
    let envelope_to_save = match adapter.import_from_local(host.as_ref()).await {
        Ok(local_env) if !local_env.uid.trim().is_empty() && local_env.uid == envelope.uid => {
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
            close_window();

            if summary.has_api_token && adapter.manifest().capabilities.supports_quota_query {
                let app_bg = app.clone();
                let db_bg = Arc::clone(db);
                let adapter_bg = Arc::clone(adapter);
                let scheduler_bg = Arc::clone(scheduler);
                tauri::async_runtime::spawn(async move {
                    let _guard = scheduler_bg.queue_lock().lock_owned().await;
                    record_import_quota_check(&db_bg, &adapter_bg, &summary, &cred_for_quota).await;
                    let _ = app_bg.emit("qwitch://account-updated", &summary.id);
                    let _ = app_bg.emit("qwitch://logs-updated", ());
                });
            }
        }
        Err(e) => {
            let _ = app.emit("qwitch://oauth-error", format!("保存账号失败: {e}"));
            close_window();
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
    let win_label = format!("oauth-login-{flow_id}");

    for (existing_label, existing_win) in app.webview_windows() {
        if existing_label == "oauth-login" || existing_label.starts_with("oauth-login-") {
            let _ = existing_win.close();
        }
    }

    enum OAuthMode {
        TraeCapture,
        WorkBuddyPoll {
            state_token: String,
        },
        QoderPkcePoll {
            nonce: String,
            verifier: String,
            machine_id: String,
        },
        MiniMaxDevicePoll {
            auth: crate::adapter::minimax::MiniMaxDeviceAuth,
        },
    }

    let (login_url, mode) = match tool_id.as_str() {
        "trae_cn" => (
            "https://www.trae.cn/login".to_string(),
            OAuthMode::TraeCapture,
        ),
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
        "minimax_code_cn" => {
            let auth = MiniMaxAdapter::create_device_authorization()
                .await
                .map_err(|e| format!("{e}"))?;
            let auth_url = auth.login_url().to_string();
            (auth_url, OAuthMode::MiniMaxDevicePoll { auth })
        }
        _ => return Err(format!("工具 {tool_id} 暂不支持内置 WebView 授权")),
    };

    let allowed_hosts = allowed_oauth_hosts(&tool_id);
    let parsed_url: tauri::Url = login_url
        .parse()
        .map_err(|e| format!("URL 解析失败: {e}"))?;
    if !is_allowed_oauth_url(&parsed_url, allowed_hosts) {
        return Err(format!("授权入口地址不在官方 HTTPS 白名单内: {parsed_url}"));
    }

    let adapter = state
        .adapters
        .get(&tool_id)
        .cloned()
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;

    let db = Arc::clone(&state.db);
    let host_arc = Arc::clone(&state.host);
    let scheduler_arc = Arc::clone(&state.scheduler);
    let master_key = state.master_key;
    let flow_seq = Arc::clone(&state.oauth_flow_seq);
    let app_handle = app.clone();
    let tool_id_owned = tool_id.clone();
    let label_owned = label.trim().to_string();
    let target_id_owned = target_account_id.filter(|s| !s.trim().is_empty());

    let app_for_popup = app.clone();
    let win_label_popup = win_label.clone();
    let init_js: String = match mode {
        OAuthMode::TraeCapture => OAUTH_CAPTURE_JS.replace("__QWITCH_NONCE__", &flow_nonce),
        OAuthMode::WorkBuddyPoll { .. } | OAuthMode::QoderPkcePoll { .. } => {
            POPUP_REDIRECT_JS.to_string()
        }
        // MiniMax 官方登录页依赖自身 window.open / 验证码 iframe，注入任何脚本都可能干扰
        OAuthMode::MiniMaxDevicePoll { .. } => String::new(),
    };

    let mut builder = WebviewWindowBuilder::new(&app, &win_label, WebviewUrl::External(parsed_url))
        .title(format!("Qwitch 内置授权登录 - {}", adapter.manifest().name))
        .inner_size(960.0, 720.0)
        .center()
        .incognito(true)
        .user_agent(OAUTH_USER_AGENT)
        .on_new_window(move |url, _features| {
            if url.as_str() != "about:blank" && is_allowed_oauth_url(&url, allowed_hosts) {
                if let Some(win) = app_for_popup.get_webview_window(&win_label_popup) {
                    let _ = win.navigate(url);
                }
            }
            tauri::webview::NewWindowResponse::Deny
        });

    if !init_js.is_empty() {
        builder = builder.initialization_script(&init_js);
    }

    if matches!(mode, OAuthMode::TraeCapture) {
        let app_nav = app_handle.clone();
        let db_nav = Arc::clone(&db);
        let scheduler_nav = Arc::clone(&scheduler_arc);
        let host_nav = Arc::clone(&host_arc);
        let adapter_nav = Arc::clone(&adapter);
        let flow_seq_nav = Arc::clone(&flow_seq);
        let expected_nonce = flow_nonce.clone();
        let tid_nav = tool_id_owned.clone();
        let lbl_nav = label_owned.clone();
        let target_nav = target_id_owned.clone();
        let win_label_nav = win_label.clone();

        builder = builder.on_navigation(move |nav_url| {
            if nav_url.host_str() == Some("qwitch.local") {
                if nav_url.scheme() != "https"
                    || nav_url.path() != "/oauth-callback"
                    || flow_seq_nav.load(Ordering::SeqCst) != flow_id
                {
                    return false;
                }
                let fragment_url = nav_url
                    .fragment()
                    .filter(|f| !f.is_empty())
                    .and_then(|frag| {
                        reqwest::Url::parse(&format!("https://qwitch.local/?{frag}")).ok()
                    });
                let find_param = |key: &str| -> Option<String> {
                    if let Some(ref fu) = fragment_url {
                        if let Some((_, v)) = fu.query_pairs().find(|(k, _)| k == key) {
                            return Some(v.to_string());
                        }
                    }
                    nav_url
                        .query_pairs()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| v.to_string())
                };

                let callback_nonce = find_param("nonce");
                if callback_nonce.as_deref() != Some(expected_nonce.as_str()) {
                    let _ = app_nav.emit(
                        "qwitch://oauth-error",
                        "内置授权回调校验失败（会话状态不匹配），已拒绝该请求",
                    );
                    return false;
                }

                let raw_payload = find_param("payload").or_else(|| find_param("token"));

                if let Some(payload_str) = raw_payload {
                    let app_clone = app_nav.clone();
                    let db_clone = Arc::clone(&db_nav);
                    let scheduler_clone = Arc::clone(&scheduler_nav);
                    let host_clone = Arc::clone(&host_nav);
                    let adapter_clone = Arc::clone(&adapter_nav);
                    let flow_seq_clone = Arc::clone(&flow_seq_nav);
                    let tid = tid_nav.clone();
                    let lbl = lbl_nav.clone();
                    let target_id = target_nav.clone();
                    let win_l = win_label_nav.clone();

                    tauri::async_runtime::spawn(async move {
                        match adapter_clone.parse_manual_token(&payload_str).await {
                            Ok(envelope) => {
                                persist_oauth_envelope(
                                    &app_clone,
                                    &db_clone,
                                    &host_clone,
                                    &master_key,
                                    &adapter_clone,
                                    &scheduler_clone,
                                    &flow_seq_clone,
                                    flow_id,
                                    &win_l,
                                    &tid,
                                    &lbl,
                                    target_id.as_deref(),
                                    envelope,
                                )
                                .await;
                            }
                            Err(e) => {
                                let _ = app_clone
                                    .emit("qwitch://oauth-error", format!("解析授权凭据失败: {e}"));
                                if flow_seq_clone.load(Ordering::SeqCst) == flow_id {
                                    if let Some(win) = app_clone.get_webview_window(&win_l) {
                                        let _ = win.close();
                                    }
                                }
                            }
                        }
                    });
                }
                return false;
            }
            is_allowed_oauth_url(nav_url, allowed_hosts)
        });
    } else {
        builder =
            builder.on_navigation(move |nav_url| is_allowed_oauth_url(nav_url, allowed_hosts));
    }

    builder
        .build()
        .map_err(|e| format!("打开内置授权窗口失败: {e}"))?;

    match mode {
        OAuthMode::TraeCapture => {}
        OAuthMode::WorkBuddyPoll { state_token } => {
            tauri::async_runtime::spawn(async move {
                let mut consecutive_errors = 0u32;
                let mut finished = false;
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if flow_seq.load(Ordering::SeqCst) != flow_id
                        || app_handle.get_webview_window(&win_label).is_none()
                    {
                        finished = true;
                        break;
                    }
                    match WorkBuddyAdapter::poll_oauth_result(&state_token).await {
                        Ok(Some(envelope)) => {
                            finished = true;
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &scheduler_arc,
                                &flow_seq,
                                flow_id,
                                &win_label,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                envelope,
                            )
                            .await;
                            break;
                        }
                        Ok(None) => {
                            consecutive_errors = 0;
                        }
                        Err(err) => {
                            let fatal = err.contains("无法唯一确认账号身份")
                                || err.contains("未获取到账号 UID");
                            consecutive_errors += 1;
                            if fatal || consecutive_errors >= 5 {
                                finished = true;
                                if flow_seq.load(Ordering::SeqCst) == flow_id {
                                    let _ = app_handle.emit("qwitch://oauth-error", err);
                                    if let Some(win) = app_handle.get_webview_window(&win_label) {
                                        let _ = win.close();
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
                if !finished && flow_seq.load(Ordering::SeqCst) == flow_id {
                    if let Some(win) = app_handle.get_webview_window(&win_label) {
                        let _ = app_handle.emit(
                            "qwitch://oauth-error",
                            "WorkBuddy 内置授权登录超时（4.5 分钟未完成），请重试",
                        );
                        let _ = win.close();
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
                let mut consecutive_errors = 0u32;
                let mut finished = false;
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if flow_seq.load(Ordering::SeqCst) != flow_id
                        || app_handle.get_webview_window(&win_label).is_none()
                    {
                        finished = true;
                        break;
                    }
                    match QoderAdapter::poll_pkce_device_token(&nonce, &verifier, &machine_id).await
                    {
                        Ok(Some(envelope)) => {
                            finished = true;
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &scheduler_arc,
                                &flow_seq,
                                flow_id,
                                &win_label,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                envelope,
                            )
                            .await;
                            break;
                        }
                        Ok(None) => {
                            consecutive_errors = 0;
                        }
                        Err(err) => {
                            consecutive_errors += 1;
                            if consecutive_errors >= 5 {
                                finished = true;
                                if flow_seq.load(Ordering::SeqCst) == flow_id {
                                    let _ = app_handle.emit("qwitch://oauth-error", err);
                                    if let Some(win) = app_handle.get_webview_window(&win_label) {
                                        let _ = win.close();
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
                if !finished && flow_seq.load(Ordering::SeqCst) == flow_id {
                    if let Some(win) = app_handle.get_webview_window(&win_label) {
                        let _ = app_handle.emit(
                            "qwitch://oauth-error",
                            "Qoder CN 内置授权登录超时（4.5 分钟未完成），请重试",
                        );
                        let _ = win.close();
                    }
                }
            });
        }
        OAuthMode::MiniMaxDevicePoll { auth } => {
            tauri::async_runtime::spawn(async move {
                loop {
                    if flow_seq.load(Ordering::SeqCst) != flow_id
                        || app_handle.get_webview_window(&win_label).is_none()
                    {
                        break;
                    }
                    if auth.is_expired() {
                        if flow_seq.load(Ordering::SeqCst) == flow_id {
                            let _ = app_handle.emit(
                                "qwitch://oauth-error",
                                "MiniMax 内置授权登录已超时，请重新发起授权",
                            );
                            if let Some(win) = app_handle.get_webview_window(&win_label) {
                                let _ = win.close();
                            }
                        }
                        break;
                    }
                    match MiniMaxAdapter::poll_device_authorization(&auth, host_arc.as_ref()).await
                    {
                        Ok(crate::adapter::minimax::DeviceAuthPoll::Pending) => {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                auth.poll_interval_ms(),
                            ))
                            .await;
                        }
                        Ok(crate::adapter::minimax::DeviceAuthPoll::SlowDown) => {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                auth.poll_interval_ms() + 5_000,
                            ))
                            .await;
                        }
                        Ok(crate::adapter::minimax::DeviceAuthPoll::Ready(envelope)) => {
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &scheduler_arc,
                                &flow_seq,
                                flow_id,
                                &win_label,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                *envelope,
                            )
                            .await;
                            break;
                        }
                        Err(err) => {
                            if flow_seq.load(Ordering::SeqCst) == flow_id {
                                let _ = app_handle.emit("qwitch://oauth-error", format!("{err}"));
                                if let Some(win) = app_handle.get_webview_window(&win_label) {
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

    #[test]
    fn switch_guard_rejects_concurrent_acquire_and_resets_on_drop() {
        let flag = Arc::new(AtomicBool::new(false));
        let first = SwitchFlagGuard::try_acquire(Arc::clone(&flag)).expect("first acquires");
        assert!(flag.load(Ordering::SeqCst));
        assert!(SwitchFlagGuard::try_acquire(Arc::clone(&flag)).is_err());
        drop(first);
        assert!(!flag.load(Ordering::SeqCst), "持有者结束后释放互斥");
        let _second =
            SwitchFlagGuard::try_acquire(Arc::clone(&flag)).expect("second acquires after release");
    }

    #[test]
    fn claim_batch_prioritizes_active_client_account() {
        let db_path = std::env::temp_dir().join(format!("qwitch_prio_{}.db", uuid::Uuid::new_v4()));
        let db = QwitchDatabase::open(&db_path).expect("open db");
        let master_key = [5u8; 32];
        let mut ids = Vec::new();
        for (uid, label) in [("u1", "one"), ("u2", "two"), ("u3", "three")] {
            let env = crate::domain::CredentialEnvelope {
                tool_id: "trae_cn".to_string(),
                uid: uid.to_string(),
                display_name: None,
                display_email: None,
                payload: crate::domain::CredentialPayload::Token {
                    access_token: "tok".to_string(),
                    refresh_token: None,
                    expires_at: None,
                    extra: Default::default(),
                },
            };
            ids.push(
                db.upsert_account(&master_key, Some(label), &env)
                    .expect("insert")
                    .id,
            );
        }
        db.set_active_on_client("trae_cn", &ids[1])
            .expect("set active");

        let ordered = prioritize_active_trae_account_on_client(
            &db,
            vec![ids[0].clone(), ids[1].clone(), ids[2].clone()],
        );
        assert_eq!(
            ordered,
            vec![ids[1].clone(), ids[0].clone(), ids[2].clone()]
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn oauth_url_whitelist_allows_official_sso_https_and_blocks_http_or_evil() {
        let trae = allowed_oauth_hosts("trae_cn");
        assert!(is_allowed_oauth_url(
            &"https://www.trae.cn/login".parse().unwrap(),
            trae
        ));
        assert!(is_allowed_oauth_url(
            &"https://open.douyin.com/platform/oauth/connect"
                .parse()
                .unwrap(),
            trae
        ));
        assert!(is_allowed_oauth_url(
            &"https://passport.feishu.cn/suite/passport/page/login"
                .parse()
                .unwrap(),
            trae
        ));
        assert!(!is_allowed_oauth_url(
            &"http://www.trae.cn/login".parse().unwrap(),
            trae
        ));
        assert!(!is_allowed_oauth_url(
            &"https://evil-trae.cn/login".parse().unwrap(),
            trae
        ));

        let wb = allowed_oauth_hosts("workbuddy");
        assert!(is_allowed_oauth_url(
            &"https://open.weixin.qq.com/connect/qrconnect"
                .parse()
                .unwrap(),
            wb
        ));
        assert!(!is_allowed_oauth_url(
            &"https://phish.example.com".parse().unwrap(),
            wb
        ));

        let qd = allowed_oauth_hosts("qoder_cn");
        assert!(is_allowed_oauth_url(
            &"https://account.aliyun.com/login/login.htm"
                .parse()
                .unwrap(),
            qd
        ));
        assert!(is_allowed_oauth_url(
            &"https://github.com/login".parse().unwrap(),
            qd
        ));
        assert!(!is_allowed_oauth_url(
            &"http://qoder.cn/login".parse().unwrap(),
            qd
        ));
        assert!(is_allowed_oauth_url(&"about:blank".parse().unwrap(), qd));
    }

    #[test]
    fn minimax_oauth_whitelist_covers_captcha_iframe_domains() {
        // on_navigation 会校验 iframe 导航：腾讯 TCaptcha 的 iframe 必须在白名单内，
        // 否则登录页会一直停在“获取验证码/验证组件加载失败”。
        let mm = allowed_oauth_hosts("minimax_code_cn");
        for url in [
            "https://account.minimax.cn/oauth-authorize?user_code=ABCD-EFGH",
            "https://account.minimax.cn/unified-login?login_redirect=%2Foauth-authorize",
            "https://turing.captcha.qcloud.com/TCaptcha.js",
            "https://ca.turing.captcha.qcloud.com/",
            "https://cloudcache.tencentcs.com/",
            "https://static.hailuoai.com/app.js",
        ] {
            assert!(
                is_allowed_oauth_url(&url.parse().unwrap(), mm),
                "应允许 {url}"
            );
        }
        assert!(!is_allowed_oauth_url(
            &"https://evil.example.com/steal".parse().unwrap(),
            mm
        ));
    }
}
