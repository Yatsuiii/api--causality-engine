use crate::error::CliError;
use engine::assertions::AssertionResult;
use engine::trace::{EdgeEvaluation, EdgeOutcome};
use engine::{ExecutionLog, StepLog};
use serde::Serialize;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn cmd_diff(a: &str, b: &str, format: &str, output: Option<String>) -> Result<(), CliError> {
    let format = DiffFormat::parse(format)?;
    let logs_a = load_logs(a)?;
    let logs_b = load_logs(b)?;

    let pairs = align_logs(&logs_a, &logs_b);
    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut total_steps = 0usize;

    for pair in &pairs {
        let step_pairs = align_steps(&pair.a.steps, &pair.b.steps);
        total_steps += step_pairs.len();
        for sp in &step_pairs {
            let mut divs = diff_step(pair.user_idx, sp);
            all_divergences.append(&mut divs);
        }
    }

    let text = match format {
        DiffFormat::Json => render_json_output(&all_divergences, total_steps),
        DiffFormat::Text => render_text(&all_divergences, total_steps),
    };

    match output {
        Some(ref path) => std::fs::write(path, &text).map_err(|e| CliError::Io {
            path: path.clone(),
            source: e,
        })?,
        None => print!("{}", text),
    }

    if all_divergences.is_empty() {
        Ok(())
    } else {
        Err(CliError::DiffFound)
    }
}

#[derive(Clone, Copy)]
enum DiffFormat {
    Text,
    Json,
}

