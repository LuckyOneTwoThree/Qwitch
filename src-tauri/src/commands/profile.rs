use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::platform::{PlatformHost, ScopedPlatformHost};

use super::{load_app_settings, resolve_client_executable, AppState};

/// 临时解压目录守卫：无论正常返回还是提前 `?` 失败，Drop 时都会清理明文临时目录。
pub(crate) struct TempDirGuard(pub(crate) PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 在目录（含至多 4 层子目录）中查找 Trae CN 的 user-data 根：
/// 即存在 `<root>/User/globalStorage/storage.json` 的目录（跳过符号链接防逃逸）。
fn find_storage_root(dir: &Path, depth: u32) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    if dir
        .join("User")
        .join("globalStorage")
        .join("storage.json")
        .is_file()
    {
        return Some(dir.to_path_buf());
    }
    let rd = std::fs::read_dir(dir).ok()?;
    for entry in rd.flatten() {
        let is_real_dir = entry
            .file_type()
            .map(|t| t.is_dir() && !t.is_symlink())
            .unwrap_or(false);
        if is_real_dir {
            let p = entry.path();
            if let Some(root) = find_storage_root(&p, depth + 1) {
                return Some(root);
            }
        }
    }
    None
}

#[cfg(unix)]
fn set_private_dir_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_private_dir_perms(_path: &Path) {}

#[cfg(unix)]
fn set_private_file_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_file_perms(_path: &Path) {}

fn extract_zip_to(zip_path: &Path, dest: &Path) -> Result<(), String> {
    use std::io::Read;

    const MAX_ZIP_FILE_BYTES: u64 = 200 * 1024 * 1024;
    const MAX_TOTAL_EXTRACTED_BYTES: u64 = 160 * 1024 * 1024;
    const MAX_ARCHIVE_ENTRIES: usize = 4096;
    const SMALL_FILE_CAP: u64 = 16 * 1024 * 1024;
    const VSCDB_FILE_CAP: u64 = 128 * 1024 * 1024;

    let meta = std::fs::metadata(zip_path).map_err(|e| format!("读取压缩包信息失败: {e}"))?;
    if meta.len() > MAX_ZIP_FILE_BYTES {
        return Err("导出包体积超过 200 MiB 上限，已拒绝解压".into());
    }

    std::fs::create_dir_all(dest).map_err(|e| format!("创建解压目录失败: {e}"))?;
    set_private_dir_perms(dest);

    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开导出包失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("无效的 zip 导出包: {e}"))?;

    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("导出包内条目数超过 4096 上限，已拒绝解压".into());
    }

    let mut total_extracted: u64 = 0;

    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("读取导出包条目失败: {e}"))?;

        if entry.enclosed_name().is_none() {
            return Err("导出包包含非法路径（疑含路径穿越）".into());
        }

        let parts: Vec<&str> = entry
            .name()
            .split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .collect();

        if parts.iter().any(|&p| p == ".." || p.contains('\0'))
            || parts.first().is_some_and(|p| p.ends_with(':'))
        {
            return Err("导出包包含非法路径（疑含路径穿越）".into());
        }

        if entry.is_dir()
            || entry.is_symlink()
            || parts.contains(&"__MACOSX")
            || parts.last().is_some_and(|f| f.starts_with("._"))
        {
            continue;
        }

        // 仅允许解压 Trae CN 登录与设备校验所需的白名单文件，并使用常量路径写盘
        let (target_rel, file_cap): (&[&str], u64) =
            if parts.ends_with(&["User", "globalStorage", "storage.json"]) {
                (&["User", "globalStorage", "storage.json"], SMALL_FILE_CAP)
            } else if parts.ends_with(&["User", "globalStorage", "state.vscdb"]) {
                (&["User", "globalStorage", "state.vscdb"], VSCDB_FILE_CAP)
            } else if parts.ends_with(&["aha", "TinyStorage"]) {
                (&["aha", "TinyStorage"], SMALL_FILE_CAP)
            } else {
                continue;
            };

        if entry.size() > file_cap {
            return Err(format!(
                "导出包内文件 {} 超过大小限制",
                target_rel.join("/")
            ));
        }

        let mut target_path = dest.to_path_buf();
        for seg in target_rel {
            target_path.push(seg);
        }
        if let Some(parent) = target_path.parent() {
            let mut cur = dest.to_path_buf();
            if let Ok(rel_parent) = parent.strip_prefix(dest) {
                for comp in rel_parent.components() {
                    cur.push(comp);
                    if !cur.exists() {
                        std::fs::create_dir_all(&cur)
                            .map_err(|e| format!("创建目标子目录失败: {e}"))?;
                        set_private_dir_perms(&cur);
                    }
                }
            }
        }

        let mut limited = entry.take(file_cap + 1);
        let mut buf = Vec::new();
        let written = limited
            .read_to_end(&mut buf)
            .map_err(|e| format!("解压条目内容失败: {e}"))? as u64;
        if written > file_cap {
            return Err(format!(
                "导出包内文件 {} 实际解压体积超限（疑似 zip 炸弹）",
                target_rel.join("/")
            ));
        }

        total_extracted = total_extracted.saturating_add(written);
        if total_extracted > MAX_TOTAL_EXTRACTED_BYTES {
            return Err("导出包总解压体积超过 160 MiB 上限，已拒绝解压".into());
        }

        std::fs::write(&target_path, &buf).map_err(|e| format!("写入解压文件失败: {e}"))?;
        set_private_file_perms(&target_path);
    }

    Ok(())
}

