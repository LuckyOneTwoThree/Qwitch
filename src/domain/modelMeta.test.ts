import { describe, expect, it } from "vitest";
import { formatModelSpec, formatTokenCount } from "./modelMeta";

describe("模型规格格式化", () => {
  it("按百万/千分档展示，且不编造数字", () => {
    expect(formatTokenCount(1_048_576)).toBe("1M");
    expect(formatTokenCount(2_097_152)).toBe("2M");
    expect(formatTokenCount(131_072)).toBe("128K");
    expect(formatTokenCount(undefined)).toBeNull();
    expect(formatTokenCount(0)).toBeNull();
  });

  it("有输出上限时一并展示", () => {
    expect(formatModelSpec({ contextWindow: 1_048_576, maxOutputTokens: 131_072 })).toBe(
      "1M 上下文 · 输出 128K"
    );
    expect(formatModelSpec({ contextWindow: 1_000_000 })).toBe("1M 上下文");
    expect(formatModelSpec({})).toBeNull();
  });
});
