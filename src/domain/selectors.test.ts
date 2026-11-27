import { describe, expect, it } from "vitest";
import type { AccountSummary, AdapterManifest } from "../types/qwitch";
import {
  getEffectiveClaimStatus,
  isAccountClaimable,
  isAccountHealthyReady,
  isAccountNeedingAttention,
  isSameCstDay,
  selectClaimCandidates,
  selectScopedStats,
  sortAccountsForFlatView,
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
    for (const toolId of [
      "workbuddy",
      "minimax_code_cn",
      "lobsterai",
      "doubaowork",
    ] as const) {
      const acc = makeAcc({
        toolId,
        lastClaimedAt: NOW_ISO,
        latestQuota: { claimStatus: "available", buckets: [], fetchedAt: NOW_ISO },
      });
      expect(getEffectiveClaimStatus(acc)).toBe("claimed");
    }
  });

  it("claimed 快照若 lastClaimedAt 为今日即使 fetchedAt 陈旧仍保持 claimed", () => {
    const acc = makeAcc({
      toolId: "qoder_cn",
      lastClaimedAt: NOW_ISO,
      latestQuota: { claimStatus: "claimed", buckets: [], fetchedAt: STALE_ISO },
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

  it("缺本机快照的冷切账号不计入候选（避免批量切换失败）", () => {
    const accounts = [
      makeAcc({ id: "a", latestQuota: available }),
      makeAcc({ id: "b", hasLocalSnapshot: false, latestQuota: available }),
    ];
    const candidates = selectClaimCandidates(accounts, adapters);
    expect(candidates.map((c) => c.id)).toEqual(["a"]);
    // 定向点击也不应把缺快照账号放进批量队列
    const targeted = selectClaimCandidates(accounts, adapters, { targetIds: ["b"] });
    expect(targeted.map((c) => c.id)).toEqual([]);
  });
});

describe("selectScopedStats 缺快照统计", () => {
  const adapters = [makeAdapter("doubaowork"), makeAdapter("trae_cn")];
  it("缺快照账号归入 unclaimedSnapshotMissingCount 而非待领", () => {
    const accounts = [
      makeAcc({
        id: "a",
        toolId: "doubaowork",
        hasLocalSnapshot: false,
        latestQuota: { claimStatus: "available", buckets: [], fetchedAt: NOW_ISO },
      }),
    ];
    const stats = selectScopedStats(accounts, adapters, {});
    expect(stats.unclaimedSnapshotMissingCount).toBe(1);
    expect(stats.claimableCount).toBe(0);
    expect(stats.claimBadgeText).toBe("1 待补快照");
  });
});
describe("sortAccountsForFlatView", () => {
  const adapters = [
    makeAdapter("trae_cn"),
    makeAdapter("workbuddy"),
    makeAdapter("qoder_cn"),
    makeAdapter("minimax_code_cn"),
    makeAdapter("lobsterai"),
    makeAdapter("doubaowork"),
  ];

  it("按「待处理 → 可领取 → 挂载中 → 其余」分层", () => {
    const normal = makeAcc({
      id: "n",
      toolId: "trae_cn",
      label: "正常",
      latestQuota: { claimStatus: "claimed", fetchedAt: NOW_ISO, buckets: [] },
    });
    const active = makeAcc({
      id: "a",
      toolId: "trae_cn",
      label: "挂载中",
      isActiveOnClient: true,
      latestQuota: { claimStatus: "claimed", fetchedAt: NOW_ISO, buckets: [] },
    });
    const claimable = makeAcc({
      id: "c",
      toolId: "trae_cn",
      label: "可领取",
      latestQuota: { claimStatus: "available", fetchedAt: NOW_ISO, buckets: [] },
    });
    const attention = makeAcc({
      id: "x",
      toolId: "trae_cn",
      label: "已过期",
      status: "expired",
    });

    const sorted = sortAccountsForFlatView(
      [normal, active, claimable, attention],
      adapters
    );

    expect(sorted.map((a) => a.id)).toEqual(["x", "c", "a", "n"]);
  });

  it("层内按工具顺序排列，与分组视图的 BUILTIN_TOOL_ORDER 一致", () => {
    // 全部为 normal 层，验证工具顺序为 trae → workbuddy → qoder → minimax → lobsterai → doubaowork
    const accounts = [
      makeAcc({ id: "dw", toolId: "doubaowork", label: "豆包工作" }),
      makeAcc({ id: "lb", toolId: "lobsterai", label: "LobsterAI" }),
      makeAcc({ id: "mm", toolId: "minimax_code_cn", label: "MiniMax" }),
      makeAcc({ id: "qd", toolId: "qoder_cn", label: "Qoder" }),
      makeAcc({ id: "tr", toolId: "trae_cn", label: "Trae" }),
      makeAcc({ id: "wb", toolId: "workbuddy", label: "WorkBuddy" }),
    ];

    const sorted = sortAccountsForFlatView(accounts, adapters);

    expect(sorted.map((a) => a.toolId)).toEqual([
      "trae_cn",
      "workbuddy",
      "qoder_cn",
      "minimax_code_cn",
      "lobsterai",
      "doubaowork",
    ]);
  });

  it("同层同工具内保持传入顺序（稳定排序，避免刷新时卡片跳动）", () => {
    const first = makeAcc({ id: "1", toolId: "trae_cn", label: "先导入" });
    const second = makeAcc({ id: "2", toolId: "trae_cn", label: "后导入" });
    const third = makeAcc({ id: "3", toolId: "trae_cn", label: "最后" });

    const sorted = sortAccountsForFlatView([first, second, third], adapters);
    expect(sorted.map((a) => a.id)).toEqual(["1", "2", "3"]);

    // 顺序颠倒输入，仍按传入顺序输出（不按 label 重排）
    const sorted2 = sortAccountsForFlatView([third, first, second], adapters);
    expect(sorted2.map((a) => a.id)).toEqual(["3", "1", "2"]);
  });

  it("已停用账号不进入「待处理」层（用户主动关闭，无需提醒）", () => {
    const disabled = makeAcc({
      id: "d",
      toolId: "trae_cn",
      label: "已停用",
      status: "disabled",
    });
    const attention = makeAcc({
      id: "x",
      toolId: "trae_cn",
      label: "风控",
      status: "risk_paused",
    });

    const sorted = sortAccountsForFlatView([disabled, attention], adapters);
    // 风控排最前；停用归入 normal 层
    expect(sorted.map((a) => a.id)).toEqual(["x", "d"]);
  });

  it("未完成授权（hasApiToken=false）归入待处理层", () => {
    const noToken = makeAcc({
      id: "nt",
      toolId: "trae_cn",
      label: "待授权",
      hasApiToken: false,
    });
    const normal = makeAcc({ id: "n", toolId: "trae_cn", label: "正常" });

    const sorted = sortAccountsForFlatView([normal, noToken], adapters);
    expect(sorted.map((a) => a.id)).toEqual(["nt", "n"]);
  });

  it("未知工具（外部插件未加载）排在已知工具之后但不丢失", () => {
    const unknown = makeAcc({ id: "u", toolId: "ext_plugin" as never, label: "外部" });
    const known = makeAcc({ id: "k", toolId: "trae_cn", label: "内置" });

    const sorted = sortAccountsForFlatView([unknown, known], adapters);
    expect(sorted.map((a) => a.id)).toEqual(["k", "u"]);
  });

  it("空输入返回空数组且不抛错", () => {
    expect(sortAccountsForFlatView([], adapters)).toEqual([]);
  });

  it("不支持切号的工具（如 CodeArts）缺本地快照时不计入待处理", () => {
    const codeartsAcc = makeAcc({
      id: "ca-1",
      toolId: "codearts",
      label: "CodeArts",
      hasApiToken: true,
      hasLocalSnapshot: false,
    });
    expect(isAccountNeedingAttention(codeartsAcc, adapters)).toBe(false);
    expect(isAccountHealthyReady(codeartsAcc, adapters)).toBe(true);
  });
});
