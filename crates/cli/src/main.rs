mod import;
mod mock;
mod report;

use ace_core::validate::validate_scenario;
use clap::{Parser, Subcommand};
use colored::Colorize;
use model::load_scenario;
use runner::{ExecutionLog, RunConfig, RunError};
use std::collections::HashMap;
use std::fs;
use std::process;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "ace",
    about = "API Causality Engine — stateful API workflow testing",
    version,
    long_about = "A production-grade engine for defining, executing, and validating \
                   stateful API workflows using YAML scenarios. Supports headers, \
                   request bodies, assertions, auth, concurrency, retry logic, \
                   variable substitution, and deterministic replay."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run an API workflow scenario
    Run {
        /// Path to the scenario YAML file
        scenario: String,

        /// Load environment variables from a .env file
        #[arg(long)]
        env: Option<String>,

        /// Override variables (e.g. --var base_url=https://api.example.com)
        #[arg(long = "var", value_parser = parse_key_val)]
        vars: Vec<(String, String)>,

        /// Show verbose output (request/response bodies)
        #[arg(short, long)]
        verbose: bool,

        /// Suppress output (only show summary)
        #[arg(short, long)]
        quiet: bool,

        /// Output log file path
        #[arg(short, long, default_value = "execution_log.json")]
        output: String,

        /// Also generate JUnit XML report
        #[arg(long)]
        junit: Option<String>,

        /// Accept invalid TLS certificates
        #[arg(long)]
        insecure: bool,

        /// HTTP proxy (e.g. http://localhost:8080)
        #[arg(long)]
        proxy: Option<String>,
    },

    /// Replay a previous execution from a JSON log
    Replay {
        /// Path to the execution log JSON file
        log_file: String,
    },

    /// Validate a scenario without running it
    Validate {
        /// Path to the scenario YAML file
        scenario: String,
    },

    /// Generate a report from an execution log
    Report {
        /// Path to the execution log JSON file
        log_file: String,

        /// Output format: json or junit
        #[arg(long, default_value = "json")]
        format: String,

        /// Output file path
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Import a Postman collection to ACE YAML scenarios
    Import {
        /// Path to the Postman collection JSON file
        collection: String,

        /// Output directory for YAML files
        #[arg(short, long, default_value = ".")]
        output: String,
    },

    /// Start a mock server from a scenario's step definitions
    Mock {
        /// Path to the scenario YAML file
        scenario: String,

        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,
    },

    /// Generate API documentation from a scenario
    Docs {
        /// Path to the scenario YAML file
        scenario: String,

        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<String>,
    },
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no '=' found in '{}'", s))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            scenario,
            env,
            vars,
            verbose,
            quiet,
            output,
            junit,
            insecure,
            proxy,
        } => {
            cmd_run(
                &scenario, env, vars, verbose, quiet, &output, junit, insecure, proxy,
            )
            .await
        }
        Commands::Replay { log_file } => cmd_replay(&log_file),
        Commands::Validate { scenario } => cmd_validate(&scenario),
        Commands::Report {
            log_file,
            format,
            output,
        } => cmd_report(&log_file, &format, output),
        Commands::Import { collection, output } => import::cmd_import(&collection, &output),
        Commands::Mock { scenario, port } => mock::cmd_mock(&scenario, port).await,
        Commands::Docs { scenario, output } => cmd_docs(&scenario, output),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn cmd_run(
    path: &str,
    env_file: Option<String>,
    vars: Vec<(String, String)>,
    verbose: bool,
    quiet: bool,
    output: &str,
    junit: Option<String>,
    insecure: bool,
    proxy: Option<String>,
) {
    // Setup tracing
    let filter = if verbose {
        "runner=debug,ace_core=debug,ace_http=debug,info"
    } else if quiet {
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
    if let Some(env_path) = &env_file {
        dotenvy::from_filename(env_path).ok();
    } else {
        dotenvy::dotenv().ok();
    }

    // Load scenario
    let yaml = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("{} Failed to read '{}': {}", "error:".red().bold(), path, e);
        process::exit(2);
    });

    let scenario = load_scenario(&yaml).unwrap_or_else(|e| {
        eprintln!("{} Failed to parse scenario: {}", "error:".red().bold(), e);
        process::exit(2);
    });

    // Pre-flight validation
    let issues = validate_scenario(&scenario);
    if !issues.is_empty() {
        eprintln!("{} Scenario validation failed:", "error:".red().bold());
        for issue in &issues {
            eprintln!("  {} {}", "•".red(), issue);
        }
        process::exit(2);
    }

    // Print header
    if !quiet {
        println!();
        println!("{} {}", "Scenario:".bold(), scenario.name.cyan());
        let concurrency = scenario.concurrency.unwrap_or(1);
        println!(
            "{} {} user(s) × {} step(s)",
            "Running:".bold(),
            concurrency,
            scenario.steps.len(),
        );

        if verbose {
            for (i, step) in scenario.steps.iter().enumerate() {
                println!(
                    "  {} {} {} {} [{} → {}]",
                    format!("{}.", i + 1).dimmed(),
                    step.method.to_string().yellow(),
                    step.url.dimmed(),
                    step.name,
                    step.transition.from.cyan(),
                    step.transition.to.cyan(),
                );
            }
        }
        println!();
    }

    // Build config
    let cli_variables: HashMap<String, String> = vars.into_iter().collect();
    let config = RunConfig {
        cli_variables,
        verbose,
        insecure,
        proxy,
    };

    // Execute
    let results = runner::run(&scenario, &config).await;

    if !quiet {
        for (i, (log, _)) in results.iter().enumerate() {
            for step in &log.steps {
                report::print_step_live(i + 1, step, verbose);
            }
        }
        report::print_summary(&results);
    }

    if let Err(e) = report::write_json_report(&results, output) {
        eprintln!("{} Failed to write log: {}", "warning:".yellow().bold(), e);
    } else if !quiet {
        println!("\n  {} {}", "Log:".dimmed(), output);
    }

    if let Some(junit_path) = &junit {
        if let Err(e) = report::write_junit_report(&results, &scenario.name, junit_path) {
            eprintln!(
                "{} Failed to write JUnit report: {}",
                "warning:".yellow().bold(),
                e
            );
        } else if !quiet {
            println!("  {} {}", "JUnit:".dimmed(), junit_path);
        }
    }

    let has_failures = results.iter().any(|(log, r)| log.failed > 0 || r.is_err());
    if has_failures {
        process::exit(1);
    }
}

