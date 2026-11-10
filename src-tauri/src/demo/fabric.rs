//! The simulated RoCE fabric: each fabric half's RoCE and PHY counters, what the
//! fabric scenarios do to them, the CRS804's bridge host and neighbor tables, and
//! the replies the Fabric doctor's commands get (ping, perftest).
//!
//! A healthy fabric under load marks a few packets with ECN and answers them with
//! CNPs (DCQCN at work); nothing is dropped or retransmitted. The scenarios break
//! that in the ways a real RoCE fabric breaks.

use super::world::{fabric_ip, mac, RealNode, World, FABRIC, NCCL_IFACE};
use super::{scenarios, DemoCluster, Role};
use crate::model::RoceCounters;
use crate::routeros::mapping::{HostEntry, Neighbor, PortTables};

/// Priority 3: the traffic class RoCE runs on (NVIDIA's default with PFC).
const ROCE_PRIO: usize = 3;

/// Counters since boot for a node's fabric half. Both worlds keep what the owner's 4-node NCCL
/// run left (out-of-sequence and sequence errors in a ring around the four nodes, a few global
/// pause frames). As found, the port trusts PCP with PFC off and nothing else has moved; the
/// everyday world is tuned for serving (DSCP, PFC on RoCE's priority) and has served a while.
pub(super) fn boot_counters(real: &RealNode, half: usize, world: World) -> RoceCounters {
    let (oos, seq) = real.oos_seq[half];
    if world == World::AsFound {
        return RoceCounters {
            ecn_marked: Some(0),
            cnp_sent: Some(0),
            cnp_handled: Some(0),
            cnp_ignored: Some(0),
            out_of_sequence: Some(oos),
            packet_seq_err: Some(seq),
            local_ack_timeout_err: Some(0),
            implied_nak_seq_err: Some(0),
            rnr_nak_retry_err: Some(0),
            duplicate_request: Some(0),
            rx_icrc_err: Some(0),
            req_cqe_err: Some(0),
            resp_cqe_err: Some(0),
            port_rcv_errors: Some(0),
            port_xmit_discards: Some(0),
            symbol_error: Some(0),
            link_downed: Some(0),
            // mlnx_qos shows no per-priority pause counters without PFC.
            rx_pause: [None; 8],
            tx_pause: [None; 8],
            rx_pause_ctrl: Some(0),
            tx_pause_ctrl: Some(real.tx_pause),
            rx_discards_phy: Some(0),
            rx_crc_errors_phy: Some(0),
            rx_symbol_err_phy: Some(0),
            rx_out_of_buffer: Some(0),
            trust: Some("pcp".into()),
            pfc_prios: Some(Vec::new()),
            rx_prio_bytes: [Some(0); 8],
            roce_v2: Some(true),
        };
    }
    let node = super::world::REAL_NODES.iter().position(|r| r.id == real.id).unwrap_or(0);
    let k = (node as u64 + 1) * 1_000 + half as u64 * 377;
    let mut c = RoceCounters {
        ecn_marked: Some(41_200 + k * 13),
        cnp_sent: Some(39_870 + k * 12),
        cnp_handled: Some(38_115 + k * 11),
        cnp_ignored: Some(0),
        out_of_sequence: Some(oos),
        packet_seq_err: Some(seq),
        local_ack_timeout_err: Some(0),
        implied_nak_seq_err: Some(0),
        rnr_nak_retry_err: Some(0),
        duplicate_request: Some(0),
        rx_icrc_err: Some(0),
        req_cqe_err: Some(0),
        resp_cqe_err: Some(0),
        port_rcv_errors: Some(0),
        port_xmit_discards: Some(0),
        symbol_error: Some(0),
        link_downed: Some(1),
        rx_pause: [Some(0); 8],
        tx_pause: [Some(0); 8],
        rx_pause_ctrl: Some(0),
        tx_pause_ctrl: Some(0),
        rx_discards_phy: Some(0),
        rx_crc_errors_phy: Some(0),
        rx_symbol_err_phy: Some(0),
        rx_out_of_buffer: Some(0),
        trust: Some("dscp".into()),
        pfc_prios: Some(vec![ROCE_PRIO as u8]),
        rx_prio_bytes: [Some(0); 8],
        roce_v2: Some(true),
    };
    c.rx_pause[ROCE_PRIO] = Some(2_400 + k * 3);
    c.tx_pause[ROCE_PRIO] = Some(1_900 + k * 2);
    c.rx_pause_ctrl = c.rx_pause[ROCE_PRIO];
    c.tx_pause_ctrl = c.tx_pause[ROCE_PRIO];
    c
}

