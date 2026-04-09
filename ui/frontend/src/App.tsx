import { useState, useEffect, useCallback, useRef } from "react";
import type { ScenarioListItem, Scenario, HistoryEntry, Environment } from "./types";
import { isScenario } from "./types";
import * as api from "./hooks/useApi";
import TopBar from "./components/TopBar";
import Sidebar from "./components/Sidebar";
import Editor from "./components/Editor";
import RunPanel from "./components/RunPanel";
import HistoryPanel from "./components/HistoryPanel";
import EnvManager from "./components/EnvManager";

export type RightPanel = "none" | "history" | "environments";

/* ── Toast notification ────────────────────────────────────────────── */
interface Toast {
  id: number;
  message: string;
  type: "error" | "success";
}
let toastId = 0;

export default function App() {
  /* ── State ───────────────────────────────────────────────────────── */
  const [scenarios, setScenarios] = useState<ScenarioListItem[]>([]);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [scenario, setScenario] = useState<Scenario | null>(null);
  const [yamlContent, setYamlContent] = useState<string>("");
  const [editorMode, setEditorMode] = useState<"visual" | "yaml">("visual");

  const [environments, setEnvironments] = useState<Environment[]>([]);
  const [activeEnv, setActiveEnv] = useState<string | null>(null);

  const [runResult, setRunResult] = useState<HistoryEntry | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [showRunPanel, setShowRunPanel] = useState(false);

  const [rightPanel, setRightPanel] = useState<RightPanel>("none");
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);

  const [backendOnline, setBackendOnline] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [loadErrors, setLoadErrors] = useState<{ scenarios: boolean; environments: boolean }>({
    scenarios: false,
    environments: false,
  });

  const notify = useCallback((message: string, type: "error" | "success" = "error") => {
    const id = ++toastId;
    setToasts((prev) => [...prev, { id, message, type }]);
    setTimeout(() => setToasts((prev) => prev.filter((t) => t.id !== id)), 4000);
  }, []);

  /* ── Load initial data ───────────────────────────────────────────── */
  useEffect(() => {
    api.checkHealth().then(() => setBackendOnline(true)).catch(() => setBackendOnline(false));
    loadScenarios();
    loadEnvironments();
  }, []);

  const loadScenarios = useCallback(async () => {
    try {
      const list = await api.fetchScenarios();
      setScenarios(list);
      setLoadErrors((prev) => ({ ...prev, scenarios: false }));
    } catch {
      setLoadErrors((prev) => ({ ...prev, scenarios: true }));
    }
  }, []);

  const loadEnvironments = useCallback(async () => {
    try {
      const envs = await api.fetchEnvironments();
      setEnvironments(envs);
      setLoadErrors((prev) => ({ ...prev, environments: false }));
    } catch {
      setLoadErrors((prev) => ({ ...prev, environments: true }));
    }
  }, []);

  const loadHistory = useCallback(async () => {
    setHistoryLoading(true);
    try {
      const h = await api.fetchHistory();
      setHistory(h);
    } catch {
      notify("Failed to load history");
    } finally {
      setHistoryLoading(false);
    }
  }, [notify]);

  /* ── Select scenario ─────────────────────────────────────────────── */
  const selectScenario = useCallback(async (file: string) => {
    if (dirty) {
      if (!window.confirm("You have unsaved changes. Discard and switch?")) return;
    }
    try {
      const [parsed, raw] = await Promise.all([
        api.fetchScenario(file),
        api.fetchScenarioRaw(file),
      ]);
      if (!isScenario(parsed.scenario)) {
        notify("Invalid scenario format received from server");
        return;
      }
      setSelectedFile(file);
      setScenario(parsed.scenario);
      setYamlContent(raw.content);
      setDirty(false);
      setShowRunPanel(false);
      setRunResult(null);
    } catch (e) {
      notify(`Failed to load scenario: ${e instanceof Error ? e.message : "Unknown error"}`);
    }
  }, [dirty, notify]);

  /* ── Save ────────────────────────────────────────────────────────── */
  const saveScenario = useCallback(async (): Promise<boolean> => {
    if (!selectedFile) return false;
    try {
      if (editorMode === "yaml") {
        await api.saveScenarioRaw(selectedFile, yamlContent);
        const parsed = await api.fetchScenario(selectedFile);
        if (isScenario(parsed.scenario)) {
          setScenario(parsed.scenario);
        }
      } else if (scenario) {
        await api.updateScenario(selectedFile, scenario);
        const raw = await api.fetchScenarioRaw(selectedFile);
        setYamlContent(raw.content);
      }
      setDirty(false);
      loadScenarios();
      notify("Scenario saved", "success");
      return true;
    } catch (e) {
      notify(`Failed to save: ${e instanceof Error ? e.message : "Unknown error"}`);
      return false;
    }
  }, [selectedFile, editorMode, yamlContent, scenario, loadScenarios, notify]);

  /* ── Run ─────────────────────────────────────────────────────────── */
  const handleRun = useCallback(async () => {
    if (!selectedFile) return;
    setIsRunning(true);
    setShowRunPanel(true);
    setRunResult(null);
    try {
      // Save first if dirty
      if (dirty) {
        const saved = await saveScenario();
        if (!saved) return;
      }
      const result = await api.runScenario(
        selectedFile,
        activeEnv ?? undefined
      );
      setRunResult(result);
    } catch (e) {
      notify(`Run failed: ${e instanceof Error ? e.message : "Unknown error"}`);
    } finally {
      setIsRunning(false);
    }
  }, [selectedFile, activeEnv, dirty, saveScenario]);

  /* ── Create scenario ─────────────────────────────────────────────── */
  const handleCreate = useCallback(async (name: string) => {
    try {
      const res = await api.createScenario({
        name,
        initial_state: "start",
        steps: [],
      });
      await loadScenarios();
      selectScenario(res.file);
    } catch (e) {
      notify(`Failed to create: ${e instanceof Error ? e.message : "Unknown error"}`);
    }
  }, [loadScenarios, selectScenario, notify]);

  /* ── Delete scenario ─────────────────────────────────────────────── */
  const handleDelete = useCallback(async (file: string) => {
    try {
      await api.deleteScenario(file);
      if (selectedFile === file) {
        setSelectedFile(null);
        setScenario(null);
        setYamlContent("");
      }
      loadScenarios();
    } catch (e) {
      notify(`Failed to delete: ${e instanceof Error ? e.message : "Unknown error"}`);
    }
  }, [selectedFile, loadScenarios, notify]);

  /* ── Duplicate ───────────────────────────────────────────────────── */
  const handleDuplicate = useCallback(async (file: string) => {
    try {
      const res = await api.duplicateScenario(file);
      await loadScenarios();
      selectScenario(res.file);
    } catch (e) {
      notify(`Failed to duplicate: ${e instanceof Error ? e.message : "Unknown error"}`);
    }
  }, [loadScenarios, selectScenario, notify]);

  /* ── Keyboard shortcuts (use refs to avoid listener churn) ────────── */
  const saveRef = useRef(saveScenario);
  const runRef = useRef(handleRun);
  saveRef.current = saveScenario;
  runRef.current = handleRun;

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "s") {
        e.preventDefault();
        saveRef.current();
      }
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        runRef.current();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  /* ── Toggle right panel ──────────────────────────────────────────── */
  const togglePanel = useCallback(
    (panel: RightPanel) => {
      if (rightPanel === panel) {
        setRightPanel("none");
      } else {
        setRightPanel(panel);
        if (panel === "history") loadHistory();
        if (panel === "environments") loadEnvironments();
      }
    },
    [rightPanel, loadHistory, loadEnvironments]
  );

  /* ── Render ──────────────────────────────────────────────────────── */
  return (
    <div id="app-root" className="flex flex-col h-full w-full bg-bg-primary">
      <TopBar
        backendOnline={backendOnline}
        activeEnv={activeEnv}
        environments={environments}
        onEnvChange={setActiveEnv}
        onRun={handleRun}
        onSave={saveScenario}
        isRunning={isRunning}
        dirty={dirty}
        hasScenario={!!selectedFile}
        onToggleHistory={() => togglePanel("history")}
        onToggleEnvs={() => togglePanel("environments")}
        rightPanel={rightPanel}
      />

      <div className="flex flex-1 overflow-hidden">
        {/* Sidebar */}
        <Sidebar
          scenarios={scenarios}
          selectedFile={selectedFile}
          onSelect={selectScenario}
          onCreate={handleCreate}
          onDelete={handleDelete}
          onDuplicate={handleDuplicate}
        />

        {/* Main editor area */}
        <main className="flex-1 flex flex-col overflow-hidden">
          {(loadErrors.scenarios || loadErrors.environments) && (
            <div className="px-4 py-2 text-xs text-error bg-error/10 border-b border-error/20">
              Backend data could not be fully loaded. Check the desktop backend connection.
            </div>
          )}
          {selectedFile && scenario ? (
            <>
              <Editor
                scenario={scenario}
                yamlContent={yamlContent}
                editorMode={editorMode}
                onEditorModeChange={setEditorMode}
                onScenarioChange={(s) => {
                  setScenario(s);
                  setDirty(true);
                }}
                onYamlChange={(y) => {
                  setYamlContent(y);
                  setDirty(true);
                }}
              />
              {showRunPanel && (
                <RunPanel
                  result={runResult}
                  isRunning={isRunning}
                  onClose={() => setShowRunPanel(false)}
                />
              )}
            </>
          ) : (
            <EmptyState onQuickOpen={() => {
              if (scenarios.length > 0) selectScenario(scenarios[0].file);
            }} />
          )}
        </main>

        {/* Right panel */}
        {rightPanel === "history" && (
          <HistoryPanel
            history={history}
            loading={historyLoading}
            onClose={() => setRightPanel("none")}
            onSelect={(entry) => {
              setRunResult(entry);
              setShowRunPanel(true);
              if (entry.scenario_file) selectScenario(entry.scenario_file);
            }}
            onDelete={async (id) => {
              await api.deleteHistoryEntry(id);
              loadHistory();
            }}
            onClear={async () => {
              await api.clearHistory();
              setHistory([]);
            }}
          />
        )}
        {rightPanel === "environments" && (
          <EnvManager
            environments={environments}
            activeEnv={activeEnv}
            onActivate={setActiveEnv}
            onClose={() => setRightPanel("none")}
            onRefresh={loadEnvironments}
          />
        )}
      </div>

      {/* Toast notifications */}
      <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={`px-4 py-2.5 rounded-lg text-xs font-medium shadow-lg animate-fade-in ${
              t.type === "error"
                ? "bg-error/90 text-white"
                : "bg-success/90 text-white"
            }`}
          >
            {t.message}
          </div>
        ))}
      </div>
    </div>
  );
}

/* ── Empty state ───────────────────────────────────────────────────── */

function EmptyState({ onQuickOpen }: { onQuickOpen: () => void }) {
  return (
    <div className="flex-1 flex items-center justify-center">
      <div className="text-center animate-fade-in">
        <div className="text-6xl mb-6 opacity-20">⚡</div>
        <h2 className="text-2xl font-semibold text-text-primary mb-2">
          ACE Desktop
        </h2>
        <p className="text-text-secondary mb-6 max-w-md">
          Select a scenario from the sidebar to start editing, or create a new
          one. Use <kbd className="px-1.5 py-0.5 bg-bg-surface rounded text-xs font-mono">Ctrl+Enter</kbd> to
          run.
        </p>
        <button
          onClick={onQuickOpen}
          className="px-5 py-2.5 bg-accent hover:bg-accent-light text-white rounded-lg font-medium transition-all duration-200 hover:shadow-lg hover:shadow-accent/20"
        >
          Open First Scenario
        </button>
      </div>
    </div>
  );
}
