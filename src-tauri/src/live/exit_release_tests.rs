//! The release build's path at the exit (`BUILD_NEEDS_FENCE`): a change needs no fence, and
//! everything else stands as it does in the debug build. On 2026-10-30 the owner's release app
//! refused the owner's own Hugging Face download ("The fence doesn't allow this change.") and a
//! multi-node load's image build ("No fence is open"); models/release_exit_tests.rs walks those
//! two through their real entry points.
//!
//! `cargo test` is a debug build, so these tests walk the release path through the one test seam,
//! `Live::walk_release_path`, which isn't compiled into the app. The two `the_compile_alone_…` tests
//! take no seam at all. One runs both ways, and the other exists only in a release build, which
//! `scripts/check.sh` runs: `cargo test --release --lib -- release_tests release_exit_tests`.
//!
//! Admin mode here is the app's own gate (`hand_over`) with a stand-in for Touch ID.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::tests::{cell2_on, docker, err, lease_file, refusals, spec, wait_hostname, TestFence};
use super::*;
use crate::admin::testing::{gate, FakeAuth};
use crate::admin::RelockReason;
use crate::errors::{ErrorKind, SbError};
use crate::fake_node::{self, fake_node, fake_spark, temp, Node, Profile, Script};
use crate::live::fence::{self, Spec};
use crate::live::fence_class::FenceClass;
use crate::live::{audit, CancelToken, Live};

const RESTART: &str = "systemctl restart -- 'earlyoom.service'";

async fn restart(live: &Live, node: &str) -> Result<(), SbError> {
    live.sudo(node, RESTART, Duration::from_secs(30)).await.map(drop).map_err(err)
}

struct Rig {
    node: Node,
    live: Arc<Live>,
    gate: Arc<AdminGate>,
    auth: FakeAuth,
    f: TestFence,
    home: PathBuf,
}

/// Admin mode unlocked the way Live unlocks it: Touch ID was asked, and said yes.
fn admin_confirmed(live: &Arc<Live>) -> (Arc<AdminGate>, FakeAuth) {
    let (g, _, auth) = gate();
    g.set_live(true);
    hand_over(&g, live);
    g.unlock(None).expect("Touch ID says yes");
    assert_eq!(auth.asked.load(Ordering::SeqCst), 1, "a Live unlock asks for Touch ID");
    (g, auth)
}

/// cell2 on a fake node, the exit enforced with `instance`'s rules and no fence open anywhere,
/// and Admin mode confirmed.
async fn rig(name: &str, script: Script, instance: &str) -> Rig {
    let home = temp(&format!("{name}-home"));
    let (node, port) = fake_node(&home, script).await;
    let base = temp(&format!("{name}-app"));
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, instance);
    live.enforce_writes(f.rules.clone());
    let (gate, auth) = admin_confirmed(&live);
    assert!(!f.rules.fence_dir.join(fence::RECORD).exists(), "no fence record");
    Rig { node, live, gate, auth, f, home }
}

/// `rig` on the release build's path.
async fn release(name: &str, script: Script) -> Rig {
    let r = rig(name, script, "release").await;
    r.live.walk_release_path();
    r
}

/// No seam: the exit as this compile built it.
#[tokio::test]
async fn the_compile_alone_decides_whether_a_change_needs_a_fence() {
    assert_eq!(BUILD_NEEDS_FENCE, cfg!(debug_assertions));
    assert_eq!(BUILD_NEEDS_FENCE, crate::identity::instance() == crate::identity::Instance::Dev);
    let r = rig("release-compile", docker(), crate::identity::instance_name()).await;
    assert_eq!(r.live.exit.needs_fence(), BUILD_NEEDS_FENCE);
    let result = restart(&r.live, "cell2").await;
    if BUILD_NEEDS_FENCE {
        let e = result.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Fenced);
        assert!(r.node.ran("systemctl restart").is_empty());
    } else {
        result.unwrap();
        assert_eq!(r.node.ran("systemctl restart").len(), 1);
    }
    // What the app itself builds: nothing in Enforce says which way this goes.
    assert_eq!(Exit::enforcing(Enforce::app()).needs_fence(), BUILD_NEEDS_FENCE);
}

/// The debug build's exit: Admin confirmed, and refused all the same.
#[cfg(debug_assertions)]
#[tokio::test]
async fn the_debug_build_refuses_every_kind_of_change_with_no_fence() {
    let r = rig("debug-none", docker(), "dev").await;
    r.gate.check().expect("Admin mode is confirmed");
    let live = &r.live;
    let no_fence = "No fence is open, so changes on the cluster are refused. This test build needs one: open it in the release SparkBox (Settings › Fence).";
    let refused = |e: SbError| assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, no_fence));

    refused(restart(live, "cell2").await.unwrap_err());
    refused(err(live.container_action("cell2", "0123456789ab", "restart").await.unwrap_err()));
    refused(err(live.sparkjob("cell2", "start download dl-1", Some(b"{}"), Duration::from_secs(10)).await.unwrap_err()));
    refused(err(live.run("cell2", &crate::models::plan::pull_script("nvcr.io/nvidia/vllm:26.05-py3"), None, Duration::from_secs(10)).await.unwrap_err()));
    refused(err(audit::as_typed(live.stream_cmd("cell2", "touch /tmp/sparkbox-debug", Duration::from_secs(10), CancelToken::new(), |_, _| {})).await.unwrap_err()));
    refused(live.admit_from_mac("cell2", FenceClass::PowerOne, "Wake-on-LAN for cell2").unwrap_err());
    assert!(r.node.ran("systemctl restart").is_empty() && r.node.ran("docker").is_empty() && r.node.ran("sparkjob.py start").is_empty());

    // A release label buys nothing: the compile decides, not the rules' instance.
    live.enforce_writes(Enforce { instance: "release".into(), ..r.f.rules.clone() });
    assert_eq!(restart(live, "cell2").await.unwrap_err().kind, ErrorKind::Fenced);
    // Nor does SPARKBOX_ADMIN_NOAUTH.
    live.enforce_writes(Enforce { noauth: true, ..r.f.rules.clone() });
    let e = restart(live, "cell2").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "SPARKBOX_ADMIN_NOAUTH needs an open fence in Live."));
    // A fence still opens it, for what it names.
    live.enforce_writes(r.f.rules.clone());
    r.f.open(&spec(&["cell2"], &[FenceClass::Service], "dev"));
    restart(live, "cell2").await.unwrap();
    assert_eq!(err(live.sparkjob("cell2", "start download dl-1", Some(b"{}"), Duration::from_secs(10)).await.unwrap_err()).message, "The fence doesn't allow this change.");
}

