import { describe, expect, it } from "vitest";
import type { AccountSummary, AdapterManifest } from "../types/qwitch";
import {
  getEffectiveClaimStatus,
  isAccountClaimable,
  isSameCstDay,
  selectClaimCandidates,
  selectScopedStats,
} from "./selectors";

const NOW_ISO = new Date().toISOString();
const STALE_ISO = "2020-01-01T00:00:00Z";

function makeAcc(partial: Partial<AccountSummary> = {}): AccountSummary {
  return {
    id: "acc-1",
    toolId: "trae_cn",
    label: "测试账号",
    uidMasked: "u_12****ab",
    status: "active",
    isActiveOnClient: false,
    hasLocalSnapshot: true,
    hasApiToken: true,
    updatedAt: NOW_ISO,
    ...partial,
  };
}

function makeAdapter(id: string, supportsDailyClaim = true): AdapterManifest {
  return {
    id,
    name: id,
    version: "1.0.0",
    isBuiltin: true,
    capabilities: {
      authMethods: ["local_import"],
      supportsQuotaQuery: true,
      supportsDailyClaim,
      supportsClientSwitch: true,
      supportsMultiInstance: false,
    },
  };
}

describe("getEffectiveClaimStatus", () => {
  it("每日重置型工具今日已领时覆盖服务端缓存的 Available", () => {
    const acc = makeAcc({
      toolId: "workbuddy",
      lastClaimedAt: NOW_ISO,
      latestQuota: { claimStatus: "available", buckets: [], fetchedAt: NOW_ISO },
    });
    expect(getEffectiveClaimStatus(acc)).toBe("claimed");
  });

  it("Qoder 活动按周期：今日已领不覆盖新出现的 Available", () => {
    const acc = makeAcc({
      toolId: "qoder_cn",
      lastClaimedAt: NOW_ISO,
      latestQuota: { claimStatus: "available", buckets: [], fetchedAt: NOW_ISO },
    });
    expect(getEffectiveClaimStatus(acc)).toBe("available");
  });

  it("陈旧快照的 claimed/available 退化为 unknown", () => {
    expect(
      getEffectiveClaimStatus(
        makeAcc({ latestQuota: { claimStatus: "claimed", buckets: [], fetchedAt: STALE_ISO } })
      )
    ).toBe("unknown");
    expect(
      getEffectiveClaimStatus(
        makeAcc({ latestQuota: { claimStatus: "available", buckets: [], fetchedAt: STALE_ISO } })
      )
    ).toBe("unknown");
  });

  it("unsupported 直通，无快照为 unknown", () => {
    expect(
      getEffectiveClaimStatus(
        makeAcc({ latestQuota: { claimStatus: "unsupported", buckets: [], fetchedAt: NOW_ISO } })
      )
    ).toBe("unsupported");
    expect(getEffectiveClaimStatus(makeAcc())).toBe("unknown");
  });

  it("CST 日界判断", () => {
    expect(isSameCstDay(NOW_ISO)).toBe(true);
    expect(isSameCstDay(STALE_ISO)).toBe(false);
    expect(isSameCstDay(undefined)).toBe(false);
    expect(isSameCstDay("not-a-date")).toBe(false);
  });
});

describe("isAccountClaimable", () => {
  it("适配器未加载时不计入可领（与卡片口径一致）", () => {
    const acc = makeAcc({
      latestQuota: { claimStatus: "available", buckets: [], fetchedAt: NOW_ISO },
    });
    expect(isAccountClaimable(acc, [])).toBe(false);
    expect(isAccountClaimable(acc, [makeAdapter("trae_cn")])).toBe(true);
  });

  it("停用/缺 token/不支持签到均不可领", () => {
    const quota = { claimStatus: "available" as const, buckets: [], fetchedAt: NOW_ISO };
    const adapters = [makeAdapter("trae_cn")];
    expect(isAccountClaimable(makeAcc({ status: "disabled", latestQuota: quota }), adapters)).toBe(false);
    expect(isAccountClaimable(makeAcc({ hasApiToken: false, latestQuota: quota }), adapters)).toBe(false);
    expect(
      isAccountClaimable(makeAcc({ latestQuota: quota }), [makeAdapter("trae_cn", false)])
    ).toBe(false);
  });
});

