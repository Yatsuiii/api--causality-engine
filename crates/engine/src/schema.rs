use model::SchemaRef;
use serde_json::Value;
use std::path::Path;

/// Synthetic URI we register the original OpenAPI document under so the
/// `jsonschema` crate can resolve `$ref`s that survived the cycle guard.
/// Local refs (`#/components/schemas/X`) inside the extracted component get
/// rewritten to `<this URI>#/components/schemas/X` so they resolve against
/// the registered root doc rather than the standalone extracted schema (which
/// has no `components` section).
pub const SYNTHETIC_OPENAPI_ROOT_URI: &str = "urn:ace:openapi-root";

#[derive(Debug)]
pub enum SchemaError {
    Io(String),
    Parse(String),
    ComponentNotFound(String),
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::Io(s) => write!(f, "io error: {}", s),
            SchemaError::Parse(s) => write!(f, "parse error: {}", s),
            SchemaError::ComponentNotFound(s) => {
                write!(f, "component not found: #/components/schemas/{}", s)
            }
        }
    }
}

/// Load a JSON or YAML file relative to `base_dir` (or absolute).
pub fn load_json_or_yaml(base_dir: Option<&Path>, path: &str) -> Result<Value, SchemaError> {
    let resolved = match base_dir {
        Some(dir) if !Path::new(path).is_absolute() => dir.join(path),
        _ => Path::new(path).to_path_buf(),
    };
    let contents = std::fs::read_to_string(&resolved)
        .map_err(|e| SchemaError::Io(format!("{}: {}", resolved.display(), e)))?;
    serde_json::from_str::<Value>(&contents)
        .or_else(|_| serde_yaml::from_str::<Value>(&contents))
        .map_err(|e| SchemaError::Parse(e.to_string()))
}

/// Resolve a `SchemaRef` to a `serde_json::Value` representing a JSONSchema.
/// For `OpenApi` refs this extracts the component, inlines local `$ref`s
/// (cycle-safe), and optionally injects `additionalProperties: false`.
/// Returns `(schema_value, root_doc_for_ref_resolution)`.
/// The root doc is `Some` only for `OpenApi` — callers should pass it to
/// `jsonschema::compile` as the URI root so unresolved `$ref`s still work.
pub fn resolve(
    schema_ref: &SchemaRef,
    base_dir: Option<&Path>,
) -> Result<(Value, Option<Value>), SchemaError> {
    match schema_ref {
        SchemaRef::Inline(v) => Ok((v.clone(), None)),
        SchemaRef::File(path) => Ok((load_json_or_yaml(base_dir, path)?, None)),
        SchemaRef::OpenApi {
            openapi,
            component,
            strict,
        } => {
            let mut doc = load_json_or_yaml(base_dir, openapi)?;
            // Normalize OpenAPI 3.0 `nullable: true` → JSON Schema null type on
            // every component schema so the jsonschema crate accepts null values.
            if let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") {
                for v in schemas.values_mut() {
                    normalize_nullable(v);
                }
            }
            if *strict {
                // Inject additionalProperties:false into every component schema
                // in the root doc so it takes effect wherever refs resolve.
                // Also strip `required` arrays: strict mode catches EXTRA fields
                // (schema drift), not missing ones. Server mocks often omit
                // required fields; failing on those is noise not signal.
                if let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") {
                    for v in schemas.values_mut() {
                        apply_strict(v);
                        strip_required(v);
                    }
                }
            }
            let schema = extract_component(&doc, component)?;
            let resolved = rewrite_refs(schema);
            Ok((resolved, Some(doc)))
        }
    }
}

fn extract_component(doc: &Value, name: &str) -> Result<Value, SchemaError> {
    doc.pointer(&format!("/components/schemas/{name}"))
        .cloned()
        .ok_or_else(|| SchemaError::ComponentNotFound(name.to_string()))
}

