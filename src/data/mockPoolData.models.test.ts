// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  buildLiveModelLabels,
  computeAgentExposedModels,
  getCatalogModelsForTool,
} from "./mockPoolData";
import type { AccountPool, UpstreamAccountCandidate } from "../types/pool";

function makePool(): AccountPool {
  return {
    id: "pool-1",
    slug: "pool-1",
    name: "主力池",
    description: "",
    sceneTag: "",
    strategy: "order",
    affinityMode: "auto_cache",
    skipActiveClientAccount: false,
    cooldownSeconds: 60,
    autoRefreshOn401: true,
    fallbackApiKeyEnabled: false,
    enabled: true,
    todayRequests: 0,
    todayFailovers: 0,
    cacheHitRate: 0,
    createdAt: "2026-01-01T00:00:00Z",
    modelPolicies: [{ modelId: "trae_cn/kimi-k3", enabled: true }],
    members: [
      {
        id: "pm-1",
        accountId: "acc-1",
        toolId: "trae_cn",
        label: "Trae 账号",
        uidMasked: "1***2",
        supportedModels: ["trae_cn/kimi-k3"],
        tier: "t1_primary",
        channel: "rust_native",
        enabled: true,
        priority: 1,
        quotaPercent: 80,
        quotaText: "80%",
        accountStatus: "active",
      },
    ],
  } as unknown as AccountPool;
}

const candidate: UpstreamAccountCandidate = {
  accountId: "acc-1",
  toolId: "trae_cn",
  label: "Trae 账号",
  uidMasked: "1***2",
  channel: "rust_native",
  defaultModels: ["kimi-k3"],
  modelOptions: [
    {
      id: "kimi-k3",
      displayName: "Kimi K3（实时）",
      vendorWireId: "Kimi-K3",
      family: "Kimi",
      source: "live",
    },
  ],
  defaultTier: "t1_primary",
  quotaPercent: 80,
  quotaText: "80%",
  accountStatus: "active",
};

describe("模型目录：实时探测结果优先", () => {
  it("buildLiveModelLabels 按 工具/模型ID 聚合并优先实时来源", () => {
    const labels = buildLiveModelLabels([candidate]);
    expect(labels.get("trae_cn/kimi-k3")).toBe("Kimi K3（实时）");
  });

  it("getCatalogModelsForTool 合并实时候选，展示名与 wire id 取实时值", () => {
    const merged = getCatalogModelsForTool("trae_cn", [candidate]);
    const kimi = merged.find((m) => m.id === "kimi-k3");
    expect(kimi?.displayName).toBe("Kimi K3（实时）");
    expect(kimi?.vendorWireId).toBe("Kimi-K3");
    expect(kimi?.source).toBe("live");
  });

  it("computeAgentExposedModels 展示名优先使用实时探测名", () => {
    const pool = makePool();
    const labels = buildLiveModelLabels([candidate]);

    const withLive = computeAgentExposedModels(
      { agentId: "opencode", mountedPoolIds: ["pool-1"], defaultModelKey: null },
      [pool],
      Date.now(),
      labels
    );
    expect(withLive[0]?.displayName).toContain("Kimi K3（实时）");

    // 无实时数据时回落到静态目录展示名
    const withoutLive = computeAgentExposedModels(
      { agentId: "opencode", mountedPoolIds: ["pool-1"], defaultModelKey: null },
      [pool],
      Date.now()
    );
    expect(withoutLive[0]?.displayName).not.toContain("（实时）");
  });
});
