use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::adapter::AdapterError;
use crate::domain::{AccountStatus, AccountSummary, AuthMethod, QuotaSnapshot, UnifiedErrorCode};
use crate::scheduler::JitterConfig;
use crate::storage::QwitchDatabase;

use super::profile::resolve_import_host;
use super::{load_app_settings, schedule_import_quota_check, AppState};

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

/// 单账号增量刷新：供前端在 `qwitch://account-updated` 事件后定向更新单卡，
/// 避免每次事件都全量 `list_accounts` 拉取重渲染整棵账号树。
#[tauri::command]
pub fn get_account_summary(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<AccountSummary, String> {
    state
        .db
        .get_account_summary(&account_id)
        .map_err(|e| e.to_string())
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

    let is_external_profile = profile_dir
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());

    let base_host = Arc::clone(&state.host);
    let tool_for_resolve = tool_id.clone();
    let profile_for_resolve = profile_dir.clone();
    let (host, _temp_guard) = tokio::task::spawn_blocking(move || {
        resolve_import_host(
            &base_host,
            &tool_for_resolve,
            profile_for_resolve.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("解析导入目录任务异常: {e}"))??;

    let mut envelope = adapter
        .import_from_local(host.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    drop(_temp_guard);
    if envelope.tool_id != tool_id {
        return Err(format!(
            "适配器返回工具标识不匹配：期望 {tool_id}，实际 {}",
            envelope.tool_id
        ));
    }

    // 跨 OS 导入：仅外部设备 profile 允许按来源设备覆盖平台，避免误删本机导入的 device_brand / os_version
    if is_external_profile {
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
        &state.master_key,
        adapter,
        &summary,
        &cred_for_quota,
        &state.scheduler,
    )
    .await;

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

    let base_host = Arc::clone(&state.host);
    let tool_for_resolve = tool_id.clone();
    let profile_for_resolve = profile_dir.clone();
    let (host, _temp_guard) = tokio::task::spawn_blocking(move || {
        resolve_import_host(
            &base_host,
            &tool_for_resolve,
            profile_for_resolve.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("解析导入目录任务异常: {e}"))??;

    let envelope = adapter
        .import_from_local(host.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    drop(_temp_guard);
    let matched = state
        .db
        .find_account_id_by_uid(&state.master_key, &tool_id, &envelope.uid)
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
        display_name: envelope.display_name,
    })
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
        &state.master_key,
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

/// 采集「本机当前登录账号」的会话快照，绑定到指定账号上。
///
/// 用途：Qoder 的 `auth.v1.dat` 为 Chromium Safe Storage 加密，本项目不实现
/// 该加密链路，因此「在线授权得到的 Token 账号」无法自动获得可写入的快照。
/// 本命令让用户在 Qoder 客户端里手动登录目标账号后，采集当时的原始字节存为
/// 该账号的 LocalSnapshot —— `apply_session` 只做原样写回，无需解密即可切号。
///
/// 安全：采集前校验本机当前登录 uid 与目标账号一致，避免把 A 的快照绑到 B 上。
#[tauri::command]
pub async fn capture_local_session(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<(), String> {
    let (summary, cred) = state
        .db
        .get_account_credential(&state.master_key, &account_id)
        .map_err(|e| e.to_string())?;

    if summary.tool_id != "qoder_cn" {
        return Err(format!("{} 暂不支持采集本机会话快照", summary.tool_id));
    }

    // 采集需要读取客户端文件，放到阻塞线程池
    let host_for_capture = Arc::clone(&state.host);
    let target_uid = cred.uid.clone();
    let snapshot = tokio::task::spawn_blocking(move || {
        let adapter = crate::adapter::qoder::QoderAdapter::with_host(Arc::clone(&host_for_capture));
        adapter.capture_session_snapshot(host_for_capture.as_ref(), &target_uid)
    })
    .await
    .map_err(|e| format!("采集任务失败: {e}"))?
    .map_err(|e| e.to_string())?;

    // 合并进既有凭据：保留 Token（查额/签到用），补上 LocalSnapshot（切号用）
    let merged = cred.merge_from(&snapshot);
    state
        .db
        .bind_account_credential(&state.master_key, &summary.id, None, &merged)
        .map_err(|e| e.to_string())?;

    let _ = state.db.append_action_log(
        Some(&summary.id),
        &summary.tool_id,
        "import",
        "success",
        None,
        &format!(
            "已采集 {} ({}) 的本机会话快照",
            summary.label, summary.uid_masked
        ),
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    let _ = app.emit("qwitch://logs-updated", ());
    Ok(())
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

    // 与领取一致：拿到锁后立即广播启动，避免前端二次点击穿透
    let _ = app.emit(
        "qwitch://batch-progress",
        crate::scheduler::BatchProgressEvent {
            action: "refresh".into(),
            running: true,
            total: account_ids.len(),
            completed: 0,
            current_label: None,
            phase: "checking".into(),
            jitter_seconds: None,
            summary_text: Some("正在初始化批量队列…".into()),
        },
    );

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

    let total = account_ids.len();
    // 拿到锁后立刻广播「已启动」，再 spawn 后台执行。
    //
    // 否则存在竞态窗口：后端 `spawn` 后立即返回 `Ok(())`，而首个进度事件要等
    // 后台任务真正开始才发出。前端在此期间 `batchProgress.running` 仍为 false，
    // 用户二次点击会穿透双守卫、被 `try_lock_owned` 拒绝，看到「已有批量任务正在进行」
    // 的红色错误提示。先发事件可让前端立即进入运行态。
    let _ = app.emit(
        "qwitch://batch-progress",
        crate::scheduler::BatchProgressEvent {
            action: "claim".into(),
            running: true,
            total,
            completed: 0,
            current_label: None,
            phase: "checking".into(),
            jitter_seconds: None,
            summary_text: Some("正在初始化批量队列…".into()),
        },
    );

    tokio::spawn(async move {
        let _guard = guard;
        let _ = scheduler
            .run_daily_claim_batch(&app, &db, &master_key, &adapters, account_ids, jitter)
            .await;
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
