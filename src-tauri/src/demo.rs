//! Built-in simulator so the whole UI works away from the home network.
//!
//! Two worlds run on the same four machines, whose identity is real (`world`: hostnames, BIOS,
//! MACs, disks, memory, idle readings, from the Live tour of Oct 29). In the everyday world the
//! first three nodes serve Llama 3.3 70B with vLLM over Ray, pipeline-parallel across the CRS804
//! fabric (cell1 is the Ray head), and the last runs Ollama on its own. The as-found world
//! (scenario `as-found`, `found`) is the cluster exactly as SparkBox found it: nothing serving,
//! the failed GLM-5.3-Flash run, native Ollama on loopback, the real services and kernel log.
//! Every simulator reads the same identity, so the snapshot, Files, Terminal, Models and Fleet
//! Command all agree.
//!
//! The simulation is stateful and can fail: stopping a Ray container, killing
//! its process, rebooting a Ray node or disabling its switch port takes the
//! model down, just as it would for real, and `scenarios` plays scripted faults
//! (a node dropping off, a disk filling, a crash loop…) one tick at a time.
//! Each node has a voice of its own (`trace`): the same function of time drives the live
//! simulator and the history synthesized for Demo, so no two nodes draw the same line and
//! the past meets the present without a jump.
//! The identity is real; everything that moves is simulated, and the UI labels it "Demo data".

pub mod commands;
pub mod fabric;
pub mod found;
pub mod ops;
pub mod scenarios;
#[cfg(test)]
mod tests;
pub mod trace;
pub mod world;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::inventory::{Inventory, NodeConfig, SwitchConfig, SwitchKind};
use crate::model::*;
#[allow(unused_imports)]
pub use found::DemoImage;
#[allow(unused_imports)]
pub use ops::{DemoContainerSpec, DemoPackage, DemoUnit, DemoUpdates};
pub use scenarios::ScenarioInfo;
use world::*;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
/// What a Spark's kernel reports of its 128 GB (121.7 GiB); each node's own total is in `world`.
const MEM_TOTAL: f64 = 130_661_117_952.0;
const CORES: usize = 20;
/// Kernel clock ticks per second (for process start times).
const HZ: u64 = 100;
/// Seconds the Ray cluster takes to read its share of the weights from disk.
const LOAD_SECONDS: f64 = 22.0;
/// Seconds an engine load set by Models holds before the GPU falls back to idle.
const ENGINE_HOLD_S: u64 = 10;
/// The board's power ceiling, for the throttle simulation only: GB10 reports its limit as N/A.
const GB10_CAP_W: f32 = 100.0;

/// What a serving engine is doing on a node, as Models' request simulator reports it.
/// Pipeline-parallel stages each get a call with their share of the work.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EngineLoad {
    pub util_pct: f32,
    /// Board power while serving; None to derive it from the utilization.
    pub power_w: Option<f32>,
    /// Generating (prefill or decode) right now.
    pub active: bool,
    /// Serving requests normally (not loading, warming up or hung): the GPU then reads the
    /// node's own trace, so the live line and the history synthesized for Demo are one curve.
    pub steady: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    RayHead,
    RayWorker,
    Ollama,
}

/// What a container is, which decides how it behaves when things around it change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    RayHead,
    RayWorker,
    Ollama,
    Plain,
    /// Added by Models (`add_container`): a serving engine SparkBox started.
    Injected,
}

struct DemoContainer {
    c: Container,
    kind: Kind,
    pid: u32,
    proc_name: String,
    /// GPU memory when fully loaded.
    gpu_mem: f64,
    rss: f64,
    idle_cpu: f64,
    busy_cpu: f64,
    nice: i32,
    threads: u32,
    /// SIGSTOP'd: in the process table as state T, doing nothing.
    frozen: bool,
    /// Stopped by hand (`docker stop`), so restart policies leave it alone.
    manual_stop: bool,
    env: Vec<String>,
    /// Host ports it listens on (host networking or published).
    listen: Vec<u16>,
    /// Tick at which an exit is due (the Ray cascade): (tick, exit code).
    exit_at: Option<(u64, i32)>,
    cpu_time: f64,
    /// `docker inspect` facts the kind doesn't give (glm53's mounts and host IPC), merged over
    /// the generated document.
    inspect: serde_json::Value,
    /// What `docker logs` prints once it has stopped (the real glm53 log); None to make it up.
    log: Option<String>,
}

impl DemoContainer {
    fn running(&self) -> bool {
        self.c.state == "running"
    }

    /// Doing work: running and not paused or stopped.
    fn active(&self) -> bool {
        self.running() && !self.frozen
    }

    fn is_ray(&self) -> bool {
        matches!(self.kind, Kind::RayHead | Kind::RayWorker)
    }
}

/// A host process (outside containers) that the Workloads window can signal.
#[derive(Clone)]
struct DemoProc {
    pid: u32,
    name: String,
    user: String,
    cmd: String,
    base_cpu: f64,
    rss: f64,
    nice: i32,
    unit: Option<String>,
    ports: Vec<(u16, &'static str)>,
    ppid: u32,
    threads: u32,
    /// Seconds after boot it started.
    started_s: u64,
    stopped: bool,
    cpu_time: f64,
    /// A kernel thread ([kthreadd], kworkers): sent with `kernel: true`, hidden unless asked.
    kernel: bool,
    /// SparkBox's own collector and its nvidia-smi.
    sparkbox: bool,
}

/// Where a node is in a reboot, shutdown or wake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Power {
    On,
    /// Ticks left in each phase: going down (Connecting), down (Offline), coming up (Connecting).
    Rebooting { down: u32, off: u32, up: u32 },
    ShuttingDown { left: u32 },
    Off,
    Waking { off: u32, up: u32 },
}

/// Bursty disk traffic: short random bursts on top of a trickle.
#[derive(Default, Clone, Copy)]
struct Io {
    read_left: u32,
    read_rate: f64,
    write_left: u32,
    write_rate: f64,
}

/// One fabric netdev's counters since boot.
#[derive(Default, Clone, Copy)]
struct Counters {
    rx_errors: u64,
    tx_errors: u64,
    rx_drops: u64,
    tx_drops: u64,
    carrier_changes: u64,
}

/// This tick's traffic, in bits per second. Stored so the switch mirrors exactly what the node reports.
#[derive(Default, Clone, Copy)]
struct Traffic {
    lan_rx: f64,
    lan_tx: f64,
    ip_rx: [f64; 2],
    ip_tx: [f64; 2],
    rdma_rx: [f64; 2],
    rdma_tx: [f64; 2],
    /// What a Wi-Fi radio on the LAN hears (its multicast and broadcast).
    wifi_rx: f64,
}

impl Traffic {
    fn fabric_rx(&self) -> f64 {
        self.ip_rx.iter().chain(&self.rdma_rx).sum()
    }
    fn fabric_tx(&self) -> f64 {
        self.ip_tx.iter().chain(&self.rdma_tx).sum()
    }
}

/// What the active scenario does to a node this tick (reset every tick).
#[derive(Default, Clone)]
struct Overlay {
    status: Option<(LinkStatus, Option<String>)>,
    latency_ms: Option<f64>,
    stale: bool,
    carrier_down: bool,
    mem_extra: f64,
    hot: bool,
    disk_write: f64,
    /// A fabric half's MTU differs from the world's 9000 (mtu-mismatch).
    mtu: [Option<u32>; 2],
    /// The GPU has fallen off the bus (gpu-xid): no utilization, idle power.
    gpu_lost: bool,
}

struct DemoNode {
    cfg: NodeConfig,
    idx: usize,
    role: Role,
    world: World,
    /// The machine it is (world::REAL_NODES): MACs, disk, memory, idle readings.
    real: &'static RealNode,
    /// Ports open on it that no simulated process owns.
    orphan_ports: Vec<(u16, &'static str)>,
    phase: f64,
    /// Smoothed GPU utilization, 0..1.
    util: f64,
    gpu_temp: f64,
    /// How warm this node runs (its spot in the rack).
    temp_offset: f64,
    cores: Vec<f64>,
    load: [f64; 3],
    disk_used: f64,
    disk_base: f64,
    disk_total: f64,
    io: Io,
    disk_read: f64,
    disk_write: f64,
    boot_unix: u64,
    /// Its switch port is disabled.
    fabric_disabled: bool,
    counters: [Counters; 2],
    traffic: Traffic,
    /// Fabric copies and weight loads started by Models (bits per second).
    ext_rx: f64,
    ext_tx: f64,
    containers: Vec<DemoContainer>,
    procs: Vec<DemoProc>,
    units: Vec<ops::Unit>,
    updates: Vec<DemoPackage>,
    power: Power,
    driver: String,
    kernel: String,
    /// Installed but waiting for a reboot: (driver, kernel).
    pending_boot: Option<(Option<String>, Option<String>)>,
    collector_pid: u32,
    next_pid: u32,
    ov: Overlay,
    /// The last snapshot, held while the collector is stalled.
    frozen: Option<(NodeSnapshot, u64, u64)>,
    /// The fabric carrier last tick, to count changes.
    carrier_was: bool,
    /// The last engine load Models set, and the tick it was set. Once a node has
    /// been driven this way its GPU follows only the engine (idle when it stops).
    engine: Option<(EngineLoad, u64)>,
    /// RoCE and PHY counters since boot, per fabric half.
    roce: [RoceCounters; 2],
    /// The simulated time its GPU last read its trace (the everyday world), for the readings that follow it.
    traced_ms: Option<u64>,
    /// Boots since the simulator started (a scenario that lasts until a restart watches it).
    boots: u32,
    /// Recent kernel events (Xid, OOM kills), newest last.
    kernel_events: Vec<KernelEvent>,
    /// The switch port its fabric cable is in, "crs804/qsfp56-dd-1-2" (follows the switch's cabling).
    cabled_to: Option<String>,
    /// perftest (ib_write_bw, ib_send_lat) is installed.
    perftest: bool,
    /// The idle GPU's throttle reasons last reading: GB10 flips between none and "SW power cap"
    /// from one sample to the next at P8.
    idle_cap: bool,
}

impl DemoNode {
    fn online(&self) -> bool {
        self.power == Power::On && !matches!(self.ov.status, Some((s, _)) if s != LinkStatus::Online)
    }

    fn powered(&self) -> bool {
        matches!(self.power, Power::On | Power::ShuttingDown { .. } | Power::Rebooting { down: 1.., .. })
    }

    /// The fabric has carrier: powered, port enabled, no flap or port-down scenario.
    fn carrier(&self) -> bool {
        self.power == Power::On && !self.fabric_disabled && !self.ov.carrier_down
    }

    fn alloc_pid(&mut self, rng: &mut fastrand::Rng) -> u32 {
        self.next_pid += rng.u32(37..900);
        self.next_pid
    }

