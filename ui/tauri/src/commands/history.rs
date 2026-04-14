//! History commands — mirrors `ui/backend/routes/history.py`.

use tauri::State;

use crate::storage::{self, HistoryEntry, WorkspaceState};

#[tauri::command]
#[specta::specta]
pub fn list_history(
    state: State<'_, WorkspaceState>,
    limit: Option<usize>,
) -> Result<Vec<HistoryEntry>, String> {
    storage::list_history(&state, limit.unwrap_or(50))
}

#[tauri::command]
#[specta::specta]
pub fn get_history_entry(
    state: State<'_, WorkspaceState>,
    id: String,
) -> Result<HistoryEntry, String> {
    storage::get_history_entry(&state, &id)?
        .ok_or_else(|| format!("History entry '{}' not found", id))
}

#[tauri::command]
#[specta::specta]
pub fn delete_history_entry(
    state: State<'_, WorkspaceState>,
    id: String,
) -> Result<(), String> {
    if !storage::delete_history_entry(&state, &id)? {
        return Err(format!("History entry '{}' not found", id));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn clear_history(state: State<'_, WorkspaceState>) -> Result<usize, String> {
    storage::clear_history(&state)
}
