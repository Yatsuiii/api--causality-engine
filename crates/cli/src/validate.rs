use crate::error::{CliError, load_scenario_file};
use ace_core::validate::validate_scenario;
use colored::Colorize;

pub fn cmd_validate(path: &str) -> Result<(), CliError> {
    let scenario = load_scenario_file(path)?;

    let issues = validate_scenario(&scenario);

    println!("\n{} {}", "Validating:".bold(), scenario.name.cyan());
    println!(
        "  {} step(s), concurrency: {}",
        scenario.steps.len(),
        scenario.concurrency.unwrap_or(1),
    );

    if issues.is_empty() {
        println!("\n  {} Scenario is valid", "✓".green().bold());
        Ok(())
    } else {
        Err(CliError::Validation(issues))
    }
}