    fn container_mut(&mut self, id: &str) -> Option<&mut DemoContainer> {
        self.containers.iter_mut().find(|c| c.c.id == id || c.c.name == id || (id.len() >= 12 && c.c.id.starts_with(&id[..12])))
    }
}

struct DemoPort {
    name: String,
    comment: String,
    speed: &'static str,
    node: Option<usize>,
    uplink: bool,
    disabled: bool,
    link_downs: u64,
    last_up_unix: u64,
    rx_errors: u64,
    fcs_errors: u64,
    rx_drops: u64,
    rx_pause: u64,
    /// Baseline per-lane receive power, dBm.
    lanes: Vec<f32>,
    tx_power: f32,
    /// Scenario overrides this tick.
    ov_down: bool,
    ov_lane: Option<(usize, f32)>,
    running_was: bool,
    /// Disabled with a revert scheduled on the switch: (tick, Unix ms) it re-enables.
    revert_at: Option<(u64, u64)>,
    /// Seconds left of an on-switch bounce (down, then back up by itself).
    bounce_left: u32,
}

struct DemoSwitch {
    cfg: SwitchConfig,
    host: String,
    boot_unix: u64,
    ports: Vec<DemoPort>,
}

pub struct DemoCluster {
    world: World,
    tick: u64,
    /// Unix ms at tick 0: simulated time is this plus a second a tick (`sim_ms`).
    t0_ms: u64,
    rng: fastrand::Rng,
    nodes: Vec<DemoNode>,
    switches: Vec<DemoSwitch>,
    /// Weights loaded across the Ray nodes, 0..1. The model serves at 1.
    ray_load: f64,
    scenario: Option<scenarios::Run>,
}

/// Debug builds only (never tests): SPARKBOX_DEMO_SCENARIO=as-found starts the simulator in that
/// scenario, so a dev run shows the cluster as found from its first second, with no everyday
/// endpoints remembered and history drawn for a cluster at rest. Choosing it later in Settings
/// starts the world over mid-run instead.
pub(crate) fn start_scenario() -> Option<String> {
    if !cfg!(debug_assertions) || cfg!(test) {
        return None;
    }
    std::env::var("SPARKBOX_DEMO_SCENARIO").ok().filter(|s| !s.is_empty() && s != "default")
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn unix_now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn jitter(rng: &mut fastrand::Rng, spread: f64) -> f64 {
    (rng.f64() - 0.5) * 2.0 * spread
}

/// A log-uniform random value between `lo` and `hi`.
fn log_uniform(rng: &mut fastrand::Rng, lo: f64, hi: f64) -> f64 {
    (lo.ln() + rng.f64() * (hi.ln() - lo.ln())).exp()
}

/// The container's main process name and resource profile, by kind.
struct Profile {
    proc_name: &'static str,
    gpu_mem: f64,
    rss: f64,
    idle_cpu: f64,
    busy_cpu: f64,
    threads: u32,
}

fn profile(kind: Kind) -> Profile {
    match kind {
        Kind::RayHead => Profile { proc_name: "python3", gpu_mem: 0.70 * MEM_TOTAL, rss: 9.4 * GIB, idle_cpu: 4.0, busy_cpu: 135.0, threads: 214 },
        Kind::RayWorker => Profile { proc_name: "ray::RayWorkerW", gpu_mem: 0.70 * MEM_TOTAL, rss: 7.1 * GIB, idle_cpu: 3.0, busy_cpu: 118.0, threads: 187 },
        Kind::Ollama => Profile { proc_name: "ollama", gpu_mem: 41.5 * GIB, rss: 1.3 * GIB, idle_cpu: 0.5, busy_cpu: 45.0, threads: 38 },
        Kind::Plain | Kind::Injected => Profile { proc_name: "python3", gpu_mem: 0.0, rss: 0.75 * GIB, idle_cpu: 1.2, busy_cpu: 3.0, threads: 24 },
    }
}

#[allow(clippy::too_many_arguments)]
fn make_container(
    node: &DemoNode,
    pid: u32,
    kind: Kind,
    name: &str,
    image: &str,
    published: &[u16],
    listen: &[u16],
    command: &str,
    started_at: u64,
    restart_policy: &str,
    labels: &[(&str, &str)],
    env: Vec<String>,
) -> DemoContainer {
    let p = profile(kind);
    let mut labels: BTreeMap<String, String> = labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    if image.starts_with("nvcr.io/nvidia/vllm") {
        labels.insert("com.nvidia.volumes.needed".into(), "nvidia_driver".into());
        labels.insert("org.opencontainers.image.version".into(), "26.05".into());
    }
    DemoContainer {
        c: Container {
            id: container_id(&node.cfg.id, name),
            name: name.into(),
            image: image.into(),
            state: "running".into(),
            started_at,
            command: command.into(),
            ports: published.iter().map(|p| format!("0.0.0.0:{p}->{p}/tcp")).collect(),
            project: labels.get("com.docker.compose.project").cloned(),
            restart_policy: Some(restart_policy.into()),
            labels,
            ..Default::default()
        },
        kind,
        pid,
        proc_name: p.proc_name.into(),
        gpu_mem: p.gpu_mem,
        rss: p.rss,
        idle_cpu: p.idle_cpu,
        busy_cpu: p.busy_cpu,
        nice: 0,
        threads: p.threads,
        frozen: false,
        manual_stop: false,
        env,
        listen: listen.to_vec(),
        exit_at: None,
        cpu_time: 0.0,
        inspect: serde_json::Value::Null,
        log: None,
    }
}

/// The environment NVIDIA's guide sets for vLLM over Ray on the Spark fabric.
fn ray_env(node_index: usize, head: bool) -> Vec<String> {
    let mut env = vec![
        "PATH=/usr/local/nvidia/bin:/usr/local/cuda/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "NVIDIA_VISIBLE_DEVICES=all".into(),
        "NVIDIA_DRIVER_CAPABILITIES=compute,utility".into(),
        format!("VLLM_HOST_IP={}", fabric_ip(node_index, 0)),
        format!("NCCL_SOCKET_IFNAME={NCCL_IFACE}"),
        format!("GLOO_SOCKET_IFNAME={NCCL_IFACE}"),
        format!("UCX_NET_DEVICES={NCCL_IFACE}"),
        format!("NCCL_IB_HCA={},{}", FABRIC[0].2, FABRIC[1].2),
        format!("MASTER_ADDR={}", fabric_ip(0, 0)),
        "HF_HOME=/models/.cache/huggingface".into(),
    ];
    if head {
        // A gated-model token, so Inspect has a secret to mask (and Reveal to show).
        env.push("HF_TOKEN=hf_demoKq3vT9mZpL2xW8rN5cB1yH6dF4sJ7aQ".into());
    } else {
        env.push(format!("RAY_ADDRESS={}:6379", fabric_ip(0, 0)));
    }
    env
}

fn base_containers(node: &mut DemoNode, rng: &mut fastrand::Rng, now: u64) -> Vec<DemoContainer> {
    let model_dir = format!("/models/{SERVED_REPO}");
    let compose = |svc: &'static str| vec![("com.docker.compose.project", "llama-70b"), ("com.docker.compose.service", svc)];
    match node.role {
        Role::RayHead => {
            let head_pid = node.alloc_pid(rng);
            let webui_pid = node.alloc_pid(rng);
            let mut head = make_container(
                node,
                head_pid,
                Kind::RayHead,
                "vllm-head",
                VLLM_IMAGE,
                &[],
                &[8000, 6379, 8265],
                &format!(
                    "vllm serve {model_dir} --served-model-name {SERVED_NAME} --pipeline-parallel-size 3 \
                     --distributed-executor-backend ray --max-model-len 32768 --gpu-memory-utilization 0.70 --port 8000"
                ),
                now - 2 * 86_400 - 7_200,
                "no",
                &compose("vllm-head"),
                ray_env(node.idx, true),
            );
            head.c.health = Some("healthy".into());
            let mut webui = make_container(
                node,
                webui_pid,
                Kind::Plain,
                "open-webui",
                WEBUI_IMAGE,
                &[3000],
                &[3000],
                "bash start.sh",
                now - 6 * 86_400,
                "unless-stopped",
                &[("com.docker.compose.project", "webui"), ("com.docker.compose.service", "open-webui")],
                vec!["PORT=8080".into(), format!("OPENAI_API_BASE_URL=http://{}:8000/v1", fabric_ip(0, 0)), "WEBUI_AUTH=true".into()],
            );
            webui.c.ports = vec!["0.0.0.0:3000->8080/tcp".into()];
            webui.c.health = Some("healthy".into());
            vec![head, webui]
        }
        Role::RayWorker => {
            let pid = node.alloc_pid(rng);
            vec![make_container(
                node,
                pid,
                Kind::RayWorker,
                "vllm-worker",
                VLLM_IMAGE,
                &[],
                &[],
                &format!("ray start --address={}:6379 --node-ip-address={} --block", fabric_ip(0, 0), fabric_ip(node.idx, 0)),
                now - 2 * 86_400 - 7_260,
                "no",
                &compose("vllm-worker"),
                ray_env(node.idx, false),
            )]
        }
        Role::Ollama => {
            let ollama_pid = node.alloc_pid(rng);
            let sglang_pid = node.alloc_pid(rng);
            let mut ollama = make_container(
                node,
                ollama_pid,
                Kind::Ollama,
                "ollama",
                OLLAMA_IMAGE,
                &[11434],
                &[11434],
                "/bin/ollama serve",
                now - 9 * 86_400,
                "unless-stopped",
                &[],
                vec!["OLLAMA_HOST=0.0.0.0:11434".into(), "OLLAMA_KEEP_ALIVE=24h".into(), "NVIDIA_VISIBLE_DEVICES=all".into()],
            );
            ollama.c.labels.insert("org.opencontainers.image.version".into(), "0.12.6".into());
            let mut sglang = make_container(
                node,
                sglang_pid,
                Kind::Plain,
                "sglang-test",
                SGLANG_IMAGE,
                &[30000],
                &[30000],
                "python3 -m sglang.launch_server --model-path Qwen/Qwen3-32B --port 30000",
                now - 5 * 86_400,
                "no",
                &[],
                vec!["NVIDIA_VISIBLE_DEVICES=all".into()],
            );
            sglang.c.state = "exited".into();
            sglang.c.finished_at = now - 3 * 86_400 - 4_000;
            sglang.c.exit_code = 137;
            sglang.c.oom_killed = true;
            vec![ollama, sglang]
        }
    }
}

/// The processes every node runs outside containers: the real daemons of a DGX Spark, the
/// cluster user's systemd and SparkBox's collector (`found::host_procs`).
fn base_procs(node: &mut DemoNode, rng: &mut fastrand::Rng) -> Vec<DemoProc> {
    let world = node.world;
    found::host_procs(node, world, rng)
}

impl DemoCluster {
    /// The everyday world, which the app starts in (or the scenario SPARKBOX_DEMO_SCENARIO
    /// names, in a debug build: see `start_scenario`).
    pub fn new(inv: &Inventory) -> Self {
        let mut d = Self::with_world(inv, World::Everyday);
        if let Some(id) = start_scenario() {
            if let Err(e) = d.set_scenario(Some(&id)) {
                eprintln!("SPARKBOX_DEMO_SCENARIO: {e}");
            }
        }
        d
    }

    /// The cluster as SparkBox found it on Oct 29 (the scenario "as-found" playing), for every
    /// package's regression tests of what the Live tour showed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn as_found() -> Self {
        let mut d = Self::new(&Inventory::example());
        d.set_scenario(Some("as-found")).expect("as-found is a scenario");
        d
    }

    /// The everyday world with its readings starting at `t0_ms` (unix ms) instead of now: the
    /// traces it reads are the same on every run of a test.
    #[cfg(test)]
    pub fn starting_at(inv: &Inventory, t0_ms: u64) -> Self {
        Self::with_world_at(inv, World::Everyday, t0_ms)
    }

    fn with_world(inv: &Inventory, world: World) -> Self {
        // The warm-up ticks end at "now", so the first live reading meets the history.
        Self::with_world_at(inv, world, unix_now_ms().saturating_sub(20_000))
    }

    fn with_world_at(inv: &Inventory, world: World, t0_ms: u64) -> Self {
        let mut rng = fastrand::Rng::with_seed(0x5BA4C);
        let now = unix_now();
        let count = inv.nodes.len();
        let found_world = world == World::AsFound;

        let nodes = inv
            .nodes
            .iter()
            .enumerate()
            .map(|(i, cfg)| {
                let role = match i {
                    0 => Role::RayHead,
                    _ if i + 1 == count && count > 1 => Role::Ollama,
                    _ => Role::RayWorker,
                };
                let ollama = role == Role::Ollama;
                let real = world::real(&cfg.id, i);
                let mut node = DemoNode {
                    cfg: cfg.clone(),
                    idx: i,
                    role,
                    world,
                    real,
                    orphan_ports: if found_world { found::orphan_ports(&cfg.id) } else { Vec::new() },
                    phase: if ollama { 2.1 } else { i as f64 * 0.15 },
                    util: 0.0,
                    gpu_temp: real.gpu_c as f64,
                    temp_offset: real.gpu_c as f64 - 44.0,
                    cores: vec![0.0; CORES],
                    load: [0.2; 3],
                    disk_total: real.disk_total as f64,
                    disk_used: 0.0,
                    disk_base: real.disk_used as f64,
                    io: Io::default(),
                    disk_read: 0.0,
                    disk_write: 0.0,
                    boot_unix: if found_world {
                        world::found(FOUND_AT - real.uptime_s, now)
                    } else {
                        now - 86_400 * (3 + i as u64) - 3_600 * (2 * i as u64 + 5)
                    },
                    fabric_disabled: false,
                    counters: [Counters::default(); 2],
                    traffic: Traffic::default(),
                    ext_rx: 0.0,
                    ext_tx: 0.0,
                    containers: Vec::new(),
                    procs: Vec::new(),
                    units: Vec::new(),
                    updates: ops::base_updates(&cfg.id),
                    power: Power::On,
                    driver: DRIVER.into(),
                    kernel: KERNEL.into(),
                    pending_boot: None,
                    collector_pid: 0,
                    next_pid: 3_000 + 2_311 * i as u32,
                    ov: Overlay::default(),
                    frozen: None,
                    carrier_was: true,
                    engine: None,
                    roce: [fabric::boot_counters(real, 0, world), fabric::boot_counters(real, 1, world)],
                    traced_ms: None,
                    boots: 0,
                    kernel_events: if found_world { found::kernel_events(&cfg.id, now) } else { Vec::new() },
                    cabled_to: None,
                    // The Ray nodes got perftest when the fabric was set up; gx10-0000 never did.
                    perftest: !ollama,
                    idle_cap: i % 2 == 0,
                };
                node.disk_used = node.disk_base;
                // The cabled port has seen the recabling (9 carrier changes, 11 on gx10-0000) and
                // dropped a few frames since boot.
                for half in 0..2 {
                    node.counters[half] = Counters { carrier_changes: real.carrier, tx_drops: real.tx_drops[half], ..Default::default() };
                }
                node.procs = base_procs(&mut node, &mut rng);
                node.containers = if found_world { found::containers(&node, now) } else { base_containers(&mut node, &mut rng, now) };
                node.units = ops::base_units(&node.cfg.id, world, i == 0 && !found_world);
                ops::settle(&mut node);
                node
            })
            .collect();

        let switches = inv
            .switches
            .iter()
            .map(|cfg| {
                let mut ports = Vec::new();
                let boot_unix = now - 41 * 86_400 - 3_312;
                if cfg.kind == SwitchKind::RouterOs {
                    // Four physical sockets, one 200G node link per socket.
                    for i in 0..4 {
                        let node = (i < count).then_some(i);
                        let optic_base = [-1.9f32, -2.2, -2.0, -2.4];
                        ports.push(DemoPort {
                            name: crs_port(i),
                            comment: node.map(|n| inv.nodes[n].name.clone()).unwrap_or_default(),
                            speed: "200G",
                            node,
                            uplink: false,
                            disabled: false,
                            link_downs: match i {
                                1 => 2,
                                _ if node.is_some() => 1,
                                _ => 0,
                            },
                            last_up_unix: boot_unix,
                            rx_errors: if i == 1 { 3 } else { 0 },
                            fcs_errors: if i == 1 { 3 } else { 0 },
                            rx_drops: 0,
                            rx_pause: if node.is_some() { 1_204 + 311 * i as u64 } else { 0 },
                            lanes: (0..4).map(|l| optic_base[l] + ((i * 7 + l * 3) % 5) as f32 * 0.08).collect(),
                            tx_power: 0.6 + (i % 3) as f32 * 0.15,
                            ov_down: false,
                            ov_lane: None,
                            running_was: node.is_some(),
                            revert_at: None,
                            bounce_left: 0,
                        });
                    }
                    ports.push(DemoPort {
                        name: "ether1".into(),
                        comment: "management uplink to Firewalla".into(),
                        speed: "1G",
                        node: None,
                        uplink: true,
                        disabled: false,
                        link_downs: 1,
                        last_up_unix: boot_unix,
                        rx_errors: 0,
                        fcs_errors: 0,
                        rx_drops: 0,
                        rx_pause: 0,
                        lanes: Vec::new(),
                        tx_power: 0.0,
                        ov_down: false,
                        ov_lane: None,
                        running_was: true,
                        revert_at: None,
                        bounce_left: 0,
                    });
                }
                // In the everyday world every device is set up, at its usual address unless the
                // inventory names one; as found, none was (Topology draws them dashed).
                let host = if found_world {
                    String::new()
                } else if cfg.host.trim().is_empty() {
                    switch_mgmt_ip(&cfg.id).unwrap_or("").to_string()
                } else {
                    cfg.host.clone()
                };
                DemoSwitch { cfg: cfg.clone(), host, boot_unix, ports }
            })
            .collect();

        let mut demo = DemoCluster { world, tick: 0, t0_ms, rng, nodes, switches, ray_load: if found_world { 0.0 } else { 1.0 }, scenario: None };
        // Fill in the nodes' process lists for the ports they listen on, then warm up so the first frame isn't all zeros.
        for _ in 0..20 {
            demo.advance();
        }
        demo
    }

    /// Starts the simulator over in another world, on the same inventory and clock. What the
    /// old world's operations changed goes with it.
    fn rebuild(&mut self, world: World) {
        let inv = Inventory { nodes: self.nodes.iter().map(|n| n.cfg.clone()).collect(), switches: self.switches.iter().map(|s| s.cfg.clone()).collect() };
        let scenario = self.scenario.take();
        *self = Self::with_world(&inv, world);
        self.scenario = scenario;
    }

    /// Ticks since the simulator started (one per second in the app). Part of the C4 contract.
    #[allow(dead_code)]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Simulated time, Unix ms: what `trace::value_at` is asked about for the live readings.
    pub fn sim_ms(&self) -> u64 {
        self.t0_ms + self.tick * 1000
    }

    fn node_index(&self, id: &str) -> Result<usize, String> {
        self.nodes.iter().position(|n| n.cfg.id == id || n.cfg.name == id).ok_or_else(|| format!("unknown node {id}"))
    }

    fn ray_nodes(&self) -> impl Iterator<Item = &DemoNode> {
        self.nodes.iter().filter(|n| n.role != Role::Ollama)
    }

    /// Everything the pipeline needs is up: every Ray node on, its fabric linked, its container running.
    fn ray_ready(&self) -> bool {
        self.ray_nodes().count() > 0
            && self.ray_nodes().all(|n| {
                n.power == Power::On
                    && n.carrier()
                    && n.containers.iter().any(|c| c.is_ray() && c.active())
            })
    }

    /// The model is loaded and answering.
    fn ray_serving(&self) -> bool {
        self.ray_ready() && self.ray_load >= 1.0
    }

    /// Something stopped a Ray container (or its node): the engine dies with it. A worker
    /// lost means the head's `vllm serve` exits too, a few seconds later.
    fn ray_cascade(&mut self, from_head: bool) {
        self.ray_load = 0.0;
        if from_head {
            return;
        }
        let at = self.tick + 3;
        for n in self.nodes.iter_mut().filter(|n| n.role == Role::RayHead) {
            for c in n.containers.iter_mut().filter(|c| c.kind == Kind::RayHead && c.running() && c.exit_at.is_none()) {
                c.exit_at = Some((at, 1));
            }
        }
    }

    /// Moves the simulation forward one second.
    pub fn advance(&mut self) {
        self.tick += 1;
        let now = unix_now();
        for n in &mut self.nodes {
            n.ov = Overlay::default();
        }
        for s in &mut self.switches {
            for p in &mut s.ports {
                p.ov_down = false;
                p.ov_lane = None;
            }
        }
        scenarios::step(self);
        ops::power_step(self, now);
        ops::timers(self, now);
        self.port_timers();

        // Switch ports that are down take their node's fabric with them.
        for s in &self.switches {
            for p in &s.ports {
                if let (Some(i), true) = (p.node, p.ov_down) {
                    if let Some(n) = self.nodes.get_mut(i) {
                        n.ov.carrier_down = true;
                    }
                }
            }
        }

        // Due exits (the Ray cascade).
        let tick = self.tick;
        for n in &mut self.nodes {
            for c in &mut n.containers {
                if let Some((at, code)) = c.exit_at {
                    if at <= tick {
                        c.exit_at = None;
                        if c.running() {
                            c.c.state = "exited".into();
                            c.c.exit_code = code;
                            c.c.finished_at = now;
                            c.frozen = false;
                        }
                    }
                }
            }
        }

        // The model loads once everything is up, and is gone as soon as anything isn't.
        if self.ray_ready() {
            if self.ray_load < 1.0 {
                self.ray_load = (self.ray_load + 1.0 / LOAD_SECONDS).min(1.0);
            }
        } else if self.ray_load > 0.0 {
            self.ray_load = 0.0;
        }
        // A pipeline stage that has stopped answering (node-offline: cell3 hangs) stalls the whole
        // pipeline: no tokens and no NCCL traffic until it's back, as SparkBox then reports it Down.
        let stalled = self.ray_nodes().any(|n| matches!(n.ov.status, Some((LinkStatus::Offline | LinkStatus::Connecting, _))));
        let serving = self.ray_serving() && !stalled;
        let loading = self.ray_ready() && self.ray_load < 1.0;

        let t = self.tick as f64;
        let tick = self.tick;
        let sim_ms = self.sim_ms();
        // A prefill now and then: a long prompt flows through the pipeline at a few Gb/s (the
        // trace's, so the Demo history has the same bursts).
        let prefill = if serving { trace::prefill_at(sim_ms) } else { 0.0 };
        let ray_count = self.ray_nodes().count();
        let head_util = self.nodes.iter().find(|n| n.role == Role::RayHead).map(|n| n.util).unwrap_or(0.0);
        let rng = &mut self.rng;

        for n in &mut self.nodes {
            if n.power != Power::On {
                n.util = 0.0;
                n.traffic = Traffic::default();
                n.disk_read = 0.0;
                n.disk_write = 0.0;
                continue;
            }
            // GB10 at P8 flips its "SW power cap" reason on and off between samples.
            n.idle_cap = if rng.f64() < 0.85 { !n.idle_cap } else { n.idle_cap };
            let ray_active = n.role != Role::Ollama && serving && n.containers.iter().any(|c| c.is_ray() && c.active());
            let other_busy = n.containers.iter().any(|c| c.active() && c.gpu_mem > 0.0 && matches!(c.kind, Kind::Ollama | Kind::Injected));
            let engine = n.engine.map(|(e, at)| if self.tick.saturating_sub(at) <= ENGINE_HOLD_S && e.active { (e.util_pct as f64 / 100.0, e.steady) } else { (0.0, false) });
            // Each node's voice (trace): its own amplitude, lag and noise on the work it does.
            let (target, follow) = if n.ov.gpu_lost {
                (0.0, true)
            } else if let Some((_, true)) = engine {
                // Models serves requests on this GPU as usual: it reads its trace.
                (trace::util_at(&n.cfg.id, sim_ms), true)
            } else if let Some((u, _)) = engine {
                // Models drives this GPU (loading, warming up, hung): it works only while it says so.
                (if u > 0.0 { trace::voiced(&n.cfg.id, u.clamp(0.0, 1.0), sim_ms) } else { 0.0 }, false)
            } else if ray_active {
                (trace::util_at(&n.cfg.id, sim_ms), true)
            } else if other_busy && n.role == Role::Ollama {
                (trace::util_at(&n.cfg.id, sim_ms), true)
            } else if other_busy {
                (demand(t, n.phase, Role::RayHead) * 0.8, false)
            } else if loading && n.role != Role::Ollama {
                (0.06, false)
            } else {
                (0.0, false)
            };
            if follow && !n.ov.gpu_lost {
                // The everyday world reads exactly the trace, so its history joins without a jump.
                n.util = target;
                n.traced_ms = Some(sim_ms);
            } else {
                n.util += (target - n.util) * 0.35;
                n.traced_ms = None;
            }
            if n.ov.gpu_lost {
                // Off the bus: no work at all from the next reading on.
                n.util = 0.0;
            }
            // A blocked air path (thermal-throttle) holds the GPU at its slowdown point, 88-91 °C.
            let heat_target = if n.ov.hot { 89.5 + 1.4 * (t / 7.0).sin() } else { (44.0 + n.temp_offset + 0.6 * (t / 41.0 + n.phase).sin() + 36.0 * n.util).min(84.0) };
            n.gpu_temp += (heat_target - n.gpu_temp) * 0.12;

            // CPU: at rest a Spark's cores read 0 with a blip here and there (0.3–1% in all), plus
            // its daemons and SparkBox's collector; then each active container packed onto cores.
            for core in n.cores.iter_mut() {
                *core = if rng.f64() < 0.1 { 0.8 + rng.f64() * 3.6 } else { 0.0 };
            }
            for p in n.procs.iter().filter(|p| !p.stopped && p.base_cpu > 0.0) {
                n.cores[p.pid as usize % CORES] += (p.base_cpu * (0.6 + rng.f64() * 0.8)).max(0.0);
            }
            let mut k = 0;
            for dc in n.containers.iter_mut() {
                if !dc.active() {
                    dc.c.cpu_pct = 0.0;
                    continue;
                }
                let u = if dc.is_ray() { if ray_active { n.util } else { 0.0 } } else { n.util };
                let cpu = (dc.idle_cpu + dc.busy_cpu * u + jitter(rng, 2.0)).max(0.0);
                dc.c.cpu_pct = cpu as f32;
                dc.cpu_time += cpu / 100.0;
                let mut left = cpu;
                while left > 0.5 {
                    let take = left.min(97.0 + rng.f64() * 3.0);
                    n.cores[k % 10] += take;
                    left -= take;
                    k += 1;
                }
            }
            for p in n.procs.iter_mut().filter(|p| !p.stopped) {
                p.cpu_time += p.base_cpu / 100.0;
            }
            for core in n.cores.iter_mut() {
                *core = core.min(100.0);
            }
            // The load average counts waiting and runnable tasks too: 0.1–0.3 at rest.
            let busy_cores = n.cores.iter().sum::<f64>() / 100.0 + 0.12 + 0.08 * (t / 97.0 + n.phase).sin().abs();
            for (avg, window) in n.load.iter_mut().zip([60.0, 300.0, 900.0]) {
                *avg += (busy_cores - *avg) * (1.0 - (-1.0f64 / window).exp());
            }

            // Disk: at rest, ext4's commits every 5 s (tens of kB/s) and now and then a journal
            // flush of a few MB/s; working, a trickle with Poisson bursts; several GB/s only while
            // weights load.
            let at_rest = !n.containers.iter().any(|c| c.active());
            let io = &mut n.io;
            if io.read_left > 0 {
                io.read_left -= 1;
            } else if rng.f64() < 0.035 {
                io.read_left = 1 + rng.u32(0..3);
                io.read_rate = log_uniform(rng, 15e6, 260e6);
            }
            if io.write_left > 0 {
                io.write_left -= 1;
            } else if rng.f64() < 0.05 {
                io.write_left = 1 + rng.u32(0..4);
                io.write_rate = log_uniform(rng, 8e6, 180e6);
            }
            let (mut read, mut write) = if at_rest {
                let commit = (tick + n.idx as u64) % 5 == 0;
                let flush = rng.f64() < 0.015;
                (
                    if rng.f64() < 0.06 { 3_600.0 + rng.f64() * 3_600.0 } else { 0.0 },
                    if flush { log_uniform(rng, 0.6e6, 2.9e6) } else if commit { 20e3 + rng.f64() * 130e3 } else if rng.f64() < 0.2 { rng.f64() * 12e3 } else { 0.0 },
                )
            } else {
                (6e4 + rng.f64() * 3e5 + if io.read_left > 0 { io.read_rate } else { 0.0 }, 1.2e5 + rng.f64() * 6e5 + if io.write_left > 0 { io.write_rate } else { 0.0 })
            };
            if loading && n.role != Role::Ollama {
                read += 2.6e9 + rng.f64() * 0.7e9;
            }
            // Ollama loading a model (just started).
            if n.containers.iter().any(|c| c.kind == Kind::Ollama && c.running() && now.saturating_sub(c.c.started_at) < 15) {
                read += 1.9e9 + rng.f64() * 0.5e9;
            }
            // Fabric copies land on disk (and are read from it on the sender).
            read += n.ext_tx / 8.0;
            write += n.ext_rx / 8.0 + n.ov.disk_write;
            n.disk_read = read;
            n.disk_write = write;

            // Network. The pipeline runs cell1 → cell2 → cell3 → (tokens back to) cell1, mostly over RDMA.
            let up = n.carrier();
            let mut tr = Traffic::default();
            let decode = |rng: &mut fastrand::Rng, u: f64| (20e6 + 60e6 * u) * (0.9 + rng.f64() * 0.2);
            if ray_active && up {
                let stage = match n.role {
                    Role::RayHead => 0,
                    _ => n.idx.min(ray_count.saturating_sub(1)),
                };
                let last = stage + 1 == ray_count;
                let act = decode(rng, n.util) + prefill;
                let back = 0.4e6 + rng.f64() * 0.3e6;
                // What this stage sends on and receives from its neighbours.
                let (tx, rx) = match (stage, last) {
                    (0, _) => (act, back),
                    (_, true) => (back, act),
                    _ => (act, act * (0.97 + rng.f64() * 0.06)),
                };
                // Each node splits its NCCL traffic across the halves its own way, and Ray's
                // control plane and object store (TCP on the NCCL interface) run at its own level.
                let (split, tcp1, tcp2) = trace::fabric_mix(&n.cfg.id);
                tr.rdma_tx = [tx * split, tx * (1.0 - split)];
                tr.rdma_rx = [rx * split, rx * (1.0 - split)];
                // Half 2 carries Ray's object-store transfers; half 1 its control plane as well.
                tr.ip_tx = [(0.3e6 + rng.f64() * 0.9e6) * tcp1, (0.08e6 + rng.f64() * 0.3e6) * tcp2];
                tr.ip_rx = [(0.3e6 + rng.f64() * 0.9e6) * tcp1, (0.08e6 + rng.f64() * 0.3e6) * tcp2];
            } else if up {
                // At rest only broadcasts arrive (ARP, mDNS, an LLDP frame every 30 s), and both halves
                // of the port hear the same ones; nothing is sent.
                let lldp = (tick + n.idx as u64 * 7) % 30 == 0;
                let rx = if lldp { 2_400.0 + rng.f64() * 1_400.0 } else if rng.f64() < 0.4 { 250.0 + rng.f64() * 610.0 } else { 0.0 };
                tr.ip_rx = [rx, rx];
            }
            if up {
                tr.ip_rx[0] += n.ext_rx;
                tr.ip_tx[0] += n.ext_tx;
            }
            // LAN: API clients and Open WebUI on the head; SparkBox's own SSH everywhere: its
            // collector's frames are most of what a node at rest sends (95–126 kb/s).
            let collector = 95e3 + rng.f64() * 31e3;
            (tr.lan_rx, tr.lan_tx) = match n.role {
                Role::RayHead if serving => (0.4e6 + head_util * 2.2e6 + rng.f64() * 0.3e6, collector + head_util * 1.1e6),
                Role::Ollama if n.util > 0.05 => (0.2e6 + rng.f64() * 0.2e6, collector + n.util * 0.6e6),
                _ => (3e3 + rng.f64() * 27e3, collector),
            };
            tr.wifi_rx = 20e3 + rng.f64() * 40e3;
            n.traffic = tr;

            // Carrier changes are counted on every flap of the cabled port.
            if up != n.carrier_was {
                for c in &mut n.counters {
                    c.carrier_changes += 1;
                }
                if !up {
                    fabric::link_downed(&mut n.roce);
                }
                n.carrier_was = up;
            }
        }

        fabric::step(self);

        // Switch ports follow their nodes' links.
        for n in &mut self.nodes {
            n.cabled_to = None;
        }
        for s in &mut self.switches {
            for p in &mut s.ports {
                // A switch SparkBox can't log in to says nothing about who's cabled where.
                if let Some(n) = p.node.and_then(|i| self.nodes.get_mut(i)).filter(|_| !s.host.is_empty()) {
                    n.cabled_to = Some(format!("{}/{}", s.cfg.id, p.name));
                }
                let running = match p.node {
                    Some(i) => !p.disabled && !p.ov_down && self.nodes.get(i).is_some_and(|n| n.powered() && !n.ov.carrier_down),
                    None => p.uplink && !p.disabled && !p.ov_down,
                };
                if running && !p.running_was {
                    p.last_up_unix = now;
                }
                if !running && p.running_was && p.node.is_some() {
                    p.link_downs += 1;
                }
                p.running_was = running;
            }
        }
    }

    pub fn snapshot(&mut self) -> ClusterSnapshot {
        let now = unix_now();
        let now_ms = unix_now_ms();
        let tick = self.tick;
        let serving = self.ray_serving();
        let ray_load = self.ray_load;
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for i in 0..self.nodes.len() {
            let snap = {
                let rng = &mut self.rng;
                let n = &self.nodes[i];
                node_snapshot(n, now, now_ms, serving, ray_load, rng)
            };
            let n = &mut self.nodes[i];
            let snap = if n.ov.stale && n.online() {
                // The collector stopped sending: the numbers stay as they were, getting older.
                let (frozen, since, seen) = n.frozen.get_or_insert_with(|| (snap.clone(), tick, now_ms)).clone();
                NodeSnapshot { stale: true, frame_age_ms: Some((tick - since) * 1000 + 400), last_seen_ms: Some(seen), ..frozen }
            } else {
                n.frozen = None;
                snap
            };
            nodes.push(snap);
        }
        hear_the_lan(&mut nodes, self.world);
        let tables: Vec<_> = self.switches.iter().map(|s| fabric::switch_tables(self, &s.cfg.id)).collect();
        let cut_off: Vec<bool> = self.switches.iter().map(|s| self.uplink_down(&s.cfg.id)).collect();
        let rng = &mut self.rng;
        let switches = self
            .switches
            .iter()
            .zip(&tables)
            .zip(cut_off)
            .map(|((s, t), cut)| {
                let snap = switch_snapshot(s, &self.nodes, &nodes, t, now, rng);
                if !cut {
                    return snap;
                }
                // Its management port is down: the REST API doesn't answer until the switch re-enables it.
                SwitchSnapshot {
                    status: LinkStatus::Offline,
                    latency_ms: None,
                    error: Some(format!("No reply from {}:443 in 5 s.", s.host)),
                    ports: snap.ports.into_iter().map(|p| SwitchPort { rx_bps: 0.0, tx_bps: 0.0, ..p }).collect(),
                    ..snap
                }
            })
            .collect();
        ClusterSnapshot { ts_ms: now_ms, demo: true, nodes, switches, scenario: self.scenario(), scenario_phase: self.scenario_phase(), ..Default::default() }
    }

    /// A `docker inspect`-shaped description of a simulated container.
    pub fn container_inspect(&self, node_id: &str, container_id: &str) -> Result<serde_json::Value, String> {
        let node = &self.nodes[self.node_index(node_id)?];
        let dc = node
            .containers
            .iter()
            .find(|c| c.c.id == container_id || c.c.name == container_id)
            .ok_or_else(|| format!("Error: No such container: {container_id}"))?;
        let c = &dc.c;
        let t = |s: u64| if s == 0 { "0001-01-01T00:00:00Z".to_string() } else { ops::iso(s) };
        let host_net = dc.is_ray() || dc.kind == Kind::Injected || dc.inspect.pointer("/HostConfig/NetworkMode").and_then(|v| v.as_str()) == Some("host");
        let ports: serde_json::Map<String, serde_json::Value> = c
            .ports
            .iter()
            .filter_map(|p| {
                let (host, inner) = p.split_once("->")?;
                let host_port = host.rsplit(':').next()?;
                Some((inner.to_string(), json!([{ "HostIp": "0.0.0.0", "HostPort": host_port }])))
            })
            .collect();
        let health = c.health.as_ref().map(|h| json!({ "Status": h, "FailingStreak": 0, "Log": [] }));
        let full_id = dc.inspect.get("Id").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| container_full_id(&node.cfg.id, &c.name));
        let mounts = if c.image.contains("vllm") || dc.kind == Kind::Injected {
            json!([{ "Type": "bind", "Source": format!("{}/{MODELS_DIR}", home()), "Destination": "/models", "Mode": "ro", "RW": false, "Propagation": "rprivate" }])
        } else if dc.kind == Kind::Ollama {
            json!([{ "Type": "volume", "Name": "ollama", "Source": "/var/lib/docker/volumes/ollama/_data", "Destination": "/root/.ollama", "RW": true }])
        } else {
            json!([])
        };
        let mut doc = json!({
            "Id": full_id,
            "Created": t(c.started_at.saturating_sub(86_400)),
            "Path": c.command.split_whitespace().next().unwrap_or(""),
            "Args": c.command.split_whitespace().skip(1).collect::<Vec<_>>(),
            "State": {
                "Status": c.state, "Running": c.state == "running" || c.state == "paused", "Paused": c.state == "paused",
                "Restarting": c.state == "restarting", "OOMKilled": c.oom_killed, "Dead": false,
                "Pid": if c.state == "running" { dc.pid } else { 0 }, "ExitCode": c.exit_code,
                "Error": "", "StartedAt": t(c.started_at), "FinishedAt": t(c.finished_at), "Health": health,
            },
            "Image": format!("sha256:{}", container_full_id("image", &c.image)),
            "Name": format!("/{}", c.name),
            "RestartCount": c.restart_count,
            "Driver": "overlayfs",
            "Platform": "linux",
            "Config": {
                "Hostname": if host_net { node.cfg.host.trim_end_matches(".local").to_string() } else { c.id.clone() },
                "Image": c.image,
                "Cmd": c.command.split_whitespace().collect::<Vec<_>>(),
                "Env": dc.env,
                "Labels": c.labels,
            },
            "HostConfig": {
                "NetworkMode": if host_net { "host" } else { "bridge" },
                "IpcMode": if host_net { "host" } else { "private" },
                "ShmSize": if dc.is_ray() { 17_179_869_184u64 } else { 67_108_864 },
                "DeviceRequests": if dc.gpu_mem > 0.0 { json!([{ "Driver": "", "Count": -1, "Capabilities": [["gpu"]] }]) } else { json!(null) },
                "RestartPolicy": { "Name": c.restart_policy.clone().unwrap_or_else(|| "no".into()), "MaximumRetryCount": 0 },
            },
            "Mounts": mounts,
            "NetworkSettings": {
                "Ports": ports,
                "Networks": if host_net { json!({ "host": { "IPAddress": "" } }) } else { json!({ "bridge": { "IPAddress": format!("172.17.0.{}", 2 + node.containers.iter().position(|x| x.c.id == c.id).unwrap_or(0)), "Gateway": "172.17.0.1" } }) },
            },
            "SparkBoxDemo": true,
        });
        // A container's own networks replace the generated ones rather than join them.
        if dc.inspect.pointer("/NetworkSettings/Networks").is_some() {
            if let Some(n) = doc.pointer_mut("/NetworkSettings") {
                n["Networks"] = json!({});
            }
        }
        merge(&mut doc, &dc.inspect);
        Ok(doc)
    }

