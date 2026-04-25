use crate::assertions::AssertionResult;
use crate::trace::EdgeEvaluation;

/// Machine-readable step failure discriminant. Set on the `StepLog` that
/// caused a run to end so `result_from_log` can reconstruct the error
/// without heuristics. Absent on successful steps and on steps that failed
/// only via assertion results.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepFailure {
    NoMatch,
    MaxTakesExceeded {
        to: String,
        limit: u32,
    },
    ExtractionMissing {
        key: String,
        path: String,
    },
    HttpError {
        message: String,
    },
    /// Unknown variant from a newer ACE version — ignored by `result_from_log`.
    #[serde(other)]
    Unknown,
}

fn default_schema_version() -> u32 {
    1
}

fn is_default_schema_version(v: &u32) -> bool {
    *v == 1
}

/// Schema version for the trace JSON format. Increment when making breaking
/// changes to `ExecutionLog` or `EdgeOutcome`. Old logs without this field
/// default to 1 (the first versioned schema).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ExecutionLog {
    #[serde(
        default = "default_schema_version",
        skip_serializing_if = "is_default_schema_version"
    )]
    pub schema_version: u32, // omitted when 1 to keep old log files compact
    pub steps: Vec<StepLog>,
    pub total_duration_ms: u64,
    pub total_steps: usize,
    pub passed: usize,
    pub failed: usize,
    #[serde(default)]
    pub iterations: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<String>,
    /// Per-task RNG seed. Populated on every run so weighted-routing outcomes
    /// can be reproduced by passing `--seed <value>` (with matching concurrency).
    #[serde(default)]
    pub seed: u64,
    /// Human-readable scenario name from `name:` in the YAML. Lets `ace show`
    /// answer "which scenario produced this log?" without forcing the reader
    /// to cross-reference filenames. Old logs deserialize fine via serde
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_name: Option<String>,
    /// Path to the scenario file (relative or absolute) that produced this
    /// log, recorded at run time. Pairs with `scenario_name` so `ace show`
    /// can render "trace from \"X\" (path/to/scenario.yaml)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_path: Option<String>,
}

impl Default for ExecutionLog {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            steps: Vec::new(),
            total_duration_ms: 0,
            total_steps: 0,
            passed: 0,
            failed: 0,
            iterations: 0,
            terminal_state: None,
            seed: 0,
            scenario_name: None,
            scenario_path: None,
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct StepLog {
    pub step_name: String,
    pub state_before: String,
    pub state_after: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub duration_ms: u64,
    pub assertions: Vec<AssertionResult>,
    /// Tag of the edge that fired the transition (if set on the edge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_edge_tag: Option<String>,
    /// Ordered list of branch names this step executed under (outermost first).
    /// None/empty = main line. Lets reporters group fan-out branch steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_path: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
    /// JSON-parsed response body with `mask:` rules applied. Present when the
    /// scenario defines at least one mask rule. Used by `ace diff` (P1.5+) to
    /// compare body content without per-request noise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body_normalized: Option<serde_json::Value>,
    /// JSONPath patterns from `mask:` rules that matched at least one node in
    /// this response. Used by `ace diff` to show "masked: x, y" in output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masked_fields: Vec<String>,
    /// Raw response headers. Populated only when the scenario defines at least
    /// one Header mask rule (so a future `--show-masked` can show pre-mask
    /// values) or when verbose mode was on. Always absent on non-verbose runs
    /// without header masking, so traces stay small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<std::collections::HashMap<String, String>>,
    /// Response headers with `mask:` rules applied. Same trigger as
    /// `response_headers`. Used by `ace diff` to compare header values without
    /// per-request noise (request IDs, timestamps, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers_normalized: Option<std::collections::HashMap<String, String>>,
    /// Header names from `mask:` rules that matched at least one header in
    /// this response. Used by `ace diff` to show "masked headers: x, y".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masked_headers: Vec<String>,
    /// Per-edge causality trace. Each entry says whether an outgoing edge
    /// matched, lost by priority/weight, or was rejected (and why). Empty
    /// on terminal/skipped steps. Old logs deserialize fine (serde default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edge_evaluations: Vec<EdgeEvaluation>,
    /// Explicit failure discriminant. Set when this step caused the run to
    /// end with a non-assertion error (no-match, max_takes, extraction
    /// missing, HTTP error). Absent on successful steps. Used by
    /// `result_from_log` to reconstruct the error without structural
    /// heuristics — avoids misclassifying self-loops as failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<StepFailure>,
}
