use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::adapter::ToolAdapter;
use crate::domain::{AccountSummary, UnifiedErrorCode};
use crate::platform::PlatformHost;
use crate::storage::QwitchDatabase;

use super::{load_app_settings, resolve_client_executable, AppState, SwitchProgressEvent};

/// 切号结果。
///
/// 必须区分「切号完成」与「客户端存活已确认」：两者不是同一件事。
/// 快照写入成功但客户端未确认存活时，界面要如实告知，而不是一律报成功。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchDesktopAccountResult {
    /// 目标账号的会话快照是否已成功写入
    pub switched: bool,
    /// 是否确认到客户端主进程存活
    pub liveness_confirmed: bool,
    /// 存活未确认等需要用户知晓的补充说明
    pub note: Option<String>,
}

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

/// 最弱存活判定：客户端是否有任意进程存在（不要求识别出主进程）。
async fn client_process_present_async(
    host: &Arc<dyn PlatformHost>,
    tool_id: &str,
) -> Result<bool, String> {
    let h = Arc::clone(host);
    let tid = tool_id.to_string();
    tokio::task::spawn_blocking(move || h.client_process_present(&tid))
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
) -> Result<SwitchDesktopAccountResult, String> {
    let _switch_guard = SwitchFlagGuard::try_acquire(Arc::clone(&state.switching_in_progress))?;
    let _queue_guard = state
        .scheduler
        .queue_lock()
        .try_lock_owned()
        .map_err(|_| "已有批量查额或签到任务正在进行，请等待完成后再试".to_string())?;

    perform_switch_core(
        &app,
        &state.db,
        &state.master_key,
        &state.host,
        &state.adapters,
        &state.tray_blur_suppress_until_ms,
        &account_id,
        force_kill,
    )
    .await
}

