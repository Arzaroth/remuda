use serde_json::{Value, json};

use super::*;

fn events(raw: &str) -> Vec<Value> {
    sse::Reader::new(raw.as_bytes())
        .filter(|e| e.data != "[DONE]")
        .map(|e| serde_json::from_str(&e.data).unwrap())
        .collect()
}

fn run(from: Format, to: Format, stream: bool, upstream: &str) -> (u16, String) {
    let c = Conversation {
        model: "client-model".into(),
        stream,
        stream_usage: true,
        ..Conversation::default()
    };
    let mut out = Vec::new();
    let mut status = 0;
    respond(
        from,
        to,
        &c,
        &Names::default(),
        upstream.as_bytes(),
        &mut out,
        |_, s, _, _| {
            status = s;
            Ok(())
        },
    )
    .unwrap();
    (status, String::from_utf8(out).unwrap())
}

const CLAUDE_STREAM: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"claude-x\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":5,\"output_tokens\":1}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\",\"input\":{}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a\\\"}\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":2}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

const CODEX_STREAM: &str = "event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"model\":\"gpt-5\"}}\n\n\
data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"reasoning\"}}\n\n\
data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"plan\"}\n\n\
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"encrypted_content\":\"ENC\"}}\n\n\
data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\"}}\n\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n\
data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read\"}}\n\n\
data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\\\"path\\\":\\\"a\\\"}\"}\n\n\
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"arguments\":\"{\\\"path\\\":\\\"a\\\"}\"}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":20,\"input_tokens_details\":{\"cached_tokens\":4},\"output_tokens\":9,\"output_tokens_details\":{\"reasoning_tokens\":3}}}}\n\n";

#[test]
fn a_chat_request_becomes_an_anthropic_one() {
    let body = json!({
        "model": "claude-x",
        "stream": false,
        "max_tokens": 100,
        "temperature": 0.5,
        "stop": "END",
        "messages": [
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": [
                {"type": "text", "text": "look"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}},
            ]},
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}},
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": "contents"},
            {"role": "user", "content": "and?"},
        ],
        "tools": [{"type": "function", "function": {"name": "read", "description": "Read", "parameters": {"type": "object"}}}],
        "tool_choice": "required",
    });
    let (out, _, c) = request(Format::Chat, Format::Messages, &body).unwrap();
    assert!(!c.stream);
    assert_eq!(out["stream"], true);
    assert_eq!(out["max_tokens"], 100);
    assert_eq!(out["temperature"], 0.5);
    assert_eq!(out["stop_sequences"], json!(["END"]));
    assert_eq!(out["system"], json!([{"type": "text", "text": "be brief"}]));
    assert_eq!(out["tools"][0]["input_schema"], json!({"type": "object"}));
    assert_eq!(out["tool_choice"], json!({"type": "any"}));
    let m = out["messages"].as_array().unwrap();
    assert_eq!(m.len(), 3);
    assert_eq!(
        m[0]["content"][1]["source"],
        json!({"type": "base64", "media_type": "image/png", "data": "AAAA"})
    );
    assert_eq!(
        m[1]["content"][0],
        json!({"type": "tool_use", "id": "call_1", "name": "read", "input": {"path": "a"}})
    );
    assert_eq!(m[2]["content"][0]["type"], "tool_result");
    assert_eq!(m[2]["content"][0]["content"][0]["text"], "contents");
    assert_eq!(m[2]["content"][1], json!({"type": "text", "text": "and?"}));
}

#[test]
fn an_anthropic_request_becomes_a_responses_one() {
    let long = "mcp__a_very_long_server_name__with_a_tool_whose_name_runs_past_sixty_four";
    let body = json!({
        "model": "gpt-5",
        "max_tokens": 4000,
        "stream": true,
        "system": [{"type": "text", "text": "sys"}],
        "thinking": {"type": "enabled", "budget_tokens": 20000},
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "plan", "signature": "remuda-reasoning:ENC"},
                {"type": "thinking", "thinking": "theirs", "signature": "claude-sig"},
                {"type": "text", "text": "ok"},
                {"type": "tool_use", "id": "toolu_1", "name": long, "input": {"x": 1}},
            ]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "done"}]},
        ],
        "tools": [{"name": long, "description": "d", "input_schema": {"type": "object"}},
                  {"type": "web_search_20250305", "name": "web_search"}],
    });
    let (out, names, c) = request(Format::Messages, Format::Responses, &body).unwrap();
    assert!(c.stream);
    assert_eq!(out["instructions"], "sys");
    assert_eq!(out["store"], false);
    assert_eq!(
        out["reasoning"],
        json!({"effort": "high", "summary": "auto"})
    );
    assert_eq!(out["include"], json!(["reasoning.encrypted_content"]));
    let tools = out["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let short = tools[0]["name"].as_str().unwrap();
    assert!(short.len() <= 64);
    assert_eq!(names.original(short), long);
    let input = out["input"].as_array().unwrap();
    let kinds: Vec<&str> = input.iter().map(|i| i["type"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "message",
            "reasoning",
            "message",
            "function_call",
            "function_call_output"
        ]
    );
    assert_eq!(input[1]["encrypted_content"], "ENC");
    assert_eq!(
        input[2]["content"][0],
        json!({"type": "output_text", "text": "ok"})
    );
    assert_eq!(input[3]["name"], short);
    assert_eq!(input[3]["arguments"], "{\"x\":1}");
    assert_eq!(
        input[4],
        json!({"type": "function_call_output", "call_id": "toolu_1", "output": "done"})
    );
}

