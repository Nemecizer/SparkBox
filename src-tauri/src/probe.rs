//! Reachability of the devices SparkBox can only probe: the Firewalla Switch X and the
//! Firewalla Gold (neither has a documented local API). Every 5 s in Live mode each one
//! gets a TCP connection attempt on 443, then 80; a refusal still proves it's up. The
//! last 60 probes give latency and loss, 5-minute buckets give 24-hour uptime, and
//! runs of misses are remembered as outages. Through the VPN, probes go via its proxy.
//!
//! It's also where a node's TCP connect time is measured (`connect_time`, for the VPN
//! window), and how every latency is printed (`ms_text`). The status bar's latency is
//! something else, said so in its tooltip: the round trip of SparkBox's last command over SSH.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::inventory::SwitchKind;
use crate::model::{LinkStatus, SwitchSnapshot};
use crate::vpn::Vpn;

pub const PROBE_EVERY: Duration = Duration::from_secs(5);
const KEEP: usize = 60;
const BUCKET_MS: u64 = 5 * 60 * 1000;
const BUCKETS: usize = 24 * 12;
/// Misses in a row before it counts as an outage (and the device as Offline).
const OUTAGE_AFTER: usize = 3;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeSample {
    pub ts_ms: u64,
    /// Round trip in ms, or None for a lost probe.
    pub ms: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outage {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeHistory {
    pub switch_id: String,
    pub target: String,
    pub samples: Vec<ProbeSample>,
    pub loss_pct: Option<f32>,
    pub uptime_pct: Option<f32>,
    pub last_outage: Option<Outage>,
}

#[derive(Default)]
struct Track {
    target: String,
    samples: VecDeque<ProbeSample>,
    /// (bucket start, answered, probes) for the last 24 hours.
    buckets: VecDeque<(u64, u32, u32)>,
    misses: usize,
    outage: Option<Outage>,
    last_outage: Option<Outage>,
    last_error: Option<String>,
}

pub struct Prober {
    vpn: Arc<Vpn>,
    tracks: Mutex<HashMap<String, Track>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// How long a TCP connection to `target` ("ip:port") takes, in ms: the connect time the VPN
/// window's table and log quote, and the status bar's tooltip beside the SSH round trip.
pub async fn connect_time(target: &str, limit: Duration) -> Result<f32, String> {
    let start = Instant::now();
    match timeout(limit, TcpStream::connect(target)).await {
        Ok(Ok(_)) => Ok(start.elapsed().as_secs_f32() * 1000.0),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out".into()),
    }
}

/// "5.6 ms", "22 ms", "1.2 s": one way to print a latency everywhere.
pub fn ms_text(ms: f32) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// One TCP probe of `host`: 443, then 80. An answer of any kind (even a refusal) means it's up.
pub async fn probe_host(host: &str, socks: Option<(String, String, String)>, limit: Duration) -> Result<f32, String> {
    let mut last = String::new();
    for port in [443u16, 80] {
        let t = Instant::now();
        let result = match &socks {
            Some((proxy, user, pass)) => timeout(limit, tokio_socks::tcp::Socks5Stream::connect_with_password(proxy.as_str(), (host, port), user, pass))
                .await
                .map(|r| r.map(drop).map_err(|e| e.to_string())),
            None => timeout(limit, TcpStream::connect((host, port))).await.map(|r| r.map(drop).map_err(|e| e.to_string())),
        };
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        match result {
            Ok(Ok(())) => return Ok(ms),
            Ok(Err(e)) if e.contains("refused") || e.contains("Refused") => return Ok(ms),
            Ok(Err(e)) => last = e,
            Err(_) => last = format!("no answer within {} ms", limit.as_millis()),
        }
    }
    Err(last)
}

impl Track {
    fn record(&mut self, at: u64, result: Result<f32, String>) {
        let ok = result.is_ok();
        self.samples.push_back(ProbeSample { ts_ms: at, ms: result.as_ref().ok().copied() });
        while self.samples.len() > KEEP {
            self.samples.pop_front();
        }
        let start = at - at % BUCKET_MS;
        match self.buckets.back_mut() {
            Some((b, answered, total)) if *b == start => {
                *answered += ok as u32;
                *total += 1;
            }
            _ => self.buckets.push_back((start, ok as u32, 1)),
        }
        while self.buckets.len() > BUCKETS {
            self.buckets.pop_front();
        }
        match result {
            Ok(_) => {
                self.misses = 0;
                self.last_error = None;
                if let Some(mut o) = self.outage.take() {
                    o.end_ms = Some(at);
                    self.last_outage = Some(o);
                }
            }
            Err(e) => {
                self.misses += 1;
                if self.misses == OUTAGE_AFTER {
                    let first = self.samples.iter().rev().nth(OUTAGE_AFTER - 1).map(|s| s.ts_ms).unwrap_or(at);
                    self.outage = Some(Outage { start_ms: first, end_ms: None, reason: format!("No answer to {OUTAGE_AFTER} probes in a row ({e})") });
                }
                self.last_error = Some(e);
            }
        }
    }

    fn loss_pct(&self) -> Option<f32> {
        (!self.samples.is_empty()).then(|| self.samples.iter().filter(|s| s.ms.is_none()).count() as f32 * 100.0 / self.samples.len() as f32)
    }

    fn uptime_pct(&self) -> Option<f32> {
        let (ok, total) = self.buckets.iter().fold((0u32, 0u32), |(a, b), (_, ok, t)| (a + ok, b + t));
        (total > 0).then(|| ok as f32 * 100.0 / total as f32)
    }
}

impl Prober {
    pub fn new(vpn: Arc<Vpn>) -> Arc<Prober> {
        Arc::new(Prober { vpn, tracks: Mutex::new(HashMap::new()) })
    }

    /// Probes one device now and records the result.
    pub async fn probe(&self, id: &str, host: &str) {
        let result = probe_host(host, self.vpn.socks().await, PROBE_TIMEOUT).await;
        self.record(id, host, now_ms(), result);
    }

    fn record(&self, id: &str, host: &str, at: u64, result: Result<f32, String>) {
        let mut tracks = self.tracks.lock().unwrap();
        let t = tracks.entry(id.to_string()).or_default();
        if t.target != host {
            *t = Track { target: host.to_string(), ..Default::default() };
        }
        t.record(at, result);
    }

    /// The device as the probes see it: never Online without an address or an answer.
    pub fn snapshot(&self, base: &SwitchSnapshot) -> SwitchSnapshot {
        let tracks = self.tracks.lock().unwrap();
        let Some(t) = tracks.get(&base.id).filter(|t| t.target == base.host.trim()) else {
            return SwitchSnapshot { status: if base.configured { LinkStatus::Connecting } else { LinkStatus::Offline }, ..base.clone() };
        };
        let last = t.samples.back();
        let status = match (last.and_then(|s| s.ms), t.misses) {
            (Some(_), _) => LinkStatus::Online,
            (None, m) if m >= OUTAGE_AFTER => LinkStatus::Offline,
            _ => LinkStatus::Online,
        };
        SwitchSnapshot {
            status,
            latency_ms: last.and_then(|s| s.ms),
            loss_pct: t.loss_pct(),
            error: (t.misses >= OUTAGE_AFTER).then(|| format!("{} hasn't answered for {} s.", base.name, t.misses as u64 * PROBE_EVERY.as_secs())),
            board: Some(if base.kind == SwitchKind::Gateway { "Firewalla Gold" } else { "Firewalla Switch X" }.into()),
            ..base.clone()
        }
    }

    pub fn history(&self, id: &str) -> ProbeHistory {
        let tracks = self.tracks.lock().unwrap();
        let Some(t) = tracks.get(id) else {
            return ProbeHistory { switch_id: id.into(), ..Default::default() };
        };
        ProbeHistory {
            switch_id: id.into(),
            target: format!("{}:443", t.target),
            samples: t.samples.iter().cloned().collect(),
            loss_pct: t.loss_pct(),
            uptime_pct: t.uptime_pct(),
            last_outage: t.outage.clone().or_else(|| t.last_outage.clone()),
        }
    }
}

/// A plausible probe history for the simulator: steady sub-millisecond answers, one
/// short outage the day before, the latest samples matching the snapshot.
pub fn demo_history(sw: &SwitchSnapshot, now: u64) -> ProbeHistory {
    let gold = sw.kind == SwitchKind::Gateway;
    let mut rng = fastrand::Rng::with_seed(if gold { 7 } else { 11 } + now / 5_000);
    let base = if gold { 0.38 } else { 0.55 };
    let samples: Vec<ProbeSample> = (0..KEEP as u64)
        .map(|i| {
            let ts = now - (KEEP as u64 - 1 - i) * PROBE_EVERY.as_millis() as u64;
            let spike = if rng.f64() < 0.04 { 2.0 + rng.f64() * 3.0 } else { 0.0 };
            ProbeSample { ts_ms: ts, ms: Some((base + rng.f64() * 0.5 + spike) as f32) }
        })
        .chain(std::iter::once(ProbeSample { ts_ms: now, ms: sw.latency_ms }))
        .skip(1)
        .collect();
    let (ago_h, minutes, reason) = if gold { (19, 4, "No answer to 48 probes in a row (rebooted after a Firewalla update)") } else { (31, 2, "No answer to 23 probes in a row (connection refused, then timed out)") };
    let start = now - ago_h * 3_600_000;
    ProbeHistory {
        switch_id: sw.id.clone(),
        target: format!("{}:443", sw.host),
        samples,
        loss_pct: Some(0.0),
        uptime_pct: Some(100.0 - minutes as f32 * 60.0 * 100.0 / 86_400.0),
        last_outage: Some(Outage { start_ms: start, end_ms: Some(start + minutes * 60_000 + 23_000), reason: reason.into() }),
    }
}

/// Probes the Firewalla devices every 5 s while the data source is Live.
pub fn start(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(PROBE_EVERY);
        loop {
            every.tick().await;
            let state = handle.state::<crate::AppState>();
            if !crate::is_live(&state) {
                continue;
            }
            let targets: Vec<(String, String)> = state
                .inventory
                .lock()
                .unwrap()
                .switches
                .iter()
                .filter(|s| matches!(s.kind, SwitchKind::Firewalla | SwitchKind::Gateway) && !s.host.trim().is_empty())
                .map(|s| (s.id.clone(), s.host.trim().to_string()))
                .collect();
            let prober = handle.state::<Arc<Prober>>().inner().clone();
            futures_util::future::join_all(targets.iter().map(|(id, host)| prober.probe(id, host))).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_listener_or_a_refusal_counts_as_up_and_silence_doesnt() {
        // 127.0.0.1 refuses 443/80 (nothing listens there in tests): still an answer.
        assert!(probe_host("127.0.0.1", None, Duration::from_millis(500)).await.is_ok());
        // A blackhole address never answers.
        let e = probe_host("10.255.255.1", None, Duration::from_millis(150)).await.unwrap_err();
        assert!(e.contains("no answer") || e.contains("unreachable") || e.contains("route"), "{e}");
    }

    #[tokio::test]
    async fn connect_time_is_one_tcp_connect_and_prints_one_way() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ms = connect_time(&l.local_addr().unwrap().to_string(), Duration::from_secs(2)).await.unwrap();
        assert!(ms < 1000.0, "{ms}");
        assert_eq!(connect_time("10.255.255.1:22", Duration::from_millis(100)).await.unwrap_err(), "timed out");
        assert_eq!((ms_text(5.54), ms_text(22.4), ms_text(1234.0)), ("5.5 ms".to_string(), "22 ms".to_string(), "1.2 s".to_string()));
    }

    #[test]
    fn loss_uptime_and_outages_are_tracked() {
        let mut t = Track { target: "192.168.50.3".into(), ..Default::default() };
        let t0 = 1_000_000_000u64;
        for i in 0..10 {
            t.record(t0 + i * 5_000, Ok(0.6));
        }
        for i in 10..14 {
            t.record(t0 + i * 5_000, Err("no answer within 1500 ms".into()));
        }
        assert!(t.outage.is_some(), "3 misses in a row is an outage");
        assert_eq!(t.outage.as_ref().unwrap().start_ms, t0 + 10 * 5_000, "it began at the first miss");
        t.record(t0 + 14 * 5_000, Ok(0.7));
        let o = t.last_outage.clone().unwrap();
        assert_eq!((o.start_ms, o.end_ms), (t0 + 50_000, Some(t0 + 70_000)));
        assert!(t.outage.is_none());
        assert!((t.loss_pct().unwrap() - 4.0 * 100.0 / 15.0).abs() < 0.01);
        assert!((t.uptime_pct().unwrap() - 11.0 * 100.0 / 15.0).abs() < 0.01);
        for i in 0..100 {
            t.record(t0 + (15 + i) * 5_000, Ok(0.5));
        }
        assert_eq!(t.samples.len(), KEEP);
    }

    #[test]
    fn an_unconfigured_or_silent_device_is_never_online() {
        let prober = Prober { vpn: crate::vpn::Vpn::new(std::env::temp_dir().join("sparkbox-probe-vpn.json"), "nope".into(), |_| {}), tracks: Mutex::default() };
        let unset = SwitchSnapshot { id: "firewalla".into(), name: "Firewalla Switch X".into(), kind: SwitchKind::Firewalla, ..Default::default() };
        assert_eq!(prober.snapshot(&unset).status, LinkStatus::Offline);
        let set = SwitchSnapshot { host: "192.168.50.3".into(), configured: true, ..unset };
        assert_eq!(prober.snapshot(&set).status, LinkStatus::Connecting, "no probe yet");
        for i in 0..3 {
            prober.record("firewalla", "192.168.50.3", 1_000 + i * 5_000, Err("no answer".into()));
        }
        let s = prober.snapshot(&set);
        assert_eq!(s.status, LinkStatus::Offline);
        assert!(s.error.unwrap().contains("hasn't answered for 15 s"));
        prober.record("firewalla", "192.168.50.3", 20_000, Ok(0.8));
        let s = prober.snapshot(&set);
        assert_eq!((s.status, s.latency_ms), (LinkStatus::Online, Some(0.8)));
        assert_eq!(prober.history("firewalla").last_outage.unwrap().end_ms, Some(20_000));
    }

    #[test]
    fn the_demo_history_ends_where_the_snapshot_is() {
        let sw = SwitchSnapshot { id: "gold".into(), kind: SwitchKind::Gateway, host: "192.168.50.1".into(), latency_ms: Some(0.42), ..Default::default() };
        let h = demo_history(&sw, 2_000_000_000_000);
        assert_eq!(h.samples.len(), KEEP);
        assert_eq!(h.samples.last().unwrap().ms, Some(0.42));
        assert!(h.uptime_pct.unwrap() < 100.0 && h.last_outage.is_some());
    }
}
