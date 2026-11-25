/**
 * 模型规格的展示格式化。
 *
 * `contextWindow` / `maxOutputTokens` 来自内嵌的 models.dev 快照，
 * 缺失时一律返回 null（不显示），绝不用"猜"的数字填充。
 */
export function formatTokenCount(tokens?: number | null): string | null {
  if (!tokens || tokens <= 0) return null;
  if (tokens >= 1_000_000) {
    // 1M / 2M 这种整数最好读；1.5M 这类四舍五入到 M 级也足够表达量级
    return `${Math.round(tokens / 1_000_000)}M`;
  }
  return `${Math.round(tokens / 1024)}K`;
}

export function formatModelSpec(opt: {
  contextWindow?: number;
  maxOutputTokens?: number;
}): string | null {
  const ctx = formatTokenCount(opt.contextWindow);
  const out = formatTokenCount(opt.maxOutputTokens);
  if (!ctx) return null;
  return out ? `${ctx} 上下文 · 输出 ${out}` : `${ctx} 上下文`;
}
