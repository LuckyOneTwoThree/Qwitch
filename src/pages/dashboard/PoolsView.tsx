import { useMemo, useState } from "react";
import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  Check,
  Clock,
  Copy,
  Cpu,
  Flame,
  Info,
  Layers,
  Link2,
  Play,
  Plus,
  RefreshCw,
  Settings2,
  ShieldAlert,
  ShieldCheck,
  Trash2,
  Unlink,
  Zap,
} from "lucide-react";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import {
  AFFINITY_MODE_META,
  CHANNEL_META,
  MEMBER_TIER_META,
  POOL_STRATEGY_META,
  buildAgentConfigSnippets,
  computeAgentExposedModels,
  computePoolAggregateQuota,
  computePoolServingAccounts,
  formatModelRateBadge,
  getMemberModelOptions,
  isHeterogeneousPool,
} from "../../data/mockPoolData";
import type {
  AccountPool,
  AffinityMode,
  AgentBinding,
  MemberTier,
  PoolStrategy,
  TargetAgentId,
} from "../../types/pool";
import { AgentBrandIcon, UpstreamMemberIcon } from "../../components/AgentBrandIcon";
import { AddPoolMembersModal } from "../../components/pool/AddPoolMembersModal";
import { BindAgentModal } from "../../components/pool/BindAgentModal";
import { ConfigDiffModal, type DiffActionKind } from "../../components/pool/ConfigDiffModal";
import { ConfirmDialog } from "../../components/ui/ConfirmDialog";

type PoolDetailTab = "models_members" | "strategy" | "agents";

const TIER_ORDER: MemberTier[] = ["t1_primary", "t2_backup", "t3_fallback"];

