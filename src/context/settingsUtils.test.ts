import { describe, expect, it } from "vitest";
import {
  DEFAULT_APP_SETTINGS,
  buildSettingsSavePayload,
  normalizeAppSettings,
  sanitizeAppSettingsPatch,
  sanitizeNonNegativeInt,
} from "./settingsUtils";

describe("settingsUtils", () => {
  it("sanitizes non-negative integers and rejects NaN/Infinity/negative floats", () => {
    expect(sanitizeNonNegativeInt(undefined, 42)).toBe(42);
    expect(sanitizeNonNegativeInt(Number.NaN, 15)).toBe(15);
    expect(sanitizeNonNegativeInt(Number.POSITIVE_INFINITY, 15)).toBe(15);
    expect(sanitizeNonNegativeInt(-12.8, 15)).toBe(0);
    expect(sanitizeNonNegativeInt(12.9, 15)).toBe(12);
    expect(sanitizeNonNegativeInt(0, 15)).toBe(0);
  });

  it("normalizes missing or invalid settings against fallback defaults", () => {
    const normalized = normalizeAppSettings(
      {
        jitterMinMs: -50,
        jitterMaxMs: 4200.7,
        logArchiveAfterDays: Number.NaN,
        logMaxActiveCount: -5,
        autoRefreshIntervalMinutes: 30.4,
        theme: "unknown" as unknown as "system",
      },
      DEFAULT_APP_SETTINGS
    );

    expect(normalized.jitterMinMs).toBe(0);
    expect(normalized.jitterMaxMs).toBe(4200);
    expect(normalized.logArchiveAfterDays).toBe(DEFAULT_APP_SETTINGS.logArchiveAfterDays);
    expect(normalized.logMaxActiveCount).toBe(0);
    expect(normalized.autoRefreshIntervalMinutes).toBe(30);
    expect(normalized.theme).toBe("system");
    expect(normalized.autoRefreshOnStartup).toBe(false);
  });

  it("sanitizes numeric fields in partial patches", () => {
    const patch = sanitizeAppSettingsPatch({
      jitterMinMs: Number.NaN,
      jitterMaxMs: -1,
      logArchiveAfterDays: 7.9,
      logMaxActiveCount: Number.NaN,
      autoRefreshIntervalMinutes: -10,
      theme: "dark",
    });

    expect(patch).toEqual({
      jitterMinMs: 500,
      jitterMaxMs: 0,
      logArchiveAfterDays: 7,
      logMaxActiveCount: 0,
      autoRefreshIntervalMinutes: 0,
      theme: "dark",
    });
  });

  it("isolates persisted settings from unrelated unsaved form drafts", () => {
    const persisted = normalizeAppSettings({
      ...DEFAULT_APP_SETTINGS,
      jitterMinMs: 1500,
      jitterMaxMs: 3500,
      logArchiveAfterDays: 14,
      logMaxActiveCount: 200,
      autoRefreshOnStartup: false,
      autoRefreshIntervalMinutes: 30,
      closeToTray: true,
    });

    const payload = buildSettingsSavePayload({ theme: "dark" }, persisted);

    expect(payload.theme).toBe("dark");
    expect(payload.jitterMinMs).toBe(1500);
    expect(payload.jitterMaxMs).toBe(3500);
    expect(payload.logArchiveAfterDays).toBe(14);
    expect(payload.logMaxActiveCount).toBe(200);
    expect(payload.autoRefreshIntervalMinutes).toBe(30);
    expect(payload.closeToTray).toBe(true);
  });

  it("ignores legacy hideDockIcon field when normalizing persisted settings", () => {
    const legacyRaw = {
      ...DEFAULT_APP_SETTINGS,
      hideDockIcon: true,
    } as Partial<typeof DEFAULT_APP_SETTINGS>;
    const normalized = normalizeAppSettings(legacyRaw);

    expect(normalized.closeToTray).toBe(true);
    expect("hideDockIcon" in normalized).toBe(false);
  });
});