/// 把用户给的「导出包」解析成 Trae CN 的 user-data 根目录。
/// 支持三种输入，尽量傻瓜化、不用用户理解内部结构：
/// - `.zip` 导出包（自动解压、递归定位）
/// - 文件夹（自动在其及子目录里找 `User/globalStorage/storage.json`）
/// - 直接选中 `storage.json`
///
/// 返回 (根目录, 临时解压目录 RAII 守卫)。
fn resolve_profile_root_input(input: &str) -> Result<(PathBuf, Option<TempDirGuard>), String> {
    let p = PathBuf::from(input.trim());
    if !p.exists() {
        return Err(format!("路径不存在：{}", p.display()));
    }
    if p.is_file() {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.ends_with(".zip") {
            let dest =
                std::env::temp_dir().join(format!("qwitch-profile-{}", uuid::Uuid::new_v4()));
            let guard = TempDirGuard(dest.clone());
            extract_zip_to(&p, &dest)?;
            return match find_storage_root(&dest, 0) {
                Some(root) => Ok((root, Some(guard))),
                None => Err("导出包内未找到 Trae CN 的 User/globalStorage/storage.json".into()),
            };
        }
        if name == "storage.json" {
            if let Some(root) = p.parent().and_then(|g| g.parent()).and_then(|u| u.parent()) {
                return Ok((root.to_path_buf(), None));
            }
        }
        return Err("请选择 .zip 导出包，或包含 Trae CN 数据的文件夹".into());
    }
    let root = find_storage_root(&p, 0).ok_or_else(|| {
        "未在该文件夹（及其子目录）中找到 Trae CN 的 User/globalStorage/storage.json".to_string()
    })?;
    Ok((root, None))
}

/// 解析导入来源宿主：指定导出包/文件夹时用 ScopedPlatformHost 把 config_dir 指向它，
/// 从而采集该 profile 独立注册的设备身份（多设备方案 A）。
pub(crate) fn resolve_import_host(
    host: &Arc<dyn PlatformHost>,
    tool_id: &str,
    profile: Option<&str>,
) -> Result<(Arc<dyn PlatformHost>, Option<TempDirGuard>), String> {
    match profile.map(str::trim).filter(|s| !s.is_empty()) {
        Some(input) => {
            let (root, guard) = resolve_profile_root_input(input)?;
            let config_dir = root.join("User").join("globalStorage");
            Ok((
                Arc::new(ScopedPlatformHost::new(
                    Arc::clone(host),
                    tool_id,
                    config_dir,
                )),
                guard,
            ))
        }
        None => Ok((Arc::clone(host), None)),
    }
}

fn mask_device_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 8 {
        format!("{}****", chars.iter().take(4).collect::<String>())
    } else {
        let prefix: String = chars.iter().take(4).collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{prefix}****{suffix}")
    }
}

