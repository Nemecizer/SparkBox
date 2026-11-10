//! P1 Session B, steps 18 to 28 with 19a, rehearsed on fake nodes (docs/acceptance/P1-session-B.md).
//! Each test walks the playbook's steps that send a command to a node through the entry points the
//! windows use (Ops, the update check, Terminals, Fleet, Models, Chat's provider, evidence), with
//! the write exit enforcing a fence signed the way Settings › Fence signs one, and asserts what the
//! exit admitted, under which class, and the activity rows it wrote; and that the same step is
//! refused where the fence leaves it out.
//!
//! Since 2026-10-30 the release build needs no fence, so Settings no longer offers these presets.
//! `cargo test` is a debug build, whose exit asks for a fence whatever instance its rules name
//! (exit.rs `BUILD_NEEDS_FENCE`), so these walks keep proving what they always did: each step's
//! class, the rows it writes, and the refusal where a fence leaves it out. The presets live on here
//! as fixtures. live/exit_release_tests.rs walks the release build's path.
//!
//! The three fences were src/lib/fence.ts's presets: "P1 Session B · cell2" (cell2; power-one,
//! apt-refresh, apt-one, apt-hold, shell, container-test, fleet, model-load-one, model-unload,
//! drop-caches; 3 h) for the test container, the restarts, the installs and first light on cell2;
//! "P1 Session B · all nodes" (all four; apt-refresh, apt-one, apt-hold, service, shell; service
//! only for nginx.service; 3 h) for Check all nodes, the terminal on cell3 and the other nodes; and
//! "P1 Session B · cell3 Wi-Fi" (cell3; wifi; 1 h) for step 26.
//!
//! A fake GB10 here answers the checks before a restart (ops/prereboot.rs `fake`), the update
//! check from its own package state (`Apt`), and apt, dpkg-query, needrestart, apt-mark and
//! systemd-run through fake_node.rs's shims; Docker, nmcli and the vLLM container through
//! stand-ins (`docker_test`, `nmcli`, models/fake_vllm.rs). What a fake can't show (a real reboot,
//! real apt and dpkg, a GPU, the windows, the timing on the cluster) is the owner's, in the
//! session, or the staging tier's (live/staging_tests.rs `staging_session_b_*`); the playbook
//! says which.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::exit::{Enforce, ExecKind, Verdict, KEY_KIND};
use super::fence::{self, Spec};
use super::fence_class::FenceClass;
use super::{audit, now_ms, Live};
use crate::errors::{ErrorKind, SbError};
use crate::fake_node::{self, fake_node, temp, Node, Script};
use crate::health::{boots, AlertState, Engine, EventKind, Severity};
use crate::inventory::Inventory;
use crate::model::{ClusterSnapshot, LinkStatus, NodeSnapshot, SystemInfo};
use crate::ops::prereboot::{self, fake::Gb10, fake::K};
use crate::ops::update_check::{self, UpdateCache};
use crate::ops::updates::{self, UpgradeArgs};
use crate::ops::{Ops, Planned};
use crate::tasks::{StepState, Task, TaskState, Tasks};
use crate::term::{Target, Terminals};

use FenceClass::{AptHold, AptOne, AptRefresh, ContainerTest, DropCaches, Fleet, ModelLoadOne, ModelUnload, PowerOne, Service, Shell, Wifi};

const MAC: &str = "Studio";
const TAG: &str = "p1-r4-rehearsal";
const FOUR: [&str; 4] = ["cell1", "cell2", "cell3", "gx10-0000"];
/// The "P1 Session B · cell2" preset, in the order src/lib/fence.ts had it.
const CELL2: [FenceClass; 10] = [PowerOne, AptRefresh, AptOne, AptHold, Shell, ContainerTest, Fleet, ModelLoadOne, ModelUnload, DropCaches];
/// The "P1 Session B · all nodes" preset, in the order src/lib/fence.ts had it.
const ALL: [FenceClass; 5] = [AptRefresh, AptOne, AptHold, Service, Shell];
/// The "P1 Session B · cell3 Wi-Fi" preset, as src/lib/fence.ts had it.
const WIFI: [FenceClass; 1] = [Wifi];

/// The "P1 Session B · cell2" preset.
fn cell2_fence() -> Spec {
    Spec { nodes: vec!["cell2".into()], classes: CELL2.to_vec(), minutes: 180, instance: "release".into(), note: "P1 Session B · cell2".into(), ..Default::default() }
}

/// The "P1 Session B · all nodes" preset.
fn all_fence() -> Spec {
    let only = BTreeMap::from([(Service, vec!["nginx.service".to_string()])]);
    Spec { nodes: FOUR.map(String::from).to_vec(), classes: ALL.to_vec(), only, minutes: 180, instance: "release".into(), note: "P1 Session B · all nodes".into() }
}

/// The "P1 Session B · cell3 Wi-Fi" preset.
fn wifi_fence() -> Spec {
    Spec { nodes: vec!["cell3".into()], classes: WIFI.to_vec(), minutes: 60, instance: "release".into(), note: "P1 Session B · cell3 Wi-Fi".into(), ..Default::default() }
}

/// The shared fence folder, the release build's signing key, and the release build's config
/// folder trusting it (as Session A's rehearsal keeps them).
struct Fences {
    dir: PathBuf,
    config: PathBuf,
    key: p256::ecdsa::SigningKey,
    file: fence::KeyFile,
}

impl Fences {
    fn new(base: &Path) -> Fences {
        let dir = base.join("SparkBox Fence");
        std::fs::create_dir_all(&dir).unwrap();
        let key = fence::new_key().unwrap();
        let (key_id, public_key) = fence::public_of(&key).unwrap();
        let f = Fences { dir, config: base.join("config-release"), key, file: fence::KeyFile { key_id, public_key, kind: KEY_KIND.into() } };
        fence::save_trust(&f.config, &f.file).unwrap();
        f
    }

    fn rules(&self) -> Enforce {
        Enforce { instance: "release".into(), fence_dir: self.dir.clone(), config_dir: Some(self.config.clone()), noauth: false, tag: TAG.into(), mac: Some(MAC.into()) }
    }

    /// Opens `spec` as fence_open does, a moment after the last one so it's the newer.
    fn open(&self, spec: &Spec) {
        std::thread::sleep(Duration::from_millis(3));
        let r = fence::record_for(spec, &self.key, now_ms(), MAC).unwrap();
        fence::write(&self.dir, &self.config, &self.file, &r).unwrap();
    }
}

// ---- A fake GB10

/// What apt on a fake node says for the update check: `apt list --upgradable` lines, when dpkg
/// last wrote its status (s), and fwupd's answer. The Session B nodes as the doctor captured them
/// on 29 October (live/fixtures/doctor/<node>/apt-upgradable.txt): cell1 libbrotli1, nginx and
/// nginx-common (security) and google-chrome-stable, 4 (3 security); the others libbrotli1 and
/// google-chrome-stable, 2 (1 security). Unattended upgrades are off on all four, as Updates
/// showed them that day; gx10-0000's polkit turns the cluster user's firmware read away.
#[derive(Clone)]
struct Apt {
    upgradable: Vec<String>,
    dpkg_s: u64,
    fwupd: String,
}

const LISTS_S: u64 = 1_793_300_000;

impl Apt {
    fn of(node: &str) -> Apt {
        let captured = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/live/fixtures/doctor").join(node).join("apt-upgradable.txt");
        let text = std::fs::read_to_string(&captured).unwrap_or_else(|e| panic!("{}: {e}", captured.display()));
        let upgradable = text.lines().filter(|l| !l.trim().is_empty() && *l != "Listing...").map(String::from).collect();
        let fwupd = if node == "gx10-0000" { "Failed to get updates: Not authorized\nfwupd-exit=1" } else { "No updatable devices\nfwupd-exit=2" };
        Apt { upgradable, dpkg_s: LISTS_S + 600, fwupd: fwupd.into() }
    }

    /// `updates::STATUS_SCRIPT`'s answer.
    fn status(&self) -> String {
        format!(
            "--- upgradable\nListing...\n{}\n--- held\n--- reboot\n--- unattended\nAPT::Periodic::Unattended-Upgrade \"0\";\n--- checked\n{LISTS_S}\n",
            self.upgradable.join("\n")
        )
    }

    /// `update_check::CHECK_SCRIPT`'s answer: the status, then DGX OS 7.6.0, fwupd, no snaps, no
    /// Ollama (so nothing asks GitHub), live-restore on, the stamps.
    fn check(&self) -> String {
        format!(
            "{}--- dgx\nDGX_SWBUILD_VERSION=\"7.2.3\"\nDGX_OTA_VERSION=\"7.6.0\"\nDGX_OTA_DATE=\"Wed Oct 15 2026\"\n--- fwupd\n{}\n--- snap\nAll snaps up to date.\n--- ollama\n--- docker\ntrue\n--- stamps\n{}",
            self.status(),
            self.fwupd,
            self.stamps()
        )
    }

    /// `update_check::STAMPS`'s answer (no /var/run/reboot-required).
    fn stamps(&self) -> String {
        format!("/var/lib/apt/periodic/update-success-stamp {LISTS_S}\n/var/lib/dpkg/status {}\n", self.dpkg_s)
    }
}

/// A GB10: the checks before a restart pass (or fail as `gb10`), the update check reads `apt`, and
/// a `sudo reboot` typed into Fleet is answered here (never by this Mac).
fn gb10(state: Gb10, apt: Arc<Mutex<Apt>>) -> Script {
    Arc::new(move |cmd: &str, _: &[u8]| {
        if let Some(a) = prereboot::fake::answer(state, cmd) {
            return Some(a);
        }
        let apt = apt.lock().unwrap();
        match cmd {
            c if c == update_check::CHECK_SCRIPT => Some((apt.check(), 0)),
            c if c == update_check::STAMPS => Some((apt.stamps(), 0)),
            c if c == updates::STATUS_SCRIPT => Some((apt.status(), 0)),
            _ => None,
        }
    })
}

/// Session B's apt-get on a fake node: a dry run of everything keeps held packages back, as apt
/// does ("The following packages have been kept back:"); anything else is fake_node.rs's apt-get.
const APT_GET_HOLDS: &str = r#"#!/bin/sh
st="$SB_FAKE_STATE/apt"
case " $* " in
  *" -s "*" dist-upgrade "*)
    held=""; free=""; n=0; k=0
    for p in $(cat "$st/pending" 2>/dev/null); do
      if grep -qx -- "$p" "$st/holds" 2>/dev/null; then held="$held $p"; k=$((k + 1)); else free="$free $p"; n=$((n + 1)); fi
    done
    echo "NOTE: This is only a simulation!"
    echo "Reading package lists..."
    echo "Building dependency tree..."
    echo "Reading state information..."
    echo "Calculating upgrade..."
    if [ -n "$held" ]; then echo "The following packages have been kept back:"; echo " $held"; fi
    echo "The following packages will be upgraded:"
    echo " $free"
    echo "$n upgraded, 0 newly installed, 0 to remove and $k not upgraded."
    for p in $free; do echo "Inst $p [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])"; done
    for p in $free; do echo "Conf $p (2.0-1 Ubuntu:24.04/noble-updates [arm64])"; done
    exit 0 ;;
esac
exec "$(dirname "$0")/apt-get.fake" "$@"
"#;

/// `apt-get update` printing the warning a Spark with Google's repo prints, on stderr as apt does,
/// then the fake's own; half a second each, so four that run at once overlap.
const APT_GET_WARNS: &str = r#"#!/bin/sh
case " $* " in
  *" update "*) sleep 0.5; echo "W: https://dl.google.com/linux/chrome/deb/dists/stable/InRelease: Key is stored in legacy trusted.gpg keyring (/etc/apt/trusted.gpg), see the DEPRECATION section in apt-key(8) for details." >&2 ;;
esac
exec "$(dirname "$0")/apt-get.fake" "$@"
"#;

/// Puts `wrapper` in front of the node's apt-get (which it runs as apt-get.fake).
fn wrap_apt_get(node: &Node, wrapper: &str) {
    std::fs::copy(node.bin_dir.join("apt-get"), node.bin_dir.join("apt-get.fake")).unwrap();
    node.bin("apt-get", wrapper);
}

/// Fake nodes named as the cluster's (`names`), each a GB10 in `state` with its own apt; this Mac
/// connected to them as the release build with its own data folder `app`.
struct Cluster {
    live: Arc<Live>,
    inventory: Inventory,
    nodes: BTreeMap<String, Node>,
    apt: BTreeMap<String, Arc<Mutex<Apt>>>,
}

async fn cluster(app: &Path, names: &[&str], state: impl Fn(&str) -> Gb10) -> Cluster {
    let (mut nodes, mut apt, mut ports) = (BTreeMap::new(), BTreeMap::new(), Vec::new());
    for name in names {
        let a = Arc::new(Mutex::new(Apt::of(name)));
        let home = app.join(format!("home-{name}"));
        std::fs::create_dir_all(&home).unwrap();
        let (node, port) = fake_node(&home, gb10(state(name), a.clone())).await;
        std::fs::create_dir_all(node.state_dir.join("apt")).unwrap();
        std::fs::write(node.state_dir.join("apt/delay"), "0.05").unwrap();
        // dpkg's database has every package apt lists as upgradable, at the version the fake
        // apt-get upgrades from.
        let installed: String = a.lock().unwrap().upgradable.iter().filter_map(|l| l.split('/').next()).map(|p| format!("{p}\t1.0-1\n")).collect();
        std::fs::write(node.state_dir.join("apt/installed"), installed).unwrap();
        // apt-get takes C1′'s `name=version` pins as the real one does (R4-UPD's stand-in).
        crate::ops::updates::fake::pin_aware(&node);
        set_frame(&node, &format!("{name}-boot-1"));
        nodes.insert(name.to_string(), node);
        apt.insert(name.to_string(), a);
        ports.push((*name, port));
    }
    let (live, inventory) = fake_node::connect(app, &ports).await;
    live.set_root_lib(&app.join("rootlib").display().to_string());
    Cluster { live, inventory, nodes, apt }
}

impl Cluster {
    fn node(&self, name: &str) -> &Node {
        &self.nodes[name]
    }

    /// A second SparkBox on the same data folder, as after a relaunch.
    async fn relaunch(&self, app: &Path) -> (Arc<Live>, Inventory) {
        let ports: Vec<(&str, u16)> = self.inventory.nodes.iter().map(|n| (n.id.as_str(), n.port)).collect();
        let (live, inventory) = fake_node::connect(app, &ports).await;
        live.set_root_lib(&app.join("rootlib").display().to_string());
        (live, inventory)
    }
}

/// The node's collector frames from now on: this boot id, every 200 ms.
fn set_frame(node: &Node, boot: &str) {
    let mut v: Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    v["bootId"] = json!(boot);
    v["system"]["uptimeS"] = json!(50_000);
    v["system"]["driver"] = json!("580.178.04");
    *node.knobs.frame.lock().unwrap() = Some(v.to_string());
    node.knobs.frame_every_ms.store(200, std::sync::atomic::Ordering::SeqCst);
}

