use ace_http::HttpResponse;
use model::{Assertion, StatusCheck, ValueCheck};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::jsonpath;

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
        }];
        let results = evaluate(&assertions, &response);
        assert!(!results[0].passed);
    }
}
