import { useCallback, useEffect, useRef, useState } from "react";
import type { ToastState, ToastTone } from "../types/qwitch";

/** Toast 状态与自动消失计时器；卸载时清理定时器。 */
export function useToast() {
  const [toast, setToast] = useState<ToastState | null>(null);
  const toastTimerRef = useRef<number | null>(null);
  const toastSeqRef = useRef(0);

  useEffect(() => {
    return () => {
      if (toastTimerRef.current !== null) {
        window.clearTimeout(toastTimerRef.current);
      }
    };
  }, []);

  const dismissToast = useCallback(() => {
    if (toastTimerRef.current !== null) {
      window.clearTimeout(toastTimerRef.current);
      toastTimerRef.current = null;
    }
    setToast(null);
  }, []);

  const showToast = useCallback((message: string, tone: ToastTone = "info") => {
    if (toastTimerRef.current !== null) {
      window.clearTimeout(toastTimerRef.current);
    }
    const id = ++toastSeqRef.current;
    setToast({ id, message, tone });
    const durationMs = tone === "error" ? 6500 : tone === "warning" ? 4500 : 3400;
    toastTimerRef.current = window.setTimeout(() => {
      setToast((prev) => (prev && prev.id === id ? null : prev));
      toastTimerRef.current = null;
    }, durationMs);
  }, []);

  return { toast, showToast, dismissToast };
}
