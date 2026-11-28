use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::domain::{AuthMethod, WireProtocolKind};
use crate::plugins::egress::{create_builtin_egress_transports, EgressTransport};
use crate::plugins::ingress::{create_builtin_ingress_plugins, IngressPlugin};
use crate::plugins::manifest::{
    EgressAccountCapabilities, EgressConstraints, EgressProfile, EgressProvides, IngressOptional,
    IngressProfile, IngressQuirks, IngressRequires, ModelIdPolicy, PluginBrand, PluginKind,
    PluginManifest, PluginSource, PluginTransport, SystemPromptPolicy,
};
use crate::storage::plugins::StoredPluginState;

#[derive(Debug, Clone)]
pub struct PluginRecord {
    pub manifest: PluginManifest,
    pub source: PluginSource,
    pub enabled: bool,
    pub pinned_hash: Option<String>,
    pub overrides_builtin: bool,
    pub plugin_dir: Option<PathBuf>,
}

pub struct PluginRegistry {
    builtins: HashMap<String, PluginManifest>,
    records: HashMap<String, PluginRecord>,
    egress_transports: HashMap<String, Arc<dyn EgressTransport>>,
    ingress_plugins: HashMap<String, Arc<dyn IngressPlugin>>,
}

static GLOBAL_EGRESS_PROFILES: RwLock<Option<HashMap<String, EgressProfile>>> = RwLock::new(None);
static GLOBAL_EGRESS_TRANSPORTS: RwLock<Option<HashMap<String, Arc<dyn EgressTransport>>>> =
    RwLock::new(None);
static GLOBAL_DISABLED_PLUGINS: RwLock<Option<HashSet<String>>> = RwLock::new(None);

fn egress_manifest(
    id: &str,
    name: &str,
    tagline: &str,
    icon: &str,
    accent: &str,
    profile: EgressProfile,
) -> PluginManifest {
    PluginManifest {
        schema_version: 2,
        id: id.to_string(),
        name: name.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        kind: PluginKind::Egress,
        transport: PluginTransport::Builtin,
        platforms: Vec::new(),
        brand: PluginBrand {
            short_tagline: tagline.to_string(),
            tagline: Some(tagline.to_string()),
            icon: Some(icon.to_string()),
            accent_color: Some(accent.to_string()),
        },
        entry: None,
        egress_profile: Some(profile),
        ingress_profile: None,
    }
}

fn ingress_manifest(
    id: &str,
    name: &str,
    tagline: &str,
    icon: &str,
    profile: IngressProfile,
) -> PluginManifest {
    PluginManifest {
        schema_version: 2,
        id: id.to_string(),
        name: name.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        kind: PluginKind::Ingress,
        transport: PluginTransport::Builtin,
        platforms: Vec::new(),
        brand: PluginBrand {
            short_tagline: tagline.to_string(),
            tagline: Some(tagline.to_string()),
            icon: Some(icon.to_string()),
            accent_color: None,
        },
        entry: None,
        egress_profile: None,
        ingress_profile: Some(profile),
    }
}

fn standard_egress_provides() -> EgressProvides {
    EgressProvides {
        streaming: true,
        tools: true,
        tool_choice: true,
        vision: false,
        thinking: true,
        encrypted_reasoning: false,
        json_schema: true,
        count_tokens: false,
        compact: false,
    }
}