#[tokio::test]
async fn on_the_release_path_a_change_needs_no_fence_and_its_line_still_names_its_class() {
    let r = release("release-ok", docker()).await;
    assert!(!r.live.exit.needs_fence());
    restart(&r.live, "cell2").await.unwrap();
    assert_eq!(r.node.ran("systemctl restart").len(), 1);
    let row = r.live.audit.entries().into_iter().find(|e| e.kind == "sudo" && e.command == RESTART && e.node == "cell2").unwrap();
    assert_eq!((row.fence.as_deref(), row.state.as_str()), (Some("service"), "ok"));

    // Whatever its class, `other` included: only an `any` fence admitted those.
    r.live.sudo("cell2", "touch /tmp/sparkbox-any", Duration::from_secs(10)).await.unwrap();
    assert_eq!(r.live.audit.entries().into_iter().find(|e| e.command == "touch /tmp/sparkbox-any").unwrap().fence.as_deref(), Some("other"));
    audit::as_typed(r.live.stream_cmd("cell2", "touch /tmp/sparkbox-fleet", Duration::from_secs(10), CancelToken::new(), |_, _| {})).await.unwrap();
    assert_eq!(r.live.audit.entries().into_iter().find(|e| e.command == "touch /tmp/sparkbox-fleet" && e.kind == "stream").unwrap().fence.as_deref(), Some("fleet"));

    // The lease is claimed as ever, the identity pin was written, and nothing was refused.
    let held: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(r.home.join(".sparkbox/controller.json")).unwrap()).unwrap();
    assert_eq!(held["instance"], "release");
    assert!(identity_pin::load(r.f.rules.config_dir.as_ref().unwrap()).contains_key("cell2"));
    assert!(refusals(&r.live).is_empty(), "{:#?}", refusals(&r.live));
}

/// Whatever a fence's file says, it's nothing to the release path: not a way in, and not a way to
/// stop the owner's own changes (a record that failed its check refused everything before).
#[tokio::test]
async fn on_the_release_path_no_fence_file_stands_in_the_way() {
    let r = release("release-files", docker()).await;
    let record = r.f.rules.fence_dir.join(fence::RECORD);
    // Tampered with.
    std::fs::write(&record, b"{not json").unwrap();
    restart(&r.live, "cell2").await.unwrap();
    // The test build's, naming another node and class.
    std::fs::remove_file(&record).unwrap();
    r.f.open(&spec(&["cell1"], &[FenceClass::ContainerTest], "dev"));
    restart(&r.live, "cell2").await.unwrap();
    assert_eq!(r.node.ran("systemctl restart").len(), 2);
    assert!(refusals(&r.live).is_empty());
}

#[tokio::test]
async fn on_the_release_path_look_only_still_refuses_all_but_reads() {
    let r = release("release-look", docker()).await;
    restart(&r.live, "cell2").await.unwrap();
    r.gate.set_look_only(true).unwrap();
    let lines = refusals(&r.live).len();

    for e in [
        restart(&r.live, "cell2").await.unwrap_err(),
        err(r.live.container_action("cell2", "0123456789ab", "restart").await.unwrap_err()),
        err(r.live.sparkjob("cell2", "start download dl-1", Some(b"{}"), Duration::from_secs(10)).await.unwrap_err()),
        err(as_use(r.live.probe("cell2", "curl -s -m 2 http://127.0.0.1:8000/v1/models", Duration::from_secs(5))).await.unwrap_err()),
        r.live.admit_from_mac("cell2", FenceClass::PowerOne, "Wake-on-LAN for cell2").unwrap_err(),
    ] {
        assert_eq!(e.kind, ErrorKind::LookOnly, "{}", e.message);
    }
    assert!(r.live.pty("cell2", 80, 24, None).await.is_err());
    assert_eq!(r.node.ran("systemctl restart").len(), 1, "only the one before look-only");
    assert!(r.node.ran("sparkjob.py start").is_empty() && r.node.ran("docker").is_empty() && r.node.ran("curl").is_empty());
    assert_eq!(refusals(&r.live).len(), lines + 6, "each refused, with its line: {:#?}", refusals(&r.live));

    assert_eq!(r.live.run("cell2", "uptime", None, Duration::from_secs(10)).await.unwrap().code, Some(0));
    // And Admin mode can't be unlocked: nothing could ask Touch ID.
    let asked = r.auth.asked.load(Ordering::SeqCst);
    assert_eq!(r.gate.unlock(None).unwrap_err(), admin::LOOK_ONLY);
    assert_eq!(r.gate.check().unwrap_err(), admin::LOOK_ONLY);
    assert_eq!(r.auth.asked.load(Ordering::SeqCst), asked);
}

