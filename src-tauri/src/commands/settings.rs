use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL, Engine as _};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::adapter::{AdapterManifest, ToolAdapter};
use crate::domain::{
    AccountStatus, AppSettingsDto, AuthMethod, CredentialEnvelope, CredentialPayload,
};
use crate::platform::PlatformHost;
use crate::storage::QwitchDatabase;

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
    _app: AppHandle,
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
    let (plugin_dir, found) = plugins
        .into_iter()
        .find(|(_, m)| m.id == id)
        .ok_or_else(|| format!("未找到外部插件 {id}"))?;
    let content_hash = crate::adapter::external::adapter_content_hash(&plugin_dir)?;
    let hash_preview = content_hash.chars().take(12).collect::<String>();
    let entry_summary = if found.entry.args.is_empty() {
        found.entry.command.clone()
    } else {
        format!("{} {}", found.entry.command, found.entry.args.join(" "))
    };

    let already_trusted = load_app_settings(&state.db)
        .trusted_adapters
        .iter()
        .any(|t| t == &id);
    let store = Arc::clone(&state.plugin_trust_tickets);
    let id_for_dialog = id.clone();
    let display = format!("{} ({})", found.name, found.version);

    // 原生模态对话框必须在主线程执行，且是阻塞调用 → 放到阻塞线程池
    let confirmed = tokio::task::spawn_blocking(move || {
        let title = if already_trusted {
            "确认重新信任已变更的外部插件"
        } else {
            "确认信任外部插件"
        };
        let detail = if already_trusted {
            format!(
                "插件：{display}\nID：{id_for_dialog}\n入口：{entry_summary}\n内容校验：{hash_preview}…\n\n\
                 检测到该插件的内容哈希已发生变更（或尚未固定校验哈希）。\
                 重新信任后，系统将固定新的内容哈希，并在随 Qwitch 启动时允许其读取本机客户端凭据与当前用户目录下的文件。\n\n\
                 仅在你确认该变更是安全升级或来源可信时继续。"
            )
        } else {
            format!(
                "插件：{display}\nID：{id_for_dialog}\n入口：{entry_summary}\n内容校验：{hash_preview}…\n\n\
                 信任后，该插件将随 Qwitch 启动并被授予访问本机客户端凭据的能力，\
                 可读取当前用户目录下的文件。\n\n\
                 仅在你确认该插件来源可信时继续。"
            )
        };
        let result = rfd::MessageDialog::new()
            .set_title(title)
            .set_description(detail)
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        result == rfd::MessageDialogResult::Yes
    })
    .await
    .map_err(|e| format!("信任确认对话框执行失败: {e}"))?;

    if !confirmed {
        return Err("已取消：未授予该插件的信任".into());
    }

    Ok(super::issue_plugin_trust_ticket(&store, &id, &content_hash))
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

