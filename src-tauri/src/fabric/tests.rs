//! The fabric monitor against the simulator's scenarios (with a controlled clock), the
//! cabling baseline in Live, the NCCL transport check, and the doctor: through the
//! simulator's real tool output, and over SSH against fake nodes.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use super::doctor::{self, Baselines, BoxFut, Checks, DoctorReport, DoctorRequest, Out, Runner, Status};
use super::*;
use crate::demo::DemoCluster;
use crate::inventory::Inventory;
use crate::model::{Container, GpuStats, SwitchPort, SwitchSnapshot};
use crate::routeros::mapping::{HostEntry, PortTables};

/// Plays the simulator for `secs` seconds through the monitor, one snapshot a second.
struct Rig {
    demo: DemoCluster,
    mon: FabricMonitor,
    t: u64,
}

impl Rig {
    fn new() -> Rig {
        Rig { demo: DemoCluster::new(&Inventory::example()), mon: FabricMonitor::new(None), t: 1_800_000_000_000 }
    }

    fn run(&mut self, secs: u64) -> (FabricState, ClusterSnapshot) {
        let mut last = None;
        for _ in 0..secs {
            self.demo.advance();
            let mut snap = self.demo.snapshot();
            self.t += 1000;
            snap.ts_ms = self.t;
            let tables: BTreeMap<String, PortTables> = [("crs804".to_string(), self.demo.switch_tables("crs804"))].into();
            let world = demo_world_cabling(&snap);
            let state = self.mon.assess(&mut snap, &tables, Some(&world));
            last = Some((state, snap));
        }
        last.unwrap()
    }
}

fn iface<'a>(s: &'a FabricState, node: &str, half: u8) -> &'a IfaceHealth {
    s.ifaces.iter().find(|i| i.node == node && i.half == half).unwrap_or_else(|| panic!("no {node} half {half}"))
}

#[test]
fn everyday_busy_links_are_protected_and_idle_priorities_stay_unknown() {
    let mut rig = Rig::new();
    let (s, snap) = rig.run(40);
    assert_eq!(s.verdict.level, Level::Info, "an idle link cannot prove its traffic priority: {:?}", s.verdict);
    assert!(s.summary.contains("PFC protection unverified"), "{}", s.summary);
    assert_eq!(s.ifaces.len(), 8, "two addressed halves per node");
    for i in &s.ifaces {
        if i.node == "gx10-0000" {
            assert_eq!(i.verdict.kind, VerdictKind::NotLossless, "idle priority unknown");
        } else {
            assert_eq!(i.verdict.kind, VerdictKind::Lossless, "{} half {}: {:?}", i.node, i.half, i.verdict);
            assert!(i.verdict.detail.starts_with("No drops, retransmits or pause storms in"), "{}", i.verdict.detail);
        }
        assert_eq!(i.mapped_by.as_deref(), Some("mac"), "placed by the bridge host table");
        assert_eq!(i.cabling_ok, Some(true));
        // Every counter is reported (none is missing in the simulator), and none of the loss ones moves.
        assert!(i.counters.iter().all(|c| c.total.is_some()));
        assert!(i.counters.iter().filter(|c| matches!(c.group, Group::Loss | Group::Phy | Group::Drop)).all(|c| c.per_s == Some(0.0)), "{:?}", i.counters);
    }
    assert!(s.cabling.is_empty());
    // The 3-node pipeline is a multi-node serving group whose NCCL traffic is RDMA.
    let t = s.transport.iter().find(|t| t.name == "llama-70b").expect("the Ray group");
    assert_eq!(t.nodes, ["cell1", "cell2", "cell3"]);
    assert!(t.env_issues.is_empty(), "{:?}", t.env_issues);
    if let Some(p) = t.rdma_pct {
        assert!(p > 90.0, "{p}");
        assert!(t.message.starts_with("NCCL over RoCE: "), "{}", t.message);
    }
    // The switch ports agree.
    let crs = snap.switches.iter().find(|s| s.id == "crs804").unwrap();
    let p12 = crs.ports.iter().find(|p| p.name == "qsfp56-dd-2-1").unwrap();
    assert_eq!((p12.node.as_deref(), p12.mapped_by.as_deref(), p12.cabling_ok), (Some("cell2"), Some("mac"), Some(true)));
    // The management uplink is known, with its evidence.
    let ether1 = crs.ports.iter().find(|p| p.name == "ether1").unwrap();
    assert!(ether1.management.as_deref().is_some_and(|w| w.contains("Firewalla")), "{:?}", ether1.management);
    assert!(crs.ports.iter().filter(|p| p.name != "ether1").all(|p| p.management.is_none()));
}

