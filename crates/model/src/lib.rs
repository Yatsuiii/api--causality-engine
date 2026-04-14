use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Scenario — top-level YAML document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub initial_state: String,
    pub steps: Vec<Step>,
    #[serde(default)]
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
    /// Maximum step executions before aborting (loop protection). Default: 100.
    #[serde(default)]
    pub max_iterations: Option<u64>,
    /// Explicitly declared terminal states. If omitted, inferred from the graph.
    #[serde(default)]
    pub terminal_states: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Auth — scenario-level authentication
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Step — a single API call in the workflow
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub method: Method,
    pub url: String,
    /// Linear mode (backward compat): single from/to transition.
    #[serde(default)]
    pub transition: Option<Transition>,
    /// Graph mode: multiple conditional transition edges.
    #[serde(default)]
    pub transitions: Option<Vec<TransitionEdge>>,
    /// Explicit state name for graph mode. Defaults to step `name`.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub body: Option<serde_yaml::Value>,
    #[serde(default)]
    pub multipart: Option<Vec<MultipartFieldDef>>,
    #[serde(default)]
    pub extract: Option<HashMap<String, String>>,
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
    /// Organisational tags (e.g. Postman folder names preserved on import).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

impl Step {
    /// The state this step handles. In graph mode defaults to `name`.
    /// In linear mode returns `transition.from`.
    pub fn state_name(&self) -> &str {
        if let Some(s) = &self.state {
            return s;
        }
        if let Some(t) = &self.transition {
            return &t.from;
        }
        &self.name
    }

