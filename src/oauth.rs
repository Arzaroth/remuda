use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;

pub fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("remuda/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build the HTTP client")
}

/// Sends a token request and returns the answer. An error quotes at most the
/// start of the body: a token endpoint never echoes a secret back in an error.
pub fn token_request(request: reqwest::blocking::RequestBuilder) -> Result<Value> {
    let resp = request.send().context("token request failed")?;
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    if !status.is_success() {
        if [
            "invalid_grant",
            "refresh_token_reused",
            "refresh_token_expired",
        ]
        .iter()
        .any(|code| body.contains(code))
        {
            bail!("the refresh token was rejected ({status}): sign this credential in again");
        }
        bail!(
            "token endpoint answered {status}: {}",
            body.chars().take(200).collect::<String>()
        );
    }
    serde_json::from_str(&body).context("malformed token response")
}

pub fn text(answer: &Value, key: &str) -> Option<String> {
    answer
        .get(key)
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

/// A JWT's payload, unverified: what the token says about itself.
pub fn claims(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// An unsigned JWT carrying `claims`, for tests.
#[cfg(test)]
pub fn fake_jwt(claims: Value) -> String {
    let enc = |v: &Value| URL_SAFE_NO_PAD.encode(v.to_string());
    format!(
        "{}.{}.sig",
        enc(&serde_json::json!({"alg": "none"})),
        enc(&claims)
    )
}
