import type {
  AccountPool,
  AffinityMode,
  AgentBinding,
  AgentExposedModel,
  GatewayRuntimeState,
  MemberTier,
  PoolMember,
  PoolModelPolicy,
  PoolSceneTemplate,
  PoolStrategy,
  RoutingTraceItem,
  TargetAgentId,
  UpstreamAccountCandidate,
  UpstreamChannel,
  UpstreamModelOption,
  WireProtocolKind,
} from "../types/pool";

export const MEMBER_TIER_META: Record<
  MemberTier,
  {
    label: string;
    shortLabel: string;
    badgeClass: string;
    description: string;
    rank: number;
  }
> = {
  t1_primary: {
    label: "T1 主力档",
    shortLabel: "T1 主力",
    badgeClass:
      "border-brand-500/30 bg-brand-500/12 text-brand-800 dark:border-brand-400/35 dark:text-brand-200",
    description: "优先承接流量；同工具多账号间按水位/策略竞争",
    rank: 1,
  },
  t2_backup: {
    label: "T2 备选降级档",
    shortLabel: "T2 备选",
    badgeClass:
      "border-sky-500/30 bg-sky-500/12 text-sky-800 dark:border-sky-400/30 dark:text-sky-200",
    description: "T1 全员冷却、风控或耗尽时自动顺延接管",
    rank: 2,
  },
  t3_fallback: {
    label: "T3 付费兜底档",
    shortLabel: "T3 兜底",
    badgeClass:
      "border-zinc-400/35 bg-zinc-500/10 text-zinc-700 dark:border-zinc-600 dark:text-zinc-300",
    description: "末位保底通道；标准 API Key 强制锁定此档防抢免费流量",
    rank: 3,
  },
};

export const POOL_STRATEGY_META: Record<
  PoolStrategy,
  { label: string; shortLabel: string; description: string }
> = {
  quota_first: {
    label: "同工具水位优先",
    shortLabel: "同工具水位",
    description:
      "跨工具严守 T1→T2→T3 与编排顺位（绝不盲比不同工具百分比）；同一工具多号间按实时水位降序择优",
  },
  drain: {
    label: "单号优先耗尽",
    shortLabel: "单号耗尽",
    description:
      "在当前最高可用梯队内持续锁定同一主力账号直到耗尽或限流，最大化 Prompt Cache 命中",
  },
  order: {
    label: "固定顺位故障转移",
    shortLabel: "固定顺位",
    description: "严格按 T1→T2→T3 与成员顺位路由，仅在 429 / 401 / 耗尽时顺延下一号",
  },
  round_robin: {
    label: "均衡轮询分摊",
    shortLabel: "均衡轮询",
    description: "在当前最高可用梯队成员间均匀轮转分摊新会话，降低单号并发限流概率",
  },
};

export const AFFINITY_MODE_META: Record<
  AffinityMode,
  { label: string; description: string }
> = {
  auto_cache: {
    label: "智能保缓存",
    description: "缓存窗口期内（5–60 分钟）自动粘住上次成功账号与对应模型",
  },
  session: {
    label: "整会话绑定",
    description: "同一对话始终路由到同一账号，避免跨号推理签名失效",
  },
  turn: {
    label: "单轮内绑定",
    description: "仅在当前 Tool Use 循环内粘滞，下一轮用户发言重新按梯队与同工具水位选号",
  },
  off: {
    label: "关闭亲和",
    description: "每次请求独立按策略重算（不推荐用于多轮长上下文）",
  },
};

export const WIRE_PROTOCOL_META: Record<
  WireProtocolKind,
  { label: string; badgeClass: string }
