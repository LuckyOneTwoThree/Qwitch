import { useEffect, useId, useMemo, useState } from "react";
import {
  CheckCircle2,
  Globe,
  KeyRound,
  Loader2,
  PlugZap,
  ShieldAlert,
  X,
} from "lucide-react";
import { Modal } from "../ui/Modal";
import { SelectMenu } from "../ui/SelectMenu";
import { useQwitch } from "../../context/QwitchContext";
import {
  API_PROVIDER_PRESETS,
  buildUpstreamEndpointPreview,
  type ApiAuthScheme,
  type ApiManualModelEntry,
  type ApiProviderPreset,
  type ApiProviderRecord,
  type ApiProviderTestResult,
} from "../../types/apiProvider";
import type { WireProtocolKind } from "../../types/pool";

const PROTOCOL_OPTIONS: Array<{ value: WireProtocolKind; label: string; desc: string }> = [
  {
    value: "openai_chat",
    label: "OpenAI Chat",
    desc: "/chat/completions（DeepSeek / OpenRouter / 通用中转）",
  },
  {
    value: "anthropic_messages",
    label: "Anthropic Messages",
    desc: "/messages（Claude 官方 / 智谱 & MiniMax Anthropic 兼容）",
  },
  {
    value: "openai_responses",
    label: "OpenAI Responses",
    desc: "/responses（OpenAI 新版 Responses API）",
  },
];

const AUTH_OPTIONS: Array<{ value: ApiAuthScheme; label: string; desc: string }> = [
  { value: "bearer", label: "Bearer Token", desc: "Authorization: Bearer <key>" },
  { value: "x_api_key", label: "x-api-key", desc: "Anthropic 风格请求头" },
  { value: "custom_header", label: "自定义请求头", desc: "指定 Header 名称携带密钥" },
];

