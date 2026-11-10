//! Streaming chat with any provider. Replies arrive as `ChatEvent`s and end with
//! one `Done` carrying the finished assistant message (with stats) to save, or one
//! `Error`. A reply that goes quiet part-way (`Llm::stream_idle`), or whose connection
//! closes before the provider's last event, ends with both: `Done` with what arrived, then
//! `Error` saying it stopped.
//!
//! A conversation's history may hold "system" turns: the system prompt, changed after the
//! first reply. Claude gets each as a mid-conversation system message where it was made, so
//! what it's sent only ever grows (the API binds Claude's reasoning to the conversation before
//! it); the other providers get the latest one as their system prompt.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::history::{ChatParams, Message, Stats};
use super::sse::{SseEvent, SseParser};
use super::throughput::{demo_tokens, Meter};
use super::{api_error, catalog, DemoSpeed, Llm, Provider, ProviderKind, ANTHROPIC_VERSION};

/// Beta header that enables `fallbacks: "default"` (Claude API only).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Beta header for `thinking.block_binding`, and the `input_transformations` it reports.
pub const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";
/// The stop reason of a reply that went quiet part-way.
pub const STALLED: &str = "stalled";
/// The stop reason of a reply whose connection closed before the provider's last event.
pub const INTERRUPTED: &str = "interrupted";

/// Claude models that take adaptive thinking and `output_config.effort`.
pub fn claude_supports_adaptive(model: &str) -> bool {
    ["claude-fable-5", "claude-mythos-5", "claude-opus-5", "claude-sonnet-5", "claude-haiku-5", "claude-opus-4-6",
        "claude-opus-4-7", "claude-opus-4-8", "claude-sonnet-4-6"]
        .iter()
        .any(|p| model.starts_with(p))
}

/// Claude models where a declined request can be retried server-side on another model.
pub fn claude_supports_fallbacks(model: &str) -> bool {
    ["claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"].contains(&model)
}

/// Claude models that take a `{role: "system"}` message part-way through a conversation (not
/// Claude Sonnet 5, nor the models before Claude Opus 4.8).
pub fn claude_supports_mid_system(model: &str) -> bool {
    ["claude-opus-5", "claude-opus-4-8", "claude-fable-5", "claude-mythos-5", "claude-sonnet-5-5", "claude-haiku-5-5"]
        .iter()
        .any(|p| model.starts_with(p))
}

/// How a system prompt changed part-way is put to the model: it replaces the earlier one.
pub fn system_change_text(prompt: &str) -> String {
    match prompt.trim() {
        "" => "The earlier system prompt no longer applies.".into(),
        p => format!("From here on, follow this system prompt instead of the earlier one:\n\n{p}"),
    }
}

/// The system prompt in effect at the end of a conversation: its latest change, else `top`.
pub fn current_system<'a>(top: &'a str, messages: &'a [Message]) -> &'a str {
    messages.iter().rev().find(|m| m.role == "system").map(|m| m.content.as_str()).unwrap_or(top)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub request_id: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub params: ChatParams,
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatEvent {
    Text { text: String },
    Thinking { text: String },
    /// Something worth showing inline, e.g. a fallback to another model.
    Notice { text: String },
    Done { message: Message },
    Error { message: String },
}

/// The authoritative end of a UI request. Tauri's invoke response can arrive before
/// the last Channel messages, so the reply and its warning must travel together.
#[derive(Debug, Default, Serialize)]
pub struct ChatCompletion {
    pub message: Option<Message>,
    pub error: Option<String>,
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Collects a reply while it streams, then turns it into a saved `Message`.
struct Reply<'a> {
    emit: &'a (dyn Fn(ChatEvent) + Send + Sync),
    meter: Meter,
    text: String,
    thinking: String,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    stop_reason: Option<String>,
    model: String,
}

impl<'a> Reply<'a> {
    fn new(emit: &'a (dyn Fn(ChatEvent) + Send + Sync), model: &str) -> Self {
        Reply {
            emit,
            meter: Meter::new(),
            text: String::new(),
            thinking: String::new(),
            input_tokens: None,
            output_tokens: None,
            stop_reason: None,
            model: model.into(),
        }
    }

    /// Output starts with the first piece of reasoning or text, whichever comes first.
    fn text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        self.meter.delta();
        self.text.push_str(t);
        (self.emit)(ChatEvent::Text { text: t.into() });
    }

    fn thinking(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        self.meter.delta();
        self.thinking.push_str(t);
        (self.emit)(ChatEvent::Thinking { text: t.into() });
    }

    /// A reply that went quiet or was cut off (`stop`: STALLED or INTERRUPTED): what arrived,
    /// with Claude's unsigned reasoning left out as for a stopped one, or `why` when nothing did.
    fn cut_short(mut self, provider: &str, stop: &str, why: String) -> Result<Message, String> {
        if self.text.is_empty() && self.thinking.is_empty() {
            return Err(why);
        }
        self.stop_reason = Some(stop.into());
        Ok(self.finish(provider, None))
    }

    fn finish(self, provider: &str, blocks: Option<Value>) -> Message {
        let total = self.meter.started.elapsed();
        let tokens_per_sec = self.output_tokens.and_then(|n| self.meter.rate(n)).map(|r| r as f32);
        Message {
            role: "assistant".into(),
            content: self.text,
            thinking: (!self.thinking.is_empty()).then_some(self.thinking),
            anthropic_blocks: blocks,
            provider: Some(provider.into()),
            model: Some(self.model),
            stop_reason: self.stop_reason,
            stats: Some(Stats {
                ttft_ms: self.meter.ttft().map(|d| d.as_millis() as u64),
                total_ms: total.as_millis() as u64,
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                tokens_per_sec,
                deltas: Some(self.meter.deltas),
                decode_ms: self.meter.span().map(|d| d.as_millis() as u64),
            }),
            created_at: unix_now(),
            id: None,
            parent_id: None,
        }
    }
}

/// Prompt size in the demo models' four-characters-a-token count.
fn prompt_estimate(req: &ChatRequest) -> u64 {
    req.messages.iter().map(|m| demo_tokens(&m.content)).sum::<u64>() + demo_tokens(&req.system)
}

/// Claude history: Claude's own replies go back exactly as received (minus what
/// must be dropped after a mid-reply fallback); everything else as plain text.
/// Declined or empty replies aren't sent back; one stopped, stalled or cut off part-way goes
/// back as its text (its reasoning was never signed). A system prompt change goes in right
/// after the question it applies from, before Claude's reply to it: as a mid-conversation
/// system message (`mid_system`), else as a last text block of that question. Either way
/// nothing already sent changes.
pub fn anthropic_messages(messages: &[Message], mid_system: bool) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut change: Option<String> = None;
    for m in messages {
        match m.role.as_str() {
            "system" => change = Some(system_change_text(&m.content)),
            "assistant" => {
                // A declined or empty reply isn't sent back; a change made with it waits for the next one.
                if m.stop_reason.as_deref() == Some("refusal") || (m.content.is_empty() && m.anthropic_blocks.is_none()) {
                    continue;
                }
                if let Some(text) = change.take() {
                    put_change(&mut out, text, mid_system);
                }
                out.push(match &m.anthropic_blocks {
                    Some(Value::Array(blocks)) => json!({ "role": "assistant", "content": replayable(blocks) }),
                    _ => json!({ "role": "assistant", "content": m.content }),
                });
            }
            _ => out.push(json!({ "role": m.role, "content": m.content })),
        }
    }
    if let Some(text) = change {
        put_change(&mut out, text, mid_system);
    }
    out
}

/// A mid-conversation system message must follow a question: never first, never after Claude.
fn put_change(out: &mut Vec<Value>, text: String, mid_system: bool) {
    if !out.last().is_some_and(|m| m["role"] == "user") {
        return;
    }
    if mid_system {
        out.push(json!({ "role": "system", "content": text }));
    } else if let Some(last) = out.last_mut() {
        let mut blocks = match last["content"].take() {
            Value::String(s) => vec![json!({ "type": "text", "text": s })],
            Value::Array(a) => a,
            _ => Vec::new(),
        };
        blocks.push(json!({ "type": "text", "text": text }));
        last["content"] = Value::Array(blocks);
    }
}

