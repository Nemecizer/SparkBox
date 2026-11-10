//! The server and the chat client against a fake upstream that speaks the real
//! Anthropic, OpenAI and vLLM-style wire formats. Keys live in the in-memory
//! test key store, never the Keychain.

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use reqwest::header::HeaderMap as ReqHeaders;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::*;
use crate::fake_upstream;
use crate::llm::chat::{ChatEvent, ChatRequest};
use crate::llm::history::{ChatParams, Message};
use crate::llm::{Llm, Provider, ProviderKind};
use crate::vpn::Vpn;

// Tests share the in-memory key store, so they take turns.
static SERIAL: Mutex<()> = Mutex::const_new(());

struct Setup {
    url: String,
    key: String,
    seen: fake_upstream::Seen,
    server: Arc<Server>,
    llm: Arc<Llm>,
    /// Every row the server handed to `on_log` (the Requests table's live feed).
    logged: Arc<StdMutex<Vec<LogEntry>>>,
}

async fn setup(name: &str) -> Setup {
    let (upstream, seen) = fake_upstream::start().await;
    let dir = std::env::temp_dir().join(format!("sparkbox-server-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let vpn = Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let llm = Llm::new(dir.join("providers.json"), vpn);
    let provider = |id: &str, kind, base: String, models: &[&str]| Provider {
        id: id.into(),
        name: id.into(),
        kind,
        base_url: base,
        models: models.iter().map(|m| m.to_string()).collect(),
        ..Default::default()
    };
    llm.upsert(provider("anthropic", ProviderKind::Anthropic, upstream.clone(), &["claude-opus-5-5"])).await.unwrap();
    llm.upsert(provider("openai", ProviderKind::Openai, format!("{upstream}/v1"), &["gpt-test"])).await.unwrap();
    llm.upsert(provider("cell1-vllm", ProviderKind::OpenaiCompatible, format!("{upstream}/v1"), &["meta-llama/Llama-3.3-70B-Instruct"]))
        .await
        .unwrap();
    secrets::set("llm:anthropic", "sk-ant-test").unwrap();
    secrets::clear("llm:openai").unwrap();

    // Keys in the store outlive a test: each starts with none.
    secrets::clear(CLIENTS_ACCOUNT).unwrap();
    let logged: Arc<StdMutex<Vec<LogEntry>>> = Arc::default();
    let feed = logged.clone();
    let server = Server::new(dir.join("server.json"), llm.clone(), move |e| feed.lock().unwrap().push(e.clone()));
    let mut settings = ServerSettings { port: 0, ..Default::default() };
    settings.aliases.insert("local".into(), "cell1-vllm/meta-llama/Llama-3.3-70B-Instruct".into());
    server.save_settings(settings).await.unwrap();
    let view = server.start().await.unwrap();
    let key = server.create_key("tests").await.unwrap();
    Setup { url: view.url.unwrap(), key, seen, server, llm, logged }
}

async fn post(s: &Setup, path: &str, body: Value, headers: &[(&str, &str)]) -> (u16, ReqHeaders, String) {
    let mut req = reqwest::Client::new().post(format!("{}{path}", s.url)).json(&body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    (status, headers, resp.text().await.unwrap())
}

fn bearer(s: &Setup) -> String {
    format!("Bearer {}", s.key)
}

fn data_lines(sse: &str) -> Vec<Value> {
    sse.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter(|d| *d != "[DONE]")
        .map(|d| serde_json::from_str(d).unwrap())
        .collect()
}

#[tokio::test]
async fn requests_need_a_sparkbox_key() {
    let _turn = SERIAL.lock().await;
    let s = setup("auth").await;
    let body = json!({ "model": "demo/echo", "max_tokens": 10, "messages": [] });

    let (status, _, text) = post(&s, "/v1/messages", body.clone(), &[]).await;
    assert_eq!(status, 401);
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["error"]["type"], "authentication_error", "Anthropic-shaped error");

    let (status, _, text) = post(&s, "/v1/chat/completions", body.clone(), &[("authorization", "Bearer sbx-wrong")]).await;
    assert_eq!(status, 401);
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["error"]["type"], "invalid_api_key", "OpenAI-shaped error");

    let (status, _, _) = post(&s, "/v1/messages", body, &[("x-api-key", &s.key)]).await;
    assert_eq!(status, 200, "x-api-key works too");

    s.server.revoke_key("tests").await.unwrap();
    let (status, _, _) = post(&s, "/v1/messages", json!({ "model": "demo/echo" }), &[("x-api-key", &s.key)]).await;
    assert_eq!(status, 401, "revoked keys stop working");
}

/// A name already used is refused: replacing it would stop the app using that key, with nothing
/// to say why. The field starts as "Claude Code", so a second click would have done it.
#[tokio::test]
async fn a_new_key_never_replaces_one_with_the_same_name() {
    let _turn = SERIAL.lock().await;
    let s = setup("duplicate-key").await;
    let before = s.server.view().await.clients;
    for name in ["tests", "  tests  "] {
        let err = s.server.create_key(name).await.unwrap_err();
        assert_eq!(err, "A key named “tests” already exists. Revoke it first, or use another name.");
    }
    let body = json!({ "model": "demo/echo", "messages": [] });
    let (status, _, text) = post(&s, "/v1/chat/completions", body.clone(), &[("authorization", &bearer(&s))]).await;
    assert_eq!(status, 200, "the first key still works: {text}");
    let after = s.server.view().await.clients;
    assert_eq!(after.iter().map(|c| (&c.name, &c.preview)).collect::<Vec<_>>(), before.iter().map(|c| (&c.name, &c.preview)).collect::<Vec<_>>());
    assert_eq!(serde_json::from_str::<Vec<ClientKey>>(&secrets::get(CLIENTS_ACCOUNT).unwrap().unwrap()).unwrap().len(), 1, "nothing was saved");

    // Revoked, the name is free again, and another name was free all along.
    let other = s.server.create_key("another app").await.unwrap();
    assert_ne!(other, s.key);
    s.server.revoke_key("tests").await.unwrap();
    let again = s.server.create_key("tests").await.unwrap();
    let (status, _, _) = post(&s, "/v1/chat/completions", body.clone(), &[("authorization", &format!("Bearer {again}"))]).await;
    assert_eq!(status, 200);
    let (status, _, _) = post(&s, "/v1/chat/completions", body, &[("authorization", &bearer(&s))]).await;
    assert_eq!(status, 401, "the old key stays revoked");
}

/// Failed key attempts show in Requests, in every API and in the model list: one row each, which
/// says whether the key was missing or wrong and holds nothing that was presented.
#[tokio::test]
async fn a_refused_key_is_one_row_with_none_of_the_key_in_it() {
    let _turn = SERIAL.lock().await;
    let s = setup("refused-keys").await;
    let wrong = "sbx-0123456789abcdef-a-key-from-somewhere-else";
    let body = json!({ "model": "demo/echo", "max_tokens": 16, "messages": [{ "role": "user", "content": "hi" }] });
    let list = |headers: &[(&'static str, String)]| {
        let mut req = reqwest::Client::new().get(format!("{}/v1/models", s.url));
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        req.send()
    };
    let (status, _, _) = post(&s, "/v1/chat/completions", body.clone(), &[]).await;
    assert_eq!(status, 401);
    let (status, _, _) = post(&s, "/v1/messages", body.clone(), &[("authorization", &format!("Bearer {wrong}"))]).await;
    assert_eq!(status, 401);
    let (status, _, _) = post(&s, "/v1/responses", body.clone(), &[("x-api-key", wrong)]).await;
    assert_eq!(status, 401);
    assert_eq!(list(&[]).await.unwrap().status().as_u16(), 401);
    assert_eq!(list(&[("authorization", format!("Bearer {wrong}"))]).await.unwrap().status().as_u16(), 401);
    assert_eq!(list(&[("authorization", "Basic d3Jvbmc=".into())]).await.unwrap().status().as_u16(), 401, "not a key at all");

    let rows = s.server.logs().await;
    let shown: Vec<(&str, &str)> = rows.iter().map(|e| (e.client.as_str(), e.api.as_str())).collect();
    assert_eq!(shown, [("(no key)", "chat/completions"), ("(wrong key)", "messages"), ("(wrong key)", "responses"), ("(no key)", "models"), ("(wrong key)", "models"), ("(no key)", "models")]);
    for e in &rows {
        assert_eq!((e.status, e.done, e.error.as_deref(), e.model.as_str(), e.provider.as_str()), (401, true, Some("Missing or unknown SparkBox key"), "", ""), "{e:?}");
        assert!(e.duration_ms.is_some(), "{e:?}");
    }
    let fed = s.logged.lock().unwrap().clone();
    assert_eq!(fed.len(), rows.len(), "each went to the window once");
    // Every field of every row, and every call: no piece of what was presented.
    for text in rows.iter().chain(&fed).map(|e| serde_json::to_string(e).unwrap()) {
        for piece in [wrong, "sbx-", "0123456789", "a-key-from", "d3Jvbmc="] {
            assert!(!text.contains(piece), "{piece} in {text}");
        }
    }

    // A good key makes no refused row, and the self-test's key isn't refused either.
    let (status, _, _) = post(&s, "/v1/chat/completions", body, &[("authorization", &bearer(&s))]).await;
    assert_eq!(status, 200);
    s.server.self_test("demo/echo").await.unwrap();
    assert_eq!(s.server.logs().await.iter().filter(|e| e.status == 401).count(), rows.len());
}

#[tokio::test]
async fn the_demo_model_answers_all_three_apis() {
    let _turn = SERIAL.lock().await;
    let s = setup("demo").await;
    let auth = bearer(&s);
    let user = json!([{ "role": "user", "content": "ping" }]);

    let (_, _, text) = post(&s, "/v1/chat/completions", json!({ "model": "demo/echo", "messages": user }), &[("authorization", &auth)]).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["choices"][0]["message"]["content"].as_str().unwrap().contains("SparkBox demo"));

    let (_, headers, text) =
        post(&s, "/v1/chat/completions", json!({ "model": "demo/echo", "stream": true, "messages": user }), &[("authorization", &auth)]).await;
    assert_eq!(headers["content-type"], "text/event-stream");
    assert!(text.trim_end().ends_with("data: [DONE]"));

    let (_, _, text) =
        post(&s, "/v1/messages", json!({ "model": "demo/echo", "stream": true, "max_tokens": 50, "messages": user }), &[("authorization", &auth)]).await;
    assert!(text.contains("event: message_stop"));

    let (_, _, text) = post(&s, "/v1/responses", json!({ "model": "demo/echo", "input": "ping" }), &[("authorization", &auth)]).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["output"][0]["content"][0]["type"], "output_text");
}

