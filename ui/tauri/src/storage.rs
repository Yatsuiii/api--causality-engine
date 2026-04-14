//! File-based storage for workspace config, environments, and history.
//!
//! Mirrors `ui/backend/services/storage.py` 1-for-1.  Path-safety helpers
//! from `ui/backend/routes/scenarios.py` lines 44–62 live here so all
//! command modules can share them.

use std::{
    collections::HashMap,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};

// ---------------------------------------------------------------------------
// Workspace state
// ---------------------------------------------------------------------------

/// Tauri managed state — holds the active workspace directory.
pub struct WorkspaceState(pub Mutex<PathBuf>);

#[derive(Serialize, Deserialize)]
struct WorkspaceConfig {
    path: PathBuf,
}

fn workspace_config_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .expect("app config dir unavailable")
        .join("workspace.json")
}

/// Load the persisted workspace dir from disk, falling back to cwd.
pub fn load_workspace(app: &AppHandle) -> PathBuf {
    let p = workspace_config_path(app);
    if p.exists() {
        if let Ok(text) = fs::read_to_string(&p) {
            if let Ok(cfg) = serde_json::from_str::<WorkspaceConfig>(&text) {
                if cfg.path.is_dir() {
                    return cfg.path;
                }
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn get_workspace_dir(state: &WorkspaceState) -> PathBuf {
    state.0.lock().unwrap().clone()
}

pub fn set_workspace_dir(
    state: &WorkspaceState,
    app: &AppHandle,
    path: &str,
) -> Result<(), String> {
    let p = PathBuf::from(path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !p.is_dir() {
        return Err(format!(
            "Path does not exist or is not a directory: {}",
            p.display()
        ));
    }
    *state.0.lock().unwrap() = p.clone();
    let cfg_path = workspace_config_path(app);
    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        cfg_path,
        serde_json::to_string_pretty(&WorkspaceConfig { path: p }).unwrap(),
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Standard sub-directories
// ---------------------------------------------------------------------------

pub fn scenarios_dir(state: &WorkspaceState) -> PathBuf {
    get_workspace_dir(state).join("examples")
}

pub fn environments_dir(state: &WorkspaceState) -> PathBuf {
    let d = get_workspace_dir(state).join(".ace").join("environments");
    fs::create_dir_all(&d).ok();
    d
}

pub fn history_dir(state: &WorkspaceState) -> PathBuf {
    let d = get_workspace_dir(state).join(".ace").join("history");
    fs::create_dir_all(&d).ok();
    d
}

// ---------------------------------------------------------------------------
// Path-safety helpers  (ported from routes/scenarios.py lines 44–62)
// ---------------------------------------------------------------------------

/// Strip path separators and null bytes to prevent path traversal.
pub fn sanitize_name(name: &str) -> Result<String, String> {
    let name = name
        .strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"))
        .unwrap_or(name);

    let name = Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().replace('\x00', ""))
        .unwrap_or_default();

    if name.is_empty() || name == "." || name == ".." {
        return Err("Invalid scenario name".to_string());
    }
    Ok(name)
}

/// Resolve a scenario name to its `.yaml` / `.yml` path.
pub fn scenario_path(state: &WorkspaceState, name: &str) -> Result<PathBuf, String> {
    let name = sanitize_name(name)?;
    let d = scenarios_dir(state);
    for ext in [".yaml", ".yml"] {
        let p = d.join(format!("{name}{ext}"));
        if p.exists() {
            return Ok(p);
        }
    }
    Ok(d.join(format!("{name}.yaml")))
}

/// All scenario files sorted by name.
pub fn list_scenario_files(state: &WorkspaceState) -> Vec<PathBuf> {
    let d = scenarios_dir(state);
    if !d.exists() {
        return vec![];
    }
    let mut files: Vec<PathBuf> = d
        .read_dir()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("yaml") | Some("yml")
            )
        })
        .collect();
    files.sort();
    files
}

/// Atomic duplicate-naming retry loop (mirrors routes/scenarios.py lines 209–225).
pub fn duplicate_scenario_file(
    state: &WorkspaceState,
    name: &str,
) -> Result<PathBuf, String> {
    let src = scenario_path(state, name)?;
    if !src.exists() {
        return Err(format!("Scenario '{}' not found", name));
    }
    let content = fs::read_to_string(&src).map_err(|e| e.to_string())?;
    let mut raw: serde_json::Value =
        serde_yaml::from_str(&content).map_err(|e| e.to_string())?;

    let base = src
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let original_name = raw
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&base)
        .to_string();

    let d = scenarios_dir(state);
    for i in 1..=100_u32 {
        let new_path = d.join(format!("{base}_copy{i}.yaml"));
        if raw.is_object() {
            raw["name"] =
                serde_json::Value::String(format!("{original_name} (copy {i})"));
        }
        let new_content = serde_yaml::to_string(&raw).map_err(|e| e.to_string())?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_path)
        {
            Ok(mut f) => {
                f.write_all(new_content.as_bytes())
                    .map_err(|e| e.to_string())?;
                return Ok(new_path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Could not find a unique copy name after 100 attempts".to_string())
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Environment {
    pub name: String,
    pub variables: HashMap<String, String>,
}

pub fn list_environments(state: &WorkspaceState) -> Result<Vec<Environment>, String> {
    let d = environments_dir(state);
    let mut envs: Vec<Environment> = d
        .read_dir()
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| {
            fs::read_to_string(e.path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
        })
        .collect();
    envs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(envs)
}

pub fn get_environment(
    state: &WorkspaceState,
    name: &str,
) -> Result<Option<Environment>, String> {
    let f = environments_dir(state).join(format!("{name}.json"));
    if !f.exists() {
        return Ok(None);
    }
    serde_json::from_str(&fs::read_to_string(f).map_err(|e| e.to_string())?)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn save_environment(state: &WorkspaceState, env: &Environment) -> Result<(), String> {
    fs::write(
        environments_dir(state).join(format!("{}.json", env.name)),
        serde_json::to_string_pretty(env).unwrap(),
    )
    .map_err(|e| e.to_string())
}

pub fn delete_environment(state: &WorkspaceState, name: &str) -> Result<bool, String> {
    let f = environments_dir(state).join(format!("{name}.json"));
    if f.exists() {
        fs::remove_file(f).map_err(|e| e.to_string())?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Type)]
pub struct HistoryEntry {
    pub id: String,
    pub scenario_name: String,
    pub scenario_file: String,
    pub environment: Option<String>,
    pub started_at: String,
    pub duration_ms: u64,
    pub total_steps: usize,
    pub passed: usize,
    pub failed: usize,
    pub log: runner::ExecutionLog,
}

pub fn list_history(state: &WorkspaceState, limit: usize) -> Result<Vec<HistoryEntry>, String> {
    let d = history_dir(state);
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = d
        .read_dir()
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, e.path()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));

    Ok(files
        .into_iter()
        .take(limit)
        .filter_map(|(_, p)| {
            fs::read_to_string(p)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
        })
        .collect())
}

pub fn get_history_entry(
    state: &WorkspaceState,
    id: &str,
) -> Result<Option<HistoryEntry>, String> {
    let f = history_dir(state).join(format!("{id}.json"));
    if !f.exists() {
        return Ok(None);
    }
    serde_json::from_str(&fs::read_to_string(f).map_err(|e| e.to_string())?)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn save_history_entry(state: &WorkspaceState, entry: &HistoryEntry) -> Result<(), String> {
    fs::write(
        history_dir(state).join(format!("{}.json", entry.id)),
        serde_json::to_string_pretty(entry).unwrap(),
    )
    .map_err(|e| e.to_string())
}

pub fn delete_history_entry(state: &WorkspaceState, id: &str) -> Result<bool, String> {
    let f = history_dir(state).join(format!("{id}.json"));
    if f.exists() {
        fs::remove_file(f).map_err(|e| e.to_string())?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn clear_history(state: &WorkspaceState) -> Result<usize, String> {
    let d = history_dir(state);
    let mut count = 0;
    for entry in d.read_dir().map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") {
            fs::remove_file(p).map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    fn make_state(dir: &Path) -> WorkspaceState {
        WorkspaceState(Mutex::new(dir.to_path_buf()))
    }

    #[test]
    fn sanitize_name_strips_extension() {
        assert_eq!(sanitize_name("foo.yaml").unwrap(), "foo");
        assert_eq!(sanitize_name("bar.yml").unwrap(), "bar");
        assert_eq!(sanitize_name("baz").unwrap(), "baz");
    }

    #[test]
    fn sanitize_name_rejects_traversal() {
        assert!(sanitize_name("../etc/passwd").is_ok()); // keeps last component: "passwd"
        assert_eq!(sanitize_name("../evil").unwrap(), "evil");
        assert!(sanitize_name("..").is_err());
        assert!(sanitize_name(".").is_err());
        assert!(sanitize_name("").is_err());
    }

    #[test]
    fn sanitize_name_strips_null_bytes() {
        let result = sanitize_name("foo\x00bar").unwrap();
        assert!(!result.contains('\x00'));
    }

    #[test]
    fn environment_roundtrip() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path());

        let env = Environment {
            name: "staging".to_string(),
            variables: [("BASE_URL".to_string(), "https://api.example.com".to_string())]
                .into_iter()
                .collect(),
        };

        save_environment(&state, &env).unwrap();
        let loaded = get_environment(&state, "staging").unwrap().unwrap();
        assert_eq!(loaded.name, "staging");
        assert_eq!(loaded.variables["BASE_URL"], "https://api.example.com");

        let list = list_environments(&state).unwrap();
        assert_eq!(list.len(), 1);

        assert!(delete_environment(&state, "staging").unwrap());
        assert!(get_environment(&state, "staging").unwrap().is_none());
    }

    #[test]
    fn history_pagination_and_clear() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path());

        for i in 0..5_u8 {
            let entry = HistoryEntry {
                id: format!("id{i:02}"),
                scenario_name: format!("scenario{i}"),
                scenario_file: format!("scenario{i}.yaml"),
                environment: None,
                started_at: "2026-01-01T00:00:00Z".to_string(),
                duration_ms: u64::from(i) * 100,
                total_steps: 1,
                passed: 1,
                failed: 0,
                log: runner::ExecutionLog::default(),
            };
            save_history_entry(&state, &entry).unwrap();
        }

        let page = list_history(&state, 3).unwrap();
        assert_eq!(page.len(), 3);

        assert!(get_history_entry(&state, "id00").unwrap().is_some());
        assert!(delete_history_entry(&state, "id00").unwrap());
        assert!(get_history_entry(&state, "id00").unwrap().is_none());

        let removed = clear_history(&state).unwrap();
        assert_eq!(removed, 4);
        assert_eq!(list_history(&state, 50).unwrap().len(), 0);
    }

    #[test]
    fn duplicate_collision_loop() {
        let dir = tempdir().unwrap();
        let examples = dir.path().join("examples");
        fs::create_dir_all(&examples).unwrap();

        let src = examples.join("hello.yaml");
        fs::write(&src, "name: Hello\ninitial_state: start\nsteps: []\n").unwrap();

        let state = make_state(dir.path());

        let first = duplicate_scenario_file(&state, "hello").unwrap();
        assert!(first.exists());
        assert_eq!(first.file_name().unwrap(), "hello_copy1.yaml");

        // Pre-create copy2 to force the loop forward.
        fs::write(examples.join("hello_copy2.yaml"), "name: taken\ninitial_state: start\nsteps: []\n")
            .unwrap();
        let third = duplicate_scenario_file(&state, "hello").unwrap();
        assert_eq!(third.file_name().unwrap(), "hello_copy3.yaml");
    }
}
