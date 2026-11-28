use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::domain::{AuthMethod, WireProtocolKind};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Egress,
    Ingress,
    Hybrid,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginTransport {
    Builtin,
    Declarative,
    Subprocess,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginSource {
    Builtin,
    External,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginEntryMode {
    #[default]
    Oneshot,
    Daemon,
}

fn default_entry_timeout_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub mode: PluginEntryMode,
    #[serde(default = "default_entry_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginBrand {
    #[serde(default)]
    pub short_tagline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemPromptPolicy {
    #[default]
    Standard,
    SanitizeAgentOpeners,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelIdPolicy {
    #[default]
    Scoped,
    AnthropicWhitelistAlias,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EgressAccountCapabilities {
    #[serde(default)]
    pub supports_quota_query: bool,
    #[serde(default)]
    pub supports_daily_claim: bool,
    #[serde(default)]
    pub supports_client_switch: bool,
    #[serde(default)]
    pub claim_requires_client_switch: bool,
    #[serde(default)]
    pub supports_multi_instance: bool,
    #[serde(default)]
    pub concurrent_with_desktop_client: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EgressProvides {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub tool_choice: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub encrypted_reasoning: bool,
    #[serde(default)]
    pub json_schema: bool,
    #[serde(default)]
    pub count_tokens: bool,
    #[serde(default)]
    pub compact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EgressConstraints {
    #[serde(default)]
    pub must_stream: bool,
    #[serde(default)]
    pub requires_instructions: bool,
    #[serde(default)]
    pub disallow_store: bool,
    #[serde(default)]
    pub strip_temperature_on_reasoning: bool,
    #[serde(default)]
    pub system_prompt_policy: SystemPromptPolicy,
}

fn default_auth_scheme() -> String {
    "bearer".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclarativeHttpSpec {
    pub base_url: String,
    #[serde(default = "default_auth_scheme")]
    pub auth_scheme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_path: Option<String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EgressProfile {
    #[serde(default)]
    pub wire_protocols: Vec<WireProtocolKind>,
    #[serde(default)]
    pub auth_methods: Vec<AuthMethod>,
    #[serde(default)]
    pub account_capabilities: EgressAccountCapabilities,
    #[serde(default)]
    pub provides: EgressProvides,
    #[serde(default)]
    pub constraints: EgressConstraints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declarative_http: Option<DeclarativeHttpSpec>,
    #[serde(default)]
    pub default_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IngressRequires {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub thinking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IngressOptional {
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub tool_choice: bool,
    #[serde(default)]
    pub count_tokens: bool,
    #[serde(default)]
    pub compact: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngressQuirks {
    #[serde(default = "default_true")]
    pub supports_custom_base_url: bool,
    #[serde(default)]
    pub model_id_policy: ModelIdPolicy,
    #[serde(default)]
    pub requires_ws_426_fallback: bool,
    #[serde(default)]
    pub supports_own_auth_passthrough: bool,
}

impl Default for IngressQuirks {
    fn default() -> Self {
        Self {
            supports_custom_base_url: true,
            model_id_policy: ModelIdPolicy::Scoped,
            requires_ws_426_fallback: false,
            supports_own_auth_passthrough: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeclarativeWiringDetect {
    #[serde(default)]
    pub macos_config_paths: Vec<String>,
    #[serde(default)]
    pub windows_config_paths: Vec<String>,
    #[serde(default)]
    pub macos_process_patterns: Vec<String>,
    #[serde(default)]
    pub windows_exe_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeclarativeWiringPatch {
    pub format: String,
    #[serde(default)]
    pub target_file_index: usize,
    #[serde(default)]
    pub set_keys: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub drift_assert_contains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclarativeWiringSpec {
    #[serde(default)]
    pub detect: DeclarativeWiringDetect,
    pub patch: DeclarativeWiringPatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IngressProfile {
    #[serde(default)]
    pub wire_protocols: Vec<WireProtocolKind>,
    #[serde(default)]
    pub requires: IngressRequires,
    #[serde(default)]
    pub optional: IngressOptional,
    #[serde(default)]
    pub quirks: IngressQuirks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declarative_wiring: Option<DeclarativeWiringSpec>,
}

fn default_schema_version() -> u32 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: PluginKind,
    pub transport: PluginTransport,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub brand: PluginBrand,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<PluginEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress_profile: Option<EgressProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress_profile: Option<IngressProfile>,
}

/// 校验插件 ID：非空、<= 64 字符、ASCII 字母数字开头，仅允许小写字母、数字、`-`、`_`，且不含 `..`。
pub fn is_valid_plugin_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 64 || id.contains("..") {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_plugin_ids() {
        assert!(is_valid_plugin_id("codex_sub"));
        assert!(is_valid_plugin_id("qoder-ingress-1"));
        assert!(!is_valid_plugin_id(""));
        assert!(!is_valid_plugin_id("..evil"));
        assert!(!is_valid_plugin_id("-bad"));
        assert!(!is_valid_plugin_id("UpperCase"));
        assert!(!is_valid_plugin_id("bad/slash"));
    }
}
