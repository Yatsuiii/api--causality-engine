use ace_http::HttpResponse;
use model::{Assertion, SchemaRef, StatusCheck, ValueCheck};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
#[cfg(feature = "schema")]
use std::sync::{Arc, Mutex};

use crate::jsonpath;

// ---------------------------------------------------------------------------
// Compiled-schema cache
// ---------------------------------------------------------------------------

/// Caches compiled JSONSchemas keyed by source identity. Polling loops
/// re-evaluate the same schema every iteration; compiling once per run is
/// cheap, compiling per step is not. Interior mutability (via Mutex) lets a
/// single cache be shared by all parallel branches in one scenario run.
#[cfg(feature = "schema")]
#[derive(Default)]
pub struct SchemaCache {
    entries: Mutex<HashMap<String, Result<Arc<jsonschema::JSONSchema>, String>>>,
}

#[cfg(not(feature = "schema"))]
#[derive(Default)]
pub struct SchemaCache;

impl SchemaCache {
    pub fn new() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// Assertion result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct AssertionResult {
    pub description: String,
    pub passed: bool,
    pub expected: String,
    pub actual: String,
}

// ---------------------------------------------------------------------------
// Evaluate all assertions for a step
// ---------------------------------------------------------------------------

pub fn evaluate(assertions: &[Assertion], response: &HttpResponse) -> Vec<AssertionResult> {
    evaluate_with_base(assertions, response, None)
}

/// Same as `evaluate`, but resolves relative `schema:` paths against `base_dir`
/// (typically the scenario file's parent directory). Absolute paths are used
/// as-is; inline schemas ignore `base_dir`. Uses a throwaway cache — callers
/// in hot loops should use [`evaluate_with_cache`] instead.
pub fn evaluate_with_base(
    assertions: &[Assertion],
    response: &HttpResponse,
    base_dir: Option<&Path>,
) -> Vec<AssertionResult> {
    let cache = SchemaCache::new();
    evaluate_with_cache(assertions, response, base_dir, &cache)
}

/// Evaluate assertions against a response, reusing `cache` for any `schema:`
/// refs. Compile cost is paid once per unique schema for the lifetime of the
/// cache — critical for polling loops that re-execute the same step N times.
pub fn evaluate_with_cache(
    assertions: &[Assertion],
    response: &HttpResponse,
    base_dir: Option<&Path>,
    cache: &SchemaCache,
) -> Vec<AssertionResult> {
    let mut results = Vec::new();

    for assertion in assertions {
        if let Some(status_check) = &assertion.status {
            results.push(eval_status(status_check, response.status));
        }

        if let Some(body_checks) = &assertion.body {
            let json: Option<serde_json::Value> = serde_json::from_str(&response.body).ok();
            for (path, check) in body_checks {
                results.push(eval_body(path, check, json.as_ref()));
            }
        }

        if let Some(expected_type) = &assertion.body_type {
            let json: Option<serde_json::Value> = serde_json::from_str(&response.body).ok();
            results.push(eval_body_type(expected_type, json.as_ref()));
        }

        if let Some(header_checks) = &assertion.header {
            for (header_name, check) in header_checks {
                results.push(eval_header(header_name, check, &response.headers));
            }
        }

        if let Some(time_check) = &assertion.response_time_ms {
            results.push(eval_response_time(time_check, response.duration_ms));
        }

        if let Some(schema_ref) = &assertion.schema {
            results.push(eval_schema(schema_ref, &response.body, base_dir, cache));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Individual assertion evaluators
// ---------------------------------------------------------------------------

fn eval_status(check: &StatusCheck, actual: u16) -> AssertionResult {
    match check {
        StatusCheck::Exact(expected) => AssertionResult {
            description: format!("status == {}", expected),
            passed: actual == *expected,
            expected: expected.to_string(),
            actual: actual.to_string(),
        },
        StatusCheck::Complex(vc) => {
            let val = serde_json::Value::Number(serde_json::Number::from(actual));
            let passed = eval_value_check(vc, Some(&val), &actual.to_string());
            AssertionResult {
                description: "status check".to_string(),
                passed,
                expected: describe_value_check(vc),
                actual: actual.to_string(),
            }
        }
    }
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Null => "null",
    }
}

fn eval_body_type(expected_type: &str, json: Option<&serde_json::Value>) -> AssertionResult {
    let actual_type = json.map(json_type_name).unwrap_or("null");
    AssertionResult {
        description: format!("body type == {}", expected_type),
        passed: actual_type == expected_type,
        expected: expected_type.to_string(),
        actual: actual_type.to_string(),
    }
}

fn eval_body(path: &str, check: &ValueCheck, json: Option<&serde_json::Value>) -> AssertionResult {
    let resolved = json.and_then(|j| jsonpath::resolve(j, path));
    let actual_str = resolved
        .as_ref()
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "<missing>".to_string());

    let passed = eval_value_check(check, resolved.as_ref(), &actual_str);

    AssertionResult {
        description: format!("body.{}", path),
        passed,
        expected: describe_value_check(check),
        actual: actual_str,
    }
}

fn eval_header(
    header_name: &str,
    check: &ValueCheck,
    headers: &HashMap<String, String>,
) -> AssertionResult {
    // Headers are case-insensitive
    let actual = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(header_name))
        .map(|(_, v)| v.clone());

    let actual_str = actual.clone().unwrap_or_else(|| "<missing>".to_string());
    let val = actual.map(serde_json::Value::String);
    let passed = eval_value_check(check, val.as_ref(), &actual_str);

    AssertionResult {
        description: format!("header.{}", header_name),
        passed,
        expected: describe_value_check(check),
        actual: actual_str,
    }
}

