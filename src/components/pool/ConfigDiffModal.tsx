import { useId, useState } from "react";
import { AlertTriangle, FileDiff, Layers, Loader2, ShieldCheck, Star, X } from "lucide-react";
import { Modal } from "../ui/Modal";
import type { AgentBinding } from "../../types/pool";
import { AgentBrandIcon } from "../AgentBrandIcon";

export type DiffActionKind = "bind" | "repair" | "unbind" | "preview" | "update_mounts";

export function ConfigDiffModal({
  open,
  binding,
  actionKind,
  targetSummary,
  beforeText,
  afterText,
  onClose,
  onConfirm,
}: {
  open: boolean;
  binding: AgentBinding | null;
  actionKind: DiffActionKind;
  targetSummary?: {
    mountedPoolNames: string[];
    defaultModelDisplay?: string | null;
  };
  beforeText: string;
  afterText: string;
  onClose: () => void;
  onConfirm?: () => void | boolean | Promise<void | boolean>;
}) {
  const titleId = useId();
  const [confirming, setConfirming] = useState(false);

  if (!binding) return null;

  const isUnbind = actionKind === "unbind";
  const poolTitle = targetSummary?.mountedPoolNames.join(" + ") ?? "";
  const title =
    actionKind === "repair"
      ? `修复 ${binding.name} 配置漂移`
      : actionKind === "unbind"
        ? `无损解绑并还原 ${binding.name}`
        : actionKind === "bind" || actionKind === "update_mounts"
          ? `确认更新 ${binding.name} 号池挂载${poolTitle ? `（${poolTitle}）` : ""}`
          : `${binding.name} 配置文件变更预览`;

  const handleConfirm = async () => {
    if (!onConfirm || confirming) return;
    setConfirming(true);
    try {
      const ok = await onConfirm();
      if (ok !== false) {
        onClose();
      }
    } finally {
      setConfirming(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      labelledBy={titleId}
      disableClose={confirming}
      flush
      maxWidthClass="max-w-3xl"
    >
      <div className="flex items-start justify-between gap-3 border-b border-zinc-200 px-5 py-4 dark:border-zinc-800">
        <div className="flex items-center gap-2.5">
          <AgentBrandIcon agentId={binding.agentId} size="sm" />
          <div>
            <h2 id={titleId} className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
              {title}
            </h2>
            <p className="mt-0.5 font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
              目标路径：{binding.configPath}
            </p>
          </div>
        </div>
        <button
          type="button"
          disabled={confirming}
          onClick={onClose}
          aria-label="关闭配置对比弹窗"
          className="btn btn-icon btn-ghost"
        >
          <X className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>

      <div className="min-h-0 flex-1 space-y-3.5 overflow-y-auto px-5 py-4">
        {targetSummary && !isUnbind && (
          <div className="flex flex-wrap items-center gap-2 rounded-xl border border-zinc-200/80 bg-zinc-50/80 px-3 py-2 text-xs dark:border-zinc-800 dark:bg-zinc-900/60">
            <span className="inline-flex items-center gap-1 font-medium text-zinc-700 dark:text-zinc-300">
              <Layers className="h-3.5 w-3.5 text-brand-600 dark:text-brand-400" aria-hidden="true" />
              将挂载号池：
            </span>
            {targetSummary.mountedPoolNames.map((name) => (
              <span key={name} className="badge badge-brand">
                {name}
              </span>
            ))}
            {targetSummary.defaultModelDisplay && (
              <span className="ml-auto inline-flex items-center gap-1 rounded-md bg-amber-500/12 px-2 py-0.5 font-mono text-[11px] font-medium text-amber-800 dark:text-amber-300">
                <Star className="h-3 w-3" aria-hidden="true" />
                默认模型：{targetSummary.defaultModelDisplay}
              </span>
            )}
          </div>
        )}

        {binding.driftReason && actionKind === "repair" && (
          <div className="flex items-start gap-2 rounded-xl border border-amber-500/35 bg-amber-500/10 p-3 text-xs text-amber-900 dark:text-amber-200">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            <div>
              <div className="font-semibold">诊断原因</div>
              <p className="mt-0.5 opacity-90">{binding.driftReason}</p>
            </div>
          </div>
        )}

        <div className="flex items-start gap-2 rounded-xl border border-brand-500/25 bg-brand-500/[0.06] p-3 text-xs text-brand-900 dark:text-brand-200">
          <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-brand-600 dark:text-brand-400" aria-hidden="true" />
          <div>
            <div className="font-semibold">无损备份保护</div>
            <p className="mt-0.5 text-[11px] opacity-90">
              {isUnbind
                ? binding.agentId === "codex_desktop"
                  ? "将从加密备份恢复原始顶层 model_provider 与 model；为防止打开历史会话报错，[model_providers.qwitch] 表定义会安全保留。"
                  : "将从本地 AES-256-GCM 加密备份精确还原原始配置文件，不留残留项。"
                : "写入前自动加密备份原始配置，仅注入必要最小键，保留您的原有注释与其他设置，随时可一键无损还原。"}
            </p>
          </div>
        </div>

        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          <div className="flex flex-col overflow-hidden rounded-xl border border-zinc-200 dark:border-zinc-800">
            <div className="flex items-center justify-between border-b border-zinc-200 bg-zinc-100/80 px-3 py-1.5 text-[11px] font-semibold text-zinc-600 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-400">
              <span>{isUnbind ? "当前已注入配置" : "挂载前（原配置）"}</span>
              <span className="font-mono">BEFORE</span>
            </div>
            <pre className="flex-1 overflow-x-auto bg-zinc-50 p-3 font-mono text-[11px] leading-relaxed text-zinc-700 dark:bg-zinc-950 dark:text-zinc-300">
              {isUnbind ? afterText : beforeText}
            </pre>
          </div>

          <div className="flex flex-col overflow-hidden rounded-xl border border-brand-500/35 dark:border-brand-400/30">
            <div className="flex items-center justify-between border-b border-brand-500/25 bg-brand-500/10 px-3 py-1.5 text-[11px] font-semibold text-brand-800 dark:border-brand-400/25 dark:text-brand-200">
              <span className="flex items-center gap-1">
                <FileDiff className="h-3.5 w-3.5" aria-hidden="true" />
                {isUnbind ? "还原后配置" : "挂载后（多号池 / 模型目录注入）"}
              </span>
              <span className="font-mono">AFTER</span>
            </div>
            <pre className="flex-1 overflow-x-auto bg-zinc-50 p-3 font-mono text-[11px] leading-relaxed text-zinc-900 dark:bg-zinc-950 dark:text-zinc-100">
              {isUnbind ? beforeText : afterText}
            </pre>
          </div>
        </div>
      </div>

      <div className="flex shrink-0 items-center justify-end gap-2 border-t border-zinc-200 bg-zinc-50/70 px-5 py-3 dark:border-zinc-800 dark:bg-zinc-900/70">
        <button
          type="button"
          disabled={confirming}
          onClick={onClose}
          className="btn btn-md btn-secondary"
        >
          {onConfirm ? "取消" : "关闭"}
        </button>
        {onConfirm && (
          <button
            type="button"
            disabled={confirming}
            onClick={() => void handleConfirm()}
            className={`btn btn-md ${isUnbind ? "btn-danger" : "btn-primary"}`}
          >
            {confirming && <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />}
            {actionKind === "repair"
              ? "确认修复漂移"
              : actionKind === "unbind"
                ? "确认还原并解绑"
                : "确认写入配置"}
          </button>
        )}
      </div>
    </Modal>
  );
}
