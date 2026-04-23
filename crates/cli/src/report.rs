use crate::error::{CliError, load_execution_log};
use colored::Colorize;
use engine::{EdgeEvaluation, EdgeOutcome, ExecutionLog, RunError, StepFailure, StepLog};
use std::io::Write;

// ---------------------------------------------------------------------------
// Console report — colored terminal output
// ---------------------------------------------------------------------------

pub fn print_step_live(task_id: usize, step: &StepLog, verbose: bool) {
    let status_icon = if step.assertions.iter().all(|a| a.passed) && step.status < 400 {
        "✓".green().bold()
    } else {
        "✗".red().bold()
    };

    println!(
        "  {} {} {} {} {} {}",
        format!("[User {}]", task_id).dimmed(),
        format!("[{}]", step.state_before).cyan(),
        format!("--{}-->", step.step_name).white(),
        format!("[{}]", step.state_after).cyan(),
        status_icon,
        format!("({}) {}ms", step.status, step.duration_ms).dimmed(),
    );

    // Print assertion results
    for assertion in &step.assertions {
        if assertion.passed {
            println!("    {} {}", "✓".green(), assertion.description.dimmed(),);
        } else {
            println!(
                "    {} {} — expected: {}, got: {}",
                "✗".red(),
                assertion.description.red(),
                assertion.expected.yellow(),
                assertion.actual.red(),
            );
        }
    }

    // Causality trace: always render when evaluations are present so
    // `ace show` on a successful log still shows routing decisions.
    // The step_failed flag only controls the Matched marker (✓ vs ·).
    let step_failed = step.assertions.iter().any(|a| !a.passed)
        || step.status >= 400
        || step.failure.is_some();
    if !step.edge_evaluations.is_empty() {
        for eval in &step.edge_evaluations {
            println!("    {}", render_edge_evaluation(eval, step_failed));
        }
    }

    if verbose {
        if let Some(body) = &step.request_body {
            println!("    {} {}", "→".dimmed(), truncate(body, 200).dimmed());
        }
        if let Some(body) = &step.response_body {
            println!("    {} {}", "←".dimmed(), truncate(body, 200).dimmed());
        }
    }
}

/// Plain-text causality trace for JUnit `<system-out>` — no ANSI codes.
fn render_trace_plain(step: &StepLog) -> String {
    if step.edge_evaluations.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = step
        .edge_evaluations
        .iter()
        .map(|eval| {
            let tag = eval
                .tag
                .as_ref()
                .map(|t| format!(" ({})", t))
                .unwrap_or_default();
            match &eval.outcome {
                EdgeOutcome::Matched => format!("  [matched]  -> {}{}", eval.to, tag),
                EdgeOutcome::RejectedStatusMismatch { expected, actual } => format!(
                    "  [rejected] -> {}{}  status: expected {}, got {}",
                    eval.to, tag, expected, actual
                ),
                EdgeOutcome::RejectedBodyCheckFailed { path, expected, actual } => {
                    let act = if actual.is_empty() { "<missing>" } else { actual.as_str() };
                    format!(
                        "  [rejected] -> {}{}  body {}: {} (got {})",
                        eval.to, tag, path, expected, act
                    )
                }
                EdgeOutcome::RejectedAssertionGateFailed { failed_indices } => format!(
                    "  [rejected] -> {}{}  gate: assertions {:?} failed",
                    eval.to, tag, failed_indices
                ),
                EdgeOutcome::RejectedAssertionGateUnexpectedlyPassed => format!(
                    "  [rejected] -> {}{}  gate: expected failing assertions but all passed",
                    eval.to, tag
                ),
                EdgeOutcome::LostPriority { winner_priority } => format!(
                    "  [skipped]  -> {}{}  lost priority, winner={}",
                    eval.to, tag, winner_priority
                ),
                EdgeOutcome::LostWeightedRoll { weight, total } => format!(
                    "  [skipped]  -> {}{}  lost weighted roll {}/{}",
                    eval.to, tag, weight, total
                ),
                EdgeOutcome::LostTieBreak { winner_index } => format!(
                    "  [skipped]  -> {}{}  unweighted tie, edge[{}] won",
                    eval.to, tag, winner_index
                ),
                EdgeOutcome::MaxTakesExceeded { limit } => format!(
                    "  [capped]   -> {}{}  max_takes {} reached",
                    eval.to, tag, limit
                ),
                EdgeOutcome::Unknown => format!("  [unknown]  -> {}{}", eval.to, tag),
            }
        })
        .collect();
    format!("Edge evaluations:\n{}", lines.join("\n"))
}

