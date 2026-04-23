use crate::assertions::SchemaCache;
use crate::auth::fetch_oauth2_token;
use crate::config::{RunConfig, RunError};
use crate::edges::{default_edge_target, evaluate_edges};
use crate::graph::Graph;
use crate::http::execute_step;
use crate::log::{ExecutionLog, StepLog};
use crate::redact::Redactor;
use crate::trace::EdgeOutcome;
use crate::variables::{self, Context};
use ace_http::{Client, ClientConfig, build_client};
use model::{FailurePolicy, FanOut, Scenario};
use rand::{SeedableRng, rngs::StdRng};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info};

fn build_redactor(config: &RunConfig, scenario: &Scenario) -> Redactor {
    let log_cfg = scenario.log.clone().unwrap_or_default();
    Redactor::new(
        config.redact,
        log_cfg.include_bodies,
        log_cfg.max_body_bytes,
        log_cfg.mask,
        log_cfg.unmask,
    )
}

fn apply_redaction(step: &mut StepLog, redactor: &Redactor) {
    step.url = redactor.redact_url(&step.url);
    if let Some(b) = step.request_body.take() {
        step.request_body = redactor.redact_body(&b);
    }
    if let Some(b) = step.response_body.take() {
        step.response_body = redactor.redact_body(&b);
    }
    for a in step.assertions.iter_mut() {
        redactor.scrub_assertion(a);
    }
}

