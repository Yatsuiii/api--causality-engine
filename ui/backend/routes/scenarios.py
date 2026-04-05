"""Scenario CRUD routes — list, read, create, update, delete YAML files."""

import logging
from fastapi import APIRouter, HTTPException
from pathlib import Path
from pydantic import BaseModel, ValidationError
from typing import Optional
import yaml

from models import Scenario
from services.storage import scenarios_dir

logger = logging.getLogger(__name__)


# ── Request models ──────────────────────────────────────────────────


class CreateScenarioRequest(BaseModel):
    name: str = "untitled"
    scenario: Optional[dict] = None
    initial_state: str = "start"
    steps: list = []


class UpdateScenarioRequest(BaseModel):
    scenario: Optional[dict] = None
    name: Optional[str] = None
    initial_state: Optional[str] = None
    steps: Optional[list] = None


class RawYamlRequest(BaseModel):
    content: str

router = APIRouter()


def _validate_scenario_dict(data: dict) -> None:
    """Validate a parsed YAML dict against the Scenario Pydantic model."""
    try:
        Scenario(**data)
    except ValidationError as e:
        errors = "; ".join(err["msg"] for err in e.errors()[:3])
        raise HTTPException(422, f"Scenario schema validation failed: {errors}")


def _sanitize_name(name: str) -> str:
    """Strip path separators and null bytes to prevent path traversal."""
    name = name.removesuffix(".yaml").removesuffix(".yml")
    # Remove any path component — keep only the final segment, strip dangerous chars
    name = Path(name).name.replace("\x00", "")
    if not name or name in (".", ".."):
        raise HTTPException(400, "Invalid scenario name")
    return name


def _scenario_path(name: str) -> Path:
    """Resolve a scenario filename (with or without .yaml) to its path."""
    name = _sanitize_name(name)
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
        logger.warning("Failed to parse scenario %s: %s", name, e)
        raise HTTPException(400, "Failed to parse YAML")


# ── Raw YAML ─────────────────────────────────────────────────────────

@router.get("/{name}/raw")
def get_scenario_raw(name: str):
    """Get raw YAML text of a scenario."""
    p = _scenario_path(name)
    if not p.exists():
        raise HTTPException(404, f"Scenario '{name}' not found")
    return {"file": p.name, "content": p.read_text(encoding="utf-8")}


@router.put("/{name}/raw")
def update_scenario_raw(name: str, body: RawYamlRequest):
    """Save raw YAML text."""
    p = _scenario_path(name)
    try:
        parsed = yaml.safe_load(body.content)
    except Exception as e:
        logger.warning("Invalid YAML for %s: %s", name, e)
        raise HTTPException(400, "Invalid YAML syntax")
    if isinstance(parsed, dict):
        _validate_scenario_dict(parsed)
    p.write_text(body.content, encoding="utf-8")
    return {"file": p.name, "status": "saved"}


# ── Create ───────────────────────────────────────────────────────────

@router.post("")
def create_scenario(body: CreateScenarioRequest):
    """Create a new scenario from JSON body, write as YAML."""
    sanitized = _sanitize_name(body.name.lower().replace(" ", "_"))
    filename = sanitized + ".yaml"
    p = scenarios_dir() / filename

    scenario_data = body.scenario if body.scenario is not None else {}
    if "name" not in scenario_data:
        scenario_data["name"] = body.name
    if "initial_state" not in scenario_data:
        scenario_data["initial_state"] = body.initial_state
    if "steps" not in scenario_data:
        scenario_data["steps"] = body.steps

    _validate_scenario_dict(scenario_data)
    yaml_content = yaml.dump(scenario_data, default_flow_style=False, sort_keys=False)
    try:
        with open(p, "x", encoding="utf-8") as f:
            f.write(yaml_content)
    except FileExistsError:
        raise HTTPException(409, f"Scenario '{filename}' already exists")
    return {"file": filename, "status": "created"}


# ── Update ───────────────────────────────────────────────────────────

@router.put("/{name}")
def update_scenario(name: str, body: UpdateScenarioRequest):
    """Update an existing scenario from JSON body."""
    p = _scenario_path(name)
    if not p.exists():
        raise HTTPException(404, f"Scenario '{name}' not found")

    scenario_data = body.scenario if body.scenario is not None else body.model_dump(exclude_none=True)
    _validate_scenario_dict(scenario_data)
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

    # Atomically claim a unique name using exclusive create
    base = p.stem
    original_name = raw.get("name", base)
    max_attempts = 100
    for i in range(1, max_attempts + 1):
        new_name = f"{base}_copy{i}"
        new_path = scenarios_dir() / f"{new_name}.yaml"
        raw["name"] = f"{original_name} (copy {i})"
        new_content = yaml.dump(raw, default_flow_style=False, sort_keys=False)
        try:
            with open(new_path, "x", encoding="utf-8") as f:
                f.write(new_content)
            return {"file": new_path.name, "status": "duplicated", "original": p.name}
        except FileExistsError:
            continue

    raise HTTPException(409, f"Could not find a unique copy name after {max_attempts} attempts")
