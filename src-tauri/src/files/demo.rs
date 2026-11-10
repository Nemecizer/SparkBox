//! Demo-mode node filesystems, kept in memory. They follow the demo world (crate::demo::world):
//! the model folders the Models window lists, /etc/hosts and netplan with the fabric address
//! plan, root-owned system files, a large log, symlinks, a freedesktop Trash and the editor's
//! backups. The demo Terminal reads and writes the same files. In the as-found world the homes
//! are the owner's as the Live tour showed them (cell1's playbooks and NCCL builds, ~/glm53 with
//! the real inner.sh) and /etc/netplan holds the real three files, root-only; the secrets in
//! ~/glm53 and hf_token.txt are placeholders.

use std::collections::{BTreeMap, HashMap};

use sha2::{Digest, Sha256};

use super::transfer::{Placed, Policy, Side};
use super::{Backup, Entry, Listing, SaveError, Saved, TextFile, Trashed};
use crate::demo::world::{self, World};
use crate::inventory::Inventory;

const PEEK: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub dir: bool,
    /// Symlink target.
    pub link: Option<String>,
    pub size: u64,
    pub mtime: u64,
    pub mode: u32,
    pub owner: String,
    /// Text files; None for binary ones (weights) and folders.
    pub content: Option<String>,
    /// For a large text file: its first and last lines (the editor's read-only viewer).
    pub peek: Option<(String, String)>,
}

impl Item {
    fn dir(mode: u32, owner: &str, mtime: u64) -> Item {
        Item { dir: true, link: None, size: 4096, mtime, mode, owner: owner.into(), content: None, peek: None }
    }

    fn text(text: impl Into<String>, mode: u32, owner: &str, mtime: u64) -> Item {
        let text = text.into();
        Item { dir: false, link: None, size: text.len() as u64, mtime, mode, owner: owner.into(), content: Some(text), peek: None }
    }

    fn blob(size: u64, mtime: u64, owner: &str) -> Item {
        Item { dir: false, link: None, size, mtime, mode: 0o644, owner: owner.into(), content: None, peek: None }
    }

    fn link(target: &str, owner: &str, mtime: u64) -> Item {
        Item { dir: false, link: Some(target.into()), size: target.len() as u64, mtime, mode: 0o777, owner: owner.into(), content: None, peek: None }
    }
}