#[tokio::test]
async fn on_the_release_path_a_writing_background_check_is_still_refused() {
    let r = release("release-probe", docker()).await;
    let e = err(r.live.probe("cell2", "touch /tmp/sparkbox-probe-wrote", Duration::from_secs(5)).await.unwrap_err());
    assert_eq!(e.kind, ErrorKind::Invalid);
    assert!(e.message.contains("Background checks only read"), "{}", e.message);
    assert!(r.node.ran("touch").is_empty());
}

#[tokio::test]
async fn on_the_release_path_a_node_answering_as_another_is_still_refused() {
    let (node2, p2) = fake_spark(&temp("release-id-cell2"), "cell2").await;
    let (_n3, p3) = fake_spark(&temp("release-id-cell3"), "cell3").await;
    let base = temp("release-id-app");
    let (live, _) = fake_node::connect_sparks(&base, &[("cell2", p2)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    live.walk_release_path();
    let (_gate, _) = admin_confirmed(&live);
    live.exit.pins.pin_new(&live, f.rules.config_dir.as_ref().unwrap(), now_ms()).await;
    restart(&live, "cell2").await.unwrap();
    assert_eq!(node2.ran("systemctl restart").len(), 1);

    // cell3 answers on cell2's address, with the cloned host key both present.
    assert_eq!(Profile::spark("cell3").fingerprint(), Profile::spark("cell2").fingerprint());
    crate::live::trust_host_key(&live.data_path("known_hosts.json"), &format!("127.0.0.1:{p3}"), &Profile::spark("cell3").fingerprint()).unwrap();
    live.start(&[cell2_on(p3)]);
    wait_hostname(&live, "cell3-example").await;
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::WrongNode, "cell2 answers as cell3-example: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence."));
    assert_eq!(refusals(&live).len(), 1);
    // Reads keep working.
    assert_eq!(live.run("cell2", "uptime", None, Duration::from_secs(10)).await.unwrap().code, Some(0));
}

#[tokio::test]
async fn on_the_release_path_the_lease_still_keeps_the_two_builds_apart() {
    let r = release("release-lease", docker()).await;
    // The test build holds cell2, under a fence of its own.
    lease_file(&r.home, "dev", 0.0);
    let e = restart(&r.live, "cell2").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "SparkBox (dev) is controlling cell2 right now."));
    assert!(r.node.ran("systemctl restart").is_empty());
    // Its heartbeat goes stale: the release build takes over.
    lease_file(&r.home, "dev", 60.0);
    r.live.exit.leases.forget();
    restart(&r.live, "cell2").await.unwrap();
    let held: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(r.home.join(".sparkbox/controller.json")).unwrap()).unwrap();
    assert_eq!(held["instance"], "release");
}

#[tokio::test]
async fn on_the_release_path_the_other_builds_containers_are_still_left_alone() {
    use crate::ops::classify::contracts::{c_l_no_key, C_U};
    let (node, port) = fake_node(&temp("release-owner-home"), fake_node::real_shell()).await;
    node.bin("docker", "exit 0");
    let mut frame: serde_json::Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    frame["containers"] = json!([
        { "id": "0123456789ab", "name": "gemma-dev", "image": "vllm", "state": "running", "startedAt": 1, "finishedAt": 0, "exitCode": 0,
          "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "vllm serve",
          "labels": { "sparkbox.instance": "dev", "sparkbox.deployment": "d1" } }
    ]);
    *node.knobs.frame.lock().unwrap() = Some(frame.to_string());
    let base = temp("release-owner-app");
    let (live, inv) = fake_node::connect(&base, &[("cell2", port)]).await;
    for _ in 0..100 {
        if live.snapshot(&inv).nodes[0].containers.iter().any(|c| c.name == "gemma-dev") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    live.walk_release_path();
    let (_gate, _) = admin_confirmed(&live);

    let e = err(live.container_action("cell2", "0123456789ab", "stop").await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "gemma-dev on cell2 is managed by the other SparkBox (dev)."));
    let unload = C_U.replace("instance=release", "instance=dev");
    let e = err(live.run("cell2", &unload, None, Duration::from_secs(10)).await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "This unload on cell2 would remove the other SparkBox's (dev) containers."));
    let load = c_l_no_key().replace("instance=release", "instance=dev");
    let e = err(as_class(FenceClass::ModelLoadOne, live.run("cell2", &load, None, Duration::from_secs(10))).await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "This load on cell2 would start a container labeled as the other SparkBox's (dev)."));
    assert!(node.ran("docker").iter().all(|c| !c.starts_with("docker stop") && !c.starts_with("docker run")), "{:?}", node.ran("docker"));
    assert!(node.ran("docker rm").is_empty(), "{:?}", node.ran("docker rm"));
    // Its own load, labelled for this build, passes.
    as_class(FenceClass::ModelLoadOne, live.run("cell2", &c_l_no_key(), None, Duration::from_secs(10))).await.unwrap();
    assert_eq!(node.ran("docker run").len(), 1);
}

