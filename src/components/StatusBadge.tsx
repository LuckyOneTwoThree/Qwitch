import type { AccountStatus, ClaimStatus } from "../types/qwitch";

export function AccountStatusBadge({ status }: { status: AccountStatus }) {
  const styles: Record<AccountStatus, string> = {
    active: "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300 border-emerald-500/25",
    expired: "bg-amber-500/12 text-amber-800 dark:text-amber-300 border-amber-500/25",
    risk_paused: "bg-rose-500/12 text-rose-800 dark:text-rose-300 border-rose-500/25",
    disabled: "bg-zinc-500/12 text-zinc-700 dark:text-zinc-300 border-zinc-500/25",
  };

  const labels: Record<AccountStatus, string> = {
    active: "正常",
    expired: "凭据过期",
    risk_paused: "风控暂停",
    disabled: "已停用",
  };

  return (
    <span
      className={`inline-flex shrink-0 whitespace-nowrap items-center rounded-full border px-2 py-0.5 text-[11px] font-medium ${styles[status]}`}
    >
      {labels[status]}
    </span>
  );
}

export function ClaimStatusBadge({ status }: { status: ClaimStatus }) {
  if (status === "unsupported") return null;

  if (status === "unknown") {
    return (
      <span className="inline-flex shrink-0 whitespace-nowrap items-center rounded-full border border-zinc-300 bg-zinc-100 px-2 py-0.5 text-[11px] font-medium text-zinc-700 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
        状态待核实
      </span>
    );
  }

  if (status === "available") {
    return (
      <span className="inline-flex shrink-0 whitespace-nowrap items-center rounded-full border border-sky-500/25 bg-sky-500/12 px-2 py-0.5 text-[11px] font-medium text-sky-800 dark:text-sky-300">
        今日待领
      </span>
    );
  }

  return (
    <span className="inline-flex shrink-0 whitespace-nowrap items-center rounded-full border border-zinc-300 bg-zinc-100 px-2 py-0.5 text-[11px] font-medium text-zinc-700 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
      今日已领
    </span>
  );
}
