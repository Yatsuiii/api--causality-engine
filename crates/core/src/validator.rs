use model::{Edge, Scenario, StatusMatch};
use std::collections::{HashMap, HashSet, VecDeque};

// ── Public diagnostic types ───────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub line: Option<usize>,
}

impl Diagnostic {
    fn error(code: &'static str, message: impl Into<String>, line: Option<usize>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            line,
        }
    }

    fn warning(code: &'static str, message: impl Into<String>, line: Option<usize>) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            line,
        }
    }
}

// ── Line index (two-pass span lookup) ────────────────────────────────────

/// Scans raw YAML for step names, states, and edge `from:` values to provide
/// approximate line numbers for diagnostics without switching YAML parsers.
pub struct LineIndex(HashMap<String, usize>);

impl LineIndex {
    pub fn build(yaml: &str) -> Self {
        let mut map = HashMap::new();
        for (i, line) in yaml.lines().enumerate() {
            let trimmed = line.trim();
            let lineno = i + 1;
            if let Some(rest) = trimmed
                .strip_prefix("- name:")
                .or_else(|| trimmed.strip_prefix("name:"))
            {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    map.entry(format!("step:{val}")).or_insert(lineno);
                }
            } else if let Some(rest) = trimmed
                .strip_prefix("- state:")
                .or_else(|| trimmed.strip_prefix("state:"))
            {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    map.entry(format!("state:{val}")).or_insert(lineno);
                }
            } else if let Some(rest) = trimmed
                .strip_prefix("- from:")
                .or_else(|| trimmed.strip_prefix("from:"))
            {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    map.entry(format!("from:{val}")).or_insert(lineno);
                }
            }
        }
        Self(map)
    }

    pub fn empty() -> Self {
        Self(HashMap::new())
    }

    pub fn step(&self, name: &str) -> Option<usize> {
        self.0.get(&format!("step:{name}")).copied()
    }

    pub fn state(&self, state: &str) -> Option<usize> {
        self.0.get(&format!("state:{state}")).copied()
    }

    pub fn from_edge(&self, from: &str) -> Option<usize> {
        self.0.get(&format!("from:{from}")).copied()
    }
}

// ── Validator ─────────────────────────────────────────────────────────────

pub fn validate_scenario(scenario: &Scenario, index: &LineIndex) -> Vec<Diagnostic> {
    let mut issues = Vec::new();

    if scenario.steps.is_empty() {
        issues.push(Diagnostic::error("E006", "Scenario has no steps", None));
        return issues;
    }

    let edges_declared = !scenario.edges.is_empty();
    if !edges_declared {
        issues.push(Diagnostic::error(
            "E006",
            "Scenario must declare at least one explicit edge",
            None,
        ));
    }

    let mut step_names = HashSet::new();
    let mut step_states = HashSet::new();

    for step in &scenario.steps {
        let step_line = index.step(&step.name);
        if step.name.trim().is_empty() {
            issues.push(Diagnostic::error(
                "E006",
                "Step with empty name found — all steps must have a non-empty name",
                None,
            ));
        }
        if !step_names.insert(&step.name) {
            issues.push(Diagnostic::error(
                "E005",
                format!("Duplicate step name: '{}'", step.name),
                step_line,
            ));
        }
        if step.state.trim().is_empty() {
            issues.push(Diagnostic::error(
                "E006",
                format!("Step '{}': state is empty", step.name),
                step_line,
            ));
        }
        if !step_states.insert(&step.state) {
            issues.push(Diagnostic::error(
                "E005",
                format!(
                    "Duplicate state '{}': handled by more than one step",
                    step.state
                ),
                index.state(&step.state),
            ));
        }
        if step.url.trim().is_empty() {
            issues.push(Diagnostic::error(
                "E006",
                format!("Step '{}': url is empty", step.name),
                step_line,
            ));
        }
        if let Some(retry) = &step.retry
            && retry.attempts == 0
        {
            issues.push(Diagnostic::error(
                "E006",
                format!("Step '{}': retry attempts must be >= 1", step.name),
                step_line,
            ));
        }
    }

    #[allow(deprecated)]
    if let Some(c) = scenario.concurrency
        && c == 0
    {
        issues.push(Diagnostic::error("E006", "Concurrency must be >= 1", None));
    }

    if let Some(max) = scenario.max_iterations
        && max == 0
    {
        issues.push(Diagnostic::error(
            "E006",
            "max_iterations must be >= 1",
            None,
        ));
    }

    if !step_states.contains(&scenario.initial_state) {
        issues.push(Diagnostic::error(
            "E006",
            format!(
                "initial_state '{}' is not handled by any step",
                scenario.initial_state
            ),
            None,
        ));
    }

    let state_set: HashSet<String> = scenario.steps.iter().map(|s| s.state.clone()).collect();
    let declared_terminals: HashSet<String> = scenario
        .terminal_states
        .as_ref()
        .map(|v| v.iter().cloned().collect())
        .unwrap_or_default();

    let mut outgoing: HashMap<&str, Vec<&Edge>> = HashMap::new();

    for edge in &scenario.edges {
        let edge_line = index.from_edge(&edge.from);
        if edge.from.trim().is_empty() {
            issues.push(Diagnostic::error(
                "E006",
                "Edge with empty 'from' state found",
                None,
            ));
        }
        if edge.to.trim().is_empty() {
            issues.push(Diagnostic::error(
                "E006",
                format!("Edge from '{}' has empty 'to' state", edge.from),
                edge_line,
            ));
        }
        if !state_set.contains(&edge.from) {
            issues.push(Diagnostic::error(
                "E004",
                format!(
                    "Edge '{} -> {}' starts from unknown state '{}'",
                    edge.from, edge.to, edge.from
                ),
                edge_line,
            ));
        }
        if !state_set.contains(&edge.to) && !declared_terminals.contains(&edge.to) {
            issues.push(Diagnostic::error(
                "E003",
                format!(
                    "Transition target '{}' is not a declared step-state or terminal_state",
                    edge.to
                ),
                edge_line,
            ));
        }
        if edge.from == edge.to && edge.when.is_none() {
            issues.push(Diagnostic::error(
                "E006",
                format!(
                    "Edge '{} -> {}' is an unconditional self-loop",
                    edge.from, edge.to
                ),
                edge_line,
            ));
        }

        outgoing.entry(&edge.from).or_default().push(edge);
    }

    for step in &scenario.steps {
        let state_line = index.state(&step.state);
        match outgoing.get(step.state.as_str()) {
            Some(edges) => {
                let has_default = edges.iter().any(|e| e.default.unwrap_or(false));
                let all_conditional = edges.iter().all(|e| e.when.is_some());
                if !has_default && all_conditional {
                    issues.push(Diagnostic::warning(
                        "E006",
                        format!(
                            "State '{}': no default edge — execution may fail if no condition matches",
                            step.state
                        ),
                        state_line,
                    ));
                }
            }
            None if edges_declared => {
                issues.push(Diagnostic::error(
                    "E002",
                    format!(
                        "State '{}': missing outgoing edge — explicit graphs require every state to transition",
                        step.state
                    ),
                    state_line,
                ));
            }
            None => {}
        }
    }

    issues.extend(validate_variable_references(scenario, index));

    let effective_terminals = declared_terminals.clone();

    if effective_terminals.is_empty() {
        issues.push(Diagnostic::error(
            "E006",
            "No terminal state is reachable from initial_state — workflow may loop forever",
            None,
        ));
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
            issues.push(Diagnostic::error(
                "E006",
                "No terminal state is reachable from initial_state — workflow may loop forever",
                None,
            ));
        }
    }

    issues
}