fn eval_response_time(check: &ValueCheck, duration_ms: u64) -> AssertionResult {
    let val = serde_json::Value::Number(serde_json::Number::from(duration_ms));
    let actual_str = format!("{}ms", duration_ms);
    let passed = eval_value_check(check, Some(&val), &actual_str);

    AssertionResult {
        description: "response_time_ms".to_string(),
        passed,
        expected: describe_value_check(check),
        actual: actual_str,
    }
}

/// Validate the response body against a JSONSchema document.
///
/// Any failure path (schema load, schema compile, body parse, validation) produces
/// a failing `AssertionResult` rather than a panic — the description records which
/// stage failed so users can act on it.
#[cfg(not(feature = "schema"))]
fn eval_schema(
    _schema_ref: &SchemaRef,
    _body: &str,
    _base_dir: Option<&Path>,
    _cache: &SchemaCache,
) -> AssertionResult {
    AssertionResult {
        description: "schema (<feature disabled>)".to_string(),
        passed: false,
        expected: "valid against schema".to_string(),
        actual: "JSONSchema support not compiled in (rebuild with --features schema)".to_string(),
    }
}

#[cfg(feature = "schema")]
fn eval_schema(
    schema_ref: &SchemaRef,
    body: &str,
    base_dir: Option<&Path>,
    cache: &SchemaCache,
) -> AssertionResult {
    let (cache_key, schema_source): (String, String) = match schema_ref {
        SchemaRef::Inline(v) => {
            // Canonicalize key ordering so two semantically-equal inline schemas
            // hit the same cache entry regardless of how serde serialized them.
            let canonical = canonical_json(v);
            (format!("inline:{}", canonical), "<inline>".to_string())
        }
        SchemaRef::File(path) => {
            let resolved = match base_dir {
                Some(dir) if !Path::new(path).is_absolute() => dir.join(path),
                _ => Path::new(path).to_path_buf(),
            };
            let display = resolved.display().to_string();
            (format!("file:{}", display), display)
        }
        SchemaRef::OpenApi {
            openapi,
            component,
            strict,
        } => {
            let resolved = match base_dir {
                Some(dir) if !std::path::Path::new(openapi).is_absolute() => {
                    dir.join(openapi).display().to_string()
                }
                _ => openapi.clone(),
            };
            let source = format!(
                "{}#/components/schemas/{}{}",
                resolved,
                component,
                if *strict { " (strict)" } else { "" }
            );
            // Strict-mode mutates the resolved schema (injects additionalProperties:
            // false). Two assertions on the same component with different `strict`
            // values must NOT share a compiled schema — include strictness in key.
            let mode = if *strict { "strict" } else { "lax" };
            (
                format!("openapi:{}:{}:{}", resolved, component, mode),
                source,
            )
        }
    };

    // Compile-on-miss; cache the Err too so we don't repeatedly read/parse a
    // broken schema file on every polling-loop iteration.
    let cached = {
        let mut entries = cache.entries.lock().expect("schema cache poisoned");
        entries
            .entry(cache_key)
            .or_insert_with(|| compile_schema(schema_ref, base_dir))
            .clone()
    };

    let compiled = match cached {
        Ok(c) => c,
        Err(msg) => {
            return AssertionResult {
                description: format!("schema ({})", schema_source),
                passed: false,
                expected: "valid JSONSchema".to_string(),
                actual: msg,
            };
        }
    };

    let body_json: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return AssertionResult {
                description: format!("schema ({})", schema_source),
                passed: false,
                expected: "JSON response body".to_string(),
                actual: format!("body parse error: {}", e),
            };
        }
    };

    match compiled.validate(&body_json) {
        Ok(()) => AssertionResult {
            description: format!("schema ({})", schema_source),
            passed: true,
            expected: "valid against schema".to_string(),
            actual: "valid".to_string(),
        },
        Err(errors) => {
            // Reshape each jsonschema error into a high-signal one-liner
            // (`+ unexpected field: foo`, `- missing required field: bar`,
            // `~ type mismatch …`). Falls through to the raw text on shapes we
            // don't recognize so we never lose information. See
            // `crate::schema::format_validation_error` for the audit.
            let messages: Vec<String> = errors
                .take(5)
                .map(|e| {
                    crate::schema::format_validation_error(
                        &e.to_string(),
                        &e.instance_path.to_string(),
                    )
                    .render_text()
                })
                .collect();
            AssertionResult {
                description: format!("schema ({})", schema_source),
                passed: false,
                expected: "valid against schema".to_string(),
                actual: messages.join("; "),
            }
        }
    }
}

