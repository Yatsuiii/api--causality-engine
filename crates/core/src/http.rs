use ace_http::{Client, HttpResponse, MultipartField, MultipartValue, RequestOptions};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use model::{Auth, Hook, MultipartFieldDef, Scenario, Step};
use std::collections::HashMap;

use crate::assertions::{self, AssertionResult};
use crate::jsonpath;
use crate::variables::{Context, resolve_template};

// ---------------------------------------------------------------------------
// Step execution result
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct StepResult {
    pub step_name: String,
    pub response: HttpResponse,
    pub assertions: Vec<AssertionResult>,
    pub extracted: HashMap<String, serde_json::Value>,
    pub attempt: u32,
}

impl StepResult {
    pub fn passed(&self) -> bool {
        self.assertions.iter().all(|a| a.passed)
    }
}

// ---------------------------------------------------------------------------
// Execute a single step
// ---------------------------------------------------------------------------

pub async fn execute_step(
    client: &Client,
    step: &Step,
    auth: Option<&Auth>,
    context: &mut Context,
) -> Result<StepResult, String> {
    let max_attempts = step.retry.as_ref().map_or(1, |r| r.attempts.max(1));
    let retry_delay = step.retry.as_ref().map_or(0, |r| r.delay_ms);

    for attempt in 1..=max_attempts {
        let url = resolve_template(&step.url, context);
        let opts = build_request_options(step, auth, context)?;

        let response = ace_http::send_request(client, step.method.as_str(), &url, &opts).await?;

        let extracted = extract_variables(step, &response);
        for (k, v) in &extracted {
            context.insert(k.clone(), v.clone());
        }

        let assertion_results = step
            .assertions
            .as_ref()
            .map(|a| assertions::evaluate(a, &response))
            .unwrap_or_default();

        let result = StepResult {
            step_name: step.name.clone(),
            response,
            assertions: assertion_results,
            extracted,
            attempt,
        };

        if result.passed() || attempt == max_attempts {
            return Ok(result);
        }

        if retry_delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(retry_delay)).await;
        }
    }

    unreachable!("loop exits via early return on the final attempt")
}

// ---------------------------------------------------------------------------
// Execute all steps in a scenario sequentially
// ---------------------------------------------------------------------------

