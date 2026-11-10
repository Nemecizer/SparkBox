//! Round 2's node operations against fake nodes (real SSH, real shell, the fake's sudo and
//! systemctl): a restart declared at once and proven by a new boot id (P1.14), a Fleet power run
//! as planned work (P1.15), daemon-reload under a service fence (P1.17), and the PID guard's
//! process-test class, with the classes the call sites tag their commands with. Round 3's checks
//! before every reboot (P1.26): the node window's restart and Fleet's refuse before any sudo when
//! the kernel that boots next can't come up with its GPU. Every fake node that restarts answers
//! those reads as a healthy GB10 (`gb10`).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::fleet::{FleetSpec, Strategy};
use super::prereboot::fake::{self as gb10_fake, Gb10, K};
use super::*;
use crate::fake_node::{connect, fake_node, real_shell, temp, Node, Script};
use crate::inventory::Inventory;
use crate::live::exit::Enforce;
use crate::live::fence::{self, Spec};
use crate::tasks::{StepState, Task, TaskState, Tasks};

fn demo() -> Arc<Mutex<DemoCluster>> {
    Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())))
}

/// Ops over fake nodes, its snapshots sent through `power_overlay` as lib.rs publish sends them.
fn published(live: Arc<Live>, inventory: Inventory, poll: Duration) -> Arc<Ops> {
    let ops = Ops::with_poll(live.clone(), demo(), Tasks::new(None), poll);
    let me = Arc::downgrade(&ops);
    ops.set_snapshot_source(move || {
        let mut snap = live.snapshot(&inventory);
        me.upgrade()?.power_overlay(&mut snap);
        Some(snap)
    });
    ops
}

/// A GB10 whose checks before a restart answer as `state` (prereboot.rs `fake`); anything else
/// runs for real on the fake node's shell.
fn gb10(state: Gb10) -> Script {
    Arc::new(move |cmd: &str, _: &[u8]| gb10_fake::answer(state, cmd))
}

/// The node's collector frames from now on: this boot id and uptime, every 200 ms.
fn set_frame(node: &Node, boot: &str, uptime_s: u64) {
    let mut v: Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    v["bootId"] = json!(boot);
    v["system"]["uptimeS"] = json!(uptime_s);
    v["system"]["driver"] = json!("580.178.04");
    *node.knobs.frame.lock().unwrap() = Some(v.to_string());
    node.knobs.frame_every_ms.store(200, std::sync::atomic::Ordering::SeqCst);
}

