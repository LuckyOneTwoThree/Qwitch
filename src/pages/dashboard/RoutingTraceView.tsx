import { useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  ArrowRight,
  ChevronDown,
  ChevronUp,
  Play,
  RefreshCw,
  Search,
  ShieldCheck,
  Sparkles,
  X,
  Zap,
} from "lucide-react";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import {
  MEMBER_TIER_META,
  TOOL_SHORT_LABELS,
  WIRE_PROTOCOL_META,
  splitScopedModelId,
} from "../../data/mockPoolData";
import { BUILTIN_TOOL_ORDER } from "../../domain/toolRegistry";
import type { RoutingTraceItem, TargetAgentId } from "../../types/pool";
import { AgentBrandIcon, UpstreamMemberIcon } from "../../components/AgentBrandIcon";

function traceMatchesTool(trace: RoutingTraceItem, toolId: string): boolean {
  if (trace.finalToolId === toolId) return true;
  if (splitScopedModelId(trace.requestedModel).toolId === toolId) return true;
  return trace.attempts.some((a) => a.toolId === toolId);
}

export function RoutingTraceView({
  onOpenSimulation,
}: {
  onOpenSimulation: (poolId?: string) => void;
}) {
  const { traces, pools, agentBindings, candidates, refreshWorkspace } = usePoolPrototype();
  const [agentFilter, setAgentFilter] = useState<"all" | TargetAgentId>("all");
  const [toolFilter, setToolFilter] = useState<"all" | string>("all");
  const [statusFilter, setStatusFilter] = useState<
    "all" | "failover_success" | "failed" | "success"
  >("all");
  const [search, setSearch] = useState("");
  const userToggledRef = useRef(false);
  const [expandedId, setExpandedId] = useState<string | null>(traces[0]?.id ?? null);

  useEffect(() => {
    void refreshWorkspace?.();
  }, [refreshWorkspace]);

  useEffect(() => {
    if (traces.length > 0 && expandedId === null && !userToggledRef.current) {
      setExpandedId(traces[0].id);
    }
  }, [traces, expandedId]);

  const stats = useMemo(() => {
    const total = traces.length;
    const failovers = traces.filter((t) => t.status === "failover_success").length;
    const failedCount = traces.filter((t) => t.status === "failed").length;
    const successCount = traces.filter((t) => t.status !== "failed").length;
    const directSuccessCount = traces.filter((t) => t.status === "success").length;
    const cacheHits = traces.filter((t) => t.cacheAffinityHit).length;
    const healthyUpstreams = candidates.filter(
      (c) => c.accountStatus === "active"
    ).length;

    return {
      total,
      successRate: total > 0 ? Math.round((successCount / total) * 100) : 100,
      directSuccessCount,
      failovers,
      failedCount,
      cacheHitRate: total > 0 ? Math.round((cacheHits / total) * 100) : 0,
      healthyUpstreams,
      totalUpstreams: candidates.length,
    };
  }, [traces, candidates]);

  const toolFilterItems = useMemo(() => {
    const ordered: string[] = [...BUILTIN_TOOL_ORDER];
    const extras = new Set<string>();
    for (const c of candidates) {
      if (!ordered.includes(c.toolId)) extras.add(c.toolId);
    }
    for (const t of traces) {
      if (t.finalToolId && !ordered.includes(t.finalToolId)) extras.add(t.finalToolId);
      const scopedTid = splitScopedModelId(t.requestedModel).toolId;
      if (scopedTid && !ordered.includes(scopedTid)) extras.add(scopedTid);
    }
    const allIds = [...ordered, ...Array.from(extras)];
    return allIds
      .map((tid) => ({
        toolId: tid,
        label: TOOL_SHORT_LABELS[tid] ?? tid,
        count: traces.filter((t) => traceMatchesTool(t, tid)).length,
      }))
      .filter(
        (item) =>
          item.count > 0 ||
          candidates.some((c) => c.toolId === item.toolId) ||
          toolFilter === item.toolId
      );
  }, [candidates, traces, toolFilter]);

  const filteredTraces = useMemo(() => {
    const q = search.trim().toLowerCase();
    return traces.filter((t) => {
      if (agentFilter !== "all" && t.agentId !== agentFilter) return false;
      if (toolFilter !== "all" && !traceMatchesTool(t, toolFilter)) return false;
      if (statusFilter !== "all" && t.status !== statusFilter) return false;
      if (!q) return true;
      const toolName = (TOOL_SHORT_LABELS[t.finalToolId] ?? t.finalToolId).toLowerCase();
      return (
        t.poolName.toLowerCase().includes(q) ||
        t.finalAccountLabel.toLowerCase().includes(q) ||
        t.finalModel.toLowerCase().includes(q) ||
        t.requestedModel.toLowerCase().includes(q) ||
        toolName.includes(q)
      );
    });
  }, [traces, agentFilter, toolFilter, statusFilter, search]);

  const agentById = useMemo(
    () => new Map(agentBindings.map((b) => [b.agentId, b])),
    [agentBindings]
  );

  const hasActiveFilter =
    Boolean(search.trim()) ||
    agentFilter !== "all" ||
    toolFilter !== "all" ||
    statusFilter !== "all";

  return (
    <div className="space-y-3.5">
      {/* 4 张 KPI 指标卡 */}
      <section className="grid grid-cols-1 gap-2.5 @sm:grid-cols-2 @3xl:grid-cols-4">
        <div className="surface-card flex min-w-0 flex-col justify-between gap-2 px-3.5 py-3">
          <div className="flex items-center justify-between gap-2">
            <span className="flex min-w-0 items-center gap-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-brand-500/12 text-brand-700 dark:text-brand-400">
                <Activity className="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              <span className="truncate">近期路由采样</span>
            </span>
            <span className="badge shrink-0 whitespace-nowrap border-emerald-500/25 bg-emerald-500/12 text-emerald-700 dark:text-emerald-300">
              成功率 {stats.successRate}%
            </span>
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.total}
            </span>
            <span className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
              直连 {stats.directSuccessCount} · 失败 {stats.failedCount}
            </span>
          </div>
        </div>

        <div className="surface-card flex min-w-0 flex-col justify-between gap-2 px-3.5 py-3">
          <div className="flex items-center justify-between gap-2">
            <span className="flex min-w-0 items-center gap-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-amber-500/12 text-amber-700 dark:text-amber-400">
                <RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              <span className="truncate">同模型 Failover</span>
            </span>
            <span className="badge badge-neutral shrink-0 whitespace-nowrap">429 / 401 自愈</span>
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.failovers}
            </span>
            <span className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
              不私换模型接管
            </span>
          </div>
        </div>

        <div className="surface-card flex min-w-0 flex-col justify-between gap-2 px-3.5 py-3">
          <div className="flex items-center justify-between gap-2">
            <span className="flex min-w-0 items-center gap-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-sky-500/12 text-sky-700 dark:text-sky-400">
                <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              <span className="truncate">亲和缓存命中</span>
            </span>
            <span className="badge shrink-0 whitespace-nowrap border-sky-500/25 bg-sky-500/12 text-sky-700 dark:text-sky-300">
              Prompt Cache
            </span>
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.cacheHitRate}%
            </span>
            <span className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
              避免跨号重算前缀
            </span>
          </div>
        </div>

        <div className="surface-card flex min-w-0 flex-col justify-between gap-2 px-3.5 py-3">
          <div className="flex items-center justify-between gap-2">
            <span className="flex min-w-0 items-center gap-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-emerald-500/12 text-emerald-700 dark:text-emerald-400">
                <ShieldCheck className="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              <span className="truncate">上游通道就绪</span>
            </span>
            <span className="badge badge-brand shrink-0 whitespace-nowrap">
              {pools.length} 个号池
            </span>
          </div>
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-2xl font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.healthyUpstreams}
              <span className="ml-1 text-xs font-normal text-zinc-500">
                /{stats.totalUpstreams}
              </span>
            </span>
            <span className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
              可参与号池调度
            </span>
          </div>
        </div>
      </section>

      {/* 双层结构化筛选栏 */}
      <section className="surface-card space-y-2.5 p-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="relative min-w-[160px] flex-1">
            <Search
              className="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400"
              aria-hidden="true"
            />
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="搜索号池、命中账号或模型..."
              aria-label="搜索路由决策记录"
              className="w-full rounded-lg border border-zinc-200 bg-zinc-50/70 py-1.5 pr-7 pl-8 text-xs focus:border-brand-500 focus:bg-white dark:border-zinc-700 dark:bg-zinc-800/70"
            />
            {search && (
              <button
                type="button"
                onClick={() => setSearch("")}
                aria-label="清除搜索"
                className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded p-0.5 text-zinc-400 transition hover:bg-zinc-200/70 hover:text-zinc-700 dark:hover:bg-zinc-700 dark:hover:text-zinc-200"
              >
                <X className="h-3 w-3" aria-hidden="true" />
              </button>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-1.5">
            <div
              role="group"
              aria-label="按路由结果筛选"
              className="inline-flex flex-wrap items-center gap-0.5 rounded-lg border border-zinc-200 bg-zinc-50 p-0.5 dark:border-zinc-700 dark:bg-zinc-800/70"
            >
              {(
                [
                  { id: "all", label: "全部", count: stats.total },
                  { id: "success", label: "成功", count: stats.directSuccessCount },
                  { id: "failover_success", label: "Failover", count: stats.failovers },
                  { id: "failed", label: "失败", count: stats.failedCount },
                ] as const
              ).map((item) => {
                const active = statusFilter === item.id;
                return (
                  <button
                    key={item.id}
                    type="button"
                    onClick={() => setStatusFilter(item.id)}
                    aria-pressed={active}
                    className={`inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium whitespace-nowrap transition ${
                      active
                        ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                        : "text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
                    }`}
                  >
                    <span>{item.label}</span>
                    <span className="text-[11px] tabular-nums opacity-70">{item.count}</span>
                  </button>
                );
              })}
            </div>

            {hasActiveFilter && (
              <button
                type="button"
                onClick={() => {
                  setSearch("");
                  setAgentFilter("all");
                  setToolFilter("all");
                  setStatusFilter("all");
                }}
                className="btn btn-xs btn-secondary whitespace-nowrap"
              >
                重置
              </button>
            )}
            <button
              type="button"
              onClick={() => void refreshWorkspace?.()}
              className="btn btn-xs btn-secondary gap-1 whitespace-nowrap"
              title="立即刷新最新路由记录"
            >
              <RefreshCw className="h-3 w-3" aria-hidden="true" />
              刷新
            </button>
            <button
              type="button"
              onClick={() => onOpenSimulation()}
              className="btn btn-xs btn-brand-soft gap-1 whitespace-nowrap"
            >
              <Play className="h-3 w-3" aria-hidden="true" />
              测试路由
            </button>
          </div>
        </div>

        <div
          role="group"
          aria-label="按 Agent 筛选"
          className="flex flex-wrap items-center gap-1.5 border-t border-zinc-200/70 pt-2 dark:border-zinc-800/80"
        >
          <span className="text-[11px] font-medium text-zinc-500 dark:text-zinc-400">Agent：</span>
          <button
            type="button"
            aria-pressed={agentFilter === "all"}
            onClick={() => setAgentFilter("all")}
            className={`chip ${agentFilter === "all" ? "chip-active" : ""}`}
          >
            全部 Agent
          </button>
          {agentBindings.map((ab) => (
            <button
              key={ab.agentId}
              type="button"
              aria-pressed={agentFilter === ab.agentId}
              onClick={() => setAgentFilter(ab.agentId)}
              className={`chip gap-1.5 ${agentFilter === ab.agentId ? "chip-active" : ""}`}
            >
              <AgentBrandIcon agentId={ab.agentId} size="xs" />
              <span>{ab.name}</span>
            </button>
          ))}
        </div>

        <div
          role="group"
          aria-label="按订阅工具筛选"
          className="flex flex-wrap items-center gap-1.5 border-t border-zinc-200/70 pt-2 dark:border-zinc-800/80"
        >
          <span className="text-[11px] font-medium text-zinc-500 dark:text-zinc-400">
            订阅工具：
          </span>
          <button
            type="button"
            aria-pressed={toolFilter === "all"}
            onClick={() => setToolFilter("all")}
            className={`chip ${toolFilter === "all" ? "chip-active" : ""}`}
          >
            全部工具
          </button>
          {toolFilterItems.map((item) => (
            <button
              key={item.toolId}
              type="button"
              aria-pressed={toolFilter === item.toolId}
              onClick={() => setToolFilter(item.toolId)}
              className={`chip gap-1.5 ${toolFilter === item.toolId ? "chip-active" : ""}`}
            >
              <UpstreamMemberIcon toolId={item.toolId} size="xs" />
              <span>{item.label}</span>
              <span className="text-[10px] tabular-nums opacity-70">{item.count}</span>
            </button>
          ))}
        </div>
      </section>

      {/* 可展开的决策链路流水 */}
      <div className="space-y-2">
        {filteredTraces.map((trace) => {
          const isOpen = expandedId === trace.id;
          const agentMeta = agentById.get(trace.agentId);
          const statusDotClass =
            trace.status === "failed"
              ? "bg-rose-500"
              : trace.status === "failover_success"
                ? "bg-amber-500"
                : "bg-emerald-500";

          return (
            <div key={trace.id} className="surface-card overflow-hidden">
              <button
                type="button"
                aria-expanded={isOpen}
                onClick={() => {
                  userToggledRef.current = true;
                  setExpandedId(isOpen ? null : trace.id);
                }}
                className="flex w-full flex-col gap-1.5 px-4 py-3 text-left transition hover:bg-zinc-50/80 dark:hover:bg-zinc-800/40"
              >
                {/* 第一行：主路由链路 + 结果徽章 */}
                <div className="flex w-full items-start justify-between gap-2.5">
                  <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1">
                    <span className={`h-2 w-2 shrink-0 rounded-full ${statusDotClass}`} />
                    <span className="inline-flex shrink-0 items-center gap-1.5">
                      <AgentBrandIcon agentId={trace.agentId} size="xs" />
                      <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                        {agentMeta?.name ?? trace.agentId}
                      </span>
                    </span>
                    <ArrowRight className="h-3 w-3 shrink-0 text-zinc-400" aria-hidden="true" />
                    <code className="max-w-full truncate rounded border border-brand-500/25 bg-brand-500/8 px-1.5 py-0.5 font-mono text-[11px] font-semibold text-brand-800 dark:text-brand-200">
                      {trace.requestedModel}
                    </code>
                    <ArrowRight className="h-3 w-3 shrink-0 text-zinc-400" aria-hidden="true" />
                    <span className="inline-flex min-w-0 items-center gap-1.5">
                      <UpstreamMemberIcon toolId={trace.finalToolId} size="xs" />
                      <span className="truncate text-xs font-medium text-zinc-800 dark:text-zinc-200">
                        {trace.finalAccountLabel}
                      </span>
                    </span>
                  </div>

                  <div className="flex shrink-0 items-center gap-1.5">
                    {trace.cacheAffinityHit && (
                      <span className="badge whitespace-nowrap border-sky-500/25 bg-sky-500/10 text-sky-700 dark:text-sky-300">
                        缓存亲和
                      </span>
                    )}
                    {trace.status === "failover_success" ? (
                      <span className="badge whitespace-nowrap border-amber-500/30 bg-amber-500/12 text-amber-800 dark:text-amber-300">
                        Failover 接管
                      </span>
                    ) : trace.status === "failed" ? (
                      <span className="badge whitespace-nowrap border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300">
                        {trace.attempts.length > 0 ? "请求失败" : "无可用成员"}
                      </span>
                    ) : (
                      <span className="badge whitespace-nowrap border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300">
                        直连命中
                      </span>
                    )}
                    {isOpen ? (
                      <ChevronUp className="h-4 w-4 shrink-0 text-zinc-400" aria-hidden="true" />
                    ) : (
                      <ChevronDown className="h-4 w-4 shrink-0 text-zinc-400" aria-hidden="true" />
                    )}
                  </div>
                </div>

                {/* 第二行：时间、号池、耗时与步数 */}
                <div className="flex w-full flex-wrap items-center gap-x-2.5 gap-y-1 pl-4 text-[11px] text-zinc-500 dark:text-zinc-400">
                  <span className="font-mono tabular-nums text-zinc-500 dark:text-zinc-400">
                    {trace.timestamp}
                  </span>
                  <span className="badge badge-neutral">{trace.poolName}</span>
                  <span className="font-mono tabular-nums">{trace.ttftMs}ms</span>
                  <span className="font-mono tabular-nums">
                    {trace.totalTokens.toLocaleString("zh-CN")} tok
                  </span>
                  <span className="tabular-nums">{trace.attempts.length} 步决策</span>
                </div>
              </button>

              {isOpen && (
                <div className="border-t border-zinc-200/80 bg-zinc-50/70 px-4 py-3 dark:border-zinc-800 dark:bg-zinc-950/60">
                  <div className="mb-2.5 flex flex-wrap items-center justify-between gap-2 text-[11px] text-zinc-500 dark:text-zinc-400">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span>协议转换：</span>
                      <span
                        className={`badge whitespace-nowrap ${WIRE_PROTOCOL_META[trace.incomingProtocol].badgeClass}`}
                      >
                        {WIRE_PROTOCOL_META[trace.incomingProtocol].label}
                      </span>
                      <ArrowRight className="h-3 w-3 shrink-0" aria-hidden="true" />
                      <span
                        className={`badge whitespace-nowrap ${WIRE_PROTOCOL_META[trace.upstreamProtocol].badgeClass}`}
                      >
                        {WIRE_PROTOCOL_META[trace.upstreamProtocol].label}
                      </span>
                    </div>
                    <span className="min-w-0 break-all font-mono">
                      锁定请求：{trace.requestedModel} → 实发模型：{trace.finalModel}
                    </span>
                  </div>

                  <div className="space-y-1.5">
                    {trace.attempts.map((att, idx) => (
                      <div
                        key={`${att.accountLabel}-${att.model}-${idx}`}
                        className="flex items-start gap-2.5 rounded-xl border border-zinc-200/80 bg-white px-3 py-2 dark:border-zinc-800 dark:bg-zinc-900"
                      >
                        <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-zinc-100 font-mono text-[11px] font-semibold text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                          {idx + 1}
                        </span>
                        <UpstreamMemberIcon toolId={att.toolId} size="xs" className="mt-0.5 shrink-0" />
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center justify-between gap-x-2 gap-y-1 text-xs">
                            <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                              <span className="truncate font-semibold text-zinc-900 dark:text-zinc-100">
                                {att.accountLabel}
                              </span>
                              {att.tier && (
                                <span
                                  className={`badge whitespace-nowrap ${MEMBER_TIER_META[att.tier].badgeClass}`}
                                >
                                  {MEMBER_TIER_META[att.tier].shortLabel}
                                </span>
                              )}
                              <code className="break-all font-mono text-[11px] text-zinc-500">
                                {att.model}
                              </code>
                            </div>

                            <div className="flex shrink-0 items-center gap-1.5">
                              <span
                                className={`badge whitespace-nowrap ${
                                  att.outcome === "ok"
                                    ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                                    : att.outcome === "401_refreshed"
                                      ? "border-sky-500/25 bg-sky-500/10 text-sky-700 dark:text-sky-300"
                                      : att.outcome === "auth_expired"
                                        ? "border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300"
                                        : att.outcome === "http_error"
                                          ? "border-rose-500/25 bg-rose-500/8 text-rose-700 dark:text-rose-300"
                                          : "border-amber-500/30 bg-amber-500/12 text-amber-800 dark:text-amber-300"
                                }`}
                              >
                                {att.outcome === "ok"
                                  ? "命中成功"
                                  : att.outcome === "429_cooldown"
                                    ? "429 冷却切换"
                                    : att.outcome === "401_refreshed"
                                      ? "401 单飞刷新"
                                      : att.outcome === "auth_expired"
                                        ? "凭据失效(需重登)"
                                        : att.outcome === "http_error"
                                          ? "上游错误(非限流)"
                                          : "活跃避让跳过"}
                              </span>
                              <span className="font-mono text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                                {att.latencyMs}ms
                              </span>
                            </div>
                          </div>
                          <p className="mt-0.5 break-words text-[11px] text-zinc-600 dark:text-zinc-400">
                            {att.detail}
                          </p>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          );
        })}

        {filteredTraces.length === 0 && (
          <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-300/80 bg-white/50 px-4 py-12 text-center dark:border-zinc-700/70 dark:bg-zinc-900/30">
            <Zap className="h-6 w-6 text-zinc-400" aria-hidden="true" />
            {hasActiveFilter ? (
              <>
                <p className="mt-2 text-xs font-medium text-zinc-700 dark:text-zinc-300">
                  当前筛选条件下暂无路由流水
                </p>
                <button
                  type="button"
                  onClick={() => {
                    setSearch("");
                    setAgentFilter("all");
                    setToolFilter("all");
                    setStatusFilter("all");
                  }}
                  className="btn btn-sm btn-secondary mt-3"
                >
                  清除筛选条件
                </button>
              </>
            ) : (
              <>
                <p className="mt-2 text-xs font-medium text-zinc-700 dark:text-zinc-300">
                  暂无路由决策采样
                </p>
                <p className="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                  当接入的 Agent 通过本地网关发起请求，或运行模拟测试时，将在此展示完整调度链路。
                </p>
                <button
                  type="button"
                  onClick={() => onOpenSimulation()}
                  className="btn btn-sm btn-primary mt-3 gap-1"
                >
                  <Play className="h-3 w-3" aria-hidden="true" />
                  立即测试路由
                </button>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
