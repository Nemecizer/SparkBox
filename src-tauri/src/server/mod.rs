//! SparkBox Server: one local endpoint (like LM Studio's) for every model SparkBox
//! can reach — your cluster, OpenAI, Anthropic and the demo — speaking the
//! OpenAI Chat Completions, OpenAI Responses (Codex) and Anthropic Messages
//! (Claude Code) APIs. Apps authenticate with SparkBox-issued keys; your real
//! provider keys never leave SparkBox.
//!
//! It only answers this Mac: it listens on 127.0.0.1, and every request must
//! name 127.0.0.1 or localhost as its Host (so a web page can't reach it by
//! DNS rebinding, the hole behind Ollama's CVE-2024-28224), carry no Origin
//! unless you allowed it, and POST JSON (so a page can't send a "simple" form
//! request without a CORS preflight). Anything else is refused with 403 and
//! logged.
//!
//! Look-only mode holds inference, as it holds Chat: the server doesn't start with
//! SparkBox (`autostart_now`), turning look-only on stops it (lib.rs), and every request
//! to a model it gets meanwhile is answered 503 (`HELD`).

pub mod translate;

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::llm::catalog;
use crate::llm::chat::{demo_cluster_reply, FALLBACK_BETA};
use crate::llm::sse::SseParser;
use crate::llm::throughput::{demo_tokens, Meter};
use crate::llm::{Llm, Provider, ProviderKind, ANTHROPIC_VERSION};
use crate::secrets;
use translate::{Api, ChunkWriter};

const CLIENTS_ACCOUNT: &str = "server:clients";
const MAX_LOG: usize = 300;
/// A refused key's row in the log (the 401's error says what to do about it).
const KEY_REFUSED: &str = "Missing or unknown SparkBox key";

/// The answer to every request to a model while look-only mode is on.
pub const HELD: &str = "SparkBox is in look-only mode: the API server doesn't send requests to models. Turn it off in SparkBox › Settings › Security & privacy.";