/// 校验并固定外部插件信任列表及内容哈希；返回需要新建/重签并经原生确认票据的插件 ID 及最终规范化后的结果。
fn resolve_trusted_adapters_and_hashes(
    previous: &AppSettingsDto,
    requested_trusted: &[String],
    requested_hashes: &HashMap<String, String>,
    disk_hashes: &HashMap<String, String>,
    tickets: Option<&HashMap<String, String>>,
    ticket_store: &std::sync::Mutex<super::PluginTrustTicketMap>,
) -> Result<(Vec<String>, HashMap<String, String>), String> {
    let mut normalized_trusted = Vec::new();
    let mut seen_trusted = std::collections::HashSet::new();
    for raw_id in requested_trusted {
        let id = raw_id.trim();
        if id.is_empty() {
            continue;
        }
        if crate::adapter::RESERVED_ADAPTER_IDS.contains(&id)
            || !crate::adapter::is_valid_adapter_id(id)
        {
            return Err(format!("非法或保留的外部插件 ID，无法加入信任列表：{id}"));
        }
        if seen_trusted.insert(id.to_string()) {
            normalized_trusted.push(id.to_string());
        }
    }

    let mut resolved_hashes = HashMap::new();
    let mut plugins_needing_ticket = Vec::new();

    for id in &normalized_trusted {
        let prev_trusted = previous.trusted_adapters.iter().any(|t| t == id);
        let requested_hash = requested_hashes
            .get(id)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let prev_hash = previous
            .trusted_adapter_hashes
            .get(id)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        // 若此前已信任且哈希未发生任何变化（未传新哈希或与历史哈希一致），
        // 视为无关设置保存（如切换主题），保持历史哈希且不打断用户。
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
                _ => {}
            }
        }

        let disk_hash = disk_hashes
            .get(id)
            .ok_or_else(|| format!("未找到外部插件 {id}，无法固定信任哈希"))?
            .clone();
        if let Some(req) = requested_hash {
            if !req.eq_ignore_ascii_case(&disk_hash) {
                return Err(format!(
                    "外部插件 {id} 的内容校验哈希与磁盘不一致，可能已被修改，请刷新后重试"
                ));
            }
        }

        // 新增信任，或已信任插件内容发生变更/原哈希缺失需要重签：均强制校验原生确认票据。
        let needs_ticket = !prev_trusted
            || prev_hash
                .as_deref()
                .is_none_or(|ph| !ph.eq_ignore_ascii_case(&disk_hash));
        if needs_ticket {
            plugins_needing_ticket.push(id.clone());
        }
        resolved_hashes.insert(id.clone(), disk_hash);
    }

    // 先检查全部所需票据都存在且合法（通过预先校验后再一次性消费），避免中途报错留下半消费状态。
    if !plugins_needing_ticket.is_empty() {
        let tickets_map = tickets.ok_or_else(|| {
            format!(
                "插件 {} 缺少原生信任确认：请在设置页重新点击启用并在系统弹窗中确认",
                plugins_needing_ticket[0]
            )
        })?;
        for id in &plugins_needing_ticket {
            if !tickets_map.contains_key(id) {
                return Err(format!(
                    "插件 {id} 缺少原生信任确认：请在设置页重新点击启用并在系统弹窗中确认"
                ));
            }
        }
        for id in &plugins_needing_ticket {
            let ticket = tickets_map.get(id).expect("checked above");
            let expected_hash = resolved_hashes.get(id).map(String::as_str).unwrap_or("");
            super::consume_plugin_trust_ticket(ticket_store, id, ticket, expected_hash)?;
        }
    }

    Ok((normalized_trusted, resolved_hashes))
}

