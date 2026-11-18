import React, { createContext, useContext, useState, useCallback, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { INITIAL_ACCOUNTS, INITIAL_LOGS, MOCK_ADAPTERS } from "../data/mockStore";
import type {
  AccountSummary,
  ActionLogCleanupRequest,
  ActionLogItem,
  ActionLogStats,
  AdapterManifest,
  AppSettings,
  AuthMethod,
  ClientHostStatus,
  ExportLogsResult,
  LogScope,
  ToolId,
} from "../types/qwitch";

export interface BatchProgressState {
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
  adapters: AdapterManifest[];
  accounts: AccountSummary[];
  logs: ActionLogItem[];
  logScope: "active" | "archived";
  setLogScope: (scope: "active" | "archived") => void;
  logStats: ActionLogStats;
  batchProgress: BatchProgressState | null;
  switchDialog: SwitchDialogState;
  toastMessage: string | null;
  theme: "light" | "dark";
  toggleTheme: () => void;
  jitterRangeMs: [number, number];
  setJitterRangeMs: (range: [number, number]) => void;
  logRetention: { archiveAfterDays: number; maxActiveCount: number };
  setLogRetention: (next: { archiveAfterDays: number; maxActiveCount: number }) => void;
  customClientPaths: Record<string, string>;
  clientStatuses: Record<string, ClientHostStatus>;
  saveAppSettings: (next: Partial<AppSettings>, silent?: boolean) => Promise<void>;
  refreshClientStatuses: (toolIds?: ToolId[]) => Promise<void>;
  refreshAccountQuota: (accountId: string) => Promise<void>;
  refreshAllQuotas: () => Promise<void>;
  claimDailyBatch: (targetIds?: string[]) => Promise<void>;
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
  recoverAccount: (accountId: string) => Promise<void>;
  deleteAccount: (accountId: string) => Promise<void>;
  archiveLogs: (request: ActionLogCleanupRequest, feedbackLabel?: string) => Promise<number>;
  unarchiveLogs: (request: ActionLogCleanupRequest, feedbackLabel?: string) => Promise<number>;
  deleteLogs: (request: ActionLogCleanupRequest, feedbackLabel?: string) => Promise<number>;
  exportLogs: (scope?: LogScope, archiveAfterExport?: boolean) => Promise<void>;
  dismissBatchProgress: () => void;
}

const QwitchContext = createContext<QwitchContextValue | null>(null);

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const detectTauriRuntime = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

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

  const [adapters, setAdapters] = useState<AdapterManifest[]>(MOCK_ADAPTERS);
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
  const [switchDialog, setSwitchDialog] = useState<SwitchDialogState>({ account: null, phase: "confirm" });
  const [toastMessage, setToastMessage] = useState<string | null>(null);
  const [theme, setTheme] = useState<"light" | "dark">("light");
  const [jitterRangeMs, setJitterRangeMs] = useState<[number, number]>([1500, 3500]);
  const [logRetention, setLogRetention] = useState<{
    archiveAfterDays: number;
    maxActiveCount: number;
  }>({ archiveAfterDays: 14, maxActiveCount: 200 });
  const [customClientPaths, setCustomClientPaths] = useState<Record<string, string>>({});
  const [clientStatuses, setClientStatuses] = useState<Record<string, ClientHostStatus>>({});

  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
  }, [theme]);

  const showToast = useCallback((msg: string) => {
    setToastMessage(msg);
    setTimeout(() => {
      setToastMessage((prev) => (prev === msg ? null : prev));
    }, 3400);
  }, []);

  const saveAppSettings = useCallback(
    async (next: Partial<AppSettings>, silent = false) => {
      const payload: AppSettings = {
        jitterMinMs: next.jitterMinMs ?? jitterRangeMs[0],
        jitterMaxMs: next.jitterMaxMs ?? jitterRangeMs[1],
        theme: next.theme ?? theme,
        customClientPaths: next.customClientPaths ?? customClientPaths,
        logArchiveAfterDays: next.logArchiveAfterDays ?? logRetention.archiveAfterDays,
        logMaxActiveCount: next.logMaxActiveCount ?? logRetention.maxActiveCount,
      };
      if (payload.jitterMinMs > payload.jitterMaxMs) {
        showToast("保存失败：最小间隔不能大于最大间隔");
        return;
      }

      setJitterRangeMs([payload.jitterMinMs, payload.jitterMaxMs]);
      setTheme(payload.theme);
      setCustomClientPaths(payload.customClientPaths);
      setLogRetention({
        archiveAfterDays: payload.logArchiveAfterDays ?? 14,
        maxActiveCount: payload.logMaxActiveCount ?? 200,
      });

      if (isTauriRuntime) {
        try {
          const saved = await invoke<AppSettings>("save_app_settings", { settings: payload });
          setJitterRangeMs([saved.jitterMinMs, saved.jitterMaxMs]);
          setTheme(saved.theme === "dark" ? "dark" : "light");
          setCustomClientPaths(saved.customClientPaths ?? {});
          setLogRetention({
            archiveAfterDays: saved.logArchiveAfterDays ?? 14,
            maxActiveCount: saved.logMaxActiveCount ?? 200,
          });
          if (!silent) showToast("设置已保存");
        } catch (err) {
          showToast(`保存设置失败：${String(err)}`);
        }
      } else if (!silent) {
        showToast("设置已保存（Mock 模式）");
      }
    },
    [customClientPaths, isTauriRuntime, jitterRangeMs, logRetention, showToast, theme]
  );

  const toggleTheme = useCallback(() => {
    const nextTheme: "light" | "dark" = theme === "dark" ? "light" : "dark";
    void saveAppSettings({ theme: nextTheme }, true);
  }, [saveAppSettings, theme]);

  const refreshClientStatuses = useCallback(
    async (toolIds?: ToolId[]) => {
      const targets =
        toolIds ??
        adapters.filter((a) => a.capabilities.supportsClientSwitch).map((a) => a.id);

      if (isTauriRuntime) {
        const entries = await Promise.all(
          targets.map(async (tid) => {
            try {
              const status = await invoke<ClientHostStatus>("detect_client_status", { toolId: tid });
              return [tid, status] as const;
            } catch {
              return [
                tid,
                { toolId: tid, isRunning: false } as ClientHostStatus,
              ] as const;
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
    [adapters, isTauriRuntime]
  );

  const refreshLogs = useCallback(
    async (targetScope?: "active" | "archived") => {
      if (!isTauriRuntime) return;
      const scope = targetScope ?? logScope;
      try {
        const [logList, stats] = await Promise.all([
          invoke<ActionLogItem[]>("get_action_logs", { scope, limit: 200, offset: 0 }),
          invoke<ActionLogStats>("get_action_log_stats"),
        ]);
        setLogsState(logList);
        setBackendLogStats(stats);
      } catch (err) {
        showToast(`刷新日志失败：${String(err)}`);
      }
    },
    [isTauriRuntime, logScope, showToast]
  );

  const syncFromBackend = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const [adList, accList, logList, stats, settings] = await Promise.all([
        invoke<AdapterManifest[]>("list_adapters"),
        invoke<AccountSummary[]>("list_accounts", { toolId: null }),
        invoke<ActionLogItem[]>("get_action_logs", { scope: logScope, limit: 200, offset: 0 }),
        invoke<ActionLogStats>("get_action_log_stats"),
        invoke<AppSettings>("get_app_settings"),
      ]);
      setAdapters(adList);
      setAccounts(accList);
      setLogsState(logList);
      setBackendLogStats(stats);
      setJitterRangeMs([settings.jitterMinMs, settings.jitterMaxMs]);
      setTheme(settings.theme === "dark" ? "dark" : "light");
      setCustomClientPaths(settings.customClientPaths ?? {});
      setLogRetention({
        archiveAfterDays: settings.logArchiveAfterDays ?? 14,
        maxActiveCount: settings.logMaxActiveCount ?? 200,
      });
    } catch (err) {
      showToast(`同步后端数据失败：${String(err)}`);
    }
  }, [isTauriRuntime, logScope, showToast]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    void refreshLogs(logScope);
  }, [isTauriRuntime, logScope, refreshLogs]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    void syncFromBackend();

    let unlistenProgress: (() => void) | undefined;
    let unlistenUpdated: (() => void) | undefined;
    let unlistenOauthErr: (() => void) | undefined;
    let unlistenSwitch: (() => void) | undefined;
    let unlistenSettings: (() => void) | undefined;
    let unlistenLogs: (() => void) | undefined;

    void listen<BatchProgressState>("qwitch://batch-progress", (event) => {
      setBatchProgress(event.payload);
      if (!event.payload.running) {
        void syncFromBackend();
      }
    }).then((fn) => {
      unlistenProgress = fn;
    });

    void listen<string>("qwitch://account-updated", () => {
      void syncFromBackend();
    }).then((fn) => {
      unlistenUpdated = fn;
    });

    void listen<string>("qwitch://oauth-error", (event) => {
      showToast(event.payload);
    }).then((fn) => {
      unlistenOauthErr = fn;
    });

    void listen<{ accountId: string; phase: SwitchDialogState["phase"] }>(
      "qwitch://switch-progress",
      (event) => {
        setSwitchDialog((prev) =>
          prev.account && prev.account.id === event.payload.accountId
            ? { ...prev, phase: event.payload.phase }
            : prev
        );
      }
    ).then((fn) => {
      unlistenSwitch = fn;
    });

    void listen<AppSettings>("qwitch://settings-updated", (event) => {
      setJitterRangeMs([event.payload.jitterMinMs, event.payload.jitterMaxMs]);
      setTheme(event.payload.theme === "dark" ? "dark" : "light");
      setCustomClientPaths(event.payload.customClientPaths ?? {});
      setLogRetention({
        archiveAfterDays: event.payload.logArchiveAfterDays ?? 14,
        maxActiveCount: event.payload.logMaxActiveCount ?? 200,
      });
    }).then((fn) => {
      unlistenSettings = fn;
    });

    void listen("qwitch://logs-updated", () => {
      void refreshLogs();
    }).then((fn) => {
      unlistenLogs = fn;
    });

    return () => {
      unlistenProgress?.();
      unlistenUpdated?.();
      unlistenOauthErr?.();
      unlistenSwitch?.();
      unlistenSettings?.();
      unlistenLogs?.();
    };
  }, [isTauriRuntime, refreshLogs, showToast, syncFromBackend]);

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
          showToast(`已刷新 ${target.label} 额度`);
        } catch (err) {
          await syncFromBackend();
          showToast(`刷新失败：${String(err)}`);
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
      showToast(`已刷新 ${target.label} 额度`);
    },
    [accounts, appendLog, isTauriRuntime, showToast, syncFromBackend]
  );

  const refreshAllQuotas = useCallback(async () => {
    if (isTauriRuntime) {
      const activeIds = accounts
        .filter((a) => a.status === "active" && a.hasApiToken !== false)
        .map((a) => a.id);
      for (const id of activeIds) {
        try {
          await invoke("refresh_account_quota", { accountId: id });
        } catch {
          // 单个失败不中断其余账号刷新
        }
      }
      await syncFromBackend();
      showToast("已刷新所有已完成在线授权账号的额度");
      return;
    }

    await sleep(600);
    const now = new Date().toISOString();
    setAccounts((prev) =>
      prev.map((acc) =>
        acc.status === "active" && acc.latestQuota
          ? { ...acc, updatedAt: now, latestQuota: { ...acc.latestQuota, fetchedAt: now } }
          : acc
      )
    );
    showToast("已刷新所有正常状态账号的额度");
  }, [accounts, isTauriRuntime, showToast, syncFromBackend]);

  const claimDailyBatch = useCallback(
    async (targetIds?: string[]) => {
      const candidates = accounts.filter((acc) => {
        if (targetIds && !targetIds.includes(acc.id)) return false;
        const adapter = adapters.find((ad) => ad.id === acc.toolId);
        return (
          acc.status === "active" &&
          acc.hasApiToken !== false &&
          Boolean(adapter?.capabilities.supportsDailyClaim)
        );
      });

      if (candidates.length === 0) {
        const missingTokenCount = accounts.filter((acc) => {
          if (targetIds && !targetIds.includes(acc.id)) return false;
          const adapter = adapters.find((ad) => ad.id === acc.toolId);
          return (
            acc.status === "active" &&
            acc.hasApiToken === false &&
            Boolean(adapter?.capabilities.supportsDailyClaim)
          );
        }).length;
        showToast(
          missingTokenCount > 0
            ? "账号尚未完成在线授权，请先点击卡片「在线授权」后再领取"
            : "当前没有可参与领取的正常账号"
        );
        return;
      }

      if (isTauriRuntime) {
        try {
          await invoke("claim_daily_batch", {
            accountIds: candidates.map((c) => c.id),
          });
        } catch (err) {
          showToast(`启动领取队列失败：${String(err)}`);
        }
        return;
      }

      let claimedCount = 0;
      let skippedCount = 0;

      setBatchProgress({
        running: true,
        total: candidates.length,
        completed: 0,
        currentLabel: candidates[0].label,
        phase: "checking",
      });

      for (let i = 0; i < candidates.length; i++) {
        const acc = candidates[i];
        setBatchProgress({
          running: true,
          total: candidates.length,
          completed: i,
          currentLabel: acc.label,
          phase: "checking",
        });
        await sleep(450);

        if (acc.latestQuota?.claimStatus === "claimed") {
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
            jitterRangeMs[0] + Math.floor(Math.random() * Math.max(200, jitterRangeMs[1] - jitterRangeMs[0]));
          setBatchProgress({
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

      setBatchProgress({
        running: false,
        total: candidates.length,
        completed: candidates.length,
        phase: "done",
        summaryText: `串行领取完成：成功 ${claimedCount} 个，幂等跳过 ${skippedCount} 个`,
      });
    },
    [accounts, adapters, appendLog, isTauriRuntime, jitterRangeMs, showToast]
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
        setSwitchDialog({ account: target, phase: "done" });
        setTimeout(() => {
          setSwitchDialog({ account: null, phase: "confirm" });
          showToast(`本机客户端已切换至 ${target.label}`);
        }, 650);
      } catch (err) {
        setSwitchDialog({ account: null, phase: "confirm" });
        showToast(`切换失败：${String(err)}`);
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
      showToast(`本机客户端已切换至 ${target.label}`);
    }, 650);
  }, [appendLog, isTauriRuntime, showToast, switchDialog.account, switchDialog.isClientRunning, syncFromBackend]);

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
              showToast(`已导入 ${imported.label} 切号快照，请点击卡片「在线授权」开启查额与签到`);
            } else if (targetAccountId) {
              showToast(`已关联本机快照：${imported.label}`);
            } else {
              showToast(`已导入账号：${imported.label}`);
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
            showToast(`已打开 ${toolName} 内置授权窗口，请在窗口中完成登录`);
            return;
          }
          await syncFromBackend();
          showToast(
            targetAccountId
              ? "已更新账号凭据"
              : `已导入账号${trimmedLabel ? `：${trimmedLabel}` : ""}`
          );
        } catch (err) {
          showToast(`导入失败：${String(err)}`);
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
        showToast("已补充/更新账号凭据");
        return;
      }

      const randHex = Math.random().toString(16).slice(2, 6);
      const id = `acc-${toolId}-${Date.now()}`;
      const supportsClaim = toolId === "workbuddy" || toolId === "qoder_cn";
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
      showToast(`已导入账号：${newAcc.label}`);
    },
    [accounts, appendLog, isTauriRuntime, showToast, syncFromBackend]
  );

  const recoverAccount = useCallback(
    async (accountId: string) => {
      const target = accounts.find((a) => a.id === accountId);
      if (!target) return;

      if (isTauriRuntime) {
        try {
          await invoke("update_account_status", { accountId, status: "active" });
          await invoke("refresh_account_quota", { accountId });
          await syncFromBackend();
          showToast(`${target.label} 已恢复正常状态`);
        } catch (err) {
          showToast(`恢复失败：${String(err)}`);
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
      showToast(`${target.label} 已恢复正常状态`);
    },
    [accounts, appendLog, isTauriRuntime, showToast, syncFromBackend]
  );

  const deleteAccount = useCallback(
    async (accountId: string) => {
      const target = accounts.find((a) => a.id === accountId);
      if (!target) return;

      if (isTauriRuntime) {
        try {
          await invoke("delete_account", { accountId });
          await syncFromBackend();
          showToast(`已删除账号：${target.label}`);
        } catch (err) {
          showToast(`删除失败：${String(err)}`);
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
      showToast(`已删除账号：${target.label}`);
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

  const archiveLogs = useCallback(
    async (request: ActionLogCleanupRequest, feedbackLabel?: string) => {
      if (isTauriRuntime) {
        try {
          const count = await invoke<number>("archive_action_logs", { request });
          await refreshLogs();
          if (count === 0) {
            showToast("当前没有可归档的匹配日志");
          } else {
            showToast(feedbackLabel ?? `已归档 ${count} 条日志`);
          }
          return count;
        } catch (err) {
          showToast(`归档日志失败：${String(err)}`);
          return 0;
        }
      }

      const now = new Date().toISOString();
      let count = 0;
      setLogsState((prev) =>
        prev.map((item) => {
          if (matchesLogCleanup(item, request, "active")) {
            count += 1;
            return { ...item, archivedAt: now };
          }
          return item;
        })
      );
      showToast(count === 0 ? "当前没有可归档的匹配日志" : feedbackLabel ?? `已归档 ${count} 条日志`);
      return count;
    },
    [isTauriRuntime, refreshLogs, showToast]
  );

  const unarchiveLogs = useCallback(
    async (request: ActionLogCleanupRequest, feedbackLabel?: string) => {
      if (isTauriRuntime) {
        try {
          const count = await invoke<number>("unarchive_action_logs", { request });
          await refreshLogs();
          showToast(
            count === 0
              ? "没有可恢复的归档日志"
              : feedbackLabel ?? `已恢复 ${count} 条日志到活跃视图`
          );
          return count;
        } catch (err) {
          showToast(`恢复日志失败：${String(err)}`);
          return 0;
        }
      }

      let count = 0;
      setLogsState((prev) =>
        prev.map((item) => {
          if (matchesLogCleanup(item, request, "archived")) {
            count += 1;
            return { ...item, archivedAt: undefined };
          }
          return item;
        })
      );
      showToast(
        count === 0 ? "没有可恢复的归档日志" : feedbackLabel ?? `已恢复 ${count} 条日志到活跃视图`
      );
      return count;
    },
    [isTauriRuntime, refreshLogs, showToast]
  );

  const deleteLogs = useCallback(
    async (request: ActionLogCleanupRequest, feedbackLabel?: string) => {
      if (isTauriRuntime) {
        try {
          const count = await invoke<number>("delete_action_logs", { request });
          await refreshLogs();
          showToast(count === 0 ? "没有可删除的匹配日志" : feedbackLabel ?? `已删除 ${count} 条日志`);
          return count;
        } catch (err) {
          showToast(`删除日志失败：${String(err)}`);
          return 0;
        }
      }

      let count = 0;
      setLogsState((prev) =>
        prev.filter((item) => {
          const matched = matchesLogCleanup(item, request, logScope);
          if (matched) count += 1;
          return !matched;
        })
      );
      showToast(count === 0 ? "没有可删除的匹配日志" : feedbackLabel ?? `已删除 ${count} 条日志`);
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
              : `已导出 ${res.count} 条至 archives/${fileName}`
          );
        } catch (err) {
          showToast(`导出日志失败：${String(err)}`);
        }
        return;
      }

      const targetItems = logsState.filter((item) => {
        if (scope === "active") return !item.archivedAt;
        if (scope === "archived") return Boolean(item.archivedAt);
        return true;
      });
      if (targetItems.length === 0) {
        showToast("当前视图没有可导出的操作日志");
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
        showToast(`已导出 ${targetItems.length} 条操作日志`);
      }
    },
    [archiveLogs, isTauriRuntime, logScope, logsState, refreshLogs, showToast]
  );

  const dismissBatchProgress = useCallback(() => {
    setBatchProgress(null);
  }, []);

  return (
    <QwitchContext.Provider
      value={{
        isTauriRuntime,
        adapters,
        accounts,
        logs,
        logScope,
        setLogScope,
        logStats,
        batchProgress,
        switchDialog,
        toastMessage,
        theme,
        toggleTheme,
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
        recoverAccount,
        deleteAccount,
        archiveLogs,
        unarchiveLogs,
        deleteLogs,
        exportLogs,
        dismissBatchProgress,
      }}
    >
      {children}
    </QwitchContext.Provider>
  );
}

export function useQwitch() {
  const ctx = useContext(QwitchContext);
  if (!ctx) throw new Error("useQwitch must be used within QwitchProvider");
  return ctx;
}