impl DiffFormat {
    fn parse(raw: &str) -> Result<Self, CliError> {
        match raw {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(CliError::BadArgument(format!(
                "invalid diff format '{other}' (expected 'text' or 'json')"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

fn load_logs(path: &str) -> Result<Vec<ExecutionLog>, CliError> {
    let raw = std::fs::read_to_string(path).map_err(|e| CliError::Io {
        path: path.to_string(),
        source: e,
    })?;
    // Accept both a single ExecutionLog and an array.
    if raw.trim_start().starts_with('[') {
        serde_json::from_str::<Vec<ExecutionLog>>(&raw)
            .map_err(|e| CliError::BadArgument(format!("invalid log {path}: {e}")))
    } else {
        let single: ExecutionLog = serde_json::from_str(&raw)
            .map_err(|e| CliError::BadArgument(format!("invalid log {path}: {e}")))?;
        Ok(vec![single])
    }
}

// ---------------------------------------------------------------------------
// Alignment
// ---------------------------------------------------------------------------

struct UserPair<'a> {
    user_idx: usize,
    a: &'a ExecutionLog,
    b: &'a ExecutionLog,
}

fn align_logs<'a>(a: &'a [ExecutionLog], b: &'a [ExecutionLog]) -> Vec<UserPair<'a>> {
    let count = a.len().min(b.len());
    if a.len() != b.len() {
        eprintln!(
            "warning: trace A has {} user(s), trace B has {} — diffing {} overlap",
            a.len(),
            b.len(),
            count
        );
    }
    (0..count)
        .map(|i| UserPair {
            user_idx: i + 1,
            a: &a[i],
            b: &b[i],
        })
        .collect()
}

struct StepPair<'a> {
    step_name: String,
    occurrence: usize,
    a: Option<&'a StepLog>,
    b: Option<&'a StepLog>,
}

fn align_steps<'a>(a: &'a [StepLog], b: &'a [StepLog]) -> Vec<StepPair<'a>> {
    // Build occurrence-indexed maps: (step_name, occurrence_idx) -> &StepLog
    let mut map_a: HashMap<(String, usize), &StepLog> = HashMap::new();
    let mut map_b: HashMap<(String, usize), &StepLog> = HashMap::new();
    let mut occ_a: HashMap<String, usize> = HashMap::new();
    let mut occ_b: HashMap<String, usize> = HashMap::new();

    for step in a {
        let occ = occ_a.entry(step.step_name.clone()).or_insert(0);
        map_a.insert((step.step_name.clone(), *occ), step);
        *occ += 1;
    }
    for step in b {
        let occ = occ_b.entry(step.step_name.clone()).or_insert(0);
        map_b.insert((step.step_name.clone(), *occ), step);
        *occ += 1;
    }

    // Union of all keys
    let mut keys: Vec<(String, usize)> =
        map_a.keys().cloned().chain(map_b.keys().cloned()).collect();
    keys.sort();
    keys.dedup();

    keys.into_iter()
        .map(|(name, occ): (String, usize)| StepPair {
            step_name: name.clone(),
            occurrence: occ,
            a: map_a.get(&(name.clone(), occ)).copied(),
            b: map_b.get(&(name, occ)).copied(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Divergence types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Divergence {
    pub user: usize,
    pub step: String,
    pub occurrence: usize,
    pub kind: DivergenceKind,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DivergenceKind {
    StepMissingInA {
        step: String,
    },
    StepMissingInB {
        step: String,
    },
    RoutingDiverged {
        a: RouteInfo,
        b: RouteInfo,
    },
    RejectionReasonChanged {
        edge_id: String,
        a_reason: String,
        b_reason: String,
    },
    OutcomeDiverged {
        a_outcome: String,
        b_outcome: String,
    },
    EdgeOnlyInA {
        edge_id: String,
        to: String,
    },
    EdgeOnlyInB {
        edge_id: String,
        to: String,
    },
}

#[derive(Debug, Serialize)]
pub struct RouteInfo {
    pub matched_edge_id: String,
    pub to: String,
    pub rejected: Vec<RejectedEdge>,
}

#[derive(Debug, Serialize)]
pub struct RejectedEdge {
    pub edge_id: String,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Per-step diffing
// ---------------------------------------------------------------------------

fn diff_step(user_idx: usize, sp: &StepPair) -> Vec<Divergence> {
    let mut out = Vec::new();
    let key = (user_idx, sp.step_name.clone(), sp.occurrence);

    match (sp.a, sp.b) {
        (None, _) => {
            out.push(Divergence {
                user: key.0,
                step: key.1.clone(),
                occurrence: key.2,
                kind: DivergenceKind::StepMissingInA { step: key.1 },
            });
            return out;
        }
        (_, None) => {
            out.push(Divergence {
                user: key.0,
                step: key.1.clone(),
                occurrence: key.2,
                kind: DivergenceKind::StepMissingInB { step: key.1 },
            });
            return out;
        }
        (Some(a), Some(b)) => {
            let a_matched = matched_edge(a);
            let b_matched = matched_edge(b);

            match (a_matched, b_matched) {
                (None, None) => {
                    // Both failed — check if for the same reason
                    let a_out = outcome_summary_for_step(a);
                    let b_out = outcome_summary_for_step(b);
                    if a_out != b_out {
                        out.push(Divergence {
                            user: key.0,
                            step: key.1,
                            occurrence: key.2,
                            kind: DivergenceKind::OutcomeDiverged {
                                a_outcome: a_out,
                                b_outcome: b_out,
                            },
                        });
                    }
                }
                (Some(a_ev), Some(b_ev)) => {
                    let a_id = effective_id(a_ev, a);
                    let b_id = effective_id(b_ev, b);
                    if a_id != b_id {
                        // Routing diverged — different winner
                        out.push(Divergence {
                            user: key.0,
                            step: key.1,
                            occurrence: key.2,
                            kind: DivergenceKind::RoutingDiverged {
                                a: build_route_info(a),
                                b: build_route_info(b),
                            },
                        });
                    } else {
                        // Same winner — check for same-edge rejection reason differences
                        let a_rejects = rejection_map(a);
                        let b_rejects = rejection_map(b);
                        let mut all_ids: Vec<String> = a_rejects
                            .keys()
                            .cloned()
                            .chain(b_rejects.keys().cloned())
                            .collect();
                        all_ids.sort();
                        all_ids.dedup();
                        for eid in all_ids {
                            match (a_rejects.get(&eid), b_rejects.get(&eid)) {
                                (Some(ar), Some(br)) if ar != br => {
                                    out.push(Divergence {
                                        user: key.0,
                                        step: key.1.clone(),
                                        occurrence: key.2,
                                        kind: DivergenceKind::RejectionReasonChanged {
                                            edge_id: eid,
                                            a_reason: ar.clone(),
                                            b_reason: br.clone(),
                                        },
                                    });
                                }
                                (Some(ar), None) => {
                                    out.push(Divergence {
                                        user: key.0,
                                        step: key.1.clone(),
                                        occurrence: key.2,
                                        kind: DivergenceKind::EdgeOnlyInA {
                                            edge_id: eid,
                                            to: ar.clone(),
                                        },
                                    });
                                }
                                (None, Some(br)) => {
                                    out.push(Divergence {
                                        user: key.0,
                                        step: key.1.clone(),
                                        occurrence: key.2,
                                        kind: DivergenceKind::EdgeOnlyInB {
                                            edge_id: eid,
                                            to: br.clone(),
                                        },
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
                (Some(_), None) | (None, Some(_)) => {
                    out.push(Divergence {
                        user: key.0,
                        step: key.1,
                        occurrence: key.2,
                        kind: DivergenceKind::RoutingDiverged {
                            a: build_route_info(a),
                            b: build_route_info(b),
                        },
                    });
                }
            }
        }
    }

    out
}

fn matched_edge(step: &StepLog) -> Option<&EdgeEvaluation> {
    step.edge_evaluations
        .iter()
        .find(|e| matches!(e.outcome, EdgeOutcome::Matched))
}

fn effective_id(ev: &EdgeEvaluation, step: &StepLog) -> String {
    if !ev.edge_id.is_empty() {
        ev.edge_id.clone()
    } else {
        fallback_edge_id(step, ev)
    }
}

fn fallback_edge_id(step: &StepLog, ev: &EdgeEvaluation) -> String {
    format!(
        "{}:{}:{}",
        step.state_before,
        ev.to,
        ev.tag.as_deref().unwrap_or("")
    )
}

fn outcome_summary_for_step(step: &StepLog) -> String {
    if let Some(f) = &step.failure {
        format!("{:?}", f)
    } else {
        "no_match".into()
    }
}

fn build_route_info(step: &StepLog) -> RouteInfo {
    let matched = matched_edge(step);
    let rejected: Vec<RejectedEdge> = step
        .edge_evaluations
        .iter()
        .filter(|e| !matches!(e.outcome, EdgeOutcome::Matched))
        .map(|e| RejectedEdge {
            edge_id: if e.edge_id.is_empty() {
                fallback_edge_id(step, e)
            } else {
                e.edge_id.clone()
            },
            reason: outcome_reason(&e.outcome, &step.assertions),
        })
        .collect();
    RouteInfo {
        matched_edge_id: matched.map(|e| effective_id(e, step)).unwrap_or_default(),
        to: matched.map(|e| e.to.clone()).unwrap_or_default(),
        rejected,
    }
}

fn rejection_map(step: &StepLog) -> HashMap<String, String> {
    step.edge_evaluations
        .iter()
        .filter(|e| !matches!(e.outcome, EdgeOutcome::Matched))
        .map(|e| {
            let id = if e.edge_id.is_empty() {
                fallback_edge_id(step, e)
            } else {
                e.edge_id.clone()
            };
            (id, outcome_reason(&e.outcome, &step.assertions))
        })
        .collect()
}

fn outcome_reason(o: &EdgeOutcome, assertions: &[AssertionResult]) -> String {
    match o {
        EdgeOutcome::RejectedStatusMismatch { expected, actual } => {
            format!("status: expected {expected}, got {actual}")
        }
        EdgeOutcome::RejectedBodyCheckFailed {
            path,
            expected,
            actual,
        } => format!("body {path}: expected {expected}, got \"{actual}\""),
        EdgeOutcome::RejectedAssertionGateFailed { failed_indices } => {
            format_failed_assertions(failed_indices, assertions)
        }
        EdgeOutcome::RejectedAssertionGateUnexpectedlyPassed => {
            "assertion gate: expected failure but all passed".into()
        }
        EdgeOutcome::LostPriority { winner_priority } => {
            format!("lost priority (winner={winner_priority})")
        }
        EdgeOutcome::LostWeightedRoll { weight, total } => {
            format!("lost weighted roll ({weight}/{total})")
        }
        EdgeOutcome::LostTieBreak { winner_index } => {
            format!("lost tie-break (winner index {winner_index})")
        }
        EdgeOutcome::MaxTakesExceeded { limit } => format!("max_takes={limit} exhausted"),
        EdgeOutcome::Matched => "matched".into(),
        EdgeOutcome::Unknown => "unknown".into(),
    }
}

/// Resolve failed-assertion indices into human-readable descriptions.
///
/// Historical format was `assertions failed: [1, 3]`. Indices alone force the
/// reader to cross-reference the trace to know which assertion broke — a
/// painful extra step when `ace diff` is the only output on screen. With the
/// AssertionResult slice in hand we can render the actual failure text.
fn format_failed_assertions(failed_indices: &[usize], assertions: &[AssertionResult]) -> String {
    if failed_indices.is_empty() {
        return "assertions failed".into();
    }
    let parts: Vec<String> = failed_indices
        .iter()
        .map(|i| match assertions.get(*i) {
            Some(a) => {
                let actual = if a.actual.is_empty() {
                    "<missing>"
                } else {
                    a.actual.as_str()
                };
                format!(
                    "{} (expected {}, got {})",
                    a.description, a.expected, actual
                )
            }
            None => format!("assertion[{i}]"),
        })
        .collect();
    format!("assertions failed: {}", parts.join("; "))
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_text(divergences: &[Divergence], total_steps: usize) -> String {
    if divergences.is_empty() {
        return format!("no divergences across {total_steps} step(s).\n");
    }

    let mut out = String::new();
    for d in divergences {
        let occ_label = if d.occurrence > 0 {
            format!(" [{}]", d.occurrence)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "User {} / step \"{}\"{}\n",
            d.user, d.step, occ_label
        ));
        match &d.kind {
            DivergenceKind::StepMissingInA { .. } => out.push_str("  ✗ step absent in trace-a\n"),
            DivergenceKind::StepMissingInB { .. } => out.push_str("  ✗ step absent in trace-b\n"),
            DivergenceKind::OutcomeDiverged {
                a_outcome,
                b_outcome,
            } => {
                out.push_str("  ↯ outcome diverged\n");
                out.push_str(&format!("      trace-a: {a_outcome}\n"));
                out.push_str(&format!("      trace-b: {b_outcome}\n"));
            }
            DivergenceKind::RoutingDiverged { a, b } => {
                out.push_str("  ↯ routing diverged\n");
                if !a.matched_edge_id.is_empty() {
                    out.push_str(&format!(
                        "      trace-a: matched edge {} → {}\n",
                        a.matched_edge_id, a.to
                    ));
                } else {
                    out.push_str("      trace-a: no match\n");
                }
                for r in &a.rejected {
                    out.push_str(&format!(
                        "               rejected edge {}  [{}]\n",
                        r.edge_id, r.reason
                    ));
                }
                if !b.matched_edge_id.is_empty() {
                    out.push_str(&format!(
                        "      trace-b: matched edge {} → {}\n",
                        b.matched_edge_id, b.to
                    ));
                } else {
                    out.push_str("      trace-b: no match\n");
                }
                for r in &b.rejected {
                    out.push_str(&format!(
                        "               rejected edge {}  [{}]\n",
                        r.edge_id, r.reason
                    ));
                }
            }
            DivergenceKind::RejectionReasonChanged {
                edge_id,
                a_reason,
                b_reason,
            } => {
                out.push_str(&format!(
                    "  ⚠ different rejection reason on edge {edge_id}\n"
                ));
                out.push_str(&format!("      trace-a: {a_reason}\n"));
                out.push_str(&format!("      trace-b: {b_reason}\n"));
            }
            DivergenceKind::EdgeOnlyInA { edge_id, to } => out.push_str(&format!(
                "  ⚠ edge {edge_id} (→ {to}) evaluated in trace-a only\n"
            )),
            DivergenceKind::EdgeOnlyInB { edge_id, to } => out.push_str(&format!(
                "  ⚠ edge {edge_id} (→ {to}) evaluated in trace-b only\n"
            )),
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "{} divergence(s) across {} step(s).\n",
        divergences.len(),
        total_steps
    ));
    out
}

fn render_json_output(divergences: &[Divergence], total_steps: usize) -> String {
    #[derive(Serialize)]
    struct Output<'a> {
        divergences: &'a [Divergence],
        summary: Summary,
    }
    #[derive(Serialize)]
    struct Summary {
        total_steps: usize,
        divergences: usize,
    }
    let v = Output {
        divergences,
        summary: Summary {
            total_steps,
            divergences: divergences.len(),
        },
    };
    serde_json::to_string_pretty(&v).expect("json serialize")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use engine::trace::{EdgeEvaluation, EdgeOutcome};
    use engine::{ExecutionLog, StepLog};

    fn make_step(
        name: &str,
        state_before: &str,
        state_after: &str,
        evals: Vec<EdgeEvaluation>,
    ) -> StepLog {
        StepLog {
            step_name: name.into(),
            state_before: state_before.into(),
            state_after: state_after.into(),
            method: "GET".into(),
            url: "http://example.com".into(),
            status: 200,
            duration_ms: 10,
            assertions: vec![],
            matched_edge_tag: None,
            branch_path: None,
            request_body: None,
            response_body: None,
            edge_evaluations: evals,
            failure: None,
        }
    }

    fn make_log(steps: Vec<StepLog>) -> ExecutionLog {
        ExecutionLog {
            steps,
            ..ExecutionLog::default()
        }
    }

    fn matched_eval(edge_id: &str, to: &str) -> EdgeEvaluation {
        EdgeEvaluation {
            edge_id: edge_id.into(),
            to: to.into(),
            tag: None,
            outcome: EdgeOutcome::Matched,
        }
    }

    fn tagged_matched_eval(edge_id: &str, to: &str, tag: &str) -> EdgeEvaluation {
        EdgeEvaluation {
            edge_id: edge_id.into(),
            to: to.into(),
            tag: Some(tag.into()),
            outcome: EdgeOutcome::Matched,
        }
    }

    fn rejected_status(edge_id: &str, to: &str, expected: &str, actual: u16) -> EdgeEvaluation {
        EdgeEvaluation {
            edge_id: edge_id.into(),
            to: to.into(),
            tag: None,
            outcome: EdgeOutcome::RejectedStatusMismatch {
                expected: expected.into(),
                actual,
            },
        }
    }

    fn run_diff(logs_a: Vec<ExecutionLog>, logs_b: Vec<ExecutionLog>) -> Vec<Divergence> {
        let pairs = align_logs(&logs_a, &logs_b);
        pairs
            .iter()
            .flat_map(|p| {
                align_steps(&p.a.steps, &p.b.steps)
                    .into_iter()
                    .flat_map(|sp| diff_step(p.user_idx, &sp))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn diff_identical_logs_has_no_divergences() {
        let a = make_log(vec![make_step(
            "pay",
            "cart",
            "paid",
            vec![matched_eval("aabb1122", "paid")],
        )]);
        let b = make_log(vec![make_step(
            "pay",
            "cart",
            "paid",
            vec![matched_eval("aabb1122", "paid")],
        )]);
        assert!(run_diff(vec![a], vec![b]).is_empty());
    }

    #[test]
    fn diff_detects_routing_divergence() {
        let a = make_log(vec![make_step(
            "checkout",
            "cart",
            "paid",
            vec![matched_eval("aabb0001", "paid")],
        )]);
        let b = make_log(vec![make_step(
            "checkout",
            "cart",
            "retry",
            vec![matched_eval("aabb0002", "retry")],
        )]);
        let divs = run_diff(vec![a], vec![b]);
        assert_eq!(divs.len(), 1);
        assert!(matches!(
            divs[0].kind,
            DivergenceKind::RoutingDiverged { .. }
        ));
    }

    #[test]
    fn diff_detects_rejection_reason_change() {
        let shared_id = "deadbeef";
        let a = make_log(vec![make_step(
            "poll",
            "wait",
            "done",
            vec![
                matched_eval("11111111", "done"),
                rejected_status(shared_id, "retry", "200", 503),
            ],
        )]);
        let b = make_log(vec![make_step(
            "poll",
            "wait",
            "done",
            vec![
                matched_eval("11111111", "done"),
                rejected_status(shared_id, "retry", "200", 404),
            ],
        )]);
        let divs = run_diff(vec![a], vec![b]);
        assert_eq!(divs.len(), 1);
        assert!(matches!(
            divs[0].kind,
            DivergenceKind::RejectionReasonChanged { .. }
        ));
    }

    #[test]
    fn diff_handles_mismatched_user_counts() {
        let make = || make_log(vec![make_step("s", "a", "b", vec![matched_eval("x", "b")])]);
        let a = vec![make(), make(), make()];
        let b = vec![make(), make()];
        assert_eq!(align_logs(&a, &b).len(), 2);
    }

    #[test]
    fn diff_handles_step_count_drift() {
        let a = make_log(vec![make_step(
            "step1",
            "a",
            "b",
            vec![matched_eval("e1", "b")],
        )]);
        let b = make_log(vec![
            make_step("step1", "a", "b", vec![matched_eval("e1", "b")]),
            make_step("step2", "b", "c", vec![matched_eval("e2", "c")]),
        ]);
        let divs = run_diff(vec![a], vec![b]);
        assert_eq!(divs.len(), 1);
        assert!(matches!(
            divs[0].kind,
            DivergenceKind::StepMissingInA { .. }
        ));
    }

    #[test]
    fn diff_fallback_matching_on_missing_edge_id() {
        let mk = || {
            make_log(vec![make_step(
                "s",
                "a",
                "b",
                vec![EdgeEvaluation {
                    edge_id: String::new(),
                    to: "b".into(),
                    tag: None,
                    outcome: EdgeOutcome::Matched,
                }],
            )])
        };
        assert!(run_diff(vec![mk()], vec![mk()]).is_empty());
    }

    #[test]
    fn diff_fallback_matching_distinguishes_tags() {
        let a = make_log(vec![make_step(
            "s",
            "a",
            "b",
            vec![tagged_matched_eval("", "b", "ok")],
        )]);
        let b = make_log(vec![make_step(
            "s",
            "a",
            "b",
            vec![tagged_matched_eval("", "b", "retry")],
        )]);
        let divs = run_diff(vec![a], vec![b]);
        assert_eq!(divs.len(), 1);
        assert!(matches!(
            divs[0].kind,
            DivergenceKind::RoutingDiverged { .. }
        ));
    }

    #[test]
    fn diff_json_output_is_valid() {
        let a = make_log(vec![make_step(
            "pay",
            "cart",
            "paid",
            vec![matched_eval("a1b2c3d4", "paid")],
        )]);
        let b = make_log(vec![make_step(
            "pay",
            "cart",
            "retry",
            vec![matched_eval("e5f6a7b8", "retry")],
        )]);
        let divs = run_diff(vec![a], vec![b]);
        let json = render_json_output(&divs, 1);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert!(parsed["divergences"].is_array());
        assert!(parsed["summary"]["total_steps"].is_number());
    }

    #[test]
    fn diff_smoke_cli() {
        use std::fs;
        use tempfile::NamedTempFile;

        let step = make_step(
            "login",
            "start",
            "logged_in",
            vec![matched_eval("cafe0001", "logged_in")],
        );
        let log = make_log(vec![step]);
        let json = serde_json::to_string(&vec![log]).unwrap();

        let fa = NamedTempFile::new().unwrap();
        let fb = NamedTempFile::new().unwrap();
        fs::write(fa.path(), &json).unwrap();
        fs::write(fb.path(), &json).unwrap();

        let result = cmd_diff(
            fa.path().to_str().unwrap(),
            fb.path().to_str().unwrap(),
            "text",
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn diff_cli_returns_diff_found_without_bad_argument() {
        use std::fs;
        use tempfile::NamedTempFile;

        let a = make_log(vec![make_step(
            "login",
            "start",
            "logged_in",
            vec![matched_eval("cafe0001", "logged_in")],
        )]);
        let b = make_log(vec![make_step(
            "login",
            "start",
            "retry",
            vec![matched_eval("cafe0002", "retry")],
        )]);

        let fa = NamedTempFile::new().unwrap();
        let fb = NamedTempFile::new().unwrap();
        let out = NamedTempFile::new().unwrap();
        fs::write(fa.path(), serde_json::to_string(&vec![a]).unwrap()).unwrap();
        fs::write(fb.path(), serde_json::to_string(&vec![b]).unwrap()).unwrap();

        let result = cmd_diff(
            fa.path().to_str().unwrap(),
            fb.path().to_str().unwrap(),
            "text",
            Some(out.path().to_str().unwrap().to_string()),
        );
        assert!(matches!(result, Err(CliError::DiffFound)));
        let rendered = fs::read_to_string(out.path()).unwrap();
        assert!(rendered.contains("routing diverged"));
    }

    #[test]
    fn diff_rejects_unknown_format() {
        let result = DiffFormat::parse("xml");
        assert!(matches!(result, Err(CliError::BadArgument(_))));
    }
}
