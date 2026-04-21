use ace_core::{
    assertions::{self, AssertionResult},
    graph::Graph,
    jsonpath,
    variables::{self, Context, resolve_template, value_to_string},
};
use ace_http::{
    Client, ClientConfig, HttpResponse, MultipartField, MultipartValue, RequestOptions,
    build_client, send_request,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use model::{
    AssertionMatch, Auth, Edge, FailurePolicy, FanOut, Hook, Scenario, StatusMatch, Step,
    TransitionCondition,
};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

#[derive(Debug)]
pub enum RunError {
    InvalidTransition {
        step: String,
        expected: String,
        actual: String,
    },
    HttpError {
        step: String,
        message: String,
    },
    AssertionFailed {
        step: String,
        failures: Vec<AssertionResult>,
    },
    Skipped {
        step: String,
        reason: String,
    },
    NoMatchingTransition {
        state: String,
        status: u16,
    },
    NoOutgoingEdges {
        step: String,
        state: String,
    },
    MaxIterationsExceeded {
        limit: u64,
    },
    ExtractionMissing {
        step: String,
        key: String,
        path: String,
    },
    EdgeMaxTakesExceeded {
        state: String,
        to: String,
        limit: u32,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::InvalidTransition {
                step,
                expected,
                actual,
            } => write!(
                f,
                "Step '{}': expected state '{}', but current state is '{}'",
                step, expected, actual
            ),
            RunError::HttpError { step, message } => {
                write!(f, "Step '{}': HTTP error: {}", step, message)
            }
            RunError::AssertionFailed { step, failures } => {
                write!(f, "Step '{}': {} assertion(s) failed", step, failures.len())
            }
            RunError::Skipped { step, reason } => {
                write!(f, "Step '{}': skipped ({})", step, reason)
            }
            RunError::NoMatchingTransition { state, status } => {
                write!(
                    f,
                    "State '{}': no matching transition for status {}",
                    state, status
                )
            }
            RunError::NoOutgoingEdges { step, state } => {
                write!(
                    f,
                    "Step '{}': state '{}' has no outgoing edges — explicit graphs require every state to transition",
                    step, state
                )
            }
            RunError::MaxIterationsExceeded { limit } => {
                write!(f, "Max iterations exceeded (limit: {})", limit)
            }
            RunError::ExtractionMissing { step, key, path } => {
                write!(
                    f,
                    "Step '{}': extraction '{}' failed — JSONPath '{}' did not resolve in response body",
                    step, key, path
                )
            }
            RunError::EdgeMaxTakesExceeded { state, to, limit } => {
                write!(
                    f,
                    "State '{}': edge to '{}' exceeded max_takes ({})",
                    state, to, limit
                )
            }
        }
    }
}

