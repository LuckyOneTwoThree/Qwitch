import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { INITIAL_ACCOUNTS, INITIAL_LOGS, MOCK_ADAPTERS } from "../data/mockStore";
import { getEffectiveClaimStatus, selectClaimCandidates, toCstDayKey } from "../domain/selectors";
import {
  buildSettingsSavePayload,
  normalizeAppSettings,
  sanitizeAppSettingsPatch,
} from "./settingsUtils";
import type {
  AccountSummary,
  ActionLogCleanupRequest,
  ActionLogItem,
  ActionLogStats,
  AdapterManifest,
  AppSettings,
  AuthMethod,
  BatchActionType,
  ClientHostStatus,
  ExportLogsResult,
  LogScope,
  ToastState,
  ToastTone,
  ToolId,
} from "../types/qwitch";

export interface BatchProgressState {
  action?: BatchActionType;
  running: boolean;
  total: number;
  completed: number;
  currentLabel?: string;
  phase: "checking" | "claiming" | "jitter_wait" | "done";
  jitterSeconds?: number;
  summaryText?: string;
}

export interface SwitchDialogState {
  account: AccountSummary | null;
  phase: "confirm" | "closing" | "writing" | "launching" | "done";
  isClientRunning?: boolean;
}

interface QwitchContextValue {
  isTauriRuntime: boolean;
  isInitializing: boolean;
  adapters: AdapterManifest[];
  accounts: AccountSummary[];
  logs: ActionLogItem[];
  logScope: "active" | "archived";
  setLogScope: (scope: "active" | "archived") => void;
  logStats: ActionLogStats;
  hasMoreLogs: boolean;
  loadMoreLogs: () => Promise<void>;
  batchProgress: BatchProgressState | null;
  switchDialog: SwitchDialogState;
  toast: ToastState | null;
  showToast: (message: string, tone?: ToastTone) => void;
  dismissToast: () => void;
  theme: "light" | "dark";
  toggleTheme: () => void;
  appSettings: AppSettings;
  jitterRangeMs: [number, number];
  setJitterRangeMs: (range: [number, number]) => void;
  logRetention: { archiveAfterDays: number; maxActiveCount: number };
  setLogRetention: (next: { archiveAfterDays: number; maxActiveCount: number }) => void;
  customClientPaths: Record<string, string>;
  clientStatuses: Record<string, ClientHostStatus>;
  saveAppSettings: (next: Partial<AppSettings>, silent?: boolean) => Promise<boolean>;
  refreshClientStatuses: (toolIds?: ToolId[]) => Promise<void>;
  refreshAccountQuota: (accountId: string) => Promise<void>;
  refreshAllQuotas: (toolId?: ToolId) => Promise<boolean>;
  claimDailyBatch: (targetIds?: string[], toolId?: ToolId, includeUnknown?: boolean) => Promise<void>;
  requestColdSwitch: (account: AccountSummary) => void;
  cancelColdSwitch: () => void;
  confirmColdSwitch: () => Promise<void>;
  importAccount: (
    toolId: ToolId,
    label: string,
    method: AuthMethod,
    rawInput?: string,
    targetAccountId?: string
  ) => Promise<void>;
  renameAccount: (accountId: string, label: string) => Promise<void>;
  toggleAccountDisabled: (accountId: string) => Promise<void>;
  recoverAccount: (accountId: string) => Promise<void>;
  deleteAccount: (accountId: string) => Promise<void>;
  archiveLogs: (
    request: ActionLogCleanupRequest,
    feedbackLabel?: string | ((count: number) => string)
  ) => Promise<number>;
  unarchiveLogs: (
    request: ActionLogCleanupRequest,
    feedbackLabel?: string | ((count: number) => string)
  ) => Promise<number>;
  deleteLogs: (
    request: ActionLogCleanupRequest,
    feedbackLabel?: string | ((count: number) => string)
  ) => Promise<number>;
  exportLogs: (scope?: LogScope, archiveAfterExport?: boolean) => Promise<void>;
  dismissBatchProgress: () => void;
  quitApp: () => Promise<void>;
  cstDayKey: string;
}

const QwitchContext = createContext<QwitchContextValue | null>(null);

const LOG_PAGE_SIZE = 200;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const detectTauriRuntime = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const getInitialTheme = (): "light" | "dark" => {
  if (typeof window === "undefined") return "light";
  try {
    const saved = window.localStorage.getItem("qwitch.theme");
    if (saved === "dark" || saved === "light") return saved;
    if (window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches) {
      return "dark";
    }
  } catch {
    // ignore storage access errors
  }
  return "light";
};

const syncThemeStorage = (nextTheme: "light" | "dark" | "system") => {
  try {
    window.localStorage.setItem("qwitch.theme", nextTheme);
  } catch {
    // ignore
  }
};

const resolveTheme = (t: "light" | "dark" | "system"): "light" | "dark" => {
  if (t === "system") {
    return typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }
  return t;
};

const detectTrayWindow = () =>
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("view") === "tray";

const isRedundantLog = (log: ActionLogItem) =>
  log.resultStatus === "skipped" ||
  (log.actionType === "query" && log.resultStatus === "success");

