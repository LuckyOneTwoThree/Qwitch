/**
 * 关于下方若干 `eslint-disable react-hooks/exhaustive-deps` 的说明：
 *
 * 被省略的依赖全部是**引用恒定**的值，纳入依赖数组不会改变行为，只会增加噪音：
 * - `appSettingsRef` / `pendingAutoClaimRef` / `pendingAccountIdsRef` 等：`useRef`
 *   返回的对象在整个组件生命周期内引用不变；
 * - `setBatchProgress` / `setSwitchDialog`：`useState` 的 setter，React 保证引用稳定。
 *
 * 这些依赖刻意不写入数组，是为了让 effect 只在**真正需要重新订阅**的条件
 * （如 `isTauriRuntime`、`syncFromBackend` 等回调身份）变化时重跑，避免因
 * ref/setter 被误当作变化源而频繁重建事件监听。
 */
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getMockAccounts, getMockAdapters } from "../data/mockStoreLoader";
import { getEffectiveClaimStatus, isAccountMissingLocalSnapshot, selectClaimCandidates, toCstDayKey } from "../domain/selectors";
import type {
  AccountSummary,
  ActionLogItem,
  ActionLogStats,
  AdapterManifest,
  AppSettings,
  AppUpdateInfo,
  AuthMethod,
  BatchProgressState,
  ClientHostStatus,
  LocalAccountPreview,
  ProfileInspectResult,
  SwitchDialogState,
  ToolId,
} from "../types/qwitch";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

interface UseAccountStoreDeps {
  isTauriRuntime: boolean;
  isTrayWindow: boolean;
  showToast: (message: string, tone?: "success" | "error" | "warning" | "info") => void;
  checkAppUpdate: (options?: { force?: boolean; silent?: boolean }) => Promise<AppUpdateInfo | null>;
  appendLog: (entry: Omit<ActionLogItem, "id" | "createdAt">) => void;
  refreshLogs: (targetScope?: "active" | "archived") => Promise<void>;
  requestSnapshot: () => { seq: number; scope: "active" | "archived"; limit: number };
  commitSnapshot: (
    list: ActionLogItem[],
    stats: ActionLogStats,
    seq: number,
    scope: "active" | "archived"
  ) => void;
  logScope: "active" | "archived";
  resetPagination: () => void;
  jitterRangeMs: [number, number];
  appSettings: AppSettings;
  appSettingsRef: React.MutableRefObject<AppSettings>;
  applyIncomingSettings: (raw?: Partial<AppSettings> | null, forceDrafts?: boolean) => void;
  batchProgress: BatchProgressState | null;
  setBatchProgress: React.Dispatch<React.SetStateAction<BatchProgressState | null>>;
  switchDialog: SwitchDialogState;
  setSwitchDialog: React.Dispatch<React.SetStateAction<SwitchDialogState>>;
  pendingAutoClaimRef: React.MutableRefObject<boolean>;
}

/**
 * Account 域 store（四 store 中的主 store）：账号池、客户端状态、查额/领取/切号/导入，
 * 以及后端事件监听、定时自动刷新等编排。依赖注入 Log / Settings / Feedback 三个 store。
 */
/** 切号结果：快照是否写入、客户端存活是否确认、补充说明 */
interface SwitchDesktopAccountResult {
  switched: boolean;
  livenessConfirmed: boolean;
  note?: string | null;
}