#[tokio::test]
async fn claude_code_requests_pass_through_with_the_real_key() {
    let _turn = SERIAL.lock().await;
    let s = setup("claudecode").await;
    let body = json!({ "model": "anthropic/claude-opus-5-5", "stream": true, "max_tokens": 1000,
                       "messages": [{ "role": "user", "content": "hi" }], "tools": [{ "name": "Read", "input_schema": {} }] });
    let auth = bearer(&s);
    let (status, _, text) =
        post(&s, "/v1/messages", body, &[("authorization", &auth), ("anthropic-version", "2023-06-01"), ("anthropic-beta", "some-beta")]).await;
    assert_eq!(status, 200);
    assert!(text.contains("\"text\":\"Hello\""), "upstream stream relayed unchanged");

    let (path, headers, sent) = s.seen.last();
    assert_eq!(path, "messages");
    assert_eq!(headers["x-api-key"], "sk-ant-test", "real key injected");
    assert!(headers.get("authorization").is_none(), "the SparkBox key never reaches Anthropic");
    assert_eq!(headers["anthropic-version"], "2023-06-01", "client's API version kept");
    assert_eq!(headers["anthropic-beta"], "some-beta");
    assert_eq!(sent["model"], "claude-opus-5-5", "provider prefix removed");
    assert_eq!(sent["tools"][0]["name"], "Read", "tools pass through untouched");
}