#[cfg(feature = "schema")]
fn compile_schema(
    schema_ref: &SchemaRef,
    base_dir: Option<&Path>,
) -> Result<Arc<jsonschema::JSONSchema>, String> {
    let (schema_value, root_doc) =
        crate::schema::resolve(schema_ref, base_dir).map_err(|e| e.to_string())?;
    let compiled = match root_doc {
        // OpenAPI: register the original document under the synthetic URI so
        // cycle-preserved `$ref`s (rewritten by `inline_refs`) can resolve.
        Some(root) => jsonschema::JSONSchema::options()
            .with_document(crate::schema::SYNTHETIC_OPENAPI_ROOT_URI.to_string(), root)
            .compile(&schema_value),
        None => jsonschema::JSONSchema::compile(&schema_value),
    };
    compiled
        .map(Arc::new)
        .map_err(|e| format!("compile error: {}", e))
}

/// Stringify a `serde_json::Value` with sorted object keys so two semantically
/// equal values produce byte-identical output regardless of insertion order.
/// Used to key the inline-schema cache.
#[cfg(feature = "schema")]
fn canonical_json(v: &serde_json::Value) -> String {
    use std::collections::BTreeMap;
    fn rebuild(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let sorted: BTreeMap<&String, serde_json::Value> =
                    map.iter().map(|(k, v)| (k, rebuild(v))).collect();
                let out: serde_json::Map<String, serde_json::Value> =
                    sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect();
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(rebuild).collect())
            }
            other => other.clone(),
        }
    }
    rebuild(v).to_string()
}

// ---------------------------------------------------------------------------
// Generic ValueCheck evaluator
// ---------------------------------------------------------------------------

