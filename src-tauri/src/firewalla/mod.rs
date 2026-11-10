//! The Firewalla Switch X's ports. The switch has no known local API, SNMP or web UI, so its panel
//! is built from three sources, each shown with where it came from:
//!
//! 1. LLDP, heard by the nodes (no setup). The switch sends it, and every node reports what it
//!    hears in its snapshot, so the window works this out itself (src/windows/switch/switchX.ts).
//! 2. The owner's labels (no setup): what's plugged in where (labels.rs).
//! 3. Firewalla's cloud (MSP) API, once the owner has a login (msp.rs). Unused until then.
//!
//! This file holds the last two: the labels, and the cloud's schedule and last answer, saved in
//! firewalla_msp.json so a restart can't ask sooner than the schedule allows. The token is never
//! in that file: it's the Keychain vault's `firewalla:msp`, and nothing looks at it until a
//! domain is saved and a poll is due. None of this changes the switch.

pub mod commands;
pub mod demo;
pub mod labels;
pub mod msp;
#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::inventory::SwitchKind;
use crate::model::NodeSnapshot;
use crate::util;
use labels::{Labels, PortLabel, PORTS};
use msp::{Client, Gid, MspError, MspSwitch, Schedule, Wanted};

/// The token's name in the Keychain vault.
pub const TOKEN_ACCOUNT: &str = "firewalla:msp";
/// How often the poll loop looks at the schedule. The schedule decides; this asks nothing.
const TICK: Duration = Duration::from_secs(30);
/// Boxes one poll may look through for the switch. An account has one; each costs a call.
const MAX_BOXES: usize = 4;

type TokenFn = Box<dyn Fn() -> Result<Option<String>, String> + Send + Sync>;
type HeldFn = Box<dyn Fn() -> bool + Send + Sync>;

/// firewalla_msp.json. Holds no secret.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    domain: Option<String>,
    /// The box the switch was last found on, so a poll costs one call, not two.
    gid: Option<String>,
    schedule: Schedule,
    last_ok_ms: Option<u64>,
    last: Option<MspSwitch>,
}

/// What the cloud layer is doing, for the panel and Settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MspStatus {
    /// An MSP domain is saved.
    pub configured: bool,
    pub domain: Option<String>,
    /// off | waiting | ok | no-token | keychain | rate-limited | auth | not-json | shape | not-listed | http | unreachable
    pub state: String,
    /// What to tell the owner when it isn't plain sailing.
    pub message: Option<String>,
    pub last_ok_ms: Option<u64>,
    pub last_try_ms: Option<u64>,
    pub next_poll_ms: Option<u64>,
    /// The last good answer. Stays through a failure that says nothing against it (the panel
    /// judges its age), and is dropped for good when the cloud's answer says it may not be this
    /// switch's, so no restart, 429 or new token can bring it back.
    pub sw: Option<MspSwitch>,
    /// Look-only mode is holding the poll.
    pub held: bool,
}

/// Everything the Switch X panel needs that isn't in the cluster snapshot.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortsView {
    pub labels: BTreeMap<u32, PortLabel>,
    pub msp: MspStatus,
}

/// What Test connection found.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MspTestResult {
    pub ok: bool,
    pub kind: Option<String>,
    pub message: String,
}

impl MspTestResult {
    fn refused(kind: &str, message: impl Into<String>) -> MspTestResult {
        MspTestResult { ok: false, kind: Some(kind.into()), message: message.into() }
    }
}

pub struct Firewalla {
    labels_path: PathBuf,
    msp_path: PathBuf,
    token: TokenFn,
    /// Look-only mode is on: looked at before every request, so one that comes on mid-round stops it.
    held: HeldFn,
    labels: Mutex<Labels>,
    /// Demo's labels: samples, changed in memory only, so Demo never touches the owner's.
    demo_labels: Mutex<Labels>,
    saved: Mutex<Saved>,
    /// The last round's failure: (state, sentence).
    problem: Mutex<Option<(String, String)>>,
    /// One round of requests at a time: a Test and a poll can't overlap.
    asking: tokio::sync::Mutex<()>,
    /// Goes up when the domain or token changes, so an answer already on its way is set aside.
    generation: AtomicU64,
    /// The test fake's base URL.
    #[cfg(test)]
    fake_base: Mutex<Option<String>>,
}