pub fn builtin_default_manifests() -> Vec<PluginManifest> {
    vec![
        // --- 8 Builtin Egress Plugins ---
        egress_manifest(
            "trae_cn",
            "Trae CN",
            "字节 AI IDE（国内版）",
            "/icons/trae-cn.svg",
            "#10b981",
            EgressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                auth_methods: vec![
                    AuthMethod::LocalImport,
                    AuthMethod::WebviewOauth,
                    AuthMethod::TokenPaste,
                ],
                account_capabilities: EgressAccountCapabilities {
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: true,
                    claim_requires_client_switch: false,
                    supports_multi_instance: false,
                    concurrent_with_desktop_client: false,
                },
                provides: standard_egress_provides(),
                constraints: EgressConstraints::default(),
                declarative_http: None,
                default_models: Vec::new(),
            },
        ),
        egress_manifest(
            "workbuddy",
            "WorkBuddy",
            "腾讯 AI 桌面协作助手",
            "/icons/workbuddy.svg",
            "#14b8a6",
            EgressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                auth_methods: vec![
                    AuthMethod::LocalImport,
                    AuthMethod::WebviewOauth,
                    AuthMethod::TokenPaste,
                ],
                account_capabilities: EgressAccountCapabilities {
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: true,
                    claim_requires_client_switch: false,
                    supports_multi_instance: false,
                    concurrent_with_desktop_client: false,
                },
                provides: standard_egress_provides(),
                constraints: EgressConstraints {
                    system_prompt_policy: SystemPromptPolicy::SanitizeAgentOpeners,
                    ..Default::default()
                },
                declarative_http: None,
                default_models: Vec::new(),
            },
        ),
        egress_manifest(
            "qoder_cn",
            "Qoder CN",
            "智能 Agent 编程工作台",
            "/icons/qoder-cn.svg",
            "#f59e0b",
            EgressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                auth_methods: vec![
                    AuthMethod::LocalImport,
                    AuthMethod::WebviewOauth,
                    AuthMethod::TokenPaste,
                ],
                account_capabilities: EgressAccountCapabilities {
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: true,
                    claim_requires_client_switch: false,
                    supports_multi_instance: false,
                    concurrent_with_desktop_client: true,
                },
                provides: standard_egress_provides(),
                constraints: EgressConstraints::default(),
                declarative_http: None,
                default_models: Vec::new(),
            },
        ),
        egress_manifest(
            "minimax_code_cn",
            "MiniMax Code CN",
            "MiniMax Code 桌面版（国内版）",
            "/icons/minimax-code.png?v=2",
            "#0ea5e9",
            EgressProfile {
                wire_protocols: vec![WireProtocolKind::AnthropicMessages],
                auth_methods: vec![
                    AuthMethod::LocalImport,
                    AuthMethod::WebviewOauth,
                    AuthMethod::TokenPaste,
                ],
                account_capabilities: EgressAccountCapabilities {
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: true,
                    claim_requires_client_switch: false,
                    supports_multi_instance: false,
                    concurrent_with_desktop_client: false,
                },
                provides: EgressProvides {
                    encrypted_reasoning: true,
                    ..standard_egress_provides()
                },
                constraints: EgressConstraints::default(),
                declarative_http: None,
                default_models: Vec::new(),
            },
        ),
        egress_manifest(
            "lobsterai",
            "LobsterAI",
            "网易有道桌面 AI Agent",
            "/icons/lobsterai.png",
            "#f43f5e",
            EgressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                auth_methods: vec![
                    AuthMethod::LocalImport,
                    AuthMethod::WebviewOauth,
                    AuthMethod::TokenPaste,
                ],
                account_capabilities: EgressAccountCapabilities {
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: true,
                    claim_requires_client_switch: false,
                    supports_multi_instance: false,
                    concurrent_with_desktop_client: true,
                },
                provides: standard_egress_provides(),
                constraints: EgressConstraints::default(),
                declarative_http: None,
                default_models: Vec::new(),
            },
        ),
        egress_manifest(
            "doubaowork",
            "豆包工作",
            "字节跳动豆包工作桌面 Agent",
            "/icons/doubaowork.png",
            "#6366f1",
            EgressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                auth_methods: vec![
                    AuthMethod::LocalImport,
                    AuthMethod::WebviewOauth,
                    AuthMethod::TokenPaste,
                ],
                account_capabilities: EgressAccountCapabilities {
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: true,
                    claim_requires_client_switch: true,
                    supports_multi_instance: false,
                    concurrent_with_desktop_client: true,
                },
                provides: standard_egress_provides(),
                constraints: EgressConstraints::default(),
                declarative_http: None,
                default_models: Vec::new(),
            },
        ),
        egress_manifest(
            "codearts",
            "码道 CodeArts",
            "华为云 CodeArts 桌面 AI IDE",
            "/icons/codearts.svg",
            "#ef4444",
            EgressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                auth_methods: vec![
                    AuthMethod::LocalImport,
                    AuthMethod::WebviewOauth,
                    AuthMethod::TokenPaste,
                ],
                account_capabilities: EgressAccountCapabilities {
                    supports_quota_query: true,
                    supports_daily_claim: true,
                    supports_client_switch: cfg!(target_os = "macos"),
                    claim_requires_client_switch: false,
                    supports_multi_instance: false,
                    concurrent_with_desktop_client: true,
                },
                provides: standard_egress_provides(),
                constraints: EgressConstraints {
                    must_stream: true,
                    ..Default::default()
                },
                declarative_http: None,
                default_models: Vec::new(),
            },
        ),
        egress_manifest(
            "custom_api_key",
            "标准 API Key",
            "通用 OpenAI / Anthropic API Key 兜底",
            "/icons/qwitch.svg",
            "#71717a",
            EgressProfile {
                wire_protocols: vec![
                    WireProtocolKind::OpenaiChat,
                    WireProtocolKind::AnthropicMessages,
                    WireProtocolKind::OpenaiResponses,
                ],
                auth_methods: vec![AuthMethod::TokenPaste],
                account_capabilities: EgressAccountCapabilities {
                    concurrent_with_desktop_client: true,
                    ..Default::default()
                },
                provides: EgressProvides {
                    vision: true,
                    encrypted_reasoning: true,
                    ..standard_egress_provides()
                },
                constraints: EgressConstraints::default(),
                declarative_http: None,
                default_models: Vec::new(),
            },
        ),
        // --- 6 Builtin Ingress Plugins ---
        ingress_manifest(
            "dsh",
            "DeepSeek Harness",
            "DeepSeek 桌面/CLI Agent",
            "/icons/agent-dsh.png",
            IngressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                requires: IngressRequires {
                    streaming: true,
                    tools: true,
                    vision: false,
                    thinking: false,
                },
                optional: IngressOptional {
                    vision: true,
                    thinking: true,
                    tool_choice: true,
                    count_tokens: false,
                    compact: false,
                },
                quirks: IngressQuirks::default(),
                declarative_wiring: None,
            },
        ),
        ingress_manifest(
            "opencode",
            "OpenCode",
            "OpenCode CLI / Desktop",
            "/icons/agent-opencode.png",
            IngressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                requires: IngressRequires {
                    streaming: true,
                    tools: true,
                    vision: false,
                    thinking: false,
                },
                optional: IngressOptional {
                    vision: true,
                    thinking: true,
                    tool_choice: true,
                    count_tokens: false,
                    compact: false,
                },
                quirks: IngressQuirks::default(),
                declarative_wiring: None,
            },
        ),
        ingress_manifest(
            "codex_desktop",
            "Codex 桌面版",
            "OpenAI Codex / ChatGPT Desktop",
            "/icons/agent-codex.png",
            IngressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiResponses],
                requires: IngressRequires {
                    streaming: true,
                    tools: true,
                    vision: false,
                    thinking: false,
                },
                optional: IngressOptional {
                    vision: true,
                    thinking: true,
                    tool_choice: true,
                    count_tokens: false,
                    compact: true,
                },
                quirks: IngressQuirks {
                    supports_custom_base_url: true,
                    model_id_policy: ModelIdPolicy::Scoped,
                    requires_ws_426_fallback: true,
                    supports_own_auth_passthrough: true,
                },
                declarative_wiring: None,
            },
        ),
        ingress_manifest(
            "claude_desktop",
            "Claude 桌面版",
            "Anthropic Claude Desktop (3p)",
            "/icons/agent-claude.png",
            IngressProfile {
                wire_protocols: vec![WireProtocolKind::AnthropicMessages],
                requires: IngressRequires {
                    streaming: true,
                    tools: true,
                    vision: false,
                    thinking: false,
                },
                optional: IngressOptional {
                    vision: true,
                    thinking: true,
                    tool_choice: true,
                    count_tokens: true,
                    compact: false,
                },
                quirks: IngressQuirks {
                    supports_custom_base_url: true,
                    model_id_policy: ModelIdPolicy::AnthropicWhitelistAlias,
                    requires_ws_426_fallback: false,
                    supports_own_auth_passthrough: false,
                },
                declarative_wiring: None,
            },
        ),
        ingress_manifest(
            "mimo",
            "MiMo Code",
            "小米 MiMo Code 编程助手",
            "/icons/agent-mimo.png",
            IngressProfile {
                wire_protocols: vec![WireProtocolKind::OpenaiChat],
                requires: IngressRequires {
                    streaming: true,
                    tools: true,
                    vision: false,
                    thinking: false,
                },
                optional: IngressOptional {
                    vision: true,
                    thinking: true,
                    tool_choice: true,
                    count_tokens: false,
                    compact: false,
                },
                quirks: IngressQuirks::default(),
                declarative_wiring: None,
            },
        ),
        ingress_manifest(
            "zcode",
            "ZCode",
            "智谱 ZCode 编程助手",
            "/icons/agent-zcode.png",
            IngressProfile {
                wire_protocols: vec![WireProtocolKind::AnthropicMessages],
                requires: IngressRequires {
                    streaming: true,
                    tools: true,
                    vision: false,
                    thinking: false,
                },
                optional: IngressOptional {
                    vision: true,
                    thinking: true,
                    tool_choice: true,
                    count_tokens: false,
                    compact: false,
                },
                quirks: IngressQuirks::default(),
                declarative_wiring: None,
            },
        ),
    ]
}