/// 只读预检一个“设备 profile 目录”是否可用于导入：
/// 是否含 storage.json、其设备 ID（脱敏）、是否与当前本机同一设备。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInspectResult {
    pub has_storage: bool,
    pub storage_path: String,
    pub device_id_masked: Option<String>,
    pub same_as_local: Option<bool>,
}

#[tauri::command]
pub async fn inspect_profile_dir(
    state: State<'_, AppState>,
    tool_id: String,
    profile_dir: String,
) -> Result<ProfileInspectResult, String> {
    let input = profile_dir.trim().to_string();
    if input.is_empty() {
        return Err("设备导出包/目录为空".into());
    }
    let host = Arc::clone(&state.host);
    tokio::task::spawn_blocking(move || {
        let (root, _temp_guard) = resolve_profile_root_input(&input)?;
        let storage = root.join("User").join("globalStorage").join("storage.json");
        let has_storage = storage.is_file();
        let config_dir = root.join("User").join("globalStorage");
        let device_id = if has_storage {
            crate::adapter::trae::TraeAdapter::read_device_id_from_config_dir(&config_dir)
        } else {
            None
        };
        let local_id = host
            .client_config_dir(&tool_id)
            .ok()
            .and_then(|d| crate::adapter::trae::TraeAdapter::read_device_id_from_config_dir(&d));
        let same_as_local = match (&device_id, &local_id) {
            (Some(a), Some(b)) => Some(a == b),
            _ => None,
        };
        Ok(ProfileInspectResult {
            has_storage,
            storage_path: storage.to_string_lossy().to_string(),
            device_id_masked: device_id.as_deref().map(mask_device_id),
            same_as_local,
        })
    })
    .await
    .map_err(|e| format!("预检任务异常中断: {e}"))?
}

/// 原生“选择文件夹”对话框（macOS 需在主线程执行 rfd 弹窗）。
#[tauri::command]
pub async fn pick_folder(app: AppHandle, default_path: Option<String>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let default = default_path.filter(|p| !p.trim().is_empty());
    let spawn = app.run_on_main_thread(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(p) = default.as_deref() {
            if std::path::Path::new(p).exists() {
                dialog = dialog.set_directory(p);
            }
        }
        let picked = dialog
            .pick_folder()
            .map(|p| p.to_string_lossy().to_string());
        let _ = tx.send(picked);
    });
    if spawn.is_err() {
        return None;
    }
    rx.await.ok().flatten()
}

/// 原生“选择文件”对话框（用于选择导出的 zip 包）。
#[tauri::command]
pub async fn pick_file(app: AppHandle, default_path: Option<String>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let def = default_path.filter(|p| !p.trim().is_empty());
    let spawn = app.run_on_main_thread(move || {
        let mut dialog = rfd::FileDialog::new().add_filter("Trae CN 导出包", &["zip"]);
        if let Some(p) = def.as_deref() {
            let path = std::path::Path::new(p);
            if path.is_dir() {
                dialog = dialog.set_directory(path);
            }
        }
        let picked = dialog.pick_file().map(|p| p.to_string_lossy().to_string());
        let _ = tx.send(picked);
    });
    if spawn.is_err() {
        return None;
    }
    rx.await.ok().flatten()
}

/// 拉起某工具的隔离 profile（`--user-data-dir`），用于首次在该 profile 登录以注册独立设备。
/// 返回 profile 根目录（即 `--user-data-dir` 指向目录），供后续按 profile 导入采集设备身份。
#[tauri::command]
pub fn launch_tool_profile(
    state: State<'_, AppState>,
    tool_id: String,
    profile_dir: String,
) -> Result<String, String> {
    let dir = profile_dir.trim();
    if dir.is_empty() {
        return Err("profile_dir 不能为空".into());
    }
    let Some(adapter) = state.adapters.get(&tool_id) else {
        return Err(format!("未知工具适配器: {tool_id}"));
    };
    if !adapter.manifest().capabilities.supports_multi_instance {
        return Err(format!(
            "工具 {} 不支持多开隔离启动",
            adapter.manifest().name
        ));
    }
    let settings = load_app_settings(&state.db);
    let exe = resolve_client_executable(state.host.as_ref(), &settings, &tool_id)
        .ok_or_else(|| format!("未检测到 {tool_id} 客户端安装路径，请先在设置中配置"))?;
    let profile = PathBuf::from(dir);
    std::fs::create_dir_all(&profile).map_err(|e| format!("创建 profile 目录失败: {e}"))?;
    let extensions_dir = profile.join("extensions");
    std::fs::create_dir_all(&extensions_dir).ok();
    let args = vec![
        "--user-data-dir".to_string(),
        profile.to_string_lossy().to_string(),
        "--extensions-dir".to_string(),
        extensions_dir.to_string_lossy().to_string(),
    ];
    state
        .host
        .launch_client(&exe, &args)
        .map_err(|e| e.to_string())?;
    Ok(profile.to_string_lossy().to_string())
}

