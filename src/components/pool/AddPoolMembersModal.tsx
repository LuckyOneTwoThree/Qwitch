import { useEffect, useId, useMemo, useState } from "react";
import { Check, Lock, Plus, ShieldAlert, Sparkles, X } from "lucide-react";
import { Modal } from "../ui/Modal";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import { MEMBER_TIER_META, UPSTREAM_MODEL_OPTIONS } from "../../data/mockPoolData";
import { getToolBrandMeta } from "../../domain/toolRegistry";
import type { AccountPool, MemberTier, UpstreamAccountCandidate } from "../../types/pool";
import { UpstreamMemberIcon } from "../AgentBrandIcon";

interface PendingPick {
  models: string[];
  tier: MemberTier;
}

export function AddPoolMembersModal({
  open,
  pool,
  onClose,
}: {
  open: boolean;
  pool: AccountPool | null;
  onClose: () => void;
}) {
  const titleId = useId();
  const { candidates, addMembersToPool, authorizeCodeArtsIsolated } = usePoolPrototype();
  const [toolFilter, setToolFilter] = useState<string>("all");
  const [pickedMap, setPickedMap] = useState<Record<string, PendingPick>>({});

  useEffect(() => {
    if (open) {
      setToolFilter("all");
      setPickedMap({});
    }
  }, [open, pool?.id]);

  const existingMemberByAccountId = useMemo(
    () => new Map((pool?.members ?? []).map((m) => [m.accountId, m])),
    [pool]
  );

  const toolTabs = useMemo(() => {
    const ids = Array.from(new Set(candidates.map((c) => c.toolId)));
    return ids.map((id) => ({
      id,
      name: id === "custom_api_key" ? "标准 API Key" : getToolBrandMeta(id).name,
    }));
  }, [candidates]);

  const filteredCandidates = useMemo(
    () =>
      toolFilter === "all"
        ? candidates
        : candidates.filter((c) => c.toolId === toolFilter),
    [candidates, toolFilter]
  );

  const pickedItems = useMemo(() => {
    const out: {
      candidate: UpstreamAccountCandidate;
      supportedModels: string[];
      tier: MemberTier;
    }[] = [];
    for (const c of candidates) {
      if (c.codeArtsMode === "client_readonly") continue;
      const pick = pickedMap[c.accountId];
      if (pick && pick.models.length > 0) {
        out.push({
          candidate: c,
          supportedModels: pick.models,
          tier: c.toolId === "custom_api_key" ? "t3_fallback" : pick.tier,
        });
      }
    }
    return out;
  }, [candidates, pickedMap]);

  const toggleModelForCandidate = (c: UpstreamAccountCandidate, modelId: string) => {
    if (c.codeArtsMode === "client_readonly") return;
    const existingMember = existingMemberByAccountId.get(c.accountId);
    if (existingMember?.supportedModels.includes(modelId)) return;

    setPickedMap((prev) => {
      const curr = prev[c.accountId] ?? {
        models: [],
        tier: c.toolId === "custom_api_key" ? "t3_fallback" : c.defaultTier,
      };
      const exists = curr.models.includes(modelId);
      const nextModels = exists
        ? curr.models.filter((m) => m !== modelId)
        : [...curr.models, modelId];
      if (nextModels.length === 0) {
        const copy = { ...prev };
        delete copy[c.accountId];
        return copy;
      }
      return { ...prev, [c.accountId]: { ...curr, models: nextModels } };
    });
  };

  const toggleWholeCandidate = (c: UpstreamAccountCandidate) => {
    if (c.codeArtsMode === "client_readonly") return;
    const existingMember = existingMemberByAccountId.get(c.accountId);
    const allOptions = (UPSTREAM_MODEL_OPTIONS[c.toolId] ?? []).map((m) => m.id);
    const availableToAdd = allOptions.filter(
      (id) => !existingMember?.supportedModels.includes(id)
    );
    if (availableToAdd.length === 0) return;

    setPickedMap((prev) => {
      if (prev[c.accountId]?.models.length) {
        const copy = { ...prev };
        delete copy[c.accountId];
        return copy;
      }
      const preferred = c.defaultModels.filter((id) => availableToAdd.includes(id));
      return {
        ...prev,
        [c.accountId]: {
          models: preferred.length > 0 ? preferred : [availableToAdd[0]],
          tier: c.toolId === "custom_api_key" ? "t3_fallback" : c.defaultTier,
        },
      };
    });
  };

  if (!pool) return null;

  return (
    <Modal open={open} onClose={onClose} labelledBy={titleId} maxWidthClass="max-w-2xl">
      <div className="flex max-h-[85vh] flex-col">
        <div className="flex items-start justify-between gap-3 border-b border-zinc-200 px-5 py-4 dark:border-zinc-800">
          <div>
            <h2 id={titleId} className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
              向「{pool.name}」添加账号或追加模型槽位
            </h2>
            <p className="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">
              支持单账号同时勾选多个模型；已在池中的账号可继续为其追加开放其余模型
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭添加成员弹窗"
            className="btn btn-icon btn-ghost"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        <div className="border-b border-zinc-200/80 px-5 py-2.5 dark:border-zinc-800">
          <div className="flex flex-wrap items-center gap-1">
            <button
              type="button"
              onClick={() => setToolFilter("all")}
              className={`chip ${toolFilter === "all" ? "chip-active" : ""}`}
            >
              全部工具 ({candidates.length})
            </button>
            {toolTabs.map((t) => (
              <button
                key={t.id}
                type="button"
                onClick={() => setToolFilter(t.id)}
                className={`chip ${toolFilter === t.id ? "chip-active" : ""}`}
              >
                {t.name}
              </button>
            ))}
          </div>
        </div>

        <div className="min-h-0 flex-1 space-y-2 overflow-y-auto px-5 py-3.5">
          {filteredCandidates.map((c) => {
            const existingMember = existingMemberByAccountId.get(c.accountId);
            const readonlyBlocked = c.codeArtsMode === "client_readonly";
            const models = UPSTREAM_MODEL_OPTIONS[c.toolId] ?? [];
            const availableToAdd = models.filter(
              (m) => !existingMember?.supportedModels.includes(m.id)
            );
            const allModelsInPool =
              Boolean(existingMember) && availableToAdd.length === 0;
            const currentPick = pickedMap[c.accountId];
            const hasNewPick = Boolean(currentPick && currentPick.models.length > 0);
            const isApiKey = c.toolId === "custom_api_key";

            return (
              <div
                key={c.accountId}
                className={`rounded-xl border px-3 py-2.5 transition ${
                  allModelsInPool
                    ? "border-zinc-200/70 bg-zinc-50 opacity-70 dark:border-zinc-800 dark:bg-zinc-900/40"
                    : readonlyBlocked
                      ? "border-amber-500/35 bg-amber-500/[0.05]"
                      : hasNewPick
                        ? "border-brand-500/50 bg-brand-50/40 dark:border-brand-400/45 dark:bg-brand-500/10"
                        : "border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900/60"
                }`}
              >
                <div className="flex flex-wrap items-center justify-between gap-2.5">
                  <div className="flex min-w-0 flex-1 items-center gap-2.5">
                    <button
                      type="button"
                      disabled={readonlyBlocked || allModelsInPool}
                      onClick={() => toggleWholeCandidate(c)}
                      aria-label={`勾选 ${c.label}`}
                      className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border transition ${
                        allModelsInPool
                          ? "border-zinc-300 bg-zinc-200 text-zinc-500 dark:border-zinc-700 dark:bg-zinc-800"
                          : readonlyBlocked
                            ? "cursor-not-allowed border-amber-400/60 bg-amber-500/10 text-amber-600"
                            : hasNewPick
                              ? "border-brand-600 bg-brand-600 text-white dark:border-brand-400 dark:bg-brand-400 dark:text-zinc-950"
                              : "border-zinc-300 bg-white dark:border-zinc-600 dark:bg-zinc-800"
                      }`}
                    >
                      {readonlyBlocked ? (
                        <Lock className="h-2.5 w-2.5" aria-hidden="true" />
                      ) : allModelsInPool || hasNewPick ? (
                        <Check className="h-3 w-3" aria-hidden="true" />
                      ) : null}
                    </button>

                    <UpstreamMemberIcon toolId={c.toolId} size="sm" />

                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="truncate text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                          {c.label}
                        </span>
                        <span className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                          {c.uidMasked}
                        </span>
                        {existingMember && (
                          <span className="badge badge-neutral">
                            {allModelsInPool ? "已全量入池" : "已在池中 · 可追加模型"}
                          </span>
                        )}
                      </div>
                      {readonlyBlocked ? (
                        <div className="mt-0.5 flex items-center gap-1 text-[11px] text-amber-700 dark:text-amber-300">
                          <ShieldAlert className="h-3 w-3 shrink-0" aria-hidden="true" />
                          <span>
                            客户端导入凭据为一次性 refresh token，禁止共享进推理池以免踢掉 IDE
                          </span>
                        </div>
                      ) : (
                        <div className="mt-0.5 text-[11px] tabular-nums text-zinc-500 dark:text-zinc-400">
                          可用水位 {c.quotaPercent}% · {c.quotaText}
                        </div>
                      )}
                    </div>
                  </div>

                  {readonlyBlocked ? (
                    <button
                      type="button"
                      onClick={() => authorizeCodeArtsIsolated(c.accountId)}
                      className="btn btn-xs btn-brand-soft shrink-0"
                    >
                      <Sparkles className="h-3 w-3" aria-hidden="true" />
                      签发独立 OAuth
                    </button>
                  ) : hasNewPick && !existingMember ? (
                    <select
                      value={isApiKey ? "t3_fallback" : currentPick.tier}
                      disabled={isApiKey}
                      onChange={(e) =>
                        setPickedMap((prev) => ({
                          ...prev,
                          [c.accountId]: {
                            ...prev[c.accountId],
                            tier: e.target.value as MemberTier,
                          },
                        }))
                      }
                      aria-label={`${c.label} 梯队`}
                      className="rounded-lg border border-zinc-200 bg-white px-2 py-1 text-[11px] text-zinc-800 focus:border-brand-500 focus:outline-none disabled:opacity-70 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-200"
                    >
                      {(Object.keys(MEMBER_TIER_META) as MemberTier[]).map((t) => (
                        <option key={t} value={t}>
                          {MEMBER_TIER_META[t].shortLabel}
                        </option>
                      ))}
                    </select>
                  ) : null}
                </div>

                {!readonlyBlocked && models.length > 0 && (
                  <div className="mt-2 flex flex-wrap items-center gap-1.5 pl-6">
                    <span className="text-[10px] text-zinc-400">多选模型:</span>
                    {models.map((m) => {
                      const alreadyOnMember = Boolean(
                        existingMember?.supportedModels.includes(m.id)
                      );
                      const picked = Boolean(currentPick?.models.includes(m.id));
                      return (
                        <button
                          key={m.id}
                          type="button"
                          disabled={alreadyOnMember}
                          onClick={() => toggleModelForCandidate(c, m.id)}
                          aria-pressed={alreadyOnMember || picked}
                          className={`inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 font-mono text-[11px] transition ${
                            alreadyOnMember
                              ? "cursor-default border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                              : picked
                                ? "border-brand-500/55 bg-brand-500/12 text-brand-800 dark:border-brand-400/50 dark:text-brand-200"
                                : "border-zinc-200 bg-zinc-50 text-zinc-500 hover:border-zinc-300 hover:text-zinc-800 dark:border-zinc-700 dark:bg-zinc-800/60 dark:text-zinc-400"
                          }`}
                        >
                          {(alreadyOnMember || picked) && (
                            <Check className="h-2.5 w-2.5" aria-hidden="true" />
                          )}
                          <span>{m.id}</span>
                          {alreadyOnMember && (
                            <span className="font-sans text-[9px] opacity-80">已在池</span>
                          )}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-zinc-200 bg-zinc-50/70 px-5 py-3 dark:border-zinc-800 dark:bg-zinc-900/70">
          <button type="button" onClick={onClose} className="btn btn-md btn-secondary">
            取消
          </button>
          <button
            type="button"
            disabled={pickedItems.length === 0}
            onClick={() => {
              addMembersToPool(pool.id, pickedItems);
              onClose();
            }}
            className="btn btn-md btn-primary gap-1.5"
          >
            <Plus className="h-3.5 w-3.5" aria-hidden="true" />
            确认加入 / 追加 ({pickedItems.reduce((s, x) => s + x.supportedModels.length, 0)} 模型槽位)
          </button>
        </div>
      </div>
    </Modal>
  );
}