/// A head, a worker and their launch files need no fence on the release path, only what stands
/// there: look-only, and the other build's labels. The line still names the class.
#[tokio::test]
async fn on_the_release_path_every_rank_of_a_load_passes_and_look_only_still_holds_them() {
    use crate::ops::classify::contracts::{c_l_no_key, C_L, C_L_HEAD, C_L_WORKER};
    let r = release("release-ranks", docker()).await;
    let ranks = [
        (FenceClass::ModelLoadOne, ExecKind::Run, C_L.to_string()),
        (FenceClass::ModelLoadOne, ExecKind::Run, c_l_no_key()),
        (FenceClass::ModelLoad, ExecKind::Run, C_L_HEAD.to_string()),
        (FenceClass::ModelLoad, ExecKind::Run, C_L_WORKER.to_string()),
        (FenceClass::ModelLoadOne, ExecKind::Sparkjob, "deploy-files d1 single".to_string()),
        (FenceClass::ModelLoad, ExecKind::Sparkjob, "deploy-files d1 head".to_string()),
    ];
    for (class, kind, cmd) in &ranks {
        let got = as_class(*class, r.live.exit.admit(&r.live, "cell2", *kind, cmd)).await;
        assert_eq!(got.map_err(|e| e.message), Ok(Admitted { fence: Some(class.id().to_string()) }), "`{cmd}`");
    }
    for cmd in [C_L_HEAD, C_L_WORKER] {
        let theirs = cmd.replace("instance=release", "instance=dev");
        let e = err(as_class(FenceClass::ModelLoad, r.live.run("cell2", &theirs, None, Duration::from_secs(10))).await.unwrap_err());
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "This load on cell2 would start a container labeled as the other SparkBox's (dev)."), "`{theirs}`");
    }
    r.gate.set_look_only(true).unwrap();
    for (class, kind, cmd) in &ranks {
        let e = as_class(*class, r.live.exit.admit(&r.live, "cell2", *kind, cmd)).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::LookOnly, "`{cmd}`: {}", e.message);
    }
}

#[tokio::test]
async fn on_the_release_path_a_wake_needs_no_fence_and_look_only_still_holds_it() {
    let r = release("release-wake", docker()).await;
    let a = r.live.admit_from_mac("cell2", FenceClass::PowerOne, "Wake-on-LAN for cell2").unwrap();
    assert_eq!(a.fence.as_deref(), Some("power-one"), "its line still names the class");
    r.gate.set_look_only(true).unwrap();
    assert_eq!(r.live.admit_from_mac("cell2", FenceClass::PowerOne, "Wake-on-LAN for cell2").unwrap_err().kind, ErrorKind::LookOnly);
}

async fn until_no_lease(live: &Live) {
    for _ in 0..500 {
        if live.exit.leases.held().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the relock left the lease held: {:?}", live.exit.leases.held());
}

/// The gate stands at the door: a new action needs Admin mode. Work that already passed it isn't
/// refused part-way when Admin relocks, for any reason, and its next change takes the lease again.
#[tokio::test]
async fn on_the_release_path_work_that_passed_the_gate_outlives_every_relock() {
    let r = release("release-relock", docker()).await;
    for why in [RelockReason::Idle, RelockReason::Explicit, RelockReason::ScreenLock, RelockReason::Sleep, RelockReason::WindowClosed] {
        r.gate.check().unwrap_or_else(|e| panic!("{why:?}: {e}"));
        restart(&r.live, "cell2").await.unwrap_or_else(|e| panic!("{why:?}: {}", e.message));
        assert!(r.gate.lock_with(why, "Locked by the test"));
        // The relock hook lets go here on a task of its own: wait for it, or it could land after
        // the next change took the lease again.
        until_no_lease(&r.live).await;
        // A new action stops at the gate…
        assert_eq!(r.gate.check().unwrap_err(), admin::LOCKED, "{why:?}");
        // …and the one already running goes on.
        restart(&r.live, "cell2").await.unwrap_or_else(|e| panic!("after {why:?}: {}", e.message));
        r.gate.unlock(None).unwrap();
    }
    assert_eq!(r.node.ran("systemctl restart").len(), 10);
    assert!(refusals(&r.live).is_empty(), "{:#?}", refusals(&r.live));
    assert_eq!(r.live.exit.leases.held(), ["cell2"]);
}

/// A round of the ticker's renewals, as `start` runs them: what `tasks` has running, or nothing.
async fn renew(live: &Live, inventory: &[String], tasks: &crate::tasks::Tasks) {
    live.renew_leases(&live.exit.fence_view(live), inventory, &tasks.working_nodes()).await;
}

/// How long ago the node last heard from the lease's holder, and who that is.
fn heartbeat(home: &std::path::Path) -> (String, f64) {
    let held: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(".sparkbox/controller.json")).unwrap()).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    (held["instance"].as_str().unwrap().to_string(), now - held["heartbeat"].as_f64().unwrap())
}

