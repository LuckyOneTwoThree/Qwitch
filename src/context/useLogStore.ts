import { useCallback, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getMockLogs } from "../data/mockStoreLoader";
import { computeMockLogStats, matchesLogCleanup, mergeUniqueLogs } from "./logStateUtils";
import type {
  ActionLogCleanupRequest,
  ActionLogItem,
  ActionLogStats,
  ExportLogsResult,
  LogScope,
} from "../types/qwitch";

const LOG_PAGE_SIZE = 200;
const LOG_MAX_LIMIT = 2000;

interface UseLogStoreArgs {
  isTauriRuntime: boolean;
  showToast: (message: string, tone?: "success" | "error" | "warning" | "info") => void;
}

/**
 * Log 域 store：集中管理操作日志列表、作用域、统计、游标分页与增删改/归档/导出。
 *
 * 从巨型 `QwitchContext` 中拆出的四个 store 之一。对外暴露稳定的 action 引用
 * （`requestSnapshot`/`commitSnapshot`/`resetPagination` 供全量同步复用），
 * 保持 `useQwitch()` 返回契约不变。
 */
export function useLogStore({ isTauriRuntime, showToast }: UseLogStoreArgs) {
  const [logsState, setLogsState] = useState<ActionLogItem[]>(
    isTauriRuntime ? [] : getMockLogs()
  );
  const [logScope, setLogScope] = useState<"active" | "archived">("active");
  const [backendLogStats, setBackendLogStats] = useState<ActionLogStats>({
    activeCount: 0,
    archivedCount: 0,
    failedActiveCount: 0,
    noiseActiveCount: 0,
  });
  const logLimitRef = useRef(LOG_PAGE_SIZE);
  const logScopeRef = useRef(logScope);
  logScopeRef.current = logScope;
  const logRequestSeqRef = useRef(0);
  const loadingMoreLogsRef = useRef(false);
  const logsRef = useRef(logsState);
  logsRef.current = logsState;

  const refreshLogs = useCallback(
    async (targetScope?: "active" | "archived") => {
      if (!isTauriRuntime) return;
      const seq = ++logRequestSeqRef.current;
      const scope = targetScope ?? logScopeRef.current;
      const limit = Math.min(LOG_MAX_LIMIT, logLimitRef.current);
      try {
        const [logList, stats] = await Promise.all([
          invoke<ActionLogItem[]>("get_action_logs", {
            scope,
            limit,
            offset: 0,
            beforeId: null,
          }),
          invoke<ActionLogStats>("get_action_log_stats"),
        ]);
        if (seq !== logRequestSeqRef.current || scope !== logScopeRef.current) {
          return;
        }
        setLogsState(logList);
        setBackendLogStats(stats);
      } catch (err) {
        showToast(`刷新日志失败：${String(err)}`, "error");
      }
    },
    [isTauriRuntime, showToast]
  );

  const loadMoreLogs = useCallback(async () => {
    if (!isTauriRuntime || loadingMoreLogsRef.current) return;
    loadingMoreLogsRef.current = true;
    const seq = logRequestSeqRef.current;
    const scope = logScopeRef.current;
    const currentLogs = logsRef.current;
    const cursorId = currentLogs.length > 0 ? currentLogs[currentLogs.length - 1].id : null;
    try {
      const more = await invoke<ActionLogItem[]>("get_action_logs", {
        scope,
        limit: LOG_PAGE_SIZE,
        offset: 0,
        beforeId: cursorId,
      });
      if (
        more.length === 0 ||
        seq !== logRequestSeqRef.current ||
        scope !== logScopeRef.current
      ) {
        return;
      }
      logLimitRef.current = Math.min(LOG_MAX_LIMIT, logsRef.current.length + more.length);
      setLogsState((prev) => mergeUniqueLogs(prev, more));
    } catch (err) {
      showToast(`加载更多日志失败：${String(err)}`, "error");
    } finally {
      loadingMoreLogsRef.current = false;
    }
  }, [isTauriRuntime, showToast]);

  const appendLog = useCallback((entry: Omit<ActionLogItem, "id" | "createdAt">) => {
    setLogsState((prev) => [
      {
        ...entry,
        id: Date.now() + Math.floor(Math.random() * 1000),
        createdAt: new Date().toISOString(),
      },
      ...prev,
    ]);
  }, []);

  const logs = useMemo(() => {
    if (isTauriRuntime) return logsState;
    return logsState.filter((item) =>
      logScope === "archived" ? Boolean(item.archivedAt) : !item.archivedAt
    );
  }, [isTauriRuntime, logScope, logsState]);

  const logStats = useMemo<ActionLogStats>(() => {
    if (isTauriRuntime) return backendLogStats;
    return computeMockLogStats(logsState);
  }, [backendLogStats, isTauriRuntime, logsState]);

  const hasMoreLogs = useMemo(() => {
    if (!isTauriRuntime) return false;
    const total = logScope === "archived" ? logStats.archivedCount : logStats.activeCount;
    return logsState.length < total;
  }, [isTauriRuntime, logScope, logStats.activeCount, logStats.archivedCount, logsState.length]);

  const archiveLogs = useCallback(
    async (
      request: ActionLogCleanupRequest,
      feedbackLabel?: string | ((count: number) => string)
    ) => {
      const formatFeedback = (count: number) =>
        typeof feedbackLabel === "function"
          ? feedbackLabel(count)
          : feedbackLabel ?? `已归档 ${count} 条日志`;
      if (isTauriRuntime) {
        try {
          const count = await invoke<number>("archive_action_logs", { request });
          await refreshLogs();
          if (count === 0) {
            showToast("当前没有可归档的匹配日志", "info");
          } else {
            showToast(formatFeedback(count), "success");
          }
          return count;
        } catch (err) {
          showToast(`归档日志失败：${String(err)}`, "error");
          return null;
        }
      }

      const now = new Date().toISOString();
      // 先纯计算匹配集再 setState：updater 内累加会被 StrictMode 重复执行导致计数翻倍
      const count = logsRef.current.filter((item) =>
        matchesLogCleanup(item, request, "active")
      ).length;
      setLogsState((prev) =>
        prev.map((item) =>
          matchesLogCleanup(item, request, "active") ? { ...item, archivedAt: now } : item
        )
      );
      showToast(
        count === 0 ? "当前没有可归档的匹配日志" : formatFeedback(count),
        count === 0 ? "info" : "success"
      );
      return count;
    },
    [isTauriRuntime, refreshLogs, showToast]
  );

  const unarchiveLogs = useCallback(
    async (
      request: ActionLogCleanupRequest,
      feedbackLabel?: string | ((count: number) => string)
    ) => {
      const formatFeedback = (count: number) =>
        typeof feedbackLabel === "function"
          ? feedbackLabel(count)
          : feedbackLabel ?? `已恢复 ${count} 条日志到活跃视图`;
      if (isTauriRuntime) {
        try {
          const count = await invoke<number>("unarchive_action_logs", { request });
          await refreshLogs();
          showToast(
            count === 0 ? "没有可恢复的归档日志" : formatFeedback(count),
            count === 0 ? "info" : "success"
          );
          return count;
        } catch (err) {
          showToast(`恢复日志失败：${String(err)}`, "error");
          return null;
        }
      }

      const count = logsRef.current.filter((item) =>
        matchesLogCleanup(item, request, "archived")
      ).length;
      setLogsState((prev) =>
        prev.map((item) =>
          matchesLogCleanup(item, request, "archived") ? { ...item, archivedAt: undefined } : item
        )
      );
      showToast(
        count === 0 ? "没有可恢复的归档日志" : formatFeedback(count),
        count === 0 ? "info" : "success"
      );
      return count;
    },
    [isTauriRuntime, refreshLogs, showToast]
  );

  const deleteLogs = useCallback(
    async (
      request: ActionLogCleanupRequest,
      feedbackLabel?: string | ((count: number) => string)
    ) => {
      const formatFeedback = (count: number) =>
        typeof feedbackLabel === "function"
          ? feedbackLabel(count)
          : feedbackLabel ?? `已删除 ${count} 条日志`;
      if (isTauriRuntime) {
        try {
          const count = await invoke<number>("delete_action_logs", { request });
          await refreshLogs();
          showToast(
            count === 0 ? "没有可删除的匹配日志" : formatFeedback(count),
            count === 0 ? "info" : "success"
          );
          return count;
        } catch (err) {
          showToast(`删除日志失败：${String(err)}`, "error");
          return null;
        }
      }

      const count = logsRef.current.filter((item) =>
        matchesLogCleanup(item, request, logScope)
      ).length;
      setLogsState((prev) => prev.filter((item) => !matchesLogCleanup(item, request, logScope)));
      showToast(
        count === 0 ? "没有可删除的匹配日志" : formatFeedback(count),
        count === 0 ? "info" : "success"
      );
      return count;
    },
    [isTauriRuntime, logScope, refreshLogs, showToast]
  );

  const exportLogs = useCallback(
    async (scope: LogScope = logScope, archiveAfterExport = false) => {
      if (isTauriRuntime) {
        try {
          const res = await invoke<ExportLogsResult>("export_action_logs", {
            scope,
            archiveAfterExport,
          });
          await refreshLogs();
          const fileName = res.filePath.split(/[\\/]/).pop() ?? res.filePath;
          showToast(
            res.archivedCount > 0
              ? `已导出 ${res.count} 条并归档（${fileName}）`
              : `已导出 ${res.count} 条至 archives/${fileName}`,
            "success"
          );
        } catch (err) {
          showToast(`导出日志失败：${String(err)}`, "error");
        }
        return;
      }

      const targetItems = logsState.filter((item) => {
        if (scope === "active") return !item.archivedAt;
        if (scope === "archived") return Boolean(item.archivedAt);
        return true;
      });
      if (targetItems.length === 0) {
        showToast("当前视图没有可导出的操作日志", "info");
        return;
      }
      const blob = new Blob(
        [
          JSON.stringify(
            {
              app: "Qwitch",
              exportedAt: new Date().toISOString(),
              scope,
              count: targetItems.length,
              logs: targetItems,
            },
            null,
            2
          ),
        ],
        { type: "application/json" }
      );
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `qwitch-logs-${scope}-${Date.now()}.json`;
      anchor.click();
      URL.revokeObjectURL(url);
      if (archiveAfterExport && scope === "active") {
        await archiveLogs({ scope: "active" }, `已导出 ${targetItems.length} 条并全部归档`);
      } else {
        showToast(`已导出 ${targetItems.length} 条操作日志`, "success");
      }
    },
    [archiveLogs, isTauriRuntime, logScope, logsState, refreshLogs, showToast]
  );

  // 供全量同步（syncFromBackend）复用的稳定接口
  const requestSnapshot = useCallback(() => {
    return {
      seq: logRequestSeqRef.current,
      scope: logScopeRef.current,
      limit: Math.min(LOG_MAX_LIMIT, logLimitRef.current),
    };
  }, []);

  const commitSnapshot = useCallback(
    (list: ActionLogItem[], stats: ActionLogStats, seq: number, scope: "active" | "archived") => {
      if (seq === logRequestSeqRef.current && scope === logScopeRef.current) {
        setLogsState(list);
        setBackendLogStats(stats);
      }
    },
    []
  );

  const resetPagination = useCallback(() => {
    logRequestSeqRef.current += 1;
    logLimitRef.current = LOG_PAGE_SIZE;
  }, []);

  return {
    logs,
    logScope,
    setLogScope,
    logStats,
    hasMoreLogs,
    loadMoreLogs,
    refreshLogs,
    appendLog,
    archiveLogs,
    unarchiveLogs,
    deleteLogs,
    exportLogs,
    requestSnapshot,
    commitSnapshot,
    resetPagination,
  };
}
