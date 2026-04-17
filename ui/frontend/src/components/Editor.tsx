import { useState, useRef, useCallback } from "react";
import {
  Code2,
  LayoutGrid,
  Plus,
  Settings2,
  GitBranch,
} from "lucide-react";
import type { Scenario, Step } from "../types";
import StepEditor from "./StepEditor";
import MonacoEditor from "@monaco-editor/react";

interface EditorProps {
  scenario: Scenario;
  yamlContent: string;
  editorMode: "visual" | "yaml";
  onEditorModeChange: (mode: "visual" | "yaml") => void;
  onScenarioChange: (s: Scenario) => void;
  onYamlChange: (y: string) => void;
}

export default function Editor({
  scenario,
  yamlContent,
  editorMode,
  onEditorModeChange,
  onScenarioChange,
  onYamlChange,
}: EditorProps) {
  const [showMeta, setShowMeta] = useState(false);
  const stepsList = scenario.steps;
  const initialState = scenario.initial_state;

  /* ── Stable keys for steps (survives reorder) ─────────────────── */
  const nextKeyId = useRef(0);
  const stepKeysRef = useRef<string[]>([]);
  // Sync key count with steps — only append/trim, never regenerate
  while (stepKeysRef.current.length < stepsList.length) {
    stepKeysRef.current.push(`step-key-${++nextKeyId.current}`);
  }
  if (stepKeysRef.current.length > stepsList.length) {
    stepKeysRef.current.length = stepsList.length;
  }

  const updateStep = (index: number, step: Step) => {
    const steps = stepsList.map((s: Step, i: number) => (i === index ? step : s));
    onScenarioChange({ ...scenario, steps });
  };

  const deleteStep = useCallback((index: number) => {
    const removedState = stepsList[index]?.state;
    const prevState = index > 0 ? stepsList[index - 1]?.state : undefined;
    const nextState = index < stepsList.length - 1 ? stepsList[index + 1]?.state : "done";
    const steps = stepsList.filter((_, i) => i !== index);
    stepKeysRef.current.splice(index, 1);
    let edges = (scenario.edges ?? []).filter(
      (edge) => edge.from !== removedState && edge.to !== removedState,
    );
    if (prevState) {
      edges = replaceDefaultEdge(edges, prevState, nextState ?? "done");
    }
    onScenarioChange({ ...scenario, steps, edges });
  }, [stepsList, scenario, onScenarioChange]);

  const addStep = () => {
    const newState =
      stepsList.length === 0 ? initialState : `state_${stepsList.length + 1}`;
    const newStep: Step = {
      name: `step ${stepsList.length + 1}`,
      state: newState,
      method: "GET",
      url: "",
    };
    let edges = [...(scenario.edges ?? [])];
    if (stepsList.length > 0) {
      edges = replaceDefaultEdge(edges, stepsList[stepsList.length - 1].state, newState);
    }
    edges.push({ from: newState, to: "done", default: true });
    stepKeysRef.current.push(`step-key-${++nextKeyId.current}`);
    onScenarioChange({ ...scenario, steps: [...stepsList, newStep], edges });
  };

  const moveStep = (from: number, to: number) => {
    if (to < 0 || to >= stepsList.length) return;
    const steps = [...stepsList];
    const [removed] = steps.splice(from, 1);
    steps.splice(to, 0, removed);
    // Reorder keys to match
    const keys = [...stepKeysRef.current];
    const [removedKey] = keys.splice(from, 1);
    keys.splice(to, 0, removedKey);
    stepKeysRef.current = keys;
    onScenarioChange({ ...scenario, steps });
  };

  /* ── State machine nodes ─────────────────────────────────────────── */
  const stateNodes = new Set<string>();
  stateNodes.add(initialState);
  stepsList.forEach((s) => {
    stateNodes.add(s.state);
  });
  (scenario.edges ?? []).forEach((edge) => {
    stateNodes.add(edge.from);
    stateNodes.add(edge.to);
  });
  const statesArray = Array.from(stateNodes);

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* Editor toolbar */}
      <div className="flex items-center justify-between px-4 py-2 border-b border-border bg-bg-secondary/30 shrink-0">
        <div className="flex items-center gap-3">
          <h1 className="text-sm font-semibold text-text-primary">
            {scenario.name}
          </h1>
          {scenario.concurrency && scenario.concurrency > 1 && (
            <span className="text-[0.6rem] font-mono text-text-muted bg-bg-surface px-1.5 py-0.5 rounded">
              ×{scenario.concurrency} users
            </span>
          )}
        </div>

        <div className="flex items-center gap-1.5">
          {/* Metadata toggle */}
          <button
            id="meta-toggle"
            onClick={() => setShowMeta(!showMeta)}
            className={`p-1.5 rounded-md text-xs transition-all duration-150 ${
              showMeta
                ? "bg-accent/15 text-accent"
                : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
            }`}
            title="Scenario Settings"
          >
            <Settings2 size={14} />
          </button>

          {/* Mode toggle */}
          <div className="flex bg-bg-surface rounded-lg border border-border p-0.5">
            <button
              id="visual-mode-btn"
              onClick={() => onEditorModeChange("visual")}
              className={`flex items-center gap-1 px-2.5 py-1 text-xs rounded-md transition-all duration-150 ${
                editorMode === "visual"
                  ? "bg-accent text-white shadow-sm"
                  : "text-text-secondary hover:text-text-primary"
              }`}
            >
              <LayoutGrid size={12} />
              Visual
            </button>
            <button
              id="yaml-mode-btn"
              onClick={() => onEditorModeChange("yaml")}
              className={`flex items-center gap-1 px-2.5 py-1 text-xs rounded-md transition-all duration-150 ${
                editorMode === "yaml"
                  ? "bg-accent text-white shadow-sm"
                  : "text-text-secondary hover:text-text-primary"
              }`}
            >
              <Code2 size={12} />
              YAML
            </button>
          </div>
        </div>
      </div>

      {/* Metadata panel */}
      {showMeta && editorMode === "visual" && (
        <div className="px-4 py-3 border-b border-border bg-bg-surface/30 animate-fade-in">
          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
            <div>
              <label className="text-[0.65rem] text-text-muted uppercase tracking-wider font-medium">
                Name
              </label>
              <input
                value={scenario.name}
                onChange={(e) =>
                  onScenarioChange({ ...scenario, name: e.target.value })
                }
                className="w-full mt-1 px-2 py-1.5 text-xs bg-bg-surface border border-border rounded-md text-text-primary focus:outline-none focus:border-accent/50"
              />
            </div>
            <div>
              <label className="text-[0.65rem] text-text-muted uppercase tracking-wider font-medium">
                Initial State
              </label>
              <input
                value={initialState}
                onChange={(e) =>
                  onScenarioChange({
                    ...scenario,
                    initial_state: e.target.value,
                  })
                }
                className="w-full mt-1 px-2 py-1.5 text-xs bg-bg-surface border border-border rounded-md text-text-primary font-mono focus:outline-none focus:border-accent/50"
              />
            </div>
            <div>
              <label className="text-[0.65rem] text-text-muted uppercase tracking-wider font-medium">
                Concurrency
              </label>
              <input
                type="number"
                min={1}
                max={100}
                value={scenario.concurrency ?? 1}
                onChange={(e) =>
                  onScenarioChange({
                    ...scenario,
                    concurrency: parseInt(e.target.value) || 1,
                  })
                }
                className="w-full mt-1 px-2 py-1.5 text-xs bg-bg-surface border border-border rounded-md text-text-primary font-mono focus:outline-none focus:border-accent/50"
              />
            </div>
            <div>
              <label className="text-[0.65rem] text-text-muted uppercase tracking-wider font-medium">
                Variables
              </label>
              <div className="mt-1 text-xs text-text-secondary">
                {scenario.variables
                  ? Object.keys(scenario.variables).length + " defined"
                  : "None"}
              </div>
            </div>
          </div>

          {/* Variables editor */}
          {scenario.variables && Object.keys(scenario.variables).length > 0 && (
            <div className="mt-3 border-t border-border pt-3">
              <div className="text-[0.65rem] text-text-muted uppercase tracking-wider font-medium mb-1.5">
                Variables
              </div>
              <div className="space-y-1">
                {Object.entries(scenario.variables).map(([key, val]) => (
                  <div key={key} className="flex gap-2">
                    <input
                      value={key}
                      readOnly
                      className="w-40 px-2 py-1 text-xs bg-bg-surface border border-border rounded text-accent font-mono"
                    />
                    <input
                      value={val}
                      onChange={(e) => {
                        const vars = { ...scenario.variables, [key]: e.target.value };
                        onScenarioChange({ ...scenario, variables: vars });
                      }}
                      className="flex-1 px-2 py-1 text-xs bg-bg-surface border border-border rounded text-text-primary font-mono focus:outline-none focus:border-accent/50"
                    />
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {/* State machine visualization */}
      {editorMode === "visual" && statesArray.length > 1 && (
        <div className="px-4 py-2 border-b border-border bg-bg-secondary/20 shrink-0">
          <div className="flex items-center gap-1.5 overflow-x-auto pb-0.5">
            <GitBranch size={12} className="text-text-muted shrink-0" />
            {statesArray.map((state, i) => (
              <span key={state} className="flex items-center gap-1.5 shrink-0">
                <span
                  className={`px-2 py-0.5 text-[0.65rem] font-mono rounded-full border ${
                    state === initialState
                      ? "border-accent/40 bg-accent/10 text-accent"
                      : state === "done"
                      ? "border-success/40 bg-success/10 text-success"
                      : "border-border bg-bg-surface text-text-secondary"
                  }`}
                >
                  {state}
                </span>
                {i < statesArray.length - 1 && (
                  <span className="text-text-muted text-[0.6rem]">→</span>
                )}
              </span>
            ))}
          </div>
        </div>
      )}

      {/* Content area */}
      <div className="flex-1 overflow-y-auto">
        {editorMode === "visual" ? (
          <div className="p-4 space-y-3 stagger">
            {stepsList.map((step, i) => (
              <StepEditor
                key={stepKeysRef.current[i]}
                step={step}
                index={i}
                total={stepsList.length}
                onChange={(s) => updateStep(i, s)}
                onDelete={() => deleteStep(i)}
                onMoveUp={() => moveStep(i, i - 1)}
                onMoveDown={() => moveStep(i, i + 1)}
              />
            ))}

            {/* Add step button */}
            <button
              id="add-step-btn"
              onClick={addStep}
              className="w-full py-3 border-2 border-dashed border-border hover:border-accent/40 rounded-xl text-text-muted hover:text-accent text-xs font-medium transition-all duration-200 flex items-center justify-center gap-1.5 hover:bg-accent/5"
            >
              <Plus size={14} />
              Add Step
            </button>
          </div>
        ) : (
          <div className="h-full" id="yaml-editor">
            <MonacoEditor
              height="100%"
              language="yaml"
              theme="vs-dark"
              value={yamlContent}
              onChange={(v) => onYamlChange(v ?? "")}
              options={{
                fontSize: 13,
                fontFamily: "'JetBrains Mono', monospace",
                minimap: { enabled: false },
                lineNumbers: "on",
                scrollBeyondLastLine: false,
                wordWrap: "on",
                tabSize: 2,
                padding: { top: 12, bottom: 12 },
                renderLineHighlight: "gutter",
                bracketPairColorization: { enabled: true },
                automaticLayout: true,
              }}
            />
          </div>
        )}
      </div>
    </div>
  );
}

function replaceDefaultEdge(
  edges: NonNullable<Scenario["edges"]>,
  from: string,
  to: string,
): NonNullable<Scenario["edges"]> {
  const next = edges.filter((edge) => !(edge.from === from && edge.default));
  next.push({ from, to, default: true });
  return next;
}