/// Admin relocks under a long job. The job makes one change and then only reads, so nothing of its
/// own keeps the lease, and an open release fence used to. Left to lapse, the test build could act
/// on the node 45 s later: while the job runs the lease stays held, whatever Admin mode's state,
/// and once it's done the node is free again.
#[tokio::test]
async fn on_the_release_path_the_lease_stays_held_under_running_work_after_a_relock() {
    let home = temp("release-work-home");
    let (node, port) = fake_node(&home, docker()).await;
    let base = temp("release-work-app");
    let (rel, _) = fake_node::connect(&base.join("release"), &[("cell2", port)]).await;
    let (dev, _) = fake_node::connect(&base.join("dev"), &[("cell2", port)]).await;
    let (fr, fd) = (TestFence::new(&base.join("release"), "release"), TestFence::new(&base.join("dev"), "dev"));
    rel.enforce_writes(fr.rules.clone());
    dev.enforce_writes(fd.rules.clone());
    rel.walk_release_path();
    let (gate, _) = admin_confirmed(&rel);
    let tasks = crate::tasks::Tasks::new(None);
    tasks.set_source(|| false);
    let inventory = ["cell2".to_string()];

    // The job starts, with its one change, and the test build is kept out.
    let job = tasks.start(crate::tasks::TaskSpec::new("model", "Download Qwen/Qwen3-8B to cell2", &inventory).on_node());
    restart(&rel, "cell2").await.unwrap();
    fd.open(&spec(&["cell2"], &[FenceClass::Service], "dev"));
    let refused = |e: SbError| assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "SparkBox (release) is controlling cell2 right now."));
    refused(restart(&dev, "cell2").await.unwrap_err());

    // Admin locks by itself. Here the build lets go, as it does on every relock.
    assert!(gate.lock_with(RelockReason::Idle, "No changes for 15 minutes"));
    for _ in 0..100 {
        if rel.exit.leases.held().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(rel.exit.leases.held().is_empty());
    assert!(!rel.exit.policy().admin_unlocked());

    // Round after round the node hears from it, with the time between rounds gone by.
    for _ in 0..3 {
        lease_file(&home, "release", lease::REFRESH.as_secs_f64());
        rel.exit.leases.forget();
        renew(&rel, &inventory, &tasks).await;
        let (holder, age) = heartbeat(&home);
        assert_eq!(holder, "release");
        assert!(age < lease::REFRESH.as_secs_f64() / 2.0, "renewed: {age} s");
    }
    assert_eq!(rel.exit.leases.held(), ["cell2"]);
    dev.exit.leases.forget();
    refused(restart(&dev, "cell2").await.unwrap_err());
    assert_eq!(node.ran("systemctl restart").len(), 1, "the test build changed nothing");

    // The job is done: nothing renews it now, and past the TTL the test build may act.
    job.finish_ok("Downloaded");
    lease_file(&home, "release", lease::TTL_S as f64 + 1.0);
    rel.exit.leases.forget();
    renew(&rel, &inventory, &tasks).await;
    assert!(heartbeat(&home).1 > lease::TTL_S as f64, "not renewed");
    dev.exit.leases.forget();
    restart(&dev, "cell2").await.unwrap();
    assert_eq!(heartbeat(&home).0, "dev");
    assert_eq!(node.ran("systemctl restart").len(), 2);
}

/// Who `renewals` names. Admin unlocked, every node; locked, the nodes work runs on and no others.
/// A job the other build's lease stands in front of takes nothing from it.
#[tokio::test]
async fn on_the_release_path_a_lease_is_renewed_for_work_and_for_nothing_else() {
    let r = release("release-renewals", docker()).await;
    let view = r.live.exit.fence_view(&r.live);
    let all = ["cell1".to_string(), "cell2".to_string()];
    let work = ["cell2".to_string(), "cell2".to_string()];
    assert_eq!(r.live.exit.renewals(&view, &all, &[]), all);
    assert_eq!(r.live.exit.renewals(&view, &all[..1], &work), all, "unlocked or not, the work's node is in");
    assert!(r.gate.lock_with(RelockReason::Idle, "No changes for 15 minutes"));
    assert!(r.live.exit.renewals(&view, &all, &[]).is_empty(), "nothing running, nothing renewed");
    assert_eq!(r.live.exit.renewals(&view, &all, &work), ["cell2"]);

    lease_file(&r.home, "dev", 0.0);
    let before = heartbeat(&r.home).1;
    r.live.renew_leases(&view, &all, &work).await;
    let (holder, age) = heartbeat(&r.home);
    assert_eq!(holder, "dev", "the test build's fresh lease stands");
    assert!(age >= before);
}

/// Look-only never writes the lease, whatever is running.
#[tokio::test]
async fn on_the_release_path_look_only_renews_no_lease_for_running_work() {
    let r = release("release-work-look", docker()).await;
    restart(&r.live, "cell2").await.unwrap();
    r.gate.set_look_only(true).unwrap();
    assert!(r.live.exit.leases.held().is_empty());
    lease_file(&r.home, "release", lease::REFRESH.as_secs_f64());
    let before = std::fs::read(r.home.join(".sparkbox/controller.json")).unwrap();
    let view = r.live.exit.fence_view(&r.live);
    let work = ["cell2".to_string()];
    assert!(r.live.exit.renewals(&view, &work, &work).is_empty());
    r.live.renew_leases(&view, &work, &work).await;
    assert_eq!(std::fs::read(r.home.join(".sparkbox/controller.json")).unwrap(), before, "nothing written on the node");
    assert!(r.live.exit.leases.held().is_empty());
}

/// Look-only that comes on part-way through a round: after `reads` asks.
struct LookOnlyAfter {
    asks: std::sync::atomic::AtomicUsize,
    reads: usize,
}

impl WritePolicy for LookOnlyAfter {
    fn look_only(&self) -> bool {
        self.asks.fetch_add(1, Ordering::SeqCst) >= self.reads
    }

    fn admin_unlocked(&self) -> bool {
        false
    }
}

/// Look-only can come on during a round, which awaits the node: what's left isn't claimed, and a
/// claim already out is let go of here, so `held()` stays empty.
#[tokio::test]
async fn on_the_release_path_look_only_coming_on_during_a_round_leaves_no_lease_held() {
    let r = release("release-work-flip", docker()).await;
    restart(&r.live, "cell2").await.unwrap();
    let view = r.live.exit.fence_view(&r.live);
    let work = ["cell2".to_string()];
    let file = r.home.join(".sparkbox/controller.json");

    // On before the claim goes out: nothing reaches the node.
    lease_file(&r.home, "release", lease::REFRESH.as_secs_f64());
    let before = std::fs::read(&file).unwrap();
    r.live.exit.leases.forget();
    r.live.set_write_policy(Arc::new(LookOnlyAfter { asks: 0.into(), reads: 1 }));
    r.live.renew_leases(&view, &work, &work).await;
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert!(r.live.exit.leases.held().is_empty());

    // On while it's out: it reached the node, and this build lets go of it.
    r.live.set_write_policy(Arc::new(LookOnlyAfter { asks: 0.into(), reads: 2 }));
    r.live.renew_leases(&view, &work, &work).await;
    assert_ne!(std::fs::read(&file).unwrap(), before);
    assert!(r.live.exit.leases.held().is_empty());
}

