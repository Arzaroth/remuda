//! Between the three formats, through one conversation shape and one stream
//! of events: a request is parsed into a [`Conversation`] and written out in
//! the provider's format, and its answer is decoded into [`Event`]s and
//! written back in the client's.

mod decode;
mod emit;
mod encode;
mod parse;
mod sse;

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};

use anyhow::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::provider::Format;

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Image {
    Base64 { media_type: String, data: String },
    Url(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Image(Image),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        /// JSON text, as the OpenAI formats carry it.
        arguments: String,
    },
    ToolResult {
        id: String,
        content: Vec<Part>,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: Option<String>,
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Named(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    fn from_budget(tokens: u64) -> Effort {
        match tokens {
            0..4096 => Effort::Low,
            4096..16384 => Effort::Medium,
            _ => Effort::High,
        }
    }

    fn budget(self) -> u64 {
        match self {
            Effort::Low => 2048,
            Effort::Medium => 8192,
            Effort::High => 24576,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }

    fn named(name: &str) -> Option<Effort> {
        match name {
            "minimal" | "low" => Some(Effort::Low),
            "medium" => Some(Effort::Medium),
            "high" | "xhigh" => Some(Effort::High),
            _ => None,
        }
    }
}

/// A request, whatever format it came in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Conversation {
    pub model: String,
    pub system: Vec<String>,
    pub turns: Vec<Turn>,
    pub tools: Vec<Tool>,
    pub tool_choice: Option<ToolChoice>,
    pub parallel_tools: Option<bool>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Vec<String>,
    pub reasoning: Option<Effort>,
    pub stream: bool,
    /// Chat clients ask for a usage chunk at the end of a stream.
    pub stream_usage: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Stop {
    #[default]
    End,
    MaxTokens,
    ToolUse,
    Sequence,
    Refusal,
}

/// Input counts every prompt token, cached or not.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
}

/// One step of an answer, whatever format it came in.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Start { id: String },
    Text(String),
    Thinking(String),
    Signature(String),
    ToolCall { id: String, name: String },
    ToolArguments(String),
    Usage(Usage),
    Stop(Stop),
    Error { kind: String, message: String },
}

fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tool-call arguments as an object; anything else is sent as none.
fn arguments(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => v,
        _ => serde_json::json!({}),
    }
}

/// Codex takes tool names of at most 64 characters; Claude Code's MCP tools
/// run longer. A long name is cut and tagged with its digest, and put back on
/// the way out.
const MAX_TOOL_NAME: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct Names {
    short: HashMap<String, String>,
}

impl Names {
    fn shorten(&mut self, name: &str, limit: Option<usize>) -> String {
        let Some(limit) = limit.filter(|l| name.len() > *l) else {
            return name.to_owned();
        };
        let digest: String = Sha256::digest(name.as_bytes())
            .iter()
            .take(4)
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut cut = limit - digest.len() - 1;
        while !name.is_char_boundary(cut) {
            cut -= 1;
        }
        let short = format!("{}_{digest}", &name[..cut]);
        self.short.insert(short.clone(), name.to_owned());
        short
    }

    pub fn original(&self, name: &str) -> String {
        self.short
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }
}

/// `body` in `from`, rewritten for an endpoint that takes `to`, always asking
/// it to stream. Also returns the tool names it shortened and whether the
/// client asked for a stream.
pub fn request(from: Format, to: Format, body: &Value) -> Result<(Value, Names, Conversation)> {
    let conversation = parse::parse(from, body)?;
    let mut names = Names::default();
    let limit = (to == Format::Responses).then_some(MAX_TOOL_NAME);
    let out = emit::emit(to, &conversation, &mut names, limit);
    Ok((out, names, conversation))
}

/// What a client was answered: the answer's id as the client got it, its
/// usage, and whether it ended in an error.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    pub id: Option<String>,
    pub usage: Option<Usage>,
    pub failed: bool,
}

impl Seen {
    fn see(&mut self, e: &Event) {
        match e {
            Event::Start { id } if !id.is_empty() => self.id = Some(id.clone()),
            Event::Usage(u) => self.usage = Some(*u),
            Event::Error { .. } => self.failed = true,
            _ => {}
        }
    }

    /// One whole answer body in `format`.
    pub fn whole(format: Format, body: &Value) -> Seen {
        let (id, usage) = decode::whole(format, body);
        Seen {
            id: (!id.is_empty()).then_some(id),
            usage: Some(usage),
            failed: false,
        }
    }
}

/// The id a client in `to` is shown for an upstream answer `id`, as the
/// encoder writes it.
fn shown_id(to: Format, id: &str) -> String {
    let prefix = match to {
        Format::Messages => "msg_",
        Format::Chat => "chatcmpl-",
        Format::Responses => "resp_",
    };
    encode::id_or(id, prefix)
}