#[cfg(test)]
mod profile_resolve_tests {
    use super::*;

    #[test]
    fn resolve_profile_root_supports_dir_nested_and_storage_json() {
        let base = std::env::temp_dir().join(format!("qw_profile_{}", uuid::Uuid::new_v4()));
        let root = base.join("Trae CN");
        std::fs::create_dir_all(root.join("User/globalStorage")).unwrap();
        std::fs::write(root.join("User/globalStorage/storage.json"), b"{}").unwrap();

        // 直接选 user-data 根
        let (r1, t1) = resolve_profile_root_input(root.to_str().unwrap()).unwrap();
        assert_eq!(r1, root);
        assert!(t1.is_none());

        // 选上级目录（递归定位）
        let (r2, _) = resolve_profile_root_input(base.to_str().unwrap()).unwrap();
        assert_eq!(r2, root);

        // 直接选 storage.json
        let sj = root.join("User/globalStorage/storage.json");
        let (r3, _) = resolve_profile_root_input(sj.to_str().unwrap()).unwrap();
        assert_eq!(r3, root);

        // 不存在的路径报错
        assert!(resolve_profile_root_input("/no/such/path/xyz").is_err());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_profile_root_extracts_valid_zip_and_guard_cleans_up_temp_dir() {
        use std::io::Write;

        let base = std::env::temp_dir().join(format!("qw_zip_ok_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let zip_path = base.join("TraeCN-profile.zip");

        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();

        writer
            .start_file("Trae CN/User/globalStorage/storage.json", opts)
            .unwrap();
        writer
            .write_all(br#"{"iCubeAuthInfo://icube-dc:dev123":"{}"}"#)
            .unwrap();

        writer.start_file("Trae CN/aha/TinyStorage", opts).unwrap();
        writer.write_all(b"tiny-token").unwrap();

        writer.start_file("Trae CN/ignored.txt", opts).unwrap();
        writer.write_all(b"should-not-extract").unwrap();

        writer
            .start_file("__MACOSX/Trae CN/User/globalStorage/._storage.json", opts)
            .unwrap();
        writer.write_all(b"apple-double").unwrap();

        writer.finish().unwrap();

        let (root, guard) = resolve_profile_root_input(zip_path.to_str().unwrap()).unwrap();
        assert!(guard.is_some());
        assert!(root.join("User/globalStorage/storage.json").is_file());
        assert!(root.join("aha/TinyStorage").is_file());
        assert!(!root.join("ignored.txt").exists());
        assert!(!root.join("Trae CN").exists());

        let extracted_dir = root.clone();
        assert!(extracted_dir.exists());
        drop(guard);
        assert!(!extracted_dir.exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn extract_zip_to_rejects_zip_slip_traversal() {
        use std::io::Write;

        let base = std::env::temp_dir().join(format!("qw_zip_slip_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let zip_path = base.join("malicious.zip");
        let dest = base.join("dest");

        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        writer.start_file("../../escape.txt", opts).unwrap();
        writer.write_all(b"evil").unwrap();
        writer.finish().unwrap();

        let err = extract_zip_to(&zip_path, &dest).unwrap_err();
        assert!(
            err.contains("路径穿越") || err.contains("非法路径"),
            "unexpected err: {err}"
        );
        assert!(!base.join("escape.txt").exists());

        let _ = std::fs::remove_dir_all(&base);
    }
}
