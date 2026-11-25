//! ACL 一致性守护。
//!
//! Qwitch 在 `build.rs` 里声明了 `AppManifest::commands`，Tauri 会**强制**校验
//! capability；命令只要漏登记在其中一处，调用就会在运行时被 ACL 拒绝，而前端
//! 往往把错误吞成 `console.warn`，表现为「功能静默失效」。
//!
//! 历史上已经踩过两次（`ensure_upstream_models_cmd` 导致自动模型探测长期不生效），
//! 所以这里把三份清单钉在一起：
//!   1. `src/lib.rs` 的 `generate_handler!` 注册
//!   2. `build.rs` 的 `APP_COMMANDS` 清单
//!   3. `capabilities/default.json` 的 `allow-*` 授权

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("读取 {rel} 失败: {e}"))
}

fn registered_commands(lib: &str) -> BTreeSet<String> {
    let start = lib
        .find("invoke_handler")
        .expect("lib.rs 中找不到 invoke_handler");
    let tail = &lib[start..];
    let end = tail.find("])").expect("invoke_handler 块没有结束");
    let block = &tail[..end];

    let mut out = BTreeSet::new();
    for prefix in ["commands::", "update::"] {
        let mut rest = block;
        while let Some(idx) = rest.find(prefix) {
            rest = &rest[idx + prefix.len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
    out
}

fn manifest_commands(build: &str) -> BTreeSet<String> {
    let start = build
        .find("APP_COMMANDS")
        .expect("build.rs 中找不到 APP_COMMANDS");
    let tail = &build[start..];
    let end = tail.find("];").expect("APP_COMMANDS 数组没有结束");
    tail[..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn allowed_commands(caps: &str) -> BTreeSet<String> {
    caps.split('"')
        .filter_map(|s| s.strip_prefix("allow-"))
        .filter(|s| !s.contains(':') && !s.contains('"'))
        .map(|s| s.replace('-', "_"))
        .collect()
}

#[test]
fn every_ipc_command_is_registered_manifested_and_allowed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let registered = registered_commands(&read(root, "src/lib.rs"));
    let manifested = manifest_commands(&read(root, "build.rs"));
    let allowed = allowed_commands(&read(root, "capabilities/default.json"));

    assert!(!registered.is_empty(), "未能解析出任何已注册命令");

    let missing_manifest: Vec<_> = registered.difference(&manifested).cloned().collect();
    let missing_acl: Vec<_> = registered.difference(&allowed).cloned().collect();
    let orphan_manifest: Vec<_> = manifested.difference(&registered).cloned().collect();

    assert!(
        missing_manifest.is_empty(),
        "build.rs 的 APP_COMMANDS 缺少这些命令（运行时会直接被 ACL 拒绝）：{missing_manifest:?}"
    );
    assert!(
        missing_acl.is_empty(),
        "capabilities/default.json 缺少这些命令的 allow-* 授权：{missing_acl:?}"
    );
    assert!(
        orphan_manifest.is_empty(),
        "build.rs 声明了未在 lib.rs 注册的命令：{orphan_manifest:?}"
    );
}

#[test]
fn tray_capabilities_are_a_subset_of_registered_commands() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let registered = registered_commands(&read(root, "src/lib.rs"));
    // 托盘只允许应用级命令的子集；窗口级权限（core:*）已在解析时排除
    let tray = allowed_commands(&read(root, "capabilities/tray.json"));
    let unknown: Vec<_> = tray.difference(&registered).cloned().collect();
    assert!(
        unknown.is_empty(),
        "tray.json 授权了未注册的命令：{unknown:?}"
    );
}
