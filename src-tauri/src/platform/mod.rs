use async_trait::async_trait;
use keyring::Entry;
use rand::RngCore;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;

const KEYRING_SERVICE: &str = "dev.qwitch.app";
const LEGACY_KEYRING_SERVICE: &str = "dev.quiver.app";
const KEYRING_MASTER_USER: &str = "master_key_v1";

#[derive(Debug, Error)]
pub enum HostError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Keyring error: {0}")]
    Keyring(String),
    #[error("Platform operation unsupported or failed: {0}")]
    OperationFailed(String),
}

#[derive(Debug, Clone)]
pub struct ClientSpec {
    pub mac_app_names: &'static [&'static str],
    pub mac_process_names: &'static [&'static str],
    pub win_exe_names: &'static [&'static str],
    pub win_folder_names: &'static [&'static str],
}

pub fn client_spec(tool_id: &str) -> Option<ClientSpec> {
    match tool_id {
        "trae" => Some(ClientSpec {
            mac_app_names: &["Trae.app"],
            mac_process_names: &["Trae"],
            win_exe_names: &["Trae.exe"],
            win_folder_names: &["Trae"],
        }),
        "trae_cn" => Some(ClientSpec {
            mac_app_names: &["Trae CN.app"],
            mac_process_names: &["Trae CN"],
            win_exe_names: &["Trae CN.exe"],
            win_folder_names: &["Trae CN"],
        }),
        "workbuddy" => Some(ClientSpec {
            mac_app_names: &["WorkBuddy.app"],
            mac_process_names: &["WorkBuddy"],
            win_exe_names: &["WorkBuddy.exe"],
            win_folder_names: &["WorkBuddy"],
        }),
        "qoder" => Some(ClientSpec {
            mac_app_names: &["Qoder.app"],
            mac_process_names: &["Qoder"],
            win_exe_names: &["Qoder.exe"],
            win_folder_names: &["Qoder"],
        }),
        "qoder_cn" => Some(ClientSpec {
            mac_app_names: &["Qoder CN.app", "Qoder.app"],
            mac_process_names: &["Qoder CN", "Qoder"],
            win_exe_names: &["Qoder CN.exe", "Qoder.exe"],
            win_folder_names: &["Qoder CN", "Qoder"],
        }),
        _ => None,
    }
}

#[async_trait]
pub trait PlatformHost: Send + Sync {
    fn qwitch_data_dir(&self) -> Result<PathBuf, HostError>;
    fn client_config_dir(&self, tool_id: &str) -> Result<PathBuf, HostError>;
    fn detect_client_executable(&self, tool_id: &str) -> Option<PathBuf>;
    fn is_client_running(&self, tool_id: &str) -> Result<bool, HostError>;
    async fn terminate_client_gracefully(
        &self,
        tool_id: &str,
        timeout_ms: u64,
    ) -> Result<(), HostError>;
    fn launch_client(&self, executable: &Path, args: &[String]) -> Result<(), HostError>;
    fn get_or_create_master_key(&self) -> Result<[u8; 32], HostError>;
}

pub struct NativePlatformHost;

impl NativePlatformHost {
    fn resolve_data_dirs() -> Result<(PathBuf, PathBuf), HostError> {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("HOME not set".into()))?;
            let app_support = home.join("Library/Application Support");
            Ok((app_support.join("qwitch"), app_support.join("quiver")))
        }
        #[cfg(target_os = "windows")]
        {
            let appdata = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("APPDATA not set".into()))?;
            Ok((appdata.join("qwitch"), appdata.join("quiver")))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("HOME not set".into()))?;
            let share = home.join(".local/share");
            Ok((share.join("qwitch"), share.join("quiver")))
        }
    }

    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else if src_path.is_file() {
                let _ = std::fs::copy(&src_path, &dst_path);
            }
        }
        Ok(())
    }

    /// 首次从旧版 Quiver 数据目录平滑迁移本地数据库、主密钥文件与自定义扩展目录
    fn migrate_legacy_data_dir(legacy_dir: &Path, new_dir: &Path) {
        if !legacy_dir.is_dir() || legacy_dir == new_dir {
            return;
        }

        let legacy_db = legacy_dir.join("quiver.db");
        let new_db = new_dir.join("qwitch.db");
        if legacy_db.is_file() && !new_db.exists() {
            let tmp_db = new_dir.join(".qwitch.db.migrating");
            if std::fs::copy(&legacy_db, &tmp_db).is_ok() {
                let _ = std::fs::rename(&tmp_db, &new_db);
                for suffix in ["-wal", "-shm"] {
                    let old_side = legacy_dir.join(format!("quiver.db{suffix}"));
                    let new_side = new_dir.join(format!("qwitch.db{suffix}"));
                    if old_side.is_file() && !new_side.exists() {
                        let _ = std::fs::copy(&old_side, &new_side);
                    }
                }
            } else {
                let _ = std::fs::remove_file(&tmp_db);
            }
        }

        let legacy_key = legacy_dir.join("master.key");
        let new_key = new_dir.join("master.key");
        if legacy_key.is_file() && !new_key.exists() {
            let tmp_key = new_dir.join(".master.key.migrating");
            if std::fs::copy(&legacy_key, &tmp_key).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &tmp_key,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
                let _ = std::fs::rename(&tmp_key, &new_key);
            } else {
                let _ = std::fs::remove_file(&tmp_key);
            }
        }

        for subdir in ["adapters", "clients"] {
            let old_sub = legacy_dir.join(subdir);
            let new_sub = new_dir.join(subdir);
            if old_sub.is_dir() && !new_sub.exists() {
                let _ = Self::copy_dir_recursive(&old_sub, &new_sub);
            }
        }
    }
}

