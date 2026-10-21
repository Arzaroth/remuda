use std::io::{Read, Write};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{Value, json};

use super::Backend;
use super::route;
use crate::fsx::now_ms;
use crate::http::Head;
use crate::http::write_head;
use crate::provider::{Format, Provider};

/// A request the proxy has read and accepted, about to be sent on.
pub struct Call<'a> {
    pub format: Format,
    pub count_tokens: bool,
    pub head: &'a Head,
    pub body: Value,
}

/// Refused before anything was written back.
#[derive(Debug)]
pub struct Refusal {
    pub status: u16,
    pub message: String,
}

fn refuse(status: u16, message: impl Into<String>) -> Refusal {
    Refusal {
        status,
        message: message.into(),
    }
}

/// A stream runs as long as the model writes, so only connecting is timed.
fn client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: OnceLock<Option<reqwest::blocking::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(None)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .ok()
        })
        .as_ref()
}

/// Client headers the upstream reads, sent on when the upstream does not set
/// its own.
const PASSED: &[&str] = &[
    "anthropic-version",
    "openai-beta",
    "accept",
    "session_id",
    "session-id",
    "thread-id",
    "version",
    "x-client-request-id",
];

fn merged_betas(ours: Option<&str>, theirs: Option<&str>) -> Option<String> {
    let mut all: Vec<&str> = Vec::new();
    for beta in ours
        .into_iter()
        .chain(theirs)
        .flat_map(|v| v.split(','))
        .map(str::trim)
    {
        if !beta.is_empty() && !all.contains(&beta) {
            all.push(beta);
        }
    }
    (!all.is_empty()).then(|| all.join(","))
}

/// The active login of the provider `model` goes to, and the model name to
/// send it.
fn pick<'b>(
    backend: &'b dyn Backend,
    model: &str,
) -> Result<(&'b dyn Provider, String, Value), Refusal> {
    let providers = backend.providers();
    let ids: Vec<&str> = providers.iter().map(|p| p.id()).collect();
    let (id, model) = route::route(model, &ids).ok_or_else(|| {
        refuse(
            404,
            format!(
                "no provider serves the model {model}; name one as provider/model, with provider one of {}",
                ids.join(", ")
            ),
        )
    })?;
    let p = *providers
        .iter()
        .find(|p| p.id() == id)
        .ok_or_else(|| refuse(404, format!("no provider {id}")))?;
    if p.formats(model).is_empty() {
        return Err(refuse(
            400,
            format!("the proxy cannot spend {} logins", p.name()),
        ));
    }
    let creds = p
        .live()
        .map_err(|e| refuse(503, format!("{} login unreadable: {e:#}", p.name())))?
        .ok_or_else(|| {
            refuse(
                503,
                format!(
                    "{} is signed out; `remuda use {}/<name>` makes a stored login active",
                    p.name(),
                    p.id()
                ),
            )
        })?;
    if p.expires_at(&creds).is_some_and(|at| at <= now_ms()) {
        return Err(refuse(
            401,
            format!(
                "the active {} login's access token has expired, and remuda never refreshes the login a CLI is using; run {} once, or switch with `remuda use`",
                p.name(),
                p.name()
            ),
        ));
    }
    Ok((p, model.to_owned(), creds))
}

