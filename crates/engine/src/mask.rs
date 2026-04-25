//! Dynamic-field masking applied at trace capture time. The path syntax is
//! a deliberate subset of JSONPath:
//!
//!   - `$.foo`           — top-level key
//!   - `$..foo`          — recursive descent on a single bare key (all depths)
//!   - `$.foo.bar`       — nested keys via dot
//!   - `$.foo[*].bar`    — array wildcard (every element)
//!
//! NOT supported: numeric indices (`$.foo[0]`), filters (`$..foo[?(...)]`),
//! slices (`$.foo[1:3]`), or wildcards in middle (`$.*.bar`). The validator
//! (rule E021) rejects unsupported syntax at scenario-load time so users
//! find out immediately rather than seeing silent zero-match runs.

use model::MaskRule;
use serde_json::Value;
use std::collections::HashMap;

/// Parse `body` as JSON, apply all `JsonPath` rules, and return the normalized
/// value together with the sorted, deduped list of JSONPath patterns that
/// actually matched at least one node.
///
/// Returns `None` when `body` is not valid JSON.
pub fn normalize_body_tracked(body: &str, rules: &[MaskRule]) -> Option<(Value, Vec<String>)> {
    let mut v: Value = serde_json::from_str(body).ok()?;
    let mut matched: Vec<String> = Vec::new();
    for rule in rules {
        if let MaskRule::JsonPath { path, replacement } = rule {
            let hits = apply_path_mask(&mut v, path, replacement);
            if hits > 0 {
                matched.push(path.clone());
            }
        }
    }
    // Stable order so repeated runs produce identical traces and
    // `ace show`/`ace diff` don't flag spurious churn.
    matched.sort();
    matched.dedup();
    Some((v, matched))
}

/// Parse `body` as JSON, apply all `JsonPath` rules, and return the normalized
/// value. Returns `None` when `body` is not valid JSON.
pub fn normalize_body(body: &str, rules: &[MaskRule]) -> Option<Value> {
    normalize_body_tracked(body, rules).map(|(v, _)| v)
}

/// Apply all `Header` rules to `headers` (case-insensitive key comparison).
pub fn normalize_headers(
    headers: &HashMap<String, String>,
    rules: &[MaskRule],
) -> HashMap<String, String> {
    normalize_headers_tracked(headers, rules).0
}

/// Same as `normalize_headers`, but also returns the sorted, deduped list of
/// rule headers (lowercase) that actually matched at least one header in
/// `headers`. Lets `ace diff` say "masked headers: …" without re-walking.
pub fn normalize_headers_tracked(
    headers: &HashMap<String, String>,
    rules: &[MaskRule],
) -> (HashMap<String, String>, Vec<String>) {
    let mut out = headers.clone();
    let mut matched: Vec<String> = Vec::new();
    for rule in rules {
        if let MaskRule::Header {
            header,
            replacement,
        } = rule
        {
            let key_lower = header.to_lowercase();
            let mut hit = false;
            for k in out.keys().cloned().collect::<Vec<_>>() {
                if k.to_lowercase() == key_lower {
                    out.insert(k, replacement.clone());
                    hit = true;
                }
            }
            if hit {
                matched.push(key_lower);
            }
        }
    }
    matched.sort();
    matched.dedup();
    (out, matched)
}

/// Returns true if any rule in `rules` is a `Header` rule. Used by the runner
/// to decide whether to capture response headers in the trace.
pub fn has_header_rules(rules: &[MaskRule]) -> bool {
    rules.iter().any(|r| matches!(r, MaskRule::Header { .. }))
}

// ---------------------------------------------------------------------------
// JSONPath pattern application
// ---------------------------------------------------------------------------

