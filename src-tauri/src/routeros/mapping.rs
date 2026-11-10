//! Which node is on which switch port, without trusting port comments: the
//! bridge host table says which MAC the switch learned behind each port, and
//! LLDP/MNDP neighbors say which host announced itself there. The nodes' own
//! fabric MACs (from the collector) close the loop. Comments are the fallback.
//!
//! Also finds the ports SparkBox must not cut: the one the switch's management
//! traffic (and so SparkBox's own connection) arrives on.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{NetRole, NodeSnapshot, SwitchPort};

/// One row of `/interface/bridge/host` or `/interface/ethernet/switch/host`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostEntry {
    /// Upper-case, colon-separated.
    pub mac: String,
    /// The port it was learned on ("on-interface"; for a switch-chip host, "ports").
    pub port: String,
    pub dynamic: bool,
    /// The switch's own MAC (the bridge itself).
    pub local: bool,
}

/// One row of `/ip/neighbor`: who announced itself on a port.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighbor {
    pub port: String,
    pub identity: String,
    pub mac: Option<String>,
    pub address: Option<String>,
    pub platform: Option<String>,
    /// lldp | mndp | cdp
    pub discovered_by: String,
}

/// What the switch knows about who is behind its ports.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortTables {
    pub hosts: Vec<HostEntry>,
    pub neighbors: Vec<Neighbor>,
    /// `/ip/address` rows: (address with prefix, interface).
    #[serde(default)]
    pub addresses: Vec<(String, String)>,
    /// `/ip/arp` rows: (address, MAC, interface).
    #[serde(default)]
    pub arp: Vec<(String, String, String)>,
}

/// "94:6d:ae:1a:2b:3c" and "94-6D-AE-1A-2B-3C" are the same MAC.
pub fn norm_mac(m: &str) -> String {
    m.trim().replace('-', ":").to_ascii_uppercase()
}

