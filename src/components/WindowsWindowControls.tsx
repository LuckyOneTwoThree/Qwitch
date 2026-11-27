import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

/**
 * Windows 无边框一体化顶栏的右上角窗口控制按钮（最小化 / 最大化还原 / 关闭）。
 * 仅在 Windows 渲染；macOS 继续使用原生 Overlay 红绿灯。
 */
export function WindowsWindowControls() {
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    let unlisten: (() => void) | null = null;

    const syncMaximized = async () => {
      try {
        const maximized = await getCurrentWindow().isMaximized();
        if (active) setIsMaximized(maximized);
      } catch {
        /* ignore */
      }
    };

    void syncMaximized();
    void (async () => {
      try {
        const fn = await getCurrentWindow().onResized(() => {
          void syncMaximized();
        });
        if (active) {
          unlisten = fn;
        } else {
          fn();
        }
      } catch {
        /* ignore */
      }
    })();

    return () => {
      active = false;
      if (unlisten) unlisten();
    };
  }, []);

  const handleMinimize = () => {
    if (!isTauri()) return;
    void getCurrentWindow().minimize().catch(() => {});
  };

  const handleToggleMaximize = () => {
    if (!isTauri()) return;
    void (async () => {
      try {
        const win = getCurrentWindow();
        await win.toggleMaximize();
        setIsMaximized(await win.isMaximized());
      } catch {
        /* ignore */
      }
    })();
  };

  const handleClose = () => {
    if (!isTauri()) return;
    void getCurrentWindow().close().catch(() => {});
  };

  return (
    <div className="ml-1 flex shrink-0 items-center gap-0.5 pl-2">
      <span
        aria-hidden="true"
        className="mr-1.5 h-4 w-px bg-zinc-200 dark:bg-zinc-700/80"
      />
      <button
        type="button"
        onClick={handleMinimize}
        title="最小化"
        aria-label="最小化窗口"
        className="inline-flex h-7 w-9 items-center justify-center rounded-md text-zinc-600 transition hover:bg-zinc-200/80 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-700/70 dark:hover:text-zinc-100"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
          <path d="M1 5h8" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
        </svg>
      </button>
      <button
        type="button"
        onClick={handleToggleMaximize}
        title={isMaximized ? "还原" : "最大化"}
        aria-label={isMaximized ? "还原窗口" : "最大化窗口"}
        className="inline-flex h-7 w-9 items-center justify-center rounded-md text-zinc-600 transition hover:bg-zinc-200/80 hover:text-zinc-900 dark:text-zinc-300 dark:hover:bg-zinc-700/70 dark:hover:text-zinc-100"
      >
        {isMaximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <path
              d="M3 2.5h4.5V7M1.5 3.5h5v5h-5z"
              stroke="currentColor"
              strokeWidth="1"
              strokeLinejoin="round"
            />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <rect
              x="1.5"
              y="1.5"
              width="7"
              height="7"
              stroke="currentColor"
              strokeWidth="1"
              strokeLinejoin="round"
            />
          </svg>
        )}
      </button>
      <button
        type="button"
        onClick={handleClose}
        title="关闭"
        aria-label="关闭窗口"
        className="inline-flex h-7 w-9 items-center justify-center rounded-md text-zinc-600 transition hover:bg-[#e81123] hover:text-white dark:text-zinc-300 dark:hover:bg-[#e81123] dark:hover:text-white"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
          <path
            d="M1.5 1.5l7 7M8.5 1.5l-7 7"
            stroke="currentColor"
            strokeWidth="1"
            strokeLinecap="round"
          />
        </svg>
      </button>
    </div>
  );
}
