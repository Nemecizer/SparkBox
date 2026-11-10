//! The CRS804 fabric switch over the RouterOS REST API, and the Firewalla devices
//! by probing (see probe.rs). In Live mode a background loop polls each configured
//! RouterOS switch every 2 s (resource, health, interfaces, ethernet monitor),
//! turns byte counters into rates, maps ports to nodes by their comment, and
//! `overlay` puts the result into every published snapshot. A switch without an
//! address is "not set up" and never shown as Online.

pub mod client;
pub mod commands;
pub mod mapping;
pub mod safety;
#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use tauri::{AppHandle, Manager};

use crate::inventory::{SwitchConfig, SwitchKind};
use crate::live::placeholder_switch;
use crate::model::{ClusterSnapshot, LinkStatus, NodeSnapshot, SwitchPort, SwitchSnapshot};
use crate::util;
use crate::vpn::Vpn;
use client::{ApiError, Client};

pub const POLL_EVERY: Duration = Duration::from_secs(2);

type PasswordFn = Box<dyn Fn(&str) -> Result<Option<String>, String> + Send + Sync>;

// Compared structurally; credentials and their tags are never logged.
#[derive(PartialEq)]
struct ClientKey {
    host: String,
    user: String,
    pin: Option<String>,
    proxy: Option<String>,
    password_hash: String,
}

#[derive(Default)]
struct SwitchState {
    snap: Option<SwitchSnapshot>,
    /// Byte counters at the last poll, per port: (when, rx, tx).
    counters: HashMap<String, (Instant, u64, u64)>,
    /// RouterOS ids (".id", e.g. "*3") by port name, for writes.
    ids: HashMap<String, String>,
    /// Every identity/transport input used to build the authenticated client.
    client: Option<(ClientKey, Arc<Client>)>,
    /// Bridge host, neighbor, address and ARP tables, and when they were read.
    tables: Option<(Instant, mapping::PortTables)>,
    /// Ports disabled with a revert scheduled on the switch: port → when it re-enables (Unix ms).
    reverts: HashMap<String, u64>,
}

/// The tables change only when something is recabled: read them every 15 s, not every poll.
const TABLES_EVERY: Duration = Duration::from_secs(15);

/// Every RouterOS switch SparkBox polls.
pub struct Switches {
    /// Pinned certificate fingerprints by switch address (switch_certs.json).
    pins_path: PathBuf,
    vpn: Arc<Vpn>,
    password: PasswordFn,
    /// "https" for real switches; "http" for the test fake.
    scheme: String,
    state: Mutex<HashMap<String, SwitchState>>,
}

/// The user SparkBox logs in as, unless the inventory names another.
pub fn user_of(cfg: &SwitchConfig) -> String {
    cfg.user.clone().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "sparkbox".into())
}

/// The node a port's comment names (by name, id or hostname), if any.
pub fn node_for_comment(comment: &str, nodes: &[NodeSnapshot]) -> Option<String> {
    let c = comment.trim().to_ascii_lowercase();
    if c.is_empty() {
        return None;
    }
    let words: Vec<&str> = c.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '.')).filter(|w| !w.is_empty()).collect();
    nodes
        .iter()
        .find(|n| {
            let names = [n.id.to_ascii_lowercase(), n.name.to_ascii_lowercase(), n.system.hostname.to_ascii_lowercase()];
            names.iter().any(|name| !name.is_empty() && words.iter().any(|w| w == name || w.trim_end_matches(".local") == name))
        })
        .map(|n| n.id.clone())
}

impl Switches {
    pub fn new(pins_path: PathBuf, vpn: Arc<Vpn>, password: impl Fn(&str) -> Result<Option<String>, String> + Send + Sync + 'static) -> Arc<Switches> {
        Arc::new(Switches { pins_path, vpn, password: Box::new(password), scheme: "https".into(), state: Mutex::new(HashMap::new()) })
    }

