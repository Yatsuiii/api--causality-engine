use crate::error::{load_execution_log, CliError};
use crate::report;
use colored::Colorize;
use runner::{ExecutionLog, RunError};

pub fn cmd_replay(path: &str) -> Result<(), CliError> {
    let logs = load_execution_log(path)?;

    println!(
        "\n{} Replaying {} execution(s)...\n",
        "▶".cyan().bold(),
        logs.len()
    );

    for (i, log) in logs.iter().enumerate() {
        for step in &log.steps {
            report::print_step_live(i + 1, step, false);
        }
    }

    let results: Vec<(ExecutionLog, Result<String, RunError>)> = logs
        .into_iter()
        .map(|log| {
            let final_state = log
                .steps
                .last()
                .map(|s| s.state_after.clone())
                .unwrap_or_else(|| "unknown".into());
            (log, Ok(final_state))
        })
        .collect();

    report::print_summary(&results);
    Ok(())
}
