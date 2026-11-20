import { AlertCircle, AlertTriangle, CheckCircle2, Info, X } from "lucide-react";
import { useQwitch } from "../../context/QwitchContext";
import type { ToastTone } from "../../types/qwitch";

const TONE_STYLES: Record<
  ToastTone,
  {
    container: string;
    iconColor: string;
    Icon: typeof Info;
  }
> = {
  success: {
    container:
      "border-emerald-500/30 bg-zinc-900 text-white dark:border-emerald-400/30 dark:bg-zinc-100 dark:text-zinc-900",
    iconColor: "text-emerald-400 dark:text-emerald-600",
    Icon: CheckCircle2,
  },
  error: {
    container:
      "border-rose-500/40 bg-rose-950 text-rose-50 dark:border-rose-400/50 dark:bg-rose-950 dark:text-rose-50",
    iconColor: "text-rose-400",
    Icon: AlertCircle,
  },
  warning: {
    container:
      "border-amber-500/40 bg-amber-950 text-amber-50 dark:border-amber-400/40 dark:bg-amber-950 dark:text-amber-50",
    iconColor: "text-amber-400",
    Icon: AlertTriangle,
  },
  info: {
    container:
      "border-zinc-700 bg-zinc-900 text-white dark:border-zinc-300 dark:bg-zinc-100 dark:text-zinc-900",
    iconColor: "text-sky-400 dark:text-sky-600",
    Icon: Info,
  },
};

export function ToastViewport({ compact = false }: { compact?: boolean }) {
  const { toast, dismissToast } = useQwitch();
  if (!toast) return null;

  const { container, iconColor, Icon } = TONE_STYLES[toast.tone];

  return (
    <div
      role={toast.tone === "error" ? "alert" : "status"}
      aria-live={toast.tone === "error" ? "assertive" : "polite"}
      className={`fixed z-50 flex max-w-sm items-center gap-2 rounded-xl border px-3.5 py-2.5 text-xs font-medium shadow-xl ${
        compact ? "right-3 bottom-3 left-3" : "right-5 bottom-5"
      } ${container}`}
    >
      <Icon className={`h-4 w-4 shrink-0 ${iconColor}`} aria-hidden="true" />
      <span className="min-w-0 flex-1 leading-snug">{toast.message}</span>
      <button
        type="button"
        onClick={dismissToast}
        aria-label="关闭提示"
        className="shrink-0 rounded p-0.5 opacity-75 transition hover:opacity-100"
      >
        <X className="h-3.5 w-3.5" aria-hidden="true" />
      </button>
    </div>
  );
}
