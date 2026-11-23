use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PoolStrategy {
    QuotaFirst,
    Drain,
    Order,
    RoundRobin,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AffinityMode {
    AutoCache,
    Session,
    Turn,
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamChannel {
    RustNative,
    OpencodeBridge,
    ApiKey,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TargetAgentId {
    CodexDesktop,
    ClaudeDesktop,
    Dsh,
    Opencode,
}

impl TargetAgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodexDesktop => "codex_desktop",
            Self::ClaudeDesktop => "claude_desktop",
            Self::Dsh => "dsh",
            Self::Opencode => "opencode",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "codex_desktop" => Some(Self::CodexDesktop),
            "claude_desktop" => Some(Self::ClaudeDesktop),
            "dsh" => Some(Self::Dsh),
            "opencode" => Some(Self::Opencode),
            _ => None,
        }
    }

    pub fn all() -> [Self; 4] {
        [
            Self::ClaudeDesktop,
            Self::Opencode,
            Self::Dsh,
            Self::CodexDesktop,
        ]
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentBindingStatus {
    Bound,
    Drifted,
    Unbound,
    NeedsRestart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WireProtocolKind {
    OpenaiChat,
    AnthropicMessages,
    OpenaiResponses,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodeArtsAuthMode {
    ClientReadonly,
    IsolatedOauth,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberTier {
    #[serde(rename = "t1_primary")]
    T1Primary,
    #[serde(rename = "t2_backup")]
    T2Backup,
    #[serde(rename = "t3_fallback")]
    T3Fallback,
}

impl MemberTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::T1Primary => "t1_primary",
            Self::T2Backup => "t2_backup",
            Self::T3Fallback => "t3_fallback",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::T1Primary => "T1 主力",
            Self::T2Backup => "T2 备选",
            Self::T3Fallback => "T3 兜底",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::T1Primary => 1,
            Self::T2Backup => 2,
            Self::T3Fallback => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolModelPolicy {
    pub model_id: String,
    pub enabled: bool,
    pub fallback_model_id: Option<String>,
    pub claude_alias_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolMember {
    pub id: String,
    pub account_id: String,
    pub tool_id: String,
    pub label: String,
    pub uid_masked: String,
    pub supported_models: Vec<String>,
    pub tier: MemberTier,
    pub channel: UpstreamChannel,
    pub enabled: bool,
    pub priority: usize,
    pub quota_percent: u8,
    pub quota_text: String,
    pub cooldown_until_ms: Option<i64>,
    pub is_active_on_client: Option<bool>,
    pub account_status: Option<String>,
    pub code_arts_mode: Option<CodeArtsAuthMode>,
    pub last_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPool {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub scene_tag: String,
    pub enabled: bool,
    pub strategy: PoolStrategy,
    pub affinity_mode: AffinityMode,
    pub skip_active_client_account: bool,
    pub cooldown_seconds: u64,
    #[serde(rename = "autoRefreshOn401")]
    pub auto_refresh_on_401: bool,
    pub fallback_api_key_enabled: bool,
    pub model_policies: Vec<PoolModelPolicy>,
    pub members: Vec<PoolMember>,
    pub today_requests: u64,
    pub today_failovers: u64,
    pub cache_hit_rate: u8,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentBindingOptions {
    pub claude_isolated_twin: Option<bool>,
    pub codex_preserve_table: Option<bool>,
    pub dsh_dual_creds_written: Option<bool>,
    pub opencode_schema_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBinding {
    pub agent_id: TargetAgentId,
    pub name: String,
    pub subtitle: String,
    pub protocol: WireProtocolKind,
    pub config_path: String,
    pub installed: bool,
    pub version: String,
    pub status: AgentBindingStatus,
    pub mounted_pool_ids: Vec<String>,
    pub default_model_key: Option<String>,
    pub drift_reason: Option<String>,
    pub proxy_warning: Option<String>,
    pub restart_hint: Option<String>,
    pub last_hit_at: Option<String>,
    pub options: AgentBindingOptions,
    pub before_snippet: String,
    pub after_snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExposedModel {
    pub key: String,
    pub pool_id: String,
    pub pool_name: String,
    pub model_id: String,
    pub display_name: String,
    pub wire_model_id: String,
    pub is_claude_disguised: bool,
    pub account_count: usize,
    pub healthy_account_count: usize,
    pub tier_summary: String,
    pub is_default: bool,
    pub collision: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RoutingAttemptOutcome {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "429_cooldown")]
    Cooldown429,
    #[serde(rename = "401_refreshed")]
    Refreshed401,
    #[serde(rename = "skipped_active")]
    SkippedActive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingAttempt {
    pub account_label: String,
    pub tool_id: String,
    pub model: String,
    pub tier: Option<MemberTier>,
    pub outcome: RoutingAttemptOutcome,
    pub latency_ms: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingTraceStatus {
    Success,
    FailoverSuccess,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingTraceItem {
    pub id: String,
    pub timestamp: String,
    pub agent_id: TargetAgentId,
    pub pool_id: String,
    pub pool_name: String,
    pub incoming_protocol: WireProtocolKind,
    pub upstream_protocol: WireProtocolKind,
    pub requested_model: String,
    pub final_account_label: String,
    pub final_tool_id: String,
    pub final_model: String,
    pub status: RoutingTraceStatus,
    pub cache_affinity_hit: bool,
    pub ttft_ms: u64,
    pub total_tokens: u64,
    pub attempts: Vec<RoutingAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayRuntimeState {
    pub running: bool,
    pub host: String,
    pub port: u16,
    pub bridge_runtime: String,
    pub bridge_version: String,
    pub global_proxy_detected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamAccountCandidate {
    pub account_id: String,
    pub tool_id: String,
    pub label: String,
    pub uid_masked: String,
    pub channel: UpstreamChannel,
    pub default_models: Vec<String>,
    pub default_tier: MemberTier,
    pub quota_percent: u8,
    pub quota_text: String,
    pub is_active_on_client: Option<bool>,
    pub account_status: String,
    pub code_arts_mode: Option<CodeArtsAuthMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePoolMemberInput {
    pub account_id: String,
    pub supported_models: Vec<String>,
    pub tier: MemberTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePoolInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    pub scene_tag: String,
    pub strategy: PoolStrategy,
    pub affinity_mode: AffinityMode,
    pub skip_active_client_account: bool,
    pub cooldown_seconds: u64,
    #[serde(rename = "autoRefreshOn401")]
    pub auto_refresh_on_401: bool,
    pub fallback_api_key_enabled: bool,
    pub members: Vec<CreatePoolMemberInput>,
    pub enabled_model_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePoolPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub strategy: Option<PoolStrategy>,
    pub affinity_mode: Option<AffinityMode>,
    pub skip_active_client_account: Option<bool>,
    pub cooldown_seconds: Option<u64>,
    #[serde(rename = "autoRefreshOn401")]
    pub auto_refresh_on_401: Option<bool>,
    pub fallback_api_key_enabled: Option<bool>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SimulationFailureMode {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "429_first")]
    First429,
    #[serde(rename = "401_refresh")]
    Refresh401,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPoolSimulationInput {
    pub agent_id: TargetAgentId,
    pub pool_id: String,
    pub model_id: String,
    pub prompt: String,
    pub failure_mode: SimulationFailureMode,
    pub session_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPoolSimulationResult {
    pub trace: RoutingTraceItem,
    pub cooled_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolWorkspaceState {
    pub gateway: GatewayRuntimeState,
    pub pools: Vec<AccountPool>,
    pub candidates: Vec<UpstreamAccountCandidate>,
    pub agent_bindings: Vec<AgentBinding>,
    pub traces: Vec<RoutingTraceItem>,
}

pub fn is_anthropic_model_name(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();
    ["claude", "sonnet", "opus", "haiku", "fable", "mythos", "anthropic"]
        .iter()
        .any(|kw| lower.contains(kw))
}

pub fn build_claude_model_alias(pool_slug: &str, model_id: &str) -> String {
    if is_anthropic_model_name(model_id) {
        return model_id.to_string();
    }
    let seed = format!("{pool_slug}/{model_id}");
    let mut hash: u32 = 2166136261;
    for b in seed.as_bytes() {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    format!("mythos-qwitch-{hash:06x}")[..20].to_string()
}

pub fn default_channel_for_tool(tool_id: &str) -> UpstreamChannel {
    match tool_id {
        "minimax_code_cn" => UpstreamChannel::RustNative,
        "custom_api_key" => UpstreamChannel::ApiKey,
        _ => UpstreamChannel::OpencodeBridge,
    }
}

pub fn default_models_for_tool(tool_id: &str) -> Vec<String> {
    match tool_id {
        "workbuddy" => vec!["claude-sonnet-4.6".into(), "claude-opus-4.6".into()],
        "minimax_code_cn" => vec!["MiniMax-M2.5".into(), "MiniMax-M2.5-highspeed".into()],
        "qoder_cn" => vec!["claude-sonnet-4.6".into(), "qwen3-coder-plus".into()],
        "trae_cn" => vec!["claude-sonnet-4.6".into(), "deepseek-v4".into()],
        "lobsterai" => vec!["lobster-coder-pro".into(), "deepseek-v4".into()],
        "doubaowork" => vec!["doubao-seed-2.1-pro".into()],
        "codearts" => vec!["pangu-coder-pro".into()],
        "custom_api_key" => vec![
            "claude-sonnet-4.6".into(),
            "deepseek-v4".into(),
            "deepseek-chat".into(),
        ],
        _ => vec!["claude-sonnet-4.6".into()],
    }
}

pub fn upstream_model_protocol(tool_id: &str, model_id: &str) -> WireProtocolKind {
    if tool_id == "minimax_code_cn" || is_anthropic_model_name(model_id) && tool_id == "custom_api_key" {
        WireProtocolKind::AnthropicMessages
    } else {
        WireProtocolKind::OpenaiChat
    }
}

pub fn sync_pool_model_policies(
    slug: &str,
    members: &[PoolMember],
    existing: &[PoolModelPolicy],
    enabled_override: Option<&[String]>,
) -> Vec<PoolModelPolicy> {
    let existing_map: HashMap<&str, &PoolModelPolicy> = existing
        .iter()
        .map(|p| (p.model_id.as_str(), p))
        .collect();
    let enabled_set: Option<HashSet<&str>> =
        enabled_override.map(|ids| ids.iter().map(|s| s.as_str()).collect());

    let mut unique_ids = Vec::new();
    let mut seen = HashSet::new();
    for m in members {
        for model_id in &m.supported_models {
            if seen.insert(model_id.clone()) {
                unique_ids.push(model_id.clone());
            }
        }
    }

    unique_ids
        .into_iter()
        .map(|model_id| {
            if let Some(prev) = existing_map.get(model_id.as_str()) {
                let mut cloned = (*prev).clone();
                if cloned.claude_alias_id.is_none() {
                    cloned.claude_alias_id = Some(build_claude_model_alias(slug, &model_id));
                }
                cloned
            } else {
                let enabled = enabled_set
                    .as_ref()
                    .map(|set| set.contains(model_id.as_str()))
                    .unwrap_or(true);
                PoolModelPolicy {
                    claude_alias_id: Some(build_claude_model_alias(slug, &model_id)),
                    model_id,
                    enabled,
                    fallback_model_id: None,
                }
            }
        })
        .collect()
}

pub fn make_default_model_key(pool_id: &str, model_id: &str) -> String {
    format!("{pool_id}::{model_id}")
}

pub fn compute_pool_serving_accounts(
    pool: &AccountPool,
    model_id: &str,
    now_ms: i64,
) -> (usize, usize, String) {
    let serving: Vec<&PoolMember> = pool
        .members
        .iter()
        .filter(|m| m.supported_models.iter().any(|id| id == model_id))
        .collect();

    let mut healthy = 0usize;
    let mut t1 = 0usize;
    let mut t2 = 0usize;
    let mut t3 = 0usize;
    for m in &serving {
        match m.tier {
            MemberTier::T1Primary => t1 += 1,
            MemberTier::T2Backup => t2 += 1,
            MemberTier::T3Fallback => t3 += 1,
        }
        let cooled = m.cooldown_until_ms.is_some_and(|until| until > now_ms);
        let skipped = pool.skip_active_client_account && m.is_active_on_client.unwrap_or(false);
        let paused = matches!(
            m.account_status.as_deref(),
            Some("risk_paused") | Some("expired") | Some("disabled")
        );
        let readonly = m.code_arts_mode == Some(CodeArtsAuthMode::ClientReadonly);
        if m.enabled && !paused && !readonly && !cooled && !skipped {
            healthy += 1;
        }
    }

    let mut parts = Vec::new();
    if t1 > 0 {
        parts.push(format!("T1×{t1}"));
    }
    if t2 > 0 {
        parts.push(format!("T2×{t2}"));
    }
    if t3 > 0 {
        parts.push(format!("T3×{t3}"));
    }
    let tier_summary = if parts.is_empty() {
        "无成员".to_string()
    } else {
        parts.join(" · ")
    };
    (serving.len(), healthy, tier_summary)
}

pub fn compute_agent_exposed_models(
    agent_id: TargetAgentId,
    mounted_pool_ids: &[String],
    default_model_key: Option<&str>,
    all_pools: &[AccountPool],
    now_ms: i64,
) -> Vec<AgentExposedModel> {
    struct Raw<'a> {
        pool: &'a AccountPool,
        policy: &'a PoolModelPolicy,
        total: usize,
        healthy: usize,
        tier_summary: String,
    }

    let mut raw_items = Vec::new();
    for pid in mounted_pool_ids {
        let Some(pool) = all_pools.iter().find(|p| &p.id == pid && p.enabled) else {
            continue;
        };
        for policy in &pool.model_policies {
            if !policy.enabled {
                continue;
            }
            let (total, healthy, tier_summary) =
                compute_pool_serving_accounts(pool, &policy.model_id, now_ms);
            if total == 0 {
                continue;
            }
            raw_items.push(Raw {
                pool,
                policy,
                total,
                healthy,
                tier_summary,
            });
        }
    }

    let mut model_counts: HashMap<&str, usize> = HashMap::new();
    for item in &raw_items {
        *model_counts.entry(item.policy.model_id.as_str()).or_insert(0) += 1;
    }

    let keys: Vec<String> = raw_items
        .iter()
        .map(|item| make_default_model_key(&item.pool.id, &item.policy.model_id))
        .collect();
    let has_explicit_default =
        default_model_key.is_some_and(|dk| keys.iter().any(|k| k == dk));

    raw_items
        .into_iter()
        .enumerate()
        .map(|(idx, item)| {
            let key = make_default_model_key(&item.pool.id, &item.policy.model_id);
            let collision = model_counts
                .get(item.policy.model_id.as_str())
                .copied()
                .unwrap_or(0)
                > 1;
            let display_name = if collision {
                format!("{} · {}", item.policy.model_id, item.pool.name)
            } else {
                item.policy.model_id.clone()
            };
            let wire_model_id = if agent_id == TargetAgentId::ClaudeDesktop {
                item.policy
                    .claude_alias_id
                    .clone()
                    .unwrap_or_else(|| build_claude_model_alias(&item.pool.slug, &item.policy.model_id))
            } else {
                format!("{}/{}", item.pool.slug, item.policy.model_id)
            };
            let is_claude_disguised =
                agent_id == TargetAgentId::ClaudeDesktop && wire_model_id != item.policy.model_id;
            let is_default = if has_explicit_default {
                default_model_key == Some(key.as_str())
            } else {
                idx == 0
            };
            AgentExposedModel {
                key,
                pool_id: item.pool.id.clone(),
                pool_name: item.pool.name.clone(),
                model_id: item.policy.model_id.clone(),
                display_name,
                wire_model_id,
                is_claude_disguised,
                account_count: item.total,
                healthy_account_count: item.healthy,
                tier_summary: item.tier_summary,
                is_default,
                collision,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_alias_preserves_anthropic_names_and_disguises_others() {
        assert_eq!(
            build_claude_model_alias("coding-main", "claude-sonnet-4.6"),
            "claude-sonnet-4.6"
        );
        let disguised = build_claude_model_alias("coding-main", "deepseek-v4");
        assert!(disguised.starts_with("mythos-qwitch-"));
        assert_eq!(disguised.len(), 20);
    }

    #[test]
    fn member_tier_serde_matches_frontend_strings() {
        assert_eq!(
            serde_json::to_string(&MemberTier::T1Primary).unwrap(),
            r#""t1_primary""#
        );
        assert_eq!(
            serde_json::to_string(&MemberTier::T2Backup).unwrap(),
            r#""t2_backup""#
        );
        assert_eq!(
            serde_json::to_string(&MemberTier::T3Fallback).unwrap(),
            r#""t3_fallback""#
        );
    }
}