/// Whether the history sends Claude's reasoning back (which the API checks against what came before it).
fn replays_thinking(messages: &[Value]) -> bool {
    messages
        .iter()
        .filter(|m| m["role"] == "assistant")
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .any(|b| matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking")))
}

/// The Messages API request for `req`, and the beta headers it needs. The top-level system
/// prompt is the one the conversation got its first reply under (ChatWindow keeps it); later
/// changes are in the history.
pub fn anthropic_request(req: &ChatRequest) -> (Value, Vec<&'static str>) {
    let messages = anthropic_messages(&req.messages, claude_supports_mid_system(&req.model));
    let mut betas = Vec::new();
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.params.max_tokens.unwrap_or(64000),
        "stream": true,
    });
    if !req.system.trim().is_empty() {
        body["system"] = json!(req.system);
    }
    if claude_supports_adaptive(&req.model) {
        let mut thinking = json!({ "type": "adaptive", "display": "summarized" });
        // Replayed reasoning is bound to the conversation before it. Should that have changed
        // after all, the API leaves those blocks out and says so (input_transformations)
        // instead of refusing the turn.
        if replays_thinking(&messages) {
            thinking["block_binding"] = json!({ "prefix_mismatch_behavior": "drop_block" });
            betas.push(BINDING_BETA);
        }
        body["thinking"] = thinking;
        if let Some(effort) = &req.params.effort {
            body["output_config"] = json!({ "effort": effort });
        }
    }
    if claude_supports_fallbacks(&req.model) {
        body["fallbacks"] = json!("default");
        betas.push(FALLBACK_BETA);
    }
    body["messages"] = Value::Array(messages);
    (body, betas)
}

/// Logs what the API did to the history it was sent, and says when it left earlier reasoning out.
fn note_transformations(list: &Value, emit: &(dyn Fn(ChatEvent) + Send + Sync)) {
    let Some(items) = list.as_array().filter(|l| !l.is_empty()) else { return };
    eprintln!("chat: Claude input_transformations {list}");
    for t in items.iter().filter(|t| t["type"] == "thinking_dropped") {
        let why = match t["reason"].as_str() {
            Some("prefix_binding_mismatch") => "the conversation before it changed",
            Some("model_binding_mismatch") => "it came from another model",
            _ => continue,
        };
        emit(ChatEvent::Notice { text: format!("Earlier reasoning ({}) was left out: {why}.", t["path"].as_str().unwrap_or("a reply")) });
    }
}

/// The Responses API request for `req`. A reasoning model (gpt-5, the o-series) gets its
/// effort and, with `summary`, a reasoning summary; it never gets a temperature, which it refuses.
pub fn openai_request(req: &ChatRequest, summary: bool) -> Value {
    let input: Vec<Value> = req
        .messages
        .iter()
        .filter(|m| m.role != "system" && !m.content.is_empty())
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    let mut body = json!({ "model": req.model, "input": input, "stream": true, "store": false });
    let system = current_system(&req.system, &req.messages);
    if !system.trim().is_empty() {
        body["instructions"] = json!(system);
    }
    if catalog::openai_reasons(&req.model) {
        let mut reasoning = serde_json::Map::new();
        if let Some(e) = &req.params.effort {
            reasoning.insert("effort".into(), json!(e));
        }
        if summary {
            reasoning.insert("summary".into(), json!("auto"));
        }
        if !reasoning.is_empty() {
            body["reasoning"] = Value::Object(reasoning);
        }
    } else if let Some(t) = req.params.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(n) = req.params.max_tokens {
        body["max_output_tokens"] = json!(n);
    }
    body
}