async fn run_once(
    scenario: &Scenario,
    task_id: usize,
    config: &RunConfig,
) -> (ExecutionLog, Result<String, RunError>) {
    let client_config = ClientConfig {
        insecure: config.insecure || scenario.insecure.unwrap_or(false),
        proxy: config.proxy.clone().or_else(|| scenario.proxy.clone()),
        default_timeout_ms: scenario.default_timeout_ms,
    };
    let client = build_client(&client_config);
    let redactor = build_redactor(config, scenario);
    let schema_cache = SchemaCache::new();

    let mut context =
        variables::build_initial_context(scenario.variables.as_ref(), &config.cli_variables);

    let mut log = ExecutionLog::default();

    if let Some(auth) = &scenario.auth
        && let Some(oauth) = &auth.oauth2
    {
        match fetch_oauth2_token(&client, oauth, &context).await {
            Ok(token) => {
                debug!(task_id, "OAuth2 token acquired");
                context.insert("$oauth_token".into(), serde_json::Value::String(token));
            }
            Err(e) => {
                return (
                    log,
                    Err(RunError::HttpError {
                        step: "<oauth2>".into(),
                        message: e,
                    }),
                );
            }
        }
    }

    let graph = Graph::build(scenario);

    let run_start = std::time::Instant::now();
    let mut current_state = scenario.initial_state.clone();
    let max_iter = scenario.max_iterations.unwrap_or(100);
    let mut edge_takes: HashMap<usize, u32> = HashMap::new();

    let base_seed = config.seed.unwrap_or_else(rand::random);
    let task_seed = base_seed.wrapping_add(task_id as u64);
    log.seed = task_seed;
    let mut rng = StdRng::seed_from_u64(task_seed);
    info!(task_id, base_seed, task_seed, "Weighted-routing seed");

    loop {
        log.iterations += 1;
        if log.iterations > max_iter {
            log.total_duration_ms = run_start.elapsed().as_millis() as u64;
            let owned_log = std::mem::take(&mut log);
            return (
                owned_log,
                Err(RunError::MaxIterationsExceeded { limit: max_iter }),
            );
        }

        let step = match graph.step_for_state(&current_state) {
            Some(step) => step,
            None => {
                log.terminal_state = Some(current_state.clone());
                break;
            }
        };

        let state_before = current_state.clone();

        match execute_step(
            step,
            &client,
            &mut context,
            scenario.auth.as_ref(),
            config,
            task_id,
            &schema_cache,
        )
        .await
        {
            Ok(result) => {
                let edges = graph.outgoing_edges(&state_before);
                if edges.is_empty() {
                    log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                    let owned_log = std::mem::take(&mut log);
                    return (
                        owned_log,
                        Err(RunError::NoOutgoingEdges {
                            step: step.name.clone(),
                            state: state_before.clone(),
                        }),
                    );
                }

                let decision = evaluate_edges(
                    edges,
                    &result.response,
                    &result.assertion_results,
                    &mut edge_takes,
                    &mut rng,
                );
                let edge_idx = match decision.chosen {
                    Some(idx) => idx,
                    None => {
                        // No-match OR max-takes cap hit. Push a synthetic
                        // StepLog carrying the evaluations so the CLI can
                        // render "why no edge fired" under this step.
                        let capped_edge =
                            decision.evaluations.iter().find_map(|e| match &e.outcome {
                                EdgeOutcome::MaxTakesExceeded { limit } => {
                                    Some((e.to.clone(), *limit))
                                }
                                _ => None,
                            });
                        let mut synth = StepLog {
                            step_name: step.name.clone(),
                            state_before: state_before.clone(),
                            state_after: state_before.clone(),
                            method: step.method.as_str().to_string(),
                            url: result.url_sent,
                            status: result.response.status,
                            duration_ms: result.response.duration_ms,
                            assertions: result.assertion_results.clone(),
                            matched_edge_tag: None,
                            branch_path: None,
                            request_body: if config.verbose {
                                result.body_sent
                            } else {
                                None
                            },
                            response_body: if config.verbose {
                                Some(result.response.body.clone())
                            } else {
                                None
                            },
                            edge_evaluations: decision.evaluations,
                        };
                        apply_redaction(&mut synth, &redactor);
                        log.steps.push(synth);
                        log.total_steps += 1;
                        log.failed += 1;
                        log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                        let owned_log = std::mem::take(&mut log);
                        let err = match capped_edge {
                            Some((to, limit)) => RunError::EdgeMaxTakesExceeded {
                                state: state_before.clone(),
                                to,
                                limit,
                            },
                            None => RunError::NoMatchingTransition {
                                state: state_before.clone(),
                                status: result.response.status,
                            },
                        };
                        return (owned_log, Err(err));
                    }
                };
                let matched_edge = edges[edge_idx];
                let evaluations = decision.evaluations;

                if let Some(fan_out) = &matched_edge.parallel {
                    let mut dispatch_step = StepLog {
                        step_name: step.name.clone(),
                        state_before: state_before.clone(),
                        state_after: fan_out.join.clone(),
                        method: step.method.as_str().to_string(),
                        url: result.url_sent,
                        status: result.response.status,
                        duration_ms: result.response.duration_ms,
                        assertions: result.assertion_results.clone(),
                        matched_edge_tag: matched_edge.tag.clone(),
                        branch_path: None,
                        request_body: if config.verbose {
                            result.body_sent
                        } else {
                            None
                        },
                        response_body: if config.verbose {
                            Some(result.response.body.clone())
                        } else {
                            None
                        },
                        edge_evaluations: evaluations,
                    };
                    apply_redaction(&mut dispatch_step, &redactor);
                    log.steps.push(dispatch_step);
                    log.total_steps += 1;
                    if result.all_passed {
                        log.passed += 1;
                    } else {
                        log.failed += 1;
                    }

                    if let Some(delay) = matched_edge.after_ms
                        && delay > 0
                    {
                        sleep(Duration::from_millis(delay)).await;
                    }

                    match execute_fan_out(
                        fan_out,
                        scenario,
                        &graph,
                        &client,
                        &mut context,
                        config,
                        task_id,
                        task_seed,
                        max_iter,
                        &mut log,
                        &redactor,
                        &schema_cache,
                    )
                    .await
                    {
                        Ok(()) => {
                            current_state = fan_out.join.clone();
                            continue;
                        }
                        Err(e) => {
                            log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                            let owned_log = std::mem::take(&mut log);
                            return (owned_log, Err(e));
                        }
                    }
                }

                let next_state = matched_edge.to.clone();

                if let Some(delay) = matched_edge.after_ms
                    && delay > 0
                {
                    sleep(Duration::from_millis(delay)).await;
                }

                let mut main_step = StepLog {
                    step_name: step.name.clone(),
                    state_before: state_before.clone(),
                    state_after: next_state.clone(),
                    method: step.method.as_str().to_string(),
                    url: result.url_sent,
                    status: result.response.status,
                    duration_ms: result.response.duration_ms,
                    assertions: result.assertion_results.clone(),
                    matched_edge_tag: matched_edge.tag.clone(),
                    branch_path: None,
                    request_body: if config.verbose {
                        result.body_sent
                    } else {
                        None
                    },
                    response_body: if config.verbose {
                        Some(result.response.body.clone())
                    } else {
                        None
                    },
                    edge_evaluations: evaluations,
                };
                apply_redaction(&mut main_step, &redactor);
                log.steps.push(main_step);

                log.total_steps += 1;
                if result.all_passed {
                    log.passed += 1;
                } else {
                    log.failed += 1;
                }

                current_state = next_state;
            }
            Err(RunError::Skipped { .. }) => {
                let edges = graph.outgoing_edges(&state_before);
                let next_state = match default_edge_target(edges) {
                    Some(target) => target,
                    None => {
                        log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                        let owned_log = std::mem::take(&mut log);
                        return (
                            owned_log,
                            Err(RunError::NoOutgoingEdges {
                                step: step.name.clone(),
                                state: state_before.clone(),
                            }),
                        );
                    }
                };
                debug!(
                    task_id,
                    step = step.name.as_str(),
                    next = next_state.as_str(),
                    "Step skipped, following default edge"
                );
                current_state = next_state;
            }
            Err(e) => {
                log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                let owned_log = std::mem::take(&mut log);
                return (owned_log, Err(e));
            }
        }
    }

    log.total_duration_ms = run_start.elapsed().as_millis() as u64;
    (std::mem::take(&mut log), Ok(current_state))
}

