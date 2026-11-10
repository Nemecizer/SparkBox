//! The simulated cluster's identity: interface names, addresses, images, model
//! folders and switch addresses. Every simulator (the snapshot, Files, Terminal,
//! Models, Fleet Command) reads these, so every view of the demo agrees.

use sha2::{Digest, Sha256};

use crate::inventory::Inventory;

/// The cabled half of each Spark's ConnectX-7: (netdev, /24 subnet, RDMA device).
/// The CX-7 port shows up as two netdevs, one per PCIe x4 link; both carry RoCE.
pub const FABRIC: [(&str, &str, &str); 2] =
    [("enp1s0f1np1", "192.168.100", "rocep1s0f1"), ("enP2p1s0f1np1", "192.168.101", "roceP2p1s0f1")];
/// The uncabled port's netdevs: present, down, no address.
pub const FABRIC_UNUSED: [(&str, &str); 2] = [("enp1s0f0np0", "rocep1s0f0"), ("enP2p1s0f0np0", "roceP2p1s0f0")];
pub const FABRIC_MTU: u32 = 9000;
pub const FABRIC_PREFIX: u8 = 24;
pub const FABRIC_SPEED_MBPS: u32 = 200_000;
/// NCCL's bootstrap interface, as set in every container's environment.
pub const NCCL_IFACE: &str = "enp1s0f1np1";

pub const LAN_IFACE: &str = "enP7s7";
/// The Realtek r8127 on the board links at 1 Gb/s to the Firewalla Switch X.
pub const LAN_SPEED_MBPS: u32 = 1_000;
pub const WIFI_IFACE: &str = "wlP9s9";
/// The chassis ids the Switch X and the CRS804 send in LLDP on the LAN.
pub const SWITCH_X_CHASSIS: &str = "20:6d:31:be:00:15";
pub const CRS804_CHASSIS: &str = "38:32:7a:79:9b:9e";

pub const VLLM_IMAGE: &str = "nvcr.io/nvidia/vllm:26.05-py3";
pub const OLLAMA_IMAGE: &str = "ollama/ollama:0.12.6";
pub const WEBUI_IMAGE: &str = "ghcr.io/open-webui/open-webui:v0.6.34";
pub const SGLANG_IMAGE: &str = "lmsysorg/sglang:v0.5.4-cu130-spark";

// The software is the same on all four nodes (doctor.txt, Oct 29).
pub const DRIVER: &str = "580.178.04";
pub const KERNEL: &str = "7.0.0-1019-nvidia";
pub const OS: &str = "NVIDIA DGX Spark (Ubuntu 24.04.5 LTS)";
pub const CUDA: &str = "13.0";
pub const CX7_FW: &str = "28.50.1002";
pub const DOCKER: &str = "29.6.2";
pub const PYTHON: &str = "3.12.3";
/// The native Ollama on cell1 and cell3 (systemd, not a container).
pub const OLLAMA_VERSION: &str = "0.21.0";

/// The model the three-node Ray cluster serves (pipeline-parallel across cell1-cell3).
pub const SERVED_REPO: &str = "meta-llama/Llama-3.3-70B-Instruct";
pub const SERVED_NAME: &str = "llama-3.3-70b";
/// Where models live on every node, under the cluster user's home; mounted at /models in containers.
pub const MODELS_DIR: &str = "models";

/// A model folder on disk: `~/models/<repo>` on each listed node, or a snapshot in the
/// Hugging Face cache (`~/.cache/huggingface/hub/models--org--name/snapshots/<revision>`).
#[derive(Clone, Copy, Debug)]
pub struct ModelFolder {
    pub repo: &'static str,
    pub nodes: &'static [&'static str],
    pub bytes: u64,
    /// Weight shards (model-00001-of-000NN.safetensors).
    pub files: u32,
    /// The Hugging Face cache revision it was downloaded at; None for a folder under ~/models.
    pub hf_revision: Option<&'static str>,
}