impl PluginRegistry {
    pub fn new_with_builtins() -> Self {
        let mut builtins = HashMap::new();
        let mut records = HashMap::new();
        for manifest in builtin_default_manifests() {
            let id = manifest.id.clone();
            builtins.insert(id.clone(), manifest.clone());
            records.insert(
                id,
                PluginRecord {
                    manifest,
                    source: PluginSource::Builtin,
                    enabled: true,
                    pinned_hash: None,
                    overrides_builtin: false,
                    plugin_dir: None,
                },
            );
        }
        let egress_transports = create_builtin_egress_transports();
        let ingress_plugins = create_builtin_ingress_plugins();
        Self {
            builtins,
            records,
            egress_transports,
            ingress_plugins,
        }
    }

    pub fn is_enabled(&self, plugin_id: &str) -> bool {
        self.records.get(plugin_id).is_none_or(|r| r.enabled)
    }

    pub fn register_egress_transport(&mut self, transport: Arc<dyn EgressTransport>) {
        self.egress_transports
            .insert(transport.id().to_string(), transport);
    }

    pub fn register_ingress_plugin(&mut self, plugin: Arc<dyn IngressPlugin>) {
        self.ingress_plugins.insert(plugin.id().to_string(), plugin);
    }

    pub fn get_egress_transport(&self, tool_id: &str) -> Option<Arc<dyn EgressTransport>> {
        let key = canonical_tool_lookup_key(tool_id);
        if !self.is_enabled(key) && !self.is_enabled(tool_id) {
            return None;
        }
        self.egress_transports
            .get(tool_id)
            .or_else(|| self.egress_transports.get(key))
            .cloned()
    }

