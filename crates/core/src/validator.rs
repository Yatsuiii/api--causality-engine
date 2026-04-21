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

        if edge.parallel.is_some() {
            // Fan-out edge: normal `to`/target checks don't apply.
            // Mutually exclusive fields are flagged as E018.
            if !edge.to.trim().is_empty()
                || edge.when.is_some()
                || edge.default.is_some()
                || edge.weight.is_some()
            {
                issues.push(Diagnostic::error(
                    "E018",
                    format!(
                        "Edge from '{}': `parallel` is mutually exclusive with `to`, `when`, `default`, and `weight`",
                        edge.from
                    ),
                    edge_line,
                ));
            }
        } else {
            if edge.to.trim().is_empty() {
                issues.push(Diagnostic::error(
                    "E006",
                    format!("Edge from '{}' has empty 'to' state", edge.from),
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
        }

        outgoing.entry(&edge.from).or_default().push(edge);
    }

    issues.extend(validate_fan_outs(
        scenario,
        &state_set,
        &declared_terminals,
        &outgoing,
        index,
    ));

    for step in &scenario.steps {
        let state_line = index.state(&step.state);
        match outgoing.get(step.state.as_str()) {
            Some(all_edges) => {
                // E019: fan-out edges cannot coexist with siblings at the same source.
                let parallel_edges: Vec<&&Edge> =
                    all_edges.iter().filter(|e| e.parallel.is_some()).collect();
                if !parallel_edges.is_empty() && all_edges.len() > parallel_edges.len() {
                    issues.push(Diagnostic::error(
                        "E019",
                        format!(
                            "State '{}': fan-out edge cannot coexist with other outgoing edges — move siblings into the fan-out branches or a separate source state",
                            step.state
                        ),
                        state_line,
                    ));
                }
                if parallel_edges.len() > 1 {
                    issues.push(Diagnostic::error(
                        "E019",
                        format!(
                            "State '{}': declares more than one `parallel` edge — only one fan-out per state is allowed",
                            step.state
                        ),
                        state_line,
                    ));
                }

                // Remaining single-target-edge validations skip fan-out edges.
                let edges: Vec<&Edge> = all_edges
                    .iter()
                    .copied()
                    .filter(|e| e.parallel.is_none())
                    .collect();
                if edges.is_empty() {
                    continue;
                }
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
                if edges.len() > 1 {
                    // Implicit (unconditional non-default) group: the group
                    // is valid if ALL siblings in it have weights (weighted
                    // routing). Mixed weighted/unweighted is E010.
                    let implicit: Vec<&&Edge> = edges
                        .iter()
                        .filter(|e| e.when.is_none() && !e.default.unwrap_or(false))
                        .collect();
                    let all_weighted =
                        !implicit.is_empty() && implicit.iter().all(|e| e.weight.is_some());
                    let any_weighted = implicit.iter().any(|e| e.weight.is_some());

                    if any_weighted && !all_weighted {
                        for edge in &implicit {
                            if edge.weight.is_none() {
                                issues.push(Diagnostic::error(
                                    "E010",
                                    format!(
                                        "State '{}': edge to '{}' is in a weighted routing group but has no `weight:` — all siblings in a weighted group must declare weights",
                                        step.state, edge.to
                                    ),
                                    index.from_edge(&edge.from),
                                ));
                            }
                        }
                    }

                    if !all_weighted {
                        for edge in &implicit {
                            issues.push(Diagnostic::error(
                                "E007",
                                format!(
                                    "State '{}': unconditional edge to '{}' must be the only outgoing edge, marked default: true, or part of a weighted group — otherwise it shadows sibling edges non-deterministically",
                                    step.state, edge.to
                                ),
                                index.from_edge(&edge.from),
                            ));
                        }
                    }

                    // Conditional-same-priority group: flag mixed weights too.
                    // Pairwise check on conditional edges sharing priority.
                    for (i, a) in edges.iter().enumerate() {
                        for b in edges.iter().skip(i + 1) {
                            if a.when.is_none() || b.when.is_none() {
                                continue;
                            }
                            if a.priority != b.priority {
                                continue;
                            }
                            if a.weight.is_some() != b.weight.is_some() {
                                let unweighted = if a.weight.is_none() { &a.to } else { &b.to };
                                issues.push(Diagnostic::error(
                                    "E010",
                                    format!(
                                        "State '{}': conditional edges to '{}' and '{}' share priority but only one has a `weight:` — weighted groups must be all-or-nothing",
                                        step.state, a.to, b.to
                                    ),
                                    index.from_edge(unweighted),
                                ));
                            }
                        }
                    }

                    // E009: flag pairs of conditional edges with the same
                    // exact status match — they overlap silently; list order
                    // (or priority) decides which wins. Suppressed when both
                    // are part of a weighted group (weights break the tie).
                    for (i, a) in edges.iter().enumerate() {
                        for b in edges.iter().skip(i + 1) {
                            let (Some(ac), Some(bc)) = (&a.when, &b.when) else {
                                continue;
                            };
                            if let (
                                Some(StatusMatch::Exact(ac_code)),
                                Some(StatusMatch::Exact(bc_code)),
                            ) = (&ac.status, &bc.status)
                                && ac_code == bc_code
                                && a.priority == b.priority
                                && !(a.weight.is_some() && b.weight.is_some())
                            {
                                issues.push(Diagnostic::warning(
                                    "E009",
                                    format!(
                                        "State '{}': edges to '{}' and '{}' both match status {} with equal priority — order decides the winner; set `priority:` or add `weight:` to both to disambiguate",
                                        step.state, a.to, b.to, ac_code
                                    ),
                                    index.from_edge(&a.from),
                                ));
                            }
                        }
                    }
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
                    if let Some(fan_out) = &edge.parallel {
                        for branch in &fan_out.branches {
                            if !visited.contains(&branch.to) {
                                queue.push_back(branch.to.clone());
                            }
                        }
                        if !visited.contains(&fan_out.join) {
                            queue.push_back(fan_out.join.clone());
                        }
                    } else if !visited.contains(&edge.to) {
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

        for step in &scenario.steps {
            if !visited.contains(&step.state) {
                issues.push(Diagnostic::warning(
                    "E008",
                    format!(
                        "State '{}' is unreachable from initial_state — step '{}' will never execute",
                        step.state, step.name
                    ),
                    index.state(&step.state),
                ));
            }
        }
    }

    issues
}

/// E011–E017: structural validation for every `parallel:` edge.
fn validate_fan_outs(
    scenario: &Scenario,
    state_set: &HashSet<String>,
    declared_terminals: &HashSet<String>,
    outgoing: &HashMap<&str, Vec<&Edge>>,
    index: &LineIndex,
) -> Vec<Diagnostic> {
    let mut issues = Vec::new();

    // Collect the names of all variables that will exist in parent-scope
    // context at join time — for E017 branch-name collision detection.
    let mut parent_scope_keys: HashSet<String> = scenario
        .variables
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for step in &scenario.steps {
        if let Some(hooks) = &step.pre_request {
            for hook in hooks {
                if let Some(sets) = &hook.set {
                    for k in sets.keys() {
                        parent_scope_keys.insert(k.clone());
                    }
                }
            }
        }
        if let Some(extract) = &step.extract {
            for k in extract.keys() {
                parent_scope_keys.insert(k.clone());
            }
        }
    }

    for edge in &scenario.edges {
        let Some(fan_out) = edge.parallel.as_ref() else {
            continue;
        };
        let edge_line = index.from_edge(&edge.from);

        // E011: fan-out needs at least 2 branches to be meaningful.
        if fan_out.branches.len() < 2 {
            issues.push(Diagnostic::error(
                "E011",
                format!(
                    "State '{}': fan-out must declare at least 2 branches (found {})",
                    edge.from,
                    fan_out.branches.len()
                ),
                edge_line,
            ));
        }

        // E013: join state must be declared.
        let join_declared =
            state_set.contains(&fan_out.join) || declared_terminals.contains(&fan_out.join);
        if !join_declared {
            issues.push(Diagnostic::error(
                "E013",
                format!(
                    "State '{}': fan-out `join:` state '{}' is not a declared step-state or terminal_state",
                    edge.from, fan_out.join
                ),
                edge_line,
            ));
        }

        let mut seen_names: HashSet<&str> = HashSet::new();
        for branch in &fan_out.branches {
            // E016: duplicate branch names within this fan-out.
            if !seen_names.insert(branch.name.as_str()) {
                issues.push(Diagnostic::error(
                    "E016",
                    format!(
                        "State '{}': duplicate branch name '{}' in fan-out",
                        edge.from, branch.name
                    ),
                    edge_line,
                ));
            }

            // E017: branch name must not collide with a parent-scope key —
            // the merge would shadow (`{{name}}` already in use).
            if parent_scope_keys.contains(&branch.name) {
                issues.push(Diagnostic::error(
                    "E017",
                    format!(
                        "State '{}': branch name '{}' collides with an existing variable or extract key — merge would shadow",
                        edge.from, branch.name
                    ),
                    edge_line,
                ));
            }

            // E012: branch target must be declared.
            if branch.to.trim().is_empty() {
                issues.push(Diagnostic::error(
                    "E012",
                    format!(
                        "State '{}': branch '{}' has empty 'to' state",
                        edge.from, branch.name
                    ),
                    edge_line,
                ));
                continue;
            }
            if !state_set.contains(&branch.to) && !declared_terminals.contains(&branch.to) {
                issues.push(Diagnostic::error(
                    "E012",
                    format!(
                        "State '{}': branch '{}' target '{}' is not a declared step-state or terminal_state",
                        edge.from, branch.name, branch.to
                    ),
                    edge_line,
                ));
                continue;
            }

            // E014 + E015: BFS from branch.to. Every branch must be able to
            // reach `join`, and must not encounter another parallel edge
            // mid-traversal (nested fan-out is out of scope for v1).
            if join_declared {
                let mut visited: HashSet<&str> = HashSet::new();
                let mut queue: VecDeque<&str> = VecDeque::from([branch.to.as_str()]);
                let mut found_join = false;
                let mut found_nested = false;

                while let Some(state) = queue.pop_front() {
                    if state == fan_out.join.as_str() {
                        found_join = true;
                        continue;
                    }
                    if !visited.insert(state) {
                        continue;
                    }
                    if let Some(edges) = outgoing.get(state) {
                        for e in edges {
                            if e.parallel.is_some() {
                                found_nested = true;
                            } else if !e.to.is_empty() {
                                queue.push_back(e.to.as_str());
                            }
                        }
                    }
                }

                if !found_join {
                    issues.push(Diagnostic::error(
                        "E014",
                        format!(
                            "State '{}': branch '{}' (start '{}') cannot reach `join:` state '{}'",
                            edge.from, branch.name, branch.to, fan_out.join
                        ),
                        edge_line,
                    ));
                }
                if found_nested {
                    issues.push(Diagnostic::error(
                        "E015",
                        format!(
                            "State '{}': branch '{}' encounters another fan-out before reaching `join:` — nested fan-out is not supported",
                            edge.from, branch.name
                        ),
                        edge_line,
                    ));
                }
            }
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

    #[test]
    fn detects_unreachable_state_e008() {
        let yaml = r#"
name: dead code
initial_state: start
terminal_states: [done]
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
  - name: orphan
    state: orphan
    method: GET
    url: http://example.com
edges:
  - from: start
    to: done
    default: true
  - from: orphan
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E008"),
            "expected E008; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn detects_overlapping_status_edges_e009() {
        let yaml = r#"
name: overlap
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
    when:
      status: 200
  - from: start
    to: b
    when:
      status: 200
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E009"),
            "expected E009; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn weighted_implicit_group_suppresses_e007() {
        let yaml = r#"
name: weighted-ok
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
        let issues = validate(yaml);
        assert!(
            !issues.iter().any(|d| d.code == "E007"),
            "weighted group should suppress E007; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
        assert!(
            !issues.iter().any(|d| d.code == "E010"),
            "uniform-weighted group should not trigger E010; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mixed_weighted_implicit_group_triggers_e010() {
        let yaml = r#"
name: mixed-weights
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
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E010"),
            "expected E010; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_happy_path_validates_clean() {
        let yaml = r#"
name: fanout-ok
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
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
edges:
  - from: start
    parallel:
      branches:
        - name: left
          to: a
        - name: right
          to: b
      join: agg
  - from: a
    to: agg
    default: true
  - from: b
    to: agg
    default: true
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().all(|d| d.severity != Severity::Error),
            "expected no errors; got: {:?}",
            issues
        );
    }

    #[test]
    fn fan_out_single_branch_triggers_e011() {
        let yaml = r#"
name: fanout-e011
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
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
edges:
  - from: start
    parallel:
      branches:
        - name: lonely
          to: a
      join: agg
  - from: a
    to: agg
    default: true
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E011"),
            "expected E011; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_unknown_branch_target_triggers_e012() {
        let yaml = r#"
name: fanout-e012
initial_state: start
terminal_states: [done]
steps:
  - name: start
    state: start
    method: GET
    url: http://example.com
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
edges:
  - from: start
    parallel:
      branches:
        - name: left
          to: missing
        - name: right
          to: agg
      join: agg
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E012"),
            "expected E012; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_unknown_join_triggers_e013() {
        let yaml = r#"
name: fanout-e013
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
edges:
  - from: start
    parallel:
      branches:
        - name: left
          to: a
        - name: right
          to: b
      join: missing_join
  - from: a
    to: done
    default: true
  - from: b
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E013"),
            "expected E013; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_unreachable_join_triggers_e014() {
        // Branch 'left' goes to a -> done, never reaches agg.
        let yaml = r#"
name: fanout-e014
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
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
edges:
  - from: start
    parallel:
      branches:
        - name: left
          to: a
        - name: right
          to: b
      join: agg
  - from: a
    to: done
    default: true
  - from: b
    to: agg
    default: true
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E014"),
            "expected E014; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_nested_triggers_e015() {
        let yaml = r#"
name: fanout-e015
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
  - name: c
    state: c
    method: GET
    url: http://example.com/c
  - name: d
    state: d
    method: GET
    url: http://example.com/d
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
  - name: inner
    state: inner
    method: GET
    url: http://example.com/inner
edges:
  - from: start
    parallel:
      branches:
        - name: outer_left
          to: a
        - name: outer_right
          to: b
      join: agg
  - from: a
    parallel:
      branches:
        - name: inner_l
          to: c
        - name: inner_r
          to: d
      join: inner
  - from: b
    to: agg
    default: true
  - from: c
    to: inner
    default: true
  - from: d
    to: inner
    default: true
  - from: inner
    to: agg
    default: true
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E015"),
            "expected E015; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_duplicate_branch_names_trigger_e016() {
        let yaml = r#"
name: fanout-e016
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
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
edges:
  - from: start
    parallel:
      branches:
        - name: dup
          to: a
        - name: dup
          to: b
      join: agg
  - from: a
    to: agg
    default: true
  - from: b
    to: agg
    default: true
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E016"),
            "expected E016; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_branch_name_collision_triggers_e017() {
        let yaml = r#"
name: fanout-e017
initial_state: start
terminal_states: [done]
variables:
  posts: "preset"
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
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
edges:
  - from: start
    parallel:
      branches:
        - name: posts
          to: a
        - name: comments
          to: b
      join: agg
  - from: a
    to: agg
    default: true
  - from: b
    to: agg
    default: true
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E017"),
            "expected E017; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_mutually_exclusive_triggers_e018() {
        let yaml = r#"
name: fanout-e018
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
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
edges:
  - from: start
    to: agg
    parallel:
      branches:
        - name: left
          to: a
        - name: right
          to: b
      join: agg
  - from: a
    to: agg
    default: true
  - from: b
    to: agg
    default: true
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E018"),
            "expected E018; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fan_out_with_siblings_triggers_e019() {
        let yaml = r#"
name: fanout-e019
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
  - name: agg
    state: agg
    method: GET
    url: http://example.com/agg
edges:
  - from: start
    parallel:
      branches:
        - name: left
          to: a
        - name: right
          to: b
      join: agg
  - from: start
    to: agg
    default: true
  - from: a
    to: agg
    default: true
  - from: b
    to: agg
    default: true
  - from: agg
    to: done
    default: true
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E019"),
            "expected E019; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mixed_weighted_conditional_group_triggers_e010() {
        let yaml = r#"
name: mixed-cond-weights
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
    when:
      status: 200
    weight: 50
  - from: start
    to: b
    when:
      status: 200
"#;
        let issues = validate(yaml);
        assert!(
            issues.iter().any(|d| d.code == "E010"),
            "expected E010; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn priority_disambiguates_overlap_no_e009() {
        let yaml = r#"
name: prioritised
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
    when:
      status: 200
    priority: 10
  - from: start
    to: b
    when:
      status: 200
    priority: 1
"#;
        let issues = validate(yaml);
        assert!(
            !issues.iter().any(|d| d.code == "E009"),
            "priority should suppress E009; got: {:?}",
            issues.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }
}