describe("selectScopedStats", () => {
  const adapters = [makeAdapter("trae_cn"), makeAdapter("qoder_cn")];
  const quotaAvailable = { claimStatus: "available" as const, buckets: [], fetchedAt: NOW_ISO };
  const quotaClaimed = { claimStatus: "claimed" as const, buckets: [], fetchedAt: NOW_ISO };

  it("健康/需关注/停用互斥闭合", () => {
    const accounts = [
      makeAcc({ id: "a", latestQuota: quotaAvailable }),
      makeAcc({ id: "b", status: "expired" }),
      makeAcc({ id: "c", status: "disabled" }),
      makeAcc({ id: "d", hasApiToken: false }),
    ];
    const stats = selectScopedStats(accounts, adapters, {});
    expect(stats.healthyCount + stats.attentionCount + stats.disabledCount).toBe(stats.total);
    expect(stats.healthyCount).toBe(1);
    expect(stats.attentionCount).toBe(2);
    expect(stats.disabledCount).toBe(1);
  });

  it("存在待核实账号时不误报已全签", () => {
    const accounts = [
      makeAcc({ id: "a", latestQuota: quotaClaimed }),
      makeAcc({ id: "b" }),
    ];
    const stats = selectScopedStats(accounts, adapters, {});
    expect(stats.allClaimed).toBe(false);
    expect(stats.claimActionState).toBe("verify_and_claim");
    expect(stats.claimBadgeText).toContain("待核实");
  });

  it("全部已领时正确报已全签", () => {
    const accounts = [
      makeAcc({ id: "a", latestQuota: quotaClaimed }),
      makeAcc({ id: "b", toolId: "qoder_cn", latestQuota: quotaClaimed }),
    ];
    const stats = selectScopedStats(accounts, adapters, {});
    expect(stats.allClaimed).toBe(true);
    expect(stats.claimBadgeText).toBe("已全签");
  });
});

describe("selectClaimCandidates", () => {
  const adapters = [makeAdapter("trae_cn"), makeAdapter("qoder_cn")];
  const available = { claimStatus: "available" as const, buckets: [], fetchedAt: NOW_ISO };

  it("默认仅纳入 available（自动巡检口径）", () => {
    const accounts = [
      makeAcc({ id: "a", latestQuota: available }),
      makeAcc({ id: "b" }),
    ];
    const candidates = selectClaimCandidates(accounts, adapters);
    expect(candidates.map((c) => c.id)).toEqual(["a"]);
  });

  it("核实并领取模式纳入 unknown（按钮实际行为回归）", () => {
    const accounts = [
      makeAcc({ id: "a", latestQuota: available }),
      makeAcc({ id: "b" }),
      makeAcc({ id: "c", latestQuota: { claimStatus: "claimed", buckets: [], fetchedAt: NOW_ISO } }),
    ];
    const candidates = selectClaimCandidates(accounts, adapters, { includeUnknown: true });
    expect(candidates.map((c) => c.id)).toEqual(["a", "b"]);
  });

  it("定向 targetIds 绕过状态过滤但仍要求 active+token+能力", () => {
    const accounts = [
      makeAcc({ id: "a", status: "disabled", latestQuota: available }),
      makeAcc({ id: "b", hasApiToken: false }),
      makeAcc({ id: "c" }),
    ];
    const candidates = selectClaimCandidates(accounts, adapters, { targetIds: ["a", "b", "c"] });
    expect(candidates.map((c) => c.id)).toEqual(["c"]);
  });

  it("工具作用域过滤", () => {
    const accounts = [
      makeAcc({ id: "a", latestQuota: available }),
      makeAcc({ id: "b", toolId: "qoder_cn", latestQuota: available }),
    ];
    const candidates = selectClaimCandidates(accounts, adapters, { toolId: "qoder_cn" });
    expect(candidates.map((c) => c.id)).toEqual(["b"]);
  });
});