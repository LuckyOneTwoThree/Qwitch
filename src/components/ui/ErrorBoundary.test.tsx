// @vitest-environment jsdom
import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ErrorBoundary } from "./ErrorBoundary";

afterEach(cleanup);

describe("ErrorBoundary", () => {
  it("渲染正常子节点", () => {
    render(
      <ErrorBoundary>
        <div>内容正常渲染</div>
      </ErrorBoundary>
    );
    expect(screen.getByText("内容正常渲染")).toBeTruthy();
  });

  it("子组件抛错时展示兜底界面", () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    function Boom(): JSX.Element {
      throw new Error("boom-边框异常");
    }
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>
    );
    expect(screen.getByText("Qwitch 界面遇到异常")).toBeTruthy();
    expect(screen.getByText(/boom-边框异常/)).toBeTruthy();
    expect(screen.getByText("重新加载")).toBeTruthy();
    errSpy.mockRestore();
  });
});
