import { describe, expect, it } from "vitest";
import { BUILTIN_TOOL_ORDER } from "../domain/toolRegistry";
import { INITIAL_ACCOUNTS, MOCK_ADAPTERS } from "./mockStore";

describe("mockStore parity", () => {
  it("has an adapter and an account for every built-in tool", () => {
    for (const id of BUILTIN_TOOL_ORDER) {
      expect(MOCK_ADAPTERS.some((a) => a.id === id)).toBe(true);
      expect(INITIAL_ACCOUNTS.some((a) => a.toolId === id)).toBe(true);
    }
  });
  it("minimax_code_cn supports daily claim", () => {
    expect(MOCK_ADAPTERS.find((a) => a.id === "minimax_code_cn")?.capabilities.supportsDailyClaim).toBe(true);
  });
});
