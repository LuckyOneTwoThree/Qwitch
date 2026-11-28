import { useEffect, useMemo, useState } from "react";
import {
  CheckCircle2,
  Edit3,
  KeyRound,
  Loader2,
  PlugZap,
  Plus,
  Power,
  ShieldAlert,
  Sparkles,
  Trash2,
} from "lucide-react";
import { useQwitch } from "../../context/QwitchContext";
import {
  API_PROVIDER_PRESETS,
  buildUpstreamEndpointPreview,
  protocolShortLabel,
  type ApiManualModelEntry,
  type ApiProviderRecord,
} from "../../types/apiProvider";
import type { AccountSummary } from "../../types/qwitch";
import { ApiProviderModal } from "../../components/api/ApiProviderModal";
import { ConfirmDialog } from "../../components/ui/ConfirmDialog";
import { Modal } from "../../components/ui/Modal";

export function ApiAssetsView({
  selectedProviderId,
  onSelectProvider,
  createTriggerSeq = 0,
}: {
  selectedProviderId: string | null;
  onSelectProvider: (id: string | null) => void;
  createTriggerSeq?: number;
}) {
  const {
    apiProviders = [],
    accounts = [],
    toggleApiProvider,
    deleteApiProvider,
    testApiProvider,
    addApiProviderKey,
    updateApiProviderKey,
    saveApiProviderModels,
    toggleAccountDisabled,
    deleteAccount,
    showToast,
  } = useQwitch();

  const [providerModalOpen, setProviderModalOpen] = useState(false);
  const [editingProvider, setEditingProvider] = useState<ApiProviderRecord | null>(null);
  const [initialPresetId, setInitialPresetId] = useState<string | null>(null);

  useEffect(() => {
    if (createTriggerSeq > 0) {
      setEditingProvider(null);
      setInitialPresetId("deepseek");
      setProviderModalOpen(true);
    }
  }, [createTriggerSeq]);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ApiProviderRecord | null>(null);

  // 追加/编辑 Key 弹窗
  const [keyModal, setKeyModal] = useState<{
    mode: "add" | "edit";
    provider: ApiProviderRecord;
    account?: AccountSummary;
  } | null>(null);
  const [keyLabel, setKeyLabel] = useState("");
  const [keySecret, setKeySecret] = useState("");
  const [keySaving, setKeySaving] = useState(false);

  // 单厂商下手动模型维护
  const [manualInput, setManualInput] = useState("");

  const keysByProvider = useMemo(() => {
    const map = new Map<string, AccountSummary[]>();
    for (const acc of accounts) {
      const list = map.get(acc.toolId) ?? [];
      list.push(acc);
      map.set(acc.toolId, list);
    }
    return map;
  }, [accounts]);

  const visibleProviders = useMemo(
    () =>
      selectedProviderId
        ? apiProviders.filter((p) => p.id === selectedProviderId)
        : apiProviders,
    [apiProviders, selectedProviderId]
  );

  const openCreateModal = (presetId?: string) => {
    setEditingProvider(null);
    setInitialPresetId(presetId ?? "deepseek");
    setProviderModalOpen(true);
  };

  const openEditModal = (provider: ApiProviderRecord) => {
    setEditingProvider(provider);
    setInitialPresetId(null);
    setProviderModalOpen(true);
  };

  const handleTestExisting = async (provider: ApiProviderRecord) => {
    if (testingId) return;
    setTestingId(provider.id);
    try {
      const res = await testApiProvider({
        providerId: provider.id,
        baseUrl: provider.baseUrl,
        wireProtocol: provider.wireProtocol,
        authScheme: provider.authScheme,
        authHeaderName: provider.authHeaderName,
        extraHeaders: provider.extraHeaders,
        modelsPath: provider.modelsPath,
        allowLanHttp: provider.allowLanHttp,
      });
      if (res.ok) {
        showToast(
          `${provider.name} 连接成功（${res.latencyMs} ms，探测到 ${res.modelCount} 个模型）`,
          "success"
        );
      } else {
        showToast(`${provider.name} 探测异常：${res.error || "连接失败"}`, "warning");
      }
    } catch (err) {
      showToast(`测试失败：${String(err)}`, "error");
    } finally {
      setTestingId(null);
    }
  };

  const handleSubmitKeyModal = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!keyModal || keySaving) return;
    setKeySaving(true);
    try {
      if (keyModal.mode === "add") {
        if (!keySecret.trim()) {
          showToast("请输入 API Key", "warning");
          return;
        }
        const ok = await addApiProviderKey({
          providerId: keyModal.provider.id,
          label: keyLabel.trim() || undefined,
          apiKey: keySecret.trim(),
        });
        if (ok !== null || keySecret.trim()) {
          setKeyModal(null);
        }
      } else if (keyModal.account) {
        await updateApiProviderKey({
          accountId: keyModal.account.id,
          label: keyLabel.trim() || undefined,
          apiKey: keySecret.trim() || undefined,
        });
        setKeyModal(null);
      }
    } finally {
      setKeySaving(false);
    }
  };

  const handleAddManualModels = async (provider: ApiProviderRecord) => {
    const tokens = manualInput
      .split(/[\n,]+/)
      .map((s) => s.trim())
      .filter(Boolean);
    if (tokens.length === 0) {
      showToast("请输入至少一个模型 ID（支持逗号或换行分隔）", "warning");
      return;
    }
    const existingIds = new Set(provider.manualModels.map((m) => m.id));
    const next: ApiManualModelEntry[] = [...provider.manualModels];
    for (const id of tokens) {
      if (!existingIds.has(id)) {
        existingIds.add(id);
        next.push({ id });
      }
    }
    const ok = await saveApiProviderModels(provider.id, next);
    if (ok) {
      setManualInput("");
    }
  };

  const handleRemoveManualModel = async (
    provider: ApiProviderRecord,
    modelId: string
  ) => {
    const next = provider.manualModels.filter((m) => m.id !== modelId);
    await saveApiProviderModels(provider.id, next);
  };

  return (
    <div className="space-y-4">
      {apiProviders.length === 0 ? (
        <div className="rounded-2xl border border-dashed border-zinc-300/90 bg-white/70 p-6 dark:border-white/12 dark:bg-zinc-900/40">
          <div className="mb-4 flex items-center gap-3">
            <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-brand-600/12 text-brand-700 dark:bg-brand-400/15 dark:text-brand-300">
              <KeyRound className="h-5 w-5" aria-hidden="true" />
            </div>
            <div>
              <h2 className="text-sm font-semibold text-zinc-900 dark:text-zinc-50">
                还没有添加任何 API 厂商资产
              </h2>
              <p className="text-xs text-zinc-500 dark:text-zinc-400">
                点击下方预设即可快速录入官方 API Key、Coding Plan、中转站或本地 Ollama，并在 Agent 接入页直接调取。
              </p>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-4">
            {API_PROVIDER_PRESETS.map((preset) => (
              <button
                key={preset.id}
                type="button"
                onClick={() => openCreateModal(preset.id)}
                className="flex flex-col items-start gap-1 rounded-xl border border-zinc-200/80 bg-white p-3 text-left transition hover:border-brand-500/40 hover:shadow-xs dark:border-white/[0.08] dark:bg-zinc-900 dark:hover:border-brand-400/40"
              >
                <div className="flex items-center gap-2">
                  <span
                    className="h-2.5 w-2.5 rounded-full"
                    style={{ backgroundColor: preset.accentColor }}
                    aria-hidden="true"
                  />
                  <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                    {preset.name}
                  </span>
                </div>
                <span className="line-clamp-1 text-[11px] text-zinc-500 dark:text-zinc-400">
                  {preset.tagline}
                </span>
              </button>
            ))}
          </div>
        </div>
      ) : (
        <div className="space-y-3">
          {visibleProviders.map((provider) => {
            const keys = keysByProvider.get(provider.id) ?? [];
            const isTesting = testingId === provider.id;
            const endpointPreview = buildUpstreamEndpointPreview(
              provider.baseUrl,
              provider.wireProtocol
            );

            return (
              <section
                key={provider.id}
                className="rounded-2xl border border-zinc-200/80 bg-white p-4 shadow-2xs dark:border-white/[0.08] dark:bg-[#14161b]"
              >
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0 space-y-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span
                        className="h-2.5 w-2.5 shrink-0 rounded-full"
                        style={{ backgroundColor: provider.accentColor || "#4d6bfe" }}
                        aria-hidden="true"
                      />
                      <button
                        type="button"
                        onClick={() => onSelectProvider(provider.id)}
                        className="text-sm font-semibold text-zinc-900 hover:text-brand-700 dark:text-zinc-50 dark:hover:text-brand-300"
                      >
                        {provider.name}
                      </button>
                      <span className="badge badge-neutral">
                        {protocolShortLabel(provider.wireProtocol)}
                      </span>
                      <span className="badge badge-neutral">
                        {keys.length} 把 Key
                      </span>
                      {!provider.enabled && (
                        <span className="badge border-zinc-300 bg-zinc-100 text-zinc-600 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-400">
                          已停用
                        </span>
                      )}
                      {provider.lastTestStatus === "ok" && (
                        <span className="badge border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300">
                          <CheckCircle2 className="h-3 w-3" aria-hidden="true" />
                          {provider.lastTestLatencyMs ?? 0} ms
                        </span>
                      )}
                      {provider.lastTestStatus === "failed" && (
                        <span
                          title={provider.lastTestError}
                          className="badge border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300"
                        >
                          <ShieldAlert className="h-3 w-3" aria-hidden="true" />
                          探测异常
                        </span>
                      )}
                    </div>
                    <div className="truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                      {endpointPreview}
                    </div>
                  </div>

                  <div className="flex flex-wrap items-center gap-1.5">
                    <button
                      type="button"
                      onClick={() => handleTestExisting(provider)}
                      disabled={isTesting}
                      className="btn btn-xs btn-secondary gap-1"
                    >
                      {isTesting ? (
                        <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
                      ) : (
                        <PlugZap className="h-3 w-3" aria-hidden="true" />
                      )}
                      测速与探测
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setKeyLabel("");
                        setKeySecret("");
                        setKeyModal({ mode: "add", provider });
                      }}
                      className="btn btn-xs btn-secondary gap-1"
                    >
                      <Plus className="h-3 w-3" aria-hidden="true" />
                      添加 Key
                    </button>
                    <button
                      type="button"
                      onClick={() => openEditModal(provider)}
                      className="btn btn-xs btn-secondary gap-1"
                    >
                      <Edit3 className="h-3 w-3" aria-hidden="true" />
                      配置
                    </button>
                    <button
                      type="button"
                      onClick={() => toggleApiProvider(provider.id, !provider.enabled)}
                      title={provider.enabled ? "停用该厂商" : "启用该厂商"}
                      className="btn btn-xs btn-secondary gap-1"
                    >
                      <Power className="h-3 w-3" aria-hidden="true" />
                      {provider.enabled ? "停用" : "启用"}
                    </button>
                    <button
                      type="button"
                      onClick={() => setDeleteTarget(provider)}
                      title="删除厂商"
                      className="btn btn-xs btn-ghost text-rose-600 hover:bg-rose-500/10 dark:text-rose-400"
                    >
                      <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                  </div>
                </div>

                {/* 多 Key 列表 */}
                <div className="mt-3 space-y-1.5 border-t border-zinc-100 pt-3 dark:border-white/[0.06]">
                  {keys.length === 0 ? (
                    <div className="text-xs text-zinc-500 dark:text-zinc-400">
                      暂无 API Key，点击右上角「添加 Key」即可加入号池调度。
                    </div>
                  ) : (
                    keys.map((acc) => (
                      <div
                        key={acc.id}
                        className="flex flex-wrap items-center justify-between gap-2 rounded-xl bg-zinc-50/90 px-3 py-2 text-xs dark:bg-white/[0.03]"
                      >
                        <div className="flex min-w-0 items-center gap-2.5">
                          <KeyRound className="h-3.5 w-3.5 shrink-0 text-zinc-400" aria-hidden="true" />
                          <span className="truncate font-medium text-zinc-800 dark:text-zinc-100">
                            {acc.label}
                          </span>
                          <span className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                            {acc.uidMasked}
                          </span>
                          {acc.status === "disabled" ? (
                            <span className="badge badge-neutral">已停用</span>
                          ) : acc.status === "expired" ? (
                            <span className="badge border-rose-500/30 bg-rose-500/10 text-rose-700 dark:text-rose-300">
                              鉴权失效
                            </span>
                          ) : (
                            <span className="badge border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300">
                              可用
                            </span>
                          )}
                        </div>
                        <div className="flex items-center gap-1.5">
                          <button
                            type="button"
                            onClick={() => {
                              setKeyLabel(acc.label);
                              setKeySecret("");
                              setKeyModal({ mode: "edit", provider, account: acc });
                            }}
                            className="btn btn-xs btn-ghost"
                          >
                            编辑/换 Key
                          </button>
                          <button
                            type="button"
                            onClick={() => toggleAccountDisabled(acc.id)}
                            className="btn btn-xs btn-ghost"
                          >
                            {acc.status === "disabled" ? "启用" : "停用"}
                          </button>
                          <button
                            type="button"
                            onClick={() => deleteAccount(acc.id)}
                            className="btn btn-xs btn-ghost text-rose-600 dark:text-rose-400"
                          >
                            移除
                          </button>
                        </div>
                      </div>
                    ))
                  )}
                </div>

                {/* 自定义/维护的模型清单 */}
                <div className="mt-3 border-t border-zinc-100 pt-3 dark:border-white/[0.06]">
                  <div className="mb-2 flex items-center justify-between gap-2">
                    <div className="flex items-center gap-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-200">
                      <Sparkles className="h-3.5 w-3.5 text-brand-600 dark:text-brand-400" aria-hidden="true" />
                      <span>已维护模型目录（{provider.manualModels.length}）</span>
                    </div>
                  </div>
                  {provider.manualModels.length > 0 && (
                    <div className="mb-2.5 flex flex-wrap gap-1.5">
                      {provider.manualModels.map((m) => (
                        <span
                          key={m.id}
                          className="inline-flex items-center gap-1 rounded-lg border border-zinc-200/80 bg-zinc-50 px-2 py-0.5 font-mono text-[11px] text-zinc-700 dark:border-white/[0.08] dark:bg-zinc-900 dark:text-zinc-200"
                        >
                          {m.id}
                          <button
                            type="button"
                            aria-label={`删除模型 ${m.id}`}
                            onClick={() => handleRemoveManualModel(provider, m.id)}
                            className="ml-0.5 text-zinc-400 hover:text-rose-500"
                          >
                            ×
                          </button>
                        </span>
                      ))}
                    </div>
                  )}
                  {(selectedProviderId === provider.id || provider.manualModels.length === 0) && (
                    <div className="flex flex-wrap items-center gap-2">
                      <input
                        type="text"
                        value={manualInput}
                        onChange={(e) => setManualInput(e.target.value)}
                        placeholder="手动添加模型 ID（多个用逗号分隔，例如 deepseek-chat, gpt-5.4）"
                        className="field-input min-w-[240px] flex-1 font-mono text-xs"
                      />
                      <button
                        type="button"
                        onClick={() => handleAddManualModels(provider)}
                        className="btn btn-xs btn-secondary"
                      >
                        添加到目录
                      </button>
                    </div>
                  )}
                </div>
              </section>
            );
          })}
        </div>
      )}

      <ApiProviderModal
        open={providerModalOpen}
        onClose={() => setProviderModalOpen(false)}
        editingProvider={editingProvider}
        initialPresetId={initialPresetId}
      />

      <Modal
        open={Boolean(keyModal)}
        onClose={() => setKeyModal(null)}
        labelledBy="api-key-modal-title"
        flush
        maxWidthClass="max-w-md"
      >
        {keyModal && (
          <form onSubmit={handleSubmitKeyModal}>
            <div className="border-b border-zinc-200/70 px-5 py-3.5 dark:border-white/[0.08]">
              <h3 id="api-key-modal-title" className="text-sm font-semibold text-zinc-900 dark:text-zinc-50">
                {keyModal.mode === "add"
                  ? `为 ${keyModal.provider.name} 添加 API Key`
                  : `编辑 Key · ${keyModal.account?.label}`}
              </h3>
            </div>
            <div className="space-y-3 px-5 py-4">
              <div>
                <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-200">
                  别名
                </label>
                <input
                  type="text"
                  value={keyLabel}
                  onChange={(e) => setKeyLabel(e.target.value)}
                  placeholder="例如：主力 Key / 备用 Key"
                  className="field-input w-full text-xs"
                />
              </div>
              <div>
                <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-200">
                  {keyModal.mode === "add" ? "API Key" : "新 API Key（留空则仅改别名）"}
                </label>
                <input
                  type="password"
                  value={keySecret}
                  onChange={(e) => setKeySecret(e.target.value)}
                  placeholder="sk-..."
                  autoComplete="off"
                  className="field-input w-full font-mono text-xs"
                />
              </div>
            </div>
            <div className="flex justify-end gap-2 border-t border-zinc-200/70 px-5 py-3 dark:border-white/[0.08]">
              <button
                type="button"
                onClick={() => setKeyModal(null)}
                className="btn btn-sm btn-secondary"
              >
                取消
              </button>
              <button type="submit" disabled={keySaving} className="btn btn-sm btn-primary">
                {keyModal.mode === "add" ? "添加" : "保存"}
              </button>
            </div>
          </form>
        )}
      </Modal>

      <ConfirmDialog
        open={Boolean(deleteTarget)}
        title={`删除 API 厂商「${deleteTarget?.name ?? ""}」？`}
        description="删除后将同步移除该厂商配置及其下属全部 API Key 与模型缓存，已挂载的 Agent 下次同步时会自动摘除该厂商模型。"
        confirmLabel="确认删除"
        danger
        onCancel={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (deleteTarget) {
            const id = deleteTarget.id;
            setDeleteTarget(null);
            void deleteApiProvider(id).then((ok) => {
              if (ok && selectedProviderId === id) {
                onSelectProvider(null);
              }
            });
          }
        }}
      />
    </div>
  );
}
