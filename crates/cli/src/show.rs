use crate::error::{CliError, load_execution_log};
use crate::report;
use colored::Colorize;
use engine::{ExecutionLog, RunError};

/// Render a previously-recorded execution log to the terminal.
///
/// Despite the historical `replay` name, this never touches the network —
/// it only re-prints what the log already says. `deprecated_alias` is set
/// when invoked via `ace replay`, in which case we warn.
pub fn cmd_show(path: &str, deprecated_alias: bool) -> Result<(), CliError> {
    if deprecated_alias {
        eprintln!(
            "{} `ace replay` is deprecated — it does not re-execute, it only prints the log. Use `ace show {}` instead.",
            "warning:".yellow().bold(),
            path
        );
    }

    let logs = load_execution_log(path)?;

    println!(
        "\n{} Showing {} recorded execution(s)...\n",
        "▶".cyan().bold(),
        logs.len()
    );

    // Name the scenario that produced the log — three logs in /tmp/ shouldn't
    // be indistinguishable. Old logs without scenario metadata fall through
    // silently rather than rendering "unknown".
    if let Some(first) = logs.first()
        && let Some(name) = &first.scenario_name
    {
        let path_suffix = first
            .scenario_path
            .as_deref()
            .map(|p| format!(" ({})", p))
            .unwrap_or_default();
        println!(
            "  {} trace from {:?}{}",
            "ACE show:".bold(),
            name,
            path_suffix.dimmed()
        );
        println!();
    }

    for (i, log) in logs.iter().enumerate() {
        for step in &log.steps {
            report::print_step_live(i + 1, step, false);
        }
    }

    let results: Vec<(ExecutionLog, Result<String, RunError>)> = logs
        .into_iter()
        .map(|log| {
            let result = report::result_from_log(&log);
            (log, result)
        })
        .collect();

    report::print_summary(&results);
    Ok(())
}