fn bump(v: &mut Option<u64>, by: u64) {
    if by > 0 {
        *v = Some(v.unwrap_or(0) + by);
    }
}

/// Adds `n` pause frames sent (`tx`) or received on RoCE's priority.
fn pause(c: &mut RoceCounters, tx: bool, n: u64) {
    if tx {
        bump(&mut c.tx_pause[ROCE_PRIO], n);
        bump(&mut c.tx_pause_ctrl, n);
    } else {
        bump(&mut c.rx_pause[ROCE_PRIO], n);
        bump(&mut c.rx_pause_ctrl, n);
    }
}

/// One second of fabric counters: the everyday trickle under load, then the playing scenario.
pub(super) fn step(d: &mut DemoCluster) {
    let playing = scenarios::playing(d);
    let rng = &mut d.rng;
    for n in d.nodes.iter_mut() {
        if !n.carrier() {
            continue;
        }
        let congested = matches!(playing, Some(("fabric-congestion", ph)) if ph < 90 && ph % 15 < 7) && n.role != Role::Ollama;
        for half in 0..2 {
            if congested {
                // An all-reduce burst: tens of Gb/s each way through the same switch queue.
                let burst = 38e9 + rng.f64() * 30e9;
                n.traffic.rdma_tx[half] += burst;
                n.traffic.rdma_rx[half] += burst * (0.95 + rng.f64() * 0.1);
            }
            let rx_g = n.traffic.rdma_rx[half] / 1e9;
            let tx_g = n.traffic.rdma_tx[half] / 1e9;
            let c = &mut n.roce[half];
            bump(&mut c.rx_prio_bytes[ROCE_PRIO], (rx_g * 1e9 / 8.0) as u64);
            // DCQCN: the switch marks a few packets once a queue builds (over ~2 Gb/s here).
            let marked = if congested {
                rng.u64(4_000..12_000)
            } else if rx_g > 2.0 {
                (rx_g * (0.5 + rng.f64() * 1.5)) as u64
            } else {
                0
            };
            bump(&mut c.ecn_marked, marked);
            bump(&mut c.cnp_sent, marked * 9 / 10);
            let handled = if congested { rng.u64(3_600..10_800) } else if tx_g > 2.0 { (tx_g * (0.4 + rng.f64() * 1.3)) as u64 } else { 0 };
            bump(&mut c.cnp_handled, handled);
            if congested {
                pause(c, true, rng.u64(40..300));
                pause(c, false, rng.u64(40..300));
            } else if rx_g > 2.0 && rng.f64() < 0.2 {
                pause(c, true, rng.u64(1..4));
            }
        }
    }

    match playing {
        Some(("roce-loss", ph)) if ph < 180 => {
            let Some(i) = d.nodes.iter().position(|n| n.cfg.id == "cell2") else { return };
            if !d.nodes[i].carrier() {
                return;
            }
            let crc = d.rng.u64(6..30);
            let c = &mut d.nodes[i].roce[1];
            bump(&mut c.out_of_sequence, d.rng.u64(40..180));
            bump(&mut c.packet_seq_err, d.rng.u64(4..20));
            bump(&mut c.local_ack_timeout_err, d.rng.u64(1..6));
            bump(&mut c.implied_nak_seq_err, d.rng.u64(3..12));
            bump(&mut c.duplicate_request, d.rng.u64(2..8));
            bump(&mut c.rx_crc_errors_phy, crc);
            let sym = d.rng.u64(2..12);
            bump(&mut c.rx_symbol_err_phy, sym);
            bump(&mut c.symbol_error, sym);
            bump(&mut c.port_rcv_errors, crc);
            // Retransmits cost throughput on that half.
            d.nodes[i].traffic.rdma_tx[1] *= 0.7;
            if let Some(p) = d.switches.iter_mut().flat_map(|s| s.ports.iter_mut()).find(|p| p.node == Some(i)) {
                p.fcs_errors += crc / 2;
                p.rx_errors += crc / 2;
            }
        }
        Some(("pfc-storm", ph)) if ph < 90 => {
            let Some(i) = d.nodes.iter().position(|n| n.cfg.id == "gx10-0000") else { return };
            if !d.nodes[i].carrier() {
                return;
            }
            let frames = 50_000 + d.rng.u64(0..3_000) - 1_500;
            pause(&mut d.nodes[i].roce[0], true, frames);
            if let Some(p) = d.switches.iter_mut().flat_map(|s| s.ports.iter_mut()).find(|p| p.node == Some(i)) {
                p.rx_pause += frames;
            }
        }
        _ => {}
    }

    // The switch counts the pause frames its nodes send it.
    for s in d.switches.iter_mut() {
        for p in s.ports.iter_mut() {
            if let Some(n) = p.node.and_then(|i| d.nodes.get(i)) {
                if n.carrier() && n.traffic.rdma_rx[0] > 2e9 && d.rng.f64() < 0.2 {
                    p.rx_pause += d.rng.u64(1..4);
                }
            }
        }
    }
}

