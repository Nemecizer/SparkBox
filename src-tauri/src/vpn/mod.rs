//! Built-in VPN to the Firewalla Gold.
//!
//! In Auto mode SparkBox first tries the cluster directly (you're home, or
//! another VPN app is already connected) and only starts its own tunnel when
//! that fails. The tunnel is user-space and private to SparkBox: nothing else
//! on the Mac is routed through it except the ports listed under
//! "Expose to this Mac".
//!
//! Like Tailscale's netmon, it looks again as soon as the network changes
//! (a different default gateway or Wi-Fi network) or the Mac wakes, and keeps
//! a log of every change with its reason. The settings lock is never held
//! across DNS lookups, Keychain reads or starting the helper, so the window
//! stays responsive while a slow network resolves.
//!
//! At home Auto never tunnels: when no node answers and this Mac is on their
//! network (by its interface's netmask; a VPN app's utun doesn't count), they're
//! off or restarting and the tunnel wouldn't help. The "Direct at home" pin
//! (`home_pin`) also keeps Auto from starting it while the Mac has no network at
//! all, so unplugging the cable at home doesn't send WireGuard handshakes.

pub mod commands;
mod conf;
mod helper;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::future::join_all;
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch, Mutex, Notify};
use tokio::time::timeout;

use crate::wol::MacIface;
use crate::{secrets, util};
pub use helper::helper_path;
use helper::{Helper, HelperEvent};

const KEY_PRIVATE: &str = "vpn:private-key";
const KEY_PRESHARED: &str = "vpn:preshared-key";
const CHECK_EVERY: Duration = Duration::from_secs(20);
const NETWORK_EVERY: Duration = Duration::from_secs(2);
const DIRECT_TIMEOUT: Duration = Duration::from_millis(1200);
const HANDSHAKE_GRACE: Duration = Duration::from_secs(15);
const HANDSHAKE_RETRY: Duration = Duration::from_secs(120);
/// WireGuard re-handshakes every two minutes while in use; older than this means it's stuck.
const STALE_HANDSHAKE_SEC: i64 = 180;
const LOG_KEEP: usize = 50;
const RATES_KEEP: usize = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VpnMode {
    #[default]
    Auto,
    Always,
    Off,
}

/// The non-secret part of the imported profile (saved in vpn.json).
/// The private and preshared keys live only in the Keychain.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnProfile {
    pub protocol: String,
    pub endpoint_host: String,
    pub endpoint_port: u16,
    pub addresses: Vec<String>,
    pub dns: Vec<String>,
    pub mtu: Option<u32>,
    pub peer_public_key: String,
    pub has_preshared_key: bool,
    pub allowed_ips: Vec<String>,
    pub keepalive: Option<u32>,
    pub obfuscation: BTreeMap<String, String>,
    pub imported_at: u64,
}

