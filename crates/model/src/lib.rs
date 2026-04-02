use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Scenario — top-level YAML document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

// ---------------------------------------------------------------------------
// Auth — scenario-level authentication
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyAuth {
    pub header: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub method: Method,
    pub url: String,
    pub transition: Transition,
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
}

// ---------------------------------------------------------------------------
// Multipart field definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub attempts: u32,
    pub delay_ms: u64,
}

// ---------------------------------------------------------------------------
// Transition (state machine edge)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transition {
    pub from: String,
    pub to: String,
}

// ---------------------------------------------------------------------------
// Assertions — flexible response validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assertion {
    #[serde(default)]
    pub status: Option<StatusCheck>,
    #[serde(default)]
    pub body: Option<HashMap<String, ValueCheck>>,
    #[serde(default)]
    pub header: Option<HashMap<String, ValueCheck>>,
    #[serde(default)]
    pub response_time_ms: Option<ValueCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatusCheck {
    Exact(u16),
    Complex(ValueCheck),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
