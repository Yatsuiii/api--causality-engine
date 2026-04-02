pub use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Response type returned from every request
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Request options (per-request)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct RequestOptions {
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub timeout_ms: Option<u64>,
    pub multipart: Option<Vec<MultipartField>>,
}

#[derive(Debug, Clone)]
pub struct MultipartField {
    pub name: String,
    pub value: MultipartValue,
}

#[derive(Debug, Clone)]
pub enum MultipartValue {
    Text(String),
    File {
        path: String,
        filename: Option<String>,
        mime: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Client configuration (per-scenario)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    pub insecure: bool,
    pub proxy: Option<String>,
    pub default_timeout_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Reusable HTTP client with cookie jar
// ---------------------------------------------------------------------------

pub fn build_client(config: &ClientConfig) -> Client {
    let mut builder = Client::builder().cookie_store(true);

    if config.insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }

    if let Some(proxy_url) = &config.proxy
        && let Ok(proxy) = reqwest::Proxy::all(proxy_url)
    {
        builder = builder.proxy(proxy);
    }

    if let Some(timeout) = config.default_timeout_ms {
        builder = builder.timeout(Duration::from_millis(timeout));
    }

    builder.build().expect("Failed to build HTTP client")
}

pub fn default_client() -> Client {
    build_client(&ClientConfig::default())
}

// ---------------------------------------------------------------------------
// Send request
// ---------------------------------------------------------------------------

pub async fn send_request(
    client: &Client,
    method: &str,
    url: &str,
    opts: &RequestOptions,
) -> Result<HttpResponse, String> {
    let mut builder = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "PATCH" => client.patch(url),
        "DELETE" => client.delete(url),
        "HEAD" => client.head(url),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, url),
        _ => return Err(format!("Unsupported method: {}", method)),
    };

    // Per-request timeout overrides client default
    if let Some(timeout) = opts.timeout_ms {
        builder = builder.timeout(Duration::from_millis(timeout));
    }

    // Apply headers
    for (key, value) in &opts.headers {
        builder = builder.header(key.as_str(), value.as_str());
    }

    // Multipart takes priority over body
    if let Some(fields) = &opts.multipart {
        let mut form = reqwest::multipart::Form::new();
        for field in fields {
            match &field.value {
                MultipartValue::Text(text) => {
                    form = form.text(field.name.clone(), text.clone());
                }
                MultipartValue::File {
                    path,
                    filename,
                    mime,
                } => {
                    let bytes = tokio::fs::read(path)
                        .await
                        .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;
                    let fname = filename
                        .clone()
                        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or("file").to_string());
                    let part = reqwest::multipart::Part::bytes(bytes).file_name(fname);
                    let part = match mime {
                        Some(m) => part
                            .mime_str(m)
                            .map_err(|e| format!("Invalid MIME type '{}': {}", m, e))?,
                        None => part,
                    };
                    form = form.part(field.name.clone(), part);
                }
            }
        }
        builder = builder.multipart(form);
    } else if let Some(body) = &opts.body {
        builder = builder.body(body.clone());
    }

    let start = Instant::now();
    let response = builder.send().await.map_err(|e| e.to_string())?;
    let duration_ms = start.elapsed().as_millis() as u64;

    let status = response.status().as_u16();

    // Collect response headers
    let mut headers = HashMap::new();
    for (key, value) in response.headers().iter() {
        if let Ok(v) = value.to_str() {
            headers.insert(key.to_string(), v.to_string());
        }
    }

    let body = response.text().await.map_err(|e| e.to_string())?;

    Ok(HttpResponse {
        status,
        headers,
        body,
        duration_ms,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unsupported_method() {
        let client = default_client();
        let opts = RequestOptions::default();
        let result = send_request(&client, "TRACE", "http://example.com", &opts).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported method"));
    }

    #[tokio::test]
    async fn all_methods_accepted() {
        let client = default_client();
        let opts = RequestOptions::default();
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
            let result = send_request(&client, method, "http://127.0.0.1:1", &opts).await;
            assert!(
                result.is_err(),
                "Expected error for {} to unreachable host",
                method
            );
            assert!(
                !result.as_ref().unwrap_err().contains("Unsupported method"),
                "Method {} should be supported",
                method
            );
        }
    }

    #[test]
    fn build_client_insecure() {
        let config = ClientConfig {
            insecure: true,
            proxy: None,
            default_timeout_ms: Some(5000),
        };
        let _client = build_client(&config);
    }

    #[test]
    fn build_client_with_proxy() {
        let config = ClientConfig {
            insecure: false,
            proxy: Some("http://localhost:8080".into()),
            default_timeout_ms: None,
        };
        let _client = build_client(&config);
    }
}
