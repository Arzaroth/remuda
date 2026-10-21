use std::collections::{HashSet, VecDeque};
use std::io::BufRead;

use serde_json::Value;

use super::emit::SIGNATURE;
use super::sse::Reader;
use super::{Event, Stop, Usage};
use crate::provider::Format;

/// The events of an upstream stream, read as they arrive.
pub fn decode(from: Format, upstream: impl BufRead) -> impl Iterator<Item = Event> {
    Decoder {
        from,
        sse: Reader::new(upstream),
        pending: VecDeque::new(),
        usage: Usage::default(),
        tools: HashSet::new(),
        calling: None,
        argued: HashSet::new(),
        calls: HashSet::new(),
        finish: None,
        started: false,
        done: false,
    }
}

struct Decoder<R> {
    from: Format,
    sse: Reader<R>,
    pending: VecDeque<Event>,
    usage: Usage,
    /// Responses: the calls this answer made, so it ends in tool use.
    tools: HashSet<String>,
    /// Responses: the call whose arguments are streaming.
    calling: Option<String>,
    /// Responses: the calls whose arguments came in deltas.
    argued: HashSet<String>,
    /// Chat: the tool call indices already started.
    calls: HashSet<u64>,
    /// Chat: how the answer stopped, said before its usage chunk.
    finish: Option<Stop>,
    started: bool,
    done: bool,
}

impl<R: BufRead> Iterator for Decoder<R> {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        loop {
            if let Some(e) = self.pending.pop_front() {
                return Some(e);
            }
            if self.done {
                return None;
            }
            let Some(sse) = self.sse.next() else {
                self.end();
                continue;
            };
            if sse.data.trim() == "[DONE]" {
                self.end();
                continue;
            }
            let Ok(data) = serde_json::from_str::<Value>(&sse.data) else {
                continue;
            };
            let kind = sse
                .event
                .or_else(|| data.get("type").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_default();
            match self.from {
                Format::Messages => self.messages(&kind, &data),
                Format::Responses => self.responses(&kind, &data),
                Format::Chat => self.chat(&data),
            }
        }
    }
}