    #[cfg(test)]
    pub fn new_plain_http(pins_path: PathBuf, vpn: Arc<Vpn>, password: impl Fn(&str) -> Result<Option<String>, String> + Send + Sync + 'static) -> Arc<Switches> {
        Arc::new(Switches { pins_path, vpn, password: Box::new(password), scheme: "http".into(), state: Mutex::new(HashMap::new()) })
    }

    fn pins(&self) -> HashMap<String, String> {
        util::load_json_or_default(&self.pins_path)
    }

    pub fn pin_for(&self, host: &str) -> Option<String> {
        self.pins().get(host.trim()).cloned()
    }

    /// Trusts a switch's certificate (after the user compared the fingerprint).
    pub fn trust(&self, host: &str, fingerprint: &str) -> Result<(), String> {
        let fp: String = fingerprint.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_lowercase();
        if fp.len() != 64 {
            return Err("That isn't a SHA-256 certificate fingerprint (64 hex digits).".into());
        }
        let mut pins = self.pins();
        pins.insert(host.trim().to_string(), fp);
        util::save_json(&self.pins_path, &pins)?;
        for st in self.state.lock().unwrap().values_mut() {
            st.client = None;
        }
        Ok(())
    }

    /// A client for this switch, reused while nothing it depends on changed.
    pub async fn client(&self, cfg: &SwitchConfig) -> Result<Arc<Client>, String> {
        let host = cfg.host.trim();
        if host.is_empty() {
            return Err(format!("{} has no address yet. Add it in Settings › Switches & gateway.", cfg.name));
        }
        let password = (self.password)(&cfg.id)?.ok_or_else(|| format!("Save the REST password for {} in Settings › Switches & gateway.", cfg.name))?;
        let pin = self.pin_for(host);
        let proxy = if self.scheme == "https" { self.vpn.proxy_url().await } else { None };
        let user = user_of(cfg);
        let key = ClientKey { host: host.into(), user: user.clone(), pin: pin.clone(), proxy: proxy.clone(), password_hash: crate::util::short_hash(&password) };
        if let Some((k, c)) = self.state.lock().unwrap().get(&cfg.id).and_then(|s| s.client.as_ref()) {
            if *k == key {
                return Ok(c.clone());
            }
        }
        let c = Arc::new(Client::new(&format!("{}://{host}", self.scheme), &user, &password, pin, proxy)?);
        self.state.lock().unwrap().entry(cfg.id.clone()).or_default().client = Some((key, c.clone()));
        Ok(c)
    }

    pub async fn via_vpn(&self) -> bool {
        self.vpn.socks().await.is_some()
    }

    /// The RouterOS id of a port, from the last poll (or asked now).
    pub async fn port_id(&self, cfg: &SwitchConfig, port: &str) -> Result<String, String> {
        if let Some(id) = self.state.lock().unwrap().get(&cfg.id).and_then(|s| s.ids.get(port).cloned()) {
            return Ok(id);
        }
        let c = self.client(cfg).await?;
        let via = self.via_vpn().await;
        let ifaces = c.interfaces().await.map_err(|e| e.explain(&user_of(cfg), &cfg.host, via))?;
        ifaces.into_iter().find(|i| i.name == port).map(|i| i.id).ok_or_else(|| format!("{} has no port {port}.", cfg.name))
    }

    /// Polls one switch and keeps the result. Errors keep the last ports (rates zeroed) and say why.
    pub async fn poll(&self, cfg: &SwitchConfig, nodes: &[NodeSnapshot]) -> SwitchSnapshot {
        let snap = match self.read(cfg, nodes).await {
            Ok(s) => s,
            Err((status, message)) => {
                let last = self.state.lock().unwrap().get(&cfg.id).and_then(|s| s.snap.clone());
                let mut s = last.unwrap_or_else(|| placeholder_switch(cfg));
                s.status = status;
                s.error = Some(message);
                s.latency_ms = None;
                for p in &mut s.ports {
                    p.rx_bps = 0.0;
                    p.tx_bps = 0.0;
                }
                s
            }
        };
        self.state.lock().unwrap().entry(cfg.id.clone()).or_default().snap = Some(snap.clone());
        snap
    }

