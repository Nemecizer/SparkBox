//! Scripted faults for the demo, so health, alerts, error states and recovery can
//! be seen (and tested) without a real cluster misbehaving. Each scenario is a
//! deterministic state machine over ticks: it breaks something, holds it, lets
//! it recover, and then plays again. At most one plays at a time; "as-found" is
//! the cluster exactly as the Live tour found it on Oct 29 (a world of its own,
//! `found`), "default" the everyday serving world (one failed service on cell1),
//! "healthy" the everyday world with no issues.

use serde::Serialize;

use super::ops::set_telemetry_broken;
use super::world::World;
use super::{profile, unix_now, DemoCluster, Kind, MEM_TOTAL};
use crate::model::{KernelEvent, LinkStatus, ScenarioPhase};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioInfo {
    pub id: String,
    pub title: String,
    pub description: String,
    /// Seconds before it plays again (0: steady while active).
    pub period_s: u64,
    /// What it touches, e.g. ["cell3"] or ["crs804"].
    pub affects: Vec<String>,
}

pub(super) struct Run {
    pub id: &'static str,
    pub started: u64,
    /// Each node's boot count when it started: a node that booted since has had its fault cleared.
    pub boots: Vec<u32>,
}

struct Def {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    period: u64,
    affects: &'static [&'static str],
}