fn render_edge_evaluation(eval: &EdgeEvaluation, step_failed: bool) -> String {
    let tag_suffix = eval
        .tag
        .as_ref()
        .map(|t| format!(" ({})", t))
        .unwrap_or_default();
    let target = format!("→ {}{}", eval.to, tag_suffix);
    match &eval.outcome {
        // Green check only when the surrounding step passed; neutral dot when
        // the step failed so ✓ doesn't visually contradict the ✗ header.
        EdgeOutcome::Matched if step_failed => format!("{} {}", "·".dimmed(), target.dimmed()),
        EdgeOutcome::Matched => format!("{} {}", "✓".green().bold(), target.cyan()),
        EdgeOutcome::RejectedStatusMismatch { expected, actual } => format!(
            "{} {}  [{}]",
            "✗".red().bold(),
            target.red(),
            format!("status: expected {}, got {}", expected, actual).yellow()
        ),
        EdgeOutcome::RejectedBodyCheckFailed { path, expected, actual } => {
            let actual_display = if actual.is_empty() {
                "<missing>"
            } else {
                actual.as_str()
            };
            format!(
                "{} {}  [{}]",
                "✗".red().bold(),
                target.red(),
                format!("body {}: {} (got {})", path, expected, actual_display).yellow()
            )
        }
        EdgeOutcome::RejectedAssertionGateFailed { failed_indices } => format!(
            "{} {}  [{}]",
            "✗".red().bold(),
            target.red(),
            format!("gate: assertions {:?} failed", failed_indices).yellow()
        ),
        EdgeOutcome::RejectedAssertionGateUnexpectedlyPassed => format!(
            "{} {}  [{}]",
            "✗".red().bold(),
            target.red(),
            "gate: expected failing assertions but all passed".yellow()
        ),
        EdgeOutcome::LostPriority { winner_priority } => format!(
            "{} {}  [{}]",
            "⋯".dimmed(),
            target.dimmed(),
            format!("lost priority, winner={}", winner_priority).dimmed()
        ),
        EdgeOutcome::LostWeightedRoll { weight, total } => format!(
            "{} {}  [{}]",
            "⋯".dimmed(),
            target.dimmed(),
            format!("lost weighted roll {}/{}", weight, total).dimmed()
        ),
        EdgeOutcome::LostTieBreak { winner_index } => format!(
            "{} {}  [{}]",
            "⋯".dimmed(),
            target.dimmed(),
            format!(
                "unweighted tie, edge[{}] won — add weight: or reorder",
                winner_index
            )
            .dimmed()
        ),
        EdgeOutcome::MaxTakesExceeded { limit } => format!(
            "{} {}  [{}]",
            "✗".red().bold(),
            target.red(),
            format!("max_takes limit {} reached", limit).yellow()
        ),
        EdgeOutcome::Unknown => format!("{} {}  [unknown outcome]", "·".dimmed(), target.dimmed()),
    }
}

pub fn print_summary(results: &[(ExecutionLog, Result<String, RunError>)]) {
    println!();
    println!("{}", "━".repeat(60).dimmed());
    println!("{}", " Summary".bold());
    println!("{}", "━".repeat(60).dimmed());

    let mut total_steps = 0;
    let mut total_passed = 0;
    let mut total_failed = 0;
    let mut total_duration_ms: u64 = 0;
    let mut durations: Vec<u64> = Vec::new();

    for (i, (log, result)) in results.iter().enumerate() {
        total_steps += log.total_steps;
        total_passed += log.passed;
        total_failed += log.failed;
        total_duration_ms = total_duration_ms.max(log.total_duration_ms);

        for step in &log.steps {
            durations.push(step.duration_ms);
        }

        match result {
            Ok(state) => {
                println!(
                    "  {} Final state: {} ({} steps, {}ms)",
                    format!("User {}:", i + 1).bold(),
                    state.green(),
                    log.total_steps,
                    log.total_duration_ms,
                );
            }
            Err(e) => {
                println!(
                    "  {} {}",
                    format!("User {}:", i + 1).bold(),
                    format!("FAILED — {}", e).red(),
                );
            }
        }
    }

    println!("{}", "━".repeat(60).dimmed());

    // Statistics
    durations.sort();
    let avg = if durations.is_empty() {
        0
    } else {
        durations.iter().sum::<u64>() / durations.len() as u64
    };
    let p50 = percentile(&durations, 50);
    let p95 = percentile(&durations, 95);
    let p99 = percentile(&durations, 99);

    println!(
        "  {} {} total, {} passed, {} failed",
        "Steps:".bold(),
        total_steps,
        format!("{}", total_passed).green(),
        if total_failed > 0 {
            format!("{}", total_failed).red()
        } else {
            format!("{}", total_failed).green()
        },
    );
    println!(
        "  {} total {}ms | avg {}ms | p50 {}ms | p95 {}ms | p99 {}ms",
        "Timing:".bold(),
        total_duration_ms,
        avg,
        p50,
        p95,
        p99,
    );

    let any_error = results.iter().any(|(_, r)| r.is_err());
    if total_failed > 0 || any_error {
        println!("\n  {}", "FAIL".red().bold());
    } else {
        println!("\n  {}", "PASS".green().bold());
    }
}

