import React, { useEffect, useMemo, useRef } from "react";
import {
  ArrowLeftRight,
  Cpu,
  Globe,
  KeyRound,
  Loader2,
  PauseCircle,
  PlayCircle,
  RefreshCw,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import type { AccountSummary } from "../../types/qwitch";
import { useQwitch } from "../../context/QwitchContext";
import { ToolBrandIcon } from "../ToolBrandIcon";
import { OverflowMenu } from "../ui/OverflowMenu";
import { ConfirmDialog } from "../ui/ConfirmDialog";
import { formatRelativeSyncTime, isSnapshotStale } from "../../domain/time";
import { getToolBrandMeta } from "../../domain/toolRegistry";
import {
  claimAvailability,
  formatQuotaNumber,
  toQuotaView,
  toStatusView,
  waterTone,
  type LedgerQuotaView,
} from "./ledgerModel";

const RENEWAL_LABEL: Record<string, string> = {
  auto_refresh: "自动续期",
  cookie_keepalive: "Cookie 保活",
  client_sync: "随客户端同步",
  manual: "手动维护",
};

const ACTION_LABEL: Record<string, string> = {
  import: "导入账号",
  query: "额度查询",
  claim: "每日签到",
  switch: "桌面切号",
  keepalive: "凭据保活",
};

const TONE_TEXT: Record<string, string> = {
  ok: "text-emerald-700 dark:text-emerald-300",
  warn: "text-amber-700 dark:text-amber-300",
  danger: "text-rose-700 dark:text-rose-300",
  muted: "text-zinc-500 dark:text-zinc-400",
};
const TONE_DOT: Record<string, string> = {
  ok: "bg-emerald-500",
  warn: "bg-amber-500",
  danger: "bg-rose-500",
  muted: "bg-zinc-400",
};

/** 单个额度桶一行：有可信总量才画条，纯余额只报数 */
function BucketRow({ quota }: { quota: LedgerQuotaView }) {
  const tone = waterTone(quota.pct);
  return (
    <div className="py-2 first:pt-0 last:pb-0">
      <div className="flex items-baseline justify-between gap-2">
        <span className="truncate text-[11.5px] text-zinc-600 dark:text-zinc-300">
          {quota.bucketLabel}
        </span>
        <span className="shrink-0 whitespace-nowrap text-[12px] font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
          {formatQuotaNumber(quota.remaining ?? 0)}
          {quota.total !== null && quota.kind === "ratio" && (
            <span className="font-normal text-zinc-500 dark:text-zinc-400">
              {" / "}
              {formatQuotaNumber(quota.total)} {quota.unit}
            </span>
          )}
          {quota.kind === "percent" && (
            <span className="font-normal text-zinc-500 dark:text-zinc-400">%</span>
          )}
          {quota.kind === "balance" && (
            <span className="ml-1 font-normal text-zinc-500 dark:text-zinc-400">{quota.unit}</span>
          )}
        </span>
      </div>
      {quota.pct !== null ? (
        <div className="mt-1.5 flex items-center gap-2">
          <span className="ledger-meter">
            <i data-tone={tone} style={{ width: `${quota.pct}%` }} />
          </span>
          <span
            className={`w-8 shrink-0 text-right text-[10.5px] tabular-nums ${
              tone === "critical"
                ? "text-rose-600 dark:text-rose-400"
                : tone === "low"
                  ? "text-amber-700 dark:text-amber-300"
                  : "text-zinc-500 dark:text-zinc-400"
            }`}
          >
            {quota.pct}%
          </span>
        </div>
      ) : (
        <p className="mt-1 text-[10.5px] text-zinc-400 dark:text-zinc-500">
          {quota.kind === "balance"
            ? "服务端未给出可信总量，不做百分比推测"
            : quota.kind === "claim"
              ? "水位由签到状态推导，不按百分比计"
              : "尚未查询额度"}
        </p>
      )}
    </div>
  );
}

function CapabilityChip({
  enabled,
  icon: Icon,
  label,
}: {
  enabled: boolean;
  icon: React.ComponentType<{ className?: string }>;
  label: string;
}) {
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-[10.5px] ${
        enabled
          ? "border-zinc-200 bg-white text-zinc-600 dark:border-white/10 dark:bg-white/[0.04] dark:text-zinc-300"
          : "border-dashed border-zinc-200 bg-transparent text-zinc-400 line-through dark:border-white/[0.08] dark:text-zinc-600"
      }`}
      title={enabled ? `${label}可用` : `该适配器不支持${label}`}
    >
      <Icon className="h-2.5 w-2.5" />
      {label}
    </span>
  );
}

/**
 * 账号详情抽屉（右侧 Overlay，不挤压表格列宽）。
 *
 * 设计约束：
 * - 只接收 id，账号数据从当前列表实时查，避免「查额后抽屉还是旧数字」
 * - Esc 关闭并把焦点还给触发它的行
 * - 不设背景遮罩：它是一个「检视面板」，开着的时候仍要能看着表格继续扫
 */
export function AccountDrawer({
  account,
  onClose,
  returnFocusTo,
}: {
  account: AccountSummary | null;
  onClose: () => void;
  /** 关闭后要还焦点的元素（通常是触发行） */
  returnFocusTo?: HTMLElement | null;
}) {
  const {
    adapters,
    logs,
    refreshAccountQuota,
    claimDailyBatch,
    requestColdSwitch,
    toggleAccountDisabled,
    deleteAccount,
    launchToolClient,
    batchProgress,
  } = useQwitch();

  const [refreshing, setRefreshing] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState(false);
  const panelRef = useRef<HTMLElement | null>(null);
  const open = Boolean(account);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // 上层还有命令面板时，Esc 只应关闭面板，不应顺带把抽屉也关掉
      if (document.querySelector('[aria-label="命令面板"]')) return;
      e.stopPropagation();
      onClose();
    };
    document.addEventListener("keydown", onKey);
    // 焦点落在面板本身（tabIndex=-1）而不是关闭按钮：
    // 屏幕阅读器会播报面板标题，同时不会给鼠标用户画一个突兀的焦点环。
    panelRef.current?.focus({ preventScroll: true });
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  useEffect(() => {
    if (open) return;
    returnFocusTo?.focus?.();
  }, [open, returnFocusTo]);

  const recentLogs = useMemo(() => {
    if (!account) return [];
    return logs.filter((l) => l.accountId === account.id).slice(0, 5);
  }, [logs, account]);

  const adapter = adapters.find((a) => a.id === account?.toolId);
  const caps = adapter?.capabilities;

  if (!account) return null;

  const quota = toQuotaView(account);
  const status = toStatusView(account, adapters);
  const claim = claimAvailability(account, adapters);
  const toolMeta = getToolBrandMeta(account.toolId, adapters);
  const buckets = account.latestQuota?.buckets ?? [];
  const stale = isSnapshotStale(account.latestQuota?.fetchedAt);
  const busy = Boolean(batchProgress?.running);

  return (
    <>
      <aside
        ref={panelRef}
        role="dialog"
        aria-modal="false"
        aria-label={`${account.label} 详情`}
        tabIndex={-1}
        className="fixed top-0 right-0 z-40 flex h-full w-[380px] max-w-[92vw] flex-col border-l border-zinc-200/80 bg-white shadow-[-16px_0_40px_-24px_rgba(15,23,42,0.28)] outline-none dark:border-white/[0.08] dark:bg-[#111216]"
      >
        <header className="flex shrink-0 items-start gap-2.5 border-b border-zinc-200/70 px-4 py-3.5 dark:border-white/[0.07]">
          <ToolBrandIcon toolId={account.toolId} size="sm" decorative />
          <div className="min-w-0 flex-1">
            <h2 className="truncate text-[14px] font-semibold tracking-tight text-zinc-900 dark:text-zinc-50">
              {account.label}
            </h2>
            <div className="mt-0.5 flex items-center gap-1.5 text-[11px] text-zinc-500 dark:text-zinc-400">
              <span className="truncate">{toolMeta.name}</span>
              {account.latestQuota?.planName && (
                <>
                  <span aria-hidden="true" className="text-zinc-300 dark:text-zinc-600">·</span>
                  <span className="truncate">{account.latestQuota.planName}</span>
                </>
              )}
              <span aria-hidden="true" className="text-zinc-300 dark:text-zinc-600">·</span>
              <span className={`inline-flex items-center gap-1 ${TONE_TEXT[status.tone]}`}>
                <span className={`h-1.5 w-1.5 rounded-full ${TONE_DOT[status.tone]}`} aria-hidden="true" />
                {status.label}
              </span>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭详情"
            className="btn btn-icon btn-ghost -mr-1 h-7 w-7 shrink-0 !p-0"
          >
            <X className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto">
          {/* 额度 */}
          <section className="border-b border-zinc-200/70 px-4 py-3.5 dark:border-white/[0.07]">
            <div className="mb-2 flex items-center justify-between">
              <h3 className="text-[10.5px] font-medium tracking-[0.08em] text-zinc-500 uppercase dark:text-zinc-400">
                额度
              </h3>
              <button
                type="button"
                onClick={() => {
                  setRefreshing(true);
                  void refreshAccountQuota(account.id).finally(() => setRefreshing(false));
                }}
                disabled={refreshing || busy || caps?.supportsQuotaQuery === false}
                className="inline-flex items-center gap-1 rounded px-1 py-0.5 text-[10.5px] text-zinc-500 transition hover:bg-zinc-100 hover:text-zinc-800 disabled:opacity-40 dark:text-zinc-400 dark:hover:bg-white/[0.06] dark:hover:text-zinc-100"
              >
                {refreshing ? (
                  <Loader2 className="h-3 w-3 animate-spin" aria-hidden="true" />
                ) : (
                  <RefreshCw className="h-3 w-3" aria-hidden="true" />
                )}
                查额
              </button>
            </div>

            {buckets.length > 0 ? (
              <div className="divide-y divide-zinc-100 dark:divide-white/[0.05]">
                {buckets.map((b) => (
                  <BucketRow
                    key={b.key}
                    quota={{
                      kind:
                        quota.kind === "claim"
                          ? "claim"
                          : b.unit === "%"
                            ? "percent"
                            : b.hasTotal === false
                              ? "balance"
                              : b.total > 0
                                ? "ratio"
                                : "balance",
                      pct:
                        quota.kind === "claim"
                          ? null
                          : b.unit === "%"
                            ? Math.max(0, Math.min(100, Math.round(b.remaining)))
                            : b.hasTotal === false || b.total <= 0
                              ? null
                              : Math.max(0, Math.min(100, Math.round((b.remaining / b.total) * 100))),
                      remaining: b.remaining,
                      total: b.total,
                      unit: b.unit,
                      extraBuckets: 0,
                      bucketLabel: b.label,
                    }}
                  />
                ))}
              </div>
            ) : (
              <p className="text-[11.5px] text-zinc-500 dark:text-zinc-400">
                尚未查询额度。点「查额」拉取当前水位。
              </p>
            )}

            <p
              className={`mt-2 text-[10.5px] ${
                stale ? "text-amber-700 dark:text-amber-300" : "text-zinc-400 dark:text-zinc-500"
              }`}
            >
              {account.latestQuota?.fetchedAt
                ? `${formatRelativeSyncTime(account.latestQuota.fetchedAt)}同步${stale ? " · 快照已过期" : ""}`
                : "从未同步"}
            </p>
          </section>

          {/* 身份 */}
          <section className="border-b border-zinc-200/70 px-4 py-3.5 dark:border-white/[0.07]">
            <h3 className="mb-2 text-[10.5px] font-medium tracking-[0.08em] text-zinc-500 uppercase dark:text-zinc-400">
              身份
            </h3>
            <dl className="space-y-1.5 text-[11.5px]">
              <div className="flex items-baseline justify-between gap-3">
                <dt className="shrink-0 text-zinc-500 dark:text-zinc-400">UID</dt>
                <dd className="truncate font-mono text-zinc-800 dark:text-zinc-200">
                  {account.uidMasked}
                </dd>
              </div>
              {account.emailMasked && (
                <div className="flex items-baseline justify-between gap-3">
                  <dt className="shrink-0 text-zinc-500 dark:text-zinc-400">邮箱</dt>
                  <dd className="truncate font-mono text-zinc-800 dark:text-zinc-200">
                    {account.emailMasked}
                  </dd>
                </div>
              )}
              {account.deviceIdMasked && (
                <div className="flex items-baseline justify-between gap-3">
                  <dt className="shrink-0 text-zinc-500 dark:text-zinc-400">设备</dt>
                  <dd className="flex min-w-0 items-center gap-1 truncate font-mono text-zinc-800 dark:text-zinc-200">
                    <Cpu className="h-3 w-3 shrink-0 text-zinc-400" aria-hidden="true" />
                    {account.deviceIdMasked}
                    {account.deviceType ? ` · ${account.deviceType}` : ""}
                  </dd>
                </div>
              )}
              {account.renewalMode && (
                <div className="flex items-baseline justify-between gap-3">
                  <dt className="shrink-0 text-zinc-500 dark:text-zinc-400">续期</dt>
                  <dd className="truncate text-zinc-800 dark:text-zinc-200">
                    {RENEWAL_LABEL[account.renewalMode] ?? account.renewalMode}
                  </dd>
                </div>
              )}
              <div className="flex items-baseline justify-between gap-3">
                <dt className="shrink-0 text-zinc-500 dark:text-zinc-400">本机</dt>
                <dd className="truncate text-zinc-800 dark:text-zinc-200">
                  {account.isActiveOnClient ? "客户端当前登录此账号" : "未挂载到客户端"}
                </dd>
              </div>
            </dl>
          </section>

          {/* 能力 */}
          <section className="border-b border-zinc-200/70 px-4 py-3.5 dark:border-white/[0.07]">
            <h3 className="mb-2 text-[10.5px] font-medium tracking-[0.08em] text-zinc-500 uppercase dark:text-zinc-400">
              能力
            </h3>
            <div className="flex flex-wrap gap-1.5">
              <CapabilityChip enabled={Boolean(caps?.supportsClientSwitch)} icon={ArrowLeftRight} label="切号" />
              <CapabilityChip enabled={Boolean(caps?.supportsQuotaQuery)} icon={RefreshCw} label="查额" />
              <CapabilityChip enabled={Boolean(caps?.supportsDailyClaim)} icon={Sparkles} label="签到" />
              <CapabilityChip
                enabled={Boolean(caps?.authMethods?.includes("webview_oauth"))}
                icon={Globe}
                label="OAuth"
              />
              <CapabilityChip
                enabled={Boolean(caps?.authMethods?.includes("token_paste"))}
                icon={KeyRound}
                label="粘贴凭据"
              />
            </div>
          </section>

          {/* 最近操作 */}
          <section className="px-4 py-3.5">
            <h3 className="mb-2 text-[10.5px] font-medium tracking-[0.08em] text-zinc-500 uppercase dark:text-zinc-400">
              最近操作
            </h3>
            {recentLogs.length > 0 ? (
              <ul className="space-y-1.5">
                {recentLogs.map((log) => (
                  <li key={log.id} className="flex items-baseline gap-2 text-[11px]">
                    <span className="w-9 shrink-0 font-mono text-[10px] text-zinc-400 dark:text-zinc-500">
                      {new Date(log.createdAt).toLocaleTimeString("zh-CN", {
                        hour: "2-digit",
                        minute: "2-digit",
                      })}
                    </span>
                    <span className="shrink-0 text-zinc-600 dark:text-zinc-300">
                      {ACTION_LABEL[log.actionType] ?? log.actionType}
                    </span>
                    <span
                      className={`truncate ${
                        log.resultStatus === "success"
                          ? "text-emerald-700 dark:text-emerald-300"
                          : log.resultStatus === "failed"
                            ? "text-rose-700 dark:text-rose-300"
                            : "text-zinc-400 dark:text-zinc-500"
                      }`}
                      title={log.messageSanitized}
                    >
                      {log.resultStatus === "success" ? "成功" : log.resultStatus === "failed" ? "失败" : "已跳过"}
                    </span>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-[11.5px] text-zinc-500 dark:text-zinc-400">还没有针对该账号的操作记录。</p>
            )}
          </section>
        </div>

        <footer className="flex shrink-0 items-center gap-1.5 border-t border-zinc-200/70 bg-zinc-50/60 px-4 py-3 dark:border-white/[0.07] dark:bg-white/[0.02]">
          {claim === "ready" || claim === "verify" ? (
            <button
              type="button"
              onClick={() => void claimDailyBatch([account.id])}
              disabled={busy}
              className="btn btn-sm btn-primary gap-1"
            >
              <Sparkles className="h-3 w-3" aria-hidden="true" />
              领取
            </button>
          ) : caps?.supportsClientSwitch ? (
            <button
              type="button"
              onClick={() => requestColdSwitch(account)}
              disabled={busy}
              className="btn btn-sm btn-primary gap-1"
            >
              <ArrowLeftRight className="h-3 w-3" aria-hidden="true" />
              切至桌面端
            </button>
          ) : null}

          <button
            type="button"
            onClick={() => void launchToolClient(account.toolId)}
            className="btn btn-sm btn-secondary gap-1"
          >
            打开客户端
          </button>

          <span className="flex-1" />

          <OverflowMenu
            label={`${account.label} 更多操作`}
            items={[
              {
                label: account.status === "disabled" ? "恢复启用" : "停用账号",
                icon: account.status === "disabled" ? PlayCircle : PauseCircle,
                onSelect: () => void toggleAccountDisabled(account.id),
              },
              {
                label: "删除账号",
                icon: Trash2,
                danger: true,
                onSelect: () => setConfirmDelete(true),
              },
            ]}
          />
        </footer>
      </aside>

      <ConfirmDialog
        open={confirmDelete}
        title="删除账号"
        description={`将从本地加密库中永久删除「${account.label}」（${account.uidMasked}）。该操作不可撤销。`}
        confirmLabel="删除"
        danger
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => {
          setConfirmDelete(false);
          onClose();
          void deleteAccount(account.id);
        }}
      />
    </>
  );
}
