//! Demo's Switch X: sample labels and a sample cloud answer, with one port in every state the
//! panel can show. Port 2 is the one the demo nodes hear over LLDP, and the cloud knows a link
//! there but no speed, so the panel shows the neutral light. Nothing here reaches the real file.

use std::collections::BTreeMap;

use super::labels::{LabelKind, Labels, PortLabel};
use super::msp::{MspPort, MspSfp, MspSwitch, POLL_EVERY_MS};
use super::MspStatus;

/// The sample read this long ago, so the panel's ages read like a real poll's.
const LAST_READ_MS: u64 = 4 * 60_000;
/// When the sample counters started (Unix seconds): 2 Oct 2026.
const STATS_SINCE_S: u64 = 1_791_000_000;

fn label(kind: LabelKind, node: Option<&str>, text: Option<&str>) -> PortLabel {
    PortLabel { kind, node: node.map(String::from), text: text.map(String::from), set_at_ms: 0 }
}

pub fn sample_labels() -> Labels {
    let ports: BTreeMap<u32, PortLabel> = [
        (1, label(LabelKind::Gold, None, None)),
        (3, label(LabelKind::Mac, None, None)),
        (4, label(LabelKind::Text, None, Some("Wi-Fi access point"))),
        (5, label(LabelKind::Node, Some("gx10-0000"), None)),
        (6, label(LabelKind::Text, None, Some("Printer"))),
        (7, label(LabelKind::Empty, None, None)),
        (9, label(LabelKind::Crs804, None, None)),
    ]
    .into();
    [("firewalla".to_string(), ports)].into()
}

fn up(port: u32, mbps: Option<u32>, rx: u64, tx: u64) -> MspPort {
    MspPort {
        port,
        link_up: Some(true),
        link_speed_mbps: mbps,
        rx_bytes: Some(rx),
        tx_bytes: Some(tx),
        rx_errors: Some(0),
        tx_errors: Some(0),
        rx_discards: Some(0),
        tx_discards: Some(0),
        stats_since_s: Some(STATS_SINCE_S),
        stp_state: Some("forwarding".into()),
        stp_role: Some("designated".into()),
        port_type: Some("trunk".into()),
        ..Default::default()
    }
}

fn down(port: u32) -> MspPort {
    MspPort { port, link_up: Some(false), stats_since_s: Some(STATS_SINCE_S), stp_state: Some("discarding".into()), stp_role: Some("disabled".into()), ..Default::default() }
}

fn sfp(port: MspPort, present: bool) -> MspPort {
    let (connector, bit_rate) = if present { ("LC", "10.3 Gbps") } else { ("N/A", "N/A") };
    MspPort { sfp: Some(MspSfp { present: Some(present), connector: Some(connector.into()), bit_rate: Some(bit_rate.into()), vendor: None }), ..port }
}

fn sample_switch() -> MspSwitch {
    const GB: u64 = 1_000_000_000;
    let poe = MspPort { poe_status: Some("delivering".into()), poe_mode: Some("802.3at".into()), poe_watts: Some(9.3), ..up(4, Some(2500), 84 * GB, 412 * GB) };
    let ports = vec![
        MspPort { uplink: true, ..up(1, Some(10_000), 1_930 * GB, 2_480 * GB) },
        up(2, None, 2_210 * GB, 1_620 * GB),
        up(3, Some(5_000), 310 * GB, 945 * GB),
        poe,
        up(5, Some(1_000), 126 * GB, 208 * GB),
        up(6, Some(100), 2 * GB, 11 * GB),
        down(7),
        down(8),
        sfp(up(9, Some(10_000), 640 * GB, 610 * GB), true),
        sfp(down(10), false),
        sfp(down(11), false),
        sfp(down(12), false),
    ];
    MspSwitch {
        mac: Some("20:6d:31:be:00:15".into()),
        name: Some("Firewalla Switch X".into()),
        ip: None,
        model: None,
        version: None,
        online: Some(true),
        ports,
        skipped: 0,
    }
}

pub fn sample_status(now_ms: u64) -> MspStatus {
    let read = now_ms.saturating_sub(LAST_READ_MS);
    MspStatus {
        configured: true,
        domain: Some("dn-demo.firewalla.net".into()),
        state: "ok".into(),
        message: None,
        last_ok_ms: Some(read),
        last_try_ms: Some(read),
        next_poll_ms: Some(read + POLL_EVERY_MS),
        sw: Some(sample_switch()),
        held: false,
    }
}
