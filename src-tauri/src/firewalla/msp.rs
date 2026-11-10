//! A read-only client for Firewalla's cloud (MSP) API, and what SparkBox understands of its
//! answers. The endpoints aren't documented: the facts here come from the source and fixtures of
//! github.com/kdesch5000/firewalla-snmp-proxy, written against a Switch SE. A Switch X is
//! unconfirmed, so every field is optional, and an answer of another shape is a plain message.
//!
//! What matters about this API, all learned the hard way by that project:
//! - An unknown path answers 200 with the web app's HTML, so the content type is what's trusted.
//! - The quota is unpublished and shows no headers. Past it, everything answers 429 and the
//!   lockout has outlasted its own `Retry-After: 3600` by hours (`Schedule`).
//! - The same API can reset counters, reboot a switch and power-cycle a port. `Request` is the
//!   whole of what this client can ask, and `get` is the only verb it has.

use std::time::{Duration, SystemTime};

use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TIMEOUT: Duration = Duration::from_secs(20);
/// A topology is a few kilobytes per switch. Far past that, it isn't what SparkBox asked for.
const MAX_BODY: usize = 4 * 1024 * 1024;

/// How often the cloud may be asked. It refreshes a port's counters about every 5 minutes.
pub const POLL_EVERY_MS: u64 = 15 * 60_000;
/// Test connection's own floor. The quota is unpublished and lockouts last hours, so no amount of
/// clicking may spend it.
pub const TEST_EVERY_MS: u64 = 5 * 60_000;
/// The first wait after a 429. It doubles with each one in a row, up to the cap.
pub const BACKOFF_MIN_MS: u64 = 3_600_000;
pub const BACKOFF_CAP_MS: u64 = 24 * 3_600_000;
/// A `Retry-After` is honoured up to this, so a bad header can't silence the poll for good.
const RETRY_AFTER_CAP_MS: u64 = 7 * 24 * 3_600_000;
/// After a refused token: nothing will change until a new one is saved.
const AUTH_WAIT_MS: u64 = 3_600_000;

/// Why a request failed, in the categories the panel explains differently.
#[derive(Clone, Debug, PartialEq)]
pub enum MspError {
    /// 429 or 503: stop calling. Retrying keeps the quota pinned and can extend a lockout.
    RateLimited { retry_after_s: Option<u64> },
    /// 401 or 403.
    Auth,
    /// 200 with something that isn't JSON: the web app's page, so a wrong domain or a path this
    /// account doesn't have.
    NotJson,
    /// JSON, but not the shape SparkBox knows. Says what was missing.
    Shape(String),
    /// A good answer that doesn't list this switch.
    NotListed(String),
    /// Look-only mode came on between two requests of a round.
    Held,
    /// Any other status.
    Http(u16),
    Timeout,
    Unreachable(String),
}

impl MspError {
    pub fn kind(&self) -> &'static str {
        match self {
            MspError::RateLimited { .. } => "rate-limited",
            MspError::Auth => "auth",
            MspError::NotJson => "not-json",
            MspError::Shape(_) => "shape",
            MspError::NotListed(_) => "not-listed",
            MspError::Held => "held",
            MspError::Http(_) => "http",
            MspError::Timeout | MspError::Unreachable(_) => "unreachable",
        }
    }

    /// A sentence for the panel and Test connection. Never holds the token: nothing here sees it.
    pub fn explain(&self) -> String {
        match self {
            MspError::RateLimited { .. } => "Firewalla's cloud asked SparkBox to wait: too many requests on this account.".into(),
            MspError::Auth => "Firewalla refused the token. Check it in Settings › Switches & gateway.".into(),
            MspError::NotJson => "Firewalla answered with a web page, not data. The MSP domain may be wrong, or this account may not have this API.".into(),
            MspError::Shape(what) => format!("SparkBox doesn't recognize Firewalla's answer ({what}), so it shows nothing from the cloud."),
            MspError::NotListed(what) => what.clone(),
            MspError::Held => "Look-only mode came on, so SparkBox stopped asking Firewalla.".into(),
            MspError::Http(code) => format!("Firewalla's cloud answered with an error (HTTP {code})."),
            MspError::Timeout => format!("Firewalla's cloud didn't answer within {} s.", TIMEOUT.as_secs()),
            MspError::Unreachable(why) => format!("Couldn't reach Firewalla's cloud: {why}"),
        }
    }
}

