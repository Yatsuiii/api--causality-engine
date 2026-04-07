use crate::error::{CliError, load_scenario_file};
use colored::Colorize;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

struct MockRoute {
    method: String,
    path: String,
    status: u16,
    response_body: String,
    response_headers: HashMap<String, String>,
}

pub async fn cmd_mock(scenario_path: &str, port: u16) -> Result<(), CliError> {
    let scenario = load_scenario_file(scenario_path)?;

    // Build routes from scenario steps
    let mut routes = Vec::new();
    for step in &scenario.steps {
        let url = &step.url;
        let path = extract_path(url);
        let method = step.method.as_str().to_string();

        let status = step
            .assertions
            .as_ref()
            .and_then(|asserts| {
                asserts.iter().find_map(|a| match &a.status {
                    Some(model::StatusCheck::Exact(code)) => Some(*code),
                    _ => None,
                })
            })
            .unwrap_or(200);

        let mut mock_body = serde_json::Map::new();
        if let Some(extract) = &step.extract {
            for json_key in extract.values() {
                mock_body.insert(
                    json_key.clone(),
                    serde_json::Value::String(format!("mock_{}", json_key)),
                );
            }
        }
        if let Some(body) = &step.body
            && let Ok(json_str) = serde_json::to_string(body)
            && let Ok(serde_json::Value::Object(obj)) = serde_json::from_str(&json_str)
        {
            for (k, v) in obj {
                mock_body.insert(k, v);
            }
        }

        if mock_body.is_empty() {
            mock_body.insert("ok".into(), serde_json::Value::Bool(true));
        }

        let response_body =
            serde_json::to_string_pretty(&serde_json::Value::Object(mock_body)).unwrap();

        let mut response_headers = HashMap::new();
        response_headers.insert("Content-Type".into(), "application/json".into());

        routes.push(MockRoute {
            method,
            path,
            status,
            response_body,
            response_headers,
        });
    }

    println!(
        "\n{} Mock server for: {}",
        "▶".cyan().bold(),
        scenario.name.cyan()
    );
    println!("  {} http://localhost:{}", "Listening:".bold(), port);
    println!("\n  {}", "Routes:".bold());
    for route in &routes {
        println!(
            "    {} {} {} → {}",
            "•".dimmed(),
            route.method.yellow(),
            route.path,
            route.status
        );
    }
    println!("\n  {} Ctrl+C to stop\n", "Tip:".dimmed());

    // Start TCP server
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .map_err(|e| CliError::Io {
            path: format!("0.0.0.0:{}", port),
            source: e,
        })?;

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        let routes_ref = &routes;

        let (reader, mut writer) = stream.into_split();
        let mut buf_reader = BufReader::new(reader);
        let mut request_line = String::new();
        if buf_reader.read_line(&mut request_line).await.is_err() {
            continue;
        }

        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let req_method = parts[0];
        let req_path = parts[1];

        // Drain headers
        loop {
            let mut line = String::new();
            if buf_reader.read_line(&mut line).await.is_err() || line.trim().is_empty() {
                break;
            }
        }

        // Find matching route
        let matched = routes_ref
            .iter()
            .find(|r| r.method == req_method && path_matches(&r.path, req_path));

        let (status, body, content_type) = match matched {
            Some(route) => (
                route.status,
                route.response_body.clone(),
                route
                    .response_headers
                    .get("Content-Type")
                    .cloned()
                    .unwrap_or_else(|| "application/json".into()),
            ),
            None => (
                404,
                r#"{"error": "Not Found", "message": "No matching mock route"}"#.to_string(),
                "application/json".to_string(),
            ),
        };

        let response = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            status_text(status),
            content_type,
            body.len(),
            body
        );

        let _ = writer.write_all(response.as_bytes()).await;

        println!(
            "  {} {} {} → {}",
            addr.to_string().dimmed(),
            req_method.yellow(),
            req_path,
            if status < 400 {
                status.to_string().green()
            } else {
                status.to_string().red()
            }
        );
    }
}

fn extract_path(url: &str) -> String {
    if let Some(after_scheme) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        && let Some(slash_pos) = after_scheme.find('/')
    {
        return after_scheme[slash_pos..].to_string();
    }
    if url.starts_with('/') {
        return url.to_string();
    }
    if let Some(slash_pos) = url.find("}/") {
        return url[slash_pos + 1..].to_string();
    }
    "/".to_string()
}

fn path_matches(pattern: &str, actual: &str) -> bool {
    let pattern_path = pattern.split('?').next().unwrap_or(pattern);
    let actual_path = actual.split('?').next().unwrap_or(actual);

    let pattern_parts: Vec<&str> = pattern_path.split('/').filter(|s| !s.is_empty()).collect();
    let actual_parts: Vec<&str> = actual_path.split('/').filter(|s| !s.is_empty()).collect();

    if pattern_parts.len() != actual_parts.len() {
        return false;
    }

    pattern_parts
        .iter()
        .zip(actual_parts.iter())
        .all(|(p, a)| p.contains("{{") || *p == *a)
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}
