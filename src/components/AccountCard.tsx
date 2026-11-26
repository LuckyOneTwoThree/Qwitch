import React, { useState } from "react";
import {
  AlertCircle,
  ArrowLeftRight,
  Check,
  ClipboardPaste,
  Clock,
  Cpu,
  ExternalLink,
  Globe,
  Link2,
  Loader2,
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
import { formatRelativeSyncTime, isSnapshotStale } from "../domain/time";
import { useQwitch } from "../context/QwitchContext";
import { ConfirmDialog } from "./ui/ConfirmDialog";
import { OverflowMenu } from "./ui/OverflowMenu";

function formatQuotaNumber(value: number): string {
  const rounded = Math.round(value * 100) / 100;
  return rounded.toLocaleString("zh-CN", { maximumFractionDigits: 2 });
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
    launchToolClient,
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
  const [launchingClient, setLaunchingClient] = useState(false);
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
  /// 凭据由官方客户端托管并自行轮换的工具（如本机导入的码道 CodeArts）。
  /// 走独立 OAuth 授权的 CodeArts 账号由 Qwitch 自动续期，不再提示仅客户端托管。
  const clientManagedCredential =
    account.toolId === "codearts" && account.renewalMode !== "auto_refresh";
  const missingApiToken = account.hasApiToken === false;
  // 豆包工作：签到发放校验绑定「设备 + 桌面端当前登录账号」。网页登录导入的
  // 其他账号点领取只会拿到 710010702 系统错误，这里直接禁用并说明要先切号。
  const claimNeedsDesktopSession =
    account.toolId === "doubaowork" && !account.isActiveOnClient;
  const missingLocalSnapshot = canSwitch && account.hasLocalSnapshot === false;
  const isBatchRunning = Boolean(batchProgress?.running);
  const effectiveClaimStatus = getEffectiveClaimStatus(account);

  const buckets = account.latestQuota?.buckets ?? [];
  const primaryBucket = buckets[0];
  const secondaryBuckets = buckets.slice(1);
  const isQoderClaimBucket =
    account.toolId === "qoder_cn" && primaryBucket?.key === "daily_credits";
  // 百分比桶：适配器拿不到绝对额度时，只能以 total = 100 / unit = "%" 表达水位。
  // 这类桶的 remaining 本身就是百分比，重复渲染「x / 100 %」纯属噪音。
  const isPercentBucket = primaryBucket?.unit === "%";
  // 余额型桶：服务端只给剩余、没有可信总量（LobsterAI 跨池积分）。
  // 这时既不能算百分比，也不该画进度条，只报剩余数字。
  const isBalanceOnlyBucket = Boolean(
    primaryBucket && primaryBucket.hasTotal === false && !isPercentBucket
  );

  // 额度快照新鲜度：只有今日同步过的快照才代表当前水位，
  // 昨日快照必须在界面上明确标注，否则用户会把陈旧额度当成实时额度。
  const quotaFetchedAt = account.latestQuota?.fetchedAt;
  const quotaStale = isSnapshotStale(quotaFetchedAt);
  const syncLabel = quotaFetchedAt
    ? formatRelativeSyncTime(quotaFetchedAt)
    : account.latestQuota
      ? "已同步"
      : "尚未查额";

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

  /// CodeArts 凭据由客户端托管并一次性轮换，Qwitch 不代刷。
  /// 过期时引导用户拉起客户端完成刷新，再回来重新导入。
  const handleLaunchClient = async () => {
    if (launchingClient) return;
    setLaunchingClient(true);
    try {
      await launchToolClient(account.toolId);
    } finally {
      setLaunchingClient(false);
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

  const hasCustomAlias = Boolean(
    account.accountName && account.accountName !== account.label
  );

  const handleSaveRename = async () => {
    if (renaming) return;
    const trimmed = labelDraft.trim();
    if ((!trimmed && hasCustomAlias) || (trimmed && trimmed !== account.label)) {
      setRenaming(true);
      try {
        await renameAccount(account.id, trimmed);
      } finally {
        setRenaming(false);
      }
    }
    setEditingLabel(false);
  };

  const handleRestoreOriginalName = async () => {
    if (renaming) return;
    setRenaming(true);
    try {
      await renameAccount(account.id, "");
    } finally {
      setRenaming(false);
      setEditingLabel(false);
    }
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
                      disabled={renaming}
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
                      placeholder={
                        account.accountName
                          ? `输入别名（留空恢复 ${account.accountName}）`
                          : "输入账号别名"
                      }
                      aria-label="编辑账号别名"
                      autoFocus
                      className="min-w-0 flex-1 rounded-[var(--radius-control)] border border-zinc-300 bg-white px-2 py-0.5 text-xs font-medium text-zinc-900 disabled:opacity-60 dark:border-zinc-600 dark:bg-zinc-800 dark:text-zinc-100"
                    />
                    <button
                      type="button"
                      disabled={renaming}
                      onClick={() => void handleSaveRename()}
                      aria-label="保存账号别名"
                      className="btn btn-icon btn-primary"
                    >
                      {renaming ? (
                        <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                      ) : (
                        <Check className="h-3.5 w-3.5" aria-hidden="true" />
                      )}
                    </button>
                    {hasCustomAlias && (
                      <button
                        type="button"
                        disabled={renaming}
                        onClick={() => void handleRestoreOriginalName()}
                        title={`恢复原用户名（${account.accountName}）`}
                        aria-label="恢复原用户名"
                        className="btn btn-icon btn-secondary"
                      >
                        <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
                      </button>
                    )}
                    <button
                      type="button"
                      disabled={renaming}
                      onClick={() => {
                        setLabelDraft(account.label);
                        setEditingLabel(false);
                      }}
                      aria-label="取消重命名"
                      className="btn btn-icon btn-secondary"
                    >
                      <X className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                  </div>
                ) : (
                  <div className="flex min-w-0 items-center gap-1">
                    <h3 className="truncate text-[15px] font-semibold text-zinc-900 dark:text-zinc-100">
                      {account.label}
                    </h3>
                    {hasCustomAlias && (
                      <span
                        title={`当前显示为本地别名，平台原用户名：${account.accountName}`}
                        className="shrink-0 rounded bg-zinc-100 px-1 py-0.5 text-[11px] leading-none font-medium text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400"
                      >
                        别名
                      </span>
                    )}
                    <button
                      type="button"
                      onClick={() => {
                        setLabelDraft(account.label);
                        setEditingLabel(true);
                      }}
                      title="修改账号别名"
                      aria-label={`修改 ${account.label} 别名`}
                      className="shrink-0 rounded p-0.5 text-zinc-400 opacity-0 transition hover:bg-zinc-100 hover:text-zinc-800 group-hover:opacity-100 focus-visible:opacity-100 dark:text-zinc-500 dark:hover:bg-zinc-800 dark:hover:text-zinc-100"
                    >
                      <Pencil className="h-3.5 w-3.5" aria-hidden="true" />
                    </button>
                  </div>
                )}

                <p className="truncate font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                  {hasCustomAlias ? `${account.accountName} · ` : ""}
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
                    className="btn btn-sm gap-1 whitespace-nowrap bg-amber-700 text-white transition hover:bg-amber-800 disabled:opacity-50 dark:bg-amber-400 dark:text-zinc-950 dark:hover:bg-amber-300"
                  >
                    <Globe className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    {oauthLaunching ? "授权中..." : "在线授权"}
                  </button>
                  {onOpenBindModal && (
                    <button
                      type="button"
                      onClick={() => onOpenBindModal(account, "token_paste")}
                      className="btn btn-sm shrink-0 gap-1 whitespace-nowrap border border-amber-500/35 bg-white/80 font-medium text-amber-900 transition hover:bg-white dark:bg-zinc-900/70 dark:text-amber-200 dark:hover:bg-zinc-800"
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
                            <span
                              className="truncate text-zinc-600 dark:text-zinc-400"
                              title={
                                isPercentBucket
                                  ? "服务端本次只返回了百分比水位，没有给出绝对额度；点「查额」可重新获取"
                                  : undefined
                              }
                            >
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
                            ) : isPercentBucket ? (
                              // 百分比桶的 total 恒为 100，再把「剩余 / 总量」原样打出来
                              // 只会得到「100 / 100 %」这种没有信息量的读数，直接给百分比。
                              <>
                                剩余 {pct}%
                                <span className="ml-1 font-normal text-zinc-500 dark:text-zinc-400">
                                  · 已用 {Math.max(0, 100 - pct)}%
                                </span>
                              </>
                            ) : isBalanceOnlyBucket ? (
                              // 只有剩余值：不拼「/ 总量」，也不给百分比，避免假读数
                              <>
                                {formatQuotaNumber(primaryBucket.remaining)}{" "}
                                {primaryBucket.unit}
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
                        {!isBalanceOnlyBucket && (
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
                        )}
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

          <div className="mt-2.5 flex flex-wrap items-center justify-between gap-x-2 gap-y-1 text-xs text-zinc-500 dark:text-zinc-400">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
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
              {!needsRecovery && !isDisabled && !missingApiToken && account.renewalMode && (
                (() => {
                  const modeLabel =
                    account.renewalMode === "auto_refresh"
                      ? "自动续期"
                      : account.renewalMode === "cookie_keepalive"
                        ? "Cookie 保活"
                        : account.renewalMode === "client_sync"
                          ? "跟随客户端"
                          : null;
                  if (!modeLabel) return null;
                  const expTitle = account.credentialExpiresAt
                    ? `当前令牌有效期至 ${new Date(account.credentialExpiresAt).toLocaleString("zh-CN")}`
                    : modeLabel;
                  return (
                    <span
                      title={expTitle}
                      className="rounded bg-zinc-100 px-1.5 py-0.5 text-[11px] font-medium text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300"
                    >
                      {modeLabel}
                    </span>
                  );
                })()
              )}
            </div>
            <span
              className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1 tabular-nums ${
                quotaStale ? "text-amber-700 dark:text-amber-300" : ""
              }`}
              title={
                quotaStale
                  ? "该额度快照不是今天的同步结果，显示的额度水位可能已过期，建议重新查额"
                  : "最近一次额度同步时间"
              }
            >
              <Clock className="h-3.5 w-3.5" aria-hidden="true" />
              {syncLabel}
              {quotaStale && <span className="font-medium">· 已过期</span>}
            </span>
          </div>
        </div>

        <div className="mt-auto flex flex-wrap items-center justify-between gap-2 border-t border-zinc-200/70 pt-2.5 dark:border-zinc-800/70">
          <div className="flex flex-wrap items-center gap-1.5">
            {needsRecovery ? (
              <>
                {clientManagedCredential && (
                  <button
                    type="button"
                    disabled={launchingClient}
                    onClick={() => void handleLaunchClient()}
                    title="该工具凭据由客户端自行刷新，打开客户端后会自动更新登录态"
                    className="btn btn-sm gap-1 whitespace-nowrap border border-amber-500/25 bg-amber-500/15 text-amber-800 transition hover:bg-amber-500/25 disabled:opacity-50 dark:text-amber-300"
                  >
                    <ExternalLink className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    {launchingClient ? "启动中..." : "打开客户端"}
                  </button>
                )}
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
                      : isDisabled
                        ? "账号已停用，恢复启用后可查额"
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
                      disabled={isBatchRunning || claimNeedsDesktopSession}
                      title={
                        claimNeedsDesktopSession
                          ? "豆包工作签到绑定桌面端登录账号：先「切至桌面端」再领取"
                          : isBatchRunning
                            ? "批量任务进行中"
                            : effectiveClaimStatus === "unknown"
                              ? "先核实今日签到状态，未领取则自动领取"
                              : "领取今日签到福利"
                      }
                      onClick={() => void claimDailyBatch([account.id])}
                      className="btn btn-sm btn-sky-soft gap-1 whitespace-nowrap disabled:opacity-45"
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
                  label: "修改别名",
                  icon: Pencil,
                  onSelect: () => {
                    setLabelDraft(account.label);
                    setEditingLabel(true);
                  },
                },
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
                // 豆包/码道即使能用 Cookie 或 safeStorage 直接切号，整份本机会话
                // 仍然更稳（客户端可能还有 leveldb 级登录态）。这里保留补全入口，
                // 不因为「可以直切」就把采集快照的能力藏起来。
                ...(canSwitch &&
                !account.isActiveOnClient &&
                (account.toolId === "doubaowork" || account.toolId === "codearts")
                  ? [
                      {
                        label: "补全本机会话快照",
                        icon: Link2,
                        disabled: associating,
                        onSelect: () => void handleAssociateLocal(),
                      },
                    ]
                  : []),
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
                <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" aria-hidden="true" />
                本机使用中
              </span>
            ) : (
              <button
                type="button"
                disabled={!canSwitch || isDisabled || isBatchRunning}
                title={
                  !canSwitch
                    ? "该适配器声明不支持切号"
                    : isDisabled
                      ? "账号已停用，恢复启用后可切号"
                      : isBatchRunning
                        ? "批量任务进行中，请稍候"
                        : undefined
                }
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
