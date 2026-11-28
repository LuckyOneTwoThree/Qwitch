import { useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  ArrowRight,
  Bot,
  ChevronDown,
  ChevronUp,
  Layers,
  Play,
  RefreshCw,
  Search,
  ShieldCheck,
  Sparkles,
  X,
  Zap,
} from "lucide-react";
import { SelectMenu } from "../../components/ui/SelectMenu";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import { useOptionalQwitch } from "../../context/QwitchContext";
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

function renderStatusBadge(trace: RoutingTraceItem) {
  if (trace.status === "failover_success") {
    return (
      <span className="badge whitespace-nowrap border-amber-500/30 bg-amber-500/12 text-amber-800 dark:text-amber-300">
        Failover 接管
      </span>
    );
  }
  if (trace.status === "failed") {
    return (
      <span className="badge whitespace-nowrap border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300">
        {trace.attempts.length > 0 ? "请求失败" : "无可用成员"}
      </span>
    );
  }
  return (
    <span className="badge whitespace-nowrap border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300">
      直连命中
    </span>
  );
}

function TraceInspectorContent({
  trace,
}: {
  trace: RoutingTraceItem;
}) {
  return (
    <div className="space-y-3">
      <div className="rounded-xl border border-zinc-200/80 bg-white px-3 py-2.5 text-[11px] dark:border-zinc-800 dark:bg-zinc-900">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap items-center gap-1.5 text-zinc-500 dark:text-zinc-400">
            <span>协议桥接：</span>
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
          {trace.cacheAffinityHit && (
            <span className="badge whitespace-nowrap border-sky-500/25 bg-sky-500/10 text-sky-700 dark:text-sky-300">
              缓存亲和命中
            </span>
          )}
        </div>
        <div className="mt-2 break-all font-mono text-[11px] text-zinc-600 dark:text-zinc-300">
          锁定请求：<span className="text-zinc-900 dark:text-zinc-100">{trace.requestedModel}</span>
          <span className="mx-1.5 text-zinc-400">→</span>
          实发模型：<span className="text-zinc-900 dark:text-zinc-100">{trace.finalModel}</span>
        </div>
      </div>

      <div className="space-y-1.5">
        <div className="text-[11px] font-semibold text-zinc-500 dark:text-zinc-400">
          决策时间线（{trace.attempts.length} 步）
        </div>
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
                              : att.outcome === "skipped_incompatible"
                                ? "border-orange-500/30 bg-orange-500/12 text-orange-800 dark:text-orange-300"
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
                              : att.outcome === "skipped_incompatible"
                                ? "能力不兼容跳过"
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
  );
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
  const [selectedId, setSelectedId] = useState<string | null>(traces[0]?.id ?? null);

  useEffect(() => {
    void refreshWorkspace?.();
  }, [refreshWorkspace]);

  const stats = useMemo(() => {
    const total = traces.length;
    const failovers = traces.filter((t) => t.status === "failover_success").length;
    const failedCount = traces.filter((t) => t.status === "failed").length;
    const successCount = traces.filter((t) => t.status !== "failed").length;
    const directSuccessCount = traces.filter((t) => t.status === "success").length;
    const cacheHits = traces.filter((t) => t.cacheAffinityHit).length;
    const healthyUpstreams = candidates.filter((c) => c.accountStatus === "active").length;

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

  const qwitchCtx = useOptionalQwitch();
  const apiProviders = useMemo(
    () => qwitchCtx?.apiProviders ?? [],
    [qwitchCtx?.apiProviders]
  );
  const apiProviderNameMap = useMemo(
    () => new Map(apiProviders.map((p) => [p.id, p.name])),
    [apiProviders]
  );

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
        label: apiProviderNameMap.get(tid) ?? TOOL_SHORT_LABELS[tid] ?? tid,
        count: traces.filter((t) => traceMatchesTool(t, tid)).length,
      }))
      .filter(
        (item) =>
          item.count > 0 ||
          candidates.some((c) => c.toolId === item.toolId) ||
          toolFilter === item.toolId
      );
  }, [candidates, traces, toolFilter, apiProviderNameMap]);

  const filteredTraces = useMemo(() => {
    const q = search.trim().toLowerCase();
    return traces.filter((t) => {
      if (agentFilter !== "all" && t.agentId !== agentFilter) return false;
      if (toolFilter !== "all" && !traceMatchesTool(t, toolFilter)) return false;
      if (statusFilter !== "all" && t.status !== statusFilter) return false;
      if (!q) return true;
      const toolName = (
        apiProviderNameMap.get(t.finalToolId) ??
        TOOL_SHORT_LABELS[t.finalToolId] ??
        t.finalToolId
      ).toLowerCase();
      return (
        t.poolName.toLowerCase().includes(q) ||
        t.finalAccountLabel.toLowerCase().includes(q) ||
        t.finalModel.toLowerCase().includes(q) ||
        t.requestedModel.toLowerCase().includes(q) ||
        toolName.includes(q)
      );
    });
  }, [traces, agentFilter, toolFilter, statusFilter, search, apiProviderNameMap]);

  useEffect(() => {
    if (filteredTraces.length === 0) {
      if (selectedId !== null && !userToggledRef.current) setSelectedId(null);
      return;
    }
    const stillVisible = filteredTraces.some((t) => t.id === selectedId);
    if (!stillVisible && !userToggledRef.current) {
      setSelectedId(filteredTraces[0].id);
    }
  }, [filteredTraces, selectedId]);

  const selectedTrace = useMemo(
    () => filteredTraces.find((t) => t.id === selectedId) ?? filteredTraces[0] ?? null,
    [filteredTraces, selectedId]
  );

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
    <div className="surface-card flex flex-col overflow-hidden @3xl:h-full @3xl:min-h-0">
      {/* 1. 顶部固定遥测指标栏 */}
      <section className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-zinc-200/80 bg-zinc-50/80 px-3.5 py-2 dark:border-zinc-800 dark:bg-zinc-950/60">
        <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1 text-xs">
          <div className="flex items-center gap-1.5">
            <Activity className="h-3.5 w-3.5 text-brand-600 dark:text-brand-400" aria-hidden="true" />
            <span className="font-semibold text-zinc-900 dark:text-zinc-100">
              采样 {stats.total}
            </span>
            <span className="badge border-emerald-500/25 bg-emerald-500/12 text-emerald-700 dark:text-emerald-300">
              成功率 {stats.successRate}%
            </span>
          </div>

          <div className="flex items-center gap-1 text-zinc-600 dark:text-zinc-400">
            <RefreshCw className="h-3 w-3 text-amber-500" aria-hidden="true" />
            <span>Failover</span>
            <strong className="tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.failovers}
            </strong>
          </div>

          <div className="flex items-center gap-1 text-zinc-600 dark:text-zinc-400">
            <Sparkles className="h-3 w-3 text-sky-500" aria-hidden="true" />
            <span>亲和缓存</span>
            <strong className="tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.cacheHitRate}%
            </strong>
          </div>

          <div className="flex items-center gap-1 text-zinc-600 dark:text-zinc-400">
            <ShieldCheck className="h-3 w-3 text-emerald-500" aria-hidden="true" />
            <span>通道就绪</span>
            <strong className="tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.healthyUpstreams}/{stats.totalUpstreams}
            </strong>
            <span className="hidden text-[11px] text-zinc-400 @lg:inline">
              · {pools.length} 号池
            </span>
          </div>
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
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
      </section>

      {/* 2. 单行紧凑筛选栏 */}
      <section className="flex shrink-0 flex-wrap items-center gap-2 border-b border-zinc-200/80 bg-white px-3.5 py-2 dark:border-zinc-800 dark:bg-zinc-900">
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
            className="field-input h-8 w-full py-0 pr-7 pl-8 text-xs"
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

        <SelectMenu
          ariaLabel="按 Agent 筛选"
          value={agentFilter}
          onChange={(v) => setAgentFilter(v as "all" | TargetAgentId)}
          options={[
            {
              value: "all",
              label: "全部 Agent",
              icon: <Bot className="h-3.5 w-3.5 text-zinc-400" aria-hidden="true" />,
              badge: stats.total,
            },
            ...agentBindings.map((ab) => ({
              value: ab.agentId,
              label: ab.name,
              icon: <AgentBrandIcon agentId={ab.agentId} size="xs" />,
              badge: traces.filter((t) => t.agentId === ab.agentId).length,
            })),
          ]}
        />

        <SelectMenu
          ariaLabel="按订阅工具筛选"
          value={toolFilter}
          onChange={setToolFilter}
          options={[
            {
              value: "all",
              label: "全部订阅工具",
              icon: <Layers className="h-3.5 w-3.5 text-zinc-400" aria-hidden="true" />,
              badge: stats.total,
            },
            ...toolFilterItems.map((item) => ({
              value: item.toolId,
              label: item.label,
              icon: <UpstreamMemberIcon toolId={item.toolId} size="xs" />,
              badge: item.count,
            })),
          ]}
        />

        <div
          role="group"
          aria-label="按路由结果筛选"
          className="inline-flex h-8 flex-wrap items-center gap-0.5 rounded-[var(--radius-control)] bg-zinc-100/80 p-0.5 dark:bg-white/[0.05]"
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
                className={`inline-flex h-full items-center gap-1 rounded-[8px] px-2 text-[11px] font-medium whitespace-nowrap transition ${
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
            className="btn btn-xs btn-secondary h-8 whitespace-nowrap px-2.5"
          >
            重置
          </button>
        )}
      </section>

      {/* 3. 左右主从观测主体：左侧高频流水列表 + 右侧固定决策详情检视器 */}
      <div className="flex min-h-0 flex-1 flex-col @3xl:grid @3xl:grid-cols-[minmax(0,1fr)_360px] @5xl:grid-cols-[minmax(0,1fr)_410px]">
        {/* 左侧流水列表 */}
        <div className="min-h-0 flex-1 divide-y divide-zinc-100 overflow-y-auto dark:divide-zinc-800/80">
          {filteredTraces.map((trace) => {
            const isSelected = selectedTrace?.id === trace.id;
            const isInlineOpen = selectedId === trace.id;
            const agentMeta = agentById.get(trace.agentId);
            const statusDotClass =
              trace.status === "failed"
                ? "bg-rose-500"
                : trace.status === "failover_success"
                  ? "bg-amber-500"
                  : "bg-emerald-500";

            return (
              <div
                key={trace.id}
                className={
                  isSelected
                    ? "border-l-2 border-l-brand-600 bg-brand-500/[0.05] dark:border-l-brand-400 dark:bg-brand-500/[0.08]"
                    : ""
                }
              >
                <button
                  type="button"
                  aria-expanded={isInlineOpen}
                  onClick={() => {
                    userToggledRef.current = true;
                    setSelectedId(isInlineOpen ? null : trace.id);
                  }}
                  className="flex w-full flex-col gap-1 px-3.5 py-2.5 text-left transition hover:bg-zinc-50/80 dark:hover:bg-zinc-800/40"
                >
                  <div className="flex w-full items-center justify-between gap-2">
                    <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-1.5 gap-y-1">
                      <span className={`h-2 w-2 shrink-0 rounded-full ${statusDotClass}`} />
                      <span className="inline-flex shrink-0 items-center gap-1">
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
                      <span className="inline-flex min-w-0 items-center gap-1">
                        <UpstreamMemberIcon toolId={trace.finalToolId} size="xs" />
                        <span className="truncate text-xs font-medium text-zinc-800 dark:text-zinc-200">
                          {trace.finalAccountLabel}
                        </span>
                      </span>
                    </div>

                    <div className="flex shrink-0 items-center gap-1.5">
                      {renderStatusBadge(trace)}
                      <span className="text-zinc-400 @3xl:hidden">
                        {isInlineOpen ? (
                          <ChevronUp className="h-4 w-4" aria-hidden="true" />
                        ) : (
                          <ChevronDown className="h-4 w-4" aria-hidden="true" />
                        )}
                      </span>
                    </div>
                  </div>

                  <div className="flex w-full flex-wrap items-center gap-x-2.5 gap-y-0.5 pl-3.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                    <span className="font-mono tabular-nums">{trace.timestamp}</span>
                    <span className="badge badge-neutral">{trace.poolName}</span>
                    <span className="font-mono tabular-nums">{trace.ttftMs}ms</span>
                    <span className="font-mono tabular-nums">
                      {trace.totalTokens.toLocaleString("zh-CN")} tok
                    </span>
                    <span className="tabular-nums">{trace.attempts.length} 步决策</span>
                  </div>
                </button>

                {/* 窄窗下（< @3xl）保留行内展开详情 */}
                {isInlineOpen && (
                  <div className="border-t border-zinc-200/70 bg-zinc-50/70 px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-950/50 @3xl:hidden">
                    <TraceInspectorContent trace={trace} />
                  </div>
                )}
              </div>
            );
          })}

          {filteredTraces.length === 0 && (
            <div className="flex flex-col items-center justify-center px-4 py-12 text-center">
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
                    className="btn btn-sm btn-primary mt-3 gap-1.5"
                  >
                    <Play className="h-3.5 w-3.5" aria-hidden="true" />
                    运行路由模拟测试
                  </button>
                </>
              )}
            </div>
          )}
        </div>

        {/* 右侧固定决策检视面板（@3xl+ 展示，独立滚动） */}
        <aside
          aria-label="路由决策详情面板"
          className="hidden min-h-0 flex-col border-l border-zinc-200/80 bg-zinc-50/55 dark:border-zinc-800 dark:bg-zinc-950/40 @3xl:flex"
        >
          {selectedTrace ? (
            <>
              <div className="shrink-0 border-b border-zinc-200/80 bg-white/80 px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-900/70">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <AgentBrandIcon agentId={selectedTrace.agentId} size="xs" />
                    <span className="truncate text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                      {agentById.get(selectedTrace.agentId)?.name ?? selectedTrace.agentId}
                    </span>
                    <span className="text-[11px] text-zinc-400">·</span>
                    <span className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                      {selectedTrace.poolName}
                    </span>
                  </div>
                  {renderStatusBadge(selectedTrace)}
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-x-2.5 gap-y-0.5 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                  <span>{selectedTrace.timestamp}</span>
                  <span>TTFT {selectedTrace.ttftMs}ms</span>
                  <span>{selectedTrace.totalTokens.toLocaleString("zh-CN")} tok</span>
                </div>
              </div>
              <div className="min-h-0 flex-1 overflow-y-auto p-3.5">
                <TraceInspectorContent trace={selectedTrace} />
              </div>
            </>
          ) : (
            <div className="flex flex-1 flex-col items-center justify-center p-6 text-center text-xs text-zinc-400">
              <Activity className="mb-2 h-6 w-6 opacity-50" aria-hidden="true" />
              <span>从左侧选择一条路由流水，查看完整决策时间线</span>
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}
