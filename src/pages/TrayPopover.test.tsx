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
});
