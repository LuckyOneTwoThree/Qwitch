/**
 * 抑制「命令自己触发的重复全量刷新」。
 *
 * 每次写命令返回时后端都会 emit 一次 `qwitch://pool-workspace-updated`，
 * 前端随后又构建一遍 200KB 全量快照——这条刷新是本命令自身造成的，纯属重复。
 * 但其它来源的事件（托盘切换网关、账号导入完成等）必须照常刷新，因此：
 *
 * - 只登记「待跳过」的次数，不按时间粗暴屏蔽；
 * - 同时给一个时间上限，避免事件丢失时计数滞留、误吞后续真实变更。
 */
export type SelfEventGuard = { count: number; until: number };

export const SELF_EVENT_WINDOW_MS = 600;

export const EMPTY_GUARD: SelfEventGuard = { count: 0, until: 0 };

/** 命令回填快照后调用：登记一次「下一个事件是我自己发的」。 */
export function markSelfEvent(guard: SelfEventGuard, now: number): SelfEventGuard {
  return { count: guard.count + 1, until: now + SELF_EVENT_WINDOW_MS };
}

/**
 * 收到事件时调用：返回是否应当跳过本次刷新，以及下一个 guard 状态。
 * 超过时间上限后计数作废，避免永久吞事件。
 */
export function consumeSelfEvent(
  guard: SelfEventGuard,
  now: number
): { skip: boolean; next: SelfEventGuard } {
  if (guard.count > 0 && now <= guard.until) {
    return { skip: true, next: { count: guard.count - 1, until: guard.until } };
  }
  return { skip: false, next: EMPTY_GUARD };
}
