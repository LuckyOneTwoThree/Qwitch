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

  it("keeps live region mounted without rendering a toast card when toast is null", () => {
    currentToast = null;
    render(<ToastViewport />);
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("portals default toast viewport to document.body at z-[120] above modals", () => {
    currentToast = { id: 3, message: "above modal", tone: "warning" };
    const { container } = render(<ToastViewport />);
    expect(container.firstChild).toBeNull();
    const region = document.body.querySelector('[aria-live="polite"]') as HTMLElement | null;
    expect(region).toBeTruthy();
    expect(region?.className).toContain("z-[120]");
  });
});
