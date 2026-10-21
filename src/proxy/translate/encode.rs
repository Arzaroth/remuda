use std::io::Write;

use serde_json::{Value, json};

use super::sse;
use super::{Conversation, Event, Names, Stop, Usage};
use crate::fsx::now_ms;
use crate::provider::Format;

#[derive(Debug, Clone, PartialEq)]
enum Block {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    Tool {
        id: String,
        name: String,
        arguments: String,
    },
}

/// An answer as it builds up, whichever way it is written out.
#[derive(Debug, Default)]
struct Answer {
    id: String,
    blocks: Vec<Block>,
    usage: Usage,
    stop: Option<Stop>,
    error: Option<(String, String)>,
}

impl Answer {
    fn fold(&mut self, e: &Event, names: &Names) {
        match e {
            Event::Start { id, .. } => self.id = id.clone(),
            Event::Text(t) => match self.blocks.last_mut() {
                Some(Block::Text(s)) => s.push_str(t),
                _ => self.blocks.push(Block::Text(t.clone())),
            },
            Event::Thinking(t) => match self.blocks.last_mut() {
                Some(Block::Thinking {
                    text,
                    signature: None,
                }) => text.push_str(t),
                _ => self.blocks.push(Block::Thinking {
                    text: t.clone(),
                    signature: None,
                }),
            },
            Event::Signature(s) => match self.blocks.last_mut() {
                Some(Block::Thinking { signature, .. }) if signature.is_none() => {
                    *signature = Some(s.clone())
                }
                Some(Block::Thinking {
                    signature: Some(sig),
                    ..
                }) => sig.push_str(s),
                _ => self.blocks.push(Block::Thinking {
                    text: String::new(),
                    signature: Some(s.clone()),
                }),
            },
            Event::ToolCall { id, name } => self.blocks.push(Block::Tool {
                id: id.clone(),
                name: names.original(name),
                arguments: String::new(),
            }),
            Event::ToolArguments(a) => {
                if let Some(Block::Tool { arguments, .. }) = self.blocks.last_mut() {
                    arguments.push_str(a);
                }
            }
            Event::Usage(u) => self.usage = *u,
            Event::Stop(s) => self.stop = Some(*s),
            Event::Error { kind, message } => self.error = Some((kind.clone(), message.clone())),
        }
    }

    fn tools(&self) -> usize {
        self.blocks
            .iter()
            .filter(|b| matches!(b, Block::Tool { .. }))
            .count()
    }

    fn stop(&self) -> Stop {
        self.stop.unwrap_or(if self.tools() > 0 {
            Stop::ToolUse
        } else {
            Stop::End
        })
    }

    /// A stream that ends without saying how is cut short.
    fn cut(&self) -> Option<(String, String)> {
        self.error.clone().or_else(|| {
            self.stop.is_none().then(|| {
                (
                    "api_error".to_owned(),
                    "the provider's stream ended before the answer did".to_owned(),
                )
            })
        })
    }
}

fn anthropic_stop(s: Stop) -> &'static str {
    match s {
        Stop::End => "end_turn",
        Stop::MaxTokens => "max_tokens",
        Stop::ToolUse => "tool_use",
        Stop::Sequence => "stop_sequence",
        Stop::Refusal => "refusal",
    }
}

fn chat_finish(s: Stop) -> &'static str {
    match s {
        Stop::End | Stop::Sequence => "stop",
        Stop::MaxTokens => "length",
        Stop::ToolUse => "tool_calls",
        Stop::Refusal => "content_filter",
    }
}

fn anthropic_usage(u: &Usage) -> Value {
    json!({
        "input_tokens": u.input.saturating_sub(u.cache_read + u.cache_write),
        "output_tokens": u.output,
        "cache_read_input_tokens": u.cache_read,
        "cache_creation_input_tokens": u.cache_write,
    })
}

fn chat_usage(u: &Usage) -> Value {
    json!({
        "prompt_tokens": u.input,
        "completion_tokens": u.output,
        "total_tokens": u.input + u.output,
        "prompt_tokens_details": {"cached_tokens": u.cache_read},
        "completion_tokens_details": {"reasoning_tokens": u.reasoning},
    })
}

