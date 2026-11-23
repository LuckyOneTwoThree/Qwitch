// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PoolPrototypeProvider } from "../../context/usePoolPrototypeStore";
import { AgentBindingsView } from "./AgentBindingsView";

afterEach(cleanup);

describe("AgentBindingsView v2", () => {
  it("supports M:N pool mounting, live client menu default switch, DSH drift Diff repair, and Codex mount", () => {
    render(
      <PoolPrototypeProvider>
        <AgentBindingsView onNavigatePools={vi.fn()} />
      </PoolPrototypeProvider>
    );

    expect(screen.getByText("Claude 桌面版")).toBeTruthy();
    expect(screen.getByText("OpenCode")).toBeTruthy();
    expect(screen.getByText("DeepSeek Harness")).toBeTruthy();
    expect(screen.getByText("Codex 桌面版")).toBeTruthy();

    // Claude 桌面版预置挂载了 2 个号池，检查客户端模型列表实时预览与 ★ 默认切换
    const claudeCard = screen.getByRole("region", { name: "Claude 桌面版 挂载卡片" });
    expect(within(claudeCard).getByText(/共挂 2 池/)).toBeTruthy();
    const setDefaultBtns = within(claudeCard).getAllByRole("button", { name: "设为默认" });
    expect(setDefaultBtns.length).toBeGreaterThan(0);
    fireEvent.click(setDefaultBtns[0]);
    expect(within(claudeCard).getByText("默认模型")).toBeTruthy();

    // 验证 DSH 漂移告警并打开 Diff 修复弹窗
    const dshCard = screen.getByRole("region", { name: "DeepSeek Harness 挂载卡片" });
    expect(within(dshCard).getByText("配置漂移")).toBeTruthy();

    fireEvent.click(within(dshCard).getByRole("button", { name: /查看差异并修复/ }));
    expect(screen.getByText("修复 DeepSeek Harness 配置漂移")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "确认修复漂移" }));

    expect(within(dshCard).getByText("已挂载并同步")).toBeTruthy();

    // 验证 Codex 桌面版点击多选池胶囊直接追加挂载
    const codexCard = screen.getByRole("region", { name: "Codex 桌面版 挂载卡片" });
    fireEvent.click(
      within(codexCard).getByRole("button", { name: "Codex 桌面版 挂载号池 主力全栈 Coding 池" })
    );
    expect(within(codexCard).getByText("待重启生效")).toBeTruthy();
  });
});
