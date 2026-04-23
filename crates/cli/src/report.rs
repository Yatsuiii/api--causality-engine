use crate::error::{CliError, load_execution_log};
use colored::Colorize;
use engine::{ExecutionLog, RunError, StepFailure};

// Re-export rendering and JUnit so callers use `report::` uniformly.
pub use crate::junit::write_junit_report;
pub use crate::render::{print_step_live, print_summary};

// ---------------------------------------------------------------------------
// JSON report
// ---------------------------------------------------------------------------

pub fn write_json_report(
    results: &[(ExecutionLog, Result<String, RunError>)],
    path: &str,
) -> std::io::Result<()> {
    let logs: Vec<_> = results.iter().map(|(log, _)| log).collect();
    let json = serde_json::to_string_pretty(&logs).expect("Failed to serialize logs");
    std::fs::write(path, &json)
}

// ---------------------------------------------------------------------------
// Reconstruct a Result from a saved ExecutionLog
// ---------------------------------------------------------------------------

/// Rebuild the `Result` side of a `(ExecutionLog, Result<…>)` pair from
/// persisted data. Reconstruction order:
/// 1. `log.failed == 0` → `Ok(state_after of last step)`
/// 2. Last step carries `failure` discriminant → reconstruct specific error
/// 3. Any step has failed assertions → `AssertionFailed`
/// 4. Fallback → `AssertionFailed` with empty failures (shouldn't happen)
pub fn result_from_log(log: &ExecutionLog) -> Result<String, RunError> {
    let final_state = log
        .steps
        .last()
        .map(|s| s.state_after.clone())
        .unwrap_or_else(|| "unknown".into());

    if log.failed == 0 {
        return Ok(final_state);
    }

    if let Some(step) = log.steps.last()
        && let Some(ref f) = step.failure
    {
        match f {
            StepFailure::NoMatch => {
                return Err(RunError::NoMatchingTransition {
                    state: step.state_before.clone(),
                    status: step.status,
                });
            }
            StepFailure::MaxTakesExceeded { to, limit } => {
                return Err(RunError::EdgeMaxTakesExceeded {
                    state: step.state_before.clone(),
                    to: to.clone(),
                    limit: *limit,
                });
            }
            StepFailure::ExtractionMissing { key, path } => {
                return Err(RunError::ExtractionMissing {
                    step: step.step_name.clone(),
                    key: key.clone(),
                    path: path.clone(),
                });
            }
            StepFailure::HttpError { message } => {
                return Err(RunError::HttpError {
                    step: step.step_name.clone(),
                    message: message.clone(),
                });
            }
            StepFailure::Unknown => {}
        }
    }

    let failures: Vec<_> = log
        .steps
        .iter()
        .flat_map(|s| s.assertions.iter().filter(|a| !a.passed).cloned())
        .collect();
    let step = log
        .steps
        .iter()
        .find(|s| s.assertions.iter().any(|a| !a.passed))
        .map(|s| s.step_name.clone())
        .unwrap_or_else(|| "unknown".into());
    Err(RunError::AssertionFailed { step, failures })
}

// ---------------------------------------------------------------------------
// `report` subcommand
// ---------------------------------------------------------------------------

