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

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
fn hide_console_std(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(target_os = "windows")]
fn hide_console_tokio(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(target_os = "macos")]
fn escape_pgrep_regex(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(
            ch,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

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
    fn get_or_create_master_key(&self, expectation: &KeyExpectation)
        -> Result<[u8; 32], HostError>;
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
        let _ = conn.busy_timeout(Duration::from_millis(2000));
        let json: String = conn
            .query_row(
                "SELECT value_json FROM app_settings WHERE key = 'app.settings'",
                [],
                |row| row.get(0),
            )
            .ok()?;
        let val: serde_json::Value = serde_json::from_str(&json).ok()?;
        let raw = val
            .get("customClientPaths")
            .and_then(|m| m.get(tool_id))
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

    #[cfg(target_os = "macos")]
    fn is_macos_proc_running(proc_name: &str) -> Result<bool, HostError> {
        if proc_name.len() <= 15 {
            let status = Command::new("pgrep")
                .args(["-x", proc_name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            return Ok(status.success());
        }
        let needle = format!("/Contents/MacOS/{}", escape_pgrep_regex(proc_name));
        let status = Command::new("pgrep")
            .args(["-f", &needle])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            return Ok(true);
        }
        let prefix: String = proc_name.chars().take(15).collect();
        let fallback_status = Command::new("pgrep")
            .args(["-x", &prefix])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(fallback_status.success())
    }

    #[cfg(target_os = "macos")]
    fn is_macos_app_bundle_running(app_name: &str) -> bool {
        let bundle_needle = format!("/{}/Contents/MacOS/", escape_pgrep_regex(app_name));
        if Command::new("pgrep")
            .args(["-f", &bundle_needle])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return true;
        }
        let clean = app_name.trim_end_matches(".app");
        Self::is_macos_proc_running(clean).unwrap_or(false)
    }

    fn is_client_running_names(
        primary_names: &[String],
        proc_names: &[String],
    ) -> Result<bool, HostError> {
        if primary_names.is_empty() && proc_names.is_empty() {
            return Ok(false);
        }

        #[cfg(target_os = "macos")]
        {
            for app_name in primary_names {
                if Self::is_macos_app_bundle_running(app_name) {
                    return Ok(true);
                }
            }
            for proc_name in proc_names {
                if Self::is_macos_proc_running(proc_name)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        #[cfg(target_os = "windows")]
        {
            let _ = proc_names;
            for exe in primary_names {
                let mut cmd = Command::new("tasklist");
                hide_console_std(&mut cmd);
                let output = cmd
                    .args(["/FI", &format!("IMAGENAME eq {exe}"), "/FO", "CSV", "/NH"])
                    .output()?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
                let target_quoted = format!("\"{}\"", exe.to_lowercase());
                if stdout.contains(&target_quoted) {
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
        keyring_persistent: bool,
        expectation: &KeyExpectation,
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

        // 收集全部候选（仅当钥匙串真正跨进程持久化时才信任其中的密钥），
        // 逐个用指纹/密文样本验证后选中匹配者；验证通过前不做任何删除/迁移
        let mut candidates: Vec<([u8; 32], KeySource)> = Vec::new();
        if keyring_persistent {
            if let Some(key) = keyring_hex.and_then(read_hex_key) {
                candidates.push((key, KeySource::Keyring));
            }
        }
        if let Some(key) = legacy_hex.and_then(read_hex_key) {
            candidates.push((key, KeySource::LegacyKeyring));
        }
        if let Some(key) = file_hex.and_then(read_hex_key) {
            candidates.push((key, KeySource::File));
        }

        for (key, source) in candidates {
            if Self::key_matches_expectation(&key, expectation) {
                return Ok((key, source));
            }
        }

        if expectation.requires_match() {
            return Err(HostError::Keyring(
                "检测到已有 Qwitch 加密数据，但没有可验证匹配的主密钥\
                 （钥匙串/历史钥匙串/本地密钥文件均缺失或与数据库不匹配）。\
                 已保留全部现有密钥文件并拒绝启动，请恢复正确的钥匙串条目或 master.key 备份后重试。"
                    .into(),
            ));
        }
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        Ok((key, KeySource::Generated))
    }

    fn key_matches_expectation(key: &[u8; 32], expectation: &KeyExpectation) -> bool {
        let fp_ok = expectation
            .fingerprint
            .as_deref()
            .map(|fp| crate::storage::crypto::key_fingerprint(key) == fp);
        let sample_ok = expectation
            .sample_blob
            .as_deref()
            .map(|blob| crate::storage::crypto::decrypt_credential(key, blob).is_ok());
        match (fp_ok, sample_ok) {
            // 指纹与样本同时存在时必须两者都匹配，防止旧 key_id 选错钥
            (Some(f), Some(s)) => f && s,
            (Some(f), None) => f,
            (None, Some(s)) => s,
            // 无指纹也无样本（首次运行/空库）：接受候选
            (None, None) => true,
        }
    }

    #[cfg(test)]
    fn get_or_create_master_key_with(
        data_dir: &Path,
        persistent: bool,
    ) -> Result<[u8; 32], HostError> {
        Self::get_or_create_master_key_with_expectation(
            data_dir,
            persistent,
            &KeyExpectation::default(),
        )
    }

    fn get_or_create_master_key_with_expectation(
        data_dir: &Path,
        persistent: bool,
        expectation: &KeyExpectation,
    ) -> Result<[u8; 32], HostError> {
        let key_file = data_dir.join("master.key");

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
            persistent,
            expectation,
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
                } else if write_key_file(&key_file, &hex_key).is_err() {
                    return Err(HostError::Keyring(
                        "历史钥匙串密钥迁移落盘失败，为避免重启后密钥不可用已中止启动".into(),
                    ));
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

/// 启动时用于验证候选主密钥是否匹配当前数据库的依据：
/// 优先 key_id 指纹 + 密文样本双验证；无指纹的旧库用样本试解密。
/// has_encrypted_data 区分"空库首装"与"已有加密数据"——
/// 不能以 qwitch.db 文件是否存在判断（open 会立即创建空库）
#[derive(Debug, Clone, Default)]
pub struct KeyExpectation {
    pub fingerprint: Option<String>,
    pub sample_blob: Option<Vec<u8>>,
    pub has_encrypted_data: bool,
}

impl KeyExpectation {
    fn requires_match(&self) -> bool {
        self.fingerprint.is_some() || self.sample_blob.is_some() || self.has_encrypted_data
    }
}

#[async_trait]
impl PlatformHost for NativePlatformHost {
    fn qwitch_data_dir(&self) -> Result<PathBuf, HostError> {
        let (dir, legacy_dir) = Self::resolve_data_dirs()?;
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
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
        Self::is_client_running_names(&primary_names, &proc_names)
    }

    async fn terminate_client_gracefully(
        &self,
        tool_id: &str,
        timeout_ms: u64,
    ) -> Result<(), HostError> {
        let tid = tool_id.to_string();
        let (primary_names, proc_names) = tokio::task::spawn_blocking(move || {
            NativePlatformHost.resolve_target_process_names(&tid)
        })
        .await
        .map_err(|e| HostError::OperationFailed(e.to_string()))?;

        if primary_names.is_empty() && proc_names.is_empty() {
            return Ok(());
        }

        let check_running = |p_names: &[String], procs: &[String]| {
            let p_clone = p_names.to_vec();
            let proc_clone = procs.to_vec();
            async move {
                tokio::task::spawn_blocking(move || {
                    NativePlatformHost::is_client_running_names(&p_clone, &proc_clone)
                })
                .await
                .map_err(|e| HostError::OperationFailed(e.to_string()))?
            }
        };

        if !check_running(&primary_names, &proc_names).await? {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            for app_name in &primary_names {
                let app_for_check = app_name.clone();
                let app_running = tokio::task::spawn_blocking(move || {
                    NativePlatformHost::is_macos_app_bundle_running(&app_for_check)
                })
                .await
                .unwrap_or(false);
                if !app_running {
                    continue;
                }
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
                let mut cmd = tokio::process::Command::new("taskkill");
                hide_console_tokio(&mut cmd);
                let _ = cmd
                    .args(["/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1000));
        while Instant::now() < deadline {
            if !check_running(&primary_names, &proc_names).await? {
                // 额外等待 300ms 确保文件锁完全释放
                tokio::time::sleep(Duration::from_millis(300)).await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        // 超时后尝试温和信号或强杀兜底
        #[cfg(target_os = "macos")]
        {
            for app_name in &primary_names {
                let needle = format!("/{}/Contents/MacOS/", escape_pgrep_regex(app_name));
                let _ = tokio::process::Command::new("pkill")
                    .args(["-TERM", "-f", &needle])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
            for proc_name in &proc_names {
                if proc_name.len() <= 15 {
                    let _ = tokio::process::Command::new("pkill")
                        .args(["-TERM", "-x", proc_name])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .await;
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            for exe in &primary_names {
                let mut cmd = tokio::process::Command::new("taskkill");
                hide_console_tokio(&mut cmd);
                let _ = cmd
                    .args(["/F", "/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        if check_running(&primary_names, &proc_names).await? {
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
                let status = cmd.status()?;
                if !status.success() {
                    return Err(HostError::OperationFailed(format!(
                        "启动客户端失败 (open exit {}): {}",
                        status,
                        executable.display()
                    )));
                }
                return Ok(());
            }
        }

        let mut child = Command::new(executable).args(args).spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn get_or_create_master_key(
        &self,
        expectation: &KeyExpectation,
    ) -> Result<[u8; 32], HostError> {
        let data_dir = self
            .qwitch_data_dir()
            .map_err(|e| HostError::Keyring(format!("无法解析数据目录，不能托管主密钥: {e}")))?;
        Self::get_or_create_master_key_with_expectation(
            &data_dir,
            Self::keyring_persistence_ok(),
            expectation,
        )
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
                let mut cmd = Command::new("reg");
                hide_console_std(&mut cmd);
                let output = cmd
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
                let mut cmd = Command::new("reg");
                hide_console_std(&mut cmd);
                let _ = cmd
                    .args(["delete", RUN_KEY, "/v", "Qwitch", "/f"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            Ok(())
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
            Ok(())
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
        let wrong_key = [8u8; 32];
        let wrong_hex = hex::encode(wrong_key);
        let no_exp = KeyExpectation::default();
        let resolve = |kr: Option<&str>,
                       legacy: Option<&str>,
                       file: Option<&str>,
                       db: bool,
                       persistent: bool,
                       exp: &KeyExpectation| {
            let mut e = exp.clone();
            e.has_encrypted_data = e.has_encrypted_data || db;
            NativePlatformHost::resolve_master_key_logic(kr, legacy, file, persistent, &e)
        };

        // 已有数据库且所有密钥源不可用：绝不生成新密钥
        assert!(resolve(None, None, None, true, true, &no_exp).is_err());
        assert!(resolve(None, None, None, true, false, &no_exp).is_err());
        // 首次运行才允许生成
        let (_, src) = resolve(None, None, None, false, true, &no_exp).expect("first run");
        assert_eq!(src, KeySource::Generated);
        // 无验证依据时持久化钥匙串优先
        let (k, src) = resolve(Some(&hex_key), None, None, true, true, &no_exp).expect("keyring");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::Keyring);
        // 非持久化（mock）钥匙串不可信，即使有值也回退文件
        let (k, src) =
            resolve(Some(&hex_key), None, Some(&hex_key), true, false, &no_exp).expect("file");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);
        // 旧版 Quiver 钥匙串迁移
        let (_, src) = resolve(None, Some(&hex_key), None, true, true, &no_exp).expect("legacy");
        assert_eq!(src, KeySource::LegacyKeyring);
        // 文件迁移
        let (_, src) =
            resolve(None, None, Some(&hex_key), true, true, &no_exp).expect("file migrate");
        assert_eq!(src, KeySource::File);
        // 非法 hex 长度跳过，落到下一来源
        let (_, src) =
            resolve(Some("abcd"), None, Some(&hex_key), true, true, &no_exp).expect("skip bad");
        assert_eq!(src, KeySource::File);

        // 冲突来源：钥匙串是旧钥、文件才是匹配 key_id 的正确钥 → 选文件
        let fp_exp = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&key)),
            sample_blob: None,
            has_encrypted_data: true,
        };
        let (k, src) = resolve(Some(&wrong_hex), None, Some(&hex_key), true, true, &fp_exp)
            .expect("conflict picks file");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);

        // 冲突来源：钥匙串匹配指纹 → 选钥匙串（即使文件也存在）
        let (k, src) = resolve(Some(&hex_key), None, Some(&wrong_hex), true, true, &fp_exp)
            .expect("conflict picks keyring");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::Keyring);

        // 全部候选都不匹配指纹 → 拒绝（保留文件交由调用方处理）
        assert!(resolve(
            Some(&wrong_hex),
            None,
            Some(&wrong_hex),
            true,
            true,
            &fp_exp
        )
        .is_err());

        // 无指纹旧库：用密文样本验证，只有能解密的候选被选中
        let env = crate::domain::CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "sample_uid".to_string(),
            display_name: None,
            display_email: None,
            payload: crate::domain::CredentialPayload::Token {
                access_token: "sample-token".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: std::collections::HashMap::new(),
            },
        };
        let blob = crate::storage::crypto::encrypt_credential(&key, &env).expect("encrypt");
        let sample_exp = KeyExpectation {
            fingerprint: None,
            sample_blob: Some(blob),
            has_encrypted_data: true,
        };
        let (k, src) = resolve(
            Some(&wrong_hex),
            None,
            Some(&hex_key),
            true,
            true,
            &sample_exp,
        )
        .expect("sample picks file");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);
        assert!(resolve(Some(&wrong_hex), None, None, true, true, &sample_exp).is_err());

        // 双验证：指纹与样本都指向正确钥 B 时，旧钥 A（钥匙串）落选、B（文件）选中
        let dual_exp = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&key)),
            sample_blob: sample_exp.sample_blob.clone(),
            has_encrypted_data: true,
        };
        let (k, src) = resolve(
            Some(&wrong_hex),
            None,
            Some(&hex_key),
            true,
            true,
            &dual_exp,
        )
        .expect("dual verification picks file");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);
        // 只有旧钥 A（指纹/样本都不匹配）→ 拒绝
        assert!(resolve(Some(&wrong_hex), None, None, true, true, &dual_exp).is_err());

        // 指纹与样本互相矛盾（部分备份恢复）：任何候选都无法同时通过 → Fail-Closed
        let split_exp = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&wrong_key)),
            sample_blob: sample_exp.sample_blob.clone(),
            has_encrypted_data: true,
        };
        assert!(resolve(
            Some(&wrong_hex),
            None,
            Some(&hex_key),
            true,
            true,
            &split_exp
        )
        .is_err());
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

        // --- 场景 0：生产顺序首装回归 — open 先创建空 DB，随后选钥必须成功生成 ---
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let dir = temp_data_dir("firstboot");
        let db =
            crate::storage::QwitchDatabase::open(&dir.join("qwitch.db")).expect("open creates db");
        assert!(dir.join("qwitch.db").exists());
        let sample = db.first_credential_blob().expect("query ok");
        assert!(sample.is_none());
        let first_exp = KeyExpectation {
            fingerprint: None,
            sample_blob: sample,
            has_encrypted_data: false,
        };
        let first_key =
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, false, &first_exp)
                .expect("空库首装必须成功生成密钥");
        assert!(dir.join("master.key").exists());
        db.verify_or_init_key_fingerprint(&dir, &first_key)
            .expect("bless first key");
        let _ = std::fs::remove_dir_all(&dir);

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

        // --- 场景 2：已有加密数据但密钥源全失 → 拒绝启动而非生成新钥 ---
        let dir = temp_data_dir("lost");
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        let lost_exp = KeyExpectation {
            has_encrypted_data: true,
            ..Default::default()
        };
        assert!(
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, false, &lost_exp)
                .is_err()
        );
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

        // --- 场景 6：钥匙串残留旧钥 + master.key 是正确钥 + key_id 指向正确钥 ---
        // 必须选中文件密钥、修复钥匙串，且不得在验证前删除正确文件
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("conflict");
        let correct_key = [7u8; 32];
        let stale_key = [4u8; 32];
        fake_store().lock().unwrap().insert(
            format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"),
            hex::encode(stale_key),
        );
        std::fs::write(dir.join("master.key"), hex::encode(correct_key)).unwrap();
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        std::fs::write(
            dir.join("key_id"),
            crate::storage::crypto::key_fingerprint(&correct_key),
        )
        .unwrap();
        let expectation = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&correct_key)),
            sample_blob: None,
            has_encrypted_data: true,
        };
        let key =
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, true, &expectation)
                .expect("conflict resolves to file key");
        assert_eq!(key, correct_key);
        let repaired = fake_store()
            .lock()
            .unwrap()
            .get(&format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"))
            .cloned();
        assert_eq!(
            repaired,
            Some(hex::encode(correct_key)),
            "钥匙串应被修复为正确钥"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 7：所有候选都不匹配 → 拒绝启动且保留全部密钥文件 ---
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("allwrong");
        fake_store().lock().unwrap().insert(
            format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"),
            hex::encode(stale_key),
        );
        std::fs::write(dir.join("master.key"), hex::encode(stale_key)).unwrap();
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        std::fs::write(
            dir.join("key_id"),
            crate::storage::crypto::key_fingerprint(&correct_key),
        )
        .unwrap();
        assert!(
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, true, &expectation)
                .is_err()
        );
        assert!(
            dir.join("master.key").exists(),
            "不匹配时不得删除任何候选密钥文件"
        );

        let _ = std::fs::remove_dir_all(&dir);

        // 恢复 mock，避免影响后续可能使用 keyring 的测试
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    }
}
