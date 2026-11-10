//! Whether this Mac can reach the cluster at all (P1.10). On Oct 29 at 17:32 the path between
//! the Mac and the nodes went away: all four nodes read "SSH timeout", five criticals went out
//! and a page per node said "cell1 error", while cell1 was up the whole time. A node that
//! doesn't answer this Mac isn't confirmed down.
//!
//! Every 5 s with Live data the monitor reads the Mac's default route (`/sbin/route -n get
//! default`), pings its gateway once (`/sbin/ping -c 1 -t 1 <gw>`) and reads the VPN's state. A
//! router that drops pings (the owner's Firewalla Gold gave this Mac no ping reply in the round-1
//! look-only run) gets a TCP connect on 443, then 80, as probe.rs probes the Firewallas: a
//! refusal still proves it's there.
//! A wall clock that ran ahead of the monotonic one is a sleep: for 60 s after it the nodes get
//! time to reconnect (`waking`). When 3 or more nodes stop answering within 30 s it also
//! probes, at once, the Firewalla Switch X (TCP 443, then 80, as probe.rs does) and any second
//! LAN address a node had in its last frame (cell3's Wi-Fi), on :22, each within 1.5 s.
//!
//! `verdict` turns those probes and the snapshot's nodes into one state that health::on_publish
//! stamps into the snapshot (`mac_reach`). While it's a problem, rules.rs raises one mac-reach
//! warning (not paged by default) and holds the node rows under it as "Offline · not confirmed
//! down". A problem ends only when a node answers again: a sleep in the middle of it, or every
//! node between a dropped session and its next timeout, doesn't end it. Nothing here runs on a
//! node.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::timeout;

use super::Health;
use crate::inventory::SwitchKind;
use crate::model::{ClusterSnapshot, LinkStatus, MacReach, MacReachState, NetRole, NodeSnapshot};

/// Failure kinds that mean this Mac couldn't get to a node, not that the node turned it away.
pub const REACH_KINDS: [&str; 4] = ["timeout", "unreachable", "dns", "refused"];
pub const EVERY: Duration = Duration::from_secs(5);
/// The wall clock running this far past the monotonic one between two rounds is a sleep.
pub const SLEEP_JUMP_MS: u64 = 5_000;
/// After a sleep, the nodes get this long to reconnect before anything is raised about them.
pub const WAKE_GRACE_MS: u64 = 60_000;
/// This many nodes leaving Online within the window probe the other paths at once.
pub const BURST_NODES: usize = 3;
pub const BURST_WINDOW_MS: u64 = 30_000;
/// While no node answers, the other paths are probed again this often.
pub const REPROBE_MS: u64 = 30_000;
/// Once a node answers again, a problem holds this long while the others reconnect (their retry
/// backoff reaches 30 s), so the end of an outage isn't paged node by node.
pub const RECOVERY_GRACE_MS: u64 = 60_000;
/// An Online node whose numbers are older than this isn't answering either (its session
/// hasn't noticed yet).
pub const STALE_FAILING_MS: u64 = 5_000;
const PROBE_LIMIT: Duration = Duration::from_millis(1500);
/// Added to a problem's sentence while it holds for the nodes still reconnecting.
const HOLD_NOTE: &str = " Nodes are answering again; waiting for the rest.";

/// The Mac's default route.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub gateway: Option<String>,
    pub interface: Option<String>,
}

/// `route -n get default`'s output; None when there's no default route ("not in table").
pub fn parse_route(text: &str) -> Option<Route> {
    let field = |name: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == name).then(|| v.trim().to_string()).filter(|v| !v.is_empty())
        })
    };
    let route = Route { gateway: field("gateway"), interface: field("interface") };
    (route.gateway.is_some() || route.interface.is_some()).then_some(route)
}

/// One probe of another path to the cluster.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathProbe {
    /// "Firewalla Switch X", "cell3 Wi-Fi (wlP9s9)".
    pub label: String,
    pub node: Option<String>,
    /// "192.168.50.229:22", or the Switch X's address.
    pub addr: String,
    pub ok: bool,
    pub error: Option<String>,
    pub at_ms: u64,
}

