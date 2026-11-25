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
 * - WorkBuddy / Qoder / Trae / MiniMax / 自定义 API：Bearer 令牌 + Base URL；
 * - 码道 CodeArts：华为云 AK/SK + STS 签名，走 `/api/v2/chat/completions`
 *   专用上游（网关里单独一条签名分支），因此**可以**调度；
 * - 豆包工作：没有任何面向第三方的模型接口（magpie 也是靠 CDP 驱动桌面端），
 *   我们只能读它的模型菜单做对照，无法作为上游，勾选没有落点。
 */
const GATEWAY_UNROUTABLE_TOOLS = new Set(["doubaowork"]);

export function isToolGatewayRoutable(toolId: string): boolean {
  return !GATEWAY_UNROUTABLE_TOOLS.has(toolId);
}

/** 不可调度时给用户的原因（用于横幅与 tooltip），可调度返回 null。 */
export function gatewayUnsupportedReason(toolId: string): string | null {
  if (toolId === "doubaowork") {
    return "豆包工作没有面向第三方的模型接口，Qwitch 只能读取它的模型菜单用于对照，无法作为网关上游调度（magpie 也是靠驱动桌面端实现的）。";
  }
  return null;
}
