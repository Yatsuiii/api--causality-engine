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
        if step.name.trim().is_empty() {
            issues.push("Step with empty name found — all steps must have a non-empty name".into());
        }
        if !seen_names.insert(&step.name) {
            issues.push(format!("Duplicate step name: '{}'", step.name));
        }
        if step.url.trim().is_empty() {
            issues.push(format!("Step '{}': url is empty", step.name));
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

// ---------------------------------------------------------------------------
// Template variable helpers
// ---------------------------------------------------------------------------

/// Extract all `{{key}}` references from a string.
fn template_refs(s: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut remaining = s;
    while let Some(start) = remaining.find("{{") {
        if let Some(end) = remaining[start + 2..].find("}}") {
            let key = remaining[start + 2..start + 2 + end].trim().to_string();
            refs.push(key);
            remaining = &remaining[start + 2 + end + 2..];
        } else {
            break;
        }
    }
    refs
}

/// Returns true for built-in dynamic variables that are always available.
fn is_builtin(key: &str) -> bool {
    matches!(key, "$uuid" | "$guid" | "$timestamp" | "$randomInt") || key.starts_with("$env.")
}

/// Collect all template references from a step's URL, headers, and body.
fn step_var_refs(step: &model::Step) -> Vec<String> {
    let mut refs = Vec::new();
    refs.extend(template_refs(&step.url));
    if let Some(headers) = &step.headers {
        for v in headers.values() {
            refs.extend(template_refs(v));
        }
    }
    if let Some(body) = &step.body {
        // Serialize to string so we can scan for {{...}} regardless of type
        if let Ok(s) = serde_json::to_string(body) {
            refs.extend(template_refs(&s));
        }
    }
    // pre_request set: values may also reference variables
    if let Some(hooks) = &step.pre_request {
        for hook in hooks {
            if let Some(sets) = &hook.set {
                for v in sets.values() {
                    refs.extend(template_refs(v));
                }
            }
        }
    }
    refs
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
        let prev_to = &scenario.steps[i - 1]
            .transition
            .as_ref()
            .expect("validate_linear: all steps have transition set")
            .to;
        let curr_from = &scenario.steps[i]
            .transition
            .as_ref()
            .expect("validate_linear: all steps have transition set")
            .from;
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

    // Undefined variable check: track what's available at each step and warn
    // about references that will silently resolve to empty string at runtime.
    // Variables become available from: scenario variables block, pre_request
    // set hooks on the current step, and extract: fields of all prior steps.
    let mut available: HashSet<String> = scenario
        .variables
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    for step in &scenario.steps {
        // pre_request sets are applied before the request fires, so their keys
        // are available within this step's own URL/headers/body.
        if let Some(hooks) = &step.pre_request {
            for hook in hooks {
                if let Some(sets) = &hook.set {
                    for k in sets.keys() {
                        available.insert(k.clone());
                    }
                }
            }
        }

        for var in step_var_refs(step) {
            if !is_builtin(&var) && !available.contains(&var) {
                issues.push(format!(
                    "Step '{}': variable '{{{{{}}}}}' is used but never declared or extracted",
                    step.name, var
                ));
            }
        }

        // After this step runs, its extract: fields become available downstream.
        if let Some(extract) = &step.extract {
            for k in extract.keys() {
                available.insert(k.clone());
            }
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

    // -----------------------------------------------------------------------
    // Empty URL and name checks
    // -----------------------------------------------------------------------

    #[test]
    fn detects_empty_url() {
        let yaml = r#"
name: empty url
initial_state: start
steps:
  - name: step1
    method: GET
    url: ""
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("url is empty")));
    }

    #[test]
    fn detects_empty_step_name() {
        let yaml = r#"
name: empty name
initial_state: start
steps:
  - name: ""
    method: GET
    url: "http://example.com"
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("empty name")));
    }

    // -----------------------------------------------------------------------
    // Undefined variable checks
    // -----------------------------------------------------------------------

    #[test]
    fn passes_when_var_declared_in_variables_block() {
        let yaml = r#"
name: declared
initial_state: start
variables:
  base_url: https://api.example.com
steps:
  - name: step1
    method: GET
    url: "{{base_url}}/users"
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    #[test]
    fn passes_when_var_extracted_by_prior_step() {
        let yaml = r#"
name: extracted
initial_state: start
steps:
  - name: login
    method: POST
    url: "http://example.com/login"
    extract:
      token: id
    transition:
      from: start
      to: profile
  - name: profile
    method: GET
    url: "http://example.com/me"
    headers:
      Authorization: "Bearer {{token}}"
    transition:
      from: profile
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    #[test]
    fn detects_undefined_variable_in_url() {
        let yaml = r#"
name: undefined var
initial_state: start
steps:
  - name: step1
    method: GET
    url: "http://example.com/users/{{user_id}}"
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(
            issues.iter().any(|i| i.contains("user_id")),
            "Expected undefined variable warning, got: {:?}",
            issues
        );
    }

    #[test]
    fn detects_undefined_variable_in_header() {
        let yaml = r#"
name: undefined header var
initial_state: start
steps:
  - name: step1
    method: GET
    url: "http://example.com/data"
    headers:
      Authorization: "Bearer {{token}}"
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("token")));
    }

    #[test]
    fn does_not_flag_builtins() {
        let yaml = r#"
name: builtins
initial_state: start
steps:
  - name: step1
    method: POST
    url: "http://example.com/events"
    headers:
      X-Request-ID: "{{$uuid}}"
      X-Time: "{{$timestamp}}"
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    #[test]
    fn does_not_flag_env_vars() {
        let yaml = r#"
name: env vars
initial_state: start
steps:
  - name: step1
    method: GET
    url: "{{$env.BASE_URL}}/health"
    headers:
      Authorization: "Bearer {{$env.API_TOKEN}}"
    transition:
      from: start
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    /// Regression: variable extracted on step N must not be flagged when used on step N+1,
    /// but a variable never extracted or declared must still be caught.
    #[test]
    fn catches_undefined_but_not_extracted() {
        let yaml = r#"
name: mixed
initial_state: start
variables:
  base_url: https://api.example.com
steps:
  - name: login
    method: POST
    url: "{{base_url}}/login"
    extract:
      token: id
    transition:
      from: start
      to: profile
  - name: profile
    method: GET
    url: "{{base_url}}/users/{{user_id}}"
    headers:
      Authorization: "Bearer {{token}}"
    transition:
      from: profile
      to: done
"#;
        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        // token and base_url are fine; user_id is never defined
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("user_id"));
    }
}
