import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  CheckCircle2,
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
import { PoolsView } from "./dashboard/PoolsView";
import { AgentBindingsView } from "./dashboard/AgentBindingsView";
import { RoutingTraceView } from "./dashboard/RoutingTraceView";
import { CreatePoolModal } from "../components/pool/CreatePoolModal";
import { ConfigDiffModal } from "../components/pool/ConfigDiffModal";
import { PoolSimulationModal } from "../components/pool/PoolSimulationModal";
import { usePoolPrototype } from "../context/usePoolPrototypeStore";
import { buildAgentConfigSnippets, computeAgentExposedModels } from "../data/mockPoolData";
import type { AccountPool, AgentBinding, TargetAgentId } from "../types/pool";

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
    pools,
    agentBindings,
    traces,
    setSelectedPoolId,
    toggleGateway,
    detectAgentEnvironment,
    mountPoolToAgents,
  } = usePoolPrototype();

  const mainScrollRef = useRef<HTMLElement | null>(null);
  const [nav, setNav] = useState<NavSection>("all");
  const prevNavRef = useRef<NavSection>(nav);
  const [createPoolOpen, setCreatePoolOpen] = useState(false);
  const [simModalOpen, setSimModalOpen] = useState(false);
  const [simDefaultPoolId, setSimDefaultPoolId] = useState<string | undefined>(undefined);
  const [simDefaultModelId, setSimDefaultModelId] = useState<string | undefined>(undefined);
  const [highlightAgentId, setHighlightAgentId] = useState<TargetAgentId | null>(null);
  const [pendingMountDiffs, setPendingMountDiffs] = useState<
    { pool: AccountPool; agentId: TargetAgentId }[]
  >([]);
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

  const activeCreationDiff = useMemo<{
    pool: AccountPool;
    binding: AgentBinding;
    nextPoolIds: string[];
    mountedPoolNames: string[];
    defaultModelDisplay: string | null;
    beforeText: string;
    afterText: string;
  } | null>(() => {
    const head = pendingMountDiffs[0];
    if (!head) return null;
    const binding = agentBindings.find((b) => b.agentId === head.agentId);
    if (!binding) return null;
    const nextPoolIds = binding.mountedPoolIds.includes(head.pool.id)
      ? binding.mountedPoolIds
      : [...binding.mountedPoolIds, head.pool.id];
    const allPoolsWithNew = pools.some((p) => p.id === head.pool.id)
      ? pools
      : [head.pool, ...pools];
    const snippets = buildAgentConfigSnippets(
      binding.agentId,
      nextPoolIds,
      binding.defaultModelKey,
      allPoolsWithNew,
      gateway,
      binding.options,
      false
    );
    const exposed = computeAgentExposedModels(
      {
        agentId: binding.agentId,
        mountedPoolIds: nextPoolIds,
        defaultModelKey: binding.defaultModelKey,
      },
      allPoolsWithNew
    );
    const defaultItem = exposed.find((e) => e.isDefault) ?? exposed[0] ?? null;
    return {
      pool: head.pool,
      binding,
      nextPoolIds,
      mountedPoolNames: nextPoolIds
        .map((id) => allPoolsWithNew.find((p) => p.id === id)?.name)
        .filter((n): n is string => Boolean(n)),
      defaultModelDisplay: defaultItem?.displayName ?? null,
      beforeText: snippets.beforeSnippet,
      afterText: snippets.afterSnippet,
    };
  }, [pendingMountDiffs, agentBindings, pools, gateway]);

  const headerTitle = useMemo(() => {
    if (nav === "pools") return "号池编排与调度策略";
    if (nav === "agents") return "Agent 指向与配置挂载";
    if (nav === "routing") return "网关路由观测与决策链路";
    if (nav === "logs") return "操作审计日志";
    if (nav === "settings") return "系统设置与客户端环境";
    if (activeToolMeta) {
      return `${activeToolMeta.name} 账号管理`;
    }
    return "账号与额度总览";
  }, [nav, activeToolMeta]);

  const headerSubtitle = useMemo(() => {
    if (nav === "pools") {
      const activePools = pools.filter((p) => p.enabled).length;
      return `已建 ${pools.length} 个号池（${activePools} 个启用）· 优先在号池内完成跨工具混编，再定向指向 Agent`;
    }
    if (nav === "agents") {
      return `已挂载 ${boundAgentCount}/${agentBindings.length} 个目标 Agent${
        driftedAgentCount > 0 ? ` · ${driftedAgentCount} 个存在配置漂移待修复` : " · Stash 原值保护就绪"
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
    pools,
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

  const isFiltered = Boolean(searchQuery.trim()) || statusFilter !== "all";
  const showFilterToolbar =
    scopeAccounts.length > 0 &&
    (nav === "all" || scopeAccounts.length > 1 || isFiltered);
  const currentLogTotal =
    logScope === "archived" ? logStats.archivedCount : logStats.activeCount;

  return (
    <div className="flex h-full overflow-hidden bg-zinc-50 text-zinc-900 dark:bg-zinc-950 dark:text-zinc-100">
      <aside className="flex h-full w-52 shrink-0 flex-col justify-between border-r border-zinc-200 bg-white/85 p-3 dark:border-zinc-800 dark:bg-zinc-900/65">
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
                    setNav("pools");
                  }}
                  aria-current={nav === "pools" ? "page" : undefined}
                  className={`nav-item ${nav === "pools" ? "nav-item-active" : ""}`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <Layers className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">号池编排</span>
                  </span>
                  <span
                    className={`shrink-0 rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                      nav === "pools"
                        ? "bg-brand-600/15 text-brand-800 dark:bg-brand-400/20 dark:text-brand-200"
                        : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                    }`}
                  >
                    {pools.length}
                  </span>
                </button>

                <button
                  type="button"
                  onClick={() => {
                    setHighlightAgentId(null);
                    setNav("agents");
                  }}
                  aria-current={nav === "agents" ? "page" : undefined}
                  className={`nav-item ${nav === "agents" ? "nav-item-active" : ""}`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <Link2 className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">Agent 指向</span>
                  </span>
                  <span
                    className={`shrink-0 rounded-full px-1.5 py-0.5 text-[11px] tabular-nums ${
                      driftedAgentCount > 0
                        ? "bg-amber-500/15 text-amber-800 dark:text-amber-300"
                        : nav === "agents"
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
          {/* 本地网关紧凑状态胶囊 */}
          <div className="flex items-center justify-between gap-1.5 rounded-xl border border-zinc-200/80 bg-zinc-50/80 px-2.5 py-1.5 text-[11px] dark:border-zinc-800 dark:bg-zinc-900/70">
            <button
              type="button"
              onClick={() => setNav("agents")}
              className="flex min-w-0 items-center gap-1.5 text-left font-medium text-zinc-700 hover:text-zinc-950 dark:text-zinc-200 dark:hover:text-white"
            >
              <span
                className={`h-2 w-2 shrink-0 rounded-full ${
                  gateway.running ? "bg-emerald-500" : "bg-zinc-400"
                }`}
              />
              <span className="truncate font-mono tabular-nums">
                :{gateway.port}
              </span>
            </button>
            <button
              type="button"
              onClick={toggleGateway}
              className={`shrink-0 rounded-md px-1.5 py-0.5 text-[11px] font-medium transition ${
                gateway.running
                  ? "bg-emerald-500/12 text-emerald-700 hover:bg-emerald-500/20 dark:text-emerald-300"
                  : "bg-zinc-200/80 text-zinc-600 hover:bg-zinc-300/70 dark:bg-zinc-800 dark:text-zinc-300"
              }`}
            >
              {gateway.running ? "网关运行" : "已停止"}
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
      </aside>

      <div className="@container flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <header className="z-20 flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-zinc-200 bg-white/85 px-5 py-3 backdrop-blur-xl dark:border-zinc-800 dark:bg-zinc-900/70">
          <div className="flex min-w-0 items-center gap-2.5">
            {activeToolFilter && <ToolBrandIcon toolId={activeToolFilter} size="sm" decorative />}
            <div className="min-w-0">
              <h1 className="truncate text-lg font-semibold tracking-tight">{headerTitle}</h1>
              <p className="truncate text-xs text-zinc-500 dark:text-zinc-400">
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
                    className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap"
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
                        className="btn btn-sm btn-primary gap-1.5 whitespace-nowrap"
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
                        className="btn btn-sm btn-primary gap-1.5 whitespace-nowrap"
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
                  className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap"
                >
                  <Plus className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  导入账号
                </button>
              </>
            )}

            {nav === "pools" && (
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
                  模拟打流
                </button>
                <button
                  type="button"
                  onClick={() => setCreatePoolOpen(true)}
                  className="btn btn-sm btn-primary gap-1.5 whitespace-nowrap"
                >
                  <Plus className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  新建号池
                </button>
              </>
            )}

            {nav === "agents" && (
              <>
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
                发起模拟打流
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

        {isAccountSection && (
          <div className="shrink-0 space-y-2.5 border-b border-zinc-200/80 bg-zinc-50/95 px-5 py-3 dark:border-zinc-800 dark:bg-zinc-950/95">
            {/* 方案 A：7 家内置工具筛选器收纳至账号工作区顶部横向胶囊条 */}
            <nav
              aria-label="账号视图导航"
              className="mask-fade-r scrollbar-none flex items-center gap-1.5 overflow-x-auto pb-0.5 pr-4"
            >
              <button
                type="button"
                onClick={() => setNav("all")}
                aria-current={nav === "all" ? "page" : undefined}
                className={`chip shrink-0 whitespace-nowrap ${
                  nav === "all" ? "chip-active" : ""
                }`}
              >
                <Layers className="h-3 w-3 shrink-0" aria-hidden="true" />
                <span>全部账号</span>
                <span className="tabular-nums opacity-80">{accounts.length}</span>
              </button>

              {visibleTools.map((tool) => {
                const active = nav === tool.id;
                const count = accountCountByTool.get(tool.id) ?? 0;
                const isRunning = clientStatuses[tool.id]?.isRunning;
                return (
                  <button
                    key={tool.id}
                    type="button"
                    onClick={() => setNav(tool.id)}
                    aria-current={active ? "page" : undefined}
                    className={`chip shrink-0 whitespace-nowrap ${active ? "chip-active" : ""}`}
                  >
                    <ToolBrandIcon toolId={tool.id} size="xs" decorative />
                    <span>{tool.name}</span>
                    {isRunning && count > 0 && (
                      <span
                        title="本机客户端运行中"
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-500"
                      />
                    )}
                    <span className="tabular-nums opacity-80">{count}</span>
                  </button>
                );
              })}
            </nav>

            <BatchClaimBanner />

            <section
              aria-label="当前作用域核心指标"
              className="grid grid-cols-1 gap-2.5 @sm:grid-cols-2 @3xl:grid-cols-4"
            >
              <button
                type="button"
                onClick={() => setStatusFilter("all")}
                aria-pressed={statusFilter === "all"}
                className={`surface-card surface-interactive flex min-w-0 flex-col justify-between gap-2.5 px-3.5 py-3 text-left ${
                  statusFilter === "all" ? "border-brand-500/60 ring-1 ring-brand-500/20" : ""
                }`}
              >
                <div className="flex w-full items-center justify-between gap-1.5">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-brand-500/12 text-brand-700 dark:text-brand-400">
                      <Layers className="h-3.5 w-3.5" aria-hidden="true" />
                    </span>
                    <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                      {activeToolMeta ? `${activeToolMeta.name} 账号` : "纳管账号"}
                    </span>
                  </div>
                </div>

                <div className="flex w-full items-baseline justify-between gap-1.5">
                  <div className="flex items-baseline gap-1">
                    <span className="text-2xl leading-none font-semibold tracking-tight tabular-nums text-zinc-900 dark:text-zinc-100">
                      {scopedStats.total}
                    </span>
                    <span className="text-xs text-zinc-500 dark:text-zinc-400">个</span>
                  </div>
                  <span className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
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
                className={`surface-card surface-interactive flex min-w-0 flex-col justify-between gap-2.5 px-3.5 py-3 text-left ${
                  statusFilter === "active_client"
                    ? "border-emerald-500/60 ring-1 ring-emerald-500/20"
                    : ""
                }`}
              >
                <div className="flex w-full items-center justify-between gap-1.5">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-emerald-500/12 text-emerald-700 dark:text-emerald-400">
                      <MonitorCheck className="h-3.5 w-3.5" aria-hidden="true" />
                    </span>
                    <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                      本机挂载
                    </span>
                  </div>
                  <span
                    className={`badge shrink-0 whitespace-nowrap ${
                      scopedStats.total > 0 && scopedStats.runningClientsCount > 0
                        ? "border-emerald-500/25 bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                        : "badge-neutral"
                    }`}
                  >
                    {scopedStats.total === 0
                      ? "无账号"
                      : scopedStats.runningClientsCount > 0
                        ? activeToolFilter
                          ? "运行中"
                          : `${scopedStats.runningClientsCount} 运行`
                        : "未启动"}
                  </span>
                </div>

                <div className="flex w-full items-baseline justify-between gap-1.5">
                  <div className="flex items-baseline">
                    <span className="text-2xl leading-none font-semibold tracking-tight tabular-nums text-zinc-900 dark:text-zinc-100">
                      {scopedStats.activeClientsCount}
                    </span>
                    <span className="ml-1 text-xs text-zinc-500 dark:text-zinc-400">
                      /{scopedStats.scopeToolCapacity}
                    </span>
                  </div>
                  <span className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
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
                className={`surface-card surface-interactive flex min-w-0 flex-col justify-between gap-2.5 px-3.5 py-3 text-left ${
                  statusFilter === "claimable"
                    ? "border-sky-500/60 ring-1 ring-sky-500/20"
                    : ""
                }`}
              >
                <div className="flex w-full items-center justify-between gap-1.5">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-sky-500/12 text-sky-700 dark:text-sky-400">
                      <CheckCircle2 className="h-3.5 w-3.5" aria-hidden="true" />
                    </span>
                    <span className="truncate text-xs font-medium text-zinc-600 dark:text-zinc-400">
                      今日签到
                    </span>
                  </div>
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
                    {scopedStats.claimBadgeText}
                  </span>
                </div>

                <div className="flex w-full items-baseline justify-between gap-1.5">
                  <div className="flex items-baseline gap-1">
                    <span className="text-2xl leading-none font-semibold tracking-tight tabular-nums text-zinc-900 dark:text-zinc-100">
                      {scopedStats.claimedCount}
                    </span>
                    <span className="text-xs text-zinc-500 dark:text-zinc-400">已签</span>
                  </div>
                  <span className="truncate text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                    {scopedStats.eligibleClaimTotal > 0
                      ? `共 ${scopedStats.eligibleClaimTotal} 可签`
                      : scopedStats.unclaimedAuthMissingCount > 0
                        ? "授权后可签"
                        : scopedStats.unsupportedClaimCount > 0
                          ? "当前套餐无签到"
                          : activeToolFilter && activeToolSupportsClaim
                            ? scopeAccounts.length === 0
                              ? "导入后可签"
                              : "暂无可签账号"
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
                className={`surface-card surface-interactive flex min-w-0 flex-col justify-between gap-2.5 px-3.5 py-3 text-left ${
                  statusFilter === "attention"
                    ? "border-amber-500/60 ring-1 ring-amber-500/20"
                    : ""
                }`}
              >
                <div className="flex w-full items-center justify-between gap-1.5">
                  <div className="flex min-w-0 items-center gap-2">
                    <span
                      className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-lg ${
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

                <div className="flex w-full items-baseline justify-between gap-1.5">
                  <div className="flex items-baseline gap-1">
                    <span className="text-2xl leading-none font-semibold tracking-tight tabular-nums text-zinc-900 dark:text-zinc-100">
                      {scopedStats.healthyCount}
                    </span>
                    <span className="text-xs text-zinc-500 dark:text-zinc-400">正常</span>
                  </div>
                  <span className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                    {scopedStats.attentionCount > 0 ? "点击筛选" : "状态完好"}
                  </span>
                </div>
              </button>
            </section>

            {showFilterToolbar && (
              <div className="surface-card flex flex-wrap items-center justify-between gap-2.5 px-3.5 py-2.5">
                <div className="relative min-w-[190px] flex-1 max-w-xs">
                  <Search
                    className="pointer-events-none absolute top-1/2 left-3 h-3.5 w-3.5 -translate-y-1/2 text-zinc-400 dark:text-zinc-500"
                    aria-hidden="true"
                  />
                  <input
                    type="text"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    placeholder="搜索昵称、UID、手机号或套餐..."
                    aria-label="搜索账号"
                    className="w-full rounded-lg border border-zinc-200 bg-zinc-50/70 py-1.5 pr-7 pl-8 text-xs text-zinc-900 transition focus:border-brand-400 focus:bg-white focus:outline-none dark:border-zinc-700 dark:bg-zinc-800/60 dark:text-zinc-100 dark:focus:border-brand-500 dark:focus:bg-zinc-800"
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
                    <div className="inline-flex shrink-0 rounded-lg border border-zinc-200 bg-zinc-50 p-0.5 dark:border-zinc-700 dark:bg-zinc-800/70">
                      <button
                        type="button"
                        onClick={() => setViewMode("grouped")}
                        title="按平台模块化分类展示"
                        className={`inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium transition ${
                          viewMode === "grouped"
                            ? "bg-white text-zinc-900 shadow-xs dark:bg-zinc-700 dark:text-zinc-100"
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
                        className={`inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium transition ${
                          viewMode === "flat"
                            ? "bg-white text-zinc-900 shadow-xs dark:bg-zinc-700 dark:text-zinc-100"
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
        )}

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
          className={`min-h-0 flex-1 px-5 py-3.5 ${
            nav === "pools"
              ? "overflow-y-auto @3xl:overflow-hidden"
              : "overflow-y-auto"
          }`}
        >
          {/* 更新提示：放在账号区顶部，日常使用路径（账号池 / 工具页）即可见，
              无需进入设置页才能发现新版本 */}
          {isAccountSection && (
            <div className="mb-3">
              <UpdateBanner onOpenSettings={() => setNav("settings")} />
            </div>
          )}

          {isAccountSection && (
            <>

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
            </>
          )}

          {nav === "pools" && (
            <PoolsView
              onOpenCreatePool={() => setCreatePoolOpen(true)}
              onOpenSimulation={(poolId, modelId) => {
                setSimDefaultPoolId(poolId);
                setSimDefaultModelId(modelId);
                setSimModalOpen(true);
              }}
              onNavigateAgents={(agentId) => {
                setHighlightAgentId(agentId ?? null);
                setNav("agents");
              }}
            />
          )}

          {nav === "agents" && (
            <AgentBindingsView
              highlightAgentId={highlightAgentId}
              onNavigatePools={(poolId) => {
                if (poolId) setSelectedPoolId(poolId);
                setNav("pools");
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
            />
          )}
        </main>
      </div>

      <ColdSwitchModal />
      <CreatePoolModal
        open={createPoolOpen}
        onClose={() => setCreatePoolOpen(false)}
        onCreatedWithAgents={(pool, agentIds) => {
          setPendingMountDiffs(agentIds.map((agentId) => ({ pool, agentId })));
        }}
      />
      {activeCreationDiff && (
        <ConfigDiffModal
          open={Boolean(activeCreationDiff)}
          binding={activeCreationDiff.binding}
          actionKind="update_mounts"
          targetSummary={{
            mountedPoolNames: activeCreationDiff.mountedPoolNames,
            defaultModelDisplay: activeCreationDiff.defaultModelDisplay,
          }}
          beforeText={activeCreationDiff.beforeText}
          afterText={activeCreationDiff.afterText}
          onClose={() => setPendingMountDiffs((prev) => prev.slice(1))}
          onConfirm={() => {
            mountPoolToAgents(activeCreationDiff.pool.id, [
              activeCreationDiff.binding.agentId,
            ]);
          }}
        />
      )}
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
