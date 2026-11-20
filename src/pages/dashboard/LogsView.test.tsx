// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ActionLogItem } from "../../types/qwitch";

vi.mock("../../context/QwitchContext", () => ({
  useQwitch: () => ({
    adapters: [],
    logs: [],
    logScope: "active",
    hasMoreLogs: false,
    loadMoreLogs: vi.fn(),
    archiveLogs: vi.fn(),
    unarchiveLogs: vi.fn(),
    deleteLogs: vi.fn(),
  }),
}));

import { LogsPanel } from "./LogsView";

const mkLog = (id: number): ActionLogItem => ({
  id,
  accountId: "acc-1",
  toolId: "trae_cn",
  actionType: "query",
  resultStatus: "success",
  messageSanitized: `log message ${id}`,
  createdAt: "2026-11-20T10:00:00+08:00",
});

afterEach(cleanup);

describe("LogsPanel", () => {
  it("exposes selection state through role=checkbox / aria-checked", () => {
    const items = [mkLog(1), mkLog(2)];
    const toggle = vi.fn();
    render(
      <LogsPanel
        currentLogTotal={2}
        filteredLogs={items}
        groupedLogs={[{ day: "今天", items }]}
        selectedLogIds={[1]}
        setConfirmLogAction={vi.fn()}
        toggleSelectLog={toggle}
      />
    );
    const boxes = screen.getAllByRole("checkbox");
    expect(boxes).toHaveLength(2);
    expect(boxes[0].getAttribute("aria-checked")).toBe("true");
    expect(boxes[1].getAttribute("aria-checked")).toBe("false");
    fireEvent.click(boxes[1]);
    expect(toggle).toHaveBeenCalledWith(2);
  });

  it("renders no checkboxes when there are no logs", () => {
    render(
      <LogsPanel
        currentLogTotal={0}
        filteredLogs={[]}
        groupedLogs={[]}
        selectedLogIds={[]}
        setConfirmLogAction={vi.fn()}
        toggleSelectLog={vi.fn()}
      />
    );
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
  });
});
