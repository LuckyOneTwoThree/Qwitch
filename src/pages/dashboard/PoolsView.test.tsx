// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PoolPrototypeProvider } from "../../context/usePoolPrototypeStore";
import { PoolsView } from "./PoolsView";
import { CreatePoolModal } from "../../components/pool/CreatePoolModal";
import { PoolSimulationModal } from "../../components/pool/PoolSimulationModal";
import { useState } from "react";

afterEach(cleanup);

function PoolsHarness({
  onNavigateAgents = vi.fn(),
}: {
  onNavigateAgents?: () => void;
}) {
  const [createOpen, setCreateOpen] = useState(false);
  const [simOpen, setSimOpen] = useState(false);
  const [simPoolId, setSimPoolId] = useState<string | undefined>(undefined);
  const [simModelId, setSimModelId] = useState<string | undefined>(undefined);

  return (
    <PoolPrototypeProvider>
      <PoolsView
        onOpenCreatePool={() => setCreateOpen(true)}
        onOpenSimulation={(id, model) => {
          setSimPoolId(id);
          setSimModelId(model);
          setSimOpen(true);
        }}
        onNavigateAgents={onNavigateAgents}
      />
      <CreatePoolModal open={createOpen} onClose={() => setCreateOpen(false)} />
      <PoolSimulationModal
        open={simOpen}
        defaultPoolId={simPoolId}
        defaultModelId={simModelId}
        onClose={() => setSimOpen(false)}
      />
    </PoolPrototypeProvider>
  );
}

describe("PoolsView v2", () => {
  it("renders tiered models & accounts tab, switches 3 tabs, and supports continuous 429 simulation", () => {
    render(<PoolsHarness />);

    expect(screen.getAllByText(/主力全栈 Coding 池/).length).toBeGreaterThan(0);
    expect(screen.getByText(/一、对外开放的模型列表/)).toBeTruthy();
    expect(screen.getByText(/二、池内账号底座（T1 主力 → T2 备选 → T3 付费兜底/)).toBeTruthy();

    // 切换到「策略与护栏」与「已挂载 Agent」分页
    fireEvent.click(screen.getByRole("tab", { name: /策略与护栏/ }));
    expect(screen.getByText(/异构混编口径护栏/)).toBeTruthy();

    fireEvent.click(screen.getByRole("tab", { name: /已挂载 Agent/ }));
    expect(screen.getByText(/挂载了「主力全栈 Coding 池」的消费端 Agent/)).toBeTruthy();
    expect(screen.getAllByText(/Claude 桌面版/).length).toBeGreaterThan(1);

    // 切回默认分页并切换到「深度推理与长文池」
    fireEvent.click(screen.getByRole("tab", { name: /模型与账号轨道/ }));
    fireEvent.click(screen.getByText("深度推理与长文池"));
    expect(screen.getByText(/豆包工作 · 主号/)).toBeTruthy();

    // 切回具有同模型多账号冗余的「主力全栈 Coding 池」，测试 429 切换与同会话连续打流
    fireEvent.click(screen.getByText("主力全栈 Coding 池"));
    fireEvent.click(screen.getByRole("button", { name: /演练切号/ }));
    expect(screen.getByText("网关路由连通性测试")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /发送测试请求/ }));
    expect(screen.getByText(/无感切换接管成功/)).toBeTruthy();

    // 同会话续发一条（验证 Prompt Cache 亲和跳过冷却号）
    fireEvent.click(screen.getByRole("button", { name: /继续发下一条/ }));
    expect(screen.getByText(/Prompt Cache 亲和命中/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.getByText(/429 冷却/)).toBeTruthy();
  });

  it("opens create pool modal with multi-model selection and creates a new pool", () => {
    render(<PoolsHarness />);

    fireEvent.click(screen.getByRole("button", { name: /新建号池/ }));
    expect(screen.getByText("新建账号调度号池")).toBeTruthy();

    const nameInput = screen.getByLabelText("号池名称") as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: "前端验证多模型池" } });

    fireEvent.click(screen.getByRole("button", { name: /创建号池/ }));
    expect(screen.getAllByText("前端验证多模型池").length).toBeGreaterThan(0);
  });
});
