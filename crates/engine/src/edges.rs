use crate::assertions::AssertionResult;
use crate::jsonpath;
use crate::trace::{self, EdgeEvaluation, EdgeOutcome};
use ace_http::HttpResponse;
use model::{AssertionMatch, Edge, StatusMatch, TransitionCondition};
use rand::{Rng, rngs::StdRng};
use std::collections::HashMap;

/// Result of transition evaluation.
///
/// `chosen` — index of the winning edge, or `None` when no pass matched.
/// `evaluations` — every edge that was considered, with why it won/lost.
/// Callers attach this to the `StepLog` so the CLI and `ace show` can render
/// the causality ("edge→X lost: status=500, needed 200").
pub(crate) struct EdgeDecision {
    pub chosen: Option<usize>,
    pub evaluations: Vec<EdgeEvaluation>,
}

/// Stable key for tracking per-edge take counts: `(from, to, tag)`.
/// Avoids the pointer-identity hazard of `edge as *const Edge as usize`.
pub(crate) type EdgeKey = (String, String, Option<String>);

pub(crate) fn evaluate_edges(
    edges: &[&Edge],
    response: &HttpResponse,
    assertion_results: &[AssertionResult],
    take_counts: &mut HashMap<EdgeKey, u32>,
    rng: &mut StdRng,
) -> EdgeDecision {
    let mut evaluations: Vec<EdgeEvaluation> = Vec::new();

    let make_eval = |idx: usize, outcome: EdgeOutcome| -> EdgeEvaluation {
        EdgeEvaluation {
            to: edges[idx].to.clone(),
            tag: edges[idx].tag.clone(),
            outcome,
        }
    };

    // Pass 1: conditional edges.
    //
    // For every `when`-gated edge, run `matches_condition`. Losers get
    // `Rejected(reason)` recorded. Matchers become candidates for `choose`.
    let mut cond_candidates: Vec<usize> = Vec::new();
    for (i, edge) in edges.iter().enumerate() {
        if let Some(cond) = edge.when.as_ref() {
            match matches_condition(cond, response, assertion_results) {
                Ok(()) => cond_candidates.push(i),
                Err(outcome) => {
                    evaluations.push(make_eval(i, outcome));
                }
            }
        }
    }

    if let Some(chosen) = choose_from(&cond_candidates, edges, rng, &mut evaluations) {
        return apply_max_takes(chosen, edges, take_counts, evaluations);
    }

    // Pass 2: explicit defaults. Unconditional, so no rejection reasons to
    // record — they're only considered because pass 1 produced no winner.
    let explicit: Vec<usize> = (0..edges.len())
        .filter(|&i| edges[i].when.is_none() && edges[i].default.unwrap_or(false))
        .collect();
    if let Some(chosen) = choose_from(&explicit, edges, rng, &mut evaluations) {
        return apply_max_takes(chosen, edges, take_counts, evaluations);
    }

    // Pass 3: implicit unconditional.
    let implicit: Vec<usize> = (0..edges.len())
        .filter(|&i| edges[i].when.is_none() && !edges[i].default.unwrap_or(false))
        .collect();
    if let Some(chosen) = choose_from(&implicit, edges, rng, &mut evaluations) {
        return apply_max_takes(chosen, edges, take_counts, evaluations);
    }

    EdgeDecision {
        chosen: None,
        evaluations,
    }
}

