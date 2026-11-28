import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AddApiProviderKeyInput,
  ApiManualModelEntry,
  ApiProviderRecord,
  ApiProviderTestInput,
  ApiProviderTestResult,
  SaveApiProviderInput,
  UpdateApiProviderKeyInput,
} from "../types/apiProvider";
import { buildUpstreamEndpointPreview, maskApiKeyPreview } from "../types/apiProvider";
import type { AccountSummary, ToastTone } from "../types/qwitch";

const MOCK_INITIAL_PROVIDERS: ApiProviderRecord[] = [
  {
    id: "api_deepseek",
    name: "DeepSeek",
    presetId: "deepseek",
    baseUrl: "https://api.deepseek.com",
    wireProtocol: "openai_chat",
    authScheme: "bearer",
    extraHeaders: {},
    manualModels: [
      { id: "deepseek-chat", displayName: "DeepSeek Chat", contextWindow: 64000, maxOutputTokens: 8192 },
      { id: "deepseek-reasoner", displayName: "DeepSeek Reasoner", contextWindow: 64000, maxOutputTokens: 8192 },
    ],
    allowLanHttp: false,
    accentColor: "#4d6bfe",
    enabled: true,
    lastTestStatus: "ok",
    lastTestLatencyMs: 210,
    lastTestedAt: new Date().toISOString(),
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  },
];

