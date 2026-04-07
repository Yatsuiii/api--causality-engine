use crate::error::{CliError, load_execution_log};
use colored::Colorize;
use runner::{ExecutionLog, RunError, StepLog};
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

    if total_failed > 0 {
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
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

            if failed_assertions.is_empty() {
                testcases.push(format!(
                    "    <testcase name=\"[User {}] {}\" classname=\"{}\" time=\"{:.3}\"/>",
                    i + 1,
                    xml_escape(&step.step_name),
                    xml_escape(scenario_name),
                    time_s,
                ));
            } else {
                total_failures += 1;
                let msg: Vec<_> = failed_assertions
                    .iter()
                    .map(|a| {
                        format!(
                            "{}: expected {}, got {}",
                            a.description, a.expected, a.actual
                        )
                    })
                    .collect();
                testcases.push(format!(
                    "    <testcase name=\"[User {}] {}\" classname=\"{}\" time=\"{:.3}\">\n      <failure message=\"{}\">{}</failure>\n    </testcase>",
                    i + 1,
                    xml_escape(&step.step_name),
                    xml_escape(scenario_name),
                    time_s,
                    xml_escape(&msg[0]),
                    xml_escape(&msg.join("\n")),
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
// `report` subcommand
// ---------------------------------------------------------------------------

pub fn cmd_report(log_path: &str, format: &str, output: Option<String>) -> Result<(), CliError> {
    let logs = load_execution_log(log_path)?;

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
