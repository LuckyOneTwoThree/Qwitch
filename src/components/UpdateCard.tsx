import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  AlertCircle,
  ArrowUpRight,
  CheckCircle2,
  Download,
  Loader2,
  RefreshCw,
  Sparkles,
} from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import type { AppUpdateInfo } from "../types/qwitch";
import packageJson from "../../package.json";

function formatSize(bytes?: number): string | null {
  if (!bytes || bytes <= 0) return null;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function formatTime(iso?: string): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? null : d.toLocaleString("zh-CN");
}

export function UpdateCard() {
  const { isTauriRuntime, showToast } = useQwitch();
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [info, setInfo] = useState<AppUpdateInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  const runCheck = useCallback(
    async (manual: boolean) => {
      if (!isTauriRuntime) {
        if (manual) {
          showToast("浏览器预览模式无法检查桌面端更新", "info");
        }
        return;
      }
      setChecking(true);
      setError(null);
      try {
        const result = await invoke<AppUpdateInfo>("check_app_update");
        setInfo(result);
        if (manual) {
          showToast(
            result.updateAvailable
              ? `发现新版本 v${result.latestVersion}`
              : `已是最新版本 v${result.currentVersion}`,
            result.updateAvailable ? "info" : "success"
          );
        }
      } catch (err) {
        setError(String(err));
      } finally {
        setChecking(false);
      }
    },
    [isTauriRuntime, showToast]
  );

  useEffect(() => {
    if (!isTauriRuntime) return;
    void runCheck(false);
  }, [isTauriRuntime, runCheck]);

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
    if (!info?.assetUrl || !info.assetName || installing) return;
    setInstalling(true);
    try {
      const message = await invoke<string>("download_and_install_update", {
        assetUrl: info.assetUrl,
        assetName: info.assetName,
        checksumsUrl: info.checksumsUrl ?? null,
      });
      showToast(message, "success");
    } catch (err) {
      setInstalling(false);
      showToast(`更新失败：${String(err)}`, "error");
    }
  }, [info, installing, showToast]);

  const currentVersion = info?.currentVersion ?? packageJson.version;
  const publishedAt = formatTime(info?.publishedAt);
  const assetSize = formatSize(info?.assetSize);
  const notes = info?.releaseNotes?.trim();

  return (
    <div className="rounded-2xl border border-zinc-200 bg-white p-5 lg:col-span-2 dark:border-zinc-800 dark:bg-zinc-900">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="flex items-center gap-2 text-sm font-semibold">
            <Sparkles className="h-4 w-4 text-indigo-500" aria-hidden="true" />
            软件更新
          </div>
          <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
            当前版本 <span className="font-mono">v{currentVersion}</span>
            {info && !info.updateAvailable ? " · 已是最新版本" : ""}
            {info?.updateAvailable ? (
              <>
                {" "}
                · 最新版本{" "}
                <span className="font-mono text-emerald-600 dark:text-emerald-400">
                  v{info.latestVersion}
                </span>
              </>
            ) : null}
          </p>
        </div>
        <button
          type="button"
          disabled={checking || installing}
          onClick={() => void runCheck(true)}
          className="inline-flex shrink-0 items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:opacity-50 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
        >
          {checking ? (
            <Loader2 className="h-3 w-3 shrink-0 animate-spin" aria-hidden="true" />
          ) : (
            <RefreshCw className="h-3 w-3 shrink-0" aria-hidden="true" />
          )}
          {checking ? "检查中..." : "检查更新"}
        </button>
      </div>

      {!isTauriRuntime && (
        <p className="mt-3 rounded-xl bg-zinc-50 p-3 text-xs text-zinc-600 dark:bg-zinc-800/60 dark:text-zinc-300">
          浏览器预览模式不发起更新检查；桌面端会读取 GitHub Releases 并分平台提供升级方案。
        </p>
      )}

      {error && (
        <div className="mt-3 flex items-start gap-2 rounded-xl border border-rose-500/20 bg-rose-500/5 p-3 text-xs text-rose-700 dark:text-rose-300">
          <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          <div className="min-w-0">
            <div className="font-medium">检查更新失败</div>
            <div className="mt-0.5 break-all leading-relaxed">{error}</div>
          </div>
        </div>
      )}

      {info?.updateAvailable && (
        <div className="mt-4 space-y-3">
          <div className="flex flex-wrap items-center gap-2 text-[11px] text-zinc-500 dark:text-zinc-400">
            <span className="rounded-full bg-zinc-100 px-2 py-0.5 dark:bg-zinc-800">
              {info.platformLabel}
            </span>
            {info.assetName && (
              <span className="truncate font-mono" title={info.assetName}>
                {info.assetName}
              </span>
            )}
            {assetSize && <span>{assetSize}</span>}
            {publishedAt && <span>发布于 {publishedAt}</span>}
          </div>

          {notes && (
            <div className="max-h-40 overflow-auto whitespace-pre-wrap rounded-xl bg-zinc-50 p-3 text-[11px] leading-relaxed text-zinc-600 dark:bg-zinc-800/60 dark:text-zinc-300">
              {notes}
            </div>
          )}

          <div className="flex flex-wrap items-center gap-2">
            {info.strategy === "windows_installer" ? (
              <button
                type="button"
                disabled={installing || !info.assetUrl}
                onClick={() => void installOnWindows()}
                className="inline-flex shrink-0 items-center gap-1.5 rounded-lg bg-zinc-900 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:opacity-60 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
              >
                {installing ? (
                  <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden="true" />
                ) : (
                  <Download className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                )}
                {installing ? "正在下载安装包..." : `下载并安装 v${info.latestVersion}`}
              </button>
            ) : (
              <button
                type="button"
                disabled={!info.assetUrl}
                onClick={() => void openUrl(info.assetUrl)}
                className="inline-flex shrink-0 items-center gap-1.5 rounded-lg bg-zinc-900 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-zinc-800 disabled:opacity-60 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
              >
                <Download className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                下载 {info.platformLabel} 安装包
              </button>
            )}
            <button
              type="button"
              onClick={() => void openUrl(info.releaseUrl)}
              className="inline-flex shrink-0 items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1.5 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
            >
              <ArrowUpRight className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
              查看 Release
            </button>
          </div>

          <p className="rounded-xl bg-amber-500/5 p-3 text-[11px] leading-relaxed text-zinc-600 dark:text-zinc-300">
            {info.hint}
          </p>

          {info.strategy === "macos_dmg" && (
            <ol className="list-decimal space-y-1 pl-5 text-[11px] leading-relaxed text-zinc-600 dark:text-zinc-400">
              <li>下载与芯片架构匹配的 DMG（Apple Silicon 选 aarch64，Intel 选 x64）。</li>
              <li>退出 Qwitch，打开 DMG，将 Qwitch 拖入「应用程序」并覆盖旧版本。</li>
              <li>
                首次打开如被拦截，执行{" "}
                <code className="rounded bg-zinc-100 px-1 py-0.5 font-mono dark:bg-zinc-800">
                  xattr -cr /Applications/Qwitch.app
                </code>
                ，或在「隐私与安全性」中选择「仍要打开」。
              </li>
            </ol>
          )}
        </div>
      )}

      {info && !info.updateAvailable && !error && (
        <div className="mt-3 flex items-center gap-2 rounded-xl border border-emerald-500/20 bg-emerald-500/5 p-3 text-xs text-emerald-700 dark:text-emerald-300">
          <CheckCircle2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          当前已是最新版本，无需升级。
        </div>
      )}
    </div>
  );
}
