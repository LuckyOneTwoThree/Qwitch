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
    fn set_launch_at_login(&self, enabled: bool) -> Result<(), String>;
}

pub struct NativePlatformHost;

impl NativePlatformHost {
    fn read_custom_client_path(&self, tool_id: &str) -> Option<PathBuf> {
        let db_path = self.qwitch_data_dir().ok()?.join("qwitch.db");
        if !db_path.is_file() {
            return None;
        }
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()?;
        let json: String = conn
            .query_row(
                "SELECT value_json FROM app_settings WHERE key = 'app.settings'",
                [],
                |row| row.get(0),
            )
            .ok()?;
        let val: serde_json::Value = serde_json::from_str(&json).ok()?;
        let raw = val
            .pointer(&format!("/customClientPaths/{tool_id}"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        let path = PathBuf::from(raw);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    fn resolve_target_process_names(&self, tool_id: &str) -> (Vec<String>, Vec<String>) {
        let mut mac_apps = Vec::new();
        let mut mac_procs = Vec::new();
        let mut win_exes = Vec::new();

        if let Some(spec) = client_spec(tool_id) {
            for a in spec.mac_app_names {
                mac_apps.push((*a).to_string());
            }
            for p in spec.mac_process_names {
                mac_procs.push((*p).to_string());
            }
            for e in spec.win_exe_names {
                win_exes.push((*e).to_string());
            }
        }

        if let Some(custom) = self.read_custom_client_path(tool_id) {
            if let Some(name) = custom.file_name().and_then(|n| n.to_str()) {
                #[cfg(target_os = "macos")]
                {
                    if name.ends_with(".app") {
                        if !mac_apps.iter().any(|x| x == name) {
                            mac_apps.push(name.to_string());
                        }
                        let stem = name.trim_end_matches(".app").to_string();
                        if !mac_procs.iter().any(|x| x == &stem) {
                            mac_procs.push(stem);
                        }
                    } else if !mac_procs.iter().any(|x| x == name) {
                        mac_procs.push(name.to_string());
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    if name.to_lowercase().ends_with(".exe")
                        && !win_exes.iter().any(|x| x.eq_ignore_ascii_case(name))
                    {
                        win_exes.push(name.to_string());
                    }
                }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                {
                    let _ = name;
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            (mac_apps, mac_procs)
        }
        #[cfg(target_os = "windows")]
        {
            let _ = (mac_apps, mac_procs);
            (win_exes, Vec::new())
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (mac_apps, mac_procs, win_exes);
            (Vec::new(), Vec::new())
        }
    }

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
                    let _ =
                        std::fs::set_permissions(&tmp_key, std::fs::Permissions::from_mode(0o600));
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

    fn keyring_persistence_ok() -> bool {
        matches!(
            keyring::default::default_credential_builder().persistence(),
            keyring::credential::CredentialPersistence::UntilDelete
        )
    }

    fn resolve_master_key_logic(
        keyring_hex: Option<&str>,
        legacy_hex: Option<&str>,
        file_hex: Option<&str>,
        db_exists: bool,
        keyring_persistent: bool,
    ) -> Result<([u8; 32], KeySource), HostError> {
        let read_hex_key = |hex_str: &str| -> Option<[u8; 32]> {
            let bytes = hex::decode(hex_str.trim()).ok()?;
            if bytes.len() != 32 {
                return None;
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Some(key)
        };

        // 仅当钥匙串真正跨进程持久化时才信任其中的密钥（mock/进程级存储重启即空）
        if keyring_persistent {
            if let Some(key) = keyring_hex.and_then(read_hex_key) {
                return Ok((key, KeySource::Keyring));
            }
        }
        if let Some(key) = legacy_hex.and_then(read_hex_key) {
            return Ok((key, KeySource::LegacyKeyring));
        }
        if let Some(key) = file_hex.and_then(read_hex_key) {
            return Ok((key, KeySource::File));
        }
        if db_exists {
            return Err(HostError::Keyring(
                "检测到已有 Qwitch 数据库，但系统钥匙串与本地密钥文件均不可用。\
                 为避免生成新密钥导致既有凭据永久无法解密，已拒绝启动。\
                 请恢复钥匙串条目或 master.key 备份后重试。"
                    .into(),
            ));
        }
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        Ok((key, KeySource::Generated))
    }

    fn get_or_create_master_key_in(data_dir: &Path) -> Result<[u8; 32], HostError> {
        Self::get_or_create_master_key_with(data_dir, Self::keyring_persistence_ok())
    }

    fn get_or_create_master_key_with(
        data_dir: &Path,
        persistent: bool,
    ) -> Result<[u8; 32], HostError> {
        let key_file = data_dir.join("master.key");
        let db_exists = data_dir.join("qwitch.db").exists();

        let write_key_file = |path: &Path, hex_key: &str| -> Result<(), std::io::Error> {
            std::fs::write(path, hex_key.as_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(())
        };
        let remove_key_file = || {
            if key_file.exists() {
                let _ = std::fs::remove_file(&key_file);
            }
        };
        let persist_in_keyring = |kr: &Entry, hex_key: &str| -> bool {
            if kr.set_password(hex_key.trim()).is_err() {
                return false;
            }
            kr.get_password()
                .ok()
                .map(|stored| stored.trim().eq_ignore_ascii_case(hex_key.trim()))
                .unwrap_or(false)
        };

        let entry = if persistent {
            Entry::new(KEYRING_SERVICE, KEYRING_MASTER_USER).ok()
        } else {
            None
        };
        let keyring_hex = entry.as_ref().and_then(|kr| kr.get_password().ok());
        let legacy_hex = if persistent {
            Entry::new(LEGACY_KEYRING_SERVICE, KEYRING_MASTER_USER)
                .ok()
                .and_then(|kr| kr.get_password().ok())
        } else {
            None
        };
        let file_hex = std::fs::read_to_string(&key_file).ok();

        let (key, source) = Self::resolve_master_key_logic(
            keyring_hex.as_deref(),
            legacy_hex.as_deref(),
            file_hex.as_deref(),
            db_exists,
            persistent,
        )?;
        let hex_key = hex::encode(key);

        match source {
            KeySource::Keyring => remove_key_file(),
            KeySource::LegacyKeyring => {
                let migrated = entry
                    .as_ref()
                    .map(|kr| persist_in_keyring(kr, &hex_key))
                    .unwrap_or(false);
                if migrated {
                    remove_key_file();
                } else {
                    let _ = write_key_file(&key_file, &hex_key);
                }
            }
            KeySource::File => {
                // 仅在钥匙串可靠持久化时才迁移并清理明文副本，否则保留文件作为唯一密钥源
                if let Some(kr) = entry.as_ref() {
                    if persist_in_keyring(kr, &hex_key) {
                        remove_key_file();
                    }
                }
            }
            KeySource::Generated => {
                let persisted = entry
                    .as_ref()
                    .map(|kr| persist_in_keyring(kr, &hex_key))
                    .unwrap_or(false);
                if persisted {
                    remove_key_file();
                } else if write_key_file(&key_file, &hex_key).is_err() {
                    return Err(HostError::Keyring(
                        "无法持久化主密钥到钥匙串或数据目录".into(),
                    ));
                }
            }
        }
        Ok(key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySource {
    Keyring,
    LegacyKeyring,
    File,
    Generated,
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
        if let Some(custom) = self.read_custom_client_path(tool_id) {
            return Some(custom);
        }
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
        let (primary_names, proc_names) = self.resolve_target_process_names(tool_id);
        if primary_names.is_empty() && proc_names.is_empty() {
            return Ok(false);
        }

        #[cfg(target_os = "macos")]
        {
            for proc_name in proc_names {
                let status = Command::new("pgrep")
                    .args(["-x", &proc_name])
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
            let _ = proc_names;
            for exe in primary_names {
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
            let _ = (primary_names, proc_names);
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

        let (primary_names, proc_names) = self.resolve_target_process_names(tool_id);
        if primary_names.is_empty() && proc_names.is_empty() {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            for app_name in &primary_names {
                let clean = app_name
                    .trim_end_matches(".app")
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                let _ = tokio::process::Command::new("osascript")
                    .args(["-e", &format!("tell application \"{clean}\" to quit")])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }

        #[cfg(target_os = "windows")]
        {
            for exe in &primary_names {
                let _ = tokio::process::Command::new("taskkill")
                    .args(["/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
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
            for proc_name in &proc_names {
                let _ = tokio::process::Command::new("pkill")
                    .args(["-TERM", "-x", proc_name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }
        #[cfg(target_os = "windows")]
        {
            for exe in &primary_names {
                let _ = tokio::process::Command::new("taskkill")
                    .args(["/F", "/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
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
        let data_dir = self
            .qwitch_data_dir()
            .map_err(|e| HostError::Keyring(format!("无法解析数据目录，不能托管主密钥: {e}")))?;
        Self::get_or_create_master_key_in(&data_dir)
    }

    fn set_launch_at_login(&self, enabled: bool) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| "无法读取 HOME 目录，不能配置 macOS 开机自启".to_string())?;
            let agents_dir = home.join("Library").join("LaunchAgents");
            let plist_path = agents_dir.join("dev.qwitch.app.plist");
            if enabled {
                let exe =
                    std::env::current_exe().map_err(|e| format!("无法获取当前程序路径: {e}"))?;
                std::fs::create_dir_all(&agents_dir)
                    .map_err(|e| format!("创建 LaunchAgents 目录失败: {e}"))?;
                let plist = render_macos_launch_agent_plist(&exe);
                std::fs::write(&plist_path, plist.as_bytes())
                    .map_err(|e| format!("写入开机自启配置失败: {e}"))?;
            } else if plist_path.exists() {
                std::fs::remove_file(&plist_path)
                    .map_err(|e| format!("移除开机自启配置失败: {e}"))?;
            }
            Ok(())
        }

        #[cfg(target_os = "windows")]
        {
            const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
            if enabled {
                let exe =
                    std::env::current_exe().map_err(|e| format!("无法获取当前程序路径: {e}"))?;
                let value = format!("\"{}\" --autostart", exe.to_string_lossy());
                let output = Command::new("reg")
                    .args([
                        "add", RUN_KEY, "/v", "Qwitch", "/t", "REG_SZ", "/d", &value, "/f",
                    ])
                    .output()
                    .map_err(|e| format!("执行 reg add 失败: {e}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "写入 Windows 开机自启注册表失败: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
            } else {
                let _ = Command::new("reg")
                    .args(["delete", RUN_KEY, "/v", "Qwitch", "/f"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            return Ok(());
        }

        #[cfg(target_os = "linux")]
        {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .ok_or_else(|| "无法定位用户配置目录，不能配置 Linux 开机自启".to_string())?;
            let autostart_dir = base.join("autostart");
            let desktop_path = autostart_dir.join("dev.qwitch.app.desktop");
            if enabled {
                let exe =
                    std::env::current_exe().map_err(|e| format!("无法获取当前程序路径: {e}"))?;
                std::fs::create_dir_all(&autostart_dir)
                    .map_err(|e| format!("创建 autostart 目录失败: {e}"))?;
                let content = format!(
                    "[Desktop Entry]\nType=Application\nName=Qwitch\nExec=\"{}\" --autostart\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
                    exe.to_string_lossy()
                );
                std::fs::write(&desktop_path, content.as_bytes())
                    .map_err(|e| format!("写入开机自启配置失败: {e}"))?;
            } else if desktop_path.exists() {
                std::fs::remove_file(&desktop_path)
                    .map_err(|e| format!("移除开机自启配置失败: {e}"))?;
            }
            return Ok(());
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = enabled;
            Ok(())
        }
    }
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn render_macos_launch_agent_plist(exe_path: &Path) -> String {
    let escaped_exe = escape_xml(&exe_path.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.qwitch.app</string>
  <key>ProgramArguments</key>
  <array>
    <string>{escaped_exe}</string>
    <string>--autostart</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn macos_launch_agent_plist_escapes_path_and_sets_autostart() {
        let exe = PathBuf::from("/Applications/Qwitch & Tools.app/Contents/MacOS/qwitch");
        let plist = render_macos_launch_agent_plist(&exe);
        assert!(plist.contains("<string>dev.qwitch.app</string>"));
        assert!(plist.contains(
            "<string>/Applications/Qwitch &amp; Tools.app/Contents/MacOS/qwitch</string>"
        ));
        assert!(plist.contains("<string>--autostart</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
    }

    #[test]
    fn master_key_resolution_decision_matrix() {
        let key = [9u8; 32];
        let hex_key = hex::encode(key);
        let resolve = |kr: Option<&str>,
                       legacy: Option<&str>,
                       file: Option<&str>,
                       db: bool,
                       persistent: bool| {
            NativePlatformHost::resolve_master_key_logic(kr, legacy, file, db, persistent)
        };

        // 已有数据库且所有密钥源不可用：绝不生成新密钥
        assert!(resolve(None, None, None, true, true).is_err());
        assert!(resolve(None, None, None, true, false).is_err());
        // 首次运行才允许生成
        let (_, src) = resolve(None, None, None, false, true).expect("first run");
        assert_eq!(src, KeySource::Generated);
        // 持久化钥匙串优先
        let (k, src) = resolve(Some(&hex_key), None, None, true, true).expect("keyring");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::Keyring);
        // 非持久化（mock）钥匙串不可信，即使有值也回退文件
        let (k, src) = resolve(Some(&hex_key), None, Some(&hex_key), true, false).expect("file");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);
        // 旧版 Quiver 钥匙串迁移
        let (_, src) = resolve(None, Some(&hex_key), None, true, true).expect("legacy");
        assert_eq!(src, KeySource::LegacyKeyring);
        // 文件迁移
        let (_, src) = resolve(None, None, Some(&hex_key), true, true).expect("file migrate");
        assert_eq!(src, KeySource::File);
        // 非法 hex 长度跳过，落到下一来源
        let (_, src) = resolve(Some("abcd"), None, Some(&hex_key), true, true).expect("skip bad");
        assert_eq!(src, KeySource::File);
    }

    struct FakeCredential {
        id: String,
    }

    fn fake_store() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
        static STORE: std::sync::OnceLock<
            std::sync::Mutex<std::collections::HashMap<String, String>>,
        > = std::sync::OnceLock::new();
        STORE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
    }

    impl keyring::credential::CredentialApi for FakeCredential {
        fn set_secret(&self, password: &[u8]) -> keyring::Result<()> {
            fake_store().lock().unwrap().insert(
                self.id.clone(),
                String::from_utf8_lossy(password).to_string(),
            );
            Ok(())
        }
        fn get_secret(&self) -> keyring::Result<Vec<u8>> {
            fake_store()
                .lock()
                .unwrap()
                .get(&self.id)
                .map(|s| s.clone().into_bytes())
                .ok_or(keyring::Error::NoEntry)
        }
        fn delete_credential(&self) -> keyring::Result<()> {
            fake_store()
                .lock()
                .unwrap()
                .remove(&self.id)
                .map(|_| ())
                .ok_or(keyring::Error::NoEntry)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct FakePersistentBuilder;

    impl keyring::credential::CredentialBuilderApi for FakePersistentBuilder {
        fn build(
            &self,
            _target: Option<&str>,
            service: &str,
            user: &str,
        ) -> keyring::Result<Box<keyring::Credential>> {
            Ok(Box::new(FakeCredential {
                id: format!("{service}:{user}"),
            }))
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn persistence(&self) -> keyring::credential::CredentialPersistence {
            keyring::credential::CredentialPersistence::UntilDelete
        }
    }

    fn temp_data_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qwitch_keytest_{tag}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    // 全局 credential builder 是进程级状态，所有涉及 builder 切换的场景
    // 必须在单个测试内串行执行，避免与其他测试竞态。
    // 注意：set_default_credential_builder 只影响 Entry::new，不影响
    // default::default_credential_builder()，因此持久化标志经参数显式传入。
    #[test]
    fn master_key_restart_regression_and_persistence_guards() {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert!(
            NativePlatformHost::keyring_persistence_ok(),
            "启用原生 feature 后平台默认钥匙串必须跨进程持久化"
        );

        // --- 场景 1：mock（非持久化）钥匙串下重启不得丢钥、不得删明文副本 ---
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());

        let dir = temp_data_dir("mock");
        let key1 =
            NativePlatformHost::get_or_create_master_key_with(&dir, false).expect("first boot");
        assert!(
            dir.join("master.key").exists(),
            "mock 环境必须保留明文密钥文件"
        );

        // 模拟已有数据库
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        // 模拟重启：新 Entry 的 mock 存储为空
        let key2 = NativePlatformHost::get_or_create_master_key_with(&dir, false).expect("restart");
        assert_eq!(key1, key2, "重启后必须取回同一主密钥");
        assert!(dir.join("master.key").exists(), "重启后明文副本仍须保留");
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 2：已有数据库但密钥源全失 → 拒绝启动而非生成新钥 ---
        let dir = temp_data_dir("lost");
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        assert!(NativePlatformHost::get_or_create_master_key_with(&dir, false).is_err());
        assert!(!dir.join("master.key").exists());
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 3：真正持久化的钥匙串 → 托管成功且不落明文 ---
        fake_store().lock().unwrap().clear();
        keyring::set_default_credential_builder(Box::new(FakePersistentBuilder));

        let dir = temp_data_dir("persistent");
        let key1 =
            NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("first boot");
        assert!(!dir.join("master.key").exists(), "持久化钥匙串不应落明文");
        let key2 = NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("restart");
        assert_eq!(key1, key2);
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 4：遗留 master.key 迁移到可靠钥匙串后才清理 ---
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("migrate");
        let legacy_key = [5u8; 32];
        std::fs::write(dir.join("master.key"), hex::encode(legacy_key)).unwrap();
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        let key = NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("migrate");
        assert_eq!(key, legacy_key);
        assert!(!dir.join("master.key").exists(), "迁移成功后清理明文");
        let key_again =
            NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("restart");
        assert_eq!(key_again, legacy_key);
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 5：旧版 Quiver 钥匙串迁移 ---
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("quiver");
        let quiver_key = [6u8; 32];
        fake_store().lock().unwrap().insert(
            format!("{LEGACY_KEYRING_SERVICE}:{KEYRING_MASTER_USER}"),
            hex::encode(quiver_key),
        );
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        let key =
            NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("quiver migrate");
        assert_eq!(key, quiver_key);
        let stored = fake_store()
            .lock()
            .unwrap()
            .get(&format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"))
            .cloned();
        assert_eq!(stored, Some(hex::encode(quiver_key)));
        let _ = std::fs::remove_dir_all(&dir);

        // 恢复 mock，避免影响后续可能使用 keyring 的测试
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    }
}
