use model::{Edge, Scenario, StatusMatch};
use std::collections::{HashMap, HashSet, VecDeque};

pub fn validate_scenario(scenario: &Scenario) -> Vec<String> {
    let mut issues = Vec::new();

    if scenario.steps.is_empty() {
        issues.push("Scenario has no steps".into());
        return issues;
    }

    let mut step_names = HashSet::new();
    let mut step_states = HashSet::new();

    for step in &scenario.steps {
        if step.name.trim().is_empty() {
            issues.push("Step with empty name found — all steps must have a non-empty name".into());
        }
        if !step_names.insert(&step.name) {
            issues.push(format!("Duplicate step name: '{}'", step.name));
        }
        if step.state.trim().is_empty() {
            issues.push(format!("Step '{}': state is empty", step.name));
        }
        if !step_states.insert(&step.state) {
            issues.push(format!(
                "Duplicate state '{}': handled by more than one step",
                step.state
            ));
        }
        if step.url.trim().is_empty() {
            issues.push(format!("Step '{}': url is empty", step.name));
        }
        if let Some(retry) = &step.retry
            && retry.attempts == 0
        {
            issues.push(format!("Step '{}': retry attempts must be >= 1", step.name));
        }
    }

    if let Some(c) = scenario.concurrency
        && c == 0
    {
        issues.push("Concurrency must be >= 1".into());
    }

    if let Some(max) = scenario.max_iterations
        && max == 0
    {
        issues.push("max_iterations must be >= 1".into());
    }

    if !step_states.contains(&scenario.initial_state) {
        issues.push(format!(
            "initial_state '{}' is not handled by any step",
            scenario.initial_state
        ));
    }

    let state_set: HashSet<String> = scenario.steps.iter().map(|s| s.state.clone()).collect();
    let declared_terminals: HashSet<String> = scenario
        .terminal_states
        .as_ref()
        .map(|v| v.iter().cloned().collect())
        .unwrap_or_default();

    let mut outgoing: HashMap<&str, Vec<&Edge>> = HashMap::new();
    let mut incoming_targets = HashSet::new();

    for edge in &scenario.edges {
        if edge.from.trim().is_empty() {
            issues.push("Edge with empty 'from' state found".into());
        }
        if edge.to.trim().is_empty() {
            issues.push(format!("Edge from '{}' has empty 'to' state", edge.from));
        }
        if !state_set.contains(&edge.from) {
            issues.push(format!(
                "Edge '{} -> {}' starts from unknown state '{}'",
                edge.from, edge.to, edge.from
            ));
        }
        if !state_set.contains(&edge.to)
            && scenario.terminal_states.is_some()
            && !declared_terminals.contains(&edge.to)
        {
            issues.push(format!(
                "Transition target '{}' is not a declared state or terminal state",
                edge.to
            ));
        }
        if edge.from == edge.to && edge.when.is_none() {
            issues.push(format!(
                "Edge '{} -> {}' is an unconditional self-loop",
                edge.from, edge.to
            ));
        }

        outgoing.entry(&edge.from).or_default().push(edge);
        incoming_targets.insert(edge.to.clone());
    }

    for step in &scenario.steps {
        if let Some(edges) = outgoing.get(step.state.as_str()) {
            let has_default = edges.iter().any(|e| e.default.unwrap_or(false));
            let all_conditional = edges.iter().all(|e| e.when.is_some());
            if !has_default && all_conditional {
                issues.push(format!(
                    "State '{}': no default edge — execution may fail if no condition matches",
                    step.state
                ));
            }
        }
    }

    issues.extend(validate_variable_references(scenario));

    let inferred_terminals: HashSet<String> = scenario
        .steps
        .iter()
        .filter(|step| !outgoing.contains_key(step.state.as_str()))
        .map(|step| step.state.clone())
        .chain(
            incoming_targets
                .iter()
                .filter(|target| !state_set.contains(*target))
                .cloned(),
        )
        .collect();

    let effective_terminals: HashSet<String> = if let Some(terminals) = &scenario.terminal_states {
        terminals.iter().cloned().collect()
    } else {
        inferred_terminals
    };

    if effective_terminals.is_empty() {
        issues.push(
            "No terminal state is reachable from initial_state — workflow may loop forever".into(),
        );
        return issues;
    }

    if state_set.contains(&scenario.initial_state) {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::from([scenario.initial_state.clone()]);

        while let Some(state) = queue.pop_front() {
            if !visited.insert(state.clone()) {
                continue;
            }
            if let Some(edges) = outgoing.get(state.as_str()) {
                for edge in edges {
                    if !visited.contains(&edge.to) {
                        queue.push_back(edge.to.clone());
                    }
                }
            }
        }

        if !effective_terminals
            .iter()
            .any(|state| visited.contains(state))
        {
            issues.push(
                "No terminal state is reachable from initial_state — workflow may loop forever"
                    .into(),
            );
        }
    }

    issues
}

