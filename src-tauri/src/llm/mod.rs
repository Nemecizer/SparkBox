//! Model providers: your cluster's engines (OpenAI-compatible), OpenAI, Anthropic,
//! and a built-in demo. Chat and the local server both resolve models through here.
//! API keys live in the Keychain as "llm:<provider id>".
//!
//! Providers come from two places: the ones saved in providers.json (yours, and
//! SparkBox's own loads), and the cluster's discovered endpoints, which Models
//! re-registers every few seconds and which are never written to disk. A discovered one
//! that stops answering stays offered for two minutes, marked so, and Chat can still send
//! to it: a health flap doesn't lose the conversation's model.

pub mod catalog;
pub mod chat;
pub mod history;
pub mod sse;
pub mod throughput;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::secrets;
use crate::vpn::Vpn;

pub const ANTHROPIC_VERSION: &str = "2026-10-21";
/// Claude default for new chats (and for the server when a request names no model).
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5-5";
/// How long a cluster provider stays offered, as not answering, after its endpoint left Ready.
pub const GRACE_MS: u64 = 2 * 60_000;
/// A reply from a cluster engine that sends nothing for this long has stopped (a GB10 that runs
/// out of unified memory can freeze instead of failing).
pub const CLUSTER_IDLE: Duration = Duration::from_secs(120);
/// The same for Claude and OpenAI, whose reasoning models can go quiet for a while before they
/// write; it still ends a reply whose connection died without closing.
pub const CLOUD_IDLE: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    /// api.anthropic.com — Messages API.
    Anthropic,
    /// api.openai.com — Responses API for chat, everything passes through for the server.
    Openai,
    /// vLLM, SGLang, TensorRT-LLM, llama.cpp, NIM, Ollama, LM Studio… (`/v1/chat/completions`).
    #[default]
    OpenaiCompatible,
    /// Built-in simulated model so chat and the server work with no keys or network.
    Demo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    /// Short id used in model names: "<id>/<model>".
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    /// For OpenAI-style providers this ends in /v1; for Anthropic it's the bare host.
    pub base_url: String,
    pub enabled: bool,
    /// Last model list fetched from the provider (everything it lists).
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    /// Offer only models that chat (no embeddings, audio, image or moderation models).
    #[serde(default = "yes")]
    pub chat_only: bool,
    /// Models turned off in AI › Providers.
    #[serde(default)]
    pub hidden: Vec<String>,
    /// Plain http is all right for this provider's private (RFC 1918) address on the home network.
    #[serde(default)]
    pub allow_http_lan: bool,
}

fn yes() -> bool {
    true
}

impl Default for Provider {
    fn default() -> Self {
        Provider {
            id: String::new(),
            name: String::new(),
            kind: ProviderKind::default(),
            base_url: String::new(),
            enabled: true,
            models: Vec::new(),
            default_model: None,
            chat_only: true,
            hidden: Vec::new(),
            allow_http_lan: false,
        }
    }
}

/// What a save did besides saving.
#[derive(Clone, Debug, PartialEq)]
pub struct Upserted {
    /// The address or kind changed, so the saved key (meant for the old one) was removed.
    pub key_cleared: bool,
}

/// Whether a provider's address may be used: https anywhere, plain http only on this Mac,
/// to a cluster node, or to a private address the user said is on the home network.
pub fn check_address(p: &Provider, node_hosts: &[String]) -> Result<(), String> {
    if p.kind == ProviderKind::Demo {
        return Ok(());
    }
    let u = url::Url::parse(&p.base_url).map_err(|_| format!("“{}” isn't a URL like https://host/v1.", p.base_url))?;
    let host = u.host_str().unwrap_or_default().trim_matches(['[', ']']).to_string();
    match u.scheme() {
        "https" => return Ok(()),
        "http" => {}
        s => return Err(format!("{s}:// isn't supported: use https:// (or http:// on this Mac or your own network).")),
    }
    let ip: Option<std::net::IpAddr> = host.parse().ok();
    if host == "localhost" || ip.is_some_and(|ip| ip.is_loopback()) || node_hosts.iter().any(|h| h == &host) {
        return Ok(());
    }
    let private = matches!(ip, Some(std::net::IpAddr::V4(v4)) if v4.is_private());
    if private && p.allow_http_lan {
        return Ok(());
    }
    if private {
        return Err(format!(
            "{host} is on a private network, but plain http sends the API key unencrypted. Tick “Allow plain HTTP on my network” if it's on yours, or use https."
        ));
    }
    Err(format!("Use https for {host}: plain http would send the API key and every prompt unencrypted."))
}

/// What the user changed on a discovered provider (they aren't saved themselves).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPrefs {
    pub enabled: bool,
    pub default_model: Option<String>,
    pub chat_only: bool,
    pub hidden: Vec<String>,
}

impl Provider {
    pub fn key_account(&self) -> String {
        format!("llm:{}", self.id)
    }

    pub fn api_key(&self) -> Result<Option<String>, String> {
        secrets::get(&self.key_account())
    }

