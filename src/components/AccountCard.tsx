import React, { useState } from "react";
import {
  AlertCircle,
  ArrowLeftRight,
  Check,
  ClipboardPaste,
  Clock,
  Cpu,
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
import type { AccountSummary, AuthMethod } from "../types/qwitch";
import { AccountStatusBadge, ClaimStatusBadge } from "./StatusBadge";
import { ToolBrandIcon } from "./ToolBrandIcon";
import { getToolBrandMeta } from "../domain/toolRegistry";
import { getEffectiveClaimStatus } from "../domain/selectors";
import { useQwitch } from "../context/QwitchContext";
import { ConfirmDialog } from "./ui/ConfirmDialog";
import { OverflowMenu } from "./ui/OverflowMenu";

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
  onOpenBindModal,
}: {
  account: AccountSummary;
  onOpenBindModal?: (account: AccountSummary, preferredMethod?: AuthMethod) => void;
}) {
  const {
    refreshAccountQuota,
    claimDailyBatch,
    requestColdSwitch,
    recoverAccount,
    captureLocalSession,
    deleteAccount,
    importAccount,
    previewLocalAccount,
    renameAccount,
    toggleAccountDisabled,
    adapters,
    batchProgress,
    isTauriRuntime,
    showToast,
  } = useQwitch();

  const [refreshing, setRefreshing] = useState(false);
  const [recovering, setRecovering] = useState(false);
  const [associating, setAssociating] = useState(false);
  const [oauthLaunching, setOauthLaunching] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [togglingStatus, setTogglingStatus] = useState(false);
  const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false);
  const [editingLabel, setEditingLabel] = useState(false);
  const [labelDraft, setLabelDraft] = useState(account.label);
  const [localPreview, setLocalPreview] = useState<
    Awaited<ReturnType<typeof previewLocalAccount>> | null
  >(null);
  const [confirmImportLocalOpen, setConfirmImportLocalOpen] = useState(false);

  const adapter = adapters.find((a) => a.id === account.toolId);
  const toolMeta = getToolBrandMeta(account.toolId, adapters);
  const isHealthy = account.status === "active";
  const isDisabled = account.status === "disabled";
  const canClaim = isHealthy && Boolean(adapter?.capabilities.supportsDailyClaim);
  const canQuery = adapter?.capabilities.supportsQuotaQuery === true;
  const canSwitch = adapter?.capabilities.supportsClientSwitch === true;
  const supportsWebviewOAuth =
    adapter?.capabilities.authMethods.includes("webview_oauth") ?? false;
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
    try {
      await refreshAccountQuota(account.id);
    } finally {
      setRefreshing(false);
    }
  };

  const handleRecover = async () => {
    setRecovering(true);
    try {
      await recoverAccount(account.id);
    } finally {
      setRecovering(false);
    }
  };

  const handleDirectOAuth = async () => {
    if (oauthLaunching) return;
    if (!supportsWebviewOAuth && onOpenBindModal) {
      onOpenBindModal(account);
      return;
    }
    setOauthLaunching(true);
    try {
      await importAccount(account.toolId, "", "webview_oauth", undefined, account.id);
    } catch {
      // 错误提示由 context 统一展示
    } finally {
      setOauthLaunching(false);
    }
  };

  const handleAssociateLocal = async () => {
    setAssociating(true);
    try {
      // Qoder 的 auth.v1.dat 为 Chromium Safe Storage 加密，import_from_local
      // 无法解密，因此走专用采集路径：后端原样读取字节 + 校验本机登录 uid，
      // 成功后该账号即获得 LocalSnapshot，可正常切号。
      if (account.toolId === "qoder_cn" && isTauriRuntime) {
        await captureLocalSession(account.id);
        return;
      }

      if (!isTauriRuntime) {
        await importAccount(account.toolId, "", "local_import", undefined, account.id).catch(
          () => {}
        );
        return;
      }

      // 先只读预检本机登录账号，避免在错误卡片上反复触发后端拒绝
      let preview: Awaited<ReturnType<typeof previewLocalAccount>>;
      try {
        preview = await previewLocalAccount(account.toolId, account.id);
      } catch (err) {
        showToast(`读取本机会话失败：${String(err)}`, "error");
        return;
      }

      if (preview.matchesTarget) {
        await importAccount(account.toolId, "", "local_import", undefined, account.id).catch(
          () => {}
        );
        return;
      }

      const localName = preview.displayName?.trim() || preview.uidMasked;
      if (preview.isFallbackUid) {
        showToast(
          "无法识别本机登录账号（客户端可能未登录或会话尚未落盘），请确认已登录后重试",
          "warning"
        );
        return;
      }
      if (preview.matchedAccountId) {
        showToast(
          `本机当前登录的是「${preview.matchedAccountLabel ?? localName}」（${preview.uidMasked}），不是「${account.label}」。请先在 ${toolMeta.name} 客户端登录「${account.label}」后再关联。`,
          "warning"
        );
        return;
      }

      // 本机账号尚未导入：引导用户直接把它添加为 Qwitch 账号
      setLocalPreview(preview);
      setConfirmImportLocalOpen(true);
    } finally {
      setAssociating(false);
    }
  };

  const handleSaveRename = async () => {
    if (renaming) return;
    const trimmed = labelDraft.trim();
    if (trimmed && trimmed !== account.label) {
      setRenaming(true);
      try {
        await renameAccount(account.id, trimmed);
      } finally {
        setRenaming(false);
      }
    }
    setEditingLabel(false);
  };

  return (
    <>
      <div
        className={`group surface-card surface-interactive flex h-full flex-col p-3.5 ${
          account.isActiveOnClient
            ? "border-emerald-500/45 ring-1 ring-emerald-500/15 dark:border-emerald-400/35 dark:ring-emerald-400/15"
            : isDisabled
              ? "bg-zinc-50/70 opacity-85 dark:bg-zinc-900/50"
              : ""
        }`}
      >
        <div className="flex flex-col">
          <div className="flex items-start justify-between gap-2">
            <div className="flex min-w-0 flex-1 items-start gap-2.5">
              <ToolBrandIcon toolId={account.toolId} size="md" className="mt-0.5" decorative />
              <div className="min-w-0 flex-1">
                {account.latestQuota?.planName && (
                  <div className="mb-0.5 flex flex-wrap items-center gap-1.5">
                    <span
                      className={`badge shrink-0 whitespace-nowrap ${toolMeta.badgeClass}`}
                    >
                      {account.latestQuota.planName}
                    </span>
                  </div>
                )}

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
                      disabled={renaming}
                      onClick={() => void handleSaveRename()}
                      aria-label="保存账号备注"
                      className="btn btn-icon btn-primary rounded-md disabled:opacity-50"
                    >
                      <Check className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setLabelDraft(account.label);
                        setEditingLabel(false);
                      }}
                      aria-label="取消重命名"
                      className="btn btn-icon btn-secondary rounded-md"
                    >
                      <X className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                  </div>
                ) : (
                  <div className="flex items-center gap-1">
                    <h3 className="truncate text-[15px] font-semibold text-zinc-900 dark:text-zinc-100">
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
                      className="shrink-0 rounded p-0.5 text-zinc-400 opacity-0 transition hover:bg-zinc-100 hover:text-zinc-800 group-hover:opacity-100 focus-visible:opacity-100 dark:text-zinc-500 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                    >
                      <Pencil className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                  </div>
                )}

                <p className="truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                  {account.uidMasked}
                  {account.emailMasked ? ` · ${account.emailMasked}` : ""}
                  {account.deviceIdMasked && (
                    <span
                      className={`inline-flex items-center gap-0.5 ${
                        account.deviceType === "win" ? "text-sky-700 dark:text-sky-300" : ""
                      }`}
                      title={`设备 ${account.deviceIdMasked}${account.deviceType ? ` · ${account.deviceType}` : ""}：该账号的独立设备身份，可与其它设备账号并行签到`}
                    >
                      {" · "}
                      <Cpu className="h-2.5 w-2.5" aria-hidden="true" />
                      {account.deviceIdMasked}
                    </span>
                  )}
                </p>
              </div>
            </div>

            <div className="flex shrink-0 flex-col items-end gap-1">
              <AccountStatusBadge
                status={account.status}
                missingApiToken={missingApiToken}
                missingLocalSnapshot={missingLocalSnapshot}
              />
              {canClaim && !missingApiToken && (
                <ClaimStatusBadge status={effectiveClaimStatus} />
              )}
            </div>
          </div>

          <div className="mt-3 flex flex-col space-y-2">
            {missingApiToken ? (
              <div className="flex flex-col gap-2 rounded-lg border border-amber-500/25 bg-amber-500/8 px-3 py-2.5 text-xs">
                <div className="flex items-start gap-1.5 text-amber-900 dark:text-amber-200">
                  <AlertCircle
                    className="mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-400"
                    aria-hidden="true"
                  />
                  <span className="leading-snug">
                    已绑定切号快照，授权后开启查额与签到
                  </span>
                </div>
                <div className="flex flex-wrap items-center gap-1.5">
                  <button
                    type="button"
                    disabled={oauthLaunching}
                    onClick={() => void handleDirectOAuth()}
                    className="btn btn-sm gap-1 whitespace-nowrap bg-amber-600 text-white transition hover:bg-amber-700 disabled:opacity-50 dark:bg-amber-500 dark:text-zinc-950 dark:hover:bg-amber-400"
                  >
                    <Globe className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    {oauthLaunching ? "授权中..." : "在线授权"}
                  </button>
                  {onOpenBindModal && (
                    <button
                      type="button"
                      onClick={() => onOpenBindModal(account, "token_paste")}
                      className="btn btn-sm shrink-0 gap-1 whitespace-nowrap border border-amber-500/35 bg-white/80 text-[11px] font-medium text-amber-900 transition hover:bg-white dark:bg-zinc-900/70 dark:text-amber-200 dark:hover:bg-zinc-800"
                    >
                      <ClipboardPaste className="h-3 w-3 shrink-0" aria-hidden="true" />
                      粘贴凭据
                    </button>
                  )}
                </div>
              </div>
            ) : primaryBucket ? (
              <>
                <div>
                  {(() => {
                    const isQoderClaimed =
                      isQoderClaimBucket && effectiveClaimStatus === "claimed";
                    const isQoderAvailable =
                      isQoderClaimBucket && effectiveClaimStatus === "available";
                    const isQoderPendingVerify =
                      isQoderClaimBucket && !isQoderClaimed && !isQoderAvailable;
                    const pct = isQoderClaimed
                      ? 100
                      : isQoderPendingVerify
                        ? 0
                        : primaryBucket.total > 0
                          ? Math.min(
                              100,
                              Math.round((primaryBucket.remaining / primaryBucket.total) * 100)
                            )
                          : 0;
                    const barColor = isQoderClaimed
                      ? "bg-emerald-500"
                      : isQoderPendingVerify
                        ? "bg-zinc-400 dark:bg-zinc-500"
                        : pct <= 15
                          ? "bg-rose-500"
                          : pct <= 35
                            ? "bg-amber-500"
                            : "meter-fill";
                    const pctBadge = isQoderClaimed
                      ? "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300"
                      : isQoderPendingVerify
                        ? "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300"
                        : pct <= 15
                          ? "bg-rose-500/12 text-rose-700 dark:text-rose-300"
                          : pct <= 35
                            ? "bg-amber-500/12 text-amber-800 dark:text-amber-300"
                            : "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300";
                    const pctTextClass = isQoderClaimed
                      ? "text-emerald-700 dark:text-emerald-300"
                      : pct <= 15
                        ? "text-rose-600 dark:text-rose-400"
                        : pct <= 35
                          ? "text-amber-700 dark:text-amber-300"
                          : "text-zinc-500 dark:text-zinc-400";

                    const badgeText = isQoderClaimed
                      ? "本周期已领"
                      : isQoderPendingVerify
                        ? "待核实"
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
                            {isQoderClaimBucket && (
                              <span
                                className={`shrink-0 whitespace-nowrap rounded px-1.5 py-0.5 text-[11px] font-semibold ${pctBadge}`}
                              >
                                {badgeText}
                              </span>
                            )}
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
                                <span className={`ml-1 font-semibold ${pctTextClass}`}>
                                  · {pct}%
                                </span>
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
                          className="meter"
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
                    className="flex items-center justify-between gap-2 rounded-lg bg-zinc-50/80 px-3 py-1.5 text-xs dark:bg-zinc-800/50"
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
              <div className="rounded-lg border border-dashed border-zinc-200 px-3 py-3 text-center text-xs text-zinc-500 dark:border-zinc-800 dark:text-zinc-400">
                {account.latestQuota
                  ? account.latestQuota.claimStatus === "unsupported" &&
                    adapter?.capabilities.supportsDailyClaim
                    ? "本周期暂无可用签到福利活动"
                    : "已同步额度，当前套餐无细分额度桶"
                  : "暂无额度快照，点击下方「查额」获取最新数据"}
              </div>
            )}
          </div>

          <div className="mt-2.5 flex items-center justify-between gap-2 text-xs text-zinc-500 dark:text-zinc-400">
            <div className="flex flex-wrap items-center gap-2">
              {needsRecovery ? (
                <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 text-amber-700 dark:text-amber-300">
                  <AlertCircle className="h-3.5 w-3.5" aria-hidden="true" />
                  {account.status === "expired" ? "在线凭据已过期" : "风控暂停，需恢复"}
                </span>
              ) : isDisabled ? (
                <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 text-zinc-600 dark:text-zinc-400">
                  <PauseCircle className="h-3.5 w-3.5" aria-hidden="true" />
                  已停用（跳过批量任务）
                </span>
              ) : (
                <span
                  className="inline-flex shrink-0 items-center gap-1 text-zinc-500 dark:text-zinc-400"
                  title="该账号当前已具备的能力"
                >
                  <Check className="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                  {(() => {
                    const caps = [
                      !missingLocalSnapshot && canSwitch ? "切号" : null,
                      !missingApiToken && canQuery ? "查额" : null,
                      !missingApiToken && canClaim ? "签到" : null,
                    ].filter(Boolean);
                    return caps.length ? `${caps.join(" · ")}就绪` : "就绪";
                  })()}
                </span>
              )}
            </div>
            <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 tabular-nums">
              <Clock className="h-3.5 w-3.5" aria-hidden="true" />
              {formatSyncTime(account.latestQuota?.fetchedAt ?? account.updatedAt)}
            </span>
          </div>
        </div>

        <div className="mt-auto flex flex-wrap items-center justify-between gap-2 border-t border-zinc-200/70 pt-2.5 dark:border-zinc-800/70">
          <div className="flex flex-wrap items-center gap-1.5">
            {needsRecovery ? (
              <>
                <button
                  type="button"
                  disabled={oauthLaunching}
                  onClick={() => void handleDirectOAuth()}
                  className="btn btn-sm gap-1 whitespace-nowrap border border-amber-500/25 bg-amber-500/15 text-amber-800 transition hover:bg-amber-500/25 disabled:opacity-50 dark:text-amber-300"
                >
                  <Globe className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  {oauthLaunching ? "授权中..." : "重新授权"}
                </button>
                {onOpenBindModal && (
                  <button
                    type="button"
                    onClick={() => onOpenBindModal(account, "token_paste")}
                    title="粘贴凭据（Token 或 JSON）"
                    aria-label={`粘贴凭据 ${account.label}`}
                    className="btn btn-icon btn-secondary shrink-0"
                  >
                    <ClipboardPaste className="h-3.5 w-3.5" aria-hidden="true" />
                  </button>
                )}
                <button
                  type="button"
                  disabled={recovering || isBatchRunning}
                  onClick={() => void handleRecover()}
                  title={recovering ? "恢复中..." : "重试恢复"}
                  aria-label={recovering ? "恢复中" : `重试恢复 ${account.label}`}
                  className="btn btn-icon btn-secondary shrink-0 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <RotateCcw
                    className={`h-3.5 w-3.5 ${recovering ? "animate-spin" : ""}`}
                    aria-hidden="true"
                  />
                </button>
              </>
            ) : missingApiToken ? null : (
              <>
                <button
                  type="button"
                  onClick={() => void handleRefresh()}
                  disabled={refreshing || isDisabled || isBatchRunning || !canQuery}
                  title={
                    !canQuery
                      ? "该适配器声明不支持查额"
                      : isBatchRunning
                        ? "批量任务进行中，查额已排队禁用"
                        : "刷新额度"
                  }
                  className="btn btn-sm btn-secondary gap-1 whitespace-nowrap disabled:cursor-not-allowed disabled:opacity-45"
                >
                  <RefreshCw
                    className={`h-3.5 w-3.5 shrink-0 ${refreshing ? "animate-spin" : ""}`}
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
                      className="btn btn-sm btn-sky-soft gap-1 whitespace-nowrap"
                    >
                      <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                      领取
                    </button>
                  )}
              </>
            )}

            <OverflowMenu
              label={`账号管理 ${account.label}`}
              items={[
                {
                  label: isDisabled ? "恢复启用" : "停用账号",
                  icon: isDisabled ? PlayCircle : PauseCircle,
                  disabled: togglingStatus,
                  onSelect: async () => {
                    if (togglingStatus) return;
                    setTogglingStatus(true);
                    try {
                      await toggleAccountDisabled(account.id);
                    } finally {
                      setTogglingStatus(false);
                    }
                  },
                },
                {
                  label: "删除账号",
                  icon: Trash2,
                  danger: true,
                  onSelect: () => setConfirmDeleteOpen(true),
                },
              ]}
            />
          </div>

          {missingLocalSnapshot ? (
            <button
              type="button"
              disabled={associating}
              onClick={() => void handleAssociateLocal()}
              title={
                account.toolId === "qoder_cn"
                  ? `读取本机 Qoder 当前登录会话的原始字节并绑定到该账号；请先在 Qoder 客户端中登录「${account.label}」`
                  : `读取本机 ${toolMeta.name} 当前登录的会话并合并到该账号；需要客户端已登录「${account.label}」`
              }
              className="btn btn-sm btn-secondary gap-1 whitespace-nowrap disabled:opacity-50"
            >
              <Link2 className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
              {associating
                ? "关联中..."
                : account.toolId === "qoder_cn"
                  ? "采集本机会话"
                  : "关联本机"}
            </button>
          ) : (
            account.isActiveOnClient ? (
              <span className="badge shrink-0 whitespace-nowrap border-emerald-500/25 bg-emerald-500/12 font-semibold text-emerald-800 dark:text-emerald-300">
                <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                本机使用中
              </span>
            ) : (
              <button
                type="button"
                disabled={!canSwitch}
                title={!canSwitch ? "该适配器声明不支持切号" : undefined}
                onClick={() => requestColdSwitch(account)}
                className="btn btn-sm btn-brand-soft gap-1 whitespace-nowrap"
              >
                <ArrowLeftRight className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                切至桌面端
              </button>
            )
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

      <ConfirmDialog
        open={confirmImportLocalOpen && Boolean(localPreview)}
        title="本机当前登录的是另一个账号"
        description={`本机 ${toolMeta.name} 当前登录：${localPreview?.displayName?.trim() || "未知账号"}（${localPreview?.uidMasked ?? ""}）；这张卡片是「${account.label}」（${account.uidMasked}）。若要让「${account.label}」可切号，请先在客户端登录它；也可以把本机账号直接导入为新的 Qwitch 账号。`}
        confirmLabel="导入本机账号"
        onCancel={() => {
          setConfirmImportLocalOpen(false);
          setLocalPreview(null);
        }}
        onConfirm={async () => {
          setConfirmImportLocalOpen(false);
          setLocalPreview(null);
          try {
            await importAccount(account.toolId, "", "local_import");
          } catch {
            // 错误 Toast 已由 context 统一展示
          }
        }}
      />
    </>
  );
});
