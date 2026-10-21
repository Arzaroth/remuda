use anyhow::{Result, bail};
use serde_json::{Map, Value};

use super::emit::{CLAUDE_SIGNATURE, SIGNATURE};
use super::{Conversation, Effort, Image, Part, Role, Tool, ToolChoice, Turn, text_of};
use crate::provider::Format;

pub fn parse(format: Format, body: &Value) -> Result<Conversation> {
    let Some(body) = body.as_object() else {
        bail!("the request body is not an object");
    };
    let mut c = Conversation {
        model: str_of(body, "model").unwrap_or_default().to_owned(),
        stream: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
        temperature: body.get("temperature").and_then(Value::as_f64),
        top_p: body.get("top_p").and_then(Value::as_f64),
        parallel_tools: body.get("parallel_tool_calls").and_then(Value::as_bool),
        ..Conversation::default()
    };
    match format {
        Format::Chat => chat(body, &mut c)?,
        Format::Responses => responses(body, &mut c)?,
        Format::Messages => messages(body, &mut c)?,
    }
    Ok(c)
}

fn str_of<'a>(o: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    o.get(key).and_then(Value::as_str)
}

fn stops(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Appends to the last turn when it has the same role, as the Anthropic
/// format requires and the OpenAI ones allow.
fn push(c: &mut Conversation, role: Role, parts: Vec<Part>) {
    if parts.is_empty() {
        return;
    }
    match c.turns.last_mut() {
        Some(last) if last.role == role => last.parts.extend(parts),
        _ => c.turns.push(Turn { role, parts }),
    }
}

fn image_url(url: &str) -> Image {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((media_type, data)) = rest.split_once(";base64,")
    {
        return Image::Base64 {
            media_type: media_type.to_owned(),
            data: data.to_owned(),
        };
    }
    Image::Url(url.to_owned())
}

/// Text and images in any of the formats' content shapes.
fn content(v: Option<&Value>) -> Vec<Part> {
    match v {
        Some(Value::String(s)) if !s.is_empty() => vec![Part::Text(s.clone())],
        Some(Value::Array(items)) => items.iter().filter_map(content_part).collect(),
        _ => Vec::new(),
    }
}

fn content_part(item: &Value) -> Option<Part> {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("text");
    match kind {
        "text" | "input_text" | "output_text" => Some(Part::Text(
            item.get("text").and_then(Value::as_str)?.to_owned(),
        )),
        "image_url" => {
            let url = item
                .pointer("/image_url/url")
                .or_else(|| item.get("image_url"))
                .and_then(Value::as_str)?;
            Some(Part::Image(image_url(url)))
        }
        "input_image" => Some(Part::Image(image_url(
            item.get("image_url").and_then(Value::as_str)?,
        ))),
        "image" => {
            let source = item.get("source")?;
            match source.get("type").and_then(Value::as_str) {
                Some("base64") => Some(Part::Image(Image::Base64 {
                    media_type: source.get("media_type")?.as_str()?.to_owned(),
                    data: source.get("data")?.as_str()?.to_owned(),
                })),
                Some("url") => Some(Part::Image(Image::Url(
                    source.get("url")?.as_str()?.to_owned(),
                ))),
                _ => None,
            }
        }
        _ => None,
    }
}

fn tool_result_content(v: Option<&Value>) -> Vec<Part> {
    match v {
        Some(Value::String(s)) => vec![Part::Text(s.clone())],
        Some(Value::Array(_)) => content(v),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![Part::Text(other.to_string())],
    }
}

fn openai_tools(c: &mut Conversation, tools: Option<&Value>, nested: bool) {
    for t in tools.and_then(Value::as_array).into_iter().flatten() {
        if t.get("type").and_then(Value::as_str).unwrap_or("function") != "function" {
            continue;
        }
        let f = if nested { t.get("function") } else { Some(t) };
        let Some(f) = f else { continue };
        let Some(name) = f.get("name").and_then(Value::as_str) else {
            continue;
        };
        c.tools.push(Tool {
            name: name.to_owned(),
            description: f
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            schema: f
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}})),
        });
    }
}

