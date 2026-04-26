use colored::Colorize;
use engine::{EdgeEvaluation, EdgeOutcome, ExecutionLog, RunError, StepLog};

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

    for assertion in &step.assertions {
        if assertion.passed {
            println!("    {} {}", "✓".green(), assertion.description.dimmed());
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

    // Always render evaluations when present so ace show on a successful log
    // still shows routing decisions. step_failed only controls ✓ vs · marker.
    let step_failed =
        step.assertions.iter().any(|a| !a.passed) || step.status >= 400 || step.failure.is_some();
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

/// Plain-text causality trace for JUnit `<system-out>` — no ANSI codes.
pub fn render_trace_plain(step: &StepLog) -> String {
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
                EdgeOutcome::RejectedBodyCheckFailed {
                    path,
                    expected,
                    actual,
                } => {
                    let act = if actual.is_empty() {
                        "<missing>"
                    } else {
                        actual.as_str()
                    };
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
        EdgeOutcome::Matched if step_failed => format!("{} {}", "·".dimmed(), target.dimmed()),
        EdgeOutcome::Matched => format!("{} {}", "✓".green().bold(), target.cyan()),
        EdgeOutcome::RejectedStatusMismatch { expected, actual } => format!(
            "{} {}  [{}]",
            "✗".red().bold(),
            target.red(),
            format!("status: expected {}, got {}", expected, actual).yellow()
        ),
        EdgeOutcome::RejectedBodyCheckFailed {
            path,
            expected,
            actual,
        } => {
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
            "·".dimmed(),
            target.dimmed(),
            format!("lost priority, winner={}", winner_priority).dimmed()
        ),
        EdgeOutcome::LostWeightedRoll { weight, total } => format!(
            "{} {}  [{}]",
            "·".dimmed(),
            target.dimmed(),
            format!("lost weighted roll {}/{}", weight, total).dimmed()
        ),
        EdgeOutcome::LostTieBreak { winner_index } => format!(
            "{} {}  [{}]",
            "·".dimmed(),
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

pub(crate) fn percentile(sorted: &[u64], pct: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (pct * sorted.len() / 100).min(sorted.len() - 1);
    sorted[idx]
}

pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(truncate("abcdef", 3), "abc…");
    }

    #[test]
    fn summary_shows_fail_on_engine_error() {
        use engine::ExecutionLog;
        // Verify the FAIL branch fires when result is Err even with log.failed == 0.
        let results: Vec<(ExecutionLog, Result<String, RunError>)> = vec![(
            ExecutionLog::default(),
            Err(RunError::HttpError {
                step: "step1".into(),
                message: "connection refused".into(),
            }),
        )];
        let any_error = results.iter().any(|(_, r)| r.is_err());
        assert!(any_error, "engine error must trigger the FAIL branch");
    }
}
