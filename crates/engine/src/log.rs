use crate::assertions::AssertionResult;
use crate::trace::EdgeEvaluation;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ExecutionLog {
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
    /// Per-edge causality trace. Each entry says whether an outgoing edge
    /// matched, lost by priority/weight, or was rejected (and why). Empty
    /// on terminal/skipped steps. Old logs deserialize fine (serde default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edge_evaluations: Vec<EdgeEvaluation>,
}
