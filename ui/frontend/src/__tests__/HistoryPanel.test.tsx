import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import HistoryPanel from "../components/HistoryPanel";
import type { HistoryEntry } from "../bindings";

const mockEntry: HistoryEntry = {
  id: "abc12345",
  scenario_name: "Login Flow",
  scenario_file: "/examples/login.yaml",
  environment: null,
  started_at: "2026-04-14T10:00:00Z",
  duration_ms: 350,
  total_steps: 3,
  passed: 2,
  failed: 1,
  log: {
    steps: [],
    total_duration_ms: 350,
    total_steps: 3,
    passed: 2,
    failed: 1,
    iterations: 1,
    terminal_state: null,
  },
};

describe("HistoryPanel", () => {
  it("shows empty state message when history is empty", () => {
    render(
      <HistoryPanel
        history={[]}
        onClose={vi.fn()}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onClear={vi.fn()}
      />
    );
    expect(screen.getByText("No run history yet.")).toBeInTheDocument();
  });

  it("renders entry and delete button calls onDelete with entry id", () => {
    const onDelete = vi.fn();
    render(
      <HistoryPanel
        history={[mockEntry]}
        onClose={vi.fn()}
        onSelect={vi.fn()}
        onDelete={onDelete}
        onClear={vi.fn()}
      />
    );

    expect(screen.getByText("Login Flow")).toBeInTheDocument();

    fireEvent.click(screen.getByTitle("Delete entry"));
    expect(onDelete).toHaveBeenCalledWith("abc12345");
  });
});