/// A local port on this Mac that relays to something on the home network.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    pub label: String,
    pub local_port: u16,
    pub target: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnSettings {
    pub mode: VpnMode,
    pub profile: Option<VpnProfile>,
    #[serde(default)]
    pub forwards: Vec<Forward>,
    /// "Direct at home": in Auto, no tunnel while this Mac is on the nodes' network or has no
    /// network at all. Off in a vpn.json saved before it existed.
    #[serde(default)]
    pub home_pin: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VpnState {
    Off,
    Direct,
    Connecting,
    Connected,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardStatus {
    pub local_port: u16,
    pub active: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnStatus {
    pub state: VpnState,
    pub protocol: Option<String>,
    /// The Firewalla's public address. Only the VPN window shows it, masked until asked.
    pub endpoint: Option<String>,
    pub handshake_age_sec: Option<i64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub latency_ms: Option<f32>,
    /// A plain-English explanation of the current state. Never contains the endpoint.
    pub message: Option<String>,
    pub forwards: Vec<ForwardStatus>,
    /// When the current Direct or Connected state began (Unix ms).
    pub up_since_ms: Option<u64>,
    /// Through the tunnel, from byte-count deltas (bits per second).
    pub rx_rate_bps: f64,
    pub tx_rate_bps: f64,
    /// "connect" or "disconnect" while the user has overridden the mode for now.
    #[serde(rename = "override")]
    pub override_mode: Option<String>,
    /// The network the Mac is on, e.g. "Wi-Fi Home · gateway 192.168.50.1".
    pub network: Option<String>,
}

impl VpnStatus {
    fn off(message: &str) -> Self {
        VpnStatus {
            state: VpnState::Off,
            protocol: None,
            endpoint: None,
            handshake_age_sec: None,
            rx_bytes: 0,
            tx_bytes: 0,
            latency_ms: None,
            message: Some(message.into()),
            forwards: Vec::new(),
            up_since_ms: None,
            rx_rate_bps: 0.0,
            tx_rate_bps: 0.0,
            override_mode: None,
            network: None,
        }
    }
}

/// One line of the connection log.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnLogEntry {
    pub ts_ms: u64,
    /// state | network | action | helper
    pub kind: String,
    pub state: Option<VpnState>,
    pub text: String,
}

/// Whether a node answers, directly or through the tunnel.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeReach {
    pub id: String,
    pub name: String,
    pub addr: String,
    /// direct | tunnel
    pub via: String,
    pub ok: bool,
    pub rtt_ms: Option<f32>,
    pub error: Option<String>,
    pub checked_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RatePoint {
    pub ts_ms: u64,
    pub rx_bps: f64,
    pub tx_bps: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnView {
    pub settings: VpnSettings,
    pub status: VpnStatus,
    /// The last 50 changes, newest last.
    pub log: Vec<VpnLogEntry>,
    pub reach: Vec<NodeReach>,
    /// Tunnel throughput, one point per stats update.
    pub rates: Vec<RatePoint>,
}

/// Something on the home network to test reachability against: a node's SSH port, or the Gold.
#[derive(Clone, Debug)]
pub struct ProbeTarget {
    pub id: String,
    pub name: String,
    /// "ip:port"
    pub addr: String,
}

struct Running {
    helper: Helper,
    generation: u64,
    started: Instant,
    /// The SOCKS5 entry point and its credentials (address, user, password).
    socks: Option<(String, String, String)>,
}

struct Inner {
    settings: VpnSettings,
    status: VpnStatus,
    running: Option<Running>,
    /// A tunnel is being started (outside the lock).
    starting: bool,
    generation: u64,
    probes: HashMap<String, oneshot::Sender<Result<f32, String>>>,
    next_probe: u64,
    /// Session-only: Some(true) connect now, Some(false) stay disconnected.
    override_: Option<bool>,
    log: VecDeque<VpnLogEntry>,
    reach: Vec<NodeReach>,
    rates: VecDeque<RatePoint>,
    last_bytes: Option<(Instant, u64, u64)>,
    last_state: Option<VpnState>,
    /// Some(true) direct, Some(false) tunnel: the path the sessions were made on.
    path: Option<bool>,
}

/// The network this Mac is on, from macOS's own tools.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MacNetwork {
    /// "Wi-Fi Home · gateway 192.168.50.1", or None when it can't tell.
    pub text: Option<String>,
    /// There's a default route (through any interface, another VPN app's utun too): somewhere a
    /// tunnel's handshake could go. False with the cable out and Wi-Fi off.
    pub routed: bool,
}

type Emit = Box<dyn Fn(&VpnStatus) + Send + Sync>;
type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;
/// Turns the Firewalla's name into an address (swappable in tests).
type Resolver = Box<dyn Fn(String, u16) -> BoxFut<Result<SocketAddr, String>> + Send + Sync>;
/// Describes the network the Mac is on (swappable in tests).
type NetworkFn = Box<dyn Fn() -> BoxFut<MacNetwork> + Send + Sync>;
type TargetsFn = Box<dyn Fn() -> Vec<ProbeTarget> + Send + Sync>;
/// This Mac's IPv4 interfaces (swappable in tests).
type IfacesFn = Box<dyn Fn() -> Vec<MacIface> + Send + Sync>;
/// One direct TCP connect to "ip:port", timed (swappable in tests, which never probe home addresses).
type ProbeFn = Box<dyn Fn(String) -> BoxFut<Result<f32, String>> + Send + Sync>;

pub struct Vpn {
    emit: Emit,
    /// The `sparkbox-tunnel` executable.
    helper: PathBuf,
    path: PathBuf,
    inner: Mutex<Inner>,
    wake: Notify,
    path_tx: watch::Sender<u64>,
    resolver: std::sync::Mutex<Resolver>,
    network: std::sync::Mutex<Arc<NetworkFn>>,
    targets: std::sync::Mutex<Option<Arc<TargetsFn>>>,
    ifaces: std::sync::Mutex<Arc<IfacesFn>>,
    prober: std::sync::Mutex<Arc<ProbeFn>>,
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn random_token() -> Result<String, String> {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// How long a TCP connection to a node's SSH port takes (probe.rs, the one place SparkBox times it).
fn direct_probe() -> ProbeFn {
    Box::new(|target| Box::pin(async move { crate::probe::connect_time(&target, DIRECT_TIMEOUT).await }))
}

/// Tries every target at once; the first to answer wins (a dead first node doesn't make us wait).
#[cfg(test)]
async fn direct_probe_any(targets: &[ProbeTarget]) -> Result<(ProbeTarget, f32), String> {
    direct_probe_first(&direct_probe(), targets).await.map_err(|rows| {
        rows.last().map(|r| format!("{}: {}", r.name, r.error.clone().unwrap_or_default())).unwrap_or_else(|| "no addresses to try".into())
    })
}

/// `direct_probe_any`, keeping each failure: when nothing answers, every target's row for the
/// table, from this round (so it never shows the last round's answers beside a tunnel attempt).
async fn direct_probe_first(probe: &ProbeFn, targets: &[ProbeTarget]) -> Result<(ProbeTarget, f32), Vec<NodeReach>> {
    let mut pending: FuturesUnordered<_> = targets.iter().map(|t| async move { (t, probe(t.addr.clone()).await) }).collect();
    let mut failed = Vec::new();
    while let Some((t, r)) = pending.next().await {
        match r {
            Ok(ms) => return Ok((t.clone(), ms)),
            Err(e) => failed.push(reach_row(t, "direct", Err(e))),
        }
    }
    // In the targets' order, as probe_direct_all gives them.
    failed.sort_by_key(|r| targets.iter().position(|t| t.id == r.id));
    Err(failed)
}

fn default_resolver() -> Resolver {
    Box::new(|host, port| {
        Box::pin(async move {
            tokio::net::lookup_host((host.as_str(), port))
                .await
                .map_err(|e| format!("Can't resolve {host}: {e}"))?
                .next()
                .ok_or_else(|| format!("Can't resolve {host}"))
        })
    })
}

/// The default route and Wi-Fi network, from macOS's own tools.
fn default_network() -> NetworkFn {
    Box::new(|| {
        Box::pin(async {
            let run = |cmd: &'static str, args: &'static [&'static str]| async move {
                let out = timeout(Duration::from_secs(2), tokio::process::Command::new(cmd).args(args).output()).await.ok()?.ok()?;
                Some(String::from_utf8_lossy(&out.stdout).into_owned())
            };
            // No default route prints "not in table" to stderr and nothing here.
            let route = run("/sbin/route", &["-n", "get", "default"]).await.and_then(|t| crate::health::reach::parse_route(&t));
            let ssid = run("/usr/sbin/networksetup", &["-getairportnetwork", "en0"])
                .await
                .and_then(|t| t.trim().strip_prefix("Current Wi-Fi Network: ").map(|s| s.trim().to_string()));
            let text = match (ssid, route.as_ref().and_then(|r| r.gateway.clone())) {
                (None, None) => None,
                (Some(s), Some(g)) => Some(format!("Wi-Fi {s} · gateway {g}")),
                (Some(s), None) => Some(format!("Wi-Fi {s}")),
                (None, Some(g)) => Some(format!("gateway {g}")),
            };
            MacNetwork { text, routed: route.is_some() }
        })
    })
}

impl Vpn {
    /// `path` is where settings are saved; `emit` receives every status change.
    pub fn new(path: PathBuf, helper: PathBuf, emit: impl Fn(&VpnStatus) + Send + Sync + 'static) -> Arc<Vpn> {
        let settings: VpnSettings = util::load_json_or_default(&path);
        let status = VpnStatus::off(if settings.profile.is_some() { "Starting…" } else { "No VPN profile imported." });
        let (path_tx, _) = watch::channel(0);
        Arc::new(Vpn {
            emit: Box::new(emit),
            helper,
            path,
            inner: Mutex::new(Inner {
                settings,
                status,
                running: None,
                starting: false,
                generation: 0,
                probes: HashMap::new(),
                next_probe: 0,
                override_: None,
                log: VecDeque::new(),
                reach: Vec::new(),
                rates: VecDeque::new(),
                last_bytes: None,
                last_state: None,
                path: None,
            }),
            wake: Notify::new(),
            path_tx,
            resolver: std::sync::Mutex::new(default_resolver()),
            network: std::sync::Mutex::new(Arc::new(default_network())),
            targets: std::sync::Mutex::new(None),
            ifaces: std::sync::Mutex::new(Arc::new(Box::new(crate::wol::mac_interfaces))),
            prober: std::sync::Mutex::new(Arc::new(direct_probe())),
        })
    }

    /// Runs the check loop and the network watcher forever. `targets` lists what to test
    /// reachability against: every node's SSH port (by IP: .local names don't resolve
    /// across a VPN) and the Gold.
    pub fn start<F>(self: &Arc<Self>, targets: F)
    where
        F: Fn() -> Vec<ProbeTarget> + Send + Sync + 'static,
    {
        *self.targets.lock().unwrap() = Some(Arc::new(Box::new(targets)));
        let vpn = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                vpn.check_now().await;
                tokio::select! {
                    _ = tokio::time::sleep(CHECK_EVERY) => {}
                    _ = vpn.wake.notified() => {}
                }
            }
        });
        let vpn = self.clone();
        tauri::async_runtime::spawn(async move { vpn.watch_network().await });
    }

    fn current_targets(&self) -> Vec<ProbeTarget> {
        let f = self.targets.lock().unwrap().clone();
        f.map(|f| f()).unwrap_or_default()
    }

    async fn check_now(self: &Arc<Self>) {
        let targets = self.current_targets();
        self.check(&targets).await;
    }

    /// Looks at the network every 2 s: a new gateway or Wi-Fi network, or the Mac waking up,
    /// triggers a check at once.
    async fn watch_network(self: Arc<Self>) {
        let mut last: Option<MacNetwork> = None;
        let mut clock = (Instant::now(), SystemTime::now());
        loop {
            let network = self.network.lock().unwrap().clone();
            let now = network().await;
            let woke = {
                let (i, s) = (Instant::now(), SystemTime::now());
                let jumped = s.duration_since(clock.1).unwrap_or_default() > i.duration_since(clock.0) + Duration::from_secs(5);
                clock = (i, s);
                jumped
            };
            let changed = last.as_ref().is_some_and(|l| *l != now);
            {
                let mut inner = self.inner.lock().await;
                inner.status.network = now.text.clone();
                if changed || woke {
                    let text = if woke {
                        "The Mac woke from sleep; checking the path again.".to_string()
                    } else {
                        format!("Network changed to {}; checking the path again.", now.text.as_deref().unwrap_or("no network"))
                    };
                    push_log(&mut inner, "network", None, text);
                }
            }
            if changed || woke {
                self.wake.notify_one();
            }
            last = Some(now);
            tokio::time::sleep(NETWORK_EVERY).await;
        }
    }

    /// Whether this Mac has a default route right now. A fresh look, not the watcher's last:
    /// that can be 2 s old, or from before the Mac slept. The window's network line follows it.
    async fn routed_now(&self) -> bool {
        let network = self.network.lock().unwrap().clone();
        let now = network().await;
        self.inner.lock().await.status.network = now.text;
        now.routed
    }

    /// Checks again now (after the Mac wakes).
    pub fn nudge(&self) {
        self.wake.notify_one();
    }

    /// Changes whenever the path to the cluster switches between direct and the tunnel,
    /// so SSH sessions made on the old path can be remade.
    pub fn path_watch(&self) -> watch::Receiver<u64> {
        self.path_tx.subscribe()
    }

    /// The tunnel's SOCKS5 entry point (address, user, password) while it's connected.
    pub async fn socks(&self) -> Option<(String, String, String)> {
        let inner = self.inner.lock().await;
        if inner.status.state != VpnState::Connected {
            return None;
        }
        inner.running.as_ref()?.socks.clone()
    }

    /// The same as a URL, for HTTP clients; requests to home addresses go through it.
    pub async fn proxy_url(&self) -> Option<String> {
        let (addr, user, pass) = self.socks().await?;
        Some(format!("socks5h://{user}:{pass}@{addr}"))
    }

    pub async fn view(&self) -> VpnView {
        let inner = self.inner.lock().await;
        VpnView {
            settings: inner.settings.clone(),
            status: inner.status.clone(),
            log: inner.log.iter().cloned().collect(),
            reach: inner.reach.clone(),
            rates: inner.rates.iter().cloned().collect(),
        }
    }

    pub async fn import(&self, conf_text: &str, endpoint_host: &str) -> Result<VpnView, String> {
        let c = conf::parse(conf_text)?;
        let (file_host, port) = c.endpoint.as_deref().map(conf::split_endpoint).unwrap_or_default();
        let host = match endpoint_host.trim() {
            "" if !file_host.is_empty() => file_host,
            "" => return Err("Enter the Firewalla's public IP address.".into()),
            h => h.to_string(),
        };
        secrets::set(KEY_PRIVATE, &c.private_key)?;
        match &c.preshared_key {
            Some(psk) => secrets::set(KEY_PRESHARED, psk)?,
            None => secrets::clear(KEY_PRESHARED)?,
        }
        let profile = VpnProfile {
            protocol: c.protocol().into(),
            endpoint_host: host,
            endpoint_port: if port == 0 { 51820 } else { port },
            addresses: c.addresses,
            dns: c.dns,
            mtu: c.mtu,
            peer_public_key: c.peer_public_key,
            has_preshared_key: c.preshared_key.is_some(),
            allowed_ips: c.allowed_ips,
            keepalive: c.keepalive,
            obfuscation: c.obfuscation,
            imported_at: unix_now(),
        };
        self.update(|s| s.profile = Some(profile), true, "Profile imported.").await?;
        Ok(self.view().await)
    }

    pub async fn set_endpoint(&self, host: &str) -> Result<VpnView, String> {
        let host = host.trim().to_string();
        if host.is_empty() {
            return Err("Enter the Firewalla's public IP address.".into());
        }
        self.update(|s| if let Some(p) = &mut s.profile { p.endpoint_host = host }, true, "Firewalla address changed.").await?;
        Ok(self.view().await)
    }

    pub async fn set_mode(&self, mode: VpnMode) -> Result<VpnView, String> {
        self.inner.lock().await.override_ = None;
        let label = match mode {
            VpnMode::Auto => "Auto",
            VpnMode::Always => "Always",
            VpnMode::Off => "Off",
        };
        self.update(|s| s.mode = mode, mode == VpnMode::Off, &format!("Mode set to {label}.")).await?;
        Ok(self.view().await)
    }

    /// Turns "Direct at home" on or off (saved in vpn.json); Auto looks again at once.
    pub async fn set_home_pin(&self, on: bool) -> Result<VpnView, String> {
        let why = if on {
            "Direct at home turned on: in Auto, no tunnel while this Mac has no network either."
        } else {
            "Direct at home turned off."
        };
        self.update(|s| s.home_pin = on, false, why).await?;
        Ok(self.view().await)
    }

    /// Connect or disconnect now, regardless of the mode, until changed (not saved).
    /// `None` goes back to following the mode.
    pub async fn set_override(&self, connect: Option<bool>) -> VpnView {
        {
            let mut inner = self.inner.lock().await;
            inner.override_ = connect;
            inner.status.override_mode = connect.map(|c| if c { "connect" } else { "disconnect" }.to_string());
            let text = match connect {
                Some(true) => "Connect chosen: using the tunnel until you choose otherwise.",
                Some(false) => "Disconnect chosen: no tunnel until you choose otherwise.",
                None => "Back to following the mode.",
            };
            push_log(&mut inner, "action", None, text.into());
        }
        if connect == Some(false) {
            self.stop_tunnel(VpnState::Off, "Disconnected by you. Choose Connect, or Auto mode, to use the tunnel again.").await;
        } else if connect == Some(true) {
            self.stop_tunnel(VpnState::Connecting, "Connecting…").await;
        }
        self.wake.notify_one();
        self.view().await
    }

    pub async fn set_forwards(&self, forwards: Vec<Forward>) -> Result<VpnView, String> {
        let mut seen = std::collections::HashSet::new();
        for f in &forwards {
            if f.local_port == 0 {
                return Err(format!("\"{}\" needs a local port from 1 to 65535.", f.label));
            }
            if !seen.insert(f.local_port) {
                return Err(format!("Local port {} is used twice.", f.local_port));
            }
            let ok = f.target.rsplit_once(':').is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0));
            if !ok {
                return Err(format!("\"{}\" needs a home address like 192.168.50.51:3000.", f.label));
            }
        }
        self.update(|s| s.forwards = forwards, true, "Exposed ports changed.").await?;
        Ok(self.view().await)
    }

    pub async fn reconnect(&self) -> VpnView {
        self.stop_tunnel(VpnState::Connecting, "Reconnecting…").await;
        self.wake.notify_one();
        self.view().await
    }

    pub async fn remove(&self) -> Result<VpnView, String> {
        secrets::clear(KEY_PRIVATE)?;
        secrets::clear(KEY_PRESHARED)?;
        self.update(|s| s.profile = None, true, "Profile removed.").await?;
        Ok(self.view().await)
    }

    /// Applies a settings change, saves it, and re-checks; `restart` drops a
    /// running tunnel so it comes back with the new settings.
    async fn update(&self, change: impl FnOnce(&mut VpnSettings), restart: bool, why: &str) -> Result<(), String> {
        let settings = {
            let mut inner = self.inner.lock().await;
            change(&mut inner.settings);
            push_log(&mut inner, "action", None, why.into());
            inner.settings.clone()
        };
        util::save_json(&self.path, &settings)?;
        if restart {
            self.stop_tunnel(VpnState::Connecting, "Applying changes…").await;
        }
        self.wake.notify_one();
        Ok(())
    }

    async fn check(self: &Arc<Self>, targets: &[ProbeTarget]) {
        let (mode, has_profile, home_pin) = {
            let inner = self.inner.lock().await;
            let mode = match inner.override_ {
                Some(true) => VpnMode::Always,
                Some(false) => VpnMode::Off,
                None => inner.settings.mode,
            };
            (mode, inner.settings.profile.is_some(), inner.settings.home_pin)
        };
        let probe = self.prober.lock().unwrap().clone();
        if !has_profile || mode == VpnMode::Off {
            let why = match (has_profile, self.inner.lock().await.override_) {
                (false, _) => "No VPN profile imported.",
                (_, Some(false)) => "Disconnected by you. Choose Connect, or Auto mode, to use the tunnel again.",
                _ => "VPN is turned off.",
            };
            self.stop_tunnel(VpnState::Off, why).await;
            let reach = probe_direct_all(&probe, targets).await;
            self.inner.lock().await.reach = reach;
            return;
        }
        if mode == VpnMode::Auto && !targets.is_empty() {
            let first = direct_probe_first(&probe, targets).await;
            if let Err(rows) = &first {
                // Nothing answered directly: the table says so while the tunnel starts.
                self.inner.lock().await.reach = rows.clone();
                // A matching subnet alone cannot identify home: hotels and hotspots can
                // overlap it. Suppress Auto only when the owner explicitly pins Direct at home.
                let ifaces = self.ifaces.lock().unwrap().clone();
                if let Some(mac) = on_nodes_lan(targets, &ifaces()).filter(|_| home_pin) {
                    let why = format!("No node answers. Direct at home is on and this Mac ({mac}) has a matching local subnet. Choose Connect if this is a different network using the same addresses.");
                    return self.stop_tunnel(VpnState::Direct, &why).await;
                }
                // Direct at home: with no network at all (the cable out, Wi-Fi off) a handshake has
                // nowhere to go, so Auto waits for the network to come back instead.
                if home_pin && !self.routed_now().await {
                    let why = "This Mac has no network right now. Direct at home is on, so SparkBox waits for the network instead of starting the tunnel.";
                    return self.stop_tunnel(VpnState::Direct, why).await;
                }
            }
            if let Ok((first, first_ms)) = first {
                // The log quotes the same probe numbers the table shows: the fastest of one round.
                let reach = probe_direct_all(&probe, targets).await;
                let (name, ms) = reach
                    .iter()
                    .filter_map(|r| r.rtt_ms.map(|ms| (r.name.clone(), ms)))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap_or((first.name.clone(), first_ms));
                self.stop_tunnel(
                    VpnState::Direct,
                    &format!("The cluster is reachable directly ({name} answered in {}): home network, or another VPN app is connected.", crate::probe::ms_text(ms)),
                )
                .await;
                let mut inner = self.inner.lock().await;
                inner.status.latency_ms = Some(ms);
                inner.reach = reach;
                let status = inner.status.clone();
                self.emit(&status);
                return;
            }
        }

        if let Err(e) = self.ensure_tunnel().await {
            return self
                .set_status(|s| {
                    s.state = VpnState::Error;
                    s.message = Some(e);
                })
                .await;
        }

        // Health: a stuck handshake gets a fresh tunnel; none yet after the grace period gets explained.
        let (age, since_start) = {
            let inner = self.inner.lock().await;
            let started = inner.running.as_ref().map(|r| r.started.elapsed()).unwrap_or_default();
            (inner.status.handshake_age_sec, started)
        };
        // A profile can disable periodic keepalives; after its engine exhausts the
        // handshake attempts, no application traffic is admitted to restart them.
        // Re-resolve DNS and replace sockets at a bounded interval even without a first handshake.
        if age.is_none() && since_start > HANDSHAKE_RETRY {
            self.stop_tunnel(VpnState::Connecting, "No handshake completed; retrying with a fresh tunnel…").await;
            self.wake.notify_one();
            return;
        }
        if age.is_some_and(|a| a > STALE_HANDSHAKE_SEC) {
            self.stop_tunnel(VpnState::Connecting, "Handshake went stale; reconnecting…").await;
            self.wake.notify_one();
            return;
        }
        if age.is_none() && since_start > HANDSHAKE_GRACE && self.inner.lock().await.status.state != VpnState::Error {
            return self
                .set_status(|s| {
                    s.state = VpnState::Error;
                    s.message = Some(
                        "No answer from the Firewalla. Check its address, that the VPN server is on, and that this network allows UDP to that port. Still trying…"
                            .into(),
                    );
                })
                .await;
        }

        if age.is_some() && !targets.is_empty() {
            let checks = join_all(targets.iter().map(|t| async move { (t, self.tunnel_probe(&t.addr).await) })).await;
            let reach: Vec<NodeReach> = checks.into_iter().map(|(t, r)| reach_row(t, "tunnel", r)).collect();
            let first = reach.iter().find(|r| r.ok).and_then(|r| r.rtt_ms);
            let none = reach.iter().all(|r| !r.ok);
            let mut inner = self.inner.lock().await;
            inner.reach = reach;
            inner.status.latency_ms = first;
            inner.status.message = none.then(|| "The tunnel is up, but no node answered through it. Check AllowedIPs in the profile.".into());
            changed(self, &mut inner);
        }
    }

    async fn ensure_tunnel(self: &Arc<Self>) -> Result<(), String> {
        // Take what's needed and let go of the lock: DNS, the Keychain and starting
        // a process can all be slow, and the window must keep answering meanwhile.
        let (profile, forwards, generation) = {
            let mut inner = self.inner.lock().await;
            if inner.override_ == Some(false) || (inner.override_.is_none() && inner.settings.mode == VpnMode::Off) {
                return Ok(());
            }
            if inner.running.is_some() || inner.starting {
                return Ok(());
            }
            inner.starting = true;
            inner.generation += 1;
            let profile = inner.settings.profile.clone();
            let forwards: Vec<Forward> = inner.settings.forwards.iter().filter(|f| f.enabled).cloned().collect();
            (profile, forwards, inner.generation)
        };
        let prepared = async {
            let profile = profile.ok_or("No VPN profile imported.")?;
            let resolve = {
                let r = self.resolver.lock().unwrap();
                r(profile.endpoint_host.clone(), profile.endpoint_port)
            };
            let endpoint = resolve.await?;
            let private_key = secrets::get(KEY_PRIVATE)?
                .ok_or("The VPN private key is missing from the Keychain; import the profile again.")?;
            let preshared = if profile.has_preshared_key {
                Some(secrets::get(KEY_PRESHARED)?.ok_or("The VPN preshared key is missing from the Keychain; import the profile again.")?)
            } else { None };
            let (user, pass) = (random_token()?, random_token()?);
            let config = json!({
                "privateKey": private_key,
                "addresses": profile.addresses,
                "dns": profile.dns,
                "mtu": profile.mtu.unwrap_or(0),
                "peerPublicKey": profile.peer_public_key,
                "presharedKey": preshared.unwrap_or_default(),
                "endpoint": endpoint.to_string(),
                "allowedIps": profile.allowed_ips,
                // Keeps NAT mappings open even if the profile doesn't ask for it.
                "keepalive": profile.keepalive.unwrap_or(25),
                "obfuscation": profile.obfuscation,
                "socksUser": user,
                "socksPass": pass,
            });
            let (tx, rx) = mpsc::unbounded_channel();
            let helper = Helper::spawn(&self.helper, &config, tx).await?;
            Ok::<_, String>((profile, endpoint, helper, rx, user, pass))
        }
        .await;

        let mut inner = self.inner.lock().await;
        if inner.generation != generation { return Ok(()); } // settings changed while preparing
        inner.starting = false;
        let (profile, endpoint, helper, rx, user, pass) = prepared?;
        if inner.running.is_some() {
            return Ok(()); // someone else got there first; ours is dropped (and the process killed)
        }
        for f in &forwards {
            helper.send(json!({
                "cmd": "forward",
                "id": f.local_port.to_string(),
                "listen": format!("127.0.0.1:{}", f.local_port),
                "target": f.target,
            }));
        }
        inner.running = Some(Running { helper, generation, started: Instant::now(), socks: None });
        inner.last_bytes = None;
        let keep = (inner.status.override_mode.clone(), inner.status.network.clone());
        inner.status = VpnStatus {
            state: VpnState::Connecting,
            protocol: Some(profile.protocol.clone()),
            endpoint: Some(endpoint.to_string()),
            message: Some("Connecting to the Firewalla…".into()),
            forwards: forwards.iter().map(|f| ForwardStatus { local_port: f.local_port, active: false, error: None }).collect(),
            override_mode: keep.0,
            network: keep.1,
            ..VpnStatus::off("")
        };
        changed(self, &mut inner);
        drop(inner);

        let vpn = self.clone();
        tauri::async_runtime::spawn(async move { vpn.pump(rx, generation, user, pass).await });
        Ok(())
    }

    /// Applies events from one helper; events from a replaced helper are ignored.
    async fn pump(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<HelperEvent>, generation: u64, user: String, pass: String) {
        while let Some(ev) = rx.recv().await {
            let mut inner = self.inner.lock().await;
            if inner.running.as_ref().map(|r| r.generation) != Some(generation) {
                return;
            }
            match ev {
                HelperEvent::Ready { socks } => {
                    if let Some(r) = &mut inner.running {
                        r.socks = Some((socks, user.clone(), pass.clone()));
                    }
                }
                HelperEvent::Stats { handshake_age_sec, rx_bytes, tx_bytes, udp_sent, udp_received, udp_send_errors } => {
                    let now = Instant::now();
                    if let Some((t, rx0, tx0)) = inner.last_bytes {
                        let dt = now.duration_since(t).as_secs_f64();
                        if dt > 0.2 {
                            // A restarted counter counts from zero, not backwards.
                            let rx_bps = rx_bytes.saturating_sub(rx0) as f64 * 8.0 / dt;
                            let tx_bps = tx_bytes.saturating_sub(tx0) as f64 * 8.0 / dt;
                            inner.status.rx_rate_bps = rx_bps;
                            inner.status.tx_rate_bps = tx_bps;
                            inner.rates.push_back(RatePoint { ts_ms: now_ms(), rx_bps, tx_bps });
                            while inner.rates.len() > RATES_KEEP {
                                inner.rates.pop_front();
                            }
                            inner.last_bytes = Some((now, rx_bytes, tx_bytes));
                        }
                    } else {
                        inner.last_bytes = Some((now, rx_bytes, tx_bytes));
                    }
                    let overdue = inner.running.as_ref().is_some_and(|r| r.started.elapsed() > HANDSHAKE_GRACE);
                    let s = &mut inner.status;
                    s.rx_bytes = rx_bytes;
                    s.tx_bytes = tx_bytes;
                    s.handshake_age_sec = (handshake_age_sec >= 0).then_some(handshake_age_sec);
                    if (0..=STALE_HANDSHAKE_SEC).contains(&handshake_age_sec) && s.state != VpnState::Connected {
                        s.state = VpnState::Connected;
                        s.message = None;
                        self.wake.notify_one(); // probe nodes promptly after the first handshake
                    } else if handshake_age_sec < 0 && overdue {
                        s.state = VpnState::Error;
                        s.message = Some(handshake_failure(udp_sent, udp_received, udp_send_errors));
                    }
                }
                HelperEvent::Probe { id, ok, ms, error } => {
                    if let Some(tx) = inner.probes.remove(&id) {
                        let _ = tx.send(if ok { Ok(ms) } else { Err(error.unwrap_or_default()) });
                    }
                }
                HelperEvent::Forward { id, ok, listen, error } => {
                    if let Some(f) = inner.status.forwards.iter_mut().find(|f| f.local_port.to_string() == id) {
                        f.active = ok;
                        f.error = (!ok).then(|| {
                            let why = error.unwrap_or_default();
                            if why.contains("address already in use") {
                                format!("Port {} is already used by another app on this Mac.", f.local_port)
                            } else {
                                why
                            }
                        });
                    } else if ok {
                        eprintln!("unexpected forward on {listen}");
                    }
                }
                HelperEvent::Error { message } => {
                    inner.status.state = VpnState::Error;
                    inner.status.message = Some(format!("Tunnel error: {message}"));
                }
                HelperEvent::Log { message } => {
                    push_log(&mut inner, "helper", None, message);
                    continue;
                }
                HelperEvent::Exited => {
                    inner.running = None;
                    inner.status.state = VpnState::Error;
                    if inner.status.message.is_none() || inner.status.handshake_age_sec.is_none() {
                        inner.status.message = Some("The tunnel helper stopped; retrying shortly.".into());
                    }
                    changed(&self, &mut inner);
                    return;
                }
            }
            changed(&self, &mut inner);
        }
    }

    async fn tunnel_probe(&self, target: &str) -> Result<f32, String> {
        let rx = {
            let mut inner = self.inner.lock().await;
            inner.next_probe += 1;
            let id = inner.next_probe.to_string();
            let (tx, rx) = oneshot::channel();
            let running = inner.running.as_ref().ok_or("no tunnel")?;
            running.helper.send(json!({ "cmd": "probe", "id": id, "target": target }));
            inner.probes.insert(id, tx);
            rx
        };
        match timeout(Duration::from_secs(6), rx).await {
            Ok(Ok(result)) => result,
            _ => Err("timed out".into()),
        }
    }

    async fn stop_tunnel(&self, state: VpnState, message: &str) {
        let mut inner = self.inner.lock().await;
        inner.generation += 1;
        inner.starting = false;
        let was_running = inner.running.take().is_some();
        inner.probes.clear();
        let differs = was_running || inner.status.state != state || inner.status.message.as_deref() != Some(message);
        if differs {
            let keep = (inner.status.override_mode.clone(), inner.status.network.clone(), inner.status.up_since_ms);
            inner.status = VpnStatus { state, ..VpnStatus::off(message) };
            inner.status.override_mode = keep.0;
            inner.status.network = keep.1;
            if inner.last_state == Some(state) {
                inner.status.up_since_ms = keep.2;
            }
            inner.rates.clear();
            inner.last_bytes = None;
            changed(self, &mut inner);
        }
    }

    async fn set_status(&self, f: impl FnOnce(&mut VpnStatus)) {
        let mut inner = self.inner.lock().await;
        f(&mut inner.status);
        changed(self, &mut inner);
    }

    fn emit(&self, status: &VpnStatus) {
        (self.emit)(status);
    }

    /// Tests: pretend the path to the cluster switched (direct ↔ tunnel).
    #[cfg(test)]
    pub async fn simulate_path(&self, direct: bool) {
        let mut inner = self.inner.lock().await;
        inner.status.state = if direct { VpnState::Direct } else { VpnState::Connected };
        changed(self, &mut inner);
    }

    #[cfg(test)]
    fn set_resolver(&self, r: Resolver) {
        *self.resolver.lock().unwrap() = r;
    }

    #[cfg(test)]
    fn set_network(&self, n: NetworkFn) {
        *self.network.lock().unwrap() = Arc::new(n);
    }

    #[cfg(test)]
    fn set_ifaces(&self, f: IfacesFn) {
        *self.ifaces.lock().unwrap() = Arc::new(f);
    }

    #[cfg(test)]
    fn set_prober(&self, p: ProbeFn) {
        *self.prober.lock().unwrap() = Arc::new(p);
    }
}

