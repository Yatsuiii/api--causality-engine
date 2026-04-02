use std::collections::HashMap;

/// Resolve all `{{...}}` placeholders in a string.
///
/// Supported patterns:
///   - `{{key}}` — context variable lookup
///   - `{{$env.KEY}}` — environment variable
///   - `{{$uuid}}` — random UUID v4
///   - `{{$timestamp}}` — unix timestamp (seconds)
///   - `{{$randomInt}}` — random integer 0–99999
pub fn resolve_template(template: &str, context: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(template.len());
    let mut remaining = template;

    while let Some(start) = remaining.find("{{") {
        result.push_str(&remaining[..start]);

        if let Some(end) = remaining[start..].find("}}") {
            let key = &remaining[start + 2..start + end];
            let replacement = resolve_key(key.trim(), context);
            result.push_str(&replacement);
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

fn resolve_key(key: &str, context: &HashMap<String, String>) -> String {
    // Built-in dynamic variables
    if key == "$uuid" {
        return uuid::Uuid::new_v4().to_string();
    }
    if key == "$timestamp" {
        return chrono::Utc::now().timestamp().to_string();
    }
    if key == "$randomInt" {
        return rand::random::<u32>().to_string();
    }

    // Environment variable: $env.KEY
    if let Some(env_key) = key.strip_prefix("$env.") {
        return std::env::var(env_key).unwrap_or_default();
    }

    // Context variable
    context.get(key).cloned().unwrap_or_default()
}

/// Resolve all templates in a HashMap of headers/values.
pub fn resolve_map(
    map: &HashMap<String, String>,
    context: &HashMap<String, String>,
) -> HashMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.clone(), resolve_template(v, context)))
        .collect()
}

/// Merge scenario-level variables into a context, resolving any templates.
pub fn build_initial_context(
    variables: Option<&HashMap<String, String>>,
    cli_overrides: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut context = HashMap::new();

    // Start with scenario-declared variables
    if let Some(vars) = variables {
        for (k, v) in vars {
            context.insert(k.clone(), v.clone());
        }
    }

    // CLI overrides take precedence
    for (k, v) in cli_overrides {
        context.insert(k.clone(), v.clone());
    }

    // Now resolve any templates within the context values themselves
    let snapshot: HashMap<String, String> = context.clone();
    for value in context.values_mut() {
        *value = resolve_template(value, &snapshot);
    }

    context
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_unchanged() {
        let ctx = HashMap::new();
        assert_eq!(resolve_template("hello world", &ctx), "hello world");
    }

    #[test]
    fn context_variable() {
        let mut ctx = HashMap::new();
        ctx.insert("token".into(), "abc123".into());
        assert_eq!(resolve_template("Bearer {{token}}", &ctx), "Bearer abc123");
    }

    #[test]
    fn multiple_variables() {
        let mut ctx = HashMap::new();
        ctx.insert("host".into(), "example.com".into());
        ctx.insert("id".into(), "42".into());
        assert_eq!(
            resolve_template("https://{{host}}/users/{{id}}", &ctx),
            "https://example.com/users/42"
        );
    }

    #[test]
    fn env_variable() {
        // SAFETY: single-threaded test, no other threads reading this var
        unsafe { std::env::set_var("ACE_TEST_VAR", "test_value") };
        let ctx = HashMap::new();
        assert_eq!(
            resolve_template("{{$env.ACE_TEST_VAR}}", &ctx),
            "test_value"
        );
    }

    #[test]
    fn uuid_generates_valid_format() {
        let ctx = HashMap::new();
        let result = resolve_template("{{$uuid}}", &ctx);
        assert_eq!(result.len(), 36); // UUID format: 8-4-4-4-12
        assert!(result.contains('-'));
    }

    #[test]
    fn timestamp_is_numeric() {
        let ctx = HashMap::new();
        let result = resolve_template("{{$timestamp}}", &ctx);
        assert!(result.parse::<i64>().is_ok());
    }

    #[test]
    fn missing_variable_becomes_empty() {
        let ctx = HashMap::new();
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
        assert_eq!(ctx.get("base_url").unwrap(), "https://override.com");
        assert_eq!(ctx.get("timeout").unwrap(), "5000");
    }
}
