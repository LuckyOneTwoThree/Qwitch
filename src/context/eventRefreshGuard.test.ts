import { describe, expect, it } from "vitest";
import {
  EMPTY_GUARD,
  SELF_EVENT_WINDOW_MS,
  consumeSelfEvent,
  markSelfEvent,
} from "./eventRefreshGuard";

describe("事件刷新守卫", () => {
  it("命令登记后只吞掉紧随其后的那一次事件", () => {
    const marked = markSelfEvent(EMPTY_GUARD, 1_000);
    const first = consumeSelfEvent(marked, 1_080);
    expect(first.skip).toBe(true);
    // 第二次事件必须照常刷新（托盘切换网关等外部变更不能被吞）
    const second = consumeSelfEvent(first.next, 1_120);
    expect(second.skip).toBe(false);
    expect(second.next).toEqual(EMPTY_GUARD);
  });

  it("超出时间窗的滞留计数自动作废，不会永久吞事件", () => {
    const marked = markSelfEvent(EMPTY_GUARD, 1_000);
    const late = consumeSelfEvent(marked, 1_000 + SELF_EVENT_WINDOW_MS + 1);
    expect(late.skip).toBe(false);
    expect(late.next).toEqual(EMPTY_GUARD);
  });

  it("连续两次写命令会登记两次，各吞一次", () => {
    let guard = markSelfEvent(EMPTY_GUARD, 1_000);
    guard = markSelfEvent(guard, 1_010);
    const a = consumeSelfEvent(guard, 1_050);
    const b = consumeSelfEvent(a.next, 1_100);
    const c = consumeSelfEvent(b.next, 1_150);
    expect([a.skip, b.skip, c.skip]).toEqual([true, true, false]);
  });
});
