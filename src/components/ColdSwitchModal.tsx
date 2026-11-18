import { useId } from "react";
import { CheckCircle2, Loader2, X } from "lucide-react";
import { useQwitch } from "../context/QwitchContext";
import { getToolBrandMeta } from "../domain/toolRegistry";
import { ToolBrandIcon } from "./ToolBrandIcon";
import { Modal } from "./ui/Modal";

const SWITCH_STEPS: Array<{ key: "closing" | "writing" | "launching"; label: string }> = [
  { key: "closing", label: "优雅关闭客户端" },
  { key: "writing", label: "写入加密快照" },
  { key: "launching", label: "重启桌面端" },
];

export function ColdSwitchModal() {
  const { switchDialog, cancelColdSwitch, confirmColdSwitch, adapters } = useQwitch();
  const titleId = useId();
  const isOpen = Boolean(switchDialog.account);
  const isProcessing = switchDialog.phase !== "confirm";

  if (!switchDialog.account) return null;

  const brand = getToolBrandMeta(switchDialog.account.toolId, adapters);
  const stepText: Record<string, string> = {
    closing: "1/3 正在优雅关闭本机客户端并等待文件锁释放...",
    writing: "2/3 正在写入目标账号加密会话快照...",
    launching: "3/3 正在重新拉起桌面客户端...",
    done: "切换完成，客户端已使用新账号启动！",
  };

  const phaseRank: Record<string, number> = {
    closing: 0,
    writing: 1,
    launching: 2,
    done: 3,
  };
  const currentRank = phaseRank[switchDialog.phase] ?? 0;

  return (
    <Modal
      open={isOpen}
      onClose={cancelColdSwitch}
      labelledBy={titleId}
      disableClose={isProcessing}
      maxWidthClass="max-w-md"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <ToolBrandIcon toolId={switchDialog.account.toolId} size="md" decorative />
          <div className="min-w-0">
            <h3 id={titleId} className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
              切换 {brand.name} 桌面端账号
            </h3>
            <p className="mt-0.5 truncate text-xs text-zinc-600 dark:text-zinc-400">
              目标账号：{switchDialog.account.label} ({switchDialog.account.uidMasked})
            </p>
          </div>
        </div>
        {!isProcessing && (
          <button
            type="button"
            onClick={cancelColdSwitch}
            aria-label="关闭切换确认窗口"
            className="shrink-0 rounded-lg p-1 text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        )}
      </div>

      {!isProcessing ? (
        <>
          {switchDialog.isClientRunning !== false ? (
            <div className="mt-4 rounded-xl border border-amber-500/25 bg-amber-500/5 p-3 text-xs leading-relaxed text-zinc-700 dark:text-zinc-300">
              检测到本地 {brand.name} 客户端正在运行。为防止会话文件损坏，Qwitch 将自动关闭当前客户端、写入选中账号的加密快照后重新启动。
              <strong className="mt-1 block font-medium text-amber-700 dark:text-amber-400">
                切换前请先保存 IDE 或对话中未保存的工作内容。
              </strong>
            </div>
          ) : (
            <div className="mt-4 rounded-xl border border-zinc-200 bg-zinc-50 p-3 text-xs leading-relaxed text-zinc-700 dark:border-zinc-800 dark:bg-zinc-800/60 dark:text-zinc-300">
              当前未检测到 {brand.name} 在运行。Qwitch 将直接写入选中账号的会话快照并重新拉起客户端。
            </div>
          )}

          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={cancelColdSwitch}
              className="shrink-0 whitespace-nowrap rounded-lg border border-zinc-200 px-3.5 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => void confirmColdSwitch()}
              className="shrink-0 whitespace-nowrap rounded-lg bg-zinc-900 px-3.5 py-1.5 text-xs font-medium text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
            >
              {switchDialog.isClientRunning !== false ? "确认关闭并切换" : "确认写入并启动"}
            </button>
          </div>
        </>
      ) : (
        <div className="mt-5 space-y-3 rounded-xl bg-zinc-50 p-4 dark:bg-zinc-800/70">
          <div className="grid grid-cols-3 gap-2">
            {SWITCH_STEPS.map((st, idx) => {
              const skipped = st.key === "closing" && switchDialog.isClientRunning === false;
              const done = !skipped && (currentRank > idx || switchDialog.phase === "done");
              const active = !skipped && currentRank === idx && switchDialog.phase !== "done";
              return (
                <div
                  key={st.key}
                  className={`flex items-center gap-1.5 rounded-lg border px-2 py-1.5 text-[11px] font-medium ${
                    done
                      ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                      : active
                        ? "border-zinc-900/20 bg-white text-zinc-900 shadow-2xs dark:border-zinc-600 dark:bg-zinc-900 dark:text-zinc-100"
                        : "border-zinc-200/70 text-zinc-500 dark:border-zinc-700/70 dark:text-zinc-400"
                  }`}
                >
                  {done ? (
                    <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-emerald-500" aria-hidden="true" />
                  ) : active ? (
                    <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden="true" />
                  ) : (
                    <span className="flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full bg-zinc-200 text-[11px] text-zinc-700 dark:bg-zinc-700 dark:text-zinc-200">
                      {idx + 1}
                    </span>
                  )}
                  <span className="truncate">{st.label}{skipped ? "（已跳过）" : ""}</span>
                </div>
              );
            })}
          </div>
          <div className="flex items-center gap-2 text-xs font-medium text-zinc-700 dark:text-zinc-200">
            {switchDialog.phase === "done" ? (
              <CheckCircle2 className="h-4 w-4 shrink-0 text-emerald-500" aria-hidden="true" />
            ) : (
              <Loader2 className="h-4 w-4 shrink-0 animate-spin text-zinc-500" aria-hidden="true" />
            )}
            <span>{stepText[switchDialog.phase]}</span>
          </div>
        </div>
      )}
    </Modal>
  );
}
