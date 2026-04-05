"""Subprocess runner for ace CLI binary."""

import subprocess
import json
import tempfile
import uuid
from datetime import datetime, timezone
from pathlib import Path
from models import ExecutionLog, HistoryEntry
from services.storage import save_history_entry, get_workspace_dir


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
    verbose: bool = True,
) -> HistoryEntry:
    """Execute a scenario via the ace CLI and return results."""
    ace = find_ace_binary()
    run_id = str(uuid.uuid4())[:8]
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
    env_vars = {}
    if environment:
        from services.storage import get_environment
        env = get_environment(environment)
        if env:
            env_vars = env.variables
            for k, v in env_vars.items():
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
        try:
            raw = json.loads(output_path.read_text(encoding="utf-8"))
            log = ExecutionLog(**raw)
        except json.JSONDecodeError as e:
            log_parse_error = f"Failed to parse output JSON: {e}"
        except Exception as e:
            log_parse_error = f"Failed to load execution log: {e}"
        else:
            log_parse_error = None
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

    scenario_name = Path(scenario_path).stem.replace("_", " ")
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
    from models import StepLog, AssertionResult
    entry = HistoryEntry(
        id=run_id,
        scenario_name=Path(scenario_path).stem.replace("_", " "),
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