/// The MSP domain in the one form the client accepts: a host name under firewalla.net, with
/// `https://` and a trailing slash allowed around it. Nothing else can receive the token.
pub fn check_domain(text: &str) -> Result<String, String> {
    let t = text.trim().trim_end_matches('/').to_ascii_lowercase();
    let host = match t.split_once("://") {
        Some(("https", rest)) => rest,
        Some(_) => return Err("The MSP domain must use https.".into()),
        None => t.as_str(),
    };
    let label_ok = |l: &str| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-');
    let ok = host.len() <= 253 && host.ends_with(".firewalla.net") && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') && host.split('.').all(label_ok);
    if ok {
        Ok(host.to_string())
    } else {
        Err("That isn't an MSP domain. It looks like dn-abc123.firewalla.net.".into())
    }
}

/// A box's id, as `/v2/boxes` gave it. It goes into a path, so it can hold nothing that changes one.
#[derive(Clone, Debug, PartialEq)]
pub struct Gid(String);

impl Gid {
    pub fn new(text: &str) -> Result<Gid, MspError> {
        let ok = !text.is_empty() && text.len() <= 64 && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if ok {
            Ok(Gid(text.to_string()))
        } else {
            Err(MspError::Shape("a box's id isn't one".into()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Everything this client can ask. There's no way to name another path.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Boxes,
    Topology(Gid),
}

impl Request {
    pub fn path(&self) -> String {
        match self {
            Request::Boxes => "/v2/boxes".into(),
            Request::Topology(gid) => format!("/v2/boxes/{}/topology", gid.as_str()),
        }
    }
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    auth: HeaderValue,
}

impl Client {
    /// `domain` goes through `check_domain`. HTTPS only, and a redirect is an error, so the token
    /// can't be walked off to another host.
    pub fn new(domain: &str, token: &str) -> Result<Client, String> {
        let host = check_domain(domain)?;
        Client::build(format!("https://{host}"), token, true)
    }

    /// The test fake, on plain HTTP on this Mac.
    #[cfg(test)]
    pub fn plain(base: &str, token: &str) -> Result<Client, String> {
        Client::build(base.trim_end_matches('/').to_string(), token, false)
    }

    /// As `new` builds it, with any address: proves what `https_only` refuses.
    #[cfg(test)]
    pub fn strict(base: &str, token: &str) -> Result<Client, String> {
        Client::build(base.trim_end_matches('/').to_string(), token, true)
    }

    fn build(base: String, token: &str, https_only: bool) -> Result<Client, String> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("SparkBox")
            .build()
            .map_err(|e| e.to_string())?;
        let mut auth = HeaderValue::from_str(&format!("Token {}", token.trim())).map_err(|_| "The saved MSP token can't be sent: it holds a line break or another character a request can't carry.".to_string())?;
        auth.set_sensitive(true);
        Ok(Client { http, base, auth })
    }

    /// One GET. The only request this client can send.
    async fn get(&self, request: &Request) -> Result<Value, MspError> {
        let url = format!("{}{}", self.base, request.path());
        let resp = self.http.get(url).header(AUTHORIZATION, self.auth.clone()).header(ACCEPT, "application/json").send().await.map_err(classify)?;
        let status = resp.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
            let retry_after_s = resp.headers().get(RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| parse_retry_after(v, SystemTime::now()));
            return Err(MspError::RateLimited { retry_after_s });
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(MspError::Auth);
        }
        if !status.is_success() {
            return Err(MspError::Http(status.as_u16()));
        }
        // An unknown path is a 200 with HTML: the status alone says nothing.
        let json = resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.to_ascii_lowercase().contains("json"));
        if !json {
            return Err(MspError::NotJson);
        }
        serde_json::from_slice(&read_capped(resp).await?).map_err(|_| MspError::NotJson)
    }

    pub async fn boxes(&self) -> Result<Vec<Gid>, MspError> {
        parse_boxes(&self.get(&Request::Boxes).await?)
    }

    pub async fn topology(&self, gid: &Gid) -> Result<Value, MspError> {
        self.get(&Request::Topology(gid.clone())).await
    }
}

/// The body, a chunk at a time, so an endless answer stops at the cap and never fills memory.
async fn read_capped(mut resp: reqwest::Response) -> Result<Vec<u8>, MspError> {
    let long = || MspError::Shape("the answer is far too long".into());
    if resp.content_length().is_some_and(|n| n > MAX_BODY as u64) {
        return Err(long());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(classify)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(long());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn classify(e: reqwest::Error) -> MspError {
    if e.is_timeout() {
        return MspError::Timeout;
    }
    // The chain's last link says what happened; the URL holds only the domain.
    let mut why: &dyn std::error::Error = &e;
    while let Some(next) = why.source() {
        why = next;
    }
    MspError::Unreachable(why.to_string())
}

/// `Retry-After` as seconds from `now`: delta-seconds or an HTTP-date. None for anything else, and
/// the caller's own schedule stands.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<u64> {
    let v = value.trim();
    if let Ok(s) = v.parse::<u64>() {
        return Some(s);
    }
    let at = httpdate::parse_http_date(v).ok()?;
    Some(at.duration_since(now).map(|d| d.as_secs()).unwrap_or(0))
}

/// When the cloud may be asked next. Saved, so a restart can't ask sooner: a fresh request during
/// a lockout only collects another `Retry-After` and pushes recovery further away.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Schedule {
    pub next_ms: u64,
    /// 429s in a row.
    pub strikes: u32,
    pub last_try_ms: u64,
}

impl Schedule {
    pub fn due(&self, now_ms: u64) -> bool {
        now_ms >= self.next_ms
    }