pub fn eval_value_check(
    check: &ValueCheck,
    value: Option<&serde_json::Value>,
    actual_str: &str,
) -> bool {
    // exists check
    if let Some(should_exist) = check.exists {
        let does_exist = value.is_some_and(|v| !v.is_null());
        if does_exist != should_exist {
            return false;
        }
        // If only exists was specified, we're done
        if check.eq.is_none()
            && check.ne.is_none()
            && check.contains.is_none()
            && check.lt.is_none()
            && check.gt.is_none()
            && check.in_list.is_none()
            && check.type_of.is_none()
        {
            return true;
        }
    }

    // eq check
    if let Some(expected) = &check.eq {
        match value {
            Some(v) => {
                if v != expected {
                    return false;
                }
            }
            None => return false,
        }
    }

    // ne check
    if let Some(not_expected) = &check.ne
        && let Some(v) = value
        && v == not_expected
    {
        return false;
    }

    // contains check (string-based)
    if let Some(substring) = &check.contains
        && !actual_str.contains(substring.as_str())
    {
        return false;
    }

    // lt check (numeric)
    if let Some(threshold) = check.lt {
        let num = to_f64(value);
        match num {
            Some(n) if n < threshold => {}
            _ => return false,
        }
    }

    // gt check
    if let Some(threshold) = check.gt {
        let num = to_f64(value);
        match num {
            Some(n) if n > threshold => {}
            _ => return false,
        }
    }

    // in check
    if let Some(list) = &check.in_list {
        match value {
            Some(v) => {
                if !list.contains(v) {
                    return false;
                }
            }
            None => return false,
        }
    }

    // type check
    if let Some(expected_type) = &check.type_of {
        let actual_type = value.map(json_type_name).unwrap_or("null");
        if actual_type != expected_type.as_str() {
            return false;
        }
    }

    true
}

fn to_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    value.and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    })
}

