use crate::render::render_trace_plain;
use engine::{ExecutionLog, RunError};
use std::io::Write;

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

                let failure_msg = if !failed_assertions.is_empty() {
                    failed_assertions
                        .iter()
                        .map(|a| format!("{}: expected {}, got {}", a.description, a.expected, a.actual))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else if let Some(ref f) = step.failure {
                    format!("{:?}", f)
                } else {
                    "step failed".into()
                };

                let trace_lines = render_trace_plain(step);
                let system_out = if trace_lines.is_empty() {
                    String::new()
                } else {
                    format!("\n      <system-out>{}</system-out>", xml_escape(&trace_lines))
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

        if let Err(e) = result
            && log.steps.is_empty()
        {
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

pub(crate) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::{EdgeEvaluation, EdgeOutcome, ExecutionLog, StepFailure, StepLog};

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
    fn xml_escape_all_chars() {
        assert_eq!(
            xml_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
    }

    #[test]
    fn no_match_step_counts_as_failure() {
        let log = no_match_log(500);
        let results: Vec<(ExecutionLog, Result<String, engine::RunError>)> = vec![(
            log,
            Err(engine::RunError::NoMatchingTransition { state: "start".into(), status: 500 }),
        )];
        let tmp = std::env::temp_dir().join("ace_junit_test.xml");
        write_junit_report(&results, "test_scenario", tmp.to_str().unwrap()).expect("write junit");
        let xml = std::fs::read_to_string(&tmp).expect("read junit");
        let _ = std::fs::remove_file(&tmp);
        assert!(xml.contains("<failure"), "expected <failure> element");
        assert!(xml.contains("failures=\"1\""), "expected failures=1");
    }

    #[test]
    fn failing_step_includes_system_out_trace() {
        let log = no_match_log(500);
        let results: Vec<(ExecutionLog, Result<String, engine::RunError>)> = vec![(
            log,
            Err(engine::RunError::NoMatchingTransition { state: "start".into(), status: 500 }),
        )];
        let tmp = std::env::temp_dir().join("ace_junit_trace_test.xml");
        write_junit_report(&results, "test_scenario", tmp.to_str().unwrap()).expect("write junit");
        let xml = std::fs::read_to_string(&tmp).expect("read junit");
        let _ = std::fs::remove_file(&tmp);
        assert!(xml.contains("<system-out>"), "expected <system-out> trace");
        assert!(xml.contains("Edge evaluations:"), "expected trace header");
    }
}
