//! Data shapes sent to the UI. Every backend (the demo simulator, live SSH)
//! produces exactly these, so the UI never knows which one is running.
//!
//! Every struct derives Default and every field added after the first release
//! is `#[serde(default)]`, so constructors use `..Default::default()` and older
//! collectors (or frames missing a field) keep working.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::inventory::SwitchKind;

/// How SparkBox's connection to a machine is doing. "Stale" (no fresh numbers)
/// and "not set up" are separate fields, not variants: the menu bar matches on this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkStatus {
    Online,
    Connecting,
    #[default]
    Offline,
    Error,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuStats {
    pub total_pct: f32,
    /// GB10: cores 0-9 are Cortex-X925 (performance), 10-19 Cortex-A725 (efficiency).
    pub per_core: Vec<f32>,
    pub load: [f32; 3],
    pub temp_c: f32,
}

/// GB10 has one LPDDR5x pool shared by CPU and GPU, and nvidia-smi reports its
/// memory as "Not Supported". These numbers come from /proc/meminfo; `gpu_alloc`
/// is the sum of per-process GPU allocations from nvidia-smi.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemStats {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub cached: u64,
    pub gpu_alloc: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// Bytes read from swap per second; unknown on the first reading or counter reset.
    #[serde(default)]
    pub swap_in_bps: Option<f64>,
    // ── r4-nodes ──
    /// /proc/pressure/memory avg10: the share of the last 10 s some (or all) tasks
    /// waited on memory. None when the kernel doesn't expose PSI.
    #[serde(default)]
    pub psi_some10: Option<f32>,
    #[serde(default)]
    pub psi_full10: Option<f32>,
    /// /proc/pressure/io avg10, the same for disk IO.
    #[serde(default)]
    pub psi_io_some10: Option<f32>,
    #[serde(default)]
    pub psi_io_full10: Option<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuProc {
    pub pid: u32,
    pub name: String,
    pub mem: u64,
    pub container: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    // A GPU that doesn't answer reads null (unknown, not idle): the frame still parses, the
    // node's error says why, and history keeps a gap there (history::sample).
    #[serde(deserialize_with = "null_zero")]
    pub util_pct: f32,
    #[serde(deserialize_with = "null_zero")]
    pub temp_c: f32,
    #[serde(deserialize_with = "null_zero")]
    pub power_w: f32,
    #[serde(deserialize_with = "null_zero")]
    pub sm_clock_mhz: u32,
    pub procs: Vec<GpuProc>,
    #[serde(default)]
    pub clock_max_mhz: Option<u32>,
    #[serde(default)]
    pub power_limit_w: Option<f32>,
    /// P0 (busiest) … P8 (idle).
    #[serde(default)]
    pub pstate: Option<String>,
    /// Active clock-throttle reasons in plain words, e.g. "HW thermal slowdown".
    #[serde(default)]
    pub throttle: Vec<String>,
    /// NVIDIA Xid errors seen in the kernel log since boot.
    #[serde(default)]
    pub xid_errors: u32,
    // ── r4-nodes ──
    /// False when nvidia-smi gave no numbers this frame (the numbers above are then 0,
    /// not readings). Use `gpu_readable`, never the error text.
    #[serde(default = "yes")]
    pub readable: bool,
    /// Why the GPU couldn't be read, when it couldn't.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskStats {
    pub mount: String,
    pub device: String,
    pub fs_type: String,
    pub total: u64,
    pub used: u64,
    /// Bytes per second.
    pub read_bps: f64,
    pub write_bps: f64,
    // ── r1-c ──
    /// What a normal user can still write (statvfs f_bavail, df's Available); None from a
    /// collector older than frame 6. ext4 keeps 5% back for root, so it's less than total − used,
    /// and 0 on a disk that's full for everyone but root.
    #[serde(default)]
    pub avail: Option<u64>,
}

// ── r1-c ──
impl DiskStats {
    /// Free space as df reports it: `avail`, or total − used from an older collector.
    pub fn free(&self) -> u64 {
        self.avail.unwrap_or_else(|| self.total.saturating_sub(self.used))
    }

