use model::{Edge, Scenario, Step};
use std::collections::HashMap;

/// Pre-computed adjacency view of a scenario graph.
///
/// Building this once at the start of a run avoids repeated linear scans over
/// `scenario.edges` for every state transition.
pub struct Graph<'a> {
    scenario: &'a Scenario,
    step_map: HashMap<&'a str, &'a Step>,
    outgoing: HashMap<&'a str, Vec<&'a Edge>>,
}

impl<'a> Graph<'a> {
    pub fn build(scenario: &'a Scenario) -> Self {
        let step_map = scenario
            .steps
            .iter()
            .map(|s| (s.state.as_str(), s))
            .collect();

        let mut outgoing: HashMap<&str, Vec<&Edge>> = HashMap::new();
        for edge in &scenario.edges {
            outgoing.entry(edge.from.as_str()).or_default().push(edge);
        }

        Self {
            scenario,
            step_map,
            outgoing,
        }
    }

    pub fn scenario(&self) -> &'a Scenario {
        self.scenario
    }

    /// Returns the step responsible for handling `state`, or `None` if `state`
    /// is a terminal (not backed by any step).
    pub fn step_for_state(&self, state: &str) -> Option<&'a Step> {
        self.step_map.get(state).copied()
    }

    /// Returns all outgoing edges from `state`.
    pub fn outgoing_edges(&self, state: &str) -> &[&'a Edge] {
        self.outgoing.get(state).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::load_scenario;

    const YAML: &str = r#"
name: graph test
initial_state: login
terminal_states: [done]
steps:
  - name: login
    state: login
    method: POST
    url: http://example.com/login
  - name: dashboard
    state: dashboard
    method: GET
    url: http://example.com/me
edges:
  - from: login
    to: dashboard
    when:
      status: 200
  - from: login
    to: done
    default: true
  - from: dashboard
    to: done
    default: true
"#;

    #[test]
    fn step_for_state_returns_step() {
        let scenario = load_scenario(YAML).unwrap();
        let graph = Graph::build(&scenario);
        assert!(graph.step_for_state("login").is_some());
        assert!(graph.step_for_state("dashboard").is_some());
    }

    #[test]
    fn step_for_terminal_returns_none() {
        let scenario = load_scenario(YAML).unwrap();
        let graph = Graph::build(&scenario);
        assert!(graph.step_for_state("done").is_none());
    }

    #[test]
    fn outgoing_edges_counts() {
        let scenario = load_scenario(YAML).unwrap();
        let graph = Graph::build(&scenario);
        assert_eq!(graph.outgoing_edges("login").len(), 2);
        assert_eq!(graph.outgoing_edges("dashboard").len(), 1);
        assert_eq!(graph.outgoing_edges("done").len(), 0);
    }
}
