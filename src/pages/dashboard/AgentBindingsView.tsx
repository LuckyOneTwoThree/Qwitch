import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ArrowDown,
  ArrowDownWideNarrow,
  ArrowUp,
  Check,
  CheckCircle2,
  CheckSquare,
  Copy,
  CopyCheck,
  Crown,
  FileCode2,
  Flame,
  PanelLeftClose,
  PanelLeftOpen,
  Puzzle,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldAlert,
  Sparkles,
  ShieldCheck,
  SlidersHorizontal,
  Square,
  Star,
  Terminal,
  Trash2,
  Unlink,
  Wrench,
  X,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import {
  TOOL_SHORT_LABELS,
  WIRE_PROTOCOL_META,
  buildAgentConfigSnippets,
  buildLiveModelLabels,
  computeAgentExposedModels,
  formatScopedModelId,
  splitScopedModelId,
} from "../../data/mockPoolData";
import type { AgentBinding, PoolStrategy, TargetAgentId } from "../../types/pool";
import type { CompatibilityReport } from "../../types/plugin";
import { AgentBrandIcon } from "../../components/AgentBrandIcon";
import { ToolBrandIcon } from "../../components/ToolBrandIcon";
import { CompatibilityBadge } from "../../components/plugins/CompatibilityBadge";
import { ConfirmDialog } from "../../components/ui/ConfirmDialog";
import { OverflowMenu } from "../../components/ui/OverflowMenu";
import { GatewayPortModal } from "../../components/gateway/GatewayPortModal";
import { ConfigDiffModal, type DiffActionKind } from "../../components/pool/ConfigDiffModal";
import {
  gatewayUnsupportedReason,
  isToolGatewayRoutable,
  isToolScopeReadonly,
} from "../../domain/toolScope";
import { formatModelSpec } from "../../domain/modelMeta";

type ModelLike = { id: string; isFree?: boolean; creditMultiplier?: number };

/// 与 Rust `recommended_model_options_for_tool` 保持一致的「有内置推荐目录」工具集
const TOOLS_WITH_PRESET = new Set(["codearts", "lobsterai", "doubaowork"]);

function isLowRateModel(opt: ModelLike) {
  return (
    Boolean(opt.isFree) ||
    (typeof opt.creditMultiplier === "number" && opt.creditMultiplier <= 0.25) ||
    /flash|turbo|mini|fast/i.test(opt.id)
  );
}

function isFlagshipModel(opt: ModelLike) {
  return (
    (typeof opt.creditMultiplier === "number" && opt.creditMultiplier >= 0.45) ||
    /pro|max|k3|seed-2\.1|pangu|claude/i.test(opt.id)
  );
}

/** 去掉展示名里的「工具短名 · 」前缀，配合左侧品牌图标，避免模型名过长 */
function shortModelLabel(name: string): string {
  const parts = name.split(" · ");
  return parts.length > 1 ? parts.slice(1).join(" · ") : name;
}

const DEFAULT_TOOL_ORDER = [
  "workbuddy",
  "qoder_cn",
  "trae_cn",
  "minimax_code_cn",
  "lobsterai",
  "doubaowork",
  "codearts",
  "custom_api_key",
] as const;

type ModelQuickFilter = "all" | "checked" | "low_rate" | "flagship";
type AgentSubTab = "models" | "scheduling" | "connection";

const AGENT_STRATEGY_OPTIONS: ReadonlyArray<{
  id: PoolStrategy;
  shortLabel: string;
  desc: string;
}> = [
  {
    id: "order",
    shortLabel: "主号优先 · 自动切备",
    desc: "优先固定消耗 #1 主账号（稳保 Prompt Cache）；429 限流或耗尽时自动顺延 #2 备用账号",
  },
  {
    id: "quota_first",
    shortLabel: "剩余额度优先",
    desc: "在已启用账号中自动路由至实时剩余额度百分比最高的账号",
  },
  {
    id: "drain",
    shortLabel: "锁定消耗当前主号",
    desc: "持续使用 #1 主账号直到耗尽，需人工切换时可随时点击「设为主号」",
  },
];

function renderStatusBadge(status: AgentBinding["status"], compact = false) {
  const padClass = compact ? "px-1.5 py-0.5 text-[11px]" : "";
  switch (status) {
    case "bound":
      return (
        <span
          className={`badge border-emerald-500/30 bg-emerald-500/12 text-emerald-800 dark:text-emerald-300 ${padClass}`}
        >
          <CheckCircle2 className="h-3 w-3 shrink-0" aria-hidden="true" />
          已挂载并同步
        </span>
      );
    case "drifted":
      return (
        <span
          className={`badge border-amber-500/35 bg-amber-500/15 text-amber-800 dark:text-amber-300 ${padClass}`}
        >
          <AlertTriangle className="h-3 w-3 shrink-0" aria-hidden="true" />
          配置漂移
        </span>
      );
    case "needs_restart":
      return (
        <span
          className={`badge border-sky-500/30 bg-sky-500/12 text-sky-800 dark:text-sky-300 ${padClass}`}
        >
          <RotateCcw className="h-3 w-3 shrink-0" aria-hidden="true" />
          待重启生效
        </span>
      );
    case "unbound":
      return <span className={`badge badge-neutral ${padClass}`}>未挂载（原生运行）</span>;
  }
}

function agentStatusMeta(status: AgentBinding["status"]): {
  dotClass: string;
  label: string;
} {
  switch (status) {
    case "bound":
      return { dotClass: "bg-emerald-500", label: "已挂载并同步" };
    case "drifted":
      return { dotClass: "bg-amber-500", label: "配置漂移" };
    case "needs_restart":
      return { dotClass: "bg-sky-500", label: "待重启生效" };
    case "unbound":
      return { dotClass: "bg-zinc-400", label: "未挂载（原生运行）" };
  }
}

