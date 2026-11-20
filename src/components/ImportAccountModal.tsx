import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Copy, FileArchive, FolderOpen, Globe, HardDriveDownload, KeyRound, Loader2, X } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { getVisibleTools } from "../domain/toolRegistry";
import { ToolBrandIcon } from "./ToolBrandIcon";
import { Modal } from "./ui/Modal";
import type { AccountSummary, AuthMethod, ProfileInspectResult, ToolId } from "../types/qwitch";

type DevicePlatform = "mac" | "win";

function detectImportPlatform(): DevicePlatform {
  if (typeof navigator !== "undefined") {
    const p = `${navigator.platform ?? ""} ${navigator.userAgent ?? ""}`;
    if (/Win/i.test(p)) return "win";
  }
  return "mac";
}

export function ImportAccountModal({  open,
  onClose,
  targetAccount,
  defaultToolId,
  defaultMethod,
}: {
  open: boolean;
  onClose: () => void;
  targetAccount?: AccountSummary | null;
  defaultToolId?: ToolId;
  defaultMethod?: AuthMethod;
}) {
  const { adapters, importAccount, pickFolder, pickFile, inspectProfileDir, showToast } =
    useQwitch();
  const titleId = useId();
  const visibleTools = useMemo(() => getVisibleTools(adapters), [adapters]);

  const [toolId, setToolId] = useState<ToolId>("trae_cn");
  const [label, setLabel] = useState("");
  const [method, setMethod] = useState<AuthMethod>("local_import");
  const [rawInput, setRawInput] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [profileMode, setProfileMode] = useState<"default" | "device">("default");
  const [profileDir, setProfileDir] = useState("");
  const [devicePlatform, setDevicePlatform] = useState<DevicePlatform>(detectImportPlatform);
  const [inspectResult, setInspectResult] = useState<ProfileInspectResult | null>(null);
  const [inspectError, setInspectError] = useState<string | null>(null);
  const [inspecting, setInspecting] = useState(false);

  const wasOpenRef = useRef(false);
  const lastTargetIdRef = useRef<string | undefined>(undefined);

  const getSupportedMethods = (tid: ToolId): AuthMethod[] => {
    const adapter = adapters.find((a) => a.id === tid);
    const declared = adapter?.capabilities.authMethods ?? ["local_import", "webview_oauth"];
    const set = new Set<AuthMethod>(declared);
    // 所有内置适配器均实现 parse_manual_token，默认开放手动 Token/JSON 粘贴补充通道
    if (adapter?.isBuiltin ?? true) {
      set.add("token_paste");
    }
    return Array.from(set);
  };

  // 仅在弹窗打开瞬间或切换 targetAccount 时初始化表单，避免后台 syncFromBackend 更新 adapters 引用重置用户输入
  useEffect(() => {
    const justOpened = open && !wasOpenRef.current;
    const targetChanged = open && targetAccount?.id !== lastTargetIdRef.current;
    wasOpenRef.current = open;

    if (!open) {
      lastTargetIdRef.current = undefined;
      setInspectResult(null);
      setInspectError(null);
      setInspecting(false);
      return;
    }

    if (!justOpened && !targetChanged) {
      return;
    }

    lastTargetIdRef.current = targetAccount?.id;
    setSubmitting(false);
    setLabel("");
    setRawInput("");
    setProfileMode("default");
    setProfileDir("");
    setInspectResult(null);
    setInspectError(null);
    setInspecting(false);
    setDevicePlatform(detectImportPlatform());

    if (targetAccount) {
      setToolId(targetAccount.toolId);
      const methods = getSupportedMethods(targetAccount.toolId);
      if (targetAccount.hasApiToken === false && methods.includes("webview_oauth")) {
        setMethod("webview_oauth");
      } else if (targetAccount.hasLocalSnapshot === false && methods.includes("local_import")) {
        setMethod("local_import");
      } else {
        setMethod(methods[0] ?? "local_import");
      }
      return;
    }

    const fallbackTool = visibleTools[0]?.id ?? "trae_cn";
    const initialTool =
      defaultToolId && visibleTools.some((t) => t.id === defaultToolId)
        ? defaultToolId
        : fallbackTool;
    setToolId(initialTool);
    const methods = getSupportedMethods(initialTool);
    if (defaultMethod && methods.includes(defaultMethod)) {
      setMethod(defaultMethod);
    } else if (methods.includes("local_import")) {
      setMethod("local_import");
    } else {
      setMethod(methods[0] ?? "webview_oauth");
    }
  }, [open, targetAccount, defaultToolId, defaultMethod, visibleTools]);

  // 若后台适配器列表更新导致当前选中方式不再受支持，仅校正导入方式，不清空用户输入的别名或 Token
  useEffect(() => {
    if (!open) return;
    const methods = getSupportedMethods(toolId);
    if (methods.length > 0 && !methods.includes(method)) {
      setMethod(methods[0]);
    }
  }, [open, toolId, method, adapters]);

  const supportedMethods = getSupportedMethods(toolId);
  // 目前仅 Trae CN 支持“隔离设备 profile”多开签到；后续可依 adapter 能力位扩展
  const supportsDeviceProfile = toolId === "trae_cn";

  // 设备目录变化时做只读预检（是否含 storage.json / 设备 ID / 是否与本机相同）
  useEffect(() => {
    if (!open || !supportsDeviceProfile || method !== "local_import" || profileMode !== "device") {
      setInspectResult(null);
      setInspectError(null);
      setInspecting(false);
      return;
    }
    const dir = profileDir.trim();
    if (!dir) {
      setInspectResult(null);
      setInspectError(null);
      setInspecting(false);
      return;
    }
    let cancelled = false;
    setInspectResult(null);
    setInspectError(null);
    setInspecting(true);
    const timer = window.setTimeout(async () => {
      try {
        const res = await inspectProfileDir(toolId, dir);
        if (!cancelled) {
          setInspectResult(res);
          setInspectError(null);
        }
      } catch (err) {
        if (!cancelled) {
          setInspectResult(null);
          setInspectError(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (!cancelled) setInspecting(false);
      }
    }, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [open, supportsDeviceProfile, method, profileMode, profileDir, toolId, inspectProfileDir]);

  const handlePickFolder = async () => {
    try {
      const picked = await pickFolder(profileDir.trim() || undefined);
      if (picked) setProfileDir(picked);
    } catch (err) {
      showToast(`选择文件夹失败：${String(err)}`, "error");
    }
  };

  const handlePickFile = async () => {
    try {
      const picked = await pickFile(profileDir.trim() || undefined);
      if (picked) setProfileDir(picked);
    } catch (err) {
      showToast(`选择导出包失败：${String(err)}`, "error");
    }
  };

  const exportCommand = (os: DevicePlatform): string =>
    os === "win"
      ? `$d="$env:APPDATA\\Trae CN"; if(-not (Test-Path "$d\\User\\globalStorage\\storage.json")){Write-Error "未找到 storage.json"; return}; $w="$env:TEMP\\TraeCN-profile"; $t="$w\\Trae CN"; Remove-Item -Recurse -Force $w -ErrorAction SilentlyContinue; try { New-Item -ItemType Directory -Force "$t\\User\\globalStorage","$t\\aha" | Out-Null; Copy-Item "$d\\User\\globalStorage\\storage.json" "$t\\User\\globalStorage\\" -Force; if(Test-Path "$d\\User\\globalStorage\\state.vscdb"){Copy-Item "$d\\User\\globalStorage\\state.vscdb" "$t\\User\\globalStorage\\" -Force}; if(Test-Path "$d\\aha"){Copy-Item "$d\\aha\\*" "$t\\aha\\" -Recurse -Force}; $o="$env:USERPROFILE\\Desktop\\TraeCN-profile.zip"; if(Test-Path $o){Remove-Item $o}; Compress-Archive -Path "$t\\*" -DestinationPath $o -Force; Write-Host "已导出 -> $o" } finally { Remove-Item -Recurse -Force $w -ErrorAction SilentlyContinue }`
      : `D="$HOME/Library/Application Support/Trae CN"; W="$(mktemp -d)"; T="$W/Trae CN"; trap 'rm -rf "$W"' EXIT; [ -f "$D/User/globalStorage/storage.json" ] || { echo "未找到 storage.json"; exit 1; }; mkdir -p "$T/User/globalStorage" "$T/aha"; cp "$D/User/globalStorage/storage.json" "$T/User/globalStorage/" && ([ -f "$D/User/globalStorage/state.vscdb" ] && cp "$D/User/globalStorage/state.vscdb" "$T/User/globalStorage/" || true) && ([ -d "$D/aha" ] && cp -R "$D/aha/." "$T/aha/" || true) && O="$HOME/Desktop/TraeCN-profile.zip" && rm -f "$O" && ditto -c -k --sequesterRsrc "$T" "$O" && echo "已导出 -> $O"`;

  const handleCopyExport = async () => {
    try {
      await navigator.clipboard.writeText(exportCommand(devicePlatform));
      showToast("导出命令已复制，去那台设备粘贴执行即可", "success");
    } catch {
      showToast("复制失败，请手动选中命令复制", "error");
    }
  };

  const isDeviceProfile =
    supportsDeviceProfile && method === "local_import" && profileMode === "device";

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (submitting) return;
    if (method === "token_paste" && !rawInput.trim()) {
      showToast("请粘贴有效的 Token 或 JSON 凭据", "warning");
      return;
    }
    if (isDeviceProfile) {
      if (!profileDir.trim()) {
        showToast("请先选择另一台设备的导出包或数据文件夹", "warning");
        return;
      }
      if (inspecting) {
        showToast("正在校验设备目录，请稍候", "info");
        return;
      }
      if (inspectError || (inspectResult && !inspectResult.hasStorage)) {
        showToast(
          inspectError || "未找到有效的 storage.json，无法作为设备 profile 导入",
          "warning"
        );
        return;
      }
    }
    setSubmitting(true);
    try {
      await importAccount(
        toolId,
        label,
        method,
        method === "token_paste" ? rawInput : undefined,
        targetAccount?.id,
        supportsDeviceProfile && method === "local_import" && profileMode === "device"
          ? profileDir.trim()
          : undefined,
        supportsDeviceProfile && method === "local_import" && profileMode === "device"
          ? devicePlatform
          : undefined
      );
      if (
        supportsDeviceProfile &&
        method === "local_import" &&
        profileMode === "device" &&
        profileDir.trim()
      ) {
        try {
          localStorage.setItem("qwitch.lastDeviceProfileDir", profileDir.trim());
        } catch {
          // ignore storage errors
        }
      }
      setLabel("");
      setRawInput("");
      onClose();
    } catch {
      // 失败时保留弹窗与已粘贴凭据，错误 Toast 已由 context 展示
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      labelledBy={titleId}
      disableClose={submitting}
      maxWidthClass="max-w-lg"
    >
      <form onSubmit={handleSubmit} className="flex max-h-[calc(100vh-5rem)] flex-col">
        <div className="flex shrink-0 items-start justify-between gap-3 border-b border-zinc-100 pb-3 dark:border-zinc-800">
          <div>
            <h3 id={titleId} className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
              {targetAccount ? `完善账号凭据：${targetAccount.label}` : "添加 / 导入账号"}
            </h3>
            <p className="mt-0.5 text-xs text-zinc-600 dark:text-zinc-400">
              {targetAccount
                ? "自动合并本机切号快照与在线授权状态，保留原有配置"
                : "所有凭据经本机加密库存储，绝不上传第三方云端"}
            </p>
          </div>
          <button
            type="button"
            disabled={submitting}
            onClick={onClose}
            aria-label="关闭导入弹窗"
            className="shrink-0 rounded-lg p-1 text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 disabled:cursor-not-allowed disabled:opacity-40 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        <div className="mt-4 min-h-0 flex-1 space-y-4 overflow-y-auto pr-1">
          <div>
            <span className="mb-1.5 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
              目标 AI 工具
            </span>
            <div className="grid grid-cols-3 gap-2">
              {visibleTools.map((tool) => {
                const active = toolId === tool.id;
                return (
                  <button
                    key={tool.id}
                    type="button"
                    disabled={Boolean(targetAccount)}
                    onClick={() => {
                      setToolId(tool.id);
                      const nextMethods = getSupportedMethods(tool.id);
                      if (!nextMethods.includes(method)) {
                        setMethod(nextMethods[0] ?? "local_import");
                      }
                    }}
                    className={`flex items-center gap-2.5 rounded-xl border p-2.5 text-left transition ${
                      targetAccount && !active
                        ? "cursor-not-allowed border-zinc-100 bg-zinc-50 text-zinc-400 dark:border-zinc-800/60 dark:bg-zinc-900/40 dark:text-zinc-500"
                        : active
                          ? "border-zinc-900 bg-zinc-900 text-white shadow-xs dark:border-zinc-100 dark:bg-zinc-100 dark:text-zinc-900"
                          : "border-zinc-200 text-zinc-700 hover:border-zinc-300 hover:bg-zinc-50 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800/60"
                    }`}
                  >
                    <ToolBrandIcon toolId={tool.id} size="sm" decorative />
                    <div className="min-w-0">
                      <div className="truncate whitespace-nowrap text-xs font-semibold">
                        {tool.name}
                      </div>
                      <div
                        className={`truncate text-[11px] ${
                          active
                            ? "text-white/80 dark:text-zinc-700"
                            : "text-zinc-500 dark:text-zinc-400"
                        }`}
                      >
                        {tool.shortTagline}
                      </div>
                    </div>
                  </button>
                );
              })}
            </div>
          </div>

          <div>
            <span className="mb-1.5 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
              导入方式
            </span>
            <div className="grid grid-cols-3 gap-2.5">
              {[
                {
                  id: "local_import" as AuthMethod,
                  label: "本机导入",
                  desc: "读取桌面端当前会话",
                  icon: HardDriveDownload,
                },
                {
                  id: "webview_oauth" as AuthMethod,
                  label: "内置授权",
                  desc: "隔离窗口登录并查额",
                  icon: Globe,
                },
                {
                  id: "token_paste" as AuthMethod,
                  label: "凭据粘贴",
                  desc: "粘贴 Token 或 JSON",
                  icon: KeyRound,
                },
              ].map((item) => {
                const supported = supportedMethods.includes(item.id);
                const active = method === item.id;
                const Icon = item.icon;
                return (
                  <button
                    key={item.id}
                    type="button"
                    disabled={!supported}
                    onClick={() => setMethod(item.id)}
                    className={`flex flex-col items-start gap-1 rounded-xl border p-2.5 text-left transition ${
                      !supported
                        ? "cursor-not-allowed border-zinc-100 bg-zinc-50 text-zinc-400 dark:border-zinc-800/60 dark:bg-zinc-900/40 dark:text-zinc-500"
                        : active
                          ? "border-zinc-900 bg-zinc-900/5 text-zinc-900 ring-1 ring-zinc-900/10 dark:border-zinc-200 dark:bg-zinc-100/10 dark:text-zinc-100 dark:ring-zinc-100/10"
                          : "border-zinc-200 text-zinc-700 hover:border-zinc-300 dark:border-zinc-700 dark:text-zinc-200"
                    }`}
                  >
                    <div className="flex items-center gap-1.5 whitespace-nowrap text-xs font-semibold">
                      <Icon className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      <span>{item.label}</span>
                    </div>
                    <span className="text-[11px] leading-snug text-zinc-500 dark:text-zinc-400">
                      {item.desc}
                    </span>
                  </button>
                );
              })}
            </div>
          </div>

          {method === "local_import" && !targetAccount && supportsDeviceProfile && (
            <div className="rounded-xl border border-zinc-200 p-3 dark:border-zinc-700">
              <span className="mb-1.5 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
                设备 profile（多设备签到）
              </span>
              <div className="grid grid-cols-2 gap-2">
                <button
                  type="button"
                  onClick={() => setProfileMode("default")}
                  className={`rounded-lg border px-2.5 py-2 text-left text-[11px] transition ${
                    profileMode === "default"
                      ? "border-zinc-900 bg-zinc-900/5 text-zinc-900 ring-1 ring-zinc-900/10 dark:border-zinc-200 dark:bg-zinc-100/10 dark:text-zinc-100"
                      : "border-zinc-200 text-zinc-600 hover:border-zinc-300 dark:border-zinc-700 dark:text-zinc-300"
                  }`}
                >
                  本机默认
                  <span className="mt-0.5 block text-[10px] text-zinc-500 dark:text-zinc-400">
                    直接用本机 device_id
                  </span>
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setProfileMode("device");
                    if (!profileDir.trim()) {
                      try {
                        const last = localStorage.getItem("qwitch.lastDeviceProfileDir");
                        if (last) setProfileDir(last);
                      } catch {
                        // ignore storage errors
                      }
                    }
                  }}
                  className={`rounded-lg border px-2.5 py-2 text-left text-[11px] transition ${
                    profileMode === "device"
                      ? "border-zinc-900 bg-zinc-900/5 text-zinc-900 ring-1 ring-zinc-900/10 dark:border-zinc-200 dark:bg-zinc-100/10 dark:text-zinc-100"
                      : "border-zinc-200 text-zinc-600 hover:border-zinc-300 dark:border-zinc-700 dark:text-zinc-300"
                  }`}
                >
                  隔离设备 profile
                  <span className="mt-0.5 block text-[10px] text-zinc-500 dark:text-zinc-400">
                    每个账号独立设备身份
                  </span>
                </button>
              </div>
              {profileMode === "device" && (
                <div className="mt-2 space-y-3">
                  <div className="rounded-lg bg-zinc-50 p-2.5 dark:bg-zinc-800/40">
                    <div className="mb-1.5 text-[11px] font-medium text-zinc-700 dark:text-zinc-200">
                      第 1 步 · 在装有 Trae CN 的那台设备上导出
                    </div>
                    <div className="mb-1.5 flex flex-wrap items-center gap-1.5">
                      <span className="text-[11px] text-zinc-500 dark:text-zinc-400">那台设备系统</span>
                      <div className="inline-flex rounded-lg bg-zinc-200/70 p-0.5 dark:bg-zinc-700/70">
                        {(["mac", "win"] as DevicePlatform[]).map((p) => (
                          <button key={p} type="button" onClick={() => setDevicePlatform(p)}
                            className={`rounded-md px-2 py-0.5 text-[11px] font-medium transition ${devicePlatform === p ? "bg-white text-zinc-900 shadow-xs dark:bg-zinc-800 dark:text-zinc-100" : "text-zinc-600 dark:text-zinc-300"}`}>
                            {p === "mac" ? "macOS" : "Windows"}
                          </button>
                        ))}
                      </div>
                      <button type="button" onClick={() => void handleCopyExport()}
                        className="inline-flex items-center gap-1 rounded-md bg-zinc-900 px-2 py-0.5 text-[11px] font-medium text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900">
                        <Copy className="h-3 w-3" aria-hidden="true" /> 复制导出命令
                      </button>
                    </div>
                    <pre className="max-h-24 overflow-auto whitespace-pre-wrap break-all rounded-md border border-zinc-200 bg-white p-2 text-[10px] leading-snug text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">{exportCommand(devicePlatform)}</pre>
                    <p className="mt-1 text-[10px] leading-snug text-zinc-500 dark:text-zinc-400">
                      先完全退出那台设备的 Trae CN，把上面的命令粘贴到 {devicePlatform === "win" ? "PowerShell" : "终端"} 执行；会在桌面生成 TraeCN-profile.zip，把它拷到本机。
                    </p>
                  </div>

                  <div>
                    <div className="mb-1.5 text-[11px] font-medium text-zinc-700 dark:text-zinc-200">
                      第 2 步 · 把 zip（或整个文件夹）选进来
                    </div>
                    <input
                      type="text"
                      placeholder="已选的 .zip / 文件夹路径（通常点下面按钮选择即可）"
                      value={profileDir}
                      onChange={(e) => setProfileDir(e.target.value)}
                      aria-label="设备导出包路径"
                      className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-2 font-mono text-[11px] text-zinc-900 focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-100"
                    />
                    <div className="mt-1.5 flex flex-wrap gap-2">
                      <button type="button" onClick={() => void handlePickFile()}
                        className="inline-flex items-center gap-1.5 rounded-lg bg-zinc-900 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900">
                        <FileArchive className="h-3.5 w-3.5" aria-hidden="true" /> 选择导出包(.zip)
                      </button>
                      <button type="button" onClick={() => void handlePickFolder()}
                        className="inline-flex items-center gap-1.5 rounded-lg border border-zinc-200 px-2.5 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800">
                        <FolderOpen className="h-3.5 w-3.5" aria-hidden="true" /> 选择文件夹
                      </button>
                    </div>
                    {profileDir.trim() && (
                      <p className="mt-1.5 text-[11px] leading-snug">
                        {inspecting ? (
                          <span className="text-zinc-500 dark:text-zinc-400">校验中…</span>
                        ) : inspectError ? (
                          <span className="text-rose-600 dark:text-rose-400">{inspectError}</span>
                        ) : inspectResult ? (
                          !inspectResult.hasStorage ? (
                            <span className="text-rose-600 dark:text-rose-400">无法识别：请选 Trae CN 导出的 zip，或含 User/globalStorage/storage.json 的文件夹</span>
                          ) : inspectResult.sameAsLocal ? (
                            <span className="text-amber-600 dark:text-amber-400">⚠️ 该设备与本机相同（同一台设备），无法并行签到；请换另一台设备/用户/虚拟机</span>
                          ) : (
                            <span className="text-emerald-600 dark:text-emerald-400">✅ 已识别设备 {inspectResult.deviceIdMasked ?? "（未知）"}，可添加</span>
                          )
                        ) : (
                          <span className="text-zinc-400">&nbsp;</span>
                        )}
                      </p>
                    )}
                  </div>
                </div>
              )}
            </div>
          )}

          <div>
            <label
              htmlFor="qwitch-import-label"
              className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300"
            >
              账号备注别名（可选）
            </label>
            <input
              id="qwitch-import-label"
              type="text"
              placeholder={targetAccount ? `留空保持：${targetAccount.label}` : "留空则自动使用官方账号昵称"}
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-2 text-xs text-zinc-900 focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-100"
            />
          </div>

          {method === "token_paste" && (
            <div>
              <label
                htmlFor="qwitch-import-raw"
                className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300"
              >
                Token 或 JSON 凭据内容
              </label>
              <textarea
                id="qwitch-import-raw"
                rows={4}
                placeholder="粘贴 Bearer Token / JWT 或完整 JSON 会话片段（本地加密落盘，绝不外传）..."
                value={rawInput}
                onChange={(e) => setRawInput(e.target.value)}
                className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-2 font-mono text-xs text-zinc-900 focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-100"
              />
            </div>
          )}

          <div className="rounded-xl bg-zinc-50 p-3 text-xs leading-relaxed text-zinc-600 dark:bg-zinc-800/60 dark:text-zinc-300">
            {method === "local_import"
              ? toolId === "qoder_cn"
                ? "将读取本机 Qoder CN 当前登录会话用于快速切号；若该账号尚未在本工具内完成在线授权，导入后在卡片点击「在线授权」即可开启查额与签到。"
                : "将自动识别本机已登录桌面端的会话与额度信息，同一账号重复导入将自动合并，不会产生冗余记录。"
              : method === "webview_oauth"
                ? "将拉起无痕隔离的官方授权窗口（支持扫码/短信/2FA）；登录成功后若与本机桌面端为同一账号，将自动关联切号快照并同步最新额度。"
                : toolId === "workbuddy"
                  ? "WorkBuddy 推荐直接粘贴完整会话 JSON（或包含用户标识的 JWT；企业版账号请附带 enterpriseId 字段），以便精准识别企业身份与接口头。"
                  : "支持导入纯 Token 或结构化 JSON；若与已有账号 UID 匹配，将自动补充双轨凭据以开启查额与签到。"}
          </div>
        </div>

        <div className="mt-4 flex shrink-0 justify-end gap-2 border-t border-zinc-100 pt-3 dark:border-zinc-800">
          <button
            type="button"
            disabled={submitting}
            onClick={onClose}
            className="shrink-0 whitespace-nowrap rounded-lg border border-zinc-200 px-3.5 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-40 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
          >
            取消
          </button>
          <button
            type="submit"
            disabled={submitting || (isDeviceProfile && inspecting)}
            className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg bg-zinc-900 px-3.5 py-1.5 text-xs font-medium text-white hover:bg-zinc-800 disabled:opacity-60 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
          >
            {submitting && <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden="true" />}
            {method === "webview_oauth"
              ? "打开授权窗口"
              : targetAccount
                ? "确认更新凭据"
                : "确认导入"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
