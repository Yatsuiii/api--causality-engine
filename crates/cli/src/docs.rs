use crate::error::{CliError, load_scenario_file, write_file};
use colored::Colorize;

pub fn cmd_docs(path: &str, output: Option<String>) -> Result<(), CliError> {
    let scenario = load_scenario_file(path)?;

    let mut doc = String::new();
    doc.push_str(&format!("# {}\n\n", scenario.name));

    if let Some(vars) = &scenario.variables {
        doc.push_str("## Variables\n\n");
        doc.push_str("| Name | Default |\n|------|--------|\n");
        for (k, v) in vars {
            doc.push_str(&format!("| `{}` | `{}` |\n", k, v));
        }
        doc.push('\n');
    }

    if let Some(auth) = &scenario.auth {
        doc.push_str("## Authentication\n\n");
        if auth.bearer.is_some() {
            doc.push_str("- **Type:** Bearer Token\n");
        }
        if auth.basic.is_some() {
            doc.push_str("- **Type:** Basic Auth\n");
        }
        if auth.api_key.is_some() {
            doc.push_str("- **Type:** API Key\n");
        }
        if auth.oauth2.is_some() {
            doc.push_str("- **Type:** OAuth2 Client Credentials\n");
        }
        doc.push('\n');
    }

    doc.push_str("## Endpoints\n\n");

    for (i, step) in scenario.steps.iter().enumerate() {
        doc.push_str(&format!(
            "### {}. {} `{}`\n\n",
            i + 1,
            step.method,
            step.url
        ));
        doc.push_str(&format!("**{}**\n\n", step.name));
        let next_states = scenario
            .edges
            .iter()
            .filter(|edge| edge.from == step.state)
            .map(|edge| edge.to.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        doc.push_str(&format!(
            "State: `{}` → `{}`\n\n",
            step.state_name(),
            if next_states.is_empty() {
                "<missing edge>"
            } else {
                &next_states
            },
        ));

        if let Some(headers) = &step.headers {
            doc.push_str("**Headers:**\n\n");
            doc.push_str("| Header | Value |\n|--------|-------|\n");
            for (k, v) in headers {
                doc.push_str(&format!("| `{}` | `{}` |\n", k, v));
            }
            doc.push('\n');
        }

        if let Some(body) = &step.body {
            doc.push_str("**Request Body:**\n\n```json\n");
            let json_val: serde_json::Value =
                serde_json::from_str(&serde_json::to_string(body).unwrap_or_default())
                    .unwrap_or(serde_json::Value::Null);
            doc.push_str(&serde_json::to_string_pretty(&json_val).unwrap_or_default());
            doc.push_str("\n```\n\n");
        }

        if let Some(assertions) = &step.assertions {
            doc.push_str("**Assertions:**\n\n");
            for assertion in assertions {
                if let Some(status) = &assertion.status {
                    match status {
                        model::StatusCheck::Exact(code) => {
                            doc.push_str(&format!("- Status: `{}`\n", code));
                        }
                        _ => doc.push_str("- Status: complex check\n"),
                    }
                }
                if let Some(body_checks) = &assertion.body {
                    for path in body_checks.keys() {
                        doc.push_str(&format!("- Body `{}`: validated\n", path));
                    }
                }
                if let Some(header_checks) = &assertion.header {
                    for name in header_checks.keys() {
                        doc.push_str(&format!("- Header `{}`: validated\n", name));
                    }
                }
                if assertion.response_time_ms.is_some() {
                    doc.push_str("- Response time: validated\n");
                }
            }
            doc.push('\n');
        }

        if let Some(extract) = &step.extract {
            doc.push_str("**Extracts:**\n\n");
            for (key, spec) in extract {
                let marker = if spec.is_required(false) {
                    " *(required)*"
                } else {
                    ""
                };
                doc.push_str(&format!("- `{}` ← `{}`{}\n", key, spec.path(), marker));
            }
            doc.push('\n');
        }
    }

    match output {
        Some(out_path) => {
            write_file(&out_path, &doc)?;
            println!("{} {}", "Docs written:".green().bold(), out_path);
        }
        None => print!("{}", doc),
    }

    Ok(())
}