/// 切号核心实现（不含队列锁 / 切号哨兵获取，由调用方负责互斥）。
///
/// 单账号命令 `switch_desktop_account` 与批量冷切领取的 `ClientSwitcher` 共用本函数：
/// 都走「关客户端 → 备份会话 → 写新会话 → 拉起 → 存活确认 → 落盘活跃」的完整链路，
/// 保证行为一致，避免两套切号逻辑分叉。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn perform_switch_core(
    app: &AppHandle,
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    host: &Arc<dyn PlatformHost>,
    adapters: &HashMap<String, Arc<dyn ToolAdapter>>,
    tray_blur_suppress_until_ms: &AtomicU64,
    account_id: &str,
    force_kill: bool,
) -> Result<SwitchDesktopAccountResult, String> {
    let (summary, cred) = db
        .get_account_credential(master_key, account_id)
        .map_err(|e| e.to_string())?;

    let adapter = adapters
        .get(&summary.tool_id)
        .ok_or_else(|| format!("适配器 {} 尚未注册", summary.tool_id))?;
    if !adapter.manifest().capabilities.supports_client_switch {
        let msg = format!("适配器 {} 不支持桌面端切号", summary.tool_id);
        log_switch_failure(
            db,
            app,
            &summary,
            UnifiedErrorCode::UnsupportedCapability,
            &msg,
        );
        return Err(msg);
    }

    let settings = load_app_settings(db);
    let Some(exe) = resolve_client_executable(host.as_ref(), &settings, &summary.tool_id) else {
        let msg = format!(
            "未检测到 {} 客户端安装路径，请在设置中配置自定义路径后再切换",
            summary.tool_id
        );
        log_switch_failure(db, app, &summary, UnifiedErrorCode::ClientNotFound, &msg);
        return Err(msg);
    };

    let running = match check_client_running_async(host, &summary.tool_id).await {
        Ok(r) => r,
        Err(e) => {
            log_switch_failure(db, app, &summary, UnifiedErrorCode::InternalError, &e);
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
        if let Err(e) = host
            .terminate_client_gracefully(&summary.tool_id, 3000)
            .await
        {
            let msg = e.to_string();
            log_switch_failure(db, app, &summary, UnifiedErrorCode::ClientRunning, &msg);
            return Err(msg);
        }
    }

    // 终止后备份权威会话（避免读到运行中半写文件）；备份失败则重拉原客户端
    let backup = match adapter.backup_session(host.as_ref()).await {
        Ok(b) => b,
        Err(e) => {
            if running {
                let _ = host.launch_client(&exe, &[]);
            }
            let msg = format!(
                "备份客户端会话失败{}: {e}",
                if running {
                    "，已尝试重新拉起原客户端"
                } else {
                    "（客户端未做任何改动）"
                }
            );
            log_switch_failure(db, app, &summary, UnifiedErrorCode::InternalError, &msg);
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

    if let Err(e) = adapter.apply_session(host.as_ref(), &cred).await {
        let note = restore_session_note(adapter, host.as_ref(), &backup).await;
        if running {
            let _ = host.launch_client(&exe, &[]);
        }
        let msg = format!("写入新会话失败，{note}: {e}");
        log_switch_failure(db, app, &summary, UnifiedErrorCode::InternalError, &msg);
        return Err(msg);
    }

    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "launching".into(),
        },
    );

    if let Err(e) = host.launch_client(&exe, &[]) {
        let note = restore_session_note(adapter, host.as_ref(), &backup).await;
        let msg = format!("拉起客户端失败，{note}（客户端当前处于关闭状态，请手动启动）: {e}");
        log_switch_failure(db, app, &summary, UnifiedErrorCode::InternalError, &msg);
        return Err(msg);
    }

    // 启动存活轮询：300ms 间隔、总超时 12s、连续两次探测存活才确认。
    //
    // 45s 是历史遗留的保守值：实测 Electron 客户端冷启动通常 1~3s，
    // 45s 只会让「启动失败」变成一次 45 秒的假死等待（期间 UI 一直停在
    // 「重启桌面端」），用户感知就是「切号卡死」。12s 足够覆盖慢启动，
    // 且探测本身已修成不依赖 `ps`，误判概率很低。
    let mut alive = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if check_client_running_async(host, &summary.tool_id)
            .await
            .unwrap_or(false)
        {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            if check_client_running_async(host, &summary.tool_id)
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
        alive = check_client_running_async(host, &summary.tool_id)
            .await
            .unwrap_or(false);
    }
    // 兜底：主进程未能识别，但客户端进程确实存在（例如 Electron 打包客户端
    // 的主二进制名为 `Electron`，极端情况下识别不出主进程）。
    // 此时**不得**回滚会话快照 —— 回滚会与正在运行的客户端抢同一个会话文件，
    // 把「切换其实已生效」变成「切换被撤销」。
    let mut liveness_confirmed = alive;
    if !alive {
        alive = client_process_present_async(host, &summary.tool_id)
            .await
            .unwrap_or(false);
        liveness_confirmed = false;
    }
    if !alive {
        let note = restore_session_note(adapter, host.as_ref(), &backup).await;
        // 失败后尽力把原客户端重新拉起，避免用户被留在“客户端已关闭”状态
        let relaunch_note = match host.launch_client(&exe, &[]) {
            Ok(()) => "，已尝试重新拉起客户端".to_string(),
            Err(e) => format!("，重新拉起客户端失败（{e}）"),
        };
        let msg = format!(
            "客户端启动后 12 秒内未确认存活，{note}{relaunch_note}；若仍无法启动请手动检查或重启系统"
        );
        log_switch_failure(db, app, &summary, UnifiedErrorCode::InternalError, &msg);
        return Err(msg);
    }

    let mut set_active_err = db.set_active_on_client(&summary.tool_id, &summary.id).err();
    if set_active_err.is_some() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        set_active_err = db.set_active_on_client(&summary.tool_id, &summary.id).err();
    }
    if let Some(e) = set_active_err {
        let msg = format!(
            "客户端已完成切号并重启，但本地活跃状态落盘失败（{e}）；无需重复切号，请刷新界面后重试"
        );
        log_switch_failure(db, app, &summary, UnifiedErrorCode::InternalError, &msg);
        return Err(msg);
    }

    let note = if liveness_confirmed {
        None
    } else {
        Some(
            "已写入目标账号会话并拉起客户端，但未确认到客户端主进程存活；若客户端未正常打开请手动启动"
                .to_string(),
        )
    };

    let _ = db.append_action_log(
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
    tray_blur_suppress_until_ms.store(now_ms.saturating_add(1800), Ordering::SeqCst);
    let app_for_tray = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
        if let Some(tray) = app_for_tray.get_webview_window("tray") {
            if tray.is_visible().unwrap_or(false) && !tray.is_focused().unwrap_or(false) {
                let _ = tray.hide();
            }
        }
    });

    Ok(SwitchDesktopAccountResult {
        switched: true,
        liveness_confirmed,
        note,
    })
}

/// 基于 `AppState` 的真实冷切号实现，供批量冷切领取引擎（`ClientSwitcher`）调用。
///
/// 每次 `switch_to` 复用 `perform_switch_core` 的完整链路，并独占切号哨兵，
/// 与手动切号命令互斥，避免批量与手动同时改本机会话文件。
pub(crate) struct AppStateClientSwitcher {
    pub app: AppHandle,
    pub db: Arc<QwitchDatabase>,
    pub master_key: [u8; 32],
    pub host: Arc<dyn PlatformHost>,
    pub adapters: Arc<HashMap<String, Arc<dyn ToolAdapter>>>,
    pub switching_in_progress: Arc<AtomicBool>,
    pub tray_blur_suppress_until_ms: Arc<AtomicU64>,
    pub force_kill: bool,
}

#[async_trait::async_trait]
impl crate::scheduler::ClientSwitcher for AppStateClientSwitcher {
    async fn switch_to(&self, account_id: &str) -> Result<(), String> {
        let _guard = SwitchFlagGuard::try_acquire(Arc::clone(&self.switching_in_progress))?;
        perform_switch_core(
            &self.app,
            &self.db,
            &self.master_key,
            &self.host,
            &self.adapters,
            &self.tray_blur_suppress_until_ms,
            account_id,
            self.force_kill,
        )
        .await
        .map(|_| ())
    }

    fn current_active_account_id(&self, tool_id: &str) -> Option<String> {
        self.db.get_active_account_id(tool_id).ok().flatten()
    }
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