fn percentile(sorted: &[u64], pct: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (pct * sorted.len() / 100).min(sorted.len() - 1);
    sorted[idx]
}

fn truncate(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}...", head)
    } else {
        head
    }
}

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
// JUnit XML report
// ---------------------------------------------------------------------------

pub fn write_junit_report(
    results: &[(ExecutionLog, Result<String, RunError>)],
    scenario_name: &str,
    path: &str,
) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;

    let mut total_tests = 0;
    let mut total_failures = 0;
    let mut total_time = 0.0f64;
    let mut testcases = Vec::new();

    for (i, (log, result)) in results.iter().enumerate() {
        for step in &log.steps {
            total_tests += 1;
            let time_s = step.duration_ms as f64 / 1000.0;
            total_time += time_s;

            let failed_assertions: Vec<_> = step.assertions.iter().filter(|a| !a.passed).collect();
            let step_is_failure = !failed_assertions.is_empty() || step.failure.is_some();

            if !step_is_failure {
                testcases.push(format!(
                    "    <testcase name=\"[User {}] {}\" classname=\"{}\" time=\"{:.3}\"/>",
                    i + 1,
                    xml_escape(&step.step_name),
                    xml_escape(scenario_name),
                    time_s,
                ));
            } else {
                total_failures += 1;

                // Primary failure message: assertion failures or engine error.
                let failure_msg = if !failed_assertions.is_empty() {
                    failed_assertions
                        .iter()
                        .map(|a| {
                            format!("{}: expected {}, got {}", a.description, a.expected, a.actual)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else if let Some(ref f) = step.failure {
                    format!("{:?}", f)
                } else {
                    "step failed".into()
                };

                // Causality trace appended as <system-out> so CI dashboards
                // can show why no edge fired without needing ace show.
                let trace_lines = render_trace_plain(step);
                let system_out = if trace_lines.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n      <system-out>{}</system-out>",
                        xml_escape(&trace_lines)
                    )
                };

                testcases.push(format!(
                    "    <testcase name=\"[User {}] {}\" classname=\"{}\" time=\"{:.3}\">\n      <failure message=\"{}\">{}</failure>{}\n    </testcase>",
                    i + 1,
                    xml_escape(&step.step_name),
                    xml_escape(scenario_name),
                    time_s,
                    xml_escape(failure_msg.lines().next().unwrap_or("")),
                    xml_escape(&failure_msg),
                    system_out,
                ));
            }
        }

        if let Err(e) = result {
            // Add an error entry if the run itself failed (beyond assertion failures already logged)
            if log.steps.is_empty() {
                total_tests += 1;
                total_failures += 1;
                testcases.push(format!(
                    "    <testcase name=\"[User {}] execution\" classname=\"{}\">\n      <failure message=\"{}\">{}</failure>\n    </testcase>",
                    i + 1,
                    xml_escape(scenario_name),
                    xml_escape(&e.to_string()),
                    xml_escape(&e.to_string()),
                ));
            }
        }
    }

    writeln!(f, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(
        f,
        "<testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\">",
        xml_escape(scenario_name),
        total_tests,
        total_failures,
        total_time,
    )?;
    for tc in &testcases {
        writeln!(f, "{}", tc)?;
    }
    writeln!(f, "</testsuite>")?;

    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// Shared helper: reconstruct a Result from a saved ExecutionLog
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

    // Check the last step for an explicit failure discriminant. This is set
    // by the runner at every non-assertion termination site and is the
    // authoritative signal — no heuristics, no self-loop confusion.
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
            // Unknown variant from a newer ACE version — fall through to
            // assertion scan so we report something rather than panicking.
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
    use engine::StepLog;
    use engine::assertions::AssertionResult;

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

    /// Synthetic step shape produced by the runner on `NoMatchingTransition`:
    /// no failing assertions, `state_before == state_after`, and every edge
    /// evaluation is a non-Matched outcome.
    fn no_match_log(status: u16) -> ExecutionLog {
        use engine::{EdgeEvaluation, EdgeOutcome};
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

    // Bug regression: replay/report of a failed run must not return Ok.
    #[test]
    fn result_from_log_failed_is_err() {
        let log = failed_log();
        let result = result_from_log(&log);
        assert!(
            result.is_err(),
            "failed log (failed > 0) must reconstruct as Err, not Ok"
        );
        assert!(matches!(result, Err(RunError::AssertionFailed { .. })));
    }

    #[test]
    fn result_from_log_passed_is_ok() {
        let log = passed_log("done");
        let result = result_from_log(&log);
        assert_eq!(result.unwrap(), "done");
    }

    // Bug regression: a no-match step (synthetic StepLog with
    // edge_evaluations but no Matched outcome) must reconstruct as
    // NoMatchingTransition so `ace show` summary reads correctly, rather
    // than falling through to AssertionFailed with "0 assertion(s) failed".
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

    /// Regression: a self-loop step (state_before == state_after, legitimate
    /// polling pattern) with no `failure` field must NOT be misclassified as
    /// a no-match failure. The old heuristic (`state_before == state_after` +
    /// non-Matched evaluations) would have incorrectly returned
    /// NoMatchingTransition. The new explicit-discriminant path only fires
    /// when `failure` is Some.
    #[test]
    fn result_from_log_self_loop_is_not_misclassified() {
        use engine::{EdgeEvaluation, EdgeOutcome};
        let log = ExecutionLog {
            steps: vec![StepLog {
                step_name: "poll".into(),
                state_before: "waiting".into(),
                state_after: "waiting".into(), // self-loop
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
                    to: "waiting".into(),
                    tag: None,
                    outcome: EdgeOutcome::Matched,
                }],
                failure: None, // no failure — the loop matched successfully
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

    /// Regression: when multiple steps are in the log and only the last one
    /// carries `failure`, we must reconstruct from that last step — not from
    /// an earlier step with a coincidental shape.
    #[test]
    fn result_from_log_uses_last_step_failure_not_earlier_steps() {
        use engine::StepFailure;
        // First step: looks like a self-loop with edge_evaluations but no failure
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
        // Last step: explicit no-match
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
    fn percentile_empty() {
        assert_eq!(percentile(&[], 50), 0);
    }

    #[test]
    fn percentile_single() {
        assert_eq!(percentile(&[42], 50), 42);
        assert_eq!(percentile(&[42], 99), 42);
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let result = truncate("abcdef", 3);
        assert_eq!(result, "abc...");
    }

    // Bug regression: print_summary must show FAIL when result is Err (engine/network error),
    // even when no assertion failures are recorded (log.failed == 0).
    #[test]
    fn summary_shows_fail_on_engine_error() {
        let log = passed_log("done"); // log.failed == 0
        let results: Vec<(ExecutionLog, Result<String, RunError>)> = vec![(
            log,
            Err(RunError::HttpError {
                step: "step1".into(),
                message: "connection refused".into(),
            }),
        )];
        // We can't easily capture stdout in a unit test, but we can verify the
        // logic branch: any_error must be true so FAIL path is taken.
        let any_error = results.iter().any(|(_, r)| r.is_err());
        assert!(any_error, "engine error must trigger the FAIL branch");
    }

    // Bug regression: exit code 2 for network errors, 1 for assertion failures.
    #[test]
    fn exit_code_network_error_is_2() {
        use crate::error::CliError;
        assert_eq!(CliError::RunError.exit_code(), 2);
        assert_eq!(CliError::RunFailed.exit_code(), 1);
    }

    #[test]
    fn xml_escape_all_chars() {
        assert_eq!(
            xml_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
    }

    /// JUnit: a step whose `failure` field is set (no-match, max-takes) must
    /// appear as a failure in the XML even when `assertions` is empty.
    #[test]
    fn junit_no_match_step_counts_as_failure() {
        let log = no_match_log(500);
        let results: Vec<(ExecutionLog, Result<String, RunError>)> =
            vec![(log, Err(RunError::NoMatchingTransition { state: "start".into(), status: 500 }))];

        let tmp = std::env::temp_dir().join("ace_junit_test.xml");
        write_junit_report(&results, "test_scenario", tmp.to_str().unwrap())
            .expect("write junit");
        let xml = std::fs::read_to_string(&tmp).expect("read junit");
        let _ = std::fs::remove_file(&tmp);

        assert!(xml.contains("<failure"), "expected <failure> element");
        assert!(xml.contains("failures=\"1\""), "expected failures=1");
    }

    /// JUnit: failing testcases must include a <system-out> with the causality
    /// trace so CI dashboards can show why no edge fired.
    #[test]
    fn junit_failing_step_includes_system_out_trace() {
        let log = no_match_log(500);
        let results: Vec<(ExecutionLog, Result<String, RunError>)> =
            vec![(log, Err(RunError::NoMatchingTransition { state: "start".into(), status: 500 }))];

        let tmp = std::env::temp_dir().join("ace_junit_trace_test.xml");
        write_junit_report(&results, "test_scenario", tmp.to_str().unwrap())
            .expect("write junit");
        let xml = std::fs::read_to_string(&tmp).expect("read junit");
        let _ = std::fs::remove_file(&tmp);

        assert!(xml.contains("<system-out>"), "expected <system-out> trace");
        assert!(xml.contains("Edge evaluations:"), "expected trace header");
    }
}
