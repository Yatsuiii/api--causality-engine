use ace_core::{
    assertions::{self, AssertionResult},
    jsonpath,
    variables::{self, resolve_template},
};
use ace_http::{
    Client, ClientConfig, MultipartField, MultipartValue, RequestOptions, build_client,
    send_request,
};
use model::{Auth, Hook, Scenario};
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
        }
    }
}

// ---------------------------------------------------------------------------
// Execution log types
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ExecutionLog {
    pub steps: Vec<StepLog>,
    pub total_duration_ms: u64,
    pub total_steps: usize,
    pub passed: usize,
    pub failed: usize,
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

#[derive(Debug, Default)]
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
    let grant_type = oauth
        .grant_type
        .as_deref()
        .unwrap_or("client_credentials")
        .to_string();

    // Build form-encoded body
    let body = format!(
        "grant_type={}&client_id={}&client_secret={}{}",
        url_encode(&grant_type),
        url_encode(&client_id),
        url_encode(&client_secret),
        oauth
            .scope
            .as_ref()
            .map(|s| format!("&scope={}", url_encode(&resolve_template(s, context))))
            .unwrap_or_default()
    );

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

fn url_encode(s: &str) -> String {
    let mut encoded = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
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
        // skip_if: evaluate simple condition "{{var}} == value" or just check if var is truthy
        if let Some(condition) = &hook.skip_if {
            let resolved = resolve_template(condition, context);
            let should_skip = resolved == "true" || resolved == "1" || !resolved.is_empty();
            if should_skip {
                return Some(format!("skip_if: {}", condition));
            }
        }

        // set: assign variables
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

        // delay_ms: sleep
        if let Some(delay) = hook.delay_ms {
            debug!(task_id, step = step_name, phase, delay, "Hook delay");
            sleep(Duration::from_millis(delay)).await;
        }

        // log: print message
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
// Runner
// ---------------------------------------------------------------------------

async fn run_once(
    scenario: &Scenario,
    task_id: usize,
    config: &RunConfig,
) -> (ExecutionLog, Result<String, RunError>) {
    // Build client with cookie jar, proxy, TLS config
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

    let mut current_state = scenario.initial_state.clone();

    let run_start = std::time::Instant::now();

    for step in &scenario.steps {
        // Validate state transition
        if step.transition.from != current_state {
            log.total_duration_ms = run_start.elapsed().as_millis() as u64;
            return (
                log,
                Err(RunError::InvalidTransition {
                    step: step.name.clone(),
                    expected: step.transition.from.clone(),
                    actual: current_state,
                }),
            );
        }

        // Execute pre-request hooks
        if let Some(hooks) = &step.pre_request
            && let Some(skip_reason) =
                execute_hooks(hooks, &mut context, task_id, &step.name, "pre").await
        {
            debug!(
                task_id,
                step = step.name.as_str(),
                reason = skip_reason.as_str(),
                "Step skipped"
            );
            current_state = step.transition.to.clone();
            continue;
        }

        // Resolve URL
        let url = resolve_template(&step.url, &context);

        // Build request headers
        let mut req_headers = HashMap::new();

        // Apply scenario-level auth
        if let Some(auth) = &scenario.auth {
            apply_auth(auth, &mut req_headers, &context);
        }

        // Apply step-level headers
        if let Some(headers) = &step.headers {
            for (k, v) in headers {
                req_headers.insert(k.clone(), resolve_template(v, &context));
            }
        }

        // Build request body
        let body = step.body.as_ref().map(|b| {
            let json_str = serde_json::to_string(b).unwrap_or_default();
            resolve_template(&json_str, &context)
        });

        // Build multipart fields
        let multipart = step.multipart.as_ref().map(|fields| {
            fields
                .iter()
                .map(|f| MultipartField {
                    name: resolve_template(&f.name, &context),
                    value: if let Some(file_path) = &f.file {
                        MultipartValue::File {
                            path: resolve_template(file_path, &context),
                            filename: f.filename.as_ref().map(|n| resolve_template(n, &context)),
                            mime: f.mime.clone(),
                        }
                    } else {
                        MultipartValue::Text(resolve_template(
                            f.value.as_deref().unwrap_or(""),
                            &context,
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

            match send_request(&client, method_str, &url, &opts).await {
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
                        if let Some(extract) = &step.extract
                            && let Err(e) = extract_context(
                                extract,
                                &response.body,
                                &mut context,
                                task_id,
                                &step.name,
                            )
                        {
                            log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                            return (log, Err(e));
                        }

                        let step_log = StepLog {
                            step_name: step.name.clone(),
                            state_before: step.transition.from.clone(),
                            state_after: step.transition.to.clone(),
                            method: method_str.to_string(),
                            url: url.clone(),
                            status: response.status,
                            duration_ms: response.duration_ms,
                            assertions: assertion_results.clone(),
                            request_body: if config.verbose { body.clone() } else { None },
                            response_body: if config.verbose {
                                Some(response.body.clone())
                            } else {
                                None
                            },
                        };

                        log.steps.push(step_log);
                        log.total_steps += 1;
                        if all_passed {
                            log.passed += 1;
                        } else {
                            log.failed += 1;
                        }

                        // Execute post-request hooks
                        if let Some(hooks) = &step.post_request {
                            execute_hooks(hooks, &mut context, task_id, &step.name, "post").await;
                        }

                        if !all_passed {
                            let failures: Vec<_> = assertion_results
                                .into_iter()
                                .filter(|a| !a.passed)
                                .collect();
                            log.total_duration_ms = run_start.elapsed().as_millis() as u64;
                            return (
                                log,
                                Err(RunError::AssertionFailed {
                                    step: step.name.clone(),
                                    failures,
                                }),
                            );
                        }

                        last_err = None;
                        break;
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

        if let Some(e) = last_err {
            log.total_duration_ms = run_start.elapsed().as_millis() as u64;
            return (
                log,
                Err(RunError::HttpError {
                    step: step.name.clone(),
                    message: e,
                }),
            );
        }

        current_state = step.transition.to.clone();
    }

    log.total_duration_ms = run_start.elapsed().as_millis() as u64;

    (log, Ok(current_state))
}

fn apply_auth(
    auth: &Auth,
    headers: &mut HashMap<String, String>,
    context: &HashMap<String, String>,
) {
    if let Some(bearer) = &auth.bearer {
        let token = resolve_template(bearer, context);
        headers.insert("Authorization".into(), format!("Bearer {}", token));
    }
    if let Some(basic) = &auth.basic {
        let user = resolve_template(&basic.username, context);
        let pass = resolve_template(&basic.password, context);
        use std::io::Write;
        let mut buf = Vec::new();
        write!(buf, "{}:{}", user, pass).unwrap();
        let encoded = base64_encode(&buf);
        headers.insert("Authorization".into(), format!("Basic {}", encoded));
    }
    if let Some(api_key) = &auth.api_key {
        let header = resolve_template(&api_key.header, context);
        let value = resolve_template(&api_key.value, context);
        headers.insert(header, value);
    }
    // OAuth2: use pre-fetched token stored in context
    if auth.oauth2.is_some()
        && let Some(token) = context.get("$oauth_token")
    {
        headers.insert("Authorization".into(), format!("Bearer {}", token));
    }
}

fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
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
        let cli_variables = config.cli_variables.clone();
        let verbose = config.verbose;
        let insecure = config.insecure;
        let proxy = config.proxy.clone();
        handles.push(tokio::spawn(async move {
            let cfg = RunConfig {
                cli_variables,
                verbose,
                insecure,
                proxy,
            };
            run_once(&scenario, i, &cfg).await
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        let result = handle.await.unwrap();
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
    use model::{Method, Scenario, Step, Transition, load_scenario};

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
            steps: vec![
                Step {
                    name: "step1".into(),
                    method: Method::Get,
                    url: "http://example.com".into(),
                    transition: Transition {
                        from: "start".into(),
                        to: "middle".into(),
                    },
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
                    transition: Transition {
                        from: "middle".into(),
                        to: "done".into(),
                    },
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
            steps: vec![Step {
                name: "bad step".into(),
                method: Method::Get,
                url: "http://example.com".into(),
                transition: Transition {
                    from: "wrong".into(),
                    to: "done".into(),
                },
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
        assert_eq!(base64_encode(b"admin:secret"), "YWRtaW46c2VjcmV0");
        assert_eq!(base64_encode(b"a:b"), "YTpi");
    }
}
