"""Scenario CRUD routes — list, read, create, update, delete YAML files."""

from fastapi import APIRouter, HTTPException
from pathlib import Path
import yaml

from models import Scenario
from services.storage import scenarios_dir

router = APIRouter()


def _scenario_path(name: str) -> Path:
    """Resolve a scenario filename (with or without .yaml) to its path."""
    name = name.removesuffix(".yaml").removesuffix(".yml")
    d = scenarios_dir()
    for ext in (".yaml", ".yml"):
        p = d / f"{name}{ext}"
        if p.exists():
            return p
    return d / f"{name}.yaml"


def _list_files() -> list[Path]:
    d = scenarios_dir()
    if not d.exists():
        return []
    files = sorted(d.glob("*.yaml")) + sorted(d.glob("*.yml"))
    return files


# ── List ─────────────────────────────────────────────────────────────

@router.get("")
def list_scenarios():
    """List all scenario YAML files in the workspace."""
    results = []
    for f in _list_files():
        try:
            raw = yaml.safe_load(f.read_text(encoding="utf-8"))
            results.append({
                "file": f.name,
                "name": raw.get("name", f.stem),
                "steps": len(raw.get("steps", [])),
                "initial_state": raw.get("initial_state", ""),
                "concurrency": raw.get("concurrency"),
            })
        except Exception:
            results.append({
                "file": f.name,
                "name": f.stem,
                "steps": 0,
                "initial_state": "",
                "concurrency": None,
                "error": "Failed to parse",
            })
    return results


# ── Read ─────────────────────────────────────────────────────────────

@router.get("/{name}")
def get_scenario(name: str):
    """Read and parse a single scenario, return as JSON."""
    p = _scenario_path(name)
    if not p.exists():
        raise HTTPException(404, f"Scenario '{name}' not found")
    try:
        raw = yaml.safe_load(p.read_text(encoding="utf-8"))
        return {"file": p.name, "scenario": raw}
    except Exception as e:
        raise HTTPException(400, f"Failed to parse YAML: {e}")


# ── Raw YAML ─────────────────────────────────────────────────────────

@router.get("/{name}/raw")
def get_scenario_raw(name: str):
    """Get raw YAML text of a scenario."""
    p = _scenario_path(name)
    if not p.exists():
        raise HTTPException(404, f"Scenario '{name}' not found")
    return {"file": p.name, "content": p.read_text(encoding="utf-8")}


@router.put("/{name}/raw")
def update_scenario_raw(name: str, body: dict):
    """Save raw YAML text."""
    p = _scenario_path(name)
    content = body.get("content", "")
    # Validate it's parseable YAML
    try:
        yaml.safe_load(content)
    except Exception as e:
        raise HTTPException(400, f"Invalid YAML: {e}")
    p.write_text(content, encoding="utf-8")
    return {"file": p.name, "status": "saved"}


# ── Create ───────────────────────────────────────────────────────────

@router.post("")
def create_scenario(body: dict):
    """Create a new scenario from JSON body, write as YAML."""
    name = body.get("name", "untitled")
    filename = name.lower().replace(" ", "_") + ".yaml"
    p = scenarios_dir() / filename
    if p.exists():
        raise HTTPException(409, f"Scenario '{filename}' already exists")

    scenario_data = body.get("scenario", body)
    # Ensure it has required fields
    if "name" not in scenario_data:
        scenario_data["name"] = name
    if "initial_state" not in scenario_data:
        scenario_data["initial_state"] = "start"
    if "steps" not in scenario_data:
        scenario_data["steps"] = []

    yaml_content = yaml.dump(scenario_data, default_flow_style=False, sort_keys=False)
    p.write_text(yaml_content, encoding="utf-8")
    return {"file": filename, "status": "created"}


# ── Update ───────────────────────────────────────────────────────────

@router.put("/{name}")
def update_scenario(name: str, body: dict):
    """Update an existing scenario from JSON body."""
    p = _scenario_path(name)
    if not p.exists():
        raise HTTPException(404, f"Scenario '{name}' not found")

    scenario_data = body.get("scenario", body)
    yaml_content = yaml.dump(scenario_data, default_flow_style=False, sort_keys=False)
    p.write_text(yaml_content, encoding="utf-8")
    return {"file": p.name, "status": "updated"}


# ── Delete ───────────────────────────────────────────────────────────

@router.delete("/{name}")
def delete_scenario(name: str):
    """Delete a scenario YAML file."""
    p = _scenario_path(name)
    if not p.exists():
        raise HTTPException(404, f"Scenario '{name}' not found")
    p.unlink()
    return {"file": p.name, "status": "deleted"}


# ── Duplicate ────────────────────────────────────────────────────────

@router.post("/{name}/duplicate")
def duplicate_scenario(name: str):
    """Duplicate a scenario file."""
    p = _scenario_path(name)
    if not p.exists():
        raise HTTPException(404, f"Scenario '{name}' not found")

    content = p.read_text(encoding="utf-8")
    raw = yaml.safe_load(content)

    # Find a unique name
    base = p.stem
    i = 1
    while True:
        new_name = f"{base}_copy{i}"
        new_path = scenarios_dir() / f"{new_name}.yaml"
        if not new_path.exists():
            break
        i += 1

    raw["name"] = raw.get("name", base) + f" (copy {i})"
    new_content = yaml.dump(raw, default_flow_style=False, sort_keys=False)
    new_path.write_text(new_content, encoding="utf-8")
    return {"file": new_path.name, "status": "duplicated", "original": p.name}
