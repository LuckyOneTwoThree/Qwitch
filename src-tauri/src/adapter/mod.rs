pub mod common;
pub mod external;
pub mod qoder;
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

pub fn restore_files(backup: &SessionBackup) -> Result<(), AdapterError> {
    let mut errors = Vec::new();
    for (path, content) in &backup.files {
        match content {
            Some(bytes) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("session");
                let tmp = match path.parent() {
                    Some(parent) => parent.join(format!(".{file_name}.qwitch-restore-tmp")),
                    None => path.with_extension("qwitch-restore-tmp"),
                };
                if let Err(e) = std::fs::write(&tmp, bytes) {
                    let _ = std::fs::remove_file(&tmp);
                    errors.push(format!("恢复会话文件失败 {}: {e}", path.display()));
                    continue;
                }
                if let Err(e) = std::fs::rename(&tmp, path) {
                    let _ = std::fs::remove_file(&tmp);
                    errors.push(format!("恢复会话文件失败 {}: {e}", path.display()));
                }
            }
            None => {
                if let Err(e) = std::fs::remove_file(path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        errors.push(format!("清理新增会话文件失败 {}: {e}", path.display()));
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

    /// Stable opaque key for a device-scoped daily claim quota, if applicable.
    /// Implementations must not expose the raw device identifier here.
    fn daily_claim_device_key(&self) -> Option<String> {
        None
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
    host: Arc<dyn PlatformHost>,
) -> HashMap<String, Arc<dyn ToolAdapter>> {
    let mut map: HashMap<String, Arc<dyn ToolAdapter>> = HashMap::new();
    // 优先注册国内版内置适配器（Trae 国际版无每日积分签到，暂不纳入）
    let builtins: Vec<Arc<dyn ToolAdapter>> = vec![
        Arc::new(trae::TraeAdapter::with_host(true, host)),
        Arc::new(workbuddy::WorkBuddyAdapter::new()),
        Arc::new(qoder::QoderAdapter::new()),
    ];
    for adapter in builtins {
        map.insert(adapter.manifest().id.clone(), adapter);
    }

    // 外部适配器必须经用户显式信任后才注册；内置及历史保留 ID 一律不允许冒名
    for (id, ext) in external::discover_external_adapters(data_dir) {
        if !is_valid_adapter_id(&id) || RESERVED_ADAPTER_IDS.contains(&id.as_str()) {
            continue;
        }
        if !trusted_adapters.iter().any(|t| t == &id) {
            continue;
        }
        map.entry(id).or_insert(ext);
    }

    map
}

/// 适配器 ID 字符集校验：防止路径穿越/注入形态的恶意 ID
pub fn is_valid_adapter_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 内置与历史数据保留的适配器 ID，外部插件不得使用（防止冒名接收旧账号凭据）
pub const RESERVED_ADAPTER_IDS: &[&str] = &["trae", "qoder", "trae_cn", "workbuddy", "qoder_cn"];

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(root: &Path, id: &str) {
        let dir = root.join("adapters").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::json!({
                "id": id,
                "name": format!("Plugin {id}"),
                "version": "0.0.1",
                "entry": { "command": "/bin/echo" },
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

        // 未信任：外部插件不注册
        let none = create_all_adapters(&root, &[], Arc::clone(&host));
        assert!(!none.contains_key("evil_plugin"));
        assert!(none.contains_key("trae_cn"));

        // 显式信任后注册
        let trusted = create_all_adapters(&root, &["evil_plugin".to_string()], Arc::clone(&host));
        assert!(trusted.contains_key("evil_plugin"));

        // 保留 ID 即使被"信任"也不允许冒名
        let spoof = create_all_adapters(
            &root,
            &["trae".to_string(), "evil_plugin".to_string()],
            host,
        );
        assert!(!spoof.contains_key("trae"));
        assert!(spoof.contains_key("trae_cn"));

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
        assert!(!root.join(".state.vscdb.qwitch-restore-tmp").exists());
        assert!(!root.join(".state.vscdb-wal.qwitch-restore-tmp").exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
