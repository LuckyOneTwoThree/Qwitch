import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Activity,
  ArrowUpDown,
  CheckCircle2,
  CircleAlert,
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
  Table2,
  UserRound,
  X,
} from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { AccountCard } from "../components/AccountCard";
import { AccountLedger } from "../components/ledger/AccountLedger";
import { AccountDrawer } from "../components/ledger/AccountDrawer";
import { CommandPalette, type CommandItem } from "../components/CommandPalette";
import {
  formatQuotaNumber,
  sortLedgerAccounts,
  toQuotaView,
  LEDGER_SORT_LABEL,
  type LedgerSortKey,
} from "../components/ledger/ledgerModel";
import { BatchClaimBanner } from "../components/BatchClaimBanner";
import { ColdSwitchModal } from "../components/ColdSwitchModal";
import { ImportAccountModal } from "../components/ImportAccountModal";
import { ConfirmDialog } from "../components/ui/ConfirmDialog";
import { QwitchLogo, ToolBrandIcon } from "../components/ToolBrandIcon";
import { UpdateBanner } from "../components/UpdateBanner";
import { WindowsWindowControls } from "../components/WindowsWindowControls";
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
import { formatRelativeSyncTime } from "../domain/time";
import { formatLogDayLabel } from "./dashboard/types";
import type {
  AllViewMode,
  ConfirmLogAction,
  LogActionFilter,
  LogResultFilter,
  NavSection,
} from "./dashboard/types";
import { LogsPanel, LogsToolbar } from "./dashboard/LogsView";
import { SettingsView, type SettingsTabId } from "./dashboard/SettingsView";
import { AgentBindingsView } from "./dashboard/AgentBindingsView";
import { RoutingTraceView } from "./dashboard/RoutingTraceView";
import { PoolSimulationModal } from "../components/pool/PoolSimulationModal";
import { usePoolPrototype } from "../context/usePoolPrototypeStore";
import type { TargetAgentId, TrayNavigateTarget } from "../types/pool";

/** 客户端启停探测间隔：足够快以贴合「我刚关掉客户端」的心理预期，
 *  又不至于让 pgrep/ps 变成常驻 CPU 占用。 */
const CLIENT_STATUS_POLL_MS = 12_000;

const VIEW_PREF_KEY = "qwitch.accounts.view";

/** 读取持久化的账号视图偏好；未知值回落到「分组」——按平台模块化展示是主视角。 */
export function readStoredAccountView(): AllViewMode {
  try {
    const raw = localStorage.getItem(VIEW_PREF_KEY);
    if (raw === "ledger" || raw === "grouped" || raw === "flat") return raw;
  } catch {
    /* 隐私模式或存储被禁用时静默回落 */
  }
  return "grouped";
}

export function writeStoredAccountView(view: AllViewMode): void {
  try {
    localStorage.setItem(VIEW_PREF_KEY, view);
  } catch {
    /* 同上 */
  }
}

export function computeVisibleToolCount(
  containerWidth: number,
  allTabWidth: number,
  toolWidths: number[],
  moreButtonWidth: number,
  gap = 6
): number {
  if (containerWidth <= 0 || toolWidths.length === 0) {
    return toolWidths.length;
  }
  const totalToolsWidth = toolWidths.reduce((sum, w) => sum + w, 0);
  const fullWidth = allTabWidth + totalToolsWidth + gap * toolWidths.length;
  if (fullWidth <= containerWidth) {
    return toolWidths.length;
  }
  const available = containerWidth - allTabWidth - gap - moreButtonWidth;
  if (available <= 0) {
    return 0;
  }
  let used = 0;
  let count = 0;
  for (const w of toolWidths) {
    const nextUsed = used === 0 ? w : used + gap + w;
    if (nextUsed > available) {
      break;
    }
    used = nextUsed;
    count += 1;
  }
  return count;
}

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