/// What the Mac-side probes found last.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probes {
    /// The route has been read since Live data started.
    pub read: bool,
    /// None: no default route.
    pub route: Option<Route>,
    /// The gateway answered the last probe; None when there was none to probe.
    pub gateway_up: Option<bool>,
    pub gateway_ms: Option<f32>,
    pub gateway_error: Option<String>,
    /// What it answered: "ping", or "tcp" when it drops pings.
    pub gateway_by: Option<String>,
    /// It has answered a ping since SparkBox started: a router that never answers ping says nothing.
    pub gateway_answers: bool,
    /// The VPN's state (off, direct, connecting, connected, error) and its message.
    pub vpn: Option<String>,
    pub vpn_message: Option<String>,
    /// Nodes get time to reconnect until then (a sleep ended).
    pub waking_until_ms: Option<u64>,
    /// Whether a Switch X with an address is in the inventory.
    pub switch_x_set_up: bool,
    /// The last probe of the Switch X and of the nodes' second addresses (bursts only).
    pub switch_x: Option<PathProbe>,
    pub second: Vec<PathProbe>,
    pub probed_ms: u64,
}

/// How a node stands for the verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    /// Online with fresh numbers, or it answered and turned SparkBox away (a key or a login):
    /// either way the path to it works.
    Answers,
    /// Doesn't answer this Mac (a timeout, no route, refused, a name that doesn't resolve, or
    /// numbers that stopped).
    Silent,
    /// Anything else (still connecting, an error without a kind).
    Unknown,
}

/// The failure kind, from the backend or else from the error text (an older backend).
pub fn kind_of(n: &NodeSnapshot) -> Option<String> {
    n.error_kind.clone().or_else(|| n.error.as_deref().and_then(crate::live::error_kind_of).map(String::from))
}

/// A node that failed in a way that says nothing about the node itself.
pub fn reach_kind(n: &NodeSnapshot) -> bool {
    kind_of(n).is_some_and(|k| REACH_KINDS.contains(&k.as_str()))
}

fn standing(n: &NodeSnapshot) -> Option<Standing> {
    if n.maintenance.is_some() || n.power_state.is_some() {
        return None; // planned work: no evidence either way
    }
    Some(match n.status {
        LinkStatus::Online if n.stale && n.frame_age_ms.unwrap_or(0) > STALE_FAILING_MS => Standing::Silent,
        LinkStatus::Online => Standing::Answers,
        _ if reach_kind(n) => Standing::Silent,
        _ if kind_of(n).is_some() => Standing::Answers,
        _ => Standing::Unknown,
    })
}

/// A state that is this Mac's problem (and holds the node rows as not confirmed down).
pub fn problem(state: MacReachState) -> bool {
    !matches!(state, MacReachState::Ok | MacReachState::Waking)
}

fn state_of(snap: &ClusterSnapshot) -> MacReachState {
    snap.mac_reach.as_ref().map(|r| r.state).unwrap_or_default()
}

/// The snapshot says this Mac can't reach the cluster.
pub fn mac_problem(snap: &ClusterSnapshot) -> bool {
    problem(state_of(snap))
}

/// The Mac woke up less than a minute ago.
pub fn waking(snap: &ClusterSnapshot) -> bool {
    state_of(snap) == MacReachState::Waking
}

/// A node whose silence is this Mac's problem: what it shows isn't evidence it's down. (One that
/// answered and turned SparkBox away, a key or a login, isn't silent.)
pub fn unconfirmed(snap: &ClusterSnapshot, n: &NodeSnapshot) -> bool {
    mac_problem(snap)
        && match n.status {
            LinkStatus::Online => n.stale,
            _ => reach_kind(n) || kind_of(n).is_none(),
        }
}

/// A node that may still be catching up after a sleep: nothing new is raised about it.
pub fn settling(snap: &ClusterSnapshot, n: &NodeSnapshot) -> bool {
    waking(snap) && (n.status != LinkStatus::Online || n.stale) && (reach_kind(n) || kind_of(n).is_none())
}

