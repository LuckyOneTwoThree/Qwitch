// @vitest-environment jsdom
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MOCK_ADAPTERS } from "../data/mockStore";
import type { AccountSummary } from "../types/qwitch";

const h = vi.hoisted(() => ({ useQwitch: vi.fn() }));
vi.mock("../context/QwitchContext", () => ({ useQwitch: h.useQwitch }));

import { AccountCard } from "./AccountCard";

afterEach(cleanup);

const account: AccountSummary = {
  id: "acc-test-1",
  toolId: "trae_cn",
  label: "测试账号甲",
  uidMasked: "u_1234****ab",
  status: "active",
  isActiveOnClient: false,
  latestQuota: {
    claimStatus: "available",
    fetchedAt: new Date().toISOString(),
    buckets: [
      { key: "fast_requests", label: "快速请求额度", used: 10, total: 100, remaining: 90, unit: "次" },
    ],
  },
  updatedAt: new Date().toISOString(),
};

let ctx: Record<string, unknown>;

beforeEach(() => {
  ctx = {
    refreshAccountQuota: vi.fn(),
    claimDailyBatch: vi.fn().mockResolvedValue(undefined),
    requestColdSwitch: vi.fn(),
    recoverAccount: vi.fn(),
    deleteAccount: vi.fn(),
    importAccount: vi.fn(),
    previewLocalAccount: vi.fn(),
    renameAccount: vi.fn(),
    toggleAccountDisabled: vi.fn(),
    adapters: MOCK_ADAPTERS,
    batchProgress: null,
    isTauriRuntime: true,
    showToast: vi.fn(),
  };
  h.useQwitch.mockReturnValue(ctx);
});

describe("AccountCard", () => {
  it("渲染账号标签与脱敏 UID", () => {
    render(<AccountCard account={account} />);
    expect(screen.getByText("测试账号甲")).toBeTruthy();
    expect(screen.getByText(/u_1234\*\*\*\*ab/)).toBeTruthy();
  });

  it("点击「切至桌面端」触发 requestColdSwitch", () => {
    render(<AccountCard account={account} />);
    fireEvent.click(screen.getByText("切至桌面端"));
    expect(ctx.requestColdSwitch).toHaveBeenCalledWith(account);
  });

  it("点击「领取」触发 claimDailyBatch", () => {
    render(<AccountCard account={account} />);
    fireEvent.click(screen.getByText("领取"));
    expect(ctx.claimDailyBatch).toHaveBeenCalledWith([account.id]);
  });

  it("通过账号管理菜单停用账号", () => {
    render(<AccountCard account={account} />);
    fireEvent.click(screen.getByLabelText(`账号管理 ${account.label}`));
    fireEvent.click(screen.getByText("停用账号"));
    expect(ctx.toggleAccountDisabled).toHaveBeenCalledWith(account.id);
  });

  it("通过账号管理菜单打开删除确认", () => {
    render(<AccountCard account={account} />);
    fireEvent.click(screen.getByLabelText(`账号管理 ${account.label}`));
    fireEvent.click(screen.getByText("删除账号"));
    expect(screen.getByText(/确认删除账号/)).toBeTruthy();
  });
});