    pub fn needs_key(&self) -> bool {
        matches!(self.kind, ProviderKind::Anthropic | ProviderKind::Openai)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    pub providers: Vec<Provider>,
    /// Choices made on discovered cluster providers, by provider id.
    #[serde(default)]
    pub discovered_prefs: BTreeMap<String, ProviderPrefs>,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        let p = |id: &str, name: &str, kind, base: &str, models: &[&str], default: Option<&str>| Provider {
            id: id.into(),
            name: name.into(),
            kind,
            base_url: base.into(),
            models: models.iter().map(|m| m.to_string()).collect(),
            default_model: default.map(Into::into),
            ..Default::default()
        };
        ProviderSettings {
            providers: vec![
                p("anthropic", "Anthropic (Claude)", ProviderKind::Anthropic, "https://api.anthropic.com",
                    &[DEFAULT_CLAUDE_MODEL], Some(DEFAULT_CLAUDE_MODEL)),
                p("openai", "OpenAI", ProviderKind::Openai, "https://api.openai.com/v1", &[], None),
                p("demo", "SparkBox demo", ProviderKind::Demo, "", &["echo"], Some("echo")),
            ],
            discovered_prefs: BTreeMap::new(),
        }
    }
}

/// What the UI sees for each provider (never the key itself).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    #[serde(flatten)]
    pub provider: Provider,
    pub has_key: bool,
    /// The models offered, filtered and in the curated order (catalog.rs).
    pub visible: Vec<String>,
    /// The model Chat and the Server start with.
    pub effective_default: Option<String>,
    /// Found on the cluster this session rather than saved.
    pub discovered: bool,
    /// Its endpoint left Ready in the last two minutes: still listed, so a chat with it isn't
    /// lost to a health flap, but not offered for a new choice.
    pub not_answering: bool,
    /// Visible models that take a reasoning effort instead of a temperature (catalog::tuning).
    pub effort_models: Vec<String>,
}

/// How fast a simulated cluster model streams, so Demo chats match the Endpoints charts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemoSpeed {
    /// Decode tokens per second for one more stream at the current load.
    pub tps: f64,
    /// Time to the first token, including any queue.
    pub ttft_ms: f64,
}

/// Requests on one provider since Models last looked: simulated ones for demo models, and
/// for engines without a metrics endpoint (Ollama) the requests SparkBox itself sends or relays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Traffic {
    pub active: u32,
    pub prompt_tokens: u64,
    pub output_tokens: u64,
    pub finished: u64,
    /// Times to first token of the requests that started streaming, in ms.
    pub ttft_ms: Vec<f64>,
    /// Mean time between tokens of each finished request that produced two or more, in ms.
    pub itl_ms: Vec<f64>,
}

#[derive(Default)]
struct Board {
    speeds: HashMap<String, DemoSpeed>,
    traffic: HashMap<String, Traffic>,
}

/// Cluster providers found this session.
#[derive(Default)]
struct Found {
    /// Ready now (Models keeps it current).
    ready: Vec<Provider>,
    /// Ones that left Ready, with when (ms): kept for GRACE_MS, as not answering.
    quiet: BTreeMap<String, (Provider, u64)>,
}

/// Where a provider in the merged list comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Origin {
    Saved,
    Cluster,
    /// A cluster provider in its grace period.
    Quiet,
}

type SourceFn = Box<dyn Fn() -> bool + Send + Sync>;

pub struct Llm {
    path: PathBuf,
    settings: Mutex<ProviderSettings>,
    found: StdMutex<Found>,
    board: StdMutex<Board>,
    vpn: Arc<Vpn>,
    direct: reqwest::Client,
    /// The client for the VPN tunnel's proxy, reused while the proxy stays the same.
    proxied: StdMutex<Option<(String, reqwest::Client)>>,
    /// Whether the app shows Demo right now (set once it runs; Demo until then).
    source: StdMutex<Option<SourceFn>>,
    /// Each node's id and the addresses it answers on, for "is cell2 still up?".
    nodes: StdMutex<Vec<(String, Vec<String>)>>,
    /// Tests shorten the stream idle limits.
    idle_override: StdMutex<Option<Duration>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Counts one request while it streams; dropping it ends the request.
pub struct TrafficGuard {
    llm: Arc<Llm>,
    provider: String,
    /// When the first token arrived, and tokens since (for the request's inter-token latency).
    stream: StdMutex<(Option<std::time::Instant>, u64)>,
}

impl TrafficGuard {
    pub fn tokens(&self, n: u64) {
        self.stream.lock().unwrap().1 += n;
        let mut b = self.llm.board.lock().unwrap();
        b.traffic.entry(self.provider.clone()).or_default().output_tokens += n;
    }

    pub fn first_token(&self, ttft_ms: f64) {
        self.stream.lock().unwrap().0.get_or_insert_with(std::time::Instant::now);
        let mut b = self.llm.board.lock().unwrap();
        b.traffic.entry(self.provider.clone()).or_default().ttft_ms.push(ttft_ms);
    }
}

impl Drop for TrafficGuard {
    fn drop(&mut self) {
        let (first, tokens) = *self.stream.lock().unwrap();
        let mut b = self.llm.board.lock().unwrap();
        let t = b.traffic.entry(self.provider.clone()).or_default();
        t.active = t.active.saturating_sub(1);
        t.finished += 1;
        if let (Some(first), 2..) = (first, tokens) {
            t.itl_ms.push(first.elapsed().as_secs_f64() * 1000.0 / (tokens - 1) as f64);
        }
    }
}

/// True for addresses that only exist on the home network, which may need the VPN.
pub fn is_home_address(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else { return false };
    match u.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_private() || ip.is_link_local(),
        Some(url::Host::Domain(d)) => d.ends_with(".local"),
        _ => false,
    }
}

