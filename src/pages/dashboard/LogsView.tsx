import { useMemo, useState } from "react";
import { Sparkles, ScrollText, Search, X, Archive, ArchiveRestore, Trash2, Download, Clock, CheckSquare, Square } from "lucide-react";
import { useQwitch } from "../../context/QwitchContext";
import { BatchClaimBanner } from "../../components/BatchClaimBanner";
import { ToolBrandIcon } from "../../components/ToolBrandIcon";
import { getToolBrandMeta } from "../../domain/toolRegistry";
import type { Dispatch, SetStateAction } from "react";
import type { ActionLogItem } from "../../types/qwitch";
import {
  ACTION_TYPE_META,
  type ConfirmLogAction,
  type LogActionFilter,
  type LogDayGroup,
  type LogResultFilter,
} from "./types";

export interface LogsToolbarProps {
  allVisibleLogsSelected: boolean;
  filteredLogs: ActionLogItem[];
  logActionFilter: LogActionFilter;
  setLogActionFilter: Dispatch<SetStateAction<LogActionFilter>>;
  logResultFilter: LogResultFilter;
  setLogResultFilter: Dispatch<SetStateAction<LogResultFilter>>;
  logSearch: string;
  setLogSearch: Dispatch<SetStateAction<string>>;
  selectedLogIds: number[];
  setSelectedLogIds: Dispatch<SetStateAction<number[]>>;
  setConfirmLogAction: Dispatch<SetStateAction<ConfirmLogAction | null>>;
  toggleSelectAllVisibleLogs: () => void;
}