pub struct DemoFs {
    nodes: HashMap<String, BTreeMap<String, Item>>,
    user: String,
    /// The world the trees were seeded for; another one starts them over.
    world: World,
    /// What the seeded files weighed per node, so `space` reads the real disk's free space.
    seeded: HashMap<String, u64>,
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn sha_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

fn join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

pub(crate) fn mode_string(item: &Item) -> String {
    let bits: String = (0..9).map(|i| if item.mode & (0o400 >> i) != 0 { ['r', 'w', 'x'][i % 3] } else { '-' }).collect();
    let kind = if item.link.is_some() { 'l' } else if item.dir { 'd' } else { '-' };
    format!("{kind}{bits}")
}

/// The demo node's place in the inventory (cell1 is 0), for its addresses.
fn node_index(node: &str) -> usize {
    let inv = Inventory::example();
    inv.nodes.iter().position(|n| n.id == node).unwrap_or(inv.nodes.len())
}

/// The node's own hostname, as `hostname` prints it ("cell1-example"), so /etc/hostname,
/// /etc/hosts and the shell agree.
fn host_name(node: &str) -> String {
    let inv = Inventory::example();
    inv.nodes.iter().find(|n| n.id == node).map(|n| n.host.trim_end_matches(".local").to_string()).unwrap_or_else(|| node.to_string())
}

fn hosts_file(node: &str) -> String {
    let inv = Inventory::example();
    let mut s = format!("127.0.0.1 localhost\n127.0.1.1 {}\n\n# Home LAN ({})\n", host_name(node), world::LAN_IFACE);
    for n in &inv.nodes {
        s.push_str(&format!("{:<15} {}\n", n.ip.clone().unwrap_or_default(), n.host));
    }
    s.push_str(&format!("\n# 200G fabric ({}, MTU {})\n", world::FABRIC[0].0, world::FABRIC_MTU));
    for (i, n) in inv.nodes.iter().enumerate() {
        s.push_str(&format!("{:<15} {}-fabric\n", world::fabric_ip(i, 0), n.id));
    }
    s.push_str(&format!("\n# Second fabric link ({})\n", world::FABRIC[1].0));
    for (i, n) in inv.nodes.iter().enumerate() {
        s.push_str(&format!("{:<15} {}-fabric2\n", world::fabric_ip(i, 1), n.id));
    }
    s.push_str("\n# The following lines are desirable for IPv6 capable hosts\n::1     ip6-localhost ip6-loopback\nff02::1 ip6-allnodes\nff02::2 ip6-allrouters\n");
    s
}

fn netplan_fabric(i: usize) -> String {
    let mut s = String::from("# ConnectX-7 fabric links to the CRS804 (written by the cluster setup)\nnetwork:\n  version: 2\n  ethernets:\n");
    for half in 0..2 {
        s.push_str(&format!(
            "    {}:\n      dhcp4: false\n      addresses: [{}/{}]\n      mtu: {}\n",
            world::FABRIC[half].0,
            world::fabric_ip(i, half),
            world::FABRIC_PREFIX,
            world::FABRIC_MTU
        ));
    }
    s
}

fn syslog_line(node: &str, n: u64) -> String {
    let msgs = [
        "systemd[1]: Started docker-%.scope - libcontainer container %.",
        "kernel: mlx5_core 0000:01:00.1 enp1s0f1np1: Link up",
        "dockerd[1388]: time=\"2026-10-28T09:14:%\" level=info msg=\"ignoring event\" module=libcontainerd namespace=moby",
        "nvidia-persistenced[1102]: device 000f:01:00.0 - persistence mode enabled.",
        "systemd[1]: Starting sysstat-collect.service - system activity accounting tool...",
        "CRON[%]: (root) CMD (command -v debian-sa1 > /dev/null && debian-sa1 1 1)",
        "systemd-resolved[612]: Clock change detected. Flushing caches.",
    ];
    let m = msgs[(n % msgs.len() as u64) as usize].replace('%', &format!("{:02}", n % 60));
    format!("Oct 28 {:02}:{:02}:{:02} {node} {m}\n", (n / 3600) % 24, (n / 60) % 60, n % 60)
}

fn big_log(node: &str, size: u64) -> Item {
    let head: String = (0..900).map(|n| syslog_line(node, n)).collect::<String>().chars().take(PEEK).collect();
    let tail: String = (40_000..40_900).map(|n| syslog_line(node, n)).collect();
    let tail = tail[tail.len().saturating_sub(PEEK)..].to_string();
    Item { peek: Some((head, tail)), ..Item::blob(size, now() - 40, "syslog") }
}

fn model_config(repo: &str) -> String {
    if repo.contains("GLM-5.3") {
        return format!(
            "{{\n  \"architectures\": [\"Glm5NextForConditionalGeneration\"],\n  \"model_type\": \"glm5_next\",\n  \"hidden_size\": 5120,\n  \"num_hidden_layers\": 64,\n  \"num_attention_heads\": 64,\n  \"kv_lora_rank\": 512,\n  \"q_lora_rank\": 1536,\n  \"qk_rope_head_dim\": 64,\n  \"index_topk\": 2048,\n  \"n_routed_experts\": 256,\n  \"num_experts_per_tok\": 8,\n  \"num_nextn_predict_layers\": 1,\n  \"max_position_embeddings\": 524288,\n  \"quantization_config\": {{\"quant_method\": \"{}\"}},\n  \"torch_dtype\": \"bfloat16\",\n  \"vocab_size\": 154880\n}}\n",
            if repo.contains("NVFP4") { "modelopt_fp4" } else { "compressed-tensors" }
        );
    }
    if repo.contains("gemma") {
        return "{\n  \"architectures\": [\"Gemma3ForConditionalGeneration\"],\n  \"model_type\": \"gemma3\",\n  \"text_config\": {\"hidden_size\": 5376, \"num_hidden_layers\": 62, \"num_attention_heads\": 32, \"num_key_value_heads\": 16, \"sliding_window\": 1024},\n  \"torch_dtype\": \"bfloat16\"\n}\n".into();
    }
    if repo.contains("Qwen3-1.7B") {
        return "{\n  \"architectures\": [\"Qwen3ForCausalLM\"],\n  \"hidden_size\": 2048,\n  \"intermediate_size\": 6144,\n  \"max_position_embeddings\": 40960,\n  \"model_type\": \"qwen3\",\n  \"num_attention_heads\": 16,\n  \"num_hidden_layers\": 28,\n  \"num_key_value_heads\": 8,\n  \"torch_dtype\": \"bfloat16\",\n  \"vocab_size\": 151936\n}\n".into();
    }
    if repo.contains("Llama") {
        "{\n  \"architectures\": [\"LlamaForCausalLM\"],\n  \"bos_token_id\": 128000,\n  \"eos_token_id\": [128001, 128008, 128009],\n  \"hidden_act\": \"silu\",\n  \"hidden_size\": 8192,\n  \"intermediate_size\": 28672,\n  \"max_position_embeddings\": 131072,\n  \"model_type\": \"llama\",\n  \"num_attention_heads\": 64,\n  \"num_hidden_layers\": 80,\n  \"num_key_value_heads\": 8,\n  \"rms_norm_eps\": 1e-05,\n  \"rope_theta\": 500000.0,\n  \"torch_dtype\": \"bfloat16\",\n  \"vocab_size\": 128256\n}\n".into()
    } else {
        "{\n  \"architectures\": [\"Qwen3ForCausalLM\"],\n  \"hidden_size\": 5120,\n  \"intermediate_size\": 25600,\n  \"max_position_embeddings\": 40960,\n  \"model_type\": \"qwen3\",\n  \"num_attention_heads\": 64,\n  \"num_hidden_layers\": 64,\n  \"num_key_value_heads\": 8,\n  \"quantization_config\": {\"quant_method\": \"fp8\", \"weight_block_size\": [128, 128]},\n  \"torch_dtype\": \"bfloat16\",\n  \"vocab_size\": 151936\n}\n".into()
    }
}

fn seed(node: &str, user: &str, world: World) -> BTreeMap<String, Item> {
    let home = format!("/home/{user}");
    let t = now() - 86_400 * 3;
    let i = node_index(node);
    let mut m = BTreeMap::new();
    let root_dirs = ["/", "/etc", "/etc/netplan", "/etc/docker", "/etc/sudoers.d", "/etc/systemd", "/etc/systemd/system", "/home", "/var", "/var/log", "/run", "/run/systemd", "/run/systemd/resolve"];
    for d in root_dirs {
        m.insert(d.to_string(), Item::dir(0o755, "root", t));
    }
    m.insert("/etc/sudoers.d".into(), Item::dir(0o750, "root", t));
    // Everyone may create files in /tmp; the sticky bit keeps them each other's to remove.
    m.insert("/tmp".into(), Item::dir(0o1777, "root", t));
    m.insert("/tmp/systemd-private-3f1c-systemd-resolved.service-Hk2Lq9".into(), Item::dir(0o700, "root", t));
    for d in ["", "/models", "/.cache", "/.cache/huggingface", "/.cache/huggingface/hub", "/.config", "/.local", "/.local/share", "/.sparkbox"] {
        m.insert(format!("{home}{d}"), Item::dir(if d.is_empty() { 0o750 } else { 0o755 }, user, t));
    }
    m.insert("/etc/hostname".into(), Item::text(format!("{}\n", host_name(node)), 0o644, "root", t));
    m.insert("/etc/hosts".into(), Item::text(hosts_file(node), 0o644, "root", t));
    if world == World::AsFound {
        // As installed: subiquity's LAN file, the CX-7 file the fabric setup wrote, and
        // NetworkManager's own. netplan wants them root-only, so reading one needs sudo.
        m.insert(
            "/etc/netplan/00-installer-config.yaml".into(),
            Item::text(format!("# This is the network config written by 'subiquity'\nnetwork:\n  ethernets:\n    {}:\n      dhcp4: true\n  version: 2\n", world::LAN_IFACE), 0o600, "root", t),
        );
        m.insert("/etc/netplan/40-cx7.yaml".into(), Item::text(netplan_fabric(i), 0o600, "root", t));
        m.insert(format!("/etc/netplan/90-NM-{NM_UUID}.yaml"), Item::text(netplan_nm(), 0o600, "root", t));
    } else {
        m.insert("/etc/netplan/40-cx7-fabric.yaml".into(), Item::text(netplan_fabric(i), 0o644, "root", t));
        m.insert(
            "/etc/netplan/50-cloud-init.yaml".into(),
            Item::text(format!("network:\n  version: 2\n  ethernets:\n    {}:\n      dhcp4: true\n      optional: true\n", world::LAN_IFACE), 0o600, "root", t),
        );
    }
    m.insert(
        "/etc/docker/daemon.json".into(),
        Item::text("{\n  \"default-runtime\": \"nvidia\",\n  \"runtimes\": {\n    \"nvidia\": {\n      \"path\": \"nvidia-container-runtime\",\n      \"args\": []\n    }\n  },\n  \"log-opts\": { \"max-size\": \"50m\", \"max-file\": \"3\" }\n}\n", 0o644, "root", t),
    );
    m.insert("/etc/sudoers.d/README".into(), Item::text("#\n# Files in /etc/sudoers.d are read by sudo. Check them with `visudo -cf <file>` before saving.\n#\n", 0o440, "root", t));
    m.insert(
        "/etc/systemd/system/sparkbox-fabric-mtu.service".into(),
        Item::text(format!("[Unit]\nDescription=Set jumbo frames on the CX-7 fabric links\nAfter=network-online.target\n\n[Service]\nType=oneshot\nExecStart=/usr/sbin/ip link set {} mtu {}\nExecStart=/usr/sbin/ip link set {} mtu {}\n\n[Install]\nWantedBy=multi-user.target\n", world::FABRIC[0].0, world::FABRIC_MTU, world::FABRIC[1].0, world::FABRIC_MTU), 0o644, "root", t),
    );
    m.insert("/run/systemd/resolve/stub-resolv.conf".into(), Item::text("nameserver 127.0.0.53\noptions edns0 trust-ad\nsearch local\n", 0o644, "systemd-resolve", t));
    m.insert("/etc/resolv.conf".into(), Item::link("../run/systemd/resolve/stub-resolv.conf", "root", t));
    m.insert("/var/log/syslog".into(), big_log(node, 18_421_337 + i as u64 * 1_310_731));
    m.insert("/var/log/dpkg.log".into(), Item::text(format!("2026-10-24 06:12:40 upgrade docker-ce:arm64 5:28.3.1-1~ubuntu.24.04~noble 5:{}-1~ubuntu.24.04~noble\n2026-10-24 06:12:52 status installed docker-ce:arm64 5:{}-1~ubuntu.24.04~noble\n", world::DOCKER, world::DOCKER), 0o644, "root", t));

    m.insert(format!("{home}/.bashrc"), Item::text("# ~/.bashrc\ncase $- in *i*) ;; *) return;; esac\nHISTCONTROL=ignoreboth\nexport HF_HOME=~/.cache/huggingface\nalias gpu='nvidia-smi'\nalias dps='docker ps --format \"table {{.Names}}\\t{{.Status}}\\t{{.Image}}\"'\n", 0o644, user, t));
    m.insert(format!("{home}/.profile"), Item::text("# ~/.profile\nif [ -n \"$BASH_VERSION\" ] && [ -f \"$HOME/.bashrc\" ]; then . \"$HOME/.bashrc\"; fi\nPATH=\"$HOME/.local/bin:$PATH\"\n", 0o644, user, t));
    if world == World::AsFound {
        found_home(&mut m, node, user, &home);
    } else {
        everyday_home(&mut m, node, user, &home, i);
    }
    model_folders(&mut m, node, user, &home, world);
    m
}

/// NetworkManager's netplan file for the LAN connection (no secrets in it).
const NM_UUID: &str = "9a6d7c52-4b1e-4f0e-8a5d-2f3c61b0e7a4";

fn netplan_nm() -> String {
    format!(
        "network:\n  version: 2\n  ethernets:\n    NM-{NM_UUID}:\n      renderer: NetworkManager\n      match:\n        name: \"{}\"\n      dhcp4: true\n      dhcp6: true\n      networkmanager:\n        uuid: \"{NM_UUID}\"\n        name: \"netplan-{}\"\n",
        world::LAN_IFACE,
        world::LAN_IFACE
    )
}

/// The script each glm53 container runs (`/bin/bash /glm53/inner.sh <rank>`): the owner's own,
/// as the editor showed it in the Live tour. Line 22 hard-codes the bf16 KV cache that no
/// attention backend on GB10 supports with sparse MLA.
pub(crate) const INNER_SH: &str = include_str!("../demo/fixtures/inner.sh");

/// How the owner started the 4-node run on each node. Rebuilt from the containers' `docker
/// inspect` (host network and IPC, 16 GiB shm, every GPU, the five mounts, config.env): the
/// real file wasn't captured.
fn node_launch_sh() -> String {
    "#!/bin/bash\n# Starts this node's rank of the 4-node GLM-5.3-Flash run: ./node-launch.sh <rank 0-3>\nset -euo pipefail\nRANK=\"${1:?rank 0-3}\"\nHERE=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nIMAGE=vllm/vllm-openai:glm53-flash-arm64-cu130\n\ndocker rm -f glm53 2>/dev/null || true\nmkdir -p ~/.cache/flashinfer ~/.triton ~/.cache/vllm\n\ndocker run -d --name glm53 \\\n  --network host --ipc host --shm-size 16g --gpus all \\\n  --env-file \"$HERE/config.env\" \\\n  -e API_KEY=\"$(cat \"$HERE/api_key\")\" \\\n  -v ~/.cache/flashinfer:/root/.cache/flashinfer \\\n  -v ~/.triton:/root/.triton \\\n  -v \"$HERE\":/glm53:ro \\\n  -v ~/models:/models:ro \\\n  -v ~/.cache/vllm:/root/.cache/vllm \\\n  \"$IMAGE\" /bin/bash /glm53/inner.sh \"$RANK\"\n\necho \"glm53 rank $RANK started; follow it with: docker logs -f glm53\"\n".into()
}

/// The as-found homes (cell1's as the tour's Files showed it, with its real times): ~/glm53 on
/// every node, whose key files are placeholders; cell1 also has the desktop folders, the
/// playbooks, the NCCL builds and the scripts.
fn found_home(m: &mut BTreeMap<String, Item>, node: &str, user: &str, home: &str) {
    let t = now() - 86_400 * 3;
    // A time from the tour, keeping its time of day and its age in days (demo::world::found).
    let at = |real: u64| crate::demo::world::found(real, now());
    // Oct 26, 3:30 PM: the owner wrote ~/glm53 before the 6:20 PM run.
    let run = at(1_793_043_000);
    m.insert(format!("{home}/.cache/huggingface/hub/version.txt"), Item::text("1", 0o644, user, t));
    for d in ["/.cache/flashinfer", "/.cache/vllm", "/.triton"] {
        m.insert(format!("{home}{d}"), Item::dir(0o755, user, run));
    }
    m.insert(format!("{home}/glm53"), Item::dir(0o775, user, run));
    // Oct 26, 5:03 PM: the NVFP4 weights staged by hand.
    m.insert(format!("{home}/models"), Item::dir(0o775, user, at(1_793_048_580)));
    m.insert(format!("{home}/.bash_logout"), Item::text("# ~/.bash_logout: executed by bash(1) when login shell exits.\n", 0o644, user, at(1_776_610_620)));
    m.insert(format!("{home}/.sudo_as_admin_successful"), Item::text("", 0o644, user, at(1_776_610_620)));
    m.insert(format!("{home}/glm53/inner.sh"), Item::text(INNER_SH, 0o775, user, run));
    m.insert(format!("{home}/glm53/node-launch.sh"), Item::text(node_launch_sh(), 0o775, user, run));
    m.insert(format!("{home}/glm53/api_key"), Item::text(format!("{}\n", crate::demo::found::API_KEY_PLACEHOLDER), 0o600, user, run));
    m.insert(
        format!("{home}/glm53/config.env"),
        Item::text(
            "# Read by node-launch.sh (docker run --env-file). Placeholder in Demo: the real one stays on the node.\nPORT=8000\nNNODES=4\nHEAD_IP=192.168.100.1\nMASTER_PORT=29501\nSERVED_NAME=glm-5.3-flash\nMODEL_LOCAL_NAME=GLM-5.3-Flash-NVFP4\nMAX_MODEL_LEN=524288\nMAX_NUM_SEQS=8\nGPU_MEM_UTIL=0.87\nMTP_TOKENS=5\nMOE_BACKEND=\nNCCL_SOCKET_IFNAME=enp1s0f1np1\nNCCL_IB_HCA=rocep1s0f1,roceP2p1s0f1\nNCCL_IB_DISABLE=0\nNCCL_IGNORE_CPU_AFFINITY=1\nGLOO_SOCKET_IFNAME=enp1s0f1np1\nTP_SOCKET_IFNAME=enp1s0f1np1\nMN_IF_NAME=enp1s0f1np1\nUCX_NET_DEVICES=enp1s0f1np1\nOMPI_MCA_btl_tcp_if_include=enp1s0f1np1\n",
            0o600,
            user,
            run,
        ),
    );
    if node != "cell1" {
        return;
    }
    let older = now() - 86_400 * 19;
    // The desktop's own folders, from the first login on Apr 19 (Downloads used May 9, Desktop Oct 4).
    for (d, real) in [("/Desktop", 1_791_128_100), ("/Documents", 1_776_610_620), ("/Downloads", 1_778_330_280), ("/Music", 1_776_610_620), ("/Pictures", 1_776_610_620), ("/Public", 1_776_610_620), ("/Templates", 1_776_610_620), ("/Videos", 1_776_610_620), ("/snap", 1_776_610_620)] {
        m.insert(format!("{home}{d}"), Item::dir(0o755, user, at(real)));
    }
    // The playbooks (pulled Oct 24), the NCCL builds of the 4-node run (Oct 26) and the mesh plugin (May 23).
    for (d, real) in [("/dgx-spark-playbooks", 1_792_869_000), ("/nccl", 1_793_041_320), ("/nccl-mesh-plugin", 1_779_578_640), ("/nccl-tests", 1_793_041_440)] {
        m.insert(format!("{home}{d}"), Item::dir(0o775, user, at(real)));
    }
    for d in ["/dgx-spark-playbooks/nvidia", "/nccl/build", "/nccl-tests/build", "/spark-vllm-docker"] {
        m.insert(format!("{home}{d}"), Item::dir(0o775, user, older));
    }
    m.insert(format!("{home}/dgx-spark-playbooks/README.md"), Item::text("# DGX Spark Playbooks\n\nStep-by-step guides for setting up and running workloads on NVIDIA DGX Spark.\n", 0o664, user, older));
    m.insert(format!("{home}/nccl-tests/build/all_reduce_perf"), Item { mode: 0o775, ..Item::blob(2_918_456, older, user) });
    m.insert(format!("{home}/nccl/build/lib"), Item::dir(0o775, user, older));
    m.insert(format!("{home}/nccl-mesh-plugin/README.md"), Item::text("# NCCL mesh plugin\n\nNet plugin for a switchless mesh of DGX Sparks.\n", 0o664, user, older));
    m.insert(format!("{home}/spark-vllm-docker/Dockerfile"), Item::text("FROM vllm/vllm-openai:nightly\n# Rebuilt for aarch64 with CUDA 13.0 for GB10 (sm_121).\nENV TORCH_CUDA_ARCH_LIST=\"12.1a\"\n", 0o664, user, older));
    m.insert(format!("{home}/glm53flash.sh"), Item::text("#!/bin/bash\n# Starts the GLM-5.3-Flash run on all four nodes, rank 0 (this node) last.\nset -e\nfor n in 3:192.168.100.4 2:192.168.100.3 1:192.168.100.2; do\n  ssh \"${n#*:}\" \"~/glm53/node-launch.sh ${n%%:*}\"\ndone\n~/glm53/node-launch.sh 0\n", 0o775, user, run));
    m.insert(format!("{home}/jumbo-frames.sh"), Item::text("#!/bin/bash\n# MTU 9000 on the CX-7 links that have a cable.\nfor i in enp1s0f1np1 enP2p1s0f1np1; do sudo ip link set \"$i\" mtu 9000; done\n", 0o775, user, older));
    m.insert(format!("{home}/discover-sparks"), Item { mode: 0o775, ..Item::blob(1_204_224, older, user) });
    m.insert(format!("{home}/hf_token.txt"), Item::text("hf_placeholder_not_a_real_token\n", 0o600, user, older));
}

/// The everyday world's home: notes, a benchmark project, the model folders.
fn everyday_home(m: &mut BTreeMap<String, Item>, node: &str, user: &str, home: &str, i: usize) {
    let t = now() - 86_400 * 3;
    for d in ["/projects", "/projects/bench"] {
        m.insert(format!("{home}{d}"), Item::dir(0o755, user, t));
    }
    m.insert(format!("{home}/notes.txt"), Item::text(format!("{node}\n- fabric: {} / {}\n- check MTU 9000 on both CX-7 links after updates\n", world::fabric_ip(i, 0), world::fabric_ip(i, 1)), 0o644, user, t));
    m.insert(format!("{home}/.cache/huggingface/hub/version.txt"), Item::text("1", 0o644, user, t));
    m.insert(
        format!("{home}/projects/bench/sweep.yaml"),
        Item::text(format!("# Throughput sweep for the served model\nmodel: {}\nendpoint: http://{}:8000/v1\nconcurrency: [1, 4, 16, 64]\nprompt_tokens: 1024\noutput_tokens: 256\nrepeat: 3\n", world::SERVED_NAME, world::fabric_ip(0, 0)), 0o644, user, t),
    );
    m.insert(
        format!("{home}/projects/bench/run_bench.py"),
        Item::text("#!/usr/bin/env python3\n\"\"\"Runs the sweep in sweep.yaml against the vLLM endpoint.\"\"\"\nimport sys\nimport time\n\nimport yaml\n\n\ndef main(path=\"sweep.yaml\"):\n    with open(path) as f:\n        cfg = yaml.safe_load(f)\n    for c in cfg[\"concurrency\"]:\n        start = time.time()\n        print(f\"concurrency={c} ...\", flush=True)\n        time.sleep(0.1)\n        print(f\"  done in {time.time() - start:.1f}s\")\n\n\nif __name__ == \"__main__\":\n    main(*sys.argv[1:])\n", 0o755, user, t),
    );
    m.insert(format!("{home}/projects/bench/results.csv"), Item::text("concurrency,prompt_tps,gen_tps,ttft_p50_ms\n1,412.0,38.1,182\n4,1460.2,131.7,240\n16,4120.8,402.3,515\n64,6890.1,688.9,1340\n", 0o644, user, t));
    if world::MODEL_FOLDERS.iter().any(|f| f.repo == world::SERVED_REPO && f.nodes.contains(&node)) {
        m.insert(format!("{home}/models/latest"), Item::link(world::SERVED_REPO, user, t));
    }
}

/// Model folders, as the Models window and the containers' /models mounts see them.
fn model_folders(m: &mut BTreeMap<String, Item>, node: &str, user: &str, home: &str, world: World) {
    let t = now() - 86_400 * 3;
    for f in world.folders().iter().filter(|f| f.nodes.contains(&node)) {
        let dir = f.path(home);
        // Every folder between home and the model (org folders, the cache's models--x/snapshots).
        let mut up = parent(&dir).to_string();
        while up.len() > home.len() && !m.contains_key(&up) {
            m.insert(up.clone(), Item::dir(0o755, user, t));
            up = parent(&up).to_string();
        }
        m.insert(dir.clone(), Item::dir(0o755, user, t));
        let small = [
            ("config.json", model_config(f.repo)),
            ("generation_config.json", "{\n  \"bos_token_id\": 128000,\n  \"do_sample\": true,\n  \"temperature\": 0.6,\n  \"top_p\": 0.9\n}\n".to_string()),
            ("tokenizer_config.json", "{\n  \"add_bos_token\": true,\n  \"model_max_length\": 131072,\n  \"tokenizer_class\": \"PreTrainedTokenizerFast\"\n}\n".to_string()),
            (
                "README.md",
                if world == World::AsFound {
                    format!("---\nlicense: mit\nlibrary_name: transformers\n---\n\n# {}\n", f.repo.rsplit('/').next().unwrap_or(f.repo))
                } else {
                    format!("# {}\n\nDownloaded with SparkBox. Served by vLLM ({}) with the folder mounted at /models.\n", f.repo, world::VLLM_IMAGE)
                },
            ),
        ];
        let mut used = 0;
        for (name, text) in small {
            used += text.len() as u64;
            m.insert(join(&dir, name), Item::text(text, 0o644, user, t));
        }
        // tokenizer.json is real JSON but too big for the editor: the head/tail viewer opens it.
        let tok = 17_209_920u64;
        used += tok;
        m.insert(
            join(&dir, "tokenizer.json"),
            Item { peek: Some(("{\n  \"version\": \"1.0\",\n  \"truncation\": null,\n  \"padding\": null,\n  \"added_tokens\": [\n    {\"id\": 128000, \"content\": \"<|begin_of_text|>\", \"special\": true},\n".into(), "      \"ĠÐ¿ÑĢÐ¾\": 128254,\n      \"ĠÑģÐ¾Ð·Ð´\": 128255\n    }\n  }\n}\n".into())), ..Item::blob(tok, t, user) },
        );
        let index = format!("{{\n  \"metadata\": {{\"total_size\": {}}},\n  \"weight_map\": {{ \"lm_head.weight\": \"model-{:05}-of-{:05}.safetensors\" }}\n}}\n", f.bytes, f.files, f.files);
        used += index.len() as u64;
        m.insert(join(&dir, "model.safetensors.index.json"), Item::text(index, 0o644, user, t));
        let shard = (f.bytes - used) / f.files as u64;
        for k in 1..=f.files {
            let size = if k == f.files { f.bytes - used - shard * (f.files as u64 - 1) } else { shard };
            m.insert(join(&dir, &format!("model-{k:05}-of-{:05}.safetensors", f.files)), Item::blob(size, t, user));
        }
    }
}

/// What a tree's files weigh (links and folders don't count).
fn used_bytes(fs: &BTreeMap<String, Item>) -> u64 {
    fs.values().filter(|i| !i.dir && i.link.is_none()).map(|i| i.size).sum()
}

fn conflict(path: &str, cur: Option<&Item>) -> SaveError {
    SaveError::ChangedOnDisk {
        message: format!("{path} changed on disk after you opened it."),
        sha256: cur.and_then(|i| i.content.as_ref()).map(|c| sha_hex(c.as_bytes())),
        mtime: cur.map(|i| i.mtime),
        size: cur.map(|i| i.size),
    }
}

impl DemoFs {
    pub fn new() -> DemoFs {
        // The same world the simulator starts in (SPARKBOX_DEMO_SCENARIO in a debug build).
        let world = if crate::demo::start_scenario().as_deref() == Some("as-found") { World::AsFound } else { World::Everyday };
        DemoFs { nodes: HashMap::new(), user: world::user(), world, seeded: HashMap::new() }
    }

