//! Runner commands — replaces the subprocess+tempfile dance in
//! `ui/backend/routes/runner.py` with a direct in-process call to the runner crate.

use std::{collections::HashMap, fs};

use chrono::Utc;
use serde::Serialize;
use specta::Type;
use tauri::State;
use tracing::info;
use uuid::Uuid;

use crate::storage::{self, HistoryEntry, WorkspaceState};

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Type)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Execute a scenario in-process and persist a HistoryEntry on completion.
///
/// Variables are merged with the precedence: scenario defaults → env vars → explicit vars.
#[tauri::command]
#[specta::specta]
pub async fn run_scenario(
    state: State<'_, WorkspaceState>,
    scenario_file: String,
    environment: Option<String>,
    variables: Option<HashMap<String, String>>,
) -> Result<HistoryEntry, String> {
    info!(scenario_file = %scenario_file, "run_scenario: starting");

    let path = storage::scenario_path(&state, &scenario_file)?;
    if !path.exists() {
        return Err(format!("Scenario file not found: {}", scenario_file));
    }

    let yaml = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut scenario = model::load_scenario(&yaml).map_err(|e| e.to_string())?;

    // Look up env variables now, before the async boundary.
    let env_vars: HashMap<String, String> = environment
        .as_deref()
        .map(|n| storage::get_environment(&state, n))
        .transpose()?
        .flatten()
        .map(|e| e.variables)
        .unwrap_or_default();

    // Merge into scenario.variables: env_vars override scenario defaults, explicit override env.
    {
        let vars = scenario.variables.get_or_insert_with(HashMap::new);
        vars.extend(env_vars);
        if let Some(explicit) = variables {
            vars.extend(explicit);
        }
    }

    let config = runner::RunConfig::default();
    let started = Utc::now();

    let results = runner::run(&scenario, &config).await;

    let (log, _outcome) = results
        .into_iter()
        .next()
        .unwrap_or_else(|| (runner::ExecutionLog::default(), Ok(String::new())));

    let scenario_name = scenario.name.clone();
    let scenario_file_str = path.to_string_lossy().into_owned();

    let entry = HistoryEntry {
        id: Uuid::new_v4().simple().to_string()[..8].to_string(),
        scenario_name,
        scenario_file: scenario_file_str,
        environment,
        started_at: started.to_rfc3339(),
        duration_ms: log.total_duration_ms,
        total_steps: log.total_steps,
        passed: log.passed,
        failed: log.failed,
        log,
    };

    storage::save_history_entry(&state, &entry)?;

    info!(id = %entry.id, passed = entry.passed, failed = entry.failed, "run_scenario: done");
    Ok(entry)
}

/// Validate a scenario YAML file without executing it.
#[tauri::command]
#[specta::specta]
pub fn validate_scenario(
    state: State<'_, WorkspaceState>,
    scenario_file: String,
) -> Result<ValidationResult, String> {
    let path = storage::scenario_path(&state, &scenario_file)?;
    if !path.exists() {
        return Err(format!("Scenario file not found: {}", scenario_file));
    }
    let yaml = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    match model::load_scenario(&yaml) {
        Ok(scenario) => {
            let errors = ace_core::validate::validate_scenario(&scenario);
            Ok(ValidationResult {
                valid: errors.is_empty(),
                errors,
            })
        }
        Err(e) => Ok(ValidationResult {
            valid: false,
            errors: vec![e.to_string()],
        }),
    }
}