impl Firewalla {
    pub fn new(labels_path: PathBuf, msp_path: PathBuf, token: impl Fn() -> Result<Option<String>, String> + Send + Sync + 'static, held: impl Fn() -> bool + Send + Sync + 'static) -> Arc<Firewalla> {
        Arc::new(Firewalla {
            labels: Mutex::new(util::load_json_or_default(&labels_path)),
            saved: Mutex::new(util::load_json_or_default(&msp_path)),
            labels_path,
            msp_path,
            token: Box::new(token),
            held: Box::new(held),
            demo_labels: Mutex::new(demo::sample_labels()),
            problem: Mutex::new(None),
            asking: tokio::sync::Mutex::new(()),
            generation: AtomicU64::new(0),
            #[cfg(test)]
            fake_base: Mutex::new(None),
        })
    }

    fn client(&self, domain: &str, token: &str) -> Result<Client, String> {
        #[cfg(test)]
        if let Some(base) = self.fake_base.lock().unwrap().as_deref() {
            return Client::plain(base, token);
        }
        Client::new(domain, token)
    }

    pub fn labels_of(&self, switch_id: &str, live: bool) -> BTreeMap<u32, PortLabel> {
        let all = if live { &self.labels } else { &self.demo_labels };
        all.lock().unwrap().get(switch_id).cloned().unwrap_or_default()
    }

    /// Sets or clears (None) one port's label, and returns what's saved. In Demo it changes the
    /// samples in memory; with Live data it's firewalla_ports.json.
    pub fn set_label(&self, switch_id: &str, port: u32, label: Option<&PortLabel>, nodes: &[String], now_ms: u64, live: bool) -> Result<Option<PortLabel>, String> {
        if port == 0 || port > PORTS {
            return Err(format!("The Switch X has no port {port}."));
        }
        let clean = label.map(|l| labels::clean(l, nodes, now_ms)).transpose()?;
        let mut all = if live { self.labels.lock().unwrap() } else { self.demo_labels.lock().unwrap() };
        let ports = all.entry(switch_id.to_string()).or_default();
        match &clean {
            Some(l) => ports.insert(port, l.clone()),
            None => ports.remove(&port),
        };
        if live {
            util::save_json(&self.labels_path, &*all)?;
        }
        Ok(clean)
    }

    fn save(&self, saved: &Saved) {
        // A failed save costs only the memory of the schedule across a restart.
        let _ = util::save_json(&self.msp_path, saved);
    }

    pub fn domain(&self) -> Option<String> {
        self.saved.lock().unwrap().domain.clone()
    }

    /// Saves the MSP domain (empty clears it). A change of domain forgets what the old one said,
    /// but never a backoff: the quota doesn't care what the domain is called.
    pub fn set_domain(&self, text: &str, now_ms: u64) -> Result<Option<String>, String> {
        let domain = Some(text.trim()).filter(|t| !t.is_empty()).map(msp::check_domain).transpose()?;
        let mut s = self.saved.lock().unwrap();
        if s.domain != domain {
            self.generation.fetch_add(1, Ordering::SeqCst);
            s.domain = domain.clone();
            forget(&mut s);
            may_ask_soon(&mut s, now_ms);
            *self.problem.lock().unwrap() = None;
            self.save(&s);
        }
        Ok(domain)
    }