/// Ops over the fake nodes, its snapshots sent through `power_overlay` as lib.rs publish sends them.
fn ops_for(live: &Arc<Live>, inventory: &Inventory) -> Arc<Ops> {
    let demo = Arc::new(Mutex::new(crate::demo::DemoCluster::new(&Inventory::example())));
    let ops = Ops::with_poll(live.clone(), demo, Tasks::new(None), Duration::from_millis(20));
    let (l, inv, me) = (live.clone(), inventory.clone(), Arc::downgrade(&ops));
    ops.set_snapshot_source(move || {
        let mut snap = l.snapshot(&inv);
        me.upgrade()?.power_overlay(&mut snap);
        Some(snap)
    });
    ops
}

/// Planned work as the alerting would be told it, in order.
#[derive(Default)]
struct Said(Mutex<Vec<String>>);

impl Said {
    fn said(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl Planned for Said {
    fn begin(&self, node: &str, _: Option<u64>, reason: &str) {
        self.0.lock().unwrap().push(format!("begin {node} {reason}"));
    }

    fn end(&self, node: &str) {
        self.0.lock().unwrap().push(format!("end {node}"));
    }

    fn resolved(&self, _: &str, _: &str, _: &str) {}
}

async fn wait_task(tasks: &Tasks, id: &str, secs: u64) -> Task {
    let started = Instant::now();
    loop {
        let t = tasks.get(id).unwrap();
        if t.finished() {
            return t;
        }
        assert!(started.elapsed() < Duration::from_secs(secs), "task {} still running: {:?}", t.title, t.steps);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn until(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(secs), "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The rows of operation `op`, oldest first, without its head line.
fn rows_of(live: &Live, op: &str) -> Vec<audit::Entry> {
    let mut rows: Vec<audit::Entry> = live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(op) && e.kind != "op").collect();
    rows.reverse();
    rows
}

fn head_of(live: &Live, op: &str) -> audit::Entry {
    live.audit.entries().into_iter().find(|e| e.op_id.as_deref() == Some(op) && e.kind == "op").unwrap_or_else(|| panic!("no head line for {op}"))
}

/// (title, kind, command, fence, state) of a row, as the playbook's tables list them.
fn cols(e: &audit::Entry) -> (String, String, String, Option<String>, String) {
    (e.title.clone(), e.kind.clone(), e.command.clone(), e.fence.clone(), e.state.clone())
}

/// How many times `cmd`, exactly, reached `node` (the update check's script contains the stamps read).
fn ran(node: &Node, cmd: &str) -> usize {
    node.log.lock().unwrap().iter().filter(|c| *c == cmd).count()
}

fn refusals(live: &Live) -> Vec<audit::Entry> {
    live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect()
}

fn lease_claim(row: &audit::Entry) -> bool {
    row.kind == "command" && row.command == format!("python3 ~/.sparkbox/sparkjob.py lease claim 'release' '{TAG}' '{MAC}' 45") && row.fence.is_none()
}

// ---- The three presets

#[test]
fn the_three_session_b_fixtures_are_valid_fences() {
    let ids: Vec<String> = FOUR.map(String::from).to_vec();
    assert_eq!(
        CELL2.map(FenceClass::id),
        ["power-one", "apt-refresh", "apt-one", "apt-hold", "shell", "container-test", "fleet", "model-load-one", "model-unload", "drop-caches"]
    );
    assert_eq!(ALL.map(FenceClass::id), ["apt-refresh", "apt-one", "apt-hold", "service", "shell"]);
    assert_eq!(WIFI.map(FenceClass::id), ["wifi"]);
    assert!(!ALL.contains(&PowerOne), "no restart can leave under the all-nodes fence");
    assert!(!CELL2.contains(&Wifi) && !ALL.contains(&Wifi), "Wi-Fi only under its own fence, on cell3");
    for spec in [cell2_fence(), all_fence(), wifi_fence()] {
        fence::validate(&spec, &ids).unwrap();
    }
    let f = Fences::new(&temp("rehearsal-b-presets"));
    let now = now_ms();
    let summary = |spec: &Spec| fence::record_for(spec, &f.key, now, MAC).unwrap().summary(now);
    assert_eq!(summary(&cell2_fence()), "cell2 · 10 classes · 3 h");
    assert_eq!(summary(&all_fence()), "cell1, cell2, cell3, gx10-0000 · 5 classes · 3 h");
    assert_eq!(summary(&wifi_fence()), "cell3 · 1 class · 1 h");
}

// ---- Step 18: the test container on cell2, a shell in it, then remove it

/// The test container's id as the stand-in's Docker prints it.
const BUSYBOX: &str = "5b0c7e1f2a3d4c5b6a7980f1e2d3c4b5a69788776655443322110ffeeddccbba";

/// Docker 29 as far as step 18 needs it, which this Mac hasn't got: `run -d --name N …` prints the
/// new container's id; `exec -it [-e …] N sh -c S` runs S in "the container" (this node's own
/// shell) while N runs, as dockerd refuses it otherwise; `stop` and `rm` print what they were
/// given. Its state is one file per container name under the node's state folder.
const DOCKER_TEST: &str = r#"#!/bin/dash
st="$SB_FAKE_STATE/docker-test"
mkdir -p "$st"
id=5b0c7e1f2a3d4c5b6a7980f1e2d3c4b5a69788776655443322110ffeeddccbba
case "$1" in
  run)
    name=""; prev=""
    for a in "$@"; do [ "$prev" = --name ] && name="$a"; prev="$a"; done
    echo running > "$st/$name"; echo "$id" ;;
  exec)
    shift
    while [ $# -gt 0 ]; do case "$1" in -it|-i|-t) shift ;; -e) shift 2 ;; *) break ;; esac; done
    name="$1"; shift
    [ "$(cat "$st/$name" 2>/dev/null)" = running ] || { echo "Error response from daemon: container $id is not running" >&2; exit 1; }
    exec "$@" ;;
  stop) for f in "$st"/*; do echo exited > "$f"; done; echo "$2" ;;
  rm) rm -f "$st"/*; echo "$2" ;;
  *) echo "docker: unknown command: docker $1" >&2; exit 1 ;;
esac
"#;

/// What cell2's collector reports: the test container in `state`, labelled as the release build
/// labels it (this test binary is a debug build, whose C7 says `sparkbox.instance=dev`).
fn show_test_container(node: &Node, state: &str) {
    let mut v: Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    v["containers"] = json!([{ "id": BUSYBOX, "name": crate::ops::containers::TEST_CONTAINER, "image": "busybox:1.36", "state": state, "startedAt": 1,
        "finishedAt": 0, "exitCode": 0, "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "sleep 3600",
        "labels": { "sparkbox.test": "1", "sparkbox.instance": "release" } }]);
    *node.knobs.frame.lock().unwrap() = Some(v.to_string());
}

async fn wait_container_state(live: &Live, inv: &Inventory, state: Option<&str>) {
    until("the test container's state in the snapshot", 10, || {
        let snap = live.snapshot(inv);
        let c = snap.nodes[0].containers.iter().find(|c| c.name == crate::ops::containers::TEST_CONTAINER).map(|c| c.state.clone());
        c.as_deref() == state
    })
    .await;
}

#[tokio::test]
async fn step18_the_test_container_a_shell_in_it_and_its_removal_on_cell2() {
    use crate::ops::containers::{docker_command, test_container_command, TEST_CONTAINER};
    let app = temp("rehearsal-b18-app");
    let c = cluster(&app, &["cell2"], |_| Gb10::Healthy).await;
    let node = c.node("cell2");
    node.bin("docker", DOCKER_TEST);
    let ops = ops_for(&c.live, &c.inventory);
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    let terminals = Terminals::new(c.live.clone());
    let emit = || -> crate::term::Emit { Box::new(|_| {}) };
    let c7 = test_container_command(crate::identity::instance_name());

    // Under the all-nodes fence (no container-test) Run test container is refused, nothing sent.
    f.open(&all_fence());
    let e = ops.container_test_run(false, "cell2").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow test containers."));
    assert!(node.ran("docker").is_empty());

    // Under the cell2 fence: Containers › Run test container (C7) as the cluster user, container-test.
    f.open(&cell2_fence());
    assert_eq!(ops.container_test_run(false, "cell2").await.unwrap(), BUSYBOX);
    assert_eq!(node.ran("docker run"), std::slice::from_ref(&c7));
    let run = c.live.audit.entries().into_iter().find(|e| e.command == c7).expect("its row");
    assert_eq!(cols(&run), ("Run the test container on cell2".into(), "command".into(), c7.clone(), Some("container-test".into()), "ok".into()));
    assert!(!run.escalated && node.ran("sudo ").is_empty(), "as the cluster user, no sudo");
    show_test_container(node, "running");
    wait_container_state(&c.live, &c.inventory, Some("running")).await;

    // A shell in it (Containers › Shell): a PTY running `docker exec -it`, admitted as `shell`.
    let shell_cmd = crate::term::command_for(&Target::ContainerShell { container: TEST_CONTAINER.into() }).unwrap().unwrap();
    assert_eq!(shell_cmd, "docker exec -it -e TERM=xterm-256color sparkbox-test-busybox sh -c 'if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi'");
    let tab = terminals.open(false, "cell2", Target::ContainerShell { container: TEST_CONTAINER.into() }, 80, 24, emit()).await.unwrap();
    until("the container shell ran on cell2", 10, || !node.ran("docker exec -it").is_empty()).await;
    terminals.close(&tab);
    let admitted = c.live.admits().into_iter().find(|a| a.cmd == shell_cmd).expect("the exit was asked");
    assert_eq!((admitted.kind, admitted.verdict, admitted.admitted), (ExecKind::Pty, Verdict::Change(Shell), true));
    let row = c.live.audit.entries().into_iter().find(|e| e.kind == "terminal").expect("the tab's row");
    assert_eq!((row.title.as_str(), row.command.as_str(), row.class.as_str()), ("Open a shell in sparkbox-test-busybox on cell2", format!("ssh cell2 {shell_cmd}").as_str(), "session"));

    // Stop, then Remove: each refused under the all-nodes fence (no container-test) with nothing
    // sent, and admitted as container-test under the cell2 one. (A container shell is a `shell`
    // change, which the all-nodes fence allows too.)
    let refused_then_done = |action: &'static str| {
        let (ops, f, node) = (ops.clone(), &f, node);
        async move {
            f.open(&all_fence());
            let e = ops.container_action(false, "cell2", BUSYBOX, action, false).await.unwrap_err();
            assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow test containers."), "{action}");
            assert!(node.ran(&docker_command(action, BUSYBOX, false).unwrap()).is_empty(), "{action}: nothing sent");
            f.open(&cell2_fence());
            ops.container_action(false, "cell2", BUSYBOX, action, false).await.unwrap();
        }
    };
    refused_then_done("stop").await;
    show_test_container(node, "exited");
    wait_container_state(&c.live, &c.inventory, Some("exited")).await;
    refused_then_done("remove").await;
    for (action, title) in [("stop", "Stop sparkbox-test-busybox on cell2"), ("remove", "Remove sparkbox-test-busybox on cell2")] {
        let cmd = docker_command(action, BUSYBOX, false).unwrap();
        let row = c.live.audit.entries().into_iter().find(|e| e.command == cmd).expect("its row");
        assert_eq!(cols(&row), (title.into(), "command".into(), cmd.clone(), Some("container-test".into()), "ok".into()), "{action}");
    }
    assert_eq!(node.ran("docker"), [c7.clone(), shell_cmd, format!("docker stop {BUSYBOX}"), format!("docker rm {BUSYBOX}")]);
    assert!(std::fs::read_dir(node.state_dir.join("docker-test")).unwrap().next().is_none(), "nothing left");
    // The three refusals, newest first: one Refused row each, and each try's head line failed
    // with the fence's words; the admitted tries' head lines ok.
    let refused: Vec<String> = refusals(&c.live).into_iter().map(|e| e.command).collect();
    assert_eq!(refused, [docker_command("remove", BUSYBOX, false).unwrap(), docker_command("stop", BUSYBOX, false).unwrap(), c7]);
    for title in ["Run the test container on cell2", "Stop sparkbox-test-busybox on cell2", "Remove sparkbox-test-busybox on cell2"] {
        let heads: Vec<(String, Option<String>)> = c.live.audit.entries().into_iter().rev().filter(|e| e.kind == "op" && e.title == title).map(|e| (e.state, e.error)).collect();
        assert_eq!(heads, [("failed".to_string(), Some("The fence doesn't allow test containers.".to_string())), ("ok".to_string(), None)], "{title}");
    }
    c.live.stop();
}

// ---- Step 19: restart cell2 from its Node window

/// Plays cell2 restarting once `systemctl reboot` reaches it: sshd goes, every connection drops,
/// and 300 ms later it's back on boot `boot`.
fn restart_on(node: &Node, trigger: &'static str, boot: &'static str) -> tokio::task::JoinHandle<Instant> {
    let node = node.clone();
    tokio::spawn(async move {
        while node.ran(trigger).is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let at = Instant::now();
        let flag = node.state_dir.join("sshd-stopped");
        std::fs::write(&flag, "").unwrap();
        node.drop_connections().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        set_frame(&node, boot);
        std::fs::remove_file(&flag).unwrap();
        at
    })
}

#[tokio::test]
async fn step19_restart_cell2_from_its_node_window() {
    let app = temp("rehearsal-b19-app");
    let c = cluster(&app, &["cell2"], |_| Gb10::Healthy).await;
    let ops = ops_for(&c.live, &c.inventory);
    let said = Arc::new(Said::default());
    ops.set_planned(said.clone());
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    f.open(&cell2_fence());
    until("cell2 is on its first boot", 10, || c.live.snapshot(&c.inventory).nodes[0].boot_id.as_deref() == Some("cell2-boot-1")).await;

    let restart = restart_on(c.node("cell2"), "systemctl reboot", "cell2-boot-2");
    let id = ops.power(false, "cell2", "reboot", 0).unwrap();
    let t = wait_task(ops.tasks(), &id, 30).await;
    let sent = restart.await.unwrap();
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    // The checks first, each line in the step; then the planned work, the restart, the new boot.
    let steps: Vec<(&str, StepState)> = t.steps.iter().map(|s| (s.label.as_str(), s.state)).collect();
    assert_eq!(
        steps,
        [
            ("Checks before the restart", StepState::Ok),
            ("Telling the nodes' agents about the planned work", StepState::Ok),
            ("Sending the restart to cell2", StepState::Ok),
            ("Waiting for cell2 to go offline", StepState::Ok),
            ("Waiting for cell2 to come back (up to 5 min)", StepState::Ok),
            ("Checking that it restarted", StepState::Ok),
            ("Checks after the restart of cell2", StepState::Ok),
        ]
    );
    let lines: Vec<&str> = t.steps[0].detail.as_deref().unwrap().lines().collect();
    assert_eq!(lines.len(), 9);
    assert_eq!(lines[0], "Kernel that boots next: 7.0.0-1019-nvidia (where /boot/vmlinuz points)");
    assert_eq!(lines.iter().filter(|l| l.contains(" (recorded): ")).count(), 3, "{lines:?}");
    assert_eq!(t.steps[5].detail.as_deref(), Some("New boot cell2-bo"));
    assert!(t.summary.as_deref().unwrap().starts_with("cell2 is back after "), "{:?}", t.summary);
    assert_eq!(said.said(), ["begin cell2 restarting", "end cell2"]);
    assert_eq!(c.live.snapshot(&c.inventory).nodes[0].boot_id.as_deref(), Some("cell2-boot-2"));
    let _ = sent;

    // The rows: the head, the two checks (probe reads, no class), the lease claim, the restart
    // as power-one through sudo.
    let head = head_of(&c.live, &id);
    assert_eq!((head.title.as_str(), head.command.as_str(), head.state.as_str(), head.fence.as_deref()), ("Restart cell2", "systemctl reboot", "ok", None));
    let rows = rows_of(&c.live, &id);
    let script = prereboot::script(K);
    let shown: Vec<(String, String, Option<String>)> = rows.iter().map(|e| (e.kind.clone(), e.command.clone(), e.fence.clone())).collect();
    assert_eq!(shown[0], ("command".into(), prereboot::KERNEL.into(), None));
    assert_eq!(shown[1], ("command".into(), script.clone(), None));
    assert!(rows[..2].iter().all(|e| e.title == "Restart cell2" && e.class == "probe" && e.state == "ok"), "{rows:#?}");
    assert!(lease_claim(&rows[2]), "{:#?}", rows[2]);
    let sudo = &rows[3];
    assert_eq!((sudo.title.as_str(), sudo.kind.as_str(), sudo.command.as_str(), sudo.fence.as_deref(), sudo.escalated), ("Restart cell2", "sudo", "systemctl reboot", Some("power-one"), true));
    assert!(matches!(sudo.state.as_str(), "ok" | "failed"), "ok, or failed with the connection: {sudo:#?}");
    assert_eq!(rows.len(), 5, "{rows:#?}");
    assert_eq!((rows[4].command.as_str(), rows[4].class.as_str(), rows[4].fence.as_deref()), (crate::ops::postboot::READ, "read", None));
    let node = c.node("cell2");
    assert_eq!(node.ran("sudo -k -S -p '' systemctl reboot").len(), 1);
    assert!(refusals(&c.live).is_empty());
}

#[tokio::test]
async fn step19_a_missing_initrd_is_refused_with_nothing_sent() {
    let app = temp("rehearsal-b19x-app");
    let c = cluster(&app, &["cell2"], |_| Gb10::NoInitrd).await;
    let ops = ops_for(&c.live, &c.inventory);
    let said = Arc::new(Said::default());
    ops.set_planned(said.clone());
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    f.open(&cell2_fence());

    let id = ops.power(false, "cell2", "reboot", 0).unwrap();
    let t = wait_task(ops.tasks(), &id, 20).await;
    let why = "No initrd for 7.0.0-1019-nvidia: the September 2026 panic. Not restarted.";
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(why)));
    assert_eq!(t.steps.iter().map(|s| (s.label.as_str(), s.state)).collect::<Vec<_>>(), [("Checks before the restart", StepState::Failed)]);
    // Nothing sent, no window, not shown restarting, and no lease claimed (nothing changed).
    let node = c.node("cell2");
    assert!(node.ran("sudo ").is_empty() && node.ran("lease claim").is_empty(), "{:?}", node.ran(""));
    assert!(said.said().is_empty());
    assert_eq!(c.live.snapshot(&c.inventory).nodes[0].power_state, None);
    let head = head_of(&c.live, &id);
    assert_eq!((head.title.as_str(), head.state.as_str(), head.error.as_deref()), ("Restart cell2", "failed", Some(why)));
    let rows = rows_of(&c.live, &id);
    assert_eq!(rows.iter().map(|e| e.command.as_str()).collect::<Vec<_>>(), [prereboot::KERNEL, prereboot::script(K).as_str()]);
    assert!(refusals(&c.live).is_empty(), "SparkBox's own refusal, not the exit's");
}

// ---- Step 19a: a restart outside SparkBox

#[tokio::test]
async fn step19a_a_restart_outside_sparkbox_is_one_node_rebooted() {
    // The shell the owner types `sudo systemctl reboot` into: admitted as `shell` under the cell2
    // preset (typing it isn't rehearsed: keystrokes are never logged, and a fake doesn't reboot).
    let app = temp("rehearsal-b19a-app");
    let c = cluster(&app, &["cell2"], |_| Gb10::Healthy).await;
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    let terminals = Terminals::new(c.live.clone());
    let emit = || -> crate::term::Emit { Box::new(|_| {}) };
    f.open(&Spec { classes: vec![PowerOne, AptRefresh, AptOne, AptHold], ..cell2_fence() });
    let e = SbError::from(terminals.open(false, "cell2", Target::Shell, 80, 24, emit()).await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow shells."));
    f.open(&cell2_fence());
    let tab = terminals.open(false, "cell2", Target::Shell, 80, 24, emit()).await.unwrap();
    terminals.close(&tab);

    // cell2 comes back on a new boot with no window SparkBox opened and no power state: one
    // node-rebooted, a warning, "Restarted on its own at …". (Its own node id: the boots record
    // is process-wide.)
    let id = "rehearsal-b19a/cell2";
    let node = |boot: &str, up_s: u64| NodeSnapshot {
        id: id.into(),
        name: "cell2".into(),
        status: LinkStatus::Online,
        boot_id: Some(boot.into()),
        system: SystemInfo { uptime_s: up_s, ..Default::default() },
        ..Default::default()
    };
    let snap = |ts_ms: u64, n: NodeSnapshot| ClusterSnapshot { ts_ms, demo: false, nodes: vec![n], ..Default::default() };
    let mut e = Engine::default();
    let t0 = now_ms() - 600_000;
    let mut events = Vec::new();
    for i in 0..3 {
        let s = snap(t0 + i * 1000, node("9cabf4f7-e39d-4c62-b9d6-20e269501402", 360_000 + i));
        boots::observe_at(&s, s.ts_ms);
        events.extend(e.step(&s, s.ts_ms));
    }
    for i in 0..3 {
        let s = snap(t0 + 120_000 + i * 1000, node("bbc047e4-5b56-4405-bb6d-b33df16b2dcb", 30 + i));
        boots::observe_at(&s, s.ts_ms);
        events.extend(e.step(&s, s.ts_ms));
    }
    let raised: Vec<_> = events.iter().filter(|ev| ev.alert.rule == boots::RULE && ev.kind == EventKind::Raised).collect();
    assert_eq!(raised.len(), 1, "{events:?}");
    let start = t0 + 120_000 - 30_000;
    assert_eq!((raised[0].alert.severity, raised[0].alert.message.clone()), (Severity::Warn, format!("Restarted on its own at {}", boots::clock(start))));
    assert_eq!(raised[0].alert.summary, format!("cell2 restarted on its own at {}", boots::clock(start)));
    assert!(e.view(t0 + 122_000, None).alerts.iter().any(|a| a.rule == boots::RULE && a.state == AlertState::Firing));

    // How the boot before ended, read from the node's journal as the cluster user: the Oct 29
    // fixture of a boot that ended with a clean shutdown (cell3's 17:45 boot).
    let fixture = |file: &str| std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/live/fixtures/boots/cell3").join(file)).unwrap();
    let before = "bbc047e45b564405bb6db33df16b2dcb";
    let list = fixture("list-boots.json");
    let ended = ["warning", "pid1", "journald"].iter().map(|p| fixture(&format!("{before}.{p}.jsonl"))).collect::<Vec<_>>().join("@@sparkbox-boot@@\n");
    let journal: Script = Arc::new(move |cmd: &str, _: &[u8]| {
        if cmd == boots::LIST_BOOTS {
            return Some((list.clone(), 0));
        }
        (cmd == boots::boot_command("-1")).then(|| (ended.clone(), 0))
    });
    std::fs::create_dir_all(app.join("home-journal")).unwrap();
    let (jnode, port) = fake_node(&app.join("home-journal"), journal).await;
    let (jlive, _) = fake_node::connect(&app.join("journal-app"), &[("cell2", port)]).await;
    let read = boots::read_journal(&jlive, "cell2", boots::Mode::Changed, &Default::default(), now_ms()).await;
    assert_eq!(read.error, None);
    assert_eq!(read.ended.iter().map(|(b, end, _)| (b.as_str(), *end)).collect::<Vec<_>>(), [(before, boots::Ended::Clean)]);
    let rows: Vec<_> = jlive.audit.entries().into_iter().filter(|e| e.command.starts_with("journalctl ")).collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|e| e.class == "probe" && e.fence.is_none() && e.kind == "command"), "{rows:#?}");
    assert!(jnode.ran("sudo").is_empty());
}

// ---- Step 20: the badge without Updates, then Check all nodes

#[tokio::test]
async fn step20_the_badge_without_updates_then_check_all_nodes() {
    let app = temp("rehearsal-b20-app");
    let c = cluster(&app, &FOUR, |_| Gb10::Healthy).await;
    for node in c.nodes.values() {
        wrap_apt_get(node, APT_GET_WARNS);
    }
    let ops = ops_for(&c.live, &c.inventory);
    let cache = Arc::new(UpdateCache::new(None));
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());

    // The background check, no window open: each node checked once, reads only.
    let mut checked = cache.tick(&ops, false).await;
    checked.sort();
    assert_eq!(checked, FOUR);
    let mut snap = c.live.snapshot(&c.inventory);
    cache.overlay_onto(&mut snap);
    let counts: Vec<(&str, u32, u32)> = snap.nodes.iter().map(|n| (n.id.as_str(), n.updates.as_ref().map_or(0, |u| u.pending), n.updates.as_ref().map_or(0, |u| u.security))).collect();
    assert_eq!(counts, [("cell1", 4, 3), ("cell2", 2, 1), ("cell3", 2, 1), ("gx10-0000", 2, 1)]);
    let firmware: Vec<Option<u32>> = snap.nodes.iter().map(|n| n.updates.as_ref().and_then(|u| u.firmware)).collect();
    assert_eq!(firmware, [Some(0), Some(0), Some(0), None], "none, and gx10-0000 needs sudo");
    assert_eq!(cache.summary("gx10-0000").and_then(|s| s.firmware).map(|f| f.state), Some(update_check::FirmwareState::NeedsSudo));
    assert!(snap.nodes.iter().all(|n| n.updates.as_ref().is_some_and(|u| !u.ollama_newer)), "no Ollama on these fakes");
    let reads: Vec<_> = c.live.audit.entries().into_iter().filter(|e| e.command == update_check::CHECK_SCRIPT).collect();
    assert_eq!(reads.len(), 4);
    assert!(c.nodes.values().all(|n| ran(n, update_check::CHECK_SCRIPT) == 1));
    assert!(reads.iter().all(|e| e.kind == "command" && e.fence.is_none() && e.class == "probe" && !e.escalated), "{reads:#?}");
    assert!(c.nodes.values().all(|n| n.ran("sudo ").is_empty()), "the background check never uses sudo");

    // Check all nodes under the cell2 preset: cell2 refreshed, the three others refused.
    f.open(&cell2_fence());
    let id = cache.refresh_all(&ops, FOUR.map(String::from).to_vec()).unwrap();
    let t = wait_task(ops.tasks(), &id, 30).await;
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("Couldn't refresh the package lists on cell1, cell3, gx10-0000")));
    let refused: Vec<(String, String, Option<String>)> = refusals(&c.live).into_iter().map(|e| (e.node, e.command, e.error)).collect();
    let mut refreshes: Vec<&str> = refused.iter().filter(|(_, cmd, _)| cmd == update_check::REFRESH).map(|(n, _, _)| n.as_str()).collect();
    refreshes.sort();
    assert_eq!(refreshes, ["cell1", "cell3", "gx10-0000"]);
    // And gx10-0000's firmware read with sudo (a sudo check): not in this fence either.
    assert_eq!(refused.len(), 4, "{refused:?}");
    assert!(refused.iter().any(|(n, cmd, _)| n == "gx10-0000" && cmd == update_check::FWUPD_SUDO), "{refused:?}");
    assert!(refused.iter().all(|(n, _, err)| err.as_deref() == Some(format!("{n} isn't in the open fence.").as_str())), "{refused:?}");
    assert!(["cell1", "cell3", "gx10-0000"].iter().all(|n| c.node(n).ran("apt-get").is_empty()));
    assert_eq!(c.node("cell2").ran("sudo -k -S -p '' apt-get -o DPkg::Lock::Timeout=300 update").len(), 1);

    // Under the all-nodes preset: four sudo rows as apt-refresh, at once, the W: line kept per node.
    // Each node's `apt-get update` waits until all four have started, so one after another can't
    // pass, and on a busy machine a quick apt can't end before the last one is under way.
    let gate = temp("rehearsal-b-step20-gate");
    for n in FOUR {
        let node = c.node(n);
        std::fs::rename(node.bin_dir.join("apt-get"), node.bin_dir.join("apt-get.next")).unwrap();
        node.bin("apt-get", &format!("#!/bin/dash\ncase \" $* \" in *\" update \"*)\n  : > '{g}/{n}'\n  i=0; while [ \"$(ls '{g}' | wc -l)\" -lt 4 ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done ;;\nesac\nexec \"$(dirname \"$0\")/apt-get.next\" \"$@\"\n", g = gate.display()));
    }
    f.open(&all_fence());
    let id = cache.refresh_all(&ops, FOUR.map(String::from).to_vec()).unwrap();
    let t = wait_task(ops.tasks(), &id, 30).await;
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    assert_eq!(t.summary.as_deref(), Some("Package lists are up to date"));
    assert!(t.warning.as_deref().is_some_and(|w| w.starts_with("apt printed warnings on ")), "{:?}", t.warning);
    let sudo: Vec<_> = rows_of(&c.live, &id).into_iter().filter(|e| e.kind == "sudo").collect();
    let mut on: Vec<&str> = sudo.iter().map(|e| e.node.as_str()).collect();
    on.sort();
    assert_eq!(on, FOUR);
    assert!(
        sudo.iter().all(|e| (e.title.as_str(), e.command.as_str(), e.fence.as_deref(), e.state.as_str()) == ("Check for updates on 4 nodes", "apt-get -o DPkg::Lock::Timeout=300 update", Some("apt-refresh"), "ok")),
        "{sudo:#?}"
    );
    // At once: each started before any of the others ended.
    let (first_end, last_start) = (sudo.iter().map(|e| e.ts_ms + e.ms).min().unwrap(), sudo.iter().map(|e| e.ts_ms).max().unwrap());
    assert!(last_start <= first_end, "the four ran together");
    // Then each node checked again, as reads.
    let rechecks: Vec<_> = rows_of(&c.live, &id).into_iter().filter(|e| e.command == update_check::CHECK_SCRIPT).collect();
    assert_eq!(rechecks.len(), 4);
    assert!(rechecks.iter().all(|e| e.kind == "command" && e.fence.is_none() && e.title == "Check for updates on 4 nodes"), "{rechecks:#?}");
    // gx10-0000's firmware read with sudo is a sudo check, which this preset doesn't have either:
    // refused, and its firmware still says it needs sudo, and why.
    let fw = refusals(&c.live).into_iter().find(|e| e.command == update_check::FWUPD_SUDO && e.op_id.as_deref() == Some(id.as_str())).expect("the sudo firmware read, refused");
    assert_eq!((fw.node.as_str(), fw.kind.as_str()), ("gx10-0000", "op"));
    // The task ended with a warning; the Refused row still says why it was refused (playbook
    // Findings 12, fixed at integration: `audit::Log::end_heads` leaves Refused rows alone).
    assert_eq!((fw.state.as_str(), fw.error.as_deref()), ("failed", Some("The fence doesn't allow sudo checks.")));
    let firmware = cache.summary("gx10-0000").and_then(|s| s.firmware).unwrap();
    assert_eq!((firmware.state, firmware.detail.as_deref()), (update_check::FirmwareState::NeedsSudo, Some("Couldn't read it with sudo: The fence doesn't allow sudo checks.")));
    assert!(t.log.iter().any(|l| l == "gx10-0000: firmware: Couldn't read it with sudo: The fence doesn't allow sudo checks."), "{:?}", t.log);
    assert!(c.node("gx10-0000").ran("fwupdmgr get-updates --json --no-unreported-check --no-metadata-check").iter().all(|c| !c.starts_with("sudo ")), "no sudo fwupd reached it");
    let w = "W: https://dl.google.com/linux/chrome/deb/dists/stable/InRelease: Key is stored in legacy trusted.gpg keyring (/etc/apt/trusted.gpg), see the DEPRECATION section in apt-key(8) for details.";
    for n in FOUR {
        assert!(t.log.iter().any(|l| l == &format!("{n}: {w}")), "{n}: {:?}", t.log);
        let refresh = cache.summary(n).and_then(|s| s.refresh).expect("the refresh record");
        assert_eq!((refresh.ok, refresh.warnings.clone()), (true, vec![w.to_string()]), "{n}");
    }
}

// ---- Step 21: the dry run and the upgrade of libbrotli1 on cell2

/// The C1′ start command for unit `sparkbox-upgrade-<id>`, log folder `dir`, as the contract
/// writes it: every package pinned to the version the dry run showed (`name=version`), and
/// `--no-remove` (with `limit`, the download limit right after --force-confold).
fn c1(dir: &str, id: &str, pins: &[&str], limit: Option<u32>) -> String {
    let dl = limit.map(|k| format!(" -o Acquire::http::Dl-Limit={k} -o Acquire::https::Dl-Limit={k}")).unwrap_or_default();
    let pinned = pins.iter().map(|p| format!("'\\''{p}'\\''")).collect::<Vec<_>>().join(" ");
    format!(
        "mkdir -p '{dir}' && chmod 0755 '{dir}' && dpkg-query -W > '{dir}/dpkg-before-{id}.txt' && systemd-run --unit sparkbox-upgrade-{id} --collect -p StandardOutput=append:{dir}/upgrade-{id}.log -p StandardError=append:{dir}/upgrade-{id}.log /bin/sh -c 'trap : TERM; env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold{dl} -y --no-remove install --only-upgrade {pinned}; code=$?; echo SPARKBOX-AFTER; apt-get check; needrestart -b -r l; echo SPARKBOX-AFTER-END; echo \"SPARKBOX-EXIT $code\"; exit $code'"
    )
}

/// The C2 discovery dry run (unchanged: names, or dist-upgrade for everything).
fn c2(packages: &[&str]) -> String {
    let verb = if packages.is_empty() { "dist-upgrade".to_string() } else { format!("install --only-upgrade {}", packages.iter().map(|p| format!("'{p}'")).collect::<Vec<_>>().join(" ")) };
    format!("apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y {verb}")
}

/// C2′: the dry run of exactly the pinned command, run just before the start.
fn c2p(pins: &[&str], limit: Option<u32>) -> String {
    let dl = limit.map(|k| format!(" -o Acquire::http::Dl-Limit={k} -o Acquire::https::Dl-Limit={k}")).unwrap_or_default();
    let pinned = pins.iter().map(|p| format!("'{p}'")).collect::<Vec<_>>().join(" ");
    format!("apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold{dl} -y --no-remove install --only-upgrade {pinned}")
}

/// The upgrade the Updates window starts after showing `sim`: bound to the plan it showed.
fn bound(sim: &updates::SimulatedUpgrade, limit: Option<u32>, security_only: bool) -> UpgradeArgs {
    UpgradeArgs { dl_limit_kbps: limit, security_only, plan: Some(sim.shown()) }
}

#[tokio::test]
async fn step21_dry_run_and_upgrade_libbrotli1_on_cell2() {
    let app = temp("rehearsal-b21-app");
    let c = cluster(&app, &["cell2"], |_| Gb10::Healthy).await;
    let ops = ops_for(&c.live, &c.inventory);
    let said = Arc::new(Said::default());
    ops.set_planned(said.clone());
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    f.open(&cell2_fence());
    let node = c.node("cell2");

    // The dry run (C2, a read): the plan it will bind to, and the pinned command it will run (C1′).
    let brotli = ["libbrotli1".to_string()];
    let sim = ops.updates_simulate(false, "cell2", &brotli).await.unwrap();
    assert_eq!(sim.command, "env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y --no-remove install --only-upgrade 'libbrotli1=2.0-1'");
    assert_eq!(sim.dry_run, c2(&["libbrotli1"]));
    assert_eq!(sim.plan, [updates::PlannedPkg { name: "libbrotli1".into(), from: Some("1.0-1".into()), to: "2.0-1".into(), new: false }]);
    assert_eq!((sim.upgrades.clone(), sim.removes.clone(), sim.refused.clone()), (vec!["libbrotli1 1.0-1 → 2.0-1".to_string()], Vec::new(), None));
    assert_eq!(node.ran(&c2(&["libbrotli1"])).len(), 1, "it ran as written, without sudo");
    let dry = c.live.audit.entries().into_iter().find(|e| e.command == c2(&["libbrotli1"])).unwrap();
    assert_eq!((dry.kind.as_str(), dry.fence.as_deref(), dry.escalated), ("command", None, false));

    // Upgrade with no plan to bind to (the window always sends the one it showed): refused.
    let e = ops.updates_upgrade_opts(false, "cell2".into(), brotli.to_vec(), UpgradeArgs::default()).unwrap_err();
    assert_eq!(e.message, updates::DRY_RUN_FIRST);

    // The upgrade, bound to the plan shown.
    let id = ops.updates_upgrade_opts(false, "cell2".into(), brotli.to_vec(), bound(&sim, None, false)).unwrap();
    let t = wait_task(ops.tasks(), &id, 60).await;
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
    let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
    let dir = c.live.upgrade_log_dir();
    let unit_id = run.unit.strip_prefix("sparkbox-upgrade-").unwrap().to_string();
    let head = head_of(&c.live, &id);
    assert_eq!((head.title.as_str(), head.command.as_str(), head.state.as_str()), ("Upgrade libbrotli1 on cell2", sim.command.as_str(), "ok"));
    let rows = rows_of(&c.live, &id);
    let start = rows.iter().find(|e| e.kind == "sudo").expect("the start");
    assert_eq!(cols(start), ("Upgrade libbrotli1 on cell2".into(), "sudo".into(), c1(&dir, &unit_id, &["libbrotli1=2.0-1"], None), Some("apt-one".into()), "ok".into()));
    assert_eq!(rows.iter().filter(|e| e.kind == "sudo").count(), 1, "one sudo command: the post-checks run inside the unit");
    // C2′ first: the dry run of exactly the pinned command, as a read, compared with the plan shown.
    let own = rows.iter().find(|e| e.command == c2p(&["libbrotli1=2.0-1"], None)).expect("the upgrade's own dry run");
    assert_eq!((own.title.as_str(), own.kind.as_str(), own.fence.as_deref()), ("Upgrade libbrotli1 on cell2", "command", None));
    assert!(rows.iter().position(|e| e.command == own.command) < rows.iter().position(|e| e.kind == "sudo"));
    // Before it, which of the plan's packages are installed (those are pinned), a read.
    let installed = rows.iter().position(|e| e.command == "dpkg-query -W -f='${Package}\\n'" && e.kind == "command" && e.fence.is_none());
    assert!(installed.is_some() && installed < rows.iter().position(|e| e.command == own.command), "{rows:#?}");
    // The package list from before, read back and filed in the evidence folder.
    let before = format!("{dir}/dpkg-before-{unit_id}.txt");
    assert_eq!(run.dpkg_before.as_deref(), Some(before.as_str()));
    let copy = run.evidence.clone().expect("the evidence copy");
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), std::fs::read_to_string(&before).unwrap());
    assert!(rows.iter().any(|e| e.command == format!("cat '{before}'") && e.kind == "command" && e.fence.is_none()), "{rows:#?}");
    assert!(rows.iter().any(|e| e.command == "dpkg --audit" && e.kind == "command" && e.fence.is_none()), "{rows:#?}");
    let follow = format!("tail -n +1 -F '{}' 2>/dev/null", run.log_path);
    assert!(rows.iter().any(|e| e.kind == "stream" && e.command == follow && e.fence.is_none()), "the log followed: {rows:#?}");
    // needrestart's list, from the unit's log: none on cell2.
    assert_eq!(run.needrestart.as_ref().map(|n| n.services.clone()), Some(Vec::new()));
    // The window opened before the unit and closed after the checks: nothing pages.
    assert_eq!(said.said().first().map(String::as_str), Some("begin cell2 upgrading"));
    assert_eq!(said.said().last().map(String::as_str), Some("end cell2"));
    assert!(refusals(&c.live).is_empty());

    // Without apt-one the start is refused, before any sudo reaches the node.
    f.open(&Spec { classes: vec![PowerOne, AptRefresh, AptHold, Shell], ..cell2_fence() });
    let sudo_before = node.ran("sudo ").len();
    let id = ops.updates_upgrade_opts(false, "cell2".into(), brotli.to_vec(), bound(&sim, None, false)).unwrap();
    let t = wait_task(ops.tasks(), &id, 30).await;
    assert_eq!(t.state, TaskState::Failed);
    assert!(t.error.as_deref().unwrap().contains("The fence doesn't allow"), "{:?}", t.error);
    assert_eq!(node.ran("sudo ").len(), sudo_before);
}

// ---- Step 22: the reattach, with the download limit

#[tokio::test]
async fn step22_reattach_with_the_download_limit() {
    let app = temp("rehearsal-b22-app");
    let c = cluster(&app, &["cell2"], |_| Gb10::Healthy).await;
    // apt stays in its download until this goes: the quit lands there however busy the machine is.
    let downloading = c.node("cell2").state_dir.join("apt/hold-download");
    std::fs::write(&downloading, "").unwrap();
    let ops = ops_for(&c.live, &c.inventory);
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    f.open(&cell2_fence());

    let chrome = vec!["google-chrome-stable".to_string()];
    let sim = ops.updates_simulate_opts(false, "cell2", &chrome, &updates::UpgradeOpts { dl_limit_kbps: Some(500), ..Default::default() }).await.unwrap();
    let id = ops.updates_upgrade_opts(false, "cell2".into(), chrome.clone(), bound(&sim, Some(500), false)).unwrap();
    until("downloading", 60, || ops.runs().iter().any(|r| r.task_id == id && r.phase == "downloading")).await;
    let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
    assert_eq!(run.dl_limit_kbps, Some(500));
    let unit_id = run.unit.strip_prefix("sparkbox-upgrade-").unwrap().to_string();
    let start = rows_of(&c.live, &id).into_iter().find(|e| e.kind == "sudo").unwrap();
    assert_eq!((start.command.clone(), start.fence.as_deref()), (c1(&c.live.upgrade_log_dir(), &unit_id, &["google-chrome-stable=2.0-1"], Some(500)), Some("apt-one")));
    assert!(c.node("cell2").ran(&c2(&["google-chrome-stable"]).replace(" -y ", " -o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500 -y ")).len() == 1, "the dry run carries the limit too");
    assert_eq!(c.node("cell2").ran(&c2p(&["google-chrome-stable=2.0-1"], Some(500))).len(), 1, "and C2′ just before the start");

    // SparkBox quits mid-download: every connection goes, apt goes on in its unit.
    c.live.stop();
    // A relaunch on the same data folder picks it up again and follows it to its end.
    let (live2, inventory2) = c.relaunch(&app).await;
    let ops2 = ops_for(&live2, &inventory2);
    live2.enforce_writes(f.rules());
    ops2.reattach_upgrades();
    let mut task = None;
    until("the picked-up task", 20, || {
        task = ops2.tasks().list().into_iter().find(|t| t.title == "Upgrade google-chrome-stable on cell2 (picked up again)");
        task.is_some()
    })
    .await;
    std::fs::remove_file(&downloading).unwrap();
    let t = wait_task(ops2.tasks(), &task.unwrap().id, 60).await;
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
    assert_eq!(std::fs::read_to_string(c.node("cell2").state_dir.join("apt/upgraded")).unwrap(), "google-chrome-stable\n");
    // Following it needs no sudo and no fence class: reads of its log and its unit.
    assert!(live2.audit.entries().iter().all(|e| e.kind != "sudo" && e.fence.is_none()), "{:#?}", live2.audit.entries());
    let follow = format!("tail -n +1 -F '{}' 2>/dev/null", run.log_path);
    let tail = live2.audit.entries().into_iter().find(|e| e.kind == "stream" && e.command == follow).expect("the log followed again");
    // The picked-up task follows the unit inside its own operation: its rows carry the task's id
    // and title (playbook Findings 13, fixed at integration).
    assert_eq!((tail.op_id.as_deref(), tail.title.as_str()), (Some(t.id.as_str()), "Upgrade google-chrome-stable on cell2 (picked up again)"));
    assert_eq!(c.node("cell2").ran("systemd-run --unit").len(), 1, "started once");
    live2.stop();
}

// ---- Step 23: SparkBox notices an upgrade made in a terminal on cell3

#[tokio::test]
async fn step23_sparkbox_notices_an_upgrade_made_in_a_terminal() {
    let app = temp("rehearsal-b23-app");
    let c = cluster(&app, &FOUR, |_| Gb10::Healthy).await;
    let ops = ops_for(&c.live, &c.inventory);
    let cache = Arc::new(UpdateCache::new(None));
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    f.open(&all_fence());

    // The owner's shell on cell3: admitted as `shell` under the all-nodes preset; under the cell2
    // one, cell3 isn't in the fence.
    let terminals = Terminals::new(c.live.clone());
    let emit = || -> crate::term::Emit { Box::new(|_| {}) };
    let tab = terminals.open(false, "cell3", Target::Shell, 80, 24, emit()).await.unwrap();
    terminals.close(&tab);
    f.open(&cell2_fence());
    let e = SbError::from(terminals.open(false, "cell3", Target::Shell, 80, 24, emit()).await.unwrap_err());
    assert_eq!(e.message, "cell3 isn't in the open fence.");
    f.open(&all_fence());

    // Checked once; cell3's security alert raised (information) on the rules' clock.
    let t0 = now_ms();
    cache.tick(&ops, false).await;
    let mut e = Engine::default();
    let step = |e: &mut Engine, ts: u64| {
        let mut snap = c.live.snapshot(&c.inventory);
        snap.ts_ms = ts;
        cache.overlay_onto(&mut snap);
        (e.step(&snap, ts), snap)
    };
    let (_, snap) = step(&mut e, t0);
    let alert = |e: &Engine, snap: &ClusterSnapshot, node: &str| e.view(snap.ts_ms, Some(snap)).alerts.into_iter().find(|a| a.rule == "security-updates-pending" && a.entity.id == node && a.state == AlertState::Firing);
    let a = alert(&e, &snap, "cell3").expect("cell3's security alert");
    assert_eq!((a.severity, a.message.as_str()), (Severity::Info, "1 security update pending"));

    // What `sudo apt-get install --only-upgrade libbrotli1` did on cell3: libbrotli1 is no longer
    // upgradable and dpkg's status file moved.
    {
        let mut apt = c.apt["cell3"].lock().unwrap();
        apt.upgradable.retain(|l| !l.starts_with("libbrotli1/"));
        apt.dpkg_s += 60;
    }
    // The next round: every node's stamps; only cell3's moved, so only cell3 is checked again.
    let checks_before = c.nodes.iter().map(|(n, node)| (n.clone(), ran(node, update_check::CHECK_SCRIPT))).collect::<BTreeMap<_, _>>();
    let changed = cache.tick(&ops, false).await;
    assert_eq!(changed, ["cell3"]);
    for (n, node) in &c.nodes {
        assert_eq!(ran(node, update_check::STAMPS), 1, "{n}: one stamps read");
        assert_eq!(ran(node, update_check::CHECK_SCRIPT), checks_before[n] + usize::from(n == "cell3"), "{n}");
    }
    assert_eq!(cache.summary("cell3").unwrap().security_since_ms, None);
    // The count drops and the alert clears at the next evaluation (a minute later on the rules' clock).
    let (events, snap) = step(&mut e, t0 + 60_000);
    let facts = snap.nodes.iter().find(|n| n.id == "cell3").unwrap().updates.clone().unwrap();
    assert_eq!((facts.pending, facts.security), (1, 0));
    assert!(events.iter().any(|ev| ev.kind == EventKind::Resolved && ev.alert.rule == "security-updates-pending" && ev.alert.entity.id == "cell3"), "{events:?}");
    assert!(alert(&e, &snap, "cell3").is_none());
    assert!(alert(&e, &snap, "cell2").is_some(), "the others still have theirs");
    // SparkBox itself changed nothing: no sudo, no change row.
    assert!(c.nodes.values().all(|n| n.ran("sudo ").is_empty()));
    assert!(c.live.audit.entries().iter().all(|e| e.kind != "sudo"));
}

// ---- Step 24: pin the GPU stack on cell2; hold and release google-chrome-stable

/// What dpkg-query lists on the fake cell2: a mix of the GPU stack and the rest, sorted as
/// dpkg-query prints them.
const INSTALLED: [&str; 14] = [
    "containerd.io",
    "cuda-toolkit-13-0",
    "dgx-spark-ota-update-meta",
    "docker-ce",
    "fwupd",
    "google-chrome-stable",
    "libbrotli1",
    "libnvidia-compute-580",
    "linux-image-7.0.0-1019-nvidia",
    "linux-modules-nvidia-580-open-7.0.0-1019-nvidia",
    "nvidia-container-toolkit",
    "nvidia-driver-580-open",
    "nvidia-spark-grub-kho",
    "openssl",
];

/// PLAN P1.27's pattern, applied to `INSTALLED` by hand: what the preset must hold.
const STACK: [&str; 10] = [
    "containerd.io",
    "cuda-toolkit-13-0",
    "dgx-spark-ota-update-meta",
    "docker-ce",
    "libnvidia-compute-580",
    "linux-image-7.0.0-1019-nvidia",
    "linux-modules-nvidia-580-open-7.0.0-1019-nvidia",
    "nvidia-container-toolkit",
    "nvidia-driver-580-open",
    "nvidia-spark-grub-kho",
];

#[tokio::test]
async fn step24_pin_the_gpu_stack_on_cell2() {
    let app = temp("rehearsal-b24-app");
    // Its checks before a restart answer as after the pin: the prebuilt modules and
    // nvidia-spark-grub-kho on hold (`hi`).
    let c = cluster(&app, &["cell2"], |_| Gb10::Pinned).await;
    let node = c.node("cell2");
    let apt = node.state_dir.join("apt");
    std::fs::write(apt.join("installed"), INSTALLED.iter().map(|p| format!("{p}\t1.0-1\n")).collect::<String>()).unwrap();
    // Pending: two routine fixes and three of the stack.
    std::fs::write(apt.join("pending"), "libbrotli1\ngoogle-chrome-stable\nnvidia-driver-580-open\nlibnvidia-compute-580\nlinux-image-7.0.0-1019-nvidia\n").unwrap();
    wrap_apt_get(node, APT_GET_HOLDS);
    let ops = ops_for(&c.live, &c.inventory);
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    f.open(&cell2_fence());
    let showhold = || std::fs::read_to_string(apt.join("holds")).unwrap_or_default().lines().map(String::from).collect::<Vec<_>>();

    // Before the pin, a dry run of everything is refused: everything needs the GPU stack pinned
    // first (said before the guard's verdict), naming what isn't held.
    let sim = ops.updates_simulate(false, "cell2", &[]).await.unwrap();
    assert_eq!(sim.dry_run, c2(&[]));
    let unheld = format!("Upgrade everything needs the GPU stack pinned first: {} and {} more aren't held on cell2", STACK[..6].join(", "), STACK.len() - 6);
    assert_eq!(sim.refused.as_deref(), Some(unheld.as_str()));

    // Pin the GPU stack: exactly PLAN's pattern of what's installed, in one apt-hold sudo row.
    let pinned = ops.updates_hold_preset(false, "cell2", true, false).await.unwrap();
    assert_eq!(pinned.packages, STACK);
    let id = pinned.task_id.unwrap();
    let t = wait_task(ops.tasks(), &id, 20).await;
    assert_eq!((t.state, t.title.as_str(), t.summary.as_deref()), (TaskState::Ok, "Pin the GPU stack on cell2", Some("Pinned 10 GPU-stack packages on cell2")));
    let hold = format!("apt-mark hold {}", STACK.iter().map(|p| format!("'{p}'")).collect::<Vec<_>>().join(" "));
    let rows = rows_of(&c.live, &id);
    let read = rows.iter().find(|e| e.command == "dpkg-query -W -f='${Package}\\n'").expect("the preset's read");
    assert_eq!((read.kind.as_str(), read.fence.as_deref()), ("command", None));
    let sudo: Vec<_> = rows.iter().filter(|e| e.kind == "sudo").collect();
    assert_eq!(sudo.iter().map(|e| cols(e)).collect::<Vec<_>>(), [("Pin the GPU stack on cell2".into(), "sudo".into(), hold, Some("apt-hold".into()), "ok".into())]);
    assert_eq!(showhold(), STACK, "showhold lists the stack, nothing else (fwupd is guarded, not held)");
    assert!(t.log.iter().any(|l| l.contains("The list is also in the evidence folder")), "{:?}", t.log);

    // A dry run of everything now keeps the stack back and has nothing sensitive: it may run.
    let sim = ops.updates_simulate(false, "cell2", &[]).await.unwrap();
    assert_eq!(sim.upgrades, ["libbrotli1 1.0-1 → 2.0-1", "google-chrome-stable 1.0-1 → 2.0-1"]);
    assert!(sim.sensitive.is_empty() && sim.refused.is_none(), "{:?} {:?}", sim.sensitive, sim.refused);
    assert_eq!(sim.held_back, ["nvidia-driver-580-open", "libnvidia-compute-580", "linux-image-7.0.0-1019-nvidia"]);
    // The checks before the next restart still count what's held: the modules and kho lines
    // read installed, and nothing blocks.
    let r = prereboot::check(&c.live, "cell2").await.unwrap();
    assert!(r.ok, "{:?}", r.blocking);
    let detail = |id: &str| r.checks.iter().find(|c| c.id == id).unwrap().detail.clone();
    assert!(detail("modules").starts_with(&format!("linux-modules-nvidia-580-open-{K} 7.0.0-1019.19+1")), "{}", detail("modules"));
    assert!(detail("kho").ends_with("nvidia-spark-grub-kho 1.0.0 is installed"), "{}", detail("kho"));

    // Hold, then release, google-chrome-stable: one apt-hold row each, showhold following.
    let chrome = ["google-chrome-stable".to_string()];
    ops.updates_hold(false, "cell2", &chrome, true).await.unwrap();
    assert!(showhold().contains(&"google-chrome-stable".to_string()));
    ops.updates_hold(false, "cell2", &chrome, false).await.unwrap();
    assert_eq!(showhold(), STACK);
    let marks: Vec<(String, String, Option<String>)> =
        c.live.audit.entries().into_iter().rev().filter(|e| e.kind == "sudo" && e.command.contains("'google-chrome-stable'")).map(|e| (e.title, e.command, e.fence)).collect();
    assert_eq!(
        marks,
        [
            ("Hold google-chrome-stable on cell2".to_string(), "apt-mark hold 'google-chrome-stable'".to_string(), Some("apt-hold".to_string())),
            ("Release google-chrome-stable on cell2".to_string(), "apt-mark unhold 'google-chrome-stable'".to_string(), Some("apt-hold".to_string())),
        ]
    );
    // Release the pin puts it back (the rollback).
    let released = ops.updates_hold_preset(false, "cell2", false, false).await.unwrap();
    assert_eq!(released.packages, STACK);
    wait_task(ops.tasks(), &released.task_id.unwrap(), 20).await;
    assert!(showhold().is_empty());
    assert!(refusals(&c.live).is_empty());
}

// ---- Step 25: the security fixes on cell1, and a service from needrestart's list

#[tokio::test]
async fn step25_security_fixes_on_cell1_and_nginx_from_the_list() {
    let app = temp("rehearsal-b25-app");
    let c = cluster(&app, &["cell1", "gx10-0000"], |_| Gb10::Healthy).await;
    let node = c.node("cell1");
    // nginx is one of cell1's fixes, and its own upgrade may restart it, so needrestart may or may
    // not name it on the real node. Here cell1's list names it; gx10-0000's names nothing.
    std::fs::write(node.state_dir.join("needrestart"), "NEEDRESTART-VER: 3.6\nNEEDRESTART-KCUR: 7.0.0-1019-nvidia\nNEEDRESTART-KEXP: 7.0.0-1019-nvidia\nNEEDRESTART-KSTA: 1\nNEEDRESTART-SVC: nginx.service\n").unwrap();
    let ops = ops_for(&c.live, &c.inventory);
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    f.open(&all_fence());

    // Upgrade security only on cell1: its three security fixes (nginx and nginx-common among
    // them), not Chrome, as apt-one.
    let fixes = |node: &'static str| {
        let ops = ops.clone();
        async move {
            let pending = ops.updates_status(false, node).await.unwrap().pending;
            let picked = crate::ops::gpu_guard::security_only(&pending);
            ops.updates_simulate(false, node, &picked).await.unwrap()
        }
    };
    let sim = fixes("cell1").await;
    let id = ops.updates_upgrade_opts(false, "cell1".into(), Vec::new(), bound(&sim, None, true)).unwrap();
    let t = wait_task(ops.tasks(), &id, 60).await;
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
    let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
    let mut picked = run.packages.clone();
    picked.sort();
    assert_eq!(picked, ["libbrotli1", "nginx", "nginx-common"]);
    let start = rows_of(&c.live, &id).into_iter().find(|e| e.kind == "sudo").unwrap();
    let unit_id = run.unit.strip_prefix("sparkbox-upgrade-").unwrap();
    let pins: Vec<String> = run.packages.iter().map(|p| format!("{p}=2.0-1")).collect();
    let pins: Vec<&str> = pins.iter().map(String::as_str).collect();
    assert_eq!((start.command.clone(), start.fence.as_deref()), (c1(&c.live.upgrade_log_dir(), unit_id, &pins, None), Some("apt-one")));
    // needrestart's list, from the unit's log: nginx.service this time.
    assert_eq!(run.needrestart.as_ref().map(|n| n.services.clone()), Some(vec!["nginx.service".to_string()]));
    assert!(t.summary.as_deref().unwrap_or("").contains("nginx.service") || t.log.iter().any(|l| l.contains("nginx.service")), "{:?} {:?}", t.summary, t.log);

    // Restart it from the list: the existing service action, admitted for nginx.service only.
    ops.service_action(false, "cell1", "nginx.service", "restart").await.unwrap();
    let row = c.live.audit.entries().into_iter().find(|e| e.command == "systemctl restart -- 'nginx.service'").unwrap();
    assert_eq!((row.title.as_str(), row.kind.as_str(), row.fence.as_deref(), row.state.as_str()), ("Restart nginx.service on cell1", "sudo", Some("service"), "ok"));
    let e = ops.service_action(false, "cell1", "earlyoom.service", "restart").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow service changes to earlyoom.service: it names only nginx.service."));
    assert!(node.ran("'earlyoom.service'").is_empty());

    // Then gx10-0000, one at a time: its one security fix. Its list names no service, so the
    // panel lists none and there's nothing to restart.
    let sim = fixes("gx10-0000").await;
    let id = ops.updates_upgrade_opts(false, "gx10-0000".into(), Vec::new(), bound(&sim, None, true)).unwrap();
    let t = wait_task(ops.tasks(), &id, 60).await;
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
    let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
    assert_eq!(run.packages, ["libbrotli1"]);
    assert_eq!(run.needrestart.as_ref().map(|n| n.services.clone()), Some(Vec::new()));
    assert!(!t.summary.as_deref().unwrap_or("").contains("old libraries"), "{:?}", t.summary);
}

// ---- Step 26: cell3's Wi-Fi off through Fleet, and back on

/// NetworkManager's `nmcli [-w N] radio wifi [on|off]`, with the abbreviations nmcli takes
/// (`r`, `wi`…), on a node whose radio state is a file: `on`/`off` set it and print nothing;
/// with no state word it prints "enabled" or "disabled". Whether the cluster user may switch it
/// without root (polkit) isn't modelled: Session B switches it as root.
const NMCLI: &str = r#"#!/bin/dash
st="$SB_FAKE_STATE/wifi"
while [ $# -gt 0 ]; do case "$1" in -w|--wait) shift 2 ;; *) break ;; esac; done
case "$1" in r|ra|rad|radi|radio) ;; *) exit 2 ;; esac
case "$2" in wi|wif|wifi) ;; *) exit 2 ;; esac
case "$3" in
  "") cat "$st" 2>/dev/null || echo enabled ;;
  on) echo enabled > "$st" ;;
  off) echo disabled > "$st" ;;
  *) exit 2 ;;
esac
"#;

/// One Fleet run, as the window starts it, to its end.
async fn fleet_run(ops: &Arc<Ops>, node: &str, command: &str, as_root: bool) -> (String, crate::ops::FleetResult) {
    let spec = crate::ops::fleet::FleetSpec { nodes: vec![node.into()], command: command.into(), sudo: as_root, timeout_s: 30, ..Default::default() };
    let id = ops.fleet_start(false, spec).unwrap();
    for _ in 0..600 {
        if let Some(mut r) = ops.fleet_results(&id) {
            return (id, r.remove(0));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the Fleet run {id} never finished");
}

#[tokio::test]
async fn step26_cell3s_wifi_off_through_fleet_and_back_on() {
    let app = temp("rehearsal-b26-app");
    let c = cluster(&app, &["cell2", "cell3"], |_| Gb10::Healthy).await;
    let cell3 = c.node("cell3");
    cell3.bin("nmcli", NMCLI);
    let ops = ops_for(&c.live, &c.inventory);
    let f = Fences::new(&app);
    c.live.enforce_writes(f.rules());
    let radio = || std::fs::read_to_string(cell3.state_dir.join("wifi")).unwrap_or_else(|_| "enabled\n".into());
    let wrapped = |cmd: &str, root: bool| crate::ops::fleet::wrap(cmd, 30, root);
    let sent = || cell3.ran("nmcli").len();

    // Refused under the other two Session B fences, nothing sent: cell3 isn't in cell2's, and the
    // all-nodes one has no `wifi`.
    for (spec, why) in [(cell2_fence(), "cell3 isn't in the open fence."), (all_fence(), "The fence doesn't allow Wi-Fi changes.")] {
        f.open(&spec);
        for cmd in ["nmcli r wifi off", "sudo nmcli radio wifi off"] {
            let (_, r) = fleet_run(&ops, "cell3", cmd, true).await;
            assert!(r.error.as_deref().is_some_and(|e| e.contains(why)), "{cmd} under {}: {r:?}", spec.note);
            let refused = refusals(&c.live).into_iter().next().unwrap();
            assert_eq!((refused.node.as_str(), refused.command.as_str(), refused.error.as_deref()), ("cell3", wrapped(cmd, true).as_str(), Some(why)));
        }
    }
    // Nor on cell2 under its own fence, which has no `wifi`.
    f.open(&cell2_fence());
    let (_, r) = fleet_run(&ops, "cell2", "sudo nmcli radio wifi off", true).await;
    assert!(r.error.as_deref().is_some_and(|e| e.contains("The fence doesn't allow Wi-Fi changes.")), "{r:?}");
    assert_eq!((sent(), radio().as_str()), (0, "enabled\n"));

    // Under the cell3 Wi-Fi fence. `nmcli r w off` isn't Wi-Fi to SparkBox (`w` is wifi or wwan),
    // so it's a Fleet command, which this fence doesn't allow.
    f.open(&wifi_fence());
    let (_, r) = fleet_run(&ops, "cell3", "nmcli r w off", true).await;
    assert!(r.error.as_deref().is_some_and(|e| e.contains("The fence doesn't allow Fleet commands.")), "{r:?}");
    assert_eq!(sent(), 0);

    // Typed with `sudo` but without "as root": admitted as wifi, and refused by sudo on the node,
    // which has no terminal to ask for the password on. Nothing changed.
    let (id, r) = fleet_run(&ops, "cell3", "sudo nmcli radio wifi off", false).await;
    assert_eq!((r.state.as_str(), r.code), ("failed", Some(1)), "{r:?}");
    assert!(r.stderr.contains("sudo: a terminal is required to read the password"), "{r:?}");
    let row = rows_of(&c.live, &id).into_iter().find(|e| e.command == wrapped("sudo nmcli radio wifi off", false)).expect("its row");
    assert_eq!((row.kind.as_str(), row.fence.as_deref()), ("stream", Some("wifi")));
    assert_eq!(radio(), "enabled\n");

    // As root: `sudo nmcli radio wifi off` (the playbook's), and the rollback `nmcli radio wifi on`;
    // then the short form and `sudo` again for the rollback. Each one sudo row, wifi, ok. Fleet
    // runs it as a command that may cut access: on its node, then logs in to it again.
    let v = crate::ops::classify::verdict("sudo nmcli radio wifi off");
    assert_eq!((v.cuts_access, v.reason.as_deref()), (true, Some("Takes the network down (nmcli radio wifi): the node may drop off the network.")));
    for (cmd, after) in [("sudo nmcli radio wifi off", "disabled\n"), ("nmcli radio wifi on", "enabled\n"), ("nmcli r wifi off", "disabled\n"), ("sudo nmcli radio wifi on", "enabled\n")] {
        let (id, r) = fleet_run(&ops, "cell3", cmd, true).await;
        assert_eq!((r.state.as_str(), r.code), ("ok", Some(0)), "{cmd}: {r:?}");
        let head = head_of(&c.live, &id);
        assert_eq!((head.title.clone(), head.command.as_str(), head.state.as_str()), (format!("Fleet: {cmd} (as root)"), cmd, "ok"));
        let sudo: Vec<_> = rows_of(&c.live, &id).into_iter().filter(|e| e.kind == "sudo").collect();
        assert_eq!(sudo.iter().map(cols).collect::<Vec<_>>(), [(format!("Fleet: {cmd} (as root)"), "sudo".into(), wrapped(cmd, true), Some("wifi".into()), "ok".into())], "{cmd}");
        // Off may cut the node off the network: Fleet runs it on its own and logs in again.
        let t = wait_task(ops.tasks(), &id, 30).await;
        let steps: Vec<(&str, StepState)> = t.steps.iter().map(|s| (s.label.as_str(), s.state)).collect();
        if cmd.ends_with(" off") {
            assert_eq!(steps, [("cell3 (1 of 1)", StepState::Ok), ("Logging in to cell3 again over SSH", StepState::Ok)], "{cmd}");
        } else {
            assert_eq!(steps, [("Running on 1 nodes at once", StepState::Ok)], "{cmd}");
        }
        assert_eq!(radio(), after, "{cmd}");
    }
    assert!(c.live.audit.entries().iter().any(|e| e.node == "cell3" && lease_claim(e)), "cell3's lease claimed for the first change");
    // The node was asked once per admitted run; nothing reached cell2.
    assert_eq!(sent(), 5);
    assert!(c.node("cell2").ran("nmcli").is_empty() && c.node("cell2").ran("sudo ").is_empty());
    c.live.stop();
}

// ---- Step 27: first light, gemma-3-27b-it on cell2

use crate::models::fake_vllm::{self, Knobs, Standin, GLM53_IMAGE};
use crate::models::plan::{self as load_plan, DeploySpec, DeploymentState};
use crate::models::{readiness, Ctx as ModelsCtx, Deployment, ModelSettings, Models};

/// gemma-3-27b-it where cell2's Library found it on 2026-10-30 (look-only check L28): a Hugging
/// Face cache snapshot under the cluster user's home.
const GEMMA_SNAPSHOT: &str = ".cache/huggingface/hub/models--google--gemma-3-27b-it/snapshots/005ad3404e59d6023443cb575daa05336842228a";
const GIB_BYTES: u64 = 1 << 30;

/// What cell2 answers that the vLLM stand-in doesn't: SparkBox's quick node check (sparkjob.py,
/// as a GB10 answers it), and `ss -Hltn 'sport = :P'`, which prints the listener when something
/// accepts on 127.0.0.1:P and nothing otherwise, exit 0, as ss does (this Mac has no ss).
fn gb10_for_loads() -> Script {
    Arc::new(|cmd: &str, _: &[u8]| {
        if cmd.contains("sparkjob.py check") {
            let facts = json!({ "home": "/home/sparkuser", "rootPath": "/home/sparkuser/models", "rootExists": true, "freeBytes": 2_000_000_000_000u64,
                "docker": true, "rsync": true, "python": "3.12.3", "imagePresent": true, "rayImagePresent": false, "vllmVersion": "", "fabric": [] });
            return Some((facts.to_string(), 0));
        }
        let port: u16 = cmd.strip_prefix("ss -Hltn 'sport = :")?.split('\'').next()?.parse().ok()?;
        let open = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(300)).is_ok();
        Some((if open { format!("LISTEN 0      4096       127.0.0.1:{port}        0.0.0.0:*\n") } else { String::new() }, 0))
    })
}

/// cell2 idle, as its collector reads it: nothing loaded, no containers, 116.7 GiB available of
/// 121.7 (L28's MemAvailable).
fn idle_frame(node: &Node) {
    let mut v: Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    v["mem"] = json!({ "total": 127_598_748u64 * 1024, "used": 5_241_972u64 * 1024, "available": 122_356_776u64 * 1024, "cached": 3_196_204u64 * 1024,
        "gpuAlloc": 0, "swapTotal": 0, "swapUsed": 0 });
    v["gpu"] = json!({ "utilPct": 0.0, "tempC": 41.0, "powerW": 4.0, "smClockMhz": 0, "procs": [] });
    v["containers"] = json!([]);
    *node.knobs.frame.lock().unwrap() = Some(v.to_string());
    node.knobs.frame_every_ms.store(200, std::sync::atomic::Ordering::SeqCst);
}

/// cell2 with the vLLM stand-in, Models and Chat's providers over it, the release build's exit
/// enforcing the fences; gemma's snapshot folder in its home.
struct Light {
    live: Arc<Live>,
    inventory: Inventory,
    node: Node,
    standin: Standin,
    models: Arc<Models>,
    llm: Arc<crate::llm::Llm>,
    tasks: Arc<Tasks>,
    ops: Arc<Ops>,
    f: Fences,
    folder: String,
    port: u16,
}

/// The build this test binary is: a debug build, so "dev". A load's containers carry their
/// build's `sparkbox.instance` label (plan::rank_command), and the exit refuses an unload whose
/// label filter names the other build, so first light runs its presets opened for this build:
/// the same nodes, classes and hours, the instance aside. The release build labels and enforces
/// "release" (contract C-L2, live::exit_tests).
fn for_this_build(spec: Spec) -> Spec {
    Spec { instance: crate::identity::instance_name().into(), ..spec }
}

async fn first_light(app: &Path, knobs: Knobs, more: &[(&str, u16)]) -> Light {
    let home = app.join("home-cell2");
    let folder = home.join(GEMMA_SNAPSHOT);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("config.json"), r#"{"architectures": ["Gemma3ForConditionalGeneration"]}"#).unwrap();
    let (node, port, standin) = fake_vllm::node_with(&home, knobs, gb10_for_loads()).await;
    idle_frame(&node);
    let ports: Vec<(&str, u16)> = [("cell2", port)].into_iter().chain(more.iter().copied()).collect();
    let (live, inventory) = fake_node::connect(app, &ports).await;
    let vpn = crate::vpn::Vpn::new(app.join("vpn2.json"), app.join("no-helper"), |_| {});
    let llm = crate::llm::Llm::new(app.join("providers.json"), vpn);
    std::fs::create_dir_all(app.join("models")).unwrap();
    let models = Models::new(app.join("models"), live.clone(), llm.clone(), None);
    let tasks = Tasks::new(None);
    models.set_tasks(tasks.clone());
    models.set_unload_wait(Duration::from_millis(200), Duration::from_secs(5));
    let ops = ops_for(&live, &inventory);
    let f = Fences::new(app);
    live.enforce_writes(Enforce { instance: crate::identity::instance_name().into(), ..f.rules() });
    Light { live, inventory, node, standin, models, llm, tasks, ops, f, folder: folder.display().to_string(), port: fake_vllm::free_port() }
}

impl Light {
    fn ctx(&self) -> ModelsCtx {
        ModelsCtx { live: true, settings: ModelSettings::default(), inventory: self.inventory.clone(), snapshot: Some(self.live.snapshot(&self.inventory)) }
    }

    /// What the Load page sends for gemma on cell2: the glm53-flash image, share 0.75, 32k
    /// context, the LAN with a per-load key (the defaults), and the owner's drop consent.
    fn gemma(&self, id: &str, drop_caches: bool) -> DeploySpec {
        DeploySpec {
            id: id.into(),
            served_name: "gemma-3-27b-it".into(),
            model_path: self.folder.clone(),
            nodes: vec!["cell2".into()],
            max_model_len: Some(32768),
            gpu_memory_utilization: 0.75,
            port: self.port,
            image_ref: GLM53_IMAGE.into(),
            drop_caches,
            ..Default::default()
        }
    }

    fn deployment(&self, id: &str) -> Deployment {
        self.models.deployments(&self.ctx()).into_iter().find(|d| d.spec.id == id).unwrap()
    }

    async fn wait_state(&self, id: &str, want: DeploymentState, secs: u64) -> Deployment {
        let started = Instant::now();
        loop {
            let d = self.deployment(id);
            if d.state == want {
                return d;
            }
            assert!(started.elapsed() < Duration::from_secs(secs), "{id} never {want:?}: {} · {:?}", d.message, d.readiness);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The newest row whose command is exactly `cmd`.
    fn row(&self, cmd: &str) -> audit::Entry {
        self.live.audit.entries().into_iter().find(|e| e.command == cmd).unwrap_or_else(|| panic!("no row for `{cmd}`"))
    }

    /// What the exit decided for `cmd`, the newest time it was asked.
    fn verdict(&self, cmd: &str) -> (Verdict, bool) {
        let a = self.live.admits().into_iter().rev().find(|a| a.cmd == cmd).unwrap_or_else(|| panic!("the exit was never asked `{cmd}`"));
        (a.verdict, a.admitted)
    }

    fn key_file(&self, id: &str) -> PathBuf {
        self.node.home.join(load_plan::key_file(id))
    }
}

impl Drop for Light {
    fn drop(&mut self) {
        self.live.stop();
    }
}

/// One reply in Chat through the load's provider (Mac → the node's port directly), as Chat's
/// window streams it: (how many pieces of text streamed, the saved reply). `stop_after`: press
/// Stop once that many pieces have arrived.
async fn chat(llm: &Arc<crate::llm::Llm>, provider: &str, prompt: &str, stop_after: Option<usize>) -> (usize, crate::llm::history::Message) {
    let req: crate::llm::chat::ChatRequest = serde_json::from_value(json!({
        "requestId": "rehearsal", "provider": provider, "model": "gemma-3-27b-it",
        "messages": [{ "role": "user", "content": prompt, "createdAt": 0 }],
    }))
    .unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let stop = Mutex::new(Some(stop));
    let pieces = std::sync::atomic::AtomicUsize::new(0);
    let done = Mutex::new(None);
    let errors = Mutex::new(Vec::new());
    let emit = |e: crate::llm::chat::ChatEvent| match e {
        crate::llm::chat::ChatEvent::Text { .. } => {
            let n = pieces.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if Some(n) == stop_after {
                if let Some(s) = stop.lock().unwrap().take() {
                    let _ = s.send(());
                }
            }
        }
        crate::llm::chat::ChatEvent::Done { message } => *done.lock().unwrap() = Some(message),
        crate::llm::chat::ChatEvent::Error { message } => errors.lock().unwrap().push(message),
        _ => {}
    };
    llm.chat(req, &emit, stopped).await;
    assert!(errors.lock().unwrap().is_empty(), "{:?}", errors.lock().unwrap());
    let reply = done.lock().unwrap().take().expect("a reply");
    (pieces.load(std::sync::atomic::Ordering::SeqCst), reply)
}

#[tokio::test]
async fn step27_first_light_on_cell2_end_to_end() {
    let app = temp("rehearsal-b27-app");
    // cell2 with most of its free memory in the page cache, so vLLM at 0.75 would refuse to start
    // without the drop; and replies streamed a token every 20 ms, so Stop lands mid-reply.
    let knobs = Knobs { mem_free: 60 * GIB_BYTES, mem_cached: 56 * GIB_BYTES, stream_ms: 20, ..Default::default() };
    let l = first_light(&app, knobs, &[]).await;
    l.f.open(&for_this_build(cell2_fence()));
    let id = "b27";
    let spec = l.gemma(id, false);

    // Before it starts (the Load page): one read, no class. A drop is needed.
    let pre = l.models.preflight(&l.ctx(), &spec).await.unwrap().nodes.remove(0);
    let mem = pre.mem.clone().expect("cell2's memory");
    assert_eq!(pre.need_free, (mem.total as f64 * 0.75) as u64 + 4 * GIB_BYTES);
    assert!(pre.drop_needed && mem.free < pre.need_free && !pre.demo, "{pre:?}");
    assert_eq!((pre.image_ref.as_str(), pre.image_id.clone()), (GLM53_IMAGE, Some(fake_vllm::test_image_id())));
    assert!(pre.model_present && pre.port_free && pre.shm.is_empty() && pre.problems.is_empty(), "{pre:?}");
    let c_p = load_plan::preflight_script(&spec);
    let read = l.row(&c_p);
    assert_eq!((read.kind.as_str(), read.fence.as_deref(), read.escalated), ("command", None, false));
    assert_eq!(l.verdict(&c_p), (Verdict::Read, true));
    // Load also reads the model's config and size for the memory plan, as the page does.
    let q = crate::live::sh_quote(&l.folder);
    let needs = format!("cat {q}/config.json 2>/dev/null; echo; du -sbL {q} 2>/dev/null | cut -f1");

    // Load without ticking the drop: refused with the figures, before anything changes.
    let e = l.models.deploy(&l.ctx(), spec.clone()).await.unwrap_err();
    assert!(e.starts_with("cell2 has 60.0 GiB free; vLLM at share 0.75 needs ") && e.ends_with("Tick 'Drop the page cache first' to let SparkBox free it (sudo; the kernel refills it)."), "{e}");
    assert!(l.node.ran("sudo ").is_empty() && l.node.ran("docker run").is_empty() && l.standin.drops() == 0);

    // Load with the drop ticked: the drop as drop-caches through sudo, the node read again, the
    // launch files with the key on stdin (C-F), then C-L2, both as model-load-one.
    let d = l.models.deploy(&l.ctx(), DeploySpec { drop_caches: true, ..spec.clone() }).await.unwrap();
    assert_eq!(d.state, DeploymentState::Starting);
    let read = l.row(&needs);
    assert_eq!((read.kind.as_str(), read.class.as_str(), read.fence.as_deref()), ("command", "read", None));
    assert_eq!(l.standin.drops(), 1);
    let drop = l.row(load_plan::DROP_CACHES);
    assert_eq!(cols(&drop), ("Drop the page cache on cell2 (vLLM counts it as used)".into(), "sudo".into(), load_plan::DROP_CACHES.into(), Some("drop-caches".into()), "ok".into()));
    assert_eq!(l.node.ran(&format!("sudo -k -S -p '' {}", load_plan::DROP_CACHES)).len(), 1, "the password on sudo's stdin only");
    let d = l.wait_state(id, DeploymentState::Ready, 60).await;
    assert_eq!(d.spec.image_id, fake_vllm::test_image_id(), "started by the image's local id");
    assert_eq!(d.baseline.iter().map(|b| (b.node.as_str(), b.shm.len())).collect::<Vec<_>>(), [("cell2", 0)]);
    let c_l = load_plan::rank_command(&d.spec, 0);
    let shown = audit::redact(&c_l);
    let run = l.row(&shown);
    assert_eq!(cols(&run), ("Load gemma-3-27b-it on cell2".into(), "command".into(), shown.clone(), Some("model-load-one".into()), "ok".into()));
    assert_eq!(l.verdict(&c_l), (Verdict::Change(ModelLoadOne), true));
    assert!(c_l.starts_with(&format!("docker run -d --name sparkbox-{id} --label sparkbox.instance=dev ")), "{c_l}");
    assert!(c_l.ends_with(&format!(" --entrypoint python3 {} /run/sparkbox/launch.py --key", fake_vllm::test_image_id())), "{c_l}");
    assert!(!c_l.contains(" -e ") && !c_l.contains("--shm-size") && !c_l.contains("ray") && !c_l.contains("--privileged"), "{c_l}");
    // A Hugging Face snapshot: its models--org--name folder is mounted (the snapshot's files link into its blobs).
    let repo = l.node.home.join(".cache/huggingface/hub/models--google--gemma-3-27b-it").display().to_string();
    assert!(c_l.contains(&format!(" -v {repo}:{repo}:ro -v ")), "{c_l}");
    // C-F, through the real sparkjob.py: what vLLM runs, as files; the key on the node's stdin, in
    // a 0400 file; never in a command or a row.
    let key = crate::secrets::get(&load_plan::key_account(id)).unwrap().expect("the load's key");
    let c_f = format!("python3 ~/.sparkbox/sparkjob.py deploy-files {id} single");
    let row = l.live.audit.entries().into_iter().find(|e| e.command.ends_with(&format!("deploy-files {id} single"))).expect("deploy-files' row");
    assert_eq!((row.kind.as_str(), row.fence.as_deref(), row.state.as_str()), ("command", Some("model-load-one"), "ok"));
    let sent = l.node.stdin.lock().unwrap().iter().find(|(cmd, _)| cmd.ends_with(&c_f) || cmd.contains(&format!("deploy-files {id} single"))).map(|(_, i)| i.clone()).expect("deploy-files' stdin");
    let sent: Value = serde_json::from_slice(&sent).unwrap();
    assert_eq!(sent["key"].as_str(), Some(key.as_str()));
    assert_eq!(sent["argv"], json!(load_plan::rank_argv(&d.spec, 0, &[])));
    let file = l.key_file(id);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!((std::fs::read_to_string(&file).unwrap(), std::fs::metadata(&file).unwrap().permissions().mode() & 0o777), (key.clone(), 0o400));
    assert!(!l.live.audit.entries().iter().any(|e| e.command.contains(&key)) && !l.standin.argv(&format!("sparkbox-{id}")).iter().any(|w| w.contains(&key)));
    assert_eq!(l.standin.seen(&format!("sparkbox-{id}"))["hasKey"].as_bool(), Some(true), "launch.py gave it to vLLM");
    // The cache folders were made for it.
    assert!([".cache/vllm", ".cache/flashinfer", ".triton"].iter().all(|f| l.node.home.join(f).is_dir()));

    // Ready only once it answered: /health, its name in /v1/models, a test token, 17+25 → 42.
    let r = d.readiness.clone().expect("the readiness check");
    assert!(r.ok && r.health && r.models_listed && r.token == Some(true), "{r:?}");
    let sanity = r.sanity.clone().expect("the sanity prompt: the first time this recipe meets this image");
    assert!(sanity.ok && sanity.answer.trim() == "42", "{sanity:?}");
    assert!(r.summary.starts_with("Ready: /health 200, gemma-3-27b-it listed, a test token in ") && r.summary.ends_with(", 17+25 → 42 (first run of this recipe and image)"), "{}", r.summary);
    let host = d.spec.host.clone();
    // The native APIs, recorded for P4 right after it became Ready.
    until("/openapi.json read once Ready", 30, || l.deployment(id).protocols.is_some()).await;
    for (cmd, verdict) in [
        (readiness::models_command(&host, l.port, true), Verdict::Read),
        (readiness::sanity_command(&host, l.port, "gemma-3-27b-it", true), Verdict::Use),
        (readiness::openapi_command(&host, l.port), Verdict::Read),
    ] {
        assert_eq!(l.verdict(&cmd), (verdict, true), "{cmd}");
        let row = l.row(&cmd);
        assert_eq!((row.kind.as_str(), row.fence.as_deref(), row.title.as_str()), ("command", None, "Load gemma-3-27b-it on cell2"), "{cmd}");
    }
    let token = l.live.admits().into_iter().find(|a| a.verdict == Verdict::Use && a.cmd.contains("/v1/completions")).expect("the test token");
    assert!(token.admitted && !token.cmd.contains(&key), "{token:?}");
    // The load's rows (the playbook's table): the pre-flight and the quick check, again after the
    // drop, which are reads running side by side; then the launch files and C-L2, the load's only
    // changes; then the readiness reads and inference (probe rows) and /openapi.json.
    let op = run.op_id.clone().unwrap();
    let rows = rows_of(&l.live, &op);
    let cmds: Vec<&str> = rows.iter().map(|e| e.command.as_str()).collect();
    let reads = cmds.iter().take_while(|c| !c.contains("deploy-files")).copied().collect::<Vec<_>>();
    assert_eq!(reads.iter().filter(|c| **c == c_p).count(), 2, "{reads:#?}");
    assert_eq!(reads.iter().filter(|c| c.starts_with("python3 ~/.sparkbox/sparkjob.py check '~/models' ") && c.ends_with(" quick")).count(), 2, "{reads:#?}");
    assert_eq!(reads.len(), 4, "{reads:#?}");
    assert!(rows[..4].iter().all(|e| e.class == "read" && e.fence.is_none()), "{rows:#?}");
    let changes: Vec<(&str, Option<&str>)> = rows.iter().filter(|e| e.class == "change").map(|e| (e.command.as_str(), e.fence.as_deref())).collect();
    assert_eq!(changes, [(row.command.as_str(), Some("model-load-one")), (shown.as_str(), Some("model-load-one"))], "{rows:#?}");
    let files = rows.iter().find(|e| e.title == "Launch files for vLLM on cell2").expect("what deploy-files got, in the log");
    assert!(files.command.contains("\"HF_HUB_OFFLINE\":\"1\"") && files.command.contains("\"vllm\",\"serve\""), "{}", files.command);
    // launch.py, only once deploy-files had been admitted, and a row of its own: the file is new here.
    let copy = rows.iter().find(|e| e.title == "Copy launch.py to cell2").expect("the launcher's row");
    assert_eq!((copy.kind.as_str(), copy.class.as_str(), copy.fence.as_deref(), copy.state.as_str()), ("app", "app", None, "ok"));
    assert!(copy.command.starts_with(&format!("~/.sparkbox/{} sha256 ", load_plan::launcher_name())), "{}", copy.command);
    let at = |cmd: &str| rows.iter().position(|e| e.command == cmd).unwrap_or_else(|| panic!("no row {cmd}"));
    assert!(at(&row.command) < at(&copy.command) && at(&copy.command) < at(&shown), "{cmds:#?}");
    assert!(!cmds.iter().any(|c| *c == load_plan::unload_command(id)), "nothing of this id to clear: leftovers stop a load at the pre-flight");
    let probes: Vec<&str> = rows.iter().filter(|e| e.class == "probe").map(|e| e.command.as_str()).collect();
    assert_eq!((probes[0], probes[probes.len() - 2], probes[probes.len() - 1]), (readiness::models_command(&d.spec.host, l.port, true).as_str(), readiness::models_command(&d.spec.host, l.port, false).as_str(), readiness::openapi_command(&d.spec.host, l.port).as_str()), "{probes:#?}");
    assert!(probes[1].contains("/v1/completions") && probes[1].contains(" -H @- "), "{}", probes[1]);
    assert_eq!(probes[2], readiness::sanity_command(&d.spec.host, l.port, "gemma-3-27b-it", true));
    let p = l.deployment(id).protocols.unwrap();
    assert_eq!((p.messages, p.count_tokens, p.responses, p.image_id.as_str()), (true, true, true, fake_vllm::test_image_id().as_str()));

    // Chat with this: the provider goes to cell2's port directly, from this Mac (no node rows).
    let provider = l.llm.get(&format!("vllm-{id}")).await.expect("in Chat once Ready");
    assert_eq!((provider.base_url.as_str(), provider.default_model.as_deref()), (d.endpoint.as_str(), Some("gemma-3-27b-it")));
    let rows_before = l.live.audit.entries().len();
    let (pieces, reply) = chat(&l.llm, &provider.id, "Tell me about the GB10.", None).await;
    let stats = reply.stats.clone().expect("the stats line");
    assert!(pieces > 10 && stats.output_tokens == Some(64) && stats.input_tokens.is_some() && stats.ttft_ms.is_some(), "{pieces} {stats:?}");
    // Stop mid-reply keeps what arrived; Regenerate asks again and gets the whole reply.
    let (stopped_at, partial) = chat(&l.llm, &provider.id, "Tell me more.", Some(5)).await;
    assert_eq!((stopped_at, partial.stop_reason.as_deref()), (5, Some("stopped")));
    assert!(partial.content.len() < reply.content.len() && !partial.content.is_empty());
    let (_, again) = chat(&l.llm, &provider.id, "Tell me more.", None).await;
    assert!(again.stop_reason.as_deref() != Some("stopped") && again.content.len() > partial.content.len(), "{again:?}");
    assert_eq!(l.live.audit.entries().len(), rows_before, "Chat writes no node row");

    // Fleet on cell2: `ps -eo comm` (the process names for P2's earlyoom rule), a read.
    let (fid, r) = fleet_run(&l.ops, "cell2", "ps -eo comm", false).await;
    assert_eq!(r.state, "ok", "{r:?}");
    let ps = rows_of(&l.live, &fid).into_iter().find(|e| e.kind == "stream").expect("its row");
    assert_eq!((ps.command.as_str(), ps.fence.as_deref(), ps.state.as_str()), (crate::ops::fleet::wrap("ps -eo comm", 30, false).as_str(), None, "ok"));

    // Unload: docker stop -t 30, then docker rm (C-U2, model-unload); then memory, the port and
    // /dev/shm checked against the baseline.
    let task = l.models.unload(&l.ctx(), id).await.unwrap();
    let t = wait_task(&l.tasks, &task, 60).await;
    assert_eq!((t.state, t.title.as_str()), (TaskState::Ok, "Unload gemma-3-27b-it from cell2"), "{:?} {:?}", t.error, t.steps);
    let steps: Vec<(&str, StepState)> = t.steps.iter().map(|s| (s.label.as_str(), s.state)).collect();
    assert_eq!(steps, [("Stopping (docker stop -t 30, then docker rm)", StepState::Ok), ("Checking memory, port and /dev/shm", StepState::Ok)]);
    let c_u = load_plan::unload_command(id);
    assert_eq!(c_u, format!("ids=$(docker ps -aq --filter label=sparkbox.instance={i} --filter label=sparkbox.deployment={id}); [ -z \"$ids\" ] || docker stop -t 30 $ids >/dev/null; [ -z \"$ids\" ] || docker rm $ids >/dev/null; rm -rf ~/.sparkbox/keys/{id} ~/.sparkbox/deploy/{id}; echo ok", i = crate::identity::instance_name()));
    let rows: Vec<(String, String, Option<String>, String)> = rows_of(&l.live, &task).into_iter().map(|e| (e.kind, e.command, e.fence, e.state)).collect();
    let containers = load_plan::load_containers_command(id);
    let read = |cmd: &str| ("command".to_string(), cmd.to_string(), None, "ok".to_string());
    let port_read = format!("ss -Hltn 'sport = :{}'", l.port);
    assert_eq!(
        rows,
        [("command".into(), c_u.clone(), Some("model-unload".into()), "ok".into()), read(&containers), read("grep MemAvailable /proc/meminfo"), read(&port_read), read("ls -la /dev/shm")]
    );
    // Before the task: whose containers of it are on each connected node (a read, no class).
    let all: Vec<audit::Entry> = l.live.audit.entries().into_iter().rev().collect();
    let asked = all.iter().position(|e| e.command == containers).expect("the containers read");
    let stopped = all.iter().position(|e| e.command == c_u && e.op_id.as_deref() == Some(task.as_str())).unwrap();
    assert!(asked < stopped && all[asked].fence.is_none() && all[asked].op_id.as_deref() != Some(task.as_str()));
    let d = l.deployment(id);
    assert_eq!(d.state, DeploymentState::Stopped);
    let report = d.unload_report.clone().expect("the unload's report");
    assert!(report.ok, "{report:?}");
    assert_eq!(report.nodes.iter().map(|n| (n.node.as_str(), n.stopped, n.mem_back, n.port_closed, n.new_shm.len())).collect::<Vec<_>>(), [("cell2", true, true, true, 0)]);
    assert!(report.summary.starts_with("Unloaded from cell2 · memory back (") && report.summary.ends_with(&format!(" · port {} closed · /dev/shm: nothing new", l.port)), "{}", report.summary);
    assert_eq!(t.summary.as_deref(), Some(report.summary.as_str()));
    // TERM, not KILL: vLLM's /dev/shm segment went with it; nothing runs; the key is gone everywhere.
    assert!(l.standin.running().is_empty() && l.standin.shm().is_empty());
    assert!(!l.key_file(id).exists() && crate::secrets::get(&load_plan::key_account(id)).unwrap().is_none());
    // Gone from Chat's picker.
    assert!(l.llm.get(&format!("vllm-{id}")).await.is_none());
    assert!(!l.llm.views().await.iter().any(|v| v.provider.id == format!("vllm-{id}")));
    assert!(refusals(&l.live).is_empty(), "{:#?}", refusals(&l.live));
}

#[tokio::test]
async fn step27_a_wrong_answer_fails_the_load_and_the_fallback_image_is_checked_again() {
    let app = temp("rehearsal-b27w-app");
    // The GB10 garbage-output bug: 17+25 comes back as "4 2 2 2…". Enough free memory: no drop.
    let l = first_light(&app, Knobs { garbage: true, ..Default::default() }, &[]).await;
    l.f.open(&for_this_build(cell2_fence()));
    let id = "b27w";
    l.models.deploy(&l.ctx(), l.gemma(id, false)).await.unwrap();
    let d = l.wait_state(id, DeploymentState::Failed, 60).await;
    assert_eq!(
        d.message,
        "It answers, but wrongly: asked 17+25, it said '4 2 2 2 2 2 2 2'. This is the GB10 garbage-output bug; try the fallback image (nvcr.io/nvidia/vllm:26.05-py3)."
    );
    let sanity = d.readiness.clone().and_then(|r| r.sanity).expect("the verdict");
    assert!(!sanity.ok);
    assert!(l.llm.get(&format!("vllm-{id}")).await.is_none(), "never in Chat");
    assert_eq!(l.standin.drops(), 0, "no drop was needed");
    // Rolled back (C-X): its log saved here, its container gone, nothing for Unload to do.
    assert!(l.standin.running().is_empty());
    let f = d.failure.clone().expect("the failure record");
    assert!(f.left_on.is_empty(), "{f:?}");
    assert!(std::fs::read_to_string(Path::new(&f.log_dir).join("r0.log")).unwrap().contains("Application startup complete."));

    // The fallback: Fleet on cell2, `docker pull nvcr.io/nvidia/vllm:<current>-py3`, a Fleet
    // command the cell2 fence allows (refused under the all-nodes one). The stand-in Docker has
    // no registry, so the pull itself is played by the image appearing on the node.
    let pull = format!("docker pull {}", readiness::FALLBACK_IMAGE);
    l.f.open(&for_this_build(all_fence()));
    let (_, r) = fleet_run(&l.ops, "cell2", &pull, false).await;
    assert!(r.error.as_deref().is_some_and(|e| e.contains("The fence doesn't allow Fleet commands.")), "{r:?}");
    l.f.open(&for_this_build(cell2_fence()));
    let (fid, _) = fleet_run(&l.ops, "cell2", &pull, false).await;
    let sent = crate::ops::fleet::wrap(&pull, 30, false);
    assert_eq!(l.verdict(&sent), (Verdict::Change(Fleet), true));
    let row = rows_of(&l.live, &fid).into_iter().find(|e| e.command == sent).expect("its row");
    assert_eq!((row.kind.as_str(), row.fence.as_deref()), ("stream", Some("fleet")));
    let fallback_id = format!("sha256:{}", "b".repeat(64));
    l.standin.set_knobs(Knobs { images: vec![(GLM53_IMAGE.into(), fake_vllm::test_image_id()), (readiness::FALLBACK_IMAGE.into(), fallback_id.clone())], ..Default::default() });

    // Load again with the fallback image: a new recipe (another image id), so 17+25 is asked again.
    let id2 = "b27f";
    l.models.deploy(&l.ctx(), DeploySpec { image_ref: readiness::FALLBACK_IMAGE.into(), ..l.gemma(id2, false) }).await.unwrap();
    let d = l.wait_state(id2, DeploymentState::Ready, 60).await;
    assert_eq!(d.spec.image_id, fallback_id);
    let r = d.readiness.clone().unwrap();
    assert!(r.sanity.as_ref().is_some_and(|s| s.ok && s.answer.trim() == "42"), "{r:?}");
    assert!(l.llm.get(&format!("vllm-{id2}")).await.is_some());
    // The verdicts are kept by recipe: the glm53 image's failed, the fallback's passed.
    let store = readiness::SanityStore::open(&app.join("models"));
    assert!(store.needs(&readiness::recipe_key(&l.deployment(id).spec)), "a failed verdict is asked again next time");
    assert!(!store.needs(&readiness::recipe_key(&d.spec)), "a passed one isn't");
    let t = wait_task(&l.tasks, &l.models.unload(&l.ctx(), id2).await.unwrap(), 60).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
}

#[tokio::test]
async fn step27_a_killed_vllm_leaves_a_shm_segment_that_the_unload_reports() {
    let app = temp("rehearsal-b27k-app");
    let l = first_light(&app, Knobs::default(), &[]).await;
    l.f.open(&for_this_build(cell2_fence()));
    let id = "b27k";
    l.models.deploy(&l.ctx(), l.gemma(id, false)).await.unwrap();
    l.wait_state(id, DeploymentState::Ready, 60).await;
    // vLLM killed outright (an OOM kill, a crash): its /dev/shm segment stays behind.
    let name = format!("sparkbox-{id}");
    l.standin.kill(&name);
    let d = l.wait_state(id, DeploymentState::Failed, 60).await;
    assert!(d.message.starts_with("vLLM stopped on cell2 (exit 137)."), "{}", d.message);
    let left = l.standin.shm();
    assert_eq!(left.len(), 1, "{left:?}");
    // Unload removes the stopped container and reports the segment; it removes nothing as root.
    let t = wait_task(&l.tasks, &l.models.unload(&l.ctx(), id).await.unwrap(), 60).await;
    assert_eq!((t.state, t.warning.as_deref()), (TaskState::Ok, Some("Unloaded, but something didn't come back; see the report.")), "{:?} {:?}", t.summary, t.error);
    let report = l.deployment(id).unload_report.expect("the report");
    assert!(!report.ok);
    assert_eq!(report.nodes[0].new_shm, left);
    assert!(report.summary.contains(&format!("/dev/shm: new since the load: {}", left[0])) && report.summary.ends_with("removing them needs sudo and your OK (not done)"), "{}", report.summary);
    assert_eq!(l.deployment(id).state, DeploymentState::Stopped, "the containers are gone: Stopped stands");
    assert_eq!(l.standin.shm(), left, "left for you");
    assert!(l.node.ran("sudo ").is_empty());
}

#[tokio::test]
async fn step27_the_load_and_the_drop_are_refused_under_the_wrong_fence_with_nothing_sent() {
    let app = temp("rehearsal-b27f-app");
    let l = first_light(&app, Knobs { mem_free: 60 * GIB_BYTES, mem_cached: 56 * GIB_BYTES, ..Default::default() }, &[]).await;

    // A drop under a fence without drop-caches: refused before sudo, nothing loaded.
    l.f.open(&for_this_build(Spec { classes: CELL2.into_iter().filter(|c| *c != DropCaches).collect(), ..cell2_fence() }));
    let e = l.models.deploy(&l.ctx(), l.gemma("b27x", true)).await.unwrap_err();
    assert_eq!(SbError::from(e).message, "The fence doesn't allow page-cache drops.");
    assert_eq!(l.standin.drops(), 0);
    assert!(l.node.ran("sudo ").is_empty() && l.node.ran("docker run").is_empty());

    // The first-light load under the all-nodes fence (no model-load-one): the drop isn't asked
    // (all-nodes has no drop-caches either), and with enough free memory the load itself is
    // refused at its launch files: no container, no key file.
    l.standin.set_knobs(Knobs::default());
    l.f.open(&for_this_build(all_fence()));
    let e = l.models.deploy(&l.ctx(), l.gemma("b27y", true)).await.err();
    let d = match e {
        Some(e) => panic!("refused before it started: {e}"),
        None => l.wait_state("b27y", DeploymentState::Failed, 30).await,
    };
    assert_eq!(SbError::from(d.message.clone()).message, "The fence doesn't allow model loads.", "{}", d.message);
    let refused: Vec<String> = refusals(&l.live).into_iter().map(|e| e.command).collect();
    assert!(refused.iter().any(|c| c.ends_with("deploy-files b27y single")), "{refused:?}");
    assert!(l.node.ran("deploy-files").is_empty(), "nothing reached the node");
    assert!(l.node.ran("docker run").is_empty() && l.standin.running().is_empty() && l.standin.argv("sparkbox-b27y").is_empty());
    assert!(!l.key_file("b27y").exists() && l.node.ran("sudo ").is_empty());
    assert!(l.llm.get("vllm-b27y").await.is_none());
    // Unload needs model-unload the same way: refused under the all-nodes fence, nothing sent.
    let t = wait_task(&l.tasks, &l.models.unload(&l.ctx(), "b27y").await.unwrap(), 30).await;
    assert_eq!(t.state, TaskState::Failed);
    assert_eq!(SbError::from(t.error.clone().unwrap()).message, "The fence doesn't allow model unloads.", "{:?}", t.error);
    assert!(l.node.ran("docker stop").is_empty() && l.node.ran(&load_plan::unload_command("b27y")).is_empty());
}

// ---- Step 28: the final evidence diff

/// `ip -br addr` as cell3 printed it on 29 October (live/fixtures/doctor/cell3/network.txt), with
/// wlP9s9 down and without its address while the radio is off (the nmcli stand-in's state).
const IP_CELL3: &str = r#"#!/bin/dash
[ "$1 $2" = "-br addr" ] || { echo "ip: this stand-in knows only -br addr" >&2; exit 1; }
printf '%s\n' 'lo               UNKNOWN        127.0.0.1/8 ' 'enP7s7           UP             192.168.50.98/24 ' 'enp1s0f1np1      UP             192.168.100.3/24 ' 'enP2p1s0f1np1    UP             192.168.101.3/24 '
if [ "$(cat "$SB_FAKE_STATE/wifi" 2>/dev/null)" = disabled ]; then echo 'wlP9s9           DOWN           '; else echo 'wlP9s9           UP             192.168.50.229/24 '; fi
printf '%s\n' 'br-2d0515ac359a  DOWN           172.18.0.1/16 ' 'docker0          DOWN           172.17.0.1/16 '
"#;

/// The step files a capture's diff names (`<node>/<step>.txt`), from its `diff -u` headers.
fn changed_files(diff: &str, dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = diff
        .lines()
        .filter_map(|l| l.strip_prefix("+++ "))
        .map(|l| l.split('\t').next().unwrap_or(l).trim())
        .filter_map(|p| Path::new(p).strip_prefix(dir).ok().map(|r| r.display().to_string()))
        .collect();
    files.sort();
    files
}

#[tokio::test]
async fn step28_the_evidence_diff_lists_only_what_session_b_changed() {
    let app = temp("rehearsal-b28-app");
    let home3 = app.join("home-cell3");
    std::fs::create_dir_all(&home3).unwrap();
    let (cell3, p3) = fake_node(&home3, fake_node::real_shell()).await;
    cell3.bin("nmcli", NMCLI);
    cell3.bin("ip", IP_CELL3);
    // cell3's Docker, with nothing of the load on it (the unload asks every connected node).
    let _docker3 = fake_vllm::install(&cell3, Knobs::default());
    let l = first_light(&app, Knobs::default(), &[("cell3", p3)]).await;
    // cell2's package database: the GPU stack and the rest, nothing held yet.
    let apt = l.node.state_dir.join("apt");
    std::fs::create_dir_all(&apt).unwrap();
    std::fs::write(apt.join("installed"), INSTALLED.iter().map(|p| format!("{p}\t1.0-1\n")).collect::<String>()).unwrap();
    let logs = app.join("Logs");
    let both = ["cell2".to_string(), "cell3".to_string()];
    let before = crate::evidence::capture_live(&l.live, &both, &logs, "p1", "before-session-b").await.unwrap();
    assert_eq!((before.nodes, before.not_connected.len()), (2, 0));

    // Session B on these two: cell2's GPU stack pinned and first light loaded and unloaded under
    // its fence; cell3's Wi-Fi off under its own.
    l.f.open(&for_this_build(cell2_fence()));
    let pinned = l.ops.updates_hold_preset(false, "cell2", true, false).await.unwrap();
    assert_eq!(pinned.packages, STACK);
    wait_task(l.ops.tasks(), &pinned.task_id.unwrap(), 20).await;
    let id = "b28";
    l.models.deploy(&l.ctx(), l.gemma(id, false)).await.unwrap();
    l.wait_state(id, DeploymentState::Ready, 60).await;
    let t = wait_task(&l.tasks, &l.models.unload(&l.ctx(), id).await.unwrap(), 60).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    l.f.open(&for_this_build(wifi_fence()));
    let (_, r) = fleet_run(&l.ops, "cell3", "sudo nmcli radio wifi off", true).await;
    assert_eq!(r.state, "ok", "{r:?}");

    // The last fence closed (Settings › Fence › Close fence), then the capture after.
    fence::close(&l.f.dir, &l.f.config, now_ms()).unwrap();
    // The capture after: its diff names exactly cell2's holds and cell3's addresses. Nothing in
    // docker ps, /dev/shm, MemAvailable or the listeners on cell2: the load left nothing behind.
    let node_changes = |rows: &[audit::Entry]| rows.iter().filter(|e| e.changes && e.node != "mac").count();
    let changes_before = node_changes(&l.live.audit.entries());
    let after = crate::evidence::capture_live(&l.live, &both, &logs, "p1", "after-session-b").await.unwrap();
    let diff = std::fs::read_to_string(after.diff.as_deref().expect("a diff against before-session-b")).unwrap();
    assert!(diff.starts_with("Compared with "), "{diff}");
    assert_eq!(changed_files(&diff, Path::new(&after.path)), ["cell2/apt-holds.txt", "cell3/ip-addr.txt"], "{diff}");
    assert!(diff.contains("-wlP9s9           UP             192.168.50.229/24 \n+wlP9s9           DOWN           \n"), "{diff}");
    assert!(STACK.iter().all(|p| diff.contains(&format!("+{p}\n"))), "{diff}");
    // The capture itself changed nothing: one app row, no change row on a node.
    let rows = l.live.audit.entries();
    assert_eq!(node_changes(&rows), changes_before);
    assert_eq!(rows[0].title, "Evidence captured");
    assert!(rows[0].command.starts_with("after-session-b · 2 nodes · "), "{:?}", rows[0]);

    // What the capture doesn't read, listed through Fleet with no fence open: a typed read.
    let ls = "ls -la /usr/local/lib/sparkbox /var/log/sparkbox ~/.sparkbox ~/.sparkbox/keys";
    for node in ["cell2", "cell3"] {
        let (id, _) = fleet_run(&l.ops, node, ls, false).await;
        let row = rows_of(&l.live, &id).into_iter().find(|e| e.kind == "stream").expect("its row");
        assert_eq!((row.command.as_str(), row.fence.as_deref(), row.class.as_str()), (crate::ops::fleet::wrap(ls, 30, false).as_str(), None, "change"), "{node}");
        assert_eq!(l.verdict(&crate::ops::fleet::wrap(ls, 30, false)), (Verdict::Read, true));
    }
    // cell2's key folder is empty after the unload.
    assert_eq!(std::fs::read_dir(l.node.home.join(".sparkbox/keys")).unwrap().count(), 0);
    assert!(refusals(&l.live).is_empty(), "{:#?}", refusals(&l.live));
}