#[derive(Default)]
struct BranchStats {
    iterations: u64,
    total_steps: usize,
    passed: usize,
    failed: usize,
}

#[allow(clippy::too_many_arguments)]
async fn execute_fan_out(
    fan_out: &FanOut,
    scenario: &Scenario,
    graph: &Graph<'_>,
    client: &Client,
    context: &mut Context,
    config: &RunConfig,
    task_id: usize,
    task_seed: u64,
    max_iter: u64,
    log: &mut ExecutionLog,
    redactor: &Redactor,
    schema_cache: &SchemaCache,
) -> Result<(), RunError> {
    let policy = fan_out.on_failure.unwrap_or_default();

    info!(
        task_id,
        branches = fan_out.branches.len(),
        join = fan_out.join.as_str(),
        policy = ?policy,
        "Fan-out dispatch"
    );

    let mut futures = Vec::with_capacity(fan_out.branches.len());
    for (idx, branch) in fan_out.branches.iter().enumerate() {
        let branch_seed = task_seed.wrapping_add(
            (idx as u64)
                .wrapping_add(1)
                .wrapping_mul(0x9E3779B97F4A7C15),
        );
        let branch_rng = StdRng::seed_from_u64(branch_seed);
        let branch_ctx = context.clone();
        futures.push(run_branch(
            scenario,
            graph,
            client,
            branch_ctx,
            config,
            task_id,
            branch.name.clone(),
            branch.to.clone(),
            fan_out.join.clone(),
            branch_rng,
            max_iter,
            redactor,
            schema_cache,
        ));
    }

    let results = futures::future::join_all(futures).await;

    let mut first_err: Option<RunError> = None;
    let mut successful_branches: Vec<(String, Context)> = Vec::new();

    for (branch, outcome) in fan_out.branches.iter().zip(results) {
        let (br_ctx, br_steps, br_stats, br_result) = outcome;
        log.steps.extend(br_steps);
        log.iterations += br_stats.iterations;
        log.total_steps += br_stats.total_steps;
        log.passed += br_stats.passed;
        log.failed += br_stats.failed;

        match br_result {
            Ok(()) => successful_branches.push((branch.name.clone(), br_ctx)),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }

    if let Some(e) = first_err {
        // fail_fast: do not pollute parent context with partial branch data.
        // all_complete: merge successful branches, then surface error.
        if matches!(policy, FailurePolicy::AllComplete) {
            merge_branches_into_parent(context, successful_branches);
        }
        return Err(e);
    }

    merge_branches_into_parent(context, successful_branches);
    Ok(())
}

fn merge_branches_into_parent(parent: &mut Context, branches: Vec<(String, Context)>) {
    for (name, ctx) in branches {
        let mut obj = serde_json::Map::with_capacity(ctx.len());
        for (k, v) in ctx {
            obj.insert(k, v);
        }
        parent.insert(name, serde_json::Value::Object(obj));
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_branch(
    scenario: &Scenario,
    graph: &Graph<'_>,
    client: &Client,
    mut context: Context,
    config: &RunConfig,
    task_id: usize,
    branch_name: String,
    start_state: String,
    stop_state: String,
    mut rng: StdRng,
    max_iter: u64,
    redactor: &Redactor,
    schema_cache: &SchemaCache,
) -> (Context, Vec<StepLog>, BranchStats, Result<(), RunError>) {
    let mut steps_out: Vec<StepLog> = Vec::new();
    let mut stats = BranchStats::default();
    let mut current_state = start_state;
    let mut edge_takes: HashMap<usize, u32> = HashMap::new();
    let branch_path = vec![branch_name.clone()];

    loop {
        if current_state == stop_state {
            return (context, steps_out, stats, Ok(()));
        }
        stats.iterations += 1;
        if stats.iterations > max_iter {
            return (
                context,
                steps_out,
                stats,
                Err(RunError::MaxIterationsExceeded { limit: max_iter }),
            );
        }

        let step = match graph.step_for_state(&current_state) {
            Some(s) => s,
            None => {
                return (
                    context,
                    steps_out,
                    stats,
                    Err(RunError::NoOutgoingEdges {
                        step: format!("<branch {} terminal>", branch_name),
                        state: current_state,
                    }),
                );
            }
        };

        let state_before = current_state.clone();

        match execute_step(
            step,
            client,
            &mut context,
            scenario.auth.as_ref(),
            config,
            task_id,
            schema_cache,
        )
        .await
        {
            Ok(result) => {
                let edges = graph.outgoing_edges(&state_before);
                if edges.is_empty() {
                    return (
                        context,
                        steps_out,
                        stats,
                        Err(RunError::NoOutgoingEdges {
                            step: step.name.clone(),
                            state: state_before,
                        }),
                    );
                }

                let decision = evaluate_edges(
                    edges,
                    &result.response,
                    &result.assertion_results,
                    &mut edge_takes,
                    &mut rng,
                );
                let edge_idx = match decision.chosen {
                    Some(idx) => idx,
                    None => {
                        let capped_edge =
                            decision.evaluations.iter().find_map(|e| match &e.outcome {
                                EdgeOutcome::MaxTakesExceeded { limit } => {
                                    Some((e.to.clone(), *limit))
                                }
                                _ => None,
                            });
                        let mut synth = StepLog {
                            step_name: step.name.clone(),
                            state_before: state_before.clone(),
                            state_after: state_before.clone(),
                            method: step.method.as_str().to_string(),
                            url: result.url_sent,
                            status: result.response.status,
                            duration_ms: result.response.duration_ms,
                            assertions: result.assertion_results.clone(),
                            matched_edge_tag: None,
                            branch_path: Some(branch_path.clone()),
                            request_body: if config.verbose {
                                result.body_sent
                            } else {
                                None
                            },
                            response_body: if config.verbose {
                                Some(result.response.body.clone())
                            } else {
                                None
                            },
                            edge_evaluations: decision.evaluations,
                        };
                        apply_redaction(&mut synth, redactor);
                        steps_out.push(synth);
                        stats.total_steps += 1;
                        stats.failed += 1;
                        let err = match capped_edge {
                            Some((to, limit)) => RunError::EdgeMaxTakesExceeded {
                                state: state_before.clone(),
                                to,
                                limit,
                            },
                            None => RunError::NoMatchingTransition {
                                state: state_before,
                                status: result.response.status,
                            },
                        };
                        return (context, steps_out, stats, Err(err));
                    }
                };

                let matched_edge = edges[edge_idx];
                let evaluations = decision.evaluations;

                // Nested fan-out is validator-blocked (E015). If we encounter
                // one anyway, fail loudly — better than silent misbehavior.
                if matched_edge.parallel.is_some() {
                    return (
                        context,
                        steps_out,
                        stats,
                        Err(RunError::HttpError {
                            step: step.name.clone(),
                            message: "nested fan-out is not supported (validator E015)".into(),
                        }),
                    );
                }

                let next_state = matched_edge.to.clone();

                if let Some(delay) = matched_edge.after_ms
                    && delay > 0
                {
                    sleep(Duration::from_millis(delay)).await;
                }

                let mut branch_step = StepLog {
                    step_name: step.name.clone(),
                    state_before: state_before.clone(),
                    state_after: next_state.clone(),
                    method: step.method.as_str().to_string(),
                    url: result.url_sent,
                    status: result.response.status,
                    duration_ms: result.response.duration_ms,
                    assertions: result.assertion_results.clone(),
                    matched_edge_tag: matched_edge.tag.clone(),
                    branch_path: Some(branch_path.clone()),
                    request_body: if config.verbose {
                        result.body_sent
                    } else {
                        None
                    },
                    response_body: if config.verbose {
                        Some(result.response.body.clone())
                    } else {
                        None
                    },
                    edge_evaluations: evaluations,
                };
                apply_redaction(&mut branch_step, redactor);
                steps_out.push(branch_step);

                stats.total_steps += 1;
                if result.all_passed {
                    stats.passed += 1;
                } else {
                    stats.failed += 1;
                }

                current_state = next_state;
            }
            Err(RunError::Skipped { .. }) => {
                let edges = graph.outgoing_edges(&state_before);
                let next_state = match default_edge_target(edges) {
                    Some(t) => t,
                    None => {
                        return (
                            context,
                            steps_out,
                            stats,
                            Err(RunError::NoOutgoingEdges {
                                step: step.name.clone(),
                                state: state_before,
                            }),
                        );
                    }
                };
                current_state = next_state;
            }
            Err(e) => return (context, steps_out, stats, Err(e)),
        }
    }
}

pub async fn run(
    scenario: &Scenario,
    config: &RunConfig,
) -> Vec<(ExecutionLog, Result<String, RunError>)> {
    #[allow(deprecated)]
    let scenario_concurrency = scenario.concurrency;
    let concurrency = config.concurrency.or(scenario_concurrency).unwrap_or(1);
    let mut handles = Vec::new();

    for i in 1..=concurrency {
        let scenario = scenario.clone();
        let cfg = config.clone();
        handles.push(tokio::spawn(
            async move { run_once(&scenario, i, &cfg).await },
        ));
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("runner task panicked"));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_branches_namespaces_context_under_branch_name() {
        let mut parent: Context = HashMap::new();
        parent.insert("shared".into(), serde_json::json!("top"));

        let mut a: Context = HashMap::new();
        a.insert("id".into(), serde_json::json!(1));
        let mut b: Context = HashMap::new();
        b.insert("id".into(), serde_json::json!(2));

        merge_branches_into_parent(&mut parent, vec![("left".into(), a), ("right".into(), b)]);

        assert_eq!(parent.get("shared"), Some(&serde_json::json!("top")));
        let left = parent.get("left").and_then(|v| v.as_object()).unwrap();
        assert_eq!(left.get("id"), Some(&serde_json::json!(1)));
        let right = parent.get("right").and_then(|v| v.as_object()).unwrap();
        assert_eq!(right.get("id"), Some(&serde_json::json!(2)));
    }

    // --- Minimal in-process HTTP mock for fan-out integration tests -------
    // Each request shape hits a distinct URL; the server answers with a
    // canned JSON body. Not a full HTTP parser — just enough to drive the
    // executor against a real TcpListener.
    async fn spawn_mock(
        responses: std::sync::Arc<std::collections::HashMap<String, (u16, String)>>,
    ) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let responses = responses.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let (status, body) = responses
                        .get(&path)
                        .cloned()
                        .unwrap_or((404, "{\"error\":\"unknown\"}".into()));
                    let resp = format!(
                        "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        status,
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        port
    }

    fn scenario_yaml_with_fanout(port: u16, policy: &str) -> String {
        format!(
            r#"
name: fanout-test
initial_state: dispatch
terminal_states: [end]
steps:
  - name: kickoff
    state: dispatch
    method: GET
    url: "http://127.0.0.1:{port}/kickoff"
    extract:
      kickoff_id: "id"
  - name: left_call
    state: branch_left
    method: GET
    url: "http://127.0.0.1:{port}/left"
    extract:
      value: "value"
  - name: right_call
    state: branch_right
    method: GET
    url: "http://127.0.0.1:{port}/right"
    extract:
      value: "value"
  - name: done
    state: joined
    method: GET
    url: "http://127.0.0.1:{port}/done"
edges:
  - from: dispatch
    parallel:
      branches:
        - name: left
          to: branch_left
        - name: right
          to: branch_right
      join: joined
      on_failure: {policy}
  - from: branch_left
    to: joined
  - from: branch_right
    to: joined
  - from: joined
    to: end
"#
        )
    }

    #[tokio::test]
    async fn fan_out_happy_path_merges_namespaced_contexts() {
        let mut responses = std::collections::HashMap::new();
        responses.insert("/kickoff".into(), (200, r#"{"id":"abc"}"#.into()));
        responses.insert("/left".into(), (200, r#"{"value":"L"}"#.into()));
        responses.insert("/right".into(), (200, r#"{"value":"R"}"#.into()));
        responses.insert("/done".into(), (200, r#"{}"#.into()));
        let port = spawn_mock(std::sync::Arc::new(responses)).await;

        let yaml = scenario_yaml_with_fanout(port, "all_complete");
        let scenario: Scenario = serde_yaml::from_str(&yaml).unwrap();
        let config = RunConfig {
            seed: Some(1),
            ..RunConfig::default()
        };

        let (log, result) = run_once(&scenario, 0, &config).await;
        assert!(result.is_ok(), "run failed: {:?}", result.err());
        assert_eq!(log.total_steps, 4, "steps: {:#?}", log.steps);

        let left_tagged = log.steps.iter().any(|s| {
            s.branch_path
                .as_ref()
                .is_some_and(|p| p == &vec!["left".to_string()])
        });
        let right_tagged = log.steps.iter().any(|s| {
            s.branch_path
                .as_ref()
                .is_some_and(|p| p == &vec!["right".to_string()])
        });
        assert!(left_tagged && right_tagged, "branch_path not tagged");
    }

    #[tokio::test]
    async fn fan_out_fail_fast_surfaces_branch_error() {
        let mut responses = std::collections::HashMap::new();
        responses.insert("/kickoff".into(), (200, r#"{"id":"abc"}"#.into()));
        responses.insert("/left".into(), (200, r#"{"value":"L"}"#.into()));
        responses.insert("/right".into(), (200, r#"{}"#.into()));
        responses.insert("/done".into(), (200, r#"{}"#.into()));
        let port = spawn_mock(std::sync::Arc::new(responses)).await;

        let yaml = scenario_yaml_with_fanout(port, "fail_fast");
        let scenario: Scenario = serde_yaml::from_str(&yaml).unwrap();
        let config = RunConfig {
            seed: Some(1),
            strict_extract: true,
            ..RunConfig::default()
        };

        let (_log, result) = run_once(&scenario, 0, &config).await;
        assert!(
            matches!(result, Err(RunError::ExtractionMissing { .. })),
            "expected ExtractionMissing, got {:?}",
            result
        );
    }
}
