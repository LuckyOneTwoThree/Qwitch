// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { WindowsWindowControls } from "./WindowsWindowControls";

describe("WindowsWindowControls", () => {
  it("renders minimize, maximize, and close buttons with accessible labels", () => {
    render(<WindowsWindowControls />);
    expect(screen.getByRole("button", { name: "最小化窗口" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "最大化窗口" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "关闭窗口" })).toBeTruthy();
  });
});
