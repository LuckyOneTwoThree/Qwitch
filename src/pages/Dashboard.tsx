import { useEffect, useMemo, useState } from "react";
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
  AlertTriangle,
  CheckSquare,
  Square,
} from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { AccountCard } from "../components/AccountCard";
import { BatchClaimBanner } from "../components/BatchClaimBanner";
import { ColdSwitchModal } from "../components/ColdSwitchModal";
import { ImportAccountModal } from "../components/ImportAccountModal";
import { QwitchLogo, ToolBrandIcon, getToolBrandMeta } from "../components/ToolBrandIcon";
import type { ActionLogItem, AuthMethod, ToolId } from "../types/qwitch";

type NavSection = "all" | "trae_cn" | "workbuddy" | "qoder_cn" | "logs" | "settings";
type AccountFilter = "all" | "active_client" | "claimable" | "attention";
type AllViewMode = "grouped" | "flat";
type LogActionFilter = "all" | ActionLogItem["actionType"];
type LogResultFilter = "all" | ActionLogItem["resultStatus"];

const TOOL_TABS: Array<{ id: "trae_cn" | "workbuddy" | "qoder_cn"; label: string }> = [
  { id: "trae_cn", label: "Trae CN" },
  { id: "workbuddy", label: "WorkBuddy" },
  { id: "qoder_cn", label: "Qoder CN" },
];

const ACTION_TYPE_META: Record<
  ActionLogItem["actionType"],
  { label: string; style: string }
