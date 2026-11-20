import type {
  ActionLogCleanupRequest,
  ActionLogItem,
  ActionLogStats,
  LogScope,
} from "../types/qwitch";

export function resolveTheme(t: "light" | "dark" | "system"): "light" | "dark" {
  if (t === "system") {
    return typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }
  return t;
}

export function isRedundantLog(log: ActionLogItem): boolean {
  return (
    log.resultStatus === "skipped" ||
    (log.actionType === "query" && log.resultStatus === "success")
  );
}

export function matchesLogCleanup(
  log: ActionLogItem,
  req: ActionLogCleanupRequest,
  defaultScope: LogScope,
  nowMs = Date.now()
): boolean {
  const scope = req.scope ?? defaultScope;
  if (scope === "active" && log.archivedAt) return false;
  if (scope === "archived" && !log.archivedAt) return false;
  if (req.ids && !req.ids.includes(log.id)) {
    return false;
  }
  if (req.olderThanDays && req.olderThanDays > 0) {
    const cutoff = nowMs - req.olderThanDays * 86_400_000;
    if (new Date(log.createdAt).getTime() >= cutoff) return false;
  }
  if (req.actionType && req.actionType !== "all" && log.actionType !== req.actionType) {
    return false;
  }
  if (req.resultStatus && req.resultStatus !== "all" && log.resultStatus !== req.resultStatus) {
    return false;
  }
  if (req.redundantOnly && !isRedundantLog(log)) return false;
  return true;
}

export function mergeUniqueLogs(
  prev: ActionLogItem[],
  more: ActionLogItem[]
): ActionLogItem[] {
  const seen = new Set(prev.map((item) => item.id));
  const merged = [...prev];
  for (const item of more) {
    if (!seen.has(item.id)) {
      seen.add(item.id);
      merged.push(item);
    }
  }
  return merged;
}

export function computeMockLogStats(logsState: ActionLogItem[]): ActionLogStats {
  let activeCount = 0;
  let archivedCount = 0;
  let failedActiveCount = 0;
  let noiseActiveCount = 0;

  for (const item of logsState) {
    if (item.archivedAt) {
      archivedCount++;
      continue;
    }
    activeCount++;
    if (item.resultStatus === "failed") {
      failedActiveCount++;
    }
    if (isRedundantLog(item)) {
      noiseActiveCount++;
    }
  }

  return {
    activeCount,
    archivedCount,
    failedActiveCount,
    noiseActiveCount,
  };
}
