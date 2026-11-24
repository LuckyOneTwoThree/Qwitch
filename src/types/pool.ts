export type PoolStrategy = "quota_first" | "drain" | "order" | "round_robin";

export type AffinityMode = "auto_cache" | "session" | "turn" | "off";

export type UpstreamChannel = "rust_native" | "opencode_bridge" | "api_key";

export type TargetAgentId = "codex_desktop" | "claude_desktop" | "dsh" | "opencode" | "mimo" | "zcode";

export type AgentBindingStatus = "bound" | "drifted" | "unbound" | "needs_restart";

export type WireProtocolKind = "openai_chat" | "anthropic_messages" | "openai_responses";

export type CodeArtsAuthMode = "client_readonly" | "isolated_oauth";

export type MemberTier = "t1_primary" | "t2_backup" | "t3_fallback";

export interface PoolModelPolicy {
  modelId: string;
  enabled: boolean;
  /** null 表示严格锁定该模型，绝不静默切换；非 null 为显式指定的兜底降级模型 */
  fallbackModelId: string | null;
  claudeAliasId?: string;
}

export interface PoolMember {
  id: string;
  accountId: string;
  toolId: string;
  label: string;
  uidMasked: string;
  /** 该账号在当前号池内开放参与调度的模型列表（支持多选） */
  supportedModels: string[];
  /** 跨工具调度梯队：T1 主力 -> T2 备选 -> T3 付费兜底 */
  tier: MemberTier;
  channel: UpstreamChannel;
  enabled: boolean;
  priority: number;
  quotaPercent: number;
  quotaText: string;
  cooldownUntilMs?: number;
  isActiveOnClient?: boolean;
  accountStatus?: "active" | "expired" | "risk_paused" | "disabled";
  codeArtsMode?: CodeArtsAuthMode;
  lastLatencyMs?: number;
}

export interface AccountPool {
  id: string;
  slug: string;
  name: string;
  description: string;
  sceneTag: string;
  enabled: boolean;
  strategy: PoolStrategy;
  affinityMode: AffinityMode;
  skipActiveClientAccount: boolean;
  cooldownSeconds: number;
  autoRefreshOn401: boolean;
  fallbackApiKeyEnabled: boolean;
  modelPolicies: PoolModelPolicy[];
  members: PoolMember[];
  todayRequests: number;
  todayFailovers: number;
  cacheHitRate: number;
  createdAt: string;
}

export interface AgentBindingOptions {
  claudeIsolatedTwin?: boolean;
  codexPreserveTable?: boolean;
  dshDualCredsWritten?: boolean;
  opencodeSchemaVersion?: "v1" | "v2";
}

export interface AgentBinding {
  agentId: TargetAgentId;
  name: string;
  subtitle: string;
  protocol: WireProtocolKind;
  configPath: string;
  installed: boolean;
  /** 目标桌面应用当前是否在运行（决定按钮显示「启动」还是「重启」） */
  running: boolean;
  version: string;
  status: AgentBindingStatus;
  /** 1 个 Agent 支持同时挂载多个组合号池（如 Coding 池 + 推理池） */
  mountedPoolIds: string[];
  /** 当前设为默认的模型标识：`${poolId}::${modelId}` */
  defaultModelKey: string | null;
  driftReason?: string;
  proxyWarning?: string;
  restartHint?: string;
  lastHitAt?: string;
  options: AgentBindingOptions;
  beforeSnippet: string;
  afterSnippet: string;
}

export interface AgentExposedModel {
  key: string;
  poolId: string;
  poolName: string;
  modelId: string;
  displayName: string;
  wireModelId: string;
  isClaudeDisguised: boolean;
  accountCount: number;
  healthyAccountCount: number;
  tierSummary: string;
  isDefault: boolean;
  collision: boolean;
}

export interface RoutingAttempt {
  accountLabel: string;
  toolId: string;
  model: string;
  tier?: MemberTier;
  outcome: "ok" | "429_cooldown" | "401_refreshed" | "skipped_active";
  latencyMs: number;
  detail: string;
}

export interface RoutingTraceItem {
  id: string;
  timestamp: string;
  agentId: TargetAgentId;
  poolId: string;
  poolName: string;
  incomingProtocol: WireProtocolKind;
  upstreamProtocol: WireProtocolKind;
  requestedModel: string;
  finalAccountLabel: string;
  finalToolId: string;
  finalModel: string;
  status: "success" | "failover_success" | "failed";
  cacheAffinityHit: boolean;
  ttftMs: number;
  totalTokens: number;
  attempts: RoutingAttempt[];
}

export interface GatewayRuntimeState {
  running: boolean;
  host: string;
  port: number;
  bridgeRuntime: "bun_ready" | "node_ready" | "missing";
  bridgeVersion: string;
  globalProxyDetected: boolean;
}

export interface UpstreamModelOption {
  id: string;
  displayName: string;
  vendorWireId: string;
  creditMultiplier?: number;
  isFree?: boolean;
  family: string;
  source: string;
}

export interface UpstreamAccountCandidate {
  accountId: string;
  toolId: string;
  label: string;
  uidMasked: string;
  channel: UpstreamChannel;
  defaultModels: string[];
  modelOptions?: UpstreamModelOption[];
  defaultTier: MemberTier;
  quotaPercent: number;
  quotaText: string;
  isActiveOnClient?: boolean;
  accountStatus: "active" | "expired" | "risk_paused" | "disabled";
  codeArtsMode?: CodeArtsAuthMode;
}

export interface PoolSceneTemplate {
  id: string;
  title: string;
  sceneTag: string;
  description: string;
  defaultName: string;
  strategy: PoolStrategy;
  affinityMode: AffinityMode;
  skipActiveClientAccount: boolean;
  cooldownSeconds: number;
  autoRefreshOn401: boolean;
  fallbackApiKeyEnabled: boolean;
  recommendedTools: string[];
}

export interface RunPoolSimulationInput {
  agentId: TargetAgentId;
  poolId: string;
  modelId: string;
  prompt: string;
  failureMode: "none" | "429_first" | "401_refresh";
  sessionKey?: string;
}

export interface RunPoolSimulationResult {
  trace: RoutingTraceItem;
  cooledMemberId?: string;
}