const DEFS: &[Def] = &[
    Def {
        id: "as-found",
        title: "Your cluster at rest (Oct 29)",
        description: "Your four nodes as SparkBox found them on Oct 29: nothing serving, the 4-node GLM-5.3-Flash run that exited after 54 s two days before, Ollama on cell1 and cell3 listening on the node only, and the recabling in the kernel log.",
        period: 0,
        affects: &[],
    },
    Def {
        id: "default",
        title: "Everyday serving",
        description: "The same machines serving: Llama 3.3 70B across cell1-cell3, Ollama on gx10-0000, and one failed service (nvidia-dgx-telemetry) on cell1.",
        period: 0,
        affects: &[],
    },
    Def { id: "healthy", title: "All healthy", description: "Everything up and nothing failed, for a clean baseline.", period: 0, affects: &[] },
    Def {
        id: "node-offline",
        title: "Node offline",
        description: "cell3 hangs for a minute (no SSH, no work), then recovers. Its fabric link stays up, but the Llama pipeline has a stage there, so it stalls until cell3 is back.",
        period: 120,
        affects: &["cell3"],
    },
    Def {
        id: "disk-filling",
        title: "Disk filling",
        description: "gx10-0000's 915 GiB disk is at 85% after a model download, then a runaway log fills it to 97% over 10 minutes; the log is cleaned up a minute later.",
        period: 780,
        affects: &["gx10-0000"],
    },
    Def {
        id: "thermal-throttle",
        title: "Thermal throttling",
        description: "cell1's GPU runs at 88-91 °C, its clock drops 30% (HW thermal slowdown, SW power cap), then it cools down.",
        period: 180,
        affects: &["cell1"],
    },
    Def {
        id: "oom-kill",
        title: "Out of memory",
        description: "Ollama on gx10-0000 loads a second model until memory reaches 97%; the kernel kills it (exit 137) and Docker restarts it.",
        period: 120,
        affects: &["gx10-0000"],
    },
    Def {
        id: "crash-loop",
        title: "Crash loop",
        description: "vllm-worker on cell2 keeps crashing and restarting every 15 seconds, so the model is down; after 4 minutes it stays up and the model reloads.",
        period: 300,
        affects: &["cell2"],
    },
    Def {
        id: "fabric-flap",
        title: "Fabric link flapping",
        description: "cell2's 200G link drops for 5 seconds out of every 20 for 2 minutes. Carrier changes, link-downs, receive errors and FCS errors climb; the model can't stay loaded.",
        period: 180,
        affects: &["cell2", "crs804"],
    },
    Def {
        id: "collector-stall",
        title: "Collector stalled",
        description: "cell1's numbers stop updating for 30 seconds (the connection is up but no data arrives), then resume.",
        period: 90,
        affects: &["cell1"],
    },
    Def {
        id: "optic-degraded",
        title: "Degraded optic",
        description: "Lane 3 on cell2's CRS804 port fades to -9.5 dBm (the others stay near -2) and FCS errors appear, until the module is reseated.",
        period: 240,
        affects: &["cell2", "crs804"],
    },
    Def {
        id: "switch-port-down",
        title: "Switch port down",
        description: "CRS804 port qsfp56-dd-3-1 goes down for a minute, taking cell3's fabric and the model with it.",
        period: 120,
        affects: &["cell3", "crs804"],
    },
    Def {
        id: "sudo-rejected",
        title: "Sudo password rejected",
        description: "Every action that needs sudo (services, power, updates) fails with \"sudo: incorrect password\", as if the saved password were out of date.",
        period: 0,
        affects: &[],
    },
    Def {
        id: "slow-node",
        title: "Slow node",
        description: "cell2 answers in 1.8-2.4 seconds instead of a few milliseconds for 90 seconds, then recovers.",
        period: 120,
        affects: &["cell2"],
    },
    // Serving faults: Models plays these (the simulator only names them).
    Def {
        id: "model-oom",
        title: "Model out of memory",
        description: "A new deployment runs out of GPU memory while loading its weights and exits with CUDA out of memory.",
        period: 0,
        affects: &["cell1"],
    },
    Def {
        id: "gated-401",
        title: "Gated model refused",
        description: "Hugging Face answers 401 for a gated repository: the token is missing or hasn't been granted access.",
        period: 0,
        affects: &[],
    },
    Def {
        id: "ray-join-timeout",
        title: "Ray worker can't join",
        description: "A multi-node deployment waits for a Ray worker that never registers with the head, and gives up after 3 minutes.",
        period: 0,
        affects: &["cell2"],
    },
    Def {
        id: "copy-disk-full",
        title: "Copy fills a disk",
        description: "A fabric copy of model weights stops when the destination disk is full.",
        period: 0,
        affects: &["cell3"],
    },
    Def {
        id: "endpoint-slow",
        title: "Endpoint slow",
        description: "llama-3.3-70b answers, but time to first token climbs past 4 seconds and tokens per second halve.",
        period: 0,
        affects: &["cell1"],
    },
    Def {
        id: "endpoint-stall",
        title: "Endpoint stalled",
        description: "llama-3.3-70b still lists its model and holds 3 running requests, but no token comes out: the one-token check times out and it goes Down as stalled.",
        period: 0,
        affects: &["cell1", "cell2", "cell3"],
    },
    // Monitoring faults: the alerting side plays these.
    Def {
        id: "agent-offline",
        title: "Agent offline",
        description: "The SparkBox agent on cell2 stops sending heartbeats, so 24/7 monitoring of that node lapses.",
        period: 0,
        affects: &["cell2"],
    },
    Def {
        id: "paging-drift",
        title: "Agent settings out of date",
        description: "cell3's agent didn't take the last settings push (sudo refused), so it would page with old rules: 24/7 turns amber, Settings › Notifications offers Push again.",
        period: 0,
        affects: &["cell3"],
    },
    Def {
        id: "gpu-xid",
        title: "GPU Xid error",
        description: "gx10-0000's kernel log reports Xid 79 (the GPU has fallen off the bus): its readings stop, Ollama goes Down, and the alert stays open until the node restarts.",
        period: 0,
        affects: &["gx10-0000"],
    },
    // Fabric faults.
    Def {
        id: "fabric-congestion",
        title: "Fabric congestion",
        description: "All-reduce bursts between cell1-cell3: ECN marks and CNPs climb as DCQCN paces senders. Nothing is dropped.",
        period: 120,
        affects: &["cell1", "cell2", "cell3", "crs804"],
    },
    Def {
        id: "roce-loss",
        title: "RoCE packet loss",
        description: "cell2's half 2 starts losing packets: out-of-sequence, ACK timeouts and CRC errors climb, and the link flaps once.",
        period: 240,
        affects: &["cell2", "crs804"],
    },
    Def {
        id: "pfc-storm",
        title: "PFC pause storm",
        description: "gx10-0000 sends 50,000 pause frames a second on its half 1 for 90 seconds, stalling the switch's priority 3 queue.",
        period: 180,
        affects: &["gx10-0000", "crs804"],
    },
    Def {
        id: "fabric-partition",
        title: "Fabric lost",
        description: "cell2 loses both fabric halves for two minutes while its LAN stays up: one incident, cell2 lost the fabric, with the model down; the node stays online.",
        period: 180,
        affects: &["cell2", "crs804"],
    },
    Def {
        id: "miscabled",
        title: "Cables swapped",
        description: "cell2 and cell3's fabric cables are in each other's CRS804 ports. Port comments still name the old nodes.",
        period: 0,
        affects: &["cell2", "cell3", "crs804"],
    },
    Def {
        id: "mtu-mismatch",
        title: "MTU mismatch",
        description: "gx10-0000's half 2 is at MTU 1500 while the rest of the fabric is at 9000, so jumbo frames to it fail.",
        period: 0,
        affects: &["gx10-0000"],
    },
];