/// The link went down: the RDMA port counts it on both halves.
pub(super) fn link_downed(c: &mut [RoceCounters; 2]) {
    for h in c.iter_mut() {
        bump(&mut h.link_downed, 1);
    }
}

/// The CRS804's bridge host table (which MAC is behind which port) and its LLDP/MNDP
/// neighbors, built from the simulated cabling the way RouterOS would report them.
pub fn switch_tables(d: &DemoCluster, switch_id: &str) -> PortTables {
    let Some(sw) = d.switches.iter().find(|s| s.cfg.id == switch_id) else { return PortTables::default() };
    let mut t = PortTables::default();
    for p in &sw.ports {
        if !p.running_was {
            continue;
        }
        if p.uplink {
            // The Firewalla Switch X and, behind it, the Gold: the switch's own management path.
            t.hosts.push(HostEntry { mac: "20:6d:31:8b:2f:b4".into(), port: p.name.clone(), dynamic: true, local: false });
            t.hosts.push(HostEntry { mac: "20:6d:31:c4:59:8c".into(), port: p.name.clone(), dynamic: true, local: false });
            t.neighbors.push(Neighbor {
                port: p.name.clone(),
                identity: "Firewalla Switch X".into(),
                mac: Some("20:6d:31:8b:2f:b4".into()),
                address: Some("192.168.50.3".into()),
                platform: Some("Firewalla".into()),
                discovered_by: "lldp".into(),
            });
            continue;
        }
        let Some(n) = p.node.and_then(|i| d.nodes.get(i)) else { continue };
        if !n.carrier() {
            continue;
        }
        for (iface, _, _) in FABRIC {
            t.hosts.push(HostEntry { mac: mac(&n.cfg.id, iface).to_uppercase(), port: p.name.clone(), dynamic: true, local: false });
        }
        // DGX OS runs lldpd: the neighbor is the node's hostname, seen from its first fabric netdev.
        t.neighbors.push(Neighbor {
            port: p.name.clone(),
            identity: n.cfg.host.trim_end_matches(".local").into(),
            mac: Some(mac(&n.cfg.id, NCCL_IFACE).to_uppercase()),
            address: Some(fabric_ip(n.idx, 0)),
            // lldpd reports the running kernel, so a node that's behind on updates shows it here too.
            platform: Some(format!("Linux {}", n.kernel)),
            discovered_by: "lldp".into(),
        });
    }
    t
}