fn responses_usage(u: &Usage) -> Value {
    json!({
        "input_tokens": u.input,
        "input_tokens_details": {"cached_tokens": u.cache_read},
        "output_tokens": u.output,
        "output_tokens_details": {"reasoning_tokens": u.reasoning},
        "total_tokens": u.input + u.output,
    })
}

fn id_or(id: &str, prefix: &str) -> String {
    if id.is_empty() {
        format!("{prefix}{}", now_ms())
    } else {
        format!("{prefix}{id}")
    }
}

fn arguments_object(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => v,
        _ => json!({}),
    }
}

/// An error event in the client's stream format.
pub fn error_event(to: Format, kind: &str, message: &str) -> (Option<&'static str>, Value) {
    match to {
        Format::Messages => (
            Some("error"),
            json!({"type": "error", "error": {"type": if kind.is_empty() { "api_error" } else { kind }, "message": message}}),
        ),
        Format::Responses => (
            Some("error"),
            json!({"type": "error", "code": kind, "message": message}),
        ),
        Format::Chat => (None, json!({"error": {"message": message, "type": kind}})),
    }
}

fn responses_item(b: &Block, index: usize, done: bool) -> Value {
    let status = if done { "completed" } else { "in_progress" };
    match b {
        Block::Text(t) => json!({
            "id": format!("msg_{index}"),
            "type": "message",
            "role": "assistant",
            "status": status,
            "content": if done {
                json!([{"type": "output_text", "text": t, "annotations": []}])
            } else {
                json!([])
            },
        }),
        Block::Thinking { text, .. } => json!({
            "id": format!("rs_{index}"),
            "type": "reasoning",
            "summary": if done && !text.is_empty() {
                json!([{"type": "summary_text", "text": text}])
            } else {
                json!([])
            },
        }),
        Block::Tool {
            id,
            name,
            arguments,
        } => json!({
            "id": format!("fc_{index}"),
            "type": "function_call",
            "status": status,
            "call_id": id,
            "name": name,
            "arguments": if done { arguments.as_str() } else { "" },
        }),
    }
}

fn whole_value(to: Format, c: &Conversation, a: &Answer) -> Value {
    match to {
        Format::Messages => {
            let content: Vec<Value> = a
                .blocks
                .iter()
                .map(|b| match b {
                    Block::Text(t) => json!({"type": "text", "text": t}),
                    Block::Thinking { text, signature } => json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": signature.clone().unwrap_or_default(),
                    }),
                    Block::Tool {
                        id,
                        name,
                        arguments,
                    } => json!({"type": "tool_use", "id": id, "name": name, "input": arguments_object(arguments)}),
                })
                .collect();
            json!({
                "id": id_or(&a.id, "msg_"),
                "type": "message",
                "role": "assistant",
                "model": c.model,
                "content": content,
                "stop_reason": anthropic_stop(a.stop()),
                "stop_sequence": null,
                "usage": anthropic_usage(&a.usage),
            })
        }
        Format::Chat => {
            let text: String = a
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect();
            let reasoning: String = a
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Thinking { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let calls: Vec<Value> = a
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Tool {
                        id,
                        name,
                        arguments,
                    } => Some(json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments}})),
                    _ => None,
                })
                .collect();
            let mut message = json!({
                "role": "assistant",
                "content": if text.is_empty() && !calls.is_empty() { Value::Null } else { json!(text) },
            });
            if !reasoning.is_empty() {
                message["reasoning_content"] = json!(reasoning);
            }
            if !calls.is_empty() {
                message["tool_calls"] = json!(calls);
            }
            json!({
                "id": id_or(&a.id, "chatcmpl-"),
                "object": "chat.completion",
                "created": now_ms() / 1000,
                "model": c.model,
                "choices": [{"index": 0, "message": message, "finish_reason": chat_finish(a.stop())}],
                "usage": chat_usage(&a.usage),
            })
        }
        Format::Responses => {
            let output: Vec<Value> = a
                .blocks
                .iter()
                .enumerate()
                .map(|(i, b)| responses_item(b, i, true))
                .collect();
            let incomplete = a.stop() == Stop::MaxTokens;
            json!({
                "id": id_or(&a.id, "resp_"),
                "object": "response",
                "created_at": now_ms() / 1000,
                "status": if incomplete { "incomplete" } else { "completed" },
                "incomplete_details": if incomplete { json!({"reason": "max_output_tokens"}) } else { Value::Null },
                "model": c.model,
                "output": output,
                "usage": responses_usage(&a.usage),
            })
        }
    }
}

