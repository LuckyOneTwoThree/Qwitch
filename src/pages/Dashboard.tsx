import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  Download,
  Globe,
  HardDriveDownload,
  Layers,
  LayoutGrid,
  LayoutList,
  Link2,
  Loader2,
  Lock,
  MonitorCheck,
  Moon,
  Play,
  Plus,
  Power,
  Radio,
  RefreshCw,
  ScrollText,
  Search,
  Settings,
  ShieldCheck,
  Sparkles,
  Sun,
  X,
} from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { AccountCard } from "../components/AccountCard";
import { BatchClaimBanner } from "../components/BatchClaimBanner";
import { ColdSwitchModal } from "../components/ColdSwitchModal";
import { ImportAccountModal } from "../components/ImportAccountModal";
import { ConfirmDialog } from "../components/ui/ConfirmDialog";
import { QwitchLogo, ToolBrandIcon } from "../components/ToolBrandIcon";
import { UpdateBanner } from "../components/UpdateBanner";
import { useUpdateActions } from "../context/useUpdateActions";
import { getToolBrandMeta, getVisibleTools } from "../domain/toolRegistry";
import {
  isAccountClaimable,
  isAccountNeedingAttention,
  selectScopeAccounts,
  selectScopedStats,
  sortAccountsForFlatView,
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
import { formatLogDayLabel } from "./dashboard/types";
import type {
  AllViewMode,
  ConfirmLogAction,
  LogActionFilter,
  LogResultFilter,
  NavSection,
} from "./dashboard/types";
import { LogsPanel, LogsToolbar } from "./dashboard/LogsView";
import { SettingsView } from "./dashboard/SettingsView";
import { AgentBindingsView } from "./dashboard/AgentBindingsView";
import { RoutingTraceView } from "./dashboard/RoutingTraceView";
import { PoolSimulationModal } from "../components/pool/PoolSimulationModal";
import { usePoolPrototype } from "../context/usePoolPrototypeStore";
import type { TargetAgentId } from "../types/pool";

function SecondAccountGhostCard({
  toolName,
  scopeHint,
  onClick,
}: {
  toolName: string;
  scopeHint: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="group flex min-h-[168px] flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-300/80 bg-white/50 p-4 text-center transition hover:border-zinc-400 hover:bg-white dark:border-zinc-700/80 dark:bg-zinc-900/40 dark:hover:border-zinc-600 dark:hover:bg-zinc-900/70"
    >
      <div className="flex h-9 w-9 items-center justify-center rounded-xl border border-zinc-200 bg-white text-zinc-500 shadow-2xs transition group-hover:scale-105 group-hover:text-zinc-800 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-400 dark:group-hover:text-zinc-200">
        <Plus className="h-4 w-4" aria-hidden="true" />
      </div>
      <span className="mt-2.5 text-xs font-medium text-zinc-700 dark:text-zinc-300">
        添加第 2 个 {toolName} 账号
      </span>
      <span className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400">
        {scopeHint}
      </span>
    </button>
  );
}

export function Dashboard() {
  const {
    isInitializing,
    isTauriRuntime,
    accounts,
    logs,
    logScope,
    logStats,
    adapters,
    refreshAllQuotas,
    claimDailyBatch,
    batchProgress,
    appSettings,
    customClientPaths,
    clientStatuses,
    resetSettingsDrafts,
    refreshClientStatuses,
    cstDayKey,
    theme,
    toggleTheme,
    quitApp,
    updateInfo,
  } = useQwitch();

  // 更新动作统一走 hook（安装 / 打开下载页），避免三处入口各自实现导致分叉
  const {
    installing: updateInstalling,
    runPrimaryAction: runUpdatePrimaryAction,
    primaryLabel: updatePrimaryLabel,
  } = useUpdateActions();
  const hasUpdate = Boolean(updateInfo?.updateAvailable);

  const {
    gateway,
    agentBindings,
    traces,
    toggleGateway,
    detectAgentEnvironment,
  } = usePoolPrototype();

  const mainScrollRef = useRef<HTMLElement | null>(null);
  const [nav, setNav] = useState<NavSection>("all");
  const prevNavRef = useRef<NavSection>(nav);
  const [simModalOpen, setSimModalOpen] = useState(false);
  const [simDefaultPoolId, setSimDefaultPoolId] = useState<string | undefined>(undefined);
  const [simDefaultModelId, setSimDefaultModelId] = useState<string | undefined>(undefined);
  const [highlightAgentId, setHighlightAgentId] = useState<TargetAgentId | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [importDefaultMethod, setImportDefaultMethod] = useState<AuthMethod | undefined>(undefined);
  const [importPresetTool, setImportPresetTool] = useState<ToolId | undefined>(undefined);
  const [bindTargetAccount, setBindTargetAccount] = useState<AccountSummary | null>(null);
  const [pathDrafts, setPathDrafts] = useState<Record<string, string>>({});
  const [pathDraftsDirty, setPathDraftsDirty] = useState(false);
  const [externalPlugins, setExternalPlugins] = useState<ExternalAdapterInfo[]>([]);
  const [trustedDraft, setTrustedDraft] = useState<string[]>([]);

  const reloadExternalPlugins = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const list = await invoke<ExternalAdapterInfo[]>("list_external_adapters");
      setExternalPlugins(list);
      setTrustedDraft(list.filter((p) => p.trusted).map((p) => p.id));
    } catch {
      // ignore plugin list errors
    }
  }, [isTauriRuntime]);

  useEffect(() => {
    const prev = prevNavRef.current;
    prevNavRef.current = nav;
    if (prev === "settings" && nav !== "settings") {
      resetSettingsDrafts();
      setPathDraftsDirty(false);
      setPathDrafts(customClientPaths);
      setTrustedDraft(externalPlugins.filter((p) => p.trusted).map((p) => p.id));
    }
  }, [customClientPaths, externalPlugins, nav, resetSettingsDrafts]);

  useEffect(() => {
    if (nav !== "settings" || !isTauriRuntime) return;
    void reloadExternalPlugins();
  }, [nav, isTauriRuntime, reloadExternalPlugins]);
  const [searchQuery, setSearchQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<AccountFilter>("all");
  const [viewMode, setViewMode] = useState<AllViewMode>("grouped");
  const [logActionFilter, setLogActionFilter] = useState<LogActionFilter>("all");
  const [logResultFilter, setLogResultFilter] = useState<LogResultFilter>("all");
  const [logSearch, setLogSearch] = useState("");
  const [selectedLogIds, setSelectedLogIds] = useState<number[]>([]);

  const [confirmLogAction, setConfirmLogAction] = useState<ConfirmLogAction | null>(null);

  const visibleTools = useMemo(() => getVisibleTools(adapters), [adapters]);
  const accountCountByTool = useMemo(() => {
    const m = new Map<string, number>();
    for (const acc of accounts) m.set(acc.toolId, (m.get(acc.toolId) ?? 0) + 1);
    return m;
  }, [accounts]);
  const orderedVisibleTools = useMemo(() => {
    const populated = visibleTools.filter((t) => (accountCountByTool.get(t.id) ?? 0) > 0);
    const empty = visibleTools.filter((t) => (accountCountByTool.get(t.id) ?? 0) === 0);
    return [...populated, ...empty];
  }, [visibleTools, accountCountByTool]);

  const toolNavRef = useRef<HTMLElement | null>(null);
  const [toolNavOverflow, setToolNavOverflow] = useState({ left: false, right: false });

  const syncToolNavOverflow = useCallback(() => {
    const el = toolNavRef.current;
    if (!el) return;
    const left = el.scrollLeft > 4;
    const right = el.scrollLeft + el.clientWidth < el.scrollWidth - 4;
    setToolNavOverflow((prev) =>
      prev.left === left && prev.right === right ? prev : { left, right }
    );
  }, []);

  useEffect(() => {
    syncToolNavOverflow();
    if (typeof window === "undefined") return;
    window.addEventListener("resize", syncToolNavOverflow);
    return () => window.removeEventListener("resize", syncToolNavOverflow);
  }, [syncToolNavOverflow, orderedVisibleTools, isInitializing, nav]);

  const scrollToolNavBy = (delta: number) => {
    toolNavRef.current?.scrollBy?.({ left: delta, behavior: "smooth" });
  };

  const activeToolFilter: ToolId | undefined =
    nav !== "all" &&
    nav !== "pools" &&
    nav !== "agents" &&
    nav !== "routing" &&
    nav !== "logs" &&
    nav !== "settings"
      ? nav
      : undefined;

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
    const onFocus = () => void refreshClientStatuses();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshClientStatuses]);

  useEffect(() => {
    if (nav === "settings") {
      void refreshClientStatuses();
    }
  }, [nav, refreshClientStatuses]);

  const isAccountSection =
    nav !== "pools" &&
    nav !== "agents" &&
    nav !== "routing" &&
    nav !== "logs" &&
    nav !== "settings";

  const scopeAccounts = useMemo(
    () => (isAccountSection ? selectScopeAccounts(accounts, activeToolFilter) : []),
    [accounts, activeToolFilter, isAccountSection]
  );

  const scopedStats = useMemo(() => {
    void cstDayKey;
    return selectScopedStats(scopeAccounts, adapters, clientStatuses, activeToolFilter);
  }, [scopeAccounts, adapters, clientStatuses, activeToolFilter, cstDayKey]);

  const filteredAccounts = useMemo(() => {
    void cstDayKey;
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
  }, [scopeAccounts, searchQuery, statusFilter, adapters, cstDayKey]);

  // 平铺视图排序：待处理 → 可领取 → 挂载中 → 其余；层内按工具顺序，
  // 与分组视图共享 BUILTIN_TOOL_ORDER，避免切换视图时卡片位置全变。
  const sortedFlatAccounts = useMemo(
    () => sortAccountsForFlatView(filteredAccounts, adapters),
    [filteredAccounts, adapters]
  );

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
    setStatusFilter("all");
    setSearchQuery("");
  }, [nav]);

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

  useEffect(() => {
    const validIds = new Set(filteredLogs.map((l) => l.id));
    setSelectedLogIds((prev) => {
      const valid = prev.filter((id) => validIds.has(id));
      return valid.length === prev.length ? prev : valid;
    });
  }, [filteredLogs]);

  const autoStartDescription = useMemo(() => {
    const ua = typeof navigator !== "undefined" ? navigator.userAgent.toLowerCase() : "";
    if (ua.includes("windows")) {
      return "登录系统时自动启动 Qwitch（Windows 写入当前用户启动项，启动后静默驻留）。";
    }
    if (ua.includes("linux")) {
      return "登录系统时自动启动 Qwitch（Linux 写入 ~/.config/autostart，启动后静默驻留）。";
    }
    return "登录系统时自动启动 Qwitch（macOS 写入 ~/Library/LaunchAgents，启动后静默驻留）。";
  }, []);

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

  const openBindModal = useCallback(
    (account: AccountSummary, preferredMethod?: AuthMethod) => {
      setImportOpen(false);
      setImportDefaultMethod(preferredMethod);
      setImportPresetTool(account.toolId);
      setBindTargetAccount(account);
    },
    []
  );

  const activeToolMeta = activeToolFilter
    ? getToolBrandMeta(activeToolFilter, adapters)
    : undefined;
  const activeToolSupportsClaim = activeToolFilter
    ? (adapters.find((a) => a.id === activeToolFilter)?.capabilities.supportsDailyClaim ?? false)
    : true;
  const isBatchBusy = Boolean(batchProgress?.running);

  const boundAgentCount = useMemo(
    () => agentBindings.filter((b) => b.mountedPoolIds.length > 0).length,
    [agentBindings]
  );
  const driftedAgentCount = useMemo(
    () => agentBindings.filter((b) => b.status === "drifted").length,
    [agentBindings]
  );

  const headerTitle = useMemo(() => {
    if (nav === "pools" || nav === "agents") return "Agent 接入与号池调度";
    if (nav === "routing") return "网关路由观测与决策链路";
    if (nav === "logs") return "操作审计日志";
    if (nav === "settings") return "系统设置与客户端环境";
    if (activeToolMeta) {
      return `${activeToolMeta.name} 账号管理`;
    }
    return "账号与额度总览";
  }, [nav, activeToolMeta]);

  const headerSubtitle = useMemo(() => {
    if (nav === "pools" || nav === "agents") {
      return `已接入 ${boundAgentCount}/${agentBindings.length} 个目标 Agent · 在每个 Agent 下直接挑选订阅工具模型并编排同工具主备顺位${
        driftedAgentCount > 0 ? ` · ${driftedAgentCount} 个存在配置漂移待修复` : ""
      }`;
    }
    if (nav === "routing") {
      return `本地网关 ${gateway.running ? `${gateway.host}:${gateway.port} 运行中` : "已停止"} · 共记录 ${traces.length} 条调度决策采样`;
    }
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
  }, [
    nav,
    boundAgentCount,
    agentBindings.length,
    driftedAgentCount,
    gateway,
    traces.length,
    logStats,
    adapters.length,
    scopedStats,
    activeToolMeta,
  ]);

  const onTitleBarDoubleClick = () => {
    void (async () => {
      if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        if (await win.isMaximized()) {
          await win.unmaximize();
        } else {
          await win.maximize();
        }
      } catch {
        /* ignore */
      }
    })();
  };

  const isFiltered = Boolean(searchQuery.trim()) || statusFilter !== "all";
  const showFilterToolbar =
    scopeAccounts.length > 0 &&
    (nav === "all" || scopeAccounts.length > 1 || isFiltered);
  const currentLogTotal =
    logScope === "archived" ? logStats.archivedCount : logStats.activeCount;

  return (
    <div className="relative flex h-full overflow-hidden bg-zinc-50 text-zinc-900 dark:bg-zinc-950 dark:text-zinc-100">
      {/* 顶部 36px 拖拽区：贯穿全宽（红黄绿悬浮其上），整条可拖动、双击最大化/还原 */}
      <div
        data-tauri-drag-region
        onDoubleClick={onTitleBarDoubleClick}
        className="absolute inset-x-0 top-0 z-50 flex h-9"
      >
        <div className="pointer-events-none w-52 shrink-0" />
        <div className="pointer-events-none flex-1 bg-white/85 backdrop-blur-xl dark:bg-zinc-900/70" />
      </div>
      <aside className="flex h-full w-52 shrink-0 flex-col border-r border-zinc-200 bg-white px-3 pb-3 pt-9 dark:border-zinc-800 dark:bg-zinc-900">
        <div className="flex min-h-0 flex-1 flex-col justify-between">
        <div className="overflow-y-auto pr-0.5">
          <div className="mb-5 flex items-center gap-2.5 px-1.5">
            <QwitchLogo size="md" />
            <div className="min-w-0">
              <div className="text-sm font-semibold tracking-tight">Qwitch</div>
              <div className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                号池与多模型网关
              </div>
            </div>
          </div>

          <div className="space-y-4">
            <div>
              <div className="mb-1.5 px-2.5 text-[11px] font-semibold tracking-wider text-zinc-500 uppercase dark:text-zinc-400">
                工作台与路由
              </div>
              <nav aria-label="主功能导航" className="space-y-1">
                <button
                  type="button"
                  onClick={() => {
                    setHighlightAgentId(null);
                    if (!isAccountSection) setNav("all");
                  }}
                  aria-current={isAccountSection ? "page" : undefined}
                  className={`nav-item ${isAccountSection ? "nav-item-active" : ""}`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <ShieldCheck className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">账号资产</span>
                  </span>
                  <span
                    className={`shrink-0 rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                      isAccountSection
                        ? "bg-brand-600/15 text-brand-800 dark:bg-brand-400/20 dark:text-brand-200"
                        : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {accounts.length}
                  </span>
                </button>

                <button
                  type="button"
                  onClick={() => {
                    setHighlightAgentId(null);
                    setNav("agents");
                  }}
                  aria-current={nav === "agents" || nav === "pools" ? "page" : undefined}
                  className={`nav-item ${nav === "agents" || nav === "pools" ? "nav-item-active" : ""}`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <Link2 className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">Agent 接入与调度</span>
                  </span>
                  <span
                    className={`shrink-0 rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                      driftedAgentCount > 0
                        ? "bg-amber-500/15 text-amber-800 dark:text-amber-300"
                        : nav === "agents" || nav === "pools"
                          ? "bg-brand-600/15 text-brand-800 dark:bg-brand-400/20 dark:text-brand-200"
                          : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {boundAgentCount}/{agentBindings.length}
                  </span>
                </button>

                <button
                  type="button"
                  onClick={() => {
                    setHighlightAgentId(null);
                    setNav("routing");
                  }}
                  aria-current={nav === "routing" ? "page" : undefined}
                  className={`nav-item ${nav === "routing" ? "nav-item-active" : ""}`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <Radio className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">路由观测</span>
                  </span>
                  <span
                    className={`shrink-0 rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                      nav === "routing"
                        ? "bg-brand-600/15 text-brand-800 dark:bg-brand-400/20 dark:text-brand-200"
                        : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {traces.length}
                  </span>
                </button>
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
                  aria-current={nav === "logs" ? "page" : undefined}
                  className={`nav-item ${nav === "logs" ? "nav-item-active" : ""}`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <ScrollText className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">操作日志</span>
                  </span>
                  <span
                    className={`shrink-0 rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                      nav === "logs"
                        ? "bg-brand-600/15 text-brand-800 dark:bg-brand-400/20 dark:text-brand-200"
                        : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {logStats.activeCount}
                  </span>
                </button>

                <button
                  type="button"
                  onClick={() => setNav("settings")}
                  aria-current={nav === "settings" ? "page" : undefined}
                  className={`nav-item ${nav === "settings" ? "nav-item-active" : ""}`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <Settings className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">设置与环境</span>
                  </span>
                  {updateInfo?.updateAvailable && (
                    <span
                      className="ml-auto h-1.5 w-1.5 shrink-0 rounded-full bg-indigo-500"
                      title={`发现新版本 v${updateInfo.latestVersion}`}
                      aria-label={`发现新版本 v${updateInfo.latestVersion}`}
                    />
                  )}
                </button>
              </nav>
            </div>
          </div>
        </div>

        <div className="space-y-2 border-t border-zinc-200/70 pt-3 dark:border-zinc-800/80">
          {/* 本地网关快捷启停控制（以启动/关闭为主交互，隐藏原始端口号） */}
          <div className="flex items-center justify-between gap-2 rounded-xl border border-zinc-200/80 bg-zinc-50/80 px-2.5 py-2 text-xs dark:border-zinc-800 dark:bg-zinc-900/70">
            <button
              type="button"
              onClick={() => setNav("agents")}
              title={`本地网关（${gateway.host}:${gateway.port}），点击进入接入与调度`}
              className="flex min-w-0 items-center gap-2 text-left font-medium text-zinc-700 hover:text-zinc-950 dark:text-zinc-200 dark:hover:text-white"
            >
              <span
                className={`h-2 w-2 shrink-0 rounded-full ${
                  gateway.running ? "bg-emerald-500" : "bg-zinc-400"
                }`}
              />
              <span className="truncate">
                {gateway.running ? "网关运行中" : "网关已关闭"}
              </span>
            </button>
            <button
              type="button"
              onClick={toggleGateway}
              aria-label={gateway.running ? "关闭本地网关" : "启动本地网关"}
              className={`inline-flex shrink-0 items-center gap-1 rounded-lg px-2 py-1 text-[11px] font-medium transition ${
                gateway.running
                  ? "bg-zinc-200/80 text-zinc-700 hover:bg-rose-500/15 hover:text-rose-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-rose-500/20 dark:hover:text-rose-300"
                  : "bg-brand-600 text-white shadow-2xs hover:bg-brand-700 dark:bg-brand-500 dark:text-zinc-950 dark:hover:bg-brand-400"
              }`}
            >
              <Power className="h-3 w-3 shrink-0" aria-hidden="true" />
              {gateway.running ? "关闭" : "启动"}
            </button>
          </div>

          {/* 精简版本与加密库底栏 */}
          {hasUpdate ? (
            <div className="rounded-xl border border-indigo-500/35 bg-indigo-500/[0.06] p-2 text-[11px] dark:border-indigo-400/30 dark:bg-indigo-400/[0.08]">
              <div className="flex items-center justify-between gap-1.5">
                <button
                  type="button"
                  onClick={() => setNav("settings")}
                  className="truncate font-medium text-indigo-800 hover:underline dark:text-indigo-200"
                >
                  新版本可升级
                </button>
                <button
                  type="button"
                  onClick={() => void runUpdatePrimaryAction()}
                  disabled={updateInstalling}
                  title={updatePrimaryLabel}
                  aria-label={updatePrimaryLabel}
                  className="inline-flex shrink-0 items-center gap-1 rounded-md bg-indigo-600 px-1.5 py-0.5 font-mono text-[10px] font-semibold text-white transition hover:bg-indigo-700 disabled:opacity-60 dark:bg-indigo-500"
                >
                  {updateInstalling ? (
                    <Loader2 className="h-2.5 w-2.5 animate-spin" aria-hidden="true" />
                  ) : (
                    <Download className="h-2.5 w-2.5" aria-hidden="true" />
                  )}
                  v{updateInfo?.latestVersion}
                </button>
              </div>
            </div>
          ) : null}

          <div className="flex items-center justify-between gap-2 px-1 text-[11px] text-zinc-500 dark:text-zinc-400">
            <span
              className="flex items-center gap-1"
              title={`Local-First 加密库：凭据经系统钥匙串与 AES-256-GCM 加密落盘；${
                appSettings.closeToTray !== false
                  ? "关闭主窗将隐藏至菜单栏。"
                  : "关闭主窗将直接退出。"
              }`}
            >
              <Lock className="h-3 w-3 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
              <span className="font-mono">v{__APP_VERSION__}</span>
            </span>
            <button
              type="button"
              onClick={() => void quitApp()}
              className="inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] font-medium text-zinc-600 transition hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
            >
              <Power className="h-3 w-3 shrink-0" aria-hidden="true" />
              退出 Qwitch
            </button>
          </div>
        </div>
        </div>
      </aside>

      <div className="@container flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <header className="z-20 flex shrink-0 flex-nowrap items-center justify-between gap-3 border-b border-zinc-200 bg-white/85 px-5 pt-9 pb-3 backdrop-blur-xl dark:border-zinc-800 dark:bg-zinc-900/70">
          <div className="flex min-w-0 flex-1 items-center gap-2.5">
            {activeToolFilter && <ToolBrandIcon toolId={activeToolFilter} size="sm" decorative />}
            <div className="min-w-0">
              <h1 className="truncate text-lg font-semibold tracking-tight">{headerTitle}</h1>
              <p className="truncate text-xs text-zinc-500 dark:text-zinc-400">
                {headerSubtitle}
              </p>
            </div>
          </div>

          <div className="flex shrink-0 items-center gap-2">
            {isAccountSection && (
              <>
                {scopeAccounts.length > 0 && (
                  <button
                    type="button"
                    onClick={() => void refreshAllQuotas(activeToolFilter)}
                    disabled={isBatchBusy}
                    title={activeToolMeta ? `刷新 ${activeToolMeta.name} 额度` : "刷新全部额度"}
                    className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap"
                  >
                    <RefreshCw
                      className={`h-3.5 w-3.5 shrink-0 ${
                        isBatchBusy && batchProgress?.action === "refresh" ? "animate-spin" : ""
                      }`}
                      aria-hidden="true"
                    />
                    {activeToolMeta ? "刷新额度" : "刷新全部"}
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
                        title={
                          activeToolMeta
                            ? `领取 ${activeToolMeta.name} 今日待领 (${scopedStats.claimableCount})`
                            : `一键领取今日待领 (${scopedStats.claimableCount})`
                        }
                        className="btn btn-sm btn-primary gap-1.5 whitespace-nowrap"
                      >
                        <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {`一键领取 (${scopedStats.claimableCount})`}
                      </button>
                    ) : scopedStats.claimActionState === "verify_and_claim" ? (
                      <button
                        type="button"
                        onClick={() => void claimDailyBatch(undefined, activeToolFilter, true)}
                        disabled={isBatchBusy}
                        title={
                          activeToolMeta
                            ? `核实并领取 ${activeToolMeta.name} (${scopedStats.unknownClaimCount})`
                            : `核实并领取 (${scopedStats.unknownClaimCount})`
                        }
                        className="btn btn-sm btn-primary gap-1.5 whitespace-nowrap"
                      >
                        <ShieldCheck className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {`核实并领取 (${scopedStats.unknownClaimCount})`}
                      </button>
                    ) : null}
                  </>
                )}

                <button
                  type="button"
                  onClick={() => openImportModal()}
                  className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap"
                >
                  <Plus className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  导入账号
                </button>
              </>
            )}

            {(nav === "agents" || nav === "pools") && (
              <>
                <button
                  type="button"
                  onClick={() => {
                    setSimDefaultPoolId(undefined);
                    setSimDefaultModelId(undefined);
                    setSimModalOpen(true);
                  }}
                  className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap"
                >
                  <Play className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  测试路由
                </button>
                <button
                  type="button"
                  onClick={detectAgentEnvironment}
                  className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap"
                >
                  <RefreshCw className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  重新探测
                </button>
                <button
                  type="button"
                  onClick={toggleGateway}
                  className={`btn btn-sm gap-1.5 whitespace-nowrap ${
                    gateway.running ? "btn-secondary" : "btn-primary"
                  }`}
                >
                  <Power className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  {gateway.running ? "停止网关" : "启动网关"}
                </button>
              </>
            )}

            {nav === "routing" && (
              <button
                type="button"
                onClick={() => {
                  setSimDefaultPoolId(undefined);
                  setSimDefaultModelId(undefined);
                  setSimModalOpen(true);
                }}
                className="btn btn-sm btn-primary gap-1.5 whitespace-nowrap"
              >
                <Play className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                测试路由
              </button>
            )}

            <button
              type="button"
              onClick={toggleTheme}
              title="切换深色/浅色主题"
              aria-label="切换深色或浅色主题"
              className="btn btn-icon btn-secondary shrink-0"
            >
              {theme === "dark" ? (
                <Sun className="h-3.5 w-3.5" aria-hidden="true" />
              ) : (
                <Moon className="h-3.5 w-3.5" aria-hidden="true" />
              )}
            </button>
          </div>
        </header>

        {nav === "logs" && (
          <LogsToolbar
            allVisibleLogsSelected={allVisibleLogsSelected}
            filteredLogs={filteredLogs}
            logActionFilter={logActionFilter}
            setLogActionFilter={setLogActionFilter}
            logResultFilter={logResultFilter}
            setLogResultFilter={setLogResultFilter}
            logSearch={logSearch}
            setLogSearch={setLogSearch}
            selectedLogIds={selectedLogIds}
            setSelectedLogIds={setSelectedLogIds}
            setConfirmLogAction={setConfirmLogAction}
            toggleSelectAllVisibleLogs={toggleSelectAllVisibleLogs}
          />
        )}

        {nav === "settings" && batchProgress && (
          <div className="shrink-0 px-5 pt-3.5">
            <BatchClaimBanner />
          </div>
        )}

        <main
          ref={mainScrollRef}
          className={`min-h-0 flex-1 ${
            isAccountSection
              ? "overflow-y-auto"
              : nav === "pools"
                ? "overflow-y-auto px-5 py-3.5 @3xl:overflow-hidden"
                : "overflow-y-auto px-5 py-3.5"
          }`}
        >
          {isAccountSection && (
            <>
              {/* 统一吸顶控制区（紧贴顶栏无缝衔接，内部包含工具切换 / 核心指标 / 搜索筛选，且全区支持滚轮滚动） */}
              <div className="sticky top-0 z-10 space-y-2.5 border-b border-zinc-200/90 bg-zinc-50 px-5 py-3 shadow-2xs dark:border-zinc-800 dark:bg-zinc-950">
                <div className="relative flex items-center gap-1.5">
                  {toolNavOverflow.left && (
                    <button
                      type="button"
                      onClick={() => scrollToolNavBy(-220)}
                      aria-label="向左滚动工具栏"
                      className="flex h-7 w-6 shrink-0 items-center justify-center rounded-lg border border-zinc-200 bg-white text-zinc-600 shadow-2xs transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800"
                    >
                      <ChevronLeft className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                  )}

                  <nav
                    ref={toolNavRef}
                    onScroll={syncToolNavOverflow}
                    aria-label="账号视图导航"
                    className={`scrollbar-none flex min-w-0 flex-1 flex-nowrap items-center gap-1.5 overflow-x-auto py-0.5 ${
                      toolNavOverflow.right ? "mask-fade-r pr-2" : ""
                    }`}
                  >
                    <button
                      type="button"
                      onClick={(e) => {
                        setNav("all");
                        e.currentTarget.scrollIntoView?.({
                          behavior: "smooth",
                          inline: "nearest",
                          block: "nearest",
                        });
                      }}
                      aria-current={nav === "all" ? "page" : undefined}
                      className={`inline-flex shrink-0 items-center gap-1.5 rounded-xl px-2.5 py-1.5 text-xs font-medium whitespace-nowrap transition ${
                        nav === "all"
                          ? "bg-brand-600 text-white shadow-2xs dark:bg-brand-500 dark:text-zinc-950"
                          : "bg-white text-zinc-700 ring-1 ring-zinc-200/80 hover:bg-zinc-100 dark:bg-zinc-900 dark:text-zinc-300 dark:ring-zinc-800 dark:hover:bg-zinc-800"
                      }`}
                    >
                      <Layers className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      <span>全部账号</span>
                      <span
                        className={`rounded-md px-1.5 py-0.5 text-[10px] leading-none font-semibold tabular-nums ${
                          nav === "all"
                            ? "bg-white/20 text-white dark:bg-zinc-950/15 dark:text-zinc-950"
                            : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                        }`}
                      >
                        {accounts.length}
                      </span>
                    </button>

                    {orderedVisibleTools.map((tool) => {
                      const active = nav === tool.id;
                      const count = accountCountByTool.get(tool.id) ?? 0;
                      const isRunning = clientStatuses[tool.id]?.isRunning;
                      return (
                        <button
                          key={tool.id}
                          type="button"
                          onClick={(e) => {
                            setNav(tool.id);
                            e.currentTarget.scrollIntoView?.({
                              behavior: "smooth",
                              inline: "nearest",
                              block: "nearest",
                            });
                          }}
                          aria-current={active ? "page" : undefined}
                          className={`inline-flex shrink-0 items-center gap-1.5 rounded-xl px-2.5 py-1.5 text-xs font-medium whitespace-nowrap transition ${
                            active
                              ? "bg-brand-600 text-white shadow-2xs dark:bg-brand-500 dark:text-zinc-950"
                              : count > 0
                                ? "bg-white text-zinc-700 ring-1 ring-zinc-200/80 hover:bg-zinc-100 dark:bg-zinc-900 dark:text-zinc-300 dark:ring-zinc-800 dark:hover:bg-zinc-800"
                                : "bg-white/60 text-zinc-500 ring-1 ring-zinc-200/70 hover:bg-white hover:text-zinc-800 dark:bg-zinc-900/50 dark:text-zinc-400 dark:ring-zinc-800/80 dark:hover:bg-zinc-900 dark:hover:text-zinc-200"
                          }`}
                        >
                          <ToolBrandIcon toolId={tool.id} size="xs" decorative />
                          <span>{tool.name}</span>
                          {isRunning && count > 0 && (
                            <span
                              title="本机客户端运行中"
                              className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                                active ? "bg-white dark:bg-zinc-950" : "bg-emerald-500"
                              }`}
                            />
                          )}
                          <span
                            className={`rounded-md px-1.5 py-0.5 text-[10px] leading-none font-semibold tabular-nums ${
                              active
                                ? "bg-white/20 text-white dark:bg-zinc-950/15 dark:text-zinc-950"
                                : count > 0
                                  ? "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                                  : "bg-zinc-100/70 text-zinc-400 dark:bg-zinc-800/70 dark:text-zinc-500"
                            }`}
                          >
                            {count}
                          </span>
                        </button>
                      );
                    })}
                  </nav>

                  {toolNavOverflow.right && (
                    <button
                      type="button"
                      onClick={() => scrollToolNavBy(220)}
                      aria-label="向右滚动工具栏"
                      className="flex h-7 w-6 shrink-0 items-center justify-center rounded-lg border border-zinc-200 bg-white text-zinc-600 shadow-2xs transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-800"
                    >
                      <ChevronRight className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                  )}
                </div>

                {/* 紧凑单行 KPI 筛选条 */}
                <section
                  aria-label="当前作用域核心指标"
                  className="grid grid-cols-2 gap-2 @2xl:grid-cols-4"
                >
                  <button
                    type="button"
                    onClick={() => setStatusFilter("all")}
                    aria-pressed={statusFilter === "all"}
                    className={`surface-card surface-interactive flex min-w-0 items-center justify-between gap-2 rounded-xl px-3 py-2 text-left ${
                      statusFilter === "all" ? "border-brand-500/60 ring-1 ring-brand-500/20" : ""
                    }`}
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-brand-500/12 text-brand-700 dark:text-brand-400">
                        <Layers className="h-3.5 w-3.5" aria-hidden="true" />
                      </span>
                      <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                        {activeToolMeta ? `${activeToolMeta.name}` : "纳管账号"}
                      </span>
                    </div>
                    <div className="flex shrink-0 items-baseline gap-0.5">
                      <span className="text-sm leading-none font-semibold tracking-tight tabular-nums text-zinc-900 dark:text-zinc-100">
                        {scopedStats.total}
                      </span>
                      <span className="text-[10px] text-zinc-500 dark:text-zinc-400">个</span>
                    </div>
                  </button>

                  <button
                    type="button"
                    onClick={() =>
                      setStatusFilter((prev) => (prev === "active_client" ? "all" : "active_client"))
                    }
                    aria-pressed={statusFilter === "active_client"}
                    className={`surface-card surface-interactive flex min-w-0 items-center justify-between gap-2 rounded-xl px-3 py-2 text-left ${
                      statusFilter === "active_client"
                        ? "border-emerald-500/60 ring-1 ring-emerald-500/20"
                        : ""
                    }`}
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-emerald-500/12 text-emerald-700 dark:text-emerald-400">
                        <MonitorCheck className="h-3.5 w-3.5" aria-hidden="true" />
                      </span>
                      <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                        本机挂载
                      </span>
                    </div>
                    <div className="flex shrink-0 items-center gap-1.5">
                      <span className="text-xs font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                        {scopedStats.activeClientsCount}
                        <span className="text-[10px] font-normal text-zinc-500 dark:text-zinc-400">
                          /{scopedStats.scopeToolCapacity}
                        </span>
                      </span>
                      <span
                        className={`badge shrink-0 whitespace-nowrap ${
                          scopedStats.total > 0 && scopedStats.runningClientsCount > 0
                            ? "border-emerald-500/25 bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                            : "badge-neutral"
                        }`}
                      >
                        {scopedStats.total === 0
                          ? "无"
                          : scopedStats.runningClientsCount > 0
                            ? "运行"
                            : "待启"}
                      </span>
                    </div>
                  </button>

                  <button
                    type="button"
                    onClick={() =>
                      setStatusFilter((prev) => (prev === "claimable" ? "all" : "claimable"))
                    }
                    aria-pressed={statusFilter === "claimable"}
                    className={`surface-card surface-interactive flex min-w-0 items-center justify-between gap-2 rounded-xl px-3 py-2 text-left ${
                      statusFilter === "claimable"
                        ? "border-sky-500/60 ring-1 ring-sky-500/20"
                        : ""
                    }`}
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-sky-500/12 text-sky-700 dark:text-sky-400">
                        <CheckCircle2 className="h-3.5 w-3.5" aria-hidden="true" />
                      </span>
                      <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                        今日签到
                      </span>
                    </div>
                    <div className="flex shrink-0 items-center gap-1.5">
                      <span className="text-xs font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                        {scopedStats.claimedCount}
                        {scopedStats.eligibleClaimTotal > 0 && (
                          <span className="text-[10px] font-normal text-zinc-500 dark:text-zinc-400">
                            /{scopedStats.eligibleClaimTotal}
                          </span>
                        )}
                      </span>
                      <span
                        className={`badge shrink-0 whitespace-nowrap ${
                          scopedStats.claimableCount > 0
                            ? "border-sky-500/25 bg-sky-500/12 text-sky-800 dark:text-sky-300"
                            : scopedStats.unclaimedAuthMissingCount > 0
                              ? "border-amber-500/25 bg-amber-500/12 text-amber-800 dark:text-amber-300"
                              : scopedStats.allClaimed
                                ? "border-emerald-500/25 bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                                : "badge-neutral"
                        }`}
                      >
                        {scopedStats.unclaimedAuthMissingCount > 0 &&
                        scopedStats.eligibleClaimTotal === 0
                          ? "授权后可签"
                          : scopedStats.claimBadgeText}
                      </span>
                    </div>
                  </button>

                  <button
                    type="button"
                    onClick={() =>
                      setStatusFilter((prev) => (prev === "attention" ? "all" : "attention"))
                    }
                    aria-pressed={statusFilter === "attention"}
                    className={`surface-card surface-interactive flex min-w-0 items-center justify-between gap-2 rounded-xl px-3 py-2 text-left ${
                      statusFilter === "attention"
                        ? "border-amber-500/60 ring-1 ring-amber-500/20"
                        : ""
                    }`}
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      <span
                        className={`flex h-6 w-6 shrink-0 items-center justify-center rounded-md ${
                          scopedStats.attentionCount > 0
                            ? "bg-amber-500/12 text-amber-700 dark:text-amber-400"
                            : "bg-emerald-500/12 text-emerald-700 dark:text-emerald-400"
                        }`}
                      >
                        <Activity className="h-3.5 w-3.5" aria-hidden="true" />
                      </span>
                      <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                        凭据健康
                      </span>
                    </div>
                    <div className="flex shrink-0 items-center gap-1.5">
                      <span className="text-xs font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                        {scopedStats.healthyCount}
                      </span>
                      <span
                        className={`badge shrink-0 whitespace-nowrap ${
                          scopedStats.attentionCount > 0
                            ? "border-amber-500/25 bg-amber-500/12 text-amber-800 dark:text-amber-300"
                            : "border-emerald-500/25 bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                        }`}
                      >
                        {scopedStats.attentionCount > 0
                          ? `${scopedStats.attentionCount} 待修`
                          : scopedStats.disabledCount > 0
                            ? `${scopedStats.disabledCount} 停用`
                            : "完好"}
                      </span>
                    </div>
                  </button>
                </section>

                {showFilterToolbar && (
                  <div className="flex flex-wrap items-center justify-between gap-2 pt-0.5">
                    <div className="relative min-w-[180px] flex-1 max-w-xs">
                      <Search
                        className="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400 dark:text-zinc-500"
                        aria-hidden="true"
                      />
                      <input
                        type="text"
                        value={searchQuery}
                        onChange={(e) => setSearchQuery(e.target.value)}
                        placeholder="搜索昵称、UID、手机号或套餐..."
                        aria-label="搜索账号"
                        className="w-full rounded-lg border border-zinc-200 bg-white py-1 pr-7 pl-8 text-xs text-zinc-900 transition focus:border-brand-400 focus:outline-none dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100 dark:focus:border-brand-500"
                      />
                      {searchQuery && (
                        <button
                          type="button"
                          onClick={() => setSearchQuery("")}
                          aria-label="清空搜索"
                          className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-0.5 text-zinc-500 transition hover:bg-zinc-200/70 hover:text-zinc-800 dark:text-zinc-400 dark:hover:bg-zinc-700 dark:hover:text-zinc-200"
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
                              className={`chip ${active ? "chip-active" : ""}`}
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
                        <div className="inline-flex shrink-0 rounded-lg border border-zinc-200 bg-white p-0.5 dark:border-zinc-700 dark:bg-zinc-900">
                          <button
                            type="button"
                            onClick={() => setViewMode("grouped")}
                            title="按平台模块化分类展示"
                            className={`inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-[11px] font-medium transition ${
                              viewMode === "grouped"
                                ? "bg-zinc-100 text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                                : "text-zinc-500 transition hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-100"
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
                                ? "bg-zinc-100 text-zinc-900 shadow-2xs dark:bg-zinc-700 dark:text-zinc-100"
                                : "text-zinc-500 transition hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-100"
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

              <div className="px-5 py-3.5">
                <div className="mb-2.5 space-y-2.5">
                  <UpdateBanner onOpenSettings={() => setNav("settings")} />
                  <BatchClaimBanner />
                </div>

                {isInitializing && scopeAccounts.length === 0 ? (
                <div
                  aria-label="正在加载本地账号库"
                  className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(340px,100%),1fr))]"
                >
                  {[0, 1, 2].map((idx) => (
                    <div
                      key={idx}
                      className="surface-card h-44 p-4"
                    >
                      <div className="flex items-center gap-3">
                        <div className="skeleton h-9 w-9 rounded-xl" />
                        <div className="flex-1 space-y-2">
                          <div className="skeleton h-3 w-24 rounded" />
                          <div className="skeleton h-4 w-36 rounded" />
                        </div>
                      </div>
                      <div className="skeleton mt-6 h-2 w-full rounded-full" />
                      <div className="mt-6 flex justify-between">
                        <div className="skeleton h-6 w-24 rounded-lg" />
                        <div className="skeleton h-6 w-20 rounded-lg" />
                      </div>
                    </div>
                  ))}
                </div>
              ) : scopeAccounts.length === 0 ? (
                <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-300/80 bg-white/60 px-6 py-14 text-center dark:border-zinc-700/70 dark:bg-zinc-900/30">
                  {activeToolFilter ? (
                    <ToolBrandIcon toolId={activeToolFilter} size="lg" decorative />
                  ) : (
                    <div className="flex flex-wrap items-center justify-center gap-2">
                      {visibleTools.map((t) => (
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
                      className="btn btn-md btn-primary gap-1.5"
                    >
                      <HardDriveDownload className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      本机一键导入
                    </button>
                    <button
                      type="button"
                      onClick={() => openImportModal("webview_oauth")}
                      className="btn btn-md btn-secondary gap-1.5"
                    >
                      <Globe className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      内置授权登录
                    </button>
                  </div>
                </div>
              ) : filteredAccounts.length === 0 ? (
                <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-zinc-300/80 bg-white/45 px-6 py-12 text-center dark:border-zinc-700/70 dark:bg-zinc-900/25">
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
                    className="btn btn-sm btn-secondary mt-4"
                  >
                    清除筛选条件
                  </button>
                </div>
              ) : nav === "all" && viewMode === "grouped" ? (
                <div className="space-y-4">
                  {groupedModules.map((group) => (
                    <section
                      key={group.id}
                      className="rounded-2xl bg-zinc-100/55 p-3.5 dark:bg-zinc-900/35"
                    >
                      <div className="mb-3 flex flex-wrap items-center justify-between gap-2 px-0.5">
                        <div className="flex flex-wrap items-center gap-2">
                          <ToolBrandIcon toolId={group.id} size="sm" decorative />
                          <h2 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
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
                          className="btn btn-xs btn-secondary shrink-0 gap-1 whitespace-nowrap"
                        >
                          <Plus className="h-3 w-3 shrink-0" aria-hidden="true" />
                          添加账号
                        </button>
                      </div>

                      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(340px,100%),1fr))]">
                        {group.accounts.map((acc) => (
                          <AccountCard key={acc.id} account={acc} onOpenBindModal={openBindModal} />
                        ))}

                        {group.accounts.length === 1 && !isFiltered && (
                          <SecondAccountGhostCard
                            toolName={group.name}
                            scopeHint="新增副号后可在本模块内一键无缝切换桌面端登录态"
                            onClick={() => openImportModal("webview_oauth", group.id)}
                          />
                        )}
                      </div>
                    </section>
                  ))}
                </div>
              ) : (
                <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(340px,100%),1fr))]">
                  {sortedFlatAccounts.map((acc) => (
                    <AccountCard key={acc.id} account={acc} onOpenBindModal={openBindModal} />
                  ))}

                  {activeToolFilter &&
                    activeToolMeta &&
                    sortedFlatAccounts.length === 1 &&
                    !isFiltered && (
                      <SecondAccountGhostCard
                        toolName={activeToolMeta.name}
                        scopeHint="新增副号后可在本页一键无缝切换桌面端登录态"
                        onClick={() => openImportModal("webview_oauth", activeToolFilter)}
                      />
                    )}
                </div>
              )}
              </div>
            </>
          )}

          {nav === "agents" && (
            <AgentBindingsView highlightAgentId={highlightAgentId} />
          )}

          {nav === "routing" && (
            <RoutingTraceView
              onOpenSimulation={(poolId) => {
                setSimDefaultPoolId(poolId);
                setSimDefaultModelId(undefined);
                setSimModalOpen(true);
              }}
            />
          )}

          {nav === "logs" && (
            <LogsPanel
              currentLogTotal={currentLogTotal}
              filteredLogs={filteredLogs}
              groupedLogs={groupedLogs}
              selectedLogIds={selectedLogIds}
              setConfirmLogAction={setConfirmLogAction}
              toggleSelectLog={toggleSelectLog}
            />
          )}

          {nav === "settings" && (
            <SettingsView
              autoStartDescription={autoStartDescription}
              externalPlugins={externalPlugins}
              trustedDraft={trustedDraft}
              setTrustedDraft={setTrustedDraft}
              pathDrafts={pathDrafts}
              setPathDrafts={setPathDrafts}
              setPathDraftsDirty={setPathDraftsDirty}
              reloadExternalPlugins={reloadExternalPlugins}
              setConfirmLogAction={setConfirmLogAction}
            />
          )}
        </main>
      </div>

      <ColdSwitchModal />
      <PoolSimulationModal
        open={simModalOpen}
        defaultPoolId={simDefaultPoolId}
        defaultModelId={simDefaultModelId}
        onClose={() => setSimModalOpen(false)}
        onOpenRoutingView={() => setNav("routing")}
      />
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
            const res = await action.run();
            if (res !== false) {
              setSelectedLogIds([]);
            }
          }
        }}
      />
    </div>
  );
}
