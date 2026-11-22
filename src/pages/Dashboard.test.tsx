// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { computeMockLogStats } from "../context/logStateUtils";
import { normalizeAppSettings } from "../context/settingsUtils";
import { INITIAL_ACCOUNTS, INITIAL_LOGS, MOCK_ADAPTERS } from "../data/mockStore";
import { toCstDayKey } from "../domain/selectors";

const resetSettingsDrafts = vi.fn();
const refreshClientStatuses = vi.fn().mockResolvedValue(undefined);

// 稳定引用：Dashboard 的 effect 依赖这些值，每次渲染返回新对象会造成无限循环
const ctxValue = vi.hoisted(() => ({ current: null as Record<string, unknown> | null }));
vi.mock("../context/QwitchContext", () => ({
  useQwitch: () => ctxValue.current,
}));
const baseCtx: Record<string, unknown> = {
  isInitializing: false,
  isTauriRuntime: false,
  accounts: INITIAL_ACCOUNTS,
  logs: INITIAL_LOGS,
  logScope: "active",
  logStats: computeMockLogStats(INITIAL_LOGS),
  adapters: MOCK_ADAPTERS,
  refreshAllQuotas: vi.fn(),
  claimDailyBatch: vi.fn(),
  batchProgress: null,
  appSettings: normalizeAppSettings(),
  customClientPaths: {},
  clientStatuses: {},
  resetSettingsDrafts,
  refreshClientStatuses,
  cstDayKey: toCstDayKey(Date.now()),
  theme: "light",
  toggleTheme: vi.fn(),
  quitApp: vi.fn(),
};
ctxValue.current = { ...baseCtx };
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../components/AccountCard", () => ({ AccountCard: () => null }));
vi.mock("../components/BatchClaimBanner", () => ({ BatchClaimBanner: () => null }));
vi.mock("../components/ColdSwitchModal", () => ({ ColdSwitchModal: () => null }));
vi.mock("../components/ImportAccountModal", () => ({ ImportAccountModal: () => null }));
vi.mock("../components/ui/ConfirmDialog", () => ({ ConfirmDialog: () => null }));
vi.mock("../components/ToolBrandIcon", () => ({
  QwitchLogo: () => null,
  ToolBrandIcon: () => null,
}));
vi.mock("./dashboard/LogsView", () => ({
  LogsToolbar: () => <div data-testid="logs-toolbar" />,
  LogsPanel: () => <div data-testid="logs-panel" />,
}));
vi.mock("./dashboard/SettingsView", () => ({
  SettingsView: () => <div data-testid="settings-view" />,
}));

import { Dashboard } from "./Dashboard";

beforeEach(() => {
  ctxValue.current = { ...baseCtx };
  // jsdom 未实现 Element.scrollTo
  Element.prototype.scrollTo = vi.fn() as unknown as typeof Element.prototype.scrollTo;
  vi.stubGlobal("__APP_VERSION__", "0.0.0-test");
  resetSettingsDrafts.mockClear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const navButton = (name: RegExp) =>
  screen.getAllByRole("button").find((b) => name.test(b.textContent ?? ""))!;

describe("Dashboard navigation", () => {
  it("starts on the all-accounts view with aria-current and a total count", () => {
    render(<Dashboard />);
    const all = within(screen.getByRole("navigation", { name: "账号视图导航" })).getAllByRole("button")[0];
    expect(all.getAttribute("aria-current")).toBe("page");
    expect(all.textContent).toContain(String(INITIAL_ACCOUNTS.length));
    expect(screen.queryByTestId("settings-view")).toBeNull();
    expect(screen.queryByTestId("logs-panel")).toBeNull();
  });

  it("shows per-tool counts computed from the account list", () => {
    render(<Dashboard />);
    const toolNav = within(screen.getByRole("navigation", { name: "账号视图导航" }));
    const buttons = toolNav.getAllByRole("button").slice(1);
    const counts = buttons.map((b) => Number(b.textContent!.match(/(\d+)\s*$/)?.[1]));
    const expected = new Map<string, number>();
    for (const a of INITIAL_ACCOUNTS) expected.set(a.toolId, (expected.get(a.toolId) ?? 0) + 1);
    expect(counts.reduce((x, y) => x + y, 0)).toBe(INITIAL_ACCOUNTS.length);
    for (const [toolId, n] of expected) {
      const name = MOCK_ADAPTERS.find((ad) => ad.id === toolId)!.name;
      const btn = buttons.find((b) => b.textContent!.includes(name))!;
      expect(btn.textContent!.endsWith(String(n))).toBe(true);
    }
  });

  it("moves aria-current and mounts the settings view", () => {
    render(<Dashboard />);
    fireEvent.click(navButton(/设置与环境/));
    expect(navButton(/设置与环境/).getAttribute("aria-current")).toBe("page");
    expect(navButton(/全部账号/).getAttribute("aria-current")).toBeNull();
    expect(screen.getByTestId("settings-view")).toBeTruthy();
  });

  it("mounts the logs toolbar and panel on the logs view", () => {
    render(<Dashboard />);
    fireEvent.click(navButton(/操作日志/));
    expect(navButton(/操作日志/).getAttribute("aria-current")).toBe("page");
    expect(screen.getByTestId("logs-toolbar")).toBeTruthy();
    expect(screen.getByTestId("logs-panel")).toBeTruthy();
  });

  it("discards unsaved settings drafts only when leaving the settings view", () => {
    render(<Dashboard />);
    fireEvent.click(navButton(/操作日志/));
    expect(resetSettingsDrafts).not.toHaveBeenCalled();
    fireEvent.click(navButton(/设置与环境/));
    expect(resetSettingsDrafts).not.toHaveBeenCalled();
    fireEvent.click(navButton(/全部账号/));
    expect(resetSettingsDrafts).toHaveBeenCalledTimes(1);
  });

  it("single-tool 1-account view with missing auth hides search toolbar, renders ghost card, and shows 授权后可签", () => {
    ctxValue.current = {
      ...baseCtx,
      accounts: [
        {
          id: "acc-dw-single",
          toolId: "doubaowork",
          label: "Lucky",
          uidMasked: "4311****39",
          status: "active",
          isActiveOnClient: true,
          hasApiToken: false,
          hasLocalSnapshot: true,
          updatedAt: new Date().toISOString(),
        },
      ],
    };

    render(<Dashboard />);
    fireEvent.click(navButton(/豆包工作/));

    expect(screen.queryByLabelText("搜索账号")).toBeNull();
    expect(screen.getByText("添加第 2 个 豆包工作 账号")).toBeTruthy();
    expect(screen.getByText("授权后可签")).toBeTruthy();
    expect(screen.queryByText("仅查额/切号")).toBeNull();
  });
});
