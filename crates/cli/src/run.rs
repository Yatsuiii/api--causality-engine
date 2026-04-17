use crate::error::{CliError, load_scenario_file};
use crate::report;
use ace_core::validate::validate_scenario;
use colored::Colorize;
use runner::RunConfig;
use std::collections::HashMap;

/// All arguments for the `run` subcommand, bundled into a struct.
pub struct RunArgs {
    pub scenario: String,
    pub env: Option<String>,
    pub vars: Vec<(String, String)>,
    pub verbose: bool,
    pub quiet: bool,
    pub output: String,
    pub junit: Option<String>,
    pub insecure: bool,
    pub proxy: Option<String>,
}

pub async fn cmd_run(args: RunArgs) -> Result<(), CliError> {
    // Setup tracing
    let filter = if args.verbose {
        "runner=debug,ace_core=debug,ace_http=debug,info"
    } else if args.quiet {
        "error"
    } else {
        "warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_target(false)
        .without_time()
        .init();

    // Load .env file
    if let Some(env_path) = &args.env {
        dotenvy::from_filename(env_path).ok();
    } else {
        dotenvy::dotenv().ok();
    }

    // Load & validate scenario
    let scenario = load_scenario_file(&args.scenario)?;

    let issues = validate_scenario(&scenario);
    if !issues.is_empty() {
        return Err(CliError::Validation(issues));
    }

    // Print header
    if !args.quiet {
        println!();
        println!("{} {}", "Scenario:".bold(), scenario.name.cyan());
        let concurrency = scenario.concurrency.unwrap_or(1);
        println!(
            "{} {} user(s) × {} step(s)",
            "Running:".bold(),
            concurrency,
            scenario.steps.len(),
        );

        if args.verbose {
            for (i, step) in scenario.steps.iter().enumerate() {
                let next_states = scenario
                    .edges
                    .iter()
                    .filter(|edge| edge.from == step.state)
                    .map(|edge| edge.to.as_str())
                    .collect::<Vec<_>>()
                    .join("|");
                println!(
                    "  {} {} {} {} [{} → {}]",
                    format!("{}.", i + 1).dimmed(),
                    step.method.to_string().yellow(),
                    step.url.dimmed(),
                    step.name,
                    step.state_name().cyan(),
                    next_states.cyan(),
                );
            }
        }
        println!();
    }

    // Build config & execute
    let cli_variables: HashMap<String, String> = args.vars.into_iter().collect();
    let config = RunConfig {
        cli_variables,
        verbose: args.verbose,
        insecure: args.insecure,
        proxy: args.proxy,
    };

    let results = runner::run(&scenario, &config).await;
    let has_assertion_failures = results.iter().any(|(log, r)| {
        log.failed > 0 || matches!(r, Err(runner::RunError::AssertionFailed { .. }))
    });
    let has_engine_errors = results.iter().any(|(_, r)| {
        matches!(
            r,
            Err(runner::RunError::HttpError { .. })
                | Err(runner::RunError::MaxIterationsExceeded { .. })
                | Err(runner::RunError::InvalidTransition { .. })
                | Err(runner::RunError::NoMatchingTransition { .. })
                | Err(runner::RunError::Skipped { .. })
        )
    });

    if !args.quiet {
        for (i, (log, _)) in results.iter().enumerate() {
            for step in &log.steps {
                report::print_step_live(i + 1, step, args.verbose);
            }
        }
        report::print_summary(&results);
    }

    if let Err(e) = report::write_json_report(&results, &args.output) {
        eprintln!("{} Failed to write log: {}", "warning:".yellow().bold(), e);
    } else if !args.quiet {
        println!("\n  {} {}", "Log:".dimmed(), args.output);
    }

    if let Some(junit_path) = &args.junit {
        if let Err(e) = report::write_junit_report(&results, &scenario.name, junit_path) {
            eprintln!(
                "{} Failed to write JUnit report: {}",
                "warning:".yellow().bold(),
                e
            );
        } else if !args.quiet {
            println!("  {} {}", "JUnit:".dimmed(), junit_path);
        }
    }

    if has_engine_errors {
        return Err(CliError::RunError);
    }
    if has_assertion_failures {
        return Err(CliError::RunFailed);
    }

    Ok(())
}
