// @vitest-environment jsdom
import { render, screen, fireEvent, cleanup, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { INITIAL_ACCOUNTS, MOCK_ADAPTERS } from "../data/mockStore";
import { createInitialAgentBindings, INITIAL_GATEWAY_STATE, makeDefaultModelKey } from "../data/mockPoolData";
import type { AccountPool, AgentBinding, RoutingTraceItem } from "../types/pool";

const h = vi.hoisted(() => ({
  useQwitch: vi.fn(),
  poolOverride: null as Record<string, unknown> | null,
}));
vi.mock("../context/QwitchContext", () => ({ useQwitch: h.useQwitch }));
vi.mock("../context/usePoolPrototypeStore", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../context/usePoolPrototypeStore")>();
  return {
    ...actual,
    usePoolPrototype: () => {
      const base = actual.usePoolPrototype();
      return h.poolOverride ? { ...base, ...h.poolOverride } : base;
    },
  };
});

import { TrayPopover } from "./TrayPopover";

afterEach(() => {
  h.poolOverride = null;
  cleanup();
});

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

  it("缺在线凭据显示「待授权」并可点去授权，缺本机快照优先显示「未关联本机」", () => {
    const onOpenMain = vi.fn();
    ctx.accounts = [
      {
        id: "tray-dw-no-token",
        toolId: "doubaowork",
        label: "豆包缺授权",
        uidMasked: "4311****39",
        status: "active",
        isActiveOnClient: true,
        hasApiToken: false,
        hasLocalSnapshot: true,
        updatedAt: new Date().toISOString(),
      },
      {
        id: "tray-dw-no-local",
        toolId: "doubaowork",
        label: "豆包缺快照",
        uidMasked: "4311****88",
        status: "active",
        isActiveOnClient: true,
        hasApiToken: true,
        hasLocalSnapshot: false,
        updatedAt: new Date().toISOString(),
      },
    ];
    render(<TrayPopover onOpenMain={onOpenMain} />);
    expect(screen.getByText("待授权")).toBeTruthy();
    expect(screen.getByText("待关联")).toBeTruthy();
    expect(screen.getByText("未关联本机")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /去授权/ }));
    expect(onOpenMain).toHaveBeenCalled();
  });

  it("Agent 接入页签支持网关启停、环境重探、漂移一键修复与路由观测直达", () => {
    const pools: AccountPool[] = [
      {
        id: "pool-dsh",
        slug: "dsh",
        name: "DSH 专属号池",
        description: "dsh",
        sceneTag: "dsh",
        enabled: true,
        strategy: "quota_first",
        affinityMode: "auto_cache",
        skipActiveClientAccount: false,
        cooldownSeconds: 60,
        autoRefreshOn401: true,
        fallbackApiKeyEnabled: false,
        modelPolicies: [
          {
            modelId: "workbuddy/deepseek-v4-pro",
            enabled: true,
            fallbackModelId: null,
          },
        ],
        members: [
          {
            id: "m1",
            accountId: "a1",
            toolId: "workbuddy",
            label: "WB 主号",
            uidMasked: "u1",
            supportedModels: ["workbuddy/deepseek-v4-pro"],
            tier: "t1_primary",
            channel: "rust_native",
            enabled: true,
            priority: 1,
            quotaPercent: 90,
            quotaText: "90%",
          },
        ],
        todayRequests: 0,
        todayFailovers: 0,
        cacheHitRate: 0,
        createdAt: "2026-11-24T00:00:00Z",
      },
    ];
    const bindings: AgentBinding[] = createInitialAgentBindings(pools, INITIAL_GATEWAY_STATE)
      .slice(0, 2)
      .map((b, idx) =>
        idx === 0
          ? {
              ...b,
              agentId: "dsh",
              name: "DeepSeek Harness",
              mountedPoolIds: ["pool-dsh"],
              defaultModelKey: makeDefaultModelKey("pool-dsh", "workbuddy/deepseek-v4-pro"),
              status: "drifted",
              installed: true,
              running: true,
            }
          : {
              ...b,
              agentId: "claude_desktop",
              name: "Claude 桌面版",
              mountedPoolIds: [],
              defaultModelKey: null,
              status: "unbound",
              installed: true,
              running: false,
            }
      );
    const traces: RoutingTraceItem[] = [
      {
        id: "t1",
        timestamp: "16:00:00",
        agentId: "dsh",
        poolId: "pool-dsh",
        poolName: "DSH 专属号池",
        requestedModel: "deepseek-v4-pro",
        incomingProtocol: "openai_chat",
        upstreamProtocol: "openai_chat",
        finalToolId: "workbuddy",
        finalAccountLabel: "WB 主号",
        finalModel: "deepseek-v4-pro",
        cacheAffinityHit: false,
        ttftMs: 120,
        totalTokens: 100,
        status: "success",
        attempts: [],
      },
    ];

    const toggleGateway = vi.fn().mockResolvedValue(undefined);
    const detectAgentEnvironment = vi.fn().mockResolvedValue(undefined);
    const repairAgentDrift = vi.fn();
    const onOpenMain = vi.fn();

    h.poolOverride = {
      gateway: { ...INITIAL_GATEWAY_STATE, running: false },
      pools,
      agentBindings: bindings,
      traces,
      toggleGateway,
      detectAgentEnvironment,
      repairAgentDrift,
    };

    render(<TrayPopover onOpenMain={onOpenMain} />);
    fireEvent.click(screen.getByRole("tab", { name: /Agent 接入/ }));

    expect(screen.getByText(/网关已关闭 · 已接入 1\/2 个 Agent · 1 漂移/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "启动" }));
    expect(toggleGateway).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByLabelText("重新探测 Agent 环境"));
    expect(detectAgentEnvironment).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "一键修复" }));
    expect(repairAgentDrift).toHaveBeenCalledWith("dsh");

    fireEvent.click(screen.getByRole("button", { name: /查看观测/ }));
    expect(onOpenMain).toHaveBeenCalledWith({ nav: "routing" });

    fireEvent.click(screen.getByRole("button", { name: /打开 Agent 接入与调度/ }));
    expect(onOpenMain).toHaveBeenCalledWith({ nav: "agents" });
  });

  it("已挂载 Agent 下拉框严格隔离自身模型，支持热切默认模型、解绑与定向跳转", () => {
    const pools: AccountPool[] = [
      {
        id: "pool-dsh",
        slug: "dsh",
        name: "DSH 号池",
        description: "dsh",
        sceneTag: "dsh",
        enabled: true,
        strategy: "quota_first",
        affinityMode: "auto_cache",
        skipActiveClientAccount: false,
        cooldownSeconds: 60,
        autoRefreshOn401: true,
        fallbackApiKeyEnabled: false,
        modelPolicies: [
          { modelId: "workbuddy/deepseek-v4-pro", enabled: true, fallbackModelId: null },
          { modelId: "workbuddy/kimi-k3.1-pro", enabled: true, fallbackModelId: null },
        ],
        members: [
          {
            id: "m1",
            accountId: "a1",
            toolId: "workbuddy",
            label: "WB-1",
            uidMasked: "u1",
            supportedModels: ["workbuddy/deepseek-v4-pro", "workbuddy/kimi-k3.1-pro"],
            tier: "t1_primary",
            channel: "rust_native",
            enabled: true,
            priority: 1,
            quotaPercent: 80,
            quotaText: "80%",
          },
        ],
        todayRequests: 0,
        todayFailovers: 0,
        cacheHitRate: 0,
        createdAt: "2026-11-24T00:00:00Z",
      },
      {
        id: "pool-other-agent",
        slug: "other",
        name: "其它 Agent 专用池",
        description: "other",
        sceneTag: "other",
        enabled: true,
        strategy: "quota_first",
        affinityMode: "auto_cache",
        skipActiveClientAccount: false,
        cooldownSeconds: 60,
        autoRefreshOn401: true,
        fallbackApiKeyEnabled: false,
        modelPolicies: [
          { modelId: "qoder_cn/qwen4-max", enabled: true, fallbackModelId: null },
        ],
        members: [
          {
            id: "m2",
            accountId: "a2",
            toolId: "qoder_cn",
            label: "Qoder-1",
            uidMasked: "u2",
            supportedModels: ["qoder_cn/qwen4-max"],
            tier: "t1_primary",
            channel: "rust_native",
            enabled: true,
            priority: 1,
            quotaPercent: 90,
            quotaText: "90%",
          },
        ],
        todayRequests: 0,
        todayFailovers: 0,
        cacheHitRate: 0,
        createdAt: "2026-11-24T00:00:00Z",
      },
    ];
    const bindings: AgentBinding[] = createInitialAgentBindings(pools, INITIAL_GATEWAY_STATE)
      .slice(0, 2)
      .map((b, idx) =>
        idx === 0
          ? {
              ...b,
              agentId: "dsh",
              name: "DeepSeek Harness",
              mountedPoolIds: ["pool-dsh"],
              defaultModelKey: makeDefaultModelKey("pool-dsh", "workbuddy/deepseek-v4-pro"),
              status: "bound",
              installed: true,
              running: true,
            }
          : {
              ...b,
              agentId: "codex_desktop",
              name: "Codex 桌面版",
              mountedPoolIds: [],
              defaultModelKey: null,
              status: "unbound",
              installed: true,
              running: false,
            }
      );

    const setAgentDefaultModel = vi.fn();
    const unbindAgent = vi.fn();
    const onOpenMain = vi.fn();
    const kimiKey = makeDefaultModelKey("pool-dsh", "workbuddy/kimi-k3.1-pro");

    h.poolOverride = {
      gateway: { ...INITIAL_GATEWAY_STATE, running: true },
      pools,
      agentBindings: bindings,
      traces: [],
      setAgentDefaultModel,
      unbindAgent,
    };

    render(<TrayPopover onOpenMain={onOpenMain} />);
    fireEvent.click(screen.getByRole("tab", { name: /Agent 接入/ }));

    const select = screen.getByLabelText("切换 DeepSeek Harness 默认模型") as HTMLSelectElement;
    const optionsText = within(select)
      .getAllByRole("option")
      .map((o) => o.textContent ?? "");
    expect(optionsText.some((t) => t.includes("DeepSeek-V4-Pro"))).toBe(true);
    expect(optionsText.some((t) => t.includes("Kimi-K3.1-Pro"))).toBe(true);
    // 严格隔离：未挂载的其它 Agent 号池模型不能混入
    expect(optionsText.some((t) => t.includes("Qwen4-Max"))).toBe(false);

    fireEvent.change(select, { target: { value: kimiKey } });
    expect(setAgentDefaultModel).toHaveBeenCalledWith("dsh", kimiKey);

    fireEvent.click(screen.getByLabelText("配置 DeepSeek Harness"));
    expect(onOpenMain).toHaveBeenCalledWith({ nav: "agents", agentId: "dsh" });

    fireEvent.click(screen.getByLabelText("解绑 DeepSeek Harness"));
    expect(unbindAgent).not.toHaveBeenCalled();
    fireEvent.click(screen.getByLabelText("确认解绑 DeepSeek Harness"));
    expect(unbindAgent).toHaveBeenCalledWith("dsh");

    fireEvent.click(screen.getByRole("button", { name: "去接入" }));
    expect(onOpenMain).toHaveBeenCalledWith({ nav: "agents", agentId: "codex_desktop" });
  });

  it("全部额度快照均为今日时不在托盘显示过期标记", () => {
    const freshFetchedAt = new Date(Date.now() - 5 * 60_000).toISOString();
    ctx.accounts = INITIAL_ACCOUNTS.map((acc) =>
      acc.latestQuota
        ? { ...acc, latestQuota: { ...acc.latestQuota, fetchedAt: freshFetchedAt } }
        : acc
    );

    render(<TrayPopover onOpenMain={vi.fn()} />);
    expect(screen.queryAllByText("已过期")).toHaveLength(0);
  });

  it("额度快照跨日时在托盘显式标记「已过期」", () => {
    const staleFetchedAt = new Date(Date.now() - 48 * 60 * 60_000).toISOString();
    const stale = {
      ...INITIAL_ACCOUNTS[0],
      latestQuota: {
        ...INITIAL_ACCOUNTS[0].latestQuota!,
        fetchedAt: staleFetchedAt,
      },
    };
    ctx.accounts = [stale, ...INITIAL_ACCOUNTS.slice(1)];

    render(<TrayPopover onOpenMain={vi.fn()} />);
    expect(screen.getAllByText("已过期").length).toBeGreaterThan(0);
  });
});
