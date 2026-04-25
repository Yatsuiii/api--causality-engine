use crate::error::CliError;
use crate::glyph;
use engine::assertions::AssertionResult;
use engine::mask;
use engine::trace::{EdgeEvaluation, EdgeOutcome};
use engine::{ExecutionLog, StepLog};
use model::MaskRule;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub struct DiffArgs {
    pub a: String,
    pub b: String,
    pub format: String,
    pub output: Option<String>,
    pub mask_extra: Option<String>,
    /// Expand `· masked: …` lines with the pre-mask values from each side.
    /// Requires raw `response_body` to have been retained — automatic when
    /// scenario `mask:` is non-empty (see P0.3 carryover note).
    pub show_masked: bool,
    /// Suppress everything except the trailing `ACE_SUMMARY:` line.
    pub quiet: bool,
}

pub fn cmd_diff(args: DiffArgs) -> Result<(), CliError> {
    let format = DiffFormat::parse(&args.format)?;
    let extra_rules = load_mask_extra(args.mask_extra.as_deref())?;
    let logs_a = load_logs(&args.a)?;
    let logs_b = load_logs(&args.b)?;

    let pairs = align_logs(&logs_a, &logs_b);
    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut total_steps = 0usize;
    // Pre-mask value lookups for --show-masked. Built lazily; same data feeds
    // both text and markdown renderers.
    let mut masked_values: HashMap<(usize, String, usize), Vec<MaskedValuePair>> = HashMap::new();

    for pair in &pairs {
        let step_pairs = align_steps(&pair.a.steps, &pair.b.steps);
        total_steps += step_pairs.len();
        for sp in &step_pairs {
            let mut divs = diff_step(pair.user_idx, sp, &extra_rules);
            if args.show_masked {
                let pairs_for_step = collect_masked_value_pairs(sp);
                if !pairs_for_step.is_empty() {
                    masked_values.insert(
                        (pair.user_idx, sp.step_name.clone(), sp.occurrence),
                        pairs_for_step,
                    );
                }
            }
            all_divergences.append(&mut divs);
        }
    }

    let summary = DiffSummaryLine::new(&args.a, &args.b, total_steps, &all_divergences);

    let render_ctx = RenderContext {
        show_masked: args.show_masked,
        masked_values: &masked_values,
    };

    let text = match format {
        DiffFormat::Json => render_json_output(&all_divergences, total_steps),
        DiffFormat::Text => render_text(&all_divergences, total_steps, &summary, &render_ctx),
        DiffFormat::Markdown => render_markdown(&all_divergences, &summary, &render_ctx),
    };

    if !args.quiet {
        match args.output {
            Some(ref path) => std::fs::write(path, &text).map_err(|e| CliError::Io {
                path: path.clone(),
                source: e,
            })?,
            None => print!("{}", text),
        }
    } else if let Some(ref path) = args.output {
        // --quiet still honors --output: the file gets the full rendered
        // diff so a CI job can inspect it on demand without parsing stdout.
        std::fs::write(path, &text).map_err(|e| CliError::Io {
            path: path.clone(),
            source: e,
        })?;
    }

    // Always emit the machine-readable ACE_SUMMARY line on stdout (even with
    // --quiet, even when --output redirects the rendered diff to a file).
    // Sinks grep for this line without parsing the trace; never put ANSI
    // codes on it.
    println!("{}", summary.as_summary_line());

    if all_divergences.is_empty() {
        Ok(())
    } else {
        Err(CliError::DiffFound)
    }
}

/// Pre-mask values for a single masked path, captured from both traces.
/// Used by `--show-masked` to render `staging: "sub_abc"  prod: "sub_xyz"`.
#[derive(Debug)]
pub(crate) struct MaskedValuePair {
    pub path: String,
    pub a: Option<String>,
    pub b: Option<String>,
}

/// Inspect both raw bodies, find values at each declared mask path, and
/// emit a stable-ordered list. Bodies that didn't survive non-verbose
/// capture are silently skipped — no value to render is not an error.
fn collect_masked_value_pairs(sp: &StepPair) -> Vec<MaskedValuePair> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    if let Some(s) = sp.a {
        for p in &s.masked_fields {
            paths.insert(p.clone());
        }
    }
    if let Some(s) = sp.b {
        for p in &s.masked_fields {
            paths.insert(p.clone());
        }
    }
    paths
        .into_iter()
        .map(|path| MaskedValuePair {
            a: sp
                .a
                .and_then(|s| value_at_path(s.response_body.as_deref(), &path)),
            b: sp
                .b
                .and_then(|s| value_at_path(s.response_body.as_deref(), &path)),
            path,
        })
        .collect()
}

/// Best-effort lookup of `path` against a raw JSON body. Returns the
/// stringified value (canonical for arrays/objects). Non-JSON or missing
/// path returns None — caller renders `<absent>`.
fn value_at_path(raw: Option<&str>, path: &str) -> Option<String> {
    let raw = raw?;
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let pointer = jsonpath_to_pointer(path)?;
    let target = v.pointer(&pointer)?;
    Some(match target {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    })
}

/// Translate the supported JSONPath subset (`$.foo.bar`) into a JSON
/// Pointer (`/foo/bar`). Only handles the literal-key dot form — same
/// subset documented in `engine::mask`. Returns None for anything that
/// can't be a pointer.
fn jsonpath_to_pointer(path: &str) -> Option<String> {
    let body = path.strip_prefix("$.")?;
    let mut out = String::new();
    for seg in body.split('.') {
        if seg.is_empty() || seg.contains(['[', ']', '*', '?']) {
            return None;
        }
        out.push('/');
        out.push_str(&seg.replace('~', "~0").replace('/', "~1"));
    }
    Some(out)
}

