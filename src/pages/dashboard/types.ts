import type { ActionLogItem, ToolId } from "../../types/qwitch";

export type NavSection =
  | "all"
  | "pools"
  | "agents"
  | "routing"
  | "logs"
  | "settings"
  | ToolId;
/**
 * 账号主视图的三种排布：
 * - ledger  ：水位台账（默认，行 × 列，可排序可多选）
 * - grouped ：按工具分组的卡片墙（保留给需要富信息的场景）
 * - flat    ：不分组的卡片平铺
 */
export type AllViewMode = "ledger" | "grouped" | "flat";
export type LogActionFilter = "all" | ActionLogItem["actionType"];
export type LogResultFilter = "all" | ActionLogItem["resultStatus"];

export interface ConfirmLogAction {
  label: string;
  detail: string;
  danger?: boolean;
  run: () => Promise<boolean | void>;
}

export interface LogDayGroup {
  day: string;
  items: ActionLogItem[];
}

export const ACTION_TYPE_META: Record<
  ActionLogItem["actionType"],
  { label: string; style: string }
> = {
  claim: {
    label: "每日签到",
    style: "bg-sky-500/12 text-sky-800 dark:text-sky-300",
  },
  query: {
    label: "额度查询",
    style: "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
  },
  switch: {
    label: "桌面切号",
    style: "bg-indigo-500/12 text-indigo-800 dark:text-indigo-300",
  },
  import: {
    label: "账号管理",
    style: "bg-emerald-500/12 text-emerald-800 dark:text-emerald-300",
  },
  keepalive: {
    label: "凭据保活",
    style: "bg-teal-500/12 text-teal-800 dark:text-teal-300",
  },
};

export const formatLogDayLabel = (iso: string) => {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "未知日期";
  const now = new Date();
  const toKey = (d: Date) =>
    `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  const key = toKey(date);
  if (key === toKey(now)) return "今天";
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (key === toKey(yesterday)) return "昨天";
  return key;
};