#[test]
fn roce_loss_is_red_on_cell2_half_2_with_the_cause() {
    let mut rig = Rig::new();
    rig.run(15);
    rig.demo.set_scenario(Some("roce-loss")).unwrap();
    let (s, _) = rig.run(15);
    assert_eq!(s.verdict.level, Level::Crit);
    assert_eq!(s.verdict.title, "RoCE is retransmitting on cell2 half 2", "{:?}", s.verdict);
    let bad = iface(&s, "cell2", 1);
    assert_eq!(bad.verdict.kind, VerdictKind::Lossy);
    assert!(bad.verdict.detail.starts_with("Out of sequence "), "{}", bad.verdict.detail);
    assert!(bad.verdict.detail.contains("CRC errors") && bad.verdict.detail.ends_with("point at the optic or cable."), "the cause is named: {}", bad.verdict.detail);
    assert!(bad.verdict.action.as_deref().unwrap_or("").contains("switch port 2-1"), "{:?}", bad.verdict.action);
    let oos = bad.counters.iter().find(|c| c.key == "outOfSequence").unwrap();
    assert!(oos.per_s.unwrap() >= 40.0, "{oos:?}");
    assert!(bad.loss_last_hour > 0);
    // Half 1 is fine.
    assert_eq!(iface(&s, "cell2", 0).verdict.kind, VerdictKind::Lossless);

    // Once it recovers the half reads "lost packets earlier" for the hour: history, so
    // information, not a warning (nothing is lost now and no alert fires).
    let (s, _) = rig.run(200);
    let after = iface(&s, "cell2", 1);
    assert_eq!((after.verdict.kind, after.verdict.level), (VerdictKind::WasLossy, Level::Info), "{:?}", after.verdict);
    assert!(after.verdict.detail.contains("in the last hour, the last"), "{}", after.verdict.detail);

    // Back to the everyday world: a simulated fault isn't carried into it.
    rig.demo.set_scenario(None).unwrap();
    let (s, _) = rig.run(15);
    assert_eq!(iface(&s, "cell2", 1).verdict.kind, VerdictKind::Lossless);
    assert_eq!(s.verdict.level, Level::Info, "idle fourth node has no measured priority");
}

#[test]
fn verdict_and_alerts_agree() {
    // roce-loss, then the everyday world: at every step from then on, an interface reads
    // lossless exactly when it has no RoCE alert that's firing (one that's recovering is fine).
    let mut sim = crate::health::tests::Sim::new(Some("roce-loss"));
    sim.run(60);
    sim.demo.set_scenario(None).unwrap();
    for step in 0..90 {
        let (_, snap) = sim.run(1);
        let view = sim.view(&snap);
        let state = sim.fabric.clone().unwrap();
        for i in &state.ifaces {
            let lossless = i.verdict.level < Level::Warn;
            let firing = view.alerts.iter().any(|a| {
                a.rule == "roce-iface" && a.entity.node.as_deref() == Some(i.node.as_str()) && a.entity.id == i.iface && a.state == crate::health::AlertState::Firing && a.recovers_at_ms.is_none()
            });
            assert_eq!(lossless, !firing, "step {step}: {} half {} reads {:?} while the alert is {}", i.node, i.half + 1, i.verdict.kind, if firing { "firing" } else { "quiet" });
        }
    }
    // While it plays, the half that loses packets is lossy in both.
    let mut sim = crate::health::tests::Sim::new(Some("roce-loss"));
    let (_, snap) = sim.run(75);
    let state = sim.fabric.clone().unwrap();
    let bad = iface(&state, "cell2", 1);
    assert_eq!(bad.verdict.kind, VerdictKind::Lossy);
    assert!(sim.view(&snap).alerts.iter().any(|a| a.rule == "roce-iface" && a.entity.id == bad.iface && a.counted));
}

#[test]
fn the_rate_table_is_the_verdicts_table() {
    let mut rig = Rig::new();
    rig.run(15);
    rig.demo.set_scenario(Some("roce-loss")).unwrap();
    let (s, _) = rig.run(15);
    let rates = rig.mon.rates().expect("assessed");
    let bad = iface(&s, "cell2", 1);
    let r = &rates.ifaces[&format!("cell2/{}", bad.iface)];
    assert_eq!(r.label(), "cell2 half 2");
    assert!(r.switch_port.as_deref().is_some_and(|p| p.starts_with("crs804/")), "{:?}", r.switch_port);
    // The verdict shows exactly the 10 s rates the alert reads.
    for c in &bad.counters {
        assert_eq!(c.per_s, r.r10(&c.key), "{}", c.key);
    }
    assert!(r.r60("outOfSequence").is_none(), "not a full minute of samples yet");
    // The pause-storm line and its 30 s are the alert table's.
    let t = crate::health::roce_rules::table();
    assert_eq!(t.pause_storm_s * 1000, PAUSE_STORM_MS);
    assert!(t.counters.iter().filter(|c| c.group == "pause").all(|c| c.warn == Some(PAUSE_STORM_PER_S)));
}

#[test]
fn a_pause_storm_must_last_30_seconds() {
    let mut rig = Rig::new();
    rig.run(5);
    rig.demo.set_scenario(Some("pfc-storm")).unwrap();
    let (s, _) = rig.run(20);
    assert_ne!(iface(&s, "gx10-0000", 0).verdict.kind, VerdictKind::PauseStorm, "not yet: 20 s");
    let tx = iface(&s, "gx10-0000", 0).counters.iter().find(|c| c.key == "txPause3").unwrap().per_s.unwrap();
    assert!(tx > 45_000.0, "{tx}");
    let (s, snap) = rig.run(15);
    let g = iface(&s, "gx10-0000", 0);
    assert_eq!(g.verdict.kind, VerdictKind::PauseStorm, "{:?}", g.verdict);
    // 50,000 a second, give or take 1,500: "49k/s", "50k/s" or "51k/s".
    assert!(["49k/s", "50k/s", "51k/s"].iter().any(|r| g.verdict.detail.starts_with(&format!("gx10-0000 half 1 sends {r} pause frames for "))), "{}", g.verdict.detail);
    assert_eq!(s.verdict.level, Level::Crit);
    // The switch counts the pause frames it receives from gx10-0000.
    let crs = snap.switches.iter().find(|s| s.id == "crs804").unwrap();
    assert!(crs.ports.iter().find(|p| p.node.as_deref() == Some("gx10-0000")).unwrap().rx_pause > 1_000_000);
}

