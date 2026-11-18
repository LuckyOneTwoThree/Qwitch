pub mod adapter;
pub mod commands;
pub mod domain;
pub mod platform;
pub mod scheduler;
pub mod storage;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, PhysicalPosition, PhysicalSize, Position, WindowEvent,
};

use adapter::create_all_adapters;
use commands::AppState;
use platform::{NativePlatformHost, PlatformHost};
use scheduler::AntiRiskScheduler;
use storage::QwitchDatabase;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let host: Arc<dyn PlatformHost> = Arc::new(NativePlatformHost);
            let data_dir = host
                .qwitch_data_dir()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            let master_key = host
                .get_or_create_master_key()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            let db = Arc::new(
                QwitchDatabase::open(&data_dir.join("qwitch.db"))
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?,
            );
            if let Ok(Some(json)) = db.get_setting("app.settings") {
                if let Ok(settings) = serde_json::from_str::<crate::domain::AppSettingsDto>(&json) {
                    let _ = db.apply_log_retention(
                        settings.log_archive_after_days,
                        settings.log_max_active_count,
                    );
                }
            } else {
                let defaults = crate::domain::AppSettingsDto::default();
                let _ = db.apply_log_retention(
                    defaults.log_archive_after_days,
                    defaults.log_max_active_count,
                );
            }

            let state = AppState {
                db,
                master_key,
                host,
                adapters: Arc::new(create_all_adapters(&data_dir)),
                scheduler: Arc::new(AntiRiskScheduler::new()),
                oauth_flow_seq: Arc::new(AtomicU64::new(0)),
                switching_in_progress: Arc::new(AtomicBool::new(false)),
            };
            app.manage(state);

            let show_main =
                MenuItem::with_id(app, "show_main", "打开 Qwitch 主面板", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出 Qwitch", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_main, &quit])?;

            let tray_icon = tauri::include_image!("icons/tray-template.png");

            TrayIconBuilder::with_id("qwitch-tray")
                .icon(tray_icon)
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Qwitch - AI 多账号与额度管理")
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("tray") {
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
                                let size = win.outer_size().unwrap_or(PhysicalSize::new(340, 420));
                                let x = (position.x - (size.width as f64) / 2.0).max(8.0) as i32;
                                #[cfg(target_os = "macos")]
                                let y = (position.y + 8.0).max(0.0) as i32;
                                #[cfg(not(target_os = "macos"))]
                                let y = (position.y - (size.height as f64) - 8.0).max(0.0) as i32;
                                let _ = win.set_position(Position::Physical(PhysicalPosition::new(x, y)));
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show_main" => {
                        if let Some(tray) = app.get_webview_window("tray") {
                            let _ = tray.hide();
                        }
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.unminimize();
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| match (window.label(), event) {
            ("main", WindowEvent::CloseRequested { api, .. }) => {
                api.prevent_close();
                let _ = window.hide();
            }
            ("tray", WindowEvent::Focused(false)) => {
                let switching = window
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|s| s.switching_in_progress.load(Ordering::SeqCst))
                    .unwrap_or(false);
                if !switching {
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::show_main_window,
            commands::quit_app,
            commands::get_app_settings,
            commands::save_app_settings,
            commands::list_adapters,
            commands::detect_client_status,
            commands::list_accounts,
            commands::import_local_account,
            commands::add_account_by_token,
            commands::start_webview_oauth,
            commands::refresh_account_quota,
            commands::refresh_quota_batch,
            commands::switch_desktop_account,
            commands::delete_account,
            commands::update_account_status,
            commands::rename_account,
            commands::claim_daily_batch,
            commands::get_action_logs,
            commands::get_action_log_stats,
            commands::archive_action_logs,
            commands::unarchive_action_logs,
            commands::delete_action_logs,
            commands::export_action_logs
        ])
        .run(tauri::generate_context!())
        .expect("error while running Qwitch application");
}
