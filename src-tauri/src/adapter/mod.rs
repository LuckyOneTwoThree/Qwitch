pub mod codearts;
pub mod common;
pub mod doubaowork;
pub mod external;
pub mod lobsterai;
pub mod minimax;
pub mod qoder;
pub mod qoder_cosy;
pub mod safe_storage;
pub mod trae;
pub mod workbuddy;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

use crate::domain::{AuthMethod, CredentialEnvelope, QuotaSnapshot, UnifiedErrorCode};
use crate::platform::PlatformHost;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("[{code:?}] {message}")]
    Unified {
        code: UnifiedErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterCapabilities {
    pub auth_methods: Vec<AuthMethod>,
    pub supports_quota_query: bool,
    pub supports_daily_claim: bool,
    pub supports_client_switch: bool,
    #[serde(default)]
    pub claim_requires_client_switch: bool,
    pub supports_multi_instance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub is_builtin: bool,
    pub capabilities: AdapterCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimResult {
    pub claimed: bool,
    pub reward_summary: String,
    pub updated_quota: Option<QuotaSnapshot>,
}

/// 切号前的客户端会话文件备份（None 表示文件原本不存在）
#[derive(Debug, Clone, Default)]
pub struct SessionBackup {
    pub files: Vec<(std::path::PathBuf, Option<Vec<u8>>)>,
}

pub fn backup_files(paths: &[std::path::PathBuf]) -> Result<SessionBackup, AdapterError> {
    let mut files = Vec::new();
    for p in paths {
        match std::fs::read(p) {
            Ok(bytes) => files.push((p.clone(), Some(bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => files.push((p.clone(), None)),
            // 读取失败 ≠ 文件不存在：中止切号，避免回滚时误删原会话文件
            Err(e) => {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("备份会话文件失败 {}：{e}，已中止切号", p.display()),
                });
            }
        }
    }
    Ok(SessionBackup { files })
}

/// 原子写入私密会话/凭据文件：Unix 下直接以 `0600` 创建临时文件再写盘，消除 `0644 -> 0600` 竞态窗口。
pub fn write_private_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), AdapterError> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("创建会话目录失败 {}: {e}", parent.display()),
        })?;
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session");
    let tmp_name = format!(".{file_name}.qwitch-tmp-{}", uuid::Uuid::new_v4());
    let tmp = match path.parent() {
        Some(parent) => parent.join(&tmp_name),
        None => std::path::PathBuf::from(&tmp_name),
    };

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp).map_err(|e| AdapterError::Unified {
        code: UnifiedErrorCode::InternalError,
        message: format!("创建临时会话文件失败 {}: {e}", path.display()),
    })?;

    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("写入临时会话文件失败 {}: {e}", path.display()),
        });
    }
    drop(file);

    // 覆盖式 rename：POSIX 允许替换仍被其他进程打开的文件，Windows 则会直接返回
    // ERROR_ACCESS_DENIED（客户端还握着 storage.json，或杀软正在扫临时文件）。
    // 切号是用户可感知的高频操作，这里做短重试而不是一次失败就放弃；成功路径
    // 第一次就返回，不引入额外延迟。
    let mut last_err = None;
    for attempt in 0..4 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                if attempt < 3 {
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    let err = last_err.map(|e| e.to_string()).unwrap_or_default();
    Err(AdapterError::Unified {
        code: UnifiedErrorCode::InternalError,
        message: format!(
            "替换会话文件失败 {}: {err}（若客户端仍在运行，请先完全退出后重试）",
            path.display()
        ),
    })
}

fn sqlite_sidecar_base(path: &Path) -> Option<std::path::PathBuf> {
    let raw = path.as_os_str().to_string_lossy();
    for suffix in ["-wal", "-shm", "-journal"] {
        if let Some(base) = raw.strip_suffix(suffix) {
            return Some(std::path::PathBuf::from(base));
        }
    }
    None
}