impl Llm {
    pub fn new(path: PathBuf, vpn: Arc<Vpn>) -> Arc<Llm> {
        let settings: ProviderSettings = crate::util::load_json_or_default(&path);
        let direct = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("HTTP client");
        Arc::new(Llm {
            path,
            settings: Mutex::new(settings),
            found: StdMutex::new(Found::default()),
            board: StdMutex::new(Board::default()),
            vpn,
            direct,
            proxied: StdMutex::new(None),
            source: StdMutex::new(None),
            nodes: StdMutex::new(Vec::new()),
            idle_override: StdMutex::new(None),
        })
    }

    /// Where the provider list learns which source the app shows: true while it shows Demo.
    pub fn set_source(&self, demo_now: impl Fn() -> bool + Send + Sync + 'static) {
        *self.source.lock().unwrap() = Some(Box::new(demo_now));
    }

    fn demo_now(&self) -> bool {
        self.source.lock().unwrap().as_ref().is_none_or(|f| f())
    }

    /// An HTTP client for `url`: cluster addresses go through the VPN tunnel when it's up.
    pub async fn client_for(&self, url: &str) -> Result<reqwest::Client, String> {
        if is_home_address(url) {
            if let Some(proxy) = self.vpn.proxy_url().await {
                return self.proxied_client(&proxy).map(|(c, _)| c);
            }
        }
        Ok(self.direct.clone())
    }

