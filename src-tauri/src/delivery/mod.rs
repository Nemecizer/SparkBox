//! Sending alerts off the Mac: ntfy, Pushover, a webhook (or Slack) and email,
//! plus the dead-man's switch (a URL the leader agent pings every minute, which
//! pages the owner itself when the pings stop). notify.rs decides what to say and
//! when; this module only delivers it and keeps the delivery log (the last 500,
//! deliveries.jsonl): one row per alert event and channel, with its outcome, so a
//! page that wasn't sent says why (held for quiet hours, a duplicate, silenced…).
//!
//! Channel settings live in notify.json; their secrets (tokens, keys, the SMTP
//! password, the webhook's signing secret, and the webhook and dead-man's-switch URLs,
//! which are credentials themselves: whoever has a Discord webhook or a healthchecks.io
//! ping URL can post to it) live in the Keychain as `notify.<channel id>.<field>` and are
//! read only to send, or to copy them to the node agents (agent::notify_json, the secret
//! notify.json) unless the channel is marked "this Mac only". notify.json keeps only the
//! URL's host, `urlHost`.
//! The agents deliver with the same gate and words (sparkbox_agent.py), so a phone
//! sees the same titles whichever side sent them.

pub mod heartbeat;
pub mod ntfy;
pub mod pushover;
pub mod smtp;
#[cfg(test)]
mod tests;
pub mod webhook;

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::health::Severity;

/// Entries kept in memory and in deliveries.jsonl.
pub const LOG_KEEP: usize = 500;
/// Waits before the retries of a delivery that failed for a reason that may pass.
pub const RETRIES: [Duration; 3] = [Duration::from_secs(30), Duration::from_secs(120), Duration::from_secs(600)];
/// After those, a critical alert keeps trying this often…
pub const CRIT_RETRY_EVERY: Duration = Duration::from_secs(30 * 60);
/// …until this long after its first try.
pub const CRIT_RETRY_FOR: Duration = Duration::from_secs(24 * 3600);

/// How many tries a delivery gets in all: 4, or for a critical alert as many as fit in 24 h.
pub fn max_attempts(critical: bool) -> u32 {
    let first = RETRIES.len() as u32 + 1;
    if !critical {
        return first;
    }
    let used: u64 = RETRIES.iter().map(|d| d.as_secs()).sum();
    first + ((CRIT_RETRY_FOR.as_secs() - used) / CRIT_RETRY_EVERY.as_secs()) as u32
}

/// The wait before try `attempt + 1`, or None when `attempt` was the last.
pub fn retry_after(attempt: u32, critical: bool) -> Option<Duration> {
    if attempt >= max_attempts(critical) {
        return None;
    }
    Some(RETRIES.get(attempt as usize - 1).copied().unwrap_or(CRIT_RETRY_EVERY))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Notification Center on this Mac.
    Mac,
    Ntfy,
    Pushover,
    Webhook,
    Email,
    /// A dead-man's switch: pinged by the leader agent every minute, never sent alerts.
    Heartbeat,
}

impl Kind {
    /// The Keychain fields each kind has.
    pub fn secret_fields(self) -> &'static [&'static str] {
        match self {
            Kind::Mac => &[],
            Kind::Heartbeat => &["url"],
            Kind::Ntfy => &["token"],
            Kind::Pushover => &["appToken", "userKey"],
            Kind::Webhook => &["url", "secret"],
            Kind::Email => &["password"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Mac => "This Mac",
            Kind::Ntfy => "ntfy",
            Kind::Pushover => "Pushover",
            Kind::Webhook => "Webhook",
            Kind::Email => "Email",
            Kind::Heartbeat => "Dead-man's switch",
        }
    }

    /// A phone or team channel alerts are sent to (not this Mac, not the dead-man's switch).
    pub fn pages(self) -> bool {
        !matches!(self, Kind::Mac | Kind::Heartbeat)
    }
}

/// One place alerts can go. No secrets: those are in the Keychain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    /// Only critical alerts (and their resolution) go here.
    #[serde(default)]
    pub critical_only: bool,
    /// ntfy server, default https://ntfy.sh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Pushover device name (empty: all the user's devices).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Only on its way in from Settings: a webhook or dead-man's-switch URL is a credential, so
    /// saving moves it to the Keychain (field "url") and keeps its host in `url_host`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The URL's host, for showing and logging it ("discord.com", "hc-ping.com").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_host: Option<String>,
    /// Webhook: send {"text": …} for a Slack-compatible incoming webhook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slack: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// "starttls" (port 587) or "ssl" (465).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// One address, or several separated by commas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Which secret fields are in the Keychain, so listing channels never reads it (a read
    /// can bring up a macOS prompt); only sending does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved: Vec<String>,
    /// Sent from this Mac only: neither it nor its secrets are copied to the node agents.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mac_only: bool,
}

