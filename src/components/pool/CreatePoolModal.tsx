import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Check, Cpu, Lock, Plus, ShieldAlert, Sparkles, X } from "lucide-react";
import { Modal } from "../ui/Modal";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import {
  AFFINITY_MODE_META,
  MEMBER_TIER_META,
  POOL_SCENE_TEMPLATES,
  POOL_STRATEGY_META,
  UPSTREAM_MODEL_OPTIONS,
} from "../../data/mockPoolData";
import { getToolBrandMeta } from "../../domain/toolRegistry";
import type {
  AccountPool,
  AffinityMode,
  MemberTier,
  PoolStrategy,
  TargetAgentId,
  UpstreamAccountCandidate,
} from "../../types/pool";
import { AgentBrandIcon, UpstreamMemberIcon } from "../AgentBrandIcon";

interface SelectedAccountDraft {
  models: string[];
  tier: MemberTier;
}

export function CreatePoolModal({
  open,
  onClose,
  onCreatedWithAgents,
}: {
  open: boolean;
  onClose: () => void;
  onCreatedWithAgents?: (pool: AccountPool, agentIds: TargetAgentId[]) => void;
}) {
  const titleId = useId();
  const {
    candidates,
    agentBindings,
    createPool,
    mountPoolToAgents,
    authorizeCodeArtsIsolated,
  } = usePoolPrototype();

  const [selectedTemplateId, setSelectedTemplateId] = useState<string>("tpl-coding");
  const [name, setName] = useState<string>("主力 Coding 混编池");
  const [description, setDescription] = useState<string>(
    "聚合多工具编程账号并显式暴露 claude-sonnet-4.6 等主力模型，严格按 T1→T2→T3 梯队 + 同工具水位调度"
  );
  const [sceneTag, setSceneTag] = useState<string>("主力 Coding");
  const [strategy, setStrategy] = useState<PoolStrategy>("quota_first");
  const [affinityMode, setAffinityMode] = useState<AffinityMode>("auto_cache");
  const [skipActiveClientAccount, setSkipActiveClientAccount] = useState<boolean>(false);
  const [cooldownSeconds, setCooldownSeconds] = useState<number>(60);
  const [autoRefreshOn401, setAutoRefreshOn401] = useState<boolean>(true);
  const [fallbackApiKeyEnabled, setFallbackApiKeyEnabled] = useState<boolean>(true);
  const [selectedAccounts, setSelectedAccounts] = useState<Record<string, SelectedAccountDraft>>(
    {}
  );
  const [disabledModels, setDisabledModels] = useState<Set<string>>(() => new Set());
  const [bindAgentIds, setBindAgentIds] = useState<TargetAgentId[]>([]);
  const candidatesRef = useRef(candidates);
  candidatesRef.current = candidates;

  const buildSelectionForTemplate = (
    list: UpstreamAccountCandidate[],
    recommendedTools: string[],
    skipActive: boolean
  ) => {
    const next: Record<string, SelectedAccountDraft> = {};
    for (const c of list) {
      if (
        c.accountStatus === "active" &&
        c.codeArtsMode !== "client_readonly" &&
        recommendedTools.includes(c.toolId)
      ) {
        if (skipActive && c.isActiveOnClient) continue;
        next[c.accountId] = {
          models: [...c.defaultModels],
          tier: c.toolId === "custom_api_key" ? "t3_fallback" : c.defaultTier,
        };
      }
    }
    return next;
  };

  const applyTemplate = (tplId: string) => {
    setSelectedTemplateId(tplId);
    const tpl = POOL_SCENE_TEMPLATES.find((t) => t.id === tplId);
    if (!tpl) return;
    setName(tpl.defaultName);
    setDescription(tpl.description);
    setSceneTag(tpl.sceneTag);
    setStrategy(tpl.strategy);
    setAffinityMode(tpl.affinityMode);
    setSkipActiveClientAccount(tpl.skipActiveClientAccount);
    setCooldownSeconds(tpl.cooldownSeconds);
    setAutoRefreshOn401(tpl.autoRefreshOn401);
    setFallbackApiKeyEnabled(tpl.fallbackApiKeyEnabled);
    setDisabledModels(new Set());
    setSelectedAccounts(
      buildSelectionForTemplate(
        candidates,
        tpl.recommendedTools,
        tpl.skipActiveClientAccount
      )
    );
  };

  useEffect(() => {
    if (!open) return;
    setBindAgentIds([]);
    setDisabledModels(new Set());
    const tpl = POOL_SCENE_TEMPLATES[0];
    setSelectedTemplateId(tpl.id);
    setName(tpl.defaultName);
    setDescription(tpl.description);
    setSceneTag(tpl.sceneTag);
    setStrategy(tpl.strategy);
    setAffinityMode(tpl.affinityMode);
    setSkipActiveClientAccount(tpl.skipActiveClientAccount);
    setCooldownSeconds(tpl.cooldownSeconds);
    setAutoRefreshOn401(tpl.autoRefreshOn401);
    setFallbackApiKeyEnabled(tpl.fallbackApiKeyEnabled);
    setSelectedAccounts(
      buildSelectionForTemplate(
        candidatesRef.current,
        tpl.recommendedTools,
        tpl.skipActiveClientAccount
      )
    );
  }, [open]);

  const selectedList = useMemo(() => {
    const out: {
      candidate: UpstreamAccountCandidate;
      supportedModels: string[];
      tier: MemberTier;
    }[] = [];
    for (const c of candidates) {
      const draft = selectedAccounts[c.accountId];
      if (draft && draft.models.length > 0 && c.codeArtsMode !== "client_readonly") {
        out.push({
          candidate: c,
          supportedModels: draft.models,
          tier: c.toolId === "custom_api_key" ? "t3_fallback" : draft.tier,
        });
      }
    }
    return out;
  }, [candidates, selectedAccounts]);

  const exposedModelUnion = useMemo(() => {
    const map = new Map<string, number>();
    for (const item of selectedList) {
      for (const mid of item.supportedModels) {
        map.set(mid, (map.get(mid) ?? 0) + 1);
      }
    }
    return Array.from(map.entries()).map(([modelId, count]) => ({
      modelId,
      count,
      enabled: !disabledModels.has(modelId),
    }));
  }, [selectedList, disabledModels]);

  const toggleCandidate = (c: UpstreamAccountCandidate) => {
    if (c.codeArtsMode === "client_readonly") return;
    setSelectedAccounts((prev) => {
      const copy = { ...prev };
      if (copy[c.accountId]) {
        delete copy[c.accountId];
      } else {
        copy[c.accountId] = {
          models: [...c.defaultModels],
          tier: c.toolId === "custom_api_key" ? "t3_fallback" : c.defaultTier,
        };
      }
      return copy;
    });
  };

  const toggleCandidateModel = (c: UpstreamAccountCandidate, modelId: string) => {
    if (c.codeArtsMode === "client_readonly") return;
    setSelectedAccounts((prev) => {
      const existing = prev[c.accountId];
      if (!existing) {
        return {
          ...prev,
          [c.accountId]: {
            models: [modelId],
            tier: c.toolId === "custom_api_key" ? "t3_fallback" : c.defaultTier,
          },
        };
      }
      const has = existing.models.includes(modelId);
      const nextModels = has
        ? existing.models.filter((m) => m !== modelId)
        : [...existing.models, modelId];
      if (nextModels.length === 0) {
        const copy = { ...prev };
        delete copy[c.accountId];
        return copy;
      }
      return {
        ...prev,
        [c.accountId]: { ...existing, models: nextModels },
      };
    });
  };

  const updateCandidateTier = (accountId: string, tier: MemberTier) => {
    setSelectedAccounts((prev) => {
      const existing = prev[accountId];
      if (!existing) return prev;
      return { ...prev, [accountId]: { ...existing, tier } };
    });
  };

  const toggleExposedModel = (modelId: string) => {
    setDisabledModels((prev) => {
      const next = new Set(prev);
      if (next.has(modelId)) next.delete(modelId);
      else next.add(modelId);
      return next;
    });
  };

  const toggleBindAgent = (agentId: TargetAgentId) => {
    setBindAgentIds((prev) =>
      prev.includes(agentId) ? prev.filter((id) => id !== agentId) : [...prev, agentId]
    );
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim() || selectedList.length === 0) return;
    const enabledModelIds = exposedModelUnion
      .filter((m) => m.enabled)
      .map((m) => m.modelId);
    const created = createPool({
      name,
      description,
      sceneTag,
      strategy,
      affinityMode,
      skipActiveClientAccount,
      cooldownSeconds,
      autoRefreshOn401,
      fallbackApiKeyEnabled,
      members: selectedList,
      enabledModelIds,
    });
    if (bindAgentIds.length > 0) {
      if (onCreatedWithAgents) {
        onCreatedWithAgents(created, bindAgentIds);
      } else {
        mountPoolToAgents(created.id, bindAgentIds);
      }
    }
    onClose();
  };

  return (
    <Modal open={open} onClose={onClose} labelledBy={titleId} maxWidthClass="max-w-3xl">
      <form onSubmit={handleSubmit} className="flex max-h-[88vh] flex-col">
        <div className="flex items-start justify-between gap-3 border-b border-zinc-200 px-5 py-4 dark:border-zinc-800">
          <div>
            <h2 id={titleId} className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
              新建账号调度号池
            </h2>
            <p className="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">
              先按梯队挑入账号并多选该账号开放的模型；客户端直接选用模型名，后台自动多号接力
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭新建号池弹窗"
            className="btn btn-icon btn-ghost"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-5 py-4">
          {/* 1. 场景模板 */}
          <section className="space-y-2">
            <div className="text-xs font-semibold text-zinc-700 dark:text-zinc-300">
              1. 选择场景模板（可自由微调）
            </div>
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              {POOL_SCENE_TEMPLATES.map((tpl) => {
                const active = selectedTemplateId === tpl.id;
                return (
                  <button
                    key={tpl.id}
                    type="button"
                    onClick={() => applyTemplate(tpl.id)}
                    className={`rounded-xl border p-3 text-left transition ${
                      active
                        ? "border-brand-500 bg-brand-50/70 ring-1 ring-brand-500/25 dark:border-brand-400/60 dark:bg-brand-500/10"
                        : "border-zinc-200 bg-white hover:border-zinc-300 dark:border-zinc-800 dark:bg-zinc-900/60 dark:hover:border-zinc-700"
                    }`}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                        {tpl.title}
                      </span>
                      <span className="badge badge-brand">{tpl.sceneTag}</span>
                    </div>
                    <p className="mt-1 text-[11px] leading-relaxed text-zinc-500 dark:text-zinc-400">
                      {tpl.description}
                    </p>
                  </button>
                );
              })}
            </div>
          </section>

          {/* 2. 基础信息与路由策略 */}
          <section className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div>
              <label
                htmlFor="pool-name-input"
                className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300"
              >
                号池名称
              </label>
              <input
                id="pool-name-input"
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
                placeholder="例如：主力全栈 Coding 池"
                className="w-full rounded-lg border border-zinc-200 bg-zinc-50/70 px-3 py-2 text-xs text-zinc-900 focus:border-brand-500 focus:bg-white focus:outline-none dark:border-zinc-700 dark:bg-zinc-800/70 dark:text-zinc-100"
              />
            </div>

            <div>
              <label
                htmlFor="pool-desc-input"
                className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300"
              >
                用途备注
              </label>
              <input
                id="pool-desc-input"
                type="text"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="描述该号池面向的任务类型"
                className="w-full rounded-lg border border-zinc-200 bg-zinc-50/70 px-3 py-2 text-xs text-zinc-900 focus:border-brand-500 focus:bg-white focus:outline-none dark:border-zinc-700 dark:bg-zinc-800/70 dark:text-zinc-100"
              />
            </div>

            <div>
              <span className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
                梯队内选号策略
              </span>
              <div className="grid grid-cols-2 gap-1.5">
                {(Object.keys(POOL_STRATEGY_META) as PoolStrategy[]).map((key) => {
                  const active = strategy === key;
                  return (
                    <button
                      key={key}
                      type="button"
                      onClick={() => setStrategy(key)}
                      className={`rounded-lg border px-2.5 py-1.5 text-left text-[11px] font-medium transition ${
                        active
                          ? "border-brand-500 bg-brand-500/12 text-brand-800 dark:border-brand-400/60 dark:text-brand-200"
                          : "border-zinc-200 bg-white text-zinc-600 hover:bg-zinc-50 dark:border-zinc-700 dark:bg-zinc-800/60 dark:text-zinc-300"
                      }`}
                    >
                      {POOL_STRATEGY_META[key].shortLabel}
                    </button>
                  );
                })}
              </div>
            </div>

            <div>
              <span className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
                会话亲和与护栏
              </span>
              <div className="flex flex-wrap gap-1.5">
                {(Object.keys(AFFINITY_MODE_META) as AffinityMode[]).map((mode) => (
                  <button
                    key={mode}
                    type="button"
                    onClick={() => setAffinityMode(mode)}
                    className={`chip ${
                      affinityMode === mode
                        ? "chip-active"
                        : "border border-zinc-200 dark:border-zinc-700"
                    }`}
                  >
                    {AFFINITY_MODE_META[mode].label}
                  </button>
                ))}
              </div>
              <label className="mt-2 flex cursor-pointer items-center gap-2 text-[11px] text-zinc-600 dark:text-zinc-400">
                <input
                  type="checkbox"
                  checked={skipActiveClientAccount}
                  onChange={(e) => setSkipActiveClientAccount(e.target.checked)}
                  className="rounded border-zinc-300 text-brand-600 focus:ring-brand-500"
                />
                跳过本机客户端正在挂载使用的账号（防互踢）
              </label>
            </div>
          </section>

          {/* 3. 账号 × 多模型挑选 */}
          <section className="space-y-2.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs font-semibold text-zinc-700 dark:text-zinc-300">
                2. 勾选账号并多选该账号参与调度的模型（已选 {selectedList.length} 个账号）
              </span>
              <span className="text-[11px] text-zinc-500 dark:text-zinc-400">
                跨工具按 T1→T2→T3 梯队降级 · API Key 自动锁定 T3
              </span>
            </div>

            <div className="max-h-64 space-y-2 overflow-y-auto rounded-xl border border-zinc-200 bg-zinc-50/70 p-2.5 dark:border-zinc-800 dark:bg-zinc-950/60">
              {candidates.map((c) => {
                const isReadonlyCodeArts = c.codeArtsMode === "client_readonly";
                const draft = selectedAccounts[c.accountId];
                const checked = Boolean(draft) && !isReadonlyCodeArts;
                const models = UPSTREAM_MODEL_OPTIONS[c.toolId] ?? [];
                const toolName =
                  c.toolId === "custom_api_key"
                    ? "标准 API Key"
                    : getToolBrandMeta(c.toolId).name;
                const isApiKey = c.toolId === "custom_api_key";

                return (
                  <div
                    key={c.accountId}
                    className={`rounded-xl border px-3 py-2.5 transition ${
                      isReadonlyCodeArts
                        ? "border-amber-500/30 bg-amber-500/[0.05]"
                        : checked
                          ? "border-brand-500/45 bg-white dark:border-brand-400/40 dark:bg-zinc-900"
                          : "border-zinc-200/80 bg-white/80 dark:border-zinc-800 dark:bg-zinc-900/50"
                    }`}
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex min-w-0 flex-1 items-center gap-2.5">
                        <button
                          type="button"
                          disabled={isReadonlyCodeArts}
                          onClick={() => toggleCandidate(c)}
                          aria-label={`选择 ${c.label}`}
                          className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border transition ${
                            isReadonlyCodeArts
                              ? "cursor-not-allowed border-amber-400/60 bg-amber-500/10 text-amber-600"
                              : checked
                                ? "border-brand-600 bg-brand-600 text-white dark:border-brand-400 dark:bg-brand-400 dark:text-zinc-950"
                                : "border-zinc-300 bg-white dark:border-zinc-600 dark:bg-zinc-800"
                          }`}
                        >
                          {isReadonlyCodeArts ? (
                            <Lock className="h-2.5 w-2.5" aria-hidden="true" />
                          ) : checked ? (
                            <Check className="h-3 w-3" aria-hidden="true" />
                          ) : null}
                        </button>

                        <UpstreamMemberIcon toolId={c.toolId} size="xs" />

                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-1.5">
                            <span className="truncate text-xs font-medium text-zinc-900 dark:text-zinc-100">
                              {c.label}
                            </span>
                            <span className="badge badge-neutral">{toolName}</span>
                            {c.isActiveOnClient && (
                              <span className="badge border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300">
                                桌面活跃
                              </span>
                            )}
                            {c.accountStatus === "risk_paused" && (
                              <span className="badge border-rose-500/25 bg-rose-500/10 text-rose-700 dark:text-rose-300">
                                风控暂停
                              </span>
                            )}
                          </div>
                          {isReadonlyCodeArts ? (
                            <div className="mt-0.5 flex items-center gap-1 text-[11px] text-amber-700 dark:text-amber-300">
                              <ShieldAlert className="h-3 w-3 shrink-0" aria-hidden="true" />
                              <span>
                                客户端 refresh token 为一次性轮换，禁止直接入池以防踢掉 IDE
                              </span>
                            </div>
                          ) : (
                            <div className="mt-0.5 text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                              剩余 {c.quotaPercent}% · {c.quotaText}
                            </div>
                          )}
                        </div>
                      </div>

                      {isReadonlyCodeArts ? (
                        <button
                          type="button"
                          onClick={() => authorizeCodeArtsIsolated(c.accountId)}
                          className="btn btn-xs btn-brand-soft shrink-0"
                        >
                          <Sparkles className="h-3 w-3" aria-hidden="true" />
                          签发独立 OAuth 解锁
                        </button>
                      ) : checked ? (
                        <select
                          value={isApiKey ? "t3_fallback" : draft.tier}
                          disabled={isApiKey}
                          onChange={(e) =>
                            updateCandidateTier(c.accountId, e.target.value as MemberTier)
                          }
                          aria-label={`${c.label} 梯队`}
                          className="rounded-lg border border-zinc-200 bg-zinc-50 px-2 py-1 text-[11px] text-zinc-800 focus:border-brand-500 focus:outline-none disabled:opacity-70 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200"
                        >
                          {(Object.keys(MEMBER_TIER_META) as MemberTier[]).map((t) => (
                            <option key={t} value={t}>
                              {MEMBER_TIER_META[t].shortLabel}
                            </option>
                          ))}
                        </select>
                      ) : null}
                    </div>

                    {/* 多选模型 Pills */}
                    {!isReadonlyCodeArts && models.length > 0 && (
                      <div className="mt-2 flex flex-wrap items-center gap-1.5 pl-6">
                        <span className="text-[10px] text-zinc-400">开放模型:</span>
                        {models.map((m) => {
                          const modelPicked = Boolean(draft?.models.includes(m.id));
                          return (
                            <button
                              key={m.id}
                              type="button"
                              onClick={() => toggleCandidateModel(c, m.id)}
                              aria-pressed={modelPicked}
                              className={`inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 font-mono text-[11px] transition ${
                                modelPicked
                                  ? "border-brand-500/50 bg-brand-500/12 text-brand-800 dark:border-brand-400/50 dark:text-brand-200"
                                  : "border-zinc-200 bg-zinc-50 text-zinc-500 hover:border-zinc-300 hover:text-zinc-800 dark:border-zinc-700 dark:bg-zinc-800/60 dark:text-zinc-400"
                              }`}
                            >
                              {modelPicked && <Check className="h-2.5 w-2.5" aria-hidden="true" />}
                              <span>{m.id}</span>
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>

            {/* 汇总将对外暴露的模型 */}
            {exposedModelUnion.length > 0 && (
              <div className="rounded-xl border border-brand-500/25 bg-brand-500/[0.04] p-3">
                <div className="flex items-center justify-between gap-2 text-xs">
                  <span className="flex items-center gap-1.5 font-semibold text-zinc-800 dark:text-zinc-200">
                    <Cpu className="h-3.5 w-3.5 text-brand-600 dark:text-brand-400" aria-hidden="true" />
                    本池对外暴露的模型清单（点击可勾选禁用冗余模型）
                  </span>
                  <span className="text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                    已启用 {exposedModelUnion.filter((m) => m.enabled).length}/
                    {exposedModelUnion.length}
                  </span>
                </div>
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {exposedModelUnion.map((m) => (
                    <button
                      key={m.modelId}
                      type="button"
                      onClick={() => toggleExposedModel(m.modelId)}
                      aria-pressed={m.enabled}
                      className={`inline-flex items-center gap-1.5 rounded-lg border px-2 py-1 text-[11px] font-mono transition ${
                        m.enabled
                          ? "border-brand-500/50 bg-white text-zinc-900 shadow-2xs dark:border-brand-400/45 dark:bg-zinc-900 dark:text-zinc-100"
                          : "border-zinc-200 bg-zinc-100/70 text-zinc-400 line-through dark:border-zinc-800 dark:bg-zinc-900/40"
                      }`}
                    >
                      <span>{m.modelId}</span>
                      <span className="rounded bg-zinc-100 px-1 text-[10px] font-sans text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400">
                        {m.count}号
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            )}
          </section>

          {/* 4. 可选：创建后追加挂载到 Agent（走 Diff 确认） */}
          <section className="space-y-2">
            <div className="flex items-center justify-between text-xs">
              <span className="font-semibold text-zinc-700 dark:text-zinc-300">
                3. 创建后立即追加挂载到 Agent（可选，将弹出配置 Diff 确认）
              </span>
            </div>
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
              {agentBindings.map((agent) => {
                const picked = bindAgentIds.includes(agent.agentId);
                return (
                  <button
                    key={agent.agentId}
                    type="button"
                    onClick={() => toggleBindAgent(agent.agentId)}
                    aria-pressed={picked}
                    className={`flex items-center gap-2 rounded-xl border px-2.5 py-2 text-left transition ${
                      picked
                        ? "border-brand-500 bg-brand-500/10 text-brand-900 dark:border-brand-400/60 dark:text-brand-100"
                        : "border-zinc-200 bg-white text-zinc-700 hover:bg-zinc-50 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-300"
                    }`}
                  >
                    <AgentBrandIcon agentId={agent.agentId} size="xs" />
                    <span className="min-w-0 flex-1 truncate text-xs font-medium">
                      {agent.name}
                    </span>
                  </button>
                );
              })}
            </div>
          </section>
        </div>

        <div className="flex items-center justify-between gap-3 border-t border-zinc-200 bg-zinc-50/70 px-5 py-3 dark:border-zinc-800 dark:bg-zinc-900/70">
          <span className="text-[11px] text-zinc-500 dark:text-zinc-400">
            创建后写入本地加密库；如勾选 Agent，将弹出 Diff 对比后追加挂载（保留已有号池）
          </span>
          <div className="flex items-center gap-2">
            <button type="button" onClick={onClose} className="btn btn-md btn-secondary">
              取消
            </button>
            <button
              type="submit"
              disabled={!name.trim() || selectedList.length === 0}
              className="btn btn-md btn-primary gap-1.5"
            >
              <Plus className="h-3.5 w-3.5" aria-hidden="true" />
              创建号池 ({selectedList.length} 账号)
            </button>
          </div>
        </div>
      </form>
    </Modal>
  );
}
