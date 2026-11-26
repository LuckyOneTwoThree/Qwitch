// @vitest-environment jsdom
import React from "react";
import { cleanup, act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PoolPrototypeProvider, usePoolPrototype } from "./usePoolPrototypeStore";
import { INITIAL_GATEWAY_STATE } from "../data/mockPoolData";

const EMPTY_WORKSPACE = {
  gateway: INITIAL_GATEWAY_STATE,
  pools: [],
  candidates: [],
  agentBindings: [],
  traces: [],
};

function createWrapper(onToast = vi.fn()) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return <PoolPrototypeProvider onToast={onToast}>{children}</PoolPrototypeProvider>;
  };
}

describe("Agent 页高频操作的往返开销", () => {
  let invokeMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invokeMock = vi.fn(async (cmd: string) => {
      if (cmd === "get_pool_workspace_state") return EMPTY_WORKSPACE;
      if (cmd === "detect_agent_running_cmd") return [];
      if (cmd === "ensure_upstream_models_cmd") {
        return {
          probedCount: 0,
          skippedCount: 0,
          noCredentialCount: 0,
          liveCount: 0,
          fallbackCount: 0,
          failedCount: 0,
        };
      }
      if (cmd === "set_agent_tool_models_cmd") return EMPTY_WORKSPACE;
      if (cmd === "apply_account_preset_models_cmd") return EMPTY_WORKSPACE;
      throw new Error(`unexpected command: ${cmd}`);
    });
    vi.doMock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
  });

  afterEach(() => {
    cleanup();
    vi.doUnmock("@tauri-apps/api/core");
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("连续勾选同一工具的多模型只下发一次写命令，且携带最终列表", async () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });
    // 等待挂载期的初始化命令跑完，避免干扰断言
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    invokeMock.mockClear();

    // 模拟用户连点：250ms 合并窗口内勾三次，最终列表为 3 个模型
    act(() => {
      result.current.setAgentToolModels("codex_desktop", "workbuddy", ["deepseek-v4-pro"]);
      result.current.setAgentToolModels("codex_desktop", "workbuddy", [
        "deepseek-v4-pro",
        "glm-5.3",
      ]);
      result.current.setAgentToolModels("codex_desktop", "workbuddy", [
        "deepseek-v4-pro",
        "glm-5.3",
        "kimi-k3.1-pro",
      ]);
    });

    // 合并窗口内不得已经发出写命令
    expect(
      invokeMock.mock.calls.filter(([cmd]) => cmd === "set_agent_tool_models_cmd")
    ).toHaveLength(0);

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 350));
    });

    const writeCalls = invokeMock.mock.calls.filter(
      ([cmd]) => cmd === "set_agent_tool_models_cmd"
    );
    expect(writeCalls).toHaveLength(1);
    expect(writeCalls[0][1]).toMatchObject({
      agentId: "codex_desktop",
      toolId: "workbuddy",
      modelIds: ["deepseek-v4-pro", "glm-5.3", "kimi-k3.1-pro"],
    });
  });

  it("运行状态轮询走轻量命令，不再拉取全量工作区快照", async () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    invokeMock.mockClear();

    await act(async () => {
      result.current.refreshAgentRunning();
      await new Promise((resolve) => setTimeout(resolve, 20));
    });

    const commands = invokeMock.mock.calls.map(([cmd]) => cmd);
    expect(commands).toContain("detect_agent_running_cmd");
    expect(commands).not.toContain("get_pool_workspace_state");
  });

  it("一键套用推荐目录会调用后端命令，而不是只在前端改状态", async () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    invokeMock.mockClear();

    await act(async () => {
      result.current.applyAccountPresetModels("acc-codearts-1");
      await new Promise((resolve) => setTimeout(resolve, 30));
    });

    const call = invokeMock.mock.calls.find(
      ([cmd]) => cmd === "apply_account_preset_models_cmd"
    );
    expect(call?.[1]).toMatchObject({ accountId: "acc-codearts-1" });
  });
});