/// Renderer-side context. Bundles flags + lookup tables so the text and
/// markdown renderers stay symmetric and don't grow per-format args.
pub(crate) struct RenderContext<'a> {
    pub show_masked: bool,
    pub masked_values: &'a HashMap<(usize, String, usize), Vec<MaskedValuePair>>,
}

#[derive(Clone, Copy)]
enum DiffFormat {
    Text,
    Json,
    Markdown,
}

impl DiffFormat {
    fn parse(raw: &str) -> Result<Self, CliError> {
        match raw {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "markdown" | "md" => Ok(Self::Markdown),
            other => Err(CliError::BadArgument(format!(
                "invalid diff format '{other}' (expected 'text', 'markdown', or 'json')"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

fn load_mask_extra(path: Option<&str>) -> Result<Vec<MaskRule>, CliError> {
    let path = match path {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let raw = std::fs::read_to_string(path).map_err(|e| CliError::Io {
        path: path.to_string(),
        source: e,
    })?;
    #[derive(serde::Deserialize)]
    struct MaskFile {
        #[serde(default)]
        mask: Vec<MaskRule>,
    }
    let file: MaskFile = serde_yaml::from_str(&raw)
        .map_err(|e| CliError::BadArgument(format!("invalid mask file {path}: {e}")))?;
    Ok(file.mask)
}

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
    /// JSONPath patterns suppressed by mask rules in this step comparison.
    /// Empty when no mask rules are active.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub masked: Vec<String>,
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
    /// Response bodies differ after `mask:` rules and `--mask-extra` are
    /// applied. Both `a` and `b` are canonicalized JSON strings (sorted keys)
    /// when the body parses as JSON, otherwise the raw text. Only emitted when
    /// the trace captured response bodies — i.e. the run was `--verbose` or the
    /// scenario declared at least one body mask rule. Skipped when routing
    /// already diverged (the bodies necessarily came from different requests).
    BodyDiverged {
        a: String,
        b: String,
    },
    /// Response headers differ after mask rules. Only emitted when the trace
    /// captured headers, which only happens when the scenario declared at
    /// least one Header mask rule (the implicit signal that headers matter).
    /// Without that signal headers are too noisy (Date, Content-Length, etc.)
    /// to be compared by default.
    HeadersDiverged {
        diff: Vec<HeaderDelta>,
    },
}

#[derive(Debug, Serialize)]
pub struct HeaderDelta {
    pub header: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RouteInfo {
    pub matched_edge_id: String,
    /// Source state of the matched edge — `from` half of the human label
    /// `from→to`. Pulled from `StepLog.state_before` so we don't expose 8-char
    /// hashes to the reader. Kept alongside `matched_edge_id` rather than
    /// replacing it: machine consumers (and ambiguity fallbacks) still want
    /// the hash.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    pub to: String,
    /// Edge tag, if any. Disambiguates two edges with the same `from→to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub rejected: Vec<RejectedEdge>,
}

#[derive(Debug, Serialize)]
pub struct RejectedEdge {
    pub edge_id: String,
    /// Source state for the human label `from→to`. Same provenance as
    /// `RouteInfo.from`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
    pub to: String,
    pub reason: String,
    /// Optional edge tag. Used to disambiguate when two edges share the same
    /// `from→to` pair (e.g. `cart→paid` with `(retry)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-step diffing
// ---------------------------------------------------------------------------

fn diff_step(user_idx: usize, sp: &StepPair, extra_rules: &[MaskRule]) -> Vec<Divergence> {
    let mut out: Vec<DivergenceKind> = Vec::new();
    let key = (user_idx, sp.step_name.clone(), sp.occurrence);

    match (sp.a, sp.b) {
        (None, _) => {
            return vec![Divergence {
                user: key.0,
                step: key.1.clone(),
                occurrence: key.2,
                kind: DivergenceKind::StepMissingInA { step: key.1 },
                masked: Vec::new(),
            }];
        }
        (_, None) => {
            return vec![Divergence {
                user: key.0,
                step: key.1.clone(),
                occurrence: key.2,
                kind: DivergenceKind::StepMissingInB { step: key.1 },
                masked: Vec::new(),
            }];
        }
        (Some(a), Some(b)) => {
            let a_matched = matched_edge(a);
            let b_matched = matched_edge(b);

            match (a_matched, b_matched) {
                (None, None) => {
                    let a_out = outcome_summary_for_step(a);
                    let b_out = outcome_summary_for_step(b);
                    if a_out != b_out {
                        out.push(DivergenceKind::OutcomeDiverged {
                            a_outcome: a_out,
                            b_outcome: b_out,
                        });
                    }
                }
                (Some(a_ev), Some(b_ev)) => {
                    let a_id = effective_id(a_ev, a);
                    let b_id = effective_id(b_ev, b);
                    if a_id != b_id {
                        out.push(DivergenceKind::RoutingDiverged {
                            a: build_route_info(a),
                            b: build_route_info(b),
                        });
                    } else {
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
                                    out.push(DivergenceKind::RejectionReasonChanged {
                                        edge_id: eid,
                                        a_reason: ar.clone(),
                                        b_reason: br.clone(),
                                    });
                                }
                                (Some(ar), None) => {
                                    out.push(DivergenceKind::EdgeOnlyInA {
                                        edge_id: eid,
                                        to: ar.clone(),
                                    });
                                }
                                (None, Some(br)) => {
                                    out.push(DivergenceKind::EdgeOnlyInB {
                                        edge_id: eid,
                                        to: br.clone(),
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
                (Some(_), None) | (None, Some(_)) => {
                    out.push(DivergenceKind::RoutingDiverged {
                        a: build_route_info(a),
                        b: build_route_info(b),
                    });
                }
            }

            // Body + header comparison runs only when routing did NOT diverge;
            // diverged routing means the bodies came from different requests,
            // so a separate body diff would just be redundant noise.
            let routing_split = out
                .iter()
                .any(|k| matches!(k, DivergenceKind::RoutingDiverged { .. }));
            if !routing_split {
                if let Some(body_div) = diff_bodies(a, b, extra_rules) {
                    out.push(body_div);
                }
                if let Some(header_div) = diff_headers(a, b, extra_rules) {
                    out.push(header_div);
                }
            }
        }
    }

    // Compute masked fields: union of scenario-embedded fields from both
    // traces, plus any extra rules that matched either body at diff time.
    let masked = {
        let a_fields = sp.a.map(|s| s.masked_fields.as_slice()).unwrap_or(&[]);
        let b_fields = sp.b.map(|s| s.masked_fields.as_slice()).unwrap_or(&[]);
        let mut combined: Vec<String> = a_fields.iter().chain(b_fields.iter()).cloned().collect();
        if !extra_rules.is_empty() {
            for step in [sp.a, sp.b].into_iter().flatten() {
                if let Some(body) = &step.response_body
                    && let Some((_, matched)) = mask::normalize_body_tracked(body, extra_rules)
                {
                    combined.extend(matched);
                }
            }
        }
        combined.sort();
        combined.dedup();
        combined
    };

    out.into_iter()
        .map(|kind| Divergence {
            user: key.0,
            step: key.1.clone(),
            occurrence: key.2,
            kind,
            masked: masked.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Body + header comparison
// ---------------------------------------------------------------------------

/// Compare response bodies between two steps after applying mask rules.
///
/// Uses `response_body_normalized` (scenario masks already baked in) when
/// present; falls back to raw `response_body`. Then applies `--mask-extra`
/// rules on top so users can suppress noise without re-running. Returns
/// `None` when either side has no body, or when the canonicalized
/// representations match. Non-JSON bodies fall back to raw-text comparison.
fn diff_bodies(a: &StepLog, b: &StepLog, extra: &[MaskRule]) -> Option<DivergenceKind> {
    let a_repr = body_repr_for_diff(a, extra)?;
    let b_repr = body_repr_for_diff(b, extra)?;
    if a_repr == b_repr {
        None
    } else {
        Some(DivergenceKind::BodyDiverged {
            a: a_repr,
            b: b_repr,
        })
    }
}

fn body_repr_for_diff(step: &StepLog, extra: &[MaskRule]) -> Option<String> {
    // Prefer normalized JSON value if present (scenario `mask:` rules already
    // applied at capture time). Apply extra rules on top so a stale trace can
    // still be re-suppressed at diff time without re-running.
    if let Some(norm) = &step.response_body_normalized {
        let mut value = norm.clone();
        if !extra.is_empty()
            && let Some((after_extra, _)) = mask::normalize_body_tracked(&value.to_string(), extra)
        {
            value = after_extra;
        }
        return Some(canonical_json_string(&value));
    }
    let raw = step.response_body.as_ref()?;
    if !extra.is_empty()
        && let Some((value, _)) = mask::normalize_body_tracked(raw, extra)
    {
        return Some(canonical_json_string(&value));
    }
    // Best-effort canonicalization for raw JSON text; non-JSON returns as-is.
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => Some(canonical_json_string(&v)),
        Err(_) => Some(raw.clone()),
    }
}

/// `serde_json::to_string` is order-stable here because serde_json defaults
/// its Map to `BTreeMap` (no `preserve_order` feature in our deps), so two
/// semantically identical values produce byte-identical output. Callers rely
/// on this for body equality comparison.
fn canonical_json_string(v: &serde_json::Value) -> String {
    serde_json::to_string(v).expect("serde_json::Value always serializes")
}

/// Compare response headers between two steps. Only fires when the trace
/// captured headers in the first place — which only happens when the scenario
/// declared a Header mask rule (the implicit opt-in signal). Header masks
/// from `--mask-extra` are applied on top before comparison.
fn diff_headers(a: &StepLog, b: &StepLog, extra: &[MaskRule]) -> Option<DivergenceKind> {
    let a_h = headers_for_diff(a, extra)?;
    let b_h = headers_for_diff(b, extra)?;

    let mut keys: BTreeSet<String> = BTreeSet::new();
    for k in a_h.keys() {
        keys.insert(k.to_lowercase());
    }
    for k in b_h.keys() {
        keys.insert(k.to_lowercase());
    }

    let mut deltas: Vec<HeaderDelta> = Vec::new();
    for key in keys {
        let a_val = lookup_ci(&a_h, &key);
        let b_val = lookup_ci(&b_h, &key);
        if a_val != b_val {
            deltas.push(HeaderDelta {
                header: key,
                a: a_val,
                b: b_val,
            });
        }
    }
    if deltas.is_empty() {
        None
    } else {
        Some(DivergenceKind::HeadersDiverged { diff: deltas })
    }
}

fn headers_for_diff(step: &StepLog, extra: &[MaskRule]) -> Option<HashMap<String, String>> {
    let base = step
        .response_headers_normalized
        .as_ref()
        .or(step.response_headers.as_ref())?
        .clone();
    if extra.is_empty() || !mask::has_header_rules(extra) {
        return Some(base);
    }
    Some(mask::normalize_headers(&base, extra))
}

fn lookup_ci(map: &HashMap<String, String>, lower_key: &str) -> Option<String> {
    map.iter()
        .find(|(k, _)| k.to_lowercase() == lower_key)
        .map(|(_, v)| v.clone())
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
    let from = step.state_before.clone();
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
            from: from.clone(),
            to: e.to.clone(),
            tag: e.tag.clone(),
            reason: outcome_reason(&e.outcome, &step.assertions),
        })
        .collect();
    RouteInfo {
        matched_edge_id: matched.map(|e| effective_id(e, step)).unwrap_or_default(),
        from: matched.map(|_| from).unwrap_or_default(),
        to: matched.map(|e| e.to.clone()).unwrap_or_default(),
        tag: matched.and_then(|e| e.tag.clone()),
        rejected,
    }
}

/// Render an edge as `from→to` with an optional ` (tag)` suffix. Preferred
/// over the bare 8-char hash for human-facing output. Falls back to the hash
/// when `from` is unavailable (legacy traces predating P0.4).
fn render_edge_label(from: &str, to: &str, tag: Option<&str>, fallback_hash: &str) -> String {
    if from.is_empty() && to.is_empty() {
        return fallback_hash.to_string();
    }
    let core = if from.is_empty() {
        format!("→{to}")
    } else {
        format!("{from}→{to}")
    };
    match tag {
        Some(t) => format!("{core} ({t})"),
        None => core,
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

/// Truncate `s` to `max` chars (boundary-safe), appending `…` when shortened.
/// Body payloads can be large; the diff is a one-liner, not a full dump.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cutoff: usize = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    format!("{}…", &s[..cutoff])
}

/// Summary state shared between the text/JSON renderers and the
/// `ACE_SUMMARY:` machine-readable line. Built once per `cmd_diff` invocation
/// so the verdict shown to humans and the JSON consumed by sinks can never
/// drift.
pub(crate) struct DiffSummaryLine {
    a: String,
    b: String,
    total_steps: usize,
    divergence_count: usize,
    affected_steps: usize,
}

impl DiffSummaryLine {
    fn new(a: &str, b: &str, total_steps: usize, divergences: &[Divergence]) -> Self {
        // Distinct (user, step, occurrence) tuples touched by at least one
        // divergence — the "N steps" half of "DRIFT — X changes across N steps".
        let mut keys: BTreeSet<(usize, String, usize)> = BTreeSet::new();
        for d in divergences {
            keys.insert((d.user, d.step.clone(), d.occurrence));
        }
        Self {
            a: a.to_string(),
            b: b.to_string(),
            total_steps,
            divergence_count: divergences.len(),
            affected_steps: keys.len(),
        }
    }

    fn verdict(&self) -> &'static str {
        if self.divergence_count == 0 {
            "CLEAN"
        } else {
            "DRIFT"
        }
    }

    fn human_line(&self) -> String {
        if self.divergence_count == 0 {
            format!(
                "ACE diff: CLEAN — {} step(s) compared, no changes · {} vs {}",
                self.total_steps, self.a, self.b
            )
        } else {
            format!(
                "ACE diff: DRIFT — {} change(s) across {} step(s) · {} vs {}",
                self.divergence_count, self.affected_steps, self.a, self.b
            )
        }
    }

    fn as_summary_line(&self) -> String {
        let payload = serde_json::json!({
            "v": glyph::SUMMARY_SCHEMA_VERSION,
            "command": "diff",
            "verdict": self.verdict(),
            "total_steps": self.total_steps,
            "divergences": self.divergence_count,
            "affected_steps": self.affected_steps,
            "a": self.a,
            "b": self.b,
        });
        format!(
            "{}{}",
            glyph::SUMMARY_PREFIX,
            serde_json::to_string(&payload).expect("summary payload serializes")
        )
    }
}

// ---------------------------------------------------------------------------
// Grouped structured renderer (P0.4 task 3)
//
// Both text and markdown output share this intermediate. A single audit of
// wording lands in both formats simultaneously, and there is no per-format
// duplication of grouping / ordering logic.
// ---------------------------------------------------------------------------

/// Build the per-step blocks. Divergences inside the same `(user, step,
/// occurrence)` are rendered under one header so the reader sees the step
/// once, not once per divergence.
fn group_by_step(divergences: &[Divergence]) -> Vec<(usize, String, usize, Vec<&Divergence>)> {
    let mut order: Vec<(usize, String, usize)> = Vec::new();
    let mut buckets: HashMap<(usize, String, usize), Vec<&Divergence>> = HashMap::new();
    for d in divergences {
        let key = (d.user, d.step.clone(), d.occurrence);
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().push(d);
    }
    order
        .into_iter()
        .map(|k| {
            let group = buckets.remove(&k).unwrap_or_default();
            (k.0, k.1, k.2, group)
        })
        .collect()
}

/// One-line headline for a divergence. Used by both renderers; differs only
/// in surrounding markup (markdown wraps it, text indents it).
fn divergence_headline(kind: &DivergenceKind) -> String {
    match kind {
        DivergenceKind::StepMissingInA { .. } => "step absent in trace-a".into(),
        DivergenceKind::StepMissingInB { .. } => "step absent in trace-b".into(),
        DivergenceKind::OutcomeDiverged { .. } => "outcome diverged".into(),
        DivergenceKind::RoutingDiverged { .. } => "routing diverged".into(),
        DivergenceKind::RejectionReasonChanged { edge_id, .. } => {
            format!("different rejection reason on edge {edge_id}")
        }
        DivergenceKind::EdgeOnlyInA { edge_id, to } => {
            format!("edge {edge_id} (→ {to}) evaluated in trace-a only")
        }
        DivergenceKind::EdgeOnlyInB { edge_id, to } => {
            format!("edge {edge_id} (→ {to}) evaluated in trace-b only")
        }
        DivergenceKind::BodyDiverged { .. } => "body diverged".into(),
        DivergenceKind::HeadersDiverged { .. } => "headers diverged".into(),
    }
}

/// Glyph for the headline. Absent kinds use `⊘`; everything else is a
/// divergence (`↯`).
fn divergence_glyph(kind: &DivergenceKind) -> &'static str {
    match kind {
        DivergenceKind::StepMissingInA { .. } | DivergenceKind::StepMissingInB { .. } => {
            glyph::ABSENT
        }
        _ => glyph::DIVERGED,
    }
}

fn render_text(
    divergences: &[Divergence],
    _total_steps: usize,
    summary: &DiffSummaryLine,
    ctx: &RenderContext,
) -> String {
    let mut out = String::new();
    out.push_str(&summary.human_line());
    out.push_str("\n\n");

    if divergences.is_empty() {
        return out;
    }

    for (user, step, occurrence, group) in group_by_step(divergences) {
        let occ_label = if occurrence > 0 {
            format!(" [{}]", occurrence)
        } else {
            String::new()
        };
        out.push_str(&format!("User {user} / step \"{step}\"{occ_label}\n"));
        for d in &group {
            out.push_str(&format!(
                "  {} {}\n",
                divergence_glyph(&d.kind),
                divergence_headline(&d.kind)
            ));
            render_divergence_body_text(&mut out, &d.kind);
        }
        // Masked summary renders once per step block, not once per divergence.
        let merged_masked = merge_masked(&group);
        if !merged_masked.is_empty() {
            out.push_str(&format!(
                "  {} masked: {}\n",
                glyph::NOTE,
                merged_masked.join(", ")
            ));
            if ctx.show_masked
                && let Some(pairs) = ctx.masked_values.get(&(user, step.clone(), occurrence))
            {
                for p in pairs {
                    let av = p.a.as_deref().unwrap_or("<absent>");
                    let bv = p.b.as_deref().unwrap_or("<absent>");
                    out.push_str(&format!("      {}\n", p.path));
                    out.push_str(&format!("        trace-a: {av}\n"));
                    out.push_str(&format!("        trace-b: {bv}\n"));
                }
            }
        }
        out.push('\n');
    }
    out
}

fn render_divergence_body_text(out: &mut String, kind: &DivergenceKind) {
    match kind {
        DivergenceKind::StepMissingInA { .. } | DivergenceKind::StepMissingInB { .. } => {}
        DivergenceKind::OutcomeDiverged {
            a_outcome,
            b_outcome,
        } => {
            out.push_str(&format!("      trace-a: {a_outcome}\n"));
            out.push_str(&format!("      trace-b: {b_outcome}\n"));
        }
        DivergenceKind::RoutingDiverged { a, b } => {
            for (label, route) in [("trace-a", a), ("trace-b", b)] {
                if !route.matched_edge_id.is_empty() {
                    out.push_str(&format!(
                        "      {label}: matched {}\n",
                        render_edge_label(
                            &route.from,
                            &route.to,
                            route.tag.as_deref(),
                            &route.matched_edge_id
                        )
                    ));
                } else {
                    out.push_str(&format!("      {label}: no match\n"));
                }
                for r in &route.rejected {
                    out.push_str(&format!(
                        "               rejected {}  [{}]\n",
                        render_edge_label(&r.from, &r.to, r.tag.as_deref(), &r.edge_id),
                        r.reason
                    ));
                }
            }
        }
        DivergenceKind::RejectionReasonChanged {
            a_reason, b_reason, ..
        } => {
            out.push_str(&format!("      trace-a: {a_reason}\n"));
            out.push_str(&format!("      trace-b: {b_reason}\n"));
        }
        DivergenceKind::EdgeOnlyInA { .. } | DivergenceKind::EdgeOnlyInB { .. } => {}
        DivergenceKind::BodyDiverged { a, b } => {
            out.push_str(&format!("      trace-a: {}\n", truncate(a, 200)));
            out.push_str(&format!("      trace-b: {}\n", truncate(b, 200)));
        }
        DivergenceKind::HeadersDiverged { diff } => {
            for d in diff {
                let av = d.a.as_deref().unwrap_or("<absent>");
                let bv = d.b.as_deref().unwrap_or("<absent>");
                out.push_str(&format!("      {}: a={} b={}\n", d.header, av, bv));
            }
        }
    }
}

/// Union of `masked` field lists across all divergences in a step block.
/// Stable order so snapshot tests don't flap.
fn merge_masked(group: &[&Divergence]) -> Vec<String> {
    let mut all: Vec<String> = group
        .iter()
        .flat_map(|d| d.masked.iter().cloned())
        .collect();
    all.sort();
    all.dedup();
    all
}

/// Markdown form for GitHub PR comments. Same content as the text renderer,
/// just wrapped in headings and `<details>` so a 100-divergence diff doesn't
/// dominate the comment by default.
fn render_markdown(
    divergences: &[Divergence],
    summary: &DiffSummaryLine,
    ctx: &RenderContext,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("### {}\n\n", summary.human_line()));
    out.push_str(&format!(
        "`{}` vs `{}`\n\n",
        summary.a.replace('`', "\\`"),
        summary.b.replace('`', "\\`")
    ));

    if divergences.is_empty() {
        out.push_str("_No divergences._\n");
        return out;
    }

    for (user, step, occurrence, group) in group_by_step(divergences) {
        let occ_label = if occurrence > 0 {
            format!(" `[{occurrence}]`")
        } else {
            String::new()
        };
        out.push_str(&format!("#### `{step}`{occ_label} — User {user}\n\n"));
        for d in &group {
            out.push_str(&format!(
                "- **{}** {}\n",
                divergence_glyph(&d.kind),
                divergence_headline(&d.kind)
            ));
            render_divergence_body_md(&mut out, &d.kind);
        }
        let merged_masked = merge_masked(&group);
        if !merged_masked.is_empty() {
            out.push_str("\n<details><summary>masked fields</summary>\n\n");
            for m in &merged_masked {
                out.push_str(&format!("- `{m}`\n"));
            }
            if ctx.show_masked
                && let Some(pairs) = ctx.masked_values.get(&(user, step.clone(), occurrence))
            {
                out.push_str("\n| path | trace-a | trace-b |\n|---|---|---|\n");
                for p in pairs {
                    let av = p.a.as_deref().unwrap_or("<absent>");
                    let bv = p.b.as_deref().unwrap_or("<absent>");
                    out.push_str(&format!(
                        "| `{}` | `{}` | `{}` |\n",
                        p.path,
                        av.replace('|', "\\|"),
                        bv.replace('|', "\\|"),
                    ));
                }
            }
            out.push_str("\n</details>\n");
        }
        out.push('\n');
    }
    out
}

fn render_divergence_body_md(out: &mut String, kind: &DivergenceKind) {
    match kind {
        DivergenceKind::StepMissingInA { .. } | DivergenceKind::StepMissingInB { .. } => {}
        DivergenceKind::OutcomeDiverged {
            a_outcome,
            b_outcome,
        } => {
            out.push_str(&format!("  - trace-a: `{a_outcome}`\n"));
            out.push_str(&format!("  - trace-b: `{b_outcome}`\n"));
        }
        DivergenceKind::RoutingDiverged { a, b } => {
            for (label, route) in [("trace-a", a), ("trace-b", b)] {
                if !route.matched_edge_id.is_empty() {
                    out.push_str(&format!(
                        "  - {label} matched → `{}`\n",
                        render_edge_label(
                            &route.from,
                            &route.to,
                            route.tag.as_deref(),
                            &route.matched_edge_id
                        )
                    ));
                } else {
                    out.push_str(&format!("  - {label}: no match\n"));
                }
                for r in &route.rejected {
                    out.push_str(&format!(
                        "    - rejected `{}` — {}\n",
                        render_edge_label(&r.from, &r.to, r.tag.as_deref(), &r.edge_id),
                        r.reason
                    ));
                }
            }
        }
        DivergenceKind::RejectionReasonChanged {
            a_reason, b_reason, ..
        } => {
            out.push_str(&format!("  - trace-a: {a_reason}\n"));
            out.push_str(&format!("  - trace-b: {b_reason}\n"));
        }
        DivergenceKind::EdgeOnlyInA { .. } | DivergenceKind::EdgeOnlyInB { .. } => {}
        DivergenceKind::BodyDiverged { a, b } => {
            out.push_str(&format!("  - trace-a: `{}`\n", truncate(a, 200)));
            out.push_str(&format!("  - trace-b: `{}`\n", truncate(b, 200)));
        }
        DivergenceKind::HeadersDiverged { diff } => {
            for d in diff {
                let av = d.a.as_deref().unwrap_or("<absent>");
                let bv = d.b.as_deref().unwrap_or("<absent>");
                out.push_str(&format!("  - `{}`: a=`{}` b=`{}`\n", d.header, av, bv));
            }
        }
    }
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
            response_body_normalized: None,
            response_headers: None,
            response_headers_normalized: None,
            masked_headers: Vec::new(),
            masked_fields: Vec::new(),
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
                    .flat_map(|sp| diff_step(p.user_idx, &sp, &[]))
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

        let result = cmd_diff(DiffArgs {
            a: fa.path().to_str().unwrap().to_string(),
            b: fb.path().to_str().unwrap().to_string(),
            format: "text".into(),
            output: None,
            mask_extra: None,
            show_masked: false,
            quiet: false,
        });
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

        let result = cmd_diff(DiffArgs {
            a: fa.path().to_str().unwrap().to_string(),
            b: fb.path().to_str().unwrap().to_string(),
            format: "text".into(),
            output: Some(out.path().to_str().unwrap().to_string()),
            mask_extra: None,
            show_masked: false,
            quiet: false,
        });
        assert!(matches!(result, Err(CliError::DiffFound)));
        let rendered = fs::read_to_string(out.path()).unwrap();
        assert!(rendered.contains("routing diverged"));
    }

    #[test]
    fn diff_rejects_unknown_format() {
        let result = DiffFormat::parse("xml");
        assert!(matches!(result, Err(CliError::BadArgument(_))));
    }

    #[test]
    fn diff_accepts_markdown_format() {
        assert!(DiffFormat::parse("markdown").is_ok());
        assert!(DiffFormat::parse("md").is_ok());
    }

    #[test]
    fn jsonpath_to_pointer_basic() {
        assert_eq!(
            super::jsonpath_to_pointer("$.foo.bar").as_deref(),
            Some("/foo/bar")
        );
        // Unsupported subset returns None — caller renders <absent>.
        assert!(super::jsonpath_to_pointer("$.foo[0]").is_none());
        assert!(super::jsonpath_to_pointer("$..created").is_none());
    }

    #[test]
    fn show_masked_renders_pre_mask_values() {
        // Two traces share a masked path with different raw values. With
        // --show-masked the renderer must surface both pre-mask values so a
        // user can audit the masking. Without the flag the body diff is
        // silenced (covered elsewhere); this test only checks the audit
        // surface.
        let mut a = step_with_body(
            "fetch",
            r#"{"id":"sub_abc"}"#,
            Some(serde_json::json!({"id":"<MASKED>"})),
        );
        a.masked_fields = vec!["$.id".into()];
        let mut b = step_with_body(
            "fetch",
            r#"{"id":"sub_xyz"}"#,
            Some(serde_json::json!({"id":"<MASKED>"})),
        );
        b.masked_fields = vec!["$.id".into()];
        // Force a routing divergence so there's a step block to render.
        a.edge_evaluations = vec![matched_eval("e_a", "paid")];
        b.edge_evaluations = vec![matched_eval("e_b", "retry")];
        a.state_after = "paid".into();
        b.state_after = "retry".into();

        let logs_a = vec![make_log(vec![a])];
        let logs_b = vec![make_log(vec![b])];

        let pairs = align_logs(&logs_a, &logs_b);
        let mut divs: Vec<Divergence> = Vec::new();
        let mut masked_values: HashMap<(usize, String, usize), Vec<MaskedValuePair>> =
            HashMap::new();
        for pair in &pairs {
            for sp in align_steps(&pair.a.steps, &pair.b.steps) {
                let pairs_for_step = collect_masked_value_pairs(&sp);
                if !pairs_for_step.is_empty() {
                    masked_values.insert(
                        (pair.user_idx, sp.step_name.clone(), sp.occurrence),
                        pairs_for_step,
                    );
                }
                divs.extend(diff_step(pair.user_idx, &sp, &[]));
            }
        }

        let summary = DiffSummaryLine::new("a.json", "b.json", 1, &divs);
        let ctx = RenderContext {
            show_masked: true,
            masked_values: &masked_values,
        };
        let text = render_text(&divs, 1, &summary, &ctx);
        assert!(
            text.contains("sub_abc") && text.contains("sub_xyz"),
            "--show-masked must surface pre-mask values; got:\n{text}"
        );
    }

    #[test]
    fn render_markdown_uses_step_headers_and_summary() {
        let a = make_log(vec![make_step(
            "pay",
            "cart",
            "paid",
            vec![matched_eval("e1", "paid")],
        )]);
        let b = make_log(vec![make_step(
            "pay",
            "cart",
            "retry",
            vec![matched_eval("e2", "retry")],
        )]);
        let divs = run_diff(vec![a], vec![b]);
        let summary = DiffSummaryLine::new("staging.json", "prod.json", 1, &divs);
        let ctx = RenderContext {
            show_masked: false,
            masked_values: &HashMap::new(),
        };
        let md = render_markdown(&divs, &summary, &ctx);
        assert!(
            md.starts_with("### ACE diff: DRIFT"),
            "verdict heading missing"
        );
        assert!(md.contains("#### `pay`"), "step subheading missing");
        assert!(md.contains("cart→paid") && md.contains("cart→retry"));
    }

    // -----------------------------------------------------------------------
    // BodyDiverged + HeadersDiverged + masking suppression
    // -----------------------------------------------------------------------

    fn run_diff_with_extra(
        logs_a: Vec<ExecutionLog>,
        logs_b: Vec<ExecutionLog>,
        extra: &[MaskRule],
    ) -> Vec<Divergence> {
        let pairs = align_logs(&logs_a, &logs_b);
        pairs
            .iter()
            .flat_map(|p| {
                align_steps(&p.a.steps, &p.b.steps)
                    .into_iter()
                    .flat_map(|sp| diff_step(p.user_idx, &sp, extra))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn step_with_body(name: &str, raw: &str, normalized: Option<serde_json::Value>) -> StepLog {
        let mut s = make_step(name, "a", "b", vec![matched_eval("e1", "b")]);
        s.response_body = Some(raw.to_string());
        s.response_body_normalized = normalized;
        s
    }

    fn step_with_headers(
        name: &str,
        raw: HashMap<String, String>,
        normalized: Option<HashMap<String, String>>,
    ) -> StepLog {
        let mut s = make_step(name, "a", "b", vec![matched_eval("e1", "b")]);
        s.response_headers = Some(raw);
        s.response_headers_normalized = normalized;
        s
    }

    #[test]
    fn body_diverged_when_normalized_bodies_differ() {
        // Both steps have normalized bodies (scenario `mask:` already applied
        // at capture time). Their content differs in a non-masked field —
        // diff must surface this as BodyDiverged.
        let a = make_log(vec![step_with_body(
            "fetch",
            r#"{"id":"sub_1","status":"active"}"#,
            Some(serde_json::json!({"id":"sub_1","status":"active"})),
        )]);
        let b = make_log(vec![step_with_body(
            "fetch",
            r#"{"id":"sub_1","status":"canceled"}"#,
            Some(serde_json::json!({"id":"sub_1","status":"canceled"})),
        )]);
        let divs = run_diff(vec![a], vec![b]);
        assert_eq!(divs.len(), 1, "expected exactly one body divergence");
        match &divs[0].kind {
            DivergenceKind::BodyDiverged { a, b } => {
                assert!(a.contains("active"), "trace-a repr should contain 'active'");
                assert!(
                    b.contains("canceled"),
                    "trace-b repr should contain 'canceled'"
                );
            }
            other => panic!("expected BodyDiverged, got {:?}", other),
        }
    }

    #[test]
    fn body_diff_suppressed_when_only_masked_field_differs() {
        // The most important assertion: if the only difference between two
        // bodies is a field that scenario masking already replaced with the
        // same placeholder, the diff must be silent. This is the whole point
        // of P0.3 — without this, masking is decorative.
        let a = make_log(vec![step_with_body(
            "fetch",
            r#"{"id":"sub_1","current_period_end":1700000000}"#,
            // Normalized: timestamp masked → identical placeholder on both sides.
            Some(serde_json::json!({"id":"sub_1","current_period_end":"<MASKED>"})),
        )]);
        let b = make_log(vec![step_with_body(
            "fetch",
            r#"{"id":"sub_1","current_period_end":1700099999}"#,
            Some(serde_json::json!({"id":"sub_1","current_period_end":"<MASKED>"})),
        )]);
        let divs = run_diff(vec![a], vec![b]);
        assert!(
            divs.is_empty(),
            "masked-field difference must not produce a divergence; got {:?}",
            divs
        );
    }

    #[test]
    fn body_diff_suppressed_by_extra_mask_rule_at_diff_time() {
        // No scenario masks were applied at capture time, so the raw bodies
        // differ in a per-request timestamp. Passing `--mask-extra` at diff
        // time must apply the mask before comparison and suppress the noise.
        let a = make_log(vec![step_with_body(
            "fetch",
            r#"{"id":"sub_1","current_period_end":1700000000}"#,
            None,
        )]);
        let b = make_log(vec![step_with_body(
            "fetch",
            r#"{"id":"sub_1","current_period_end":1700099999}"#,
            None,
        )]);
        let extra = vec![MaskRule::JsonPath {
            path: "$.current_period_end".into(),
            replacement: "<TS>".into(),
        }];
        let divs = run_diff_with_extra(vec![a], vec![b], &extra);
        assert!(
            divs.is_empty(),
            "extra-mask rule must suppress the timestamp diff; got {:?}",
            divs
        );
    }

    #[test]
    fn body_diff_skipped_when_routing_diverges() {
        // When routing already diverged, comparing bodies adds noise — the
        // bodies necessarily came from different requests. Only the routing
        // divergence should be emitted.
        let mut a_step = make_step("c", "cart", "paid", vec![matched_eval("e_a", "paid")]);
        a_step.response_body = Some(r#"{"flow":"happy"}"#.to_string());
        let mut b_step = make_step("c", "cart", "retry", vec![matched_eval("e_b", "retry")]);
        b_step.response_body = Some(r#"{"flow":"sad"}"#.to_string());
        let divs = run_diff(vec![make_log(vec![a_step])], vec![make_log(vec![b_step])]);
        assert_eq!(
            divs.len(),
            1,
            "only routing divergence expected, got {:?}",
            divs
        );
        assert!(matches!(
            divs[0].kind,
            DivergenceKind::RoutingDiverged { .. }
        ));
    }

    #[test]
    fn body_diff_silent_when_no_body_captured() {
        // No body fields on either side (default capture behavior in
        // non-verbose runs without masks). Body diff must not fire.
        let a = make_log(vec![make_step("s", "a", "b", vec![matched_eval("e", "b")])]);
        let b = make_log(vec![make_step("s", "a", "b", vec![matched_eval("e", "b")])]);
        assert!(run_diff(vec![a], vec![b]).is_empty());
    }

    #[test]
    fn body_diff_falls_back_to_raw_when_no_normalized() {
        // Verbose run (raw body present, no normalized). Bodies differ in a
        // non-masked way → BodyDiverged from raw comparison.
        let a = make_log(vec![step_with_body("s", r#"{"x":1}"#, None)]);
        let b = make_log(vec![step_with_body("s", r#"{"x":2}"#, None)]);
        let divs = run_diff(vec![a], vec![b]);
        assert_eq!(divs.len(), 1);
        assert!(matches!(divs[0].kind, DivergenceKind::BodyDiverged { .. }));
    }

    #[test]
    fn body_diff_canonicalizes_key_order() {
        // Same JSON content with different key insertion order must compare
        // equal. Relies on serde_json's default BTreeMap-backed Map.
        let a = make_log(vec![step_with_body("s", r#"{"a":1,"b":2}"#, None)]);
        let b = make_log(vec![step_with_body("s", r#"{"b":2,"a":1}"#, None)]);
        assert!(
            run_diff(vec![a], vec![b]).is_empty(),
            "key-order difference must not be a divergence"
        );
    }

    #[test]
    fn headers_diverged_when_captured_and_differ() {
        // Both steps captured headers (scenario declared a header mask rule).
        // After scenario masking, one header still differs → HeadersDiverged.
        let mut a_raw = HashMap::new();
        a_raw.insert("X-Request-Id".to_string(), "abc".to_string());
        a_raw.insert("Content-Type".to_string(), "application/json".to_string());
        let mut a_norm = a_raw.clone();
        a_norm.insert("X-Request-Id".to_string(), "<MASKED>".to_string());

        let mut b_raw = HashMap::new();
        b_raw.insert("X-Request-Id".to_string(), "xyz".to_string());
        b_raw.insert("Content-Type".to_string(), "text/html".to_string());
        let mut b_norm = b_raw.clone();
        b_norm.insert("X-Request-Id".to_string(), "<MASKED>".to_string());

        let a = make_log(vec![step_with_headers("s", a_raw, Some(a_norm))]);
        let b = make_log(vec![step_with_headers("s", b_raw, Some(b_norm))]);
        let divs = run_diff(vec![a], vec![b]);
        assert_eq!(divs.len(), 1);
        match &divs[0].kind {
            DivergenceKind::HeadersDiverged { diff } => {
                // X-Request-Id was masked to the same placeholder, so it
                // must NOT appear in the diff. Content-Type genuinely differs.
                let headers: Vec<_> = diff.iter().map(|d| d.header.as_str()).collect();
                assert!(headers.contains(&"content-type"));
                assert!(!headers.contains(&"x-request-id"));
            }
            other => panic!("expected HeadersDiverged, got {:?}", other),
        }
    }

    #[test]
    fn headers_diff_silent_when_no_headers_captured() {
        // No header capture (scenario didn't declare any header mask rules).
        // No HeadersDiverged should fire even when a routing match would
        // otherwise let it be emitted.
        let a = make_log(vec![make_step("s", "a", "b", vec![matched_eval("e", "b")])]);
        let b = make_log(vec![make_step("s", "a", "b", vec![matched_eval("e", "b")])]);
        assert!(run_diff(vec![a], vec![b]).is_empty());
    }

    #[test]
    fn headers_diff_suppressed_by_extra_header_mask() {
        // Raw headers differ on X-Request-Id; passing an extra header mask
        // at diff time must normalize them to the same placeholder and
        // suppress the divergence.
        let mut a_raw = HashMap::new();
        a_raw.insert("X-Request-Id".to_string(), "abc".to_string());
        let mut b_raw = HashMap::new();
        b_raw.insert("X-Request-Id".to_string(), "xyz".to_string());

        let a = make_log(vec![step_with_headers("s", a_raw, None)]);
        let b = make_log(vec![step_with_headers("s", b_raw, None)]);
        let extra = vec![MaskRule::Header {
            header: "x-request-id".into(),
            replacement: "<RID>".into(),
        }];
        let divs = run_diff_with_extra(vec![a], vec![b], &extra);
        assert!(
            divs.is_empty(),
            "extra header mask must suppress request-id diff; got {:?}",
            divs
        );
    }
}
