import { useEffect, useId, useRef, useState } from "react";
import { ArrowRight, Check, FileCode2, X } from "lucide-react";
import { Modal } from "../ui/Modal";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import { WIRE_PROTOCOL_META } from "../../data/mockPoolData";
import type { AccountPool, AgentBinding, TargetAgentId } from "../../types/pool";
import { AgentBrandIcon } from "../AgentBrandIcon";

export function BindAgentModal({
  open,
  pool,
  onClose,
  onPreviewDiff,
}: {
  open: boolean;
  pool: AccountPool | null;
  onClose: () => void;
  onPreviewDiff?: (binding: AgentBinding, nextPoolIds: string[]) => void;
}) {
  const titleId = useId();
  const { agentBindings, mountPoolToAgents } = usePoolPrototype();
  const [selectedAgents, setSelectedAgents] = useState<TargetAgentId[]>([]);
  const bindingsRef = useRef(agentBindings);
  bindingsRef.current = agentBindings;
  const poolId = pool?.id;

  useEffect(() => {
    if (!open || !poolId) return;
    setSelectedAgents(
      bindingsRef.current
        .filter((b) => b.mountedPoolIds.includes(poolId))
        .map((b) => b.agentId)
    );
  }, [open, poolId]);

  if (!pool) return null;

  const toggle = (id: TargetAgentId) => {
    setSelectedAgents((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]
    );
  };

  const handleApply = () => {
    const toMount = selectedAgents.filter((id) => {
      const b = agentBindings.find((item) => item.agentId === id);
      return b && !b.mountedPoolIds.includes(pool.id);
    });
    if (toMount.length > 0) {
      mountPoolToAgents(pool.id, toMount);
    }
    onClose();
  };

  const pendingNewCount = selectedAgents.filter((id) => {
    const b = agentBindings.find((item) => item.agentId === id);
    return b && !b.mountedPoolIds.includes(pool.id);
  }).length;

  return (
    <Modal open={open} onClose={onClose} labelledBy={titleId} maxWidthClass="max-w-xl">
      <div className="flex flex-col">
        <div className="flex items-start justify-between gap-3 border-b border-zinc-200 px-5 py-4 dark:border-zinc-800">
          <div>
            <h2 id={titleId} className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
              将「{pool.name}」挂载至消费端 Agent
            </h2>
            <p className="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">
              1 个 Agent 支持同时挂载多个号池；追加挂载会保留该 Agent 已有号池，并在下拉清单列出本池启用的{" "}
              {pool.modelPolicies.filter((m) => m.enabled).length} 个模型
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭指向 Agent 弹窗"
            className="btn btn-icon btn-ghost"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        <div className="space-y-2.5 px-5 py-4">
          {agentBindings.map((b) => {
            const checked = selectedAgents.includes(b.agentId);
            const alreadyMounted = b.mountedPoolIds.includes(pool.id);
            const nextPoolIds = alreadyMounted
              ? b.mountedPoolIds
              : [...b.mountedPoolIds, pool.id];

            return (
              <div
                key={b.agentId}
                className={`flex items-center justify-between gap-3 rounded-xl border p-3 transition ${
                  checked
                    ? "border-brand-500/60 bg-brand-50/40 dark:border-brand-400/50 dark:bg-brand-500/10"
                    : "border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900/60"
                }`}
              >
                <div className="flex min-w-0 flex-1 items-center gap-3">
                  <button
                    type="button"
                    onClick={() => toggle(b.agentId)}
                    aria-label={`指向 ${b.name}`}
                    className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border transition ${
                      checked
                        ? "border-brand-600 bg-brand-600 text-white dark:border-brand-400 dark:bg-brand-400 dark:text-zinc-950"
                        : "border-zinc-300 bg-white dark:border-zinc-600 dark:bg-zinc-800"
                    }`}
                  >
                    {checked && <Check className="h-3 w-3" aria-hidden="true" />}
                  </button>

                  <AgentBrandIcon agentId={b.agentId} size="sm" />

                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                        {b.name}
                      </span>
                      <span className={`badge ${WIRE_PROTOCOL_META[b.protocol].badgeClass}`}>
                        {WIRE_PROTOCOL_META[b.protocol].label}
                      </span>
                      {alreadyMounted ? (
                        <span className="badge border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300">
                          已挂载本池
                        </span>
                      ) : b.mountedPoolIds.length > 0 ? (
                        <span className="badge badge-neutral">
                          已挂 {b.mountedPoolIds.length} 池（将保留）
                        </span>
                      ) : null}
                    </div>
                    <div className="mt-0.5 truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                      {b.configPath}
                    </div>
                  </div>
                </div>

                {onPreviewDiff && (
                  <button
                    type="button"
                    onClick={() => onPreviewDiff(b, nextPoolIds)}
                    className="btn btn-xs btn-secondary shrink-0 gap-1"
                  >
                    <FileCode2 className="h-3 w-3" aria-hidden="true" />
                    预览 Diff
                  </button>
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
            disabled={selectedAgents.length === 0}
            onClick={handleApply}
            className="btn btn-md btn-primary gap-1.5"
          >
            {pendingNewCount > 0 ? `追加挂载到 ${pendingNewCount} 个 Agent` : "确认完成"}
            <ArrowRight className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>
      </div>
    </Modal>
  );
}