    pub fn get_ingress_plugin(&self, agent_id: &str) -> Option<Arc<dyn IngressPlugin>> {
        if !self.is_enabled(agent_id) {
            return None;
        }
        self.ingress_plugins.get(agent_id).cloned()
    }

    pub fn list_enabled_ingress_plugins(&self) -> Vec<Arc<dyn IngressPlugin>> {
        let mut out: Vec<Arc<dyn IngressPlugin>> = self
            .ingress_plugins
            .values()
            .filter(|p| self.is_enabled(p.id()))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.id().cmp(b.id()));
        out
    }

    pub fn apply_stored_states(&mut self, states: &[StoredPluginState]) {
        for st in states {
            if let Some(rec) = self.records.get_mut(&st.plugin_id) {
                rec.enabled = st.enabled;
            }
        }
    }

    pub fn register_external(
        &mut self,
        manifest: PluginManifest,
        pinned_hash: String,
        dir: PathBuf,
        enabled: bool,
    ) -> Result<(), String> {
        if !crate::plugins::manifest::is_valid_plugin_id(&manifest.id) {
            return Err(format!("无效插件 ID: {}", manifest.id));
        }
        let overrides_builtin = self.builtins.contains_key(&manifest.id);
        let id = manifest.id.clone();
        self.records.insert(
            id,
            PluginRecord {
                manifest,
                source: PluginSource::External,
                enabled,
                pinned_hash: Some(pinned_hash),
                overrides_builtin,
                plugin_dir: Some(dir),
            },
        );
        Ok(())
    }

    /// 卸载外部插件；若存在同 ID 官方内置插件，自动无损恢复内置版本。
    pub fn remove_external(&mut self, plugin_id: &str) -> bool {
        let is_ext = self
            .records
            .get(plugin_id)
            .is_some_and(|r| r.source == PluginSource::External);
        if !is_ext {
            return false;
        }
        if let Some(builtin) = self.builtins.get(plugin_id) {
            self.records.insert(
                plugin_id.to_string(),
                PluginRecord {
                    manifest: builtin.clone(),
                    source: PluginSource::Builtin,
                    enabled: true,
                    pinned_hash: None,
                    overrides_builtin: false,
                    plugin_dir: None,
                },
            );
        } else {
            self.records.remove(plugin_id);
        }
        true
    }

    pub fn set_enabled(&mut self, plugin_id: &str, enabled: bool) -> Result<(), String> {
        let Some(rec) = self.records.get_mut(plugin_id) else {
            return Err(format!("找不到插件: {plugin_id}"));
        };
        rec.enabled = enabled;
        Ok(())
    }

    pub fn get(&self, plugin_id: &str) -> Option<&PluginRecord> {
        self.records.get(plugin_id)
    }

    pub fn list_all(&self) -> Vec<&PluginRecord> {
        let mut out: Vec<&PluginRecord> = self.records.values().collect();
        out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        out
    }

    pub fn list_enabled_egress(&self) -> Vec<&PluginRecord> {
        let mut out: Vec<&PluginRecord> = self
            .records
            .values()
            .filter(|r| {
                r.enabled && matches!(r.manifest.kind, PluginKind::Egress | PluginKind::Hybrid)
            })
            .collect();
        out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        out
    }

    pub fn list_enabled_ingress(&self) -> Vec<&PluginRecord> {
        let mut out: Vec<&PluginRecord> = self
            .records
            .values()
            .filter(|r| {
                r.enabled && matches!(r.manifest.kind, PluginKind::Ingress | PluginKind::Hybrid)
            })
            .collect();
        out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        out
    }

    pub fn egress_profile(&self, tool_id: &str) -> Option<&EgressProfile> {
        self.records
            .get(tool_id)
            .filter(|r| r.enabled)
            .and_then(|r| r.manifest.egress_profile.as_ref())
    }

    pub fn ingress_profile(&self, agent_id: &str) -> Option<&IngressProfile> {
        self.records
            .get(agent_id)
            .filter(|r| r.enabled)
            .and_then(|r| r.manifest.ingress_profile.as_ref())
    }

    pub fn egress_ids(&self) -> HashSet<String> {
        self.records
            .values()
            .filter(|r| matches!(r.manifest.kind, PluginKind::Egress | PluginKind::Hybrid))
            .map(|r| r.manifest.id.clone())
            .collect()
    }

    pub fn concurrent_with_desktop_client(&self, tool_id: &str) -> bool {
        self.egress_profile(tool_id)
            .is_some_and(|p| p.account_capabilities.concurrent_with_desktop_client)
    }
}