/// What the Fabric doctor's commands print on a simulated node, or None for anything else.
pub(super) fn reply(d: &DemoCluster, node: usize, cmd: &str) -> Option<(u32, String, String)> {
    let n = &d.nodes[node];
    let ok = |s: String| Some((0, s, String::new()));
    if cmd.starts_with("command -v ") {
        // "command -v a; command -v b": one lookup per tool, as the doctor sends them.
        let names: Vec<&str> = cmd.split(';').filter_map(|part| part.trim().strip_prefix("command -v ")).flat_map(|r| r.split_whitespace()).collect();
        let found: String = names
            .iter()
            .map(|b| b.trim_matches('\''))
            .filter(|b| n.perftest || !b.starts_with("ib_"))
            // nccl-tests was built on the Ray nodes, next to their MPI.
            .filter(|b| n.role != Role::Ollama || !matches!(*b, "all_reduce_perf" | "mpirun"))
            .filter(|b| matches!(*b, "ib_write_bw" | "ib_send_lat" | "ping" | "ethtool" | "ibv_devinfo" | "all_reduce_perf" | "mpirun"))
            .map(|b| if b == "all_reduce_perf" { "/opt/nccl-tests/build/all_reduce_perf\n".to_string() } else { format!("/usr/bin/{b}\n") })
            .collect();
        return Some((if found.lines().count() == names.len() { 0 } else { 1 }, found, String::new()));
    }
    if cmd.starts_with("ping ") {
        return Some(ping(d, node, cmd));
    }
    if cmd.trim_start_matches('\'').starts_with("mpirun") {
        return Some(all_reduce(d, cmd));
    }
    if cmd.contains("ib_write_bw") || cmd.contains("ib_send_lat") {
        if !n.perftest {
            let bin = if cmd.contains("ib_write_bw") { "ib_write_bw" } else { "ib_send_lat" };
            return Some((127, String::new(), format!("bash: line 1: {bin}: command not found\n")));
        }
        // The server, started in the background: its PID.
        if cmd.contains("nohup ") {
            return ok(format!("{}\n", 40_000 + d.tick % 9_000 + node as u64 * 13));
        }
        return Some(perftest(d, node, cmd));
    }
    if cmd.starts_with("kill ") || cmd.starts_with("pkill ") {
        return ok(String::new());
    }
    None
}

/// The half a fabric address is on, and the node index it belongs to.
fn peer_of(d: &DemoCluster, ip: &str) -> Option<(usize, usize)> {
    (0..d.nodes.len()).flat_map(|i| (0..2).map(move |h| (i, h))).find(|&(i, h)| fabric_ip(d.nodes[i].idx, h) == ip)
}

/// The word after `flag`, without the shell quotes SparkBox puts around every argument.
fn arg_after<'a>(cmd: &'a str, flag: &str) -> Option<&'a str> {
    let mut it = cmd.split_whitespace();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().map(|v| v.trim_matches('\''));
        }
    }
    None
}

fn last_arg(cmd: &str) -> &str {
    cmd.split_whitespace().last().unwrap_or("").trim_matches('\'')
}

fn mtu_of(d: &DemoCluster, node: usize, half: usize) -> u32 {
    d.nodes[node].ov.mtu[half].unwrap_or(super::world::FABRIC_MTU)
}