fn openai_tool_choice(v: Option<&Value>) -> Option<ToolChoice> {
    match v? {
        Value::String(s) => match s.as_str() {
            "auto" => Some(ToolChoice::Auto),
            "none" => Some(ToolChoice::None),
            "required" => Some(ToolChoice::Required),
            _ => None,
        },
        Value::Object(o) => o
            .get("name")
            .or_else(|| o.get("function").and_then(|f| f.get("name")))
            .and_then(Value::as_str)
            .map(|n| ToolChoice::Named(n.to_owned())),
        _ => None,
    }
}

fn chat(body: &Map<String, Value>, c: &mut Conversation) -> Result<()> {
    c.max_tokens = body
        .get("max_completion_tokens")
        .or_else(|| body.get("max_tokens"))
        .and_then(Value::as_u64);
    c.stop = stops(body.get("stop"));
    c.reasoning = str_of(body, "reasoning_effort").and_then(Effort::named);
    c.stream_usage = body
        .get("stream_options")
        .and_then(|o| o.get("include_usage"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    openai_tools(c, body.get("tools"), true);
    c.tool_choice = openai_tool_choice(body.get("tool_choice"));
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        bail!("messages is required");
    };
    for m in messages {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
        match role {
            "system" | "developer" => {
                let text = text_of(&content(m.get("content")));
                if !text.is_empty() {
                    c.system.push(text);
                }
            }
            "user" => push(c, Role::User, content(m.get("content"))),
            "assistant" => {
                let mut parts = Vec::new();
                if let Some(r) = m.get("reasoning_content").and_then(Value::as_str)
                    && !r.is_empty()
                {
                    parts.push(Part::Thinking {
                        text: r.to_owned(),
                        signature: None,
                    });
                }
                parts.extend(content(m.get("content")));
                for call in m
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let f = call.get("function");
                    parts.push(Part::ToolCall {
                        id: call
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        name: f
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        arguments: f
                            .and_then(|f| f.get("arguments"))
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_owned(),
                    });
                }
                push(c, Role::Assistant, parts);
            }
            "tool" | "function" => {
                let id = m
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                push(
                    c,
                    Role::User,
                    vec![Part::ToolResult {
                        id,
                        content: tool_result_content(m.get("content")),
                        is_error: false,
                    }],
                );
            }
            _ => {}
        }
    }
    Ok(())
}

fn responses(body: &Map<String, Value>, c: &mut Conversation) -> Result<()> {
    c.max_tokens = body.get("max_output_tokens").and_then(Value::as_u64);
    c.reasoning = body
        .get("reasoning")
        .and_then(|r| r.get("effort"))
        .and_then(Value::as_str)
        .and_then(Effort::named);
    if let Some(i) = str_of(body, "instructions")
        && !i.is_empty()
    {
        c.system.push(i.to_owned());
    }
    openai_tools(c, body.get("tools"), false);
    c.tool_choice = openai_tool_choice(body.get("tool_choice"));
    let items: Vec<Value> = match body.get("input") {
        Some(Value::String(s)) => vec![serde_json::json!({"role": "user", "content": s})],
        Some(Value::Array(a)) => a.clone(),
        _ => bail!("input is required"),
    };
    for item in &items {
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message");
        match kind {
            "message" => {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let parts = content(item.get("content"));
                match role {
                    "system" | "developer" => {
                        let text = text_of(&parts);
                        if !text.is_empty() {
                            c.system.push(text);
                        }
                    }
                    "assistant" => push(c, Role::Assistant, parts),
                    _ => push(c, Role::User, parts),
                }
            }
            "function_call" => push(
                c,
                Role::Assistant,
                vec![Part::ToolCall {
                    id: item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    arguments: item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_owned(),
                }],
            ),
            "function_call_output" => {
                let output = match item.get("output") {
                    Some(Value::String(s)) => vec![Part::Text(s.clone())],
                    other => tool_result_content(other),
                };
                push(
                    c,
                    Role::User,
                    vec![Part::ToolResult {
                        id: item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        content: output,
                        is_error: false,
                    }],
                );
            }
            "reasoning" => {
                let text = item
                    .get("summary")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                push(
                    c,
                    Role::Assistant,
                    vec![Part::Thinking {
                        text,
                        signature: item
                            .get("encrypted_content")
                            .and_then(Value::as_str)
                            .map(|e| {
                                if e.starts_with(CLAUDE_SIGNATURE) {
                                    e.to_owned()
                                } else {
                                    format!("{SIGNATURE}{e}")
                                }
                            }),
                    }],
                );
            }
            _ => {}
        }
    }
    Ok(())
}