export function LogsToolbar({
  allVisibleLogsSelected,
  filteredLogs,
  logActionFilter,
  setLogActionFilter,
  logResultFilter,
  setLogResultFilter,
  logSearch,
  setLogSearch,
  selectedLogIds,
  setSelectedLogIds,
  setConfirmLogAction,
  toggleSelectAllVisibleLogs,
}: LogsToolbarProps) {
  const { logStats, logScope, setLogScope, archiveLogs, unarchiveLogs, deleteLogs, exportLogs } = useQwitch();
  return (
    <div className="shrink-0 space-y-2.5 border-b border-zinc-200/80 bg-zinc-50/95 px-5 py-3 dark:border-zinc-800 dark:bg-zinc-950/95">
      <BatchClaimBanner />

      <div className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-zinc-200 bg-white p-3 dark:border-zinc-800 dark:bg-zinc-900">
        <div className="inline-flex rounded-xl border border-zinc-200 bg-zinc-50 p-0.5 dark:border-zinc-800 dark:bg-zinc-950/60">
          <button
            type="button"
            onClick={() => setLogScope("active")}
            className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
              logScope === "active"
                ? "bg-zinc-900 text-white shadow-2xs dark:bg-zinc-100 dark:text-zinc-900"
                : "text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
            }`}
          >
            <ScrollText className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            活跃日志
            <span
              className={`rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                logScope === "active"
                  ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                  : "bg-zinc-200/70 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
              }`}
            >
              {logStats.activeCount}
            </span>
          </button>
          <button
            type="button"
            onClick={() => setLogScope("archived")}
            className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
              logScope === "archived"
                ? "bg-zinc-900 text-white shadow-2xs dark:bg-zinc-100 dark:text-zinc-900"
                : "text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
            }`}
          >
            <Archive className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            已归档
            <span
              className={`rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                logScope === "archived"
                  ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                  : "bg-zinc-200/70 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
              }`}
            >
              {logStats.archivedCount}
            </span>
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          {logScope === "active" ? (
            <>
              <button
                type="button"
                disabled={logStats.noiseActiveCount === 0}
                onClick={() =>
                  void archiveLogs(
                    { scope: "active", redundantOnly: true },
                    (count) =>
                      `已归档 ${count} 条日常冗余记录（已保留签到、切号与异常）`
                  )
                }
                title="将日常“额度查询成功”和“幂等跳过”归档，仅保留签到成功、切号、导入及所有异常"
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-45 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
              >
                <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                清理冗余
                {logStats.noiseActiveCount > 0 && (
                  <span className="rounded-full bg-white/20 px-1.5 text-[11px] tabular-nums dark:bg-zinc-900/15">
                    {logStats.noiseActiveCount}
                  </span>
                )}
              </button>

              <button
                type="button"
                disabled={logStats.activeCount === 0}
                onClick={() =>
                  void archiveLogs(
                    { scope: "active", olderThanDays: 7 },
                    "已将 7 天前活跃日志移入归档"
                  )
                }
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
              >
                <Clock className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                归档 7 天前
              </button>

              <button
                type="button"
                disabled={logStats.activeCount === 0}
                onClick={() =>
                  setConfirmLogAction({
                    label: "归档当前全部活跃日志？",
                    detail: `将把 ${logStats.activeCount} 条活跃日志全部转入「已归档」，可在归档区查看、导出或恢复。`,
                    run: async () => (await archiveLogs({ scope: "active" })) !== null,
                  })
                }
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
              >
                <Archive className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                全部归档
              </button>

              <button
                type="button"
                disabled={logStats.activeCount === 0}
                onClick={() => void exportLogs("active", false)}
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
              >
                <Download className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                导出
              </button>

              <button
                type="button"
                disabled={logStats.activeCount === 0}
                onClick={() =>
                  setConfirmLogAction({
                    label: "确认清空全部活跃日志？",
                    detail: `将永久删除 ${logStats.activeCount} 条活跃日志，不可恢复；若需留存请先点击「全部归档」或「导出」。`,
                    danger: true,
                    run: async () => (await deleteLogs({ scope: "active" })) !== null,
                  })
                }
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-rose-200 bg-rose-50/70 px-2.5 py-1.5 text-xs font-medium text-rose-700 transition hover:bg-rose-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-rose-500/30 dark:bg-rose-500/10 dark:text-rose-300 dark:hover:bg-rose-500/20"
              >
                <Trash2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                清空活跃
              </button>
            </>
          ) : (
            <>
              <button
                type="button"
                disabled={logStats.archivedCount === 0}
                onClick={() => void exportLogs("archived", false)}
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-45 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
              >
                <Download className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                导出归档 JSON
              </button>

              <button
                type="button"
                disabled={logStats.archivedCount === 0}
                onClick={() =>
                  void unarchiveLogs(
                    { scope: "archived" },
                    (count) => `已恢复 ${count} 条归档日志`
                  )
                }
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
              >
                <ArchiveRestore className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                全部恢复
              </button>

              <button
                type="button"
                disabled={logStats.archivedCount === 0}
                onClick={() =>
                  setConfirmLogAction({
                    label: "确认彻底清空已归档日志？",
                    detail: `将永久删除 ${logStats.archivedCount} 条已归档历史记录，建议先点击「导出归档 JSON」保存备份。`,
                    danger: true,
                    run: async () => (await deleteLogs({ scope: "archived" })) !== null,
                  })
                }
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-rose-200 bg-rose-50/70 px-2.5 py-1.5 text-xs font-medium text-rose-700 transition hover:bg-rose-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-rose-500/30 dark:bg-rose-500/10 dark:text-rose-300 dark:hover:bg-rose-500/20"
              >
                <Trash2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                彻底清空归档
              </button>
            </>
          )}
        </div>
      </div>

      <div className="flex flex-col gap-2.5 rounded-2xl border border-zinc-200 bg-white p-3 dark:border-zinc-800 dark:bg-zinc-900">
        <div className="flex flex-wrap items-center justify-between gap-2.5">
          <div className="relative min-w-[220px] flex-1">
            <Search
              className="pointer-events-none absolute top-1/2 left-3 h-3.5 w-3.5 -translate-y-1/2 text-zinc-500 dark:text-zinc-400"
              aria-hidden="true"
            />
            <input
              type="text"
              value={logSearch}
              onChange={(e) => setLogSearch(e.target.value)}
              placeholder="搜索日志内容、平台名称或错误码..."
              aria-label="搜索操作日志"
              className="w-full rounded-xl border border-zinc-200 bg-zinc-50/70 py-1.5 pr-8 pl-8 text-xs text-zinc-900 transition focus:border-zinc-400 focus:bg-white dark:border-zinc-700 dark:bg-zinc-800/60 dark:text-zinc-100 dark:focus:border-zinc-500"
            />
            {logSearch && (
              <button
                type="button"
                onClick={() => setLogSearch("")}
                className="absolute top-1/2 right-2.5 -translate-y-1/2 text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-200"
                aria-label="清除搜索"
              >
                <X className="h-3.5 w-3.5" aria-hidden="true" />
              </button>
            )}
          </div>

          {filteredLogs.length > 0 && (
            <div className="flex flex-wrap items-center gap-1.5">
              <button
                type="button"
                onClick={toggleSelectAllVisibleLogs}
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200"
              >
                {allVisibleLogsSelected ? (
                  <CheckSquare className="h-3.5 w-3.5 text-zinc-900 dark:text-zinc-100" aria-hidden="true" />
                ) : (
                  <Square className="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" aria-hidden="true" />
                )}
                {allVisibleLogsSelected ? "取消全选" : `全选 (${filteredLogs.length})`}
              </button>

              {selectedLogIds.length > 0 && (
                <>
                  {logScope === "active" ? (
                    <button
                      type="button"
                      onClick={async () => {
                        const ids = [...selectedLogIds];
                        const count = await archiveLogs({ scope: "active", ids });
                        if (count !== null) {
                          setSelectedLogIds([]);
                        }
                      }}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900"
                    >
                      <Archive className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      归档已选 ({selectedLogIds.length})
                    </button>
                  ) : (
                    <button
                      type="button"
                      onClick={async () => {
                        const ids = [...selectedLogIds];
                        const count = await unarchiveLogs({ scope: "archived", ids });
                        if (count !== null) {
                          setSelectedLogIds([]);
                        }
                      }}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900"
                    >
                      <ArchiveRestore className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      恢复已选 ({selectedLogIds.length})
                    </button>
                  )}

                  <button
                    type="button"
                    onClick={() =>
                      setConfirmLogAction({
                        label: `确认永久删除已选 ${selectedLogIds.length} 条日志？`,
                        detail: "删除后将无法在活跃或归档列表中恢复。",
                        danger: true,
                        run: async () => {
                          const ids = [...selectedLogIds];
                          return (await deleteLogs({ scope: logScope, ids })) !== null;
                        },
                      })
                    }
                    className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-rose-200 bg-rose-50 px-2.5 py-1.5 text-xs font-medium text-rose-700 hover:bg-rose-100 dark:border-rose-500/30 dark:bg-rose-500/10 dark:text-rose-300"
                  >
                    <Trash2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    删除已选 ({selectedLogIds.length})
                  </button>
                </>
              )}
            </div>
          )}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-2.5">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="mr-1 text-[11px] text-zinc-500 dark:text-zinc-400">
              动作：
            </span>
            {(
              [
                { id: "all", label: "全部" },
                { id: "claim", label: "每日签到" },
                { id: "query", label: "额度查询" },
                { id: "switch", label: "桌面切号" },
                { id: "import", label: "账号管理" },
              ] as const
            ).map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setLogActionFilter(item.id)}
                className={`shrink-0 whitespace-nowrap rounded-lg px-2.5 py-1 text-xs font-medium transition ${
                  logActionFilter === item.id
                    ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                    : "bg-zinc-100 text-zinc-700 hover:bg-zinc-200/70 dark:bg-zinc-800 dark:text-zinc-300"
                }`}
              >
                {item.label}
              </button>
            ))}
          </div>

          <div className="flex flex-wrap items-center gap-1.5">
            <span className="mr-1 text-[11px] text-zinc-500 dark:text-zinc-400">
              结果：
            </span>
            {(
              [
                { id: "all", label: "全部" },
                { id: "success", label: "成功" },
                { id: "skipped", label: "幂等跳过" },
                { id: "failed", label: "异常" },
              ] as const
            ).map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setLogResultFilter(item.id)}
                className={`shrink-0 whitespace-nowrap rounded-lg px-2.5 py-1 text-xs font-medium transition ${
                  logResultFilter === item.id
                    ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                    : "bg-zinc-100 text-zinc-700 hover:bg-zinc-200/70 dark:bg-zinc-800 dark:text-zinc-300"
                }`}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

