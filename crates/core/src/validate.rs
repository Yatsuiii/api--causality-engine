use model::Scenario;
use std::collections::{HashMap, HashSet, VecDeque};

/// Returns true if any step uses graph-mode `transitions` field.
pub fn is_graph_mode(scenario: &Scenario) -> bool {
    scenario.steps.iter().any(|s| s.transitions.is_some())
}

/// Pre-flight validation of a scenario's state machine.
/// Returns a list of issues found (empty = valid).
pub fn validate_scenario(scenario: &Scenario) -> Vec<String> {
    let mut issues = Vec::new();

    if scenario.steps.is_empty() {
        issues.push("Scenario has no steps".into());
        return issues;
    }

    // Check that each step has exactly one of transition or transitions
    for step in &scenario.steps {
        if step.transition.is_some() && step.transitions.is_some() {
            issues.push(format!(
                "Step '{}': cannot have both 'transition' and 'transitions'",
                step.name,
            ));
        }
        if step.transition.is_none() && step.transitions.is_none() {
            issues.push(format!(
                "Step '{}': must have either 'transition' or 'transitions'",
                step.name,
            ));
        }
    }

    if !issues.is_empty() {
        return issues;
    }

    // Check for mixed mode (some linear, some graph)
    let has_linear = scenario.steps.iter().any(|s| s.transition.is_some());
    let has_graph = is_graph_mode(scenario);
    if has_linear && has_graph {
        issues.push(
            "Mixed mode: all steps must use either 'transition' (linear) or 'transitions' (graph), not both"
                .into(),
        );
        return issues;
    }

    // Common checks
    let mut seen_names = HashSet::new();
    for step in &scenario.steps {
        if !seen_names.insert(&step.name) {
            issues.push(format!("Duplicate step name: '{}'", step.name));
        }
    }

    if let Some(c) = scenario.concurrency
        && c == 0
    {
        issues.push("Concurrency must be >= 1".into());
    }

    for step in &scenario.steps {
        if let Some(retry) = &step.retry
            && retry.attempts == 0
        {
            issues.push(format!("Step '{}': retry attempts must be >= 1", step.name));
        }
    }

    if has_graph {
        issues.extend(validate_graph(scenario));
    } else {
        issues.extend(validate_linear(scenario));
    }

    issues
}

