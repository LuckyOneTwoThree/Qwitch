// @vitest-environment jsdom
import { renderHook, act, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSettings } from "../types/qwitch";

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke }));

import { useSettingsStore } from "./useSettingsStore";

beforeEach(() => {
  h.invoke.mockReset();
  window.localStorage.clear();
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("useSettingsStore", () => {
  it("取消插件信任弹窗时不污染前端状态，后续设置保存仍可正常提交", async () => {
    const showToast = vi.fn();
    const pendingAutoClaimRef = { current: false };

    const { result } = renderHook(() =>
      useSettingsStore({
        isTauriRuntime: true,
        showToast,
        pendingAutoClaimRef,
      })
    );

    h.invoke.mockImplementation(async (cmd: string, args?: { settings?: AppSettings }) => {
      if (cmd === "request_plugin_trust") {
        throw new Error("已取消：未授予该插件的信任");
      }
      if (cmd === "save_app_settings") {
        return args?.settings;
      }
      return null;
    });

    let ok = true;
    await act(async () => {
      ok = await result.current.saveAppSettings({
        trustedAdapters: ["ext_demo"],
        trustedAdapterHashes: { ext_demo: "hash_1" },
      });
    });

    expect(ok).toBe(false);
    expect(result.current.appSettings.trustedAdapters).toEqual([]);
    expect(result.current.appSettingsRef.current.trustedAdapters).toEqual([]);
    expect(h.invoke).toHaveBeenCalledTimes(1);
    expect(h.invoke).toHaveBeenCalledWith("request_plugin_trust", { adapterId: "ext_demo" });

    // 取消之后修改主题，不应携带未批准的插件信任也不应触发报错
    let themeOk = false;
    await act(async () => {
      themeOk = await result.current.saveAppSettings({ theme: "dark" }, true);
    });

    expect(themeOk).toBe(true);
    expect(result.current.appSettings.theme).toBe("dark");
    expect(result.current.appSettings.trustedAdapters).toEqual([]);
    expect(h.invoke).toHaveBeenLastCalledWith(
      "save_app_settings",
      expect.objectContaining({
        settings: expect.objectContaining({
          theme: "dark",
          trustedAdapters: [],
        }),
        trustedAdapterTickets: {},
      })
    );
  });

  it("已信任插件内容哈希变化时，重新申请原生确认票据并提交", async () => {
    const showToast = vi.fn();
    const pendingAutoClaimRef = { current: false };

    const { result } = renderHook(() =>
      useSettingsStore({
        isTauriRuntime: true,
        showToast,
        pendingAutoClaimRef,
      })
    );

    act(() => {
      result.current.applyIncomingSettings(
        {
          trustedAdapters: ["ext_demo"],
          trustedAdapterHashes: { ext_demo: "hash_old" },
        },
        true
      );
    });

    h.invoke.mockImplementation(async (cmd: string, args?: { settings?: AppSettings }) => {
      if (cmd === "request_plugin_trust") {
        return "ticket-resign-1";
      }
      if (cmd === "save_app_settings") {
        return args?.settings;
      }
      return null;
    });

    let ok = false;
    await act(async () => {
      ok = await result.current.saveAppSettings({
        trustedAdapters: ["ext_demo"],
        trustedAdapterHashes: { ext_demo: "hash_new" },
      });
    });

    expect(ok).toBe(true);
    expect(h.invoke).toHaveBeenNthCalledWith(1, "request_plugin_trust", {
      adapterId: "ext_demo",
    });
    expect(h.invoke).toHaveBeenNthCalledWith(
      2,
      "save_app_settings",
      expect.objectContaining({
        trustedAdapterTickets: { ext_demo: "ticket-resign-1" },
      })
    );
  });

  it("save_app_settings 后端返回失败时正确回滚前端状态与主题", async () => {
    const showToast = vi.fn();
    const pendingAutoClaimRef = { current: false };

    const { result } = renderHook(() =>
      useSettingsStore({
        isTauriRuntime: true,
        showToast,
        pendingAutoClaimRef,
      })
    );

    act(() => {
      result.current.applyIncomingSettings({ theme: "light" }, true);
    });
    expect(result.current.theme).toBe("light");

    h.invoke.mockRejectedValue(new Error("磁盘不可写"));

    let ok = true;
    await act(async () => {
      ok = await result.current.saveAppSettings({ theme: "dark" });
    });

    expect(ok).toBe(false);
    expect(result.current.appSettings.theme).toBe("light");
    expect(result.current.theme).toBe("light");
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining("保存设置失败"), "error");
  });
});