/// `ping -M do -s <size> -c <n> -W 1 -I <iface> <peer>`, as iputils prints it.
fn ping(d: &DemoCluster, node: usize, cmd: &str) -> (u32, String, String) {
    let size: u32 = arg_after(cmd, "-s").and_then(|s| s.parse().ok()).unwrap_or(56);
    let count: u32 = arg_after(cmd, "-c").and_then(|s| s.parse().ok()).unwrap_or(3);
    let iface = arg_after(cmd, "-I").unwrap_or(NCCL_IFACE);
    let peer = last_arg(cmd);
    let half = FABRIC.iter().position(|(i, _, _)| *i == iface).unwrap_or(0);
    let src = fabric_ip(d.nodes[node].idx, half);
    let head = format!("PING {peer} ({peer}) from {src} {iface}: {size}({}) bytes of data.\n", size + 28);
    let local_mtu = mtu_of(d, node, half);
    if size + 28 > local_mtu {
        let err = format!("ping: local error: message too long, mtu={local_mtu}\n").repeat(count as usize);
        let tail = format!("\n--- {peer} ping statistics ---\n{count} packets transmitted, 0 received, +{count} errors, 100% packet loss, time {}ms\n", (count - 1) * 1000 + 4);
        return (1, head + &tail, err);
    }
    let reachable = peer_of(d, peer).is_some_and(|(i, h)| {
        let p = &d.nodes[i];
        h == half && p.carrier() && d.nodes[node].carrier() && size + 28 <= mtu_of(d, i, h)
    });
    if !reachable {
        let tail = format!("\n--- {peer} ping statistics ---\n{count} packets transmitted, 0 received, 100% packet loss, time {}ms\n", (count - 1) * 1000 + 7);
        return (1, head + &tail, String::new());
    }
    let mut rng = fastrand::Rng::with_seed(d.tick ^ (node as u64) << 8);
    let base = 0.09 + size as f64 / 9000.0 * 0.05;
    let times: Vec<f64> = (0..count).map(|_| base + rng.f64() * 0.04).collect();
    let mut out = head;
    for (seq, t) in times.iter().enumerate() {
        out += &format!("{} bytes from {peer}: icmp_seq={} ttl=64 time={t:.3} ms\n", size + 8, seq + 1);
    }
    let (min, max) = times.iter().fold((f64::MAX, 0.0f64), |(a, b), &t| (a.min(t), b.max(t)));
    let avg = times.iter().sum::<f64>() / times.len() as f64;
    out += &format!(
        "\n--- {peer} ping statistics ---\n{count} packets transmitted, {count} received, 0% packet loss, time {}ms\nrtt min/avg/max/mdev = {min:.3}/{avg:.3}/{max:.3}/{:.3} ms\n",
        (count - 1) * 1000 + 2,
        (max - min) / 2.0
    );
    (0, out, String::new())
}

/// The bandwidth a half really delivers between two nodes in the playing scenario, Gb/s.
fn half_gbps(d: &DemoCluster, a: usize, b: usize, half: usize, rng: &mut fastrand::Rng) -> f64 {
    let mut bw = if half == 0 { 93.0 + rng.f64() * 3.0 } else { 92.0 + rng.f64() * 3.0 };
    let degraded = |id: &str| d.nodes[a].cfg.id == id || d.nodes[b].cfg.id == id;
    match scenarios::playing(d) {
        Some(("roce-loss", ph)) if ph < 180 && half == 1 && degraded("cell2") => bw = 58.0 + rng.f64() * 6.0,
        Some(("optic-degraded", ph)) if (30..180).contains(&ph) && degraded("cell2") => bw = 71.0 + rng.f64() * 5.0,
        Some(("fabric-congestion", ph)) if ph < 90 => bw *= 0.82 + rng.f64() * 0.06,
        _ => {}
    }
    bw
}

