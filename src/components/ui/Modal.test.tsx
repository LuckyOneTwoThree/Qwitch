// @vitest-environment jsdom
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Modal } from "./Modal";

/**
 * `Modal` 的焦点陷阱是全仓库唯一手写的复杂交互逻辑（Tab 循环、Escape、
 * 焦点归还、body 滚动锁定），此前完全没有测试覆盖。
 */

afterEach(() => {
  cleanup();
  document.body.style.overflow = "";
});

const waitFrame = () =>
  new Promise<void>((resolve) => {
    // jsdom 的 requestAnimationFrame 由 setTimeout 模拟
    setTimeout(resolve, 30);
  });

function renderModal(props: Partial<React.ComponentProps<typeof Modal>> = {}) {
  const onClose = vi.fn();
  const utils = render(
    <Modal open onClose={onClose} labelledBy="modal-title" {...props}>
      <h2 id="modal-title">标题</h2>
      <button type="button">第一个</button>
      <button type="button">第二个</button>
    </Modal>
  );
  return { ...utils, onClose };
}

describe("Modal 焦点陷阱", () => {
  it("打开时聚焦第一个可聚焦元素", async () => {
    renderModal();
    // 实现通过 requestAnimationFrame 延后聚焦
    await waitFrame();
    expect(document.activeElement?.textContent).toBe("第一个");
  });

  it("Tab 在最后一个元素上回到第一个（正向循环）", () => {
    renderModal();
    const buttons = screen.getAllByRole("button");
    const last = buttons[buttons.length - 1];
    last.focus();

    fireEvent.keyDown(document, { key: "Tab" });

    expect(document.activeElement).toBe(buttons[0]);
  });

  it("Shift+Tab 在第一个元素上跳到最后一个（反向循环）", () => {
    renderModal();
    const buttons = screen.getAllByRole("button");
    const first = buttons[0];
    first.focus();

    fireEvent.keyDown(document, { key: "Tab", shiftKey: true });

    expect(document.activeElement).toBe(buttons[buttons.length - 1]);
  });

  it("Escape 触发 onClose", () => {
    const { onClose } = renderModal();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("disableClose 时 Escape 不触发 onClose", () => {
    const { onClose } = renderModal({ disableClose: true });
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("打开时锁定 body 滚动，关闭后恢复", () => {
    const { unmount } = renderModal();
    expect(document.body.style.overflow).toBe("hidden");
    unmount();
    expect(document.body.style.overflow).not.toBe("hidden");
  });

  it("关闭后焦点归还到打开前的元素", async () => {
    const trigger = document.createElement("button");
    trigger.textContent = "触发按钮";
    document.body.appendChild(trigger);
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    const { unmount } = renderModal();
    await waitFrame();
    // 打开后焦点已移入 Modal
    expect(document.activeElement).not.toBe(trigger);

    unmount();
    // 卸载清理阶段把焦点归还给打开前的元素
    expect(document.activeElement).toBe(trigger);

    trigger.remove();
  });

  it("open=false 时不渲染任何内容且不抢焦点", () => {
    render(
      <Modal open={false} onClose={vi.fn()} labelledBy="modal-title">
        <h2 id="modal-title">标题</h2>
        <button type="button">按钮</button>
      </Modal>
    );
    expect(screen.queryByText("标题")).toBeNull();
    expect(document.body.style.overflow).not.toBe("hidden");
  });

  it("叠加多个 Modal 时 Escape 仅关闭最顶层弹窗", () => {
    const onCloseOuter = vi.fn();
    const onCloseInner = vi.fn();
    render(
      <>
        <Modal open onClose={onCloseOuter} labelledBy="outer-title">
          <h2 id="outer-title">外层弹窗</h2>
          <button type="button">外层按钮</button>
        </Modal>
        <Modal open onClose={onCloseInner} labelledBy="inner-title" flush>
          <h2 id="inner-title">内层弹窗</h2>
          <button type="button">内层按钮</button>
        </Modal>
      </>
    );

    fireEvent.keyDown(document, { key: "Escape" });
    expect(onCloseInner).toHaveBeenCalledTimes(1);
    expect(onCloseOuter).not.toHaveBeenCalled();
  });
});
