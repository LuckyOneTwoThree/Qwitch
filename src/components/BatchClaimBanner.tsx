import { Loader2, ShieldCheck, X } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";

export function BatchClaimBanner({ compact = false }: { compact?: boolean }) {
  const { batchProgress, dismissBatchProgress } = useQwitch();
  if (!batchProgress) return null;

  const pct =
    batchProgress.total > 0
      ? Math.round((batchProgress.completed / batchProgress.total) * 100)
      : 0;
  const isRefresh = batchProgress.action === "refresh";

  let statusLine = "";
  if (batchProgress.phase === "checking") {
    statusLine = isRefresh
      ? `正在查额：${batchProgress.currentLabel ?? ""}...`
      : `正在幂等检查：${batchProgress.currentLabel ?? ""}...`;
  } else if (batchProgress.phase === "claiming") {
    statusLine = `正在领取每日额度：${batchProgress.currentLabel ?? ""}...`;
  } else if (batchProgress.phase === "jitter_wait") {
    statusLine = `防风控错峰等待 ${batchProgress.jitterSeconds ?? 2}s...`;
  } else {
    statusLine =
      batchProgress.summaryText ?? (isRefresh ? "批量查额已完成" : "领取队列已完成");
  }

  return (
    <div
      className={`rounded-xl border border-zinc-200 bg-white shadow-sm dark:border-zinc-800 dark:bg-zinc-900 ${
        compact ? "p-2.5" : "p-3.5"
      }`}
    >
      <div className="flex items-center justify-between gap-2.5">
        <div className="flex min-w-0 items-center gap-2 text-xs">
          {batchProgress.running ? (
            <Loader2 className="h-4 w-4 shrink-0 animate-spin text-sky-500" aria-hidden="true" />
          ) : (
            <ShieldCheck className="h-4 w-4 shrink-0 text-emerald-500" aria-hidden="true" />
          )}
          <span className="truncate font-medium text-zinc-900 dark:text-zinc-100">
            {statusLine}
          </span>
          <span className="shrink-0 tabular-nums text-zinc-500 dark:text-zinc-400">
            ({batchProgress.completed}/{batchProgress.total})
          </span>
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          <span className="rounded-md bg-zinc-100 px-1.5 py-0.5 text-[11px] font-medium tabular-nums text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
            {pct}%
          </span>
          {!batchProgress.running && (
            <button
              type="button"
              onClick={dismissBatchProgress}
              aria-label="关闭任务进度条"
              className="rounded p-1 text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
            >
              <X className="h-3.5 w-3.5" aria-hidden="true" />
            </button>
          )}
        </div>
      </div>

      <div
        role="progressbar"
        aria-label={isRefresh ? "批量额度刷新进度" : "批量签到领取进度"}
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800"
      >
        <div
          className="h-full rounded-full bg-sky-500 transition-all duration-300"
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  );
}
