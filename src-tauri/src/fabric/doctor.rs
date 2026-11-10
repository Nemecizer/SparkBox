//! The Fabric doctor ("Test fabric"): an MTU audit, a jumbo-frame ping matrix, and
//! perftest bandwidth and latency between every pair of the chosen nodes on each
//! fabric half, optionally an nccl-tests all-reduce across them. Results are
//! compared with what a GB10 half delivers (92 Gb/s by default) and with the
//! baselines saved from the first all-pass run (fabric_baselines.json).
//!
//! Every command goes through a `Runner`: SSH in Live (each one in the activity log),
//! the simulator in Demo (which answers with real tool output, so the same parsers
//! run). A perftest server is started detached under `timeout 30`; its PID is kept
//! so a cancel, or the end of the pair, kills it.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::parse::{self, Ping};
use crate::live::sh_quote;
use crate::model::{ClusterSnapshot, LinkStatus, NetRole};

/// What a GB10 fabric half delivers with ib_write_bw (PCIe Gen5 x4 per half).
pub const DEFAULT_EXPECTED_GBPS: f64 = 92.0;
/// Below this share of expected (or of the baseline) a pair fails.
const PASS_SHARE: f64 = 0.85;
pub const JUMBO_PAYLOAD: u32 = 8972;
pub const FABRIC_MTU: u32 = 9000;
const PERFTEST_PORT: u16 = 18515;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Checks {
    pub mtu: bool,
    pub ping: bool,
    pub bandwidth: bool,
    pub latency: bool,
    pub nccl: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DoctorRequest {
    pub nodes: Vec<String>,
    pub checks: Checks,
    /// Gb/s per half; 92 when not set.
    pub expected_gbps: Option<f64>,
    /// nccl-tests binaries, when not on the PATH.
    pub all_reduce_perf: Option<String>,
    pub mpirun: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skipped,
    Running,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prereq {
    pub node: String,
    /// Tools that aren't installed, e.g. ["ib_write_bw", "ib_send_lat"].
    pub missing: Vec<String>,
    /// "perftest isn't installed on cell2"
    pub message: String,
    /// The package that brings them (installable from the window).
    pub package: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MtuRow {
    pub node: String,
    pub iface: String,
    pub half: u8,
    pub mtu: Option<u32>,
    pub switch_port: Option<String>,
    pub switch_mtu: Option<u32>,
    pub status: Status,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PingCell {
    pub from: String,
    pub to: String,
    pub half: u8,
    pub status: Status,
    /// "ok", "fails: MTU" or "unreachable".
    pub result: String,
    pub rtt_ms: Option<f64>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BwCell {
    pub from: String,
    pub to: String,
    pub half: u8,
    pub gbps: Option<f64>,
    pub lat_us: Option<f64>,
    pub expected_gbps: f64,
    pub baseline_gbps: Option<f64>,
    pub status: Status,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NcclResult {
    pub nodes: Vec<String>,
    pub busbw_gbps: Option<f64>,
    pub expected_gbps: f64,
    pub status: Status,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub task_id: Option<String>,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub demo: bool,
    pub nodes: Vec<String>,
    pub checks: Checks,
    pub expected_gbps: f64,
    pub prereqs: Vec<Prereq>,
    pub mtu: Vec<MtuRow>,
    pub ping: Vec<PingCell>,
    pub bandwidth: Vec<BwCell>,
    pub nccl: Option<NcclResult>,
    /// One line per finding, worst first ("cell2→cell3 half 2: 61 Gb/s, 34% below its baseline of 93 Gb/s").
    pub findings: Vec<String>,
    pub baseline_saved: bool,
    pub cancelled: bool,
    /// What's running now, e.g. "ib_write_bw cell1 → cell2, half 1".
    pub current: Option<String>,
}

/// A pair's saved result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub gbps: f64,
    pub lat_us: Option<f64>,
    pub ts_ms: u64,
}

/// "cell1>cell2/1" → baseline, in fabric_baselines.json.
pub type Baselines = BTreeMap<String, Baseline>;

pub fn pair_key(from: &str, to: &str, half: u8) -> String {
    format!("{from}>{to}/{}", half + 1)
}

pub struct Out {
    pub code: Option<u32>,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn text(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Runs a command on a node: SSH in Live, the simulator in Demo.
pub trait Runner: Send + Sync {
    /// `stoppable`: a cancel stops it (the perftest client); cleanup runs with false.
    fn run<'a>(&'a self, node: &'a str, cmd: String, limit: Duration, stoppable: bool) -> BoxFut<'a, Result<Out, String>>;
    /// Time passing between steps (Demo paces itself so progress can be followed).
    fn pause<'a>(&'a self, _what: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async {})
    }
}

/// A node's cabled fabric half, as the doctor needs it.
#[derive(Clone, Debug)]
pub struct Port {
    pub node: String,
    pub name: String,
    pub half: u8,
    pub iface: String,
    pub ip: String,
    pub dev: Option<String>,
    pub mtu: Option<u32>,
    pub switch_port: Option<String>,
    pub switch_mtu: Option<u32>,
}

/// The chosen nodes' addressed fabric halves, from the latest snapshot.
pub fn ports(snap: &ClusterSnapshot, nodes: &[String]) -> Vec<Port> {
    let mut out = Vec::new();
    for n in snap.nodes.iter().filter(|n| nodes.contains(&n.id)) {
        let fabric: Vec<&crate::model::NetStats> = n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.ip.is_some()).collect();
        for x in &fabric {
            let switch_mtu = x.switch_port.as_deref().and_then(|sp| {
                let (sw, port) = sp.split_once('/')?;
                snap.switches.iter().find(|s| s.id == sw)?.ports.iter().find(|p| p.name == port)?.mtu
            });
            out.push(Port {
                node: n.id.clone(),
                name: n.name.clone(),
                half: super::half_of(x, &fabric),
                iface: x.iface.clone(),
                ip: x.ip.clone().unwrap_or_default().split('/').next().unwrap_or("").to_string(),
                dev: x.rdma_dev.clone(),
                mtu: x.mtu,
                switch_port: x.switch_port.as_deref().map(|s| s.split('/').nth(1).unwrap_or(s).to_string()),
                switch_mtu,
            });
        }
    }
    out
}

/// The commands, exactly as sent. Interface, device and address are validated before use.
pub fn ping_cmd(iface: &str, peer: &str) -> String {
    format!("ping -M do -s {JUMBO_PAYLOAD} -c 3 -W 1 -I {} {}", sh_quote(iface), sh_quote(peer))
}

pub fn small_ping_cmd(iface: &str, peer: &str) -> String {
    format!("ping -c 2 -W 1 -I {} {}", sh_quote(iface), sh_quote(peer))
}

pub fn bw_server_cmd(dev: &str) -> String {
    format!("nohup timeout 30 ib_write_bw -d {} -x 3 -F --report_gbits -D 5 -q 4 >/dev/null 2>&1 & echo $!", sh_quote(dev))
}

pub fn bw_client_cmd(dev: &str, peer: &str) -> String {
    format!("timeout 30 ib_write_bw -d {} -x 3 -F --report_gbits -D 5 -q 4 {}", sh_quote(dev), sh_quote(peer))
}

pub fn lat_server_cmd(dev: &str) -> String {
    format!("nohup timeout 30 ib_send_lat -d {} -x 3 -F >/dev/null 2>&1 & echo $!", sh_quote(dev))
}

pub fn lat_client_cmd(dev: &str, peer: &str) -> String {
    format!("timeout 30 ib_send_lat -d {} -x 3 -F {}", sh_quote(dev), sh_quote(peer))
}

/// One `command -v` per tool: dash (Ubuntu's /bin/sh) answers only for the first name given.
pub const PREREQ_CMD: &str = "command -v ib_write_bw; command -v ib_send_lat";
pub const INSTALL_CMD: &str = "apt-get install -y perftest";

/// Interface names, RDMA devices and IPv4 addresses only: nothing else reaches a command line.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

pub fn valid_ip(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>().is_ok()
}

/// What the doctor is told along the way.
pub trait Progress: Send + Sync {
    fn update(&self, report: &DoctorReport);
    fn cancelled(&self) -> bool;
    fn step(&self, _label: &str) {}
}

fn grade(gbps: f64, expected: f64, baseline: Option<f64>) -> (Status, String) {
    if gbps < expected * PASS_SHARE {
        return (Status::Fail, format!("{gbps:.0} Gb/s, below 85% of the expected {expected:.0} Gb/s"));
    }
    if let Some(b) = baseline {
        if gbps < b * PASS_SHARE {
            return (Status::Warn, format!("{gbps:.0} Gb/s, {:.0}% below its baseline of {b:.0} Gb/s", (1.0 - gbps / b) * 100.0));
        }
    }
    (Status::Pass, format!("{gbps:.0} Gb/s"))
}

/// Runs the doctor. `report` is filled in as it goes (and passed to `progress`), so the
/// window shows each cell as it lands.
pub async fn run(runner: &dyn Runner, snap: &ClusterSnapshot, req: &DoctorRequest, baselines: &Baselines, report: &Mutex<DoctorReport>, progress: &dyn Progress) {
    let expected = req.expected_gbps.filter(|g| *g > 0.0).unwrap_or(DEFAULT_EXPECTED_GBPS);
    let online: Vec<String> = req.nodes.iter().filter(|id| snap.nodes.iter().any(|n| &n.id == *id && n.status == LinkStatus::Online)).cloned().collect();
    let all = ports(snap, &online);
    let name = |id: &str| snap.nodes.iter().find(|n| n.id == id).map(|n| n.name.clone()).unwrap_or_else(|| id.to_string());
    let push = |f: &dyn Fn(&mut DoctorReport)| {
        let mut r = report.lock().unwrap();
        f(&mut r);
        progress.update(&r);
    };
    for id in req.nodes.iter().filter(|id| !online.contains(id)) {
        let n = name(id);
        push(&|r| r.findings.push(format!("{n} isn't connected; it was left out.")));
    }

    // MTU audit: every addressed fabric interface at 9000, its switch port at least that, all alike.
    if req.checks.mtu {
        progress.step("MTU audit");
        for p in &all {
            let (status, message) = match (p.mtu, p.switch_mtu) {
                (None, _) => (Status::Warn, "MTU not reported".to_string()),
                (Some(m), _) if m != FABRIC_MTU => (Status::Fail, format!("MTU {m}; the fabric uses {FABRIC_MTU}")),
                (Some(m), Some(s)) if s < m => (Status::Fail, format!("Switch port {} allows {s}, less than {m}", p.switch_port.as_deref().map(super::short_port).unwrap_or_default())),
                (Some(m), _) => (Status::Pass, format!("{m}")),
            };
            let row = MtuRow { node: p.node.clone(), iface: p.iface.clone(), half: p.half, mtu: p.mtu, switch_port: p.switch_port.clone(), switch_mtu: p.switch_mtu, status: status.clone(), message: message.clone() };
            let f = (status == Status::Fail).then(|| format!("{} half {}: {message}", p.name, p.half + 1));
            push(&|r| {
                r.mtu.push(row.clone());
                if let Some(f) = &f {
                    r.findings.push(f.clone());
                }
            });
        }
        runner.pause("mtu").await;
    }

    // Jumbo ping matrix, both directions, per half.
    if req.checks.ping {
        progress.step("Jumbo-frame ping");
        for a in &all {
            for b in all.iter().filter(|b| b.node != a.node && b.half == a.half) {
                if progress.cancelled() {
                    return finish_cancelled(report, progress);
                }
                if !(valid_name(&a.iface) && valid_ip(&b.ip)) {
                    continue;
                }
                let out = runner.run(&a.node, ping_cmd(&a.iface, &b.ip), Duration::from_secs(10), true).await;
                let (status, result, rtt, detail) = match out.as_ref().map(|o| parse::ping(&o.text())) {
                    Ok(Ping::Ok { rtt_ms }) => (Status::Pass, "ok".to_string(), Some(rtt_ms), None),
                    Ok(Ping::TooBig { mtu }) => (Status::Fail, "fails: MTU".into(), None, Some(match mtu {
                        Some(m) => format!("{} can't send 9000-byte frames (MTU {m} on the path)", a.name),
                        None => "The frame is too big for the path".into(),
                    })),
                    Ok(Ping::Lost) => {
                        // Small packets get through: something on the path drops jumbo frames.
                        let small = runner.run(&a.node, small_ping_cmd(&a.iface, &b.ip), Duration::from_secs(8), true).await;
                        match small.as_ref().map(|o| parse::ping(&o.text())) {
                            Ok(Ping::Ok { .. }) => (Status::Fail, "fails: MTU".into(), None, Some(format!("Small packets reach {} but 9000-byte frames don't", b.name))),
                            _ => (Status::Fail, "unreachable".into(), None, Some(format!("No reply from {} on half {}", b.name, a.half + 1))),
                        }
                    }
                    Err(e) => (Status::Fail, "unreachable".into(), None, Some(e.clone())),
                };
                let cell = PingCell { from: a.node.clone(), to: b.node.clone(), half: a.half, status: status.clone(), result: result.clone(), rtt_ms: rtt, detail: detail.clone() };
                let f = (status == Status::Fail).then(|| format!("{}→{} half {}: {result}{}", a.name, b.name, a.half + 1, detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default()));
                push(&|r| {
                    r.ping.push(cell.clone());
                    if let Some(f) = &f {
                        r.findings.push(f.clone());
                    }
                });
                runner.pause("ping").await;
            }
        }
    }

    // perftest: is it there?
    let mut usable: Vec<String> = online.clone();
    if req.checks.bandwidth || req.checks.latency {
        progress.step("Checking for perftest");
        for id in &online {
            let out = runner.run(id, PREREQ_CMD.into(), Duration::from_secs(10), false).await;
            let found = out.as_ref().map(|o| o.stdout.clone()).unwrap_or_default();
            let missing: Vec<String> = ["ib_write_bw", "ib_send_lat"].iter().filter(|b| !found.lines().any(|l| l.trim().ends_with(&format!("/{b}")))).map(|b| b.to_string()).collect();
            if !missing.is_empty() {
                usable.retain(|u| u != id);
                let n = name(id);
                let pre = Prereq { node: id.clone(), missing, message: format!("perftest isn't installed on {n}"), package: Some("perftest".into()) };
                push(&|r| {
                    r.findings.push(format!("perftest isn't installed on {n}: its pairs were skipped."));
                    r.prereqs.push(pre.clone());
                });
            }
        }
    }

    // Bandwidth and latency, one pair at a time (perftest servers share a port).
    if req.checks.bandwidth || req.checks.latency {
        progress.step("Bandwidth and latency");
        let pairs: Vec<(&Port, &Port)> = all
            .iter()
            .flat_map(|a| all.iter().filter(move |b| b.half == a.half && a.node < b.node).map(move |b| (a, b)))
            .collect();
        for (a, b) in pairs {
            if progress.cancelled() {
                return finish_cancelled(report, progress);
            }
            let key = pair_key(&a.node, &b.node, a.half);
            let base = baselines.get(&key).map(|b| b.gbps);
            let mut cell = BwCell { from: a.node.clone(), to: b.node.clone(), half: a.half, gbps: None, lat_us: None, expected_gbps: expected, baseline_gbps: base, status: Status::Skipped, message: String::new() };
            let (Some(dev_a), Some(dev_b)) = (a.dev.as_deref().filter(|d| valid_name(d)), b.dev.as_deref().filter(|d| valid_name(d))) else {
                cell.message = "No RDMA device on this half".into();
                push(&|r| r.bandwidth.push(cell.clone()));
                continue;
            };
            if !usable.contains(&a.node) || !usable.contains(&b.node) {
                cell.message = "perftest isn't installed".into();
                push(&|r| r.bandwidth.push(cell.clone()));
                continue;
            }
            if !valid_ip(&b.ip) {
                continue;
            }
            cell.status = Status::Running;
            let label = format!("{} → {}, half {}", a.name, b.name, a.half + 1);
            push(&|r| {
                r.current = Some(label.clone());
                r.bandwidth.push(cell.clone());
            });
            let mut failure: Option<String> = None;
            if req.checks.bandwidth {
                match pair_test(runner, &b.node, &a.node, bw_server_cmd(dev_b), bw_client_cmd(dev_a, &b.ip)).await {
                    Ok(text) => match parse::ib_write_bw(&text) {
                        Ok(g) => cell.gbps = Some(g),
                        Err(e) => failure = Some(e),
                    },
                    Err(e) => failure = Some(e),
                }
            }
            if req.checks.latency && failure.is_none() && !progress.cancelled() {
                match pair_test(runner, &b.node, &a.node, lat_server_cmd(dev_b), lat_client_cmd(dev_a, &b.ip)).await {
                    Ok(text) => cell.lat_us = parse::ib_send_lat(&text).ok(),
                    Err(e) => failure = Some(e),
                }
            }
            if progress.cancelled() {
                // Stopped mid-pair: the client was ended and its server stopped; nothing was measured.
                cell.status = Status::Skipped;
                cell.message = "Stopped".into();
                push(&|r| {
                    if let Some(c) = r.bandwidth.iter_mut().rev().find(|c| c.from == cell.from && c.to == cell.to && c.half == cell.half) {
                        *c = cell.clone();
                    }
                });
                return finish_cancelled(report, progress);
            }
            let (status, message) = match (failure, cell.gbps) {
                (Some(e), _) => (Status::Fail, e),
                (None, Some(g)) => grade(g, expected, base),
                (None, None) => (Status::Pass, cell.lat_us.map(|l| format!("{l:.2} µs")).unwrap_or_default()),
            };
            cell.status = status.clone();
            cell.message = message.clone();
            let finding = (status != Status::Pass).then(|| format!("{}→{} half {}: {message}", a.name, b.name, a.half + 1));
            push(&|r| {
                if let Some(c) = r.bandwidth.iter_mut().rev().find(|c| c.from == cell.from && c.to == cell.to && c.half == cell.half) {
                    *c = cell.clone();
                }
                r.current = None;
                if let Some(f) = &finding {
                    r.findings.push(f.clone());
                }
            });
            runner.pause("pair").await;
        }
    }

    // nccl-tests across the chosen nodes, when it's installed.
    if req.checks.nccl && !progress.cancelled() {
        progress.step("nccl-tests all-reduce");
        let result = nccl(runner, &online, req, expected, &all).await;
        let f = (result.status != Status::Pass).then(|| format!("All-reduce: {}", result.message));
        push(&|r| {
            r.nccl = Some(result.clone());
            if let Some(f) = &f {
                r.findings.push(f.clone());
            }
        });
    }

    push(&|r| {
        r.current = None;
        r.finished_ms = Some(now_ms());
    });
}

/// Starts the server side detached on `server`, runs the client on `client`, then
/// stops the server (it would exit by itself after one test or 30 s).
async fn pair_test(runner: &dyn Runner, server: &str, client: &str, server_cmd: String, client_cmd: String) -> Result<String, String> {
    let started = runner.run(server, server_cmd, Duration::from_secs(10), false).await?;
    let pid: Option<u32> = started.stdout.trim().parse().ok();
    if !started.ok() || pid.is_none() {
        return Err(format!("Couldn't start the perftest server: {}", started.text().trim()));
    }
    // Give it a moment to listen on its port.
    runner.pause("server").await;
    let out = runner.run(client, client_cmd, Duration::from_secs(35), true).await;
    if let Some(pid) = pid {
        let _ = runner.run(server, format!("kill {pid} 2>/dev/null; true"), Duration::from_secs(5), false).await;
    }
    let out = out?;
    let text = out.text();
    if out.code == Some(124) {
        return Err(format!("Timed out after 30 s (is port {PERFTEST_PORT} blocked, or the peer's RDMA device down?)"));
    }
    Ok(text)
}

async fn nccl(runner: &dyn Runner, nodes: &[String], req: &DoctorRequest, expected: f64, all: &[Port]) -> NcclResult {
    let bin = req.all_reduce_perf.clone().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "all_reduce_perf".into());
    let mpirun = req.mpirun.clone().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "mpirun".into());
    let skip = |message: String| NcclResult { nodes: nodes.to_vec(), busbw_gbps: None, expected_gbps: expected, status: Status::Skipped, message };
    if nodes.len() < 2 {
        return skip("All-reduce needs two or more nodes".into());
    }
    let Some(head) = nodes.first() else { return skip("No nodes".into()) };
    let found = runner.run(head, format!("command -v {}; command -v {}", sh_quote(&bin), sh_quote(&mpirun)), Duration::from_secs(10), false).await;
    let lines = found.as_ref().map(|o| o.stdout.lines().count()).unwrap_or(0);
    if lines < 2 {
        return skip(format!("nccl-tests isn't installed on {head}: build it (github.com/NVIDIA/nccl-tests, make MPI=1) and set the paths"));
    }
    let hosts: Vec<String> = nodes.iter().filter_map(|n| all.iter().find(|p| &p.node == n && p.half == 0).map(|p| format!("{}:1", p.ip))).collect();
    let hcas: Vec<String> = all.iter().filter(|p| &p.node == head).filter_map(|p| p.dev.clone()).filter(|d| valid_name(d)).collect();
    let iface = all.iter().find(|p| &p.node == head && p.half == 0).map(|p| p.iface.clone()).unwrap_or_default();
    if hosts.len() != nodes.len() || !valid_name(&iface) {
        return skip("Every node needs an addressed fabric half 1".into());
    }
    let cmd = format!(
        "{} -np {} -H {} -x NCCL_IB_HCA={} -x NCCL_SOCKET_IFNAME={} {} -b 1M -e 8G -f 2 -g 1",
        sh_quote(&mpirun),
        nodes.len(),
        sh_quote(&hosts.join(",")),
        sh_quote(&hcas.join(",")),
        sh_quote(&iface),
        sh_quote(&bin)
    );
    match runner.run(head, cmd, Duration::from_secs(300), true).await.map(|o| parse::all_reduce(&o.text())) {
        Ok(Ok(r)) => {
            // Ring all-reduce bus bandwidth tops out near one node's fabric bandwidth (both halves).
            let want = expected * 2.0 * 0.5;
            let (status, message) = if r.wrong > 0 {
                (Status::Fail, format!("{} wrong values: results don't match", r.wrong))
            } else if r.peak_busbw_gbps < want * PASS_SHARE {
                (Status::Warn, format!("Bus bandwidth {:.0} Gb/s at {} GiB, below the {want:.0} Gb/s a healthy fabric gives", r.peak_busbw_gbps, r.largest_bytes >> 30))
            } else {
                (Status::Pass, format!("Bus bandwidth {:.0} Gb/s at {} GiB", r.peak_busbw_gbps, r.largest_bytes >> 30))
            };
            NcclResult { nodes: nodes.to_vec(), busbw_gbps: Some(r.peak_busbw_gbps), expected_gbps: want, status, message }
        }
        Ok(Err(e)) | Err(e) => NcclResult { nodes: nodes.to_vec(), busbw_gbps: None, expected_gbps: expected, status: Status::Fail, message: e },
    }
}

fn finish_cancelled(report: &Mutex<DoctorReport>, progress: &dyn Progress) {
    let mut r = report.lock().unwrap();
    r.cancelled = true;
    r.current = None;
    r.finished_ms = Some(now_ms());
    for c in r.bandwidth.iter_mut().filter(|c| c.status == Status::Running) {
        c.status = Status::Skipped;
        c.message = "Stopped".into();
    }
    progress.update(&r);
}

/// The first run where every bandwidth pair passed becomes the baseline (if none exists yet).
pub fn maybe_save_baselines(path: Option<&PathBuf>, existing: &Baselines, report: &DoctorReport) -> Option<Baselines> {
    let measured: Vec<&BwCell> = report.bandwidth.iter().filter(|c| c.gbps.is_some()).collect();
    if !existing.is_empty() || report.cancelled || measured.is_empty() || report.bandwidth.iter().any(|c| c.status != Status::Pass) {
        return None;
    }
    let now = now_ms();
    let fresh: Baselines = measured.iter().map(|c| (pair_key(&c.from, &c.to, c.half), Baseline { gbps: c.gbps.unwrap_or(0.0), lat_us: c.lat_us, ts_ms: now })).collect();
    if let Some(p) = path {
        crate::util::save_json(p, &fresh).ok()?;
    }
    Some(fresh)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
