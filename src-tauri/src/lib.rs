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
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
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
            db.verify_or_init_key_fingerprint(&data_dir, &master_key)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            let startup_settings = commands::load_app_settings(&db);
            let _ = db.apply_log_retention(
                startup_settings.log_archive_after_days,
                startup_settings.log_max_active_count,
            );
            commands::apply_runtime_app_policy(app.handle(), &startup_settings);
            if startup_settings.launch_at_login {
                let _ = host.set_launch_at_login(true);
            }
            let launched_by_autostart = std::env::args().any(|a| a == "--autostart");
            if startup_settings.start_minimized || launched_by_autostart {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.hide();
                }
            }

            let state = AppState {
                db,
                master_key,
                host,
                adapters: Arc::new(create_all_adapters(&data_dir, &startup_settings.trusted_adapters)),
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
                        rect,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("tray") {
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
                                let scale = win.scale_factor().unwrap_or(1.0);
                                let (icon_x, icon_y) = match rect.position {
                                    Position::Physical(p) => (p.x as f64, p.y as f64),
                                    Position::Logical(p) => (p.x * scale, p.y * scale),
                                };
                                let (icon_w, icon_h) = match rect.size {
                                    tauri::Size::Physical(s) => (s.width as f64, s.height as f64),
                                    tauri::Size::Logical(s) => (s.width * scale, s.height * scale),
                                };
                                let size = win.outer_size().unwrap_or(PhysicalSize::new(
                                    (384.0 * scale) as u32,
                                    (500.0 * scale) as u32,
                                ));
                                let center_x = if icon_w > 0.0 {
                                    icon_x + icon_w / 2.0
                                } else {
                                    position.x
                                };
                                let raw_x = center_x - (size.width as f64) / 2.0;
                                let x = if let Some(mon) = win
                                    .current_monitor()
                                    .ok()
                                    .flatten()
                                    .or_else(|| app.primary_monitor().ok().flatten())
                                {
                                    let mon_x = mon.position().x as f64;
                                    let mon_w = mon.size().width as f64;
                                    let margin = 4.0 * scale;
                                    let max_x =
                                        (mon_x + mon_w - (size.width as f64) - margin).max(mon_x + margin);
                                    raw_x.clamp(mon_x + margin, max_x) as i32
                                } else {
                                    raw_x.max(8.0) as i32
                                };

                                #[cfg(target_os = "macos")]
                                let y = if icon_h > 0.0 {
                                    (icon_y + icon_h + 2.0 * scale).max(0.0) as i32
                                } else {
                                    (position.y + 18.0 * scale).max(0.0) as i32
                                };
                                #[cfg(not(target_os = "macos"))]
                                let y = if icon_h > 0.0 {
                                    (icon_y - (size.height as f64) - 2.0 * scale).max(0.0) as i32
                                } else {
                                    (position.y - (size.height as f64) - 8.0 * scale).max(0.0)
                                        as i32
                                };

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
                let close_to_tray = window
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|s| commands::load_app_settings(&s.db).close_to_tray)
                    .unwrap_or(true);
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    window.app_handle().exit(0);
                }
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
            commands::list_external_adapters,
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
        .build(tauri::generate_context!())
        .expect("error while building Qwitch application")
        .run(|app_handle, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    if let Some(win) = app_handle.get_webview_window("main") {
                        let _ = win.unminimize();
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        });
}