impl std::error::Error for RunError {}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ExecutionLog {
    pub steps: Vec<StepLog>,
    pub total_duration_ms: u64,
    pub total_steps: usize,
    pub passed: usize,
    pub failed: usize,
    #[serde(default)]
    pub iterations: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<String>,
    /// Per-task RNG seed. Populated on every run so weighted-routing outcomes
    /// can be reproduced by passing `--seed <value>` (with matching concurrency).
    #[serde(default)]
    pub seed: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct StepLog {
    pub step_name: String,
    pub state_before: String,
    pub state_after: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub duration_ms: u64,
    pub assertions: Vec<AssertionResult>,
    /// Tag of the edge that fired the transition (if set on the edge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_edge_tag: Option<String>,
    /// Ordered list of branch names this step executed under (outermost first).
    /// None/empty = main line. Lets reporters group fan-out branch steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_path: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RunConfig {
    pub cli_variables: HashMap<String, String>,
    pub verbose: bool,
    pub insecure: bool,
    pub proxy: Option<String>,
    /// CLI-supplied concurrency override. Takes precedence over the deprecated
    /// `scenario.concurrency` field.
    pub concurrency: Option<usize>,
    /// When true, a missing JSONPath in an `extract:` block causes the step to fail
    /// instead of silently leaving the variable undefined.
    pub strict_extract: bool,
    /// Base RNG seed for weighted edge routing. When unset, a random seed is
    /// generated per run and echoed via `ExecutionLog.seed` for replay.
    pub seed: Option<u64>,
}

async fn fetch_oauth2_token(
    client: &Client,
    oauth: &model::OAuth2Config,
    context: &Context,
) -> Result<String, String> {
    let token_url = resolve_template(&oauth.token_url, context);
    let client_id = resolve_template(&oauth.client_id, context);
    let client_secret = resolve_template(&oauth.client_secret, context);
    let grant_type = oauth.grant_type.as_deref().unwrap_or("client_credentials");

    let body = {
        let mut params = form_urlencoded::Serializer::new(String::new());
        params.append_pair("grant_type", grant_type);
        params.append_pair("client_id", &client_id);
        params.append_pair("client_secret", &client_secret);
        if let Some(scope) = &oauth.scope {
            params.append_pair("scope", &resolve_template(scope, context));
        }
        params.finish()
    };

    let mut headers = HashMap::new();
    headers.insert(
        "Content-Type".into(),
        "application/x-www-form-urlencoded".into(),
    );

    let opts = RequestOptions {
        headers,
        body: Some(body),
        timeout_ms: Some(30000),
        multipart: None,
    };

    let response = send_request(client, "POST", &token_url, &opts)
        .await
        .map_err(|e| format!("OAuth2 token request failed: {}", e))?;

    if response.status != 200 {
        return Err(format!(
            "OAuth2 token endpoint returned {}: {}",
            response.status, response.body
        ));
    }

    let json: serde_json::Value = serde_json::from_str(&response.body)
        .map_err(|e| format!("OAuth2 response parse failed: {}", e))?;

    json.get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "OAuth2 response missing 'access_token' field".to_string())
}

async fn execute_hooks(
    hooks: &[Hook],
    context: &mut Context,
    task_id: usize,
    step_name: &str,
    phase: &str,
) -> Option<String> {
    for hook in hooks {
        if let Some(condition) = &hook.skip_if {
            let resolved = resolve_template(condition, context);
            if resolved == "true" || resolved == "1" {
                return Some(format!("skip_if: {}", condition));
            }
        }

        if let Some(vars) = &hook.set {
            for (key, value_template) in vars {
                let value = resolve_template(value_template, context);
                debug!(
                    task_id,
                    step = step_name,
                    phase,
                    key = key.as_str(),
                    value = value.as_str(),
                    "Hook set"
                );
                context.insert(key.clone(), serde_json::Value::String(value));
            }
        }

        if let Some(delay) = hook.delay_ms {
            debug!(task_id, step = step_name, phase, delay, "Hook delay");
            sleep(Duration::from_millis(delay)).await;
        }

        if let Some(msg) = &hook.log {
            let resolved = resolve_template(msg, context);
            info!(
                task_id,
                step = step_name,
                phase,
                message = resolved.as_str(),
                "Hook log"
            );
        }
    }
    None
}

