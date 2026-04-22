//! Secret redaction for execution logs.
//!
//! Scrubs URL query params, JSON/form body values, and assertion `actual`/`expected`
//! strings whose key matches a built-in list of sensitive substrings (token,
//! password, api_key, …). Assertion evaluation runs on the real value — only the
//! display form that lands in `execution_log.json` is masked.

use crate::assertions::AssertionResult;
use serde_json::Value;

pub const MASK: &str = "***";

/// Substrings that mark a key as sensitive. Matched case-insensitively as a
/// substring of the key (so `access_token` matches `token`, `client_secret`
/// matches `secret`). Entries are chosen so one substring covers a family.
const DEFAULT_SENSITIVE_KEYS: &[&str] = &[
    "token",
    "password",
    "passwd",
    "secret",
    "authorization",
    "api_key",
    "apikey",
    "api-key",
    "cookie",
    "bearer",
    "credential",
    "private_key",
    "private-key",
    "privatekey",
    "session",
    "passphrase",
];

#[derive(Debug, Clone)]
pub struct Redactor {
    enabled: bool,
    include_bodies: bool,
    max_body_bytes: usize,
    extra_mask: Vec<String>,
    unmask: Vec<String>,
}

impl Redactor {
    pub fn new(
        enabled: bool,
        include_bodies: bool,
        max_body_bytes: usize,
        extra_mask: Vec<String>,
        unmask: Vec<String>,
    ) -> Self {
        Self {
            enabled,
            include_bodies,
            max_body_bytes,
            extra_mask: extra_mask.into_iter().map(|s| s.to_ascii_lowercase()).collect(),
            unmask: unmask.into_iter().map(|s| s.to_ascii_lowercase()).collect(),
        }
    }