/// After a mid-reply fallback, thinking and tool-use blocks that came before the
/// last `fallback` marker must not be sent back. Nor must reasoning with no signature (a
/// reply cut off part-way, saved so before SparkBox checked for the stream's end): the API
/// refuses it.
fn replayable(blocks: &[Value]) -> Vec<Value> {
    let boundary = blocks.iter().rposition(|b| b["type"] == "fallback").unwrap_or(0);
    blocks
        .iter()
        .enumerate()
        .filter(|(i, b)| *i >= boundary || !matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking" | "tool_use")))
        .filter(|(_, b)| b["type"] != "thinking" || b["signature"].as_str().is_some_and(|s| !s.is_empty()))
        .map(|(_, b)| b.clone())
        .collect()
}

impl Llm {
    /// Stream progress only; return both terminal values in the command response.
    /// Keep `chat`'s event contract for non-IPC consumers and its protocol tests.
    pub async fn chat_completion(self: &Arc<Self>, req: ChatRequest, emit: &(dyn Fn(ChatEvent) + Send + Sync), cancel: oneshot::Receiver<()>) -> ChatCompletion {
        let completion = std::sync::Mutex::new(ChatCompletion::default());
        let partial = std::sync::Mutex::new((String::new(), String::new()));
        let provider = req.provider.clone();
        let model = req.model.clone();
        self.chat(req, &|event| match event {
            ChatEvent::Done { message } => completion.lock().unwrap().message = Some(message),
            ChatEvent::Error { message } => completion.lock().unwrap().error = Some(message),
            progress => {
                match &progress {
                    ChatEvent::Text { text } => partial.lock().unwrap().0.push_str(text),
                    ChatEvent::Thinking { text } => partial.lock().unwrap().1.push_str(text),
                    _ => {}
                }
                emit(progress);
            }
        }, cancel).await;
        let mut completion = completion.into_inner().unwrap();
        let (text, thinking) = partial.into_inner().unwrap();
        // A provider can explicitly fail after emitting output, without returning a
        // Message. Keep that output too, even if all progress callbacks arrive late.
        // The original error stays authoritative; no usage or signatures are invented.
        if completion.message.is_none() && completion.error.is_some() && (!text.is_empty() || !thinking.is_empty()) {
            completion.message = Some(Message {
                role: "assistant".into(), content: text, thinking: (!thinking.is_empty()).then_some(thinking),
                provider: Some(provider), model: Some(model), stop_reason: Some(INTERRUPTED.into()),
                anthropic_blocks: None, stats: None, created_at: unix_now(), id: None, parent_id: None,
            });
        }
        completion
    }

    /// Streams one reply. `cancel` ends it early, keeping what arrived so far.
    pub async fn chat(self: &Arc<Self>, req: ChatRequest, emit: &(dyn Fn(ChatEvent) + Send + Sync), cancel: oneshot::Receiver<()>) {
        let Some(provider) = self.chat_provider(&req.provider).await else {
            return emit(ChatEvent::Error { message: format!("Unknown provider {}", req.provider) });
        };
        let result = match provider.kind {
            ProviderKind::Anthropic => self.chat_anthropic(&provider, &req, emit, cancel).await,
            ProviderKind::Openai => self.chat_openai_responses(&provider, &req, emit, cancel).await,
            ProviderKind::OpenaiCompatible => {
                // Counted for the Endpoints window (engines like Ollama publish no metrics of their own).
                let load = self.track(&provider.id, prompt_estimate(&req));
                let result = self.chat_completions(&provider, &req, emit, cancel).await;
                if let Ok(m) = &result {
                    let stats = m.stats.clone().unwrap_or_default();
                    load.tokens(stats.output_tokens.unwrap_or_else(|| demo_tokens(&m.content)));
                    if let Some(t) = stats.ttft_ms {
                        load.first_token(t as f64);
                    }
                }
                result
            }
            ProviderKind::Demo => {
                let last = req.messages.iter().rev().find(|m| m.role == "user").map(|m| m.content.as_str()).unwrap_or("");
                let prompt = prompt_estimate(&req);
                // A simulated cluster model streams at the speed its Endpoints charts show, and its load shows there.
                let speed = self.demo_speed(&provider.id);
                let load = speed.map(|_| self.track(&provider.id, prompt));
                Ok(chat_demo(&provider.id, &req.model, last, prompt, speed, load.as_ref(), emit, cancel).await)
            }
        };
        match result {
            Ok(message) => {
                let why = match message.stop_reason.as_deref() {
                    Some(STALLED) => Some(self.stalled(&provider)),
                    Some(INTERRUPTED) => Some(self.interrupted(&provider)),
                    _ => None,
                };
                emit(ChatEvent::Done { message });
                if let Some(message) = why {
                    emit(ChatEvent::Error { message });
                }
            }
            Err(e) => emit(ChatEvent::Error { message: e }),
        }
    }

    /// Sends a streaming request; no answer within the idle limit counts as stopped.
    async fn post_stream(&self, url: &str, req: reqwest::RequestBuilder, p: &Provider) -> Result<reqwest::Response, String> {
        let resp = match tokio::time::timeout(self.stream_idle(p), req.send()).await {
            Ok(r) => r.map_err(|e| format!("Couldn't reach {} ({url}): {e}", p.name))?,
            Err(_) => return Err(self.stalled(p)),
        };
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        Err(api_error(&p.name, status, &body))
    }

    async fn chat_anthropic(
        &self,
        p: &Provider,
        req: &ChatRequest,
        emit: &(dyn Fn(ChatEvent) + Send + Sync),
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Message, String> {
        let key = p.api_key()?.ok_or("Add your Anthropic API key under AI › Providers.")?;
        let (body, betas) = anthropic_request(req);
        let url = format!("{}/v1/messages", p.base_url.trim_end_matches('/'));
        let mut http = self
            .client_for(&url)
            .await?
            .post(&url)
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION);
        if !betas.is_empty() {
            http = http.header("anthropic-beta", betas.join(","));
        }
        let resp = self.post_stream(&url, http.json(&body), p).await?;

        let mut reply = Reply::new(emit, &req.model);
        let mut blocks: Vec<Value> = Vec::new();
        let mut parser = SseParser::default();
        let mut stream = resp.bytes_stream();
        let idle = self.stream_idle(p);
        // `ended`: the stop reason (or message_stop) came, so the reply is whole.
        let (mut stopped, mut stalled, mut ended) = (false, false, false);
        loop {
            let chunk = tokio::select! {
                c = tokio::time::timeout(idle, stream.next()) => c,
                _ = &mut cancel => { stopped = true; break; }
            };
            let Ok(chunk) = chunk else {
                stalled = true;
                break;
            };
            let chunk = match chunk {
                Some(Ok(c)) => c,
                // The connection broke (a chunked reply cut off): ends as a close does.
                Some(Err(e)) => {
                    eprintln!("chat: the reply from {} broke off: {e}", p.name);
                    break;
                }
                None => break,
            };
            for SseEvent { data, .. } in parser.push(&chunk) {
                let ev: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                match ev["type"].as_str() {
                    Some("message_start") => {
                        if let Some(m) = ev["message"]["model"].as_str() {
                            reply.model = m.into();
                        }
                        reply.input_tokens = ev["message"]["usage"]["input_tokens"].as_u64();
                        note_transformations(&ev["message"]["input_transformations"], emit);
                    }
                    Some("content_block_start") => {
                        let block = ev["content_block"].clone();
                        if block["type"] == "fallback" {
                            emit(ChatEvent::Notice {
                                text: format!(
                                    "{} declined; {} continued.",
                                    block["from"]["model"].as_str().unwrap_or("Claude"),
                                    block["to"]["model"].as_str().unwrap_or("another model")
                                ),
                            });
                        }
                        blocks.push(block);
                    }
                    Some("content_block_delta") => {
                        let i = ev["index"].as_u64().unwrap_or(0) as usize;
                        let d = &ev["delta"];
                        let Some(block) = blocks.get_mut(i) else { continue };
                        match d["type"].as_str() {
                            Some("text_delta") => {
                                let t = d["text"].as_str().unwrap_or("");
                                let joined = format!("{}{t}", block["text"].as_str().unwrap_or(""));
                                block["text"] = json!(joined);
                                reply.text(t);
                            }
                            Some("thinking_delta") => {
                                let t = d["thinking"].as_str().unwrap_or("");
                                let joined = format!("{}{t}", block["thinking"].as_str().unwrap_or(""));
                                block["thinking"] = json!(joined);
                                reply.thinking(t);
                            }
                            Some("signature_delta") => block["signature"] = d["signature"].clone(),
                            _ => {}
                        }
                    }
                    Some("message_delta") => {
                        // Again after a fallback part-way through the reply.
                        note_transformations(&ev["input_transformations"], emit);
                        note_transformations(&ev["delta"]["input_transformations"], emit);
                        if let Some(r) = ev["delta"]["stop_reason"].as_str() {
                            reply.stop_reason = Some(r.into());
                            ended = true;
                        }
                        if let Some(n) = ev["usage"]["output_tokens"].as_u64() {
                            reply.output_tokens = Some(n);
                        }
                    }
                    Some("message_stop") => ended = true,
                    Some("error") => {
                        return Err(format!("Anthropic: {}", ev["error"]["message"].as_str().unwrap_or("stream error")));
                    }
                    _ => {}
                }
            }
        }
        if stopped {
            reply.stop_reason = Some("stopped".into());
            // An interrupted reply has unsigned reasoning; keep only its text.
            return Ok(reply.finish(&p.id, None));
        }
        if stalled {
            return reply.cut_short(&p.id, STALLED, self.stalled(p));
        }
        // The connection closed (or broke) before the end: its last reasoning has no signature.
        if !ended {
            return reply.cut_short(&p.id, INTERRUPTED, self.interrupted(p));
        }
        if reply.stop_reason.as_deref() == Some("refusal") {
            emit(ChatEvent::Notice { text: "Claude declined this request.".into() });
        }
        Ok(reply.finish(&p.id, Some(Value::Array(blocks))))
    }

    async fn chat_openai_responses(
        &self,
        p: &Provider,
        req: &ChatRequest,
        emit: &(dyn Fn(ChatEvent) + Send + Sync),
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Message, String> {
        let key = p.api_key()?.ok_or("Add your OpenAI API key under AI › Providers.")?;
        let url = format!("{}/responses", p.base_url.trim_end_matches('/'));
        let client = self.client_for(&url).await?;
        let send = |summary: bool| client.post(&url).bearer_auth(&key).json(&openai_request(req, summary));
        let resp = match self.post_stream(&url, send(true), p).await {
            // Reasoning summaries need a verified OpenAI organization: answer without them.
            Err(e) if catalog::openai_reasons(&req.model) && e.contains(" returned 400:") && e.to_lowercase().contains("summar") => {
                emit(ChatEvent::Notice { text: "OpenAI doesn't give this account reasoning summaries, so this reply comes without one.".into() });
                self.post_stream(&url, send(false), p).await?
            }
            r => r?,
        };

        let mut reply = Reply::new(emit, &req.model);
        let mut parser = SseParser::default();
        let mut stream = resp.bytes_stream();
        let idle = self.stream_idle(p);
        // `ended`: the response's last event came (or Stop), so the reply is whole.
        let (mut stalled, mut ended) = (false, false);
        loop {
            let chunk = tokio::select! {
                c = tokio::time::timeout(idle, stream.next()) => c,
                _ = &mut cancel => { reply.stop_reason = Some("stopped".into()); ended = true; break; }
            };
            let Ok(chunk) = chunk else {
                stalled = true;
                break;
            };
            let chunk = match chunk {
                Some(Ok(c)) => c,
                // The connection broke (a chunked reply cut off): ends as a close does.
                Some(Err(e)) => {
                    eprintln!("chat: the reply from {} broke off: {e}", p.name);
                    break;
                }
                None => break,
            };
            for SseEvent { data, .. } in parser.push(&chunk) {
                let ev: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                match ev["type"].as_str() {
                    Some("response.output_text.delta") => reply.text(ev["delta"].as_str().unwrap_or("")),
                    Some("response.reasoning_summary_text.delta") => reply.thinking(ev["delta"].as_str().unwrap_or("")),
                    Some("response.completed") | Some("response.incomplete") => {
                        let r = &ev["response"];
                        if let Some(m) = r["model"].as_str() {
                            reply.model = m.into();
                        }
                        reply.input_tokens = r["usage"]["input_tokens"].as_u64();
                        reply.output_tokens = r["usage"]["output_tokens"].as_u64();
                        reply.stop_reason = r["status"].as_str().map(String::from);
                        ended = true;
                    }
                    Some("response.failed") => {
                        return Err(format!("OpenAI: {}", ev["response"]["error"]["message"].as_str().unwrap_or("request failed")));
                    }
                    Some("error") => {
                        return Err(format!("OpenAI: {}", ev["message"].as_str().or(ev["error"]["message"].as_str()).unwrap_or("stream error")));
                    }
                    _ => {}
                }
            }
        }
        if stalled {
            return reply.cut_short(&p.id, STALLED, self.stalled(p));
        }
        if !ended {
            return reply.cut_short(&p.id, INTERRUPTED, self.interrupted(p));
        }
        Ok(reply.finish(&p.id, None))
    }

    async fn chat_completions(
        &self,
        p: &Provider,
        req: &ChatRequest,
        emit: &(dyn Fn(ChatEvent) + Send + Sync),
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Message, String> {
        let mut messages = Vec::new();
        let system = current_system(&req.system, &req.messages);
        if !system.trim().is_empty() {
            messages.push(json!({ "role": "system", "content": system }));
        }
        messages.extend(req.messages.iter().filter(|m| m.role != "system" && !m.content.is_empty()).map(|m| json!({ "role": m.role, "content": m.content })));
        let mut body = json!({
            "model": req.model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if let Some(t) = req.params.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(n) = req.params.max_tokens {
            body["max_tokens"] = json!(n);
        }
        let url = format!("{}/chat/completions", p.base_url.trim_end_matches('/'));
        let mut http = self.client_for(&url).await?.post(&url).json(&body);
        if let Some(key) = p.api_key()? {
            http = http.bearer_auth(key);
        }
        let resp = self.post_stream(&url, http, p).await?;

        let mut reply = Reply::new(emit, &req.model);
        let mut think = ThinkTags::default();
        let mut parser = SseParser::default();
        let mut stream = resp.bytes_stream();
        let idle = self.stream_idle(p);
        // `ended`: a finish_reason or [DONE] came (or Stop), so the reply is whole.
        let (mut stalled, mut ended) = (false, false);
        'read: loop {
            let chunk = tokio::select! {
                c = tokio::time::timeout(idle, stream.next()) => c,
                _ = &mut cancel => { reply.stop_reason = Some("stopped".into()); ended = true; break; }
            };
            let Ok(chunk) = chunk else {
                stalled = true;
                break;
            };
            let chunk = match chunk {
                Some(Ok(c)) => c,
                // The connection broke (a chunked reply cut off): ends as a close does.
                Some(Err(e)) => {
                    eprintln!("chat: the reply from {} broke off: {e}", p.name);
                    break;
                }
                None => break,
            };
            for SseEvent { data, .. } in parser.push(&chunk) {
                if data.trim() == "[DONE]" {
                    ended = true;
                    break 'read;
                }
                let ev: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                if let Some(msg) = ev["error"]["message"].as_str() {
                    return Err(format!("{}: {msg}", p.name));
                }
                let choice = &ev["choices"][0];
                let delta = &choice["delta"];
                // vLLM/SGLang send reasoning_content; Ollama and others send reasoning.
                let reasoning = delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()).unwrap_or("");
                reply.thinking(reasoning);
                if let Some(content) = delta["content"].as_str() {
                    for (is_thinking, part) in think.split(content) {
                        if is_thinking { reply.thinking(&part) } else { reply.text(&part) }
                    }
                }
                if let Some(r) = choice["finish_reason"].as_str() {
                    reply.stop_reason = Some(r.into());
                    ended = true;
                }
                if let Some(m) = ev["model"].as_str() {
                    reply.model = m.into();
                }
                if ev["usage"].is_object() {
                    reply.input_tokens = ev["usage"]["prompt_tokens"].as_u64();
                    reply.output_tokens = ev["usage"]["completion_tokens"].as_u64();
                }
            }
        }
        if stalled {
            return reply.cut_short(&p.id, STALLED, self.stalled(p));
        }
        if !ended {
            return reply.cut_short(&p.id, INTERRUPTED, self.interrupted(p));
        }
        Ok(reply.finish(&p.id, None))
    }
}

/// Splits inline `<think>…</think>` reasoning (DeepSeek/Qwen style) out of
/// streamed text, even when the tags are cut across chunks.
#[derive(Default)]
struct ThinkTags {
    inside: bool,
    pending: String,
}

impl ThinkTags {
    fn split(&mut self, chunk: &str) -> Vec<(bool, String)> {
        let mut s = std::mem::take(&mut self.pending) + chunk;
        let mut out = Vec::new();
        loop {
            let tag = if self.inside { "</think>" } else { "<think>" };
            if let Some(i) = s.find(tag) {
                if i > 0 {
                    out.push((self.inside, s[..i].to_string()));
                }
                s = s[i + tag.len()..].to_string();
                self.inside = !self.inside;
                continue;
            }
            // Hold back a possible partial tag at the end.
            let keep = (1..tag.len()).rev().find(|&n| s.len() >= n && s.is_char_boundary(s.len() - n) && tag.starts_with(&s[s.len() - n..]));
            if let Some(n) = keep {
                self.pending = s[s.len() - n..].to_string();
                s.truncate(s.len() - n);
            }
            if !s.is_empty() {
                out.push((self.inside, s));
            }
            return out;
        }
    }
}

/// A simulated cluster model's answer: short, because a 70B model on GB10s really is slow.
pub fn demo_cluster_reply(model: &str, prompt: &str) -> String {
    let p = prompt.trim();
    let quoted = if p.chars().count() > 80 { format!("{}…", p.chars().take(80).collect::<String>()) } else { p.to_string() };
    format!(
        "This is **{model}** in SparkBox's demo cluster, so the reply is simulated, but it streams at the speed the \
         Endpoints window shows for this model right now. You asked: *{quoted}*"
    )
}

/// The demo model's answer to a prompt.
pub fn demo_reply(prompt: &str) -> String {
    let p = prompt.trim();
    let quoted = if p.chars().count() > 120 { format!("{}…", p.chars().take(120).collect::<String>()) } else { p.to_string() };
    format!(
        "This is the **SparkBox demo model**, so nothing left your Mac. You said:\n\n> {quoted}\n\n\
         Once you add an API key or a cluster endpoint under *AI › Providers*, pick it above and the same chat goes to a real model.\n\n\
         A few things this reply exercises:\n\n\
         - Markdown lists and **bold** text\n\
         - Streaming, one word at a time\n\
         - A code block with a copy button:\n\n\
         ```bash\ncurl http://127.0.0.1:8484/v1/models -H \"Authorization: Bearer $SPARKBOX_KEY\"\n```\n"
    )
}

/// Streams the demo reply word by word. The built-in demo model thinks briefly first; a
/// simulated cluster model waits its time to first token and then streams at `speed`.
/// Tokens are counted as four characters each.
#[allow(clippy::too_many_arguments)]
async fn chat_demo(
    provider: &str,
    model: &str,
    prompt: &str,
    prompt_tokens: u64,
    speed: Option<DemoSpeed>,
    load: Option<&super::TrafficGuard>,
    emit: &(dyn Fn(ChatEvent) + Send + Sync),
    mut cancel: oneshot::Receiver<()>,
) -> Message {
    let mut reply = Reply::new(emit, model);
    reply.input_tokens = Some(prompt_tokens);
    let text = match speed {
        Some(s) => {
            let wait = s.ttft_ms.clamp(50.0, 20_000.0);
            tokio::time::sleep(Duration::from_millis(wait as u64)).await;
            if let Some(l) = load {
                l.first_token(wait);
            }
            demo_cluster_reply(model, prompt)
        }
        None => {
            for t in ["Reading ", "the ", "prompt ", "and ", "planning ", "a ", "short ", "answer."] {
                reply.thinking(t);
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            demo_reply(prompt)
        }
    };
    let tps = speed.map(|s| s.tps.max(0.2)).unwrap_or(70.0);
    for piece in text.split_inclusive(' ') {
        if cancel.try_recv().is_ok() {
            reply.stop_reason = Some("stopped".into());
            break;
        }
        reply.text(piece);
        let n = demo_tokens(piece);
        if let Some(l) = load {
            l.tokens(n);
        }
        tokio::time::sleep(Duration::from_secs_f64(n.max(1) as f64 / tps)).await;
    }
    reply.output_tokens = Some(demo_tokens(&reply.text) + demo_tokens(&reply.thinking));
    reply.stop_reason.get_or_insert_with(|| "end_turn".into());
    reply.finish(provider, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str, blocks: Option<Value>) -> Message {
        Message {
            role: role.into(),
            content: content.into(),
            thinking: None,
            anthropic_blocks: blocks,
            provider: None,
            model: None,
            stop_reason: None,
            stats: None,
            created_at: 0,
            id: None,
            parent_id: None,
        }
    }

    #[test]
    fn claude_replies_are_replayed_exactly() {
        let blocks = json!([{ "type": "thinking", "thinking": "", "signature": "sig" }, { "type": "text", "text": "Hi" }]);
        let out = anthropic_messages(&[msg("user", "Hello", None), msg("assistant", "Hi", Some(blocks.clone())), msg("user", "More", None)], true);
        assert_eq!(out[1]["content"], blocks);
        assert_eq!(out[2], json!({ "role": "user", "content": "More" }));
    }

    #[test]
    fn reasoning_before_a_fallback_is_dropped() {
        let blocks = json!([
            { "type": "thinking", "thinking": "a", "signature": "s1" },
            { "type": "text", "text": "partial " },
            { "type": "fallback", "from": { "model": "claude-opus-5-5" }, "to": { "model": "claude-opus-5" } },
            { "type": "thinking", "thinking": "b", "signature": "s2" },
            { "type": "text", "text": "rest" }
        ]);
        let out = anthropic_messages(&[msg("user", "q", None), msg("assistant", "partial rest", Some(blocks))], true);
        let types: Vec<&str> = out[1]["content"].as_array().unwrap().iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["text", "fallback", "thinking", "text"]);
    }

    #[test]
    fn declined_replies_are_not_sent_back() {
        let mut refused = msg("assistant", "", Some(json!([])));
        refused.stop_reason = Some("refusal".into());
        let out = anthropic_messages(&[msg("user", "q", None), refused, msg("user", "q2", None)], true);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn think_tags_split_across_chunks() {
        let mut t = ThinkTags::default();
        let mut got = Vec::new();
        for chunk in ["<thi", "nk>plan", "ning</th", "ink>Answer"] {
            got.extend(t.split(chunk));
        }
        let thinking: String = got.iter().filter(|(t, _)| *t).map(|(_, s)| s.as_str()).collect();
        let text: String = got.iter().filter(|(t, _)| !*t).map(|(_, s)| s.as_str()).collect();
        assert_eq!((thinking.as_str(), text.as_str()), ("planning", "Answer"));
    }

    #[test]
    fn model_capabilities() {
        assert!(claude_supports_adaptive("claude-opus-5-5"));
        assert!(claude_supports_adaptive("claude-haiku-5-5"));
        assert!(!claude_supports_adaptive("claude-haiku-4-5"));
        assert!(claude_supports_fallbacks("claude-opus-5-5"));
        assert!(!claude_supports_fallbacks("claude-haiku-5-5"));
        for m in ["claude-opus-5-5", "claude-opus-5", "claude-opus-4-8", "claude-fable-5-1", "claude-sonnet-5-5", "claude-haiku-5-5"] {
            assert!(claude_supports_mid_system(m), "{m}");
        }
        for m in ["claude-sonnet-5", "claude-opus-4-7", "claude-sonnet-4-6", "claude-haiku-4-5"] {
            assert!(!claude_supports_mid_system(m), "{m}");
        }
    }

    fn with_id(mut m: Message, id: &str) -> Message {
        m.id = Some(id.into());
        m
    }

    fn reply(text: &str, sig: &str) -> Message {
        let blocks = json!([{ "type": "thinking", "thinking": "Plan.", "signature": sig }, { "type": "text", "text": text }]);
        let mut m = msg("assistant", text, Some(blocks));
        m.stop_reason = Some("end_turn".into());
        m
    }

    fn request(model: &str, system: &str, messages: Vec<Message>) -> ChatRequest {
        ChatRequest {
            request_id: "r".into(),
            provider: "anthropic".into(),
            model: model.into(),
            system: system.into(),
            params: ChatParams { effort: Some("high".into()), ..Default::default() },
            messages,
        }
    }

    /// The branch ChatWindow sends: root to leaf (thread.ts).
    fn branch(all: &[Message], leaf: &[&str]) -> Vec<Message> {
        leaf.iter().map(|id| all.iter().find(|m| m.id.as_deref() == Some(*id)).unwrap().clone()).collect()
    }

    #[test]
    fn a_three_turn_claude_chat_with_an_edit_a_regenerate_and_a_system_change_stays_append_only() {
        let model = "claude-opus-5-5";
        // Turn 1, then the first question edited: a new branch from the root.
        let mut all = vec![with_id(msg("user", "Name a fruit", None), "u1"), with_id(reply("Apple", "s1"), "a1")];
        all.push(with_id(msg("user", "Name a red fruit", None), "u1b"));
        let (first, betas) = anthropic_request(&request(model, "Be brief.", branch(&all, &["u1b"])));
        assert_eq!(first["system"], "Be brief.");
        assert_eq!(first["messages"], json!([{ "role": "user", "content": "Name a red fruit" }]), "the edit's branch doesn't carry the old reply");
        assert!(first["thinking"].get("block_binding").is_none(), "nothing replayed yet");
        assert_eq!(betas, [FALLBACK_BETA]);
        all.push(with_id(reply("Cherry", "s2"), "a1b"));

        // Turn 2, and a regenerate of it: the same request, byte for byte.
        all.push(with_id(msg("user", "Another?", None), "u2"));
        let turn2 = request(model, "Be brief.", branch(&all, &["u1b", "a1b", "u2"]));
        let (second, betas) = anthropic_request(&turn2);
        let (again, _) = anthropic_request(&turn2);
        assert_eq!(serde_json::to_string(&second).unwrap(), serde_json::to_string(&again).unwrap(), "a regenerate sends the same history");
        assert_eq!(second["messages"][1]["content"][0]["signature"], "s2", "Claude's reply goes back as received");
        assert_eq!(second["thinking"], json!({ "type": "adaptive", "display": "summarized", "block_binding": { "prefix_mismatch_behavior": "drop_block" } }));
        assert_eq!(betas, [BINDING_BETA, FALLBACK_BETA]);
        all.push(with_id(reply("Strawberry", "s3"), "a2"));
        all.push(with_id(reply("Raspberry", "s3b"), "a2b"));

        // The system prompt changed before turn 3: the top-level one stays as it was, and the
        // change follows the new question.
        all.push(with_id(msg("user", "And a green one?", None), "u3"));
        all.push(with_id(msg("system", "Answer in French.", None), "sys3"));
        let (third, betas) = anthropic_request(&request(model, "Be brief.", branch(&all, &["u1b", "a1b", "u2", "a2b", "u3", "sys3"])));
        assert_eq!(third["system"], "Be brief.", "frozen");
        let m = third["messages"].as_array().unwrap();
        assert_eq!(m.len(), 6);
        assert_eq!(m[..3], second["messages"].as_array().unwrap()[..], "what turn 2 sent is unchanged");
        assert_eq!(m[3]["content"][0]["signature"], "s3b", "the regenerated reply is the one on the branch");
        assert_eq!(m[4], json!({ "role": "user", "content": "And a green one?" }));
        assert_eq!(m[5], json!({ "role": "system", "content": "From here on, follow this system prompt instead of the earlier one:\n\nAnswer in French." }), "last, after the question");
        assert_eq!(third["thinking"]["block_binding"]["prefix_mismatch_behavior"], "drop_block");
        assert_eq!(betas, [BINDING_BETA, FALLBACK_BETA]);
        assert_eq!((third["model"].as_str(), third["fallbacks"].as_str(), third["output_config"]["effort"].as_str()), (Some(model), Some("default"), Some("high")));
        assert!(third.get("temperature").is_none());

        // Turn 4: the change stays where it was made, followed by Claude's reply to it.
        all.push(with_id(reply("Pomme verte", "s4"), "a3"));
        all.push(with_id(msg("user", "Merci", None), "u4"));
        let (fourth, _) = anthropic_request(&request(model, "Be brief.", branch(&all, &["u1b", "a1b", "u2", "a2b", "u3", "sys3", "a3", "u4"])));
        let m4 = fourth["messages"].as_array().unwrap();
        assert_eq!(m4[..6], m[..]);
        assert_eq!((m4[6]["role"].as_str(), m4[7]["role"].as_str()), (Some("assistant"), Some("user")));
        assert_eq!(fourth["system"], "Be brief.");
    }

    #[test]
    fn a_system_change_for_a_model_without_mid_conversation_system_messages_joins_the_question() {
        let history = vec![msg("user", "Hi", None), reply("Hello", "s1"), msg("user", "Again", None), msg("system", "", None)];
        let (body, _) = anthropic_request(&request("claude-sonnet-5", "Be brief.", history));
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3, "no system role for this model");
        assert_eq!(m[2]["content"], json!([{ "type": "text", "text": "Again" }, { "type": "text", "text": "The earlier system prompt no longer applies." }]));
        assert_eq!(body["system"], "Be brief.");
    }

    #[test]
    fn a_change_made_with_a_declined_reply_waits_for_the_next_one() {
        let mut refused = msg("assistant", "", Some(json!([])));
        refused.stop_reason = Some("refusal".into());
        let out = anthropic_messages(&[msg("user", "q", None), msg("system", "Be kind.", None), refused, msg("user", "q2", None)], true);
        let roles: Vec<&str> = out.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "user", "system"], "still after a question, and last");
        // A change can't open the conversation.
        assert!(anthropic_messages(&[msg("system", "x", None)], true).is_empty());
    }

    #[test]
    fn other_providers_get_the_latest_system_prompt_as_theirs() {
        let history = vec![msg("user", "Hi", None), msg("system", "Answer in French.", None), msg("assistant", "Bonjour", None), msg("user", "Merci", None)];
        assert_eq!(current_system("Be brief.", &history), "Answer in French.");
        assert_eq!(current_system("Be brief.", &history[..1]), "Be brief.");
        let mut req = request("gpt-4.1", "Be brief.", history);
        req.params.temperature = Some(0.4);
        let body = openai_request(&req, true);
        assert_eq!(body["instructions"], "Answer in French.");
        assert_eq!(body["input"].as_array().unwrap().len(), 3, "no system turn in the input");
    }

    #[test]
    fn openai_reasoning_models_get_an_effort_and_a_summary_and_no_temperature() {
        let mut req = request("gpt-5.5", "", vec![msg("user", "Hi", None)]);
        req.params = ChatParams { effort: Some("medium".into()), temperature: Some(0.7), max_tokens: Some(2000) };
        let body = openai_request(&req, true);
        assert_eq!(body["reasoning"], json!({ "effort": "medium", "summary": "auto" }));
        assert!(body.get("temperature").is_none(), "gpt-5 refuses a temperature");
        assert_eq!(body["max_output_tokens"], 2000);
        assert_eq!(openai_request(&req, false)["reasoning"], json!({ "effort": "medium" }));
        req.params.effort = None;
        req.model = "o4-mini".into();
        assert_eq!(openai_request(&req, true)["reasoning"], json!({ "summary": "auto" }));
        // A model that samples keeps its temperature and gets no reasoning.
        req.model = "gpt-4.1".into();
        let body = openai_request(&req, true);
        assert_eq!(body["temperature"], json!(0.7f32));
        assert!(body.get("reasoning").is_none());
    }

    // ---- Against a local server that answers as the provider would

    /// One connection: records the raw request, answers with `head` and `body`, then holds the
    /// connection open for `hold` (a model that stopped mid-reply) or closes it.
    async fn upstream(head: &'static str, body: String, hold: Duration) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        upstreams(vec![(head, body, hold)]).await
    }

    /// Connections one after another, each answered with the next reply; every raw request is kept.
    async fn upstreams(replies: Vec<(&'static str, String, Duration)>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let got = seen.clone();
        tokio::spawn(async move {
            for (head, body, hold) in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = sock.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            got.lock().unwrap().push(text);
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                sock.write_all(format!("{head}\r\n\r\n{body}").as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
                if hold.is_zero() {
                    drop(sock);
                } else {
                    tokio::spawn(async move {
                        tokio::time::sleep(hold).await;
                        drop(sock);
                    });
                }
            }
        });
        (base, seen)
    }

    const SSE_OK: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close";

    async fn app(name: &str) -> Arc<Llm> {
        let dir = std::env::temp_dir().join(format!("sparkbox-chat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Llm::new(dir.join("providers.json"), crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}))
    }

    async fn run(llm: &Arc<Llm>, req: ChatRequest) -> Vec<ChatEvent> {
        let events = std::sync::Mutex::new(Vec::new());
        let (_stop, rx) = oneshot::channel();
        llm.chat(req, &|e| events.lock().unwrap().push(e), rx).await;
        events.into_inner().unwrap()
    }

    #[tokio::test]
    async fn claude_gets_the_binding_beta_and_says_when_reasoning_was_left_out() {
        let sse = [
            json!({ "type": "message_start", "message": { "model": "claude-opus-5-5", "usage": { "input_tokens": 30 },
                "input_transformations": [{ "type": "thinking_dropped", "path": "messages.1.content.0", "reason": "prefix_binding_mismatch" }] } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Kiwi" } }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 2 } }),
        ]
        .iter()
        .map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap()))
        .collect::<String>();
        let (base, seen) = upstream(SSE_OK, sse, Duration::ZERO).await;
        let llm = app("claude-binding").await;
        llm.upsert(Provider { id: "claude-r2".into(), name: "Claude".into(), kind: ProviderKind::Anthropic, base_url: base, ..Default::default() }).await.unwrap();
        crate::secrets::set("llm:claude-r2", "sk-ant-test").unwrap();
        let mut req = request("claude-opus-5-5", "Be brief.", vec![msg("user", "Fruit?", None), reply("Apple", "s1"), msg("user", "Green?", None), msg("system", "Answer in French.", None)]);
        req.provider = "claude-r2".into();
        let events = run(&llm, req).await;
        crate::secrets::clear("llm:claude-r2").unwrap();

        let raw = seen.lock().unwrap()[0].clone();
        let head = raw.to_ascii_lowercase();
        assert!(head.contains("anthropic-version: 2026-10-21"), "{raw}");
        assert!(head.contains(&format!("anthropic-beta: {BINDING_BETA},{FALLBACK_BETA}")), "{raw}");
        let body: Value = serde_json::from_str(&raw[raw.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(body["thinking"]["block_binding"]["prefix_mismatch_behavior"], "drop_block");
        assert_eq!(body["messages"][3]["role"], "system");
        assert!(events.iter().any(|e| matches!(e, ChatEvent::Notice { text } if text == "Earlier reasoning (messages.1.content.0) was left out: the conversation before it changed.")), "{events:?}");
        assert!(matches!(events.last(), Some(ChatEvent::Done { message }) if message.content == "Kiwi"));
    }

    fn cluster_request(provider: &str) -> ChatRequest {
        ChatRequest { provider: provider.into(), model: "qwen3-8b".into(), params: ChatParams::default(), ..request("qwen3-8b", "", vec![msg("user", "Hi", None)]) }
    }

    async fn cluster_llm(name: &str, base: String) -> Arc<Llm> {
        let llm = app(name).await;
        llm.upsert(Provider { id: format!("cell2-{name}"), name: "vLLM on cell2".into(), base_url: format!("{base}/v1"), ..Default::default() }).await.unwrap();
        llm.set_nodes(vec![("cell2".into(), vec!["127.0.0.1".into()])]);
        llm.set_stream_idle(Duration::from_millis(400));
        llm
    }

    #[tokio::test]
    async fn ipc_completion_keeps_the_reply_and_warning_together_without_terminal_channel_events() {
        let data = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "Half", "reasoning_content": "Plan" } }] }));
        let finished = format!("{data}data: {}\n\n", json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }));
        let failed = format!("data: {}\n\n", json!({ "error": { "message": "fixture engine failed" } }));
        let chunked = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked";
        // A missing final chunk, clean EOF, a quiet stream, a completed reply and an
        // empty failure must all reach the IPC response independently of progress delivery.
        for (name, head, body, hold, reason, warning) in [
            ("ipc-chunked", chunked, format!("{:x}\r\n{data}\r\n", data.len()), Duration::ZERO, Some(INTERRUPTED), true),
            ("ipc-eof", SSE_OK, data.clone(), Duration::ZERO, Some(INTERRUPTED), true),
            ("ipc-provider-error", SSE_OK, format!("{data}{failed}"), Duration::ZERO, Some(INTERRUPTED), true),
            ("ipc-idle", SSE_OK, data, Duration::from_secs(30), Some(STALLED), true),
            ("ipc-finished", SSE_OK, finished, Duration::ZERO, Some("stop"), false),
            ("ipc-empty", SSE_OK, String::new(), Duration::ZERO, None, true),
            ("ipc-error-before-output", SSE_OK, failed, Duration::ZERO, None, true),
        ] {
            let (base, seen) = upstream(head, body, hold).await;
            let llm = cluster_llm(name, base).await;
            let events = std::sync::Mutex::new(Vec::new());
            let (_stop, rx) = oneshot::channel();
            let completion = llm.chat_completion(cluster_request(&format!("cell2-{name}")), &|e| events.lock().unwrap().push(e), rx).await;
            assert_eq!(completion.message.as_ref().and_then(|m| m.stop_reason.as_deref()), reason, "{name}");
            assert_eq!(completion.error.is_some(), warning, "{name}");
            if let Some(message) = &completion.message {
                assert_eq!(message.content, "Half", "{name}");
                assert_eq!(message.thinking.as_deref(), Some("Plan"), "{name}");
                if name == "ipc-provider-error" {
                    assert!(message.stats.is_none());
                    assert!(message.anthropic_blocks.is_none());
                    assert!(completion.error.as_deref().unwrap().contains("fixture engine failed"));
                }
            }
            let wire = serde_json::to_value(&completion).unwrap();
            assert_eq!(wire["message"].is_null(), reason.is_none(), "{name}");
            assert_eq!(wire["error"].is_string(), warning, "{name}");
            assert!(events.lock().unwrap().iter().all(|e| matches!(e, ChatEvent::Text { .. } | ChatEvent::Thinking { .. } | ChatEvent::Notice { .. })), "terminal values must not race on the channel: {name}");
            assert_eq!(seen.lock().unwrap().len(), 1, "never resend after an interruption: {name}");
        }
    }

    #[tokio::test]
    async fn ipc_completion_keeps_user_stop_distinct_from_a_connection_failure() {
        let data = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "Stopped here" } }] }));
        let (base, seen) = upstream(SSE_OK, data, Duration::from_secs(30)).await;
        let llm = cluster_llm("ipc-stop", base).await;
        let (stop, rx) = oneshot::channel();
        let stop = std::sync::Mutex::new(Some(stop));
        let completion = llm.chat_completion(cluster_request("cell2-ipc-stop"), &|e| {
            if matches!(e, ChatEvent::Text { .. }) {
                if let Some(stop) = stop.lock().unwrap().take() {
                    let _ = stop.send(());
                }
            }
        }, rx).await;
        let message = completion.message.unwrap();
        assert_eq!(message.content, "Stopped here");
        assert_eq!(message.stop_reason.as_deref(), Some("stopped"));
        assert!(completion.error.is_none());
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_reply_that_goes_quiet_ends_with_what_arrived_and_says_the_model_stopped() {
        let first = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "Hel" } }] }));
        let (base, _) = upstream(SSE_OK, first, Duration::from_secs(30)).await;
        let llm = cluster_llm("idle-part", base).await;
        let started = std::time::Instant::now();
        let events = run(&llm, cluster_request("cell2-idle-part")).await;
        assert!(started.elapsed() < Duration::from_secs(10), "ended by the idle limit, not the server");
        assert!(matches!(&events[0], ChatEvent::Text { text } if text == "Hel"), "{events:?}");
        assert!(matches!(&events[1], ChatEvent::Done { message } if message.content == "Hel" && message.stop_reason.as_deref() == Some(STALLED)), "{events:?}");
        assert!(matches!(&events[2], ChatEvent::Error { message } if message == "The model stopped responding (is cell2 still up?)"), "{events:?}");
    }

    /// The events in a few words, for assertions.
    fn shape(events: &[ChatEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| match e {
                ChatEvent::Done { message } => format!("Done({:?}, {})", message.stop_reason.as_deref().unwrap_or(""), message.content),
                ChatEvent::Error { message } => format!("Error({message})"),
                ChatEvent::Notice { text } => format!("Notice({text})"),
                ChatEvent::Text { text } => format!("Text({text})"),
                ChatEvent::Thinking { text } => format!("Thinking({text})"),
            })
            .collect()
    }

    fn sse(events: &[Value]) -> String {
        events.iter().map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())).collect()
    }

    #[tokio::test]
    async fn a_claude_reply_whose_connection_closes_part_way_keeps_its_text_and_not_its_unsigned_reasoning() {
        // The connection closes cleanly part-way through: no signature, no message_delta, no message_stop.
        let body = sse(&[
            json!({ "type": "message_start", "message": { "model": "claude-opus-5-5", "usage": { "input_tokens": 30 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "", "signature": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Let me think" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Partial ans" } }),
        ]);
        let (base, _) = upstream(SSE_OK, body, Duration::ZERO).await;
        let llm = app("claude-cut").await;
        llm.upsert(Provider { id: "claude-cut".into(), name: "Claude".into(), kind: ProviderKind::Anthropic, base_url: base, ..Default::default() }).await.unwrap();
        crate::secrets::set("llm:claude-cut", "sk-ant-test").unwrap();
        let mut req = request("claude-opus-5-5", "", vec![msg("user", "Fruit?", None)]);
        req.provider = "claude-cut".into();
        let events = run(&llm, req).await;
        crate::secrets::clear("llm:claude-cut").unwrap();
        assert_eq!(shape(&events), ["Thinking(Let me think)", "Text(Partial ans)", "Done(\"interrupted\", Partial ans)", "Error(The connection to Claude closed before the reply finished.)"]);
        let Some(ChatEvent::Done { message }) = events.iter().find(|e| matches!(e, ChatEvent::Done { .. })).cloned() else { unreachable!() };
        assert_eq!(message.anthropic_blocks, None, "its reasoning has no signature");
        // The next turn sends what arrived as plain text.
        let (next, _) = anthropic_request(&request("claude-opus-5-5", "", vec![msg("user", "Fruit?", None), message, msg("user", "Go on", None)]));
        assert_eq!(next["messages"][1], json!({ "role": "assistant", "content": "Partial ans" }));
    }

    #[tokio::test]
    async fn a_claude_reply_that_got_its_stop_reason_is_finished_even_if_message_stop_never_came() {
        let body = sse(&[
            json!({ "type": "message_start", "message": { "model": "claude-opus-5-5", "usage": { "input_tokens": 30 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "", "signature": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Plan." } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig-1" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Kiwi" } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 9 } }),
        ]);
        let (base, _) = upstream(SSE_OK, body, Duration::ZERO).await;
        let llm = app("claude-ended").await;
        llm.upsert(Provider { id: "claude-ended".into(), name: "Claude".into(), kind: ProviderKind::Anthropic, base_url: base, ..Default::default() }).await.unwrap();
        crate::secrets::set("llm:claude-ended", "sk-ant-test").unwrap();
        let mut req = request("claude-opus-5-5", "", vec![msg("user", "Fruit?", None)]);
        req.provider = "claude-ended".into();
        let events = run(&llm, req).await;
        crate::secrets::clear("llm:claude-ended").unwrap();
        assert_eq!(shape(&events), ["Thinking(Plan.)", "Text(Kiwi)", "Done(\"end_turn\", Kiwi)"]);
        let Some(ChatEvent::Done { message }) = events.last().cloned() else { unreachable!() };
        assert_eq!(message.anthropic_blocks.unwrap()[0]["signature"], "sig-1", "kept to send back");
    }

    #[test]
    fn reasoning_saved_without_its_signature_is_never_sent_back() {
        // What a reply cut off part-way was saved with before SparkBox checked for the stream's end.
        let blocks = json!([{ "type": "thinking", "thinking": "Let me think", "signature": "" }, { "type": "text", "text": "Partial ans" }]);
        let old = msg("assistant", "Partial ans", Some(blocks));
        let signed = reply("Apple", "s1");
        let (body, betas) = anthropic_request(&request("claude-opus-5-5", "", vec![msg("user", "Fruit?", None), old, msg("user", "Go on", None), signed, msg("user", "More", None)]));
        assert_eq!(body["messages"][1]["content"], json!([{ "type": "text", "text": "Partial ans" }]));
        assert_eq!(body["messages"][3]["content"][0]["signature"], "s1", "signed reasoning still goes back");
        let unsigned = body["messages"].as_array().unwrap().iter().filter_map(|m| m["content"].as_array()).flatten().filter(|b| b["type"] == "thinking" && b["signature"].as_str().unwrap_or("").is_empty()).count();
        assert_eq!(unsigned, 0);
        assert!(betas.contains(&BINDING_BETA));
    }

    #[tokio::test]
    async fn an_openai_reply_whose_connection_closes_part_way_says_so() {
        let body = sse(&[json!({ "type": "response.output_text.delta", "delta": "Half" })]);
        let (base, _) = upstream(SSE_OK, body, Duration::ZERO).await;
        let llm = app("openai-cut").await;
        llm.upsert(Provider { id: "openai-cut".into(), name: "OpenAI".into(), kind: ProviderKind::Openai, base_url: format!("{base}/v1"), ..Default::default() }).await.unwrap();
        crate::secrets::set("llm:openai-cut", "sk-test").unwrap();
        let mut req = request("gpt-4.1", "", vec![msg("user", "Hi", None)]);
        req.provider = "openai-cut".into();
        let events = run(&llm, req).await;
        crate::secrets::clear("llm:openai-cut").unwrap();
        assert_eq!(shape(&events), ["Text(Half)", "Done(\"interrupted\", Half)", "Error(The connection to OpenAI closed before the reply finished.)"]);
    }

    #[tokio::test]
    async fn a_cluster_reply_whose_engine_dies_part_way_says_which_node() {
        // The engine's process dies (its socket closes): no finish_reason, no [DONE].
        let first = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "The answer is" } }] }));
        let (base, _) = upstream(SSE_OK, first, Duration::ZERO).await;
        let llm = cluster_llm("engine-cut", base).await;
        let events = run(&llm, cluster_request("cell2-engine-cut")).await;
        assert_eq!(shape(&events), ["Text(The answer is)", "Done(\"interrupted\", The answer is)", "Error(The connection closed before the reply finished (is cell2 still up?))"]);

        // The same over a chunked response (vLLM's): the cut is a read error, and ends the same way.
        let data = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "Half" } }] }));
        let chunked = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked";
        let (base, _) = upstream(chunked, format!("{:x}\r\n{data}\r\n", data.len()), Duration::ZERO).await;
        let llm = cluster_llm("engine-cut-chunked", base).await;
        let events = run(&llm, cluster_request("cell2-engine-cut-chunked")).await;
        assert_eq!(shape(&events), ["Text(Half)", "Done(\"interrupted\", Half)", "Error(The connection closed before the reply finished (is cell2 still up?))"]);

        // A reply that got its finish_reason is finished, [DONE] or not.
        let ended = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "All of it" }, "finish_reason": "stop" }] }));
        let (base, _) = upstream(SSE_OK, ended, Duration::ZERO).await;
        let llm = cluster_llm("engine-ended", base).await;
        assert_eq!(shape(&run(&llm, cluster_request("cell2-engine-ended")).await), ["Text(All of it)", "Done(\"stop\", All of it)"]);

        // Nothing at all before the close: just the error.
        let (base, _) = upstream(SSE_OK, String::new(), Duration::ZERO).await;
        let llm = cluster_llm("engine-none", base).await;
        assert_eq!(shape(&run(&llm, cluster_request("cell2-engine-none")).await), ["Error(The connection closed before the reply finished (is cell2 still up?))"]);
    }

    #[tokio::test]
    async fn a_model_that_sends_nothing_ends_the_reply_with_the_message() {
        let (base, _) = upstream(SSE_OK, String::new(), Duration::from_secs(30)).await;
        let llm = cluster_llm("idle-none", base).await;
        let events = run(&llm, cluster_request("cell2-idle-none")).await;
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(matches!(&events[0], ChatEvent::Error { message } if message == "The model stopped responding (is cell2 still up?)"), "{events:?}");
    }

    #[tokio::test]
    async fn a_provider_that_just_stopped_answering_is_still_sent_to() {
        let done = format!("data: {}\n\ndata: [DONE]\n\n", json!({ "choices": [{ "delta": { "content": "Back" }, "finish_reason": "stop" }] }));
        let (base, _) = upstream(SSE_OK, done, Duration::ZERO).await;
        let llm = app("flap").await;
        let p = Provider { id: "cell2-vllm-8000".into(), name: "vLLM on cell2".into(), base_url: format!("{base}/v1"), models: vec!["qwen3-8b".into()], ..Default::default() };
        llm.set_discovered(vec![p]);
        llm.set_discovered(vec![]);
        let events = run(&llm, cluster_request("cell2-vllm-8000")).await;
        assert!(matches!(events.last(), Some(ChatEvent::Done { message }) if message.content == "Back"), "no 'Unknown provider' during a flap: {events:?}");
    }

    #[tokio::test]
    async fn an_openai_account_without_reasoning_summaries_gets_its_answer_without_one() {
        let refused = json!({ "error": { "message": "Your organization must be verified to generate reasoning summaries.", "type": "invalid_request_error" } }).to_string();
        let sse = [
            json!({ "type": "response.output_text.delta", "delta": "Hi" }),
            json!({ "type": "response.completed", "response": { "model": "gpt-5.5", "status": "completed", "usage": { "input_tokens": 3, "output_tokens": 1 } } }),
        ]
        .iter()
        .map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap()))
        .collect::<String>();
        let bad = "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\nconnection: close";
        let (base, seen) = upstreams(vec![(bad, refused, Duration::ZERO), (SSE_OK, sse, Duration::ZERO)]).await;
        let llm = app("openai-summary").await;
        llm.upsert(Provider { id: "openai-r2".into(), name: "OpenAI".into(), kind: ProviderKind::Openai, base_url: format!("{base}/v1"), ..Default::default() }).await.unwrap();
        crate::secrets::set("llm:openai-r2", "sk-test").unwrap();
        let mut req = request("gpt-5.5", "", vec![msg("user", "Hi", None)]);
        req.provider = "openai-r2".into();
        req.params = ChatParams { effort: Some("low".into()), temperature: Some(0.3), max_tokens: None };
        let events = run(&llm, req).await;
        crate::secrets::clear("llm:openai-r2").unwrap();
        let bodies: Vec<Value> = seen.lock().unwrap().iter().map(|r| serde_json::from_str(&r[r.find("\r\n\r\n").unwrap() + 4..]).unwrap()).collect();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["reasoning"], json!({ "effort": "low", "summary": "auto" }));
        assert_eq!(bodies[1]["reasoning"], json!({ "effort": "low" }), "asked again without the summary");
        assert!(bodies.iter().all(|b| b.get("temperature").is_none()));
        assert!(events.iter().any(|e| matches!(e, ChatEvent::Notice { text } if text.contains("reasoning summaries"))), "{events:?}");
        assert!(matches!(events.last(), Some(ChatEvent::Done { message }) if message.content == "Hi"), "{events:?}");
    }
}
