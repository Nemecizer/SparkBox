//! The refusals of 2026-10-30, through their real entry points: the owner's release app refused the
//! owner's own Hugging Face download ("The fence doesn't allow this change."), and a load on two
//! nodes needs the same path. On the release build's path both go through with Admin confirmed, and
//! stay refused in look-only and on an identity mismatch; the debug build refuses both with no
//! fence. live/exit_release_tests.rs has the rest of the path.
//!
//! The exit's rules name this compile's instance, as a load's labels do (the seam,
//! `Live::walk_release_path`, moves only the fence decision). Admin mode is the app's own gate
//! with a stand-in for Touch ID, and no fence record exists anywhere.

use std::sync::atomic::Ordering;

use super::*;
use crate::admin::testing::gate;
use crate::admin::{AdminGate, RelockReason};
use crate::errors::{ErrorKind, SbError};
use crate::fake_node::Node;
use crate::live::exit::{hand_over, Enforce};
use crate::live::{fence, identity_pin};

#[derive(Clone, Copy, PartialEq)]
#[cfg_attr(not(debug_assertions), allow(dead_code))]
enum Path_ {
    Release,
    Debug,
}

struct Rig {
    c: Cluster,
    head: Node,
    worker: Node,
    gate: Arc<AdminGate>,
    config: PathBuf,
}

const ANSWERS: &str = "running 0\n200\nINFO Application startup complete.\n";

/// A GB10 for these tests: `vllm_script`, and sparkjob.py's answer to a started job.
fn gb10(n: u8) -> Script {
    let engine = vllm_script(Arc::new(Mutex::new(vec![ANSWERS])), n);
    Arc::new(move |cmd, stdin| {
        if cmd.contains("sparkjob.py start download") {
            let id = cmd.split_whitespace().last().unwrap_or_default();
            return Some((json!({ "id": id, "kind": "download", "state": "running" }).to_string(), 0));
        }
        engine(cmd, stdin)
    })
}

/// cell1 and cell2 with this script, the exit enforced with no fence, and Admin confirmed.
async fn rig(name: &str, path: Path_, script: impl Fn(u8) -> Script) -> Rig {
    let (head, p1) = fake_node(&temp(&format!("{name}-1")), script(1)).await;
    let (worker, p2) = fake_node(&temp(&format!("{name}-2")), script(2)).await;
    let c = cluster(name, &[("cell1", p1), ("cell2", p2)]).await;
    c.models.set_unload_wait(Duration::from_millis(50), Duration::from_millis(100));
    let instance = crate::identity::instance_name();
    let config = c.dir.join(format!("config-{instance}"));
    std::fs::create_dir_all(&config).unwrap();
    let rules = Enforce { instance: instance.into(), fence_dir: c.dir.join("SparkBox Fence"), config_dir: Some(config.clone()), noauth: false, tag: "release-exit".into(), mac: Some("Studio".into()) };
    assert!(!rules.fence_dir.join(fence::RECORD).exists());
    c.live.enforce_writes(rules);
    if path == Path_::Release {
        c.live.walk_release_path();
    }
    let (gate, _, auth) = gate();
    gate.set_live(true);
    hand_over(&gate, &c.live);
    gate.unlock(None).expect("Touch ID says yes");
    assert_eq!(auth.asked.load(Ordering::SeqCst), 1, "a Live unlock asks for Touch ID");
    Rig { c, head, worker, gate, config }
}

/// What `Models::download` starts once Hugging Face has answered.
async fn download(r: &Rig, id: &str) -> Result<Job, SbError> {
    let model = hf::parse_model(&json!({ "id": "Qwen/Qwen3-8B", "sha": "abc123" }), Value::Null, None, "");
    let spec = download_spec(&model, "~/models/Qwen/Qwen3-8B", "nvcr.io/nvidia/vllm:26.05-py3", String::new());
    r.c.models.start_job("cell2", "download", id, &spec).await.map_err(SbError::from)
}

/// A load on both nodes.
async fn load(r: &Rig, id: &str) -> Deployment {
    r.c.models.deploy(&r.c.ctx(), DeploySpec { id: id.into(), ..spec(&["cell1", "cell2"], 2, 1) }).await.unwrap()
}

/// cell2 was pinned as another machine: what the pin sees when a different machine answers.
fn cell2_answers_as_someone_else(r: &Rig) {
    let pin = identity_pin::Pin { hostname: "cell3-example".into(), pinned_ms: 1, ..Default::default() };
    crate::util::save_json(&r.config.join(identity_pin::FILE), &std::collections::BTreeMap::from([("cell2", pin)])).unwrap();
}

#[tokio::test]
async fn a_download_and_a_two_node_load_go_through_with_admin_confirmed() {
    let r = rig("rel-ok", Path_::Release, gb10).await;
    // What `admin::require` asks at the start of each command.
    r.gate.check().expect("Admin mode is confirmed");

    let job = download(&r, "dl-rel-ok").await.unwrap();
    assert_eq!((job.node.as_str(), job.state.as_str()), ("cell2", "running"));
    assert_eq!(r.worker.ran("sparkbox.py start download").len() + r.worker.ran("sparkjob.py start download").len(), 1);

    r.gate.check().unwrap();
    load(&r, "rel-ok").await;
    let d = wait_state(&r.c, "rel-ok", "ready").await;
    assert_eq!(d.stage, Some(Stage::Ready));
    for node in [&r.head, &r.worker] {
        assert_eq!(node.ran("sparkjob.py deploy-files").len(), 1, "its launch files were written");
        assert_eq!(node.ran("docker run -d").len(), 1, "and its container started");
    }
    let refused: Vec<_> = r.c.live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect();
    assert!(refused.is_empty(), "{refused:#?}");
    // The download's row is `other`, which only an `any` fence admitted; a load's rows say what they are.
    let rows = r.c.live.audit.entries();
    let row = |what: &str| rows.iter().find(|e| e.command.contains(what)).unwrap_or_else(|| panic!("no row for {what}"));
    assert_eq!(row("sparkjob.py start download").fence.as_deref(), Some("other"));
    assert_eq!(row("sparkjob.py deploy-files").fence.as_deref(), Some("model-load"));
    assert_eq!(row("docker run -d").fence.as_deref(), Some("model-load"));
    r.c.models.unload(&r.c.ctx(), "rel-ok").await.unwrap();
    wait_state(&r.c, "rel-ok", "stopped").await;
    r.c.live.stop();
}

