// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { computeMockLogStats } from "../context/logStateUtils";
import { normalizeAppSettings } from "../context/settingsUtils";
import { INITIAL_ACCOUNTS, INITIAL_LOGS, MOCK_ADAPTERS } from "../data/mockStore";
import { toCstDayKey } from "../domain/selectors";

const resetSettingsDrafts = vi.fn();
const refreshClientStatuses = vi.fn().mockResolvedValue(undefined);
const launchToolClient = vi.fn().mockResolvedValue(undefined);

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
  launchToolClient,
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

import { computeVisibleToolCount, Dashboard } from "./Dashboard";

beforeEach(() => {
  ctxValue.current = { ...baseCtx };
  // 视图偏好会落盘，用例之间必须隔离，否则前一个用例切到「分组」会污染后一个
  localStorage.clear();
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
    const mainAll = within(screen.getByRole("navigation", { name: "主功能导航" })).getAllByRole(
      "button"
    )[0];
    expect(mainAll.textContent).toContain("账号资产");
    expect(mainAll.getAttribute("aria-current")).toBe("page");

    const toolAll = within(screen.getByRole("navigation", { name: "账号视图导航" })).getAllByRole(
      "button"
    )[0];
    expect(toolAll.getAttribute("aria-current")).toBe("page");
    expect(toolAll.textContent).toContain(String(INITIAL_ACCOUNTS.length));
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
    expect(navButton(/账号资产/).getAttribute("aria-current")).toBeNull();
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
    fireEvent.click(navButton(/账号资产/));
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
    // 本用例断言的是分组视图里的「补位卡」，显式选择而不依赖默认视图
    fireEvent.click(screen.getByRole("button", { name: "分组" }));
    const toolNav = within(screen.getByRole("navigation", { name: "账号视图导航" }));
    fireEvent.click(toolNav.getByRole("button", { name: /豆包工作/ }));

    expect(screen.queryByLabelText("搜索账号")).toBeNull();
    expect(screen.getByText("添加第 2 个 豆包工作 账号")).toBeTruthy();
    expect(screen.getByText("授权后可签")).toBeTruthy();
    expect(screen.queryByText("仅查额/切号")).toBeNull();
  });

  it("resets searchQuery and statusFilter when switching nav tabs", () => {
    render(<Dashboard />);
    const searchInput = screen.getByLabelText("搜索账号") as HTMLInputElement;
    fireEvent.change(searchInput, { target: { value: "Trae" } });
    expect(searchInput.value).toBe("Trae");

    const toolNav = within(screen.getByRole("navigation", { name: "账号视图导航" }));
    fireEvent.click(toolNav.getByRole("button", { name: /豆包工作/ }));
    expect(screen.queryByText("当前筛选条件下暂无匹配账号")).toBeNull();

    fireEvent.click(toolNav.getByRole("button", { name: /全部账号/ }));
    const resetInput = screen.getByLabelText("搜索账号") as HTMLInputElement;
    expect(resetInput.value).toBe("");
  });

  it("switches to agents and routing workspaces from the consolidated sidebar", () => {
    render(<Dashboard />);
    const mainNav = within(screen.getByRole("navigation", { name: "主功能导航" }));

    expect(mainNav.queryByRole("button", { name: /号池编排/ })).toBeNull();
    fireEvent.click(mainNav.getByRole("button", { name: /Agent 接入与调度/ }));
    expect(screen.getByRole("heading", { name: "Agent 接入与号池调度" })).toBeTruthy();

    fireEvent.click(mainNav.getByRole("button", { name: /路由观测/ }));
    expect(screen.getByRole("heading", { name: "网关路由观测与决策链路" })).toBeTruthy();
  });

  it("renders quick-launch client buttons in grouped and single-tool views", async () => {
    launchToolClient.mockClear();
    render(<Dashboard />);
    fireEvent.click(screen.getByRole("button", { name: "分组" }));

    const groupedLaunch = screen.getByRole("button", { name: /快捷启动 Trae CN 客户端/ });
    fireEvent.click(groupedLaunch);
    await waitFor(() => expect(launchToolClient).toHaveBeenCalledWith("trae_cn"));

    launchToolClient.mockClear();
    const toolNav = within(screen.getByRole("navigation", { name: "账号视图导航" }));
    fireEvent.click(toolNav.getByRole("button", { name: /Trae CN/ }));

    const headerLaunch = screen.getByRole("button", { name: /快捷启动 Trae CN 客户端/ });
    await waitFor(() => expect((headerLaunch as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(headerLaunch);
    await waitFor(() => expect(launchToolClient).toHaveBeenCalledWith("trae_cn"));
  });

  it("computes Priority+ visible tool count when overflowing", () => {
    // 未测量或全部放得下时完整展示
    expect(computeVisibleToolCount(0, 100, [100, 100, 100], 96, 6)).toBe(3);
    expect(computeVisibleToolCount(450, 100, [100, 100, 100], 96, 6)).toBe(3);
    // 溢出时预留“更多”按钮宽度，仅保留能放下的前置工具
    expect(computeVisibleToolCount(400, 100, [100, 100, 100], 96, 6)).toBe(1);
    expect(computeVisibleToolCount(410, 100, [100, 100, 100], 96, 6)).toBe(2);
    expect(computeVisibleToolCount(150, 100, [100, 100], 96, 6)).toBe(0);
  });
});

describe("Dashboard 导航可达性", () => {
  it("从工具子页点击侧栏「账号资产」可回到全部账号视图（不再出现点击无响应的死路）", () => {
    render(<Dashboard />);

    const toolNav = within(screen.getByRole("navigation", { name: "账号视图导航" }));
    const toolBtn = toolNav.getByRole("button", { name: /豆包工作/ });
    fireEvent.click(toolBtn);
    expect(toolBtn.getAttribute("aria-current")).toBe("page");

    // 工具子页下侧栏「账号资产」同样高亮为当前分区，
    // 此时点击必须回到该分区根作用域，否则用户无法从侧栏回到全部账号。
    const mainNav = within(screen.getByRole("navigation", { name: "主功能导航" }));
    const accountAssets = mainNav.getAllByRole("button")[0];
    expect(accountAssets.textContent).toContain("账号资产");
    fireEvent.click(accountAssets);

    const allBtn = within(
      screen.getByRole("navigation", { name: "账号视图导航" })
    ).getAllByRole("button")[0];
    expect(allBtn.textContent).toContain("全部账号");
    expect(allBtn.getAttribute("aria-current")).toBe("page");
  });

  it("从日志页点击侧栏「账号资产」同样回到全部账号视图", () => {
    render(<Dashboard />);
    fireEvent.click(navButton(/操作日志/));
    fireEvent.click(navButton(/账号资产/));

    const allBtn = within(
      screen.getByRole("navigation", { name: "账号视图导航" })
    ).getAllByRole("button")[0];
    expect(allBtn.getAttribute("aria-current")).toBe("page");
  });
});

describe("Dashboard 指标口径自洽", () => {
  /** 取文本中的第一个数字（KPI 卡主指标） */
  const firstNumber = (s: string) => s.match(/\d+/)?.[0];
  it("指标条数字即点击后台账里真实可见的账号数量", () => {
    render(<Dashboard />);
    // 断言基于台账行，显式切换而非依赖默认视图
    fireEvent.click(screen.getByRole("button", { name: "台账" }));

    const titles = [
      "显示全部账号",
      "仅显示本机客户端正在使用的账号",
      "仅显示今日待领取额度的账号",
      "仅显示凭据过期、风险暂停或待授权的账号",
    ];

    for (const title of titles) {
      const card = screen.getByTitle(title);
      const shown = firstNumber(card.textContent ?? "");
      fireEvent.click(card);
      const rows = screen
        .getAllByRole("row")
        .filter((r) => r.getAttribute("aria-rowindex") !== null);
      expect(rows.length).toBe(Number(shown));
    }
  });

  it("KPI 卡标签与筛选芯片标签一一对应，卡片数字即点击后可见的集合大小", () => {
    render(<Dashboard />);

    // 卡片标题即「点击后要看到的集合」的语义
    expect(screen.getByTitle("仅显示今日待领取额度的账号")).toBeTruthy();
    expect(screen.getByTitle("仅显示凭据过期、风险暂停或待授权的账号")).toBeTruthy();
    // 旧标题（与筛选语义不符）不再出现
    expect(screen.queryByText("凭据健康")).toBeNull();
    expect(screen.queryByText("今日签到")).toBeNull();
  });

  it("KPI 卡主指标与徽章数字之间不得连写，避免「2」+「10」被读成「210」", () => {
    render(<Dashboard />);
    const card = screen.getByTitle("仅显示凭据过期、风险暂停或待授权的账号");
    // 主指标后必须紧跟非数字字符，确保文本层不会出现歧义数字串
    expect(card.textContent).toMatch(/^待完善\d+正常/);
  });

  it("KPI 卡采用紧凑单行布局：左右分栏且标签与值区不互相折行挤压", () => {
    render(<Dashboard />);
    const card = screen.getByTitle("仅显示今日待领取额度的账号");
    expect(card.className).toContain("flex");
    expect(card.className).toContain("items-center");
    expect(card.className).toContain("justify-between");
  });
});
