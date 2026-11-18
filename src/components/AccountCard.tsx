import { useEffect, useState } from "react";
import {
  RefreshCw,
  Sparkles,
  ArrowLeftRight,
  RotateCcw,
  Trash2,
  Globe,
  Link2,
  Clock,
  Check,
} from "lucide-react";
import type { AccountSummary } from "../types/qwitch";
import { AccountStatusBadge, ClaimStatusBadge } from "./StatusBadge";
import { ToolBrandIcon, getToolBrandMeta } from "./ToolBrandIcon";
import { useQwitch } from "../context/QwitchContext";

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

export function AccountCard({ account }: { account: AccountSummary }) {
  const {
    refreshAccountQuota,
    claimDailyBatch,
    requestColdSwitch,
    recoverAccount,
    deleteAccount,
    importAccount,
    adapters,
  } = useQwitch();
  const [refreshing, setRefreshing] = useState(false);
  const [associating, setAssociating] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  useEffect(() => {
    if (!confirmDelete) return;
    const timer = setTimeout(() => setConfirmDelete(false), 3000);
    return () => clearTimeout(timer);
  }, [confirmDelete]);

  const adapter = adapters.find((a) => a.id === account.toolId);
  const toolMeta = getToolBrandMeta(account.toolId);
  const canClaim = account.status === "active" && Boolean(adapter?.capabilities.supportsDailyClaim);
  const needsRecovery = account.status === "expired" || account.status === "risk_paused";
  const missingApiToken = account.hasApiToken === false;
  const missingLocalSnapshot = account.hasLocalSnapshot === false;

  const buckets = account.latestQuota?.buckets ?? [];
  const primaryBucket = buckets[0];
  const secondaryBuckets = buckets.slice(1);

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

  return (
    <div
      className={`group flex h-full flex-col justify-between rounded-2xl border bg-white p-3.5 transition-all dark:bg-zinc-900/90 ${
        account.isActiveOnClient
          ? "border-emerald-500/45 shadow-xs ring-1 ring-emerald-500/15 dark:border-emerald-400/35 dark:ring-emerald-400/15"
          : "border-zinc-200 hover:border-zinc-300 dark:border-zinc-800 dark:hover:border-zinc-700"
      }`}
    >
      <div className="flex flex-1 flex-col">
        <div className="flex items-start justify-between gap-2">
          <div className="flex min-w-0 flex-1 items-start gap-2.5">
            <ToolBrandIcon toolId={account.toolId} size="md" className="mt-0.5" />
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="shrink-0 whitespace-nowrap text-[11px] font-medium text-zinc-500 dark:text-zinc-400">
                  {toolMeta.name}
                </span>
                {account.latestQuota?.planName && (
                  <span
                    className={`shrink-0 whitespace-nowrap rounded-md border px-1.5 py-0.5 text-[10px] font-medium ${toolMeta.badgeClass}`}
                  >
                    {account.latestQuota.planName}
                  </span>
                )}
                {account.isActiveOnClient && (
                  <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-full bg-emerald-500/12 px-2 py-0.5 text-[10px] font-semibold text-emerald-700 dark:text-emerald-300">
                    <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                    本机使用中
                  </span>
                )}
              </div>
              <h3 className="mt-0.5 truncate text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                {account.label}
              </h3>
              <p className="truncate font-mono text-[11px] text-zinc-400 dark:text-zinc-500">
                {account.uidMasked}
                {account.emailMasked ? ` · ${account.emailMasked}` : ""}
              </p>
            </div>
          </div>

          <div className="flex shrink-0 flex-col items-end gap-1">
            <AccountStatusBadge status={account.status} />
            {account.latestQuota && !missingApiToken && (
              <ClaimStatusBadge status={account.latestQuota.claimStatus} />
            )}
          </div>
        </div>

        <div className="mt-3 flex flex-1 flex-col justify-center space-y-2">
          {missingApiToken ? (
            <div className="flex items-center justify-between gap-2.5 rounded-xl border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-xs">
              <span className="leading-snug text-amber-800 dark:text-amber-200">
                已绑定切号快照 · 授权后开启查额与签到
              </span>
              <button
                type="button"
                onClick={handleDirectOAuth}
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-amber-600 px-2.5 py-1 text-[11px] font-medium text-white transition hover:bg-amber-700 dark:bg-amber-500 dark:text-zinc-950 dark:hover:bg-amber-400"
              >
                <Globe className="h-3 w-3 shrink-0" />
                在线授权
              </button>
            </div>
          ) : primaryBucket ? (
            <>
              <div>
                {(() => {
                  const pct =
                    primaryBucket.total > 0
                      ? Math.min(
                          100,
                          Math.round((primaryBucket.remaining / primaryBucket.total) * 100)
                        )
                      : 0;
                  const barColor =
                    pct <= 15
                      ? "bg-rose-500"
                      : pct <= 35
                        ? "bg-amber-500"
                        : "bg-zinc-900 dark:bg-zinc-100";
                  const pctBadge =
                    pct <= 15
                      ? "bg-rose-500/10 text-rose-600 dark:text-rose-400"
                      : pct <= 35
                        ? "bg-amber-500/10 text-amber-700 dark:text-amber-300"
                        : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300";
                  return (
                    <>
                      <div className="mb-1.5 flex items-center justify-between gap-2 text-xs">
                        <div className="flex min-w-0 items-center gap-1.5">
                          <span className="truncate text-zinc-500 dark:text-zinc-400">
                            {primaryBucket.label}
                          </span>
                          <span
                            className={`shrink-0 whitespace-nowrap rounded px-1.5 py-0.5 text-[10px] font-semibold tabular-nums ${pctBadge}`}
                          >
                            {pct}%
                          </span>
                        </div>
                        <span className="shrink-0 whitespace-nowrap font-medium tabular-nums text-zinc-900 dark:text-zinc-100">
                          {formatQuotaNumber(primaryBucket.remaining)} /{" "}
                          {formatQuotaNumber(primaryBucket.total)} {primaryBucket.unit}
                        </span>
                      </div>
                      <div className="h-2 w-full overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
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
                  <span className="truncate text-zinc-500 dark:text-zinc-400">{bucket.label}</span>
                  <span className="shrink-0 whitespace-nowrap font-medium tabular-nums text-zinc-800 dark:text-zinc-200">
                    {formatQuotaNumber(bucket.remaining)} {bucket.unit}
                  </span>
                </div>
              ))}
            </>
          ) : (
            <div className="rounded-xl border border-dashed border-zinc-200 px-3 py-3 text-center text-xs text-zinc-400 dark:border-zinc-800 dark:text-zinc-500">
              暂无额度快照，点击下方「查额」获取最新数据
            </div>
          )}
        </div>

        <div className="mt-2.5 flex items-center justify-between gap-2 text-[11px] text-zinc-400 dark:text-zinc-500">
          <div className="flex flex-wrap items-center gap-2">
            {!missingLocalSnapshot && (
              <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1">
                <Check className="h-3 w-3 text-emerald-500" />
                桌面切号就绪
              </span>
            )}
            {!missingApiToken && (
              <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1">
                <Check className="h-3 w-3 text-sky-500" />
                {canClaim ? "查额与签到就绪" : "在线查额就绪"}
              </span>
            )}
          </div>
          <span className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 tabular-nums">
            <Clock className="h-3 w-3" />
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
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg bg-amber-500/15 px-2.5 py-1 text-xs font-medium text-amber-700 transition hover:bg-amber-500/25 dark:text-amber-300"
              >
                <Globe className="h-3 w-3 shrink-0" />
                重新授权
              </button>
              <button
                type="button"
                onClick={() => recoverAccount(account.id)}
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-600 transition hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800"
              >
                <RotateCcw className="h-3 w-3 shrink-0" />
                重试
              </button>
            </>
          ) : missingApiToken ? null : (
            <>
              <button
                type="button"
                onClick={handleRefresh}
                disabled={refreshing}
                title="刷新额度"
                className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:cursor-not-allowed disabled:opacity-40 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800"
              >
                <RefreshCw className={`h-3 w-3 shrink-0 ${refreshing ? "animate-spin" : ""}`} />
                查额
              </button>

              {canClaim && (
                <button
                  type="button"
                  disabled={account.latestQuota?.claimStatus === "claimed"}
                  onClick={() => claimDailyBatch([account.id])}
                  className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border px-2.5 py-1 text-xs font-medium transition ${
                    account.latestQuota?.claimStatus === "claimed"
                      ? "cursor-default border-zinc-200 bg-zinc-50 text-zinc-400 dark:border-zinc-800 dark:bg-zinc-800/60 dark:text-zinc-500"
                      : "border-zinc-200 text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800"
                  }`}
                >
                  <Sparkles
                    className={`h-3 w-3 shrink-0 ${
                      account.latestQuota?.claimStatus === "claimed" ? "text-zinc-400" : "text-sky-500"
                    }`}
                  />
                  {account.latestQuota?.claimStatus === "claimed" ? "今日已签" : "领取"}
                </button>
              )}
            </>
          )}

          <button
            type="button"
            onClick={() => {
              if (confirmDelete) {
                setConfirmDelete(false);
                void deleteAccount(account.id);
              } else {
                setConfirmDelete(true);
              }
            }}
            title={confirmDelete ? "再次点击确认删除" : "删除账号"}
            className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border px-2 py-1 text-xs font-medium transition ${
              confirmDelete
                ? "border-rose-500/40 bg-rose-500/10 text-rose-600 hover:bg-rose-500/20 dark:text-rose-400"
                : "border-zinc-200 text-zinc-400 hover:border-rose-300 hover:text-rose-600 dark:border-zinc-700 dark:text-zinc-500 dark:hover:border-rose-800 dark:hover:text-rose-400"
            }`}
          >
            <Trash2 className="h-3 w-3 shrink-0" />
            {confirmDelete && <span>确认删除</span>}
          </button>
        </div>

        {missingLocalSnapshot ? (
          <button
            type="button"
            disabled={associating}
            onClick={() => void handleAssociateLocal()}
            title="读取本机客户端会话并合并到该账号以启用桌面端切换"
            className="inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg border border-zinc-200 px-2.5 py-1 text-xs font-medium text-zinc-700 transition hover:bg-zinc-100 disabled:opacity-50 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-800"
          >
            <Link2 className="h-3 w-3 shrink-0" />
            {associating ? "关联中..." : "关联本机"}
          </button>
        ) : (
          <button
            type="button"
            disabled={account.isActiveOnClient}
            onClick={() => requestColdSwitch(account)}
            className={`inline-flex shrink-0 whitespace-nowrap items-center gap-1 rounded-lg px-2.5 py-1 text-xs font-medium transition ${
              account.isActiveOnClient
                ? "cursor-default bg-zinc-100 text-zinc-400 dark:bg-zinc-800 dark:text-zinc-500"
                : "bg-zinc-900 text-white hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
            }`}
          >
            <ArrowLeftRight className="h-3 w-3 shrink-0" />
            {account.isActiveOnClient ? "当前使用中" : "切至桌面端"}
          </button>
        )}
      </div>
    </div>
  );
}