fn text(v: &Value, pointer: &str) -> String {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn count(v: &Value, pointer: &str) -> u64 {
    v.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

fn anthropic_stop(reason: &str) -> Stop {
    match reason {
        "max_tokens" | "model_context_window_exceeded" => Stop::MaxTokens,
        "tool_use" => Stop::ToolUse,
        "stop_sequence" => Stop::Sequence,
        "refusal" => Stop::Refusal,
        _ => Stop::End,
    }
}

fn chat_stop(reason: &str) -> Stop {
    match reason {
        "length" => Stop::MaxTokens,
        "tool_calls" | "function_call" => Stop::ToolUse,
        "content_filter" => Stop::Refusal,
        _ => Stop::End,
    }
}

impl<R> Decoder<R> {
    fn push(&mut self, e: Event) {
        self.pending.push_back(e);
    }

    /// A chat stream says how it stopped before its usage, and ends with
    /// `[DONE]` or by closing.
    fn end(&mut self) {
        if let Some(stop) = self.finish.take() {
            self.push(Event::Usage(self.usage));
            self.push(Event::Stop(stop));
        }
        self.done = true;
    }

    fn chat(&mut self, d: &Value) {
        if let Some(e) = d.get("error") {
            self.push(Event::Error {
                kind: text(e, "/type"),
                message: text(e, "/message"),
            });
            self.done = true;
            return;
        }
        if !self.started {
            self.started = true;
            self.push(Event::Start { id: text(d, "/id") });
        }
        if let Some(u) = d.get("usage").filter(|u| u.is_object()) {
            self.usage = Usage {
                input: count(u, "/prompt_tokens"),
                output: count(u, "/completion_tokens"),
                cache_read: count(u, "/prompt_tokens_details/cached_tokens"),
                cache_write: 0,
                reasoning: count(u, "/completion_tokens_details/reasoning_tokens"),
            };
        }
        let Some(choice) = d.pointer("/choices/0") else {
            return;
        };
        let delta = &choice["delta"];
        for key in ["/reasoning_content", "/reasoning"] {
            let t = text(delta, key);
            if !t.is_empty() {
                self.push(Event::Thinking(t));
            }
        }
        let t = text(delta, "/content");
        if !t.is_empty() {
            self.push(Event::Text(t));
        }
        for (i, call) in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(i as u64);
            if self.calls.insert(index) {
                self.push(Event::ToolCall {
                    id: text(call, "/id"),
                    name: text(call, "/function/name"),
                });
            }
            let args = text(call, "/function/arguments");
            if !args.is_empty() {
                self.push(Event::ToolArguments(args));
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish = Some(chat_stop(reason));
        }
    }

    fn messages(&mut self, kind: &str, d: &Value) {
        match kind {
            "message_start" => {
                let m = &d["message"];
                self.usage.cache_read = count(m, "/usage/cache_read_input_tokens");
                self.usage.cache_write = count(m, "/usage/cache_creation_input_tokens");
                self.usage.input = count(m, "/usage/input_tokens")
                    + self.usage.cache_read
                    + self.usage.cache_write;
                self.usage.output = count(m, "/usage/output_tokens");
                self.push(Event::Start { id: text(m, "/id") });
            }
            "content_block_start" => {
                let b = &d["content_block"];
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let t = text(b, "/text");
                        if !t.is_empty() {
                            self.push(Event::Text(t));
                        }
                    }
                    Some("thinking") => {
                        let t = text(b, "/thinking");
                        if !t.is_empty() {
                            self.push(Event::Thinking(t));
                        }
                    }
                    Some("tool_use") => self.push(Event::ToolCall {
                        id: text(b, "/id"),
                        name: text(b, "/name"),
                    }),
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = &d["delta"];
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => self.push(Event::Text(text(delta, "/text"))),
                    Some("thinking_delta") => self.push(Event::Thinking(text(delta, "/thinking"))),
                    Some("signature_delta") => {
                        self.push(Event::Signature(text(delta, "/signature")))
                    }
                    Some("input_json_delta") => {
                        self.push(Event::ToolArguments(text(delta, "/partial_json")))
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(out) = d.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    self.usage.output = out;
                }
                self.push(Event::Usage(self.usage));
                if let Some(reason) = d.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.push(Event::Stop(anthropic_stop(reason)));
                }
            }
            "message_stop" => self.done = true,
            "error" => {
                self.push(Event::Error {
                    kind: text(d, "/error/type"),
                    message: text(d, "/error/message"),
                });
                self.done = true;
            }
            _ => {}
        }
    }

    fn responses(&mut self, kind: &str, d: &Value) {
        match kind {
            "response.created" => self.push(Event::Start {
                id: text(d, "/response/id"),
            }),
            "response.output_item.added" => {
                let item = &d["item"];
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let id = text(item, "/call_id");
                    self.tools.insert(id.clone());
                    self.calling = Some(id.clone());
                    self.push(Event::ToolCall {
                        id,
                        name: text(item, "/name"),
                    });
                }
            }
            "response.output_text.delta" => self.push(Event::Text(text(d, "/delta"))),
            "response.reasoning_summary_part.added"
                if d.get("summary_index").and_then(Value::as_u64) > Some(0) =>
            {
                self.push(Event::Thinking("\n\n".to_owned()))
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                self.push(Event::Thinking(text(d, "/delta")))
            }
            "response.function_call_arguments.delta" => {
                if let Some(id) = self.calling.clone() {
                    self.argued.insert(id);
                }
                self.push(Event::ToolArguments(text(d, "/delta")))
            }
            "response.output_item.done" => {
                let item = &d["item"];
                match item.get("type").and_then(Value::as_str) {
                    Some("reasoning") => {
                        if let Some(enc) = item.get("encrypted_content").and_then(Value::as_str) {
                            self.push(Event::Signature(format!("{SIGNATURE}{enc}")));
                        }
                    }
                    Some("function_call") => {
                        let id = text(item, "/call_id");
                        if !self.argued.contains(&id) {
                            let args = text(item, "/arguments");
                            if !args.is_empty() {
                                self.push(Event::ToolArguments(args));
                            }
                        }
                    }
                    _ => {}
                }
            }
            "response.completed" | "response.incomplete" => {
                let r = &d["response"];
                self.usage = Usage {
                    input: count(r, "/usage/input_tokens"),
                    output: count(r, "/usage/output_tokens"),
                    cache_read: count(r, "/usage/input_tokens_details/cached_tokens"),
                    cache_write: 0,
                    reasoning: count(r, "/usage/output_tokens_details/reasoning_tokens"),
                };
                self.push(Event::Usage(self.usage));
                let stop = match r
                    .pointer("/incomplete_details/reason")
                    .and_then(Value::as_str)
                {
                    Some("max_output_tokens") => Stop::MaxTokens,
                    Some("content_filter") => Stop::Refusal,
                    _ if !self.tools.is_empty() => Stop::ToolUse,
                    _ => Stop::End,
                };
                self.push(Event::Stop(stop));
                self.done = true;
            }
            "response.failed" => {
                self.push(Event::Error {
                    kind: text(d, "/response/error/code"),
                    message: text(d, "/response/error/message"),
                });
                self.done = true;
            }
            "error" => {
                self.push(Event::Error {
                    kind: text(d, "/code"),
                    message: text(d, "/message"),
                });
                self.done = true;
            }
            _ => {}
        }
    }
}