    /// The trees of the cluster as found on Oct 29 (the as-found world), for tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn as_found() -> DemoFs {
        DemoFs { world: World::AsFound, ..DemoFs::new() }
    }

    /// Follows the simulator into another world: every node's tree starts over as that world
    /// has it (what was changed in the old one goes with it).
    pub fn set_world(&mut self, world: World) {
        if world != self.world {
            self.world = world;
            self.nodes.clear();
            self.seeded.clear();
        }
    }

    pub fn home(&self) -> String {
        format!("/home/{}", self.user)
    }

    fn fs(&mut self, node: &str) -> &mut BTreeMap<String, Item> {
        let (user, world) = (self.user.clone(), self.world);
        if !self.nodes.contains_key(node) {
            let tree = seed(node, &user, world);
            self.seeded.insert(node.to_string(), used_bytes(&tree));
            self.nodes.insert(node.to_string(), tree);
        }
        self.nodes.get_mut(node).expect("seeded")
    }

    /// "~", "~/x", relative-free absolute paths, "..": the canonical demo path.
    pub fn normalize(&self, path: &str) -> String {
        let home = self.home();
        let p = match path {
            "" | "~" => home,
            p if p.starts_with("~/") => format!("{home}/{}", &p[2..]),
            p => p.to_string(),
        };
        super::paths::clean(&p)
    }

