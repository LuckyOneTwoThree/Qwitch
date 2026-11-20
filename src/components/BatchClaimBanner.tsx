import { AlertTriangle, CheckCircle2, Loader2, X } from "lucide-react";
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
    const secs =
      typeof batchProgress.jitterSeconds === "number"
        ? batchProgress.jitterSeconds.toFixed(1)
        : "2.0";
    statusLine = batchProgress.summaryText?.trim() || `防风控错峰等待 ${secs}s...`;
  } else {
    statusLine =
      batchProgress.summaryText ?? (isRefresh ? "批量查额已完成" : "领取队列已完成");
  }

  if (!batchProgress.running) {
    const hasFailure =
      /异常\s+[1-9]/.test(statusLine) || statusLine.includes("失败");

    return (
      <div
        role="status"
        aria-live="polite"
        className={`flex items-center justify-between gap-2 rounded-lg border transition-all ${
          compact ? "px-2.5 py-1.5" : "px-3 py-1.5"
        } ${
          hasFailure
            ? "border-amber-500/25 bg-amber-500/[0.08] text-amber-900 dark:border-amber-400/25 dark:bg-amber-500/10 dark:text-amber-200"
            : "border-emerald-500/25 bg-emerald-500/[0.08] text-emerald-900 dark:border-emerald-400/25 dark:bg-emerald-500/10 dark:text-emerald-200"
        }`}
      >
        <div className="flex min-w-0 items-center gap-1.5 text-xs">
          {hasFailure ? (
            <AlertTriangle
              className="h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-400"
              aria-hidden="true"
            />
          ) : (
            <CheckCircle2
              className="h-3.5 w-3.5 shrink-0 text-emerald-600 dark:text-emerald-400"
              aria-hidden="true"
            />
          )}
          <span className="truncate font-medium">{statusLine}</span>
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          {!compact && (
            <span className="text-[11px] opacity-70">自动收起</span>
          )}
          <button
            type="button"
            onClick={dismissBatchProgress}
            aria-label="关闭任务状态提示"
            className="rounded-md p-0.5 opacity-70 transition hover:bg-black/5 hover:opacity-100 dark:hover:bg-white/10"
          >
            <X className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>
      </div>
    );
  }

  return (
    <div
      role="status"
      aria-live="polite"
      className={`rounded-lg border border-sky-500/20 bg-sky-500/[0.06] dark:border-sky-400/20 dark:bg-sky-500/10 ${
        compact ? "px-2.5 py-2" : "px-3 py-2"
      }`}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5 text-xs">
          <Loader2
            className="h-3.5 w-3.5 shrink-0 animate-spin text-sky-600 dark:text-sky-400"
            aria-hidden="true"
          />
          <span className="truncate font-medium text-zinc-900 dark:text-zinc-100">
            {statusLine}
          </span>
          <span className="shrink-0 tabular-nums text-zinc-600 dark:text-zinc-400">
            ({batchProgress.completed}/{batchProgress.total})
          </span>
        </div>

        <span className="badge shrink-0 border-sky-500/25 bg-sky-500/12 tabular-nums text-sky-800 dark:text-sky-300">
          {pct}%
        </span>
      </div>

      <div
        role="progressbar"
        aria-label={isRefresh ? "批量额度刷新进度" : "批量签到领取进度"}
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        className="mt-1.5 h-1 w-full overflow-hidden rounded-full bg-sky-500/15 dark:bg-sky-400/15"
      >
        <div
          className="h-full rounded-full bg-sky-500 transition-all duration-300"
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  );
}