#[tauri::command]
pub fn save_app_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: AppSettingsDto,
    // 每个「本次新增信任」或「重签变更内容哈希」的插件都必须携带原生确认签发的一次性票据。
    // 未提供时视为空表，等价于「无票据」，由下方逐项校验拒绝。
    trusted_adapter_tickets: Option<HashMap<String, String>>,
) -> Result<AppSettingsDto, String> {
    // 防风控抖动下限：不允许 0ms 连发
    settings.jitter_min_ms = settings.jitter_min_ms.max(500);
    settings.jitter_max_ms = settings.jitter_max_ms.max(settings.jitter_min_ms);
    let previous = load_app_settings(&state.db);
    // 端口只能经由 update_gateway_port_cmd 变更；普通设置保存不得覆盖已持久化端口
    settings.gateway_port = previous.gateway_port.max(1024);
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

    // 外部插件信任与内容哈希固定：规范化 trusted_adapters，并在首次信任或内容变更重签时强制票据校验
    let mut disk_hashes = HashMap::new();
    let has_non_empty_trusted = settings
        .trusted_adapters
        .iter()
        .any(|id| !id.trim().is_empty());
    if has_non_empty_trusted {
        let data_dir = state.host.qwitch_data_dir().map_err(|e| e.to_string())?;
        for (dir, m) in crate::adapter::external::scan_external_manifests(&data_dir) {
            if let Ok(hash) = crate::adapter::external::adapter_content_hash(&dir) {
                disk_hashes.insert(m.id, hash);
            }
        }
    }
    let (normalized_trusted, resolved_hashes) = resolve_trusted_adapters_and_hashes(
        &previous,
        &settings.trusted_adapters,
        &settings.trusted_adapter_hashes,
        &disk_hashes,
        trusted_adapter_tickets.as_ref(),
        &state.plugin_trust_tickets,
    )?;
    settings.trusted_adapters = normalized_trusted;
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
    if state.switching_in_progress.load(Ordering::SeqCst) {
        return Ok(());
    }
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

    let (detected, logged_out): (Vec<(String, String)>, Vec<String>) =
        tokio::task::spawn_blocking(move || {
            let mut detected = Vec::new();
            let mut logged_out = Vec::new();
            for (tool_id, adapter) in candidates {
                match adapter.detect_local_active_uid(host.as_ref()) {
                    Some(uid) => detected.push((tool_id, uid)),
                    // 客户端明确登出：必须在下一轮把「本机使用中」清掉，
                    // 否则卡片会停在既不能关联本机、也不能切号的死胡同
                    None if adapter.detect_local_logged_out(host.as_ref()) == Some(true) => {
                        logged_out.push(tool_id)
                    }
                    None => {}
                }
            }
            (detected, logged_out)
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
                if let Some(adapter) = state.adapters.get(&tool_id) {
                    let lock = state.gateway.refresh_lock_for(&id);
                    let _guard = lock.lock().await;
                    if sync_active_local_credential_if_fresher(
                        &state.db,
                        &state.master_key,
                        &state.host,
                        adapter,
                        &id,
                    )
                    .await
                    {
                        let _ = app.emit("qwitch://account-updated", &id);
                        let _ = app.emit("qwitch://logs-updated", ());
                    }
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

    // 客户端明确登出的工具：清掉遗留的「本机使用中」，让「关联本机 / 切至桌面端」重新出现
    for tool_id in logged_out {
        if let Ok(cleared) = state.db.clear_active_on_client(&tool_id) {
            if cleared > 0 {
                let _ = state.db.append_action_log(
                    None,
                    &tool_id,
                    "reconcile",
                    "success",
                    None,
                    &format!("{tool_id} 客户端已登出，已清除 {cleared} 个账号的「本机使用中」标记"),
                );
                let _ = app.emit("qwitch://account-updated", &tool_id);
            }
        }
    }
    Ok(())
}

fn decode_jwt_exp_ms(token: &str) -> Option<i64> {
    let payload_b64 = token.split('.').nth(1)?;
    let bytes = BASE64_URL.decode(payload_b64.as_bytes()).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let exp = v.get("exp").and_then(|x| x.as_i64())?;
    Some(if exp < 10_000_000_000 {
        exp.saturating_mul(1000)
    } else {
        exp
    })
}

fn credential_expires_at_ms(cred: &CredentialEnvelope) -> Option<i64> {
    match &cred.payload {
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
            .or_else(|| decode_jwt_exp_ms(access_token)),
        CredentialPayload::LocalSnapshot { files } => {
            use base64::engine::general_purpose::STANDARD as BASE64;
            let raw = files.get("decrypted_token.txt")?;
            let bytes = BASE64.decode(raw).ok()?;
            let tok = std::str::from_utf8(&bytes).ok()?;
            decode_jwt_exp_ms(tok)
        }
        CredentialPayload::Cookies { .. } => None,
    }
}

fn access_token_hint(cred: &CredentialEnvelope) -> Option<String> {
    match &cred.payload {
        CredentialPayload::Token { access_token, .. } => {
            (!access_token.trim().is_empty()).then(|| access_token.clone())
        }
        CredentialPayload::LocalSnapshot { files } => files.get("decrypted_token.txt").cloned(),
        CredentialPayload::Cookies { .. } => None,
    }
}

/// 当本机活跃客户端里的凭据比 Qwitch 库中记录更新（或账号已标过期）时，
/// 静默将最新凭据回吸写入加密库，避免 CodeArts / 桌面客户端自刷新后 Qwitch 仍误报过期。
pub(crate) async fn sync_active_local_credential_if_fresher(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    host: &Arc<dyn PlatformHost>,
    adapter: &Arc<dyn ToolAdapter>,
    account_id: &str,
) -> bool {
    let Ok((summary, stored_cred)) = db.get_account_credential(master_key, account_id) else {
        return false;
    };
    if summary.status == AccountStatus::Disabled {
        return false;
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let stored_exp = credential_expires_at_ms(&stored_cred);
    let near_expiry = stored_exp.is_some_and(|exp| exp - now_ms <= 65 * 60 * 1000);
    if summary.status != AccountStatus::Expired && !near_expiry {
        return false;
    }

    let Ok(incoming) = adapter.import_from_local(host.as_ref()).await else {
        return false;
    };
    let stored_real = !stored_cred.uid.trim().is_empty()
        && !CredentialEnvelope::is_fallback_uid(&stored_cred.uid);
    let incoming_real =
        !incoming.uid.trim().is_empty() && !CredentialEnvelope::is_fallback_uid(&incoming.uid);
    if stored_real && incoming_real && stored_cred.uid != incoming.uid {
        return false;
    }

    let incoming_exp = credential_expires_at_ms(&incoming);
    let fresher_exp = match (stored_exp, incoming_exp) {
        (Some(old_e), Some(new_e)) => new_e > old_e + 60_000,
        (None, Some(_)) => true,
        _ => false,
    };
    let token_changed = match (
        access_token_hint(&stored_cred),
        access_token_hint(&incoming),
    ) {
        (Some(old_t), Some(new_t)) => old_t != new_t,
        (None, Some(_)) => true,
        _ => false,
    };
    let still_valid_for_expired = summary.status == AccountStatus::Expired
        && incoming_exp.is_some_and(|e| e > now_ms + 60_000);
    if !fresher_exp && !token_changed && !still_valid_for_expired {
        return false;
    }

    if db
        .bind_account_credential(master_key, account_id, None, &incoming)
        .is_ok()
    {
        let _ = db.append_action_log(
            Some(account_id),
            &summary.tool_id,
            "keepalive",
            "success",
            None,
            &format!("已从本机活跃客户端同步 {} 的最新凭据", summary.label),
        );
        return true;
    }
    false
}

#[cfg(test)]
mod plugin_trust_ticket_tests {
    use super::super::{
        consume_plugin_trust_ticket, issue_plugin_trust_ticket, PluginTrustTicketMap,
        PLUGIN_TRUST_TICKET_TTL_MS,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn ticket_is_single_use_and_bound_to_adapter_id_and_hash() {
        let store: Mutex<PluginTrustTicketMap> = Mutex::new(HashMap::new());
        let ticket = issue_plugin_trust_ticket(&store, "my_plugin", "hash_v1");

        // 错配 adapter id 无效
        assert!(consume_plugin_trust_ticket(&store, "other_plugin", &ticket, "hash_v1").is_err());
        // 错配票据无效
        assert!(consume_plugin_trust_ticket(&store, "my_plugin", "bogus", "hash_v1").is_err());
        // 正确票据首次消费成功
        consume_plugin_trust_ticket(&store, "my_plugin", &ticket, "hash_v1")
            .expect("first consume");
        // 二次消费失败（一次性）
        assert!(
            consume_plugin_trust_ticket(&store, "my_plugin", &ticket, "hash_v1").is_err(),
            "票据必须一次性，重放应被拒绝"
        );

        // 审批后内容哈希变更必须拒绝并销毁原票据
        let ticket2 = issue_plugin_trust_ticket(&store, "my_plugin", "hash_v1");
        assert!(
            consume_plugin_trust_ticket(&store, "my_plugin", &ticket2, "hash_tampered").is_err()
        );
        assert!(consume_plugin_trust_ticket(&store, "my_plugin", &ticket2, "hash_v1").is_err());
    }

    #[test]
    fn reissuing_invalidates_previous_ticket() {
        let store: Mutex<PluginTrustTicketMap> = Mutex::new(HashMap::new());
        let first = issue_plugin_trust_ticket(&store, "p1", "h1");
        let second = issue_plugin_trust_ticket(&store, "p1", "h1");
        assert_ne!(first, second);
        assert!(consume_plugin_trust_ticket(&store, "p1", &first, "h1").is_err());
        consume_plugin_trust_ticket(&store, "p1", &second, "h1").expect("latest ticket valid");
    }

    #[test]
    fn expired_ticket_is_rejected() {
        let store: Mutex<PluginTrustTicketMap> = Mutex::new(HashMap::new());
        let ticket = issue_plugin_trust_ticket(&store, "p2", "h2");
        // 手动把签发时间回拨到超过 TTL
        {
            let mut map = store.lock().unwrap();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            map.insert(
                "p2".to_string(),
                (
                    ticket.clone(),
                    "h2".to_string(),
                    now - PLUGIN_TRUST_TICKET_TTL_MS - 1_000,
                ),
            );
        }
        assert!(consume_plugin_trust_ticket(&store, "p2", &ticket, "h2").is_err());
    }

    #[test]
    fn missing_ticket_is_rejected() {
        let store: Mutex<PluginTrustTicketMap> = Mutex::new(HashMap::new());
        assert!(consume_plugin_trust_ticket(&store, "never_issued", "x", "h").is_err());
    }

    #[test]
    fn re_signing_changed_plugin_requires_fresh_ticket() {
        use super::resolve_trusted_adapters_and_hashes;
        use crate::domain::AppSettingsDto;

        let store: Mutex<PluginTrustTicketMap> = Mutex::new(HashMap::new());
        let previous = AppSettingsDto {
            trusted_adapters: vec!["ext_demo".to_string()],
            trusted_adapter_hashes: HashMap::from([(
                "ext_demo".to_string(),
                "old_hash".to_string(),
            )]),
            ..Default::default()
        };
        let disk_hashes = HashMap::from([("ext_demo".to_string(), "new_hash".to_string())]);

        // 无关设置保存（未传新哈希或传回旧哈希）：保留旧哈希，无需票据
        let (trusted_keep, hashes_keep) = resolve_trusted_adapters_and_hashes(
            &previous,
            &["ext_demo".to_string()],
            &HashMap::new(),
            &disk_hashes,
            None,
            &store,
        )
        .expect("unrelated save should keep pinned hash without ticket");
        assert_eq!(trusted_keep, vec!["ext_demo".to_string()]);
        assert_eq!(
            hashes_keep.get("ext_demo").map(String::as_str),
            Some("old_hash")
        );

        // 插件内容被修改，试图静默重签为 new_hash 且不带票据：必须拒绝
        let req_new = HashMap::from([("ext_demo".to_string(), "new_hash".to_string())]);
        let err = resolve_trusted_adapters_and_hashes(
            &previous,
            &["ext_demo".to_string()],
            &req_new,
            &disk_hashes,
            None,
            &store,
        )
        .expect_err("resigning modified plugin without ticket must be rejected");
        assert!(err.contains("缺少原生信任确认"));

        // 签发并携带原生确认票据后允许重签为 new_hash，且票据一次性消费
        let ticket = issue_plugin_trust_ticket(&store, "ext_demo", "new_hash");
        let tickets = HashMap::from([("ext_demo".to_string(), ticket)]);
        let (_, hashes_resigned) = resolve_trusted_adapters_and_hashes(
            &previous,
            &["ext_demo".to_string()],
            &req_new,
            &disk_hashes,
            Some(&tickets),
            &store,
        )
        .expect("resigning with valid ticket should succeed");
        assert_eq!(
            hashes_resigned.get("ext_demo").map(String::as_str),
            Some("new_hash")
        );
        // 重放同一票据失败
        assert!(resolve_trusted_adapters_and_hashes(
            &previous,
            &["ext_demo".to_string()],
            &req_new,
            &disk_hashes,
            Some(&tickets),
            &store,
        )
        .is_err());
    }
}