fn messages(body: &Map<String, Value>, c: &mut Conversation) -> Result<()> {
    c.max_tokens = body.get("max_tokens").and_then(Value::as_u64);
    c.stop = stops(body.get("stop_sequences"));
    match body.get("system") {
        Some(Value::String(s)) if !s.is_empty() => c.system.push(s.clone()),
        Some(Value::Array(blocks)) => c.system.extend(
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
        ),
        _ => {}
    }
    let thinking = body.get("thinking");
    match thinking.and_then(|t| t.get("type")).and_then(Value::as_str) {
        Some("enabled") => {
            c.reasoning = Some(
                thinking
                    .and_then(|t| t.get("budget_tokens"))
                    .and_then(Value::as_u64)
                    .map_or(Effort::Medium, Effort::from_budget),
            )
        }
        Some("adaptive") => {
            c.reasoning = Some(
                body.get("output_config")
                    .and_then(|o| o.get("effort"))
                    .and_then(Value::as_str)
                    .and_then(Effort::named)
                    .unwrap_or(Effort::Medium),
            )
        }
        _ => {}
    }
    for t in body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = t.get("name").and_then(Value::as_str) else {
            continue;
        };
        if t.get("input_schema").is_none() {
            continue;
        }
        c.tools.push(Tool {
            name: name.to_owned(),
            description: t
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            schema: t["input_schema"].clone(),
        });
    }
    c.tool_choice =
        body.get("tool_choice")
            .and_then(|tc| match tc.get("type").and_then(Value::as_str)? {
                "auto" => Some(ToolChoice::Auto),
                "any" => Some(ToolChoice::Required),
                "none" => Some(ToolChoice::None),
                "tool" => Some(ToolChoice::Named(tc.get("name")?.as_str()?.to_owned())),
                _ => None,
            });
    let serial = body
        .get("tool_choice")
        .and_then(|tc| tc.get("disable_parallel_tool_use"))
        .and_then(Value::as_bool);
    if serial == Some(true) {
        c.parallel_tools = Some(false);
    }
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        bail!("messages is required");
    };
    for m in messages {
        let role = match m.get("role").and_then(Value::as_str) {
            Some("assistant") => Role::Assistant,
            _ => Role::User,
        };
        let parts = match m.get("content") {
            Some(Value::String(s)) => vec![Part::Text(s.clone())],
            Some(Value::Array(blocks)) => blocks.iter().filter_map(message_block).collect(),
            _ => Vec::new(),
        };
        push(c, role, parts);
    }
    Ok(())
}

fn message_block(b: &Value) -> Option<Part> {
    match b.get("type").and_then(Value::as_str)? {
        "tool_use" => Some(Part::ToolCall {
            id: b.get("id")?.as_str()?.to_owned(),
            name: b.get("name")?.as_str()?.to_owned(),
            arguments: b
                .get("input")
                .map(Value::to_string)
                .unwrap_or_else(|| "{}".to_owned()),
        }),
        "tool_result" => Some(Part::ToolResult {
            id: b.get("tool_use_id")?.as_str()?.to_owned(),
            content: tool_result_content(b.get("content")),
            is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        }),
        "thinking" => Some(Part::Thinking {
            text: b
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            signature: b
                .get("signature")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "redacted_thinking" => None,
        _ => content_part(b),
    }
}