pub fn render_state_graph(scenario: &Scenario) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("initial_state: {}", scenario.initial_state));
    lines.push("mode: graph".to_string());

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

fn validate_variable_references(scenario: &Scenario, index: &LineIndex) -> Vec<Diagnostic> {
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
        let step_line = index.step(&step.name);
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
                issues.push(Diagnostic::error(
                    "E001",
                    format!(
                        "Step '{}': variable '{{{{{}}}}}' is used but never declared or extracted",
                        step.name, var
                    ),
                    step_line,
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

    fn validate(yaml: &str) -> Vec<Diagnostic> {
        let scenario = load_scenario(yaml).unwrap();
        let index = LineIndex::build(yaml);
        validate_scenario(&scenario, &index)
    }

    #[test]
    fn validates_explicit_graph() {
        let yaml = r#"
name: ok
initial_state: start
terminal_states: [done]
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
        assert!(validate(yaml).is_empty());
    }

    #[test]
    fn detects_undeclared_terminal_target() {
        let yaml = r#"
name: missing terminal
initial_state: start
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
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E003"),
            "expected E003; got: {:?}",
            issues.iter().map(|d| &d.code).collect::<Vec<_>>()
        );
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
        let issues = validate(yaml);
        assert!(issues.iter().any(|d| d.message.contains("initial_state")));
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
        let issues = validate(yaml);
        assert!(issues.iter().any(|d| d.code == "E004"));
    }

    #[test]
    fn empty_edges_does_not_cascade_per_step_errors() {
        let yaml = r#"
name: no edges
initial_state: start
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
  - name: next
    state: next
    method: GET
    url: http://example.com
edges: []
"#;
        let issues = validate(yaml);
        assert!(
            issues
                .iter()
                .any(|d| d.message.contains("at least one explicit edge")),
        );
        assert!(
            !issues.iter().any(|d| d.code == "E002"),
            "per-step missing-edge errors should not cascade when edges is empty; got: {:?}",
            issues
        );
    }

    #[test]
    fn detects_missing_outgoing_edge() {
        let yaml = r#"
name: bad edge
initial_state: start
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
  - name: next
    state: next
    method: GET
    url: http://example.com
edges:
  - from: start
    to: next
    default: true
"#;
        let issues = validate(yaml);
        assert!(issues.iter().any(|d| d.code == "E002"));
    }

    #[test]
    fn line_index_finds_step_lines() {
        let yaml = r#"
name: ok
initial_state: start
terminal_states: [done]
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
        let index = LineIndex::build(yaml);
        assert!(index.step("start").is_some());
        assert!(index.state("start").is_some());
        assert!(index.from_edge("start").is_some());
    }
}
