//! Owner expectations are comparison data only. Launches still use fresh node checks.
use crate::model::{LinkStatus, NetRole, NodeSnapshot};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub iface: String,
    pub ip: String,
    #[serde(default)]
    pub rdma: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Expectations {
    #[serde(default)]
    pub links: Vec<Link>,
    /// None: no preference; false: worker preferred. Advisory, never a load permission.
    #[serde(default)]
    pub head_candidate: Option<bool>,
}

fn name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

impl Expectations {
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for l in &self.links {
            if !name(&l.iface) {
                return Err("Fabric interface names need 1–64 letters, digits, dots, underscores or hyphens.".into());
            }
            if !seen.insert(&l.iface) {
                return Err(format!("Fabric interface {} is listed twice.", l.iface));
            }
            if l.ip.parse::<std::net::Ipv4Addr>().is_err() {
                return Err(format!("{} needs a valid fabric IPv4 address.", l.iface));
            }
            if !l.rdma.is_empty() && !name(&l.rdma) {
                return Err("RDMA device names need 1–64 letters, digits, dots, underscores or hyphens, or can be left blank.".into());
            }
        }
        Ok(())
    }

    pub fn differences(&self, reported: Option<&[Link]>) -> Vec<String> {
        if self.links.is_empty() {
            return vec![];
        }
        let Some(reported) = reported else { return vec!["Fresh fabric readings are unavailable; saved expectations have not been checked.".into()] };
        let mut out = vec![];
        for expected in &self.links {
            let Some(actual) = reported.iter().find(|l| l.iface == expected.iface) else {
                out.push(format!("{} is not reported as a fabric interface.", expected.iface));
                continue;
            };
            if expected.ip != actual.ip {
                out.push(format!(
                    "{}: expected IPv4 {}, reported {}.",
                    expected.iface,
                    expected.ip,
                    if actual.ip.is_empty() { "no address" } else { &actual.ip }
                ));
            }
            if !expected.rdma.is_empty() && expected.rdma != actual.rdma {
                out.push(format!(
                    "{}: expected RDMA device {}, reported {}.",
                    expected.iface,
                    expected.rdma,
                    if actual.rdma.is_empty() { "no device" } else { &actual.rdma }
                ));
            }
        }
        for actual in reported.iter().filter(|l| !l.ip.is_empty() && !self.links.iter().any(|e| e.iface == l.iface)) {
            out.push(format!("{} is reported on the fabric but is not in the saved expectations.", actual.iface));
        }
        out
    }
}

/// None is unknown; an empty list is a fresh report with no fabric interfaces.
pub fn reported(node: &NodeSnapshot) -> Option<Vec<Link>> {
    (node.status == LinkStatus::Online && !node.stale).then(|| {
        node.nets
            .iter()
            .filter(|n| n.role == NetRole::Fabric)
            .map(|n| Link { iface: n.iface.clone(), ip: n.ip.clone().unwrap_or_default(), rdma: n.rdma_dev.clone().unwrap_or_default() })
            .collect()
    })
}

