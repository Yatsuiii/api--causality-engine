use model::ValueCheck;
use serde::{Deserialize, Serialize};

/// One edge's outcome during transition evaluation.
///
/// Produced for every edge considered in the winning pass (or for all edges
/// when no pass matched). `outcome` says whether the edge fired, lost to a
/// peer, or was rejected — and why. Persisted on `StepLog` so `ace show` /
/// Tauri re-render offline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EdgeEvaluation {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub outcome: EdgeOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeOutcome {
    /// This edge won and drove the transition.
    Matched,
    /// Edge condition did not match the response.
    Rejected(EdgeRejectReason),
    /// Condition matched but a higher-priority peer won.
    LostPriority { winner_priority: i32 },
    /// Condition matched at the top priority tier but lost a weighted roll.
    LostWeightedRoll { weight: u32, total: u64 },
    /// Edge was selected but its `max_takes` limit was already reached.
    /// Surfaces alongside `RunError::EdgeMaxTakesExceeded`.
    MaxTakesExceeded { limit: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeRejectReason {
    StatusMismatch {
        expected: String,
        actual: u16,
    },
    BodyCheckFailed {
        path: String,
        expected: String,
        actual: String,
    },
    /// `condition.assertions: passed` was required but some assertion failed.
    AssertionGateFailed {
        failed_indices: Vec<usize>,
    },
    /// `condition.assertions: failed` was required but every assertion passed.
    AssertionGateUnexpectedlyPassed,
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
