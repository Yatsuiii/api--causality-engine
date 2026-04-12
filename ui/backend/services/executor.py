"""Subprocess runner for ace CLI binary."""

import json
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from uuid import uuid4

import re

import yaml

from models import AssertionResult, ExecutionLog, HistoryEntry, StepLog
from services.storage import get_environment, get_workspace_dir, save_history_entry


def _read_scenario_name(scenario_path: str) -> str:
    """Read the `name` field from a scenario YAML, falling back to the stem."""
    try:
        raw = yaml.safe_load(Path(scenario_path).read_text(encoding="utf-8"))
        if isinstance(raw, dict) and isinstance(raw.get("name"), str):
            return raw["name"]
    except Exception:
        pass
    return re.sub(r"[-_]+", " ", Path(scenario_path).stem).strip().title()


def find_ace_binary() -> str:
    """Locate the ace CLI binary."""
    workspace = get_workspace_dir()
    # Check common locations
    candidates = [
        workspace / "target" / "release" / "ace.exe",
        workspace / "target" / "release" / "ace",
        workspace / "target" / "debug" / "ace.exe",
        workspace / "target" / "debug" / "ace",
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    # Fallback: assume it's on PATH
    return "ace"


def run_scenario(
    scenario_path: str,
    environment: str | None = None,
    variables: dict[str, str] | None = None,
) -> HistoryEntry:
    """Execute a scenario via the ace CLI and return results."""
    ace = find_ace_binary()
    run_id = uuid4().hex[:8]
    output_fd = tempfile.NamedTemporaryFile(
        suffix=".json", prefix=f"ace_run_{run_id}_", delete=False,
    )
    output_file = output_fd.name
    output_fd.close()

    cmd = [ace, "run", scenario_path, "-o", output_file, "-v"]

    if variables:
        for k, v in variables.items():
            cmd.extend(["--var", f"{k}={v}"])

    # Load environment variables if specified
    if environment:
        env = get_environment(environment)
        if env:
            for k, v in env.variables.items():
                cmd.extend(["--var", f"{k}={v}"])

    started_at = datetime.now(timezone.utc).isoformat()

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=120,
            cwd=str(get_workspace_dir()),
        )
    except FileNotFoundError:
        return _make_error_entry(
            run_id, scenario_path, environment, started_at,
            "ace binary not found. Build with: cargo build --release"
        )
    except subprocess.TimeoutExpired:
        return _make_error_entry(
            run_id, scenario_path, environment, started_at,
            "Execution timed out (120s)"
        )

    # Parse output log
    log = ExecutionLog()
    output_path = Path(output_file)
    if output_path.exists():
        log_parse_error: str | None = None
        try:
            raw = json.loads(output_path.read_text(encoding="utf-8"))
            log = ExecutionLog(**raw)
        except json.JSONDecodeError as e:
            log_parse_error = f"Failed to parse output JSON: {e}"
        except Exception as e:
            log_parse_error = f"Failed to load execution log: {e}"
        finally:
            output_path.unlink(missing_ok=True)

        if log_parse_error:
            return _make_error_entry(
                run_id, scenario_path, environment, started_at,
                log_parse_error,
            )

    # If no log was produced, create one from stdout/stderr
    if not log.steps and result.returncode != 0:
        return _make_error_entry(
            run_id, scenario_path, environment, started_at,
            result.stderr or result.stdout or f"Exit code {result.returncode}"
        )

    scenario_name = _read_scenario_name(scenario_path)
    entry = HistoryEntry(
        id=run_id,
        scenario_name=scenario_name,
        scenario_file=scenario_path,
        environment=environment,
        started_at=started_at,
        duration_ms=log.total_duration_ms,
        total_steps=log.total_steps,
        passed=log.passed,
        failed=log.failed,
        log=log,
    )
    save_history_entry(entry)
    return entry


def _make_error_entry(
    run_id: str, scenario_path: str, environment: str | None,
    started_at: str, error: str,
) -> HistoryEntry:
    entry = HistoryEntry(
        id=run_id,
        scenario_name=_read_scenario_name(scenario_path),
        scenario_file=scenario_path,
        environment=environment,
        started_at=started_at,
        duration_ms=0,
        total_steps=0,
        passed=0,
        failed=1,
        log=ExecutionLog(
            steps=[StepLog(
                step_name="error",
                method="",
                url="",
                status=0,
                duration_ms=0,
                assertions=[AssertionResult(
                    description=error,
                    passed=False,
                    expected="success",
                    actual="error",
                )],
            )],
            total_steps=0,
            passed=0,
            failed=1,
        ),
    )
    save_history_entry(entry)
    return entry
