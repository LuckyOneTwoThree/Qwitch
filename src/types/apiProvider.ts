import type { UpstreamModelOption, WireProtocolKind } from "./pool";

export const API_PROVIDER_PREFIX = "api_";

export function isApiProviderToolId(toolId: string): boolean {
  return toolId.startsWith(API_PROVIDER_PREFIX) && toolId !== "api_assets";
}

export type ApiAuthScheme = "bearer" | "x_api_key" | "custom_header";

export interface ApiManualModelEntry {
  id: string;
  displayName?: string;
  vendorWireId?: string;
  contextWindow?: number;
  maxOutputTokens?: number;
}

export interface ApiProviderRecord {
  id: string;
  name: string;
  presetId?: string;
  baseUrl: string;
  wireProtocol: WireProtocolKind;
  authScheme: ApiAuthScheme;
  authHeaderName?: string;
  extraHeaders: Record<string, string>;
  modelsPath?: string;
  manualModels: ApiManualModelEntry[];
  allowLanHttp: boolean;
  accentColor?: string;
  icon?: string;
  enabled: boolean;
  lastTestStatus?: string;
  lastTestLatencyMs?: number;
  lastTestError?: string;
  lastTestedAt?: string;
  createdAt: string;
  updatedAt: string;
}

export interface SaveApiProviderInput {
  id?: string;
  name: string;
  presetId?: string;
  baseUrl: string;
  wireProtocol: WireProtocolKind;
  authScheme: ApiAuthScheme;
  authHeaderName?: string;
  extraHeaders: Record<string, string>;
  modelsPath?: string;
  manualModels: ApiManualModelEntry[];
  allowLanHttp: boolean;
  accentColor?: string;
  icon?: string;
  enabled?: boolean;
  initialApiKey?: string;
  initialKeyLabel?: string;
}

export interface AddApiProviderKeyInput {
  providerId: string;
  label?: string;
  apiKey: string;
}

export interface UpdateApiProviderKeyInput {
  accountId: string;
  label?: string;
  apiKey?: string;
}

export interface ApiProviderTestInput {
  providerId?: string;
  baseUrl: string;
  wireProtocol: WireProtocolKind;
  authScheme: ApiAuthScheme;
  authHeaderName?: string;
  extraHeaders: Record<string, string>;
  modelsPath?: string;
  allowLanHttp: boolean;
  apiKey?: string;
}

export interface ApiProviderTestResult {
  ok: boolean;
  latencyMs: number;
  discoveredModels: UpstreamModelOption[];
  modelCount: number;
  endpointPreview: string;
  error?: string;
}

export interface ApiProviderPreset {
  id: string;
  name: string;
  tagline: string;
  baseUrl: string;
  wireProtocol: WireProtocolKind;
  authScheme: ApiAuthScheme;
  accentColor: string;
  defaultModels: string[];
}

