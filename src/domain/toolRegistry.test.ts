import { describe, expect, it } from "vitest";
import { BUILTIN_TOOL_ORDER, getToolBrandMeta } from "./toolRegistry";

describe("toolRegistry", () => {
  it("lists the built-in tools in order", () => {
    expect([...BUILTIN_TOOL_ORDER]).toEqual([
      "trae_cn",
      "workbuddy",
      "qoder_cn",
      "minimax_code_cn",
      "lobsterai",
      "doubaowork",
    ]);
  });
  it("returns brand meta for every built-in tool", () => {
    for (const id of BUILTIN_TOOL_ORDER) expect(getToolBrandMeta(id)).toBeTruthy();
  });
  it("does not throw for unknown tools", () => {
    expect(() => getToolBrandMeta("unknown_tool")).not.toThrow();
  });
});
