import { X, RefreshCw, Server } from "lucide-react";
import type { Environment } from "../types";

interface EnvManagerProps {
  environments: Environment[];
  activeEnv: string | null;
  onActivate: (name: string | null) => void;
  onClose: () => void;
  onRefresh: () => void;
}

export default function EnvManager({
  environments,
  activeEnv,
  onActivate,
  onClose,
  onRefresh,
}: EnvManagerProps) {
  return (
    <div className="w-80 border-l border-border bg-bg-secondary flex flex-col h-full animate-fade-in">
      <div className="flex items-center justify-between p-4 border-b border-border shrink-0">
        <h2 className="text-sm font-semibold text-text-primary">Environments</h2>
        <div className="flex items-center gap-2">
          <button
            onClick={onRefresh}
            className="text-text-muted hover:text-text-primary transition-colors p-1 rounded"
            title="Refresh environments"
          >
            <RefreshCw size={14} />
          </button>
          <button
            onClick={onClose}
            className="text-text-muted hover:text-text-primary transition-colors p-1 rounded hover:bg-bg-hover"
          >
            <X size={16} />
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {/* No Environment option */}
        <div
          onClick={() => onActivate(null)}
          className={`p-3 rounded-lg border cursor-pointer transition-all ${
            activeEnv === null
              ? "bg-accent/10 border-accent/50 group"
              : "bg-bg-surface border-border hover:border-text-muted"
          }`}
        >
          <div className="flex items-center gap-2">
            <Server size={14} className={activeEnv === null ? "text-accent" : "text-text-muted"} />
            <span className={`text-sm ${activeEnv === null ? "text-accent font-medium" : "text-text-primary"}`}>
              No Environment
            </span>
          </div>
          <p className="text-xs text-text-muted mt-1 ml-6">
            Run without any environment variables.
          </p>
        </div>

        {environments.length === 0 && (
          <div className="text-center text-text-muted text-xs mt-6">
            No environments found.
          </div>
        )}

        {environments.map((env) => (
          <div
            key={env.name}
            onClick={() => onActivate(env.name)}
            className={`p-3 rounded-lg border cursor-pointer transition-all ${
              activeEnv === env.name
                ? "bg-accent/10 border-accent/50 group"
                : "bg-bg-surface border-border hover:border-text-muted"
            }`}
          >
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Server size={14} className={activeEnv === env.name ? "text-accent" : "text-text-muted"} />
                <span className={`text-sm ${activeEnv === env.name ? "text-accent font-medium" : "text-text-primary"}`}>
                  {env.name}
                </span>
              </div>
            </div>
            
            <div className="mt-2 ml-6 text-xs text-text-muted space-y-1">
              <div className="flex justify-between">
                <span>Variables:</span>
                <span className="font-mono text-text-secondary">{Object.keys(env.variables).length}</span>
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
