import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Monitor, PanelTop, Moon, Sun } from "lucide-react";
import { QwitchProvider, useQwitch } from "./context/QwitchContext";
import { Dashboard } from "./pages/Dashboard";
import { TrayPopover } from "./pages/TrayPopover";
import { ToastViewport } from "./components/ui/ToastViewport";

function PrototypeShell() {
  const params = new URLSearchParams(window.location.search);
  const isTauriTrayWindow = params.get("view") === "tray";
  const isTauriRuntime = "__TAURI_INTERNALS__" in window;

  const [previewMode, setPreviewMode] = useState<"dashboard" | "tray">("dashboard");
  const { theme, toggleTheme } = useQwitch();

  if (isTauriTrayWindow) {
    return (
      <div className="relative h-screen w-screen">
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
    <div className="relative min-h-screen">
      {!isTauriRuntime && (
        <div className="sticky top-0 z-40 flex items-center justify-between border-b border-zinc-200 bg-zinc-100/90 px-4 py-1.5 text-xs backdrop-blur dark:border-zinc-800 dark:bg-zinc-900/90">
          <span className="font-medium text-zinc-600 dark:text-zinc-400">
            Qwitch 交互原型预览（Mock 数据模式）
          </span>
          <div className="flex items-center gap-2">
            <div className="inline-flex rounded-lg bg-zinc-200/80 p-0.5 dark:bg-zinc-800">
              <button
                type="button"
                onClick={() => setPreviewMode("dashboard")}
                className={`inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[11px] font-medium transition ${
                  previewMode === "dashboard"
                    ? "bg-white text-zinc-900 shadow-xs dark:bg-zinc-700 dark:text-zinc-100"
                    : "text-zinc-600 dark:text-zinc-400"
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
                    ? "bg-white text-zinc-900 shadow-xs dark:bg-zinc-700 dark:text-zinc-100"
                    : "text-zinc-600 dark:text-zinc-400"
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
              className="rounded-lg border border-zinc-200 bg-white p-1.5 text-zinc-600 hover:text-zinc-900 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
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
        <Dashboard />
      ) : (
        <div className="flex min-h-[calc(100vh-40px)] items-center justify-center bg-zinc-200/60 p-8 dark:bg-zinc-950">
          <div className="relative h-[420px] w-[340px] overflow-hidden rounded-2xl border border-zinc-200 shadow-2xl dark:border-zinc-800">
            <TrayPopover onOpenMain={() => setPreviewMode("dashboard")} />
          </div>
        </div>
      )}

      <ToastViewport compact={previewMode === "tray"} />
    </div>
  );
}

export default function App() {
  return (
    <QwitchProvider>
      <PrototypeShell />
    </QwitchProvider>
  );
}