#[async_trait]
impl PlatformHost for NativePlatformHost {
    fn qwitch_data_dir(&self) -> Result<PathBuf, HostError> {
        let (dir, legacy_dir) = Self::resolve_data_dirs()?;
        std::fs::create_dir_all(&dir)?;
        Self::migrate_legacy_data_dir(&legacy_dir, &dir);
        Ok(dir)
    }

    fn client_config_dir(&self, tool_id: &str) -> Result<PathBuf, HostError> {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("HOME not set".into()))?;
            let app_support = home.join("Library/Application Support");
            let path = match tool_id {
                "trae" => app_support.join("Trae/User/globalStorage"),
                "trae_cn" => app_support.join("Trae CN/User/globalStorage"),
                "workbuddy" => app_support.join("CodeBuddyExtension/Data/Public/auth"),
                "qoder" => find_latest_prefixed_dir(&app_support, "com.qoder.app")
                    .unwrap_or_else(|| app_support.join("com.qoder.app")),
                "qoder_cn" => find_latest_prefixed_dir(&app_support, "com.qodercn.app")
                    .unwrap_or_else(|| app_support.join("com.qodercn.app")),
                _ => self.qwitch_data_dir()?.join("clients").join(tool_id),
            };
            Ok(path)
        }
        #[cfg(target_os = "windows")]
        {
            let roaming = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("APPDATA not set".into()))?;
            let local = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| roaming.clone());
            let path = match tool_id {
                "trae" => roaming.join("Trae").join("User").join("globalStorage"),
                "trae_cn" => roaming.join("Trae CN").join("User").join("globalStorage"),
                "workbuddy" => local
                    .join("CodeBuddyExtension")
                    .join("Data")
                    .join("Public")
                    .join("auth"),
                "qoder" => find_latest_prefixed_dir(&roaming, "com.qoder.app")
                    .unwrap_or_else(|| roaming.join("com.qoder.app")),
                "qoder_cn" => find_latest_prefixed_dir(&roaming, "com.qodercn.app")
                    .unwrap_or_else(|| roaming.join("com.qodercn.app")),
                _ => self.qwitch_data_dir()?.join("clients").join(tool_id),
            };
            Ok(path)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let base = self.qwitch_data_dir()?;
            Ok(base.join("clients").join(tool_id))
        }
    }

    fn detect_client_executable(&self, tool_id: &str) -> Option<PathBuf> {
        let spec = client_spec(tool_id)?;

        #[cfg(target_os = "macos")]
        {
            let mut roots = vec![PathBuf::from("/Applications")];
            if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
                roots.push(home.join("Applications"));
            }
            for root in roots {
                for app_name in spec.mac_app_names {
                    let bundle = root.join(app_name);
                    if bundle.is_dir() {
                        return Some(bundle);
                    }
                }
            }
            None
        }

        #[cfg(target_os = "windows")]
        {
            let mut bases = Vec::new();
            if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
                bases.push(local.join("Programs"));
            }
            if let Some(pf) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
                bases.push(pf);
            }
            if let Some(pf86) = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from) {
                bases.push(pf86);
            }
            for base in bases {
                for folder in spec.win_folder_names {
                    for exe in spec.win_exe_names {
                        let candidate = base.join(folder).join(exe);
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
            }
            None
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = spec;
            None
        }
    }

    fn is_client_running(&self, tool_id: &str) -> Result<bool, HostError> {
        let Some(spec) = client_spec(tool_id) else {
            return Ok(false);
        };

        #[cfg(target_os = "macos")]
        {
            for proc_name in spec.mac_process_names {
                let status = Command::new("pgrep")
                    .args(["-x", proc_name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()?;
                if status.success() {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        #[cfg(target_os = "windows")]
        {
            for exe in spec.win_exe_names {
                let output = Command::new("tasklist")
                    .args(["/FI", &format!("IMAGENAME eq {exe}"), "/NH"])
                    .output()?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
                if stdout.contains(&exe.to_lowercase()) {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = spec;
            Ok(false)
        }
    }

    async fn terminate_client_gracefully(
        &self,
        tool_id: &str,
        timeout_ms: u64,
    ) -> Result<(), HostError> {
        if !self.is_client_running(tool_id)? {
            return Ok(());
        }

        let Some(spec) = client_spec(tool_id) else {
            return Ok(());
        };

        #[cfg(target_os = "macos")]
        {
            for app_name in spec.mac_app_names {
                let clean = app_name.trim_end_matches(".app");
                let _ = Command::new("osascript")
                    .args(["-e", &format!("tell application \"{clean}\" to quit")])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }

        #[cfg(target_os = "windows")]
        {
            for exe in spec.win_exe_names {
                let _ = Command::new("taskkill")
                    .args(["/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1000));
        while Instant::now() < deadline {
            if !self.is_client_running(tool_id)? {
                // 额外等待 300ms 确保文件锁完全释放
                tokio::time::sleep(Duration::from_millis(300)).await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        // 超时后尝试温和信号或强杀兜底
        #[cfg(target_os = "macos")]
        {
            for proc_name in spec.mac_process_names {
                let _ = Command::new("pkill")
                    .args(["-TERM", "-x", proc_name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        #[cfg(target_os = "windows")]
        {
            for exe in spec.win_exe_names {
                let _ = Command::new("taskkill")
                    .args(["/F", "/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        if self.is_client_running(tool_id)? {
            return Err(HostError::OperationFailed(format!(
                "无法关闭正在运行的 {tool_id} 客户端，请手动退出后重试"
            )));
        }

        Ok(())
    }

    fn launch_client(&self, executable: &Path, args: &[String]) -> Result<(), HostError> {
        #[cfg(target_os = "macos")]
        {
            if executable.extension().and_then(|e| e.to_str()) == Some("app") {
                let mut cmd = Command::new("open");
                cmd.arg("-a").arg(executable);
                if !args.is_empty() {
                    cmd.arg("--args").args(args);
                }
                cmd.spawn()?;
                return Ok(());
            }
        }

        Command::new(executable).args(args).spawn()?;
        Ok(())
    }

    fn get_or_create_master_key(&self) -> Result<[u8; 32], HostError> {
        let key_file = self.qwitch_data_dir().ok().map(|d| d.join("master.key"));
        let read_hex_key = |hex_str: &str| -> Result<[u8; 32], HostError> {
            let bytes = hex::decode(hex_str.trim())
                .map_err(|e| HostError::Keyring(format!("Invalid key format: {e}")))?;
            if bytes.len() != 32 {
                return Err(HostError::Keyring(
                    "Stored master key length is not 32 bytes".into(),
                ));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(key)
        };
        let write_key_file = |path: &Path, hex_key: &str| -> Result<(), std::io::Error> {
            std::fs::write(path, hex_key.as_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(())
        };

        let entry = Entry::new(KEYRING_SERVICE, KEYRING_MASTER_USER).ok();

        if let Some(ref kr) = entry {
            if let Ok(hex_key) = kr.get_password() {
                if let Ok(key) = read_hex_key(&hex_key) {
                    if let Some(ref kf) = key_file {
                        let needs_sync = std::fs::read_to_string(kf)
                            .ok()
                            .map(|s| s.trim() != hex_key.trim())
                            .unwrap_or(true);
                        if needs_sync {
                            let _ = write_key_file(kf, hex_key.trim());
                        }
                    }
                    return Ok(key);
                }
            }
        }

        if let Some(ref kf) = key_file {
            if let Ok(file_hex) = std::fs::read_to_string(kf) {
                if let Ok(key) = read_hex_key(&file_hex) {
                    if let Some(ref kr) = entry {
                        let _ = kr.set_password(file_hex.trim());
                    }
                    return Ok(key);
                }
            }
        }

        if let Ok(legacy_kr) = Entry::new(LEGACY_KEYRING_SERVICE, KEYRING_MASTER_USER) {
            if let Ok(legacy_hex) = legacy_kr.get_password() {
                if let Ok(key) = read_hex_key(&legacy_hex) {
                    if let Some(ref kf) = key_file {
                        let _ = write_key_file(kf, legacy_hex.trim());
                    }
                    if let Some(ref kr) = entry {
                        let _ = kr.set_password(legacy_hex.trim());
                    }
                    return Ok(key);
                }
            }
        }

        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        let hex_key = hex::encode(key);

        let mut persisted = false;
        if let Some(ref kf) = key_file {
            if write_key_file(kf, &hex_key).is_ok() {
                persisted = true;
            }
        }
        if let Some(ref kr) = entry {
            if kr.set_password(&hex_key).is_ok() {
                persisted = true;
            }
        }

        if persisted {
            Ok(key)
        } else {
            Err(HostError::Keyring(
                "Failed to persist master key to keyring or data directory".into(),
            ))
        }
    }
}

fn find_latest_prefixed_dir(parent: &Path, prefix: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(parent).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        match &best {
            Some((best_time, _)) if *best_time >= mtime => {}
            _ => best = Some((mtime, path)),
        }
    }

    best.map(|(_, p)| p)
}