    /// Follows symlinks in the folder part of a path (and at the end when `last`).
    fn resolved(&mut self, node: &str, path: &str, last: bool) -> String {
        self.fs(node);
        let mut p = self.normalize(path);
        for _ in 0..8 {
            let fs = &self.nodes[node];
            let parts: Vec<String> = p.split('/').filter(|s| !s.is_empty()).map(String::from).collect();
            let mut out = String::new();
            let mut jumped = None;
            for (k, part) in parts.iter().enumerate() {
                let next = format!("{out}/{part}");
                let at_end = k + 1 == parts.len();
                if let Some(target) = fs.get(&next).and_then(|i| i.link.clone()).filter(|_| !at_end || last) {
                    let base = if target.starts_with('/') { target } else { format!("{}/{target}", if out.is_empty() { "" } else { &out }) };
                    let rest = parts[k + 1..].join("/");
                    jumped = Some(super::paths::clean(&if rest.is_empty() { base } else { format!("{base}/{rest}") }));
                    break;
                }
                out = next;
            }
            match jumped {
                Some(j) => p = j,
                None => return if out.is_empty() { "/".into() } else { out },
            }
        }
        p
    }

    pub fn exists(&mut self, node: &str, path: &str) -> bool {
        let p = self.resolved(node, path, false);
        self.fs(node).contains_key(&p)
    }

