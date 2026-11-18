export type BuiltinToolId = "trae" | "trae_cn" | "workbuddy" | "qoder" | "qoder_cn";
export type ToolId = BuiltinToolId | (string & {});

export type AuthMethod = "webview_oauth" | "local_import" | "token_paste";

export type AccountStatus = "active" | "expired" | "risk_paused" | "disabled";

export type ClaimStatus = "unknown" | "available" | "claimed" | "unsupported";

export type UnifiedErrorCode =
  | "AUTH_EXPIRED"
  | "RISK_TRIGGERED"
  | "RATE_LIMITED"
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

export interface AppSettings {
  jitterMinMs: number;
  jitterMaxMs: number;
  theme: "light" | "dark";
  customClientPaths: Record<string, string>;
}

export interface ActionLogItem {
  id: number;
  accountId?: string;
  toolId: ToolId;
  actionType: "import" | "query" | "claim" | "switch";
  resultStatus: "success" | "skipped" | "failed";
  errorCode?: UnifiedErrorCode;
  messageSanitized: string;
  createdAt: string;
}