#[test]
fn a_responses_request_becomes_an_anthropic_one() {
    let body = json!({
        "model": "claude-x",
        "instructions": "be kind",
        "input": [
            {"role": "developer", "content": "more rules"},
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": "42"},
        ],
        "reasoning": {"effort": "low"},
        "max_output_tokens": 100,
    });
    let (out, _, _) = request(Format::Responses, Format::Messages, &body).unwrap();
    assert_eq!(
        out["system"],
        json!([{"type": "text", "text": "be kind"}, {"type": "text", "text": "more rules"}])
    );
    assert_eq!(
        out["thinking"],
        json!({"type": "enabled", "budget_tokens": 2048})
    );
    assert!(out["max_tokens"].as_u64().unwrap() > 2048);
    let m = out["messages"].as_array().unwrap();
    assert_eq!(m[1]["content"][0]["type"], "tool_use");
    assert_eq!(
        m[2]["content"][0],
        json!({"type": "tool_result", "tool_use_id": "c1", "content": [{"type": "text", "text": "42"}]})
    );
    let text_input = json!({"model": "claude-x", "input": "just text"});
    let (out, _, _) = request(Format::Responses, Format::Messages, &text_input).unwrap();
    assert_eq!(
        out["messages"],
        json!([{"role": "user", "content": [{"type": "text", "text": "just text"}]}])
    );
}

#[test]
fn a_chat_request_becomes_a_responses_one() {
    let body = json!({
        "model": "gpt-5",
        "reasoning_effort": "minimal",
        "messages": [
            {"role": "developer", "content": "rules"},
            {"role": "user", "content": "hi"},
        ],
        "tool_choice": {"type": "function", "function": {"name": "f"}},
        "tools": [{"type": "function", "function": {"name": "f", "parameters": {"type": "object"}}}],
    });
    let (out, _, _) = request(Format::Chat, Format::Responses, &body).unwrap();
    assert_eq!(out["instructions"], "rules");
    assert_eq!(out["reasoning"]["effort"], "low");
    assert_eq!(out["tool_choice"], json!({"type": "function", "name": "f"}));
    assert_eq!(
        out["input"],
        json!([{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}])
    );
}

#[test]
fn a_claude_stream_reaches_a_chat_client_as_chunks() {
    let (status, out) = run(Format::Messages, Format::Chat, true, CLAUDE_STREAM);
    assert_eq!(status, 200);
    assert!(out.trim_end().ends_with("data: [DONE]"));
    let chunks = events(&out);
    let deltas: Vec<&Value> = chunks
        .iter()
        .filter_map(|c| c.pointer("/choices/0/delta"))
        .collect();
    assert_eq!(deltas[0]["role"], "assistant");
    assert!(deltas.iter().any(|d| d["reasoning_content"] == "hmm"));
    let text: String = deltas
        .iter()
        .filter_map(|d| d["content"].as_str())
        .collect();
    assert_eq!(text, "Hello");
    let calls: Vec<&Value> = deltas.iter().filter_map(|d| d.get("tool_calls")).collect();
    assert_eq!(calls[0][0]["id"], "toolu_1");
    assert_eq!(calls[0][0]["function"]["name"], "read");
    let args: String = calls[1..]
        .iter()
        .filter_map(|c| c[0]["function"]["arguments"].as_str())
        .collect();
    assert_eq!(args, "{\"path\":\"a\"}");
    let finish = chunks.iter().find_map(|c| {
        c.pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
    });
    assert_eq!(finish, Some("tool_calls"));
    let usage = &chunks.last().unwrap()["usage"];
    assert_eq!(usage["prompt_tokens"], 15);
    assert_eq!(usage["completion_tokens"], 7);
    assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 5);
    assert_eq!(chunks[0]["model"], "client-model");
}

#[test]
fn a_claude_answer_reaches_a_chat_client_whole() {
    let (status, out) = run(Format::Messages, Format::Chat, false, CLAUDE_STREAM);
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&out).unwrap();
    let m = &v["choices"][0]["message"];
    assert_eq!(m["content"], "Hello");
    assert_eq!(m["reasoning_content"], "hmm");
    assert_eq!(
        m["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"a\"}"
    );
    assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(v["object"], "chat.completion");
}

