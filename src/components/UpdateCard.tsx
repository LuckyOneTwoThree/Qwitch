import { useCallback, useEffect } from "react";
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
import { useUpdateActions } from "../context/useUpdateActions";

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
  const {
    isTauriRuntime,
    showToast,
    updateInfo: info,
    updateChecking: checking,
    updateError: error,
    checkAppUpdate,
  } = useQwitch();
  const { installing, runPrimaryAction, openDownloadPage } = useUpdateActions();

  const runCheck = useCallback(
    async (manual: boolean) => {
      if (!isTauriRuntime) {
        if (manual) {
          showToast("浏览器预览模式无法检查桌面端更新", "info");
        }
        return;
      }
      const result = await checkAppUpdate({ force: manual, silent: false });
      if (manual && result) {
        showToast(
          result.updateAvailable
            ? `发现新版本 v${result.latestVersion}`
            : `已是最新版本 v${result.currentVersion}`,
          result.updateAvailable ? "info" : "success"
        );
      }
    },
    [checkAppUpdate, isTauriRuntime, showToast]
  );

  useEffect(() => {
    if (!isTauriRuntime) return;
    void runCheck(false);
  }, [isTauriRuntime, runCheck]);

  const currentVersion = info?.currentVersion ?? __APP_VERSION__;
  const publishedAt = formatTime(info?.publishedAt);
  const assetSize = formatSize(info?.assetSize);
  const notes = info?.releaseNotes?.trim();

  return (
    <div className="surface-card p-5 @lg:col-span-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="flex items-center gap-2 text-sm font-semibold">
            <Sparkles className="h-4 w-4 text-brand-600 dark:text-brand-400" aria-hidden="true" />
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
          className="btn btn-sm btn-secondary shrink-0 gap-1"
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
        <div
          role="alert"
          className="mt-3 flex items-start gap-2 rounded-xl border border-rose-500/20 bg-rose-500/5 p-3 text-xs text-rose-700 dark:text-rose-300"
        >
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
            <span className="badge badge-neutral">{info.platformLabel}</span>
            {info.assetName && (
              <span className="truncate font-mono" title={info.assetName}>
                {info.assetName}
              </span>
            )}
            {assetSize && <span>{assetSize}</span>}
            {publishedAt && <span>发布于 {publishedAt}</span>}
          </div>

          {notes && (
            <div className="max-h-40 overflow-auto whitespace-pre-wrap rounded-xl bg-zinc-50 p-3 text-xs leading-relaxed text-zinc-600 dark:bg-zinc-800/60 dark:text-zinc-300">
              {notes}
            </div>
          )}

          <div className="flex flex-wrap items-center gap-2">
            {info.strategy === "windows_installer" ? (
              info.assetUrl && info.checksumsUrl ? (
                <button
                  type="button"
                  disabled={installing}
                  onClick={() => void runPrimaryAction()}
                  className="btn btn-sm btn-primary shrink-0 gap-1.5"
                >
                  {installing ? (
                    <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden="true" />
                  ) : (
                    <Download className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  )}
                  {installing ? "正在下载安装包..." : `下载并安装 v${info.latestVersion}`}
                </button>
              ) : (
                <span className="inline-flex items-center rounded-lg border border-amber-500/25 bg-amber-500/10 px-2.5 py-1.5 text-xs text-amber-800 dark:text-amber-300">
                  未提供 SHA-256 校验清单，自动安装已禁用，请点击「查看 Release」手动下载并校验
                </span>
              )
            ) : (
              <button
                type="button"
                disabled={!info.assetUrl}
                title={!info.assetUrl ? "当前平台未匹配到直接安装包，请点击「查看 Release」手动下载" : undefined}
                onClick={() => void openDownloadPage(info.assetUrl)}
                className="btn btn-sm btn-primary shrink-0 gap-1.5"
              >
                <Download className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                下载 {info.platformLabel} 安装包
              </button>
            )}
            <button
              type="button"
              onClick={() => void openDownloadPage(info.releaseUrl)}
              className="btn btn-sm btn-secondary shrink-0 gap-1"
            >
              <ArrowUpRight className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
              查看 Release
            </button>
          </div>

          <p className="rounded-xl bg-amber-500/5 p-3 text-xs leading-relaxed text-zinc-600 dark:text-zinc-300">
            {info.hint}
          </p>

          {info.strategy === "macos_dmg" && (
            <ol className="list-decimal space-y-1 pl-5 text-xs leading-relaxed text-zinc-600 dark:text-zinc-400">
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