/// Apply a single JSONPath `path` rule to `value`, replacing every matching
/// leaf with `replacement`. Returns the number of replacements made.
fn apply_path_mask(value: &mut Value, path: &str, replacement: &str) -> usize {
    let rest = match path.strip_prefix('$') {
        Some(r) => r,
        None => return 0,
    };

    if let Some(key) = rest.strip_prefix("..") {
        apply_recursive_mask(value, key, replacement)
    } else if let Some(rest) = rest.strip_prefix('.') {
        let segs = parse_segments(rest);
        apply_segments_mask(value, &segs, replacement)
    } else {
        0
    }
}

/// Recursively descend through every object/array and replace any object key
/// named `key` at any depth.
fn apply_recursive_mask(value: &mut Value, key: &str, replacement: &str) -> usize {
    let mut count = 0;
    match value {
        Value::Object(map) => {
            if map.contains_key(key) {
                map.insert(key.to_string(), Value::String(replacement.to_string()));
                count += 1;
            }
            for v in map.values_mut() {
                count += apply_recursive_mask(v, key, replacement);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                count += apply_recursive_mask(v, key, replacement);
            }
        }
        _ => {}
    }
    count
}

#[derive(Debug)]
enum Seg {
    Key(String),
    Wildcard,
}

/// Parse a dot-notation path (with optional `[*]` wildcards) into segments.
/// `"data[*].id"` → `[Key("data"), Wildcard, Key("id")]`.
fn parse_segments(path: &str) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            continue;
        }
        if let Some(pos) = part.find("[*]") {
            let key_part = &part[..pos];
            if !key_part.is_empty() {
                segs.push(Seg::Key(key_part.to_string()));
            }
            segs.push(Seg::Wildcard);
        } else {
            segs.push(Seg::Key(part.to_string()));
        }
    }
    segs
}

