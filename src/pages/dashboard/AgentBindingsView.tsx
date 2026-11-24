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
  Layers,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldAlert,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Square,
  Star,
  Terminal,
  Unlink,
  Wrench,
  X,
  Zap,
} from "lucide-react";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import {
  TOOL_SHORT_LABELS,
  WIRE_PROTOCOL_META,
  buildAgentConfigSnippets,
  computeAgentExposedModels,
  formatScopedModelId,
  splitScopedModelId,
} from "../../data/mockPoolData";
import type { AgentBinding, PoolStrategy, TargetAgentId } from "../../types/pool";
import { AgentBrandIcon } from "../../components/AgentBrandIcon";
import { ToolBrandIcon } from "../../components/ToolBrandIcon";
import { ConfigDiffModal, type DiffActionKind } from "../../components/pool/ConfigDiffModal";

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
  const padClass = compact ? "px-1.5 py-0.5 text-[10px]" : "";
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

export function AgentBindingsView({
  highlightAgentId,
}: {
  highlightAgentId?: TargetAgentId | null;
  onNavigatePools?: (poolId?: string) => void;
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
    repairAgentDrift,
    restartAgentClient,
    toggleAgentOption,
    injectMemberCooldown,
    clearMemberCooldown,
    authorizeCodeArtsIsolated,
    getAgentToolModelOptions,
    getAgentToolEnabledModelIds,
    getAgentToolAccounts,
    getAgentPrimaryPool,
    setAgentToolModels,
    selectAllAgentToolModels,
    selectRecommendedAgentToolModels,
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

  useEffect(() => {
    if (highlightAgentId) {
      setSelectedAgentId(highlightAgentId);
    }
  }, [highlightAgentId]);

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

  const activeExposedModels = useMemo(() => {
    if (!activeBinding) return [];
    return computeAgentExposedModels(activeBinding, pools, nowMs);
  }, [activeBinding, pools, nowMs]);

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

  const filteredToolModels = useMemo(() => {
    const q = modelSearch.trim().toLowerCase();
    return toolModelOptions.filter((opt) => {
      if (modelQuickFilter === "checked" && !enabledModelSet.has(opt.id)) {
        return false;
      }
      if (
        modelQuickFilter === "low_rate" &&
        !opt.isFree &&
        !(typeof opt.creditMultiplier === "number" && opt.creditMultiplier <= 0.25) &&
        !/flash|turbo|mini|fast/i.test(opt.id)
      ) {
        return false;
      }
      if (
        modelQuickFilter === "flagship" &&
        !(
          typeof opt.creditMultiplier === "number" && opt.creditMultiplier >= 0.45
        ) &&
        !/pro|max|k3|seed-2\.1|pangu|claude/i.test(opt.id)
      ) {
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
      if (
        opt.isFree ||
        (typeof opt.creditMultiplier === "number" && opt.creditMultiplier <= 0.25) ||
        /flash|turbo|mini|fast/i.test(opt.id)
      ) {
        lowRate += 1;
      }
      if (
        (typeof opt.creditMultiplier === "number" && opt.creditMultiplier >= 0.45) ||
        !/flash|turbo|mini|fast/i.test(opt.id)
      ) {
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
      void navigator.clipboard.writeText(text);
    }
    setCopiedId(id);
    window.setTimeout(() => {
      setCopiedId((prev) => (prev === id ? null : prev));
    }, 1500);
  };

  const openDiffForMountChange = (binding: AgentBinding, nextPoolIds: string[]) => {
    const snippets = buildAgentConfigSnippets(
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
      nowMs
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
  const customEndpointEnv = `export OPENAI_BASE_URL=http://${gateway.host}:${gateway.port}/v1
export OPENAI_API_KEY=qwitch-bind-${activeBinding?.agentId ?? "custom"}
# 直接指定订阅工具与模型（严格锁模不跨工具混调）：${effectiveCustomWire}
# 或 Anthropic Messages 协议:
export ANTHROPIC_BASE_URL=http://${gateway.host}:${gateway.port}
export ANTHROPIC_AUTH_TOKEN=qwitch-bind-${activeBinding?.agentId ?? "claude-desktop"}`;

  const activeToolName = TOOL_SHORT_LABELS[activeToolId] ?? activeToolId;
  const hasLiveDiscovered = toolModelOptions.some((o) => o.source === "live");
  const currentStrategy: PoolStrategy = activeAgentPool?.strategy ?? "order";
  const currentSkipActive = activeAgentPool?.skipActiveClientAccount ?? true;
  const currentCooldownSec = activeAgentPool?.cooldownSeconds ?? 60;

  const activeStrategyDesc =
    AGENT_STRATEGY_OPTIONS.find((opt) => opt.id === currentStrategy)?.desc ??
    AGENT_STRATEGY_OPTIONS[0].desc;

  const renderToolSwitcher = () => {
    if (!activeBinding) return null;
    return (
      <section className="surface-card p-3">
        <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-1.5 text-xs font-semibold text-zinc-800 dark:text-zinc-200">
            <Layers className="h-3.5 w-3.5 text-brand-600 dark:text-brand-400" aria-hidden="true" />
            <span>订阅工具（按工具独立计费与限流）</span>
          </div>
          <span className="text-[11px] text-zinc-400">
            切换不同工具配置开放模型与主备顺位
          </span>
        </div>

        <div
          role="tablist"
          aria-label="订阅工具切换栏"
          className="flex flex-wrap gap-1.5"
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

            return (
              <button
                key={toolId}
                type="button"
                role="tab"
                aria-selected={isActive}
                onClick={() => {
                  setSelectedToolId(toolId);
                  setModelSearch("");
                  setModelQuickFilter("all");
                }}
                className={`flex items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-left text-xs transition ${
                  isActive
                    ? "border-brand-500 bg-brand-500/10 font-semibold text-brand-900 shadow-2xs dark:border-brand-400 dark:text-brand-100"
                    : selectedToolCount > 0
                      ? "border-emerald-500/35 bg-emerald-500/[0.04] text-zinc-800 hover:bg-zinc-50 dark:text-zinc-200 dark:hover:bg-zinc-900"
                      : "border-zinc-200 bg-white text-zinc-600 hover:bg-zinc-50 dark:border-zinc-800 dark:bg-zinc-900/50 dark:text-zinc-400"
                }`}
              >
                <ToolBrandIcon toolId={toolId} size="xs" decorative />
                <div className="min-w-0">
                  <div className="flex items-center gap-1">
                    <span className="truncate">{toolName}</span>
                    {hasDefaultHere && (
                      <Star
                        className="h-3 w-3 shrink-0 fill-amber-500 text-amber-500"
                        aria-label="包含默认模型"
                      />
                    )}
                    <span
                      className={`text-[10px] font-normal tabular-nums ${
                        selectedToolCount > 0
                          ? "font-semibold text-brand-700 dark:text-brand-300"
                          : "text-zinc-400"
                      }`}
                    >
                      {selectedToolCount}/{totalToolModels}
                    </span>
                  </div>
                  <div className="text-[10px] font-normal text-zinc-500 dark:text-zinc-400">
                    {toolAccs.length > 0 ? `${toolAccs.length} 账号` : "未导入"}
                  </div>
                </div>
              </button>
            );
          })}
        </div>
      </section>
    );
  };

  return (
    <div className="space-y-3.5">
      {/* 顶部精简网关状态条（顶栏已提供启停/重探/测试路由主操作，此处仅作状态自检） */}
      <section className="surface-card flex flex-wrap items-center justify-between gap-2.5 px-3.5 py-2">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <div className="flex items-center gap-1.5">
            <span
              className={`h-2 w-2 rounded-full ${
                gateway.running ? "bg-emerald-500" : "bg-zinc-400"
              }`}
            />
            <span className="font-semibold text-zinc-900 dark:text-zinc-100">
              本地网关
            </span>
            <code className="rounded bg-zinc-100 px-1.5 py-0.5 font-mono text-[11px] tabular-nums text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
              {gateway.host}:{gateway.port}
            </code>
          </div>

          {isTauriRuntime ? (
            <>
              <span
                className={`badge ${
                  gateway.bridgeRuntime === "missing"
                    ? "border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300"
                    : "border-indigo-500/25 bg-indigo-500/10 text-indigo-700 dark:text-indigo-300"
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
                {gateway.globalProxyDetected ? "⚠️ 检测到全局代理" : "✓ 本机回环直连正常"}
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
                    : "border-indigo-500/25 bg-indigo-500/10 text-indigo-700 dark:text-indigo-300"
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
                {gateway.globalProxyDetected ? "⚠️ 检测到全局代理" : "✓ 本机回环直连正常"}
              </button>
            </>
          )}
        </div>

        <span className="text-[11px] text-zinc-500 dark:text-zinc-400">
          先选 Agent → 配置模型 → 按需在「账号调度」调整主备顺位
        </span>
      </section>

      {/* 顶部 4 格紧凑目标 Agent 切换栏 */}
      <aside
        aria-label="目标 Agent 客户端列表"
        className="grid grid-cols-1 gap-2 @lg:grid-cols-2 @3xl:grid-cols-4"
      >
        {agentBindings.map((binding) => {
          const isSelected = activeBinding?.agentId === binding.agentId;
          const exposed = computeAgentExposedModels(binding, pools, nowMs);
          const defModel = exposed.find((m) => m.isDefault) ?? exposed[0];

          return (
            <section
              key={binding.agentId}
              aria-label={`${binding.name} 挂载卡片`}
              className={`surface-card relative flex flex-col justify-between gap-2 p-2.5 transition ${
                isSelected
                  ? "border-brand-500 bg-brand-500/[0.05] ring-2 ring-brand-500/20 dark:border-brand-400 dark:bg-brand-500/[0.08]"
                  : binding.status === "drifted"
                    ? "border-amber-500/50 hover:border-amber-500"
                    : "hover:border-zinc-300 dark:hover:border-zinc-700"
              }`}
            >
              <button
                type="button"
                onClick={() => setSelectedAgentId(binding.agentId)}
                aria-pressed={isSelected}
                className="flex w-full flex-col gap-2 text-left"
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="flex min-w-0 items-center gap-2">
                    <AgentBrandIcon agentId={binding.agentId} size="sm" />
                    <div className="min-w-0">
                      <h2 className="truncate text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                        {binding.name}
                      </h2>
                      <span
                        className={`mt-0.5 inline-flex rounded px-1 py-0.2 text-[10px] ${WIRE_PROTOCOL_META[binding.protocol].badgeClass}`}
                      >
                        {WIRE_PROTOCOL_META[binding.protocol].label}
                      </span>
                    </div>
                  </div>
                  {renderStatusBadge(binding.status, true)}
                </div>

                <div className="flex items-center justify-between gap-1.5 rounded-lg bg-zinc-100/75 px-2 py-1 text-[11px] dark:bg-zinc-950/60">
                  {defModel ? (
                    <span className="flex min-w-0 items-center gap-1 font-medium text-zinc-800 dark:text-zinc-200">
                      <Star
                        className="h-3 w-3 shrink-0 fill-amber-500 text-amber-500"
                        aria-hidden="true"
                      />
                      <span className="truncate font-mono">{defModel.displayName}</span>
                    </span>
                  ) : (
                    <span className="text-zinc-400">未配置模型</span>
                  )}
                  <span className="shrink-0 rounded bg-white px-1.5 py-0.5 text-[10px] font-medium tabular-nums text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                    {exposed.length} 模型
                  </span>
                </div>
              </button>

              {binding.status === "drifted" && (
                <button
                  type="button"
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
                  className="btn btn-xs btn-primary w-full justify-center gap-1"
                >
                  <Wrench className="h-3 w-3" aria-hidden="true" />
                  查看差异并修复
                </button>
              )}
            </section>
          );
        })}
      </aside>

      {/* 当前选中 Agent 的分标签工作区 */}
      {activeBinding && (
        <main
          aria-label={`${activeBinding.name} 专属配置工作室`}
          className="min-w-0 space-y-3.5"
        >
          {/* A. Agent 头部状态、主操作与 3 分区 Tab 栏 */}
          <section className="surface-card space-y-3 p-3.5">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="flex min-w-0 items-center gap-3">
                <AgentBrandIcon agentId={activeBinding.agentId} size="md" />
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <h2 className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
                      {activeBinding.name}
                    </h2>
                    <span className="font-mono text-[11px] text-zinc-400">
                      v{activeBinding.version}
                    </span>
                    {renderStatusBadge(activeBinding.status)}
                    <span
                      className={`badge ${WIRE_PROTOCOL_META[activeBinding.protocol].badgeClass}`}
                    >
                      {WIRE_PROTOCOL_META[activeBinding.protocol].label}
                    </span>
                  </div>
                  <div className="mt-0.5 flex flex-wrap items-center gap-2 text-[11px] text-zinc-500 dark:text-zinc-400">
                    <code className="truncate font-mono">{activeBinding.configPath}</code>
                    {activeBinding.lastHitAt && (
                      <span>· 最近调用：{activeBinding.lastHitAt}</span>
                    )}
                  </div>
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-1.5">
                <button
                  type="button"
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
                  <FileCode2 className="h-3.5 w-3.5" aria-hidden="true" />
                  预览配置 Diff
                </button>

                {activeBinding.status === "drifted" ? (
                  <button
                    type="button"
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
                    <Wrench className="h-3 w-3" aria-hidden="true" />
                    查看差异并修复
                  </button>
                ) : activeBinding.status === "needs_restart" ? (
                  <button
                    type="button"
                    onClick={() => restartAgentClient(activeBinding.agentId)}
                    className="btn btn-xs btn-sky-soft gap-1"
                  >
                    <RotateCcw className="h-3 w-3" aria-hidden="true" />
                    {activeBinding.agentId === "codex_desktop" ? "重启 app-server" : "立即重启生效"}
                  </button>
                ) : null}

                {activeBinding.mountedPoolIds.length > 0 ? (
                  <button
                    type="button"
                    onClick={() => openDiffForMountChange(activeBinding, [])}
                    className="btn btn-xs btn-danger gap-1"
                  >
                    <Unlink className="h-3 w-3" aria-hidden="true" />
                    全部解绑并还原
                  </button>
                ) : (
                  <button
                    type="button"
                    disabled={toolModelOptions.length === 0}
                    onClick={() =>
                      selectRecommendedAgentToolModels(activeBinding.agentId, activeToolId)
                    }
                    className="btn btn-xs btn-primary gap-1"
                  >
                    <Zap className="h-3 w-3" aria-hidden="true" />
                    一键接入推荐模型
                  </button>
                )}
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

            {/* 3 分区二级导航 Tab */}
            <div
              role="tablist"
              aria-label="Agent 配置分区"
              className="flex flex-wrap items-center gap-1 border-t border-zinc-100 pt-2.5 dark:border-zinc-800/80"
            >
              <button
                type="button"
                role="tab"
                aria-selected={subTab === "models"}
                onClick={() => setSubTab("models")}
                className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
                  subTab === "models"
                    ? "bg-brand-600 text-white shadow-2xs dark:bg-brand-500 dark:text-zinc-950"
                    : "text-zinc-600 hover:bg-zinc-100 dark:text-zinc-400 dark:hover:bg-zinc-800/80"
                }`}
              >
                <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
                <span>模型配置</span>
                <span className="rounded px-1 py-0.2 font-mono text-[10px] opacity-85">
                  {activeExposedModels.length}
                </span>
              </button>

              <button
                type="button"
                role="tab"
                aria-selected={subTab === "scheduling"}
                onClick={() => setSubTab("scheduling")}
                className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
                  subTab === "scheduling"
                    ? "bg-brand-600 text-white shadow-2xs dark:bg-brand-500 dark:text-zinc-950"
                    : "text-zinc-600 hover:bg-zinc-100 dark:text-zinc-400 dark:hover:bg-zinc-800/80"
                }`}
              >
                <Crown className="h-3.5 w-3.5" aria-hidden="true" />
                <span>账号调度</span>
                <span className="rounded px-1 py-0.2 font-mono text-[10px] opacity-85">
                  {toolAccounts.filter((a) => a.enabled).length}/{toolAccounts.length}
                </span>
              </button>

              <button
                type="button"
                role="tab"
                aria-selected={subTab === "connection"}
                onClick={() => setSubTab("connection")}
                className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
                  subTab === "connection"
                    ? "bg-brand-600 text-white shadow-2xs dark:bg-brand-500 dark:text-zinc-950"
                    : "text-zinc-600 hover:bg-zinc-100 dark:text-zinc-400 dark:hover:bg-zinc-800/80"
                }`}
              >
                <SlidersHorizontal className="h-3.5 w-3.5" aria-hidden="true" />
                <span>接入设置</span>
              </button>
            </div>
          </section>

          {/* 分区 1：模型配置（全宽订阅模型列表 + 底部实时预览） */}
          {subTab === "models" && (
            <div className="space-y-3.5">
              {renderToolSwitcher()}

              <section
                aria-label={`${activeToolName} 订阅模型选择`}
                className="surface-card flex flex-col justify-between gap-3 p-4"
              >
                <div className="space-y-3">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <div className="flex items-center gap-2">
                        <h3 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                          {activeToolName} 订阅模型
                        </h3>
                        <span
                          className={`badge ${
                            hasLiveDiscovered
                              ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                              : "badge-neutral"
                          }`}
                        >
                          {hasLiveDiscovered ? "实时接口探测" : "真实协议目录"}
                        </span>
                      </div>
                      <p className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                        已勾选{" "}
                        <strong className="text-zinc-900 dark:text-zinc-100">
                          {enabledBareModelIds.length}
                        </strong>{" "}
                        / {toolModelOptions.length} 个模型（将以{" "}
                        <code>{activeToolId}/模型名</code> 暴露给 {activeBinding.name}，勾选即实时写入配置）
                      </p>
                    </div>

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

                  {/* 搜索与快速筛选栏 */}
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="relative min-w-[200px] flex-1">
                      <Search
                        className="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400"
                        aria-hidden="true"
                      />
                      <input
                        type="text"
                        aria-label="搜索订阅模型"
                        value={modelSearch}
                        onChange={(e) => setModelSearch(e.target.value)}
                        placeholder={`搜索 ${activeToolName} 模型名、家族或 Wire ID...`}
                        className="w-full rounded-xl border border-zinc-200 bg-zinc-50 py-1.5 pr-8 pl-8 text-xs text-zinc-900 outline-none transition focus:border-brand-500 focus:bg-white dark:border-zinc-700 dark:bg-zinc-950/60 dark:text-zinc-100"
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

                    <div className="flex flex-wrap items-center gap-1">
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
                          onClick={() => setModelQuickFilter(tab.id)}
                          className={`rounded-lg px-2 py-1 text-[11px] font-medium transition ${
                            modelQuickFilter === tab.id
                              ? "bg-brand-600 text-white dark:bg-brand-500"
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
                    className="flex flex-wrap items-center gap-1.5 rounded-xl border border-zinc-200/80 bg-zinc-50/80 p-2 dark:border-zinc-800 dark:bg-zinc-950/50"
                  >
                    <button
                      type="button"
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
                      onClick={() =>
                        selectRecommendedAgentToolModels(activeBinding.agentId, activeToolId)
                      }
                      className="btn btn-xs btn-secondary gap-1"
                    >
                      <Sparkles className="h-3 w-3 text-amber-500" aria-hidden="true" />
                      仅选推荐主力
                    </button>
                    <button
                      type="button"
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
                      disabled={enabledBareModelIds.length === 0}
                      onClick={() =>
                        clearAgentToolModels(activeBinding.agentId, activeToolId)
                      }
                      className="btn btn-xs btn-secondary text-rose-600 hover:bg-rose-50 disabled:opacity-45 dark:text-rose-400 dark:hover:bg-rose-950/40"
                    >
                      清空本工具
                    </button>
                  </div>

                  {/* 双列宽松模型复选列表 */}
                  <div className="grid max-h-[380px] grid-cols-1 gap-1.5 overflow-y-auto pr-1 @2xl:grid-cols-2">
                    {filteredToolModels.length === 0 ? (
                      <div className="col-span-full rounded-xl border border-dashed border-zinc-200 py-8 text-center text-xs text-zinc-400 dark:border-zinc-800">
                        当前筛选条件下无匹配模型
                      </div>
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
                                  <span className="rounded bg-zinc-100 px-1.5 py-0.5 text-[10px] text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400">
                                    {opt.family}
                                  </span>
                                  {opt.isFree ? (
                                    <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] font-medium text-emerald-700 dark:text-emerald-300">
                                      免费
                                    </span>
                                  ) : typeof opt.creditMultiplier === "number" ? (
                                    <span
                                      className={`rounded px-1.5 py-0.5 font-mono text-[10px] tabular-nums ${
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
                                onClick={() =>
                                  setAgentDefaultToolModel(
                                    activeBinding.agentId,
                                    activeToolId,
                                    opt.id
                                  )
                                }
                                className="shrink-0 rounded-lg px-2 py-1 text-[11px] font-medium text-brand-700 transition hover:bg-brand-500/10 dark:text-brand-300"
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
            <div className="space-y-3.5">
              {renderToolSwitcher()}

              <section
                aria-label={`${activeToolName} 同工具账号调度方案`}
                className="surface-card flex flex-col gap-3.5 p-4"
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
                  <div className="grid grid-cols-1 gap-1.5 sm:grid-cols-3">
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
                        className="rounded-lg border border-zinc-200 bg-white px-1.5 py-0.5 font-mono text-[11px] dark:border-zinc-700 dark:bg-zinc-800"
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
                                  className={`rounded-md px-1.5 py-0.5 text-[10px] font-semibold ${
                                    isPrimary
                                      ? "bg-brand-600 text-white dark:bg-brand-500"
                                      : "bg-zinc-200/80 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                                  }`}
                                >
                                  {isPrimary ? "#1 主账号" : `#${idx + 1} 备用账号`}
                                </span>
                                <span className="truncate text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                                  {acc.label}
                                </span>
                                <code className="text-[10px] text-zinc-400">{acc.uidMasked}</code>
                              </div>

                              <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-[10px]">
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
                              {!isPrimary && !isReadonlyCodeArts && (
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
                              <span className="truncate font-mono text-[10px] tabular-nums text-zinc-500 dark:text-zinc-400">
                                {acc.quotaPercent}% · {acc.quotaText}
                              </span>
                            </div>

                            <div className="flex shrink-0 items-center gap-1.5">
                              {isReadonlyCodeArts ? (
                                <button
                                  type="button"
                                  onClick={() => authorizeCodeArtsIsolated(acc.accountId)}
                                  className="btn btn-xs btn-primary"
                                >
                                  签发独立 OAuth
                                </button>
                              ) : (
                                <>
                                  {activeAgentPool && !acc.id.startsWith("cand-") && (
                                    cooldownRemain > 0 ? (
                                      <button
                                        type="button"
                                        onClick={() =>
                                          clearMemberCooldown(activeAgentPool.id, acc.id)
                                        }
                                        className="rounded-lg border border-emerald-500/40 px-1.5 py-0.5 text-[10px] font-medium text-emerald-700 hover:bg-emerald-500/10 dark:text-emerald-300"
                                      >
                                        解除冷却
                                      </button>
                                    ) : (
                                      <button
                                        type="button"
                                        onClick={() =>
                                          injectMemberCooldown(activeAgentPool.id, acc.id)
                                        }
                                        className="rounded-lg border border-zinc-200 px-1.5 py-0.5 text-[10px] text-zinc-500 hover:bg-zinc-100 dark:border-zinc-700 dark:hover:bg-zinc-800"
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
                                    className={`rounded-lg px-2 py-0.5 text-[10px] font-medium transition ${
                                      acc.enabled
                                        ? "bg-emerald-500/15 text-emerald-800 hover:bg-emerald-500/25 dark:text-emerald-300"
                                        : "bg-zinc-200 text-zinc-600 hover:bg-zinc-300 dark:bg-zinc-800 dark:text-zinc-400"
                                    }`}
                                  >
                                    {acc.enabled ? "参与调度" : "已停用"}
                                  </button>
                                </>
                              )}
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
                      {activeBinding.mountedPoolIds.length > 0 && (
                        <button
                          type="button"
                          onClick={() => restartAgentClient("codex_desktop")}
                          className="btn btn-xs btn-secondary"
                        >
                          重启 app-server
                        </button>
                      )}
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
                        OpenCode JSONC 键控模型表格式（切换即时重写 <code>opencode.jsonc</code>）
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
                </div>
              </section>

              {/* 通用标准端点生成卡 */}
              <section className="surface-card p-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="flex items-center gap-2.5">
                    <AgentBrandIcon agentId="custom_endpoint" size="sm" />
                    <div>
                      <h2 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                        通用标准网关端点（适用于任意兼容 OpenAI / Anthropic 的客户端）
                      </h2>
                      <p className="text-[11px] text-zinc-500 dark:text-zinc-400">
                        选择已接入的模型后直接生成环境变量，请求将自动按当前 Agent 的同工具主备顺位路由
                      </p>
                    </div>
                  </div>

                  <div className="flex flex-wrap items-center gap-2">
                    <select
                      value={effectiveCustomWire}
                      onChange={(e) => setCustomModelWire(e.target.value)}
                      aria-label="选择导出环境变量的目标模型"
                      className="rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 font-mono text-xs dark:border-zinc-700 dark:bg-zinc-800"
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
        </main>
      )}

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
    </div>
  );
}
