import type { UpstreamAccountCandidate } from "../types/pool";

/**
 * 该工具下的账号是否全部为「本机只读凭据」。
 *
 * 这类账号（典型是码道从本机客户端导入的凭据）只用于额度监控，
 * 不参与网关调度，因此勾选模型不会产生任何效果——UI 必须据此禁用勾选，
 * 而不是让用户点了没反应。
 */
export function isToolScopeReadonly(
  candidates: UpstreamAccountCandidate[],
  toolId: string
): boolean {
  const scoped = candidates.filter((c) => c.toolId === toolId);
  return scoped.length > 0 && scoped.every((c) => c.codeArtsMode === "client_readonly");
}
