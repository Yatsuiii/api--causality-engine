use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

const VALUE_CHECK_OPERATORS: &[&str] =
    &["eq", "ne", "contains", "exists", "lt", "gt", "in", "type"];

fn is_value_check_map(map: &serde_yaml::Mapping) -> bool {
    map.keys().any(|k| {
        k.as_str()
            .map(|s| VALUE_CHECK_OPERATORS.contains(&s))
            .unwrap_or(false)
    })
}

fn flatten_body_map(
    prefix: &str,
    val: &serde_yaml::Value,
    out: &mut HashMap<String, ValueCheck>,
) -> Result<(), String> {
    match val {
        serde_yaml::Value::Mapping(map) if is_value_check_map(map) || prefix.is_empty() => {
            if is_value_check_map(map) {
                let vc: ValueCheck =
                    serde_yaml::from_value(val.clone()).map_err(|e| e.to_string())?;
                if !prefix.is_empty() {
                    out.insert(prefix.to_string(), vc);
                    return Ok(());
                }
            }
            for (k, v) in map {
                let key = k
                    .as_str()
                    .ok_or_else(|| "body key is not a string".to_string())?;
                let child_prefix = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{}.{}", prefix, key)
                };
                flatten_body_map(&child_prefix, v, out)?;
            }
        }
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let key = k
                    .as_str()
                    .ok_or_else(|| "body key is not a string".to_string())?;
                let child_prefix = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{}.{}", prefix, key)
                };
                flatten_body_map(&child_prefix, v, out)?;
            }
        }
        _ => {
            return Err(format!(
                "body assertion at '{}' must be a mapping (e.g. {{ exists: true }})",
                prefix
            ));
        }
    }
    Ok(())
}

fn deserialize_body_checks<'de, D>(
    deserializer: D,
) -> Result<Option<HashMap<String, ValueCheck>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<serde_yaml::Value> = Option::deserialize(deserializer)?;
    match raw {
        None => Ok(None),
        Some(val) => {
            let mut map = HashMap::new();
            flatten_body_map("", &val, &mut map).map_err(serde::de::Error::custom)?;
            Ok(Some(map))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub initial_state: String,
    pub steps: Vec<Step>,
    pub edges: Vec<Edge>,
    /// Deprecated: use the CLI `--concurrency / -c` flag instead. This field
    /// will be removed in a future release.
    #[serde(default)]
    #[deprecated(
        since = "0.1.6",
        note = "use the CLI `--concurrency / -c` flag; scenario-level concurrency will be removed"
    )]
    pub concurrency: Option<usize>,
    #[serde(default)]
    pub auth: Option<Auth>,
    #[serde(default)]
    pub variables: Option<HashMap<String, String>>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub insecure: Option<bool>,
    #[serde(default)]
    pub default_timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_iterations: Option<u64>,
    #[serde(default)]
    pub terminal_states: Option<Vec<String>>,
    /// Execution-log redaction and sizing policy. `None` means built-in defaults
    /// apply (mask secrets, include bodies, 64KB cap). Override per scenario
    /// when the built-in key list mislabels a field.
    #[serde(default)]
    pub log: Option<LogConfig>,
}

/// Per-scenario overrides for what lands in `execution_log.json`.
///
/// The built-in redactor already masks values under a list of sensitive keys
/// (`token`, `password`, `api_key`, ...). Use `mask` to extend that list for
/// domain-specific secrets and `unmask` to allowlist a field whose name
/// accidentally collides with a sensitive substring (e.g. `session_id` when
/// you genuinely want it in logs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    /// Extra key substrings to redact (case-insensitive, same matching rules
    /// as the built-in list).
    #[serde(default)]
    pub mask: Vec<String>,
    /// Exact key names to exempt from redaction even if they would otherwise
    /// match the built-in or `mask` lists.
    #[serde(default)]
    pub unmask: Vec<String>,
    /// When `false`, `request_body` and `response_body` are omitted from the
    /// log entirely. Default `true`.
    #[serde(default = "LogConfig::default_include_bodies")]
    pub include_bodies: bool,
    /// Truncate logged bodies larger than this (bytes). Default 65536.
    #[serde(default = "LogConfig::default_max_body_bytes")]
    pub max_body_bytes: usize,
}

