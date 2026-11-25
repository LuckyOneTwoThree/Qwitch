import { useEffect, useId, useState } from "react";
import { CheckCircle2, Loader2, X, XCircle } from "lucide-react";
import { Modal } from "../ui/Modal";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";

type CheckResult = { available: boolean; inUseBySelf: boolean; message: string } | null;

function parsePortInput(raw: string): number | null {
  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) return null;
  const parsed = Number(trimmed);
  if (!Number.isSafeInteger(parsed) || parsed < 1024 || parsed > 65535) return null;
  return parsed;
}

export function GatewayPortModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { gateway, updateGatewayPort, checkGatewayPort } = usePoolPrototype();
  const titleId = useId();
  const [port, setPort] = useState(String(gateway.port));
  const [checking, setChecking] = useState(false);
  const [applying, setApplying] = useState(false);
  const [result, setResult] = useState<CheckResult>(null);

  useEffect(() => {
    if (open) {
      setPort(String(gateway.port));
      setResult(null);
    }
  }, [open, gateway.port]);

  const busy = checking || applying;
  const parsedPort = parsePortInput(port);
  const isUnchangedPort = parsedPort === gateway.port;

  const runCheck = async () => {
    if (busy) return;
    if (parsedPort === null) {
      setResult({ available: false, inUseBySelf: false, message: "端口必须在 1024–65535 之间" });
      return;
    }
    setChecking(true);
    try {
      setResult(await checkGatewayPort(parsedPort));
    } catch (err) {
      setResult({
        available: false,
        inUseBySelf: false,
        message: err instanceof Error ? err.message : "端口检测失败，请重试",
      });
    } finally {
      setChecking(false);
    }
  };

  const apply = async () => {
    if (busy || isUnchangedPort) return;
    if (parsedPort === null) {
      setResult({ available: false, inUseBySelf: false, message: "端口必须在 1024–65535 之间" });
      return;
    }
    setApplying(true);
    try {
      const res = await updateGatewayPort(parsedPort);
      if (res.ok) {
        onClose();
      } else {
        setResult({ available: false, inUseBySelf: false, message: res.message ?? "更新失败" });
      }
    } catch (err) {
      setResult({
        available: false,
        inUseBySelf: false,
        message: err instanceof Error ? err.message : "端口更新失败，请重试",
      });
    } finally {
      setApplying(false);
    }
  };

  const resultId = `${titleId}-result`;

  return (
    <Modal
      open={open}
      onClose={onClose}
      disableClose={applying}
      labelledBy={titleId}
      maxWidthClass="max-w-md"
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void apply();
        }}
      >
        <div className="flex items-start justify-between gap-3">
          <h2 id={titleId} className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
            网关端口与占用检测
          </h2>
          <button
            type="button"
            disabled={applying}
            onClick={onClose}
            aria-label="关闭网关端口弹窗"
            className="btn btn-icon btn-ghost shrink-0"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
        <p className="mt-1 text-xs leading-relaxed text-zinc-600 dark:text-zinc-300">
          监听地址固定为 <code>127.0.0.1</code>，端口范围 <b>1024–65535</b>。修改会自动重启网关并同步所有已挂载
          Agent 配置；如需立即让桌面客户端生效，可在「系统设置 → 客户端环境」一键重启运行中的 Agent。
        </p>

        <div className="mt-4">
          <label
            htmlFor={`${titleId}-port`}
            className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300"
          >
            监听端口
          </label>
          <div className="flex items-center gap-2">
            <input
              id={`${titleId}-port`}
              type="number"
              min={1024}
              max={65535}
              disabled={busy}
              aria-invalid={result ? !result.available : undefined}
              aria-describedby={result ? resultId : undefined}
              value={port}
              onChange={(e) => {
                setPort(e.target.value);
                setResult(null);
              }}
              className="field-input w-40 font-mono text-sm"
            />
            <button
              type="button"
              onClick={() => void runCheck()}
              disabled={busy}
              aria-busy={checking}
              className="btn btn-sm btn-secondary gap-1"
            >
              {checking ? <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" /> : null}
              {checking ? "检测中…" : "检测占用"}
            </button>
          </div>
        </div>

        {result && (
          <div
            id={resultId}
            role={result.available ? "status" : "alert"}
            className={`mt-3 flex items-start gap-2 rounded-lg border px-3 py-2 text-xs ${
              result.available
                ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-800 dark:text-emerald-300"
                : "border-rose-500/30 bg-rose-500/10 text-rose-800 dark:text-rose-300"
            }`}
          >
            {result.available ? (
              <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            ) : (
              <XCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            )}
            <span>
              {result.message}
              {result.inUseBySelf ? "（当前网关自身）" : ""}
            </span>
          </div>
        )}

        <div className="mt-5 flex justify-end gap-2">
          <button
            type="button"
            disabled={applying}
            onClick={onClose}
            className="btn btn-md btn-secondary"
          >
            取消
          </button>
          <button
            type="submit"
            disabled={busy || isUnchangedPort}
            title={isUnchangedPort ? "端口与当前运行端口相同" : undefined}
            aria-busy={applying}
            className="btn btn-md btn-primary gap-1"
          >
            {applying ? <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" /> : null}
            {applying ? "应用中…" : "应用端口"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