    /// What `docker logs <container>` prints for a container that has stopped, and when it
    /// stopped (Unix s); None while it runs (Terminal then follows it) or for an unknown name.
    pub fn container_log(&self, node_id: &str, container: &str) -> Option<(String, u64)> {
        let node = &self.nodes[self.node_index(node_id).ok()?];
        let dc = node.containers.iter().find(|c| c.c.id == container || c.c.name == container || (container.len() >= 12 && c.c.id.starts_with(&container[..12])))?;
        if dc.running() || dc.c.state == "paused" || dc.c.state == "restarting" {
            return None;
        }
        let text = dc.log.clone().unwrap_or_else(|| format!("{} exited with code {}\n", dc.proc_name, dc.c.exit_code));
        Some((text, dc.c.finished_at))
    }

    /// `docker image ls` on a node (the images the Images window lists).
    pub fn images(&self, node_id: &str) -> Result<Vec<DemoImage>, String> {
        let i = self.node_index(node_id)?;
        if !self.nodes[i].online() {
            return Err(format!("{} isn't connected.", self.nodes[i].cfg.name));
        }
        let n = &self.nodes[i];
        let now = unix_now();
        Ok(match self.world {
            World::AsFound => found::images(&n.cfg.id, now),
            World::Everyday => {
                // Every image a container here was made from, built a few weeks back.
                let mut seen = Vec::new();
                n.containers
                    .iter()
                    .filter(|c| !seen.contains(&c.c.image) && { seen.push(c.c.image.clone()); true })
                    .map(|c| {
                        let (repository, tag) = c.c.image.rsplit_once(':').unwrap_or((&c.c.image, "latest"));
                        let bytes = if c.c.image.contains("vllm") { 19_874_201_600 } else if c.c.image.contains("sglang") { 24_311_652_352 } else if c.c.image.contains("ollama") { 3_472_883_712 } else { 4_318_920_704 };
                        DemoImage {
                            repository: repository.into(),
                            tag: tag.into(),
                            id: container_full_id("image", &c.c.image)[..12].to_string(),
                            created_at: ops::docker_created(c.c.started_at.saturating_sub(21 * 86_400)),
                            bytes,
                            size: format!("{:.1}GB", bytes as f64 / 1e9),
                        }
                    })
                    .collect()
            }
        })
    }

