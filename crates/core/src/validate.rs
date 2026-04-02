use model::Scenario;
use std::collections::HashSet;

/// Pre-flight validation of a scenario's state machine.
/// Returns a list of issues found (empty = valid).
pub fn validate_scenario(scenario: &Scenario) -> Vec<String> {
    let mut issues = Vec::new();

    if scenario.steps.is_empty() {
        issues.push("Scenario has no steps".into());
        return issues;
    }

    // Check first step starts from initial_state
    if scenario.steps[0].transition.from != scenario.initial_state {
        issues.push(format!(
            "First step '{}' expects state '{}', but initial_state is '{}'",
            scenario.steps[0].name, scenario.steps[0].transition.from, scenario.initial_state,
        ));
    }

    // Check sequential state continuity
    for i in 1..scenario.steps.len() {
        let prev_to = &scenario.steps[i - 1].transition.to;
        let curr_from = &scenario.steps[i].transition.from;
        if prev_to != curr_from {
            issues.push(format!(
                "State gap: step '{}' transitions to '{}', but step '{}' expects '{}'",
                scenario.steps[i - 1].name,
                prev_to,
                scenario.steps[i].name,
                curr_from,
            ));
        }
    }

    // Check for duplicate step names
    let mut seen_names = HashSet::new();
    for step in &scenario.steps {
        if !seen_names.insert(&step.name) {
            issues.push(format!("Duplicate step name: '{}'", step.name));
        }
    }

    // Validate concurrency
    if let Some(c) = scenario.concurrency
        && c == 0
    {
        issues.push("Concurrency must be >= 1".into());
    }

    // Validate retry config
    for step in &scenario.steps {
        if let Some(retry) = &step.retry
            && retry.attempts == 0
        {
            issues.push(format!("Step '{}': retry attempts must be >= 1", step.name));
        }
    }

    issues
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use model::load_scenario;

    #[test]
    fn valid_scenario() {
        let yaml = r#"
name: valid
initial_state: start
steps:
  - name: step1
    method: GET
    url: http://example.com
    transition:
      from: start
      to: middle
  - name: step2
    method: POST
    url: http://example.com
    transition:
      from: middle
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    #[test]
    fn detects_initial_state_mismatch() {
        let yaml = r#"
name: bad
initial_state: start
steps:
  - name: step1
    method: GET
    url: http://example.com
    transition:
      from: wrong
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("initial_state"));
    }

    #[test]
    fn detects_state_gap() {
        let yaml = r#"
name: gap
initial_state: start
steps:
  - name: step1
    method: GET
    url: http://example.com
    transition:
      from: start
      to: middle
  - name: step2
    method: GET
    url: http://example.com
    transition:
      from: wrong
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("State gap"));
    }

    #[test]
    fn detects_duplicate_names() {
        let yaml = r#"
name: dup
initial_state: start
steps:
  - name: fetch
    method: GET
    url: http://example.com
    transition:
      from: start
      to: middle
  - name: fetch
    method: GET
    url: http://example.com
    transition:
      from: middle
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("Duplicate")));
    }

    #[test]
    fn detects_empty_scenario() {
        let yaml = r#"
name: empty
initial_state: start
steps: []
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("no steps")));
    }
}
