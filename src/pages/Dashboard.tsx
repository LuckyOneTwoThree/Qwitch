import { useEffect, useMemo, useRef, useState } from "react";
import {
  RefreshCw,
  Sparkles,
  Plus,
  ShieldCheck,
  Layers,
  ScrollText,
  Settings,
  Lock,
  Puzzle,
  Moon,
  Sun,
  FolderSearch,
  Save,
  HardDriveDownload,
  Globe,
  Search,
  X,
  Activity,
  MonitorCheck,
  CheckCircle2,
  LayoutGrid,
  LayoutList,
  Archive,
  ArchiveRestore,
  Trash2,
  Download,
  Clock,
  CheckSquare,
  Square,
  Power,
  AppWindow,
  Timer,
} from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { AccountCard } from "../components/AccountCard";
import { BatchClaimBanner } from "../components/BatchClaimBanner";
import { ColdSwitchModal } from "../components/ColdSwitchModal";
import { ImportAccountModal } from "../components/ImportAccountModal";
import { UpdateCard } from "../components/UpdateCard";
import { ConfirmDialog } from "../components/ui/ConfirmDialog";
import { QwitchLogo, ToolBrandIcon } from "../components/ToolBrandIcon";
import { getToolBrandMeta, getVisibleTools } from "../domain/toolRegistry";
import {
  isAccountClaimable,
  isAccountNeedingAttention,
  selectScopeAccounts,
  selectScopedStats,
  type AccountFilter,
} from "../domain/selectors";
import type {
  AccountSummary,
  ActionLogItem,
  AuthMethod,
  ExternalAdapterInfo,
  ToolId,
} from "../types/qwitch";
import { invoke } from "@tauri-apps/api/core";
import packageJson from "../../package.json";

type NavSection = "all" | "logs" | "settings" | ToolId;
type AllViewMode = "grouped" | "flat";
type LogActionFilter = "all" | ActionLogItem["actionType"];
type LogResultFilter = "all" | ActionLogItem["resultStatus"];

const ACTION_TYPE_META: Record<
  ActionLogItem["actionType"],
  { label: string; style: string }
> = {
  claim: {
    label: "每日签到",
    style: "bg-sky-500/12 text-sky-800 dark:text-sky-300",
  },
  query: {
    label: "额度查询",
    style: "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
  },
  switch: {
    label: "桌面切号",
    style: "bg-indigo-500/12 text-indigo-800 dark:text-indigo-300",
  },
  import: {
    label: "账号管理",
    style: "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300",
  },
};