/// What to test reachability against: each node's SSH port by IP, and the Gold's (when set up).
pub fn targets(inv: &crate::inventory::Inventory) -> Vec<ProbeTarget> {
    let mut out: Vec<ProbeTarget> = inv
        .nodes
        .iter()
        .filter_map(|n| Some(ProbeTarget { id: n.id.clone(), name: n.name.clone(), addr: format!("{}:{}", n.ip.as_deref().filter(|ip| !ip.is_empty())?, n.port) }))
        .collect();
    for s in inv.switches.iter().filter(|s| s.kind == crate::inventory::SwitchKind::Gateway && !s.host.trim().is_empty()) {
        out.push(ProbeTarget { id: s.id.clone(), name: s.name.clone(), addr: format!("{}:22", s.host.trim()) });
    }
    out
}

fn push_log(inner: &mut Inner, kind: &str, state: Option<VpnState>, text: String) {
    inner.log.push_back(VpnLogEntry { ts_ms: now_ms(), kind: kind.into(), state, text });
    while inner.log.len() > LOG_KEEP {
        inner.log.pop_front();
    }
}

fn state_word(s: VpnState) -> &'static str {
    match s {
        VpnState::Off => "Off",
        VpnState::Direct => "Direct",
        VpnState::Connecting => "Connecting",
        VpnState::Connected => "Connected",
        VpnState::Error => "Problem",
    }
}

