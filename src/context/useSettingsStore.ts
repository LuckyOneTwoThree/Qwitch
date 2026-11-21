import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { resolveTheme } from "./logStateUtils";
import {
  buildSettingsSavePayload,
  normalizeAppSettings,
  sanitizeAppSettingsPatch,
} from "./settingsUtils";
import type { AppSettings } from "../types/qwitch";

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

interface UseSettingsStoreArgs {
  isTauriRuntime: boolean;
  showToast: (message: string, tone?: "success" | "error" | "warning" | "info") => void;
  /** 关闭自动领取时同步清除主流程的待领取标记（跨 store 副作用） */
  pendingAutoClaimRef: React.MutableRefObject<boolean>;
}

/**
 * Settings 域 store：应用设置、主题、抖动区间、日志保留、自定义客户端路径，
 * 以及保存串行链/脏标记保护逻辑。四 store 拆分之一。
 */
export function useSettingsStore({
  isTauriRuntime,
  showToast,
  pendingAutoClaimRef,
}: UseSettingsStoreArgs) {
  const [appSettings, setAppSettings] = useState<AppSettings>(() => normalizeAppSettings());
  const [theme, setTheme] = useState<"light" | "dark">(getInitialTheme);
  const [jitterRangeMs, setJitterRangeMsState] = useState<[number, number]>([1500, 3500]);
  const [logRetention, setLogRetentionState] = useState<{
    archiveAfterDays: number;
    maxActiveCount: number;
  }>({ archiveAfterDays: 14, maxActiveCount: 200 });
  const [customClientPaths, setCustomClientPaths] = useState<Record<string, string>>({});

  const appSettingsRef = useRef(appSettings);
  appSettingsRef.current = appSettings;
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

  const resetSettingsDrafts = useCallback(() => {
    jitterDirtyRef.current = false;
    logRetentionDirtyRef.current = false;
    const persisted = appSettingsRef.current;
    setJitterRangeMsState([persisted.jitterMinMs, persisted.jitterMaxMs]);
    setLogRetentionState({
      archiveAfterDays: persisted.logArchiveAfterDays ?? 14,
      maxActiveCount: persisted.logMaxActiveCount ?? 200,
    });
  }, []);

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
    [pendingAutoClaimRef]
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

          if (isTauriRuntime) {
            // 外部插件信任/重签必须经后端原生对话框确认：
            // 对「本次新增信任」或「内容哈希发生变化（重签）」的插件逐个索取一次性票据。
            // 弹窗确认放在前端乐观更新之前：用户一旦取消，前端状态完全未脏化，
            // 不会导致后续保存设置永久失败。
            const trustedAdapterTickets: Record<string, string> = {};
            const pluginsNeedingTicket = (payload.trustedAdapters ?? []).filter((id) => {
              const prevTrusted = (previous.trustedAdapters ?? []).includes(id);
              if (!prevTrusted) return true;
              const nextHash = payload.trustedAdapterHashes?.[id]?.trim();
              const prevHash = previous.trustedAdapterHashes?.[id]?.trim();
              if (!nextHash) return false;
              return !prevHash || prevHash.toLowerCase() !== nextHash.toLowerCase();
            });
            for (const id of pluginsNeedingTicket) {
              try {
                trustedAdapterTickets[id] = await invoke<string>(
                  "request_plugin_trust",
                  { adapterId: id }
                );
              } catch (err) {
                showToast(
                  `插件 ${id} 未获得信任确认：${String(err)}`,
                  "warning"
                );
                return false;
              }
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

            try {
              const saved = await invoke<AppSettings>("save_app_settings", {
                settings: payload,
                trustedAdapterTickets,
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
    [applyIncomingSettings, isTauriRuntime, pendingAutoClaimRef, showToast]
  );

  const toggleTheme = useCallback(() => {
    const nextTheme: "light" | "dark" = theme === "dark" ? "light" : "dark";
    void saveAppSettings({ theme: nextTheme }, true);
  }, [saveAppSettings, theme]);

  return {
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
  };
}