const formatLogDayLabel = (iso: string) => {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "未知日期";
  const now = new Date();
  const toKey = (d: Date) =>
    `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  const key = toKey(date);
  if (key === toKey(now)) return "今天";
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (key === toKey(yesterday)) return "昨天";
  return key;
};

function SettingToggleRow({
  id,
  label,
  description,
  checked,
  onChange,
}: {
  id: string;
  label: string;
  description: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-3 rounded-xl border border-zinc-100 bg-zinc-50/70 px-3 py-2.5 dark:border-zinc-800 dark:bg-zinc-800/40">
      <label htmlFor={id} className="min-w-0 flex-1 cursor-pointer">
        <div className="text-xs font-medium text-zinc-900 dark:text-zinc-100">{label}</div>
        <p className="mt-0.5 text-[11px] leading-snug text-zinc-600 dark:text-zinc-400">
          {description}
        </p>
      </label>
      <button
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`relative mt-0.5 inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors ${
          checked
            ? "bg-zinc-900 dark:bg-zinc-100"
            : "bg-zinc-300 dark:bg-zinc-700"
        }`}
      >
        <span
          className={`inline-block h-4 w-4 transform rounded-full shadow-xs transition-transform ${
            checked
              ? "translate-x-4 bg-white dark:bg-zinc-900"
              : "translate-x-0.5 bg-white dark:bg-zinc-200"
          }`}
        />
      </button>
    </div>
  );
}

export function Dashboard() {
  const {
    isInitializing,
    isTauriRuntime,
    accounts,
    logs,
    logScope,
    setLogScope,
    logStats,
    hasMoreLogs,
    loadMoreLogs,
    adapters,
    refreshAllQuotas,
    claimDailyBatch,
    batchProgress,
    appSettings,
    jitterRangeMs,
    setJitterRangeMs,
    logRetention,
    setLogRetention,
    customClientPaths,
    clientStatuses,
    saveAppSettings,
    refreshClientStatuses,
    archiveLogs,
    unarchiveLogs,
    deleteLogs,
    exportLogs,
    theme,
    toggleTheme,
    quitApp,
    showToast,
  } = useQwitch();

  const mainScrollRef = useRef<HTMLElement | null>(null);
  const [nav, setNav] = useState<NavSection>("all");
  const [importOpen, setImportOpen] = useState(false);
  const [importDefaultMethod, setImportDefaultMethod] = useState<AuthMethod | undefined>(undefined);
  const [importPresetTool, setImportPresetTool] = useState<ToolId | undefined>(undefined);
  const [bindTargetAccount, setBindTargetAccount] = useState<AccountSummary | null>(null);
  const [loadingMoreLogs, setLoadingMoreLogs] = useState(false);
  const [pathDrafts, setPathDrafts] = useState<Record<string, string>>({});
  const [pathDraftsDirty, setPathDraftsDirty] = useState(false);
  const [externalPlugins, setExternalPlugins] = useState<ExternalAdapterInfo[]>([]);
  const [trustedDraft, setTrustedDraft] = useState<string[]>([]);

  useEffect(() => {
    if (nav !== "settings" || !isTauriRuntime) return;
    let cancelled = false;
    invoke<ExternalAdapterInfo[]>("list_external_adapters")
      .then((list) => {
        if (cancelled) return;
        setExternalPlugins(list);
        setTrustedDraft(list.filter((p) => p.trusted).map((p) => p.id));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [nav, isTauriRuntime]);
  const [searchQuery, setSearchQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<AccountFilter>("all");
  const [viewMode, setViewMode] = useState<AllViewMode>("grouped");
  const [logActionFilter, setLogActionFilter] = useState<LogActionFilter>("all");
  const [logResultFilter, setLogResultFilter] = useState<LogResultFilter>("all");
  const [logSearch, setLogSearch] = useState("");
  const [selectedLogIds, setSelectedLogIds] = useState<number[]>([]);

  useEffect(() => {
    setSelectedLogIds((prev) => {
      const valid = prev.filter((id) => logs.some((l) => l.id === id));
      return valid.length === prev.length ? prev : valid;
    });
  }, [logs]);
  const [confirmLogAction, setConfirmLogAction] = useState<{
    label: string;
    detail: string;
    danger?: boolean;
    run: () => Promise<void>;
  } | null>(null);

  const visibleTools = useMemo(() => getVisibleTools(adapters), [adapters]);

  const activeToolFilter: ToolId | undefined =
    nav !== "all" && nav !== "logs" && nav !== "settings" ? nav : undefined;

  useEffect(() => {
    // 用户正在编辑时，后台同步不得覆盖未保存的路径草稿
    if (!pathDraftsDirty) {
      setPathDrafts(customClientPaths);
    }
  }, [customClientPaths, pathDraftsDirty]);

  useEffect(() => {
    void refreshClientStatuses();
  }, [refreshClientStatuses]);

  useEffect(() => {
    if (nav === "settings") {
      void refreshClientStatuses();
    }
  }, [nav, refreshClientStatuses]);

  const scopeAccounts = useMemo(
    () =>
      nav === "logs" || nav === "settings"
        ? []
        : selectScopeAccounts(accounts, activeToolFilter),
    [accounts, activeToolFilter, nav]
  );

  const scopedStats = useMemo(
    () => selectScopedStats(scopeAccounts, adapters, clientStatuses, activeToolFilter),
    [scopeAccounts, adapters, clientStatuses, activeToolFilter]
  );

  const filteredAccounts = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return scopeAccounts.filter((acc) => {
      if (statusFilter === "active_client" && !acc.isActiveOnClient) return false;
      if (statusFilter === "claimable" && !isAccountClaimable(acc, adapters)) {
        return false;
      }
      if (statusFilter === "attention" && !isAccountNeedingAttention(acc)) {
        return false;
      }
      if (!q) return true;
      const haystack = [
        acc.label,
        acc.uidMasked,
        acc.emailMasked ?? "",
        acc.latestQuota?.planName ?? "",
        getToolBrandMeta(acc.toolId, adapters).name,
      ]
        .join(" ")
        .toLowerCase();
      return haystack.includes(q);
    });
  }, [scopeAccounts, searchQuery, statusFilter, adapters]);

  const groupedModules = useMemo(() => {
    const isFiltering = Boolean(searchQuery.trim()) || statusFilter !== "all";
    const knownToolIds = new Set<string>(visibleTools.map((t) => t.id));
    const extraToolIds = Array.from(
      new Set(filteredAccounts.map((a) => a.toolId).filter((id) => !knownToolIds.has(id)))
    );

    const list = [
      ...visibleTools.map((t) => ({ id: t.id, name: t.name })),
      ...extraToolIds.map((id) => ({ id, name: getToolBrandMeta(id, adapters).name })),
    ];

    return list
      .map((tool) => {
        const toolAccounts = filteredAccounts.filter((a) => a.toolId === tool.id);
        const activeAccount = toolAccounts.find((a) => a.isActiveOnClient);
        return {
          ...tool,
          accounts: toolAccounts,
          activeAccount,
          isRunning: Boolean(clientStatuses[tool.id]?.isRunning),
        };
      })
      .filter((group) =>
        isFiltering
          ? group.accounts.length > 0
          : group.accounts.length > 0 || accounts.length === 0
      );
  }, [filteredAccounts, searchQuery, statusFilter, clientStatuses, accounts.length, visibleTools, adapters]);

  useEffect(() => {
    setSelectedLogIds([]);
    setConfirmLogAction(null);
    mainScrollRef.current?.scrollTo({ top: 0 });
  }, [logScope, nav]);

  const filteredLogs = useMemo(() => {
    const q = logSearch.trim().toLowerCase();
    return logs.filter((log) => {
      if (logActionFilter !== "all" && log.actionType !== logActionFilter) return false;
      if (logResultFilter !== "all" && log.resultStatus !== logResultFilter) return false;
      if (!q) return true;
      const toolName = getToolBrandMeta(log.toolId, adapters).name;
      return [log.messageSanitized, toolName, log.toolId, log.errorCode ?? ""]
        .join(" ")
        .toLowerCase()
        .includes(q);
    });
  }, [logs, logActionFilter, logResultFilter, logSearch, adapters]);

  const groupedLogs = useMemo(() => {
    const groups: Array<{ day: string; items: ActionLogItem[] }> = [];
    for (const log of filteredLogs) {
      const day = formatLogDayLabel(log.createdAt);
      const last = groups[groups.length - 1];
      if (last && last.day === day) {
        last.items.push(log);
      } else {
        groups.push({ day, items: [log] });
      }
    }
    return groups;
  }, [filteredLogs]);

  const visibleLogIds = useMemo(() => filteredLogs.map((l) => l.id), [filteredLogs]);
  const allVisibleLogsSelected =
    visibleLogIds.length > 0 && visibleLogIds.every((id) => selectedLogIds.includes(id));

  const toggleSelectLog = (id: number) => {
    setSelectedLogIds((prev) =>
      prev.includes(id) ? prev.filter((item) => item !== id) : [...prev, id]
    );
  };

  const toggleSelectAllVisibleLogs = () => {
    if (allVisibleLogsSelected) {
      setSelectedLogIds([]);
    } else {
      setSelectedLogIds(visibleLogIds);
    }
  };

  const openImportModal = (defaultMethod?: AuthMethod, toolId?: ToolId) => {
    setBindTargetAccount(null);
    setImportDefaultMethod(defaultMethod);
    setImportPresetTool(toolId ?? activeToolFilter);
    setImportOpen(true);
  };

  const activeToolMeta = activeToolFilter
    ? getToolBrandMeta(activeToolFilter, adapters)
    : undefined;
  const activeToolSupportsClaim = activeToolFilter
    ? (adapters.find((a) => a.id === activeToolFilter)?.capabilities.supportsDailyClaim ?? false)
    : true;
  const isAccountSection = nav !== "logs" && nav !== "settings";
  const isBatchBusy = Boolean(batchProgress?.running);

  const headerTitle = useMemo(() => {
    if (nav === "logs") return "操作审计日志";
    if (nav === "settings") return "系统设置与客户端环境";
    if (activeToolMeta) {
      return `${activeToolMeta.name} 账号管理`;
    }
    return "账号与额度总览";
  }, [nav, activeToolMeta]);

  const headerSubtitle = useMemo(() => {
    if (nav === "logs") {
      return `活跃 ${logStats.activeCount} 条 · 已归档 ${logStats.archivedCount} 条${
        logStats.noiseActiveCount > 0
          ? ` · 可清理日常冗余 ${logStats.noiseActiveCount} 条`
          : ""
      }`;
    }
    if (nav === "settings") {
      return `已加载 ${adapters.length} 个工具适配器 · ${scopedStats.runningClientsCount} 个桌面客户端运行中`;
    }
    if (activeToolMeta) {
      return `${activeToolMeta.tagline} · 当前纳管 ${scopedStats.total} 个账号 · ${scopedStats.claimBadgeText}`;
    }
    return `共 ${scopedStats.total} 个纳管账号 · 今日已签 ${scopedStats.claimedCount} 个 · ${scopedStats.claimBadgeText}`;
  }, [nav, logStats, adapters.length, scopedStats, activeToolMeta]);

  const isFiltered = Boolean(searchQuery.trim()) || statusFilter !== "all";
  const currentLogTotal =
    logScope === "archived" ? logStats.archivedCount : logStats.activeCount;

  return (
    <div className="flex h-full overflow-hidden bg-zinc-50 text-zinc-900 dark:bg-zinc-950 dark:text-zinc-100">
      <aside className="flex h-full w-54 shrink-0 flex-col justify-between border-r border-zinc-200 bg-white/85 p-3.5 dark:border-zinc-800 dark:bg-zinc-900/65">
        <div className="overflow-y-auto pr-0.5">
          <div className="mb-5 flex items-center gap-2.5 px-1.5">
            <QwitchLogo size="md" />
            <div className="min-w-0">
              <div className="text-sm font-semibold tracking-tight">Qwitch</div>
              <div className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                AI 账号与额度工作台
              </div>
            </div>
          </div>

          <div className="space-y-4">
            <div>
              <div className="mb-1.5 px-2.5 text-[11px] font-semibold tracking-wider text-zinc-500 uppercase dark:text-zinc-400">
                工作台视图
              </div>
              <nav aria-label="账号视图导航" className="space-y-1">
                <button
                  type="button"
                  onClick={() => setNav("all")}
                  className={`flex w-full items-center justify-between rounded-xl px-2.5 py-2 text-xs font-medium transition ${
                    nav === "all"
                      ? "bg-zinc-900 text-white shadow-2xs dark:bg-zinc-100 dark:text-zinc-900"
                      : "text-zinc-700 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <Layers className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    全部账号
                  </span>
                  <span
                    className={`rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                      nav === "all"
                        ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                        : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {accounts.length}
                  </span>
                </button>

                {visibleTools.map((tool) => {
                  const active = nav === tool.id;
                  const count = accounts.filter((a) => a.toolId === tool.id).length;
                  const isRunning = clientStatuses[tool.id]?.isRunning;
                  return (
                    <button
                      key={tool.id}
                      type="button"
                      onClick={() => setNav(tool.id)}
                      className={`flex w-full items-center justify-between rounded-xl px-2.5 py-2 text-xs font-medium transition ${
                        active
                          ? "bg-zinc-900 text-white shadow-2xs dark:bg-zinc-100 dark:text-zinc-900"
                          : "text-zinc-700 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                      }`}
                    >
                      <span className="flex min-w-0 items-center gap-2">
                        <ToolBrandIcon toolId={tool.id} size="xs" decorative />
                        <span className="truncate">{tool.name}</span>
                        {isRunning && (
                          <span
                            title="本机客户端运行中"
                            className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-500"
                          />
                        )}
                      </span>
                      <span
                        className={`rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                          active
                            ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                            : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                        }`}
                      >
                        {count}
                      </span>
                    </button>
                  );
                })}
              </nav>
            </div>

            <div>
              <div className="mb-1.5 px-2.5 text-[11px] font-semibold tracking-wider text-zinc-500 uppercase dark:text-zinc-400">
                系统与审计
              </div>
              <nav aria-label="系统管理导航" className="space-y-1">
                <button
                  type="button"
                  onClick={() => setNav("logs")}
                  className={`flex w-full items-center justify-between rounded-xl px-2.5 py-2 text-xs font-medium transition ${
                    nav === "logs"
                      ? "bg-zinc-900 text-white shadow-2xs dark:bg-zinc-100 dark:text-zinc-900"
                      : "text-zinc-700 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <ScrollText className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    操作日志
                  </span>
                  <span
                    className={`rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                      nav === "logs"
                        ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                        : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {logStats.activeCount}
                  </span>
                </button>

                <button
                  type="button"
                  onClick={() => setNav("settings")}
                  className={`flex w-full items-center justify-between rounded-xl px-2.5 py-2 text-xs font-medium transition ${
                    nav === "settings"
                      ? "bg-zinc-900 text-white shadow-2xs dark:bg-zinc-100 dark:text-zinc-900"
                      : "text-zinc-700 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <Settings className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    设置与环境
                  </span>
                </button>
              </nav>
            </div>
          </div>
        </div>

        <div className="space-y-2">
          <div className="rounded-xl border border-zinc-200 bg-zinc-50 p-2.5 text-[11px] text-zinc-600 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-400">
            <div className="flex items-center justify-between gap-2">
              <span className="flex items-center gap-1.5 font-medium text-zinc-800 dark:text-zinc-200">
                <Lock className="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                Local-First 加密库
              </span>
              <span className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                v{packageJson.version}
              </span>
            </div>
            <p className="mt-1 leading-snug">
              凭据经系统钥匙串与 AES-256-GCM 加密落盘；
              {appSettings.closeToTray !== false
                ? "关闭主窗将隐藏窗口并仅留菜单栏常驻。"
                : "关闭主窗将直接退出程序。"}
            </p>
          </div>

          <button
            type="button"
            onClick={() => void quitApp()}
            className="flex w-full items-center justify-center gap-1.5 rounded-xl border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
          >
            <Power className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            退出 Qwitch
          </button>
        </div>
      </aside>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <header className="z-20 flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-zinc-200 bg-white/90 px-5 py-3 backdrop-blur dark:border-zinc-800 dark:bg-zinc-900/80">
          <div className="flex min-w-0 items-center gap-2.5">
            {activeToolFilter && <ToolBrandIcon toolId={activeToolFilter} size="sm" decorative />}
            <div className="min-w-0">
              <h1 className="truncate text-sm font-semibold tracking-tight">{headerTitle}</h1>
              <p className="truncate text-[11px] text-zinc-600 dark:text-zinc-400">
                {headerSubtitle}
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            {isAccountSection && (
              <>
                {scopeAccounts.length > 0 && (
                  <button
                    type="button"
                    onClick={() => void refreshAllQuotas(activeToolFilter)}
                    disabled={isBatchBusy}
                    className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                  >
                    <RefreshCw
                      className={`h-3.5 w-3.5 shrink-0 ${
                        isBatchBusy && batchProgress?.action === "refresh" ? "animate-spin" : ""
                      }`}
                      aria-hidden="true"
                    />
                    {activeToolMeta ? `刷新 ${activeToolMeta.name}` : "刷新全部额度"}
                  </button>
                )}

                {scopeAccounts.length > 0 && activeToolSupportsClaim && (
                  <>
                    {scopedStats.claimActionState === "all_claimed" ? (
                      <span
                        role="status"
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg border border-emerald-500/30 bg-emerald-500/12 px-2.5 py-1.5 text-xs font-medium text-emerald-800 dark:text-emerald-300"
                      >
                        <CheckCircle2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {scopedStats.unclaimedAuthMissingCount > 0
                          ? `可签已全领 (${scopedStats.claimedCount})`
                          : `今日已全签 (${scopedStats.claimedCount})`}
                      </span>
                    ) : scopedStats.claimActionState === "claimable" ? (
                      <button
                        type="button"
                        onClick={() => void claimDailyBatch(undefined, activeToolFilter)}
                        disabled={isBatchBusy}
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:opacity-50 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                      >
                        <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {activeToolMeta
                          ? `领取 ${activeToolMeta.name} (${scopedStats.claimableCount})`
                          : `一键领取 (${scopedStats.claimableCount})`}
                      </button>
                    ) : scopedStats.claimActionState === "verify_and_claim" ? (
                      <button
                        type="button"
                        onClick={() => void claimDailyBatch(undefined, activeToolFilter, true)}
                        disabled={isBatchBusy}
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:opacity-50 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                      >
                        <ShieldCheck className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {activeToolMeta
                          ? `核实并领 ${activeToolMeta.name} (${scopedStats.unknownClaimCount})`
                          : `核实并领取 (${scopedStats.unknownClaimCount})`}
                      </button>
                    ) : null}
                  </>
                )}

                <button
                  type="button"
                  onClick={() => openImportModal()}
                  className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                >
                  <Plus className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  导入账号
                </button>
              </>
            )}

            <button
              type="button"
              onClick={toggleTheme}
              title="切换深色/浅色主题"
              aria-label="切换深色或浅色主题"
              className="shrink-0 rounded-lg border border-zinc-200 bg-white p-1.5 text-zinc-600 transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700"
            >
              {theme === "dark" ? (
                <Sun className="h-3.5 w-3.5" aria-hidden="true" />
              ) : (
                <Moon className="h-3.5 w-3.5" aria-hidden="true" />
              )}
            </button>
          </div>
        </header>

        {isAccountSection && (
          <div className="shrink-0 space-y-2.5 border-b border-zinc-200/80 bg-zinc-50/95 px-5 py-3 dark:border-zinc-800 dark:bg-zinc-950/95">
            <BatchClaimBanner />

            <section
              aria-label="当前作用域核心指标"
              className="grid grid-cols-2 gap-2.5 sm:grid-cols-4"
            >
              <button
                type="button"
                onClick={() => setStatusFilter("all")}
                aria-pressed={statusFilter === "all"}
                className="flex min-w-0 flex-col justify-between rounded-xl border border-zinc-200 bg-white px-3 py-2 text-left transition hover:border-zinc-300 dark:border-zinc-800 dark:bg-zinc-900/90 dark:hover:border-zinc-700"
              >
                <div className="flex w-full items-center justify-between gap-1.5">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-md bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
                      <Layers className="h-3 w-3" aria-hidden="true" />
                    </span>
                    <span className="truncate text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
                      {activeToolMeta ? `${activeToolMeta.name} 账号` : "纳管账号"}
                    </span>
                  </div>
                  {!activeToolFilter && (
                    <div className="hidden shrink-0 items-center gap-1 xl:flex">
                      {visibleTools.slice(0, 3).map((t) => (
                        <span
                          key={t.id}
                          title={`${t.name}: ${accounts.filter((a) => a.toolId === t.id).length}`}
                          className="inline-flex items-center gap-0.5 rounded bg-zinc-100 px-1 py-0.5 text-[11px] text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                        >
                          <ToolBrandIcon toolId={t.id} size="xs" decorative />
                          <span className="tabular-nums">
                            {accounts.filter((a) => a.toolId === t.id).length}
                          </span>
                        </span>
                      ))}
                    </div>
                  )}
                </div>

                <div className="mt-1.5 flex w-full items-baseline justify-between gap-1.5">
                  <div className="flex items-baseline gap-1">
                    <span className="text-lg leading-none font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                      {scopedStats.total}
                    </span>
                    <span className="text-[11px] text-zinc-600 dark:text-zinc-400">个</span>
                  </div>
                  <span className="truncate text-[11px] text-zinc-600 dark:text-zinc-400">
                    {statusFilter === "all" ? "当前总览" : "点击看全部"}
                  </span>
                </div>
              </button>

              <button
                type="button"
                onClick={() =>
                  setStatusFilter((prev) => (prev === "active_client" ? "all" : "active_client"))
                }
                aria-pressed={statusFilter === "active_client"}
                className={`flex min-w-0 flex-col justify-between rounded-xl border bg-white px-3 py-2 text-left transition dark:bg-zinc-900/90 ${
                  statusFilter === "active_client"
                    ? "border-emerald-500/60 ring-1 ring-emerald-500/20 dark:border-emerald-400/50"
                    : "border-zinc-200 hover:border-zinc-300 dark:border-zinc-800 dark:hover:border-zinc-700"
                }`}
              >
                <div className="flex w-full items-center justify-between gap-1.5">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-md bg-emerald-500/12 text-emerald-700 dark:text-emerald-400">
                      <MonitorCheck className="h-3 w-3" aria-hidden="true" />
                    </span>
                    <span className="truncate text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
                      本机挂载
                    </span>
                  </div>
                  <span
                    className={`shrink-0 whitespace-nowrap rounded-full px-1.5 py-0.5 text-[11px] font-medium ${
                      scopedStats.runningClientsCount > 0
                        ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                        : "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {scopedStats.runningClientsCount > 0
                      ? activeToolFilter
                        ? "运行中"
                        : `${scopedStats.runningClientsCount} 运行`
                      : "未启动"}
                  </span>
                </div>

                <div className="mt-1.5 flex w-full items-baseline justify-between gap-1.5">
                  <div className="flex items-baseline">
                    <span className="text-lg leading-none font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                      {scopedStats.activeClientsCount}
                    </span>
                    <span className="ml-0.5 text-[11px] text-zinc-600 dark:text-zinc-400">
                      /{scopedStats.scopeToolCapacity}
                    </span>
                  </div>
                  <span className="truncate text-[11px] text-zinc-600 dark:text-zinc-400">
                    {statusFilter === "active_client" ? "已筛选" : "使用中"}
                  </span>
                </div>
              </button>

              <button
                type="button"
                onClick={() =>
                  setStatusFilter((prev) => (prev === "claimable" ? "all" : "claimable"))
                }
                aria-pressed={statusFilter === "claimable"}
                className={`flex min-w-0 flex-col justify-between rounded-xl border bg-white px-3 py-2 text-left transition dark:bg-zinc-900/90 ${
                  statusFilter === "claimable"
                    ? "border-sky-500/60 ring-1 ring-sky-500/20 dark:border-sky-400/50"
                    : "border-zinc-200 hover:border-zinc-300 dark:border-zinc-800 dark:hover:border-zinc-700"
                }`}
              >
                <div className="flex w-full items-center justify-between gap-1.5">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-md bg-sky-500/12 text-sky-700 dark:text-sky-400">
                      <CheckCircle2 className="h-3 w-3" aria-hidden="true" />
                    </span>
                    <span className="truncate text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
                      今日签到
                    </span>
                  </div>
                  <span
                    className={`shrink-0 whitespace-nowrap rounded-full px-1.5 py-0.5 text-[11px] font-medium ${
                      scopedStats.claimableCount > 0
                        ? "bg-sky-500/12 text-sky-800 dark:text-sky-300"
                        : scopedStats.unclaimedAuthMissingCount > 0
                          ? "bg-amber-500/12 text-amber-800 dark:text-amber-300"
                          : scopedStats.allClaimed
                            ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                            : "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {scopedStats.claimBadgeText}
                  </span>
                </div>

                <div className="mt-1.5 flex w-full items-baseline justify-between gap-1.5">
                  <div className="flex items-baseline gap-1">
                    <span className="text-lg leading-none font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                      {scopedStats.claimedCount}
                    </span>
                    <span className="text-[11px] text-zinc-600 dark:text-zinc-400">已签</span>
                  </div>
                  <span className="truncate text-[11px] tabular-nums text-zinc-600 dark:text-zinc-400">
                    {scopedStats.eligibleClaimTotal > 0
                      ? `共 ${scopedStats.eligibleClaimTotal} 可签`
                      : "仅查额/切号"}
                  </span>
                </div>
              </button>

              <button
                type="button"
                onClick={() =>
                  setStatusFilter((prev) => (prev === "attention" ? "all" : "attention"))
                }
                aria-pressed={statusFilter === "attention"}
                className={`flex min-w-0 flex-col justify-between rounded-xl border bg-white px-3 py-2 text-left transition dark:bg-zinc-900/90 ${
                  statusFilter === "attention"
                    ? "border-amber-500/60 ring-1 ring-amber-500/20 dark:border-amber-400/50"
                    : "border-zinc-200 hover:border-zinc-300 dark:border-zinc-800 dark:hover:border-zinc-700"
                }`}
              >
                <div className="flex w-full items-center justify-between gap-1.5">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <span
                      className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-md ${
                        scopedStats.attentionCount > 0
                          ? "bg-amber-500/12 text-amber-700 dark:text-amber-400"
                          : "bg-emerald-500/12 text-emerald-700 dark:text-emerald-400"
                      }`}
                    >
                      <Activity className="h-3 w-3" aria-hidden="true" />
                    </span>
                    <span className="truncate text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
                      凭据健康
                    </span>
                  </div>
                  <span
                    className={`shrink-0 whitespace-nowrap rounded-full px-1.5 py-0.5 text-[11px] font-medium ${
                      scopedStats.attentionCount > 0
                        ? "bg-amber-500/12 text-amber-800 dark:text-amber-300"
                        : "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                    }`}
                  >
                    {scopedStats.attentionCount > 0
                      ? `${scopedStats.attentionCount} 待修`
                      : scopedStats.disabledCount > 0
                        ? `${scopedStats.disabledCount} 停用`
                        : "完好"}
                  </span>
                </div>

                <div className="mt-1.5 flex w-full items-baseline justify-between gap-1.5">
                  <div className="flex items-baseline gap-1">
                    <span className="text-lg leading-none font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                      {scopedStats.healthyCount}
                    </span>
                    <span className="text-[11px] text-zinc-600 dark:text-zinc-400">正常</span>
                  </div>
                  <span className="truncate text-[11px] text-zinc-600 dark:text-zinc-400">
                    {scopedStats.attentionCount > 0 ? "点击筛选" : "状态完好"}
                  </span>
                </div>
              </button>
            </section>

            {scopeAccounts.length > 0 && (
              <div className="flex flex-wrap items-center justify-between gap-2.5 rounded-xl border border-zinc-200 bg-white px-3.5 py-2 dark:border-zinc-800 dark:bg-zinc-900/90">
                <div className="relative min-w-[190px] flex-1 max-w-xs">
                  <Search
                    className="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-zinc-500 dark:text-zinc-400"
                    aria-hidden="true"
                  />
                  <input
                    type="text"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    placeholder="搜索昵称、UID、手机号或套餐..."
                    aria-label="搜索账号"
                    className="w-full rounded-lg border border-zinc-200 bg-zinc-50 py-1 pr-7 pl-8 text-xs text-zinc-900 transition focus:border-zinc-400 focus:bg-white dark:border-zinc-700 dark:bg-zinc-800/70 dark:text-zinc-100 dark:focus:bg-zinc-800"
                  />
                  {searchQuery && (
                    <button
                      type="button"
                      onClick={() => setSearchQuery("")}
                      aria-label="清空搜索"
                      className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-0.5 text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-200"
                    >
                      <X className="h-3 w-3" aria-hidden="true" />
                    </button>
                  )}
                </div>

                <div className="flex flex-wrap items-center gap-2">
                  <div className="flex flex-wrap items-center gap-1">
                    {(
                      [
                        { id: "all", label: "全部" },
                        { id: "active_client", label: "使用中" },
                        { id: "claimable", label: "待领" },
                        { id: "attention", label: "待完善" },
                      ] as const
                    ).map((f) => {
                      const active = statusFilter === f.id;
                      const cnt = scopedStats.filterCounts[f.id];
                      return (
                        <button
                          key={f.id}
                          type="button"
                          onClick={() => setStatusFilter(f.id)}
                          className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg px-2 py-1 text-xs font-medium transition ${
                            active
                              ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                              : "bg-zinc-100 text-zinc-700 hover:bg-zinc-200/70 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700/70"
                          }`}
                        >
                          <span>{f.label}</span>
                          <span
                            className={`text-[11px] tabular-nums ${
                              active ? "opacity-80" : "text-zinc-500 dark:text-zinc-400"
                            }`}
                          >
                            {cnt}
                          </span>
                        </button>
                      );
                    })}
                  </div>

                  {nav === "all" && (
                    <div className="inline-flex shrink-0 rounded-lg border border-zinc-200 bg-zinc-50 p-0.5 dark:border-zinc-700 dark:bg-zinc-800/70">
                      <button
                        type="button"
                        onClick={() => setViewMode("grouped")}
                        title="按平台模块化分类展示"
                        className={`inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-[11px] font-medium transition ${
                          viewMode === "grouped"
                            ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                            : "text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
                        }`}
                      >
                        <LayoutList className="h-3 w-3 shrink-0" aria-hidden="true" />
                        分组
                      </button>
                      <button
                        type="button"
                        onClick={() => setViewMode("flat")}
                        title="紧凑平铺展示"
                        className={`inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-[11px] font-medium transition ${
                          viewMode === "flat"
                            ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                            : "text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
                        }`}
                      >
                        <LayoutGrid className="h-3 w-3 shrink-0" aria-hidden="true" />
                        平铺
                      </button>
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>
        )}

        {nav === "logs" && (
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
                          run: async () => {
                            await archiveLogs({ scope: "active" });
                          },
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
                          run: async () => {
                            await deleteLogs({ scope: "active" });
                          },
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
                          run: async () => {
                            await deleteLogs({ scope: "archived" });
                          },
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
                              setSelectedLogIds([]);
                              await archiveLogs({ scope: "active", ids });
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
                              setSelectedLogIds([]);
                              await unarchiveLogs({ scope: "archived", ids });
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
                                setSelectedLogIds([]);
                                await deleteLogs({ scope: logScope, ids });
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
        )}

        {nav === "settings" && batchProgress && (
          <div className="shrink-0 px-5 pt-3.5">
            <BatchClaimBanner />
          </div>
        )}

        <main ref={mainScrollRef} className="min-h-0 flex-1 overflow-y-auto px-5 py-3.5">
          {isAccountSection && (
            <>

              {isInitializing && scopeAccounts.length === 0 ? (
                <div
                  aria-label="正在加载本地账号库"
                  className="grid grid-cols-1 gap-3.5 md:grid-cols-2 2xl:grid-cols-3"
                >
                  {[0, 1, 2].map((idx) => (
                    <div
                      key={idx}
                      className="h-44 animate-pulse rounded-2xl border border-zinc-200 bg-white p-4 dark:border-zinc-800 dark:bg-zinc-900"
                    >
                      <div className="flex items-center gap-3">
                        <div className="h-9 w-9 rounded-xl bg-zinc-200 dark:bg-zinc-800" />
                        <div className="flex-1 space-y-2">
                          <div className="h-3 w-24 rounded bg-zinc-200 dark:bg-zinc-800" />
                          <div className="h-4 w-36 rounded bg-zinc-200 dark:bg-zinc-800" />
                        </div>
                      </div>
                      <div className="mt-6 h-2 w-full rounded-full bg-zinc-100 dark:bg-zinc-800" />
                      <div className="mt-6 flex justify-between">
                        <div className="h-6 w-24 rounded-lg bg-zinc-100 dark:bg-zinc-800" />
                        <div className="h-6 w-20 rounded-lg bg-zinc-200 dark:bg-zinc-800" />
                      </div>
                    </div>
                  ))}
                </div>
              ) : scopeAccounts.length === 0 ? (
                <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-200 bg-white/75 px-6 py-14 text-center dark:border-zinc-800 dark:bg-zinc-900/40">
                  {activeToolFilter ? (
                    <ToolBrandIcon toolId={activeToolFilter} size="lg" decorative />
                  ) : (
                    <div className="flex items-center gap-2">
                      {visibleTools.slice(0, 4).map((t) => (
                        <ToolBrandIcon key={t.id} toolId={t.id} size="md" decorative />
                      ))}
                    </div>
                  )}
                  <h2 className="mt-4 text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                    {activeToolMeta
                      ? `尚未导入 ${activeToolMeta.name} 账号`
                      : "尚未导入任何 AI 工具账号"}
                  </h2>
                  <p className="mt-1 max-w-md text-xs leading-relaxed text-zinc-600 dark:text-zinc-400">
                    可从本机已登录桌面客户端一键导入当前账号，或通过安全隔离的内置授权窗口登录新账号并同步最新额度。
                  </p>
                  <div className="mt-5 flex flex-wrap items-center justify-center gap-2.5">
                    <button
                      type="button"
                      onClick={() => openImportModal("local_import")}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-xl bg-zinc-900 px-4 py-2 text-xs font-medium text-white transition hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                    >
                      <HardDriveDownload className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      本机一键导入
                    </button>
                    <button
                      type="button"
                      onClick={() => openImportModal("webview_oauth")}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-xl border border-zinc-200 bg-white px-4 py-2 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                    >
                      <Globe className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      内置授权登录
                    </button>
                  </div>
                </div>
              ) : filteredAccounts.length === 0 ? (
                <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-200 bg-white/60 px-6 py-12 text-center dark:border-zinc-800 dark:bg-zinc-900/30">
                  <Search className="h-6 w-6 text-zinc-400 dark:text-zinc-600" aria-hidden="true" />
                  <h2 className="mt-3 text-sm font-medium text-zinc-800 dark:text-zinc-200">
                    当前筛选条件下暂无匹配账号
                  </h2>
                  <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                    尝试更换搜索关键词，或重置状态过滤条件查看全部账号。
                  </p>
                  <button
                    type="button"
                    onClick={() => {
                      setSearchQuery("");
                      setStatusFilter("all");
                    }}
                    className="mt-4 rounded-lg border border-zinc-200 bg-white px-3.5 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                  >
                    清除筛选条件
                  </button>
                </div>
              ) : nav === "all" && viewMode === "grouped" ? (
                <div className="space-y-4">
                  {groupedModules.map((group) => (
                    <section
                      key={group.id}
                      className="rounded-2xl border border-zinc-200/80 bg-zinc-100/45 p-3.5 dark:border-zinc-800/90 dark:bg-zinc-900/40"
                    >
                      <div className="mb-3 flex flex-wrap items-center justify-between gap-2 px-0.5">
                        <div className="flex flex-wrap items-center gap-2">
                          <ToolBrandIcon toolId={group.id} size="sm" decorative />
                          <h2 className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                            {group.name}
                          </h2>
                          <span className="rounded-full bg-zinc-200/80 px-2 py-0.5 text-[11px] font-medium tabular-nums text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
                            {group.accounts.length} 个账号
                          </span>
                          <span
                            className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium ${
                              group.isRunning
                                ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                                : "bg-zinc-200/60 text-zinc-600 dark:bg-zinc-800/80 dark:text-zinc-400"
                            }`}
                          >
                            {group.isRunning && (
                              <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                            )}
                            {group.isRunning ? "桌面端运行中" : "桌面端未运行"}
                          </span>
                          {group.activeAccount && (
                            <span className="hidden text-[11px] text-zinc-600 sm:inline dark:text-zinc-400">
                              · 当前挂载：{group.activeAccount.label}
                            </span>
                          )}
                        </div>

                        <button
                          type="button"
                          onClick={() => openImportModal("webview_oauth", group.id)}
                          className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1 text-[11px] font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                        >
                          <Plus className="h-3 w-3 shrink-0" aria-hidden="true" />
                          添加账号
                        </button>
                      </div>

                      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-3">
                        {group.accounts.map((acc) => (
                          <AccountCard key={acc.id} account={acc} onOpenBindModal={setBindTargetAccount} />
                        ))}

                        {group.accounts.length === 1 && !isFiltered && (
                          <button
                            type="button"
                            onClick={() => openImportModal("webview_oauth", group.id)}
                            className="group flex min-h-[168px] flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-300/80 bg-white/50 p-4 text-center transition hover:border-zinc-400 hover:bg-white dark:border-zinc-700/80 dark:bg-zinc-900/40 dark:hover:border-zinc-600 dark:hover:bg-zinc-900/70"
                          >
                            <div className="flex h-9 w-9 items-center justify-center rounded-xl border border-zinc-200 bg-white text-zinc-500 shadow-2xs transition group-hover:scale-105 group-hover:text-zinc-800 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-400 dark:group-hover:text-zinc-200">
                              <Plus className="h-4 w-4" aria-hidden="true" />
                            </div>
                            <span className="mt-2.5 text-xs font-medium text-zinc-700 dark:text-zinc-300">
                              添加第 2 个 {group.name} 账号
                            </span>
                            <span className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                              新增副号后可在本模块内一键无缝切换桌面端登录态
                            </span>
                          </button>
                        )}
                      </div>
                    </section>
                  ))}
                </div>
              ) : (
                <div className="grid grid-cols-1 gap-3.5 md:grid-cols-2 2xl:grid-cols-3">
                  {filteredAccounts.map((acc) => (
                    <AccountCard key={acc.id} account={acc} onOpenBindModal={setBindTargetAccount} />
                  ))}
                </div>
              )}
            </>
          )}

          {nav === "logs" && (
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
                            const checked = selectedLogIds.includes(log.id);
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
          )}

          {nav === "settings" && (
            <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
              <div className="rounded-2xl border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2 text-sm font-semibold">
                    <AppWindow className="h-4 w-4 text-sky-500" aria-hidden="true" />
                    桌面常驻与启动偏好
                  </div>
                  <span className="rounded-full bg-zinc-100 px-2 py-0.5 text-[11px] text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400">
                    即时生效
                  </span>
                </div>
                <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                  控制关闭主窗口时的常驻行为及系统登录自动启动策略。
                </p>

                <div className="mt-4 space-y-2.5">
                  <div className="rounded-xl border border-zinc-100 bg-zinc-50/70 px-3 py-2.5 dark:border-zinc-800 dark:bg-zinc-800/40">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div>
                        <div className="text-xs font-medium text-zinc-900 dark:text-zinc-100">
                          外观主题
                        </div>
                        <p className="mt-0.5 text-[11px] text-zinc-600 dark:text-zinc-400">
                          支持始终浅色、深色或随系统外观自动切换。
                        </p>
                      </div>
                      <div className="flex flex-wrap items-center gap-1">
                        {[
                          { label: "跟随系统", value: "system" as const },
                          { label: "浅色", value: "light" as const },
                          { label: "深色", value: "dark" as const },
                        ].map((opt) => {
                          const selected = appSettings.theme === opt.value;
                          return (
                            <button
                              key={opt.value}
                              type="button"
                              aria-pressed={selected}
                              onClick={() => void saveAppSettings({ theme: opt.value }, true)}
                              className={`rounded-lg px-2.5 py-1 text-[11px] font-medium transition ${
                                selected
                                  ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                                  : "border border-zinc-200 bg-white text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800"
                              }`}
                            >
                              {opt.label}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  </div>
                  <SettingToggleRow
                    id="qwitch-close-to-tray"
                    label="关闭主窗口时隐藏至菜单栏常驻"
                    description="开启后点击关闭按钮将隐藏主窗口并收起程序坞/任务栏图标，仅保留顶部菜单栏常驻；关闭则直接退出 Qwitch。"
                    checked={appSettings.closeToTray !== false}
                    onChange={(next) => void saveAppSettings({ closeToTray: next })}
                  />
                  <SettingToggleRow
                    id="qwitch-launch-at-login"
                    label="开机自动启动"
                    description="登录系统时自动启动 Qwitch（macOS 写入 ~/Library/LaunchAgents，启动后静默驻留）。"
                    checked={Boolean(appSettings.launchAtLogin)}
                    onChange={(next) => void saveAppSettings({ launchAtLogin: next })}
                  />
                  <SettingToggleRow
                    id="qwitch-start-minimized"
                    label="启动时静默驻留菜单栏"
                    description="手动打开应用时默认不弹出主窗口，直接隐身于菜单栏，减少对桌面的打扰。"
                    checked={Boolean(appSettings.startMinimized)}
                    onChange={(next) => void saveAppSettings({ startMinimized: next })}
                  />
                </div>
              </div>

              <div className="rounded-2xl border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2 text-sm font-semibold">
                    <Timer className="h-4 w-4 text-indigo-500" aria-hidden="true" />
                    自动化巡检与每日签到策略
                  </div>
                  <span className="rounded-full bg-zinc-100 px-2 py-0.5 text-[11px] text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400">
                    即时生效
                  </span>
                </div>
                <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                  主窗口可见时按单队列防风控节奏静默核实额度、同步跨天签到状态；隐藏到托盘后将暂停周期性刷新。
                </p>

                <div className="mt-4 space-y-2.5">
                  <SettingToggleRow
                    id="qwitch-auto-refresh-startup"
                    label="启动时自动同步额度与签到状态"
                    description="应用启动完成初始化后，自动在后台串行刷新一次已授权账号的最新额度与跨天签到状态。"
                    checked={Boolean(appSettings.autoRefreshOnStartup)}
                    onChange={(next) => void saveAppSettings({ autoRefreshOnStartup: next })}
                  />
                  <SettingToggleRow
                    id="qwitch-auto-claim-daily"
                    label="发现待领福利时自动后台签到"
                    description="结合启动巡检或后台定时刷新，若确认账号今日可领 (Available)，按防风控抖动延迟自动领取。"
                    checked={Boolean(appSettings.autoClaimDaily)}
                    onChange={(next) => {
                      if (next) {
                        setConfirmLogAction({
                          label: "启用自动后台签到？",
                          detail:
                            "开启后将在启动巡检或后台定时刷新发现可领福利时自动发起领取，属于无人值守敏感动作，可能增加平台风控概率。确认启用？",
                          danger: true,
                          run: async () => {
                            await saveAppSettings({ autoClaimDaily: true });
                          },
                        });
                      } else {
                        void saveAppSettings({ autoClaimDaily: false });
                      }
                    }}
                  />

                  <div className="rounded-xl border border-zinc-100 bg-zinc-50/70 px-3 py-2.5 dark:border-zinc-800 dark:bg-zinc-800/40">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div>
                        <div className="text-xs font-medium text-zinc-900 dark:text-zinc-100">
                          后台定时刷新间隔
                        </div>
                        <p className="mt-0.5 text-[11px] text-zinc-600 dark:text-zinc-400">
                          主窗口保持可见期间周期性检查额度与次日新签到活动（隐藏到托盘后自动暂停）。
                        </p>
                      </div>
                      <div className="flex flex-wrap items-center gap-1">
                        {[
                          { label: "关闭", value: 0 },
                          { label: "15 分钟", value: 15 },
                          { label: "30 分钟", value: 30 },
                          { label: "60 分钟", value: 60 },
                          { label: "120 分钟", value: 120 },
                        ].map((opt) => {
                          const current = appSettings.autoRefreshIntervalMinutes ?? 0;
                          const selected = current === opt.value;
                          return (
                            <button
                              key={opt.value}
                              type="button"
                              onClick={() =>
                                void saveAppSettings({
                                  autoRefreshIntervalMinutes: opt.value,
                                })
                              }
                              className={`rounded-lg px-2 py-1 text-[11px] font-medium transition ${
                                selected
                                  ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                                  : "bg-white text-zinc-700 hover:bg-zinc-200/70 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700"
                              }`}
                            >
                              {opt.label}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  </div>
                </div>
              </div>

              <div className="rounded-2xl border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2 text-sm font-semibold">
                    <ShieldCheck className="h-4 w-4 text-emerald-500" aria-hidden="true" />
                    防风控串行调度参数
                  </div>
                  <button
                    type="button"
                    onClick={() =>
                      void saveAppSettings({
                        jitterMinMs: jitterRangeMs[0],
                        jitterMaxMs: jitterRangeMs[1],
                      })
                    }
                    className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-zinc-900 px-2.5 py-1 text-[11px] font-medium text-white transition hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                  >
                    <Save className="h-3 w-3 shrink-0" aria-hidden="true" />
                    保存配置
                  </button>
                </div>
                <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                  批量查额与每日领取均强制单队列串行，并在账号间加入随机抖动延迟。
                </p>
                <div className="mt-4 grid grid-cols-2 gap-3">
                  <div>
                    <label
                      htmlFor="qwitch-jitter-min"
                      className="mb-1 block text-xs text-zinc-600 dark:text-zinc-400"
                    >
                      最小间隔 (ms)
                    </label>
                    <input
                      id="qwitch-jitter-min"
                      type="number"
                      min={500}
                      value={jitterRangeMs[0]}
                      onChange={(e) => {
                        const parsed = Number(e.target.value);
                        const next = Number.isFinite(parsed) ? Math.max(0, Math.floor(parsed)) : 0;
                        setJitterRangeMs([next, jitterRangeMs[1]]);
                      }}
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                    />
                  </div>
                  <div>
                    <label
                      htmlFor="qwitch-jitter-max"
                      className="mb-1 block text-xs text-zinc-600 dark:text-zinc-400"
                    >
                      最大间隔 (ms)
                    </label>
                    <input
                      id="qwitch-jitter-max"
                      type="number"
                      min={500}
                      value={jitterRangeMs[1]}
                      onChange={(e) => {
                        const parsed = Number(e.target.value);
                        const next = Number.isFinite(parsed) ? Math.max(0, Math.floor(parsed)) : 0;
                        setJitterRangeMs([jitterRangeMs[0], next]);
                      }}
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                    />
                  </div>
                </div>
              </div>

              <div className="rounded-2xl border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2 text-sm font-semibold">
                    <Archive className="h-4 w-4 text-amber-500" aria-hidden="true" />
                    日志保留与自动归档策略
                  </div>
                  <button
                    type="button"
                    onClick={() =>
                      void saveAppSettings({
                        logArchiveAfterDays: Math.max(0, Math.floor(logRetention.archiveAfterDays)),
                        logMaxActiveCount: Math.max(0, Math.floor(logRetention.maxActiveCount)),
                      })
                    }
                    className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-zinc-900 px-2.5 py-1 text-[11px] font-medium text-white transition hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                  >
                    <Save className="h-3 w-3 shrink-0" aria-hidden="true" />
                    保存并执行
                  </button>
                </div>
                <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                  启动及保存时自动将超期或超出上限的旧日志移入归档，填 0 表示不限制。
                </p>
                <div className="mt-4 grid grid-cols-2 gap-3">
                  <div>
                    <label
                      htmlFor="qwitch-log-days"
                      className="mb-1 block text-xs text-zinc-600 dark:text-zinc-400"
                    >
                      自动归档天数（天）
                    </label>
                    <input
                      id="qwitch-log-days"
                      type="number"
                      min={0}
                      value={logRetention.archiveAfterDays}
                      onChange={(e) => {
                        const parsed = Number(e.target.value);
                        const next = Number.isFinite(parsed) ? Math.max(0, Math.floor(parsed)) : 0;
                        setLogRetention({
                          ...logRetention,
                          archiveAfterDays: next,
                        });
                      }}
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                    />
                  </div>
                  <div>
                    <label
                      htmlFor="qwitch-log-max"
                      className="mb-1 block text-xs text-zinc-600 dark:text-zinc-400"
                    >
                      活跃日志上限（条）
                    </label>
                    <input
                      id="qwitch-log-max"
                      type="number"
                      min={0}
                      value={logRetention.maxActiveCount}
                      onChange={(e) => {
                        const parsed = Number(e.target.value);
                        const next = Number.isFinite(parsed) ? Math.max(0, Math.floor(parsed)) : 0;
                        setLogRetention({
                          ...logRetention,
                          maxActiveCount: next,
                        });
                      }}
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                    />
                  </div>
                </div>
              </div>

              <div className="rounded-2xl border border-zinc-200 bg-white p-5 lg:col-span-2 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex items-center gap-2 text-sm font-semibold">
                  <Puzzle className="h-4 w-4 text-sky-500" aria-hidden="true" />
                  已注册适配器（内置 + 外部扩展）
                </div>
                <div className="mt-3 grid grid-cols-1 gap-2.5 md:grid-cols-3">
                  {adapters.map((ad) => (
                    <div
                      key={ad.id}
                      className="flex items-center justify-between gap-3 rounded-xl border border-zinc-100 px-3 py-2.5 text-xs dark:border-zinc-800"
                    >
                      <div className="flex min-w-0 items-center gap-2.5">
                        <ToolBrandIcon toolId={ad.id} size="sm" decorative />
                        <div className="min-w-0">
                          <div className="truncate font-semibold text-zinc-900 dark:text-zinc-100">
                            {ad.name}
                          </div>
                          <div className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                            {getToolBrandMeta(ad.id, adapters).tagline}
                          </div>
                        </div>
                      </div>
                      <span className="shrink-0 whitespace-nowrap rounded-md bg-zinc-100 px-2 py-0.5 text-[11px] text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
                        v{ad.version} · {ad.capabilities.supportsDailyClaim ? "查额与签到" : "仅查额与切号"}
                      </span>
                    </div>
                  ))}
                </div>
              </div>

              {isTauriRuntime && (
                <div className="rounded-2xl border border-zinc-200 bg-white p-5 lg:col-span-2 dark:border-zinc-800 dark:bg-zinc-900">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <div className="flex items-center gap-2 text-sm font-semibold">
                        <ShieldCheck className="h-4 w-4 text-emerald-500" aria-hidden="true" />
                        外部插件信任管理
                      </div>
                      <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                        外部适配器仅在显式启用后才会加载并接收账号凭据；保存后需重启 Qwitch 生效。内置与保留 ID 不可被外部插件冒用。
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={async () => {
                        const ok = await saveAppSettings({ trustedAdapters: trustedDraft }, true);
                        if (ok) {
                          showToast("插件信任设置已保存，重启 Qwitch 后生效", "success");
                        }
                      }}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-zinc-900 px-2.5 py-1 text-xs font-medium text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                    >
                      <Save className="h-3 w-3 shrink-0" aria-hidden="true" />
                      保存信任设置
                    </button>
                  </div>
                  {externalPlugins.length === 0 ? (
                    <p className="mt-3 text-xs text-zinc-500 dark:text-zinc-400">
                      未发现外部插件。将插件目录放入 Qwitch 数据目录的 adapters/ 下后重新进入本页即可看到。
                    </p>
                  ) : (
                    <div className="mt-3 space-y-2">
                      {externalPlugins.map((plugin) => {
                        const enabled = trustedDraft.includes(plugin.id);
                        return (
                          <div
                            key={plugin.id}
                            className="flex items-center justify-between gap-3 rounded-xl border border-zinc-100 px-3 py-2.5 text-xs dark:border-zinc-800"
                          >
                            <div className="min-w-0">
                              <div className="truncate font-semibold text-zinc-900 dark:text-zinc-100">
                                {plugin.name}
                                <span className="ml-2 font-normal text-zinc-500 dark:text-zinc-400">
                                  {plugin.id} · v{plugin.version}
                                </span>
                              </div>
                              {plugin.reserved && (
                                <div className="mt-0.5 text-[11px] text-rose-600 dark:text-rose-400">
                                  该 ID 为内置/历史保留，已被禁止加载
                                </div>
                              )}
                            </div>
                            <button
                              type="button"
                              role="switch"
                              aria-checked={enabled}
                              aria-label={`${enabled ? "停用" : "启用"}插件 ${plugin.name}`}
                              disabled={plugin.reserved}
                              onClick={() =>
                                setTrustedDraft((prev) =>
                                  enabled ? prev.filter((id) => id !== plugin.id) : [...prev, plugin.id]
                                )
                              }
                              className={`relative h-5 w-9 shrink-0 rounded-full transition disabled:cursor-not-allowed disabled:opacity-40 ${
                                enabled ? "bg-emerald-500" : "bg-zinc-300 dark:bg-zinc-600"
                              }`}
                            >
                              <span
                                className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${
                                  enabled ? "left-4.5" : "left-0.5"
                                }`}
                              />
                            </button>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              )}

              <div className="rounded-2xl border border-zinc-200 bg-white p-5 lg:col-span-2 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <div className="flex items-center gap-2 text-sm font-semibold">
                      <FolderSearch className="h-4 w-4 text-indigo-500" aria-hidden="true" />
                      本机客户端环境探测与路径覆盖
                    </div>
                    <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                      若目标桌面客户端安装在非标准目录，可在此配置自定义可执行路径（.app 或 .exe），将同步作用于进程检测、解密与重启。
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => void refreshClientStatuses()}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
                    >
                      <RefreshCw className="h-3 w-3 shrink-0" aria-hidden="true" />
                      重新探测
                    </button>
                    <button
                      type="button"
                      onClick={async () => {
                        const ok = await saveAppSettings({ customClientPaths: pathDrafts });
                        if (ok) {
                          setPathDraftsDirty(false);
                          await refreshClientStatuses();
                        }
                      }}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-zinc-900 px-2.5 py-1 text-xs font-medium text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                    >
                      <Save className="h-3 w-3 shrink-0" aria-hidden="true" />
                      保存路径覆盖
                    </button>
                  </div>
                </div>

                <div className="mt-4 space-y-3">
                  {adapters
                    .filter((ad) => ad.capabilities.supportsClientSwitch)
                    .map((ad) => {
                      const status = clientStatuses[ad.id];
                      return (
                        <div
                          key={ad.id}
                          className="rounded-xl border border-zinc-100 bg-zinc-50/70 p-3.5 text-xs dark:border-zinc-800 dark:bg-zinc-800/40"
                        >
                          <div className="flex flex-wrap items-center justify-between gap-2">
                            <div className="flex items-center gap-2.5">
                              <ToolBrandIcon toolId={ad.id} size="sm" decorative />
                              <span className="font-semibold text-zinc-900 dark:text-zinc-100">
                                {ad.name}
                              </span>
                              <span className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                                {ad.id}
                              </span>
                            </div>
                            <span
                              className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-[11px] font-medium ${
                                status?.isRunning
                                  ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                                  : "bg-zinc-200/70 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                              }`}
                            >
                              {status?.isRunning && (
                                <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                              )}
                              {status?.isRunning ? "桌面客户端运行中" : "未运行"}
                            </span>
                          </div>

                          <div className="mt-2.5 grid grid-cols-1 gap-1.5 text-[11px] text-zinc-600 dark:text-zinc-400 md:grid-cols-2">
                            <div className="truncate">
                              <span className="text-zinc-500 dark:text-zinc-400">探测应用：</span>
                              <span className="font-mono text-zinc-800 dark:text-zinc-200">
                                {status?.executablePath ?? "未检测到安装路径"}
                              </span>
                            </div>
                            <div className="truncate">
                              <span className="text-zinc-500 dark:text-zinc-400">会话目录：</span>
                              <span className="font-mono text-zinc-800 dark:text-zinc-200">
                                {status?.configDir ?? "未知"}
                              </span>
                            </div>
                          </div>

                          <div className="mt-2.5">
                            <input
                              type="text"
                              value={pathDrafts[ad.id] ?? ""}
                              aria-label={`${ad.name} 自定义客户端路径`}
                              onChange={(e) => {
                                setPathDrafts((prev) => ({ ...prev, [ad.id]: e.target.value }));
                                setPathDraftsDirty(true);
                              }}
                              placeholder="自定义客户端路径（留空使用自动探测路径）"
                              className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 font-mono text-[11px] text-zinc-900 focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100"
                            />
                          </div>
                        </div>
                      );
                    })}
                </div>
              </div>

              <UpdateCard />
            </div>
          )}
        </main>
      </div>

      <ColdSwitchModal />
      <ImportAccountModal
        open={importOpen || Boolean(bindTargetAccount)}
        defaultToolId={bindTargetAccount?.toolId ?? importPresetTool ?? activeToolFilter}
        defaultMethod={importDefaultMethod}
        targetAccount={bindTargetAccount}
        onClose={() => {
          setImportOpen(false);
          setBindTargetAccount(null);
          setImportDefaultMethod(undefined);
          setImportPresetTool(undefined);
        }}
      />
      <ConfirmDialog
        open={Boolean(confirmLogAction)}
        title={confirmLogAction?.label ?? ""}
        description={confirmLogAction?.detail ?? ""}
        danger={confirmLogAction?.danger}
        confirmLabel="确认执行"
        onCancel={() => setConfirmLogAction(null)}
        onConfirm={async () => {
          const action = confirmLogAction;
          setConfirmLogAction(null);
          if (action) {
            await action.run();
            setSelectedLogIds([]);
          }
        }}
      />
    </div>
  );
}