    pub fn is_dir(&mut self, node: &str, path: &str) -> bool {
        let p = self.resolved(node, path, true);
        self.fs(node).get(&p).is_some_and(|i| i.dir)
    }

    /// Free and total bytes of the node's root disk: the real disk, less what was added since.
    pub fn space(&mut self, node: &str) -> (u64, u64) {
        let used = used_bytes(self.fs(node));
        let added = used as i64 - self.seeded.get(node).copied().unwrap_or(used) as i64;
        let real = world::real(node, node_index(node));
        let taken = (real.disk_used as i64 + added).max(0) as u64;
        (real.disk_total.saturating_sub(taken), real.disk_total)
    }

    pub fn list(&mut self, node: &str, path: &str) -> Result<Listing, String> {
        let path = self.resolved(node, path, true);
        let user = self.user.clone();
        let fs = self.fs(node);
        match fs.get(&path) {
            Some(i) if i.dir => {}
            Some(_) => return Err(format!("{path} is a file")),
            None => return Err(format!("can't open {path}: No such file or directory")),
        }
        if path.starts_with("/etc/sudoers.d") || (path.starts_with("/root") && user != "root") {
            return Err(format!("can't open {path}: Permission denied"));
        }
        let entries: Vec<Entry> = fs
            .iter()
            .filter(|(p, _)| p.as_str() != "/" && parent(p) == path)
            .map(|(p, i)| Entry {
                name: base(p).into(),
                kind: if i.link.is_some() { "link" } else if i.dir { "dir" } else { "file" }.into(),
                size: i.size,
                mtime: i.mtime,
                mode: mode_string(i),
                target: i.link.clone(),
                link_dir: false,
                owner: Some(i.owner.clone()),
            })
            .collect();
        let mut entries = entries;
        for e in entries.iter_mut().filter(|e| e.kind == "link") {
            let p = join(&path, &e.name);
            let real = self.resolved(node, &p, true);
            e.link_dir = self.fs(node).get(&real).is_some_and(|i| i.dir);
        }
        let (free, capacity) = self.space(node);
        Ok(Listing { place: node.into(), path, entries, free: Some(free), capacity: Some(capacity) })
    }

    fn get(&mut self, node: &str, path: &str) -> Result<(String, Item), String> {
        let p = self.resolved(node, path, true);
        let item = self.fs(node).get(&p).cloned().ok_or_else(|| format!("{p}: No such file or directory"))?;
        Ok((p, item))
    }