    async fn read(&self, cfg: &SwitchConfig, nodes: &[NodeSnapshot]) -> Result<SwitchSnapshot, (LinkStatus, String)> {
        let c = self.client(cfg).await.map_err(|e| (LinkStatus::Error, e))?;
        let via = self.via_vpn().await;
        let explain = |e: ApiError| {
            let status = if matches!(e, ApiError::Timeout | ApiError::Unreachable(_)) { LinkStatus::Offline } else { LinkStatus::Error };
            let mut message = e.explain(&user_of(cfg), &cfg.host, via);
            if e == ApiError::Auth {
                message.push_str(" Automatic checks wait 30 seconds before retrying. Test connection checks now.");
            }
            (status, message)
        };
        // Preserve the actual rejection and its alert; do not hammer the same account.
        // An explicit Test connection is a separate read, with no automatic retry.
        if c.auth_retry_pending(Instant::now()) {
            return Err(explain(ApiError::Auth));
        }
        let t = Instant::now();
        let resource = c.resource().await.map_err(explain)?;
        let latency = t.elapsed().as_secs_f32() * 1000.0;
        let sensors = client::optional_read(c.health().await).map_err(explain)?;
        let ifaces = c.interfaces().await.map_err(explain)?;
        let ether: Vec<&client::Iface> = ifaces.iter().filter(|i| i.kind == "ether" || i.name.starts_with("qsfp") || i.name.starts_with("sfp") || i.name.starts_with("ether")).collect();
        let watched: Vec<String> = ether.iter().filter(|i| i.running).map(|i| i.name.clone()).collect();
        let monitors = client::optional_read(c.monitor(&watched).await).map_err(explain)?;
        let stats = client::optional_read(c.ethernet_stats().await).map_err(explain)?;
        let stale_tables = self.state.lock().unwrap().get(&cfg.id).and_then(|s| s.tables.as_ref()).is_none_or(|(at, _)| at.elapsed() >= TABLES_EVERY);
        let tables = if stale_tables { Some(c.port_tables().await.map_err(explain)?) } else { None };

        let now = Instant::now();
        let mut st = self.state.lock().unwrap();
        let st = st.entry(cfg.id.clone()).or_default();
        st.ids = ifaces.iter().map(|i| (i.name.clone(), i.id.clone())).collect();
        if let Some(t) = tables {
            st.tables = Some((now, t));
        }
        let wall = crate::demo::unix_now() * 1000;
        st.reverts.retain(|_, at| *at > wall);
        let ports: Vec<SwitchPort> = ether
            .iter()
            .map(|i| {
                let (rx, tx) = match st.counters.get(&i.name) {
                    Some((at, rx0, tx0)) => {
                        let dt = now.duration_since(*at).as_secs_f64();
                        (client::rate_bps(*rx0, i.rx_byte, dt), client::rate_bps(*tx0, i.tx_byte, dt))
                    }
                    None => (0.0, 0.0),
                };
                st.counters.insert(i.name.clone(), (now, i.rx_byte, i.tx_byte));
                let m = monitors.iter().find(|m| m.name == i.name);
                let (fcs, pause) = stats.get(&i.name).copied().unwrap_or((0, 0));
                let worst = m.and_then(|m| m.lane_rx_dbm.iter().cloned().reduce(f32::min));
                let capacity = m.and_then(|m| m.rate_mbps).unwrap_or(if i.name.starts_with("qsfp56-dd") { 200_000 } else if i.name.starts_with("ether") { 1_000 } else { 0 });
                SwitchPort {
                    name: i.name.clone(),
                    comment: i.comment.clone(),
                    running: i.running,
                    disabled: i.disabled,
                    speed: m.and_then(|m| m.rate_mbps).map(|r| if r >= 1_000 { format!("{}G", r / 1_000) } else { format!("{r}M") }).unwrap_or_default(),
                    rx_bps: rx,
                    tx_bps: tx,
                    rx_errors: i.rx_error,
                    tx_errors: i.tx_error,
                    module_temp_c: m.and_then(|m| m.temp_c),
                    rx_power_dbm: worst,
                    // By comment until the fabric monitor places it by MAC or LLDP (mapping::apply).
                    node: node_for_comment(&i.comment, nodes),
                    mapped_by: node_for_comment(&i.comment, nodes).map(|_| "comment".to_string()),
                    mtu: i.mtu,
                    link_downs: i.link_downs,
                    last_link_up_ms: i.last_link_up_ms,
                    rx_drops: i.rx_drop,
                    tx_drops: i.tx_drop,
                    fcs_errors: fcs,
                    rx_pause: pause,
                    tx_power_dbm: m.and_then(|m| m.tx_power_dbm),
                    lane_rx_power_dbm: m.map(|m| m.lane_rx_dbm.clone()).unwrap_or_default(),
                    sfp_vendor: m.and_then(|m| m.vendor.clone()),
                    sfp_part: m.and_then(|m| m.part.clone()),
                    capacity_mbps: capacity,
                    cabling_ok: None,
                    expected_node: None,
                    management: None,
                    revert_at_ms: st.reverts.get(&i.name).copied(),
                }
            })
            .collect();
        let temp = sensors.iter().filter(|s| s.unit == "°C").map(|s| s.value).reduce(f64::max).map(|t| t as f32);
        Ok(SwitchSnapshot {
            id: cfg.id.clone(),
            name: cfg.name.clone(),
            kind: cfg.kind,
            host: cfg.host.clone(),
            status: LinkStatus::Online,
            latency_ms: Some(latency),
            version: resource.version,
            cpu_pct: resource.cpu_pct,
            temp_c: temp,
            uptime_s: resource.uptime_s,
            ports,
            configured: true,
            board: resource.board,
            sensors,
            loss_pct: None,
            error: None,
        })
    }