pub fn cmd_report(log_path: &str, format: &str, output: Option<String>) -> Result<(), CliError> {
    let logs = load_execution_log(log_path)?;

    let results: Vec<(ExecutionLog, Result<String, RunError>)> = logs
        .into_iter()
        .map(|log| {
            let result = result_from_log(&log);
            (log, result)
        })
        .collect();

    match format {
        "json" => {
            let out_path = output.unwrap_or_else(|| "report.json".into());
            write_json_report(&results, &out_path).map_err(|e| CliError::Io {
                path: out_path.clone(),
                source: e,
            })?;
            println!("{} {}", "Report written:".green().bold(), out_path);
        }
        "junit" => {
            let out_path = output.unwrap_or_else(|| "report.xml".into());
            write_junit_report(&results, "scenario", &out_path).map_err(|e| CliError::Io {
                path: out_path.clone(),
                source: e,
            })?;
            println!("{} {}", "Report written:".green().bold(), out_path);
        }
        other => {
            return Err(CliError::BadArgument(format!(
                "Unknown format '{}'. Use 'json' or 'junit'.",
                other
            )));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use engine::assertions::AssertionResult;
    use engine::{EdgeEvaluation, EdgeOutcome, StepFailure, StepLog};

    fn passed_log(state_after: &str) -> ExecutionLog {
        ExecutionLog {
            steps: vec![StepLog {
                step_name: "step1".into(),
                state_before: "start".into(),
                state_after: state_after.into(),
                method: "GET".into(),
                url: "http://example.com".into(),
                status: 200,
                duration_ms: 50,
                assertions: vec![AssertionResult {
                    description: "status == 200".into(),
                    passed: true,
                    expected: "200".into(),
                    actual: "200".into(),
                }],
                matched_edge_tag: None,
                branch_path: None,
                request_body: None,
                response_body: None,
                edge_evaluations: Vec::new(),
                failure: None,
            }],
            total_duration_ms: 50,
            total_steps: 1,
            passed: 1,
            failed: 0,
            iterations: 1,
            terminal_state: Some(state_after.into()),
            seed: 0,
            schema_version: 1,
        }
    }

    fn failed_log() -> ExecutionLog {
        ExecutionLog {
            steps: vec![StepLog {
                step_name: "step1".into(),
                state_before: "start".into(),
                state_after: "done".into(),
                method: "GET".into(),
                url: "http://example.com".into(),
                status: 404,
                duration_ms: 50,
                assertions: vec![AssertionResult {
                    description: "status == 200".into(),
                    passed: false,
                    expected: "200".into(),
                    actual: "404".into(),
                }],
                matched_edge_tag: None,
                branch_path: None,
                request_body: None,
                response_body: None,
                edge_evaluations: Vec::new(),
                failure: None,
            }],
            total_duration_ms: 50,
            total_steps: 1,
            passed: 0,
            failed: 1,
            iterations: 1,
            terminal_state: None,
            seed: 0,
            schema_version: 1,
        }
    }

    fn no_match_log(status: u16) -> ExecutionLog {
        ExecutionLog {
            steps: vec![StepLog {
                step_name: "call".into(),
                state_before: "start".into(),
                state_after: "start".into(),
                method: "GET".into(),
                url: "http://example.com".into(),
                status,
                duration_ms: 10,
                assertions: Vec::new(),
                matched_edge_tag: None,
                branch_path: None,
                request_body: None,
                response_body: None,
                edge_evaluations: vec![EdgeEvaluation {
                    edge_id: String::new(),
                    to: "done".into(),
                    tag: None,
                    outcome: EdgeOutcome::RejectedStatusMismatch {
                        expected: "200".into(),
                        actual: status,
                    },
                }],
                failure: Some(StepFailure::NoMatch),
            }],
            total_duration_ms: 10,
            total_steps: 1,
            passed: 0,
            failed: 1,
            iterations: 1,
            terminal_state: None,
            seed: 0,
            schema_version: 1,
        }
    }

    #[test]
    fn result_from_log_failed_is_err() {
        let log = failed_log();
        let result = result_from_log(&log);
        assert!(result.is_err(), "failed log must reconstruct as Err");
        assert!(matches!(result, Err(RunError::AssertionFailed { .. })));
    }

    #[test]
    fn result_from_log_passed_is_ok() {
        assert_eq!(result_from_log(&passed_log("done")).unwrap(), "done");
    }

    #[test]
    fn result_from_log_reconstructs_no_match_from_evaluations() {
        let log = no_match_log(500);
        match result_from_log(&log) {
            Err(RunError::NoMatchingTransition { state, status }) => {
                assert_eq!(state, "start");
                assert_eq!(status, 500);
            }
            other => panic!("expected NoMatchingTransition, got {:?}", other),
        }
    }

    /// Self-loop with failure:None must not be misclassified as no-match.
    #[test]
    fn result_from_log_self_loop_is_not_misclassified() {
        let log = ExecutionLog {
            steps: vec![StepLog {
                step_name: "poll".into(),
                state_before: "waiting".into(),
                state_after: "waiting".into(),
                method: "GET".into(),
                url: "http://example.com/status".into(),
                status: 200,
                duration_ms: 20,
                assertions: Vec::new(),
                matched_edge_tag: None,
                branch_path: None,
                request_body: None,
                response_body: None,
                edge_evaluations: vec![EdgeEvaluation {
                    edge_id: String::new(),
                    to: "waiting".into(),
                    tag: None,
                    outcome: EdgeOutcome::Matched,
                }],
                failure: None,
            }],
            total_duration_ms: 20,
            total_steps: 1,
            passed: 1,
            failed: 0,
            iterations: 1,
            terminal_state: None,
            seed: 0,
            schema_version: 1,
        };
        assert!(
            result_from_log(&log).is_ok(),
            "self-loop with failure:None must be Ok"
        );
    }

    /// result_from_log uses the last step's failure, not any earlier step.
    #[test]
    fn result_from_log_uses_last_step_failure_not_earlier_steps() {
        let step1 = StepLog {
            step_name: "poll".into(),
            state_before: "waiting".into(),
            state_after: "waiting".into(),
            method: "GET".into(),
            url: "http://example.com".into(),
            status: 200,
            duration_ms: 10,
            assertions: Vec::new(),
            matched_edge_tag: None,
            branch_path: None,
            request_body: None,
            response_body: None,
            edge_evaluations: Vec::new(),
            failure: None,
        };
        let step2 = StepLog {
            step_name: "call".into(),
            state_before: "active".into(),
            state_after: "active".into(),
            method: "GET".into(),
            url: "http://example.com".into(),
            status: 503,
            duration_ms: 10,
            assertions: Vec::new(),
            matched_edge_tag: None,
            branch_path: None,
            request_body: None,
            response_body: None,
            edge_evaluations: Vec::new(),
            failure: Some(StepFailure::NoMatch),
        };
        let log = ExecutionLog {
            steps: vec![step1, step2],
            total_duration_ms: 20,
            total_steps: 2,
            passed: 1,
            failed: 1,
            iterations: 2,
            terminal_state: None,
            seed: 0,
            schema_version: 1,
        };
        match result_from_log(&log) {
            Err(RunError::NoMatchingTransition { state, status }) => {
                assert_eq!(state, "active");
                assert_eq!(status, 503);
            }
            other => panic!(
                "expected NoMatchingTransition from last step, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn result_from_log_empty_log_is_ok_unknown() {
        let log = ExecutionLog {
            steps: vec![],
            total_duration_ms: 0,
            total_steps: 0,
            passed: 0,
            failed: 0,
            iterations: 0,
            terminal_state: None,
            seed: 0,
            schema_version: 1,
        };
        assert_eq!(result_from_log(&log).unwrap(), "unknown");
    }

    #[test]
    fn exit_code_network_error_is_2() {
        use crate::error::CliError;
        assert_eq!(CliError::RunError.exit_code(), 2);
        assert_eq!(CliError::RunFailed.exit_code(), 1);
    }
}
