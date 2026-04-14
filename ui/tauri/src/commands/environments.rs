//! Environment commands — mirrors `ui/backend/routes/environments.py`.

use std::collections::HashMap;

use tauri::State;

use crate::storage::{self, Environment, WorkspaceState};

#[tauri::command]
#[specta::specta]
pub fn list_environments(state: State<'_, WorkspaceState>) -> Result<Vec<Environment>, String> {
    storage::list_environments(&state)
}

#[tauri::command]
#[specta::specta]
pub fn get_environment(
    state: State<'_, WorkspaceState>,
    name: String,
) -> Result<Environment, String> {
    storage::get_environment(&state, &name)?
        .ok_or_else(|| format!("Environment '{}' not found", name))
}

#[tauri::command]
#[specta::specta]
pub fn create_environment(
    state: State<'_, WorkspaceState>,
    name: String,
    variables: HashMap<String, String>,
) -> Result<Environment, String> {
    if storage::get_environment(&state, &name)?.is_some() {
        return Err(format!("Environment '{}' already exists", name));
    }
    let env = Environment { name, variables };
    storage::save_environment(&state, &env)?;
    Ok(env)
}

#[tauri::command]
#[specta::specta]
pub fn update_environment(
    state: State<'_, WorkspaceState>,
    name: String,
    variables: HashMap<String, String>,
) -> Result<Environment, String> {
    let existing = storage::get_environment(&state, &name)?
        .ok_or_else(|| format!("Environment '{}' not found", name))?;
    let updated = Environment {
        name: existing.name,
        variables,
    };
    storage::save_environment(&state, &updated)?;
    Ok(updated)
}

#[tauri::command]
#[specta::specta]
pub fn delete_environment(state: State<'_, WorkspaceState>, name: String) -> Result<(), String> {
    if !storage::delete_environment(&state, &name)? {
        return Err(format!("Environment '{}' not found", name));
    }
    Ok(())
}