    /// Still backing off from a 429.
    pub fn limited(&self, now_ms: u64) -> bool {
        self.strikes > 0 && !self.due(now_ms)
    }

    /// A request is about to leave: the next poll is already 15 minutes off, and that's saved first,
    /// so a process that dies waiting for the answer can't ask again when it's started.
    pub fn asking(&mut self, now_ms: u64) {
        self.last_try_ms = now_ms;
        self.next_ms = self.next_ms.max(now_ms + POLL_EVERY_MS);
    }

    /// Any answer that isn't a quota signal: the normal wait, and the strikes are over.
    pub fn answered(&mut self, now_ms: u64) {
        self.strikes = 0;
        self.next_ms = now_ms + POLL_EVERY_MS;
    }

    /// No answer, or one that says nothing about the quota: the normal wait, strikes kept.
    pub fn failed(&mut self, now_ms: u64) {
        self.next_ms = self.next_ms.max(now_ms + POLL_EVERY_MS);
    }

    pub fn refused(&mut self, now_ms: u64) {
        self.next_ms = self.next_ms.max(now_ms + AUTH_WAIT_MS);
    }

    /// A 429: at least an hour, doubling each time, and never less than the server asked for.
    pub fn rate_limited(&mut self, now_ms: u64, retry_after_s: Option<u64>) {
        self.strikes = self.strikes.saturating_add(1);
        let doubled = BACKOFF_MIN_MS.saturating_mul(1 << (self.strikes - 1).min(10)).min(BACKOFF_CAP_MS);
        let asked = retry_after_s.unwrap_or(0).saturating_mul(1000).min(RETRY_AFTER_CAP_MS);
        self.next_ms = now_ms + doubled.max(asked);
    }
}

// ---- The answers

/// What `/v2/boxes` must give: at least one box with a `gid`.
pub fn parse_boxes(v: &Value) -> Result<Vec<Gid>, MspError> {
    let boxes = v.as_array().ok_or_else(|| MspError::Shape("the list of boxes isn't a list".into()))?;
    let gids: Vec<Gid> = boxes.iter().filter_map(|b| b.get("gid")?.as_str()).filter_map(|g| Gid::new(g).ok()).collect();
    if boxes.is_empty() {
        return Err(MspError::NotListed("This token sees no Firewalla boxes.".into()));
    }
    if gids.is_empty() {
        return Err(MspError::Shape("no box has a gid".into()));
    }
    Ok(gids)
}

/// How to recognize the Switch X in a topology: its address, or a MAC LLDP heard from it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wanted {
    pub host: String,
    pub macs: Vec<String>,
}

