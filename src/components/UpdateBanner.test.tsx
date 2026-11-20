// @vitest-environment jsdom
import { render, screen, cleanup, fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppUpdateInfo } from "../types/qwitch";

/**
 * `UpdateBanner` 把「发现新版本」从设置页底部提升到主窗口常驻位置。
 * 关键行为：只在确实有新版本时出现、可关闭、按平台给出不同动作。
 */

const h = vi.hoisted(() => ({
  useQwitch: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("../context/QwitchContext", () => ({ useQwitch: h.useQwitch }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke }));

import { UpdateBanner } from "./UpdateBanner";

afterEach(cleanup);

function makeInfo(overrides: Partial<AppUpdateInfo> = {}): AppUpdateInfo {
  return {
    updateAvailable: true,
    currentVersion: "0.4.0",
    latestVersion: "0.5.0",
    releaseUrl: "https://github.com/LuckyOneTwoThree/Qwitch/releases/tag/v0.5.0",
    releaseNotes: "修复若干问题",
    assetName: "Qwitch_0.5.0_x64-setup.exe",
    assetUrl: "https://github.com/x/download/v0.5.0/Qwitch_0.5.0_x64-setup.exe",
    assetSize: 1024 * 1024,
    checksumsUrl: "https://github.com/x/download/v0.5.0/SHA256SUMS.txt",
    strategy: "auto_install",
    platformLabel: "Windows",
    canAutoInstall: true,
    hint: "",
    ...overrides,
  } as AppUpdateInfo;
}

function setup(info: AppUpdateInfo | null, opts: { isTauriRuntime?: boolean } = {}) {
  const showToast = vi.fn();
  h.useQwitch.mockReturnValue({
    isTauriRuntime: opts.isTauriRuntime ?? true,
    showToast,
    updateInfo: info,
  });
  const onOpenSettings = vi.fn();
  const utils = render(<UpdateBanner onOpenSettings={onOpenSettings} />);
  return { ...utils, showToast, onOpenSettings };
}

beforeEach(() => {
  h.useQwitch.mockReset();
  h.invoke.mockReset();
  h.invoke.mockResolvedValue("ok");
});

describe("UpdateBanner", () => {
  it("无新版本时不渲染任何内容", () => {
    setup(makeInfo({ updateAvailable: false }));
    expect(screen.queryByText(/发现新版本/)).toBeNull();
  });

  it("updateInfo 为 null 时不渲染", () => {
    setup(null);
    expect(screen.queryByText(/发现新版本/)).toBeNull();
  });

  it("浏览器预览模式不渲染（避免误导）", () => {
    setup(makeInfo(), { isTauriRuntime: false });
    expect(screen.queryByText(/发现新版本/)).toBeNull();
  });

  it("有新版本时展示版本号对比", () => {
    setup(makeInfo());
    expect(screen.getByText(/发现新版本/)).toBeTruthy();
    expect(screen.getByText("v0.5.0")).toBeTruthy();
    expect(screen.getByText(/当前 v0\.4\.0/)).toBeTruthy();
  });

  it("支持应用内安装时展示「立即更新」并调用安装命令", async () => {
    setup(makeInfo({ canAutoInstall: true, assetUrl: "https://x/a.exe" }));
    const btn = screen.getByRole("button", { name: /立即更新/ });
    fireEvent.click(btn);
    expect(h.invoke).toHaveBeenCalledWith(
      "download_and_install_update",
      expect.objectContaining({ assetName: "Qwitch_0.5.0_x64-setup.exe" })
    );
  });

  it("不支持应用内安装时展示「下载」并打开下载页", async () => {
    setup(makeInfo({ canAutoInstall: false, assetUrl: "https://x/a.dmg" }));
    const btn = screen.getByRole("button", { name: /下载/ });
    fireEvent.click(btn);
    expect(h.invoke).toHaveBeenCalledWith("open_update_url", { url: "https://x/a.dmg" });
  });

  it("点击「详情」跳转设置页", () => {
    const { onOpenSettings } = setup(makeInfo());
    fireEvent.click(screen.getByRole("button", { name: /详情/ }));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("关闭后本次运行不再提示", () => {
    setup(makeInfo());
    expect(screen.getByText(/发现新版本/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /关闭更新提示/ }));
    expect(screen.queryByText(/发现新版本/)).toBeNull();
  });
});