/// Walk `schema` and rewrite every local `$ref` (`#/...`) to the synthetic
/// root URI (`urn:ace:openapi-root#/...`). This is O(nodes) with no
/// recursion explosion — we never expand refs inline. The jsonschema
/// compiler resolves them against the root document registered via
/// `with_document`. Replacing the old `inline_refs` expansion avoids
/// blowing up on large specs (e.g. Stripe's 7 MB OpenAPI file) where full
/// inlining produces hundreds of MB of JSON.
fn rewrite_refs(schema: Value) -> Value {
    match schema {
        Value::Object(mut map) => {
            if let Some(Value::String(ref_str)) = map.get("$ref") {
                if ref_str.starts_with('#') {
                    let rewritten = format!("{SYNTHETIC_OPENAPI_ROOT_URI}{ref_str}");
                    map.insert("$ref".to_string(), Value::String(rewritten));
                }
                return Value::Object(map);
            }
            for val in map.values_mut() {
                *val = rewrite_refs(val.take());
            }
            Value::Object(map)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(rewrite_refs).collect()),
        other => other,
    }
}

/// Convert OpenAPI 3.0 `nullable: true` to JSON Schema null-union in place.
/// OpenAPI uses `nullable: true` as a sibling to `type`/`$ref`/`anyOf`; JSON
/// Schema has no such keyword. Three patterns handled:
///   - `{anyOf: [...], nullable: true}` → append `{type: null}` to anyOf
///   - `{type: "T", nullable: true}`    → `type: ["T", "null"]`
///   - `{$ref: "...", nullable: true}`  → `{anyOf: [{$ref: "..."}, {type: "null"}]}`
fn normalize_nullable(schema: &mut Value) {
    let Value::Object(map) = schema else { return };

    // Recurse first so children are already normalized.
    for val in map.values_mut() {
        normalize_nullable(val);
    }

    if !matches!(map.get("nullable"), Some(Value::Bool(true))) {
        return;
    }
    map.remove("nullable");

    if let Some(Value::Array(any_of)) = map.get_mut("anyOf") {
        any_of.push(serde_json::json!({"type": "null"}));
    } else if let Some(ref_val) = map.remove("$ref") {
        let branches = vec![
            serde_json::json!({"$ref": ref_val}),
            serde_json::json!({"type": "null"}),
        ];
        map.insert("anyOf".to_string(), Value::Array(branches));
    } else {
        // enum + nullable: add null to enum so null passes validation.
        if let Some(Value::Array(enum_vals)) = map.get_mut("enum") {
            if !enum_vals.contains(&Value::Null) {
                enum_vals.push(Value::Null);
            }
        }
        // type + nullable: widen scalar type to accept null.
        if let Some(Value::String(t)) = map.get("type").cloned() {
            map.insert("type".to_string(), serde_json::json!([t, "null"]));
        }
    }
    // If nothing matched, leave as-is (validator will decide).
}

/// Recursively remove all `required` arrays from a schema tree.
fn strip_required(schema: &mut Value) {
    let Value::Object(map) = schema else { return };
    map.remove("required");
    for val in map.values_mut() {
        strip_required(val);
    }
}

/// Walk `schema` and inject `"additionalProperties": false` on every
/// `type: object` node (or any object node without an explicit type) that
/// doesn't already have `additionalProperties` set. Recurses into
/// `properties` values, `items`, and `oneOf`/`anyOf`/`allOf` branches.
pub fn apply_strict(schema: &mut Value) {
    let Value::Object(map) = schema else { return };

    let is_object = match map.get("type") {
        Some(Value::String(t)) => t == "object",
        None => map.contains_key("properties"),
        _ => false,
    };

    if is_object && !map.contains_key("additionalProperties") {
        map.insert("additionalProperties".to_string(), Value::Bool(false));
    }

    // Recurse into properties values.
    if let Some(Value::Object(props)) = map.get_mut("properties") {
        for v in props.values_mut() {
            apply_strict(v);
        }
    }

    // Recurse into items (arrays).
    if let Some(items) = map.get_mut("items") {
        apply_strict(items);
    }

    // Recurse into composition keywords.
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(Value::Array(branches)) = map.get_mut(key) {
            for branch in branches.iter_mut() {
                apply_strict(branch);
            }
        }
    }

    // Recurse into anyOf/oneOf inside additionalProperties if it's an object.
    if let Some(ap) = map
        .get_mut("additionalProperties")
        .filter(|v| v.is_object())
    {
        apply_strict(ap);
    }
}

