use serde_json::{Map, Value, json};

use super::{Conversation, Image, Names, Part, Role, ToolChoice, Turn, arguments, text_of};
use crate::provider::Format;

/// Signatures remuda hands a client in another provider's format, tagged
/// with their maker: each provider refuses the other's.
pub const SIGNATURE: &str = "remuda-reasoning:";
pub const CLAUDE_SIGNATURE: &str = "remuda-claude:";

const DEFAULT_MAX_TOKENS: u64 = 32_000;

pub fn emit(to: Format, c: &Conversation, names: &mut Names, limit: Option<usize>) -> Value {
    match to {
        Format::Messages => messages(c),
        Format::Responses => responses(c, names, limit),
        Format::Chat => chat(c),
    }
}

fn anthropic_image(image: &Image) -> Value {
    match image {
        Image::Base64 { media_type, data } => {
            json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}})
        }
        Image::Url(url) => json!({"type": "image", "source": {"type": "url", "url": url}}),
    }
}

fn anthropic_block(p: &Part) -> Option<Value> {
    match p {
        Part::Text(t) if !t.is_empty() => Some(json!({"type": "text", "text": t})),
        Part::Image(i) => Some(anthropic_image(i)),
        Part::ToolCall {
            id,
            name,
            arguments: args,
        } => Some(json!({"type": "tool_use", "id": id, "name": name, "input": arguments(args)})),
        Part::ToolResult {
            id,
            content,
            is_error,
        } => {
            let blocks: Vec<Value> = content.iter().filter_map(anthropic_block).collect();
            let mut b = json!({"type": "tool_result", "tool_use_id": id, "content": blocks});
            if *is_error {
                b["is_error"] = json!(true);
            }
            Some(b)
        }
        Part::Thinking {
            text,
            signature: Some(sig),
        } if sig.starts_with(CLAUDE_SIGNATURE) => Some(json!({
            "type": "thinking",
            "thinking": text,
            "signature": &sig[CLAUDE_SIGNATURE.len()..],
        })),
        Part::Thinking { .. } | Part::Text(_) => None,
    }
}

/// Claude 4.6 and later think adaptively with an effort, and from 4.7 refuse a
/// token budget; earlier models take only the budget. The version is read off
/// the name (`claude-sonnet-4-5`, `claude-3-7-sonnet`, `claude-opus-5-5`), and
/// a name without one is taken to be new.
fn adaptive(model: &str) -> bool {
    let numbers: Vec<u64> = model
        .split('-')
        .filter_map(|s| s.parse().ok())
        .take(2)
        .collect();
    match numbers.as_slice() {
        [major, minor, ..] if *minor < 100 => (*major, *minor) >= (4, 6),
        [major, ..] => *major > 4,
        [] => true,
    }
}

/// With thinking on, Claude wants the assistant turn a tool result answers to
/// open with its signed thinking, and takes no forced tool choice.
fn thinks(c: &Conversation, turns: &[Value]) -> bool {
    let forced = matches!(
        c.tool_choice,
        Some(ToolChoice::Required | ToolChoice::Named(_))
    );
    let unsigned = turns
        .iter()
        .rev()
        .find(|t| t["role"] == "assistant")
        .and_then(|t| t["content"].as_array())
        .is_some_and(|blocks| {
            blocks.iter().any(|b| b["type"] == "tool_use") && blocks[0]["type"] != "thinking"
        });
    !forced && !unsigned
}