pub fn planning_warnings(inventory: &super::Inventory, snapshots: &[NodeSnapshot], selected: &[String], fabric_listen: bool) -> Vec<String> {
    if selected.len() < 2 && !fabric_listen {
        return vec![];
    }
    let mut out = vec![];
    for (index, id) in selected.iter().enumerate() {
        let Some(node) = inventory.nodes.iter().find(|n| &n.id == id) else {
            continue;
        };
        let Some(expected) = &node.fabric else {
            continue;
        };
        let observed = snapshots.iter().find(|n| &n.id == id).and_then(reported);
        out.extend(expected.differences(observed.as_deref()).into_iter().map(|w| format!("{}: {w}", node.name)));
        if selected.len() > 1 && index == 0 && expected.head_candidate == Some(false) {
            out.push(format!("{} is selected as the head, but its saved role is Worker preferred. Review the node order or role preference.", node.name));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> Link {
        Link { iface: "enp1s0f1np1".into(), ip: "192.168.100.51".into(), rdma: "rocep1s0f1".into() }
    }
    fn expected() -> Expectations {
        Expectations { links: vec![link()], head_candidate: Some(false) }
    }

    #[test]
    fn shared_validation_cases() {
        let cases: serde_json::Value = serde_json::from_str(include_str!("fixtures/validation.json")).unwrap();
        for c in cases.as_array().unwrap() {
            let f: Expectations = serde_json::from_value(c["fabric"].clone()).unwrap();
            assert_eq!(f.validate().is_ok(), c["valid"].as_bool().unwrap(), "{}", c["name"]);
        }
    }

    #[test]
    fn differences_cover_unknown_absent_changed_and_extra_facts() {
        let f = expected();
        assert!(f.differences(Some(&[link()])).is_empty());
        assert!(f.differences(None)[0].contains("unavailable"));
        assert!(f.differences(Some(&[]))[0].contains("not reported"));
        let changed = Link { ip: "192.168.100.99".into(), rdma: "other".into(), ..link() };
        let warnings = f.differences(Some(&[changed]));
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("expected IPv4 192.168.100.51, reported 192.168.100.99"));
        assert!(warnings[1].contains("expected RDMA device rocep1s0f1, reported other"));
        let extra = Link { iface: "enP2p1s0f1np1".into(), ..link() };
        assert!(f.differences(Some(&[link(), extra]))[0].contains("not in the saved"));
        let spare = Link { iface: "spare".into(), ip: "".into(), rdma: "unused".into() };
        assert!(f.differences(Some(&[link(), spare])).is_empty());
        let optional_rdma = Expectations { links: vec![Link { rdma: "".into(), ..link() }], ..f };
        assert!(optional_rdma.differences(Some(&[link()])).is_empty());
        assert!(Expectations::default().differences(None).is_empty());
    }

    #[test]
    fn legacy_inventory_round_trips_and_metadata_never_reconnects() {
        let mut inventory = super::super::Inventory::example();
        let old: super::super::Inventory = serde_json::from_value(serde_json::to_value(&inventory).unwrap()).unwrap();
        assert!(old.nodes.iter().all(|n| n.fabric.is_none()));
        inventory.nodes[0].fabric = Some(expected());
        assert!(!inventory.requires_reconnect(&old));
        let dir = crate::fake_node::temp("inventory-fabric");
        let path = dir.join("inventory.json");
        inventory.save(&path).unwrap();
        assert_eq!(super::super::Inventory::load(&path), inventory);
        let before = std::fs::read(&path).unwrap();
        inventory.nodes[0].fabric.as_mut().unwrap().links[0].ip = "invalid".into();
        assert!(inventory.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before, "invalid metadata preserves the saved file");
        inventory.nodes[0].host = "other.local".into();
        assert!(inventory.requires_reconnect(&old));
        inventory = old.clone();
        inventory.nodes.swap(0, 1);
        assert!(inventory.requires_reconnect(&old));
    }

    #[test]
    fn planning_warns_only_for_fabric_loads_and_never_trusts_stale_facts() {
        let mut inventory = super::super::Inventory::example();
        let mut snap = crate::demo::DemoCluster::new(&inventory).snapshot();
        let first = inventory.nodes[0].id.clone();
        let second = inventory.nodes[1].id.clone();
        let node = snap.nodes.iter_mut().find(|n| n.id == first).unwrap();
        inventory.nodes[0].fabric = Some(Expectations { links: reported(node).unwrap(), head_candidate: Some(false) });
        let ids = vec![first.clone(), second];
        let warnings = planning_warnings(&inventory, &snap.nodes, &ids, false);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Worker preferred"));
        assert!(planning_warnings(&inventory, &snap.nodes, &[first.clone()], false).is_empty());
        assert!(planning_warnings(&inventory, &snap.nodes, &[first.clone()], true).is_empty());
        snap.nodes.iter_mut().find(|n| n.id == first).unwrap().stale = true;
        assert!(planning_warnings(&inventory, &snap.nodes, &[first.clone()], true)[0].contains("unavailable"));
        let node = snap.nodes.iter_mut().find(|n| n.id == first).unwrap();
        node.stale = false;
        node.status = LinkStatus::Offline;
        assert!(reported(node).is_none());
        assert!(planning_warnings(&inventory, &[], &ids, false)[0].contains("unavailable"));
        inventory.nodes[0].fabric.as_mut().unwrap().head_candidate = Some(true);
        assert!(!planning_warnings(&inventory, &[], &ids, false).iter().any(|w| w.contains("Worker preferred")));
    }
}