/// "cell3", "cell1 and cell3", "cell1, cell2 and cell3".
fn names(list: &[String]) -> String {
    match list {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The state, its title and one sentence, from the probes and the nodes. Pure.
pub fn verdict(p: &Probes, nodes: &[NodeSnapshot], now: u64) -> (MacReachState, String, String) {
    if let Some(until) = p.waking_until_ms.filter(|t| now < *t) {
        let ago = WAKE_GRACE_MS.saturating_sub(until - now) / 1000;
        return (MacReachState::Waking, "This Mac just woke up".into(), format!("It woke up {ago} s ago; the nodes get a minute to reconnect before anything is raised about them."));
    }
    let ok = (MacReachState::Ok, String::new(), String::new());
    if !p.read {
        return ok;
    }
    let stood: Vec<(&NodeSnapshot, Standing)> = nodes.iter().filter_map(|n| standing(n).map(|s| (n, s))).collect();
    let silent: Vec<String> = stood.iter().filter(|(_, s)| *s == Standing::Silent).map(|(n, _)| n.name.clone()).collect();
    if silent.is_empty() || stood.iter().any(|(_, s)| *s == Standing::Answers) {
        return ok;
    }
    let away = matches!(p.vpn.as_deref(), Some("connected" | "connecting" | "error"));
    let lost = if away { "This Mac lost its network" } else { "This Mac lost the home network" };
    let Some(route) = &p.route else {
        return (MacReachState::NoRoute, lost.into(), "It has no network route, so no node can be reached. Check its Wi-Fi or cable.".into());
    };
    let gw = route.gateway.clone().unwrap_or_default();
    if p.gateway_up == Some(false) && p.gateway_answers {
        return (MacReachState::LostHomeNetwork, lost.into(), format!("Its router ({gw}) doesn't answer, so no node can be reached."));
    }
    if p.vpn.as_deref() == Some("error") {
        let why = p.vpn_message.as_deref().map(|m| format!(" ({m})")).unwrap_or_default();
        return (MacReachState::VpnProblem, "This Mac can't reach the cluster: VPN problem".into(), format!("The VPN is in error{why} and no node answers through it."));
    }
    if silent.len() < 2 {
        return ok; // one node alone isn't a path problem
    }
    let wifi: Vec<String> = p.second.iter().filter(|x| x.ok).map(|x| x.label.clone()).collect();
    let router = if p.gateway_up == Some(true) { format!("The router ({gw}) answers, but no node does over the wired LAN.") } else { "No node answers.".to_string() };
    let switch = match (&p.switch_x, p.switch_x_set_up) {
        (_, false) => " Switch X not set up, so SparkBox can't probe it.".to_string(),
        (Some(x), true) if x.ok => " The Firewalla Switch X answers.".to_string(),
        (Some(_), true) => " The Firewalla Switch X doesn't answer either.".to_string(),
        (None, true) => String::new(),
    };
    if !wifi.is_empty() {
        let who: Vec<String> = p.second.iter().filter(|x| x.ok).filter_map(|x| x.node.clone()).collect::<HashSet<_>>().into_iter().collect::<Vec<_>>();
        let mut who = who;
        who.sort();
        let title = format!("Only {}'s Wi-Fi answers: wired LAN switch?", names(&who));
        return (MacReachState::WiredSwitch, title, format!("{router} {} answers on its Wi-Fi address, so the wired path is the likely fault.{switch}", names(&wifi)));
    }
    (MacReachState::LanBetween, "This Mac can't reach any node; the LAN between them?".into(), format!("{router}{switch} The nodes aren't confirmed down."))
}

/// Whether two rounds' clocks say the Mac slept between them: the wall clock ran ahead of the
/// monotonic one (which stops while the Mac sleeps).
pub fn slept(wall_ms: u64, mono_ms: u64) -> bool {
    wall_ms.saturating_sub(mono_ms) > SLEEP_JUMP_MS
}

#[derive(Default)]
struct Inner {
    probes: Probes,
    current: MacReach,
    /// When the first node answered again after a problem.
    recovering_since: Option<u64>,
    /// Nodes answering in the last snapshot, and when nodes stopped (the last 30 s).
    answering: HashSet<String>,
    left: VecDeque<u64>,
    /// A burst asked for the other paths and the probe loop hasn't taken it yet.
    burst: bool,
    /// Each node's second LAN addresses from its last frame while online: (name, iface, ip).
    seconds: HashMap<String, Vec<(String, String, String)>>,
    /// Gateways that have answered a ping.
    answered: HashSet<String>,
}

/// The monitor's state, kept in Health.
#[derive(Default)]
pub struct Monitor {
    inner: Mutex<Inner>,
    /// Wakes the probe loop at once (a burst).
    wake: Notify,
}

/// What `reach_state` returns: the verdict and the probes behind it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReachView {
    /// False with Demo data: the monitor runs with Live data only.
    pub live: bool,
    pub reach: MacReach,
    pub probes: Probes,
}

impl Monitor {
    /// Fills in `snap.mac_reach` (Live only) from the last probes and this snapshot's nodes,
    /// and notes nodes leaving Online for the burst probe.
    pub fn stamp(&self, snap: &mut ClusterSnapshot) {
        if snap.demo {
            snap.mac_reach = None;
            return;
        }
        let now = snap.ts_ms;
        let mut inner = self.inner.lock().unwrap();
        if inner.observe(snap, now) {
            self.wake.notify_one();
        }
        let (mut state, mut title, mut detail) = verdict(&inner.probes, &snap.nodes, now);
        let prev = inner.current.clone();
        let stood: Vec<Standing> = snap.nodes.iter().filter_map(standing).collect();
        if problem(state) {
            inner.recovering_since = None;
        } else if problem(prev.state) && !stood.is_empty() && !stood.contains(&Standing::Answers) {
            // No node answers yet: a problem ends only when one does. The wake grace says nothing
            // about the path, and neither does every node being between a dropped session and its
            // next timeout ("No reply from cell1 in 10 s; reconnecting…" has no kind).
            (state, title) = (prev.state, prev.title.clone());
            detail = prev.detail.split(HOLD_NOTE).next().unwrap_or("").to_string();
            inner.recovering_since = None;
        } else if problem(prev.state) {
            // Nodes come back one by one: hold the problem while the rest reconnect.
            let waiting = stood.iter().any(|s| *s != Standing::Answers);
            let since = *inner.recovering_since.get_or_insert(now);
            if waiting && now.saturating_sub(since) < RECOVERY_GRACE_MS {
                (state, title) = (prev.state, prev.title.clone());
                detail = format!("{}{HOLD_NOTE}", prev.detail.split(HOLD_NOTE).next().unwrap_or(""));
            } else {
                inner.recovering_since = None;
            }
        }
        let route = inner.probes.route.clone().unwrap_or_default();
        let since_ms = if state == prev.state && prev.since_ms > 0 { prev.since_ms } else { now };
        inner.current = MacReach { state, since_ms, title, detail, gateway: route.gateway, interface: route.interface };
        snap.mac_reach = Some(inner.current.clone());
    }

    pub fn view(&self, live: bool) -> ReachView {
        let inner = self.inner.lock().unwrap();
        if !live {
            return ReachView { live, reach: MacReach { detail: "Available with Live data".into(), ..Default::default() }, probes: Probes::default() };
        }
        ReachView { live, reach: inner.current.clone(), probes: inner.probes.clone() }
    }

    /// A sleep ended at `now`: the nodes get a minute to reconnect.
    pub fn woke(&self, now: u64) {
        self.inner.lock().unwrap().probes.waking_until_ms = Some(now + WAKE_GRACE_MS);
    }

    /// Demo data: nothing is probed, and the next Live start begins afresh.
    pub(super) fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        let answered = std::mem::take(&mut inner.answered);
        *inner = Inner { answered, ..Default::default() };
    }

    /// The probes found this round (`switch_x`/`second` only when they were run).
    #[cfg(test)]
    pub fn set_probes(&self, p: Probes) {
        self.inner.lock().unwrap().probes = p;
    }

    /// Whether this round should probe the other paths: a burst asked (taken here, so one that
    /// comes during the round asks again), or no node answers and they were last probed over 30 s ago.
    fn wants_paths(&self, now: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let due = problem(inner.current.state) && now.saturating_sub(inner.probes.probed_ms) >= REPROBE_MS;
        std::mem::take(&mut inner.burst) || due
    }

    /// The second addresses to probe: each node's Wi-Fi (or second LAN port) from its last frame.
    fn second_targets(&self) -> Vec<(String, String, String)> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<(String, String, String)> = inner.seconds.values().flat_map(|v| v.iter().cloned()).collect();
        out.sort();
        out
    }
}