    pub fn read(&mut self, node: &str, path: &str) -> Result<TextFile, String> {
        let (path, item) = self.get(node, path)?;
        if item.dir {
            return Err(format!("{path} is a folder"));
        }
        if item.owner == "root" && item.mode & 0o004 == 0 && self.user != "root" {
            return Err(format!("can't read {path}: Permission denied (it's root-only; Fleet Command can read it with sudo)"));
        }
        let writable = item.owner == self.user;
        let base = TextFile {
            path: path.clone(),
            content: String::new(),
            size: item.size,
            mtime: item.mtime,
            sha256: String::new(),
            mode: mode_string(&item),
            owner: format!("{0}:{0}", item.owner),
            writable,
            large: false,
            head: None,
            tail: None,
        };
        match (&item.content, &item.peek) {
            (Some(c), _) => Ok(TextFile { content: c.clone(), sha256: sha_hex(c.as_bytes()), ..base }),
            (None, Some((h, t))) => Ok(TextFile { large: true, head: Some(h.clone()), tail: Some(t.clone()), ..base }),
            _ => Err(format!("{path} looks like a binary file")),
        }
    }

    /// Saves like a node does: conflict check, root-owned files need sudo, the old text is kept
    /// as a backup under ~/.sparkbox/backups (the newest five).
    pub fn commit(&mut self, node: &str, path: &str, content: &str, expect: Option<&str>, sudo: bool) -> Result<Saved, SaveError> {
        let path = self.resolved(node, path, true);
        let user = self.user.clone();
        let home = self.home();
        let fs = self.fs(node);
        if !fs.get(parent(&path)).is_some_and(|i| i.dir) {
            return Err(SaveError::Failed { message: format!("{} doesn't exist", parent(&path)) });
        }
        let current = fs.get(&path).cloned();
        if let Some(expect) = expect.filter(|e| !e.is_empty() && *e != "-") {
            let cur_sha = current.as_ref().and_then(|i| i.content.as_ref()).map(|c| sha_hex(c.as_bytes()));
            if cur_sha.as_deref() != Some(expect) {
                return Err(conflict(&path, current.as_ref()));
            }
        }
        let owner = current.as_ref().map(|i| i.owner.clone()).unwrap_or_else(|| user.clone());
        if owner != user && !sudo {
            return Err(SaveError::NeedsSudo { message: format!("{path} belongs to {owner}. Save it with sudo to keep its owner and permissions.") });
        }
        let mut backup = None;
        if let Some(cur) = current.as_ref().filter(|i| i.content.is_some()) {
            let dir = format!("{home}/.sparkbox/backups/{}", &sha_hex(path.as_bytes())[..16]);
            fs.entry(format!("{home}/.sparkbox/backups")).or_insert_with(|| Item::dir(0o700, &user, now()));
            fs.entry(dir.clone()).or_insert_with(|| Item::dir(0o700, &user, now()));
            let stamp = chrono_like(now());
            let mut name = format!("{}.{stamp}", base(&path));
            let mut n = 1;
            while fs.contains_key(&join(&dir, &name)) {
                n += 1;
                name = format!("{}.{stamp}-{n}", base(&path));
            }
            let b = join(&dir, &name);
            fs.insert(b.clone(), Item { mode: 0o600, owner: user.clone(), mtime: now(), ..cur.clone() });
            let prefix = format!("{dir}/{}.", base(&path));
            let mut old: Vec<String> = fs.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
            old.sort();
            for k in old.iter().take(old.len().saturating_sub(5)) {
                fs.remove(k);
            }
            backup = Some(b);
        }
        let mode = current.as_ref().map(|i| i.mode).unwrap_or(0o644);
        let item = Item { mode, owner, ..Item::text(content, mode, &user, now()) };
        let (mtime, size) = (item.mtime, item.size);
        fs.insert(path.clone(), item);
        Ok(Saved { path, sha256: sha_hex(content.as_bytes()), mtime, size, backup })
    }

