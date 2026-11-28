import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FolderPlus, Loader2, PackagePlus, Puzzle, Trash2 } from "lucide-react";
import type { PluginItemDto } from "../../types/plugin";

const TRANSPORT_LABEL: Record<PluginItemDto["transport"], string> = {
  builtin: "内置",
  declarative: "声明式",
  subprocess: "子进程",
};

export function PluginCenterCard({
  isTauriRuntime,
  onToast,
  onPluginsChanged,
}: {
  isTauriRuntime: boolean;
  onToast: (msg: string, tone?: "success" | "error" | "info") => void;
  onPluginsChanged?: () => void;
}) {
  const [plugins, setPlugins] = useState<PluginItemDto[]>([]);
  const [loading, setLoading] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);

  const loadPlugins = useCallback(async () => {
    if (!isTauriRuntime) return;
    setLoading(true);
    try {
      const list = await invoke<PluginItemDto[]>("list_plugins_cmd");
      setPlugins(list);
    } catch (e) {
      onToast(String(e), "error");
    } finally {
      setLoading(false);
    }
  }, [isTauriRuntime, onToast]);

  useEffect(() => {
    void loadPlugins();
  }, [loadPlugins]);

  const handleInstallPath = async (sourcePath: string | null) => {
    if (!sourcePath) return;
    setInstalling(true);
    try {
      const installed = await invoke<PluginItemDto>("install_external_plugin_cmd", {
        sourcePath,
      });
      await loadPlugins();
      onPluginsChanged?.();
      onToast(`已安装插件 ${installed.name} (${installed.id})`, "success");
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("已取消")) {
        onToast(msg, "error");
      }
    } finally {
      setInstalling(false);
    }
  };

  const handleToggle = async (plugin: PluginItemDto) => {
    setBusyId(plugin.id);
    try {
      const next = await invoke<PluginItemDto[]>("toggle_plugin_cmd", {
        pluginId: plugin.id,
        enabled: !plugin.enabled,
      });
      setPlugins(next);
      onPluginsChanged?.();
      onToast(`${plugin.name} 已${!plugin.enabled ? "启用" : "停用"}`, "success");
    } catch (e) {
      onToast(String(e), "error");
    } finally {
      setBusyId(null);
    }
  };

  const handleRemove = async (plugin: PluginItemDto) => {
    setBusyId(plugin.id);
    try {
      const next = await invoke<PluginItemDto[]>("remove_external_plugin_cmd", {
        pluginId: plugin.id,
      });
      setPlugins(next);
      onPluginsChanged?.();
      onToast(
        plugin.overridesBuiltin
          ? `已卸载外部覆盖，${plugin.name} 已恢复为官方内置版本`
          : `已卸载外部插件 ${plugin.name}`,
        "success"
      );
    } catch (e) {
      onToast(String(e), "error");
    } finally {
      setBusyId(null);
    }
  };

  const egressList = plugins.filter((p) => p.kind === "egress" || p.kind === "hybrid");
  const ingressList = plugins.filter((p) => p.kind === "ingress" || p.kind === "hybrid");

  const renderGroup = (title: string, items: PluginItemDto[]) => (
    <div className="mt-4">
      <div className="mb-2 text-xs font-semibold text-zinc-700 dark:text-zinc-300">
        {title}（{items.length}）
      </div>
      <div className="grid grid-cols-1 gap-2 @md:grid-cols-2">
        {items.map((p) => (
          <div
            key={p.id}
            className="flex items-center justify-between gap-2.5 rounded-xl border border-zinc-100 px-3 py-2.5 text-xs dark:border-zinc-800"
          >
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="truncate font-semibold text-zinc-900 dark:text-zinc-100">
                  {p.name}
                </span>
                <span className="rounded bg-zinc-100 px-1.5 py-0.5 text-[10px] text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                  {TRANSPORT_LABEL[p.transport]}
                </span>
                {p.overridesBuiltin && (
                  <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] text-amber-700 dark:text-amber-300">
                    覆盖内置
                  </span>
                )}
              </div>
              <div className="mt-0.5 truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                {p.id} · v{p.version}
                {p.wireProtocols.length > 0 ? ` · ${p.wireProtocols.join(", ")}` : ""}
                {p.pinnedHash ? ` · #${p.pinnedHash.slice(0, 8)}` : ""}
              </div>
            </div>

            <div className="flex shrink-0 items-center gap-2">
              {p.source === "external" && (
                <button
                  type="button"
                  disabled={busyId === p.id}
                  onClick={() => void handleRemove(p)}
                  title={p.overridesBuiltin ? "卸载覆盖并恢复官方内置" : "卸载外部插件"}
                  className="inline-flex items-center gap-1 rounded-lg border border-rose-500/25 bg-rose-500/8 px-2 py-1 text-[11px] text-rose-700 hover:bg-rose-500/15 disabled:opacity-50 dark:text-rose-300"
                >
                  <Trash2 className="h-3 w-3" aria-hidden="true" />
                  {p.overridesBuiltin ? "恢复内置" : "卸载"}
                </button>
              )}
              <button
                type="button"
                role="switch"
                aria-checked={p.enabled}
                aria-label={`${p.enabled ? "停用" : "启用"}插件 ${p.name}`}
                disabled={busyId === p.id}
                onClick={() => void handleToggle(p)}
                className={`relative h-5 w-9 shrink-0 rounded-full transition disabled:opacity-50 ${
                  p.enabled ? "bg-emerald-500" : "bg-zinc-300 dark:bg-zinc-600"
                }`}
              >
                <span
                  className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${
                    p.enabled ? "left-4.5" : "left-0.5"
                  }`}
                />
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );

  return (
    <div className="surface-card p-5 @lg:col-span-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="flex items-center gap-2 text-sm font-semibold">
            <Puzzle className="h-4 w-4 text-sky-500" aria-hidden="true" />
            双端插件中心（出口订阅 + 入口 Agent）
          </div>
          <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
            内置工具与 Agent 均已插件化，可随时停用或导入自定义插件（支持 .zip、声明式 .json 或目录）。
          </p>
        </div>

        {isTauriRuntime && (
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              disabled={installing}
              onClick={async () => {
                const picked = await invoke<string | null>("pick_file", {
                  extensions: ["zip", "json"],
                });
                await handleInstallPath(picked);
              }}
              className="btn btn-sm btn-primary gap-1.5 whitespace-nowrap disabled:opacity-50"
            >
              {installing ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
              ) : (
                <PackagePlus className="h-3.5 w-3.5" aria-hidden="true" />
              )}
              导入 .zip / .json
            </button>
            <button
              type="button"
              disabled={installing}
              onClick={async () => {
                const picked = await invoke<string | null>("pick_folder", {});
                await handleInstallPath(picked);
              }}
              className="btn btn-sm btn-secondary gap-1.5 whitespace-nowrap disabled:opacity-50"
            >
              <FolderPlus className="h-3.5 w-3.5" aria-hidden="true" />
              导入目录
            </button>
          </div>
        )}
      </div>

      {!isTauriRuntime ? (
        <p className="mt-3 text-xs text-zinc-500 dark:text-zinc-400">
          当前处于浏览器预览模式，插件安装与动态启停请在桌面端运行。
        </p>
      ) : loading && plugins.length === 0 ? (
        <div className="mt-3 flex items-center gap-2 text-xs text-zinc-500">
          <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
          正在加载插件清单…
        </div>
      ) : (
        <>
          {renderGroup("出口插件（订阅 / 上游）", egressList)}
          {renderGroup("入口插件（Agent 接入）", ingressList)}
        </>
      )}
    </div>
  );
}
