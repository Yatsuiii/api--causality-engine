use crate::error::{CliError, load_scenario_file};
use ace_core::validate::{render_state_graph, validate_scenario};
use colored::Colorize;

pub fn cmd_validate(path: &str, show_graph: bool) -> Result<(), CliError> {
    let scenario = load_scenario_file(path)?;

    let issues = validate_scenario(&scenario);

    println!();
    println!("{} {}", "Validation".bold(), "Report".bold().cyan());
    println!(
        "{} {} | {} {} | {} {}",
        "Scenario:".bold(),
        scenario.name.cyan(),
        "Steps:".bold(),
        scenario.steps.len(),
        "Concurrency:".bold(),
        scenario.concurrency.unwrap_or(1),
    );

    if show_graph {
        println!("\n{}", "State Graph".bold());
        for line in render_state_graph(&scenario) {
            println!("  {}", line.dimmed());
        }
    }

    println!("\n{}", "Static Checks".bold());
    if issues.is_empty() {
        println!("  {} no validation issues found", "✓".green().bold());
        return Ok(());
    }

    println!(
        "  {} found {} issue(s)",
        "✗".red().bold(),
        issues.len().to_string().red()
    );
    Err(CliError::Validation(issues))
}
