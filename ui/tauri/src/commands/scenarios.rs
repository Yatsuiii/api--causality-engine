//! Scenario CRUD commands — mirrors `ui/backend/routes/scenarios.py`.

use std::{fs, io::Write as _};

use serde::Serialize;
use serde_json::Value;
use specta::Type;
use tauri::State;

use crate::storage::{self, WorkspaceState};

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Type)]
pub struct ScenarioSummary {
    pub file: String,
    pub name: String,
    pub steps: usize,
    pub initial_state: String,
    pub concurrency: Option<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Type)]
pub struct ScenarioFile {
    pub file: String,
    pub scenario: Value,
}

#[derive(Debug, Serialize, Type)]
pub struct RawScenario {
    pub file: String,
    pub content: String,
}

#[derive(Debug, Serialize, Type)]
pub struct DuplicatedScenario {
    pub file: String,
    pub original: String,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_summary(path: &std::path::Path) -> ScenarioSummary {
    let file = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    match fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_yaml::from_str::<Value>(&t).ok())
    {
        Some(raw) => {
            let name = raw
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&stem)
                .to_string();
            let steps = raw
                .get("steps")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let initial_state = raw
                .get("initial_state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let concurrency = raw
                .get("concurrency")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            ScenarioSummary {
                file,
                name,
                steps,
                initial_state,
                concurrency,
                error: None,
            }
        }
        None => ScenarioSummary {
            file,
            name: stem,
            steps: 0,
            initial_state: String::new(),
            concurrency: None,
            error: Some("Failed to parse".to_string()),
        },
    }
}

fn validate_scenario_value(data: &Value) -> Result<(), String> {
    serde_json::from_value::<model::Scenario>(data.clone())
        .map(|_| ())
        .map_err(|e| format!("Scenario validation failed: {}", e))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
#[specta::specta]
pub fn list_scenarios(state: State<'_, WorkspaceState>) -> Vec<ScenarioSummary> {
    storage::list_scenario_files(&state)
        .iter()
        .map(|p| parse_summary(p))
        .collect()
}

#[tauri::command]
#[specta::specta]
pub fn get_scenario(
    state: State<'_, WorkspaceState>,
    name: String,
) -> Result<ScenarioFile, String> {
    let p = storage::scenario_path(&state, &name)?;
    if !p.exists() {
        return Err(format!("Scenario '{}' not found", name));
    }
    let text = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let scenario: Value =
        serde_yaml::from_str(&text).map_err(|e| format!("Failed to parse YAML: {}", e))?;
    let file = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(ScenarioFile { file, scenario })
}

#[tauri::command]
#[specta::specta]
pub fn get_scenario_raw(
    state: State<'_, WorkspaceState>,
    name: String,
) -> Result<RawScenario, String> {
    let p = storage::scenario_path(&state, &name)?;
    if !p.exists() {
        return Err(format!("Scenario '{}' not found", name));
    }
    let content = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let file = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(RawScenario { file, content })
}

/// Write raw YAML text back to disk. Validates YAML syntax before saving.
#[tauri::command]
#[specta::specta]
pub fn update_scenario_raw(
    state: State<'_, WorkspaceState>,
    name: String,
    content: String,
) -> Result<String, String> {
    let p = storage::scenario_path(&state, &name)?;
    // Reject malformed YAML before clobbering the file.
    serde_yaml::from_str::<Value>(&content)
        .map_err(|e| format!("Invalid YAML: {}", e))?;
    fs::write(&p, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(p.file_name().unwrap_or_default().to_string_lossy().into_owned())
}

#[tauri::command]
#[specta::specta]
pub fn create_scenario(
    state: State<'_, WorkspaceState>,
    name: String,
    scenario: Option<Value>,
    initial_state: Option<String>,
    steps: Option<Value>,
) -> Result<String, String> {
    let sanitized = storage::sanitize_name(&name.to_lowercase().replace(' ', "_"))?;
    let filename = format!("{}.yaml", sanitized);
    let p = storage::scenarios_dir(&state).join(&filename);

    if p.exists() {
        return Err(format!("Scenario '{}' already exists", filename));
    }

    // Build the scenario data, filling in defaults where missing.
    let mut data = match scenario {
        Some(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    if !data.contains_key("name") {
        data.insert("name".into(), Value::String(name));
    }
    if !data.contains_key("initial_state") {
        data.insert(
            "initial_state".into(),
            Value::String(initial_state.unwrap_or_else(|| "start".to_string())),
        );
    }
    if !data.contains_key("steps") {
        data.insert(
            "steps".into(),
            steps.unwrap_or(Value::Array(vec![])),
        );
    }

    validate_scenario_value(&Value::Object(data.clone()))?;

    let yaml = serde_yaml::to_string(&data).map_err(|e| e.to_string())?;

    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!("Scenario '{}' already exists", filename)
            } else {
                e.to_string()
            }
        })?;
    f.write_all(yaml.as_bytes()).map_err(|e| e.to_string())?;

    Ok(filename)
}

#[tauri::command]
#[specta::specta]
pub fn update_scenario(
    state: State<'_, WorkspaceState>,
    name: String,
    scenario: Value,
) -> Result<String, String> {
    let p = storage::scenario_path(&state, &name)?;
    if !p.exists() {
        return Err(format!("Scenario '{}' not found", name));
    }
    validate_scenario_value(&scenario)?;
    let yaml = serde_yaml::to_string(&scenario).map_err(|e| e.to_string())?;
    fs::write(&p, yaml.as_bytes()).map_err(|e| e.to_string())?;
    Ok(p.file_name().unwrap_or_default().to_string_lossy().into_owned())
}

#[tauri::command]
#[specta::specta]
pub fn delete_scenario(
    state: State<'_, WorkspaceState>,
    name: String,
) -> Result<String, String> {
    let p = storage::scenario_path(&state, &name)?;
    if !p.exists() {
        return Err(format!("Scenario '{}' not found", name));
    }
    let file = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    fs::remove_file(&p).map_err(|e| e.to_string())?;
    Ok(file)
}

#[tauri::command]
#[specta::specta]
pub fn duplicate_scenario(
    state: State<'_, WorkspaceState>,
    name: String,
) -> Result<DuplicatedScenario, String> {
    let src = storage::scenario_path(&state, &name)?;
    if !src.exists() {
        return Err(format!("Scenario '{}' not found", name));
    }
    let original = src.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let dest = storage::duplicate_scenario_file(&state, &name)?;
    let file = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(DuplicatedScenario {
        file,
        original,
        status: "duplicated".to_string(),
    })
}
