pub mod adapter;
pub mod commands;
pub mod domain;
pub mod platform;
pub mod scheduler;
pub mod storage;
pub mod update;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, PhysicalPosition, Position, WindowEvent,
};

use adapter::create_all_adapters;
use commands::AppState;
use platform::{NativePlatformHost, PlatformHost};
use scheduler::AntiRiskScheduler;
use storage::QwitchDatabase;

/// `tauri dev` 运行的是裸可执行文件，没有 .app bundle 图标，
/// 运行时把 Qwitch 官方图标写入 NSApplication，避免程序坞显示通用 exec 图标。
#[cfg(target_os = "macos")]
fn apply_macos_app_icon() {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(include_bytes!("../icons/icon.png"));
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    unsafe {
        NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            commands::present_main_window(app);
        }))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            apply_macos_app_icon();

            let host: Arc<dyn PlatformHost> = Arc::new(NativePlatformHost);
            let data_dir = host
                .qwitch_data_dir()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            // 先开库（SQLite 本身不加密），取得指纹/多条密文样本后再选主密钥，
            // 确保单条密文损坏或孤儿 key_id 场景下不会误判或误删正确密钥
            let db = Arc::new(
                QwitchDatabase::open(&data_dir.join("qwitch.db"))
                    .map_err(|e| std::io::Error::other(e.to_string()))?,
            );
            let sample_blobs = db
                .credential_sample_blobs(5)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let expectation = platform::KeyExpectation {
                fingerprint: std::fs::read_to_string(data_dir.join("key_id"))
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                has_encrypted_data: !sample_blobs.is_empty(),
                sample_blobs,
            };
            let master_key = host
                .get_or_create_master_key(&expectation)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            db.verify_or_init_key_fingerprint(&data_dir, &master_key)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            // 安全迁移：失败不再静默吞掉。
            //
            // 这三项迁移对应「凭据指纹加盐」「密文 AAD 绑定」「历史 PII 脱敏」三条
            // 安全升级。此前失败仅 eprintln 后照常启动，用户完全无感知，安全升级
            // 静默未生效。现在收集失败项，启动完成后以原生弹窗明确告知（不阻断
            // 启动，避免把可用性问题放大为不可用），并在日志中留下可追溯记录。
            let mut migration_warnings: Vec<String> = Vec::new();
            if let Err(e) = db.migrate_uid_hash(&master_key) {
                eprintln!("Qwitch uid_hash 加密指纹迁移失败: {e}");
                migration_warnings.push(format!("凭据指纹加盐升级未完成：{e}"));
            }
            if let Err(e) = db.migrate_credential_aad(&master_key) {
                eprintln!("Qwitch 凭据 AAD 绑定迁移失败: {e}");
                migration_warnings.push(format!("凭据 AAD 绑定升级未完成：{e}"));
            }
            // 已升级但仍有行停留在旧格式（AAD 可被绕过）时同样告警
            let legacy_aad_ids = db.legacy_aad_account_ids();
            if !legacy_aad_ids.is_empty() {
                eprintln!(
                    "Qwitch 有 {} 个账号的凭据仍为无 AAD 的旧格式（跨账号密文替换防护未生效）",
                    legacy_aad_ids.len()
                );
                migration_warnings.push(format!(
                    "有 {} 个账号的凭据仍为旧格式（未绑定 AAD），跨账号密文替换防护未生效；\
                     建议重新导入这些账号以完成升级",
                    legacy_aad_ids.len()
                ));
            }
            if let Err(e) = db.migrate_legacy_pii(&master_key) {
                eprintln!("Qwitch 历史 PII 迁移失败: {e}");
                migration_warnings.push(format!("历史数据脱敏未完成：{e}"));
            }
            let startup_settings = commands::load_app_settings(&db);
            platform::sync_custom_client_paths(&startup_settings.custom_client_paths);
            let _ = db.apply_log_retention(
                startup_settings.log_archive_after_days,
                startup_settings.log_max_active_count,
            );
            if startup_settings.launch_at_login {
                let _ = host.set_launch_at_login(true);
            }
            // 安全迁移未完成时，在窗口就绪后以原生弹窗明确告知用户。
            // 放在后台线程：对话框是阻塞调用，不得阻塞 setup 主线程。
            if !migration_warnings.is_empty() {
                let detail = migration_warnings.join("\n\n");
                std::thread::spawn(move || {
                    rfd::MessageDialog::new()
                        .set_title("Qwitch 安全升级未完全生效")
                        .set_description(format!(
                            "以下安全升级未能完成，应用仍可正常使用，但对应防护处于降级状态：\n\n{detail}"
                        ))
                        .set_level(rfd::MessageLevel::Warning)
                        .show();
                });
            }

            let launched_by_autostart = std::env::args().any(|a| a == "--autostart");
            if !startup_settings.start_minimized && !launched_by_autostart {
                commands::present_main_window(app.handle());
            } else {
                commands::set_macos_dock_visible(app.handle(), false);
            }

            let adapters = Arc::new(create_all_adapters(
                &data_dir,
                &startup_settings.trusted_adapters,
                &startup_settings.trusted_adapter_hashes,
                Arc::clone(&host),
            ));
            let state = AppState {
                db,
                master_key,
                host,
                adapters,
                scheduler: Arc::new(AntiRiskScheduler::new()),
                oauth_flow_seq: Arc::new(AtomicU64::new(0)),
                consumed_oauth_nonces: Arc::new(std::sync::Mutex::new(
                    std::collections::HashSet::new(),
                )),
                switching_in_progress: Arc::new(AtomicBool::new(false)),
                last_reconcile_ms: Arc::new(AtomicU64::new(0)),
                tray_blur_suppress_until_ms: Arc::new(AtomicU64::new(0)),
                plugin_trust_tickets: Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
            };
            app.manage(state);

            // WAL 周期治理：每 5 分钟一次 PASSIVE checkpoint（不阻塞读者）。
            // 与 PRAGMA wal_autocheckpoint 互补——自动 checkpoint 在长读事务下可能
            // 长期无法推进，定时兜底可防止 -wal 无限增长。
            {
                // db 已在 AppState 构造时被移动，这里从已管理的 state 取回
                let db_bg = match app.try_state::<AppState>() {
                    Some(st) => Arc::clone(&st.db),
                    None => return Err("AppState 未就绪".into()),
                };
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                        let db_iter = Arc::clone(&db_bg);
                        let _ = tokio::task::spawn_blocking(move || {
                            db_iter.checkpoint_wal_passive()
                        })
                        .await;
                    }
                });
            }

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
                                let monitors = win.available_monitors().unwrap_or_default();
                                let point_in_mon = |m: &tauri::Monitor, px: f64, py: f64| -> bool {
                                    let mx = m.position().x as f64;
                                    let my = m.position().y as f64;
                                    let mw = m.size().width as f64;
                                    let mh = m.size().height as f64;
                                    px >= mx && px < mx + mw && py >= my && py < my + mh
                                };
                                let target_mon = monitors
                                    .iter()
                                    .find(|m| {
                                        (position.x != 0.0 || position.y != 0.0)
                                            && point_in_mon(m, position.x, position.y)
                                    })
                                    .cloned()
                                    .or_else(|| {
                                        monitors
                                            .iter()
                                            .find(|m| {
                                                let s = m.scale_factor();
                                                let (rx, ry) = match rect.position {
                                                    Position::Physical(p) => {
                                                        (p.x as f64, p.y as f64)
                                                    }
                                                    Position::Logical(p) => (p.x * s, p.y * s),
                                                };
                                                point_in_mon(m, rx, ry)
                                            })
                                            .cloned()
                                    })
                                    .or_else(|| win.current_monitor().ok().flatten())
                                    .or_else(|| app.primary_monitor().ok().flatten());

                                let scale = target_mon
                                    .as_ref()
                                    .map(|m| m.scale_factor())
                                    .or_else(|| win.scale_factor().ok())
                                    .unwrap_or(1.0);
                                let old_scale = win.scale_factor().unwrap_or(scale).max(0.5);
                                let (win_w, win_h) = match win.outer_size() {
                                    Ok(s) => (
                                        (s.width as f64 / old_scale) * scale,
                                        (s.height as f64 / old_scale) * scale,
                                    ),
                                    Err(_) => (384.0 * scale, 500.0 * scale),
                                };
                                let (icon_x, icon_y) = match rect.position {
                                    Position::Physical(p) => (p.x as f64, p.y as f64),
                                    Position::Logical(p) => (p.x * scale, p.y * scale),
                                };
                                let (icon_w, icon_h) = match rect.size {
                                    tauri::Size::Physical(s) => (s.width as f64, s.height as f64),
                                    tauri::Size::Logical(s) => (s.width * scale, s.height * scale),
                                };
                                let center_x = if icon_w > 0.0 {
                                    icon_x + icon_w / 2.0
                                } else {
                                    position.x
                                };
                                let raw_x = center_x - win_w / 2.0;

                                #[cfg(target_os = "macos")]
                                let raw_y = if icon_h > 0.0 {
                                    icon_y + icon_h + 2.0 * scale
                                } else {
                                    position.y + 18.0 * scale
                                };
                                #[cfg(not(target_os = "macos"))]
                                let raw_y = {
                                    let icon_center_y = if icon_h > 0.0 {
                                        icon_y + icon_h / 2.0
                                    } else {
                                        position.y
                                    };
                                    let in_top_half = target_mon
                                        .as_ref()
                                        .map(|m| {
                                            let my = m.position().y as f64;
                                            let mh = m.size().height as f64;
                                            icon_center_y < my + mh / 2.0
                                        })
                                        .unwrap_or(false);
                                    if in_top_half {
                                        if icon_h > 0.0 {
                                            icon_y + icon_h + 2.0 * scale
                                        } else {
                                            position.y + 18.0 * scale
                                        }
                                    } else if icon_h > 0.0 {
                                        icon_y - win_h - 2.0 * scale
                                    } else {
                                        position.y - win_h - 8.0 * scale
                                    }
                                };

                                let (x, y) = if let Some(mon) = target_mon {
                                    let mon_x = mon.position().x as f64;
                                    let mon_y = mon.position().y as f64;
                                    let mon_w = mon.size().width as f64;
                                    let mon_h = mon.size().height as f64;
                                    let margin = 4.0 * scale;
                                    let max_x =
                                        (mon_x + mon_w - win_w - margin).max(mon_x + margin);
                                    let max_y =
                                        (mon_y + mon_h - win_h - margin).max(mon_y + margin);
                                    (
                                        raw_x.clamp(mon_x + margin, max_x) as i32,
                                        raw_y.clamp(mon_y + margin, max_y) as i32,
                                    )
                                } else {
                                    (raw_x as i32, raw_y as i32)
                                };

                                let _ = win
                                    .set_position(Position::Physical(PhysicalPosition::new(x, y)));
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show_main" => {
                        commands::present_main_window(app);
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
                    commands::hide_main_window_to_tray(window.app_handle());
                } else {
                    window.app_handle().exit(0);
                }
            }
            ("tray", WindowEvent::Focused(false)) => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let suppress_blur = window
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|s| {
                        s.switching_in_progress.load(Ordering::SeqCst)
                            || now_ms < s.tray_blur_suppress_until_ms.load(Ordering::SeqCst)
                    })
                    .unwrap_or(false);
                if !suppress_blur {
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
            commands::request_plugin_trust,
            commands::detect_client_status,
            commands::reconcile_active_accounts,
            commands::list_accounts,
            commands::get_account_summary,
            commands::import_local_account,
            commands::preview_local_account,
            commands::launch_tool_profile,
            commands::inspect_profile_dir,
            commands::pick_folder,
            commands::pick_file,
            commands::add_account_by_token,
            commands::start_webview_oauth,
            commands::refresh_account_quota,
            commands::capture_local_session,
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
            commands::export_action_logs,
            update::check_app_update,
            update::download_and_install_update,
            update::open_update_url
        ])
        .build(tauri::generate_context!());

    match app_result {
        Ok(app) => app.run(|app_handle, event| {
            // 退出前把 WAL 合并回主库并清空 -wal：避免残留大体积 -wal 占用磁盘，
            // 也让下次启动直接读主库。覆盖所有退出路径（菜单退出 / 关闭窗口 / exit()）。
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<commands::AppState>() {
                    let db = std::sync::Arc::clone(&state.db);
                    let _ = std::thread::spawn(move || {
                        db.checkpoint_wal_truncate();
                    })
                    .join();
                }
                return;
            }

            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    commands::present_main_window(app_handle);
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        }),
        Err(e) => {
            // GUI 下 panic 用户看不到原因：用原生错误弹窗展示恢复指引
            #[cfg(target_os = "macos")]
            let hint = format!(
                "{e}\n\n若你在覆盖升级 Qwitch.app 后看到过 macOS 钥匙串授权弹窗（例如 dev.qwitch.app），请重新打开 Qwitch 并点击「始终允许」；若误点了拒绝，也可在系统「钥匙串访问」中授权 Qwitch 或恢复 master.key 备份。数据目录中的密钥文件与数据库均已被保留。"
            );
            #[cfg(not(target_os = "macos"))]
            let hint = format!(
                "{e}\n\n若为密钥相关错误，请恢复正确的系统钥匙串条目或 master.key 备份后重试；数据目录中的密钥文件与数据库均已被保留。"
            );
            rfd::MessageDialog::new()
                .set_title("Qwitch 启动失败")
                .set_description(hint)
                .set_level(rfd::MessageLevel::Error)
                .show();
            std::process::exit(1);
        }
    }
}