    /// The token was saved (`present`) or removed.
    pub fn token_changed(&self, present: bool, now_ms: u64) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut s = self.saved.lock().unwrap();
        if !present {
            forget(&mut s);
        }
        may_ask_soon(&mut s, now_ms);
        *self.problem.lock().unwrap() = None;
        self.save(&s);
    }

    pub fn status(&self, now_ms: u64) -> MspStatus {
        let s = self.saved.lock().unwrap();
        if s.domain.is_none() {
            return MspStatus { state: "off".into(), ..Default::default() };
        }
        let problem = self.problem.lock().unwrap().clone();
        let tried = (s.schedule.last_try_ms > 0).then_some(s.schedule.last_try_ms);
        let (state, message) = if s.schedule.limited(now_ms) {
            let at = crate::health::maintenance::local_hhmm(s.schedule.next_ms);
            ("rate-limited".to_string(), Some(format!("{} SparkBox tries again at {at}.", MspError::RateLimited { retry_after_s: None }.explain())))
        } else if let Some((state, message)) = problem {
            (state, Some(message))
        } else if s.last_ok_ms.is_some() {
            ("ok".to_string(), None)
        } else {
            ("waiting".to_string(), None)
        };
        MspStatus {
            configured: true,
            domain: s.domain.clone(),
            state,
            message,
            last_ok_ms: s.last_ok_ms,
            last_try_ms: tried,
            next_poll_ms: Some(s.schedule.next_ms.max(now_ms)),
            sw: s.last.clone(),
            held: false,
        }
    }

    /// What the panel shows of the cloud: samples in Demo, and whether look-only is holding the poll.
    pub fn status_for(&self, live: bool, look_only: bool, now_ms: u64) -> MspStatus {
        let mut msp = if live { self.status(now_ms) } else { demo::sample_status(now_ms) };
        msp.held = live && msp.configured && look_only;
        msp
    }

    /// One turn of the poll loop. Held with Demo data and in look-only mode, as Test connection
    /// is held there: no call that carries the token leaves. (The CRS804's poll in
    /// `routeros::start` differs: it runs in look-only, because its login stays on the LAN.)
    pub async fn tick(&self, live: bool, look_only: bool, want: Option<Wanted>, now_ms: u64) {
        if !live || look_only || self.domain().is_none() {
            return;
        }
        if let Some(want) = want {
            self.poll_if_due(&want, now_ms).await;
        }
    }

    /// One poll, if a domain is saved and the schedule says so.
    pub async fn poll_if_due(&self, want: &Wanted, now_ms: u64) {
        let Ok(_one) = self.asking.try_lock() else { return };
        let domain = {
            let s = self.saved.lock().unwrap();
            if !s.schedule.due(now_ms) {
                return;
            }
            (s.domain.clone(), self.generation.load(Ordering::SeqCst))
        };
        if let (Some(domain), generation) = domain {
            let _ = self.round(&domain, generation, want, now_ms).await;
        }
    }

    /// Test connection as the window asks for it: with Demo data nothing is sent.
    pub async fn test_connection(&self, live: bool, want: Option<Wanted>, now_ms: u64) -> MspTestResult {
        if !live {
            return MspTestResult { ok: true, kind: None, message: "Demo data: nothing was sent to Firewalla. Switch to Live data to test the real one.".into() };
        }
        let Some(want) = want else {
            return MspTestResult::refused("not-configured", "Give the Firewalla Switch X an address first. SparkBox finds it in Firewalla's answer by its address, or by the MAC your nodes hear.");
        };
        self.test(&want, now_ms).await
    }

    /// Test connection: one round now, unless a backoff or the last few minutes say otherwise.
    pub async fn test(&self, want: &Wanted, now_ms: u64) -> MspTestResult {
        let _one = self.asking.lock().await;
        let (domain, schedule, generation) = {
            let s = self.saved.lock().unwrap();
            (s.domain.clone(), s.schedule.clone(), self.generation.load(Ordering::SeqCst))
        };
        let Some(domain) = domain else { return MspTestResult::refused("not-configured", "Save the MSP domain first.") };
        if schedule.limited(now_ms) {
            return MspTestResult::refused("rate-limited", format!("Not tested: {} SparkBox tries again at {}.", MspError::RateLimited { retry_after_s: None }.explain(), crate::health::maintenance::local_hhmm(schedule.next_ms)));
        }
        if now_ms < schedule.last_try_ms + msp::TEST_EVERY_MS {
            return MspTestResult::refused("wait", "Not tested: SparkBox asked Firewalla in the last 5 minutes. Firewalla limits how often it can ask, so try again after that.");
        }
        match self.round(&domain, generation, want, now_ms).await {
            Ok(sw) => {
                let up = sw.ports.iter().filter(|p| p.link_up == Some(true)).count();
                let name = sw.name.as_deref().unwrap_or("the switch");
                MspTestResult { ok: true, kind: None, message: format!("Connected. Firewalla reports {} ports on {name}, {up} with a link.", sw.ports.len()) }
            }
            Err((kind, message)) => MspTestResult::refused(&kind, message),
        }
    }

    /// Refuses the next request if look-only mode has come on since this round began.
    fn may_ask(&self) -> Result<(), MspError> {
        if (self.held)() {
            Err(MspError::Held)
        } else {
            Ok(())
        }
    }

    /// Finds the switch: on the box it was last on, else on each box the token sees.
    async fn read(&self, client: &Client, want: &Wanted) -> Result<(Gid, MspSwitch), MspError> {
        let known = self.saved.lock().unwrap().gid.clone().and_then(|g| Gid::new(&g).ok());
        if let Some(gid) = known {
            self.may_ask()?;
            match client.topology(&gid).await.and_then(|v| msp::parse_topology(&v, want)) {
                Ok(sw) => return Ok((gid, sw)),
                // The box may be gone from this account: look for it again.
                Err(MspError::NotJson | MspError::NotListed(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let mut last = MspError::NotListed("This token sees no Firewalla boxes.".into());
        self.may_ask()?;
        for gid in client.boxes().await?.into_iter().take(MAX_BOXES) {
            self.may_ask()?;
            match client.topology(&gid).await.and_then(|v| msp::parse_topology(&v, want)) {
                Ok(sw) => return Ok((gid, sw)),
                Err(e @ MspError::NotListed(_)) => last = e,
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// One round of requests, and what it does to the schedule. Returns (state, sentence) on failure.
    async fn round(&self, domain: &str, generation: u64, want: &Wanted, now_ms: u64) -> Result<MspSwitch, (String, String)> {
        let fail = |state: &str, message: String| {
            *self.problem.lock().unwrap() = Some((state.to_string(), message.clone()));
            Err((state.to_string(), message))
        };
        let token = match (self.token)() {
            Ok(Some(t)) if !t.trim().is_empty() => t,
            Ok(_) => {
                self.finish(|s| s.schedule.failed(now_ms));
                return fail("no-token", "Save the MSP token in Settings › Switches & gateway.".into());
            }
            Err(e) => {
                self.finish(|s| s.schedule.failed(now_ms));
                return fail("keychain", format!("SparkBox couldn't read the MSP token from the Keychain: {e}"));
            }
        };
        let client = match self.client(domain, &token) {
            Ok(c) => c,
            Err(e) => {
                self.finish(|s| s.schedule.failed(now_ms));
                return fail("unreachable", e);
            }
        };
        // Saved before anything leaves: a process killed mid-request can't ask again on restart.
        self.finish(|s| s.schedule.asking(now_ms));
        let answer = self.read(&client, want).await;
        if self.generation.load(Ordering::SeqCst) != generation {
            // The domain or token changed meanwhile, so this answers a question nobody's asking.
            // A quota signal still counts: the limit doesn't care what changed.
            if let Err(MspError::RateLimited { retry_after_s }) = answer {
                self.finish(|s| s.schedule.rate_limited(now_ms, retry_after_s));
            }
            return Err(("changed".into(), "The Firewalla settings changed while SparkBox was asking, so it set that answer aside.".into()));
        }
        if let Err(MspError::Held) = answer {
            return Err((MspError::Held.kind().into(), MspError::Held.explain()));
        }
        match answer {
            Ok((gid, sw)) => {
                *self.problem.lock().unwrap() = None;
                let kept = sw.clone();
                self.finish(move |s| {
                    s.gid = Some(gid.as_str().to_string());
                    s.last = Some(kept);
                    s.last_ok_ms = Some(now_ms);
                    s.schedule.answered(now_ms);
                });
                Ok(sw)
            }
            Err(e) => {
                let retry = if let MspError::RateLimited { retry_after_s } = e { Some(retry_after_s) } else { None };
                self.finish(|s| {
                    // These say the last answer may not be this switch's: it's gone, not hidden,
                    // so no restart, 429 or new token can bring it back.
                    if matches!(e, MspError::NotListed(_) | MspError::Shape(_) | MspError::NotJson) {
                        s.last = None;
                        s.last_ok_ms = None;
                    }
                    match (&e, retry) {
                        (_, Some(after)) => s.schedule.rate_limited(now_ms, after),
                        (MspError::Auth, _) => s.schedule.refused(now_ms),
                        (MspError::Timeout | MspError::Unreachable(_) | MspError::Http(_), _) => s.schedule.failed(now_ms),
                        // It answered, and a 200 says the quota is fine.
                        _ => s.schedule.answered(now_ms),
                    }
                });
                fail(e.kind(), e.explain())
            }
        }
    }

    fn finish(&self, change: impl FnOnce(&mut Saved)) {
        let mut s = self.saved.lock().unwrap();
        change(&mut s);
        self.save(&s);
    }
}

/// Forgets what the cloud said.
fn forget(s: &mut Saved) {
    s.gid = None;
    s.last = None;
    s.last_ok_ms = None;
}

/// A new domain or token gets its own try 5 minutes after the last one, except during a backoff.
fn may_ask_soon(s: &mut Saved, now_ms: u64) {
    if !s.schedule.limited(now_ms) {
        s.schedule.next_ms = s.schedule.next_ms.min(s.schedule.last_try_ms + msp::TEST_EVERY_MS);
    }
}

/// The system name the Switch X sends in LLDP ("Firewalla-Switch-X"). Not any Firewalla switch:
/// another one on the LAN is a different switch.
pub fn is_firewalla_switch(system: &str) -> bool {
    system.chars().filter(char::is_ascii_alphanumeric).collect::<String>().eq_ignore_ascii_case("firewallaswitchx")
}

/// The chassis ids the nodes hear the Switch X send.
fn heard_macs(nodes: &[NodeSnapshot]) -> Vec<String> {
    let peers = nodes.iter().flat_map(|n| &n.nets).flat_map(|x| &x.lldp_peers);
    let mut macs: Vec<String> = peers.filter(|p| p.system.as_deref().is_some_and(is_firewalla_switch)).filter_map(|p| p.chassis_id.clone()).collect();
    macs.sort();
    macs.dedup();
    macs
}

/// How to know the Switch X in the cloud's answer: its address, and the chassis LLDP hears.
fn wanted(state: &crate::AppState) -> Option<Wanted> {
    let inventory = state.inventory.lock().unwrap();
    let host = inventory.switches.iter().find(|s| s.kind == SwitchKind::Firewalla).map(|s| s.host.trim().to_string()).unwrap_or_default();
    drop(inventory);
    let macs = state.latest.read().unwrap().as_ref().map(|s| heard_macs(&s.nodes)).unwrap_or_default();
    (!host.is_empty() || !macs.is_empty()).then_some(Wanted { host, macs })
}

/// Asks the cloud when the schedule says so: only with Live data, outside look-only mode, and
/// only once a domain is saved (`Firewalla::tick`).
pub fn start(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(TICK);
        loop {
            every.tick().await;
            let state = handle.state::<crate::AppState>();
            let fw = handle.state::<Arc<Firewalla>>().inner().clone();
            let want = wanted(&state);
            fw.tick(crate::is_live(&state), crate::admin::look_only(), want, commands::now_ms()).await;
        }
    });
}
