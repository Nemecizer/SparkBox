//! The list of machines SparkBox manages, saved as JSON in the app's config
//! directory (~/Library/Application Support/com.nemecj.sparkbox/inventory.json).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::util;

pub mod fabric;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SwitchKind {
    /// MikroTik (the CRS804 fabric switch), read and controlled over the RouterOS REST API.
    #[default]
    RouterOs,
    /// The Firewalla Switch X: changed only in the Firewalla app, so SparkBox probes it and reads
    /// its ports from LLDP, the owner's labels and Firewalla's cloud (firewalla/).
    Firewalla,
    /// The Firewalla Gold router and VPN endpoint, probed like the Switch X.
    Gateway,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeConfig {
    pub id: String,
    pub name: String,
    /// mDNS hostname, used only when `ip` is empty.
    pub host: String,
    /// The address SparkBox connects to, first.
    pub ip: Option<String>,
    pub model: String,
    pub port: u16,
    pub user: String,
    /// Owner-confirmed expectations, never a source of launch addresses or network changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fabric: Option<fabric::Expectations>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchConfig {
    pub id: String,
    pub name: String,
    pub kind: SwitchKind,
    /// Empty until set in Settings.
    pub host: String,
    pub user: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub nodes: Vec<NodeConfig>,
    pub switches: Vec<SwitchConfig>,
}

fn node(id: &str, host: &str, ip: &str, model: &str) -> NodeConfig {
    NodeConfig {
        id: id.into(),
        name: id.into(),
        host: host.into(),
        ip: Some(ip.into()),
        model: model.into(),
        port: 22,
        user: "sparkuser".into(),
        fabric: None,
    }
}

impl Inventory {
    /// Synthetic topology for offline tests; never a first-run configuration.
    pub fn example() -> Self {
        Inventory {
            nodes: vec![
                node("cell1", "cell1-example.local", "192.168.50.51", "MSI EdgeXpert"),
                node("cell2", "cell2-example.local", "192.168.50.154", "MSI EdgeXpert"),
                node("cell3", "cell3-example.local", "192.168.50.98", "MSI EdgeXpert"),
                node("gx10-0000", "gx10-0000.local", "192.168.50.181", "ASUS Ascent GX10"),
            ],
            switches: vec![
                SwitchConfig {
                    id: "crs804".into(),
                    name: "CRS804 DDQ".into(),
                    kind: SwitchKind::RouterOs,
                    host: String::new(),
                    user: Some("sparkbox".into()),
                },
                SwitchConfig {
                    id: "firewalla".into(),
                    name: "Firewalla Switch X".into(),
                    kind: SwitchKind::Firewalla,
                    host: String::new(),
                    user: None,
                },
                SwitchConfig {
                    id: "gold".into(),
                    name: "Firewalla Gold".into(),
                    kind: SwitchKind::Gateway,
                    host: String::new(),
                    user: None,
                },
            ],
        }
    }
}

impl Inventory {
    /// Loads inventory.json. One that can't be read is set aside as inventory.json.corrupt-<time>
    /// (reported through `persistence_warnings`) and the default inventory is used.
    pub fn load(path: &Path) -> Self {
        let mut inv: Inventory = util::load_json_or_default(path);
        // Inventories saved before the Gold was added get it, unconfigured.
        if !inv.nodes.is_empty() && !inv.switches.iter().any(|s| s.kind == SwitchKind::Gateway) {
            if let Some(gold) = Inventory::example().switches.into_iter().find(|s| s.kind == SwitchKind::Gateway) {
                inv.switches.push(gold);
            }
        }
        inv
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        for n in &self.nodes {
            if let Some(f) = &n.fabric {
                f.validate().map_err(|e| format!("{}: {e}", n.name))?;
            }
        }
        util::save_json(path, self)
    }

    /// Fabric expectations alone do not change connections or reset Demo's running loads.
    pub fn requires_reconnect(&self, old: &Self) -> bool {
        let (mut new, mut old) = (self.clone(), old.clone());
        for n in new.nodes.iter_mut().chain(old.nodes.iter_mut()) {
            n.fabric = None;
        }
        new != old
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventories_round_trip_gain_the_gold_and_survive_a_broken_file() {
        let dir = std::env::temp_dir().join(format!("sparkbox-inventory-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("inventory.json");
        assert_eq!(Inventory::load(&path), Inventory::default(), "missing configuration stays empty");
        assert!(Inventory::default().nodes.is_empty());
        assert!(Inventory::default().switches.is_empty());
        assert!(crate::demo::DemoCluster::new(&Inventory::default()).snapshot().nodes.is_empty());

        let mut old = Inventory::example();
        old.switches.retain(|s| s.kind != SwitchKind::Gateway);
        old.nodes[0].ip = Some("10.0.0.9".into());
        old.save(&path).unwrap();
        let back = Inventory::load(&path);
        assert_eq!(back.nodes[0].ip.as_deref(), Some("10.0.0.9"));
        let gold = back.switches.iter().find(|s| s.kind == SwitchKind::Gateway).unwrap();
        assert_eq!((gold.name.as_str(), gold.host.as_str()), ("Firewalla Gold", ""), "added, not set up");

        std::fs::write(&path, &std::fs::read_to_string(&path).unwrap()[..40]).unwrap();
        assert_eq!(Inventory::load(&path), Inventory::default(), "broken configuration never restores example endpoints");
        assert!(util::warning_for(&path).is_some(), "and a warning for the banner");
        assert!(std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("inventory.json.corrupt-")));
        util::clear_warning(&path);
    }
}