    /// Normalize into a consistent edge list. Call after deserialization.
    pub fn resolved_edges(&self) -> Result<(String, Vec<TransitionEdge>), String> {
        match (&self.transition, &self.transitions) {
            (Some(t), None) => Ok((
                t.from.clone(),
                vec![TransitionEdge {
                    to: t.to.clone(),
                    when: None,
                    default: Some(true),
                }],
            )),
            (None, Some(edges)) => Ok((self.state_name().to_string(), edges.clone())),
            (Some(_), Some(_)) => Err(format!(
                "Step '{}': cannot have both 'transition' and 'transitions'",
                self.name
            )),
            (None, None) => Err(format!(
                "Step '{}': must have either 'transition' or 'transitions'",
                self.name
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Multipart field definition
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Pre/post request hooks
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// HTTP method enum (validated at parse time)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Retry configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryConfig {
    #[serde(default = "RetryConfig::default_attempts")]
    pub attempts: u32,
    #[serde(default = "RetryConfig::default_delay_ms")]
    pub delay_ms: u64,
}

impl RetryConfig {
    fn default_attempts() -> u32 {
        3
    }
    fn default_delay_ms() -> u64 {
        1000
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            attempts: 3,
            delay_ms: 1000,
        }
    }
}

// ---------------------------------------------------------------------------
// Transition (state machine edge) — linear mode (backward compat)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub from: String,
    pub to: String,
}

// ---------------------------------------------------------------------------
// Graph-mode transitions — conditional edges
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionEdge {
    pub to: String,
    #[serde(default)]
    pub when: Option<TransitionCondition>,
    #[serde(default)]
    pub default: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionCondition {
    #[serde(default)]
    pub status: Option<StatusMatch>,
    #[serde(default)]
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

// ---------------------------------------------------------------------------
// Assertions — flexible response validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    #[serde(default)]
    pub status: Option<StatusCheck>,
    #[serde(default)]
    pub body: Option<HashMap<String, ValueCheck>>,
    /// Assert the JSON type of the entire response body: "array", "object", "string", etc.
    #[serde(default)]
    pub body_type: Option<String>,
    #[serde(default)]
    pub header: Option<HashMap<String, ValueCheck>>,
    #[serde(default)]
    pub response_time_ms: Option<ValueCheck>,
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
    /// JSON type name: "array", "object", "string", "number", "boolean", "null".
    #[serde(default, rename = "type")]
    pub type_of: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

pub fn load_scenario(yaml: &str) -> Result<Scenario, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_scenario() {
        let yaml = r#"
name: test
initial_state: start
steps:
  - name: get users
    method: GET
    url: https://example.com/users
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        assert_eq!(scenario.name, "test");
        assert_eq!(scenario.steps.len(), 1);
        assert_eq!(scenario.steps[0].method, Method::Get);
    }

    #[test]
    fn parse_full_scenario() {
        let yaml = r#"
name: full test
initial_state: start
auth:
  bearer: "my-token"
variables:
  base_url: https://api.example.com
concurrency: 3
proxy: http://localhost:8080
insecure: true
steps:
  - name: create user
    method: POST
    url: "{{base_url}}/users"
    headers:
      Content-Type: application/json
    body:
      name: "Alice"
    timeout_ms: 5000
    assert:
      - status: 201
      - body:
          id:
            exists: true
      - header:
          content-type:
            contains: "json"
      - response_time_ms:
          lt: 2000
    extract:
      user_id: "id"
    retry:
      attempts: 2
      delay_ms: 500
    transition:
      from: start
      to: created
"#;
        let scenario = load_scenario(yaml).unwrap();
        assert_eq!(scenario.name, "full test");
        assert_eq!(scenario.concurrency, Some(3));
        assert_eq!(scenario.proxy.as_deref(), Some("http://localhost:8080"));
        assert_eq!(scenario.insecure, Some(true));

        let auth = scenario.auth.as_ref().unwrap();
        assert_eq!(auth.bearer.as_deref(), Some("my-token"));

        let step = &scenario.steps[0];
        assert_eq!(step.method, Method::Post);
        assert!(step.headers.is_some());
        assert!(step.body.is_some());
        assert_eq!(step.timeout_ms, Some(5000));
        assert_eq!(step.assertions.as_ref().unwrap().len(), 4);
    }

    #[test]
    fn parse_all_methods() {
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
            let yaml = format!(
                r#"
name: test
initial_state: start
steps:
  - name: step
    method: {method}
    url: http://example.com
    transition:
      from: start
      to: done
"#
            );
            let scenario = load_scenario(&yaml).unwrap();
            assert_eq!(scenario.steps[0].method.as_str(), method);
        }
    }

    #[test]
    fn parse_basic_auth() {
        let yaml = r#"
name: test
initial_state: start
auth:
  basic:
    username: admin
    password: secret
steps:
  - name: step
    method: GET
    url: http://example.com
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let basic = scenario.auth.as_ref().unwrap().basic.as_ref().unwrap();
        assert_eq!(basic.username, "admin");
        assert_eq!(basic.password, "secret");
    }

    #[test]
    fn parse_api_key_auth() {
        let yaml = r#"
name: test
initial_state: start
auth:
  api_key:
    header: X-API-Key
    value: my-key-123
steps:
  - name: step
    method: GET
    url: http://example.com
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let api_key = scenario.auth.as_ref().unwrap().api_key.as_ref().unwrap();
        assert_eq!(api_key.header, "X-API-Key");
        assert_eq!(api_key.value, "my-key-123");
    }

    #[test]
    fn parse_oauth2() {
        let yaml = r#"
name: test
initial_state: start
auth:
  oauth2:
    token_url: https://auth.example.com/token
    client_id: my-client
    client_secret: my-secret
    scope: "read write"
steps:
  - name: step
    method: GET
    url: http://example.com
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let oauth = scenario.auth.as_ref().unwrap().oauth2.as_ref().unwrap();
        assert_eq!(oauth.token_url, "https://auth.example.com/token");
        assert_eq!(oauth.client_id, "my-client");
        assert_eq!(oauth.scope.as_deref(), Some("read write"));
    }

    #[test]
    fn parse_multipart() {
        let yaml = r#"
name: test
initial_state: start
steps:
  - name: upload
    method: POST
    url: http://example.com/upload
    multipart:
      - name: file
        file: ./test.png
        filename: avatar.png
        mime: image/png
      - name: description
        value: "My avatar"
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let mp = scenario.steps[0].multipart.as_ref().unwrap();
        assert_eq!(mp.len(), 2);
        assert_eq!(mp[0].name, "file");
        assert!(mp[0].file.is_some());
        assert_eq!(mp[1].name, "description");
        assert_eq!(mp[1].value.as_deref(), Some("My avatar"));
    }

    #[test]
    fn parse_hooks() {
        let yaml = r#"
name: test
initial_state: start
steps:
  - name: step
    method: GET
    url: http://example.com
    pre_request:
      - set:
          timestamp: "{{$timestamp}}"
      - delay_ms: 100
      - log: "Starting request"
    post_request:
      - log: "Request complete"
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let pre = scenario.steps[0].pre_request.as_ref().unwrap();
        assert_eq!(pre.len(), 3);
        assert!(pre[0].set.is_some());
        assert_eq!(pre[1].delay_ms, Some(100));
        assert_eq!(pre[2].log.as_deref(), Some("Starting request"));
    }

    // -----------------------------------------------------------------------
    // Graph-mode transition tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_graph_transitions() {
        let yaml = r#"
name: branching
initial_state: login
steps:
  - name: login
    state: login
    method: POST
    url: http://example.com/auth
    transitions:
      - to: dashboard
        when:
          status: 200
      - to: retry
        when:
          status: 429
      - to: failed
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let step = &scenario.steps[0];
        assert!(step.transition.is_none());
        let edges = step.transitions.as_ref().unwrap();
        assert_eq!(edges.len(), 3);
        assert_eq!(edges[0].to, "dashboard");
        assert!(edges[0].when.is_some());
        assert_eq!(edges[2].default, Some(true));
    }

    #[test]
    fn resolved_edges_linear() {
        let yaml = r#"
name: test
initial_state: start
steps:
  - name: step1
    method: GET
    url: http://example.com
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let (from, edges) = scenario.steps[0].resolved_edges().unwrap();
        assert_eq!(from, "start");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, "done");
        assert_eq!(edges[0].default, Some(true));
    }

