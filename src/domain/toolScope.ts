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

/**
 * 该工具能否被 Qwitch 网关实际调度。
 *
 * 网关的 `resolve_upstream_auth` 只能走「Bearer 令牌 + Base URL」这一条路
 * （WorkBuddy / Qoder / Trae / MiniMax / 自定义 API）。码道的凭据是华为云
 * AK/SK + STS 签名，`access_token` 恒为空，网关既拿不到 bearer token，
 * 也没有 AK/SK 签名实现——即使把账号放进号池，请求也只会以
 * 「缺少有效 API Token」失败。因此码道的模型勾选没有落点。
 */
const GATEWAY_UNROUTABLE_TOOLS = new Set(["codearts"]);

export function isToolGatewayRoutable(toolId: string): boolean {
  return !GATEWAY_UNROUTABLE_TOOLS.has(toolId);
}
