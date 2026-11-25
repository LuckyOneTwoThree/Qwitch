// @vitest-environment jsdom
import React from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  PoolPrototypeProvider,
  usePoolPrototype,
} from "./usePoolPrototypeStore";
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

function probeItem(overrides: Partial<{
  accountId: string;
  toolId: string;
  source: string;
  modelCount: number;
  error: string | null;
}> = {}) {
  return {
    accountId: "acc-1",
    toolId: "workbuddy",
    source: "live",
    modelCount: 12,
    error: null,
    ...overrides,
  };
}

describe("模型目录探测结果的如实反馈", () => {
  let invokeMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    // 标记为 Tauri 运行时，走真实命令分支
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invokeMock = vi.fn(async (cmd: string) => {
      if (cmd === "get_pool_workspace_state") return EMPTY_WORKSPACE;
      if (cmd === "ensure_upstream_models_cmd") {
        return {
          probedCount: 0,
          skippedCount: 0,
          liveCount: 0,
          fallbackCount: 0,
          failedCount: 0,
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    });
    vi.doMock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
  });

  afterEach(() => {
    vi.doUnmock("@tauri-apps/api/core");
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("全部账号实时探测成功时才给成功提示", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_pool_workspace_state") return EMPTY_WORKSPACE;
      if (cmd === "ensure_upstream_models_cmd") {
        return {
          probedCount: 0,
          skippedCount: 0,
          liveCount: 0,
          fallbackCount: 0,
          failedCount: 0,
        };
      }
      if (cmd === "refresh_upstream_models_cmd") {
        return {
          workspace: EMPTY_WORKSPACE,
          probedCount: 2,
          liveCount: 2,
          fallbackCount: 0,
          failedCount: 0,
          items: [probeItem(), probeItem({ accountId: "acc-2" })],
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    });

    const onToast = vi.fn();
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(onToast),
    });

    await act(async () => {
      await result.current.refreshUpstreamModels();
    });

    expect(onToast).toHaveBeenCalledTimes(1);
    const [message, tone] = onToast.mock.calls[0];
    expect(tone).toBe("success");
    expect(message).toContain("实时探测成功 2 个");
    // 旧实现的无条件成功文案必须消失
    expect(message).not.toContain("已完成全部上游账号真实模型目录探测");
  });

  it("存在回退账号时降级为警告并说明回退来源", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_pool_workspace_state") return EMPTY_WORKSPACE;
      if (cmd === "ensure_upstream_models_cmd") {
        return {
          probedCount: 0,
          skippedCount: 0,
          liveCount: 0,
          fallbackCount: 0,
          failedCount: 0,
        };
      }
      if (cmd === "refresh_upstream_models_cmd") {
        return {
          workspace: EMPTY_WORKSPACE,
          probedCount: 2,
          liveCount: 1,
          fallbackCount: 1,
          failedCount: 0,
          items: [
            probeItem(),
            probeItem({
              accountId: "acc-2",
              toolId: "trae_cn",
              source: "catalog",
              modelCount: 8,
              error: "Trae CN 凭据缺少 access token",
            }),
          ],
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    });

    const onToast = vi.fn();
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(onToast),
    });

    await act(async () => {
      await result.current.refreshUpstreamModels();
    });

    const [message, tone] = onToast.mock.calls[0];
    expect(tone).toBe("warning");
    expect(message).toContain("实时探测成功 1 个");
    expect(message).toContain("回退内置目录 1 个");
    expect(message).toContain("缺少 access token");
  });

  it("全部失败时按错误提示，不给绿色成功", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_pool_workspace_state") return EMPTY_WORKSPACE;
      if (cmd === "ensure_upstream_models_cmd") {
        return {
          probedCount: 0,
          skippedCount: 0,
          liveCount: 0,
          fallbackCount: 0,
          failedCount: 0,
        };
      }
      if (cmd === "refresh_upstream_models_cmd") {
        return {
          workspace: EMPTY_WORKSPACE,
          probedCount: 1,
          liveCount: 0,
          fallbackCount: 0,
          failedCount: 1,
          items: [
            probeItem({
              source: "catalog",
              modelCount: 0,
              error: "请求超时",
            }),
          ],
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    });

    const onToast = vi.fn();
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(onToast),
    });

    await act(async () => {
      await result.current.refreshUpstreamModels();
    });

    const [message, tone] = onToast.mock.calls[0];
    expect(tone).toBe("error");
    expect(message).toContain("失败 1 个");
  });

  it("启动时自动触发一次 TTL 感知的探测", async () => {
    const { result } = renderHook(() => usePoolPrototype(), {
      wrapper: createWrapper(),
    });

    await waitFor(() => {
      const called = invokeMock.mock.calls.filter(
        ([cmd]) => cmd === "ensure_upstream_models_cmd"
      );
      expect(called.length).toBeGreaterThan(0);
    });

    // 自动探测静默：不产生任何 toast
    expect(result.current.isRefreshingModels).toBe(false);
  });
});
