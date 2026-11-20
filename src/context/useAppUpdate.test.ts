// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

import { useAppUpdate } from "./useAppUpdate";

const info = { updateAvailable: true, latestVersion: "9.9.9" } as never;

describe("useAppUpdate", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("returns null without invoking outside Tauri", async () => {
    const { result } = renderHook(() => useAppUpdate(false));
    let r: unknown;
    await act(async () => {
      r = await result.current.checkAppUpdate();
    });
    expect(r).toBeNull();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("caches within 10 minutes unless forced", async () => {
    invokeMock.mockResolvedValue(info);
    const { result } = renderHook(() => useAppUpdate(true));
    await act(async () => {
      await result.current.checkAppUpdate();
    });
    await act(async () => {
      await result.current.checkAppUpdate();
    });
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await act(async () => {
      await result.current.checkAppUpdate({ force: true });
    });
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(result.current.updateInfo).toBe(info);
  });

  it("merges concurrent checks into one invoke", async () => {
    let resolve!: (v: unknown) => void;
    invokeMock.mockReturnValue(new Promise((r) => (resolve = r)));
    const { result } = renderHook(() => useAppUpdate(true));
    let p1!: Promise<unknown>, p2!: Promise<unknown>;
    act(() => {
      p1 = result.current.checkAppUpdate();
      p2 = result.current.checkAppUpdate();
    });
    await act(async () => {
      resolve(info);
      await Promise.all([p1, p2]);
    });
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it("surfaces errors unless silent", async () => {
    invokeMock.mockImplementation(() => Promise.reject("network down"));
    const { result } = renderHook(() => useAppUpdate(true));
    await act(async () => {
      await result.current.checkAppUpdate({ silent: true });
    });
    expect(result.current.updateError).toBeNull();
    await act(async () => {
      await result.current.checkAppUpdate();
    });
    expect(result.current.updateError).toBe("network down");
    expect(result.current.updateChecking).toBe(false);
  });
});
