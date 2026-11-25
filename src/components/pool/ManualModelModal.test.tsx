// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ManualModelModal } from "./ManualModelModal";
import type { UpstreamAccountCandidate } from "../../types/pool";

function makeCandidate(
  overrides: Partial<UpstreamAccountCandidate> = {}
): UpstreamAccountCandidate {
  return {
    accountId: "acc-codearts-1",
    toolId: "codearts",
    label: "码道 · 主号",
    uidMasked: "ca_10****23",
    channel: "opencode_bridge",
    defaultModels: ["deepseek-v4.1-flash"],
    modelOptions: [
      {
        id: "deepseek-v4.1-flash",
        displayName: "deepseek-v4.1-flash",
        vendorWireId: "deepseek-v4.1-flash",
        family: "DeepSeek",
        source: "manual",
      },
    ],
    defaultTier: "t2_backup",
    quotaPercent: 60,
    quotaText: "600 / 1,000 积分",
    isActiveOnClient: false,
    accountStatus: "active",
    codeArtsMode: "client_readonly",
    ...overrides,
  };
}

afterEach(cleanup);

describe("ManualModelModal", () => {
  it("回填当前模型、按行解析后保存、清空时提交空数组", () => {
    const onSave = vi.fn();
    render(
      <ManualModelModal
        open
        onClose={vi.fn()}
        toolName="码道 CodeArts"
        candidates={[makeCandidate()]}
        onSave={onSave}
        onApplyPreset={vi.fn()}
        presetAvailable
      />
    );

    const textarea = screen.getByLabelText("码道 · 主号 的模型列表（每行一个）");
    expect((textarea as HTMLTextAreaElement).value).toBe("deepseek-v4.1-flash");

    fireEvent.change(textarea, {
      target: { value: "deepseek-v4.1-flash\n  glm-5.3-flash  \n\nkimi-k3" },
    });
    fireEvent.click(screen.getByRole("button", { name: /保存/ }));
    expect(onSave).toHaveBeenCalledWith("acc-codearts-1", [
      "deepseek-v4.1-flash",
      "glm-5.3-flash",
      "kimi-k3",
    ]);

    fireEvent.click(screen.getByRole("button", { name: /清空/ }));
    expect(onSave).toHaveBeenCalledWith("acc-codearts-1", []);
  });

  it("未探测的账号显示「未探测」来源标记，避免把兜底表当成真实目录", () => {
    render(
      <ManualModelModal
        open
        onClose={vi.fn()}
        toolName="码道 CodeArts"
        candidates={[
          makeCandidate({
            modelOptions: [
              {
                id: "deepseek-v4-pro",
                displayName: "DeepSeek-V4-Pro",
                vendorWireId: "deepseek-v4-pro",
                family: "DeepSeek",
                source: "catalog",
              },
            ],
          }),
        ]}
        onSave={vi.fn()}
        onApplyPreset={vi.fn()}
        presetAvailable={false}
      />
    );

    expect(screen.getByText("未探测")).toBeTruthy();
  });

  it("该工具没有账号时给出空态而不是空白弹窗", () => {
    render(
      <ManualModelModal
        open
        onClose={vi.fn()}
        toolName="码道 CodeArts"
        candidates={[]}
        onSave={vi.fn()}
        onApplyPreset={vi.fn()}
        presetAvailable={false}
      />
    );
    expect(screen.getByText("该工具下还没有账号")).toBeTruthy();
  });

  it("有内置预设时可一键套用，并如实标注「内置预设」来源", () => {
    const onApplyPreset = vi.fn();
    render(
      <ManualModelModal
        open
        onClose={vi.fn()}
        toolName="码道 CodeArts"
        candidates={[
          makeCandidate({
            modelOptions: [
              {
                id: "deepseek-v4.1-flash",
                displayName: "deepseek-v4.1-flash",
                vendorWireId: "deepseek-v4.1-flash",
                family: "DeepSeek",
                source: "preset",
              },
            ],
          }),
        ]}
        onSave={vi.fn()}
        onApplyPreset={onApplyPreset}
        presetAvailable
      />
    );

    expect(screen.getByText("内置预设")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /套用推荐目录/ }));
    expect(onApplyPreset).toHaveBeenCalledWith("acc-codearts-1");
  });

  it("没有预设的工具不显示套用按钮", () => {
    render(
      <ManualModelModal
        open
        onClose={vi.fn()}
        toolName="WorkBuddy"
        candidates={[makeCandidate({ toolId: "workbuddy" })]}
        onSave={vi.fn()}
        onApplyPreset={vi.fn()}
        presetAvailable={false}
      />
    );
    expect(screen.queryByRole("button", { name: /套用推荐目录/ })).toBeNull();
  });
});
