// @vitest-environment jsdom
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MOCK_ADAPTERS } from "../data/mockStore";
import type { AccountSummary } from "../types/qwitch";

const h = vi.hoisted(() => ({ useQwitch: vi.fn() }));
vi.mock("../context/QwitchContext", () => ({ useQwitch: h.useQwitch }));

import { ColdSwitchModal } from "./ColdSwitchModal";

afterEach(cleanup);

const account: AccountSummary = {
  id: "acc-1",
  toolId: "trae_cn",
  label: "目标账号A",
  uidMasked: "u_****9",
  status: "active",
  isActiveOnClient: false,
  updatedAt: new Date().toISOString(),
};

let ctx: Record<string, unknown>;

beforeEach(() => {
  ctx = {
    switchDialog: { account: null, phase: "confirm" },
    cancelColdSwitch: vi.fn(),
    confirmColdSwitch: vi.fn(),
    adapters: MOCK_ADAPTERS,
  };
  h.useQwitch.mockReturnValue(ctx);
});

describe("ColdSwitchModal", () => {
  it("无目标账号时不渲染", () => {
    const { container } = render(<ColdSwitchModal />);
    expect(container.textContent).toBe("");
  });

  it("展示目标账号并支持取消/确认", () => {
    ctx.switchDialog = { account, phase: "confirm", isClientRunning: true };
    render(<ColdSwitchModal />);
    expect(screen.getByText("切换 Trae CN 桌面端账号")).toBeTruthy();
    expect(screen.getByText(/目标账号：目标账号A/)).toBeTruthy();

    fireEvent.click(screen.getByText("取消"));
    expect(ctx.cancelColdSwitch).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByText("确认关闭并切换"));
    expect(ctx.confirmColdSwitch).toHaveBeenCalledTimes(1);
  });
});