export function PoolsView({
  onOpenCreatePool,
  onOpenSimulation,
  onNavigateAgents,
}: {
  onOpenCreatePool: () => void;
  onOpenSimulation: (poolId?: string, modelId?: string) => void;
  onNavigateAgents: (highlightAgentId?: TargetAgentId) => void;
}) {
  const {
    gateway,
    pools,
    selectedPoolId,
    setSelectedPoolId,
    candidates,
    agentBindings,
    nowMs,
    isRefreshingModels,
    refreshUpstreamModels,
    updatePool,
    deletePool,
    togglePoolEnabled,
    reorderPoolMember,
    togglePoolMember,
    togglePoolMemberModel,
    updatePoolMemberTier,
    togglePoolModelEnabled,
    setPoolModelFallback,
    removePoolMember,
    injectMemberCooldown,
    clearMemberCooldown,
    mountPoolToAgents,
    unmountPoolFromAgent,
  } = usePoolPrototype();

  const [detailTab, setDetailTab] = useState<PoolDetailTab>("models_members");
  const [addMembersOpen, setAddMembersOpen] = useState(false);
  const [bindModalPool, setBindModalPool] = useState<AccountPool | null>(null);
  const [confirmDeletePool, setConfirmDeletePool] = useState<AccountPool | null>(null);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);
  const [diffState, setDiffState] = useState<{
    binding: AgentBinding;
    actionKind: DiffActionKind;
    nextPoolIds: string[];
    poolId: string;
    beforeText: string;
    afterText: string;
  } | null>(null);

  const selectedPool = useMemo(
    () => pools.find((p) => p.id === selectedPoolId) ?? pools[0] ?? null,
    [pools, selectedPoolId]
  );

  const bindingsByPoolId = useMemo(() => {
    const map = new Map<string, AgentBinding[]>();
    for (const b of agentBindings) {
      for (const pid of b.mountedPoolIds) {
        const list = map.get(pid) ?? [];
        list.push(b);
        map.set(pid, list);
      }
    }
    return map;
  }, [agentBindings]);

  const sortedMembers = useMemo(() => {
    if (!selectedPool) return [];
    return [...selectedPool.members].sort((a, b) => a.priority - b.priority);
  }, [selectedPool]);

  const membersByTier = useMemo(() => {
    const grouped: Record<MemberTier, typeof sortedMembers> = {
      t1_primary: [],
      t2_backup: [],
      t3_fallback: [],
    };
    for (const m of sortedMembers) {
      grouped[m.tier].push(m);
    }
    return grouped;
  }, [sortedMembers]);

  const readyCandidateCount = useMemo(
    () =>
      candidates.filter(
        (c) => c.accountStatus === "active" && c.codeArtsMode !== "client_readonly"
      ).length,
    [candidates]
  );
  const enabledPoolCount = useMemo(
    () => pools.filter((p) => p.enabled).length,
    [pools]
  );
  const boundAgentCount = useMemo(
    () => agentBindings.filter((b) => b.mountedPoolIds.length > 0).length,
    [agentBindings]
  );
  const selectedIsHeterogeneous = useMemo(
    () => (selectedPool ? isHeterogeneousPool(selectedPool.members) : false),
    [selectedPool]
  );

  const copyText = (key: string, text: string) => {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(text);
    }
    setCopiedKey(key);
    window.setTimeout(() => {
      setCopiedKey((prev) => (prev === key ? null : prev));
    }, 1500);
  };

  const openMountDiff = (binding: AgentBinding, nextPoolIds: string[], poolId: string) => {
    const snippets = buildAgentConfigSnippets(
      binding.agentId,
      nextPoolIds,
      binding.defaultModelKey,
      pools,
      gateway,
      binding.options,
      false
    );
    setDiffState({
      binding,
      actionKind: nextPoolIds.includes(poolId) ? "update_mounts" : "unbind",
      nextPoolIds,
      poolId,
      beforeText: snippets.beforeSnippet,
      afterText: snippets.afterSnippet,
    });
  };

  return (
    <div className="flex min-h-0 flex-col gap-3.5 @3xl:h-full">
      {/* 顶部流程态势条 */}
      <section className="surface-card flex shrink-0 flex-wrap items-center justify-between gap-3 px-4 py-2.5">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <div className="inline-flex items-center gap-1.5 rounded-lg bg-zinc-100 px-2.5 py-1.5 font-medium text-zinc-700 dark:bg-zinc-800 dark:text-zinc-200">
            <span className="h-2 w-2 rounded-full bg-emerald-500" />
            <span>可用账号源</span>
            <span className="font-mono font-semibold tabular-nums">{readyCandidateCount}</span>
          </div>
          <ArrowRight className="h-3.5 w-3.5 text-zinc-400" aria-hidden="true" />
          <div className="inline-flex items-center gap-1.5 rounded-lg bg-brand-500/12 px-2.5 py-1.5 font-medium text-brand-800 dark:text-brand-200">
            <Layers className="h-3.5 w-3.5" aria-hidden="true" />
            <span>活跃号池</span>
            <span className="font-mono font-semibold tabular-nums">
              {enabledPoolCount}/{pools.length}
            </span>
          </div>
          <ArrowRight className="h-3.5 w-3.5 text-zinc-400" aria-hidden="true" />
          <button
            type="button"
            onClick={() => onNavigateAgents()}
            className="inline-flex items-center gap-1.5 rounded-lg bg-sky-500/12 px-2.5 py-1.5 font-medium text-sky-800 transition hover:bg-sky-500/20 dark:text-sky-200"
          >
            <Link2 className="h-3.5 w-3.5" aria-hidden="true" />
            <span>已挂载 Agent（支持多池）</span>
            <span className="font-mono font-semibold tabular-nums">
              {boundAgentCount}/{agentBindings.length}
            </span>
          </button>
        </div>

        <div className="flex items-center gap-2">
          <button
            type="button"
            disabled={!selectedPool}
            onClick={() => onOpenSimulation(selectedPool?.id)}
            className="btn btn-sm btn-secondary gap-1.5"
          >
            <Play className="h-3.5 w-3.5" aria-hidden="true" />
            测试路由 / Failover
          </button>
          <button
            type="button"
            onClick={onOpenCreatePool}
            className="btn btn-sm btn-primary gap-1.5"
          >
            <Plus className="h-3.5 w-3.5" aria-hidden="true" />
            新建号池
          </button>
        </div>
      </section>

      {pools.length === 0 ? (
        <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-300/80 bg-white/60 px-6 py-14 text-center dark:border-zinc-700/70 dark:bg-zinc-900/30">
          <Layers className="h-8 w-8 text-zinc-400" aria-hidden="true" />
          <h2 className="mt-3 text-sm font-semibold text-zinc-900 dark:text-zinc-100">
            尚未创建任何调度号池
          </h2>
          <p className="mt-1 max-w-md text-xs text-zinc-500 dark:text-zinc-400">
            先把已导入的 7 家工具账号按梯队组成号池并勾选开放模型，再一键挂载给多个消费端 Agent。
          </p>
          <button
            type="button"
            onClick={onOpenCreatePool}
            className="btn btn-md btn-primary mt-4 gap-1.5"
          >
            <Plus className="h-3.5 w-3.5" aria-hidden="true" />
            立即创建第一个号池
          </button>
        </div>
      ) : (
        /* 主从双栏独立滚动工作台 */
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-4 @3xl:grid-cols-[290px_minmax(0,1fr)]">
          {/* 左栏：号池列表（宽屏下独立滚动，永不随右侧下滑而消失） */}
          <div
            className="space-y-2.5 @3xl:min-h-0 @3xl:overflow-y-auto @3xl:pr-1"
            role="list"
            aria-label="号池清单"
          >
            {pools.map((pool) => {
              const selected = selectedPool?.id === pool.id;
              const aggQuota = computePoolAggregateQuota(pool.members);
              const boundAgents = bindingsByPoolId.get(pool.id) ?? [];
              const uniqueTools = Array.from(
                new Set(pool.members.map((m) => m.toolId))
              );
              const enabledModels = pool.modelPolicies.filter((m) => m.enabled);

              return (
                <div
                  key={pool.id}
                  role="listitem"
                  className={`surface-card surface-interactive cursor-pointer p-3.5 transition ${
                    selected
                      ? "border-brand-500 ring-2 ring-brand-500/20 dark:border-brand-400/70"
                      : ""
                  } ${!pool.enabled ? "opacity-75" : ""}`}
                  onClick={() => setSelectedPoolId(pool.id)}
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-1">
                        <span className="badge badge-brand">{pool.sceneTag}</span>
                        <span className="badge badge-neutral">
                          {POOL_STRATEGY_META[pool.strategy].shortLabel}
                        </span>
                      </div>
                      <h3 className="mt-1.5 truncate text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                        {pool.name}
                      </h3>
                    </div>

                    <button
                      type="button"
                      role="switch"
                      aria-checked={pool.enabled}
                      aria-label={`${pool.name} 启停开关`}
                      onClick={(e) => {
                        e.stopPropagation();
                        togglePoolEnabled(pool.id);
                      }}
                      className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition ${
                        pool.enabled
                          ? "bg-brand-600 dark:bg-brand-500"
                          : "bg-zinc-300 dark:bg-zinc-700"
                      }`}
                    >
                      <span
                        className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition ${
                          pool.enabled ? "translate-x-4.5" : "translate-x-1"
                        }`}
                      />
                    </button>
                  </div>

                  {/* 对外开放模型概览 */}
                  <div className="mt-2 flex flex-wrap gap-1">
                    {enabledModels.slice(0, 2).map((mp) => (
                      <span
                        key={mp.modelId}
                        className="rounded border border-zinc-200/80 bg-zinc-50 px-1.5 py-0.5 font-mono text-[10px] text-zinc-600 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                      >
                        {mp.modelId}
                      </span>
                    ))}
                    {enabledModels.length > 2 && (
                      <span className="rounded bg-zinc-100 px-1.5 py-0.5 text-[10px] tabular-nums text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400">
                        +{enabledModels.length - 2} 模型
                      </span>
                    )}
                  </div>

                  {/* 堆叠工具图标 + 水位 */}
                  <div className="mt-2.5 flex items-center justify-between gap-2">
                    <div className="flex items-center">
                      <div className="flex -space-x-1.5">
                        {uniqueTools.slice(0, 5).map((tid) => (
                          <UpstreamMemberIcon
                            key={tid}
                            toolId={tid}
                            size="xs"
                            className="ring-2 ring-white dark:ring-zinc-900"
                          />
                        ))}
                      </div>
                      <span className="ml-2 text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                        {pool.members.length} 账号 · {enabledModels.length} 模型
                      </span>
                    </div>
                    <span className="text-xs font-semibold tabular-nums text-zinc-800 dark:text-zinc-200">
                      均余 {aggQuota}%
                    </span>
                  </div>

                  <div className="meter mt-1.5">
                    <div
                      className={`h-full rounded-full transition-all duration-300 ${
                        aggQuota <= 20
                          ? "bg-rose-500"
                          : aggQuota <= 40
                            ? "bg-amber-500"
                            : "meter-fill"
                      }`}
                      style={{ width: `${aggQuota}%` }}
                    />
                  </div>

                  {/* 已挂载 Agent */}
                  <div className="mt-2.5 flex flex-wrap items-center justify-between gap-1.5 border-t border-zinc-200/70 pt-2 dark:border-zinc-800/80">
                    {boundAgents.length > 0 ? (
                      <div className="flex flex-wrap items-center gap-1">
                        {boundAgents.map((ab) => (
                          <button
                            key={ab.agentId}
                            type="button"
                            onClick={(e) => {
                              e.stopPropagation();
                              onNavigateAgents(ab.agentId);
                            }}
                            className="inline-flex items-center gap-1 rounded-md bg-zinc-100 px-1.5 py-0.5 text-[11px] font-medium text-zinc-700 transition hover:bg-zinc-200 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700"
                          >
                            <AgentBrandIcon agentId={ab.agentId} size="xs" />
                            <span>{ab.name}</span>
                          </button>
                        ))}
                      </div>
                    ) : (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          setBindModalPool(pool);
                        }}
                        className="text-[11px] font-medium text-brand-700 hover:underline dark:text-brand-300"
                      >
                        + 挂载到 Agent
                      </button>
                    )}

                    <span className="text-[11px] tabular-nums text-zinc-400">
                      今日 {pool.todayRequests} 次
                    </span>
                  </div>
                </div>
              );
            })}
          </div>

          {/* 右栏：选中号池精密编排面板（固定头部 + 3 分页独立滚动） */}
          {selectedPool && (
            <div className="surface-card flex flex-col overflow-hidden @3xl:min-h-0">
              {/* 固定顶栏 + 分页开关 */}
              <div className="shrink-0 space-y-3 border-b border-zinc-200/80 px-4.5 py-3.5 dark:border-zinc-800">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <h2 className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
                        {selectedPool.name}
                      </h2>
                      <span className="badge badge-brand">{selectedPool.sceneTag}</span>
                      <span
                        className={`badge ${
                          selectedIsHeterogeneous
                            ? "border-indigo-500/25 bg-indigo-500/10 text-indigo-700 dark:text-indigo-300"
                            : "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                        }`}
                      >
                        {selectedIsHeterogeneous ? "跨工具混编 · T1→T2→T3" : "同工具纯血池"}
                      </span>
                      {!selectedPool.enabled && (
                        <span className="badge border-amber-500/30 bg-amber-500/12 text-amber-800 dark:text-amber-300">
                          已暂停路由
                        </span>
                      )}
                    </div>
                    <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">
                      {selectedPool.description}
                    </p>
                  </div>

                  <div className="flex flex-wrap items-center gap-1.5">
                    <button
                      type="button"
                      onClick={() => setBindModalPool(selectedPool)}
                      className="btn btn-sm btn-brand-soft gap-1.5"
                    >
                      <Link2 className="h-3.5 w-3.5" aria-hidden="true" />
                      挂载 Agent
                    </button>
                    <button
                      type="button"
                      onClick={() => onOpenSimulation(selectedPool.id)}
                      className="btn btn-sm btn-secondary gap-1.5"
                    >
                      <Play className="h-3.5 w-3.5" aria-hidden="true" />
                      演练切号
                    </button>
                    <button
                      type="button"
                      onClick={() => setConfirmDeletePool(selectedPool)}
                      aria-label={`删除号池 ${selectedPool.name}`}
                      className="btn btn-icon btn-danger"
                      title="删除号池"
                    >
                      <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                  </div>
                </div>

                {/* 右侧 3 分页导航 */}
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div
                    role="tablist"
                    aria-label="号池编排分页"
                    className="inline-flex rounded-xl bg-zinc-100 p-1 dark:bg-zinc-800/80"
                  >
                    <button
                      type="button"
                      role="tab"
                      aria-selected={detailTab === "models_members"}
                      onClick={() => setDetailTab("models_members")}
                      className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1 text-xs font-medium transition ${
                        detailTab === "models_members"
                          ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                          : "text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
                      }`}
                    >
                      <Cpu className="h-3.5 w-3.5" aria-hidden="true" />
                      模型与账号轨道 (
                      {selectedPool.modelPolicies.filter((p) => p.enabled).length}/
                      {selectedPool.members.length})
                    </button>

                    <button
                      type="button"
                      role="tab"
                      aria-selected={detailTab === "strategy"}
                      onClick={() => setDetailTab("strategy")}
                      className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1 text-xs font-medium transition ${
                        detailTab === "strategy"
                          ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                          : "text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
                      }`}
                    >
                      <Settings2 className="h-3.5 w-3.5" aria-hidden="true" />
                      策略与护栏 · {POOL_STRATEGY_META[selectedPool.strategy].shortLabel}
                    </button>

                    <button
                      type="button"
                      role="tab"
                      aria-selected={detailTab === "agents"}
                      onClick={() => setDetailTab("agents")}
                      className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1 text-xs font-medium transition ${
                        detailTab === "agents"
                          ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                          : "text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
                      }`}
                    >
                      <Zap className="h-3.5 w-3.5" aria-hidden="true" />
                      已挂载 Agent ({(bindingsByPoolId.get(selectedPool.id) ?? []).length})
                    </button>
                  </div>

                  <span className="text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400">
                    今日 {selectedPool.todayRequests} 次 · Failover{" "}
                    {selectedPool.todayFailovers} 次 · Cache 命中{" "}
                    {selectedPool.cacheHitRate}%
                  </span>
                </div>
              </div>

              {/* 独立滚动内容区 */}
              <div className="min-h-0 flex-1 space-y-5 overflow-y-auto p-4.5">
                {detailTab === "models_members" && (
                  <>
                    {/* 区块 1：对外开放模型清单（首要选择项） */}
                    <section className="space-y-2.5">
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <div>
                          <h3 className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                            一、对外开放的模型列表（消费端 Agent 下拉框直接选择）
                          </h3>
                          <p className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                            网关严格锁定用户所选模型，仅在具备该模型的同模账号间自动切换；仅配置了「显式降级」才会在全号耗尽后转入备用模型
                          </p>
                        </div>
                      </div>

                      <div className="space-y-2">
                        {selectedPool.modelPolicies.map((policy) => {
                          const serving = computePoolServingAccounts(
                            selectedPool,
                            policy.modelId,
                            nowMs
                          );
                          const isDisguised =
                            Boolean(policy.claudeAliasId) &&
                            policy.claudeAliasId !== policy.modelId;
                          const otherEnabledModels = selectedPool.modelPolicies.filter(
                            (p) => p.modelId !== policy.modelId && p.enabled
                          );

                          return (
                            <div
                              key={policy.modelId}
                              className={`flex flex-wrap items-center justify-between gap-2.5 rounded-xl border px-3 py-2.5 transition ${
                                policy.enabled
                                  ? "border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900/70"
                                  : "border-zinc-200/60 bg-zinc-50/70 opacity-65 dark:border-zinc-800 dark:bg-zinc-900/35"
                              }`}
                            >
                              <div className="flex min-w-0 flex-1 items-center gap-2.5">
                                <button
                                  type="button"
                                  role="switch"
                                  aria-checked={policy.enabled}
                                  aria-label={`对外暴露模型 ${policy.modelId}`}
                                  onClick={() =>
                                    togglePoolModelEnabled(selectedPool.id, policy.modelId)
                                  }
                                  className={`relative inline-flex h-4.5 w-8 shrink-0 items-center rounded-full transition ${
                                    policy.enabled
                                      ? "bg-brand-600 dark:bg-brand-500"
                                      : "bg-zinc-300 dark:bg-zinc-700"
                                  }`}
                                >
                                  <span
                                    className={`inline-block h-3 w-3 transform rounded-full bg-white shadow transition ${
                                      policy.enabled ? "translate-x-4" : "translate-x-0.5"
                                    }`}
                                  />
                                </button>

                                <div className="min-w-0 flex-1">
                                  <div className="flex flex-wrap items-center gap-1.5">
                                    <code className="font-mono text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                                      {policy.modelId}
                                    </code>
                                    <span className="badge badge-brand tabular-nums">
                                      {serving.healthy}/{serving.total} 账号就绪 ({serving.tierSummary})
                                    </span>
                                    {isDisguised && policy.claudeAliasId ? (
                                      <button
                                        type="button"
                                        onClick={() =>
                                          copyText(
                                            `alias-${policy.modelId}`,
                                            policy.claudeAliasId!
                                          )
                                        }
                                        title="Claude 桌面版拦截非 Anthropic 名称，网关底层自动伪装为此 ID，界面显示名仍保持原模型名"
                                        className="inline-flex items-center gap-1 rounded border border-[#D97757]/25 bg-[#D97757]/8 px-1.5 py-0.5 font-mono text-[10px] text-[#B8502C] dark:text-[#F29C7D]"
                                      >
                                        <span>Claude 伪装: {policy.claudeAliasId}</span>
                                        {copiedKey === `alias-${policy.modelId}` ? (
                                          <Check className="h-2.5 w-2.5 text-emerald-500" />
                                        ) : (
                                          <Copy className="h-2.5 w-2.5 opacity-70" />
                                        )}
                                      </button>
                                    ) : (
                                      <span className="badge border-emerald-500/20 bg-emerald-500/10 text-[10px] text-emerald-700 dark:text-emerald-300">
                                        Claude 原生过白名单
                                      </span>
                                    )}
                                  </div>
                                </div>
                              </div>

                              <div className="flex flex-wrap items-center gap-1.5">
                                <select
                                  value={policy.fallbackModelId ?? ""}
                                  onChange={(e) =>
                                    setPoolModelFallback(
                                      selectedPool.id,
                                      policy.modelId,
                                      e.target.value || null
                                    )
                                  }
                                  aria-label={`${policy.modelId} 降级兜底策略`}
                                  className="rounded-lg border border-zinc-200 bg-zinc-50 px-2 py-1 text-[11px] text-zinc-700 focus:border-brand-500 focus:outline-none dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200"
                                >
                                  <option value="">严格锁模（全号不可用时报错）</option>
                                  {otherEnabledModels.map((opt) => (
                                    <option key={opt.modelId} value={opt.modelId}>
                                      全灭后显式降级 → {opt.modelId}
                                    </option>
                                  ))}
                                </select>

                                <button
                                  type="button"
                                  disabled={!policy.enabled || serving.total === 0}
                                  onClick={() =>
                                    onOpenSimulation(selectedPool.id, policy.modelId)
                                  }
                                  className="btn btn-xs btn-secondary gap-1"
                                >
                                  <Play className="h-3 w-3" aria-hidden="true" />
                                  测此模型
                                </button>
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    </section>

                    {/* 区块 2：池内账号底座（按 T1 / T2 / T3 梯队分组 + 多选模型开关） */}
                    <section className="space-y-3 border-t border-zinc-200/80 pt-4 dark:border-zinc-800">
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <div>
                          <h3 className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                            二、池内账号底座（T1 主力 → T2 备选 → T3 付费兜底 · 多模型多选实时热生效）
                          </h3>
                          <p className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                            直接在账号卡片内点击模型标签即可多选/取消该账号承接的模型，随时调整 T1/T2 梯队
                          </p>
                        </div>
                        <div className="flex flex-wrap items-center gap-1.5">
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
                            {isRefreshingModels ? "探测模型中..." : "刷新真实模型"}
                          </button>
                          <button
                            type="button"
                            onClick={() => setAddMembersOpen(true)}
                            className="btn btn-xs btn-primary gap-1"
                          >
                            <Plus className="h-3 w-3" aria-hidden="true" />
                            添加账号 / 追加模型
                          </button>
                        </div>
                      </div>

                      {TIER_ORDER.map((tierKey) => {
                        const list = membersByTier[tierKey];
                        if (list.length === 0) return null;
                        const tierMeta = MEMBER_TIER_META[tierKey];

                        return (
                          <div key={tierKey} className="space-y-2">
                            <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-zinc-100/75 px-2.5 py-1.5 dark:bg-zinc-800/50">
                              <div className="flex items-center gap-2">
                                <span className={`badge ${tierMeta.badgeClass}`}>
                                  {tierMeta.label}
                                </span>
                                <span className="text-[11px] text-zinc-600 dark:text-zinc-400">
                                  {tierMeta.description}
                                </span>
                              </div>
                              <span className="font-mono text-[11px] tabular-nums text-zinc-400">
                                {list.length} 个账号
                              </span>
                            </div>

                            <div className="space-y-2">
                              {list.map((member) => {
                                const globalIdx = sortedMembers.findIndex(
                                  (m) => m.id === member.id
                                );
                                const modelOptions = getMemberModelOptions(
                                  member,
                                  candidates
                                );
                                const cooldownLeftSec =
                                  typeof member.cooldownUntilMs === "number" &&
                                  member.cooldownUntilMs > nowMs
                                    ? Math.ceil((member.cooldownUntilMs - nowMs) / 1000)
                                    : 0;
                                const skippedForActive =
                                  selectedPool.skipActiveClientAccount &&
                                  Boolean(member.isActiveOnClient);
                                const isRiskPaused = member.accountStatus === "risk_paused";
                                const channelMeta = CHANNEL_META[member.channel];
                                const isApiKey = member.toolId === "custom_api_key";

                                return (
                                  <div
                                    key={member.id}
                                    className={`rounded-xl border p-3 transition ${
                                      cooldownLeftSec > 0
                                        ? "border-amber-500/45 bg-amber-500/[0.04]"
                                        : isRiskPaused
                                          ? "border-rose-500/40 bg-rose-500/[0.04]"
                                          : !member.enabled || skippedForActive
                                            ? "border-zinc-200/70 bg-zinc-50/70 opacity-75 dark:border-zinc-800 dark:bg-zinc-900/40"
                                            : "border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900/70"
                                    }`}
                                  >
                                    <div className="flex flex-wrap items-center justify-between gap-2.5">
                                      <div className="flex min-w-0 flex-1 items-center gap-2.5">
                                        {/* 顺位号与上下移 */}
                                        <div className="flex items-center gap-1">
                                          <span className="w-5 text-center font-mono text-[11px] font-semibold tabular-nums text-zinc-400">
                                            #{globalIdx + 1}
                                          </span>
                                          <div className="flex flex-col">
                                            <button
                                              type="button"
                                              disabled={globalIdx === 0}
                                              onClick={() =>
                                                reorderPoolMember(
                                                  selectedPool.id,
                                                  member.id,
                                                  "up"
                                                )
                                              }
                                              aria-label={`上移 ${member.label}`}
                                              className="rounded p-0.5 text-zinc-400 hover:bg-zinc-100 hover:text-zinc-800 disabled:opacity-25 dark:hover:bg-zinc-800 dark:hover:text-zinc-200"
                                            >
                                              <ArrowUp className="h-3 w-3" aria-hidden="true" />
                                            </button>
                                            <button
                                              type="button"
                                              disabled={globalIdx === sortedMembers.length - 1}
                                              onClick={() =>
                                                reorderPoolMember(
                                                  selectedPool.id,
                                                  member.id,
                                                  "down"
                                                )
                                              }
                                              aria-label={`下移 ${member.label}`}
                                              className="rounded p-0.5 text-zinc-400 hover:bg-zinc-100 hover:text-zinc-800 disabled:opacity-25 dark:hover:bg-zinc-800 dark:hover:text-zinc-200"
                                            >
                                              <ArrowDown className="h-3 w-3" aria-hidden="true" />
                                            </button>
                                          </div>
                                        </div>

                                        <UpstreamMemberIcon toolId={member.toolId} size="sm" />

                                        <div className="min-w-0 flex-1">
                                          <div className="flex flex-wrap items-center gap-1.5">
                                            <span className="truncate text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                                              {member.label}
                                            </span>
                                            <span className={`badge ${channelMeta.badgeClass}`}>
                                              {channelMeta.shortLabel}
                                            </span>

                                            {skippedForActive && (
                                              <span className="badge border-amber-500/30 bg-amber-500/12 text-amber-800 dark:text-amber-300">
                                                桌面活跃 · 已避让
                                              </span>
                                            )}
                                            {!skippedForActive && member.isActiveOnClient && (
                                              <span className="badge border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300">
                                                桌面端挂载中
                                              </span>
                                            )}
                                            {isRiskPaused && (
                                              <span className="badge border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300">
                                                <ShieldAlert className="h-2.5 w-2.5" aria-hidden="true" />
                                                风控暂停
                                              </span>
                                            )}
                                            {cooldownLeftSec > 0 && (
                                              <span className="badge border-amber-500/35 bg-amber-500/15 text-amber-800 tabular-nums dark:text-amber-200">
                                                <Clock className="h-2.5 w-2.5" aria-hidden="true" />
                                                429 冷却 {cooldownLeftSec}s
                                              </span>
                                            )}
                                          </div>

                                          {/* 实时水位条 */}
                                          <div className="mt-1.5 flex items-center gap-2.5">
                                            <div className="meter max-w-[140px]">
                                              <div
                                                className={`h-full rounded-full ${
                                                  member.quotaPercent <= 15
                                                    ? "bg-rose-500"
                                                    : member.quotaPercent <= 40
                                                      ? "bg-amber-500"
                                                      : "meter-fill"
                                                }`}
                                                style={{ width: `${member.quotaPercent}%` }}
                                              />
                                            </div>
                                            <span className="text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                                              {isApiKey
                                                ? member.quotaText
                                                : `${member.quotaPercent}% · ${member.quotaText}`}
                                            </span>
                                          </div>
                                        </div>
                                      </div>

                                      {/* 梯队调整与冷却控制 */}
                                      <div className="flex flex-wrap items-center gap-1.5">
                                        <select
                                          value={member.tier}
                                          disabled={isApiKey}
                                          onChange={(e) =>
                                            updatePoolMemberTier(
                                              selectedPool.id,
                                              member.id,
                                              e.target.value as MemberTier
                                            )
                                          }
                                          title={
                                            isApiKey
                                              ? "按量 API Key 自动锁定 T3 兜底档"
                                              : "调整账号梯队"
                                          }
                                          aria-label={`${member.label} 梯队`}
                                          className="rounded-lg border border-zinc-200 bg-zinc-50 px-2 py-1 text-[11px] text-zinc-800 focus:border-brand-500 focus:outline-none disabled:opacity-70 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200"
                                        >
                                          {TIER_ORDER.map((tk) => (
                                            <option key={tk} value={tk}>
                                              {MEMBER_TIER_META[tk].shortLabel}
                                            </option>
                                          ))}
                                        </select>

                                        {cooldownLeftSec > 0 ? (
                                          <button
                                            type="button"
                                            onClick={() =>
                                              clearMemberCooldown(selectedPool.id, member.id)
                                            }
                                            className="btn btn-xs btn-brand-soft"
                                          >
                                            解除冷却
                                          </button>
                                        ) : (
                                          <button
                                            type="button"
                                            onClick={() =>
                                              injectMemberCooldown(selectedPool.id, member.id)
                                            }
                                            title="将当前账号临时置入冷却期，观察同模型流量自动切换至下一账号"
                                            className="btn btn-xs btn-ghost text-zinc-500 hover:text-amber-700 dark:hover:text-amber-300"
                                          >
                                            <Flame className="h-3 w-3" aria-hidden="true" />
                                            临时冷却
                                          </button>
                                        )}

                                        <button
                                          type="button"
                                          role="switch"
                                          aria-checked={member.enabled}
                                          aria-label={`启用或停用 ${member.label}`}
                                          onClick={() =>
                                            togglePoolMember(selectedPool.id, member.id)
                                          }
                                          className={`relative inline-flex h-4.5 w-8 shrink-0 items-center rounded-full transition ${
                                            member.enabled
                                              ? "bg-brand-600 dark:bg-brand-500"
                                              : "bg-zinc-300 dark:bg-zinc-700"
                                          }`}
                                        >
                                          <span
                                            className={`inline-block h-3 w-3 transform rounded-full bg-white shadow transition ${
                                              member.enabled ? "translate-x-4" : "translate-x-0.5"
                                            }`}
                                          />
                                        </button>

                                        <button
                                          type="button"
                                          onClick={() =>
                                            removePoolMember(selectedPool.id, member.id)
                                          }
                                          aria-label={`移出 ${member.label}`}
                                          className="btn btn-icon btn-ghost text-zinc-400 hover:text-rose-600"
                                        >
                                          <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                                        </button>
                                      </div>
                                    </div>

                                    {/* 行内多模型多选 Pills */}
                                    {modelOptions.length > 0 && (
                                      <div className="mt-2.5 flex flex-wrap items-center gap-1.5 border-t border-zinc-100 pt-2 pl-8 dark:border-zinc-800/70">
                                        <span className="text-[10px] text-zinc-400">
                                          承接模型（可多选）:
                                        </span>
                                        {modelOptions.map((opt) => {
                                          const active = member.supportedModels.includes(opt.id);
                                          const rateLabel = formatModelRateBadge(opt);
                                          return (
                                            <button
                                              key={opt.id}
                                              type="button"
                                              aria-pressed={active}
                                              title={`标准 ID: ${opt.id} · 底层 Wire ID: ${opt.vendorWireId}`}
                                              onClick={() =>
                                                togglePoolMemberModel(
                                                  selectedPool.id,
                                                  member.id,
                                                  opt.id
                                                )
                                              }
                                              className={`inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 font-mono text-[11px] transition ${
                                                active
                                                  ? "border-brand-500/45 bg-brand-500/10 text-brand-800 dark:border-brand-400/45 dark:text-brand-200"
                                                  : "border-zinc-200 bg-zinc-50 text-zinc-400 hover:border-zinc-300 hover:text-zinc-700 dark:border-zinc-800 dark:bg-zinc-800/40 dark:text-zinc-500"
                                              }`}
                                            >
                                              {active && (
                                                <Check className="h-2.5 w-2.5" aria-hidden="true" />
                                              )}
                                              <span>{opt.displayName || opt.id}</span>
                                              {rateLabel && (
                                                <span
                                                  className={`rounded px-1 font-sans text-[9px] ${
                                                    opt.isFree
                                                      ? "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300"
                                                      : "bg-black/5 text-zinc-500 dark:bg-white/10 dark:text-zinc-400"
                                                  }`}
                                                >
                                                  {rateLabel}
                                                </span>
                                              )}
                                            </button>
                                          );
                                        })}
                                      </div>
                                    )}
                                  </div>
                                );
                              })}
                            </div>
                          </div>
                        );
                      })}
                    </section>
                  </>
                )}

                {detailTab === "strategy" && (
                  <div className="space-y-4">
                    {/* 异构混编口径明确告知 */}
                    <div
                      className={`flex items-start gap-2.5 rounded-xl border p-3 text-xs ${
                        selectedIsHeterogeneous
                          ? "border-indigo-500/30 bg-indigo-500/[0.06] text-indigo-950 dark:text-indigo-200"
                          : "border-emerald-500/30 bg-emerald-500/[0.06] text-emerald-950 dark:text-emerald-200"
                      }`}
                    >
                      <Info className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                      <div>
                        <div className="font-semibold">
                          {selectedIsHeterogeneous
                            ? "异构混编口径护栏：跨工具不盲比百分比，同工具内按水位竞争"
                            : "同构单工具池：池内账号全部为同类型额度量纲"}
                        </div>
                        <p className="mt-0.5 text-[11px] opacity-90">
                          {selectedIsHeterogeneous
                            ? "不同 AI 工具的积分 / 快速次数 / Credits 重置周期各不相同。网关严格锁定用户所选模型，先按「T1 主力 → T2 备选 → T3 付费兜底」及成员顺位排序，仅在相同工具的多个账号间比较实时水位百分比。"
                            : "当前池内订阅账号来自同一工具，水位百分比可精确直接对比；标准 API Key 仍自动置于 T3 末位兜底。"}
                        </p>
                      </div>
                    </div>

                    {/* 4 种梯队内调度策略 */}
                    <section className="space-y-2">
                      <h3 className="text-xs font-semibold text-zinc-800 dark:text-zinc-200">
                        梯队内选号调度策略
                      </h3>
                      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                        {(Object.keys(POOL_STRATEGY_META) as PoolStrategy[]).map((key) => {
                          const active = selectedPool.strategy === key;
                          const meta = POOL_STRATEGY_META[key];
                          return (
                            <button
                              key={key}
                              type="button"
                              onClick={() => updatePool(selectedPool.id, { strategy: key })}
                              aria-pressed={active}
                              className={`rounded-xl border p-3 text-left transition ${
                                active
                                  ? "border-brand-500 bg-brand-50/70 ring-1 ring-brand-500/20 dark:border-brand-400/60 dark:bg-brand-500/12"
                                  : "border-zinc-200 bg-zinc-50/60 hover:bg-white dark:border-zinc-800 dark:bg-zinc-900/50 dark:hover:bg-zinc-900"
                              }`}
                            >
                              <div className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                                {meta.label}
                              </div>
                              <p className="mt-0.5 text-[11px] leading-snug text-zinc-500 dark:text-zinc-400">
                                {meta.description}
                              </p>
                            </button>
                          );
                        })}
                      </div>
                    </section>

                    {/* 会话亲和与风控护栏 */}
                    <section className="grid grid-cols-1 gap-4 rounded-xl border border-zinc-200/80 bg-zinc-50/70 p-3.5 dark:border-zinc-800 dark:bg-zinc-950/50 lg:grid-cols-2">
                      <div className="space-y-2">
                        <div className="text-xs font-semibold text-zinc-800 dark:text-zinc-200">
                          会话亲和（Prompt Cache 保护）
                        </div>
                        <div className="flex flex-wrap gap-1.5">
                          {(Object.keys(AFFINITY_MODE_META) as AffinityMode[]).map((mode) => (
                            <button
                              key={mode}
                              type="button"
                              onClick={() =>
                                updatePool(selectedPool.id, { affinityMode: mode })
                              }
                              className={`chip ${
                                selectedPool.affinityMode === mode
                                  ? "chip-active"
                                  : "border border-zinc-200 bg-white dark:border-zinc-700 dark:bg-zinc-900"
                              }`}
                            >
                              {AFFINITY_MODE_META[mode].label}
                            </button>
                          ))}
                        </div>
                        <p className="text-[11px] text-zinc-500 dark:text-zinc-400">
                          {AFFINITY_MODE_META[selectedPool.affinityMode].description}
                        </p>
                      </div>

                      <div className="space-y-2">
                        <div className="text-xs font-semibold text-zinc-800 dark:text-zinc-200">
                          防互踢、刷新与付费兜底护栏
                        </div>
                        <div className="space-y-2 text-xs">
                          <label className="flex cursor-pointer items-center justify-between gap-2">
                            <span className="text-zinc-700 dark:text-zinc-300">
                              跳过桌面端正在挂载的活跃账号
                            </span>
                            <input
                              type="checkbox"
                              checked={selectedPool.skipActiveClientAccount}
                              onChange={(e) =>
                                updatePool(selectedPool.id, {
                                  skipActiveClientAccount: e.target.checked,
                                })
                              }
                              className="rounded border-zinc-300 text-brand-600 focus:ring-brand-500"
                            />
                          </label>

                          <label className="flex cursor-pointer items-center justify-between gap-2">
                            <span className="text-zinc-700 dark:text-zinc-300">
                              401 单飞自动刷新并回写加密库
                            </span>
                            <input
                              type="checkbox"
                              checked={selectedPool.autoRefreshOn401}
                              onChange={(e) =>
                                updatePool(selectedPool.id, {
                                  autoRefreshOn401: e.target.checked,
                                })
                              }
                              className="rounded border-zinc-300 text-brand-600 focus:ring-brand-500"
                            />
                          </label>

                          <label className="flex cursor-pointer items-center justify-between gap-2">
                            <span className="text-zinc-700 dark:text-zinc-300">
                              允许 T3 付费 API Key 在订阅全灭时兜底
                            </span>
                            <input
                              type="checkbox"
                              checked={selectedPool.fallbackApiKeyEnabled}
                              onChange={(e) =>
                                updatePool(selectedPool.id, {
                                  fallbackApiKeyEnabled: e.target.checked,
                                })
                              }
                              className="rounded border-zinc-300 text-brand-600 focus:ring-brand-500"
                            />
                          </label>

                          <div className="flex items-center justify-between gap-2 pt-0.5">
                            <span className="text-zinc-700 dark:text-zinc-300">
                              429 限流冷却退避窗口
                            </span>
                            <select
                              value={selectedPool.cooldownSeconds}
                              onChange={(e) =>
                                updatePool(selectedPool.id, {
                                  cooldownSeconds: Number(e.target.value),
                                })
                              }
                              aria-label="429 限流冷却秒数"
                              className="rounded-lg border border-zinc-200 bg-white px-2 py-1 text-[11px] tabular-nums dark:border-zinc-700 dark:bg-zinc-900"
                            >
                              <option value={30}>30 秒</option>
                              <option value={60}>60 秒</option>
                              <option value={90}>90 秒</option>
                              <option value={120}>120 秒</option>
                            </select>
                          </div>
                        </div>
                      </div>
                    </section>
                  </div>
                )}

                {detailTab === "agents" && (
                  <section className="space-y-3">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div>
                        <h3 className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                          挂载了「{selectedPool.name}」的消费端 Agent
                        </h3>
                        <p className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                          单个 Agent 可同时挂载多个号池，下方展示各 Agent 从本池获得的模型列表与默认状态
                        </p>
                      </div>
                      <button
                        type="button"
                        onClick={() => setBindModalPool(selectedPool)}
                        className="btn btn-xs btn-brand-soft gap-1"
                      >
                        <Link2 className="h-3 w-3" aria-hidden="true" />
                        追加挂载 Agent
                      </button>
                    </div>

                    {(bindingsByPoolId.get(selectedPool.id) ?? []).length === 0 ? (
                      <div className="rounded-xl border border-dashed border-zinc-300/80 p-6 text-center dark:border-zinc-700">
                        <p className="text-xs text-zinc-500 dark:text-zinc-400">
                          暂无 Agent 挂载此号池。点击右上角「挂载 Agent」或前往「Agent 指向」自由勾选多池组合。
                        </p>
                      </div>
                    ) : (
                      <div className="space-y-2.5">
                        {(bindingsByPoolId.get(selectedPool.id) ?? []).map((ab) => {
                          const exposedFromThisPool = computeAgentExposedModels(
                            ab,
                            pools,
                            nowMs
                          ).filter((m) => m.poolId === selectedPool.id);

                          return (
                            <div
                              key={ab.agentId}
                              className="rounded-xl border border-zinc-200 bg-white p-3.5 dark:border-zinc-800 dark:bg-zinc-900/70"
                            >
                              <div className="flex flex-wrap items-center justify-between gap-2">
                                <div className="flex items-center gap-2">
                                  <AgentBrandIcon agentId={ab.agentId} size="sm" />
                                  <div>
                                    <div className="flex items-center gap-1.5">
                                      <button
                                        type="button"
                                        onClick={() => onNavigateAgents(ab.agentId)}
                                        className="text-xs font-semibold text-zinc-900 hover:underline dark:text-zinc-100"
                                      >
                                        {ab.name}
                                      </button>
                                      {ab.status === "bound" ? (
                                        <ShieldCheck
                                          className="h-3.5 w-3.5 text-emerald-500"
                                          aria-hidden="true"
                                        />
                                      ) : (
                                        <ShieldAlert
                                          className="h-3.5 w-3.5 text-amber-500"
                                          aria-hidden="true"
                                        />
                                      )}
                                      <span className="badge badge-neutral">
                                        共挂 {ab.mountedPoolIds.length} 池
                                      </span>
                                    </div>
                                    <div className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                                      {ab.configPath}
                                    </div>
                                  </div>
                                </div>

                                <div className="flex items-center gap-1.5">
                                  <button
                                    type="button"
                                    onClick={() => onNavigateAgents(ab.agentId)}
                                    className="btn btn-xs btn-secondary"
                                  >
                                    前往挂载台
                                  </button>
                                  <button
                                    type="button"
                                    onClick={() =>
                                      openMountDiff(
                                        ab,
                                        ab.mountedPoolIds.filter((id) => id !== selectedPool.id),
                                        selectedPool.id
                                      )
                                    }
                                    className="btn btn-xs btn-danger gap-1"
                                  >
                                    <Unlink className="h-3 w-3" aria-hidden="true" />
                                    卸载本池
                                  </button>
                                </div>
                              </div>

                              <div className="mt-2.5 flex flex-wrap gap-1.5 border-t border-zinc-100 pt-2 dark:border-zinc-800/70">
                                {exposedFromThisPool.map((item) => (
                                  <span
                                    key={item.key}
                                    className={`inline-flex items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-[11px] ${
                                      item.isDefault
                                        ? "border-amber-500/40 bg-amber-500/10 text-amber-800 dark:text-amber-300"
                                        : "border-zinc-200 bg-zinc-50 text-zinc-700 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                                    }`}
                                  >
                                    {item.isDefault && <span>★</span>}
                                    <span>{item.displayName}</span>
                                  </span>
                                ))}
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </section>
                )}
              </div>
            </div>
          )}
        </div>
      )}

      <AddPoolMembersModal
        open={addMembersOpen}
        pool={selectedPool}
        onClose={() => setAddMembersOpen(false)}
      />

      <BindAgentModal
        open={Boolean(bindModalPool)}
        pool={bindModalPool}
        onClose={() => setBindModalPool(null)}
        onPreviewDiff={(binding, nextPoolIds) => {
          const targetPoolId = bindModalPool?.id ?? "";
          setBindModalPool(null);
          openMountDiff(binding, nextPoolIds, targetPoolId);
        }}
      />

      {diffState && (
        <ConfigDiffModal
          open={Boolean(diffState)}
          binding={diffState.binding}
          actionKind={diffState.actionKind}
          targetSummary={{
            mountedPoolNames: diffState.nextPoolIds
              .map((id) => pools.find((p) => p.id === id)?.name)
              .filter((x): x is string => Boolean(x)),
          }}
          beforeText={diffState.beforeText}
          afterText={diffState.afterText}
          onClose={() => setDiffState(null)}
          onConfirm={() => {
            if (diffState.nextPoolIds.includes(diffState.poolId)) {
              mountPoolToAgents(diffState.poolId, [diffState.binding.agentId]);
            } else {
              unmountPoolFromAgent(diffState.binding.agentId, diffState.poolId);
            }
          }}
        />
      )}

      <ConfirmDialog
        open={Boolean(confirmDeletePool)}
        title={`删除号池「${confirmDeletePool?.name ?? ""}」`}
        description="删除后，当前挂载了该号池的 Agent 将自动移除本池并按需更新或无损还原配置（不会删除你的账号资产）。"
        danger
        confirmLabel="确认删除"
        onCancel={() => setConfirmDeletePool(null)}
        onConfirm={() => {
          if (confirmDeletePool) {
            deletePool(confirmDeletePool.id);
          }
          setConfirmDeletePool(null);
        }}
      />
    </div>
  );
}
