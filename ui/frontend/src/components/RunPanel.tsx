import { X, Play, Loader2, CheckCircle2, XCircle } from "lucide-react";
import type { HistoryEntry } from "../types";

interface RunPanelProps {
  result: HistoryEntry | null;
  isRunning: boolean;
  onClose: () => void;
}

export default function RunPanel({ result, isRunning, onClose }: RunPanelProps) {
  return (
    <div className="h-64 border-t border-border bg-bg-secondary flex flex-col animate-slide-up shrink-0">
      <div className="flex items-center justify-between p-3 border-b border-border bg-bg-surface/50">
        <div className="flex items-center gap-3">
          <h3 className="font-semibold text-text-primary text-sm flex items-center gap-2">
            <Play size={14} className="text-accent" />
            Execution Results
          </h3>
          {isRunning && (
            <span className="flex items-center gap-1.5 text-xs text-text-muted bg-bg-secondary px-2 py-0.5 rounded-full border border-border">
              <Loader2 size={12} className="animate-spin text-accent" />
              Running scenario...
            </span>
          )}
          {result && (
            <span className="flex items-center gap-2 text-xs">
              <span className="text-text-muted">Duration: {result.duration_ms}ms</span>
              <span className="w-px h-3 bg-border" />
              <span className="text-success flex gap-1 items-center">
                <CheckCircle2 size={12} /> {result.passed} passed
              </span>
              <span className="text-error flex gap-1 items-center">
                <XCircle size={12} /> {result.failed} failed
              </span>
            </span>
          )}
        </div>
        <button
          onClick={onClose}
          className="text-text-muted hover:text-text-primary p-1 rounded hover:bg-bg-hover transition-colors"
        >
          <X size={16} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-4 bg-bg-primary">
        {!result && !isRunning ? (
          <div className="h-full flex items-center justify-center text-text-muted text-sm">
            Run a scenario to see results here.
          </div>
        ) : isRunning ? (
          <div className="h-full flex flex-col items-center justify-center text-text-muted gap-3">
            <Loader2 size={24} className="animate-spin text-accent" />
            <p className="text-sm">Executing engine... please wait.</p>
          </div>
        ) : result ? (
          <div className="space-y-4">
            {result.log.steps.map((step, idx) => (
              <div key={idx} className="bg-bg-surface rounded-lg border border-border overflow-hidden">
                <div className="flex items-center justify-between p-3 border-b border-border bg-bg-secondary/30">
                  <div className="flex items-center gap-3">
                    <span className={`px-2 py-0.5 rounded text-xs font-mono font-medium ${
                      step.status >= 200 && step.status < 300 
                        ? 'bg-success/10 text-success border border-success/20' 
                        : 'bg-error/10 text-error border border-error/20'
                    }`}>
                      {step.method} {step.status}
                    </span>
                    <span className="font-semibold text-text-primary text-sm">{step.step_name}</span>
                    <span className="text-xs text-text-muted font-mono bg-bg-secondary px-1.5 py-0.5 rounded border border-border truncate max-w-[200px]">
                      {step.url}
                    </span>
                  </div>
                  <span className="text-xs text-text-muted">{step.duration_ms}ms</span>
                </div>
                
                <div className="p-3 space-y-3">
                  <div className="grid grid-cols-2 gap-4">
                    <div>
                      <div className="text-[0.65rem] text-text-muted uppercase tracking-wider font-medium mb-1">Assertions</div>
                      {step.assertions.length === 0 ? (
                        <div className="text-xs text-text-secondary italic">No assertions</div>
                      ) : (
                        <ul className="space-y-1">
                          {step.assertions.map((assert, i) => (
                            <li key={i} className="flex items-start gap-2 text-xs">
                              {assert.passed ? (
                                <CheckCircle2 size={12} className="text-success mt-0.5 shrink-0" />
                              ) : (
                                <XCircle size={12} className="text-error mt-0.5 shrink-0" />
                              )}
                              <div>
                                <span className={assert.passed ? "text-text-primary" : "text-error"}>
                                  {assert.description}
                                </span>
                                {!assert.passed && assert.actual && (
                                  <div className="text-[0.65rem] text-text-muted mt-0.5 font-mono bg-bg-secondary p-1 rounded">
                                    Expected: {assert.expected} | Actual: {assert.actual}
                                  </div>
                                )}
                              </div>
                            </li>
                          ))}
                        </ul>
                      )}
                    </div>
                    <div>
                      <div className="text-[0.65rem] text-text-muted uppercase tracking-wider font-medium mb-1">State Transition</div>
                      <div className="flex items-center gap-2 text-xs font-mono">
                        <span className="bg-bg-secondary px-1.5 py-0.5 rounded border border-border">{step.state_before}</span>
                        <span className="text-text-muted">→</span>
                        <span className="bg-success/10 text-success px-1.5 py-0.5 rounded border border-success/20">{step.state_after}</span>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
}
