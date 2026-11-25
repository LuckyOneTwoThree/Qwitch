use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

/// 实时探测得到的 `标准模型 ID -> 上游真实 wire ID` 注册表。
///
/// 背景：上游真实模型 ID 往往是大小写敏感的（如 `DeepSeek-V4-Pro`），
/// 而网关内部以归一化后的标准 ID（`deepseek-v4-pro`）做调度。
/// 静态映射表只覆盖已知模型，静态目录之外的实时模型一旦被归一化，
/// 还原出来的 wire ID 就与上游不一致 —— 请求会被上游拒绝或静默降级。
///
/// 这里用进程内注册表承接实时探测结果：探测成功即登记，
/// 网关解析 wire ID 时「静态表优先、未命中再查注册表」。
static LIVE_WIRE_IDS: RwLock<Option<HashMap<(String, String), String>>> = RwLock::new(None);

/// 工具 ID 别名归一：注册与查询使用同一套 key 规则。
fn wire_registry_tool_key(tool_id: &str) -> &str {
    match tool_id {
        "qoder" => "qoder_cn",
        other => other,
    }
}

/// 登记一次实时探测得到的模型 wire ID。
///
/// 仅登记 `source == "live"` 的条目：静态兜底目录里的 wire ID 是猜测值，
/// 登记进去反而会让网关优先使用猜测值而不是上游真实值。
pub fn register_live_wire_ids(tool_id: &str, models: &[UpstreamModelOption]) {
    let key = wire_registry_tool_key(tool_id).to_string();
    let mut guard = match LIVE_WIRE_IDS.write() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let map = guard.get_or_insert_with(HashMap::new);
    for model in models {
        if model.source != "live" {
            continue;
        }
        let wire = model.vendor_wire_id.trim();
        if wire.is_empty() {
            continue;
        }
        map.insert((key.clone(), model.id.clone()), wire.to_string());
    }
}

/// 查询实时探测登记的 wire ID（不存在则返回 `None`）。
pub fn live_wire_model_id(tool_id: &str, canonical_model_id: &str) -> Option<String> {
    let key = wire_registry_tool_key(tool_id).to_string();
    let guard = LIVE_WIRE_IDS.read().ok()?;
    let map = guard.as_ref()?;
    map.get(&(key, canonical_model_id.to_string())).cloned()
}

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
    Mimo,
    Zcode,
}

