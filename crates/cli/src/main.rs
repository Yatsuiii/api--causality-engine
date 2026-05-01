mod diff;
mod error;
mod glyph;
mod import;
mod init;
mod junit;
mod mock;
mod render;
mod report;
mod run;
mod show;
mod validate;

use clap::{Parser, Subcommand, ValueEnum};
use run::RunArgs;
use std::process;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum RedactMode {
    /// Mask values under sensitive keys before writing to execution_log.json (default).
    On,
    /// Disable redaction. Raw URLs, bodies, and assertion values land in the log.
    Off,
}

impl RedactMode {
    pub fn enabled(self) -> bool {
        matches!(self, RedactMode::On)
    }
}

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "ace",
    about = "API Causality Engine — stateful API workflow testing",
    version,
    long_about = "A CLI for defining, executing, and validating stateful API \
                   workflows from YAML scenarios. Supports headers, request bodies, \
                   assertions, auth, concurrency, retry logic, variable substitution, \
                   and deterministic weighted routing via --seed.",
    after_help = "EXIT CODES:\n  \
        0   no diff (diff) | passed (run)\n  \
        1   diff found (diff) | assertion failed (run)\n  \
        2   error (file not found, network, bad scenario, engine failure)"
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

        /// Number of parallel runs of the whole scenario (VUs)
        #[arg(short = 'c', long)]
        concurrency: Option<usize>,

        /// Fail the step if any `extract:` JSONPath does not resolve (default: warn and continue)
        #[arg(long)]
        strict_extract: bool,

        /// Base RNG seed for weighted edge routing (deterministic replay)
        #[arg(long)]
        seed: Option<u64>,

        /// Redact sensitive values (tokens, passwords, api keys) in the
        /// execution log and stdout. Default: on.
        #[arg(long, value_enum, default_value_t = RedactMode::On)]
        redact: RedactMode,
    },

    /// Print a previous execution log to the terminal (no re-execution)
    Show {
        /// Path to the execution log JSON file
        log_file: String,
    },

    /// Deprecated alias for `show`. The command does NOT re-execute — it only
    /// re-renders the logged run. Use `ace show <log>` instead.
    #[command(hide = true)]
    Replay {
        /// Path to the execution log JSON file
        log_file: String,
    },

    /// Validate a scenario without running it
    Validate {
        /// Path to the scenario YAML file
        scenario: String,
        /// Print the resolved state graph preview
        #[arg(long, default_value_t = false)]
        graph: bool,
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

    /// Diff two execution logs and show routing divergences
    Diff {
        /// Path to trace A (e.g. staging.json)
        trace_a: String,

        /// Path to trace B (e.g. prod.json)
        trace_b: String,

        /// Output format: text, markdown, or json
        #[arg(long, default_value = "text")]
        format: String,

        /// Write output to file instead of stdout
        #[arg(short, long)]
        output: Option<String>,

        /// Extra mask rules YAML (same format as scenario `mask:` block).
        /// Useful when diffing traces you did not produce yourself.
        #[arg(long)]
        mask_extra: Option<String>,

        /// Expand each `· masked: …` line with the pre-mask values from
        /// both traces. Requires raw `response_body` to have been retained
        /// (automatic when scenario `mask:` is non-empty).
        #[arg(long)]
        show_masked: bool,

        /// Suppress all output except the `ACE_SUMMARY: …` line. Useful for
        /// CI scripts that only want the verdict JSON.
        #[arg(short, long)]
        quiet: bool,
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
            concurrency,
            strict_extract,
            seed,
            redact,
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
                concurrency,
                strict_extract,
                seed,
                redact: redact.enabled(),
            })
            .await
        }
        Commands::Show { log_file } => show::cmd_show(&log_file, false),
        Commands::Replay { log_file } => show::cmd_show(&log_file, true),
        Commands::Validate { scenario, graph } => validate::cmd_validate(&scenario, graph),
        Commands::Report {
            log_file,
            format,
            output,
        } => report::cmd_report(&log_file, &format, output),
        Commands::Init { output, minimal } => init::cmd_init(&output, minimal),
        Commands::Import { collection, output } => import::cmd_import(&collection, &output),
        Commands::Mock { scenario, port } => mock::cmd_mock(&scenario, port).await,
        Commands::Diff {
            trace_a,
            trace_b,
            format,
            output,
            mask_extra,
            show_masked,
            quiet,
        } => diff::cmd_diff(diff::DiffArgs {
            a: trace_a,
            b: trace_b,
            format,
            output,
            mask_extra,
            show_masked,
            quiet,
        }),
    };

    if let Err(e) = result {
        process::exit(e.report());
    }
}