/// Where each looping scenario is in its cycle: (end of the stretch in seconds, phase).
/// A scenario that isn't listed is steady: in its fault phase for as long as it plays.
const PHASES: &[(&str, &[(u64, &str)])] = &[
    ("node-offline", &[(63, "fault"), (66, "recovering"), (120, "healthy")]),
    ("disk-filling", &[(660, "fault"), (680, "recovering"), (780, "healthy")]),
    ("thermal-throttle", &[(120, "fault"), (150, "recovering"), (180, "healthy")]),
    ("oom-kill", &[(55, "fault"), (70, "recovering"), (120, "healthy")]),
    ("crash-loop", &[(240, "fault"), (263, "recovering"), (300, "healthy")]),
    ("fabric-flap", &[(120, "fault"), (143, "recovering"), (180, "healthy")]),
    ("collector-stall", &[(30, "fault"), (90, "healthy")]),
    ("optic-degraded", &[(180, "fault"), (200, "recovering"), (240, "healthy")]),
    ("switch-port-down", &[(60, "fault"), (83, "recovering"), (120, "healthy")]),
    ("slow-node", &[(90, "fault"), (120, "healthy")]),
    ("fabric-congestion", &[(90, "fault"), (120, "healthy")]),
    ("roce-loss", &[(180, "fault"), (200, "recovering"), (240, "healthy")]),
    ("pfc-storm", &[(90, "fault"), (180, "healthy")]),
    ("fabric-partition", &[(120, "fault"), (143, "recovering"), (180, "healthy")]),
];

const CRASH_CYCLE: u64 = 15;
const CRASH_FOR: u64 = 240;
const FILL_TICKS: u64 = 600;

impl DemoCluster {
    pub fn scenarios(&self) -> Vec<ScenarioInfo> {
        DEFS.iter()
            .map(|d| ScenarioInfo {
                id: d.id.into(),
                title: d.title.into(),
                description: d.description.into(),
                period_s: d.period,
                affects: d.affects.iter().map(|a| a.to_string()).collect(),
            })
            .collect()
    }

    /// The scenario playing, or None for the everyday world.
    pub fn scenario(&self) -> Option<String> {
        self.scenario.as_ref().map(|r| r.id.to_string())
    }

    /// Where the playing scenario is: "Node offline · recovering", and how long until it moves on.
    pub fn scenario_phase(&self) -> Option<ScenarioPhase> {
        let run = self.scenario.as_ref()?;
        let def = DEFS.iter().find(|d| d.id == run.id)?;
        if matches!(run.id, "healthy" | "as-found") {
            return Some(ScenarioPhase { phase: "healthy".into(), label: def.title.into(), next_change_s: None });
        }
        let elapsed = self.tick.saturating_sub(run.started);
        let Some((_, stretches)) = PHASES.iter().find(|(id, _)| *id == run.id) else {
            return Some(ScenarioPhase { phase: "fault".into(), label: format!("{} · fault", def.title), next_change_s: None });
        };
        let ph = if def.period > 0 { elapsed % def.period } else { elapsed };
        let (end, phase) = stretches.iter().find(|(end, _)| ph < *end).copied().unwrap_or((def.period, "healthy"));
        Some(ScenarioPhase { phase: phase.into(), label: format!("{} · {phase}", def.title), next_change_s: Some(end.saturating_sub(ph) as u32) })
    }

    /// Switches scenario (None or "default" for the everyday world). Whatever the old one
    /// broke is put back first, so each starts from the same baseline.
    pub fn set_scenario(&mut self, id: Option<&str>) -> Result<(), String> {
        let id = match id.map(str::trim) {
            None | Some("") | Some("default") => None,
            Some(id) => Some(DEFS.iter().find(|d| d.id == id).map(|d| d.id).ok_or_else(|| format!("unknown scenario {id}"))?),
        };
        if let Some(old) = self.scenario.take() {
            exit(self, old.id);
        }
        if let Some(id) = id {
            enter(self, id);
            self.scenario = Some(Run { id, started: self.tick, boots: self.nodes.iter().map(|n| n.boots).collect() });
        }
        Ok(())
    }
}

fn idx(d: &DemoCluster, name: &str) -> Option<usize> {
    d.nodes.iter().position(|n| n.cfg.id == name)
}