#[tokio::test]
async fn in_look_only_both_are_still_refused() {
    let r = rig("rel-look", Path_::Release, gb10).await;
    r.gate.set_look_only(true).unwrap();

    let e = download(&r, "dl-rel-look").await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::LookOnly);
    assert!(r.worker.ran("sparkjob.py start").is_empty());

    load(&r, "rel-look").await;
    let d = wait_state(&r.c, "rel-look", "failed").await;
    assert_eq!(d.message, crate::admin::LOOK_ONLY);
    for node in [&r.head, &r.worker] {
        assert!(node.ran("deploy-files").is_empty() && node.ran("docker run").is_empty());
        assert!(!node.home.join(format!(".sparkbox/{}", plan::launcher_name())).exists(), "launch.py waits for the exit");
    }
    r.c.live.stop();
}

#[tokio::test]
async fn on_an_identity_mismatch_both_are_still_refused() {
    let r = rig("rel-pin", Path_::Release, gb10).await;
    cell2_answers_as_someone_else(&r);

    let e = download(&r, "dl-rel-pin").await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::WrongNode);
    assert!(e.message.starts_with("cell2 answers as ") && e.message.contains("changes refused"), "{}", e.message);
    assert!(r.worker.ran("sparkjob.py start").is_empty());

    load(&r, "rel-pin").await;
    let d = wait_state(&r.c, "rel-pin", "failed").await;
    assert!(d.message.contains("changes refused"), "{}", d.message);
    assert!(r.worker.ran("deploy-files").is_empty() && r.worker.ran("docker run").is_empty());
    // cell1 got its files before cell2 refused: the rollback took them away again.
    assert!(r.head.ran("docker run").is_empty(), "the head never started");
    assert_eq!(r.head.ran("ids=$(docker ps -aq").len(), 1, "{:?}", r.head.ran("ids=$"));
    r.c.live.stop();
}

/// The debug build, with Admin confirmed all the same: what the release build did before, and
/// what the test build keeps doing.
#[cfg(debug_assertions)]
#[tokio::test]
async fn the_debug_build_refuses_both_with_no_fence() {
    let r = rig("dbg-none", Path_::Debug, gb10).await;
    r.gate.check().expect("Admin mode is confirmed");

    let e = download(&r, "dl-dbg-none").await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Fenced);
    assert!(e.message.starts_with("No fence is open"), "{}", e.message);
    assert!(r.worker.ran("sparkjob.py start").is_empty());

    load(&r, "dbg-none").await;
    let d = wait_state(&r.c, "dbg-none", "failed").await;
    assert!(d.message.starts_with("No fence is open"), "{}", d.message);
    for node in [&r.head, &r.worker] {
        assert!(node.ran("deploy-files").is_empty() && node.ran("docker run").is_empty());
        assert!(!node.home.join(format!(".sparkbox/{}", plan::launcher_name())).exists(), "launch.py waits for the exit");
    }
    assert!(!r.c.live.audit.entries().iter().any(|e| e.command.contains("launch.py")), "nothing to log");
    r.c.live.stop();
}

/// The Mac's screen locks while a load's workers are being started: the load finishes all the same.
/// A worker's `docker run` is a process of its own on its node, as it is on a GB10: it waits until
/// the test has relocked.
#[tokio::test]
async fn a_load_outlives_admin_relocking_while_it_starts() {
    let script = |n: u8| -> Script {
        let engine = gb10(n);
        Arc::new(move |cmd: &str, stdin: &[u8]| if n == 2 && cmd.contains("docker run -d") { None } else { engine(cmd, stdin) })
    };
    let r = rig("rel-relock", Path_::Release, script).await;
    let flags = temp("rel-relock-flags");
    std::fs::create_dir_all(&flags).unwrap();
    let flag = |name: &str| flags.join(name);
    let docker = format!(
        "case \"$1\" in\n  run) touch '{}'; while [ ! -e '{}' ]; do sleep 0.05; done ;;\nesac\nexit 0\n",
        flag("starting").display(),
        flag("relocked").display()
    );
    r.worker.bin("docker", &docker);

    load(&r, "rel-relock").await;
    for _ in 0..1500 {
        if flag("starting").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(flag("starting").exists(), "the worker's run started");
    assert!(r.head.ran("docker run -d").is_empty(), "the head waits for it");

    assert!(r.gate.lock_with(RelockReason::ScreenLock, "The screen locked"));
    assert_eq!(r.gate.check().unwrap_err(), crate::admin::LOCKED, "a new action stops at the gate");
    std::fs::write(flag("relocked"), b"").unwrap();

    let d = wait_state(&r.c, "rel-relock", "ready").await;
    assert_eq!(d.stage, Some(Stage::Ready));
    assert_eq!((r.head.ran("docker run -d").len(), r.worker.ran("docker run -d").len()), (1, 1), "the head started after the relock");
    let refused: Vec<_> = r.c.live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect();
    assert!(refused.is_empty(), "{refused:#?}");
    r.c.live.stop();
}
