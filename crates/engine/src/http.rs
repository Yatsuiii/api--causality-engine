use crate::assertions::{self, AssertionResult, SchemaCache};
use crate::auth::apply_auth;
use crate::config::{RunConfig, RunError};
use crate::jsonpath;
use crate::variables::{self, Context, resolve_template};
use ace_http::{
    Client, HttpResponse, MultipartField, MultipartValue, RequestOptions, send_request,
};
use model::{Auth, BackoffPolicy, Hook, JitterMode, RetryConfig, Step};
use rand::Rng;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

pub(crate) struct StepResult {
    pub response: HttpResponse,
    pub assertion_results: Vec<AssertionResult>,
    pub all_passed: bool,
    pub body_sent: Option<String>,
    pub url_sent: String,
}

pub(crate) async fn execute_hooks(
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_step(
    step: &Step,
    client: &Client,
    context: &mut Context,
    scenario_auth: Option<&Auth>,
    _config: &RunConfig,
    task_id: usize,
    schema_cache: &SchemaCache,
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

    let retry_cfg = step.retry.as_ref();
    let max_attempts = retry_cfg.map_or(1, |r| r.attempts.max(1));
    let mut last_err: Option<String> = None;
    let method_str = step.method.as_str();

    for attempt in 1..=max_attempts {
        if attempt > 1
            && let Some(rc) = retry_cfg
        {
            let delay_ms = compute_retry_delay(rc, attempt);
            info!(
                task_id,
                step = step.name.as_str(),
                attempt,
                max_attempts,
                delay_ms,
                "Retrying"
            );
            sleep(Duration::from_millis(delay_ms)).await;
        }

        match send_request(client, method_str, &url, &opts).await {
            Ok(response) => {
                let should_retry_status = retry_cfg
                    .map(|rc| rc.should_retry_status(response.status))
                    .unwrap_or(false);

                if !should_retry_status || attempt == max_attempts {
                    let assertion_results = if let Some(asserts) = &step.assertions {
                        assertions::evaluate_with_cache(
                            asserts,
                            &response,
                            _config.scenario_dir.as_deref(),
                            schema_cache,
                        )
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
                    "Retryable status, will retry"
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

pub(crate) fn extract_context(
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

/// Delay (ms) to sleep before the given retry attempt (attempt >= 2).
/// Public for unit-testing.
pub fn compute_retry_delay(rc: &RetryConfig, attempt: u32) -> u64 {
    let base = match rc.backoff {
        BackoffPolicy::Fixed => rc.delay_ms,
        BackoffPolicy::Exponential => {
            // attempt 2 is the first sleep (exp = 0). attempt 3 is exp = 1, etc.
            let exp = attempt.saturating_sub(2) as i32;
            let raw = (rc.delay_ms as f64) * rc.multiplier.powi(exp);
            if raw.is_finite() && raw >= 0.0 {
                raw.min(u64::MAX as f64) as u64
            } else {
                rc.max_delay_ms
            }
        }
    };
    let capped = base.min(rc.max_delay_ms);
    apply_jitter(capped, rc.jitter)
}

fn apply_jitter(delay: u64, mode: JitterMode) -> u64 {
    if delay == 0 {
        return 0;
    }
    match mode {
        JitterMode::None => delay,
        JitterMode::Full => rand::thread_rng().gen_range(0..=delay),
        JitterMode::Equal => {
            let half = delay / 2;
            half + rand::thread_rng().gen_range(0..=(delay - half))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_cfg(backoff: BackoffPolicy, jitter: JitterMode) -> RetryConfig {
        RetryConfig {
            attempts: 5,
            delay_ms: 100,
            backoff,
            multiplier: 2.0,
            max_delay_ms: 30_000,
            jitter,
            retry_on: Vec::new(),
        }
    }

    #[test]
    fn retry_delay_fixed_stays_flat() {
        let rc = retry_cfg(BackoffPolicy::Fixed, JitterMode::None);
        assert_eq!(compute_retry_delay(&rc, 2), 100);
        assert_eq!(compute_retry_delay(&rc, 3), 100);
        assert_eq!(compute_retry_delay(&rc, 5), 100);
    }

    #[test]
    fn retry_delay_exponential_doubles() {
        let rc = retry_cfg(BackoffPolicy::Exponential, JitterMode::None);
        assert_eq!(compute_retry_delay(&rc, 2), 100);
        assert_eq!(compute_retry_delay(&rc, 3), 200);
        assert_eq!(compute_retry_delay(&rc, 4), 400);
        assert_eq!(compute_retry_delay(&rc, 5), 800);
    }

    #[test]
    fn retry_delay_exponential_capped_by_max() {
        let mut rc = retry_cfg(BackoffPolicy::Exponential, JitterMode::None);
        rc.delay_ms = 1000;
        rc.max_delay_ms = 3000;
        assert_eq!(compute_retry_delay(&rc, 2), 1000);
        assert_eq!(compute_retry_delay(&rc, 3), 2000);
        assert_eq!(compute_retry_delay(&rc, 4), 3000);
        assert_eq!(compute_retry_delay(&rc, 10), 3000);
    }

    #[test]
    fn retry_delay_jitter_full_stays_in_range() {
        let rc = retry_cfg(BackoffPolicy::Fixed, JitterMode::Full);
        for _ in 0..20 {
            let d = compute_retry_delay(&rc, 2);
            assert!(d <= 100, "full jitter should be <= delay, got {}", d);
        }
    }

    #[test]
    fn retry_delay_jitter_equal_stays_in_half_range() {
        let rc = retry_cfg(BackoffPolicy::Fixed, JitterMode::Equal);
        for _ in 0..20 {
            let d = compute_retry_delay(&rc, 2);
            assert!(
                (50..=100).contains(&d),
                "equal jitter should be delay/2..=delay, got {}",
                d
            );
        }
    }

    #[test]
    fn retry_predicate_default_set() {
        let rc = RetryConfig::default();
        assert!(rc.should_retry_status(500));
        assert!(rc.should_retry_status(502));
        assert!(rc.should_retry_status(429));
        assert!(rc.should_retry_status(408));
        assert!(!rc.should_retry_status(400));
        assert!(!rc.should_retry_status(401));
        assert!(!rc.should_retry_status(404));
        assert!(!rc.should_retry_status(200));
        assert!(!rc.should_retry_status(301));
    }

    #[test]
    fn retry_predicate_explicit_overrides_default() {
        let rc = RetryConfig {
            retry_on: vec![404, 503],
            ..Default::default()
        };
        assert!(rc.should_retry_status(404));
        assert!(rc.should_retry_status(503));
        assert!(!rc.should_retry_status(500));
        assert!(!rc.should_retry_status(429));
    }
}