impl Inner {
    /// Notes who answers; true when 3 or more nodes stopped within 30 s (a burst).
    fn observe(&mut self, snap: &ClusterSnapshot, now: u64) -> bool {
        for n in &snap.nodes {
            let answers = n.status == LinkStatus::Online && !n.stale;
            if answers {
                let seconds = n
                    .nets
                    .iter()
                    .filter(|x| matches!(x.role, NetRole::Lan | NetRole::Wifi) && x.up)
                    .filter_map(|x| x.ip.as_ref().filter(|ip| **ip != n.host && ip.parse::<std::net::Ipv4Addr>().is_ok()).map(|ip| (n.name.clone(), x.iface.clone(), ip.clone())))
                    .collect();
                self.seconds.insert(n.id.clone(), seconds);
                self.answering.insert(n.id.clone());
            } else if self.answering.remove(&n.id) {
                self.left.push_back(now);
            }
        }
        while self.left.front().is_some_and(|t| now.saturating_sub(*t) > BURST_WINDOW_MS) {
            self.left.pop_front();
        }
        if self.left.len() >= BURST_NODES && !self.burst {
            self.burst = true;
            self.left.clear();
            return true;
        }
        false
    }
}

/// `/sbin/route -n get default`.
async fn read_route() -> Option<Route> {
    let out = timeout(Duration::from_secs(3), tokio::process::Command::new("/sbin/route").args(["-n", "get", "default"]).kill_on_drop(true).output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None; // "not in table": no default route
    }
    parse_route(&String::from_utf8_lossy(&out.stdout))
}