fn evaluate_edges(
    edges: &[&Edge],
    response: &HttpResponse,
    assertion_results: &[AssertionResult],
    current_state: &str,
    take_counts: &mut HashMap<usize, u32>,
    rng: &mut StdRng,
) -> Result<usize, RunError> {
    let commit = |idx: usize, counts: &mut HashMap<usize, u32>| -> Result<usize, RunError> {
        let edge = edges[idx];
        if let Some(limit) = edge.max_takes {
            let key = edge as *const Edge as usize;
            let count = counts.entry(key).or_insert(0);
            if *count >= limit {
                return Err(RunError::EdgeMaxTakesExceeded {
                    state: edge.from.clone(),
                    to: edge.to.clone(),
                    limit,
                });
            }
            *count += 1;
        }
        Ok(idx)
    };

    // Collapse a candidate list to the highest-priority tier, then either
    // weighted-sample (all have weights) or take the first in list order.
    let choose = |candidates: Vec<usize>, rng: &mut StdRng| -> Option<usize> {
        if candidates.is_empty() {
            return None;
        }
        let max_pri = candidates
            .iter()
            .map(|&i| edges[i].priority.unwrap_or(0))
            .max()
            .unwrap();
        let top: Vec<usize> = candidates
            .into_iter()
            .filter(|&i| edges[i].priority.unwrap_or(0) == max_pri)
            .collect();

        if top.len() == 1 {
            return Some(top[0]);
        }
        let all_weighted = top.iter().all(|&i| edges[i].weight.is_some());
        if !all_weighted {
            return Some(top[0]);
        }
        let total: u64 = top.iter().map(|&i| edges[i].weight.unwrap() as u64).sum();
        if total == 0 {
            return Some(top[0]);
        }
        let mut roll = rng.gen_range(0..total);
        for &i in &top {
            let w = edges[i].weight.unwrap() as u64;
            if w > roll {
                return Some(i);
            }
            roll -= w;
        }
        Some(top[top.len() - 1])
    };

    // Pass 1: conditional matches.
    let cond: Vec<usize> = (0..edges.len())
        .filter(|&i| {
            edges[i]
                .when
                .as_ref()
                .is_some_and(|c| matches_condition(c, response, assertion_results))
        })
        .collect();
    if let Some(idx) = choose(cond, rng) {
        return commit(idx, take_counts);
    }

    // Pass 2: explicit defaults.
    let explicit: Vec<usize> = (0..edges.len())
        .filter(|&i| edges[i].when.is_none() && edges[i].default.unwrap_or(false))
        .collect();
    if let Some(idx) = choose(explicit, rng) {
        return commit(idx, take_counts);
    }

    // Pass 3: implicit unconditional.
    let implicit: Vec<usize> = (0..edges.len())
        .filter(|&i| edges[i].when.is_none() && !edges[i].default.unwrap_or(false))
        .collect();
    if let Some(idx) = choose(implicit, rng) {
        return commit(idx, take_counts);
    }

    Err(RunError::NoMatchingTransition {
        state: current_state.to_string(),
        status: response.status,
    })
}

fn matches_condition(
    condition: &TransitionCondition,
    response: &HttpResponse,
    assertion_results: &[AssertionResult],
) -> bool {
    if let Some(status_match) = &condition.status {
        let matches = match status_match {
            StatusMatch::Exact(code) => response.status == *code,
            StatusMatch::Complex(vc) => {
                let val = serde_json::Value::Number(serde_json::Number::from(response.status));
                ace_core::assertions::eval_value_check(vc, Some(&val), &response.status.to_string())
            }
        };
        if !matches {
            return false;
        }
    }

    if let Some(body_checks) = &condition.body {
        let json: Option<serde_json::Value> = serde_json::from_str(&response.body).ok();
        for (path, check) in body_checks {
            let resolved = json.as_ref().and_then(|j| jsonpath::resolve(j, path));
            let actual_str = resolved
                .as_ref()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            if !ace_core::assertions::eval_value_check(check, resolved.as_ref(), &actual_str) {
                return false;
            }
        }
    }

    if let Some(assertion_match) = &condition.assertions {
        let all_passed = assertion_results.iter().all(|a| a.passed);
        match assertion_match {
            AssertionMatch::Passed => {
                if !all_passed {
                    return false;
                }
            }
            AssertionMatch::Failed => {
                if all_passed {
                    return false;
                }
            }
        }
    }

    true
}

struct StepResult {
    response: HttpResponse,
    assertion_results: Vec<AssertionResult>,
    all_passed: bool,
    body_sent: Option<String>,
    url_sent: String,
}

