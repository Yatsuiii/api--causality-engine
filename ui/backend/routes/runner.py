"""Runner routes — execute and validate scenarios via ace CLI."""

import subprocess
from pathlib import Path

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from services.executor import find_ace_binary, run_scenario
from services.storage import get_workspace_dir, scenarios_dir

router = APIRouter()


class RunRequest(BaseModel):
    scenario_file: str
    environment: str | None = None
    variables: dict[str, str] | None = None


class ValidateRequest(BaseModel):
    scenario_file: str


# ── Run ──────────────────────────────────────────────────────────────

@router.post("/run")
def run(req: RunRequest):
    """Execute a scenario and return results."""
    # Resolve scenario path
    scenario_path = _resolve_scenario(req.scenario_file)
    if not Path(scenario_path).exists():
        raise HTTPException(404, f"Scenario file not found: {req.scenario_file}")

    entry = run_scenario(
        scenario_path=scenario_path,
        environment=req.environment,
        variables=req.variables,
    )
    return entry.model_dump()


# ── Validate ─────────────────────────────────────────────────────────

@router.post("/validate")
def validate(req: ValidateRequest):
    """Validate a scenario YAML via ace CLI 'validate' subcommand."""
    scenario_path = _resolve_scenario(req.scenario_file)
    if not Path(scenario_path).exists():
        raise HTTPException(404, f"Scenario file not found: {req.scenario_file}")

    ace = find_ace_binary()
    try:
        result = subprocess.run(
            [ace, "validate", scenario_path],
            capture_output=True,
            text=True,
            timeout=30,
            cwd=str(get_workspace_dir()),
        )
        return {
            "valid": result.returncode == 0,
            "stdout": result.stdout,
            "stderr": result.stderr,
            "exit_code": result.returncode,
        }
    except FileNotFoundError:
        raise HTTPException(500, "ace binary not found. Build with: cargo build --release")
    except subprocess.TimeoutExpired:
        raise HTTPException(504, "Validation timed out")


def _resolve_scenario(name: str) -> str:
    """Resolve scenario name to full path within the scenarios dir.

    Rejects absolute paths and traversal sequences to prevent
    arbitrary file access outside the scenarios directory.
    """
    d = scenarios_dir().resolve()

    # Try relative to scenarios dir
    for ext in ("", ".yaml", ".yml"):
        candidate = (d / f"{name}{ext}").resolve()
        if candidate.is_relative_to(d) and candidate.exists():
            return str(candidate)

    # Return a safe default (will fail later with a 404)
    safe_name = Path(name).name  # strip any directory components
    return str(d / safe_name)