/// One ping (`/sbin/ping -c 1 -t 1 <addr>`): the round trip, or why there was none.
async fn ping(addr: std::net::IpAddr) -> Result<f32, String> {
    let t = Instant::now();
    let run = tokio::process::Command::new("/sbin/ping").args(["-c", "1", "-t", "1", &addr.to_string()]).kill_on_drop(true).output();
    match timeout(Duration::from_secs(3), run).await {
        Ok(Ok(out)) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let ms = text.split("time=").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse::<f32>().ok());
            Ok(ms.unwrap_or_else(|| t.elapsed().as_secs_f32() * 1000.0))
        }
        Ok(Ok(_)) => Err("no reply in 1 s".into()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("no reply in 3 s".into()),
    }
}

/// A TCP connect to `host:port` within 1.5 s (through the VPN's proxy when it's up). A refusal
/// still proves the address answers.
async fn tcp(host: &str, port: u16, socks: Option<(String, String, String)>) -> Result<f32, String> {
    let t = Instant::now();
    let result = match &socks {
        Some((proxy, user, pass)) => timeout(PROBE_LIMIT, tokio_socks::tcp::Socks5Stream::connect_with_password(proxy.as_str(), (host, port), user, pass)).await.map(|r| r.map(drop).map_err(|e| e.to_string())),
        None => timeout(PROBE_LIMIT, TcpStream::connect((host, port))).await.map(|r| r.map(drop).map_err(|e| e.to_string())),
    };
    match result {
        Ok(Ok(())) => Ok(t.elapsed().as_secs_f32() * 1000.0),
        Ok(Err(e)) if e.to_lowercase().contains("refused") => Ok(t.elapsed().as_secs_f32() * 1000.0),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(format!("no answer within {} ms", PROBE_LIMIT.as_millis())),
    }
}

fn now_ms() -> u64 {
    crate::history::now_ms()
}

