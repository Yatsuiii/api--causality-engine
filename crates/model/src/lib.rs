use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub initial_state: String,
    pub steps: Vec<Step>,
    pub concurrency: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    pub name: String,
    pub method: String,
    pub url: String,
    pub transition: Transition,
    pub extract: Option<HashMap<String, String>>,
    pub retry: Option<RetryConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    pub attempts: u32,
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Transition {
    pub from: String,
    pub to: String,
}

pub fn load_scenario(yaml: &str) -> Result<Scenario, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_scenario() {
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
        assert_eq!(scenario.steps[0].transition.from, "start");
        assert_eq!(scenario.steps[0].transition.to, "done");
    }
}
