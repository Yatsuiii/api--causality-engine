use reqwest::Client;

pub async fn send_request(method: &str, url: &str) -> Result<(u16, String), String> {
    let client = Client::new();

    let response = match method {
        "GET" => client.get(url).send().await,
        "POST" => client.post(url).send().await,
        _ => return Err(format!("Unsupported method: {}", method)),
    };

    match response {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok((status, body))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unsupported_method() {
        let result = send_request("DELETE", "http://example.com").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported method"));
    }
}
