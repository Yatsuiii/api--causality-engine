import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import Sidebar from "../components/Sidebar";
import type { ScenarioSummary } from "../bindings";

const scenarios: ScenarioSummary[] = [
  {
    file: "login.yaml",
    name: "Login Flow",
    steps: 3,
    initial_state: "idle",
    concurrency: null,
    error: null,
  },
  {
    file: "checkout.yaml",
    name: "Checkout Flow",
    steps: 5,
    initial_state: "idle",
    concurrency: null,
    error: null,
  },
];

describe("Sidebar", () => {
  it("renders scenario names and calls onSelect with file path when clicked", () => {
    const onSelect = vi.fn();
    render(
      <Sidebar
        scenarios={scenarios}
        selectedFile={null}
        onSelect={onSelect}
        onCreate={vi.fn()}
        onDelete={vi.fn()}
        onDuplicate={vi.fn()}
      />
    );

    expect(screen.getByText("Login Flow")).toBeInTheDocument();
    expect(screen.getByText("Checkout Flow")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Login Flow"));
    expect(onSelect).toHaveBeenCalledWith("login.yaml");
  });

  it("shows create form on + click and calls onCreate on Enter", () => {
    const onCreate = vi.fn();
    render(
      <Sidebar
        scenarios={[]}
        selectedFile={null}
        onSelect={vi.fn()}
        onCreate={onCreate}
        onDelete={vi.fn()}
        onDuplicate={vi.fn()}
      />
    );

    fireEvent.click(screen.getByTitle("New Scenario"));

    const input = screen.getByPlaceholderText("scenario name");
    fireEvent.change(input, { target: { value: "my-new-scenario" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onCreate).toHaveBeenCalledWith("my-new-scenario");
  });
});
