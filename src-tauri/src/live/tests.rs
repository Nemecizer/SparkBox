//! The live backend against fake Sparks (fake_node.rs): real SSH servers in the test that
//! run what they're sent with `sh -c`, record every command and stream collector frames.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use std::path::Path;

use super::*;
use crate::fake_node::{fake_node, real_shell, temp, Script};
use crate::inventory::{Inventory, NodeConfig};
use crate::vpn::Vpn;

// Each test process gets fresh keys. The cloned-node fixture shares one key;
// the distinct-node fixture uses another. No private key material is distributed.
pub(crate) static HOST_KEY: std::sync::LazyLock<String> = std::sync::LazyLock::new(ephemeral_host_key);
pub(crate) static HOST_KEY_GX10: std::sync::LazyLock<String> = std::sync::LazyLock::new(ephemeral_host_key);

fn ephemeral_host_key() -> String {
    use russh::keys::ssh_key::{private::Ed25519Keypair, LineEnding, PrivateKey};
    use zeroize::Zeroizing;
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(seed.as_mut()).expect("test host-key randomness");
    let pair = Ed25519Keypair::from_seed(&seed);
    PrivateKey::new(pair.into(), "sparkbox-ephemeral-test")
        .unwrap().to_openssh(LineEnding::LF).unwrap().to_string()
}

/// A frame shaped exactly like collector.py's output.
pub(crate) fn frame() -> String {
    json!({
        "v": 1,
        "cpu": { "totalPct": 12.5, "perCore": [100.0, 2.0], "load": [1.1, 0.9, 0.7], "tempC": 58.0 },
        "mem": { "total": 128_000_000_000u64, "used": 96_000_000_000u64, "available": 32_000_000_000u64,
                 "cached": 9_000_000_000u64, "gpuAlloc": 88_000_000_000u64, "swapTotal": 0, "swapUsed": 0 },
        "gpu": { "utilPct": 91.0, "tempC": 71.0, "powerW": 84.0, "smClockMhz": 2418,
                 "procs": [{ "pid": 4242, "name": "python3", "mem": 88_000_000_000u64, "container": "vllm-head" }] },
        "disks": [{ "mount": "/", "device": "/dev/nvme0n1p2", "fsType": "ext4", "total": 3_900_000_000_000u64,
                    "used": 1_200_000_000_000u64, "readBps": 1024.5, "writeBps": 0.0 }],
        "nets": [{ "iface": "enp1s0f0np0", "role": "fabric", "up": true, "speedMbps": 200000, "rxBps": 1.8e10,
                   "txBps": 1.7e10, "ip": "10.100.0.11" },
                 { "iface": "enP7s7", "role": "lan", "up": true, "speedMbps": 10000, "rxBps": 1000.0, "txBps": 500.0, "ip": null }],
        "system": { "hostname": "cell1-example", "model": "NVIDIA DGX Spark", "os": "DGX OS 7.2 (Ubuntu 24.04.3 LTS)",
                    "kernel": "6.11.0-1016-nvidia", "driver": "580.95.05", "cuda": "13.0", "uptimeS": 3600 },
        "containers": [{ "id": "0123456789ab", "name": "vllm-head", "image": "nvcr.io/nvidia/vllm:25.09-py3",
                         "state": "running", "startedAt": 1_790_000_000u64, "finishedAt": 0, "exitCode": 0,
                         "cpuPct": 104.2, "mem": 9_000_000_000u64, "gpuMem": 88_000_000_000u64,
                         "ports": ["0.0.0.0:8000->8000/tcp"], "project": "llama-70b", "command": "vllm serve" }],
        "errors": ["Docker: example warning"],
    })
    .to_string()
}

