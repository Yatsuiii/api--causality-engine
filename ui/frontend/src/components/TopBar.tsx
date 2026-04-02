import {
  Play,
  Save,
  Clock,
  Globe,
  ChevronDown,
  Loader2,
  Wifi,
  WifiOff,
} from "lucide-react";
import type { Environment } from "../types";
import type { RightPanel } from "../App";
import { useState, useRef, useEffect } from "react";

interface TopBarProps {
  backendOnline: boolean;
  activeEnv: string | null;
  environments: Environment[];
  onEnvChange: (env: string | null) => void;
  onRun: () => void;
  onSave: () => void;
  isRunning: boolean;
  dirty: boolean;
  hasScenario: boolean;
  onToggleHistory: () => void;
  onToggleEnvs: () => void;
  rightPanel: RightPanel;
}

export default function TopBar({
  backendOnline,
  activeEnv,
  environments,
  onEnvChange,
  onRun,
  onSave,
  isRunning,
  dirty,
  hasScenario,
  onToggleHistory,
  onToggleEnvs,
  rightPanel,
}: TopBarProps) {
  const [envOpen, setEnvOpen] = useState(false);
  const envRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (envRef.current && !envRef.current.contains(e.target as Node)) {
        setEnvOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, []);

  return (
    <header
      id="topbar"
      className="h-13 flex items-center justify-between px-4 border-b border-border bg-bg-secondary/80 backdrop-blur-sm shrink-0 select-none"
    >
      {/* Left: Logo */}
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-2">
          <span className="text-xl font-bold bg-gradient-to-r from-accent to-accent-light bg-clip-text text-transparent">
            ⚡ ACE
          </span>
          <span className="text-[0.65rem] font-mono text-text-muted bg-bg-surface px-1.5 py-0.5 rounded">
            v0.1
          </span>
        </div>

        <div className="w-px h-5 bg-border mx-1" />

        {/* Backend status */}
        <div className="flex items-center gap-1.5 text-xs text-text-secondary">
          {backendOnline ? (
            <>
              <Wifi size={12} className="text-success" />
              <span className="text-success">Online</span>
            </>
          ) : (
            <>
              <WifiOff size={12} className="text-error" />
              <span className="text-error">Offline</span>
            </>
          )}
        </div>
      </div>

      {/* Center: Main actions */}
      <div className="flex items-center gap-2">
        {/* Environment selector */}
        <div className="relative" ref={envRef}>
          <button
            id="env-selector"
            onClick={() => setEnvOpen(!envOpen)}
            className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-text-secondary hover:text-text-primary bg-bg-surface hover:bg-bg-hover rounded-lg border border-border transition-all duration-150"
          >
            <Globe size={13} />
            <span>{activeEnv ?? "No Environment"}</span>
            <ChevronDown size={12} className={`transition-transform duration-150 ${envOpen ? "rotate-180" : ""}`} />
          </button>

          {envOpen && (
            <div className="absolute top-full mt-1 right-0 w-48 bg-bg-surface border border-border rounded-lg shadow-xl shadow-black/30 overflow-hidden z-50 animate-fade-in">
              <button
                onClick={() => { onEnvChange(null); setEnvOpen(false); }}
                className={`w-full text-left px-3 py-2 text-xs hover:bg-bg-hover transition-colors ${activeEnv === null ? "text-accent" : "text-text-secondary"}`}
              >
                No Environment
              </button>
              {environments.map((env) => (
                <button
                  key={env.name}
                  onClick={() => { onEnvChange(env.name); setEnvOpen(false); }}
                  className={`w-full text-left px-3 py-2 text-xs hover:bg-bg-hover transition-colors ${activeEnv === env.name ? "text-accent" : "text-text-secondary"}`}
                >
                  {env.name}
                  <span className="ml-1 text-text-muted">
                    ({Object.keys(env.variables).length} vars)
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Save button */}
        <button
          id="save-btn"
          onClick={onSave}
          disabled={!hasScenario || !dirty}
          className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-lg border border-border transition-all duration-150 disabled:opacity-30 disabled:cursor-not-allowed hover:bg-bg-hover text-text-secondary hover:text-text-primary"
          title="Save (Ctrl+S)"
        >
          <Save size={13} />
          <span>Save</span>
          {dirty && (
            <span className="w-1.5 h-1.5 bg-warning rounded-full" />
          )}
        </button>

        {/* Run button */}
        <button
          id="run-btn"
          onClick={onRun}
          disabled={!hasScenario || isRunning}
          className={`flex items-center gap-1.5 px-4 py-1.5 text-xs font-semibold rounded-lg transition-all duration-200 disabled:opacity-40 disabled:cursor-not-allowed ${
            isRunning
              ? "bg-accent/20 text-accent animate-pulse-glow"
              : "bg-accent hover:bg-accent-light text-white hover:shadow-lg hover:shadow-accent/25"
          }`}
          title="Run (Ctrl+Enter)"
        >
          {isRunning ? (
            <Loader2 size={13} className="animate-spin" />
          ) : (
            <Play size={13} fill="currentColor" />
          )}
          <span>{isRunning ? "Running…" : "Run"}</span>
        </button>
      </div>

      {/* Right: Panel toggles */}
      <div className="flex items-center gap-1">
        <button
          id="history-toggle"
          onClick={onToggleHistory}
          className={`p-2 rounded-lg text-xs transition-all duration-150 ${
            rightPanel === "history"
              ? "bg-accent/15 text-accent"
              : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
          }`}
          title="Execution History"
        >
          <Clock size={15} />
        </button>
        <button
          id="env-toggle"
          onClick={onToggleEnvs}
          className={`p-2 rounded-lg text-xs transition-all duration-150 ${
            rightPanel === "environments"
              ? "bg-accent/15 text-accent"
              : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
          }`}
          title="Manage Environments"
        >
          <Globe size={15} />
        </button>
      </div>
    </header>
  );
}
