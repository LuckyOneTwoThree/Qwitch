// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useToast } from "./useToast";

describe("useToast", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows a toast and auto-dismisses after the tone duration", () => {
    const { result } = renderHook(() => useToast());
    act(() => result.current.showToast("hello", "info"));
    expect(result.current.toast?.message).toBe("hello");
    act(() => vi.advanceTimersByTime(3399));
    expect(result.current.toast).not.toBeNull();
    act(() => vi.advanceTimersByTime(1));
    expect(result.current.toast).toBeNull();
  });

  it("keeps error toasts longer than info toasts", () => {
    const { result } = renderHook(() => useToast());
    act(() => result.current.showToast("boom", "error"));
    act(() => vi.advanceTimersByTime(6499));
    expect(result.current.toast?.tone).toBe("error");
    act(() => vi.advanceTimersByTime(1));
    expect(result.current.toast).toBeNull();
  });

  it("a newer toast is not dismissed by the older timer", () => {
    const { result } = renderHook(() => useToast());
    act(() => result.current.showToast("first", "info"));
    act(() => vi.advanceTimersByTime(2000));
    act(() => result.current.showToast("second", "info"));
    act(() => vi.advanceTimersByTime(2000));
    expect(result.current.toast?.message).toBe("second");
  });

  it("dismissToast clears immediately", () => {
    const { result } = renderHook(() => useToast());
    act(() => result.current.showToast("x", "warning"));
    act(() => result.current.dismissToast());
    expect(result.current.toast).toBeNull();
  });
});