/// A GB10's docker, which the fake Mac doesn't have.
fn docker() -> Script {
    Arc::new(|cmd, _| {
        if cmd.starts_with("docker inspect") {
            return Some((r#"[{"Id": "0123456789abcdef", "Name": "/vllm-head", "Config": {"Image": "nvcr.io/nvidia/vllm:26.05-py3"}}]"#.into(), 0));
        }
        cmd.starts_with("docker ").then(|| (String::new(), 0))
    })
}

fn node(port: u16) -> NodeConfig {
    NodeConfig {
        id: "cell1".into(),
        name: "cell1".into(),
        host: "cell1-example.local".into(),
        ip: Some("127.0.0.1".into()),
        model: "MSI EdgeXpert".into(),
        port,
        user: "sparkuser".into(),
        fabric: None,
    }
}

fn live(name: &str, password: Option<&'static str>) -> (Arc<Live>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("sparkbox-live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let vpn = Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let known = dir.join("known_hosts.json");
    let live = Live::new(vpn, known.clone(), move || Ok(password.map(String::from)));
    live.sshlog.set_path(dir.join("ssh.log"));
    (live, known)
}

/// What the SSH log says so far.
fn sshlog_of(live: &Live) -> String {
    live.sshlog.file().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()
}

type Conns = (Option<Arc<ssh::Session>>, Option<Arc<ssh::Session>>);

/// A node's control and bulk connections. Holding them keeps their addresses from being reused,
/// so `same` against them later means the very same connections.
fn conns(live: &Live, id: &str) -> Conns {
    let nodes = live.nodes.lock().unwrap();
    let s = nodes.get(id).unwrap();
    (s.session.clone(), s.bulk.clone())
}

fn same(a: &Conns, b: &Conns) -> bool {
    let eq = |x: &Option<Arc<ssh::Session>>, y: &Option<Arc<ssh::Session>>| match (x, y) {
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        (None, None) => true,
        _ => false,
    };
    eq(&a.0, &b.0) && eq(&a.1, &b.1)
}

/// What the Connect wizard's "Trust" does once the user compared the fingerprint.
fn trusted(known: &Path, port: u16) {
    crate::fake_node::trust_ports(known, &[port]);
}

async fn wait_for(live: &Live, inv: &Inventory, want: LinkStatus) -> NodeSnapshot {
    for _ in 0..100 {
        let snap = live.snapshot(inv);
        if snap.nodes[0].status == want {
            return snap.nodes[0].clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never reached {want:?}: {:?}", live.snapshot(inv).nodes[0].error);
}

fn inventory(port: u16) -> Inventory {
    Inventory { nodes: vec![node(port)], switches: vec![] }
}

#[tokio::test]
async fn installs_the_collector_and_streams_live_numbers() {
    let home = temp("live-stream-home");
    let (spark, port) = fake_node(&home, real_shell()).await;
    let (live, known) = live("stream", Some("pw"));
    trusted(&known, port);
    live.start(&[node(port)]);
    let n = wait_for(&live, &inventory(port), LinkStatus::Online).await;

    assert_eq!(n.gpu.util_pct, 91.0);
    assert_eq!(n.gpu.procs[0].container.as_deref(), Some("vllm-head"));
    assert_eq!(n.mem.gpu_alloc, 88_000_000_000);
    assert_eq!(n.nets[0].role, NetRole::Fabric);
    assert_eq!(n.containers[0].name, "vllm-head");
    assert_eq!(n.system.model, "MSI EdgeXpert (NVIDIA DGX Spark)");
    assert_eq!(n.error.as_deref(), Some("Docker: example warning"), "collector warnings surface");
    assert!(n.latency_ms.is_some());

    assert_eq!(std::fs::read_to_string(home.join(".sparkbox/collector.py")).unwrap(), COLLECTOR, "the collector was copied verbatim");
    assert_eq!(std::fs::read_to_string(home.join(".sparkbox/roce.py")).unwrap(), super::ROCE, "and roce.py next to it, which it imports");
    let log = spark.log.lock().unwrap().clone();
    assert_eq!(log[0], "true");
    assert!(log[1].starts_with("sha256sum"));
    assert!(log[2].contains("cat > ~/.sparkbox/collector.py.new"));
    assert!(log[3].starts_with("sha256sum ~/.sparkbox/roce.py"));
    assert!(log[4].contains("cat > ~/.sparkbox/roce.py.new"));
    assert_eq!(log[5], "exec python3 -u ~/.sparkbox/collector.py");
    let raw = spark.raw.lock().unwrap().clone();
    assert_eq!(raw[1], "env LC_ALL=C sh -c 'sha256sum ~/.sparkbox/collector.py 2>/dev/null'", "exactly as sent");
    assert!(std::fs::read_to_string(known).unwrap().contains(&format!("127.0.0.1:{port}")), "host key pinned");
    assert_eq!(live.logins()["cell1"].method, ssh::AuthMethod::Password);
    live.stop();
}

#[test]
fn kernel_events_and_nccl_settings_come_through_from_the_frame() {
    let mut v: serde_json::Value = serde_json::from_str(&frame()).unwrap();
    v["kernelEvents"] = json!([{ "tsMs": 1_790_000_000_000u64, "kind": "xid", "code": 79, "severity": "crit", "process": null,
                                 "text": "NVRM: Xid (PCI:000f:01:00): 79, pid='<unknown>', GPU has fallen off the bus." }]);
    v["containers"][0]["envFlags"] = json!({ "NCCL_IB_HCA": "rocep1s0f1", "NCCL_SOCKET_IFNAME": "enp1s0f1np1" });
    v["bootId"] = json!("5f1c9a2e-7c0b-4d8e-9a51-1b2c3d4e5f60");
    let mut st = NodeState::new(0);
    st.status = LinkStatus::Online;
    st.frame = Some(serde_json::from_value(v).unwrap());
    let n = node_snapshot(&node(22), Some(&st));
    assert_eq!(n.kernel_events.len(), 1, "the health engine's kernel rules read these");
    assert_eq!((n.kernel_events[0].kind.as_str(), n.kernel_events[0].code), ("xid", Some(79)));
    assert_eq!(n.containers[0].env_flags.get("NCCL_IB_HCA").map(String::as_str), Some("rocep1s0f1"), "the NCCL transport check reads these");
    assert_eq!(n.boot_id.as_deref(), Some("5f1c9a2e-7c0b-4d8e-9a51-1b2c3d4e5f60"), "the Xid latch knows a reboot by it");

    // An older collector sends none of them.
    let mut old = NodeState::new(0);
    old.frame = Some(serde_json::from_str(&frame()).unwrap());
    let n = node_snapshot(&node(22), Some(&old));
    assert!(n.kernel_events.is_empty() && n.containers[0].env_flags.is_empty() && n.boot_id.is_none());
}

#[test]
fn a_gpu_that_doesnt_answer_keeps_the_frame_and_leaves_a_gap_in_history() {
    // What collector.py sends while nvidia-smi hangs (NO_GPU, NO_GPU_EXTRA and gpu.error).
    let mut v: serde_json::Value = serde_json::from_str(&frame()).unwrap();
    v["gpu"] = json!({ "utilPct": null, "tempC": null, "powerW": null, "smClockMhz": null, "procs": [],
                       "clockMaxMhz": null, "powerLimitW": null, "pstate": null, "throttle": [],
                       "error": "nvidia-smi didn't answer in 10 s" });
    v["errors"] = json!(["nvidia-smi didn't answer in 10 s"]);
    let mut st = NodeState::new(0);
    st.status = LinkStatus::Online;
    st.frame = Some(serde_json::from_value(v).expect("null GPU numbers don't drop the frame"));
    let n = node_snapshot(&node(22), Some(&st));
    assert_eq!(n.cpu.total_pct, 12.5, "the rest of the frame is there");
    assert_eq!(crate::health::rules::gpu_error(&n).as_deref(), Some("nvidia-smi didn't answer in 10 s"), "the GPU-unreadable rule reads this");
    let row = crate::history::sample(&n);
    let at = |m: &str| row[crate::history::METRICS.iter().position(|x| *x == m).unwrap()];
    for m in ["gpu", "power", "gpuTemp", "smClock"] {
        assert!(at(m).is_nan(), "{m} is a gap, not 0");
    }
    assert_eq!(at("cpu"), 12.5);

    // Once it answers again the numbers are recorded.
    st.frame = Some(serde_json::from_str(&frame()).unwrap());
    assert_eq!(crate::history::sample(&node_snapshot(&node(22), Some(&st)))[1], 91.0);
}

#[tokio::test]
async fn container_actions_run_docker_and_reject_odd_input() {
    let (spark, port) = fake_node(&temp("live-docker-home"), docker()).await;
    let (live, known) = live("docker", Some("pw"));
    trusted(&known, port);
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;

    live.container_action("cell1", "0123456789ab", "restart").await.unwrap();
    assert!(spark.raw.lock().unwrap().contains(&"env LC_ALL=C sh -c 'docker restart 0123456789ab'".to_string()), "in the C locale");
    assert!(live.audit.entries().iter().any(|e| e.command == "docker restart 0123456789ab"), "and in the activity log");
    live.container_action("cell1", "0123456789ab", "remove").await.unwrap();
    assert!(spark.log.lock().unwrap().contains(&"docker rm 0123456789ab".to_string()));
    assert_eq!(live.container_action("cell1", "0123456789ab; rm -rf /", "stop").await.unwrap_err(), "bad container id");
    assert!(live.container_action("cell1", "0123456789ab", "explode").await.is_err());
    assert!(live.container_action("cell9", "0123456789ab", "stop").await.unwrap_err().contains("isn't connected"));

    let info = live.container_inspect("cell1", "0123456789ab").await.unwrap();
    assert_eq!(info["Name"], "/vllm-head");
    assert!(spark.log.lock().unwrap().contains(&"docker inspect --type container 0123456789ab".to_string()));
    assert_eq!(live.container_inspect("cell1", "x; reboot").await.unwrap_err(), "bad container id");
    live.stop();
}

#[tokio::test]
async fn a_wrong_or_missing_password_is_explained() {
    let (_spark, port) = fake_node(&temp("live-badpw-home"), real_shell()).await;
    let (wrong, known) = live("badpw", Some("nope"));
    trusted(&known, port);
    wrong.start(&[node(port)]);
    let n = wait_for(&wrong, &inventory(port), LinkStatus::Error).await;
    assert!(n.error.unwrap().contains("password for sparkuser@127.0.0.1 was rejected"));
    wrong.stop();

    let (missing, known) = live("nopw", None);
    trusted(&known, port);
    missing.start(&[node(port)]);
    let n = wait_for(&missing, &inventory(port), LinkStatus::Error).await;
    let e = n.error.unwrap();
    assert!(e.contains("Save the cluster password"), "{e}");
    missing.stop();
}

#[tokio::test]
async fn a_changed_host_key_is_refused() {
    let (_spark, port) = fake_node(&temp("live-hostkey-home"), real_shell()).await;
    let (live, known) = live("hostkey", Some("pw"));
    std::fs::create_dir_all(known.parent().unwrap()).unwrap();
    std::fs::write(&known, json!({ format!("127.0.0.1:{port}"): "SHA256:not-the-real-key" }).to_string()).unwrap();
    live.start(&[node(port)]);
    let n = wait_for(&live, &inventory(port), LinkStatus::Error).await;
    assert!(n.error.unwrap().contains("host key for 127.0.0.1"));
    live.stop();
}

/// Look-only, switched by the test (the app's comes from the Admin gate).
pub(crate) struct TestPolicy(pub std::sync::atomic::AtomicBool);

impl TestPolicy {
    pub(crate) fn look_only() -> Arc<TestPolicy> {
        Arc::new(TestPolicy(std::sync::atomic::AtomicBool::new(true)))
    }
}

impl exit::WritePolicy for TestPolicy {
    fn look_only(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn admin_unlocked(&self) -> bool {
        true
    }
}

/// Enforces the write exit on `live` (dev build rules under `base`) with a signed fence open for
/// `nodes` and `classes`: as the release build opens one for the test build.
pub(crate) fn open_fence(live: &Live, base: &Path, nodes: &[&str], classes: &[fence_class::FenceClass]) {
    let rules = exit::Enforce {
        instance: "dev".into(),
        fence_dir: base.join("SparkBox Fence"),
        config_dir: Some(base.join("config-dev")),
        noauth: false,
        tag: "p1-r2-test".into(),
        mac: Some("Studio".into()),
    };
    let config = rules.config_dir.clone().unwrap();
    std::fs::create_dir_all(&rules.fence_dir).unwrap();
    let key = fence::new_key().unwrap();
    let (key_id, public_key) = fence::public_of(&key).unwrap();
    let file = fence::KeyFile { key_id, public_key, kind: "keychain".into() };
    fence::save_trust(&config, &file).unwrap();
    let spec = fence::Spec { nodes: nodes.iter().map(|n| n.to_string()).collect(), classes: classes.to_vec(), minutes: 180, instance: "dev".into(), ..Default::default() };
    let record = fence::record_for(&spec, &key, now_ms(), "Studio").unwrap();
    fence::write(&rules.fence_dir, &config, &file, &record).unwrap();
    live.enforce_writes(rules);
}

/// The doctor's whole script, and each section of it, only reads, as the write exit judges it.
#[test]
fn the_doctor_only_reads() {
    use crate::ops::classify::{classify, escalates, Kind};
    use fence_class::{class_of, Ctx, Shape};
    let script = doctor_script();
    for cmd in std::iter::once(script.as_str()).chain(DOCTOR_SECTIONS.iter().map(|(_, c)| *c)) {
        assert_eq!(class_of(exit::ExecKind::Run, cmd, &Ctx::default()), Shape::Read, "`{cmd}`");
        assert!(matches!(classify(cmd), Kind::Read | Kind::Probe) && !escalates(cmd), "`{cmd}`");
    }
    // P1.13's additions are all there, and no sudo in the script itself.
    for name in ["sshd-config", "machine-id", "image-store", "grub", "ufw", "apt", "dgx-release"] {
        assert!(script.contains(&format!("echo '----- {name}'")), "{name}");
    }
    assert!(!script.contains("sudo") && !script.contains("python3 -u"), "the collector's side comes from its frames, not a second run");
    // The sudo-only line isn't a read: it runs tagged as a sudo check, under a fence.
    assert_ne!(class_of(exit::ExecKind::Sudo, DOCTOR_SSHD_T, &Ctx::default()), Shape::Read);
}

#[tokio::test]
async fn doctor_reports_every_node() {
    // A node that isn't connected gets a connection of its own for the doctor.
    let (spark, port) = fake_node(&temp("live-doctor-home"), docker()).await;
    let (live, known) = live("doctor", Some("pw"));
    trusted(&known, port);
    let report = live.doctor(&[node(port)], "test header").await;
    assert!(report.contains("test header"));
    assert!(report.contains("cell1 (127.0.0.1)"));
    assert!(report.contains("\n----- host\n"), "{report}");
    assert!(report.contains("\n----- collector (the last frame SparkBox received)\nNo frame: Connecting"), "{report}");
    // The script is the command itself, as the write exit saw it (not a shell fed on stdin).
    let ran = spark.ran("echo '----- host'");
    assert_eq!(ran.len(), 1, "{:?}", spark.log.lock().unwrap());
    assert_eq!(ran[0], doctor_script());
    assert!(spark.ran("bash -s").is_empty());
    assert!(sshlog_of(&live).contains(" cell1 session_closed conn=doctor "), "{}", sshlog_of(&live));
    let row = live.audit.entries().into_iter().find(|e| e.title == "Run doctor").expect("the doctor's activity row");
    assert_eq!((row.node.as_str(), row.class.as_str(), row.changes, row.state.as_str()), ("cell1", "read", false, "ok"));
}

/// P1.13 on cell1 as the Live tour found it: the doctor's new sections, the sudo-only line
/// skipped as needing sudo, and the collector's side from its frames.
#[tokio::test]
async fn the_doctor_report_has_the_new_sections_for_a_spark() {
    let (_spark, port) = crate::fake_node::fake_spark(&temp("live-doctor-spark"), "cell1").await;
    let (live, inv) = crate::fake_node::connect_sparks(&temp("live-doctor-spark-app"), &[("cell1", port)]).await;
    let report = live.doctor(&inv.nodes, "test header").await;
    let section = |name: &str| -> String {
        let start = report.find(&format!("\n----- {name}\n")).unwrap_or_else(|| panic!("no {name} section: {report}"));
        let body = &report[start + name.len() + 8..];
        body[..body.find("\n----- ").unwrap_or(body.len())].to_string()
    };
    assert_eq!(section("sshd-config").trim(), "/etc/ssh/sshd_config:#MaxSessions 10");
    assert_eq!(section("machine-id").trim().len(), 32);
    assert!(section("image-store").starts_with("overlay2 [[\"Backing Filesystem\",\"extfs\"]"), "{}", section("image-store"));
    assert_eq!(section("grub").trim(), "GRUB_DEFAULT=0");
    assert_eq!(section("ufw").trim(), "ENABLED=no");
    assert!(section("apt").contains("APT::Periodic::Unattended-Upgrade \"0\";"), "{}", section("apt"));
    assert!(section("dgx-release").contains("DGX_SWBUILD_VERSION=\"7.2.3\""), "{}", section("dgx-release"));
    assert_eq!(section("sshd-effective (sudo sshd -T)").trim(), "needs sudo: run the doctor with Admin unlocked to read it on cell1");
    let collector = section("collector (the last frame SparkBox received)");
    assert!(collector.starts_with("cell1-example · Online · frame "), "{collector}");
    assert!(collector.contains(" processes, 1 containers, ") && collector.contains("\"cpu\":"), "{collector}");
    // The sections the doctor always had still read as before.
    assert!(section("python").contains("Python 3.12.3"), "{}", section("python"));
    assert!(section("docker").contains("glm53"), "{}", section("docker"));
    live.stop();
}

/// The doctor leaves through the write exit: under look-only its reads run and its sudo line
/// is refused there; a doctor whose script wrote would be refused whole, before it ran.
#[tokio::test]
async fn look_only_lets_the_doctor_read_and_holds_its_sudo_line() {
    let home = temp("live-doctor-look");
    let (spark, port) = fake_node(&home, real_shell()).await;
    let (live, inv) = crate::fake_node::connect(&temp("live-doctor-look-app"), &[("cell1", port)]).await;
    live.set_write_policy(TestPolicy::look_only());

    let report = live.doctor_with(&inv.nodes, "h", true).await;
    assert!(report.contains("\n----- host\n") && report.contains("\n----- grub\n"), "{report}");
    assert!(report.contains("\n----- sshd-effective (sudo sshd -T)\nneeds sudo: SparkBox is in look-only mode"), "{report}");
    assert!(spark.ran("sshd -T").is_empty(), "the sudo line never reached the node");
    let refused: Vec<audit::Entry> = live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(refused[0].command.contains("sshd -T"), "{refused:?}");

    // The same path with a script that writes: refused by the exit, and nothing ran.
    let wrote = live.doctor_running(&inv.nodes, "h", false, "echo; echo '----- host'; hostname; touch ~/doctor-wrote", DOCTOR_LIMIT).await;
    assert!(wrote.contains("Diagnostics refused: SparkBox is in look-only mode"), "{wrote}");
    assert!(spark.ran("doctor-wrote").is_empty() && !home.join("doctor-wrote").exists());
    live.stop();
}

/// In an owner's session the doctor's sudo line runs, as a sudo check the fence allows.
#[tokio::test]
async fn under_a_fence_that_allows_sudo_checks_the_doctor_reads_sshds_settings() {
    let (spark, port) = fake_node(&temp("live-doctor-fence"), real_shell()).await;
    let base = temp("live-doctor-fence-app");
    let (live, inv) = crate::fake_node::connect(&base, &[("cell1", port)]).await;
    open_fence(&live, &base, &["cell1"], &[fence_class::FenceClass::SudoProbe]);
    let report = live.doctor_with(&inv.nodes, "h", true).await;
    assert!(report.contains("\n----- sshd-effective (sudo sshd -T)\nmaxsessions 10\nmaxstartups 10:30:100\nallowtcpforwarding yes\ndisableforwarding no\n"), "{report}");
    assert_eq!(spark.ran("sudo -k -S -p '' sshd -T").len(), 1);
    let row = live.audit.entries().into_iter().find(|e| e.kind == "sudo" && e.command.starts_with("sshd -T")).unwrap();
    assert_eq!((row.fence.as_deref(), row.state.as_str()), (Some("sudo-probe"), "ok"));

    // A fence for other classes doesn't stretch to it.
    let base2 = temp("live-doctor-fence2-app");
    let (spark2, port2) = fake_node(&temp("live-doctor-fence2"), real_shell()).await;
    let (live2, inv2) = crate::fake_node::connect(&base2, &[("cell1", port2)]).await;
    open_fence(&live2, &base2, &["cell1"], &[fence_class::FenceClass::Service]);
    let report = live2.doctor_with(&inv2.nodes, "h", true).await;
    assert!(report.contains("\n----- sshd-effective (sudo sshd -T)\nneeds sudo: The fence doesn't allow sudo checks."), "{report}");
    assert!(spark2.ran("sshd -T").is_empty());
    live.stop();
    live2.stop();
}

/// The doctor's report says so where a node's run stopped: what ran is kept, then one line on
/// why the rest is missing, then the node's sshd and collector sections as usual.
#[test]
fn a_doctor_run_cut_short_is_named_in_the_report() {
    let out = |code: Option<u32>, signal: Option<&str>| ssh::Output { code, signal: signal.map(String::from), ..Default::default() };
    let limit = Duration::from_secs(90);
    let missing = ": the sections after this point are missing.";
    assert_eq!(doctor_cut_short(&out(Some(0), None), limit), None);
    assert_eq!(doctor_cut_short(&ssh::Output { timed_out: true, ..Default::default() }, limit).unwrap(), format!("Diagnostics stopped after 90 s{missing}"));
    assert_eq!(doctor_cut_short(&out(None, Some("KILL")), limit).unwrap(), format!("Diagnostics cut short: the doctor's command was killed (SIGKILL){missing}"));
    assert_eq!(doctor_cut_short(&out(None, None), limit).unwrap(), format!("Diagnostics cut short: the connection ended with no exit status{missing}"));
    assert_eq!(doctor_cut_short(&out(Some(2), None), limit).unwrap(), format!("The doctor's command ended with exit 2{missing}"));
    assert_eq!(doctor_cut_short(&ssh::Output { code: Some(0), truncated: true, ..Default::default() }, limit).unwrap(), format!("The doctor's output went past 1 MiB{missing}"));
}

/// The verifier's drop: the node's connection goes while nvidia-smi is still running.
#[tokio::test]
async fn a_node_lost_during_the_doctor_is_named_in_the_report() {
    let (node, port) = fake_node(&temp("live-doctor-drop"), real_shell()).await;
    let (live, inv) = crate::fake_node::connect(&temp("live-doctor-drop-app"), &[("cell1", port)]).await;
    // A whole run first: no such line.
    let whole = live.doctor(&inv.nodes, "h").await;
    assert!(whole.contains("\n----- dgx-release\n") && !whole.contains("sections after this point are missing"), "{whole}");

    node.bin("nvidia-smi", "#!/bin/sh\nsleep 6\necho slow-smi\n");
    let l2 = live.clone();
    let nodes = inv.nodes.clone();
    let run = tokio::spawn(async move { l2.doctor(&nodes, "h").await });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    node.drop_connections().await;
    let report = tokio::time::timeout(Duration::from_secs(30), run).await.expect("the doctor returned").unwrap();
    assert!(
        report.contains("\n----- nvidia-smi\n\nDiagnostics cut short: the connection ended with no exit status: the sections after this point are missing.\n\n----- sshd-effective (sudo sshd -T)\n"),
        "{report}"
    );
    assert!(!report.contains("----- gpu-query") && report.contains("\n----- collector (the last frame SparkBox received)\n"), "{report}");
    let row = live.audit.entries().into_iter().find(|e| e.title == "Run doctor").unwrap();
    assert_eq!(row.state, "failed");
    live.stop();
}

/// The time limit, a death by signal and an exit other than 0 each keep what ran and say why
/// the rest is missing.
#[tokio::test]
async fn a_doctor_stopped_killed_or_failing_part_way_says_so() {
    let (_node, port) = fake_node(&temp("live-doctor-short"), real_shell()).await;
    let (live, inv) = crate::fake_node::connect(&temp("live-doctor-short-app"), &[("cell1", port)]).await;
    let script = |end: &str| format!("echo; echo '----- host'; hostname; echo; echo '----- next'; {end}; echo; echo '----- after'; true");
    let cases = [
        ("sleep 30", Duration::from_secs(2), "Diagnostics stopped after 2 s"),
        ("kill -KILL $$", DOCTOR_LIMIT, "Diagnostics cut short: the doctor's command was killed (SIGKILL)"),
        ("exit 3", DOCTOR_LIMIT, "The doctor's command ended with exit 3"),
    ];
    for (end, limit, why) in cases {
        let report = live.doctor_running(&inv.nodes, "h", false, &script(end), limit).await;
        assert!(report.contains("\n----- host\n") && report.contains("\n----- next\n"), "{end}: {report}");
        assert!(report.contains(&format!("\n----- next\n\n{why}: the sections after this point are missing.\n\n----- sshd-effective")), "{end}: {report}");
        assert!(!report.contains("----- after"), "{end}: {report}");
    }
    live.stop();
}

#[test]
fn nodes_without_data_yet_still_show_up() {
    let (live, _) = live("empty", Some("pw"));
    let snap = live.snapshot(&inventory(22));
    assert_eq!(snap.nodes[0].status, LinkStatus::Connecting);
    assert!(!snap.demo);
}

#[tokio::test]
async fn terminals_get_a_pty_echo_keystrokes_resize_and_exit() {
    use crate::term::{Emit, Target, TermEvent, Terminals};
    use base64::Engine;

    let (spark, port) = fake_node(&temp("live-pty-home"), real_shell()).await;
    let (live, known) = live("pty", Some("pw"));
    trusted(&known, port);
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;
    let terminals = Terminals::new(live.clone());

    let screen: Arc<Mutex<(String, Option<Option<u32>>)>> = Arc::default();
    let s = screen.clone();
    let emit: Emit = Box::new(move |e| match e {
        TermEvent::Data { data } => {
            let b = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
            s.lock().unwrap().0.push_str(&String::from_utf8_lossy(&b));
        }
        TermEvent::Exit { code, .. } => s.lock().unwrap().1 = Some(code),
        _ => {}
    });
    let id = terminals.open(false, "cell1", Target::Shell, 100, 30, emit).await.unwrap();
    terminals.authorize_shell(&id).unwrap();
    async fn wait(screen: &Arc<Mutex<(String, Option<Option<u32>>)>>, f: impl Fn(&(String, Option<Option<u32>>)) -> bool) -> bool {
        for _ in 0..250 {
            if f(&screen.lock().unwrap()) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }
    assert!(wait(&screen, |s| s.0.contains("$ ")).await, "prompt: {:?}", screen.lock().unwrap());

    // A real terminal on the node: it echoes, turns Return into a newline and knows its size.
    terminals.input(&id, "echo héllo\r".as_bytes().to_vec()).unwrap();
    assert!(wait(&screen, |s| s.0.contains("echo héllo\r\n") && s.0.contains("\r\nhéllo\r\n")).await, "echo: {:?}", screen.lock().unwrap());
    terminals.resize(&id, 120, 40).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    terminals.input(&id, b"stty size\r".to_vec()).unwrap();
    assert!(wait(&screen, |s| s.0.contains("40 120")).await, "size: {:?}", screen.lock().unwrap());
    terminals.input(&id, b"exit\r".to_vec()).unwrap();
    assert!(wait(&screen, |s| s.1 == Some(Some(0))).await, "exit: {:?}", screen.lock().unwrap());

    let log = spark.log.lock().unwrap().clone();
    assert!(log.contains(&"pty xterm-256color 100x30".to_string()), "{log:?}");
    assert!(log.contains(&"shell".to_string()));
    assert!(log.contains(&"resize 120x40".to_string()));

    // A shell inside a container runs docker exec under the PTY.
    terminals.open(false, "cell1", Target::ContainerShell { container: "vllm-head".into() }, 80, 24, Box::new(|_| {})).await.unwrap();
    let mut ran = false;
    for _ in 0..100 {
        ran = spark.log.lock().unwrap().iter().any(|c| c.starts_with("docker exec -it -e TERM=xterm-256color vllm-head"));
        if ran {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ran, "docker exec ran under the PTY");
    live.stop();
}

#[tokio::test]
async fn the_activity_log_records_commands_without_secrets() {
    let (_spark, port) = fake_node(&temp("live-audit-home"), docker()).await;
    let (live, known) = live("audit", Some("pw"));
    trusted(&known, port);
    let path = known.parent().unwrap().join("activity.jsonl");
    live.audit.set_path(path.clone());
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;

    live.run("cell1", "docker restart 0123456789ab", None, Duration::from_secs(10)).await.unwrap();
    live.sudo("cell1", "true # restart ollama", Duration::from_secs(10)).await.unwrap();
    live.run("cell1", "systemctl status --no-pager -n 25 -- 'x'", None, Duration::from_secs(10)).await.unwrap();

    let entries = live.audit.entries();
    let find = |c: &str| entries.iter().find(|e| e.command == c).unwrap_or_else(|| panic!("{c} not logged: {entries:?}")).clone();
    let restart = find("docker restart 0123456789ab");
    assert_eq!((restart.kind.as_str(), restart.code, restart.changes, restart.state.as_str()), ("command", Some(0), true, "ok"));
    let sudo = find("true # restart ollama");
    assert_eq!(sudo.kind, "sudo", "logged without the sudo prefix");
    assert!(!find("systemctl status --no-pager -n 25 -- 'x'").changes, "read-only checks are marked");
    assert!(!entries.iter().any(|e| e.command == "true"), "latency pings aren't logged");

    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.contains("docker restart 0123456789ab"));
    assert!(file.contains("\"state\":\"running\""), "written ahead, before the command ran");
    assert!(!file.contains("\"pw"), "the password never reaches the log");
    // A restart reads the file back: one entry per command, its last state.
    let again = audit::Log::new();
    again.set_path(path);
    assert!(again.entries().iter().any(|e| e.command == "docker restart 0123456789ab" && e.state == "ok"));
    live.stop();
}

// ---- Robustness: sessions, staleness, sudo and cancellation, against the fake node.

async fn eventually(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} didn't happen within {secs} s");
}

async fn one_node(name: &str) -> (crate::fake_node::Node, Arc<Live>, Inventory) {
    let (node, port) = fake_node(&temp(&format!("{name}-home")), real_shell()).await;
    let (live, inv) = crate::fake_node::connect(&temp(&format!("{name}-app")), &[("cell1", port)]).await;
    (node, live, inv)
}

#[tokio::test]
async fn an_unknown_host_key_is_refused_with_its_fingerprint_until_trusted() {
    let (_spark, port) = fake_node(&temp("live-tofu-home"), real_shell()).await;
    let (live, known) = live("tofu", Some("pw"));
    live.start(&[node(port)]);
    let n = wait_for(&live, &inventory(port), LinkStatus::Error).await;
    let fp = crate::fake_node::host_fingerprint();
    let e = n.error.unwrap();
    assert!(e.contains("hasn't connected to 127.0.0.1") && e.contains(&fp), "{e}");
    match live.connect_logged(&node(port), "control").await.map(|(s, _)| s) {
        Err(SshError::HostKeyUnknown { address, fingerprint }) => assert_eq!((address, fingerprint), (format!("127.0.0.1:{port}"), fp.clone())),
        other => panic!("expected an unknown key, got {:?}", other.err()),
    }
    trust_host_key(&known, &format!("127.0.0.1:{port}"), &fp).unwrap();
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;
    live.stop();
}

#[tokio::test]
async fn an_unreadable_known_hosts_refuses_to_connect() {
    let (_spark, port) = fake_node(&temp("live-khbroken-home"), real_shell()).await;
    let (live, known) = live("khbroken", Some("pw"));
    std::fs::create_dir_all(known.parent().unwrap()).unwrap();
    std::fs::write(&known, "{\"127.0.0.1:").unwrap();
    live.start(&[node(port)]);
    let n = wait_for(&live, &inventory(port), LinkStatus::Error).await;
    let e = n.error.unwrap();
    assert!(e.starts_with("known_hosts.json couldn't be read; the original is kept at ") && e.contains("Forget host keys"), "{e}");
    forget_host_keys(&known).unwrap();
    trusted(&known, port);
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;
    live.stop();
}

#[tokio::test]
async fn stop_and_start_leave_exactly_one_connection_and_collector_per_node() {
    let (node, live, inv) = one_node("zombies").await;
    eventually("the collector runs", 5, || node.collectors() == 1).await;
    // The same inventory again keeps what's running.
    for _ in 0..3 {
        live.start(&inv.nodes);
        wait_for(&live, &inv, LinkStatus::Online).await;
    }
    assert_eq!((node.total_connections(), node.open_connections(), node.collectors()), (1, 1, 1), "nothing was remade");
    // Stopping and starting makes it again, once.
    for _ in 0..3 {
        live.stop();
        live.start(&inv.nodes);
        wait_for(&live, &inv, LinkStatus::Online).await;
    }
    eventually("old sessions close", 5, || node.open_connections() == 1 && node.collectors() == 1).await;
    assert!(node.total_connections() >= 4);
    live.stop();
    eventually("stop closes everything", 5, || node.open_connections() == 0 && node.collectors() == 0).await;
}

#[tokio::test]
async fn stalled_frames_go_stale_within_4_s_and_the_collector_restarts_on_the_same_connection() {
    let (node, live, inv) = one_node("stale").await;
    node.knobs.frame_every_ms.store(200, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let fresh = live.snapshot(&inv).nodes[0].clone();
    assert!(!fresh.stale && fresh.frame_age_ms.unwrap() < 1500 && fresh.last_seen_ms.is_some());

    let (before, control) = (node.total_connections(), conns(&live, "cell1"));
    node.knobs.stall_frames.store(true, std::sync::atomic::Ordering::SeqCst);
    eventually("stale", 4, || live.snapshot(&inv).nodes[0].stale).await;
    let s = live.snapshot(&inv).nodes[0].clone();
    assert!(s.frame_age_ms.unwrap() >= 3000 && s.status == LinkStatus::Online, "old numbers, marked as old");
    let last_frame_ms = s.last_seen_ms.unwrap();
    eventually("a collector restart", 15, || sshlog_of(&live).contains(" cell1 collector_restart count=1")).await;
    // Timed from the last frame, which can arrive a little before the stall starts on a busy machine.
    let quiet = now_ms().saturating_sub(last_frame_ms);
    assert!(quiet >= DROP_AFTER.as_millis() as u64, "not before {} s without data ({quiet} ms)", DROP_AFTER.as_secs());
    node.knobs.stall_frames.store(false, std::sync::atomic::Ordering::SeqCst);
    eventually("fresh again", 6, || {
        let n = &live.snapshot(&inv).nodes[0];
        n.status == LinkStatus::Online && !n.stale
    })
    .await;
    assert_eq!(node.total_connections(), before, "no new connection");
    assert!(same(&conns(&live, "cell1"), &control), "the same control connection");
    eventually("one collector left", 5, || node.collectors() == 1).await;
    assert_eq!(node.ran("exec python3 -u ~/.sparkbox/collector.py").len(), 2, "started again there");
    live.stop();
}

#[tokio::test]
async fn a_dropped_connection_comes_back() {
    let (node, live, inv) = one_node("drop").await;
    let before = node.total_connections();
    node.drop_connections().await;
    eventually("reconnected", 8, || node.total_connections() > before && live.snapshot(&inv).nodes[0].status == LinkStatus::Online).await;
    live.stop();
}

#[tokio::test]
async fn a_vpn_path_change_remakes_the_sessions() {
    let (node, live, inv) = one_node("path").await;
    live.vpn.simulate_path(true).await;
    let before = node.total_connections();
    live.vpn.simulate_path(false).await;
    eventually("reconnected on the new path", 8, || node.total_connections() > before).await;
    wait_for(&live, &inv, LinkStatus::Online).await;
    live.stop();
}

#[tokio::test]
async fn sudo_sends_the_password_only_to_its_own_prompt() {
    let (node, live, _) = one_node("sudo").await;
    let out = live.sudo("cell1", "id -un", Duration::from_secs(10)).await.unwrap();
    assert_eq!(out.code, Some(0));
    assert!(!out.stderr.contains("SPARKBOX-SUDO"), "the prompt is removed: {:?}", out.stderr);
    let raw = node.raw.lock().unwrap().clone();
    let sent = raw.iter().find(|c| c.contains("sudo -k -S -p 'SPARKBOX-SUDO-")).unwrap();
    assert!(sent.starts_with("sudo -k -S -p 'SPARKBOX-SUDO-") && sent.ends_with(":' -- env LC_ALL=C sh -c 'id -un'"), "{sent}");
    assert!(node.stdin.lock().unwrap().iter().any(|(c, s)| c == "sudo -k -S -p '' id -un" && s == b"pw\n"));

    // NOPASSWD: sudo never asks, so the password must never reach the command.
    node.knobs.nopasswd.store(true, std::sync::atomic::Ordering::SeqCst);
    let out = live.sudo("cell1", "cat", Duration::from_secs(15)).await.unwrap();
    assert_eq!(out.code, Some(0));
    assert!(!out.stdout.contains("pw"), "the password leaked into the command: {:?}", out.stdout);
    let (_, stdin) = node.stdin.lock().unwrap().iter().rev().find(|(c, _)| c.ends_with(" cat")).cloned().unwrap();
    assert!(stdin.is_empty(), "nothing was written to it: {stdin:?}");
    assert!(!live.audit.entries().iter().any(|e| e.command.contains("pw\n")));
    live.stop();
}

#[tokio::test]
async fn a_rejected_sudo_password_trips_the_breaker_until_it_changes() {
    let (fake, port) = fake_node(&temp("breaker-home"), real_shell()).await;
    let dir = temp("breaker-app");
    let known = dir.join("known_hosts.json");
    trusted(&known, port);
    let password = Arc::new(Mutex::new("pw".to_string()));
    let pw = password.clone();
    let live = Live::new(Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}), known, move || Ok(Some(pw.lock().unwrap().clone())));
    let inv = Inventory { nodes: vec![NodeConfig { id: "cell2".into(), name: "cell2".into(), ..node(port) }], switches: vec![] };
    live.start(&inv.nodes);
    wait_for(&live, &inv, LinkStatus::Online).await;
    let node = fake;

    *node.knobs.sudo_password.lock().unwrap() = "something-else".into();
    let e = live.sudo("cell2", "systemctl restart -- 'ollama.service'", Duration::from_secs(10)).await.unwrap_err();
    assert_eq!(e, "The saved cluster password was rejected by sudo on cell2. Save the correct password in Settings.");
    let tries = node.raw.lock().unwrap().iter().filter(|c| c.contains("SPARKBOX-SUDO")).count();
    let again = live.sudo("cell2", "true", Duration::from_secs(10)).await.unwrap_err();
    assert_eq!(again, e, "fails fast");
    assert_eq!(node.raw.lock().unwrap().iter().filter(|c| c.contains("SPARKBOX-SUDO")).count(), tries, "without asking the node again");

    // A new password (saved in Settings) closes the breaker by itself; so does a reset.
    *password.lock().unwrap() = "something-else".into();
    assert_eq!(live.sudo("cell2", "true", Duration::from_secs(10)).await.unwrap().code, Some(0));
    live.reset_sudo_breaker();
    live.stop();
}

#[tokio::test]
async fn cancelling_or_timing_out_stops_the_command_on_the_node() {
    let (node, live, _) = one_node("cancel").await;
    let token = CancelToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        t.cancel();
    });
    let started = Instant::now();
    let out = live.run_cancellable("cell1", "sleep 30", None, Duration::from_secs(60), token).await.unwrap();
    assert!(out.cancelled && started.elapsed() < Duration::from_secs(3), "{out:?} after {:?}", started.elapsed());
    eventually("TERM reached the node", 3, || node.knobs.signals.lock().unwrap().iter().any(|s| s == "TERM")).await;
    let entry = live.audit.entries().into_iter().find(|e| e.command == "sleep 30").unwrap();
    assert_eq!(entry.error.as_deref(), Some("Stopped"));

    let out = live.run_cancellable("cell1", "sleep 30", None, Duration::from_secs(1), CancelToken::new()).await.unwrap();
    assert!(out.timed_out && !out.cancelled);
    let out = live.run_cancellable("cell1", "echo done", None, Duration::from_secs(10), CancelToken::new()).await.unwrap();
    assert_eq!((out.code, out.stdout.as_str()), (Some(0), "done\n"));
    live.stop();
}

#[tokio::test]
async fn streams_arrive_in_chunks_and_stop_at_one_mebibyte() {
    let (_node, live, _) = one_node("streamcap").await;
    let mut got = Vec::new();
    let out = live
        .stream_cmd("cell1", "printf 'a\\nb\\n'; printf err >&2", Duration::from_secs(10), CancelToken::new(), |k, d| got.push((k, d.to_vec())))
        .await
        .unwrap();
    assert_eq!(out.code, Some(0));
    let text: Vec<u8> = got.iter().filter(|(k, _)| *k == StreamKind::Stdout).flat_map(|(_, d)| d.clone()).collect();
    assert_eq!(text, b"a\nb\n");
    assert!(got.iter().any(|(k, d)| *k == StreamKind::Stderr && d == b"err"));

    let mut total = 0;
    let out = live.stream_cmd("cell1", "yes | head -c 3000000", Duration::from_secs(20), CancelToken::new(), |_, d| total += d.len()).await.unwrap();
    assert!(out.truncated && total == OUTPUT_CAP, "{total}");
    live.stop();
}

#[tokio::test]
async fn commands_run_in_the_c_locale_and_probes_stay_out_of_the_log() {
    let (node, live, _) = one_node("locale").await;
    assert_eq!(live.run("cell1", "echo hi", None, Duration::from_secs(5)).await.unwrap().stdout, "hi\n");
    assert!(node.raw.lock().unwrap().contains(&"env LC_ALL=C sh -c 'echo hi'".to_string()));
    // The node's own locale is UTF-8; SparkBox's commands see C.
    assert_eq!(live.run("cell1", "locale | grep LC_CTYPE; echo $LANG", None, Duration::from_secs(5)).await.unwrap().stdout, "LC_CTYPE=\"C\"\nen_US.UTF-8\n");
    live.probe("cell1", "echo quiet", Duration::from_secs(5)).await.unwrap();
    assert!(!live.audit.entries().iter().any(|e| e.command.contains("quiet")));
    assert!(live.audit.entries().iter().any(|e| e.command == "echo hi"));
    let home = live.home_dir("cell1").await.unwrap();
    assert_eq!(home, node.home.display().to_string());
    let asked = node.raw.lock().unwrap().iter().filter(|c| c.contains("$HOME")).count();
    live.home_dir("cell1").await.unwrap();
    assert_eq!(node.raw.lock().unwrap().iter().filter(|c| c.contains("$HOME")).count(), asked, "cached");
    live.stop();
}

#[tokio::test]
async fn an_unreadable_frame_is_reported_on_the_node() {
    let (node, live, inv) = one_node("badframe").await;
    *node.knobs.frame.lock().unwrap() = Some("{\"cpu\": oops".into());
    eventually("the parse error shows", 4, || live.snapshot(&inv).nodes[0].error.as_deref().is_some_and(|e| e.contains("couldn't read"))).await;
    *node.knobs.frame.lock().unwrap() = None;
    eventually("and clears", 4, || live.snapshot(&inv).nodes[0].error.is_none() || live.snapshot(&inv).nodes[0].error.as_deref() == Some("Docker: example warning")).await;
    live.stop();
}

// ---- Exact commands (what used to be rewritten on the way to the node now runs as written)

#[tokio::test]
async fn compound_commands_run_exactly_as_written() {
    let (node, live, _) = one_node("compound").await;
    std::fs::write(node.home.join("there"), "x").unwrap();
    let cmd = "if [ -e there ]; then echo \"it's there\" >&2; exit 3; fi; echo free";
    let out = live.run("cell1", cmd, None, Duration::from_secs(5)).await.unwrap();
    assert_eq!((out.code, out.stderr.as_str()), (Some(3), "it's there\n"));
    let out = live.run("cell1", "if [ -e nothere ]; then exit 3; fi; for i in 1 2; do printf $i; done; echo", None, Duration::from_secs(5)).await.unwrap();
    assert_eq!((out.code, out.stdout.as_str()), (Some(0), "12\n"));
    // sudo too: the same wrapper behind sudo's own prompt.
    let out = live.sudo("cell1", "if true; then echo root-ish; fi; case x in x) echo ok;; esac", Duration::from_secs(10)).await.unwrap();
    assert_eq!(out.stdout, "root-ish\nok\n");
    let raw = node.raw.lock().unwrap().clone();
    assert!(raw.iter().any(|c| c == &wire(cmd)), "{raw:?}");
    assert!(raw.iter().all(|c| !c.starts_with("LC_ALL=C ")), "no bare prefix that breaks compound commands");
    live.stop();
}

#[tokio::test]
async fn a_command_killed_by_a_signal_or_cut_off_is_a_failure() {
    let (node, live, _) = one_node("signals").await;
    node.kill_child_with("KILL", "sleep 5");
    let out = live.run("cell1", "sleep 5; echo never", None, Duration::from_secs(10)).await.unwrap();
    assert_eq!((out.code, out.signal.as_deref()), (None, Some("KILL")), "exit-signal, not exit-status");
    assert_eq!(out.failure("tar", "cell2").unwrap(), "tar on cell2 was killed (SIGKILL)");
    let entry = live.audit.entries().into_iter().find(|e| e.command.starts_with("sleep 5")).unwrap();
    assert_eq!((entry.state.as_str(), entry.error.as_deref()), ("failed", Some("Killed (SIGKILL)")));

    node.close_without_status("echo cut");
    let out = live.run("cell1", "echo cut", None, Duration::from_secs(10)).await.unwrap();
    assert_eq!((out.code, out.signal.as_deref()), (None, None));
    assert_eq!(out.failure("tar", "cell2").unwrap(), "tar on cell2 ended without reporting success");
    assert!(live.run("cell1", "true; echo fine", None, Duration::from_secs(5)).await.unwrap().failure("x", "cell1").is_none());
    live.stop();
}

#[tokio::test]
async fn stdin_streams_to_a_running_command() {
    // The fake starts the command at exec and passes stdin on as it arrives, like sshd: a
    // command that answers before stdin ends is seen answering.
    let (_node, live, _) = one_node("stdinstream").await;
    let mut exec = live.exec_io("cell1", "head -n 1; echo got-first-line").await.unwrap();
    exec.write.data(&b"one\n"[..]).await.unwrap();
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !seen.contains("got-first-line") {
        match tokio::time::timeout_at(deadline, exec.read.wait()).await.expect("no reply before EOF") {
            Some(russh::ChannelMsg::Data { data }) => seen.push_str(&String::from_utf8_lossy(&data)),
            Some(_) => {}
            None => break,
        }
    }
    assert_eq!(seen, "one\ngot-first-line\n");
    let _ = exec.write.eof().await;
    live.stop();
}

// ---- sudo breaker, per node

#[tokio::test]
async fn the_sudo_breaker_is_per_node_until_two_nodes_reject() {
    let (a, pa) = fake_node(&temp("brk2-a"), real_shell()).await;
    let (b, pb) = fake_node(&temp("brk2-b"), real_shell()).await;
    let (c, pc) = fake_node(&temp("brk2-c"), real_shell()).await;
    let (live, _) = crate::fake_node::connect(&temp("brk2-app"), &[("cell1", pa), ("cell2", pb), ("cell3", pc)]).await;
    *b.knobs.sudo_password.lock().unwrap() = "other".into();
    let e = live.sudo("cell2", "true", Duration::from_secs(10)).await.unwrap_err();
    assert!(e.contains("rejected by sudo on cell2"), "{e}");
    assert_eq!(live.sudo("cell1", "echo ok", Duration::from_secs(10)).await.unwrap().stdout, "ok\n", "cell1 still gets sudo");
    assert!(live.sudo("cell2", "true", Duration::from_secs(10)).await.unwrap_err().contains("on cell2"), "named right");

    // A second node rejecting it: the password is wrong, not one node's sudoers.
    *c.knobs.sudo_password.lock().unwrap() = "other".into();
    assert!(live.sudo("cell3", "true", Duration::from_secs(10)).await.unwrap_err().contains("on cell3"));
    let tries = a.raw.lock().unwrap().len();
    let all = live.sudo("cell1", "true", Duration::from_secs(10)).await.unwrap_err();
    assert_eq!(all, "The saved cluster password was rejected by sudo on cell2 and cell3, so SparkBox has stopped using it everywhere. Save the correct password in Settings.");
    assert_eq!(a.raw.lock().unwrap().len(), tries, "cell1 wasn't asked");
    live.reset_sudo_breaker();
    assert_eq!(live.sudo("cell1", "echo ok", Duration::from_secs(10)).await.unwrap().stdout, "ok\n");
    live.stop();
}

// ---- Root runs SparkBox's own script, never one the cluster user can change

#[tokio::test]
async fn root_runs_a_verified_copy_of_sparkjob_in_isolated_mode() {
    let (node, live, _) = one_node("rootpy").await;
    let lib = node.home.parent().unwrap().join(format!("rootlib-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&lib);
    live.set_root_lib(&lib.display().to_string());
    let v = live.sparkjob_root("cell1", "df /", Duration::from_secs(30)).await.unwrap();
    assert!(v["capacity"].as_u64().unwrap() > 0, "{v}");
    assert_eq!(std::fs::read_to_string(lib.join("sparkjob.py")).unwrap(), SPARKJOB, "installed from SparkBox's copy");
    let sha = sha256_hex(SPARKJOB.as_bytes());
    let f = sh_quote(&format!("{}/sparkjob.py", lib.display()));
    let run = format!("sudo -k -S -p '' [ \"$(sha256sum < {f} 2>/dev/null | cut -c1-64)\" = {sha} ] || exit 97; exec python3 -I {f} df /");
    let log = node.log.lock().unwrap().clone();
    assert_eq!(log.iter().filter(|c| **c == run).count(), 2, "first refused (no copy yet), then run: {log:?}");
    assert!(log.iter().any(|c| c.starts_with("sudo -k -S -p '' umask 022 && mkdir -p ")), "installed under sudo");
    assert!(!log.iter().any(|c| c.starts_with("sudo") && c.contains(".sparkbox/sparkjob.py")), "root never runs the user's copy");
    assert!(std::fs::read_to_string(node.state_dir.join("chown.log")).unwrap().contains("chown 0:0"), "made root's");

    // Someone changes root's copy: it isn't run; SparkBox's is put back first.
    std::fs::write(lib.join("sparkjob.py"), "import os; os.system('touch pwned')").unwrap();
    live.sparkjob_root("cell1", "df /", Duration::from_secs(30)).await.unwrap();
    assert!(!node.home.join("pwned").exists());
    assert_eq!(std::fs::read_to_string(lib.join("sparkjob.py")).unwrap(), SPARKJOB);
    live.stop();
}

// ---- Login: keys from files and from an ssh-agent, before the password

fn keygen(dir: &Path, name: &str, passphrase: &str) -> (PathBuf, russh::keys::PublicKey) {
    let path = dir.join(name);
    let ok = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", passphrase, "-C", name, "-f"]).arg(&path).status().unwrap().success();
    assert!(ok, "ssh-keygen");
    let public = russh::keys::PublicKey::from_openssh(std::fs::read_to_string(path.with_extension("pub")).unwrap().trim()).unwrap();
    (path, public)
}

#[tokio::test]
async fn a_key_file_logs_in_before_the_password() {
    let (fake, port) = fake_node(&temp("keyfile-home"), real_shell()).await;
    let dir = temp("keyfile-app");
    let (key, public) = keygen(&dir, "id_spark", "");
    fake.authorize_key(public);
    fake.set_login_password(None);
    let (live, known) = live("keyfile", Some("pw"));
    trusted(&known, port);
    live.set_login_settings(&sshconfig::LoginSettings { identity_file: Some(key.display().to_string()), ..Default::default() }).unwrap();
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;
    let login = live.logins()["cell1"].clone();
    assert_eq!(login.method, ssh::AuthMethod::Key);
    assert_eq!(login.detail, key.display().to_string());
    assert_eq!(fake.knobs.logins.lock().unwrap()[0], "publickey");
    // sudo still uses the cluster password.
    assert_eq!(live.sudo("cell1", "echo ok", Duration::from_secs(10)).await.unwrap().stdout, "ok\n");
    live.stop();

    // A key with a passphrase needs it (from the Keychain); without it, the password is next.
    let (locked, locked_pub) = keygen(&dir, "id_locked", "s3cret");
    fake.authorize_key(locked_pub);
    fake.set_login_password(Some("pw"));
    live.set_login_settings(&sshconfig::LoginSettings { identity_file: Some(locked.display().to_string()), ..Default::default() }).unwrap();
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;
    assert_eq!(live.logins()["cell1"].method, ssh::AuthMethod::Password);
    live.stop();
    crate::secrets::set(&sshconfig::passphrase_account(&locked.display().to_string()), "s3cret").unwrap();
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;
    assert_eq!(live.logins()["cell1"].method, ssh::AuthMethod::Key);
    live.stop();
}

#[tokio::test]
async fn an_ssh_agent_logs_in_first() {
    if std::process::Command::new("ssh-agent").arg("-h").output().is_err() {
        return; // no OpenSSH client tools here
    }
    let (fake, port) = fake_node(&temp("agent-home"), real_shell()).await;
    let dir = temp("agent-app");
    let (key, public) = keygen(&dir, "id_agent", "");
    fake.authorize_key(public);
    fake.set_login_password(None);
    let sock = dir.join("agent.sock");
    let mut agent = std::process::Command::new("ssh-agent").arg("-D").arg("-a").arg(&sock).stdout(std::process::Stdio::null()).spawn().unwrap();
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let added = std::process::Command::new("ssh-add").arg("-q").arg(&key).env("SSH_AUTH_SOCK", &sock).status().unwrap();
    assert!(added.success(), "ssh-add");
    std::fs::remove_file(&key).unwrap(); // only the agent has it now
    let (live, known) = live("agent", None);
    trusted(&known, port);
    live.set_agent_socket(Some(sock.clone()));
    live.start(&[node(port)]);
    wait_for(&live, &inventory(port), LinkStatus::Online).await;
    let login = live.logins()["cell1"].clone();
    assert_eq!((login.method, login.detail.as_str()), (ssh::AuthMethod::Agent, "id_agent via agent"));
    live.stop();
    let _ = agent.kill();
    let _ = agent.wait();
}

#[tokio::test]
async fn ssh_config_names_the_host_port_user_and_key() {
    let (fake, port) = fake_node(&temp("sshcfg-home"), real_shell()).await;
    let dir = temp("sshcfg-app");
    let (key, public) = keygen(&dir, "id_cfg", "");
    fake.authorize_key(public);
    let config = dir.join("config");
    std::fs::write(&config, format!("Host cell1-example.local\n  HostName 127.0.0.1\n  Port {port}\n  User sparkuser\n  IdentityFile {}\n  IdentitiesOnly yes\nMatch all\n  User nobody\n", key.display())).unwrap();
    let (live, known) = live("sshcfg", None);
    trusted(&known, port);
    live.set_ssh_config_path(Some(config));
    // The inventory's address and port are wrong; the config block for the node's name wins.
    let cfg = NodeConfig { ip: Some("10.9.9.9".into()), port: 1, ..node(port) };
    let (agent, keys, password, notes, jump) = live.login_plan(&cfg);
    assert_eq!((agent, keys.len(), password, jump), (None, 1, false, None));
    assert_eq!(notes, ["Match blocks in ~/.ssh/config are ignored."]);
    live.start(std::slice::from_ref(&cfg));
    wait_for(&live, &Inventory { nodes: vec![cfg.clone()], switches: vec![] }, LinkStatus::Online).await;
    assert_eq!(live.logins()["cell1"].method, ssh::AuthMethod::Key);
    live.stop();
}

#[tokio::test]
async fn a_proxy_jump_reaches_the_node_through_another() {
    let (jump, jport) = fake_node(&temp("jump-home"), real_shell()).await;
    let (_target, tport) = fake_node(&temp("jumped-home"), real_shell()).await;
    let dir = temp("jump-app");
    let config = dir.join("config");
    std::fs::write(&config, format!("Host cell1-example.local\n  HostName 127.0.0.1\n  Port {tport}\n  ProxyJump sparkuser@127.0.0.1:{jport}\n")).unwrap();
    let (live, known) = live("jump", Some("pw"));
    crate::fake_node::trust_ports(&known, &[jport, tport]);
    live.set_ssh_config_path(Some(config));
    live.start(&[node(9)]);
    wait_for(&live, &inventory(9), LinkStatus::Online).await;
    assert!(jump.log.lock().unwrap().contains(&format!("direct-tcpip 127.0.0.1:{tport}")), "{:?}", jump.log.lock().unwrap());
    assert_eq!(live.run("cell1", "echo through", None, Duration::from_secs(5)).await.unwrap().stdout, "through\n");
    live.stop();
}

/// SB_TEST_SSHD_DIR is one variable for the whole test run: the tests that set it take turns.
static SSHD_DIR: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn key_login_is_added_proven_and_only_then_password_login_turned_off() {
    let _turn = SSHD_DIR.lock().await;
    let (fake, live, inv) = one_node("setupkey").await;
    let dir = temp("setupkey-keys");
    let (key, public) = keygen(&dir, "id_new", "");
    std::env::set_var("SB_TEST_SSHD_DIR", fake.state_dir.display().to_string());
    let r = live.setup_key_login(&inv.nodes, &key.display().to_string(), true).await;
    assert!(r[0].verified && r[0].password_off, "{r:?}");
    assert_eq!(r[0].message, "Key login works; password login is off");
    let listed = std::fs::read_to_string(fake.home.join(".ssh/authorized_keys")).unwrap();
    assert_eq!(listed.lines().filter(|l| l.contains(&public.to_openssh().unwrap()[..40])).count(), 1);
    assert!(fake.knobs.logins.lock().unwrap().contains(&"publickey".to_string()), "proven with the key alone");
    let dropin = std::fs::read_to_string(fake.state_dir.join("sshd_config.d/00-sparkbox-keys-only.conf")).unwrap();
    assert_eq!(dropin, "PasswordAuthentication no\nKbdInteractiveAuthentication no\n");
    assert!(std::fs::read_to_string(fake.state_dir.join("systemctl.log")).unwrap().contains("reload ssh"), "sshd -t passed, then reload");
    // Twice is harmless: the key is listed once.
    live.setup_key_login(&inv.nodes, &key.display().to_string(), false).await;
    let listed = std::fs::read_to_string(fake.home.join(".ssh/authorized_keys")).unwrap();
    assert_eq!(listed.lines().count(), 1);
    // A missing key says so and changes nothing.
    let r = live.setup_key_login(&inv.nodes, &dir.join("nope").display().to_string(), true).await;
    assert!(!r[0].verified && r[0].message.contains("Couldn't read"), "{r:?}");
    live.stop();
}

/// Ubuntu's cloud image ships 50-cloud-init.conf with `PasswordAuthentication yes`, and sshd
/// keeps the first value it reads: SparkBox's drop-in sorts first, and "off" is only reported
/// when `sshd -T` says so.
#[tokio::test]
async fn password_login_is_off_only_when_sshd_says_so() {
    let _turn = SSHD_DIR.lock().await;
    let (fake, live, inv) = one_node("keysonly").await;
    let dir = temp("keysonly-keys");
    let (key, _) = keygen(&dir, "id_new", "");
    let conf = fake.state_dir.join("sshd_config.d");
    std::fs::create_dir_all(&conf).unwrap();
    std::fs::write(conf.join("50-cloud-init.conf"), "PasswordAuthentication yes\n").unwrap();
    std::env::set_var("SB_TEST_SSHD_DIR", fake.state_dir.display().to_string());
    let r = live.setup_key_login(&inv.nodes, &key.display().to_string(), true).await;
    assert!(r[0].password_off, "00- comes before 50-cloud-init: {r:?}");
    let effective = live.run("cell1", "sshd -T | grep passwordauthentication", None, Duration::from_secs(10)).await.unwrap().stdout;
    assert_eq!(effective, "passwordauthentication no\n");
    // A drop-in that sorts before SparkBox's and says yes: sshd -T says passwords still work, so
    // SparkBox says so, removes its own file and reloads nothing.
    std::fs::write(conf.join("00-a-admin.conf"), "PasswordAuthentication yes\n").unwrap();
    let r = live.setup_key_login(&inv.nodes, &key.display().to_string(), true).await;
    assert!(r[0].verified && !r[0].password_off, "{r:?}");
    assert!(r[0].message.contains("sshd still allows passwords"), "{r:?}");
    assert!(!conf.join("00-sparkbox-keys-only.conf").exists(), "its own file is taken back out");
    live.stop();
}

/// The fake runs commands with dash, as sshd does with an Ubuntu node's `sh`: a bashism fails
/// here, as it would on a Spark, and `sh` on its PATH is dash too.
#[tokio::test]
async fn the_fake_node_runs_dash_so_bashisms_fail_as_on_a_spark() {
    let (_node, live, _) = one_node("dash").await;
    let run = |cmd: &'static str| {
        let live = live.clone();
        async move { live.run("cell1", cmd, None, Duration::from_secs(10)).await.unwrap() }
    };
    let bash_test = run("[[ a == a ]] && echo yes").await;
    assert_ne!(bash_test.code, Some(0), "dash has no [[: {bash_test:?}");
    assert!(bash_test.stderr.contains("[[: not found"), "{:?}", bash_test.stderr);
    assert_eq!(run("[ a = a ] && echo yes").await.stdout, "yes\n", "POSIX test works");
    assert_ne!(run("echo {1..3} | grep -q '1 2 3'").await.code, Some(0), "no brace expansion");
    assert!(run("arr=(a b); echo ${arr[0]}").await.code != Some(0), "no arrays");
    // The wrapper's own `sh -c` is dash as well (it's on the node's PATH first).
    let inner = run("readlink -f \"$(command -v sh)\"").await;
    assert!(inner.stdout.trim().ends_with("dash"), "{inner:?}");
    live.stop();
}

// ---- The owner's Sparks as the Live tour found them (fake_node::Profile::spark)

/// What snapshot-tour-2.json (the real cluster, Oct 29) says each node listens on.
const TOUR_PORTS: [(&str, &[u16]); 4] = [
    ("cell1", &[22, 25, 53, 631, 3350, 3389, 8080, 11000, 11434]),
    ("cell2", &[22, 25, 53, 631, 3350, 3389, 11000]),
    ("cell3", &[22, 25, 53, 631, 3350, 3389, 11000, 11434]),
    ("gx10-0000", &[22, 25, 53, 631, 3350, 3389, 11000, 43179]),
];

/// Four fake Sparks, each with its own key pinned, and Live connected to them.
async fn the_cluster(name: &str) -> (Arc<Live>, Inventory, Vec<crate::fake_node::Node>) {
    use crate::fake_node::{fake_spark, inventory, Profile};
    let mut nodes = Vec::new();
    let mut ports = Vec::new();
    for id in ["cell1", "cell2", "cell3", "gx10-0000"] {
        let (n, port) = fake_spark(&temp(&format!("{name}-{id}")), id).await;
        nodes.push(n);
        ports.push((id, port));
    }
    let (live, known) = live(name, Some("pw"));
    live.set_ssh_config_path(None);
    live.set_agent_socket(None);
    for (id, port) in &ports {
        trust_host_key(&known, &format!("127.0.0.1:{port}"), &Profile::spark(id).fingerprint()).unwrap();
    }
    let mut inv = inventory(&ports);
    for n in &mut inv.nodes {
        n.model = if n.id == "gx10-0000" { "ASUS Ascent GX10".into() } else { "MSI EdgeXpert".into() };
    }
    live.start(&inv.nodes);
    (live, inv, nodes)
}

#[tokio::test]
async fn a_real_spark_reads_as_itself() {
    let (live, inv, _nodes) = the_cluster("real-spark").await;
    // Wait for the real collector's complete first report, including GPU and process
    // samples. Interpreter startup time on this host is not the property under test.
    let snap = crate::fake_node::wait_for("four complete Spark collector reports", || {
        let snap = live.snapshot(&inv);
        snap.nodes.iter().all(|n| n.status == LinkStatus::Online && n.system.driver == "580.178.04" && n.system.docker.is_some() && !n.processes.is_empty()).then_some(snap)
    }, || format!("{:?}", live.snapshot(&inv).nodes.iter().map(|n| (&n.id, n.status, &n.error)).collect::<Vec<_>>())).await;
    let by = |id: &str| snap.nodes.iter().find(|n| n.id == id).unwrap().clone();
    for (i, id) in ["cell1", "cell2", "cell3", "gx10-0000"].iter().enumerate() {
        let n = by(id);
        assert_eq!(n.status, LinkStatus::Online, "{id}: {:?}", n.error);
        let s = &n.system;
        let gx10 = *id == "gx10-0000";
        assert_eq!(s.hostname, if gx10 { "gx10-0000".to_string() } else { format!("{id}-example") });
        assert_eq!(s.model, if gx10 { "ASUS Ascent GX10 (GX10)" } else { "MSI EdgeXpert (MS-C931)" });
        assert_eq!(s.bios.as_deref(), Some(if gx10 { "GX10DGX.0106.2026.0707.0731" } else { "5.36_1.8.0" }));
        assert_eq!(s.os, "NVIDIA DGX Spark (Ubuntu 24.04.5 LTS)");
        assert_eq!((s.kernel.as_str(), s.driver.as_str(), s.cuda.as_str()), ("7.0.0-1019-nvidia", "580.178.04", "13.0"));
        assert_eq!((s.cx7_fw.as_deref(), s.docker.as_deref(), s.ntp_synced), (Some("28.50.1002"), Some("29.6.2"), Some(true)));
        assert_eq!(n.mem.total, if gx10 { 130594021376 } else if *id == "cell3" { 130661122048 } else { 130661117952 });
        let g = &n.gpu;
        assert!(g.readable && g.error.is_none(), "{id}: {:?}", g.error);
        assert_eq!((g.clock_max_mhz, g.power_limit_w, g.pstate.as_deref(), g.sm_clock_mhz, g.util_pct), (Some(3003), None, Some("P8"), 208, 0.0));
        let ip = |iface: &str| n.nets.iter().find(|x| x.iface == iface).and_then(|x| x.ip.clone());
        assert_eq!(ip("enp1s0f1np1"), Some(format!("192.168.100.{}", i + 1)));
        assert_eq!(ip("enP2p1s0f1np1"), Some(format!("192.168.101.{}", i + 1)));
        let port1 = n.nets.iter().find(|x| x.iface == "enp1s0f0np0").unwrap();
        let port2 = n.nets.iter().find(|x| x.iface == "enp1s0f1np1").unwrap();
        assert_eq!((port1.up, port2.up, port2.speed_mbps, port2.mtu), (false, true, 200_000, Some(9000)), "port 1 uncabled, port 2 at 200G");
        let wifi = n.nets.iter().find(|x| x.iface == "wlP9s9").unwrap();
        assert_eq!(wifi.role, NetRole::Wifi);
        let lan = n.nets.iter().find(|x| x.iface == "enP7s7").unwrap();
        assert_eq!((lan.speed_mbps, lan.lan_wol.as_ref().map(|w| w.state.as_str())), (1000, Some("unknown")), "Wake-on-LAN can't be read without root");
        let ports: Vec<u16> = n.listeners.iter().map(|l| l.port).collect();
        assert_eq!(ports, TOUR_PORTS[i].1.to_vec(), "{id}: the same ports as the real node");
        assert!(n.listeners.iter().all(|l| l.pid.is_none()), "no owners without root");
        assert_eq!(n.listeners.iter().find(|l| l.port == 11000).and_then(|l| l.scope.as_deref()), Some("local"));
        assert!(n.proc_total as usize > n.processes.len(), "{id}: {} of {}", n.processes.len(), n.proc_total);
        let names: Vec<&str> = n.processes.iter().map(|p| p.name.as_str()).collect();
        for want in ["dockerd", "containerd", "dashboard-service", "systemd-journald", "sshd", "avahi-daemon", "NetworkManager"] {
            assert!(names.contains(&want), "{id}: no {want} in {names:?}");
        }
        assert!(n.processes.iter().any(|p| p.sparkbox && p.cmd.contains(".sparkbox/collector.py")), "{id}: the collector is marked");
        assert_eq!(n.processes.iter().any(|p| p.name == "ollama" && p.user == "ollama"), matches!(*id, "cell1" | "cell3"), "{id}: ollama");
        assert_eq!(names.contains(&"nginx"), *id == "cell1");
        let key = n.host_key.as_ref().unwrap();
        assert_eq!(key.shared_with, if gx10 { vec![] } else { ["cell1", "cell2", "cell3"].iter().filter(|x| *x != id).map(|x| x.to_string()).collect::<Vec<_>>() });
    }
    let cell3 = by("cell3");
    let wifi = cell3.nets.iter().find(|x| x.iface == "wlP9s9").unwrap();
    assert_eq!((wifi.up, wifi.ip.as_deref(), wifi.carrier_changes, wifi.speed_mbps), (true, Some("192.168.50.229"), Some(154), 867));
    assert_eq!(wifi.wifi.as_ref().and_then(|w| w.signal_dbm), Some(-52));
    assert!(!by("cell1").nets.iter().find(|x| x.iface == "wlP9s9").unwrap().up, "only cell3's Wi-Fi is up");
    let containers: usize = snap.nodes.iter().map(|n| n.containers.len()).sum();
    assert_eq!(containers, 9, "glm53 on all four, workbench-proxy on cell3, four old ones on gx10-0000");
    let glm = by("cell1").containers.into_iter().find(|c| c.name == "glm53").unwrap();
    assert_eq!((glm.state.as_str(), glm.exit_code, glm.image.as_str()), ("exited", 1, "vllm/vllm-openai:glm53-flash-arm64-cu130"));
    assert_eq!(glm.env_flags.get("NCCL_SOCKET_IFNAME").map(String::as_str), Some("enp1s0f1np1"));

    // What SparkBox's read-only commands get back, as from the real node.
    let netplan = live.probe("cell1", "cat /etc/netplan/40-cx7.yaml", Duration::from_secs(10)).await.unwrap();
    assert_eq!((netplan.code, netplan.stderr.trim()), (Some(1), "cat: /etc/netplan/40-cx7.yaml: Permission denied"), "root's and 600");
    let inspect = live.container_inspect("cell1", &glm.id).await.unwrap();
    let env: Vec<&str> = inspect["Config"]["Env"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert!(env.contains(&"SERVED_NAME=glm-5.3-flash") && env.contains(&"API_KEY=<redacted>"), "{env:?}");
    let journal = live.probe("cell1", "journalctl -u 'ollama.service' -n 25 --no-pager -o json", Duration::from_secs(10)).await.unwrap();
    let lines = crate::ops::services::parse_journal_json(&journal.stdout);
    assert!(lines[0].text.contains("Listening on 127.0.0.1:11434 (version 0.21.0)"), "{:?}", lines.first());
    let apt = live.probe("cell1", crate::ops::updates::STATUS_SCRIPT, Duration::from_secs(10)).await.unwrap();
    let pending: Vec<String> = crate::ops::updates::parse_status("cell1", &apt.stdout).pending.into_iter().map(|p| p.name).collect();
    assert_eq!(pending.len(), 4, "{pending:?}");
    assert!(["libbrotli1", "nginx", "nginx-common", "google-chrome-stable"].iter().all(|p| pending.contains(&p.to_string())), "{pending:?}");
    live.stop();
}

#[tokio::test]
async fn a_first_contact_and_a_changed_key_are_told_apart() {
    use crate::fake_node::{fake_spark, Profile};
    let (_n, port) = fake_spark(&temp("hk-cell3"), "cell3").await;
    let (live, known) = live("hostkeys", Some("pw"));
    live.set_ssh_config_path(None);
    live.set_agent_socket(None);
    let inv = crate::fake_node::inventory(&[("cell3", port)]);
    live.start(&inv.nodes);
    let n = wait_for(&live, &inv, LinkStatus::Error).await;
    assert_eq!(n.error_kind.as_deref(), Some("hostKeyUnknown"), "{:?}", n.error);
    let key = n.host_key.clone().unwrap();
    assert_eq!((key.address, key.fingerprint.clone()), (format!("127.0.0.1:{port}"), Profile::spark("cell3").fingerprint()));
    live.stop();
    // Pinned to another key (gx10-0000's): a real mismatch.
    trust_host_key(&known, &format!("127.0.0.1:{port}"), &Profile::spark("gx10-0000").fingerprint()).unwrap();
    live.start(&inv.nodes);
    let n = wait_for(&live, &inv, LinkStatus::Error).await;
    assert_eq!(n.error_kind.as_deref(), Some("hostKeyChanged"), "{:?}", n.error);
    assert!(n.error.as_deref().unwrap().contains("changed (was SHA256:"), "{:?}", n.error);
    assert_eq!(n.host_key.unwrap().fingerprint, key.fingerprint, "the key it presents now");
    live.stop();
}

// ---- Connections that stay up (P1.4, P1.5, P1.6)

#[tokio::test]
async fn a_frozen_collector_restarts_on_the_same_connection() {
    use crate::fake_node::{connect_sparks, fake_spark};
    // A real Spark whose collector sends three frames, then stays open and silent.
    let (node, port) = fake_spark(&temp("freeze-cell2"), "cell2").await;
    node.freeze_frames_after(3);
    let (live, inv) = connect_sparks(&temp("freeze-app"), &[("cell2", port)]).await;
    let (control, before) = (conns(&live, "cell2"), node.total_connections());
    eventually("the collector restarts", 30, || sshlog_of(&live).contains(" cell2 collector_restart count=1")).await;
    let log = sshlog_of(&live);
    let stall = log.lines().find(|l| l.contains(" cell2 frame_stall secs=")).unwrap_or_else(|| panic!("no frame_stall: {log}"));
    let secs: u64 = stall.rsplit('=').next().unwrap().parse().unwrap();
    assert!(secs >= DROP_AFTER.as_secs(), "{stall}");
    eventually("fresh frames again", 20, || {
        let n = &live.snapshot(&inv).nodes[0];
        n.status == LinkStatus::Online && !n.stale
    })
    .await;
    assert!(same(&conns(&live, "cell2"), &control), "the same control connection");
    assert_eq!(node.total_connections(), before, "no new connection");
    let log = sshlog_of(&live);
    assert!(!log.contains(" reconnect ") && !log.contains(" session_closed "), "no reconnect: {log}");
    assert_eq!(node.ran("exec python3 -u ~/.sparkbox/collector.py").len(), 2, "a second collector on the same connection");
    // sshd leaves a plain command running when its channel closes: the stuck one is sent TERM.
    assert!(node.knobs.signals.lock().unwrap().iter().any(|s| s == "TERM"), "{:?}", node.knobs.signals.lock().unwrap());
    live.stop();
}

#[tokio::test]
async fn a_node_that_stops_answering_is_reconnected_once_true_fails() {
    let (node, live, inv) = one_node("hung").await;
    let before = node.total_connections();
    node.hang();
    eventually("the stall check fails and SparkBox reconnects", 30, || sshlog_of(&live).contains(" cell1 reconnect ")).await;
    let log = sshlog_of(&live);
    assert!(log.contains(" cell1 frame_stall secs="), "{log}");
    assert!(log.contains(" cell1 exec_true_failed by=stall msg=\"no reply in 10 s\""), "{log}");
    assert!(log.contains(" cell1 session_closed conn=control reason=\"no reply from cell1 in 10 s\""), "{log}");
    assert!(!log.contains("collector_restart"), "a dead connection isn't a stalled collector: {log}");
    node.thaw();
    eventually("back on a new connection", 30, || node.total_connections() > before && live.snapshot(&inv).nodes[0].status == LinkStatus::Online).await;
    live.stop();
}

#[tokio::test]
async fn more_than_three_restarts_in_five_minutes_counts_as_a_dead_connection() {
    let (node, live, inv) = one_node("restarts").await;
    // Shorten nothing: four stalls of 10 s each, the fourth reconnects.
    node.knobs.stall_frames.store(true, std::sync::atomic::Ordering::SeqCst);
    eventually("the fourth stall reconnects", 60, || sshlog_of(&live).contains(" cell1 reconnect ")).await;
    let log = sshlog_of(&live);
    assert_eq!(log.matches(" collector_restart ").count(), 3, "{log}");
    assert_eq!(log.matches(" frame_stall ").count(), 4, "{log}");
    assert!(log.contains("reason=\"the collector stalled 4 times in 5 minutes\""), "{log}");
    node.knobs.stall_frames.store(false, std::sync::atomic::Ordering::SeqCst);
    wait_for(&live, &inv, LinkStatus::Online).await;
    live.stop();
}

/// A command SparkBox stops waiting for (its future dropped: a time limit around it, a task
/// let go of) is ended on the node by SparkBox: sshd leaves a plain command running when its
/// channel closes, and so does the fake.
#[tokio::test]
async fn a_run_its_caller_drops_is_ended_with_term_not_left_running() {
    let home = temp("live-dropped-run");
    let (node, port) = fake_node(&home, real_shell()).await;
    let (live, _) = crate::fake_node::connect(&temp("live-dropped-run-app"), &[("cell1", port)]).await;
    let pid_file = home.join("run.pid");
    let dropped = tokio::time::timeout(Duration::from_millis(400), live.run("cell1", "echo $$ > run.pid; exec sleep 30", None, Duration::from_secs(60))).await;
    assert!(dropped.is_err(), "still running when it was dropped");
    let mut pid = None;
    eventually("its pid", 5, || {
        pid = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse::<i32>().ok());
        pid.is_some()
    })
    .await;
    let pid = pid.unwrap();
    eventually("TERM ended it", 5, || unsafe { libc::kill(pid, 0) } != 0).await;
    assert_eq!(node.knobs.signals.lock().unwrap().as_slice(), ["TERM"], "SparkBox sent TERM, once");

    // A command that ran to its end gets no signal on the way out.
    assert_eq!(live.run("cell1", "echo done", None, Duration::from_secs(10)).await.unwrap().stdout, "done\n");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(node.knobs.signals.lock().unwrap().len(), 1);
    live.stop();
}

/// A terminal that's dropped (its window gone without the pump, a cancelled open) hangs up on
/// the node: the login shell ends and its channel's place comes back.
#[tokio::test]
async fn a_dropped_terminal_hangs_up_its_login_shell() {
    let home = temp("live-pty-drop");
    let (_node, port) = fake_node(&home, real_shell()).await;
    let (live, _) = crate::fake_node::connect(&temp("live-pty-drop-app"), &[("cell1", port)]).await;
    let mut pty = live.pty("cell1", 80, 24, None).await.unwrap();
    pty_says(&mut pty, "echo $$ > shell.pid; echo written-$((1+1))", "written-2").await;
    let pid: i32 = std::fs::read_to_string(home.join("shell.pid")).unwrap().trim().parse().unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "the login shell runs");
    assert_eq!(live.channel_stats("cell1").in_use, 2, "the collector and the terminal");
    drop(pty);
    eventually("the login shell ended", 5, || unsafe { libc::kill(pid, 0) } != 0).await;
    eventually("its place came back", 15, || live.channel_stats("cell1").in_use == 1).await;
    live.stop();
}

/// Types a command into a terminal and waits for `want` in what comes back.
async fn pty_says(pty: &mut Pty, cmd: &str, want: &str) {
    pty.writer().data(format!("{cmd}\r").as_bytes()).await.unwrap();
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !seen.contains(want) {
        match tokio::time::timeout_at(deadline, pty.read.wait()).await {
            Ok(Some(russh::ChannelMsg::Data { data })) => seen.push_str(&String::from_utf8_lossy(&data)),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => panic!("no {want:?} from the terminal: {seen:?}"),
        }
    }
}

#[tokio::test]
async fn saving_the_inventory_reconnects_only_the_nodes_that_changed() {
    let ids = ["cell1", "cell2", "cell3", "gx10-0000"];
    let mut fakes = Vec::new();
    let mut ports = Vec::new();
    for id in ids {
        let (n, p) = fake_node(&temp(&format!("diff-{id}")), real_shell()).await;
        fakes.push(n);
        ports.push((id, p));
    }
    let dir = temp("diff-app");
    let (live, inv) = crate::fake_node::connect(&dir, &ports).await;
    let mut pty = live.pty("cell1", 80, 24, None).await.unwrap();
    pty_says(&mut pty, "echo one-$((1+1))", "one-2").await;
    let cell1 = conns(&live, "cell1");
    assert!(cell1.0.is_some() && cell1.1.is_some(), "the terminal is on cell1's bulk connection");
    let totals: Vec<usize> = fakes.iter().map(|f| f.total_connections()).collect();

    // cell3 is now reached at another address (its key trusted there too); nothing else changed.
    let p3 = ports[2].1;
    trust_host_key(&dir.join("known_hosts.json"), &format!("localhost:{p3}"), &crate::fake_node::host_fingerprint()).unwrap();
    let mut nodes = inv.nodes.clone();
    nodes[2].ip = Some("localhost".into());
    let moved = Inventory { nodes: nodes.clone(), switches: vec![] };
    live.start(&nodes);
    eventually("cell3 reconnected at its new address", 15, || fakes[2].total_connections() > totals[2] && live.snapshot(&moved).nodes[2].status == LinkStatus::Online).await;
    assert!(same(&conns(&live, "cell1"), &cell1), "cell1 keeps its control and bulk connections");
    for i in [0, 1, 3] {
        assert_eq!(fakes[i].total_connections(), totals[i], "{} made no new connection", ids[i]);
    }
    assert!(fakes[2].total_connections() > totals[2]);
    pty_says(&mut pty, "echo two-$((2+2))", "two-4").await;
    assert!(sshlog_of(&live).contains(" cell3 session_closed conn=control reason=\"cell3's settings changed\""), "{}", sshlog_of(&live));

    // A node taken out is stopped; put back, it starts again. cell1 is never touched.
    live.start(&nodes[..3]);
    eventually("gx10-0000 stopped", 5, || fakes[3].open_connections() == 0).await;
    live.start(&nodes);
    eventually("gx10-0000 back", 10, || live.snapshot(&moved).nodes[3].status == LinkStatus::Online).await;
    assert!(same(&conns(&live, "cell1"), &cell1));
    assert_eq!(fakes[0].total_connections(), totals[0]);
    pty_says(&mut pty, "echo three-$((3+3))", "three-6").await;
    live.stop();
}

#[tokio::test]
async fn twelve_commands_at_once_stay_within_sshds_ten_channels() {
    let (node, live, _) = one_node("twelve").await;
    let started = Instant::now();
    // Twelve different commands: twelve identical reads would share one run.
    let runs: Vec<_> = (0..12)
        .map(|i| {
            let live = live.clone();
            tokio::spawn(async move { live.run("cell1", &format!("sleep 1; echo {i}"), None, Duration::from_secs(30)).await })
        })
        .collect();
    for r in futures_util::future::join_all(runs).await {
        assert_eq!(r.unwrap().unwrap().code, Some(0));
    }
    let peak = node.peak_sessions();
    assert!((9..=10).contains(&peak), "eight commands and the collector at most, not twelve: {peak}");
    assert_eq!(node.knobs.refused.load(std::sync::atomic::Ordering::SeqCst), 0, "never over the node's limit");
    assert!(started.elapsed() >= Duration::from_secs(2), "eight at a time: the rest waited their turn");
    assert_eq!(live.channel_stats("cell1"), ssh::ChannelStats { in_use: 1, queued: 0, refusals: 0, bulk_connections: 0 }, "only the collector afterwards");
    live.stop();
}

#[tokio::test]
async fn a_refused_channel_is_asked_again_not_reported() {
    let (node, live, inv) = one_node("refused").await;
    node.knobs.refuse_next.store(2, std::sync::atomic::Ordering::SeqCst);
    let out = live.run("cell1", "echo ok", None, Duration::from_secs(10)).await.unwrap();
    assert_eq!(out.stdout, "ok\n");
    assert_eq!(live.channel_stats("cell1").refusals, 2);
    let log = sshlog_of(&live);
    assert!(log.contains(" cell1 channel_refused attempt=1 ") && log.contains(" cell1 channel_refused attempt=2 "), "{log}");

    // A node that keeps refusing: busy, not a timeout or a lost node, and it stays Online.
    node.knobs.refuse_next.store(100, std::sync::atomic::Ordering::SeqCst);
    let e = live.run("cell1", "echo again", None, Duration::from_secs(10)).await.unwrap_err();
    node.knobs.refuse_next.store(0, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(e, "SSH channels busy on cell1");
    assert_eq!(error_kind_of(&e), Some("busy"));
    assert_eq!(live.channel_stats("cell1").refusals, 7, "asked five times");
    assert_eq!(live.snapshot(&inv).nodes[0].status, LinkStatus::Online);
    assert_eq!(live.run("cell1", "echo fine", None, Duration::from_secs(10)).await.unwrap().stdout, "fine\n");
    live.stop();
}

#[tokio::test]
async fn a_command_with_no_channel_free_for_20_s_fails_as_busy() {
    let (_node, live, _) = one_node("busy").await;
    let control = live.session("cell1").unwrap();
    let held: Vec<ssh::Slot> = (0..ssh::CHANNELS).map(|_| control.try_slot().expect("eight places")).collect();
    let started = Instant::now();
    let l = live.clone();
    let waiting = tokio::spawn(async move { l.run("cell1", "echo late", None, Duration::from_secs(5)).await });
    eventually("queued", 2, || live.channel_stats("cell1").queued == 1).await;
    let e = waiting.await.unwrap().unwrap_err();
    assert_eq!(e, "SSH channels busy on cell1");
    assert!(started.elapsed() >= ssh::CHANNEL_WAIT, "{:?}", started.elapsed());
    assert!(sshlog_of(&live).contains(" cell1 channels_busy waited_s=20"), "{}", sshlog_of(&live));
    drop(held);
    assert_eq!(live.run("cell1", "echo now", None, Duration::from_secs(5)).await.unwrap().stdout, "now\n");
    live.stop();
}

#[tokio::test]
async fn long_lived_channels_spill_to_an_extra_bulk_connection_that_closes_when_idle() {
    let (node, live, _) = one_node("spill").await;
    let mut execs = Vec::new();
    for _ in 0..9 {
        execs.push(live.exec_io("cell1", "cat").await.unwrap());
    }
    assert_eq!(live.channel_stats("cell1").bulk_connections, 2);
    assert_eq!(live.channel_stats("cell1").in_use, 10, "the collector, eight on the bulk connection and one on the extra");
    assert_eq!(node.open_connections(), 3, "control, bulk and one extra");
    assert!(node.peak_sessions() <= 10);
    let log = sshlog_of(&live);
    assert!(log.contains(" cell1 bulk_open") && log.contains(" cell1 bulk_extra_open bulk_connections=2"), "{log}");
    assert!(log.contains("conn=bulk-extra"), "{log}");
    for e in &execs {
        e.write.eof().await.unwrap(); // cat ends
    }
    drop(execs);
    // Each place comes back once the node has closed its channel.
    eventually("the places come back", 5, || live.channel_stats("cell1").in_use == 1).await;
    live.close_idle_bulk();
    assert_eq!(live.channel_stats("cell1").bulk_connections, 2, "kept for a minute after its last use");
    for (_, last_used) in live.nodes.lock().unwrap().get_mut("cell1").unwrap().extra.iter_mut() {
        *last_used = Instant::now().checked_sub(BULK_IDLE + Duration::from_secs(1)).unwrap();
    }
    live.close_idle_bulk();
    assert_eq!(live.channel_stats("cell1").bulk_connections, 1);
    eventually("the extra connection closes", 5, || node.open_connections() == 2).await;
    assert!(sshlog_of(&live).contains(" cell1 session_closed conn=bulk-extra reason=\"unused for 60 s\""), "{}", sshlog_of(&live));
    live.stop();
}

#[tokio::test]
async fn sparkjobs_checksum_is_checked_once_per_connection() {
    let (node, live, _) = one_node("helperhash").await;
    let sums = || node.ran("sha256sum ~/.sparkbox/sparkjob.py").len();
    live.sparkjob("cell1", "df /", None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(sums(), 1, "checked (and copied) on first use");
    let v = live.sparkjob("cell1", "df /", None, Duration::from_secs(30)).await.unwrap();
    assert!(v["capacity"].as_u64().unwrap() > 0, "{v}");
    assert_eq!(sums(), 1, "the second call runs no sha256sum");
    assert_eq!(node.ran("python3 ~/.sparkbox/sparkjob.py df /").len(), 2);
    // A new connection checks again.
    let before = node.total_connections();
    node.drop_connections().await;
    eventually("reconnected", 10, || live.is_connected("cell1") && node.total_connections() > before).await;
    live.sparkjob("cell1", "df /", None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(sums(), 2);
    live.stop();
}

#[tokio::test]
async fn a_reconnect_marks_the_commands_running_on_it_interrupted() {
    let (_node, live, inv) = one_node("interrupt").await;
    let l = live.clone();
    let running = tokio::spawn(async move { l.run("cell1", "sleep 20", None, Duration::from_secs(60)).await });
    eventually("it runs", 5, || live.audit.entries().iter().any(|e| e.command == "sleep 20" && e.state == "running")).await;
    live.reconnect_all();
    let _ = tokio::time::timeout(Duration::from_secs(10), running).await.expect("the command returns once its connection is gone");
    let e = live.audit.entries().into_iter().find(|e| e.command == "sleep 20").unwrap();
    assert_eq!((e.state.as_str(), e.error.as_deref()), ("failed", Some("Interrupted: the connection was reset (the login settings changed)")));
    // Written once the connection has closed, which can be after the command returned.
    eventually("the close and the reconnect logged", 8, || {
        let log = sshlog_of(&live);
        log.contains(" cell1 session_closed conn=control reason=\"the login settings changed\"") && log.contains(" cell1 reconnect reason=\"the login settings changed\"")
    })
    .await;
    let log = sshlog_of(&live);
    let duration = regex::Regex::new(r#"(?m) cell1 session_closed conn=control reason="the login settings changed" connected_ms=(\d+)$"#).unwrap();
    let elapsed_ms: u128 = duration.captures(&log).expect("the ended control connection has its own duration")[1].parse().unwrap();
    assert!(elapsed_ms > 0, "the connection served a command before reconnecting");
    wait_for(&live, &inv, LinkStatus::Online).await;
    live.stop();
}

#[tokio::test]
async fn the_ssh_log_has_each_connect_and_each_nodes_channel_use() {
    let (_node, live, _) = one_node("sshlog").await;
    live.stream_cmd("cell1", "echo bulk", Duration::from_secs(10), CancelToken::new(), |_, _| {}).await.unwrap();
    live.log_channel_stats();
    let log = sshlog_of(&live);
    let line = |pattern: &str| regex::Regex::new(&format!(r"(?m)^\d{{4}}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{{3}}[+-]\d\d:\d\d dev cell1 {pattern}$")).unwrap();
    assert!(line(r"connect_start path=direct addr=127\.0\.0\.1:\d+ conn=control").is_match(&log), "{log}");
    assert!(line(r"connect_ok path=direct addr=127\.0\.0\.1:\d+ auth=password ms=\d+ conn=control").is_match(&log), "{log}");
    assert!(line(r"connect_ok path=direct addr=127\.0\.0\.1:\d+ auth=password ms=\d+ conn=bulk").is_match(&log), "{log}");
    assert!(line("channel_stats in_use=1 queued=0 refusals=0 bulk_connections=1").is_match(&log), "{log}");
    assert!(!log.contains("=pw") && !log.contains("echo bulk"), "no passwords or commands: {log}");
    assert!(log.lines().next().is_some_and(|l| l.ends_with(" dev mac live_start nodes=1")), "the connects follow a start: {log}");
    live.stop();
    let log = sshlog_of(&live);
    assert!(log.contains(" cell1 session_closed conn=control reason=\"Live data stopped\"") && log.contains(" dev mac live_stop"), "{log}");
}

#[tokio::test]
async fn a_vpn_path_change_is_logged_as_such() {
    let (_node, live, inv) = one_node("pathlog").await;
    live.vpn.simulate_path(true).await;
    live.vpn.simulate_path(false).await;
    eventually("the path change", 8, || sshlog_of(&live).contains(" cell1 path_change from=direct to=vpn")).await;
    // The reconnect line follows once the old connection has closed.
    eventually("the reconnect line", 8, || sshlog_of(&live).contains(" cell1 reconnect reason=\"the network path changed\"")).await;
    wait_for(&live, &inv, LinkStatus::Online).await;
    live.stop();
}

#[test]
fn a_node_that_doesnt_answer_is_offline_and_a_refused_login_an_error() {
    let f = |m: &str| link_status(NodeFailure::from(m.to_string()));
    let (status, timeout) = f("SSH timeout: no reply from 192.168.50.154:22 in 10 s. The node may be off or restarting, or this Mac isn't on its network.");
    assert_eq!((status, timeout.kind, timeout.message.as_str()), (LinkStatus::Offline, Some("timeout"), "No SSH reply in 10 s"));
    assert_eq!(f("SSH handshake with 192.168.50.98:22 timed out (through the VPN)").0, LinkStatus::Offline);
    assert_eq!(f("Couldn't reach 192.168.50.98:22: Connection refused (os error 61)").0, LinkStatus::Offline);
    assert_eq!(f("Couldn't reach 192.168.50.98:22: No route to host (os error 65)").0, LinkStatus::Offline);
    assert_eq!(f("Couldn't reach cell9.local:22: failed to lookup address information: nodename nor servname provided, or not known").0, LinkStatus::Offline);
    assert_eq!(f("The password for sparkuser@192.168.50.154 was rejected. Check it in Settings.").0, LinkStatus::Error);
    assert_eq!(f("The collector on cell2 stopped (exit 2): boom").0, LinkStatus::Error, "anything unexpected");
    let unknown = NodeFailure::from(SshError::HostKeyUnknown { address: "a:22".into(), fingerprint: "SHA256:x".into() });
    assert_eq!(link_status(unknown).0, LinkStatus::Error);
    let changed = NodeFailure::from(SshError::HostKeyChanged { address: "a:22".into(), pinned: "SHA256:x".into(), fingerprint: "SHA256:y".into() });
    assert_eq!(link_status(changed).0, LinkStatus::Error);
}

#[tokio::test]
async fn a_node_whose_ssh_doesnt_answer_reads_offline_no_ssh_reply() {
    let (fake, port) = fake_node(&temp("noreply-home"), real_shell()).await;
    fake.hang();
    let (quiet, known) = live("noreply", Some("pw"));
    trusted(&known, port);
    quiet.start(&[node(port)]);
    let inv = inventory(port);
    eventually("offline", 15, || quiet.snapshot(&inv).nodes[0].status == LinkStatus::Offline).await;
    let n = quiet.snapshot(&inv).nodes[0].clone();
    assert_eq!((n.error.as_deref(), n.error_kind.as_deref()), (Some("No SSH reply in 10 s"), Some("timeout")));
    assert!(sshlog_of(&quiet).contains(&format!(" cell1 connect_fail path=direct addr=127.0.0.1:{port} kind=timeout ")), "{}", sshlog_of(&quiet));
    let failure_duration = regex::Regex::new(r"(?m) connect_fail .* ms=(\d+) conn=control$").unwrap();
    let log = sshlog_of(&quiet);
    let elapsed_ms: u128 = failure_duration.captures(&log).expect("failed attempts include elapsed milliseconds")[1].parse().unwrap();
    assert!(elapsed_ms >= 10_000, "a full handshake timeout is distinguishable from an immediate refusal: {log}");
    fake.thaw();
    quiet.stop();

    // Nothing listening: refused, Offline too. A rejected password stays an Error.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (refused, _) = live("refusedport", Some("pw"));
    refused.start(&[node(closed)]);
    eventually("offline", 5, || refused.snapshot(&inventory(closed)).nodes[0].status == LinkStatus::Offline).await;
    assert_eq!(refused.snapshot(&inventory(closed)).nodes[0].error_kind.as_deref(), Some("refused"));
    assert!(failure_duration.is_match(&sshlog_of(&refused)), "refused TCP attempts are timed too");
    refused.stop();
    let (wrong, known) = live("wrongpw", Some("nope"));
    let (_f, p) = fake_node(&temp("wrongpw-home"), real_shell()).await;
    trusted(&known, p);
    wrong.start(&[node(p)]);
    let n = wait_for(&wrong, &inventory(p), LinkStatus::Error).await;
    assert_eq!(n.error_kind.as_deref(), Some("authFailed"));
    let log = sshlog_of(&wrong);
    assert!(failure_duration.is_match(&log), "rejected authentication stays a distinct, timed failure");
    assert!(!log.contains("nope"), "elapsed-time diagnostics must not include the password");
    wrong.stop();
}

// ---- Round 1 fixes: a collector that ends, dropped channels, kicks, a replaced sparkjob.py

#[tokio::test]
async fn a_collector_killed_on_the_node_restarts_on_the_same_connection() {
    use crate::fake_node::{connect_sparks, fake_spark};
    // A real Spark running the real collector.py; the OOM killer takes it.
    let (node, port) = fake_spark(&temp("killed-cell2"), "cell2").await;
    let (live, inv) = connect_sparks(&temp("killed-app"), &[("cell2", port)]).await;
    let (control, before) = (conns(&live, "cell2"), node.total_connections());
    let seen: Arc<Mutex<Vec<LinkStatus>>> = Arc::default();
    let watcher = {
        let (live, inv, seen) = (live.clone(), inv.clone(), seen.clone());
        tokio::spawn(async move {
            loop {
                seen.lock().unwrap().push(live.snapshot(&inv).nodes[0].status);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
    };
    // This node's collector only: its home is the test's own.
    let pattern = format!("{}/.sparkbox/collector.py", node.home.display());
    assert!(std::process::Command::new("pkill").args(["-KILL", "-f", &pattern]).status().unwrap().success(), "pkill found the collector");
    eventually("the collector restarts", 15, || sshlog_of(&live).contains(" cell2 collector_restart count=1")).await;
    let restarted_ms = now_ms();
    eventually("fresh frames again", 15, || {
        let n = &live.snapshot(&inv).nodes[0];
        n.last_seen_ms.is_some_and(|t| t > restarted_ms) && !n.stale
    })
    .await;
    watcher.abort();
    let log = sshlog_of(&live);
    assert!(log.contains(" cell2 collector_exit code=none signal=KILL"), "{log}");
    assert!(!log.contains(" reconnect ") && !log.contains(" session_closed "), "no reconnect: {log}");
    assert!(same(&conns(&live, "cell2"), &control), "the same control connection");
    assert_eq!(node.total_connections(), before, "no new connection");
    let statuses = seen.lock().unwrap().clone();
    assert!(statuses.len() > 20 && statuses.iter().all(|s| *s == LinkStatus::Online), "Online all along: {statuses:?}");
    live.stop();
}

#[tokio::test]
async fn a_node_without_python3_is_an_error_on_a_connection_that_stays_up() {
    let (fake, port) = fake_node(&temp("nopython-home"), real_shell()).await;
    fake.knobs.real_collector.store(true, std::sync::atomic::Ordering::SeqCst);
    // What dash says when `exec python3` finds none.
    fake.bin("python3", "echo 'sh: 1: exec: python3: not found' >&2; exit 127");
    let (live, known) = live("nopython", Some("pw"));
    trusted(&known, port);
    live.start(&[node(port)]);
    let n = wait_for(&live, &inventory(port), LinkStatus::Error).await;
    assert_eq!(n.error.as_deref(), Some("python3 isn't installed on cell1."));
    let before = fake.total_connections();
    assert_eq!(live.run("cell1", "echo still-here", None, Duration::from_secs(5)).await.unwrap().stdout, "still-here\n", "commands still run on it");
    assert!(live.is_connected("cell1"));
    assert_eq!(fake.total_connections(), before);
    let log = sshlog_of(&live);
    assert!(log.contains(" cell1 collector_exit code=127 signal=none"), "{log}");
    assert!(!log.contains(" reconnect ") && !log.contains(" session_closed "), "{log}");
    live.stop();
}

#[tokio::test]
async fn a_collector_that_keeps_ending_is_shown_and_its_connection_kept() {
    let (fake, port) = fake_node(&temp("crashing-home"), real_shell()).await;
    fake.knobs.real_collector.store(true, std::sync::atomic::Ordering::SeqCst);
    fake.bin("python3", "echo 'Traceback: boom' >&2; exit 1");
    let (live, known) = live("crashing", Some("pw"));
    trusted(&known, port);
    live.start(&[node(port)]);
    let inv = inventory(port);
    eventually("the collector is held", 30, || live.snapshot(&inv).nodes[0].status == LinkStatus::Error).await;
    let n = live.snapshot(&inv).nodes[0].clone();
    assert_eq!(n.error.as_deref(), Some("The collector on cell1 stopped 4 times in 5 minutes (exit 1: Traceback: boom); SparkBox tries again in a minute."));
    let log = sshlog_of(&live);
    assert_eq!(log.matches(" cell1 collector_exit code=1 ").count(), 4, "{log}");
    assert_eq!(log.matches(" cell1 collector_restart ").count(), 3, "{log}");
    assert!(log.contains(" cell1 collector_held secs=60 "), "{log}");
    assert!(!log.contains(" reconnect ") && !log.contains(" session_closed "), "the connection stays: {log}");
    assert_eq!(fake.total_connections(), 1);
    assert_eq!(live.run("cell1", "echo up", None, Duration::from_secs(5)).await.unwrap().stdout, "up\n");
    live.stop();
}

#[tokio::test]
async fn a_dropped_exec_ends_its_command_and_keeps_its_place_until_the_node_has_closed_it() {
    let (node, live, _) = one_node("execdrop").await;
    // Three copies whose callers let go without ending their input (cancelled transfers).
    for _ in 0..3 {
        drop(live.exec_io("cell1", "cat").await.unwrap());
    }
    let mut held = Vec::new();
    for i in 0..8 {
        held.push(live.exec_io("cell1", "cat").await.unwrap_or_else(|e| panic!("open {i}: {e}")));
    }
    assert_eq!(node.knobs.refused.load(std::sync::atomic::Ordering::SeqCst), 0, "never over the node's limit");
    eventually("the three abandoned commands got TERM", 5, || node.knobs.signals.lock().unwrap().iter().filter(|s| *s == "TERM").count() >= 3).await;
    eventually("their places are free once the node closed them", 5, || live.channel_stats("cell1").in_use == 9).await;
    drop(held);
    eventually("and these eight's", 5, || live.channel_stats("cell1").in_use == 1).await;
    assert_eq!(node.knobs.refused.load(std::sync::atomic::Ordering::SeqCst), 0);
    live.stop();
}

#[tokio::test]
async fn a_bulk_connection_the_node_refuses_spills_to_an_extra_one() {
    let (node, live, _) = one_node("refusespill").await;
    live.stream_cmd("cell1", "true", Duration::from_secs(10), CancelToken::new(), |_, _| {}).await.unwrap();
    assert_eq!(live.channel_stats("cell1").bulk_connections, 1);
    // The bulk connection's node counts places SparkBox thinks are free (a lower MaxSessions, channels still closing).
    node.knobs.refuse_next.store(5, std::sync::atomic::Ordering::SeqCst);
    let exec = live.exec_io("cell1", "cat").await.expect("opened on an extra connection");
    let stats = live.channel_stats("cell1");
    assert_eq!((stats.bulk_connections, stats.refusals), (2, 5), "{stats:?}");
    let log = sshlog_of(&live);
    assert!(log.contains(" cell1 bulk_full in_use=") && log.contains(" cell1 bulk_extra_open bulk_connections=2"), "{log}");
    exec.write.eof().await.unwrap();
    drop(exec);
    live.stop();
}

#[tokio::test]
async fn two_kicks_close_together_reconnect_once() {
    let (node, live, inv) = one_node("twokicks").await;
    let before = node.total_connections();
    live.reconnect_all();
    live.reconnect_all();
    eventually("reconnected", 10, || node.total_connections() > before && live.snapshot(&inv).nodes[0].status == LinkStatus::Online).await;
    // A kick carried over would end the new connection within moments of connect_ok.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(node.total_connections(), before + 1, "one new connection");
    let log = sshlog_of(&live);
    assert!(!log.contains("asked to reconnect"), "{log}");
    assert_eq!(log.matches(" cell1 reconnect reason=\"the login settings changed\"").count(), 1, "{log}");
    assert_eq!(live.snapshot(&inv).nodes[0].status, LinkStatus::Online);
    live.stop();
}

#[tokio::test]
async fn a_sparkjob_another_sparkbox_put_there_is_never_run() {
    let (node, live, _) = one_node("helperswap").await;
    let sums = || node.ran("sha256sum ~/.sparkbox/sparkjob.py").len();
    live.sparkjob("cell1", "df /", None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(sums(), 1);
    // The other SparkBox (an older release, say) copies its own over ours mid-connection.
    let mine = node.home.join(".sparkbox/sparkjob.py");
    let theirs = "import pathlib\npathlib.Path('their-copy-ran').touch()\nprint('{\"capacity\": 0}')\n";
    std::fs::write(&mine, theirs).unwrap();
    let v = live.sparkjob("cell1", "df /", None, Duration::from_secs(30)).await.unwrap();
    assert!(v["capacity"].as_u64().unwrap() > 0, "ours answered: {v}");
    assert!(!node.home.join("their-copy-ran").exists(), "theirs never ran");
    assert_eq!(std::fs::read_to_string(&mine).unwrap(), SPARKJOB, "ours is back");
    assert_eq!(sums(), 2, "checked again once it was found changed");

    // A sparkjob command sent any other way (the lease's own exec) is refused too, and the next
    // check puts ours back.
    std::fs::write(&mine, theirs).unwrap();
    let session = live.session("cell1").unwrap();
    let out = session.exec(&wire("python3 ~/.sparkbox/sparkjob.py df /"), None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(out.code, Some(COPY_STALE), "{out:?}");
    assert!(out.stderr.contains("isn't this SparkBox's copy"), "{out:?}");
    assert!(!node.home.join("their-copy-ran").exists());
    live.ensure_file("cell1", "sparkjob.py", SPARKJOB).await.unwrap();
    assert_eq!(std::fs::read_to_string(&mine).unwrap(), SPARKJOB);
    assert_eq!(sums(), 3);
    live.stop();
}

/// P1.46: a key on a process's or a container's command line in the node's frame is masked
/// before the snapshot leaves the backend (Workloads, "Copy command line", the Containers list).
#[tokio::test]
async fn the_snapshot_masks_secrets_in_process_and_container_command_lines() {
    let (node, live, inv) = one_node("snapmask").await;
    let key = "sk-live-abcdefgh12345678";
    let hf = "hf_AbCdEf0123456789AbCdEf0123456789ab";
    let mut f: serde_json::Value = serde_json::from_str(&frame()).unwrap();
    f["processes"] = json!([{ "pid": 4242, "name": "vllm", "user": "sparkuser", "cmd": format!("vllm serve /models/x --api-key {key} --port 8000"), "cpuPct": 1.0, "rss": 1, "nice": 0 }]);
    f["containers"][0]["command"] = json!(format!("bash -c 'HF_TOKEN={hf} vllm serve /models/x --api-key {key}'"));
    *node.knobs.frame.lock().unwrap() = Some(f.to_string());
    eventually("the frame arrives", 5, || live.snapshot(&inv).nodes[0].processes.len() == 1).await;
    let snap = live.snapshot(&inv);
    let (cmd, command) = (&snap.nodes[0].processes[0].cmd, &snap.nodes[0].containers[0].command);
    assert_eq!(cmd, "vllm serve /models/x --api-key ‹redacted› --port 8000");
    assert!(command.contains("HF_TOKEN=‹redacted›") && command.contains("--api-key ‹redacted›"), "{command}");
    assert!(![cmd, command].iter().any(|t| t.contains(key) || t.contains(hf)), "{cmd} / {command}");
    live.stop();
}

#[tokio::test]
async fn a_collector_with_a_long_stderr_that_isnt_ascii_is_read_to_its_end() {
    let (_node, live, inv) = one_node("utf8err").await;
    // Its own connection: the collector's place on the control one is taken.
    let s = live.connect(&inv.nodes[0], "test").await.unwrap();
    // 6000 bytes of "€" (3 bytes each): the old cut at len − 2000 fell inside one.
    let end = s.stream(&wire("python3 -c \"import sys; sys.stderr.buffer.write(b'\\xe2\\x82\\xac' * 2000)\"; exit 3"), |_| {}).await.unwrap();
    assert_eq!(end.code, Some(3));
    assert!(end.stderr.len() <= 2002 && !end.stderr.is_empty() && end.stderr.chars().all(|c| c == '€'), "{} bytes", end.stderr.len());
    s.close().await;
    live.stop();
}

// ---- R3-FOOT: shared reads, and rows never left running

/// The activity rows whose command is `cmd`.
fn rows_of(live: &Live, cmd: &str) -> Vec<audit::Entry> {
    live.audit.entries().into_iter().filter(|e| e.command == cmd).collect()
}

#[tokio::test]
async fn two_identical_reads_at_once_share_one_exec_and_one_row() {
    let (node, live, _) = one_node("shared-run").await;
    // Each exec prints its own shell's pid: a shared run gives both callers the same one.
    let cmd = "sleep 0.5; echo shared-$$";
    let (a, b) = tokio::join!(live.run("cell1", cmd, None, Duration::from_secs(10)), live.run("cell1", cmd, None, Duration::from_secs(10)));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(a.stdout.starts_with("shared-") && a.stdout == b.stdout, "{a:?} {b:?}");
    assert_eq!(node.ran("echo shared-").len(), 1, "one exec on the node");
    let rows = rows_of(&live, cmd);
    assert_eq!(rows.len(), 1, "one activity row");
    assert_eq!(rows[0].state, "ok");
    // Asked again once that run finished: run afresh (a read right after a change must be fresh).
    let c = live.run("cell1", cmd, None, Duration::from_secs(10)).await.unwrap();
    assert_ne!(c.stdout, a.stdout);
    assert_eq!((node.ran("echo shared-").len(), rows_of(&live, cmd).len()), (2, 2));
    live.stop();
}

#[tokio::test]
async fn a_probe_is_answered_from_its_result_for_2_s_only() {
    let (node, live, _) = one_node("shared-probe").await;
    let cmd = "echo probe-$$";
    // Each ask from a task of its own, as a window's command or another poller asks.
    let ask = |live: &Arc<Live>| {
        let live = live.clone();
        async move { tokio::spawn(async move { live.probe("cell1", cmd, Duration::from_secs(5)).await }).await.unwrap().unwrap() }
    };
    let first = ask(&live).await;
    let again = ask(&live).await;
    assert_eq!(first.stdout, again.stdout);
    assert_eq!(node.ran("echo probe-").len(), 1, "within 2 s: the same result, no second exec");
    tokio::time::sleep(PROBE_REUSE + Duration::from_millis(200)).await;
    let later = ask(&live).await;
    assert_ne!(later.stdout, first.stdout);
    assert_eq!(node.ran("echo probe-").len(), 2, "after 2 s: asked again");
    // A change sent to the node in between: the next probe asks afresh, even within 2 s.
    live.run("cell1", "touch changed-here", None, Duration::from_secs(5)).await.unwrap();
    let after_change = ask(&live).await;
    assert_ne!(after_change.stdout, later.stdout);
    assert_eq!(node.ran("echo probe-").len(), 3);
    // No caller gets the same reading twice: a loop asking again (an endpoint round's next
    // scrape) gets a fresh one, whoever ran the last.
    let looped = tokio::spawn({
        let live = live.clone();
        async move {
            let a = live.probe("cell1", cmd, Duration::from_secs(5)).await.unwrap();
            let b = live.probe("cell1", cmd, Duration::from_secs(5)).await.unwrap();
            let c = live.probe("cell1", cmd, Duration::from_secs(5)).await.unwrap();
            (a.stdout, b.stdout, c.stdout)
        }
    })
    .await
    .unwrap();
    assert_eq!(looped.0, after_change.stdout, "new to this caller: the reading of a moment ago");
    assert!(looped.1 != looped.0 && looped.2 != looped.1, "{looped:?}");
    assert_eq!(node.ran("echo probe-").len(), 5);
    // A caller outside any task (one that can't be told apart) always gets a fresh one.
    let (x, y) = (live.probe("cell1", cmd, Duration::from_secs(5)).await.unwrap(), live.probe("cell1", cmd, Duration::from_secs(5)).await.unwrap());
    assert_ne!(x.stdout, y.stdout);
    // Two callers that shared one run: neither gets that reading again when it asks again.
    let slow = "sleep 0.3; echo slow-$$";
    let twice = |live: &Arc<Live>| {
        let live = live.clone();
        tokio::spawn(async move {
            let a = live.probe("cell1", slow, Duration::from_secs(5)).await.unwrap().stdout;
            let b = live.probe("cell1", slow, Duration::from_secs(5)).await.unwrap().stdout;
            (a, b)
        })
    };
    let (one, two) = (twice(&live), twice(&live));
    let (one, two) = (one.await.unwrap(), two.await.unwrap());
    assert_eq!(one.0, two.0, "the first asks shared one run");
    assert!(one.1 != one.0 && two.1 != two.0, "{one:?} {two:?}");
    // Nor is one node's result another's: the key is the node too.
    assert!(live.probe("cell9", cmd, Duration::from_secs(5)).await.is_err());
    live.stop();
}

#[tokio::test]
async fn a_read_asked_after_a_change_doesnt_join_one_from_before_it() {
    let (node, live, _) = one_node("shared-epoch").await;
    let cmd = "sleep 1; echo epoch-$$";
    let later = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        live.run("cell1", "touch moved-on", None, Duration::from_secs(5)).await.unwrap();
        // The first read is still running, but it began before the change.
        live.run("cell1", cmd, None, Duration::from_secs(10)).await
    };
    let (a, b) = tokio::join!(live.run("cell1", cmd, None, Duration::from_secs(10)), later);
    assert_ne!(a.unwrap().stdout, b.unwrap().stdout);
    assert_eq!(node.ran("echo epoch-").len(), 2);
    live.stop();
}

#[tokio::test]
async fn a_change_is_never_shared() {
    let (node, live, _) = one_node("shared-change").await;
    let cmd = "sleep 0.3; touch twice-$$";
    let (a, b) = tokio::join!(live.run("cell1", cmd, None, Duration::from_secs(10)), live.run("cell1", cmd, None, Duration::from_secs(10)));
    assert_eq!((a.unwrap().code, b.unwrap().code), (Some(0), Some(0)));
    assert_eq!(node.ran("touch twice-").len(), 2, "two execs");
    assert_eq!(rows_of(&live, cmd).len(), 2, "two rows");
    // A read someone typed (Fleet) isn't shared either, nor one with input.
    let typed = "sleep 0.3; echo typed-$$";
    let (a, b) = audit::as_typed(async { tokio::join!(live.run("cell1", typed, None, Duration::from_secs(10)), live.run("cell1", typed, None, Duration::from_secs(10))) }).await;
    assert_ne!(a.unwrap().stdout, b.unwrap().stdout);
    let fed = "sleep 0.3; cat; echo fed-$$";
    let (a, b) = tokio::join!(live.run("cell1", fed, Some(b"x"), Duration::from_secs(10)), live.run("cell1", fed, Some(b"x"), Duration::from_secs(10)));
    assert_ne!(a.unwrap().stdout, b.unwrap().stdout);
    live.stop();
}

#[tokio::test]
async fn when_the_caller_running_a_shared_read_stops_waiting_the_other_runs_it() {
    let (node, live, _) = one_node("shared-drop").await;
    let cmd = "sleep 1; echo lead-$$";
    let leader = tokio::time::timeout(Duration::from_millis(300), live.run("cell1", cmd, None, Duration::from_secs(10)));
    let follower = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        live.run("cell1", cmd, None, Duration::from_secs(10)).await
    };
    let (gone, got) = tokio::join!(leader, follower);
    assert!(gone.is_err(), "the first caller stopped waiting");
    assert!(got.unwrap().stdout.starts_with("lead-"), "the second got an answer of its own");
    assert_eq!(node.ran("echo lead-").len(), 2);
    let mut states: Vec<String> = rows_of(&live, cmd).into_iter().map(|e| e.state).collect();
    states.sort();
    assert_eq!(states, vec!["cancelled".to_string(), "ok".to_string()]);
    live.stop();
}

/// Ported from round 2's verifier (v_dropped_run_row_state): a run SparkBox stops waiting for.
#[tokio::test]
async fn a_dropped_run_ends_its_row_cancelled_and_the_command_gets_term() {
    let home = temp("dropped-run");
    let (node, port) = fake_node(&home, real_shell()).await;
    let (live, _) = crate::fake_node::connect(&temp("dropped-run-app"), &[("cell1", port)]).await;
    let cmd = "echo $$ > run.pid; exec sleep 30";
    let dropped = tokio::time::timeout(Duration::from_millis(400), live.run("cell1", cmd, None, Duration::from_secs(60))).await;
    assert!(dropped.is_err(), "still running when dropped: {dropped:?}");
    let row = rows_of(&live, cmd).pop().expect("its row");
    assert_eq!((row.state.as_str(), row.error.as_deref()), ("cancelled", Some(audit::STOPPED_WAITING)), "never left running");
    let mut pid = None;
    eventually("its pid", 5, || {
        pid = std::fs::read_to_string(home.join("run.pid")).ok().and_then(|t| t.trim().parse::<i32>().ok());
        pid.is_some()
    })
    .await;
    let pid = pid.unwrap();
    eventually("TERM ended the command", 5, || unsafe { libc::kill(pid, 0) } != 0).await;
    assert!(node.knobs.signals.lock().unwrap().iter().any(|s| s == "TERM"));
    // A reconnect afterwards doesn't rewrite it as interrupted.
    node.drop_connections().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(rows_of(&live, cmd).pop().unwrap().state, "cancelled");
    live.stop();
}

/// The sudo variant (sudo_exchange's CloseOnDrop::term): its row ends cancelled too.
#[tokio::test]
async fn a_dropped_sudo_ends_its_row_cancelled_and_the_command_gets_term() {
    let home = temp("dropped-sudo");
    let (node, port) = fake_node(&home, real_shell()).await;
    let (live, _) = crate::fake_node::connect(&temp("dropped-sudo-app"), &[("cell1", port)]).await;
    let cmd = "echo $$ > sudo.pid; exec sleep 30";
    let dropped = tokio::time::timeout(Duration::from_millis(1500), live.sudo("cell1", cmd, Duration::from_secs(60))).await;
    assert!(dropped.is_err(), "still running when dropped: {dropped:?}");
    let row = live.audit.entries().into_iter().find(|e| e.kind == "sudo" && e.command == cmd).expect("its row");
    assert_eq!((row.state.as_str(), row.error.as_deref()), ("cancelled", Some(audit::STOPPED_WAITING)));
    let mut pid = None;
    eventually("its pid", 5, || {
        pid = std::fs::read_to_string(home.join("sudo.pid")).ok().and_then(|t| t.trim().parse::<i32>().ok());
        pid.is_some()
    })
    .await;
    let pid = pid.unwrap();
    eventually("TERM ended the sudo'd command", 5, || unsafe { libc::kill(pid, 0) } != 0).await;
    assert!(node.knobs.signals.lock().unwrap().iter().any(|s| s == "TERM"));
    live.stop();
}

/// Dropped before its channel opened (a node that stopped answering): the row says nothing ran.
#[tokio::test]
async fn a_run_dropped_before_it_was_sent_says_it_didnt_run() {
    let (node, live, _) = one_node("dropped-unsent").await;
    node.hang();
    let cmd = "touch never-sent";
    let dropped = tokio::time::timeout(Duration::from_millis(400), live.run("cell1", cmd, None, Duration::from_secs(60))).await;
    node.thaw();
    assert!(dropped.is_err());
    let row = rows_of(&live, cmd).pop().expect("its row");
    assert_eq!((row.state.as_str(), row.error.as_deref()), ("cancelled", Some(audit::STOPPED_BEFORE_SENT)));
    assert!(node.ran("never-sent").is_empty());
    live.stop();
}

/// C11: a background poll on a node that isn't connected is a failed check in the poll's
/// operation, not a failed change.
#[tokio::test]
async fn a_poll_of_a_node_that_isnt_connected_is_a_failed_check() {
    let (_node, live, _) = one_node("poll-offline").await;
    let poll = audit::OpMeta { title: "Check updates".into(), source: "poll".into() };
    let out = audit::with_op_id("poll-1".into(), poll, audit::as_probe(live.run("cell9", "apt list --upgradable", None, Duration::from_secs(5)))).await;
    assert!(out.is_err());
    let row = rows_of(&live, "apt list --upgradable").pop().unwrap();
    assert_eq!((row.op_id.as_deref(), row.class.as_str(), row.changes, row.state.as_str(), row.source.as_str()), (Some("poll-1"), "probe", false, "failed", "poll"));
    assert_eq!(row.kind, "command");
    live.stop();
}

/// A read joined by a caller that allowed longer: each gets its own limit, never the other's
/// "took longer than" (round 3's verifier, v_shared_follower_gets_the_leaders_shorter_limit).
#[tokio::test]
async fn a_shared_read_keeps_each_callers_own_time_limit() {
    let (node, live, _) = one_node("shared-limit").await;
    let cmd = "sleep 2; echo lim-$$";
    let follower = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        live.run("cell1", cmd, None, Duration::from_secs(20)).await
    };
    let (a, b) = tokio::join!(live.run("cell1", cmd, None, Duration::from_secs(1)), follower);
    assert!(a.unwrap_err().contains("took longer than 1s"));
    assert!(b.as_ref().is_ok_and(|o| o.stdout.starts_with("lim-")), "the 20 s caller got its answer: {b:?}");
    assert_eq!(node.ran("echo lim-").len(), 2);
    // The other way round: one that allowed 1 s isn't kept waiting on a 20 s caller's run.
    let started = Instant::now();
    let follower = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let r = live.run("cell1", cmd, None, Duration::from_secs(1)).await;
        (r, started.elapsed())
    };
    let (a, (b, took)) = tokio::join!(live.run("cell1", cmd, None, Duration::from_secs(20)), follower);
    assert!(a.is_ok());
    assert!(b.unwrap_err().contains("took longer than 1s") && took < Duration::from_millis(1800), "{took:?}");
    // The same limit still shares one run.
    let (a, b) = tokio::join!(live.probe("cell1", cmd, Duration::from_secs(5)), live.probe("cell1", cmd, Duration::from_secs(5)));
    assert_eq!(a.unwrap().stdout, b.unwrap().stdout);
    live.stop();
}

/// Workloads' exit watch (process_alive every 500 ms, each its own task): a fresh probe sees the
/// exit at the next ask, where a plain probe within 2 s of the last would still say "running"
/// (round 3's verifier, v_exit_watch_sees_an_exit_late_through_probe_reuse).
#[tokio::test]
async fn a_fresh_probe_sees_an_exit_at_the_next_ask() {
    let (node, live, _) = one_node("fresh-probe").await;
    let mut child = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    let cmd = format!("ps -o pid= -p {}", child.id());
    let ask = |fresh: bool| {
        let (live, cmd) = (live.clone(), cmd.clone());
        async move {
            tokio::spawn(async move { if fresh { live.probe_fresh("cell1", &cmd, Duration::from_secs(10)).await } else { live.probe("cell1", &cmd, Duration::from_secs(10)).await } })
                .await
                .unwrap()
                .unwrap()
        }
    };
    assert!(!ask(true).await.stdout.trim().is_empty(), "running");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(ask(true).await.stdout.trim().is_empty(), "gone at the next ask");
    assert_eq!(node.ran("ps -o pid=").len(), 2, "each asked the node");
    // A plain probe right after takes the fresh one's result (it's new to that task); a fresh
    // one after that still asks the node.
    assert!(ask(false).await.stdout.trim().is_empty());
    assert_eq!(node.ran("ps -o pid=").len(), 2);
    ask(true).await;
    assert_eq!(node.ran("ps -o pid=").len(), 3);
    live.stop();
}

/// Live → Demo while a log follower's bulk connection is still being made (round 3's staging
/// run): the connection is closed as it comes up, and the follower never runs on it.
#[tokio::test]
async fn stopping_while_a_bulk_connection_is_made_closes_it_and_runs_nothing() {
    let (node, live, _) = one_node("stop-mid-bulk").await;
    node.hang();
    let follower = tokio::spawn({
        let live = live.clone();
        async move { live.stream_cmd("cell1", "echo followed-after-stop", Duration::from_secs(30), CancelToken::new(), |_, _| {}).await }
    });
    eventually("the bulk connection is being made", 5, || sshlog_of(&live).contains("conn=bulk")).await;
    live.stop();
    node.thaw();
    let out = tokio::time::timeout(Duration::from_secs(20), follower).await.unwrap().unwrap();
    assert_eq!(out.unwrap_err(), not_connected("cell1"));
    assert!(node.ran("followed-after-stop").is_empty(), "nothing ran on it");
    let log = sshlog_of(&live);
    let after = &log[log.find("live_stop").expect("stopped")..];
    assert!(!after.contains("bulk_open"), "{log}");
    assert!(after.contains(" cell1 session_closed conn=bulk reason=\"the node was stopped while it connected\""), "{log}");
    eventually("every connection closed", 5, || node.open_connections() == 0).await;
}

/// The same for an extra bulk connection, made when the first one is full.
#[tokio::test]
async fn stopping_while_an_extra_bulk_connection_is_made_closes_it() {
    let (node, live, _) = one_node("stop-mid-extra").await;
    let mut execs = Vec::new();
    while live.channel_stats("cell1").bulk_connections < 2 && execs.len() < 8 {
        execs.push(live.exec_io("cell1", "cat").await.unwrap());
    }
    assert_eq!(live.channel_stats("cell1").bulk_connections, 1, "the first one has room for eight");
    node.hang();
    let ninth = tokio::spawn({
        let live = live.clone();
        async move { live.exec_io("cell1", "cat").await.map(|_| ()) }
    });
    eventually("the extra connection is being made", 5, || sshlog_of(&live).contains("conn=bulk-extra")).await;
    live.stop();
    node.thaw();
    let out = tokio::time::timeout(Duration::from_secs(20), ninth).await.unwrap().unwrap();
    assert_eq!(out.unwrap_err(), not_connected("cell1"));
    let log = sshlog_of(&live);
    let after = &log[log.find("live_stop").expect("stopped")..];
    assert!(!after.contains("bulk_extra_open"), "{log}");
    assert!(after.contains(" cell1 session_closed conn=bulk-extra reason=\"the node was stopped while it connected\""), "{log}");
    drop(execs);
    eventually("every connection closed", 5, || node.open_connections() == 0).await;
}

/// The wizard's login check obeys "Fall back to the cluster password" as a real connection does:
/// with it off, a node that only takes the password fails at Log in, where before the check passed
/// and the real connection then failed. sudo is another step and keeps the password.
#[tokio::test]
async fn connect_with_obeys_the_password_fallback_and_sudo_keeps_the_password() {
    use crate::live::sshconfig::LoginSettings;
    use crate::onboard::probe_node;
    let (spark, port) = fake_node(&temp("fallback-home"), real_shell()).await;
    let base = temp("fallback-app");
    let (live, inv) = crate::fake_node::connect(&base, &[("cell2", port)]).await;
    live.set_agent_socket(None);
    open_fence(&live, &base, &["cell2"], &[crate::live::fence_class::FenceClass::SudoProbe]);
    let cfg = &inv.nodes[0];
    let status = |steps: &[crate::onboard::CheckStep], id: &str| steps.iter().find(|s| s.id == id).map(|s| s.status.clone());

    let on = probe_node(&live, cfg, Some("pw".into()), None, true).await;
    assert_eq!((status(&on, "auth").as_deref(), status(&on, "sudo").as_deref()), (Some("ok"), Some("ok")), "{on:?}");

    // Off: the check and a real connection agree that nothing logs in.
    live.set_login_settings(&LoginSettings { password_fallback: false, ..Default::default() }).unwrap();
    assert!(live.connect_with(cfg, Some("pw"), ssh::Trust::Normal).await.is_err(), "connect_with ignored the setting");
    assert!(live.connect_logged(cfg, "control").await.is_err(), "a real connection refuses too");
    let off = probe_node(&live, cfg, Some("pw".into()), None, true).await;
    assert_eq!(status(&off, "auth").as_deref(), Some("fail"), "{off:?}");

    // Off, with a key that works: the key logs in, and sudo still takes the password.
    let key = base.join("id_ed25519");
    assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&key).status().unwrap().success());
    spark.authorize_key(russh::keys::PublicKey::from_openssh(std::fs::read_to_string(key.with_extension("pub")).unwrap().trim()).unwrap());
    live.set_login_settings(&LoginSettings { identity_file: Some(key.display().to_string()), password_fallback: false, ..Default::default() }).unwrap();
    spark.stdin.lock().unwrap().clear();
    let keyed = probe_node(&live, cfg, Some("pw".into()), None, true).await;
    assert_eq!((status(&keyed, "auth").as_deref(), status(&keyed, "sudo").as_deref()), (Some("ok"), Some("ok")), "{keyed:?}");
    assert_eq!(keyed.iter().find(|s| s.id == "method").unwrap().auth_method, Some(ssh::AuthMethod::Key));
    assert!(spark.stdin.lock().unwrap().iter().any(|(c, s)| c == "sudo -k -S -p '' true" && s == b"pw\n"), "sudo got the password on stdin");
    live.stop();
}