async fn execute_step(
    step: &Step,
    client: &Client,
    context: &mut Context,
    scenario_auth: Option<&Auth>,
    _config: &RunConfig,
    task_id: usize,
) -> Result<StepResult, RunError> {
    if let Some(hooks) = &step.pre_request
        && let Some(skip_reason) = execute_hooks(hooks, context, task_id, &step.name, "pre").await
    {
        return Err(RunError::Skipped {
            step: step.name.clone(),
            reason: skip_reason,
        });
    }

    let url = resolve_template(&step.url, context);
    let mut req_headers = HashMap::new();

    if let Some(auth) = scenario_auth {
        apply_auth(auth, &mut req_headers, context);
    }

    if let Some(headers) = &step.headers {
        for (k, v) in headers {
            req_headers.insert(k.clone(), resolve_template(v, context));
        }
    }

    let body = step.body.as_ref().map(|b| {
        let json_str = serde_json::to_string(b).expect("scenario body should always serialize");
        resolve_template(&json_str, context)
    });

    let multipart = step.multipart.as_ref().map(|fields| {
        fields
            .iter()
            .map(|f| MultipartField {
                name: resolve_template(&f.name, context),
                value: if let Some(file_path) = &f.file {
                    MultipartValue::File {
                        path: resolve_template(file_path, context),
                        filename: f.filename.as_ref().map(|n| resolve_template(n, context)),
                        mime: f.mime.clone(),
                    }
                } else {
                    MultipartValue::Text(resolve_template(
                        f.value.as_deref().unwrap_or(""),
                        context,
                    ))
                },
            })
            .collect()
    });

    if body.is_some()
        && multipart.is_none()
        && !req_headers.contains_key("Content-Type")
        && !req_headers.contains_key("content-type")
    {
        req_headers.insert("Content-Type".into(), "application/json".into());
    }

    let opts = RequestOptions {
        headers: req_headers,
        body: body.clone(),
        timeout_ms: step.timeout_ms,
        multipart,
    };

    let max_attempts = step.retry.as_ref().map_or(1, |r| r.attempts);
    let delay_ms = step.retry.as_ref().map_or(0, |r| r.delay_ms);
    let mut last_err: Option<String> = None;
    let method_str = step.method.as_str();

    for attempt in 1..=max_attempts {
        if attempt > 1 {
            info!(
                task_id,
                step = step.name.as_str(),
                attempt,
                max_attempts,
                "Retrying"
            );
            sleep(Duration::from_millis(delay_ms)).await;
        }

        match send_request(client, method_str, &url, &opts).await {
            Ok(response) => {
                let success = response.status >= 200 && response.status < 400;
                if success || attempt == max_attempts {
                    let assertion_results = if let Some(asserts) = &step.assertions {
                        assertions::evaluate(asserts, &response)
                    } else {
                        Vec::new()
                    };

                    let all_passed = assertion_results.iter().all(|a| a.passed);

                    if let Some(extract) = &step.extract {
                        extract_context(
                            extract,
                            &response.body,
                            context,
                            task_id,
                            &step.name,
                            _config.strict_extract,
                        )?;
                    }

                    if let Some(hooks) = &step.post_request {
                        execute_hooks(hooks, context, task_id, &step.name, "post").await;
                    }

                    return Ok(StepResult {
                        response,
                        assertion_results,
                        all_passed,
                        body_sent: body.clone(),
                        url_sent: url.clone(),
                    });
                }

                warn!(
                    task_id,
                    step = step.name.as_str(),
                    status = response.status,
                    attempt,
                    "Non-success status, will retry"
                );
                last_err = Some(format!("status {}", response.status));
            }
            Err(e) => {
                error!(
                    task_id,
                    step = step.name.as_str(),
                    error = e.as_str(),
                    attempt,
                    "Request failed"
                );
                last_err = Some(e);
            }
        }
    }

    Err(RunError::HttpError {
        step: step.name.clone(),
        message: last_err.unwrap_or_else(|| "unknown error".into()),
    })
}

fn apply_auth(auth: &Auth, headers: &mut HashMap<String, String>, context: &Context) {
    if let Some(bearer) = &auth.bearer {
        let token = resolve_template(bearer, context);
        headers
            .entry("Authorization".into())
            .or_insert_with(|| format!("Bearer {}", token));
    }
    if let Some(basic) = &auth.basic {
        let user = resolve_template(&basic.username, context);
        let pass = resolve_template(&basic.password, context);
        let encoded = BASE64.encode(format!("{}:{}", user, pass));
        headers
            .entry("Authorization".into())
            .or_insert_with(|| format!("Basic {}", encoded));
    }
    if let Some(api_key) = &auth.api_key {
        let header = resolve_template(&api_key.header, context);
        let value = resolve_template(&api_key.value, context);
        headers.entry(header).or_insert(value);
    }
    if auth.oauth2.is_some()
        && let Some(token) = context.get("$oauth_token").map(value_to_string)
    {
        headers
            .entry("Authorization".into())
            .or_insert_with(|| format!("Bearer {}", token));
    }
}

