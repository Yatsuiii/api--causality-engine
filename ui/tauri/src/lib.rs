pub mod commands;
pub mod storage;

use storage::WorkspaceState;
use tauri::Manager;
use tauri_specta::{Builder, collect_commands};
use tracing_subscriber::EnvFilter;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ace_tauri_lib=info,executor=info,warn")),
        )
        .with_target(false)
        .init();

    let builder = Builder::<tauri::Wry>::new().commands(collect_commands![
        // workspace
        commands::workspace::get_workspace,
        commands::workspace::set_workspace,
        // scenarios
        commands::scenarios::list_scenarios,
        commands::scenarios::get_scenario,
        commands::scenarios::get_scenario_raw,
        commands::scenarios::update_scenario_raw,
        commands::scenarios::create_scenario,
        commands::scenarios::update_scenario,
        commands::scenarios::delete_scenario,
        commands::scenarios::duplicate_scenario,
        // environments
        commands::environments::list_environments,
        commands::environments::get_environment,
        commands::environments::create_environment,
        commands::environments::update_environment,
        commands::environments::delete_environment,
        // history
        commands::history::list_history,
        commands::history::get_history_entry,
        commands::history::delete_history_entry,
        commands::history::clear_history,
        // runner
        commands::runner::run_scenario,
        commands::runner::validate_scenario,
    ]);

    // Emit TypeScript bindings whenever the library is compiled in dev mode.
    #[cfg(debug_assertions)]
    builder
        .export(
            specta_typescript::Typescript::default(),
            "../frontend/src/bindings.ts",
        )
        .expect("failed to export TypeScript bindings");

    tauri::Builder::default()
        .invoke_handler(builder.invoke_handler())
        .setup(|app| {
            let workspace = storage::load_workspace(app.handle());
            app.manage(WorkspaceState(std::sync::Mutex::new(workspace)));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