fn same_mac(a: &str, b: &str) -> bool {
    let norm = |m: &str| m.replace('-', ":").to_ascii_lowercase();
    !a.is_empty() && norm(a) == norm(b)
}

/// One port as the cloud reports it. `None` means Firewalla didn't say, never zero: a field the
/// Switch X leaves out mustn't read as a clean counter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MspPort {
    pub port: u32,
    pub link_up: Option<bool>,
    /// In Mb/s (1000 on a Switch SE's gigabit link). Left out while the link is down.
    pub link_speed_mbps: Option<u32>,
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub rx_errors: Option<u64>,
    pub tx_errors: Option<u64>,
    pub rx_discards: Option<u64>,
    pub tx_discards: Option<u64>,
    /// When the counters last started from zero (Unix seconds).
    pub stats_since_s: Option<u64>,
    pub poe_status: Option<String>,
    pub poe_mode: Option<String>,
    pub poe_watts: Option<f64>,
    pub stp_state: Option<String>,
    pub stp_role: Option<String>,
    /// "access" or "trunk".
    pub port_type: Option<String>,
    pub sfp: Option<MspSfp>,
    /// The port that leads up to the Firewalla box.
    pub uplink: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MspSfp {
    pub present: Option<bool>,
    pub connector: Option<String>,
    pub bit_rate: Option<String>,
    pub vendor: Option<String>,
}

/// The switch as the cloud reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MspSwitch {
    pub mac: Option<String>,
    pub name: Option<String>,
    pub ip: Option<String>,
    pub model: Option<String>,
    pub version: Option<String>,
    pub online: Option<bool>,
    pub ports: Vec<MspPort>,
    /// Port entries SparkBox couldn't place on the panel, or that repeat a port, left out.
    pub skipped: u32,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// A count that may come as a number or, as `port` does, as a string of digits.