impl ModelFolder {
    /// The folder's absolute path for a home directory, e.g. /home/sparkuser/models/meta-llama/Llama-3.3-70B-Instruct.
    pub fn path(&self, home: &str) -> String {
        let home = home.trim_end_matches('/');
        match self.hf_revision {
            Some(rev) => format!("{home}/{HF_HUB}/models--{}/snapshots/{rev}", self.repo.replace('/', "--")),
            None => format!("{home}/{MODELS_DIR}/{}", self.repo),
        }
    }

    /// The folder the library scan found it under: ~/models or the Hugging Face cache.
    pub fn root(&self, home: &str) -> String {
        format!("{}/{}", home.trim_end_matches('/'), if self.hf_revision.is_some() { HF_HUB } else { MODELS_DIR })
    }
}

/// The Hugging Face cache, under the cluster user's home.
pub const HF_HUB: &str = ".cache/huggingface/hub";

const fn folder(repo: &'static str, nodes: &'static [&'static str], bytes: u64, files: u32) -> ModelFolder {
    ModelFolder { repo, nodes, bytes, files, hf_revision: None }
}

/// The everyday world's folders: the served Llama and a Qwen to try.
pub const MODEL_FOLDERS: &[ModelFolder] = &[
    folder(SERVED_REPO, &["cell1", "cell2", "cell3"], 141_107_412_992, 30),
    // 28.8 GiB
    folder("Qwen/Qwen3.8-27B-FP8", &["cell1"], 30_923_764_531, 7),
];

/// What the nodes really hold (the Library in the Live tour, Oct 29): GLM-5.3-Flash in NVFP4
/// staged by hand under ~/models on all four, its W4A16 sibling and two smaller models in the
/// Hugging Face cache.
pub const AS_FOUND_FOLDERS: &[ModelFolder] = &[
    // 181 GiB
    folder("GLM-5.3-Flash-NVFP4", &["cell1", "cell2", "cell3", "gx10-0000"], 194_349_826_048, 40),
    ModelFolder {
        repo: "AIAgens/GLM-5.3-Flash-UNCENSORED-W4A16",
        nodes: &["cell1", "cell2", "cell3", "gx10-0000"],
        bytes: 194_352_117_760,
        files: 40,
        hf_revision: Some("6f0c3e8b2a41d97e5c1b0a3f8d2e64b9c7a15d03"),
    },
    // 51.1 GiB
    ModelFolder { repo: "google/gemma-3-27b-it", nodes: &["cell1", "cell2", "cell3"], bytes: 54_867_490_816, files: 12, hf_revision: Some("005ad3404e59d6023443cb575daa05336842228a") },
    // 3.8 GiB
    ModelFolder { repo: "Qwen/Qwen3-1.7B", nodes: &["cell1"], bytes: 4_080_218_931, files: 2, hf_revision: Some("70d244cc86ccca08cf5af4e1e306ecf908b1ad5e") },
];

/// Which world the simulator plays: the everyday showcase (a model served across the cluster),
/// or the cluster exactly as SparkBox found it in the Live tour on Oct 29.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum World {
    #[default]
    Everyday,
    AsFound,
}

impl World {
    /// The model folders on the nodes in this world.
    pub fn folders(self) -> &'static [ModelFolder] {
        match self {
            World::Everyday => MODEL_FOLDERS,
            World::AsFound => AS_FOUND_FOLDERS,
        }
    }
}

/// The time of the Live snapshot the as-found world reproduces (Oct 29, 07:43:55 EDT): every
/// real timestamp below is relative to it.
pub const FOUND_AT: u64 = 1_793_274_235;

/// A real Unix time from the Live tour, moved by whole days so it keeps its time of day and
/// its age in days: the glm53 run is "2 days ago" and the recabling "3 days ago, 2:16 PM"
/// whatever day the demo runs.
pub fn found(real: u64, now: u64) -> u64 {
    let off = crate::health::maintenance::local_offset_s();
    let day = |t: u64| (t as i64 + off).div_euclid(86_400);
    let days = day(now) - day(FOUND_AT);
    (real as i64 + days * 86_400).max(0) as u64
}

