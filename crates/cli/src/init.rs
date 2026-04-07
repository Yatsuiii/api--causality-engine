use crate::error::CliError;
use colored::Colorize;
use std::path::Path;

const FULL_TEMPLATE: &str = r#"name: My API Scenario
initial_state: get_post

variables:
  base_url: https://jsonplaceholder.typicode.com
  post_id: "1"

steps:
  - name: get_post
    method: GET
    url: "{{base_url}}/posts/{{post_id}}"
    transition:
      from: get_post
      to: get_comments
    assert:
      - status: 200
        body:
          id: { eq: 1 }
          title: { exists: true }
    extract:
      post_title: title

  - name: get_comments
    method: GET
    url: "{{base_url}}/posts/{{post_id}}/comments"
    transition:
      from: get_comments
      to: done
    assert:
      - status: 200
"#;

const MINIMAL_TEMPLATE: &str = r#"name: My API Scenario
initial_state: request

steps:
  - name: request
    method: GET
    url: https://example.com/api/resource
    transition:
      from: request
      to: done
    assert:
      - status: 200
"#;

pub fn cmd_init(output: &str, minimal: bool) -> Result<(), CliError> {
    let path = Path::new(output);

    if path.exists() {
        return Err(CliError::BadArgument(format!(
            "file already exists: {}",
            output
        )));
    }

    let content = if minimal {
        MINIMAL_TEMPLATE
    } else {
        FULL_TEMPLATE
    };

    std::fs::write(path, content).map_err(|e| CliError::Io {
        path: output.to_string(),
        source: e,
    })?;

    println!("{} {}", "Created:".green().bold(), output);
    println!("  Run with: {}", format!("ace run {}", output).cyan());

    Ok(())
}