export const API_PROVIDER_PRESETS: readonly ApiProviderPreset[] = [
  {
    id: "deepseek",
    name: "DeepSeek",
    tagline: "官方 OpenAI 兼容端点",
    baseUrl: "https://api.deepseek.com",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#4d6bfe",
    defaultModels: ["deepseek-chat", "deepseek-reasoner"],
  },
  {
    id: "openai",
    name: "OpenAI",
    tagline: "官方 Responses / Chat 端点",
    baseUrl: "https://api.openai.com/v1",
    wireProtocol: "openai_responses",
    authScheme: "bearer",
    accentColor: "#10a37f",
    defaultModels: ["gpt-5.4", "gpt-5.4-mini", "o4-mini"],
  },
  {
    id: "anthropic",
    name: "Anthropic",
    tagline: "Claude 官方 Messages API",
    baseUrl: "https://api.anthropic.com",
    wireProtocol: "anthropic_messages",
    authScheme: "x_api_key",
    accentColor: "#d97757",
    defaultModels: ["claude-sonnet-4-6", "claude-opus-4-6", "claude-haiku-4-5"],
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    tagline: "多模型统一聚合网关",
    baseUrl: "https://openrouter.ai/api/v1",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#6366f1",
    defaultModels: ["anthropic/claude-sonnet-4.6", "openai/gpt-5.4", "deepseek/deepseek-r1"],
  },
  {
    id: "zhipu",
    name: "智谱 GLM / Coding",
    tagline: "智谱开放平台 & Coding Plan",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#2563eb",
    defaultModels: ["glm-5.3", "glm-5.3-flash"],
  },
  {
    id: "dashscope",
    name: "阿里云百炼",
    tagline: "通义千问 OpenAI 兼容模式",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#f97316",
    defaultModels: ["qwen3-coder-plus", "qwen-max"],
  },
  {
    id: "volcengine",
    name: "火山方舟 Ark",
    tagline: "豆包 / DeepSeek 方舟端点",
    baseUrl: "https://ark.cn-beijing.volces.com/api/v3",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#0ea5e9",
    defaultModels: ["doubao-seed-2.1-pro", "deepseek-v3-250324"],
  },
  {
    id: "siliconflow",
    name: "硅基流动",
    tagline: "SiliconFlow 高性价比推理",
    baseUrl: "https://api.siliconflow.cn/v1",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#8b5cf6",
    defaultModels: ["deepseek-ai/DeepSeek-V3", "Qwen/Qwen3-Coder-480B-A35B-Instruct"],
  },
  {
    id: "moonshot",
    name: "Moonshot Kimi",
    tagline: "月之暗面 Kimi 开放平台",
    baseUrl: "https://api.moonshot.cn/v1",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#14b8a6",
    defaultModels: ["kimi-k2.5", "moonshot-v1-128k"],
  },
  {
    id: "minimax_api",
    name: "MiniMax 开放平台",
    tagline: "MiniMax Anthropic 兼容端点",
    baseUrl: "https://api.minimaxi.com/anthropic",
    wireProtocol: "anthropic_messages",
    authScheme: "x_api_key",
    accentColor: "#ec4899",
    defaultModels: ["MiniMax-M2.5"],
  },
  {
    id: "ollama",
    name: "本地 Ollama",
    tagline: "本机或局域网开源模型",
    baseUrl: "http://127.0.0.1:11434/v1",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#71717a",
    defaultModels: ["qwen3-coder:30b", "deepseek-r1:14b"],
  },
  {
    id: "custom",
    name: "自定义兼容",
    tagline: "任意 OpenAI / Anthropic 中转",
    baseUrl: "https://api.openai.com/v1",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    accentColor: "#64748b",
    defaultModels: [],
  },
];

export function maskApiKeyPreview(key: string): string {
  const trimmed = key.trim();
  if (!trimmed) return "••••";
  if (trimmed.length > 10) {
    return `${trimmed.slice(0, 5)}•••${trimmed.slice(-4)}`;
  }
  if (trimmed.length > 4) {
    return `${trimmed.slice(0, 2)}•••${trimmed.slice(-2)}`;
  }
  return "••••";
}

export function buildUpstreamEndpointPreview(
  baseUrl: string,
  protocol: WireProtocolKind
): string {
  const trimmed = baseUrl.trim();
  if (!trimmed) return "";
  if (trimmed.endsWith("#")) {
    return trimmed.slice(0, -1).replace(/\/+$/, "");
  }
  const base = trimmed.replace(/\/+$/, "");
  const schemeIdx = base.indexOf("://");
  const rest = schemeIdx >= 0 ? base.slice(schemeIdx + 3) : base;
  const slashIdx = rest.indexOf("/");
  const pathPart = slashIdx >= 0 ? rest.slice(slashIdx + 1) : "";
  const hasVersion = pathPart
    .split("/")
    .some((seg) => /^v\d+/i.test(seg));
  const suffix =
    protocol === "anthropic_messages"
      ? "messages"
      : protocol === "openai_responses"
        ? "responses"
        : "chat/completions";
  return hasVersion ? `${base}/${suffix}` : `${base}/v1/${suffix}`;
}

export function protocolShortLabel(protocol: WireProtocolKind): string {
  switch (protocol) {
    case "anthropic_messages":
      return "Anthropic Messages";
    case "openai_responses":
      return "OpenAI Responses";
    case "openai_chat":
    default:
      return "OpenAI Chat";
  }
}