/// Pick a winner from a candidate list, filtering by max priority then by
/// weighted roll. Losers are appended to `evaluations` with `LostPriority`
/// or `LostWeightedRoll`. Returns the winner's edge index, or `None` if the
/// candidate list was empty.
fn choose_from(
    candidates: &[usize],
    edges: &[&Edge],
    rng: &mut StdRng,
    evaluations: &mut Vec<EdgeEvaluation>,
) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }

    let max_pri = candidates
        .iter()
        .map(|&i| edges[i].priority.unwrap_or(0))
        .max()
        .expect("non-empty");

    let mut top: Vec<usize> = Vec::new();
    for &i in candidates {
        if edges[i].priority.unwrap_or(0) == max_pri {
            top.push(i);
        } else {
            evaluations.push(EdgeEvaluation {
                to: edges[i].to.clone(),
                tag: edges[i].tag.clone(),
                outcome: EdgeOutcome::LostPriority {
                    winner_priority: max_pri,
                },
            });
        }
    }

    if top.len() == 1 {
        let winner = top[0];
        evaluations.push(EdgeEvaluation {
            to: edges[winner].to.clone(),
            tag: edges[winner].tag.clone(),
            outcome: EdgeOutcome::Matched,
        });
        return Some(winner);
    }

    let all_weighted = top.iter().all(|&i| edges[i].weight.is_some());
    if !all_weighted {
        // Unweighted tie: first-in-list wins. Emit LostTieBreak for peers so
        // the user knows why — add `weight:` or reorder to make it explicit.
        let winner = top[0];
        evaluations.push(EdgeEvaluation {
            to: edges[winner].to.clone(),
            tag: edges[winner].tag.clone(),
            outcome: EdgeOutcome::Matched,
        });
        for &i in &top[1..] {
            evaluations.push(EdgeEvaluation {
                to: edges[i].to.clone(),
                tag: edges[i].tag.clone(),
                outcome: EdgeOutcome::LostTieBreak { winner_index: winner },
            });
        }
        return Some(winner);
    }

    let total: u64 = top.iter().map(|&i| edges[i].weight.unwrap() as u64).sum();
    if total == 0 {
        let winner = top[0];
        evaluations.push(EdgeEvaluation {
            to: edges[winner].to.clone(),
            tag: edges[winner].tag.clone(),
            outcome: EdgeOutcome::Matched,
        });
        return Some(winner);
    }

    let mut roll = rng.gen_range(0..total);
    let mut winner: Option<usize> = None;
    for &i in &top {
        let w = edges[i].weight.unwrap() as u64;
        if winner.is_none() && w > roll {
            winner = Some(i);
        } else if winner.is_none() {
            roll -= w;
        }
    }
    let winner = winner.unwrap_or(top[top.len() - 1]);

    for &i in &top {
        if i == winner {
            evaluations.push(EdgeEvaluation {
                to: edges[i].to.clone(),
                tag: edges[i].tag.clone(),
                outcome: EdgeOutcome::Matched,
            });
        } else {
            evaluations.push(EdgeEvaluation {
                to: edges[i].to.clone(),
                tag: edges[i].tag.clone(),
                outcome: EdgeOutcome::LostWeightedRoll {
                    weight: edges[i].weight.unwrap(),
                    total,
                },
            });
        }
    }

    Some(winner)
}

/// Check `max_takes` on the chosen edge. If the cap is hit, flip its outcome
/// to `MaxTakesExceeded` (so the CLI can render "✗ edge fired but capped")
/// and set `chosen = None` — caller returns `RunError::EdgeMaxTakesExceeded`
/// but now has the trace to explain it.
fn apply_max_takes(
    chosen: usize,
    edges: &[&Edge],
    take_counts: &mut HashMap<EdgeKey, u32>,
    mut evaluations: Vec<EdgeEvaluation>,
) -> EdgeDecision {
    let edge = edges[chosen];
    if let Some(limit) = edge.max_takes {
        let key = (edge.from.clone(), edge.to.clone(), edge.tag.clone());
        let count = take_counts.entry(key).or_insert(0);
        if *count >= limit {
            for ev in evaluations.iter_mut() {
                if ev.to == edge.to
                    && ev.tag == edge.tag
                    && matches!(ev.outcome, EdgeOutcome::Matched)
                {
                    ev.outcome = EdgeOutcome::MaxTakesExceeded { limit };
                    break;
                }
            }
            return EdgeDecision {
                chosen: None,
                evaluations,
            };
        }
        *count += 1;
    }
    EdgeDecision {
        chosen: Some(chosen),
        evaluations,
    }
}

