use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::adapter::{AdapterManifest, ToolAdapter};
use crate::domain::{AppSettingsDto, AuthMethod};

use super::{load_app_settings, resolve_client_executable, AppState, SETTINGS_KEY};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientHostStatus {
    pub tool_id: String,
    pub executable_path: Option<String>,
    pub config_dir: Option<String>,
    pub is_running: bool,
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
    pub content_hash: String,
    pub hash_mismatch: bool,
}

/// 请求用户确认信任某个外部插件（原生对话框，不经渲染进程）。
///
/// 返回一次性票据，前端需在 `save_app_settings` 的
/// `trustedAdapterTickets` 中回传该票据，后端校验并消费后才允许写入
/// `trusted_adapters`。这样即使渲染进程被完全控制，也无法单方面完成
/// 「信任插件 → 加载插件 → 插件读取本机凭据」的链路。
#[tauri::command]
pub async fn request_plugin_trust(
    app: AppHandle,
    state: State<'_, AppState>,
    adapter_id: String,
) -> Result<String, String> {
    let id = adapter_id.trim().to_string();
    if !crate::adapter::is_valid_adapter_id(&id)
        || crate::adapter::RESERVED_ADAPTER_IDS.contains(&id.as_str())
    {
        return Err(format!("非法或保留的外部插件 ID：{id}"));
    }
    let data_dir = state.host.qwitch_data_dir().map_err(|e| e.to_string())?;
    let plugins = crate::adapter::external::scan_external_manifests(&data_dir);
    let found = plugins
        .iter()
        .find(|(_, m)| m.id == id)
        .map(|(_, m)| m.clone())
        .ok_or_else(|| format!("未找到外部插件 {id}"))?;

    let store = Arc::clone(&state.plugin_trust_tickets);
    let id_for_dialog = id.clone();
    let display = format!("{} ({})", found.name, found.version);

    // 原生模态对话框必须在主线程执行，且是阻塞调用 → 放到阻塞线程池
    let confirmed = tokio::task::spawn_blocking(move || {
        let result = rfd::MessageDialog::new()
            .set_title("确认信任外部插件")
            .set_description(format!(
                "插件：{display}\nID：{id_for_dialog}\n\n\
                 信任后，该插件将随 Qwitch 启动并被授予访问本机客户端凭据的能力，\
                 可读取当前用户目录下的文件。\n\n\
                 仅在你确认该插件来源可信时继续。"
            ))
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        result == rfd::MessageDialogResult::Yes
    })
    .await
    .map_err(|e| format!("信任确认对话框执行失败: {e}"))?;

    let _ = app.emit("qwitch://logs-updated", ());
    if !confirmed {
        return Err("已取消：未授予该插件的信任".into());
    }

    Ok(super::issue_plugin_trust_ticket(&store, &id))
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
            .map(|(dir, m)| {
                let reserved = crate::adapter::RESERVED_ADAPTER_IDS.contains(&m.id.as_str());
                let trusted = !reserved && settings.trusted_adapters.iter().any(|t| t == &m.id);
                let content_hash =
                    crate::adapter::external::adapter_content_hash(&dir).unwrap_or_default();
                let pinned_hash = settings
                    .trusted_adapter_hashes
                    .get(&m.id)
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty());
                let hash_mismatch = trusted
                    && (content_hash.is_empty()
                        || pinned_hash.is_none_or(|h| !h.eq_ignore_ascii_case(&content_hash)));
                ExternalAdapterInfo {
                    trusted,
                    reserved,
                    content_hash,
                    hash_mismatch,
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

#[tauri::command]
pub fn get_app_settings(state: State<'_, AppState>) -> Result<AppSettingsDto, String> {
    Ok(load_app_settings(&state.db))
}

#[tauri::command]
pub fn save_app_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: AppSettingsDto,
    // 每个「本次新增信任」的插件都必须携带原生确认签发的一次性票据。
    // 未提供时视为空表，等价于「无票据」，由下方逐项校验拒绝。
    trusted_adapter_tickets: Option<HashMap<String, String>>,
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
        // 与启动前的复核保持一致：此处提前拒绝「指向系统二进制」等危险路径，
        // 让用户在设置保存阶段就得到明确反馈，而不是等到切号/启动时才失败。
        crate::platform::validate_client_executable(tool, p)
            .map_err(|e| format!("工具 {tool} 的自定义客户端路径不可用：{e}"))?;
    }
    // 仅当“开机自启”发生变化时才重新配置，避免每次保存设置都触碰系统注册/或误报错
    if settings.launch_at_login != previous.launch_at_login {
        state.host.set_launch_at_login(settings.launch_at_login)?;
    }

    // 外部插件信任与内容哈希固定：规范化 trusted_adapters，并在首次信任或显式重签时固定磁盘 SHA-256
    let mut normalized_trusted = Vec::new();
    let mut seen_trusted = std::collections::HashSet::new();
    for raw_id in &settings.trusted_adapters {
        let id = raw_id.trim();
        if id.is_empty() {
            continue;
        }
        if crate::adapter::RESERVED_ADAPTER_IDS.contains(&id)
            || !crate::adapter::is_valid_adapter_id(id)
        {
            return Err(format!("非法或保留的外部插件 ID，无法加入信任列表：{id}"));
        }
        // 仅「本次新增信任」需要原生确认票据；取消信任或保持既有信任无需票据，
        // 避免用户每次保存无关设置都被弹窗打断。
        let already_trusted = previous.trusted_adapters.iter().any(|t| t == id);
        if !already_trusted {
            let tickets = trusted_adapter_tickets.as_ref();
            let ticket = tickets.and_then(|t| t.get(id)).ok_or_else(|| {
                format!("插件 {id} 缺少原生信任确认：请在设置页重新点击启用并在系统弹窗中确认")
            })?;
            super::consume_plugin_trust_ticket(&state.plugin_trust_tickets, id, ticket)?;
        }
        if seen_trusted.insert(id.to_string()) {
            normalized_trusted.push(id.to_string());
        }
    }
    settings.trusted_adapters = normalized_trusted;

    let mut resolved_hashes = HashMap::new();
    if !settings.trusted_adapters.is_empty() {
        let data_dir = state.host.qwitch_data_dir().map_err(|e| e.to_string())?;
        let disk_adapters: HashMap<String, PathBuf> =
            crate::adapter::external::scan_external_manifests(&data_dir)
                .into_iter()
                .map(|(dir, m)| (m.id, dir))
                .collect();

        for id in &settings.trusted_adapters {
            let requested_hash = settings
                .trusted_adapter_hashes
                .get(id)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let prev_trusted = previous.trusted_adapters.iter().any(|t| t == id);
            let prev_hash = previous
                .trusted_adapter_hashes
                .get(id)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            if prev_trusted {
                match (&prev_hash, &requested_hash) {
                    (Some(ph), None) => {
                        resolved_hashes.insert(id.clone(), ph.clone());
                        continue;
                    }
                    (Some(ph), Some(rh)) if rh.eq_ignore_ascii_case(ph) => {
                        resolved_hashes.insert(id.clone(), ph.clone());
                        continue;
                    }
                    (None, None) => {
                        continue;
                    }
                    _ => {}
                }
            }

            let dir = disk_adapters
                .get(id)
                .ok_or_else(|| format!("未找到外部插件 {id}，无法固定信任哈希"))?;
            let disk_hash = crate::adapter::external::adapter_content_hash(dir)?;
            if let Some(req) = requested_hash {
                if !req.eq_ignore_ascii_case(&disk_hash) {
                    return Err(format!(
                        "外部插件 {id} 的内容校验哈希与磁盘不一致，可能已被修改，请刷新后重试"
                    ));
                }
            }
            resolved_hashes.insert(id.clone(), disk_hash);
        }
    }
    settings.trusted_adapter_hashes = resolved_hashes;

    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    state
        .db
        .set_setting(SETTINGS_KEY, &json)
        .map_err(|e| e.to_string())?;
    crate::platform::sync_custom_client_paths(&settings.custom_client_paths);
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

/// 校正“本机使用中”标记：纯本地只读探测各工具客户端当前真实登录账号，
/// 与本库账号比对后更新 is_active_on_client（读不到真实 uid 的工具跳过，绝不误清；禁止联网或拉起外部解密进程）。
#[tauri::command]
pub async fn reconcile_active_accounts(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let prev_ms = state.last_reconcile_ms.load(Ordering::Relaxed);
    if now_ms.saturating_sub(prev_ms) < 3_000 {
        return Ok(());
    }
    if state
        .last_reconcile_ms
        .compare_exchange(prev_ms, now_ms, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        return Ok(());
    }

    let host = Arc::clone(&state.host);
    let candidates: Vec<(String, Arc<dyn ToolAdapter>)> = state
        .adapters
        .iter()
        .filter(|(_, adapter)| {
            adapter
                .manifest()
                .capabilities
                .auth_methods
                .contains(&AuthMethod::LocalImport)
        })
        .map(|(k, v)| (k.clone(), Arc::clone(v)))
        .collect();

    let detected: Vec<(String, String)> = tokio::task::spawn_blocking(move || {
        candidates
            .into_iter()
            .filter_map(|(tool_id, adapter)| {
                adapter
                    .detect_local_active_uid(host.as_ref())
                    .map(|uid| (tool_id, uid))
            })
            .collect()
    })
    .await
    .map_err(|e| format!("探测本机活跃账号失败: {e}"))?;

    for (tool_id, uid) in detected {
        let trimmed = uid.trim();
        if trimmed.is_empty() || crate::domain::CredentialEnvelope::is_fallback_uid(trimmed) {
            continue;
        }
        let current_active = state.db.get_active_account_id(&tool_id).ok().flatten();
        match state
            .db
            .find_account_id_by_uid(&state.master_key, &tool_id, trimmed)
        {
            Ok(Some((id, _))) => {
                if current_active.as_deref() != Some(id.as_str())
                    && state.db.set_active_on_client(&tool_id, &id).is_ok()
                {
                    let _ = app.emit("qwitch://account-updated", &id);
                }
            }
            Ok(None) => {
                if let Ok(cleared) = state.db.clear_active_on_client(&tool_id) {
                    if cleared > 0 {
                        let _ = app.emit("qwitch://account-updated", &tool_id);
                    }
                }
            }
            Err(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod plugin_trust_ticket_tests {
    use super::super::{
        consume_plugin_trust_ticket, issue_plugin_trust_ticket, PLUGIN_TRUST_TICKET_TTL_MS,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn ticket_is_single_use_and_bound_to_adapter_id() {
        let store: Mutex<HashMap<String, (String, u64)>> = Mutex::new(HashMap::new());
        let ticket = issue_plugin_trust_ticket(&store, "my_plugin");

        // 错配 adapter id 无效
        assert!(consume_plugin_trust_ticket(&store, "other_plugin", &ticket).is_err());
        // 错配票据无效
        assert!(consume_plugin_trust_ticket(&store, "my_plugin", "bogus").is_err());
        // 正确票据首次消费成功
        consume_plugin_trust_ticket(&store, "my_plugin", &ticket).expect("first consume");
        // 二次消费失败（一次性）
        assert!(
            consume_plugin_trust_ticket(&store, "my_plugin", &ticket).is_err(),
            "票据必须一次性，重放应被拒绝"
        );
    }

    #[test]
    fn reissuing_invalidates_previous_ticket() {
        let store: Mutex<HashMap<String, (String, u64)>> = Mutex::new(HashMap::new());
        let first = issue_plugin_trust_ticket(&store, "p1");
        let second = issue_plugin_trust_ticket(&store, "p1");
        assert_ne!(first, second);
        assert!(consume_plugin_trust_ticket(&store, "p1", &first).is_err());
        consume_plugin_trust_ticket(&store, "p1", &second).expect("latest ticket valid");
    }

    #[test]
    fn expired_ticket_is_rejected() {
        let store: Mutex<HashMap<String, (String, u64)>> = Mutex::new(HashMap::new());
        let ticket = issue_plugin_trust_ticket(&store, "p2");
        // 手动把签发时间回拨到超过 TTL
        {
            let mut map = store.lock().unwrap();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            map.insert(
                "p2".to_string(),
                (ticket.clone(), now - PLUGIN_TRUST_TICKET_TTL_MS - 1_000),
            );
        }
        assert!(consume_plugin_trust_ticket(&store, "p2", &ticket).is_err());
    }

    #[test]
    fn missing_ticket_is_rejected() {
        let store: Mutex<HashMap<String, (String, u64)>> = Mutex::new(HashMap::new());
        assert!(consume_plugin_trust_ticket(&store, "never_issued", "x").is_err());
    }
}
