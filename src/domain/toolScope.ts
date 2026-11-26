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
 * - WorkBuddy / Qoder / Trae / MiniMax / 有道龙虾 / 自定义 API：HTTP/Bearer 或私有签名协议；
 * - 码道 CodeArts：华为云 AK/SK + STS 签名，走 `/api/v2/chat/completions`；
 * - 豆包工作：Cookie 会话直连 `https://www.doubao.com/chat/completion` + SSE 块解析，同样可调度。
 */
const GATEWAY_UNROUTABLE_TOOLS = new Set<string>();

export function isToolGatewayRoutable(toolId: string): boolean {
  return !GATEWAY_UNROUTABLE_TOOLS.has(toolId);
}

/** 不可调度时给用户的原因（用于横幅与 tooltip），可调度返回 null。 */
export function gatewayUnsupportedReason(_toolId: string): string | null {
  return null;
}