/// After any status change: log a new state with its reason, note when it began, tell
/// SSH sessions if the path to the cluster switched, and send it to the UI.
fn changed(vpn: &Vpn, inner: &mut Inner) {
    let state = inner.status.state;
    if inner.last_state != Some(state) {
        let reason = inner.status.message.clone().unwrap_or_else(|| match state {
            VpnState::Connected => "Handshake with the Firewalla completed.".into(),
            _ => String::new(),
        });
        let text = if reason.is_empty() { state_word(state).to_string() } else { format!("{}: {reason}", state_word(state)) };
        push_log(inner, "state", Some(state), text);
        inner.status.up_since_ms = matches!(state, VpnState::Direct | VpnState::Connected).then(now_ms);
        let path = match state {
            VpnState::Direct | VpnState::Off => Some(true),
            VpnState::Connected => Some(false),
            _ => None,
        };
        if let Some(p) = path {
            if inner.path.is_some_and(|old| old != p) || !p {
                vpn.path_tx.send_modify(|g| *g += 1);
            }
            inner.path = Some(p);
        }
        inner.last_state = Some(state);
    }
    vpn.emit(&inner.status);
}

fn handshake_failure(sent: u64, received: u64, errors: u64) -> String {
    let next = if errors > 0 { "macOS could not send UDP traffic. Check this Mac's network and VPN permissions." }
        else if sent == 0 { "The tunnel has not sent a handshake. Choose Reconnect." }
        else if received == 0 { "No UDP reply arrived. Compare the server address and port with the working WireGuard profile." }
        else { "UDP arrived, but no authenticated handshake completed. Reimport the working profile to check its keys." };
    format!("No handshake yet ({sent} UDP packets sent, {received} received, {errors} send errors). {next} Keep the WireGuard app disconnected while testing. Still trying…")
}

