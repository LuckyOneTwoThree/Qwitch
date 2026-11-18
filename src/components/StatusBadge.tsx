import type { AccountStatus, ClaimStatus } from "../types/qwitch";

export function AccountStatusBadge({ status }: { status: AccountStatus }) {
  const styles: Record<AccountStatus, string> = {
    active: "bg-emerald-500/10 text-emerald-700 dark:text-emerald-400 border-emerald-500/20",
    expired: "bg-amber-500/10 text-amber-700 dark:text-amber-400 border-amber-500/20",
    risk_paused: "bg-rose-500/10 text-rose-700 dark:text-rose-400 border-rose-500/20",
    disabled: "bg-zinc-500/10 text-zinc-600 dark:text-zinc-400 border-zinc-500/20",
  };

  const labels: Record<AccountStatus, string> = {
    active: "正常",
    expired: "凭据过期",
    risk_paused: "风控暂停",
    disabled: "已停用",
  };

  return (
    <span className={`inline-flex shrink-0 whitespace-nowrap items-center rounded-full border px-2 py-0.5 text-[11px] font-medium ${styles[status]}`}>
      {labels[status]}
    </span>
  );
}

export function ClaimStatusBadge({ status }: { status: ClaimStatus }) {
  if (status === "unsupported" || status === "unknown") return null;

  if (status === "available") {
    return (
      <span className="inline-flex shrink-0 whitespace-nowrap items-center rounded-full border border-sky-500/20 bg-sky-500/10 px-2 py-0.5 text-[11px] font-medium text-sky-700 dark:text-sky-400">
        今日待领
      </span>
    );
  }

  return (
    <span className="inline-flex shrink-0 whitespace-nowrap items-center rounded-full border border-zinc-200 bg-zinc-100 px-2 py-0.5 text-[11px] font-medium text-zinc-600 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-400">
      今日已领
    </span>
  );
}