fn messages(c: &Conversation) -> Value {
    let mut turns: Vec<Value> = Vec::new();
    for Turn { role, parts } in &c.turns {
        let mut blocks: Vec<Value> = parts.iter().filter_map(anthropic_block).collect();
        if *role == Role::User {
            blocks.sort_by_key(|b| b["type"] != "tool_result");
        }
        if blocks.is_empty() {
            continue;
        }
        let role = if *role == Role::User {
            "user"
        } else {
            "assistant"
        };
        match turns.last_mut() {
            Some(last) if last["role"] == role => {
                if let Some(a) = last["content"].as_array_mut() {
                    a.extend(blocks);
                }
            }
            _ => turns.push(json!({"role": role, "content": blocks})),
        }
    }
    let mut out = Map::new();
    out.insert("model".into(), json!(c.model));
    let mut max_tokens = c.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    if let Some(effort) = c.reasoning.filter(|_| thinks(c, &turns)) {
        if adaptive(&c.model) {
            out.insert("thinking".into(), json!({"type": "adaptive"}));
            out.insert("output_config".into(), json!({"effort": effort.name()}));
        } else {
            let budget = effort.budget();
            max_tokens = max_tokens.max(budget + 4096);
            out.insert(
                "thinking".into(),
                json!({"type": "enabled", "budget_tokens": budget}),
            );
        }
    } else {
        if let Some(t) = c.temperature {
            out.insert("temperature".into(), json!(t));
        }
        if let Some(p) = c.top_p {
            out.insert("top_p".into(), json!(p));
        }
    }
    out.insert("max_tokens".into(), json!(max_tokens));
    if !c.system.is_empty() {
        let system: Vec<Value> = c
            .system
            .iter()
            .map(|s| json!({"type": "text", "text": s}))
            .collect();
        out.insert("system".into(), json!(system));
    }
    out.insert("messages".into(), json!(turns));
    if !c.tools.is_empty() {
        let tools: Vec<Value> = c
            .tools
            .iter()
            .map(|t| {
                let mut v = json!({"name": t.name, "input_schema": t.schema});
                if let Some(d) = &t.description {
                    v["description"] = json!(d);
                }
                v
            })
            .collect();
        out.insert("tools".into(), json!(tools));
        let mut choice = match &c.tool_choice {
            Some(ToolChoice::Required) => json!({"type": "any"}),
            Some(ToolChoice::None) => json!({"type": "none"}),
            Some(ToolChoice::Named(n)) => json!({"type": "tool", "name": n}),
            Some(ToolChoice::Auto) | None => json!({"type": "auto"}),
        };
        if c.parallel_tools == Some(false) && choice["type"] != "none" {
            choice["disable_parallel_tool_use"] = json!(true);
        }
        if c.tool_choice.is_some() || c.parallel_tools == Some(false) {
            out.insert("tool_choice".into(), choice);
        }
    }
    if !c.stop.is_empty() {
        out.insert("stop_sequences".into(), json!(c.stop));
    }
    out.insert("stream".into(), json!(true));
    Value::Object(out)
}

fn openai_image(image: &Image) -> String {
    match image {
        Image::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        Image::Url(url) => url.clone(),
    }
}

fn responses(c: &Conversation, names: &mut Names, limit: Option<usize>) -> Value {
    let mut input: Vec<Value> = Vec::new();
    let mut reasoned = false;
    for Turn { role, parts } in &c.turns {
        let mut content: Vec<Value> = Vec::new();
        let flush = |content: &mut Vec<Value>, input: &mut Vec<Value>| {
            if content.is_empty() {
                return;
            }
            let role = if *role == Role::User {
                "user"
            } else {
                "assistant"
            };
            input
                .push(json!({"type": "message", "role": role, "content": std::mem::take(content)}));
        };
        for p in parts {
            match p {
                Part::Text(t) if !t.is_empty() => {
                    let kind = if *role == Role::User {
                        "input_text"
                    } else {
                        "output_text"
                    };
                    content.push(json!({"type": kind, "text": t}));
                }
                Part::Image(i) if *role == Role::User => {
                    content.push(json!({"type": "input_image", "image_url": openai_image(i)}));
                }
                Part::ToolCall {
                    id,
                    name,
                    arguments,
                } => {
                    flush(&mut content, &mut input);
                    input.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": names.shorten(name, limit),
                        "arguments": arguments,
                    }));
                }
                Part::ToolResult { id, content: c, .. } => {
                    flush(&mut content, &mut input);
                    input.push(json!({"type": "function_call_output", "call_id": id, "output": text_of(c)}));
                }
                Part::Thinking {
                    text,
                    signature: Some(sig),
                } if sig.starts_with(SIGNATURE) => {
                    flush(&mut content, &mut input);
                    reasoned = true;
                    let summary: Vec<Value> = if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![json!({"type": "summary_text", "text": text})]
                    };
                    input.push(json!({
                        "type": "reasoning",
                        "summary": summary,
                        "encrypted_content": &sig[SIGNATURE.len()..],
                    }));
                }
                _ => {}
            }
        }
        flush(&mut content, &mut input);
    }
    let mut out = Map::new();
    out.insert("model".into(), json!(c.model));
    out.insert("instructions".into(), json!(c.system.join("\n\n")));
    out.insert("input".into(), json!(input));
    if !c.tools.is_empty() {
        let tools: Vec<Value> = c
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": names.shorten(&t.name, limit),
                    "description": t.description.clone().unwrap_or_default(),
                    "parameters": t.schema,
                    "strict": false,
                })
            })
            .collect();
        out.insert("tools".into(), json!(tools));
        let choice = match &c.tool_choice {
            Some(ToolChoice::Required) => json!("required"),
            Some(ToolChoice::None) => json!("none"),
            Some(ToolChoice::Named(n)) => {
                json!({"type": "function", "name": names.shorten(n, limit)})
            }
            Some(ToolChoice::Auto) | None => json!("auto"),
        };
        out.insert("tool_choice".into(), choice);
        if let Some(p) = c.parallel_tools {
            out.insert("parallel_tool_calls".into(), json!(p));
        }
    }
    if let Some(m) = c.max_tokens {
        out.insert("max_output_tokens".into(), json!(m));
    }
    if let Some(t) = c.temperature {
        out.insert("temperature".into(), json!(t));
    }
    if let Some(p) = c.top_p {
        out.insert("top_p".into(), json!(p));
    }
    if let Some(effort) = c.reasoning {
        out.insert(
            "reasoning".into(),
            json!({"effort": effort.name(), "summary": "auto"}),
        );
    }
    if c.reasoning.is_some() || reasoned {
        out.insert("include".into(), json!(["reasoning.encrypted_content"]));
    }
    out.insert("stream".into(), json!(true));
    out.insert("store".into(), json!(false));
    Value::Object(out)
}