fn cmd_replay(path: &str) {
    let json = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("{} Failed to read '{}': {}", "error:".red().bold(), path, e);
        process::exit(2);
    });

    let logs: Vec<ExecutionLog> = serde_json::from_str(&json).unwrap_or_else(|e| {
        eprintln!("{} Failed to parse log: {}", "error:".red().bold(), e);
        process::exit(2);
    });

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
}

fn cmd_validate(path: &str) {
    let yaml = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("{} Failed to read '{}': {}", "error:".red().bold(), path, e);
        process::exit(2);
    });

    let scenario = load_scenario(&yaml).unwrap_or_else(|e| {
        eprintln!("{} Failed to parse scenario: {}", "error:".red().bold(), e);
        process::exit(2);
    });

    let issues = validate_scenario(&scenario);

    println!("\n{} {}", "Validating:".bold(), scenario.name.cyan());
    println!(
        "  {} step(s), concurrency: {}",
        scenario.steps.len(),
        scenario.concurrency.unwrap_or(1),
    );

    if issues.is_empty() {
        println!("\n  {} Scenario is valid", "✓".green().bold());
    } else {
        println!("\n  {} {} issue(s) found:", "✗".red().bold(), issues.len());
        for issue in &issues {
            println!("    {} {}", "•".red(), issue);
        }
        process::exit(1);
    }
}

fn cmd_report(log_path: &str, format: &str, output: Option<String>) {
    let json = fs::read_to_string(log_path).unwrap_or_else(|e| {
        eprintln!(
            "{} Failed to read '{}': {}",
            "error:".red().bold(),
            log_path,
            e
        );
        process::exit(2);
    });

    let logs: Vec<ExecutionLog> = serde_json::from_str(&json).unwrap_or_else(|e| {
        eprintln!("{} Failed to parse log: {}", "error:".red().bold(), e);
        process::exit(2);
    });

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

    match format {
        "json" => {
            let out_path = output.unwrap_or_else(|| "report.json".into());
            report::write_json_report(&results, &out_path).unwrap_or_else(|e| {
                eprintln!("{} {}", "error:".red().bold(), e);
                process::exit(2);
            });
            println!("{} {}", "Report written:".green().bold(), out_path);
        }
        "junit" => {
            let out_path = output.unwrap_or_else(|| "report.xml".into());
            report::write_junit_report(&results, "scenario", &out_path).unwrap_or_else(|e| {
                eprintln!("{} {}", "error:".red().bold(), e);
                process::exit(2);
            });
            println!("{} {}", "Report written:".green().bold(), out_path);
        }
        other => {
            eprintln!(
                "{} Unknown format '{}'. Use 'json' or 'junit'.",
                "error:".red().bold(),
                other
            );
            process::exit(2);
        }
    }
}