fn reach_row(t: &ProbeTarget, via: &str, r: Result<f32, String>) -> NodeReach {
    let (ok, rtt_ms, error) = match r {
        Ok(ms) => (true, Some(ms), None),
        Err(e) => (false, None, Some(e)),
    };
    NodeReach { id: t.id.clone(), name: t.name.clone(), addr: t.addr.clone(), via: via.into(), ok, rtt_ms, error, checked_ms: now_ms() }
}

/// This Mac's address on one of the targets' networks (by its interface's netmask; tunnels
/// never count), when it's on the cluster's LAN.
fn on_nodes_lan(targets: &[ProbeTarget], ifaces: &[MacIface]) -> Option<std::net::Ipv4Addr> {
    targets.iter().find_map(|t| crate::wol::mac_on_lan(t.addr.rsplit_once(':').map(|(host, _)| host), ifaces)).map(|i| i.addr)
}

async fn probe_direct_all(probe: &ProbeFn, targets: &[ProbeTarget]) -> Vec<NodeReach> {
    join_all(targets.iter().map(|t| async move { reach_row(t, "direct", probe(t.addr.clone()).await) })).await
}

#[cfg(test)]
mod tests {
    //! The manager against a real helper and a fake Firewalla (another helper process).
    use super::helper::tests::{built_helper, CLIENT, SERVER};
    use super::*;
    use crate::wol::iface;

    // Tests share the in-memory key store, so they take turns.
    static SERIAL: Mutex<()> = Mutex::const_new(());