    pub fn include_bodies(&self) -> bool {
        self.include_bodies
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// True if the given key name should be masked. Case-insensitive substring
    /// match against the built-in list + `extra_mask`; `unmask` overrides.
    pub fn is_sensitive_key(&self, key: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let lower = key.to_ascii_lowercase();
        if self.unmask.iter().any(|u| u == &lower) {
            return false;
        }
        DEFAULT_SENSITIVE_KEYS.iter().any(|k| lower.contains(k))
            || self.extra_mask.iter().any(|k| lower.contains(k.as_str()))
    }

    /// True if any segment of a dotted JSONPath is sensitive.
    /// `body.user.api_key` → true; `body.profile.name` → false.
    pub fn is_sensitive_path(&self, path: &str) -> bool {
        path.split('.').any(|seg| self.is_sensitive_key(seg))
    }

    /// Redact a URL: strip userinfo (user:pass@), mask sensitive query params.
    /// Returns the input unchanged if redaction is disabled.
    pub fn redact_url(&self, url: &str) -> String {
        if !self.enabled {
            return url.to_string();
        }

        let (scheme_end, rest_start) = match url.find("://") {
            Some(i) => (i + 3, i + 3),
            None => (0, 0),
        };
        let scheme = &url[..scheme_end];
        let rest = &url[rest_start..];

        // Authority runs up to the first of `/`, `?`, or `#` — whichever
        // appears first. Missing the `#` case meant `https://h#frag` dropped
        // the fragment into the authority and lost it.
        let authority_end = rest
            .find(|c| c == '/' || c == '?' || c == '#')
            .unwrap_or(rest.len());
        let path_and_query = &rest[authority_end..];
        let authority = &rest[..authority_end];

        let authority_scrubbed = match authority.rfind('@') {
            Some(i) => format!("{}@{}", MASK, &authority[i + 1..]),
            None => authority.to_string(),
        };

        let (path_only, query_and_frag) = match path_and_query.find('?') {
            Some(i) => (&path_and_query[..i], &path_and_query[i..]),
            None => (path_and_query, ""),
        };

        let rebuilt_query = if let Some(q_rest) = query_and_frag.strip_prefix('?') {
            let (query, frag) = match q_rest.find('#') {
                Some(i) => (&q_rest[..i], &q_rest[i..]),
                None => (q_rest, ""),
            };
            let scrubbed: Vec<String> = query
                .split('&')
                .map(|pair| match pair.split_once('=') {
                    Some((k, _)) if self.is_sensitive_key(k) => format!("{}={}", k, MASK),
                    _ => pair.to_string(),
                })
                .collect();
            format!("?{}{}", scrubbed.join("&"), frag)
        } else {
            String::new()
        };

        format!("{}{}{}{}", scheme, authority_scrubbed, path_only, rebuilt_query)
    }

    /// Redact a body string. Tries JSON first, falls back to form-encoded, else
    /// returns unchanged. Applies body size cap and include_bodies gate.
    pub fn redact_body(&self, body: &str) -> Option<String> {
        if !self.include_bodies {
            return None;
        }

        let redacted = if !self.enabled {
            body.to_string()
        } else if let Ok(mut val) = serde_json::from_str::<Value>(body) {
            self.redact_json_value(&mut val);
            serde_json::to_string(&val).unwrap_or_else(|_| body.to_string())
        } else if looks_form_encoded(body) {
            self.redact_form_body(body)
        } else {
            body.to_string()
        };

        Some(self.truncate(redacted))
    }

    fn redact_form_body(&self, body: &str) -> String {
        body.split('&')
            .map(|pair| match pair.split_once('=') {
                Some((k, _)) if self.is_sensitive_key(k) => format!("{}={}", k, MASK),
                _ => pair.to_string(),
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Walk a JSON value, replacing values whose key is sensitive with `"***"`.
    /// For arrays: applies recursively but the array index is not a key, so
    /// elements inherit the parent's mask decision (we pass `inherit_mask`).
    pub fn redact_json_value(&self, val: &mut Value) {
        self.redact_json_value_inner(val, false);
    }

    fn redact_json_value_inner(&self, val: &mut Value, inherit_mask: bool) {
        match val {
            Value::Object(map) => {
                for (k, v) in map.iter_mut() {
                    let masked = inherit_mask || self.is_sensitive_key(k);
                    if masked && !matches!(v, Value::Object(_) | Value::Array(_)) {
                        *v = Value::String(MASK.to_string());
                    } else {
                        self.redact_json_value_inner(v, masked);
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    if inherit_mask && !matches!(item, Value::Object(_) | Value::Array(_)) {
                        *item = Value::String(MASK.to_string());
                    } else {
                        self.redact_json_value_inner(item, inherit_mask);
                    }
                }
            }
            _ => {}
        }
    }

    fn truncate(&self, s: String) -> String {
        if s.len() <= self.max_body_bytes {
            return s;
        }
        let mut cut = self.max_body_bytes;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!(
            "{}... [truncated, {} total bytes]",
            &s[..cut],
            s.len()
        )
    }

    /// Mask an assertion display value when the assertion's path is sensitive.
    /// Returns `***` or the original string.
    pub fn mask_if_sensitive_path(&self, path: &str, value: &str) -> String {
        if self.is_sensitive_path(path) {
            MASK.to_string()
        } else {
            value.to_string()
        }
    }

    /// Scrub the display strings on an assertion result when the asserted path
    /// or header name is sensitive. Does not change `passed` — evaluation has
    /// already run on the real value.
    pub fn scrub_assertion(&self, r: &mut AssertionResult) {
        if !self.enabled {
            return;
        }
        let desc = r.description.as_str();
        let matched = if let Some(path) = desc.strip_prefix("body.") {
            self.is_sensitive_path(path)
        } else if let Some(name) = desc.strip_prefix("header.") {
            self.is_sensitive_key(name)
        } else {
            false
        };
        if matched {
            r.actual = MASK.to_string();
            r.expected = MASK.to_string();
        }
    }
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new(true, true, 65_536, Vec::new(), Vec::new())
    }
}

fn looks_form_encoded(s: &str) -> bool {
    let t = s.trim_start();
    !t.starts_with('{') && !t.starts_with('[') && t.contains('=')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r() -> Redactor {
        Redactor::default()
    }

    #[test]
    fn sensitive_key_substring_match() {
        let r = r();
        assert!(r.is_sensitive_key("token"));
        assert!(r.is_sensitive_key("access_token"));
        assert!(r.is_sensitive_key("REFRESH_TOKEN"));
        assert!(r.is_sensitive_key("client_secret"));
        assert!(r.is_sensitive_key("Authorization"));
        assert!(r.is_sensitive_key("X-Api-Key"));
        assert!(!r.is_sensitive_key("name"));
        assert!(!r.is_sensitive_key("user_id"));
        assert!(!r.is_sensitive_key("email"));
    }

    #[test]
    fn unmask_overrides_default() {
        let r = Redactor::new(true, true, 1 << 20, vec![], vec!["session_id".into()]);
        // "session_id" contains "session" by substring, but unmask is exact-match
        // on the whole key — we unmask only fields literally named "session_id".
        assert!(!r.is_sensitive_key("session_id"));
        assert!(r.is_sensitive_key("session_token"));
    }

    #[test]
    fn extra_mask_adds_keys() {
        let r = Redactor::new(true, true, 1 << 20, vec!["ssn".into()], vec![]);
        assert!(r.is_sensitive_key("user_ssn"));
        assert!(r.is_sensitive_key("SSN"));
    }

    #[test]
    fn disabled_masks_nothing() {
        let r = Redactor::new(false, true, usize::MAX, vec![], vec![]);
        assert!(!r.is_sensitive_key("password"));
        assert_eq!(r.redact_url("https://a:b@h/p?token=x"), "https://a:b@h/p?token=x");
    }

    #[test]
    fn sensitive_path_walks_dots() {
        let r = r();
        assert!(r.is_sensitive_path("token"));
        assert!(r.is_sensitive_path("body.user.api_key"));
        assert!(r.is_sensitive_path("auth.bearer.value"));
        assert!(!r.is_sensitive_path("body.user.name"));
    }

    #[test]
    fn url_strips_userinfo() {
        let r = r();
        assert_eq!(
            r.redact_url("https://admin:hunter2@api.example.com/v1/users"),
            "https://***@api.example.com/v1/users"
        );
    }

    #[test]
    fn url_masks_query_params() {
        let r = r();
        assert_eq!(
            r.redact_url("https://api.example.com/u?api_key=sk_live_abc&page=2"),
            "https://api.example.com/u?api_key=***&page=2"
        );
    }

    #[test]
    fn url_preserves_fragment() {
        let r = r();
        assert_eq!(
            r.redact_url("https://h/p?token=x&name=y#section"),
            "https://h/p?token=***&name=y#section"
        );
    }

    #[test]
    fn url_preserves_fragment_with_no_path_or_query() {
        // Regression: authority detection used to stop only at `/` or `?`, so
        // `https://h#frag` swallowed the fragment into the authority and
        // dropped it.
        let r = r();
        assert_eq!(r.redact_url("https://h#frag"), "https://h#frag");
        assert_eq!(
            r.redact_url("https://admin:pw@h#frag"),
            "https://***@h#frag"
        );
    }

    #[test]
    fn url_with_no_query_or_userinfo() {
        let r = r();
        assert_eq!(r.redact_url("https://h/p/1"), "https://h/p/1");
    }

    #[test]
    fn json_body_masks_nested_values() {
        let r = r();
        let body = json!({
            "user": { "email": "a@b.c", "password": "hunter2" },
            "token": "sk_live_abc",
            "profile": { "name": "Jane" }
        })
        .to_string();

        let out = r.redact_body(&body).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["user"]["password"], json!("***"));
        assert_eq!(parsed["user"]["email"], json!("a@b.c"));
        assert_eq!(parsed["token"], json!("***"));
        assert_eq!(parsed["profile"]["name"], json!("Jane"));
    }

    #[test]
    fn json_array_under_masked_key_masks_all_elements() {
        let r = r();
        let body = json!({ "tokens": ["a", "b", "c"], "names": ["x", "y"] }).to_string();
        let out = r.redact_body(&body).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["tokens"], json!(["***", "***", "***"]));
        assert_eq!(parsed["names"], json!(["x", "y"]));
    }

    #[test]
    fn form_body_masks_sensitive_keys() {
        let r = r();
        let body = "grant_type=client_credentials&client_id=abc&client_secret=super&scope=read";
        let out = r.redact_body(body).unwrap();
        assert_eq!(
            out,
            "grant_type=client_credentials&client_id=abc&client_secret=***&scope=read"
        );
    }

    #[test]
    fn non_json_non_form_body_unchanged() {
        let r = r();
        let body = "just some plain text response";
        let out = r.redact_body(body).unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn include_bodies_false_returns_none() {
        let r = Redactor::new(true, false, 1 << 20, vec![], vec![]);
        assert!(r.redact_body(r#"{"a":1}"#).is_none());
    }

    #[test]
    fn body_truncation_adds_marker() {
        let r = Redactor::new(true, true, 16, vec![], vec![]);
        let body = "x".repeat(100);
        let out = r.redact_body(&body).unwrap();
        assert!(out.starts_with(&"x".repeat(16)));
        assert!(out.contains("truncated"));
        assert!(out.contains("100"));
    }

    #[test]
    fn mask_if_sensitive_path() {
        let r = r();
        assert_eq!(r.mask_if_sensitive_path("token", "abc"), "***");
        assert_eq!(r.mask_if_sensitive_path("user.api_key", "xyz"), "***");
        assert_eq!(r.mask_if_sensitive_path("user.name", "Jane"), "Jane");
    }

    #[test]
    fn scrub_assertion_masks_sensitive_body_paths() {
        let r = r();
        let mut a = AssertionResult {
            description: "body.user.api_key".into(),
            passed: true,
            expected: "exists: true".into(),
            actual: "sk_live_abc123".into(),
        };
        r.scrub_assertion(&mut a);
        assert_eq!(a.actual, "***");
        assert_eq!(a.expected, "***");
        assert!(a.passed); // evaluation unchanged
    }

    #[test]
    fn scrub_assertion_masks_sensitive_headers() {
        let r = r();
        let mut a = AssertionResult {
            description: "header.Authorization".into(),
            passed: true,
            expected: "contains \"Bearer\"".into(),
            actual: "Bearer eyJhbGc...".into(),
        };
        r.scrub_assertion(&mut a);
        assert_eq!(a.actual, "***");
    }

    #[test]
    fn scrub_assertion_leaves_nonsensitive_alone() {
        let r = r();
        let mut a = AssertionResult {
            description: "body.user.email".into(),
            passed: true,
            expected: "exists: true".into(),
            actual: "jane@example.com".into(),
        };
        r.scrub_assertion(&mut a);
        assert_eq!(a.actual, "jane@example.com");

        let mut status = AssertionResult {
            description: "status == 200".into(),
            passed: true,
            expected: "200".into(),
            actual: "200".into(),
        };
        r.scrub_assertion(&mut status);
        assert_eq!(status.actual, "200");
    }

    #[test]
    fn disabled_body_preserves_secrets() {
        // --redact=off escape hatch: the secret value survives into the log.
        let r = Redactor::new(false, true, usize::MAX, vec![], vec![]);
        let body = r#"{"token":"keep-me"}"#;
        let out = r.redact_body(body).unwrap();
        assert_eq!(out, body);
    }
}