/// The whole answer as one body, or the error that ended it.
pub fn whole(
    to: Format,
    c: &Conversation,
    names: &Names,
    events: impl Iterator<Item = Event>,
) -> Result<Value, (String, String)> {
    let mut a = Answer::default();
    for e in events {
        a.fold(&e, names);
    }
    match a.cut() {
        Some(e) => Err(e),
        None => Ok(whole_value(to, c, &a)),
    }
}

/// Writes the answer as the client's stream, event by event.
pub fn stream(
    to: Format,
    c: &Conversation,
    names: &Names,
    events: impl Iterator<Item = Event>,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let mut w = Writer {
        to,
        c,
        out,
        seq: 0,
        started: false,
    };
    let mut a = Answer::default();
    for e in events {
        if !w.started {
            if let Event::Start { id, .. } = &e {
                a.id = id.clone();
            }
            w.start(&a)?;
        }
        let before = a.blocks.len();
        a.fold(&e, names);
        if a.blocks.len() > before {
            if before > 0 {
                w.close(&a, before - 1)?;
            }
            w.open(&a, a.blocks.len() - 1)?;
        }
        let index = a.blocks.len().saturating_sub(1);
        match &e {
            Event::Text(t) => w.delta(&a, index, Delta::Text(t))?,
            Event::Thinking(t) => w.delta(&a, index, Delta::Thinking(t))?,
            Event::Signature(s) => w.delta(&a, index, Delta::Signature(s))?,
            Event::ToolArguments(s) if matches!(a.blocks.last(), Some(Block::Tool { .. })) => {
                w.delta(&a, index, Delta::Arguments(s))?
            }
            Event::Error { .. } => break,
            _ => {}
        }
    }
    if !w.started {
        w.start(&a)?;
    }
    if let Some((kind, message)) = a.cut() {
        let (event, data) = error_event(to, &kind, &message);
        sse::write(w.out, event, &data.to_string())?;
        if to == Format::Chat {
            sse::write(w.out, None, "[DONE]")?;
        }
        return Ok(());
    }
    if !a.blocks.is_empty() {
        w.close(&a, a.blocks.len() - 1)?;
    }
    w.finish(&a)
}

