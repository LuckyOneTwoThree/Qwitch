import { useEffect, useState } from "react";
import { AlertTriangle, PencilLine, Save, Sparkles, Trash2 } from "lucide-react";
import { Modal } from "../ui/Modal";
import type { UpstreamAccountCandidate } from "../../types/pool";

const SOURCE_META: Record<string, { label: string; className: string; hint: string }> = {
  live: {
    label: "实时探测",
    className: "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
    hint: "来自上游真实接口，最可信",
  },
  client: {
    label: "本机客户端",
    className: "border-teal-500/30 bg-teal-500/10 text-teal-700 dark:text-teal-300",
    hint: "从本机客户端读取到的真实模型清单",
  },
  preset: {
    label: "内置预设",
    className: "border-indigo-500/30 bg-indigo-500/10 text-indigo-700 dark:text-indigo-300",
    hint: "内置推荐目录，未与上游核对；可一键套用后再修改",
  },
  manual: {
    label: "手工维护",
    className: "border-sky-500/30 bg-sky-500/10 text-sky-700 dark:text-sky-300",
    hint: "由你手工填写，不受自动探测覆盖之外的影响",
  },
  catalog: {
    label: "未探测",
    className: "border-amber-500/30 bg-amber-500/10 text-amber-800 dark:text-amber-300",
    hint: "该工具没有可用的探测接口，当前展示的是静态兜底表，可能与真实可用模型不一致",
  },
};

function resolveSource(candidate: UpstreamAccountCandidate): string {
  const options = candidate.modelOptions ?? [];
  if (options.length === 0) return "catalog";
  if (options.every((m) => m.source === "live")) return "live";
  if (options.every((m) => m.source === "client")) return "client";
  if (options.every((m) => m.source === "manual")) return "manual";
  if (options.every((m) => m.source === "preset")) return "preset";
  return "catalog";
}

export function ManualModelModal({
  open,
  onClose,
  toolName,
  candidates,
  onSave,
  onApplyPreset,
  presetAvailable,
}: {
  open: boolean;
  onClose: () => void;
  toolName: string;
  candidates: UpstreamAccountCandidate[];
  onSave: (accountId: string, modelIds: string[]) => void;
  onApplyPreset: (accountId: string) => void;
  presetAvailable: boolean;
}) {
  const [drafts, setDrafts] = useState<Record<string, string>>({});

  useEffect(() => {
    if (!open) return;
    const next: Record<string, string> = {};
    for (const account of candidates) {
      next[account.accountId] = (account.modelOptions ?? [])
        .map((m) => m.vendorWireId || m.id)
        .join("\n");
    }
    setDrafts(next);
  }, [open, candidates]);

  return (
    <Modal
      open={open}
      onClose={onClose}
      labelledBy="manual-model-title"
      describedBy="manual-model-desc"
      maxWidthClass="max-w-2xl"
    >
      <div className="space-y-3">
        <div className="flex items-start gap-2">
          <span className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-brand-500/12 text-brand-700 dark:text-brand-400">
            <PencilLine className="h-4 w-4" aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <h2
              id="manual-model-title"
              className="text-sm font-semibold text-zinc-900 dark:text-zinc-100"
            >
              {toolName} · 手工维护模型目录
            </h2>
            <p
              id="manual-model-desc"
              className="mt-0.5 text-[11px] leading-relaxed text-zinc-500 dark:text-zinc-400"
            >
              每行一个模型 ID（写上游真实 ID，例如 <code>deepseek-v4.1-flash</code>）。
              保存后该账号的可用模型即以这里为准；若之后实时探测成功，自动回落到实时结果。
            </p>
          </div>
        </div>

        <div className="flex items-start gap-2 rounded-xl border border-amber-500/25 bg-amber-500/[0.07] px-3 py-2 text-[11px] leading-relaxed text-amber-800 dark:text-amber-300">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <span>
            码道 CodeArts 没有可用的模型目录接口，静态兜底表只是猜测。请按客户端里真实可选的模型填写，
            否则网关把请求转到上游时会被拒（400 / 模型不存在）。
          </span>
        </div>

        <div className="max-h-[52vh] space-y-2 overflow-y-auto pr-0.5">
          {candidates.length === 0 ? (
            <div className="rounded-xl border border-dashed border-zinc-200 py-8 text-center text-xs text-zinc-400 dark:border-zinc-800">
              该工具下还没有账号
            </div>
          ) : (
            candidates.map((account) => {
              const source = resolveSource(account);
              const meta = SOURCE_META[source] ?? SOURCE_META.catalog;
              const draft = drafts[account.accountId] ?? "";
              return (
                <div
                  key={account.accountId}
                  className="rounded-xl border border-zinc-200/80 bg-white p-2.5 dark:border-zinc-800 dark:bg-zinc-900/60"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex min-w-0 items-center gap-1.5">
                      <span className="truncate text-xs font-medium text-zinc-900 dark:text-zinc-100">
                        {account.label}
                      </span>
                      <span className="shrink-0 font-mono text-[11px] text-zinc-400">
                        {account.uidMasked}
                      </span>
                    </div>
                    <span
                      className={`badge shrink-0 ${meta.className}`}
                      title={meta.hint}
                    >
                      {meta.label}
                    </span>
                  </div>

                  <textarea
                    value={draft}
                    onChange={(e) =>
                      setDrafts((prev) => ({ ...prev, [account.accountId]: e.target.value }))
                    }
                    rows={3}
                    spellCheck={false}
                    aria-label={`${account.label} 的模型列表（每行一个）`}
                    placeholder={"deepseek-v4.1-flash\nglm-5.3-flash\nkimi-k3.1-pro"}
                    className="field-input mt-2 w-full resize-y font-mono text-[11px] leading-relaxed"
                  />

                  <div className="mt-1.5 flex flex-wrap items-center justify-end gap-1.5">
                    {presetAvailable && (
                      <button
                        type="button"
                        onClick={() => onApplyPreset(account.accountId)}
                        className="btn btn-xs btn-secondary gap-1"
                        title="用内置推荐目录覆盖该账号（来源标记为「内置预设」）"
                      >
                        <Sparkles className="h-3 w-3" aria-hidden="true" />
                        套用推荐目录
                      </button>
                    )}
                    <button
                      type="button"
                      onClick={() => {
                        setDrafts((prev) => ({ ...prev, [account.accountId]: "" }));
                        onSave(account.accountId, []);
                      }}
                      className="btn btn-xs btn-secondary gap-1 text-rose-600 hover:bg-rose-50 dark:text-rose-400 dark:hover:bg-rose-950/40"
                    >
                      <Trash2 className="h-3 w-3" aria-hidden="true" />
                      清空
                    </button>
                    <button
                      type="button"
                      onClick={() =>
                        onSave(
                          account.accountId,
                          draft
                            .split(/[\n,]/)
                            .map((line) => line.trim())
                            .filter(Boolean)
                        )
                      }
                      className="btn btn-xs btn-primary gap-1"
                    >
                      <Save className="h-3 w-3" aria-hidden="true" />
                      保存
                    </button>
                  </div>
                </div>
              );
            })
          )}
        </div>

        <div className="flex items-center justify-end">
          <button type="button" onClick={onClose} className="btn btn-md btn-secondary">
            关闭
          </button>
        </div>
      </div>
    </Modal>
  );
}
