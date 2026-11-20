import { useId } from "react";
import { AlertTriangle, X } from "lucide-react";
import { Modal } from "./Modal";

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = "确认执行",
  cancelLabel = "取消",
  danger = false,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
}) {
  const titleId = useId();

  return (
    <Modal open={open} onClose={onCancel} labelledBy={titleId} maxWidthClass="max-w-md">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-3">
          <div
            className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-xl ${
              danger
                ? "bg-rose-500/12 text-rose-600 dark:text-rose-400"
                : "bg-amber-500/12 text-amber-600 dark:text-amber-400"
            }`}
          >
            <AlertTriangle className="h-4 w-4" aria-hidden="true" />
          </div>
          <div className="min-w-0">
            <h3 id={titleId} className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
              {title}
            </h3>
            <p className="mt-1 text-xs leading-relaxed text-zinc-600 dark:text-zinc-300">
              {description}
            </p>
          </div>
        </div>
        <button
          type="button"
          onClick={onCancel}
          aria-label="关闭确认弹窗"
          className="btn btn-sm btn-ghost shrink-0 p-1"
        >
          <X className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>

      <div className="mt-5 flex justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          className="btn btn-md btn-secondary shrink-0 whitespace-nowrap"
        >
          {cancelLabel}
        </button>
        <button
          type="button"
          onClick={() => void onConfirm()}
          className={`btn btn-md shrink-0 whitespace-nowrap ${
            danger ? "btn-danger" : "btn-primary"
          }`}
        >
          {confirmLabel}
        </button>
      </div>
    </Modal>
  );
}
