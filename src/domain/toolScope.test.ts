import { describe, expect, it } from "vitest";
import { isToolScopeReadonly } from "./toolScope";
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
