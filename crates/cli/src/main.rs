mod docs;
mod error;
mod import;
mod init;
mod mock;
mod replay;
mod report;
mod run;
mod validate;

use clap::{Parser, Subcommand};
use run::RunArgs;
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

    /// Scaffold a new scenario YAML file
    Init {
        /// Output file path
        #[arg(default_value = "ace.yaml")]
        output: String,

        /// Write a minimal skeleton instead of a full example
        #[arg(long)]
        minimal: bool,
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
// Main — single dispatch + single error handler
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
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
            run::cmd_run(RunArgs {
                scenario,
                env,
                vars,
                verbose,
                quiet,
                output,
                junit,
                insecure,
                proxy,
            })
            .await
        }
        Commands::Replay { log_file } => replay::cmd_replay(&log_file),
        Commands::Validate { scenario } => validate::cmd_validate(&scenario),
        Commands::Report {
            log_file,
            format,
            output,
        } => report::cmd_report(&log_file, &format, output),
        Commands::Init { output, minimal } => init::cmd_init(&output, minimal),
        Commands::Import { collection, output } => import::cmd_import(&collection, &output),
        Commands::Mock { scenario, port } => mock::cmd_mock(&scenario, port).await,
        Commands::Docs { scenario, output } => docs::cmd_docs(&scenario, output),
    };

    if let Err(e) = result {
        process::exit(e.report());
    }
}