export interface LogsPanelProps {
  currentLogTotal: number;
  filteredLogs: ActionLogItem[];
  groupedLogs: LogDayGroup[];
  selectedLogIds: number[];
  setConfirmLogAction: Dispatch<SetStateAction<ConfirmLogAction | null>>;
  toggleSelectLog: (id: number) => void;
}

export function LogsPanel({
  currentLogTotal,
  filteredLogs,
  groupedLogs,
  selectedLogIds,
  setConfirmLogAction,
  toggleSelectLog,
}: LogsPanelProps) {
  const { adapters, logs, logScope, hasMoreLogs, loadMoreLogs, archiveLogs, unarchiveLogs, deleteLogs } = useQwitch();
  const [loadingMoreLogs, setLoadingMoreLogs] = useState(false);
  const selectedLogIdSet = useMemo(() => new Set(selectedLogIds), [selectedLogIds]);
  return (
      <div className="space-y-3">
        <div className="overflow-hidden rounded-2xl border border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900">
          {filteredLogs.length === 0 ? (
            <div className="px-6 py-12 text-center">
              <div className="text-xs font-medium text-zinc-700 dark:text-zinc-300">
                {logScope === "archived"
                  ? "归档区暂无历史日志"
                  : "当前筛选条件下暂无操作记录"}
              </div>
              <p className="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                {logScope === "archived"
                  ? "当日常冗余或超期日志被归档后，可在此查看、导出或恢复。"
                  : "若需查看历史已归档日志，请切换到上方「已归档」标签页。"}
              </p>
            </div>
          ) : (
            <div>
              {groupedLogs.map((group) => (
                <div key={group.day}>
                  <div className="flex items-center justify-between border-y border-zinc-100 bg-zinc-50/80 px-4 py-1.5 text-[11px] font-medium text-zinc-600 first:border-t-0 dark:border-zinc-800 dark:bg-zinc-950/40 dark:text-zinc-400">
                    <span>{group.day}</span>
                    <span className="tabular-nums text-zinc-500 dark:text-zinc-400">
                      {group.items.length} 条
                    </span>
                  </div>
                  <div className="divide-y divide-zinc-100 dark:divide-zinc-800">
                    {group.items.map((log) => {
                      const actionMeta = ACTION_TYPE_META[log.actionType] ?? {
                        label: log.actionType,
                        style: "bg-zinc-100 text-zinc-700",
                      };
                      const toolName = getToolBrandMeta(log.toolId, adapters).name;
                      const checked = selectedLogIdSet.has(log.id);
                      return (
                        <div
                          key={log.id}
                          className={`group flex items-center justify-between gap-3 px-4 py-2.5 text-xs transition ${
                            checked
                              ? "bg-zinc-100/80 dark:bg-zinc-800/70"
                              : "hover:bg-zinc-50/80 dark:hover:bg-zinc-800/40"
                          }`}
                        >
                          <div className="flex min-w-0 items-center gap-2.5">
                            <button
                              type="button"
                              role="checkbox"
                              aria-checked={checked}
                              onClick={() => toggleSelectLog(log.id)}
                              className={`shrink-0 transition ${
                                checked || selectedLogIds.length > 0
                                  ? "opacity-100"
                                  : "opacity-45 group-hover:opacity-100 focus-visible:opacity-100"
                              }`}
                              aria-label={checked ? "取消选择该条日志" : "选择该条日志"}
                            >
                              {checked ? (
                                <CheckSquare className="h-3.5 w-3.5 text-zinc-900 dark:text-zinc-100" aria-hidden="true" />
                              ) : (
                                <Square className="h-3.5 w-3.5 text-zinc-500 dark:text-zinc-400" aria-hidden="true" />
                              )}
                            </button>
                            <ToolBrandIcon toolId={log.toolId} size="sm" decorative />
                            <span
                              className={`shrink-0 whitespace-nowrap rounded-md px-2 py-0.5 text-[11px] font-medium ${actionMeta.style}`}
                            >
                              {actionMeta.label}
                            </span>
                            <span
                              className={`shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[11px] font-medium ${
                                log.resultStatus === "success"
                                  ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                                  : log.resultStatus === "skipped"
                                    ? "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                                    : "bg-rose-500/12 text-rose-800 dark:text-rose-300"
                              }`}
                            >
                              {log.resultStatus === "success"
                                ? "成功"
                                : log.resultStatus === "skipped"
                                  ? log.errorCode === "DEVICE_DAILY_LIMIT_REACHED"
                                    ? "设备限额"
                                    : "已跳过"
                                  : "异常"}
                            </span>
                            <span className="shrink-0 whitespace-nowrap font-medium text-zinc-600 dark:text-zinc-400">
                              {toolName}
                            </span>
                            {log.errorCode && (
                              <span
                                className={`shrink-0 rounded px-1.5 py-0.5 font-mono text-[11px] ${
                                  log.resultStatus === "failed"
                                    ? "bg-rose-500/12 text-rose-700 dark:text-rose-300"
                                    : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                                }`}
                              >
                                {log.errorCode}
                              </span>
                            )}
                            <span
                              title={log.messageSanitized}
                              className="truncate text-zinc-800 dark:text-zinc-200"
                            >
                              {log.messageSanitized}
                            </span>
                          </div>

                          <div className="flex shrink-0 items-center gap-2">
                            <span
                              title={new Date(log.createdAt).toLocaleString("zh-CN")}
                              className="font-mono text-[11px] text-zinc-500 tabular-nums dark:text-zinc-400"
                            >
                              {new Date(log.createdAt).toLocaleTimeString("zh-CN", {
                                hour: "2-digit",
                                minute: "2-digit",
                                second: "2-digit",
                              })}
                            </span>
                            <div className="flex items-center gap-1 opacity-0 transition group-hover:opacity-100 focus-within:opacity-100">
                              {logScope === "active" ? (
                                <button
                                  type="button"
                                  title="归档此条日志"
                                  aria-label="归档此条日志"
                                  onClick={() =>
                                    void archiveLogs({ scope: "active", ids: [log.id] })
                                  }
                                  className="rounded-md p-1 text-zinc-500 transition hover:bg-zinc-200/70 hover:text-zinc-800 dark:text-zinc-400 dark:hover:bg-zinc-700 dark:hover:text-zinc-200"
                                >
                                  <Archive className="h-3.5 w-3.5" aria-hidden="true" />
                                </button>
                              ) : (
                                <button
                                  type="button"
                                  title="恢复到活跃日志"
                                  aria-label="恢复到活跃日志"
                                  onClick={() =>
                                    void unarchiveLogs({ scope: "archived", ids: [log.id] })
                                  }
                                  className="rounded-md p-1 text-zinc-500 transition hover:bg-zinc-200/70 hover:text-zinc-800 dark:text-zinc-400 dark:hover:bg-zinc-700 dark:hover:text-zinc-200"
                                >
                                  <ArchiveRestore className="h-3.5 w-3.5" aria-hidden="true" />
                                </button>
                              )}
                              <button
                                type="button"
                                title="永久删除此条日志"
                                aria-label="永久删除此条日志"
                                onClick={() =>
                                  setConfirmLogAction({
                                    label: "永久删除此条日志？",
                                    detail: "该操作不可撤销，日志将从本地库中永久移除。",
                                    danger: true,
                                    run: async () => {
                                      await deleteLogs({ scope: logScope, ids: [log.id] });
                                    },
                                  })
                                }
                                className="rounded-md p-1 text-zinc-500 transition hover:bg-rose-500/10 hover:text-rose-600 dark:text-zinc-400 dark:hover:text-rose-400"
                              >
                                <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                              </button>
                            </div>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </div>
              ))}
            </div>
          )}

          {hasMoreLogs && (
            <div className="flex items-center justify-center border-t border-zinc-100 bg-zinc-50/50 px-4 py-2.5 dark:border-zinc-800 dark:bg-zinc-950/30">
              <button
                type="button"
                disabled={loadingMoreLogs}
                onClick={async () => {
                  setLoadingMoreLogs(true);
                  await loadMoreLogs();
                  setLoadingMoreLogs(false);
                }}
                className="rounded-lg border border-zinc-200 bg-white px-3.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
              >
                {loadingMoreLogs
                  ? "加载中..."
                  : `加载更多历史日志（已显示 ${logs.length} / 共 ${currentLogTotal} 条）`}
              </button>
            </div>
          )}
        </div>
      </div>
  );
}
