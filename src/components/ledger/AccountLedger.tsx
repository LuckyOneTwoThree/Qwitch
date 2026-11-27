import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeftRight,
  Check,
  MoreHorizontal,
  PauseCircle,
  PlayCircle,
  RefreshCw,
  Sparkles,
  Trash2,
} from "lucide-react";
import type { AccountSummary } from "../../types/qwitch";
import { useQwitch } from "../../context/QwitchContext";
import { ToolBrandIcon } from "../ToolBrandIcon";
import { OverflowMenu } from "../ui/OverflowMenu";
import { ConfirmDialog } from "../ui/ConfirmDialog";
import { formatRelativeSyncTime, isSnapshotStale } from "../../domain/time";
import {
  claimAvailability,
  formatQuotaNumber,
  sortLedgerAccounts,
  toQuotaView,
  toStatusView,
  toolNameOf,
  waterTone,
  LEDGER_SORT_LABEL,
  type LedgerSortKey,
} from "./ledgerModel";

const TONE_TEXT: Record<string, string> = {
  ok: "text-zinc-500 dark:text-zinc-400",
  warn: "text-amber-700 dark:text-amber-300",
  danger: "text-rose-700 dark:text-rose-300",
  muted: "text-zinc-400 dark:text-zinc-500",
};
const TONE_DOT: Record<string, string> = {
  ok: "bg-zinc-300 dark:bg-zinc-600",
  warn: "bg-amber-500",
  danger: "bg-rose-500",
  muted: "bg-zinc-300 dark:bg-zinc-600",
};

export interface AccountLedgerProps {
  /** 已做作用域与搜索/状态过滤，未排序 */
  accounts: AccountSummary[];
  sort: LedgerSortKey;
  onSortChange: (sort: LedgerSortKey) => void;
  selectedIds: Set<string>;
  onSelectedIdsChange: (next: Set<string>) => void;
  /** 打开账号详情抽屉 */
  onOpenDetail?: (account: AccountSummary) => void;
  /** 已选但不在当前筛选里的数量，用于批量条提示 */
  hiddenSelectedCount?: number;
}