fn matches_condition(
    condition: &TransitionCondition,
    response: &HttpResponse,
    assertion_results: &[AssertionResult],
) -> Result<(), EdgeOutcome> {
    if let Some(status_match) = &condition.status {
        let matches = match status_match {
            StatusMatch::Exact(code) => response.status == *code,
            StatusMatch::Complex(vc) => {
                let val = serde_json::Value::Number(serde_json::Number::from(response.status));
                crate::assertions::eval_value_check(vc, Some(&val), &response.status.to_string())
            }
        };
        if !matches {
            let expected = match status_match {
                StatusMatch::Exact(c) => c.to_string(),
                StatusMatch::Complex(vc) => trace::describe_value_check(vc),
            };
            return Err(EdgeOutcome::RejectedStatusMismatch {
                expected,
                actual: response.status,
            });
        }
    }

    if let Some(body_checks) = &condition.body {
        let json: Option<serde_json::Value> = serde_json::from_str(&response.body).ok();
        for (path, check) in body_checks {
            let resolved = json.as_ref().and_then(|j| jsonpath::resolve(j, path));
            let actual_str = resolved
                .as_ref()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            if !crate::assertions::eval_value_check(check, resolved.as_ref(), &actual_str) {
                return Err(EdgeOutcome::RejectedBodyCheckFailed {
                    path: path.clone(),
                    expected: trace::describe_value_check(check),
                    actual: actual_str,
                });
            }
        }
    }

    if let Some(assertion_match) = &condition.assertions {
        let failed_indices: Vec<usize> = assertion_results
            .iter()
            .enumerate()
            .filter(|(_, a)| !a.passed)
            .map(|(i, _)| i)
            .collect();
        let all_passed = failed_indices.is_empty();
        match assertion_match {
            AssertionMatch::Passed => {
                if !all_passed {
                    return Err(EdgeOutcome::RejectedAssertionGateFailed { failed_indices });
                }
            }
            AssertionMatch::Failed => {
                if all_passed {
                    return Err(EdgeOutcome::RejectedAssertionGateUnexpectedlyPassed);
                }
            }
        }
    }

    Ok(())
}