/// Whether the server starts when SparkBox does: its setting, unless look-only mode holds it.
pub fn autostart_now(autostart: bool, look_only: bool) -> bool {
    autostart && !look_only
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
    pub port: u16,
    /// Every request must carry a SparkBox key (Authorization: Bearer … or x-api-key).
    pub require_key: bool,
    /// Start the server when SparkBox starts.
    pub autostart: bool,
    /// Short names for "<provider>/<model>", e.g. "local" → "cell1-vllm-8000/llama-3.3-70b".
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    /// Web origins allowed to call the server (none by default), e.g. "http://localhost:3000".
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

impl Default for ServerSettings {
    fn default() -> Self {
        ServerSettings { port: 8484, require_key: true, autostart: false, aliases: BTreeMap::new(), allowed_origins: Vec::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientKey {
    name: String,
    key: String,
    created_at: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientKeyView {
    pub name: String,
    pub created_at: u64,
    /// First characters only; the full key is shown once, when created.
    pub preview: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub settings: ServerSettings,
    pub running: bool,
    pub url: Option<String>,
    pub error: Option<String>,
    pub clients: Vec<ClientKeyView>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// The same id comes again with the usage once the reply has finished.
    pub id: u64,
    pub ts_ms: u64,
    pub client: String,
    pub api: String,
    pub model: String,
    pub provider: String,
    pub status: u16,
    /// Time until the response started (streams keep going after this).
    pub ms: u64,
    pub stream: bool,
    pub error: Option<String>,
    /// Filled in when the reply ends: tokens in and out, the whole time, and the speed (llm::throughput).
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub duration_ms: Option<u64>,
    pub tokens_per_sec: Option<f64>,
    pub done: bool,
}

struct Running {
    _shutdown: oneshot::Sender<()>,
    url: String,
}

struct Inner {
    settings: ServerSettings,
    running: Option<Running>,
    error: Option<String>,
    logs: VecDeque<LogEntry>,
}

/// Why a request's key wasn't accepted. Nothing here holds any of what it presented.
#[derive(Clone, Copy, Debug, PartialEq)]
enum KeyRefused {
    Missing,
    Wrong,
}

impl KeyRefused {
    /// What the request log calls the client.
    fn client(self) -> &'static str {
        match self {
            KeyRefused::Missing => "(no key)",
            KeyRefused::Wrong => "(wrong key)",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfTestResult {
    pub api: String,
    pub ok: bool,
    pub status: u16,
    pub ms: u64,
    /// The model's reply, or what went wrong.
    pub detail: String,
}

/// Name the self-test's requests carry in the request log.
const SELF_TEST_CLIENT: &str = "SparkBox self-test";

pub struct Server {
    llm: Arc<Llm>,
    /// Lets the Server window's self-test call the loopback API; lives only in memory.
    self_test_key: String,
    path: PathBuf,
    inner: Mutex<Inner>,
    clients: RwLock<Vec<ClientKey>>,
    on_log: Box<dyn Fn(&LogEntry) + Send + Sync>,
    /// The port really bound (the setting may say 0 in tests), for the Host check.
    bound: AtomicU16,
    /// Allowed origins, readable without the async lock.
    origins: StdMutex<Vec<String>>,
    next_id: AtomicU64,
    /// Look-only mode is on (`admin::look_only`; tests set their own): requests to models get `HELD`.
    held: RwLock<Arc<dyn Fn() -> bool + Send + Sync>>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn new_key() -> Result<String, String> {
    let mut buf = [0u8; 20];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    Ok(format!("sbx-{}", buf.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// An error in the shape the calling API's SDKs expect.
fn api_error(api: Option<Api>, status: StatusCode, message: &str) -> Response {
    let body = if api == Some(Api::Messages) {
        let kind = match status {
            StatusCode::UNAUTHORIZED => "authentication_error",
            StatusCode::NOT_FOUND => "not_found_error",
            StatusCode::BAD_REQUEST => "invalid_request_error",
            _ => "api_error",
        };
        json!({ "type": "error", "error": { "type": kind, "message": message } })
    } else {
        let kind = if status == StatusCode::UNAUTHORIZED { "invalid_api_key" } else { "invalid_request_error" };
        json!({ "error": { "message": message, "type": kind, "code": kind } })
    };
    (status, Json(body)).into_response()
}

fn sse_response(rx: mpsc::UnboundedReceiver<String>) -> Response {
    let stream = UnboundedReceiverStream::new(rx).map(|s| Ok::<_, Infallible>(Bytes::from(s)));
    let mut resp = Response::new(Body::from_stream(stream));
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    resp
}

impl Server {
    pub fn new(path: PathBuf, llm: Arc<Llm>, on_log: impl Fn(&LogEntry) + Send + Sync + 'static) -> Arc<Server> {
        let settings: ServerSettings = crate::util::load_json_or_default(&path);
        let origins = StdMutex::new(settings.allowed_origins.clone());
        let clients = secrets::get(CLIENTS_ACCOUNT)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Arc::new(Server {
            llm,
            self_test_key: new_key().expect("random key"),
            path,
            inner: Mutex::new(Inner { settings, running: None, error: None, logs: VecDeque::new() }),
            clients: RwLock::new(clients),
            on_log: Box::new(on_log),
            bound: AtomicU16::new(0),
            origins,
            next_id: AtomicU64::new(1),
            held: RwLock::new(Arc::new(crate::admin::look_only)),
        })
    }

    /// What holds requests to models (look-only mode by default).
    #[cfg(test)]
    pub fn hold_when(&self, held: impl Fn() -> bool + Send + Sync + 'static) {
        *self.held.write().unwrap() = Arc::new(held);
    }

    fn held(&self) -> bool {
        let held = self.held.read().unwrap().clone();
        held()
    }

    pub async fn autostart(self: &Arc<Self>) -> bool {
        self.inner.lock().await.settings.autostart
    }

    pub async fn view(&self) -> ServerView {
        let inner = self.inner.lock().await;
        let clients = self
            .clients
            .read()
            .unwrap()
            .iter()
            .map(|c| ClientKeyView { name: c.name.clone(), created_at: c.created_at, preview: format!("{}…", &c.key[..10]) })
            .collect();
        ServerView {
            settings: inner.settings.clone(),
            running: inner.running.is_some(),
            url: inner.running.as_ref().map(|r| r.url.clone()),
            error: inner.error.clone(),
            clients,
        }
    }

    pub async fn logs(&self) -> Vec<LogEntry> {
        self.inner.lock().await.logs.iter().cloned().collect()
    }

    /// The providers apps can use: the ones answering, as AI › Providers lists them, so with Live
    /// data the demo model (and any simulated cluster model) is neither listed nor answered.
    async fn offered(&self) -> Vec<Provider> {
        self.llm.views().await.into_iter().filter(|v| !v.not_answering).map(|v| v.provider).collect()
    }

    /// Every new or changed alias must point at a model a provider really offers. One already
    /// `saved` isn't asked again: its model can leave for a while (a load comes and goes), and that
    /// shouldn't stop every other setting from saving. Until it's back a request for it gets a 404.
    pub async fn check_aliases(&self, aliases: &BTreeMap<String, String>, saved: &BTreeMap<String, String>) -> Result<(), String> {
        let providers = self.offered().await;
        for (alias, target) in aliases.iter().filter(|(a, t)| saved.get(*a) != Some(*t)) {
            if alias.is_empty() || alias.contains('/') || !alias.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c)) {
                return Err(format!("The alias “{alias}” may use letters, digits and . _ : - only (no /)."));
            }
            let known = target.split_once('/').is_some_and(|(pid, model)| {
                providers.iter().any(|p| p.id == pid && (p.models.iter().any(|m| m == model) || catalog::visible_models(p).iter().any(|m| m == model)))
            });
            if !known {
                return Err(format!("The alias “{alias}” points at {target}, which no provider offers. Pick a model from the list."));
            }
        }
        Ok(())
    }

    pub async fn save_settings(self: &Arc<Self>, settings: ServerSettings) -> Result<ServerView, String> {
        let saved = self.inner.lock().await.settings.aliases.clone();
        self.check_aliases(&settings.aliases, &saved).await?;
        if let Some(bad) = settings.allowed_origins.iter().find(|o| url::Url::parse(o).map(|u| u.origin().ascii_serialization() != o.trim_end_matches('/')).unwrap_or(true)) {
            return Err(format!("“{bad}” isn't an origin like http://localhost:3000 (scheme, host and port only)."));
        }
        let restart = {
            let mut inner = self.inner.lock().await;
            let restart = inner.running.is_some() && inner.settings.port != settings.port;
            inner.settings = settings;
            *self.origins.lock().unwrap() = inner.settings.allowed_origins.clone();
            crate::util::save_json(&self.path, &inner.settings)?;
            restart
        };
        if restart {
            self.stop().await;
            self.start().await?;
        }
        Ok(self.view().await)
    }

    pub async fn start(self: &Arc<Self>) -> Result<ServerView, String> {
        let mut inner = self.inner.lock().await;
        if inner.running.is_some() {
            drop(inner);
            return Ok(self.view().await);
        }
        let port = inner.settings.port;
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::AddrInUse {
                    format!("Port {port} is already used by another app. Pick a different port.")
                } else {
                    format!("Couldn't start on port {port}: {e}")
                };
                inner.error = Some(msg.clone());
                return Err(msg);
            }
        };
        let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        self.bound.store(bound, Ordering::SeqCst);
        let url = format!("http://127.0.0.1:{bound}");
        let (tx, rx) = oneshot::channel::<()>();
        let app = self.router();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).with_graceful_shutdown(async { let _ = rx.await; }).await;
        });
        inner.running = Some(Running { _shutdown: tx, url });
        inner.error = None;
        drop(inner);
        Ok(self.view().await)
    }

    pub async fn stop(&self) -> ServerView {
        // Dropping the sender triggers the graceful shutdown.
        self.inner.lock().await.running = None;
        self.view().await
    }

    /// Look-only just came on (lib.rs hold hook): stops the server if it runs, and returns its
    /// view then, or None when it wasn't running.
    pub async fn stop_for_hold(&self) -> Option<ServerView> {
        self.inner.lock().await.running.as_ref()?;
        Some(self.stop().await)
    }

    pub async fn create_key(&self, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Give the key a name, e.g. the app that will use it.".into());
        }
        let mut clients = self.clients.read().unwrap().clone();
        // Never replaces one: the app using it would stop working with nothing to say why.
        if clients.iter().any(|c| c.name == name) {
            return Err(format!("A key named “{name}” already exists. Revoke it first, or use another name."));
        }
        let key = new_key()?;
        clients.push(ClientKey { name: name.into(), key: key.clone(), created_at: now_secs() });
        secrets::set(CLIENTS_ACCOUNT, &serde_json::to_string(&clients).map_err(|e| e.to_string())?)?;
        *self.clients.write().unwrap() = clients;
        Ok(key)
    }

    pub async fn revoke_key(&self, name: &str) -> Result<(), String> {
        let mut clients = self.clients.read().unwrap().clone();
        clients.retain(|c| c.name != name);
        secrets::set(CLIENTS_ACCOUNT, &serde_json::to_string(&clients).map_err(|e| e.to_string())?)?;
        *self.clients.write().unwrap() = clients;
        Ok(())
    }

    fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/", get(root))
            .route("/v1/models", get(models))
            .route("/v1/chat/completions", post(|s, h, b| handle(s, h, b, Api::ChatCompletions, "chat/completions")))
            .route("/v1/responses", post(|s, h, b| handle(s, h, b, Api::Responses, "responses")))
            .route("/v1/messages", post(|s, h, b| handle(s, h, b, Api::Messages, "messages")))
            .route("/v1/messages/count_tokens", post(|s, h, b| handle(s, h, b, Api::Messages, "messages/count_tokens")))
            .route("/v1/completions", post(|s, h, b| handle(s, h, b, Api::Completions, "completions")))
            .route("/v1/embeddings", post(|s, h, b| handle(s, h, b, Api::Embeddings, "embeddings")))
            .layer(middleware::from_fn_with_state(self.clone(), guard))
            // Agents send whole repositories' worth of context.
            .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
            .with_state(self.clone())
    }

    /// Why a request is refused before anything else looks at it, or None to let it in.
    fn refusal(&self, method: &Method, headers: &HeaderMap) -> Option<String> {
        let port = self.bound.load(Ordering::SeqCst);
        let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
        let ok_host = [format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")].iter().any(|h| h.eq_ignore_ascii_case(host));
        if !ok_host {
            return Some(format!("Host “{host}” refused: SparkBox Server only answers 127.0.0.1:{port} or localhost:{port}."));
        }
        if let Some(origin) = headers.get(header::ORIGIN) {
            let origin = origin.to_str().unwrap_or("?");
            if !self.origins.lock().unwrap().iter().any(|o| o.trim_end_matches('/').eq_ignore_ascii_case(origin)) {
                return Some(format!("Origin {origin} refused: web pages can't use SparkBox Server unless you allow their origin."));
            }
        }
        if method == Method::POST {
            let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
            let mime = ct.split(';').next().unwrap_or("").trim();
            if !mime.eq_ignore_ascii_case("application/json") {
                return Some(format!("Content-Type “{ct}” refused: send JSON (application/json)."));
            }
        }
        None
    }

    fn new_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Fills in a logged request's usage once its reply has finished.
    async fn finish_log(&self, id: u64, usage: Usage) {
        let mut inner = self.inner.lock().await;
        let Some(e) = inner.logs.iter_mut().find(|e| e.id == id) else { return };
        e.input_tokens = usage.input.or(e.input_tokens);
        e.output_tokens = usage.output.or(e.output_tokens);
        e.duration_ms = Some(usage.started.elapsed().as_millis() as u64);
        e.tokens_per_sec = e.output_tokens.and_then(|n| usage.meter.rate(n));
        e.done = true;
        let out = e.clone();
        drop(inner);
        (self.on_log)(&out);
    }

    /// Which client is calling, or why its key wasn't accepted.
    async fn authenticate(&self, headers: &HeaderMap) -> Result<String, KeyRefused> {
        if !self.inner.lock().await.settings.require_key {
            return Ok("(no key required)".into());
        }
        let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
        let x_key = headers.get("x-api-key").and_then(|v| v.to_str().ok());
        let presented = bearer.or(x_key).ok_or(KeyRefused::Missing)?.trim();
        if same(&self.self_test_key, presented) {
            return Ok(SELF_TEST_CLIENT.into());
        }
        self.clients.read().unwrap().iter().find(|c| same(&c.key, presented)).map(|c| c.name.clone()).ok_or(KeyRefused::Wrong)
    }

    /// A refused key's row in the log. It says which of the two it was, and holds nothing the
    /// request presented.
    async fn log_refused(&self, refused: KeyRefused, api: &str, started: Instant) {
        let ms = started.elapsed().as_millis() as u64;
        self.log(LogEntry {
            id: self.new_id(),
            ts_ms: now_ms(),
            client: refused.client().into(),
            api: api.into(),
            status: StatusCode::UNAUTHORIZED.as_u16(),
            ms,
            error: Some(KEY_REFUSED.into()),
            duration_ms: Some(ms),
            done: true,
            ..Default::default()
        })
        .await;
    }

    /// "provider/model", an alias, or a bare model name some provider lists.
    async fn resolve(&self, model: &str) -> Result<(Provider, String), String> {
        let target = self.inner.lock().await.settings.aliases.get(model).cloned().unwrap_or_else(|| model.to_string());
        let providers: Vec<Provider> = self.offered().await.into_iter().filter(|p| p.enabled).collect();
        if let Some((pid, rest)) = target.split_once('/') {
            if let Some(p) = providers.iter().find(|p| p.id == pid) {
                return Ok((p.clone(), rest.to_string()));
            }
        }
        let offered = |p: &Provider| catalog::visible_models(p).contains(&target);
        if let Some(p) = providers.iter().find(|p| offered(p)).or_else(|| providers.iter().find(|p| p.models.contains(&target))) {
            return Ok((p.clone(), target));
        }
        let family = if target.starts_with("claude-") {
            Some(ProviderKind::Anthropic)
        } else if ["gpt-", "o1", "o3", "o4", "codex-", "chatgpt-"].iter().any(|p| target.starts_with(p)) {
            Some(ProviderKind::Openai)
        } else {
            None
        };
        if let Some(p) = family.and_then(|k| providers.iter().find(|p| p.kind == k)) {
            return Ok((p.clone(), target));
        }
        Err(format!(
            "No provider serves model '{model}'. Use '<provider>/<model>' from GET /v1/models, or add an alias in SparkBox › AI › Server."
        ))
    }

    /// Sends one small request in each API format through the running server,
    /// exactly as an outside app would.
    pub async fn self_test(&self, model: &str) -> Result<Vec<SelfTestResult>, String> {
        let url = self.inner.lock().await.running.as_ref().map(|r| r.url.clone()).ok_or("Start the server first.")?;
        let prompt = "Reply with the single word OK.";
        let client = reqwest::Client::builder().timeout(Duration::from_secs(120)).build().map_err(|e| e.to_string())?;
        let calls = [
            ("chat/completions", json!({ "model": model, "messages": [{ "role": "user", "content": prompt }] })),
            ("responses", json!({ "model": model, "input": prompt })),
            ("messages", json!({ "model": model, "max_tokens": 1024, "messages": [{ "role": "user", "content": prompt }] })),
        ];
        let mut out = Vec::new();
        for (path, body) in calls {
            let started = Instant::now();
            let resp = client
                .post(format!("{url}/v1/{path}"))
                .bearer_auth(&self.self_test_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&body)
                .send()
                .await;
            let ms = started.elapsed().as_millis() as u64;
            let result = match resp {
                Err(e) => SelfTestResult { api: path.into(), ok: false, status: 0, ms, detail: e.to_string() },
                Ok(r) => {
                    let status = r.status().as_u16();
                    let v: Value = r.json().await.unwrap_or(Value::Null);
                    let reply = match path {
                        "chat/completions" => v["choices"][0]["message"]["content"].as_str().map(String::from),
                        "responses" => v["output"].as_array().map(|items| {
                            items.iter().flat_map(|i| i["content"].as_array().cloned().unwrap_or_default())
                                .filter_map(|c| c["text"].as_str().map(String::from)).collect::<String>()
                        }),
                        _ => v["content"].as_array().map(|blocks| {
                            blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<String>()
                        }),
                    };
                    let error = v["error"]["message"].as_str().map(String::from);
                    let ok = status < 400 && reply.is_some();
                    let detail = if ok { reply.unwrap_or_default() } else { error.unwrap_or_else(|| v.to_string()) };
                    let detail: String = detail.trim().chars().take(240).collect();
                    SelfTestResult { api: path.into(), ok, status, ms, detail }
                }
            };
            out.push(result);
        }
        Ok(out)
    }

    async fn log(&self, entry: LogEntry) {
        (self.on_log)(&entry);
        let mut inner = self.inner.lock().await;
        inner.logs.push_back(entry);
        while inner.logs.len() > MAX_LOG {
            inner.logs.pop_front();
        }
    }

    /// Streams `resp` to the client unchanged while reading the usage out of it; the log
    /// entry `id` gets the tokens and timing when the reply ends (or the client goes away).
    /// `load` counts the request against a cluster engine for its Endpoints card (Ollama has no metrics of its own).
    fn tap(self: &Arc<Self>, resp: Response, id: u64, started: Instant, load: Option<crate::llm::TrafficGuard>) -> Response {
        let sse = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("text/event-stream"));
        let (parts, body) = resp.into_parts();
        let tap = Tap { server: self.clone(), id, sse, parser: SseParser::default(), buf: Vec::new(), usage: Some(Usage::new(started)), load };
        let stream = futures_util::stream::unfold((body.into_data_stream(), tap), |(mut body, mut tap)| async move {
            let chunk = body.next().await?;
            if let Ok(bytes) = &chunk {
                tap.observe(bytes);
            }
            Some((chunk, (body, tap)))
        });
        Response::from_parts(parts, Body::from_stream(stream))
    }

    /// Sends the request to the provider as-is (model renamed, auth swapped) and streams the reply back.
    async fn passthrough(&self, p: &Provider, path: &str, headers: &HeaderMap, body: &Value) -> Result<Response, String> {
        let base = p.base_url.trim_end_matches('/');
        let url = if p.kind == ProviderKind::Anthropic { format!("{base}/v1/{path}") } else { format!("{base}/{path}") };
        let mut req = self
            .llm
            .client_for(&url)
            .await?
            .post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(body).map_err(|e| e.to_string())?);
        for name in ["accept", "anthropic-beta", "openai-beta"] {
            if let Some(v) = headers.get(name) {
                req = req.header(name, v.as_bytes());
            }
        }
        let key = p.api_key()?;
        match p.kind {
            ProviderKind::Anthropic => {
                let version = headers.get("anthropic-version").and_then(|v| v.to_str().ok()).unwrap_or(ANTHROPIC_VERSION);
                req = req
                    .header("x-api-key", key.ok_or("Add your Anthropic API key in SparkBox › AI › Providers.")?)
                    .header("anthropic-version", version);
            }
            ProviderKind::Openai => req = req.bearer_auth(key.ok_or("Add your OpenAI API key in SparkBox › AI › Providers.")?),
            _ => {
                if let Some(v) = headers.get("anthropic-version") {
                    req = req.header("anthropic-version", v.as_bytes());
                }
                if let Some(k) = key {
                    req = req.bearer_auth(&k).header("x-api-key", k);
                }
            }
        }
        let upstream = req.send().await.map_err(|e| format!("Couldn't reach {} ({url}): {e}", p.name))?;
        let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let kept: Vec<(axum::http::HeaderName, HeaderValue)> = upstream
            .headers()
            .iter()
            .filter(|(name, _)| matches!(name.as_str(), "content-type" | "request-id" | "x-request-id" | "openai-processing-ms" | "retry-after"))
            .filter_map(|(name, value)| Some((name.as_str().parse().ok()?, HeaderValue::from_bytes(value.as_bytes()).ok()?)))
            .collect();
        let mut resp = Response::new(Body::from_stream(upstream.bytes_stream()));
        *resp.status_mut() = status;
        resp.headers_mut().extend(kept);
        Ok(resp)
    }

    /// Chat Completions client → Claude: converts the request, then the reply (streamed or not).
    async fn chat_via_claude(&self, p: &Provider, body: &Value, upstream_model: &str, label: &str) -> Result<Response, String> {
        let (claude_body, fallback) = translate::chat_to_anthropic(body, upstream_model)?;
        let key = p.api_key()?.ok_or("Add your Anthropic API key in SparkBox › AI › Providers.")?;
        let url = format!("{}/v1/messages", p.base_url.trim_end_matches('/'));
        let mut req = self
            .llm
            .client_for(&url)
            .await?
            .post(&url)
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&claude_body);
        if fallback {
            req = req.header("anthropic-beta", FALLBACK_BETA);
        }
        let upstream = req.send().await.map_err(|e| format!("Couldn't reach Anthropic: {e}"))?;
        let status = upstream.status();
        if !status.is_success() {
            let err: Value = upstream.json().await.unwrap_or(Value::Null);
            return Ok(api_error(
                Some(Api::ChatCompletions),
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                &crate::llm::api_error("Anthropic", status.as_u16(), &err),
            ));
        }
        let created = now_secs();
        if !claude_body["stream"].as_bool().unwrap_or(false) {
            let msg: Value = upstream.json().await.map_err(|e| e.to_string())?;
            return Ok(Json(translate::anthropic_to_chat(&msg, label, created)).into_response());
        }
        let include_usage = body["stream_options"]["include_usage"].as_bool().unwrap_or(false);
        let mut writer = ChunkWriter::new(label, created, include_usage);
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut parser = SseParser::default();
            let mut stream = upstream.bytes_stream();
            while let Some(Ok(chunk)) = stream.next().await {
                for ev in parser.push(&chunk) {
                    let v: Value = serde_json::from_str(&ev.data).unwrap_or(Value::Null);
                    for c in writer.on_anthropic_event(&v) {
                        if tx.send(format!("data: {c}\n\n")).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send("data: [DONE]\n\n".into());
        });
        Ok(sse_response(rx))
    }
}

async fn root() -> Json<Value> {
    Json(json!({
        "name": "SparkBox Server",
        "endpoints": ["GET /v1/models", "POST /v1/chat/completions", "POST /v1/responses", "POST /v1/messages",
                      "POST /v1/completions", "POST /v1/embeddings"],
    }))
}

async fn models(State(s): State<Arc<Server>>, headers: HeaderMap) -> Response {
    let started = Instant::now();
    if let Err(refused) = s.authenticate(&headers).await {
        s.log_refused(refused, "models", started).await;
        return api_error(None, StatusCode::UNAUTHORIZED, &format!("{KEY_REFUSED}."));
    }
    let mut data = Vec::new();
    // Shaped to satisfy both OpenAI and Anthropic model-list readers.
    let item = |id: String, owner: &str| {
        json!({ "id": id, "object": "model", "type": "model", "display_name": id, "created": 0,
                "created_at": "1970-01-01T00:00:00Z", "owned_by": owner })
    };
    for p in s.offered().await.into_iter().filter(|p| p.enabled) {
        if p.needs_key() && p.api_key().ok().flatten().is_none() {
            continue;
        }
        data.extend(catalog::visible_models(&p).iter().map(|m| item(format!("{}/{m}", p.id), &p.id)));
    }
    for alias in s.inner.lock().await.settings.aliases.keys() {
        data.push(item(alias.clone(), "sparkbox-alias"));
    }
    Json(json!({ "object": "list", "data": data, "has_more": false })).into_response()
}

/// Refuses foreign Hosts, unknown Origins and non-JSON POSTs before anything else runs.
async fn guard(State(s): State<Arc<Server>>, req: Request, next: Next) -> Response {
    let Some(why) = s.refusal(req.method(), req.headers()) else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_string();
    let api = match path.as_str() {
        p if p.starts_with("/v1/messages") => Some(Api::Messages),
        _ => None,
    };
    s.log(LogEntry {
        id: s.new_id(),
        ts_ms: now_ms(),
        client: "(refused)".into(),
        api: path.trim_start_matches("/v1/").to_string(),
        status: 403,
        error: Some(why.clone()),
        done: true,
        ..Default::default()
    })
    .await;
    api_error(api, StatusCode::FORBIDDEN, &why)
}

/// What a reply cost, gathered while it streams.
struct Usage {
    started: Instant,
    meter: Meter,
    input: Option<u64>,
    output: Option<u64>,
}

impl Usage {
    fn new(started: Instant) -> Usage {
        Usage { started, meter: Meter { started, ..Meter::new() }, input: None, output: None }
    }

    /// One JSON object from any of the APIs: a stream event or a whole reply.
    fn read(&mut self, v: &Value) {
        let u = if v["usage"].is_object() {
            &v["usage"]
        } else if v["response"]["usage"].is_object() {
            &v["response"]["usage"]
        } else {
            &v["message"]["usage"]
        };
        if let Some(n) = u["prompt_tokens"].as_u64().or(u["input_tokens"].as_u64()) {
            self.input = Some(n);
        }
        if let Some(n) = u["completion_tokens"].as_u64().or(u["output_tokens"].as_u64()).filter(|n| *n > 0) {
            self.output = Some(n);
        }
        let delta = v["choices"][0]["delta"]["content"].as_str().or(v["choices"][0]["delta"]["reasoning_content"].as_str())
            .or(v["delta"]["text"].as_str())
            .or(v["delta"]["thinking"].as_str())
            .or(v["delta"].as_str().filter(|_| v["type"].as_str().is_some_and(|t| t.ends_with(".delta"))));
        if delta.is_some_and(|d| !d.is_empty()) {
            self.meter.delta();
        }
    }
}

/// Watches a reply on its way to the client (see Server::tap).
struct Tap {
    server: Arc<Server>,
    id: u64,
    sse: bool,
    parser: SseParser,
    /// A plain JSON reply, kept until it ends (capped) to read its usage.
    buf: Vec<u8>,
    usage: Option<Usage>,
    load: Option<crate::llm::TrafficGuard>,
}

impl Tap {
    fn observe(&mut self, bytes: &[u8]) {
        let Some(u) = self.usage.as_mut() else { return };
        if self.sse {
            for ev in self.parser.push(bytes) {
                if let Ok(v) = serde_json::from_str::<Value>(&ev.data) {
                    u.read(&v);
                }
            }
        } else if self.buf.len() < 4 * 1024 * 1024 {
            self.buf.extend_from_slice(bytes);
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        let Some(mut usage) = self.usage.take() else { return };
        if !self.sse {
            if let Ok(v) = serde_json::from_slice::<Value>(&self.buf) {
                usage.read(&v);
            }
        }
        if let Some(load) = self.load.take() {
            load.tokens(usage.output.unwrap_or(usage.meter.deltas));
            if let Some(t) = usage.meter.ttft() {
                load.first_token(t.as_secs_f64() * 1000.0);
            }
        }
        let (server, id) = (self.server.clone(), self.id);
        tokio::spawn(async move { server.finish_log(id, usage).await });
    }
}

async fn handle(State(s): State<Arc<Server>>, headers: HeaderMap, body: Bytes, api: Api, path: &'static str) -> Response {
    let started = Instant::now();
    let client = match s.authenticate(&headers).await {
        Ok(client) => client,
        Err(refused) => {
            s.log_refused(refused, api.label(), started).await;
            return api_error(Some(api), StatusCode::UNAUTHORIZED, &format!("{KEY_REFUSED}. Create one in SparkBox › AI › Server."));
        }
    };
    let mut body: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return api_error(Some(api), StatusCode::BAD_REQUEST, &format!("Request body isn't valid JSON: {e}")),
    };
    let requested = body["model"].as_str().unwrap_or("").to_string();
    let stream = body["stream"].as_bool().unwrap_or(false);
    let mut entry = LogEntry {
        id: s.new_id(),
        ts_ms: now_ms(),
        client,
        api: api.label().into(),
        model: requested.clone(),
        stream,
        ..Default::default()
    };
    let mut load = None;

    let result: Result<Response, (StatusCode, String)> = async {
        // Look-only holds inference whichever provider would answer, as Chat does.
        if s.held() {
            return Err((StatusCode::SERVICE_UNAVAILABLE, HELD.to_string()));
        }
        if requested.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "The request has no model.".to_string()));
        }
        let (provider, upstream_model) = s.resolve(&requested).await.map_err(|e| (StatusCode::NOT_FOUND, e))?;
        entry.provider = provider.id.clone();
        body["model"] = json!(upstream_model);
        let bad_gateway = |e: String| (StatusCode::BAD_GATEWAY, e);
        match (provider.kind, api) {
            (ProviderKind::Demo, _) if path.ends_with("count_tokens") => Ok(Json(json!({ "input_tokens": 1 })).into_response()),
            (ProviderKind::Demo, Api::Embeddings) => Err((StatusCode::BAD_REQUEST, "The demo model doesn't make embeddings.".into())),
            (ProviderKind::Demo, _) => {
                let created = now_secs();
                // A simulated cluster model answers at its simulated speed, and counts as load on it.
                let speed = s.llm.demo_speed(&format!("{}/{upstream_model}", provider.id)).or_else(|| s.llm.demo_speed(&provider.id));
                let text = match speed {
                    Some(_) => demo_cluster_reply(&upstream_model, &translate::last_user_text(api, &body)),
                    None => crate::llm::chat::demo_reply(&translate::last_user_text(api, &body)),
                };
                if !stream {
                    return Ok(Json(translate::demo_response_with(api, &body, &text, &requested, created)).into_response());
                }
                let (tx, rx) = mpsc::unbounded_channel();
                let frames = translate::demo_stream_with(api, &body, &text, &requested, created);
                let load = speed.map(|_| s.llm.track(&provider.id, demo_tokens(&translate::last_user_text(api, &body))));
                tokio::spawn(async move {
                    if let Some(sp) = speed {
                        tokio::time::sleep(Duration::from_millis(sp.ttft_ms.clamp(50.0, 20_000.0) as u64)).await;
                        if let Some(l) = &load {
                            l.first_token(sp.ttft_ms);
                        }
                    }
                    let n = frames.len().max(1);
                    let tokens = demo_tokens(&text);
                    for f in frames {
                        if tx.send(f).is_err() {
                            return;
                        }
                        let each = match speed {
                            // One word a frame: its share of the reply's tokens at the simulated rate.
                            Some(sp) => tokens as f64 / n as f64 / sp.tps.max(0.2),
                            None => 0.015,
                        };
                        if let Some(l) = &load {
                            l.tokens((tokens as f64 / n as f64).round() as u64);
                        }
                        tokio::time::sleep(Duration::from_secs_f64(each)).await;
                    }
                });
                Ok(sse_response(rx))
            }
            (ProviderKind::Anthropic, Api::Messages) => s.passthrough(&provider, path, &headers, &body).await.map_err(bad_gateway),
            (ProviderKind::Anthropic, Api::ChatCompletions) => {
                s.chat_via_claude(&provider, &body, &upstream_model, &requested).await.map_err(|e| (StatusCode::BAD_REQUEST, e))
            }
            (ProviderKind::Anthropic, Api::Responses) => Err((
                StatusCode::BAD_REQUEST,
                "Claude can't be used through the OpenAI Responses API yet. Use /v1/messages or /v1/chat/completions.".into(),
            )),
            (ProviderKind::Anthropic, Api::Completions | Api::Embeddings) => Err((
                StatusCode::BAD_REQUEST,
                format!("Claude has no /v1/{path} API. Use /v1/messages or /v1/chat/completions."),
            )),
            (ProviderKind::Openai, Api::Messages) => Err((
                StatusCode::BAD_REQUEST,
                "OpenAI models can't be used through the Anthropic Messages API yet. Use /v1/chat/completions or /v1/responses.".into(),
            )),
            // OpenAI and the cluster engines: same API on both sides, so pass it through.
            // (vLLM also serves /v1/responses and /v1/messages natively.)
            _ => {
                if provider.kind == ProviderKind::OpenaiCompatible {
                    load = Some(s.llm.track(&provider.id, demo_tokens(&translate::last_user_text(api, &body))));
                }
                s.passthrough(&provider, path, &headers, &body).await.map_err(bad_gateway)
            }
        }
    }
    .await;

    let response = match result {
        Ok(r) => r,
        Err((status, msg)) => {
            entry.error = Some(msg.clone());
            api_error(Some(api), status, &msg)
        }
    };
    entry.status = response.status().as_u16();
    entry.ms = started.elapsed().as_millis() as u64;
    let id = entry.id;
    let ok = entry.error.is_none() && entry.status < 400;
    if !ok {
        entry.done = true;
        entry.duration_ms = Some(entry.ms);
    }
    s.log(entry).await;
    if ok {
        s.tap(response, id, started, load)
    } else {
        response
    }
}

#[cfg(test)]
mod tests;