fn enter(d: &mut DemoCluster, id: &str) {
    let now = unix_now();
    match id {
        "as-found" => d.rebuild(World::AsFound),
        "healthy" => {
            if let Some(i) = idx(d, "cell1") {
                set_telemetry_broken(&mut d.nodes[i], false, now);
            }
            for n in &mut d.nodes {
                for c in n.containers.iter_mut().filter(|c| c.c.state == "exited" && c.c.exit_code != 0) {
                    c.c.exit_code = 0;
                    c.c.oom_killed = false;
                }
            }
        }
        "crash-loop" => {
            if let Some(c) = idx(d, "cell2").and_then(|i| d.nodes[i].containers.iter_mut().find(|c| c.kind == Kind::RayWorker)) {
                c.c.restart_policy = Some("on-failure".into());
            }
        }
        "miscabled" => swap_cables(d),
        "gpu-xid" => {
            if let Some(i) = idx(d, "gx10-0000") {
                let pid = d.nodes[i].containers.iter().find(|c| c.kind == Kind::Ollama).map(|c| c.pid).unwrap_or(0);
                push_event(
                    &mut d.nodes[i],
                    KernelEvent {
                        ts_ms: now * 1000,
                        kind: "xid".into(),
                        code: Some(79),
                        text: format!("NVRM: Xid (PCI:000f:01:00): 79, pid={pid}, name=ollama, GPU has fallen off the bus."),
                        severity: "crit".into(),
                        process: Some("ollama".into()),
                    },
                );
            }
        }
        _ => {}
    }
}

/// cell2 and cell3 trade CRS804 ports (the comments stay where they were).
fn swap_cables(d: &mut DemoCluster) {
    let (Some(a), Some(b)) = (idx(d, "cell2"), idx(d, "cell3")) else { return };
    for s in &mut d.switches {
        for p in &mut s.ports {
            if p.node == Some(a) {
                p.node = Some(b);
            } else if p.node == Some(b) {
                p.node = Some(a);
            }
        }
    }
}

/// Adds a kernel log event, keeping the most recent 50.
pub(super) fn push_event(n: &mut super::DemoNode, e: KernelEvent) {
    n.kernel_events.push(e);
    let extra = n.kernel_events.len().saturating_sub(50);
    n.kernel_events.drain(..extra);
}

/// The scenario playing and how far into its cycle it is, in seconds.
pub(super) fn playing(d: &DemoCluster) -> Option<(&'static str, u64)> {
    let run = d.scenario.as_ref()?;
    let period = DEFS.iter().find(|x| x.id == run.id).map(|x| x.period).unwrap_or(0);
    let elapsed = d.tick.saturating_sub(run.started);
    Some((run.id, if period > 0 { elapsed % period } else { elapsed }))
}

fn exit(d: &mut DemoCluster, id: &str) {
    let now = unix_now();
    match id {
        "as-found" => {
            d.rebuild(World::Everyday);
            return;
        }
        "healthy" => {
            if let Some(i) = idx(d, "cell1") {
                set_telemetry_broken(&mut d.nodes[i], true, now);
            }
            if let Some(c) = d.nodes.iter_mut().flat_map(|n| n.containers.iter_mut()).find(|c| c.c.name == "sglang-test" && c.c.state == "exited") {
                c.c.exit_code = 137;
                c.c.oom_killed = true;
            }
        }
        "disk-filling" => {
            for n in &mut d.nodes {
                n.disk_used = n.disk_base;
            }
        }
        "oom-kill" => {
            if let Some(i) = idx(d, "gx10-0000") {
                for c in d.nodes[i].containers.iter_mut().filter(|c| c.kind == Kind::Ollama) {
                    c.gpu_mem = profile(Kind::Ollama).gpu_mem;
                    if c.c.state == "exited" && c.c.oom_killed {
                        c.c.state = "running".into();
                        c.c.started_at = now;
                        c.c.oom_killed = false;
                        c.c.restart_count += 1;
                    }
                }
            }
        }
        "crash-loop" => {
            if let Some(c) = idx(d, "cell2").and_then(|i| d.nodes[i].containers.iter_mut().find(|c| c.kind == Kind::RayWorker)) {
                c.c.restart_policy = Some("no".into());
                if c.c.state == "restarting" {
                    c.c.state = "running".into();
                    c.c.started_at = now;
                }
            }
        }
        "miscabled" => swap_cables(d),
        "gpu-xid" => {
            // Leaving it is the restart that clears the fault: a new boot (and boot_id).
            if let Some(i) = idx(d, "gx10-0000") {
                d.nodes[i].boot_unix = now;
                d.nodes[i].boots += 1;
            }
        }
        _ => {}
    }
    for n in &mut d.nodes {
        n.frozen = None;
        // The events a scenario logged leave with it, so the baseline stays clean.
        n.kernel_events.clear();
    }
}