#[test]
fn congestion_is_informative_and_loses_nothing() {
    let mut rig = Rig::new();
    rig.run(5);
    rig.demo.set_scenario(Some("fabric-congestion")).unwrap();
    let (s, _) = rig.run(6);
    let c = iface(&s, "cell2", 0);
    assert_eq!(c.verdict.kind, VerdictKind::Congested, "{:?}", c.verdict);
    assert_eq!(c.verdict.level, Level::Info);
    assert_eq!(c.verdict.detail, "DCQCN is pacing senders; expected during all-reduce bursts.");
    assert_eq!(c.loss_last_hour, 0);
    assert!(s.verdict.level < Level::Warn, "congestion alone isn't a problem: {:?}", s.verdict);
    assert!(s.verdict.title.starts_with("Congested on "), "{}", s.verdict.title);
}

#[test]
fn swapped_cables_are_found_by_mac_and_can_be_accepted() {
    let mut rig = Rig::new();
    rig.run(3);
    rig.demo.set_scenario(Some("miscabled")).unwrap();
    let (s, snap) = rig.run(3);
    let msgs: Vec<&str> = s.cabling.iter().map(|c| c.message.as_str()).collect();
    assert!(msgs.contains(&"cell2 half 1 is on 3-1; expected 2-1"), "{msgs:?}");
    assert!(msgs.contains(&"cell3 half 2 is on 2-1; expected 3-1"), "{msgs:?}");
    assert_eq!(s.cabling.len(), 4);
    assert_eq!(s.verdict.title, "Cabling changed");
    let crs = snap.switches.iter().find(|s| s.id == "crs804").unwrap();
    let p12 = crs.ports.iter().find(|p| p.name == "qsfp56-dd-2-1").unwrap();
    // The comment still says cell2, but the MACs behind the port are cell3's.
    assert_eq!(p12.comment, "cell2");
    assert_eq!((p12.node.as_deref(), p12.mapped_by.as_deref()), (Some("cell3"), Some("mac")));
    assert_eq!((p12.cabling_ok, p12.expected_node.as_deref()), (Some(false), Some("cell2")));
    let cell2 = snap.nodes.iter().find(|n| n.id == "cell2").unwrap();
    assert!(cell2.nets.iter().filter(|x| x.ip.is_some() && x.role == NetRole::Fabric).all(|x| x.switch_port.as_deref() == Some("crs804/qsfp56-dd-3-1")));

    // Accepting makes the swap the baseline (Demo: until the scenario changes).
    assert_eq!(rig.mon.accept_cabling(true, Some("miscabled".into())).unwrap(), 8);
    let (s, _) = rig.run(2);
    assert!(s.cabling.is_empty(), "{:?}", s.cabling);
    rig.demo.set_scenario(None).unwrap();
    let (s, _) = rig.run(2);
    assert!(s.cabling.is_empty(), "back to the world's own cabling: {:?}", s.cabling);
}

#[test]
fn a_fabric_link_down_and_mtu_1500_are_not_hidden() {
    let mut rig = Rig::new();
    rig.run(3);
    rig.demo.set_scenario(Some("switch-port-down")).unwrap();
    let (s, _) = rig.run(3);
    let down = iface(&s, "cell3", 0);
    assert_eq!((down.verdict.kind, down.verdict.level), (VerdictKind::Down, Level::Crit));
    rig.demo.set_scenario(Some("mtu-mismatch")).unwrap();
    let (s, _) = rig.run(2);
    assert_eq!(iface(&s, "gx10-0000", 1).mtu, Some(1500));
    assert_eq!(iface(&s, "gx10-0000", 0).mtu, Some(9000));
}

