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

#[async_trait]
pub trait ToolAdapter: Send + Sync {
    fn manifest(&self) -> &AdapterManifest;

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError>;

    async fn parse_manual_token(&self, raw_input: &str) -> Result<CredentialEnvelope, AdapterError>;

    async fn inspect_quota(&self, cred: &CredentialEnvelope) -> Result<QuotaSnapshot, AdapterError>;

    async fn claim_daily(&self, cred: &CredentialEnvelope) -> Result<ClaimResult, AdapterError>;

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError>;
}

pub fn create_all_adapters(data_dir: &Path, trusted_adapters: &[String]) -> HashMap<String, Arc<dyn ToolAdapter>> {
    let mut map: HashMap<String, Arc<dyn ToolAdapter>> = HashMap::new();
    // 优先注册国内版内置适配器（Trae 国际版无每日积分签到，暂不纳入）
    let builtins: Vec<Arc<dyn ToolAdapter>> = vec![
        Arc::new(trae::TraeAdapter::new(true)),
        Arc::new(workbuddy::WorkBuddyAdapter::new()),
        Arc::new(qoder::QoderAdapter::new()),
    ];
    for adapter in builtins {
        map.insert(adapter.manifest().id.clone(), adapter);
    }

    // 外部适配器必须经用户显式信任后才注册；内置及历史保留 ID 一律不允许冒名
    for (id, ext) in external::discover_external_adapters(data_dir) {
        if RESERVED_ADAPTER_IDS.contains(&id.as_str()) {
            continue;
        }
        if !trusted_adapters.iter().any(|t| t == &id) {
            continue;
        }
        map.entry(id).or_insert(ext);
    }

    map
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

        // 未信任：外部插件不注册
        let none = create_all_adapters(&root, &[]);
        assert!(!none.contains_key("evil_plugin"));
        assert!(none.contains_key("trae_cn"));

        // 显式信任后注册
        let trusted = create_all_adapters(&root, &["evil_plugin".to_string()]);
        assert!(trusted.contains_key("evil_plugin"));

        // 保留 ID 即使被"信任"也不允许冒名
        let spoof = create_all_adapters(&root, &["trae".to_string(), "evil_plugin".to_string()]);
        assert!(!spoof.contains_key("trae"));
        assert!(spoof.contains_key("trae_cn"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