/// In the debug build a lease rests on Admin mode or the fence alone: work running under it keeps
/// nothing, so a closed fence still lets go of the node.
#[cfg(debug_assertions)]
#[tokio::test]
async fn the_debug_build_renews_no_lease_for_running_work() {
    let r = rig("debug-work", docker(), "dev").await;
    r.f.open(&spec(&["cell2"], &[FenceClass::Service], "dev"));
    restart(&r.live, "cell2").await.unwrap();
    assert!(r.gate.lock_with(RelockReason::Idle, "No changes for 15 minutes"));
    let work = ["cell2".to_string()];
    let view = r.live.exit.fence_view(&r.live);
    assert_eq!(r.live.exit.renewals(&view, &work, &[]), ["cell2"], "the open fence's node");
    close_fence(&r.live).await.unwrap();
    let view = r.live.exit.fence_view(&r.live);
    assert!(r.live.exit.renewals(&view, &work, &work).is_empty(), "the fence closed: running work keeps nothing");
}

/// The ticker asks what's running, and hands it to the renewals `the_lease_stays_held…` walks.
#[test]
fn the_lease_ticker_asks_what_is_running() {
    let src = include_str!("exit.rs");
    let ticker = src.split("pub fn start(app: &AppHandle) {").nth(1).and_then(|b| b.split("\n}\n").next()).unwrap();
    assert!(ticker.contains("let working = work_under_way(&app, &state);"));
    assert!(ticker.contains("live.renew_leases(&view, &inventory, &working).await;"));
    let asks = src.split("fn work_under_way(").nth(1).and_then(|b| b.split("\n}\n").next()).unwrap();
    assert!(asks.contains("t.working_nodes()") && asks.contains("state.models.serving_nodes()"));
}

