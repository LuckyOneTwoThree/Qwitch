// @vitest-environment jsdom
import React from "react";
import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  PoolPrototypeProvider,
  usePoolPrototype,
} from "./usePoolPrototypeStore";
import { makeDefaultModelKey } from "../data/mockPoolData";

function createWrapper(onToast = vi.fn()) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return <PoolPrototypeProvider onToast={onToast}>{children}</PoolPrototypeProvider>;
  };
}

describe("usePoolPrototypeStore v2", () => {
  it("creates multi-model pools, enforces API Key T3 tier lock, mounts M:N agents, and cleans up on delete", () => {
    const onToast = vi.fn();
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(onToast),
    });

    const initialLen = result.current.pools.length;
    const candidateWb = result.current.candidates.find((c) => c.accountId === "acc-wb-1")!;
    const candidateApiKey = result.current.candidates.find(
      (c) => c.accountId === "acc-apikey-deepseek"
    )!;

    let createdId = "";
    act(() => {
      const created = result.current.createPool({
        name: "多模型专项池",
        description: "验证多选模型与梯队",
        sceneTag: "测试",
        strategy: "order",
        affinityMode: "auto_cache",
        skipActiveClientAccount: false,
        cooldownSeconds: 60,
        autoRefreshOn401: true,
        fallbackApiKeyEnabled: true,
        members: [
          {
            candidate: candidateWb,
            supportedModels: ["claude-sonnet-4.6", "claude-opus-4.6"],
            tier: "t1_primary",
          },
          {
            candidate: candidateApiKey,
            supportedModels: ["claude-sonnet-4.6", "deepseek-v4"],
            tier: "t1_primary", // 应被强制锁回 t3_fallback
          },
        ],
      });
      createdId = created.id;
    });

    const createdPool = result.current.pools.find((p) => p.id === createdId)!;
    expect(result.current.pools.length).toBe(initialLen + 1);
    expect(createdPool.modelPolicies.map((m) => m.modelId)).toEqual(
      expect.arrayContaining(["claude-sonnet-4.6", "claude-opus-4.6", "deepseek-v4"])
    );
    expect(
      createdPool.members.find((m) => m.accountId === "acc-apikey-deepseek")?.tier
    ).toBe("t3_fallback");

    // 给已有成员切换支持模型（保留至少 1 个模型）
    const wbMemberId = createdPool.members[0].id;
    act(() => {
      result.current.togglePoolMemberModel(createdId, wbMemberId, "deepseek-v4");
    });
    expect(
      result.current.pools
        .find((p) => p.id === createdId)!
        .members.find((m) => m.id === wbMemberId)?.supportedModels
    ).toContain("deepseek-v4");

    // 多号池同时挂载至 Claude（保留原有挂载池）
    const claudeBeforeCount = result.current.agentBindings.find(
      (b) => b.agentId === "claude_desktop"
    )!.mountedPoolIds.length;
    act(() => {
      result.current.mountPoolToAgents(createdId, ["claude_desktop", "codex_desktop"]);
    });

    const claudeAfter = result.current.agentBindings.find(
      (b) => b.agentId === "claude_desktop"
    )!;
    expect(claudeAfter.mountedPoolIds.length).toBe(claudeBeforeCount + 1);
    expect(claudeAfter.mountedPoolIds).toContain(createdId);

    // 切换 Claude 默认模型
    const nextDefault = makeDefaultModelKey(createdId, "claude-opus-4.6");
    act(() => {
      result.current.setAgentDefaultModel("claude_desktop", nextDefault);
    });
    expect(
      result.current.agentBindings.find((b) => b.agentId === "claude_desktop")
        ?.defaultModelKey
    ).toBe(nextDefault);

    // 删除号池：Codex 归零解绑，Claude 保留其他已有号池并自动重新收敛默认模型
    act(() => {
      result.current.deletePool(createdId);
    });
    expect(result.current.pools.length).toBe(initialLen);
    expect(
      result.current.agentBindings.find((b) => b.agentId === "codex_desktop")
        ?.mountedPoolIds
    ).toEqual([]);
    expect(
      result.current.agentBindings.find((b) => b.agentId === "codex_desktop")?.status
    ).toBe("unbound");
    const claudeFinal = result.current.agentBindings.find(
      (b) => b.agentId === "claude_desktop"
    )!;
    expect(claudeFinal.mountedPoolIds).not.toContain(createdId);
    expect(claudeFinal.mountedPoolIds.length).toBe(claudeBeforeCount);
    expect(claudeFinal.defaultModelKey).not.toBe(nextDefault);
  });

  it("merges new models onto existing account members and guards client_readonly CodeArts", () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    const poolId = result.current.pools[0].id;
    const beforeLen = result.current.pools[0].members.length;
    const wbPrimary = result.current.candidates.find((c) => c.accountId === "acc-wb-1")!;
    const readonlyCodeArts = result.current.candidates.find(
      (c) => c.accountId === "acc-codearts-1"
    )!;

    // 尝试加入 client_readonly 应被拦截
    act(() => {
      result.current.addMembersToPool(poolId, [
        { candidate: readonlyCodeArts, supportedModels: ["pangu-coder-pro"], tier: "t2_backup" },
      ]);
    });
    expect(result.current.pools[0].members.length).toBe(beforeLen);

    // 为已在池的 WorkBuddy 主号追加 glm-5.3 模型（合并槽位而不是重复行）
    act(() => {
      result.current.addMembersToPool(poolId, [
        { candidate: wbPrimary, supportedModels: ["glm-5.3"], tier: "t1_primary" },
      ]);
    });
    expect(result.current.pools[0].members.length).toBe(beforeLen);
    expect(
      result.current.pools[0].members.find((m) => m.accountId === "acc-wb-1")
        ?.supportedModels
    ).toContain("glm-5.3");

    // 签发独立 OAuth 后 CodeArts 可安全入池
    act(() => {
      result.current.authorizeCodeArtsIsolated("acc-codearts-1");
    });
    const unlocked = result.current.candidates.find(
      (c) => c.accountId === "acc-codearts-1"
    )!;
    act(() => {
      result.current.addMembersToPool(poolId, [
        { candidate: unlocked, supportedModels: ["pangu-coder-pro"], tier: "t2_backup" },
      ]);
    });
    expect(result.current.pools[0].members.length).toBe(beforeLen + 1);
  });

  it("enforces strict model lock, intra-tool quota ordering, 429 cooldown failover, and session affinity", () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    // 请求主力池的 claude-opus-4.6（仅 T1 WorkBuddy×2 支持；WorkBuddy 主号 73% 优先于 副号 62%）
    let cooledId = "";
    act(() => {
      const sim1 = result.current.runPoolSimulation({
        agentId: "claude_desktop",
        poolId: "pool-coding-main",
        modelId: "claude-opus-4.6",
        prompt: "测试严格锁模 + 429 Failover",
        failureMode: "429_first",
        sessionKey: "session-alpha",
      });
      cooledId = sim1.cooledMemberId ?? "";
      expect(sim1.trace.status).toBe("failover_success");
      expect(sim1.trace.requestedModel).toBe("claude-opus-4.6");
      expect(sim1.trace.finalModel).toBe("claude-opus-4.6");
      expect(sim1.trace.attempts[0].accountLabel).toContain("WorkBuddy · 主号");
      expect(sim1.trace.attempts[1].accountLabel).toContain("WorkBuddy · 副号");
    });

    // 同会话连续发下一条请求：主号还在冷却，自动由 2 号通过 Prompt Cache 亲和直接命中
    act(() => {
      const sim2 = result.current.runPoolSimulation({
        agentId: "claude_desktop",
        poolId: "pool-coding-main",
        modelId: "claude-opus-4.6",
        prompt: "继续追问同一上下文",
        failureMode: "none",
        sessionKey: "session-alpha",
      });
      expect(sim2.trace.status).toBe("success");
      expect(sim2.trace.cacheAffinityHit).toBe(true);
      expect(sim2.trace.finalAccountLabel).toContain("WorkBuddy · 副号");
    });

    // 若再给第 2 号也触发 429，严格锁模下绝不私自切到 MiniMax 或 DeepSeek，必须明确失败
    act(() => {
      const sim3 = result.current.runPoolSimulation({
        agentId: "claude_desktop",
        poolId: "pool-coding-main",
        modelId: "claude-opus-4.6",
        prompt: "最后一个 opus 账号也限流",
        failureMode: "429_first",
        sessionKey: "session-alpha",
      });
      expect(sim3.trace.status).toBe("failed");
      expect(sim3.trace.attempts[0].detail).toContain("严格锁模拒绝换模型");
    });

    // 清理冷却
    act(() => {
      result.current.clearMemberCooldown("pool-coding-main", cooledId);
    });
    expect(
      result.current.pools[0].members.find((m) => m.id === cooledId)?.cooldownUntilMs
    ).toBeUndefined();
  });

  it("supports explicit fallback model when explicitly configured by user", () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    // 给 claude-opus-4.6 显式配置降级目标 claude-sonnet-4.6
    act(() => {
      result.current.setPoolModelFallback(
        "pool-coding-main",
        "claude-opus-4.6",
        "claude-sonnet-4.6"
      );
    });

    // 先对所有 opus 账号注入 429
    const opusMembers = result.current.pools[0].members.filter((m) =>
      m.supportedModels.includes("claude-opus-4.6")
    );
    act(() => {
      for (const m of opusMembers) {
        result.current.injectMemberCooldown("pool-coding-main", m.id);
      }
    });

    // 再发请求，因显式配置了 fallbackModelId，允许安全降级到 claude-sonnet-4.6
    act(() => {
      const res = result.current.runPoolSimulation({
        agentId: "claude_desktop",
        poolId: "pool-coding-main",
        modelId: "claude-opus-4.6",
        prompt: "验证显式降级",
        failureMode: "none",
      });
      expect(res.trace.status).toBe("failover_success");
      expect(res.trace.requestedModel).toBe("claude-opus-4.6");
      expect(res.trace.finalModel).toBe("claude-sonnet-4.6");
    });
  });
});