#[test]
fn a_codex_stream_reaches_an_anthropic_client_as_events() {
    let (_, out) = run(Format::Responses, Format::Messages, true, CODEX_STREAM);
    let names: Vec<String> = sse::Reader::new(out.as_bytes())
        .map(|e| e.event.unwrap())
        .collect();
    assert_eq!(names.first().map(String::as_str), Some("message_start"));
    assert_eq!(names.last().map(String::as_str), Some("message_stop"));
    let all = events(&out);
    let starts: Vec<&Value> = all
        .iter()
        .filter(|e| e["type"] == "content_block_start")
        .map(|e| &e["content_block"])
        .collect();
    assert_eq!(starts[0]["type"], "thinking");
    assert_eq!(starts[1]["type"], "text");
    assert_eq!(
        starts[2],
        &json!({"type": "tool_use", "id": "call_1", "name": "read", "input": {}})
    );
    let sig = all
        .iter()
        .find(|e| e.pointer("/delta/type") == Some(&json!("signature_delta")))
        .unwrap();
    assert_eq!(sig["delta"]["signature"], "remuda-reasoning:ENC");
    let args: Vec<&Value> = all
        .iter()
        .filter(|e| e.pointer("/delta/type") == Some(&json!("input_json_delta")))
        .collect();
    assert_eq!(
        args.len(),
        1,
        "arguments streamed in deltas are not sent twice"
    );
    let stops = all
        .iter()
        .filter(|e| e["type"] == "content_block_stop")
        .count();
    assert_eq!(stops, 3);
    let delta = all.iter().find(|e| e["type"] == "message_delta").unwrap();
    assert_eq!(delta["delta"]["stop_reason"], "tool_use");
    assert_eq!(delta["usage"]["input_tokens"], 16);
    assert_eq!(delta["usage"]["cache_read_input_tokens"], 4);
    assert_eq!(delta["usage"]["output_tokens"], 9);
}

#[test]
fn a_codex_answer_reaches_an_anthropic_client_whole() {
    let (status, out) = run(Format::Responses, Format::Messages, false, CODEX_STREAM);
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["type"], "message");
    assert_eq!(v["stop_reason"], "tool_use");
    assert_eq!(
        v["content"][0],
        json!({"type": "thinking", "thinking": "plan", "signature": "remuda-reasoning:ENC"})
    );
    assert_eq!(v["content"][1], json!({"type": "text", "text": "Hi"}));
    assert_eq!(v["content"][2]["input"], json!({"path": "a"}));
}

#[test]
fn a_claude_stream_reaches_a_responses_client_as_items() {
    let (_, out) = run(Format::Messages, Format::Responses, true, CLAUDE_STREAM);
    let all = events(&out);
    let kinds: Vec<&str> = all.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(kinds[0], "response.created");
    assert_eq!(*kinds.last().unwrap(), "response.completed");
    let seqs: Vec<u64> = all
        .iter()
        .map(|e| e["sequence_number"].as_u64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
    let done = all.last().unwrap();
    let output = done["response"]["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[1]["content"][0]["text"], "Hello");
    assert_eq!(output[2]["type"], "function_call");
    assert_eq!(output[2]["arguments"], "{\"path\":\"a\"}");
    assert_eq!(done["response"]["usage"]["input_tokens"], 15);
    assert!(kinds.contains(&"response.function_call_arguments.done"));
}

#[test]
fn a_tool_name_shortened_on_the_way_out_comes_back_whole() {
    let long = "x".repeat(80);
    let mut names = Names::default();
    let short = names.shorten(&long, Some(64));
    assert_eq!(short.len(), 64);
    assert_eq!(names.shorten("short", Some(64)), "short");
    let upstream = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"type": "response.output_item.added", "item": {"type": "function_call", "call_id": "c", "name": short}}),
        json!({"type": "response.completed", "response": {}}),
    );
    let c = Conversation::default();
    let mut out = Vec::new();
    respond(
        Format::Responses,
        Format::Messages,
        &c,
        &names,
        upstream.as_bytes(),
        &mut out,
        |_, _, _, _| Ok(()),
    )
    .unwrap();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["content"][0]["name"], long);
}

#[test]
fn a_stream_cut_short_or_failed_says_so() {
    let cut =
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\"}}\n\n";
    let (_, out) = run(Format::Messages, Format::Chat, true, cut);
    assert!(out.contains("ended before the answer did"), "{out}");
    let (status, out) = run(Format::Messages, Format::Chat, false, cut);
    assert_eq!(status, 502);
    assert!(out.contains("ended before"), "{out}");
    let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"boom\"}}}\n\n";
    let (_, out) = run(Format::Responses, Format::Messages, true, failed);
    let last = events(&out).pop().unwrap();
    assert_eq!(last["type"], "error");
    assert_eq!(last["error"]["message"], "boom");
}

#[test]
fn an_upstream_error_message_is_found() {
    assert_eq!(error_message(r#"{"error":{"message":"bad"}}"#), "bad");
    assert_eq!(error_message(r#"{"detail":"nope"}"#), "nope");
    assert_eq!(error_message("plain"), "plain");
}
