import { describe, expect, it } from "vitest";
import { mergeUniqueLogs, computeMockLogStats, resolveTheme } from "./logStateUtils";
import type { ActionLogItem } from "../types/qwitch";

const mk = (id: string, over: Partial<ActionLogItem> = {}): ActionLogItem =>
  ({ id, ...over }) as ActionLogItem;

describe("logStateUtils", () => {
  it("mergeUniqueLogs drops duplicate ids and keeps order", () => {
    const merged = mergeUniqueLogs([mk("a"), mk("b")], [mk("b"), mk("c")]);
    expect(merged.map((l) => l.id)).toEqual(["a", "b", "c"]);
  });
  it("computeMockLogStats counts total", () => {
    const stats = computeMockLogStats([mk("a"), mk("b")]);
    expect(JSON.stringify(stats)).toContain("2");
  });
  it("resolveTheme passes explicit themes through", () => {
    expect(resolveTheme("dark")).toBe("dark");
    expect(resolveTheme("light")).toBe("light");
  });
});