/// Pick a fallback transition target when a step is skipped — prefer the
/// explicit default edge; otherwise take the first outgoing edge.
pub(crate) fn default_edge_target(edges: &[&Edge]) -> Option<String> {
    edges
        .iter()
        .find(|edge| edge.default.unwrap_or(false))
        .map(|edge| edge.to.clone())
        .or_else(|| edges.first().map(|edge| edge.to.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn resp(status: u16) -> HttpResponse {
        HttpResponse {
            status,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 1,
        }
    }

    fn pick_to(edges: &[&Edge], resp: &HttpResponse) -> String {
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);
        let decision = evaluate_edges(edges, resp, &[], &mut counts, &mut rng);
        let idx = decision.chosen.expect("expected a matching edge");
        edges[idx].to.clone()
    }

    #[test]
    fn evaluate_edges_uses_default_fallback() {
        let edges = [Edge {
            from: "start".into(),
            to: "done".into(),
            default: Some(true),
            ..Edge::default()
        }];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(200)), "done");
    }

    #[test]
    fn evaluate_edges_matches_status_rule() {
        use model::ValueCheck;
        let edges = [
            Edge {
                from: "start".into(),
                to: "retry".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Complex(ValueCheck {
                        gt: Some(399.0),
                        ..ValueCheck::default()
                    })),
                    body: None,
                    assertions: None,
                }),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "done".into(),
                default: Some(true),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(500)), "retry");
    }

    #[test]
    fn evaluate_edges_treats_unconditional_as_implicit_default() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "fallback".into(),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "retry".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(500)),
                    body: None,
                    assertions: None,
                }),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(500)), "retry");
        assert_eq!(pick_to(&refs, &resp(200)), "fallback");
    }

    #[test]
    fn evaluate_edges_explicit_default_beats_implicit() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "implicit".into(),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "explicit".into(),
                default: Some(true),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(200)), "explicit");
    }

    #[test]
    fn evaluate_edges_respects_priority() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "low_pri".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(500)),
                    body: None,
                    assertions: None,
                }),
                priority: Some(1),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "high_pri".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(500)),
                    body: None,
                    assertions: None,
                }),
                priority: Some(10),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(500)), "high_pri");
    }

    #[test]
    fn evaluate_edges_enforces_max_takes() {
        let edges = [Edge {
            from: "start".into(),
            to: "retry".into(),
            default: Some(true),
            max_takes: Some(2),
            ..Edge::default()
        }];
        let refs: Vec<&Edge> = edges.iter().collect();
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);
        assert!(
            evaluate_edges(&refs, &resp(200), &[], &mut counts, &mut rng)
                .chosen
                .is_some()
        );
        assert!(
            evaluate_edges(&refs, &resp(200), &[], &mut counts, &mut rng)
                .chosen
                .is_some()
        );
        let third = evaluate_edges(&refs, &resp(200), &[], &mut counts, &mut rng);
        assert!(third.chosen.is_none());
        assert!(
            third
                .evaluations
                .iter()
                .any(|e| matches!(e.outcome, EdgeOutcome::MaxTakesExceeded { limit: 2 }))
        );
    }

    #[test]
    fn evaluate_edges_weighted_is_deterministic_per_seed() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "a".into(),
                weight: Some(70),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "b".into(),
                weight: Some(30),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();

        let mut rng1 = StdRng::seed_from_u64(42);
        let mut rng2 = StdRng::seed_from_u64(42);
        let mut counts1 = HashMap::new();
        let mut counts2 = HashMap::new();
        let first = evaluate_edges(&refs, &resp(200), &[], &mut counts1, &mut rng1);
        let second = evaluate_edges(&refs, &resp(200), &[], &mut counts2, &mut rng2);
        assert_eq!(first.chosen, second.chosen);

        let mut rng = StdRng::seed_from_u64(123);
        let mut hits_a = 0;
        for _ in 0..1000 {
            let mut counts = HashMap::new();
            let idx = evaluate_edges(&refs, &resp(200), &[], &mut counts, &mut rng)
                .chosen
                .expect("weighted pick always selects");
            if edges[idx].to == "a" {
                hits_a += 1;
            }
        }
        assert!(
            (600..=800).contains(&hits_a),
            "expected ~700 hits on 'a', got {hits_a}"
        );
    }

    #[test]
    fn evaluate_edges_weighted_zero_total_falls_back_to_first() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "a".into(),
                weight: Some(0),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "b".into(),
                weight: Some(0),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(200)), "a");
    }

    // ---- Causality trace (edge_evaluations) ----------------------------

    #[test]
    fn trace_records_rejected_edges_on_no_match() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "ok".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(200)),
                    body: None,
                    assertions: None,
                }),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "auth_failed".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(401)),
                    body: None,
                    assertions: None,
                }),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);
        let decision = evaluate_edges(&refs, &resp(500), &[], &mut counts, &mut rng);
        assert!(decision.chosen.is_none());
        assert_eq!(decision.evaluations.len(), 2);
        for ev in &decision.evaluations {
            match &ev.outcome {
                EdgeOutcome::RejectedStatusMismatch { actual, .. } => {
                    assert_eq!(*actual, 500);
                }
                other => panic!("expected RejectedStatusMismatch, got {:?}", other),
            }
        }
    }

    #[test]
    fn trace_matched_winner_is_recorded() {
        let edges = [Edge {
            from: "start".into(),
            to: "done".into(),
            default: Some(true),
            ..Edge::default()
        }];
        let refs: Vec<&Edge> = edges.iter().collect();
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);
        let decision = evaluate_edges(&refs, &resp(200), &[], &mut counts, &mut rng);
        assert!(decision.chosen.is_some());
        assert_eq!(decision.evaluations.len(), 1);
        assert!(matches!(
            decision.evaluations[0].outcome,
            EdgeOutcome::Matched
        ));
        assert_eq!(decision.evaluations[0].to, "done");
    }

    #[test]
    fn trace_priority_losers_recorded() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "low_pri".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(500)),
                    body: None,
                    assertions: None,
                }),
                priority: Some(1),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "high_pri".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(500)),
                    body: None,
                    assertions: None,
                }),
                priority: Some(10),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);
        let decision = evaluate_edges(&refs, &resp(500), &[], &mut counts, &mut rng);
        assert_eq!(decision.chosen, Some(1));
        let low = decision
            .evaluations
            .iter()
            .find(|e| e.to == "low_pri")
            .expect("low_pri entry");
        assert!(matches!(
            low.outcome,
            EdgeOutcome::LostPriority {
                winner_priority: 10
            }
        ));
    }

    #[test]
    fn trace_weighted_losers_recorded() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "a".into(),
                weight: Some(70),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "b".into(),
                weight: Some(30),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(42);
        let decision = evaluate_edges(&refs, &resp(200), &[], &mut counts, &mut rng);
        assert!(decision.chosen.is_some());
        assert_eq!(decision.evaluations.len(), 2);
        let matched_count = decision
            .evaluations
            .iter()
            .filter(|e| matches!(e.outcome, EdgeOutcome::Matched))
            .count();
        let lost_count = decision
            .evaluations
            .iter()
            .filter(|e| matches!(e.outcome, EdgeOutcome::LostWeightedRoll { total: 100, .. }))
            .count();
        assert_eq!(matched_count, 1);
        assert_eq!(lost_count, 1);
    }

    #[test]
    fn trace_body_check_failure_recorded() {
        use model::ValueCheck;
        let mut body_checks: HashMap<String, ValueCheck> = HashMap::new();
        body_checks.insert(
            "status".into(),
            ValueCheck {
                eq: Some(serde_json::json!("ok")),
                ..ValueCheck::default()
            },
        );
        let edges = [Edge {
            from: "start".into(),
            to: "ok".into(),
            when: Some(TransitionCondition {
                status: Some(StatusMatch::Exact(200)),
                body: Some(body_checks),
                assertions: None,
            }),
            ..Edge::default()
        }];
        let refs: Vec<&Edge> = edges.iter().collect();
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);
        let bad_resp = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: r#"{"status":"error"}"#.into(),
            duration_ms: 1,
        };
        let decision = evaluate_edges(&refs, &bad_resp, &[], &mut counts, &mut rng);
        assert!(decision.chosen.is_none());
        assert_eq!(decision.evaluations.len(), 1);
        match &decision.evaluations[0].outcome {
            EdgeOutcome::RejectedBodyCheckFailed { path, actual, .. } => {
                assert_eq!(path, "status");
                assert_eq!(actual, "error");
            }
            other => panic!("expected RejectedBodyCheckFailed, got {:?}", other),
        }
    }

    #[test]
    fn trace_max_takes_cap_produces_no_chosen_with_recorded_outcome() {
        let edges = [Edge {
            from: "start".into(),
            to: "retry".into(),
            default: Some(true),
            max_takes: Some(1),
            ..Edge::default()
        }];
        let refs: Vec<&Edge> = edges.iter().collect();
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);

        let first = evaluate_edges(&refs, &resp(200), &[], &mut counts, &mut rng);
        assert!(first.chosen.is_some());
        assert!(
            first
                .evaluations
                .iter()
                .any(|e| matches!(e.outcome, EdgeOutcome::Matched))
        );

        let second = evaluate_edges(&refs, &resp(200), &[], &mut counts, &mut rng);
        assert!(second.chosen.is_none());
        assert!(
            second
                .evaluations
                .iter()
                .any(|e| matches!(e.outcome, EdgeOutcome::MaxTakesExceeded { limit: 1 }))
        );
    }
}