    pub fn latest(&self, id: &str) -> Option<SwitchSnapshot> {
        self.state.lock().unwrap().get(id).and_then(|s| s.snap.clone())
    }

    /// The last bridge host, neighbor, address and ARP tables read from a switch.
    pub fn tables(&self, id: &str) -> Option<mapping::PortTables> {
        self.state.lock().unwrap().get(id).and_then(|s| s.tables.as_ref().map(|(_, t)| t.clone()))
    }

    /// Notes (or clears) a revert the switch will run for a disabled port.
    pub fn set_revert(&self, id: &str, port: &str, at_ms: Option<u64>) {
        let mut st = self.state.lock().unwrap();
        let st = st.entry(id.to_string()).or_default();
        match at_ms {
            Some(t) => st.reverts.insert(port.to_string(), t),
            None => st.reverts.remove(port),
        };
    }
}

/// Live mode: switches come from the RouterOS client and the probes. Demo snapshots pass through.
pub fn overlay(app: &AppHandle, mut snap: ClusterSnapshot) -> ClusterSnapshot {
    if snap.demo {
        return snap;
    }
    let (Some(switches), Some(prober)) = (app.try_state::<Arc<Switches>>(), app.try_state::<Arc<crate::probe::Prober>>()) else {
        return snap;
    };
    for s in snap.switches.iter_mut() {
        if !s.configured {
            continue;
        }
        *s = match s.kind {
            SwitchKind::RouterOs => switches.latest(&s.id).filter(|x| x.host == s.host).unwrap_or_else(|| s.clone()),
            SwitchKind::Firewalla | SwitchKind::Gateway => prober.snapshot(s),
        };
    }
    snap
}

/// Polls RouterOS switches every 2 s while the data source is Live.
pub fn start(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(POLL_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            every.tick().await;
            let state = handle.state::<crate::AppState>();
            if !crate::is_live(&state) {
                continue;
            }
            let switches: Vec<SwitchConfig> = state.inventory.lock().unwrap().switches.iter().filter(|s| s.kind == SwitchKind::RouterOs && !s.host.trim().is_empty()).cloned().collect();
            let nodes = state.latest.read().unwrap().as_ref().map(|s| s.nodes.clone()).unwrap_or_default();
            let svc = handle.state::<Arc<Switches>>().inner().clone();
            join_all(switches.iter().map(|cfg| svc.poll(cfg, &nodes))).await;
        }
    });
}