    fn manager(name: &str) -> Arc<Vpn> {
        let path = std::env::temp_dir().join(format!("sparkbox-vpn-test-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Vpn::new(path, built_helper(), |_| {})
    }

    fn target(addr: &str) -> Vec<ProbeTarget> {
        vec![ProbeTarget { id: "cell1".into(), name: "cell1".into(), addr: addr.into() }]
    }

    const AWG: &str = "Jc = 3\nJmin = 30\nJmax = 60\nS1 = 12\nS2 = 17\nH1 = 111111\nH2 = 222222\nH3 = 333333\nH4 = 444444\n";

    fn conf(port: u16, awg: bool) -> String {
        format!(
            "[Interface]\nPrivateKey = {}\nAddress = 10.98.0.2/32\n{}\n[Peer]\nPublicKey = {}\nEndpoint = 127.0.0.1:{port}\nAllowedIPs = 10.98.0.0/24\nPersistentKeepalive = 25\n",
            CLIENT.0,
            if awg { AWG } else { "" },
            SERVER.1
        )
    }

    /// Starts a helper acting as the Firewalla; returns it (keep it alive) and its UDP port.
    async fn fake_firewalla(awg: bool) -> (Helper, u16) {
        let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let obfuscation = if awg {
            json!({ "jc": "3", "jmin": "30", "jmax": "60", "s1": "12", "s2": "17",
                    "h1": "111111", "h2": "222222", "h3": "333333", "h4": "444444" })
        } else {
            json!({})
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let helper = Helper::spawn(&built_helper(), &json!({
            "privateKey": SERVER.0, "addresses": ["10.98.0.1/24"], "peerPublicKey": CLIENT.1,
            "allowedIps": ["10.98.0.2/32"], "obfuscation": obfuscation, "listenPort": port,
        }), tx).await.unwrap();
        while !matches!(rx.recv().await, Some(HelperEvent::Ready { .. })) {}
        (helper, port)
    }

    async fn wait_for(vpn: &Vpn, want: VpnState) -> VpnStatus {
        for _ in 0..100 {
            let s = vpn.view().await.status;
            if s.state == want {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("never reached {want:?}; last status: {:?}", vpn.view().await.status);
    }

    #[tokio::test]
    async fn unanswered_handshake_reports_udp_attempts_without_exposing_profile_keys() {
        let _turn = SERIAL.lock().await;
        let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let vpn = manager("udp-evidence");
        vpn.import(&conf(silent.local_addr().unwrap().port(), false), "127.0.0.1").await.unwrap();
        vpn.set_mode(VpnMode::Always).await.unwrap();
        vpn.ensure_tunnel().await.unwrap();
        vpn.inner.lock().await.running.as_mut().unwrap().started -= HANDSHAKE_GRACE + Duration::from_secs(1);
        let message = wait_for(&vpn, VpnState::Error).await.message.unwrap();
        assert!(message.contains("No UDP reply arrived"), "{message}");
        assert!(!message.contains("127.0.0.1") && !message.contains(CLIENT.0) && !message.contains(SERVER.1));
        assert!(vpn.socks().await.is_none(), "unconfirmed tunnel is never passed to SSH");
        let old_generation = vpn.inner.lock().await.generation;
        vpn.inner.lock().await.running.as_mut().unwrap().started -= HANDSHAKE_RETRY;
        vpn.check(&[]).await;
        assert!(vpn.inner.lock().await.running.is_none(), "expired first-handshake attempt is retired");
        vpn.check(&[]).await;
        assert!(vpn.inner.lock().await.running.as_ref().is_some_and(|r| r.generation > old_generation));
    }

    #[tokio::test]
    async fn first_tunnel_and_reconnected_tunnel_refresh_ssh_sessions() {
        let vpn = manager("initial-path");
        let mut path = vpn.path_watch();
        vpn.simulate_path(false).await;
        assert!(path.has_changed().unwrap(), "first tunnel replaces initial direct attempts");
        path.borrow_and_update();
        vpn.simulate_path(false).await;
        assert!(!path.has_changed().unwrap(), "stats alone do not reconnect SSH");
        vpn.stop_tunnel(VpnState::Connecting, "Reconnecting").await;
        vpn.simulate_path(false).await;
        assert!(path.has_changed().unwrap(), "new tunnel has a new SOCKS listener");
        path.borrow_and_update();
        vpn.stop_tunnel(VpnState::Off, "Disconnected").await;
        assert!(path.has_changed().unwrap(), "disconnect restores direct SSH promptly");
    }

    #[tokio::test]
    async fn disconnect_during_resolution_cancels_the_pending_tunnel() {
        let _turn = SERIAL.lock().await;
        let vpn = manager("cancel-start");
        vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
        let (tx, rx) = oneshot::channel();
        let rx = std::sync::Mutex::new(Some(rx));
        vpn.set_resolver(Box::new(move |_, _| {
            let rx = rx.lock().unwrap().take().unwrap();
            Box::pin(async move { rx.await.unwrap(); Ok("127.0.0.1:9".parse().unwrap()) })
        }));
        let start = tokio::spawn({ let vpn = vpn.clone(); async move { vpn.ensure_tunnel().await } });
        while !vpn.inner.lock().await.starting { tokio::task::yield_now().await; }
        vpn.set_override(Some(false)).await;
        tx.send(()).unwrap();
        start.await.unwrap().unwrap();
        assert!(vpn.inner.lock().await.running.is_none());
        assert_eq!(vpn.view().await.status.state, VpnState::Off);
    }

    #[test]
    fn no_handshake_diagnosis_distinguishes_udp_from_authentication() {
        assert!(handshake_failure(0, 0, 1).contains("could not send"));
        assert!(handshake_failure(3, 0, 0).contains("No UDP reply"));
        assert!(handshake_failure(3, 2, 0).contains("no authenticated handshake"));
    }

    #[tokio::test]
    async fn always_mode_connects_over_amneziawg() {
        let _turn = SERIAL.lock().await;
        let (_firewalla, port) = fake_firewalla(true).await;
        let vpn = manager("always");

        let view = vpn.import(&conf(port, true), "127.0.0.1").await.unwrap();
        assert_eq!(view.settings.profile.unwrap().protocol, "amneziawg");
        assert_eq!(secrets::get(KEY_PRIVATE).unwrap().as_deref(), Some(CLIENT.0), "key went to the key store");

        vpn.set_mode(VpnMode::Always).await.unwrap();
        vpn.check(&target("10.98.0.1:22")).await;
        let s = wait_for(&vpn, VpnState::Connected).await;
        assert_eq!(s.protocol.as_deref(), Some("amneziawg"));
        assert!(s.handshake_age_sec.is_some());
        assert!(s.up_since_ms.is_some());
        let view = vpn.view().await;
        assert!(view.log.iter().any(|e| e.state == Some(VpnState::Connected)), "{:?}", view.log);
        assert!(!s.message.unwrap_or_default().contains("127.0.0.1"), "the endpoint never appears in messages");
    }

    #[tokio::test]
    async fn auto_mode_stays_direct_when_the_cluster_answers() {
        let _turn = SERIAL.lock().await;
        let cell1 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let vpn = manager("direct");
        vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();

        vpn.check(&target(&cell1.local_addr().unwrap().to_string())).await;
        let view = vpn.view().await;
        assert_eq!(view.status.state, VpnState::Direct);
        assert!(view.status.latency_ms.is_some());
        assert!(view.reach.iter().all(|r| r.ok && r.via == "direct"));
        assert!(vpn.inner.lock().await.running.is_none(), "no tunnel while direct works");
    }

    #[test]
    fn at_home_a_silent_cluster_is_not_a_reason_to_tunnel() {
        let t = |addr: &str| ProbeTarget { id: "cell1".into(), name: "cell1".into(), addr: addr.into() };
        let home = [iface("en0", "192.168.50.229", "255.255.255.0")];
        let cafe = [iface("en0", "10.0.0.23", "255.255.255.0")];
        let nodes = [t("192.168.50.51:22"), t("192.168.50.154:22")];
        assert_eq!(on_nodes_lan(&nodes, &home), Some("192.168.50.229".parse().unwrap()));
        assert_eq!(on_nodes_lan(&nodes, &cafe), None, "away: Auto starts the tunnel");
        assert_eq!(on_nodes_lan(&[t("gold.example.net:22")], &home), None, "a name isn't an address on the LAN");
        // The netmask decides, and another VPN app's utun in the LAN's range is not the LAN.
        assert_eq!(on_nodes_lan(&nodes, &[iface("en0", "192.168.48.20", "255.255.252.0")]), Some("192.168.48.20".parse().unwrap()));
        assert_eq!(on_nodes_lan(&nodes, &[cafe[0].clone(), iface("utun4", "192.168.50.7", "255.255.255.0")]), None);
    }

    /// The four Sparks and the Gold as `targets` lists them. Never probed for real here: the
    /// tests below give the manager a prober that answers for them.
    fn home_targets() -> Vec<ProbeTarget> {
        [("cell1", "192.168.50.51"), ("cell2", "192.168.50.154"), ("cell3", "192.168.50.98"), ("gx10-0000", "192.168.50.181"), ("gold", "192.168.50.1")]
            .iter()
            .map(|(id, ip)| ProbeTarget { id: id.to_string(), name: id.to_string(), addr: format!("{ip}:22") })
            .collect()
    }

    /// Counts what Auto tried: direct probes (every one times out, as at 17:32 on Oct 29) and
    /// tunnel starts (the resolver is the first thing a tunnel start calls; it fails, so no
    /// helper process or handshake ever runs in these tests).
    struct Silent {
        probes: Arc<std::sync::atomic::AtomicUsize>,
        tunnels: Arc<std::sync::atomic::AtomicUsize>,
    }

    fn silent_cluster(vpn: &Vpn, ifaces: Vec<MacIface>, routed: bool) -> Silent {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (probes, tunnels) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let p = probes.clone();
        vpn.set_prober(Box::new(move |_| {
            p.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err::<f32, _>("timed out".to_string()) })
        }));
        let t = tunnels.clone();
        vpn.set_resolver(Box::new(move |_, _| {
            t.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err("test: no tunnel here".to_string()) })
        }));
        vpn.set_ifaces(Box::new(move || ifaces.clone()));
        vpn.set_network(Box::new(move || Box::pin(async move { MacNetwork { text: routed.then(|| "gateway 192.168.50.1".into()), routed } })));
        Silent { probes, tunnels }
    }

    impl Silent {
        fn tunnels(&self) -> usize {
            self.tunnels.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn auto_at_home_starts_no_tunnel_when_every_node_is_silent() {
        let _turn = SERIAL.lock().await;
        let homes = [
            ("a /24 home LAN", vec![iface("en0", "192.168.50.113", "255.255.255.0"), iface("bridge100", "10.211.55.2", "255.255.255.0")]),
            // 192.168.48.0/22 holds the nodes' 192.168.50.x: three octets compared said "away".
            ("a /22 home LAN", vec![iface("en0", "192.168.48.20", "255.255.252.0")]),
        ];
        for pin in [false, true] {
            for (what, ifaces) in &homes {
                let vpn = manager("home-silent");
                vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
                vpn.set_home_pin(pin).await.unwrap();
                let seen = silent_cluster(&vpn, ifaces.clone(), true);
                vpn.check(&home_targets()).await;
                let view = vpn.view().await;
                if !pin {
                    assert_eq!(seen.tunnels(), 1, "overlapping local subnets must not suppress Auto");
                    assert_eq!(view.status.state, VpnState::Error, "test resolver refuses");
                    continue;
                }
                assert_eq!(view.status.state, VpnState::Direct, "{what}, pin {pin}: {:?}", view.status);
                assert!(view.status.message.as_deref().unwrap_or_default().contains("matching local subnet"), "{what}: {:?}", view.status.message);
                assert_eq!(seen.tunnels(), 0, "{what}, pin {pin}: Auto tried to start the tunnel");
                assert!(vpn.inner.lock().await.running.is_none(), "{what}: no helper, so no handshake");
                assert!(seen.probes.load(std::sync::atomic::Ordering::SeqCst) >= 5, "every node and the Gold were tried");
                assert!(view.reach.len() == 5 && view.reach.iter().all(|r| !r.ok && r.via == "direct"), "{what}: {:?}", view.reach);
            }
        }
    }

    #[tokio::test]
    async fn a_vpn_apps_utun_in_the_lans_range_is_not_home() {
        let _turn = SERIAL.lock().await;
        // Away at a café, with another VPN app's utun holding an address in 192.168.50.0/24 that
        // doesn't route to the nodes: nothing answers, so Auto starts the tunnel, pin or not.
        for pin in [false, true] {
            let vpn = manager("utun");
            vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
            vpn.set_home_pin(pin).await.unwrap();
            let seen = silent_cluster(&vpn, vec![iface("en0", "10.0.0.23", "255.255.255.0"), iface("utun4", "192.168.50.7", "255.255.255.0")], true);
            vpn.check(&home_targets()).await;
            assert_eq!(seen.tunnels(), 1, "pin {pin}: away, the tunnel is the way in");
            assert_eq!(vpn.view().await.status.state, VpnState::Error, "the test's resolver refuses");
        }
    }

    #[tokio::test]
    async fn direct_at_home_waits_for_the_network_instead_of_tunnelling() {
        let _turn = SERIAL.lock().await;
        // The cable out and Wi-Fi off (Session A step 1): no default route, only Parallels'
        // bridge left, which isn't the nodes' network.
        let unplugged = vec![iface("bridge100", "10.211.55.2", "255.255.255.0")];
        let vpn = manager("unplugged");
        vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
        vpn.set_home_pin(true).await.unwrap();
        let seen = silent_cluster(&vpn, unplugged.clone(), false);
        vpn.check(&home_targets()).await;
        let view = vpn.view().await;
        assert_eq!((view.status.state, seen.tunnels()), (VpnState::Direct, 0), "{:?}", view.status);
        assert!(view.status.message.unwrap().contains("Direct at home is on"));
        assert!(vpn.inner.lock().await.running.is_none());
        assert!(view.log.iter().any(|e| e.kind == "action" && e.text.starts_with("Direct at home turned on")), "{:?}", view.log);

        // Without the pin Auto tries the tunnel, as it always has.
        let vpn = manager("unplugged-nopin");
        vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
        let seen = silent_cluster(&vpn, unplugged.clone(), false);
        vpn.check(&home_targets()).await;
        assert_eq!(seen.tunnels(), 1);

        // With a network again (a café's), the pin doesn't stop it: away, the tunnel is the way in.
        let vpn = manager("unplugged-cafe");
        vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
        vpn.set_home_pin(true).await.unwrap();
        let seen = silent_cluster(&vpn, vec![iface("en0", "10.0.0.23", "255.255.255.0")], true);
        vpn.check(&home_targets()).await;
        assert_eq!(seen.tunnels(), 1);
    }

    #[tokio::test]
    async fn direct_at_home_looks_at_the_network_now_not_at_the_watchers_last_look() {
        let _turn = SERIAL.lock().await;
        use std::sync::atomic::{AtomicBool, Ordering};
        // The watcher looked while Wi-Fi was up; then Wi-Fi went (or the Mac slept and woke) and a
        // check ran before its next look: the 20 s timer, or Live's nudge a second after a wake.
        let vpn = manager("pin-fresh");
        vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
        vpn.set_home_pin(true).await.unwrap();
        let seen = silent_cluster(&vpn, vec![iface("bridge100", "10.211.55.2", "255.255.255.0")], false);
        let routed = Arc::new(AtomicBool::new(true));
        let r = routed.clone();
        vpn.set_network(Box::new(move || {
            let on = r.load(Ordering::SeqCst);
            Box::pin(async move { MacNetwork { text: on.then(|| "Wi-Fi Home · gateway 192.168.50.1".into()), routed: on } })
        }));
        let watcher = tokio::spawn(vpn.clone().watch_network());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(vpn.view().await.status.network.as_deref(), Some("Wi-Fi Home · gateway 192.168.50.1"), "the watcher has looked");
        routed.store(false, Ordering::SeqCst);
        vpn.check(&home_targets()).await;
        let view = vpn.view().await;
        assert_eq!((view.status.state, seen.tunnels()), (VpnState::Direct, 0), "{:?}", view.status);
        assert!(view.status.message.as_deref().unwrap_or_default().contains("no network right now"), "{:?}", view.status.message);
        assert_eq!(view.status.network, None, "the window shows the look Auto went by");
        watcher.abort();

        // The other way: the watcher last saw no route, then Wi-Fi joined a café's network. Away,
        // the tunnel is the way in, without waiting for the watcher's next look.
        let vpn = manager("pin-fresh-cafe");
        vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
        vpn.set_home_pin(true).await.unwrap();
        let seen = silent_cluster(&vpn, vec![iface("en0", "10.0.0.23", "255.255.255.0")], false);
        let routed = Arc::new(AtomicBool::new(false));
        let r = routed.clone();
        vpn.set_network(Box::new(move || {
            let on = r.load(Ordering::SeqCst);
            Box::pin(async move { MacNetwork { text: on.then(|| "Wi-Fi Cafe · gateway 10.0.0.1".into()), routed: on } })
        }));
        let watcher = tokio::spawn(vpn.clone().watch_network());
        tokio::time::sleep(Duration::from_millis(200)).await;
        routed.store(true, Ordering::SeqCst);
        vpn.check(&home_targets()).await;
        assert_eq!(seen.tunnels(), 1);
        watcher.abort();
    }

    #[tokio::test]
    async fn the_home_pin_is_saved_and_an_older_vpn_json_reads_it_as_off() {
        let _turn = SERIAL.lock().await;
        let vpn = manager("pin-saved");
        assert!(!vpn.view().await.settings.home_pin);
        let view = vpn.set_home_pin(true).await.unwrap();
        assert!(view.settings.home_pin);
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&vpn.path).unwrap()).unwrap();
        assert_eq!(saved["homePin"], json!(true), "{saved}");
        assert!(Vpn::new(vpn.path.clone(), built_helper(), |_| {}).view().await.settings.home_pin, "it survives a restart");
        // A vpn.json from before the pin existed.
        std::fs::write(&vpn.path, r#"{"mode":"auto","profile":null,"forwards":[]}"#).unwrap();
        let old = Vpn::new(vpn.path.clone(), built_helper(), |_| {}).view().await.settings;
        assert_eq!((old.mode, old.home_pin), (VpnMode::Auto, false));
        // Turning it off again is saved too.
        vpn.set_home_pin(false).await.unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&vpn.path).unwrap()).unwrap();
        assert_eq!(saved["homePin"], json!(false));
    }

