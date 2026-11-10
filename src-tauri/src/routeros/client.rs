//! A RouterOS 7 REST client for the CRS804: Basic auth over HTTPS, with the switch's
//! self-signed certificate pinned on first use (its SHA-256 must match what the user
//! trusted). RouterOS returns every value as a string ("true", "45C", "-2.1dBm"), so
//! everything here is parsed defensively.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const TIMEOUT: Duration = Duration::from_secs(5);
const AUTH_RETRY: Duration = Duration::from_secs(30);

/// Unsupported optional data may be absent. A rejected login or certificate is
/// never optional: stop this poll instead of publishing an apparently healthy switch.
pub(super) fn optional_read<T: Default>(result: Result<T, ApiError>) -> Result<T, ApiError> {
    match result {
        Err(e @ (ApiError::Auth | ApiError::TlsUntrusted { .. } | ApiError::TlsChanged { .. })) => Err(e),
        other => Ok(other.unwrap_or_default()),
    }
}

/// SHA-256 of a certificate (DER), as 64 lowercase hex digits.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect()
}

/// Groups a fingerprint for people to compare: "3A:7F:…".
pub fn pretty_fingerprint(hex: &str) -> String {
    hex.as_bytes().chunks(2).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect::<Vec<_>>().join(":")
}