    #[test]
    fn resolved_edges_graph() {
        let yaml = r#"
name: test
initial_state: check
steps:
  - name: check
    method: GET
    url: http://example.com
    transitions:
      - to: pass
        when:
          status: 200
      - to: fail
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let (from, edges) = scenario.steps[0].resolved_edges().unwrap();
        assert_eq!(from, "check");
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn resolved_edges_rejects_both() {
        let step = Step {
            name: "bad".into(),
            method: Method::Get,
            url: "http://example.com".into(),
            transition: Some(Transition {
                from: "a".into(),
                to: "b".into(),
            }),
            transitions: Some(vec![TransitionEdge {
                to: "c".into(),
                when: None,
                default: Some(true),
            }]),
            state: None,
            headers: None,
            body: None,
            multipart: None,
            extract: None,
            retry: None,
            assertions: None,
            timeout_ms: None,
            pre_request: None,
            post_request: None,
            tags: None,
        };
        assert!(step.resolved_edges().is_err());
    }

    #[test]
    fn resolved_edges_rejects_neither() {
        let step = Step {
            name: "bad".into(),
            method: Method::Get,
            url: "http://example.com".into(),
            transition: None,
            transitions: None,
            state: None,
            headers: None,
            body: None,
            multipart: None,
            extract: None,
            retry: None,
            assertions: None,
            timeout_ms: None,
            pre_request: None,
            post_request: None,
            tags: None,
        };
        assert!(step.resolved_edges().is_err());
    }

    #[test]
    fn state_name_defaults() {
        let step = Step {
            name: "my_step".into(),
            method: Method::Get,
            url: "http://example.com".into(),
            transition: None,
            transitions: Some(vec![]),
            state: None,
            headers: None,
            body: None,
            multipart: None,
            extract: None,
            retry: None,
            assertions: None,
            timeout_ms: None,
            pre_request: None,
            post_request: None,
            tags: None,
        };
        assert_eq!(step.state_name(), "my_step");
    }

    #[test]
    fn state_name_explicit() {
        let step = Step {
            name: "my_step".into(),
            method: Method::Get,
            url: "http://example.com".into(),
            transition: None,
            transitions: Some(vec![]),
            state: Some("custom_state".into()),
            headers: None,
            body: None,
            multipart: None,
            extract: None,
            retry: None,
            assertions: None,
            timeout_ms: None,
            pre_request: None,
            post_request: None,
            tags: None,
        };
        assert_eq!(step.state_name(), "custom_state");
    }

    #[test]
    fn parse_max_iterations() {
        let yaml = r#"
name: loop test
initial_state: start
max_iterations: 50
steps:
  - name: poll
    method: GET
    url: http://example.com
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        assert_eq!(scenario.max_iterations, Some(50));
    }

    #[test]
    fn parse_transition_condition_body() {
        let yaml = r#"
name: test
initial_state: check
steps:
  - name: check
    method: GET
    url: http://example.com
    transitions:
      - to: ready
        when:
          body:
            status:
              eq: "complete"
      - to: wait
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let edges = scenario.steps[0].transitions.as_ref().unwrap();
        let condition = edges[0].when.as_ref().unwrap();
        let body_check = condition.body.as_ref().unwrap();
        assert!(body_check.contains_key("status"));
    }

    #[test]
    fn parse_assertion_match_condition() {
        let yaml = r#"
name: test
initial_state: verify
steps:
  - name: verify
    method: GET
    url: http://example.com
    assert:
      - status: 200
    transitions:
      - to: success
        when:
          assertions: passed
      - to: handle_error
        when:
          assertions: failed
"#;
        let scenario = load_scenario(yaml).unwrap();
        let edges = scenario.steps[0].transitions.as_ref().unwrap();
        let cond0 = edges[0].when.as_ref().unwrap();
        assert_eq!(cond0.assertions, Some(AssertionMatch::Passed));
        let cond1 = edges[1].when.as_ref().unwrap();
        assert_eq!(cond1.assertions, Some(AssertionMatch::Failed));
    }
}