/// One machine as the doctor report and the Live snapshot show it.
#[derive(Clone, Copy, Debug)]
pub struct RealNode {
    pub id: &'static str,
    /// DMI platform, shown after the inventory's model: "MSI EdgeXpert (MS-C931)".
    pub platform: &'static str,
    pub bios: &'static str,
    pub serial: &'static str,
    /// enP7s7, enp1s0f0np0, enp1s0f1np1, enP2p1s0f0np0, enP2p1s0f1np1, wlP9s9, docker0.
    pub macs: [&'static str; 7],
    pub mem_total: u64,
    /// At idle: memory in use, page cache and swap in use.
    pub mem_used: u64,
    pub mem_cached: u64,
    pub swap_used: u64,
    pub disk_total: u64,
    pub disk_used: u64,
    pub efi_used: u64,
    /// At idle: GPU watts and °C, CPU °C (cell2 runs warmest).
    pub gpu_w: f32,
    pub gpu_c: f32,
    pub cpu_c: f32,
    /// Cumulative RoCE out-of-sequence and packet-sequence errors per half, left from the
    /// 4-node NCCL run: they form a ring (a node's sequence errors are its neighbour's
    /// out-of-sequence count) and never reset until a reboot.
    pub oos_seq: [(u64, u64); 2],
    pub tx_drops: [u64; 2],
    pub tx_pause: u64,
    /// Carrier changes on the cabled CX-7 port since boot.
    pub carrier: u64,
    /// Seconds the node had been up at FOUND_AT.
    pub uptime_s: u64,
    /// Wi-Fi: (carrier changes, address when associated).
    pub wifi: (u64, Option<&'static str>),
}

pub const REAL_NODES: [RealNode; 4] = [
    RealNode {
        id: "cell1",
        platform: "MS-C931",
        bios: "5.36_1.8.0",
        serial: "DEMO-SERIAL-0003",
        macs: ["fc:9d:05:d5:54:96", "fc:9d:05:71:99:97", "fc:9d:05:c3:bb:46", "fc:9d:05:ea:6a:92", "fc:9d:05:ed:d4:6c", "9c:c7:d3:b6:d1:8d", "0a:91:a0:58:5a:a9"],
        mem_total: 130_661_117_952,
        mem_used: 5_172_477_952,
        mem_cached: 3_985_915_904,
        swap_used: 73_752_576,
        disk_total: 4_030_802_149_376,
        disk_used: 1_212_625_289_216,
        efi_used: 7_733_248,
        gpu_w: 4.12,
        gpu_c: 45.0,
        cpu_c: 46.8,
        oos_seq: [(87, 341), (83, 341)],
        tx_drops: [6, 7],
        tx_pause: 54,
        carrier: 9,
        uptime_s: 311_751,
        wifi: (0, None),
    },
    RealNode {
        id: "cell2",
        platform: "MS-C931",
        bios: "5.36_1.8.0",
        serial: "DEMO-SERIAL-0001",
        macs: ["fc:9d:05:9b:e9:14", "fc:9d:05:cb:20:fe", "fc:9d:05:3d:3e:ae", "fc:9d:05:7c:8e:56", "fc:9d:05:d1:dc:1c", "9c:c7:d3:63:e8:45", "2e:4f:f1:32:a7:a0"],
        mem_total: 130_661_117_952,
        mem_used: 4_439_896_064,
        mem_cached: 3_850_792_960,
        swap_used: 94_027_776,
        disk_total: 4_030_802_149_376,
        disk_used: 961_690_247_168,
        efi_used: 6_680_576,
        gpu_w: 5.21,
        gpu_c: 49.0,
        cpu_c: 52.5,
        oos_seq: [(342, 248), (341, 250)],
        tx_drops: [5, 4],
        tx_pause: 72,
        carrier: 9,
        uptime_s: 311_718,
        wifi: (0, None),
    },
    RealNode {
        id: "cell3",
        platform: "MS-C931",
        bios: "5.36_1.8.0",
        serial: "DEMO-SERIAL-0002",
        macs: ["fc:9d:05:5e:bd:9c", "fc:9d:05:35:6e:f3", "fc:9d:05:ea:06:b7", "fc:9d:05:b7:ac:08", "fc:9d:05:33:7f:84", "9c:c7:d3:82:b2:2f", "e6:ff:c9:bf:d6:9d"],
        mem_total: 130_661_122_048,
        mem_used: 4_491_431_936,
        mem_cached: 4_792_160_256,
        swap_used: 42_315_776,
        disk_total: 4_030_802_149_376,
        disk_used: 1_054_891_638_784,
        efi_used: 6_680_576,
        gpu_w: 3.72,
        gpu_c: 44.0,
        cpu_c: 46.6,
        oos_seq: [(248, 167), (250, 169)],
        tx_drops: [4, 6],
        tx_pause: 76,
        carrier: 9,
        uptime_s: 311_707,
        // On the home LAN twice: its Wi-Fi joined the same /24 as enP7s7.
        wifi: (154, Some("192.168.50.229")),
    },
    RealNode {
        id: "gx10-0000",
        platform: "GX10",
        bios: "GX10DGX.0106.2026.0707.0731",
        serial: "DEMO-SERIAL-0004",
        macs: ["30:c5:99:79:4d:07", "30:c5:99:7a:0e:4a", "30:c5:99:ad:8d:87", "30:c5:99:0d:7a:72", "30:c5:99:f7:54:60", "50:bb:b5:13:89:c6", "b2:0d:71:b3:9b:33"],
        mem_total: 130_594_021_376,
        mem_used: 4_932_288_512,
        mem_cached: 2_956_083_200,
        swap_used: 69_230_592,
        disk_total: 982_819_848_192,
        disk_used: 373_903_597_568,
        efi_used: 7_729_152,
        gpu_w: 5.37,
        gpu_c: 44.0,
        cpu_c: 45.7,
        oos_seq: [(166, 87), (169, 83)],
        tx_drops: [6, 5],
        tx_pause: 46,
        carrier: 11,
        uptime_s: 323_790,
        wifi: (1, None),
    },
];

/// 16 GiB of swap on every node.
pub const SWAP_TOTAL: u64 = 17_179_865_088;

/// The real machine behind a node: by id, else by its place in the inventory.
pub fn real(node: &str, index: usize) -> &'static RealNode {
    REAL_NODES.iter().find(|r| r.id == node).unwrap_or(&REAL_NODES[index % REAL_NODES.len()])
}

const IFACES: [&str; 7] = [LAN_IFACE, "enp1s0f0np0", "enp1s0f1np1", "enP2p1s0f0np0", "enP2p1s0f1np1", WIFI_IFACE, "docker0"];

/// Management addresses on the home LAN (192.168.50.0/24), by inventory switch id.
pub const SWITCH_MGMT_IPS: &[(&str, &str)] = &[("crs804", "192.168.50.2"), ("firewalla", "192.168.50.3"), ("gold", "192.168.50.1")];

pub fn switch_mgmt_ip(switch_id: &str) -> Option<&'static str> {
    SWITCH_MGMT_IPS.iter().find(|(id, _)| *id == switch_id).map(|(_, ip)| *ip)
}