/// `ib_write_bw` / `ib_send_lat` client output, as perftest 24.x prints it.
fn perftest(d: &DemoCluster, node: usize, cmd: &str) -> (u32, String, String) {
    let dev = arg_after(cmd, "-d").unwrap_or("rocep1s0f1");
    let half = FABRIC.iter().position(|(_, _, r)| *r == dev).unwrap_or(0);
    let peer_ip = last_arg(cmd);
    let Some((peer, peer_half)) = peer_of(d, peer_ip) else {
        return (1, String::new(), format!(" Couldn't connect to {peer_ip}:18515\nUnable to open file descriptor for socket connection Unable to init the socket connection\n"));
    };
    if peer_half != half || !d.nodes[peer].carrier() || !d.nodes[node].carrier() || !d.nodes[peer].perftest {
        return (1, String::new(), format!(" Couldn't connect to {peer_ip}:18515\nUnable to open file descriptor for socket connection Unable to init the socket connection\n"));
    }
    let mut rng = fastrand::Rng::with_seed(d.tick.wrapping_mul(31) ^ ((node * 4 + peer) as u64) ^ ((half as u64) << 12));
    let mtu = mtu_of(d, node, half).min(mtu_of(d, peer, half)).min(4096);
    let mtu = [256, 512, 1024, 2048, 4096].into_iter().rfind(|m| *m <= mtu).unwrap_or(1024);
    let src = fabric_ip(d.nodes[node].idx, half);
    let dst = fabric_ip(d.nodes[peer].idx, half);
    let addr = |ip: &str, lid: u32, qpn: u32, psn: u32| {
        let gid: String = ip.split('.').map(|o| format!("{:03}", o.parse::<u32>().unwrap_or(0))).collect::<Vec<_>>().join(":");
        format!(" local address: LID 0000 QPN 0x{qpn:04x} PSN 0x{psn:06x} RKey 0x{lid:06x} VAddr 0x00ffff{qpn:06x}\n GID: 00:00:00:00:00:00:00:00:00:00:255:255:{gid}\n")
    };
    if cmd.contains("ib_send_lat") {
        let typical = 2.1 + rng.f64() * 0.5 + if half == 1 { 0.05 } else { 0.0 };
        let (min, max) = (typical - 0.18, typical + 3.9 + rng.f64() * 2.0);
        let out = format!(
            "---------------------------------------------------------------------------------------\n                    Send Latency Test\n Dual-port       : OFF\t\tDevice         : {dev}\n Number of qps   : 1\t\tTransport type : IB\n Connection type : RC\t\tUsing SRQ      : OFF\n PCIe relax order: ON\n TX depth        : 1\n Mtu             : {mtu}[B]\n Link type       : Ethernet\n GID index       : 3\n Max inline data : 236[B]\n rdma_cm QPs\t : OFF\n Data ex. method : Ethernet\n---------------------------------------------------------------------------------------\n{}---------------------------------------------------------------------------------------\n #bytes #iterations    t_min[usec]    t_max[usec]  t_typical[usec]    t_avg[usec]    t_stdev[usec]   99% percentile[usec]   99.9% percentile[usec] \n 2       1000          {min:.2}           {max:.2}         {typical:.2}     \t{:.2}       \t{:.2}   \t\t{:.2}   \t\t{:.2}   \n---------------------------------------------------------------------------------------\n",
            addr(&src, 0, 0x0108 + node as u32, 0x3a1f00 + rng.u32(0..255)) + &addr(&dst, 0, 0x0109 + peer as u32, 0x51c200 + rng.u32(0..255)).replace("local", "remote"),
            typical + 0.06,
            0.21 + rng.f64() * 0.1,
            typical + 0.9,
            max - 0.4,
        );
        return (0, out, String::new());
    }
    let bw = half_gbps(d, node, peer, half, &mut rng);
    let peak = bw + 0.4 + rng.f64() * 0.6;
    let mut qps = String::new();
    for q in 0..4u32 {
        qps += &addr(&src, 0, 0x0110 + q, 0x2b0000 + rng.u32(0..0xffff));
    }
    let mut remote = String::new();
    for q in 0..4u32 {
        remote += &addr(&dst, 0, 0x0120 + q, 0x6c0000 + rng.u32(0..0xffff)).replace("local", "remote");
    }
    let out = format!(
        "---------------------------------------------------------------------------------------\n                    RDMA_Write BW Test\n Dual-port       : OFF\t\tDevice         : {dev}\n Number of qps   : 4\t\tTransport type : IB\n Connection type : RC\t\tUsing SRQ      : OFF\n PCIe relax order: ON\n ibv_wr* API     : ON\n TX depth        : 128\n CQ Moderation   : 1\n Mtu             : {mtu}[B]\n Link type       : Ethernet\n GID index       : 3\n Max inline data : 0[B]\n rdma_cm QPs\t : OFF\n Data ex. method : Ethernet\n---------------------------------------------------------------------------------------\n{qps}{remote}---------------------------------------------------------------------------------------\n #bytes     #iterations    BW peak[Gb/sec]    BW average[Gb/sec]   MsgRate[Mpps]\n 65536      {}          {peak:.2}             {bw:.2}  \t\t   {:.6}\n---------------------------------------------------------------------------------------\n",
        (bw * 1e9 * 5.0 / 8.0 / 65536.0) as u64,
        bw * 1e9 / 8.0 / 65536.0 / 1e6,
    );
    (0, out, String::new())
}

