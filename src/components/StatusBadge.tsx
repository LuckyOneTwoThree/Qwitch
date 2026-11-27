import type { AccountStatus, ClaimStatus } from "../types/qwitch";

/**
 * 状态徽标统一使用 `.badge` 基元 + 语义色调，
 * 保证「正常 / 过期 / 风控 / 停用」四种状态在圆角、内边距、字重上完全一致。
 */
export function AccountStatusBadge({
  status,
  missingApiToken = false,
  missingLocalSnapshot = false,
}: {
  status: AccountStatus;
  missingApiToken?: boolean;
  missingLocalSnapshot?: boolean;
}) {
  if (status === "active" && (missingApiToken || missingLocalSnapshot)) {
    const missingBoth = missingApiToken && missingLocalSnapshot;
    return (
      <span
        title={
          missingBoth
            ? "缺少在线授权凭据与本机切号快照"
            : missingApiToken
              ? "缺少在线授权凭据（授权后开启查额与签到）"
              : "未关联本机客户端会话（关联后可一键冷切换）"
        }
        className="badge bg-amber-500/12 text-amber-800 dark:bg-amber-500/15 dark:text-amber-300"
      >
        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-500" aria-hidden="true" />
        {missingApiToken ? "待授权" : "待关联"}
      </span>
    );
  }

  const styles: Record<AccountStatus, string> = {
    active: "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300",
    expired: "bg-amber-500/12 text-amber-800 dark:bg-amber-500/15 dark:text-amber-300",
    risk_paused: "bg-rose-500/12 text-rose-700 dark:bg-rose-500/15 dark:text-rose-300",
    disabled: "bg-zinc-900/[0.06] text-zinc-600 dark:bg-white/[0.08] dark:text-zinc-300",
  };

  const labels: Record<AccountStatus, string> = {
    active: "正常",
    expired: "凭据过期",
    risk_paused: "风控暂停",
    disabled: "已停用",
  };

  return (
    <span className={`badge ${styles[status]}`}>
      <span
        className={`h-1.5 w-1.5 shrink-0 rounded-full ${
          status === "active"
            ? "bg-emerald-500"
            : status === "expired"
              ? "bg-amber-500"
              : status === "risk_paused"
                ? "bg-rose-500"
                : "bg-zinc-400"
        }`}
        aria-hidden="true"
      />
      {labels[status]}
    </span>
  );
}

export function ClaimStatusBadge({ status }: { status: ClaimStatus }) {
  if (status === "unsupported") return null;

  if (status === "unknown") {
    return (
      <span className="badge badge-neutral">
        状态待核实
      </span>
    );
  }

  if (status === "available") {
    return (
      <span className="badge bg-sky-500/12 text-sky-700 dark:bg-sky-500/15 dark:text-sky-300">
        今日待领
      </span>
    );
  }

  return <span className="badge badge-neutral">今日已领</span>;
}
