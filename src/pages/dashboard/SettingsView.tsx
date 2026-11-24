import { useEffect, useMemo, useState } from "react";
import {
  RefreshCw,
  ShieldCheck,
  Puzzle,
  FolderSearch,
  Save,
  Archive,
  AppWindow,
  Timer,
  Network,
  Loader2,
  CheckCircle2,
  XCircle,
} from "lucide-react";

type SettingsTabId = "general" | "automation" | "clients" | "plugins";

const SETTINGS_TABS: ReadonlyArray<{
  id: SettingsTabId;
  label: string;
  desc: string;
}> = [
  { id: "general", label: "常规与外观", desc: "主题、菜单栏常驻与自启动" },
  { id: "automation", label: "自动化与日志", desc: "定时巡检、防风控延迟与日志归档" },
  { id: "clients", label: "客户端环境", desc: "桌面端进程探测与路径覆盖" },
  { id: "plugins", label: "插件与关于", desc: "适配器清单、外部插件与版本更新" },
];
import { useQwitch } from "../../context/QwitchContext";
import { UpdateCard } from "../../components/UpdateCard";
import { ConfirmDialog } from "../../components/ui/ConfirmDialog";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import { ToolBrandIcon } from "../../components/ToolBrandIcon";
import { getToolBrandMeta, getVisibleTools } from "../../domain/toolRegistry";
import type { Dispatch, SetStateAction } from "react";
import type { ExternalAdapterInfo } from "../../types/qwitch";
import type { ConfirmLogAction } from "./types";

function SettingToggleRow({
  id,
  label,
  description,
  checked,
  onChange,
}: {
  id: string;
  label: string;
  description: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-3 rounded-xl border border-zinc-100 bg-zinc-50/70 px-3 py-2.5 dark:border-zinc-800 dark:bg-zinc-800/40">
      <label htmlFor={id} className="min-w-0 flex-1 cursor-pointer">
        <div className="text-xs font-medium text-zinc-900 dark:text-zinc-100">{label}</div>
        <p className="mt-0.5 text-xs leading-snug text-zinc-600 dark:text-zinc-400">
          {description}
        </p>
      </label>
      <button
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`relative mt-0.5 inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors ${
          checked
            ? "bg-brand-700 dark:bg-brand-400"
            : "bg-zinc-300 dark:bg-zinc-700"
        }`}
      >
        <span
          className={`inline-block h-4 w-4 transform rounded-full shadow-xs transition-transform ${
            checked
              ? "translate-x-4 bg-white dark:bg-zinc-900"
              : "translate-x-0.5 bg-white dark:bg-zinc-200"
          }`}
        />
      </button>
    </div>
  );
}

export interface SettingsViewProps {
  autoStartDescription: string;
  externalPlugins: ExternalAdapterInfo[];
  trustedDraft: string[];
  setTrustedDraft: Dispatch<SetStateAction<string[]>>;
  pathDrafts: Record<string, string>;
  setPathDrafts: Dispatch<SetStateAction<Record<string, string>>>;
  setPathDraftsDirty: Dispatch<SetStateAction<boolean>>;
  reloadExternalPlugins: () => Promise<void>;
  setConfirmLogAction: Dispatch<SetStateAction<ConfirmLogAction | null>>;
}

