use crate::variables::{Context, resolve_template, value_to_string};
use ace_http::{Client, RequestOptions, send_request};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use model::Auth;
use std::collections::HashMap;

pub(crate) async fn fetch_oauth2_token(
    client: &Client,
    oauth: &model::OAuth2Config,
    context: &Context,
) -> Result<String, String> {
    let token_url = resolve_template(&oauth.token_url, context);
    let client_id = resolve_template(&oauth.client_id, context);
    let client_secret = resolve_template(&oauth.client_secret, context);
    let grant_type = oauth.grant_type.as_deref().unwrap_or("client_credentials");

    let body = {
        let mut params = form_urlencoded::Serializer::new(String::new());
        params.append_pair("grant_type", grant_type);
        params.append_pair("client_id", &client_id);
        params.append_pair("client_secret", &client_secret);
        if let Some(scope) = &oauth.scope {
            params.append_pair("scope", &resolve_template(scope, context));
        }
        params.finish()
    };

    let mut headers = HashMap::new();
    headers.insert(
        "Content-Type".into(),
        "application/x-www-form-urlencoded".into(),
    );

    let opts = RequestOptions {
        headers,
        body: Some(body),
        timeout_ms: Some(30000),
        multipart: None,
    };

    let response = send_request(client, "POST", &token_url, &opts)
        .await
        .map_err(|e| format!("OAuth2 token request failed: {}", e))?;

    if response.status != 200 {
        return Err(format!(
            "OAuth2 token endpoint returned {}: {}",
            response.status, response.body
        ));
    }

    let json: serde_json::Value = serde_json::from_str(&response.body)
        .map_err(|e| format!("OAuth2 response parse failed: {}", e))?;

    json.get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "OAuth2 response missing 'access_token' field".to_string())
}

pub(crate) fn apply_auth(auth: &Auth, headers: &mut HashMap<String, String>, context: &Context) {
    if let Some(bearer) = &auth.bearer {
        let token = resolve_template(bearer, context);
        headers
            .entry("Authorization".into())
            .or_insert_with(|| format!("Bearer {}", token));
    }
    if let Some(basic) = &auth.basic {
        let user = resolve_template(&basic.username, context);
        let pass = resolve_template(&basic.password, context);
        let encoded = BASE64.encode(format!("{}:{}", user, pass));
        headers
            .entry("Authorization".into())
            .or_insert_with(|| format!("Basic {}", encoded));
    }
    if let Some(api_key) = &auth.api_key {
        let header = resolve_template(&api_key.header, context);
        let value = resolve_template(&api_key.value, context);
        headers.entry(header).or_insert(value);
    }
    if auth.oauth2.is_some()
        && let Some(token) = context.get("$oauth_token").map(value_to_string)
    {
        headers
            .entry("Authorization".into())
            .or_insert_with(|| format!("Bearer {}", token));
    }
}
