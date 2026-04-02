"""Runner routes — execute and validate scenarios via ace CLI."""

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from typing import Optional
from pathlib import Path

from services.executor import run_scenario, find_ace_binary
from services.storage import scenarios_dir

router = APIRouter()


class RunRequest(BaseModel):
    scenario_file: str
    environment: Optional[str] = None
    variables: Optional[dict[str, str]] = None


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
    import subprocess
    from services.storage import get_workspace_dir

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
    """Resolve scenario name to full path in scenarios dir."""
    d = scenarios_dir()
    # If it's already an absolute path, use it
    p = Path(name)
    if p.is_absolute() and p.exists():
        return str(p)
    # Try relative to scenarios dir
    for ext in ("", ".yaml", ".yml"):
        candidate = d / f"{name}{ext}"
        if candidate.exists():
            return str(candidate)
    # Return as-is (will fail later with a 404)
    return str(d / name)
