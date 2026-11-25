import { useState } from "react";
import { ArrowUpRight, Download, Loader2, Sparkles, X } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { useUpdateActions } from "../context/useUpdateActions";

/**
 * 主窗口顶部的「有新版本」提示条。
 *
 * 背景：更新入口此前只存在于设置页最底部（`SettingsView` 中的 `UpdateCard`），
 * 用户在日常使用路径（账号池 / 托盘）完全看不到更新提示。本组件把「发现新版本」
 * 这一状态提升到主窗口常驻可见的位置，点击后直接跳转设置页查看详情。
 *
 * 设计取舍：
 * - 仅在**确实有新版本**时渲染，避免常驻占位；
 * - 允许用户关闭（本次会话内不再提示），关闭状态不持久化——
 *   下次启动仍会提示，避免用户永久错过安全更新；
 * - 主按钮区分平台：Windows 可直接应用内安装，macOS 走浏览器下载页。
 */
export function UpdateBanner({
  onOpenSettings,
  dismissed: controlledDismissed,
  onDismiss,
}: {
  onOpenSettings: () => void;
  dismissed?: boolean;
  onDismiss?: () => void;
}) {
  const { isTauriRuntime, updateInfo: info } = useQwitch();
  const [localDismissed, setLocalDismissed] = useState(false);
  const dismissed = controlledDismissed ?? localDismissed;
  const handleDismiss = onDismiss ?? (() => setLocalDismissed(true));
  const {
    installing,
    runPrimaryAction,
    primaryLabel,
  } = useUpdateActions();

  // 仅在有新版本时出现；浏览器预览模式不提示
  if (!isTauriRuntime || !info?.updateAvailable || dismissed) return null;

  const canAutoInstall = info.canAutoInstall && Boolean(info.assetUrl);

  return (
    <div
      role="status"
      className="flex flex-wrap items-center gap-2 rounded-xl border border-brand-500/25 bg-brand-500/[0.07] px-3 py-2 text-xs text-brand-900 dark:border-brand-400/25 dark:bg-brand-400/10 dark:text-brand-100"
    >
      <Sparkles className="h-3.5 w-3.5 shrink-0 text-brand-600 dark:text-brand-300" aria-hidden="true" />
      <span className="min-w-0 flex-1 leading-snug">
        发现新版本{" "}
        <span className="font-mono font-semibold">v{info.latestVersion}</span>
        <span className="text-brand-800/75 dark:text-brand-200/75">
          {" "}
          · 当前 v{info.currentVersion}
        </span>
      </span>

      <button
        type="button"
        disabled={installing}
        onClick={() => void runPrimaryAction()}
        title={primaryLabel}
        aria-label={primaryLabel}
        className="btn btn-xs btn-primary shrink-0 gap-1 whitespace-nowrap"
      >
        {installing ? (
          <Loader2 className="h-3 w-3 shrink-0 animate-spin" aria-hidden="true" />
        ) : (
          <Download className="h-3 w-3 shrink-0" aria-hidden="true" />
        )}
        {installing ? "安装中..." : canAutoInstall ? "立即更新" : "下载"}
      </button>

      <button
        type="button"
        onClick={onOpenSettings}
        className="btn btn-xs btn-secondary shrink-0 gap-1 whitespace-nowrap"
      >
        详情
        <ArrowUpRight className="h-3 w-3 shrink-0" aria-hidden="true" />
      </button>

      <button
        type="button"
        onClick={handleDismiss}
        title="本次运行不再提示"
        aria-label="关闭更新提示"
        className="shrink-0 rounded-md p-1 text-brand-800/75 transition hover:bg-brand-500/10 hover:text-brand-950 dark:text-brand-200/75 dark:hover:text-brand-50"
      >
        <X className="h-3.5 w-3.5" aria-hidden="true" />
      </button>
    </div>
  );
}
