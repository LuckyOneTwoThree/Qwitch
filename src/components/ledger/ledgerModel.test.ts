import { describe, expect, it } from "vitest";
import type { AccountSummary, QuotaBucket } from "../../types/qwitch";
import {
  claimAvailability,
  riskBand,
  sortLedgerAccounts,
  toQuotaView,
  toStatusView,
  waterTone,
  LEDGER_BAND,
} from "./ledgerModel";

function account(over: Partial<AccountSummary> = {}): AccountSummary {
  const base: AccountSummary = {
    id: "acc-1",
    toolId: "trae_cn",
    label: "主力号",
    uidMasked: "u_0000",
    status: "active",
    isActiveOnClient: false,
    updatedAt: "2026-11-27T08:00:00.000Z",
    hasApiToken: true,
    hasLocalSnapshot: true,
    latestQuota: {
      claimStatus: "available",
      fetchedAt: "2026-11-27T08:00:00.000Z",
      buckets: [
        {
          key: "daily",
          label: "快速请求额度",
          used: 220,
          total: 600,
          remaining: 380,
          unit: "次",
        },
      ],
    },
  };
  // 显式传 undefined 表示「没有快照」，因此必须后展开覆盖默认值
  return { ...base, ...over };
}

function bucket(over: Partial<QuotaBucket> = {}): QuotaBucket {
  return {
    key: "daily",
    label: "额度",
    used: 0,
    total: 100,
    remaining: 100,
    unit: "次",
    ...over,
  };
}

describe("toQuotaView：五类异构额度必须区分对待", () => {
  it("有可信总量时给出百分比", () => {
    const view = toQuotaView(
      account({
        latestQuota: {
          claimStatus: "available",
          fetchedAt: "2026-11-27T08:00:00.000Z",
          buckets: [bucket({ total: 600, remaining: 380 })],
        },
      })
    );
    expect(view.kind).toBe("ratio");
    expect(view.pct).toBe(63);
    expect(view.extraBuckets).toBe(0);
  });

  it("百分比桶不再伪造总量", () => {
    const view = toQuotaView(
      account({
        latestQuota: {
          claimStatus: "available",
          fetchedAt: "2026-11-27T08:00:00.000Z",
          buckets: [bucket({ unit: "%", total: 100, remaining: 42 })],
        },
      })
    );
    expect(view.kind).toBe("percent");
    expect(view.pct).toBe(42);
  });

  it("hasTotal === false 的纯余额桶不产出百分比", () => {
    const view = toQuotaView(
      account({
        latestQuota: {
          claimStatus: "available",
          fetchedAt: "2026-11-27T08:00:00.000Z",
          buckets: [bucket({ unit: "积分", remaining: 361, total: 661, hasTotal: false })],
        },
      })
    );
    expect(view.kind).toBe("balance");
    expect(view.pct).toBeNull();
    expect(view.remaining).toBe(361);
  });

  it("Qoder 的 daily_credits 桶按签到计，不参与百分比排序", () => {
    const view = toQuotaView(
      account({
        toolId: "qoder_cn",
        latestQuota: {
          claimStatus: "claimed",
          fetchedAt: "2026-11-27T08:00:00.000Z",
          buckets: [bucket({ key: "daily_credits", total: 100, remaining: 100 })],
        },
      })
    );
    expect(view.kind).toBe("claim");
    expect(view.pct).toBeNull();
  });

  it("没有快照时返回 empty，而不是 0%", () => {
    expect(toQuotaView(account({ latestQuota: undefined })).kind).toBe("empty");
  });

  it("副桶数量以 +N 暴露，不丢失信息", () => {
    const view = toQuotaView(
      account({
        latestQuota: {
          claimStatus: "available",
          fetchedAt: "2026-11-27T08:00:00.000Z",
          buckets: [bucket(), bucket({ key: "adv", remaining: 12 })],
        },
      })
    );
    expect(view.extraBuckets).toBe(1);
  });
});

describe("riskBand：分带而非单一百分比", () => {
  it("过期/风控/待授权归入阻断带", () => {
    expect(riskBand(account({ status: "expired" }))).toBe(LEDGER_BAND.blocked);
    expect(riskBand(account({ status: "risk_paused" }))).toBe(LEDGER_BAND.blocked);
    expect(riskBand(account({ hasApiToken: false }))).toBe(LEDGER_BAND.blocked);
  });

  it("按百分比落到危急/偏低/健康三带", () => {
    const quota = (remaining: number) => ({
      claimStatus: "available" as const,
      fetchedAt: "2026-11-27T08:00:00.000Z",
      buckets: [bucket({ total: 100, remaining })],
    });
    expect(riskBand(account({ latestQuota: quota(10) }))).toBe(LEDGER_BAND.critical);
    expect(riskBand(account({ latestQuota: quota(30) }))).toBe(LEDGER_BAND.low);
    expect(riskBand(account({ latestQuota: quota(80) }))).toBe(LEDGER_BAND.healthy);
  });

  it("无总量 / 未查额归入不可排序带，而不是被当成 0%", () => {
    expect(riskBand(account({ latestQuota: undefined }))).toBe(LEDGER_BAND.unknown);
    expect(
      riskBand(
        account({
          latestQuota: {
            claimStatus: "available",
            fetchedAt: "2026-11-27T08:00:00.000Z",
            buckets: [bucket({ hasTotal: false, remaining: 361 })],
          },
        })
      )
    ).toBe(LEDGER_BAND.unknown);
  });

  it("已停用排在最后，不参与阻断带", () => {
    expect(riskBand(account({ status: "disabled" }))).toBe(LEDGER_BAND.disabled);
  });
});

