import { Loader2, ShieldCheck, X } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";

export function BatchClaimBanner() {
  const { batchProgress, dismissBatchProgress } = useQwitch();
  if (!batchProgress) return null;

  const pct = batchProgress.total > 0 ? Math.round((batchProgress.completed / batchProgress.total) * 100) : 0;

  let statusLine = "";
  if (batchProgress.phase === "checking") {
    statusLine = `正在幂等检查：${batchProgress.currentLabel ?? ""}...`;
  } else if (batchProgress.phase === "claiming") {
    statusLine = `正在领取每日额度：${batchProgress.currentLabel ?? ""}...`;
  } else if (batchProgress.phase === "jitter_wait") {
    statusLine = `防风控错峰等待 ${batchProgress.jitterSeconds ?? 2}s，即将处理下一个账号...`;
  } else {
    statusLine = batchProgress.summaryText ?? "领取队列已完成";
  }

  return (
    <div className="rounded-xl border border-zinc-200 bg-white p-3.5 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2.5 text-xs">
          {batchProgress.running ? (
            <Loader2 className="h-4 w-4 animate-spin text-sky-500" />
          ) : (
            <ShieldCheck className="h-4 w-4 text-emerald-500" />
          )}
          <span className="font-medium text-zinc-900 dark:text-zinc-100">{statusLine}</span>
          <span className="tabular-nums text-zinc-400">
            ({batchProgress.completed}/{batchProgress.total})
          </span>
        </div>

        <div className="flex shrink-0 items-center gap-2">
          <span className="rounded-md bg-zinc-100 px-1.5 py-0.5 text-[11px] font-medium tabular-nums text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
            {pct}%
          </span>
          {!batchProgress.running && (
            <button
              type="button"
              onClick={dismissBatchProgress}
              className="rounded p-1 text-zinc-400 hover:bg-zinc-100 hover:text-zinc-700 dark:hover:bg-zinc-800 dark:hover:text-zinc-200"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          )}
        </div>
      </div>

      <div className="mt-2.5 h-1.5 w-full overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
        <div
          className="h-full rounded-full bg-sky-500 transition-all duration-300"
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  );
}
