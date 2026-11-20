// @vitest-environment jsdom
import { renderHook, act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppUpdateInfo } from "../types/qwitch";

/**
 * `useUpdateActions` 收敛了三条更新入口（设置页 / 主窗口 banner / 托盘）的
 * 动作实现。此前三份逻辑已分叉（托盘版一度静默吞错），此测试锁定统一后的语义。
 */

const h = vi.hoisted(() => ({
  useQwitch: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("../context/QwitchContext", () => ({ useQwitch: h.useQwitch }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke }));

import { useUpdateActions } from "./useUpdateActions";

function makeInfo(overrides: Partial<AppUpdateInfo> = {}): AppUpdateInfo {
  return {
    updateAvailable: true,
    currentVersion: "0.4.3",
    latestVersion: "0.5.0",
    releaseUrl: "https://github.com/x/releases/tag/v0.5.0",
    releaseNotes: "",
    assetName: "Qwitch_0.5.0_x64-setup.exe",
    assetUrl: "https://github.com/x/Qwitch_0.5.0_x64-setup.exe",
    assetSize: 1024,
    checksumsUrl: "https://github.com/x/SHA256SUMS.txt",
    strategy: "auto_install",
    platformLabel: "Windows",
    canAutoInstall: true,
    hint: "",
    ...overrides,
  } as AppUpdateInfo;
}

let showToast: ReturnType<typeof vi.fn>;

function setup(info: AppUpdateInfo | null) {
  showToast = vi.fn();
  h.useQwitch.mockReturnValue({ updateInfo: info, showToast });
  return renderHook(() => useUpdateActions());
}

beforeEach(() => {
  h.useQwitch.mockReset();
  h.invoke.mockReset();
  h.invoke.mockResolvedValue("安装程序已启动");
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("useUpdateActions", () => {
  it("支持应用内安装时调用下载安装命令并提示成功", async () => {
    const { result } = setup(makeInfo({ canAutoInstall: true }));
    await act(async () => {
      await result.current.runPrimaryAction();
    });
    expect(h.invoke).toHaveBeenCalledWith("download_and_install_update", {
      assetUrl: "https://github.com/x/Qwitch_0.5.0_x64-setup.exe",
      assetName: "Qwitch_0.5.0_x64-setup.exe",
      checksumsUrl: "https://github.com/x/SHA256SUMS.txt",
    });
    expect(showToast).toHaveBeenCalledWith("安装程序已启动", "success");
  });

  it("不支持应用内安装时打开下载页而非报错", async () => {
    const { result } = setup(
      makeInfo({ canAutoInstall: false, assetUrl: "https://github.com/x/a.dmg" })
    );
    await act(async () => {
      await result.current.runPrimaryAction();
    });
    expect(h.invoke).toHaveBeenCalledWith("open_update_url", {
      url: "https://github.com/x/a.dmg",
    });
    expect(h.invoke).not.toHaveBeenCalledWith(
      "download_and_install_update",
      expect.anything()
    );
  });

  it("安装失败时提示错误并复位按钮状态（不静默）", async () => {
    h.invoke.mockRejectedValue(new Error("checksum mismatch"));
    const { result } = setup(makeInfo());
    await act(async () => {
      await result.current.runPrimaryAction();
    });
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining("更新失败"),
      "error"
    );
    // 复位后可重试
    expect(result.current.installing).toBe(false);
  });

  it("安装信息不完整时提示而非静默返回", async () => {
    const { result } = setup(makeInfo({ checksumsUrl: undefined }));
    await act(async () => {
      await result.current.runPrimaryAction();
    });
    expect(h.invoke).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining("更新信息不完整"),
      "warning"
    );
  });

  it("打开下载页失败时提示错误", async () => {
    h.invoke.mockRejectedValue(new Error("no handler"));
    const { result } = setup(makeInfo());
    await act(async () => {
      await result.current.openDownloadPage("https://x/a.dmg");
    });
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining("打开下载页失败"),
      "error"
    );
  });

  it("primaryLabel 按平台能力区分文案", () => {
    const { result: auto } = setup(makeInfo({ canAutoInstall: true }));
    expect(auto.current.primaryLabel).toBe("立即更新到 v0.5.0");

    const { result: manual } = setup(makeInfo({ canAutoInstall: false }));
    expect(manual.current.primaryLabel).toBe("下载 v0.5.0");
  });

  it("updateInfo 为 null 时不做任何调用", async () => {
    const { result } = setup(null);
    await act(async () => {
      await result.current.runPrimaryAction();
    });
    expect(h.invoke).not.toHaveBeenCalled();
  });
});
