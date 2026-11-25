import { describe, expect, it } from "vitest";
import {
  cstDayDistance,
  formatRelativeSyncTime,
  isSameCstDay,
  isSnapshotStale,
  toCstDayKey,
} from "./time";

// 固定「现在」：2026-03-15 10:00:00 CST === 2026-03-15T02:00:00Z
const NOW = Date.parse("2026-03-15T02:00:00.000Z");

describe("domain/time · CST 日界", () => {
  it("toCstDayKey 按 CST 自然日归一（UTC 16:00 之后即进入次日）", () => {
    expect(toCstDayKey(Date.parse("2026-03-14T15:59:59.000Z"))).toBe("2026-03-14");
    expect(toCstDayKey(Date.parse("2026-03-14T16:00:00.000Z"))).toBe("2026-03-15");
  });

  it("isSameCstDay 判定今日/昨日/非法输入", () => {
    expect(isSameCstDay("2026-03-15T02:00:00.000Z", NOW)).toBe(true);
    expect(isSameCstDay("2026-03-14T02:00:00.000Z", NOW)).toBe(false);
    expect(isSameCstDay(undefined, NOW)).toBe(false);
    expect(isSameCstDay("not-a-date", NOW)).toBe(false);
  });

  it("cstDayDistance 跨 CST 边界计算整日差", () => {
    expect(cstDayDistance(Date.parse("2026-03-15T02:00:00.000Z"), NOW)).toBe(0);
    expect(cstDayDistance(Date.parse("2026-03-14T02:00:00.000Z"), NOW)).toBe(-1);
    expect(cstDayDistance(Date.parse("2026-03-13T02:00:00.000Z"), NOW)).toBe(-2);
  });
});

describe("domain/time · 快照新鲜度", () => {
  it("今日快照不算过期，跨日快照视为过期", () => {
    expect(isSnapshotStale("2026-03-15T01:30:00.000Z", NOW)).toBe(false);
    expect(isSnapshotStale("2026-03-14T01:30:00.000Z", NOW)).toBe(true);
  });

  it("缺失或非法时间戳不判定为过期", () => {
    expect(isSnapshotStale(undefined, NOW)).toBe(false);
    expect(isSnapshotStale("not-a-date", NOW)).toBe(false);
  });

  it("系统时钟回拨导致的未来时间戳不判为过期", () => {
    expect(isSnapshotStale("2026-03-15T09:00:00.000Z", NOW)).toBe(false);
  });
});

describe("domain/time · 相对时间表达", () => {
  it("1 分钟内显示「刚刚」", () => {
    expect(formatRelativeSyncTime("2026-03-15T01:59:30.000Z", NOW)).toBe("刚刚");
  });

  it("同一天内显示分钟/小时前", () => {
    expect(formatRelativeSyncTime("2026-03-15T01:45:00.000Z", NOW)).toBe("15 分钟前");
    expect(formatRelativeSyncTime("2026-03-14T22:00:00.000Z", NOW)).toBe("4 小时前");
  });

  it("昨天显示「昨天 HH:MM」，更早显示日期——绝不返回无法判断日期的裸时刻", () => {
    const yesterday = formatRelativeSyncTime("2026-03-14T01:30:00.000Z", NOW);
    expect(yesterday.startsWith("昨天")).toBe(true);
    expect(yesterday).not.toMatch(/^\d{2}:\d{2}$/);

    expect(formatRelativeSyncTime("2026-03-10T01:30:00.000Z", NOW)).toMatch(/^\d+月\d+日$/);
  });

  it("缺失时间戳显示「尚未同步」，非法时间戳显示「已同步」", () => {
    expect(formatRelativeSyncTime(undefined, NOW)).toBe("尚未同步");
    expect(formatRelativeSyncTime("not-a-date", NOW)).toBe("已同步");
  });

  it("未来时间戳按「刚刚」处理，不出现负数表达", () => {
    expect(formatRelativeSyncTime("2026-03-15T09:00:00.000Z", NOW)).toBe("刚刚");
  });
});
