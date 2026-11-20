import { useCallback, useEffect, useRef, useState } from "react";
import { useToast } from "./useToast";
import { useAppUpdate } from "./useAppUpdate";
import type { BatchProgressState, SwitchDialogState } from "../types/qwitch";

interface UseFeedbackStoreArgs {
  isTauriRuntime: boolean;
}

/**
 * Feedback 域 store：toast、软件更新状态、批量进度、切号弹窗状态与自动收起。
 *
 * 四 store 拆分之一。`batchProgress`/`switchDialog` 作为可变状态暴露 setter，
 * 供 Account（主）store 的动作在批量/切号流程中驱动。
 */
export function useFeedbackStore({ isTauriRuntime }: UseFeedbackStoreArgs) {
  const { toast, showToast, dismissToast } = useToast();
  const { updateInfo, updateChecking, updateError, checkAppUpdate } = useAppUpdate(isTauriRuntime);
  const [batchProgress, setBatchProgress] = useState<BatchProgressState | null>(null);
  const [switchDialog, setSwitchDialog] = useState<SwitchDialogState>({
    account: null,
    phase: "confirm",
  });
  const batchDismissTimerRef = useRef<number | null>(null);

  const dismissBatchProgress = useCallback(() => {
    setBatchProgress(null);
  }, []);

  // 批量完成后自动收起进度条（失败文案停留更久）
  useEffect(() => {
    if (batchDismissTimerRef.current !== null) {
      window.clearTimeout(batchDismissTimerRef.current);
      batchDismissTimerRef.current = null;
    }
    if (!batchProgress || batchProgress.running) return;
    const summary = batchProgress.summaryText ?? "";
    const hasFailure = /异常\s+[1-9]/.test(summary) || summary.includes("失败");
    const delayMs = hasFailure ? 5000 : 3200;
    batchDismissTimerRef.current = window.setTimeout(() => {
      setBatchProgress(null);
      batchDismissTimerRef.current = null;
    }, delayMs);
    return () => {
      if (batchDismissTimerRef.current !== null) {
        window.clearTimeout(batchDismissTimerRef.current);
        batchDismissTimerRef.current = null;
      }
    };
  }, [batchProgress]);

  useEffect(() => {
    return () => {
      if (batchDismissTimerRef.current !== null) {
        window.clearTimeout(batchDismissTimerRef.current);
      }
    };
  }, []);

  return {
    toast,
    showToast,
    dismissToast,
    updateInfo,
    updateChecking,
    updateError,
    checkAppUpdate,
    batchProgress,
    setBatchProgress,
    switchDialog,
    setSwitchDialog,
    dismissBatchProgress,
  };
}
