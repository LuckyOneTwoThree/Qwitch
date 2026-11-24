import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Monitor, PanelTop, Moon, Sun } from "lucide-react";
import { QwitchProvider, useQwitch } from "./context/QwitchContext";
import { PoolPrototypeProvider } from "./context/usePoolPrototypeStore";
import { Dashboard } from "./pages/Dashboard";
import { TrayPopover } from "./pages/TrayPopover";
import { ToastViewport } from "./components/ui/ToastViewport";

function PrototypeShell() {
  const params = new URLSearchParams(window.location.search);
  const isTauriTrayWindow = params.get("view") === "tray";
  const isTauriRuntime = "__TAURI_INTERNALS__" in window;

  const [previewMode, setPreviewMode] = useState<"dashboard" | "tray">("dashboard");
  const { theme, toggleTheme } = useQwitch();

  useEffect(() => {
    document.documentElement.classList.toggle("tray-window", isTauriTrayWindow);
    return () => {
      document.documentElement.classList.remove("tray-window");
    };
  }, [isTauriTrayWindow]);

  if (isTauriTrayWindow) {
    return (
      <div className="relative h-screen w-screen bg-transparent p-2 pt-1 pb-2.5">
        <TrayPopover
          onOpenMain={() => {
            void invoke("show_main_window");
          }}
        />
        <ToastViewport compact />
      </div>
    );
  }

  return (
    <div className="relative flex h-screen flex-col overflow-hidden">
      {!isTauriRuntime && (
        <div className="z-40 flex shrink-0 items-center justify-between border-b border-zinc-200 bg-zinc-100/80 px-4 py-1.5 text-xs backdrop-blur-xl dark:border-zinc-800 dark:bg-zinc-900/80">
          <span className="font-medium text-zinc-500 dark:text-zinc-400">
            Qwitch 浏览器预览模式（非桌面端运行时）
          </span>
          <div className="flex items-center gap-2">
            <div className="inline-flex rounded-lg bg-zinc-200/70 p-0.5 dark:bg-zinc-800">
              <button
                type="button"
                onClick={() => setPreviewMode("dashboard")}
                className={`inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[11px] font-medium transition ${
                  previewMode === "dashboard"
                    ? "bg-white text-zinc-900 shadow-card dark:bg-zinc-700 dark:text-zinc-100"
                    : "text-zinc-500 transition hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-100"
                }`}
              >
                <Monitor className="h-3 w-3" aria-hidden="true" />
                主窗口 Dashboard
              </button>
              <button
                type="button"
                onClick={() => setPreviewMode("tray")}
                className={`inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[11px] font-medium transition ${
                  previewMode === "tray"
                    ? "bg-white text-zinc-900 shadow-card dark:bg-zinc-700 dark:text-zinc-100"
                    : "text-zinc-500 transition hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-100"
                }`}
              >
                <PanelTop className="h-3 w-3" aria-hidden="true" />
                系统托盘 Popover
              </button>
            </div>
            <button
              type="button"
              onClick={toggleTheme}
              title="切换深色/浅色主题"
              aria-label="切换深色或浅色主题"
              className="btn btn-icon btn-secondary"
            >
              {theme === "dark" ? (
                <Sun className="h-3.5 w-3.5" aria-hidden="true" />
              ) : (
                <Moon className="h-3.5 w-3.5" aria-hidden="true" />
              )}
            </button>
          </div>
        </div>
      )}

      {previewMode === "dashboard" ? (
        <div className="min-h-0 flex-1 overflow-hidden">
          <Dashboard />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto bg-zinc-100 p-8 dark:bg-zinc-950">
          <div className="relative h-[500px] w-[384px] p-2 pt-1 pb-2.5">
            <TrayPopover onOpenMain={() => setPreviewMode("dashboard")} />
          </div>
        </div>
      )}

      <ToastViewport compact={previewMode === "tray"} />
    </div>
  );
}

function AppPoolBridge({ children }: { children: React.ReactNode }) {
  const { showToast } = useQwitch();
  return <PoolPrototypeProvider onToast={showToast}>{children}</PoolPrototypeProvider>;
}

export default function App() {
  return (
    <QwitchProvider>
      <AppPoolBridge>
        <PrototypeShell />
      </AppPoolBridge>
    </QwitchProvider>
  );
}
