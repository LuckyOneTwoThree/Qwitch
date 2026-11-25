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
            supportedModels: ["deepseek-v4-pro", "kimi-k3.1-pro"],
            tier: "t1_primary",
          },
          {
            candidate: candidateApiKey,
            supportedModels: ["deepseek-v4-pro", "glm-5.3"],
            tier: "t1_primary", // 应被强制锁回 t3_fallback
          },
        ],
      });
      createdId = created.id;
    });

    const createdPool = result.current.pools.find((p) => p.id === createdId)!;
    expect(result.current.pools.length).toBe(initialLen + 1);
    expect(createdPool.modelPolicies.map((m) => m.modelId)).toEqual(
      expect.arrayContaining(["deepseek-v4-pro", "kimi-k3.1-pro", "glm-5.3"])
    );
    expect(
      createdPool.members.find((m) => m.accountId === "acc-apikey-deepseek")?.tier
    ).toBe("t3_fallback");

    // 给已有成员切换支持模型（保留至少 1 个模型）
    const wbMemberId = createdPool.members[0].id;
    act(() => {
      result.current.togglePoolMemberModel(createdId, wbMemberId, "glm-5.3");
    });
    expect(
      result.current.pools
        .find((p) => p.id === createdId)!
        .members.find((m) => m.id === wbMemberId)?.supportedModels
    ).toContain("glm-5.3");

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
    const nextDefault = makeDefaultModelKey(createdId, "kimi-k3.1-pro");
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

  it("merges new models onto existing account members（码道只读凭据现在也可入池）", () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    const poolId = result.current.pools[0].id;
    // 按 id 取池：池数组的顺序可能因更新而变化，用下标会串池
    const poolById = () => result.current.pools.find((p) => p.id === poolId)!;
    const beforeLen = poolById().members.length;
    const wbPrimary = result.current.candidates.find((c) => c.accountId === "acc-wb-1")!;
    const readonlyCodeArts = result.current.candidates.find(
      (c) => c.accountId === "acc-codearts-1"
    )!;
    const wbAlreadyMember = poolById().members.some((m) => m.accountId === "acc-wb-1");

    // 码道现在由网关注入 AK/SK 签名直连，本机只读凭据也能入池（不再被拦截）
    act(() => {
      result.current.addMembersToPool(poolId, [
        { candidate: readonlyCodeArts, supportedModels: ["deepseek-v4.1-flash"], tier: "t2_backup" },
      ]);
    });
    expect(poolById().members.length).toBe(beforeLen + 1);
    expect(
      poolById().members.find((m) => m.accountId === "acc-codearts-1")?.supportedModels
    ).toContain("deepseek-v4.1-flash");

    // 为已在池的 WorkBuddy 主号追加 kimi-k3 模型（合并槽位而不是重复行）
    act(() => {
      result.current.addMembersToPool(poolId, [
        { candidate: wbPrimary, supportedModels: ["kimi-k3"], tier: "t1_primary" },
      ]);
    });
    expect(poolById().members.length).toBe(beforeLen + 1);
    expect(
      poolById().members.find((m) => m.accountId === "acc-wb-1")?.supportedModels
    ).toContain("kimi-k3");
    if (wbAlreadyMember) {
      // 同一账号再次加入只合并模型槽位，不新增成员
      expect(
        poolById().members.filter((m) => m.accountId === "acc-wb-1")
      ).toHaveLength(1);
    }
  });

  it("enforces strict model lock, intra-tool quota ordering, 429 cooldown failover, and session affinity", () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    // 请求主力池的 kimi-k3.1-pro（仅 T1 WorkBuddy×2 支持；WorkBuddy 主号 73% 优先于 副号 62%）
    let cooledId = "";
    act(() => {
      const sim1 = result.current.runPoolSimulation({
        agentId: "claude_desktop",
        poolId: "pool-coding-main",
        modelId: "kimi-k3.1-pro",
        prompt: "测试严格锁模 + 429 Failover",
        failureMode: "429_first",
        sessionKey: "session-alpha",
      });
      cooledId = sim1.cooledMemberId ?? "";
      expect(sim1.trace.status).toBe("failover_success");
      expect(sim1.trace.requestedModel).toBe("kimi-k3.1-pro");
      expect(sim1.trace.finalModel).toBe("kimi-k3.1-pro");
      expect(sim1.trace.attempts[0].accountLabel).toContain("WorkBuddy · 主号");
      expect(sim1.trace.attempts[1].accountLabel).toContain("WorkBuddy · 副号");
    });

    // 同会话连续发下一条请求：主号还在冷却，自动由 2 号通过 Prompt Cache 亲和直接命中
    act(() => {
      const sim2 = result.current.runPoolSimulation({
        agentId: "claude_desktop",
        poolId: "pool-coding-main",
        modelId: "kimi-k3.1-pro",
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
        modelId: "kimi-k3.1-pro",
        prompt: "最后一个 kimi-k3.1-pro 账号也限流",
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

    // 给 kimi-k3.1-pro 显式配置降级目标 deepseek-v4-pro
    act(() => {
      result.current.setPoolModelFallback(
        "pool-coding-main",
        "kimi-k3.1-pro",
        "deepseek-v4-pro"
      );
    });

    // 先对所有 kimi-k3.1-pro 账号注入 429
    const targetMembers = result.current.pools[0].members.filter((m) =>
      m.supportedModels.includes("kimi-k3.1-pro")
    );
    act(() => {
      for (const m of targetMembers) {
        result.current.injectMemberCooldown("pool-coding-main", m.id);
      }
    });

    // 再发请求，因显式配置了 fallbackModelId，允许安全降级到 deepseek-v4-pro
    act(() => {
      const res = result.current.runPoolSimulation({
        agentId: "claude_desktop",
        poolId: "pool-coding-main",
        modelId: "kimi-k3.1-pro",
        prompt: "验证显式降级",
        failureMode: "none",
      });
      expect(res.trace.status).toBe("failover_success");
      expect(res.trace.requestedModel).toBe("kimi-k3.1-pro");
      expect(res.trace.finalModel).toBe("deepseek-v4-pro");
    });
  });

  it("supports agent-centric batch model selection, primary account promotion, quota sorting, and scoped simulation", () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    // 为 Codex 桌面版一键全选 WorkBuddy 模型并清空后重新全选
    act(() => {
      result.current.selectAllAgentToolModels("codex_desktop", "workbuddy");
    });
    expect(
      result.current.getAgentToolEnabledModelIds("codex_desktop", "workbuddy").length
    ).toBeGreaterThanOrEqual(10);

    act(() => {
      result.current.clearAgentToolModels("codex_desktop", "workbuddy");
    });
    expect(
      result.current.getAgentToolEnabledModelIds("codex_desktop", "workbuddy")
    ).toEqual([]);

    act(() => {
      result.current.selectAllAgentToolModels("codex_desktop", "workbuddy");
    });
    const recModels = result.current.getAgentToolEnabledModelIds("codex_desktop", "workbuddy");
    expect(recModels.length).toBeGreaterThanOrEqual(10);

    // 调整同工具主备顺位：禁用跳过活跃号护栏，把低额度副号提为主号，并按额度重新排顺位
    act(() => {
      result.current.updateAgentToolRoutingOptions("codex_desktop", "workbuddy", {
        strategy: "order",
        skipActiveClientAccount: false,
      });
    });
    const beforeAccounts = result.current.getAgentToolAccounts("codex_desktop", "workbuddy");
    expect(beforeAccounts.length).toBeGreaterThanOrEqual(2);
    const secondAcc = beforeAccounts[1];

    act(() => {
      result.current.promoteAgentToolPrimaryAccount(
        "codex_desktop",
        "workbuddy",
        secondAcc.accountId
      );
    });
    const promotedAccounts = result.current.getAgentToolAccounts("codex_desktop", "workbuddy");
    expect(promotedAccounts[0].accountId).toBe(secondAcc.accountId);

    // 运行 scoped 请求 workbuddy/deepseek-v4-pro：必须仅在 WorkBuddy 内先打新主号、429 后切备用号
    const dedicatedPoolId = result.current.getAgentPrimaryPool("codex_desktop")!.id;
    act(() => {
      const sim = result.current.runPoolSimulation({
        agentId: "codex_desktop",
        poolId: dedicatedPoolId,
        modelId: "workbuddy/deepseek-v4-pro",
        prompt: "测试同工具主备切备",
        failureMode: "429_first",
      });
      expect(sim.trace.status).toBe("failover_success");
      expect(sim.trace.attempts[0].toolId).toBe("workbuddy");
      expect(sim.trace.attempts[1].toolId).toBe("workbuddy");
      expect(sim.trace.attempts[0].accountLabel).toBe(secondAcc.label);
    });

    // 按额度自动重排并仅启用主号
    act(() => {
      result.current.sortAgentToolAccountsByQuota("codex_desktop", "workbuddy");
      result.current.setAllAgentToolAccountsEnabled("codex_desktop", "workbuddy", true);
    });
    const finalAccs = result.current.getAgentToolAccounts("codex_desktop", "workbuddy");
    expect(finalAccs[0].quotaPercent).toBeGreaterThanOrEqual(finalAccs[1].quotaPercent);
    expect(finalAccs[0].enabled).toBe(true);
    expect(finalAccs[1].enabled).toBe(false);
  });

  it("只读码道账号会成为号池成员（签名上游可直接调度）", () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    const readonlyCodeArts = result.current.candidates.find(
      (c) => c.accountId === "acc-codearts-1"
    )!;
    expect(readonlyCodeArts.codeArtsMode).toBe("client_readonly");

    act(() => {
      result.current.setAgentToolModels("codex_desktop", "codearts", ["deepseek-v4.1-flash"]);
    });

    // 历史实现把只读凭据排除在号池之外，导致「点了没反应」；
    // 现在网关有码道签名上游，只读账号也会被物化进专属号池，勾选能落地。
    const pool = result.current.getAgentPrimaryPool("codex_desktop");
    expect(pool?.members.some((m) => m.accountId === "acc-codearts-1")).toBe(true);
  });

  it("解绑后的 Agent 再次仅勾选某工具 1 个模型时，不会连带点亮其他工具的残留模型", () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    act(() => {
      result.current.setAgentToolModels("mimo", "qoder_cn", ["qwen3.8-max"]);
    });
    expect(result.current.getAgentToolEnabledModelIds("mimo", "qoder_cn")).toEqual([
      "qwen3.8-max",
    ]);

    act(() => {
      result.current.unbindAgent("mimo");
    });
    expect(result.current.getAgentToolEnabledModelIds("mimo", "qoder_cn")).toEqual([]);

    act(() => {
      result.current.setAgentToolModels("mimo", "workbuddy", ["deepseek-v4-pro"]);
    });

    expect(result.current.getAgentToolEnabledModelIds("mimo", "workbuddy")).toEqual([
      "deepseek-v4-pro",
    ]);
    expect(result.current.getAgentToolEnabledModelIds("mimo", "qoder_cn")).toEqual([]);
    expect(result.current.getAgentToolEnabledModelIds("mimo", "trae_cn")).toEqual([]);
    expect(result.current.getAgentToolEnabledModelIds("mimo", "minimax_code_cn")).toEqual([]);
  });
});