fn blank(v: &Option<String>) -> bool {
    v.as_deref().is_none_or(|s| s.trim().is_empty())
}

impl Channel {
    /// A channel of `kind` with nothing filled in.
    pub fn empty(id: &str, kind: Kind, name: &str) -> Channel {
        Channel {
            id: id.into(),
            kind,
            name: name.into(),
            enabled: true,
            critical_only: false,
            server: None,
            topic: None,
            device: None,
            url: None,
            url_host: None,
            slack: None,
            host: None,
            port: None,
            security: None,
            user: None,
            from: None,
            to: None,
            saved: Vec::new(),
            mac_only: false,
        }
    }

    /// What it reaches, without secrets: (field, value) for every setting whose change
    /// points the channel somewhere else, so its saved secrets no longer belong to it.
    pub fn destination(&self) -> Vec<(&'static str, String)> {
        let o = |v: &Option<String>| v.as_deref().unwrap_or("").trim().to_string();
        vec![
            ("kind", self.kind.label().to_string()),
            ("server", o(&self.server)),
            ("URL host", o(&self.url_host)),
            ("host", o(&self.host)),
            ("port", self.port.map(|p| p.to_string()).unwrap_or_default()),
            ("security", o(&self.security)),
            ("username", o(&self.user)),
        ]
    }

    /// Checks what a channel needs before it's saved. `has_secret` says whether a field is
    /// in the Keychain (or about to be); `lan_hosts` are the cluster's own addresses, where
    /// plain http is allowed like on any private address.
    pub fn validate(&self, has_secret: impl Fn(&str) -> bool, lan_hosts: &[String]) -> Result<(), String> {
        let id_ok = !self.id.is_empty() && self.id.len() <= 40 && self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !id_ok {
            return Err(format!("\"{}\" isn't a usable channel id.", self.id));
        }
        if self.name.trim().is_empty() {
            return Err("Give the channel a name.".into());
        }
        match self.kind {
            Kind::Mac => {}
            Kind::Ntfy => {
                if blank(&self.topic) {
                    return Err(format!("{}: enter the ntfy topic.", self.name));
                }
                if let Some(s) = self.server.as_deref().filter(|s| !s.trim().is_empty()) {
                    safe_url(s, "the token and every alert", lan_hosts).map_err(|e| format!("{}: {e}", self.name))?;
                }
            }
            Kind::Pushover => {
                if !has_secret("appToken") || !has_secret("userKey") {
                    return Err(format!("{}: Pushover needs the app token and your user key.", self.name));
                }
            }
            // The URL is a Keychain field: one on its way in is checked here, a saved one was when it was saved.
            Kind::Webhook => match self.url.as_deref().filter(|u| !u.trim().is_empty()) {
                Some(u) => {
                    url_check(Kind::Webhook, u, lan_hosts).map_err(|e| format!("{}: {e}", self.name))?;
                }
                None if !has_secret("url") => return Err(format!("{}: enter the webhook URL.", self.name)),
                None => {}
            },
            Kind::Heartbeat => {
                match self.url.as_deref().filter(|u| !u.trim().is_empty()) {
                    Some(u) => {
                        url_check(Kind::Heartbeat, u, lan_hosts).map_err(|e| format!("{}: {e}", self.name))?;
                    }
                    None if !has_secret("url") => return Err(format!("{}: enter the check's URL.", self.name)),
                    None => {}
                }
                if self.mac_only {
                    return Err(format!("{}: the dead-man's switch is pinged by the node agents, so it can't be this Mac only.", self.name));
                }
            }
            Kind::Email => {
                if blank(&self.host) || blank(&self.from) || blank(&self.to) {
                    return Err(format!("{}: email needs the SMTP server, a From address and a To address.", self.name));
                }
                for a in self.to.as_deref().unwrap_or("").split(',').map(str::trim).chain([self.from.as_deref().unwrap_or("").trim()]) {
                    if a.parse::<lettre::Address>().is_err() && a.parse::<lettre::message::Mailbox>().is_err() {
                        return Err(format!("{}: \"{a}\" isn't an email address.", self.name));
                    }
                }
                if !matches!(self.security.as_deref(), None | Some("starttls") | Some("ssl")) && !smtp::loopback(self.host.as_deref().unwrap_or("")) {
                    return Err(format!("{}: choose STARTTLS or SSL; SparkBox won't send a password unencrypted.", self.name));
                }
            }
        }
        Ok(())
    }