export function SettingsView({
  autoStartDescription,
  externalPlugins,
  trustedDraft,
  setTrustedDraft,
  pathDrafts,
  setPathDrafts,
  setPathDraftsDirty,
  reloadExternalPlugins,
  setConfirmLogAction,
}: SettingsViewProps) {
  const {
    adapters,
    appSettings,
    clientStatuses,
    isTauriRuntime,
    jitterRangeMs,
    setJitterRangeMs,
    logRetention,
    setLogRetention,
    refreshClientStatuses,
    saveAppSettings,
    showToast,
  } = useQwitch();
  const [activeTab, setActiveTab] = useState<SettingsTabId>("general");
  const {
    gateway,
    agentBindings,
    updateGatewayPort,
    checkGatewayPort,
    restartRunningDesktopAgents,
  } = usePoolPrototype();
  const [portInput, setPortInput] = useState(String(gateway.port));
  const [portChecking, setPortChecking] = useState(false);
  const [portApplying, setPortApplying] = useState(false);
  const [portCheck, setPortCheck] = useState<{
    available: boolean;
    inUseBySelf: boolean;
    message: string;
  } | null>(null);
  const [restartConfirmOpen, setRestartConfirmOpen] = useState(false);
  const runningDesktopAgents = useMemo(
    () =>
      agentBindings.filter(
        (b) =>
          ["claude_desktop", "codex_desktop", "mimo", "zcode"].includes(b.agentId) &&
          b.running
      ),
    [agentBindings]
  );
  useEffect(() => {
    setPortInput(String(gateway.port));
  }, [gateway.port]);
  const orderedAdapters = useMemo(() => {
    if (adapters.length === 0) return [];
    const byId = new Map(adapters.map((ad) => [ad.id, ad] as const));
    return getVisibleTools(adapters)
      .map((entry) => byId.get(entry.id))
      .filter((ad): ad is NonNullable<typeof ad> => Boolean(ad));
  }, [adapters]);
  const runningClientsCount = useMemo(
    () =>
      orderedAdapters.filter(
        (ad) => ad.capabilities.supportsClientSwitch && clientStatuses[ad.id]?.isRunning
      ).length,
    [orderedAdapters, clientStatuses]
  );

  return (
    <div className="space-y-4">
      <nav
        aria-label="设置分类导航"
        className="surface-card flex flex-wrap items-center justify-between gap-2 p-2"
      >
        <div role="tablist" aria-label="设置分类" className="flex flex-wrap items-center gap-1">
          {SETTINGS_TABS.map((tab) => {
            const selected = activeTab === tab.id;
            return (
              <button
                key={tab.id}
                type="button"
                role="tab"
                aria-selected={selected}
                onClick={() => setActiveTab(tab.id)}
                className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
                  selected
                    ? "bg-brand-600 text-white shadow-2xs dark:bg-brand-500 dark:text-zinc-950"
                    : "text-zinc-600 hover:bg-zinc-100 dark:text-zinc-400 dark:hover:bg-zinc-800/80"
                }`}
              >
                <span>{tab.label}</span>
                {tab.id === "clients" && runningClientsCount > 0 && (
                  <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
                )}
              </button>
            );
          })}
        </div>
        <span className="hidden px-2 text-[11px] text-zinc-500 @xl:inline dark:text-zinc-400">
          {SETTINGS_TABS.find((t) => t.id === activeTab)?.desc}
        </span>
      </nav>

      {activeTab === "general" && (
        <div className="surface-card p-5">
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center gap-2 text-sm font-semibold">
              <AppWindow className="h-4 w-4 text-sky-500" aria-hidden="true" />
              桌面常驻与启动偏好
            </div>
            <span className="badge badge-neutral">即时生效</span>
          </div>
          <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
            控制关闭主窗口时的常驻行为及系统登录自动启动策略。
          </p>

          <div className="mt-4 space-y-2.5">
            <div className="rounded-xl border border-zinc-100 bg-zinc-50/70 px-3 py-2.5 dark:border-zinc-800 dark:bg-zinc-800/40">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <div className="text-xs font-medium text-zinc-900 dark:text-zinc-100">
                    外观主题
                  </div>
                  <p className="mt-0.5 text-xs text-zinc-600 dark:text-zinc-400">
                    支持始终浅色、深色或随系统外观自动切换。
                  </p>
                </div>
                <div className="flex flex-wrap items-center gap-1">
                  {[
                    { label: "跟随系统", value: "system" as const },
                    { label: "浅色", value: "light" as const },
                    { label: "深色", value: "dark" as const },
                  ].map((opt) => {
                    const selected = appSettings.theme === opt.value;
                    return (
                      <button
                        key={opt.value}
                        type="button"
                        aria-pressed={selected}
                        onClick={() => void saveAppSettings({ theme: opt.value }, true)}
                        className={`btn btn-xs ${
                          selected ? "btn-primary" : "btn-secondary"
                        }`}
                      >
                        {opt.label}
                      </button>
                    );
                  })}
                </div>
              </div>
            </div>
            <SettingToggleRow
              id="qwitch-close-to-tray"
              label="关闭主窗口时隐藏至菜单栏常驻"
              description="开启后点击关闭按钮将隐藏主窗口并收起程序坞/任务栏图标，仅保留顶部菜单栏常驻；关闭则直接退出 Qwitch。"
              checked={appSettings.closeToTray !== false}
              onChange={(next) => void saveAppSettings({ closeToTray: next })}
            />
            <SettingToggleRow
              id="qwitch-launch-at-login"
              label="开机自动启动"
              description={autoStartDescription}
              checked={Boolean(appSettings.launchAtLogin)}
              onChange={(next) => void saveAppSettings({ launchAtLogin: next })}
            />
            <SettingToggleRow
              id="qwitch-start-minimized"
              label="启动时静默驻留菜单栏"
              description="手动打开应用时默认不弹出主窗口，直接隐身于菜单栏，减少对桌面的打扰。"
              checked={Boolean(appSettings.startMinimized)}
              onChange={(next) => void saveAppSettings({ startMinimized: next })}
            />
          </div>
        </div>
      )}

      {activeTab === "automation" && (
        <div className="grid grid-cols-1 gap-4 @lg:grid-cols-2">
          <div className="surface-card p-5 @lg:col-span-2">
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-2 text-sm font-semibold">
                <Timer className="h-4 w-4 text-indigo-500" aria-hidden="true" />
                自动化巡检与每日签到策略
              </div>
              <span className="badge badge-neutral">即时生效</span>
            </div>
            <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
              主窗口可见时按单队列防风控节奏静默核实额度、同步跨天签到状态；隐藏到托盘后将暂停周期性刷新。
            </p>

            <div className="mt-4 space-y-2.5">
              <SettingToggleRow
                id="qwitch-auto-refresh-startup"
                label="启动时自动同步额度与签到状态"
                description="应用启动完成初始化后，自动在后台串行刷新一次已授权账号的最新额度与跨天签到状态。"
                checked={Boolean(appSettings.autoRefreshOnStartup)}
                onChange={(next) => void saveAppSettings({ autoRefreshOnStartup: next })}
              />
              <SettingToggleRow
                id="qwitch-auto-claim-daily"
                label="发现待领福利时自动后台签到"
                description="结合启动巡检或后台定时刷新，若确认账号今日可领 (Available)，按防风控抖动延迟自动领取。"
                checked={Boolean(appSettings.autoClaimDaily)}
                onChange={(next) => {
                  if (next) {
                    setConfirmLogAction({
                      label: "启用自动后台签到？",
                      detail:
                        "开启后将在启动巡检或后台定时刷新发现可领福利时自动发起领取，属于无人值守敏感动作，可能增加平台风控概率。确认启用？",
                      danger: true,
                      run: async () => {
                        await saveAppSettings({ autoClaimDaily: true });
                      },
                    });
                  } else {
                    void saveAppSettings({ autoClaimDaily: false });
                  }
                }}
              />

              <div className="rounded-xl border border-zinc-100 bg-zinc-50/70 px-3 py-2.5 dark:border-zinc-800 dark:bg-zinc-800/40">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <div className="text-xs font-medium text-zinc-900 dark:text-zinc-100">
                      后台定时刷新间隔
                    </div>
                    <p className="mt-0.5 text-xs text-zinc-600 dark:text-zinc-400">
                      主窗口保持可见期间周期性检查额度与次日新签到活动（隐藏到托盘后自动暂停）。
                    </p>
                  </div>
                  <div className="flex flex-wrap items-center gap-1">
                    {[
                      { label: "关闭", value: 0 },
                      { label: "15 分钟", value: 15 },
                      { label: "30 分钟", value: 30 },
                      { label: "60 分钟", value: 60 },
                      { label: "120 分钟", value: 120 },
                    ].map((opt) => {
                      const current = appSettings.autoRefreshIntervalMinutes ?? 0;
                      const selected = current === opt.value;
                      return (
                        <button
                          key={opt.value}
                          type="button"
                          onClick={() =>
                            void saveAppSettings({
                              autoRefreshIntervalMinutes: opt.value,
                            })
                          }
                          className={`chip ${
                            selected ? "chip-active" : ""
                          }`}
                        >
                          {opt.label}
                        </button>
                      );
                    })}
                  </div>
                </div>
              </div>
            </div>
          </div>

          <div className="surface-card p-5">
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-2 text-sm font-semibold">
                <ShieldCheck className="h-4 w-4 text-emerald-500" aria-hidden="true" />
                防风控串行调度参数
              </div>
              <button
                type="button"
                onClick={() =>
                  void saveAppSettings({
                    jitterMinMs: jitterRangeMs[0],
                    jitterMaxMs: jitterRangeMs[1],
                  })
                }
                className="btn btn-xs btn-primary shrink-0 gap-1 whitespace-nowrap"
              >
                <Save className="h-3 w-3 shrink-0" aria-hidden="true" />
                保存配置
              </button>
            </div>
            <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
              批量查额与每日领取均强制单队列串行，并在账号间加入随机抖动延迟。
            </p>
            <div className="mt-4 grid grid-cols-2 gap-3">
              <div>
                <label
                  htmlFor="qwitch-jitter-min"
                  className="mb-1 block text-xs text-zinc-600 dark:text-zinc-400"
                >
                  最小间隔 (ms)
                </label>
                <input
                  id="qwitch-jitter-min"
                  type="number"
                  min={0}
                  max={60000}
                  value={jitterRangeMs[0]}
                  onChange={(e) => {
                    const parsed = Number(e.target.value);
                    const next = Number.isFinite(parsed)
                      ? Math.min(60000, Math.max(0, Math.floor(parsed)))
                      : 0;
                    setJitterRangeMs([next, jitterRangeMs[1]]);
                  }}
                  className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                />
              </div>
              <div>
                <label
                  htmlFor="qwitch-jitter-max"
                  className="mb-1 block text-xs text-zinc-600 dark:text-zinc-400"
                >
                  最大间隔 (ms)
                </label>
                <input
                  id="qwitch-jitter-max"
                  type="number"
                  min={0}
                  max={60000}
                  value={jitterRangeMs[1]}
                  onChange={(e) => {
                    const parsed = Number(e.target.value);
                    const next = Number.isFinite(parsed)
                      ? Math.min(60000, Math.max(0, Math.floor(parsed)))
                      : 0;
                    setJitterRangeMs([jitterRangeMs[0], next]);
                  }}
                  className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                />
              </div>
            </div>
          </div>

          <div className="surface-card p-5">
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-2 text-sm font-semibold">
                <Archive className="h-4 w-4 text-amber-500" aria-hidden="true" />
                日志保留与自动归档策略
              </div>
              <button
                type="button"
                onClick={() =>
                  void saveAppSettings({
                    logArchiveAfterDays: Math.min(
                      3650,
                      Math.max(0, Math.floor(logRetention.archiveAfterDays))
                    ),
                    logMaxActiveCount: Math.min(
                      100000,
                      Math.max(0, Math.floor(logRetention.maxActiveCount))
                    ),
                  })
                }
                className="btn btn-xs btn-primary shrink-0 gap-1 whitespace-nowrap"
              >
                <Save className="h-3 w-3 shrink-0" aria-hidden="true" />
                保存并执行
              </button>
            </div>
            <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
              启动及保存时自动将超期或超出上限的旧日志移入归档，填 0 表示不限制。
            </p>
            <div className="mt-4 grid grid-cols-2 gap-3">
              <div>
                <label
                  htmlFor="qwitch-log-days"
                  className="mb-1 block text-xs text-zinc-600 dark:text-zinc-400"
                >
                  自动归档天数（天）
                </label>
                <input
                  id="qwitch-log-days"
                  type="number"
                  min={0}
                  max={3650}
                  value={logRetention.archiveAfterDays}
                  onChange={(e) => {
                    const parsed = Number(e.target.value);
                    const next = Number.isFinite(parsed)
                      ? Math.min(3650, Math.max(0, Math.floor(parsed)))
                      : 0;
                    setLogRetention({
                      ...logRetention,
                      archiveAfterDays: next,
                    });
                  }}
                  className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                />
              </div>
              <div>
                <label
                  htmlFor="qwitch-log-max"
                  className="mb-1 block text-xs text-zinc-600 dark:text-zinc-400"
                >
                  活跃日志上限（条）
                </label>
                <input
                  id="qwitch-log-max"
                  type="number"
                  min={0}
                  max={100000}
                  value={logRetention.maxActiveCount}
                  onChange={(e) => {
                    const parsed = Number(e.target.value);
                    const next = Number.isFinite(parsed)
                      ? Math.min(100000, Math.max(0, Math.floor(parsed)))
                      : 0;
                    setLogRetention({
                      ...logRetention,
                      maxActiveCount: next,
                    });
                  }}
                  className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
                />
              </div>
            </div>
          </div>
        </div>
      )}

      {activeTab === "clients" && (
        <div className="surface-card p-5">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <div className="flex items-center gap-2 text-sm font-semibold">
                <FolderSearch className="h-4 w-4 text-indigo-500" aria-hidden="true" />
                本机客户端环境探测与路径覆盖
              </div>
              <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                若目标桌面客户端安装在非标准目录，可在此配置自定义可执行路径（.app 或 .exe），将同步作用于进程检测、解密与重启。
              </p>
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void refreshClientStatuses()}
                className="btn btn-sm btn-secondary shrink-0 gap-1 whitespace-nowrap"
              >
                <RefreshCw className="h-3 w-3 shrink-0" aria-hidden="true" />
                重新探测
              </button>
              <button
                type="button"
                onClick={async () => {
                  const ok = await saveAppSettings({ customClientPaths: pathDrafts });
                  if (ok) {
                    setPathDraftsDirty(false);
                    await refreshClientStatuses();
                  }
                }}
                className="btn btn-sm btn-primary shrink-0 gap-1 whitespace-nowrap"
              >
                <Save className="h-3 w-3 shrink-0" aria-hidden="true" />
                保存路径覆盖
              </button>
            </div>
          </div>

          <div className="mt-4 rounded-xl border border-zinc-200 bg-zinc-50/70 p-3.5 dark:border-zinc-800 dark:bg-zinc-800/40">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-2 text-sm font-semibold">
                <Network className="h-4 w-4 text-emerald-500" aria-hidden="true" />
                本地网关端口与占用检测
              </div>
              <code className="rounded bg-white px-2 py-1 font-mono text-[11px] tabular-nums text-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">
                {gateway.host}:{gateway.port}
              </code>
            </div>
            <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
              监听地址固定 127.0.0.1，端口范围 1024–65535。修改会自动重启网关并重新同步所有已挂载
              Agent 的 baseURL；桌面客户端需重启后生效。
            </p>
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <input
                type="number"
                min={1024}
                max={65535}
                value={portInput}
                onChange={(e) => {
                  setPortInput(e.target.value);
                  setPortCheck(null);
                }}
                aria-label="网关监听端口"
                className="w-40 rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 font-mono text-sm text-zinc-900 focus:border-brand-400 focus:outline-none dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100"
              />
              <button
                type="button"
                disabled={portChecking}
                onClick={async () => {
                  setPortChecking(true);
                  try {
                    setPortCheck(await checkGatewayPort(Number.parseInt(portInput, 10)));
                  } finally {
                    setPortChecking(false);
                  }
                }}
                className="btn btn-sm btn-secondary gap-1"
              >
                {portChecking ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
                检测占用
              </button>
              <button
                type="button"
                disabled={portApplying}
                onClick={async () => {
                  setPortApplying(true);
                  try {
                    const res = await updateGatewayPort(Number.parseInt(portInput, 10));
                    if (res.ok) {
                      if (runningDesktopAgents.length > 0) {
                        setRestartConfirmOpen(true);
                      } else {
                        showToast("端口已更新；当前没有运行中的桌面 Agent", "info");
                      }
                    } else {
                      setPortCheck({
                        available: false,
                        inUseBySelf: false,
                        message: res.message ?? "更新失败",
                      });
                    }
                  } finally {
                    setPortApplying(false);
                  }
                }}
                className="btn btn-sm btn-primary gap-1"
              >
                {portApplying ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
                应用端口
              </button>
            </div>
            {portCheck && (
              <div
                className={`mt-2.5 flex items-start gap-2 rounded-lg border px-3 py-2 text-xs ${
                  portCheck.available
                    ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-800 dark:text-emerald-300"
                    : "border-rose-500/30 bg-rose-500/10 text-rose-800 dark:text-rose-300"
                }`}
              >
                {portCheck.available ? (
                  <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                ) : (
                  <XCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                )}
                <span>
                  {portCheck.message}
                  {portCheck.inUseBySelf ? "（当前网关自身）" : ""}
                </span>
              </div>
            )}
          </div>

          <div className="mt-4 space-y-3">
            {orderedAdapters
              .filter((ad) => ad.capabilities.supportsClientSwitch)
              .map((ad) => {
                const status = clientStatuses[ad.id];
                return (
                  <div
                    key={ad.id}
                    className="rounded-xl border border-zinc-100 bg-zinc-50/70 p-3.5 text-xs dark:border-zinc-800 dark:bg-zinc-800/40"
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex items-center gap-2.5">
                        <ToolBrandIcon toolId={ad.id} size="sm" decorative />
                        <span className="font-semibold text-zinc-900 dark:text-zinc-100">
                          {ad.name}
                        </span>
                        <span className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                          {ad.id}
                        </span>
                      </div>
                      <span
                        className={`badge gap-1.5 ${
                          status?.isRunning
                            ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                            : "badge-neutral"
                        }`}
                      >
                        {status?.isRunning && (
                          <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                        )}
                        {status?.isRunning ? "桌面客户端运行中" : "未运行"}
                      </span>
                    </div>

                    <div className="mt-2.5 grid grid-cols-1 gap-1.5 text-xs text-zinc-600 dark:text-zinc-400 @md:grid-cols-2">
                      <div className="truncate">
                        <span className="text-zinc-500 dark:text-zinc-400">探测应用：</span>
                        <span className="font-mono text-zinc-800 dark:text-zinc-200">
                          {status?.executablePath ?? "未检测到安装路径"}
                        </span>
                      </div>
                      <div className="truncate">
                        <span className="text-zinc-500 dark:text-zinc-400">会话目录：</span>
                        <span className="font-mono text-zinc-800 dark:text-zinc-200">
                          {status?.configDir ?? "未知"}
                        </span>
                      </div>
                    </div>

                    <div className="mt-2.5">
                      <input
                        type="text"
                        value={pathDrafts[ad.id] ?? ""}
                        aria-label={`${ad.name} 自定义客户端路径`}
                        onChange={(e) => {
                          setPathDrafts((prev) => ({ ...prev, [ad.id]: e.target.value }));
                          setPathDraftsDirty(true);
                        }}
                        placeholder="自定义客户端路径（留空使用自动探测路径）"
                        className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-1.5 font-mono text-[11px] text-zinc-900 focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100"
                      />
                    </div>
                  </div>
                );
              })}
          </div>
        </div>
      )}

      {activeTab === "plugins" && (
        <div className="grid grid-cols-1 gap-4 @lg:grid-cols-2">
          <div className="surface-card p-5 @lg:col-span-2">
            <div className="flex items-center gap-2 text-sm font-semibold">
              <Puzzle className="h-4 w-4 text-sky-500" aria-hidden="true" />
              已注册适配器（内置 + 外部扩展）
            </div>
            <div className="mt-3 grid grid-cols-1 gap-2.5 @md:grid-cols-3">
              {orderedAdapters.map((ad) => (
                <div
                  key={ad.id}
                  className="flex items-center justify-between gap-3 rounded-xl border border-zinc-100 px-3 py-2.5 text-xs dark:border-zinc-800"
                >
                  <div className="flex min-w-0 items-center gap-2.5">
                    <ToolBrandIcon toolId={ad.id} size="sm" decorative />
                    <div className="min-w-0">
                      <div className="truncate font-semibold text-zinc-900 dark:text-zinc-100">
                        {ad.name}
                      </div>
                      <div className="truncate text-xs text-zinc-500 dark:text-zinc-400">
                        {getToolBrandMeta(ad.id, adapters).tagline}
                      </div>
                    </div>
                  </div>
                  <span
                    className="badge badge-neutral shrink-0"
                    title={ad.capabilities.supportsDailyClaim ? "查额与签到" : "仅查额与切号"}
                  >
                    v{ad.version}
                  </span>
                </div>
              ))}
            </div>
          </div>

          {isTauriRuntime && (
            <div className="surface-card p-5 @lg:col-span-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <div className="flex items-center gap-2 text-sm font-semibold">
                    <ShieldCheck className="h-4 w-4 text-emerald-500" aria-hidden="true" />
                    外部插件信任管理
                  </div>
                  <p className="mt-1 text-xs text-zinc-600 dark:text-zinc-400">
                    外部适配器仅在显式启用后才会加载并接收账号凭据；保存后需重启 Qwitch 生效。内置与保留 ID 不可被外部插件冒用。
                  </p>
                </div>
                <button
                  type="button"
                  onClick={async () => {
                    const hashesDraft: Record<string, string> = {};
                    for (const id of trustedDraft) {
                      const plugin = externalPlugins.find((p) => p.id === id);
                      if (plugin?.contentHash) {
                        hashesDraft[id] = plugin.contentHash;
                      }
                    }
                    const ok = await saveAppSettings(
                      {
                        trustedAdapters: trustedDraft,
                        trustedAdapterHashes: hashesDraft,
                      },
                      true
                    );
                    if (ok) {
                      await reloadExternalPlugins();
                      showToast("插件信任设置已保存，重启 Qwitch 后生效", "success");
                    }
                  }}
                  className="btn btn-sm btn-primary shrink-0 gap-1 whitespace-nowrap"
                >
                  <Save className="h-3 w-3 shrink-0" aria-hidden="true" />
                  保存信任设置
                </button>
              </div>
              {externalPlugins.length === 0 ? (
                <p className="mt-3 text-xs text-zinc-500 dark:text-zinc-400">
                  未发现外部插件。将插件目录放入 Qwitch 数据目录的 adapters/ 下后重新进入本页即可看到。
                </p>
              ) : (
                <div className="mt-3 space-y-2">
                  {externalPlugins.map((plugin) => {
                    const enabled = trustedDraft.includes(plugin.id);
                    return (
                      <div
                        key={plugin.id}
                        className="flex items-center justify-between gap-3 rounded-xl border border-zinc-100 px-3 py-2.5 text-xs dark:border-zinc-800"
                      >
                        <div className="min-w-0">
                          <div className="truncate font-semibold text-zinc-900 dark:text-zinc-100">
                            {plugin.name}
                            <span className="ml-2 font-normal text-zinc-500 dark:text-zinc-400">
                              {plugin.id} · v{plugin.version}
                            </span>
                          </div>
                          {plugin.reserved && (
                            <div className="mt-0.5 text-xs text-rose-600 dark:text-rose-400">
                              该 ID 为内置/历史保留，已被禁止加载
                            </div>
                          )}
                          {!plugin.reserved && plugin.hashMismatch && (
                            <div className="mt-0.5 text-xs text-amber-600 dark:text-amber-400">
                              插件目录内容已变更或尚未固定校验哈希，当前已被阻止加载；确认安全后请重新点击「保存信任设置」
                            </div>
                          )}
                        </div>
                        <button
                          type="button"
                          role="switch"
                          aria-checked={enabled}
                          aria-label={`${enabled ? "停用" : "启用"}插件 ${plugin.name}`}
                          disabled={plugin.reserved}
                          onClick={() =>
                            setTrustedDraft((prev) =>
                              enabled ? prev.filter((id) => id !== plugin.id) : [...prev, plugin.id]
                            )
                          }
                          className={`relative h-5 w-9 shrink-0 rounded-full transition disabled:cursor-not-allowed disabled:opacity-40 ${
                            enabled ? "bg-emerald-500" : "bg-zinc-300 dark:bg-zinc-600"
                          }`}
                        >
                          <span
                            className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${
                              enabled ? "left-4.5" : "left-0.5"
                            }`}
                          />
                        </button>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          )}

          <UpdateCard />
        </div>
      )}

      <ConfirmDialog
        open={restartConfirmOpen}
        title="重启运行中的桌面 Agent？"
        description={`网关端口已更新，是否立即重启以下 ${runningDesktopAgents.length} 个运行中的桌面 Agent 以加载新地址：${runningDesktopAgents
          .map((b) => b.name)
          .join("、")}。重启会关闭并重新打开对应应用。`}
        confirmLabel="立即重启"
        onCancel={() => setRestartConfirmOpen(false)}
        onConfirm={async () => {
          setRestartConfirmOpen(false);
          await restartRunningDesktopAgents();
        }}
      />
    </div>
  );
}