fn cmd_docs(path: &str, output: Option<String>) {
    let yaml = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("{} Failed to read '{}': {}", "error:".red().bold(), path, e);
        process::exit(2);
    });

    let scenario = load_scenario(&yaml).unwrap_or_else(|e| {
        eprintln!("{} Failed to parse scenario: {}", "error:".red().bold(), e);
        process::exit(2);
    });

    let mut doc = String::new();
    doc.push_str(&format!("# {}\n\n", scenario.name));

    if let Some(vars) = &scenario.variables {
        doc.push_str("## Variables\n\n");
        doc.push_str("| Name | Default |\n|------|--------|\n");
        for (k, v) in vars {
            doc.push_str(&format!("| `{}` | `{}` |\n", k, v));
        }
        doc.push('\n');
    }

    if let Some(auth) = &scenario.auth {
        doc.push_str("## Authentication\n\n");
        if auth.bearer.is_some() {
            doc.push_str("- **Type:** Bearer Token\n");
        }
        if auth.basic.is_some() {
            doc.push_str("- **Type:** Basic Auth\n");
        }
        if auth.api_key.is_some() {
            doc.push_str("- **Type:** API Key\n");
        }
        if auth.oauth2.is_some() {
            doc.push_str("- **Type:** OAuth2 Client Credentials\n");
        }
        doc.push('\n');
    }

    doc.push_str("## Endpoints\n\n");

    for (i, step) in scenario.steps.iter().enumerate() {
        doc.push_str(&format!(
            "### {}. {} `{}`\n\n",
            i + 1,
            step.method,
            step.url
        ));
        doc.push_str(&format!("**{}**\n\n", step.name));
        doc.push_str(&format!(
            "State transition: `{}` → `{}`\n\n",
            step.transition.from, step.transition.to
        ));

        if let Some(headers) = &step.headers {
            doc.push_str("**Headers:**\n\n");
            doc.push_str("| Header | Value |\n|--------|-------|\n");
            for (k, v) in headers {
                doc.push_str(&format!("| `{}` | `{}` |\n", k, v));
            }
            doc.push('\n');
        }

        if let Some(body) = &step.body {
            doc.push_str("**Request Body:**\n\n```json\n");
            let json_val: serde_json::Value =
                serde_json::from_str(&serde_json::to_string(body).unwrap_or_default())
                    .unwrap_or(serde_json::Value::Null);
            doc.push_str(&serde_json::to_string_pretty(&json_val).unwrap_or_default());
            doc.push_str("\n```\n\n");
        }

        if let Some(assertions) = &step.assertions {
            doc.push_str("**Assertions:**\n\n");
            for assertion in assertions {
                if let Some(status) = &assertion.status {
                    match status {
                        model::StatusCheck::Exact(code) => {
                            doc.push_str(&format!("- Status: `{}`\n", code));
                        }
                        _ => doc.push_str("- Status: complex check\n"),
                    }
                }
                if let Some(body_checks) = &assertion.body {
                    for path in body_checks.keys() {
                        doc.push_str(&format!("- Body `{}`: validated\n", path));
                    }
                }
                if let Some(header_checks) = &assertion.header {
                    for name in header_checks.keys() {
                        doc.push_str(&format!("- Header `{}`: validated\n", name));
                    }
                }
                if assertion.response_time_ms.is_some() {
                    doc.push_str("- Response time: validated\n");
                }
            }
            doc.push('\n');
        }

        if let Some(extract) = &step.extract {
            doc.push_str("**Extracts:**\n\n");
            for (key, path) in extract {
                doc.push_str(&format!("- `{}` ← `{}`\n", key, path));
            }
            doc.push('\n');
        }
    }

    match output {
        Some(path) => {
            fs::write(&path, &doc).unwrap_or_else(|e| {
                eprintln!("{} {}", "error:".red().bold(), e);
                process::exit(2);
            });
            println!("{} {}", "Docs written:".green().bold(), path);
        }
        None => print!("{}", doc),
    }
}