    /// The world playing: the everyday showcase, or the cluster as found.
    pub fn world(&self) -> World {
        self.world
    }

    /// Docker actions on a simulated container, with Docker's own refusals.
    pub fn container_action(&mut self, node_id: &str, container_id: &str, action: &str) -> Result<(), String> {
        let i = self.node_index(node_id)?;
        if !self.nodes[i].online() {
            return Err(format!("{} isn't connected.", self.nodes[i].cfg.name));
        }
        let now = unix_now();
        let tick = self.tick;
        let node = &mut self.nodes[i];
        let dc = node.container_mut(container_id).ok_or_else(|| format!("Error response from daemon: No such container: {container_id}"))?;
        let name = dc.c.name.clone();
        let was_active = dc.active();
        let mut lost: Option<bool> = None;
        match action {
            "start" => {
                if dc.running() || dc.c.state == "paused" {
                    return Ok(());
                }
                dc.c.state = "running".into();
                dc.c.started_at = now;
                dc.c.exit_code = 0;
                dc.c.oom_killed = false;
                dc.manual_stop = false;
                dc.frozen = false;
                if dc.c.health.is_some() {
                    dc.c.health = Some("starting".into());
                }
            }
            "stop" | "kill" => {
                if !dc.running() && dc.c.state != "paused" && dc.c.state != "restarting" {
                    if action == "kill" {
                        return Err(format!("Error response from daemon: cannot kill container: {name}: container {} is not running", dc.c.id));
                    }
                    return Ok(());
                }
                dc.c.state = "exited".into();
                dc.c.finished_at = now;
                dc.c.exit_code = if action == "kill" { 137 } else { 0 };
                dc.manual_stop = true;
                dc.frozen = false;
                dc.exit_at = None;
                if dc.is_ray() {
                    lost = Some(dc.kind == Kind::RayHead);
                }
            }
            "restart" => {
                dc.c.state = "running".into();
                dc.c.started_at = now;
                dc.c.exit_code = 0;
                dc.manual_stop = false;
                dc.frozen = false;
                dc.exit_at = None;
                if dc.c.health.is_some() {
                    dc.c.health = Some("starting".into());
                }
                if dc.is_ray() {
                    lost = Some(dc.kind == Kind::RayHead);
                }
            }
            "pause" => {
                if !dc.running() {
                    return Err(format!("Error response from daemon: container {} is not running", dc.c.id));
                }
                dc.c.state = "paused".into();
                if dc.is_ray() {
                    lost = Some(dc.kind == Kind::RayHead);
                }
            }
            "unpause" => {
                if dc.c.state != "paused" {
                    return Err(format!("Error response from daemon: container {} is not paused", dc.c.id));
                }
                dc.c.state = "running".into();
            }
            "remove" => {
                if dc.running() || dc.c.state == "paused" {
                    return Err(format!(
                        "Error response from daemon: cannot remove container \"/{name}\": container is running: stop the container before removing or force remove"
                    ));
                }
                node.containers.retain(|c| c.c.name != name);
            }
            other => return Err(format!("unsupported action {other}")),
        }
        let _ = (was_active, tick);
        if let Some(head) = lost {
            self.ray_cascade(head);
        }
        Ok(())
    }

