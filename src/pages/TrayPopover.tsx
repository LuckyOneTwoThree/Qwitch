import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ArrowLeftRight,
  CheckCircle2,
  Download,
  ExternalLink,
  Layers,
  Loader2,
  Moon,
  Power,
  RefreshCw,
  ShieldCheck,
  Sparkles,
  Sun,
} from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { usePoolPrototype } from "../context/usePoolPrototypeStore";
import { useUpdateActions } from "../context/useUpdateActions";
import { getToolBrandMeta, getVisibleTools } from "../domain/toolRegistry";
import { getEffectiveClaimStatus, selectScopedStats } from "../domain/selectors";
import { computeAgentExposedModels, computePoolAggregateQuota, makeDefaultModelKey } from "../data/mockPoolData";
import { BatchClaimBanner } from "../components/BatchClaimBanner";
import { ColdSwitchModal } from "../components/ColdSwitchModal";
import { AccountStatusBadge, ClaimStatusBadge } from "../components/StatusBadge";
import { QwitchLogo, ToolBrandIcon } from "../components/ToolBrandIcon";
import { AgentBrandIcon } from "../components/AgentBrandIcon";
import type { ToolId } from "../types/qwitch";

function formatCompactQuota(value: number): string {
  const rounded = Math.round(value * 100) / 100;
  return rounded.toLocaleString("zh-CN", { maximumFractionDigits: 2 });
}