    /// What the channel list shows under the name: "ntfy.sh · sparkbox-alerts".
    pub fn target(&self) -> String {
        match self.kind {
            Kind::Mac => "Notification Center".into(),
            Kind::Ntfy => format!("{} · {}", self.server.as_deref().unwrap_or(ntfy::DEFAULT_SERVER).trim_start_matches("https://"), self.topic.as_deref().unwrap_or("")),
            Kind::Pushover => self.device.clone().filter(|d| !d.is_empty()).map(|d| format!("Device {d}")).unwrap_or_else(|| "All devices".into()),
            Kind::Webhook => self.url_host.clone().unwrap_or_default(),
            Kind::Email => self.to.clone().unwrap_or_default(),
            Kind::Heartbeat => self.url_host.as_deref().map(|h| heartbeat::service(&format!("https://{h}/"))).unwrap_or_default(),
        }
    }
}

/// A new webhook or dead-man's-switch URL, before it goes to the Keychain: an http(s) URL
/// that's safe to send to (`safe_url`). Returns its host for `url_host`.
pub fn url_check(kind: Kind, url: &str, lan_hosts: &[String]) -> Result<String, String> {
    let what = if kind == Kind::Heartbeat { "the check's address" } else { "every alert" };
    let u = safe_url(url, what, lan_hosts)?;
    Ok(u.host_str().unwrap_or_default().to_string())
}

/// An http(s) URL, or why not.
pub fn http_url(s: &str) -> Result<url::Url, String> {
    let u = url::Url::parse(s.trim()).map_err(|_| format!("\"{s}\" isn't a URL."))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(format!("\"{s}\" has to be an http or https URL."));
    }
    Ok(u)
}

/// Whether a host is only on the home network: this Mac, a cluster node, a private or
/// link-local address, or a local name (.local, .lan, .home.arpa, or no dots at all).
/// The same addresses Providers lets use plain http.
pub fn on_lan(host: &str, lan_hosts: &[String]) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || lan_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        Ok(std::net::IpAddr::V6(v6)) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
        Err(_) => !host.contains('.') || [".local", ".lan", ".home.arpa", ".internal"].iter().any(|s| host.ends_with(s)),
    }
}

/// An https URL, or plain http to an address on the home network (`on_lan`). `what` is
/// what plain http would expose: "Use https for ntfy.example.com: plain http would send
/// the token and every alert unencrypted."
pub fn safe_url(s: &str, what: &str, lan_hosts: &[String]) -> Result<url::Url, String> {
    let u = http_url(s)?;
    let host = u.host_str().unwrap_or_default().to_string();
    if u.scheme() == "http" && !on_lan(&host, lan_hosts) {
        return Err(format!("Use https for {host}: plain http would send {what} unencrypted."));
    }
    Ok(u)
}

/// What gets sent. The same words go to every channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub title: String,
    pub body: String,
    pub severity: Severity,
    /// firing | resolved | test
    pub state: String,
    pub node: Option<String>,
    pub rule: Option<String>,
    pub alert_id: Option<String>,
    pub ts_ms: u64,
}

/// Where the HTTP channels send; tests point these at local servers.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub pushover: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints { pushover: pushover::API.into() }
    }
}

/// Why a delivery failed, and whether trying again could help.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub message: String,
    pub retry: bool,
}

impl Failure {
    pub fn new(message: impl Into<String>, retry: bool) -> Failure {
        Failure { message: message.into(), retry }
    }
}

/// An HTTP status as a sentence that says what to fix.
pub fn http_failure(status: u16, detail: Option<&str>, secret_word: &str) -> Failure {
    let hint = match status {
        401 | 403 => format!("check the {secret_word}"),
        404 => "check the address".into(),
        413 => "the message was too large".into(),
        429 => "rate limited; it will retry".into(),
        500..=599 => "the server had a problem; it will retry".into(),
        _ => detail.unwrap_or("the server refused it").to_string(),
    };
    let hint = match detail {
        Some(d) if !d.is_empty() && matches!(status, 400 | 401 | 403 | 422) => format!("{hint} ({d})"),
        _ => hint,
    };
    Failure::new(format!("HTTP {status} — {hint}"), status == 429 || status >= 500)
}