// ---------------------------------------------------------------------------
// Validation-error reshaping (P0.4 task 4 — schema-failure renderer)
// ---------------------------------------------------------------------------

/// Structured shape for a single jsonschema validation error.
///
/// `jsonschema`'s stock English ("Additional properties are not allowed
/// ('X' was unexpected) at /") buries the actionable signal — the field name —
/// inside boilerplate. We pattern-match on the three high-signal shapes, then
/// fall through to the raw text for anything we don't recognize so we never
/// silently swallow detail.
///
/// The renderer (text/markdown/json) consumes this enum so a single audit of
/// the wording can land in all three formats simultaneously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaErrorShape {
    /// `+ unexpected field: foo` — a `strict: true` schema rejected an extra
    /// property on the response. The actionable signal is the field name.
    UnexpectedField { field: String, path: String },
    /// `- missing required field: foo` — a required property is absent.
    MissingField { field: String, path: String },
    /// `~ type mismatch at $.path: expected integer, got string ("42")` — a
    /// value's runtime type does not match the schema's declared type.
    TypeMismatch {
        path: String,
        expected: String,
        actual_type: String,
        actual_value: String,
    },
    /// Fallback: jsonschema produced a message we don't have a tighter
    /// rendering for. Preserved verbatim so we don't lose information.
    Other(String),
}

impl SchemaErrorShape {
    /// Render to a single line suitable for terminal output. Markdown and
    /// JSON renderers can consume the structured enum directly.
    pub fn render_text(&self) -> String {
        match self {
            SchemaErrorShape::UnexpectedField { field, path } => {
                if path.is_empty() || path == "/" {
                    format!("+ unexpected field: {field}")
                } else {
                    format!("+ unexpected field: {field} at {path}")
                }
            }
            SchemaErrorShape::MissingField { field, path } => {
                if path.is_empty() || path == "/" {
                    format!("- missing required field: {field}")
                } else {
                    format!("- missing required field: {field} at {path}")
                }
            }
            SchemaErrorShape::TypeMismatch {
                path,
                expected,
                actual_type,
                actual_value,
            } => {
                let where_ = if path.is_empty() { "$" } else { path.as_str() };
                format!(
                    "~ type mismatch at {where_}: expected {expected}, got {actual_type} ({actual_value})"
                )
            }
            SchemaErrorShape::Other(msg) => msg.clone(),
        }
    }
}