pub fn restore_files(backup: &SessionBackup) -> Result<(), AdapterError> {
    let mut errors = Vec::new();
    let mut failed_bases: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();

    let mut primary = Vec::new();
    let mut sidecars = Vec::new();
    for (path, content) in &backup.files {
        if let Some(base) = sqlite_sidecar_base(path) {
            sidecars.push((path, content, base));
        } else {
            primary.push((path, content));
        }
    }

    for (path, content) in primary {
        match content {
            Some(bytes) => {
                if let Err(AdapterError::Unified { message, .. }) =
                    write_private_file_atomic(path, bytes)
                {
                    failed_bases.insert(path.clone());
                    errors.push(format!("恢复会话文件失败 {}: {message}", path.display()));
                }
            }
            None => {
                if let Err(e) = std::fs::remove_file(path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        failed_bases.insert(path.clone());
                        errors.push(format!("清理新增会话文件失败 {}: {e}", path.display()));
                    }
                }
            }
        }
    }

    for (path, content, base) in sidecars {
        if failed_bases.contains(&base) {
            errors.push(format!(
                "主库恢复失败，已保留附属日志 {} 以防数据库损坏",
                path.display()
            ));
            continue;
        }
        match content {
            Some(bytes) => {
                if let Err(AdapterError::Unified { message, .. }) =
                    write_private_file_atomic(path, bytes)
                {
                    errors.push(format!("恢复附属日志失败 {}: {message}", path.display()));
                }
            }
            None => {
                if let Err(e) = std::fs::remove_file(path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        errors.push(format!("清理新增附属日志失败 {}: {e}", path.display()));
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: errors.join("；"),
        })
    }
}

#[async_trait]
pub trait ToolAdapter: Send + Sync {
    fn manifest(&self) -> &AdapterManifest;

    /// 纯本地只读探测当前客户端登录账号的真实 UID（严禁联网或拉起外部解密子进程）。
    fn detect_local_active_uid(&self, host: &dyn PlatformHost) -> Option<String> {
        let _ = host;
        None
    }

    /// 纯本地判定「客户端当前是否**明确没登录**」。
    ///
    /// `detect_local_active_uid` 返回 `None` 同时覆盖「没登录」和「本机没装/没数据」，
    /// 两者混在一起会让「本机使用中」标记永远清不掉——实测豆包客户端退出登录后
    /// Preferences 里 `saman.last_user` 被清空，卡片仍显示「本机使用中」，
    /// 用户既看不到「关联本机」也看不到「切至桌面端」。
    ///
    /// 返回 `Some(true)` 表示可以确定客户端未登录（调用方可安全清空该工具的在登标记）；
    /// `None` 表示无法判断，保持原有「绝不误清」策略。
    fn detect_local_logged_out(&self, host: &dyn PlatformHost) -> Option<bool> {
        let _ = host;
        None
    }

    /// Stable opaque key for a device-scoped daily claim quota, if applicable.
    /// Implementations must not expose the raw device identifier here.
    fn daily_claim_device_key(&self) -> Option<String> {
        None
    }

    /// 按账号凭据解析设备维度的每日额度 key（多设备方案：每个账号可携带自己的设备身份）。
    /// 默认退回适配器级 `daily_claim_device_key`，只有支持多设备身份的适配器才需重写。
    fn daily_claim_device_key_for(&self, cred: &CredentialEnvelope) -> Option<String> {
        let _ = cred;
        self.daily_claim_device_key()
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError>;

    async fn parse_manual_token(&self, raw_input: &str)
        -> Result<CredentialEnvelope, AdapterError>;

    async fn inspect_quota(&self, cred: &CredentialEnvelope)
        -> Result<QuotaSnapshot, AdapterError>;

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError>;

    /// 请求前按需刷新凭据（如 OAuth refresh token）。
    /// 返回 Some 表示凭据已更新，调用方需持久化并继续使用新凭据；None 表示无需刷新。
    async fn refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        let _ = cred;
        Ok(None)
    }

