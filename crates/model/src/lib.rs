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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
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
}