    /// Models' request simulator reports what an engine is doing on a node (one call per
    /// pipeline stage). Without a fresh call for 10 s the GPU goes back to idle.
    pub fn set_engine_load(&mut self, node: &str, load: EngineLoad) {
        let tick = self.tick;
        if let Some(n) = self.nodes.iter_mut().find(|n| n.cfg.id == node || n.cfg.name == node) {
            n.engine = Some((load, tick));
        }
    }

    /// Enables or disables a port. A disable reverts on the switch after `revert_s`
    /// seconds unless `keep_port_disabled` is called first (0: no revert).
    pub fn set_port_disabled_for(&mut self, switch_id: &str, port: &str, disabled: bool, revert_s: u32) -> Result<Option<u64>, String> {
        let tick = self.tick;
        let sw = self.switches.iter_mut().find(|s| s.cfg.id == switch_id).ok_or_else(|| format!("unknown switch {switch_id}"))?;
        let p = sw.ports.iter_mut().find(|p| p.name == port).ok_or_else(|| format!("unknown port {port}"))?;
        p.disabled = disabled;
        p.revert_at = (disabled && revert_s > 0).then(|| (tick + revert_s as u64, unix_now_ms() + revert_s as u64 * 1000));
        let at = p.revert_at.map(|(_, ms)| ms);
        let node = p.node;
        if let Some(i) = node {
            let was_ray = self.nodes[i].role != Role::Ollama;
            self.nodes[i].fabric_disabled = disabled;
            if disabled && was_ray {
                self.ray_cascade(true);
            }
        }
        Ok(at)
    }

