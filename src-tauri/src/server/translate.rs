//! Format conversion for the server: OpenAI Chat Completions ⇄ Anthropic
//! Messages (text conversations), and demo replies in each API's own format.

use serde_json::{json, Value};

use crate::llm::chat::claude_supports_fallbacks;
use crate::llm::throughput::demo_tokens;

/// Which API a client called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    ChatCompletions,
    Responses,
    Messages,
    /// Legacy text completions and embeddings: passed straight through to OpenAI-style engines.
    Completions,
    Embeddings,
}

impl Api {
    pub fn label(self) -> &'static str {
        match self {
            Api::ChatCompletions => "chat/completions",
            Api::Responses => "responses",
            Api::Messages => "messages",
            Api::Completions => "completions",
            Api::Embeddings => "embeddings",
        }
    }
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str().or_else(|| p.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The newest user text in a request of any of the three APIs.
pub fn last_user_text(api: Api, body: &Value) -> String {
    let items = match api {
        Api::Responses if body["input"].is_string() => return body["input"].as_str().unwrap_or("").into(),
        Api::Responses => &body["input"],
        Api::Completions | Api::Embeddings => return text_of(&body["prompt"]) + &text_of(&body["input"]),
        _ => &body["messages"],
    };
    items
        .as_array()
        .and_then(|a| a.iter().rev().find(|m| m["role"] == "user"))
        .map(|m| text_of(&m["content"]))
        .unwrap_or_default()
}

/// Chat Completions request → Anthropic Messages request. Returns the body and
/// whether the server-side fallback beta header must be sent.
pub fn chat_to_anthropic(body: &Value, model: &str) -> Result<(Value, bool), String> {
    if body["tools"].as_array().is_some_and(|t| !t.is_empty()) {
        return Err("Tool calls to Claude through the Chat Completions API aren't supported yet; use the Anthropic Messages API (/v1/messages) instead.".into());
    }
    let mut system = Vec::new();
    let mut messages = Vec::new();
    for m in body["messages"].as_array().into_iter().flatten() {
        let text = text_of(&m["content"]);
        match m["role"].as_str() {
            Some("system") | Some("developer") => system.push(text),
            Some(role @ ("user" | "assistant")) if !text.is_empty() => messages.push(json!({ "role": role, "content": text })),
            _ => {}
        }
    }
    let stream = body["stream"].as_bool().unwrap_or(false);
    let max_tokens = body["max_completion_tokens"].as_u64().or(body["max_tokens"].as_u64()).unwrap_or(if stream { 64000 } else { 16000 });
    // temperature / top_p are dropped on purpose: Claude's current API rejects sampling parameters.
    let mut out = json!({ "model": model, "max_tokens": max_tokens, "messages": messages, "stream": stream });
    if !system.is_empty() {
        out["system"] = json!(system.join("\n\n"));
    }
    match &body["stop"] {
        Value::String(s) => out["stop_sequences"] = json!([s]),
        Value::Array(a) if !a.is_empty() => out["stop_sequences"] = json!(a),
        _ => {}
    }
    let fallback = claude_supports_fallbacks(model);
    if fallback {
        out["fallbacks"] = json!("default");
    }
    Ok((out, fallback))
}

fn finish_reason(stop: &str) -> &'static str {
    match stop {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        "refusal" => "content_filter",
        _ => "stop",
    }
}

/// A finished Anthropic message → a `chat.completion` object.
pub fn anthropic_to_chat(msg: &Value, model_label: &str, created: u64) -> Value {
    let text: String = msg["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let input = msg["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output = msg["usage"]["output_tokens"].as_u64().unwrap_or(0);
    json!({
        "id": format!("chatcmpl-{}", msg["id"].as_str().unwrap_or("sparkbox")),
        "object": "chat.completion",
        "created": created,
        "model": model_label,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": text },
            "finish_reason": finish_reason(msg["stop_reason"].as_str().unwrap_or("end_turn")),
        }],
        "usage": { "prompt_tokens": input, "completion_tokens": output, "total_tokens": input + output },
    })
}

/// Converts Anthropic stream events into `chat.completion.chunk` payloads.
pub struct ChunkWriter {
    id: String,
    model: String,
    created: u64,
    include_usage: bool,
    sent_role: bool,
    input_tokens: u64,
    output_tokens: u64,
}