describe("sortLedgerAccounts", () => {
  const blocked = account({ id: "a-blocked", status: "expired", label: "过期号" });
  const critical = account({
    id: "b-critical",
    label: "危急号",
    latestQuota: {
      claimStatus: "available",
      fetchedAt: "2026-11-27T08:00:00.000Z",
      buckets: [bucket({ total: 100, remaining: 5 })],
    },
  });
  const healthy = account({
    id: "c-healthy",
    label: "健康号",
    latestQuota: {
      claimStatus: "available",
      fetchedAt: "2026-11-27T08:00:00.000Z",
      buckets: [bucket({ total: 100, remaining: 90 })],
    },
  });
  const unknown = account({ id: "d-unknown", label: "未查额号", latestQuota: undefined });

  it("默认排序先阻断、再按水位、不可排序垫底", () => {
    const sorted = sortLedgerAccounts([healthy, unknown, critical, blocked], "risk");
    expect(sorted.map((a) => a.id)).toEqual([
      "a-blocked",
      "b-critical",
      "c-healthy",
      "d-unknown",
    ]);
  });

  it("水位升序把没有百分比的排在最后，而不是当成 0", () => {
    const sorted = sortLedgerAccounts([unknown, healthy, critical], "quota-asc");
    expect(sorted.map((a) => a.id)).toEqual(["b-critical", "c-healthy", "d-unknown"]);
  });

  it("同名时以 id 兜底，保证排序稳定不跳动", () => {
    const a = account({ id: "x1", label: "同名" });
    const b = account({ id: "x2", label: "同名" });
    expect(sortLedgerAccounts([b, a], "name").map((x) => x.id)).toEqual(["x1", "x2"]);
  });
});

describe("状态三轴互不覆盖", () => {
  it("本机在用与可用性并行表达", () => {
    const view = toStatusView(account({ status: "active", isActiveOnClient: true }));
    expect(view.label).toBe("正常");
    expect(view.mounted).toBe(true);
  });

  it("在用但凭据过期时，状态仍报过期", () => {
    const view = toStatusView(account({ status: "expired", isActiveOnClient: true }));
    expect(view.label).toBe("凭据过期");
    expect(view.mounted).toBe(true);
  });

  it("缺本机快照且适配器支持切号时提示待关联", () => {
    const view = toStatusView(account({ hasLocalSnapshot: false }), [
      {
        id: "trae_cn",
        name: "Trae CN",
        version: "1",
        isBuiltin: true,
        capabilities: {
          authMethods: ["webview_oauth"],
          supportsQuotaQuery: true,
          supportsDailyClaim: true,
          supportsClientSwitch: true,
          supportsMultiInstance: false,
        },
      },
    ]);
    expect(view.label).toBe("待关联");
  });
});

describe("claimAvailability", () => {
  const adapters = [
    {
      id: "trae_cn",
      name: "Trae CN",
      version: "1",
      isBuiltin: true,
      capabilities: {
        authMethods: ["webview_oauth" as const],
        supportsQuotaQuery: true,
        supportsDailyClaim: true,
        supportsClientSwitch: true,
        supportsMultiInstance: false,
      },
    },
  ];

  it("可领取时返回 ready", () => {
    expect(claimAvailability(account(), adapters)).toBe("ready");
  });

  it("今日已领返回 done，不再显示领取按钮", () => {
    const claimed = account({ lastClaimedAt: new Date().toISOString() });
    expect(claimAvailability(claimed, adapters)).toBe("done");
  });

  it("停用账号不提供领取入口", () => {
    expect(claimAvailability(account({ status: "disabled" }), adapters)).toBe("off");
  });
});

describe("waterTone 与卡片阈值一致", () => {
  it("15/35 是分界点", () => {
    expect(waterTone(15)).toBe("critical");
    expect(waterTone(16)).toBe("low");
    expect(waterTone(35)).toBe("low");
    expect(waterTone(36)).toBe("ok");
    expect(waterTone(null)).toBe("none");
  });
});
