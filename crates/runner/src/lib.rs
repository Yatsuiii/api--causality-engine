use ace_core::{
    assertions::{self, AssertionResult},
    jsonpath,
    variables::{self, resolve_template},
};
use ace_http::{
    Client, ClientConfig, HttpResponse, MultipartField, MultipartValue, RequestOptions,
    build_client, send_request,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use model::{AssertionMatch, Auth, Edge, Hook, Scenario, StatusMatch, Step, TransitionCondition};
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
}

async fn fetch_oauth2_token(
    client: &Client,
    oauth: &model::OAuth2Config,
    context: &HashMap<String, String>,
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
    context: &mut HashMap<String, String>,
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
                context.insert(key.clone(), value);
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
) -> Result<String, RunError> {
    for edge in edges {
        if let Some(condition) = &edge.when {
            if matches_condition(condition, response, assertion_results) {
                return Ok(edge.to.clone());
            }
        } else if edge.default.unwrap_or(false) {
            continue;
        } else {
            return Ok(edge.to.clone());
        }
    }

    for edge in edges {
        if edge.default.unwrap_or(false) {
            return Ok(edge.to.clone());
        }
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
    context: &mut HashMap<String, String>,
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
                        extract_context(extract, &response.body, context, task_id, &step.name)?;
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

fn apply_auth(
    auth: &Auth,
    headers: &mut HashMap<String, String>,
    context: &HashMap<String, String>,
) {
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
        && let Some(token) = context.get("$oauth_token")
    {
        headers
            .entry("Authorization".into())
            .or_insert_with(|| format!("Bearer {}", token));
    }
}

fn extract_context(
    extract: &HashMap<String, String>,
    body: &str,
    context: &mut HashMap<String, String>,
    task_id: usize,
    step_name: &str,
) -> Result<(), RunError> {
    let json: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                task_id,
                step = step_name,
                error = %e,
                "Response body is not valid JSON; skipping all extract: paths"
            );
            return Ok(());
        }
    };

    for (context_key, json_path) in extract {
        if let Some(value) = jsonpath::extract_string(&json, json_path) {
            debug!(
                task_id,
                step = step_name,
                key = context_key.as_str(),
                value = value.as_str(),
                "Extracted"
            );
            context.insert(context_key.clone(), value);
        } else {
            warn!(
                task_id,
                step = step_name,
                path = json_path.as_str(),
                "Extraction path not found"
            );
        }
    }

    Ok(())
}

fn outgoing_edges<'a>(scenario: &'a Scenario, state: &str) -> Vec<&'a Edge> {
    scenario
        .edges
        .iter()
        .filter(|edge| edge.from == state)
        .collect()
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
                context.insert("$oauth_token".into(), token);
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

    let step_map: HashMap<String, &Step> = scenario
        .steps
        .iter()
        .map(|s| (s.state.clone(), s))
        .collect();

    let run_start = std::time::Instant::now();
    let mut current_state = scenario.initial_state.clone();
    let max_iter = scenario.max_iterations.unwrap_or(100);

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

        let step = match step_map.get(&current_state) {
            Some(step) => *step,
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
                let edges = outgoing_edges(scenario, &state_before);
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

                let next_state = match evaluate_edges(
                    &edges,
                    &result.response,
                    &result.assertion_results,
                    &state_before,
                ) {
                    Ok(next_state) => next_state,
                    Err(e) => {
                        log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                        let owned_log = std::mem::take(&mut log);
                        return (owned_log, Err(e));
                    }
                };

                log.steps.push(StepLog {
                    step_name: step.name.clone(),
                    state_before: state_before.clone(),
                    state_after: next_state.clone(),
                    method: step.method.as_str().to_string(),
                    url: result.url_sent,
                    status: result.response.status,
                    duration_ms: result.response.duration_ms,
                    assertions: result.assertion_results.clone(),
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
                let edges = outgoing_edges(scenario, &state_before);
                let next_state = match default_edge_target(&edges) {
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

    #[test]
    fn evaluate_edges_uses_default_fallback() {
        let edges = [Edge {
            from: "start".into(),
            to: "done".into(),
            when: None,
            default: Some(true),
        }];
        let refs: Vec<&Edge> = edges.iter().collect();
        let response = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 1,
        };

        let next = evaluate_edges(&refs, &response, &[], "start").unwrap();
        assert_eq!(next, "done");
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
                default: None,
            },
            Edge {
                from: "start".into(),
                to: "done".into(),
                when: None,
                default: Some(true),
            },
        ];
        let refs: Vec<&Edge> = edges.iter().collect();
        let response = HttpResponse {
            status: 500,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 1,
        };

        let next = evaluate_edges(&refs, &response, &[], "start").unwrap();
        assert_eq!(next, "retry");
    }
}
