use std::io::{BufReader, Read};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::forward::client;
use super::route;
use super::translate;
use crate::provider::{Format, Provider};

const DEADLINE: Duration = Duration::from_secs(60);
const PROMPT: &str = ".";

/// The smallest request that counts as use: one short prompt on the
/// cheapest model, for as few tokens as the format lets a client ask for.
fn body(format: Format, model: &str) -> Value {
    match format {
        Format::Messages => json!({
            "model": model,
            "max_tokens": 1,
            "stream": true,
            "messages": [{"role": "user", "content": PROMPT}],
        }),
        Format::Responses => json!({
            "model": model,
            "stream": true,
            "reasoning": {"effort": "low"},
            "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": PROMPT}]}],
        }),
        Format::Chat => json!({
            "model": model,
            "max_tokens": 1,
            "stream": true,
            "messages": [{"role": "user", "content": PROMPT}],
        }),
    }
}

/// Sends `creds`' provider the smallest request it counts, and reads the
/// answer to its end. Returns the model it went to.
pub fn ping(p: &dyn Provider, creds: &Value) -> Result<&'static str> {
    let model = route::cheapest(p.id())
        .with_context(|| format!("{} has no model to keep warm with", p.name()))?;
    let format = *p
        .formats(model)
        .first()
        .with_context(|| format!("{} takes no request remuda can send", p.name()))?;
    let mut body = body(format, model);
    p.shape(format, &mut body, None)?;
    let upstream = p
        .upstream(creds, format, model)
        .with_context(|| format!("the {} login holds no token", p.name()))?;
    let client = client().context("no HTTP client")?;
    let mut request = client
        .post(&upstream.url)
        .timeout(DEADLINE)
        .header("content-type", "application/json");
    for (name, value) in &upstream.headers {
        request = request.header(*name, value);
    }
    if format == Format::Messages
        && !upstream
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("anthropic-version"))
    {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let mut resp = request
        .body(body.to_string())
        .send()
        .with_context(|| format!("{} could not be reached", p.name()))?;
    let status = resp.status();
    if !status.is_success() {
        let mut text = String::new();
        let _ = (&mut resp).take(64 * 1024).read_to_string(&mut text);
        bail!(
            "{} answered {status}: {}",
            p.name(),
            translate::error_message(&text)
        );
    }
    translate::finished(format, BufReader::new(resp))
        .map_err(|e| anyhow::anyhow!("{}: {e}", p.name()))?;
    Ok(model)
}

#[cfg(test)]
mod tests {
    use mockito::Matcher;

    use super::*;
    use crate::claude::{self, Claude};
    use crate::ops::testing::*;

    #[test]
    fn claude_is_sent_one_token_on_haiku_as_claude_code() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/v1/messages")
            .match_query(Matcher::Any)
            .match_header("authorization", "Bearer a-live")
            .match_body(Matcher::PartialJson(json!({
                "model": "claude-haiku-4-5",
                "max_tokens": 1,
                "system": [{"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."}],
            })))
            .with_body("event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n")
            .create();
        let claude = Claude::at(
            std::path::Path::new("/nonexistent"),
            claude::Api::local(&server.url()),
        );
        let creds = oauth("a-live", "r", HOUR);
        assert_eq!(ping(&claude, &creds).unwrap(), "claude-haiku-4-5");
        upstream.assert();
    }

    #[test]
    fn a_refused_or_broken_answer_is_an_error() {
        let mut server = mockito::Server::new();
        let claude = Claude::at(
            std::path::Path::new("/nonexistent"),
            claude::Api::local(&server.url()),
        );
        let creds = oauth("a-live", "r", HOUR);
        let refused = server
            .mock("POST", "/v1/messages")
            .match_query(Matcher::Any)
            .with_status(429)
            .with_body(r#"{"error":{"message":"rate limited"}}"#)
            .create();
        let e = ping(&claude, &creds).unwrap_err().to_string();
        assert!(e.contains("429") && e.contains("rate limited"), "{e}");
        refused.remove();
        server
            .mock("POST", "/v1/messages")
            .match_query(Matcher::Any)
            .with_body(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n",
            )
            .create();
        let e = ping(&claude, &creds).unwrap_err().to_string();
        assert!(e.contains("ended before"), "{e}");
    }

    #[test]
    fn a_chat_provider_is_sent_one_token_too() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/anthropic/v1/messages")
            .match_header("anthropic-version", "2023-06-01")
            .match_body(Matcher::PartialJson(
                json!({"model": "glm-4.6", "max_tokens": 1}),
            ))
            .with_body(
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            )
            .create();
        let glm = crate::apikey::ApiKey::glm()
            .with(None, |_| None)
            .inferring_at(&server.url());
        ping(&glm, &json!({"key": "zk"})).unwrap();
        upstream.assert();
        let cursor_like = crate::cursor::Cursor::at(
            std::path::Path::new("/nonexistent"),
            crate::cursor::Api::local(&server.url()),
        );
        let e = ping(&cursor_like, &json!({})).unwrap_err().to_string();
        assert!(e.contains("no model to keep warm with"), "{e}");
    }

    #[test]
    fn codex_is_sent_a_short_low_effort_response() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/responses")
            .match_header("chatgpt-account-id", "ws-1")
            .match_header("originator", "codex_cli_rs")
            .match_body(Matcher::PartialJson(json!({
                "model": "gpt-5", "stream": true, "store": false,
                "reasoning": {"effort": "low"},
            })))
            .with_body(
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n",
            )
            .create();
        let codex = crate::codex::Codex::at(
            std::path::Path::new("/nonexistent"),
            crate::codex::Api::local(&server.url()),
            0,
        );
        let creds =
            json!({"tokens": {"access_token": "a", "refresh_token": "r", "account_id": "ws-1"}});
        assert_eq!(ping(&codex, &creds).unwrap(), "gpt-5");
        upstream.assert();
    }
}
