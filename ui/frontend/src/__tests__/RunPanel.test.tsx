import { render, screen } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import RunPanel from "../components/RunPanel";
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

describe("RunPanel", () => {
  it("shows placeholder when idle (no result, not running)", () => {
    render(<RunPanel result={null} isRunning={false} onClose={vi.fn()} />);
    expect(
      screen.getByText("Run a scenario to see results here.")
    ).toBeInTheDocument();
  });

  it("shows passed/failed counts and duration from a HistoryEntry", () => {
    render(<RunPanel result={mockEntry} isRunning={false} onClose={vi.fn()} />);
    expect(screen.getByText(/2 passed/)).toBeInTheDocument();
    expect(screen.getByText(/1 failed/)).toBeInTheDocument();
    expect(screen.getByText(/350ms/)).toBeInTheDocument();
  });
});