/// Copies an upstream stream in `format` to the client as it arrives, reading
/// its id and usage on the way. Returns false when the client went away.
pub fn relay(format: Format, upstream: impl Read, out: &mut impl Write, seen: &mut Seen) -> bool {
    let mut relay = Relay {
        upstream,
        out,
        gone: false,
    };
    {
        let mut reader = std::io::BufReader::new(&mut relay);
        for e in decode::decode(format, &mut reader) {
            seen.see(&e);
        }
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    }
    !relay.gone
}

/// A reader that writes on to the client whatever it reads, and ends when the
/// client is gone.
struct Relay<'a, R, W> {
    upstream: R,
    out: &'a mut W,
    gone: bool,
}

impl<R: Read, W: Write> Read for Relay<'_, R, W> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.gone {
            return Ok(0);
        }
        let n = self.upstream.read(buf).unwrap_or(0);
        if n > 0
            && self
                .out
                .write_all(&buf[..n])
                .and_then(|()| self.out.flush())
                .is_err()
        {
            self.gone = true;
            return Ok(0);
        }
        Ok(n)
    }
}

/// Reads an upstream stream in `from` and writes the answer the client asked
/// for in `to`: a stream when it streamed, else one JSON body. `head` writes
/// the status line and headers for a status and content type. What the
/// client was told goes into `seen`.
#[allow(clippy::too_many_arguments)]
pub fn respond<W: Write>(
    from: Format,
    to: Format,
    conversation: &Conversation,
    names: &Names,
    upstream: impl BufRead,
    out: &mut W,
    head: impl FnOnce(&mut W, u16, &str, Option<usize>) -> std::io::Result<()>,
    seen: &mut Seen,
) -> std::io::Result<()> {
    let result = answer(from, to, conversation, names, upstream, out, head, seen);
    if let Some(id) = &seen.id {
        seen.id = Some(shown_id(to, id));
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn answer<W: Write>(
    from: Format,
    to: Format,
    conversation: &Conversation,
    names: &Names,
    upstream: impl BufRead,
    out: &mut W,
    head: impl FnOnce(&mut W, u16, &str, Option<usize>) -> std::io::Result<()>,
    seen: &mut Seen,
) -> std::io::Result<()> {
    let events = decode::decode(from, upstream).inspect(|e| seen.see(e));
    if conversation.stream {
        head(out, 200, "text/event-stream", None)?;
        return encode::stream(to, conversation, names, events, out);
    }
    let (status, body) = match encode::whole(to, conversation, names, events) {
        Ok(body) => (200, body),
        Err((kind, message)) => {
            let status = status_of(&kind);
            (
                status,
                super::listen::error_body(Some(to), status, &message),
            )
        }
    };
    let body = body.to_string();
    head(out, status, "application/json", Some(body.len()))?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

/// A Responses stream collected into the response it describes, with every
/// item as the upstream sent it: the completed event's own `output` can be
/// empty.
pub fn collect(upstream: impl BufRead) -> Result<Value, (String, String)> {
    let mut items: Vec<(u64, Value)> = Vec::new();
    for e in sse::Reader::new(upstream) {
        let Ok(d) = serde_json::from_str::<Value>(&e.data) else {
            continue;
        };
        let kind = e
            .event
            .or_else(|| d["type"].as_str().map(str::to_owned))
            .unwrap_or_default();
        match kind.as_str() {
            "response.output_item.done" => {
                let index = d["output_index"].as_u64().unwrap_or(items.len() as u64);
                items.push((index, d["item"].clone()));
            }
            "response.completed" | "response.incomplete" => {
                let mut response = d["response"].clone();
                if !items.is_empty() {
                    items.sort_by_key(|(i, _)| *i);
                    response["output"] = Value::Array(items.into_iter().map(|(_, v)| v).collect());
                }
                return Ok(response);
            }
            "response.failed" => {
                return Err((
                    d.pointer("/response/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    d.pointer("/response/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                ));
            }
            "error" => {
                return Err((
                    d["code"].as_str().unwrap_or_default().to_owned(),
                    d["message"].as_str().unwrap_or_default().to_owned(),
                ));
            }
            _ => {}
        }
    }
    Err((
        "api_error".to_owned(),
        "the provider's stream ended before the answer did".to_owned(),
    ))
}

/// Reads a stream to its end: whether the answer stopped, or the error it
/// ended on.
pub fn finished(from: Format, upstream: impl BufRead) -> Result<(), String> {
    for e in decode::decode(from, upstream) {
        match e {
            Event::Stop(_) => return Ok(()),
            Event::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
    Err("the stream ended before the answer did".to_owned())
}

/// An upstream's error body, retold in the client's shape.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = v
        .pointer("/error/message")
        .or_else(|| v.pointer("/message"))
        .or_else(|| v.pointer("/detail"))
        .and_then(Value::as_str);
    message.unwrap_or(body).chars().take(500).collect()
}

/// The status an error in a stream would have had, had it come first.
pub fn status_of(kind: &str) -> u16 {
    if kind.contains("rate_limit") {
        429
    } else if kind.contains("overloaded") {
        529
    } else if kind.contains("invalid_request") {
        400
    } else {
        502
    }
}

#[cfg(test)]
mod tests;