impl ChunkWriter {
    pub fn new(model: &str, created: u64, include_usage: bool) -> Self {
        ChunkWriter {
            id: format!("chatcmpl-sparkbox-{created}"),
            model: model.into(),
            created,
            include_usage,
            sent_role: false,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> String {
        json!({
            "id": self.id, "object": "chat.completion.chunk", "created": self.created, "model": self.model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        })
        .to_string()
    }

    /// Text delta → chunk; also usable directly by the demo.
    pub fn text(&mut self, t: &str) -> Vec<String> {
        let mut out = Vec::new();
        if !self.sent_role {
            self.sent_role = true;
            out.push(self.chunk(json!({ "role": "assistant", "content": "" }), None));
        }
        out.push(self.chunk(json!({ "content": t }), None));
        out
    }

    pub fn on_anthropic_event(&mut self, ev: &Value) -> Vec<String> {
        match ev["type"].as_str() {
            Some("message_start") => {
                self.input_tokens = ev["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0);
                Vec::new()
            }
            Some("content_block_delta") if ev["delta"]["type"] == "text_delta" => self.text(ev["delta"]["text"].as_str().unwrap_or("")),
            Some("message_delta") => {
                self.output_tokens = ev["usage"]["output_tokens"].as_u64().unwrap_or(self.output_tokens);
                match ev["delta"]["stop_reason"].as_str() {
                    Some(r) => self.finish(finish_reason(r)),
                    None => Vec::new(),
                }
            }
            Some("error") => vec![json!({ "error": { "message": ev["error"]["message"], "type": ev["error"]["type"] } }).to_string()],
            _ => Vec::new(),
        }
    }

    pub fn finish(&mut self, reason: &str) -> Vec<String> {
        let mut out = vec![self.chunk(json!({}), Some(reason))];
        if self.include_usage {
            out.push(
                json!({
                    "id": self.id, "object": "chat.completion.chunk", "created": self.created, "model": self.model,
                    "choices": [],
                    "usage": { "prompt_tokens": self.input_tokens, "completion_tokens": self.output_tokens,
                               "total_tokens": self.input_tokens + self.output_tokens },
                })
                .to_string(),
            );
        }
        out
    }
}

/// The demo model's reply (`text`) in the shape each API expects (non-streaming). Tokens are
/// four characters each, as everywhere in Demo.
pub fn demo_response_with(api: Api, body: &Value, text: &str, model_label: &str, created: u64) -> Value {
    let prompt = demo_tokens(&last_user_text(api, body)).max(1);
    let words = demo_tokens(text);
    match api {
        Api::ChatCompletions => json!({
            "id": format!("chatcmpl-demo-{created}"), "object": "chat.completion", "created": created, "model": model_label,
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": prompt, "completion_tokens": words, "total_tokens": words + prompt },
        }),
        Api::Completions => json!({
            "id": format!("cmpl-demo-{created}"), "object": "text_completion", "created": created, "model": model_label,
            "choices": [{ "index": 0, "text": text, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": prompt, "completion_tokens": words, "total_tokens": words + prompt },
        }),
        Api::Embeddings => json!({ "error": { "message": "The demo model doesn't make embeddings.", "type": "invalid_request_error" } }),
        Api::Messages => json!({
            "id": format!("msg_demo_{created}"), "type": "message", "role": "assistant", "model": model_label,
            "content": [{ "type": "text", "text": text }], "stop_reason": "end_turn", "stop_sequence": null,
            "usage": { "input_tokens": prompt, "output_tokens": words },
        }),
        Api::Responses => json!({
            "id": format!("resp_demo_{created}"), "object": "response", "created_at": created, "status": "completed",
            "model": model_label,
            "output": [{ "type": "message", "id": format!("msg_demo_{created}"), "status": "completed", "role": "assistant",
                         "content": [{ "type": "output_text", "text": text, "annotations": [] }] }],
            "usage": { "input_tokens": prompt, "output_tokens": words, "total_tokens": words + prompt },
        }),
    }
}

/// A demo reply as a sequence of SSE frames, one word per frame (already formatted, "event:"
/// lines included where the API uses them).
pub fn demo_stream_with(api: Api, body: &Value, text: &str, model_label: &str, created: u64) -> Vec<String> {
    let text = text.to_string();
    let pieces: Vec<&str> = text.split_inclusive(' ').collect();
    let words = demo_tokens(&text);
    let prompt = demo_tokens(&last_user_text(api, body)).max(1);
    let named = |name: &str, v: Value| format!("event: {name}\ndata: {v}\n\n");
    let mut out = Vec::new();
    match api {
        Api::ChatCompletions => {
            let include_usage = body["stream_options"]["include_usage"].as_bool().unwrap_or(false);
            let mut w = ChunkWriter::new(model_label, created, include_usage);
            w.input_tokens = prompt;
            w.output_tokens = words;
            for p in &pieces {
                out.extend(w.text(p).into_iter().map(|c| format!("data: {c}\n\n")));
            }
            out.extend(w.finish("stop").into_iter().map(|c| format!("data: {c}\n\n")));
            out.push("data: [DONE]\n\n".into());
        }
        Api::Messages => {
            out.push(named("message_start", json!({ "type": "message_start", "message": {
                "id": format!("msg_demo_{created}"), "type": "message", "role": "assistant", "model": model_label,
                "content": [], "stop_reason": null, "usage": { "input_tokens": prompt, "output_tokens": 0 } } })));
            out.push(named("content_block_start", json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } })));
            for p in &pieces {
                out.push(named("content_block_delta", json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": p } })));
            }
            out.push(named("content_block_stop", json!({ "type": "content_block_stop", "index": 0 })));
            out.push(named("message_delta", json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn", "stop_sequence": null }, "usage": { "output_tokens": words } })));
            out.push(named("message_stop", json!({ "type": "message_stop" })));
        }
        Api::Completions | Api::Embeddings => {
            let full = demo_response_with(api, body, &text, model_label, created);
            out.push(format!("data: {full}\n\n"));
            out.push("data: [DONE]\n\n".into());
        }
        Api::Responses => {
            let full = demo_response_with(api, body, &text, model_label, created);
            let mut created_ev = full.clone();
            created_ev["status"] = json!("in_progress");
            created_ev["output"] = json!([]);
            out.push(named("response.created", json!({ "type": "response.created", "response": created_ev })));
            for p in &pieces {
                out.push(named("response.output_text.delta", json!({ "type": "response.output_text.delta", "output_index": 0, "content_index": 0, "delta": p })));
            }
            out.push(named("response.output_text.done", json!({ "type": "response.output_text.done", "output_index": 0, "content_index": 0, "text": text })));
            out.push(named("response.completed", json!({ "type": "response.completed", "response": full })));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_requests_become_claude_requests() {
        let body = json!({
            "model": "anthropic/claude-opus-5-5", "stream": true, "temperature": 0.2, "stop": "END",
            "messages": [
                { "role": "system", "content": "Be brief." },
                { "role": "user", "content": [{ "type": "text", "text": "Hi" }] },
                { "role": "assistant", "content": "Hello" },
                { "role": "user", "content": "How are you?" }
            ]
        });
        let (out, fallback) = chat_to_anthropic(&body, "claude-opus-5-5").unwrap();
        assert!(fallback);
        assert_eq!(out["system"], "Be brief.");
        assert_eq!(out["messages"].as_array().unwrap().len(), 3);
        assert_eq!(out["messages"][0], json!({ "role": "user", "content": "Hi" }));
        assert_eq!(out["max_tokens"], 64000);
        assert_eq!(out["stop_sequences"], json!(["END"]));
        assert!(out.get("temperature").is_none(), "Claude rejects sampling parameters");
        assert_eq!(out["fallbacks"], "default");
    }

    #[test]
    fn tools_through_chat_completions_are_refused_clearly() {
        let body = json!({ "messages": [], "tools": [{ "type": "function" }] });
        assert!(chat_to_anthropic(&body, "claude-opus-5-5").unwrap_err().contains("/v1/messages"));
    }

    #[test]
    fn claude_stream_becomes_chunks() {
        let mut w = ChunkWriter::new("anthropic/claude-opus-5-5", 1, true);
        let mut out = Vec::new();
        for ev in [
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 7 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "hmm" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Hi" } }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 3 } }),
        ] {
            out.extend(w.on_anthropic_event(&ev));
        }
        let parsed: Vec<Value> = out.iter().map(|s| serde_json::from_str(s).unwrap()).collect();
        assert_eq!(parsed[0]["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(parsed[1]["choices"][0]["delta"]["content"], "Hi", "thinking is not leaked as text");
        assert_eq!(parsed[2]["choices"][0]["finish_reason"], "stop");
        assert_eq!(parsed[3]["usage"]["total_tokens"], 10);
    }

    #[test]
    fn last_user_text_reads_every_api() {
        assert_eq!(last_user_text(Api::Responses, &json!({ "input": "hello" })), "hello");
        let msgs = json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }] }] });
        assert_eq!(last_user_text(Api::Messages, &msgs), "ab");
    }
}
