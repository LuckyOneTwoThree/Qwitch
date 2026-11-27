import type { AccountSummary, AdapterManifest, ToolId } from "../../types/qwitch";
import { getEffectiveClaimStatus, isAccountMissingLocalSnapshot } from "../../domain/selectors";
import { getToolBrandMeta } from "../../domain/toolRegistry";

/**
 * 台账（Ledger）纯逻辑层。
 *
 * 之所以把「额度视图 / 风险分带 / 排序」抽出来，是因为 Qwitch 的额度模型高度异构：
 * - ratio   ：服务端给了可信总量（380 / 600 次）
 * - percent ：只有百分比桶（total 恒为 100）
 * - balance ：只有剩余、没有可信总量（LobsterAI 跨池积分等），不能算百分比
 * - claim   ：水位由签到状态推导（Qoder daily_credits），不是真实百分比
 * - empty   ：还没查过额度
 *
 * 这五类混在同一个「水位」列里，任何单一百分比排序都会失真，
 * 所以排序用的不是 pct，而是「风险分带」。
 */

export type LedgerQuotaKind = "ratio" | "percent" | "balance" | "claim" | "empty";

export interface LedgerQuotaView {
  kind: LedgerQuotaKind;
  /** 0–100，仅 ratio / percent 有值 */
  pct: number | null;
  remaining: number | null;
  total: number | null;
  unit: string;
  /** 主桶之外的额度桶数量（副桶用 +N 角标提示） */
  extraBuckets: number;
  bucketLabel: string;
}

export const EMPTY_QUOTA_VIEW: LedgerQuotaView = {
  kind: "empty",
  pct: null,
  remaining: null,
  total: null,
  unit: "",
  extraBuckets: 0,
  bucketLabel: "",
};

export function formatQuotaNumber(value: number): string {
  const rounded = Math.round(value * 100) / 100;
  return rounded.toLocaleString("zh-CN", { maximumFractionDigits: 2 });
}

/**
 * 主桶 → 可渲染视图。判定顺序与 `AccountCard` 保持一致，
 * 避免同一个账号在卡片与台账里出现两种读数。
 */
export function toQuotaView(account: AccountSummary): LedgerQuotaView {
  const buckets = account.latestQuota?.buckets ?? [];
  const primary = buckets[0];
  if (!primary) return EMPTY_QUOTA_VIEW;

  const extraBuckets = Math.max(0, buckets.length - 1);
  const base = {
    remaining: primary.remaining,
    total: primary.total,
    unit: primary.unit,
    extraBuckets,
    bucketLabel: primary.label,
  };

  // Qoder 的每日额度桶水位由签到状态推导，不能当作真实百分比参与排序
  if (account.toolId === "qoder_cn" && primary.key === "daily_credits") {
    return { ...base, kind: "claim", pct: null };
  }

  if (primary.unit === "%") {
    const pct = clampPct(primary.remaining);
    return { ...base, kind: "percent", pct, total: 100 };
  }

  // hasTotal === false：服务端没给可信总量，画进度条就是假读数
  if (primary.hasTotal === false) {
    return { ...base, kind: "balance", pct: null };
  }

  if (primary.total > 0) {
    const pct = clampPct(Math.round((primary.remaining / primary.total) * 100));
    return { ...base, kind: "ratio", pct };
  }

  return { ...base, kind: "balance", pct: null };
}

function clampPct(n: number): number {
  if (!Number.isFinite(n)) return 0;
  return Math.max(0, Math.min(100, Math.round(n)));
}

/** 水位色阶：与卡片保持同一阈值（≤15 危急 / ≤35 偏低 / 其余健康） */
export function waterTone(pct: number | null): "critical" | "low" | "ok" | "none" {
  if (pct === null) return "none";
  if (pct <= 15) return "critical";
  if (pct <= 35) return "low";
  return "ok";
}

export const LEDGER_BAND = {
  blocked: 0,
  critical: 1,
  low: 2,
  healthy: 3,
  unknown: 4,
  disabled: 5,
} as const;

export type LedgerBand = (typeof LEDGER_BAND)[keyof typeof LEDGER_BAND];

export const LEDGER_BAND_LABEL: Record<LedgerBand, string> = {
  0: "阻断",
  1: "危急",
  2: "偏低",
  3: "健康",
  4: "无水位",
  5: "已停用",
};

/**
 * 风险分带。
 *
 * 「按水位排序」这个名字会误导：它排的其实是「下一个会卡住我的是谁」。
 * 分带之间不可比（没有水位的账号谈不上多少），带内才比较数值。
 */
