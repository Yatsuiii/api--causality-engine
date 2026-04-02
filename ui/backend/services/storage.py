"""File-based storage for workspace, environments, and history."""

from pathlib import Path
import json
from models import Environment, HistoryEntry

_workspace_dir: Path = Path.cwd().parent.parent  # default: project root


def get_workspace_dir() -> Path:
    return _workspace_dir


def set_workspace_dir(path: str) -> None:
    global _workspace_dir
    _workspace_dir = Path(path)


def scenarios_dir() -> Path:
    return get_workspace_dir() / "examples"


def environments_dir() -> Path:
    d = get_workspace_dir() / ".ace" / "environments"
    d.mkdir(parents=True, exist_ok=True)
    return d


def history_dir() -> Path:
    d = get_workspace_dir() / ".ace" / "history"
    d.mkdir(parents=True, exist_ok=True)
    return d


# ── Environments ─────────────────────────────────────────────────────

def list_environments() -> list[Environment]:
    envs = []
    d = environments_dir()
    for f in sorted(d.glob("*.json")):
        data = json.loads(f.read_text(encoding="utf-8"))
        envs.append(Environment(**data))
    return envs


def get_environment(name: str) -> Environment | None:
    f = environments_dir() / f"{name}.json"
    if not f.exists():
        return None
    return Environment(**json.loads(f.read_text(encoding="utf-8")))


def save_environment(env: Environment) -> None:
    f = environments_dir() / f"{env.name}.json"
    f.write_text(json.dumps(env.model_dump(), indent=2), encoding="utf-8")


def delete_environment(name: str) -> bool:
    f = environments_dir() / f"{name}.json"
    if f.exists():
        f.unlink()
        return True
    return False


# ── History ──────────────────────────────────────────────────────────

def list_history(limit: int = 50) -> list[HistoryEntry]:
    entries = []
    d = history_dir()
    files = sorted(d.glob("*.json"), key=lambda p: p.stat().st_mtime, reverse=True)
    for f in files[:limit]:
        data = json.loads(f.read_text(encoding="utf-8"))
        entries.append(HistoryEntry(**data))
    return entries


def get_history_entry(entry_id: str) -> HistoryEntry | None:
    f = history_dir() / f"{entry_id}.json"
    if not f.exists():
        return None
    return HistoryEntry(**json.loads(f.read_text(encoding="utf-8")))


def save_history_entry(entry: HistoryEntry) -> None:
    f = history_dir() / f"{entry.id}.json"
    f.write_text(json.dumps(entry.model_dump(), indent=2), encoding="utf-8")


def delete_history_entry(entry_id: str) -> bool:
    f = history_dir() / f"{entry_id}.json"
    if f.exists():
        f.unlink()
        return True
    return False