/// `mpirun … all_reduce_perf -b 1M -e 8G -f 2 -g 1`, as nccl-tests prints it.
fn all_reduce(d: &DemoCluster, cmd: &str) -> (u32, String, String) {
    let hosts: Vec<usize> = arg_after(cmd, "-H")
        .unwrap_or("")
        .split(',')
        .filter_map(|h| peer_of(d, h.split(':').next().unwrap_or("")).map(|(i, _)| i))
        .collect();
    if let Some(&i) = hosts.iter().find(|&&i| d.nodes[i].role == Role::Ollama || !d.nodes[i].carrier()) {
        let n = &d.nodes[i];
        return (
            1,
            String::new(),
            format!("--------------------------------------------------------------------------\nmpirun was unable to launch the specified application as it could not access\nor execute an executable:\n\nExecutable: all_reduce_perf\nNode: {}\n--------------------------------------------------------------------------\n", n.cfg.host.trim_end_matches(".local")),
        );
    }
    let mut rng = fastrand::Rng::with_seed(d.tick ^ 0xA11);
    let degraded = matches!(scenarios::playing(d), Some(("roce-loss", ph)) if ph < 180) && hosts.iter().any(|&i| d.nodes[i].cfg.id == "cell2");
    let peak = if degraded { 7.9 } else { 12.2 } + rng.f64() * 0.4;
    let mut out = String::from("# nThread 1 nGpus 1 minBytes 1048576 maxBytes 8589934592 step: 2(factor) warmup iters: 5 iters: 20 agg iters: 1 validation: 1 graph: 0\n#\n# Using devices\n");
    for (rank, &i) in hosts.iter().enumerate() {
        out += &format!("#  Rank {rank:2} Group  0 Pid {:6} on {:>10} device  0 [0x01] NVIDIA GB10\n", 41_000 + i * 977, d.nodes[i].cfg.host.trim_end_matches(".local"));
    }
    out += "#\n#                                                              out-of-place                       in-place          \n#       size         count      type   redop    root     time   algbw   busbw #wrong     time   algbw   busbw #wrong\n#        (B)    (elements)                               (us)  (GB/s)  (GB/s)            (us)  (GB/s)  (GB/s)       \n";
    let ranks = hosts.len().max(2) as f64;
    let mut sum = 0.0;
    let mut rows = 0.0;
    for size in [1u64 << 20, 1 << 24, 1 << 28, 1 << 31, 1 << 33] {
        // Small messages are latency-bound; the bus fills up from a few hundred MB.
        let frac = (size as f64 / 2.7e8).min(1.0).powf(0.35);
        let bus = peak * frac;
        let alg = bus / (2.0 * (ranks - 1.0) / ranks);
        let us = size as f64 / (alg * 1e3);
        out += &format!("{size:>12} {:>13}     float     sum      -1 {us:8.1} {alg:7.2} {bus:7.2}      0 {:8.1} {alg:7.2} {bus:7.2}      0\n", size / 4, us * 0.998);
        sum += bus;
        rows += 1.0;
    }
    out += &format!("# Out of bounds values : 0 OK\n# Avg bus bandwidth    : {:.4} \n#\n", sum / rows);
    (0, out, String::new())
}

impl DemoCluster {
    /// The CRS804's bridge host and neighbor tables (what the Switch window's Neighbors tab shows).
    pub fn switch_tables(&self, switch_id: &str) -> PortTables {
        switch_tables(self, switch_id)
    }

    /// `apt-get install perftest` on a simulated node.
    pub fn install_perftest(&mut self, node: &str) -> Result<(), String> {
        let i = self.node_index(node)?;
        self.nodes[i].perftest = true;
        Ok(())
    }
}