fn extract_context(
    extract: &HashMap<String, model::ExtractSpec>,
    body: &str,
    context: &mut Context,
    task_id: usize,
    step_name: &str,
    global_strict: bool,
) -> Result<(), RunError> {
    let json: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            let any_required = extract.values().any(|s| s.is_required(global_strict));
            if any_required {
                return Err(RunError::ExtractionMissing {
                    step: step_name.to_string(),
                    key: "<response>".into(),
                    path: format!("response body is not valid JSON: {e}"),
                });
            }
            warn!(
                task_id,
                step = step_name,
                error = %e,
                "Response body is not valid JSON; skipping all extract: paths"
            );
            return Ok(());
        }
    };

    for (context_key, spec) in extract {
        let json_path = spec.path();
        if let Some(value) = jsonpath::resolve(&json, json_path) {
            debug!(
                task_id,
                step = step_name,
                key = context_key.as_str(),
                value = %variables::value_to_string(&value),
                "Extracted"
            );
            context.insert(context_key.clone(), value);
        } else if spec.is_required(global_strict) {
            return Err(RunError::ExtractionMissing {
                step: step_name.to_string(),
                key: context_key.clone(),
                path: json_path.to_string(),
            });
        } else {
            warn!(
                task_id,
                step = step_name,
                path = json_path,
                "Extraction path not found"
            );
        }
    }

    Ok(())
}

