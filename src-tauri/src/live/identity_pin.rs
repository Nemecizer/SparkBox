//! Which machine answers as each node (P1.8). cell1, cell2 and cell3 present one cloned host
//! key and SparkBox connects by address, so the key alone doesn't say which Spark answered.
//! Each node is pinned at its first good frame after its key is trusted: hostname, LAN MAC and
//! /etc/machine-id, kept in node_pins.json in the config folder (not inventory.json, which
//! Settings rewrites). A machine-id two nodes share (a cloned image) isn't pinned: those nodes
//! are pinned by hostname and LAN MAC only, and node_identity says so.
//!
//! Before every change the exit compares what the node says now with its pin. A mismatch
//! refuses the change ("cell2 answers as cell3-example"); reads keep working. Re-trusting a
//! changed host key clears the pin, and Settings › Fence re-pins a node that was moved or
//! rebuilt (with a fresh Touch ID).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{wire, Live};
use crate::errors::{ErrorKind, SbError};
use crate::model::NetRole;

pub const FILE: &str = "node_pins.json";
/// Read when a node has no frame yet: its name, machine-id and the LAN port's MAC.
const FACTS: &str = "hostname; cat /etc/machine-id; cat /sys/class/net/$(ip -o route get 1.1.1.1 | awk '{print $5}')/address";

/// What a node was when it was pinned.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pin {
    pub hostname: String,
    pub lan_mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// Its machine-id is shared with another node, so it isn't pinned.
    pub cloned_machine_id: bool,
    pub host_key: Option<String>,
    pub pinned_ms: u64,
}

/// What a node says it is now.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Facts {
    pub hostname: String,
    pub lan_mac: Option<String>,
    pub machine_id: Option<String>,
}

/// One node in Settings › Fence: its pin, what it says now, and whether they agree.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityView {
    pub node: String,
    pub name: String,
    pub pin: Option<Pin>,
    pub now: Option<Facts>,
    /// None while there's nothing to compare.
    pub matches: Option<bool>,
    /// "machine-id is shared by cell1, cell2 and cell3; pinned hostname and LAN MAC"
    pub note: Option<String>,
}

/// The pins, and what this session has read from each node.
#[derive(Default)]
pub struct Pins {
    file: Mutex<()>,
    /// /etc/machine-id by node, for the connection it was read on (None: unreadable).
    machine_ids: Mutex<HashMap<String, (ReadOn, Option<String>)>>,
    /// The no-frame read, for the connection it was made on.
    probed: Mutex<HashMap<String, (ReadOn, Facts)>>,
}

