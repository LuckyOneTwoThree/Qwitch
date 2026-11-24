import type { AppSettings } from "../types/qwitch";

export const DEFAULT_APP_SETTINGS: AppSettings = {
  jitterMinMs: 1500,
  jitterMaxMs: 3500,
  theme: "system",
  customClientPaths: {},
  logArchiveAfterDays: 14,
  logMaxActiveCount: 200,
  closeToTray: true,
  launchAtLogin: false,
  startMinimized: false,
  autoRefreshOnStartup: false,
  autoRefreshIntervalMinutes: 0,
  autoClaimDaily: false,
  trustedAdapters: [],
  trustedAdapterHashes: {},
  gatewayPort: 8187,
};

export function sanitizeNonNegativeInt(value: unknown, fallback = 0): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return fallback;
  }
  return Math.max(0, Math.floor(value));
}

export function normalizeAppSettings(
  raw?: Partial<AppSettings> | null,
  fallback: AppSettings = DEFAULT_APP_SETTINGS
): AppSettings {
  const rawPort = raw?.gatewayPort;
  const fallbackPort = fallback.gatewayPort ?? 8187;
  const gatewayPort =
    typeof rawPort === "number" && Number.isFinite(rawPort)
      ? Math.min(65535, Math.max(1024, Math.floor(rawPort)))
      : fallbackPort;
  return {
    jitterMinMs: sanitizeNonNegativeInt(raw?.jitterMinMs, fallback.jitterMinMs),
    jitterMaxMs: sanitizeNonNegativeInt(raw?.jitterMaxMs, fallback.jitterMaxMs),
    theme:
      raw?.theme === "dark"
        ? "dark"
        : raw?.theme === "light"
          ? "light"
          : raw?.theme === "system"
            ? "system"
            : fallback.theme,
    customClientPaths: raw?.customClientPaths ?? fallback.customClientPaths,
    logArchiveAfterDays: sanitizeNonNegativeInt(
      raw?.logArchiveAfterDays,
      fallback.logArchiveAfterDays ?? 14
    ),
    logMaxActiveCount: sanitizeNonNegativeInt(
      raw?.logMaxActiveCount,
      fallback.logMaxActiveCount ?? 200
    ),
    closeToTray: raw?.closeToTray ?? fallback.closeToTray ?? true,
    launchAtLogin: raw?.launchAtLogin ?? fallback.launchAtLogin ?? false,
    startMinimized: raw?.startMinimized ?? fallback.startMinimized ?? false,
    autoRefreshOnStartup:
      raw?.autoRefreshOnStartup ?? fallback.autoRefreshOnStartup ?? false,
    autoRefreshIntervalMinutes: sanitizeNonNegativeInt(
      raw?.autoRefreshIntervalMinutes,
      fallback.autoRefreshIntervalMinutes ?? 0
    ),
    autoClaimDaily: raw?.autoClaimDaily ?? fallback.autoClaimDaily ?? false,
    trustedAdapters: raw?.trustedAdapters ?? fallback.trustedAdapters ?? [],
    trustedAdapterHashes:
      raw?.trustedAdapterHashes ?? fallback.trustedAdapterHashes ?? {},
    gatewayPort,
  };
}

export function sanitizeAppSettingsPatch(next: Partial<AppSettings>): Partial<AppSettings> {
  const sanitized: Partial<AppSettings> = { ...next };
  if (next.jitterMinMs !== undefined) {
    sanitized.jitterMinMs = sanitizeNonNegativeInt(next.jitterMinMs, 500);
  }
  if (next.jitterMaxMs !== undefined) {
    sanitized.jitterMaxMs = sanitizeNonNegativeInt(next.jitterMaxMs, 500);
  }
  if (next.logArchiveAfterDays !== undefined) {
    sanitized.logArchiveAfterDays = sanitizeNonNegativeInt(next.logArchiveAfterDays, 0);
  }
  if (next.logMaxActiveCount !== undefined) {
    sanitized.logMaxActiveCount = sanitizeNonNegativeInt(next.logMaxActiveCount, 0);
  }
  if (next.autoRefreshIntervalMinutes !== undefined) {
    sanitized.autoRefreshIntervalMinutes = sanitizeNonNegativeInt(
      next.autoRefreshIntervalMinutes,
      0
    );
  }
  if (next.gatewayPort !== undefined) {
    sanitized.gatewayPort =
      typeof next.gatewayPort === "number" && Number.isFinite(next.gatewayPort)
        ? Math.min(65535, Math.max(1024, Math.floor(next.gatewayPort)))
        : 8187;
  }
  return sanitized;
}

/**
 * 仅将本次显式变更的字段合并到已落盘配置上，绝不夹带其他表单分区尚未保存的草稿
 */
export function buildSettingsSavePayload(
  next: Partial<AppSettings>,
  persisted: AppSettings
): AppSettings {
  return normalizeAppSettings(sanitizeAppSettingsPatch(next), persisted);
}
