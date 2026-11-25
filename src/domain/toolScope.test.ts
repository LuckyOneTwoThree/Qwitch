import { describe, expect, it } from "vitest";
import { isToolGatewayRoutable, isToolScopeReadonly } from "./toolScope";
import type { UpstreamAccountCandidate } from "../types/pool";

function cand(overrides: Partial<UpstreamAccountCandidate> = {}): UpstreamAccountCandidate {
  return {
    accountId: "acc-1",
    toolId: "codearts",
    label: "码道",
    uidMasked: "u***1",
    channel: "opencode_bridge",
    defaultModels: [],
    modelOptions: [],
    defaultTier: "t2_backup",
    quotaPercent: 50,
    quotaText: "",
    isActiveOnClient: false,
    accountStatus: "active",
    ...overrides,
  };
}

describe("工具作用域只读判定", () => {
  it("全部账号为本机只读凭据时判为只读（模型不可勾选）", () => {
    expect(
      isToolScopeReadonly([cand({ codeArtsMode: "client_readonly" })], "codearts")
    ).toBe(true);
  });

  it("只要有一个账号完成独立授权，就不再整片禁用", () => {
    expect(
      isToolScopeReadonly(
        [
          cand({ accountId: "a", codeArtsMode: "client_readonly" }),
          cand({ accountId: "b", codeArtsMode: "isolated_oauth" }),
        ],
        "codearts"
      )
    ).toBe(false);
  });

  it("该工具没有账号时返回 false（走空态而不是禁用态）", () => {
    expect(isToolScopeReadonly([], "codearts")).toBe(false);
    expect(
      isToolScopeReadonly([cand({ toolId: "workbuddy" })], "codearts")
    ).toBe(false);
  });
});

describe("网关可调度性", () => {
  it("码道使用 AK/SK 签名，没有 bearer 令牌，网关无法调度", () => {
    expect(isToolGatewayRoutable("codearts")).toBe(false);
  });

  it("Bearer 类上游仍然可以调度", () => {
    for (const tool of ["workbuddy", "qoder_cn", "trae_cn", "minimax_code_cn", "custom_api_key"]) {
      expect(isToolGatewayRoutable(tool)).toBe(true);
    }
  });
});