fn describe_value_check(check: &ValueCheck) -> String {
    let mut parts = Vec::new();
    if let Some(v) = &check.eq {
        parts.push(format!("== {}", v));
    }
    if let Some(v) = &check.ne {
        parts.push(format!("!= {}", v));
    }
    if let Some(v) = &check.contains {
        parts.push(format!("contains \"{}\"", v));
    }
    if let Some(v) = &check.exists {
        parts.push(format!("exists: {}", v));
    }
    if let Some(v) = check.lt {
        parts.push(format!("< {}", v));
    }
    if let Some(v) = check.gt {
        parts.push(format!("> {}", v));
    }
    if let Some(v) = &check.in_list {
        parts.push(format!("in {:?}", v));
    }
    if let Some(v) = &check.type_of {
        parts.push(format!("type == {}", v));
    }
    parts.join(", ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_response(status: u16, body: &str, duration_ms: u64) -> HttpResponse {
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        HttpResponse {
            status,
            headers,
            body: body.into(),
            duration_ms,
        }
    }

    #[test]
    fn status_exact_pass() {
        let response = make_response(200, "{}", 50);
        let assertions = vec![Assertion {
            status: Some(StatusCheck::Exact(200)),
            body: None,
            header: None,
            response_time_ms: None,
            body_type: None,
            schema: None,
        }];
        let results = evaluate(&assertions, &response);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[test]
    fn status_exact_fail() {
        let response = make_response(404, "{}", 50);
        let assertions = vec![Assertion {
            status: Some(StatusCheck::Exact(200)),
            body: None,
            header: None,
            response_time_ms: None,
            body_type: None,
            schema: None,
        }];
        let results = evaluate(&assertions, &response);
        assert!(!results[0].passed);
    }

    #[test]
    fn body_exists_check() {
        let response = make_response(200, r#"{"id": 42, "name": "Alice"}"#, 50);
        let mut body_checks = HashMap::new();
        body_checks.insert(
            "id".into(),
            ValueCheck {
                exists: Some(true),
                ..Default::default()
            },
        );
        let assertions = vec![Assertion {
            status: None,
            body: Some(body_checks),
            header: None,
            response_time_ms: None,
            body_type: None,
            schema: None,
        }];
        let results = evaluate(&assertions, &response);
        assert!(results[0].passed);
    }

    #[test]
    fn body_nested_path() {
        let response = make_response(200, r#"{"data": {"user": {"name": "Alice"}}}"#, 50);
        let mut body_checks = HashMap::new();
        body_checks.insert(
            "data.user.name".into(),
            ValueCheck {
                eq: Some(serde_json::json!("Alice")),
                ..Default::default()
            },
        );
        let assertions = vec![Assertion {
            status: None,
            body: Some(body_checks),
            header: None,
            response_time_ms: None,
            body_type: None,
            schema: None,
        }];
        let results = evaluate(&assertions, &response);
        assert!(results[0].passed);
    }

    #[test]
    fn header_contains() {
        let response = make_response(200, "{}", 50);
        let mut header_checks = HashMap::new();
        header_checks.insert(
            "content-type".into(),
            ValueCheck {
                contains: Some("json".into()),
                ..Default::default()
            },
        );
        let assertions = vec![Assertion {
            status: None,
            body: None,
            header: Some(header_checks),
            response_time_ms: None,
            body_type: None,
            schema: None,
        }];
        let results = evaluate(&assertions, &response);
        assert!(results[0].passed);
    }

    #[test]
    fn response_time_check() {
        let response = make_response(200, "{}", 150);
        let assertions = vec![Assertion {
            status: None,
            body: None,
            header: None,
            response_time_ms: Some(ValueCheck {
                lt: Some(2000.0),
                ..Default::default()
            }),
            body_type: None,
            schema: None,
        }];
        let results = evaluate(&assertions, &response);
        assert!(results[0].passed);
    }

    #[test]
    fn response_time_check_fail() {
        let response = make_response(200, "{}", 3000);
        let assertions = vec![Assertion {
            status: None,
            body: None,
            header: None,
            response_time_ms: Some(ValueCheck {
                lt: Some(2000.0),
                ..Default::default()
            }),
            body_type: None,
            schema: None,
        }];
        let results = evaluate(&assertions, &response);
        assert!(!results[0].passed);
    }

    #[cfg(feature = "schema")]
    fn schema_assertion(schema: SchemaRef) -> Assertion {
        Assertion {
            status: None,
            body: None,
            header: None,
            response_time_ms: None,
            body_type: None,
            schema: Some(schema),
        }
    }

    #[cfg(feature = "schema")]
    #[test]
    fn inline_schema_valid_body_passes() {
        let response = make_response(200, r#"{"id": 42, "email": "a@b.com"}"#, 10);
        let schema = serde_json::json!({
            "type": "object",
            "required": ["id", "email"],
            "properties": {
                "id": { "type": "integer" },
                "email": { "type": "string" }
            }
        });
        let results = evaluate(&[schema_assertion(SchemaRef::Inline(schema))], &response);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed, "got: {:?}", results[0]);
    }

    #[cfg(feature = "schema")]
    #[test]
    fn inline_schema_invalid_body_fails_with_path() {
        let response = make_response(200, r#"{"id": "not-an-int"}"#, 10);
        let schema = serde_json::json!({
            "type": "object",
            "required": ["id", "email"],
            "properties": {
                "id": { "type": "integer" },
                "email": { "type": "string" }
            }
        });
        let results = evaluate(&[schema_assertion(SchemaRef::Inline(schema))], &response);
        assert!(!results[0].passed);
        // Failure message should mention either the missing field or the type mismatch.
        assert!(
            results[0].actual.contains("email")
                || results[0].actual.contains("id")
                || results[0].actual.contains("integer"),
            "failure should name the offending field; got: {}",
            results[0].actual
        );
    }

    #[cfg(feature = "schema")]
    #[test]
    fn schema_non_json_body_fails_cleanly() {
        let response = make_response(200, "<html>not json</html>", 10);
        let schema = serde_json::json!({ "type": "object" });
        let results = evaluate(&[schema_assertion(SchemaRef::Inline(schema))], &response);
        assert!(!results[0].passed);
        assert!(
            results[0].actual.contains("body parse error"),
            "should report body parse failure; got: {}",
            results[0].actual
        );
    }

    #[cfg(feature = "schema")]
    #[test]
    fn schema_file_missing_reports_io_error() {
        let response = make_response(200, "{}", 10);
        let results = evaluate(
            &[schema_assertion(SchemaRef::File(
                "/definitely/does/not/exist.json".into(),
            ))],
            &response,
        );
        assert!(!results[0].passed);
        assert!(
            results[0].actual.contains("io error"),
            "should report io error; got: {}",
            results[0].actual
        );
    }

    #[cfg(feature = "schema")]
    #[test]
    fn schema_file_resolves_relative_to_base_dir() {
        use std::io::Write;
        // Write a schema file to a tempdir and validate against it.
        let tmp = std::env::temp_dir().join(format!(
            "ace_schema_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let schema_path = tmp.join("user.json");
        let mut f = std::fs::File::create(&schema_path).unwrap();
        writeln!(
            f,
            r#"{{"type": "object", "required": ["id"], "properties": {{"id": {{"type": "integer"}}}}}}"#
        )
        .unwrap();

        let response = make_response(200, r#"{"id": 7}"#, 10);
        let results = evaluate_with_base(
            &[schema_assertion(SchemaRef::File("user.json".into()))],
            &response,
            Some(&tmp),
        );
        assert!(results[0].passed, "got: {:?}", results[0]);

        let _ = std::fs::remove_file(&schema_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[cfg(feature = "schema")]
    #[test]
    fn openapi_schema_strict_catches_extra_field() {
        use std::io::Write;
        // Minimal OpenAPI doc with a Subscription component.
        let spec = r#"{
            "openapi": "3.0.0",
            "components": {
                "schemas": {
                    "Subscription": {
                        "type": "object",
                        "required": ["id", "status"],
                        "properties": {
                            "id":     { "type": "string" },
                            "status": { "type": "string" }
                        }
                    }
                }
            }
        }"#;
        let tmp = std::env::temp_dir().join(format!(
            "ace_openapi_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let spec_path = tmp.join("api.json");
        std::fs::File::create(&spec_path)
            .unwrap()
            .write_all(spec.as_bytes())
            .unwrap();

        let ref_ = SchemaRef::OpenApi {
            openapi: "api.json".into(),
            component: "Subscription".into(),
            strict: true,
        };

        // Conforming body — should pass.
        let ok_body = r#"{"id": "sub_123", "status": "active"}"#;
        let results = evaluate_with_base(
            &[schema_assertion(ref_.clone())],
            &make_response(200, ok_body, 10),
            Some(&tmp),
        );
        assert!(results[0].passed, "expected pass, got: {:?}", results[0]);

        // Body with extra field `discounts` — should fail with strict.
        let drift_body = r#"{"id": "sub_123", "status": "active", "discounts": []}"#;
        let results = evaluate_with_base(
            &[schema_assertion(ref_.clone())],
            &make_response(200, drift_body, 10),
            Some(&tmp),
        );
        assert!(
            !results[0].passed,
            "expected fail on extra field, got: {:?}",
            results[0]
        );
        assert!(
            results[0].actual.contains("discounts"),
            "error should mention 'discounts', got: {}",
            results[0].actual
        );

        // Non-strict — extra field should pass.
        let ref_lax = SchemaRef::OpenApi {
            openapi: "api.json".into(),
            component: "Subscription".into(),
            strict: false,
        };
        let results = evaluate_with_base(
            &[schema_assertion(ref_lax)],
            &make_response(200, drift_body, 10),
            Some(&tmp),
        );
        assert!(
            results[0].passed,
            "non-strict should pass extra field, got: {:?}",
            results[0]
        );

        let _ = std::fs::remove_file(&spec_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[cfg(feature = "schema")]
    #[test]
    fn openapi_component_not_found_reports_error() {
        use std::io::Write;
        let spec = r#"{"openapi": "3.0.0", "components": {"schemas": {}}}"#;
        let tmp = std::env::temp_dir().join(format!(
            "ace_openapi_notfound_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let spec_path = tmp.join("api.json");
        std::fs::File::create(&spec_path)
            .unwrap()
            .write_all(spec.as_bytes())
            .unwrap();

        let ref_ = SchemaRef::OpenApi {
            openapi: "api.json".into(),
            component: "Missing".into(),
            strict: false,
        };
        let results = evaluate_with_base(
            &[schema_assertion(ref_)],
            &make_response(200, r#"{"id": 1}"#, 10),
            Some(&tmp),
        );
        assert!(!results[0].passed);
        assert!(
            results[0].actual.contains("Missing"),
            "got: {}",
            results[0].actual
        );

        let _ = std::fs::remove_file(&spec_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[cfg(feature = "schema")]
    #[test]
    fn openapi_strict_and_lax_share_no_cache_entry() {
        // Regression: cache key used to omit `strict`. Asserting the same
        // component twice — first lax, then strict — could return the lax
        // schema for the strict assertion, falsely passing extra fields.
        use std::io::Write;
        let spec = r#"{
            "openapi": "3.0.0",
            "components": {
                "schemas": {
                    "Subscription": {
                        "type": "object",
                        "required": ["id"],
                        "properties": { "id": { "type": "string" } }
                    }
                }
            }
        }"#;
        let tmp = std::env::temp_dir().join(format!(
            "ace_openapi_strict_cache_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let spec_path = tmp.join("api.json");
        std::fs::File::create(&spec_path)
            .unwrap()
            .write_all(spec.as_bytes())
            .unwrap();

        let lax = SchemaRef::OpenApi {
            openapi: "api.json".into(),
            component: "Subscription".into(),
            strict: false,
        };
        let strict = SchemaRef::OpenApi {
            openapi: "api.json".into(),
            component: "Subscription".into(),
            strict: true,
        };
        // Drift body: extra `discounts` field. Lax must accept; strict must reject.
        let drift = r#"{"id": "sub_1", "discounts": []}"#;

        // Share a single cache across both assertions to mimic a real scenario run.
        let cache = SchemaCache::new();
        let lax_results = evaluate_with_cache(
            &[schema_assertion(lax)],
            &make_response(200, drift, 10),
            Some(&tmp),
            &cache,
        );
        let strict_results = evaluate_with_cache(
            &[schema_assertion(strict)],
            &make_response(200, drift, 10),
            Some(&tmp),
            &cache,
        );

        assert!(
            lax_results[0].passed,
            "lax should accept extra field; got: {:?}",
            lax_results[0]
        );
        assert!(
            !strict_results[0].passed,
            "strict must reject extra field even when lax was compiled first; got: {:?}",
            strict_results[0]
        );
        assert!(
            strict_results[0].actual.contains("discounts"),
            "strict failure must mention 'discounts'; got: {}",
            strict_results[0].actual
        );

        let _ = std::fs::remove_file(&spec_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[cfg(feature = "schema")]
    #[test]
    fn inline_schema_cache_canonicalizes_key_order() {
        // Two semantically identical inline schemas with different key insertion
        // order must produce identical cache keys (otherwise the cache leaks).
        let a =
            serde_json::json!({ "type": "object", "properties": { "id": { "type": "integer" } } });
        let b_str = r#"{"properties":{"id":{"type":"integer"}},"type":"object"}"#;
        let b: serde_json::Value = serde_json::from_str(b_str).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }

    #[cfg(feature = "schema")]
    #[test]
    fn cyclic_openapi_component_compiles_and_validates_deep_ref() {
        // End-to-end coverage for the cyclic-$ref fix. A self-referential
        // `Node` schema must (a) compile without infinite recursion and (b)
        // actually validate the second-level field, not just the first hop.
        // Without the synthetic-URI rewrite + with_document() registration,
        // the preserved `#/components/schemas/Node` would resolve relative to
        // the extracted standalone Node schema (no `components` section) and
        // either fail compilation or silently no-op on the nested validation.
        use std::io::Write;
        let spec = r##"{
            "openapi": "3.0.0",
            "components": {
                "schemas": {
                    "Node": {
                        "type": "object",
                        "required": ["value"],
                        "properties": {
                            "value": { "type": "integer" },
                            "next":  { "$ref": "#/components/schemas/Node" }
                        }
                    }
                }
            }
        }"##;
        let tmp = std::env::temp_dir().join(format!(
            "ace_openapi_cyclic_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let spec_path = tmp.join("api.json");
        std::fs::File::create(&spec_path)
            .unwrap()
            .write_all(spec.as_bytes())
            .unwrap();

        let node = SchemaRef::OpenApi {
            openapi: "api.json".into(),
            component: "Node".into(),
            strict: false,
        };

        // (a) Valid: nested chain with correct types.
        let valid_body = r#"{"value": 1, "next": {"value": 2, "next": {"value": 3}}}"#;
        let valid = evaluate_with_base(
            &[schema_assertion(node.clone())],
            &make_response(200, valid_body, 10),
            Some(&tmp),
        );
        // Compile must succeed (Some(root) path) and validation must pass.
        assert!(
            valid[0].passed,
            "valid cyclic body must validate; got {:?}",
            valid[0]
        );

        // (b) Invalid at depth 2: `next.value` is a string, not integer.
        // Catches the case where the deep $ref silently no-ops.
        let bad_body = r#"{"value": 1, "next": {"value": "not-an-int"}}"#;
        let cache = SchemaCache::new();
        let bad = evaluate_with_cache(
            &[schema_assertion(node)],
            &make_response(200, bad_body, 10),
            Some(&tmp),
            &cache,
        );
        assert!(
            !bad[0].passed,
            "deep type violation must fail validation; got passing result {:?}",
            bad[0]
        );

        let _ = std::fs::remove_file(&spec_path);
        let _ = std::fs::remove_dir(&tmp);
    }
}
