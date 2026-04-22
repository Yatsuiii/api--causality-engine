use serde_json::Value;
use std::collections::HashMap;
use tracing::warn;

/// Typed variable context. Values preserve their native JSON type so that
/// numeric comparisons (lt/gt) and equality checks work correctly without
/// relying on string parsing.
pub type Context = HashMap<String, Value>;

/// Serialize a `Value` to the string used when interpolating into a template.
/// Strings are returned as-is; other types use their JSON representation.
pub fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Parse a bare string into the most specific scalar type that fits.
/// Used when loading scenario-declared variables (which are always YAML
/// strings) into the typed context.
fn parse_scalar(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_owned()))
}

/// Resolve all `{{...}}` placeholders in a string.
///
/// Supported patterns:
///   - `{{key}}` — context variable lookup
///   - `{{$env.KEY}}` — environment variable
///   - `{{$uuid}}` — random UUID v4
///   - `{{$timestamp}}` — unix timestamp (seconds)
///   - `{{$randomInt}}` — random integer 0–99999
pub fn resolve_template(template: &str, context: &Context) -> String {
    let mut result = String::with_capacity(template.len());
    let mut remaining = template;

    while let Some(start) = remaining.find("{{") {
        result.push_str(&remaining[..start]);

        if let Some(end) = remaining[start..].find("}}") {
            let key = &remaining[start + 2..start + end];
            let trimmed = key.trim();
            match resolve_key(trimmed, context) {
                Some(replacement) => result.push_str(&replacement),
                None => {
                    warn!(
                        var = trimmed,
                        "Template variable is undefined; interpolating empty string"
                    );
                }
            }
            remaining = &remaining[start + end + 2..];
        } else {
            // No closing }}, just push the rest
            result.push_str(&remaining[start..]);
            remaining = "";
        }
    }

    result.push_str(remaining);
    result
}

/// Returns `None` when the variable is undefined (so callers can warn),
/// `Some("")` when intentionally empty, `Some(value)` when resolved.
fn resolve_key(key: &str, context: &Context) -> Option<String> {
    // Built-in dynamic variables — always resolve
    if key == "$uuid" {
        return Some(uuid::Uuid::new_v4().to_string());
    }
    if key == "$timestamp" {
        return Some(chrono::Utc::now().timestamp().to_string());
    }
    if key == "$randomInt" {
        use rand::Rng;
        return Some(rand::thread_rng().gen_range(0..=99999).to_string());
    }

    // Environment variable: $env.KEY — missing env → None (warn)
    if let Some(env_key) = key.strip_prefix("$env.") {
        return std::env::var(env_key).ok();
    }

    // Context variable — missing key → None (warn)
    context.get(key).map(value_to_string)
}

/// Resolve all templates in a HashMap of headers/values.
pub fn resolve_map(map: &HashMap<String, String>, context: &Context) -> HashMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.clone(), resolve_template(v, context)))
        .collect()
}

/// Merge scenario-level variables and CLI overrides into a typed context.
///
/// Scenario variables declared as YAML strings are parsed into their native
/// types (bool, number, string). CLI overrides are always treated as strings
/// because the shell has no type information.
pub fn build_initial_context(
    variables: Option<&HashMap<String, String>>,
    cli_overrides: &HashMap<String, String>,
) -> Context {
    let mut context = Context::new();

    if let Some(vars) = variables {
        for (k, v) in vars {
            context.insert(k.clone(), parse_scalar(v));
        }
    }

    for (k, v) in cli_overrides {
        context.insert(k.clone(), Value::String(v.clone()));
    }

    let snapshot = context.clone();
    for value in context.values_mut() {
        if let Value::String(s) = value {
            *s = resolve_template(s, &snapshot);
        }
    }

    context
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn str_ctx(pairs: &[(&str, &str)]) -> Context {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn plain_text_unchanged() {
        let ctx = Context::new();
        assert_eq!(resolve_template("hello world", &ctx), "hello world");
    }

    #[test]
    fn context_variable() {
        let ctx = str_ctx(&[("token", "abc123")]);
        assert_eq!(resolve_template("Bearer {{token}}", &ctx), "Bearer abc123");
    }

    #[test]
    fn multiple_variables() {
        let ctx = str_ctx(&[("host", "example.com"), ("id", "42")]);
        assert_eq!(
            resolve_template("https://{{host}}/users/{{id}}", &ctx),
            "https://example.com/users/42"
        );
    }

    #[test]
    fn numeric_variable_interpolates_as_string() {
        let mut ctx = Context::new();
        ctx.insert("count".into(), Value::Number(42.into()));
        assert_eq!(resolve_template("count={{count}}", &ctx), "count=42");
    }

    #[test]
    fn bool_variable_interpolates_as_string() {
        let mut ctx = Context::new();
        ctx.insert("flag".into(), Value::Bool(true));
        assert_eq!(resolve_template("flag={{flag}}", &ctx), "flag=true");
    }

    #[test]
    fn env_variable() {
        // SAFETY: single-threaded test, no other threads reading this var
        unsafe { std::env::set_var("ACE_TEST_VAR", "test_value") };
        let ctx = Context::new();
        assert_eq!(
            resolve_template("{{$env.ACE_TEST_VAR}}", &ctx),
            "test_value"
        );
    }

    #[test]
    fn uuid_generates_valid_format() {
        let ctx = Context::new();
        let result = resolve_template("{{$uuid}}", &ctx);
        assert_eq!(result.len(), 36);
        assert!(result.contains('-'));
    }

    #[test]
    fn timestamp_is_numeric() {
        let ctx = Context::new();
        let result = resolve_template("{{$timestamp}}", &ctx);
        assert!(result.parse::<i64>().is_ok());
    }

    #[test]
    fn missing_variable_becomes_empty() {
        let ctx = Context::new();
        assert_eq!(resolve_template("{{missing}}", &ctx), "");
    }

    #[test]
    fn build_context_with_overrides() {
        let mut vars = HashMap::new();
        vars.insert("base_url".into(), "https://default.com".into());
        vars.insert("timeout".into(), "5000".into());

        let mut overrides = HashMap::new();
        overrides.insert("base_url".into(), "https://override.com".into());

        let ctx = build_initial_context(Some(&vars), &overrides);
        assert_eq!(
            ctx.get("base_url").unwrap(),
            &Value::String("https://override.com".into())
        );
        // "5000" should be parsed as a number
        assert_eq!(ctx.get("timeout").unwrap(), &Value::Number(5000.into()));
    }

    #[test]
    fn parse_scalar_bool() {
        assert_eq!(parse_scalar("true"), Value::Bool(true));
        assert_eq!(parse_scalar("false"), Value::Bool(false));
    }

    #[test]
    fn parse_scalar_integer() {
        assert_eq!(parse_scalar("42"), Value::Number(42.into()));
    }

    #[test]
    fn parse_scalar_string() {
        assert_eq!(parse_scalar("hello"), Value::String("hello".to_owned()));
    }
}