fn canonical_tool_lookup_key(tool_id: &str) -> &str {
    match tool_id {
        "qoder" => "qoder_cn",
        "trae" => "trae_cn",
        other => other,
    }
}

/// 将运行期可变 `PluginRegistry` 的 Egress Profile 与 Transport 快照同步至全局读表，
/// 供 `split_scoped_model_id`、`scheduler` 与 `gateway` 高频路径零锁阻塞读取。
pub fn sync_global_registry_snapshot(registry: &PluginRegistry) {
    let mut profile_map = HashMap::new();
    let mut transport_map = HashMap::new();
    let mut disabled_set = HashSet::new();
    for rec in registry.records.values() {
        if !rec.enabled {
            disabled_set.insert(rec.manifest.id.clone());
            continue;
        }
        if matches!(rec.manifest.kind, PluginKind::Egress | PluginKind::Hybrid) {
            if let Some(profile) = &rec.manifest.egress_profile {
                profile_map.insert(rec.manifest.id.clone(), profile.clone());
            }
            if let Some(tr) = registry.egress_transports.get(&rec.manifest.id) {
                transport_map.insert(rec.manifest.id.clone(), Arc::clone(tr));
            }
        }
    }
    {
        let mut guard = GLOBAL_EGRESS_PROFILES
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(profile_map);
    }
    {
        let mut guard = GLOBAL_EGRESS_TRANSPORTS
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(transport_map);
    }
    {
        let mut guard = GLOBAL_DISABLED_PLUGINS
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(disabled_set);
    }
}

pub fn global_is_plugin_enabled(plugin_id: &str) -> bool {
    let key = canonical_tool_lookup_key(plugin_id);
    if let Ok(guard) = GLOBAL_DISABLED_PLUGINS.read() {
        if let Some(set) = guard.as_ref() {
            return !set.contains(plugin_id) && !set.contains(key);
        }
    }
    true
}

