import { describe, expect, it } from "vitest";
import {
  API_PROVIDER_PRESETS,
  buildUpstreamEndpointPreview,
  isApiProviderToolId,
  maskApiKeyPreview,
} from "./apiProvider";

describe("apiProvider domain helpers", () => {
  it("recognizes api_ prefix tool IDs", () => {
    expect(isApiProviderToolId("api_deepseek")).toBe(true);
    expect(isApiProviderToolId("api_custom")).toBe(true);
    expect(isApiProviderToolId("api_assets")).toBe(false);
    expect(isApiProviderToolId("trae_cn")).toBe(false);
    expect(isApiProviderToolId("custom_api_key")).toBe(false);
  });

  it("builds upstream endpoints with version detection and strict # mode", () => {
    expect(
      buildUpstreamEndpointPreview("https://api.deepseek.com", "openai_chat")
    ).toBe("https://api.deepseek.com/v1/chat/completions");
    expect(
      buildUpstreamEndpointPreview("https://openrouter.ai/api/v1", "openai_chat")
    ).toBe("https://openrouter.ai/api/v1/chat/completions");
    expect(
      buildUpstreamEndpointPreview("https://open.bigmodel.cn/api/paas/v4", "openai_chat")
    ).toBe("https://open.bigmodel.cn/api/paas/v4/chat/completions");
    expect(
      buildUpstreamEndpointPreview("https://api.anthropic.com", "anthropic_messages")
    ).toBe("https://api.anthropic.com/v1/messages");
    expect(
      buildUpstreamEndpointPreview("https://api.openai.com/v1", "openai_responses")
    ).toBe("https://api.openai.com/v1/responses");
    expect(
      buildUpstreamEndpointPreview("https://relay.example.com/custom#", "openai_chat")
    ).toBe("https://relay.example.com/custom");
  });

  it("masks API keys safely and ships 12 presets", () => {
    expect(maskApiKeyPreview("sk-1234567890abcd")).toBe("sk-12•••abcd");
    expect(maskApiKeyPreview("123")).toBe("••••");
    expect(API_PROVIDER_PRESETS.length).toBe(12);
    expect(new Set(API_PROVIDER_PRESETS.map((p) => p.id)).size).toBe(12);
  });
});