pub fn load(config: &Path) -> BTreeMap<String, Pin> {
    std::fs::read(config.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The node's LAN MAC in a frame: the LAN port carrying `address`, else the first LAN port.
fn lan_mac(frame: &super::Frame, address: &str) -> Option<String> {
    let lan = || frame.nets.iter().filter(|n| n.role == NetRole::Lan && n.mac.is_some());
    lan().find(|n| n.ip.as_deref() == Some(address)).or_else(|| lan().next()).and_then(|n| n.mac.clone()).map(|m| m.to_lowercase())
}

/// Which connection a read was made on: its serial, never its address, which a later
/// connection can reuse once the old one is gone.
type ReadOn = u64;

fn session_key(s: &Arc<super::ssh::Session>) -> ReadOn {
    s.serial()
}

fn same_session(key: &ReadOn, s: &Arc<super::ssh::Session>) -> bool {
    *key == session_key(s)
}

/// "cell1, cell2 and cell3"
fn names(list: &[String]) -> String {
    match list {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

impl Pins {
    fn save(&self, config: &Path, f: impl FnOnce(&mut BTreeMap<String, Pin>)) -> Result<(), String> {
        let _one = self.file.lock().unwrap();
        let mut pins = load(config);
        f(&mut pins);
        crate::util::save_json(&config.join(FILE), &pins)
    }

    /// /etc/machine-id, read once per connection (world-readable; not in the activity log).
    async fn machine_id(&self, live: &Live, node: &str) -> Option<String> {
        let session = live.session(node)?;
        if let Some((k, id)) = self.machine_ids.lock().unwrap().get(node) {
            if same_session(k, &session) {
                return id.clone();
            }
        }
        let out = session.exec(&wire("cat /etc/machine-id"), None, Duration::from_secs(10)).await.ok()?;
        let id = Some(out.stdout.trim().to_string()).filter(|s| out.code == Some(0) && s.len() >= 16 && s.chars().all(|c| c.is_ascii_hexdigit()));
        self.machine_ids.lock().unwrap().insert(node.to_string(), (session_key(&session), id.clone()));
        id
    }

    /// The connection `node`'s machine-id was last read on (tests).
    #[cfg(test)]
    pub fn machine_id_read_on(&self, node: &str) -> Option<ReadOn> {
        self.machine_ids.lock().unwrap().get(node).map(|(k, _)| *k)
    }

    /// What `node` says it is now: from its latest frame, else one read per connection.
    async fn facts(&self, live: &Live, node: &str) -> Option<Facts> {
        let address = live.configs.lock().unwrap().get(node).map(super::address).unwrap_or_default();
        let from_frame = live.nodes.lock().unwrap().get(node).and_then(|s| s.frame.as_ref().map(|f| (f.system.hostname.clone(), lan_mac(f, &address))));
        let machine_id = self.machine_id(live, node).await;
        if let Some((hostname, lan_mac)) = from_frame.filter(|(h, _)| !h.is_empty()) {
            return Some(Facts { hostname, lan_mac, machine_id });
        }
        let session = live.session(node)?;
        if let Some((k, f)) = self.probed.lock().unwrap().get(node) {
            if same_session(k, &session) {
                return Some(f.clone());
            }
        }
        let out = session.exec(&wire(FACTS), None, Duration::from_secs(10)).await.ok()?;
        let mut lines = out.stdout.lines().map(str::trim);
        let hostname = lines.next().unwrap_or("").to_string();
        let id = lines.next().filter(|l| l.len() >= 16 && l.chars().all(|c| c.is_ascii_hexdigit())).map(String::from);
        let mac = lines.next().filter(|l| l.len() == 17 && l.contains(':')).map(str::to_lowercase);
        let facts = Facts { hostname, lan_mac: mac, machine_id: machine_id.or(id) };
        if facts.hostname.is_empty() {
            return None;
        }
        self.probed.lock().unwrap().insert(node.to_string(), (session_key(&session), facts.clone()));
        Some(facts)
    }

    /// The other nodes whose machine-id (read this session, or pinned) is `id`.
    fn sharing(&self, config: &Path, node: &str, id: &str) -> Vec<String> {
        let mut out: Vec<String> = self.machine_ids.lock().unwrap().iter().filter(|(n, (_, m))| *n != node && m.as_deref() == Some(id)).map(|(n, _)| n.clone()).collect();
        out.extend(load(config).into_iter().filter(|(n, p)| n != node && p.machine_id.as_deref() == Some(id)).map(|(n, _)| n));
        out.sort();
        out.dedup();
        out
    }

    /// Pins every connected node that has a frame and no pin yet, once every connected node's
    /// machine-id has been read (so a shared one is seen before anything is pinned). A pinned
    /// machine-id that turns out to be shared is dropped from every pin holding it.
    pub async fn pin_new(&self, live: &Live, config: &Path, now_ms: u64) {
        let connected: Vec<String> = live.nodes.lock().unwrap().iter().filter(|(_, s)| s.frame.is_some() && s.session.as_ref().is_some_and(|x| !x.is_closed())).map(|(n, _)| n.clone()).collect();
        let mut ids = HashMap::new();
        for n in &connected {
            ids.insert(n.clone(), self.machine_id(live, n).await);
        }
        let pins = load(config);
        let mut count: HashMap<String, usize> = HashMap::new();
        for id in ids.values().flatten().chain(pins.iter().filter(|(n, _)| !ids.contains_key(*n)).filter_map(|(_, p)| p.machine_id.as_ref())) {
            *count.entry(id.clone()).or_default() += 1;
        }
        let shared = |id: &Option<String>| id.as_ref().is_some_and(|i| count.get(i).copied().unwrap_or(0) > 1);
        let mut new = Vec::new();
        for n in connected.iter().filter(|n| !pins.contains_key(*n)) {
            if let Some(f) = self.facts(live, n).await {
                let host_key = live.nodes.lock().unwrap().get(n).and_then(|s| s.host_key.as_ref().map(|k| k.fingerprint.clone()));
                let cloned = shared(&f.machine_id);
                new.push((n.clone(), Pin { hostname: f.hostname, lan_mac: f.lan_mac, machine_id: f.machine_id.filter(|_| !cloned), cloned_machine_id: cloned, host_key, pinned_ms: now_ms }));
            }
        }
        let unshare: Vec<String> = pins.iter().filter(|(_, p)| shared(&p.machine_id)).map(|(n, _)| n.clone()).collect();
        if new.is_empty() && unshare.is_empty() {
            return;
        }
        let _ = self.save(config, |all| {
            for (n, p) in new {
                all.entry(n).or_insert(p);
            }
            for n in unshare {
                if let Some(p) = all.get_mut(&n) {
                    p.machine_id = None;
                    p.cloned_machine_id = true;
                }
            }
        });
    }

    /// Before a change on `node`: does the machine answering match its pin? A node with no pin
    /// yet is pinned now, from what it says (the first connection after trust).
    pub async fn check(&self, live: &Live, config: &Path, node: &str, now_ms: u64) -> Result<(), SbError> {
        if !load(config).contains_key(node) {
            self.pin_new(live, config, now_ms).await;
        }
        let Some(pin) = load(config).remove(node) else {
            return Err(SbError::new(ErrorKind::WrongNode, format!("SparkBox hasn't pinned which machine {node} is yet, so changes are refused. Wait for its first data, or re-pin it in Settings › Fence.")).on(node));
        };
        let Some(now) = self.facts(live, node).await else {
            return Err(SbError::new(ErrorKind::WrongNode, format!("{node} didn't say which machine it is, so changes are refused.")).on(node));
        };
        let name = live.configs.lock().unwrap().get(node).map(|c| c.name.clone()).unwrap_or_else(|| node.to_string());
        let wrong = now.hostname != pin.hostname
            || matches!((&pin.lan_mac, &now.lan_mac), (Some(a), Some(b)) if a != b)
            || matches!((&pin.machine_id, &now.machine_id), (Some(a), Some(b)) if a != b);
        if wrong {
            return Err(SbError::new(ErrorKind::WrongNode, format!("{name} answers as {}: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence.", now.hostname)).on(node));
        }
        Ok(())
    }

    /// Forgets `node`'s pin (its host key was replaced); it's pinned again at its next connection.
    pub fn clear(&self, config: &Path, node: &str) -> Result<(), String> {
        self.probed.lock().unwrap().remove(node);
        self.save(config, |all| {
            all.remove(node);
        })
    }

    /// Pins `node` again from what it says now (Settings › Fence › Re-pin identity).
    pub async fn repin(&self, live: &Live, config: &Path, node: &str, now_ms: u64) -> Result<Pin, String> {
        self.probed.lock().unwrap().remove(node);
        let f = self.facts(live, node).await.ok_or_else(|| format!("{node} isn't connected, or didn't say which machine it is."))?;
        let cloned = f.machine_id.as_deref().is_some_and(|id| !self.sharing(config, node, id).is_empty());
        let host_key = live.nodes.lock().unwrap().get(node).and_then(|s| s.host_key.as_ref().map(|k| k.fingerprint.clone()));
        let pin = Pin { hostname: f.hostname, lan_mac: f.lan_mac, machine_id: f.machine_id.filter(|_| !cloned), cloned_machine_id: cloned, host_key, pinned_ms: now_ms };
        self.save(config, |all| {
            all.insert(node.to_string(), pin.clone());
        })?;
        Ok(pin)
    }

    /// Every node's pin and what it says now, for Settings › Fence.
    pub async fn view(&self, live: &Live, config: &Path, nodes: &[(String, String)]) -> Vec<IdentityView> {
        let pins = load(config);
        let mut out = Vec::new();
        for (id, name) in nodes {
            let pin = pins.get(id).cloned();
            let now = if live.is_connected(id) { self.facts(live, id).await } else { None };
            let matches = match (&pin, &now) {
                (Some(p), Some(n)) => Some(n.hostname == p.hostname && !matches!((&p.lan_mac, &n.lan_mac), (Some(a), Some(b)) if a != b) && !matches!((&p.machine_id, &n.machine_id), (Some(a), Some(b)) if a != b)),
                _ => None,
            };
            let note = pin.as_ref().filter(|p| p.cloned_machine_id).map(|_| {
                let id_now = now.as_ref().and_then(|n| n.machine_id.clone());
                let mut with: Vec<String> = id_now.map(|m| self.sharing(config, id, &m)).unwrap_or_default();
                with.push(id.clone());
                with.sort();
                let shown: Vec<String> = with.iter().map(|w| nodes.iter().find(|(i, _)| i == w).map(|(_, n)| n.clone()).unwrap_or_else(|| w.clone())).collect();
                if shown.len() > 1 {
                    format!("machine-id is shared by {}; pinned hostname and LAN MAC", names(&shown))
                } else {
                    "machine-id is shared with another node; pinned hostname and LAN MAC".to_string()
                }
            });
            out.push(IdentityView { node: id.clone(), name: name.clone(), pin, now, matches, note });
        }
        out
    }
}
