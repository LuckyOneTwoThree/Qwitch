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

  it("Qoder 账号缺本机快照时展示「采集本机会话」，并调用采集而非导入", async () => {
    const captureLocalSession = vi.fn().mockResolvedValue(true);
    h.useQwitch.mockReturnValue({
      ...h.useQwitch(),
      captureLocalSession,
      importAccount: vi.fn(),
    });

    const qoderAccount: AccountSummary = {
      ...account,
      id: "acc-qoder-1",
      toolId: "qoder_cn" as never,
      label: "Qoder 账号甲",
      hasLocalSnapshot: false,
      hasApiToken: true,
    };

    render(<AccountCard account={qoderAccount} />);

    // 文案必须区分：Qoder 走「采集」而非「关联」，因为其快照是加密字节
    const btn = screen.getByRole("button", { name: /采集本机会话/ });
    fireEvent.click(btn);

    await Promise.resolve();
    // 关键：Qoder 不得走 importAccount(local_import)——macOS 上无法解密必然失败
    expect(captureLocalSession).toHaveBeenCalledWith("acc-qoder-1");
  });

  it("缺在线凭据时徽章显示「待授权」而非「正常」，并展示授权入口", () => {
    const onOpenBindModal = vi.fn();
    render(
      <AccountCard
        account={{
          ...account,
          hasApiToken: false,
          hasLocalSnapshot: true,
          latestQuota: undefined,
        }}
        onOpenBindModal={onOpenBindModal}
      />
    );

    expect(screen.getByText("待授权")).toBeTruthy();
    expect(screen.queryByText("正常")).toBeNull();
    expect(screen.getByRole("button", { name: /在线授权/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /粘贴凭据/ }));
    expect(onOpenBindModal).toHaveBeenCalledWith(expect.objectContaining({ id: account.id }), "token_paste");
  });

  it("缺本机切号快照时徽章显示「待关联」而非「正常」", () => {
    render(
      <AccountCard
        account={{
          ...account,
          hasApiToken: true,
          hasLocalSnapshot: false,
        }}
      />
    );

    expect(screen.getByText("待关联")).toBeTruthy();
    expect(screen.queryByText("正常")).toBeNull();
  });

  it("健康账号展示凭据续期模式徽章", () => {
    render(
      <AccountCard
        account={{
          ...account,
          renewalMode: "auto_refresh",
          credentialExpiresAt: new Date(Date.now() + 3_600_000).toISOString(),
        }}
      />
    );
    expect(screen.getByText("自动续期")).toBeTruthy();
  });

  it("设置别名后仍保留展示原用户名，并支持一键恢复原用户名", () => {
    render(
      <AccountCard
        account={{
          ...account,
          label: "主力号",
          accountName: "真实用户名",
        }}
      />
    );

    expect(screen.getByText("主力号")).toBeTruthy();
    expect(screen.getByText("别名")).toBeTruthy();
    expect(screen.getByText(/真实用户名/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "修改 主力号 别名" }));
    fireEvent.click(screen.getByRole("button", { name: "恢复原用户名" }));
    expect(ctx.renameAccount).toHaveBeenCalledWith(account.id, "");
  });

  it("不支持桌面切号的工具（如 CodeArts）缺本地快照时不显示「待关联」或「关联本机」", () => {
    render(
      <AccountCard
        account={{
          ...account,
          toolId: "codearts",
          hasApiToken: true,
          hasLocalSnapshot: false,
          renewalMode: "auto_refresh",
        }}
      />
    );

    expect(screen.getByText("正常")).toBeTruthy();
    expect(screen.queryByText("待关联")).toBeNull();
    expect(screen.queryByRole("button", { name: /关联本机/ })).toBeNull();
  });

  it("今日快照显示相对时间且不标记过期", () => {
    render(
      <AccountCard
        account={{
          ...account,
          latestQuota: {
            ...account.latestQuota!,
            fetchedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
          },
        }}
      />
    );

    expect(screen.getByText("5 分钟前")).toBeTruthy();
    expect(screen.queryByText("· 已过期")).toBeNull();
  });

  it("跨日快照显式标记「已过期」，避免昨日额度被误读为当前水位", () => {
    render(
      <AccountCard
        account={{
          ...account,
          latestQuota: {
            ...account.latestQuota!,
            // 前天快照：必然不属于今日 CST 自然日
            fetchedAt: new Date(Date.now() - 48 * 60 * 60_000).toISOString(),
          },
        }}
      />
    );

    expect(screen.getByText("· 已过期")).toBeTruthy();
    expect(
      screen.getByTitle(/该额度快照不是今天的同步结果/)
    ).toBeTruthy();
  });

  it("从未查额的账号显示「尚未查额」而非具体时刻", () => {
    render(<AccountCard account={{ ...account, latestQuota: undefined }} />);
    expect(screen.getByText("尚未查额")).toBeTruthy();
  });

  it("已停用账号禁用「切至桌面端」与「查额」并给出明确原因提示", () => {
    render(
      <AccountCard
        account={{
          ...account,
          status: "disabled",
          isActiveOnClient: false,
        }}
      />
    );
    const switchBtn = screen.getByRole("button", { name: /切至桌面端/ }) as HTMLButtonElement;
    const queryBtn = screen.getByRole("button", { name: /查额/ }) as HTMLButtonElement;
    expect(switchBtn.disabled).toBe(true);
    expect(switchBtn.title).toContain("已停用");
    expect(queryBtn.disabled).toBe(true);
    expect(queryBtn.title).toContain("已停用");
  });

  it("非桌面在登的豆包账号只要具备本机快照即可直接点击「领取」触发自动冷切", () => {
    const dwAcc: AccountSummary = {
      ...account,
      id: "acc-dw-snap",
      toolId: "doubaowork",
      label: "豆包非在登有快照",
      isActiveOnClient: false,
      hasLocalSnapshot: true,
    };
    render(<AccountCard account={dwAcc} />);
    const claimBtn = screen.getByRole("button", { name: /领取/ }) as HTMLButtonElement;
    expect(claimBtn.disabled).toBe(false);
    expect(claimBtn.title).toContain("自动切换桌面端");
    fireEvent.click(claimBtn);
    expect(ctx.claimDailyBatch).toHaveBeenCalledWith(["acc-dw-snap"]);
  });

  it("非桌面在登的豆包账号若缺少本机快照则禁用「领取」并提示补快照", () => {
    const dwNoSnap: AccountSummary = {
      ...account,
      id: "acc-dw-nosnap",
      toolId: "doubaowork",
      label: "豆包缺快照",
      isActiveOnClient: false,
      hasLocalSnapshot: false,
    };
    render(<AccountCard account={dwNoSnap} />);
    const claimBtn = screen.getByRole("button", { name: /领取/ }) as HTMLButtonElement;
    expect(claimBtn.disabled).toBe(true);
    expect(claimBtn.title).toContain("补快照");
  });
});
