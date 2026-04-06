use crate::error::{read_file, write_file, CliError};
use colored::Colorize;
use serde_json::Value;

/// Import a Postman Collection v2.1 JSON and convert to ACE YAML scenarios.
pub fn cmd_import(collection_path: &str, output_dir: &str) -> Result<(), CliError> {
    let json_str = read_file(collection_path)?;

    let collection: Value = serde_json::from_str(&json_str).map_err(CliError::JsonParse)?;

    let name = collection
        .pointer("/info/name")
        .and_then(|v| v.as_str())
        .unwrap_or("imported");

    let items = collection
        .get("item")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if items.is_empty() {
        eprintln!(
            "{} No items found in collection",
            "warning:".yellow().bold()
        );
        return Ok(());
    }

    // Flatten nested folders
    let requests = flatten_items(&items);

    if requests.is_empty() {
        eprintln!(
            "{} No requests found in collection",
            "warning:".yellow().bold()
        );
        return Ok(());
    }

    // Build YAML
    let mut yaml = String::new();
    yaml.push_str(&format!("name: {}\n", sanitize_yaml_str(name)));
    yaml.push_str("initial_state: start\n");
    yaml.push_str("steps:\n");

    let mut prev_state = "start".to_string();

    for (i, req) in requests.iter().enumerate() {
        let req_name = req
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("request");

        let request_obj = req.get("request").unwrap_or(req);

        let method = request_obj
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET");

        let url = extract_url(request_obj);

        let next_state = if i == requests.len() - 1 {
            "done".to_string()
        } else {
            slugify(req_name)
        };

        yaml.push_str(&format!("  - name: {}\n", sanitize_yaml_str(req_name)));
        yaml.push_str(&format!("    method: {}\n", method.to_uppercase()));
        yaml.push_str(&format!("    url: \"{}\"\n", url));

        // Headers
        if let Some(headers) = request_obj.get("header").and_then(|v| v.as_array()) {
            let filtered: Vec<_> = headers
                .iter()
                .filter(|h| !h.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false))
                .collect();

            if !filtered.is_empty() {
                yaml.push_str("    headers:\n");
                for h in &filtered {
                    let key = h.get("key").and_then(|v| v.as_str()).unwrap_or("");
                    let value = h.get("value").and_then(|v| v.as_str()).unwrap_or("");
                    yaml.push_str(&format!(
                        "      {}: \"{}\"\n",
                        key,
                        value.replace('"', "\\\"")
                    ));
                }
            }
        }

        // Body
        if let Some(body) = request_obj.get("body")
            && let Some(raw) = body.get("raw").and_then(|v| v.as_str())
            && let Ok(json_body) = serde_json::from_str::<Value>(raw)
        {
            yaml.push_str("    body:\n");
            let body_yaml = serde_yaml::to_string(&json_body).unwrap_or_default();
            for line in body_yaml.lines() {
                yaml.push_str(&format!("      {}\n", line));
            }
        }

        yaml.push_str("    transition:\n");
        yaml.push_str(&format!("      from: {}\n", prev_state));
        yaml.push_str(&format!("      to: {}\n", next_state));

        prev_state = next_state;
    }

    // Write output
    let filename = format!("{}.yaml", slugify(name));
    let output_path = if output_dir == "." {
        filename.clone()
    } else {
        std::fs::create_dir_all(output_dir).ok();
        format!("{}/{}", output_dir, filename)
    };

    write_file(&output_path, &yaml)?;

    println!(
        "\n{} Imported {} requests from Postman collection",
        "✓".green().bold(),
        requests.len()
    );
    println!("  {} {}", "Output:".bold(), output_path);
    println!(
        "\n  {} Add assertions and extract fields to complete the scenario",
        "Tip:".dimmed()
    );

    Ok(())
}

fn flatten_items(items: &[Value]) -> Vec<Value> {
    let mut result = Vec::new();
    for item in items {
        if item.get("request").is_some() {
            result.push(item.clone());
        }
        if let Some(sub_items) = item.get("item").and_then(|v| v.as_array()) {
            result.extend(flatten_items(sub_items));
        }
    }
    result
}

fn extract_url(request: &Value) -> String {
    if let Some(url) = request.get("url") {
        if let Some(raw) = url.get("raw").and_then(|v| v.as_str()) {
            return convert_postman_vars(raw);
        }
        if let Some(raw) = url.as_str() {
            return convert_postman_vars(raw);
        }
    }
    "http://example.com".to_string()
}

/// Convert Postman's {{var}} syntax — it's the same as ACE's, so just pass through.
/// But Postman uses `:param` for path params, convert those.
fn convert_postman_vars(url: &str) -> String {
    // Replace :param with {{param}} for path parameters
    let mut result = String::new();
    let mut chars = url.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == ':' && result.ends_with('/') {
            // Path parameter
            let mut param = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_alphanumeric() || next == '_' {
                    param.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            if !param.is_empty() {
                result.push_str(&format!("{{{{{}}}}}", param));
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn sanitize_yaml_str(s: &str) -> String {
    if s.contains(':') || s.contains('#') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}
