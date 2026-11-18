import { useState } from "react";
import { ArrowLeftRight, Sparkles, ExternalLink, CheckCircle2, RefreshCw } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { ColdSwitchModal } from "../components/ColdSwitchModal";
import { QwitchLogo, ToolBrandIcon } from "../components/ToolBrandIcon";

const TOOL_GROUPS = [
  { id: "trae_cn", name: "Trae CN" },
  { id: "workbuddy", name: "WorkBuddy" },
  { id: "qoder_cn", name: "Qoder CN" },
] as const;

export function TrayPopover({ onOpenMain }: { onOpenMain?: () => void }) {
  const { accounts, claimDailyBatch, requestColdSwitch, refreshAllQuotas, batchProgress } = useQwitch();
  const [refreshing, setRefreshing] = useState(false);

  const handleRefreshAll = async () => {
    setRefreshing(true);
    await refreshAllQuotas();
    setRefreshing(false);
  };

  return (
    <div className="flex h-full w-full flex-col justify-between bg-white p-3.5 text-zinc-900 dark:bg-zinc-900 dark:text-zinc-100">
      <div className="space-y-2.5 overflow-y-auto pr-0.5">
        <div className="flex items-center justify-between border-b border-zinc-100 pb-2.5 dark:border-zinc-800">
          <div className="flex items-center gap-2">
            <QwitchLogo size="sm" />
            <span className="text-xs font-semibold tracking-tight">Qwitch 快捷面板</span>
          </div>
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              onClick={() => void handleRefreshAll()}
              disabled={refreshing}
              title="刷新全部额度"
              className="rounded-md border border-zinc-200 p-1 text-zinc-500 transition hover:bg-zinc-100 hover:text-zinc-900 disabled:opacity-50 dark:border-zinc-700 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
            >
              <RefreshCw className={`h-3 w-3 ${refreshing ? "animate-spin" : ""}`} />
            </button>
            <button
              type="button"
              onClick={onOpenMain}
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-md px-1.5 py-1 text-[11px] font-medium text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
            >
              主面板
              <ExternalLink className="h-3 w-3 shrink-0" />
            </button>
          </div>
        </div>

        {TOOL_GROUPS.map((group) => {
          const toolAccounts = accounts.filter((a) => a.toolId === group.id && a.status === "active");
          const activeAcc = toolAccounts.find((a) => a.isActiveOnClient) ?? toolAccounts[0];
          const firstBucket = activeAcc?.latestQuota?.buckets[0];
          const pct =
            firstBucket && firstBucket.total > 0
              ? Math.min(100, Math.round((firstBucket.remaining / firstBucket.total) * 100))
              : null;

          return (
            <div
              key={group.id}
              className="rounded-xl border border-zinc-200/80 bg-zinc-50/70 p-2.5 dark:border-zinc-800 dark:bg-zinc-800/40"
            >
              <div className="flex items-center justify-between gap-2">
                <div className="flex items-center gap-1.5">
                  <ToolBrandIcon toolId={group.id} size="xs" />
                  <span className="text-[11px] font-semibold text-zinc-700 dark:text-zinc-200">
                    {group.name}
                  </span>
                </div>
                {firstBucket && (
                  <span className="shrink-0 whitespace-nowrap text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                    余 {Math.round(firstBucket.remaining * 100) / 100} {firstBucket.unit}
                  </span>
                )}
              </div>

              {pct !== null && (
                <div className="mt-1.5 h-1 w-full overflow-hidden rounded-full bg-zinc-200/70 dark:bg-zinc-700/70">
                  <div
                    className={`h-full rounded-full ${
                      pct <= 15
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
                <div className="mt-2 rounded-lg border border-dashed border-zinc-200/80 px-2 py-1.5 text-center text-[10px] text-zinc-400 dark:border-zinc-700/80 dark:text-zinc-500">
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
                            ? "cursor-default text-zinc-400 dark:text-zinc-500"
                            : "text-zinc-600 hover:bg-white/80 dark:text-zinc-400 dark:hover:bg-zinc-800/70"
                      }`}
                    >
                      <span className="truncate">{acc.label}</span>
                      {acc.isActiveOnClient ? (
                        <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-emerald-500" />
                      ) : acc.hasLocalSnapshot === false ? (
                        <span className="shrink-0 text-[10px] text-zinc-400">未关联本机</span>
                      ) : (
                        <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 text-[10px] text-zinc-400">
                          <ArrowLeftRight className="h-2.5 w-2.5 shrink-0" />
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

      <div className="mt-3 border-t border-zinc-100 pt-2.5 dark:border-zinc-800">
        <button
          type="button"
          onClick={() => claimDailyBatch()}
          disabled={Boolean(batchProgress?.running)}
          className="inline-flex w-full shrink-0 whitespace-nowrap items-center justify-center gap-1.5 rounded-xl bg-zinc-900 px-3 py-2 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:opacity-50 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
        >
          <Sparkles className="h-3.5 w-3.5 shrink-0" />
          {batchProgress?.running ? "串行领取中..." : "手动一键领取全部待领"}
        </button>
      </div>

      <ColdSwitchModal />
    </div>
  );
}