/// Linear-mode validation: sequential state continuity.
fn validate_linear(scenario: &Scenario) -> Vec<String> {
    let mut issues = Vec::new();

    let first = scenario.steps[0].transition.as_ref()
        .expect("validate_linear is only called after validate_scenario confirms all steps have transition set");
    if first.from != scenario.initial_state {
        issues.push(format!(
            "First step '{}' expects state '{}', but initial_state is '{}'",
            scenario.steps[0].name, first.from, scenario.initial_state,
        ));
    }

    for i in 1..scenario.steps.len() {
        let prev_to = &scenario.steps[i - 1].transition.as_ref()
            .expect("validate_linear: all steps have transition set").to;
        let curr_from = &scenario.steps[i].transition.as_ref()
            .expect("validate_linear: all steps have transition set").from;
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

    issues
}

/// Graph-mode validation: state graph reachability and integrity.
fn validate_graph(scenario: &Scenario) -> Vec<String> {
    let mut issues = Vec::new();

    // Build state -> step mapping
    let mut state_map: HashMap<String, &str> = HashMap::new();
    for step in &scenario.steps {
        let state = step.state_name().to_string();
        if let Some(existing) = state_map.get(&state) {
            issues.push(format!(
                "Duplicate state '{}': handled by both '{}' and '{}'",
                state, existing, step.name,
            ));
        } else {
            state_map.insert(state, &step.name);
        }
    }

    // Check initial_state is handled by some step
    if !state_map.contains_key(&scenario.initial_state) {
        issues.push(format!(
            "initial_state '{}' is not handled by any step",
            scenario.initial_state,
        ));
    }

    // Collect all edge targets and check for dangling references
    let mut all_targets: HashSet<String> = HashSet::new();
    let terminal_set: HashSet<String> = scenario
        .terminal_states
        .as_ref()
        .map(|v| v.iter().cloned().collect())
        .unwrap_or_default();

    for step in &scenario.steps {
        let edges = step.transitions.as_ref()
            .expect("validate_graph is only called after validate_scenario confirms all steps have transitions set");

        // Check each step has at least one edge
        if edges.is_empty() {
            issues.push(format!("Step '{}': transitions list is empty", step.name));
            continue;
        }

        // Check for default edge
        let has_default = edges.iter().any(|e| e.default.unwrap_or(false));
        let all_conditional = edges.iter().all(|e| e.when.is_some());
        if !has_default && all_conditional {
            issues.push(format!(
                "Step '{}': no default transition — execution may fail at runtime if no condition matches",
                step.name,
            ));
        }

        // Check for unconditional self-loops
        let state = step.state_name();
        for edge in edges {
            all_targets.insert(edge.to.clone());
            if edge.to == state && edge.when.is_none() {
                issues.push(format!(
                    "Step '{}': unconditional self-loop to state '{}'",
                    step.name, state,
                ));
            }
        }
    }

    // Check all targets are either handled states or terminal states
    for target in &all_targets {
        if !state_map.contains_key(target) && !terminal_set.contains(target) {
            // It's an inferred terminal state — that's fine.
            // But if terminal_states was explicitly declared, it must be listed.
            if scenario.terminal_states.is_some() {
                issues.push(format!(
                    "Transition target '{}' is not a declared state or terminal state",
                    target,
                ));
            }
        }
    }

    // Check at least one terminal state is reachable via BFS
    let inferred_terminals: HashSet<&String> = all_targets
        .iter()
        .filter(|t| !state_map.contains_key(*t))
        .collect();

    let effective_terminals: HashSet<&String> = if scenario.terminal_states.is_some() {
        terminal_set.iter().collect()
    } else {
        inferred_terminals
    };

    if effective_terminals.is_empty() {
        issues.push(
            "No terminal state is reachable from initial_state — workflow may loop forever".into(),
        );
    } else if state_map.contains_key(&scenario.initial_state) {
        // BFS from initial_state
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(scenario.initial_state.clone());

        while let Some(state) = queue.pop_front() {
            if !visited.insert(state.clone()) {
                continue;
            }
            // Find step handling this state
            if let Some(step) = scenario.steps.iter().find(|s| s.state_name() == state)
                && let Some(edges) = &step.transitions
            {
                for edge in edges {
                    if !visited.contains(&edge.to) {
                        queue.push_back(edge.to.clone());
                    }
                }
            }
        }

        let terminal_reachable = effective_terminals.iter().any(|t| visited.contains(*t));
        if !terminal_reachable {
            issues.push(
                "No terminal state is reachable from initial_state — workflow may loop forever"
                    .into(),
            );
        }
    }

    // Validate max_iterations
    if let Some(max) = scenario.max_iterations
        && max == 0
    {
        issues.push("max_iterations must be >= 1".into());
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

    // -----------------------------------------------------------------------
    // Graph-mode validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn valid_graph_scenario() {
        let yaml = r#"
name: graph
initial_state: login
steps:
  - name: login
    method: POST
    url: http://example.com/auth
    transitions:
      - to: dashboard
        when:
          status: 200
      - to: failed
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    #[test]
    fn graph_detects_duplicate_state() {
        let yaml = r#"
name: dup state
initial_state: check
steps:
  - name: check_a
    state: check
    method: GET
    url: http://example.com
    transitions:
      - to: done
        default: true
  - name: check_b
    state: check
    method: GET
    url: http://example.com
    transitions:
      - to: done
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("Duplicate state")));
    }

    #[test]
    fn graph_detects_missing_initial_state() {
        let yaml = r#"
name: missing
initial_state: nonexistent
steps:
  - name: step1
    method: GET
    url: http://example.com
    transitions:
      - to: done
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("initial_state")));
    }

    #[test]
    fn graph_detects_unconditional_self_loop() {
        let yaml = r#"
name: loop
initial_state: spin
steps:
  - name: spin
    method: GET
    url: http://example.com
    transitions:
      - to: spin
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("self-loop")));
    }

    #[test]
    fn graph_warns_no_default_edge() {
        let yaml = r#"
name: no default
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
        when:
          status: 500
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("no default")));
    }

    #[test]
    fn graph_detects_mixed_mode() {
        let yaml = r#"
name: mixed
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
    transitions:
      - to: done
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("Mixed mode")));
    }

    #[test]
    fn graph_unreachable_terminal() {
        let yaml = r#"
name: unreachable
initial_state: a
steps:
  - name: a
    method: GET
    url: http://example.com
    transitions:
      - to: b
        default: true
  - name: b
    method: GET
    url: http://example.com
    transitions:
      - to: a
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("loop forever")));
    }
}