/// The fabric address of a node: `half` 0 is enp1s0f1np1 (192.168.100.x), 1 is enP2p1s0f1np1 (192.168.101.x).
pub fn fabric_ip(node_index: usize, half: usize) -> String {
    format!("{}.{}", FABRIC[half % 2].1, node_index + 1)
}

/// The CRS804 port a node's fabric cable goes to: two 200G breakout ports per QSFP56-DD cage.
pub fn crs_port(node_index: usize) -> String {
    format!("qsfp56-dd-{}-1", node_index + 1)
}

fn hex(seed: &str, len: usize) -> String {
    Sha256::digest(seed.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()[..len].to_string()
}

/// A stable 12-hex container id, so ids survive restarts of SparkBox and match across views.
pub fn container_id(node: &str, name: &str) -> String {
    hex(&format!("container/{node}/{name}"), 12)
}

/// The full 64-hex id `docker inspect` shows.
pub fn container_full_id(node: &str, name: &str) -> String {
    hex(&format!("container/{node}/{name}"), 64)
}

/// A node's interface MAC: the real one for the four known machines (their own block: LAN,
/// then the CX-7 functions), a stable made-up one for any other node.
pub fn mac(node: &str, iface: &str) -> String {
    if let (Some(r), Some(k)) = (REAL_NODES.iter().find(|r| r.id == node), IFACES.iter().position(|i| *i == iface)) {
        return r.macs[k].to_string();
    }
    let h = hex(&format!("mac/{node}/{iface}"), 6);
    let oui = if iface.starts_with("enp1") || iface.starts_with("enP2") { "94:6d:ae" } else { "4c:bb:47" };
    format!("{oui}:{}:{}:{}", &h[0..2], &h[2..4], &h[4..6])
}

/// The cluster user (the same account on every node).
pub fn user() -> String {
    Inventory::example().nodes.first().map(|n| n.user.clone()).unwrap_or_else(|| "sparkuser".into())
}

pub fn home() -> String {
    format!("/home/{}", user())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_and_well_formed() {
        assert_eq!(fabric_ip(0, 0), "192.168.100.1");
        assert_eq!(fabric_ip(3, 1), "192.168.101.4");
        assert_eq!(crs_port(2), "qsfp56-dd-3-1");
        let id = container_id("cell1", "vllm-head");
        assert_eq!(id.len(), 12);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id, container_id("cell1", "vllm-head"));
        assert_ne!(id, container_id("cell2", "vllm-head"));
        assert!(container_full_id("cell1", "vllm-head").starts_with(&id));
        assert_eq!(mac("cell1", "enp1s0f1np1"), "fc:9d:05:c3:bb:46", "the real MAC");
        assert_eq!(mac("gx10-0000", LAN_IFACE), "30:c5:99:79:4d:07");
        assert_eq!(mac("cell9", "enp1s0f1np1").len(), 17);
        assert!(mac("cell9", "enp1s0f1np1").starts_with("94:6d:ae"));
        assert_eq!(MODEL_FOLDERS[0].path("/home/sparkuser"), "/home/sparkuser/models/meta-llama/Llama-3.3-70B-Instruct");
        assert_eq!(AS_FOUND_FOLDERS[0].path("/home/sparkuser"), "/home/sparkuser/models/GLM-5.3-Flash-NVFP4");
        assert!(AS_FOUND_FOLDERS[1].path("/home/sparkuser").starts_with("/home/sparkuser/.cache/huggingface/hub/models--AIAgens--GLM-5.3-Flash-UNCENSORED-W4A16/snapshots/"));
        assert_eq!(AS_FOUND_FOLDERS[2].root("/home/sparkuser"), "/home/sparkuser/.cache/huggingface/hub");
        assert_eq!(real("gx10-0000", 0).bios, "GX10DGX.0106.2026.0707.0731");
        assert_eq!(real("lab9", 5).id, "cell2", "an unknown node borrows a real one by position");
        // Real times keep their time of day and their age in days.
        let now = FOUND_AT + 3 * 86_400 + 600;
        assert_eq!(found(FOUND_AT - 2 * 86_400, now), FOUND_AT + 86_400);
        assert_eq!(switch_mgmt_ip("gold"), Some("192.168.50.1"));
        assert_eq!(user(), "sparkuser");
    }
}