#[tokio::test]
async fn openai_style_apps_can_talk_to_claude() {
    let _turn = SERIAL.lock().await;
    let s = setup("translate").await;
    let auth = bearer(&s);
    let body = json!({
        "model": "anthropic/claude-opus-5-5", "stream": true, "stream_options": { "include_usage": true }, "temperature": 0.3,
        "messages": [{ "role": "system", "content": "Be brief." }, { "role": "user", "content": "hi" }]
    });
    let (status, _, text) = post(&s, "/v1/chat/completions", body, &[("authorization", &auth)]).await;
    assert_eq!(status, 200);
    let chunks = data_lines(&text);
    let content: String = chunks.iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
    assert_eq!(content, "Hello there", "thinking stays out of the text");
    assert!(chunks.iter().any(|c| c["choices"][0]["finish_reason"] == "stop"));
    assert_eq!(chunks.last().unwrap()["usage"]["total_tokens"], 17);
    assert!(text.trim_end().ends_with("data: [DONE]"));

    let (_, headers, sent) = s.seen.last();
    assert_eq!(sent["system"], "Be brief.");
    assert!(sent.get("temperature").is_none());
    assert_eq!(sent["fallbacks"], "default");
    assert_eq!(headers["anthropic-beta"], crate::llm::chat::FALLBACK_BETA);
    assert_eq!(headers["anthropic-version"], crate::llm::ANTHROPIC_VERSION);

    let (_, _, text) = post(&s, "/v1/chat/completions", json!({ "model": "anthropic/claude-opus-5-5", "messages": [{ "role": "user", "content": "hi" }] }), &[("authorization", &auth)]).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["choices"][0]["message"]["content"], "Hello there");
    assert_eq!(v["model"], "anthropic/claude-opus-5-5");
}