pub fn forward(backend: &dyn Backend, mut call: Call, out: &mut impl Write) -> Result<(), Refusal> {
    let model = call
        .body
        .get("model")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| refuse(400, "the request names no model"))?
        .to_owned();
    let (p, model, creds) = pick(backend, &model)?;
    let formats = p.formats(&model);
    if !formats.contains(&call.format) {
        let takes: Vec<&str> = formats.iter().map(|f| f.name()).collect();
        return Err(refuse(
            400,
            format!(
                "{} takes {} requests, not {}",
                p.name(),
                takes.join(" or "),
                call.format.name()
            ),
        ));
    }
    call.body["model"] = json!(model);
    let user_agent = call.head.header("User-Agent");
    p.shape(call.format, &mut call.body, user_agent)
        .map_err(|e| refuse(400, format!("{e:#}")))?;
    let upstream = p
        .upstream(&creds, call.format, &model)
        .ok_or_else(|| refuse(503, format!("the active {} login holds no token", p.name())))?;
    let mut url = upstream.url;
    if call.count_tokens {
        let (path, query) = url
            .split_once('?')
            .map_or((url.as_str(), ""), |(a, b)| (a, b));
        if !path.ends_with("/messages") {
            return Err(refuse(501, format!("{} cannot count tokens", p.name())));
        }
        url = if query.is_empty() {
            format!("{path}/count_tokens")
        } else {
            format!("{path}/count_tokens?{query}")
        };
    }

    let client = client().ok_or_else(|| refuse(500, "no HTTP client"))?;
    let mut request = client.post(&url).header("content-type", "application/json");
    let set = |name: &str| {
        upstream
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    };
    for (name, value) in &upstream.headers {
        if name.eq_ignore_ascii_case("anthropic-beta") {
            continue;
        }
        request = request.header(*name, value);
    }
    for name in PASSED {
        if !set(name)
            && let Some(value) = call.head.header(name)
        {
            request = request.header(*name, value);
        }
    }
    if call.format == Format::Messages {
        if !set("anthropic-version") && call.head.header("anthropic-version").is_none() {
            request = request.header("anthropic-version", "2023-06-01");
        }
        let ours = upstream
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("anthropic-beta"))
            .map(|(_, v)| v.as_str());
        if let Some(betas) = merged_betas(ours, call.head.header("anthropic-beta")) {
            request = request.header("anthropic-beta", betas);
        }
    }
    let body = serde_json::to_vec(&call.body).map_err(|e| refuse(500, e.to_string()))?;
    let mut resp = request
        .body(body)
        .send()
        .map_err(|e| refuse(502, format!("{} could not be reached: {e}", p.name())))?;

    let status = resp.status().as_u16();
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let content_type = header("content-type").unwrap_or_else(|| "application/json".to_owned());
    let mut extra = Vec::new();
    for name in ["retry-after", "request-id", "x-request-id"] {
        if let Some(v) = header(name) {
            extra.push((name, v));
        }
    }
    let length = resp.content_length().map(|n| n as usize);
    if write_head(out, status, &content_type, length, &extra).is_err() {
        return Ok(());
    }
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match resp.read(&mut chunk) {
            Ok(0) | Err(_) => return Ok(()),
            Ok(n) => {
                if out
                    .write_all(&chunk[..n])
                    .and_then(|()| out.flush())
                    .is_err()
                {
                    return Ok(());
                }
            }
        }
    }
}

/// The models of every provider with an active login, in the shape the
/// client asked for.
pub fn models(backend: &dyn Backend, anthropic: bool) -> Value {
    let mut ids = Vec::new();
    for p in backend.providers() {
        let live = matches!(p.live(), Ok(Some(_)));
        if !live {
            continue;
        }
        for m in route::models(p.id()) {
            let formats = p.formats(route::bare(m));
            let reachable = if anthropic {
                formats.contains(&Format::Messages)
            } else {
                formats.iter().any(|f| *f != Format::Messages)
            };
            if reachable {
                ids.push((p.id(), *m));
            }
        }
    }
    if anthropic {
        let data: Vec<Value> = ids
            .iter()
            .map(|(_, id)| {
                json!({"type": "model", "id": id, "display_name": id, "created_at": "1970-01-01T00:00:00Z"})
            })
            .collect();
        json!({
            "data": data,
            "has_more": false,
            "first_id": ids.first().map(|(_, id)| id),
            "last_id": ids.last().map(|(_, id)| id),
        })
    } else {
        let data: Vec<Value> = ids
            .iter()
            .map(
                |(owner, id)| json!({"id": id, "object": "model", "created": 0, "owned_by": owner}),
            )
            .collect();
        json!({"object": "list", "data": data})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn betas_are_joined_once_each() {
        assert_eq!(
            merged_betas(Some("a,b"), Some("b, c")).as_deref(),
            Some("a,b,c")
        );
        assert_eq!(merged_betas(None, Some("x")).as_deref(), Some("x"));
        assert_eq!(merged_betas(None, None), None);
    }
}
