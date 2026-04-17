import { Trash2, ArrowUp, ArrowDown, ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import { HTTP_METHODS } from "../types";
import type { HttpMethod, Step } from "../types";

interface StepEditorProps {
  step: Step;
  index: number;
  total: number;
  onChange: (step: Step) => void;
  onDelete: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
}

export default function StepEditor({
  step,
  index,
  total,
  onChange,
  onDelete,
  onMoveUp,
  onMoveDown,
}: StepEditorProps) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="bg-bg-surface border border-border rounded-lg overflow-hidden transition-all duration-200 hover:border-accent/40">
      <div className="flex items-center p-3 gap-3">
        {/* Expand toggle */}
        <button
          onClick={() => setExpanded(!expanded)}
          className="text-text-muted hover:text-text-primary p-1 rounded hover:bg-bg-hover transition-colors"
        >
          {expanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
        </button>

        {/* Method & URL */}
        <select
          value={step.method}
          onChange={(e) => onChange({ ...step, method: e.target.value as HttpMethod })}
          className="bg-bg-secondary text-xs font-mono px-2 py-1.5 border border-border rounded text-accent focus:outline-none focus:border-accent"
        >
          {HTTP_METHODS.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>

        <input
          value={step.url}
          onChange={(e) => onChange({ ...step, url: e.target.value })}
          placeholder="https://api.example.com/v1/users"
          className="flex-1 bg-bg-secondary text-sm px-3 py-1.5 border border-border rounded text-text-primary focus:outline-none focus:border-accent"
        />

        {/* Action buttons */}
        <div className="flex items-center gap-1 shrink-0 ml-2">
          <button
            onClick={onMoveUp}
            disabled={index === 0}
            className="p-1.5 text-text-muted hover:text-text-primary disabled:opacity-30 disabled:hover:text-text-muted rounded hover:bg-bg-hover transition-colors"
          >
            <ArrowUp size={14} />
          </button>
          <button
            onClick={onMoveDown}
            disabled={index === total - 1}
            className="p-1.5 text-text-muted hover:text-text-primary disabled:opacity-30 disabled:hover:text-text-muted rounded hover:bg-bg-hover transition-colors"
          >
            <ArrowDown size={14} />
          </button>
          <div className="w-px h-4 bg-border mx-1" />
          <button
            onClick={onDelete}
            className="p-1.5 text-text-muted hover:text-error rounded hover:bg-error/10 transition-colors"
          >
            <Trash2 size={14} />
          </button>
        </div>
      </div>

      {/* Expanded details */}
      {expanded && (
        <div className="p-4 border-t border-border bg-bg-secondary/10 space-y-4">
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-[0.65rem] text-text-muted uppercase tracking-wider font-medium mb-1.5">
                Step Name
              </label>
              <input
                value={step.name}
                onChange={(e) => onChange({ ...step, name: e.target.value })}
                className="w-full bg-bg-secondary text-xs px-2.5 py-1.5 border border-border rounded text-text-primary focus:outline-none focus:border-accent"
              />
            </div>
            <div>
              <label className="block text-[0.65rem] text-text-muted uppercase tracking-wider font-medium mb-1.5">
                State
              </label>
              <input
                value={step.state}
                onChange={(e) => onChange({ ...step, state: e.target.value })}
                className="w-full bg-bg-secondary text-xs font-mono px-2.5 py-1.5 border border-border rounded text-text-primary focus:outline-none focus:border-accent"
              />
            </div>
          </div>
          
          <div>
            <label className="flex items-center justify-between text-[0.65rem] text-text-muted uppercase tracking-wider font-medium mb-1.5">
              <span>Request Body (JSON)</span>
            </label>
            <textarea
              value={typeof step.body === 'object' && step.body !== null ? JSON.stringify(step.body, null, 2) : (typeof step.body === 'string' ? step.body : '')}
              onChange={(e) => {
                try {
                  const val = e.target.value;
                  const parsed = val ? JSON.parse(val) : undefined;
                  onChange({ ...step, body: parsed });
                } catch {
                  onChange({ ...step, body: e.target.value });
                }
              }}
              rows={4}
              className="w-full bg-bg-secondary text-xs font-mono px-2.5 py-2 border border-border rounded text-text-primary focus:outline-none focus:border-accent resize-y"
              placeholder="{}"
            />
          </div>
        </div>
      )}
    </div>
  );
}
