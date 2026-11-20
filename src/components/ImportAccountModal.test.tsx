// @vitest-environment jsdom
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MOCK_ADAPTERS } from "../data/mockStore";

const h = vi.hoisted(() => ({ useQwitch: vi.fn() }));
vi.mock("../context/QwitchContext", () => ({ useQwitch: h.useQwitch }));

import { ImportAccountModal } from "./ImportAccountModal";

afterEach(cleanup);

let ctx: Record<string, unknown>;

beforeEach(() => {
  ctx = {
    adapters: MOCK_ADAPTERS,
    importAccount: vi.fn(),
    pickFolder: vi.fn().mockResolvedValue(null),
    pickFile: vi.fn().mockResolvedValue(null),
    inspectProfileDir: vi.fn(),
    showToast: vi.fn(),
  };
  h.useQwitch.mockReturnValue(ctx);
});

describe("ImportAccountModal", () => {
  it("打开时展示标题与导入方式", () => {
    render(<ImportAccountModal open onClose={vi.fn()} />);
    expect(screen.getByText("添加 / 导入账号")).toBeTruthy();
    expect(screen.getByText("导入方式")).toBeTruthy();
    expect(screen.getByText("本机导入")).toBeTruthy();
  });

  it("点击关闭按钮触发 onClose", () => {
    const onClose = vi.fn();
    render(<ImportAccountModal open onClose={onClose} />);
    fireEvent.click(screen.getByLabelText("关闭导入弹窗"));
    expect(onClose).toHaveBeenCalled();
  });
});
