// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { PoolPrototypeProvider } from "../../context/usePoolPrototypeStore";
import { AgentBindingsView } from "./AgentBindingsView";

afterEach(cleanup);

describe("AgentBindingsView (Agent-centric tool/model & same-tool account scheduler)", () => {
  it("supports per-agent tool switching, batch model selection, default switch, primary/backup account switching, DSH drift repair, and Codex model selection", () => {
    render(
      <PoolPrototypeProvider>
        <AgentBindingsView />
      </PoolPrototypeProvider>
    );

    const agentList = screen.getByRole("complementary", { name: "目标 Agent 客户端列表" });
    const cardOrder = within(agentList)
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent);
    expect(cardOrder).toEqual(["DeepSeek Harness", "OpenCode", "Codex 桌面版", "Claude 桌面版"]);

    // 切换到 Claude 桌面版测试「全选当前」「清空本工具」「仅选推荐主力」与「设为默认」
    const claudeCard = screen.getByRole("region", { name: "Claude 桌面版 挂载卡片" });
    fireEvent.click(within(claudeCard).getByRole("button", { name: /Claude 桌面版/ }));
    const modelSection = screen.getByRole("region", { name: "WorkBuddy 订阅模型选择" });
    fireEvent.click(within(modelSection).getByRole("button", { name: "全选当前" }));
    expect(within(modelSection).getByText(/\/ 15 个模型/)).toBeTruthy();
    expect(within(modelSection).getByText("客户端下拉菜单实时预览", { exact: false })).toBeTruthy();

    // 切换到 Qoder CN 订阅工具并批量选中低倍率模型、设第一个为默认
    const toolTabs = screen.getByRole("tablist", { name: "订阅工具切换栏" });
    fireEvent.click(within(toolTabs).getByRole("tab", { name: /Qoder CN/ }));
    const qoderModelSection = screen.getByRole("region", { name: "Qoder CN 订阅模型选择" });
    fireEvent.click(within(qoderModelSection).getByRole("button", { name: "仅选免费/低倍率" }));
    const setDefaultBtns = within(qoderModelSection).getAllByRole("button", { name: "设为默认" });
    fireEvent.click(setDefaultBtns[0]);
    expect(within(qoderModelSection).getByText("默认模型")).toBeTruthy();

    // 搜索筛选 + 清空本工具
    const searchInput = within(qoderModelSection).getByLabelText("搜索订阅模型") as HTMLInputElement;
    fireEvent.change(searchInput, { target: { value: "qwen4" } });
    expect(within(qoderModelSection).getByText("Qwen4-Max")).toBeTruthy();
    fireEvent.click(within(qoderModelSection).getByLabelText("清除搜索"));
    fireEvent.click(within(qoderModelSection).getByRole("button", { name: "清空本工具" }));
    expect(within(qoderModelSection).getByRole("button", { name: "已勾选 (0)" })).toBeTruthy();

    // 切换到「账号调度」子分区测试同工具多账号调度：将 #2 备用账号一键「设为主号」及批量启停
    const subTabs = screen.getByRole("tablist", { name: "Agent 配置分区" });
    fireEvent.click(within(subTabs).getByRole("tab", { name: /账号调度/ }));
    fireEvent.click(within(screen.getByRole("tablist", { name: "订阅工具切换栏" })).getByRole("tab", { name: /WorkBuddy/ }));
    const schedSection = screen.getByRole("region", { name: "WorkBuddy 同工具账号调度方案" });
    const promoteBtns = within(schedSection).getAllByRole("button", { name: "设为主号" });
    expect(promoteBtns.length).toBeGreaterThan(0);
    fireEvent.click(promoteBtns[0]);
    fireEvent.click(within(schedSection).getByRole("button", { name: "仅启用主号" }));
    expect(within(schedSection).getByText(/1\/\d+ 启用/)).toBeTruthy();
    fireEvent.click(within(schedSection).getByRole("button", { name: "全部启用" }));

    // 验证「接入设置」子分区包含配置 Diff 与通用标准端点
    fireEvent.click(within(subTabs).getByRole("tab", { name: /接入设置/ }));
    expect(screen.getByText(/通用标准网关端点/)).toBeTruthy();

    // 切回「模型配置」选中 DeepSeek Harness，验证配置漂移告警与 Diff 修复弹窗
    fireEvent.click(within(subTabs).getByRole("tab", { name: /模型配置/ }));
    const dshCard = screen.getByRole("region", { name: "DeepSeek Harness 挂载卡片" });
    expect(within(dshCard).getByText("配置漂移")).toBeTruthy();
    fireEvent.click(within(dshCard).getByRole("button", { name: /查看差异并修复/ }));
    expect(screen.getByText("修复 DeepSeek Harness 配置漂移")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "确认修复漂移" }));
    expect(within(dshCard).getByText("已挂载并同步")).toBeTruthy();

    // 选中尚未挂载的 Codex 桌面版，直接勾选 WorkBuddy 推荐主力模型，状态进入待重启生效
    const codexCard = screen.getByRole("region", { name: "Codex 桌面版 挂载卡片" });
    fireEvent.click(within(codexCard).getByRole("button", { name: /Codex 桌面版/ }));
    const codexWbSection = screen.getByRole("region", { name: "WorkBuddy 订阅模型选择" });
    fireEvent.click(within(codexWbSection).getByRole("button", { name: "仅选推荐主力" }));
    expect(within(codexCard).getByText("待重启生效")).toBeTruthy();
  });
});
