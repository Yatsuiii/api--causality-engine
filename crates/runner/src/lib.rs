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
use model::{
    AssertionMatch, Auth, Hook, Scenario, StatusMatch, Step, TransitionCondition, TransitionEdge,
};
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

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
            RunError::MaxIterationsExceeded { limit } => {
                write!(f, "Max iterations exceeded (limit: {})", limit)
            }
        }
    }
}

impl std::error::Error for RunError {}

// ---------------------------------------------------------------------------
// Execution log types
// ---------------------------------------------------------------------------

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
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

// ---------------------------------------------------------------------------
// Run configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct RunConfig {
    pub cli_variables: HashMap<String, String>,
    pub verbose: bool,
    pub insecure: bool,
    pub proxy: Option<String>,
}

// ---------------------------------------------------------------------------
// OAuth2 token fetcher
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Hook executor
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Transition evaluation (graph mode)
// ---------------------------------------------------------------------------

/// Evaluate transition edges and return the next state.
/// First conditional match wins; falls back to default edge.
fn evaluate_transitions(
    edges: &[TransitionEdge],
    response: &HttpResponse,
    assertion_results: &[AssertionResult],
    current_state: &str,
) -> Result<String, RunError> {
    // First pass: check conditional edges in declaration order
    for edge in edges {
        if let Some(condition) = &edge.when {
            if matches_condition(condition, response, assertion_results) {
                return Ok(edge.to.clone());
            }
        } else if edge.default.unwrap_or(false) {
            // Skip default edges in first pass
            continue;
        } else {
            // Unconditional non-default edge — always matches
            return Ok(edge.to.clone());
        }
    }

    // Second pass: find the default edge
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

/// Check if a transition condition matches the response.
/// AND semantics: all specified fields must match.
fn matches_condition(
    condition: &TransitionCondition,
    response: &HttpResponse,
    assertion_results: &[AssertionResult],
) -> bool {
    // Status check
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

    // Body checks
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

    // Assertion outcome check
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

// ---------------------------------------------------------------------------
// Step execution helper
// ---------------------------------------------------------------------------

struct StepResult {
    response: HttpResponse,
    assertion_results: Vec<AssertionResult>,
    all_passed: bool,
    body_sent: Option<String>,
}

async fn execute_step(
    step: &Step,
    client: &Client,
    context: &mut HashMap<String, String>,
    scenario_auth: Option<&Auth>,
    _config: &RunConfig,
    task_id: usize,
) -> Result<StepResult, RunError> {
    // Execute pre-request hooks
    if let Some(hooks) = &step.pre_request
        && let Some(skip_reason) = execute_hooks(hooks, context, task_id, &step.name, "pre").await
    {
        return Err(RunError::Skipped {
            step: step.name.clone(),
            reason: skip_reason,
        });
    }

    // Resolve URL
    let url = resolve_template(&step.url, context);

    // Build request headers
    let mut req_headers = HashMap::new();

    if let Some(auth) = scenario_auth {
        apply_auth(auth, &mut req_headers, context);
    }

    if let Some(headers) = &step.headers {
        for (k, v) in headers {
            req_headers.insert(k.clone(), resolve_template(v, context));
        }
    }

    // Build request body
    let body = step.body.as_ref().map(|b| {
        let json_str = serde_json::to_string(b).expect(
            "step body is a serde_yaml::Value that parsed cleanly — serialization cannot fail",
        );
        resolve_template(&json_str, context)
    });

    // Build multipart fields
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

    // Auto-set Content-Type for JSON bodies (not for multipart)
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

    // Retry loop
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
                    debug!(
                        task_id,
                        step = step.name.as_str(),
                        status = response.status,
                        duration_ms = response.duration_ms,
                        attempt,
                        "Request completed"
                    );

                    // Run assertions
                    let assertion_results = if let Some(asserts) = &step.assertions {
                        assertions::evaluate(asserts, &response)
                    } else {
                        Vec::new()
                    };

                    let all_passed = assertion_results.iter().all(|a| a.passed);

                    // Extract context values
                    if let Some(extract) = &step.extract {
                        extract_context(extract, &response.body, context, task_id, &step.name)?;
                    }

                    // Execute post-request hooks
                    if let Some(hooks) = &step.post_request {
                        execute_hooks(hooks, context, task_id, &step.name, "post").await;
                    }

                    return Ok(StepResult {
                        response,
                        assertion_results,
                        all_passed,
                        body_sent: body.clone(),
                    });
                } else {
                    warn!(
                        task_id,
                        step = step.name.as_str(),
                        status = response.status,
                        attempt,
                        "Non-success status, will retry"
                    );
                    last_err = Some(format!("status {}", response.status));
                }
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

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

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

    let mut log = ExecutionLog {
        steps: Vec::new(),
        total_duration_ms: 0,
        total_steps: 0,
        passed: 0,
        failed: 0,
        iterations: 0,
        terminal_state: None,
    };

    // OAuth2: fetch token before execution if configured
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

    let run_start = std::time::Instant::now();

    let graph_mode = ace_core::validate::is_graph_mode(scenario);

    if graph_mode {
        run_graph_mode(
            scenario,
            &client,
            &mut context,
            &mut log,
            config,
            task_id,
            run_start,
        )
        .await
    } else {
        run_linear_mode(
            scenario,
            &client,
            &mut context,
            &mut log,
            config,
            task_id,
            run_start,
        )
        .await
    }
}

async fn run_linear_mode(
    scenario: &Scenario,
    client: &Client,
    context: &mut HashMap<String, String>,
    log: &mut ExecutionLog,
    config: &RunConfig,
    task_id: usize,
    run_start: std::time::Instant,
) -> (ExecutionLog, Result<String, RunError>) {
    let mut current_state = scenario.initial_state.clone();

    for step in &scenario.steps {
        let transition = step.transition.as_ref().expect(
            "run_linear_mode is only called after validate_scenario confirms linear layout",
        );

        // Validate state transition
        if transition.from != current_state {
            log.total_duration_ms = run_start.elapsed().as_millis() as u64;
            return (
                std::mem::take(log),
                Err(RunError::InvalidTransition {
                    step: step.name.clone(),
                    expected: transition.from.clone(),
                    actual: current_state,
                }),
            );
        }

        match execute_step(
            step,
            client,
            context,
            scenario.auth.as_ref(),
            config,
            task_id,
        )
        .await
        {
            Ok(result) => {
                let step_log = StepLog {
                    step_name: step.name.clone(),
                    state_before: transition.from.clone(),
                    state_after: transition.to.clone(),
                    method: step.method.as_str().to_string(),
                    url: resolve_template(&step.url, context),
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
                };

                log.steps.push(step_log);
                log.total_steps += 1;
                if result.all_passed {
                    log.passed += 1;
                } else {
                    log.failed += 1;
                }

                if !result.all_passed {
                    let failures: Vec<_> = result
                        .assertion_results
                        .into_iter()
                        .filter(|a| !a.passed)
                        .collect();
                    log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                    let owned_log = std::mem::take(log);
                    return (
                        owned_log,
                        Err(RunError::AssertionFailed {
                            step: step.name.clone(),
                            failures,
                        }),
                    );
                }

                current_state = transition.to.clone();
            }
            Err(RunError::Skipped { .. }) => {
                debug!(task_id, step = step.name.as_str(), "Step skipped");
                current_state = transition.to.clone();
                continue;
            }
            Err(e) => {
                log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                let owned_log = std::mem::take(log);
                return (owned_log, Err(e));
            }
        }
    }

    log.total_duration_ms = run_start.elapsed().as_millis() as u64;
    (std::mem::take(log), Ok(current_state))
}

async fn run_graph_mode(
    scenario: &Scenario,
    client: &Client,
    context: &mut HashMap<String, String>,
    log: &mut ExecutionLog,
    config: &RunConfig,
    task_id: usize,
    run_start: std::time::Instant,
) -> (ExecutionLog, Result<String, RunError>) {
    // Build step lookup by state name
    let step_map: HashMap<String, &Step> = scenario
        .steps
        .iter()
        .map(|s| (s.state_name().to_string(), s))
        .collect();

    let mut current_state = scenario.initial_state.clone();
    let max_iter = scenario.max_iterations.unwrap_or(100);

    loop {
        log.iterations += 1;
        if log.iterations > max_iter {
            log.total_duration_ms = run_start.elapsed().as_millis() as u64;
            let owned_log = std::mem::take(log);
            return (
                owned_log,
                Err(RunError::MaxIterationsExceeded { limit: max_iter }),
            );
        }

        // Look up step for current state; if none, it's a terminal state
        let step = match step_map.get(&current_state) {
            Some(s) => *s,
            None => {
                log.terminal_state = Some(current_state.clone());
                break;
            }
        };

        let state_before = current_state.clone();

        match execute_step(
            step,
            client,
            context,
            scenario.auth.as_ref(),
            config,
            task_id,
        )
        .await
        {
            Ok(result) => {
                // Evaluate transitions to determine next state
                let (_, edges) = step.resolved_edges().expect(
                    "run_graph_mode is only called after validate_scenario confirms graph layout",
                );
                let next_state = match evaluate_transitions(
                    &edges,
                    &result.response,
                    &result.assertion_results,
                    &current_state,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                        let owned_log = std::mem::take(log);
                        return (owned_log, Err(e));
                    }
                };

                let step_log = StepLog {
                    step_name: step.name.clone(),
                    state_before,
                    state_after: next_state.clone(),
                    method: step.method.as_str().to_string(),
                    url: resolve_template(&step.url, context),
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
                };

                log.steps.push(step_log);
                log.total_steps += 1;
                if result.all_passed {
                    log.passed += 1;
                } else {
                    log.failed += 1;
                }

                current_state = next_state;
            }
            Err(RunError::Skipped { .. }) => {
                // Skipped step: take default transition
                let (_, edges) = step.resolved_edges().expect(
                    "run_graph_mode is only called after validate_scenario confirms graph layout",
                );
                let next_state = edges
                    .iter()
                    .find(|e| e.default.unwrap_or(false))
                    .map(|e| e.to.clone())
                    .unwrap_or_else(|| edges[0].to.clone());
                debug!(
                    task_id,
                    step = step.name.as_str(),
                    next = next_state.as_str(),
                    "Step skipped, following default transition"
                );
                current_state = next_state;
            }
            Err(e) => {
                log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                let owned_log = std::mem::take(log);
                return (owned_log, Err(e));
            }
        }
    }

    log.total_duration_ms = run_start.elapsed().as_millis() as u64;
    (std::mem::take(log), Ok(current_state))
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
    let json: serde_json::Value = serde_json::from_str(body).map_err(|e| RunError::HttpError {
        step: step_name.to_string(),
        message: format!("Failed to parse JSON for extraction: {}", e),
    })?;

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

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub async fn run(
    scenario: &Scenario,
    config: &RunConfig,
) -> Vec<(ExecutionLog, Result<String, RunError>)> {
    let concurrency = scenario.concurrency.unwrap_or(1);

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
        let result = handle.await.expect("runner task panicked");
        results.push(result);
    }

    results
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use model::{Method, Scenario, Step, Transition, TransitionEdge, load_scenario};

    fn default_config() -> RunConfig {
        RunConfig::default()
    }

    #[tokio::test]
    async fn valid_transitions() {
        let scenario = Scenario {
            name: "test".into(),
            initial_state: "start".into(),
            concurrency: None,
            auth: None,
            variables: None,
            proxy: None,
            insecure: None,
            default_timeout_ms: None,
            max_iterations: None,
            terminal_states: None,
            steps: vec![
                Step {
                    name: "step1".into(),
                    method: Method::Get,
                    url: "http://example.com".into(),
                    transition: Some(Transition {
                        from: "start".into(),
                        to: "middle".into(),
                    }),
                    transitions: None,
                    state: None,
                    headers: None,
                    body: None,
                    multipart: None,
                    extract: None,
                    retry: None,
                    assertions: None,
                    timeout_ms: None,
                    pre_request: None,
                    post_request: None,
                },
                Step {
                    name: "step2".into(),
                    method: Method::Post,
                    url: "http://example.com".into(),
                    transition: Some(Transition {
                        from: "middle".into(),
                        to: "done".into(),
                    }),
                    transitions: None,
                    state: None,
                    headers: None,
                    body: None,
                    multipart: None,
                    extract: None,
                    retry: None,
                    assertions: None,
                    timeout_ms: None,
                    pre_request: None,
                    post_request: None,
                },
            ],
        };

        let results = run(&scenario, &default_config()).await;
        assert_eq!(results.len(), 1);
        let (log, state) = &results[0];
        let state = state.as_ref().unwrap();
        assert_eq!(state, "done");
        assert_eq!(log.steps.len(), 2);
        assert_eq!(log.passed, 2);
    }

    #[tokio::test]
    async fn invalid_transition() {
        let scenario = Scenario {
            name: "test".into(),
            initial_state: "start".into(),
            concurrency: None,
            auth: None,
            variables: None,
            proxy: None,
            insecure: None,
            default_timeout_ms: None,
            max_iterations: None,
            terminal_states: None,
            steps: vec![Step {
                name: "bad step".into(),
                method: Method::Get,
                url: "http://example.com".into(),
                transition: Some(Transition {
                    from: "wrong".into(),
                    to: "done".into(),
                }),
                transitions: None,
                state: None,
                headers: None,
                body: None,
                multipart: None,
                extract: None,
                retry: None,
                assertions: None,
                timeout_ms: None,
                pre_request: None,
                post_request: None,
            }],
        };

        let results = run(&scenario, &default_config()).await;
        assert!(matches!(
            &results[0].1,
            Err(RunError::InvalidTransition { .. })
        ));
    }

    #[tokio::test]
    async fn roundtrip_yaml() {
        let yaml = r#"
name: flow
initial_state: init
steps:
  - name: fetch
    method: GET
    url: http://example.com
    transition:
      from: init
      to: fetched
"#;
        let scenario = load_scenario(yaml).unwrap();
        let results = run(&scenario, &default_config()).await;
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn base64_encode_works() {
        assert_eq!(BASE64.encode(b"admin:secret"), "YWRtaW46c2VjcmV0");
        assert_eq!(BASE64.encode(b"a:b"), "YTpi");
    }

    // -----------------------------------------------------------------------
    // Graph-mode tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn graph_mode_simple_branch() {
        let yaml = r#"
name: graph test
initial_state: fetch
steps:
  - name: fetch
    method: GET
    url: http://example.com
    transitions:
      - to: done
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let results = run(&scenario, &default_config()).await;
        assert_eq!(results.len(), 1);
        let (log, state) = &results[0];
        assert!(state.is_ok());
        assert_eq!(state.as_ref().unwrap(), "done");
        assert_eq!(log.total_steps, 1);
        assert_eq!(log.terminal_state.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn graph_mode_multi_step() {
        let yaml = r#"
name: multi step graph
initial_state: step1
steps:
  - name: step1
    method: GET
    url: http://example.com
    transitions:
      - to: step2
        default: true
  - name: step2
    method: GET
    url: http://example.com
    transitions:
      - to: done
        default: true
"#;
        let scenario = load_scenario(yaml).unwrap();
        let results = run(&scenario, &default_config()).await;
        let (log, state) = &results[0];
        assert_eq!(state.as_ref().unwrap(), "done");
        assert_eq!(log.total_steps, 2);
    }

    #[test]
    fn evaluate_transitions_default() {
        let edges = vec![TransitionEdge {
            to: "next".into(),
            when: None,
            default: Some(true),
        }];
        let response = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 50,
        };
        let result = evaluate_transitions(&edges, &response, &[], "current");
        assert_eq!(result.unwrap(), "next");
    }

    #[test]
    fn evaluate_transitions_status_match() {
        let edges = vec![
            TransitionEdge {
                to: "success".into(),
                when: Some(TransitionCondition {
                    status: Some(StatusMatch::Exact(200)),
                    body: None,
                    assertions: None,
                }),
                default: None,
            },
            TransitionEdge {
                to: "error".into(),
                when: None,
                default: Some(true),
            },
        ];
        let response = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 50,
        };
        assert_eq!(
            evaluate_transitions(&edges, &response, &[], "current").unwrap(),
            "success"
        );

        let response_500 = HttpResponse {
            status: 500,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 50,
        };
        assert_eq!(
            evaluate_transitions(&edges, &response_500, &[], "current").unwrap(),
            "error"
        );
    }

    #[test]
    fn evaluate_transitions_no_match() {
        let edges = vec![TransitionEdge {
            to: "only_200".into(),
            when: Some(TransitionCondition {
                status: Some(StatusMatch::Exact(200)),
                body: None,
                assertions: None,
            }),
            default: None,
        }];
        let response = HttpResponse {
            status: 500,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 50,
        };
        assert!(matches!(
            evaluate_transitions(&edges, &response, &[], "current"),
            Err(RunError::NoMatchingTransition { .. })
        ));
    }

    #[test]
    fn evaluate_transitions_assertion_routing() {
        let edges = vec![
            TransitionEdge {
                to: "pass_state".into(),
                when: Some(TransitionCondition {
                    status: None,
                    body: None,
                    assertions: Some(AssertionMatch::Passed),
                }),
                default: None,
            },
            TransitionEdge {
                to: "fail_state".into(),
                when: Some(TransitionCondition {
                    status: None,
                    body: None,
                    assertions: Some(AssertionMatch::Failed),
                }),
                default: None,
            },
        ];
        let response = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: "{}".into(),
            duration_ms: 50,
        };

        let passed = vec![AssertionResult {
            description: "test".into(),
            passed: true,
            expected: "200".into(),
            actual: "200".into(),
        }];
        assert_eq!(
            evaluate_transitions(&edges, &response, &passed, "current").unwrap(),
            "pass_state"
        );

        let failed = vec![AssertionResult {
            description: "test".into(),
            passed: false,
            expected: "200".into(),
            actual: "500".into(),
        }];
        assert_eq!(
            evaluate_transitions(&edges, &response, &failed, "current").unwrap(),
            "fail_state"
        );
    }
}