    /// No revert, and the management uplink refused outright (the scenario tests use it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_port_disabled(&mut self, switch_id: &str, port: &str, disabled: bool) -> Result<(), String> {
        let uplink = self.switches.iter().find(|s| s.cfg.id == switch_id).and_then(|s| s.ports.iter().find(|p| p.name == port)).is_some_and(|p| p.uplink);
        if uplink && disabled {
            return Err("That port carries SparkBox's own connection to the switch.".into());
        }
        self.set_port_disabled_for(switch_id, port, disabled, 0).map(drop)
    }

    /// Cancels a pending revert: the port stays disabled.
    pub fn keep_port_disabled(&mut self, switch_id: &str, port: &str) -> Result<(), String> {
        let sw = self.switches.iter_mut().find(|s| s.cfg.id == switch_id).ok_or_else(|| format!("unknown switch {switch_id}"))?;
        let p = sw.ports.iter_mut().find(|p| p.name == port).ok_or_else(|| format!("unknown port {port}"))?;
        if !p.disabled {
            return Err(format!("{port} isn't disabled."));
        }
        p.revert_at = None;
        Ok(())
    }

    /// The switch takes the port down for 3 s and brings it back itself (an on-device script).
    pub fn bounce_port(&mut self, switch_id: &str, port: &str) -> Result<(), String> {
        let sw = self.switches.iter_mut().find(|s| s.cfg.id == switch_id).ok_or_else(|| format!("unknown switch {switch_id}"))?;
        let p = sw.ports.iter_mut().find(|p| p.name == port).ok_or_else(|| format!("unknown port {port}"))?;
        if p.disabled {
            return Err(format!("{port} is disabled; enable it instead."));
        }
        p.bounce_left = 3;
        let node = p.node;
        if node.is_some_and(|i| self.nodes[i].role != Role::Ollama) {
            self.ray_cascade(true);
        }
        Ok(())
    }

    /// Runs the switch's own timers: due reverts re-enable their port, bounces count down.
    fn port_timers(&mut self) {
        let tick = self.tick;
        let mut enable = Vec::new();
        for s in &mut self.switches {
            for p in &mut s.ports {
                if p.revert_at.is_some_and(|(t, _)| t <= tick) {
                    enable.push((s.cfg.id.clone(), p.name.clone()));
                }
                if p.bounce_left > 0 {
                    p.bounce_left -= 1;
                    p.ov_down = true;
                }
            }
        }
        for (sw, port) in enable {
            let _ = self.set_port_disabled_for(&sw, &port, false, 0);
        }
    }

    /// SparkBox can't reach the switch while its management uplink is down.
    fn uplink_down(&self, switch_id: &str) -> bool {
        self.switches.iter().find(|s| s.cfg.id == switch_id).is_some_and(|s| s.ports.iter().any(|p| p.uplink && (p.disabled || p.bounce_left > 0)))
    }

    /// Edits a switch port's comment (which names the node cabled to it).
    pub fn set_port_comment(&mut self, switch_id: &str, port: &str, comment: &str) -> Result<(), String> {
        let sw = self.switches.iter_mut().find(|s| s.cfg.id == switch_id).ok_or_else(|| format!("unknown switch {switch_id}"))?;
        let p = sw.ports.iter_mut().find(|p| p.name == port).ok_or_else(|| format!("unknown port {port}"))?;
        p.comment = comment.trim().to_string();
        Ok(())
    }

    /// Zeroes a port's error counters, as `/interface/reset-counters` does.
    pub fn reset_port_counters(&mut self, switch_id: &str, port: &str) -> Result<(), String> {
        let sw = self.switches.iter_mut().find(|s| s.cfg.id == switch_id).ok_or_else(|| format!("unknown switch {switch_id}"))?;
        let p = sw.ports.iter_mut().find(|p| p.name == port).ok_or_else(|| format!("unknown port {port}"))?;
        p.rx_errors = 0;
        p.fcs_errors = 0;
        p.rx_drops = 0;
        p.rx_pause = 0;
        p.link_downs = 0;
        Ok(())
    }
}

/// Target GPU utilization for a node that is serving.
fn demand(t: f64, phase: f64, role: Role) -> f64 {
    match role {
        // Chat use: idle most of the time with short busy spells.
        Role::Ollama => {
            let s = (t / 53.0 + phase).sin() + 0.5 * (t / 17.0).sin();
            if s > 0.9 {
                0.75 + 0.2 * (t / 3.0).sin()
            } else {
                0.0
            }
        }
        // Pipeline-parallel serving keeps all Ray nodes busy together, with an occasional lull.
        _ => {
            if (t / 240.0).sin() > 0.88 {
                return 0.02;
            }
            (0.55 + 0.25 * (t / 37.0 + phase).sin() + 0.12 * (t / 9.0 + phase).sin()).clamp(0.0, 1.0)
        }
    }
}

