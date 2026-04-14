use crate::error::{CliError, read_file, write_file};
use colored::Colorize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn cmd_import(collection_path: &str, output_dir: &str) -> Result<(), CliError> {
    let json_str = read_file(collection_path)?;
    let collection: Value = serde_json::from_str(&json_str).map_err(CliError::JsonParse)?;

    let name = collection
        .pointer("/info/name")
        .and_then(|v| v.as_str())
        .unwrap_or("imported");

    // Fix 1: collect collection-level variables (non-empty values only)
    let coll_vars: Vec<(String, String)> = collection
        .get("variable")
        .and_then(|v| v.as_array())
        .map(|vars| {
            vars.iter()
                .filter_map(|v| {
                    let key = v.get("key")?.as_str()?;
                    let value = v.get("value")?.as_str().unwrap_or("");
                    if value.is_empty() {
                        return None;
                    }
                    Some((key.to_string(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

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

    // Fix 6: flatten while tracking which folder each request came from
    let requests = flatten_with_folders(&items, None);

    if requests.is_empty() {
        eprintln!(
            "{} No requests found in collection",
            "warning:".yellow().bold()
        );
        return Ok(());
    }

    let yaml = build_yaml(name, &coll_vars, &requests);

    let filename = format!("{}.yaml", slugify(name));
    let output_path = if output_dir == "." {
        filename
    } else {
        std::fs::create_dir_all(output_dir).map_err(|e| CliError::Io {
            path: output_dir.to_string(),
            source: e,
        })?;
        format!("{}/{}", output_dir, filename)
    };
    write_file(&output_path, &yaml)?;

    // Fix 2: surface a warning for every step that had untranslatable scripts
    let warned: Vec<&ImportedRequest> = requests
        .iter()
        .filter(|r| {
            !r.test_script.untranslatable.is_empty() || !r.pre_script_untranslatable.is_empty()
        })
        .collect();

    println!(
        "\n{} Imported {} requests from Postman collection",
        "✓".green().bold(),
        requests.len()
    );
    println!("  {} {}", "Output:".bold(), output_path);

    if !warned.is_empty() {
        eprintln!(
            "\n  {} {} step(s) had untranslatable scripts — see # WARN comments in output",
            "warning:".yellow().bold(),
            warned.len()
        );
        for r in &warned {
            eprintln!("    • {}", r.name);
        }
    }

    println!(
        "\n  {} Review extract: and assert: fields before running",
        "Tip:".dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

struct ImportedRequest {
    name: String,
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    /// Ready-to-emit indented YAML lines for the body block (no `body:` header).
    body_yaml: Option<String>,
    /// True when `{{var}}` in a non-string JSON position was coerced to a string.
    body_had_coercion: bool,
    /// Original raw body kept for a fallback comment when we couldn't parse it.
    body_raw_fallback: Option<String>,
    /// Which Postman folder this request lived in (for comment markers).
    folder: Option<String>,
    test_script: ScriptInfo,
    /// Translated `pm.variables.set()` calls → ACE `pre_request: - set:` entries.
    pre_request_sets: Vec<(String, String)>,
    /// Pre-request lines that couldn't be translated (emitted as # WARN).
    pre_script_untranslatable: Vec<String>,
}

#[derive(Default)]
struct ScriptInfo {
    status: Option<u16>,
    /// Body fields asserted to exist (`pm.expect(json.field).to.exist`).
    body_exists: Vec<String>,
    /// Body field equality checks (`pm.expect(json.field).to.equal(value)`): (field, yaml_value).
    body_eq: Vec<(String, String)>,
    /// `pm.environment.set()` calls that resolved to a simple json-path.
    extractions: Vec<(String, String)>,
    /// Computed sets needing a post_request hook: (final_var, raw_var, template).
    computed_sets: Vec<(String, String, String)>,
    /// Root body type assertion (`pm.expect(json).to.be.an('array')`).
    body_type: Option<String>,
    /// Field-level type assertions (`pm.expect(json.field).to.be.a('string')`): (field, type).
    body_field_types: Vec<(String, String)>,
    /// Response-time upper bound from `.to.be.below(N)`.
    response_time_lt: Option<u64>,
    /// JS lines we could not translate — emitted as # WARN comments.
    untranslatable: Vec<String>,
}

// ---------------------------------------------------------------------------
// Flatten Postman items while recording the enclosing folder name
// ---------------------------------------------------------------------------

fn flatten_with_folders(items: &[Value], folder: Option<&str>) -> Vec<ImportedRequest> {
    let mut result = Vec::new();
    for item in items {
        if item.get("request").is_some() {
            if let Some(req) = parse_request(item, folder) {
                result.push(req);
            }
        } else if let Some(sub) = item.get("item").and_then(|v| v.as_array()) {
            let folder_name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");
            result.extend(flatten_with_folders(sub, Some(folder_name)));
        }
    }
    result
}

fn parse_request(item: &Value, folder: Option<&str>) -> Option<ImportedRequest> {
    let name = item.get("name")?.as_str()?.to_string();
    let request = item.get("request").unwrap_or(item);

    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET")
        .to_uppercase();

    let url = extract_url(request);

    let headers: Vec<(String, String)> = request
        .get("header")
        .and_then(|v| v.as_array())
        .map(|hs| {
            hs.iter()
                .filter(|h| !h.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false))
                .filter_map(|h| {
                    let key = h.get("key")?.as_str()?;
                    let value = h.get("value")?.as_str().unwrap_or("");
                    Some((key.to_string(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    // Fix 7: parse body with template-variable fallback
    let raw_body = request
        .get("body")
        .and_then(|b| b.get("raw"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let (body_yaml, body_had_coercion, body_raw_fallback) = match raw_body {
        Some(ref raw) => {
            let (yaml, coerced) = body_to_yaml(raw);
            let fallback = if yaml.is_none() {
                Some(raw.clone())
            } else {
                None
            };
            (yaml, coerced, fallback)
        }
        None => (None, false, None),
    };

    // Parse Postman event scripts
    let events = item.get("event").and_then(|v| v.as_array());
    let (test_script, pre_request_sets, pre_script_untranslatable) = match events {
        Some(events) => {
            let test_lines: Vec<String> = events
                .iter()
                .filter(|e| e.get("listen").and_then(|v| v.as_str()) == Some("test"))
                .flat_map(|e| {
                    e.pointer("/script/exec")
                        .and_then(|v| v.as_array())
                        .map(|ls| {
                            ls.iter()
                                .filter_map(|l| l.as_str().map(|s| s.to_string()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect();

            let pre_lines: Vec<String> = events
                .iter()
                .filter(|e| e.get("listen").and_then(|v| v.as_str()) == Some("prerequest"))
                .flat_map(|e| {
                    e.pointer("/script/exec")
                        .and_then(|v| v.as_array())
                        .map(|ls| {
                            ls.iter()
                                .filter_map(|l| l.as_str().map(|s| s.to_string()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect();

            let pre_refs: Vec<&str> = pre_lines.iter().map(|s| s.as_str()).collect();
            let (pre_sets, pre_untranslatable) = parse_pre_script(&pre_refs);

            let refs: Vec<&str> = test_lines.iter().map(|s| s.as_str()).collect();
            let script = parse_test_script(&refs);

            (script, pre_sets, pre_untranslatable)
        }
        None => (ScriptInfo::default(), vec![], vec![]),
    };

    Some(ImportedRequest {
        name,
        method,
        url,
        headers,
        body_yaml,
        body_had_coercion,
        body_raw_fallback,
        folder: folder.map(|s| s.to_string()),
        test_script,
        pre_request_sets,
        pre_script_untranslatable,
    })
}

// ---------------------------------------------------------------------------
// Postman test-script parser (Fix 2 / 3 / 4 / 5)
// ---------------------------------------------------------------------------

fn parse_test_script(lines: &[&str]) -> ScriptInfo {
    let mut info = ScriptInfo::default();

    for &line in lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }

        // status: pm.response.to.have.status(NNN)
        if t.contains("have.status(")
            && let Some(s) = extract_status(t)
        {
            info.status = Some(s);
            continue;
        }

        // body exists: pm.expect(json.field).to.exist
        if t.contains("pm.expect(json.")
            && t.contains(".to.exist")
            && let Some(field) = extract_expect_exist(t)
        {
            info.body_exists.push(field);
            continue;
        }

        // type assertions: pm.expect(json).to.be.a/an('type')  or  pm.expect(json.field)...
        if t.contains("pm.expect(")
            && (t.contains(".to.be.a(") || t.contains(".to.be.an("))
            && let Some((field, type_name)) = extract_expect_type(t)
        {
            if field == "$" {
                info.body_type = Some(type_name);
            } else {
                info.body_field_types.push((field, type_name));
            }
            continue;
        }

        // body equality: pm.expect(json.field).to.equal(value)
        if t.contains("pm.expect(json.")
            && t.contains(".to.equal(")
            && let Some((field, value)) = extract_expect_equal(t)
        {
            info.body_eq.push((field, value));
            continue;
        }

        // response time: .to.be.below(NNN)
        if t.contains("responseTime")
            && t.contains(".below(")
            && let Some(ms) = extract_below_ms(t)
        {
            info.response_time_lt = Some(ms);
            continue;
        }

        // extraction: pm.environment.set('var', json.path)
        if t.contains("pm.environment.set(") {
            match try_parse_env_set(t) {
                Some(EnvSet::Simple { var, path }) => info.extractions.push((var, path)),
                Some(EnvSet::Computed {
                    var,
                    raw_var,
                    raw_path,
                    template,
                }) => {
                    info.extractions.push((raw_var.clone(), raw_path));
                    info.computed_sets.push((var, raw_var, template));
                }
                None => info.untranslatable.push(t.to_string()),
            }
            continue;
        }

        if !is_structural_js(t) {
            info.untranslatable.push(t.to_string());
        }
    }

    info
}

/// Parse a Postman pre-request script into:
/// - translated `pm.variables.set()` literal calls → `pre_request: - set:` entries
/// - everything else that isn't structural or a redundant guard → WARN lines
fn parse_pre_script(lines: &[&str]) -> (Vec<(String, String)>, Vec<String>) {
    let mut sets = Vec::new();
    let mut untranslatable = Vec::new();
    let mut skip_block_depth: usize = 0;

    for &line in lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }

        // Track brace depth so we can skip entire guard blocks
        let opens = t.chars().filter(|&c| c == '{').count();
        let closes = t.chars().filter(|&c| c == '}').count();

        if skip_block_depth > 0 {
            skip_block_depth = skip_block_depth
                .saturating_add(opens)
                .saturating_sub(closes);
            continue;
        }

        // pm.variables.set('key', 'literal') → pre_request set
        if t.contains("pm.variables.set(")
            && let Some((k, v)) = try_parse_variables_set(t)
        {
            sets.push((k, v));
            continue;
        }

        // Guard patterns — redundant in ACE's state-machine model, skip silently
        // e.g. `var token = pm.environment.get('token');`
        //      `if (!token) { throw ... }`
        if t.contains("pm.environment.get(")
            || t.starts_with("if (!")
            || t.starts_with("if(!")
            || t.contains("throw new Error(")
        {
            skip_block_depth = skip_block_depth
                .saturating_add(opens)
                .saturating_sub(closes);
            continue;
        }

        if !is_structural_js(t) {
            untranslatable.push(t.to_string());
        }
    }

    (sets, untranslatable)
}

/// Parse `pm.variables.set('key', 'literal_string')` — literal values only.
fn try_parse_variables_set(line: &str) -> Option<(String, String)> {
    let pos = line.find("pm.variables.set(")?;
    let rest = &line[pos + 17..];

    let q1 = rest.chars().next()?;
    if q1 != '\'' && q1 != '"' {
        return None;
    }
    let inner = &rest[1..];
    let end_k = inner.find(q1)?;
    let key = inner[..end_k].to_string();

    let after = inner[end_k + 1..].trim_start();
    if !after.starts_with(',') {
        return None;
    }
    let val_expr = after[1..].trim_start();

    // Only accept literal strings ('value' or "value") — not computed expressions
    let value = unquote_js_string(val_expr.trim_end_matches([')', ';']))?;
    Some((key, value))
}

fn extract_status(line: &str) -> Option<u16> {
    let pos = line.find("have.status(")?;
    let rest = &line[pos + 12..];
    let end = rest.find(')')?;
    rest[..end].trim().parse().ok()
}

/// Returns `("$", type_name)` for root checks and `("field", type_name)` for field checks.
fn extract_expect_type(line: &str) -> Option<(String, String)> {
    // Extract the type name from .to.be.a('type') or .to.be.an('type')
    let marker = if line.contains(".to.be.an(") {
        ".to.be.an("
    } else {
        ".to.be.a("
    };
    let pos = line.find(marker)?;
    let rest = &line[pos + marker.len()..];
    let end = rest.find(')')?;
    let type_name = unquote_js_string(rest[..end].trim())?.to_lowercase();

    // Root: pm.expect(json).to.be...
    if line.contains("pm.expect(json)") {
        return Some(("$".to_string(), type_name));
    }

    // Field: pm.expect(json.field).to.be...
    if line.contains("pm.expect(json.") {
        let field = extract_expect_exist(&line.replace(marker, ".to.exist("))?;
        return Some((field, type_name));
    }

    None
}

fn extract_expect_equal(line: &str) -> Option<(String, String)> {
    let field = extract_expect_exist(line.replace(".to.equal(", ".to.exist(").as_str())?;
    // field extracted — now get the value inside .to.equal(...)
    let pos = line.find(".to.equal(")?;
    let rest = &line[pos + 10..];
    let end = rest.find(')')?;
    let raw = rest[..end].trim();
    let yaml_val = js_value_to_yaml(raw);
    Some((field, yaml_val))
}

fn js_value_to_yaml(raw: &str) -> String {
    // Quoted string: 'foo' or "foo"
    if let Some(inner) = raw
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
    {
        return format!("\"{}\"", inner.replace('"', "\\\""));
    }
    // Number or boolean — emit as-is
    raw.to_string()
}

fn extract_expect_exist(line: &str) -> Option<String> {
    let pos = line.find("pm.expect(json.")?;
    let rest = &line[pos + 15..];
    let end = rest.find(')')?;
    let field = rest[..end].trim();
    if !field.is_empty()
        && field
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        Some(field.to_string())
    } else {
        None
    }
}

fn extract_below_ms(line: &str) -> Option<u64> {
    let pos = line.find(".below(")?;
    let rest = &line[pos + 7..];
    let end = rest.find(')')?;
    rest[..end].trim().parse().ok()
}

/// Result of parsing a `pm.environment.set()` call.
enum EnvSet {
    /// Simple extraction: `extract: var: path`
    Simple { var: String, path: String },
    /// Computed value needing a hook: extract raw field, then compose via post_request.
    Computed {
        var: String,
        /// Intermediate variable name used to hold the extracted field.
        raw_var: String,
        /// JSON path for the extracted field.
        raw_path: String,
        /// Template string for the post_request set (uses `{{raw_var}}`).
        template: String,
    },
}

fn try_parse_env_set(line: &str) -> Option<EnvSet> {
    let pos = line.find("pm.environment.set(")?;
    let rest = &line[pos + 19..];

    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let inner = &rest[1..];
    let end_q = inner.find(quote)?;
    let var_name = inner[..end_q].to_string();

    let after = inner[end_q + 1..].trim_start();
    if !after.starts_with(',') {
        return None;
    }
    let value_expr = after[1..].trim_start();

    // Simple dot-path: json.field  or  json.nested.field
    if let Some(path) = value_expr.strip_prefix("json.") {
        let end = path.find([')', ' ', ';']).unwrap_or(path.len());
        let field = &path[..end];
        if !field.is_empty()
            && field
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            return Some(EnvSet::Simple {
                var: var_name,
                path: field.to_string(),
            });
        }
    }

    // Array index at root: json[N].field  →  [N].field
    if value_expr.starts_with("json[") {
        let bracket = &value_expr[4..]; // strip "json", keep "[N].field..."
        let end = bracket.find([')', ' ', ';']).unwrap_or(bracket.len());
        let path = &bracket[..end];
        if is_valid_index_path(path) {
            return Some(EnvSet::Simple {
                var: var_name,
                path: path.to_string(),
            });
        }
    }

    // String concatenation: 'literal' + json.field  OR  json.field + 'literal'
    if let Some(computed) = try_parse_string_concat(value_expr, &var_name) {
        return Some(computed);
    }

    None
}

/// Returns true for paths like `[0].id` or `[2].data.name`.
fn is_valid_index_path(path: &str) -> bool {
    path.starts_with('[')
        && path
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '[' | ']' | '.' | '_'))
}

/// Detect `'prefix' + json.field` or `json.field + 'suffix'` and return a Computed variant.
fn try_parse_string_concat(expr: &str, var_name: &str) -> Option<EnvSet> {
    // Find the `+` operator
    let plus = expr.find(" + ")?;
    let left = expr[..plus].trim();
    let right = expr[plus + 3..].trim();
    // Trim trailing `)` or `;` from right
    let right = right.trim_end_matches([')', ';']).trim();

    let (prefix, suffix, json_part) =
        if (left.starts_with('\'') || left.starts_with('"')) && right.starts_with("json.") {
            let lit = unquote_js_string(left)?;
            (lit, String::new(), right)
        } else if left.starts_with("json.") && (right.starts_with('\'') || right.starts_with('"')) {
            let lit = unquote_js_string(right)?;
            (String::new(), lit, left)
        } else {
            return None;
        };

    let field = json_part.strip_prefix("json.")?;
    if field.is_empty()
        || !field
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }

    let raw_var = format!("_{}_raw", slugify(var_name));
    let template = format!("{}{{{{{}}}}}{}", prefix, raw_var, suffix);

    Some(EnvSet::Computed {
        var: var_name.to_string(),
        raw_var,
        raw_path: field.to_string(),
        template,
    })
}

fn unquote_js_string(s: &str) -> Option<String> {
    let inner = s
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| s.strip_prefix('"').and_then(|s| s.strip_suffix('"')))?;
    Some(inner.to_string())
}

fn is_structural_js(line: &str) -> bool {
    matches!(line, "});" | "})" | "}" | "};" | "{" | ");")
        || line.starts_with("pm.test(")
        || line.starts_with("var ")
        || line.starts_with("let ")
        || line.starts_with("const ")
        || line.starts_with("if (")
        || line.starts_with("if(")
        || line.starts_with("function ")
        || line.ends_with('{')
}

// ---------------------------------------------------------------------------
// Body parsing (Fix 7)
// ---------------------------------------------------------------------------

/// Try to convert a Postman raw body string to indented YAML lines.
/// Returns `(yaml_lines, had_coercion)`.
fn body_to_yaml(raw: &str) -> (Option<String>, bool) {
    // Fast path: valid JSON as-is
    if let Ok(json) = serde_json::from_str::<Value>(raw) {
        return (Some(json_to_yaml_lines(&json)), false);
    }

    // Slow path: body contains `{{var}}` in a non-string position — wrap in quotes
    if raw.contains("{{") {
        let fixed = fix_template_vars_in_json(raw);
        if let Ok(json) = serde_json::from_str::<Value>(&fixed) {
            return (Some(json_to_yaml_lines(&json)), true);
        }
    }

    (None, false)
}

fn json_to_yaml_lines(json: &Value) -> String {
    let yaml = serde_yaml::to_string(json).unwrap_or_default();
    yaml.lines()
        .filter(|l| *l != "---")
        .map(|l| format!("      {}\n", l))
        .collect()
}

/// Wrap any `{{var}}` token that appears outside a JSON string context in double-quotes
/// so the body can be parsed as valid JSON.
fn fix_template_vars_in_json(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 32);
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            escaped = false;
            out.push(ch);
            continue;
        }
        if in_string && ch == '\\' {
            escaped = true;
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            out.push(ch);
            continue;
        }
        if !in_string && ch == '{' && chars.peek() == Some(&'{') {
            // Unquoted {{var}} — wrap it
            out.push('"');
            out.push(ch);
            out.push(chars.next().unwrap()); // second {
            loop {
                match chars.next() {
                    Some('}') => {
                        out.push('}');
                        if chars.peek() == Some(&'}') {
                            out.push(chars.next().unwrap()); // second }
                            break;
                        }
                    }
                    Some(c) => out.push(c),
                    None => break,
                }
            }
            out.push('"');
            continue;
        }
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------
// YAML builder
// ---------------------------------------------------------------------------

fn build_yaml(name: &str, vars: &[(String, String)], requests: &[ImportedRequest]) -> String {
    let mut yaml = String::new();

    yaml.push_str(&format!("name: {}\n", sanitize_yaml_str(name)));
    yaml.push_str("initial_state: start\n");

    // Fix 1: variables block
    if !vars.is_empty() {
        yaml.push_str("variables:\n");
        for (k, v) in vars {
            yaml.push_str(&format!("  {}: {}\n", k, sanitize_yaml_str(v)));
        }
    }

    yaml.push_str("steps:\n");

    let mut prev_state = "start".to_string();
    let mut last_folder: Option<String> = None;

    for (i, req) in requests.iter().enumerate() {
        // Fix 6: folder comment marker when we cross into a new folder
        if req.folder != last_folder {
            if let Some(ref f) = req.folder {
                yaml.push_str(&format!("  # --- Folder: {} ---\n", f));
            }
            last_folder = req.folder.clone();
        }

        let next_state = if i == requests.len() - 1 {
            "done".to_string()
        } else {
            slugify(&req.name)
        };

        yaml.push_str(&format!("  - name: {}\n", sanitize_yaml_str(&req.name)));
        yaml.push_str(&format!("    method: {}\n", req.method));
        yaml.push_str(&format!("    url: \"{}\"\n", req.url));

        // Emit tags for folder membership so it is machine-readable, not just a comment
        if let Some(ref folder) = req.folder {
            yaml.push_str(&format!("    tags: [{}]\n", folder));
        }

        if !req.headers.is_empty() {
            yaml.push_str("    headers:\n");
            for (k, v) in &req.headers {
                yaml.push_str(&format!("      {}: \"{}\"\n", k, v.replace('"', "\\\"")));
            }
        }

        // Fix 7: body with coercion note / raw fallback comment
        if req.body_had_coercion {
            yaml.push_str(
                "    # NOTE: one or more body fields had {{var}} in a non-string JSON position\n",
            );
            yaml.push_str("    #       and were coerced to strings to preserve the template.\n");
            yaml.push_str("    #       Adjust types manually if the API requires a number.\n");
        }
        if let Some(ref lines) = req.body_yaml {
            yaml.push_str("    body:\n");
            yaml.push_str(lines);
        } else if let Some(ref raw) = req.body_raw_fallback {
            yaml.push_str("    # WARN: body could not be parsed — reproduce manually:\n");
            for line in raw.lines() {
                yaml.push_str(&format!("    #   {}\n", line));
            }
        }

        // Fix 4 + 5: assertions from test scripts + heuristic status fallback
        let effective_status = req
            .test_script
            .status
            .unwrap_or_else(|| if req.method == "POST" { 201 } else { 200 });
        let has_body_checks = !req.test_script.body_exists.is_empty()
            || !req.test_script.body_eq.is_empty()
            || !req.test_script.body_field_types.is_empty();

        {
            yaml.push_str("    assert:\n");
            yaml.push_str(&format!("      - status: {}\n", effective_status));
            if has_body_checks {
                yaml.push_str("      - body:\n");
                for field in &req.test_script.body_exists {
                    yaml.push_str(&format!("          {}:\n", field));
                    yaml.push_str("            exists: true\n");
                }
                for (field, value) in &req.test_script.body_eq {
                    yaml.push_str(&format!("          {}:\n", field));
                    yaml.push_str(&format!("            eq: {}\n", value));
                }
                for (field, type_name) in &req.test_script.body_field_types {
                    yaml.push_str(&format!("          {}:\n", field));
                    yaml.push_str(&format!("            type: {}\n", type_name));
                }
            }
            if let Some(type_name) = &req.test_script.body_type {
                yaml.push_str(&format!("      - body_type: {}\n", type_name));
            }
            if let Some(ms) = req.test_script.response_time_lt {
                yaml.push_str("      - response_time_ms:\n");
                yaml.push_str(&format!("          lt: {}\n", ms));
            }
        }

        // Fix 3: extract block
        if !req.test_script.extractions.is_empty() {
            yaml.push_str("    extract:\n");
            for (var, path) in &req.test_script.extractions {
                // Quote paths that start with `[` (array index) to keep YAML valid
                if path.starts_with('[') {
                    yaml.push_str(&format!("      {}: \"{}\"\n", var, path));
                } else {
                    yaml.push_str(&format!("      {}: {}\n", var, path));
                }
            }
        }

        // Computed sets (string concatenation) → post_request hooks
        if !req.test_script.computed_sets.is_empty() {
            yaml.push_str("    post_request:\n");
            for (final_var, _raw_var, template) in &req.test_script.computed_sets {
                yaml.push_str("      - set:\n");
                yaml.push_str(&format!(
                    "          {}: \"{}\"\n",
                    final_var,
                    template.replace('"', "\\\"")
                ));
            }
        }

        // Translated pre-request sets → pre_request hook
        if !req.pre_request_sets.is_empty() {
            yaml.push_str("    pre_request:\n");
            yaml.push_str("      - set:\n");
            for (k, v) in &req.pre_request_sets {
                yaml.push_str(&format!(
                    "          {}: \"{}\"\n",
                    k,
                    v.replace('"', "\\\"")
                ));
            }
        }

        // WARN for untranslatable pre-request lines
        if !req.pre_script_untranslatable.is_empty() {
            yaml.push_str("    # WARN: pre-request lines not translated:\n");
            for line in &req.pre_script_untranslatable {
                yaml.push_str(&format!("    #   {}\n", line));
            }
        }

        // Fix 2: warn comments for untranslatable test-script lines
        if !req.test_script.untranslatable.is_empty() {
            yaml.push_str("    # WARN: test script lines not translated:\n");
            for line in &req.test_script.untranslatable {
                yaml.push_str(&format!("    #   {}\n", line));
            }
        }

        yaml.push_str("    transition:\n");
        yaml.push_str(&format!("      from: {}\n", prev_state));
        yaml.push_str(&format!("      to: {}\n", next_state));

        prev_state = next_state;
    }

    yaml
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

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

/// Postman `{{var}}` is identical to ACE's syntax — pass through.
/// Postman `:param` path params → `{{param}}`.
fn convert_postman_vars(url: &str) -> String {
    let mut result = String::new();
    let mut chars = url.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == ':' && result.ends_with('/') {
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

// ---------------------------------------------------------------------------
// String utilities
// ---------------------------------------------------------------------------

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn sanitize_yaml_str(s: &str) -> String {
    if s.contains(':')
        || s.contains('#')
        || s.contains('"')
        || s.contains('\'')
        || s.starts_with('{')
        || s.contains('—')
    {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}