fn count(v: &Value, key: &str) -> Option<u64> {
    match v.get(key)? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn parse_port(p: &Value) -> Option<MspPort> {
    let port = u32::try_from(count(p, "port")?).ok().filter(|n| (1..=super::labels::PORTS).contains(n))?;
    let stp = p.get("stp").filter(|s| s.is_object());
    let settings = p.get("settings").filter(|s| s.is_object());
    let sfp = p.get("sfpInfo").filter(|s| s.is_object()).map(|s| MspSfp {
        present: s.get("present").and_then(Value::as_bool),
        connector: text(s, "connectorType"),
        bit_rate: text(s, "nominalBitRate"),
        vendor: text(s, "vendorOui"),
    });
    Some(MspPort {
        port,
        link_up: p.get("linkUp").and_then(Value::as_bool),
        link_speed_mbps: count(p, "linkSpeed").and_then(|s| u32::try_from(s).ok()).filter(|s| *s > 0),
        rx_bytes: count(p, "rxBytes"),
        tx_bytes: count(p, "txBytes"),
        rx_errors: count(p, "rxErrorFrames"),
        tx_errors: count(p, "txErrorFrames"),
        rx_discards: count(p, "rxDiscardFrames"),
        tx_discards: count(p, "txDiscardFrames"),
        stats_since_s: count(p, "statsSinceTs"),
        poe_status: text(p, "poeStatus"),
        poe_mode: text(p, "poeMode"),
        poe_watts: p.get("poePower").and_then(Value::as_f64),
        stp_state: stp.and_then(|s| text(s, "portState")),
        stp_role: stp.and_then(|s| text(s, "portRole")),
        port_type: settings.and_then(|s| text(s, "type")),
        sfp,
        uplink: p.get("uplink").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn parse_switch(node: &Value) -> Result<MspSwitch, MspError> {
    let raw = node.get("ports").and_then(Value::as_array).filter(|p| !p.is_empty()).ok_or_else(|| MspError::Shape("the switch has no list of ports".into()))?;
    let mut ports: Vec<MspPort> = raw.iter().filter_map(parse_port).collect();
    if ports.is_empty() {
        return Err(MspError::Shape("none of its ports has a number".into()));
    }
    if ports.iter().all(|p| p.link_up.is_none()) {
        return Err(MspError::Shape("no port says whether it has a link".into()));
    }
    let uplink = node.get("uplink").and_then(|u| count(u, "localPort"));
    for p in &mut ports {
        p.uplink |= uplink == Some(p.port as u64);
    }
    // The API's numbers are strings, so its order can't be trusted.
    ports.sort_by_key(|p| p.port);
    // A port listed twice keeps its first entry, and no speed if the two disagree. If they
    // disagree on the link itself, neither is believed, so the port isn't lit from this answer.
    ports.dedup_by(|again, first| {
        let same = again.port == first.port;
        if same && (again.link_speed_mbps != first.link_speed_mbps || again.link_up != first.link_up) {
            first.link_speed_mbps = None;
        }
        if same && again.link_up != first.link_up {
            first.link_up = None;
        }
        same
    });
    Ok(MspSwitch {
        mac: text(node, "mac").or_else(|| text(node, "id")),
        name: text(node, "name"),
        ip: text(node, "ip"),
        model: text(node, "model"),
        version: text(node, "version"),
        online: node.get("online").and_then(Value::as_bool),
        skipped: (raw.len() - ports.len()) as u32,
        ports,
    })
}

/// Finds the wanted switch in `/v2/boxes/<gid>/topology` (`{"nodes":[…]}`, the switch being the
/// node with `"type":"switch"`). By address first, because the owner typed it, then by MAC. With
/// no match it says so, even when there's one switch to pick, because a guess here would paint
/// another switch's ports.
pub fn parse_topology(v: &Value, want: &Wanted) -> Result<MspSwitch, MspError> {
    let nodes = v.get("nodes").and_then(Value::as_array).ok_or_else(|| MspError::Shape("it has no list of devices".into()))?;
    let switches: Vec<&Value> = nodes.iter().filter(|n| text(n, "type").is_some_and(|t| t.eq_ignore_ascii_case("switch"))).collect();
    if switches.is_empty() {
        return Err(MspError::NotListed("Firewalla lists no switch on this box.".into()));
    }
    let host = want.host.trim();
    let by_address = |n: &&&Value| !host.is_empty() && ["ip", "reservedIP"].iter().filter_map(|k| text(n, k)).any(|a| a == host);
    let by_mac = |n: &&&Value| ["mac", "id"].iter().filter_map(|k| text(n, k)).any(|m| want.macs.iter().any(|w| same_mac(w, &m)));
    match switches.iter().find(by_address).or_else(|| switches.iter().find(by_mac)) {
        Some(n) => parse_switch(n),
        None => {
            let n = switches.len();
            Err(MspError::NotListed(format!("Firewalla lists {n} switch{}, and none has the address {host} or a MAC LLDP heard from this switch.", if n == 1 { "" } else { "es" })))
        }
    }
}