    /// Percent used as df's Use% has it, used / (used + avail): the space root keeps back
    /// isn't free to anyone else. Over the total without `avail`.
    pub fn used_pct(&self) -> f64 {
        let whole = self.avail.map_or(self.total, |a| self.used + a);
        if whole > 0 {
            self.used as f64 / whole as f64 * 100.0
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetRole {
    #[default]
    Lan,
    Fabric,
    Wifi,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetStats {
    pub iface: String,
    pub role: NetRole,
    pub up: bool,
    /// 0 when unknown (a Wi-Fi link without iw sends null): the UI shows "—", never "0M".
    #[serde(deserialize_with = "null_zero")]
    pub speed_mbps: u32,
    /// Bits per second through the kernel's IP stack (TCP, NCCL sockets…).
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub ip: Option<String>,
    #[serde(default)]
    pub mtu: Option<u32>,
    #[serde(default)]
    pub mac: Option<String>,
    /// Counters since boot.
    #[serde(default)]
    pub rx_errors: u64,
    #[serde(default)]
    pub tx_errors: u64,
    #[serde(default)]
    pub rx_drops: u64,
    #[serde(default)]
    pub tx_drops: u64,
    /// Link up/down transitions since boot.
    #[serde(default)]
    pub carrier_changes: Option<u64>,
    /// The RDMA device on this netdev (e.g. rocep1s0f1). RoCE traffic bypasses the
    /// IP counters, so NCCL's bulk transfers show up only in the rdma_* rates.
    #[serde(default)]
    pub rdma_dev: Option<String>,
    #[serde(default)]
    pub rdma_rx_bps: Option<f64>,
    #[serde(default)]
    pub rdma_tx_bps: Option<f64>,
    /// The switch port this interface is cabled to, as "<switch id>/<port>".
    #[serde(default)]
    pub switch_port: Option<String>,
    // ── r2-fabric ──
    /// RoCE and PHY counters (fabric interfaces with an RDMA device only).
    #[serde(default)]
    pub roce: Option<RoceCounters>,
    /// Wake-on-LAN on the LAN interface, from ethtool or NetworkManager.
    #[serde(default, deserialize_with = "wol_info")]
    pub lan_wol: Option<WolInfo>,
    // ── r4-nodes ──
    /// The Wi-Fi association (role Wifi only), from `iw dev <if> link`. `speed_mbps`
    /// is then the tx bitrate, or 0 when iw isn't there: unknown, shown as "—".
    #[serde(default)]
    pub wifi: Option<WifiInfo>,
    /// The switch port LLDP reports on the other end of this interface.
    #[serde(default)]
    pub lldp: Option<LldpNeighbor>,
    // ── r1-c ──
    /// Every LLDP neighbour heard here that isn't this node (frame 6): a switch, or another
    /// node through one. The UI tells them apart by the cluster's hostnames and MACs.
    #[serde(default)]
    pub lldp_peers: Vec<LldpNeighbor>,
    /// Only this node itself was heard: each ConnectX-7 half hears its sibling through the switch.
    #[serde(default)]
    pub lldp_self_only: bool,
}

/// Cumulative RoCE and PHY counters for one fabric interface. A counter the
/// driver doesn't expose is `None`, never 0: absent is not the same as clean.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoceCounters {
    // /sys/class/infiniband/<dev>/ports/1/hw_counters
    #[serde(default)]
    pub ecn_marked: Option<u64>,
    #[serde(default)]
    pub cnp_sent: Option<u64>,
    #[serde(default)]
    pub cnp_handled: Option<u64>,
    #[serde(default)]
    pub cnp_ignored: Option<u64>,
    #[serde(default)]
    pub out_of_sequence: Option<u64>,
    #[serde(default)]
    pub packet_seq_err: Option<u64>,
    #[serde(default)]
    pub local_ack_timeout_err: Option<u64>,
    #[serde(default)]
    pub implied_nak_seq_err: Option<u64>,
    #[serde(default)]
    pub rnr_nak_retry_err: Option<u64>,
    #[serde(default)]
    pub duplicate_request: Option<u64>,
    #[serde(default)]
    pub rx_icrc_err: Option<u64>,
    #[serde(default)]
    pub req_cqe_err: Option<u64>,
    #[serde(default)]
    pub resp_cqe_err: Option<u64>,
    // /sys/class/infiniband/<dev>/ports/1/counters
    #[serde(default)]
    pub port_rcv_errors: Option<u64>,
    #[serde(default)]
    pub port_xmit_discards: Option<u64>,
    #[serde(default)]
    pub symbol_error: Option<u64>,
    #[serde(default)]
    pub link_downed: Option<u64>,
    // ethtool -S <iface>, per priority 0-7
    #[serde(default, deserialize_with = "eight")]
    pub rx_pause: [Option<u64>; 8],
    #[serde(default, deserialize_with = "eight")]
    pub tx_pause: [Option<u64>; 8],
    #[serde(default)]
    pub rx_pause_ctrl: Option<u64>,
    #[serde(default)]
    pub tx_pause_ctrl: Option<u64>,
    #[serde(default)]
    pub rx_discards_phy: Option<u64>,
    #[serde(default)]
    pub rx_crc_errors_phy: Option<u64>,
    #[serde(default)]
    pub rx_symbol_err_phy: Option<u64>,
    #[serde(default)]
    pub rx_out_of_buffer: Option<u64>,
    // mlnx_qos -i <iface> and the GID table, when readable
    /// "pcp" or "dscp".
    #[serde(default)]
    pub trust: Option<String>,
    /// Priorities with PFC enabled.
    #[serde(default)]
    pub pfc_prios: Option<Vec<u8>>,
    #[serde(default, deserialize_with = "eight")]
    pub rx_prio_bytes: [Option<u64>; 8],
    /// At least one RoCE v2 GID is configured.
    #[serde(default)]
    pub roce_v2: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    #[serde(default)]
    pub collector_version: Option<String>,
    #[serde(default)]
    pub container_toolkit: Option<String>,
    #[serde(default)]
    pub image_store: Option<String>,
    pub hostname: String,
    pub model: String,
    pub os: String,
    pub kernel: String,
    pub driver: String,
    pub cuda: String,
    pub uptime_s: u64,
    #[serde(default)]
    pub bios: Option<String>,
    /// ConnectX-7 firmware.
    #[serde(default)]
    pub cx7_fw: Option<String>,
    #[serde(default)]
    pub docker: Option<String>,
    #[serde(default)]
    pub ntp_synced: Option<bool>,
    #[serde(default)]
    pub clock_offset_ms: Option<f32>,
    /// /var/run/reboot-required exists (a kernel or driver update is waiting).
    #[serde(default)]
    pub reboot_required: bool,
    // ── p1r3-check ──
    /// /etc/dgx-release: DGX_SWBUILD_VERSION ("7.2.3"), the last DGX_OTA_VERSION ("7.6.0") and
    /// its DGX_OTA_DATE, as the file gives it.
    #[serde(default)]
    pub dgx_build: Option<String>,
    #[serde(default)]
    pub dgx_ota: Option<String>,
    #[serde(default)]
    pub dgx_ota_date: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    /// Full immutable Docker image identity, never the mutable tag.
    #[serde(default)]
    pub image_id: Option<String>,
    pub id: String,
    pub name: String,
    pub image: String,
    /// Docker state: running | exited | restarting | paused | created
    pub state: String,
    /// Unix seconds.
    pub started_at: u64,
    pub finished_at: u64,
    pub exit_code: i32,
    pub cpu_pct: f32,
    pub mem: u64,
    pub gpu_mem: u64,
    pub ports: Vec<String>,
    /// Docker Compose project, when the container belongs to one.
    pub project: Option<String>,
    pub command: String,
    /// healthy | starting | unhealthy, when the image has a health check.
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub restart_count: u32,
    #[serde(default)]
    pub oom_killed: bool,
    /// no | always | unless-stopped | on-failure
    #[serde(default)]
    pub restart_policy: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    // ── r2-fabric ──
    /// NCCL_* and VLLM_HOST_IP from the container's environment (secret-looking
    /// values redacted). They feed the NCCL transport check.
    #[serde(default)]
    pub env_flags: BTreeMap<String, String>,
}

/// One of the busiest processes on a node (top CPU, plus every GPU user and listener).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub user: String,
    pub cmd: String,
    pub cpu_pct: f32,
    pub rss: u64,
    pub nice: i32,
    pub container: Option<String>,
    /// systemd unit (e.g. ollama.service) when it runs as a service.
    pub unit: Option<String>,
    #[serde(default)]
    pub ports: Vec<u16>,
    #[serde(default)]
    pub gpu_mem: u64,
    /// The kernel's one-letter state: R running, S sleeping, D waiting on IO, T stopped, Z zombie.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub ppid: Option<u32>,
    #[serde(default)]
    pub threads: Option<u32>,
    /// Clock ticks after boot (/proc/<pid>/stat field 22).
    #[serde(default)]
    pub start_time: Option<u64>,
    #[serde(default)]
    pub cpu_time_s: Option<f64>,
    // ── r4-nodes ──
    /// The kernel's 15-character name (/proc/<pid>/comm). `name` is the full one from argv[0].
    #[serde(default)]
    pub comm: Option<String>,
    /// A kernel thread (kworker, ksoftirqd…): hidden unless asked for.
    #[serde(default)]
    pub kernel: bool,
    /// SparkBox's own collector and the commands it runs.
    #[serde(default)]
    pub sparkbox: bool,
}

/// A listening TCP port. The process is known only for the cluster user's own processes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listener {
    pub port: u16,
    pub addr: String,
    pub pid: Option<u32>,
    pub process: Option<String>,
    // ── r4-nodes ──
    /// Who can reach it: local (loopback) | all (every interface) | lan | fabric (192.168.100-101).
    #[serde(default)]
    pub scope: Option<String>,
    /// v4 | v6 | both (the same port bound on both families is one row).
    #[serde(default)]
    pub family: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSnapshot {
    pub id: String,
    pub name: String,
    pub host: String,
    pub status: LinkStatus,
    pub latency_ms: Option<f32>,
    pub error: Option<String>,
    pub cpu: CpuStats,
    pub mem: MemStats,
    pub gpu: GpuStats,
    pub disks: Vec<DiskStats>,
    pub nets: Vec<NetStats>,
    pub system: SystemInfo,
    pub containers: Vec<Container>,
    pub processes: Vec<Process>,
    pub listeners: Vec<Listener>,
    /// No fresh numbers for over 3 s: what's shown is the last frame, not the present.
    #[serde(default)]
    pub stale: bool,
    /// How old the numbers shown are.
    #[serde(default)]
    pub frame_age_ms: Option<u64>,
    /// Unix ms of the last frame received.
    #[serde(default)]
    pub last_seen_ms: Option<u64>,
    /// systemd units in the failed state.
    #[serde(default)]
    pub failed_units: Vec<String>,
    /// rebooting | shutting-down | off | waking, while a power action is under way.
    #[serde(default)]
    pub power_state: Option<String>,
    // ── r2-fabric ──
    /// The most recent kernel events (Xid, mlx5, OOM kill, hung task), newest last, at most 50.
    #[serde(default)]
    pub kernel_events: Vec<KernelEvent>,
    /// The always-on node agent's heartbeat, when one is installed.
    #[serde(default)]
    pub agent: Option<AgentBeat>,
    // ── r3-incidents ──
    /// /proc/sys/kernel/random/boot_id: changes on every boot, so a latched GPU
    /// fault knows when the GPU has been reset.
    #[serde(default)]
    pub boot_id: Option<String>,
    /// A fatal GPU fault (latched Xid, or GPU readings unreadable). Its endpoints are Down.
    #[serde(default)]
    pub gpu_fault: Option<GpuFault>,
    /// Planned work SparkBox started on this node (a restart or shutdown).
    #[serde(default)]
    pub maintenance: Option<MaintInfo>,
    // ── r3-agent ──
    // ── r3-serving ──
    // ── r3-safety ──
    // ── r4-nodes ──
    /// Every process on the node, before the list was cut to the ones sent.
    #[serde(default)]
    pub proc_total: u32,
    /// Why the connection failed, as a kind: hostKeyUnknown | hostKeyChanged | authFailed |
    /// timeout | refused | unreachable | dns. The UI words it; `error` keeps the detail.
    #[serde(default)]
    pub error_kind: Option<String>,
    /// The node's SSH host key, once seen (pinned or offered on first contact).
    #[serde(default)]
    pub host_key: Option<HostKeyInfo>,
    // ── r4-serving ──
    // ── r4-alerts ──
    // ── r4-agent ──
    // ── r4-safety ──
    // ── r4-demo ──
    // ── r4-craft ──
    // ── p1r3-check ──
    /// What the update check last found on the node (ops/update_check.rs `overlay`, Live only).
    #[serde(default)]
    pub updates: Option<UpdateFacts>,
    // ── p1r3-foot ──
    /// How many processes the node reported, where the list sent to the windows each second is
    /// cut to the 15 busiest and the listeners' owners (footprint::trim_for_ui); processes_get
    /// has them all.
    #[serde(default)]
    pub process_count: Option<u32>,
}

/// A node's pending updates as the sidebar badge and the update alert rules read them
/// (ops/update_check.rs `UpdateSummary::facts`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFacts {
    pub pending: u32,
    pub security: u32,
    /// When SparkBox first saw security updates pending (cleared once there are none).
    pub security_since_ms: Option<u64>,
    pub reboot_required: bool,
    /// /var/run/reboot-required's mtime.
    pub reboot_since_ms: Option<u64>,
    /// When the package lists were last refreshed.
    pub lists_ms: Option<u64>,
    pub checked_ms: u64,
    /// Firmware updates fwupd offers (0: none); None when it couldn't say.
    pub firmware: Option<u32>,
    pub ollama_newer: bool,
    #[serde(default)]
    pub engine_images: Option<u32>,
    /// The DGX OS OTA version apt offers (dgx-spark-ota-update-meta).
    pub ota_pending: Option<String>,
    /// Read before the node last (re)connected (kept from the last launch, or from before it
    /// went away) and not checked since: the update rules leave their alerts as they are and
    /// raise none from it.
    #[serde(default)]
    pub unconfirmed: bool,
    /// The security updates among them in the GPU stack (SparkBox can't upgrade those yet);
    /// None when not known (facts saved by an earlier build).
    #[serde(default)]
    pub security_gpu: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelEvent {
    pub ts_ms: u64,
    /// xid | mlx5 | oom | hung
    pub kind: String,
    /// The Xid number, for kind "xid".
    #[serde(default)]
    pub code: Option<u32>,
    pub text: String,
    /// warn | crit
    pub severity: String,
    #[serde(default)]
    pub process: Option<String>,
    // ── r4-alerts ──
    // health::kernel::stamp makes a ConnectX-7 cable or link change kind "link", severity "info",
    // before anything reads the snapshot; the Events tab groups them (DecodedEvent.group).
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBeat {
    pub version: String,
    pub last_beat_ms: u64,
    /// The oldest sample the agent still holds.
    #[serde(default)]
    pub history_from_ms: Option<u64>,
    /// Rules firing on the node right now.
    #[serde(default)]
    pub firing: u32,
    // ── r3-paging ──
    /// The settings hash the agent reports (agent.json, rules.json, notify.json as loaded).
    #[serde(default)]
    pub config_hash: Option<String>,
    /// The reported hash equals the one SparkBox last pushed; None when there's nothing to compare.
    #[serde(default)]
    pub config_current: Option<bool>,
    /// Why the last settings push to this node failed, while it's still pending.
    #[serde(default)]
    pub push_error: Option<String>,
    /// When it failed.
    #[serde(default)]
    pub push_error_ms: Option<u64>,
    /// End of the agent's last completed sampling cycle.
    #[serde(default)]
    pub last_cycle_ms: Option<u64>,
    /// Peers nothing answers on, as this agent sees them.
    #[serde(default)]
    pub peers_missing: Vec<String>,
    /// "fabric" or "network" when this agent can't see its peers itself.
    #[serde(default)]
    pub isolated: Option<String>,
    /// Endpoints this agent probes with a one-token check.
    #[serde(default)]
    pub endpoints_probed: Vec<String>,
    #[serde(default)]
    pub last_delivery: Option<DeliveryBeat>,
    /// The dead-man's switch answered this agent's last ping at this time.
    #[serde(default)]
    pub heartbeat_ok_ms: Option<u64>,
    /// Messages waiting in the agent's outbox.
    #[serde(default)]
    pub outbox_pending: u32,
    // ── r3-agent ──
    /// This agent leads and checks the fabric switch's management address.
    #[serde(default)]
    pub switch_watched: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchPort {
    pub name: String,
    pub comment: String,
    pub running: bool,
    pub disabled: bool,
    pub speed: String,
    /// Bits per second, as seen by the switch.
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub module_temp_c: Option<f32>,
    /// The weakest lane's receive power.
    pub rx_power_dbm: Option<f32>,
    /// The node cabled to this port (by comment, or by MAC when the bridge host table shows it).
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub mtu: Option<u32>,
    #[serde(default)]
    pub link_downs: u64,
    #[serde(default)]
    pub last_link_up_ms: Option<u64>,
    #[serde(default)]
    pub rx_drops: u64,
    #[serde(default)]
    pub tx_drops: u64,
    #[serde(default)]
    pub fcs_errors: u64,
    #[serde(default)]
    pub rx_pause: u64,
    #[serde(default)]
    pub tx_power_dbm: Option<f32>,
    /// One value per optical lane (4 for a 200G breakout of a QSFP56-DD).
    #[serde(default)]
    pub lane_rx_power_dbm: Vec<f32>,
    #[serde(default)]
    pub sfp_vendor: Option<String>,
    #[serde(default)]
    pub sfp_part: Option<String>,
    /// Link capacity in Mb/s (200000 for a 200G port), for utilisation.
    #[serde(default)]
    pub capacity_mbps: u32,
    /// False when the node seen on the port differs from the learned cabling baseline.
    #[serde(default)]
    pub cabling_ok: Option<bool>,
    // ── r2-fabric ──
    /// The node the cabling baseline expects here, when it differs from what's seen.
    #[serde(default)]
    pub expected_node: Option<String>,
    /// How `node` was found: mac | lldp | comment.
    #[serde(default)]
    pub mapped_by: Option<String>,
    /// Why this port carries SparkBox's own path to the switch, when it does: disabling
    /// or bouncing it is refused unless typed out, and always reverts on the switch.
    #[serde(default)]
    pub management: Option<String>,
    /// While a disable is waiting to revert on the switch: when it will (Unix ms).
    #[serde(default)]
    pub revert_at_ms: Option<u64>,
}

/// A health reading from a switch: a temperature, a fan or a power supply.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sensor {
    pub name: String,
    pub value: f64,
    /// "°C", "RPM", "V", "W" or "" for a plain ok/fail state.
    pub unit: String,
    pub ok: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchSnapshot {
    pub id: String,
    pub name: String,
    pub kind: SwitchKind,
    pub host: String,
    pub status: LinkStatus,
    pub latency_ms: Option<f32>,
    pub version: Option<String>,
    pub cpu_pct: Option<f32>,
    pub temp_c: Option<f32>,
    pub uptime_s: Option<u64>,
    pub ports: Vec<SwitchPort>,
    /// False until an address is set in Settings; such a switch is always Offline ("Not set up").
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub board: Option<String>,
    #[serde(default)]
    pub sensors: Vec<Sensor>,
    /// Lost probes over the last minute or so, for devices SparkBox can only ping.
    #[serde(default)]
    pub loss_pct: Option<f32>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterSnapshot {
    #[serde(default)]
    pub versions: Option<crate::versions::Report>,
    pub ts_ms: u64,
    /// True while the numbers come from the built-in simulator.
    pub demo: bool,
    pub nodes: Vec<NodeSnapshot>,
    pub switches: Vec<SwitchSnapshot>,
    /// The demo scenario being played, if any (Demo only).
    #[serde(default)]
    pub scenario: Option<String>,
    // ── r2-fabric ──
    /// Where the scenario is in its cycle (Demo only).
    #[serde(default)]
    pub scenario_phase: Option<ScenarioPhase>,
    // ── r1-h ──
    /// Whether this Mac can reach the cluster at all (health/reach.rs), stamped by
    /// health::on_publish (Live only; None in Demo).
    #[serde(default)]
    pub mac_reach: Option<MacReach>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioPhase {
    /// fault | recovering | healthy
    pub phase: String,
    /// "Node offline · recovering"
    pub label: String,
    /// Seconds until the next phase, when the scenario moves on by itself.
    #[serde(default)]
    pub next_change_s: Option<u32>,
}

/// Eight per-priority values from a list of any length (a short or long list from an
/// older or newer collector mustn't drop the whole frame).
fn eight<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[Option<u64>; 8], D::Error> {
    let v: Option<Vec<Option<u64>>> = Option::deserialize(d)?;
    let mut out = [None; 8];
    for (slot, x) in out.iter_mut().zip(v.unwrap_or_default()) {
        *slot = x;
    }
    Ok(out)
}

fn yes() -> bool {
    true
}

/// A number the collector sends as null when it couldn't read it.
fn null_zero<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

// Fields and types other packages add (append under your marker, `#[serde(default)]`).
// ── r2-design ──
// ── r2-shell ──
// ── r2-monitor ──
// ── r2-ai ──
// ── r2-live ──
// ── r2-safety ──
// ── r3-incidents ──

/// A GPU fault that keeps the node's endpoints Down until the GPU is back.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuFault {
    /// The Xid number, for source "xid".
    #[serde(default)]
    pub code: Option<u32>,
    /// "GPU has fallen off the bus (Xid 79)"
    pub text: String,
    pub since_ms: u64,
    /// xid | unreadable
    pub source: String,
}

/// A maintenance window: SparkBox restarted or shut down this node on purpose.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintInfo {
    /// When the node should be back (Unix ms); None for a shutdown (until Wake).
    #[serde(default)]
    pub until_ms: Option<u64>,
    /// "restarting" or "shut down"
    pub reason: String,
    pub started_ms: u64,
}
// ── r3-paging ──

/// An agent's latest delivery attempt, from its status.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryBeat {
    pub ms: u64,
    pub ok: bool,
    pub channel: String,
}
// ── r3-agent ──
// ── r3-serving ──
// ── r3-charts ──
// ── r3-desk ──
// ── r3-safety ──
// ── r4-nodes ──

impl Default for GpuStats {
    fn default() -> Self {
        GpuStats {
            util_pct: 0.0,
            temp_c: 0.0,
            power_w: 0.0,
            sm_clock_mhz: 0,
            procs: Vec::new(),
            clock_max_mhz: None,
            power_limit_w: None,
            pstate: None,
            throttle: Vec::new(),
            xid_errors: 0,
            readable: true,
            error: None,
        }
    }
}

/// The node's GPU gave numbers this frame. An offline node reads as unreadable too:
/// its last numbers are not the present.
pub fn gpu_readable(n: &NodeSnapshot) -> bool {
    n.status == LinkStatus::Online && n.gpu.readable
}

/// Wi-Fi as `iw dev <if> link` reports it without root. A field iw didn't print is None.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiInfo {
    #[serde(default)]
    pub ssid: Option<String>,
    #[serde(default)]
    pub signal_dbm: Option<i32>,
    #[serde(default)]
    pub bitrate_mbps: Option<f32>,
}

/// Wake-on-LAN as far as SparkBox can read it without root.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WolInfo {
    /// on | off | unknown
    pub state: String,
    /// ethtool | networkmanager | none
    #[serde(default)]
    pub source: String,
    /// "On (NetworkManager: magic packet)", "Not readable without sudo (ethtool needs root)"…
    #[serde(default)]
    pub detail: String,
    /// ethtool's Wake-on letters when ethtool could read them ("g" magic packet, "d" off).
    #[serde(default)]
    pub raw: Option<String>,
}

impl WolInfo {
    /// From ethtool's Wake-on letters: "g" in them means magic packets wake the node.
    pub fn from_ethtool(letters: &str) -> WolInfo {
        let on = letters.contains('g');
        WolInfo {
            state: if on { "on" } else if letters == "d" { "off" } else { "unknown" }.into(),
            source: "ethtool".into(),
            detail: if on { "On (magic packet)".into() } else if letters == "d" { "Off".into() } else { format!("Wake-on: {letters}") },
            raw: Some(letters.to_string()),
        }
    }
}

// Older collectors and the Demo world give the ethtool letters alone.
impl From<&str> for WolInfo {
    fn from(letters: &str) -> Self {
        WolInfo::from_ethtool(letters)
    }
}

// Compares as the ethtool letters, as `lan_wol` did when it was a plain string.
impl std::ops::Deref for WolInfo {
    type Target = str;
    fn deref(&self) -> &str {
        self.raw.as_deref().unwrap_or("")
    }
}

/// `lanWol` as an object, or as the ethtool letters an older collector sends.
fn wol_info<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<WolInfo>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Letters(String),
        Info(WolInfo),
    }
    Ok(match Option::<Either>::deserialize(d)? {
        None => None,
        Some(Either::Letters(s)) => Some(WolInfo::from_ethtool(&s)),
        Some(Either::Info(i)) => Some(i),
    })
}

