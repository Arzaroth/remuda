use serde_json::{Map, Value, json};

use super::{Conversation, Image, Names, Part, Role, ToolChoice, Turn};
use crate::provider::Format;

/// A signature remuda minted from an OpenAI reasoning item, so it goes back
/// only to the format it came from.
pub const SIGNATURE: &str = "remuda-reasoning:";

const DEFAULT_MAX_TOKENS: u64 = 32_000;

pub fn emit(to: Format, c: &Conversation, names: &mut Names, limit: Option<usize>) -> Value {
    match to {
        Format::Messages => messages(c),
        Format::Responses => responses(c, names, limit),
        Format::Chat => unreachable!("every provider that takes chat takes it from every client"),
    }
}

fn arguments(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => v,
        _ => json!({}),
    }
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
        // A signature from another provider is refused, and one is needed.
        Part::Thinking { .. } | Part::Text(_) => None,
    }
}

fn messages(c: &Conversation) -> Value {
    let mut turns: Vec<Value> = Vec::new();
    for Turn { role, parts } in &c.turns {
        let mut blocks: Vec<Value> = parts.iter().filter_map(anthropic_block).collect();
        if *role == Role::User {
            // Tool results lead their turn.
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
    if let Some(effort) = c.reasoning {
        let budget = effort.budget();
        max_tokens = max_tokens.max(budget + 4096);
        out.insert(
            "thinking".into(),
            json!({"type": "enabled", "budget_tokens": budget}),
        );
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