/// The pinning rule on its own: the certificate must be the one trusted, and nothing is trusted by default.
pub fn check_pin(pin: Option<&str>, der: &[u8]) -> Result<(), PinProblem> {
    let seen = fingerprint(der);
    match pin {
        Some(p) if p.eq_ignore_ascii_case(&seen) => Ok(()),
        Some(p) => Err(PinProblem::Changed { pinned: p.to_string(), seen }),
        None => Err(PinProblem::Untrusted { seen }),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PinProblem {
    Untrusted { seen: String },
    Changed { pinned: String, seen: String },
}

/// Accepts exactly one certificate (by fingerprint) and remembers what the switch presented.
#[derive(Debug)]
struct PinVerifier {
    pin: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap() = Some(fingerprint(end_entity.as_ref()));
        check_pin(self.pin.as_deref(), end_entity.as_ref())
            .map(|_| ServerCertVerified::assertion())
            .map_err(|_| rustls::Error::General("certificate not trusted".into()))
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Why a request failed, in the categories the UI explains differently.
#[derive(Clone, Debug, PartialEq)]
pub enum ApiError {
    /// First contact, or a certificate nobody trusted yet: show the fingerprint.
    TlsUntrusted { fingerprint: String },
    TlsChanged { pinned: String, fingerprint: String },
    Auth,
    /// Nothing answers HTTPS, or there's no /rest (www-ssl off, RouterOS 6).
    RestDisabled(String),
    Timeout,
    Unreachable(String),
    Other(String),
}

impl ApiError {
    pub fn kind(&self) -> &'static str {
        match self {
            ApiError::TlsUntrusted { .. } => "tls-untrusted",
            ApiError::TlsChanged { .. } => "tls-changed",
            ApiError::Auth => "auth",
            ApiError::RestDisabled(_) => "rest-disabled",
            ApiError::Timeout => "timeout",
            ApiError::Unreachable(_) => "unreachable",
            ApiError::Other(_) => "other",
        }
    }

    /// The certificate to offer for trust. Only a certificate failure has one: a wrong password
    /// past a pinned certificate isn't fixed by trusting it again.
    pub fn cert_to_trust(&self) -> Option<&str> {
        match self {
            ApiError::TlsUntrusted { fingerprint } | ApiError::TlsChanged { fingerprint, .. } => Some(fingerprint),
            _ => None,
        }
    }

    /// A sentence for the UI. `user`, `host` and `via_vpn` fill in the details.
    pub fn explain(&self, user: &str, host: &str, via_vpn: bool) -> String {
        match self {
            ApiError::TlsUntrusted { fingerprint } => format!(
                "{host} presented a certificate SparkBox hasn't trusted yet (SHA-256 {}). Compare it with System › Certificates on the switch, then choose Trust.",
                pretty_fingerprint(fingerprint)
            ),
            ApiError::TlsChanged { pinned, fingerprint } => format!(
                "{host}'s certificate changed (was {}, now {}). If you replaced it on the switch, trust the new one; otherwise something may be intercepting the connection.",
                pretty_fingerprint(pinned),
                pretty_fingerprint(fingerprint)
            ),
            ApiError::Auth => format!("{host} refused user \"{user}\" (401). Check the saved username and REST password in Settings › Switches & gateway, the user's allowed client addresses and REST access policies. This response alone does not identify which check failed."),
            ApiError::RestDisabled(why) => format!(
                "{host} doesn't offer the REST API ({why}). Enable www-ssl with a certificate in IP › Services (RouterOS 7.1 or later)."
            ),
            ApiError::Timeout => format!("No answer from {host} within {} s{}.", TIMEOUT.as_secs(), if via_vpn { " through the VPN" } else { "" }),
            ApiError::Unreachable(why) => format!("Couldn't reach {host}{}: {why}", if via_vpn { " through the VPN" } else { "" }),
            ApiError::Other(why) => why.clone(),
        }
    }
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    user: String,
    password: String,
    pin: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    auth_rejected_at: Mutex<Option<Instant>>,
}

impl Client {
    /// `base` is "https://192.168.50.2" (or "http://…" in tests). `proxy` is the VPN's SOCKS URL when tunnelled.
    pub fn new(base: &str, user: &str, password: &str, pin: Option<String>, proxy: Option<String>) -> Result<Client, String> {
        let seen = Arc::new(Mutex::new(None));
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinVerifier { pin: pin.clone(), seen: seen.clone(), provider }))
            .with_no_client_auth();
        let mut builder = reqwest::Client::builder().use_preconfigured_tls(tls).timeout(TIMEOUT).connect_timeout(TIMEOUT);
        if let Some(p) = proxy {
            builder = builder.proxy(reqwest::Proxy::all(p).map_err(|e| e.to_string())?);
        }
        Ok(Client {
            http: builder.build().map_err(|e| e.to_string())?,
            base: base.trim_end_matches('/').to_string(),
            user: user.to_string(),
            password: password.to_string(),
            pin,
            seen,
            auth_rejected_at: Mutex::new(None),
        })
    }

    /// The certificate the switch presented on the last connection.
    pub fn seen_fingerprint(&self) -> Option<String> {
        self.seen.lock().unwrap().clone()
    }

    pub(super) fn auth_retry_pending(&self, now: Instant) -> bool {
        self.auth_rejected_at.lock().unwrap().is_some_and(|at| now.saturating_duration_since(at) < AUTH_RETRY)
    }

    fn classify(&self, e: reqwest::Error) -> ApiError {
        // The verifier saw a certificate that isn't the pinned one: that's why it failed.
        if let Some(seen) = self.seen_fingerprint() {
            if !self.pin.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(&seen)) {
                return match &self.pin {
                    Some(p) => ApiError::TlsChanged { pinned: p.clone(), fingerprint: seen },
                    None => ApiError::TlsUntrusted { fingerprint: seen },
                };
            }
        }
        if e.is_timeout() {
            return ApiError::Timeout;
        }
        let text = format!("{e:?}");
        if e.is_connect() {
            if text.contains("Connection refused") || text.contains("ConnectionRefused") {
                return ApiError::RestDisabled("nothing listens on its HTTPS port".into());
            }
            return ApiError::Unreachable(root_cause(&e));
        }
        ApiError::Other(root_cause(&e))
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value, ApiError> {
        let resp = req.basic_auth(&self.user, Some(&self.password)).send().await.map_err(|e| self.classify(e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            *self.auth_rejected_at.lock().unwrap() = Some(Instant::now());
            return Err(ApiError::Auth);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(ApiError::RestDisabled("no /rest on this router".into()));
        }
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let why = body.get("detail").or_else(|| body.get("message")).and_then(Value::as_str).unwrap_or("request failed");
            return Err(ApiError::Other(format!("The switch said: {why} ({status})")));
        }
        *self.auth_rejected_at.lock().unwrap() = None;
        Ok(body)
    }

    pub async fn get(&self, path: &str) -> Result<Value, ApiError> {
        self.send(self.http.get(format!("{}/rest{path}", self.base))).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        self.send(self.http.post(format!("{}/rest{path}", self.base)).json(&body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        self.send(self.http.patch(format!("{}/rest{path}", self.base)).json(&body)).await
    }

    // ---- The calls SparkBox makes

    pub async fn resource(&self) -> Result<Resource, ApiError> {
        Ok(parse_resource(&self.get("/system/resource").await?))
    }

    pub async fn health(&self) -> Result<Vec<crate::model::Sensor>, ApiError> {
        Ok(parse_health(&self.get("/system/health").await?))
    }

    pub async fn interfaces(&self) -> Result<Vec<Iface>, ApiError> {
        let fields = ".id,name,type,running,disabled,comment,mtu,l2mtu,actual-mtu,mac-address,rx-byte,tx-byte,rx-error,tx-error,rx-drop,tx-drop,link-downs,last-link-up-time";
        Ok(parse_interfaces(&self.get(&format!("/interface?.proplist={fields}")).await?))
    }

    /// One snapshot of `/interface ethernet monitor` for these ports: rate, FEC and optics per lane.
    pub async fn monitor(&self, names: &[String]) -> Result<Vec<Monitor>, ApiError> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let v = self.post("/interface/ethernet/monitor", json!({ "numbers": names.join(","), "once": "" })).await?;
        Ok(v.as_array().map(|a| a.iter().map(parse_monitor).collect()).unwrap_or_default())
    }

    /// FCS errors and pause frames, from the ethernet statistics (not every model reports them).
    pub async fn ethernet_stats(&self) -> Result<BTreeMap<String, (u64, u64)>, ApiError> {
        let v = self.post("/interface/ethernet/print", json!({ "stats": "", ".proplist": "name,rx-fcs-error,rx-pause" })).await?;
        Ok(v.as_array()
            .map(|a| a.iter().filter_map(|x| Some((str_of(x, "name")?, (uint(x, "rx-fcs-error").unwrap_or(0), uint(x, "rx-pause").unwrap_or(0))))).collect())
            .unwrap_or_default())
    }

    pub async fn set_disabled(&self, id: &str, disabled: bool) -> Result<(), ApiError> {
        self.patch(&format!("/interface/ethernet/{}", encode_id(id)), json!({ "disabled": if disabled { "true" } else { "false" } })).await.map(drop)
    }

    pub async fn set_comment(&self, id: &str, comment: &str) -> Result<(), ApiError> {
        self.patch(&format!("/interface/ethernet/{}", encode_id(id)), json!({ "comment": comment })).await.map(drop)
    }

    pub async fn blink(&self, name: &str) -> Result<(), ApiError> {
        self.post("/interface/ethernet/blink", json!({ "numbers": name, "duration": "10s" })).await.map(drop)
    }

    pub async fn reset_counters(&self, name: &str) -> Result<(), ApiError> {
        self.post("/interface/reset-counters", json!({ "numbers": name })).await.map(drop)
    }

    /// Who is behind each port: the bridge host table, the switch chip's host table,
    /// LLDP/MNDP neighbors, the switch's own addresses and its ARP table. A table the
    /// model doesn't have (no bridge, no switch chip menu) is left empty.
    pub async fn port_tables(&self) -> Result<crate::routeros::mapping::PortTables, ApiError> {
        use crate::routeros::mapping as m;
        let hosts = optional_read(self.get("/interface/bridge/host").await.map(|v| m::parse_bridge_hosts(&v)))?;
        let chip = optional_read(self.get("/interface/ethernet/switch/host").await.map(|v| m::parse_switch_hosts(&v)))?;
        let neighbors = optional_read(self.get("/ip/neighbor").await.map(|v| m::parse_neighbors(&v)))?;
        let addresses = optional_read(self.get("/ip/address").await.map(|v| m::parse_addresses(&v)))?;
        let arp = optional_read(self.get("/ip/arp").await.map(|v| m::parse_arp(&v)))?;
        let mut all = hosts;
        for h in chip {
            if !all.iter().any(|x| x.mac == h.mac) {
                all.push(h);
            }
        }
        Ok(m::PortTables { hosts: all, neighbors, addresses, arp })
    }

    /// Runs `source` on the switch itself: adds it as a script, runs it, removes it.
    /// The script carries on even if this connection drops (a bounce re-enables its port).
    pub async fn run_script(&self, name: &str, source: &str) -> Result<(), ApiError> {
        let added = self.put("/system/script", json!({ "name": name, "source": source, "policy": "read,write,test" })).await?;
        let id = str_of(&added, ".id").unwrap_or_else(|| name.to_string());
        let run = self.post("/system/script/run", json!({ ".id": id })).await;
        let _ = self.delete(&format!("/system/script/{}", encode_id(&id))).await;
        run.map(drop)
    }

    /// Schedules `on_event` to run once, `delay_s` from now by the switch's own clock; returns the
    /// scheduler entry's id. The entry gets an explicit start date and time and no interval (an
    /// entry with an interval and no start time can run the moment it's added, then repeat), and
    /// is read back: if the switch's `next-run` isn't that time, the entry is removed and this fails.
    pub async fn schedule_once(&self, name: &str, delay_s: u32, on_event: &str) -> Result<String, ApiError> {
        let clock = self.get("/system/clock").await?;
        let now = SwitchTime::parse(&str_of(&clock, "date").unwrap_or_default(), &str_of(&clock, "time").unwrap_or_default())
            .ok_or_else(|| ApiError::Other("The switch's clock couldn't be read, so no revert was scheduled.".into()))?;
        let at = now.plus(delay_s);
        let added = self
            .put(
                "/system/scheduler",
                json!({ "name": name, "start-date": at.date(), "start-time": at.time(), "interval": "0s", "on-event": on_event, "policy": "read,write,test" }),
            )
            .await?;
        let id = str_of(&added, ".id").unwrap_or_else(|| name.to_string());
        let back = self.get(&format!("/system/scheduler/{}", encode_id(&id))).await;
        let next = back.as_ref().ok().and_then(|v| str_of(v, "next-run")).unwrap_or_default();
        let ok = SwitchTime::parse_stamp(&next).is_some_and(|n| n.seconds().abs_diff(at.seconds()) <= 2);
        if !ok {
            let _ = self.delete(&format!("/system/scheduler/{}", encode_id(&id))).await;
            let said = if next.is_empty() { "no next run".to_string() } else { format!("a next run of {next}") };
            return Err(ApiError::Other(format!("The switch reported {said} for the revert instead of {} {}, so it was removed.", at.date(), at.time())));
        }
        Ok(id)
    }

    pub async fn unschedule(&self, name: &str) -> Result<(), ApiError> {
        let rows = self.get(&format!("/system/scheduler?name={name}")).await?;
        for id in rows.as_array().map(|a| a.iter().filter_map(|r| str_of(r, ".id")).collect::<Vec<_>>()).unwrap_or_default() {
            self.delete(&format!("/system/scheduler/{}", encode_id(&id))).await?;
        }
        Ok(())
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        self.send(self.http.put(format!("{}/rest{path}", self.base)).json(&body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value, ApiError> {
        self.send(self.http.delete(format!("{}/rest{path}", self.base))).await
    }
}

/// A wall-clock time as the switch keeps it (its own time zone), in whole seconds. RouterOS 7.10
/// and later write dates as 2026-10-29; older ones as oct/29/2026. Both are read; times are
/// written back in the form the switch used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchTime {
    y: i64,
    m: u32,
    d: u32,
    secs: u32,
    iso: bool,
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

fn days_in(y: i64, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl SwitchTime {
    pub fn parse(date: &str, time: &str) -> Option<SwitchTime> {
        let date = date.trim();
        let (y, m, d, iso) = if let Some((mon, rest)) = date.split_once('/') {
            let m = MONTHS.iter().position(|x| x.eq_ignore_ascii_case(mon))? as u32 + 1;
            let (d, y) = rest.split_once('/')?;
            (y.parse().ok()?, m, d.parse().ok()?, false)
        } else {
            let mut it = date.split('-');
            (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?, true)
        };
        let mut t = time.trim().split(':').map(|x| x.parse::<u32>());
        let (h, mi, s) = (t.next()?.ok()?, t.next()?.ok()?, t.next().unwrap_or(Ok(0)).ok()?);
        ((1..=12).contains(&m) && d >= 1 && d <= days_in(y, m) && h < 24 && mi < 60 && s < 60).then_some(SwitchTime { y, m, d, secs: h * 3600 + mi * 60 + s, iso })
    }

    /// "2026-10-29 10:43:13" or "oct/29/2026 10:43:13", as `next-run` reads.
    pub fn parse_stamp(s: &str) -> Option<SwitchTime> {
        let (date, time) = s.trim().split_once(' ')?;
        SwitchTime::parse(date, time)
    }

    pub fn plus(self, s: u32) -> SwitchTime {
        let mut t = self;
        let total = t.secs + s;
        t.secs = total % 86_400;
        for _ in 0..total / 86_400 {
            t.d += 1;
            if t.d > days_in(t.y, t.m) {
                (t.d, t.m) = (1, t.m + 1);
                if t.m > 12 {
                    (t.m, t.y) = (1, t.y + 1);
                }
            }
        }
        t
    }

    pub fn date(&self) -> String {
        if self.iso {
            format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
        } else {
            format!("{}/{:02}/{:04}", MONTHS[self.m as usize - 1], self.d, self.y)
        }
    }

    pub fn time(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.secs / 3600, self.secs % 3600 / 60, self.secs % 60)
    }

    /// Seconds since 1970-01-01 in the switch's zone (only for comparing two of them).
    fn seconds(&self) -> u64 {
        let (y, m) = if self.m <= 2 { (self.y - 1, self.m + 9) } else { (self.y, self.m - 3) };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * m as i64 + 2) / 5 + self.d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        (days * 86_400 + self.secs as i64).max(0) as u64
    }
}

fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur: &dyn std::error::Error = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

/// RouterOS ids look like "*1A"; the star goes into the URL percent-encoded.
fn encode_id(id: &str) -> String {
    id.replace('*', "%2A")
}

// ---- Parsing RouterOS's string-typed JSON

fn str_of(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// The leading number of a value like "45C", "-2.1dBm", "3", 3 or "200Gbps".
pub fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let s = s.trim();
            let end = s.char_indices().find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+')))).map(|(i, _)| i).unwrap_or(s.len());
            s[..end].parse().ok()
        }
        _ => None,
    }
}

fn num(v: &Value, key: &str) -> Option<f64> {
    number(v.get(key)?)
}

fn uint(v: &Value, key: &str) -> Option<u64> {
    num(v, key).filter(|n| *n >= 0.0).map(|n| n as u64)
}

pub fn boolean(v: &Value, key: &str) -> Option<bool> {
    match v.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.as_str() {
            "true" | "yes" => Some(true),
            "false" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// RouterOS durations: "6w1d02:55:14", "1d23h12m3s", "3h4m", "45s", "02:00:01".
pub fn duration_s(s: &str) -> Option<u64> {
    let s = s.trim();
    // A clock (hh:mm:ss) may follow the weeks and days.
    let (units, clock) = match s.rfind(|c: char| c.is_ascii_alphabetic()) {
        Some(i) if s[i + 1..].contains(':') => (&s[..=i], Some(&s[i + 1..])),
        None if s.contains(':') => ("", Some(s)),
        _ => (s, None),
    };
    let mut total = 0u64;
    let mut any = false;
    if let Some(clock) = clock {
        let parts: Vec<u64> = clock.split(':').filter_map(|p| p.parse().ok()).collect();
        if parts.len() != 3 {
            return None;
        }
        total += parts[0] * 3_600 + parts[1] * 60 + parts[2];
        any = true;
    }
    let mut n = String::new();
    for c in units.chars() {
        if c.is_ascii_digit() {
            n.push(c);
            continue;
        }
        let v: u64 = n.parse().ok()?;
        n.clear();
        total += v * match c {
            'w' => 604_800,
            'd' => 86_400,
            'h' => 3_600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        any = true;
    }
    (any && n.is_empty()).then_some(total)
}

/// "2026-10-23 08:41:12" (RouterOS 7.10+) or "oct/23/2026 08:41:12" → Unix ms, read as UTC.
pub fn timestamp_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    let (date, time) = s.split_once(' ')?;
    let (y, m, d) = if date.contains('-') {
        let mut p = date.split('-');
        (p.next()?.parse::<i64>().ok()?, p.next()?.parse::<u32>().ok()?, p.next()?.parse::<u32>().ok()?)
    } else {
        let mut p = date.split('/');
        let mon = p.next()?.to_ascii_lowercase();
        let m = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|x| *x == mon)? as u32 + 1;
        let d = p.next()?.parse().ok()?;
        (p.next()?.parse().ok()?, m, d)
    };
    let t: Vec<u64> = time.split(':').filter_map(|x| x.parse().ok()).collect();
    if t.len() != 3 {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400) as u64 + t[0] * 3_600 + t[1] * 60 + t[2]) * 1000)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resource {
    pub board: Option<String>,
    pub version: Option<String>,
    pub cpu_pct: Option<f32>,
    pub uptime_s: Option<u64>,
}

pub fn parse_resource(v: &Value) -> Resource {
    Resource {
        board: str_of(v, "board-name"),
        version: str_of(v, "version"),
        cpu_pct: num(v, "cpu-load").map(|n| n as f32),
        uptime_s: str_of(v, "uptime").and_then(|u| duration_s(&u)),
    }
}

/// `/system/health` lists {name, value, type}; older versions return one object of name: value.
pub fn parse_health(v: &Value) -> Vec<crate::model::Sensor> {
    let pairs: Vec<(String, Value, String)> = match v {
        Value::Array(a) => a.iter().filter_map(|x| Some((str_of(x, "name")?, x.get("value")?.clone(), str_of(x, "type").unwrap_or_default()))).collect(),
        Value::Object(o) => o.iter().map(|(k, v)| (k.clone(), v.clone(), String::new())).collect(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for (name, value, kind) in pairs {
        let label = |n: &str| {
            let digits: String = n.chars().filter(|c| c.is_ascii_digit()).collect();
            if n.contains("cpu") {
                "CPU temperature".to_string()
            } else if n.contains("switch") {
                "Switch chip temperature".to_string()
            } else if n.starts_with("sfp") {
                "Optics temperature".to_string()
            } else {
                format!("Board temperature{}", if digits.is_empty() || digits == "1" { String::new() } else { format!(" {digits}") })
            }
        };
        if name.contains("temperature") {
            if let Some(t) = number(&value) {
                out.push(crate::model::Sensor { name: label(&name), value: t, unit: "°C".into(), ok: t < 85.0 });
            }
        } else if name.starts_with("fan") && name.contains("speed") {
            let n: String = name.chars().filter(|c| c.is_ascii_digit()).collect();
            if let Some(rpm) = number(&value) {
                out.push(crate::model::Sensor { name: format!("Fan {n}"), value: rpm, unit: "RPM".into(), ok: rpm > 500.0 });
            }
        } else if name.starts_with("psu") && name.ends_with("state") {
            let n: String = name.chars().filter(|c| c.is_ascii_digit()).collect();
            let ok = value.as_str().is_some_and(|s| s == "ok");
            out.push(crate::model::Sensor { name: format!("PSU{n}"), value: if ok { 1.0 } else { 0.0 }, unit: String::new(), ok });
        } else if (name.contains("voltage") || kind == "V") && number(&value).is_some() {
            out.push(crate::model::Sensor { name: "Voltage".into(), value: number(&value).unwrap_or(0.0), unit: "V".into(), ok: true });
        } else if (name.contains("power") || kind == "W") && number(&value).is_some() {
            out.push(crate::model::Sensor { name: "Power".into(), value: number(&value).unwrap_or(0.0), unit: "W".into(), ok: true });
        }
    }
    out
}

/// One row of `/interface`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Iface {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub running: bool,
    pub disabled: bool,
    pub comment: String,
    pub mtu: Option<u32>,
    pub mac: Option<String>,
    pub rx_byte: u64,
    pub tx_byte: u64,
    pub rx_error: u64,
    pub tx_error: u64,
    pub rx_drop: u64,
    pub tx_drop: u64,
    pub link_downs: u64,
    pub last_link_up_ms: Option<u64>,
}

pub fn parse_interfaces(v: &Value) -> Vec<Iface> {
    let Some(rows) = v.as_array() else { return Vec::new() };
    rows.iter()
        .filter_map(|x| {
            Some(Iface {
                id: str_of(x, ".id")?,
                name: str_of(x, "name")?,
                kind: str_of(x, "type").unwrap_or_default(),
                running: boolean(x, "running").unwrap_or(false),
                disabled: boolean(x, "disabled").unwrap_or(false),
                comment: str_of(x, "comment").unwrap_or_default(),
                mtu: num(x, "l2mtu").or_else(|| num(x, "actual-mtu")).or_else(|| num(x, "mtu")).map(|n| n as u32),
                mac: str_of(x, "mac-address"),
                rx_byte: uint(x, "rx-byte").unwrap_or(0),
                tx_byte: uint(x, "tx-byte").unwrap_or(0),
                rx_error: uint(x, "rx-error").unwrap_or(0),
                tx_error: uint(x, "tx-error").unwrap_or(0),
                rx_drop: uint(x, "rx-drop").unwrap_or(0),
                tx_drop: uint(x, "tx-drop").unwrap_or(0),
                link_downs: uint(x, "link-downs").unwrap_or(0),
                last_link_up_ms: str_of(x, "last-link-up-time").and_then(|t| timestamp_ms(&t)),
            })
        })
        .collect()
}

/// One port from `/interface ethernet monitor`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub link_ok: bool,
    /// Mb/s, from "rate": "200Gbps".
    pub rate_mbps: Option<u32>,
    pub fec: Option<String>,
    pub vendor: Option<String>,
    pub part: Option<String>,
    pub temp_c: Option<f32>,
    pub tx_power_dbm: Option<f32>,
    pub lane_rx_dbm: Vec<f32>,
}

fn dbm_list(v: &Value) -> Vec<f32> {
    match v {
        Value::String(s) => s.split(',').filter_map(|p| number(&Value::String(p.trim().to_string())).map(|n| n as f32)).collect(),
        Value::Number(_) => number(v).map(|n| vec![n as f32]).unwrap_or_default(),
        Value::Array(a) => a.iter().filter_map(|x| number(x).map(|n| n as f32)).collect(),
        _ => Vec::new(),
    }
}

pub fn parse_monitor(v: &Value) -> Monitor {
    // Per-lane values come as "sfp-rx-power": "-1.9dBm,-2.0dBm,…" or as sfp-rx-power1, -2…
    let mut lanes = v.get("sfp-rx-power").map(dbm_list).unwrap_or_default();
    if lanes.len() <= 1 {
        let mut numbered: Vec<(u32, f32)> = v
            .as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(k, val)| Some((k.strip_prefix("sfp-rx-power")?.trim_start_matches('-').parse::<u32>().ok()?, number(val)? as f32)))
                    .collect()
            })
            .unwrap_or_default();
        numbered.sort_by_key(|(i, _)| *i);
        if !numbered.is_empty() {
            lanes = numbered.into_iter().map(|(_, d)| d).collect();
        }
    }
    let rate = str_of(v, "rate").and_then(|r| {
        let n = number(&Value::String(r.clone()))?;
        Some(if r.contains('G') { n * 1_000.0 } else if r.contains('M') { n } else { n / 1_000.0 } as u32)
    });
    Monitor {
        name: str_of(v, "name").unwrap_or_default(),
        link_ok: str_of(v, "status").is_some_and(|s| s == "link-ok"),
        rate_mbps: rate,
        fec: str_of(v, "fec").or_else(|| str_of(v, "fec-mode")),
        vendor: str_of(v, "sfp-vendor-name"),
        part: str_of(v, "sfp-vendor-part-number"),
        temp_c: num(v, "sfp-temperature").map(|n| n as f32),
        tx_power_dbm: v.get("sfp-tx-power").map(dbm_list).and_then(|l| l.into_iter().reduce(f32::max)),
        lane_rx_dbm: lanes,
    }
}

