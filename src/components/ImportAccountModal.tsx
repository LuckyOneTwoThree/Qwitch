import { useEffect, useState } from "react";
import { Globe, HardDriveDownload, Loader2, X } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { ToolBrandIcon } from "./ToolBrandIcon";
import type { AccountSummary, AuthMethod, ToolId } from "../types/qwitch";

const TOOL_OPTIONS: Array<{ id: ToolId; name: string; sub: string }> = [
  { id: "trae_cn", name: "Trae CN", sub: "字节 AI IDE" },
  { id: "workbuddy", name: "WorkBuddy", sub: "腾讯 AI 助手" },
  { id: "qoder_cn", name: "Qoder CN", sub: "Agent 编程" },
];

export function ImportAccountModal({
  open,
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
  const { adapters, importAccount } = useQwitch();
  const [toolId, setToolId] = useState<ToolId>("trae_cn");
  const [label, setLabel] = useState("");
  const [method, setMethod] = useState<AuthMethod>("local_import");
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setSubmitting(false);
    if (targetAccount) {
      setToolId(targetAccount.toolId);
      setLabel("");
      const methods = adapters.find((a) => a.id === targetAccount.toolId)?.capabilities.authMethods ?? [
        "local_import",
      ];
      if (targetAccount.hasApiToken === false && methods.includes("webview_oauth")) {
        setMethod("webview_oauth");
      } else if (targetAccount.hasLocalSnapshot === false && methods.includes("local_import")) {
        setMethod("local_import");
      } else {
        setMethod(methods[0] ?? "local_import");
      }
      return;
    }

    const initialTool =
      defaultToolId && TOOL_OPTIONS.some((t) => t.id === defaultToolId) ? defaultToolId : "trae_cn";
    setToolId(initialTool);
    setLabel("");
    const methods = adapters.find((a) => a.id === initialTool)?.capabilities.authMethods ?? [
      "local_import",
      "webview_oauth",
    ];
    if (defaultMethod && methods.includes(defaultMethod)) {
      setMethod(defaultMethod);
    } else if (methods.includes("local_import")) {
      setMethod("local_import");
    } else {
      setMethod(methods[0] ?? "webview_oauth");
    }
  }, [open, targetAccount, defaultToolId, defaultMethod, adapters]);

  if (!open) return null;

  const selectedAdapter = adapters.find((a) => a.id === toolId);
  const supportedMethods = selectedAdapter?.capabilities.authMethods ?? ["local_import", "webview_oauth"];

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (submitting) return;
    setSubmitting(true);
    try {
      await importAccount(toolId, label, method, undefined, targetAccount?.id);
      setLabel("");
      onClose();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 backdrop-blur-xs">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-lg rounded-2xl border border-zinc-200 bg-white p-5 shadow-xl dark:border-zinc-800 dark:bg-zinc-900"
      >
        <div className="flex items-start justify-between gap-3 border-b border-zinc-100 pb-3 dark:border-zinc-800">
          <div>
            <h3 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
              {targetAccount ? `完善账号凭据：${targetAccount.label}` : "添加 / 导入账号"}
            </h3>
            <p className="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">
              {targetAccount
                ? "自动合并本机切号快照与在线授权状态，保留原有配置"
                : "所有凭据经本机加密库存储，绝不上传第三方云端"}
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="shrink-0 rounded-lg p-1 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="mt-4 space-y-4">
          <div>
            <label className="mb-1.5 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
              目标 AI 工具
            </label>
            <div className="grid grid-cols-3 gap-2">
              {TOOL_OPTIONS.map((tool) => {
                const active = toolId === tool.id;
                return (
                  <button
                    key={tool.id}
                    type="button"
                    disabled={Boolean(targetAccount)}
                    onClick={() => {
                      setToolId(tool.id);
                      const nextMethods = adapters.find((a) => a.id === tool.id)?.capabilities.authMethods ?? [
                        "local_import",
                        "webview_oauth",
                      ];
                      if (!nextMethods.includes(method)) {
                        setMethod(nextMethods[0] ?? "local_import");
                      }
                    }}
                    className={`flex items-center gap-2.5 rounded-xl border p-2.5 text-left transition ${
                      targetAccount && !active
                        ? "cursor-not-allowed border-zinc-100 bg-zinc-50 text-zinc-300 dark:border-zinc-800/60 dark:bg-zinc-900/40 dark:text-zinc-600"
                        : active
                          ? "border-zinc-900 bg-zinc-900 text-white shadow-xs dark:border-zinc-100 dark:bg-zinc-100 dark:text-zinc-900"
                          : "border-zinc-200 text-zinc-700 hover:border-zinc-300 hover:bg-zinc-50 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800/60"
                    }`}
                  >
                    <ToolBrandIcon toolId={tool.id} size="sm" />
                    <div className="min-w-0">
                      <div className="truncate whitespace-nowrap text-xs font-semibold">
                        {tool.name}
                      </div>
                      <div
                        className={`truncate text-[10px] ${
                          active
                            ? "text-white/75 dark:text-zinc-600"
                            : "text-zinc-400 dark:text-zinc-500"
                        }`}
                      >
                        {tool.sub}
                      </div>
                    </div>
                  </button>
                );
              })}
            </div>
          </div>

          <div>
            <label className="mb-1.5 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
              导入方式
            </label>
            <div className="grid grid-cols-2 gap-3">
              {[
                {
                  id: "local_import" as AuthMethod,
                  label: "本机一键导入",
                  desc: "直接读取桌面端当前登录账号",
                  icon: HardDriveDownload,
                },
                {
                  id: "webview_oauth" as AuthMethod,
                  label: "内置授权登录",
                  desc: "独立窗口登录，适合多账号或激活查额",
                  icon: Globe,
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
                    className={`flex flex-col items-start gap-1 rounded-xl border p-3 text-left transition ${
                      !supported
                        ? "cursor-not-allowed border-zinc-100 bg-zinc-50 text-zinc-300 dark:border-zinc-800/60 dark:bg-zinc-900/40 dark:text-zinc-600"
                        : active
                          ? "border-zinc-900 bg-zinc-900/5 text-zinc-900 ring-1 ring-zinc-900/10 dark:border-zinc-200 dark:bg-zinc-100/10 dark:text-zinc-100 dark:ring-zinc-100/10"
                          : "border-zinc-200 text-zinc-600 hover:border-zinc-300 dark:border-zinc-700 dark:text-zinc-300"
                    }`}
                  >
                    <div className="flex items-center gap-2 whitespace-nowrap text-xs font-semibold">
                      <Icon className="h-4 w-4 shrink-0" />
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

          <div>
            <label className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
              账号备注别名（可选）
            </label>
            <input
              type="text"
              placeholder={targetAccount ? `留空保持：${targetAccount.label}` : "留空则自动使用官方账号昵称"}
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-2 text-xs text-zinc-900 outline-none focus:border-zinc-400 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-100"
            />
          </div>

          {method === "local_import" ? (
            <div className="rounded-xl bg-zinc-50 p-3 text-xs leading-relaxed text-zinc-600 dark:bg-zinc-800/60 dark:text-zinc-300">
              {toolId === "qoder_cn"
                ? "将读取本机 Qoder CN 当前登录会话用于快速切号；若该账号尚未在本工具内完成在线授权，导入后在卡片点击「在线授权」即可开启查额与签到。"
                : "将自动识别本机已登录桌面端的会话与额度信息，同一账号重复导入将自动合并，不会产生冗余记录。"}
            </div>
          ) : (
            <div className="rounded-xl bg-zinc-50 p-3 text-xs leading-relaxed text-zinc-600 dark:bg-zinc-800/60 dark:text-zinc-300">
              将拉起无痕隔离的官方授权窗口（支持扫码/短信/2FA）；登录成功后若与本机桌面端为同一账号，将自动关联切号快照并同步最新额度。
            </div>
          )}
        </div>

        <div className="mt-5 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="shrink-0 whitespace-nowrap rounded-lg border border-zinc-200 px-3.5 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800"
          >
            取消
          </button>
          <button
            type="submit"
            disabled={submitting}
            className="inline-flex shrink-0 whitespace-nowrap items-center gap-1.5 rounded-lg bg-zinc-900 px-3.5 py-1.5 text-xs font-medium text-white hover:bg-zinc-800 disabled:opacity-60 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
          >
            {submitting && <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" />}
            {method === "webview_oauth" ? "打开授权窗口" : targetAccount ? "确认更新快照" : "确认一键导入"}
          </button>
        </div>
      </form>
    </div>
  );
}