/// Every .rs file under src/, tests or not, with its path.
fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((path.clone(), std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    out
}

/// With no fence in the release build, `cfg!(debug_assertions)` where the variable is read is all
/// that stands between SPARKBOX_ADMIN_NOAUTH and a Live unlock with no owner present. A debug build
/// can't test that by running it, so this guards the source, in every build, as
/// `every_command_is_gated_or_listed` does: each read of the variable stops at the compile first.
/// `the_compile_alone_keeps_noauth_out_of_the_release_build` runs it in a release build.
#[test]
fn each_read_of_noauth_stops_at_the_compile_first() {
    let reads_it = |l: &&str| !l.trim_start().starts_with("//") && l.contains("SPARKBOX_ADMIN_NOAUTH") && l.contains("env::var");
    let reads: Vec<(PathBuf, String)> = sources()
        .into_iter()
        .filter(|(path, _)| !path.ends_with("exit_release_tests.rs"))
        .flat_map(|(path, text)| text.lines().filter(reads_it).map(|l| (path.clone(), l.trim().to_string())).collect::<Vec<_>>())
        .collect();
    let lines: Vec<&str> = reads.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(reads.len(), 2, "the gate's and the exit's, no others: {lines:?}");
    for (path, line) in &reads {
        assert!(line.contains("cfg!(debug_assertions) && "), "{}: {line}", path.display());
        assert!(line.find("cfg!(debug_assertions)") < line.find("env::var("), "{}: {line}", path.display());
    }
}

/// The variable set, in a release compile (`scripts/check.sh`): Live still asks for Touch ID, with
/// the gate built as the app builds it. `testing::gate` sets `no_auth` by hand, so it can't show this.
#[cfg(not(debug_assertions))]
#[test]
fn the_compile_alone_keeps_noauth_out_of_the_release_build() {
    use crate::admin::testing::FakeClock;
    std::env::set_var("SPARKBOX_ADMIN_NOAUTH", "1");
    assert!(!scripted_unlock());
    assert!(!Enforce::app().noauth);
    let auth = FakeAuth::default();
    let g = AdminGate::new(Box::new(FakeClock::default()), Box::new(auth.clone()), None);
    g.set_live(true);
    assert!(g.state().requires_auth, "Live asks for Touch ID");
    g.unlock(None).unwrap();
    assert_eq!(auth.asked.load(Ordering::SeqCst), 1, "Touch ID was asked");
    g.lock_with(RelockReason::Explicit, "Locked by the test");
    auth.refuse.store(true, Ordering::SeqCst);
    assert!(g.unlock(None).is_err(), "no Touch ID, no unlock");
    assert_eq!(g.check().unwrap_err(), admin::LOCKED);
    std::env::remove_var("SPARKBOX_ADMIN_NOAUTH");
}

/// A leftover on the node, and the Files that lists it.
struct Left {
    r: Rig,
    files: Arc<crate::files::Files>,
    stage: PathBuf,
    quiet: PathBuf,
    busy: PathBuf,
}

/// A staging folder a failed copy left on cell2, listed, beside two it didn't: one idle past the
/// sweep's 10 minutes, one written a moment ago.
fn left_behind(r: Rig) -> Left {
    let stage = r.home.join(".sparkbox-incoming-7-abc");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::write(stage.join("half.bin"), "partial").unwrap();
    let quiet = r.home.join(".sparkbox-incoming-8-quiet");
    std::fs::create_dir_all(&quiet).unwrap();
    std::fs::write(quiet.join("half.bin"), "partial").unwrap();
    let idle = std::time::SystemTime::now() - Duration::from_secs(20 * 60);
    for p in [quiet.join("half.bin"), quiet.clone()] {
        std::fs::File::open(&p).unwrap().set_modified(idle).unwrap();
    }
    let busy = r.home.join(".sparkbox-incoming-9-busy");
    std::fs::create_dir_all(&busy).unwrap();
    std::fs::write(busy.join("half.bin"), "being written").unwrap();

    // The rig's own folder: tests run side by side, and `temp` removes and remakes the name it's given.
    let files = crate::files::Files::new(r.live.clone(), r.home.join("sparkbox-backups"));
    files.set_gate(r.gate.clone());
    files.set_retry_every(Duration::from_millis(40));
    let home = r.home.canonicalize().unwrap().display().to_string();
    files.keep_leftover(crate::files::Leftover { place: "cell2".into(), dir: home.clone(), nonce: "7-abc".into(), path: format!("{home}/.sparkbox-incoming-7-abc"), bytes: None, why: "connection lost".into() });
    Left { r, files, stage, quiet, busy }
}

/// Waits for the retry's first refusal at the exit (the retry runs on a task of its own).
async fn until_refused(live: &Live) {
    for _ in 0..500 {
        if !refusals(live).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no refusal came");
}

async fn until_gone(files: &crate::files::Files) {
    for _ in 0..200 {
        if files.leftovers().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("still listed: {:?}", files.leftovers());
}

/// The critics' case: this delete used to go out with Admin locked, and it swept folders the failed
/// copy never made. It waits as a push of the alert settings does, and its line says what it was.
#[tokio::test]
async fn on_the_release_path_a_leftover_waits_for_admin_mode_and_says_what_it_was() {
    let l = left_behind(release("release-leftover", fake_node::real_shell()).await);
    let r = &l.r;
    assert!(r.gate.lock_with(RelockReason::Explicit, "Locked by the owner"));
    let expires = r.gate.state().expires_at_ms;
    l.files.retry_leftovers();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(r.node.ran("cleanup").is_empty() && r.node.ran("sweep").is_empty(), "nothing sent with Admin locked");
    assert!(l.stage.exists() && l.quiet.exists(), "nothing deleted");
    assert_eq!(l.files.leftovers().len(), 1, "still listed, with Remove beside it");
    assert_eq!(r.gate.state().expires_at_ms, expires, "the wait doesn't touch Admin mode");

    // Unlocked, it goes on by itself: the folder the copy left, then the idle one beside it.
    r.gate.unlock(None).unwrap();
    until_gone(&l.files).await;
    assert_eq!((r.node.ran("cleanup").len(), r.node.ran("sweep").len()), (1, 1));
    assert!(!l.stage.exists() && !l.quiet.exists());
    assert!(l.busy.exists(), "a folder written to a moment ago stands");
    for word in ["cleanup", "sweep"] {
        let row = r.live.audit.entries().into_iter().find(|e| e.command.contains(word)).unwrap_or_else(|| panic!("{word} has a line"));
        assert_eq!((row.title.as_str(), row.source.as_str(), row.fence.as_deref()), ("Remove the staging folder left on cell2", "system", Some("other")), "{word}");
    }
}

/// The Remove button removes what it names, and nothing beside it. Its line says the owner asked.
#[tokio::test]
async fn on_the_release_path_remove_takes_only_that_folder_and_says_what_it_was() {
    let l = left_behind(release("release-leftover-remove", fake_node::real_shell()).await);
    let path = l.files.leftovers()[0].path.clone();
    l.files.remove_leftover("cell2", &path).await.unwrap();
    assert!(l.files.leftovers().is_empty());
    assert!(!l.stage.exists() && l.quiet.exists());
    assert!(l.r.node.ran("sweep").is_empty());
    let row = l.r.live.audit.entries().into_iter().find(|e| e.command.contains("cleanup")).expect("its line");
    assert_eq!((row.title.as_str(), row.source.as_str()), ("Remove the staging folder left on cell2", "ui"));
}

/// Admin relocks while the folder is coming off the node: the sweep, a second delete, stays home.
#[tokio::test]
async fn on_the_release_path_a_relock_part_way_stops_the_leftover_sweep() {
    let l = left_behind(release("release-leftover-relock", fake_node::real_shell()).await);
    let r = &l.r;
    let python = std::process::Command::new("sh").args(["-c", "command -v python3"]).output().unwrap().stdout;
    let python = String::from_utf8_lossy(&python);
    let entered = r.home.join("cleanup-entered");
    let release = r.home.join("cleanup-release");
    r.node.bin("python3", &format!("#!/bin/sh\nif [ \"$2\" = cleanup ]; then\n  : > \"$HOME/cleanup-entered\"\n  waited=0\n  while [ ! -f \"$HOME/cleanup-release\" ]; do\n    [ \"$waited\" -lt 1200 ] || exit 75\n    waited=$((waited + 1))\n    sleep 0.05\n  done\nfi\nexec {} \"$@\"\n", crate::live::sh_quote(python.trim())));
    l.files.retry_leftovers();
    fake_node::wait_for("cleanup entered its barrier", || entered.exists().then_some(()), || format!("commands: {:?}; audit: {:?}", r.node.log.lock().unwrap(), r.live.audit.entries())).await;
    assert_eq!(r.node.ran("cleanup").len(), 1);
    assert!(r.gate.lock_with(RelockReason::Idle, "No changes for 15 minutes"));
    std::fs::write(release, "continue").unwrap();
    fake_node::wait_for("in-flight cleanup finished after relock", || l.files.leftovers().is_empty().then_some(()), || format!("leftovers: {:?}; audit: {:?}", l.files.leftovers(), r.live.audit.entries())).await;
    assert!(!l.stage.exists(), "the folder that was already on its way out");
    assert!(r.node.ran("sweep").is_empty(), "{:?}", r.node.ran("sweep"));
    assert!(l.quiet.exists());
}

/// The test build's lease stands between the sweep and that build's staging folder.
#[tokio::test]
async fn on_the_release_path_a_leftover_stops_at_the_test_builds_lease() {
    let l = left_behind(release("release-leftover-lease", fake_node::real_shell()).await);
    let r = &l.r;
    lease_file(&r.home, "dev", 0.0);
    l.files.retry_leftovers();
    until_refused(&r.live).await;
    assert!(r.node.ran("cleanup").is_empty() && r.node.ran("sweep").is_empty(), "stopped at the exit");
    assert!(l.stage.exists() && l.quiet.exists());
    assert_eq!(l.files.leftovers().len(), 1);
    let refused = refusals(&r.live);
    let why = refused.first().and_then(|e| e.error.as_deref()).unwrap_or_default();
    assert!(why.starts_with("Clean up a copy to ") && why.ends_with(": SparkBox (dev) is controlling cell2 right now."), "{why}");
}

/// Admin unlocked is not enough in the debug build.
#[cfg(debug_assertions)]
#[tokio::test]
async fn the_debug_build_refuses_a_leftover_with_no_fence() {
    let l = left_behind(rig("debug-leftover", fake_node::real_shell(), "dev").await);
    l.r.gate.check().expect("Admin mode is confirmed");
    l.files.retry_leftovers();
    until_refused(&l.r.live).await;
    assert!(l.r.node.ran("cleanup").is_empty() && l.r.node.ran("sweep").is_empty());
    assert!(l.stage.exists());
    assert_eq!(l.files.leftovers().len(), 1);
    let refused = refusals(&l.r.live);
    let why = refused.first().and_then(|e| e.error.as_deref()).unwrap_or_default();
    assert!(why.ends_with("No fence is open, so changes on the cluster are refused. This test build needs one: open it in the release SparkBox (Settings › Fence)."), "it tried, and the exit said no: {why}");
}

/// A compile that disagrees with its profile stops at `compile_error!`, so a test can't watch it
/// happen: the round's report has the commands that do. This keeps the guard from going quietly.
#[test]
fn a_compile_that_disagrees_with_its_profile_is_refused() {
    assert_eq!(cfg!(sparkbox_debug_profile), cfg!(debug_assertions));
    let src = include_str!("exit.rs");
    for guard in ["#[cfg(all(sparkbox_debug_profile, not(debug_assertions)))]\ncompile_error!(", "#[cfg(all(not(sparkbox_debug_profile), debug_assertions))]\ncompile_error!("] {
        assert!(src.contains(guard), "{guard}");
    }
    assert!(include_str!("../../build.rs").contains(r#"println!("cargo:rustc-cfg=sparkbox_debug_profile");"#));
}

#[test]
fn the_app_opens_a_fence_only_for_the_test_build() {
    let dev = spec(&["cell2"], &[FenceClass::ContainerTest], "dev");
    for_the_test_build(&dev).unwrap();
    let e = for_the_test_build(&Spec { instance: "release".into(), ..dev.clone() }).unwrap_err();
    assert_eq!(e.message, "A fence is for the test build: the release build's changes need Admin mode, not a fence.");
    // Whatever the spec says: the refusal is for anything that isn't the test build's.
    assert!(for_the_test_build(&Spec { instance: String::new(), ..dev }).is_err());
}

/// `Exit::default` under cfg(test) has no fence, pin or lease, and `--cfg test` from a wrapper
/// compiles as if it were a test. `run` is the only way into the app, and it stops there. If the
/// stop ever goes, the first check fails with other text, so this test can't start the app.
#[test]
#[should_panic(expected = "A SparkBox compiled with cfg(test) has no write exit.")]
fn a_build_compiled_as_a_test_stops_before_the_app_starts() {
    let body = include_str!("../lib.rs").split("pub fn run() {").nth(1).unwrap();
    let first = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("//")).unwrap();
    assert_eq!(first, "if cfg!(test) {");
    crate::run();
}

/// Nothing outside the compile reaches the decision: it's a constant, and the seam tests use is
/// compiled into tests only. A guard on the source, as `every_command_is_gated_or_listed` is.
#[test]
fn nothing_at_run_time_can_lift_the_fence() {
    let src = include_str!("exit.rs");
    assert!(src.contains("pub const BUILD_NEEDS_FENCE: bool = cfg!(debug_assertions);"));
    let body = src.split("fn needs_fence(&self) -> bool {").nth(1).and_then(|b| b.split("\n    }\n").next()).unwrap();
    assert_eq!(
        body.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>(),
        ["#[cfg(test)]", "if self.release_path.load(Ordering::SeqCst) {", "return false;", "}", "BUILD_NEEDS_FENCE"],
    );
    for item in ["release_path: AtomicBool,", "release_path: AtomicBool::new(false),", "pub fn walk_release_path(&self) {", "use std::sync::atomic::{AtomicBool, Ordering};"] {
        let at = src.find(item).unwrap_or_else(|| panic!("{item}"));
        assert!(src[..at].trim_end().ends_with("#[cfg(test)]"), "{item} must be compiled into tests only");
    }
    let code: Vec<&str> = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
    assert_eq!(code.iter().filter(|l| l.contains("BUILD_NEEDS_FENCE")).count(), 2, "the constant and needs_fence, nothing else");
    assert_eq!(code.iter().filter(|l| l.contains("release_path")).count(), 5, "the field, its start, needs_fence, and the test-only setter (name and body)");
    assert_eq!(code.iter().filter(|l| l.contains("needs_fence()")).count(), 3, "the two ways a change leaves, admit_change and admit_from_mac, and `renewals`, which admits nothing");
}