pub async fn execute_scenario(
    client: &Client,
    scenario: &Scenario,
    context: &mut Context,
) -> Result<Vec<StepResult>, String> {
    let mut results = Vec::with_capacity(scenario.steps.len());

    for step in &scenario.steps {
        if let Some(hooks) = &step.pre_request {
            run_hooks(hooks, context, true).await;
        }

        let result = execute_step(client, step, scenario.auth.as_ref(), context).await?;

        if let Some(hooks) = &step.post_request {
            run_hooks(hooks, context, false).await;
        }

        results.push(result);
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Build RequestOptions from a Step + Auth + context
// ---------------------------------------------------------------------------

fn build_request_options(
    step: &Step,
    auth: Option<&Auth>,
    context: &Context,
) -> Result<RequestOptions, String> {
    let mut headers: HashMap<String, String> = step
        .headers
        .as_ref()
        .map(|h| {
            h.iter()
                .map(|(k, v)| (k.clone(), resolve_template(v, context)))
                .collect()
        })
        .unwrap_or_default();

    if let Some(auth) = auth {
        apply_auth(auth, &mut headers, context);
    }

    let body = step.body.as_ref().map(|b| {
        let raw = serde_json::to_string(b).unwrap_or_default();
        resolve_template(&raw, context)
    });

    if body.is_some()
        && !headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("content-type"))
    {
        headers.insert("Content-Type".into(), "application/json".into());
    }

    let multipart = step
        .multipart
        .as_ref()
        .map(|fields| build_multipart(fields, context));

    Ok(RequestOptions {
        headers,
        body,
        timeout_ms: step.timeout_ms,
        multipart,
    })
}

// ---------------------------------------------------------------------------
// Hook execution
// ---------------------------------------------------------------------------

async fn run_hooks(hooks: &[Hook], context: &mut Context, allow_delay: bool) {
    for hook in hooks {
        if let Some(skip_expr) = &hook.skip_if {
            let resolved = resolve_template(skip_expr, context);
            if resolved == "true" || resolved == "1" {
                continue;
            }
        }
        if let Some(vars) = &hook.set {
            for (k, v) in vars {
                let resolved = resolve_template(v, context);
                context.insert(k.clone(), serde_json::Value::String(resolved));
            }
        }
        if allow_delay && let Some(delay) = hook.delay_ms {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Auth helpers
// ---------------------------------------------------------------------------

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
}

// ---------------------------------------------------------------------------
// Multipart builder
// ---------------------------------------------------------------------------

fn build_multipart(fields: &[MultipartFieldDef], context: &Context) -> Vec<MultipartField> {
    fields
        .iter()
        .map(|f| {
            let name = resolve_template(&f.name, context);
            let value = if let Some(file_path) = &f.file {
                MultipartValue::File {
                    path: resolve_template(file_path, context),
                    filename: f.filename.as_ref().map(|s| resolve_template(s, context)),
                    mime: f.mime.as_ref().map(|s| resolve_template(s, context)),
                }
            } else {
                MultipartValue::Text(
                    f.value
                        .as_ref()
                        .map(|v| resolve_template(v, context))
                        .unwrap_or_default(),
                )
            };
            MultipartField { name, value }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Variable extraction from response
// ---------------------------------------------------------------------------

fn extract_variables(step: &Step, response: &HttpResponse) -> HashMap<String, serde_json::Value> {
    let mut extracted = HashMap::new();

    let Some(extract_map) = &step.extract else {
        return extracted;
    };

    let json: Option<serde_json::Value> = serde_json::from_str(&response.body).ok();

    for (var_name, spec) in extract_map {
        if let Some(ref json) = json
            && let Some(value) = jsonpath::resolve(json, spec.path())
        {
            extracted.insert(var_name.clone(), value);
        }
    }

    extracted
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use model::{ApiKeyAuth, Auth, BasicAuth};

    fn empty_ctx() -> Context {
        Context::new()
    }

    #[test]
    fn apply_bearer_auth() {
        let auth = Auth {
            bearer: Some("my-token".into()),
            basic: None,
            api_key: None,
            oauth2: None,
        };
        let mut headers = HashMap::new();
        apply_auth(&auth, &mut headers, &empty_ctx());
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer my-token");
    }

    #[test]
    fn apply_basic_auth() {
        let auth = Auth {
            bearer: None,
            basic: Some(BasicAuth {
                username: "admin".into(),
                password: "secret".into(),
            }),
            api_key: None,
            oauth2: None,
        };
        let mut headers = HashMap::new();
        apply_auth(&auth, &mut headers, &empty_ctx());
        let value = headers.get("Authorization").unwrap();
        assert!(value.starts_with("Basic "));
    }

    #[test]
    fn apply_api_key_auth() {
        let auth = Auth {
            bearer: None,
            basic: None,
            api_key: Some(ApiKeyAuth {
                header: "X-API-Key".into(),
                value: "key-123".into(),
            }),
            oauth2: None,
        };
        let mut headers = HashMap::new();
        apply_auth(&auth, &mut headers, &empty_ctx());
        assert_eq!(headers.get("X-API-Key").unwrap(), "key-123");
    }

    #[test]
    fn auth_does_not_override_existing_header() {
        let auth = Auth {
            bearer: Some("new-token".into()),
            basic: None,
            api_key: None,
            oauth2: None,
        };
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer existing".into());
        apply_auth(&auth, &mut headers, &empty_ctx());
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer existing");
    }

    #[test]
    fn auth_resolves_templates() {
        let auth = Auth {
            bearer: Some("{{token}}".into()),
            basic: None,
            api_key: None,
            oauth2: None,
        };
        let mut headers = HashMap::new();
        let mut ctx = Context::new();
        ctx.insert(
            "token".into(),
            serde_json::Value::String("resolved-token".into()),
        );
        apply_auth(&auth, &mut headers, &ctx);
        assert_eq!(
            headers.get("Authorization").unwrap(),
            "Bearer resolved-token"
        );
    }

    #[test]
    fn extract_from_json_response() {
        let step = model::Step {
            name: "test".into(),
            state: "a".into(),
            method: model::Method::Get,
            url: "http://example.com".into(),
            headers: None,
            body: None,
            multipart: None,
            extract: Some({
                let mut m = HashMap::new();
                m.insert("user_id".into(), "data.id".into());
                m.insert("name".into(), "data.name".into());
                m
            }),
            retry: None,
            assertions: None,
            timeout_ms: None,
            pre_request: None,
            post_request: None,
            tags: None,
        };

        let response = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: r#"{"data": {"id": "42", "name": "Alice"}}"#.into(),
            duration_ms: 50,
        };

        let extracted = extract_variables(&step, &response);
        assert_eq!(
            extracted.get("user_id").unwrap(),
            &serde_json::Value::String("42".into())
        );
        assert_eq!(
            extracted.get("name").unwrap(),
            &serde_json::Value::String("Alice".into())
        );
    }

    #[test]
    fn extract_preserves_native_number_type() {
        let step = model::Step {
            name: "test".into(),
            state: "a".into(),
            method: model::Method::Get,
            url: "http://example.com".into(),
            headers: None,
            body: None,
            multipart: None,
            extract: Some({
                let mut m = HashMap::new();
                m.insert("count".into(), "data.count".into());
                m
            }),
            retry: None,
            assertions: None,
            timeout_ms: None,
            pre_request: None,
            post_request: None,
            tags: None,
        };

        let response = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: r#"{"data": {"count": 42}}"#.into(),
            duration_ms: 50,
        };

        let extracted = extract_variables(&step, &response);
        assert_eq!(extracted.get("count").unwrap(), &serde_json::json!(42));
    }

    #[test]
    fn extract_missing_path_is_skipped() {
        let step = model::Step {
            name: "test".into(),
            state: "a".into(),
            method: model::Method::Get,
            url: "http://example.com".into(),
            headers: None,
            body: None,
            multipart: None,
            extract: Some({
                let mut m = HashMap::new();
                m.insert("missing".into(), "no.such.path".into());
                m
            }),
            retry: None,
            assertions: None,
            timeout_ms: None,
            pre_request: None,
            post_request: None,
            tags: None,
        };

        let response = HttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: r#"{"id": 1}"#.into(),
            duration_ms: 50,
        };

        let extracted = extract_variables(&step, &response);
        assert!(extracted.is_empty());
    }
}