    #[tokio::test]
    async fn the_first_node_to_answer_decides_even_when_the_first_listed_is_dead() {
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        let alive = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let targets = vec![
            ProbeTarget { id: "cell1".into(), name: "cell1".into(), addr: dead.clone() },
            ProbeTarget { id: "gold".into(), name: "Firewalla Gold".into(), addr: alive.local_addr().unwrap().to_string() },
        ];
        let (t, _) = direct_probe_any(&targets).await.unwrap();
        assert_eq!(t.id, "gold");
        assert!(direct_probe_any(&targets[..1]).await.unwrap_err().starts_with("cell1: "));
        assert!(direct_probe_any(&[]).await.is_err());
    }

    #[tokio::test]
    async fn a_network_change_triggers_a_check_and_is_logged() {
        let vpn = manager("netmon");
        let net = Arc::new(std::sync::Mutex::new(Some("Wi-Fi Home · gateway 192.168.50.1".to_string())));
        let n = net.clone();
        vpn.set_network(Box::new(move || {
            let text = n.lock().unwrap().clone();
            Box::pin(async move { MacNetwork { routed: text.is_some(), text } })
        }));
        let watcher = tokio::spawn(vpn.clone().watch_network());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(vpn.view().await.status.network.as_deref(), Some("Wi-Fi Home · gateway 192.168.50.1"), "the window's network line");
        *net.lock().unwrap() = Some("Wi-Fi Cafe · gateway 10.0.0.1".into());
        // The check loop is waiting on `wake`; it fires within one network poll.
        tokio::time::timeout(Duration::from_secs(4), vpn.wake.notified()).await.expect("a check was triggered");
        let view = vpn.view().await;
        assert!(view.log.iter().any(|e| e.kind == "network" && e.text.contains("Wi-Fi Cafe")), "{:?}", view.log);
        assert_eq!(view.status.network.as_deref(), Some("Wi-Fi Cafe · gateway 10.0.0.1"));
        watcher.abort();
    }