export function AccountLedger({
  accounts,
  sort,
  onSortChange,
  selectedIds,
  onSelectedIdsChange,
  onOpenDetail,
  hiddenSelectedCount = 0,
}: AccountLedgerProps) {
  const {
    adapters,
    clientStatuses,
    refreshAccountQuota,
    claimDailyBatch,
    requestColdSwitch,
    toggleAccountDisabled,
    deleteAccount,
    launchToolClient,
    batchProgress,
  } = useQwitch();

  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
  const [activeIndex, setActiveIndex] = useState(0);
  const [deleteTarget, setDeleteTarget] = useState<AccountSummary | null>(null);
  const rowRefs = useRef<Array<HTMLDivElement | null>>([]);
  const lastClickedIndex = useRef<number | null>(null);

  const rows = useMemo(() => sortLedgerAccounts(accounts, sort), [accounts, sort]);
  const batchRunning = Boolean(batchProgress?.running);

  // 行数是变动的（筛选/作用域切换）；若 activeIndex 越界，就没有任何一行带 tabIndex=0，
  // 整个表格会从 Tab 序列里消失。这里随行数收敛。
  useEffect(() => {
    setActiveIndex((i) => (rows.length === 0 ? 0 : Math.min(i, rows.length - 1)));
  }, [rows.length]);

  const markBusy = (ids: string[], busy: boolean) => {
    setBusyIds((prev) => {
      const next = new Set(prev);
      for (const id of ids) {
        if (busy) next.add(id);
        else next.delete(id);
      }
      return next;
    });
  };

  const runForAccount = useCallback(
    async (account: AccountSummary, run: () => Promise<void>) => {
      markBusy([account.id], true);
      try {
        await run();
      } finally {
        markBusy([account.id], false);
      }
    },
    []
  );

  const toggleOne = useCallback(
    (id: string) => {
      const next = new Set(selectedIds);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      onSelectedIdsChange(next);
    },
    [selectedIds, onSelectedIdsChange]
  );

  const selectRange = useCallback(
    (from: number, to: number) => {
      const [lo, hi] = from <= to ? [from, to] : [to, from];
      const next = new Set(selectedIds);
      for (let i = lo; i <= hi; i += 1) {
        const acc = rows[i];
        if (acc) next.add(acc.id);
      }
      onSelectedIdsChange(next);
    },
    [rows, selectedIds, onSelectedIdsChange]
  );

  const handleSelectClick = (index: number, event: React.MouseEvent) => {
    const acc = rows[index];
    if (!acc) return;
    if (event.shiftKey && lastClickedIndex.current !== null) {
      selectRange(lastClickedIndex.current, index);
    } else {
      toggleOne(acc.id);
      lastClickedIndex.current = index;
    }
    setActiveIndex(index);
  };

  const focusRow = (index: number) => {
    const clamped = Math.max(0, Math.min(rows.length - 1, index));
    rowRefs.current[clamped]?.focus();
    setActiveIndex(clamped);
  };

  const handleRowKeyDown = (index: number, event: React.KeyboardEvent<HTMLDivElement>) => {
    // 只在行本身获得焦点时接管按键，避免抢走行内按钮的空格/回车
    if (event.target !== event.currentTarget) return;
    const acc = rows[index];
    if (!acc) return;

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        focusRow(index + 1);
        break;
      case "ArrowUp":
        event.preventDefault();
        focusRow(index - 1);
        break;
      case "Home":
        event.preventDefault();
        focusRow(0);
        break;
      case "End":
        event.preventDefault();
        focusRow(rows.length - 1);
        break;
      case " ":
        // 与复选框对齐：空格是「切换」，不是「只加选」
        event.preventDefault();
        if (event.shiftKey && lastClickedIndex.current !== null) {
          selectRange(lastClickedIndex.current, index);
        } else {
          toggleOne(acc.id);
          lastClickedIndex.current = index;
        }
        break;
      case "Enter":
        event.preventDefault();
        if (event.metaKey || event.ctrlKey) {
          void runForAccount(acc, () => refreshAccountQuota(acc.id));
        } else {
          onOpenDetail?.(acc);
        }
        break;
      default:
        break;
    }
  };

  const allVisibleSelected = rows.length > 0 && rows.every((r) => selectedIds.has(r.id));

  const toggleSelectAll = () => {
    if (allVisibleSelected) {
      const next = new Set(selectedIds);
      for (const r of rows) next.delete(r.id);
      onSelectedIdsChange(next);
    } else {
      const next = new Set(selectedIds);
      for (const r of rows) next.add(r.id);
      onSelectedIdsChange(next);
    }
  };

  const sortButton = (key: LedgerSortKey, label: string, className: string) => (
    <button
      type="button"
      onClick={() => onSortChange(key)}
      aria-label={`排序：${LEDGER_SORT_LABEL[key]}`}
      className={`${className} inline-flex items-center gap-1 rounded px-1 py-0.5 text-left transition hover:text-zinc-800 dark:hover:text-zinc-200 ${
        sort === key ? "font-semibold text-zinc-800 dark:text-zinc-100" : ""
      }`}
    >
      {label}
      {sort === key && <span aria-hidden="true">↑</span>}
    </button>
  );

  return (
    <>
      <div className="overflow-hidden rounded-xl border border-zinc-200/70 bg-white dark:border-white/[0.07] dark:bg-white/[0.015]">
      <div
        role="grid"
        aria-label="账号台账"
        aria-rowcount={rows.length}
        aria-colcount={8}
        className="max-h-full"
      >
        <div role="row" className="ledger-grid ledger-head font-medium">
          <span aria-hidden="true" />
          <span role="columnheader" aria-sort={sort === "name" ? "ascending" : "none"}>
            {sortButton("name", "账号", "")}
          </span>
          <span role="columnheader" className="ledger-col-tool">工具</span>
          <span role="columnheader" className="ledger-col-tier">套餐</span>
          <span role="columnheader" className="text-right">剩余额度</span>
          <span role="columnheader">{sortButton("quota-asc", "水位", "")}</span>
          <span role="columnheader">状态</span>
          <span role="columnheader" className="ledger-col-sync">
            {sortButton("recent", "同步", "")}
          </span>
          <span aria-hidden="true" />
        </div>

        {rows.map((account, index) => {
          const quota = toQuotaView(account);
          const status = toStatusView(account, adapters);
          const claim = claimAvailability(account, adapters);
          const tone = waterTone(quota.pct);
          const selected = selectedIds.has(account.id);
          const busy = busyIds.has(account.id);
          const adapter = adapters.find((a) => a.id === account.toolId);
          const canQuery = adapter?.capabilities.supportsQuotaQuery === true;
          const canSwitch = adapter?.capabilities.supportsClientSwitch === true;
          const stale = isSnapshotStale(account.latestQuota?.fetchedAt);
          const syncLabel = account.latestQuota?.fetchedAt
            ? formatRelativeSyncTime(account.latestQuota.fetchedAt)
            : "未查额";

          return (
            <div
              key={account.id}
              role="row"
              ref={(el) => {
                rowRefs.current[index] = el;
              }}
              tabIndex={index === activeIndex ? 0 : -1}
              data-selected={selected}
              data-row-index={index}
              aria-selected={selected}
              aria-rowindex={index + 2}
              onKeyDown={(e) => handleRowKeyDown(index, e)}
              onFocus={() => setActiveIndex(index)}
              onClick={() => setActiveIndex(index)}
              onDoubleClick={() => onOpenDetail?.(account)}
              className="ledger-grid ledger-row cursor-default"
            >
              <span role="gridcell" className="flex items-center">
                <button
                  type="button"
                  role="checkbox"
                  aria-checked={selected}
                  aria-label={`选择 ${account.label}`}
                  data-checked={selected}
                  onClick={(e) => handleSelectClick(index, e)}
                  className="ledger-check"
                >
                  <Check className="h-2.5 w-2.5" aria-hidden="true" strokeWidth={3} />
                </button>
              </span>

              <span role="gridcell" className="flex min-w-0 items-center gap-2">
                <ToolBrandIcon toolId={account.toolId} size="xs" decorative />
                <span className="min-w-0 flex-1">
                  <span className="flex min-w-0 items-center gap-1.5">
                    {status.mounted && (
                      <span
                        title="本机客户端当前登录此账号"
                        aria-label="本机在用"
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500 shadow-[0_0_0_2.5px_rgba(16,185,129,0.16)]"
                      />
                    )}
                    <span className="truncate text-[12.5px] font-semibold tracking-tight text-zinc-900 dark:text-zinc-100">
                      {account.label}
                    </span>
                  </span>
                  <span className="mt-px block truncate font-mono text-[10.5px] text-zinc-500 dark:text-zinc-400">
                    {account.uidMasked}
                    {account.emailMasked ? ` · ${account.emailMasked}` : ""}
                  </span>
                </span>
              </span>

              <span role="gridcell" className="ledger-col-tool min-w-0 items-center gap-1.5">
                <span className="truncate text-[11.5px] text-zinc-600 dark:text-zinc-300">
                  {toolNameOf(account.toolId, adapters)}
                </span>
                {clientStatuses[account.toolId]?.isRunning && (
                  <span
                    className="h-1 w-1 shrink-0 rounded-full bg-emerald-500"
                    title="客户端运行中"
                    aria-hidden="true"
                  />
                )}
              </span>

              <span role="gridcell" className="ledger-col-tier items-center">
                {account.latestQuota?.planName ? (
                  <span className="inline-flex max-w-full items-center rounded bg-zinc-50 px-1.5 py-px text-[10px] font-medium text-zinc-600 ring-1 ring-inset ring-zinc-200 dark:bg-white/[0.04] dark:text-zinc-300 dark:ring-white/10">
                    <span className="truncate">{account.latestQuota.planName}</span>
                  </span>
                ) : (
                  <span className="text-[10.5px] text-zinc-400 dark:text-zinc-600">—</span>
                )}
              </span>

              <span role="gridcell" className="truncate text-right">
                {quota.kind === "ratio" || quota.kind === "percent" ? (
                  <>
                    <span className="text-[12.5px] font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                      {quota.kind === "percent"
                        ? `${quota.pct}%`
                        : formatQuotaNumber(quota.remaining ?? 0)}
                    </span>
                    {quota.kind === "ratio" && (
                      <span className="ml-1 text-[10.5px] text-zinc-500 dark:text-zinc-400">
                        / {formatQuotaNumber(quota.total ?? 0)} {quota.unit}
                      </span>
                    )}
                    {quota.extraBuckets > 0 && (
                      <span
                        title={`另有 ${quota.extraBuckets} 个额度桶`}
                        className="ml-1 rounded bg-zinc-100 px-1 py-px text-[10px] text-zinc-500 dark:bg-white/[0.08] dark:text-zinc-400"
                      >
                        +{quota.extraBuckets}
                      </span>
                    )}
                  </>
                ) : quota.kind === "balance" ? (
                  <>
                    <span className="text-[12.5px] font-semibold tabular-nums text-zinc-900 dark:text-zinc-100">
                      {formatQuotaNumber(quota.remaining ?? 0)}
                    </span>
                    <span className="ml-1 text-[10.5px] text-zinc-500 dark:text-zinc-400">
                      {quota.unit}
                    </span>
                  </>
                ) : quota.kind === "claim" ? (
                  <span className="text-[11.5px] text-zinc-500 dark:text-zinc-400">
                    {claim === "done" ? "本周期已领" : "按签到计"}
                  </span>
                ) : (
                  <span className="text-[11.5px] text-zinc-400 dark:text-zinc-500">未查额</span>
                )}
              </span>

              <span role="gridcell" className="flex items-center gap-2">
                {quota.pct === null ? (
                  <span
                    className="text-[10.5px] text-zinc-400 dark:text-zinc-600"
                    title={quota.kind === "balance" ? "服务端未给出可信总量，不做百分比推测" : undefined}
                  >
                    —
                  </span>
                ) : (
                  <>
                    <span
                      role="progressbar"
                      aria-label={`${account.label} 水位`}
                      aria-valuenow={quota.pct}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      className="ledger-meter"
                    >
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
                  </>
                )}
              </span>

              <span role="gridcell" className={`flex items-center gap-1.5 text-[11.5px] ${TONE_TEXT[status.tone]}`}>
                <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${TONE_DOT[status.tone]}`} aria-hidden="true" />
                <span className="truncate">{status.label}</span>
              </span>

              <span
                role="gridcell"
                className={`ledger-col-sync items-center font-mono text-[10.5px] ${
                  stale ? "text-amber-700 dark:text-amber-300" : "text-zinc-500 dark:text-zinc-400"
                }`}
                title={account.latestQuota?.fetchedAt ? "最近一次额度同步时间" : "尚未查询额度"}
              >
                <span className="truncate">{syncLabel}</span>
              </span>

              <span role="gridcell" className="ledger-actions">
                <button
                  type="button"
                  onClick={() => void runForAccount(account, () => refreshAccountQuota(account.id))}
                  disabled={busy || batchRunning || !canQuery}
                  aria-label={`查额 ${account.label}`}
                  title={canQuery ? "刷新额度" : "该适配器不支持查额"}
                  className="ledger-act-secondary btn btn-icon btn-ghost h-6 w-6 shrink-0 !p-0 disabled:opacity-40"
                >
                  <RefreshCw className={`h-3 w-3 ${busy ? "animate-spin" : ""}`} aria-hidden="true" />
                </button>

                {(claim === "ready" || claim === "verify") && (
                  <button
                    type="button"
                    onClick={() => void runForAccount(account, () => claimDailyBatch([account.id]))}
                    disabled={busy || batchRunning}
                    aria-label={`领取 ${account.label}`}
                    title={claim === "verify" ? "先核实今日签到状态，未领则自动领取" : "领取今日签到福利"}
                    className="ledger-act-secondary btn btn-icon h-6 w-6 shrink-0 !p-0 text-brand-700 disabled:opacity-40 dark:text-brand-300"
                  >
                    <Sparkles className="h-3 w-3" aria-hidden="true" />
                  </button>
                )}

                {canSwitch && (
                  <button
                    type="button"
                    onClick={() => requestColdSwitch(account)}
                    disabled={busy || batchRunning}
                    aria-label={`切换 ${account.label} 至桌面端`}
                    title="切换至桌面端"
                    className="ledger-act-secondary btn btn-icon btn-ghost h-6 w-6 shrink-0 !p-0 disabled:opacity-40"
                  >
                    <ArrowLeftRight className="h-3 w-3" aria-hidden="true" />
                  </button>
                )}

                <OverflowMenu
                  label={`账号管理 ${account.label}`}
                  items={[
                    {
                      label: "打开客户端",
                      icon: MoreHorizontal,
                      onSelect: () => void launchToolClient(account.toolId),
                    },
                    {
                      label: account.status === "disabled" ? "恢复启用" : "停用账号",
                      icon: account.status === "disabled" ? PlayCircle : PauseCircle,
                      disabled: busy,
                      onSelect: () =>
                        void runForAccount(account, () => toggleAccountDisabled(account.id)),
                    },
                    {
                      label: "删除账号",
                      icon: Trash2,
                      danger: true,
                      onSelect: () => setDeleteTarget(account),
                    },
                  ]}
                />
              </span>
            </div>
          );
        })}
      </div>

      {/* 全选行：放在表尾，避免表头左侧出现一个「全选」把列宽挤歪 */}
      <div className="flex items-center justify-between gap-3 border-t border-zinc-200/70 px-3.5 py-2 text-[11px] text-zinc-500 dark:border-white/[0.07] dark:text-zinc-400">
        <button
          type="button"
          onClick={toggleSelectAll}
          aria-pressed={allVisibleSelected}
          className="inline-flex items-center gap-2 rounded px-1 py-0.5 transition hover:text-zinc-800 dark:hover:text-zinc-200"
        >
          <span data-checked={allVisibleSelected} className="ledger-check">
            <Check className="h-2.5 w-2.5" aria-hidden="true" strokeWidth={3} />
          </span>
          全选当前 {rows.length} 个
        </button>
        <span className="tabular-nums">
          共 {rows.length} 个账号
          {hiddenSelectedCount > 0 && ` · 另有 ${hiddenSelectedCount} 个已选不在当前筛选`}
        </span>
      </div>
      </div>

      {selectedIds.size > 0 && (
        <div className="pointer-events-none fixed inset-x-0 bottom-5 z-30 flex justify-center pl-[208px]">
          <div className="pointer-events-auto flex items-center gap-1.5 rounded-xl border border-zinc-200 bg-white px-3 py-1.5 shadow-float dark:border-white/[0.1] dark:bg-zinc-900">
            <span className="mr-1 text-xs text-zinc-600 dark:text-zinc-300">
              已选 <b className="font-semibold text-zinc-900 dark:text-zinc-50">{selectedIds.size}</b> 个
            </span>
            <button
              type="button"
              onClick={() => {
                // 串行：批量查额会真实打上游接口，并发容易触发限流
                void (async () => {
                  for (const id of selectedIds) {
                    await refreshAccountQuota(id).catch(() => undefined);
                  }
                })();
              }}
              disabled={batchRunning}
              className="btn btn-sm btn-ghost gap-1"
            >
              <RefreshCw className="h-3 w-3" aria-hidden="true" />
              查额
            </button>
            <button
              type="button"
              onClick={() => void claimDailyBatch([...selectedIds])}
              disabled={batchRunning}
              className="btn btn-sm btn-brand-soft gap-1"
            >
              <Sparkles className="h-3 w-3" aria-hidden="true" />
              领取
            </button>
            <button
              type="button"
              onClick={() => {
                // 直接遍历选中集合：选中项可能不在当前筛选结果里，
                // 用 rows.find() 会静默漏掉它们。
                for (const id of selectedIds) void toggleAccountDisabled(id);
              }}
              disabled={batchRunning}
              className="btn btn-sm btn-ghost gap-1"
            >
              <PauseCircle className="h-3 w-3" aria-hidden="true" />
              停用
            </button>
            <button
              type="button"
              onClick={() => onSelectedIdsChange(new Set())}
              className="ml-1 rounded px-1.5 text-xs text-zinc-500 transition hover:text-zinc-800 dark:hover:text-zinc-200"
            >
              取消
            </button>
          </div>
        </div>
      )}

      <ConfirmDialog
        open={Boolean(deleteTarget)}
        title="删除账号"
        description={
          deleteTarget
            ? `将从本地加密库中永久删除「${deleteTarget.label}」（${deleteTarget.uidMasked}）。该操作不可撤销，已加密的快照与凭据一并移除。`
            : ""
        }
        confirmLabel="删除"
        danger
        onCancel={() => setDeleteTarget(null)}
        onConfirm={() => {
          const target = deleteTarget;
          setDeleteTarget(null);
          if (target) void deleteAccount(target.id);
        }}
      />
    </>
  );
}