/// A neighbour LLDP reports (lldpcli, readable without root on the Sparks).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LldpNeighbor {
    /// The neighbour's system name ("crs804").
    #[serde(default)]
    pub system: Option<String>,
    /// Its port ("qsfp56-dd-1-2").
    #[serde(default)]
    pub port: Option<String>,
    // ── r1-c ──
    /// The chassis and port ids as sent (a MAC in lower case, or a name).
    #[serde(default)]
    pub chassis_id: Option<String>,
    #[serde(default)]
    pub port_id: Option<String>,
}

/// A node's SSH host key as SparkBox sees it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyInfo {
    /// host:port as pinned in known_hosts.
    pub address: String,
    /// "SHA256:wLFU…"
    pub fingerprint: String,
    /// Other nodes pinned to the same key (a cloned image).
    #[serde(default)]
    pub shared_with: Vec<String>,
}
// ── r4-serving ──
// ── r4-alerts ──
// ── r4-agent ──
// ── r4-safety ──
// ── r4-demo ──
// ── r4-craft ──
// ── r1-h ──

/// What the cluster-path monitor (health/reach.rs) makes of this Mac's own connectivity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MacReachState {
    #[default]
    Ok,
    /// The Mac's router doesn't answer.
    LostHomeNetwork,
    /// The router answers, but no node does.
    LanBetween,
    /// Only a node's Wi-Fi address answers.
    WiredSwitch,
    /// The VPN is in Error while nodes don't answer.
    VpnProblem,
    /// The Mac has no default route.
    NoRoute,
    /// The Mac woke up in the last minute: nodes get time to reconnect.
    Waking,
}

/// This Mac's path to the cluster, as `ClusterSnapshot.mac_reach` carries it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacReach {
    pub state: MacReachState,
    /// When it last changed state.
    #[serde(default)]
    pub since_ms: u64,
    /// What a mac-reach alert is titled: "This Mac can't reach any node; the LAN between them?".
    #[serde(default)]
    pub title: String,
    /// One sentence: "The router (192.168.50.1) answers; the Firewalla Switch X doesn't." Empty when ok.
    #[serde(default)]
    pub detail: String,
    /// The default route's gateway and interface ("192.168.50.1", "en0").
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub interface: Option<String>,
}