impl LogConfig {
    fn default_include_bodies() -> bool {
        true
    }
    fn default_max_body_bytes() -> usize {
        65_536
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            mask: Vec::new(),
            unmask: Vec::new(),
            include_bodies: Self::default_include_bodies(),
            max_body_bytes: Self::default_max_body_bytes(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    #[serde(default)]
    pub bearer: Option<String>,
    #[serde(default)]
    pub basic: Option<BasicAuth>,
    #[serde(default)]
    pub api_key: Option<ApiKeyAuth>,
    #[serde(default)]
    pub oauth2: Option<OAuth2Config>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyAuth {
    pub header: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuth2Config {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default, rename = "grant_type")]
    pub grant_type: Option<String>,
}

/// How a single `extract:` entry is specified. Accepts either a bare JSONPath
/// string (permissive) or a struct with `path` + flags (fine-grained control).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtractSpec {
    Path(String),
    Detailed {
        path: String,
        #[serde(default)]
        required: bool,
    },
}

impl ExtractSpec {
    pub fn path(&self) -> &str {
        match self {
            ExtractSpec::Path(p) => p,
            ExtractSpec::Detailed { path, .. } => path,
        }
    }

    /// Whether a missing JSONPath on this extract should fail the step.
    /// Bare-string specs inherit from the global `--strict-extract` flag;
    /// struct-form specs use their own `required` field.
    pub fn is_required(&self, global_strict: bool) -> bool {
        match self {
            ExtractSpec::Path(_) => global_strict,
            ExtractSpec::Detailed { required, .. } => *required,
        }
    }
}

impl From<&str> for ExtractSpec {
    fn from(s: &str) -> Self {
        ExtractSpec::Path(s.to_string())
    }
}

impl From<String> for ExtractSpec {
    fn from(s: String) -> Self {
        ExtractSpec::Path(s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub state: String,
    pub method: Method,
    pub url: String,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub body: Option<serde_yaml::Value>,
    #[serde(default)]
    pub multipart: Option<Vec<MultipartFieldDef>>,
    #[serde(default)]
    pub extract: Option<HashMap<String, ExtractSpec>>,
    #[serde(default)]
    pub retry: Option<RetryConfig>,
    #[serde(default, rename = "assert")]
    pub assertions: Option<Vec<Assertion>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub pre_request: Option<Vec<Hook>>,
    #[serde(default)]
    pub post_request: Option<Vec<Hook>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

impl Step {
    pub fn state_name(&self) -> &str {
        &self.state
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultipartFieldDef {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    #[serde(default)]
    pub set: Option<HashMap<String, String>>,
    #[serde(default)]
    pub log: Option<String>,
    #[serde(default)]
    pub delay_ms: Option<u64>,
    #[serde(default)]
    pub skip_if: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryConfig {
    #[serde(default = "RetryConfig::default_attempts")]
    pub attempts: u32,
    /// Initial delay between attempts (ms). For exponential backoff this is the
    /// base; for fixed backoff it's the delay used between every attempt.
    #[serde(default = "RetryConfig::default_delay_ms")]
    pub delay_ms: u64,
    /// `fixed` (default) holds `delay_ms` constant. `exponential` doubles the
    /// delay each attempt (scaled by `multiplier`), capped at `max_delay_ms`.
    #[serde(default)]
    pub backoff: BackoffPolicy,
    /// Growth factor for exponential backoff. Default 2.0. Ignored for fixed.
    #[serde(default = "RetryConfig::default_multiplier")]
    pub multiplier: f64,
    /// Upper bound for any single retry delay (ms). Default 30_000.
    #[serde(default = "RetryConfig::default_max_delay_ms")]
    pub max_delay_ms: u64,
    /// Jitter applied to each computed delay. `none` (default) is deterministic;
    /// `full` randomizes uniformly in `[0, delay]`; `equal` splits as
    /// `delay/2 + random(0, delay/2)`.
    #[serde(default)]
    pub jitter: JitterMode,
    /// Status codes that trigger a retry. Empty (default) means use the built-in
    /// set: `[408, 429, 500, 501, 502, 503, 504]`. Any HTTP status not in this
    /// list is returned to the caller — including 401/404, which older ACE
    /// versions retried. Transport errors (timeouts, connection refused) always
    /// retry regardless of this list.
    #[serde(default)]
    pub retry_on: Vec<u16>,
}

impl RetryConfig {
    fn default_attempts() -> u32 {
        3
    }
    fn default_delay_ms() -> u64 {
        1000
    }
    fn default_multiplier() -> f64 {
        2.0
    }
    fn default_max_delay_ms() -> u64 {
        30_000
    }

    /// Built-in retry set applied when `retry_on` is empty. Matches the
    /// AWS/Google/Polly industry default: timeout-adjacent + server errors.
    pub const DEFAULT_RETRY_STATUSES: &'static [u16] = &[408, 429, 500, 501, 502, 503, 504];

    /// Should the given HTTP status trigger a retry under this config?
    pub fn should_retry_status(&self, status: u16) -> bool {
        let list: &[u16] = if self.retry_on.is_empty() {
            Self::DEFAULT_RETRY_STATUSES
        } else {
            &self.retry_on
        };
        list.contains(&status)
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            attempts: Self::default_attempts(),
            delay_ms: Self::default_delay_ms(),
            backoff: BackoffPolicy::default(),
            multiplier: Self::default_multiplier(),
            max_delay_ms: Self::default_max_delay_ms(),
            jitter: JitterMode::default(),
            retry_on: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackoffPolicy {
    #[default]
    Fixed,
    Exponential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JitterMode {
    #[default]
    None,
    Full,
    Equal,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    /// Target state for single-target edges. Omitted on fan-out edges; the
    /// validator requires non-empty `to` whenever `parallel` is unset.
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub when: Option<TransitionCondition>,
    #[serde(default)]
    pub default: Option<bool>,
    /// Higher values evaluated first among matching conditional edges. Ties
    /// broken by list order. Unspecified edges default to 0.
    #[serde(default)]
    pub priority: Option<i32>,
    /// Optional human-readable label surfaced in step logs when this edge is
    /// traversed. Useful for distinguishing branches in traces.
    #[serde(default)]
    pub tag: Option<String>,
    /// Delay in milliseconds before the transition fires. The blessed pattern
    /// for polling/backoff on self-loops.
    #[serde(default)]
    pub after_ms: Option<u64>,
    /// Maximum times this edge may be traversed per scenario run. Exceeding
    /// the cap returns `RunError::EdgeMaxTakesExceeded` before the transition.
    #[serde(default)]
    pub max_takes: Option<u32>,
    /// Relative weight for probabilistic routing among sibling edges in the
    /// same routing group. When set on all siblings in a group, the executor
    /// samples via a seeded RNG (reproducible via `RunConfig.seed`). Mixing
    /// weighted and unweighted siblings is a validator error (E010).
    #[serde(default)]
    pub weight: Option<u32>,
    /// Fan-out definition. When present, the edge spawns one concurrent branch
    /// per entry in `parallel.branches`, each running its own sub-FSM, and
    /// rejoins at `parallel.join`. Mutually exclusive with `to`, `when`,
    /// `default`, `weight` (validator E018).
    #[serde(default)]
    pub parallel: Option<FanOut>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FanOut {
    pub branches: Vec<Branch>,
    pub join: String,
    #[serde(default)]
    pub on_failure: Option<FailurePolicy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub name: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    #[default]
    FailFast,
    AllComplete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionCondition {
    #[serde(default)]
    pub status: Option<StatusMatch>,
    #[serde(default, deserialize_with = "deserialize_body_checks")]
    pub body: Option<HashMap<String, ValueCheck>>,
    #[serde(default)]
    pub assertions: Option<AssertionMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatusMatch {
    Exact(u16),
    Complex(ValueCheck),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionMatch {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    #[serde(default)]
    pub status: Option<StatusCheck>,
    #[serde(default, deserialize_with = "deserialize_body_checks")]
    pub body: Option<HashMap<String, ValueCheck>>,
    #[serde(default)]
    pub body_type: Option<String>,
    #[serde(default)]
    pub header: Option<HashMap<String, ValueCheck>>,
    #[serde(default)]
    pub response_time_ms: Option<ValueCheck>,
    /// JSONSchema validation for the response body. Accepts either an inline
    /// schema object or a string path (resolved relative to the scenario file).
    #[serde(default)]
    pub schema: Option<SchemaRef>,
}

/// Reference to a JSONSchema: inline object or file path.
/// A bare string is treated as a path; an object is treated as the schema itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SchemaRef {
    File(String),
    Inline(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatusCheck {
    Exact(u16),
    Complex(ValueCheck),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueCheck {
    #[serde(default)]
    pub eq: Option<serde_json::Value>,
    #[serde(default)]
    pub ne: Option<serde_json::Value>,
    #[serde(default)]
    pub contains: Option<String>,
    #[serde(default)]
    pub exists: Option<bool>,
    #[serde(default)]
    pub lt: Option<f64>,
    #[serde(default)]
    pub gt: Option<f64>,
    #[serde(default, rename = "in")]
    pub in_list: Option<Vec<serde_json::Value>>,
    #[serde(default, rename = "type")]
    pub type_of: Option<String>,
}

pub fn load_scenario(yaml: &str) -> Result<Scenario, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_graph_scenario() {
        let yaml = r#"
name: login flow
initial_state: login
steps:
  - name: login
    state: login
    method: POST
    url: https://example.com/login
  - name: dashboard
    state: dashboard
    method: GET
    url: https://example.com/me
edges:
  - from: login
    to: dashboard
    when:
      status: 200
  - from: login
    to: failed
    default: true
  - from: dashboard
    to: done
    default: true
"#;

        let scenario = load_scenario(yaml).unwrap();
        assert_eq!(scenario.steps.len(), 2);
        assert_eq!(scenario.edges.len(), 3);
        assert_eq!(scenario.steps[0].state_name(), "login");
        assert_eq!(scenario.edges[0].from, "login");
    }

    #[test]
    fn parse_nested_body_checks() {
        let yaml = r#"
name: nested
initial_state: fetch
steps:
  - name: fetch
    state: fetch
    method: GET
    url: https://example.com
    assert:
      - body:
          data:
            user:
              id:
                exists: true
edges:
  - from: fetch
    to: done
    default: true
"#;

        let scenario = load_scenario(yaml).unwrap();
        let checks = scenario.steps[0].assertions.as_ref().unwrap()[0]
            .body
            .as_ref()
            .unwrap();
        assert!(checks.contains_key("data.user.id"));
    }

    #[test]
    fn load_scenario_requires_explicit_state() {
        let yaml = r#"
name: invalid
initial_state: start
steps:
  - name: step1
    method: GET
    url: https://example.com
edges: []
"#;

        assert!(load_scenario(yaml).is_err());
    }

    #[test]
    fn load_scenario_requires_explicit_edges() {
        let yaml = r#"
name: invalid
initial_state: start
steps:
  - name: step1
    state: start
    method: GET
    url: https://example.com
"#;

        assert!(load_scenario(yaml).is_err());
    }

    #[test]
    fn parse_parallel_edge() {
        let yaml = r#"
name: fanout
initial_state: start
terminal_states: [done]
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
  - name: a
    state: a
    method: GET
    url: http://example.com/a
  - name: b
    state: b
    method: GET
    url: http://example.com/b
  - name: join
    state: aggregate
    method: GET
    url: http://example.com/join
edges:
  - from: start
    parallel:
      branches:
        - name: left
          to: a
        - name: right
          to: b
      join: aggregate
      on_failure: all_complete
  - from: a
    to: aggregate
    default: true
  - from: b
    to: aggregate
    default: true
  - from: aggregate
    to: done
    default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let fan_out = scenario.edges[0].parallel.as_ref().unwrap();
        assert_eq!(fan_out.branches.len(), 2);
        assert_eq!(fan_out.branches[0].name, "left");
        assert_eq!(fan_out.branches[1].to, "b");
        assert_eq!(fan_out.join, "aggregate");
        assert_eq!(fan_out.on_failure, Some(FailurePolicy::AllComplete));
        assert!(scenario.edges[0].to.is_empty());
    }

    #[test]
    fn parse_weighted_edges() {
        let yaml = r#"
name: weighted
initial_state: start
terminal_states: [a, b]
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
edges:
  - from: start
    to: a
    weight: 70
  - from: start
    to: b
    weight: 30
"#;
        let scenario = load_scenario(yaml).unwrap();
        assert_eq!(scenario.edges[0].weight, Some(70));
        assert_eq!(scenario.edges[1].weight, Some(30));
    }

    #[test]
    fn extract_spec_bare_and_detailed_forms_parse() {
        let yaml = r#"
name: extract-shapes
initial_state: fetch
steps:
  - name: fetch
    state: fetch
    method: GET
    url: https://example.com
    extract:
      user_id: "$.id"
      token:
        path: "$.token"
        required: true
edges:
  - from: fetch
    to: done
    default: true
terminal_states:
  - done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let extract = scenario.steps[0].extract.as_ref().unwrap();

        let bare = extract.get("user_id").unwrap();
        assert_eq!(bare.path(), "$.id");
        assert!(!bare.is_required(false));
        assert!(bare.is_required(true)); // inherits global strict

        let detailed = extract.get("token").unwrap();
        assert_eq!(detailed.path(), "$.token");
        assert!(detailed.is_required(false)); // self-declared
        assert!(detailed.is_required(true));
    }

    #[test]
    fn parse_schema_assertion_inline_and_file() {
        let yaml = r#"
name: schema-shapes
initial_state: fetch
steps:
  - name: fetch
    state: fetch
    method: GET
    url: https://example.com
    assert:
      - schema: ./schemas/user.json
      - schema:
          type: object
          required: [id]
          properties:
            id: { type: integer }
edges:
  - from: fetch
    to: done
    default: true
terminal_states:
  - done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let asserts = scenario.steps[0].assertions.as_ref().unwrap();
        assert_eq!(asserts.len(), 2);
        match asserts[0].schema.as_ref().unwrap() {
            SchemaRef::File(p) => assert_eq!(p, "./schemas/user.json"),
            other => panic!("expected file ref, got {:?}", other),
        }
        match asserts[1].schema.as_ref().unwrap() {
            SchemaRef::Inline(v) => assert!(v.get("properties").is_some()),
            other => panic!("expected inline schema, got {:?}", other),
        }
    }
}