async fn wait_task(ops: &Ops, id: &str, limit: Duration) -> Task {
    let started = Instant::now();
    loop {
        let t = ops.tasks().get(id).unwrap();
        if t.finished() {
            return t;
        }
        assert!(started.elapsed() < limit, "task {} still running: {:?}", t.title, t.steps);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn wait_for_boot(ops: &Ops, node: &str, boot: &str) {
    let started = Instant::now();
    while !ops.node_view(node).is_some_and(|n| n.status == LinkStatus::Online && n.boot_id.as_deref() == Some(boot)) {
        assert!(started.elapsed() < Duration::from_secs(20), "{node} never showed {boot}: {:?}", ops.node_view(node).map(|n| (n.status, n.boot_id)));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Plays a GB10 restarting once a command containing `trigger` reaches it: what SparkBox shows
/// of the node at that moment is kept (`look`), then sshd goes and every connection drops, and
/// after `down` it's back on boot `boot` (None: the same boot, its uptime starting over anyway).
fn restart_on(node: &Node, trigger: &'static str, boot: Option<&'static str>, down: Duration, look: impl Fn() -> Option<NodeSnapshot> + Send + 'static) -> tokio::task::JoinHandle<(Instant, Option<NodeSnapshot>)> {
    let node = node.clone();
    tokio::spawn(async move {
        while node.ran(trigger).is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let (at, seen) = (Instant::now(), look());
        let flag = node.state_dir.join("sshd-stopped");
        std::fs::write(&flag, "").unwrap();
        node.drop_connections().await;
        tokio::time::sleep(down).await;
        let same: Value = serde_json::from_str(&node.knobs.frame.lock().unwrap().clone().unwrap()).unwrap();
        set_frame(&node, boot.unwrap_or(same["bootId"].as_str().unwrap()), 4);
        std::fs::remove_file(&flag).unwrap();
        (at, seen)
    })
}

// ---- P1.14 Live power state and boot-id proof

#[tokio::test]
async fn a_restart_is_declared_at_once_and_proven_by_a_new_boot_id() {
    let (node, port) = fake_node(&temp("r2ops-boot-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r2ops-boot-app"), &[("cell1", port)]).await;
    let ops = published(live, inventory, Duration::from_millis(20));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let look = { let ops = ops.clone(); move || ops.node_view("cell1") };
    let restart = restart_on(&node, "systemctl reboot", Some("boot-b"), Duration::from_millis(300), look);

    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    // The snapshot after the command reached it: still answering on its old boot, already restarting.
    let (_, seen) = restart.await.unwrap();
    let seen = seen.unwrap();
    assert_eq!((seen.status, seen.power_state.as_deref(), seen.boot_id.as_deref()), (LinkStatus::Online, Some("rebooting"), Some("boot-a")));
    assert_eq!(node.ran("sudo -k -S -p '' systemctl reboot").len(), 1, "through sudo, once");
    // Proven by the new boot id; how long it took to go and to come back is in the summary.
    let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Checks before the restart", "Sending the restart to cell1", "Waiting for cell1 to go offline", "Waiting for cell1 to come back (up to 5 min)", "Checking that it restarted", "Checks after the restart of cell1"]);
    assert!(t.steps.iter().all(|s| s.state == StepState::Ok));
    assert_eq!(t.steps[4].detail.as_deref(), Some("New boot boot-b"));
    let summary = t.summary.unwrap();
    assert!(summary.starts_with("cell1 is back after ") && summary.contains(" (offline ") && summary.contains("after the restart was sent, back on a new boot "), "{summary}");
    // On its new boot nothing is declared any more; the command went to the node, so look-only wouldn't stop it.
    let after = ops.node_view("cell1").unwrap();
    assert_eq!((after.power_state, after.boot_id.as_deref()), (None, Some("boot-b")));
    assert!(t.on_node);
    // A second restart while one is declared is refused.
    ops.declare(false, "cell1", "rebooting");
    assert_eq!(ops.power(false, "cell1", "poweroff", 0).unwrap_err().message, "cell1 is already restarting.");
    ops.undeclare("cell1");
}

#[tokio::test]
async fn a_restart_that_never_takes_the_node_down_didnt_happen() {
    let (node, port) = fake_node(&temp("r2ops-stays-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r2ops-stays-app"), &[("cell1", port)]).await;
    let ops = published(live, inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    // Declared while the restart is on its way, though the node never goes.
    let started = Instant::now();
    while ops.node_view("cell1").and_then(|n| n.power_state).is_none() {
        assert!(started.elapsed() < Duration::from_secs(5), "never declared");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    assert_eq!(t.state, TaskState::Failed);
    assert!(t.error.unwrap().starts_with("The restart didn't happen: cell1 kept answering for a minute."));
    assert_eq!(node.ran("systemctl reboot").len(), 1);
    assert_eq!(ops.node_view("cell1").unwrap().power_state, None, "the task is over: nothing declared");
}

#[tokio::test]
async fn a_node_back_on_its_old_boot_didnt_restart_whatever_its_uptime_says() {
    let (node, port) = fake_node(&temp("r2ops-same-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r2ops-same-app"), &[("cell1", port)]).await;
    let ops = published(live, inventory, Duration::from_millis(25));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    // It drops and answers again with its uptime at 4 s, but on the boot it had.
    let restart = restart_on(&node, "systemctl reboot", None, Duration::from_millis(200), || None);
    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    restart.await.unwrap();
    assert_eq!(t.state, TaskState::Failed, "{:?}", t.steps);
    assert_eq!(t.error.as_deref(), Some("cell1 answers again on the boot it had before (its boot id didn't change), so it didn't restart."));
    let n = ops.node_view("cell1").unwrap();
    assert_eq!((n.system.uptime_s, n.power_state), (4, None));
}

// ---- P1.15 Fleet power verbs open maintenance windows

#[tokio::test]
async fn restart_reattaches_over_ssh_even_when_snapshots_miss_the_offline_interval() {
    let dir = temp("restart-reattach-app");
    let (node, port) = fake_node(&temp("restart-reattach-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&dir, &[("cell1", port)]).await;
    let ops = published(live.clone(), inventory.clone(), Duration::from_millis(100));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    // Coalesced delivery: the consumer only sees the last online frame until fresh readings
    // arrive on the next boot. SSH really drops and must reconnect without any reconnect call.
    let last = Mutex::new(live.snapshot(&inventory));
    let reader = live.clone();
    ops.set_snapshot_source(move || {
        let next = reader.snapshot(&inventory);
        let mut last = last.lock().unwrap();
        if next.nodes[0].status == LinkStatus::Online && !next.nodes[0].stale { *last = next; }
        Some(last.clone())
    });
    let connections = node.total_connections();
    let restart = restart_on(&node, "systemctl reboot", Some("boot-b"), Duration::from_millis(100), || None);
    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    restart.await.unwrap();
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    assert!(t.summary.unwrap().contains("back on a new boot"));
    assert!(t.steps.iter().any(|s| s.detail.as_deref().is_some_and(|d| d.contains("offline interval was not observed"))));
    assert_eq!(ops.node_view("cell1").unwrap().boot_id.as_deref(), Some("boot-b"));
    assert!(node.total_connections() > connections, "SSH must reconnect automatically");
    assert_eq!(node.ran("sudo -k -S -p '' systemctl reboot").len(), 1);
    live.stop();
}

#[tokio::test]
async fn restart_requires_a_fresh_nonempty_boot_baseline_before_sending_anything() {
    let (node, port) = fake_node(&temp("restart-baseline-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("restart-baseline-app"), &[("cell1", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(5));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let original = ops.cluster().unwrap();
    for (boot, stale) in [(None, false), (Some(" "), false), (Some("boot-a"), true)] {
        let mut snap = original.clone();
        snap.nodes[0].boot_id = boot.map(String::from);
        snap.nodes[0].stale = stale;
        ops.set_snapshot_source(move || Some(snap.clone()));
        assert!(ops.power(false, "cell1", "reboot", 0).unwrap_err().message.contains("Nothing was sent"));
    }
    assert!(node.ran("systemctl reboot").is_empty());
    live.stop();
}

#[tokio::test]
async fn restart_never_uses_a_stale_or_empty_new_boot_as_proof() {
    for (tag, boot, stale) in [("stale", "boot-b", true), ("empty", "", false)] {
        let (node, port) = fake_node(&temp(&format!("restart-{tag}-home")), gb10(Gb10::Healthy)).await;
        set_frame(&node, "boot-a", 50_000);
        let (live, inventory) = connect(&temp(&format!("restart-{tag}-app")), &[("cell1", port)]).await;
        let ops = published(live.clone(), inventory, Duration::from_millis(5));
        wait_for_boot(&ops, "cell1", "boot-a").await;
        let snap = ops.cluster().unwrap();
        let observed = node.clone();
        ops.set_snapshot_source(move || {
            let mut snap = snap.clone();
            if !observed.ran("systemctl reboot").is_empty() {
                snap.nodes[0].boot_id = Some(boot.into());
                snap.nodes[0].stale = stale;
            }
            Some(snap)
        });
        let id = ops.power(false, "cell1", "reboot", 0).unwrap();
        let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
        assert_eq!(t.state, TaskState::Failed, "{tag}: {:?}", t.summary);
        assert_eq!(node.ran("sudo -k -S -p '' systemctl reboot").len(), 1);
        live.stop();
    }
}

#[tokio::test]
async fn restart_timeout_keeps_reconnecting_without_sending_a_second_restart() {
    let (node, port) = fake_node(&temp("restart-late-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("restart-late-app"), &[("cell1", port)]).await;
    // Short task deadline; the real connection loop still uses its normal retry interval.
    let ops = published(live.clone(), inventory, Duration::from_millis(2));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let restart = restart_on(&node, "systemctl reboot", Some("boot-b"), Duration::from_millis(500), || None);
    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    assert_eq!(t.state, TaskState::Failed, "{:?}", t.summary);
    assert!(t.error.unwrap().contains("keeps trying to reconnect; it won't send another restart"));
    restart.await.unwrap();
    wait_for_boot(&ops, "cell1", "boot-b").await;
    assert_eq!(ops.tasks().get(&id).unwrap().state, TaskState::Failed, "a late connection does not rewrite the timed-out task");
    assert_eq!(node.ran("sudo -k -S -p '' systemctl reboot").len(), 1);
    live.stop();
}

/// Planned work as the app keeps it (health::maintenance's Live windows), and what was said in
/// which order. The agents' note says whether it left inside the Fleet run's typed scope.
#[derive(Default)]
struct Windows(Mutex<Vec<(Instant, String)>>);

impl Windows {
    fn note(&self, what: String) {
        self.0.lock().unwrap().push((Instant::now(), what));
    }

    fn said(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|(_, s)| s.clone()).collect()
    }

    fn when(&self, what: &str) -> Instant {
        self.0.lock().unwrap().iter().find(|(_, s)| s.starts_with(what)).unwrap().0
    }
}

impl Planned for Windows {
    fn begin(&self, node: &str, until_ms: Option<u64>, reason: &str) {
        crate::health::maintenance::begin_at(false, node, until_ms, reason, now_ms());
        self.note(format!("begin {node} {reason}{}", if until_ms.is_some() { " until" } else { "" }));
    }

    fn notify_agents<'a>(&'a self, _op_id: &'a str, nodes: &'a [String], _: Option<u64>, _: &'a str) -> BoxFuture<'a, Vec<(String, String)>> {
        Box::pin(async move {
            self.note(format!("agents told about {}{}", nodes.join(","), if crate::live::audit::in_typed() { " (typed)" } else { "" }));
            Vec::new()
        })
    }

    fn end(&self, node: &str) {
        crate::health::maintenance::end_at(false, node);
        self.note(format!("end {node}"));
    }

    fn resolved(&self, _: &str, _: &str, _: &str) {}
}

/// A GB10 for Fleet: `sudo reboot` is answered (a stand-in `reboot` is first on its PATH too),
/// so nothing on this Mac is ever asked to restart; its checks before a restart answer as `state`.
fn restartable_as(state: Gb10) -> Script {
    Arc::new(move |cmd: &str, _: &[u8]| gb10_fake::answer(state, cmd).or_else(|| cmd.ends_with("sh -c 'reboot'").then(|| (String::new(), 0))))
}

fn restartable() -> Script {
    restartable_as(Gb10::Healthy)
}

// The Live windows are process-wide: the lock keeps the other tests that stamp Live snapshots
// from closing these (and this one's from closing theirs) for the whole run.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_fleet_restart_of_two_nodes_is_planned_work_one_node_at_a_time() {
    let _live = crate::health::maintenance::live_test_lock();
    let (a, pa) = fake_node(&temp("r2ops-fleet-a"), restartable()).await;
    let (b, pb) = fake_node(&temp("r2ops-fleet-b"), restartable()).await;
    for (n, boot) in [(&a, "a-1"), (&b, "b-1")] {
        n.bin("reboot", "#!/bin/sh\nexit 0\n");
        set_frame(n, boot, 50_000);
    }
    let (live, inventory) = connect(&temp("r2ops-fleet-app"), &[("r2f-1", pa), ("r2f-2", pb)]).await;
    let ops = published(live, inventory, Duration::from_millis(20));
    wait_for_boot(&ops, "r2f-1", "a-1").await;
    wait_for_boot(&ops, "r2f-2", "b-1").await;
    let windows = Arc::new(Windows::default());
    ops.set_planned(windows.clone());
    // r2f-1 goes down before its answer is sent (no exit status); r2f-2 answers first.
    a.close_without_status("sh -c 'reboot'");
    let ra = restart_on(&a, "sh -c 'reboot'", Some("a-2"), Duration::from_millis(300), || None);
    let rb = restart_on(&b, "sh -c 'reboot'", Some("b-2"), Duration::from_millis(300), || None);
    // What the alerting is handed meanwhile: each snapshot stamped with the windows, as on_publish does.
    let away: Arc<Mutex<Vec<ClusterSnapshot>>> = Arc::default();
    let sampler = {
        let (ops, away) = (ops.clone(), away.clone());
        tokio::spawn(async move {
            loop {
                if let Some(mut s) = ops.cluster() {
                    crate::health::maintenance::stamp(&mut s, &[], 120_000);
                    if s.nodes.iter().any(|n| n.status != LinkStatus::Online) {
                        away.lock().unwrap().push(s);
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    };

    let spec = FleetSpec { nodes: vec!["r2f-1".into(), "r2f-2".into()], command: "reboot".into(), sudo: true, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(false, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(60)).await;
    sampler.abort();
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    let ((a_sent, _), (b_sent, _)) = (ra.await.unwrap(), rb.await.unwrap());

    // One window per node, opened before its restart went out and closed once it was back,
    // the agents told first (outside the run's typed scope); the second node only after the first.
    assert_eq!(
        windows.said(),
        ["begin r2f-1 restarting until", "agents told about r2f-1", "end r2f-1", "begin r2f-2 restarting until", "agents told about r2f-2", "end r2f-2"]
    );
    assert!(windows.when("agents told about r2f-1") < a_sent && windows.when("agents told about r2f-2") < b_sent);
    assert!(windows.when("end r2f-1") < b_sent, "one node at a time");
    assert!(crate::health::maintenance::windows(false).keys().all(|n| !n.starts_with("r2f-")), "both closed");
    assert!(t.log.iter().any(|l| l == "r2f-1: the connection closed as the command ran"), "{:?}", t.log);
    let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(
        labels[..6],
        ["Checks before the restart of r2f-1", "Telling the nodes' agents about the planned work", "r2f-1 (1 of 2)", "Waiting for r2f-1 to go away", "Waiting for r2f-1 to come back (up to 6 min)", "Logging in to r2f-1 again over SSH"]
    );
    assert!(labels.contains(&"Checks before the restart of r2f-2"), "each node is checked before its own restart: {labels:?}");
    assert_eq!(ops.node_view("r2f-2").unwrap().boot_id.as_deref(), Some("b-2"));

    // While r2f-1 was away its offline alert was Planned: counted nowhere, paged to nobody.
    let snap = away.lock().unwrap().iter().find(|s| s.nodes[0].status != LinkStatus::Online).cloned().expect("r2f-1 was seen away");
    assert!(snap.nodes[0].maintenance.as_ref().is_some_and(|m| m.reason == "restarting"), "{:?}", snap.nodes[0].maintenance);
    let mut e = crate::health::Engine::default();
    let mut events = Vec::new();
    let mut s = snap.clone();
    for _ in 0..90 {
        s.ts_ms += 1000;
        events.extend(e.step(&s, s.ts_ms));
    }
    let v = e.view(s.ts_ms, Some(&s));
    let off = v.alerts.iter().find(|x| x.rule == "node-offline" && x.entity.id == "r2f-1").expect("offline raised");
    assert!(off.planned.as_deref().is_some_and(|p| p.starts_with("r2f-1 restarting (SparkBox, ")) && !off.counted, "{off:?}");
    let offline: Vec<_> = events.iter().filter(|ev| ev.alert.rule == "node-offline").collect();
    assert!(!offline.is_empty() && offline.iter().all(|ev| ev.alert.muted(ev.ts_ms)), "{events:?}");
}

// ---- P1.17 daemon-reload

/// A signed fence this build trusts, under `base` (as live/exit_tests.rs opens them).
struct TestFence {
    rules: Enforce,
    key: p256::ecdsa::SigningKey,
    file: fence::KeyFile,
}

impl TestFence {
    fn new(base: &Path) -> TestFence {
        let rules = Enforce {
            instance: "release".into(),
            fence_dir: base.join("SparkBox Fence"),
            config_dir: Some(base.join("config-release")),
            noauth: false,
            tag: "p1-r2-test".into(),
            mac: Some("Studio".into()),
        };
        std::fs::create_dir_all(&rules.fence_dir).unwrap();
        let key = fence::new_key().unwrap();
        let (key_id, public_key) = fence::public_of(&key).unwrap();
        let file = fence::KeyFile { key_id, public_key, kind: "keychain".into() };
        fence::save_trust(rules.config_dir.as_ref().unwrap(), &file).unwrap();
        TestFence { rules, key, file }
    }

    async fn open(&self, nodes: &[&str], classes: &[FenceClass], only: &[(FenceClass, &[&str])]) {
        // A new record a moment after the last, as a new fence is.
        tokio::time::sleep(Duration::from_millis(5)).await;
        let mut spec = Spec { nodes: nodes.iter().map(|n| n.to_string()).collect(), classes: classes.to_vec(), minutes: 180, instance: "release".into(), ..Default::default() };
        for (c, items) in only {
            spec.only.insert(*c, items.iter().map(|i| i.to_string()).collect());
        }
        let r = fence::record_for(&spec, &self.key, now_ms(), "Studio").unwrap();
        fence::write(&self.rules.fence_dir, self.rules.config_dir.as_ref().unwrap(), &self.file, &r).unwrap();
    }
}

fn row(live: &Live, command_part: &str) -> crate::live::audit::Entry {
    live.audit.entries().into_iter().find(|e| e.command.contains(command_part) && e.title != "Refused").unwrap_or_else(|| panic!("no row for {command_part}"))
}

#[tokio::test]
async fn daemon_reload_runs_through_sudo_and_a_service_fence_for_two_units_admits_it() {
    let (node, port) = fake_node(&temp("r2ops-reload-home"), real_shell()).await;
    let base = temp("r2ops-reload-app");
    let (live, inventory) = connect(&base, &[("cell2", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(5));
    let f = TestFence::new(&base);
    live.enforce_writes(f.rules.clone());
    // Session A's service class: earlyoom and cups only.
    f.open(&["cell2"], &[FenceClass::Service], &[(FenceClass::Service, &["earlyoom.service", "cups.service"])]).await;

    ops.service_action(false, "cell2", "", "daemon-reload").await.unwrap();
    assert_eq!(node.ran("sudo -k -S -p '' systemctl daemon-reload").len(), 1);
    assert!(node.stdin.lock().unwrap().iter().any(|(c, pw)| c.ends_with("systemctl daemon-reload") && pw == b"pw\n"), "the password on stdin");
    let told = std::fs::read_to_string(node.state_dir.join("systemctl.log")).unwrap();
    assert_eq!(told.lines().filter(|l| l.contains("daemon-reload")).collect::<Vec<_>>(), ["daemon-reload"], "the node's systemctl got exactly that");
    let r = row(&live, "systemctl daemon-reload");
    assert_eq!((r.fence.as_deref(), r.escalated, r.title.as_str()), (Some("service"), true, "Reload the unit files on cell2"));
    let task = ops.tasks().list().into_iter().find(|t| t.title == "Reload the unit files on cell2").unwrap();
    assert_eq!((task.state, task.summary.as_deref()), (TaskState::Ok, Some("Unit files reloaded")));

    // Across nodes too, with no unit; a unit with it is refused before anything runs.
    let id = ops.services_apply(false, vec!["cell2".into()], String::new(), "daemon-reload".into()).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!((t.state, t.title.as_str()), (TaskState::Ok, "Reload the unit files on 1 node"));
    assert_eq!(node.ran("systemctl daemon-reload").len(), 2);
    assert!(ops.service_action(false, "cell2", "ssh.service", "daemon-reload").await.unwrap_err().message.contains("acts on no service"));
    assert_eq!(services::service_impact("", "daemon-reload", &[services::NodeCtx { name: "cell2".into(), ..Default::default() }], false).title, "Reload the unit files on cell2?");

    // The `service` tag keeps the unit a restart names: one outside the list is still refused.
    let e = ops.service_action(false, "cell2", "ssh.service", "restart").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow service changes to ssh.service: it names only earlyoom.service, cups.service."));
    assert!(node.ran("restart -- 'ssh.service'").is_empty());
    // Demo has no daemon-reload of its own.
    assert_eq!(ops.service_action(true, "cell2", "", "daemon-reload").await.unwrap_err().message, "Available with Live data.");
}

// ---- The PID guard and the process-test class

/// A realistic /proc/<pid>/stat line (52 fields, Linux 6.x) for a process named `comm`, started
/// `start` clock ticks after boot (field 22).
fn stat_line(pid: u32, comm: &str, start: u64) -> String {
    let mut fields: Vec<String> = vec![pid.to_string(), format!("({comm})")];
    fields.extend("S 1 4242 4242 0 -1 4194560 112 0 0 0 3 1 0 0 20 0 1 0".split(' ').map(String::from));
    fields.push(start.to_string());
    fields.extend("5726208 300 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0".split(' ').map(String::from));
    assert_eq!(fields.len(), 52);
    fields.join(" ")
}

#[tokio::test]
async fn the_pid_guard_reads_the_start_time_after_a_name_with_spaces_and_parens() {
    let proc_root = temp("r2ops-proc");
    let run = |cmd: String| std::process::Command::new("/bin/dash").args(["-c", &cmd]).output().unwrap();
    for comm in ["(a) b", "x) y", "python3"] {
        // A process of our own to signal, with the stat line the kernel would write for it.
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        std::fs::create_dir_all(proc_root.join(pid.to_string())).unwrap();
        std::fs::write(proc_root.join(format!("{pid}/stat")), stat_line(pid, comm, 987_654) + "\n").unwrap();
        let root = proc_root.display().to_string();
        // Another start time: the PID belongs to another program now; it's left alone.
        let out = run(pid_guard_cmd(pid, "TERM", Some(987_655), &root));
        assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stderr).trim()), (Some(3), "gone"), "{comm}");
        assert!(child.try_wait().unwrap().is_none(), "{comm}: still running");
        // Its own start time: signalled.
        let out = run(pid_guard_cmd(pid, "TERM", Some(987_654), &root));
        assert_eq!(out.status.code(), Some(0), "{comm}: {}", String::from_utf8_lossy(&out.stderr));
        let status = child.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(15), "{comm}");
    }
}

/// A fake node where the guarded kill of 4242 (and its renice) answer as on a GB10.
fn processes() -> Script {
    Arc::new(|cmd: &str, _: &[u8]| {
        (cmd.contains("/proc/4242/stat") && cmd.ends_with("kill -s TERM 4242") || cmd.ends_with("renice -n 5 -p 4242")).then(|| (String::new(), 0))
    })
}

#[tokio::test]
async fn a_guarded_signal_is_a_process_test_change_and_only_that_fence_admits_it() {
    let (node, port) = fake_node(&temp("r2ops-pid-home"), processes()).await;
    let base = temp("r2ops-pid-app");
    let (live, inventory) = connect(&base, &[("cell1", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(5));
    let f = TestFence::new(&base);
    live.enforce_writes(f.rules.clone());

    // Without process-test: refused at the exit, and the node never saw it.
    f.open(&["cell1"], &[FenceClass::Service, FenceClass::Shell], &[]).await;
    let e = ops.signal(false, "cell1", 4242, "TERM", Some(777)).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow process signals."));
    assert!(node.ran("kill -s TERM 4242").is_empty());
    assert_eq!(ops.renice(false, "cell1", 4242, 5).await.unwrap_err().kind, ErrorKind::Fenced);

    // Under process-test: the guarded command (several commands, so its own shape is `other`)
    // goes through, logged as process-test.
    f.open(&["cell1"], &[FenceClass::ProcessTest], &[]).await;
    ops.signal(false, "cell1", 4242, "TERM", Some(777)).await.unwrap();
    let sent = node.ran("kill -s TERM 4242");
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with("s=$(sed 's/^.*) //' /proc/4242/stat 2>/dev/null | cut -d' ' -f20); [ \"$s\" = \"777\" ]"), "{}", sent[0]);
    assert_eq!(row(&live, "kill -s TERM 4242").fence.as_deref(), Some("process-test"));
    ops.renice(false, "cell1", 4242, 5).await.unwrap();
    assert_eq!(row(&live, "renice -n 5 -p 4242").fence.as_deref(), Some("process-test"));
}

// ---- A shell is the shell class

#[tokio::test]
async fn a_terminal_shell_needs_a_fence_with_shells() {
    let (_node, port) = fake_node(&temp("r2ops-shell-home"), real_shell()).await;
    let base = temp("r2ops-shell-app");
    let (live, _) = connect(&base, &[("cell1", port)]).await;
    let f = TestFence::new(&base);
    live.enforce_writes(f.rules.clone());
    let terms = crate::term::Terminals::new(live.clone());
    let emit = || -> crate::term::Emit { Box::new(|_| {}) };
    f.open(&["cell1"], &[FenceClass::Service], &[]).await;
    let e = SbError::from(terms.open(false, "cell1", crate::term::Target::Shell, 80, 24, emit()).await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow shells."));
    f.open(&["cell1"], &[FenceClass::Shell], &[]).await;
    let id = terms.open(false, "cell1", crate::term::Target::Shell, 80, 24, emit()).await.unwrap();
    terms.close(&id);
    // A log follower only reads: no fence needed for it.
    f.open(&["cell1"], &[FenceClass::Service], &[]).await;
    let id = terms.open(false, "cell1", crate::term::Target::Journal { unit: "ollama.service".into(), since: None }, 80, 24, emit()).await.unwrap();
    terms.close(&id);
}

// ---- R2-OPS fixes: the root retry under process-test, mask under a service fence, Fleet's
// shutdown and Verify, one power command at a time, a scheduled one the node won't call off

/// `published`, with a scheduled restart's minute this long.
fn published_timed(live: Arc<Live>, inventory: Inventory, poll: Duration, minute: Duration) -> Arc<Ops> {
    let ops = Ops::with_timing(live.clone(), demo(), Tasks::new(None), poll, minute);
    let me = Arc::downgrade(&ops);
    ops.set_snapshot_source(move || {
        let mut snap = live.snapshot(&inventory);
        me.upgrade()?.power_overlay(&mut snap);
        Some(snap)
    });
    ops
}

/// What planned work was said, in order and when, without touching the process's maintenance
/// windows. The agents take `agents` to answer (AppPlanned allows them up to 6 s).
#[derive(Default)]
struct Said {
    agents: Duration,
    lines: Mutex<Vec<(Instant, String)>>,
}

impl Said {
    fn new(agents: Duration) -> Arc<Said> {
        Arc::new(Said { agents, ..Default::default() })
    }

    fn note(&self, what: String) {
        self.lines.lock().unwrap().push((Instant::now(), what));
    }

    fn said(&self) -> Vec<String> {
        self.lines.lock().unwrap().iter().map(|(_, s)| s.clone()).collect()
    }

    fn when(&self, what: &str) -> Instant {
        self.lines.lock().unwrap().iter().find(|(_, s)| s == what).unwrap_or_else(|| panic!("never said {what}")).0
    }
}

impl Planned for Said {
    fn begin(&self, node: &str, _: Option<u64>, reason: &str) {
        self.note(format!("begin {node} {reason}"));
    }

    fn notify_agents<'a>(&'a self, _: &'a str, nodes: &'a [String], _: Option<u64>, _: &'a str) -> BoxFuture<'a, Vec<(String, String)>> {
        Box::pin(async move {
            tokio::time::sleep(self.agents).await;
            self.note(format!("agents told about {}", nodes.join(",")));
            Vec::new()
        })
    }

    fn end(&self, node: &str) {
        self.note(format!("end {node}"));
    }

    fn resolved(&self, _: &str, _: &str, _: &str) {}
}

/// A process this Mac's root owns: the cluster user's signal to it is refused (EPERM), as for
/// a root daemon on a GB10, so a SIGCONT from these tests reaches nothing. One of the daemons
/// that run as long as the Mac is up, when there is one (a short-lived root process can exit
/// before the signal, and `kill` then says "No such process").
fn root_pid() -> u32 {
    let out = std::process::Command::new("ps").args(["-A", "-o", "pid=,uid=,comm="]).output().unwrap();
    let roots: Vec<(u32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (pid, uid, comm) = (it.next()?.parse::<u32>().ok()?, it.next()?, it.next().unwrap_or(""));
            (uid == "0" && pid > 1).then(|| (pid, comm.to_string()))
        })
        .collect();
    let lasting = ["/usr/libexec/configd", "/usr/libexec/logd", "/usr/sbin/notifyd", "/usr/sbin/syslogd"];
    roots.iter().find(|(_, comm)| lasting.contains(&comm.as_str())).or(roots.first()).map(|(pid, _)| *pid).expect("a root process")
}

/// util-linux renice as the cluster user: only root may lower a nice value.
const RENICE: &str = r#"#!/bin/sh
# renice -n N -p PID
n="$2"; pid="$4"
if [ "$SB_FAKE_ROOT" != 1 ] && [ "$n" -lt 0 ]; then
  echo "renice: failed to set priority for $pid (process ID): Permission denied" >&2
  exit 1
fi
echo "$pid (process ID) old priority 0, new priority $n"
"#;

#[tokio::test]
async fn a_process_test_fence_never_admits_the_root_retry_of_a_signal_or_a_renice() {
    let pid = root_pid();
    let (node, port) = fake_node(&temp("r2ops-root-home"), real_shell()).await;
    // Its /proc/<pid>/stat as the kernel writes it (this Mac has no /proc): the guard's sed reads it.
    let proc_root = temp("r2ops-root-proc");
    std::fs::create_dir_all(proc_root.join(pid.to_string())).unwrap();
    std::fs::write(proc_root.join(format!("{pid}/stat")), stat_line(pid, "sshd", 777) + "\n").unwrap();
    node.bin("sed", &format!("#!/bin/sh\ncase \"$2\" in /proc/*) exec /usr/bin/sed \"$1\" \"{}${{2#/proc}}\" ;; esac\nexec /usr/bin/sed \"$@\"\n", proc_root.display()));
    node.bin("renice", RENICE);
    let base = temp("r2ops-root-app");
    let (live, inventory) = connect(&base, &[("cell2", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(5));
    let f = TestFence::new(&base);
    live.enforce_writes(f.rules.clone());
    // T2's fence: cell2, container-test, process-test, files-test.
    f.open(&["cell2"], &[FenceClass::ContainerTest, FenceClass::ProcessTest, FenceClass::FilesTest], &[]).await;

    // The guarded signal goes as the cluster user (process-test) and the node refuses it; its
    // root retry is `other`, which the fence refuses before anything reaches the node.
    let e = ops.signal(false, "cell2", pid, "CONT", Some(777)).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."));
    let sent = node.ran(&format!("kill -s CONT {pid}"));
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].starts_with("s=$(sed "), "the user-level guard: {}", sent[0]);
    assert_eq!(row(&live, &format!("kill -s CONT {pid}")).fence.as_deref(), Some("process-test"));
    let refused = live.audit.entries().into_iter().filter(|e| e.title == "Refused" && e.command.ends_with(&format!("kill -s CONT {pid}"))).count();
    assert_eq!(refused, 1, "the root retry is in the log as refused");

    // A negative nice is root's: the same, before any sudo. A positive one stays user-level.
    let e = ops.renice(false, "cell2", pid, -5).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."));
    assert_eq!(node.ran(&format!("renice -n -5 -p {pid}")).len(), 1);
    ops.renice(false, "cell2", pid, 5).await.unwrap();
    assert!(node.ran("sudo ").is_empty(), "no sudo reached the node: {:?}", node.ran("sudo "));

    // The release build's `any` fence admits the root retry: through sudo, the password on stdin.
    f.open(&["cell2"], &[FenceClass::Any], &[]).await;
    ops.renice(false, "cell2", pid, -5).await.unwrap();
    assert_eq!(node.ran(&format!("sudo -k -S -p '' renice -n -5 -p {pid}")).len(), 1);
    assert!(node.stdin.lock().unwrap().iter().any(|(c, pw)| c.ends_with(&format!("renice -n -5 -p {pid}")) && pw == b"pw\n"));
    let root = live.audit.entries().into_iter().find(|e| e.kind == "sudo" && e.command == format!("renice -n -5 -p {pid}")).expect("the sudo row");
    assert_eq!((root.fence.as_deref(), root.escalated), (Some("other"), true));
}

#[tokio::test]
async fn a_service_fence_admits_no_mask_unmask_or_reset_failed() {
    let (node, port) = fake_node(&temp("r2ops-mask-home"), real_shell()).await;
    let base = temp("r2ops-mask-app");
    let (live, inventory) = connect(&base, &[("cell2", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(5));
    let f = TestFence::new(&base);
    live.enforce_writes(f.rules.clone());
    // A service fence without a unit list.
    f.open(&["cell2"], &[FenceClass::Service], &[]).await;
    for action in ["mask", "unmask", "reset-failed"] {
        let e = ops.service_action(false, "cell2", "ssh.service", action).await.unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."), "{action}");
    }
    assert!(node.ran("'ssh.service'").is_empty(), "{:?}", node.ran("'ssh.service'"));
    // A restart is a service change: admitted as one.
    ops.service_action(false, "cell2", "cups.service", "restart").await.unwrap();
    assert_eq!(row(&live, "systemctl restart -- 'cups.service'").fence.as_deref(), Some("service"));
    // Under `any`, a mask goes as what it is.
    f.open(&["cell2"], &[FenceClass::Any], &[]).await;
    ops.service_action(false, "cell2", "ssh.service", "mask").await.unwrap();
    assert_eq!(node.ran("sudo -k -S -p '' systemctl mask -- 'ssh.service'").len(), 1);
    assert_eq!(row(&live, "systemctl mask -- 'ssh.service'").fence.as_deref(), Some("other"));
}

/// systemd's shutdown: -r/-P +N schedules through logind, -c calls it off. With `logind-down`
/// in the node's state folder, logind doesn't answer and nothing is called off.
const SHUTDOWN: &str = r#"#!/bin/sh
case "$1" in
  -c)
    if [ -e "$SB_FAKE_STATE/logind-down" ]; then
      echo "Failed to talk to logind, shutdown hasn't been cancelled: Connection timed out" >&2
      exit 1
    fi ;;
  -r) echo "Reboot scheduled for $(date), use 'shutdown -c' to cancel." >&2 ;;
  -P) echo "Shutdown scheduled for $(date), use 'shutdown -c' to cancel." >&2 ;;
esac
exit 0
"#;

#[tokio::test]
async fn a_second_power_command_is_refused_while_the_first_tells_the_agents_or_waits() {
    let (node, port) = fake_node(&temp("r2ops-claim-home"), gb10(Gb10::Healthy)).await;
    node.bin("shutdown", SHUTDOWN);
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r2ops-claim-app"), &[("cell1", port)]).await;
    let ops = published(live, inventory, Duration::from_millis(20));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    ops.set_planned(Said::new(Duration::from_millis(800)));

    let first = ops.power(false, "cell1", "reboot", 0).unwrap();
    // Its checks passed; it's telling the agents.
    let started = Instant::now();
    while !ops.tasks().get(&first).unwrap().steps.iter().any(|s| s.label == "Telling the nodes' agents about the planned work") {
        assert!(started.elapsed() < Duration::from_secs(10), "never told the agents");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // The first is still telling the agents: nothing has reached the node, and the second is refused.
    assert!(node.ran("systemctl reboot").is_empty());
    assert_eq!(ops.power(false, "cell1", "poweroff", 0).unwrap_err().message, "cell1 is already restarting.");
    assert_eq!(ops.power(false, "cell1", "reboot", 0).unwrap_err().message, "cell1 is already restarting.");
    // (This fake node never goes down: the restart didn't happen, and nothing stays declared.)
    let t = wait_task(&ops, &first, Duration::from_secs(20)).await;
    assert_eq!(t.state, TaskState::Failed);
    assert_eq!((node.ran("systemctl reboot").len(), node.ran("systemctl poweroff").len()), (1, 0), "one command reached the node");
    assert_eq!(ops.node_view("cell1").unwrap().power_state, None);

    // Scheduled: it holds the node but isn't shown (the node is up until its time).
    let id = ops.power(false, "cell1", "reboot", 5).unwrap();
    let started = Instant::now();
    while node.ran("shutdown -r +5").is_empty() {
        assert!(started.elapsed() < Duration::from_secs(10), "never scheduled");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(ops.power(false, "cell1", "poweroff", 0).unwrap_err().message, "cell1 already has a restart scheduled.");
    assert_eq!(ops.node_view("cell1").unwrap().power_state, None, "not shown while it waits");
    // Called off on the node: cancelled, and the node is free again.
    ops.tasks().cancel(&id).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!((t.state, t.summary.as_deref()), (TaskState::Cancelled, Some("The scheduled restart of cell1 was called off.")));
    assert_eq!(node.ran("sudo -k -S -p '' shutdown -c").len(), 1);
    assert!(ops.declared.lock().unwrap().get("cell1").is_none());
}

#[tokio::test]
async fn a_scheduled_restart_the_node_wont_call_off_is_followed_through_and_the_task_fails() {
    let (node, port) = fake_node(&temp("r2ops-calloff-home"), gb10(Gb10::Healthy)).await;
    node.bin("shutdown", SHUTDOWN);
    std::fs::write(node.state_dir.join("logind-down"), "").unwrap();
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r2ops-calloff-app"), &[("cell1", port)]).await;
    let minute = Duration::from_millis(1500);
    let ops = published_timed(live, inventory, Duration::from_millis(20), minute);
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());

    let id = ops.power(false, "cell1", "reboot", 1).unwrap();
    let started = Instant::now();
    while node.ran("shutdown -r +1").is_empty() {
        assert!(started.elapsed() < Duration::from_secs(10), "never scheduled");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // On the node it's due a minute (here 1.5 s) later: then it restarts, and is back on a new boot.
    let due = Instant::now() + minute;
    let restart = {
        let (node, ops) = (node.clone(), ops.clone());
        tokio::spawn(async move {
            tokio::time::sleep_until((due + Duration::from_millis(150)).into()).await;
            // SparkBox's clock started when the reply reached it, after `due`'s, so its time comes
            // by a margin no fixed wait covers on a busy machine: wait for what it declares then.
            let waited = Instant::now();
            let mut seen = ops.node_view("cell1");
            while seen.as_ref().and_then(|n| n.power_state.as_deref()) != Some("rebooting") && waited.elapsed() < Duration::from_secs(10) {
                tokio::time::sleep(Duration::from_millis(5)).await;
                seen = ops.node_view("cell1");
            }
            let at = Instant::now();
            let flag = node.state_dir.join("sshd-stopped");
            std::fs::write(&flag, "").unwrap();
            node.drop_connections().await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            set_frame(&node, "boot-b", 4);
            std::fs::remove_file(&flag).unwrap();
            (at, seen)
        })
    };
    ops.tasks().cancel(&id).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    let (went, seen) = restart.await.unwrap();

    // Not "cancelled": the restart went ahead, and the task says why it couldn't be called off.
    assert_eq!(t.state, TaskState::Failed, "{:?}", t.steps);
    let error = t.error.clone().unwrap();
    assert!(error.starts_with("Couldn't call off the restart of cell1 (Failed to talk to logind, shutdown hasn't been cancelled: Connection timed out), so it stayed due at "), "{error}");
    assert!(error.contains(". cell1 is back after "), "{error}");
    let step = t.steps.iter().find(|s| s.label == "Calling off the scheduled restart").unwrap();
    assert_eq!(step.state, StepState::Failed);
    assert!(step.detail.as_deref().unwrap().starts_with("Couldn't call off the restart: cell1 still restarts at "), "{:?}", step.detail);
    assert_eq!(node.ran("sudo -k -S -p '' shutdown -c").len(), 1);
    // Declared restarting at its time; the window and the agents' note held until it was back.
    assert_eq!(seen.and_then(|n| n.power_state).as_deref(), Some("rebooting"));
    assert_eq!(said.said(), ["begin cell1 restarting", "agents told about cell1", "end cell1"]);
    assert!(said.when("end cell1") > went);
    let after = ops.node_view("cell1").unwrap();
    assert_eq!((after.power_state, after.boot_id.as_deref()), (None, Some("boot-b")));
}

/// A GB10 for a Fleet shutdown: `sudo poweroff` is answered (a stand-in `poweroff` is first on
/// its PATH too), so nothing on this Mac is ever asked to power off.
fn switchable() -> Script {
    Arc::new(|cmd: &str, _: &[u8]| cmd.ends_with("sh -c 'poweroff'").then(|| (String::new(), 0)))
}

/// Takes the node off once a shutdown reaches it: sshd goes, every connection drops, and it
/// stays off. Returns when it went.
fn off_on_poweroff(node: &Node) -> tokio::task::JoinHandle<Instant> {
    let node = node.clone();
    tokio::spawn(async move {
        while node.ran("sh -c 'poweroff'").is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let at = Instant::now();
        std::fs::write(node.state_dir.join("sshd-stopped"), "").unwrap();
        node.drop_connections().await;
        at
    })
}

#[tokio::test]
async fn a_fleet_shutdown_of_two_nodes_is_done_once_both_are_off_and_their_windows_stay_open() {
    let (a, pa) = fake_node(&temp("r2ops-off-a"), switchable()).await;
    let (b, pb) = fake_node(&temp("r2ops-off-b"), switchable()).await;
    for (n, boot) in [(&a, "a-1"), (&b, "b-1")] {
        n.bin("poweroff", "#!/bin/sh\nexit 0\n");
        set_frame(n, boot, 50_000);
    }
    let (live, inventory) = connect(&temp("r2ops-off-app"), &[("r2o-1", pa), ("r2o-2", pb)]).await;
    let ops = published(live, inventory, Duration::from_millis(20));
    wait_for_boot(&ops, "r2o-1", "a-1").await;
    wait_for_boot(&ops, "r2o-2", "b-1").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());
    let (oa, ob) = (off_on_poweroff(&a), off_on_poweroff(&b));

    let spec = FleetSpec { nodes: vec!["r2o-1".into(), "r2o-2".into()], command: "poweroff".into(), sudo: true, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(false, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(60)).await;
    assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("2 of 2 ok")), "{:?} {:?}", t.error, t.steps);
    let results = ops.fleet_results(&id).unwrap();
    assert_eq!(results.iter().map(|r| (r.node.as_str(), r.state.as_str())).collect::<Vec<_>>(), [("r2o-1", "ok"), ("r2o-2", "ok")], "{results:?}");
    assert_eq!((a.ran("sh -c 'poweroff'").len(), b.ran("sh -c 'poweroff'").len()), (1, 1));
    // One at a time: r2o-2's window opened only once r2o-1 had gone.
    let (a_off, _) = (oa.await.unwrap(), ob.await.unwrap());
    assert!(said.when("begin r2o-2 shut down") > a_off);
    // Both stay planned until they're woken: their windows never closed.
    assert_eq!(said.said(), ["begin r2o-1 shut down", "agents told about r2o-1", "begin r2o-2 shut down", "agents told about r2o-2"]);
    let offs: Vec<_> = t.steps.iter().filter_map(|s| s.detail.as_deref()).filter(|d| d.ends_with("is off (planned until it's woken)")).collect();
    assert_eq!(offs, ["r2o-1 is off (planned until it's woken)", "r2o-2 is off (planned until it's woken)"]);
}

/// The models on two nodes, as Verify sees them: r2v-1's answers after `loading`, r2v-2's
/// stays down. `answered`: when r2v-1's did.
struct Models {
    loading: Duration,
    answered: Mutex<Option<Instant>>,
}

impl Verifier for Models {
    fn affected(&self, scope: &verify::Scope) -> Vec<verify::EndpointRef> {
        let verify::Scope::Nodes(nodes) = scope else { return Vec::new() };
        let model = |n: &str| if n == "r2v-1" { "gemma-3-27b-it" } else { "glm-4.5-air" };
        nodes.iter().map(|n| verify::EndpointRef { id: format!("ep-{n}"), model: model(n).into(), nodes: vec![n.clone()] }).collect()
    }

    fn wait<'a>(&'a self, refs: &'a [verify::EndpointRef], _: Duration, progress: Box<dyn Fn(&verify::EndpointRef, String) + Send + Sync + 'a>) -> BoxFuture<'a, Vec<(verify::EndpointRef, verify::VerifyOutcome)>> {
        Box::pin(async move {
            progress(&refs[0], format!("{}: loading 40%", refs[0].model));
            tokio::time::sleep(self.loading).await;
            refs.iter()
                .map(|r| {
                    if r.nodes[0] == "r2v-1" {
                        *self.answered.lock().unwrap() = Some(Instant::now());
                        (r.clone(), verify::VerifyOutcome::Ready { after_s: 95, untested: None })
                    } else {
                        (r.clone(), verify::VerifyOutcome::Down { reason: "it answers 503".into() })
                    }
                })
                .collect()
        })
    }
}

#[tokio::test]
async fn a_fleet_restart_closes_each_window_only_once_the_nodes_models_answer() {
    let (a, pa) = fake_node(&temp("r2ops-verify-a"), restartable()).await;
    let (b, pb) = fake_node(&temp("r2ops-verify-b"), restartable()).await;
    for (n, boot) in [(&a, "a-1"), (&b, "b-1")] {
        n.bin("reboot", "#!/bin/sh\nexit 0\n");
        set_frame(n, boot, 50_000);
    }
    let (live, inventory) = connect(&temp("r2ops-verify-app"), &[("r2v-1", pa), ("r2v-2", pb)]).await;
    let ops = published(live, inventory, Duration::from_millis(20));
    wait_for_boot(&ops, "r2v-1", "a-1").await;
    wait_for_boot(&ops, "r2v-2", "b-1").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());
    let models = Arc::new(Models { loading: Duration::from_millis(400), answered: Mutex::default() });
    ops.set_verifier(models.clone());
    let ra = restart_on(&a, "sh -c 'reboot'", Some("a-2"), Duration::from_millis(300), || None);
    let rb = restart_on(&b, "sh -c 'reboot'", Some("b-2"), Duration::from_millis(300), || None);

    let spec = FleetSpec { nodes: vec!["r2v-1".into(), "r2v-2".into()], command: "reboot".into(), sudo: true, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(false, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(60)).await;
    let (_, (b_sent, _)) = (ra.await.unwrap(), rb.await.unwrap());

    // Both came through; r2v-2's model didn't answer again: the run's warning, not a failure.
    assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("2 of 2 ok")), "{:?} {:?}", t.error, t.steps);
    assert_eq!(t.warning.as_deref(), Some("r2v-2 restarted, but glm-4.5-air didn't come back: it answers 503"));
    let warnings: Vec<Option<String>> = ops.fleet_results(&id).unwrap().into_iter().map(|r| r.warning).collect();
    assert_eq!(warnings, [None, Some("r2v-2 restarted, but glm-4.5-air didn't come back: it answers 503".to_string())]);
    // r2v-1's window closed once its model answered, and before r2v-2's restart went out.
    assert_eq!(said.said(), ["begin r2v-1 restarting", "agents told about r2v-1", "end r2v-1", "begin r2v-2 restarting", "agents told about r2v-2", "end r2v-2"]);
    let answered = models.answered.lock().unwrap().expect("r2v-1's model answered");
    assert!(said.when("end r2v-1") >= answered && said.when("end r2v-1") < b_sent);
    let waits: Vec<(&str, StepState)> = t.steps.iter().filter(|s| s.label.starts_with("Waiting for ") && s.label.ends_with(" to answer…")).map(|s| (s.label.as_str(), s.state)).collect();
    assert_eq!(waits, [("Waiting for gemma-3-27b-it to answer…", StepState::Ok), ("Waiting for glm-4.5-air to answer…", StepState::Failed)]);
}

// ---- P1.26 Checks before every reboot

/// A GB10 whose checks answer as the state in `now`, which a test changes between restarts.
fn gb10_switchable(now: Arc<Mutex<Gb10>>) -> Script {
    Arc::new(move |cmd: &str, _: &[u8]| gb10_fake::answer(*now.lock().unwrap(), cmd))
}

/// Whether `cmd` is the checks' second read (step B for the fake's kernel).
fn step_b(node: &Node) -> usize {
    node.ran(&prereboot::script(K)).len()
}

#[tokio::test]
async fn a_restart_is_refused_before_any_sudo_when_its_next_kernel_cant_come_up_with_its_gpu() {
    let state = Arc::new(Mutex::new(Gb10::Healthy));
    let (node, port) = fake_node(&temp("r3reboot-refuse-home"), gb10_switchable(state.clone())).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r3reboot-refuse-app"), &[("cell1", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());

    let cases = [
        (Gb10::NoInitrd, "No initrd for 7.0.0-1019-nvidia: the September 2026 panic. Not restarted.", "Its initrd: /boot/initrd.img-7.0.0-1019-nvidia is missing or empty"),
        (Gb10::Mismatch, "7.0.0-1019-nvidia would boot driver 580.95.05 against userspace 580.178.04. Not restarted.", "Module and userspace match: module 580.95.05, userspace 580.178.04"),
        (Gb10::Dkms, "nvidia-dkms-580-open is installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted.", "No DKMS NVIDIA packages: nvidia-dkms-580-open installed"),
        // On hold (`hi`): "Pin the GPU stack" holds nvidia-dkms-*. Still installed, still blocking.
        (Gb10::HeldDkms, "nvidia-dkms-580-open is installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted.", "No DKMS NVIDIA packages: nvidia-dkms-580-open installed"),
        (Gb10::NoModule,"No NVIDIA module for 7.0.0-1019-nvidia: it would boot without its GPU. Not restarted.", "Its NVIDIA module: none for 7.0.0-1019-nvidia (modinfo: ERROR: Module nvidia not found.)"),
        (Gb10::NoVmlinuz, "Can't tell which kernel boots next: /boot/vmlinuz isn't there. Not restarted.", "Kernel that boots next: /boot/vmlinuz isn't there"),
    ];
    for (i, (bad, why, line)) in cases.into_iter().enumerate() {
        *state.lock().unwrap() = bad;
        let id = ops.power(false, "cell1", "reboot", 0).unwrap();
        // While its checks run the node isn't shown restarting.
        let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(why)), "{bad:?}: {:?}", t.steps);
        // The only step is the checks, failed, with each line.
        assert_eq!(t.steps.iter().map(|s| (s.label.as_str(), s.state)).collect::<Vec<_>>(), [("Checks before the restart", StepState::Failed)], "{bad:?}");
        assert!(t.steps[0].detail.as_deref().unwrap().lines().any(|l| l == line), "{bad:?}: {:?}", t.steps[0].detail);
        assert!(!t.on_node, "{bad:?}: nothing reached the node");
        // Nothing was sent: no sudo at all, no window, no agents told, nothing declared.
        assert!(node.ran("sudo ").is_empty(), "{bad:?}: {:?}", node.ran("sudo "));
        assert!(node.ran("systemctl reboot").is_empty());
        assert!(said.said().is_empty(), "{bad:?}: {:?}", said.said());
        assert_eq!(ops.node_view("cell1").unwrap().power_state, None);
        assert!(ops.declared.lock().unwrap().get("cell1").is_none(), "{bad:?}: the claim is let go");
        // The op's head line is failed with the reason; the reads are SparkBox's own probes.
        let head = live.audit.entries().into_iter().find(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind == "op").unwrap();
        assert_eq!((head.state.as_str(), head.error.as_deref()), ("failed", Some(why)));
        let reads: Vec<_> = live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind == "command").collect();
        assert!(reads.iter().all(|e| e.class == "probe" && e.fence.is_none()), "{bad:?}: {reads:#?}");
        assert_eq!(reads.iter().filter(|e| e.command == prereboot::KERNEL).count(), 1);
        // Step B runs only for a kernel step A named.
        assert_eq!(step_b(&node), if bad == Gb10::NoVmlinuz { i } else { i + 1 }, "{bad:?}");
    }

    // Healthy again, and pinned (the prebuilt modules and the kho package on hold, as after
    // Session B step 24): the checks pass, the step lists them, and the restart goes out through sudo.
    *state.lock().unwrap() = Gb10::Pinned;
    let restart = restart_on(&node, "systemctl reboot", Some("boot-b"), Duration::from_millis(200), || None);
    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    restart.await.unwrap();
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    assert_eq!((t.steps[0].label.as_str(), t.steps[0].state), ("Checks before the restart", StepState::Ok));
    let lines: Vec<&str> = t.steps[0].detail.as_deref().unwrap().lines().collect();
    assert_eq!(lines.len(), 9, "{lines:?}");
    assert_eq!(lines[1], "Its initrd: /boot/initrd.img-7.0.0-1019-nvidia is there");
    assert!(lines[6].starts_with(&format!("Prebuilt NVIDIA module packages (recorded): linux-modules-nvidia-580-open-{K} 7.0.0-1019.19+1")), "held, still installed: {lines:?}");
    assert!(lines[7].ends_with("nvidia-spark-grub-kho 1.0.0 is installed"), "{lines:?}");
    assert!(lines[8].starts_with("GRUB_DEFAULT (recorded): GRUB_DEFAULT=0"), "{lines:?}");
    assert_eq!(node.ran("sudo -k -S -p '' systemctl reboot").len(), 1);
    // The checks came first, then the window and the agents, then the command.
    let log = node.log.lock().unwrap().clone();
    let at = |c: &str| log.iter().rposition(|l| l == c).unwrap();
    assert!(at(prereboot::KERNEL) < at(&prereboot::script(K)) && at(&prereboot::script(K)) < at("sudo -k -S -p '' systemctl reboot"));
    assert_eq!(said.said(), ["begin cell1 restarting", "agents told about cell1", "end cell1"]);
}

#[tokio::test]
async fn a_restart_is_held_but_not_shown_while_its_checks_run() {
    let (node, port) = fake_node(&temp("r3reboot-hold-home"), gb10(Gb10::NoInitrd)).await;
    node.bin("shutdown", SHUTDOWN);
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r3reboot-hold-app"), &[("cell1", port)]).await;
    let ops = published(live, inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    // A slow node: each read takes 600 ms.
    node.knobs.slow_ms.store(600, std::sync::atomic::Ordering::SeqCst);
    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(ops.power(false, "cell1", "reboot", 0).unwrap_err().message, "cell1 is being checked before a restart.");
    assert_eq!(ops.power(false, "cell1", "poweroff", 0).unwrap_err().message, "cell1 is being checked before a restart.");
    assert_eq!(ops.node_view("cell1").unwrap().power_state, None, "not restarting yet: it may not restart at all");
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    node.knobs.slow_ms.store(0, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(t.state, TaskState::Failed);
    // Refused: the node is free again, and a shutdown isn't checked.
    let id = ops.power(false, "cell1", "poweroff", 5).unwrap();
    let started = Instant::now();
    while node.ran("shutdown -P +5").is_empty() {
        assert!(started.elapsed() < Duration::from_secs(10), "never scheduled: {:?}", ops.tasks().get(&id).map(|t| t.steps));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let t = ops.tasks().get(&id).unwrap();
    assert!(t.steps.iter().all(|s| !s.label.starts_with("Checks before")), "{:?}", t.steps);
    assert_eq!(node.ran(prereboot::KERNEL).len(), 1, "only the restart's checks ran");
    ops.tasks().cancel(&id).unwrap();
    wait_task(&ops, &id, Duration::from_secs(10)).await;
}

#[tokio::test]
async fn a_scheduled_restart_is_checked_when_it_is_scheduled() {
    let state = Arc::new(Mutex::new(Gb10::NoInitrd));
    let (node, port) = fake_node(&temp("r3reboot-sched-home"), gb10_switchable(state.clone())).await;
    node.bin("shutdown", SHUTDOWN);
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r3reboot-sched-app"), &[("cell1", port)]).await;
    let ops = published(live, inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());

    // Refused when it's scheduled: nothing is scheduled on the node.
    let id = ops.power(false, "cell1", "reboot", 5).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("No initrd for 7.0.0-1019-nvidia: the September 2026 panic. Not restarted.")));
    assert_eq!(t.steps[0].label, "Checks before the restart (run now, when it's scheduled)");
    assert!(node.ran("shutdown").is_empty() && node.ran("sudo ").is_empty() && said.said().is_empty());

    // Passing: checked now, then scheduled (and called off here, so nothing restarts).
    *state.lock().unwrap() = Gb10::Healthy;
    let id = ops.power(false, "cell1", "reboot", 5).unwrap();
    let started = Instant::now();
    while node.ran("shutdown -r +5").is_empty() {
        assert!(started.elapsed() < Duration::from_secs(10), "never scheduled");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let t = ops.tasks().get(&id).unwrap();
    assert_eq!((t.steps[0].label.as_str(), t.steps[0].state), ("Checks before the restart (run now, when it's scheduled)", StepState::Ok));
    ops.tasks().cancel(&id).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.state, TaskState::Cancelled);
    assert_eq!(node.ran(prereboot::KERNEL).len(), 2, "checked once per scheduling (called off before the recheck a minute before it)");
}

/// A scheduled restart is checked again a minute before its time: the node changed meanwhile
/// (its userspace driver was removed, its config kept), so SparkBox calls the restart off on the
/// node (`shutdown -c`, as a power-one change) and the task fails with the reasons. Nothing
/// restarts; the window closes.
#[tokio::test]
async fn a_scheduled_restart_whose_checks_fail_a_minute_before_is_called_off() {
    let state = Arc::new(Mutex::new(Gb10::Healthy));
    let (node, port) = fake_node(&temp("r4upd-recheck-home"), gb10_switchable(state.clone())).await;
    node.bin("shutdown", SHUTDOWN);
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r4upd-recheck-app"), &[("cell1", port)]).await;
    let minute = Duration::from_millis(1500);
    let ops = published_timed(live, inventory, Duration::from_millis(20), minute);
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());

    let id = ops.power(false, "cell1", "reboot", 2).unwrap();
    let started = Instant::now();
    while node.ran("shutdown -r +2").is_empty() {
        assert!(started.elapsed() < Duration::from_secs(10), "never scheduled");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    *state.lock().unwrap() = Gb10::RemovedUserspace;
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    let why = "libnvidia-compute-580 is removed (only its config is left): the driver's userspace isn't installed. Not restarted.";
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(why)), "{:?}", t.steps);
    assert_eq!(node.ran(prereboot::KERNEL).len(), 2, "checked when scheduled and a minute before");
    assert_eq!(node.ran("sudo -k -S -p '' shutdown -c").len(), 1, "called off on the node");
    let labels: Vec<(&str, StepState)> = t.steps.iter().map(|s| (s.label.as_str(), s.state)).collect();
    assert!(labels.contains(&("Checks before the restart (again, a minute before it)", StepState::Failed)), "{labels:?}");
    assert!(labels.contains(&("Calling off the scheduled restart: its checks failed", StepState::Ok)), "{labels:?}");
    let tags: Vec<_> = ops.live.admits().into_iter().filter(|a| a.admitted && a.cmd.contains("shutdown -c")).map(|a| a.tag).collect();
    assert_eq!(tags, [Some(crate::live::exit::Tag::Class(FenceClass::PowerOne))]);
    assert!(node.ran("systemctl reboot").is_empty());
    assert_eq!(said.said().last().map(String::as_str), Some("end cell1"));
    assert!(ops.declared.lock().unwrap().get("cell1").is_none());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_fleet_restart_checks_each_node_before_its_own_restart_and_stops_at_one_that_fails() {
    let _live = crate::health::maintenance::live_test_lock();
    let (a, pa) = fake_node(&temp("r3reboot-fleet-a"), restartable()).await;
    let (b, pb) = fake_node(&temp("r3reboot-fleet-b"), restartable_as(Gb10::Mismatch)).await;
    let (c, pc) = fake_node(&temp("r3reboot-fleet-c"), restartable()).await;
    for (n, boot) in [(&a, "a-1"), (&b, "b-1"), (&c, "c-1")] {
        n.bin("reboot", "#!/bin/sh\nexit 0\n");
        set_frame(n, boot, 50_000);
    }
    let (live, inventory) = connect(&temp("r3reboot-fleet-app"), &[("r3f-1", pa), ("r3f-2", pb), ("r3f-3", pc)]).await;
    let ops = published(live, inventory, Duration::from_millis(20));
    for (n, boot) in [("r3f-1", "a-1"), ("r3f-2", "b-1"), ("r3f-3", "c-1")] {
        wait_for_boot(&ops, n, boot).await;
    }
    let windows = Arc::new(Windows::default());
    ops.set_planned(windows.clone());
    let ra = restart_on(&a, "sh -c 'reboot'", Some("a-2"), Duration::from_millis(300), || None);

    let spec = FleetSpec { nodes: vec!["r3f-1".into(), "r3f-2".into(), "r3f-3".into()], command: "reboot".into(), sudo: true, timeout_s: 10, ..Default::default() };
    let id = ops.fleet_start(false, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(60)).await;
    ra.await.unwrap();
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("1 of 3 ok · 1 skipped")), "{:?}", t.steps);
    let results = ops.fleet_results(&id).unwrap();
    let states: Vec<(&str, &str)> = results.iter().map(|r| (r.node.as_str(), r.state.as_str())).collect();
    assert_eq!(states, [("r3f-1", "ok"), ("r3f-2", "failed"), ("r3f-3", "skipped")]);
    assert_eq!(results[1].error.as_deref(), Some("7.0.0-1019-nvidia would boot driver 580.95.05 against userspace 580.178.04. Not restarted."));
    assert_eq!(results[2].error.as_deref(), Some("Not run: r3f-2 didn't come through"));
    // r3f-1 restarted under its window; r3f-2 was checked and refused before its window or any
    // sudo; r3f-3 wasn't even checked.
    assert_eq!(windows.said(), ["begin r3f-1 restarting until", "agents told about r3f-1", "end r3f-1"]);
    assert_eq!((a.ran("sh -c 'reboot'").len(), b.ran("sudo ").len(), c.ran("sudo ").len()), (1, 0, 0));
    assert_eq!((b.ran(prereboot::KERNEL).len(), c.ran(prereboot::KERNEL).len()), (1, 0));
    let check = |label: &str| t.steps.iter().find(|s| s.label == label).map(|s| s.state);
    assert_eq!(check("Checks before the restart of r3f-1"), Some(StepState::Ok));
    assert_eq!(check("Checks before the restart of r3f-2"), Some(StepState::Failed));
    assert_eq!(check("Checks before the restart of r3f-3"), None);
    // The checks are SparkBox's own reads under the run's op, not typed: no Changes row, no class.
    let reads: Vec<_> = ops.live.audit.entries().into_iter().filter(|e| e.command == prereboot::KERNEL).collect();
    assert_eq!(reads.len(), 2);
    assert!(reads.iter().all(|e| e.op_id.as_deref() == Some(id.as_str()) && e.class == "probe" && !e.changes && e.fence.is_none()), "{reads:#?}");
}

#[tokio::test]
async fn under_session_bs_cell2_fence_the_restart_is_power_one_and_its_checks_need_no_class() {
    let (node, port) = fake_node(&temp("r3reboot-fence-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let base = temp("r3reboot-fence-app");
    let (live, inventory) = connect(&base, &[("cell2", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "cell2", "boot-a").await;
    let f = TestFence::new(&base);
    live.enforce_writes(f.rules.clone());

    // "P1 Session B · all nodes" (no power-one): the checks pass, the restart itself is refused.
    f.open(&["cell1", "cell2", "cell3", "gx10-0000"], &[FenceClass::AptRefresh, FenceClass::AptOne, FenceClass::AptHold, FenceClass::Service, FenceClass::Shell], &[(FenceClass::Service, &["nginx.service"])]).await;
    let id = ops.power(false, "cell2", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("The fence doesn't allow restarts and shutdowns.")), "{:?}", t.steps);
    assert_eq!((t.steps[0].label.as_str(), t.steps[0].state), ("Checks before the restart", StepState::Ok));
    assert!(node.ran("systemctl reboot").is_empty());

    // "P1 Session B · cell2": the restart goes out as power-one.
    f.open(&["cell2"], &[FenceClass::PowerOne, FenceClass::AptRefresh, FenceClass::AptOne, FenceClass::AptHold, FenceClass::Shell], &[]).await;
    let restart = restart_on(&node, "systemctl reboot", Some("boot-b"), Duration::from_millis(200), || None);
    let id = ops.power(false, "cell2", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    restart.await.unwrap();
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    let rows: Vec<_> = live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind != "op").collect();
    let sudo = rows.iter().find(|e| e.kind == "sudo").unwrap();
    assert_eq!((sudo.command.as_str(), sudo.fence.as_deref(), sudo.escalated), ("systemctl reboot", Some("power-one"), true));
    let script = prereboot::script(K);
    let reads: Vec<_> = rows.iter().filter(|e| e.command == prereboot::KERNEL || e.command == script).collect();
    // Newest first, as the log lists them.
    assert_eq!(reads.iter().map(|e| (e.kind.as_str(), e.command.as_str())).collect::<Vec<_>>(), [("command", script.as_str()), ("command", prereboot::KERNEL)]);
    assert!(reads.iter().all(|e| e.fence.is_none() && e.class == "probe" && e.state == "ok"), "the reads need no class: {reads:#?}");
    // The restart's own change claimed cell2's lease first, as any first change under a fence does.
    assert!(rows.iter().any(|e| e.kind == "command" && e.command.starts_with("python3 ~/.sparkbox/sparkjob.py lease claim 'release' ")), "{rows:#?}");
    assert!(live.audit.entries().iter().all(|e| e.title != "Refused" || e.command == "systemctl reboot"), "only the first restart was refused");
}

/// Ops over the simulator; every look at the snapshot is one tick of simulated time.
fn demo_ops(name: &str) -> Arc<Ops> {
    let d = demo();
    let dir = temp(&format!("r3reboot-demo-{name}"));
    let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let live = Live::new(vpn, dir.join("kh.json"), || Ok(None));
    let ops = Ops::with_poll(live, d.clone(), Tasks::new(None), Duration::from_millis(1));
    ops.set_snapshot_source(move || {
        let mut d = d.lock().unwrap();
        d.advance();
        Some(d.snapshot())
    });
    ops
}

#[tokio::test]
async fn a_demo_restart_says_its_checks_need_live_data_and_is_otherwise_unchanged() {
    let ops = demo_ops("power");
    let id = audit::with_op(OpMeta::default(), async { ops.power(true, "cell3", "reboot", 0).unwrap() }).await;
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    let steps: Vec<(&str, StepState)> = t.steps.iter().map(|s| (s.label.as_str(), s.state)).collect();
    assert_eq!(
        steps,
        [
            ("Checks before the restart: Available with Live data", StepState::Skipped),
            ("Sending the restart to cell3", StepState::Ok),
            ("Waiting for cell3 to go offline", StepState::Ok),
            ("Waiting for cell3 to come back (up to 5 min)", StepState::Ok),
            ("Checking that it restarted", StepState::Ok),
            ("Checks after the restart", StepState::Skipped),
        ]
    );
    assert!(t.summary.unwrap().starts_with("cell3 is back after "));
    let log = ops.live.audit.entries();
    assert!(log.iter().any(|e| e.command == "sudo systemctl reboot" && e.simulated));
    assert!(log.iter().all(|e| !e.command.contains("vmlinuz")), "Demo reads nothing");

    // A Fleet restart in Demo: the same words, and the simulated node restarts.
    let spec = FleetSpec { nodes: vec!["cell2".into()], command: "reboot".into(), sudo: true, timeout_s: 10, ..Default::default() };
    let id = ops.fleet_start(true, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    assert_eq!((t.steps[0].label.as_str(), t.steps[0].state), ("Checks before the restart: Available with Live data", StepState::Skipped));
}

/// The command the restart confirmation asks (`node_prereboot_check`) reads what the task reads.
#[tokio::test]
async fn the_checks_alone_read_the_two_commands_and_change_nothing() {
    let (node, port) = fake_node(&temp("r3reboot-alone-home"), gb10(Gb10::Dkms)).await;
    let (live, _) = connect(&temp("r3reboot-alone-app"), &[("cell2", port)]).await;
    let r = prereboot::check(&live, "cell2").await.unwrap();
    assert_eq!((r.node.as_str(), r.kernel.as_deref(), r.ok), ("cell2", Some(K), false));
    assert_eq!(r.blocking.len(), 1);
    assert_eq!(node.ran("vmlinuz"), [prereboot::KERNEL]);
    assert_eq!(node.ran("--- initrd"), [prereboot::script(K)]);
    assert!(node.ran("sudo ").is_empty());
    // Not connected: an error, not a pass.
    assert!(prereboot::check(&live, "cell9").await.is_err());
}

/// Look-only, as Settings › Security sets it.
struct LookOnly;

impl crate::live::exit::WritePolicy for LookOnly {
    fn look_only(&self) -> bool {
        true
    }

    fn admin_unlocked(&self) -> bool {
        false
    }
}

/// What the owed look-only check on the cluster does: the checks read in look-only, and the
/// restart they precede is still refused there, before any sudo.
#[tokio::test]
async fn in_look_only_the_checks_read_and_the_restart_is_refused() {
    let (node, port) = fake_node(&temp("r3reboot-look-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r3reboot-look-app"), &[("cell2", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "cell2", "boot-a").await;
    live.set_write_policy(Arc::new(LookOnly));

    let r = prereboot::check(&live, "cell2").await.unwrap();
    assert!(r.ok, "{:?}", r.blocking);
    assert_eq!(node.ran("vmlinuz").len(), 1);
    let id = ops.power(false, "cell2", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    assert_eq!(t.state, TaskState::Failed, "{:?}", t.steps);
    assert_eq!((t.steps[0].label.as_str(), t.steps[0].state), ("Checks before the restart", StepState::Ok));
    assert_eq!(t.error.as_deref(), Some(crate::admin::LOOK_ONLY));
    assert!(node.ran("sudo ").is_empty() && node.ran("systemctl reboot").is_empty());
    assert!(live.audit.entries().iter().all(|e| e.title != "Refused" || e.command == "systemctl reboot"), "only the restart was refused");
}

// ---- R3-REBOOT fixes: Cancel during the checks, one power command per node across the node
// window and Fleet, no restart while an upgrade runs, a typed restart on its own

async fn until_step(ops: &Ops, id: &str, label: &str) {
    let started = Instant::now();
    while !ops.tasks().get(id).unwrap().steps.iter().any(|s| s.label == label) {
        assert!(started.elapsed() < Duration::from_secs(15), "never reached {label}: {:?}", ops.tasks().get(id).map(|t| t.steps));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn steps_of(t: &Task) -> Vec<(&str, StepState)> {
    t.steps.iter().map(|s| (s.label.as_str(), s.state)).collect()
}

#[tokio::test]
async fn cancel_or_look_only_during_the_checks_sends_nothing_and_lets_the_node_go() {
    let (node, port) = fake_node(&temp("r3fix-cancel-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r3fix-cancel-app"), &[("cell1", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());
    // A slow node: each read takes 3 s, so the checks are still running when Cancel comes.
    node.knobs.slow_ms.store(3000, std::sync::atomic::Ordering::SeqCst);
    for how in ["cancel", "look-only"] {
        let id = ops.power(false, "cell1", "reboot", 0).unwrap();
        until_step(&ops, &id, "Checks before the restart").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let pressed = Instant::now();
        if how == "cancel" {
            ops.tasks().cancel(&id).unwrap();
        } else {
            assert_eq!(ops.tasks().hold_non_read("Look-only turned on").stopped, 1);
        }
        let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
        // At once, not after the reads (6 s on this node).
        assert!(pressed.elapsed() < Duration::from_millis(2500), "{how}: took {:?}", pressed.elapsed());
        // Look-only's own words end a task it stopped (Tasks::hold_non_read).
        let said_why = if how == "cancel" { "The restart of cell1 was called off before anything was sent." } else { "Look-only turned on: stopped before its next step" };
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Cancelled, Some(said_why)), "{how}: {:?}", t.steps);
        assert_eq!(steps_of(&t), [("Checks before the restart", StepState::Skipped)], "{how}");
        assert!(!t.on_node, "{how}");
        // Nothing sent, no window, no agents told, nothing declared, nothing shown.
        assert!(node.ran("sudo ").is_empty() && node.ran("systemctl reboot").is_empty(), "{how}: {:?}", node.ran("sudo "));
        assert!(said.said().is_empty(), "{how}: {:?}", said.said());
        assert!(ops.declared.lock().unwrap().get("cell1").is_none(), "{how}: the claim is let go");
        assert_eq!(ops.node_view("cell1").unwrap().power_state, None);
        let head = live.audit.entries().into_iter().find(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind == "op").unwrap();
        assert_ne!(head.state, "failed", "{how}: {head:?}");
    }
    node.knobs.slow_ms.store(0, std::sync::atomic::Ordering::SeqCst);
}

#[tokio::test]
async fn cancel_while_the_agents_are_told_sends_nothing_and_closes_the_window() {
    let (node, port) = fake_node(&temp("r3fix-agents-home"), gb10(Gb10::Healthy)).await;
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r3fix-agents-app"), &[("cell1", port)]).await;
    let ops = published(live, inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "cell1", "boot-a").await;
    // The agents take 600 ms to answer.
    let said = Said::new(Duration::from_millis(600));
    ops.set_planned(said.clone());
    for (action, what, reason, look_only) in [("reboot", "restart", "restarting", false), ("poweroff", "shutdown", "shut down", false), ("reboot", "restart", "restarting", true)] {
        let id = ops.power(false, "cell1", action, 0).unwrap();
        until_step(&ops, &id, "Telling the nodes' agents about the planned work").await;
        if look_only {
            assert_eq!(ops.tasks().hold_non_read("Look-only turned on").stopped, 1);
        } else {
            ops.tasks().cancel(&id).unwrap();
        }
        let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
        let words = if look_only { "Look-only turned on: stopped before its next step".to_string() } else { format!("The {what} of cell1 was called off before anything was sent.") };
        assert_eq!((t.state, t.summary.clone()), (TaskState::Cancelled, Some(words)), "{action}: {:?}", t.steps);
        // The window opened, the agents heard, and it closed again; the command never left.
        assert_eq!(said.said(), [format!("begin cell1 {reason}"), "agents told about cell1".to_string(), "end cell1".to_string()], "{action}");
        assert!(node.ran("sudo ").is_empty(), "{action}: {:?}", node.ran("sudo "));
        assert!(ops.declared.lock().unwrap().get("cell1").is_none(), "{action}");
        said.lines.lock().unwrap().clear();
    }

    // Cancel once the restart is on its way calls off nothing: when it then fails, it failed.
    ops.set_planned(Said::new(Duration::ZERO));
    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    until_step(&ops, &id, "Waiting for cell1 to go offline").await;
    ops.tasks().cancel(&id).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    assert_eq!(t.state, TaskState::Failed, "{:?}", t.steps);
    assert!(t.error.as_deref().unwrap().starts_with("The restart didn't happen: cell1 kept answering for a minute."), "{:?}", t.error);
    assert_eq!(node.ran("sudo -k -S -p '' systemctl reboot").len(), 1);
}

#[tokio::test]
async fn a_fleet_cancel_during_the_checks_opens_no_window_and_sends_nothing() {
    let (a, pa) = fake_node(&temp("r3fix-fcancel-a"), restartable()).await;
    a.bin("reboot", "#!/bin/sh\nexit 0\n");
    set_frame(&a, "a-1", 50_000);
    let (live, inventory) = connect(&temp("r3fix-fcancel-app"), &[("r3c-1", pa)]).await;
    let ops = published(live, inventory, Duration::from_millis(20));
    wait_for_boot(&ops, "r3c-1", "a-1").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());
    a.knobs.slow_ms.store(3000, std::sync::atomic::Ordering::SeqCst);
    let spec = FleetSpec { nodes: vec!["r3c-1".into()], command: "reboot".into(), sudo: true, timeout_s: 10, ..Default::default() };
    let id = ops.fleet_start(false, spec).unwrap();
    until_step(&ops, &id, "Checks before the restart of r3c-1").await;
    let pressed = Instant::now();
    ops.tasks().cancel(&id).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    a.knobs.slow_ms.store(0, std::sync::atomic::Ordering::SeqCst);
    assert!(pressed.elapsed() < Duration::from_millis(2500), "took {:?}", pressed.elapsed());
    assert_eq!(t.state, TaskState::Cancelled, "{:?}", t.steps);
    let results = ops.fleet_results(&id).unwrap();
    assert_eq!(results.iter().map(|r| (r.node.as_str(), r.state.as_str())).collect::<Vec<_>>(), [("r3c-1", "cancelled")]);
    assert!(a.ran("sh -c 'reboot'").is_empty() && a.ran("sudo ").is_empty(), "{:?}", a.ran("sudo "));
    assert!(said.said().is_empty(), "no window opened: {:?}", said.said());
    assert!(ops.declared.lock().unwrap().get("r3c-1").is_none());
}

/// `restart_on`, for Fleet's `sudo reboot`.
fn fleet_restart(node: &Node, boot: &'static str, look: impl Fn() -> Option<NodeSnapshot> + Send + 'static) -> tokio::task::JoinHandle<(Instant, Option<NodeSnapshot>)> {
    restart_on(node, "sh -c 'reboot'", Some(boot), Duration::from_millis(300), look)
}

#[tokio::test]
async fn the_node_window_and_fleet_never_both_restart_one_node() {
    let (a, pa) = fake_node(&temp("r3fix-both-a"), restartable()).await;
    a.bin("reboot", "#!/bin/sh\nexit 0\n");
    set_frame(&a, "a-1", 50_000);
    let (live, inventory) = connect(&temp("r3fix-both-app"), &[("r3b-1", pa)]).await;
    let ops = published(live, inventory, Duration::from_millis(20));
    wait_for_boot(&ops, "r3b-1", "a-1").await;
    let fleet = || FleetSpec { nodes: vec!["r3b-1".into()], command: "reboot".into(), sudo: true, timeout_s: 10, ..Default::default() };

    // The node window first: while its checks run, and then while it restarts, Fleet is refused.
    a.knobs.slow_ms.store(400, std::sync::atomic::Ordering::SeqCst);
    let restart = restart_on(&a, "systemctl reboot", Some("a-2"), Duration::from_millis(300), || None);
    let id = ops.power(false, "r3b-1", "reboot", 0).unwrap();
    until_step(&ops, &id, "Checks before the restart").await;
    assert_eq!(ops.fleet_start(false, fleet()).unwrap_err().message, "r3b-1 is being checked before a restart.");
    until_step(&ops, &id, "Waiting for r3b-1 to go offline").await;
    a.knobs.slow_ms.store(0, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(ops.fleet_start(false, fleet()).unwrap_err().message, "r3b-1 is already restarting.");
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    restart.await.unwrap();
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    assert_eq!((a.ran("systemctl reboot").len(), a.ran("sh -c 'reboot'").len()), (1, 0), "one restart reached the node");
    assert_eq!(ops.tasks().list().iter().filter(|t| t.kind == "fleet").count(), 0, "no Fleet run started");

    // Fleet first: held as restart-checking during its checks, then rebooting until it's back
    // (the agents take 600 ms, so the node still answers while they're told).
    ops.set_planned(Said::new(Duration::from_millis(600)));
    a.knobs.slow_ms.store(400, std::sync::atomic::Ordering::SeqCst);
    let look = { let ops = ops.clone(); move || ops.node_view("r3b-1") };
    let restart = fleet_restart(&a, "a-3", look);
    let id = ops.fleet_start(false, fleet()).unwrap();
    until_step(&ops, &id, "Checks before the restart of r3b-1").await;
    assert_eq!(ops.power(false, "r3b-1", "reboot", 0).unwrap_err().message, "r3b-1 is being checked before a restart.");
    assert_eq!(ops.power(false, "r3b-1", "poweroff", 0).unwrap_err().message, "r3b-1 is being checked before a restart.");
    assert_eq!(ops.node_view("r3b-1").unwrap().power_state, None, "not shown while it may not restart at all");
    until_step(&ops, &id, "Telling the nodes' agents about the planned work").await;
    a.knobs.slow_ms.store(0, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(ops.power(false, "r3b-1", "reboot", 0).unwrap_err().message, "r3b-1 is already restarting.");
    assert!(a.ran("sh -c 'reboot'").is_empty(), "still nothing sent");
    let t = wait_task(&ops, &id, Duration::from_secs(60)).await;
    let (_, seen) = restart.await.unwrap();
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
    // Declared from before the command went, as the node window's restart is (P1.14).
    assert_eq!(seen.and_then(|n| n.power_state).as_deref(), Some("rebooting"));
    assert_eq!((a.ran("systemctl reboot").len(), a.ran("sh -c 'reboot'").len()), (1, 1));
    assert!(ops.declared.lock().unwrap().get("r3b-1").is_none(), "back on a new boot: nothing declared");
    assert_eq!(ops.node_view("r3b-1").unwrap().boot_id.as_deref(), Some("a-3"));
    assert_eq!(ops.power_busy("r3b-1", "restart it"), None, "free again");
}

/// A GB10 that answers the checks before a restart as healthy, and the upgrade's dry run and
/// status as updates.rs's tests do; apt-get, systemd-run and dpkg-query are fake_node.rs's.
fn upgradable() -> Script {
    Arc::new(|cmd: &str, _: &[u8]| {
        if let Some(a) = gb10_fake::answer(Gb10::Healthy, cmd) {
            return Some(a);
        }
        if cmd.starts_with("apt-get -s ") {
            return Some(("Inst libfoo [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])\n".into(), 0));
        }
        cmd.contains("apt list --upgradable").then(|| ("--- upgradable\n--- held\n--- reboot\n--- unattended\n--- checked\n".to_string(), 0))
    })
}

#[tokio::test]
async fn no_restart_or_shutdown_while_an_upgrade_sparkbox_started_runs() {
    let (node, port) = fake_node(&temp("r3fix-upg-home"), upgradable()).await;
    updates::fake::pin_aware(&node);
    std::fs::create_dir_all(node.state_dir.join("apt")).unwrap();
    std::fs::write(node.state_dir.join("apt/delay"), "0.7").unwrap();
    set_frame(&node, "boot-a", 50_000);
    let (live, inventory) = connect(&temp("r3fix-upg-app"), &[("r3up-1", port)]).await;
    let ops = published(live, inventory, Duration::from_millis(10));
    wait_for_boot(&ops, "r3up-1", "boot-a").await;
    let said = Said::new(Duration::ZERO);
    ops.set_planned(said.clone());
    let restart = "r3up-1 is installing packages (Upgrade libfoo on r3up-1); restart it when that finishes.";
    let shutdown = "r3up-1 is installing packages (Upgrade libfoo on r3up-1); shut it down when that finishes.";
    let fleet = |command: &str| FleetSpec { nodes: vec!["r3up-1".into()], command: command.into(), sudo: true, timeout_s: 10, ..Default::default() };
    let refused = |ops: &Arc<Ops>| {
        assert_eq!(ops.power(false, "r3up-1", "reboot", 0).unwrap_err().message, restart);
        assert_eq!(ops.power(false, "r3up-1", "reboot", 5).unwrap_err().message, restart);
        assert_eq!(ops.power(false, "r3up-1", "poweroff", 0).unwrap_err().message, shutdown);
        assert_eq!(ops.fleet_start(false, fleet("reboot")).unwrap_err().message, restart);
        assert_eq!(ops.fleet_start(false, fleet("poweroff")).unwrap_err().message, shutdown);
    };

    // From the upgrade's dry run on, and while apt downloads in its unit.
    let shown = ops.updates_simulate(false, "r3up-1", &["libfoo".into()]).await.unwrap().shown();
    let id = ops.updates_upgrade_opts(false, "r3up-1".into(), vec!["libfoo".into()], updates::UpgradeArgs { plan: Some(shown), ..Default::default() }).unwrap();
    refused(&ops);
    let started = Instant::now();
    while !ops.runs().iter().any(|r| r.task_id == id && r.phase == "downloading") {
        assert!(started.elapsed() < Duration::from_secs(20), "never downloading: {:?}", ops.tasks().get(&id).map(|t| t.log));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    refused(&ops);
    // Nothing was sent, no restart window opened, no task started.
    assert!(node.ran(prereboot::KERNEL).is_empty() && node.ran("systemctl reboot").is_empty() && node.ran("poweroff").is_empty() && node.ran("shutdown").is_empty());
    assert!(said.said().iter().all(|s| !s.contains("restarting") && !s.contains("shut down")), "{:?}", said.said());
    assert!(ops.tasks().list().iter().all(|t| t.kind == "updates"), "{:?}", ops.tasks().list().iter().map(|t| &t.title).collect::<Vec<_>>());
    let t = wait_task(&ops, &id, Duration::from_secs(60)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
    assert_eq!(ops.power_busy("r3up-1", "restart it"), None, "once it's done, the node may restart");

    // An upgrade task that started between the restart's look and its claim (once the restart
    // is claimed, updates.rs holds a new upgrade off): the restart stops after its checks.
    node.knobs.slow_ms.store(500, std::sync::atomic::Ordering::SeqCst);
    let power = ops.power(false, "r3up-1", "reboot", 0).unwrap();
    until_step(&ops, &power, "Checks before the restart").await;
    let slipped = ops.tasks().start(TaskSpec::new("updates", "Upgrade libfoo on r3up-1", &["r3up-1".to_string()]).cancellable());
    slipped.set_retry("updates_upgrade", json!({ "node": "r3up-1", "packages": ["libfoo"] }));
    let t = wait_task(&ops, &power, Duration::from_secs(30)).await;
    node.knobs.slow_ms.store(0, std::sync::atomic::Ordering::SeqCst);
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(restart)), "{:?}", t.steps);
    assert_eq!(steps_of(&t), [("Checks before the restart", StepState::Ok)]);
    assert!(node.ran("systemctl reboot").is_empty() && said.said().iter().all(|s| !s.contains("restarting")));
    assert!(ops.declared.lock().unwrap().get("r3up-1").is_none());
    slipped.finish_ok("Done");
}

#[test]
fn a_typed_restart_is_the_restart_alone() {
    use super::classify::{verdict, Power};
    use super::fleet::restart_alone;
    for alone in [
        "reboot",
        "sudo reboot",
        "/sbin/reboot",
        "systemctl reboot",
        "sudo systemctl reboot",
        "systemctl --no-wall reboot",
        "systemctl start reboot.target",
        "shutdown -r now",
        "shutdown -r +1 'back soon'",
        "init 6",
        "sudo chroot / reboot",
        "sudo -u root reboot",
        "env LC_ALL=C reboot",
        "nohup reboot",
        "nice -n 5 reboot",
        "timeout 5s systemctl reboot",
        "timeout --signal=TERM --kill-after=5s 10s reboot",
        "sh -c reboot",
        "sh -c 'systemctl reboot'",
    ] {
        assert_eq!(verdict(alone).power, Some(Power::Restart), "{alone}");
        assert!(restart_alone(alone), "{alone}");
    }
    for after_a_change in [
        "apt-get install -y linux-image-7.0.0-1020-nvidia && reboot",
        "apt-get -y install nvidia-dkms-580-open; systemctl reboot",
        "rm /boot/initrd.img-7.0.0-1019-nvidia; reboot",
        "touch /tmp/x; reboot",
        "sleep 1 && reboot",
        "reboot & apt-get install -y x",
        "reboot > /boot/initrd.img-7.0.0-1019-nvidia",
        "reboot $(touch /tmp/x)",
        "sh -c 'apt-get install -y x; reboot'",
        "bash -c 'rm -f /boot/initrd.img-7.0.0-1019-nvidia && reboot'",
        "echo reboot | sh",
        "sudo -i reboot",
        "docker run --rm -v /:/host alpine chroot /host reboot",
        "env -S 'reboot'",
        "env -iS 'reboot'",
        "env -iSreboot",
        "env --split-string=reboot",
    ] {
        assert_eq!(verdict(after_a_change).power, Some(Power::Restart), "{after_a_change}");
        assert!(!restart_alone(after_a_change), "{after_a_change}");
    }
}

#[tokio::test]
async fn fleet_refuses_a_restart_typed_after_a_change_before_anything_runs() {
    let (node, port) = fake_node(&temp("r3fix-typed-home"), gb10(Gb10::Healthy)).await;
    node.bin("reboot", "#!/bin/sh\nexit 0\n");
    set_frame(&node, "boot-a", 50_000);
    let base = temp("r3fix-typed-app");
    let (live, inventory) = connect(&base, &[("cell2", port)]).await;
    let ops = published(live.clone(), inventory, Duration::from_millis(20));
    wait_for_boot(&ops, "cell2", "boot-a").await;
    let f = TestFence::new(&base);
    live.enforce_writes(f.rules.clone());
    // "P1 Session B · cell2": power-one, no fleet.
    f.open(&["cell2"], &[FenceClass::PowerOne, FenceClass::AptRefresh, FenceClass::AptOne, FenceClass::AptHold, FenceClass::Shell], &[]).await;
    let marker = node.state_dir.join("first-half-ran");
    let typed = format!("touch '{}'; reboot", marker.display());
    let spec = FleetSpec { nodes: vec!["cell2".into()], command: typed, sudo: true, timeout_s: 10, ..Default::default() };
    let e = ops.fleet_start(false, spec).unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Invalid, "Run the restart on its own: SparkBox checks the kernel the node boots next just before it restarts."));
    assert!(!marker.exists(), "the first half never ran");
    assert!(node.ran("touch").is_empty() && node.ran(prereboot::KERNEL).is_empty() && node.ran("sudo ").is_empty());
    assert!(ops.tasks().list().is_empty(), "no task started");
}
