import { useMemo } from "react";
import { ArrowLeftRight, Sparkles, ExternalLink, CheckCircle2, RefreshCw } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { getVisibleTools } from "../domain/toolRegistry";
import { BatchClaimBanner } from "../components/BatchClaimBanner";
import { ColdSwitchModal } from "../components/ColdSwitchModal";
import { QwitchLogo, ToolBrandIcon } from "../components/ToolBrandIcon";

export function TrayPopover({ onOpenMain }: { onOpenMain?: () => void }) {
  const {
    adapters,
    accounts,
    claimDailyBatch,
    requestColdSwitch,
    refreshAllQuotas,
    batchProgress,
  } = useQwitch();

  const visibleTools = useMemo(() => getVisibleTools(adapters), [adapters]);
  const isBusy = Boolean(batchProgress?.running);

  return (
    <div className="flex h-full w-full flex-col justify-between bg-white p-3 text-zinc-900 dark:bg-zinc-900 dark:text-zinc-100">
      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-0.5">
        <div className="flex items-center justify-between border-b border-zinc-100 pb-2 dark:border-zinc-800">
          <div className="flex items-center gap-2">
            <QwitchLogo size="sm" />
            <span className="text-xs font-semibold tracking-tight">Qwitch 快捷面板</span>
          </div>
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              onClick={() => void refreshAllQuotas()}
              disabled={isBusy}
              title="刷新全部额度"
              aria-label="刷新全部额度"
              className="rounded-md border border-zinc-200 p-1 text-zinc-600 transition hover:bg-zinc-100 hover:text-zinc-900 disabled:opacity-50 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
            >
              <RefreshCw
                className={`h-3 w-3 ${
                  isBusy && batchProgress?.action === "refresh" ? "animate-spin" : ""
                }`}
                aria-hidden="true"
              />
            </button>
            <button
              type="button"
              onClick={onOpenMain}
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-md px-1.5 py-1 text-[11px] font-medium text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
            >
              主面板
              <ExternalLink className="h-3 w-3 shrink-0" aria-hidden="true" />
            </button>
          </div>
        </div>

        <BatchClaimBanner compact />

        {visibleTools.map((group) => {
          const toolAccounts = accounts.filter(
            (a) => a.toolId === group.id && a.status === "active"
          );
          const activeAcc = toolAccounts.find((a) => a.isActiveOnClient) ?? toolAccounts[0];
          const firstBucket = activeAcc?.latestQuota?.buckets[0];
          const isQoderClaimed =
            group.id === "qoder_cn" &&
            firstBucket?.key === "daily_credits" &&
            activeAcc?.latestQuota?.claimStatus === "claimed";
          const pct = isQoderClaimed
            ? 100
            : firstBucket && firstBucket.total > 0
              ? Math.min(100, Math.round((firstBucket.remaining / firstBucket.total) * 100))
              : null;

          return (
            <div
              key={group.id}
              className="rounded-xl border border-zinc-200/80 bg-zinc-50/70 p-2.5 dark:border-zinc-800 dark:bg-zinc-800/40"
            >
              <div className="flex items-center justify-between gap-2">
                <div className="flex items-center gap-1.5">
                  <ToolBrandIcon toolId={group.id} size="xs" decorative />
                  <span className="text-[11px] font-semibold text-zinc-800 dark:text-zinc-200">
                    {group.name}
                  </span>
                </div>
                {firstBucket && (
                  <span className="shrink-0 whitespace-nowrap text-[11px] tabular-nums text-zinc-600 dark:text-zinc-400">
                    {isQoderClaimed
                      ? `已领 ${Math.round(firstBucket.remaining * 100) / 100} ${firstBucket.unit}`
                      : `余 ${Math.round(firstBucket.remaining * 100) / 100} ${firstBucket.unit}`}
                  </span>
                )}
              </div>

              {pct !== null && (
                <div
                  role="progressbar"
                  aria-label={`${group.name} 剩余额度百分比`}
                  aria-valuenow={pct}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  className="mt-1.5 h-1 w-full overflow-hidden rounded-full bg-zinc-200/80 dark:bg-zinc-700/70"
                >
                  <div
                    className={`h-full rounded-full ${
                      isQoderClaimed
                        ? "bg-emerald-600 dark:bg-emerald-400"
                        : pct <= 15
                          ? "bg-rose-500"
                          : pct <= 35
                            ? "bg-amber-500"
                            : "bg-zinc-900 dark:bg-zinc-200"
                    }`}
                    style={{ width: `${pct}%` }}
                  />
                </div>
              )}

              {toolAccounts.length === 0 ? (
                <div className="mt-2 rounded-lg border border-dashed border-zinc-200/80 px-2 py-1.5 text-center text-[11px] text-zinc-500 dark:border-zinc-700/80 dark:text-zinc-400">
                  暂未导入账号，请在主面板导入
                </div>
              ) : (
                <div className="mt-2 space-y-1">
                  {toolAccounts.map((acc) => (
                    <button
                      key={acc.id}
                      type="button"
                      disabled={acc.isActiveOnClient || acc.hasLocalSnapshot === false}
                      onClick={() => requestColdSwitch(acc)}
                      className={`flex w-full items-center justify-between gap-2 rounded-lg px-2 py-1.5 text-left text-xs transition ${
                        acc.isActiveOnClient
                          ? "bg-white font-medium text-zinc-900 shadow-xs dark:bg-zinc-800 dark:text-zinc-100"
                          : acc.hasLocalSnapshot === false
                            ? "cursor-default text-zinc-500 dark:text-zinc-400"
                            : "text-zinc-700 hover:bg-white/80 dark:text-zinc-300 dark:hover:bg-zinc-800/70"
                      }`}
                    >
                      <span className="truncate">{acc.label}</span>
                      {acc.isActiveOnClient ? (
                        <CheckCircle2
                          className="h-3.5 w-3.5 shrink-0 text-emerald-600 dark:text-emerald-400"
                          aria-label="当前使用中"
                        />
                      ) : acc.hasLocalSnapshot === false ? (
                        <span className="shrink-0 text-[11px] text-zinc-500 dark:text-zinc-400">
                          未关联本机
                        </span>
                      ) : (
                        <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                          <ArrowLeftRight className="h-2.5 w-2.5 shrink-0" aria-hidden="true" />
                          切换
                        </span>
                      )}
                    </button>
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </div>

      <div className="mt-2.5 shrink-0 border-t border-zinc-100 pt-2 dark:border-zinc-800">
        <button
          type="button"
          onClick={() => void claimDailyBatch()}
          disabled={isBusy}
          className="inline-flex w-full shrink-0 whitespace-nowrap items-center justify-center gap-1.5 rounded-xl bg-zinc-900 px-3 py-2 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:opacity-50 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
        >
          <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          {isBusy
            ? batchProgress?.action === "refresh"
              ? "串行查额中..."
              : "串行领取中..."
            : "手动一键领取全部待领"}
        </button>
      </div>

      <ColdSwitchModal />
    </div>
  );
}