    /// 强制刷新凭据（例如上游已返回 401 时跳过本地 `expires_at` 提前量判断）。
    /// 默认退回 `refresh_credential`；支持 OAuth 刷新的适配器应重写此方法以绕过本地缓存判断。
    async fn force_refresh_credential(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<Option<CredentialEnvelope>, AdapterError> {
        self.refresh_credential(cred).await
    }

    /// 查询账号在服务端的最新资料（昵称、邮箱），用于在刷新额度时同步用户名变更。
    /// 默认返回 `None`（不额外发起网络请求）。
    async fn fetch_account_profile(
        &self,
        _cred: &CredentialEnvelope,
    ) -> Option<(Option<String>, Option<String>)> {
        None
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError>;

    /// 切号写入前备份受影响的客户端文件，用于失败回滚。
    /// 默认空备份（不支持回滚的适配器保持原行为）。
    async fn backup_session(&self, host: &dyn PlatformHost) -> Result<SessionBackup, AdapterError> {
        let _ = host;
        Ok(SessionBackup::default())
    }

    async fn restore_session(
        &self,
        host: &dyn PlatformHost,
        backup: &SessionBackup,
    ) -> Result<(), AdapterError> {
        let _ = host;
        restore_files(backup)
    }
}

pub fn create_all_adapters(
    data_dir: &Path,
    trusted_adapters: &[String],
    trusted_adapter_hashes: &HashMap<String, String>,
    host: Arc<dyn PlatformHost>,
) -> HashMap<String, Arc<dyn ToolAdapter>> {
    let mut map: HashMap<String, Arc<dyn ToolAdapter>> = HashMap::new();
    // 优先注册国内版内置适配器（Trae 国际版无每日积分签到，暂不纳入）
    let builtins: Vec<Arc<dyn ToolAdapter>> = vec![
        Arc::new(trae::TraeAdapter::with_host(true, Arc::clone(&host))),
        // WorkBuddy / Qoder 同样注入宿主：其内部读取客户端配置（解密运行时路径、
        // machine-id）必须走同一 PlatformHost，否则多实例 profile 隔离场景会
        // 读到真实本机配置而非目标 profile。
        Arc::new(workbuddy::WorkBuddyAdapter::with_host(Arc::clone(&host))),
        Arc::new(qoder::QoderAdapter::with_host(Arc::clone(&host))),
        Arc::new(minimax::MiniMaxAdapter::new()),
        Arc::new(lobsterai::LobsterAiAdapter::new()),
        Arc::new(doubaowork::DoubaoWorkAdapter::with_host(Arc::clone(&host))),
        // CodeArts 为额度监控型适配器（无每日签到），见 adapter/codearts.rs 模块文档
        Arc::new(codearts::CodeArtsAdapter::with_host(Arc::clone(&host))),
    ];
    for adapter in builtins {
        map.insert(adapter.manifest().id.clone(), adapter);
    }

    // 外部适配器必须经用户显式信任且目录内容哈希与已固定值一致后才注册；内置及保留 ID 一律不允许冒名
    for (id, (ext, dir)) in external::discover_external_adapters(data_dir) {
        if !is_valid_adapter_id(&id) || RESERVED_ADAPTER_IDS.contains(&id.as_str()) {
            continue;
        }
        if !trusted_adapters.iter().any(|t| t == &id) {
            continue;
        }
        let Some(expected_hash) = trusted_adapter_hashes.get(&id) else {
            continue;
        };
        let Ok(actual_hash) = external::adapter_content_hash(&dir) else {
            continue;
        };
        if &actual_hash != expected_hash {
            continue;
        }
        map.entry(id)
            .or_insert_with(|| Arc::new(ext.with_pinned_hash(actual_hash)));
    }

    map
}

/// 适配器 ID 字符集校验：防止路径穿越/注入形态的恶意 ID
pub fn is_valid_adapter_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != "."
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 内置与历史数据保留的适配器 ID，外部插件不得使用（防止冒名接收旧账号凭据）
pub const RESERVED_ADAPTER_IDS: &[&str] = &[
    "trae",
    "qoder",
    "trae_cn",
    "workbuddy",
    "qoder_cn",
    "minimax_code_cn",
    "lobsterai",
    "doubaowork",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(root: &Path, id: &str) {
        let dir = root.join("adapters").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        // 入口必须是插件目录内的实际文件：绝对路径（如 /bin/echo）指向目录外，
        // 不受 adapter_content_hash 约束，现已被 is_valid_entry_command 拒绝。
        std::fs::write(dir.join("run.sh"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::json!({
                "id": id,
                "name": format!("Plugin {id}"),
                "version": "0.0.1",
                "entry": { "command": "./run.sh" },
                "capabilities": {}
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn external_adapters_require_explicit_trust_and_reserved_ids_are_blocked() {
        let root = std::env::temp_dir().join(format!("qwitch_trust_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        write_manifest(&root, "evil_plugin");
        write_manifest(&root, "trae");

        let host: Arc<dyn PlatformHost> = Arc::new(crate::platform::NativePlatformHost);
        let plugin_hash =
            external::adapter_content_hash(&root.join("adapters").join("evil_plugin"))
                .expect("hash");
        let trae_hash =
            external::adapter_content_hash(&root.join("adapters").join("trae")).expect("hash");
        let empty_hashes = HashMap::new();
        let valid_hashes = HashMap::from([
            ("evil_plugin".to_string(), plugin_hash),
            ("trae".to_string(), trae_hash),
        ]);

        // 未信任：外部插件不注册
        let none = create_all_adapters(&root, &[], &empty_hashes, Arc::clone(&host));
        assert!(!none.contains_key("evil_plugin"));
        assert!(none.contains_key("trae_cn"));

        // 仅传 ID 无固定哈希：拒绝注册
        let missing_hash = create_all_adapters(
            &root,
            &["evil_plugin".to_string()],
            &empty_hashes,
            Arc::clone(&host),
        );
        assert!(!missing_hash.contains_key("evil_plugin"));

        // 显式信任且哈希匹配后注册
        let trusted = create_all_adapters(
            &root,
            &["evil_plugin".to_string()],
            &valid_hashes,
            Arc::clone(&host),
        );
        assert!(trusted.contains_key("evil_plugin"));

        // 保留 ID 即使被"信任"且哈希匹配也不允许冒名
        let spoof = create_all_adapters(
            &root,
            &["trae".to_string(), "evil_plugin".to_string()],
            &valid_hashes,
            Arc::clone(&host),
        );
        assert!(!spoof.contains_key("trae"));
        assert!(spoof.contains_key("trae_cn"));

        // 插件内容被篡改后，已固定的旧哈希失配，拒绝加载
        std::fs::write(
            root.join("adapters").join("evil_plugin").join("extra.py"),
            b"evil",
        )
        .unwrap();
        let tampered =
            create_all_adapters(&root, &["evil_plugin".to_string()], &valid_hashes, host);
        assert!(!tampered.contains_key("evil_plugin"));

        assert!(!is_valid_adapter_id("."));
        assert!(!is_valid_adapter_id(".hidden"));
        assert!(!is_valid_adapter_id("plugin."));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn session_backup_restore_roundtrip_and_missing_file_removal() {
        let root =
            std::env::temp_dir().join(format!("qwitch_backup_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let existing = root.join("storage.json");
        let absent = root.join("state.vscdb");
        std::fs::write(&existing, b"original-session").unwrap();

        let backup = backup_files(&[existing.clone(), absent.clone()]).expect("backup");
        assert_eq!(
            backup.files[0].1.as_deref(),
            Some(b"original-session".as_slice())
        );
        assert!(backup.files[1].1.is_none());

        // 模拟切号写入
        std::fs::write(&existing, b"new-session").unwrap();
        std::fs::write(&absent, b"new-vscdb").unwrap();

        restore_files(&backup).expect("restore");
        assert_eq!(std::fs::read(&existing).unwrap(), b"original-session");
        assert!(!absent.exists(), "原本不存在的文件回滚后应删除");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn backup_aborts_on_unreadable_existing_file_instead_of_marking_absent() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("qwitch_backup_perm_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let locked = root.join("storage.json");
        std::fs::write(&locked, b"original").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        // 存在但不可读：必须报错中止，绝不能记为 None（回滚会误删）
        assert!(backup_files(std::slice::from_ref(&locked)).is_err());

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_handles_wal_files_and_continues_after_error() {
        let root =
            std::env::temp_dir().join(format!("qwitch_restore_all_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("state.vscdb");
        let wal = root.join("state.vscdb-wal");
        std::fs::write(&db, b"orig-db").unwrap();
        std::fs::write(&wal, b"orig-wal").unwrap();

        let backup = backup_files(&[db.clone(), wal.clone()]).expect("backup");
        std::fs::write(&db, b"dirty-db").unwrap();
        std::fs::write(&wal, b"dirty-wal").unwrap();

        restore_files(&backup).expect("restore wal + db");
        assert_eq!(std::fs::read(&db).unwrap(), b"orig-db");
        assert_eq!(std::fs::read(&wal).unwrap(), b"orig-wal");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_preserves_wal_sidecar_when_main_db_restore_fails() {
        let root =
            std::env::temp_dir().join(format!("qwitch_restore_wal_guard_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("state.vscdb");
        let wal = root.join("state.vscdb-wal");
        std::fs::write(&db, b"orig-db").unwrap();
        // 原来没有 wal 文件（backup 中对应 None）
        let backup = backup_files(&[db.clone(), wal.clone()]).expect("backup");

        // 模拟切号后产生新 wal，且主库路径无法覆盖（被替换为非空目录）
        std::fs::remove_file(&db).unwrap();
        std::fs::create_dir_all(db.join("blocking_sub")).unwrap();
        std::fs::write(&wal, b"active-wal-data").unwrap();

        assert!(restore_files(&backup).is_err());
        assert!(
            wal.exists(),
            "主库恢复失败时绝不能删除其 -wal，否则会破坏 SQLite 数据库"
        );
        assert_eq!(std::fs::read(&wal).unwrap(), b"active-wal-data");

        let _ = std::fs::remove_dir_all(&root);
    }
}