    pub fn backups(&mut self, node: &str, path: &str) -> Vec<Backup> {
        let path = self.resolved(node, path, true);
        let dir = format!("{}/.sparkbox/backups/{}", self.home(), &sha_hex(path.as_bytes())[..16]);
        let prefix = format!("{dir}/{}.", base(&path));
        let mut found: Vec<Backup> = self
            .fs(node)
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, i)| Backup { path: k.clone(), mtime: i.mtime, size: i.size })
            .collect();
        found.sort_by(|a, b| b.path.cmp(&a.path));
        found
    }

    /// Appends to (or creates) a text file, for the demo shell's `>` and `>>`.
    pub fn write_text(&mut self, node: &str, path: &str, text: &str, append: bool) -> Result<(), String> {
        let path = self.resolved(node, path, true);
        let user = self.user.clone();
        let fs = self.fs(node);
        if !fs.get(parent(&path)).is_some_and(|i| i.dir) {
            return Err(format!("{path}: No such file or directory"));
        }
        let cur = fs.get(&path).cloned();
        if cur.as_ref().is_some_and(|i| i.dir) {
            return Err(format!("{path}: Is a directory"));
        }
        let owner = cur.as_ref().map(|i| i.owner.clone()).unwrap_or_else(|| user.clone());
        let open_dir = fs.get(parent(&path)).is_some_and(|d| d.owner == user || d.mode & 0o002 != 0);
        if owner != user || (cur.is_none() && !open_dir) {
            return Err(format!("{path}: Permission denied"));
        }
        let mut content = if append { cur.as_ref().and_then(|i| i.content.clone()).unwrap_or_default() } else { String::new() };
        content.push_str(text);
        let mode = cur.map(|i| i.mode).unwrap_or(0o644);
        fs.insert(path, Item::text(content, mode, &owner, now()));
        Ok(())
    }

    pub fn mkdir(&mut self, node: &str, path: &str) -> Result<(), String> {
        let path = self.resolved(node, path, false);
        let user = self.user.clone();
        let fs = self.fs(node);
        if fs.contains_key(&path) {
            return Err(format!("{path} already exists"));
        }
        match fs.get(parent(&path)) {
            Some(p) if p.dir && (p.owner == user || p.mode & 0o002 != 0) => {}
            Some(p) if p.dir => return Err(format!("can't create {path}: Permission denied")),
            _ => return Err(format!("{}: No such file or directory", parent(&path))),
        }
        fs.insert(path, Item::dir(0o755, &user, now()));
        Ok(())
    }

    fn subtree(fs: &BTreeMap<String, Item>, path: &str) -> Vec<String> {
        let prefix = format!("{path}/");
        fs.keys().filter(|k| k.as_str() == path || k.starts_with(&prefix)).cloned().collect()
    }

    fn check_writable(&mut self, node: &str, path: &str) -> Result<(), String> {
        let user = self.user.clone();
        let fs = self.fs(node);
        let dir = fs.get(parent(path));
        let own_dir = dir.is_some_and(|d| d.owner == user);
        // A sticky world-writable folder (/tmp): your own files only.
        let sticky_own = dir.is_some_and(|d| d.mode & 0o1002 == 0o1002) && fs.get(path).is_some_and(|i| i.owner == user);
        if !own_dir && !sticky_own {
            return Err(format!("{node} won't let {user} change {}: Permission denied", parent(path)));
        }
        Ok(())
    }

    fn move_tree(fs: &mut BTreeMap<String, Item>, from: &str, to: &str) {
        for k in Self::subtree(fs, from) {
            let item = fs.remove(&k).unwrap();
            fs.insert(format!("{to}{}", &k[from.len()..]), item);
        }
    }

    pub fn rename(&mut self, node: &str, from: &str, to: &str) -> Result<(), String> {
        let (from, to) = (self.resolved(node, from, false), self.resolved(node, to, false));
        self.check_writable(node, &from)?;
        let fs = self.fs(node);
        if !fs.contains_key(&from) {
            return Err(format!("{from}: No such file or directory"));
        }
        if fs.contains_key(&to) {
            return Err(format!("{to} already exists"));
        }
        Self::move_tree(fs, &from, &to);
        Ok(())
    }

    /// Moves an item into ~/.local/share/Trash/files with a .trashinfo, like a node does.
    pub fn trash(&mut self, node: &str, path: &str) -> Result<Trashed, String> {
        let path = self.resolved(node, path, false);
        let home = self.home();
        if super::paths::is_protected(&path, &home) {
            return Err(format!("SparkBox won't delete {path}."));
        }
        self.check_writable(node, &path)?;
        let user = self.user.clone();
        let fs = self.fs(node);
        if !fs.contains_key(&path) {
            return Err(format!("{path}: No such file or directory"));
        }
        let trash = format!("{home}/.local/share/Trash");
        for d in [trash.clone(), format!("{trash}/files"), format!("{trash}/info")] {
            fs.entry(d).or_insert_with(|| Item::dir(0o700, &user, now()));
        }
        let name = base(&path).to_string();
        let mut slot = name.clone();
        let mut n = 1;
        while fs.contains_key(&format!("{trash}/files/{slot}")) {
            n += 1;
            slot = format!("{name} {n}");
        }
        let dest = format!("{trash}/files/{slot}");
        Self::move_tree(fs, &path, &dest);
        let info = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", path.replace(' ', "%20"), chrono_like(now()));
        fs.insert(format!("{trash}/info/{slot}.trashinfo"), Item::text(info, 0o600, &user, now()));
        Ok(Trashed { path, trashed: Some(dest) })
    }

    pub fn untrash(&mut self, node: &str, trashed: &str) -> Result<String, String> {
        let trashed = self.normalize(trashed);
        let fs = self.fs(node);
        let slot = base(&trashed).to_string();
        let info_path = format!("{}/info/{slot}.trashinfo", parent(parent(&trashed)));
        let original = fs
            .get(&info_path)
            .and_then(|i| i.content.clone())
            .and_then(|c| c.lines().find_map(|l| l.strip_prefix("Path=").map(|p| p.replace("%20", " "))))
            .ok_or_else(|| format!("SparkBox can't tell where {trashed} came from."))?;
        if fs.contains_key(&original) {
            return Err(format!("{original} already exists, so the trashed copy was left in the Trash."));
        }
        if !fs.contains_key(&trashed) {
            return Err(format!("{trashed} is no longer in the Trash."));
        }
        Self::move_tree(fs, &trashed, &original);
        fs.remove(&info_path);
        Ok(original)
    }

    pub fn remove(&mut self, node: &str, path: &str) -> Result<(), String> {
        let path = self.resolved(node, path, false);
        if super::paths::is_protected(&path, &self.home()) {
            return Err(format!("SparkBox won't delete {path}."));
        }
        self.check_writable(node, &path)?;
        let fs = self.fs(node);
        let keys = Self::subtree(fs, &path);
        if keys.is_empty() {
            return Err(format!("{path}: No such file or directory"));
        }
        for k in keys {
            fs.remove(&k);
        }
        Ok(())
    }

    pub fn chmod(&mut self, node: &str, path: &str, mode: u32) -> Result<(), String> {
        let path = self.resolved(node, path, true);
        let user = self.user.clone();
        let item = self.fs(node).get_mut(&path).ok_or_else(|| format!("{path}: No such file or directory"))?;
        if item.owner != user {
            return Err(format!("chmod: changing permissions of '{path}': Operation not permitted"));
        }
        item.mode = mode;
        Ok(())
    }

    pub fn size(&mut self, node: &str, path: &str) -> Result<u64, String> {
        let path = self.resolved(node, path, true);
        let fs = self.fs(node);
        let keys = Self::subtree(fs, &path);
        if keys.is_empty() {
            return Err(format!("{path}: No such file or directory"));
        }
        Ok(keys.iter().filter_map(|k| fs.get(k)).filter(|i| !i.dir && i.link.is_none()).map(|i| i.size).sum())
    }

    /// An item's facts for a transfer's pre-flight check (None when it doesn't exist).
    pub fn side(&mut self, node: &str, dir: &str, name: &str) -> Option<Side> {
        let dir = self.resolved(node, dir, true);
        let p = join(&dir, name);
        let fs = self.fs(node);
        let item = fs.get(&p)?.clone();
        let keys = Self::subtree(fs, &p);
        let mut bytes = 0;
        let mut tar = 0;
        for k in &keys {
            let i = &fs[k];
            tar += 512;
            if !i.dir && i.link.is_none() {
                bytes += i.size;
                tar += i.size.div_ceil(512) * 512;
            }
        }
        Some(Side {
            kind: if item.link.is_some() { "link" } else if item.dir { "dir" } else { "file" }.into(),
            bytes,
            entries: keys.len() as u64,
            mtime: item.mtime,
            tar_bytes: tar,
        })
    }

    /// The items under `dir/name`, relative to `dir`, in order (folders before their contents).
    pub fn export(&mut self, node: &str, dir: &str, name: &str) -> Result<Vec<(String, Item)>, String> {
        let dir = self.resolved(node, dir, true);
        let p = join(&dir, name);
        let fs = self.fs(node);
        let keys = Self::subtree(fs, &p);
        if keys.is_empty() {
            return Err(format!("{p}: No such file or directory"));
        }
        Ok(keys.into_iter().map(|k| (k[dir.len()..].trim_start_matches('/').to_string(), fs[&k].clone())).collect())
    }

    /// Puts exported items into `dest` following the conflict policy, like sparkjob.py `place`.
    pub fn place(&mut self, node: &str, dest: &str, name: &str, items: Vec<(String, Item)>, policy: Option<Policy>) -> Result<Placed, String> {
        let dest = self.resolved(node, dest, true);
        let user = self.user.clone();
        let target = join(&dest, name);
        {
            let fs = self.fs(node);
            match fs.get(&dest) {
                Some(d) if d.dir && d.owner == user => {}
                Some(d) if d.dir => return Err(format!("{node} won't let {user} write into {dest}: Permission denied")),
                _ => return Err(format!("{dest}: No such file or directory")),
            }
        }
        let fs = self.fs(node);
        let exists = fs.contains_key(&target);
        let mut policy = policy;
        if policy.is_none() && exists {
            policy = Some(Policy::KeepBoth);
        }
        let stamp = |i: &Item| Item { owner: user.clone(), ..i.clone() };
        let put = |fs: &mut BTreeMap<String, Item>, at: &str, items: &[(String, Item)]| {
            for (rel, item) in items {
                let rest = &rel[name.len()..];
                fs.insert(format!("{at}{rest}"), stamp(item));
            }
        };
        let action = match (exists, policy) {
            (_, Some(Policy::Skip)) => return Ok(Placed { name: name.into(), action: "skipped".into(), as_name: name.into() }),
            (false, _) => {
                put(fs, &target, &items);
                "copied"
            }
            (true, Some(Policy::KeepBoth)) | (true, None) => {
                let dir = items.first().is_some_and(|(_, i)| i.dir);
                let (stem, ext) = match (dir, name.rfind('.')) {
                    (false, Some(i)) if i > 0 => (&name[..i], &name[i..]),
                    _ => (name, ""),
                };
                let mut n = 2;
                while fs.contains_key(&join(&dest, &format!("{stem} {n}{ext}"))) {
                    n += 1;
                }
                let other = format!("{stem} {n}{ext}");
                put(fs, &join(&dest, &other), &items);
                return Ok(Placed { name: name.into(), action: "keptBoth".into(), as_name: other });
            }
            (true, Some(Policy::Replace)) => {
                for k in Self::subtree(fs, &target) {
                    fs.remove(&k);
                }
                put(fs, &target, &items);
                "replaced"
            }
            (true, Some(p @ (Policy::Merge | Policy::ReplaceIfNewer))) => {
                for (rel, item) in &items {
                    let at = format!("{target}{}", &rel[name.len()..]);
                    match fs.get(&at) {
                        Some(cur) if cur.dir && item.dir => {}
                        Some(cur) if p == Policy::ReplaceIfNewer && cur.mtime >= item.mtime => {}
                        _ => {
                            fs.insert(at, stamp(item));
                        }
                    }
                }
                "merged"
            }
        };
        Ok(Placed { name: name.into(), action: action.into(), as_name: name.into() })
    }
}

