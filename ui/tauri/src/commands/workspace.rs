//! Workspace commands — get and set the active workspace directory.

use tauri::State;

use crate::storage::{self, WorkspaceState};

/// Return the absolute path of the current workspace directory.
#[tauri::command]
#[specta::specta]
pub fn get_workspace(state: State<'_, WorkspaceState>) -> String {
    storage::get_workspace_dir(&state)
        .to_string_lossy()
        .into_owned()
}

/// Persist a new workspace directory; errors if the path does not exist.
#[tauri::command]
#[specta::specta]
pub fn set_workspace(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    path: String,
) -> Result<String, String> {
    storage::set_workspace_dir(&state, &app, &path)?;
    Ok(storage::get_workspace_dir(&state)
        .to_string_lossy()
        .into_owned())
}
