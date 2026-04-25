use model::SchemaRef;
use serde_json::Value;
use std::collections::HashSet;
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
            let doc = load_json_or_yaml(base_dir, openapi)?;
            let schema = extract_component(&doc, component)?;
            let mut resolved = inline_refs(schema, &doc, &mut HashSet::new());
            if *strict {
                apply_strict(&mut resolved);
            }
            Ok((resolved, Some(doc)))
        }
    }
}

fn extract_component(doc: &Value, name: &str) -> Result<Value, SchemaError> {
    doc.pointer(&format!("/components/schemas/{name}"))
        .cloned()
        .ok_or_else(|| SchemaError::ComponentNotFound(name.to_string()))
}

/// Walk `schema`, replacing `{"$ref": "#/components/schemas/X"}` with the
/// resolved subschema from `root`. Tracks visited refs to break cycles; on a
/// cycle the `$ref` object is preserved but rewritten to point at the
/// synthetic root URI (`urn:ace:openapi-root#/...`) so the `jsonschema`
/// crate can resolve it against the registered root document at compile
/// time. Without that rewrite, the bare `#/components/schemas/X` would
/// resolve relative to the extracted standalone schema (which has no
/// `components` section) and fail to compile.
fn inline_refs(schema: Value, root: &Value, visiting: &mut HashSet<String>) -> Value {
    match schema {
        Value::Object(mut map) => {
            if let Some(Value::String(ref_str)) = map.get("$ref") {
                let ref_str = ref_str.clone();
                if let Some(ptr) = local_ref_to_pointer(&ref_str) {
                    if visiting.contains(&ref_str) {
                        // Cycle: rewrite to the synthetic root URI so the
                        // jsonschema compiler can resolve it via the
                        // registered root document.
                        map.insert(
                            "$ref".to_string(),
                            Value::String(format!("{SYNTHETIC_OPENAPI_ROOT_URI}{ref_str}")),
                        );
                        return Value::Object(map);
                    }
                    if let Some(target) = root.pointer(&ptr) {
                        visiting.insert(ref_str.clone());
                        let inlined = inline_refs(target.clone(), root, visiting);
                        visiting.remove(&ref_str);
                        return inlined;
                    }
                }
                // Non-local ref or pointer not found — leave as-is.
                return Value::Object(map);
            }
            for val in map.values_mut() {
                *val = inline_refs(val.take(), root, visiting);
            }
            Value::Object(map)
        }
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|v| inline_refs(v, root, visiting))
                .collect(),
        ),
        other => other,
    }
}

/// Convert a local JSON Reference like `#/components/schemas/Foo` to a
/// JSON Pointer `/components/schemas/Foo`.
fn local_ref_to_pointer(ref_str: &str) -> Option<String> {
    ref_str.strip_prefix('#').map(|s| s.to_string())
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
    if raw.contains("Additional properties are not allowed")
        && let Some(field) = extract_quoted(raw, "'", "' was unexpected")
    {
        return SchemaErrorShape::UnexpectedField { field, path };
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
    fn inline_refs_simple() {
        let root = json!({
            "components": {
                "schemas": {
                    "Address": { "type": "object", "properties": { "city": { "type": "string" } } }
                }
            }
        });
        let schema = json!({
            "type": "object",
            "properties": {
                "addr": { "$ref": "#/components/schemas/Address" }
            }
        });
        let result = inline_refs(schema, &root, &mut HashSet::new());
        let addr = &result["properties"]["addr"];
        assert_eq!(addr["type"], "object");
        assert!(addr.get("$ref").is_none());
    }

    #[test]
    fn inline_refs_cycle() {
        // LinkedList node references itself.
        let root = json!({
            "components": {
                "schemas": {
                    "Node": {
                        "type": "object",
                        "properties": {
                            "next": { "$ref": "#/components/schemas/Node" }
                        }
                    }
                }
            }
        });
        let schema = root["components"]["schemas"]["Node"].clone();
        // Should not stack-overflow. The first-level ref is expanded once;
        // the nested forward ref is preserved as a $ref string so the
        // jsonschema compiler can resolve it against the root doc.
        let result = inline_refs(schema, &root, &mut HashSet::new());
        // First expansion: next becomes the Node object.
        assert_eq!(result["properties"]["next"]["type"], "object");
        // Second level: the nested next ref is preserved (cycle guard fired)
        // but rewritten to the synthetic root URI so the jsonschema compiler
        // can resolve it via the registered root document.
        let nested_next = &result["properties"]["next"]["properties"]["next"];
        assert_eq!(
            nested_next["$ref"],
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