const matchesLogCleanup = (
  log: ActionLogItem,
  req: ActionLogCleanupRequest,
  defaultScope: LogScope
) => {
  const scope = req.scope ?? defaultScope;
  if (scope === "active" && log.archivedAt) return false;
  if (scope === "archived" && !log.archivedAt) return false;
  if (req.ids) {
    if (!req.ids.includes(log.id)) return false;
  }
  if (req.olderThanDays && req.olderThanDays > 0) {
    const cutoff = Date.now() - req.olderThanDays * 86_400_000;
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
};

export function QwitchProvider({ children }: { children: React.ReactNode }) {
  const isTauriRuntime = detectTauriRuntime();
  const isTrayWindow = detectTrayWindow();

  const [isInitializing, setIsInitializing] = useState<boolean>(isTauriRuntime);
  const [adapters, setAdapters] = useState<AdapterManifest[]>(
    isTauriRuntime ? [] : MOCK_ADAPTERS
  );
  const [accounts, setAccounts] = useState<AccountSummary[]>(
    isTauriRuntime ? [] : INITIAL_ACCOUNTS
  );
  const [logsState, setLogsState] = useState<ActionLogItem[]>(
    isTauriRuntime ? [] : INITIAL_LOGS
  );
  const [logScope, setLogScope] = useState<"active" | "archived">("active");
  const [backendLogStats, setBackendLogStats] = useState<ActionLogStats>({
    activeCount: 0,
    archivedCount: 0,
    failedActiveCount: 0,
    noiseActiveCount: 0,
  });
  const [batchProgress, setBatchProgress] = useState<BatchProgressState | null>(null);
  const [switchDialog, setSwitchDialog] = useState<SwitchDialogState>({
    account: null,
    phase: "confirm",
  });
  const [toast, setToast] = useState<ToastState | null>(null);
  const [appSettings, setAppSettings] = useState<AppSettings>(() => normalizeAppSettings());
  const [theme, setTheme] = useState<"light" | "dark">(getInitialTheme);
  const [jitterRangeMs, setJitterRangeMsState] = useState<[number, number]>([1500, 3500]);
  const [logRetention, setLogRetentionState] = useState<{
    archiveAfterDays: number;
    maxActiveCount: number;
  }>({ archiveAfterDays: 14, maxActiveCount: 200 });
  const [customClientPaths, setCustomClientPaths] = useState<Record<string, string>>({});
  const [clientStatuses, setClientStatuses] = useState<Record<string, ClientHostStatus>>({});
  const [cstDayKey, setCstDayKey] = useState<string>(() => toCstDayKey(Date.now()));
  const cstDayKeyRef = useRef(cstDayKey);
  cstDayKeyRef.current = cstDayKey;
  const jitterDirtyRef = useRef(false);
  const logRetentionDirtyRef = useRef(false);
  const settingsSaveChainRef = useRef<Promise<boolean>>(Promise.resolve(true));

  const setJitterRangeMs = useCallback((range: [number, number]) => {
    jitterDirtyRef.current = true;
    setJitterRangeMsState(range);
  }, []);

  const setLogRetention = useCallback(
    (next: { archiveAfterDays: number; maxActiveCount: number }) => {
      logRetentionDirtyRef.current = true;
      setLogRetentionState(next);
    },
    []
  );

  const toastTimerRef = useRef<number | null>(null);
  const toastSeqRef = useRef(0);
  const logLimitRef = useRef(LOG_PAGE_SIZE);
  const logScopeRef = useRef(logScope);
  logScopeRef.current = logScope;
  const didMountLogsRef = useRef(false);
  const syncDebounceTimerRef = useRef<number | null>(null);
  const appSettingsRef = useRef(appSettings);
  appSettingsRef.current = appSettings;
  const accountsRef = useRef(accounts);
  accountsRef.current = accounts;
  const logsRef = useRef(logsState);
  logsRef.current = logsState;
  const adaptersRef = useRef(adapters);
  adaptersRef.current = adapters;
  const batchRunningRef = useRef(Boolean(batchProgress?.running));
  batchRunningRef.current = Boolean(batchProgress?.running);
  const pendingAutoClaimRef = useRef(false);
  const startupAutoRunDoneRef = useRef(false);
  const batchDismissTimerRef = useRef<number | null>(null);
  const settingsLoadedRef = useRef(false);
  const logsDebounceTimerRef = useRef<number | null>(null);

  const applyIncomingSettings = useCallback(
    (raw?: Partial<AppSettings> | null, forceDrafts = false) => {
      const normalized = normalizeAppSettings(raw, appSettingsRef.current);
      setAppSettings(normalized);
      appSettingsRef.current = normalized;
      if (!normalized.autoClaimDaily) {
        pendingAutoClaimRef.current = false;
      }
      setTheme(resolveTheme(normalized.theme));
      syncThemeStorage(normalized.theme);
      setCustomClientPaths(normalized.customClientPaths);
      if (forceDrafts || !jitterDirtyRef.current) {
        setJitterRangeMsState([normalized.jitterMinMs, normalized.jitterMaxMs]);
      }
      if (forceDrafts || !logRetentionDirtyRef.current) {
        setLogRetentionState({
          archiveAfterDays: normalized.logArchiveAfterDays ?? 14,
          maxActiveCount: normalized.logMaxActiveCount ?? 200,
        });
      }
    },
    []
  );

  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
  }, [theme]);

  // system 主题运行时跟随系统深浅色切换（兼容旧版 WKWebView addListener）
  useEffect(() => {
    if (appSettings.theme !== "system" || typeof window === "undefined" || !window.matchMedia) {
      return;
    }
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setTheme(mq.matches ? "dark" : "light");
    if (typeof mq.addEventListener === "function") {
      mq.addEventListener("change", onChange);
      return () => mq.removeEventListener("change", onChange);
    }
    if (typeof mq.addListener === "function") {
      mq.addListener(onChange);
      return () => mq.removeListener(onChange);
    }
  }, [appSettings.theme]);

  useEffect(() => {
    return () => {
      if (toastTimerRef.current !== null) {
        window.clearTimeout(toastTimerRef.current);
      }
      if (syncDebounceTimerRef.current !== null) {
        window.clearTimeout(syncDebounceTimerRef.current);
      }
      if (batchDismissTimerRef.current !== null) {
        window.clearTimeout(batchDismissTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (batchDismissTimerRef.current !== null) {
      window.clearTimeout(batchDismissTimerRef.current);
      batchDismissTimerRef.current = null;
    }
    if (!batchProgress || batchProgress.running) return;
    const summary = batchProgress.summaryText ?? "";
    const hasFailure = /异常\s+[1-9]/.test(summary) || summary.includes("失败");
    const delayMs = hasFailure ? 5000 : 3200;
    batchDismissTimerRef.current = window.setTimeout(() => {
      setBatchProgress(null);
      batchDismissTimerRef.current = null;
    }, delayMs);
    return () => {
      if (batchDismissTimerRef.current !== null) {
        window.clearTimeout(batchDismissTimerRef.current);
        batchDismissTimerRef.current = null;
      }
    };
  }, [batchProgress]);

  const dismissToast = useCallback(() => {
    if (toastTimerRef.current !== null) {
      window.clearTimeout(toastTimerRef.current);
      toastTimerRef.current = null;
    }
    setToast(null);
  }, []);

  const showToast = useCallback((message: string, tone: ToastTone = "info") => {
    if (toastTimerRef.current !== null) {
      window.clearTimeout(toastTimerRef.current);
    }
    const id = ++toastSeqRef.current;
    setToast({ id, message, tone });
    const durationMs = tone === "error" ? 6500 : tone === "warning" ? 4500 : 3400;
    toastTimerRef.current = window.setTimeout(() => {
      setToast((prev) => (prev && prev.id === id ? null : prev));
      toastTimerRef.current = null;
    }, durationMs);
  }, []);

  const saveAppSettings = useCallback(
    (next: Partial<AppSettings>, silent = false): Promise<boolean> => {
      const run = settingsSaveChainRef.current
        .catch(() => false)
        .then(async () => {
          const previous = appSettingsRef.current;
          const sanitizedNext = sanitizeAppSettingsPatch(next);
          const touchesJitter =
            next.jitterMinMs !== undefined || next.jitterMaxMs !== undefined;
          const touchesRetention =
            next.logArchiveAfterDays !== undefined ||
            next.logMaxActiveCount !== undefined;
          const payload = buildSettingsSavePayload(sanitizedNext, previous);

          if (touchesJitter && payload.jitterMinMs > payload.jitterMaxMs) {
            showToast("保存失败：最小间隔不能大于最大间隔", "error");
            return false;
          }

          if (sanitizedNext.autoClaimDaily === false) {
            pendingAutoClaimRef.current = false;
          }

          setAppSettings(payload);
          appSettingsRef.current = payload;
          setTheme(resolveTheme(payload.theme));
          syncThemeStorage(payload.theme);
          setCustomClientPaths(payload.customClientPaths);
          if (touchesJitter) {
            setJitterRangeMsState([payload.jitterMinMs, payload.jitterMaxMs]);
          }
          if (touchesRetention) {
            setLogRetentionState({
              archiveAfterDays: payload.logArchiveAfterDays ?? 14,
              maxActiveCount: payload.logMaxActiveCount ?? 200,
            });
          }

          if (isTauriRuntime) {
            try {
              const saved = await invoke<AppSettings>("save_app_settings", {
                settings: payload,
              });
              if (touchesJitter) jitterDirtyRef.current = false;
              if (touchesRetention) logRetentionDirtyRef.current = false;
              applyIncomingSettings(saved);
              if (!silent) showToast("设置已保存", "success");
              return true;
            } catch (err) {
              // 回滚只恢复已保存字段，保留未保存的抖动/日志保留草稿
              setAppSettings(previous);
              appSettingsRef.current = previous;
              setTheme(resolveTheme(previous.theme));
              syncThemeStorage(previous.theme);
              setCustomClientPaths(previous.customClientPaths);
              showToast(`保存设置失败：${String(err)}`, "error");
              return false;
            }
          } else {
            if (touchesJitter) jitterDirtyRef.current = false;
            if (touchesRetention) logRetentionDirtyRef.current = false;
            applyIncomingSettings(payload);
            if (!silent) showToast("设置已保存（Mock 模式）", "success");
            return true;
          }
        });
      settingsSaveChainRef.current = run;
      return run;
    },
    [applyIncomingSettings, isTauriRuntime, showToast]
  );

  const toggleTheme = useCallback(() => {
    const nextTheme: "light" | "dark" = theme === "dark" ? "light" : "dark";
    void saveAppSettings({ theme: nextTheme }, true);
  }, [saveAppSettings, theme]);

  const switchableAdapterIdsKey = useMemo(
    () =>
      adapters
        .filter((a) => a.capabilities.supportsClientSwitch)
        .map((a) => a.id)
        .sort()
        .join(","),
    [adapters]
  );

  const refreshClientStatuses = useCallback(
    async (toolIds?: ToolId[]) => {
      const targets =
        toolIds ??
        (switchableAdapterIdsKey ? switchableAdapterIdsKey.split(",") : []);

      if (targets.length === 0) return;

      if (isTauriRuntime) {
        const entries = await Promise.all(
          targets.map(async (tid) => {
            try {
              const status = await invoke<ClientHostStatus>("detect_client_status", {
                toolId: tid,
              });
              return [tid, status] as const;
            } catch {
              return [tid, { toolId: tid, isRunning: false } as ClientHostStatus] as const;
            }
          })
        );
        setClientStatuses((prev) => ({
          ...prev,
          ...Object.fromEntries(entries),
        }));
        return;
      }

      const mockEntries = targets.map((tid) => [
        tid,
        {
          toolId: tid,
          executablePath:
            tid === "trae_cn"
              ? "/Applications/Trae CN.app"
              : tid === "workbuddy"
                ? "/Applications/WorkBuddy.app"
                : "/Applications/Qoder.app",
          configDir: `~/Library/Application Support/${tid}`,
          isRunning: tid === "trae_cn",
        } satisfies ClientHostStatus,
      ]);
      setClientStatuses((prev) => ({
        ...prev,
        ...Object.fromEntries(mockEntries),
      }));
    },
    [isTauriRuntime, switchableAdapterIdsKey]
  );

  const refreshLogs = useCallback(
    async (targetScope?: "active" | "archived") => {
      if (!isTauriRuntime) return;
      const scope = targetScope ?? logScopeRef.current;
      try {
        const [logList, stats] = await Promise.all([
          invoke<ActionLogItem[]>("get_action_logs", {
            scope,
            limit: logLimitRef.current,
            offset: 0,
          }),
          invoke<ActionLogStats>("get_action_log_stats"),
        ]);
        setLogsState(logList);
        setBackendLogStats(stats);
      } catch (err) {
        showToast(`刷新日志失败：${String(err)}`, "error");
      }
    },
    [isTauriRuntime, showToast]
  );

  const loadMoreLogs = useCallback(async () => {
    if (!isTauriRuntime) return;
    const scope = logScopeRef.current;
    try {
      const more = await invoke<ActionLogItem[]>("get_action_logs", {
        scope,
        limit: LOG_PAGE_SIZE,
        offset: logsState.length,
      });
      if (more.length === 0) return;
      logLimitRef.current = logsState.length + more.length;
      setLogsState((prev) => {
        const seen = new Set(prev.map((item) => item.id));
        const merged = [...prev];
        for (const item of more) {
          if (!seen.has(item.id)) {
            merged.push(item);
          }
        }
        return merged;
      });
    } catch (err) {
      showToast(`加载更多日志失败：${String(err)}`, "error");
    }
  }, [isTauriRuntime, logsState.length, showToast]);

  const syncFromBackend = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const [adList, accList, logList, stats, settings] = await Promise.all([
        invoke<AdapterManifest[]>("list_adapters"),
        invoke<AccountSummary[]>("list_accounts", { toolId: null }),
        invoke<ActionLogItem[]>("get_action_logs", {
          scope: logScopeRef.current,
          limit: logLimitRef.current,
          offset: 0,
        }),
        invoke<ActionLogStats>("get_action_log_stats"),
        invoke<AppSettings>("get_app_settings"),
      ]);
      setAdapters(adList);
      setAccounts(accList);
      setLogsState(logList);
      setBackendLogStats(stats);
      // 设置只在首次加载时应用；后续后台同步不得覆盖用户未保存的草稿
      // （远端真实变更经 settings-updated 事件单独下发）
      if (!settingsLoadedRef.current) {
        settingsLoadedRef.current = true;
        applyIncomingSettings(settings, true);
      }
    } catch (err) {
      showToast(`同步后端数据失败：${String(err)}`, "error");
    } finally {
      setIsInitializing(false);
    }
  }, [applyIncomingSettings, isTauriRuntime, showToast]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    if (!didMountLogsRef.current) {
      didMountLogsRef.current = true;
      return;
    }
    logLimitRef.current = LOG_PAGE_SIZE;
    void refreshLogs(logScope);
  }, [isTauriRuntime, logScope, refreshLogs]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    void syncFromBackend();

    let cancelled = false;
    const unlisteners: Array<() => void> = [];

    const register = <T,>(
      event: string,
      handler: (payload: T) => void
    ) => {
      void listen<T>(event, (ev) => handler(ev.payload)).then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisteners.push(fn);
        }
      });
    };

    const scheduleSync = () => {
      if (syncDebounceTimerRef.current !== null) {
        window.clearTimeout(syncDebounceTimerRef.current);
      }
      syncDebounceTimerRef.current = window.setTimeout(() => {
        syncDebounceTimerRef.current = null;
        void syncFromBackend();
      }, 120);
    };

    register<BatchProgressState>("qwitch://batch-progress", (payload) => {
      setBatchProgress(payload);
      if (!payload.running) {
        scheduleSync();
        if (
          payload.action === "refresh" &&
          !isTrayWindow &&
          pendingAutoClaimRef.current
        ) {
          pendingAutoClaimRef.current = false;
          if (appSettingsRef.current.autoClaimDaily) {
            window.setTimeout(() => {
              void triggerAutoClaimAvailableRef.current();
            }, 250);
          }
        }
      }
    });

    register<string>("qwitch://account-updated", () => {
      scheduleSync();
    });

    register<string>("qwitch://oauth-error", (msg) => {
      showToast(msg, "error");
    });

    register<{ accountId: string; phase: SwitchDialogState["phase"] }>(
      "qwitch://switch-progress",
      (payload) => {
        setSwitchDialog((prev) =>
          prev.account && prev.account.id === payload.accountId
            ? { ...prev, phase: payload.phase }
            : prev
        );
      }
    );

    register<AppSettings>("qwitch://settings-updated", (payload) => {
      applyIncomingSettings(payload);
    });

    register<unknown>("qwitch://logs-updated", () => {
      if (logsDebounceTimerRef.current !== null) {
        window.clearTimeout(logsDebounceTimerRef.current);
      }
      logsDebounceTimerRef.current = window.setTimeout(() => {
        logsDebounceTimerRef.current = null;
        void refreshLogs();
      }, 150);
    });

    return () => {
      cancelled = true;
      if (syncDebounceTimerRef.current !== null) {
        window.clearTimeout(syncDebounceTimerRef.current);
        syncDebounceTimerRef.current = null;
      }
      if (logsDebounceTimerRef.current !== null) {
        window.clearTimeout(logsDebounceTimerRef.current);
        logsDebounceTimerRef.current = null;
      }
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, [applyIncomingSettings, isTrayWindow, isTauriRuntime, refreshLogs, showToast, syncFromBackend]);

  const appendLog = useCallback(
    (entry: Omit<ActionLogItem, "id" | "createdAt">) => {
      setLogsState((prev) => [
        {
          ...entry,
          id: Date.now() + Math.floor(Math.random() * 1000),
          createdAt: new Date().toISOString(),
        },
        ...prev,
      ]);
    },
    []
  );

  const refreshAccountQuota = useCallback(
    async (accountId: string) => {
      const target = accounts.find((a) => a.id === accountId);
      if (!target) return;

      if (isTauriRuntime) {
        try {
          await invoke("refresh_account_quota", { accountId });
          await syncFromBackend();
          showToast(`已刷新 ${target.label} 额度`, "success");
        } catch (err) {
          await syncFromBackend();
          showToast(`刷新失败：${String(err)}`, "error");
        }
        return;
      }

      await sleep(450);
      const now = new Date().toISOString();
      setAccounts((prev) =>
        prev.map((acc) =>
          acc.id === accountId && acc.latestQuota
            ? { ...acc, updatedAt: now, latestQuota: { ...acc.latestQuota, fetchedAt: now } }
            : acc
        )
      );
      appendLog({
        accountId,
        toolId: target.toolId,
        actionType: "query",
        resultStatus: "success",
        messageSanitized: `已刷新 ${target.label} (${target.uidMasked}) 的最新额度水位`,
      });
      showToast(`已刷新 ${target.label} 额度`, "success");
    },
    [accounts, appendLog, isTauriRuntime, showToast, syncFromBackend]
  );

  const refreshAllQuotas = useCallback(
    async (toolId?: ToolId): Promise<boolean> => {
      if (batchProgress?.running) {
        showToast("当前已有批量任务正在执行，请稍候", "warning");
        return false;
      }

      const candidates = accounts.filter((a) => {
        if (toolId && a.toolId !== toolId) return false;
        if (a.status !== "active" || a.hasApiToken === false) return false;
        const adapter = adapters.find((ad) => ad.id === a.toolId);
        return adapter?.capabilities.supportsQuotaQuery === true;
      });

      if (candidates.length === 0) {
        showToast("当前范围下没有已完成在线授权的正常账号", "info");
        return false;
      }

      if (isTauriRuntime) {
        try {
          await invoke("refresh_quota_batch", {
            accountIds: candidates.map((c) => c.id),
          });
          return true;
        } catch (err) {
          showToast(`启动批量查额失败：${String(err)}`, "error");
          return false;
        }
      }

      setBatchProgress({
        action: "refresh",
        running: true,
        total: candidates.length,
        completed: 0,
        currentLabel: candidates[0].label,
        phase: "checking",
      });

      for (let i = 0; i < candidates.length; i++) {
        const acc = candidates[i];
        setBatchProgress({
          action: "refresh",
          running: true,
          total: candidates.length,
          completed: i,
          currentLabel: acc.label,
          phase: "checking",
        });
        await sleep(320);
        const now = new Date().toISOString();
        setAccounts((prev) =>
          prev.map((item) =>
            item.id === acc.id && item.latestQuota
              ? { ...item, updatedAt: now, latestQuota: { ...item.latestQuota, fetchedAt: now } }
              : item
          )
        );
        appendLog({
          accountId: acc.id,
          toolId: acc.toolId,
          actionType: "query",
          resultStatus: "success",
          messageSanitized: `已刷新 ${acc.label} (${acc.uidMasked}) 的最新额度`,
        });

        if (i < candidates.length - 1) {
          setBatchProgress({
            action: "refresh",
            running: true,
            total: candidates.length,
            completed: i + 1,
            phase: "jitter_wait",
            jitterSeconds: 0.4,
          });
          await sleep(350);
        }
      }

      const summaryText = `额度刷新完成：成功 ${candidates.length} 个，跳过 0 个，异常 0 个`;
      setBatchProgress({
        action: "refresh",
        running: false,
        total: candidates.length,
        completed: candidates.length,
        phase: "done",
        summaryText,
      });
      if (!isTrayWindow && pendingAutoClaimRef.current) {
        pendingAutoClaimRef.current = false;
        if (appSettingsRef.current.autoClaimDaily) {
          window.setTimeout(() => {
            void triggerAutoClaimAvailableRef.current();
          }, 250);
        }
      }
      return true;
    },
    [accounts, adapters, appendLog, batchProgress?.running, isTauriRuntime, isTrayWindow, showToast]
  );

  const claimDailyBatch = useCallback(
    async (targetIds?: string[], toolId?: ToolId, includeUnknown?: boolean) => {
      if (batchProgress?.running) {
        showToast("当前已有批量任务正在执行，请稍候", "warning");
        return;
      }

      const candidates = selectClaimCandidates(accounts, adapters, {
        targetIds,
        toolId,
        includeUnknown,
      });

      if (candidates.length === 0) {
        const eligibleAll = accounts.filter((acc) => {
          if (targetIds && !targetIds.includes(acc.id)) return false;
          if (!targetIds && toolId && acc.toolId !== toolId) return false;
          const adapter = adapters.find((ad) => ad.id === acc.toolId);
          return (
            acc.status === "active" &&
            acc.hasApiToken !== false &&
            Boolean(adapter?.capabilities.supportsDailyClaim)
          );
        });
        const alreadyClaimedCount = eligibleAll.filter(
          (acc) => getEffectiveClaimStatus(acc) === "claimed"
        ).length;
        const unknownCount = eligibleAll.filter(
          (acc) => getEffectiveClaimStatus(acc) === "unknown"
        ).length;
        const missingTokenCount = accounts.filter((acc) => {
          if (targetIds && !targetIds.includes(acc.id)) return false;
          if (!targetIds && toolId && acc.toolId !== toolId) return false;
          const adapter = adapters.find((ad) => ad.id === acc.toolId);
          return (
            acc.status === "active" &&
            acc.hasApiToken === false &&
            Boolean(adapter?.capabilities.supportsDailyClaim)
          );
        }).length;
        if (alreadyClaimedCount > 0 && missingTokenCount === 0 && unknownCount === 0) {
          showToast(`今日可签到账号已全部领取完成 (${alreadyClaimedCount} 个)`, "success");
          return;
        }
        showToast(
          missingTokenCount > 0
            ? "账号尚未完成在线授权，请先点击卡片「在线授权」后再领取"
            : unknownCount > 0
              ? `存在 ${unknownCount} 个待核实账号，请点击「核实并领取」先查状态再领取`
              : "当前范围下没有可参与领取的正常账号",
          missingTokenCount > 0 ? "warning" : "info"
        );
        return;
      }

      if (isTauriRuntime) {
        try {
          await invoke("claim_daily_batch", {
            accountIds: candidates.map((c) => c.id),
          });
        } catch (err) {
          showToast(`启动领取队列失败：${String(err)}`, "error");
        }
        return;
      }

      let claimedCount = 0;
      let skippedCount = 0;

      setBatchProgress({
        action: "claim",
        running: true,
        total: candidates.length,
        completed: 0,
        currentLabel: candidates[0].label,
        phase: "checking",
      });

      for (let i = 0; i < candidates.length; i++) {
        const acc = candidates[i];
        setBatchProgress({
          action: "claim",
          running: true,
          total: candidates.length,
          completed: i,
          currentLabel: acc.label,
          phase: "checking",
        });
        await sleep(450);

        if (getEffectiveClaimStatus(acc) === "claimed") {
          skippedCount++;
          appendLog({
            accountId: acc.id,
            toolId: acc.toolId,
            actionType: "claim",
            resultStatus: "skipped",
            messageSanitized: `${acc.label} 今日已领取，幂等校验自动跳过`,
          });
        } else {
          setBatchProgress({
            action: "claim",
            running: true,
            total: candidates.length,
            completed: i,
            currentLabel: acc.label,
            phase: "claiming",
          });
          await sleep(550);
          claimedCount++;
          const now = new Date().toISOString();
          const bonus = 100;
          setAccounts((prev) =>
            prev.map((item) => {
              if (item.id !== acc.id || !item.latestQuota) return item;
              return {
                ...item,
                lastClaimedAt: now,
                updatedAt: now,
                latestQuota: {
                  ...item.latestQuota,
                  claimStatus: "claimed",
                  fetchedAt: now,
                  buckets: item.latestQuota.buckets.map((b, idx) =>
                    idx === 0
                      ? {
                          ...b,
                          total: b.total + bonus,
                          remaining: b.remaining + bonus,
                        }
                      : b
                  ),
                },
              };
            })
          );
          appendLog({
            accountId: acc.id,
            toolId: acc.toolId,
            actionType: "claim",
            resultStatus: "success",
            messageSanitized: `${acc.label} (${acc.uidMasked}) 成功领取 +${bonus} 额度`,
          });
        }

        if (i < candidates.length - 1) {
          const jitterMs =
            jitterRangeMs[0] +
            Math.floor(Math.random() * Math.max(200, jitterRangeMs[1] - jitterRangeMs[0]));
          setBatchProgress({
            action: "claim",
            running: true,
            total: candidates.length,
            completed: i + 1,
            currentLabel: candidates[i + 1].label,
            phase: "jitter_wait",
            jitterSeconds: Number((jitterMs / 1000).toFixed(1)),
          });
          await sleep(Math.min(jitterMs, 1600));
        }
      }

      const summaryText = `串行领取完成：成功 ${claimedCount} 个，幂等跳过 ${skippedCount} 个，异常 0 个`;
      setBatchProgress({
        action: "claim",
        running: false,
        total: candidates.length,
        completed: candidates.length,
        phase: "done",
        summaryText,
      });
    },
    [accounts, adapters, appendLog, batchProgress?.running, isTauriRuntime, jitterRangeMs, showToast]
  );

  const requestColdSwitch = useCallback(
    (account: AccountSummary) => {
      setSwitchDialog({ account, phase: "confirm", isClientRunning: true });
      if (isTauriRuntime) {
        void invoke<ClientHostStatus>("detect_client_status", { toolId: account.toolId })
          .then((status) => {
            setSwitchDialog((prev) =>
              prev.account && prev.account.id === account.id
                ? { ...prev, isClientRunning: status.isRunning }
                : prev
            );
          })
          .catch(() => {});
      }
    },
    [isTauriRuntime]
  );

  const cancelColdSwitch = useCallback(() => {
    setSwitchDialog({ account: null, phase: "confirm" });
  }, []);

  const confirmColdSwitch = useCallback(async () => {
    const target = switchDialog.account;
    if (!target) return;

    if (isTauriRuntime) {
      setSwitchDialog({
        account: target,
        phase: switchDialog.isClientRunning === false ? "writing" : "closing",
        isClientRunning: switchDialog.isClientRunning,
      });
      try {
        await invoke("switch_desktop_account", {
          accountId: target.id,
          forceKill: true,
        });
        await syncFromBackend();
        void refreshClientStatuses([target.toolId]);
        setSwitchDialog({ account: target, phase: "done" });
        setTimeout(() => {
          setSwitchDialog({ account: null, phase: "confirm" });
          showToast(`本机客户端已切换至 ${target.label}`, "success");
        }, 650);
      } catch (err) {
        void refreshClientStatuses([target.toolId]);
        setSwitchDialog({ account: null, phase: "confirm" });
        showToast(`切换失败：${String(err)}`, "error");
      }
      return;
    }

    setSwitchDialog({ account: target, phase: "closing" });
    await sleep(600);
    setSwitchDialog({ account: target, phase: "writing" });
    await sleep(550);
    setSwitchDialog({ account: target, phase: "launching" });
    await sleep(600);

    setAccounts((prev) =>
      prev.map((acc) =>
        acc.toolId === target.toolId
          ? { ...acc, isActiveOnClient: acc.id === target.id }
          : acc
      )
    );
    appendLog({
      accountId: target.id,
      toolId: target.toolId,
      actionType: "switch",
      resultStatus: "success",
      messageSanitized: `已将本机客户端切换至 ${target.label} (${target.uidMasked}) 并重新拉起`,
    });
    setSwitchDialog({ account: target, phase: "done" });
    setTimeout(() => {
      setSwitchDialog({ account: null, phase: "confirm" });
      showToast(`本机客户端已切换至 ${target.label}`, "success");
    }, 650);
  }, [
    appendLog,
    isTauriRuntime,
    refreshClientStatuses,
    showToast,
    switchDialog.account,
    switchDialog.isClientRunning,
    syncFromBackend,
  ]);

  const importAccount = useCallback(
    async (
      toolId: ToolId,
      label: string,
      method: AuthMethod,
      rawInput?: string,
      targetAccountId?: string
    ) => {
      const trimmedLabel = label.trim();
      const displayLabel = trimmedLabel || `${toolId.toUpperCase()} 新账号`;

      if (isTauriRuntime) {
        const toolName = adapters.find((a) => a.id === toolId)?.name ?? toolId;
        try {
          if (method === "local_import") {
            const imported = await invoke<AccountSummary>("import_local_account", {
              toolId,
              label: trimmedLabel,
              targetAccountId: targetAccountId ?? null,
            });
            await syncFromBackend();
            if (imported.hasApiToken === false) {
              showToast(
                `已导入 ${imported.label} 切号快照，请点击卡片「在线授权」开启查额与签到`,
                "warning"
              );
            } else if (targetAccountId) {
              showToast(`已关联本机快照：${imported.label}`, "success");
            } else {
              showToast(`已导入账号：${imported.label}`, "success");
            }
            return;
          } else if (method === "token_paste") {
            await invoke<AccountSummary>("add_account_by_token", {
              toolId,
              label: trimmedLabel,
              rawInput: rawInput ?? "",
              targetAccountId: targetAccountId ?? null,
            });
          } else if (method === "webview_oauth") {
            await invoke("start_webview_oauth", {
              toolId,
              label: trimmedLabel,
              targetAccountId: targetAccountId ?? null,
            });
            showToast(`已打开 ${toolName} 内置授权窗口，请在窗口中完成登录`, "info");
            return;
          }
          await syncFromBackend();
          showToast(
            targetAccountId
              ? "已更新账号凭据"
              : `已导入账号${trimmedLabel ? `：${trimmedLabel}` : ""}`,
            "success"
          );
        } catch (err) {
          showToast(`导入失败：${String(err)}`, "error");
          throw err;
        }
        return;
      }

      const now = new Date().toISOString();
      if (targetAccountId) {
        const target = accounts.find((a) => a.id === targetAccountId);
        setAccounts((prev) =>
          prev.map((acc) =>
            acc.id === targetAccountId
              ? {
                  ...acc,
                  label: trimmedLabel || acc.label,
                  status: "active",
                  hasLocalSnapshot: true,
                  hasApiToken: true,
                  updatedAt: now,
                }
              : acc
          )
        );
        appendLog({
          accountId: targetAccountId,
          toolId,
          actionType: "import",
          resultStatus: "success",
          messageSanitized: `通过 ${method} 补充/更新账号 ${trimmedLabel || target?.label || targetAccountId} 凭据`,
        });
        showToast("已补充/更新账号凭据", "success");
        return;
      }

      const randHex = Math.random().toString(16).slice(2, 6);
      const id = `acc-${toolId}-${Date.now()}`;
      const supportsClaim =
        toolId === "trae_cn" || toolId === "workbuddy" || toolId === "qoder_cn";
      const newAcc: AccountSummary = {
        id,
        toolId,
        label: displayLabel,
        uidMasked: `${toolId.slice(0, 2)}_${randHex}****9f`,
        status: "active",
        isActiveOnClient: false,
        hasLocalSnapshot: method === "local_import" || toolId !== "qoder_cn",
        hasApiToken: method !== "local_import" || toolId !== "qoder_cn",
        latestQuota: {
          planName: "Free",
          claimStatus: supportsClaim ? "available" : "unsupported",
          fetchedAt: now,
          buckets: [
            {
              key: "default",
              label: supportsClaim ? "可用额度" : "快速请求额度",
              used: 0,
              total: 500,
              remaining: 500,
              unit: supportsClaim ? "Credits" : "次",
            },
          ],
        },
        updatedAt: now,
      };
      setAccounts((prev) => [newAcc, ...prev]);
      appendLog({
        accountId: id,
        toolId,
        actionType: "import",
        resultStatus: "success",
        messageSanitized: `通过 ${method} 导入账号 ${newAcc.label} (${newAcc.uidMasked})`,
      });
      showToast(`已导入账号：${newAcc.label}`, "success");
    },
    [accounts, adapters, appendLog, isTauriRuntime, showToast, syncFromBackend]
  );

  const renameAccount = useCallback(
    async (accountId: string, label: string) => {
      const trimmed = label.trim();
      if (!trimmed) {
        showToast("账号备注不能为空", "warning");
        return;
      }

      if (isTauriRuntime) {
        try {
          await invoke("rename_account", { accountId, label: trimmed });
          await syncFromBackend();
          showToast(`已重命名为 ${trimmed}`, "success");
        } catch (err) {
          showToast(`重命名失败：${String(err)}`, "error");
        }
        return;
      }

      setAccounts((prev) =>
        prev.map((acc) =>
          acc.id === accountId ? { ...acc, label: trimmed, updatedAt: new Date().toISOString() } : acc
        )
      );
      showToast(`已重命名为 ${trimmed}`, "success");
    },
    [isTauriRuntime, showToast, syncFromBackend]
  );

  const toggleAccountDisabled = useCallback(
    async (accountId: string) => {
      const target = accounts.find((a) => a.id === accountId);
      if (!target) return;
      const nextStatus = target.status === "disabled" ? "active" : "disabled";

      if (isTauriRuntime) {
        try {
          await invoke("update_account_status", { accountId, status: nextStatus });
          await syncFromBackend();
          showToast(
            nextStatus === "disabled"
              ? `已停用 ${target.label}（将跳过批量操作）`
              : `已恢复启用 ${target.label}`,
            "info"
          );
        } catch (err) {
          showToast(`更新状态失败：${String(err)}`, "error");
        }
        return;
      }

      setAccounts((prev) =>
        prev.map((acc) => (acc.id === accountId ? { ...acc, status: nextStatus } : acc))
      );
      showToast(
        nextStatus === "disabled"
          ? `已停用 ${target.label}（将跳过批量操作）`
          : `已恢复启用 ${target.label}`,
        "info"
      );
    },
    [accounts, isTauriRuntime, showToast, syncFromBackend]
  );

  const recoverAccount = useCallback(
    async (accountId: string) => {
      const target = accounts.find((a) => a.id === accountId);
      if (!target) return;

      if (isTauriRuntime) {
        const adapter = adapters.find((a) => a.id === target.toolId);
        const canProbeQuota =
          target.hasApiToken !== false && adapter?.capabilities.supportsQuotaQuery === true;
        try {
          if (canProbeQuota) {
            // 仅查额探测；后端仅在查额成功时将 Expired/RiskPaused 恢复为 Active，失败保留原状态
            await invoke("refresh_account_quota", { accountId });
          } else {
            await invoke("update_account_status", { accountId, status: "active" });
          }
          await syncFromBackend();
          showToast(`${target.label} 已恢复正常状态`, "success");
        } catch (err) {
          await syncFromBackend();
          showToast(`恢复失败：${String(err)}`, "error");
        }
        return;
      }

      setAccounts((prev) =>
        prev.map((acc) => (acc.id === accountId ? { ...acc, status: "active" } : acc))
      );
      appendLog({
        accountId,
        toolId: target.toolId,
        actionType: "import",
        resultStatus: "success",
        messageSanitized: `已重新验证并恢复 ${target.label} 的正常状态`,
      });
      showToast(`${target.label} 已恢复正常状态`, "success");
    },
    [accounts, adapters, appendLog, isTauriRuntime, showToast, syncFromBackend]
  );

  const deleteAccount = useCallback(
    async (accountId: string) => {
      const target = accounts.find((a) => a.id === accountId);
      if (!target) return;

      if (isTauriRuntime) {
        try {
          await invoke("delete_account", { accountId });
          await syncFromBackend();
          showToast(`已删除账号：${target.label}`, "success");
        } catch (err) {
          showToast(`删除失败：${String(err)}`, "error");
        }
        return;
      }

      setAccounts((prev) => prev.filter((acc) => acc.id !== accountId));
      appendLog({
        accountId,
        toolId: target.toolId,
        actionType: "import",
        resultStatus: "success",
        messageSanitized: `已删除账号 ${target.label} (${target.uidMasked})`,
      });
      showToast(`已删除账号：${target.label}`, "success");
    },
    [accounts, appendLog, isTauriRuntime, showToast, syncFromBackend]
  );

  const logs = useMemo(() => {
    if (isTauriRuntime) return logsState;
    return logsState.filter((item) =>
      logScope === "archived" ? Boolean(item.archivedAt) : !item.archivedAt
    );
  }, [isTauriRuntime, logScope, logsState]);

  const logStats = useMemo<ActionLogStats>(() => {
    if (isTauriRuntime) return backendLogStats;
    const active = logsState.filter((item) => !item.archivedAt);
    const archived = logsState.filter((item) => Boolean(item.archivedAt));
    return {
      activeCount: active.length,
      archivedCount: archived.length,
      failedActiveCount: active.filter((item) => item.resultStatus === "failed").length,
      noiseActiveCount: active.filter(isRedundantLog).length,
    };
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
          return 0;
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
          return 0;
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
          return 0;
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

  const dismissBatchProgress = useCallback(() => {
    setBatchProgress(null);
  }, []);

  const quitApp = useCallback(async () => {
    if (isTauriRuntime) {
      await invoke("quit_app");
      return;
    }
    showToast("Mock 模式下无需退出应用", "info");
  }, [isTauriRuntime, showToast]);

  const triggerAutoClaimAvailable = useCallback(async () => {
    if (isTrayWindow || batchRunningRef.current) return;
    try {
      const latestAccounts = isTauriRuntime
        ? await invoke<AccountSummary[]>("list_accounts", { toolId: null })
        : accountsRef.current;
      const availableIds = latestAccounts
        .filter((acc) => {
          const adapter = adaptersRef.current.find((ad) => ad.id === acc.toolId);
          return (
            acc.status === "active" &&
            acc.hasApiToken !== false &&
            Boolean(adapter?.capabilities.supportsDailyClaim) &&
            getEffectiveClaimStatus(acc) === "available"
          );
        })
        .map((acc) => acc.id);
      if (availableIds.length === 0) return;
      if (isTauriRuntime) {
        await invoke("claim_daily_batch", { accountIds: availableIds });
      } else {
        await claimDailyBatch(availableIds);
      }
    } catch {
      // background automation errors should not crash UI
    }
  }, [claimDailyBatch, isTauriRuntime, isTrayWindow]);

  const refreshAllQuotasRef = useRef(refreshAllQuotas);
  refreshAllQuotasRef.current = refreshAllQuotas;
  const triggerAutoClaimAvailableRef = useRef(triggerAutoClaimAvailable);
  triggerAutoClaimAvailableRef.current = triggerAutoClaimAvailable;

  useEffect(() => {
    const timer = window.setInterval(() => {
      const nextDay = toCstDayKey(Date.now());
      if (nextDay !== cstDayKeyRef.current) {
        cstDayKeyRef.current = nextDay;
        setCstDayKey(nextDay);
        if (isTauriRuntime) {
          void syncFromBackend();
        }
      }
    }, 60_000);
    return () => window.clearInterval(timer);
  }, [isTauriRuntime, syncFromBackend]);

  useEffect(() => {
    if (isTrayWindow || isInitializing || startupAutoRunDoneRef.current) return;
    const timer = window.setTimeout(() => {
      if (startupAutoRunDoneRef.current) return;
      startupAutoRunDoneRef.current = true;
      const currentSettings = appSettingsRef.current;
      const hasRefreshable = accountsRef.current.some((a) => {
        if (a.status !== "active" || a.hasApiToken === false) return false;
        const ad = adaptersRef.current.find((item) => item.id === a.toolId);
        return ad?.capabilities.supportsQuotaQuery === true;
      });
      if (!hasRefreshable || batchRunningRef.current) return;
      if (currentSettings.autoRefreshOnStartup) {
        pendingAutoClaimRef.current = Boolean(currentSettings.autoClaimDaily);
        void refreshAllQuotasRef.current().then((started) => {
          if (!started) {
            pendingAutoClaimRef.current = false;
          }
        });
      } else if (currentSettings.autoClaimDaily) {
        void triggerAutoClaimAvailableRef.current();
      }
    }, 700);
    return () => window.clearTimeout(timer);
  }, [isInitializing, isTrayWindow]);

  useEffect(() => {
    if (isTrayWindow || isInitializing) return;
    const mins = appSettings.autoRefreshIntervalMinutes ?? 0;
    if (mins <= 0) return;
    const intervalId = window.setInterval(() => {
      if (batchRunningRef.current) return;
      // 主窗口隐藏到托盘后不再静默刷新（Human-in-the-Loop 边界）
      if (typeof document !== "undefined" && document.visibilityState !== "visible") return;
      const hasRefreshable = accountsRef.current.some((a) => {
        if (a.status !== "active" || a.hasApiToken === false) return false;
        const ad = adaptersRef.current.find((item) => item.id === a.toolId);
        return ad?.capabilities.supportsQuotaQuery === true;
      });
      if (!hasRefreshable) return;
      pendingAutoClaimRef.current = Boolean(appSettingsRef.current.autoClaimDaily);
      void refreshAllQuotasRef.current().then((started) => {
        if (!started) {
          pendingAutoClaimRef.current = false;
        }
      });
    }, mins * 60_000);
    return () => window.clearInterval(intervalId);
  }, [appSettings.autoRefreshIntervalMinutes, isInitializing, isTrayWindow]);

  const contextValue = useMemo<QwitchContextValue>(
    () => ({
      isTauriRuntime,
      isInitializing,
      adapters,
      accounts,
      logs,
      logScope,
      setLogScope,
      logStats,
      hasMoreLogs,
      loadMoreLogs,
      batchProgress,
      switchDialog,
      toast,
      showToast,
      dismissToast,
      theme,
      toggleTheme,
      appSettings,
      jitterRangeMs,
      setJitterRangeMs,
      logRetention,
      setLogRetention,
      customClientPaths,
      clientStatuses,
      saveAppSettings,
      refreshClientStatuses,
      refreshAccountQuota,
      refreshAllQuotas,
      claimDailyBatch,
      requestColdSwitch,
      cancelColdSwitch,
      confirmColdSwitch,
      importAccount,
      renameAccount,
      toggleAccountDisabled,
      recoverAccount,
      deleteAccount,
      archiveLogs,
      unarchiveLogs,
      deleteLogs,
      exportLogs,
      dismissBatchProgress,
      quitApp,
      cstDayKey,
    }),
    [
      isTauriRuntime,
      isInitializing,
      adapters,
      accounts,
      logs,
      logScope,
      logStats,
      hasMoreLogs,
      loadMoreLogs,
      batchProgress,
      switchDialog,
      toast,
      showToast,
      dismissToast,
      theme,
      toggleTheme,
      appSettings,
      jitterRangeMs,
      setJitterRangeMs,
      logRetention,
      setLogRetention,
      customClientPaths,
      clientStatuses,
      saveAppSettings,
      refreshClientStatuses,
      refreshAccountQuota,
      refreshAllQuotas,
      claimDailyBatch,
      requestColdSwitch,
      cancelColdSwitch,
      confirmColdSwitch,
      importAccount,
      renameAccount,
      toggleAccountDisabled,
      recoverAccount,
      deleteAccount,
      archiveLogs,
      unarchiveLogs,
      deleteLogs,
      exportLogs,
      dismissBatchProgress,
      quitApp,
      cstDayKey,
    ]
  );

  return <QwitchContext.Provider value={contextValue}>{children}</QwitchContext.Provider>;
}

export function useQwitch() {
  const ctx = useContext(QwitchContext);
  if (!ctx) throw new Error("useQwitch must be used within QwitchProvider");
  return ctx;
}