fn chat(c: &Conversation) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if !c.system.is_empty() {
        messages.push(json!({"role": "system", "content": c.system.join("\n\n")}));
    }
    for Turn { role, parts } in &c.turns {
        if *role == Role::User {
            let mut content: Vec<Value> = Vec::new();
            for p in parts {
                match p {
                    Part::ToolResult { id, content: c, .. } => messages
                        .push(json!({"role": "tool", "tool_call_id": id, "content": text_of(c)})),
                    Part::Text(t) if !t.is_empty() => {
                        content.push(json!({"type": "text", "text": t}))
                    }
                    Part::Image(i) => content
                        .push(json!({"type": "image_url", "image_url": {"url": openai_image(i)}})),
                    _ => {}
                }
            }
            if !content.is_empty() {
                messages.push(json!({"role": "user", "content": content}));
            }
            continue;
        }
        let text = text_of(parts);
        let calls: Vec<Value> = parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall {
                    id,
                    name,
                    arguments,
                } => Some(json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments}})),
                _ => None,
            })
            .collect();
        if text.is_empty() && calls.is_empty() {
            continue;
        }
        let mut m = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
        if !calls.is_empty() {
            m["tool_calls"] = json!(calls);
        }
        messages.push(m);
    }
    let mut out = Map::new();
    out.insert("model".into(), json!(c.model));
    out.insert("messages".into(), json!(messages));
    if !c.tools.is_empty() {
        let tools: Vec<Value> = c
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name,
                    "description": t.description.clone().unwrap_or_default(),
                    "parameters": t.schema,
                }})
            })
            .collect();
        out.insert("tools".into(), json!(tools));
        let choice = match &c.tool_choice {
            Some(ToolChoice::Required) => json!("required"),
            Some(ToolChoice::None) => json!("none"),
            Some(ToolChoice::Named(n)) => json!({"type": "function", "function": {"name": n}}),
            Some(ToolChoice::Auto) | None => json!("auto"),
        };
        out.insert("tool_choice".into(), choice);
        if let Some(p) = c.parallel_tools {
            out.insert("parallel_tool_calls".into(), json!(p));
        }
    }
    if let Some(m) = c.max_tokens {
        out.insert("max_tokens".into(), json!(m));
    }
    if let Some(t) = c.temperature {
        out.insert("temperature".into(), json!(t));
    }
    if let Some(p) = c.top_p {
        out.insert("top_p".into(), json!(p));
    }
    if !c.stop.is_empty() {
        out.insert("stop".into(), json!(c.stop));
    }
    if let Some(effort) = c.reasoning {
        out.insert("reasoning_effort".into(), json!(effort.name()));
    }
    out.insert("stream".into(), json!(true));
    out.insert("stream_options".into(), json!({"include_usage": true}));
    Value::Object(out)
}