/// Applies the playing scenario for this tick: overlays on nodes and ports, and the
/// one-off events (a kill, a restart) at the ticks they happen.
pub(super) fn step(d: &mut DemoCluster) {
    let Some(run) = &d.scenario else { return };
    let id = run.id;
    let period = DEFS.iter().find(|x| x.id == id).map(|x| x.period).unwrap_or(0);
    let elapsed = d.tick.saturating_sub(run.started);
    let ph = if period > 0 { elapsed % period } else { elapsed };
    let now = unix_now();

    match id {
        "node-offline" => {
            let Some(i) = idx(d, "cell3") else { return };
            let ip = d.nodes[i].cfg.ip.clone().unwrap_or_else(|| d.nodes[i].cfg.host.clone());
            d.nodes[i].ov.status = match ph {
                0..=2 => Some((LinkStatus::Connecting, Some(format!("No reply from {ip}:22 yet; retrying…")))),
                3..=62 => Some((LinkStatus::Offline, Some(format!("SSH timeout: no reply from {ip}:22 in 15 s")))),
                63..=65 => Some((LinkStatus::Connecting, Some("Reconnecting…".into()))),
                _ => None,
            };
        }
        "disk-filling" => {
            let Some(i) = idx(d, "gx10-0000") else { return };
            let n = &mut d.nodes[i];
            let full = n.disk_total * 0.97;
            // It starts on the 85% warning line (the download), and the log fills the rest.
            let base = n.disk_total * 0.85;
            let before = n.disk_used;
            n.disk_used = match ph {
                p if p < FILL_TICKS => base + (full - base) * (p + 1) as f64 / FILL_TICKS as f64,
                p if p < 660 => full,
                p if p < 680 => full - (full - base) * (p - 659) as f64 / 20.0,
                _ => base,
            };
            n.ov.disk_write = (n.disk_used - before).max(0.0);
        }
        "thermal-throttle" => {
            if let Some(i) = idx(d, "cell1") {
                d.nodes[i].ov.hot = ph < 120;
            }
        }
        "oom-kill" => {
            let Some(i) = idx(d, "gx10-0000") else { return };
            let n = &mut d.nodes[i];
            let others: f64 = n.containers.iter().filter(|c| c.running() && c.kind != Kind::Ollama).map(|c| c.rss + c.gpu_mem).sum::<f64>()
                + n.procs.iter().map(|p| p.rss).sum::<f64>()
                + 4.1 * super::GIB;
            let base = profile(Kind::Ollama).gpu_mem;
            let Some(c) = n.containers.iter_mut().find(|c| c.kind == Kind::Ollama && !c.manual_stop) else { return };
            match ph {
                0..=41 => {
                    if c.running() {
                        let room = (MEM_TOTAL * 0.97 - others - c.rss - base).max(0.0);
                        c.gpu_mem = base + room * ((ph + 1) as f64 / 40.0).min(1.0);
                    }
                }
                42 => {
                    let killed = c.running();
                    if killed {
                        c.c.state = "exited".into();
                        c.c.exit_code = 137;
                        c.c.oom_killed = true;
                        c.c.finished_at = now;
                    }
                    c.gpu_mem = base;
                    let (pid, vm) = (c.pid, (c.gpu_mem + c.rss) / 1024.0);
                    if killed {
                        push_event(
                            n,
                            KernelEvent {
                                ts_ms: now * 1000,
                                kind: "oom".into(),
                                code: None,
                                text: format!(
                                    "Out of memory: Killed process {pid} (ollama) total-vm:{:.0}kB, anon-rss:{:.0}kB, file-rss:0kB, shmem-rss:0kB, UID:0 pgtables:181244kB oom_score_adj:0",
                                    vm * 1.08,
                                    vm
                                ),
                                severity: "crit".into(),
                                process: Some("ollama".into()),
                            },
                        );
                    }
                }
                55 => {
                    if c.c.state == "exited" && c.c.oom_killed {
                        // unless-stopped: Docker brings it back.
                        c.c.state = "running".into();
                        c.c.started_at = now;
                        c.c.exit_code = 0;
                        c.c.oom_killed = false;
                        c.c.restart_count += 1;
                        c.pid += 1_201;
                    }
                }
                _ => c.gpu_mem = base,
            }
        }
        "crash-loop" => {
            let Some(i) = idx(d, "cell2") else { return };
            let mut reset = false;
            if let Some(c) = d.nodes[i].containers.iter_mut().find(|c| c.kind == Kind::RayWorker && !c.manual_stop) {
                if ph < CRASH_FOR {
                    let crashing = (ph / CRASH_CYCLE).is_multiple_of(2);
                    if crashing && c.c.state != "restarting" {
                        c.c.state = "restarting".into();
                        c.c.exit_code = 1;
                        c.c.finished_at = now;
                        c.c.restart_count += 1;
                        reset = true;
                    } else if !crashing && c.c.state == "restarting" {
                        c.c.state = "running".into();
                        c.c.started_at = now;
                        c.pid += 97;
                    }
                } else if c.c.state == "restarting" {
                    c.c.state = "running".into();
                    c.c.started_at = now;
                }
            }
            if reset {
                d.ray_load = 0.0;
            }
        }
        "fabric-flap" => {
            let Some(i) = idx(d, "cell2") else { return };
            let flapping = ph < 120;
            let down = flapping && ph % 20 < 5;
            d.nodes[i].ov.carrier_down = down;
            if flapping && !down {
                // A marginal link: errors trickle in while it's up.
                for half in 0..2 {
                    let e = d.rng.u64(0..4);
                    d.nodes[i].counters[half].rx_errors += e;
                    d.nodes[i].counters[half].rx_drops += e / 2;
                }
                let fcs = d.rng.u64(0..5);
                if let Some(p) = port_of(d, i) {
                    p.fcs_errors += fcs;
                    p.rx_errors += fcs;
                }
            }
        }
        "collector-stall" => {
            if let Some(i) = idx(d, "cell1") {
                d.nodes[i].ov.stale = ph < 30;
            }
        }
        "optic-degraded" => {
            let Some(i) = idx(d, "cell2") else { return };
            let flaky = (30..180).contains(&ph) && d.rng.f64() < 0.3;
            let Some(p) = port_of(d, i) else { return };
            let base = p.lanes.get(2).copied().unwrap_or(-2.0);
            let dbm = match ph {
                0..=29 => base + (-9.5 - base) * (ph + 1) as f32 / 30.0,
                30..=179 => -9.5,
                180..=199 => -9.5 + (base + 9.5) * (ph - 179) as f32 / 20.0,
                _ => base,
            };
            p.ov_lane = Some((2, dbm));
            if flaky {
                p.fcs_errors += 1;
                p.rx_errors += 1;
            }
        }
        "switch-port-down" => {
            for s in &mut d.switches {
                if let Some(p) = s.ports.iter_mut().find(|p| p.name == "qsfp56-dd-3-1") {
                    p.ov_down = ph < 60;
                }
            }
        }
        "slow-node" => {
            if let Some(i) = idx(d, "cell2") {
                if ph < 90 {
                    d.nodes[i].ov.latency_ms = Some(1_800.0 + d.rng.f64() * 600.0);
                }
            }
        }
        "gpu-xid" => {
            // Off the bus until the node restarts (a reboot from SparkBox fixes it).
            let boots = run.boots.clone();
            if let Some(i) = idx(d, "gx10-0000") {
                d.nodes[i].ov.gpu_lost = boots.get(i).is_some_and(|b| d.nodes[i].boots <= *b);
            }
        }
        "fabric-partition" => {
            if let Some(i) = idx(d, "cell2") {
                d.nodes[i].ov.carrier_down = ph < 120;
            }
        }
        "mtu-mismatch" => {
            if let Some(i) = idx(d, "gx10-0000") {
                d.nodes[i].ov.mtu[1] = Some(1500);
            }
        }
        "roce-loss" => {
            // The marginal link drops once, 100 s in; its counters are bumped in `fabric::step`.
            if let Some(i) = idx(d, "cell2") {
                d.nodes[i].ov.carrier_down = (100..103).contains(&ph);
            }
        }
        _ => {}
    }
}

/// The CRS804 port a node is cabled to.
fn port_of(d: &mut DemoCluster, node: usize) -> Option<&mut super::DemoPort> {
    d.switches.iter_mut().flat_map(|s| s.ports.iter_mut()).find(|p| p.node == Some(node))
}
