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
use std::io::{BufRead, Write};

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
    Start { id: String, model: String },
    Text(String),
    Thinking(String),
    Signature(String),
    ToolCall { id: String, name: String },
    ToolArguments(String),
    Usage(Usage),
    Stop(Stop),
    Error { kind: String, message: String },
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

/// Reads an upstream stream in `from` and writes the answer the client asked
/// for in `to`: a stream when it streamed, else one JSON body. `head` writes
/// the status line and headers for a status and content type.
pub fn respond<W: Write>(
    from: Format,
    to: Format,
    conversation: &Conversation,
    names: &Names,
    upstream: impl BufRead,
    out: &mut W,
    head: impl FnOnce(&mut W, u16, &str, Option<usize>) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let events = decode::decode(from, upstream);
    if conversation.stream {
        head(out, 200, "text/event-stream", None)?;
        return encode::stream(to, conversation, names, events, out);
    }
    let (status, body) = match encode::whole(to, conversation, names, events) {
        Ok(body) => (200, body),
        Err((_, message)) => (502, super::listen::error_body(Some(to), 502, &message)),
    };
    let body = body.to_string();
    head(out, status, "application/json", Some(body.len()))?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

/// An upstream's error body, retold in the client's shape.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = v
        .pointer("/error/message")
        .or_else(|| v.pointer("/message"))
        .or_else(|| v.pointer("/detail"))
        .and_then(Value::as_str);
    match message {
        Some(m) => m.to_owned(),
        None => body.chars().take(500).collect(),
    }
}

#[cfg(test)]
mod tests;
