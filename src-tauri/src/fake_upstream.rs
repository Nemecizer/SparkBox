//! Test-only stand-in for the Anthropic, OpenAI and cluster-engine APIs (a vLLM's
//! model list, /health, /metrics and completions included). It records every
//! request it receives and replies with canned (streamed or plain) responses in
//! each API's real wire format.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

#[derive(Clone, Default)]
pub struct Seen(Arc<Mutex<Vec<(String, HeaderMap, Value)>>>);

impl Seen {
    pub fn last(&self) -> (String, HeaderMap, Value) {
        self.0.lock().unwrap().last().cloned().expect("upstream got no request")
    }

    /// The first request to `path` ("completions", "models", "health"…), if any came.
    pub fn first(&self, path: &str) -> Option<(String, HeaderMap, Value)> {
        self.0.lock().unwrap().iter().find(|(p, ..)| p == path).cloned()
    }
}

/// Frames with an empty name are bare `data:` lines (OpenAI style); `done` appends `[DONE]`.
fn sse(frames: &[(&str, Value)], done: bool) -> Response {
    let mut body: String = frames
        .iter()
        .map(|(name, v)| if name.is_empty() { format!("data: {v}\n\n") } else { format!("event: {name}\ndata: {v}\n\n") })
        .collect();
    if done {
        body.push_str("data: [DONE]\n\n");
    }
    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// A reply that matches the model-feasibility schema (models::advisor).
pub fn analysis() -> Value {
    json!({
        "verdict": "good", "summary": "Fits on four nodes with room for long contexts.",
        "recommended": { "nodes": 4, "parallelism": "tensor", "tensorParallelSize": 4, "pipelineParallelSize": 1,
            "expertParallel": false, "maxModelLen": 131072, "gpuMemoryUtilization": 0.8, "extraVllmArgs": [],
            "why": "The weights need three nodes; tensor parallel needs a power of two." },
        "alternatives": [{ "nodes": 3, "parallelism": "pipeline", "tradeoff": "Slower per request." }],
        "expectedPerformance": "Roughly 20 tokens/s for one stream.",
        "engineSupport": { "status": "likely", "notes": "Recent vLLM supports this architecture." },
        "risks": ["Long first load."], "betterVariant": { "repo": "", "reason": "" }
    })
}

fn anthropic(body: &Value) -> Response {
    if body["output_config"]["format"]["type"] == "json_schema" {
        return axum::Json(json!({
            "id": "msg_2", "type": "message", "role": "assistant", "model": body["model"],
            "content": [{ "type": "thinking", "thinking": "", "signature": "s" }, { "type": "text", "text": analysis().to_string() }],
            "stop_reason": "end_turn", "usage": { "input_tokens": 900, "output_tokens": 300 },
        }))
        .into_response();
    }
    if !body["stream"].as_bool().unwrap_or(false) {
        return axum::Json(json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
            "content": [{ "type": "text", "text": "Hello there" }], "stop_reason": "end_turn",
            "usage": { "input_tokens": 12, "output_tokens": 5 },
        }))
        .into_response();
    }
    sse(&[
        ("message_start", json!({ "type": "message_start", "message": { "id": "msg_1", "model": "claude-opus-5-5", "usage": { "input_tokens": 12 } } })),
        ("content_block_start", json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } })),
        ("content_block_delta", json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Plan." } })),
        ("content_block_delta", json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig123" } })),
        ("content_block_stop", json!({ "type": "content_block_stop", "index": 0 })),
        ("content_block_start", json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } })),
        ("content_block_delta", json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Hello" } })),
        ("content_block_delta", json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": " there" } })),
        ("content_block_stop", json!({ "type": "content_block_stop", "index": 1 })),
        ("message_delta", json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 5 } })),
        ("message_stop", json!({ "type": "message_stop" })),
    ], false)
}

fn chat_completions(body: &Value) -> Response {
    let model = body["model"].clone();
    if !body["stream"].as_bool().unwrap_or(false) {
        return axum::Json(json!({
            "id": "cmpl-1", "object": "chat.completion", "model": model,
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "Local answer" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 2, "completion_tokens": 2, "total_tokens": 4 },
        }))
        .into_response();
    }
    sse(&[
        ("", json!({ "model": model, "choices": [{ "delta": { "role": "assistant", "reasoning_content": "think" } }] })),
        ("", json!({ "model": model, "choices": [{ "delta": { "content": "Local" } }] })),
        ("", json!({ "model": model, "choices": [{ "delta": { "content": " answer" }, "finish_reason": "stop" }] })),
        ("", json!({ "model": model, "choices": [], "usage": { "prompt_tokens": 2, "completion_tokens": 2 } })),
    ], true)
}