> = {
  claim: {
    label: "每日签到",
    style: "bg-sky-500/10 text-sky-700 dark:text-sky-300",
  },
  query: {
    label: "额度查询",
    style: "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
  },
  switch: {
    label: "桌面切号",
    style: "bg-indigo-500/10 text-indigo-700 dark:text-indigo-300",
  },
  import: {
    label: "账号导入",
    style: "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
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

export function Dashboard() {
  const {
    accounts,
    logs,
    logScope,
    setLogScope,
    logStats,
    adapters,
    refreshAllQuotas,
    claimDailyBatch,
    batchProgress,
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
  } = useQwitch();

  const [nav, setNav] = useState<NavSection>("all");
  const [importOpen, setImportOpen] = useState(false);
  const [importDefaultMethod, setImportDefaultMethod] = useState<AuthMethod | undefined>(undefined);
  const [importPresetTool, setImportPresetTool] = useState<ToolId | undefined>(undefined);
  const [refreshingAll, setRefreshingAll] = useState(false);
  const [pathDrafts, setPathDrafts] = useState<Record<string, string>>({});
  const [searchQuery, setSearchQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<AccountFilter>("all");
  const [viewMode, setViewMode] = useState<AllViewMode>("grouped");
  const [logActionFilter, setLogActionFilter] = useState<LogActionFilter>("all");
  const [logResultFilter, setLogResultFilter] = useState<LogResultFilter>("all");
  const [logSearch, setLogSearch] = useState("");
  const [selectedLogIds, setSelectedLogIds] = useState<number[]>([]);
  const [confirmLogAction, setConfirmLogAction] = useState<{
    label: string;
    detail: string;
    danger?: boolean;
    run: () => Promise<void>;
  } | null>(null);

  const activeToolFilter: ToolId | undefined =
    nav === "trae_cn" || nav === "workbuddy" || nav === "qoder_cn" ? nav : undefined;

  useEffect(() => {
    setPathDrafts(customClientPaths);
  }, [customClientPaths]);

  useEffect(() => {
    void refreshClientStatuses();
  }, [refreshClientStatuses]);

  useEffect(() => {
    if (nav === "settings") {
      void refreshClientStatuses();
    }
  }, [nav, refreshClientStatuses]);

  const scopeAccounts = useMemo(() => {
    if (nav === "all") return accounts;
    if (nav === "trae_cn" || nav === "workbuddy" || nav === "qoder_cn") {
      return accounts.filter((a) => a.toolId === nav);
    }
    return [];
  }, [accounts, nav]);

  const filteredAccounts = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return scopeAccounts.filter((acc) => {
      if (statusFilter === "active_client" && !acc.isActiveOnClient) return false;
      if (
        statusFilter === "claimable" &&
        !(
          acc.status === "active" &&
          acc.hasApiToken !== false &&
          acc.latestQuota?.claimStatus === "available"
        )
      ) {
        return false;
      }
      if (
        statusFilter === "attention" &&
        acc.status === "active" &&
        acc.hasApiToken !== false &&
        acc.hasLocalSnapshot !== false
      ) {
        return false;
      }
      if (!q) return true;
      const haystack = [
        acc.label,
        acc.uidMasked,
        acc.emailMasked ?? "",
        acc.latestQuota?.planName ?? "",
        getToolBrandMeta(acc.toolId).name,
      ]
        .join(" ")
        .toLowerCase();
      return haystack.includes(q);
    });
  }, [scopeAccounts, searchQuery, statusFilter]);

  const groupedModules = useMemo(() => {
    const isFiltering = Boolean(searchQuery.trim()) || statusFilter !== "all";
    const knownToolIds = new Set<string>(TOOL_TABS.map((t) => t.id));
    const extraToolIds = Array.from(
      new Set(filteredAccounts.map((a) => a.toolId).filter((id) => !knownToolIds.has(id)))
    );

    const list = [
      ...TOOL_TABS.map((t) => ({ id: t.id as ToolId, name: t.label })),
      ...extraToolIds.map((id) => ({ id, name: getToolBrandMeta(id).name })),
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
      .filter((group) => (isFiltering ? group.accounts.length > 0 : group.accounts.length > 0 || accounts.length === 0));
  }, [filteredAccounts, searchQuery, statusFilter, clientStatuses, accounts.length]);

  useEffect(() => {
    setSelectedLogIds([]);
    setConfirmLogAction(null);
  }, [logScope, nav]);

  const filteredLogs = useMemo(() => {
    const q = logSearch.trim().toLowerCase();
    return logs.filter((log) => {
      if (logActionFilter !== "all" && log.actionType !== logActionFilter) return false;
      if (logResultFilter !== "all" && log.resultStatus !== logResultFilter) return false;
      if (!q) return true;
      const toolName = getToolBrandMeta(log.toolId).name;
      return [log.messageSanitized, toolName, log.toolId, log.errorCode ?? ""]
        .join(" ")
        .toLowerCase()
        .includes(q);
    });
  }, [logs, logActionFilter, logResultFilter, logSearch]);

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

  const stats = useMemo(() => {
    const activeCount = accounts.filter((a) => a.status === "active").length;
    const claimableCount = accounts.filter(
      (a) =>
        a.status === "active" &&
        a.hasApiToken !== false &&
        a.latestQuota?.claimStatus === "available"
    ).length;
    const claimedCount = accounts.filter(
      (a) =>
        a.status === "active" &&
        a.hasApiToken !== false &&
        a.latestQuota?.claimStatus === "claimed"
    ).length;
    const attentionCount = accounts.filter(
      (a) =>
        a.status === "expired" ||
        a.status === "risk_paused" ||
        a.hasApiToken === false ||
        a.hasLocalSnapshot === false
    ).length;
    const activeClients = accounts.filter((a) => a.isActiveOnClient);
    const runningClientsCount = TOOL_TABS.filter(
      (t) => clientStatuses[t.id]?.isRunning
    ).length;

    return {
      total: accounts.length,
      activeCount,
      claimableCount,
      claimedCount,
      attentionCount,
      activeClients,
      runningClientsCount,
    };
  }, [accounts, clientStatuses]);

  const scopeFilterCounts = useMemo(() => {
    return {
      all: scopeAccounts.length,
      active_client: scopeAccounts.filter((a) => a.isActiveOnClient).length,
      claimable: scopeAccounts.filter(
        (a) =>
          a.status === "active" &&
          a.hasApiToken !== false &&
          a.latestQuota?.claimStatus === "available"
      ).length,
      attention: scopeAccounts.filter(
        (a) =>
          a.status === "expired" ||
          a.status === "risk_paused" ||
          a.hasApiToken === false ||
          a.hasLocalSnapshot === false
      ).length,
    };
  }, [scopeAccounts]);

  const handleRefreshAll = async () => {
    setRefreshingAll(true);
    await refreshAllQuotas();
    setRefreshingAll(false);
  };

  const openImportModal = (defaultMethod?: AuthMethod, toolId?: ToolId) => {
    setImportDefaultMethod(defaultMethod);
    setImportPresetTool(toolId);
    setImportOpen(true);
  };

  const headerTitle = useMemo(() => {
    if (nav === "logs") return "操作审计日志";
    if (nav === "settings") return "系统设置与客户端环境";
    if (activeToolFilter) {
      return `${getToolBrandMeta(activeToolFilter).name} 账号管理`;
    }
    return "账号与额度总览";
  }, [nav, activeToolFilter]);

  const headerSubtitle = useMemo(() => {
    if (nav === "logs") {
      return `活跃 ${logStats.activeCount} 条 · 已归档 ${logStats.archivedCount} 条${
        logStats.noiseActiveCount > 0
          ? ` · 可清理日常冗余 ${logStats.noiseActiveCount} 条`
          : ""
      }`;
    }
    if (nav === "settings") {
      return `已加载 ${adapters.length} 个工具适配器 · ${stats.runningClientsCount} 个桌面客户端运行中`;
    }
    if (activeToolFilter) {
      return `${getToolBrandMeta(activeToolFilter).tagline} · 当前纳管 ${scopeAccounts.length} 个账号`;
    }
    return `共 ${stats.total} 个纳管账号 · 今日已签 ${stats.claimedCount} 个，待领 ${stats.claimableCount} 个`;
  }, [nav, logStats, adapters.length, stats, activeToolFilter, scopeAccounts.length]);

  const isFiltered = Boolean(searchQuery.trim()) || statusFilter !== "all";

  return (
    <div className="flex min-h-screen bg-zinc-50 text-zinc-900 dark:bg-zinc-950 dark:text-zinc-100">
      <aside className="sticky top-0 flex h-screen w-54 shrink-0 flex-col justify-between border-r border-zinc-200 bg-white/85 p-3.5 dark:border-zinc-800 dark:bg-zinc-900/65">
        <div className="overflow-y-auto pr-0.5">
          <div className="mb-5 flex items-center gap-2.5 px-1.5">
            <QwitchLogo size="md" />
            <div className="min-w-0">
              <div className="text-sm font-semibold tracking-tight">Qwitch</div>
              <div className="truncate text-[11px] text-zinc-400">AI 账号与额度工作台</div>
            </div>
          </div>

          <div className="space-y-4">
            <div>
              <div className="mb-1.5 px-2.5 text-[10px] font-semibold tracking-wider text-zinc-400 uppercase">
                工作台视图
              </div>
              <nav className="space-y-1">
                <button
                  type="button"
                  onClick={() => setNav("all")}
                  className={`flex w-full items-center justify-between rounded-xl px-2.5 py-2 text-xs font-medium transition ${
                    nav === "all"
                      ? "bg-zinc-900 text-white shadow-2xs dark:bg-zinc-100 dark:text-zinc-900"
                      : "text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <Layers className="h-3.5 w-3.5 shrink-0" />
                    全部账号
                  </span>
                  <span
                    className={`rounded-full px-1.5 py-0.5 text-[10px] tabular-nums ${
                      nav === "all"
                        ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                        : "bg-zinc-100 text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400"
                    }`}
                  >
                    {accounts.length}
                  </span>
                </button>

                {TOOL_TABS.map((tool) => {
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
                          : "text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                      }`}
                    >
                      <span className="flex min-w-0 items-center gap-2">
                        <ToolBrandIcon toolId={tool.id} size="xs" />
                        <span className="truncate">{tool.label}</span>
                        {isRunning && (
                          <span
                            title="本机客户端运行中"
                            className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-500"
                          />
                        )}
                      </span>
                      <span
                        className={`rounded-full px-1.5 py-0.5 text-[10px] tabular-nums ${
                          active
                            ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                            : "bg-zinc-100 text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400"
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
              <div className="mb-1.5 px-2.5 text-[10px] font-semibold tracking-wider text-zinc-400 uppercase">
                系统与审计
              </div>
              <nav className="space-y-1">
                <button
                  type="button"
                  onClick={() => setNav("logs")}
                  className={`flex w-full items-center justify-between rounded-xl px-2.5 py-2 text-xs font-medium transition ${
                    nav === "logs"
                      ? "bg-zinc-900 text-white shadow-2xs dark:bg-zinc-100 dark:text-zinc-900"
                      : "text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <ScrollText className="h-3.5 w-3.5 shrink-0" />
                    操作日志
                  </span>
                  <span
                    className={`rounded-full px-1.5 py-0.5 text-[10px] tabular-nums ${
                      nav === "logs"
                        ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                        : "bg-zinc-100 text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400"
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
                      : "text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                  }`}
                >
                  <span className="flex items-center gap-2">
                    <Settings className="h-3.5 w-3.5 shrink-0" />
                    设置与环境
                  </span>
                </button>
              </nav>
            </div>
          </div>
        </div>

        <div className="rounded-xl border border-zinc-200 bg-zinc-50 p-2.5 text-[11px] text-zinc-500 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-400">
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-1.5 font-medium text-zinc-700 dark:text-zinc-200">
              <Lock className="h-3.5 w-3.5 text-emerald-500" />
              Local-First 加密库
            </span>
            <span className="font-mono text-[10px] text-zinc-400">v0.1.0</span>
          </div>
          <p className="mt-1 leading-snug">
            凭据经钥匙串与 AES-256-GCM 加密存于本机，零云端上传。
          </p>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-20 flex flex-wrap items-center justify-between gap-3 border-b border-zinc-200 bg-white/80 px-5 py-3 backdrop-blur dark:border-zinc-800 dark:bg-zinc-900/65">
          <div className="flex min-w-0 items-center gap-2.5">
            {activeToolFilter && <ToolBrandIcon toolId={activeToolFilter} size="sm" />}
            <div className="min-w-0">
              <h1 className="truncate text-sm font-semibold tracking-tight">{headerTitle}</h1>
              <p className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                {headerSubtitle}
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={handleRefreshAll}
              disabled={refreshingAll}
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
            >
              <RefreshCw className={`h-3.5 w-3.5 shrink-0 ${refreshingAll ? "animate-spin" : ""}`} />
              刷新全部额度
            </button>
            <button
              type="button"
              onClick={() => claimDailyBatch()}
              disabled={Boolean(batchProgress?.running)}
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:opacity-50 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
            >
              <Sparkles className="h-3.5 w-3.5 shrink-0" />
              手动一键领取
            </button>
            <button
              type="button"
              onClick={() => openImportModal()}
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
            >
              <Plus className="h-3.5 w-3.5 shrink-0" />
              导入账号
            </button>
            <button
              type="button"
              onClick={toggleTheme}
              title="切换深色/浅色主题"
              className="shrink-0 rounded-lg border border-zinc-200 bg-white p-1.5 text-zinc-600 transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700"
            >
              {theme === "dark" ? <Sun className="h-3.5 w-3.5" /> : <Moon className="h-3.5 w-3.5" />}
            </button>
          </div>
        </header>

        <main className="flex-1 space-y-3.5 p-5">
          <BatchClaimBanner />

          {nav !== "logs" && nav !== "settings" && (
            <>
              <section className="grid grid-cols-2 gap-2.5 md:grid-cols-4">
                <div className="flex items-center justify-between gap-2 rounded-xl border border-zinc-200 bg-white px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-900/90">
                  <div className="flex min-w-0 items-center gap-2.5">
                    <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                      <Layers className="h-3.5 w-3.5" />
                    </div>
                    <div className="min-w-0">
                      <div className="truncate text-[10px] font-medium text-zinc-400">纳管账号</div>
                      <div className="text-base leading-tight font-semibold tabular-nums">
                        {stats.total}
                      </div>
                    </div>
                  </div>
                  <div className="hidden shrink-0 items-center gap-1 xl:flex">
                    {TOOL_TABS.map((t) => (
                      <button
                        key={t.id}
                        type="button"
                        onClick={() => setNav(t.id)}
                        title={`${t.label}: ${accounts.filter((a) => a.toolId === t.id).length}`}
                        className="inline-flex items-center gap-0.5 rounded bg-zinc-50 px-1 py-0.5 text-[10px] text-zinc-600 hover:bg-zinc-100 dark:bg-zinc-800 dark:text-zinc-300"
                      >
                        <ToolBrandIcon toolId={t.id} size="xs" />
                        <span className="tabular-nums">
                          {accounts.filter((a) => a.toolId === t.id).length}
                        </span>
                      </button>
                    ))}
                  </div>
                </div>

                <div className="flex items-center justify-between gap-2 rounded-xl border border-zinc-200 bg-white px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-900/90">
                  <div className="flex min-w-0 items-center gap-2.5">
                    <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-emerald-500/10 text-emerald-600 dark:text-emerald-400">
                      <MonitorCheck className="h-3.5 w-3.5" />
                    </div>
                    <div className="min-w-0">
                      <div className="truncate text-[10px] font-medium text-zinc-400">本机挂载</div>
                      <div className="text-base leading-tight font-semibold tabular-nums">
                        {stats.activeClients.length}
                        <span className="ml-0.5 text-[11px] font-normal text-zinc-400">
                          /{TOOL_TABS.length}
                        </span>
                      </div>
                    </div>
                  </div>
                  <span className="shrink-0 whitespace-nowrap rounded-full bg-zinc-100 px-2 py-0.5 text-[10px] font-medium text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                    {stats.runningClientsCount > 0 ? `${stats.runningClientsCount} 运行中` : "未启动"}
                  </span>
                </div>

                <div className="flex items-center justify-between gap-2 rounded-xl border border-zinc-200 bg-white px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-900/90">
                  <div className="flex min-w-0 items-center gap-2.5">
                    <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-sky-500/10 text-sky-600 dark:text-sky-400">
                      <CheckCircle2 className="h-3.5 w-3.5" />
                    </div>
                    <div className="min-w-0">
                      <div className="truncate text-[10px] font-medium text-zinc-400">今日签到</div>
                      <div className="text-base leading-tight font-semibold tabular-nums">
                        {stats.claimedCount}
                        <span className="ml-1 text-[11px] font-normal text-zinc-400">已签</span>
                      </div>
                    </div>
                  </div>
                  <span
                    className={`shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[10px] font-medium ${
                      stats.claimableCount > 0
                        ? "bg-sky-500/10 text-sky-700 dark:text-sky-300"
                        : "bg-zinc-100 text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400"
                    }`}
                  >
                    {stats.claimableCount > 0 ? `${stats.claimableCount} 待领` : "已全签"}
                  </span>
                </div>

                <div className="flex items-center justify-between gap-2 rounded-xl border border-zinc-200 bg-white px-3.5 py-2.5 dark:border-zinc-800 dark:bg-zinc-900/90">
                  <div className="flex min-w-0 items-center gap-2.5">
                    <div
                      className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-lg ${
                        stats.attentionCount > 0
                          ? "bg-amber-500/10 text-amber-600 dark:text-amber-400"
                          : "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
                      }`}
                    >
                      <Activity className="h-3.5 w-3.5" />
                    </div>
                    <div className="min-w-0">
                      <div className="truncate text-[10px] font-medium text-zinc-400">凭据健康</div>
                      <div className="text-base leading-tight font-semibold tabular-nums">
                        {stats.activeCount}
                        <span className="ml-1 text-[11px] font-normal text-zinc-400">正常</span>
                      </div>
                    </div>
                  </div>
                  <span
                    className={`shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[10px] font-medium ${
                      stats.attentionCount > 0
                        ? "bg-amber-500/10 text-amber-700 dark:text-amber-300"
                        : "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                    }`}
                  >
                    {stats.attentionCount > 0 ? `${stats.attentionCount} 需完善` : "完好"}
                  </span>
                </div>
              </section>

              {scopeAccounts.length > 0 && (
                <div className="flex flex-wrap items-center justify-between gap-2.5 rounded-xl border border-zinc-200 bg-white px-3.5 py-2 dark:border-zinc-800 dark:bg-zinc-900/90">
                  <div className="relative min-w-[190px] flex-1 max-w-xs">
                    <Search className="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400" />
                    <input
                      type="text"
                      value={searchQuery}
                      onChange={(e) => setSearchQuery(e.target.value)}
                      placeholder="搜索昵称、UID、手机号或套餐..."
                      className="w-full rounded-lg border border-zinc-200 bg-zinc-50 py-1 pr-7 pl-8 text-xs text-zinc-900 outline-none transition focus:border-zinc-400 focus:bg-white dark:border-zinc-700 dark:bg-zinc-800/70 dark:text-zinc-100 dark:focus:bg-zinc-800"
                    />
                    {searchQuery && (
                      <button
                        type="button"
                        onClick={() => setSearchQuery("")}
                        className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-0.5 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200"
                      >
                        <X className="h-3 w-3" />
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
                        const cnt = scopeFilterCounts[f.id];
                        return (
                          <button
                            key={f.id}
                            type="button"
                            onClick={() => setStatusFilter(f.id)}
                            className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg px-2 py-1 text-xs font-medium transition ${
                              active
                                ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                                : "bg-zinc-100 text-zinc-600 hover:bg-zinc-200/70 dark:bg-zinc-800 dark:text-zinc-300 dark:hover:bg-zinc-700/70"
                            }`}
                          >
                            <span>{f.label}</span>
                            <span
                              className={`text-[10px] tabular-nums ${
                                active ? "opacity-75" : "text-zinc-400 dark:text-zinc-500"
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
                              : "text-zinc-500 hover:text-zinc-800 dark:text-zinc-400"
                          }`}
                        >
                          <LayoutList className="h-3 w-3 shrink-0" />
                          分组
                        </button>
                        <button
                          type="button"
                          onClick={() => setViewMode("flat")}
                          title="紧凑平铺展示"
                          className={`inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-[11px] font-medium transition ${
                            viewMode === "flat"
                              ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                              : "text-zinc-500 hover:text-zinc-800 dark:text-zinc-400"
                          }`}
                        >
                          <LayoutGrid className="h-3 w-3 shrink-0" />
                          平铺
                        </button>
                      </div>
                    )}
                  </div>
                </div>
              )}

              {scopeAccounts.length === 0 ? (
                <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-200 bg-white/75 px-6 py-14 text-center dark:border-zinc-800 dark:bg-zinc-900/40">
                  {activeToolFilter ? (
                    <ToolBrandIcon toolId={activeToolFilter} size="lg" />
                  ) : (
                    <div className="flex items-center gap-2">
                      {TOOL_TABS.map((t) => (
                        <ToolBrandIcon key={t.id} toolId={t.id} size="md" />
                      ))}
                    </div>
                  )}
                  <h2 className="mt-4 text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                    {activeToolFilter
                      ? `尚未导入 ${getToolBrandMeta(activeToolFilter).name} 账号`
                      : "尚未导入任何 AI 工具账号"}
                  </h2>
                  <p className="mt-1 max-w-md text-xs leading-relaxed text-zinc-500 dark:text-zinc-400">
                    可从本机已登录桌面客户端一键导入当前账号，或通过安全隔离的内置授权窗口登录新账号并同步最新额度。
                  </p>
                  <div className="mt-5 flex flex-wrap items-center justify-center gap-2.5">
                    <button
                      type="button"
                      onClick={() => openImportModal("local_import")}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-xl bg-zinc-900 px-4 py-2 text-xs font-medium text-white transition hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                    >
                      <HardDriveDownload className="h-3.5 w-3.5 shrink-0" />
                      本机一键导入
                    </button>
                    <button
                      type="button"
                      onClick={() => openImportModal("webview_oauth")}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-xl border border-zinc-200 bg-white px-4 py-2 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                    >
                      <Globe className="h-3.5 w-3.5 shrink-0" />
                      内置授权登录
                    </button>
                  </div>
                </div>
              ) : filteredAccounts.length === 0 ? (
                <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-200 bg-white/60 px-6 py-12 text-center dark:border-zinc-800 dark:bg-zinc-900/30">
                  <Search className="h-6 w-6 text-zinc-300 dark:text-zinc-600" />
                  <h2 className="mt-3 text-sm font-medium text-zinc-800 dark:text-zinc-200">
                    当前筛选条件下暂无匹配账号
                  </h2>
                  <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">
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
                          <ToolBrandIcon toolId={group.id} size="sm" />
                          <h2 className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                            {group.name}
                          </h2>
                          <span className="rounded-full bg-zinc-200/70 px-2 py-0.5 text-[10px] font-medium tabular-nums text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                            {group.accounts.length} 个账号
                          </span>
                          <span
                            className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium ${
                              group.isRunning
                                ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                                : "bg-zinc-200/50 text-zinc-500 dark:bg-zinc-800/80 dark:text-zinc-400"
                            }`}
                          >
                            {group.isRunning && (
                              <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                            )}
                            {group.isRunning ? "桌面端运行中" : "桌面端未运行"}
                          </span>
                          {group.activeAccount && (
                            <span className="hidden text-[11px] text-zinc-500 sm:inline dark:text-zinc-400">
                              · 当前挂载：{group.activeAccount.label}
                            </span>
                          )}
                        </div>

                        <button
                          type="button"
                          onClick={() => openImportModal("webview_oauth", group.id)}
                          className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1 text-[11px] font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                        >
                          <Plus className="h-3 w-3 shrink-0" />
                          添加账号
                        </button>
                      </div>

                      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-3">
                        {group.accounts.map((acc) => (
                          <AccountCard key={acc.id} account={acc} />
                        ))}

                        {group.accounts.length === 1 && !isFiltered && (
                          <button
                            type="button"
                            onClick={() => openImportModal("webview_oauth", group.id)}
                            className="group flex min-h-[168px] flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-300/80 bg-white/50 p-4 text-center transition hover:border-zinc-400 hover:bg-white dark:border-zinc-700/80 dark:bg-zinc-900/40 dark:hover:border-zinc-600 dark:hover:bg-zinc-900/70"
                          >
                            <div className="flex h-9 w-9 items-center justify-center rounded-xl border border-zinc-200 bg-white text-zinc-400 shadow-2xs transition group-hover:scale-105 group-hover:text-zinc-700 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-400 dark:group-hover:text-zinc-200">
                              <Plus className="h-4 w-4" />
                            </div>
                            <span className="mt-2.5 text-xs font-medium text-zinc-700 dark:text-zinc-300">
                              添加第 2 个 {group.name} 账号
                            </span>
                            <span className="mt-0.5 text-[11px] text-zinc-400 dark:text-zinc-500">
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
                    <AccountCard key={acc.id} account={acc} />
                  ))}
                </div>
              )}
            </>
          )}

          {nav === "logs" && (
            <div className="space-y-3">
              {/* 顶部：活跃/已归档切换 + 归档/清理动作条 */}
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
                    <ScrollText className="h-3.5 w-3.5 shrink-0" />
                    活跃日志
                    <span
                      className={`rounded-full px-1.5 py-0.2 text-[10px] tabular-nums ${
                        logScope === "active"
                          ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                          : "bg-zinc-200/70 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
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
                    <Archive className="h-3.5 w-3.5 shrink-0" />
                    已归档
                    <span
                      className={`rounded-full px-1.5 py-0.2 text-[10px] tabular-nums ${
                        logScope === "archived"
                          ? "bg-white/20 text-white dark:bg-zinc-900/15 dark:text-zinc-900"
                          : "bg-zinc-200/70 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
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
                            `已归档 ${logStats.noiseActiveCount} 条日常冗余记录（已保留签到、切号与异常）`
                          )
                        }
                        title="将日常“额度查询成功”和“幂等跳过”归档，仅保留签到成功、切号、导入及所有异常"
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-45 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                      >
                        <Sparkles className="h-3.5 w-3.5 shrink-0" />
                        清理冗余
                        {logStats.noiseActiveCount > 0 && (
                          <span className="rounded-full bg-white/20 px-1.5 text-[10px] tabular-nums dark:bg-zinc-900/15">
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
                        <Clock className="h-3.5 w-3.5 shrink-0" />
                        归档 7 天前
                      </button>

                      <button
                        type="button"
                        disabled={logStats.activeCount === 0}
                        onClick={() =>
                          setConfirmLogAction({
                            label: "归档当前全部活跃日志",
                            detail: `将把 ${logStats.activeCount} 条活跃日志全部转入「已归档」，可在归档区查看、导出或恢复。`,
                            run: async () => {
                              await archiveLogs({ scope: "active" });
                            },
                          })
                        }
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                      >
                        <Archive className="h-3.5 w-3.5 shrink-0" />
                        全部归档
                      </button>

                      <button
                        type="button"
                        disabled={logStats.activeCount === 0}
                        onClick={() => void exportLogs("active", false)}
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                      >
                        <Download className="h-3.5 w-3.5 shrink-0" />
                        导出
                      </button>

                      <button
                        type="button"
                        disabled={logStats.activeCount === 0}
                        onClick={() =>
                          setConfirmLogAction({
                            label: "清空活跃日志",
                            detail: `将永久删除 ${logStats.activeCount} 条活跃日志，不可恢复；若需留存请先点击「全部归档」或「导出」。`,
                            danger: true,
                            run: async () => {
                              await deleteLogs({ scope: "active" });
                            },
                          })
                        }
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-rose-200 bg-rose-50/70 px-2.5 py-1.5 text-xs font-medium text-rose-700 transition hover:bg-rose-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-rose-500/30 dark:bg-rose-500/10 dark:text-rose-300 dark:hover:bg-rose-500/20"
                      >
                        <Trash2 className="h-3.5 w-3.5 shrink-0" />
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
                        <Download className="h-3.5 w-3.5 shrink-0" />
                        导出归档 JSON
                      </button>

                      <button
                        type="button"
                        disabled={logStats.archivedCount === 0}
                        onClick={() =>
                          void unarchiveLogs(
                            { scope: "archived" },
                            `已恢复 ${logStats.archivedCount} 条归档日志`
                          )
                        }
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-zinc-700"
                      >
                        <ArchiveRestore className="h-3.5 w-3.5 shrink-0" />
                        全部恢复
                      </button>

                      <button
                        type="button"
                        disabled={logStats.archivedCount === 0}
                        onClick={() =>
                          setConfirmLogAction({
                            label: "彻底清空已归档日志",
                            detail: `将永久删除 ${logStats.archivedCount} 条已归档历史记录，建议先点击「导出归档 JSON」保存备份。`,
                            danger: true,
                            run: async () => {
                              await deleteLogs({ scope: "archived" });
                            },
                          })
                        }
                        className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-rose-200 bg-rose-50/70 px-2.5 py-1.5 text-xs font-medium text-rose-700 transition hover:bg-rose-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-rose-500/30 dark:bg-rose-500/10 dark:text-rose-300 dark:hover:bg-rose-500/20"
                      >
                        <Trash2 className="h-3.5 w-3.5 shrink-0" />
                        彻底清空归档
                      </button>
                    </>
                  )}
                </div>
              </div>

              {/* 二次确认条（防止误清空） */}
              {confirmLogAction && (
                <div
                  className={`flex flex-wrap items-center justify-between gap-3 rounded-2xl border px-4 py-3 text-xs ${
                    confirmLogAction.danger
                      ? "border-rose-200 bg-rose-50/80 text-rose-900 dark:border-rose-500/30 dark:bg-rose-500/10 dark:text-rose-200"
                      : "border-amber-200 bg-amber-50/80 text-amber-900 dark:border-amber-500/30 dark:bg-amber-500/10 dark:text-amber-200"
                  }`}
                >
                  <div className="flex min-w-0 items-center gap-2.5">
                    <AlertTriangle className="h-4 w-4 shrink-0" />
                    <div className="min-w-0">
                      <span className="font-semibold">{confirmLogAction.label}：</span>
                      <span className="opacity-90">{confirmLogAction.detail}</span>
                    </div>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <button
                      type="button"
                      onClick={() => setConfirmLogAction(null)}
                      className="rounded-lg border border-zinc-300/80 bg-white px-2.5 py-1 text-xs font-medium text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200"
                    >
                      取消
                    </button>
                    <button
                      type="button"
                      onClick={async () => {
                        const action = confirmLogAction;
                        setConfirmLogAction(null);
                        await action.run();
                        setSelectedLogIds([]);
                      }}
                      className={`rounded-lg px-3 py-1 text-xs font-medium text-white transition ${
                        confirmLogAction.danger
                          ? "bg-rose-600 hover:bg-rose-700"
                          : "bg-zinc-900 hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900"
                      }`}
                    >
                      确认执行
                    </button>
                  </div>
                </div>
              )}

              {/* 搜索 + 动作/结果筛选栏 */}
              <div className="flex flex-col gap-2.5 rounded-2xl border border-zinc-200 bg-white p-3 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex flex-wrap items-center justify-between gap-2.5">
                  <div className="relative min-w-[220px] flex-1">
                    <Search className="pointer-events-none absolute top-1/2 left-3 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400" />
                    <input
                      type="text"
                      value={logSearch}
                      onChange={(e) => setLogSearch(e.target.value)}
                      placeholder="搜索日志内容、平台名称或错误码..."
                      className="w-full rounded-xl border border-zinc-200 bg-zinc-50/70 py-1.5 pr-8 pl-8 text-xs text-zinc-900 outline-none transition focus:border-zinc-400 focus:bg-white dark:border-zinc-700 dark:bg-zinc-800/60 dark:text-zinc-100 dark:focus:border-zinc-500"
                    />
                    {logSearch && (
                      <button
                        type="button"
                        onClick={() => setLogSearch("")}
                        className="absolute top-1/2 right-2.5 -translate-y-1/2 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200"
                        aria-label="清除搜索"
                      >
                        <X className="h-3.5 w-3.5" />
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
                          <CheckSquare className="h-3.5 w-3.5 text-zinc-900 dark:text-zinc-100" />
                        ) : (
                          <Square className="h-3.5 w-3.5 text-zinc-400" />
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
                              <Archive className="h-3.5 w-3.5 shrink-0" />
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
                              <ArchiveRestore className="h-3.5 w-3.5 shrink-0" />
                              恢复已选 ({selectedLogIds.length})
                            </button>
                          )}

                          <button
                            type="button"
                            onClick={async () => {
                              const ids = [...selectedLogIds];
                              setSelectedLogIds([]);
                              await deleteLogs({ scope: logScope, ids });
                            }}
                            className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-rose-200 bg-rose-50 px-2.5 py-1.5 text-xs font-medium text-rose-700 hover:bg-rose-100 dark:border-rose-500/30 dark:bg-rose-500/10 dark:text-rose-300"
                          >
                            <Trash2 className="h-3.5 w-3.5 shrink-0" />
                            删除已选 ({selectedLogIds.length})
                          </button>
                        </>
                      )}
                    </div>
                  )}
                </div>

                <div className="flex flex-wrap items-center justify-between gap-2.5">
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="mr-1 text-[11px] text-zinc-400">动作：</span>
                    {(
                      [
                        { id: "all", label: "全部" },
                        { id: "claim", label: "每日签到" },
                        { id: "query", label: "额度查询" },
                        { id: "switch", label: "桌面切号" },
                        { id: "import", label: "账号导入" },
                      ] as const
                    ).map((item) => (
                      <button
                        key={item.id}
                        type="button"
                        onClick={() => setLogActionFilter(item.id)}
                        className={`shrink-0 whitespace-nowrap rounded-lg px-2.5 py-1 text-xs font-medium transition ${
                          logActionFilter === item.id
                            ? "bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900"
                            : "bg-zinc-100 text-zinc-600 hover:bg-zinc-200/70 dark:bg-zinc-800 dark:text-zinc-300"
                        }`}
                      >
                        {item.label}
                      </button>
                    ))}
                  </div>

                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="mr-1 text-[11px] text-zinc-400">结果：</span>
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
                            : "bg-zinc-100 text-zinc-600 hover:bg-zinc-200/70 dark:bg-zinc-800 dark:text-zinc-300"
                        }`}
                      >
                        {item.label}
                      </button>
                    ))}
                  </div>
                </div>
              </div>

              {/* 按日期分组的操作日志列表 */}
              <div className="overflow-hidden rounded-2xl border border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900">
                {filteredLogs.length === 0 ? (
                  <div className="px-6 py-12 text-center">
                    <div className="text-xs font-medium text-zinc-600 dark:text-zinc-300">
                      {logScope === "archived"
                        ? "归档区暂无历史日志"
                        : "当前筛选条件下暂无操作记录"}
                    </div>
                    <p className="mt-1 text-[11px] text-zinc-400 dark:text-zinc-500">
                      {logScope === "archived"
                        ? "当日常冗余或超期日志被归档后，可在此查看、导出或恢复。"
                        : "若需查看历史已归档日志，请切换到上方「已归档」标签页。"}
                    </p>
                  </div>
                ) : (
                  <div>
                    {groupedLogs.map((group) => (
                      <div key={group.day}>
                        <div className="flex items-center justify-between border-y border-zinc-100 bg-zinc-50/80 px-4 py-1.5 text-[11px] font-medium text-zinc-500 first:border-t-0 dark:border-zinc-800 dark:bg-zinc-950/40 dark:text-zinc-400">
                          <span>{group.day}</span>
                          <span className="tabular-nums text-zinc-400">{group.items.length} 条</span>
                        </div>
                        <div className="divide-y divide-zinc-100 dark:divide-zinc-800">
                          {group.items.map((log) => {
                            const actionMeta = ACTION_TYPE_META[log.actionType] ?? {
                              label: log.actionType,
                              style: "bg-zinc-100 text-zinc-600",
                            };
                            const toolName = getToolBrandMeta(log.toolId).name;
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
                                        : "opacity-35 group-hover:opacity-100"
                                    }`}
                                    aria-label={checked ? "取消选择该条日志" : "选择该条日志"}
                                  >
                                    {checked ? (
                                      <CheckSquare className="h-3.5 w-3.5 text-zinc-900 dark:text-zinc-100" />
                                    ) : (
                                      <Square className="h-3.5 w-3.5 text-zinc-400" />
                                    )}
                                  </button>
                                  <ToolBrandIcon toolId={log.toolId} size="sm" />
                                  <span
                                    className={`shrink-0 whitespace-nowrap rounded-md px-2 py-0.5 text-[10px] font-medium ${actionMeta.style}`}
                                  >
                                    {actionMeta.label}
                                  </span>
                                  <span
                                    className={`shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[10px] font-medium ${
                                      log.resultStatus === "success"
                                        ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
                                        : log.resultStatus === "skipped"
                                          ? "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400"
                                          : "bg-rose-500/10 text-rose-700 dark:text-rose-400"
                                    }`}
                                  >
                                    {log.resultStatus === "success"
                                      ? "成功"
                                      : log.resultStatus === "skipped"
                                        ? "幂等跳过"
                                        : "异常"}
                                  </span>
                                  <span className="shrink-0 whitespace-nowrap font-medium text-zinc-500 dark:text-zinc-400">
                                    {toolName}
                                  </span>
                                  {log.errorCode && (
                                    <span className="shrink-0 rounded bg-rose-500/10 px-1.5 py-0.5 font-mono text-[10px] text-rose-600 dark:text-rose-300">
                                      {log.errorCode}
                                    </span>
                                  )}
                                  <span className="truncate text-zinc-800 dark:text-zinc-200">
                                    {log.messageSanitized}
                                  </span>
                                </div>

                                <div className="flex shrink-0 items-center gap-2">
                                  <span
                                    title={new Date(log.createdAt).toLocaleString("zh-CN")}
                                    className="font-mono text-[11px] text-zinc-400 tabular-nums"
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
                                        onClick={() =>
                                          void archiveLogs({ scope: "active", ids: [log.id] })
                                        }
                                        className="rounded-md p-1 text-zinc-400 transition hover:bg-zinc-200/70 hover:text-zinc-700 dark:hover:bg-zinc-700 dark:hover:text-zinc-200"
                                      >
                                        <Archive className="h-3.5 w-3.5" />
                                      </button>
                                    ) : (
                                      <button
                                        type="button"
                                        title="恢复到活跃日志"
                                        onClick={() =>
                                          void unarchiveLogs({ scope: "archived", ids: [log.id] })
                                        }
                                        className="rounded-md p-1 text-zinc-400 transition hover:bg-zinc-200/70 hover:text-zinc-700 dark:hover:bg-zinc-700 dark:hover:text-zinc-200"
                                      >
                                        <ArchiveRestore className="h-3.5 w-3.5" />
                                      </button>
                                    )}
                                    <button
                                      type="button"
                                      title="永久删除此条日志"
                                      onClick={() =>
                                        void deleteLogs({ scope: logScope, ids: [log.id] })
                                      }
                                      className="rounded-md p-1 text-zinc-400 transition hover:bg-rose-500/10 hover:text-rose-600 dark:hover:text-rose-400"
                                    >
                                      <Trash2 className="h-3.5 w-3.5" />
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
              </div>
            </div>
          )}

          {nav === "settings" && (
            <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
              <div className="rounded-2xl border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2 text-sm font-semibold">
                    <ShieldCheck className="h-4 w-4 text-emerald-500" />
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
                    <Save className="h-3 w-3 shrink-0" />
                    保存配置
                  </button>
                </div>
                <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">
                  批量手动领取时，账号之间强制串行并加入随机抖动延迟。
                </p>
                <div className="mt-4 grid grid-cols-2 gap-3">
                  <div>
                    <label className="mb-1 block text-xs text-zinc-500">最小间隔 (ms)</label>
                    <input
                      type="number"
                      value={jitterRangeMs[0]}
                      onChange={(e) => setJitterRangeMs([Number(e.target.value), jitterRangeMs[1]])}
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                    />
                  </div>
                  <div>
                    <label className="mb-1 block text-xs text-zinc-500">最大间隔 (ms)</label>
                    <input
                      type="number"
                      value={jitterRangeMs[1]}
                      onChange={(e) => setJitterRangeMs([jitterRangeMs[0], Number(e.target.value)])}
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                    />
                  </div>
                </div>
              </div>

              <div className="rounded-2xl border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2 text-sm font-semibold">
                    <Archive className="h-4 w-4 text-amber-500" />
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
                    <Save className="h-3 w-3 shrink-0" />
                    保存并执行
                  </button>
                </div>
                <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">
                  启动及保存时自动将超期或超出上限的旧日志移入归档，填 0 表示不限制。
                </p>
                <div className="mt-4 grid grid-cols-2 gap-3">
                  <div>
                    <label className="mb-1 block text-xs text-zinc-500">自动归档天数（天）</label>
                    <input
                      type="number"
                      min={0}
                      value={logRetention.archiveAfterDays}
                      onChange={(e) =>
                        setLogRetention({
                          ...logRetention,
                          archiveAfterDays: Math.max(0, Number(e.target.value)),
                        })
                      }
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                    />
                  </div>
                  <div>
                    <label className="mb-1 block text-xs text-zinc-500">活跃日志上限（条）</label>
                    <input
                      type="number"
                      min={0}
                      value={logRetention.maxActiveCount}
                      onChange={(e) =>
                        setLogRetention({
                          ...logRetention,
                          maxActiveCount: Math.max(0, Number(e.target.value)),
                        })
                      }
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                    />
                  </div>
                </div>
              </div>

              <div className="rounded-2xl border border-zinc-200 bg-white p-5 lg:col-span-2 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex items-center gap-2 text-sm font-semibold">
                  <Puzzle className="h-4 w-4 text-sky-500" />
                  已注册内置适配器
                </div>
                <div className="mt-3 grid grid-cols-1 gap-2.5 md:grid-cols-3">
                  {adapters.map((ad) => (
                    <div
                      key={ad.id}
                      className="flex items-center justify-between gap-3 rounded-xl border border-zinc-100 px-3 py-2.5 text-xs dark:border-zinc-800"
                    >
                      <div className="flex min-w-0 items-center gap-2.5">
                        <ToolBrandIcon toolId={ad.id} size="sm" />
                        <div className="min-w-0">
                          <div className="truncate font-semibold text-zinc-900 dark:text-zinc-100">
                            {ad.name}
                          </div>
                          <div className="truncate text-[11px] text-zinc-400">
                            {getToolBrandMeta(ad.id).tagline}
                          </div>
                        </div>
                      </div>
                      <span className="shrink-0 whitespace-nowrap rounded-md bg-zinc-100 px-2 py-0.5 text-[11px] text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                        v{ad.version} · {ad.capabilities.supportsDailyClaim ? "查额与签到" : "仅查额与切号"}
                      </span>
                    </div>
                  ))}
                </div>
              </div>

              <div className="rounded-2xl border border-zinc-200 bg-white p-5 lg:col-span-2 dark:border-zinc-800 dark:bg-zinc-900">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <div className="flex items-center gap-2 text-sm font-semibold">
                      <FolderSearch className="h-4 w-4 text-indigo-500" />
                      本机客户端环境探测与路径覆盖
                    </div>
                    <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">
                      若目标桌面客户端安装在非标准目录，可在此配置自定义可执行路径（.app 或 .exe）。
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => void refreshClientStatuses()}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
                    >
                      <RefreshCw className="h-3 w-3 shrink-0" />
                      重新探测
                    </button>
                    <button
                      type="button"
                      onClick={async () => {
                        await saveAppSettings({ customClientPaths: pathDrafts });
                        await refreshClientStatuses();
                      }}
                      className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-zinc-900 px-2.5 py-1 text-xs font-medium text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                    >
                      <Save className="h-3 w-3 shrink-0" />
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
                              <ToolBrandIcon toolId={ad.id} size="sm" />
                              <span className="font-semibold text-zinc-900 dark:text-zinc-100">
                                {ad.name}
                              </span>
                              <span className="font-mono text-[11px] text-zinc-400">{ad.id}</span>
                            </div>
                            <span
                              className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-[10px] font-medium ${
                                status?.isRunning
                                  ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-400"
                                  : "bg-zinc-200/70 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400"
                              }`}
                            >
                              {status?.isRunning && (
                                <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                              )}
                              {status?.isRunning ? "桌面客户端运行中" : "未运行"}
                            </span>
                          </div>

                          <div className="mt-2.5 grid grid-cols-1 gap-1.5 text-[11px] text-zinc-500 dark:text-zinc-400 md:grid-cols-2">
                            <div className="truncate">
                              <span className="text-zinc-400">探测应用：</span>
                              <span className="font-mono text-zinc-700 dark:text-zinc-300">
                                {status?.executablePath ?? "未检测到安装路径"}
                              </span>
                            </div>
                            <div className="truncate">
                              <span className="text-zinc-400">会话目录：</span>
                              <span className="font-mono text-zinc-700 dark:text-zinc-300">
                                {status?.configDir ?? "未知"}
                              </span>
                            </div>
                          </div>

                          <div className="mt-2.5">
                            <input
                              type="text"
                              value={pathDrafts[ad.id] ?? ""}
                              onChange={(e) =>
                                setPathDrafts((prev) => ({ ...prev, [ad.id]: e.target.value }))
                              }
                              placeholder="自定义客户端路径（留空使用自动探测路径）"
                              className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 font-mono text-[11px] text-zinc-900 outline-none focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100"
                            />
                          </div>
                        </div>
                      );
                    })}
                </div>
              </div>
            </div>
          )}
        </main>
      </div>

      <ColdSwitchModal />
      <ImportAccountModal
        open={importOpen}
        defaultToolId={importPresetTool ?? activeToolFilter}
        defaultMethod={importDefaultMethod}
        onClose={() => {
          setImportOpen(false);
          setImportDefaultMethod(undefined);
          setImportPresetTool(undefined);
        }}
      />
    </div>
  );
}
