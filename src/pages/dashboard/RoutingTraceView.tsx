import { useMemo, useState } from "react";
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
  Zap,
} from "lucide-react";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import { MEMBER_TIER_META, WIRE_PROTOCOL_META } from "../../data/mockPoolData";
import type { TargetAgentId } from "../../types/pool";
import { AgentBrandIcon, UpstreamMemberIcon } from "../../components/AgentBrandIcon";

export function RoutingTraceView({
  onOpenSimulation,
}: {
  onOpenSimulation: (poolId?: string) => void;
}) {
  const { traces, pools, agentBindings, candidates } = usePoolPrototype();
  const [agentFilter, setAgentFilter] = useState<"all" | TargetAgentId>("all");
  const [statusFilter, setStatusFilter] = useState<"all" | "failover_success" | "success">("all");
  const [search, setSearch] = useState("");
  const [expandedId, setExpandedId] = useState<string | null>(traces[0]?.id ?? null);

  const stats = useMemo(() => {
    const total = traces.length;
    const failovers = traces.filter((t) => t.status === "failover_success").length;
    const cacheHits = traces.filter((t) => t.cacheAffinityHit).length;
    const successCount = traces.filter((t) => t.status !== "failed").length;
    const healthyUpstreams = candidates.filter(
      (c) => c.accountStatus === "active" && c.codeArtsMode !== "client_readonly"
    ).length;

    return {
      total,
      successRate: total > 0 ? Math.round((successCount / total) * 100) : 100,
      failovers,
      cacheHitRate: total > 0 ? Math.round((cacheHits / total) * 100) : 0,
      healthyUpstreams,
      totalUpstreams: candidates.length,
    };
  }, [traces, candidates]);

  const filteredTraces = useMemo(() => {
    const q = search.trim().toLowerCase();
    return traces.filter((t) => {
      if (agentFilter !== "all" && t.agentId !== agentFilter) return false;
      if (statusFilter !== "all" && t.status !== statusFilter) return false;
      if (!q) return true;
      return (
        t.poolName.toLowerCase().includes(q) ||
        t.finalAccountLabel.toLowerCase().includes(q) ||
        t.finalModel.toLowerCase().includes(q) ||
        t.requestedModel.toLowerCase().includes(q)
      );
    });
  }, [traces, agentFilter, statusFilter, search]);

  return (
    <div className="space-y-4">
      {/* 4 张 KPI 指标卡 */}
      <section className="grid grid-cols-1 gap-2.5 @sm:grid-cols-2 @3xl:grid-cols-4">
        <div className="surface-card flex flex-col justify-between gap-2 px-3.5 py-3">
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              <span className="flex h-7 w-7 items-center justify-center rounded-lg bg-brand-500/12 text-brand-700 dark:text-brand-400">
                <Activity className="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              近期路由采样
            </span>
            <span className="badge border-emerald-500/25 bg-emerald-500/12 text-emerald-700 dark:text-emerald-300">
              成功率 {stats.successRate}%
            </span>
          </div>
          <div className="flex items-baseline justify-between">
            <span className="text-2xl font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.total}
            </span>
            <span className="text-[11px] text-zinc-500 dark:text-zinc-400">
              覆盖 {pools.length} 个号池
            </span>
          </div>
        </div>

        <div className="surface-card flex flex-col justify-between gap-2 px-3.5 py-3">
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              <span className="flex h-7 w-7 items-center justify-center rounded-lg bg-amber-500/12 text-amber-700 dark:text-amber-400">
                <RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              同模型 Failover
            </span>
            <span className="badge badge-neutral">429 / 401 自愈</span>
          </div>
          <div className="flex items-baseline justify-between">
            <span className="text-2xl font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.failovers}
            </span>
            <span className="text-[11px] text-zinc-500 dark:text-zinc-400">
              不私换模型零中断接管
            </span>
          </div>
        </div>

        <div className="surface-card flex flex-col justify-between gap-2 px-3.5 py-3">
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              <span className="flex h-7 w-7 items-center justify-center rounded-lg bg-sky-500/12 text-sky-700 dark:text-sky-400">
                <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              亲和缓存命中
            </span>
            <span className="badge border-sky-500/25 bg-sky-500/12 text-sky-700 dark:text-sky-300">
              Prompt Cache
            </span>
          </div>
          <div className="flex items-baseline justify-between">
            <span className="text-2xl font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.cacheHitRate}%
            </span>
            <span className="text-[11px] text-zinc-500 dark:text-zinc-400">
              避免跨号重算前缀
            </span>
          </div>
        </div>

        <div className="surface-card flex flex-col justify-between gap-2 px-3.5 py-3">
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-2 text-xs font-medium text-zinc-600 dark:text-zinc-400">
              <span className="flex h-7 w-7 items-center justify-center rounded-lg bg-emerald-500/12 text-emerald-700 dark:text-emerald-400">
                <ShieldCheck className="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              上游通道就绪
            </span>
            <span className="badge badge-brand">7 工具 + API Key</span>
          </div>
          <div className="flex items-baseline justify-between">
            <span className="text-2xl font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
              {stats.healthyUpstreams}
              <span className="ml-1 text-xs font-normal text-zinc-500">
                /{stats.totalUpstreams}
              </span>
            </span>
            <span className="text-[11px] text-zinc-500 dark:text-zinc-400">可参与号池调度</span>
          </div>
        </div>
      </section>

      {/* 筛选栏 */}
      <section className="surface-card flex flex-wrap items-center justify-between gap-2.5 px-3.5 py-2.5">
        <div className="flex flex-wrap items-center gap-2">
          <div className="relative min-w-[180px]">
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
              className="w-full rounded-lg border border-zinc-200 bg-zinc-50/70 py-1.5 pr-3 pl-8 text-xs focus:border-brand-500 focus:bg-white focus:outline-none dark:border-zinc-700 dark:bg-zinc-800/70"
            />
          </div>

          <div className="flex flex-wrap items-center gap-1">
            <button
              type="button"
              onClick={() => setAgentFilter("all")}
              className={`chip ${agentFilter === "all" ? "chip-active" : ""}`}
            >
              全部 Agent
            </button>
            {agentBindings.map((ab) => (
              <button
                key={ab.agentId}
                type="button"
                onClick={() => setAgentFilter(ab.agentId)}
                className={`chip ${agentFilter === ab.agentId ? "chip-active" : ""}`}
              >
                {ab.name}
              </button>
            ))}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            onClick={() =>
              setStatusFilter((prev) =>
                prev === "failover_success" ? "all" : "failover_success"
              )
            }
            className={`chip ${
              statusFilter === "failover_success"
                ? "chip-active"
                : "border border-zinc-200 dark:border-zinc-700"
            }`}
          >
            仅看 Failover 切号
          </button>

          <button
            type="button"
            onClick={() => onOpenSimulation()}
            className="btn btn-xs btn-primary gap-1"
          >
            <Play className="h-3 w-3" aria-hidden="true" />
            测试路由
          </button>
        </div>
      </section>

      {/* 可展开的决策链路流水 */}
      <div className="space-y-2">
        {filteredTraces.map((trace) => {
          const isOpen = expandedId === trace.id;
          const agentMeta = agentBindings.find((b) => b.agentId === trace.agentId);

          return (
            <div key={trace.id} className="surface-card overflow-hidden">
              <button
                type="button"
                onClick={() => setExpandedId(isOpen ? null : trace.id)}
                className="flex w-full flex-wrap items-center justify-between gap-3 px-4 py-3 text-left transition hover:bg-zinc-50/80 dark:hover:bg-zinc-800/40"
              >
                <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                  <span className="font-mono text-[11px] tabular-nums text-zinc-400">
                    {trace.timestamp}
                  </span>

                  <div className="flex items-center gap-1.5">
                    <AgentBrandIcon agentId={trace.agentId} size="xs" />
                    <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                      {agentMeta?.name ?? trace.agentId}
                    </span>
                  </div>

                  <ArrowRight className="h-3 w-3 text-zinc-400" aria-hidden="true" />

                  <code className="rounded border border-brand-500/25 bg-brand-500/8 px-1.5 py-0.5 font-mono text-[11px] font-semibold text-brand-800 dark:text-brand-200">
                    {trace.requestedModel}
                  </code>

                  <span className="badge badge-neutral">{trace.poolName}</span>

                  <ArrowRight className="h-3 w-3 text-zinc-400" aria-hidden="true" />

                  <div className="flex items-center gap-1.5">
                    <UpstreamMemberIcon toolId={trace.finalToolId} size="xs" />
                    <span className="text-xs font-medium text-zinc-800 dark:text-zinc-200">
                      {trace.finalAccountLabel}
                    </span>
                  </div>
                </div>

                <div className="flex shrink-0 flex-wrap items-center gap-2">
                  {trace.cacheAffinityHit && (
                    <span className="badge border-sky-500/25 bg-sky-500/10 text-sky-700 dark:text-sky-300">
                      缓存亲和
                    </span>
                  )}
                  {trace.status === "failover_success" && (
                    <span className="badge border-amber-500/30 bg-amber-500/12 text-amber-800 dark:text-amber-300">
                      Failover 接管
                    </span>
                  )}
                  {trace.status === "failed" && (
                    <span className="badge border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300">
                      无可用成员
                    </span>
                  )}
                  <span className="font-mono text-[11px] tabular-nums text-zinc-500">
                    {trace.ttftMs}ms · {trace.totalTokens.toLocaleString("zh-CN")} tok
                  </span>
                  {isOpen ? (
                    <ChevronUp className="h-4 w-4 text-zinc-400" aria-hidden="true" />
                  ) : (
                    <ChevronDown className="h-4 w-4 text-zinc-400" aria-hidden="true" />
                  )}
                </div>
              </button>

              {isOpen && (
                <div className="border-t border-zinc-200/80 bg-zinc-50/70 px-4 py-3 dark:border-zinc-800 dark:bg-zinc-950/60">
                  <div className="mb-2.5 flex flex-wrap items-center justify-between gap-2 text-[11px] text-zinc-500 dark:text-zinc-400">
                    <div className="flex flex-wrap items-center gap-2">
                      <span>协议转换：</span>
                      <span className={`badge ${WIRE_PROTOCOL_META[trace.incomingProtocol].badgeClass}`}>
                        {WIRE_PROTOCOL_META[trace.incomingProtocol].label}
                      </span>
                      <ArrowRight className="h-3 w-3" aria-hidden="true" />
                      <span className={`badge ${WIRE_PROTOCOL_META[trace.upstreamProtocol].badgeClass}`}>
                        {WIRE_PROTOCOL_META[trace.upstreamProtocol].label}
                      </span>
                    </div>
                    <span className="font-mono">
                      锁定请求：{trace.requestedModel} → 实发模型：{trace.finalModel}
                    </span>
                  </div>

                  <div className="space-y-1.5">
                    {trace.attempts.map((att, idx) => (
                      <div
                        key={idx}
                        className="flex items-start gap-2.5 rounded-xl border border-zinc-200/80 bg-white px-3 py-2 dark:border-zinc-800 dark:bg-zinc-900"
                      >
                        <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-zinc-100 font-mono text-[10px] font-semibold text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                          {idx + 1}
                        </span>
                        <UpstreamMemberIcon toolId={att.toolId} size="xs" className="mt-0.5" />
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-1.5 text-xs">
                            <span className="font-semibold text-zinc-900 dark:text-zinc-100">
                              {att.accountLabel}
                            </span>
                            {att.tier && (
                              <span className={`badge ${MEMBER_TIER_META[att.tier].badgeClass}`}>
                                {MEMBER_TIER_META[att.tier].shortLabel}
                              </span>
                            )}
                            <code className="font-mono text-[11px] text-zinc-500">
                              {att.model}
                            </code>
                            <span
                              className={`badge ${
                                att.outcome === "ok"
                                  ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                                  : att.outcome === "401_refreshed"
                                    ? "border-sky-500/25 bg-sky-500/10 text-sky-700 dark:text-sky-300"
                                    : "border-amber-500/30 bg-amber-500/12 text-amber-800 dark:text-amber-300"
                              }`}
                            >
                              {att.outcome === "ok"
                                ? "命中成功"
                                : att.outcome === "429_cooldown"
                                  ? "429 冷却切换"
                                  : att.outcome === "401_refreshed"
                                    ? "401 单飞刷新"
                                    : "活跃避让跳过"}
                            </span>
                            <span className="font-mono text-[11px] tabular-nums text-zinc-400">
                              {att.latencyMs}ms
                            </span>
                          </div>
                          <p className="mt-0.5 text-[11px] text-zinc-600 dark:text-zinc-400">
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
          <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-300/80 bg-white/50 py-12 text-center dark:border-zinc-700/70 dark:bg-zinc-900/30">
            <Zap className="h-6 w-6 text-zinc-400" aria-hidden="true" />
            <p className="mt-2 text-xs font-medium text-zinc-700 dark:text-zinc-300">
              当前筛选条件下暂无路由流水
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