fn responses(body: &Value) -> Response {
    if body["text"]["format"]["type"] == "json_schema" {
        return axum::Json(json!({
            "id": "resp_1", "object": "response", "status": "completed", "model": body["model"],
            "output": [
                { "type": "reasoning", "summary": [] },
                { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": analysis().to_string() }] }
            ],
        }))
        .into_response();
    }
    sse(&[
        ("response.created", json!({ "type": "response.created", "response": { "status": "in_progress" } })),
        ("response.output_text.delta", json!({ "type": "response.output_text.delta", "delta": "Hi" })),
        ("response.output_text.delta", json!({ "type": "response.output_text.delta", "delta": " from OpenAI" })),
        ("response.completed", json!({ "type": "response.completed", "response": {
            "model": "gpt-test", "status": "completed", "usage": { "input_tokens": 3, "output_tokens": 4 } } })),
    ], false)
}

/// Time to first token and between tokens of the streamed completions (for benchmark tests).
pub const TTFT_MS: u64 = 40;
pub const ITL_MS: u64 = 4;

fn completions(body: &Value) -> Response {
    if body["stream"].as_bool().unwrap_or(false) {
        // One token per chunk at a steady pace, then usage, like vLLM with ignore_eos.
        let n = body["max_tokens"].as_u64().unwrap_or(8);
        let prompt = body["prompt"].as_array().map(|a| a.len() as u64).unwrap_or(1);
        let frames = futures_util::stream::unfold(0u64, move |i| async move {
            if i > n + 1 {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(if i == 0 { TTFT_MS } else { ITL_MS })).await;
            let frame = if i < n {
                format!("data: {}\n\n", json!({ "choices": [{ "index": 0, "text": " tok", "finish_reason": null }] }))
            } else if i == n {
                format!("data: {}\n\n", json!({ "choices": [], "usage": { "prompt_tokens": prompt, "completion_tokens": n } }))
            } else {
                "data: [DONE]\n\n".to_string()
            };
            Some((Ok::<_, std::convert::Infallible>(frame), i + 1))
        });
        return ([(header::CONTENT_TYPE, "text/event-stream")], axum::body::Body::from_stream(frames)).into_response();
    }
    axum::Json(json!({
        "id": "cmpl-2", "object": "text_completion", "model": body["model"],
        "choices": [{ "index": 0, "text": " world", "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 },
    }))
    .into_response()
}

fn embeddings(body: &Value) -> Response {
    axum::Json(json!({
        "object": "list", "model": body["model"],
        "data": [{ "object": "embedding", "index": 0, "embedding": [0.25, -0.5, 0.125] }],
        "usage": { "prompt_tokens": 2, "total_tokens": 2 },
    }))
    .into_response()
}

/// What a vLLM server answers to GETs: its model list, /health (an empty 200) and /metrics.
fn engine_get(path: &'static str, seen: Seen) -> impl Fn(HeaderMap) -> std::future::Ready<Response> + Clone {
    move |headers: HeaderMap| {
        seen.0.lock().unwrap().push((path.to_string(), headers, Value::Null));
        std::future::ready(match path {
            "models" => axum::Json(json!({ "object": "list", "data": [{ "id": "llama-3.3-70b", "object": "model", "max_model_len": 32768 }] })).into_response(),
            "metrics" => include_str!("models/fixtures/vllm_metrics.txt").into_response(),
            _ => ().into_response(),
        })
    }
}

/// Starts the fake on a random port. Returns its base URL (no trailing slash).
pub async fn start() -> (String, Seen) {
    let seen = Seen::default();
    let record = |path: &'static str, seen: Seen| {
        move |headers: HeaderMap, body: Bytes| {
            let seen = seen.clone();
            async move {
                let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                seen.0.lock().unwrap().push((path.to_string(), headers, v.clone()));
                match path {
                    "messages" => anthropic(&v),
                    "chat/completions" => chat_completions(&v),
                    "completions" => completions(&v),
                    "embeddings" => embeddings(&v),
                    _ => responses(&v),
                }
            }
        }
    };
    let app = Router::new()
        .route("/v1/messages", post(record("messages", seen.clone())))
        .route("/v1/chat/completions", post(record("chat/completions", seen.clone())))
        .route("/v1/responses", post(record("responses", seen.clone())))
        .route("/v1/completions", post(record("completions", seen.clone())))
        .route("/v1/embeddings", post(record("embeddings", seen.clone())))
        .route("/v1/models", get(engine_get("models", seen.clone())))
        .route("/health", get(engine_get("health", seen.clone())))
        .route("/metrics", get(engine_get("metrics", seen.clone())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}