export function useApiProviderStore({
  isTauriRuntime,
  isTrayWindow,
  showToast,
  refreshAccounts,
}: {
  isTauriRuntime: boolean;
  isTrayWindow: boolean;
  showToast: (message: string, tone?: ToastTone) => void;
  refreshAccounts: () => Promise<void>;
}) {
  const [apiProviders, setApiProviders] = useState<ApiProviderRecord[]>(() =>
    isTauriRuntime ? [] : MOCK_INITIAL_PROVIDERS
  );

  const refreshApiProviders = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const list = await invoke<ApiProviderRecord[]>("list_api_providers_cmd");
      setApiProviders(list);
    } catch (err) {
      console.warn("[Qwitch] list_api_providers_cmd failed:", err);
    }
  }, [isTauriRuntime]);

  useEffect(() => {
    if (!isTauriRuntime || isTrayWindow) return;
    void refreshApiProviders();
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen("qwitch://api-providers-updated", () => {
      if (!cancelled) void refreshApiProviders();
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [isTauriRuntime, isTrayWindow, refreshApiProviders]);

  const saveApiProvider = useCallback(
    async (input: SaveApiProviderInput): Promise<ApiProviderRecord | null> => {
      if (!isTauriRuntime) {
        const now = new Date().toISOString();
        const slug = (input.id ?? `api_${input.name.toLowerCase().replace(/[^a-z0-9]+/g, "_")}`).replace(/^_+/, "");
        const rec: ApiProviderRecord = {
          id: slug.startsWith("api_") ? slug : `api_${slug}`,
          name: input.name,
          presetId: input.presetId,
          baseUrl: input.baseUrl,
          wireProtocol: input.wireProtocol,
          authScheme: input.authScheme,
          authHeaderName: input.authHeaderName,
          extraHeaders: input.extraHeaders,
          modelsPath: input.modelsPath,
          manualModels: input.manualModels,
          allowLanHttp: input.allowLanHttp,
          accentColor: input.accentColor,
          icon: input.icon,
          enabled: input.enabled ?? true,
          createdAt: now,
          updatedAt: now,
        };
        setApiProviders((prev) => {
          const idx = prev.findIndex((p) => p.id === rec.id);
          if (idx >= 0) {
            const copy = [...prev];
            copy[idx] = { ...copy[idx], ...rec, createdAt: copy[idx].createdAt };
            return copy;
          }
          return [...prev, rec];
        });
        showToast(`已保存 API 厂商「${rec.name}」`, "success");
        return rec;
      }
      try {
        const saved = await invoke<ApiProviderRecord>("save_api_provider_cmd", { input });
        await Promise.all([refreshApiProviders(), refreshAccounts()]);
        showToast(`已保存 API 厂商「${saved.name}」`, "success");
        return saved;
      } catch (err) {
        showToast(`保存 API 厂商失败：${String(err)}`, "error");
        return null;
      }
    },
    [isTauriRuntime, refreshAccounts, refreshApiProviders, showToast]
  );

  const deleteApiProvider = useCallback(
    async (id: string): Promise<boolean> => {
      if (!isTauriRuntime) {
        setApiProviders((prev) => prev.filter((p) => p.id !== id));
        showToast("已删除 API 厂商", "success");
        return true;
      }
      try {
        await invoke("delete_api_provider_cmd", { id });
        await Promise.all([refreshApiProviders(), refreshAccounts()]);
        showToast("已删除 API 厂商及关联 Key", "success");
        return true;
      } catch (err) {
        showToast(`删除失败：${String(err)}`, "error");
        return false;
      }
    },
    [isTauriRuntime, refreshAccounts, refreshApiProviders, showToast]
  );

  const toggleApiProvider = useCallback(
    async (id: string, enabled: boolean): Promise<void> => {
      if (!isTauriRuntime) {
        setApiProviders((prev) =>
          prev.map((p) => (p.id === id ? { ...p, enabled } : p))
        );
        return;
      }
      try {
        const list = await invoke<ApiProviderRecord[]>("toggle_api_provider_cmd", {
          id,
          enabled,
        });
        setApiProviders(list);
      } catch (err) {
        showToast(`切换状态失败：${String(err)}`, "error");
      }
    },
    [isTauriRuntime, showToast]
  );

  const testApiProvider = useCallback(
    async (input: ApiProviderTestInput): Promise<ApiProviderTestResult> => {
      if (!isTauriRuntime) {
        const endpointPreview = buildUpstreamEndpointPreview(
          input.baseUrl,
          input.wireProtocol
        );
        return {
          ok: true,
          latencyMs: 145,
          discoveredModels: [
            {
              id: "deepseek-chat",
              displayName: "DeepSeek Chat",
              vendorWireId: "deepseek-chat",
              family: "DeepSeek",
              source: "live",
            },
          ],
          modelCount: 1,
          endpointPreview,
        };
      }
      const res = await invoke<ApiProviderTestResult>("test_api_provider_cmd", { input });
      if (input.providerId) {
        void refreshApiProviders();
      }
      return res;
    },
    [isTauriRuntime, refreshApiProviders]
  );

  const addApiProviderKey = useCallback(
    async (input: AddApiProviderKeyInput): Promise<AccountSummary | null> => {
      if (!isTauriRuntime) {
        showToast(`已添加 Key（${maskApiKeyPreview(input.apiKey)}）`, "success");
        return null;
      }
      try {
        const acc = await invoke<AccountSummary>("add_api_provider_key_cmd", { input });
        await refreshAccounts();
        showToast(`已添加 API Key「${acc.label}」`, "success");
        return acc;
      } catch (err) {
        showToast(`添加 API Key 失败：${String(err)}`, "error");
        return null;
      }
    },
    [isTauriRuntime, refreshAccounts, showToast]
  );

  const updateApiProviderKey = useCallback(
    async (input: UpdateApiProviderKeyInput): Promise<AccountSummary | null> => {
      if (!isTauriRuntime) {
        showToast("已更新 API Key", "success");
        return null;
      }
      try {
        const acc = await invoke<AccountSummary>("update_api_provider_key_cmd", { input });
        await refreshAccounts();
        showToast(`已更新 API Key「${acc.label}」`, "success");
        return acc;
      } catch (err) {
        showToast(`更新 API Key 失败：${String(err)}`, "error");
        return null;
      }
    },
    [isTauriRuntime, refreshAccounts, showToast]
  );

  const saveApiProviderModels = useCallback(
    async (id: string, models: ApiManualModelEntry[]): Promise<boolean> => {
      if (!isTauriRuntime) {
        setApiProviders((prev) =>
          prev.map((p) => (p.id === id ? { ...p, manualModels: models } : p))
        );
        showToast(`已保存 ${models.length} 个模型`, "success");
        return true;
      }
      try {
        await invoke<ApiProviderRecord>("save_api_provider_models_cmd", {
          id,
          models,
        });
        await refreshApiProviders();
        showToast(`已保存 ${models.length} 个自定义模型`, "success");
        return true;
      } catch (err) {
        showToast(`保存模型目录失败：${String(err)}`, "error");
        return false;
      }
    },
    [isTauriRuntime, refreshApiProviders, showToast]
  );

  return {
    apiProviders,
    refreshApiProviders,
    saveApiProvider,
    deleteApiProvider,
    toggleApiProvider,
    testApiProvider,
    addApiProviderKey,
    updateApiProviderKey,
    saveApiProviderModels,
  };
}