> = {
  openai_responses: {
    label: "Responses",
    badgeClass:
      "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  },
  anthropic_messages: {
    label: "Anthropic",
    badgeClass:
      "border-amber-500/25 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  },
  openai_chat: {
    label: "Chat",
    badgeClass:
      "border-sky-500/25 bg-sky-500/10 text-sky-700 dark:text-sky-300",
  },
};

export const CHANNEL_META: Record<
  UpstreamChannel,
  { label: string; shortLabel: string; badgeClass: string }
> = {
  rust_native: {
    label: "Rust 原生通道",
    shortLabel: "Rust 原生",
    badgeClass:
      "border-brand-500/25 bg-brand-500/10 text-brand-800 dark:text-brand-300",
  },
  opencode_bridge: {
    label: "OpenCode 桥接",
    shortLabel: "插件桥接",
    badgeClass:
      "border-indigo-500/25 bg-indigo-500/10 text-indigo-700 dark:text-indigo-300",
  },
  api_key: {
    label: "标准 API Key 兜底",
    shortLabel: "API Key",
    badgeClass:
      "border-zinc-400/30 bg-zinc-500/10 text-zinc-700 dark:text-zinc-300",
  },
};

export interface CatalogModelEntry extends UpstreamModelOption {
  label: string;
  protocol: WireProtocolKind;
}

function makeCatalogModel(
  id: string,
  displayName: string,
  vendorWireId: string,
  family: string,
  protocol: WireProtocolKind = "openai_chat",
  creditMultiplier?: number,
  isFree = false
): CatalogModelEntry {
  return {
    id,
    label: displayName,
    displayName,
    vendorWireId,
    family,
    protocol,
    creditMultiplier,
    isFree,
    source: "catalog",
  };
}

export const UPSTREAM_MODEL_OPTIONS: Record<string, CatalogModelEntry[]> = {
  workbuddy: [
    makeCatalogModel("deepseek-v4-pro", "DeepSeek-V4-Pro", "deepseek-v4-pro", "DeepSeek", "openai_chat", 0.51),
    makeCatalogModel("deepseek-v4.1-flash", "DeepSeek-V4.1-Flash", "deepseek-v4.1-flash", "DeepSeek", "openai_chat", 0.11),
    makeCatalogModel("glm-5.3", "GLM-5.3", "glm-5.3", "GLM", "openai_chat", 0.69),
    makeCatalogModel("glm-5.3-flash", "GLM-5.3-Flash", "glm-5.3-flash", "GLM", "openai_chat", 0.08),
    makeCatalogModel("glm-5.2", "GLM-5.2", "glm-5.2", "GLM", "openai_chat", 0.79),
    makeCatalogModel("kimi-k3.1-pro", "Kimi-K3.1-Pro", "kimi-k3.1-pro", "Kimi", "openai_chat", 0.97),
    makeCatalogModel("kimi-k3", "Kimi-K3", "kimi-k3-1", "Kimi", "openai_chat", 1.62),
    makeCatalogModel("kimi-k2.8-preview", "Kimi-K2.8-Preview", "kimi-k2.8-preview", "Kimi", "openai_chat", 0.77),
    makeCatalogModel("minimax-m3.1-flash", "MiniMax-M3.1-Flash", "minimax-m3.1-flash", "MiniMax", "openai_chat", 0.08),
    makeCatalogModel("step-5-preview", "Step-5-Preview", "step-5-preview", "Step", "openai_chat", 0.33),
    makeCatalogModel("mimo-v2.6-pro", "MiMo-V2.6-Pro", "mimo-v2.6-pro", "MiMo", "openai_chat", 0.13),
    makeCatalogModel("hy4flash-free", "Hy4-Flash-Free", "hy4flash-free", "Hunyuan", "openai_chat", 0, true),
    makeCatalogModel("hy4std", "Hy4", "hy4std", "Hunyuan", "openai_chat", 0.29),
    makeCatalogModel("hy4flash", "Hy4-Flash", "hy4flash", "Hunyuan", "openai_chat", 0.03),
    makeCatalogModel("space-bunny", "Space-Bunny", "space-bunny", "Hunyuan", "openai_chat", 0.08),
  ],
  minimax_code_cn: [
    makeCatalogModel("minimax-m3.1-flash", "MiniMax-M3.1-Flash", "MiniMax-M3.1-Flash-Preview", "MiniMax", "anthropic_messages"),
    makeCatalogModel("minimax-m3", "MiniMax-M3", "MiniMax-M3", "MiniMax", "anthropic_messages"),
    makeCatalogModel("minimax-m2.7", "MiniMax-M2.7", "MiniMax-M2.7", "MiniMax", "anthropic_messages"),
    makeCatalogModel("minimax-m2.7-highspeed", "MiniMax-M2.7-Highspeed", "MiniMax-M2.7-highspeed", "MiniMax", "anthropic_messages"),
  ],
  qoder_cn: [
    makeCatalogModel("deepseek-v4-pro", "DeepSeek-V4-Pro", "dmodel", "DeepSeek", "openai_chat", 0.5),
    makeCatalogModel("deepseek-v4.1-flash", "DeepSeek-V4.1-Flash", "dfmodel", "DeepSeek", "openai_chat", 0.1),
    makeCatalogModel("glm-5.3", "GLM-5.3", "gmodel", "GLM", "openai_chat", 0.8),
    makeCatalogModel("glm-5.3-flash", "GLM-5.3-Flash", "gfmodel", "GLM", "openai_chat", 0.1),
    makeCatalogModel("kimi-k3.1-pro", "Kimi-K3.1-Pro", "kmodel_latest", "Kimi", "openai_chat", 1.0),
    makeCatalogModel("kimi-k2.8-preview", "Kimi-K2.8-Preview", "kmodel", "Kimi", "openai_chat", 0.8),
    makeCatalogModel("minimax-m3.1-flash", "MiniMax-M3.1-Flash", "mmodel", "MiniMax", "openai_chat", 0.1),
    makeCatalogModel("qwen4-max", "Qwen4-Max", "qmodel_latest", "Qwen", "openai_chat", 0.4),
    makeCatalogModel("qwen3.8-max", "Qwen3.8-Max", "qmodel_38max", "Qwen", "openai_chat", 0.5),
    makeCatalogModel("qwen3.8-flash", "Qwen3.8-Flash", "qfmodel", "Qwen", "openai_chat", 0.1),
    makeCatalogModel("step-5-preview", "Step-5-Preview", "jmodel", "Step", "openai_chat", 0.3),
    makeCatalogModel("fast", "Fast", "fast", "Other", "openai_chat", 0.6),
  ],
  trae_cn: [
    makeCatalogModel("deepseek-v4-pro", "DeepSeek-V4-Pro", "DeepSeek-V4-Pro", "DeepSeek", "openai_chat", 0.72),
    makeCatalogModel("deepseek-v4.1-flash", "DeepSeek-V4.1-Flash", "deepseek-v4.1-flash", "DeepSeek", "openai_chat", 0.12),
    makeCatalogModel("glm-5.3", "GLM-5.3", "glm-5.3", "GLM", "openai_chat", 0.78),
    makeCatalogModel("glm-5.3-flash", "GLM-5.3-Flash", "glm-5.3-flash", "GLM", "openai_chat", 0.08),
    makeCatalogModel("glm-5.2", "GLM-5.2", "glm-5.2", "GLM", "openai_chat", 0.78),
    makeCatalogModel("kimi-k3.1-pro", "Kimi-K3.1-Pro", "kimi-k3.1-pro", "Kimi", "openai_chat", 1.2),
    makeCatalogModel("kimi-k3", "Kimi-K3", "kimi-k3", "Kimi", "openai_chat", 1.83),
    makeCatalogModel("kimi-k2.8-preview", "Kimi-K2.8-Preview", "kimi-k2.8-preview", "Kimi", "openai_chat", 0.98),
    makeCatalogModel("kimi-k2.7-code", "Kimi-K2.7-Code", "kimi-k2.7-code", "Kimi", "openai_chat", 0.83),
    makeCatalogModel("minimax-m3.1-flash", "MiniMax-M3.1-Flash", "minimax-m3.1-flash", "MiniMax", "openai_chat", 0.09),
    makeCatalogModel("minimax-m3", "MiniMax-M3", "minimax-m3", "MiniMax", "openai_chat", 0.26),
    makeCatalogModel("qwen3.8-flash", "Qwen3.8-Flash", "qwen3.8-flash", "Qwen", "openai_chat", 0.08),
    makeCatalogModel("step-5-preview", "Step-5-Preview", "step-5-preview", "Step", "openai_chat", 0.48),
    makeCatalogModel("mimo-v2.6-pro", "MiMo-V2.6-Pro", "mimo-v2.6-pro", "MiMo", "openai_chat", 0.15),
    makeCatalogModel("doubao-seed-2.1-pro", "Doubao-Seed-2.1-Pro", "Doubao-Seed-2.1-Pro", "Doubao", "openai_chat", 0.8),
    makeCatalogModel("doubao-seed-2.1-turbo", "Doubao-Seed-2.1-Turbo", "Doubao-Seed-2.1-Turbo", "Doubao", "openai_chat", 0.2),
    makeCatalogModel("doubao-seed-code", "Doubao-Seed-Code", "Doubao-Seed-Code", "Doubao", "openai_chat", 0.06),
  ],
  lobsterai: [
    makeCatalogModel("deepseek-v4-pro", "DeepSeek-V4-Pro", "deepseek-v4-pro", "DeepSeek"),
    makeCatalogModel("deepseek-v4.1-flash", "DeepSeek-V4.1-Flash", "deepseek-v4.1-flash", "DeepSeek"),
    makeCatalogModel("glm-5.3", "GLM-5.3", "glm-5.3", "GLM"),
    makeCatalogModel("kimi-k3.1-pro", "Kimi-K3.1-Pro", "kimi-k3.1-pro", "Kimi"),
    makeCatalogModel("qwen4-max", "Qwen4-Max", "qwen4-max", "Qwen"),
  ],
  doubaowork: [
    makeCatalogModel("doubao-seed-2.1-pro", "Doubao-Seed-2.1-Pro", "Doubao-Seed-2.1-Pro", "Doubao"),
    makeCatalogModel("doubao-seed-code", "Doubao-Seed-Code", "Doubao-Seed-Code", "Doubao"),
    makeCatalogModel("deepseek-v4-pro", "DeepSeek-V4-Pro", "DeepSeek-V4-Pro", "DeepSeek"),
    makeCatalogModel("glm-5.3", "GLM-5.3", "GLM-5.3", "GLM"),
    makeCatalogModel("kimi-k3.1-pro", "Kimi-K3.1-Pro", "Kimi-K3.1-Pro", "Kimi"),
  ],
  codearts: [
    makeCatalogModel("deepseek-v4-pro", "DeepSeek-V4-Pro", "deepseek-v4-pro", "DeepSeek"),
    makeCatalogModel("glm-5.3", "GLM-5.3", "glm-5.3", "GLM"),
    makeCatalogModel("glm-5.2", "GLM-5.2", "glm-5.2", "GLM"),
    makeCatalogModel("openpangu-coder-pro", "openPangu-Coder-Pro", "openPangu-Coder-Pro", "Pangu"),
  ],
  custom_api_key: [
    makeCatalogModel("deepseek-v4-pro", "DeepSeek-V4-Pro", "deepseek-v4-pro", "DeepSeek"),
    makeCatalogModel("glm-5.3", "GLM-5.3", "glm-5.3", "GLM"),
    makeCatalogModel("deepseek-chat", "deepseek-chat", "deepseek-chat", "DeepSeek"),
    makeCatalogModel("claude-sonnet-4.6", "claude-sonnet-4.6", "claude-sonnet-4.6", "Claude", "anthropic_messages"),
  ],
};

function mergeFallbackModelIds(
  base: CatalogModelEntry[],
  extraIds: string[],
  toolId: string
): CatalogModelEntry[] {
  const seen = new Set(base.map((m) => m.id));
  const out = [...base];
  for (const id of extraIds) {
    if (!seen.has(id)) {
      seen.add(id);
      out.push(
        makeCatalogModel(
          id,
          id,
          id,
          "Other",
          toolId === "minimax_code_cn" ? "anthropic_messages" : "openai_chat"
        )
      );
    }
  }
  return out;
}

export function getCandidateModelOptions(
  candidate: UpstreamAccountCandidate
): CatalogModelEntry[] {
  const proto = (id: string): WireProtocolKind =>
    candidate.toolId === "minimax_code_cn" ||
    (candidate.toolId === "custom_api_key" && isAnthropicModelName(id))
      ? "anthropic_messages"
      : "openai_chat";

  if (candidate.modelOptions && candidate.modelOptions.length > 0) {
    return candidate.modelOptions.map((m) => ({
      ...m,
      label: m.displayName || m.id,
      protocol: proto(m.id),
    }));
  }
  const fallback = UPSTREAM_MODEL_OPTIONS[candidate.toolId] ?? [];
  return mergeFallbackModelIds(fallback, candidate.defaultModels ?? [], candidate.toolId);
}

export function getMemberModelOptions(
  member: Pick<PoolMember, "accountId" | "toolId" | "supportedModels">,
  candidates: UpstreamAccountCandidate[]
): CatalogModelEntry[] {
  const matched = candidates.find((c) => c.accountId === member.accountId);
  const base = matched
    ? getCandidateModelOptions(matched)
    : (UPSTREAM_MODEL_OPTIONS[member.toolId] ?? []);
  return mergeFallbackModelIds(base, member.supportedModels, member.toolId);
}

export function formatModelRateBadge(
  opt?: Pick<UpstreamModelOption, "creditMultiplier" | "isFree">
): string | null {
  if (!opt) return null;
  if (opt.isFree) return "免费";
  if (typeof opt.creditMultiplier === "number" && opt.creditMultiplier > 0) {
    const rounded = Number(opt.creditMultiplier.toFixed(2));
    return `${rounded}×`;
  }
  return null;
}

export const TOOL_DEFAULT_CHANNEL: Record<string, UpstreamChannel> = {
  minimax_code_cn: "rust_native",
  workbuddy: "opencode_bridge",
  qoder_cn: "opencode_bridge",
  trae_cn: "opencode_bridge",
  lobsterai: "opencode_bridge",
  doubaowork: "opencode_bridge",
  codearts: "opencode_bridge",
  custom_api_key: "api_key",
};

export const AVAILABLE_UPSTREAM_CANDIDATES: UpstreamAccountCandidate[] = import.meta.env.DEV ? [
  {
    accountId: "acc-wb-1",
    toolId: "workbuddy",
    label: "WorkBuddy · 主号",
    uidMasked: "wb_72a1****8e",
    channel: "opencode_bridge",
    defaultModels: ["deepseek-v4-pro", "glm-5.3", "kimi-k3.1-pro"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.workbuddy,
    defaultTier: "t1_primary",
    quotaPercent: 73,
    quotaText: "1,450 / 2,000 积分",
    isActiveOnClient: true,
    accountStatus: "active",
  },
  {
    accountId: "acc-wb-2",
    toolId: "workbuddy",
    label: "WorkBuddy · 副号",
    uidMasked: "wb_39f0****14",
    channel: "opencode_bridge",
    defaultModels: ["deepseek-v4-pro", "kimi-k3.1-pro", "glm-5.3"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.workbuddy,
    defaultTier: "t1_primary",
    quotaPercent: 62,
    quotaText: "620 / 1,000 积分",
    isActiveOnClient: false,
    accountStatus: "active",
  },
  {
    accountId: "acc-wb-3",
    toolId: "workbuddy",
    label: "WorkBuddy · 备用号 03",
    uidMasked: "wb_91c2****5a",
    channel: "opencode_bridge",
    defaultModels: ["hy4flash-free", "deepseek-v4.1-flash"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.workbuddy,
    defaultTier: "t2_backup",
    quotaPercent: 30,
    quotaText: "300 / 1,000 积分",
    isActiveOnClient: false,
    accountStatus: "risk_paused",
  },
  {
    accountId: "acc-minimax-1",
    toolId: "minimax_code_cn",
    label: "MiniMax Code CN · 主号",
    uidMasked: "mm_91c4****7e",
    channel: "rust_native",
    defaultModels: ["minimax-m3.1-flash", "minimax-m3"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.minimax_code_cn,
    defaultTier: "t1_primary",
    quotaPercent: 86,
    quotaText: "855 / 1,000 Credits",
    isActiveOnClient: true,
    accountStatus: "active",
  },
  {
    accountId: "acc-qoder-1",
    toolId: "qoder_cn",
    label: "Qoder · 日常主力",
    uidMasked: "qd_5e82****4d",
    channel: "opencode_bridge",
    defaultModels: ["deepseek-v4-pro", "glm-5.3", "qwen4-max"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.qoder_cn,
    defaultTier: "t2_backup",
    quotaPercent: 56,
    quotaText: "280 / 500 Credits",
    isActiveOnClient: true,
    accountStatus: "active",
  },
  {
    accountId: "acc-qoder-2",
    toolId: "qoder_cn",
    label: "Qoder · 实验号",
    uidMasked: "qd_2b44****9a",
    channel: "opencode_bridge",
    defaultModels: ["qwen4-max", "qwen3.8-max", "kimi-k3.1-pro"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.qoder_cn,
    defaultTier: "t1_primary",
    quotaPercent: 82,
    quotaText: "410 / 500 Credits",
    isActiveOnClient: false,
    accountStatus: "active",
  },
  {
    accountId: "acc-trae-1",
    toolId: "trae_cn",
    label: "Trae CN · 主力工作号",
    uidMasked: "u_8f3a****91c",
    channel: "opencode_bridge",
    defaultModels: ["deepseek-v4-pro", "glm-5.3", "doubao-seed-2.1-pro"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.trae_cn,
    defaultTier: "t1_primary",
    quotaPercent: 63,
    quotaText: "380 / 600 次",
    isActiveOnClient: true,
    accountStatus: "active",
  },
  {
    accountId: "acc-trae-2",
    toolId: "trae_cn",
    label: "Trae CN · 个人备用号",
    uidMasked: "u_4c19****02b",
    channel: "opencode_bridge",
    defaultModels: ["deepseek-v4-pro", "glm-5.3", "minimax-m3.1-flash"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.trae_cn,
    defaultTier: "t2_backup",
    quotaPercent: 42,
    quotaText: "42 / 100 次",
    isActiveOnClient: false,
    accountStatus: "active",
  },
  {
    accountId: "acc-lobster-1",
    toolId: "lobsterai",
    label: "LobsterAI · 主号",
    uidMasked: "17****38",
    channel: "opencode_bridge",
    defaultModels: ["deepseek-v4-pro", "deepseek-v4.1-flash", "glm-5.3"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.lobsterai,
    defaultTier: "t2_backup",
    quotaPercent: 86,
    quotaText: "2,580 / 3,000 积分",
    isActiveOnClient: true,
    accountStatus: "active",
  },
  {
    accountId: "acc-doubaowork-1",
    toolId: "doubaowork",
    label: "豆包工作 · 主号",
    uidMasked: "dw_71****9e",
    channel: "opencode_bridge",
    defaultModels: ["doubao-seed-2.1-pro", "doubao-seed-code"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.doubaowork,
    defaultTier: "t1_primary",
    quotaPercent: 88,
    quotaText: "880 / 1,000 额度",
    isActiveOnClient: true,
    accountStatus: "active",
  },
  {
    accountId: "acc-codearts-oauth-1",
    toolId: "codearts",
    label: "码道 CodeArts · 独立 OAuth 推理号",
    uidMasked: "ca_98e1****3d2",
    channel: "opencode_bridge",
    defaultModels: ["openpangu-coder-pro", "glm-5.3"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.codearts,
    defaultTier: "t2_backup",
    quotaPercent: 94,
    quotaText: "3,760 / 4,000 积分",
    isActiveOnClient: false,
    accountStatus: "active",
    codeArtsMode: "isolated_oauth",
  },
  {
    accountId: "acc-codearts-1",
    toolId: "codearts",
    label: "码道 CodeArts · 主号（客户端导入）",
    uidMasked: "ca_c02b****8f7",
    channel: "opencode_bridge",
    defaultModels: ["openpangu-coder-pro", "glm-5.3"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.codearts,
    defaultTier: "t2_backup",
    quotaPercent: 100,
    quotaText: "4,500 / 4,500 积分",
    isActiveOnClient: true,
    accountStatus: "active",
    codeArtsMode: "client_readonly",
  },
  {
    accountId: "acc-apikey-deepseek",
    toolId: "custom_api_key",
    label: "官方按量 API Key · 末位兜底",
    uidMasked: "sk-ds****91f4",
    channel: "api_key",
    defaultModels: ["deepseek-v4-pro", "glm-5.3", "deepseek-chat", "claude-sonnet-4.6"],
    modelOptions: UPSTREAM_MODEL_OPTIONS.custom_api_key,
    defaultTier: "t3_fallback",
    quotaPercent: 95,
    quotaText: "余额 ¥48.20",
    isActiveOnClient: false,
    accountStatus: "active",
  },
] : [];

export const POOL_SCENE_TEMPLATES: PoolSceneTemplate[] = import.meta.env.DEV ? [
  {
    id: "tpl-coding",
    title: "全栈 Coding 多模型池",
    sceneTag: "主力 Coding",
    description:
      "聚合多工具编程账号并显式暴露 DeepSeek-V4-Pro / GLM-5.3 / MiniMax-M3.1-Flash，严格按 T1→T2→T3 梯队 + 同工具水位调度",
    defaultName: "主力全栈 Coding 池",
    strategy: "quota_first",
    affinityMode: "auto_cache",
    skipActiveClientAccount: false,
    cooldownSeconds: 60,
    autoRefreshOn401: true,
    fallbackApiKeyEnabled: true,
    recommendedTools: ["workbuddy", "minimax_code_cn", "qoder_cn", "trae_cn", "custom_api_key"],
  },
  {
    id: "tpl-free-harvest",
    title: "每日签到免费收割池",
    sceneTag: "零成本收割",
    description: "串联多平台每日签到免费号，避开桌面活跃号，顺位耗尽免费积分",
    defaultName: "每日签到免费池",
    strategy: "drain",
    affinityMode: "auto_cache",
    skipActiveClientAccount: true,
    cooldownSeconds: 90,
    autoRefreshOn401: true,
    fallbackApiKeyEnabled: false,
    recommendedTools: ["minimax_code_cn", "lobsterai", "trae_cn", "qoder_cn"],
  },
  {
    id: "tpl-deep-reason",
    title: "深度推理与架构设计池",
    sceneTag: "深度推理",
    description: "暴露 Opus / Thinking / Pro 推理模型，整会话绑定保护思维链与缓存不中断",
    defaultName: "深度推理长文池",
    strategy: "order",
    affinityMode: "session",
    skipActiveClientAccount: false,
    cooldownSeconds: 60,
    autoRefreshOn401: true,
    fallbackApiKeyEnabled: true,
    recommendedTools: ["doubaowork", "workbuddy", "qoder_cn"],
  },
  {
    id: "tpl-blank",
    title: "空白自定义号池",
    sceneTag: "自定义",
    description: "自由挑选账号梯队（T1/T2/T3）与多选对外模型",
    defaultName: "自定义号池",
    strategy: "quota_first",
    affinityMode: "auto_cache",
    skipActiveClientAccount: false,
    cooldownSeconds: 60,
    autoRefreshOn401: true,
    fallbackApiKeyEnabled: false,
    recommendedTools: [],
  },
] : [];

export const SCOPED_TOOL_IDS = new Set([
  "workbuddy",
  "minimax_code_cn",
  "qoder_cn",
  "qoder",
  "trae_cn",
  "lobsterai",
  "doubaowork",
  "codearts",
  "custom_api_key",
]);

export const TOOL_SHORT_LABELS: Record<string, string> = {
  workbuddy: "WorkBuddy",
  minimax_code_cn: "MiniMax",
  qoder_cn: "Qoder CN",
  qoder: "Qoder CN",
  trae_cn: "Trae CN",
  lobsterai: "有道龙虾",
  doubaowork: "豆包工作",
  codearts: "码道 CodeArts",
  custom_api_key: "标准 API",
};

export function splitScopedModelId(raw: string): {
  toolId: string | null;
  modelId: string;
} {
  const trimmed = raw.trim();
  const slashIdx = trimmed.indexOf("/");
  if (slashIdx > 0) {
    const prefix = trimmed.slice(0, slashIdx);
    const rest = trimmed.slice(slashIdx + 1).trim();
    if (SCOPED_TOOL_IDS.has(prefix) && rest.length > 0) {
      return { toolId: prefix, modelId: rest };
    }
  }
  return { toolId: null, modelId: trimmed };
}

export function formatScopedModelId(toolId: string, modelId: string): string {
  const { modelId: bare } = splitScopedModelId(modelId);
  return `${toolId}/${bare}`;
}

export function getCatalogModelsForTool(
  toolId: string,
  candidates: UpstreamAccountCandidate[] = []
): CatalogModelEntry[] {
  const catalog = UPSTREAM_MODEL_OPTIONS[toolId] ?? [];
  const catalogMap = new Map(catalog.map((c) => [c.id, c]));
  const result: CatalogModelEntry[] = [];
  const seen = new Set<string>();

  for (const cand of candidates) {
    if (cand.toolId !== toolId || !cand.modelOptions) continue;
    for (const opt of cand.modelOptions) {
      if (seen.has(opt.id)) continue;
      seen.add(opt.id);
      const base = catalogMap.get(opt.id);
      result.push({
        id: opt.id,
        label: opt.displayName || base?.label || opt.id,
        displayName: opt.displayName || base?.displayName || opt.id,
        vendorWireId: opt.vendorWireId || base?.vendorWireId || opt.id,
        family: opt.family || base?.family || "Other",
        protocol:
          base?.protocol ??
          (toolId === "minimax_code_cn" ? "anthropic_messages" : "openai_chat"),
        creditMultiplier: opt.creditMultiplier ?? base?.creditMultiplier,
        isFree: opt.isFree ?? base?.isFree ?? false,
        source: opt.source || "live",
      });
    }
  }

  for (const item of catalog) {
    if (seen.has(item.id)) continue;
    seen.add(item.id);
    result.push(item);
  }
  return result;
}

export function isAnthropicModelName(modelId: string): boolean {
  return /claude|sonnet|opus|haiku|fable|mythos|anthropic/i.test(modelId);
}

export function buildClaudeModelAlias(poolSlug: string, modelId: string): string {
  const { toolId, modelId: bare } = splitScopedModelId(modelId);
  if (isAnthropicModelName(bare) && !toolId) {
    return bare;
  }
  const seed = `${poolSlug}/${modelId}`;
  let hash = 2166136261;
  for (let i = 0; i < seed.length; i++) {
    hash ^= seed.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  const suffix = (hash >>> 0).toString(16).padStart(6, "0").slice(0, 6);
  return `mythos-qwitch-${suffix}`;
}

export function syncPoolModelPolicies(
  slug: string,
  members: PoolMember[],
  existing: PoolModelPolicy[] = [],
  defaultEnabledIds?: Set<string>
): PoolModelPolicy[] {
  const existingMap = new Map(existing.map((p) => [p.modelId, p]));
  const uniqueModelIds: string[] = [];
  const seen = new Set<string>();
  for (const m of members) {
    for (const modelId of m.supportedModels) {
      if (!seen.has(modelId)) {
        seen.add(modelId);
        uniqueModelIds.push(modelId);
      }
    }
  }
  return uniqueModelIds.map((modelId) => {
    const prev = existingMap.get(modelId);
    if (prev) {
      return {
        ...prev,
        claudeAliasId: prev.claudeAliasId ?? buildClaudeModelAlias(slug, modelId),
      };
    }
    return {
      modelId,
      enabled: defaultEnabledIds ? defaultEnabledIds.has(modelId) : true,
      fallbackModelId: null,
      claudeAliasId: buildClaudeModelAlias(slug, modelId),
    };
  });
}

export function isHeterogeneousPool(members: PoolMember[]): boolean {
  const distinctTools = new Set(
    members.filter((m) => m.toolId !== "custom_api_key").map((m) => m.toolId)
  );
  return distinctTools.size > 1;
}

export function computePoolAggregateQuota(members: PoolMember[]): number {
  const active = members.filter(
    (m) =>
      m.toolId !== "custom_api_key" &&
      m.enabled &&
      m.accountStatus !== "risk_paused" &&
      m.accountStatus !== "expired"
  );
  if (active.length === 0) return 0;
  const sum = active.reduce((acc, m) => acc + m.quotaPercent, 0);
  return Math.round(sum / active.length);
}

export function computePoolServingAccounts(
  pool: AccountPool,
  modelId: string,
  nowMs = Date.now()
): {
  total: number;
  healthy: number;
  tierSummary: string;
} {
  const { toolId: scopedTid, modelId: bareModel } = splitScopedModelId(modelId);
  const serving = pool.members.filter(
    (m) =>
      (!scopedTid || m.toolId === scopedTid) &&
      m.supportedModels.some(
        (id) => id === modelId || splitScopedModelId(id).modelId === bareModel
      )
  );
  let healthy = 0;
  let t1 = 0;
  let t2 = 0;
  let t3 = 0;
  for (const m of serving) {
    if (m.tier === "t1_primary") t1 += 1;
    else if (m.tier === "t2_backup") t2 += 1;
    else t3 += 1;

    const cooled = typeof m.cooldownUntilMs === "number" && m.cooldownUntilMs > nowMs;
    const skipped = pool.skipActiveClientAccount && Boolean(m.isActiveOnClient);
    if (
      m.enabled &&
      m.accountStatus !== "risk_paused" &&
      m.accountStatus !== "expired" &&
      m.codeArtsMode !== "client_readonly" &&
      !cooled &&
      !skipped
    ) {
      healthy += 1;
    }
  }
  const parts: string[] = [];
  if (t1 > 0) parts.push(`T1×${t1}`);
  if (t2 > 0) parts.push(`T2×${t2}`);
  if (t3 > 0) parts.push(`T3×${t3}`);
  return {
    total: serving.length,
    healthy,
    tierSummary: parts.join(" · ") || "无成员",
  };
}

export function makeDefaultModelKey(poolId: string, modelId: string): string {
  return `${poolId}::${modelId}`;
}

/**
 * 汇总「实时探测到的模型展示名」，key 为 `工具/模型ID`。
 *
 * 静态目录里的展示名是人工维护的兜底值，可能与上游当前下发的名称不一致
 * （例如上游把 `kimi-k3` 改名为 `Kimi K3.1`）。实时探测结果优先。
 */
export function buildLiveModelLabels(
  candidates: UpstreamAccountCandidate[] = []
): Map<string, string> {
  const map = new Map<string, string>();
  for (const cand of candidates) {
    for (const opt of cand.modelOptions ?? []) {
      const name = opt.displayName?.trim();
      if (!name) continue;
      const key = `${cand.toolId}/${opt.id}`;
      const existing = map.get(key);
      if (!existing || opt.source === "live") {
        map.set(key, name);
      }
    }
  }
  return map;
}

export function computeAgentExposedModels(
  binding: Pick<AgentBinding, "agentId" | "mountedPoolIds" | "defaultModelKey">,
  allPools: AccountPool[],
  nowMs = Date.now(),
  liveModelLabels?: Map<string, string>
): AgentExposedModel[] {
  const mountedPools = binding.mountedPoolIds
    .map((id) => allPools.find((p) => p.id === id))
    .filter((p): p is AccountPool => Boolean(p && p.enabled));

  const rawItems: {
    pool: AccountPool;
    policy: PoolModelPolicy;
    serving: ReturnType<typeof computePoolServingAccounts>;
  }[] = [];

  for (const pool of mountedPools) {
    for (const policy of pool.modelPolicies) {
      if (!policy.enabled) continue;
      const serving = computePoolServingAccounts(pool, policy.modelId, nowMs);
      if (serving.total === 0) continue;
      rawItems.push({ pool, policy, serving });
    }
  }

  const modelCount = new Map<string, number>();
  for (const item of rawItems) {
    modelCount.set(item.policy.modelId, (modelCount.get(item.policy.modelId) ?? 0) + 1);
  }

  const keys = rawItems.map((item) => makeDefaultModelKey(item.pool.id, item.policy.modelId));
  const hasExplicitDefault =
    Boolean(binding.defaultModelKey) && keys.includes(binding.defaultModelKey as string);

  return rawItems.map(({ pool, policy, serving }, idx) => {
    const key = makeDefaultModelKey(pool.id, policy.modelId);
    const collision = (modelCount.get(policy.modelId) ?? 0) > 1;
    const { toolId: scopedTid, modelId: bareModel } = splitScopedModelId(policy.modelId);
    const humanLabel = scopedTid
      ? (liveModelLabels?.get(`${scopedTid}/${bareModel}`) ??
        UPSTREAM_MODEL_OPTIONS[scopedTid]?.find((o) => o.id === bareModel)?.displayName ??
        bareModel)
      : policy.modelId;
    const displayName = scopedTid
      ? collision
        ? `${TOOL_SHORT_LABELS[scopedTid] ?? scopedTid} · ${humanLabel} · ${pool.name}`
        : `${TOOL_SHORT_LABELS[scopedTid] ?? scopedTid} · ${humanLabel}`
      : collision
        ? `${policy.modelId} · ${pool.name}`
        : policy.modelId;
    const wireModelId =
      binding.agentId === "claude_desktop"
        ? scopedTid && isAnthropicModelName(bareModel) && !collision
          ? bareModel
          : policy.claudeAliasId ?? buildClaudeModelAlias(pool.slug, policy.modelId)
        : scopedTid
          ? collision
            ? `${pool.slug}/${scopedTid}/${bareModel}`
            : `${scopedTid}/${bareModel}`
          : `${pool.slug}/${policy.modelId}`;
    return {
      key,
      poolId: pool.id,
      poolName: pool.name,
      modelId: policy.modelId,
      displayName,
      wireModelId,
      isClaudeDisguised:
        binding.agentId === "claude_desktop" && wireModelId !== bareModel,
      accountCount: serving.total,
      healthyAccountCount: serving.healthy,
      tierSummary: serving.tierSummary,
      isDefault: hasExplicitDefault ? key === binding.defaultModelKey : idx === 0,
      collision,
    };
  });
}

export const INITIAL_GATEWAY_STATE: GatewayRuntimeState = {
  running: true,
  host: "127.0.0.1",
  port: 8187,
  bridgeRuntime: "bun_ready",
  bridgeVersion: "Bun 1.2.15",
  globalProxyDetected: true,
};

const POOL_1_MEMBERS: PoolMember[] = import.meta.env.DEV ? [
  {
    id: "pm-1",
    accountId: "acc-wb-1",
    toolId: "workbuddy",
    label: "WorkBuddy · 主号",
    uidMasked: "wb_72a1****8e",
    supportedModels: ["deepseek-v4-pro", "kimi-k3.1-pro", "glm-5.3"],
    tier: "t1_primary",
    channel: "opencode_bridge",
    enabled: true,
    priority: 1,
    quotaPercent: 73,
    quotaText: "1,450 / 2,000 积分",
    isActiveOnClient: true,
    accountStatus: "active",
    lastLatencyMs: 690,
  },
  {
    id: "pm-2",
    accountId: "acc-wb-2",
    toolId: "workbuddy",
    label: "WorkBuddy · 副号",
    uidMasked: "wb_39f0****14",
    supportedModels: ["deepseek-v4-pro", "kimi-k3.1-pro"],
    tier: "t1_primary",
    channel: "opencode_bridge",
    enabled: true,
    priority: 2,
    quotaPercent: 62,
    quotaText: "620 / 1,000 积分",
    isActiveOnClient: false,
    accountStatus: "active",
    lastLatencyMs: 740,
  },
  {
    id: "pm-3",
    accountId: "acc-minimax-1",
    toolId: "minimax_code_cn",
    label: "MiniMax Code CN · 主号",
    uidMasked: "mm_91c4****7e",
    supportedModels: ["minimax-m3.1-flash", "minimax-m3"],
    tier: "t1_primary",
    channel: "rust_native",
    enabled: true,
    priority: 3,
    quotaPercent: 86,
    quotaText: "855 / 1,000 Credits",
    isActiveOnClient: true,
    accountStatus: "active",
    lastLatencyMs: 410,
  },
  {
    id: "pm-4",
    accountId: "acc-qoder-1",
    toolId: "qoder_cn",
    label: "Qoder · 日常主力",
    uidMasked: "qd_5e82****4d",
    supportedModels: ["deepseek-v4-pro", "qwen4-max"],
    tier: "t2_backup",
    channel: "opencode_bridge",
    enabled: true,
    priority: 4,
    quotaPercent: 56,
    quotaText: "280 / 500 Credits",
    isActiveOnClient: true,
    accountStatus: "active",
    lastLatencyMs: 660,
  },
  {
    id: "pm-5",
    accountId: "acc-trae-2",
    toolId: "trae_cn",
    label: "Trae CN · 个人备用号",
    uidMasked: "u_4c19****02b",
    supportedModels: ["deepseek-v4-pro", "glm-5.3"],
    tier: "t2_backup",
    channel: "opencode_bridge",
    enabled: true,
    priority: 5,
    quotaPercent: 42,
    quotaText: "42 / 100 次",
    isActiveOnClient: false,
    accountStatus: "active",
    lastLatencyMs: 710,
  },
  {
    id: "pm-6",
    accountId: "acc-lobster-1",
    toolId: "lobsterai",
    label: "LobsterAI · 主号",
    uidMasked: "17****38",
    supportedModels: ["deepseek-v4-pro", "deepseek-v4.1-flash"],
    tier: "t2_backup",
    channel: "opencode_bridge",
    enabled: true,
    priority: 6,
    quotaPercent: 86,
    quotaText: "2,580 / 3,000 积分",
    isActiveOnClient: true,
    accountStatus: "active",
    lastLatencyMs: 540,
  },
  {
    id: "pm-7",
    accountId: "acc-codearts-oauth-1",
    toolId: "codearts",
    label: "码道 CodeArts · 独立 OAuth 推理号",
    uidMasked: "ca_98e1****3d2",
    supportedModels: ["openpangu-coder-pro", "glm-5.3"],
    tier: "t2_backup",
    channel: "opencode_bridge",
    enabled: true,
    priority: 7,
    quotaPercent: 94,
    quotaText: "3,760 / 4,000 积分",
    isActiveOnClient: false,
    accountStatus: "active",
    codeArtsMode: "isolated_oauth",
    lastLatencyMs: 640,
  },
  {
    id: "pm-8",
    accountId: "acc-apikey-deepseek",
    toolId: "custom_api_key",
    label: "官方按量 API Key · 末位兜底",
    uidMasked: "sk-ds****91f4",
    supportedModels: ["deepseek-v4-pro", "glm-5.3", "deepseek-chat"],
    tier: "t3_fallback",
    channel: "api_key",
    enabled: true,
    priority: 8,
    quotaPercent: 95,
    quotaText: "余额 ¥48.20",
    isActiveOnClient: false,
    accountStatus: "active",
    lastLatencyMs: 360,
  },
] : [];

const POOL_2_MEMBERS: PoolMember[] = import.meta.env.DEV ? [
  {
    id: "pm-dr-1",
    accountId: "acc-doubaowork-1",
    toolId: "doubaowork",
    label: "豆包工作 · 主号",
    uidMasked: "dw_71****9e",
    supportedModels: ["doubao-seed-2.1-pro", "doubao-seed-code"],
    tier: "t1_primary",
    channel: "opencode_bridge",
    enabled: true,
    priority: 1,
    quotaPercent: 88,
    quotaText: "880 / 1,000 额度",
    isActiveOnClient: true,
    accountStatus: "active",
    lastLatencyMs: 880,
  },
  {
    id: "pm-dr-2",
    accountId: "acc-qoder-2",
    toolId: "qoder_cn",
    label: "Qoder · 实验号",
    uidMasked: "qd_2b44****9a",
    supportedModels: ["qwen4-max"],
    tier: "t1_primary",
    channel: "opencode_bridge",
    enabled: true,
    priority: 2,
    quotaPercent: 82,
    quotaText: "410 / 500 Credits",
    isActiveOnClient: false,
    accountStatus: "active",
    lastLatencyMs: 740,
  },
  {
    id: "pm-dr-3",
    accountId: "acc-wb-2",
    toolId: "workbuddy",
    label: "WorkBuddy · 副号",
    uidMasked: "wb_39f0****14",
    supportedModels: ["kimi-k3.1-pro"],
    tier: "t1_primary",
    channel: "opencode_bridge",
    enabled: true,
    priority: 3,
    quotaPercent: 62,
    quotaText: "620 / 1,000 积分",
    isActiveOnClient: false,
    accountStatus: "active",
    lastLatencyMs: 920,
  },
] : [];

const POOL_3_MEMBERS: PoolMember[] = import.meta.env.DEV ? [
  {
    id: "pm-fd-1",
    accountId: "acc-trae-1",
    toolId: "trae_cn",
    label: "Trae CN · 主力工作号",
    uidMasked: "u_8f3a****91c",
    supportedModels: ["deepseek-v4.1-flash", "glm-5.3-flash"],
    tier: "t1_primary",
    channel: "opencode_bridge",
    enabled: true,
    priority: 1,
    quotaPercent: 63,
    quotaText: "380 / 600 次",
    isActiveOnClient: true,
    accountStatus: "active",
    lastLatencyMs: 710,
  },
  {
    id: "pm-fd-2",
    accountId: "acc-qoder-2",
    toolId: "qoder_cn",
    label: "Qoder · 实验号",
    uidMasked: "qd_2b44****9a",
    supportedModels: ["qwen3.8-flash"],
    tier: "t1_primary",
    channel: "opencode_bridge",
    enabled: true,
    priority: 2,
    quotaPercent: 82,
    quotaText: "410 / 500 Credits",
    isActiveOnClient: false,
    accountStatus: "active",
    lastLatencyMs: 650,
  },
  {
    id: "pm-fd-3",
    accountId: "acc-wb-3",
    toolId: "workbuddy",
    label: "WorkBuddy · 备用号 03",
    uidMasked: "wb_91c2****5a",
    supportedModels: ["hy4flash-free", "deepseek-v4.1-flash"],
    tier: "t2_backup",
    channel: "opencode_bridge",
    enabled: true,
    priority: 3,
    quotaPercent: 30,
    quotaText: "300 / 1,000 积分",
    isActiveOnClient: false,
    accountStatus: "risk_paused",
    lastLatencyMs: 950,
  },
] : [];

export const INITIAL_POOLS: AccountPool[] = import.meta.env.DEV ? [
  {
    id: "pool-coding-main",
    slug: "coding-main",
    name: "主力全栈 Coding 池",
    description:
      "覆盖 7 大内置工具 + T3 付费兜底：跨工具标准模型归一化，网关严格按 T1→T2→T3 与同工具水位自动选号并还原底层 Wire ID",
    sceneTag: "主力 Coding",
    enabled: true,
    strategy: "quota_first",
    affinityMode: "auto_cache",
    skipActiveClientAccount: false,
    cooldownSeconds: 60,
    autoRefreshOn401: true,
    fallbackApiKeyEnabled: true,
    modelPolicies: syncPoolModelPolicies(
      "coding-main",
      POOL_1_MEMBERS,
      [],
      new Set([
        "deepseek-v4-pro",
        "kimi-k3.1-pro",
        "minimax-m3.1-flash",
        "glm-5.3",
        "openpangu-coder-pro",
      ])
    ),
    members: POOL_1_MEMBERS,
    todayRequests: 148,
    todayFailovers: 7,
    cacheHitRate: 84,
    createdAt: "2026-11-20T09:00:00+08:00",
  },
  {
    id: "pool-deep-reason",
    slug: "deep-reason",
    name: "深度推理与长文池",
    description:
      "开放 doubao-seed-2.1-pro / qwen4-max / kimi-k3.1-pro，开启整会话亲和防止跨账号思维签名失效",
    sceneTag: "深度推理",
    enabled: true,
    strategy: "drain",
    affinityMode: "session",
    skipActiveClientAccount: false,
    cooldownSeconds: 60,
    autoRefreshOn401: true,
    fallbackApiKeyEnabled: false,
    modelPolicies: syncPoolModelPolicies(
      "deep-reason",
      POOL_2_MEMBERS,
      [],
      new Set(["doubao-seed-2.1-pro", "qwen4-max", "kimi-k3.1-pro"])
    ),
    members: POOL_2_MEMBERS,
    todayRequests: 52,
    todayFailovers: 2,
    cacheHitRate: 91,
    createdAt: "2026-11-21T14:20:00+08:00",
  },
  {
    id: "pool-fast-daily",
    slug: "fast-daily",
    name: "每日签到免费收割池",
    description:
      "优先消耗 hy4flash-free / Flash 免费与低倍率短效额度，自动跳过本机客户端正在使用的活跃账号防互踢",
    sceneTag: "免费收割",
    enabled: true,
    strategy: "order",
    affinityMode: "auto_cache",
    skipActiveClientAccount: true,
    cooldownSeconds: 90,
    autoRefreshOn401: true,
    fallbackApiKeyEnabled: false,
    modelPolicies: syncPoolModelPolicies(
      "fast-daily",
      POOL_3_MEMBERS,
      [],
      new Set(["deepseek-v4.1-flash", "qwen3.8-flash", "hy4flash-free"])
    ),
    members: POOL_3_MEMBERS,
    todayRequests: 31,
    todayFailovers: 3,
    cacheHitRate: 76,
    createdAt: "2026-11-22T08:30:00+08:00",
  },
] : [];

export function buildAgentConfigSnippets(
  agentId: TargetAgentId,
  mountedPoolIds: string[],
  defaultModelKey: string | null,
  allPools: AccountPool[],
  gateway: GatewayRuntimeState,
  options: AgentBinding["options"],
  isDrifted: boolean
): { beforeSnippet: string; afterSnippet: string } {
  const baseUrl = `http://${gateway.host}:${gateway.port}`;
  const exposed = computeAgentExposedModels(
    { agentId, mountedPoolIds, defaultModelKey },
    allPools
  );
  const defaultItem = exposed.find((e) => e.isDefault) ?? exposed[0] ?? null;
  const fallbackWireModel = defaultItem?.wireModelId ?? "coding-main/deepseek-v4-pro";

  switch (agentId) {
    case "claude_desktop": {
      const targetDir = options.claudeIsolatedTwin
        ? "~/Library/Application Support/Claude-Qwitch-3p"
        : "~/Library/Application Support/Claude-3p";
      const modelMapComment =
        exposed.length > 0
          ? exposed
              .map(
                (m) =>
                  `//   ${m.isDefault ? "★ " : ""}${m.wireModelId} -> display_name: "${m.displayName}" (${m.tierSummary})`
              )
              .join("\n")
          : "//   （当前未挂载号池）";
      return {
        beforeSnippet: `// ${targetDir}/claude_desktop_config.json\n{\n  "deploymentMode": "cloud",\n  "mcpServers": { "filesystem": { "command": "npx" } }\n}`,
        afterSnippet: `// ${targetDir}/claude_desktop_config.json\n{\n  "deploymentMode": "3p",\n  "mcpServers": { "filesystem": { "command": "npx" } }\n}\n\n// ${targetDir}/configLibrary/00000000-0000-4000-8000-717769746368.json\n{\n  "inferenceProvider": "gateway",\n  "inferenceGatewayBaseUrl": "${baseUrl}",\n  "inferenceGatewayApiKey": "qwitch-bind-claude-desktop",\n  "inferenceGatewayAuthScheme": "bearer",\n  "disableDeploymentModeChooser": true,\n  "coworkEgressAllowedHosts": ["*"],\n  "toolSearchEnabled": true\n}\n// GET /v1/models 暴露清单（共 ${exposed.length} 个模型，非 Anthropic 名自动正则伪装）:\n${modelMapComment}`,
      };
    }
    case "codex_desktop": {
      const catalogComment =
        exposed.length > 0
          ? exposed
              .map((m) => `#   - ${m.wireModelId} (${m.displayName}, ${m.tierSummary})`)
              .join("\n")
          : "#   - (未挂载号池)";
      return {
        beforeSnippet: `# ~/.codex/config.toml\nmodel = "gpt-5.4"\n# 未写入 Qwitch provider`,
        afterSnippet: `# ~/.codex/config.toml\n# qwitch:managed-start\nmodel_provider = "qwitch"\nmodel = "${fallbackWireModel}"\nmodel_catalog_json = "~/.codex/qwitch_models.json"\n# qwitch:managed-end\n\n[model_providers.qwitch]\nname = "Qwitch Multi-Pool Gateway"\nbase_url = "${baseUrl}/v1"\nwire_api = "responses"\nexperimental_bearer_token = "qwitch-bind-codex-desktop"\nhttp_headers = { "x-openai-actor-authorization" = "qwitch" }\nrequires_openai_auth = true\n# 关联 Catalog 暴露的模型清单（支持在 Codex 内直接切换）:\n${catalogComment}\n# 注：解绑时仅还原顶层 model_provider，保留此表防历史线程加载崩溃`,
      };
    }
    case "dsh": {
      const beforeCreds = isDrifted
        ? `# ~/.dsh/.credentials.yaml  [⚠️ 检测缺失 QWITCH_GATEWAY_KEY]\nrefs:\n  DEEPSEEK_API_KEY: "sk-orig-***"`
        : `# ~/.dsh/.credentials.yaml\nrefs:\n  DEEPSEEK_API_KEY: "sk-orig-***"`;
      const modelsYaml =
        exposed.length > 0
          ? exposed
              .map(
                (m) =>
                  `          - id: "${m.wireModelId}"\n            name: "${m.displayName}"\n            contextWindow: 200000\n            maxTokens: 64000\n            input: [text, image]`
              )
              .join("\n")
          : `          - id: "${fallbackWireModel}"\n            name: "Qwitch Default"\n            contextWindow: 200000\n            maxTokens: 64000\n            input: [text, image]`;
      return {
        beforeSnippet: `# ~/.dsh/profiles/desktop/cordis.patch.yml\n- id: llm-deepseek\n  config:\n    model: deepseek-chat\n\n${beforeCreds}`,
        afterSnippet: `# ~/.dsh/profiles/desktop/cordis.patch.yml\n- id: llm-pi-ai\n  name: "@deepseek-ai/dsh-llm-pi-ai"\n  config:\n    providers:\n      qwitch:\n        displayName: Qwitch\n        apiKeyEnv: QWITCH_GATEWAY_KEY\n        api: openai-completions\n        baseURL: "${baseUrl}/v1"\n        models:\n${modelsYaml}\n        streamIdleTimeoutMs: 600000\n\n# ~/.dsh/.env & ~/.dsh/.credentials.yaml（桌面端强依赖）\nversion: 1\nrefs:\n  QWITCH_GATEWAY_KEY: "qwitch-bind-dsh"`,
      };
    }
    case "opencode": {
      if (options.opencodeSchemaVersion === "v1") {
        const modelsObj = Object.fromEntries(
          exposed.map((m) => [
            m.wireModelId,
            {
              name: m.displayName,
              limit: { context: 200000, output: 64000 },
              modalities: { input: ["text", "image"], output: ["text"] },
              tool_call: true,
            },
          ])
        );
        return {
          beforeSnippet: `// ~/.config/opencode/opencode.jsonc\n{\n  "model": "deepseek/deepseek-chat"\n}`,
          afterSnippet: `// ~/.config/opencode/opencode.jsonc (OpenCode v1)\n${JSON.stringify(
            {
              model: `qwitch/${fallbackWireModel}`,
              provider: {
                qwitch: {
                  npm: "@ai-sdk/openai-compatible",
                  name: "Qwitch 号池网关",
                  options: {
                    baseURL: `${baseUrl}/v1`,
                    apiKey: "qwitch-bind-opencode",
                  },
                  models: modelsObj,
                },
              },
            },
            null,
            2
          )}`,
        };
      }
      const modelsMapV2 = Object.fromEntries(
        exposed.map((m) => [
          m.wireModelId,
          {
            name: m.displayName,
            capabilities: {
              tools: true,
              input: ["text", "image"],
              output: ["text"],
            },
            limit: { context: 200000, output: 64000 },
          },
        ])
      );
      return {
        beforeSnippet: `// ~/.config/opencode/opencode.jsonc\n{\n  "model": "deepseek/deepseek-chat"\n}`,
        afterSnippet: `// ~/.config/opencode/opencode.jsonc (OpenCode v2 providers)\n${JSON.stringify(
          {
            model: `qwitch/${fallbackWireModel}`,
            providers: {
              qwitch: {
                name: "Qwitch 号池网关",
                package: "aisdk:@ai-sdk/openai-compatible",
                settings: {
                  baseURL: `${baseUrl}/v1`,
                  apiKey: "qwitch-bind-opencode",
                },
                models: modelsMapV2,
              },
            },
          },
          null,
          2
        )}`,
      };
    }
    case "mimo": {
      return {
        beforeSnippet: `// ~/.config/mimocode/mimocode.jsonc`,
        afterSnippet: `// ~/.config/mimocode/mimocode.jsonc\n${JSON.stringify(
          {
            model: `qwitch/${fallbackWireModel}`,
            provider: {
              qwitch: {
                npm: "@ai-sdk/openai-compatible",
                name: "Qwitch 号池网关",
                options: { baseURL: `${baseUrl}/v1`, apiKey: "qwitch-bind-mimo" },
                models: {},
              },
            },
          },
          null,
          2
        )}`,
      };
    }
    case "zcode": {
      return {
        beforeSnippet: `// ~/.zcode/v2/config.json`,
        afterSnippet: `// ~/.zcode/v2/config.json\n${JSON.stringify(
          {
            provider: {
              qwitch: {
                name: "Qwitch 号池网关",
                kind: "anthropic",
                options: { apiKey: "qwitch-bind-zcode", baseURL: baseUrl },
                enabled: true,
                source: "custom",
              },
            },
          },
          null,
          2
        )}`,
      };
    }
  }
}

export const TARGET_AGENT_ORDER: readonly TargetAgentId[] = [
  "dsh",
  "opencode",
  "codex_desktop",
  "claude_desktop",
  "mimo",
  "zcode",
];

export function createInitialAgentBindings(
  pools: AccountPool[],
  gateway: GatewayRuntimeState
): AgentBinding[] {
  const raw: Omit<AgentBinding, "beforeSnippet" | "afterSnippet">[] = [
    {
      agentId: "dsh",
      name: "DeepSeek Harness",
      subtitle: "dsh 桌面端与 Web 代理 · 写入 llm-pi-ai 自定义路由",
      protocol: "openai_chat",
      configPath: "~/.dsh/profiles/desktop/cordis.patch.yml",
      installed: true,
      running: true,
      version: "0.2.1",
      status: "drifted",
      mountedPoolIds: ["pool-deep-reason"],
      defaultModelKey: makeDefaultModelKey("pool-deep-reason", "doubao-seed-2.1-pro"),
      driftReason:
        "检测到 ~/.dsh/.credentials.yaml 缺失 QWITCH_GATEWAY_KEY（仅写入了 .env，dsh 桌面版不读取 .env 会报鉴权缺失）",
      lastHitAt: "1 小时前",
      options: {
        dshDualCredsWritten: false,
      },
    },
    {
      agentId: "opencode",
      name: "OpenCode",
      subtitle: "CLI / TUI / OpenChamber 编码代理 · 支持多号池同列切换",
      protocol: "openai_chat",
      configPath: "~/.config/opencode/opencode.jsonc",
      installed: true,
      running: true,
      version: "2.2.6",
      status: "bound",
      mountedPoolIds: ["pool-coding-main", "pool-fast-daily"],
      defaultModelKey: makeDefaultModelKey("pool-coding-main", "deepseek-v4-pro"),
      lastHitAt: "12 分钟前",
      options: {
        opencodeSchemaVersion: "v2",
      },
    },
    {
      agentId: "codex_desktop",
      name: "Codex 桌面版",
      subtitle: "ChatGPT / Codex App · 独立 [model_providers.qwitch] Responses 端点",
      protocol: "openai_responses",
      configPath: "~/.codex/config.toml",
      installed: true,
      running: true,
      version: "26.1021",
      status: "unbound",
      mountedPoolIds: [],
      defaultModelKey: null,
      proxyWarning: gateway.globalProxyDetected
        ? "检测到系统开启 HTTP 全局代理，Codex 内置 reqwest 默认不读 macOS 忽略列表，直连 127.0.0.1 可能返回 502（建议设置 NO_PROXY=127.0.0.1,localhost）"
        : undefined,
      options: {
        codexPreserveTable: true,
      },
    },
    {
      agentId: "claude_desktop",
      name: "Claude 桌面版",
      subtitle: "官方 3p 网关模式 · 同时挂载多号池，下拉框直选具体模型",
      protocol: "anthropic_messages",
      configPath: "~/Library/Application Support/Claude-3p/configLibrary/",
      installed: true,
      running: true,
      version: "2.7032.0",
      status: "bound",
      mountedPoolIds: ["pool-coding-main", "pool-deep-reason"],
      defaultModelKey: makeDefaultModelKey("pool-coding-main", "deepseek-v4-pro"),
      lastHitAt: "3 分钟前",
      options: {
        claudeIsolatedTwin: false,
      },
    },
    {
      agentId: "mimo",
      name: "MiMo Code",
      subtitle: "小米 MiMo / MiMo Code · OpenCode 兼容 provider，直连本地网关",
      protocol: "openai_chat",
      configPath: "~/.config/mimocode/mimocode.jsonc",
      installed: true,
      running: true,
      version: "MiMo Code",
      status: "unbound",
      mountedPoolIds: [],
      defaultModelKey: null,
      options: { opencodeSchemaVersion: "v1" },
    },
    {
      agentId: "zcode",
      name: "ZCode",
      subtitle: "ZCode 桌面版 · Anthropic Messages 自定义 provider",
      protocol: "anthropic_messages",
      configPath: "~/.zcode/v2/config.json",
      installed: true,
      running: true,
      version: "ZCode",
      status: "unbound",
      mountedPoolIds: [],
      defaultModelKey: null,
      options: {},
    },
  ];

  return raw.map((item) => {
    const snippets = buildAgentConfigSnippets(
      item.agentId,
      item.mountedPoolIds,
      item.defaultModelKey,
      pools,
      gateway,
      item.options,
      item.status === "drifted"
    );
    return {
      ...item,
      ...snippets,
    };
  });
}

export const INITIAL_ROUTING_TRACES: RoutingTraceItem[] = import.meta.env.DEV ? [
  {
    id: "tr-1008",
    timestamp: "13:42:18",
    agentId: "claude_desktop",
    poolId: "pool-coding-main",
    poolName: "主力全栈 Coding 池",
    incomingProtocol: "anthropic_messages",
    upstreamProtocol: "openai_chat",
    requestedModel: "deepseek-v4-pro",
    finalAccountLabel: "WorkBuddy · 副号",
    finalToolId: "workbuddy",
    finalModel: "deepseek-v4-pro",
    status: "failover_success",
    cacheAffinityHit: false,
    ttftMs: 740,
    totalTokens: 14280,
    attempts: [
      {
        accountLabel: "WorkBuddy · 主号",
        toolId: "workbuddy",
        model: "deepseek-v4-pro",
        tier: "t1_primary",
        outcome: "429_cooldown",
        latencyMs: 195,
        detail: "T1 首号并发限流 HTTP 429，置入 60s 冷却；严格锁定 deepseek-v4-pro 切换下一候选",
      },
      {
        accountLabel: "WorkBuddy · 副号",
        toolId: "workbuddy",
        model: "deepseek-v4-pro",
        tier: "t1_primary",
        outcome: "ok",
        latencyMs: 545,
        detail: "T1 备选号无感接管成功（同工具水位 62%），模型保持 deepseek-v4-pro 未漂移",
      },
    ],
  },
  {
    id: "tr-1007",
    timestamp: "13:38:04",
    agentId: "claude_desktop",
    poolId: "pool-coding-main",
    poolName: "主力全栈 Coding 池",
    incomingProtocol: "anthropic_messages",
    upstreamProtocol: "openai_chat",
    requestedModel: "deepseek-v4-pro",
    finalAccountLabel: "WorkBuddy · 主号",
    finalToolId: "workbuddy",
    finalModel: "deepseek-v4-pro",
    status: "success",
    cacheAffinityHit: true,
    ttftMs: 490,
    totalTokens: 28400,
    attempts: [
      {
        accountLabel: "WorkBuddy · 主号",
        toolId: "workbuddy",
        model: "deepseek-v4-pro",
        tier: "t1_primary",
        outcome: "ok",
        latencyMs: 490,
        detail: "命中会话亲和缓存（2m14s 内同会话），经 OpenCode 桥接内存注入 Token",
      },
    ],
  },
  {
    id: "tr-1006",
    timestamp: "13:31:52",
    agentId: "opencode",
    poolId: "pool-coding-main",
    poolName: "主力全栈 Coding 池",
    incomingProtocol: "openai_chat",
    upstreamProtocol: "anthropic_messages",
    requestedModel: "minimax-m3.1-flash",
    finalAccountLabel: "MiniMax Code CN · 主号",
    finalToolId: "minimax_code_cn",
    finalModel: "minimax-m3.1-flash",
    status: "failover_success",
    cacheAffinityHit: false,
    ttftMs: 595,
    totalTokens: 9640,
    attempts: [
      {
        accountLabel: "MiniMax Code CN · 主号",
        toolId: "minimax_code_cn",
        model: "minimax-m3.1-flash",
        tier: "t1_primary",
        outcome: "401_refreshed",
        latencyMs: 185,
        detail: "AccessToken 临期 401，RefreshSingleFlight 单飞刷新并加密写回 qwitch.db",
      },
      {
        accountLabel: "MiniMax Code CN · 主号",
        toolId: "minimax_code_cn",
        model: "minimax-m3.1-flash",
        tier: "t1_primary",
        outcome: "ok",
        latencyMs: 410,
        detail: "已转换为底层 MiniMax-M3.1-Flash-Preview，经 Rust 原生通道原地重试成功",
      },
    ],
  },
  {
    id: "tr-1005",
    timestamp: "13:19:40",
    agentId: "claude_desktop",
    poolId: "pool-coding-main",
    poolName: "主力全栈 Coding 池",
    incomingProtocol: "anthropic_messages",
    upstreamProtocol: "openai_chat",
    requestedModel: `openpangu-coder-pro (${buildClaudeModelAlias("coding-main", "openpangu-coder-pro")})`,
    finalAccountLabel: "码道 CodeArts · 独立 OAuth 推理号",
    finalToolId: "codearts",
    finalModel: "openpangu-coder-pro",
    status: "success",
    cacheAffinityHit: true,
    ttftMs: 640,
    totalTokens: 18910,
    attempts: [
      {
        accountLabel: "码道 CodeArts · 独立 OAuth 推理号",
        toolId: "codearts",
        model: "openpangu-coder-pro",
        tier: "t2_backup",
        outcome: "ok",
        latencyMs: 640,
        detail: "还原 Claude 伪装 ID，使用隔离 OAuth 会话（不与客户端一次性刷新令牌互斥）",
      },
    ],
  },
  {
    id: "tr-1004",
    timestamp: "12:55:11",
    agentId: "dsh",
    poolId: "pool-deep-reason",
    poolName: "深度推理与长文池",
    incomingProtocol: "openai_chat",
    upstreamProtocol: "openai_chat",
    requestedModel: "doubao-seed-2.1-pro",
    finalAccountLabel: "豆包工作 · 主号",
    finalToolId: "doubaowork",
    finalModel: "doubao-seed-2.1-pro",
    status: "success",
    cacheAffinityHit: true,
    ttftMs: 840,
    totalTokens: 32100,
    attempts: [
      {
        accountLabel: "豆包工作 · 主号",
        toolId: "doubaowork",
        model: "doubao-seed-2.1-pro",
        tier: "t1_primary",
        outcome: "ok",
        latencyMs: 840,
        detail: "单号耗尽策略 + 整会话绑定命中，经 OpenCode 桥接流式输出思考链",
      },
    ],
  },
  {
    id: "tr-1003",
    timestamp: "12:14:09",
    agentId: "opencode",
    poolId: "pool-fast-daily",
    poolName: "每日签到免费收割池",
    incomingProtocol: "openai_chat",
    upstreamProtocol: "openai_chat",
    requestedModel: "deepseek-v4.1-flash",
    finalAccountLabel: "Trae CN · 主力工作号",
    finalToolId: "trae_cn",
    finalModel: "deepseek-v4.1-flash",
    status: "success",
    cacheAffinityHit: false,
    ttftMs: 710,
    totalTokens: 11520,
    attempts: [
      {
        accountLabel: "Trae CN · 主力工作号",
        toolId: "trae_cn",
        model: "deepseek-v4.1-flash",
        tier: "t1_primary",
        outcome: "ok",
        latencyMs: 710,
        detail: "复用快照内置账号级设备指纹 (9390****7536)，未篡改宿主机标识",
      },
    ],
  },
  {
    id: "tr-1002",
    timestamp: "11:48:33",
    agentId: "opencode",
    poolId: "pool-fast-daily",
    poolName: "每日签到免费收割池",
    incomingProtocol: "openai_chat",
    upstreamProtocol: "openai_chat",
    requestedModel: "qwen3.8-flash",
    finalAccountLabel: "Qoder · 实验号",
    finalToolId: "qoder_cn",
    finalModel: "qwen3.8-flash",
    status: "success",
    cacheAffinityHit: false,
    ttftMs: 650,
    totalTokens: 6420,
    attempts: [
      {
        accountLabel: "Qoder · 实验号",
        toolId: "qoder_cn",
        model: "qwen3.8-flash",
        tier: "t1_primary",
        outcome: "ok",
        latencyMs: 650,
        detail: "自动映射为 Qoder 底层槽位 qfmodel，顺位命中每日签到免费 Credits 账号",
      },
    ],
  },
  {
    id: "tr-1001",
    timestamp: "11:02:47",
    agentId: "claude_desktop",
    poolId: "pool-coding-main",
    poolName: "主力全栈 Coding 池",
    incomingProtocol: "anthropic_messages",
    upstreamProtocol: "openai_chat",
    requestedModel: "deepseek-v4-pro",
    finalAccountLabel: "官方按量 API Key · 末位兜底",
    finalToolId: "custom_api_key",
    finalModel: "deepseek-v4-pro",
    status: "failover_success",
    cacheAffinityHit: false,
    ttftMs: 550,
    totalTokens: 7850,
    attempts: [
      {
        accountLabel: "WorkBuddy · 主号",
        toolId: "workbuddy",
        model: "deepseek-v4-pro",
        tier: "t1_primary",
        outcome: "429_cooldown",
        latencyMs: 190,
        detail: "T1 WorkBuddy 并发限流 429；T2 Qoder (dmodel) / Trae 同步冷却中",
      },
      {
        accountLabel: "官方按量 API Key · 末位兜底",
        toolId: "custom_api_key",
        model: "deepseek-v4-pro",
        tier: "t3_fallback",
        outcome: "ok",
        latencyMs: 360,
        detail: "T1/T2 免费订阅全冷却后触发 T3 末位兜底，保持 deepseek-v4-pro 连续响应",
      },
    ],
  },
] : [];
