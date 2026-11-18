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

pub fn create_all_adapters(data_dir: &Path) -> HashMap<String, Arc<dyn ToolAdapter>> {
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

    // 加载外部适配器（若与内置同 ID 则不覆盖内置，保障核心稳定性）
    for (id, ext) in external::discover_external_adapters(data_dir) {
        map.entry(id).or_insert(ext);
    }

    map
}