pub fn global_egress_transport(tool_id: &str) -> Option<Arc<dyn EgressTransport>> {
    let key = canonical_tool_lookup_key(tool_id);
    if let Ok(guard) = GLOBAL_EGRESS_TRANSPORTS.read() {
        if let Some(map) = guard.as_ref() {
            return map.get(tool_id).or_else(|| map.get(key)).cloned();
        }
    }
    create_builtin_egress_transports().get(key).cloned()
}

fn builtin_egress_fallback_map() -> HashMap<String, EgressProfile> {
    let mut map = HashMap::new();
    for m in builtin_default_manifests() {
        if matches!(m.kind, PluginKind::Egress | PluginKind::Hybrid) {
            if let Some(p) = m.egress_profile {
                map.insert(m.id, p);
            }
        }
    }
    #[cfg(test)]
    {
        map.insert(
            "no_tools_test_egress".to_string(),
            EgressProfile {
                provides: EgressProvides {
                    tools: false,
                    tool_choice: false,
                    ..standard_egress_provides()
                },
                ..Default::default()
            },
        );
    }
    map
}

pub fn is_registered_egress_id(id: &str) -> bool {
    let key = canonical_tool_lookup_key(id);
    if let Ok(guard) = GLOBAL_EGRESS_PROFILES.read() {
        if let Some(map) = guard.as_ref() {
            return map.contains_key(key) || map.contains_key(id);
        }
    }
    matches!(
        key,
        "workbuddy"
            | "minimax_code_cn"
            | "qoder_cn"
            | "trae_cn"
            | "lobsterai"
            | "doubaowork"
            | "codearts"
            | "custom_api_key"
    )
}

pub fn global_egress_profile(tool_id: &str) -> Option<EgressProfile> {
    let key = canonical_tool_lookup_key(tool_id);
    if let Ok(guard) = GLOBAL_EGRESS_PROFILES.read() {
        if let Some(map) = guard.as_ref() {
            return map.get(tool_id).or_else(|| map.get(key)).cloned();
        }
    }
    builtin_egress_fallback_map().get(key).cloned()
}

pub fn global_egress_concurrent_with_desktop_client(tool_id: &str) -> bool {
    global_egress_profile(tool_id)
        .is_some_and(|p| p.account_capabilities.concurrent_with_desktop_client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_override_and_restore_builtin_works_cleanly() {
        let mut reg = PluginRegistry::new_with_builtins();
        assert!(reg.get("trae_cn").is_some());
        assert_eq!(reg.get("trae_cn").unwrap().source, PluginSource::Builtin);

        let mut ext = builtin_default_manifests()
            .into_iter()
            .find(|m| m.id == "trae_cn")
            .unwrap();
        ext.version = "9.9.9".to_string();
        ext.transport = PluginTransport::Subprocess;

        reg.register_external(ext, "deadbeef".into(), PathBuf::from("/tmp/trae_cn"), true)
            .unwrap();
        let overridden = reg.get("trae_cn").unwrap();
        assert_eq!(overridden.source, PluginSource::External);
        assert!(overridden.overrides_builtin);
        assert_eq!(overridden.manifest.version, "9.9.9");

        // 卸载覆盖插件后自动恢复内置版本
        assert!(reg.remove_external("trae_cn"));
        let restored = reg.get("trae_cn").unwrap();
        assert_eq!(restored.source, PluginSource::Builtin);
        assert!(!restored.overrides_builtin);
    }

    #[test]
    fn disabling_plugin_hides_it_from_enabled_lists_and_transports() {
        let mut reg = PluginRegistry::new_with_builtins();
        assert!(reg.is_enabled("trae_cn"));
        assert!(reg.get_egress_transport("trae_cn").is_some());
        assert!(reg.get_ingress_plugin("opencode").is_some());

        reg.set_enabled("trae_cn", false).unwrap();
        assert!(!reg.is_enabled("trae_cn"));
        assert!(reg.get_egress_transport("trae_cn").is_none());
        assert!(!reg
            .list_enabled_egress()
            .iter()
            .any(|r| r.manifest.id == "trae_cn"));

        reg.set_enabled("opencode", false).unwrap();
        assert!(reg.get_ingress_plugin("opencode").is_none());
        assert!(!reg
            .list_enabled_ingress_plugins()
            .iter()
            .any(|p| p.id() == "opencode"));
    }
}