export function AgentBindingsView({
  highlightAgentId,
  onOpenPluginSettings,
}: {
  highlightAgentId?: TargetAgentId | null;
  onOpenPluginSettings?: () => void;
}) {
  const {
    gateway,
    pools,
    candidates,
    agentBindings,
    nowMs,
    isRefreshingModels,
    refreshUpstreamModels,
    cycleBridgeRuntime,
    toggleProxySimulation,
    updateAgentMountedPools,
    unbindAgent,
    clearAgentWiring,
    clearAllAgentWiring,
    repairAgentDrift,
    restartAgentClient,
    restartingAgentId,
    refreshAgentRunning,
    toggleAgentOption,
    injectMemberCooldown,
    clearMemberCooldown,
    authorizeCodeArtsIsolated,
    applyAccountPresetModels,
    getAgentToolModelOptions,
    getAgentToolEnabledModelIds,
    getAgentToolAccounts,
    getAgentPrimaryPool,
    setAgentToolModels,
    selectAllAgentToolModels,
    selectLowRateAgentToolModels,
    invertAgentToolModels,
    clearAgentToolModels,
    setAgentDefaultToolModel,
    promoteAgentToolPrimaryAccount,
    reorderAgentToolAccount,
    toggleAgentToolAccountEnabled,
    setAllAgentToolAccountsEnabled,
    sortAgentToolAccountsByQuota,
    updateAgentToolRoutingOptions,
    syncToolScheduleToAllAgents,
    copyAgentToolModelPlan,
    syncAgentToolModelPlanToAllAgents,
    getAgentConfiguredToolIds,
    copyAgentAllToolModelPlans,
    syncAllToolModelPlansToAllAgents,
  } = usePoolPrototype();

  const isTauriRuntime =
    typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

  const [selectedAgentId, setSelectedAgentId] = useState<TargetAgentId>(
    highlightAgentId ?? agentBindings[0]?.agentId ?? "dsh"
  );
  const [subTab, setSubTab] = useState<AgentSubTab>("models");
  const [selectedToolId, setSelectedToolId] = useState<string>("workbuddy");
  const [modelSearch, setModelSearch] = useState<string>("");
  const [modelQuickFilter, setModelQuickFilter] = useState<ModelQuickFilter>("all");
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [diffModal, setDiffModal] = useState<{
    binding: AgentBinding;
    actionKind: DiffActionKind;
    nextPoolIds?: string[];
    mountedPoolNames: string[];
    defaultModelDisplay?: string | null;
    beforeText: string;
    afterText: string;
  } | null>(null);
  const [clearConfirm, setClearConfirm] = useState<{
    mode: "restore" | "force";
    agentId?: TargetAgentId;
  } | null>(null);
  const [portModalOpen, setPortModalOpen] = useState(false);
  const [compatMap, setCompatMap] = useState<Record<string, CompatibilityReport>>({});
  const [agentRailCollapsed, setAgentRailCollapsed] = useState(true);
  const [agentSearch, setAgentSearch] = useState("");
  const [agentStatusFilter, setAgentStatusFilter] = useState<
    "all" | "bound" | "drifted" | "unbound"
  >("all");

  useEffect(() => {
    if (highlightAgentId) {
      setSelectedAgentId(highlightAgentId);
    }
  }, [highlightAgentId]);

  // 每 6s 轻量刷新各目标应用运行状态，使按钮在「启动 / 重启」间自动切换（隐藏至托盘时暂停轮询）
  useEffect(() => {
    refreshAgentRunning();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") {
        refreshAgentRunning();
      }
    }, 6000);
    const handleVisibility = () => {
      if (document.visibilityState === "visible") {
        refreshAgentRunning();
      }
    };
    document.addEventListener("visibilitychange", handleVisibility);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", handleVisibility);
    };
  }, [refreshAgentRunning]);

  const activeBinding = useMemo(
    () =>
      agentBindings.find((b) => b.agentId === selectedAgentId) ??
      agentBindings[0] ??
      null,
    [agentBindings, selectedAgentId]
  );

  // 动态收敛工具顺序：有账号的工具排在前面，方便一眼点选
  const orderedToolIds = useMemo(() => {
    const accountCounts = new Map<string, number>();
    for (const c of candidates) {
      accountCounts.set(c.toolId, (accountCounts.get(c.toolId) ?? 0) + 1);
    }
    const baseSet = new Set<string>(DEFAULT_TOOL_ORDER);
    for (const tid of accountCounts.keys()) {
      baseSet.add(tid);
    }
    return Array.from(baseSet).sort((a, b) => {
      const ca = accountCounts.get(a) ?? 0;
      const cb = accountCounts.get(b) ?? 0;
      if ((ca > 0) !== (cb > 0)) return ca > 0 ? -1 : 1;
      const ia = DEFAULT_TOOL_ORDER.indexOf(a as (typeof DEFAULT_TOOL_ORDER)[number]);
      const ib = DEFAULT_TOOL_ORDER.indexOf(b as (typeof DEFAULT_TOOL_ORDER)[number]);
      if (ia !== -1 && ib !== -1) return ia - ib;
      if (ia !== -1) return -1;
      if (ib !== -1) return 1;
      return a.localeCompare(b);
    });
  }, [candidates]);

  const activeToolId = useMemo(
    () =>
      orderedToolIds.includes(selectedToolId)
        ? selectedToolId
        : (orderedToolIds[0] ?? "workbuddy"),
    [orderedToolIds, selectedToolId]
  );

  useEffect(() => {
    const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
    if (!isTauri || !activeBinding?.agentId || orderedToolIds.length === 0) return;
    let cancelled = false;
    invoke<Record<string, CompatibilityReport>>("check_agent_egress_compatibility_cmd", {
      agentId: activeBinding.agentId,
      egressPluginIds: orderedToolIds,
    })
      .then((res) => {
        if (!cancelled) setCompatMap(res);
      })
      .catch(() => {
        if (!cancelled) setCompatMap({});
      });
    return () => {
      cancelled = true;
    };
  }, [activeBinding?.agentId, orderedToolIds]);

  // 实时探测到的模型展示名优先于静态兜底目录
  const liveModelLabels = useMemo(() => buildLiveModelLabels(candidates), [candidates]);

  const activeExposedModels = useMemo(() => {
    if (!activeBinding) return [];
    return computeAgentExposedModels(activeBinding, pools, nowMs, liveModelLabels);
  }, [activeBinding, pools, nowMs, liveModelLabels]);

  const defaultExposedItem = useMemo(
    () => activeExposedModels.find((m) => m.isDefault) ?? activeExposedModels[0] ?? null,
    [activeExposedModels]
  );

  const activeAgentPool = useMemo(
    () => (activeBinding ? getAgentPrimaryPool(activeBinding.agentId) : null),
    [activeBinding, getAgentPrimaryPool]
  );

  const toolModelOptions = useMemo(
    () => getAgentToolModelOptions(activeToolId),
    [activeToolId, getAgentToolModelOptions]
  );

  const enabledBareModelIds = useMemo(
    () =>
      activeBinding
        ? getAgentToolEnabledModelIds(activeBinding.agentId, activeToolId)
        : [],
    [activeBinding, activeToolId, getAgentToolEnabledModelIds]
  );

  const enabledModelSet = useMemo(
    () => new Set(enabledBareModelIds),
    [enabledBareModelIds]
  );

  // 可作为「方案来源」的其他 Agent：同一工具下已经勾过模型，才有复制价值
  const modelPlanCopySources = useMemo(() => {
    if (!activeBinding) return [];
    return agentBindings
      .filter((b) => b.agentId !== activeBinding.agentId)
      .map((b) => ({
        agentId: b.agentId,
        name: b.name,
        modelCount: getAgentToolEnabledModelIds(b.agentId, activeToolId).length,
        toolCount: getAgentConfiguredToolIds(b.agentId).length,
      }))
      .filter((s) => s.modelCount > 0)
      .sort((a, b) => b.modelCount - a.modelCount);
  }, [
    activeBinding,
    activeToolId,
    agentBindings,
    getAgentConfiguredToolIds,
    getAgentToolEnabledModelIds,
  ]);

  // 当前 Agent 已经配过模型的订阅工具（决定「全部订阅」批量同步的范围）
  const configuredToolIds = useMemo(
    () => (activeBinding ? getAgentConfiguredToolIds(activeBinding.agentId) : []),
    [activeBinding, getAgentConfiguredToolIds]
  );

  // 全局范围（跨订阅）的「方案来源」：不依赖当前选中的是哪个订阅
  const modelPlanGlobalSources = useMemo(() => {
    if (!activeBinding) return [];
    return agentBindings
      .filter((b) => b.agentId !== activeBinding.agentId)
      .map((b) => ({
        agentId: b.agentId,
        name: b.name,
        toolCount: getAgentConfiguredToolIds(b.agentId).length,
      }))
      .filter((s) => s.toolCount > 0)
      .sort((a, b) => b.toolCount - a.toolCount);
  }, [activeBinding, agentBindings, getAgentConfiguredToolIds]);

  const filteredToolModels = useMemo(() => {
    const q = modelSearch.trim().toLowerCase();
    return toolModelOptions.filter((opt) => {
      if (modelQuickFilter === "checked" && !enabledModelSet.has(opt.id)) {
        return false;
      }
      if (modelQuickFilter === "low_rate" && !isLowRateModel(opt)) {
        return false;
      }
      if (modelQuickFilter === "flagship" && !isFlagshipModel(opt)) {
        return false;
      }
      if (!q) return true;
      return `${opt.displayName} ${opt.id} ${opt.vendorWireId} ${opt.family}`
        .toLowerCase()
        .includes(q);
    });
  }, [toolModelOptions, modelQuickFilter, enabledModelSet, modelSearch]);

  const filterCounts = useMemo(() => {
    let lowRate = 0;
    let flagship = 0;
    for (const opt of toolModelOptions) {
      if (isLowRateModel(opt)) {
        lowRate += 1;
      }
      if (isFlagshipModel(opt)) {
        flagship += 1;
      }
    }
    return {
      all: toolModelOptions.length,
      checked: enabledBareModelIds.length,
      lowRate,
      flagship,
    };
  }, [toolModelOptions, enabledBareModelIds.length]);

  const toolAccounts = useMemo(
    () =>
      activeBinding
        ? getAgentToolAccounts(activeBinding.agentId, activeToolId)
        : [],
    [activeBinding, activeToolId, getAgentToolAccounts]
  );
  // 标准 API（custom_api_key）是「通用 API Key」兜底工具：未配置该账号时，
  // 模型勾选 / 调度编排不会产生任何路由效果——必须明示，而不是摆出完整交互
  const standardApiUnconfigured =
    activeToolId === "custom_api_key" && (toolAccounts?.length ?? 0) === 0;

  const [customModelWire, setCustomModelWire] = useState<string>("");

  const customExportModels = useMemo(() => {
    if (activeExposedModels.length > 0) {
      return activeExposedModels.map((m) => ({
        wireId: m.wireModelId,
        label: m.displayName,
      }));
    }
    return toolModelOptions.slice(0, 6).map((m) => ({
      wireId: formatScopedModelId(activeToolId, m.id),
      label: `${TOOL_SHORT_LABELS[activeToolId] ?? activeToolId} · ${m.displayName}`,
    }));
  }, [activeExposedModels, toolModelOptions, activeToolId]);

  const effectiveCustomWire = useMemo(() => {
    if (customExportModels.some((m) => m.wireId === customModelWire)) {
      return customModelWire;
    }
    return (
      defaultExposedItem?.wireModelId ??
      customExportModels[0]?.wireId ??
      "workbuddy/deepseek-v4-pro"
    );
  }, [customExportModels, customModelWire, defaultExposedItem]);

  const copyText = (id: string, text: string) => {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(text).catch(() => {});
    }
    setCopiedId(id);
    window.setTimeout(() => {
      setCopiedId((prev) => (prev === id ? null : prev));
    }, 1500);
  };

  const openDiffForMountChange = (binding: AgentBinding, nextPoolIds: string[]) => {
    const useRealUnbindSnippets =
      nextPoolIds.length === 0 &&
      Boolean(binding.beforeSnippet?.trim()) &&
      Boolean(binding.afterSnippet?.trim());
    const snippets = useRealUnbindSnippets
      ? {
          beforeSnippet: binding.beforeSnippet,
          afterSnippet: binding.afterSnippet,
        }
      : buildAgentConfigSnippets(
          binding.agentId,
          nextPoolIds,
          binding.defaultModelKey,
          pools,
          gateway,
          binding.options,
          false
        );
    const exposed = computeAgentExposedModels(
      {
        agentId: binding.agentId,
        mountedPoolIds: nextPoolIds,
        defaultModelKey: binding.defaultModelKey,
      },
      pools,
      nowMs,
      liveModelLabels
    );
    const defaultItem = exposed.find((e) => e.isDefault) ?? exposed[0] ?? null;
    setDiffModal({
      binding,
      actionKind: nextPoolIds.length === 0 ? "unbind" : "update_mounts",
      nextPoolIds,
      mountedPoolNames: nextPoolIds
        .map((id) => pools.find((p) => p.id === id)?.name)
        .filter((n): n is string => Boolean(n)),
      defaultModelDisplay: defaultItem?.displayName ?? null,
      beforeText: snippets.beforeSnippet,
      afterText: snippets.afterSnippet,
    });
  };

  const noProxyCmd = "export NO_PROXY=127.0.0.1,localhost,::1";
  const resolvedGatewayToken =
    activeBinding?.gatewayToken ??
    `qwitch-bind-${(activeBinding?.agentId ?? "custom").replace(/_/g, "-")}`;
  const customEndpointEnv = `export OPENAI_BASE_URL=http://${gateway.host}:${gateway.port}/v1
export OPENAI_API_KEY=${resolvedGatewayToken}
# 直接指定订阅工具与模型（严格锁模不跨工具混调）：${effectiveCustomWire}
# 或 Anthropic Messages 协议:
export ANTHROPIC_BASE_URL=http://${gateway.host}:${gateway.port}
export ANTHROPIC_AUTH_TOKEN=${resolvedGatewayToken}`;

  const activeToolName = TOOL_SHORT_LABELS[activeToolId] ?? activeToolId;
  // 该工具的账号是否全部为「本机只读凭据」：这类账号不参与网关，勾选模型不会生效
  const toolAccountsForScope = candidates.filter((c) => c.toolId === activeToolId);
  const scopeReadonly = isToolScopeReadonly(candidates, activeToolId);
  // 网关侧根本没有该工具的上游实现（码道是 AK/SK 签名，无 bearer 令牌）
  const gatewayUnsupported = !isToolGatewayRoutable(activeToolId);
  // 只有「网关根本没有该工具的上游」才会让勾选失去落点；
  // 只读凭据现在也能调度（码道走签名上游），不再阻止勾选
  const scopeNotSelectable = gatewayUnsupported;
  const presetAvailable = TOOLS_WITH_PRESET.has(activeToolId);

  // 模型目录的可信度必须如实展示：实时 / 本机客户端 / 内置预设 / 手工 / 未探测
  const modelSourceBadge = (() => {
    const sources = new Set(toolModelOptions.map((o) => o.source));
    if (toolModelOptions.length === 0) return { label: "未探测", tone: "warn" as const };
    if (sources.has("live")) {
      return {
        label: sources.size === 1 ? "实时接口探测" : "部分实时",
        tone: "ok" as const,
      };
    }
    if (sources.has("client")) {
      return {
        label: sources.size === 1 ? "本机客户端目录" : "部分来自客户端",
        tone: "ok" as const,
      };
    }
    if (sources.has("preset")) return { label: "内置预设", tone: "info" as const };
    if (sources.has("cache")) return { label: "本机缓存目录", tone: "info" as const };
    return { label: "未探测（静态兜底）", tone: "warn" as const };
  })();
  const currentStrategy: PoolStrategy = activeAgentPool?.strategy ?? "order";
  const currentSkipActive = activeAgentPool?.skipActiveClientAccount ?? true;
  const currentCooldownSec = activeAgentPool?.cooldownSeconds ?? 60;

  const exposedByAgent = useMemo(() => {
    const map = new Map<string, ReturnType<typeof computeAgentExposedModels>>();
    for (const b of agentBindings) {
      map.set(b.agentId, computeAgentExposedModels(b, pools, nowMs));
    }
    return map;
  }, [agentBindings, pools, nowMs]);

  const activeStrategyDesc =
    AGENT_STRATEGY_OPTIONS.find((opt) => opt.id === currentStrategy)?.desc ??
    AGENT_STRATEGY_OPTIONS[0].desc;

  const renderToolSwitcher = () => {
    if (!activeBinding) return null;
    return (
      <section className="shrink-0 border-b border-zinc-200/80 bg-zinc-50/55 px-3.5 py-2 dark:border-zinc-800/80 dark:bg-zinc-950/35">
        <div
          role="tablist"
          aria-label="订阅工具切换栏"
          className="flex flex-wrap items-center gap-1"
        >
          {orderedToolIds.map((toolId) => {
            const isActive = toolId === activeToolId;
            const toolName = TOOL_SHORT_LABELS[toolId] ?? toolId;
            const totalToolModels = getAgentToolModelOptions(toolId).length;
            const selectedToolCount = getAgentToolEnabledModelIds(
              activeBinding.agentId,
              toolId
            ).length;
            const toolAccs = getAgentToolAccounts(activeBinding.agentId, toolId);
            const hasDefaultHere =
              defaultExposedItem &&
              splitScopedModelId(defaultExposedItem.modelId).toolId === toolId;
            const compat = compatMap[toolId];
            const compatNote =
              compat?.level === "compatible"
                ? "协议兼容"
                : compat?.level === "degraded"
                  ? "降级可用"
                  : compat?.level === "incompatible"
                    ? "不兼容"
                    : null;
            const tip = [
              toolName,
              toolAccs.length > 0 ? `${toolAccs.length} 账号` : "未导入账号",
              `已选 ${selectedToolCount}/${totalToolModels} 模型`,
              compatNote,
            ]
              .filter(Boolean)
              .join(" · ");

            return (
              <button
                key={toolId}
                type="button"
                role="tab"
                aria-selected={isActive}
                title={tip}
                onClick={() => {
                  setSelectedToolId(toolId);
                  setModelSearch("");
                  setModelQuickFilter("all");
                }}
                className={`flex items-center gap-1.5 whitespace-nowrap rounded-lg px-2 py-1 text-xs transition ${
                  isActive
                    ? "bg-brand-500/12 font-semibold text-brand-900 ring-1 ring-brand-500/35 shadow-2xs dark:bg-brand-400/15 dark:text-brand-100 dark:ring-brand-400/35"
                    : selectedToolCount > 0
                      ? "bg-emerald-500/[0.07] text-zinc-800 ring-1 ring-emerald-500/20 hover:bg-emerald-500/[0.12] dark:text-zinc-200"
                      : "bg-white/80 text-zinc-600 ring-1 ring-zinc-200/80 hover:bg-zinc-100 dark:bg-zinc-900/60 dark:text-zinc-400 dark:ring-zinc-800 dark:hover:bg-zinc-800"
                }`}
              >
                <ToolBrandIcon toolId={toolId} size="xs" decorative />
                <span>{toolName}</span>
                {compat?.level === "compatible" ? (
                  <span
                    aria-hidden="true"
                    className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-500"
                  />
                ) : compat ? (
                  <CompatibilityBadge report={compat} />
                ) : null}
                {hasDefaultHere && (
                  <Star
                    className="h-3 w-3 shrink-0 fill-amber-500 text-amber-500"
                    aria-label="包含默认模型"
                  />
                )}
                <span
                  className={`text-[11px] font-normal tabular-nums ${
                    selectedToolCount > 0
                      ? "font-semibold text-brand-700 dark:text-brand-300"
                      : "text-zinc-400"
                  }`}
                >
                  {selectedToolCount}/{totalToolModels}
                </span>
              </button>
            );
          })}
        </div>
      </section>
    );
  };

  return (
    <div className="surface-card flex flex-col overflow-hidden @3xl:h-full @3xl:min-h-0">
      {/* 顶部固定网关状态栏（整页工作台顶栏，不随左右面板滚动） */}
      <section className="flex shrink-0 flex-wrap items-center justify-between gap-2.5 border-b border-zinc-200/80 bg-zinc-50/80 px-3.5 py-2 dark:border-zinc-800 dark:bg-zinc-950/60">
        <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs">
          <div className="flex items-center gap-1.5">
            <span
              aria-hidden="true"
              className={`h-2 w-2 rounded-full ${
                gateway.running ? "bg-emerald-500" : "bg-zinc-400"
              }`}
            />
            <span className="font-semibold text-zinc-900 dark:text-zinc-100">
              本地网关
            </span>
            <button
              type="button"
              onClick={() => setPortModalOpen(true)}
              title="修改网关端口 / 检测占用（完整批量重启见系统设置）"
              className="rounded bg-zinc-100 px-1.5 py-0.5 font-mono text-[11px] tabular-nums text-zinc-700 transition hover:bg-zinc-200 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700"
            >
              {gateway.host}:{gateway.port}
            </button>
          </div>

          {isTauriRuntime ? (
            <>
              <span
                className={`badge ${
                  gateway.bridgeRuntime === "missing"
                    ? "border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300"
                    : "badge-neutral"
                }`}
              >
                插件桥：{gateway.bridgeVersion}
              </span>
              <span
                className={`badge ${
                  gateway.globalProxyDetected
                    ? "border-amber-500/35 bg-amber-500/12 text-amber-800 dark:text-amber-300"
                    : "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                }`}
              >
                {gateway.globalProxyDetected ? (
                  <>
                    <AlertTriangle className="h-3 w-3 shrink-0" aria-hidden="true" />
                    全局代理
                  </>
                ) : (
                  <>
                    <Check className="h-3 w-3 shrink-0" aria-hidden="true" />
                    回环正常
                  </>
                )}
              </span>
            </>
          ) : (
            <>
              <button
                type="button"
                onClick={cycleBridgeRuntime}
                title="切换 OpenCode 插件桥接检测态"
                className={`badge cursor-pointer ${
                  gateway.bridgeRuntime === "missing"
                    ? "border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300"
                    : "badge-neutral"
                }`}
              >
                插件桥：{gateway.bridgeVersion}
              </button>
              <button
                type="button"
                onClick={toggleProxySimulation}
                title="切换系统全局代理检测态"
                className={`badge cursor-pointer ${
                  gateway.globalProxyDetected
                    ? "border-amber-500/35 bg-amber-500/12 text-amber-800 dark:text-amber-300"
                    : "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                }`}
              >
                {gateway.globalProxyDetected ? (
                  <>
                    <AlertTriangle className="h-3 w-3 shrink-0" aria-hidden="true" />
                    全局代理
                  </>
                ) : (
                  <>
                    <Check className="h-3 w-3 shrink-0" aria-hidden="true" />
                    回环正常
                  </>
                )}
              </button>
            </>
          )}
        </div>

        <div className="flex min-w-0 flex-1 items-center justify-end gap-2">
          <span className="hidden min-w-0 truncate text-[11px] text-zinc-500 @2xl:inline dark:text-zinc-400">
            选 Agent → 配模型 → 调主备顺位
          </span>
          <button
            type="button"
            onClick={() => setClearConfirm({ mode: "restore" })}
            className="btn btn-xs btn-danger shrink-0 gap-1"
            title="清除所有 Agent 的 Qwitch 配置：有备份则还原，无备份则强制清除 Qwitch 写入"
          >
            <Trash2 className="h-3 w-3" aria-hidden="true" />
            全部清除
          </button>
        </div>
      </section>

      {/* 左右主从分栏主体：宽窗左右独立滚动，窄窗自动降级为顶部单行横向胶囊条 */}
      <div
        className={`flex min-h-0 flex-1 flex-col @3xl:grid ${
          agentRailCollapsed
            ? "@3xl:grid-cols-[56px_minmax(0,1fr)]"
            : "@3xl:grid-cols-[232px_minmax(0,1fr)] @5xl:grid-cols-[248px_minmax(0,1fr)]"
        }`}
      >
        <aside
          aria-label="目标 Agent 客户端列表"
          className="flex flex-col border-b border-zinc-200/80 bg-zinc-50/50 p-2 dark:border-zinc-800 dark:bg-zinc-950/40 @3xl:min-h-0 @3xl:border-b-0 @3xl:border-r"
        >
          {/* 桌面端左侧栏标题与折叠开关 */}
          <div
            className={`hidden items-center border-b border-zinc-200/70 pb-2 dark:border-zinc-800/80 @3xl:flex ${
              agentRailCollapsed ? "justify-center" : "justify-between gap-1.5"
            }`}
          >
            {!agentRailCollapsed && (
              <div className="flex min-w-0 items-center gap-1.5">
                <span className="truncate text-xs font-semibold text-zinc-800 dark:text-zinc-200">
                  接入端 Agent
                </span>
                <span className="rounded bg-zinc-100 px-1.5 py-0.5 text-[11px] font-medium tabular-nums text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400">
                  {agentBindings.length}
                </span>
              </div>
            )}
            <button
              type="button"
              onClick={() => setAgentRailCollapsed((v) => !v)}
              aria-label={agentRailCollapsed ? "展开 Agent 列表" : "收起 Agent 列表"}
              title={agentRailCollapsed ? "展开 Agent 列表" : "收起为图标轨"}
              className="btn btn-icon btn-ghost h-6 w-6 shrink-0"
            >
              {agentRailCollapsed ? (
                <PanelLeftOpen className="h-3.5 w-3.5" aria-hidden="true" />
              ) : (
                <PanelLeftClose className="h-3.5 w-3.5" aria-hidden="true" />
              )}
            </button>
          </div>

          {/* 桌面端展开态：搜索与状态筛选 */}
          {!agentRailCollapsed && (
            <div className="mt-2 hidden space-y-1.5 @3xl:block">
              <div className="relative">
                <Search
                  className="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-zinc-400"
                  aria-hidden="true"
                />
                <input
                  type="text"
                  value={agentSearch}
                  onChange={(e) => setAgentSearch(e.target.value)}
                  aria-label="筛选 Agent 客户端"
                  placeholder="搜索 Agent…"
                  className="field-input w-full py-1 pl-6 pr-6 text-xs"
                />
                {agentSearch && (
                  <button
                    type="button"
                    aria-label="清除 Agent 搜索"
                    onClick={() => setAgentSearch("")}
                    className="absolute right-1.5 top-1/2 -translate-y-1/2 text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200"
                  >
                    <X className="h-3 w-3" aria-hidden="true" />
                  </button>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-1">
                {(
                  [
                    { id: "all", label: "全部" },
                    { id: "bound", label: "已挂载" },
                    { id: "drifted", label: "漂移" },
                    { id: "unbound", label: "未挂载" },
                  ] as const
                ).map((item) => {
                  const driftedCount =
                    item.id === "drifted"
                      ? agentBindings.filter((b) => b.status === "drifted").length
                      : 0;
                  return (
                    <button
                      key={item.id}
                      type="button"
                      aria-pressed={agentStatusFilter === item.id}
                      onClick={() => setAgentStatusFilter(item.id)}
                      className={`rounded-md px-1.5 py-0.5 text-[11px] font-medium transition ${
                        agentStatusFilter === item.id
                          ? "bg-brand-500/12 text-brand-900 ring-1 ring-brand-500/30 dark:bg-brand-400/15 dark:text-brand-100"
                          : "text-zinc-600 hover:bg-zinc-100 dark:text-zinc-400 dark:hover:bg-zinc-800"
                      }`}
                    >
                      {item.label}
                      {driftedCount > 0 ? ` (${driftedCount})` : ""}
                    </button>
                  );
                })}
              </div>
            </div>
          )}

          {/* 列表主体：窄窗单行横向滚动 + 固定插件入口；宽窗纵向列表 */}
          <div className="flex items-center gap-2 @3xl:mt-2 @3xl:min-h-0 @3xl:flex-1 @3xl:flex-col @3xl:items-stretch">
            <div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto pb-0.5 @3xl:flex-col @3xl:items-stretch @3xl:overflow-x-visible @3xl:overflow-y-auto @3xl:pb-0">
              {agentBindings.length === 0 && (
                <div className="px-2 py-2 text-xs text-zinc-500 dark:text-zinc-400">
                  尚未加载到目标 Agent，请点击顶部「重新探测」
                </div>
              )}
              {(() => {
                const visibleBindings = agentRailCollapsed
                  ? agentBindings
                  : agentBindings.filter((b) => {
                      if (agentStatusFilter !== "all" && b.status !== agentStatusFilter) {
                        return false;
                      }
                      const q = agentSearch.trim().toLowerCase();
                      if (!q) return true;
                      const protoLabel = WIRE_PROTOCOL_META[b.protocol].label.toLowerCase();
                      return (
                        b.name.toLowerCase().includes(q) ||
                        b.agentId.toLowerCase().includes(q) ||
                        protoLabel.includes(q)
                      );
                    });

                if (agentBindings.length > 0 && visibleBindings.length === 0) {
                  return (
                    <div className="hidden px-2 py-4 text-center text-xs text-zinc-400 @3xl:block">
                      无匹配的 Agent
                    </div>
                  );
                }

                return visibleBindings.map((binding) => {
                  const isSelected = activeBinding?.agentId === binding.agentId;
                  const exposed = exposedByAgent.get(binding.agentId) ?? [];
                  const defModel = exposed.find((m) => m.isDefault) ?? exposed[0];
                  const statusMeta = agentStatusMeta(binding.status);
                  const tipParts = [
                    binding.name,
                    WIRE_PROTOCOL_META[binding.protocol].label,
                    statusMeta.label,
                    defModel ? `默认: ${shortModelLabel(defModel.displayName)}` : "未配置模型",
                    `${exposed.length} 模型`,
                  ];

                  return (
                    <section
                      key={binding.agentId}
                      aria-label={`${binding.name} 挂载卡片`}
                      className={`flex shrink-0 flex-row items-center gap-1.5 rounded-lg border transition ${
                        agentRailCollapsed
                          ? "@3xl:flex-col @3xl:items-center @3xl:p-1"
                          : "px-2 py-1.5 @3xl:flex-col @3xl:items-stretch @3xl:gap-1.5 @3xl:px-2.5 @3xl:py-2"
                      } ${
                        isSelected
                          ? "border-brand-500 bg-brand-500/10 text-brand-950 ring-1 ring-brand-500/25 dark:border-brand-400 dark:bg-brand-500/15 dark:text-brand-100"
                          : binding.status === "drifted"
                            ? "border-amber-500/50 bg-amber-500/[0.06] text-zinc-900 hover:border-amber-500 dark:text-zinc-100"
                            : "border-zinc-200/90 bg-zinc-50/70 text-zinc-700 hover:border-zinc-300 hover:bg-zinc-100/80 dark:border-zinc-800 dark:bg-zinc-900/50 dark:text-zinc-300 dark:hover:border-zinc-700"
                      }`}
                    >
                      <button
                        type="button"
                        onClick={() => setSelectedAgentId(binding.agentId)}
                        aria-pressed={isSelected}
                        title={tipParts.join(" · ")}
                        className={`flex min-w-0 flex-1 items-center gap-2 text-left ${
                          agentRailCollapsed ? "@3xl:justify-center" : ""
                        }`}
                      >
                        <span className="relative inline-flex shrink-0">
                          <AgentBrandIcon agentId={binding.agentId} size="xs" />
                          <span
                            aria-hidden="true"
                            className={`absolute -bottom-0.5 -right-0.5 h-2 w-2 rounded-full ring-1 ring-white dark:ring-zinc-900 ${statusMeta.dotClass}`}
                          />
                        </span>
                        <div
                          className={`min-w-0 flex-1 ${
                            agentRailCollapsed ? "@3xl:hidden" : ""
                          }`}
                        >
                          <div className="flex items-center justify-between gap-1.5">
                            <h2 className="max-w-[132px] truncate text-xs font-semibold leading-tight @3xl:max-w-none">
                              {binding.name}
                            </h2>
                            <span className="shrink-0 rounded bg-black/[0.05] px-1 py-0.5 text-[10px] font-medium tabular-nums text-zinc-500 dark:bg-white/[0.08] dark:text-zinc-400">
                              {exposed.length}
                            </span>
                          </div>
                          <div className="mt-0.5 hidden items-center gap-1 truncate text-[11px] text-zinc-500 dark:text-zinc-400 @3xl:flex">
                            <span className="shrink-0">
                              {WIRE_PROTOCOL_META[binding.protocol].label}
                            </span>
                            <span>·</span>
                            <span className="truncate">
                              {defModel ? shortModelLabel(defModel.displayName) : statusMeta.label}
                            </span>
                          </div>
                        </div>
                        <span className="sr-only">{statusMeta.label}</span>
                      </button>

                      {binding.status === "drifted" && (
                        <button
                          type="button"
                          aria-label="查看差异并修复"
                          title="查看差异并修复"
                          onClick={() => {
                            setSelectedAgentId(binding.agentId);
                            setDiffModal({
                              binding,
                              actionKind: "repair",
                              nextPoolIds: binding.mountedPoolIds,
                              mountedPoolNames: binding.mountedPoolIds
                                .map((id) => pools.find((p) => p.id === id)?.name)
                                .filter((n): n is string => Boolean(n)),
                              beforeText: binding.beforeSnippet,
                              afterText: binding.afterSnippet,
                            });
                          }}
                          className={`inline-flex shrink-0 items-center justify-center gap-1 rounded bg-amber-500 px-1.5 py-0.5 text-[10px] font-semibold text-white hover:bg-amber-600 @3xl:btn @3xl:btn-xs @3xl:btn-primary @3xl:w-full ${
                            agentRailCollapsed ? "@3xl:hidden" : ""
                          }`}
                        >
                          <Wrench className="h-2.5 w-2.5 @3xl:h-3 @3xl:w-3" aria-hidden="true" />
                          <span className="@3xl:hidden">修复</span>
                          <span className="hidden @3xl:inline">查看差异并修复</span>
                        </button>
                      )}
                    </section>
                  );
                });
              })()}
            </div>

            {onOpenPluginSettings && (
              <div className="shrink-0 border-l border-zinc-200/80 pl-2 dark:border-zinc-800 @3xl:border-l-0 @3xl:border-t @3xl:pl-0 @3xl:pt-2">
                <button
                  type="button"
                  onClick={onOpenPluginSettings}
                  title="前往插件管理安装或启用更多 Agent 与订阅工具"
                  className={`inline-flex w-full items-center gap-1.5 rounded-lg border border-dashed border-zinc-300 bg-zinc-50/70 px-2 py-1.5 text-xs font-medium text-zinc-700 transition hover:border-brand-500/70 hover:bg-brand-50/50 hover:text-brand-900 dark:border-zinc-700 dark:bg-zinc-900/40 dark:text-zinc-200 dark:hover:border-brand-400/60 dark:hover:bg-brand-500/10 dark:hover:text-brand-100 ${
                    agentRailCollapsed ? "@3xl:justify-center @3xl:px-0" : "@3xl:justify-center"
                  }`}
                >
                  <Puzzle className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  <span className={agentRailCollapsed ? "@3xl:hidden" : ""}>
                    <span className="@3xl:hidden">插件</span>
                    <span className="hidden @3xl:inline">插件管理 · 扩展更多</span>
                  </span>
                </button>
              </div>
            )}
          </div>
        </aside>

      {/* 当前选中 Agent 的固定顶栏 + 固定订阅工具栏 + 独立滚动工作区 */}
      {activeBinding && (
        <section
          aria-label={`${activeBinding.name} 专属配置工作室`}
          className="@container/studio flex min-h-0 min-w-0 flex-1 flex-col bg-white dark:bg-zinc-900/90"
        >
          {/* A. 固定顶栏：Agent 身份、自适应单排主操作与底边下划线 3 分区 Tab */}
          <section className="shrink-0 border-b border-zinc-200/80 bg-white px-3.5 pt-2.5 dark:border-zinc-800 dark:bg-zinc-900">
            <div className="flex items-center justify-between gap-2">
              <div className="flex min-w-0 flex-1 items-center gap-2">
                <AgentBrandIcon agentId={activeBinding.agentId} size="sm" />
                <div className="min-w-0 flex-1">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <h2 className="truncate text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                      {activeBinding.name}
                    </h2>
                    <span className="hidden shrink-0 font-mono text-[11px] text-zinc-400 @xl/studio:inline">
                      v{activeBinding.version}
                    </span>
                    {renderStatusBadge(activeBinding.status, true)}
                    <span
                      className={`badge hidden shrink-0 @lg/studio:inline-flex ${WIRE_PROTOCOL_META[activeBinding.protocol].badgeClass}`}
                    >
                      {WIRE_PROTOCOL_META[activeBinding.protocol].label}
                    </span>
                  </div>
                  <div className="mt-0.5 flex items-center gap-2 truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                    <code className="truncate font-mono">{activeBinding.configPath}</code>
                    {activeBinding.lastHitAt && (
                      <span className="hidden shrink-0 @2xl/studio:inline">
                        · 最近调用：{activeBinding.lastHitAt}
                      </span>
                    )}
                  </div>
                </div>
              </div>

              <div className="flex shrink-0 items-center gap-1">
                <button
                  type="button"
                  title="预览配置 Diff"
                  aria-label="预览配置 Diff"
                  onClick={() =>
                    setDiffModal({
                      binding: activeBinding,
                      actionKind: "preview",
                      mountedPoolNames: activeBinding.mountedPoolIds
                        .map((id) => pools.find((p) => p.id === id)?.name)
                        .filter((n): n is string => Boolean(n)),
                      defaultModelDisplay: defaultExposedItem?.displayName ?? null,
                      beforeText: activeBinding.beforeSnippet,
                      afterText: activeBinding.afterSnippet,
                    })
                  }
                  className="btn btn-xs btn-secondary gap-1"
                >
                  <FileCode2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  <span className="hidden @2xl/studio:inline">预览配置 Diff</span>
                </button>

                {activeBinding.status === "drifted" ? (
                  <button
                    type="button"
                    title="查看差异并修复"
                    aria-label="查看差异并修复"
                    onClick={() =>
                      setDiffModal({
                        binding: activeBinding,
                        actionKind: "repair",
                        nextPoolIds: activeBinding.mountedPoolIds,
                        mountedPoolNames: activeBinding.mountedPoolIds
                          .map((id) => pools.find((p) => p.id === id)?.name)
                          .filter((n): n is string => Boolean(n)),
                        beforeText: activeBinding.beforeSnippet,
                        afterText: activeBinding.afterSnippet,
                      })
                    }
                    className="btn btn-xs btn-primary gap-1"
                  >
                    <Wrench className="h-3 w-3 shrink-0" aria-hidden="true" />
                    <span className="hidden @2xl/studio:inline">查看差异并修复</span>
                  </button>
                ) : activeBinding.agentId === "opencode" ? (
                  <span
                    className="hidden text-[11px] text-zinc-400 @2xl/studio:inline"
                    title="OpenCode 为终端客户端，配置在下次启动/新会话生效"
                  >
                    下次启动生效
                  </span>
                ) : !activeBinding.installed ? (
                  <span
                    className="hidden text-[11px] text-zinc-400 @2xl/studio:inline"
                    title="未检测到该客户端安装，请先在系统设置配置客户端路径"
                  >
                    未安装
                  </span>
                ) : (
                  <button
                    type="button"
                    onClick={() => void restartAgentClient(activeBinding.agentId)}
                    disabled={restartingAgentId === activeBinding.agentId}
                    aria-busy={restartingAgentId === activeBinding.agentId}
                    title={`${activeBinding.running ? "重启" : "启动"} ${activeBinding.name} 以加载最新 Qwitch 配置`}
                    aria-label={`${activeBinding.running ? "重启" : "启动"} ${activeBinding.name}`}
                    className="btn btn-xs btn-sky-soft gap-1 disabled:cursor-not-allowed disabled:opacity-60"
                  >
                    <RotateCcw
                      className={`h-3 w-3 shrink-0 ${restartingAgentId === activeBinding.agentId ? "animate-spin" : ""}`}
                      aria-hidden="true"
                    />
                    <span className="hidden @2xl/studio:inline">
                      {restartingAgentId === activeBinding.agentId
                        ? "处理中…"
                        : activeBinding.running
                          ? "重启"
                          : "启动"}
                    </span>
                  </button>
                )}

                {activeBinding.mountedPoolIds.length > 0 ? (
                  <button
                    type="button"
                    title="全部解绑并还原"
                    aria-label="全部解绑并还原"
                    onClick={() => openDiffForMountChange(activeBinding, [])}
                    className="btn btn-xs btn-danger gap-1"
                  >
                    <Unlink className="h-3 w-3 shrink-0" aria-hidden="true" />
                    <span className="hidden @2xl/studio:inline">全部解绑并还原</span>
                  </button>
                ) : null}

                <button
                  type="button"
                  onClick={() =>
                    setClearConfirm({ mode: "force", agentId: activeBinding.agentId })
                  }
                  title="强制清除：仅删除该 Agent 配置中的 Qwitch 写入（保留其它配置）"
                  aria-label="强制清除"
                  className="btn btn-xs btn-secondary gap-1"
                >
                  <Trash2 className="h-3 w-3 shrink-0" aria-hidden="true" />
                  <span className="hidden @2xl/studio:inline">强制清除</span>
                </button>
              </div>
            </div>

            {activeBinding.status === "drifted" && activeBinding.driftReason && (
              <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-amber-500/35 bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-200">
                <div className="flex items-start gap-2">
                  <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                  <span>{activeBinding.driftReason}</span>
                </div>
              </div>
            )}

            {activeBinding.status === "needs_restart" && activeBinding.restartHint && (
              <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-sky-500/30 bg-sky-500/10 px-3 py-2 text-xs text-sky-900 dark:text-sky-200">
                <span>{activeBinding.restartHint}</span>
              </div>
            )}

            {activeBinding.proxyWarning && (
              <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-amber-500/30 bg-amber-500/[0.07] px-3 py-2 text-xs text-amber-900 dark:text-amber-200">
                <span>{activeBinding.proxyWarning}</span>
                <button
                  type="button"
                  onClick={() => copyText("noproxy", noProxyCmd)}
                  className="btn btn-xs btn-secondary shrink-0 gap-1"
                >
                  {copiedId === "noproxy" ? (
                    <Check className="h-3 w-3 text-emerald-500" aria-hidden="true" />
                  ) : (
                    <Copy className="h-3 w-3" aria-hidden="true" />
                  )}
                  复制 NO_PROXY
                </button>
              </div>
            )}

            {/* 3 分区二级导航 Tab（贴底边下划线 + 右侧全部方案与插件入口） */}
            <div className="mt-1.5 flex flex-wrap items-center justify-between gap-2 border-t border-zinc-100 pt-0.5 dark:border-zinc-800/80">
              <div
                role="tablist"
                aria-label="Agent 配置分区"
                className="-mb-px flex flex-wrap items-center gap-1"
              >
                <button
                  type="button"
                  role="tab"
                  aria-selected={subTab === "models"}
                  onClick={() => setSubTab("models")}
                  className={`inline-flex items-center gap-1.5 border-b-2 px-3 py-1.5 text-xs font-medium transition ${
                    subTab === "models"
                      ? "border-brand-600 text-brand-700 dark:border-brand-400 dark:text-brand-300"
                      : "border-transparent text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-200"
                  }`}
                >
                  <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
                  <span>模型配置</span>
                  <span className="rounded bg-zinc-100 px-1.5 py-0.5 font-mono text-[11px] text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                    {activeExposedModels.length}
                  </span>
                </button>

                <button
                  type="button"
                  role="tab"
                  aria-selected={subTab === "scheduling"}
                  onClick={() => setSubTab("scheduling")}
                  className={`inline-flex items-center gap-1.5 border-b-2 px-3 py-1.5 text-xs font-medium transition ${
                    subTab === "scheduling"
                      ? "border-brand-600 text-brand-700 dark:border-brand-400 dark:text-brand-300"
                      : "border-transparent text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-200"
                  }`}
                >
                  <Crown className="h-3.5 w-3.5" aria-hidden="true" />
                  <span>账号调度</span>
                  <span className="rounded bg-zinc-100 px-1.5 py-0.5 font-mono text-[11px] text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                    {toolAccounts.filter((a) => a.enabled).length}/{toolAccounts.length}
                  </span>
                </button>

                <button
                  type="button"
                  role="tab"
                  aria-selected={subTab === "connection"}
                  onClick={() => setSubTab("connection")}
                  className={`inline-flex items-center gap-1.5 border-b-2 px-3 py-1.5 text-xs font-medium transition ${
                    subTab === "connection"
                      ? "border-brand-600 text-brand-700 dark:border-brand-400 dark:text-brand-300"
                      : "border-transparent text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-200"
                  }`}
                >
                  <SlidersHorizontal className="h-3.5 w-3.5" aria-hidden="true" />
                  <span>接入设置</span>
                </button>
              </div>

              {(subTab === "models" || subTab === "scheduling") && (() => {
                const globalItems =
                  subTab === "models"
                    ? [
                        ...(configuredToolIds.length > 1
                          ? [
                              {
                                label: `同步全部 ${configuredToolIds.length} 个订阅到其余 Agent`,
                                icon: CopyCheck,
                                onSelect: () =>
                                  syncAllToolModelPlansToAllAgents(activeBinding.agentId),
                              },
                            ]
                          : []),
                        ...modelPlanGlobalSources.map((source) => ({
                          label: `从 ${source.name} 复制全部 ${source.toolCount} 个订阅`,
                          icon: Copy,
                          onSelect: () =>
                            copyAgentAllToolModelPlans(source.agentId, activeBinding.agentId),
                        })),
                      ]
                    : [];
                if (globalItems.length === 0 && !onOpenPluginSettings) return null;
                return (
                  <div className="flex shrink-0 items-center gap-1.5 pb-1">
                    {globalItems.length > 0 && (
                      <OverflowMenu
                        display="text"
                        label="全部方案"
                        items={globalItems}
                      />
                    )}
                    {onOpenPluginSettings && (
                      <button
                        type="button"
                        onClick={onOpenPluginSettings}
                        title="前往插件管理安装或配置更多订阅工具"
                        className="inline-flex items-center gap-1 rounded-lg border border-dashed border-zinc-300 bg-zinc-50/70 px-2 py-1 text-xs font-medium text-zinc-600 transition hover:border-brand-500/70 hover:bg-brand-50/50 hover:text-brand-900 dark:border-zinc-700 dark:bg-zinc-900/50 dark:text-zinc-300 dark:hover:border-brand-400/60 dark:hover:bg-brand-500/10 dark:hover:text-brand-100"
                      >
                        <Puzzle className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        <span>插件</span>
                      </button>
                    )}
                  </div>
                );
              })()}
            </div>
          </section>

          {/* B. 固定订阅工具切换栏（紧凑自动换行，零横向滚动、不随下方列表滚动） */}
          {(subTab === "models" || subTab === "scheduling") && renderToolSwitcher()}

          {/* C. 右侧独立滚动内容区（不论左侧栏收起或展开都能正常上下滚动） */}
          <div className="min-h-0 flex-1 overflow-y-auto px-3.5 py-3">
          {/* 分区 1：模型配置（全宽订阅模型列表 + 底部实时预览） */}
          {subTab === "models" && (
            <div className="space-y-2.5">
              {standardApiUnconfigured && (
                <div className="rounded-lg border border-amber-500/30 bg-amber-500/[0.08] p-3 text-xs text-amber-800 dark:border-amber-400/25 dark:bg-amber-500/10 dark:text-amber-200">
                  标准 API 是「通用 API Key」兜底工具，当前尚未配置该账号：请先到
                  「账号资产 → 添加账号 → 通用 API Key」填写 Base URL 与 Key。未配置
                  账号时，此处的模型勾选与调度编排不会产生任何路由效果。
                </div>
              )}
              <section
                aria-label={`${activeToolName} 订阅模型选择`}
                className="flex flex-col justify-between gap-2.5"
              >
                <div className="space-y-2.5">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <h3 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                        {activeToolName} 订阅模型
                      </h3>
                      <span
                        className={`badge ${
                          modelSourceBadge.tone === "ok"
                            ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                            : modelSourceBadge.tone === "info"
                              ? "border-sky-500/30 bg-sky-500/10 text-sky-700 dark:text-sky-300"
                              : "border-amber-500/30 bg-amber-500/10 text-amber-800 dark:text-amber-300"
                        }`}
                        title={
                          modelSourceBadge.tone === "warn"
                            ? "该工具没有可用的探测接口，当前列表来自静态兜底表，可能与真实可用模型不一致；可点「刷新真实模型」或「套用推荐目录」"
                            : undefined
                        }
                      >
                        {modelSourceBadge.label}
                      </span>
                      <span
                        className="text-[11px] text-zinc-500 dark:text-zinc-400"
                        title={`将以 ${activeToolId}/模型名 暴露给 ${activeBinding.name}，勾选即实时写入配置`}
                      >
                        已勾选{" "}
                        <strong className="text-zinc-900 dark:text-zinc-100">
                          {enabledBareModelIds.length}
                        </strong>{" "}
                        / {toolModelOptions.length} 个模型
                      </span>
                    </div>

                    <div className="flex shrink-0 items-center gap-1.5">
                      {modelPlanCopySources.length > 0 && (
                        <OverflowMenu
                          display="text"
                          label="复制本订阅"
                          items={modelPlanCopySources.map((source) => ({
                            label: `${source.name}（${source.modelCount} 个模型）`,
                            icon: Copy,
                            onSelect: () =>
                              copyAgentToolModelPlan(
                                source.agentId,
                                activeBinding.agentId,
                                activeToolId
                              ),
                          }))}
                        />
                      )}
                      <button
                        type="button"
                        disabled={scopeNotSelectable || enabledBareModelIds.length === 0}
                        onClick={() =>
                          syncAgentToolModelPlanToAllAgents(activeBinding.agentId, activeToolId)
                        }
                        className="btn btn-xs btn-secondary gap-1 disabled:opacity-45"
                        title="把当前订阅的模型与默认模型写入其余所有 Agent；跨订阅请用上方「全部订阅方案」"
                      >
                        <CopyCheck className="h-3 w-3 text-brand-500" aria-hidden="true" />
                        同步本订阅
                      </button>
                      {presetAvailable &&
                        modelSourceBadge.tone === "warn" &&
                        toolAccountsForScope[0] && (
                          <button
                            type="button"
                            onClick={() =>
                              applyAccountPresetModels(toolAccountsForScope[0].accountId)
                            }
                            className="btn btn-xs btn-primary gap-1"
                            title="用内置推荐目录替换当前静态兜底列表（来源标记为「内置预设」）"
                          >
                            <Sparkles className="h-3 w-3" aria-hidden="true" />
                            套用推荐目录
                          </button>
                        )}
                      <button
                        type="button"
                        disabled={isRefreshingModels}
                        onClick={() => void refreshUpstreamModels()}
                        className="btn btn-xs btn-secondary gap-1"
                      >
                        <RefreshCw
                          className={`h-3 w-3 ${isRefreshingModels ? "animate-spin" : ""}`}
                          aria-hidden="true"
                        />
                        刷新真实模型
                      </button>
                    </div>
                  </div>

                  {gatewayUnsupported && (
                    <div className="flex flex-wrap items-start gap-2 rounded-xl border border-amber-500/25 bg-amber-500/[0.07] px-3 py-2 text-[11px] leading-relaxed text-amber-800 dark:text-amber-300">
                      <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      <span className="min-w-0 flex-1">
                        <strong className="font-medium">{activeToolName} 暂不支持网关调度。</strong>
                        {gatewayUnsupportedReason(activeToolId)}
                        这里展示的模型目录仅用于和客户端对照。
                      </span>
                    </div>
                  )}

                  {!gatewayUnsupported && scopeReadonly && activeToolId === "codearts" && (
                    <div className="flex flex-wrap items-start gap-2 rounded-xl border border-sky-500/25 bg-sky-500/[0.07] px-3 py-2 text-[11px] leading-relaxed text-sky-800 dark:text-sky-300">
                      <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      <span className="min-w-0 flex-1">
                        当前使用本机导入的码道凭据：Qwitch 会用它签名直连，但<strong className="font-medium">不会主动刷新</strong>（刷新会与 IDE 互踢）。
                        STS 到期后请求会失败，届时重新导入一次即可。
                      </span>
                    </div>
                  )}

                  {/* 搜索与快速筛选栏 */}
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="relative min-w-[160px] flex-1">
                      <Search
                        className="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400"
                        aria-hidden="true"
                      />
                      <input
                        type="text"
                        aria-label="搜索订阅模型"
                        value={modelSearch}
                        onChange={(e) => setModelSearch(e.target.value)}
                        placeholder={`搜索 ${activeToolName} 模型名、家族或模型 ID...`}
                        className="field-input w-full bg-zinc-50 py-1 pr-8 pl-8 focus:bg-white dark:bg-zinc-950/60"
                      />
                      {modelSearch && (
                        <button
                          type="button"
                          aria-label="清除搜索"
                          onClick={() => setModelSearch("")}
                          className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-0.5 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200"
                        >
                          <X className="h-3.5 w-3.5" aria-hidden="true" />
                        </button>
                      )}
                    </div>

                    <div role="group" aria-label="模型快速筛选" className="flex flex-wrap items-center gap-1">
                      {(
                        [
                          { id: "all", label: `全部 (${filterCounts.all})` },
                          { id: "checked", label: `已勾选 (${filterCounts.checked})` },
                          { id: "low_rate", label: `免费/低倍率 (${filterCounts.lowRate})` },
                          { id: "flagship", label: `主力旗舰 (${filterCounts.flagship})` },
                        ] as const
                      ).map((tab) => (
                        <button
                          key={tab.id}
                          type="button"
                          aria-pressed={modelQuickFilter === tab.id}
                          onClick={() => setModelQuickFilter(tab.id)}
                          className={`rounded-lg px-2 py-1 text-[11px] font-medium transition ${
                            modelQuickFilter === tab.id
                              ? "bg-brand-700 text-white dark:bg-brand-400 dark:text-zinc-950"
                              : "bg-zinc-100 text-zinc-600 hover:bg-zinc-200/70 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700"
                          }`}
                        >
                          {tab.label}
                        </button>
                      ))}
                    </div>
                  </div>

                  {/* 一键批量勾选工具栏 */}
                  <div
                    role="toolbar"
                    aria-label="模型批量选择工具栏"
                    className="flex flex-wrap items-center gap-1.5 border-b border-zinc-100 pb-2 dark:border-zinc-800/80"
                  >
                    <button
                      type="button"
                      disabled={scopeNotSelectable}
                      onClick={() =>
                        selectAllAgentToolModels(
                          activeBinding.agentId,
                          activeToolId,
                          modelSearch || modelQuickFilter !== "all"
                            ? filteredToolModels.map((m) => m.id)
                            : undefined
                        )
                      }
                      className="btn btn-xs btn-secondary gap-1"
                    >
                      <CheckSquare className="h-3 w-3 text-brand-600 dark:text-brand-400" aria-hidden="true" />
                      全选当前
                    </button>
                    <button
                      type="button"
                      disabled={scopeNotSelectable}
                      onClick={() =>
                        selectLowRateAgentToolModels(activeBinding.agentId, activeToolId)
                      }
                      className="btn btn-xs btn-secondary gap-1"
                    >
                      <Flame className="h-3 w-3 text-emerald-500" aria-hidden="true" />
                      仅选免费/低倍率
                    </button>
                    <button
                      type="button"
                      disabled={scopeNotSelectable}
                      onClick={() =>
                        invertAgentToolModels(
                          activeBinding.agentId,
                          activeToolId,
                          modelSearch || modelQuickFilter !== "all"
                            ? filteredToolModels.map((m) => m.id)
                            : undefined
                        )
                      }
                      className="btn btn-xs btn-secondary"
                    >
                      反选
                    </button>
                    <button
                      type="button"
                      disabled={scopeNotSelectable || enabledBareModelIds.length === 0}
                      onClick={() =>
                        clearAgentToolModels(activeBinding.agentId, activeToolId)
                      }
                      className="btn btn-xs btn-secondary text-rose-600 hover:bg-rose-50 disabled:opacity-45 dark:text-rose-400 dark:hover:bg-rose-950/40"
                    >
                      清空本工具
                    </button>
                  </div>

                  {/* 双列宽松模型复选列表 */}
                  <div className="grid grid-cols-1 gap-1.5 @2xl:grid-cols-2">
                    {filteredToolModels.length === 0 ? (
                      toolModelOptions.length === 0 ? (
                        <div className="col-span-full flex flex-col items-center gap-2 rounded-xl border border-dashed border-zinc-200 py-8 text-center dark:border-zinc-800">
                          <span className="text-xs text-zinc-500 dark:text-zinc-400">
                            {activeToolName} 暂无模型目录：该工具没有可用的自动探测接口
                          </span>
                          <span className="text-[11px] text-zinc-400">
                            可先点「刷新真实模型」读取上游目录；仍为空时用「套用推荐目录」按真实账号核对后的内置清单
                          </span>
                          <div className="flex flex-wrap items-center justify-center gap-1.5">
                            {presetAvailable && toolAccountsForScope[0] && (
                              <button
                                type="button"
                                onClick={() =>
                                  applyAccountPresetModels(toolAccountsForScope[0].accountId)
                                }
                                className="btn btn-xs btn-primary gap-1"
                              >
                                <Sparkles className="h-3 w-3" aria-hidden="true" />
                                套用推荐目录
                              </button>
                            )}
                          </div>
                        </div>
                      ) : (
                        <div className="col-span-full rounded-xl border border-dashed border-zinc-200 py-8 text-center text-xs text-zinc-400 dark:border-zinc-800">
                          当前筛选条件下无匹配模型
                        </div>
                      )
                    ) : (
                      filteredToolModels.map((opt) => {
                        const checked = enabledModelSet.has(opt.id);
                        const legacyFirstTool = activeAgentPool?.members.find(
                          (m) => m.enabled && m.supportedModels.includes(opt.id)
                        )?.toolId;
                        const isCurrentDefault =
                          Boolean(defaultExposedItem) &&
                          (defaultExposedItem!.modelId === formatScopedModelId(activeToolId, opt.id) ||
                            (splitScopedModelId(defaultExposedItem!.modelId).toolId === null &&
                              defaultExposedItem!.modelId === opt.id &&
                              legacyFirstTool === activeToolId));

                        return (
                          <div
                            key={opt.id}
                            className={`flex items-center justify-between gap-2 rounded-xl border px-2.5 py-2 text-xs transition ${
                              isCurrentDefault
                                ? "border-amber-500/50 bg-amber-500/[0.07] dark:border-amber-400/40"
                                : checked
                                  ? "border-brand-500/45 bg-brand-500/[0.04] dark:border-brand-400/40"
                                  : "border-zinc-200/80 bg-white hover:border-zinc-300 dark:border-zinc-800 dark:bg-zinc-900/60"
                            }`}
                          >
                            <button
                              type="button"
                              role="checkbox"
                              aria-checked={checked}
                              aria-label={`选择模型 ${opt.displayName}`}
                              disabled={scopeNotSelectable}
                              title={
                                scopeNotSelectable
                                  ? gatewayUnsupported
                                    ? "该工具未接入网关调度，模型勾选不会生效"
                                    : "该工具账号为本机只读凭据，不参与网关调度"
                                  : undefined
                              }
                              onClick={() => {
                                const next = checked
                                  ? enabledBareModelIds.filter((id) => id !== opt.id)
                                  : [...enabledBareModelIds, opt.id];
                                setAgentToolModels(activeBinding.agentId, activeToolId, next);
                              }}
                              className="flex min-w-0 flex-1 items-center gap-2 text-left"
                            >
                              <span
                                className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border transition ${
                                  checked
                                    ? "border-brand-600 bg-brand-600 text-white dark:border-brand-400 dark:bg-brand-400 dark:text-zinc-950"
                                    : "border-zinc-300 bg-white dark:border-zinc-600 dark:bg-zinc-900"
                                }`}
                              >
                                {checked && <Check className="h-3 w-3" aria-hidden="true" />}
                              </span>

                              <div className="min-w-0 flex-1">
                                <div className="flex flex-wrap items-center gap-1.5">
                                  <span className="truncate font-mono font-semibold text-zinc-900 dark:text-zinc-100">
                                    {opt.displayName}
                                  </span>
                                  <span className="rounded bg-zinc-100 px-1.5 py-0.5 text-[11px] text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400">
                                    {opt.family}
                                  </span>
                                  {(() => {
                                    const spec = formatModelSpec(opt);
                                    return spec ? (
                                      <span
                                        className="rounded bg-zinc-100 px-1.5 py-0.5 text-[11px] tabular-nums text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400"
                                        title={spec}
                                      >
                                        {spec}
                                      </span>
                                    ) : null;
                                  })()}
                                  {opt.source === "manual" ? (
                                    <span
                                      className="rounded bg-sky-500/12 px-1.5 py-0.5 text-[11px] font-medium text-sky-700 dark:text-sky-300"
                                      title="由你手工填写"
                                    >
                                      手工
                                    </span>
                                  ) : opt.source === "preset" ? (
                                    <span
                                      className="rounded bg-indigo-500/12 px-1.5 py-0.5 text-[11px] font-medium text-indigo-700 dark:text-indigo-300"
                                      title="内置推荐目录，未与上游核对；如与实际不符请修改"
                                    >
                                      预设
                                    </span>
                                  ) : opt.source === "client" ? (
                                    <span
                                      className="rounded bg-teal-500/12 px-1.5 py-0.5 text-[11px] font-medium text-teal-700 dark:text-teal-300"
                                      title="从本机客户端读取的真实模型清单"
                                    >
                                      本机
                                    </span>
                                  ) : opt.source !== "live" ? (
                                    <span
                                      className="rounded bg-amber-500/12 px-1.5 py-0.5 text-[11px] font-medium text-amber-800 dark:text-amber-300"
                                      title="该条目来自静态兜底表，未经过上游探测"
                                    >
                                      未探测
                                    </span>
                                  ) : null}
                                  {opt.isFree ? (
                                    <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-[11px] font-medium text-emerald-700 dark:text-emerald-300">
                                      免费
                                    </span>
                                  ) : typeof opt.creditMultiplier === "number" ? (
                                    <span
                                      className={`rounded px-1.5 py-0.5 font-mono text-[11px] tabular-nums ${
                                        opt.creditMultiplier <= 0.2
                                          ? "bg-sky-500/12 text-sky-700 dark:text-sky-300"
                                          : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400"
                                      }`}
                                    >
                                      {opt.creditMultiplier}×
                                    </span>
                                  ) : null}
                                </div>
                              </div>
                            </button>

                            {isCurrentDefault ? (
                              <span className="inline-flex shrink-0 items-center gap-1 rounded-lg bg-amber-500/15 px-2 py-1 text-[11px] font-semibold text-amber-800 dark:text-amber-300">
                                <Star className="h-3 w-3 fill-current" aria-hidden="true" />
                                默认模型
                              </span>
                            ) : (
                              <button
                                type="button"
                                disabled={scopeNotSelectable}
                                onClick={() =>
                                  setAgentDefaultToolModel(
                                    activeBinding.agentId,
                                    activeToolId,
                                    opt.id
                                  )
                                }
                                className="shrink-0 rounded-lg px-2 py-1 text-[11px] font-medium text-brand-700 transition hover:bg-brand-500/10 disabled:opacity-40 disabled:hover:bg-transparent dark:text-brand-300"
                              >
                                设为默认
                              </button>
                            )}
                          </div>
                        );
                      })
                    )}
                  </div>
                </div>
              </section>
            </div>
          )}

          {/* 分区 2：账号调度（紧凑策略分段 + 主备顺位队列） */}
          {subTab === "scheduling" && (
            <div className="space-y-3">
              {standardApiUnconfigured && (
                <div className="rounded-lg border border-amber-500/30 bg-amber-500/[0.08] p-3 text-xs text-amber-800 dark:border-amber-400/25 dark:bg-amber-500/10 dark:text-amber-200">
                  标准 API 尚未配置「通用 API Key」账号：请先到「账号资产 → 添加账号 →
                  通用 API Key」完成配置；此处的调度编排与主备顺位在配置账号前不会生效。
                </div>
              )}
              <section
                aria-label={`${activeToolName} 同工具账号调度方案`}
                className="flex flex-col gap-3.5"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <div className="flex items-center gap-1.5">
                      <Crown className="h-4 w-4 text-amber-500" aria-hidden="true" />
                      <h3 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                        {activeToolName} 主备账号调度
                      </h3>
                      <span className="badge border-sky-500/25 bg-sky-500/10 text-sky-700 dark:text-sky-300">
                        {toolAccounts.filter((a) => a.enabled).length}/{toolAccounts.length} 启用
                      </span>
                    </div>
                    <p className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                      同工具额度与模型口径一致 · 仅作用于 {activeBinding.name}
                    </p>
                  </div>

                  <button
                    type="button"
                    disabled={toolAccounts.length === 0}
                    onClick={() =>
                      syncToolScheduleToAllAgents(activeBinding.agentId, activeToolId)
                    }
                    className="btn btn-xs btn-secondary gap-1 disabled:opacity-45"
                  >
                    <CopyCheck className="h-3 w-3 text-brand-500" aria-hidden="true" />
                    同步到全部 Agent
                  </button>
                </div>

                {/* 紧凑横向分段策略选择器 + 1 行当前策略说明 */}
                <div className="space-y-1.5">
                  <div className="grid grid-cols-1 gap-1.5 @lg:grid-cols-3">
                    {AGENT_STRATEGY_OPTIONS.map((mode) => {
                      const active = currentStrategy === mode.id;
                      return (
                        <button
                          key={mode.id}
                          type="button"
                          aria-pressed={active}
                          onClick={() =>
                            updateAgentToolRoutingOptions(activeBinding.agentId, activeToolId, {
                              strategy: mode.id,
                            })
                          }
                          className={`rounded-xl border px-3 py-2 text-xs font-medium transition ${
                            active
                              ? "border-brand-500 bg-brand-500/10 font-semibold text-brand-900 dark:border-brand-400 dark:text-brand-100"
                              : "border-zinc-200/80 bg-zinc-50/70 text-zinc-700 hover:bg-white dark:border-zinc-800 dark:bg-zinc-950/40 dark:text-zinc-300"
                          }`}
                        >
                          {mode.shortLabel}
                        </button>
                      );
                    })}
                  </div>
                  <p className="px-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                    {activeStrategyDesc}
                  </p>
                </div>

                {/* 账号批量操作与防互踢护栏 */}
                <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-zinc-200/80 bg-zinc-50/75 px-3 py-2 dark:border-zinc-800 dark:bg-zinc-950/45">
                  <div className="flex flex-wrap items-center gap-1.5">
                    <button
                      type="button"
                      disabled={toolAccounts.length === 0}
                      onClick={() =>
                        setAllAgentToolAccountsEnabled(activeBinding.agentId, activeToolId, false)
                      }
                      className="btn btn-xs btn-secondary gap-1 disabled:opacity-45"
                    >
                      <Check className="h-3 w-3 text-emerald-500" aria-hidden="true" />
                      全部启用
                    </button>
                    <button
                      type="button"
                      disabled={toolAccounts.length === 0}
                      onClick={() =>
                        setAllAgentToolAccountsEnabled(activeBinding.agentId, activeToolId, true)
                      }
                      className="btn btn-xs btn-secondary gap-1 disabled:opacity-45"
                    >
                      <Square className="h-3 w-3 text-zinc-500" aria-hidden="true" />
                      仅启用主号
                    </button>
                    <button
                      type="button"
                      disabled={toolAccounts.length <= 1}
                      onClick={() =>
                        sortAgentToolAccountsByQuota(activeBinding.agentId, activeToolId)
                      }
                      className="btn btn-xs btn-secondary gap-1 disabled:opacity-45"
                    >
                      <ArrowDownWideNarrow className="h-3 w-3 text-sky-500" aria-hidden="true" />
                      按额度排顺位
                    </button>
                  </div>

                  <div className="flex flex-wrap items-center gap-3 text-[11px] text-zinc-600 dark:text-zinc-400">
                    <label className="flex cursor-pointer items-center gap-1.5">
                      <input
                        type="checkbox"
                        checked={currentSkipActive}
                        onChange={(e) =>
                          updateAgentToolRoutingOptions(activeBinding.agentId, activeToolId, {
                            skipActiveClientAccount: e.target.checked,
                          })
                        }
                        className="rounded border-zinc-300 text-brand-600 focus:ring-brand-500"
                      />
                      <span>跳过桌面端活跃账号（防互踢）</span>
                    </label>

                    <label className="flex items-center gap-1">
                      <span>429 冷却</span>
                      <select
                        aria-label="429 冷却时长"
                        value={currentCooldownSec}
                        onChange={(e) =>
                          updateAgentToolRoutingOptions(activeBinding.agentId, activeToolId, {
                            cooldownSeconds: Number(e.target.value),
                          })
                        }
                        className="field-input h-7 py-0 pl-2 font-mono text-[11px]"
                      >
                        <option value={30}>30s</option>
                        <option value={60}>60s</option>
                        <option value={90}>90s</option>
                        <option value={120}>120s</option>
                      </select>
                    </label>
                  </div>
                </div>

                {/* 有序同工具主备账号队列 */}
                <div className="grid grid-cols-1 gap-2 @2xl:grid-cols-2">
                  {toolAccounts.length === 0 ? (
                    <div className="col-span-full rounded-xl border border-dashed border-zinc-200 py-8 text-center text-xs text-zinc-400 dark:border-zinc-800">
                      当前尚未导入 {activeToolName} 账号，可在左侧「账号资产」先导入账号
                    </div>
                  ) : (
                    toolAccounts.map((acc, idx) => {
                      const isPrimary = idx === 0;
                      const cooldownRemain =
                        acc.cooldownUntilMs && acc.cooldownUntilMs > nowMs
                          ? Math.ceil((acc.cooldownUntilMs - nowMs) / 1000)
                          : 0;
                      const isReadonlyCodeArts = acc.codeArtsMode === "client_readonly";

                      return (
                        <div
                          key={acc.accountId}
                          className={`rounded-xl border p-3 transition ${
                            isPrimary
                              ? "border-brand-500/50 bg-brand-500/[0.05] dark:border-brand-400/45"
                              : acc.enabled
                                ? "border-zinc-200/80 bg-white dark:border-zinc-800 dark:bg-zinc-900/60"
                                : "border-zinc-200/60 bg-zinc-50/70 opacity-70 dark:border-zinc-800/70 dark:bg-zinc-950/40"
                          }`}
                        >
                          <div className="flex flex-wrap items-start justify-between gap-2">
                            <div className="min-w-0 flex-1">
                              <div className="flex flex-wrap items-center gap-1.5">
                                <span
                                  className={`rounded-md px-1.5 py-0.5 text-[11px] font-semibold ${
                                    isPrimary
                                      ? "bg-brand-700 text-white dark:bg-brand-400 dark:text-zinc-950"
                                      : "bg-zinc-200/80 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                                  }`}
                                >
                                  {isPrimary ? "#1 主账号" : `#${idx + 1} 备用账号`}
                                </span>
                                <span className="truncate text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                                  {acc.label}
                                </span>
                                <code className="text-[11px] text-zinc-400">{acc.uidMasked}</code>
                              </div>

                              <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-[11px]">
                                {acc.isActiveOnClient && (
                                  <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-amber-800 dark:text-amber-300">
                                    桌面端挂载中
                                  </span>
                                )}
                                {cooldownRemain > 0 && (
                                  <span className="rounded bg-rose-500/15 px-1.5 py-0.5 font-medium text-rose-700 dark:text-rose-300">
                                    冷却中 {cooldownRemain}s
                                  </span>
                                )}
                                {isReadonlyCodeArts && (
                                  <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-amber-800 dark:text-amber-300">
                                    仅读客户端凭据（需隔离授权）
                                  </span>
                                )}
                              </div>
                            </div>

                            <div className="flex shrink-0 items-center gap-1">
                              {!isPrimary && (
                                <button
                                  type="button"
                                  onClick={() =>
                                    promoteAgentToolPrimaryAccount(
                                      activeBinding.agentId,
                                      activeToolId,
                                      acc.accountId
                                    )
                                  }
                                  className="btn btn-xs btn-primary"
                                >
                                  设为主号
                                </button>
                              )}

                              {toolAccounts.length > 1 && (
                                <>
                                  <button
                                    type="button"
                                    aria-label={`上移 ${acc.label}`}
                                    disabled={idx === 0}
                                    onClick={() =>
                                      reorderAgentToolAccount(
                                        activeBinding.agentId,
                                        activeToolId,
                                        acc.accountId,
                                        "up"
                                      )
                                    }
                                    className="rounded-lg border border-zinc-200 p-1 text-zinc-500 hover:bg-zinc-100 disabled:opacity-35 dark:border-zinc-700 dark:hover:bg-zinc-800"
                                  >
                                    <ArrowUp className="h-3 w-3" aria-hidden="true" />
                                  </button>
                                  <button
                                    type="button"
                                    aria-label={`下移 ${acc.label}`}
                                    disabled={idx === toolAccounts.length - 1}
                                    onClick={() =>
                                      reorderAgentToolAccount(
                                        activeBinding.agentId,
                                        activeToolId,
                                        acc.accountId,
                                        "down"
                                      )
                                    }
                                    className="rounded-lg border border-zinc-200 p-1 text-zinc-500 hover:bg-zinc-100 disabled:opacity-35 dark:border-zinc-700 dark:hover:bg-zinc-800"
                                  >
                                    <ArrowDown className="h-3 w-3" aria-hidden="true" />
                                  </button>
                                </>
                              )}
                            </div>
                          </div>

                          <div className="mt-2.5 flex items-center justify-between gap-2 text-[11px]">
                            <div className="flex min-w-0 flex-1 items-center gap-2">
                              <div className="h-1.5 w-20 shrink-0 overflow-hidden rounded-full bg-zinc-200 dark:bg-zinc-800">
                                <div
                                  className={`h-full rounded-full ${
                                    acc.quotaPercent >= 50
                                      ? "bg-emerald-500"
                                      : acc.quotaPercent >= 20
                                        ? "bg-amber-500"
                                        : "bg-rose-500"
                                  }`}
                                  style={{ width: `${Math.min(100, Math.max(0, acc.quotaPercent))}%` }}
                                />
                              </div>
                              <span className="truncate font-mono text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                                {acc.quotaPercent}% · {acc.quotaText}
                              </span>
                            </div>

                            <div className="flex shrink-0 items-center gap-1.5">
                              {isReadonlyCodeArts && (
                                <button
                                  type="button"
                                  onClick={() => authorizeCodeArtsIsolated(acc.accountId)}
                                  className="btn btn-xs btn-secondary"
                                >
                                  签发独立 OAuth
                                </button>
                              )}
                              {activeAgentPool && !acc.id.startsWith("cand-") && (
                                    cooldownRemain > 0 ? (
                                      <button
                                        type="button"
                                        onClick={() =>
                                          clearMemberCooldown(activeAgentPool.id, acc.id)
                                        }
                                        className="rounded-lg border border-emerald-500/40 px-1.5 py-0.5 text-[11px] font-medium text-emerald-700 hover:bg-emerald-500/10 dark:text-emerald-300"
                                      >
                                        解除冷却
                                      </button>
                                    ) : (
                                      <button
                                        type="button"
                                        onClick={() =>
                                          injectMemberCooldown(activeAgentPool.id, acc.id)
                                        }
                                        className="rounded-lg border border-zinc-200 px-1.5 py-0.5 text-[11px] text-zinc-500 hover:bg-zinc-100 dark:border-zinc-700 dark:hover:bg-zinc-800"
                                      >
                                        临时冷却
                                      </button>
                                    )
                                  )}

                                  <button
                                    type="button"
                                    aria-pressed={acc.enabled}
                                    onClick={() =>
                                      toggleAgentToolAccountEnabled(
                                        activeBinding.agentId,
                                        activeToolId,
                                        acc.accountId
                                      )
                                    }
                                    className={`rounded-lg px-2 py-0.5 text-[11px] font-medium transition ${
                                      acc.enabled
                                        ? "bg-emerald-500/15 text-emerald-800 hover:bg-emerald-500/25 dark:text-emerald-300"
                                        : "bg-zinc-200 text-zinc-600 hover:bg-zinc-300 dark:bg-zinc-800 dark:text-zinc-400"
                                    }`}
                                  >
                                    {acc.enabled ? "参与调度" : "已停用"}
                                  </button>
                            </div>
                          </div>
                        </div>
                      );
                    })
                  )}
                </div>
              </section>
            </div>
          )}

          {/* 分区 3：接入设置与通用端点 */}
          {subTab === "connection" && (
            <div className="space-y-3.5">
              <section className="surface-card space-y-3 p-4">
                <div>
                  <h3 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                    {activeBinding.name} 兼容选项与配置文件
                  </h3>
                  <p className="mt-0.5 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                    {activeBinding.configPath}
                  </p>
                </div>

                <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl bg-zinc-50 px-3 py-2.5 text-xs text-zinc-600 dark:bg-zinc-950/55 dark:text-zinc-400">
                  {activeBinding.agentId === "claude_desktop" && (
                    <label className="flex w-full cursor-pointer items-center justify-between gap-2">
                      <span>
                        macOS 独立双开隔离副本（保留官方 Claude 登录态 · 非 Anthropic 模型自动映射{" "}
                        <code>mythos-qwitch-*</code>）
                      </span>
                      <input
                        type="checkbox"
                        checked={Boolean(activeBinding.options.claudeIsolatedTwin)}
                        onChange={() =>
                          toggleAgentOption("claude_desktop", "claudeIsolatedTwin")
                        }
                        className="rounded border-zinc-300 text-brand-600 focus:ring-brand-500"
                      />
                    </label>
                  )}

                  {activeBinding.agentId === "codex_desktop" && (
                    <div className="flex w-full flex-wrap items-center justify-between gap-2">
                      <span className="flex items-center gap-1.5">
                        <ShieldCheck className="h-3.5 w-3.5 text-emerald-500" aria-hidden="true" />
                        已写入 <code>qwitch_models.json</code> · 保留官方 ChatGPT 登录模型直连，解绑不破坏历史线程
                      </span>
                    </div>
                  )}

                  {activeBinding.agentId === "dsh" && (
                    <span>
                      双凭据校验：<code>.env</code> 已写入 · <code>.credentials.yaml</code>{" "}
                      {activeBinding.options.dshDualCredsWritten ? (
                        <strong className="text-emerald-600 dark:text-emerald-400">已同步</strong>
                      ) : (
                        <strong className="text-amber-600 dark:text-amber-400">
                          缺失（待修复）
                        </strong>
                      )}
                    </span>
                  )}

                  {activeBinding.agentId === "opencode" && (
                    <div className="flex w-full items-center justify-between gap-2">
                      <span>
                        OpenCode 配置文件格式（切换即时重写 <code>opencode.jsonc</code>）
                      </span>
                      <button
                        type="button"
                        onClick={() => toggleAgentOption("opencode", "opencodeSchemaVersion")}
                        className="chip border border-zinc-200 bg-white font-mono dark:border-zinc-700 dark:bg-zinc-800"
                      >
                        {activeBinding.options.opencodeSchemaVersion === "v1"
                          ? "v1 (provider)"
                          : "v2 (providers)"}
                      </button>
                    </div>
                  )}

                  {activeBinding.agentId === "mimo" && (
                    <div className="flex w-full flex-wrap items-center justify-between gap-2">
                      <span className="flex items-center gap-1.5">
                        <ShieldCheck className="h-3.5 w-3.5 text-emerald-500" aria-hidden="true" />
                        已写入 <code>mimocode.jsonc</code>（仅托管 <code>provider.qwitch</code> 节点），桌面端重启后生效
                      </span>
                    </div>
                  )}

                  {activeBinding.agentId === "zcode" && (
                    <div className="flex w-full flex-wrap items-center justify-between gap-2">
                      <span className="flex items-center gap-1.5">
                        <ShieldCheck className="h-3.5 w-3.5 text-emerald-500" aria-hidden="true" />
                        已写入 <code>config.json</code>（仅托管 <code>provider.qwitch</code> 节点），桌面端重启后生效
                      </span>
                    </div>
                  )}
                </div>
              </section>

              {/* 通用标准端点生成卡 */}
              <section className="surface-card p-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="flex items-center gap-2.5">
                    <AgentBrandIcon agentId="custom_endpoint" size="sm" />
                    <div>
                      <h3 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                        通用标准网关端点（适用于任意兼容 OpenAI / Anthropic 的客户端）
                      </h3>
                      <p className="text-[11px] text-zinc-500 dark:text-zinc-400">
                        选择已接入的模型后直接生成环境变量，请求将自动按当前 Agent 的同工具主备顺位路由
                      </p>
                    </div>
                  </div>

                  <div className="flex min-w-0 flex-wrap items-center gap-2">
                    <select
                      value={effectiveCustomWire}
                      onChange={(e) => setCustomModelWire(e.target.value)}
                      aria-label="选择导出环境变量的目标模型"
                      className="field-input max-w-full min-w-0 font-mono"
                    >
                      {customExportModels.map((m) => (
                        <option key={m.wireId} value={m.wireId}>
                          {m.label} ({m.wireId})
                        </option>
                      ))}
                    </select>
                    <button
                      type="button"
                      onClick={() => copyText("customEnv", customEndpointEnv)}
                      className="btn btn-sm btn-secondary gap-1.5"
                    >
                      {copiedId === "customEnv" ? (
                        <Check className="h-3.5 w-3.5 text-emerald-500" aria-hidden="true" />
                      ) : (
                        <Terminal className="h-3.5 w-3.5" aria-hidden="true" />
                      )}
                      {copiedId === "customEnv" ? "已复制环境变量" : "复制环境变量"}
                    </button>
                  </div>
                </div>

                <pre className="mt-3 overflow-x-auto rounded-xl border border-zinc-200/80 bg-zinc-50 p-3 font-mono text-[11px] leading-relaxed text-zinc-700 dark:border-zinc-800 dark:bg-zinc-950 dark:text-zinc-300">
                  {customEndpointEnv}
                </pre>
              </section>
            </div>
          )}
          </div>
        </section>
      )}
      </div>

      {diffModal && (
        <ConfigDiffModal
          open={Boolean(diffModal)}
          binding={diffModal.binding}
          actionKind={diffModal.actionKind}
          targetSummary={{
            mountedPoolNames: diffModal.mountedPoolNames,
            defaultModelDisplay: diffModal.defaultModelDisplay,
          }}
          beforeText={diffModal.beforeText}
          afterText={diffModal.afterText}
          onClose={() => setDiffModal(null)}
          onConfirm={
            diffModal.actionKind === "preview"
              ? undefined
              : () => {
                  if (diffModal.actionKind === "repair") {
                    repairAgentDrift(diffModal.binding.agentId);
                  } else if (diffModal.actionKind === "unbind") {
                    unbindAgent(diffModal.binding.agentId);
                  } else if (
                    diffModal.actionKind === "update_mounts" &&
                    diffModal.nextPoolIds
                  ) {
                    updateAgentMountedPools(
                      diffModal.binding.agentId,
                      diffModal.nextPoolIds
                    );
                  }
                }
          }
        />
      )}

      <ConfirmDialog
        open={clearConfirm !== null}
        danger
        title={
          clearConfirm?.agentId
            ? "强制清除该 Agent 的 Qwitch 写入"
            : "全部清除所有 Agent 的 Qwitch 配置"
        }
        description={
          clearConfirm?.agentId
            ? "将仅删除该 Agent 配置中 Qwitch 写入的 provider / 路由 / 令牌键，保留其它配置。此操作可用「全部解绑并还原」前先备份，但强制清除不依赖备份。"
            : `将对 ${agentBindings.length} 个目标 Agent 执行清除：有备份的还原原始文件，无备份的强制删除 Qwitch 写入。不会删除账号、号池或停止网关。`
        }
        confirmLabel="确认清除"
        onCancel={() => setClearConfirm(null)}
        onConfirm={() => {
          const target = clearConfirm;
          setClearConfirm(null);
          if (!target) return;
          if (target.agentId) {
            clearAgentWiring(target.agentId, target.mode);
          } else {
            clearAllAgentWiring(target.mode, false);
          }
        }}
      />

      <GatewayPortModal open={portModalOpen} onClose={() => setPortModalOpen(false)} />

    </div>
  );
}
