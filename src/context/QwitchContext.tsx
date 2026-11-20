import React, { createContext, useContext, useMemo, useRef } from "react";
import { useLogStore } from "./useLogStore";
import { useSettingsStore } from "./useSettingsStore";
import { useFeedbackStore } from "./useFeedbackStore";
import { useAccountStore } from "./useAccountStore";
import type {
  AccountSummary,
  ActionLogCleanupRequest,
  ActionLogItem,
  ActionLogStats,
  AdapterManifest,
  AppSettings,
  AppUpdateInfo,
  AuthMethod,
  BatchProgressState,
  ClientHostStatus,
  LocalAccountPreview,
  LogScope,
  ProfileInspectResult,
  SwitchDialogState,
  ToastState,
  ToastTone,
  ToolId,
} from "../types/qwitch";

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
  reconcileActiveAccounts: () => Promise<void>;
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
    targetAccountId?: string,
    profileDir?: string,
    devicePlatform?: string
  ) => Promise<void>;
  launchToolProfile: (toolId: ToolId, profileDir: string) => Promise<string>;
  pickFolder: (defaultPath?: string) => Promise<string | null>;
  pickFile: (defaultPath?: string) => Promise<string | null>;
  inspectProfileDir: (toolId: ToolId, profileDir: string) => Promise<ProfileInspectResult>;
  previewLocalAccount: (toolId: ToolId, accountId?: string) => Promise<LocalAccountPreview>;
  renameAccount: (accountId: string, label: string) => Promise<void>;
  toggleAccountDisabled: (accountId: string) => Promise<void>;
  recoverAccount: (accountId: string) => Promise<void>;
  deleteAccount: (accountId: string) => Promise<void>;
  archiveLogs: (
    request: ActionLogCleanupRequest,
    feedbackLabel?: string | ((count: number) => string)
  ) => Promise<number | null>;
  unarchiveLogs: (
    request: ActionLogCleanupRequest,
    feedbackLabel?: string | ((count: number) => string)
  ) => Promise<number | null>;
  deleteLogs: (
    request: ActionLogCleanupRequest,
    feedbackLabel?: string | ((count: number) => string)
  ) => Promise<number | null>;
  exportLogs: (scope?: LogScope, archiveAfterExport?: boolean) => Promise<void>;
  dismissBatchProgress: () => void;
  quitApp: () => Promise<void>;
  cstDayKey: string;
  updateInfo: AppUpdateInfo | null;
  updateChecking: boolean;
  updateError: string | null;
  checkAppUpdate: (options?: {
    force?: boolean;
    silent?: boolean;
  }) => Promise<AppUpdateInfo | null>;
  resetSettingsDrafts: () => void;
}

const QwitchContext = createContext<QwitchContextValue | null>(null);

const detectTauriRuntime = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const detectTrayWindow = () =>
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("view") === "tray";

export function QwitchProvider({ children }: { children: React.ReactNode }) {
  const isTauriRuntime = detectTauriRuntime();
  const isTrayWindow = detectTrayWindow();
  // Feedback 域 store：toast/更新/批量进度/切号弹窗
  const feedbackStore = useFeedbackStore({ isTauriRuntime });
  const {
    toast,
    showToast,
    dismissToast,
    updateInfo,
    updateChecking,
    updateError,
    checkAppUpdate,
    batchProgress,
    setBatchProgress,
    switchDialog,
    setSwitchDialog,
    dismissBatchProgress,
  } = feedbackStore;

  // 主流程共享的待领取标记（Settings store 关闭自动领取时清除；Account store 消费）
  const pendingAutoClaimRef = useRef(false);

  // Log 域 store
  const logStore = useLogStore({ isTauriRuntime, showToast });
  const {
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
  } = logStore;

  // Settings 域 store
  const settingsStore = useSettingsStore({ isTauriRuntime, showToast, pendingAutoClaimRef });
  const {
    appSettings,
    appSettingsRef,
    theme,
    jitterRangeMs,
    logRetention,
    customClientPaths,
    setJitterRangeMs,
    setLogRetention,
    resetSettingsDrafts,
    applyIncomingSettings,
    saveAppSettings,
    toggleTheme,
  } = settingsStore;

  // Account 域 store（组合以上三个 store，承载账号池/查额/领取/切号/导入/同步编排）
  const accountStore = useAccountStore({
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
  });
  const {
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
    deleteAccount,
    quitApp,
  } = accountStore;
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
      deleteAccount,
      archiveLogs,
      unarchiveLogs,
      deleteLogs,
      exportLogs,
      dismissBatchProgress,
      quitApp,
      cstDayKey,
      updateInfo,
      updateChecking,
      updateError,
      checkAppUpdate,
      resetSettingsDrafts,
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
      deleteAccount,
      archiveLogs,
      unarchiveLogs,
      deleteLogs,
      exportLogs,
      dismissBatchProgress,
      quitApp,
      cstDayKey,
      updateInfo,
      updateChecking,
      updateError,
      checkAppUpdate,
      resetSettingsDrafts,
    ]
  );

  return <QwitchContext.Provider value={contextValue}>{children}</QwitchContext.Provider>;
}

export function useQwitch() {
  const ctx = useContext(QwitchContext);
  if (!ctx) throw new Error("useQwitch must be used within QwitchProvider");
  return ctx;
}