/// Bits per second from two byte counters `dt_s` apart. A counter that went backwards
/// was reset (or wrapped): what it counted since then is the best estimate.
pub fn rate_bps(prev: u64, now: u64, dt_s: f64) -> f64 {
    if dt_s <= 0.0 {
        return 0.0;
    }
    let delta = if now >= prev { now - prev } else { now };
    delta as f64 * 8.0 / dt_s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routeros_strings_parse_defensively() {
        assert_eq!(number(&json!("45C")), Some(45.0));
        assert_eq!(number(&json!("-2.13dBm")), Some(-2.13));
        assert_eq!(number(&json!(7)), Some(7.0));
        assert_eq!(number(&json!("n/a")), None);
        assert_eq!(duration_s("6w1d02:55:14"), Some(6 * 604_800 + 86_400 + 2 * 3_600 + 55 * 60 + 14));
        assert_eq!(duration_s("1d23h12m3s"), Some(86_400 + 23 * 3_600 + 12 * 60 + 3));
        assert_eq!(duration_s("02:00:01"), Some(7_201));
        assert_eq!(duration_s("soon"), None);
        assert_eq!(timestamp_ms("1970-01-02 00:00:01"), Some(86_401_000));
        assert_eq!(timestamp_ms("oct/23/2026 08:41:12"), timestamp_ms("2026-10-23 08:41:12"));
        let r = parse_resource(&json!({ "board-name": "CRS804-4DDQ", "version": "7.20.2 (stable)", "cpu-load": "4", "uptime": "6w1d02:55:14" }));
        assert_eq!((r.board.as_deref(), r.cpu_pct), (Some("CRS804-4DDQ"), Some(4.0)));
    }

    #[test]
    fn health_lists_and_old_objects_both_become_sensors() {
        let s = parse_health(&json!([
            { ".id": "*1", "name": "cpu-temperature", "value": "47", "type": "C" },
            { ".id": "*2", "name": "switch-temperature", "value": "59", "type": "C" },
            { ".id": "*3", "name": "fan1-speed", "value": "5820", "type": "RPM" },
            { ".id": "*4", "name": "fan2-speed", "value": "0", "type": "RPM" },
            { ".id": "*5", "name": "psu1-state", "value": "ok" },
            { ".id": "*6", "name": "psu2-state", "value": "fail" },
        ]));
        let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["CPU temperature", "Switch chip temperature", "Fan 1", "Fan 2", "PSU1", "PSU2"]);
        assert!(!s[3].ok && !s[5].ok && s[4].ok, "a stopped fan and a failed PSU are flagged");
        assert_eq!(parse_health(&json!({ "temperature": "41", "fan1-speed": "4000" })).len(), 2);
    }

    #[test]
    fn monitor_reads_rates_and_every_lane() {
        let m = parse_monitor(&json!({ "name": "qsfp56-dd-1-1", "status": "link-ok", "rate": "200Gbps", "sfp-vendor-name": "MikroTik",
            "sfp-temperature": "45C", "sfp-tx-power": "0.7dBm,0.8dBm", "sfp-rx-power": "-1.9dBm,-2.0dBm,-9.5dBm,-2.1dBm" }));
        assert_eq!((m.link_ok, m.rate_mbps, m.temp_c), (true, Some(200_000), Some(45.0)));
        assert_eq!(m.lane_rx_dbm, vec![-1.9, -2.0, -9.5, -2.1]);
        assert_eq!(m.tx_power_dbm, Some(0.8));
        let numbered = parse_monitor(&json!({ "name": "x", "sfp-rx-power1": "-2", "sfp-rx-power2": "-3.5dBm" }));
        assert_eq!(numbered.lane_rx_dbm, vec![-2.0, -3.5]);
    }

    #[test]
    fn rates_survive_a_counter_reset() {
        assert_eq!(rate_bps(1_000, 3_000, 2.0), 8_000.0);
        assert_eq!(rate_bps(9_000_000, 500, 1.0), 4_000.0, "reset: what it counted since");
        assert_eq!(rate_bps(5, 5, 0.0), 0.0);
    }

    #[test]
    fn pinning_trusts_one_certificate_and_nothing_by_default() {
        let cert = b"a certificate";
        let fp = fingerprint(cert);
        assert_eq!(fp.len(), 64);
        assert_eq!(check_pin(Some(&fp), cert), Ok(()));
        assert_eq!(check_pin(Some(&fp.to_uppercase()), cert), Ok(()));
        assert_eq!(check_pin(None, cert), Err(PinProblem::Untrusted { seen: fp.clone() }));
        assert!(matches!(check_pin(Some(&fingerprint(b"other")), cert), Err(PinProblem::Changed { .. })));
        assert_eq!(pretty_fingerprint("0a1b"), "0A:1B");
    }
}