/// Reshape a single jsonschema-produced message + instance_path into a
/// `SchemaErrorShape`. `raw` is the `Display` form of `jsonschema::ValidationError`;
/// `instance_path` is the JSON-pointer path string (e.g. `/foo/0/bar`).
///
/// The matching is intentionally permissive — jsonschema's wording has shifted
/// across versions, so we look for the unambiguous markers (`'X' was unexpected`,
/// ` is a required property`, ` is not of type `) rather than full prefixes.
pub fn format_validation_error(raw: &str, instance_path: &str) -> SchemaErrorShape {
    let path = if instance_path.is_empty() {
        "/".to_string()
    } else {
        instance_path.to_string()
    };

    // "Additional properties are not allowed ('discounts' was unexpected)"
    // "Additional properties are not allowed ('a', 'b' were unexpected)"
    if raw.contains("Additional properties are not allowed") {
        // Extract all single-quoted field names from the message.
        let fields: Vec<String> = raw
            .split('\'')
            .enumerate()
            .filter_map(|(i, s)| {
                if i % 2 == 1 && !s.is_empty() {
                    Some(s.to_string())
                } else {
                    None
                }
            })
            .collect();
        if let Some(first) = fields.first() {
            if fields.len() == 1 {
                return SchemaErrorShape::UnexpectedField {
                    field: first.clone(),
                    path,
                };
            }
            // Multiple fields: render as "unexpected field: a; unexpected field: b"
            // via Other so all names appear in the diff output.
            let rendered = fields
                .iter()
                .map(|f| {
                    if path == "/" {
                        format!("+ unexpected field: {f}")
                    } else {
                        format!("+ unexpected field: {f} at {path}")
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            return SchemaErrorShape::Other(rendered);
        }
    }

    // "\"foo\" is a required property"
    if let Some(field) = extract_quoted(raw, "\"", "\" is a required property") {
        return SchemaErrorShape::MissingField { field, path };
    }

    // "\"42\" is not of type \"integer\""
    if let Some(idx) = raw.find(" is not of type ") {
        let value_part_raw = raw[..idx].trim();
        let type_part = raw[idx + " is not of type ".len()..]
            .trim_matches('"')
            .trim_end_matches(['.', ' ']);
        // Detect type from the *raw* rendering before quote-stripping. A
        // leading `"` means the value is a JSON string; numbers/bools/null
        // come through unquoted.
        let actual_type = guess_actual_type_from_raw(value_part_raw);
        let actual_value = value_part_raw.trim_matches('"').to_string();
        return SchemaErrorShape::TypeMismatch {
            path,
            expected: type_part.to_string(),
            actual_type,
            actual_value,
        };
    }

    SchemaErrorShape::Other(format!("{raw} at {path}"))
}

/// Return the substring between `open` and `close` if both markers exist in
/// order. Used to lift `'discounts'` out of "...('discounts' was unexpected)".
fn extract_quoted(haystack: &str, open: &str, close: &str) -> Option<String> {
    let start = haystack.find(open)?;
    let after_open = &haystack[start + open.len()..];
    let end = after_open.find(close)?;
    Some(after_open[..end].to_string())
}

/// Best-effort guess at the runtime JSON type of a value as rendered by
/// jsonschema. Inspects the *raw* form (with quotes intact) because that's
/// the only signal that distinguishes `"42"` (string) from `42` (number).
fn guess_actual_type_from_raw(rendered: &str) -> String {
    let t = rendered.trim();
    if t.starts_with('"') {
        "string".into()
    } else if t == "null" {
        "null".into()
    } else if t == "true" || t == "false" {
        "boolean".into()
    } else if t.starts_with('[') {
        "array".into()
    } else if t.starts_with('{') {
        "object".into()
    } else if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        "number".into()
    } else {
        "string".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_component_found() {
        let doc = json!({
            "components": {
                "schemas": {
                    "Foo": { "type": "object", "properties": { "id": { "type": "integer" } } }
                }
            }
        });
        let s = extract_component(&doc, "Foo").unwrap();
        assert_eq!(s["type"], "object");
    }

    #[test]
    fn extract_component_not_found() {
        let doc = json!({ "components": { "schemas": {} } });
        assert!(matches!(
            extract_component(&doc, "Missing"),
            Err(SchemaError::ComponentNotFound(_))
        ));
    }

    #[test]
    fn rewrite_refs_rewrites_local_refs() {
        let schema = json!({
            "type": "object",
            "properties": {
                "addr": { "$ref": "#/components/schemas/Address" }
            }
        });
        let result = rewrite_refs(schema);
        assert_eq!(
            result["properties"]["addr"]["$ref"],
            format!("{SYNTHETIC_OPENAPI_ROOT_URI}#/components/schemas/Address")
        );
    }

    #[test]
    fn rewrite_refs_leaves_external_refs_alone() {
        let schema = json!({ "$ref": "https://example.com/schema.json" });
        let result = rewrite_refs(schema);
        assert_eq!(result["$ref"], "https://example.com/schema.json");
    }

    #[test]
    fn rewrite_refs_cycle_safe() {
        // Deep nesting should not stack-overflow — rewrite_refs never recurses
        // into the value of a $ref, it just rewrites the string.
        let schema = json!({
            "type": "object",
            "properties": {
                "next": { "$ref": "#/components/schemas/Node" }
            }
        });
        let result = rewrite_refs(schema);
        assert_eq!(
            result["properties"]["next"]["$ref"],
            format!("{SYNTHETIC_OPENAPI_ROOT_URI}#/components/schemas/Node")
        );
    }

    #[test]
    fn apply_strict_top_level() {
        let mut s = json!({ "type": "object", "properties": { "id": { "type": "integer" } } });
        apply_strict(&mut s);
        assert_eq!(s["additionalProperties"], false);
    }

    #[test]
    fn apply_strict_nested() {
        let mut s = json!({
            "type": "object",
            "properties": {
                "address": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } }
                }
            }
        });
        apply_strict(&mut s);
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["properties"]["address"]["additionalProperties"], false);
    }

    #[test]
    fn apply_strict_does_not_overwrite_existing() {
        let mut s = json!({
            "type": "object",
            "additionalProperties": true,
            "properties": {}
        });
        apply_strict(&mut s);
        assert_eq!(s["additionalProperties"], true);
    }

    #[test]
    fn apply_strict_inside_oneof() {
        let mut s = json!({
            "oneOf": [
                { "type": "object", "properties": { "a": { "type": "string" } } },
                { "type": "string" }
            ]
        });
        apply_strict(&mut s);
        assert_eq!(s["oneOf"][0]["additionalProperties"], false);
        assert!(s["oneOf"][1].get("additionalProperties").is_none());
    }

    #[test]
    fn apply_strict_no_type_with_properties() {
        let mut s = json!({ "properties": { "id": { "type": "integer" } } });
        apply_strict(&mut s);
        assert_eq!(s["additionalProperties"], false);
    }

    // -----------------------------------------------------------------------
    // format_validation_error
    // -----------------------------------------------------------------------

    #[test]
    fn format_unexpected_field_at_root() {
        let shape = format_validation_error(
            "Additional properties are not allowed ('discounts' was unexpected)",
            "",
        );
        assert_eq!(
            shape,
            SchemaErrorShape::UnexpectedField {
                field: "discounts".into(),
                path: "/".into(),
            }
        );
        assert_eq!(shape.render_text(), "+ unexpected field: discounts");
    }

    #[test]
    fn format_missing_required_field() {
        let shape = format_validation_error("\"id\" is a required property", "/items/0");
        assert_eq!(
            shape,
            SchemaErrorShape::MissingField {
                field: "id".into(),
                path: "/items/0".into(),
            }
        );
        assert_eq!(
            shape.render_text(),
            "- missing required field: id at /items/0"
        );
    }

    #[test]
    fn format_type_mismatch_string_for_integer() {
        let shape = format_validation_error(
            "\"42\" is not of type \"integer\"",
            "/subscription/quantity",
        );
        match &shape {
            SchemaErrorShape::TypeMismatch {
                path,
                expected,
                actual_type,
                actual_value,
            } => {
                assert_eq!(path, "/subscription/quantity");
                assert_eq!(expected, "integer");
                assert_eq!(actual_type, "string");
                assert_eq!(actual_value, "42");
            }
            other => panic!("expected TypeMismatch, got {:?}", other),
        }
    }

    #[test]
    fn format_unrecognized_falls_through_to_other() {
        let shape = format_validation_error(
            "everything is on fire and nothing makes sense",
            "/somewhere",
        );
        match shape {
            SchemaErrorShape::Other(msg) => {
                assert!(msg.contains("everything is on fire"));
                assert!(msg.contains("/somewhere"));
            }
            other => panic!("expected Other, got {:?}", other),
        }
    }
}