/// Runs the monitor: every 5 s with Live data (and at once on a burst). Started once.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(health) = app.try_state::<Arc<Health>>().map(|h| h.inner().clone()) else { return };
        let (mut wall, mut mono) = (now_ms(), Instant::now());
        loop {
            tokio::select! {
                _ = tokio::time::sleep(EVERY) => {}
                _ = health.reach.wake.notified() => {}
            }
            let (w, m) = (now_ms(), Instant::now());
            if slept(w.saturating_sub(wall), m.duration_since(mono).as_millis() as u64) {
                health.reach.woke(w);
            }
            (wall, mono) = (w, m);
            let Some(state) = app.try_state::<crate::AppState>() else { continue };
            if !crate::is_live(&state) {
                health.reach.clear();
                continue;
            }
            let vpn = state.vpn.clone();
            let switch_x: Option<(String, String)> = state.inventory.lock().unwrap().switches.iter().find(|s| s.kind == SwitchKind::Firewalla && !s.host.trim().is_empty()).map(|s| (s.name.clone(), s.host.trim().to_string()));
            let route = read_route().await;
            let gateway = route.as_ref().and_then(|r| r.gateway.as_deref()).and_then(|g| g.parse::<std::net::IpAddr>().ok());
            let pinged = match gateway {
                Some(g) => Some(match ping(g).await {
                    Ok(ms) => Ok((ms, "ping")),
                    // It may drop pings: a TCP answer (or a refusal) on 443 or 80 still proves it's there.
                    Err(e) => crate::probe::probe_host(&g.to_string(), None, PROBE_LIMIT).await.map(|ms| (ms, "tcp")).map_err(|t| format!("{e} (ping); {t} (TCP 443, 80)")),
                }),
                None => None,
            };
            let status = vpn.view().await.status;
            let vpn_word = serde_json::to_value(status.state).ok().and_then(|v| v.as_str().map(String::from));
            let paths = if health.reach.wants_paths(w) {
                let socks = vpn.socks().await;
                let sx = match &switch_x {
                    Some((name, host)) => {
                        let r = crate::probe::probe_host(host, socks.clone(), PROBE_LIMIT).await;
                        Some(PathProbe { label: name.clone(), node: None, addr: host.clone(), ok: r.is_ok(), error: r.err(), at_ms: now_ms() })
                    }
                    None => None,
                };
                let targets = health.reach.second_targets();
                let found = futures_util::future::join_all(targets.iter().map(|(node, iface, ip)| {
                    let socks = socks.clone();
                    async move {
                        let r = tcp(ip, 22, socks).await;
                        PathProbe { label: format!("{node}'s Wi-Fi ({iface}, {ip})"), node: Some(node.clone()), addr: format!("{ip}:22"), ok: r.is_ok(), error: r.err(), at_ms: now_ms() }
                    }
                }))
                .await;
                Some((sx, found))
            } else {
                None
            };
            let mut inner = health.reach.inner.lock().unwrap();
            let p = &mut inner.probes;
            p.read = true;
            p.route = route;
            p.gateway_up = pinged.as_ref().map(|r| r.is_ok());
            p.gateway_ms = pinged.as_ref().and_then(|r| r.as_ref().ok().map(|x| x.0));
            p.gateway_by = pinged.as_ref().and_then(|r| r.as_ref().ok().map(|x| x.1.to_string()));
            p.gateway_error = pinged.as_ref().and_then(|r| r.as_ref().err().cloned());
            p.vpn = vpn_word;
            p.vpn_message = status.message.clone();
            p.switch_x_set_up = switch_x.is_some();
            if let Some((sx, second)) = paths {
                p.switch_x = sx;
                p.second = second;
                p.probed_ms = w;
            }
            if let (Some(g), Some(Ok(_))) = (gateway, &pinged) {
                inner.answered.insert(g.to_string());
            }
            let gw = inner.probes.route.as_ref().and_then(|r| r.gateway.clone()).unwrap_or_default();
            inner.probes.gateway_answers = inner.answered.contains(&gw);
        }
    });
}

