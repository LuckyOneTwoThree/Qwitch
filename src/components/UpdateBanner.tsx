import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowUpRight, Download, Loader2, Sparkles, X } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";

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
export function UpdateBanner({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { isTauriRuntime, showToast, updateInfo: info } = useQwitch();
  const [dismissed, setDismissed] = useState(false);
  const [installing, setInstalling] = useState(false);

  const openUrl = useCallback(
    async (url?: string) => {
      if (!url) return;
      try {
        await invoke("open_update_url", { url });
      } catch (err) {
        showToast(`打开下载页失败：${String(err)}`, "error");
      }
    },
    [showToast]
  );

  const installOnWindows = useCallback(async () => {
    if (!info?.assetUrl || !info.assetName || !info.checksumsUrl || installing) return;
    setInstalling(true);
    try {
      const message = await invoke<string>("download_and_install_update", {
        assetUrl: info.assetUrl,
        assetName: info.assetName,
        checksumsUrl: info.checksumsUrl,
      });
      showToast(message, "success");
    } catch (err) {
      setInstalling(false);
      showToast(`更新失败：${String(err)}`, "error");
    }
  }, [info, installing, showToast]);

  // 仅在有新版本时出现；浏览器预览模式不提示
  if (!isTauriRuntime || !info?.updateAvailable || dismissed) return null;

  const canAutoInstall = info.canAutoInstall && Boolean(info.assetUrl);

  return (
    <div
      role="status"
      className="flex flex-wrap items-center gap-2 rounded-xl border border-indigo-500/25 bg-indigo-500/[0.07] px-3 py-2 text-xs text-indigo-900 dark:border-indigo-400/25 dark:bg-indigo-400/10 dark:text-indigo-100"
    >
      <Sparkles className="h-3.5 w-3.5 shrink-0 text-indigo-600 dark:text-indigo-300" aria-hidden="true" />
      <span className="min-w-0 flex-1 leading-snug">
        发现新版本{" "}
        <span className="font-mono font-semibold">v{info.latestVersion}</span>
        <span className="text-indigo-700/70 dark:text-indigo-200/70">
          {" "}
          · 当前 v{info.currentVersion}
        </span>
      </span>

      {canAutoInstall ? (
        <button
          type="button"
          disabled={installing}
          onClick={() => void installOnWindows()}
          className="btn btn-xs btn-primary shrink-0 gap-1 whitespace-nowrap"
        >
          {installing ? (
            <Loader2 className="h-3 w-3 shrink-0 animate-spin" aria-hidden="true" />
          ) : (
            <Download className="h-3 w-3 shrink-0" aria-hidden="true" />
          )}
          {installing ? "安装中..." : "立即更新"}
        </button>
      ) : (
        <button
          type="button"
          onClick={() => void openUrl(info.assetUrl ?? info.releaseUrl)}
          className="btn btn-xs btn-primary shrink-0 gap-1 whitespace-nowrap"
        >
          <Download className="h-3 w-3 shrink-0" aria-hidden="true" />
          下载
        </button>
      )}

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
        onClick={() => setDismissed(true)}
        title="本次运行不再提示"
        aria-label="关闭更新提示"
        className="shrink-0 rounded-md p-1 text-indigo-700/60 transition hover:bg-indigo-500/10 hover:text-indigo-900 dark:text-indigo-200/60 dark:hover:text-indigo-100"
      >
        <X className="h-3.5 w-3.5" aria-hidden="true" />
      </button>
    </div>
  );
}