fn default_edge_target(edges: &[&Edge]) -> Option<String> {
    edges
        .iter()
        .find(|edge| edge.default.unwrap_or(false))
        .map(|edge| edge.to.clone())
        .or_else(|| edges.first().map(|edge| edge.to.clone()))
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

                let edge_idx = match evaluate_edges(
                    edges,
                    &result.response,
                    &result.assertion_results,
                    &state_before,
                    &mut edge_takes,
                    &mut rng,
                ) {
                    Ok(idx) => idx,
                    Err(e) => {
                        log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                        let owned_log = std::mem::take(&mut log);
                        return (owned_log, Err(e));
                    }
                };
                let matched_edge = edges[edge_idx];

                if let Some(fan_out) = &matched_edge.parallel {
                    // Log the dispatch step itself before running branches.
                    log.steps.push(StepLog {
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
                    });
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

                log.steps.push(StepLog {
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
                });

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

                let edge_idx = match evaluate_edges(
                    edges,
                    &result.response,
                    &result.assertion_results,
                    &state_before,
                    &mut edge_takes,
                    &mut rng,
                ) {
                    Ok(idx) => idx,
                    Err(e) => return (context, steps_out, stats, Err(e)),
                };

                let matched_edge = edges[edge_idx];

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

                steps_out.push(StepLog {
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
                });

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
    use model::{Edge, ValueCheck};

    fn pick_to(edges: &[&Edge], resp: &HttpResponse) -> String {
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);
        let idx = evaluate_edges(edges, resp, &[], "start", &mut counts, &mut rng).unwrap();
        edges[idx].to.clone()
    }

    fn resp(status: u16) -> HttpResponse {
        HttpResponse {
            status,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 1,
        }
    }

    #[test]
    fn evaluate_edges_uses_default_fallback() {
        let edges = [Edge {
            from: "start".into(),
            to: "done".into(),
            default: Some(true),
            ..Edge::default()
        }];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(200)), "done");
    }

    #[test]
    fn evaluate_edges_matches_status_rule() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "retry".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Complex(ValueCheck {
                        gt: Some(399.0),
                        ..ValueCheck::default()
                    })),
                    body: None,
                    assertions: None,
                }),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "done".into(),
                default: Some(true),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(500)), "retry");
    }

    #[test]
    fn evaluate_edges_treats_unconditional_as_implicit_default() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "fallback".into(),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "retry".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(500)),
                    body: None,
                    assertions: None,
                }),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(500)), "retry");
        assert_eq!(pick_to(&refs, &resp(200)), "fallback");
    }

    #[test]
    fn evaluate_edges_explicit_default_beats_implicit() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "implicit".into(),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "explicit".into(),
                default: Some(true),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(200)), "explicit");
    }

    #[test]
    fn evaluate_edges_respects_priority() {
        // Two conditional edges both match status 500; higher priority wins
        // regardless of list order.
        let edges = [
            Edge {
                from: "start".into(),
                to: "low_pri".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(500)),
                    body: None,
                    assertions: None,
                }),
                priority: Some(1),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "high_pri".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(500)),
                    body: None,
                    assertions: None,
                }),
                priority: Some(10),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(500)), "high_pri");
    }

    #[test]
    fn evaluate_edges_enforces_max_takes() {
        let edges = [Edge {
            from: "start".into(),
            to: "retry".into(),
            default: Some(true),
            max_takes: Some(2),
            ..Edge::default()
        }];
        let refs: Vec<&Edge> = edges.iter().collect();
        let mut counts = HashMap::new();
        let mut rng = StdRng::seed_from_u64(0);
        assert!(evaluate_edges(&refs, &resp(200), &[], "start", &mut counts, &mut rng).is_ok());
        assert!(evaluate_edges(&refs, &resp(200), &[], "start", &mut counts, &mut rng).is_ok());
        let third = evaluate_edges(&refs, &resp(200), &[], "start", &mut counts, &mut rng);
        assert!(matches!(
            third,
            Err(RunError::EdgeMaxTakesExceeded { limit: 2, .. })
        ));
    }

    #[test]
    fn evaluate_edges_weighted_is_deterministic_per_seed() {
        // Two implicit unconditional edges with weights 70/30. With the same
        // seed we must always pick the same edge; distribution over many rolls
        // should roughly match weights.
        let edges = [
            Edge {
                from: "start".into(),
                to: "a".into(),
                weight: Some(70),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "b".into(),
                weight: Some(30),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();

        let mut rng1 = StdRng::seed_from_u64(42);
        let mut rng2 = StdRng::seed_from_u64(42);
        let mut counts1 = HashMap::new();
        let mut counts2 = HashMap::new();
        let first = evaluate_edges(&refs, &resp(200), &[], "start", &mut counts1, &mut rng1);
        let second = evaluate_edges(&refs, &resp(200), &[], "start", &mut counts2, &mut rng2);
        assert_eq!(first.unwrap(), second.unwrap());

        // Roughly 70/30 distribution over 1000 rolls; allow ±10%.
        let mut rng = StdRng::seed_from_u64(123);
        let mut hits_a = 0;
        for _ in 0..1000 {
            let mut counts = HashMap::new();
            let idx =
                evaluate_edges(&refs, &resp(200), &[], "start", &mut counts, &mut rng).unwrap();
            if edges[idx].to == "a" {
                hits_a += 1;
            }
        }
        assert!(
            (600..=800).contains(&hits_a),
            "expected ~700 hits on 'a', got {hits_a}"
        );
    }

    #[test]
    fn evaluate_edges_weighted_zero_total_falls_back_to_first() {
        let edges = [
            Edge {
                from: "start".into(),
                to: "a".into(),
                weight: Some(0),
                ..Edge::default()
            },
            Edge {
                from: "start".into(),
                to: "b".into(),
                weight: Some(0),
                ..Edge::default()
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        assert_eq!(pick_to(&refs, &resp(200)), "a");
    }

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
        // 4 visible steps: kickoff + 2 branch steps + done.
        assert_eq!(log.total_steps, 4, "steps: {:#?}", log.steps);

        // Branch steps carry branch_path tags.
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
        // /right returns an empty JSON body — the required extract will miss
        // under strict_extract, producing a RunError::ExtractionMissing.
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
