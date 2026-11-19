import { useEffect, useMemo, useState } from "react";
import {
  ArrowLeftRight,
  CheckCircle2,
  ExternalLink,
  Loader2,
  Moon,
  Power,
  RefreshCw,
  ShieldCheck,
  Sparkles,
  Sun,
} from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { getToolBrandMeta, getVisibleTools } from "../domain/toolRegistry";
import { getEffectiveClaimStatus, selectScopedStats } from "../domain/selectors";
import { BatchClaimBanner } from "../components/BatchClaimBanner";
import { ColdSwitchModal } from "../components/ColdSwitchModal";
import { AccountStatusBadge, ClaimStatusBadge } from "../components/StatusBadge";
import { QwitchLogo, ToolBrandIcon } from "../components/ToolBrandIcon";
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
    refreshAllQuotas,
    refreshClientStatuses,
    batchProgress,
    theme,
    toggleTheme,
    quitApp,
  } = useQwitch();

  const [toolFilter, setToolFilter] = useState<"all" | ToolId>("all");

  useEffect(() => {
    void refreshClientStatuses();
    const handleFocus = () => {
      void refreshClientStatuses();
    };
    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, [refreshClientStatuses]);

  const visibleTools = useMemo(() => getVisibleTools(adapters), [adapters]);
  const isBusy = Boolean(batchProgress?.running);

  const globalStats = useMemo(
    () => selectScopedStats(accounts, adapters, clientStatuses),
    [accounts, adapters, clientStatuses]
  );

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
          className="inline-flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border border-emerald-500/30 bg-emerald-500/12 px-3 py-2 text-xs font-medium text-emerald-800 dark:text-emerald-300"
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
          className="inline-flex min-w-0 flex-1 shrink-0 whitespace-nowrap items-center justify-center gap-1.5 rounded-xl bg-zinc-900 px-3 py-2 text-xs font-medium text-white shadow-xs transition hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
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
          className="inline-flex min-w-0 flex-1 shrink-0 whitespace-nowrap items-center justify-center gap-1.5 rounded-xl bg-zinc-900 px-3 py-2 text-xs font-medium text-white shadow-xs transition hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
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
        className="inline-flex min-w-0 flex-1 shrink-0 whitespace-nowrap items-center justify-center gap-1.5 rounded-xl border border-zinc-200 bg-zinc-100 px-3 py-2 text-xs font-medium text-zinc-500 dark:border-zinc-800 dark:bg-zinc-800/80 dark:text-zinc-400"
      >
        {globalStats.unclaimedAuthMissingCount > 0
          ? `${globalStats.unclaimedAuthMissingCount} 个账号待主面板授权`
          : "暂无可签到账号"}
      </button>
    );
  };

  return (
    <div className="flex h-full w-full flex-col overflow-hidden rounded-[18px] border border-black/[0.08] bg-zinc-50/95 text-zinc-900 shadow-[0_14px_36px_-6px_rgba(0,0,0,0.22),0_4px_12px_-2px_rgba(0,0,0,0.10)] backdrop-blur-xl dark:border-white/[0.12] dark:bg-zinc-950/95 dark:text-zinc-100">
      {/* 顶部栏 */}
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
              className="rounded-lg border border-zinc-200 bg-white p-1.5 text-zinc-600 transition hover:bg-zinc-100 hover:text-zinc-900 disabled:opacity-45 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700 dark:hover:text-zinc-100"
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
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2 py-1 text-[11px] font-medium text-zinc-700 transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
            >
              主面板
              <ExternalLink className="h-3 w-3 shrink-0" aria-hidden="true" />
            </button>
          </div>
        </div>

        {/* 快捷工具筛选条 */}
        {populatedGroups.length > 1 && (
          <div
            role="tablist"
            aria-label="快捷工具筛选"
            className="mt-2 flex items-center gap-1 overflow-x-auto pb-0.5"
          >
            <button
              type="button"
              role="tab"
              aria-selected={resolvedToolFilter === "all"}
              onClick={() => setToolFilter("all")}
              className={`inline-flex shrink-0 items-center gap-1 rounded-full px-1.5 py-0.5 text-[11px] font-medium transition ${
                resolvedToolFilter === "all"
                  ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                  : "bg-zinc-100 text-zinc-700 hover:bg-zinc-200/80 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700"
              }`}
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
                  className={`inline-flex shrink-0 items-center gap-1 rounded-full px-1.5 py-0.5 text-[11px] font-medium transition ${
                    selected
                      ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                      : "bg-zinc-100 text-zinc-700 hover:bg-zinc-200/80 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700"
                  }`}
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

      {/* 账号列表滚动画布 */}
      <div className="min-h-0 flex-1 space-y-2.5 overflow-y-auto p-3">
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
              const supportsClaim = adapter ? adapter.capabilities.supportsDailyClaim : true;

              return (
                <section key={group.id} className="space-y-1.5">
                  <div className="flex items-center justify-between px-1">
                    <div className="flex min-w-0 items-center gap-1.5">
                      <ToolBrandIcon toolId={group.id} size="xs" decorative />
                      <span className="truncate text-[11px] font-semibold text-zinc-800 dark:text-zinc-200">
                        {group.name}
                      </span>
                      <span className="rounded-full bg-zinc-200/80 px-1.5 py-0.2 text-[11px] font-medium tabular-nums text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
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
                      const pct = isQoderClaimed
                        ? 100
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
                          className={`rounded-xl border bg-white p-2.5 transition dark:bg-zinc-900 ${
                            acc.isActiveOnClient
                              ? "border-emerald-500/45 shadow-xs ring-1 ring-emerald-500/15 dark:border-emerald-400/35"
                              : isDisabled
                                ? "border-zinc-200/80 opacity-75 dark:border-zinc-800"
                                : "border-zinc-200/90 hover:border-zinc-300 dark:border-zinc-800 dark:hover:border-zinc-700"
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
                                <span className="shrink-0 rounded border border-zinc-200 bg-zinc-50 px-1.5 py-0.2 text-[11px] font-medium text-zinc-700 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
                                  {acc.latestQuota.planName}
                                </span>
                              )}
                            </div>

                            <div className="flex shrink-0 items-center gap-1">
                              {acc.status !== "active" ? (
                                <AccountStatusBadge status={acc.status} />
                              ) : (
                                !missingApiToken && (
                                  <ClaimStatusBadge status={effectiveClaim} />
                                )
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
                                  className="mt-1 h-1.5 w-full overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800"
                                >
                                  <div
                                    className={`h-full rounded-full transition-all duration-300 ${
                                      isQoderClaimed
                                        ? "bg-emerald-600 dark:bg-emerald-400"
                                        : pct <= 15
                                          ? "bg-rose-500"
                                          : pct <= 35
                                            ? "bg-amber-500"
                                            : "bg-zinc-900 dark:bg-zinc-100"
                                    }`}
                                    style={{ width: `${pct}%` }}
                                  />
                                </div>
                              )}
                            </div>
                          ) : missingApiToken ? (
                            <div className="mt-1.5 text-[11px] text-amber-800 dark:text-amber-300">
                              仅绑本地快照 · 前往主面板授权后可查额与签到
                            </div>
                          ) : null}

                          {/* 底部操作行 */}
                          <div className="mt-2 flex items-center justify-between gap-2 border-t border-zinc-100 pt-1.5 dark:border-zinc-800/80">
                            <span className="truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                              {acc.uidMasked}
                            </span>

                            <div className="flex shrink-0 items-center gap-1.5">
                              {canInlineClaim && (
                                <button
                                  type="button"
                                  disabled={isBusy}
                                  onClick={() => void claimDailyBatch([acc.id])}
                                  className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-md border border-sky-500/30 bg-sky-500/12 px-2 py-0.5 text-[11px] font-medium text-sky-800 transition hover:bg-sky-500/20 disabled:opacity-45 dark:text-sky-300"
                                >
                                  <Sparkles className="h-2.5 w-2.5 shrink-0" aria-hidden="true" />
                                  领取
                                </button>
                              )}

                              {acc.isActiveOnClient ? (
                                <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-md bg-emerald-500/12 px-2 py-0.5 text-[11px] font-medium text-emerald-800 dark:text-emerald-300">
                                  <CheckCircle2 className="h-2.5 w-2.5 shrink-0" aria-hidden="true" />
                                  使用中
                                </span>
                              ) : missingLocal ? (
                                <span className="shrink-0 whitespace-nowrap rounded-md bg-zinc-100 px-2 py-0.5 text-[11px] text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400">
                                  未关联本机
                                </span>
                              ) : (
                                <button
                                  type="button"
                                  onClick={() => requestColdSwitch(acc)}
                                  className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-md bg-zinc-900 px-2 py-0.5 text-[11px] font-medium text-white transition hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
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
      </div>

      {/* 底部智能操作栏 */}
      <footer className="flex shrink-0 items-center gap-2 border-t border-zinc-200/80 bg-white/90 px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-900/90">
        {renderFooterClaimButton()}

        <button
          type="button"
          onClick={() => void quitApp()}
          title="退出 Qwitch"
          aria-label="退出 Qwitch"
          className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-xl border border-zinc-200 bg-white px-2.5 py-2 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700 dark:hover:text-zinc-100"
        >
          <Power className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          退出
        </button>
      </footer>

      <ColdSwitchModal />
    </div>
  );
}
