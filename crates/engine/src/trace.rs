use model::{Edge, ValueCheck};
use serde::{Deserialize, Serialize};

/// One edge's outcome during transition evaluation.
///
/// Produced for every edge considered in the winning pass (or for all edges
/// when no pass matched). `outcome` says whether the edge fired, lost to a
/// peer, or was rejected — and why. Persisted on `StepLog` so `ace show`
/// can render causality offline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EdgeEvaluation {
    /// Stable 8-hex identity for this edge definition. Computed from
    /// (from, to, when-tag, tag, priority) so two traces can be aligned
    /// even when multiple edges share the same `to` state.
    /// Old logs without this field deserialize to empty string; diff falls
    /// back to (from, to, tag) matching.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub edge_id: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub outcome: EdgeOutcome,
}

/// Compute a stable 8-hex identity for an edge.
/// Uses FNV-1a over a canonical debug payload. It is not cryptographic; it is
/// only intended to keep trace alignment stable across runs and binaries.
pub fn edge_id(edge: &Edge) -> String {
    let payload = format!(
        "{}|{}|{:?}|{:?}|{:?}",
        edge.from, edge.to, edge.when, edge.tag, edge.priority
    );
    let mut hash = 0xcbf29ce484222325u64;
    for byte in payload.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")[..8].to_string()
}

/// Flat outcome enum. Previously nested `Rejected(EdgeRejectReason)` which
/// caused a duplicate `kind` field when serde serialized the inner tagged
/// enum. Flattened variants have infinite headroom; older `ace show` binaries
/// deserialize unknown variants as `Unknown` via `#[serde(other)]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeOutcome {
    /// This edge won and drove the transition.
    Matched,
    /// Edge condition did not match: HTTP status code mismatch.
    RejectedStatusMismatch { expected: String, actual: u16 },
    /// Edge condition did not match: body JSONPath check failed.
    RejectedBodyCheckFailed {
        path: String,
        expected: String,
        actual: String,
    },
    /// `condition.assertions: passed` required but some assertions failed.
    RejectedAssertionGateFailed { failed_indices: Vec<usize> },
    /// `condition.assertions: failed` required but all assertions passed.
    RejectedAssertionGateUnexpectedlyPassed,
    /// Condition matched but a higher-priority peer won.
    LostPriority { winner_priority: i32 },
    /// Condition matched at the top priority tier but lost a weighted roll.
    LostWeightedRoll { weight: u32, total: u64 },
    /// Tied with peers at the same priority (no weights); first-in-list won.
    /// Add `weight:` or reorder edges to make routing explicit.
    LostTieBreak { winner_index: usize },
    /// Edge was selected but its `max_takes` limit was already exhausted.
    MaxTakesExceeded { limit: u32 },
    /// Unknown variant from a newer ACE version — ignored gracefully.
    #[serde(other)]
    Unknown,
}

/// Render a ValueCheck into a terse human string ("= 200", "contains 'x'",
/// "> 42", "in [200,201]"). Used to describe what an edge required when it
/// was rejected.
pub fn describe_value_check(vc: &ValueCheck) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = &vc.eq {
        parts.push(format!("= {}", render_json(v)));
    }
    if let Some(v) = &vc.ne {
        parts.push(format!("!= {}", render_json(v)));
    }
    if let Some(s) = &vc.contains {
        parts.push(format!("contains '{}'", s));
    }
    if let Some(b) = vc.exists {
        parts.push(if b { "exists".into() } else { "missing".into() });
    }
    if let Some(n) = vc.lt {
        parts.push(format!("< {}", n));
    }
    if let Some(n) = vc.gt {
        parts.push(format!("> {}", n));
    }
    if let Some(list) = &vc.in_list {
        let inner: Vec<String> = list.iter().map(render_json).collect();
        parts.push(format!("in [{}]", inner.join(",")));
    }
    if let Some(t) = &vc.type_of {
        parts.push(format!("type={}", t));
    }
    if parts.is_empty() {
        "<empty check>".into()
    } else {
        parts.join(" AND ")
    }
}

fn render_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("'{}'", s),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::Edge;

    fn make_edge(from: &str, to: &str, priority: Option<i32>, tag: Option<&str>) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            priority,
            tag: tag.map(|s| s.into()),
            ..Edge::default()
        }
    }

    #[test]
    fn rejected_status_mismatch_round_trips() {
        let eval = EdgeEvaluation {
            edge_id: "abc12345".into(),
            to: "done".into(),
            tag: None,
            outcome: EdgeOutcome::RejectedStatusMismatch {
                expected: "200".into(),
                actual: 500,
            },
        };
        let json = serde_json::to_string(&eval).expect("serialize");
        let back: EdgeEvaluation = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back.to, "done");
        match back.outcome {
            EdgeOutcome::RejectedStatusMismatch { actual, .. } => assert_eq!(actual, 500),
            other => panic!("expected RejectedStatusMismatch, got {:?}", other),
        }
    }

    #[test]
    fn unknown_variant_deserializes_gracefully() {
        let json = r#"{"to":"x","outcome":{"kind":"future_variant_not_yet_known"}}"#;
        let back: EdgeEvaluation = serde_json::from_str(json).expect("round trip");
        assert_eq!(back.outcome, EdgeOutcome::Unknown);
    }

    #[test]
    fn lost_tie_break_round_trips() {
        let eval = EdgeEvaluation {
            edge_id: String::new(),
            to: "b".into(),
            tag: None,
            outcome: EdgeOutcome::LostTieBreak { winner_index: 0 },
        };
        let json = serde_json::to_string(&eval).expect("serialize");
        let back: EdgeEvaluation = serde_json::from_str(&json).expect("round trip");
        assert!(matches!(
            back.outcome,
            EdgeOutcome::LostTieBreak { winner_index: 0 }
        ));
    }

    #[test]
    fn edge_id_stable_across_runs() {
        let e = make_edge("start", "done", None, None);
        assert_eq!(edge_id(&e), edge_id(&e));
    }

    #[test]
    fn edge_id_changes_on_priority_bump() {
        let e1 = make_edge("start", "done", Some(1), None);
        let e2 = make_edge("start", "done", Some(10), None);
        assert_ne!(edge_id(&e1), edge_id(&e2));
    }

    #[test]
    fn edge_id_changes_on_condition_change() {
        let mut e1 = make_edge("start", "done", None, None);
        e1.when = Some(model::TransitionCondition {
            status: Some(model::StatusMatch::Exact(200)),
            body: None,
            assertions: None,
        });
        let mut e2 = make_edge("start", "done", None, None);
        e2.when = Some(model::TransitionCondition {
            status: Some(model::StatusMatch::Exact(503)),
            body: None,
            assertions: None,
        });
        assert_ne!(edge_id(&e1), edge_id(&e2));
    }

    #[test]
    fn edge_id_survives_round_trip() {
        let e = make_edge("a", "b", Some(5), Some("mytag"));
        let id = edge_id(&e);
        let eval = EdgeEvaluation {
            edge_id: id.clone(),
            to: "b".into(),
            tag: Some("mytag".into()),
            outcome: EdgeOutcome::Matched,
        };
        let json = serde_json::to_string(&eval).expect("serialize");
        let back: EdgeEvaluation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.edge_id, id);
    }

    #[test]
    fn old_log_without_edge_id_deserializes_to_empty() {
        let json = r#"{"to":"done","outcome":{"kind":"matched"}}"#;
        let back: EdgeEvaluation = serde_json::from_str(json).expect("round trip");
        assert_eq!(back.edge_id, "");
    }
}