fn s(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn yes(v: &Value, k: &str) -> bool {
    matches!(s(v, k).as_deref(), Some("true" | "yes"))
}

/// `/interface/bridge/host`: mac-address, on-interface, dynamic, local.
pub fn parse_bridge_hosts(v: &Value) -> Vec<HostEntry> {
    v.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    Some(HostEntry { mac: norm_mac(&s(r, "mac-address")?), port: s(r, "on-interface")?, dynamic: yes(r, "dynamic"), local: yes(r, "local") })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `/interface/ethernet/switch/host` (the switch chip's own table): mac-address, ports.
pub fn parse_switch_hosts(v: &Value) -> Vec<HostEntry> {
    v.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let port = s(r, "ports").or_else(|| s(r, "port"))?;
                    let mac = norm_mac(&s(r, "mac-address")?);
                    // A host on several ports (a trunk) can't place a node.
                    (!port.contains(',')).then(|| HostEntry { mac, port, dynamic: yes(r, "dynamic"), local: false })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `/ip/neighbor`: interface, identity, mac-address, address, platform, discovered-by.
pub fn parse_neighbors(v: &Value) -> Vec<Neighbor> {
    v.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    // RouterOS reports a bridged neighbor as "qsfp56-dd-1-1,bridge"; the port comes first.
                    let port = s(r, "interface")?.split(',').next()?.trim().to_string();
                    Some(Neighbor {
                        port,
                        identity: s(r, "identity").unwrap_or_default(),
                        mac: s(r, "mac-address").map(|m| norm_mac(&m)),
                        address: s(r, "address").or_else(|| s(r, "address4")),
                        platform: s(r, "platform").or_else(|| s(r, "system-description")),
                        discovered_by: s(r, "discovered-by").unwrap_or_else(|| "mndp".into()),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `/ip/arp`: address, mac-address, interface (incomplete entries have no MAC and are skipped).
pub fn parse_arp(v: &Value) -> Vec<(String, String, String)> {
    v.as_array()
        .map(|rows| rows.iter().filter_map(|r| Some((s(r, "address")?, norm_mac(&s(r, "mac-address")?), s(r, "interface")?))).collect())
        .unwrap_or_default()
}

/// `/ip/address`: address ("192.168.50.2/24") and interface.
pub fn parse_addresses(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|rows| rows.iter().filter_map(|r| Some((s(r, "address")?, s(r, "interface")?))).collect())
        .unwrap_or_default()
}

/// A node's fabric interface as the switch sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub node: String,
    pub iface: String,
    pub port: String,
    /// mac | lldp | comment
    pub by: &'static str,
}

/// Every node fabric interface the tables place on a port: by MAC first, then by LLDP
/// identity (the node's hostname), then by a port comment naming the node.
pub fn locate(ports: &[SwitchPort], t: &PortTables, nodes: &[NodeSnapshot]) -> Vec<Seen> {
    let mut out = Vec::new();
    for n in nodes {
        let fabric: Vec<_> = n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.mac.is_some()).collect();
        let mut placed = false;
        for x in &fabric {
            let m = norm_mac(x.mac.as_deref().unwrap_or(""));
            if let Some(h) = t.hosts.iter().find(|h| !h.local && h.mac == m) {
                out.push(Seen { node: n.id.clone(), iface: x.iface.clone(), port: h.port.clone(), by: "mac" });
                placed = true;
            }
        }
        if placed {
            continue;
        }
        let names = [n.id.to_ascii_lowercase(), n.name.to_ascii_lowercase(), n.system.hostname.to_ascii_lowercase()];
        let mine = |identity: &str| {
            let id = identity.trim().trim_end_matches(".local").to_ascii_lowercase();
            !id.is_empty() && names.iter().any(|x| !x.is_empty() && (*x == id || x.trim_end_matches(".local") == id))
        };
        if let Some(nb) = t.neighbors.iter().find(|nb| mine(&nb.identity) || nb.mac.as_deref().is_some_and(|m| fabric.iter().any(|x| norm_mac(x.mac.as_deref().unwrap_or("")) == m))) {
            for x in fabric.iter().filter(|x| x.up) {
                out.push(Seen { node: n.id.clone(), iface: x.iface.clone(), port: nb.port.clone(), by: "lldp" });
            }
            continue;
        }
        if let Some(p) = ports.iter().find(|p| comment_names(&p.comment, n)) {
            for x in fabric.iter().filter(|x| x.up) {
                out.push(Seen { node: n.id.clone(), iface: x.iface.clone(), port: p.name.clone(), by: "comment" });
            }
        }
    }
    out
}

/// A port comment like "cell2" or "cell2 (fabric)" names this node.
pub fn comment_names(comment: &str, n: &NodeSnapshot) -> bool {
    let c = comment.to_ascii_lowercase();
    let words: Vec<&str> = c.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')).filter(|w| !w.is_empty()).collect();
    [&n.id, &n.name].iter().any(|name| {
        let name = name.to_ascii_lowercase();
        words.iter().any(|w| *w == name || w.trim_end_matches(".local") == name)
    })
}

/// Sets each port's node and `mapped_by` from what the tables show.
pub fn apply(ports: &mut [SwitchPort], t: &PortTables, nodes: &[NodeSnapshot]) -> Vec<Seen> {
    let seen = locate(ports, t, nodes);
    let mut by_port: BTreeMap<&str, &Seen> = BTreeMap::new();
    for s in &seen {
        // MAC beats LLDP beats comment when two disagree about a port.
        let rank = |b: &str| match b {
            "mac" => 0,
            "lldp" => 1,
            _ => 2,
        };
        match by_port.get(s.port.as_str()) {
            Some(old) if rank(old.by) <= rank(s.by) => {}
            _ => {
                by_port.insert(s.port.as_str(), s);
            }
        }
    }
    for p in ports.iter_mut() {
        match by_port.get(p.name.as_str()) {
            Some(s) => {
                p.node = Some(s.node.clone());
                p.mapped_by = Some(s.by.to_string());
            }
            None => {
                p.node = None;
                p.mapped_by = None;
            }
        }
    }
    seen
}

/// The ports that carry the switch's management path, each with the evidence: the port
/// its management address sits on, the ports where the LAN gateway (the Gold, and so the
/// VPN), the Switch X or this Mac are learned (via `/ip/arp` and the bridge host table),
/// and any port a Firewalla announces itself on. Disabling one of these can cut SparkBox
/// off from the switch. Without any tables, a port whose comment says uplink or
/// management is assumed to be one.
pub fn management_ports(ports: &[SwitchPort], t: &PortTables, mgmt_ip: &str, path_ips: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |p: &str, why: String| {
        if !p.is_empty() && !out.iter().any(|(x, _)| x == p) {
            out.push((p.to_string(), why));
        }
    };
    let ip_only = mgmt_ip.split('/').next().unwrap_or("").trim();
    for (addr, iface) in &t.addresses {
        if !ip_only.is_empty() && addr.split('/').next() == Some(ip_only) && ports.iter().any(|p| &p.name == iface) {
            add(iface, format!("The switch's management address {ip_only} is on this port."));
        }
    }
    for (ip, mac, iface) in t.arp.iter().filter(|(ip, _, _)| path_ips.iter().any(|p| p == ip)) {
        if ports.iter().any(|p| &p.name == iface) {
            add(iface, format!("{ip} (on SparkBox's path to the switch) is reached through this port."));
        }
        for h in t.hosts.iter().filter(|h| !h.local && &h.mac == mac) {
            add(&h.port, format!("{ip} (on SparkBox's path to the switch) is learned on this port."));
        }
    }
    for nb in &t.neighbors {
        let who = format!("{} {}", nb.identity, nb.platform.clone().unwrap_or_default()).to_ascii_lowercase();
        if who.contains("firewalla") || nb.address.as_deref().is_some_and(|a| path_ips.iter().any(|p| p == a)) {
            add(&nb.port, format!("{} announces itself on this port.", nb.identity));
        }
    }
    if t.hosts.is_empty() && t.neighbors.is_empty() && t.addresses.is_empty() {
        for p in ports {
            let c = p.comment.to_ascii_lowercase();
            if ["uplink", "management", "mgmt", "firewalla", "gateway"].iter().any(|w| c.contains(w)) {
                add(&p.name, format!("Its comment (\"{}\") says it's the switch's uplink.", p.comment));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NetStats;
    use serde_json::json;

    fn node(id: &str, macs: [&str; 2]) -> NodeSnapshot {
        NodeSnapshot {
            id: id.into(),
            name: id.into(),
            nets: macs
                .iter()
                .enumerate()
                .map(|(i, m)| NetStats { iface: format!("half{i}"), role: NetRole::Fabric, up: true, mac: Some(m.to_string()), ..Default::default() })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn mac_beats_a_stale_comment_and_lldp_and_comments_fill_the_rest() {
        let hosts = parse_bridge_hosts(&json!([
            { ".id": "*1", "mac-address": "94:6D:AE:00:00:21", "on-interface": "qsfp56-dd-2-1", "dynamic": "true", "local": "false" },
            { ".id": "*2", "mac-address": "94:6D:AE:00:00:22", "on-interface": "qsfp56-dd-2-1", "dynamic": "true", "local": "false" },
            { ".id": "*3", "mac-address": "48:A9:8A:00:00:01", "on-interface": "bridge", "dynamic": "false", "local": "true" },
        ]));
        let neighbors = parse_neighbors(&json!([
            { ".id": "*1", "interface": "qsfp56-dd-1-2,bridge", "identity": "cell3-example", "mac-address": "94:6d:ae:00:00:99", "discovered-by": "lldp" },
        ]));
        let t = PortTables { hosts, neighbors, ..Default::default() };
        let mut ports = vec![
            SwitchPort { name: "qsfp56-dd-1-1".into(), comment: "cell1".into(), ..Default::default() },
            SwitchPort { name: "qsfp56-dd-1-2".into(), comment: "cell2".into(), ..Default::default() },
            SwitchPort { name: "qsfp56-dd-2-1".into(), comment: "cell3".into(), ..Default::default() },
        ];
        let mut cell3 = node("cell3", ["94:6d:ae:00:00:31", "94:6d:ae:00:00:32"]);
        cell3.system.hostname = "cell3-example".into();
        let nodes = vec![node("cell1", ["94:6d:ae:00:00:11", "94:6d:ae:00:00:12"]), node("cell2", ["94:6d:ae:00:00:21", "94:6d:ae:00:00:22"]), cell3];
        let seen = apply(&mut ports, &t, &nodes);
        // cell2's MACs are on 2-1 even though 2-1's comment says cell3.
        assert_eq!(ports[2].node.as_deref(), Some("cell2"));
        assert_eq!(ports[2].mapped_by.as_deref(), Some("mac"));
        assert_eq!(seen.iter().filter(|s| s.node == "cell2").map(|s| s.port.as_str()).collect::<Vec<_>>(), ["qsfp56-dd-2-1", "qsfp56-dd-2-1"]);
        // cell3 announced itself on 1-2 by LLDP.
        assert_eq!(ports[1].node.as_deref(), Some("cell3"));
        assert_eq!(ports[1].mapped_by.as_deref(), Some("lldp"));
        // cell1 is found only by its port comment.
        assert_eq!(ports[0].node.as_deref(), Some("cell1"));
        assert_eq!(ports[0].mapped_by.as_deref(), Some("comment"));
    }

    #[test]
    fn switch_chip_hosts_on_a_trunk_place_nothing() {
        let h = parse_switch_hosts(&json!([
            { "mac-address": "94:6D:AE:00:00:21", "ports": "qsfp56-dd-1-2", "dynamic": "true" },
            { "mac-address": "94:6D:AE:00:00:99", "ports": "qsfp56-dd-1-1,qsfp56-dd-1-2", "dynamic": "true" },
        ]));
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].port, "qsfp56-dd-1-2");
    }

    #[test]
    fn management_port_is_found_by_address_gateway_mac_or_firewalla_neighbor() {
        let ports = vec![SwitchPort { name: "ether1".into(), ..Default::default() }, SwitchPort { name: "qsfp56-dd-1-1".into(), ..Default::default() }];
        let by_addr = PortTables { addresses: vec![("192.168.50.2/24".into(), "ether1".into())], ..Default::default() };
        let names = |v: Vec<(String, String)>| v.into_iter().map(|(p, _)| p).collect::<Vec<_>>();
        assert_eq!(names(management_ports(&ports, &by_addr, "192.168.50.2", &[])), ["ether1"]);
        // The gateway's MAC (from the switch's ARP table) is learned behind ether1.
        let by_mac = PortTables {
            hosts: parse_bridge_hosts(&json!([{ "mac-address": "20:6d:31:c4:59:8c", "on-interface": "ether1", "dynamic": "true" }])),
            arp: parse_arp(&json!([{ "address": "192.168.50.1", "mac-address": "20:6d:31:c4:59:8c", "interface": "bridge" }, { "address": "192.168.50.9", "interface": "bridge" }])),
            ..Default::default()
        };
        assert_eq!(by_mac.arp.len(), 1, "an incomplete ARP entry has no MAC");
        let found = management_ports(&ports, &by_mac, "192.168.50.2", &["192.168.50.1".into()]);
        assert_eq!(found[0].0, "ether1");
        assert!(found[0].1.contains("192.168.50.1"), "{found:?}");
        assert!(management_ports(&ports, &by_mac, "192.168.50.2", &["192.168.50.77".into()]).is_empty());
        let by_nb = PortTables { neighbors: vec![Neighbor { port: "ether1".into(), identity: "Firewalla Switch X".into(), discovered_by: "lldp".into(), ..Default::default() }], ..Default::default() };
        assert_eq!(names(management_ports(&ports, &by_nb, "192.168.50.2", &[])), ["ether1"]);
        // A bridge address (not a port) says nothing about which port carries it.
        let bridge = PortTables { addresses: vec![("192.168.50.2/24".into(), "bridge".into())], ..Default::default() };
        assert!(management_ports(&ports, &bridge, "192.168.50.2", &[]).is_empty());
        // No tables at all: fall back to what the comments say.
        let commented = vec![SwitchPort { name: "ether1".into(), comment: "management uplink to Firewalla".into(), ..Default::default() }, ports[1].clone()];
        assert_eq!(names(management_ports(&commented, &PortTables::default(), "192.168.50.2", &[])), ["ether1"]);
    }
}
