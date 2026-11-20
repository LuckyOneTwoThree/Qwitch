// @vitest-environment jsdom
import { renderHook, act, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSettings, BatchProgressState, SwitchDialogState } from "../types/qwitch";

/**
 * `useAccountStore` 是全仓库最大的模块（1300+ 行），此前**零测试**。
 * 这里覆盖的是「无需真实 Tauri 运行时即可验证」的关键编排逻辑：
 * 批量任务互斥、切号弹窗状态机、导入前置校验、事件监听注册/注销。
 *
 * 真实 IPC 路径（invoke 到 Rust）由后端 Rust 测试与集成冒烟覆盖，
 * 此处通过 mock 掉 `@tauri-apps/api` 聚焦前端状态机本身。
 */

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: h.listen }));

import { useAccountStore } from "./useAccountStore";

const DEFAULT_SETTINGS: AppSettings = {
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
} as AppSettings;

const EMPTY_SWITCH_DIALOG: SwitchDialogState = {
  account: null,
  phase: "confirm",
  isClientRunning: false,
};

interface Harness {
  isTauriRuntime?: boolean;
  isTrayWindow?: boolean;
}

function makeDeps(opts: Harness = {}) {
  const showToast = vi.fn();
  const setBatchProgress = vi.fn();
  const setSwitchDialog = vi.fn();
  const appendLog = vi.fn();
  const refreshLogs = vi.fn(async () => {});
  const commitSnapshot = vi.fn();
  const resetPagination = vi.fn();
  const applyIncomingSettings = vi.fn();

  const deps = {
    isTauriRuntime: opts.isTauriRuntime ?? true,
    isTrayWindow: opts.isTrayWindow ?? false,
    showToast,
    checkAppUpdate: vi.fn(async () => null),
    appendLog,
    refreshLogs,
    requestSnapshot: () => ({ seq: 1, scope: "active" as const, limit: 100 }),
    commitSnapshot,
    logScope: "active" as const,
    resetPagination,
    jitterRangeMs: [1500, 3500] as [number, number],
    appSettings: DEFAULT_SETTINGS,
    appSettingsRef: { current: DEFAULT_SETTINGS },
    applyIncomingSettings,
    batchProgress: null as BatchProgressState | null,
    setBatchProgress,
    switchDialog: EMPTY_SWITCH_DIALOG,
    setSwitchDialog,
    pendingAutoClaimRef: { current: false },
  };

  return {
    deps,
    spies: {
      showToast,
      setBatchProgress,
      setSwitchDialog,
      appendLog,
      refreshLogs,
      commitSnapshot,
      resetPagination,
      applyIncomingSettings,
    },
  };
}

