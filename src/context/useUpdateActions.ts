import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useQwitch } from "../context/QwitchContext";

/**
 * 更新动作的统一实现（安装 / 打开下载页）。
 *
 * 背景：`UpdateCard`（设置页）、`UpdateBanner`（主窗口）、`TrayPopover`（托盘）
 * 曾各自实现一遍 `download_and_install_update` 与 `open_update_url` 调用，
 * 三份逻辑已经出现分叉（托盘版一度静默吞掉错误）。此处收敛为单一 hook，
 * 保证三条入口的错误处理与平台分支完全一致。
 *
 * 用法：
 * ```tsx
 * const { installing, runPrimaryAction, openDownloadPage } = useUpdateActions();
 * ```
 */
export function useUpdateActions() {
  const { updateInfo, showToast } = useQwitch();
  const [installing, setInstalling] = useState(false);

  /** 打开浏览器下载页（macOS 或不支持应用内安装时使用）。 */
  const openDownloadPage = useCallback(
    async (url?: string) => {
      const target = url ?? updateInfo?.assetUrl ?? updateInfo?.releaseUrl;
      if (!target) return;
      try {
        await invoke("open_update_url", { url: target });
      } catch (err) {
        showToast(`打开下载页失败：${String(err)}`, "error");
      }
    },
    [showToast, updateInfo]
  );

  /**
   * 主行为：能应用内安装则安装，否则打开下载页。
   *
   * 安装成功后进程会自行退出以完成覆盖安装，因此无需恢复 `installing`；
   * 仅在失败时复位并提示，避免用户反复重试却看不到原因。
   */
  const runPrimaryAction = useCallback(async () => {
    if (!updateInfo) return;

    const canAutoInstall =
      updateInfo.canAutoInstall && Boolean(updateInfo.assetUrl);

    if (!canAutoInstall) {
      await openDownloadPage();
      return;
    }

    if (installing) return;
    if (!updateInfo.assetUrl || !updateInfo.assetName || !updateInfo.checksumsUrl) {
      showToast("更新信息不完整，请前往设置页查看详情", "warning");
      return;
    }

    setInstalling(true);
    try {
      const message = await invoke<string>("download_and_install_update", {
        assetUrl: updateInfo.assetUrl,
        assetName: updateInfo.assetName,
        checksumsUrl: updateInfo.checksumsUrl,
      });
      showToast(message, "success");
    } catch (err) {
      setInstalling(false);
      showToast(`更新失败：${String(err)}`, "error");
    }
  }, [installing, openDownloadPage, showToast, updateInfo]);

  /** 主行为按钮的文案与无障碍标签，按平台/能力区分。 */
  const primaryLabel = updateInfo?.canAutoInstall
    ? `立即更新到 v${updateInfo?.latestVersion ?? ""}`
    : `下载 v${updateInfo?.latestVersion ?? ""}`;

  return { installing, runPrimaryAction, openDownloadPage, primaryLabel };
}