impl TargetAgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodexDesktop => "codex_desktop",
            Self::ClaudeDesktop => "claude_desktop",
            Self::Dsh => "dsh",
            Self::Opencode => "opencode",
            Self::Mimo => "mimo",
            Self::Zcode => "zcode",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "codex_desktop" => Some(Self::CodexDesktop),
            "claude_desktop" => Some(Self::ClaudeDesktop),
            "dsh" => Some(Self::Dsh),
            "opencode" => Some(Self::Opencode),
            "mimo" => Some(Self::Mimo),
            "zcode" => Some(Self::Zcode),
            _ => None,
        }
    }

    pub fn all() -> [Self; 6] {
        [
            Self::Dsh,
            Self::Opencode,
            Self::CodexDesktop,
            Self::ClaudeDesktop,
            Self::Mimo,
            Self::Zcode,
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
    /// 目标应用当前是否在运行（用于 UI 显示「启动 / 重启」）
    pub running: bool,
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
    #[serde(rename = "auth_expired")]
    AuthExpired,
    #[serde(rename = "skipped_active")]
    SkippedActive,
    /// 上游返回了既非 429、也非鉴权失效的真实错误（HTTP 4xx/5xx）或请求未到达。
    /// 此前这类失败被错误地贴上 `Cooldown429` 标签，掩盖了诸如「端点 404 / 协议不匹配」
    /// 的真实原因，排查时被误导为限流。现单独成类，detail 里保留真实状态码与响应片段。
    #[serde(rename = "http_error")]
    HttpError,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamModelOption {
    /// 跨工具统一归一化后的标准模型 ID（例如 "deepseek-v4-pro", "glm-5.3"）
    pub id: String,
    /// 人类可读的标准展示名（例如 "DeepSeek-V4-Pro", "GLM-5.3"）
    pub display_name: String,
    /// 该上游工具实际发请求时使用的底层模型 ID（例如 Qoder CN 为 "dmodel", Trae CN 为 "DeepSeek-V4-Pro"）
    pub vendor_wire_id: String,
    /// 积分消耗倍率（如 Some(0.5), Some(0.0)），未知时为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_multiplier: Option<f64>,
    /// 是否为 0 积分免费模型
    #[serde(default)]
    pub is_free: bool,
    /// 上下文窗口（tokens）；未知为 None（不猜）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// 单次最大输出 tokens；未知为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// 模型家族分类（"DeepSeek" | "GLM" | "Kimi" | "Qwen" | "MiniMax" | "Doubao" | "Hunyuan" | "Step" | "MiMo" | "Pangu" | "Claude" | "GPT" | "Other"）
    pub family: String,
    /// 数据来源（"live" 表示实时接口探测，"catalog" 表示真实协议目录兜底）
    pub source: String,
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
    #[serde(default)]
    pub model_options: Vec<UpstreamModelOption>,
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

pub fn split_scoped_model_id(raw: &str) -> (Option<&str>, &str) {
    if let Some((prefix, rest)) = raw.trim().split_once('/') {
        if matches!(
            prefix,
            "workbuddy"
                | "minimax_code_cn"
                | "qoder_cn"
                | "qoder"
                | "trae_cn"
                | "lobsterai"
                | "doubaowork"
                | "codearts"
                | "custom_api_key"
        ) && !rest.trim().is_empty()
        {
            return (Some(prefix), rest.trim());
        }
    }
    (None, raw.trim())
}

pub fn format_scoped_model_id(tool_id: &str, model_id: &str) -> String {
    let (_, bare) = split_scoped_model_id(model_id);
    format!("{tool_id}/{bare}")
}

pub fn tool_short_display_name(tool_id: &str) -> &'static str {
    match tool_id {
        "workbuddy" => "WorkBuddy",
        "minimax_code_cn" => "MiniMax",
        "qoder_cn" | "qoder" => "Qoder CN",
        "trae_cn" => "Trae CN",
        "lobsterai" => "有道龙虾",
        "doubaowork" => "豆包工作",
        "codearts" => "码道 CodeArts",
        "custom_api_key" => "标准 API",
        _ => "订阅工具",
    }
}

pub fn is_anthropic_model_name(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();
    [
        "claude",
        "sonnet",
        "opus",
        "haiku",
        "fable",
        "mythos",
        "anthropic",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

pub fn build_claude_model_alias(pool_slug: &str, model_id: &str) -> String {
    let (scoped_tid, bare) = split_scoped_model_id(model_id);
    if is_anthropic_model_name(bare) && scoped_tid.is_none() {
        return bare.to_string();
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

struct CanonicalModelSpec {
    canonical_id: &'static str,
    display_name: &'static str,
    family: &'static str,
}

const CANONICAL_MODEL_SPECS: &[CanonicalModelSpec] = &[
    CanonicalModelSpec {
        canonical_id: "deepseek-v4-pro",
        display_name: "DeepSeek-V4-Pro",
        family: "DeepSeek",
    },
    CanonicalModelSpec {
        canonical_id: "deepseek-v4.1-flash",
        display_name: "DeepSeek-V4.1-Flash",
        family: "DeepSeek",
    },
    CanonicalModelSpec {
        canonical_id: "glm-5.3",
        display_name: "GLM-5.3",
        family: "GLM",
    },
    CanonicalModelSpec {
        canonical_id: "glm-5.3-flash",
        display_name: "GLM-5.3-Flash",
        family: "GLM",
    },
    CanonicalModelSpec {
        canonical_id: "glm-5.2",
        display_name: "GLM-5.2",
        family: "GLM",
    },
    CanonicalModelSpec {
        canonical_id: "kimi-k3.1-pro",
        display_name: "Kimi-K3.1-Pro",
        family: "Kimi",
    },
    CanonicalModelSpec {
        canonical_id: "kimi-k3",
        display_name: "Kimi-K3",
        family: "Kimi",
    },
    CanonicalModelSpec {
        canonical_id: "kimi-k2.8-preview",
        display_name: "Kimi-K2.8-Preview",
        family: "Kimi",
    },
    CanonicalModelSpec {
        canonical_id: "kimi-k2.7-code",
        display_name: "Kimi-K2.7-Code",
        family: "Kimi",
    },
    CanonicalModelSpec {
        canonical_id: "minimax-m3.1-flash",
        display_name: "MiniMax-M3.1-Flash",
        family: "MiniMax",
    },
    CanonicalModelSpec {
        canonical_id: "minimax-m3",
        display_name: "MiniMax-M3",
        family: "MiniMax",
    },
    CanonicalModelSpec {
        canonical_id: "minimax-m2.7",
        display_name: "MiniMax-M2.7",
        family: "MiniMax",
    },
    CanonicalModelSpec {
        canonical_id: "minimax-m2.7-highspeed",
        display_name: "MiniMax-M2.7-Highspeed",
        family: "MiniMax",
    },
    CanonicalModelSpec {
        canonical_id: "qwen4-max",
        display_name: "Qwen4-Max",
        family: "Qwen",
    },
    CanonicalModelSpec {
        canonical_id: "qwen3.8-max",
        display_name: "Qwen3.8-Max",
        family: "Qwen",
    },
    CanonicalModelSpec {
        canonical_id: "qwen3.8-flash",
        display_name: "Qwen3.8-Flash",
        family: "Qwen",
    },
    CanonicalModelSpec {
        canonical_id: "step-5-preview",
        display_name: "Step-5-Preview",
        family: "Step",
    },
    CanonicalModelSpec {
        canonical_id: "mimo-v2.6-pro",
        display_name: "MiMo-V2.6-Pro",
        family: "MiMo",
    },
    CanonicalModelSpec {
        canonical_id: "mimo-v2.6-flash",
        display_name: "MiMo-V2.6-Flash",
        family: "MiMo",
    },
    CanonicalModelSpec {
        canonical_id: "hy4flash-free",
        display_name: "Hy4-Flash-Free",
        family: "Hunyuan",
    },
    CanonicalModelSpec {
        canonical_id: "hy4std",
        display_name: "Hy4",
        family: "Hunyuan",
    },
    CanonicalModelSpec {
        canonical_id: "hy4flash",
        display_name: "Hy4-Flash",
        family: "Hunyuan",
    },
    CanonicalModelSpec {
        canonical_id: "space-bunny",
        display_name: "Space-Bunny",
        family: "Hunyuan",
    },
    CanonicalModelSpec {
        canonical_id: "doubao-seed-2.1-pro",
        display_name: "Doubao-Seed-2.1-Pro",
        family: "Doubao",
    },
    CanonicalModelSpec {
        canonical_id: "doubao-seed-2.1-turbo",
        display_name: "Doubao-Seed-2.1-Turbo",
        family: "Doubao",
    },
    CanonicalModelSpec {
        canonical_id: "doubao-seed-code",
        display_name: "Doubao-Seed-Code",
        family: "Doubao",
    },
    CanonicalModelSpec {
        canonical_id: "openpangu-coder-pro",
        display_name: "openPangu-Coder-Pro",
        family: "Pangu",
    },
    CanonicalModelSpec {
        canonical_id: "fast",
        display_name: "Fast",
        family: "Other",
    },
    CanonicalModelSpec {
        canonical_id: "auto",
        display_name: "Auto",
        family: "Other",
    },
];

/// 工具层原始模型 ID / Qoder 槽位 Key -> 跨工具标准 Canonical ID 的对齐表
fn map_raw_to_canonical_id(
    tool_id: &str,
    raw_id: &str,
    raw_display_name: Option<&str>,
) -> Option<&'static str> {
    let trimmed = raw_id.trim();
    let lower = trimmed.to_ascii_lowercase();
    let display_lower = raw_display_name.unwrap_or("").trim().to_ascii_lowercase();

    // 1. Qoder CN 专有底层槽位 key 映射
    if tool_id == "qoder_cn" || tool_id == "qoder" {
        match lower.as_str() {
            "dmodel" => return Some("deepseek-v4-pro"),
            "dfmodel" => return Some("deepseek-v4.1-flash"),
            "gmodel" => return Some("glm-5.3"),
            "gfmodel" => return Some("glm-5.3-flash"),
            "kmodel_latest" => return Some("kimi-k3.1-pro"),
            "kmodel" => return Some("kimi-k2.8-preview"),
            "mmodel" => return Some("minimax-m3.1-flash"),
            "qmodel_latest" => return Some("qwen4-max"),
            "qmodel_38max" => return Some("qwen3.8-max"),
            "qfmodel" => return Some("qwen3.8-flash"),
            "jmodel" => return Some("step-5-preview"),
            "fast" => return Some("fast"),
            _ => {}
        }
    }

    // 2. 跨工具标准 ID 与常见别名映射
    match lower.as_str() {
        "deepseek-v4-pro" | "deepseek-v4-pro-official" => Some("deepseek-v4-pro"),
        "deepseek-v4.1-flash"
        | "deepseek-v4-flash"
        | "deepseek-v4-flash-official"
        | "deepseek-flash" => Some("deepseek-v4.1-flash"),
        "glm-5.3" => Some("glm-5.3"),
        "glm-5.3-flash" => Some("glm-5.3-flash"),
        "glm-5.2" => Some("glm-5.2"),
        "kimi-k3.1-pro" => Some("kimi-k3.1-pro"),
        "kimi-k3" | "kimi-k3-1" => Some("kimi-k3"),
        "kimi-k2.8-preview" => Some("kimi-k2.8-preview"),
        "kimi-k2.7-code" => Some("kimi-k2.7-code"),
        "minimax-m3.1-flash" | "minimax-m3.1-flash-preview" | "m3.1-flash-preview" => {
            Some("minimax-m3.1-flash")
        }
        "minimax-m3" => Some("minimax-m3"),
        "minimax-m2.7" => Some("minimax-m2.7"),
        "minimax-m2.7-highspeed" => Some("minimax-m2.7-highspeed"),
        "qwen4-max" | "qwen-4-max" => Some("qwen4-max"),
        "qwen3.8-max" | "qwen-3.8-max" | "qwen3.8-max-preview" => Some("qwen3.8-max"),
        "qwen3.8-flash" | "qwen-3.8-flash" => Some("qwen3.8-flash"),
        "step-5-preview" => Some("step-5-preview"),
        "mimo-v2.6-pro" | "xiaomi-mimo-v2.6-pro" => Some("mimo-v2.6-pro"),
        "mimo-v2.6-flash" => Some("mimo-v2.6-flash"),
        "hy4flash-free" => Some("hy4flash-free"),
        "hy4std" => Some("hy4std"),
        "hy4flash" => Some("hy4flash"),
        "space-bunny" => Some("space-bunny"),
        "doubao-seed-2.1-pro" | "seed-2.1-pro" | "seed-2.1-pro-0915" | "seed-code-pro-0430" => {
            Some("doubao-seed-2.1-pro")
        }
        "doubao-seed-2.1-turbo" | "seed-2.1-turbo" | "doubao-seed-2.0-code" => {
            Some("doubao-seed-2.1-turbo")
        }
        "doubao-seed-code" | "doubao_1_6" | "seed-code" => Some("doubao-seed-code"),
        "openpangu-coder-pro" => Some("openpangu-coder-pro"),
        "fast" => Some("fast"),
        "auto" => Some("auto"),
        _ => {
            // 如果原始 ID 未命中（如动态新模型），结合展示名匹配常见规范名称
            match display_lower.as_str() {
                "deepseek-v4-pro" => Some("deepseek-v4-pro"),
                "deepseek-v4.1-flash" | "deepseek-flash" => Some("deepseek-v4.1-flash"),
                "glm-5.3" => Some("glm-5.3"),
                "glm-5.3-flash" => Some("glm-5.3-flash"),
                "glm-5.2" => Some("glm-5.2"),
                "kimi-k3.1-pro" => Some("kimi-k3.1-pro"),
                "kimi-k3" => Some("kimi-k3"),
                "kimi-k2.8-preview" => Some("kimi-k2.8-preview"),
                "minimax-m3.1-flash" | "m3.1-flash-preview" => Some("minimax-m3.1-flash"),
                "minimax-m3" => Some("minimax-m3"),
                "qwen4-max" => Some("qwen4-max"),
                "qwen3.8-max" => Some("qwen3.8-max"),
                "qwen3.8-flash" => Some("qwen3.8-flash"),
                "step-5-preview" => Some("step-5-preview"),
                "xiaomi mimo-v2.6-pro" | "mimo-v2.6-pro" => Some("mimo-v2.6-pro"),
                _ => None,
            }
        }
    }
}

pub fn infer_model_family(model_id: &str) -> String {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("deepseek") || lower == "dmodel" || lower == "dfmodel" {
        "DeepSeek".into()
    } else if lower.contains("glm") || lower == "gmodel" || lower == "gfmodel" {
        "GLM".into()
    } else if lower.contains("kimi") || lower == "kmodel" || lower == "kmodel_latest" {
        "Kimi".into()
    } else if lower.contains("minimax") || lower.contains("m3.") || lower == "mmodel" {
        "MiniMax".into()
    } else if lower.contains("qwen") || lower.starts_with("qm") || lower == "qfmodel" {
        "Qwen".into()
    } else if lower.contains("doubao") || lower.contains("seed") {
        "Doubao".into()
    } else if lower.contains("hy") || lower.contains("hunyuan") || lower.contains("bunny") {
        "Hunyuan".into()
    } else if lower.contains("step") || lower == "jmodel" {
        "Step".into()
    } else if lower.contains("mimo") {
        "MiMo".into()
    } else if lower.contains("pangu") {
        "Pangu".into()
    } else if is_anthropic_model_name(&lower) {
        "Claude".into()
    } else if lower.contains("gpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
    {
        "GPT".into()
    } else {
        "Other".into()
    }
}

pub fn normalize_upstream_model(
    tool_id: &str,
    raw_id: &str,
    raw_display_name: Option<&str>,
    credit_multiplier: Option<f64>,
    is_free: bool,
    source: &str,
) -> UpstreamModelOption {
    let wire = raw_id.trim().to_string();
    let canonical = map_raw_to_canonical_id(tool_id, &wire, raw_display_name);
    if let Some(cid) = canonical {
        if let Some(spec) = CANONICAL_MODEL_SPECS.iter().find(|s| s.canonical_id == cid) {
            return enrich_model_meta(UpstreamModelOption {
                id: spec.canonical_id.to_string(),
                display_name: spec.display_name.to_string(),
                vendor_wire_id: wire,
                credit_multiplier,
                is_free,
                context_window: None,
                max_output_tokens: None,
                family: spec.family.to_string(),
                source: source.to_string(),
            });
        }
    }

    let normalized_id = wire.to_ascii_lowercase();
    let display_name = raw_display_name
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "-")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| wire.clone());
    enrich_model_meta(UpstreamModelOption {
        id: normalized_id.clone(),
        display_name,
        vendor_wire_id: wire,
        credit_multiplier,
        is_free,
        context_window: None,
        max_output_tokens: None,
        family: infer_model_family(&normalized_id),
        source: source.to_string(),
    })
}

/// 用内嵌的 models.dev 快照补齐上下文窗口 / 最大输出。
///
/// 只在 id 命中时补：未命中保持 None，UI 不显示，避免把猜的数字当成上游事实。
pub fn enrich_model_meta(mut model: UpstreamModelOption) -> UpstreamModelOption {
    if let Some(meta) = crate::domain::model_meta::lookup(&model.id) {
        if meta.context > 0 {
            model.context_window = Some(meta.context);
        }
        model.max_output_tokens = meta.output;
    }
    model
}

pub fn normalize_canonical_model_id(tool_id: &str, raw_or_canonical: &str) -> String {
    let (scoped_tid, bare) = split_scoped_model_id(raw_or_canonical);
    let effective_tid = scoped_tid.unwrap_or(tool_id);
    let canonical_bare =
        normalize_upstream_model(effective_tid, bare, None, None, false, "catalog").id;
    if scoped_tid.is_some() {
        format!("{effective_tid}/{canonical_bare}")
    } else {
        canonical_bare
    }
}

/// 同一账号拉取结果去重：若多个变体对齐到同一标准 ID（如 `deepseek-v4.1-flash` 与 `DeepSeek-V4-Flash-Official`），
/// 优先保留 `vendor_wire_id` 更直接一致（大小写忽略相等）的条目。
pub fn dedup_model_options(options: Vec<UpstreamModelOption>) -> Vec<UpstreamModelOption> {
    let mut out: Vec<UpstreamModelOption> = Vec::with_capacity(options.len());
    let mut idx_by_id: HashMap<String, usize> = HashMap::new();
    for opt in options {
        if let Some(&pos) = idx_by_id.get(&opt.id) {
            let existing = &out[pos];
            let incoming_exact = existing.vendor_wire_id.to_ascii_lowercase() != existing.id
                && opt.vendor_wire_id.to_ascii_lowercase() == opt.id;
            let incoming_cheaper = match (opt.credit_multiplier, existing.credit_multiplier) {
                (Some(a), Some(b)) => a < b && opt.vendor_wire_id.to_ascii_lowercase() == opt.id,
                _ => false,
            };
            if incoming_exact || incoming_cheaper {
                out[pos] = opt;
            }
        } else {
            idx_by_id.insert(opt.id.clone(), out.len());
            out.push(opt);
        }
    }
    out
}

/// 将号池内归一化标准模型 ID（或原始 ID）还原为指定上游工具 API 所需的真实 `vendor_wire_id`
pub fn resolve_vendor_wire_model_id(tool_id: &str, canonical_or_raw_model_id: &str) -> String {
    let trimmed = canonical_or_raw_model_id.trim();
    let lower = trimmed.to_ascii_lowercase();
    let cid = map_raw_to_canonical_id(tool_id, trimmed, None).unwrap_or(lower.as_str());

    match tool_id {
        "qoder_cn" | "qoder" => match cid {
            "deepseek-v4-pro" => Some("dmodel".into()),
            "deepseek-v4.1-flash" => Some("dfmodel".into()),
            "glm-5.3" => Some("gmodel".into()),
            "glm-5.3-flash" => Some("gfmodel".into()),
            "kimi-k3.1-pro" => Some("kmodel_latest".into()),
            "kimi-k2.8-preview" => Some("kmodel".into()),
            "minimax-m3.1-flash" => Some("mmodel".into()),
            "qwen4-max" => Some("qmodel_latest".into()),
            "qwen3.8-max" => Some("qmodel_38max".into()),
            "qwen3.8-flash" => Some("qfmodel".into()),
            "step-5-preview" => Some("jmodel".into()),
            "fast" => Some("fast".into()),
            _ => None,
        },
        "workbuddy" => match cid {
            "kimi-k3" => Some("kimi-k3-1".into()),
            _ => {
                if CANONICAL_MODEL_SPECS.iter().any(|s| s.canonical_id == cid) {
                    Some(cid.to_string())
                } else {
                    None
                }
            }
        },
        "trae_cn" => match cid {
            "deepseek-v4-pro" => Some("DeepSeek-V4-Pro".into()),
            "doubao-seed-2.1-pro" => Some("Doubao-Seed-2.1-Pro".into()),
            "doubao-seed-2.1-turbo" => Some("Doubao-Seed-2.1-Turbo".into()),
            "doubao-seed-code" => Some("Doubao-Seed-Code".into()),
            "glm-5.3"
            | "glm-5.3-flash"
            | "glm-5.2"
            | "kimi-k3.1-pro"
            | "kimi-k3"
            | "kimi-k2.8-preview"
            | "kimi-k2.7-code"
            | "deepseek-v4.1-flash"
            | "minimax-m3"
            | "minimax-m3.1-flash"
            | "mimo-v2.6-pro"
            | "mimo-v2.6-flash"
            | "step-5-preview"
            | "qwen4-max"
            | "qwen3.8-max"
            | "qwen3.8-flash" => Some(cid.to_string()),
            _ => None,
        },
        "minimax_code_cn" => match cid {
            "minimax-m3.1-flash" => Some("MiniMax-M3.1-Flash-Preview".into()),
            "minimax-m3" => Some("MiniMax-M3".into()),
            "minimax-m2.7" => Some("MiniMax-M2.7".into()),
            "minimax-m2.7-highspeed" => Some("MiniMax-M2.7-highspeed".into()),
            _ => None,
        },
        "codearts" => match cid {
            "openpangu-coder-pro" => Some("openPangu-Coder-Pro".into()),
            "glm-5.3" => Some("glm-5.3".into()),
            "glm-5.2" => Some("glm-5.2".into()),
            "deepseek-v4-pro" => Some("deepseek-v4-pro".into()),
            _ => None,
        },
        _ => None,
    }
    // 静态映射表未命中时，退回实时探测登记的 wire ID：
    // 静态目录之外的实时模型（如 Trae 新增的 `Qwen3.8-Max`）大小写敏感，
    // 沿用归一化后的 ID 会被上游拒收。
    .or_else(|| live_wire_model_id(tool_id, cid))
    .unwrap_or_else(|| trimmed.to_string())
}

fn catalog_opt(
    tool_id: &str,
    raw_id: &str,
    rate: Option<f64>,
    is_free: bool,
) -> UpstreamModelOption {
    normalize_upstream_model(tool_id, raw_id, None, rate, is_free, "catalog")
}

/// 经各厂商真实线上协议报文核验的兜底模型目录（在网络不可达或尚无在线缓存时启用）
pub fn default_model_options_for_tool(tool_id: &str) -> Vec<UpstreamModelOption> {
    match tool_id {
        "workbuddy" => vec![
            catalog_opt(tool_id, "deepseek-v4-pro", Some(0.51), false),
            catalog_opt(tool_id, "deepseek-v4.1-flash", Some(0.11), false),
            catalog_opt(tool_id, "glm-5.3", Some(0.69), false),
            catalog_opt(tool_id, "glm-5.3-flash", Some(0.08), false),
            catalog_opt(tool_id, "glm-5.2", Some(0.79), false),
            catalog_opt(tool_id, "kimi-k3.1-pro", Some(0.97), false),
            catalog_opt(tool_id, "kimi-k3-1", Some(1.62), false),
            catalog_opt(tool_id, "kimi-k2.8-preview", Some(0.77), false),
            catalog_opt(tool_id, "minimax-m3.1-flash", Some(0.08), false),
            catalog_opt(tool_id, "step-5-preview", Some(0.33), false),
            catalog_opt(tool_id, "mimo-v2.6-pro", Some(0.13), false),
            catalog_opt(tool_id, "hy4flash-free", Some(0.0), true),
            catalog_opt(tool_id, "hy4std", Some(0.29), false),
            catalog_opt(tool_id, "hy4flash", Some(0.03), false),
            catalog_opt(tool_id, "space-bunny", Some(0.08), false),
            catalog_opt(tool_id, "auto", Some(0.0), false),
        ],
        "qoder_cn" => vec![
            catalog_opt(tool_id, "dmodel", Some(0.5), false),
            catalog_opt(tool_id, "dfmodel", Some(0.1), false),
            catalog_opt(tool_id, "gmodel", Some(0.8), false),
            catalog_opt(tool_id, "gfmodel", Some(0.1), false),
            catalog_opt(tool_id, "kmodel_latest", Some(1.0), false),
            catalog_opt(tool_id, "kmodel", Some(0.8), false),
            catalog_opt(tool_id, "mmodel", Some(0.1), false),
            catalog_opt(tool_id, "qmodel_latest", Some(0.4), false),
            catalog_opt(tool_id, "qmodel_38max", Some(0.5), false),
            catalog_opt(tool_id, "qfmodel", Some(0.1), false),
            catalog_opt(tool_id, "jmodel", Some(0.3), false),
            catalog_opt(tool_id, "fast", Some(0.6), false),
        ],
        "trae_cn" => vec![
            catalog_opt(tool_id, "DeepSeek-V4-Pro", Some(0.72), false),
            catalog_opt(tool_id, "deepseek-v4.1-flash", Some(0.12), false),
            catalog_opt(tool_id, "glm-5.3", Some(0.78), false),
            catalog_opt(tool_id, "glm-5.3-flash", Some(0.08), false),
            catalog_opt(tool_id, "glm-5.2", Some(0.78), false),
            catalog_opt(tool_id, "kimi-k3.1-pro", Some(1.2), false),
            catalog_opt(tool_id, "kimi-k3", Some(1.83), false),
            catalog_opt(tool_id, "kimi-k2.8-preview", Some(0.98), false),
            catalog_opt(tool_id, "kimi-k2.7-code", Some(0.83), false),
            catalog_opt(tool_id, "minimax-m3.1-flash", Some(0.09), false),
            catalog_opt(tool_id, "minimax-m3", Some(0.26), false),
            catalog_opt(tool_id, "qwen3.8-flash", Some(0.08), false),
            catalog_opt(tool_id, "step-5-preview", Some(0.48), false),
            catalog_opt(tool_id, "mimo-v2.6-pro", Some(0.15), false),
            catalog_opt(tool_id, "Doubao-Seed-2.1-Pro", Some(0.8), false),
            catalog_opt(tool_id, "Doubao-Seed-2.1-Turbo", Some(0.2), false),
            catalog_opt(tool_id, "Doubao-Seed-Code", Some(0.06), false),
        ],
        "minimax_code_cn" => vec![
            catalog_opt(tool_id, "MiniMax-M3.1-Flash-Preview", Some(0.0), false),
            catalog_opt(tool_id, "MiniMax-M3", Some(0.0), false),
            catalog_opt(tool_id, "MiniMax-M2.7", Some(0.0), false),
            catalog_opt(tool_id, "MiniMax-M2.7-highspeed", Some(0.0), false),
        ],
        // 码道没有可靠的模型目录接口（本地内核 `/cag/model` 需要 IDE 私有令牌，
        // 云端 inferhub 路径未公开），历史上这里写死了 4 个猜测模型，实测与该账号
        // 真实可用模型（deepseek-v4.1-flash / glm-5.3-flash / kimi-k3.1-pro / kimi-k3）
        // 几乎零交集，等于凭空展示不存在的模型。这里不再编造，改为空目录：
        // 用户可在「Agent 接入 → 模型选择」里手工维护该账号的真实模型。
        "codearts" => vec![],
        "doubaowork" => vec![
            catalog_opt(tool_id, "Doubao-Seed-2.1-Pro", None, false),
            catalog_opt(tool_id, "Doubao-Seed-Code", None, false),
            catalog_opt(tool_id, "DeepSeek-V4-Pro", None, false),
            catalog_opt(tool_id, "GLM-5.3", None, false),
            catalog_opt(tool_id, "Kimi-K3.1-Pro", None, false),
        ],
        "lobsterai" => vec![
            catalog_opt(tool_id, "deepseek-v4-pro", None, false),
            catalog_opt(tool_id, "deepseek-v4.1-flash", None, false),
            catalog_opt(tool_id, "glm-5.3", None, false),
            catalog_opt(tool_id, "kimi-k3.1-pro", None, false),
            catalog_opt(tool_id, "qwen4-max", None, false),
        ],
        "custom_api_key" => vec![
            catalog_opt(tool_id, "deepseek-v4-pro", None, false),
            catalog_opt(tool_id, "glm-5.3", None, false),
            catalog_opt(tool_id, "deepseek-chat", None, false),
            catalog_opt(tool_id, "claude-sonnet-4.6", None, false),
        ],
        _ => vec![
            catalog_opt(tool_id, "deepseek-v4-pro", None, false),
            catalog_opt(tool_id, "glm-5.3", None, false),
        ],
    }
}

pub fn default_models_for_tool(tool_id: &str) -> Vec<String> {
    default_model_options_for_tool(tool_id)
        .into_iter()
        .map(|m| m.id)
        .collect()
}

/// 内置「推荐目录」：给没有自动探测能力的工具做一键套用。
///
/// 与 `default_model_options_for_tool`（静态兜底）的区别：
/// - 兜底是系统**自动**展示的猜测，来源标记 `catalog`，UI 标「未探测」；
/// - 推荐目录必须由用户显式点一次「套用」，来源标记 `preset`，UI 标「内置预设」，
///   语义是「我认可这组模型可用」，可随时改成手工维护。
pub fn recommended_model_options_for_tool(tool_id: &str) -> Vec<UpstreamModelOption> {
    // 目录来自 resources/model_presets.json：加模型、改输出上限都不必重新编译
    crate::domain::model_meta::preset_entries(tool_id)
        .iter()
        .map(|entry| {
            let mut m = normalize_upstream_model(
                tool_id,
                &entry.id,
                entry.name.as_deref(),
                None,
                false,
                "preset",
            );
            // 预设里的 context/output 优先于元数据快照：这些数字来自上游官方配置
            if let Some(ctx) = entry.context {
                m.context_window = Some(ctx);
            }
            if let Some(out) = entry.output {
                m.max_output_tokens = Some(out);
            }
            m
        })
        .collect()
}

pub fn upstream_model_protocol(tool_id: &str, model_id: &str) -> WireProtocolKind {
    if tool_id == "minimax_code_cn"
        || is_anthropic_model_name(model_id) && tool_id == "custom_api_key"
    {
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
    let existing_map: HashMap<&str, &PoolModelPolicy> =
        existing.iter().map(|p| (p.model_id.as_str(), p)).collect();
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
    let (scoped_tid, bare_model) = split_scoped_model_id(model_id);
    let serving: Vec<&PoolMember> = pool
        .members
        .iter()
        .filter(|m| {
            scoped_tid.is_none_or(|tid| m.tool_id == tid)
                && m.supported_models
                    .iter()
                    .any(|id| id == model_id || split_scoped_model_id(id).1 == bare_model)
        })
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
        // 码道不参与切号流程，桌面端活跃不构成跳过理由
        let skipped = pool.skip_active_client_account
            && m.is_active_on_client.unwrap_or(false)
            && m.tool_id != "codearts";
        let paused = matches!(
            m.account_status.as_deref(),
            Some("risk_paused") | Some("expired") | Some("disabled")
        );
        // API Key 兜底关闭时，custom_api_key 成员不计入可用（与 gateway 调度口径一致）
        let api_key_disabled = m.tool_id == "custom_api_key" && !pool.fallback_api_key_enabled;
        if m.enabled && !paused && !cooled && !skipped && !api_key_disabled {
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
        *model_counts
            .entry(item.policy.model_id.as_str())
            .or_insert(0) += 1;
    }

    let keys: Vec<String> = raw_items
        .iter()
        .map(|item| make_default_model_key(&item.pool.id, &item.policy.model_id))
        .collect();
    let has_explicit_default = default_model_key.is_some_and(|dk| keys.iter().any(|k| k == dk));

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
            let (scoped_tid, bare_model) = split_scoped_model_id(&item.policy.model_id);
            let display_name = if let Some(tid) = scoped_tid {
                let human_label = default_model_options_for_tool(tid)
                    .into_iter()
                    .find(|o| o.id == bare_model)
                    .map(|o| o.display_name)
                    .unwrap_or_else(|| bare_model.to_string());
                let base_name = format!("{} · {}", tool_short_display_name(tid), human_label);
                if collision {
                    format!("{base_name} · {}", item.pool.name)
                } else {
                    base_name
                }
            } else if collision {
                format!("{} · {}", item.policy.model_id, item.pool.name)
            } else {
                item.policy.model_id.clone()
            };
            let wire_model_id = if agent_id == TargetAgentId::ClaudeDesktop {
                if scoped_tid.is_some() && is_anthropic_model_name(bare_model) && !collision {
                    bare_model.to_string()
                } else {
                    item.policy.claude_alias_id.clone().unwrap_or_else(|| {
                        build_claude_model_alias(&item.pool.slug, &item.policy.model_id)
                    })
                }
            } else if let Some(tid) = scoped_tid {
                if collision {
                    format!("{}/{tid}/{bare_model}", item.pool.slug)
                } else {
                    format!("{tid}/{bare_model}")
                }
            } else {
                format!("{}/{}", item.pool.slug, item.policy.model_id)
            };
            let is_claude_disguised =
                agent_id == TargetAgentId::ClaudeDesktop && wire_model_id != bare_model;
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
    fn normalizes_cross_tool_models_and_resolves_vendor_wire_ids() {
        let wb = normalize_upstream_model(
            "workbuddy",
            "deepseek-v4-pro",
            Some("DeepSeek-V4-Pro"),
            Some(0.51),
            false,
            "live",
        );
        let qd = normalize_upstream_model(
            "qoder_cn",
            "dmodel",
            Some("DeepSeek-V4-Pro"),
            Some(0.5),
            false,
            "live",
        );
        let tr = normalize_upstream_model(
            "trae_cn",
            "DeepSeek-V4-Pro",
            Some("DeepSeek-V4-Pro"),
            Some(0.72),
            false,
            "live",
        );

        assert_eq!(wb.id, "deepseek-v4-pro");
        assert_eq!(qd.id, "deepseek-v4-pro");
        assert_eq!(tr.id, "deepseek-v4-pro");
        assert_eq!(qd.vendor_wire_id, "dmodel");

        assert_eq!(
            resolve_vendor_wire_model_id("qoder_cn", "deepseek-v4-pro"),
            "dmodel"
        );
        assert_eq!(
            resolve_vendor_wire_model_id("qoder_cn", "glm-5.3"),
            "gmodel"
        );
        assert_eq!(
            resolve_vendor_wire_model_id("qoder_cn", "kimi-k3.1-pro"),
            "kmodel_latest"
        );
        assert_eq!(
            resolve_vendor_wire_model_id("workbuddy", "kimi-k3"),
            "kimi-k3-1"
        );
        assert_eq!(
            resolve_vendor_wire_model_id("trae_cn", "deepseek-v4-pro"),
            "DeepSeek-V4-Pro"
        );
        assert_eq!(
            resolve_vendor_wire_model_id("minimax_code_cn", "minimax-m3.1-flash"),
            "MiniMax-M3.1-Flash-Preview"
        );

        let wb_defaults = default_models_for_tool("workbuddy");
        assert!(wb_defaults.contains(&"deepseek-v4-pro".to_string()));
        assert!(!wb_defaults.contains(&"claude-sonnet-4.6".to_string()));
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

    #[test]
    fn scoped_model_ids_separate_tools_cleanly_in_agent_menu() {
        let pool = AccountPool {
            id: "pool-agent-opencode".into(),
            slug: "opencode".into(),
            name: "OpenCode 专属路由".into(),
            description: "".into(),
            scene_tag: "Agent".into(),
            enabled: true,
            strategy: PoolStrategy::Order,
            affinity_mode: AffinityMode::AutoCache,
            skip_active_client_account: false,
            cooldown_seconds: 60,
            auto_refresh_on_401: true,
            fallback_api_key_enabled: true,
            model_policies: vec![
                PoolModelPolicy {
                    model_id: "workbuddy/deepseek-v4-pro".into(),
                    enabled: true,
                    fallback_model_id: None,
                    claude_alias_id: None,
                },
                PoolModelPolicy {
                    model_id: "qoder_cn/deepseek-v4-pro".into(),
                    enabled: true,
                    fallback_model_id: None,
                    claude_alias_id: None,
                },
            ],
            members: vec![
                PoolMember {
                    id: "m1".into(),
                    account_id: "a1".into(),
                    tool_id: "workbuddy".into(),
                    label: "WB 主号".into(),
                    uid_masked: "u1".into(),
                    supported_models: vec!["workbuddy/deepseek-v4-pro".into()],
                    tier: MemberTier::T1Primary,
                    channel: UpstreamChannel::OpencodeBridge,
                    enabled: true,
                    priority: 1,
                    quota_percent: 80,
                    quota_text: "".into(),
                    cooldown_until_ms: None,
                    is_active_on_client: Some(false),
                    account_status: Some("active".into()),
                    code_arts_mode: None,
                    last_latency_ms: Some(400),
                },
                PoolMember {
                    id: "m2".into(),
                    account_id: "a2".into(),
                    tool_id: "qoder_cn".into(),
                    label: "Qoder 主号".into(),
                    uid_masked: "u2".into(),
                    supported_models: vec!["qoder_cn/deepseek-v4-pro".into()],
                    tier: MemberTier::T1Primary,
                    channel: UpstreamChannel::OpencodeBridge,
                    enabled: true,
                    priority: 2,
                    quota_percent: 60,
                    quota_text: "".into(),
                    cooldown_until_ms: None,
                    is_active_on_client: Some(false),
                    account_status: Some("active".into()),
                    code_arts_mode: None,
                    last_latency_ms: Some(420),
                },
            ],
            today_requests: 0,
            today_failovers: 0,
            cache_hit_rate: 0,
            created_at: "".into(),
        };

        let exposed = compute_agent_exposed_models(
            TargetAgentId::Opencode,
            &["pool-agent-opencode".to_string()],
            None,
            &[pool],
            1000,
        );
        assert_eq!(exposed.len(), 2);
        assert_eq!(exposed[0].wire_model_id, "workbuddy/deepseek-v4-pro");
        assert_eq!(exposed[0].display_name, "WorkBuddy · DeepSeek-V4-Pro");
        assert_eq!(exposed[0].account_count, 1);
        assert_eq!(exposed[1].wire_model_id, "qoder_cn/deepseek-v4-pro");
        assert_eq!(exposed[1].display_name, "Qoder CN · DeepSeek-V4-Pro");
        assert_eq!(exposed[1].account_count, 1);
    }

    #[test]
    fn live_wire_registry_restores_probed_wire_id_for_unknown_models() {
        // 静态映射表未收录的实时模型：归一化后的标准 ID 往往不是上游真实 ID，
        // 实时探测登记后必须能还原出大小写敏感的真实 ID。
        let live = UpstreamModelOption {
            id: "kimi-k4-preview".into(),
            display_name: "Kimi K4 Preview".into(),
            vendor_wire_id: "Kimi-K4-Preview".into(),
            credit_multiplier: None,
            is_free: false,
            context_window: None,
            max_output_tokens: None,
            family: "Kimi".into(),
            source: "live".into(),
        };
        register_live_wire_ids("trae_cn", std::slice::from_ref(&live));
        assert_eq!(
            resolve_vendor_wire_model_id("trae_cn", "kimi-k4-preview"),
            "Kimi-K4-Preview"
        );

        // 工具别名（qoder ↔ qoder_cn）共用同一份注册表
        register_live_wire_ids("qoder_cn", std::slice::from_ref(&live));
        assert_eq!(
            resolve_vendor_wire_model_id("qoder", "kimi-k4-preview"),
            "Kimi-K4-Preview"
        );
    }

    #[test]
    fn live_wire_registry_ignores_catalog_entries_and_keeps_static_priority() {
        // 静态兜底目录里的 wire ID 是猜测值，不得覆盖静态映射表
        let catalog = UpstreamModelOption {
            id: "deepseek-v4-pro".into(),
            display_name: "DeepSeek-V4-Pro".into(),
            vendor_wire_id: "deepseek-v4-pro".into(),
            credit_multiplier: None,
            is_free: false,
            context_window: None,
            max_output_tokens: None,
            family: "DeepSeek".into(),
            source: "catalog".into(),
        };
        register_live_wire_ids("trae_cn", std::slice::from_ref(&catalog));
        // 静态表命中优先：仍返回静态映射的 `DeepSeek-V4-Pro`
        assert_eq!(
            resolve_vendor_wire_model_id("trae_cn", "deepseek-v4-pro"),
            "DeepSeek-V4-Pro"
        );
    }

    #[test]
    fn model_metadata_fills_context_and_output_only_when_known() {
        use crate::domain::normalize_upstream_model;
        // 命中：上下文/输出上限来自 models.dev 快照
        let known = normalize_upstream_model(
            "codearts",
            "deepseek-v4.1-flash",
            None,
            None,
            false,
            "client",
        );
        assert_eq!(known.context_window, Some(1_000_000));
        assert_eq!(known.max_output_tokens, Some(384_000));

        // 未命中：保持 None，绝不猜
        let unknown = normalize_upstream_model(
            "trae_cn",
            "codewise-nes-a4-027-aide",
            None,
            None,
            false,
            "live",
        );
        assert_eq!(unknown.context_window, None);
        assert_eq!(unknown.max_output_tokens, None);

        // 大小写不敏感（豆包静态表用展示名当 id）
        let upper = normalize_upstream_model(
            "doubaowork",
            "Doubao-Seed-2.1-Pro",
            None,
            None,
            false,
            "preset",
        );
        assert!(upper.context_window.is_some());
    }

    #[test]
    fn presets_come_from_data_file_and_cover_all_three_tools() {
        for tool in ["codearts", "lobsterai", "doubaowork"] {
            let preset = recommended_model_options_for_tool(tool);
            assert!(!preset.is_empty(), "{tool} 应有内置推荐目录");
            assert!(
                preset.iter().all(|m| m.source == "preset"),
                "{tool} 的推荐目录来源必须标成 preset"
            );
        }
        // 没有预设的工具返回空，UI 会退到静态兜底
        assert!(recommended_model_options_for_tool("workbuddy").is_empty());
        assert!(crate::domain::model_meta::known_model_count() > 20);
    }
}
