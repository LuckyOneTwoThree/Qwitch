/**
 * 时间与「数据新鲜度」工具。
 *
 * 背景：Qwitch 的核心承诺是「额度水位可信」。此前账号卡片只显示 `HH:MM`
 * 的同步时刻，昨天 03:00 的快照与今天 03:00 的快照在界面上**完全无法区分**，
 * 用户会把陈旧额度误读成当前额度 —— 这是额度管理器最严重的信任事故。
 *
 * 本模块提供：
 * - CST（UTC+8）日界计算：签到按自然日重置，跨时区判断必须以 CST 为准；
 * - 相对时间表达：让「多久之前同步」一眼可读；
 * - 过期判定：快照不属于今日 CST 日时即为「过期快照」，需要显式提示。
 */

const CST_OFFSET_MS = 8 * 60 * 60 * 1000;
const MINUTE_MS = 60 * 1000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

/** 将时间戳归一到 CST 自然日，返回 `YYYY-MM-DD`。 */
export function toCstDayKey(timestampMs: number): string {
  const d = new Date(timestampMs + CST_OFFSET_MS);
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** 两个时间戳在 CST 自然日上相差的天数（a - b）。 */
export function cstDayDistance(aMs: number, bMs: number): number {
  const a = new Date(aMs + CST_OFFSET_MS);
  const b = new Date(bMs + CST_OFFSET_MS);
  const aDay = Date.UTC(a.getUTCFullYear(), a.getUTCMonth(), a.getUTCDate());
  const bDay = Date.UTC(b.getUTCFullYear(), b.getUTCMonth(), b.getUTCDate());
  return Math.round((aDay - bDay) / DAY_MS);
}

/** 判断 ISO 时间是否落在「今天」（CST 自然日）。 */
export function isSameCstDay(iso?: string, now: number = Date.now()): boolean {
  if (!iso) return false;
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return false;
  return toCstDayKey(ms) === toCstDayKey(now);
}

/**
 * 额度快照是否已过期。
 *
 * 判定标准：快照不属于今日 CST 自然日即视为过期。签到福利按自然日重置，
 * 因此昨天的额度水位对今天的决策没有参考价值，必须让用户看见「这是昨天的数」。
 */
export function isSnapshotStale(iso?: string, now: number = Date.now()): boolean {
  if (!iso) return false;
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return false;
  // 未来时间戳（系统时钟回拨）不视为过期
  if (ms > now) return false;
  return toCstDayKey(ms) !== toCstDayKey(now);
}

/**
 * 相对时间表达（用于「最近同步」）。
 *
 * - 1 分钟内 → `刚刚`
 * - 同一天内 → `N 分钟前` / `N 小时前`
 * - 昨天 → `昨天 HH:MM`
 * - 更早 → `M月D日`
 *
 * 不返回裸 `HH:MM`，因为裸时刻无法区分「今天」与「若干天前」。
 */
export function formatRelativeSyncTime(iso?: string, now: number = Date.now()): string {
  if (!iso) return "尚未同步";
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return "已同步";

  const diff = now - ms;
  // 未来时间戳：视为刚刚同步，避免出现「-3 分钟前」这类荒谬表达
  if (diff < MINUTE_MS) return "刚刚";

  const minutes = Math.floor(diff / MINUTE_MS);
  if (minutes < 60 && isSameCstDay(iso, now)) return `${minutes} 分钟前`;

  const hours = Math.floor(diff / HOUR_MS);
  if (hours < 24 && isSameCstDay(iso, now)) return `${hours} 小时前`;

  const d = new Date(ms);
  const hhmm = d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  if (cstDayDistance(ms, now) === -1) return `昨天 ${hhmm}`;
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}
