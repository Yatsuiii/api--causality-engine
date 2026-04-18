use crate::error::{CliError, load_execution_log};
use crate::report;
use colored::Colorize;
use executor::{ExecutionLog, RunError};

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
            let result = report::result_from_log(&log);
            (log, result)
        })
        .collect();

    report::print_summary(&results);
    Ok(())
}