/// "Couldn't reach ntfy.sh: connection refused".
pub fn network_failure(host: &str, e: &reqwest::Error) -> Failure {
    let why = if e.is_timeout() {
        "no answer in 15 s".to_string()
    } else if e.is_connect() {
        "couldn't connect".to_string()
    } else {
        e.to_string()
    };
    Failure::new(format!("Couldn't reach {host}: {why}"), true)
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(15)).connect_timeout(Duration::from_secs(8)).build().unwrap_or_default()
}

/// Sends one message on one channel. `secret(field)` reads the channel's Keychain fields.
pub async fn send(ch: &Channel, secret: &(dyn Fn(&str) -> Option<String> + Sync), msg: &Message, http: &reqwest::Client, ep: &Endpoints) -> Result<(), Failure> {
    match ch.kind {
        Kind::Mac => Err(Failure::new("This Mac's notifications are shown by notify.rs", false)),
        Kind::Heartbeat => heartbeat::ping(&secret("url").or_else(|| ch.url.clone()).unwrap_or_default(), http).await.map(|_| ()),
        Kind::Ntfy => ntfy::send(ch, secret("token"), msg, http).await,
        Kind::Pushover => pushover::send(ch, secret("appToken"), secret("userKey"), msg, http, &ep.pushover).await,
        Kind::Webhook => webhook::send(ch, secret("url").or_else(|| ch.url.clone()), secret("secret"), msg, http).await,
        Kind::Email => smtp::send(ch, secret("password"), msg).await,
    }
}

/// HMAC-SHA256, hex: the webhook's signature (RFC 2104 over sha2, no extra crate).
pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut k = if key.len() > BLOCK { Sha256::digest(key).to_vec() } else { key.to_vec() };
    k.resize(BLOCK, 0);
    let inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let mut h = Sha256::new();
    h.update(&inner);
    h.update(data);
    let inner_hash = h.finalize();
    let mut h = Sha256::new();
    h.update(&outer);
    h.update(inner_hash);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// What happened to one alert event on one channel (or, when it wasn't sent anywhere, why).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Delivered (or, for a test, answered).
    #[default]
    Sent,
    /// Its incident was paged a moment ago; the next message about it lists this one.
    Coalesced,
    /// The same alert, at the same severity, was sent under 5 minutes ago (a restart re-raised it).
    Duplicate,
    /// A warning during quiet hours or with critical-only on, or a quiet resolve.
    Held,
    Silenced,
    /// Work SparkBox started (a restart, a shutdown).
    Planned,
    /// Went from critical down to warning.
    Eased,
    /// A resolve of something that was never paged, or a switch that's off.
    NotPaged,
    Failed,
    /// Demo: recorded, not sent.
    Demo,
    /// Left to the node's agent, which pages for it while it's healthy and current.
    Agent,
    // ── r4-alerts ──
    /// A dev run with SPARKBOX_DEV_MUTE_NOTIFY=1: decided and recorded, never shown or sent.
    Muted,
    /// Raised again within 10 minutes of its resolve: recorded on its incident, not paged again.
    Flapping,
}

/// One attempt to deliver, for the log and the channel's "Sent 2 min ago".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub ts_ms: u64,
    pub channel: String,
    pub channel_name: String,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    pub severity: Severity,
    pub ok: bool,
    pub error: Option<String>,
    /// "mac", or "agent:<node>" for a test sent by a node agent.
    pub via: String,
    /// Demo: recorded, not sent.
    #[serde(default)]
    pub demo: bool,
    #[serde(default)]
    pub test: bool,
    #[serde(default = "one")]
    pub attempts: u32,
    #[serde(default)]
    pub outcome: Outcome,
    /// Shared by every event that went out in one (grouped) message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident: Option<String>,
    /// Why it wasn't sent: "Quiet hours, 22:00 to 07:00 · America/New_York", the silence's reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A failed delivery that will be tried again then…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_ms: Option<u64>,
    /// …out of this many tries in all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    /// How long the channel took to accept it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

fn one() -> u32 {
    1
}

impl Delivery {
    /// A row for `ch` (or for no channel, when nothing was sent) with the rest left empty.
    pub fn row(ch: Option<&Channel>, title: &str, severity: Severity, outcome: Outcome) -> Delivery {
        Delivery {
            id: delivery_id(),
            ts_ms: crate::history::now_ms(),
            channel: ch.map(|c| c.id.clone()).unwrap_or_default(),
            channel_name: ch.map(|c| c.name.clone()).unwrap_or_default(),
            kind: ch.map(|c| c.kind).unwrap_or(Kind::Mac),
            title: title.into(),
            body: String::new(),
            severity,
            ok: !matches!(outcome, Outcome::Failed),
            error: None,
            via: "mac".into(),
            demo: false,
            test: false,
            attempts: 1,
            outcome,
            message_id: None,
            rule: None,
            alert_id: None,
            incident: None,
            reason: None,
            next_retry_ms: None,
            max_attempts: None,
            latency_ms: None,
        }
    }
}

