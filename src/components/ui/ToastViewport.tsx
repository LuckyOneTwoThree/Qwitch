import { useEffect } from "react";
import { AlertCircle, AlertTriangle, CheckCircle2, Info, X } from "lucide-react";
import { useQwitch } from "../../context/QwitchContext";
import type { ToastTone } from "../../types/qwitch";

/**
 * Toast 使用「深色浮层 + 语义色图标」的统一语言：
 * 背景压深让提示从页面中浮起，语义色只出现在图标上，避免整块高饱和色带来的廉价感。
 */
const TONE_STYLES: Record<
  ToastTone,
  {
    container: string;
    iconColor: string;
    closeHover: string;
    Icon: typeof Info;
  }
> = {
  success: {
    container:
      "border-emerald-400/35 bg-zinc-900 text-zinc-50 dark:border-emerald-300/30 dark:bg-zinc-100 dark:text-zinc-900",
    iconColor: "text-emerald-400 dark:text-emerald-600",
    closeHover: "hover:bg-white/15 dark:hover:bg-black/10",
    Icon: CheckCircle2,
  },
  error: {
    container:
      "border-rose-500/45 bg-rose-950 text-rose-50 dark:border-rose-400/50 dark:bg-rose-950 dark:text-rose-50",
    iconColor: "text-rose-400",
    closeHover: "hover:bg-white/15",
    Icon: AlertCircle,
  },
  warning: {
    container:
      "border-amber-500/45 bg-amber-950 text-amber-50 dark:border-amber-400/45 dark:bg-amber-950 dark:text-amber-50",
    iconColor: "text-amber-400",
    closeHover: "hover:bg-white/15",
    Icon: AlertTriangle,
  },
  info: {
    container:
      "border-zinc-700 bg-zinc-900 text-zinc-50 dark:border-zinc-300 dark:bg-zinc-100 dark:text-zinc-900",
    iconColor: "text-sky-400 dark:text-sky-600",
    closeHover: "hover:bg-white/15 dark:hover:bg-black/10",
    Icon: Info,
  },
};

export function ToastViewport({
  compact = false,
  contained = false,
}: {
  compact?: boolean;
  contained?: boolean;
}) {
  const { toast, dismissToast } = useQwitch();

  useEffect(() => {
    if (!toast) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !document.querySelector('[role="dialog"], [role="alertdialog"]')) {
        dismissToast();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toast, dismissToast]);

  const toneStyle = toast ? TONE_STYLES[toast.tone] : null;
  const Icon = toneStyle?.Icon;

  return (
    <div
      aria-live={toast?.tone === "error" ? "assertive" : "polite"}
      aria-atomic="true"
      className={`pointer-events-none z-50 ${
        contained ? "absolute" : "fixed"
      } ${compact ? "right-3 bottom-14 left-3" : "right-5 bottom-5"}`}
    >
      {toast && toneStyle && Icon && (
        <div
          role={toast.tone === "error" ? "alert" : "status"}
          className={`pointer-events-auto flex max-w-sm animate-fade-up items-center gap-2 rounded-xl border px-3.5 py-2.5 text-xs font-medium shadow-float backdrop-blur-xl ${toneStyle.container}`}
        >
          <Icon className={`h-4 w-4 shrink-0 ${toneStyle.iconColor}`} aria-hidden="true" />
          <span className="min-w-0 flex-1 leading-snug">{toast.message}</span>
          <button
            type="button"
            onClick={dismissToast}
            aria-label="关闭提示"
            className={`shrink-0 rounded p-0.5 opacity-75 transition hover:opacity-100 ${toneStyle.closeHover}`}
          >
            <X className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>
      )}
    </div>
  );
}