export function Dashboard({
  externalNavTarget = null,
}: {
  externalNavTarget?: TrayNavigateTarget | null;
} = {}) {
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
    launchToolClient,
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
    gatewayBusy,
    detectAgentEnvironment,
    detectingAgents,
    ensureUpstreamModels,
  } = usePoolPrototype();

  const mainScrollRef = useRef<HTMLElement | null>(null);
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const [nav, setNav] = useState<NavSection>("all");
  const prevNavRef = useRef<NavSection>(nav);
  const ensuredAgentsRef = useRef(false);
  const [updateBannerDismissed, setUpdateBannerDismissed] = useState(false);
  const [confirmQuitOpen, setConfirmQuitOpen] = useState(false);
  const [simModalOpen, setSimModalOpen] = useState(false);
  const [simDefaultPoolId, setSimDefaultPoolId] = useState<string | undefined>(undefined);
  const [simDefaultModelId, setSimDefaultModelId] = useState<string | undefined>(undefined);
  const [highlightAgentId, setHighlightAgentId] = useState<TargetAgentId | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [settingsInitialTab, setSettingsInitialTab] = useState<SettingsTabId | undefined>(undefined);
  const [importDefaultMethod, setImportDefaultMethod] = useState<AuthMethod | undefined>(undefined);
  const [importPresetTool, setImportPresetTool] = useState<ToolId | undefined>(undefined);
  const [bindTargetAccount, setBindTargetAccount] = useState<AccountSummary | null>(null);
  const [launchingToolId, setLaunchingToolId] = useState<string | null>(null);
  const handleLaunchToolClient = useCallback(
    async (toolId: ToolId) => {
      if (launchingToolId) return;
      setLaunchingToolId(toolId);
      try {
        await launchToolClient(toolId);
      } finally {
        setLaunchingToolId(null);
      }
    },
    [launchToolClient, launchingToolId]
  );
  const [pathDrafts, setPathDrafts] = useState<Record<string, string>>({});
  const [pathDraftsDirty, setPathDraftsDirty] = useState(false);
  const [externalPlugins, setExternalPlugins] = useState<ExternalAdapterInfo[]>([]);
  const [trustedDraft, setTrustedDraft] = useState<string[]>([]);

  useEffect(() => {
    if (!externalNavTarget) return;
    if (externalNavTarget.nav) {
      setNav(externalNavTarget.nav);
    }
    setHighlightAgentId(externalNavTarget.agentId ?? null);
  }, [externalNavTarget]);

  // 首次进入 Agent 调度页时补一次 TTL 感知的模型探测：
  // 该页是「模型目录」的主要消费场景，若此时缓存已过期，用户会看到不完整的模型列表。
  useEffect(() => {
    if (nav !== "agents" || ensuredAgentsRef.current) return;
    ensuredAgentsRef.current = true;
    void ensureUpstreamModels();
  }, [ensureUpstreamModels, nav]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const fn = await listen<TrayNavigateTarget>("qwitch://navigate", (event) => {
          if (disposed) return;
          const payload = event.payload;
          if (payload?.nav) {
            setNav(payload.nav);
          }
          setHighlightAgentId(payload?.agentId ?? null);
        });
        if (disposed) fn();
        else unlisten = fn;
      } catch {
        /* ignore event listen errors outside full Tauri runtime */
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [isTauriRuntime]);

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
  const [viewMode, setViewMode] = useState<AllViewMode>(() => readStoredAccountView());
  const [ledgerSort, setLedgerSort] = useState<LedgerSortKey>("risk");
  const [selectedAccountIds, setSelectedAccountIds] = useState<Set<string>>(() => new Set());
  const [detailAccountId, setDetailAccountId] = useState<string | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const detailReturnFocusRef = useRef<HTMLElement | null>(null);
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

  // 侧栏只列「已有账号」的工具：0 账号的工具不占导航位（走「导入账号」入口），
  // 但空库时仍完整展示，避免新用户看到一个没有内容的侧栏。
  const sidebarTools = useMemo(
    () =>
      orderedVisibleTools.filter(
        (t) => (accountCountByTool.get(t.id) ?? 0) > 0 || accounts.length === 0
      ),
    [orderedVisibleTools, accountCountByTool, accounts.length]
  );

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

  // 客户端启停轮询：此前只在挂载 / 窗口重新聚焦 / 进入设置页时探测，
  // 用户在 Qwitch 里关掉某个客户端（或客户端自己退出）后，界面会一直停在
  // 「桌面端运行中」。这里补一条低频轮询，仅在页面可见时执行。
  useEffect(() => {
    if (!isTauriRuntime) return;
    const tick = () => {
      if (typeof document !== "undefined" && document.visibilityState !== "visible") return;
      void refreshClientStatuses();
    };
    const timer = window.setInterval(tick, CLIENT_STATUS_POLL_MS);
    const onVisible = () => {
      if (document.visibilityState === "visible") tick();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [isTauriRuntime, refreshClientStatuses]);

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
      if (statusFilter === "attention" && !isAccountNeedingAttention(acc, adapters)) {
        return false;
      }
      if (!q) return true;
      const haystack = [
        acc.label,
        acc.accountName ?? "",
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

  // 视图偏好落盘：下次启动保持用户上次选择的排布
  useEffect(() => {
    writeStoredAccountView(viewMode);
  }, [viewMode]);

  // 抽屉只持有 id；账号消失（删除/切作用域）时自动收起
  useEffect(() => {
    if (!detailAccountId) return;
    if (!accounts.some((a) => a.id === detailAccountId)) setDetailAccountId(null);
  }, [accounts, detailAccountId]);

  useEffect(() => {
    setDetailAccountId(null);
  }, [nav]);

  const detailAccount = useMemo(
    () => (detailAccountId ? accounts.find((a) => a.id === detailAccountId) ?? null : null),
    [accounts, detailAccountId]
  );

  // 选中集合按 id 保存，但账号被删除后必须剪掉，否则批量条会统计到幽灵账号
  useEffect(() => {
    setSelectedAccountIds((prev) => {
      if (prev.size === 0) return prev;
      const alive = new Set(accounts.map((a) => a.id));
      const next = new Set([...prev].filter((id) => alive.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [accounts]);

  useEffect(() => {
    setSelectedLogIds([]);
    setConfirmLogAction(null);
    mainScrollRef.current?.scrollTo({ top: 0 });
  }, [logScope, nav]);

  const filteredLogs = useMemo(() => {
    if (nav !== "logs") return [];
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
  }, [nav, logs, logActionFilter, logResultFilter, logSearch, adapters]);

  useEffect(() => {
    if (nav !== "logs") return;
    const validIds = new Set(filteredLogs.map((l) => l.id));
    setSelectedLogIds((prev) => {
      const valid = prev.filter((id) => validIds.has(id));
      return valid.length === prev.length ? prev : valid;
    });
  }, [nav, filteredLogs]);

  // 全局桌面快捷键：Cmd/Ctrl+K 聚焦搜索，Cmd/Ctrl+, 打开设置，Cmd/Ctrl+1~4 切换主工作区
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      // 只拦模态层：账号详情抽屉是非模态检视面板，开着时仍应能用 ⌘K
      if (document.querySelector('[role="alertdialog"], [role="dialog"][aria-modal="true"]')) return;
      const target = e.target as HTMLElement | null;
      const isEditing =
        Boolean(target) &&
        (target!.tagName === "INPUT" ||
          target!.tagName === "TEXTAREA" ||
          target!.tagName === "SELECT" ||
          target!.isContentEditable);

      const key = e.key.toLowerCase();
      if (key === "k") {
        e.preventDefault();
        setPaletteOpen(true);
        return;
      }
      if (isEditing) return;
      if (key === ",") {
        e.preventDefault();
        setNav("settings");
      } else if (key === "1") {
        e.preventDefault();
        setNav("all");
      } else if (key === "2") {
        e.preventDefault();
        setNav("agents");
      } else if (key === "3") {
        e.preventDefault();
        setNav("routing");
      } else if (key === "4") {
        e.preventDefault();
        setNav("logs");
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [nav]);

  // 只有 macOS 走 `titleBarStyle: "Overlay"`（见 tauri.conf.json），窗口没有原生标题栏，
  // 因此需要为左上角红绿灯按钮预留高度并自绘拖拽区。Windows / Linux 仍是原生装饰，
  // 再预留这 40px 就变成「原生标题栏 + 一条空白」的双层标题。
  const reservesTrafficLightSpace = useMemo(() => {
    if (typeof navigator === "undefined") return true;
    return !/windows|linux/i.test(navigator.userAgent);
  }, []);
  const isWindows = useMemo(() => {
    if (typeof navigator === "undefined") return false;
    return /windows/i.test(navigator.userAgent);
  }, []);
  const usesCustomTitleBar = reservesTrafficLightSpace || isWindows;

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

  // 用 useCallback 固定引用：它会进入 ⌘K 面板的依赖数组，
  // 普通函数每次渲染都是新引用，会让整个面板列表无谓重建。
  const openImportModal = useCallback(
    (defaultMethod?: AuthMethod, toolId?: ToolId) => {
      setBindTargetAccount(null);
      setImportDefaultMethod(defaultMethod);
      setImportPresetTool(toolId ?? activeToolFilter);
      setImportOpen(true);
    },
    [activeToolFilter]
  );

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

  // ⌘K 面板：账号优先（输入即定位），动作与导航兜底
  const paletteItems = useMemo<CommandItem[]>(() => {
    // 与台账同序：最该处理的账号排在最前，⌘K 打开即可回车
    const accountItems: CommandItem[] = sortLedgerAccounts(scopeAccounts, "risk").map((acc) => {
      const q = toQuotaView(acc);
      const meta =
        q.pct !== null
          ? `${q.pct}%`
          : q.kind === "balance"
            ? `${formatQuotaNumber(q.remaining ?? 0)} ${q.unit}`
            : "未查额";
      return {
        id: `acc:${acc.id}`,
        group: "账号",
        label: acc.label,
        meta,
        keywords: `${acc.uidMasked} ${acc.emailMasked ?? ""} ${getToolBrandMeta(acc.toolId, adapters).name}`,
        icon: UserRound,
        toolId: acc.toolId,
        run: () => {
          detailReturnFocusRef.current = null;
          setDetailAccountId(acc.id);
        },
      };
    });

    const actionItems: CommandItem[] = [
      {
        id: "act:claim",
        group: "操作",
        label: scopedStats.claimableCount > 0
          ? `一键领取今日待领（${scopedStats.claimableCount}）`
          : "一键领取今日待领",
        icon: Sparkles,
        keywords: "claim 签到 领取 daily",
        run: () => void claimDailyBatch(undefined, activeToolFilter),
      },
      {
        id: "act:refresh",
        group: "操作",
        label: "刷新全部额度",
        icon: RefreshCw,
        keywords: "refresh 查额 quota",
        run: () => void refreshAllQuotas(activeToolFilter),
      },
      {
        id: "act:import",
        group: "操作",
        label: "导入账号",
        icon: Plus,
        keywords: "import add 新增",
        run: () => openImportModal(),
      },
      {
        id: "act:gateway",
        group: "操作",
        label: gateway.running ? "停止本地网关" : "启动本地网关",
        icon: Power,
        keywords: "gateway 网关 proxy",
        run: () => void toggleGateway(),
      },
    ];

    const navItems: CommandItem[] = [
      { id: "nav:agents", group: "导航", label: "Agent 接入与调度", icon: Link2, keywords: "agent pool 号池", run: () => setNav("agents") },
      { id: "nav:routing", group: "导航", label: "路由观测", icon: Radio, keywords: "routing trace 路由", run: () => setNav("routing") },
      { id: "nav:logs", group: "导航", label: "操作日志", icon: ScrollText, keywords: "logs 日志 审计", run: () => setNav("logs") },
      { id: "nav:settings", group: "导航", label: "设置与环境", icon: Settings, keywords: "settings 设置 偏好", run: () => setNav("settings") },
    ];

    return [...accountItems, ...actionItems, ...navItems];
  }, [
    scopeAccounts,
    adapters,
    scopedStats.claimableCount,
    activeToolFilter,
    gateway.running,
    claimDailyBatch,
    refreshAllQuotas,
    toggleGateway,
    openImportModal,
  ]);

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

  // 副标题不再复述指标条里已经有的数字，改为「指标条没有的信息」：
  // 数据新鲜度与工具覆盖面。
  const latestSyncLabel = useMemo(() => {
    let newest = 0;
    for (const acc of scopeAccounts) {
      const iso = acc.latestQuota?.fetchedAt;
      if (!iso) continue;
      const t = Date.parse(iso);
      if (Number.isFinite(t) && t > newest) newest = t;
    }
    if (!newest) return "尚未同步";
    return formatRelativeSyncTime(new Date(newest).toISOString());
  }, [scopeAccounts]);

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
      return `${activeToolMeta.tagline} · 最近同步 ${latestSyncLabel}`;
    }
    return `本地加密库 · ${sidebarTools.length} 个工具 · 最近同步 ${latestSyncLabel}`;
  }, [
    nav,
    latestSyncLabel,
    sidebarTools.length,
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

  const isInteractiveTarget = (target: EventTarget | null) => {
    const el = target as HTMLElement | null;
    return (
      !!el &&
      !!el.closest('button, a, input, select, textarea, [role="button"], [role="tab"]')
    );
  };
  const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

  // 手动触发窗口拖动（比 data-tauri-drag-region 更可靠，且可精确排除按钮区域）
  // 仅在鼠标真正移动超过阈值后才进入系统拖拽，避免“按下即拖”吞掉单击/双击
  const onTitleBarMouseDown = (e: {
    button?: number;
    clientX?: number;
    clientY?: number;
    target: EventTarget | null;
  }) => {
    if (e.button !== 0 || isInteractiveTarget(e.target) || !isTauri()) return;
    const startX = e.clientX ?? 0;
    const startY = e.clientY ?? 0;
    let moved = false;
    const cleanup = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", cleanup);
    };
    const onMove = (ev: MouseEvent) => {
      if (moved) return;
      if (Math.hypot(ev.clientX - startX, ev.clientY - startY) < 4) return;
      moved = true;
      cleanup();
      try {
        void getCurrentWindow().startDragging();
      } catch {
        /* ignore */
      }
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", cleanup);
  };

  const onTitleBarDoubleClick = (e?: { target: EventTarget | null }) => {
    if (isInteractiveTarget(e?.target ?? null) || !isTauri()) return;
    void (async () => {
      try {
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
    <div className="relative flex h-full overflow-hidden bg-[#f6f7f9] text-zinc-900 dark:bg-[#0c0d10] dark:text-zinc-100">

      <aside className="flex h-full w-52 shrink-0 flex-col border-r border-zinc-200/60 bg-white/95 px-2.5 pb-3 pt-0 dark:border-white/[0.06] dark:bg-[#111216]">
        {reservesTrafficLightSpace && (
          <div onMouseDown={onTitleBarMouseDown} onDoubleClick={onTitleBarDoubleClick} className="h-10 shrink-0 -mx-2.5" />
        )}
        <div className="flex min-h-0 flex-1 flex-col justify-between">
        {/* 工作台区独立滚动：工具增多时不会把「系统与审计」顶出视口；
            底部渐隐提示还有未显示的条目，避免行被硬切一半 */}
        <div className="mask-fade-b min-h-0 flex-1 overflow-y-auto pb-2">
          <div
            onMouseDown={isWindows ? onTitleBarMouseDown : undefined}
            onDoubleClick={isWindows ? onTitleBarDoubleClick : undefined}
            className={`mb-4 flex items-center gap-2.5 px-1.5 ${isWindows ? "pt-2.5" : ""}`}
          >
            <QwitchLogo size="md" />
            <div className="min-w-0">
              <div className="text-sm font-semibold tracking-tight">Qwitch</div>
              <div className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                号池与多模型网关
              </div>
            </div>
          </div>

          <div className="space-y-5">
            <div>
              <div className="mb-1.5 px-1.5 text-[11px] font-semibold tracking-wider text-zinc-500 uppercase dark:text-zinc-400">
                工作台与路由
              </div>
              <nav aria-label="主功能导航" className="space-y-1">
                <button
                  type="button"
                  onClick={() => {
                    setHighlightAgentId(null);
                    // 始终回到该分区的根作用域（全部账号）。
                    // 旧实现在工具子页（isAccountSection 仍为 true）时不会切换，
                    // 导致侧栏「账号资产」变成死路：点击无反应却仍显示为当前页。
                    setNav("all");
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

                {/* 工具作用域 = 侧栏二级导航。
                    原先用顶部横向 Tabs，9 个工具就要靠 computeVisibleToolCount()
                    逐个 offsetWidth 测量 + 「展开 N」兜底；纵向列表天然可扩展，
                    也不再需要把布局宽度塞进状态。 */}
                <div
                  role="navigation"
                  aria-label="账号视图导航"
                  className="mt-0.5 mb-1 ml-2 space-y-px border-l border-zinc-200/70 pl-1.5 dark:border-white/[0.08]"
                >
                  <button
                    type="button"
                    onClick={() => {
                      setHighlightAgentId(null);
                      setNav("all");
                    }}
                    aria-current={nav === "all" ? "page" : undefined}
                    className={`flex w-full items-center gap-2 rounded-md px-2 py-1 text-[11.5px] transition ${
                      nav === "all"
                        ? "bg-zinc-900/[0.055] font-semibold text-zinc-900 dark:bg-white/[0.08] dark:text-zinc-50"
                        : "text-zinc-500 hover:bg-zinc-900/[0.035] hover:text-zinc-800 dark:text-zinc-400 dark:hover:bg-white/[0.05] dark:hover:text-zinc-100"
                    }`}
                  >
                    <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-[5px] bg-zinc-100 text-zinc-500 ring-1 ring-black/6 dark:bg-zinc-800/80 dark:text-zinc-400 dark:ring-white/10">
                      <Layers className="h-2.5 w-2.5" aria-hidden="true" />
                    </span>
                    <span className="min-w-0 flex-1 truncate text-left">全部账号</span>
                    <span
                      className={`shrink-0 tabular-nums text-[10.5px] ${
                        nav === "all"
                          ? "text-zinc-600 dark:text-zinc-300"
                          : "text-zinc-400 dark:text-zinc-500"
                      }`}
                    >
                      {accounts.length}
                    </span>
                  </button>

                  {sidebarTools.map((tool) => {
                    const count = accountCountByTool.get(tool.id) ?? 0;
                    const running = Boolean(clientStatuses[tool.id]?.isRunning);
                    const active = nav === tool.id;
                    return (
                      <button
                        key={tool.id}
                        type="button"
                        onClick={() => {
                          setHighlightAgentId(null);
                          setNav(tool.id);
                        }}
                        aria-current={active ? "page" : undefined}
                        className={`flex w-full items-center gap-2 rounded-md px-2 py-1 text-[11.5px] transition ${
                          active
                            ? "bg-zinc-900/[0.055] font-semibold text-zinc-900 dark:bg-white/[0.08] dark:text-zinc-50"
                            : "text-zinc-500 hover:bg-zinc-900/[0.035] hover:text-zinc-800 dark:text-zinc-400 dark:hover:bg-white/[0.05] dark:hover:text-zinc-100"
                        }`}
                      >
                        <ToolBrandIcon toolId={tool.id} size="xs" decorative />
                        <span className="min-w-0 flex-1 truncate text-left">{tool.name}</span>
                        {running && count > 0 && (
                          <span
                            aria-hidden="true"
                            title="客户端运行中"
                            className="h-1 w-1 shrink-0 rounded-full bg-emerald-500"
                          />
                        )}
                        <span
                          className={`shrink-0 tabular-nums text-[10.5px] ${
                            active
                              ? "text-zinc-600 dark:text-zinc-300"
                              : "text-zinc-400 dark:text-zinc-500"
                          }`}
                        >
                          {count}
                        </span>
                      </button>
                    );
                  })}
                </div>

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

        </div>
        </div>

        <div className="shrink-0 pt-3">
              <div className="mb-1.5 px-1.5 text-[11px] font-semibold tracking-wider text-zinc-500 uppercase dark:text-zinc-400">
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
                      className="ml-auto h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500"
                      title={`发现新版本 v${updateInfo.latestVersion}`}
                      aria-label={`发现新版本 v${updateInfo.latestVersion}`}
                    />
                  )}
                </button>
              </nav>
            </div>
        <div className="space-y-2 border-t border-zinc-200/60 pt-3 dark:border-white/[0.06]">
          {/* 本地网关快捷启停控制（以启动/关闭为主交互，隐藏原始端口号） */}
          <div className="flex items-center justify-between gap-2 rounded-xl bg-zinc-100/80 px-2 py-2 text-xs transition-colors hover:bg-zinc-100 dark:bg-white/[0.04] dark:hover:bg-white/[0.06]">
            <button
              type="button"
              onClick={() => setNav("agents")}
              title={`本地网关（${gateway.host}:${gateway.port}），点击进入接入与调度`}
              className="flex min-w-0 items-center gap-2 text-left font-medium text-zinc-700 hover:text-zinc-950 dark:text-zinc-200 dark:hover:text-white"
            >
              <span
                aria-hidden="true"
                className={`h-2 w-2 shrink-0 rounded-full ${
                  gateway.running
                    ? "bg-emerald-500 shadow-[0_0_0_3px_rgba(16,185,129,0.18)]"
                    : "bg-zinc-400"
                }`}
              />
              <span className="truncate">
                {gateway.running ? "网关运行中" : "网关已关闭"}
              </span>
            </button>
            <button
              type="button"
              onClick={() => void toggleGateway()}
              disabled={gatewayBusy}
              aria-busy={gatewayBusy}
              aria-label={gateway.running ? "关闭本地网关" : "启动本地网关"}
              className={`btn btn-xs shrink-0 ${
                gateway.running
                  ? "bg-zinc-200/80 text-zinc-700 hover:bg-rose-500/15 hover:text-rose-700 dark:bg-zinc-800 dark:text-zinc-200 dark:hover:bg-rose-500/20 dark:hover:text-rose-300"
                  : "btn-primary"
              }`}
            >
              {gatewayBusy ? (
                <Loader2 className="h-3 w-3 shrink-0 animate-spin" aria-hidden="true" />
              ) : (
                <Power className="h-3 w-3 shrink-0" aria-hidden="true" />
              )}
              {gatewayBusy ? "处理中" : gateway.running ? "关闭" : "启动"}
            </button>
          </div>

          {/* 精简版本与加密库底栏 */}
          {hasUpdate ? (
            <div className="rounded-xl border border-brand-500/30 bg-brand-500/[0.06] p-2 text-[11px] dark:border-brand-400/25 dark:bg-brand-400/[0.08]">
              <div className="flex items-center justify-between gap-1.5">
                <button
                  type="button"
                  onClick={() => setNav("settings")}
                  className="truncate font-medium text-brand-800 hover:underline dark:text-brand-200"
                >
                  新版本可升级
                </button>
                <button
                  type="button"
                  onClick={() => void runUpdatePrimaryAction()}
                  disabled={updateInstalling}
                  title={updatePrimaryLabel}
                  aria-label={updatePrimaryLabel}
                  className="inline-flex shrink-0 items-center gap-1 rounded-md bg-brand-700 px-1.5 py-0.5 font-mono text-[11px] font-semibold text-white transition hover:bg-brand-800 disabled:opacity-60 dark:bg-brand-400 dark:text-zinc-950"
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

          <div className="flex items-center justify-between gap-2 px-1.5 text-[11px] text-zinc-500 dark:text-zinc-400">
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
              onClick={() => {
                if (gateway.running || batchProgress?.running) {
                  setConfirmQuitOpen(true);
                } else {
                  void quitApp();
                }
              }}
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
        <header
          onMouseDown={usesCustomTitleBar ? onTitleBarMouseDown : undefined}
          onDoubleClick={usesCustomTitleBar ? onTitleBarDoubleClick : undefined}
          className="z-20 flex shrink-0 items-start justify-between gap-x-2 border-b border-zinc-200/60 bg-white/80 px-5 pt-2 pb-3 backdrop-blur-xl dark:border-white/[0.06] dark:bg-[#111216]/80"
        >
          <div className="flex min-w-0 flex-1 flex-wrap items-center justify-between gap-x-3 gap-y-2">
            <div className="flex min-w-[160px] flex-1 items-center gap-2.5">
              {activeToolFilter && <ToolBrandIcon toolId={activeToolFilter} size="sm" decorative />}
              <div className="min-w-0">
                <h1 className="truncate text-lg font-semibold tracking-tight">{headerTitle}</h1>
                <p className="truncate text-xs text-zinc-500 dark:text-zinc-400">
                  {headerSubtitle}
                </p>
              </div>
            </div>

            <div className="flex flex-wrap items-center justify-end gap-1.5">
            <BatchClaimBanner inline />
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
                    ) : scopedStats.unclaimedSnapshotMissingCount > 0 ? (
                      <button
                        type="button"
                        onClick={() => setStatusFilter("attention")}
                        title="这些账号缺本机快照，无法冷切领取；请在卡片上「补快照」或重新导入本机"
                        className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap"
                      >
                        <CircleAlert className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {`${scopedStats.unclaimedSnapshotMissingCount} 待补快照`}
                      </button>
                    ) : scopedStats.claimBlockedCount > 0 ? (
                      // 非交互徽章：今日不可领是正常业务状态（按设备每日一次），
                      // 不是待完善，点击过滤没有意义
                      <span
                        title="该活动按设备限制每日一次：本设备今天已有账号完成签到，其他账号卡片上有说明；明日资格刷新后可再试"
                        className="inline-flex items-center gap-1.5 rounded-[var(--radius-control)] border border-zinc-300/70 bg-zinc-100/80 px-2.5 py-1.5 text-xs font-medium text-zinc-600 dark:border-zinc-600/40 dark:bg-zinc-800/60 dark:text-zinc-300"
                      >
                        <CircleAlert className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {`${scopedStats.claimBlockedCount} 今日不可领`}
                      </span>
                    ) : null}
                  </>
                )}

                {!activeToolFilter && (
                  <button
                    type="button"
                    onClick={() => openImportModal()}
                    className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap"
                  >
                    <Plus className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    导入账号
                  </button>
                )}
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
                  onClick={() => void detectAgentEnvironment()}
                  disabled={detectingAgents}
                  aria-busy={detectingAgents}
                  className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap disabled:cursor-not-allowed disabled:opacity-60"
                >
                  <RefreshCw
                    className={`h-3.5 w-3.5 shrink-0 ${detectingAgents ? "animate-spin" : ""}`}
                    aria-hidden="true"
                  />
                  {detectingAgents ? "探测中…" : "重新探测"}
                </button>
                <button
                  type="button"
                  onClick={() => void toggleGateway()}
                  disabled={gatewayBusy}
                  aria-busy={gatewayBusy}
                  className={`btn btn-sm gap-1.5 whitespace-nowrap ${
                    gateway.running ? "btn-secondary" : "btn-primary"
                  } disabled:cursor-not-allowed disabled:opacity-60`}
                >
                  {gatewayBusy ? (
                    <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden="true" />
                  ) : (
                    <Power className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  )}
                  {gatewayBusy ? "处理中…" : gateway.running ? "停止网关" : "启动网关"}
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
          </div>

          {isWindows && (
            <div className="flex shrink-0 items-center self-start pt-0.5">
              <WindowsWindowControls />
            </div>
          )}
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

        <main
          ref={mainScrollRef}
          className={`min-h-0 flex-1 ${
            isAccountSection
              ? "overflow-y-auto"
              : nav === "pools" || nav === "agents" || nav === "routing"
                ? "overflow-y-auto px-4 py-3 @3xl:flex @3xl:flex-col @3xl:overflow-hidden"
                : "overflow-y-auto px-5 py-3.5"
          }`}
        >
          {isAccountSection && (
            <>
              {/* 统一吸顶控制区（紧贴顶栏无缝衔接，内部包含工具切换 / 核心指标 / 搜索筛选，且全区支持滚轮滚动） */}
              <div className="sticky top-0 z-10 space-y-2.5 border-b border-zinc-200/50 bg-[#f6f7f9]/85 px-5 py-3 backdrop-blur-md dark:border-white/[0.05] dark:bg-[#0c0d10]/85">
                {showFilterToolbar && (
                  <div className="flex flex-wrap items-center justify-between gap-2 pt-0.5">
                    <div className="relative min-w-[180px] flex-1 max-w-xs">
                      <Search
                        className="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400 dark:text-zinc-500"
                        aria-hidden="true"
                      />
                      <input
                        ref={searchInputRef}
                        type="text"
                        value={searchQuery}
                        onChange={(e) => setSearchQuery(e.target.value)}
                        placeholder="搜索昵称、UID、手机号或套餐..."
                        aria-label="搜索账号"
                        className="field-input w-full py-1 pr-10 pl-8"
                      />
                      {searchQuery ? (
                        <button
                          type="button"
                          onClick={() => setSearchQuery("")}
                          aria-label="清空搜索"
                          className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-0.5 text-zinc-500 transition hover:bg-zinc-200/70 hover:text-zinc-800 dark:text-zinc-400 dark:hover:bg-zinc-700 dark:hover:text-zinc-200"
                        >
                          <X className="h-3 w-3" aria-hidden="true" />
                        </button>
                      ) : (
                        <kbd
                          aria-hidden="true"
                          className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 rounded border border-zinc-200 bg-zinc-50 px-1 py-0.5 font-mono text-[10px] leading-none text-zinc-400 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-500"
                        >
                          {reservesTrafficLightSpace ? "⌘K" : "Ctrl K"}
                        </kbd>
                      )}
                    </div>

                    {viewMode === "ledger" && (
                      <button
                        type="button"
                        onClick={() =>
                          setLedgerSort((prev) => {
                            const order: LedgerSortKey[] = ["risk", "quota-asc", "quota-desc", "recent"];
                            return order[(order.indexOf(prev) + 1) % order.length];
                          })
                        }
                        title="切换列表排序方式"
                        className="inline-flex shrink-0 items-center gap-1 rounded-lg px-2 py-1 text-[11px] font-medium text-zinc-600 transition hover:bg-zinc-900/[0.05] hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-white/[0.06] dark:hover:text-zinc-50"
                      >
                        <ArrowUpDown className="h-3 w-3 shrink-0" aria-hidden="true" />
                        {LEDGER_SORT_LABEL[ledgerSort]}
                      </button>
                    )}

                    {/* 视图切换：台账为默认，卡片两档保留给需要富信息的场景。
                        状态筛选已收敛到下方指标条，此处不再重复第二套胶囊。 */}
                    <div
                      role="group"
                      aria-label="视图模式"
                      className="ml-auto inline-flex shrink-0 rounded-lg bg-zinc-200/65 p-0.5 dark:bg-white/[0.06]"
                    >
                      {(
                        nav === "all"
                          ? ([
                              { id: "grouped", label: "分组", icon: LayoutList, title: "按平台分组的卡片墙（默认）" },
                              { id: "ledger", label: "列表", icon: Table2, title: "账号列表：一屏看完全部账号与水位" },
                              { id: "flat", label: "平铺", icon: LayoutGrid, title: "不分组的卡片平铺" },
                            ] as const)
                          : ([
                              { id: "flat", label: "卡片", icon: LayoutGrid, title: "卡片视图（默认）" },
                              { id: "ledger", label: "列表", icon: Table2, title: "账号列表：一屏看完全部账号与水位" },
                            ] as const)
                      ).map((v) => {
                        // 工具作用域下 grouped 会回落成 flat，这里按真实渲染结果标注选中态
                        const shown = nav === "all" || viewMode === "ledger" ? viewMode : "flat";
                        const active = shown === v.id;
                        return (
                          <button
                            key={v.id}
                            type="button"
                            aria-pressed={active}
                            onClick={() => setViewMode(v.id)}
                            title={v.title}
                            className={`inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-[11px] font-medium transition ${
                              active
                                ? "bg-white text-zinc-900 shadow-2xs dark:bg-zinc-800 dark:text-zinc-100"
                                : "text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-100"
                            }`}
                          >
                            <v.icon className="h-3 w-3 shrink-0" aria-hidden="true" />
                            {v.label}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}
                {/* 指标条即状态筛选：随吸顶区一起固定，滚动时始终可切换作用域。
                    它曾经在内容流里，滚一屏就消失，用户要滚回顶部才能改筛选。 */}
              {/* 紧凑单行指标条：4 项指标收敛为一体式分层条带，兼顾窄高窗口空间与单行不折行 */}
              <section
                aria-label="当前作用域核心指标"
                className="surface-card grid grid-cols-2 divide-y divide-slate-900/[0.06] overflow-hidden rounded-xl @3xl:grid-cols-4 @3xl:divide-x @3xl:divide-y-0 dark:divide-white/[0.06]"
              >
                <button
                  type="button"
                  onClick={() => setStatusFilter("all")}
                  aria-pressed={statusFilter === "all"}
                  title="显示全部账号"
                  className={`relative flex min-w-0 items-center justify-between gap-2 px-3 py-2.5 text-left transition-colors ${
                    statusFilter === "all"
                      ? "bg-brand-500/[0.06] after:absolute after:inset-x-3 after:bottom-0 after:h-[2px] after:rounded-full after:bg-brand-500 dark:bg-brand-400/[0.08]"
                      : "hover:bg-slate-900/[0.025] dark:hover:bg-white/[0.03]"
                  }`}
                >
                  <div className="flex min-w-0 items-center gap-1.5">
                    <Layers
                      className="h-3.5 w-3.5 shrink-0 text-brand-600 dark:text-brand-400"
                      aria-hidden="true"
                    />
                    <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                      {activeToolMeta ? `${activeToolMeta.name}` : "纳管账号"}
                    </span>
                  </div>
                  <div className="flex shrink-0 items-baseline gap-0.5">
                    <span className="text-sm leading-none font-semibold tracking-tight tabular-nums text-zinc-900 dark:text-zinc-50">
                      {scopedStats.total}
                    </span>
                    <span className="text-[11px] text-zinc-500 dark:text-zinc-400">个</span>
                  </div>
                </button>

                <button
                  type="button"
                  onClick={() =>
                    setStatusFilter((prev) => (prev === "active_client" ? "all" : "active_client"))
                  }
                  aria-pressed={statusFilter === "active_client"}
                  title="仅显示本机客户端正在使用的账号"
                  className={`relative flex min-w-0 items-center justify-between gap-2 px-3 py-2.5 text-left transition-colors ${
                    statusFilter === "active_client"
                      ? "bg-emerald-500/[0.06] after:absolute after:inset-x-3 after:bottom-0 after:h-[2px] after:rounded-full after:bg-emerald-500 dark:bg-emerald-400/[0.08]"
                      : "hover:bg-slate-900/[0.025] dark:hover:bg-white/[0.03]"
                  }`}
                >
                  <div className="flex min-w-0 items-center gap-1.5">
                    <MonitorCheck
                      className="h-3.5 w-3.5 shrink-0 text-emerald-600 dark:text-emerald-400"
                      aria-hidden="true"
                    />
                    <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                      本机挂载
                    </span>
                  </div>
                  <div className="flex shrink-0 items-center gap-1.5">
                    <span className="text-sm leading-none font-semibold tabular-nums text-zinc-900 dark:text-zinc-50">
                      {scopedStats.activeClientsCount}
                      <span className="text-[11px] font-normal text-zinc-400 dark:text-zinc-500">
                        /{scopedStats.scopeToolCapacity}
                      </span>
                    </span>
                    <span
                      className={`shrink-0 whitespace-nowrap rounded-md px-1.5 py-0.5 text-[11px] leading-none font-medium ${
                        scopedStats.total > 0 && scopedStats.runningClientsCount > 0
                          ? "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300"
                          : "bg-zinc-900/[0.055] text-zinc-600 dark:bg-white/[0.08] dark:text-zinc-300"
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
                  title="仅显示今日待领取额度的账号"
                  className={`relative flex min-w-0 items-center justify-between gap-2 px-3 py-2.5 text-left transition-colors ${
                    statusFilter === "claimable"
                      ? "bg-sky-500/[0.06] after:absolute after:inset-x-3 after:bottom-0 after:h-[2px] after:rounded-full after:bg-sky-500 dark:bg-sky-400/[0.08]"
                      : "hover:bg-slate-900/[0.025] dark:hover:bg-white/[0.03]"
                  }`}
                >
                  <div className="flex min-w-0 items-center gap-1.5">
                    <CheckCircle2
                      className="h-3.5 w-3.5 shrink-0 text-sky-600 dark:text-sky-400"
                      aria-hidden="true"
                    />
                    <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                      今日待领
                    </span>
                  </div>
                  <div className="flex shrink-0 items-center gap-1.5">
                    <span className="text-sm leading-none font-semibold tabular-nums text-zinc-900 dark:text-zinc-50">
                      {scopedStats.claimableCount}
                    </span>
                    <span
                      className={`shrink-0 whitespace-nowrap rounded-md px-1.5 py-0.5 text-[11px] leading-none font-medium ${
                        scopedStats.claimableCount > 0
                          ? "bg-sky-500/12 text-sky-700 dark:bg-sky-500/15 dark:text-sky-300"
                          : scopedStats.unclaimedAuthMissingCount > 0
                            ? "bg-amber-500/12 text-amber-800 dark:bg-amber-500/15 dark:text-amber-300"
                            : scopedStats.allClaimed
                              ? "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300"
                              : "bg-zinc-900/[0.055] text-zinc-600 dark:bg-white/[0.08] dark:text-zinc-300"
                      }`}
                    >
                      {scopedStats.unclaimedAuthMissingCount > 0 &&
                      scopedStats.eligibleClaimTotal === 0
                        ? "授权后可签"
                        : scopedStats.eligibleClaimTotal > 0
                          ? `已签 ${scopedStats.claimedCount}/${scopedStats.eligibleClaimTotal}`
                          : "无可签"}
                    </span>
                  </div>
                </button>

                <button
                  type="button"
                  onClick={() =>
                    setStatusFilter((prev) => (prev === "attention" ? "all" : "attention"))
                  }
                  aria-pressed={statusFilter === "attention"}
                  title="仅显示凭据过期、风险暂停或待授权的账号"
                  className={`relative flex min-w-0 items-center justify-between gap-2 px-3 py-2.5 text-left transition-colors ${
                    statusFilter === "attention"
                      ? "bg-amber-500/[0.06] after:absolute after:inset-x-3 after:bottom-0 after:h-[2px] after:rounded-full after:bg-amber-500 dark:bg-amber-400/[0.08]"
                      : "hover:bg-slate-900/[0.025] dark:hover:bg-white/[0.03]"
                  }`}
                >
                  <div className="flex min-w-0 items-center gap-1.5">
                    <Activity
                      className={`h-3.5 w-3.5 shrink-0 ${
                        scopedStats.attentionCount > 0
                          ? "text-amber-600 dark:text-amber-400"
                          : "text-emerald-600 dark:text-emerald-400"
                      }`}
                      aria-hidden="true"
                    />
                    <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                      待完善
                    </span>
                  </div>
                  <div className="flex shrink-0 items-center gap-1.5">
                    <span className="text-sm leading-none font-semibold tabular-nums text-zinc-900 dark:text-zinc-50">
                      {scopedStats.attentionCount}
                    </span>
                    <span
                      className={`shrink-0 whitespace-nowrap rounded-md px-1.5 py-0.5 text-[11px] leading-none font-medium ${
                        scopedStats.attentionCount > 0
                          ? "bg-amber-500/12 text-amber-800 dark:bg-amber-500/15 dark:text-amber-300"
                          : "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300"
                      }`}
                    >
                      {scopedStats.attentionCount > 0
                        ? `正常 ${scopedStats.healthyCount}`
                        : scopedStats.disabledCount > 0
                          ? `停用 ${scopedStats.disabledCount}`
                          : "全部正常"}
                    </span>
                  </div>
                </button>
              </section>
              </div>


              <div className="space-y-3.5 px-5 py-3.5">
                <UpdateBanner
                  onOpenSettings={() => setNav("settings")}
                  dismissed={updateBannerDismissed}
                  onDismiss={() => setUpdateBannerDismissed(true)}
                />

                {/* 单个工具的模块条：进入某个工具后，分组头就没了，
                    「快捷启动 / 添加账号」也就跟着消失。把模块级操作留在
                    列表上方，和分组视图里的模块头保持一致的位置与语义。 */}
                {activeToolFilter && activeToolMeta && scopeAccounts.length > 0 && (
                  <section
                    aria-label={`${activeToolMeta.name} 模块操作`}
                    className="flex flex-wrap items-center justify-between gap-2 px-0.5"
                  >
                    <div className="flex min-w-0 flex-wrap items-center gap-2">
                      <span
                        aria-hidden="true"
                        className={`h-3.5 w-[3px] shrink-0 rounded-full ${
                          clientStatuses[activeToolFilter]?.isRunning
                            ? "bg-emerald-500"
                            : "bg-zinc-300 dark:bg-zinc-700"
                        }`}
                      />
                      <ToolBrandIcon toolId={activeToolFilter} size="sm" decorative />
                      <h2 className="text-[13px] font-semibold tracking-tight text-zinc-900 dark:text-zinc-100">
                        {activeToolMeta.name}
                      </h2>
                      <span className="text-[11px] font-medium tabular-nums text-zinc-500 dark:text-zinc-400">
                        {scopeAccounts.length} 个账号
                      </span>
                      <span className="text-zinc-300 dark:text-zinc-700" aria-hidden="true">
                        ·
                      </span>
                      <span
                        className={`inline-flex items-center gap-1.5 text-[11px] font-medium ${
                          clientStatuses[activeToolFilter]?.isRunning
                            ? "text-emerald-700 dark:text-emerald-400"
                            : "text-zinc-500 dark:text-zinc-400"
                        }`}
                      >
                        {clientStatuses[activeToolFilter]?.isRunning && (
                          <span
                            className="h-1.5 w-1.5 rounded-full bg-emerald-500 shadow-[0_0_0_2px_rgba(16,185,129,0.2)]"
                            aria-hidden="true"
                          />
                        )}
                        {clientStatuses[activeToolFilter]?.isRunning
                          ? "桌面端运行中"
                          : "桌面端未运行"}
                      </span>
                      {scopeAccounts.find((a) => a.isActiveOnClient) && (
                        <span className="hidden text-[11px] text-zinc-500 sm:inline dark:text-zinc-400">
                          · 当前挂载：{scopeAccounts.find((a) => a.isActiveOnClient)!.label}
                        </span>
                      )}
                    </div>

                    <div className="flex shrink-0 items-center gap-1">
                      <button
                        type="button"
                        onClick={() => void handleLaunchToolClient(activeToolFilter)}
                        disabled={launchingToolId === activeToolFilter}
                        title={
                          clientStatuses[activeToolFilter]?.isRunning
                            ? `唤起 ${activeToolMeta.name} 客户端`
                            : `快捷启动 ${activeToolMeta.name} 客户端`
                        }
                        aria-label={
                          clientStatuses[activeToolFilter]?.isRunning
                            ? `唤起 ${activeToolMeta.name} 客户端`
                            : `快捷启动 ${activeToolMeta.name} 客户端`
                        }
                        className="btn btn-xs btn-ghost shrink-0 gap-1 whitespace-nowrap"
                      >
                        {launchingToolId === activeToolFilter ? (
                          <Loader2 className="h-3 w-3 shrink-0 animate-spin" aria-hidden="true" />
                        ) : (
                          <Play className="h-3 w-3 shrink-0" aria-hidden="true" />
                        )}
                        {clientStatuses[activeToolFilter]?.isRunning ? "打开客户端" : "快捷启动"}
                      </button>
                      <button
                        type="button"
                        onClick={() => openImportModal(undefined, activeToolFilter)}
                        className="btn btn-xs btn-ghost shrink-0 gap-1 whitespace-nowrap"
                      >
                        <Plus className="h-3 w-3 shrink-0" aria-hidden="true" />
                        添加账号
                      </button>
                    </div>
                  </section>
                )}

                {isInitializing && scopeAccounts.length === 0 ? (
                <div
                  role="status"
                  aria-busy="true"
                  aria-label="正在加载本地账号库"
                  className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(300px,100%),1fr))]"
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
              ) : viewMode === "ledger" ? (
                <AccountLedger
                  accounts={filteredAccounts}
                  sort={ledgerSort}
                  onSortChange={setLedgerSort}
                  selectedIds={selectedAccountIds}
                  onSelectedIdsChange={setSelectedAccountIds}
                  hiddenSelectedCount={
                    [...selectedAccountIds].filter(
                      (id) => !filteredAccounts.some((a) => a.id === id)
                    ).length
                  }
                  onOpenDetail={(acc) => {
                    detailReturnFocusRef.current =
                      document.activeElement instanceof HTMLElement
                        ? document.activeElement
                        : null;
                    setDetailAccountId(acc.id);
                  }}
                />
              ) : nav === "all" && viewMode === "grouped" ? (
                <div className="space-y-6">
                  {groupedModules.map((group) => (
                    <section key={group.id}>
                      <div className="mb-2.5 flex flex-wrap items-center justify-between gap-2 px-0.5">
                        <div className="flex flex-wrap items-center gap-2">
                          <span
                            aria-hidden="true"
                            className={`h-3.5 w-[3px] shrink-0 rounded-full ${
                              group.isRunning
                                ? "bg-emerald-500"
                                : "bg-zinc-300 dark:bg-zinc-700"
                            }`}
                          />
                          <ToolBrandIcon toolId={group.id} size="sm" decorative />
                          <h2 className="text-[13px] font-semibold tracking-tight text-zinc-900 dark:text-zinc-100">
                            {group.name}
                          </h2>
                          <span className="text-[11px] font-medium tabular-nums text-zinc-500 dark:text-zinc-400">
                            {group.accounts.length} 个账号
                          </span>
                          <span className="text-zinc-300 dark:text-zinc-700" aria-hidden="true">
                            ·
                          </span>
                          <span
                            className={`inline-flex items-center gap-1.5 text-[11px] font-medium ${
                              group.isRunning
                                ? "text-emerald-700 dark:text-emerald-400"
                                : "text-zinc-500 dark:text-zinc-400"
                            }`}
                          >
                            {group.isRunning && (
                              <span
                                className="h-1.5 w-1.5 rounded-full bg-emerald-500 shadow-[0_0_0_2px_rgba(16,185,129,0.2)]"
                                aria-hidden="true"
                              />
                            )}
                            {group.isRunning ? "桌面端运行中" : "桌面端未运行"}
                          </span>
                          {group.activeAccount && (
                            <span className="hidden text-[11px] text-zinc-500 sm:inline dark:text-zinc-400">
                              · 当前挂载：{group.activeAccount.label}
                            </span>
                          )}
                        </div>

                        <div className="flex shrink-0 items-center gap-1">
                          <button
                            type="button"
                            onClick={() => void handleLaunchToolClient(group.id)}
                            disabled={launchingToolId === group.id}
                            title={
                              group.isRunning
                                ? `唤起 ${group.name} 客户端`
                                : `快捷启动 ${group.name} 客户端`
                            }
                            aria-label={
                              group.isRunning
                                ? `唤起 ${group.name} 客户端`
                                : `快捷启动 ${group.name} 客户端`
                            }
                            className="btn btn-xs btn-ghost shrink-0 gap-1 whitespace-nowrap"
                          >
                            {launchingToolId === group.id ? (
                              <Loader2
                                className="h-3 w-3 shrink-0 animate-spin"
                                aria-hidden="true"
                              />
                            ) : (
                              <Play className="h-3 w-3 shrink-0" aria-hidden="true" />
                            )}
                            {group.isRunning ? "打开客户端" : "快捷启动"}
                          </button>

                          <button
                            type="button"
                            onClick={() => openImportModal(undefined, group.id)}
                            className="btn btn-xs btn-ghost shrink-0 gap-1 whitespace-nowrap"
                          >
                            <Plus className="h-3 w-3 shrink-0" aria-hidden="true" />
                            添加账号
                          </button>
                        </div>
                      </div>

                      <div className="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(min(300px,100%),1fr))]">
                        {group.accounts.map((acc) => (
                          <AccountCard key={acc.id} account={acc} onOpenBindModal={openBindModal} />
                        ))}

                        {group.accounts.length === 1 && !isFiltered && (
                          <SecondAccountGhostCard
                            toolName={group.name}
                            scopeHint="新增副号后可在本模块内一键无缝切换桌面端登录态"
                            onClick={() => openImportModal(undefined, group.id)}
                          />
                        )}
                      </div>
                    </section>
                  ))}
                </div>
              ) : (
                <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(300px,100%),1fr))]">
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
                        onClick={() => openImportModal(undefined, activeToolFilter)}
                      />
                    )}
                </div>
              )}
              </div>
            </>
          )}

          {(nav === "agents" || nav === "pools") && (
            <AgentBindingsView
              highlightAgentId={highlightAgentId}
              onOpenPluginSettings={() => {
                setSettingsInitialTab("plugins");
                setNav("settings");
              }}
            />
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
              initialTab={settingsInitialTab}
              onInitialTabConsumed={() => setSettingsInitialTab(undefined)}
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
        onOpenPluginSettings={() => {
          setImportOpen(false);
          setBindTargetAccount(null);
          setImportDefaultMethod(undefined);
          setImportPresetTool(undefined);
          setSettingsInitialTab("plugins");
          setNav("settings");
        }}
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
      <ConfirmDialog
        open={confirmQuitOpen}
        title="确认退出 Qwitch？"
        description={
          gateway.running && batchProgress?.running
            ? "当前本地路由网关仍在运行，且有批量任务正在执行；退出将同时停止网关并中断未完成任务。"
            : gateway.running
              ? "当前本地路由网关仍在运行，退出后已接入的 Agent 将无法通过本地网关调度请求。"
              : "当前有批量额度刷新/领取任务正在执行，退出将中断未完成任务。"
        }
        confirmLabel="确认退出"
        danger
        onCancel={() => setConfirmQuitOpen(false)}
        onConfirm={async () => {
          setConfirmQuitOpen(false);
          await quitApp();
        }}
      />

      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        items={paletteItems}
      />

      <AccountDrawer
        account={detailAccount}
        returnFocusTo={detailReturnFocusRef.current}
        onClose={() => setDetailAccountId(null)}
      />
    </div>
  );
}