export function riskBand(account: AccountSummary): LedgerBand {
  if (account.status === "disabled") return LEDGER_BAND.disabled;
  if (
    account.status === "expired" ||
    account.status === "risk_paused" ||
    account.hasApiToken === false
  ) {
    return LEDGER_BAND.blocked;
  }

  const q = toQuotaView(account);
  if (q.kind === "ratio" || q.kind === "percent") {
    const pct = q.pct ?? 0;
    if (pct <= 15) return LEDGER_BAND.critical;
    if (pct <= 35) return LEDGER_BAND.low;
    return LEDGER_BAND.healthy;
  }

  // 纯余额型 / 签到型 / 未查额：没有可比的水位
  return LEDGER_BAND.unknown;
}

export type LedgerSortKey = "risk" | "quota-asc" | "quota-desc" | "recent" | "name";

export const LEDGER_SORT_LABEL: Record<LedgerSortKey, string> = {
  risk: "风险优先",
  "quota-asc": "水位从低到高",
  "quota-desc": "水位从高到低",
  recent: "最近同步",
  name: "按名称",
};

function timeValue(iso?: string): number {
  if (!iso) return 0;
  const t = Date.parse(iso);
  return Number.isNaN(t) ? 0 : t;
}

/**
 * 排序。所有比较器都以 id 兜底，保证稳定排序，
 * 否则 React 重渲染时行会跳动，鼠标点击容易点错行。
 */
export function sortLedgerAccounts(
  accounts: AccountSummary[],
  sort: LedgerSortKey
): AccountSummary[] {
  const decorated = accounts.map((acc) => ({
    acc,
    band: riskBand(acc),
    quota: toQuotaView(acc),
    syncedAt: timeValue(acc.latestQuota?.fetchedAt ?? acc.updatedAt),
  }));

  const byPct = (a: (typeof decorated)[number], b: (typeof decorated)[number]) => {
    const pa = a.quota.pct;
    const pb = b.quota.pct;
    if (pa === null && pb === null) return 0;
    if (pa === null) return 1;
    if (pb === null) return -1;
    return pa - pb;
  };

  decorated.sort((x, y) => {
    switch (sort) {
      case "quota-asc":
        return byPct(x, y) || x.band - y.band || x.acc.id.localeCompare(y.acc.id);
      case "quota-desc":
        return -byPct(x, y) || x.band - y.band || x.acc.id.localeCompare(y.acc.id);
      case "recent":
        return y.syncedAt - x.syncedAt || x.band - y.band || x.acc.id.localeCompare(y.acc.id);
      case "name":
        return (
          x.acc.label.localeCompare(y.acc.label, "zh-CN") || x.acc.id.localeCompare(y.acc.id)
        );
      case "risk":
      default:
        return (
          x.band - y.band ||
          byPct(x, y) ||
          // 同水位下，更久没同步的排前面（更可能读到陈旧数据）
          x.syncedAt - y.syncedAt ||
          x.acc.id.localeCompare(y.acc.id)
        );
    }
  });

  return decorated.map((d) => d.acc);
}

export interface LedgerStatusView {
  label: string;
  tone: "ok" | "warn" | "danger" | "muted";
  /** 本机客户端当前登录的是这个账号 */
  mounted: boolean;
}

/**
 * 状态列只表达「可用性」这一条轴。
 * 本机挂载（isActiveOnClient）与签到状态（claimStatus）是另外两条正交轴，
 * 分别用名称前的圆点、行内领取按钮的启停态表达，不再挤进同一列。
 */
export function toStatusView(
  account: AccountSummary,
  adapters?: AdapterManifest[]
): LedgerStatusView {
  const mounted = account.isActiveOnClient;
  if (account.status === "expired") return { label: "凭据过期", tone: "warn", mounted };
  if (account.status === "risk_paused") return { label: "风控暂停", tone: "danger", mounted };
  if (account.status === "disabled") return { label: "已停用", tone: "muted", mounted };
  if (account.hasApiToken === false) return { label: "待授权", tone: "warn", mounted };
  if (isAccountMissingLocalSnapshot(account, adapters)) {
    return { label: "待关联", tone: "warn", mounted };
  }
  return { label: "正常", tone: "ok", mounted };
}

/** 行内是否应显示领取按钮（可领 / 待核实） */
export function claimAvailability(
  account: AccountSummary,
  adapters?: AdapterManifest[]
): "ready" | "verify" | "done" | "off" {
  if (account.status !== "active" || account.hasApiToken === false) return "off";
  const adapter = adapters?.find((a) => a.id === account.toolId);
  if (!adapter?.capabilities.supportsDailyClaim) return "off";
  if (adapter.capabilities.supportsClientSwitch && account.hasLocalSnapshot === false) {
    return "off";
  }
  const claim = getEffectiveClaimStatus(account);
  if (claim === "available") return "ready";
  if (claim === "unknown") return "verify";
  if (claim === "claimed") return "done";
  return "off";
}

export function toolNameOf(toolId: ToolId, adapters?: AdapterManifest[]): string {
  return getToolBrandMeta(toolId, adapters).name;
}