enum Delta<'a> {
    Text(&'a str),
    Thinking(&'a str),
    Signature(&'a str),
    Arguments(&'a str),
}

struct Writer<'a, W> {
    to: Format,
    c: &'a Conversation,
    out: &'a mut W,
    seq: u64,
    started: bool,
}

impl<W: Write> Writer<'_, W> {
    fn send(&mut self, event: Option<&str>, mut data: Value) -> std::io::Result<()> {
        if self.to == Format::Responses {
            data["sequence_number"] = json!(self.seq);
            self.seq += 1;
        }
        sse::write(self.out, event, &data.to_string())
    }

    fn chat(&mut self, a: &Answer, delta: Value, finish: Value) -> std::io::Result<()> {
        let chunk = json!({
            "id": id_or(&a.id, "chatcmpl-"),
            "object": "chat.completion.chunk",
            "created": now_ms() / 1000,
            "model": self.c.model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        });
        self.send(None, chunk)
    }

    fn response_shell(&self, a: &Answer, status: &str) -> Value {
        json!({
            "id": id_or(&a.id, "resp_"),
            "object": "response",
            "created_at": now_ms() / 1000,
            "status": status,
            "model": self.c.model,
            "output": [],
        })
    }

    fn start(&mut self, a: &Answer) -> std::io::Result<()> {
        self.started = true;
        match self.to {
            Format::Messages => self.send(
                Some("message_start"),
                json!({"type": "message_start", "message": {
                    "id": id_or(&a.id, "msg_"),
                    "type": "message",
                    "role": "assistant",
                    "model": self.c.model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0},
                }}),
            ),
            Format::Chat => self.chat(a, json!({"role": "assistant", "content": ""}), Value::Null),
            Format::Responses => {
                let shell = self.response_shell(a, "in_progress");
                self.send(
                    Some("response.created"),
                    json!({"type": "response.created", "response": shell}),
                )?;
                self.send(
                    Some("response.in_progress"),
                    json!({"type": "response.in_progress", "response": shell}),
                )
            }
        }
    }

    fn open(&mut self, a: &Answer, i: usize) -> std::io::Result<()> {
        let b = &a.blocks[i];
        match self.to {
            Format::Messages => {
                let block = match b {
                    Block::Text(_) => json!({"type": "text", "text": ""}),
                    Block::Thinking { .. } => {
                        json!({"type": "thinking", "thinking": "", "signature": ""})
                    }
                    Block::Tool { id, name, .. } => {
                        json!({"type": "tool_use", "id": id, "name": name, "input": {}})
                    }
                };
                self.send(
                    Some("content_block_start"),
                    json!({"type": "content_block_start", "index": i, "content_block": block}),
                )
            }
            Format::Chat => match b {
                Block::Tool { id, name, .. } => {
                    let index = a.blocks[..i]
                        .iter()
                        .filter(|b| matches!(b, Block::Tool { .. }))
                        .count();
                    self.chat(
                        a,
                        json!({"tool_calls": [{"index": index, "id": id, "type": "function", "function": {"name": name, "arguments": ""}}]}),
                        Value::Null,
                    )
                }
                _ => Ok(()),
            },
            Format::Responses => {
                self.send(
                    Some("response.output_item.added"),
                    json!({"type": "response.output_item.added", "output_index": i, "item": responses_item(b, i, false)}),
                )?;
                match b {
                    Block::Text(_) => self.send(
                        Some("response.content_part.added"),
                        json!({"type": "response.content_part.added", "item_id": format!("msg_{i}"), "output_index": i,
                               "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}}),
                    ),
                    Block::Thinking { .. } => self.send(
                        Some("response.reasoning_summary_part.added"),
                        json!({"type": "response.reasoning_summary_part.added", "item_id": format!("rs_{i}"), "output_index": i,
                               "summary_index": 0, "part": {"type": "summary_text", "text": ""}}),
                    ),
                    Block::Tool { .. } => Ok(()),
                }
            }
        }
    }

    fn delta(&mut self, a: &Answer, i: usize, d: Delta) -> std::io::Result<()> {
        match self.to {
            Format::Messages => {
                let delta = match d {
                    Delta::Text(t) => json!({"type": "text_delta", "text": t}),
                    Delta::Thinking(t) => json!({"type": "thinking_delta", "thinking": t}),
                    Delta::Signature(s) => json!({"type": "signature_delta", "signature": s}),
                    Delta::Arguments(s) => json!({"type": "input_json_delta", "partial_json": s}),
                };
                self.send(
                    Some("content_block_delta"),
                    json!({"type": "content_block_delta", "index": i, "delta": delta}),
                )
            }
            Format::Chat => match d {
                Delta::Text(t) => self.chat(a, json!({"content": t}), Value::Null),
                Delta::Thinking(t) => self.chat(a, json!({"reasoning_content": t}), Value::Null),
                Delta::Signature(_) => Ok(()),
                Delta::Arguments(s) => {
                    let index = a.tools().saturating_sub(1);
                    self.chat(
                        a,
                        json!({"tool_calls": [{"index": index, "function": {"arguments": s}}]}),
                        Value::Null,
                    )
                }
            },
            Format::Responses => match d {
                Delta::Text(t) => self.send(
                    Some("response.output_text.delta"),
                    json!({"type": "response.output_text.delta", "item_id": format!("msg_{i}"), "output_index": i, "content_index": 0, "delta": t}),
                ),
                Delta::Thinking(t) => self.send(
                    Some("response.reasoning_summary_text.delta"),
                    json!({"type": "response.reasoning_summary_text.delta", "item_id": format!("rs_{i}"), "output_index": i, "summary_index": 0, "delta": t}),
                ),
                Delta::Signature(_) => Ok(()),
                Delta::Arguments(s) => self.send(
                    Some("response.function_call_arguments.delta"),
                    json!({"type": "response.function_call_arguments.delta", "item_id": format!("fc_{i}"), "output_index": i, "delta": s}),
                ),
            },
        }
    }

    fn close(&mut self, a: &Answer, i: usize) -> std::io::Result<()> {
        let b = &a.blocks[i];
        match self.to {
            Format::Messages => self.send(
                Some("content_block_stop"),
                json!({"type": "content_block_stop", "index": i}),
            ),
            Format::Chat => Ok(()),
            Format::Responses => {
                match b {
                    Block::Text(t) => {
                        self.send(
                            Some("response.output_text.done"),
                            json!({"type": "response.output_text.done", "item_id": format!("msg_{i}"), "output_index": i, "content_index": 0, "text": t}),
                        )?;
                        self.send(
                            Some("response.content_part.done"),
                            json!({"type": "response.content_part.done", "item_id": format!("msg_{i}"), "output_index": i, "content_index": 0,
                                   "part": {"type": "output_text", "text": t, "annotations": []}}),
                        )?;
                    }
                    Block::Thinking { text, .. } => {
                        self.send(
                            Some("response.reasoning_summary_text.done"),
                            json!({"type": "response.reasoning_summary_text.done", "item_id": format!("rs_{i}"), "output_index": i, "summary_index": 0, "text": text}),
                        )?;
                        self.send(
                            Some("response.reasoning_summary_part.done"),
                            json!({"type": "response.reasoning_summary_part.done", "item_id": format!("rs_{i}"), "output_index": i, "summary_index": 0,
                                   "part": {"type": "summary_text", "text": text}}),
                        )?;
                    }
                    Block::Tool { arguments, .. } => {
                        self.send(
                            Some("response.function_call_arguments.done"),
                            json!({"type": "response.function_call_arguments.done", "item_id": format!("fc_{i}"), "output_index": i, "arguments": arguments}),
                        )?;
                    }
                }
                self.send(
                    Some("response.output_item.done"),
                    json!({"type": "response.output_item.done", "output_index": i, "item": responses_item(b, i, true)}),
                )
            }
        }
    }

    fn finish(&mut self, a: &Answer) -> std::io::Result<()> {
        match self.to {
            Format::Messages => {
                self.send(
                    Some("message_delta"),
                    json!({"type": "message_delta", "delta": {"stop_reason": anthropic_stop(a.stop()), "stop_sequence": null},
                           "usage": anthropic_usage(&a.usage)}),
                )?;
                self.send(Some("message_stop"), json!({"type": "message_stop"}))
            }
            Format::Chat => {
                self.chat(a, json!({}), json!(chat_finish(a.stop())))?;
                if self.c.stream_usage {
                    let chunk = json!({
                        "id": id_or(&a.id, "chatcmpl-"),
                        "object": "chat.completion.chunk",
                        "created": now_ms() / 1000,
                        "model": self.c.model,
                        "choices": [],
                        "usage": chat_usage(&a.usage),
                    });
                    self.send(None, chunk)?;
                }
                sse::write(self.out, None, "[DONE]")
            }
            Format::Responses => {
                let full = whole_value(Format::Responses, self.c, a);
                let kind = if full["status"] == "incomplete" {
                    "response.incomplete"
                } else {
                    "response.completed"
                };
                self.send(Some(kind), json!({"type": kind, "response": full}))
            }
        }
    }
}
