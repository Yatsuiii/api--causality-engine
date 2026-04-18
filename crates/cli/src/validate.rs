use crate::error::{CliError, print_diagnostics, read_file};
use ace_core::validator::{LineIndex, render_state_graph, validate_scenario};
use colored::Colorize;

pub fn cmd_validate(path: &str, show_graph: bool) -> Result<(), CliError> {
    let yaml = read_file(path)?;
    let scenario = model::load_scenario(&yaml).map_err(CliError::YamlParse)?;
    let index = LineIndex::build(&yaml);

    let diagnostics = validate_scenario(&scenario, &index);

    println!();
    println!("{} {}", "Validation".bold(), "Report".bold().cyan());
    #[allow(deprecated)]
    let legacy_concurrency = scenario.concurrency.unwrap_or(1);
    println!(
        "{} {} | {} {} | {} {}",
        "Scenario:".bold(),
        scenario.name.cyan(),
        "Steps:".bold(),
        scenario.steps.len(),
        "Concurrency:".bold(),
        legacy_concurrency,
    );

    if show_graph {
        println!("\n{}", "State Graph".bold());
        for line in render_state_graph(&scenario) {
            println!("  {}", line.dimmed());
        }
    }

    println!("\n{}", "Static Checks".bold());
    if diagnostics.is_empty() {
        println!("  {} no validation issues found", "✓".green().bold());
        return Ok(());
    }

    let (error_count, warn_count) = print_diagnostics(&diagnostics, path);

    eprintln!();
    if error_count > 0 {
        eprintln!(
            "  {} found {} error(s), {} warning(s)",
            "✗".red().bold(),
            error_count.to_string().red(),
            warn_count,
        );
        Err(CliError::ValidationFailed)
    } else {
        eprintln!("  {} found {} warning(s)", "⚠".yellow().bold(), warn_count,);
        Ok(())
    }
}
