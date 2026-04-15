use colored::Colorize;
use std::fmt;

/// Unified error type for all CLI commands.
#[derive(Debug)]
pub enum CliError {
    /// File I/O error (read/write).
    Io {
        path: String,
        source: std::io::Error,
    },
    /// YAML parsing failed.
    YamlParse(serde_yaml::Error),
    /// JSON parsing failed.
    JsonParse(serde_json::Error),
    /// Scenario validation found issues.
    Validation(Vec<String>),
    /// One or more scenario steps failed at runtime (assertion failure).
    RunFailed,
    /// One or more scenario steps encountered an engine/network error.
    RunError,
    /// A user-supplied argument was invalid.
    BadArgument(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Io { path, source } => write!(f, "Failed to read '{}': {}", path, source),
            CliError::YamlParse(e) => write!(f, "Failed to parse scenario: {}", e),
            CliError::JsonParse(e) => write!(f, "Failed to parse JSON: {}", e),
            CliError::Validation(issues) => {
                writeln!(f, "Scenario validation failed:")?;
                for issue in issues {
                    writeln!(f, "  {} {}", "•".red(), issue)?;
                }
                Ok(())
            }
            CliError::RunFailed => write!(f, "One or more steps failed"),
            CliError::RunError => {
                write!(f, "One or more steps encountered a network or engine error")
            }
            CliError::BadArgument(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for CliError {}

impl CliError {
    /// Exit code for this error category.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::RunFailed => 1,
            CliError::RunError => 2,
            _ => 2,
        }
    }

    /// Print the error to stderr with colour, then return the exit code.
    pub fn report(&self) -> i32 {
        eprintln!("{} {}", "error:".red().bold(), self);
        self.exit_code()
    }
}

// ── Helpers that replace the duplicated read→parse→exit pattern ──────────

/// Read a file to string, mapping the IO error.
pub fn read_file(path: &str) -> Result<String, CliError> {
    std::fs::read_to_string(path).map_err(|e| CliError::Io {
        path: path.to_string(),
        source: e,
    })
}

/// Read + parse a scenario YAML in one shot.
pub fn load_scenario_file(path: &str) -> Result<model::Scenario, CliError> {
    let yaml = read_file(path)?;
    model::load_scenario(&yaml).map_err(CliError::YamlParse)
}

/// Read + parse an execution-log JSON in one shot.
pub fn load_execution_log(path: &str) -> Result<Vec<runner::ExecutionLog>, CliError> {
    let json = read_file(path)?;
    serde_json::from_str(&json).map_err(CliError::JsonParse)
}

/// Write bytes to a file, mapping the IO error.
pub fn write_file(path: &str, contents: &str) -> Result<(), CliError> {
    std::fs::write(path, contents).map_err(|e| CliError::Io {
        path: path.to_string(),
        source: e,
    })
}