export function useAccountStore({
  isTauriRuntime,
  isTrayWindow,
  showToast,
  checkAppUpdate,
  appendLog,
  refreshLogs,
  requestSnapshot,
  commitSnapshot,
  logScope,
  resetPagination,
  jitterRangeMs,
  appSettings,
  appSettingsRef,
  applyIncomingSettings,
  batchProgress,
  setBatchProgress,
  switchDialog,
  setSwitchDialog,
  pendingAutoClaimRef,
}: UseAccountStoreDeps) {
  const [isInitializing, setIsInitializing] = useState<boolean>(isTauriRuntime);
  // 经 loader 取值：生产构建下 getMock* 恒返回空集合，不会出现假账号
  const [adapters, setAdapters] = useState<AdapterManifest[]>(() =>
    isTauriRuntime ? [] : getMockAdapters()
  );
  const [accounts, setAccounts] = useState<AccountSummary[]>(() =>
    isTauriRuntime ? [] : getMockAccounts()
  );
  const [clientStatuses, setClientStatuses] = useState<Record<string, ClientHostStatus>>({});
  const [cstDayKey, setCstDayKey] = useState<string>(() => toCstDayKey(Date.now()));
  const cstDayKeyRef = useRef(cstDayKey);
  cstDayKeyRef.current = cstDayKey;

  // 账号/适配器全量同步的请求序号：用于丢弃「先发出但后返回」的过期响应
  const accountSyncSeqRef = useRef(0);
  const didMountLogsRef = useRef(false);
  const syncDebounceTimerRef = useRef<number | null>(null);
  const accountSyncTimerRef = useRef<number | null>(null);
  const pendingAccountIdsRef = useRef<Set<string>>(new Set());
  const accountsRef = useRef(accounts);
  accountsRef.current = accounts;
  const adaptersRef = useRef(adapters);
  adaptersRef.current = adapters;
  const clientStatusesRef = useRef(clientStatuses);
  clientStatusesRef.current = clientStatuses;
  const batchRunningRef = useRef(Boolean(batchProgress?.running));
  batchRunningRef.current = Boolean(batchProgress?.running);
  const batchLaunchLockRef = useRef(false);
  const coldSwitchDetectSeqRef = useRef(0);
  const coldSwitchSubmittingRef = useRef(false);
  const coldSwitchDoneTimerRef = useRef<number | null>(null);
  const startupAutoRunDoneRef = useRef(false);
  const updateCheckDoneRef = useRef(false);
  const settingsLoadedRef = useRef(false);
  const logsDebounceTimerRef = useRef<number | null>(null);


  useEffect(() => {
    return () => {
      if (syncDebounceTimerRef.current !== null) {
        window.clearTimeout(syncDebounceTimerRef.current);
      }
      if (coldSwitchDoneTimerRef.current !== null) {
        window.clearTimeout(coldSwitchDoneTimerRef.current);
      }
    };
  }, []);



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
                : tid === "minimax_code_cn"
                  ? "/Applications/MiniMax.app"
                  : tid === "lobsterai"
                    ? "/Applications/LobsterAI.app"
                    : tid === "doubaowork"
                      ? "/Applications/DoubaoWork.app"
                      : "/Applications/Qoder.app",
          configDir:
            tid === "minimax_code_cn"
              ? "~/.minimax"
              : tid === "lobsterai"
                ? "~/Library/Application Support/LobsterAI"
                : tid === "doubaowork"
                  ? "~/Library/Application Support/DoubaoWork"
                  : `~/Library/Application Support/${tid}`,
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

  // 校正“本机使用中”标记：读取桌面端真实登录账号后对齐（启动时执行一次）
  const reconcileActiveAccounts = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      await invoke("reconcile_active_accounts");
    } catch {
      // 尽力而为，失败不影响主流程
    }
  }, [isTauriRuntime]);

  const syncFromBackend = useCallback(async () => {
    if (!isTauriRuntime) return;
    const { seq, scope, limit } = requestSnapshot();
    accountSyncSeqRef.current += 1;
    const accountSeq = accountSyncSeqRef.current;
    try {
      const [adList, accList, logList, stats, settings] = await Promise.all([
        invoke<AdapterManifest[]>("list_adapters"),
        invoke<AccountSummary[]>("list_accounts", { toolId: null }),
        invoke<ActionLogItem[]>("get_action_logs", {
          scope,
          limit,
          offset: 0,
          beforeId: null,
        }),
        invoke<ActionLogStats>("get_action_log_stats"),
        invoke<AppSettings>("get_app_settings"),
      ]);
      // 账号/适配器同样需要序号守卫：并发调用（如事件去抖与手动刷新重叠）时，
      // 先发出但后返回的旧响应会覆盖新状态，表现为「已删除的账号又回来」。
      if (accountSeq !== accountSyncSeqRef.current) {
        return;
      }
      setAdapters(adList);
      setAccounts(accList);
      commitSnapshot(logList, stats, seq, scope);
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
  }, [applyIncomingSettings, commitSnapshot, isTauriRuntime, requestSnapshot, showToast]);

  // 定向增量刷新若干账号（`qwitch://account-updated` 后只取被改动账号），
  // 任一 id 取不到（如删除/工具级事件传的是 tool_id）则回退全量同步以保持一致。
  const refreshAccountSummaries = useCallback(
    async (ids: string[]) => {
      if (!isTauriRuntime || ids.length === 0) return;
      const seq = accountSyncSeqRef.current;
      try {
        const fetched = await Promise.all(
          ids.map((id) =>
            invoke<AccountSummary>("get_account_summary", { accountId: id }).catch(() => null)
          )
        );
        if (seq !== accountSyncSeqRef.current) {
          return;
        }
        if (fetched.some((a) => a === null)) {
          await syncFromBackend();
          return;
        }
        const list = fetched as AccountSummary[];
        accountSyncSeqRef.current += 1;
        setAccounts((prev) => {
          const byId = new Map(list.map((a) => [a.id, a] as const));
          // 若本次更新含某个工具新的本机使用中账号，清除本地同工具下其他账号的旧标记，
          // 防止切号/本机导入/状态校正后跨窗口同时出现多个「使用中」账号。
          const activeByTool = new Map<string, string>();
          for (const item of list) {
            if (item.isActiveOnClient) {
              activeByTool.set(item.toolId, item.id);
            }
          }
          const prevIds = new Set(prev.map((p) => p.id));
          const updated = prev.map((p) => {
            const incoming = byId.get(p.id);
            if (incoming) return incoming;
            const activeId = activeByTool.get(p.toolId);
            if (activeId && activeId !== p.id && p.isActiveOnClient) {
              return { ...p, isActiveOnClient: false };
            }
            return p;
          });
          const added = list.filter((a) => !prevIds.has(a.id));
          return [...added, ...updated];
        });
      } catch (err) {
        showToast(`定向刷新账号失败：${String(err)}`, "error");
        await syncFromBackend();
      }
    },
    [isTauriRuntime, showToast, syncFromBackend]
  );

  useEffect(() => {
    if (!isTauriRuntime) return;
    if (!didMountLogsRef.current) {
      didMountLogsRef.current = true;
      return;
    }
    resetPagination();
    void refreshLogs(logScope);
  }, [isTauriRuntime, logScope, refreshLogs, resetPagination]);

  useEffect(() => {
    if (!isTauriRuntime) return;
    void (async () => {
      if (!isTrayWindow) {
        await reconcileActiveAccounts();
      }
      await syncFromBackend();
    })();

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

    register<string>("qwitch://account-updated", (payload) => {
      if (payload) pendingAccountIdsRef.current.add(payload);
      if (accountSyncTimerRef.current !== null) {
        window.clearTimeout(accountSyncTimerRef.current);
      }
      accountSyncTimerRef.current = window.setTimeout(() => {
        accountSyncTimerRef.current = null;
        const ids = Array.from(pendingAccountIdsRef.current);
        pendingAccountIdsRef.current.clear();
        void refreshAccountSummaries(ids);
      }, 100);
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
      if (accountSyncTimerRef.current !== null) {
        window.clearTimeout(accountSyncTimerRef.current);
        accountSyncTimerRef.current = null;
      }
      // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
      pendingAccountIdsRef.current.clear();
      if (logsDebounceTimerRef.current !== null) {
        window.clearTimeout(logsDebounceTimerRef.current);
        logsDebounceTimerRef.current = null;
      }
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
  }, [applyIncomingSettings, isTrayWindow, isTauriRuntime, reconcileActiveAccounts, refreshAccountSummaries, refreshLogs, showToast, syncFromBackend]);

  // 回到 Qwitch 窗口时自动复核“本机当前账号”（限频，避免频繁重读客户端文件）
  useEffect(() => {
    if (!isTauriRuntime || isTrayWindow) return;
    let last = 0;
    const onFocus = () => {
      const now = Date.now();
      if (now - last < 10_000) return;
      last = now;
      void reconcileActiveAccounts().then(() => syncFromBackend());
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [isTauriRuntime, isTrayWindow, reconcileActiveAccounts, syncFromBackend]);

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
      if (batchProgress?.running || batchLaunchLockRef.current) {
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
        batchLaunchLockRef.current = true;
        setBatchProgress({
          action: "refresh",
          running: true,
          total: candidates.length,
          completed: 0,
          currentLabel: candidates[0].label,
          phase: "checking",
        });
        try {
          await invoke("refresh_quota_batch", {
            accountIds: candidates.map((c) => c.id),
          });
          return true;
        } catch (err) {
          setBatchProgress(null);
          showToast(`启动批量查额失败：${String(err)}`, "error");
          return false;
        } finally {
          batchLaunchLockRef.current = false;
        }
      }

      batchLaunchLockRef.current = true;
      try {
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
      } finally {
        batchLaunchLockRef.current = false;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
    [accounts, adapters, appendLog, batchProgress?.running, isTauriRuntime, isTrayWindow, showToast]
  );

  const claimDailyBatch = useCallback(
    async (targetIds?: string[], toolId?: ToolId, includeUnknown?: boolean) => {
      if (batchProgress?.running || batchLaunchLockRef.current) {
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
        const missingSnapshotCount = accounts.filter((acc) => {
          if (targetIds && !targetIds.includes(acc.id)) return false;
          if (!targetIds && toolId && acc.toolId !== toolId) return false;
          const adapter = adapters.find((ad) => ad.id === acc.toolId);
          return (
            acc.status === "active" &&
            acc.hasApiToken !== false &&
            Boolean(adapter?.capabilities.supportsDailyClaim) &&
            isAccountMissingLocalSnapshot(acc, adapters)
          );
        }).length;
        if (alreadyClaimedCount > 0 && missingTokenCount === 0 && unknownCount === 0 && missingSnapshotCount === 0) {
          showToast(`今日可签到账号已全部领取完成 (${alreadyClaimedCount} 个)`, "success");
          return;
        }
        showToast(
          missingTokenCount > 0
            ? "账号尚未完成在线授权，请先点击卡片「在线授权」后再领取"
            : missingSnapshotCount > 0
              ? `有 ${missingSnapshotCount} 个账号缺本机快照，无法冷切领取，请在对应卡片「补快照」/ 重新导入本机后重试`
              : unknownCount > 0
                ? `存在 ${unknownCount} 个待核实账号，请点击「核实并领取」先查状态再领取`
                : "当前范围下没有可参与领取的正常账号",
          missingTokenCount > 0 || missingSnapshotCount > 0 ? "warning" : "info"
        );
        return;
      }

      if (isTauriRuntime) {
        batchLaunchLockRef.current = true;
        setBatchProgress({
          action: "claim",
          running: true,
          total: candidates.length,
          completed: 0,
          currentLabel: candidates[0].label,
          phase: "checking",
        });
        try {
          await invoke("claim_daily_batch", {
            accountIds: candidates.map((c) => c.id),
          });
        } catch (err) {
          setBatchProgress(null);
          showToast(`启动领取队列失败：${String(err)}`, "error");
        } finally {
          batchLaunchLockRef.current = false;
        }
        return;
      }

      let claimedCount = 0;
      let skippedCount = 0;
      batchLaunchLockRef.current = true;
      try {

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
      } finally {
        batchLaunchLockRef.current = false;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
    [accounts, adapters, appendLog, batchProgress?.running, isTauriRuntime, jitterRangeMs, showToast]
  );

  const requestColdSwitch = useCallback(
    (account: AccountSummary) => {
      if (coldSwitchDoneTimerRef.current !== null) {
        window.clearTimeout(coldSwitchDoneTimerRef.current);
        coldSwitchDoneTimerRef.current = null;
      }
      coldSwitchSubmittingRef.current = false;
      const seq = ++coldSwitchDetectSeqRef.current;
      const initialRunning = clientStatusesRef.current[account.toolId]?.isRunning ?? true;
      setSwitchDialog({ account, phase: "confirm", isClientRunning: initialRunning });
      if (isTauriRuntime) {
        void invoke<ClientHostStatus>("detect_client_status", { toolId: account.toolId })
          .then((status) => {
            if (coldSwitchDetectSeqRef.current !== seq) return;
            setSwitchDialog((prev) =>
              prev.account && prev.account.id === account.id && prev.phase === "confirm"
                ? { ...prev, isClientRunning: status.isRunning }
                : prev
            );
          })
          .catch(() => {});
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
    [isTauriRuntime]
  );

  const cancelColdSwitch = useCallback(() => {
    if (coldSwitchSubmittingRef.current) return;
    coldSwitchDetectSeqRef.current += 1;
    if (coldSwitchDoneTimerRef.current !== null) {
      window.clearTimeout(coldSwitchDoneTimerRef.current);
      coldSwitchDoneTimerRef.current = null;
    }
    setSwitchDialog({ account: null, phase: "confirm" });
  // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
  }, []);

  const confirmColdSwitch = useCallback(async () => {
    const target = switchDialog.account;
    if (!target || coldSwitchSubmittingRef.current) return;
    coldSwitchSubmittingRef.current = true;
    coldSwitchDetectSeqRef.current += 1;

    if (isTauriRuntime) {
      setSwitchDialog({
        account: target,
        phase: switchDialog.isClientRunning === false ? "writing" : "closing",
        isClientRunning: switchDialog.isClientRunning,
      });
      try {
        const result = await invoke<SwitchDesktopAccountResult>("switch_desktop_account", {
          accountId: target.id,
          forceKill: true,
        });
        await syncFromBackend();
        void refreshClientStatuses([target.toolId]);
        coldSwitchSubmittingRef.current = false;
        setSwitchDialog((prev) => ({ ...prev, account: target, phase: "done" }));
        // 快照写入成功 ≠ 客户端已确认存活：两者分开播报，不一律报成功
        if (result && result.livenessConfirmed === false) {
          showToast(
            `已切换至 ${target.label}，但未确认客户端存活${result.note ? `：${result.note}` : ""}`,
            "warning"
          );
        } else {
          showToast(`本机客户端已切换至 ${target.label}`, "success");
        }
        if (coldSwitchDoneTimerRef.current !== null) {
          window.clearTimeout(coldSwitchDoneTimerRef.current);
        }
        coldSwitchDoneTimerRef.current = window.setTimeout(() => {
          coldSwitchDoneTimerRef.current = null;
          setSwitchDialog({ account: null, phase: "confirm" });
        }, 650);
      } catch (err) {
        coldSwitchSubmittingRef.current = false;
        void refreshClientStatuses([target.toolId]);
        setSwitchDialog({ account: null, phase: "confirm" });
        showToast(`切换失败：${String(err)}`, "error");
      }
      return;
    }

    try {
      setSwitchDialog((prev) => ({ ...prev, account: target, phase: "closing" }));
      await sleep(600);
      setSwitchDialog((prev) => ({ ...prev, account: target, phase: "writing" }));
      await sleep(550);
      setSwitchDialog((prev) => ({ ...prev, account: target, phase: "launching" }));
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
      setSwitchDialog((prev) => ({ ...prev, account: target, phase: "done" }));
      if (coldSwitchDoneTimerRef.current !== null) {
        window.clearTimeout(coldSwitchDoneTimerRef.current);
      }
      coldSwitchDoneTimerRef.current = window.setTimeout(() => {
        coldSwitchDoneTimerRef.current = null;
        coldSwitchSubmittingRef.current = false;
        setSwitchDialog({ account: null, phase: "confirm" });
        showToast(`本机客户端已切换至 ${target.label}`, "success");
      }, 650);
    } catch (err) {
      coldSwitchSubmittingRef.current = false;
      setSwitchDialog({ account: null, phase: "confirm" });
      showToast(`切换失败：${String(err)}`, "error");
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
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
      targetAccountId?: string,
      profileDir?: string,
      devicePlatform?: string
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
              profileDir: profileDir ?? null,
              devicePlatform: devicePlatform ?? null,
            });
            await syncFromBackend();
            const devSuffix = imported.deviceIdMasked
              ? `（设备 ${imported.deviceIdMasked}${imported.deviceType ? ` · ${imported.deviceType}` : ""}）`
              : "";
            if (imported.hasApiToken === false) {
              showToast(
                `已导入 ${imported.label}${devSuffix} 切号快照，请点击卡片「在线授权」开启查额与签到`,
                "warning"
              );
            } else if (targetAccountId) {
              showToast(
                `已关联本机快照：${imported.label}${devSuffix}，现在可一键切号`,
                "success"
              );
            } else {
              showToast(`已导入账号：${imported.label}${devSuffix}`, "success");
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
          showToast(
            `${targetAccountId ? "关联失败" : "导入失败"}：${String(err)}`,
            "error"
          );
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
        toolId === "trae_cn" ||
        toolId === "workbuddy" ||
        toolId === "qoder_cn" ||
        toolId === "minimax_code_cn" ||
        toolId === "lobsterai" ||
        toolId === "doubaowork";
      const isDualTrack = toolId === "qoder_cn" || toolId === "doubaowork";
      const newAcc: AccountSummary = {
        id,
        toolId,
        label: displayLabel,
        uidMasked: `${toolId.slice(0, 2)}_${randHex}****9f`,
        status: "active",
        isActiveOnClient: false,
        hasLocalSnapshot: method === "local_import" || !isDualTrack,
        hasApiToken: method !== "local_import" || !isDualTrack,
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

  const launchToolProfile = useCallback(
    async (toolId: ToolId, profileDir: string): Promise<string> => {
      if (!isTauriRuntime) {
        throw new Error("浏览器预览模式无法启动隔离客户端");
      }
      return invoke<string>("launch_tool_profile", { toolId, profileDir });
    },
    [isTauriRuntime]
  );

  const pickFolder = useCallback(
    async (defaultPath?: string): Promise<string | null> => {
      if (!isTauriRuntime) return null;
      return invoke<string | null>("pick_folder", { defaultPath: defaultPath ?? null });
    },
    [isTauriRuntime]
  );

  const pickFile = useCallback(
    async (defaultPath?: string): Promise<string | null> => {
      if (!isTauriRuntime) return null;
      return invoke<string | null>("pick_file", { defaultPath: defaultPath ?? null });
    },
    [isTauriRuntime]
  );

  const inspectProfileDir = useCallback(
    async (toolId: ToolId, profileDir: string): Promise<ProfileInspectResult> => {
      if (!isTauriRuntime) {
        return { hasStorage: false, storagePath: "" };
      }
      return invoke<ProfileInspectResult>("inspect_profile_dir", { toolId, profileDir });
    },
    [isTauriRuntime]
  );

  const previewLocalAccount = useCallback(
    async (toolId: ToolId, accountId?: string) => {
      if (!isTauriRuntime) {
        throw new Error("浏览器预览模式无法读取本机会话");
      }
      return invoke<LocalAccountPreview>("preview_local_account", {
        toolId,
        accountId: accountId ?? null,
        profileDir: null,
      });
    },
    [isTauriRuntime]
  );

  const renameAccount = useCallback(
    async (accountId: string, label: string) => {
      const trimmed = label.trim();

      if (isTauriRuntime) {
        try {
          await invoke("rename_account", { accountId, label: trimmed });
          await syncFromBackend();
          showToast(trimmed ? `已设置别名为 ${trimmed}` : "已恢复原用户名", "success");
        } catch (err) {
          showToast(`重命名失败：${String(err)}`, "error");
        }
        return;
      }

      setAccounts((prev) =>
        prev.map((acc) => {
          if (acc.id !== accountId) return acc;
          const nextLabel = trimmed || acc.accountName || acc.label;
          return { ...acc, label: nextLabel, updatedAt: new Date().toISOString() };
        })
      );
      showToast(trimmed ? `已设置别名为 ${trimmed}` : "已恢复原用户名", "success");
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

  /**
   * 采集本机当前登录账号的会话快照，绑定到指定账号。
   *
   * 用于 Qoder：其 auth.v1.dat 为 Chromium Safe Storage 加密，无法由在线授权的
   * Token 生成可写入快照。用户需先在客户端里登录目标账号，再由本方法采集原始字节
   * ——apply_session 只做原样写回，因此无需解密即可实现切号。
   *
   * 后端会校验本机登录 uid 与目标账号一致，不一致直接拒绝。
   */
  /// 启动指定工具的官方客户端（不注入多开参数）。
  ///
  /// 用途：CodeArts 这类由客户端自行托管并轮换凭据的工具，凭据过期时
  /// 由用户一键拉起客户端完成刷新，Qwitch 不代刷（refresh token 一次性轮换，
  /// 双端刷新会互相作废）。
  const launchToolClient = useCallback(
    async (toolId: string) => {
      if (!isTauriRuntime) {
        showToast("浏览器预览模式无法启动本地客户端", "info");
        return;
      }
      const toolName = adapters.find((a) => a.id === toolId)?.name ?? toolId;
      try {
        await invoke<string>("launch_tool_client", { toolId });
        showToast(`已启动 ${toolName} 客户端`, "success");
        window.setTimeout(() => void refreshClientStatuses(), 1200);
      } catch (err) {
        showToast(`启动 ${toolName} 失败：${String(err)}`, "error");
      }
    },
    [adapters, isTauriRuntime, refreshClientStatuses, showToast]
  );

  const captureLocalSession = useCallback(
    async (accountId: string): Promise<boolean> => {
      const target = accounts.find((a) => a.id === accountId);
      if (!target) return false;

      if (!isTauriRuntime) {
        showToast("浏览器预览模式无法采集本机会话", "info");
        return false;
      }

      try {
        await invoke("capture_local_session", { accountId });
        await syncFromBackend();
        showToast(`${target.label} 已采集本机会话，现在可以切换该账号`, "success");
        return true;
      } catch (err) {
        showToast(`采集失败：${String(err)}`, "error");
        return false;
      }
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
          void refreshClientStatuses([target.toolId]);
          showToast(`已删除账号：${target.label}`, "success");
        } catch (err) {
          showToast(`删除失败：${String(err)}`, "error");
        }
        return;
      }

      setAccounts((prev) => prev.filter((acc) => acc.id !== accountId));
      void refreshClientStatuses([target.toolId]);
      appendLog({
        accountId,
        toolId: target.toolId,
        actionType: "import",
        resultStatus: "success",
        messageSanitized: `已删除账号 ${target.label} (${target.uidMasked})`,
      });
      showToast(`已删除账号：${target.label}`, "success");
    },
    [accounts, appendLog, isTauriRuntime, showToast, syncFromBackend, refreshClientStatuses]
  );


  const quitApp = useCallback(async () => {
    if (isTauriRuntime) {
      await invoke("quit_app");
      return;
    }
    showToast("浏览器预览模式下无需退出应用", "info");
  }, [isTauriRuntime, showToast]);

  const triggerAutoClaimAvailable = useCallback(async () => {
    if (isTrayWindow || batchRunningRef.current || batchLaunchLockRef.current) return;
    const pickAvailableIds = (list: AccountSummary[]) =>
      list
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

    if (isTauriRuntime) {
      batchLaunchLockRef.current = true;
      try {
        const latestAccounts = await invoke<AccountSummary[]>("list_accounts", { toolId: null });
        const availableIds = pickAvailableIds(latestAccounts);
        if (availableIds.length === 0) return;
        await invoke("claim_daily_batch", { accountIds: availableIds });
      } catch {
        // background automation errors should not crash UI
      } finally {
        batchLaunchLockRef.current = false;
      }
      return;
    }

    try {
      const availableIds = pickAvailableIds(accountsRef.current);
      if (availableIds.length === 0) return;
      await claimDailyBatch(availableIds);
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
  // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
  }, [isInitializing, isTrayWindow]);

  // 启动后静默检查一次 GitHub Releases；失败不打扰用户。
  //
  // **托盘窗口同样需要检查**：主窗口与托盘是两个独立 webview，各自持有自己的
  // `updateInfo` 状态，不会跨窗口共享。此前这里用 `isTrayWindow` 直接跳过，
  // 导致托盘的 `updateInfo` 恒为 null —— 更新条带永远不显示，用户只有在主窗口
  // 才能看到更新提示。
  //
  // 差异仅在提示方式：主窗口用 Toast 告知；托盘不加 Toast（避免高频弹出打扰），
  // 只让顶部条带自然出现。托盘窗口更早触发（1.5s vs 4s），因为它是高频入口。
  useEffect(() => {
    if (!isTauriRuntime || isInitializing || updateCheckDoneRef.current) return;
    const timer = window.setTimeout(() => {
      if (updateCheckDoneRef.current) return;
      updateCheckDoneRef.current = true;
      void checkAppUpdate({ silent: true }).then((info) => {
        if (info?.updateAvailable && !isTrayWindow) {
          showToast(
            `发现新版本 v${info.latestVersion}，可在「设置 → 软件更新」查看`,
            "info"
          );
        }
      });
    }, isTrayWindow ? 1500 : 4000);
    return () => window.clearTimeout(timer);
  }, [checkAppUpdate, isInitializing, isTauriRuntime, isTrayWindow, showToast]);

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
  // eslint-disable-next-line react-hooks/exhaustive-deps -- 省略的均为 useRef/useState setter，引用恒定（见文件头说明）
  }, [appSettings.autoRefreshIntervalMinutes, isInitializing, isTrayWindow]);


  return {
    isInitializing,
    adapters,
    accounts,
    clientStatuses,
    cstDayKey,
    refreshClientStatuses,
    reconcileActiveAccounts,
    refreshAccountQuota,
    refreshAllQuotas,
    claimDailyBatch,
    requestColdSwitch,
    cancelColdSwitch,
    confirmColdSwitch,
    importAccount,
    launchToolProfile,
    pickFolder,
    pickFile,
    inspectProfileDir,
    previewLocalAccount,
    renameAccount,
    toggleAccountDisabled,
    recoverAccount,
    launchToolClient,
    captureLocalSession,
    deleteAccount,
    quitApp,
  };
}