/// "2026-10-28T09:14:03"-style local-free timestamp for trashinfo files and backup names.
fn chrono_like(secs: u64) -> String {
    let days = secs / 86_400;
    let (y, m, d) = civil(days as i64);
    let s = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Year, month, day, hour and minute (UTC) of a Unix time.
pub(crate) fn calendar(secs: u64) -> (i64, u32, u32, u64, u64) {
    let (y, m, d) = civil((secs / 86_400) as i64);
    (y, m, d, (secs % 86_400) / 3600, (secs / 60) % 60)
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_as_found_homes_and_netplan_are_the_real_ones() {
        let mut fs = DemoFs::as_found();
        let home = fs.home();
        // cell1's home as the tour's Files showed it.
        let names: Vec<String> = fs.list("cell1", "~").unwrap().entries.into_iter().filter(|e| !e.name.starts_with('.')).map(|e| e.name).collect();
        for want in ["dgx-spark-playbooks", "glm53", "models", "nccl", "nccl-mesh-plugin", "nccl-tests", "spark-vllm-docker", "glm53flash.sh", "hf_token.txt", "jumbo-frames.sh", "discover-sparks"] {
            assert!(names.iter().any(|n| n == want), "{want} in {names:?}");
        }
        assert!(!names.iter().any(|n| n == "notes.txt" || n == "projects"), "nothing the everyday world made up");
        // With the desktop's folders, the modes and the times of shot 34 (Oct 26, 3:30 PM for ~/glm53).
        let entries = fs.list("cell1", "~").unwrap().entries;
        let row = |name: &str| entries.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("{name}")).clone();
        for (name, mode) in [("Desktop", "drwxr-xr-x"), ("Documents", "drwxr-xr-x"), ("Downloads", "drwxr-xr-x"), ("Music", "drwxr-xr-x"), ("glm53", "drwxrwxr-x"), ("models", "drwxrwxr-x"), ("nccl-mesh-plugin", "drwxrwxr-x")] {
            assert_eq!(row(name).mode, mode, "{name}");
        }
        assert_eq!(row("glm53").mtime, crate::demo::world::found(1_793_043_000, now()));
        assert!(row("nccl-tests").mtime > row("nccl").mtime && row("models").mtime > row("glm53").mtime, "the order they were made in on Oct 26");
        // ~/glm53 on every node: the real inner.sh (line 22 is the bf16 KV cache), the secrets as 600 placeholders.
        for node in ["cell1", "cell2", "cell3", "gx10-0000"] {
            let inner = fs.read(node, "~/glm53/inner.sh").unwrap();
            assert_eq!(inner.content.lines().nth(21), Some("  --kv-cache-dtype bfloat16 \\"), "{node}");
            assert_eq!(inner.mode, "-rwxrwxr-x");
            for secret in ["api_key", "config.env"] {
                let f = fs.read(node, &format!("~/glm53/{secret}")).unwrap();
                assert_eq!(f.mode, "-rw-------", "{node} {secret}");
                assert!(!f.content.contains("sk-") && !f.content.contains("hf_"), "a placeholder: {}", f.content);
            }
            assert!(fs.exists(node, "~/models/GLM-5.3-Flash-NVFP4/config.json"));
            assert!(fs.read(node, "~/glm53/node-launch.sh").unwrap().content.contains("--shm-size 16g --gpus all"));
        }
        let token = fs.read("cell1", "~/hf_token.txt").unwrap();
        assert_eq!((token.mode.as_str(), token.content.as_str()), ("-rw-------", "hf_placeholder_not_a_real_token\n"));
        assert!(!fs.exists("cell2", "~/hf_token.txt"), "only cell1's home has the scripts");
        // The model folders the Library lists, at their real sizes; the Hugging Face cache's snapshots.
        for f in world::AS_FOUND_FOLDERS {
            for node in f.nodes {
                assert_eq!(fs.size(node, &f.path(&home)).unwrap(), f.bytes, "{} on {node}", f.repo);
            }
        }
        assert!(fs.list("cell1", "~/.cache/huggingface/hub").unwrap().entries.iter().any(|e| e.name == "models--Qwen--Qwen3-1.7B"));
        assert!(!fs.exists("cell2", "~/.cache/huggingface/hub/models--Qwen--Qwen3-1.7B"));
        // /etc/netplan: the three real files, root 600: reading one is refused, as it is for real.
        let netplan: Vec<(String, String)> = fs.list("cell1", "/etc/netplan").unwrap().entries.into_iter().map(|e| (e.name, e.mode)).collect();
        assert_eq!(netplan.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["00-installer-config.yaml", "40-cx7.yaml", &format!("90-NM-{NM_UUID}.yaml")]);
        assert!(netplan.iter().all(|(_, m)| m == "-rw-------"));
        let err = fs.read("cell1", "/etc/netplan/40-cx7.yaml").unwrap_err();
        assert!(err.contains("Permission denied"), "{err}");
        // What Read with sudo would show is there (the fabric addresses).
        let (_, item) = fs.export("cell3", "/etc/netplan", "40-cx7.yaml").unwrap().into_iter().next().unwrap();
        assert!(item.content.unwrap().contains("addresses: [192.168.100.3/24]"));
        // The free space is the real disk's: cell1 2.6 TiB of 3.7.
        let (free, total) = fs.space("cell1");
        assert_eq!(total, 4_030_802_149_376);
        assert_eq!(free, 4_030_802_149_376 - 1_212_625_289_216);
    }

    #[test]
    fn switching_worlds_starts_the_trees_over() {
        let mut fs = DemoFs::new();
        assert!(fs.exists("cell1", "~/notes.txt") && fs.exists("cell1", "/etc/netplan/40-cx7-fabric.yaml"));
        fs.write_text("cell1", "~/notes.txt", "mine\n", true).unwrap();
        fs.set_world(World::AsFound);
        assert!(!fs.exists("cell1", "~/notes.txt") && fs.exists("cell1", "~/glm53/inner.sh"));
        fs.set_world(World::Everyday);
        assert!(!fs.read("cell1", "~/notes.txt").unwrap().content.contains("mine"), "the everyday tree as it was seeded");
    }
}
