export type BuiltinToolId =
  | "trae"
  | "trae_cn"
  | "workbuddy"
  | "qoder"
  | "qoder_cn"
  | "minimax_code_cn"
  | "lobsterai"
  | "doubaowork"
  | "codearts";
export type ToolId = BuiltinToolId | (string & {});

export type AuthMethod = "webview_oauth" | "local_import" | "token_paste";

export type AccountStatus = "active" | "expired" | "risk_paused" | "disabled";

export type ClaimStatus = "unknown" | "available" | "claimed" | "unsupported";

export type UnifiedErrorCode =
  | "AUTH_EXPIRED"
  | "RISK_TRIGGERED"
  | "RATE_LIMITED"
  | "DEVICE_DAILY_LIMIT_REACHED"
  | "NETWORK_ERROR"
  | "CLIENT_RUNNING"
  | "CLIENT_NOT_FOUND"
  | "UNSUPPORTED_CAPABILITY"
  | "INVALID_CREDENTIAL"
  | "INTERNAL_ERROR";

export interface AdapterCapabilities {
  authMethods: AuthMethod[];
  supportsQuotaQuery: boolean;
  supportsDailyClaim: boolean;
  supportsClientSwitch: boolean;
  supportsMultiInstance: boolean;
}

export interface AdapterManifest {
  id: ToolId;
  name: string;
  version: string;
  isBuiltin: boolean;
  capabilities: AdapterCapabilities;
}

export interface QuotaBucket {
  key: string;
  label: string;
  used: number;
  total: number;
  remaining: number;
  unit: string;
  resetAt?: string;
}

export interface QuotaSnapshot {
  planName?: string;
  claimStatus: ClaimStatus;
  buckets: QuotaBucket[];
  fetchedAt: string;
}

export interface AccountSummary {
  id: string;
  toolId: ToolId;
  label: string;
  uidMasked: string;
  emailMasked?: string;
  status: AccountStatus;
  isActiveOnClient: boolean;
  hasLocalSnapshot?: boolean;
  hasApiToken?: boolean;
  deviceIdMasked?: string;
  deviceType?: string;
  latestQuota?: QuotaSnapshot;
  lastClaimedAt?: string;
  updatedAt: string;
}

export interface ClientHostStatus {
  toolId: ToolId;
  executablePath?: string;
  configDir?: string;
  isRunning: boolean;
}

export interface LocalAccountPreview {
  toolId: ToolId;
  uidMasked: string;
  displayName?: string;
  isFallbackUid: boolean;
  matchesTarget: boolean;
  matchedAccountId?: string;
  matchedAccountLabel?: string;
}

export interface AppUpdateInfo {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  releaseUrl: string;
  releaseNotes: string;
  publishedAt?: string;
  assetName?: string;
  assetUrl?: string;
  assetSize?: number;
  checksumsUrl?: string;
  strategy: "windows_installer" | "macos_dmg" | "manual";
  platformLabel: string;
  canAutoInstall: boolean;
  hint: string;
}

export interface AppSettings {
  jitterMinMs: number;
  jitterMaxMs: number;
  theme: "light" | "dark" | "system";
  customClientPaths: Record<string, string>;
  logArchiveAfterDays?: number;
  logMaxActiveCount?: number;
  closeToTray?: boolean;
  launchAtLogin?: boolean;
  startMinimized?: boolean;
  autoRefreshOnStartup?: boolean;
  autoRefreshIntervalMinutes?: number;
  autoClaimDaily?: boolean;
  trustedAdapters?: string[];
  trustedAdapterHashes?: Record<string, string>;
  gatewayPort?: number;
}

export interface ProfileInspectResult {
  hasStorage: boolean;
  storagePath: string;
  deviceIdMasked?: string;
  sameAsLocal?: boolean;
}

export interface ExternalAdapterInfo {
  id: string;
  name: string;
  version: string;
  platforms: string[];
  trusted: boolean;
  reserved: boolean;
  contentHash: string;
  hashMismatch: boolean;
}

export type LogScope = "active" | "archived" | "all";

export interface ActionLogItem {
  id: number;
  accountId?: string;
  toolId: ToolId;
  actionType: "import" | "query" | "claim" | "switch" | "keepalive";
  resultStatus: "success" | "skipped" | "failed";
  errorCode?: UnifiedErrorCode;
  messageSanitized: string;
  createdAt: string;
  archivedAt?: string;
}

export interface ActionLogStats {
  activeCount: number;
  archivedCount: number;
  failedActiveCount: number;
  noiseActiveCount: number;
}

export interface ActionLogCleanupRequest {
  scope?: LogScope;
  ids?: number[];
  olderThanDays?: number;
  actionType?: string;
  resultStatus?: string;
  redundantOnly?: boolean;
}

export interface ExportLogsResult {
  filePath: string;
  count: number;
  archivedCount: number;
}

export type ToastTone = "success" | "error" | "warning" | "info";

export interface ToastState {
  id: number;
  message: string;
  tone: ToastTone;
}

export type BatchActionType = "claim" | "refresh";

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
