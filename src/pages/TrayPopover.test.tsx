// @vitest-environment jsdom
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { INITIAL_ACCOUNTS, MOCK_ADAPTERS } from "../data/mockStore";

const h = vi.hoisted(() => ({ useQwitch: vi.fn() }));
vi.mock("../context/QwitchContext", () => ({ useQwitch: h.useQwitch }));

import { TrayPopover } from "./TrayPopover";

afterEach(cleanup);

let ctx: Record<string, unknown>;

beforeEach(() => {
  ctx = {
    adapters: MOCK_ADAPTERS,
    accounts: INITIAL_ACCOUNTS,
    isInitializing: false,
    clientStatuses: {},
    claimDailyBatch: vi.fn(),
    requestColdSwitch: vi.fn(),
    switchDialog: { account: null, phase: "confirm" },
    cancelColdSwitch: vi.fn(),
    refreshAllQuotas: vi.fn(),
    refreshClientStatuses: vi.fn().mockResolvedValue(undefined),
    batchProgress: null,
    dismissBatchProgress: vi.fn(),
    cstDayKey: "2026-11-20",
    theme: "light",
    toggleTheme: vi.fn(),
    quitApp: vi.fn(),
    updateInfo: null,
    updateChecking: false,
    checkAppUpdate: vi.fn().mockResolvedValue(null),
    isTauriRuntime: true,
    showToast: vi.fn(),
  };
  h.useQwitch.mockReturnValue(ctx);
});

describe("TrayPopover", () => {
  it("渲染托盘主面板入口与账号", () => {
    render(<TrayPopover onOpenMain={vi.fn()} />);
    expect(screen.getByText("主面板")).toBeTruthy();
    // 至少渲染一个真实账号标签
    expect(screen.getByText(/Trae CN · 主力工作号/)).toBeTruthy();
  });

  it("点击主面板触发 onOpenMain，切换主题触发 toggleTheme，退出触发 quitApp", () => {
    const onOpenMain = vi.fn();
    render(<TrayPopover onOpenMain={onOpenMain} />);
    fireEvent.click(screen.getByText("主面板"));
    expect(onOpenMain).toHaveBeenCalled();
    fireEvent.click(screen.getByLabelText("切换深色或浅色主题"));
    expect(ctx.toggleTheme).toHaveBeenCalled();
    fireEvent.click(screen.getByLabelText("退出 Qwitch"));
    expect(ctx.quitApp).toHaveBeenCalled();
  });

  it("无新版本时不显示任何更新入口（避免噪音）", () => {
    render(<TrayPopover onOpenMain={vi.fn()} />);
    // footer 只保留「领取」与「退出」两个主要动作；
    // 无更新时的「检查更新」入口已移除——点了只会得到「已是最新」
    expect(screen.queryByLabelText(/检查更新/)).toBeNull();
    expect(screen.queryByLabelText(/立即更新|下载 v/)).toBeNull();
  });

  it("有新版本时顶部出现更新条带并展示版本号", () => {
    ctx.updateInfo = {
      updateAvailable: true,
      currentVersion: "0.4.0",
      latestVersion: "0.5.0",
      releaseUrl: "https://github.com/x/releases",
      assetUrl: "https://github.com/x/a.dmg",
      assetName: "Qwitch_0.5.0_aarch64.dmg",
      checksumsUrl: "https://github.com/x/SHA256SUMS.txt",
      canAutoInstall: false,
      releaseNotes: "",
      strategy: "manual",
      platformLabel: "macOS",
      hint: "",
    };
    render(<TrayPopover onOpenMain={vi.fn()} />);
    expect(screen.getByText("v0.5.0")).toBeTruthy();
    expect(screen.getByText(/新版本/)).toBeTruthy();
  });

  it("无新版本时不出现更新条带", () => {
    render(<TrayPopover onOpenMain={vi.fn()} />);
    expect(screen.queryByText(/新版本/)).toBeNull();
  });

  it("有新版本时展示图标按钮（安装 / 详情），无文字标签", () => {
    ctx.updateInfo = {
      updateAvailable: true,
      currentVersion: "0.4.2",
      latestVersion: "0.5.0",
      releaseUrl: "https://github.com/x/releases",
      assetUrl: "https://github.com/x/a.dmg",
      assetName: "Qwitch_0.5.0_aarch64.dmg",
      checksumsUrl: "https://github.com/x/SHA256SUMS.txt",
      canAutoInstall: false,
      releaseNotes: "",
      strategy: "manual",
      platformLabel: "macOS",
      hint: "",
    };
    render(<TrayPopover onOpenMain={vi.fn()} />);
    // 384px 宽度下用图标而非文字，避免与 footer 主按钮争抢空间
    expect(screen.getByLabelText(/下载 v0\.5\.0/)).toBeTruthy();
    expect(screen.getByLabelText("查看更新详情")).toBeTruthy();
    expect(screen.queryByText("立即更新")).toBeNull();
  });

  it("托盘内安装失败时给出 Toast 提示，而非静默吞掉", async () => {
    const invokeMock = vi.fn().mockRejectedValue(new Error("checksum mismatch"));
    vi.doMock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
    ctx.updateInfo = {
      updateAvailable: true,
      currentVersion: "0.4.1",
      latestVersion: "0.5.0",
      releaseUrl: "https://github.com/x/releases",
      assetUrl: "https://github.com/x/a.exe",
      assetName: "Qwitch_0.5.0_x64-setup.exe",
      checksumsUrl: "https://github.com/x/SHA256SUMS.txt",
      canAutoInstall: true,
      releaseNotes: "",
      strategy: "auto_install",
      platformLabel: "Windows",
      hint: "",
    };
    render(<TrayPopover onOpenMain={vi.fn()} />);
    // 关键：失败必须让用户看见，此前是静默 catch，用户只能反复重试
    const btn = screen.getByRole("button", { name: /立即更新/ });
    fireEvent.click(btn);
    await Promise.resolve();
    expect(ctx.showToast).toHaveBeenCalledWith(
      expect.stringContaining("更新失败"),
      "error"
    );
    vi.doUnmock("@tauri-apps/api/core");
  });
});