export function ApiProviderModal({
  open,
  onClose,
  editingProvider,
  initialPresetId,
}: {
  open: boolean;
  onClose: () => void;
  editingProvider?: ApiProviderRecord | null;
  initialPresetId?: string | null;
}) {
  const { saveApiProvider, testApiProvider, showToast } = useQwitch();
  const titleId = useId();

  const [presetId, setPresetId] = useState<string>("deepseek");
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [wireProtocol, setWireProtocol] = useState<WireProtocolKind>("openai_chat");
  const [authScheme, setAuthScheme] = useState<ApiAuthScheme>("bearer");
  const [authHeaderName, setAuthHeaderName] = useState("");
  const [modelsPath, setModelsPath] = useState("");
  const [allowLanHttp, setAllowLanHttp] = useState(false);
  const [accentColor, setAccentColor] = useState("#4d6bfe");
  const [manualModels, setManualModels] = useState<ApiManualModelEntry[]>([]);
  const [initialApiKey, setInitialApiKey] = useState("");
  const [initialKeyLabel, setInitialKeyLabel] = useState("");
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<ApiProviderTestResult | null>(null);

  const applyPreset = (preset: ApiProviderPreset) => {
    setPresetId(preset.id);
    setName(preset.name);
    setBaseUrl(preset.baseUrl);
    setWireProtocol(preset.wireProtocol);
    setAuthScheme(preset.authScheme);
    setAuthHeaderName("");
    setModelsPath("");
    setAllowLanHttp(preset.id === "ollama");
    setAccentColor(preset.accentColor);
    setManualModels(
      preset.defaultModels.map((id) => ({
        id,
      }))
    );
    setTestResult(null);
  };

  useEffect(() => {
    if (!open) return;
    setSaving(false);
    setTesting(false);
    setTestResult(null);
    setInitialApiKey("");
    setInitialKeyLabel("");

    if (editingProvider) {
      setPresetId(editingProvider.presetId ?? "custom");
      setName(editingProvider.name);
      setBaseUrl(editingProvider.baseUrl);
      setWireProtocol(editingProvider.wireProtocol);
      setAuthScheme(editingProvider.authScheme);
      setAuthHeaderName(editingProvider.authHeaderName ?? "");
      setModelsPath(editingProvider.modelsPath ?? "");
      setAllowLanHttp(editingProvider.allowLanHttp);
      setAccentColor(editingProvider.accentColor ?? "#4d6bfe");
      setManualModels(editingProvider.manualModels ?? []);
      return;
    }

    const preset =
      API_PROVIDER_PRESETS.find((p) => p.id === initialPresetId) ??
      API_PROVIDER_PRESETS[0];
    applyPreset(preset);
  }, [open, editingProvider, initialPresetId]);

  const endpointPreview = useMemo(
    () => buildUpstreamEndpointPreview(baseUrl, wireProtocol),
    [baseUrl, wireProtocol]
  );

  const handleTest = async () => {
    if (testing) return;
    if (!baseUrl.trim()) {
      showToast("请先填写 Base URL", "warning");
      return;
    }
    setTesting(true);
    setTestResult(null);
    try {
      const res = await testApiProvider({
        providerId: editingProvider?.id,
        baseUrl: baseUrl.trim(),
        wireProtocol,
        authScheme,
        authHeaderName:
          authScheme === "custom_header" ? authHeaderName.trim() || undefined : undefined,
        extraHeaders: editingProvider?.extraHeaders ?? {},
        modelsPath: modelsPath.trim() || undefined,
        allowLanHttp,
        apiKey: initialApiKey.trim() || undefined,
      });
      setTestResult(res);
      if (res.ok && res.discoveredModels.length > 0 && manualModels.length === 0) {
        setManualModels(
          res.discoveredModels.slice(0, 20).map((m) => ({
            id: m.vendorWireId || m.id,
            displayName: m.displayName,
            vendorWireId: m.vendorWireId,
            contextWindow: m.contextWindow,
            maxOutputTokens: m.maxOutputTokens,
          }))
        );
      }
    } catch (err) {
      setTestResult({
        ok: false,
        latencyMs: 0,
        discoveredModels: [],
        modelCount: 0,
        endpointPreview,
        error: String(err),
      });
    } finally {
      setTesting(false);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (saving) return;
    if (!name.trim()) {
      showToast("请填写厂商名称", "warning");
      return;
    }
    if (!baseUrl.trim()) {
      showToast("请填写 Base URL", "warning");
      return;
    }
    if (!editingProvider && !initialApiKey.trim()) {
      showToast("请至少填写第一把 API Key", "warning");
      return;
    }
    setSaving(true);
    try {
      const saved = await saveApiProvider({
        id: editingProvider?.id,
        name: name.trim(),
        presetId,
        baseUrl: baseUrl.trim(),
        wireProtocol,
        authScheme,
        authHeaderName:
          authScheme === "custom_header" ? authHeaderName.trim() || undefined : undefined,
        extraHeaders: editingProvider?.extraHeaders ?? {},
        modelsPath: modelsPath.trim() || undefined,
        manualModels,
        allowLanHttp,
        accentColor,
        enabled: editingProvider?.enabled ?? true,
        initialApiKey: editingProvider ? undefined : initialApiKey.trim(),
        initialKeyLabel: editingProvider ? undefined : initialKeyLabel.trim() || undefined,
      });
      if (saved) {
        onClose();
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      labelledBy={titleId}
      flush
      maxWidthClass="max-w-2xl"
    >
      <div className="flex max-h-[85vh] flex-col">
        <div className="flex shrink-0 items-center justify-between border-b border-zinc-200/70 px-5 py-3.5 dark:border-white/[0.08]">
          <div className="flex items-center gap-2.5">
            <div className="flex h-8 w-8 items-center justify-center rounded-xl bg-brand-600/12 text-brand-700 dark:bg-brand-400/15 dark:text-brand-300">
              <KeyRound className="h-4 w-4" aria-hidden="true" />
            </div>
            <div>
              <h2 id={titleId} className="text-sm font-semibold text-zinc-900 dark:text-zinc-50">
                {editingProvider ? `编辑 API 厂商 · ${editingProvider.name}` : "添加 API 厂商资产"}
              </h2>
              <p className="text-[11px] text-zinc-500 dark:text-zinc-400">
                显式声明协议与端点，接入本地三协议网关与多 Key 号池调度
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭"
            className="btn btn-icon btn-ghost"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="flex min-h-0 flex-1 flex-col">
          <div className="min-h-0 flex-1 space-y-3.5 overflow-y-auto px-5 py-4">
            {!editingProvider && (
              <div>
                <div className="mb-1.5 text-[11px] font-medium text-zinc-500 dark:text-zinc-400">
                  快速选择厂商预设（均可手动覆盖）
                </div>
                <div className="flex flex-wrap gap-1.5">
                  {API_PROVIDER_PRESETS.map((preset) => {
                    const active = presetId === preset.id;
                    return (
                      <button
                        key={preset.id}
                        type="button"
                        onClick={() => applyPreset(preset)}
                        className={`inline-flex items-center gap-1.5 rounded-lg border px-2.5 py-1 text-xs font-medium transition ${
                          active
                            ? "border-brand-500/40 bg-brand-600/10 text-brand-700 dark:border-brand-400/40 dark:bg-brand-400/15 dark:text-brand-200"
                            : "border-zinc-200 bg-white text-zinc-600 hover:border-zinc-300 hover:text-zinc-900 dark:border-white/[0.08] dark:bg-zinc-900 dark:text-zinc-300 dark:hover:border-white/20"
                        }`}
                      >
                        <span
                          className="h-2 w-2 rounded-full"
                          style={{ backgroundColor: preset.accentColor }}
                          aria-hidden="true"
                        />
                        {preset.name}
                      </button>
                    );
                  })}
                </div>
              </div>
            )}

            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div>
                <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-200">
                  厂商名称
                </label>
                <input
                  type="text"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="例如：DeepSeek / OpenRouter"
                  className="field-input w-full"
                />
              </div>

              <div>
                <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-200">
                  上游协议（显式声明）
                </label>
                <SelectMenu
                  value={wireProtocol}
                  onChange={(v) => setWireProtocol(v as WireProtocolKind)}
                  options={PROTOCOL_OPTIONS.map((o) => ({
                    value: o.value,
                    label: o.label,
                  }))}
                  ariaLabel="选择上游协议"
                  className="h-9 w-full justify-between"
                />
              </div>
            </div>

            <div>
              <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-200">
                Base URL
              </label>
              <input
                type="text"
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                placeholder="https://api.deepseek.com（末尾加 # 可禁用自动拼 /v1）"
                className="field-input w-full font-mono text-xs"
              />
              {endpointPreview && (
                <div className="mt-1 flex items-center gap-1.5 text-[11px] text-zinc-500 dark:text-zinc-400">
                  <Globe className="h-3 w-3 shrink-0" aria-hidden="true" />
                  <span className="truncate font-mono">最终请求地址：{endpointPreview}</span>
                </div>
              )}
            </div>

            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div>
                <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-200">
                  鉴权方式
                </label>
                <SelectMenu
                  value={authScheme}
                  onChange={(v) => setAuthScheme(v as ApiAuthScheme)}
                  options={AUTH_OPTIONS.map((o) => ({
                    value: o.value,
                    label: o.label,
                  }))}
                  ariaLabel="选择鉴权方式"
                  className="h-9 w-full justify-between"
                />
              </div>

              {authScheme === "custom_header" ? (
                <div>
                  <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-200">
                    自定义请求头名称
                  </label>
                  <input
                    type="text"
                    value={authHeaderName}
                    onChange={(e) => setAuthHeaderName(e.target.value)}
                    placeholder="例如：api-key / X-API-Key"
                    className="field-input w-full font-mono text-xs"
                  />
                </div>
              ) : (
                <div>
                  <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-200">
                    模型列表路径（可选）
                  </label>
                  <input
                    type="text"
                    value={modelsPath}
                    onChange={(e) => setModelsPath(e.target.value)}
                    placeholder="默认自动推算 /v1/models"
                    className="field-input w-full font-mono text-xs"
                  />
                </div>
              )}
            </div>

            <label className="flex cursor-pointer items-center gap-2 text-xs text-zinc-600 dark:text-zinc-300">
              <input
                type="checkbox"
                checked={allowLanHttp}
                onChange={(e) => setAllowLanHttp(e.target.checked)}
                className="h-3.5 w-3.5 rounded border-zinc-300 text-brand-600 focus:ring-brand-500"
              />
              <span>允许局域网明文 HTTP（如自建内网 New-API / 局域网 Ollama；默认仅允许 HTTPS 与本机回环）</span>
            </label>

            {!editingProvider && (
              <div className="rounded-xl border border-zinc-200/80 bg-zinc-50/70 p-3 dark:border-white/[0.08] dark:bg-white/[0.02]">
                <div className="mb-2 text-xs font-medium text-zinc-800 dark:text-zinc-200">
                  第一把 API Key（保存后可继续追加多把 Key 组成号池）
                </div>
                <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
                  <div className="sm:col-span-2">
                    <input
                      type="password"
                      value={initialApiKey}
                      onChange={(e) => setInitialApiKey(e.target.value)}
                      placeholder="sk-..."
                      autoComplete="off"
                      className="field-input w-full font-mono text-xs"
                    />
                  </div>
                  <div>
                    <input
                      type="text"
                      value={initialKeyLabel}
                      onChange={(e) => setInitialKeyLabel(e.target.value)}
                      placeholder="别名（可选，如 主力 Key）"
                      className="field-input w-full text-xs"
                    />
                  </div>
                </div>
              </div>
            )}

            <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-zinc-200/80 bg-white px-3 py-2.5 dark:border-white/[0.08] dark:bg-zinc-900/50">
              <div className="min-w-0 flex-1 text-xs">
                {testResult ? (
                  testResult.ok ? (
                    <div className="flex items-center gap-1.5 text-emerald-700 dark:text-emerald-300">
                      <CheckCircle2 className="h-4 w-4 shrink-0" aria-hidden="true" />
                      <span>
                        连接成功 · {testResult.latencyMs} ms · 探测到 {testResult.modelCount} 个模型
                      </span>
                    </div>
                  ) : (
                    <div className="flex items-start gap-1.5 text-amber-700 dark:text-amber-300">
                      <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                      <span className="break-words">{testResult.error || "连接失败"}</span>
                    </div>
                  )
                ) : (
                  <span className="text-zinc-500 dark:text-zinc-400">
                    可先测试连通性并自动拉取 /models 目录（支持未保存草稿）
                  </span>
                )}
              </div>
              <button
                type="button"
                onClick={handleTest}
                disabled={testing}
                className="btn btn-sm btn-secondary shrink-0 gap-1.5"
              >
                {testing ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                ) : (
                  <PlugZap className="h-3.5 w-3.5" aria-hidden="true" />
                )}
                测试连接
              </button>
            </div>
          </div>

          <div className="flex shrink-0 items-center justify-end gap-2 border-t border-zinc-200/70 bg-white px-5 py-3 dark:border-white/[0.08] dark:bg-[#14161b]">
            <button type="button" onClick={onClose} className="btn btn-sm btn-secondary">
              取消
            </button>
            <button type="submit" disabled={saving} className="btn btn-sm btn-primary gap-1.5">
              {saving && <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />}
              {editingProvider ? "保存配置" : "添加厂商"}
            </button>
          </div>
        </form>
      </div>
    </Modal>
  );
}
