use crate::assertions::AssertionResult;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug)]
pub enum RunError {
    InvalidTransition {
        step: String,
        expected: String,
        actual: String,
    },
    HttpError {
        step: String,
        message: String,
    },
    AssertionFailed {
        step: String,
        failures: Vec<AssertionResult>,
    },
    Skipped {
        step: String,
        reason: String,
    },
    NoMatchingTransition {
        state: String,
        status: u16,
    },
    NoOutgoingEdges {
        step: String,
        state: String,
    },
    MaxIterationsExceeded {
        limit: u64,
    },
    ExtractionMissing {
        step: String,
        key: String,
        path: String,
    },
    EdgeMaxTakesExceeded {
        state: String,
        to: String,
        limit: u32,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::InvalidTransition {
                step,
                expected,
                actual,
            } => write!(
                f,
                "Step '{}': expected state '{}', but current state is '{}'",
                step, expected, actual
            ),
            RunError::HttpError { step, message } => {
                write!(f, "Step '{}': HTTP error: {}", step, message)
            }
            RunError::AssertionFailed { step, failures } => {
                write!(f, "Step '{}': {} assertion(s) failed", step, failures.len())
            }
            RunError::Skipped { step, reason } => {
                write!(f, "Step '{}': skipped ({})", step, reason)
            }
            RunError::NoMatchingTransition { state, status } => {
                write!(
                    f,
                    "State '{}': no matching transition for status {}",
                    state, status
                )
            }
            RunError::NoOutgoingEdges { step, state } => {
                write!(
                    f,
                    "Step '{}': state '{}' has no outgoing edges — explicit graphs require every state to transition",
                    step, state
                )
            }
            RunError::MaxIterationsExceeded { limit } => {
                write!(f, "Max iterations exceeded (limit: {})", limit)
            }
            RunError::ExtractionMissing { step, key, path } => {
                write!(
                    f,
                    "Step '{}': extraction '{}' failed — JSONPath '{}' did not resolve in response body",
                    step, key, path
                )
            }
            RunError::EdgeMaxTakesExceeded { state, to, limit } => {
                write!(
                    f,
                    "State '{}': edge to '{}' exceeded max_takes ({})",
                    state, to, limit
                )
            }
        }
    }
}

impl std::error::Error for RunError {}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub cli_variables: HashMap<String, String>,
    pub verbose: bool,
    pub insecure: bool,
    pub proxy: Option<String>,
    /// CLI-supplied concurrency override. Takes precedence over the deprecated
    /// `scenario.concurrency` field.
    pub concurrency: Option<usize>,
    /// When true, a missing JSONPath in an `extract:` block causes the step to fail
    /// instead of silently leaving the variable undefined.
    pub strict_extract: bool,
    /// Base RNG seed for weighted edge routing. When unset, a random seed is
    /// generated per run and echoed via `ExecutionLog.seed` for replay.
    pub seed: Option<u64>,
    /// Global redaction switch. `true` masks sensitive values (tokens,
    /// passwords, api keys) in log URLs, bodies, and assertion results before
    /// they reach `execution_log.json`. `false` disables all masking — the
    /// `--redact=off` escape hatch for debugging. Per-scenario `log:` policy
    /// (body inclusion, size cap, extra mask/unmask) still applies either way.
    pub redact: bool,
    /// Directory that relative `schema:` file paths in assertions resolve
    /// against — typically the scenario file's parent. Unset means resolve
    /// against the CWD.
    pub scenario_dir: Option<std::path::PathBuf>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            cli_variables: HashMap::new(),
            verbose: false,
            insecure: false,
            proxy: None,
            concurrency: None,
            strict_extract: false,
            seed: None,
            redact: true,
            scenario_dir: None,
        }
    }
}
