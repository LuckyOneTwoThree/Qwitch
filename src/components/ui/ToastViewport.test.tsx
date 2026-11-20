// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ToastState } from "../../types/qwitch";

let currentToast: ToastState | null = null;
vi.mock("../../context/QwitchContext", () => ({
  useQwitch: () => ({ toast: currentToast, dismissToast: vi.fn() }),
}));

import { ToastViewport } from "./ToastViewport";

afterEach(cleanup);

describe("ToastViewport", () => {
  it("uses role=alert for errors", () => {
    currentToast = { id: 1, message: "bad", tone: "error" };
    render(<ToastViewport />);
    expect(screen.getByRole("alert").textContent).toContain("bad");
  });

  it("uses role=status for non-errors", () => {
    currentToast = { id: 2, message: "ok", tone: "info" };
    render(<ToastViewport />);
    expect(screen.getByRole("status").textContent).toContain("ok");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("renders nothing without a toast", () => {
    currentToast = null;
    const { container } = render(<ToastViewport />);
    expect(container.firstChild).toBeNull();
  });
});