    /// The client for the tunnel's proxy, and whether it was just made: one per proxy URL,
    /// so requests through the tunnel share its connections (a new proxy replaces it).
    fn proxied_client(&self, proxy: &str) -> Result<(reqwest::Client, bool), String> {
        let mut cur = self.proxied.lock().unwrap();
        if let Some((_, client)) = cur.as_ref().filter(|(url, _)| url == proxy) {
            return Ok((client.clone(), false));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .proxy(reqwest::Proxy::all(proxy).map_err(|e| e.to_string())?)
            .build()
            .map_err(|e| e.to_string())?;
        *cur = Some((proxy.to_string(), client.clone()));
        Ok((client, true))
    }

    /// Saved providers, then cluster ones, Ready before quiet (a saved provider wins over a
    /// cluster one with its id).
    fn merged(&self, s: &ProviderSettings) -> Vec<(Provider, Origin)> {
        let mut out: Vec<(Provider, Origin)> = s.providers.iter().map(|p| (p.clone(), Origin::Saved)).collect();
        let found = self.found.lock().unwrap();
        let cluster = found.ready.iter().map(|p| (p, Origin::Cluster)).chain(found.quiet.values().map(|(p, _)| (p, Origin::Quiet)));
        for (d, origin) in cluster {
            if out.iter().any(|(p, _)| p.id == d.id) {
                continue;
            }
            let mut d = d.clone();
            if let Some(pref) = s.discovered_prefs.get(&d.id) {
                d.enabled = pref.enabled;
                d.default_model = pref.default_model.clone().or(d.default_model);
                d.chat_only = pref.chat_only;
                d.hidden = pref.hidden.clone();
            }
            out.push((d, origin));
        }
        out
    }

    /// What AI › Providers, Chat and the Server list. With Live data the demo model (and any
    /// simulated cluster model) isn't offered.
    pub async fn views(&self) -> Vec<ProviderView> {
        let live = !self.demo_now();
        let s = self.settings.lock().await;
        self.merged(&s)
            .into_iter()
            .filter(|(p, _)| !(live && p.kind == ProviderKind::Demo))
            .map(|(p, origin)| {
                let visible = catalog::visible_models(&p);
                ProviderView {
                    has_key: p.api_key().ok().flatten().is_some(),
                    effort_models: visible.iter().filter(|m| catalog::tuning(p.kind, m) == catalog::Tuning::Effort).cloned().collect(),
                    visible,
                    effective_default: catalog::default_model(&p),
                    discovered: origin != Origin::Saved,
                    not_answering: origin == Origin::Quiet,
                    provider: p,
                }
            })
            .collect()
    }

    /// A provider that's answering (one in its grace period isn't: the Server and benchmarks
    /// don't send to it).
    pub async fn get(&self, id: &str) -> Option<Provider> {
        self.all().await.into_iter().find(|p| p.id == id)
    }

    /// The provider a chat is with, including one that stopped answering in the last two
    /// minutes: its reply then fails or times out on its own, rather than "Unknown provider".
    pub async fn chat_provider(&self, id: &str) -> Option<Provider> {
        let s = self.settings.lock().await;
        self.merged(&s).into_iter().map(|(p, _)| p).find(|p| p.id == id)
    }

    pub async fn all(&self) -> Vec<Provider> {
        let s = self.settings.lock().await;
        self.merged(&s).into_iter().filter(|(_, o)| *o != Origin::Quiet).map(|(p, _)| p).collect()
    }

    async fn save(&self, s: &ProviderSettings) -> Result<(), String> {
        crate::util::save_json(&self.path, s)
    }

    /// Replaces the cluster providers that are Ready; true when anything changed. One that
    /// left is kept for GRACE_MS, as not answering (`forget_quiet` drops it sooner).
    pub fn set_discovered(&self, list: Vec<Provider>) -> bool {
        self.set_discovered_at(list, now_ms())
    }

    pub fn set_discovered_at(&self, mut list: Vec<Provider>, now: u64) -> bool {
        list.sort_by(|a, b| a.id.cmp(&b.id));
        let mut f = self.found.lock().unwrap();
        let quiet_before: Vec<String> = f.quiet.keys().cloned().collect();
        let left: Vec<Provider> = f.ready.iter().filter(|p| !list.iter().any(|x| x.id == p.id)).cloned().collect();
        for p in left {
            f.quiet.entry(p.id.clone()).or_insert((p, now));
        }
        for p in &list {
            f.quiet.remove(&p.id);
        }
        f.quiet.retain(|_, (_, since)| now.saturating_sub(*since) < GRACE_MS);
        let same = f.ready.len() == list.len()
            && f.ready.iter().zip(&list).all(|(a, b)| a.id == b.id && a.base_url == b.base_url && a.models == b.models && a.kind == b.kind && a.name == b.name)
            && f.quiet.keys().eq(quiet_before.iter());
        f.ready = list;
        !same
    }

    /// Drops quiet providers `keep` says no longer have an endpoint (unloaded, stopped, gone):
    /// the grace is for a flap, not for something that was taken away.
    pub fn forget_quiet(&self, keep: impl Fn(&str) -> bool) {
        self.found.lock().unwrap().quiet.retain(|id, _| keep(id));
    }

    pub fn is_discovered(&self, id: &str) -> bool {
        let f = self.found.lock().unwrap();
        f.ready.iter().any(|p| p.id == id) || f.quiet.contains_key(id)
    }

    /// Ids of the providers saved in providers.json.
    pub async fn saved_ids(&self) -> Vec<String> {
        self.settings.lock().await.providers.iter().map(|p| p.id.clone()).collect()
    }

    /// Takes a saved provider out of providers.json, leaving its key where it is (a stale
    /// registration, `Models::reconcile`; Remove in AI › Providers also clears the key).
    pub async fn unsave(&self, id: &str) -> Result<bool, String> {
        let mut s = self.settings.lock().await;
        let before = s.providers.len();
        s.providers.retain(|p| p.id != id);
        if s.providers.len() == before {
            return Ok(false);
        }
        self.save(&s).await.map(|_| true)
    }

    /// A provider saved by a scan that the cluster now finds by itself: the user's choices on it
    /// are kept as a discovered provider's, and the saved copy goes, so the cluster's own entry
    /// (offered only while it answers) takes its place. False when there's no such saved one.
    pub async fn adopt_scanned(&self, id: &str) -> Result<bool, String> {
        let mut s = self.settings.lock().await;
        let Some(i) = s.providers.iter().position(|p| p.id == id && p.kind == ProviderKind::OpenaiCompatible && !p.id.starts_with("vllm-")) else {
            return Ok(false);
        };
        let p = s.providers.remove(i);
        let prefs = ProviderPrefs { enabled: p.enabled, default_model: p.default_model, chat_only: p.chat_only, hidden: p.hidden };
        s.discovered_prefs.entry(p.id).or_insert(prefs);
        self.save(&s).await.map(|_| true)
    }

    // ---- Which node a provider runs on, and how long its replies may go quiet

    /// Each node's id and its addresses (host name, IP), set by Models each round.
    pub fn set_nodes(&self, nodes: Vec<(String, Vec<String>)>) {
        *self.nodes.lock().unwrap() = nodes;
    }

    /// The node a provider's address belongs to, if it's one of the cluster's.
    pub fn node_of(&self, url: &str) -> Option<String> {
        let host = url::Url::parse(url).ok()?.host_str()?.trim_matches(['[', ']']).to_string();
        self.nodes.lock().unwrap().iter().find(|(_, addrs)| addrs.iter().any(|a| a == &host)).map(|(id, _)| id.clone())
    }

    /// How long a reply from `p` may send nothing before it counts as stopped.
    pub fn stream_idle(&self, p: &Provider) -> Duration {
        if let Some(d) = *self.idle_override.lock().unwrap() {
            return d;
        }
        if matches!(p.kind, ProviderKind::Anthropic | ProviderKind::Openai) { CLOUD_IDLE } else { CLUSTER_IDLE }
    }

    #[cfg(test)]
    pub fn set_stream_idle(&self, d: Duration) {
        *self.idle_override.lock().unwrap() = Some(d);
    }

    /// What a reply that went quiet ends with.
    pub fn stalled(&self, p: &Provider) -> String {
        match self.node_of(&p.base_url) {
            Some(node) => format!("The model stopped responding (is {node} still up?)"),
            None => format!("The model stopped responding: nothing came from {} for {} s.", p.name, self.stream_idle(p).as_secs()),
        }
    }

    /// What a reply whose connection closed before its last event ends with.
    pub fn interrupted(&self, p: &Provider) -> String {
        match self.node_of(&p.base_url) {
            Some(node) => format!("The connection closed before the reply finished (is {node} still up?)"),
            None => format!("The connection to {} closed before the reply finished.", p.name),
        }
    }

    /// Adds or replaces a provider (matched by id). For a discovered provider only the
    /// user's choices (enabled, default, model list) are kept.
    pub async fn upsert(&self, p: Provider) -> Result<(), String> {
        let id_ok = !p.id.is_empty() && p.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !id_ok {
            return Err("Provider ids may only use letters, digits, '-' and '_'.".into());
        }
        let mut s = self.settings.lock().await;
        if !s.providers.iter().any(|x| x.id == p.id) && self.is_discovered(&p.id) {
            let prefs = ProviderPrefs { enabled: p.enabled, default_model: p.default_model, chat_only: p.chat_only, hidden: p.hidden };
            s.discovered_prefs.insert(p.id, prefs);
            return self.save(&s).await;
        }
        match s.providers.iter_mut().find(|x| x.id == p.id) {
            Some(existing) => *existing = p,
            None => s.providers.push(p),
        }
        self.save(&s).await
    }

    /// Saves a provider from AI › Providers: its address must pass `check_address`, and when
    /// its address or kind changes the key saved for the old one is removed (it would otherwise
    /// be sent to the new address).
    pub async fn upsert_checked(&self, p: Provider, node_hosts: &[String]) -> Result<Upserted, String> {
        let discovered = self.is_discovered(&p.id);
        let old = {
            let s = self.settings.lock().await;
            s.providers.iter().find(|x| x.id == p.id).cloned()
        };
        if !discovered || old.is_some() {
            check_address(&p, node_hosts)?;
        }
        let moved = old.as_ref().is_some_and(|o| o.base_url.trim_end_matches('/') != p.base_url.trim_end_matches('/') || o.kind != p.kind);
        let had_key = moved && p.api_key().ok().flatten().is_some();
        self.upsert(p.clone()).await?;
        if had_key {
            secrets::clear(&p.key_account())?;
        }
        Ok(Upserted { key_cleared: had_key })
    }

    pub async fn remove(&self, id: &str) -> Result<(), String> {
        let mut s = self.settings.lock().await;
        s.providers.retain(|p| p.id != id);
        s.discovered_prefs.remove(id);
        secrets::clear(&format!("llm:{id}"))?;
        self.save(&s).await
    }

    // ---- Traffic, and the demo cluster's speeds

    /// How fast each simulated cluster model answers right now (set by Models each tick).
    pub fn set_demo_speeds(&self, speeds: HashMap<String, DemoSpeed>) {
        self.board.lock().unwrap().speeds = speeds;
    }

    pub fn demo_speed(&self, provider: &str) -> Option<DemoSpeed> {
        self.board.lock().unwrap().speeds.get(provider).copied()
    }

    /// Starts counting a request on `provider` (from Chat or the Server).
    pub fn track(self: &Arc<Self>, provider: &str, prompt_tokens: u64) -> TrafficGuard {
        let mut b = self.board.lock().unwrap();
        let t = b.traffic.entry(provider.to_string()).or_default();
        t.active += 1;
        t.prompt_tokens += prompt_tokens;
        TrafficGuard { llm: self.clone(), provider: provider.into(), stream: StdMutex::new((None, 0)) }
    }

    /// Requests in flight now, plus tokens, first-token times and finished requests since the last call.
    pub fn take_traffic(&self, provider: &str) -> Traffic {
        let mut b = self.board.lock().unwrap();
        let t = b.traffic.entry(provider.to_string()).or_default();
        let out = t.clone();
        *t = Traffic { active: t.active, ..Default::default() };
        out
    }

    /// Fetches the provider's model list and remembers it.
    pub async fn refresh_models(&self, id: &str) -> Result<Vec<String>, String> {
        let p = self.get(id).await.ok_or("Unknown provider")?;
        let models = self.fetch_models(&p).await?;
        let mut s = self.settings.lock().await;
        if let Some(x) = s.providers.iter_mut().find(|x| x.id == id) {
            x.models = models.clone();
            // Keep the saved default while it's still offered; otherwise the flagship.
            x.default_model = catalog::default_model(x);
        } else {
            return Ok(models); // a discovered provider: Models keeps its list current
        }
        self.save(&s).await?;
        Ok(models)
    }

    async fn fetch_models(&self, p: &Provider) -> Result<Vec<String>, String> {
        let key = p.api_key()?;
        if p.needs_key() && key.is_none() {
            return Err(format!("Add your {} API key first.", p.name));
        }
        let url = format!("{}/{}", p.base_url.trim_end_matches('/'), if p.kind == ProviderKind::Anthropic { "v1/models?limit=100" } else { "models" });
        let mut req = self.client_for(&url).await?.get(&url).timeout(Duration::from_secs(15));
        req = match p.kind {
            ProviderKind::Demo => return Ok(vec!["echo".into()]),
            ProviderKind::Anthropic => req
                .header("x-api-key", key.unwrap_or_default())
                .header("anthropic-version", ANTHROPIC_VERSION),
            _ => match key {
                Some(k) => req.bearer_auth(k),
                None => req,
            },
        };
        let resp = req.send().await.map_err(|e| format!("Couldn't reach {}: {e}", p.name))?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| format!("Unexpected reply from {}: {e}", p.name))?;
        if !status.is_success() {
            return Err(api_error(&p.name, status.as_u16(), &body));
        }
        // Everything is kept; catalog.rs decides what Chat and the Server offer.
        let mut ids: Vec<String> = body["data"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .unwrap_or_default();
        ids.sort_by(|a, b| catalog::compare(p.kind, a, b));
        Ok(ids)
    }
}

/// Engines SparkBox looks for when scanning the cluster: port, id suffix, label.
const ENGINES: [(u16, &str, &str); 4] = [
    (8000, "vllm", "vLLM / TensorRT-LLM / NIM"),
    (30000, "sglang", "SGLang"),
    (11434, "ollama", "Ollama"),
    (8080, "llamacpp", "llama.cpp"),
];

impl Llm {
    /// Probes each node's usual engine ports for an OpenAI-compatible server and
    /// adds what it finds as providers. `nodes` is (name, ip).
    pub async fn scan(&self, nodes: Vec<(String, String)>) -> Result<Vec<Provider>, String> {
        let probes = nodes.iter().flat_map(|(name, ip)| {
            ENGINES.iter().map(move |(port, engine, label)| async move {
                let base = format!("http://{ip}:{port}/v1");
                let url = format!("{base}/models");
                let client = self.client_for(&url).await.ok()?;
                let resp = client.get(&url).timeout(Duration::from_secs(3)).send().await.ok()?;
                let body: Value = resp.json().await.ok()?;
                let models: Vec<String> = body["data"].as_array()?.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect();
                Some(Provider {
                    id: format!("{name}-{engine}-{port}"),
                    name: format!("{label} on {name}"),
                    kind: ProviderKind::OpenaiCompatible,
                    base_url: base,
                    default_model: models.first().cloned(),
                    models,
                    ..Default::default()
                })
            })
        });
        let found: Vec<Provider> = futures_util::future::join_all(probes).await.into_iter().flatten().collect();
        for p in &found {
            self.upsert(p.clone()).await?;
        }
        Ok(found)
    }
}

/// Turns an error body from any provider into one readable line.
pub fn api_error(provider: &str, status: u16, body: &Value) -> String {
    let msg = body["error"]["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .or_else(|| body["message"].as_str())
        .or_else(|| body["detail"].as_str())
        .unwrap_or("no details");
    let hint = match status {
        401 | 403 => " Check the API key.",
        404 => " The model or endpoint wasn't found.",
        429 => " Rate limited; try again shortly.",
        _ => "",
    };
    format!("{provider} returned {status}: {msg}.{hint}")
}

#[cfg(test)]
mod address_tests {
    use super::*;

    fn p(url: &str, lan: bool) -> Provider {
        Provider { id: "x".into(), kind: ProviderKind::OpenaiCompatible, base_url: url.into(), allow_http_lan: lan, ..Default::default() }
    }

    #[test]
    fn plain_http_only_to_this_mac_the_cluster_or_a_ticked_home_address() {
        let nodes = vec!["192.168.50.51".to_string(), "cell1.local".to_string()];
        assert!(check_address(&p("https://api.example.com/v1", false), &nodes).is_ok());
        assert!(check_address(&p("http://127.0.0.1:1234/v1", false), &nodes).is_ok());
        assert!(check_address(&p("http://localhost:11434/v1", false), &nodes).is_ok());
        assert!(check_address(&p("http://[::1]:8080/v1", false), &nodes).is_ok());
        assert!(check_address(&p("http://192.168.50.51:8000/v1", false), &nodes).is_ok(), "an inventory node");
        assert!(check_address(&p("http://cell1.local:8000/v1", false), &nodes).is_ok());
        let lan = check_address(&p("http://192.168.50.20:1234/v1", false), &nodes).unwrap_err();
        assert!(lan.contains("Allow plain HTTP on my network"), "{lan}");
        assert!(check_address(&p("http://192.168.50.20:1234/v1", true), &nodes).is_ok(), "ticked");
        assert!(check_address(&p("http://10.0.0.5/v1", true), &nodes).is_ok());
        let public = check_address(&p("http://203.0.113.9/v1", true), &nodes).unwrap_err();
        assert!(public.contains("Use https"), "a public address never gets plain http: {public}");
        assert!(check_address(&p("http://models.example.com/v1", true), &nodes).is_err());
        assert!(check_address(&p("ftp://x/v1", false), &nodes).is_err());
        assert!(check_address(&Provider { kind: ProviderKind::Demo, base_url: "demo".into(), ..Default::default() }, &nodes).is_ok());
    }

    #[tokio::test]
    async fn a_new_address_or_kind_drops_the_key_saved_for_the_old_one() {
        let dir = std::env::temp_dir().join(format!("sparkbox-upsert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}));
        let lm = Provider { id: "lmstudio-up".into(), name: "LM Studio".into(), ..p("http://192.168.50.20:1234/v1", true) };
        assert!(!llm.upsert_checked(lm.clone(), &[]).await.unwrap().key_cleared);
        secrets::set(&lm.key_account(), "sk-old").unwrap();
        // Same address, other settings: the key stays.
        let same = llm.upsert_checked(Provider { enabled: false, ..lm.clone() }, &[]).await.unwrap();
        assert!(!same.key_cleared);
        assert_eq!(secrets::get(&lm.key_account()).unwrap().as_deref(), Some("sk-old"));
        // Repointed: the old key isn't sent to the new host.
        let moved = llm.upsert_checked(Provider { base_url: "https://models.example.com/v1".into(), ..lm.clone() }, &[]).await.unwrap();
        assert!(moved.key_cleared);
        assert_eq!(secrets::get(&lm.key_account()).unwrap(), None);
        // A refused address changes nothing.
        let refused = llm.upsert_checked(Provider { base_url: "http://203.0.113.9/v1".into(), ..lm.clone() }, &[]).await;
        assert!(refused.is_err());
        assert_eq!(llm.get("lmstudio-up").await.unwrap().base_url, "https://models.example.com/v1");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_addresses_are_recognised() {
        assert!(is_home_address("http://192.168.50.51:8000/v1"));
        assert!(is_home_address("http://cell1-example.local:8000/v1"));
        assert!(is_home_address("http://10.100.0.11:30000/v1"));
        assert!(!is_home_address("https://api.openai.com/v1"));
        assert!(!is_home_address("http://127.0.0.1:8484/v1"));
    }

    #[tokio::test]
    async fn discovered_providers_are_offered_but_never_saved() {
        let dir = std::env::temp_dir().join(format!("sparkbox-llm-discovered-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let vpn = Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
        let llm = Llm::new(dir.join("providers.json"), vpn);
        let found = Provider {
            id: "cell1-vllm-8000".into(),
            name: "vLLM on cell1 :8000".into(),
            base_url: "http://192.168.50.51:8000/v1".into(),
            models: vec!["llama-3.3-70b".into(), "BAAI/bge-m3".into()],
            ..Default::default()
        };
        assert!(llm.set_discovered(vec![found.clone()]));
        assert!(!llm.set_discovered(vec![found.clone()]), "unchanged");
        let v = llm.views().await.into_iter().find(|p| p.provider.id == "cell1-vllm-8000").unwrap();
        assert!(v.discovered);
        assert_eq!(v.visible, ["llama-3.3-70b"], "the embedding model isn't offered for chat");
        assert_eq!(v.effective_default.as_deref(), Some("llama-3.3-70b"));

        // Turning it off is remembered as a preference, not by saving the provider itself.
        llm.upsert(Provider { enabled: false, ..found.clone() }).await.unwrap();
        let saved = std::fs::read_to_string(dir.join("providers.json")).unwrap();
        assert!(!saved.contains("192.168.50.51"), "{saved}");
        assert!(!llm.get("cell1-vllm-8000").await.unwrap().enabled);
        // Taken away (its endpoint is gone): gone from Chat and the Server at once.
        llm.set_discovered(vec![]);
        llm.forget_quiet(|_| false);
        assert!(llm.get("cell1-vllm-8000").await.is_none());
        assert!(llm.views().await.iter().all(|v| v.provider.id != "cell1-vllm-8000"));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn found(id: &str) -> Provider {
        Provider { id: id.into(), name: format!("vLLM on cell2 :8000 ({id})"), base_url: "http://192.168.50.154:8000/v1".into(), models: vec!["qwen3-8b".into()], ..Default::default() }
    }

    #[tokio::test]
    async fn a_cluster_provider_stays_two_minutes_after_it_leaves_ready_marked_not_answering() {
        let dir = std::env::temp_dir().join(format!("sparkbox-llm-grace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}));
        let t0 = 1_000_000;
        assert!(llm.set_discovered_at(vec![found("cell2-vllm-8000")], t0));
        let view = |llm: &Arc<Llm>| {
            let llm = llm.clone();
            async move { llm.views().await.into_iter().find(|v| v.provider.id == "cell2-vllm-8000") }
        };
        assert!(!view(&llm).await.unwrap().not_answering);

        // A flap: no longer Ready. Still listed (not answering), and Chat can still send to it,
        // but the Server and a new pick don't use it.
        assert!(llm.set_discovered_at(vec![], t0 + 1_000), "it went quiet: a change");
        let v = view(&llm).await.expect("kept through the flap");
        assert!(v.not_answering && v.discovered);
        assert!(llm.chat_provider("cell2-vllm-8000").await.is_some(), "no 'Unknown provider' mid-chat");
        assert!(llm.get("cell2-vllm-8000").await.is_none(), "the Server doesn't route to it");
        assert!(llm.is_discovered("cell2-vllm-8000"), "its settings stay a discovered provider's");
        assert!(!llm.set_discovered_at(vec![], t0 + 60_000), "nothing new a minute in");
        assert!(view(&llm).await.unwrap().not_answering);

        // Back within the grace: answering again, no longer marked.
        llm.set_discovered_at(vec![found("cell2-vllm-8000")], t0 + 90_000);
        assert!(!view(&llm).await.unwrap().not_answering);

        // Quiet for two minutes: dropped.
        llm.set_discovered_at(vec![], t0 + 100_000);
        llm.set_discovered_at(vec![], t0 + 100_000 + GRACE_MS - 1);
        assert!(view(&llm).await.is_some(), "just inside the two minutes");
        assert!(llm.set_discovered_at(vec![], t0 + 100_000 + GRACE_MS), "dropped: a change");
        assert!(view(&llm).await.is_none());
        assert!(llm.chat_provider("cell2-vllm-8000").await.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn with_live_data_the_demo_provider_isnt_offered() {
        let dir = std::env::temp_dir().join(format!("sparkbox-llm-demo-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}));
        let demo = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let d = demo.clone();
        llm.set_source(move || d.load(std::sync::atomic::Ordering::SeqCst));
        // A simulated cluster model from Demo, still registered when the source changes.
        llm.set_discovered(vec![Provider { id: "vllm-qwen".into(), kind: ProviderKind::Demo, models: vec!["qwen3.8-27b".into()], ..Default::default() }]);
        let ids = |llm: &Arc<Llm>| {
            let llm = llm.clone();
            async move { llm.views().await.into_iter().map(|v| v.provider.id).collect::<Vec<_>>() }
        };
        assert_eq!(ids(&llm).await, ["anthropic", "openai", "demo", "vllm-qwen"]);
        demo.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(ids(&llm).await, ["anthropic", "openai"], "Live offers no simulated model");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn requests_through_one_proxy_share_one_client() {
        let dir = std::env::temp_dir().join(format!("sparkbox-llm-pool-{}", std::process::id()));
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}));
        let a = "socks5h://sparkbox:secret@127.0.0.1:41080";
        assert!(llm.proxied_client(a).unwrap().1, "the first request makes it");
        assert!(!llm.proxied_client(a).unwrap().1, "the second reuses it");
        assert!(!llm.proxied_client(a).unwrap().1);
        let b = "socks5h://sparkbox:other@127.0.0.1:41081";
        assert!(llm.proxied_client(b).unwrap().1, "a new tunnel gets its own");
        assert!(llm.proxied_client(a).unwrap().1, "and replaces the old one");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_scanned_provider_the_cluster_finds_keeps_its_settings_as_a_discovered_one() {
        let dir = std::env::temp_dir().join(format!("sparkbox-llm-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}));
        llm.upsert(Provider { enabled: false, hidden: vec!["x".into()], ..found("cell2-vllm-8000") }).await.unwrap();
        llm.upsert(Provider { id: "vllm-qwen".into(), ..found("vllm-qwen") }).await.unwrap();
        assert!(llm.adopt_scanned("cell2-vllm-8000").await.unwrap());
        assert!(!llm.adopt_scanned("vllm-qwen").await.unwrap(), "a load's own registration isn't a scan's");
        assert!(!llm.saved_ids().await.contains(&"cell2-vllm-8000".to_string()));
        llm.set_discovered(vec![found("cell2-vllm-8000")]);
        let p = llm.chat_provider("cell2-vllm-8000").await.unwrap();
        assert_eq!((p.enabled, p.hidden.as_slice()), (false, &["x".to_string()][..]), "the choices made on it carry over");
        assert!(llm.unsave("vllm-qwen").await.unwrap());
        assert!(!llm.unsave("vllm-qwen").await.unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_provider_on_a_node_names_it_when_its_reply_stops() {
        let dir = std::env::temp_dir().join(format!("sparkbox-llm-stalled-{}", std::process::id()));
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}));
        llm.set_nodes(vec![("cell2".into(), vec!["cell2-example.local".into(), "192.168.50.154".into()])]);
        assert_eq!(llm.stalled(&found("cell2-vllm-8000")), "The model stopped responding (is cell2 still up?)");
        let claude = Provider { name: "Anthropic (Claude)".into(), kind: ProviderKind::Anthropic, base_url: "https://api.anthropic.com".into(), ..Default::default() };
        assert_eq!(llm.stalled(&claude), "The model stopped responding: nothing came from Anthropic (Claude) for 300 s.");
        assert_eq!(llm.interrupted(&found("cell2-vllm-8000")), "The connection closed before the reply finished (is cell2 still up?)");
        assert_eq!(llm.interrupted(&claude), "The connection to Anthropic (Claude) closed before the reply finished.");
        assert_eq!(llm.stream_idle(&found("x")), CLUSTER_IDLE);
    }

    #[test]
    fn errors_read_like_sentences() {
        let body = serde_json::json!({"error": {"type": "authentication_error", "message": "invalid x-api-key"}});
        assert_eq!(api_error("Anthropic", 401, &body), "Anthropic returned 401: invalid x-api-key. Check the API key.");
    }
}