#[tokio::test]
async fn cluster_engines_aliases_and_bare_names_route_correctly() {
    let _turn = SERIAL.lock().await;
    let s = setup("routing").await;
    let auth = bearer(&s);
    let user = json!([{ "role": "user", "content": "hi" }]);

    for model in ["meta-llama/Llama-3.3-70B-Instruct", "local", "cell1-vllm/meta-llama/Llama-3.3-70B-Instruct"] {
        let (status, _, text) = post(&s, "/v1/chat/completions", json!({ "model": model, "messages": user }), &[("authorization", &auth)]).await;
        assert_eq!(status, 200, "{model}: {text}");
        let (path, headers, sent) = s.seen.last();
        assert_eq!(path, "chat/completions");
        assert_eq!(sent["model"], "meta-llama/Llama-3.3-70B-Instruct", "{model}");
        assert!(headers.get("authorization").is_none(), "no key configured for the engine");
    }

    let (status, _, text) = post(&s, "/v1/chat/completions", json!({ "model": "mystery-model", "messages": user }), &[("authorization", &auth)]).await;
    assert_eq!(status, 404);
    assert!(text.contains("GET /v1/models"));

    let (status, _, text) = post(&s, "/v1/responses", json!({ "model": "openai/gpt-test", "input": "hi" }), &[("authorization", &auth)]).await;
    assert_eq!(status, 502, "OpenAI without a key is refused before anything is sent");
    assert!(text.contains("OpenAI API key"));
}

/// Look-only holds inference here as it holds Chat: every request to a model, the cluster's or
/// a cloud provider's, is answered 503 and logged, and nothing reaches the model. When look-only
/// ends, requests go through again. The server doesn't autostart in look-only either.
#[tokio::test]
async fn look_only_holds_every_request_to_a_model() {
    let _turn = SERIAL.lock().await;
    let s = setup("lookonly").await;
    let on = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = on.clone();
    s.server.hold_when(move || flag.load(Ordering::SeqCst));
    let auth = bearer(&s);
    let user = json!([{ "role": "user", "content": "hi" }]);
    let calls = [
        ("/v1/chat/completions", json!({ "model": "local", "messages": user })),
        ("/v1/chat/completions", json!({ "model": "demo/echo", "messages": user })),
        ("/v1/messages", json!({ "model": "anthropic/claude-opus-5-5", "max_tokens": 16, "messages": user })),
        ("/v1/responses", json!({ "model": "cell1-vllm/meta-llama/Llama-3.3-70B-Instruct", "input": "hi" })),
        ("/v1/completions", json!({ "model": "local", "prompt": "hi" })),
        ("/v1/embeddings", json!({ "model": "local", "input": "hi" })),
    ];
    for (path, body) in &calls {
        let (status, _, text) = post(&s, path, body.clone(), &[("authorization", &auth)]).await;
        assert_eq!(status, 503, "{path}: {text}");
        assert!(text.contains("look-only mode"), "{path}: {text}");
    }
    for path in ["chat/completions", "messages", "responses", "completions", "embeddings"] {
        assert!(s.seen.first(path).is_none(), "{path} reached a model");
    }
    let logs = s.server.logs().await;
    assert_eq!(logs.len(), calls.len());
    assert!(logs.iter().all(|e| e.status == 503 && e.error.as_deref() == Some(HELD)), "{logs:?}");

    on.store(false, Ordering::SeqCst);
    let (status, _, text) = post(&s, "/v1/chat/completions", json!({ "model": "local", "messages": user }), &[("authorization", &auth)]).await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(s.seen.last().0, "chat/completions");

    assert!(autostart_now(true, false));
    assert!(!autostart_now(true, true), "look-only skips the autostart");
    assert!(!autostart_now(false, false) && !autostart_now(false, true));

    // Look-only turning on stops a running server (lib.rs hold hook): nothing answers after.
    let view = s.server.stop_for_hold().await.expect("it was running");
    assert!(!view.running && view.url.is_none());
    for _ in 0..50 {
        if reqwest::Client::new().get(format!("{}/v1/models", s.url)).send().await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reqwest::Client::new().get(format!("{}/v1/models", s.url)).send().await.is_err(), "the port is closed");
    assert!(s.server.stop_for_hold().await.is_none(), "not running: nothing to stop");
}