fn apply_segments_mask(value: &mut Value, segs: &[Seg], replacement: &str) -> usize {
    match segs {
        [] => 0,
        [Seg::Key(k)] => {
            if let Value::Object(map) = value
                && map.contains_key(k.as_str())
            {
                map.insert(k.clone(), Value::String(replacement.to_string()));
                return 1;
            }
            0
        }
        [Seg::Key(k), rest @ ..] => {
            if let Value::Object(map) = value
                && let Some(child) = map.get_mut(k.as_str())
            {
                return apply_segments_mask(child, rest, replacement);
            }
            0
        }
        [Seg::Wildcard, rest @ ..] => {
            if let Value::Array(arr) = value {
                let mut count = 0;
                for elem in arr.iter_mut() {
                    count += apply_segments_mask(elem, rest, replacement);
                }
                return count;
            }
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(paths: &[&str]) -> Vec<MaskRule> {
        paths
            .iter()
            .map(|p| MaskRule::JsonPath {
                path: p.to_string(),
                replacement: "<MASKED>".to_string(),
            })
            .collect()
    }

    fn header_rules(headers: &[&str]) -> Vec<MaskRule> {
        headers
            .iter()
            .map(|h| MaskRule::Header {
                header: h.to_string(),
                replacement: "<MASKED>".to_string(),
            })
            .collect()
    }

    #[test]
    fn top_level_key() {
        let body = r#"{"id":"abc","name":"Alice"}"#;
        let (v, matched) = normalize_body_tracked(body, &rules(&["$.id"])).unwrap();
        assert_eq!(v["id"], json!("<MASKED>"));
        assert_eq!(v["name"], json!("Alice"));
        assert_eq!(matched, vec!["$.id"]);
    }

    #[test]
    fn recursive_descent() {
        let body = r#"{"created":1,"nested":{"created":2}}"#;
        let (v, matched) = normalize_body_tracked(body, &rules(&["$..created"])).unwrap();
        assert_eq!(v["created"], json!("<MASKED>"));
        assert_eq!(v["nested"]["created"], json!("<MASKED>"));
        assert_eq!(matched, vec!["$..created"]);
    }

    #[test]
    fn array_wildcard() {
        let body = r#"{"data":[{"id":"x"},{"id":"y"}]}"#;
        let (v, _) = normalize_body_tracked(body, &rules(&["$.data[*].id"])).unwrap();
        assert_eq!(v["data"][0]["id"], json!("<MASKED>"));
        assert_eq!(v["data"][1]["id"], json!("<MASKED>"));
    }

    #[test]
    fn nested_path() {
        let body = r#"{"a":{"b":{"c":"secret"}}}"#;
        let (v, _) = normalize_body_tracked(body, &rules(&["$.a.b.c"])).unwrap();
        assert_eq!(v["a"]["b"]["c"], json!("<MASKED>"));
    }

    #[test]
    fn non_json_body_returns_none() {
        let result = normalize_body("not json", &rules(&["$.id"]));
        assert!(result.is_none());
    }

    #[test]
    fn unmatched_path_not_in_matched_list() {
        let body = r#"{"name":"Alice"}"#;
        let (_, matched) = normalize_body_tracked(body, &rules(&["$.id"])).unwrap();
        assert!(matched.is_empty());
    }

    #[test]
    fn matched_paths_are_sorted_and_deduped() {
        let body = r#"{"id":"x","z":1,"a":"y"}"#;
        // Insertion order on the rule list is z, a, id; expect alphabetic out.
        let (_, matched) =
            normalize_body_tracked(body, &rules(&["$.z", "$.a", "$.id", "$.id"])).unwrap();
        assert_eq!(matched, vec!["$.a", "$.id", "$.z"]);
    }

    #[test]
    fn header_mask_case_insensitive() {
        let mut headers = HashMap::new();
        headers.insert("X-Request-Id".to_string(), "abc123".to_string());
        headers.insert("content-type".to_string(), "application/json".to_string());

        let result = normalize_headers(&headers, &header_rules(&["x-request-id"]));
        assert_eq!(result["X-Request-Id"], "<MASKED>");
        assert_eq!(result["content-type"], "application/json");
    }

    #[test]
    fn header_mask_tracked_returns_lowercased_matched_only() {
        let mut headers = HashMap::new();
        headers.insert("X-Request-Id".to_string(), "abc".to_string());
        headers.insert("Date".to_string(), "Mon".to_string());

        let rules = header_rules(&["X-Request-Id", "Date", "Missing-Header"]);
        let (norm, matched) = normalize_headers_tracked(&headers, &rules);
        assert_eq!(norm["X-Request-Id"], "<MASKED>");
        assert_eq!(norm["Date"], "<MASKED>");
        assert_eq!(matched, vec!["date", "x-request-id"]);
    }

    #[test]
    fn has_header_rules_detects_only_header_kind() {
        assert!(!has_header_rules(&rules(&["$.id"])));
        assert!(has_header_rules(&header_rules(&["x-id"])));
        let mixed = vec![
            MaskRule::JsonPath {
                path: "$.id".into(),
                replacement: "x".into(),
            },
            MaskRule::Header {
                header: "x-id".into(),
                replacement: "x".into(),
            },
        ];
        assert!(has_header_rules(&mixed));
    }

    #[test]
    fn no_rules_body_unchanged() {
        let body = r#"{"id":"abc"}"#;
        let (v, matched) = normalize_body_tracked(body, &[]).unwrap();
        assert_eq!(v["id"], json!("abc"));
        assert!(matched.is_empty());
    }

    #[test]
    fn recursive_descent_in_array() {
        let body = r#"{"items":[{"ts":1},{"ts":2}]}"#;
        let (v, _) = normalize_body_tracked(body, &rules(&["$..ts"])).unwrap();
        assert_eq!(v["items"][0]["ts"], json!("<MASKED>"));
        assert_eq!(v["items"][1]["ts"], json!("<MASKED>"));
    }

    #[test]
    fn custom_replacement_string() {
        let body = r#"{"id":"abc"}"#;
        let r = vec![MaskRule::JsonPath {
            path: "$.id".to_string(),
            replacement: "<ID>".to_string(),
        }];
        let v = normalize_body(body, &r).unwrap();
        assert_eq!(v["id"], json!("<ID>"));
    }
}
