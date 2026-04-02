import { X, Trash2, Clock, CheckCircle2, XCircle } from "lucide-react";
import type { HistoryEntry } from "../types";

interface HistoryPanelProps {
  history: HistoryEntry[];
  onClose: () => void;
  onSelect: (entry: HistoryEntry) => void;
  onDelete: (id: string) => void;
  onClear: () => void;
}

export default function HistoryPanel({
  history,
  onClose,
  onSelect,
  onDelete,
  onClear,
}: HistoryPanelProps) {
  return (
    <div className="w-80 border-l border-border bg-bg-secondary flex flex-col h-full animate-fade-in">
      <div className="flex items-center justify-between p-4 border-b border-border shrink-0">
        <h2 className="text-sm font-semibold text-text-primary">Run History</h2>
        <div className="flex items-center gap-2">
          {history.length > 0 && (
            <button
              onClick={onClear}
              className="text-text-muted hover:text-error transition-colors p-1 rounded"
              title="Clear all"
            >
              <Trash2 size={16} />
            </button>
          )}
          <button
            onClick={onClose}
            className="text-text-muted hover:text-text-primary transition-colors p-1 rounded hover:bg-bg-hover"
          >
            <X size={16} />
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-2 space-y-2">
        {history.length === 0 ? (
          <div className="text-center text-text-muted text-xs mt-10">
            No run history yet.
          </div>
        ) : (
          history.map((entry) => (
            <div
              key={entry.id}
              className="bg-bg-surface border border-border rounded-lg p-3 hover:border-accent/40 cursor-pointer transition-colors group"
              onClick={() => onSelect(entry)}
            >
              <div className="flex items-center justify-between mb-2">
                <span className="font-medium text-text-primary text-xs truncate max-w-[150px]">
                  {entry.scenario_name}
                </span>
                <span className="text-[0.65rem] text-text-muted flex items-center gap-1">
                  <Clock size={10} />
                  {new Date(entry.started_at).toLocaleTimeString()}
                </span>
              </div>
              
              <div className="flex items-center justify-between text-xs">
                <div className="flex items-center gap-3">
                  <span className="flex items-center gap-1 text-success">
                    <CheckCircle2 size={12} /> {entry.passed}
                  </span>
                  <span className="flex items-center gap-1 text-error">
                    <XCircle size={12} /> {entry.failed}
                  </span>
                </div>
                <span className="text-text-muted">{entry.duration_ms}ms</span>
              </div>
              
              <div className="mt-2 flex justify-end opacity-0 group-hover:opacity-100 transition-opacity">
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    onDelete(entry.id);
                  }}
                  className="text-text-muted hover:text-error p-1 rounded bg-bg-secondary"
                  title="Delete entry"
                >
                  <Trash2 size={12} />
                </button>
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