export function TrayPopover({ onOpenMain }: { onOpenMain?: () => void }) {
  const {
    adapters,
    accounts,
    isInitializing,
    clientStatuses,
    claimDailyBatch,
    requestColdSwitch,
    switchDialog,
    cancelColdSwitch,
    refreshAllQuotas,
    refreshClientStatuses,
    batchProgress,
    cstDayKey,
    theme,
    toggleTheme,
    quitApp,
    updateInfo,
    isTauriRuntime,
  } = useQwitch();

  const [trayTab, setTrayTab] = useState<"accounts" | "pools">("accounts");
  const [toolFilter, setToolFilter] = useState<"all" | ToolId>("all");
  const {
    gateway,
    pools,
    agentBindings,
    toggleGateway,
    gatewayBusy,
    updateAgentMountedPools,
    setAgentDefaultModel,
    unbindAgent,
    repairAgentDrift,
    restartAgentClient,
    restartingAgentId,
  } = usePoolPrototype();
  const {
    installing: updateInstalling,
    runPrimaryAction: runUpdatePrimaryAction,
    primaryLabel: updatePrimaryLabel,
  } = useUpdateActions();

  useEffect(() => {
    void refreshClientStatuses();
    const handleFocus = () => {
      void refreshClientStatuses();
    };
    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, [refreshClientStatuses]);

  useEffect(() => {
    const handleBlur = () => {
      if (switchDialog.account && switchDialog.phase === "confirm") {
        cancelColdSwitch();
      }
    };
    window.addEventListener("blur", handleBlur);
    return () => window.removeEventListener("blur", handleBlur);
  }, [cancelColdSwitch, switchDialog.account, switchDialog.phase]);

  const visibleTools = useMemo(() => getVisibleTools(adapters), [adapters]);
  const isBusy = Boolean(batchProgress?.running);

  const globalStats = useMemo(() => {
    void cstDayKey;
    return selectScopedStats(accounts, adapters, clientStatuses);
  }, [accounts, adapters, clientStatuses, cstDayKey]);

  const toolGroups = useMemo(() => {
    const knownToolIds = new Set<string>(visibleTools.map((t) => t.id));
    const extraIds = Array.from(
      new Set(accounts.map((a) => a.toolId).filter((id) => !knownToolIds.has(id)))
    );
    const ordered = [
      ...visibleTools.map((t) => ({ id: t.id, name: t.name })),
      ...extraIds.map((id) => ({ id, name: getToolBrandMeta(id, adapters).name })),
    ];

    return ordered.map((tool) => ({
      ...tool,
      accounts: accounts.filter((a) => a.toolId === tool.id),
      isRunning: Boolean(clientStatuses[tool.id]?.isRunning),
    }));
  }, [accounts, visibleTools, adapters, clientStatuses]);

  const populatedGroups = useMemo(
    () => toolGroups.filter((g) => g.accounts.length > 0),
    [toolGroups]
  );

  const emptyToolNames = useMemo(
    () => toolGroups.filter((g) => g.accounts.length === 0).map((g) => g.name),
    [toolGroups]
  );

  const activeFilterValid =
    toolFilter === "all" || populatedGroups.some((g) => g.id === toolFilter);
  const resolvedToolFilter = activeFilterValid ? toolFilter : "all";

  const displayedGroups = useMemo(
    () =>
      resolvedToolFilter === "all"
        ? populatedGroups
        : populatedGroups.filter((g) => g.id === resolvedToolFilter),
    [populatedGroups, resolvedToolFilter]
  );

  const headerSubtitle = useMemo(() => {
    if (accounts.length === 0) {
      return "Local-First 多账号与额度快捷管理";
    }
    const clientPart = `${globalStats.runningClientsCount} 客户端运行`;
    if (globalStats.eligibleClaimTotal > 0) {
      return `${clientPart} · 今日 ${globalStats.claimedCount}/${globalStats.eligibleClaimTotal} 已签`;
    }
    return `${clientPart} · ${accounts.length} 个纳管账号`;
  }, [accounts.length, globalStats]);

  const renderFooterClaimButton = () => {
    if (isBusy) {
      return (
        <button
          type="button"
          disabled
          className="inline-flex min-w-0 flex-1 shrink-0 whitespace-nowrap items-center justify-center gap-1.5 rounded-xl bg-zinc-900 px-3 py-2 text-xs font-medium text-white opacity-75 dark:bg-zinc-100 dark:text-zinc-900"
        >
          <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden="true" />
          {batchProgress?.action === "refresh" ? "串行查额中..." : "串行领取中..."}
        </button>
      );
    }

    if (globalStats.claimActionState === "all_claimed") {
      return (
        <div
          role="status"
          className="badge min-w-0 flex-1 justify-center gap-1.5 border-emerald-500/30 bg-emerald-500/12 px-3 py-2 text-xs font-medium text-emerald-800 dark:text-emerald-300"
        >
          <CheckCircle2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span className="truncate">
            {globalStats.unclaimedAuthMissingCount > 0
              ? `可签已全领 (${globalStats.claimedCount}/${globalStats.eligibleClaimTotal}) · ${globalStats.unclaimedAuthMissingCount} 待授权`
              : `今日已全部签到 (${globalStats.claimedCount}/${globalStats.eligibleClaimTotal})`}
          </span>
        </div>
      );
    }

    if (globalStats.claimActionState === "claimable") {
      return (
        <button
          type="button"
          onClick={() => void claimDailyBatch()}
          className="btn btn-md btn-primary min-w-0 flex-1 shrink-0 gap-1.5 whitespace-nowrap"
        >
          <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          一键领取全部待领 ({globalStats.claimableCount})
        </button>
      );
    }

    if (globalStats.claimActionState === "verify_and_claim") {
      return (
        <button
          type="button"
          onClick={() => void claimDailyBatch(undefined, undefined, true)}
          className="btn btn-md btn-primary min-w-0 flex-1 shrink-0 gap-1.5 whitespace-nowrap"
        >
          <ShieldCheck className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          核实并一键领取 ({globalStats.unknownClaimCount})
        </button>
      );
    }

    return (
      <button
        type="button"
        disabled
        className="btn btn-md min-w-0 flex-1 shrink-0 gap-1.5 whitespace-nowrap border border-zinc-200 bg-zinc-100 text-zinc-500 dark:border-zinc-800 dark:bg-zinc-800/80 dark:text-zinc-400"
      >
        {globalStats.unclaimedAuthMissingCount > 0
          ? `${globalStats.unclaimedAuthMissingCount} 个账号待主面板授权`
          : "暂无可签到账号"}
      </button>
    );
  };

  return (
    <div className="flex h-full w-full flex-col overflow-hidden rounded-[18px] border border-black/[0.08] bg-zinc-50/95 text-zinc-900 shadow-float backdrop-blur-xl dark:border-white/[0.12] dark:bg-zinc-950/95 dark:text-zinc-100">
      {/* 顶部栏 */}
      {/* 更新条带：仅在确实有新版本时出现。
          设计要点：
          - 动作使用**纯图标按钮**（与 header 的刷新/主题按钮同构），避免在
            384px 宽度下与文字标签争抢空间；
          - 整条可点击，主行为是「获取更新」（Windows 应用内安装 / macOS 打开
            下载页），右侧箭头按钮引导至主面板查看完整更新说明；
          - 不再提供「检查更新」入口：无更新时该入口只会得到「已是最新」，
            属于噪音；手动检查保留在主面板设置页。 */}
      {isTauriRuntime && updateInfo?.updateAvailable && (
        <div className="flex shrink-0 items-center gap-2 border-b border-indigo-500/20 bg-indigo-500/[0.07] px-3.5 py-1.5 dark:border-indigo-400/20 dark:bg-indigo-400/[0.09]">
          <Sparkles
            className="h-3.5 w-3.5 shrink-0 text-indigo-600 dark:text-indigo-300"
            aria-hidden="true"
          />
          <span className="min-w-0 flex-1 truncate text-[11px] leading-snug text-indigo-900 dark:text-indigo-100">
            新版本{" "}
            <span className="font-mono font-semibold">v{updateInfo.latestVersion}</span>
          </span>

          <button
            type="button"
            disabled={updateInstalling}
            onClick={() => void runUpdatePrimaryAction()}
            title={updatePrimaryLabel}
            aria-label={updatePrimaryLabel}
            className="grid h-6 w-6 shrink-0 place-items-center rounded-md bg-indigo-600 text-white transition hover:bg-indigo-700 disabled:opacity-60 dark:bg-indigo-500 dark:hover:bg-indigo-400"
          >
            {updateInstalling ? (
              <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
            ) : (
              <Download className="h-3 w-3" aria-hidden="true" />
            )}
          </button>

          {onOpenMain && (
            <button
              type="button"
              onClick={onOpenMain}
              title="查看更新详情"
              aria-label="查看更新详情"
              className="grid h-6 w-6 shrink-0 place-items-center rounded-md text-indigo-700 transition hover:bg-indigo-500/15 dark:text-indigo-200"
            >
              <ExternalLink className="h-3 w-3" aria-hidden="true" />
            </button>
          )}
        </div>
      )}

      <header className="shrink-0 border-b border-zinc-200/80 bg-white/90 px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-900/90">
        <div className="flex items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-2">
            <QwitchLogo size="sm" />
            <div className="min-w-0">
              <div className="text-xs leading-tight font-semibold tracking-tight text-zinc-900 dark:text-zinc-100">
                Qwitch
              </div>
              <div className="truncate text-[11px] leading-tight text-zinc-600 dark:text-zinc-400">
                {headerSubtitle}
              </div>
            </div>
          </div>

          <div className="flex shrink-0 items-center gap-1">
            <button
              type="button"
              onClick={() => void refreshAllQuotas()}
              disabled={isBusy || accounts.length === 0}
              title="刷新全部额度"
              aria-label="刷新全部额度"
              className="btn btn-icon btn-secondary disabled:opacity-45"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${
                  isBusy && batchProgress?.action === "refresh" ? "animate-spin" : ""
                }`}
                aria-hidden="true"
              />
            </button>

            <button
              type="button"
              onClick={toggleTheme}
              title="切换深色/浅色主题"
              aria-label="切换深色或浅色主题"
              className="rounded-lg border border-zinc-200 bg-white p-1.5 text-zinc-600 transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700 dark:hover:text-zinc-100"
            >
              {theme === "dark" ? (
                <Sun className="h-3.5 w-3.5" aria-hidden="true" />
              ) : (
                <Moon className="h-3.5 w-3.5" aria-hidden="true" />
              )}
            </button>

            <button
              type="button"
              onClick={onOpenMain}
              className="btn btn-xs btn-secondary shrink-0 gap-1 whitespace-nowrap"
            >
              主面板
              <ExternalLink className="h-3 w-3 shrink-0" aria-hidden="true" />
            </button>
          </div>
        </div>

        {/* 托盘双态切换：账号与签到 | 号池与指向 */}
        <div
          role="tablist"
          aria-label="托盘视图模式"
          className="mt-2 grid grid-cols-2 gap-1 rounded-xl bg-zinc-100 p-0.5 dark:bg-zinc-800/80"
        >
          <button
            type="button"
            role="tab"
            aria-selected={trayTab === "accounts"}
            onClick={() => setTrayTab("accounts")}
            className={`rounded-lg px-2 py-1 text-[11px] font-medium transition ${
              trayTab === "accounts"
                ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                : "text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
            }`}
          >
            账号与签到
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={trayTab === "pools"}
            onClick={() => setTrayTab("pools")}
            className={`inline-flex items-center justify-center gap-1.5 rounded-lg px-2 py-1 text-[11px] font-medium transition ${
              trayTab === "pools"
                ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                : "text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
            }`}
          >
            <span
              className={`h-1.5 w-1.5 rounded-full ${
                gateway.running ? "bg-emerald-500" : "bg-zinc-400"
              }`}
            />
            号池与指向
          </button>
        </div>

        {/* 快捷工具筛选条 */}
        {trayTab === "accounts" && populatedGroups.length > 1 && (
          <div
            role="tablist"
            aria-label="快捷工具筛选"
            className="mask-fade-r scrollbar-none mt-2 flex items-center gap-1 overflow-x-auto scroll-smooth pr-6 pb-0.5"
          >
            <button
              type="button"
              role="tab"
              aria-selected={resolvedToolFilter === "all"}
              onClick={() => setToolFilter("all")}
              className={`chip ${resolvedToolFilter === "all" ? "chip-active" : ""}`}
            >
              全部
              <span className="tabular-nums opacity-80">{accounts.length}</span>
            </button>
            {populatedGroups.map((g) => {
              const selected = resolvedToolFilter === g.id;
              return (
                <button
                  key={g.id}
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  onClick={() => setToolFilter(g.id)}
                  className={`chip ${selected ? "chip-active" : ""}`}
                >
                  <ToolBrandIcon toolId={g.id} size="xs" decorative />
                  <span>{g.name}</span>
                  <span className="tabular-nums opacity-80">{g.accounts.length}</span>
                </button>
              );
            })}
          </div>
        )}
      </header>

      {/* 账号 / 号池滚动画布 */}
      <div className="mask-fade-b min-h-0 flex-1 space-y-2.5 overflow-y-auto scroll-smooth p-3 pb-5">
        {trayTab === "pools" ? (
          <div className="space-y-2.5">
            {/* 本地网关控制条 */}
            <div className="surface-card flex items-center justify-between gap-2 px-3 py-2">
              <div className="flex min-w-0 items-center gap-2 text-xs">
                <span
                  className={`h-2 w-2 shrink-0 rounded-full ${
                    gateway.running ? "bg-emerald-500" : "bg-zinc-400"
                  }`}
                />
                <span className="font-semibold text-zinc-900 dark:text-zinc-100">本地网关</span>
                <code className="truncate font-mono text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                  {gateway.host}:{gateway.port}
                </code>
              </div>
              <button
                type="button"
                onClick={() => void toggleGateway()}
                disabled={gatewayBusy}
                aria-busy={gatewayBusy}
                className={`btn btn-xs shrink-0 gap-1 disabled:cursor-not-allowed disabled:opacity-60 ${
                  gateway.running ? "btn-secondary" : "btn-primary"
                }`}
              >
                {gatewayBusy ? (
                  <Loader2 className="h-3 w-3 shrink-0 animate-spin" aria-hidden="true" />
                ) : null}
                {gatewayBusy ? "处理中" : gateway.running ? "停止" : "启动"}
              </button>
            </div>

            {/* 各目标 Agent 快速默认模型热切换 */}
            <section className="space-y-1.5">
              <div className="flex items-center justify-between px-1 text-[11px] font-semibold text-zinc-500 dark:text-zinc-400">
                <span>Agent 指向与 ★ 默认模型热切</span>
                <span>多号池可在主面板配置</span>
              </div>
              {agentBindings.map((b) => {
                const exposed = computeAgentExposedModels(b, pools);
                const selectedVal = b.defaultModelKey ?? "";
                const poolOptions = pools.flatMap((p) =>
                  p.modelPolicies
                    .filter((mp) => mp.enabled)
                    .map((mp) => ({
                      key: makeDefaultModelKey(p.id, mp.modelId),
                      label: `${mp.modelId} · ${p.name}`,
                      mounted: b.mountedPoolIds.includes(p.id),
                    }))
                );

                return (
                  <div key={b.agentId} className="surface-card space-y-1.5 p-2.5">
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex min-w-0 items-center gap-1.5">
                        <AgentBrandIcon agentId={b.agentId} size="xs" />
                        <span className="truncate text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                          {b.name}
                        </span>
                        {b.mountedPoolIds.length > 0 && (
                          <span className="badge badge-neutral">
                            已挂 {b.mountedPoolIds.length} 池 · {exposed.length} 模
                          </span>
                        )}
                        {b.status === "drifted" && (
                          <span className="inline-flex items-center gap-0.5 text-[10px] font-medium text-amber-600 dark:text-amber-300">
                            <AlertTriangle className="h-2.5 w-2.5" aria-hidden="true" />
                            漂移
                          </span>
                        )}
                        {b.status === "needs_restart" && (
                          <span className="text-[10px] font-medium text-sky-600 dark:text-sky-300">
                            待重启
                          </span>
                        )}
                      </div>

                      {b.status === "drifted" ? (
                        <button
                          type="button"
                          onClick={() => repairAgentDrift(b.agentId)}
                          className="btn btn-xs btn-primary shrink-0"
                        >
                          修复
                        </button>
                      ) : b.status === "needs_restart" ? (
                        <button
                          type="button"
                          onClick={() => void restartAgentClient(b.agentId)}
                          disabled={restartingAgentId === b.agentId}
                          aria-busy={restartingAgentId === b.agentId}
                          className="btn btn-xs btn-sky-soft shrink-0 gap-1 disabled:cursor-not-allowed disabled:opacity-60"
                        >
                          {restartingAgentId === b.agentId ? (
                            <Loader2 className="h-2.5 w-2.5 shrink-0 animate-spin" aria-hidden="true" />
                          ) : null}
                          {restartingAgentId === b.agentId ? "处理中…" : "重启"}
                        </button>
                      ) : null}
                    </div>

                    <select
                      value={selectedVal}
                      aria-label={`切换 ${b.name} 默认模型或号池`}
                      onChange={(e) => {
                        const val = e.target.value;
                        if (!val) {
                          unbindAgent(b.agentId);
                          return;
                        }
                        const [pid] = val.split("::");
                        if (b.mountedPoolIds.includes(pid)) {
                          setAgentDefaultModel(b.agentId, val);
                        } else {
                          updateAgentMountedPools(
                            b.agentId,
                            [...b.mountedPoolIds, pid],
                            val
                          );
                        }
                      }}
                      className="w-full rounded-lg border border-zinc-200 bg-zinc-50 px-2 py-1 text-[11px] font-medium text-zinc-800 focus:border-brand-500 focus:outline-none dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-100"
                    >
                      <option value="">-- 未指向（使用原厂配置） --</option>
                      {poolOptions.map((opt) => (
                        <option key={opt.key} value={opt.key}>
                          {opt.mounted ? "★ " : "+ 挂载并设为默认: "}
                          {opt.label}
                        </option>
                      ))}
                    </select>
                  </div>
                );
              })}
            </section>

            {/* 号池水位速览 */}
            <section className="space-y-1.5">
              <div className="px-1 text-[11px] font-semibold text-zinc-500 dark:text-zinc-400">
                号池聚合水位
              </div>
              {pools.map((pool) => {
                const agg = computePoolAggregateQuota(pool.members);
                return (
                  <div key={pool.id} className="surface-card px-2.5 py-2">
                    <div className="flex items-center justify-between gap-2 text-[11px]">
                      <span className="truncate font-semibold text-zinc-900 dark:text-zinc-100">
                        {pool.name}
                      </span>
                      <span className="shrink-0 tabular-nums text-zinc-600 dark:text-zinc-300">
                        均余 {agg}% · {pool.members.length} 成员
                      </span>
                    </div>
                    <div className="meter mt-1.5">
                      <div className="meter-fill" style={{ width: `${agg}%` }} />
                    </div>
                  </div>
                );
              })}
            </section>
          </div>
        ) : (
          <>
            <BatchClaimBanner compact />

        {isInitializing ? (
          <div className="space-y-2.5" aria-label="正在加载账号" role="status">
            {[0, 1, 2].map((i) => (
              <div
                key={i}
                className="h-16 animate-pulse rounded-2xl border border-zinc-200 bg-zinc-100 dark:border-zinc-800 dark:bg-zinc-800/60"
              />
            ))}
          </div>
        ) : accounts.length === 0 ? (
          <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-200 bg-white px-4 py-10 text-center dark:border-zinc-800 dark:bg-zinc-900">
            <QwitchLogo size="md" />
            <div className="mt-3 text-xs font-semibold text-zinc-900 dark:text-zinc-100">
              尚未纳管任何 AI 工具账号
            </div>
            <p className="mt-1 text-[11px] leading-relaxed text-zinc-600 dark:text-zinc-400">
              前往主面板可一键导入本机客户端登录态，或通过隔离授权窗口添加账号。
            </p>
            <button
              type="button"
              onClick={onOpenMain}
              className="mt-3.5 inline-flex items-center gap-1.5 rounded-xl bg-zinc-900 px-3.5 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
            >
              打开主面板导入
              <ExternalLink className="h-3 w-3" aria-hidden="true" />
            </button>
          </div>
        ) : (
          <>
            {displayedGroups.map((group) => {
              const adapter = adapters.find((a) => a.id === group.id);
              const supportsClaim = adapter ? adapter.capabilities.supportsDailyClaim : false;

              return (
                <section key={group.id} className="space-y-1.5">
                  <div className="flex items-center justify-between px-1">
                    <div className="flex min-w-0 items-center gap-1.5">
                      <ToolBrandIcon toolId={group.id} size="xs" decorative />
                      <span className="truncate text-[11px] font-semibold text-zinc-800 dark:text-zinc-200">
                        {group.name}
                      </span>
                      <span className="badge badge-neutral tabular-nums">
                        {group.accounts.length}
                      </span>
                    </div>
                    <span
                      className={`inline-flex shrink-0 items-center gap-1 rounded-full px-1.5 py-0.5 text-[11px] font-medium ${
                        group.isRunning
                          ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                          : "text-zinc-600 dark:text-zinc-400"
                      }`}
                    >
                      {group.isRunning && (
                        <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                      )}
                      {group.isRunning ? "运行中" : "未运行"}
                    </span>
                  </div>

                  <div className="space-y-1.5">
                    {group.accounts.map((acc) => {
                      const effectiveClaim = getEffectiveClaimStatus(acc);
                      const missingApiToken = acc.hasApiToken === false;
                      const missingLocal = acc.hasLocalSnapshot === false;
                      const isDisabled = acc.status === "disabled";
                      const firstBucket = acc.latestQuota?.buckets[0];
                      const isQoderClaimBucket =
                        group.id === "qoder_cn" && firstBucket?.key === "daily_credits";
                      const isQoderClaimed =
                        isQoderClaimBucket && effectiveClaim === "claimed";
                      const isQoderPendingVerify =
                        isQoderClaimBucket &&
                        !isQoderClaimed &&
                        effectiveClaim !== "available";
                      const pct = isQoderClaimed
                        ? 100
                        : isQoderPendingVerify
                          ? 0
                          : firstBucket && firstBucket.total > 0
                            ? Math.min(
                                100,
                                Math.round((firstBucket.remaining / firstBucket.total) * 100)
                              )
                            : null;

                      const canInlineClaim =
                        supportsClaim &&
                        acc.status === "active" &&
                        !missingApiToken &&
                        effectiveClaim === "available";

                      return (
                        <div
                          key={acc.id}
                          className={`surface-card surface-interactive p-2.5 ${
                            acc.isActiveOnClient
                              ? "border-emerald-500/45 ring-1 ring-emerald-500/15 dark:border-emerald-400/35"
                              : isDisabled
                                ? "opacity-75"
                                : ""
                          }`}
                        >
                          <div className="flex items-center justify-between gap-2">
                            <div className="flex min-w-0 items-center gap-1.5">
                              {acc.isActiveOnClient && (
                                <span
                                  title="当前本机客户端挂载账号"
                                  className="h-2 w-2 shrink-0 rounded-full bg-emerald-500"
                                />
                              )}
                              <span className="truncate text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                                {acc.label}
                              </span>
                              {acc.latestQuota?.planName && (
                                <span className="badge badge-neutral shrink-0 border-zinc-200 dark:border-zinc-700">
                                  {acc.latestQuota.planName}
                                </span>
                              )}
                            </div>

                            <div className="flex shrink-0 items-center gap-1">
                              {acc.status !== "active" || missingApiToken || missingLocal ? (
                                <AccountStatusBadge
                                  status={acc.status}
                                  missingApiToken={missingApiToken}
                                  missingLocalSnapshot={missingLocal}
                                />
                              ) : (
                                supportsClaim && <ClaimStatusBadge status={effectiveClaim} />
                              )}
                            </div>
                          </div>

                          {/* 额度水位条 */}
                          {!missingApiToken && firstBucket ? (
                            <div className="mt-2">
                              <div className="flex items-center justify-between gap-2 text-[11px]">
                                <span className="truncate text-zinc-600 dark:text-zinc-400">
                                  {firstBucket.label}
                                  {pct !== null && !isQoderClaimed && !isQoderClaimBucket && (
                                    <span className="ml-1 font-semibold text-zinc-800 tabular-nums dark:text-zinc-200">
                                      剩余 {pct}%
                                    </span>
                                  )}
                                </span>
                                <span className="shrink-0 font-medium tabular-nums text-zinc-800 dark:text-zinc-200">
                                  {isQoderClaimBucket ? (
                                    <>
                                      {formatCompactQuota(firstBucket.remaining)}{" "}
                                      {firstBucket.unit}
                                    </>
                                  ) : (
                                    <>
                                      {formatCompactQuota(firstBucket.remaining)} /{" "}
                                      {formatCompactQuota(firstBucket.total)} {firstBucket.unit}
                                    </>
                                  )}
                                </span>
                              </div>
                              {pct !== null && (
                                <div
                                  role="progressbar"
                                  aria-label={`${acc.label} ${firstBucket.label}`}
                                  aria-valuenow={pct}
                                  aria-valuemin={0}
                                  aria-valuemax={100}
                                  className="meter mt-1"
                                >
                                  <div
                                    className={`h-full rounded-full transition-all duration-300 ${
                                      isQoderClaimed
                                        ? "bg-emerald-500"
                                        : pct <= 15
                                          ? "bg-rose-500"
                                          : pct <= 35
                                            ? "bg-amber-500"
                                            : "meter-fill"
                                    }`}
                                    style={{ width: `${pct}%` }}
                                  />
                                </div>
                              )}
                            </div>
                          ) : missingApiToken ? (
                            <div className="mt-1.5 flex items-center justify-between gap-2 text-[11px] text-amber-800 dark:text-amber-300">
                              <span className="truncate">仅绑本地快照 · 授权后可查额与签到</span>
                              {onOpenMain && (
                                <button
                                  type="button"
                                  onClick={onOpenMain}
                                  className="inline-flex shrink-0 items-center gap-0.5 font-medium underline underline-offset-2 transition hover:text-amber-950 dark:hover:text-amber-200"
                                >
                                  去授权
                                  <ExternalLink className="h-2.5 w-2.5" aria-hidden="true" />
                                </button>
                              )}
                            </div>
                          ) : null}

                          {/* 底部操作行 */}
                          <div className="mt-2 flex items-center justify-between gap-2 border-t border-zinc-200/70 pt-1.5 dark:border-zinc-800/70">
                            <span className="truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                              {acc.uidMasked}
                            </span>

                            <div className="flex shrink-0 items-center gap-1.5">
                              {canInlineClaim && (
                                <button
                                  type="button"
                                  disabled={isBusy}
                                  onClick={() => void claimDailyBatch([acc.id])}
                                  className="btn btn-xs shrink-0 gap-1 whitespace-nowrap border border-sky-500/30 bg-sky-500/12 text-sky-800 transition hover:bg-sky-500/20 disabled:opacity-45 dark:text-sky-300"
                                >
                                  <Sparkles className="h-2.5 w-2.5 shrink-0" aria-hidden="true" />
                                  领取
                                </button>
                              )}

                              {missingLocal ? (
                                <span className="badge badge-neutral shrink-0 whitespace-nowrap">
                                  未关联本机
                                </span>
                              ) : acc.isActiveOnClient ? (
                                <span className="badge shrink-0 whitespace-nowrap border-emerald-500/25 bg-emerald-500/12 text-emerald-800 dark:text-emerald-300">
                                  <CheckCircle2 className="h-2.5 w-2.5 shrink-0" aria-hidden="true" />
                                  使用中
                                </span>
                              ) : (
                                <button
                                  type="button"
                                  disabled={
                                    adapters.find((a) => a.id === acc.toolId)?.capabilities
                                      .supportsClientSwitch !== true
                                  }
                                  title="切换本机客户端账号"
                                  onClick={() => requestColdSwitch(acc)}
                                  className="btn btn-xs btn-brand-soft shrink-0 gap-1 whitespace-nowrap disabled:cursor-not-allowed disabled:opacity-45"
                                >
                                  <ArrowLeftRight className="h-2.5 w-2.5 shrink-0" aria-hidden="true" />
                                  切换
                                </button>
                              )}
                            </div>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </section>
              );
            })}

            {emptyToolNames.length > 0 && resolvedToolFilter === "all" && (
              <button
                type="button"
                onClick={onOpenMain}
                className="flex w-full items-center justify-between gap-2 rounded-xl border border-dashed border-zinc-200 bg-white/80 px-3 py-2 text-left text-[11px] text-zinc-600 transition hover:border-zinc-300 hover:bg-white hover:text-zinc-900 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-400 dark:hover:border-zinc-700 dark:hover:text-zinc-200"
              >
                <span className="truncate">
                  未导入：{emptyToolNames.join(" · ")}
                </span>
                <span className="shrink-0 font-medium text-zinc-800 dark:text-zinc-200">
                  去主面板添加 →
                </span>
              </button>
            )}
          </>
        )}
          </>
        )}
      </div>

      {/* 底部智能操作栏 */}
      <footer className="flex shrink-0 items-center gap-2 border-t border-zinc-200/80 bg-white/90 px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-900/90">
        {trayTab === "pools" ? (
          <button
            type="button"
            onClick={onOpenMain}
            className="btn btn-md btn-primary min-w-0 flex-1 gap-1.5 whitespace-nowrap"
          >
            <Layers className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            打开主面板编排号池
          </button>
        ) : (
          renderFooterClaimButton()
        )}

        <button
          type="button"
          onClick={() => void quitApp()}
          title="退出 Qwitch"
          aria-label="退出 Qwitch"
          className="btn btn-md btn-secondary shrink-0 gap-1 whitespace-nowrap"
        >
          <Power className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          退出
        </button>
      </footer>

      <ColdSwitchModal />
    </div>
  );
}
