import { AlertTriangle, CheckCircle2, Loader2, X } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";

export function BatchClaimBanner({
  compact = false,
  inline = false,
}: {
  compact?: boolean;
  inline?: boolean;
}) {
  const { batchProgress, dismissBatchProgress } = useQwitch();
  if (!batchProgress) return null;

  const pct =
    batchProgress.total > 0
      ? Math.round((batchProgress.completed / batchProgress.total) * 100)
      : 0;
  const isRefresh = batchProgress.action === "refresh";

  let statusLine = "";
  let inlineLabel = "";
  if (batchProgress.phase === "checking") {
    const label = batchProgress.currentLabel ?? "";
    statusLine = isRefresh ? `正在查额：${label}...` : `正在幂等检查：${label}...`;
    inlineLabel = isRefresh ? `查额 ${label}` : `检查 ${label}`;
  } else if (batchProgress.phase === "claiming") {
    const label = batchProgress.currentLabel ?? "";
    statusLine = `正在领取每日额度：${label}...`;
    inlineLabel = `领取 ${label}`;
  } else if (batchProgress.phase === "jitter_wait") {
    const secs =
      typeof batchProgress.jitterSeconds === "number"
        ? batchProgress.jitterSeconds.toFixed(1)
        : "2.0";
    statusLine = batchProgress.summaryText?.trim() || `防风控错峰等待 ${secs}s...`;
    inlineLabel = `等待 ${secs}s`;
  } else {
    statusLine =
      batchProgress.summaryText ?? (isRefresh ? "批量查额已完成" : "领取队列已完成");
    inlineLabel = statusLine;
  }

  if (inline) {
    if (!batchProgress.running) {
      const hasFailure =
        /异常\s+[1-9]/.test(statusLine) || statusLine.includes("失败");
      return (
        <div
          role="status"
          aria-live="polite"
          title={statusLine}
          className={`inline-flex h-[34px] max-w-[240px] items-center gap-1.5 rounded-[var(--radius-control)] border px-2.5 text-xs leading-5 ${
            hasFailure
              ? "border-amber-500/30 bg-amber-50/80 text-amber-900 dark:border-amber-400/25 dark:bg-amber-500/12 dark:text-amber-200"
              : "border-brand-500/25 bg-brand-50/80 text-brand-800 dark:border-brand-400/25 dark:bg-brand-500/12 dark:text-brand-200"
          }`}
        >
          {hasFailure ? (
            <AlertTriangle
              className="h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-400"
              aria-hidden="true"
            />
          ) : (
            <CheckCircle2
              className="h-3.5 w-3.5 shrink-0 text-brand-600 dark:text-brand-400"
              aria-hidden="true"
            />
          )}
          <span className="truncate font-medium">{inlineLabel}</span>
          <button
            type="button"
            onClick={dismissBatchProgress}
            aria-label="关闭任务状态提示"
            className="-mr-0.5 inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-md opacity-65 transition hover:bg-black/5 hover:opacity-100 dark:hover:bg-white/10"
          >
            <X className="h-3 w-3" aria-hidden="true" />
          </button>
        </div>
      );
    }

    return (
      <div
        role="status"
        aria-live="polite"
        title={statusLine}
        className="relative inline-flex h-[34px] max-w-[240px] items-center overflow-hidden rounded-[var(--radius-control)] border border-zinc-200/90 bg-white px-2.5 text-xs leading-5 shadow-2xs dark:border-zinc-700/80 dark:bg-zinc-900"
      >
        <div
          role="progressbar"
          aria-label={isRefresh ? "批量额度刷新进度" : "批量签到领取进度"}
          aria-valuenow={pct}
          aria-valuemin={0}
          aria-valuemax={100}
          className="pointer-events-none absolute inset-y-0 left-0 bg-brand-500/[0.11] transition-[width] duration-300 ease-out dark:bg-brand-400/[0.15]"
          style={{ width: `${pct}%` }}
        />
        <div className="relative z-10 flex min-w-0 items-center gap-2">
          <Loader2
            className="h-3.5 w-3.5 shrink-0 animate-spin text-brand-600 dark:text-brand-400"
            aria-hidden="true"
          />
          <span className="max-w-[116px] truncate font-medium text-zinc-700 dark:text-zinc-200">
            {inlineLabel}
          </span>
          <span className="shrink-0 font-mono text-[11px] font-medium tabular-nums text-brand-700 dark:text-brand-300">
            {batchProgress.completed}/{batchProgress.total}
          </span>
        </div>
      </div>
    );
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
      className={`rounded-lg border border-brand-500/20 bg-brand-500/[0.06] dark:border-brand-400/20 dark:bg-brand-500/10 ${
        compact ? "px-2.5 py-2" : "px-3 py-2"
      }`}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5 text-xs">
          <Loader2
            className="h-3.5 w-3.5 shrink-0 animate-spin text-brand-600 dark:text-brand-400"
            aria-hidden="true"
          />
          <span className="truncate font-medium text-zinc-900 dark:text-zinc-100">
            {statusLine}
          </span>
          <span className="shrink-0 tabular-nums text-zinc-600 dark:text-zinc-400">
            ({batchProgress.completed}/{batchProgress.total})
          </span>
        </div>

        <span className="badge shrink-0 border-brand-500/25 bg-brand-500/12 tabular-nums text-brand-800 dark:text-brand-300">
          {pct}%
        </span>
      </div>

      <div
        role="progressbar"
        aria-label={isRefresh ? "批量额度刷新进度" : "批量签到领取进度"}
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        className="mt-1.5 h-1 w-full overflow-hidden rounded-full bg-brand-500/15 dark:bg-brand-400/15"
      >
        <div
          className="h-full rounded-full bg-brand-500 transition-all duration-300"
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  );
}