/// The cluster-path monitor's verdict and the probes behind it (Alerts, the harness).
#[tauri::command]
pub fn reach_state(app: AppHandle, health: State<'_, Arc<Health>>) -> ReachView {
    let live = app.try_state::<crate::AppState>().is_some_and(|s| crate::is_live(&s));
    health.reach.view(live)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_route_reads_as_macos_prints_it() {
        let text = "   route to: default\ndestination: default\n       mask: default\n    gateway: 192.168.50.1\n  interface: en0\n      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>\n";
        assert_eq!(parse_route(text), Some(Route { gateway: Some("192.168.50.1".into()), interface: Some("en0".into()) }));
        // A VPN's point-to-point route names only its interface.
        assert_eq!(parse_route("destination: default\n  interface: utun4\n"), Some(Route { gateway: None, interface: Some("utun4".into()) }));
        assert_eq!(parse_route("route: writing to routing socket: not in table\n"), None);
    }

    fn node(id: &str, host: &str, wifi: Option<&str>) -> NodeSnapshot {
        let net = |iface: &str, role: NetRole, ip: &str| crate::model::NetStats { iface: iface.into(), role, up: true, ip: Some(ip.into()), ..Default::default() };
        let mut nets = vec![net("enP7s7", NetRole::Lan, host), net("enp1s0f1np1", NetRole::Fabric, "192.168.100.3")];
        nets.extend(wifi.map(|ip| net("wlP9s9", NetRole::Wifi, ip)));
        NodeSnapshot { id: id.into(), name: id.into(), host: host.into(), status: LinkStatus::Online, nets, ..Default::default() }
    }

    #[test]
    fn three_nodes_stopping_within_30_s_probe_the_other_paths_at_once() {
        let m = Monitor::default();
        m.set_probes(Probes { read: true, route: Some(Route { gateway: Some("192.168.50.1".into()), interface: Some("en0".into()) }), gateway_up: Some(true), gateway_answers: true, ..Default::default() });
        let mut s = ClusterSnapshot {
            ts_ms: 1_000_000,
            nodes: vec![node("cell1", "192.168.50.51", None), node("cell2", "192.168.50.154", None), node("cell3", "192.168.50.98", Some("192.168.50.229")), node("gx10-0000", "192.168.50.181", None)],
            ..Default::default()
        };
        m.stamp(&mut s);
        // cell3's Wi-Fi is the second path; its own LAN address and the fabric aren't.
        assert_eq!(m.second_targets(), [("cell3".to_string(), "wlP9s9".to_string(), "192.168.50.229".to_string())]);
        assert!(!m.wants_paths(s.ts_ms), "at rest nothing else is probed");
        let stop = |s: &mut ClusterSnapshot, i: usize, after_ms: u64| {
            s.ts_ms += after_ms;
            (s.nodes[i].status, s.nodes[i].error_kind) = (LinkStatus::Error, Some("timeout".into()));
            s.nodes[i].nets.clear(); // an offline node's frame is gone; its last one is kept
            m.stamp(s);
        };
        stop(&mut s, 0, 1000);
        stop(&mut s, 1, 9000);
        assert!(!m.wants_paths(s.ts_ms), "two isn't a burst");
        stop(&mut s, 2, 15_000);
        assert!(m.wants_paths(s.ts_ms), "the third within 30 s is");
        assert_eq!(m.second_targets().len(), 1, "cell3's Wi-Fi from its last frame");
        // Spread out over minutes, nodes stopping one by one aren't a burst.
        let m = Monitor::default();
        let mut s = ClusterSnapshot { ts_ms: 1_000_000, nodes: vec![node("a", "10.0.0.1", None), node("b", "10.0.0.2", None), node("c", "10.0.0.3", None)], ..Default::default() };
        m.stamp(&mut s);
        for i in 0..3 {
            s.ts_ms += 40_000;
            s.nodes[i].status = LinkStatus::Offline;
            m.stamp(&mut s);
        }
        assert!(!m.wants_paths(s.ts_ms));
    }

    #[test]
    fn a_sleep_is_the_wall_clock_running_ahead() {
        assert!(!slept(5_000, 5_000));
        assert!(!slept(9_900, 5_000), "a slow round isn't a sleep");
        assert!(slept(600_000, 5_000));
    }
}
