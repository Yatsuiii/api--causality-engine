import { useState } from "react";
import {
  FileText,
  Plus,
  Search,
  Trash2,
  Copy,
  ChevronRight,
  Zap,
} from "lucide-react";
import type { ScenarioListItem } from "../types";

interface SidebarProps {
  scenarios: ScenarioListItem[];
  selectedFile: string | null;
  onSelect: (file: string) => void;
  onCreate: (name: string) => void;
  onDelete: (file: string) => void;
  onDuplicate: (file: string) => void;
}

export default function Sidebar({
  scenarios,
  selectedFile,
  onSelect,
  onCreate,
  onDelete,
  onDuplicate,
}: SidebarProps) {
  const [search, setSearch] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [newName, setNewName] = useState("");
  const [contextMenu, setContextMenu] = useState<{
    file: string;
    x: number;
    y: number;
  } | null>(null);

  const filtered = scenarios.filter(
    (s) =>
      s.name.toLowerCase().includes(search.toLowerCase()) ||
      s.file.toLowerCase().includes(search.toLowerCase())
  );

  const handleCreate = () => {
    if (newName.trim()) {
      onCreate(newName.trim());
      setNewName("");
      setShowCreate(false);
    }
  };

  const handleContext = (e: React.MouseEvent, file: string) => {
    e.preventDefault();
    setContextMenu({ file, x: e.clientX, y: e.clientY });
  };

  return (
    <>
      <aside
        id="sidebar"
        className="w-64 flex flex-col border-r border-border bg-bg-secondary/50 shrink-0 select-none"
      >
        {/* Header */}
        <div className="p-3 border-b border-border">
          <div className="flex items-center justify-between mb-2.5">
            <h2 className="text-xs font-semibold text-text-secondary uppercase tracking-wider">
              Scenarios
            </h2>
            <button
              id="create-scenario-btn"
              onClick={() => setShowCreate(!showCreate)}
              className="p-1 rounded-md hover:bg-bg-hover text-text-secondary hover:text-accent transition-colors duration-150"
              title="New Scenario"
            >
              <Plus size={14} />
            </button>
          </div>

          {/* Search */}
          <div className="relative">
            <Search
              size={13}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 text-text-muted"
            />
            <input
              id="scenario-search"
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Filter scenarios…"
              className="w-full pl-8 pr-3 py-1.5 text-xs bg-bg-surface border border-border rounded-md text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent/50 focus:ring-1 focus:ring-accent/20 transition-all duration-150"
            />
          </div>

          {/* Create form */}
          {showCreate && (
            <div className="mt-2 flex gap-1.5 animate-fade-in">
              <input
                autoFocus
                type="text"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && handleCreate()}
                placeholder="scenario name"
                className="flex-1 px-2 py-1.5 text-xs bg-bg-surface border border-border rounded-md text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent/50"
              />
              <button
                onClick={handleCreate}
                className="px-2 py-1.5 text-xs bg-accent hover:bg-accent-light text-white rounded-md transition-colors"
              >
                Create
              </button>
            </div>
          )}
        </div>

        {/* Scenario list */}
        <div className="flex-1 overflow-y-auto py-1 stagger">
          {filtered.length === 0 ? (
            <div className="px-3 py-8 text-center text-text-muted text-xs">
              {scenarios.length === 0
                ? "No scenarios found"
                : "No matches"}
            </div>
          ) : (
            filtered.map((s) => (
              <button
                key={s.file}
                onClick={() => onSelect(s.file)}
                onContextMenu={(e) => handleContext(e, s.file)}
                className={`w-full text-left px-3 py-2 flex items-center gap-2.5 group transition-all duration-150 ${
                  selectedFile === s.file
                    ? "bg-accent/10 border-l-2 border-accent"
                    : "border-l-2 border-transparent hover:bg-bg-hover"
                }`}
              >
                <FileText
                  size={14}
                  className={
                    selectedFile === s.file
                      ? "text-accent shrink-0"
                      : "text-text-muted shrink-0"
                  }
                />
                <div className="flex-1 min-w-0">
                  <div
                    className={`text-xs font-medium truncate ${
                      selectedFile === s.file
                        ? "text-text-primary"
                        : "text-text-secondary group-hover:text-text-primary"
                    }`}
                  >
                    {s.name}
                  </div>
                  <div className="flex items-center gap-2 mt-0.5">
                    <span className="text-[0.6rem] text-text-muted font-mono">
                      {s.file}
                    </span>
                    <span className="text-[0.6rem] text-text-muted flex items-center gap-0.5">
                      <Zap size={8} />
                      {s.steps} steps
                    </span>
                  </div>
                </div>
                <ChevronRight
                  size={12}
                  className={`shrink-0 transition-opacity ${
                    selectedFile === s.file
                      ? "opacity-60 text-accent"
                      : "opacity-0 group-hover:opacity-40 text-text-muted"
                  }`}
                />
              </button>
            ))
          )}
        </div>
      </aside>

      {/* Context menu */}
      {contextMenu && (
        <>
          <div
            className="fixed inset-0 z-40"
            onClick={() => setContextMenu(null)}
          />
          <div
            className="fixed z-50 bg-bg-surface border border-border rounded-lg shadow-xl shadow-black/40 py-1 min-w-[140px] animate-fade-in"
            style={{ left: contextMenu.x, top: contextMenu.y }}
          >
            <button
              className="w-full text-left px-3 py-1.5 text-xs text-text-secondary hover:text-text-primary hover:bg-bg-hover flex items-center gap-2 transition-colors"
              onClick={() => {
                onDuplicate(contextMenu.file);
                setContextMenu(null);
              }}
            >
              <Copy size={12} /> Duplicate
            </button>
            <div className="border-t border-border my-1" />
            <button
              className="w-full text-left px-3 py-1.5 text-xs text-error hover:bg-error/10 flex items-center gap-2 transition-colors"
              onClick={() => {
                onDelete(contextMenu.file);
                setContextMenu(null);
              }}
            >
              <Trash2 size={12} /> Delete
            </button>
          </div>
        </>
      )}
    </>
  );
}