/// The delivery log: the last 100 in memory and in deliveries.jsonl. A retry replaces
/// its own entry instead of adding one.
pub struct Log {
    path: Option<PathBuf>,
    entries: Mutex<VecDeque<Delivery>>,
}

impl Log {
    pub fn open(path: Option<PathBuf>) -> Log {
        let mut entries = VecDeque::new();
        if let Some(text) = path.as_ref().and_then(|p| fs::read_to_string(p).ok()) {
            for line in text.lines() {
                if let Ok(mut d) = serde_json::from_str::<Delivery>(line) {
                    // Rows from before outcomes existed: say what `ok` and `demo` said.
                    if d.outcome == Outcome::Sent && (!d.ok || d.demo) {
                        d.outcome = if d.demo { Outcome::Demo } else { Outcome::Failed };
                    }
                    entries.retain(|e: &Delivery| e.id != d.id);
                    entries.push_back(d);
                    if entries.len() > LOG_KEEP {
                        entries.pop_front();
                    }
                }
            }
        }
        let log = Log { path, entries: Mutex::new(entries) };
        log.rewrite();
        log
    }

    fn rewrite(&self) {
        let Some(path) = &self.path else { return };
        let text: String = self.entries.lock().unwrap().iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
        let _ = crate::util::write_atomic(path, text.as_bytes());
    }

    pub fn record(&self, d: Delivery) {
        let mut entries = self.entries.lock().unwrap();
        let replaced = entries.iter().position(|e| e.id == d.id).map(|i| entries.remove(i)).is_some();
        entries.push_back(d.clone());
        while entries.len() > LOG_KEEP {
            entries.pop_front();
        }
        drop(entries);
        if replaced {
            self.rewrite();
        } else if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let (Ok(line), Ok(mut f)) = (serde_json::to_string(&d), fs::OpenOptions::new().create(true).append(true).open(path)) {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    /// Newest first.
    #[cfg(test)]
    pub fn list(&self, limit: usize) -> Vec<Delivery> {
        self.entries.lock().unwrap().iter().rev().take(limit).cloned().collect()
    }

    /// The newest delivery per channel (rows that went out or failed; not the outcomes of
    /// events that weren't sent).
    pub fn last_by_channel(&self) -> HashMap<String, Delivery> {
        self.last_by_channel_where(|_| true)
    }

    /// The newest `limit` rows of one data source, newest first: Live never lists Demo's
    /// deliveries, nor Demo Live's. A Test is a real send from either and shows in both.
    pub fn list_in(&self, limit: usize, demo: bool) -> Vec<Delivery> {
        self.entries.lock().unwrap().iter().rev().filter(|d| d.test || d.demo == demo).take(limit).cloned().collect()
    }

    /// `last_by_channel` for one data source (a Test counts in both).
    pub fn last_by_channel_in(&self, demo: bool) -> HashMap<String, Delivery> {
        self.last_by_channel_where(|d| d.test || d.demo == demo)
    }

    fn last_by_channel_where(&self, keep: impl Fn(&Delivery) -> bool) -> HashMap<String, Delivery> {
        let mut out = HashMap::new();
        for d in self.entries.lock().unwrap().iter().filter(|d| keep(d) && !d.channel.is_empty() && matches!(d.outcome, Outcome::Sent | Outcome::Failed | Outcome::Demo)) {
            out.insert(d.channel.clone(), d.clone());
        }
        out
    }

    /// The newest failure (a delivery, or a test from a node) since `since_ms` on a channel
    /// that hasn't delivered since.
    pub fn last_failure(&self, since_ms: u64) -> Option<Delivery> {
        let entries = self.entries.lock().unwrap();
        let mut fine_since: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
        for d in entries.iter().rev() {
            let key = (d.channel.clone(), d.via.clone());
            match d.outcome {
                Outcome::Sent => {
                    fine_since.insert(key);
                }
                Outcome::Failed if d.ts_ms >= since_ms && !fine_since.contains(&key) => return Some(d.clone()),
                _ => {}
            }
        }
        None
    }

    /// Records several rows with one write when any replaces an earlier one.
    pub fn record_all(&self, rows: Vec<Delivery>) {
        for d in rows {
            self.record(d);
        }
    }
}

pub fn delivery_id() -> String {
    format!("d-{}-{:06x}", crate::history::now_ms(), fastrand::u32(..0xff_ffff))
}