pub fn render_state_graph(scenario: &Scenario) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("initial_state: {}", scenario.initial_state));
    lines.push("mode: graph".to_string());

    if scenario.edges.is_empty() {
        for step in &scenario.steps {
            lines.push(format!("[{}] --(terminal)--> [{}]", step.state, step.state));
        }
        return lines;
    }

    for edge in &scenario.edges {
        let label = if edge.default.unwrap_or(false) {
            "default".to_string()
        } else if let Some(cond) = &edge.when {
            let mut parts = Vec::new();
            if let Some(status) = &cond.status {
                let s = match status {
                    StatusMatch::Exact(code) => format!("status={code}"),
                    StatusMatch::Complex(_) => "status=<rule>".to_string(),
                };
                parts.push(s);
            }
            if cond.assertions.is_some() {
                parts.push("assertions".to_string());
            }
            if cond.body.is_some() {
                parts.push("body".to_string());
            }
            if parts.is_empty() {
                "when".to_string()
            } else {
                parts.join("&")
            }
        } else {
            "always".to_string()
        };

        lines.push(format!("[{}] --({label})--> [{}]", edge.from, edge.to));
    }

    lines
}

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

fn is_builtin(key: &str) -> bool {
    matches!(key, "$uuid" | "$guid" | "$timestamp" | "$randomInt") || key.starts_with("$env.")
}

fn validate_variable_references(scenario: &Scenario) -> Vec<String> {
    let mut issues = Vec::new();
    let mut available: HashSet<String> = scenario
        .variables
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    for step in &scenario.steps {
        if let Some(hooks) = &step.pre_request {
            for hook in hooks {
                if let Some(sets) = &hook.set {
                    for k in sets.keys() {
                        available.insert(k.clone());
                    }
                }
            }
        }
        if let Some(extract) = &step.extract {
            for k in extract.keys() {
                available.insert(k.clone());
            }
        }
    }

    for step in &scenario.steps {
        let mut refs = template_refs(&step.url);
        if let Some(headers) = &step.headers {
            for v in headers.values() {
                refs.extend(template_refs(v));
            }
        }
        if let Some(body) = &step.body
            && let Ok(s) = serde_json::to_string(body)
        {
            refs.extend(template_refs(&s));
        }
        if let Some(hooks) = &step.pre_request {
            for hook in hooks {
                if let Some(sets) = &hook.set {
                    for v in sets.values() {
                        refs.extend(template_refs(v));
                    }
                }
            }
        }

        for var in refs {
            if !is_builtin(&var) && !available.contains(&var) {
                issues.push(format!(
                    "Step '{}': variable '{{{{{}}}}}' is used but never declared or extracted",
                    step.name, var
                ));
            }
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::load_scenario;

    #[test]
    fn validates_explicit_graph() {
        let yaml = r#"
name: ok
initial_state: start
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
  - name: finish
    state: finish
    method: GET
    url: http://example.com
edges:
  - from: start
    to: finish
    default: true
  - from: finish
    to: done
    default: true
"#;

        let scenario = load_scenario(yaml).unwrap();
        assert!(validate_scenario(&scenario).is_empty());
    }

    #[test]
    fn detects_missing_initial_state() {
        let yaml = r#"
name: bad
initial_state: missing
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
edges:
  - from: start
    to: done
    default: true
"#;

        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(issues.iter().any(|i| i.contains("initial_state")));
    }

    #[test]
    fn detects_unknown_edge_source() {
        let yaml = r#"
name: bad edge
initial_state: start
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
edges:
  - from: wrong
    to: done
    default: true
"#;

        let scenario = load_scenario(yaml).unwrap();
        let issues = validate_scenario(&scenario);
        assert!(
            issues
                .iter()
                .any(|i| i.contains("starts from unknown state"))
        );
    }
}