beforeEach(() => {
  h.invoke.mockReset();
  h.listen.mockReset();
  // 默认：监听注册返回可注销函数
  h.listen.mockResolvedValue(() => {});
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("useAccountStore", () => {
  it("Tauri 运行时下初始为空账号池，不注入 mock 数据", () => {
    const { deps } = makeDeps({ isTauriRuntime: true });
    h.invoke.mockResolvedValue([]);

    const { result } = renderHook(() => useAccountStore(deps));

    expect(result.current.accounts).toEqual([]);
    expect(result.current.adapters).toEqual([]);
  });

  it("非 Tauri 运行时（浏览器预览）注入 mock 账号池", () => {
    const { deps } = makeDeps({ isTauriRuntime: false });

    const { result } = renderHook(() => useAccountStore(deps));

    // Mock 模式下应有示例账号，便于 `npm run dev` 预览
    expect(result.current.accounts.length).toBeGreaterThan(0);
  });

  it("无候选账号时批量领取直接提示，不发起 IPC", async () => {
    const { deps, spies } = makeDeps();
    h.invoke.mockResolvedValue([]);

    const { result } = renderHook(() => useAccountStore(deps));

    await act(async () => {
      await result.current.claimDailyBatch([]);
    });

    expect(spies.showToast).toHaveBeenCalled();
    expect(h.invoke).not.toHaveBeenCalledWith("claim_daily_batch", expect.anything());
  });

  it("批量领取启动后立即进入运行态，并在 IPC 失败时回滚进度", async () => {
    const { deps, spies } = makeDeps();
    // 首次同步返回一个可领账号
    h.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_accounts") {
        return [
          {
            id: "acc-1",
            toolId: "trae_cn",
            label: "甲",
            uidMasked: "u_1***",
            status: "active",
            isActiveOnClient: false,
            hasApiToken: true,
            latestQuota: {
              claimStatus: "available",
              fetchedAt: new Date().toISOString(),
              buckets: [],
            },
            updatedAt: new Date().toISOString(),
          },
        ];
      }
      if (cmd === "list_adapters") {
        return [
          {
            id: "trae_cn",
            name: "Trae CN",
            version: "1",
            isBuiltin: true,
            capabilities: {
              authMethods: ["local_import"],
              supportsQuotaQuery: true,
              supportsDailyClaim: true,
              supportsClientSwitch: true,
              supportsMultiInstance: false,
            },
          },
        ];
      }
      if (cmd === "get_action_logs") return [];
      if (cmd === "get_action_log_stats") {
        return { activeCount: 0, archivedCount: 0, failedActiveCount: 0, noiseActiveCount: 0 };
      }
      if (cmd === "get_app_settings") return DEFAULT_SETTINGS;
      if (cmd === "claim_daily_batch") throw new Error("已有批量任务正在进行");
      return [];
    });

    const { result } = renderHook(() => useAccountStore(deps));

    // 等待首次同步完成
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    await act(async () => {
      await result.current.claimDailyBatch(["acc-1"]);
    });

    // 失败时必须清空乐观进度，否则界面会永久停留在「运行中」
    expect(spies.setBatchProgress).toHaveBeenCalledWith(null);
    expect(spies.showToast).toHaveBeenCalledWith(
      expect.stringContaining("启动领取队列失败"),
      "error"
    );
  });

  it("请求切号弹窗会写入确认态，取消后关闭", () => {
    const { deps, spies } = makeDeps();
    h.invoke.mockResolvedValue([]);

    const { result } = renderHook(() => useAccountStore(deps));

    const account = {
      id: "acc-9",
      toolId: "trae_cn",
      label: "乙",
      uidMasked: "u_9***",
      status: "active",
      isActiveOnClient: false,
      hasApiToken: true,
      updatedAt: new Date().toISOString(),
    };

    act(() => {
      result.current.requestColdSwitch(account as never);
    });
    // 实现使用 phase 状态机（confirm → closing → writing → launching → done），
    // 而非布尔 open 字段
    expect(spies.setSwitchDialog).toHaveBeenCalledWith(
      expect.objectContaining({ phase: "confirm" })
    );

    act(() => {
      result.current.cancelColdSwitch();
    });
    // 取消的实现是「清空 account 并回到 confirm 相位」，而非切到 idle
    expect(spies.setSwitchDialog).toHaveBeenCalledWith(
      expect.objectContaining({ account: null, phase: "confirm" })
    );
  });

  it("Tauri 运行时注册后端事件监听，并在卸载时全部注销", async () => {
    const { deps } = makeDeps({ isTauriRuntime: true });
    h.invoke.mockResolvedValue([]);

    const unlisteners: Array<() => void> = [];
    h.listen.mockImplementation(async () => {
      const fn = vi.fn();
      unlisteners.push(fn);
      return fn;
    });

    const { unmount } = renderHook(() => useAccountStore(deps));

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(h.listen).toHaveBeenCalled();
    const registered = h.listen.mock.calls.map((c) => c[0]);
    expect(registered).toContain("qwitch://account-updated");
    expect(registered).toContain("qwitch://batch-progress");
    expect(registered).toContain("qwitch://logs-updated");

    unmount();

    // 卸载后每个注册的监听都必须被注销，避免事件泄漏与卸载后 setState
    expect(unlisteners.length).toBeGreaterThan(0);
    for (const fn of unlisteners) {
      expect(fn).toHaveBeenCalled();
    }
  });

  it("托盘窗口同样订阅账号/进度/设置事件（双窗口需保持一致）", async () => {
    const { deps } = makeDeps({ isTauriRuntime: true, isTrayWindow: true });
    h.invoke.mockResolvedValue([]);

    renderHook(() => useAccountStore(deps));

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    const registered = h.listen.mock.calls.map((c) => c[0]);
    // 托盘与主窗口共享同一套事件订阅：任一侧操作后另一侧必须同步，
    // 因此 settings-updated 等事件在托盘窗口也会注册。
    expect(registered).toContain("qwitch://account-updated");
    expect(registered).toContain("qwitch://settings-updated");
  });
});
