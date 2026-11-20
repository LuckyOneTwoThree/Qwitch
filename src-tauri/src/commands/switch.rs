use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::adapter::ToolAdapter;
use crate::domain::{AccountSummary, UnifiedErrorCode};
use crate::platform::PlatformHost;
use crate::storage::QwitchDatabase;

use super::{load_app_settings, resolve_client_executable, AppState, SwitchProgressEvent};

struct SwitchFlagGuard {
    flag: Arc<AtomicBool>,
}

impl SwitchFlagGuard {
    fn try_acquire(flag: Arc<AtomicBool>) -> Result<Self, String> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "切号进行中，请等待当前切换完成后再试".to_string())?;
        Ok(Self { flag })
    }
}

impl Drop for SwitchFlagGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
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
    let _switch_guard = SwitchFlagGuard::try_acquire(Arc::clone(&state.switching_in_progress))?;
    let _queue_guard = state
        .scheduler
        .queue_lock()
        .try_lock_owned()
        .map_err(|_| "已有批量查额或签到任务正在进行，请等待完成后再试".to_string())?;

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

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    state
        .tray_blur_suppress_until_ms
        .store(now_ms.saturating_add(1800), Ordering::SeqCst);
    let app_for_tray = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
        if let Some(tray) = app_for_tray.get_webview_window("tray") {
            if tray.is_visible().unwrap_or(false) && !tray.is_focused().unwrap_or(false) {
                let _ = tray.hide();
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