    #[tokio::test]
    async fn the_lock_is_free_while_a_slow_resolver_runs() {
        let _turn = SERIAL.lock().await;
        let vpn = manager("slowdns");
        vpn.import(&conf(9, false), "firewalla.example").await.unwrap();
        vpn.set_resolver(Box::new(|_, port| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                Ok(SocketAddr::from(([127, 0, 0, 1], port)))
            })
        }));
        let v = vpn.clone();
        let starting = tokio::spawn(async move { v.ensure_tunnel().await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let t = Instant::now();
        let view = vpn.view().await;
        assert!(t.elapsed() < Duration::from_millis(100), "view waited {:?} on the lock", t.elapsed());
        assert!(view.settings.profile.is_some());
        assert!(vpn.inner.lock().await.starting, "still resolving");
        starting.await.unwrap().unwrap();
        assert!(vpn.inner.lock().await.running.is_some());
    }

    #[tokio::test]
    async fn switching_between_direct_and_tunnel_tells_the_sessions() {
        let vpn = manager("path");
        let mut path = vpn.path_watch();
        vpn.simulate_path(true).await;
        vpn.simulate_path(true).await;
        assert!(!path.has_changed().unwrap(), "the first path isn't a change");
        vpn.simulate_path(false).await;
        assert!(path.has_changed().unwrap());
        path.borrow_and_update();
        vpn.simulate_path(true).await;
        assert!(path.has_changed().unwrap());
    }

    #[tokio::test]
    async fn auto_mode_falls_back_to_the_tunnel_and_remove_stops_it() {
        let _turn = SERIAL.lock().await;
        let (_firewalla, port) = fake_firewalla(false).await;
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        let vpn = manager("fallback");
        // No address typed: the endpoint comes from the file.
        vpn.import(&conf(port, false), "").await.unwrap();

        vpn.check(&target(&closed)).await;
        let s = wait_for(&vpn, VpnState::Connected).await;
        assert_eq!(s.protocol.as_deref(), Some("wireguard"));
        // The table shows this round's direct failure until the tunnel's own probes replace it.
        let reach = vpn.inner.lock().await.reach.clone();
        assert!(reach.len() == 1 && reach[0].id == "cell1" && !reach[0].ok, "{reach:?}");

        // Disconnect overrides Auto until undone.
        let view = vpn.set_override(Some(false)).await;
        assert_eq!(view.status.state, VpnState::Off);
        assert_eq!(view.status.override_mode.as_deref(), Some("disconnect"));
        vpn.check(&target(&closed)).await;
        assert!(vpn.inner.lock().await.running.is_none());
        vpn.set_override(None).await;

        vpn.remove().await.unwrap();
        vpn.check(&target(&closed)).await;
        assert_eq!(vpn.view().await.status.state, VpnState::Off);
        assert!(vpn.inner.lock().await.running.is_none());
        assert_eq!(secrets::get(KEY_PRIVATE).unwrap(), None, "keys are forgotten");
    }

    #[tokio::test]
    async fn off_mode_never_starts_a_tunnel() {
        let _turn = SERIAL.lock().await;
        let vpn = manager("off");
        vpn.import(&conf(9, false), "127.0.0.1").await.unwrap();
        vpn.set_mode(VpnMode::Off).await.unwrap();
        vpn.check(&target("127.0.0.1:1")).await;
        assert_eq!(vpn.view().await.status.state, VpnState::Off);
        assert!(vpn.inner.lock().await.running.is_none());
    }

    #[tokio::test]
    async fn exposed_ports_are_validated() {
        let _turn = SERIAL.lock().await;
        let vpn = manager("forwards");
        let f = |label: &str, port: u16, target: &str| Forward { label: label.into(), local_port: port, target: target.into(), enabled: true };
        assert!(vpn.set_forwards(vec![f("a", 3000, "192.168.50.51:3000"), f("b", 3000, "192.168.50.51:8000")]).await.unwrap_err().contains("used twice"));
        assert!(vpn.set_forwards(vec![f("a", 0, "192.168.50.51:3000")]).await.is_err());
        assert!(vpn.set_forwards(vec![f("a", 3000, "cell1")]).await.unwrap_err().contains("home address"));
        assert_eq!(vpn.set_forwards(vec![f("a", 3000, "192.168.50.51:3000")]).await.unwrap().settings.forwards.len(), 1);
    }

    #[tokio::test]
    async fn a_profile_without_an_address_needs_one_typed_in() {
        let _turn = SERIAL.lock().await;
        let vpn = manager("noaddr");
        let text = conf(9, false).replace("Endpoint = 127.0.0.1:9\n", "");
        assert!(vpn.import(&text, "").await.unwrap_err().contains("public IP"));
        let view = vpn.import(&text, "203.0.113.24").await.unwrap();
        let p = view.settings.profile.unwrap();
        assert_eq!((p.endpoint_host.as_str(), p.endpoint_port), ("203.0.113.24", 51820));
    }
}
