use serde_json::Value;

/// Resolve a dot-notation path with optional array indexing.
///
/// Examples:
///   - `"id"` → `json["id"]`
///   - `"data.user.name"` → `json["data"]["user"]["name"]`
///   - `"items[0].id"` → `json["items"][0]["id"]`
///   - `"a.b[2].c"` → `json["a"]["b"][2]["c"]`
pub fn resolve(json: &Value, path: &str) -> Option<Value> {
    let mut current = json;

    for segment in parse_segments(path) {
        match segment {
            Segment::Key(key) => {
                current = current.get(key)?;
            }
            Segment::Index(idx) => {
                current = current.get(idx)?;
            }
        }
    }

    Some(current.clone())
}

/// Extract a string value from JSON using a dot-notation path.
pub fn extract_string(json: &Value, path: &str) -> Option<String> {
    let value = resolve(json, path)?;
    Some(match &value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Path parser
// ---------------------------------------------------------------------------

enum Segment<'a> {
    Key(&'a str),
    Index(usize),
}

fn parse_segments(path: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();

    for part in path.split('.') {
        if let Some(bracket_pos) = part.find('[') {
            // Key before the bracket
            let key = &part[..bracket_pos];
            if !key.is_empty() {
                segments.push(Segment::Key(key));
            }

            // Parse all bracket indices like [0][1]
            let rest = &part[bracket_pos..];
            let mut i = 0;
            let bytes = rest.as_bytes();
            while i < bytes.len() {
                if bytes[i] == b'[' {
                    if let Some(close) = rest[i..].find(']') {
                        let idx_str = &rest[i + 1..i + close];
                        if let Ok(idx) = idx_str.parse::<usize>() {
                            segments.push(Segment::Index(idx));
                        }
                        i += close + 1;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else {
            segments.push(Segment::Key(part));
        }
    }

    segments
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn simple_key() {
        let data = json!({"id": 42});
        assert_eq!(resolve(&data, "id"), Some(json!(42)));
    }

    #[test]
    fn nested_key() {
        let data = json!({"data": {"user": {"name": "Alice"}}});
        assert_eq!(resolve(&data, "data.user.name"), Some(json!("Alice")));
    }

    #[test]
    fn array_index() {
        let data = json!({"items": [{"id": 1}, {"id": 2}]});
        assert_eq!(resolve(&data, "items[0].id"), Some(json!(1)));
        assert_eq!(resolve(&data, "items[1].id"), Some(json!(2)));
    }

    #[test]
    fn missing_key() {
        let data = json!({"id": 1});
        assert_eq!(resolve(&data, "nonexistent"), None);
    }

    #[test]
    fn deep_nested_with_arrays() {
        let data = json!({
            "response": {
                "data": [
                    {"users": [{"name": "Alice"}, {"name": "Bob"}]}
                ]
            }
        });
        assert_eq!(
            resolve(&data, "response.data[0].users[1].name"),
            Some(json!("Bob"))
        );
    }

    #[test]
    fn extract_string_values() {
        let data = json!({"name": "Alice", "age": 30});
        assert_eq!(extract_string(&data, "name"), Some("Alice".into()));
        assert_eq!(extract_string(&data, "age"), Some("30".into()));
    }
}
