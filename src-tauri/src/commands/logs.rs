use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::domain::{
    ActionLogCleanupRequest, ActionLogItem, ActionLogStats, ExportLogsResult, LogScope,
};

use super::AppState;

#[tauri::command]
pub fn get_action_logs(
    state: State<'_, AppState>,
    scope: Option<LogScope>,
    limit: Option<u32>,
    offset: Option<u32>,
    before_id: Option<i64>,
) -> Result<Vec<ActionLogItem>, String> {
    state
        .db
        .list_action_logs(
            scope.unwrap_or(LogScope::Active),
            limit.unwrap_or(100).clamp(1, 2_000),
            offset.unwrap_or(0),
            before_id,
        )
        .map_err(|e| e.to_string())
}

/// 日志统计：对 `action_logs` 做无 WHERE 的 `SUM(CASE...)` 全表扫描，
/// 随日志量线性增长。改为异步命令 + `spawn_blocking`，避免占用主线程。
#[tauri::command]
pub async fn get_action_log_stats(state: State<'_, AppState>) -> Result<ActionLogStats, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || db.get_action_log_stats())
        .await
        .map_err(|e| format!("日志统计任务失败: {e}"))?
        .map_err(|e| e.to_string())
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

/// 导出日志：读取 + 序列化 + 写盘 + 可选归档全部在 `spawn_blocking` 中执行，
/// 避免占用异步工作线程导致界面卡顿。
#[tauri::command]
pub async fn export_action_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    scope: Option<LogScope>,
    archive_after_export: Option<bool>,
) -> Result<ExportLogsResult, String> {
    let target_scope = scope.unwrap_or(LogScope::Archived);
    let archive_flag = archive_after_export.unwrap_or(false) && target_scope == LogScope::Active;
    let db = Arc::clone(&state.db);
    let host = Arc::clone(&state.host);

    let result = tokio::task::spawn_blocking(move || -> Result<ExportLogsResult, String> {
        let data_dir = host.qwitch_data_dir().map_err(|e| e.to_string())?;
        let logs = db
            .list_action_logs(target_scope, 10_000, 0, None)
            .map_err(|e| e.to_string())?;
        if logs.is_empty() {
            return Err("当前视图没有可导出的操作日志".into());
        }

        let archive_dir = data_dir.join("archives");
        std::fs::create_dir_all(&archive_dir).map_err(|e| e.to_string())?;

        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f");
        let scope_tag = match target_scope {
            LogScope::Active => "active",
            LogScope::Archived => "archived",
            LogScope::All => "all",
        };
        let file_path = archive_dir.join(format!("action-logs-{scope_tag}-{stamp}.json"));
        let count = logs.len();
        let ids: Vec<i64> = if archive_flag {
            logs.iter().map(|item| item.id).collect()
        } else {
            Vec::new()
        };
        let payload = serde_json::json!({
            "app": "Qwitch",
            "exportedAt": chrono::Utc::now().to_rfc3339(),
            "scope": scope_tag,
            "count": count,
            "logs": logs,
        });
        let pretty = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
        crate::adapter::write_private_file_atomic(&file_path, pretty.as_bytes())
            .map_err(|e| e.to_string())?;

        let archived_count = if archive_flag && !ids.is_empty() {
            db.archive_action_logs(&ActionLogCleanupRequest {
                scope: Some(LogScope::Active),
                ids: Some(ids),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?
        } else {
            0
        };

        Ok(ExportLogsResult {
            file_path: file_path.to_string_lossy().to_string(),
            count,
            archived_count,
        })
    })
    .await
    .map_err(|e| format!("导出日志任务失败: {e}"))??;

    if result.archived_count > 0 {
        let _ = app.emit("qwitch://logs-updated", ());
    }

    Ok(result)
}