#[tokio::test]
async fn model_list_shows_usable_models_and_aliases() {
    let _turn = SERIAL.lock().await;
    let s = setup("models").await;
    let resp = reqwest::Client::new().get(format!("{}/v1/models", s.url)).bearer_auth(&s.key).send().await.unwrap();
    let v: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = v["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"anthropic/claude-opus-5-5"));
    assert!(ids.contains(&"cell1-vllm/meta-llama/Llama-3.3-70B-Instruct"));
    assert!(ids.contains(&"demo/echo"));
    assert!(ids.contains(&"local"));
    assert!(!ids.iter().any(|m| m.starts_with("openai/")), "providers missing a key are hidden");

}

/// With Live data the server offers what AI › Providers offers: GET /v1/models (the OpenAI and
/// the Anthropic readers both read it) leaves the demo model out, and a request naming it, by
/// either name, is "model not found" in every API. Demo still lists and answers it.
#[tokio::test]
async fn with_live_data_the_demo_model_is_neither_listed_nor_answered() {
    let _turn = SERIAL.lock().await;
    let s = setup("live-demo").await;
    let live = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = live.clone();
    s.llm.set_source(move || !flag.load(Ordering::SeqCst));
    let ids = || async {
        let v: Value = reqwest::Client::new().get(format!("{}/v1/models", s.url)).bearer_auth(&s.key).send().await.unwrap().json().await.unwrap();
        v["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect::<Vec<_>>()
    };
    assert!(ids().await.contains(&"demo/echo".to_string()), "Demo lists it");

    live.store(true, Ordering::SeqCst);
    let listed = ids().await;
    assert!(!listed.iter().any(|m| m.starts_with("demo/") || m == "echo"), "{listed:?}");
    for m in ["anthropic/claude-opus-5-5", "cell1-vllm/meta-llama/Llama-3.3-70B-Instruct", "local"] {
        assert!(listed.contains(&m.to_string()), "{m}: {listed:?}");
    }
    let auth = bearer(&s);
    let user = json!([{ "role": "user", "content": "ping" }]);
    for model in ["demo/echo", "echo"] {
        let calls = [
            ("/v1/chat/completions", json!({ "model": model, "messages": user })),
            ("/v1/responses", json!({ "model": model, "input": "ping" })),
            ("/v1/messages", json!({ "model": model, "max_tokens": 16, "messages": user })),
            ("/v1/messages/count_tokens", json!({ "model": model, "messages": user })),
        ];
        for (path, body) in calls {
            let (status, _, text) = post(&s, path, body, &[("authorization", &auth)]).await;
            assert_eq!(status, 404, "{model} {path}: {text}");
            assert!(text.contains(&format!("No provider serves model '{model}'")), "{model} {path}: {text}");
            if path.starts_with("/v1/messages") {
                assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["error"]["type"], "not_found_error", "{path}");
            }
        }
    }
    assert!(s.server.logs().await.iter().all(|e| e.provider.is_empty() && e.status == 404), "nothing went to the demo model");
    // An alias can't point at it either.
    let mut settings = s.server.view().await.settings;
    settings.aliases.insert("quick".into(), "demo/echo".into());
    assert!(s.server.save_settings(settings).await.unwrap_err().contains("which no provider offers"));

    // Back in Demo, it answers again.
    live.store(false, Ordering::SeqCst);
    let (status, _, text) = post(&s, "/v1/chat/completions", json!({ "model": "demo/echo", "messages": user }), &[("authorization", &auth)]).await;
    assert_eq!(status, 200, "{text}");
}

#[tokio::test]
async fn every_request_is_logged() {
    let _turn = SERIAL.lock().await;
    let s = setup("logs").await;
    post(&s, "/v1/chat/completions", json!({ "model": "demo/echo", "messages": [] }), &[("authorization", &bearer(&s))]).await;
    post(&s, "/v1/chat/completions", json!({ "model": "nope", "messages": [] }), &[("authorization", &bearer(&s))]).await;
    let logs = s.server.logs().await;
    assert_eq!(logs.len(), 2);
    assert_eq!((logs[0].client.as_str(), logs[0].provider.as_str(), logs[0].status), ("tests", "demo", 200));
    assert_eq!(logs[1].status, 404);
    assert!(logs[1].error.is_some());
}

/// Waits for the log entry of the latest request to be completed with its usage.
async fn finished(s: &Setup) -> LogEntry {
    for _ in 0..100 {
        if let Some(e) = s.server.logs().await.into_iter().last().filter(|e| e.done) {
            return e;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the request never finished: {:?}", s.server.logs().await);
}

#[tokio::test]
async fn the_log_records_tokens_time_and_speed() {
    let _turn = SERIAL.lock().await;
    let s = setup("usage").await;
    let auth = bearer(&s);
    // A streamed cluster reply: usage from its last chunk.
    let body = json!({ "model": "local", "stream": true, "messages": [{ "role": "user", "content": "hi" }] });
    let (status, _, _) = post(&s, "/v1/chat/completions", body, &[("authorization", &auth)]).await;
    assert_eq!(status, 200);
    let e = finished(&s).await;
    assert_eq!((e.input_tokens, e.output_tokens), (Some(2), Some(2)));
    assert!(e.duration_ms.is_some());
    assert_eq!(e.tokens_per_sec, None, "two chunks in a burst: no speed to report");
    // …and it counts as traffic on that engine, for its Endpoints card.
    let t = s.llm.take_traffic("cell1-vllm");
    assert_eq!((t.finished, t.output_tokens, t.active), (1, 2, 0));
    // Claude, translated to Chat Completions and not streamed.
    let body = json!({ "model": "anthropic/claude-opus-5-5", "messages": [{ "role": "user", "content": "hi" }] });
    post(&s, "/v1/chat/completions", body, &[("authorization", &auth)]).await;
    let e = finished(&s).await;
    assert_eq!((e.input_tokens, e.output_tokens), (Some(12), Some(5)));
    // The demo model counts four characters to a token.
    let body = json!({ "model": "demo/echo", "stream": true, "max_tokens": 50, "messages": [{ "role": "user", "content": "ping" }] });
    post(&s, "/v1/messages", body, &[("authorization", &auth)]).await;
    let e = finished(&s).await;
    let text = crate::llm::chat::demo_reply("ping");
    assert_eq!(e.output_tokens, Some(crate::llm::throughput::demo_tokens(&text)));
    assert!(e.tokens_per_sec.is_some_and(|r| r < 1000.0), "{:?}", e.tokens_per_sec);
}

async fn raw(s: &Setup, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut req = reqwest::Client::new().post(format!("{}{path}", s.url)).body(body.to_string());
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

#[tokio::test]
async fn foreign_hosts_origins_and_form_posts_are_refused() {
    let _turn = SERIAL.lock().await;
    let s = setup("guard").await;
    let auth = bearer(&s);
    let ok = json!({ "model": "demo/echo", "messages": [] }).to_string();
    let json_ct = ("content-type", "application/json");

    // DNS rebinding: a page on evil.example resolving to 127.0.0.1 still sends its own Host.
    let (status, text) = raw(&s, "/v1/chat/completions", &[("authorization", &auth), json_ct, ("host", "evil.example:8484")], &ok).await;
    assert_eq!(status, 403);
    assert!(text.contains("Host"), "{text}");
    let port = s.url.rsplit(':').next().unwrap().to_string();
    for host in [format!("localhost:{port}"), format!("127.0.0.1:{port}")] {
        let (status, _) = raw(&s, "/v1/chat/completions", &[("authorization", &auth), json_ct, ("host", &host)], &ok).await;
        assert_eq!(status, 200, "{host}");
    }
    let (status, _) = raw(&s, "/v1/chat/completions", &[("authorization", &auth), json_ct, ("host", &format!("localhost:{}", port.parse::<u16>().unwrap() + 1))], &ok).await;
    assert_eq!(status, 403, "the right name on the wrong port");

    // A browser page: it always sends Origin.
    let (status, text) = raw(&s, "/v1/messages", &[("authorization", &auth), json_ct, ("origin", "https://evil.example")], &ok).await;
    assert_eq!(status, 403);
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["type"], "error", "Anthropic-shaped for /v1/messages");

    // A "simple" cross-site form post skips the CORS preflight; it isn't JSON.
    let (status, text) = raw(&s, "/v1/chat/completions", &[("authorization", &auth), ("content-type", "text/plain")], &ok).await;
    assert_eq!(status, 403);
    assert!(text.contains("application/json"));
    let (status, _) = raw(&s, "/v1/chat/completions", &[("authorization", &auth), ("content-type", "application/json; charset=utf-8")], &ok).await;
    assert_eq!(status, 200);

    // Every refusal is in the log.
    let refused: Vec<LogEntry> = s.server.logs().await.into_iter().filter(|e| e.status == 403).collect();
    assert_eq!(refused.len(), 4);
    assert!(refused.iter().all(|e| e.client == "(refused)" && e.error.is_some()));

    // An origin you allowed gets through.
    let mut settings = s.server.view().await.settings;
    settings.allowed_origins = vec!["http://localhost:3000".into()];
    s.server.save_settings(settings).await.unwrap();
    let (status, _) = raw(&s, "/v1/chat/completions", &[("authorization", &auth), json_ct, ("origin", "http://localhost:3000")], &ok).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn aliases_must_point_at_a_real_model() {
    let _turn = SERIAL.lock().await;
    let s = setup("aliases").await;
    let mut settings = s.server.view().await.settings;
    settings.aliases.insert("fast".into(), "cell1-vllm/no-such-model".into());
    let err = s.server.save_settings(settings.clone()).await.unwrap_err();
    assert!(err.contains("which no provider offers"), "{err}");
    settings.aliases.remove("fast");
    settings.aliases.insert("bad/name".into(), "demo/echo".into());
    assert!(s.server.save_settings(settings.clone()).await.unwrap_err().contains("no /"));
    settings.aliases.remove("bad/name");
    settings.aliases.insert("claude".into(), "anthropic/claude-opus-5-5".into());
    s.server.save_settings(settings).await.unwrap();
    assert!(s.server.view().await.settings.aliases.contains_key("claude"));
}

/// A saved alias whose model has gone (a load comes and goes) doesn't block the other settings,
/// and stays saved; a new or changed one is checked as ever, and a request for the gone one gets
/// the usual 404 until the model is back.
#[tokio::test]
async fn a_saved_alias_whose_model_has_gone_doesnt_block_saving() {
    let _turn = SERIAL.lock().await;
    let s = setup("gone-alias").await;
    let mut settings = s.server.view().await.settings;
    settings.aliases.insert("fast".into(), "cell1-vllm/meta-llama/Llama-3.3-70B-Instruct".into());
    s.server.save_settings(settings.clone()).await.unwrap();

    // The load goes: nothing offers either alias's model any more.
    s.llm.remove("cell1-vllm").await.unwrap();
    settings.autostart = true;
    let view = s.server.save_settings(settings.clone()).await.expect("another setting still saves");
    assert!(view.settings.autostart);
    assert_eq!(view.settings.aliases, settings.aliases, "and the aliases stay");
    let (status, _, text) = post(&s, "/v1/chat/completions", json!({ "model": "fast", "messages": [] }), &[("authorization", &bearer(&s))]).await;
    assert_eq!(status, 404);
    assert!(text.contains("No provider serves model 'fast'"), "{text}");

    // Added, or pointed at something else: checked.
    let mut added = settings.clone();
    added.aliases.insert("new".into(), "cell1-vllm/meta-llama/Llama-3.3-70B-Instruct".into());
    assert!(s.server.save_settings(added).await.unwrap_err().contains("The alias “new” points at"));
    let mut changed = settings.clone();
    changed.aliases.insert("fast".into(), "cell1-vllm/no-such-model".into());
    assert!(s.server.save_settings(changed).await.unwrap_err().contains("The alias “fast” points at"));
    let mut renamed = settings.clone();
    let target = renamed.aliases.remove("fast").unwrap();
    renamed.aliases.insert("quick".into(), target);
    assert!(s.server.save_settings(renamed).await.unwrap_err().contains("The alias “quick” points at"), "a new name is a new alias");
    // Removing the gone one is always fine, and it can't come back unchecked after that.
    settings.aliases.remove("fast");
    let target = "cell1-vllm/meta-llama/Llama-3.3-70B-Instruct".to_string();
    s.server.save_settings(settings.clone()).await.unwrap();
    settings.aliases.insert("fast".into(), target);
    assert!(s.server.save_settings(settings).await.is_err());
}

#[tokio::test]
async fn embeddings_and_completions_pass_through_to_engines() {
    let _turn = SERIAL.lock().await;
    let s = setup("passthrough").await;
    let auth = bearer(&s);
    let (status, _, text) = post(&s, "/v1/embeddings", json!({ "model": "local", "input": "hello" }), &[("authorization", &auth)]).await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["data"][0]["embedding"][1], -0.5);
    let (path, _, sent) = s.seen.last();
    assert_eq!((path.as_str(), sent["model"].as_str()), ("embeddings", Some("meta-llama/Llama-3.3-70B-Instruct")));
    let (status, _, text) = post(&s, "/v1/completions", json!({ "model": "local", "prompt": "hello" }), &[("authorization", &auth)]).await;
    assert_eq!(status, 200);
    assert!(text.contains("\" world\""));
    let (status, _, text) = post(&s, "/v1/embeddings", json!({ "model": "anthropic/claude-opus-5-5", "input": "x" }), &[("authorization", &auth)]).await;
    assert_eq!(status, 400, "{text}");
    let e = finished(&s).await;
    assert_eq!(e.status, 400);
}

#[tokio::test]
async fn self_test_exercises_all_three_apis() {
    let _turn = SERIAL.lock().await;
    let s = setup("selftest").await;
    let results = s.server.self_test("demo/echo").await.unwrap();
    let apis: Vec<&str> = results.iter().map(|r| r.api.as_str()).collect();
    assert_eq!(apis, ["chat/completions", "responses", "messages"]);
    assert!(results.iter().all(|r| r.ok && r.detail.contains("SparkBox demo")), "{results:?}");
    assert!(s.server.logs().await.iter().all(|l| l.client == "SparkBox self-test"));

    // Claude through Chat Completions (converted) and Messages (passed through) works;
    // the Responses API is refused with a clear reason.
    let results = s.server.self_test("anthropic/claude-opus-5-5").await.unwrap();
    assert!(results[0].ok && results[0].detail == "Hello there", "{results:?}");
    assert!(!results[1].ok && results[1].detail.contains("Responses API"));
    assert!(results[2].ok && results[2].detail == "Hello there");
}

// --- The chat client (used by the Chat window) against the same fake upstream.

async fn chat(llm: &Arc<Llm>, provider: &str, model: &str, history: Vec<Message>) -> (Vec<ChatEvent>, Message) {
    let events = StdMutex::new(Vec::new());
    let emit = |e: ChatEvent| events.lock().unwrap().push(e);
    let (_stop, rx) = oneshot::channel();
    let req = ChatRequest {
        request_id: "r1".into(),
        provider: provider.into(),
        model: model.into(),
        system: "Be brief.".into(),
        params: ChatParams { effort: Some("high".into()), temperature: Some(0.5), max_tokens: None },
        messages: history,
    };
    llm.chat(req, &emit, rx).await;
    let events = events.into_inner().unwrap();
    let done = events.iter().find_map(|e| match e {
        ChatEvent::Done { message } => Some(message.clone()),
        _ => None,
    });
    let done = done.unwrap_or_else(|| panic!("no Done event: {events:?}"));
    (events, done)
}

fn user(text: &str) -> Message {
    Message { role: "user".into(), content: text.into(), thinking: None, anthropic_blocks: None, provider: None, model: None,
              stop_reason: None, stats: None, created_at: 0, id: None, parent_id: None }
}

#[tokio::test]
async fn chat_with_claude_keeps_its_blocks_for_the_next_turn() {
    let _turn = SERIAL.lock().await;
    let s = setup("chat-claude").await;
    let (events, msg) = chat(&s.llm, "anthropic", "claude-opus-5-5", vec![user("hi")]).await;
    assert!(events.iter().any(|e| matches!(e, ChatEvent::Thinking { text } if text == "Plan.")));
    assert_eq!(msg.content, "Hello there");
    assert_eq!(msg.thinking.as_deref(), Some("Plan."));
    let blocks = msg.anthropic_blocks.clone().unwrap();
    assert_eq!(blocks[0]["signature"], "sig123", "signed reasoning kept for replay");
    assert_eq!(blocks[1]["text"], "Hello there");
    let stats = msg.stats.clone().unwrap();
    assert_eq!((stats.input_tokens, stats.output_tokens), (Some(12), Some(5)));

    let (_, _, sent) = s.seen.last();
    assert_eq!(sent["thinking"], json!({ "type": "adaptive", "display": "summarized" }));
    assert_eq!(sent["output_config"]["effort"], "high");
    assert!(sent.get("temperature").is_none(), "never sent to Claude");
    assert_eq!(sent["system"], "Be brief.");

    // Second turn: the first reply goes back exactly as received.
    chat(&s.llm, "anthropic", "claude-opus-5-5", vec![user("hi"), msg, user("more")]).await;
    let (_, _, sent) = s.seen.last();
    assert_eq!(sent["messages"][1]["content"], blocks);
}

#[tokio::test]
async fn chat_with_openai_uses_the_responses_api() {
    let _turn = SERIAL.lock().await;
    let s = setup("chat-openai").await;
    secrets::set("llm:openai", "sk-openai-test").unwrap();
    let (_, msg) = chat(&s.llm, "openai", "gpt-test", vec![user("hi")]).await;
    assert_eq!(msg.content, "Hi from OpenAI");
    assert_eq!(msg.stats.unwrap().output_tokens, Some(4));
    let (path, headers, sent) = s.seen.last();
    assert_eq!(path, "responses");
    assert_eq!(headers["authorization"], "Bearer sk-openai-test");
    assert_eq!(sent["instructions"], "Be brief.");
    assert_eq!(sent["temperature"], 0.5);
    secrets::clear("llm:openai").unwrap();
}

#[tokio::test]
async fn chat_with_a_cluster_engine_streams_reasoning_and_text() {
    let _turn = SERIAL.lock().await;
    let s = setup("chat-local").await;
    let (_, msg) = chat(&s.llm, "cell1-vllm", "meta-llama/Llama-3.3-70B-Instruct", vec![user("hi")]).await;
    assert_eq!(msg.content, "Local answer");
    assert_eq!(msg.thinking.as_deref(), Some("think"));
    assert_eq!(msg.stop_reason.as_deref(), Some("stop"));
    let (_, _, sent) = s.seen.last();
    assert_eq!(sent["messages"][0], json!({ "role": "system", "content": "Be brief." }));
    assert_eq!(sent["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn chat_errors_are_explained() {
    let _turn = SERIAL.lock().await;
    let s = setup("chat-errors").await;
    secrets::clear("llm:anthropic").unwrap();
    let events = StdMutex::new(Vec::new());
    let (_stop, rx) = oneshot::channel();
    let req = ChatRequest { request_id: "x".into(), provider: "anthropic".into(), model: "claude-opus-5-5".into(),
                            system: String::new(), params: ChatParams::default(), messages: vec![user("hi")] };
    s.llm.chat(req, &|e| events.lock().unwrap().push(e), rx).await;
    let events = events.into_inner().unwrap();
    assert!(matches!(&events[0], ChatEvent::Error { message } if message.contains("Anthropic API key")));
}
