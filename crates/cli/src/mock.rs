use crate::error::{CliError, load_scenario_file};
use colored::Colorize;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const CORS_HEADERS: &str = "Access-Control-Allow-Origin: *\r\n\
    Access-Control-Allow-Methods: GET, POST, PUT, PATCH, DELETE, OPTIONS\r\n\
    Access-Control-Allow-Headers: Content-Type, Authorization, X-Requested-With\r\n";

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

        // Response body is shaped from extract: fields only — those define what
        // the scenario expects the response to contain. The request body: fields
        // are what is *sent*, not what is received, so they must not appear here.
        let mut mock_body = serde_json::Map::new();
        if let Some(extract) = &step.extract {
            for json_key in extract.values() {
                mock_body.insert(
                    json_key.clone(),
                    serde_json::Value::String(format!("mock_{}", json_key)),
                );
            }
        }

        if mock_body.is_empty() {
            mock_body.insert("ok".into(), serde_json::Value::Bool(true));
        }

        let response_body = serde_json::to_string_pretty(&serde_json::Value::Object(mock_body))
            .expect("serialization of in-memory serde_json::Map cannot fail");

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

    // Warn about duplicate method+path combinations — only the first match is
    // ever served; later steps with the same route are silently unreachable.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut collisions: Vec<String> = Vec::new();
    for (i, route) in routes.iter().enumerate() {
        let key = format!("{} {}", route.method, route.path);
        if let Some(first) = seen.get(&key) {
            collisions.push(format!(
                "step {} and step {} both map to {} {}",
                first + 1,
                i + 1,
                route.method,
                route.path
            ));
        } else {
            seen.insert(key, i);
        }
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
    if !collisions.is_empty() {
        eprintln!("\n  {}", "warning: duplicate routes detected — only the first match is served:".yellow().bold());
        for c in &collisions {
            eprintln!("    {} {}", "•".yellow(), c);
        }
        eprintln!("  {}", "Consider using unique paths or path parameters per step.".dimmed());
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

        // Drain headers, tracking Content-Length so we can consume the body too.
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            if buf_reader.read_line(&mut line).await.is_err() || line.trim().is_empty() {
                break;
            }
            let lower = line.to_lowercase();
            if let Some(rest) = lower.strip_prefix("content-length:") {
                content_length = rest.trim().parse().unwrap_or(0);
            }
        }

        // Consume the request body so the client doesn't stall waiting for it
        // to be read before it accepts the response.
        if content_length > 0 {
            let mut body_buf = vec![0u8; content_length];
            let _ = tokio::io::AsyncReadExt::read_exact(&mut buf_reader, &mut body_buf).await;
        }

        // Handle CORS preflight
        if req_method == "OPTIONS" {
            let response = format!(
                "HTTP/1.1 204 No Content\r\n{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                CORS_HEADERS
            );
            let _ = writer.write_all(response.as_bytes()).await;
            continue;
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
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
            status,
            status_text(status),
            content_type,
            body.len(),
            CORS_HEADERS,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // extract_path
    // -----------------------------------------------------------------------

    #[test]
    fn extract_path_absolute_url() {
        assert_eq!(extract_path("https://api.example.com/users/1"), "/users/1");
    }

    #[test]
    fn extract_path_template_base_url() {
        assert_eq!(extract_path("{{base_url}}/posts"), "/posts");
    }

    #[test]
    fn extract_path_template_with_param() {
        assert_eq!(extract_path("{{base_url}}/posts/{{post_id}}"), "/posts/{{post_id}}");
    }

    #[test]
    fn extract_path_already_slash() {
        assert_eq!(extract_path("/health"), "/health");
    }

    #[test]
    fn extract_path_with_query_string() {
        assert_eq!(
            extract_path("{{base_url}}/posts?userId={{user_id}}"),
            "/posts?userId={{user_id}}"
        );
    }

    // -----------------------------------------------------------------------
    // path_matches
    // -----------------------------------------------------------------------

    #[test]
    fn path_matches_exact() {
        assert!(path_matches("/users/1", "/users/1"));
    }

    #[test]
    fn path_matches_template_param() {
        assert!(path_matches("/users/{{user_id}}", "/users/42"));
    }

    #[test]
    fn path_matches_template_does_not_cross_segments() {
        assert!(!path_matches("/users/{{user_id}}", "/users/42/profile"));
    }

    #[test]
    fn path_matches_different_literals() {
        assert!(!path_matches("/users/1", "/users/2"));
    }

    #[test]
    fn path_matches_ignores_query_string() {
        assert!(path_matches("/posts?userId={{user_id}}", "/posts?userId=5"));
    }

    #[test]
    fn path_matches_prefix_not_enough() {
        assert!(!path_matches("/posts", "/posts/comments"));
    }

    // -----------------------------------------------------------------------
    // status_text
    // -----------------------------------------------------------------------

    #[test]
    fn status_text_known_codes() {
        assert_eq!(status_text(200), "OK");
        assert_eq!(status_text(201), "Created");
        assert_eq!(status_text(204), "No Content");
        assert_eq!(status_text(400), "Bad Request");
        assert_eq!(status_text(401), "Unauthorized");
        assert_eq!(status_text(404), "Not Found");
        assert_eq!(status_text(422), "Unprocessable Entity");
        assert_eq!(status_text(429), "Too Many Requests");
        assert_eq!(status_text(500), "Internal Server Error");
        assert_eq!(status_text(503), "Service Unavailable");
    }

    #[test]
    fn status_text_unknown_does_not_return_ok() {
        // Bug 5 regression: unknown codes must not claim to be "OK"
        assert_ne!(status_text(418), "OK");
        assert_ne!(status_text(599), "OK");
    }
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}