fn node(id: &str, i: u8, env: &[(&str, &str)], rdma: f64, ip: f64) -> NodeSnapshot {
    let fabric = |iface: &str, half: u8| NetStats {
        iface: iface.into(),
        role: NetRole::Fabric,
        up: true,
        speed_mbps: 200_000,
        ip: Some(format!("192.168.{}.{i}", 100 + half)),
        mac: Some(format!("94:6d:ae:00:0{half}:0{i}")),
        rx_bps: ip,
        tx_bps: ip,
        rdma_rx_bps: Some(rdma),
        rdma_tx_bps: Some(rdma),
        ..Default::default()
    };
    NodeSnapshot {
        id: id.into(),
        name: id.into(),
        status: LinkStatus::Online,
        gpu: GpuStats { util_pct: 60.0, ..Default::default() },
        nets: vec![NetStats { iface: "enP7s7".into(), role: NetRole::Lan, up: true, ..Default::default() }, fabric("enp1s0f1np1", 0), fabric("enP2p1s0f1np1", 1)],
        containers: vec![Container {
            name: "vllm".into(),
            state: "running".into(),
            labels: [("sparkbox.deployment".to_string(), "qwen-2n".to_string())].into(),
            env_flags: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn nccl_on_tcp_sockets_and_bad_env_are_called_out() {
    let mon = FabricMonitor::new(None);
    let mut snap = ClusterSnapshot {
        ts_ms: 1_000,
        nodes: vec![
            node("a", 1, &[("NCCL_IB_DISABLE", "1"), ("NCCL_SOCKET_IFNAME", "enP7s7")], 0.0, 4e9),
            node("b", 2, &[("NCCL_IB_HCA", "rocep1s0f1"), ("NCCL_SOCKET_IFNAME", "enp1s0f1np1")], 0.0, 4e9),
        ],
        ..Default::default()
    };
    let mut s = mon.assess(&mut snap, &BTreeMap::new(), None);
    for i in 0..4 {
        snap.ts_ms += 1000 * (i + 1);
        s = mon.assess(&mut snap, &BTreeMap::new(), None);
    }
    let t = &s.transport[0];
    assert_eq!(t.name, "qwen-2n");
    assert_eq!(t.message, "NCCL is using TCP sockets (RDMA 0%)");
    assert_eq!(t.level, Level::Warn);
    assert!(t.env_issues.iter().any(|e| e.starts_with("NCCL_IB_DISABLE=1 on a")), "{:?}", t.env_issues);
    assert!(t.env_issues.iter().any(|e| e.starts_with("NCCL_IB_HCA isn't set on a")), "{:?}", t.env_issues);
    assert!(t.env_issues.iter().any(|e| e.starts_with("NCCL_SOCKET_IFNAME=enP7s7 on a is the LAN")), "{:?}", t.env_issues);
    assert!(!t.env_issues.iter().any(|e| e.contains(" on b")), "b is set up right: {:?}", t.env_issues);
    assert_eq!(s.verdict.title, "NCCL is using TCP sockets (RDMA 0%)");
    // No RoCE counters from these nodes: said, not shown as lossless.
    assert!(s.ifaces.iter().all(|i| i.verdict.kind == VerdictKind::NoData));
}

/// A model copied over the fabric (TCP) from one member while the pipeline serves over RDMA:
/// that member's IP bytes dwarf its RDMA, but NCCL hasn't fallen back to sockets.
#[test]
fn a_load_from_labels_is_a_serving_group_and_falling_back_to_tcp_is_seen() {
    // C-L2 has no -e and runs its image by id, so only the labels say this is a model server.
    let all: serde_json::Value = serde_json::from_str(include_str!("../models/fixtures/loads-c-l2.json")).unwrap();
    let rank = |r: usize, i: u8| {
        let mut n = node(all["tp2"][r]["node"].as_str().unwrap(), i, &[], 0.0, 4e9);
        n.containers = vec![serde_json::from_value(all["tp2"][r]["container"].clone()).unwrap()];
        n
    };
    assert!(rank(0, 1).containers[0].image.starts_with("sha256:") && rank(0, 1).containers[0].env_flags.is_empty(), "the fixture is C-L2's shape");
    let mon = FabricMonitor::new(None);
    let mut snap = ClusterSnapshot { ts_ms: 1_000, nodes: vec![rank(0, 1), rank(1, 2)], ..Default::default() };
    let mut s = mon.assess(&mut snap, &BTreeMap::new(), None);
    for i in 0..4 {
        snap.ts_ms += 1000 * (i + 1);
        s = mon.assess(&mut snap, &BTreeMap::new(), None);
    }
    assert_eq!(s.transport.len(), 1, "{:?}", s.transport);
    let t = &s.transport[0];
    assert_eq!(t.name, "m2tp2");
    assert_eq!(t.message, "NCCL is using TCP sockets (RDMA 0%)");
    assert!(t.env_issues.is_empty(), "a load's env isn't readable, so it isn't judged: {:?}", t.env_issues);
    assert_eq!(s.verdict.title, "NCCL is using TCP sockets (RDMA 0%)");
}

#[test]
fn a_model_copy_from_one_member_isnt_nccl_on_tcp() {
    let mon = FabricMonitor::new(None);
    let mut snap = ClusterSnapshot {
        ts_ms: 1_000,
        nodes: vec![
            node("a", 1, &[("NCCL_IB_HCA", "rocep1s0f1")], 40e6, 4.7e9),
            node("b", 2, &[("NCCL_IB_HCA", "rocep1s0f1")], 40e6, 1e6),
            node("c", 3, &[("NCCL_IB_HCA", "rocep1s0f1")], 40e6, 1e6),
        ],
        ..Default::default()
    };
    let mut s = mon.assess(&mut snap, &BTreeMap::new(), None);
    for i in 0..4 {
        snap.ts_ms += 1000 * (i + 1);
        s = mon.assess(&mut snap, &BTreeMap::new(), None);
    }
    let t = &s.transport[0];
    assert_eq!(t.level, Level::Ok, "{}", t.message);
    assert!(t.message.starts_with("NCCL over RoCE: 98%"), "{}", t.message);
    // Every member on sockets is still called out.
    for n in &mut snap.nodes {
        for x in n.nets.iter_mut().filter(|x| x.role == NetRole::Fabric) {
            (x.rdma_rx_bps, x.rdma_tx_bps, x.rx_bps, x.tx_bps) = (Some(0.0), Some(0.0), 4e8, 4e8);
        }
    }
    let mon = FabricMonitor::new(None);
    for i in 0..5 {
        snap.ts_ms += 1000 * (i + 1);
        s = mon.assess(&mut snap, &BTreeMap::new(), None);
    }
    assert_eq!(s.transport[0].message, "NCCL is using TCP sockets (RDMA 0%)");
}

fn live_switch(ports: &[(&str, &str)]) -> SwitchSnapshot {
    SwitchSnapshot {
        id: "crs804".into(),
        name: "CRS804 DDQ".into(),
        kind: SwitchKind::RouterOs,
        host: "192.168.50.2".into(),
        status: LinkStatus::Online,
        configured: true,
        ports: ports.iter().map(|(name, comment)| SwitchPort { name: name.to_string(), comment: comment.to_string(), running: true, ..Default::default() }).collect(),
        ..Default::default()
    }
}

#[test]
fn live_cabling_is_learned_once_saved_and_compared() {
    let dir = std::env::temp_dir().join(format!("sb-fabric-{}-{}", std::process::id(), fastrand::u32(..)));
    let path = dir.join("fabric_cabling.json");
    let mon = FabricMonitor::new(Some(path.clone()));
    let tables = |a: &str, b: &str| -> BTreeMap<String, PortTables> {
        let hosts = vec![
            HostEntry { mac: "94:6D:AE:00:00:01".into(), port: a.into(), dynamic: true, local: false },
            HostEntry { mac: "94:6D:AE:00:01:01".into(), port: a.into(), dynamic: true, local: false },
            HostEntry { mac: "94:6D:AE:00:00:02".into(), port: b.into(), dynamic: true, local: false },
            HostEntry { mac: "94:6D:AE:00:01:02".into(), port: b.into(), dynamic: true, local: false },
        ];
        [("crs804".to_string(), PortTables { hosts, ..Default::default() })].into()
    };
    let mut snap = ClusterSnapshot {
        ts_ms: 1_000,
        nodes: vec![node("a", 1, &[], 1e9, 1e6), node("b", 2, &[], 1e9, 1e6)],
        switches: vec![live_switch(&[("qsfp56-dd-1-1", "a"), ("qsfp56-dd-1-2", "b")])],
        ..Default::default()
    };
    let s = mon.assess(&mut snap, &tables("qsfp56-dd-1-1", "qsfp56-dd-1-2"), None);
    assert!(s.baseline_set && s.cabling.is_empty());
    let saved: Cabling = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["crs804"]["b/enP2p1s0f1np1"], "qsfp56-dd-1-2");
    // The nodes' fabric interfaces now name their switch port.
    assert_eq!(snap.nodes[1].nets[1].switch_port.as_deref(), Some("crs804/qsfp56-dd-1-2"));

    // Recabled: a fresh monitor (as after a restart) reads the saved baseline and flags it.
    let mon = FabricMonitor::new(Some(path.clone()));
    snap.ts_ms += 1000;
    let s = mon.assess(&mut snap, &tables("qsfp56-dd-1-2", "qsfp56-dd-1-1"), None);
    let msgs: Vec<&str> = s.cabling.iter().map(|c| c.message.as_str()).collect();
    assert!(msgs.contains(&"a half 1 is on 1-2; expected 1-1"), "{msgs:?}");
    let p11 = &snap.switches[0].ports[0];
    assert_eq!((p11.node.as_deref(), p11.cabling_ok, p11.expected_node.as_deref()), (Some("b"), Some(false), Some("a")));
    // Accept: saved, and quiet again.
    mon.accept_cabling(false, None).unwrap();
    let saved: Cabling = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["crs804"]["a/enp1s0f1np1"], "qsfp56-dd-1-2");
    snap.ts_ms += 1000;
    assert!(mon.assess(&mut snap, &tables("qsfp56-dd-1-2", "qsfp56-dd-1-1"), None).cabling.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_counter_stays_missing() {
    let mon = FabricMonitor::new(None);
    let mut n = node("a", 1, &[], 1e9, 1e6);
    n.nets[1].roce = Some(RoceCounters { out_of_sequence: Some(5), ..Default::default() });
    let mut snap = ClusterSnapshot { ts_ms: 1_000, nodes: vec![n], ..Default::default() };
    let s = mon.assess(&mut snap, &BTreeMap::new(), None);
    let c = &iface(&s, "a", 0).counters;
    assert_eq!(c.iter().find(|c| c.key == "outOfSequence").unwrap().total, Some(5));
    let crc = c.iter().find(|c| c.key == "rxCrcErrorsPhy").unwrap();
    assert_eq!((crc.total, crc.per_s), (None, None), "absent, never 0");
}

// ---- The doctor

/// The simulator's replies, with no pacing.
struct SimRunner(Mutex<DemoCluster>);

impl Runner for SimRunner {
    fn run<'a>(&'a self, node: &'a str, cmd: String, _limit: Duration, _stoppable: bool) -> BoxFut<'a, Result<Out, String>> {
        Box::pin(async move {
            let (code, stdout, stderr) = self.0.lock().unwrap().fleet_reply(node, &cmd);
            Ok(Out { code: Some(code), stdout, stderr })
        })
    }
}

struct Quiet;

impl doctor::Progress for Quiet {
    fn update(&self, _r: &DoctorReport) {}
    fn cancelled(&self) -> bool {
        false
    }
}

fn all_checks() -> Checks {
    Checks { mtu: true, ping: true, bandwidth: true, latency: true, nccl: false }
}

fn snapshot_of(rig: &mut Rig) -> ClusterSnapshot {
    rig.run(2).1
}

async fn doctor_on(rig: &mut Rig, nodes: &[&str], baselines: &Baselines) -> DoctorReport {
    let snap = snapshot_of(rig);
    let req = DoctorRequest { nodes: nodes.iter().map(|s| s.to_string()).collect(), checks: all_checks(), ..Default::default() };
    let report = Mutex::new(DoctorReport::default());
    let demo = std::mem::replace(&mut rig.demo, DemoCluster::new(&Inventory { nodes: vec![], switches: vec![] }));
    let runner = SimRunner(Mutex::new(demo));
    doctor::run(&runner, &snap, &req, baselines, &report, &Quiet).await;
    rig.demo = runner.0.into_inner().unwrap();
    report.into_inner().unwrap()
}

#[tokio::test]
async fn the_doctor_in_demo_passes_measures_and_saves_a_baseline() {
    let mut rig = Rig::new();
    rig.run(3);
    let r = doctor_on(&mut rig, &["cell1", "cell2", "cell3", "gx10-0000"], &Baselines::new()).await;
    assert_eq!(r.mtu.len(), 8);
    assert!(r.mtu.iter().all(|m| m.status == Status::Pass), "{:?}", r.mtu);
    // 4 nodes, both directions, 2 halves.
    assert_eq!(r.ping.len(), 24);
    assert!(r.ping.iter().all(|p| p.result == "ok"), "{:?}", r.ping);
    // gx10-0000 has no perftest: said once, its pairs skipped.
    assert_eq!(r.prereqs.len(), 1);
    assert_eq!(r.prereqs[0].message, "perftest isn't installed on gx10-0000");
    assert_eq!(r.bandwidth.len(), 12);
    let measured: Vec<_> = r.bandwidth.iter().filter(|c| c.gbps.is_some()).collect();
    assert_eq!(measured.len(), 6, "3 Ray pairs × 2 halves");
    for c in &measured {
        let g = c.gbps.unwrap();
        let (lo, hi) = if c.half == 0 { (93.0, 96.0) } else { (92.0, 95.0) };
        assert!((lo..=hi).contains(&g), "{} → {} half {}: {g}", c.from, c.to, c.half + 1);
        let lat = c.lat_us.unwrap();
        assert!((2.1..=2.7).contains(&lat), "{lat}");
        assert_eq!(c.status, Status::Pass);
    }
    // Not all pairs ran (gx10's were skipped), so no baseline yet.
    assert!(doctor::maybe_save_baselines(None, &Baselines::new(), &r).is_none());

    // Install perftest, run again: everything passes and becomes the baseline.
    rig.demo.install_perftest("gx10-0000").unwrap();
    let r = doctor_on(&mut rig, &["cell1", "cell2", "cell3", "gx10-0000"], &Baselines::new()).await;
    assert!(r.prereqs.is_empty() && r.findings.is_empty(), "{:?}", r.findings);
    let dir = std::env::temp_dir().join(format!("sb-baselines-{}-{}", std::process::id(), fastrand::u32(..)));
    let path = dir.join("fabric_baselines.json");
    let saved = doctor::maybe_save_baselines(Some(&path), &Baselines::new(), &r).expect("first all-pass run");
    assert_eq!(saved.len(), 12);
    let on_disk: Baselines = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(on_disk, saved);
    // A later run doesn't replace it.
    assert!(doctor::maybe_save_baselines(Some(&path), &saved, &r).is_none());

    // roce-loss: cell2's half 2 pairs fall below 85% of expected.
    rig.demo.set_scenario(Some("roce-loss")).unwrap();
    let r = doctor_on(&mut rig, &["cell2", "cell3"], &saved).await;
    let h2 = r.bandwidth.iter().find(|c| c.half == 1).unwrap();
    assert_eq!(h2.status, Status::Fail, "{h2:?}");
    assert!(r.findings.iter().any(|f| f.starts_with("cell2→cell3 half 2: ") && f.contains("below 85% of the expected 92 Gb/s")), "{:?}", r.findings);
    assert_eq!(r.bandwidth.iter().find(|c| c.half == 0).unwrap().status, Status::Pass);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn nccl_tests_run_across_the_ray_nodes_and_explain_a_node_without_them() {
    let mut rig = Rig::new();
    let snap = snapshot_of(&mut rig);
    let run = |nodes: &[&str]| DoctorRequest { nodes: nodes.iter().map(|s| s.to_string()).collect(), checks: Checks { nccl: true, ..Default::default() }, ..Default::default() };
    let runner = SimRunner(Mutex::new(std::mem::replace(&mut rig.demo, DemoCluster::new(&Inventory { nodes: vec![], switches: vec![] }))));
    let report = Mutex::new(DoctorReport::default());
    doctor::run(&runner, &snap, &run(&["cell1", "cell2", "cell3"]), &Baselines::new(), &report, &Quiet).await;
    let n = report.into_inner().unwrap().nccl.unwrap();
    assert_eq!(n.status, Status::Pass, "{n:?}");
    let bw = n.busbw_gbps.unwrap();
    assert!((97.0..=102.0).contains(&bw), "{bw}");
    assert_eq!(n.message, format!("Bus bandwidth {bw:.0} Gb/s at 8 GiB"));
    let report = Mutex::new(DoctorReport::default());
    doctor::run(&runner, &snap, &run(&["cell1", "gx10-0000"]), &Baselines::new(), &report, &Quiet).await;
    let n = report.into_inner().unwrap().nccl.unwrap();
    assert_eq!(n.status, Status::Fail);
    assert!(n.message.starts_with("mpirun was unable to launch"), "{}", n.message);
}

#[tokio::test]
async fn the_doctor_compares_with_the_baseline() {
    let mut rig = Rig::new();
    rig.run(3);
    // A baseline from a better day: 110 Gb/s on cell1→cell2 half 1.
    let base: Baselines = [(doctor::pair_key("cell1", "cell2", 0), doctor::Baseline { gbps: 140.0, lat_us: None, ts_ms: 1 })].into();
    let r = doctor_on(&mut rig, &["cell1", "cell2"], &base).await;
    let c = r.bandwidth.iter().find(|c| c.half == 0).unwrap();
    assert_eq!(c.status, Status::Warn);
    assert!(c.message.contains("% below its baseline of 140 Gb/s"), "{}", c.message);
}

#[tokio::test]
async fn the_doctor_finds_the_mtu_mismatch() {
    let mut rig = Rig::new();
    rig.run(3);
    rig.demo.set_scenario(Some("mtu-mismatch")).unwrap();
    let r = doctor_on(&mut rig, &["cell1", "gx10-0000"], &Baselines::new()).await;
    let audit = r.mtu.iter().find(|m| m.node == "gx10-0000" && m.half == 1).unwrap();
    assert_eq!((audit.status.clone(), audit.message.as_str()), (Status::Fail, "MTU 1500; the fabric uses 9000"));
    let from_gx = r.ping.iter().find(|p| p.from == "gx10-0000" && p.half == 1).unwrap();
    assert_eq!(from_gx.result, "fails: MTU");
    assert!(from_gx.detail.as_deref().unwrap().contains("MTU 1500"), "{from_gx:?}");
    let to_gx = r.ping.iter().find(|p| p.to == "gx10-0000" && p.half == 1).unwrap();
    assert_eq!(to_gx.result, "fails: MTU", "small pings get through, jumbo frames don't: {to_gx:?}");
    assert!(r.ping.iter().filter(|p| p.half == 0).all(|p| p.result == "ok"));
}

// ---- Over SSH, against fake nodes running the real commands

/// ib_write_bw / ib_send_lat stand-ins: with no peer address they're the server (they
/// note their PID, wait, and record a TERM); with one they print real perftest output,
/// after `$SB_CLIENT_SLEEP` seconds.
fn perftest_shim(fixture: &str, name: &str) -> String {
    format!(
        "last=\"\"; for a in \"$@\"; do last=\"$a\"; done\n\
         case \"$last\" in\n\
           192.168.*) sleep ${{SB_CLIENT_SLEEP:-0}}; cat <<'EOF'\n{fixture}EOF\n;;\n\
           *) echo $$ >> \"$HOME/{name}-servers\"; trap 'echo $$ >> \"$HOME/{name}-stopped\"; exit 143' TERM; sleep 30 & wait ;;\n\
         esac\n"
    )
}

async fn fake_pair(tag: &str, client_sleep: u32) -> (crate::fake_node::Node, crate::fake_node::Node, std::sync::Arc<crate::live::Live>, ClusterSnapshot, std::path::PathBuf) {
    use crate::fake_node::{connect, fake_node, real_shell, temp};
    let dir = temp(tag);
    let (na, pa) = fake_node(&dir.join("a"), real_shell()).await;
    let (nb, pb) = fake_node(&dir.join("b"), real_shell()).await;
    for n in [&na, &nb] {
        std::fs::create_dir_all(&n.home).unwrap();
        let bw = perftest_shim(include_str!("fixtures/ib_write_bw.txt"), "ib_write_bw");
        let lat = perftest_shim(include_str!("fixtures/ib_send_lat.txt"), "ib_send_lat");
        n.bin("ib_write_bw", &format!("SB_CLIENT_SLEEP={client_sleep}\n{bw}"));
        n.bin("ib_send_lat", &lat);
        n.bin("ping", &format!("cat <<'EOF'\n{}EOF\n", include_str!("fixtures/ping_ok.txt")));
    }
    let (live, _inv) = connect(&dir, &[("a", pa), ("b", pb)]).await;
    let mut a = node("a", 1, &[], 0.0, 0.0);
    let mut b = node("b", 2, &[], 0.0, 0.0);
    for n in [&mut a, &mut b] {
        n.nets[1].rdma_dev = Some("rocep1s0f1".into());
        n.nets[2].rdma_dev = Some("roceP2p1s0f1".into());
        n.nets[1].mtu = Some(9000);
        n.nets[2].mtu = Some(9000);
    }
    (na, nb, live, ClusterSnapshot { nodes: vec![a, b], ..Default::default() }, dir)
}

fn lines(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path).unwrap_or_default().lines().map(String::from).collect()
}

#[tokio::test]
async fn the_doctor_runs_its_exact_commands_over_ssh() {
    let (na, nb, live, snap, _dir) = fake_pair("fabric-doctor", 0).await;
    let req = DoctorRequest { nodes: vec!["a".into(), "b".into()], checks: all_checks(), ..Default::default() };
    let runner = super::commands::LiveRunner { live: live.clone(), cancel: crate::live::CancelToken::new() };
    let report = Mutex::new(DoctorReport::default());
    doctor::run(&runner, &snap, &req, &Baselines::new(), &report, &Quiet).await;
    let r = report.into_inner().unwrap();
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert!(r.prereqs.is_empty(), "the shims are on the PATH: {:?}", r.prereqs);
    assert_eq!(r.bandwidth.len(), 2);
    assert!(r.bandwidth.iter().all(|c| c.gbps == Some(92.95) && c.lat_us == Some(2.12) && c.status == Status::Pass), "{:?}", r.bandwidth);
    assert_eq!(r.ping.len(), 4);
    // The server ran detached on b, the client on a against b's fabric address.
    assert_eq!(nb.ran("nohup timeout 30 ib_write_bw -d 'rocep1s0f1' -x 3 -F --report_gbits -D 5 -q 4 >/dev/null 2>&1 & echo $!").len(), 1);
    assert_eq!(na.ran("timeout 30 ib_write_bw -d 'rocep1s0f1' -x 3 -F --report_gbits -D 5 -q 4 '192.168.100.2'").len(), 1);
    assert_eq!(na.ran("timeout 30 ib_send_lat -d 'roceP2p1s0f1' -x 3 -F '192.168.101.2'").len(), 1);
    assert_eq!(na.ran("ping -M do -s 8972 -c 3 -W 1 -I 'enp1s0f1np1' '192.168.100.2'").len(), 1);
    // Every server that started was stopped after its pair (TERM through `timeout`).
    let started = lines(&nb.home.join("ib_write_bw-servers"));
    assert_eq!(started.len(), 2, "one per half");
    for _ in 0..50 {
        if lines(&nb.home.join("ib_write_bw-stopped")).len() == 2 && lines(&nb.home.join("ib_send_lat-stopped")).len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(lines(&nb.home.join("ib_write_bw-stopped")).len(), 2, "servers: {started:?}");
    assert_eq!(lines(&nb.home.join("ib_send_lat-stopped")).len(), 2);
    // Every command is in the activity log.
    let logged: Vec<String> = live.audit.entries().into_iter().map(|e| e.command).collect();
    assert!(logged.iter().any(|c| c.starts_with("timeout 30 ib_write_bw")), "{logged:?}");
}

/// Stop: the running client is ended, its server is stopped, and the rest is skipped.
#[tokio::test]
async fn stopping_the_doctor_kills_the_perftest_server() {
    struct StopAfter(std::time::Instant, Duration);
    impl doctor::Progress for StopAfter {
        fn update(&self, _r: &DoctorReport) {}
        fn cancelled(&self) -> bool {
            self.0.elapsed() >= self.1
        }
    }
    let (_na, nb, live, snap, _dir) = fake_pair("fabric-stop", 20).await;
    let req = DoctorRequest { nodes: vec!["a".into(), "b".into()], checks: Checks { bandwidth: true, ..Default::default() }, ..Default::default() };
    let cancel = crate::live::CancelToken::new();
    let runner = super::commands::LiveRunner { live: live.clone(), cancel: cancel.clone() };
    let stop_in = Duration::from_millis(1500);
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(stop_in).await;
        c2.cancel();
    });
    let report = Mutex::new(DoctorReport::default());
    let t = std::time::Instant::now();
    doctor::run(&runner, &snap, &req, &Baselines::new(), &report, &StopAfter(std::time::Instant::now(), stop_in)).await;
    assert!(t.elapsed() < Duration::from_secs(10), "the 20 s client was stopped: {:?}", t.elapsed());
    let r = report.into_inner().unwrap();
    assert!(r.cancelled, "{r:?}");
    assert!(r.bandwidth.iter().all(|c| c.gbps.is_none()));
    // The first pair's server was started, and stopped.
    for _ in 0..50 {
        if !lines(&nb.home.join("ib_write_bw-stopped")).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(lines(&nb.home.join("ib_write_bw-servers")).len(), 1, "the second half never started");
    assert_eq!(lines(&nb.home.join("ib_write_bw-stopped")).len(), 1);
}

#[test]
fn pfc_off_on_the_real_fabric_is_one_advisory_and_nothing_amber() {
    // The Live snapshot of Oct 29 (health/fixtures/live-idle-2.json): every fabric port at trust
    // pcp with no PFC priority, idle, its counters still holding the out-of-sequence packets RoCE
    // resent while it ran.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/health/fixtures/live-idle-2.json");
    let mut snap: ClusterSnapshot = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mon = FabricMonitor::new(None);
    let mut state = mon.assess(&mut snap, &BTreeMap::new(), None);
    for _ in 0..3 {
        snap.ts_ms += 1000;
        state = mon.assess(&mut snap, &BTreeMap::new(), None);
    }
    let up: Vec<&IfaceHealth> = state.ifaces.iter().filter(|i| i.up).collect();
    assert_eq!(up.len(), 8, "port 2 of each node, both halves");
    assert!(up.iter().all(|i| i.verdict.kind == VerdictKind::NotLossless && i.verdict.level == Level::Info && i.verdict.title == "RoCE priority unprotected"), "{:?}", up.iter().map(|i| (&i.iface, &i.verdict.title, i.verdict.level)).collect::<Vec<_>>());
    assert!(state.ifaces.iter().all(|i| i.verdict.level < Level::Warn), "nothing amber");
    assert_eq!((state.verdict.level, state.verdict.title.as_str()), (Level::Info, "RoCE priority unprotected"));
    assert_eq!(state.summary, "Fabric up · PFC protection unverified · 8 fabric ports");
    let a = state.advisory.expect("one advisory");
    assert_eq!(a.title, "Fabric PFC needs verification.");
    assert!(a.body.contains("83–342 out-of-sequence packets each"));
    assert!(a.body.contains("NCCL_IB_TC=106") && a.body.contains("DSCP 26 mapped to priority 3"));
    assert!(a.body.contains("node-side evidence only"));
    // And the health engine raises nothing for it.
    let h = crate::health::Health::new(None);
    h.set_rates(mon.rates());
    for _ in 0..5 {
        snap.ts_ms += 1000;
        assert!(h.evaluate(&snap).events.is_empty());
    }
}
