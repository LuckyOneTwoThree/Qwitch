import React, { useState } from "react";
import {
  AlertCircle,
  ArrowLeftRight,
  Check,
  Clock,
  Globe,
  Link2,
  PauseCircle,
  Pencil,
  PlayCircle,
  RefreshCw,
  RotateCcw,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import type { AccountSummary } from "../types/qwitch";
import { AccountStatusBadge, ClaimStatusBadge } from "./StatusBadge";
import { ToolBrandIcon } from "./ToolBrandIcon";
import { getToolBrandMeta } from "../domain/toolRegistry";
import { getEffectiveClaimStatus } from "../domain/selectors";
import { useQwitch } from "../context/QwitchContext";
import { ConfirmDialog } from "./ui/ConfirmDialog";

function formatQuotaNumber(value: number): string {
  const rounded = Math.round(value * 100) / 100;
  return rounded.toLocaleString("zh-CN", { maximumFractionDigits: 2 });
}

function formatSyncTime(iso?: string): string {
  if (!iso) return "尚未同步";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "已同步";
  return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
}

export const AccountCard = React.memo(function AccountCard({
  account,
}: {
  account: AccountSummary;
}) {
  const {
    refreshAccountQuota,
    claimDailyBatch,
    requestColdSwitch,
    recoverAccount,
    deleteAccount,
    importAccount,
    renameAccount,
    toggleAccountDisabled,
    adapters,
    batchProgress,
  } = useQwitch();

  const [refreshing, setRefreshing] = useState(false);
  const [associating, setAssociating] = useState(false);
  const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false);
  const [editingLabel, setEditingLabel] = useState(false);
  const [labelDraft, setLabelDraft] = useState(account.label);

  const adapter = adapters.find((a) => a.id === account.toolId);
  const toolMeta = getToolBrandMeta(account.toolId, adapters);
  const isHealthy = account.status === "active";
  const isDisabled = account.status === "disabled";
  const canClaim = isHealthy && Boolean(adapter?.capabilities.supportsDailyClaim);
  const needsRecovery = account.status === "expired" || account.status === "risk_paused";
  const missingApiToken = account.hasApiToken === false;
  const missingLocalSnapshot = account.hasLocalSnapshot === false;
  const isBatchRunning = Boolean(batchProgress?.running);
  const effectiveClaimStatus = getEffectiveClaimStatus(account);

  const buckets = account.latestQuota?.buckets ?? [];
  const primaryBucket = buckets[0];
  const secondaryBuckets = buckets.slice(1);
  const isQoderClaimBucket =
    account.toolId === "qoder_cn" && primaryBucket?.key === "daily_credits";

  const handleRefresh = async () => {
    setRefreshing(true);
    await refreshAccountQuota(account.id);
    setRefreshing(false);
  };

  const handleDirectOAuth = () => {
    void importAccount(account.toolId, "", "webview_oauth", undefined, account.id);
  };

  const handleAssociateLocal = async () => {
    setAssociating(true);
    await importAccount(account.toolId, "", "local_import", undefined, account.id);
    setAssociating(false);
  };

  const handleSaveRename = async () => {
    const trimmed = labelDraft.trim();
    if (trimmed && trimmed !== account.label) {
      await renameAccount(account.id, trimmed);
    }
    setEditingLabel(false);
  };

  return (
    <>
      <div
        className={`group flex h-full flex-col justify-between rounded-2xl border bg-white p-3.5 transition-all dark:bg-zinc-900/90 ${
          account.isActiveOnClient
            ? "border-emerald-500/45 shadow-xs ring-1 ring-emerald-500/15 dark:border-emerald-400/35 dark:ring-emerald-400/15"
            : isDisabled
              ? "border-zinc-200/80 bg-zinc-50/70 opacity-85 dark:border-zinc-800 dark:bg-zinc-900/50"
              : "border-zinc-200 hover:border-zinc-300 dark:border-zinc-800 dark:hover:border-zinc-700"
        }`}
      >
        <div className="flex flex-1 flex-col">
          <div className="flex items-start justify-between gap-2">
            <div className="flex min-w-0 flex-1 items-start gap-2.5">
              <ToolBrandIcon toolId={account.toolId} size="md" className="mt-0.5" decorative />
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-1.5">
                  <span className="shrink-0 whitespace-nowrap text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
                    {toolMeta.name}
                  </span>
                  {account.latestQuota?.planName && (
                    <span
                      className={`shrink-0 whitespace-nowrap rounded-md border px-1.5 py-0.5 text-[11px] font-medium ${toolMeta.badgeClass}`}
                    >
                      {account.latestQuota.planName}
                    </span>
                  )}
                  {account.isActiveOnClient && (
                    <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-full bg-emerald-500/12 px-2 py-0.5 text-[11px] font-semibold text-emerald-800 dark:text-emerald-300">
                      <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                      本机使用中
                    </span>
                  )}
                </div>

                {editingLabel ? (
                  <div className="mt-1 flex items-center gap-1">
                    <input
                      type="text"
                      value={labelDraft}
                      onChange={(e) => setLabelDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          void handleSaveRename();
                        } else if (e.key === "Escape") {
                          setLabelDraft(account.label);
                          setEditingLabel(false);
                        }
                      }}
                      aria-label="编辑账号备注"
                      autoFocus
                      className="min-w-0 flex-1 rounded-md border border-zinc-300 bg-white px-2 py-0.5 text-xs font-medium text-zinc-900 dark:border-zinc-600 dark:bg-zinc-800 dark:text-zinc-100"
                    />
                    <button
                      type="button"
                      onClick={() => void handleSaveRename()}
                      aria-label="保存账号备注"
                      className="rounded-md bg-zinc-900 p-1 text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900"
                    >
                      <Check className="h-3 w-3" aria-hidden="true" />
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setLabelDraft(account.label);
                        setEditingLabel(false);
                      }}
                      aria-label="取消重命名"
                      className="rounded-md border border-zinc-200 p-1 text-zinc-600 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-300"
                    >
                      <X className="h-3 w-3" aria-hidden="true" />
                    </button>
                  </div>
                ) : (
                  <div className="mt-0.5 flex items-center gap-1">
                    <h3 className="truncate text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                      {account.label}
                    </h3>
                    <button
                      type="button"
                      onClick={() => {
                        setLabelDraft(account.label);
                        setEditingLabel(true);
                      }}
                      title="修改账号备注"
                      aria-label={`修改 ${account.label} 备注`}
                      className="shrink-0 rounded p-0.5 text-zinc-500 opacity-0 transition hover:bg-zinc-100 hover:text-zinc-800 group-hover:opacity-100 focus-visible:opacity-100 dark:text-zinc-400 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                    >
                      <Pencil className="h-3 w-3" aria-hidden="true" />
                    </button>
                  </div>
                )}

                <p className="truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                  {account.uidMasked}
                  {account.emailMasked ? ` · ${account.emailMasked}` : ""}
                </p>
              </div>
            </div>

            <div className="flex shrink-0 flex-col items-end gap-1">
              <AccountStatusBadge status={account.status} />
              {canClaim && !missingApiToken && (
                <ClaimStatusBadge status={effectiveClaimStatus} />
              )}
            </div>
          </div>

          <div className="mt-3 flex flex-1 flex-col justify-center space-y-2">
            {missingApiToken ? (
              <div className="flex items-center justify-between gap-2.5 rounded-xl border border-amber-500/25 bg-amber-500/8 px-3 py-2 text-xs">
                <span className="leading-snug text-amber-900 dark:text-amber-200">
                  已绑定切号快照 · 授权后开启查额与签到
                </span>
                <button
                  type="button"
                  onClick={handleDirectOAuth}
                  className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-amber-600 px-2.5 py-1 text-[11px] font-medium text-white transition hover:bg-amber-700 dark:bg-amber-500 dark:text-zinc-950 dark:hover:bg-amber-400"
                >
                  <Globe className="h-3 w-3 shrink-0" aria-hidden="true" />
                  在线授权
                </button>
              </div>
            ) : primaryBucket ? (
              <>
                <div>
                  {(() => {
                    const isQoderClaimed =
                      isQoderClaimBucket && effectiveClaimStatus === "claimed";
                    const pct = isQoderClaimed
                      ? 100
                      : primaryBucket.total > 0
                        ? Math.min(
                            100,
                            Math.round((primaryBucket.remaining / primaryBucket.total) * 100)
                          )
                        : 0;
                    const barColor = isQoderClaimed
                      ? "bg-emerald-600 dark:bg-emerald-400"
                      : pct <= 15
                        ? "bg-rose-500"
                        : pct <= 35
                          ? "bg-amber-500"
                          : "bg-zinc-900 dark:bg-zinc-100";
                    const pctBadge = isQoderClaimed
                      ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                      : pct <= 15
                        ? "bg-rose-500/12 text-rose-700 dark:text-rose-300"
                        : pct <= 35
                          ? "bg-amber-500/12 text-amber-800 dark:text-amber-300"
                          : "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300";

                    const badgeText = isQoderClaimed
                      ? "本周期已领"
                      : isQoderClaimBucket
                        ? "今日可领"
                        : `剩余 ${pct}%`;

                    return (
                      <>
                        <div className="mb-1.5 flex items-center justify-between gap-2 text-xs">
                          <div className="flex min-w-0 items-center gap-1.5">
                            <span className="truncate text-zinc-600 dark:text-zinc-400">
                              {primaryBucket.label}
                            </span>
                            <span
                              className={`shrink-0 whitespace-nowrap rounded px-1.5 py-0.5 text-[11px] font-semibold tabular-nums ${pctBadge}`}
                            >
                              {badgeText}
                            </span>
                          </div>
                          <span className="shrink-0 whitespace-nowrap font-medium tabular-nums text-zinc-900 dark:text-zinc-100">
                            {isQoderClaimBucket ? (
                              <>
                                {formatQuotaNumber(primaryBucket.remaining)} {primaryBucket.unit}
                              </>
                            ) : (
                              <>
                                {formatQuotaNumber(primaryBucket.remaining)} /{" "}
                                {formatQuotaNumber(primaryBucket.total)} {primaryBucket.unit}
                              </>
                            )}
                          </span>
                        </div>
                        <div
                          role="progressbar"
                          aria-label={`${account.label} ${primaryBucket.label}`}
                          aria-valuenow={pct}
                          aria-valuemin={0}
                          aria-valuemax={100}
                          className="h-2 w-full overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800"
                        >
                          <div
                            className={`h-full rounded-full transition-all duration-300 ${barColor}`}
                            style={{ width: `${pct}%` }}
                          />
                        </div>
                      </>
                    );
                  })()}
                </div>

                {secondaryBuckets.map((bucket) => (
                  <div
                    key={bucket.key}
                    className="flex items-center justify-between gap-2 rounded-xl bg-zinc-50 px-3 py-1.5 text-[11px] dark:bg-zinc-800/60"
                  >
                    <span className="truncate text-zinc-600 dark:text-zinc-400">
                      {bucket.label}
                    </span>
                    <span className="shrink-0 whitespace-nowrap font-medium tabular-nums text-zinc-800 dark:text-zinc-200">
                      {formatQuotaNumber(bucket.remaining)} {bucket.unit}
                    </span>
                  </div>
                ))}
              </>
            ) : (
              <div className="rounded-xl border border-dashed border-zinc-200 px-3 py-3 text-center text-xs text-zinc-500 dark:border-zinc-800 dark:text-zinc-400">
                {account.latestQuota?.claimStatus === "unsupported"
                  ? "本周期暂无可用签到福利活动"
                  : "暂无额度快照，点击下方「查额」获取最新数据"}
              </div>
            )}
          </div>

          <div className="mt-2.5 flex items-center justify-between gap-2 text-[11px] text-zinc-500 dark:text-zinc-400">
            <div className="flex flex-wrap items-center gap-2">
              {needsRecovery ? (
                <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 text-amber-700 dark:text-amber-300">
                  <AlertCircle className="h-3 w-3" aria-hidden="true" />
                  {account.status === "expired" ? "在线凭据已过期" : "风控暂停，需恢复"}
                </span>
              ) : isDisabled ? (
                <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 text-zinc-600 dark:text-zinc-400">
                  <PauseCircle className="h-3 w-3" aria-hidden="true" />
                  已停用（跳过批量任务）
                </span>
              ) : (
                <>
                  {!missingLocalSnapshot && (
                    <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1">
                      <Check className="h-3 w-3 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                      桌面切号就绪
                    </span>
                  )}
                  {!missingApiToken && (
                    <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1">
                      <Check className="h-3 w-3 text-sky-600 dark:text-sky-400" aria-hidden="true" />
                      {canClaim ? "查额与签到就绪" : "在线查额就绪"}
                    </span>
                  )}
                </>
              )}
            </div>
            <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 tabular-nums">
              <Clock className="h-3 w-3" aria-hidden="true" />
              {formatSyncTime(account.latestQuota?.fetchedAt ?? account.updatedAt)}
            </span>
          </div>
        </div>

        <div className="mt-2.5 flex flex-wrap items-center justify-between gap-2 border-t border-zinc-100 pt-2.5 dark:border-zinc-800">
          <div className="flex flex-wrap items-center gap-1.5">
            {needsRecovery ? (
              <>
                <button
                  type="button"
                  onClick={handleDirectOAuth}
                  className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-amber-500/15 px-2.5 py-1 text-xs font-medium text-amber-800 transition hover:bg-amber-500/25 dark:text-amber-300"
                >
                  <Globe className="h-3 w-3 shrink-0" aria-hidden="true" />
                  重新授权
                </button>
                <button
                  type="button"
                  onClick={() => void recoverAccount(account.id)}
                  className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
                >
                  <RotateCcw className="h-3 w-3 shrink-0" aria-hidden="true" />
                  重试
                </button>
              </>
            ) : missingApiToken ? null : (
              <>
                <button
                  type="button"
                  onClick={() => void handleRefresh()}
                  disabled={refreshing || isDisabled}
                  title="刷新额度"
                  className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-45 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
                >
                  <RefreshCw
                    className={`h-3 w-3 shrink-0 ${refreshing ? "animate-spin" : ""}`}
                    aria-hidden="true"
                  />
                  查额
                </button>

                {canClaim &&
                  (effectiveClaimStatus === "available" ||
                    effectiveClaimStatus === "unknown") && (
                    <button
                      type="button"
                      disabled={isBatchRunning}
                      title={
                        effectiveClaimStatus === "unknown"
                          ? "先核实今日签到状态，未领取则自动领取"
                          : "领取今日签到福利"
                      }
                      onClick={() => void claimDailyBatch([account.id])}
                      className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border px-2.5 py-1 text-xs font-medium transition ${
                        isBatchRunning
                          ? "cursor-default border-zinc-200 bg-zinc-50 text-zinc-500 dark:border-zinc-800 dark:bg-zinc-800/60 dark:text-zinc-400"
                          : "border-zinc-200 text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
                      }`}
                    >
                      <Sparkles
                        className="h-3 w-3 shrink-0 text-sky-600 dark:text-sky-400"
                        aria-hidden="true"
                      />
                      领取
                    </button>
                  )}
              </>
            )}

            <button
              type="button"
              onClick={() => void toggleAccountDisabled(account.id)}
              title={isDisabled ? "恢复启用" : "停用账号（跳过批量队列）"}
              aria-label={isDisabled ? `恢复启用 ${account.label}` : `停用 ${account.label}`}
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2 py-1 text-xs font-medium text-zinc-600 transition hover:bg-zinc-100 hover:text-zinc-900 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
            >
              {isDisabled ? (
                <PlayCircle className="h-3 w-3 shrink-0 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
              ) : (
                <PauseCircle className="h-3 w-3 shrink-0" aria-hidden="true" />
              )}
            </button>

            <button
              type="button"
              onClick={() => setConfirmDeleteOpen(true)}
              title="删除账号"
              aria-label={`删除账号 ${account.label}`}
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2 py-1 text-xs font-medium text-zinc-500 transition hover:border-rose-300 hover:text-rose-600 dark:border-zinc-700 dark:text-zinc-400 dark:hover:border-rose-800 dark:hover:text-rose-400"
            >
              <Trash2 className="h-3 w-3 shrink-0" aria-hidden="true" />
            </button>
          </div>

          {missingLocalSnapshot ? (
            <button
              type="button"
              disabled={associating}
              onClick={() => void handleAssociateLocal()}
              title="读取本机客户端会话并合并到该账号以启用桌面端切换"
              className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:opacity-50 dark:border-zinc-700 dark:text-zinc-200 dark:hover:bg-zinc-800"
            >
              <Link2 className="h-3 w-3 shrink-0" aria-hidden="true" />
              {associating ? "关联中..." : "关联本机"}
            </button>
          ) : (
            <button
              type="button"
              disabled={account.isActiveOnClient}
              onClick={() => requestColdSwitch(account)}
              className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg px-2.5 py-1 text-xs font-medium transition ${
                account.isActiveOnClient
                  ? "cursor-default bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-400"
                  : "bg-zinc-900 text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
              }`}
            >
              <ArrowLeftRight className="h-3 w-3 shrink-0" aria-hidden="true" />
              {account.isActiveOnClient ? "当前使用中" : "切至桌面端"}
            </button>
          )}
        </div>
      </div>

      <ConfirmDialog
        open={confirmDeleteOpen}
        title={`确认删除账号「${account.label}」？`}
        description={`将从本地加密库永久移除该账号（${account.uidMasked}）及其额度快照，此操作不可撤销（不会删除官方桌面端账号）。`}
        confirmLabel="确认删除"
        danger
        onCancel={() => setConfirmDeleteOpen(false)}
        onConfirm={async () => {
          setConfirmDeleteOpen(false);
          await deleteAccount(account.id);
        }}
      />
    </>
  );
});
