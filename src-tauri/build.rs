fn main() {
    // 为应用自定义 IPC 命令声明 ACL 清单。
    //
    // 一旦这里声明了 `app_manifest`，Tauri 会对**所有**自定义命令强制校验
    // capability（见 tauri `webview/mod.rs` 中 `has_app_acl_manifest` 分支），
    // 未在 capabilities 中授权的命令会被直接拒绝。因此：
    //   * 本列表必须与 `lib.rs` 的 `generate_handler!` 完全一致；
    //   * `capabilities/default.json` 必须显式允许这些命令。
    //
    // 新增命令时三处需同步：`lib.rs` 注册、本列表、`capabilities/default.json`。
    const APP_COMMANDS: &[&str] = &[
        "show_main_window",
        "quit_app",
        "get_app_settings",
        "save_app_settings",
        "list_adapters",
        "list_external_adapters",
        "request_plugin_trust",
        "detect_client_status",
        "reconcile_active_accounts",
        "list_accounts",
        "get_account_summary",
        "import_local_account",
        "preview_local_account",
        "launch_tool_profile",
        "inspect_profile_dir",
        "pick_folder",
        "pick_file",
        "add_account_by_token",
        "start_webview_oauth",
        "refresh_account_quota",
        "refresh_quota_batch",
        "switch_desktop_account",
        "delete_account",
        "update_account_status",
        "rename_account",
        "claim_daily_batch",
        "get_action_logs",
        "get_action_log_stats",
        "archive_action_logs",
        "unarchive_action_logs",
        "delete_action_logs",
        "export_action_logs",
        "check_app_update",
        "download_and_install_update",
        "open_update_url",
    ];

    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(APP_COMMANDS));

    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