/// Lays `over` onto `doc`: objects merge key by key, anything else replaces.
fn merge(doc: &mut serde_json::Value, over: &serde_json::Value) {
    match (doc, over) {
        (serde_json::Value::Object(d), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                match d.get_mut(k) {
                    Some(slot) if slot.is_object() && v.is_object() => merge(slot, v),
                    _ => {
                        d.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (_, serde_json::Value::Null) => {}
        (d, o) => *d = o.clone(),
    }
}

/// The environment the NCCL transport check reads: NCCL_* and VLLM_HOST_IP only.
pub(crate) fn env_flags(env: &[String]) -> BTreeMap<String, String> {
    env.iter()
        .filter_map(|kv| kv.split_once('='))
        .filter(|(k, _)| k.starts_with("NCCL_") || *k == "VLLM_HOST_IP")
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The inventory's model with the board's DMI platform, as Live shows it: "MSI EdgeXpert (MS-C931)".
fn model_name(n: &DemoNode) -> String {
    if n.cfg.model.contains('(') || n.cfg.model.is_empty() {
        n.cfg.model.clone()
    } else {
        format!("{} ({})", n.cfg.model, n.real.platform)
    }
}

/// What the Live backend shows for a node it can't read: who it is and why, no numbers.
fn bare_snapshot(n: &DemoNode, status: LinkStatus, error: Option<String>, power_state: Option<&str>) -> NodeSnapshot {
    NodeSnapshot {
        id: n.cfg.id.clone(),
        name: n.cfg.name.clone(),
        host: n.cfg.ip.clone().unwrap_or_else(|| n.cfg.host.clone()),
        status,
        error,
        system: SystemInfo { hostname: n.cfg.host.trim_end_matches(".local").into(), model: model_name(n), ..Default::default() },
        power_state: power_state.map(Into::into),
        ..Default::default()
    }
}

/// Synthetic fingerprints model a shared-key group and a distinct-key node.
/// They are illustrative hashes, not identities captured from real equipment.
fn host_key(n: &DemoNode) -> HostKeyInfo {
    const CELLS: &str = "SHA256:BZ5Ax5nfR3u/LCFrjzg3eroa9t08yevI46BSyXedAbg";
    const GX10: &str = "SHA256:nIaTibRJmp5ZwhYU6VwFEDVsD0+j5agpqky/U9s2yC0";
    let cell = n.real.platform == "MS-C931";
    HostKeyInfo {
        address: format!("{}:{}", n.cfg.ip.clone().unwrap_or_else(|| n.cfg.host.clone()), n.cfg.port),
        fingerprint: if cell { CELLS } else { GX10 }.into(),
        shared_with: if cell { ["cell1", "cell2", "cell3"].iter().filter(|c| **c != n.cfg.id).map(|c| c.to_string()).collect() } else { Vec::new() },
    }
}

/// Who can reach a listening address: this node only, every interface, the LAN or the fabric.
fn scope_of(addr: &str) -> &'static str {
    if addr.starts_with("127.") || addr == "::1" {
        "local"
    } else if addr.starts_with("192.168.100.") || addr.starts_with("192.168.101.") {
        "fabric"
    } else if addr.starts_with("192.168.") {
        "lan"
    } else {
        "all"
    }
}

/// Folds `ss -tln`'s sockets into one row per port, the way Live reads them without root:
/// the IPv4 address shown, the family "both" when the port is bound on IPv6 too.
fn listeners_of(sockets: &[(u16, String, Option<(u32, String)>)]) -> Vec<Listener> {
    let mut out: Vec<Listener> = Vec::new();
    for (port, addr, owner) in sockets {
        let v6 = addr.contains(':');
        if let Some(l) = out.iter_mut().find(|l| l.port == *port) {
            let fam = l.family.as_deref().unwrap_or("v4");
            if (fam == "v4" && v6) || (fam == "v6" && !v6) {
                l.family = Some("both".into());
            }
            // A dual-stack row shows its IPv4 address; a wider scope wins.
            if !v6 && l.addr.contains(':') {
                l.addr = addr.clone();
            }
            if scope_of(addr) == "all" {
                l.scope = Some("all".into());
            }
            continue;
        }
        out.push(Listener {
            port: *port,
            addr: addr.clone(),
            pid: owner.as_ref().map(|o| o.0),
            process: owner.as_ref().map(|o| o.1.clone()),
            scope: Some(scope_of(addr).into()),
            family: Some(if v6 { "v6" } else { "v4" }.into()),
        });
    }
    out.sort_by_key(|x| x.port);
    out
}

/// The cumulative counters of a CX-7 port with no cable in: all zero, under the same QoS.
fn unused_roce(world: World) -> Option<RoceCounters> {
    (world == World::AsFound).then(|| RoceCounters {
        ecn_marked: Some(0),
        cnp_sent: Some(0),
        cnp_handled: Some(0),
        cnp_ignored: Some(0),
        out_of_sequence: Some(0),
        packet_seq_err: Some(0),
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
        rx_pause_ctrl: Some(0),
        tx_pause_ctrl: Some(0),
        rx_discards_phy: Some(0),
        rx_crc_errors_phy: Some(0),
        rx_symbol_err_phy: Some(0),
        rx_out_of_buffer: Some(0),
        trust: Some("pcp".into()),
        pfc_prios: Some(Vec::new()),
        roce_v2: Some(true),
        ..Default::default()
    })
}

fn node_snapshot(n: &DemoNode, now: u64, now_ms: u64, serving: bool, ray_load: f64, rng: &mut fastrand::Rng) -> NodeSnapshot {
    match n.power {
        Power::On => {}
        Power::Rebooting { down: 1.., .. } => return bare_snapshot(n, LinkStatus::Connecting, Some("Rebooting…".into()), Some("rebooting")),
        Power::Rebooting { off: 1.., .. } => {
            return bare_snapshot(n, LinkStatus::Offline, Some("Rebooting… waiting for the node to come back".into()), Some("rebooting"))
        }
        Power::Rebooting { .. } => return bare_snapshot(n, LinkStatus::Connecting, Some("Reconnecting after the reboot…".into()), Some("rebooting")),
        Power::ShuttingDown { .. } => return bare_snapshot(n, LinkStatus::Connecting, Some("Shutting down…".into()), Some("shutting-down")),
        Power::Off => return bare_snapshot(n, LinkStatus::Offline, Some("Powered off. Use Wake to start it.".into()), Some("off")),
        Power::Waking { off: 1.., .. } => return bare_snapshot(n, LinkStatus::Offline, Some("Starting up…".into()), Some("waking")),
        Power::Waking { .. } => return bare_snapshot(n, LinkStatus::Connecting, Some("Reconnecting…".into()), Some("waking")),
    }
    if let Some((status, error)) = &n.ov.status {
        if *status != LinkStatus::Online {
            return bare_snapshot(n, *status, error.clone(), None);
        }
    }

    let real = n.real;
    let found_world = n.world == World::AsFound;
    let util = n.util;
    let tr = &n.traffic;
    // GPU memory: Ray engines hold theirs only while the model is (being) loaded.
    let gpu_of = |c: &DemoContainer| -> f64 {
        if !c.running() && c.c.state != "paused" {
            return 0.0;
        }
        match c.kind {
            Kind::RayHead | Kind::RayWorker => c.gpu_mem * ray_load,
            _ => c.gpu_mem,
        }
    };
    let live: Vec<&DemoContainer> = n.containers.iter().filter(|c| c.running() || c.c.state == "paused").collect();
    let gpu_alloc: f64 = live.iter().map(|c| gpu_of(c)).sum();
    // The node at rest uses what the real one does (its daemons included); containers add theirs.
    let mem_total = real.mem_total as f64;
    let rss: f64 = live.iter().map(|c| c.rss).sum();
    let used = (real.mem_used as f64 + rss + gpu_alloc + n.ov.mem_extra + jitter(rng, 0.04 * GIB)).min(mem_total * 0.995);
    let cached = (real.mem_cached as f64 + jitter(rng, 0.02 * GIB)).min((mem_total - used - 0.5 * GIB).max(0.2 * GIB));

    let total_pct = n.cores.iter().sum::<f64>() / CORES as f64;
    let active = util > 0.02;
    let hot = n.gpu_temp >= 87.0;
    let clock_max = 3_003u32;
    // At rest GB10 sits at P8 with its SM at 208 MHz; working, it boosts.
    let mut clock = if active { 2_418 - rng.u32(0..120) } else { 208 };
    let mut throttle = Vec::new();
    if hot {
        clock = (clock as f64 * 0.7) as u32;
        throttle = vec!["HW thermal slowdown".to_string(), "SW power cap".to_string()];
    } else if !active && n.idle_cap {
        // Idle GB10 reports "SW power cap" every other sample: it isn't throttling anything.
        throttle = vec!["SW power cap".to_string()];
    }
    let idle_w = real.gpu_w as f64;
    // At rest GB10 reads 3.7–5.4 W, each node its own level (trace::idle_gpu_at).
    let mut power = if active { (idle_w + 82.0 * util + jitter(rng, 1.2)).max(idle_w) as f32 } else { trace::idle_gpu_at(&n.cfg.id, now_ms).0 };
    // An engine that reports its own draw wins while it's generating.
    if let Some((EngineLoad { power_w: Some(w), active: true, .. }, _)) = n.engine {
        if util > 0.02 {
            power = (w + jitter(rng, 0.8) as f32).max(idle_w as f32);
        }
    }
    if let (Some(ms), false) = (n.traced_ms, hot) {
        power = trace::value_at(&n.cfg.id, "gpuPowerW", ms).unwrap_or(power);
    }
    if hot {
        power = power.min(GB10_CAP_W - 1.5 + rng.f32());
    }

    let fabric_up = n.carrier();
    let lan_wol = if found_world { None } else { Some("g".into()) };
    let mut nets = vec![NetStats {
        iface: LAN_IFACE.into(),
        role: NetRole::Lan,
        up: true,
        speed_mbps: LAN_SPEED_MBPS,
        rx_bps: tr.lan_rx,
        tx_bps: tr.lan_tx,
        ip: n.cfg.ip.clone(),
        mtu: Some(1500),
        mac: Some(mac(&n.cfg.id, LAN_IFACE)),
        carrier_changes: Some(2),
        lan_wol,
        ..Default::default()
    }];
    for half in 0..2 {
        let (iface, _, rdma) = FABRIC[half];
        let (unused, unused_rdma) = FABRIC_UNUSED[half];
        // Port 1 of the CX-7 has no cable: present, down, no address.
        nets.push(NetStats {
            iface: unused.into(),
            role: NetRole::Fabric,
            mtu: Some(1500),
            mac: Some(mac(&n.cfg.id, unused)),
            carrier_changes: Some(2),
            rdma_dev: Some(unused_rdma.into()),
            rdma_rx_bps: Some(0.0),
            rdma_tx_bps: Some(0.0),
            roce: unused_roce(n.world),
            ..Default::default()
        });
        let c = n.counters[half];
        nets.push(NetStats {
            iface: iface.into(),
            role: NetRole::Fabric,
            up: fabric_up,
            speed_mbps: if fabric_up { FABRIC_SPEED_MBPS } else { 0 },
            rx_bps: tr.ip_rx[half],
            tx_bps: tr.ip_tx[half],
            ip: Some(fabric_ip(n.idx, half)),
            mtu: Some(n.ov.mtu[half].unwrap_or(FABRIC_MTU)),
            mac: Some(mac(&n.cfg.id, iface)),
            rx_errors: c.rx_errors,
            tx_errors: c.tx_errors,
            rx_drops: c.rx_drops,
            tx_drops: c.tx_drops,
            carrier_changes: Some(c.carrier_changes),
            rdma_dev: Some(rdma.into()),
            rdma_rx_bps: Some(tr.rdma_rx[half]),
            rdma_tx_bps: Some(tr.rdma_tx[half]),
            switch_port: n.cabled_to.clone(),
            roce: Some(n.roce[half].clone()),
            lan_wol: None,
            ..Default::default()
        });
    }
    // The Wi-Fi radio is on every node; cell3's joined the home LAN too (a second address on the /24).
    let (wifi_changes, wifi_ip) = real.wifi;
    let wifi_up = wifi_ip.is_some();
    nets.push(NetStats {
        iface: WIFI_IFACE.into(),
        role: NetRole::Wifi,
        up: wifi_up,
        ip: wifi_ip.map(Into::into),
        rx_bps: if wifi_up { tr.wifi_rx } else { 0.0 },
        mac: Some(mac(&n.cfg.id, WIFI_IFACE)),
        mtu: Some(1500),
        carrier_changes: Some(wifi_changes),
        tx_drops: if wifi_up { 7 } else { 0 },
        ..Default::default()
    });

    let containers = n
        .containers
        .iter()
        .map(|dc| {
            let mut c = dc.c.clone();
            c.env_flags = env_flags(&dc.env);
            if dc.running() || c.state == "paused" {
                c.mem = (dc.rss + jitter(rng, 0.02 * dc.rss)) as u64;
                c.gpu_mem = gpu_of(dc) as u64;
                // vLLM's health check fails until the weights are in.
                if dc.is_ray() && c.health.is_some() {
                    c.health = Some(if serving { "healthy" } else if ray_load > 0.0 { "starting" } else { "unhealthy" }.into());
                }
            }
            c
        })
        .collect();

    let boot_ticks = |started: u64| started.saturating_sub(n.boot_unix) * HZ;
    let mut processes: Vec<Process> = Vec::new();
    for c in n.containers.iter().filter(|c| c.running() || c.c.state == "paused") {
        let gpu = gpu_of(c);
        processes.push(Process {
            pid: c.pid,
            name: c.proc_name.clone(),
            user: "root".into(),
            cmd: c.c.command.clone(),
            cpu_pct: c.c.cpu_pct,
            rss: c.rss as u64,
            nice: c.nice,
            container: Some(c.c.name.clone()),
            ports: c.listen.clone(),
            gpu_mem: gpu as u64,
            state: Some(if c.frozen || c.c.state == "paused" { "T" } else if c.c.cpu_pct > 50.0 { "R" } else { "S" }.into()),
            ppid: Some(c.pid - 22),
            threads: Some(c.threads),
            start_time: Some(boot_ticks(c.c.started_at)),
            cpu_time_s: Some(c.cpu_time + now.saturating_sub(c.c.started_at) as f64 * c.idle_cpu / 100.0),
            comm: Some(c.proc_name.chars().take(15).collect()),
            ..Default::default()
        });
    }
    for p in &n.procs {
        let cpu = if p.stopped { 0.0 } else { (p.base_cpu + jitter(rng, p.base_cpu * 0.3)).max(0.0) };
        // Without root, ss names the sockets of the cluster user's own processes only.
        let mut ports: Vec<u16> = if p.user == n.cfg.user { p.ports.iter().map(|(port, _)| *port).collect() } else { Vec::new() };
        ports.dedup();
        processes.push(Process {
            pid: p.pid,
            name: p.name.clone(),
            user: p.user.clone(),
            cmd: p.cmd.clone(),
            cpu_pct: cpu as f32,
            rss: p.rss as u64,
            nice: p.nice,
            unit: p.unit.clone(),
            ports,
            state: Some(if p.stopped { "T" } else if p.kernel { "I" } else if p.sparkbox && p.name == "python3" { "R" } else { "S" }.into()),
            ppid: Some(p.ppid),
            threads: Some(p.threads),
            start_time: Some(p.started_s * HZ),
            cpu_time_s: Some(p.cpu_time + now.saturating_sub(n.boot_unix + p.started_s) as f64 * p.base_cpu / 100.0),
            // /proc/<pid>/comm keeps 15 characters ("dashboard-servi").
            comm: Some(p.name.chars().take(15).collect()),
            kernel: p.kernel,
            sparkbox: p.sparkbox,
            ..Default::default()
        });
    }
    // Everything else on the node (kworkers, sessions, short-lived children) is counted, not sent.
    let proc_total = processes.len() as u32 + 344 + 9 * n.idx as u32;

    // `ss -tln` without root names only the cluster user's own sockets: none of these.
    let mut sockets: Vec<(u16, String, Option<(u32, String)>)> = Vec::new();
    for p in n.procs.iter().filter(|p| !p.stopped) {
        let own = p.user == n.cfg.user;
        for (port, addr) in &p.ports {
            if let Some(addr) = found::port_address(&n.cfg.id, *port, addr) {
                sockets.push((*port, addr.to_string(), own.then(|| (p.pid, p.name.clone()))));
            }
        }
    }
    for c in n.containers.iter().filter(|c| c.running() || c.c.state == "paused") {
        for port in &c.listen {
            // vLLM's API comes up only once the model is loaded.
            if c.kind == Kind::RayHead && *port == 8000 && !serving {
                continue;
            }
            sockets.push((*port, "0.0.0.0".into(), None));
        }
    }
    for (port, addr) in &n.orphan_ports {
        sockets.push((*port, addr.to_string(), None));
    }
    let listeners = listeners_of(&sockets);

    let failed_units = n.units.iter().filter(|u| u.active == "failed").map(|u| u.unit.clone()).collect();
    // The collector's GPU error, as in Live (nvidia-smi can't reach a GPU that's off the bus).
    let gpu_error = n.ov.gpu_lost.then(|| "GPU has fallen off the bus (Xid 79)".to_string());
    let latency = n.ov.latency_ms.unwrap_or(11.0 + rng.f64() * 18.0 + n.idx as f64 * 0.6 + if rng.f64() < 0.08 { 30.0 } else { 0.0 });
    let frame_age = rng.u64(80..900) + n.ov.latency_ms.map(|l| l as u64).unwrap_or(0);
    let gpu_c = real.gpu_c as f64;

    NodeSnapshot {
        id: n.cfg.id.clone(),
        name: n.cfg.name.clone(),
        host: n.cfg.ip.clone().unwrap_or_else(|| n.cfg.host.clone()),
        status: LinkStatus::Online,
        latency_ms: Some(latency as f32),
        error: n.ov.gpu_lost.then(|| "GPU has fallen off the bus (Xid 79)".to_string()),
        cpu: CpuStats {
            total_pct: total_pct as f32,
            per_core: n.cores.iter().map(|&c| (c * 10.0).round() as f32 / 10.0).collect(),
            load: n.load.map(|l| (l * 100.0).round() as f32 / 100.0),
            temp_c: (real.cpu_c as f64 + (n.gpu_temp - gpu_c) * 0.7 + total_pct * 0.08 + jitter(rng, 0.3)) as f32,
        },
        mem: MemStats {
            total: real.mem_total,
            used: used as u64,
            available: (mem_total - used) as u64,
            cached: cached as u64,
            gpu_alloc: gpu_alloc as u64,
            swap_total: SWAP_TOTAL,
            swap_used: real.swap_used,
            ..Default::default()
        },
        gpu: GpuStats {
            util_pct: if active { (util * 100.0 + jitter(rng, 2.0)).clamp(0.0, 100.0) as f32 } else { 0.0 },
            temp_c: (n.gpu_temp + jitter(rng, 0.3)).round() as f32,
            power_w: (power * 100.0).round() / 100.0,
            sm_clock_mhz: clock,
            procs: live
                .iter()
                .filter(|c| gpu_of(c) > 0.0)
                .map(|c| GpuProc { pid: c.pid, name: c.proc_name.clone(), mem: gpu_of(c) as u64, container: Some(c.c.name.clone()) })
                .collect(),
            clock_max_mhz: Some(clock_max),
            // nvidia-smi reports GB10's power limit as N/A.
            power_limit_w: None,
            pstate: Some(if util > 0.4 { "P0" } else if active { "P2" } else { "P8" }.into()),
            throttle,
            xid_errors: n.kernel_events.iter().filter(|e| e.kind == "xid").count() as u32,
            readable: !n.ov.gpu_lost,
            error: gpu_error,
        },
        disks: vec![
            DiskStats {
                mount: "/".into(),
                device: "/dev/nvme0n1p2".into(),
                fs_type: "ext4".into(),
                total: n.disk_total as u64,
                used: n.disk_used as u64,
                read_bps: n.disk_read,
                write_bps: n.disk_write,
                // Demo is frozen (§3.3): no df-style avail, so free reads total − used as before.
                avail: None,
            },
            DiskStats {
                mount: "/boot/efi".into(),
                device: "/dev/nvme0n1p1".into(),
                fs_type: "vfat".into(),
                total: 535_805_952,
                used: real.efi_used,
                read_bps: 0.0,
                write_bps: 0.0,
                avail: None,
            },
        ],
        nets,
        system: SystemInfo {
            hostname: n.cfg.host.trim_end_matches(".local").into(),
            model: model_name(n),
            os: OS.into(),
            kernel: n.kernel.clone(),
            driver: n.driver.clone(),
            cuda: CUDA.into(),
            uptime_s: now.saturating_sub(n.boot_unix),
            bios: Some(real.bios.into()),
            cx7_fw: Some(CX7_FW.into()),
            docker: Some(DOCKER.into()),
            ntp_synced: Some(n.units.iter().any(|u| u.unit == "systemd-timesyncd.service" && u.active == "active")),
            // Live can't read the offset without timedatectl's privileges; the everyday world shows one.
            clock_offset_ms: (!found_world).then(|| (0.3 + n.idx as f64 * 0.41 + jitter(rng, 0.2)) as f32),
            reboot_required: n.pending_boot.is_some(),
            ..Default::default()
        },
        containers,
        processes,
        listeners,
        stale: false,
        frame_age_ms: Some(frame_age),
        last_seen_ms: Some(now_ms.saturating_sub(frame_age)),
        failed_units,
        power_state: None,
        kernel_events: n.kernel_events.clone(),
        agent: None,
        boot_id: Some(boot_id(&n.cfg.id, n.boot_unix + u64::from(n.boots) * 1_000_003)),
        proc_total,
        host_key: Some(host_key(n)),
        ..Default::default()
    }
}

/// /proc/sys/kernel/random/boot_id for a simulated node: new on every (simulated) boot.
fn boot_id(node: &str, boot_unix: u64) -> String {
    let h = node.bytes().fold(0xcbf2_9ce4_8422_2325u64 ^ boot_unix.wrapping_mul(0x9e37_79b9_7f4a_7c15), |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let g = h.rotate_left(29) ^ 0x5bd1_e995;
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", h >> 32, (h >> 16) & 0xffff, h & 0xfff, (g >> 48) & 0x3fff | 0x8000, g & 0xffff_ffff_ffff)
}

/// What each node hears over LLDP on its LAN port, as on the real LAN: the Switch X on one port,
/// the CRS804's ether2 and each other node. In the everyday world gx10-0000 has a Switch X port
/// to itself, hearing only the switch, so the panel has a port with one node as well as several.
fn hear_the_lan(nodes: &mut [NodeSnapshot], world: World) {
    let alone = |n: &NodeSnapshot| n.id == "gx10-0000" && world != World::AsFound;
    let lan = |n: &NodeSnapshot| n.nets.iter().find(|x| x.role == NetRole::Lan).and_then(|x| x.mac.clone());
    let heard: Vec<(String, LldpNeighbor)> = nodes.iter().filter(|n| !alone(n)).map(|n| (n.id.clone(), LldpNeighbor { system: Some(n.system.hostname.clone()), port: Some(LAN_IFACE.into()), chassis_id: lan(n), port_id: lan(n) })).collect();
    for n in nodes.iter_mut() {
        let port = if alone(n) { "Gi0/5" } else { "Gi0/2" };
        let mut peers = vec![LldpNeighbor { system: Some("Firewalla-Switch-X".into()), port: Some(port.into()), chassis_id: Some(SWITCH_X_CHASSIS.into()), port_id: Some(port.into()) }];
        if !alone(n) {
            peers.push(LldpNeighbor { system: Some("CRS804".into()), port: Some("ether2".into()), chassis_id: Some(CRS804_CHASSIS.into()), port_id: Some("ether2".into()) });
            peers.extend(heard.iter().filter(|(id, _)| *id != n.id).map(|(_, p)| p.clone()));
        }
        if let Some(x) = n.nets.iter_mut().find(|x| x.role == NetRole::Lan) {
            x.lldp_peers = peers;
        }
    }
}

fn switch_snapshot(
    sw: &DemoSwitch,
    sim: &[DemoNode],
    nodes: &[NodeSnapshot],
    tables: &crate::routeros::mapping::PortTables,
    now: u64,
    rng: &mut fastrand::Rng,
) -> SwitchSnapshot {
    let configured = !sw.host.is_empty();
    let base = SwitchSnapshot {
        id: sw.cfg.id.clone(),
        name: sw.cfg.name.clone(),
        kind: sw.cfg.kind,
        host: sw.host.clone(),
        configured,
        ..Default::default()
    };
    if !configured {
        return SwitchSnapshot { status: LinkStatus::Offline, ..base };
    }
    match sw.cfg.kind {
        SwitchKind::Firewalla | SwitchKind::Gateway => {
            let gold = sw.cfg.kind == SwitchKind::Gateway;
            SwitchSnapshot {
                status: LinkStatus::Online,
                latency_ms: Some((if gold { 0.35 } else { 0.5 } + rng.f64() * 0.6) as f32),
                loss_pct: Some(0.0),
                board: Some(if gold { "Firewalla Gold" } else { "Firewalla Switch X" }.into()),
                version: gold.then(|| "Firewalla 1.981".into()),
                ..base
            }
        }
        SwitchKind::RouterOs => {
            let mut ports: Vec<SwitchPort> = sw
                .ports
                .iter()
                .map(|p| {
                    let node = p.node.and_then(|i| nodes.get(i));
                    let simn = p.node.and_then(|i| sim.get(i));
                    let running = p.running_was;
                    // The switch receives what the node transmits on its fabric, and vice versa.
                    let (rx, tx) = match (simn, p.uplink) {
                        (Some(n), _) if running => {
                            let m = |v: f64, rng: &mut fastrand::Rng| v * (1.0 + jitter(rng, 0.015));
                            (m(n.traffic.fabric_tx(), rng), m(n.traffic.fabric_rx(), rng))
                        }
                        (_, true) => (0.8e6 + rng.f64() * 2.6e6, 3e5 + rng.f64() * 9e5),
                        _ => (0.0, 0.0),
                    };
                    let optic = running && !p.uplink && p.node.is_some();
                    let lanes: Vec<f32> = if optic {
                        p.lanes
                            .iter()
                            .enumerate()
                            .map(|(l, v)| match p.ov_lane {
                                Some((lane, dbm)) if lane == l => dbm + jitter(rng, 0.05) as f32,
                                _ => v + jitter(rng, 0.04) as f32,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let worst = lanes.iter().cloned().fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.min(v))));
                    SwitchPort {
                        name: p.name.clone(),
                        comment: p.comment.clone(),
                        running,
                        disabled: p.disabled,
                        speed: if running { p.speed.into() } else { String::new() },
                        rx_bps: rx,
                        tx_bps: tx,
                        rx_errors: p.rx_errors,
                        tx_errors: 0,
                        module_temp_c: optic.then(|| (44.0 + p.node.unwrap_or(0) as f64 * 1.3 + rng.f64() * 0.8) as f32),
                        rx_power_dbm: worst,
                        node: node.map(|n| n.id.clone()),
                        mtu: Some(if p.uplink { 1500 } else { 9216 }),
                        link_downs: p.link_downs,
                        last_link_up_ms: running.then_some(p.last_up_unix * 1000),
                        rx_drops: p.rx_drops,
                        tx_drops: 0,
                        fcs_errors: p.fcs_errors,
                        rx_pause: p.rx_pause,
                        tx_power_dbm: optic.then_some(p.tx_power),
                        lane_rx_power_dbm: lanes,
                        // Only the cabled breakout ports have optics in; the spare cages are empty.
                        sfp_vendor: p.node.is_some().then(|| "MikroTik".into()),
                        sfp_part: p.node.is_some().then(|| "XQ+DD85-2Q56-SR4".into()),
                        capacity_mbps: if p.uplink { 1_000 } else { FABRIC_SPEED_MBPS },
                        // Who's on the port comes from the bridge host table, as in Live; the
                        // cabling baseline (fabric::FabricMonitor) then fills cabling_ok.
                        cabling_ok: None,
                        expected_node: None,
                        mapped_by: None,
                        management: None,
                        revert_at_ms: p.revert_at.map(|(_, ms)| ms),
                    }
                })
                .collect();
            crate::routeros::mapping::apply(&mut ports, tables, nodes);
            // A port without link has no MACs behind it: the switch forgot them, but the
            // node is still cabled there (as the fabric monitor remembers in Live).
            for (sp, dp) in ports.iter_mut().zip(&sw.ports) {
                if sp.node.is_none() && !sp.running {
                    if let Some(n) = dp.node.and_then(|i| nodes.get(i)) {
                        sp.node = Some(n.id.clone());
                        sp.mapped_by = Some("mac".into());
                    }
                }
            }
            let cpu = 3.0 + rng.f64() * 5.0;
            SwitchSnapshot {
                status: LinkStatus::Online,
                latency_ms: Some((0.4 + rng.f64() * 1.2) as f32),
                version: Some("7.20.2 (stable)".into()),
                board: Some("CRS804-4DDQ".into()),
                cpu_pct: Some(cpu as f32),
                temp_c: Some((52.0 + rng.f64() * 2.5) as f32),
                uptime_s: Some(now.saturating_sub(sw.boot_unix)),
                sensors: vec![
                    Sensor { name: "CPU temperature".into(), value: (47.0 + rng.f64() * 1.5).round(), unit: "°C".into(), ok: true },
                    Sensor { name: "Switch chip temperature".into(), value: (58.0 + rng.f64() * 2.0).round(), unit: "°C".into(), ok: true },
                    Sensor { name: "Board temperature".into(), value: (39.0 + rng.f64()).round(), unit: "°C".into(), ok: true },
                    Sensor { name: "Fan 1".into(), value: (5_820.0 + jitter(rng, 60.0)).round(), unit: "RPM".into(), ok: true },
                    Sensor { name: "Fan 2".into(), value: (5_760.0 + jitter(rng, 60.0)).round(), unit: "RPM".into(), ok: true },
                    Sensor { name: "PSU1".into(), value: 1.0, unit: String::new(), ok: true },
                    Sensor { name: "PSU2".into(), value: 1.0, unit: String::new(), ok: true },
                ],
                loss_pct: Some(0.0),
                ports,
                ..base
            }
        }
    }
}
