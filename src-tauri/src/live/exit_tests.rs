//! The write exit against fake nodes (fake_node.rs): look-only, the fence, the controller
//! lease, the identity pin and the other SparkBox's containers, through the real Live entry
//! points. Each test opts in to enforcement (`Live::enforce_writes`); the default stays
//! permissive for every other test. A test is a debug build, so the exit asks for a fence here
//! whatever instance the rules name (exit_release_tests.rs walks the release build's path).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::errors::{ErrorKind, SbError};
use crate::fake_node::{self, fake_node, fake_spark, temp, Profile, Script};
use crate::inventory::NodeConfig;
use crate::live::fence::{self, Spec};
use crate::live::fence_class::FenceClass;
use crate::live::{audit, identity_pin, Live};
use crate::model::LinkStatus;

/// Look-only and Admin mode, switched by the test.
pub(super) struct Policy {
    look_only: AtomicBool,
}

impl Policy {
    pub(super) fn new(look_only: bool) -> Arc<Policy> {
        Arc::new(Policy { look_only: AtomicBool::new(look_only) })
    }
}

impl WritePolicy for Policy {
    fn look_only(&self) -> bool {
        self.look_only.load(Ordering::SeqCst)
    }

    fn admin_unlocked(&self) -> bool {
        true
    }
}

/// A fence folder, this build's config folder and the signing key, all under `base`.
pub(super) struct TestFence {
    pub(super) rules: Enforce,
    key: p256::ecdsa::SigningKey,
    file: fence::KeyFile,
}

impl TestFence {
    pub(super) fn new(base: &Path, instance: &str) -> TestFence {
        let rules = Enforce {
            instance: instance.into(),
            fence_dir: base.join("SparkBox Fence"),
            config_dir: Some(base.join(format!("config-{instance}"))),
            noauth: false,
            tag: "p1-r1-test".into(),
            mac: Some("Studio".into()),
        };
        std::fs::create_dir_all(&rules.fence_dir).unwrap();
        let key = fence::new_key().unwrap();
        let (key_id, public_key) = fence::public_of(&key).unwrap();
        let file = fence::KeyFile { key_id, public_key, kind: "keychain".into() };
        fence::save_trust(rules.config_dir.as_ref().unwrap(), &file).unwrap();
        TestFence { rules, key, file }
    }

    fn config(&self) -> PathBuf {
        self.rules.config_dir.clone().unwrap()
    }

    /// Opens a fence for `spec`, created `ago_min` minutes ago.
    fn open_at(&self, spec: &Spec, ago_min: u64) -> fence::Record {
        let r = fence::record_for(spec, &self.key, now_ms() - ago_min * 60_000, "Studio").unwrap();
        fence::write(&self.rules.fence_dir, &self.config(), &self.file, &r).unwrap();
        r
    }

    pub(super) fn open(&self, spec: &Spec) -> fence::Record {
        self.open_at(spec, 0)
    }
}

pub(super) fn spec(nodes: &[&str], classes: &[FenceClass], instance: &str) -> Spec {
    Spec { nodes: nodes.iter().map(|n| n.to_string()).collect(), classes: classes.to_vec(), minutes: 180, instance: instance.into(), ..Default::default() }
}

/// A GB10's docker, which the fake Mac doesn't have.
pub(super) fn docker() -> Script {
    Arc::new(|cmd: &str, _: &[u8]| cmd.starts_with("docker ").then(|| (String::new(), 0)))
}

pub(super) fn err(e: String) -> SbError {
    SbError::from(e)
}

pub(super) fn refusals(live: &Live) -> Vec<audit::Entry> {
    live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect()
}

const RESTART: &str = "systemctl restart -- 'earlyoom.service'";

async fn restart(live: &Live, node: &str) -> Result<(), SbError> {
    live.sudo(node, RESTART, Duration::from_secs(30)).await.map(drop).map_err(err)
}

#[tokio::test]
async fn look_only_holds_changes_inference_and_the_lease_but_not_reads() {
    let home = temp("exit-look-home");
    let (node, port) = fake_node(&home, docker()).await;
    let (live, _) = fake_node::connect(&temp("exit-look-app"), &[("cell1", port)]).await;
    let policy = Policy::new(true);
    live.set_write_policy(policy.clone());

    // A change: refused with LookOnly, one "Refused" line, and nothing reached the node.
    let e = err(live.container_action("cell1", "0123456789ab", "restart").await.unwrap_err());
    assert_eq!((e.kind, e.node.as_deref()), (ErrorKind::LookOnly, Some("cell1")));
    assert!(node.ran("docker restart").is_empty());
    let rows = refusals(&live);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].command.as_str(), rows[0].state.as_str(), rows[0].changes), ("docker restart 0123456789ab", "failed", true));
    // sudo, a sparkjob write and a shell are held the same way.
    assert_eq!(restart(&live, "cell1").await.unwrap_err().kind, ErrorKind::LookOnly);
    assert_eq!(err(live.sparkjob("cell1", "rm '/tmp/x'", None, Duration::from_secs(10)).await.unwrap_err()).kind, ErrorKind::LookOnly);
    assert!(live.pty("cell1", 80, 24, None).await.is_err());
    assert!(node.ran("systemctl restart").is_empty() && node.ran("sparkjob.py rm").is_empty());

    // A read passes, and so does sparkjob's.
    let out = live.run("cell1", "uptime", None, Duration::from_secs(10)).await.unwrap();
    assert_eq!(out.code, Some(0));
    live.sparkjob("cell1", "df /", None, Duration::from_secs(20)).await.unwrap();

    // Inference on a node (an engine's readiness, a test token) is held too.
    let e = err(as_use(live.probe("cell1", "curl -s -m 2 http://127.0.0.1:8000/v1/models", Duration::from_secs(5))).await.unwrap_err());
    assert_eq!(e.kind, ErrorKind::LookOnly);
    assert!(node.ran("curl").is_empty());
    // A background refusal is written once, as a probe line, not as a change.
    for _ in 0..3 {
        let _ = as_use(live.probe("cell1", "curl -s -m 2 http://127.0.0.1:8000/metrics", Duration::from_secs(5))).await;
    }
    let held: Vec<_> = refusals(&live).into_iter().filter(|r| r.command.contains("/metrics")).collect();
    assert_eq!(held.len(), 1);
    assert_eq!((held[0].class.as_str(), held[0].changes), ("probe", false));

    // The lease is bookkeeping: held as well.
    let e = err(live.sparkjob("cell1", "lease claim 'dev' 't' 'Studio' 45", None, Duration::from_secs(10)).await.unwrap_err());
    assert_eq!(e.kind, ErrorKind::LookOnly);
    assert!(!home.join(".sparkbox/controller.json").exists());

    // The approved helper refresh still happens: a changed collector.py is put back on reconnect.
    std::fs::write(home.join(".sparkbox/collector.py"), "# changed by hand\n").unwrap();
    live.reconnect_all();
    let mut back = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if std::fs::read_to_string(home.join(".sparkbox/collector.py")).is_ok_and(|c| c == super::super::COLLECTOR) {
            back = true;
            break;
        }
    }
    assert!(back, "install() refreshed the collector in look-only");

    // Background checks only read, look-only or not.
    policy.look_only.store(false, Ordering::SeqCst);
    let e = err(live.probe("cell1", "touch /tmp/sparkbox-probe-wrote", Duration::from_secs(5)).await.unwrap_err());
    assert_eq!(e.kind, ErrorKind::Invalid);
    assert!(e.message.contains("Background checks only read"), "{}", e.message);
    assert!(node.ran("touch").is_empty());
}

#[tokio::test]
async fn the_fence_admits_only_what_it_names() {
    let (_n2, p2) = fake_node(&temp("exit-fence-home2"), docker()).await;
    let (_n3, p3) = fake_node(&temp("exit-fence-home3"), docker()).await;
    let base = temp("exit-fence-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", p2), ("cell3", p3)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());

    // No fence: refused.
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Fenced);
    assert!(e.message.contains("No fence is open"), "{}", e.message);

    // Expired (before any newer fence was seen: an older record after one is refused below).
    let mut s = spec(&["cell2"], &[FenceClass::Service], "release");
    s.minutes = 60;
    let old = f.open_at(&s, 61);
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!(e.message, format!("The fence expired at {}.", fence::clock(old.expires_ms)));

    // cell2 and `service`: admitted, and its activity line names the class that let it through.
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    restart(&live, "cell2").await.unwrap();
    let row = live.audit.entries().into_iter().find(|e| e.kind == "sudo" && e.command == RESTART && e.node == "cell2").unwrap();
    assert_eq!(row.fence.as_deref(), Some("service"));
    // A read needs no fence, and its line names none.
    live.run("cell2", "uptime", None, Duration::from_secs(10)).await.unwrap();
    assert_eq!(live.audit.entries().into_iter().find(|e| e.command == "uptime").unwrap().fence, None);

    // Another node, another class.
    let e = restart(&live, "cell3").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "cell3 isn't in the open fence."));
    f.open(&spec(&["cell2"], &[FenceClass::ContainerOwner], "release"));
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow service changes.");

    // A hand edit adding gx10-0000 fails its check; putting the original back is still refused.
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    restart(&live, "cell2").await.unwrap();
    let path = f.rules.fence_dir.join(fence::RECORD);
    let original = std::fs::read(&path).unwrap();
    let mut edited: serde_json::Value = serde_json::from_slice(&original).unwrap();
    edited["nodes"] = json!(["cell2", "gx10-0000"]);
    std::fs::write(&path, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!(e.message, "The fence record failed its check: open a new one.");
    std::fs::write(&path, &original).unwrap();
    assert_eq!(restart(&live, "cell2").await.unwrap_err().message, "The fence record failed its check: open a new one.", "the latch holds");
    tokio::time::sleep(Duration::from_millis(5)).await;
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    restart(&live, "cell2").await.expect("a new fence opens it again");

    // A fence for the other build.
    f.open(&spec(&["cell2"], &[FenceClass::Service], "dev"));
    assert_eq!(restart(&live, "cell2").await.unwrap_err().message, "This fence is for the dev build.");
    let dev = Enforce { instance: "dev".into(), ..f.rules.clone() };
    live.enforce_writes(dev);
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    assert_eq!(restart(&live, "cell2").await.unwrap_err().message, "This fence is for the release build.");
    live.enforce_writes(f.rules.clone());

    // `any` can't be opened for the dev build.
    let ids = vec!["cell2".to_string(), "cell3".to_string()];
    assert!(fence::validate(&spec(&["cell2"], &[FenceClass::Any], "dev"), &ids).is_err());
    // `any` (release only) admits a change SparkBox doesn't name.
    f.open(&spec(&["cell2"], &[FenceClass::Any], "release"));
    live.sudo("cell2", "touch /tmp/sparkbox-any", Duration::from_secs(10)).await.unwrap();

    // `only`: earlyoom.service yes, cups.service no.
    let mut s = spec(&["cell2"], &[FenceClass::Service], "release");
    s.only.insert(FenceClass::Service, vec!["earlyoom.service".into()]);
    f.open(&s);
    restart(&live, "cell2").await.unwrap();
    let e = live.sudo("cell2", "systemctl restart -- 'cups.service'", Duration::from_secs(10)).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow service changes to cups.service: it names only earlyoom.service.");

    // SPARKBOX_ADMIN_NOAUTH with no fence open.
    fence::close(&f.rules.fence_dir, &f.config(), now_ms()).unwrap();
    live.enforce_writes(Enforce { noauth: true, ..f.rules.clone() });
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "SPARKBOX_ADMIN_NOAUTH needs an open fence in Live."));

    // Every refusal above is one "Refused" line.
    assert_eq!(refusals(&live).len(), 10, "{:#?}", refusals(&live).iter().map(|r| r.error.clone()).collect::<Vec<_>>());
}

#[tokio::test]
async fn fleet_commands_are_their_own_class() {
    let (_n, port) = fake_node(&temp("exit-fleet-home"), docker()).await;
    let base = temp("exit-fleet-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    // A typed read runs; a typed change needs `fleet`, whatever its shape.
    let token = crate::live::CancelToken::new;
    audit::as_typed(live.stream_cmd("cell2", "uptime", Duration::from_secs(10), token(), |_, _| {})).await.unwrap();
    let e = audit::as_typed(live.stream_cmd("cell2", "touch /tmp/sparkbox-fleet", Duration::from_secs(10), token(), |_, _| {})).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow Fleet commands.");
    let e = audit::as_typed(live.sudo("cell2", RESTART, Duration::from_secs(10))).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow Fleet commands.", "typed sudo is Fleet's too");
    f.open(&spec(&["cell2"], &[FenceClass::Fleet], "release"));
    audit::as_typed(live.stream_cmd("cell2", "touch /tmp/sparkbox-fleet", Duration::from_secs(10), token(), |_, _| {})).await.unwrap();
    let row = live.audit.entries().into_iter().find(|e| e.command == "touch /tmp/sparkbox-fleet" && e.kind == "stream").unwrap();
    assert_eq!(row.fence.as_deref(), Some("fleet"));
    // A call site's own tag decides over the shape.
    let e = as_class(FenceClass::PowerOne, live.run("cell2", "uptime", None, Duration::from_secs(10))).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow restarts and shutdowns.");
}

/// Writes a lease file on the node as another SparkBox would have.
pub(super) fn lease_file(home: &Path, instance: &str, age_s: f64) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    std::fs::write(home.join(".sparkbox/controller.json"), json!({"instance": instance, "tag": "g0", "mac": "Studio", "heartbeat": now - age_s}).to_string()).unwrap();
}

#[tokio::test]
async fn the_lease_keeps_the_two_sparkboxes_apart() {
    let home = temp("exit-lease-home");
    let (_n, port) = fake_node(&home, docker()).await;
    let base = temp("exit-lease-app");
    let (release, _) = fake_node::connect(&base.join("release"), &[("cell2", port)]).await;
    let (dev, _) = fake_node::connect(&base.join("dev"), &[("cell2", port)]).await;
    let fr = TestFence::new(&base.join("release"), "release");
    let fd = TestFence::new(&base.join("dev"), "dev");
    release.enforce_writes(fr.rules.clone());
    dev.enforce_writes(fd.rules.clone());
    fr.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    fd.open(&spec(&["cell2"], &[FenceClass::Service], "dev"));

    // The release build changes cell2 and holds its lease; the dev build is told who does.
    restart(&release, "cell2").await.unwrap();
    let held: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(".sparkbox/controller.json")).unwrap()).unwrap();
    assert_eq!((held["instance"].as_str(), held["tag"].as_str(), held["mac"].as_str()), (Some("release"), Some("p1-r1-test"), Some("Studio")));
    let e = restart(&dev, "cell2").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "SparkBox (release) is controlling cell2 right now."));
    // The claim's answer is kept for 5 s: a second try doesn't ask the node again.
    assert_eq!(restart(&dev, "cell2").await.unwrap_err().kind, ErrorKind::OtherController);
    // Its claim was written to the log once (when it took the lease), not on every change.
    restart(&release, "cell2").await.unwrap();
    assert_eq!(release.audit.entries().iter().filter(|e| e.command.contains("lease claim")).count(), 1);

    // A heartbeat 60 s old is stale: the dev build takes over.
    lease_file(&home, "release", 60.0);
    dev.exit.leases.forget();
    restart(&dev, "cell2").await.unwrap();
    let held: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(".sparkbox/controller.json")).unwrap()).unwrap();
    assert_eq!(held["instance"], "dev");
    release.exit.leases.forget();
    assert_eq!(restart(&release, "cell2").await.unwrap_err().message, "SparkBox (dev) is controlling cell2 right now.");

    // Releasing (a clean fence close or a quit) frees it at once.
    release_all(&dev, true).await;
    assert!(!home.join(".sparkbox/controller.json").exists());
    release.exit.leases.forget();
    restart(&release, "cell2").await.unwrap();
    // What lease_state reads: the holder, from the node's own clock.
    let out = release.probe("cell2", lease::SHOW, Duration::from_secs(10)).await.unwrap();
    let view = lease::parse_show("cell2", &out.stdout, "release");
    assert_eq!((view.held_by.as_deref(), view.mine), (Some("release"), true));
    assert!(view.age_s.unwrap() < 10);
    let view = lease::parse_show("cell2", &out.stdout, "dev");
    assert!(!view.mine);
}

/// Points `cell2` at another port: what the inventory would say after an IP change.
pub(super) fn cell2_on(port: u16) -> NodeConfig {
    NodeConfig { id: "cell2".into(), name: "cell2".into(), host: "cell2.local".into(), ip: Some("127.0.0.1".into()), model: "MSI EdgeXpert".into(), port, user: "sparkuser".into(), fabric: None }
}

pub(super) async fn wait_hostname(live: &Live, want: &str) {
    let inv = crate::inventory::Inventory { nodes: live.configs.lock().unwrap().values().cloned().collect(), switches: vec![] };
    for _ in 0..200 {
        let snap = live.snapshot(&inv);
        if snap.nodes.iter().any(|n| n.id == "cell2" && n.status == LinkStatus::Online && n.system.hostname == want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("cell2 never answered as {want}");
}

#[tokio::test]
async fn a_node_answering_as_another_is_refused_changes_but_still_read() {
    let (_n2, p2) = fake_spark(&temp("exit-id-cell2"), "cell2").await;
    let (_n3, p3) = fake_spark(&temp("exit-id-cell3"), "cell3").await;
    let base = temp("exit-id-app");
    let (live, _) = fake_node::connect_sparks(&base, &[("cell2", p2)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));

    // Its first good frame pins it: hostname and LAN MAC from the frame.
    live.exit.pins.pin_new(&live, &f.config(), now_ms()).await;
    let pin = identity_pin::load(&f.config()).remove("cell2").unwrap();
    assert_eq!((pin.hostname.as_str(), pin.lan_mac.as_deref()), ("cell2-example", Some("fc:9d:05:9b:e9:14")));
    assert_eq!(pin.host_key.as_deref(), Some(Profile::spark("cell2").fingerprint().as_str()));
    restart(&live, "cell2").await.unwrap();

    // cell3 answers on cell2's address, with the cloned host key both present.
    assert_eq!(Profile::spark("cell3").fingerprint(), Profile::spark("cell2").fingerprint());
    crate::live::trust_host_key(&live.data_path("known_hosts.json"), &format!("127.0.0.1:{p3}"), &Profile::spark("cell3").fingerprint()).unwrap();
    live.start(&[cell2_on(p3)]);
    wait_hostname(&live, "cell3-example").await;
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::WrongNode);
    assert_eq!(e.message, "cell2 answers as cell3-example: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence.");
    // Reads keep working.
    assert_eq!(live.run("cell2", "uptime", None, Duration::from_secs(10)).await.unwrap().code, Some(0));
    let view = live.exit.pins.view(&live, &f.config(), &[("cell2".into(), "cell2".into())]).await;
    assert_eq!(view[0].matches, Some(false));
    assert_eq!(view[0].now.as_ref().map(|n| n.hostname.as_str()), Some("cell3-example"));

    // Re-pinning accepts it as cell2 (Settings › Fence, with Touch ID).
    live.exit.pins.repin(&live, &f.config(), "cell2", now_ms()).await.unwrap();
    restart(&live, "cell2").await.unwrap();
}

#[tokio::test]
async fn a_cloned_machine_id_pins_hostname_and_lan_mac_only() {
    let mut ports = Vec::new();
    for (name, id) in [("cell1", "4f1c0a6be0e04d6a9a3f0c2b5d6e7f80"), ("cell2", "4f1c0a6be0e04d6a9a3f0c2b5d6e7f80"), ("gx10-0000", "9a9a0a6be0e04d6a9a3f0c2b5d6e7f81")] {
        let home = temp(&format!("exit-mid-{name}"));
        let (node, port) = fake_spark(&home, name).await;
        std::fs::write(PathBuf::from(format!("{}.spark/root/etc/machine-id", home.display())), format!("{id}\n")).unwrap();
        ports.push((name, port, node));
    }
    let base = temp("exit-mid-app");
    let list: Vec<(&str, u16)> = ports.iter().map(|(n, p, _)| (*n, *p)).collect();
    let (live, _) = fake_node::connect_sparks(&base, &list).await;
    let config = live.exit_config_dir();
    live.exit.pins.pin_new(&live, &config, now_ms()).await;
    let pins = identity_pin::load(&config);
    for n in ["cell1", "cell2"] {
        let p = &pins[n];
        assert!(p.cloned_machine_id && p.machine_id.is_none(), "{n}: {p:?}");
        assert!(p.lan_mac.is_some() && p.hostname.ends_with("-example"));
    }
    assert_eq!((pins["gx10-0000"].machine_id.as_deref(), pins["gx10-0000"].cloned_machine_id), (Some("9a9a0a6be0e04d6a9a3f0c2b5d6e7f81"), false));
    let names: Vec<(String, String)> = ["cell1", "cell2", "gx10-0000"].iter().map(|n| (n.to_string(), n.to_string())).collect();
    let view = live.exit.pins.view(&live, &config, &names).await;
    assert_eq!(view[0].note.as_deref(), Some("machine-id is shared by cell1 and cell2; pinned hostname and LAN MAC"));
    assert_eq!(view[2].note, None);
    assert!(view.iter().all(|v| v.matches == Some(true)), "{view:#?}");
    // The pins never land in inventory.json.
    assert!(config.join(identity_pin::FILE).exists() && !config.join("inventory.json").exists());
}

#[tokio::test]
async fn the_other_sparkboxs_containers_are_left_alone() {
    let (node, port) = fake_node(&temp("exit-owner-home"), docker()).await;
    let mut frame: serde_json::Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    frame["containers"] = json!([
        { "id": "0123456789ab", "name": "gemma-release", "image": "vllm", "state": "running", "startedAt": 1, "finishedAt": 0, "exitCode": 0,
          "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "vllm serve",
          "labels": { "sparkbox.instance": "release", "sparkbox.deployment": "d1" } },
        { "id": "fedcba987654", "name": "glm53", "image": "vllm", "state": "exited", "startedAt": 1, "finishedAt": 2, "exitCode": 0,
          "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "vllm serve", "labels": {} }
    ]);
    *node.knobs.frame.lock().unwrap() = Some(frame.to_string());
    let base = temp("exit-owner-app");
    let (live, inv) = fake_node::connect(&base, &[("cell1", port)]).await;
    for _ in 0..100 {
        if live.snapshot(&inv).nodes[0].containers.iter().any(|c| c.name == "gemma-release") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell1"], &[FenceClass::ContainerOwner, FenceClass::ModelUnload], "dev"));

    let e = err(live.container_action("cell1", "0123456789ab", "stop").await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "gemma-release on cell1 is managed by the other SparkBox (release)."));
    assert!(node.ran("docker stop").is_empty());
    // A container with no instance label (the owner's own) is the fence's call: allowed here.
    live.container_action("cell1", "fedcba987654", "stop").await.unwrap();
    assert_eq!(node.ran("docker stop").len(), 1);
}

#[test]
fn tests_get_a_permissive_exit_unless_they_opt_in() {
    // Every other fake_node write test runs on this default: no fence, lease or pin, look-only off.
    assert!(Exit::default().rules().is_none());
    assert!(!Exit::default().policy().look_only());
    // The app's rules: this build (dev in a debug build), the shared fence folder, its tag.
    let app = Exit::enforcing(Enforce::app());
    let rules = app.rules().unwrap();
    assert_eq!(rules.instance, "dev");
    assert!(rules.fence_dir.ends_with("Library/Application Support/SparkBox Fence"));
    assert!(!rules.tag.is_empty());
}

#[tokio::test]
async fn files_test_covers_only_the_test_folder_by_its_real_path() {
    let home = temp("exit-files-home");
    let (_n, port) = fake_node(&home, docker()).await;
    let base = temp("exit-files-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::FilesTest], "dev"));
    std::fs::create_dir_all(home.join("sparkbox-test/old")).unwrap();
    std::fs::create_dir_all(home.join("models/keep")).unwrap();
    // Files sends the node's real path; the exit asks the node for its $HOME to place it.
    let inside = crate::live::sh_quote(&format!("{}/sparkbox-test/old", home.display()));
    live.sparkjob("cell2", &format!("rm {inside}"), None, Duration::from_secs(20)).await.unwrap();
    assert!(!home.join("sparkbox-test/old").exists());
    let outside = crate::live::sh_quote(&format!("{}/models/keep", home.display()));
    let e = err(live.sparkjob("cell2", &format!("rm {outside}"), None, Duration::from_secs(20)).await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow this change.");
    assert!(home.join("models/keep").exists());
}

#[tokio::test]
async fn the_session_a_preset_admits_daemon_reload() {
    let (node, port) = fake_node(&temp("exit-reload-home"), docker()).await;
    let base = temp("exit-reload-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    // The "P1 Session A" preset: these classes, service only for earlyoom and cups.
    let mut s = spec(&["cell2"], &[FenceClass::SudoProbe, FenceClass::RootHelper, FenceClass::Service, FenceClass::Fleet, FenceClass::Shell, FenceClass::ContainerOwner], "release");
    s.only.insert(FenceClass::Service, vec!["earlyoom.service".into(), "cups.service".into()]);
    f.open(&s);
    // Acceptance step 6: restart earlyoom, disable and enable cups, daemon-reload.
    restart(&live, "cell2").await.unwrap();
    for cmd in ["systemctl disable -- 'cups.service'", "systemctl enable -- 'cups.service'", "systemctl daemon-reload"] {
        live.sudo("cell2", cmd, Duration::from_secs(30)).await.map_err(err).unwrap_or_else(|e| panic!("`{cmd}`: {}", e.message));
    }
    assert_eq!(node.ran("systemctl daemon-reload").len(), 1);
    let row = live.audit.entries().into_iter().find(|e| e.command == "systemctl daemon-reload").unwrap();
    assert_eq!(row.fence.as_deref(), Some("service"));
    // Any other unit is still outside it.
    let e = live.sudo("cell2", "systemctl restart -- 'sshd.service'", Duration::from_secs(30)).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow service changes to sshd.service: it names only earlyoom.service, cups.service.");
    assert!(node.ran("systemctl restart -- 'sshd.service'").is_empty() && node.ran("restart -- sshd.service").is_empty());
}

/// cell1's frame with a container the release build made and one of the owner's.
async fn two_owners(name: &str) -> (fake_node::Node, Arc<Live>, PathBuf) {
    with_containers(name, json!([])).await
}

/// `two_owners`, with `more` containers in the frame too.
async fn with_containers(name: &str, more: serde_json::Value) -> (fake_node::Node, Arc<Live>, PathBuf) {
    let (node, port) = fake_node(&temp(&format!("{name}-home")), docker()).await;
    let mut frame: serde_json::Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    let mut containers = json!([
        { "id": "0123456789ab", "name": "gemma-release", "image": "vllm", "state": "running", "startedAt": 1, "finishedAt": 0, "exitCode": 0,
          "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "vllm serve",
          "labels": { "sparkbox.instance": "release", "sparkbox.deployment": "d1" } },
        { "id": "fedcba987654", "name": "glm53", "image": "vllm", "state": "exited", "startedAt": 1, "finishedAt": 2, "exitCode": 0,
          "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "vllm serve", "labels": {} }
    ]);
    containers.as_array_mut().unwrap().extend(more.as_array().unwrap().iter().cloned());
    frame["containers"] = containers;
    *node.knobs.frame.lock().unwrap() = Some(frame.to_string());
    let base = temp(&format!("{name}-app"));
    let (live, inv) = fake_node::connect(&base, &[("cell1", port)]).await;
    for _ in 0..100 {
        if live.snapshot(&inv).nodes[0].containers.iter().any(|c| c.name == "gemma-release") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (node, live, base)
}

#[tokio::test]
async fn a_typed_fleet_command_cant_touch_the_other_builds_containers() {
    let (node, live, base) = two_owners("exit-fleet-owner").await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell1"], &[FenceClass::Fleet], "dev"));
    let typed = |cmd: &'static str| {
        let live = live.clone();
        async move { audit::as_typed(live.stream_cmd("cell1", cmd, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err) }
    };
    // `fleet` lets typed changes through, but not onto the release build's container: by name,
    // by id, in a longer command, or as `docker container …`.
    for cmd in ["docker rm gemma-release", "docker stop 0123456789ab", "uptime && docker kill -s KILL gemma-release", "docker container restart gemma-release"] {
        let e = typed(cmd).await.unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "gemma-release on cell1 is managed by the other SparkBox (release)."), "`{cmd}`");
    }
    assert!(node.ran("docker").is_empty(), "{:?}", node.ran("docker"));
    // The owner's own container is the fence's call: `fleet` allows it.
    typed("docker stop glm53").await.unwrap();
    assert_eq!(node.ran("docker stop glm53").len(), 1);
}

#[tokio::test]
async fn the_full_readiness_check_is_a_change_and_the_quick_one_a_read() {
    let home = temp("exit-check-home");
    let (node, port) = fake_node(&home, docker()).await;
    // sparkjob.py's own docker calls (image inspect, run) reach this one, which notes them.
    node.bin("docker", r#"#!/bin/sh
echo "$*" >> "$SB_FAKE_STATE/docker.log"
case "$1 $2" in
  "image inspect") echo '[{"Id":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]' ;;
  "run --rm") echo 0.11.0 ;;
esac
exit 0
"#);
    let docker_log = || std::fs::read_to_string(node.state_dir.join("docker.log")).unwrap_or_default();
    let base = temp("exit-check-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    // An older SparkBox's spelling: no instance, so its container would carry no instance label.
    let full = "check '~/models' 'vllm:img' 'vllm:img-ray'";
    let quick = "check '~/models' 'vllm:img' 'vllm:img-ray' quick";

    // Look-only: the quick check reads; the full one is refused before it reaches the node.
    let policy = Policy::new(true);
    live.set_write_policy(policy.clone());
    let v = live.sparkjob("cell2", quick, None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(v["vllmVersion"], "");
    let e = err(live.sparkjob("cell2", full, None, Duration::from_secs(30)).await.unwrap_err());
    assert_eq!(e.kind, ErrorKind::LookOnly);
    assert!(!docker_log().contains("run "), "{}", docker_log());

    // Not look-only, no fence: refused too, and so is Fleet's typed copy of it.
    policy.look_only.store(false, Ordering::SeqCst);
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    let e = err(live.sparkjob("cell2", full, None, Duration::from_secs(30)).await.unwrap_err());
    assert_eq!(e.kind, ErrorKind::Fenced);
    f.open(&spec(&["cell2"], &[FenceClass::ContainerTest, FenceClass::SudoProbe], "release"));
    let e = err(live.sparkjob("cell2", full, None, Duration::from_secs(30)).await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow this change.");
    assert!(!docker_log().contains("run "), "{}", docker_log());
    // An `any` fence admits it: the container runs, and its line is a change of class `other`.
    f.open(&spec(&["cell2"], &[FenceClass::Any], "release"));
    let v = live.sparkjob("cell2", full, None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(v["vllmVersion"], "0.11.0");
    assert!(docker_log().contains("--label sparkbox.test=1 --entrypoint python3 sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), "{}", docker_log());
    let row = live.audit.entries().into_iter().rev().find(|e| e.command.contains("sparkjob.py check") && !e.command.ends_with("quick") && e.title != "Refused").unwrap();
    assert_eq!((row.fence.as_deref(), row.changes), (Some("other"), true), "{row:?}");

    // With this build's instance named, it starts a labelled test container: `container-test`.
    let labelled = "check '~/models' 'vllm:img' 'vllm:img-ray' --instance 'release'";
    f.open(&spec(&["cell2"], &[FenceClass::SudoProbe], "release"));
    let e = err(live.sparkjob("cell2", labelled, None, Duration::from_secs(30)).await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow test containers.");
    // A full check starts the image for vLLM's version, then again to see the GPU.
    let version_runs = || docker_log().lines().filter(|l| l.starts_with("run ") && l.contains("--entrypoint python3")).map(String::from).collect::<Vec<_>>();
    let runs = || version_runs().len();
    assert_eq!(runs(), 1, "{}", docker_log());
    f.open(&spec(&["cell2"], &[FenceClass::ContainerTest], "release"));
    let v = live.sparkjob("cell2", labelled, None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(v["vllmVersion"], "0.11.0");
    let run = version_runs().last().unwrap().clone();
    let name = run.split_whitespace().nth(3).unwrap().to_string();
    assert!(name.starts_with("sparkbox-checkimage-") && name.len() == "sparkbox-checkimage-".len() + 8, "{run}");
    assert_eq!(run, format!("run --rm --name {name} --label sparkbox.test=1 --label sparkbox.instance=release --entrypoint python3 sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa -c import vllm; print(vllm.__version__)"));
    let row = live.audit.entries().into_iter().find(|e| e.command.contains("sparkjob.py check") && e.command.ends_with("--instance 'release'") && e.title != "Refused").unwrap();
    assert_eq!((row.fence.as_deref(), row.changes), (Some("container-test"), true), "{row:?}");
    // The quick check with the instance named still only reads, look-only included.
    policy.look_only.store(true, Ordering::SeqCst);
    live.sparkjob("cell2", "check '~/models' 'vllm:img' 'vllm:img-ray' quick --instance 'release'", None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(err(live.sparkjob("cell2", labelled, None, Duration::from_secs(30)).await.unwrap_err()).kind, ErrorKind::LookOnly);
    assert_eq!(runs(), 2);
}

#[tokio::test]
async fn files_test_refuses_a_link_out_of_the_test_folder() {
    let home = temp("exit-link-home");
    let (_n, port) = fake_node(&home, docker()).await;
    let base = temp("exit-link-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::FilesTest], "dev"));
    std::fs::create_dir_all(home.join("models/glm53")).unwrap();
    std::fs::write(home.join("models/glm53/config.json"), "{}").unwrap();
    std::fs::create_dir_all(home.join("sparkbox-test")).unwrap();
    std::os::unix::fs::symlink(&home, home.join("sparkbox-test/link")).unwrap();
    // The path is under ~/sparkbox-test as written, so the fence admits it as files-test…
    let through = crate::live::sh_quote(&format!("{}/sparkbox-test/link/models/glm53", home.display()));
    for verb in ["rm", "trash"] {
        let e = err(live.sparkjob("cell2", &format!("{verb} {through}"), None, Duration::from_secs(20)).await.unwrap_err());
        // …and sparkjob.py, which follows the link, refuses it there.
        assert!(e.message.contains("leads outside ~/sparkbox-test"), "{verb}: {}", e.message);
    }
    assert!(home.join("models/glm53/config.json").exists(), "the model is untouched");
    let row = live.audit.entries().into_iter().find(|e| e.command.contains("sparkjob.py rm")).unwrap();
    assert_eq!(row.fence.as_deref(), Some("files-test"));
    // The link itself is inside: removing it is fine, and leaves what it pointed at.
    let link = crate::live::sh_quote(&format!("{}/sparkbox-test/link", home.display()));
    live.sparkjob("cell2", &format!("rm {link}"), None, Duration::from_secs(20)).await.unwrap();
    assert!(!home.join("sparkbox-test/link").exists() && home.join("models/glm53/config.json").exists());
}

#[tokio::test]
async fn a_wake_is_fenced_like_a_change() {
    let (peer, port) = fake_node(&temp("exit-wake-peer"), fake_node::real_shell()).await;
    let base = temp("exit-wake-app");
    let (live, inv) = fake_node::connect(&base, &[("cell1", port)]).await;
    for _ in 0..100 {
        if live.snapshot(&inv).nodes.iter().any(|n| n.status == LinkStatus::Online) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let snap = live.snapshot(&inv);
    let book: crate::wol::MacBook = [("cell3".to_string(), ("enP7s7".to_string(), "4c:bb:47:0a:1b:2c".to_string(), Some("127.0.0.3".to_string())))].into();
    let listener = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    listener.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let udp = listener.local_addr().unwrap().port();
    let extra = ["127.0.0.1".to_string()];
    let wake = || crate::wol::send_with(&live, &snap, &book, "cell3", udp, &extra);

    // Look-only: refused before any packet leaves, with one "Refused" line.
    let policy = Policy::new(true);
    live.set_write_policy(policy.clone());
    let e = err(wake().await.unwrap_err());
    assert_eq!((e.kind, e.node.as_deref()), (ErrorKind::LookOnly, Some("cell3")));
    assert_eq!(refusals(&live).iter().filter(|r| r.command == "Wake-on-LAN for cell3").count(), 1);

    // No fence, or a fence for another node: refused, and the peer ran nothing.
    policy.look_only.store(false, Ordering::SeqCst);
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    assert_eq!(err(wake().await.unwrap_err()).kind, ErrorKind::Fenced);
    f.open(&spec(&["cell2"], &[FenceClass::PowerOne], "dev"));
    assert_eq!(err(wake().await.unwrap_err()).message, "cell3 isn't in the open fence.");
    f.open(&spec(&["cell3"], &[FenceClass::Service], "dev"));
    assert_eq!(err(wake().await.unwrap_err()).message, "The fence doesn't allow restarts and shutdowns.");
    assert!(peer.ran("python3 -I -c ").is_empty());

    // `power-one` for cell3: the wake goes, relayed by cell1, which isn't in the fence (the
    // packet changes nothing on it).
    f.open(&spec(&["cell3"], &[FenceClass::PowerOne], "dev"));
    let route = wake().await.unwrap();
    assert_eq!(route, crate::wol::WakeRoute::Peer { node: "cell1".into(), name: "cell1".into() });
    let mut buf = [0u8; 200];
    let (n, _) = listener.recv_from(&mut buf).unwrap();
    assert_eq!(&buf[..n], crate::wol::magic_packet("4c:bb:47:0a:1b:2c").unwrap().as_slice());
    assert_eq!(peer.ran("python3 -I -c ").len(), 1);
}

#[tokio::test]
async fn a_closed_fence_put_back_is_refused_at_the_exit() {
    let (_n, port) = fake_node(&temp("exit-closed-home"), docker()).await;
    let base = temp("exit-closed-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    restart(&live, "cell2").await.unwrap();
    let path = f.rules.fence_dir.join(fence::RECORD);
    let saved = std::fs::read(&path).unwrap();
    // Closed the way fence_close closes it, then the saved file put back by hand.
    fence::close(&f.rules.fence_dir, &f.config(), now_ms()).unwrap();
    assert!(restart(&live, "cell2").await.unwrap_err().message.contains("No fence is open"));
    std::fs::write(&path, &saved).unwrap();
    assert_eq!(restart(&live, "cell2").await.unwrap_err().message, "The fence record failed its check: open a new one.");
    assert_eq!(live.exit.fence_view(&live).state, fence::State::Failed);
    // A wider, older fence put back over a newer, narrower one: refused the same way.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let wide = f.open(&spec(&["cell2", "cell3"], &[FenceClass::Service, FenceClass::PowerOne], "release"));
    let wide_bytes = std::fs::read(&path).unwrap();
    restart(&live, "cell2").await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    restart(&live, "cell2").await.unwrap();
    std::fs::write(&path, &wide_bytes).unwrap();
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!(e.message, "The fence record failed its check: open a new one.");
    assert_eq!(live.exit.fence_view(&live).record.map(|r| r.id), Some(wide.id));
    // A new fence opens.
    tokio::time::sleep(Duration::from_millis(5)).await;
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    restart(&live, "cell2").await.unwrap();
}

#[test]
fn the_release_signing_path_makes_keeps_and_repins_its_key() {
    use std::cell::RefCell;
    let base = temp("exit-sign");
    let f = TestFence::new(&base, "release");
    let config = base.join("config-signing");
    let vault: RefCell<Option<String>> = RefCell::new(None);
    let sets = RefCell::new(0);
    let get = || Ok(vault.borrow().clone());
    let set = |t: &str| {
        *sets.borrow_mut() += 1;
        *vault.borrow_mut() = Some(t.to_string());
        Ok(())
    };
    let s = spec(&["cell2"], &[FenceClass::Service], "release");
    // First open: a key is made and kept, this build trusts it, and the record checks out.
    let first = open_with(&f.rules, &config, &s, now_ms(), "Studio", get, set).unwrap();
    assert_eq!(*sets.borrow(), 1);
    let trusted = fence::load_trust(&config).unwrap();
    assert_eq!(trusted.key_id, first.key_id);
    assert_eq!(fence::load_key_file(&f.rules.fence_dir).unwrap().key_id, first.key_id);
    assert_eq!(fence::Verifier::default().check(&f.rules.fence_dir, &config, now_ms()).state, fence::State::Valid);
    // Second open: the same key, nothing new kept.
    std::thread::sleep(Duration::from_millis(5));
    let second = open_with(&f.rules, &config, &s, now_ms(), "Studio", get, set).unwrap();
    assert_eq!((*sets.borrow(), second.key_id.as_str()), (1, first.key_id.as_str()));
    // The key in the Keychain changed: trust moves to it, and its fence checks out.
    *vault.borrow_mut() = Some(fence::key_to_text(&fence::new_key().unwrap()));
    std::thread::sleep(Duration::from_millis(5));
    let third = open_with(&f.rules, &config, &s, now_ms(), "Studio", get, set).unwrap();
    assert_ne!(third.key_id, first.key_id);
    assert_eq!(fence::load_trust(&config).unwrap().key_id, third.key_id);
    assert_eq!(fence::Verifier::default().check(&f.rules.fence_dir, &config, now_ms()).record.map(|r| r.id), Some(third.id));
    // A Keychain that can't be read, or holds garbage: refused, nothing written.
    let before = std::fs::read(f.rules.fence_dir.join(fence::RECORD)).unwrap();
    assert!(open_with(&f.rules, &config, &s, now_ms(), "Studio", || Err("denied".into()), set).is_err());
    assert!(open_with(&f.rules, &config, &s, now_ms(), "Studio", || Ok(Some("not a key".into())), set).is_err());
    assert_eq!(std::fs::read(f.rules.fence_dir.join(fence::RECORD)).unwrap(), before);
}

#[test]
fn scripted_unlocks_never_change_what_sparkbox_trusts() {
    // SPARKBOX_ADMIN_NOAUTH: fence_trust_key, node_identity_repin and a changed host key refuse.
    let e = trust_needs_touch_id(true).unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Invalid, "Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts."));
    trust_needs_touch_id(false).unwrap();
    // While the dev build shares the release build's settings, its trust is the release pin.
    assert!(shared_config_refusal(false).unwrap_err().message.contains("already trusts the key the release build pinned"));
    shared_config_refusal(true).unwrap();
    // With its own settings, it pins the key next to the record.
    let base = temp("exit-trust");
    let f = TestFence::new(&base, "release");
    assert!(trust_key_file(&f.rules, &base.join("dev")).unwrap_err().message.contains("no fence key yet"));
    crate::util::save_json(&f.rules.fence_dir.join(fence::KEY_FILE), &f.file).unwrap();
    assert_eq!(trust_key_file(&f.rules, &base.join("dev")).unwrap().key_id, f.file.key_id);
    assert_eq!(fence::load_trust(&base.join("dev")).unwrap().key_id, f.file.key_id);
    // The env var is unset in tests.
    assert!(!scripted_unlock());
}

// ---- R2-SAFE: round 1's spill

#[tokio::test]
async fn a_shell_or_command_in_the_other_builds_container_is_refused_at_the_exit() {
    let test = json!([{ "id": "abcdefabcdef", "name": "sparkbox-test-busybox", "image": "busybox:1.36", "state": "running", "startedAt": 1, "finishedAt": 0,
        "exitCode": 0, "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "sleep 3600",
        "labels": { "sparkbox.test": "1", "sparkbox.instance": "dev" } }]);
    let (node, live, base) = with_containers("exit-exec-owner", test).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell1"], &[FenceClass::Shell, FenceClass::Fleet, FenceClass::ContainerTest], "dev"));
    let shell_in = |c: &str| crate::term::command_for(&crate::term::Target::ContainerShell { container: c.into() }).unwrap().unwrap();
    let other = "gemma-release on cell1 is managed by the other SparkBox (release).";
    // A terminal's shell in the release build's container, by name or id, and a command streamed into it.
    for cmd in [shell_in("gemma-release"), shell_in("0123456789ab")] {
        let e = err(live.pty("cell1", 80, 24, Some(&cmd)).await.err().unwrap());
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, other), "`{cmd}`");
    }
    let e = err(live.exec_io("cell1", "docker exec -i -u 0 gemma-release sh").await.err().unwrap());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, other));
    // Typed in Fleet, under a `fleet` fence: refused the same way.
    for cmd in ["docker exec gemma-release id", "docker container exec -it gemma-release sh", "uptime; docker exec -e A=1 0123456789ab true"] {
        let typed = audit::as_typed(live.stream_cmd("cell1", cmd, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await;
        let e = err(typed.unwrap_err());
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, other), "`{cmd}`");
    }
    assert!(node.ran("docker exec").is_empty(), "{:?}", node.ran(""));
    // This build's own test container: a shell in it opens.
    let pty = live.pty("cell1", 80, 24, Some(&shell_in("sparkbox-test-busybox"))).await;
    assert!(pty.is_ok(), "{:?}", pty.err());
    for _ in 0..100 {
        if !node.ran("docker exec -it -e TERM=xterm-256color sparkbox-test-busybox").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(node.ran("docker exec").len(), 1, "{:?}", node.ran(""));
    drop(pty);
    let row = live.audit.entries().into_iter().filter(|e| e.title == "Refused").count();
    assert_eq!(row, 6, "one Refused line each");
}

/// A lease file this build holds on the node, claimed by a change under an open fence.
async fn holding_a_lease(name: &str) -> (fake_node::Node, PathBuf, Arc<Live>, TestFence) {
    let home = temp(&format!("{name}-home"));
    let (node, port) = fake_node(&home, docker()).await;
    let base = temp(&format!("{name}-app"));
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    (node, home, live, f)
}

#[tokio::test]
async fn look_only_and_relocks_let_go_of_the_lease_without_writing_on_the_node() {
    let (node, home, live, f) = holding_a_lease("exit-relock").await;
    let (gate, _, _) = crate::admin::testing::gate();
    live.set_write_policy(gate.clone());
    gate.unlock(None).unwrap();
    restart(&live, "cell2").await.unwrap();
    assert_eq!(live.exit.leases.held(), ["cell2"]);
    let lease = home.join(".sparkbox/controller.json");
    assert!(lease.exists());
    // Look-only turning on relocks (an explicit relock): the hook `start` installs lets go of
    // the lease here only, and the node's own 45 s TTL frees it.
    let (tx, rx) = std::sync::mpsc::channel();
    gate.on_relock(move |why, _| {
        let _ = tx.send(why);
    });
    gate.set_look_only(true).unwrap();
    let why = rx.try_recv().unwrap();
    assert_eq!(why, crate::admin::RelockReason::Explicit);
    release_on_relock(&live, why).await;
    assert!(node.ran("lease release").is_empty(), "{:?}", node.ran("lease"));
    assert!(live.exit.leases.held().is_empty(), "the claim is gone here");
    assert!(lease.exists(), "nothing was written on the node");
    // Closing the fence while look-only is on writes nothing either.
    close_fence(&live).await.unwrap();
    assert!(node.ran("lease release").is_empty());

    // Not look-only: a clean fence close gives the lease back on the node.
    gate.set_look_only(false).unwrap();
    gate.unlock(None).unwrap();
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    live.exit.leases.forget();
    restart(&live, "cell2").await.unwrap();
    assert_eq!(live.exit.leases.held(), ["cell2"]);
    close_fence(&live).await.unwrap();
    assert_eq!(node.ran("lease release").len(), 1);
    assert!(!lease.exists() && live.exit.leases.held().is_empty());
}

#[test]
fn only_a_quit_while_not_look_only_sends_the_lease_back() {
    use crate::admin::RelockReason::*;
    for why in [Idle, Explicit, ScreenLock, Sleep, WindowClosed, LiveData] {
        assert!(!relock_sends(why, false) && !relock_sends(why, true), "{why:?}");
    }
    assert!(relock_sends(Quit, false));
    assert!(!relock_sends(Quit, true));
}

#[tokio::test]
async fn a_relock_with_no_lease_held_sends_nothing() {
    let (node, _home, live, _f) = holding_a_lease("exit-relock-idle").await;
    for why in [crate::admin::RelockReason::ScreenLock, crate::admin::RelockReason::Sleep, crate::admin::RelockReason::Idle] {
        restart(&live, "cell2").await.unwrap();
        release_on_relock(&live, why).await;
        assert!(live.exit.leases.held().is_empty(), "{why:?}");
    }
    assert!(node.ran("lease release").is_empty());
    // A quit (not look-only) sends it.
    restart(&live, "cell2").await.unwrap();
    release_on_relock(&live, crate::admin::RelockReason::Quit).await;
    assert_eq!(node.ran("lease release").len(), 1);
}

#[tokio::test]
async fn a_lease_claim_that_meets_a_stale_copy_puts_it_back_and_asks_again() {
    let (node, home, live, _f) = holding_a_lease("exit-stale").await;
    // This connection has checked its copy of sparkjob.py once (a read)…
    live.sparkjob("cell2", "df /", None, Duration::from_secs(20)).await.unwrap();
    let installs = || node.ran("sparkjob.py.new").len();
    let before = installs();
    // …then another SparkBox put its own copy there.
    std::fs::write(home.join(".sparkbox/sparkjob.py"), "# the other SparkBox's copy\n").unwrap();
    restart(&live, "cell2").await.expect("the claim is asked again once the copy is back");
    assert_eq!(installs(), before + 1, "one reinstall");
    assert_eq!(node.ran("lease claim").len(), 2, "the claim that met the stale copy, and its retry");
    assert_eq!(std::fs::read_to_string(home.join(".sparkbox/sparkjob.py")).unwrap(), crate::live::SPARKJOB);
    let held: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(".sparkbox/controller.json")).unwrap()).unwrap();
    assert_eq!(held["instance"], "release");
    assert_eq!(node.ran("systemctl restart").len(), 1);
}

#[test]
fn the_touch_id_sheet_names_the_fence_it_opens() {
    use FenceClass::*;
    let mut t2 = spec(&["cell2"], &[ContainerTest, ProcessTest, FilesTest], "dev");
    t2.minutes = 480;
    assert_eq!(open_reason(&t2), "open a fence: cell2 · 3 classes (container-test, process-test, files-test) · 8 h · for the dev build");
    let a = spec(&["cell1", "cell2", "cell3", "gx10-0000"], &[SudoProbe, RootHelper, Service, Fleet, Shell, ContainerOwner], "release");
    let words = open_reason(&a);
    for want in ["cell1, cell2, cell3, gx10-0000", "6 classes", "sudo-probe, root-helper, service, fleet, shell, container-owner", "· 3 h ·", "release build"] {
        assert!(words.contains(want), "{want}: {words}");
    }
    let mut one = spec(&["gx10-0000"], &[PowerOne], "release");
    one.minutes = 150;
    assert_eq!(open_reason(&one), "open a fence: gx10-0000 · 1 class (power-one) · 2 h 30 min · for the release build");

    // The gate asks macOS with these words, and the confirmation covers one fence_open.
    let (g, _, auth) = crate::admin::testing::gate();
    g.confirm_with_words("fence_open", &open_reason(&t2)).unwrap();
    assert_eq!(auth.reasons.lock().unwrap().as_slice(), [open_reason(&t2)]);
    g.check_confirmed("fence_open").unwrap();
    assert!(g.check_confirmed("fence_open").is_err(), "it covers one call");
    // Refused, nothing asked: a cancelled sheet, and look-only.
    auth.refuse.store(true, Ordering::SeqCst);
    assert!(g.confirm_with_words("fence_open", &open_reason(&t2)).is_err());
    assert!(g.check_confirmed("fence_open").is_err());
    auth.refuse.store(false, Ordering::SeqCst);
    g.set_look_only(true).unwrap();
    let asked = auth.asked.load(Ordering::SeqCst);
    assert_eq!(g.confirm_with_words("fence_open", &open_reason(&t2)).unwrap_err(), crate::admin::LOOK_ONLY);
    assert_eq!(auth.asked.load(Ordering::SeqCst), asked);
}

#[tokio::test]
async fn a_pid_guarded_signal_is_process_test_where_it_is_tagged() {
    let (_n, port) = fake_node(&temp("exit-pid-home"), docker()).await;
    let base = temp("exit-pid-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    // A real process, and a /proc stat line for it as the kernel writes one: the name (field 2)
    // has spaces and `)` in it, so the start time (field 22) is counted after the last `)`.
    let proc_root = temp("exit-pid-proc");
    let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    std::fs::create_dir_all(proc_root.join(pid.to_string())).unwrap();
    let stat = format!("{pid} (a) b) 9 S) S 1 {pid} {pid} 0 -1 4194304 101 0 0 0 0 0 0 0 20 0 1 0 987654 5926912 120 18446744073709551615");
    std::fs::write(proc_root.join(format!("{pid}/stat")), stat).unwrap();
    let guard = |start: u64| crate::ops::pid_guard_cmd(pid, "TERM", Some(start), proc_root.to_str().unwrap());
    // Its shape is several commands, so on its own it's `other`.
    assert!(matches!(class_of(ExecKind::Run, &guard(987654), &Ctx::default()), Shape::Change(FenceClass::Other, _)));
    let run = |cmd: String| {
        let live = live.clone();
        async move { live.run("cell2", &cmd, None, Duration::from_secs(10)).await.map_err(err) }
    };
    // No fence: refused, tagged or not.
    assert_eq!(as_class(FenceClass::ProcessTest, run(guard(987654))).await.unwrap_err().kind, ErrorKind::Fenced);
    // A fence without process-test refuses it as process signals.
    f.open(&spec(&["cell2"], &[FenceClass::FilesTest], "dev"));
    assert_eq!(as_class(FenceClass::ProcessTest, run(guard(987654))).await.unwrap_err().message, "The fence doesn't allow process signals.");
    // A process-test fence: untagged it's still `other`; tagged where it's sent, it's admitted.
    f.open(&spec(&["cell2"], &[FenceClass::ProcessTest], "dev"));
    assert_eq!(run(guard(987654)).await.unwrap_err().message, "The fence doesn't allow this change.");
    assert!(child.try_wait().unwrap().is_none());
    // A different start time (the PID went to another program): exit 3, nothing signalled.
    let out = as_class(FenceClass::ProcessTest, run(guard(111))).await.unwrap();
    assert_eq!((out.code, out.stderr.trim()), (Some(3), "gone"));
    assert!(child.try_wait().unwrap().is_none());
    let out = as_class(FenceClass::ProcessTest, run(guard(987654))).await.unwrap();
    assert_eq!(out.code, Some(0), "{out:?}");
    assert!(!child.wait().unwrap().success(), "the same process got SIGTERM");
    let row = live.audit.entries().into_iter().rev().find(|e| e.command == guard(987654) && e.title != "Refused").unwrap();
    assert_eq!(row.fence.as_deref(), Some("process-test"));
}

#[tokio::test]
async fn files_test_keeps_trash_and_undo_inside_the_test_folder() {
    let home = temp("exit-untrash-home");
    let (_n, port) = fake_node(&home, docker()).await;
    let base = temp("exit-untrash-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::FilesTest], "dev"));
    let q = |p: &Path| crate::live::sh_quote(&p.display().to_string());
    std::fs::create_dir_all(home.join("sparkbox-test")).unwrap();
    std::fs::write(home.join("sparkbox-test/a.txt"), "one").unwrap();
    // Trash and Undo in the folder: the item goes to the folder's own Trash and comes back.
    let v = live.sparkjob("cell2", &format!("trash {}", q(&home.join("sparkbox-test/a.txt"))), None, Duration::from_secs(20)).await.unwrap();
    let trashed = PathBuf::from(v[0]["trashed"].as_str().unwrap());
    assert_eq!(trashed, home.join("sparkbox-test/.sparkbox-trash/files/a.txt"));
    assert!(!home.join(".local/share/Trash").exists(), "nothing outside the folder");
    live.sparkjob("cell2", &format!("untrash {}", q(&trashed)), None, Duration::from_secs(20)).await.unwrap();
    assert_eq!(std::fs::read_to_string(home.join("sparkbox-test/a.txt")).unwrap(), "one");
    // An item in the folder's Trash whose trashinfo says it came from ~/models: refused on the node.
    let info = home.join("sparkbox-test/.sparkbox-trash/info");
    std::fs::write(home.join("sparkbox-test/.sparkbox-trash/files/x"), "x").unwrap();
    std::fs::write(info.join("x.trashinfo"), format!("[Trash Info]\nPath={}/models/x\nDeletionDate=2026-10-30T10:00:00\n", home.display())).unwrap();
    let e = err(live.sparkjob("cell2", &format!("untrash {}", q(&home.join("sparkbox-test/.sparkbox-trash/files/x"))), None, Duration::from_secs(20)).await.unwrap_err());
    assert!(e.message.contains("outside ~/sparkbox-test"), "{}", e.message);
    assert!(!home.join("models/x").exists() && home.join("sparkbox-test/.sparkbox-trash/files/x").exists());
    // The home Trash puts things back wherever they came from: not files-test.
    std::fs::create_dir_all(home.join(".local/share/Trash/files")).unwrap();
    std::fs::create_dir_all(home.join(".local/share/Trash/info")).unwrap();
    std::fs::write(home.join(".local/share/Trash/files/y"), "y").unwrap();
    std::fs::write(home.join(".local/share/Trash/info/y.trashinfo"), format!("[Trash Info]\nPath={}/models/y\n", home.display())).unwrap();
    let e = err(live.sparkjob("cell2", &format!("untrash {}", q(&home.join(".local/share/Trash/files/y"))), None, Duration::from_secs(20)).await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."));
    assert!(!home.join("models/y").exists());
}

/// Another machine with cell2's hostname, LAN MAC and host key but its own machine-id, on cell2's
/// address: the machine-id read on the old connection isn't taken for the new one. The cache is
/// kept against the connection's serial, which no later connection has, never its address in
/// memory, which a new connection can reuse once the old one is gone (only on some runs, so the
/// serial is what this checks every time).
#[tokio::test]
async fn a_new_connection_reads_the_machine_id_again() {
    let (ha, hb) = (temp("exit-mid-again-a"), temp("exit-mid-again-b"));
    let (_na, pa) = fake_spark(&ha, "cell2").await;
    let (nb, pb) = fake_spark(&hb, "cell2").await;
    for (h, id) in [(&ha, "1111aaaa2222bbbb3333cccc4444dddd"), (&hb, "9999eeee8888ffff7777aaaa6666bbbb")] {
        std::fs::write(PathBuf::from(format!("{}.spark/root/etc/machine-id", h.display())), format!("{id}\n")).unwrap();
    }
    let base = temp("exit-mid-again-app");
    let (live, _) = fake_node::connect_sparks(&base, &[("cell2", pa)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    live.exit.pins.pin_new(&live, &f.config(), now_ms()).await;
    let pin = identity_pin::load(&f.config()).remove("cell2").unwrap();
    assert_eq!((pin.hostname.as_str(), pin.machine_id.as_deref()), ("cell2-example", Some("1111aaaa2222bbbb3333cccc4444dddd")));
    restart(&live, "cell2").await.unwrap();
    let first = live.session("cell2").unwrap().serial();
    assert_eq!(live.exit.pins.machine_id_read_on("cell2"), Some(first));

    // The look-alike answers on cell2's address: a new connection, so its machine-id is read again.
    crate::live::trust_host_key(&live.data_path("known_hosts.json"), &format!("127.0.0.1:{pb}"), &Profile::spark("cell2").fingerprint()).unwrap();
    live.start(&[cell2_on(pb)]);
    for _ in 0..200 {
        if !nb.ran("").is_empty() && live.is_connected("cell2") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_hostname(&live, "cell2-example").await;
    let second = live.session("cell2").unwrap().serial();
    assert_ne!(second, first, "a new connection has a new serial");
    let e = restart(&live, "cell2").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::WrongNode, "cell2 answers as cell2-example: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence."));
    assert!(nb.ran("systemctl restart").is_empty(), "nothing reached the look-alike");
    // Read again on the new connection, and kept against it.
    assert!(!nb.ran("cat /etc/machine-id").is_empty(), "{:?}", nb.ran(""));
    assert_eq!(live.exit.pins.machine_id_read_on("cell2"), Some(second));
}

// ---- R2-SAFE round 2: the verifier's defects

/// A typed command whose power or docker verb hides behind xargs, or whose shell sources its
/// rcfile from stdin, is a change (not a read): with no fence, and in look-only, it's refused
/// before anything reaches the node.
#[tokio::test]
async fn typed_xargs_fed_and_rcfile_shells_are_refused_and_run_nothing() {
    let (node, live, base) = two_owners("exit-xargs-rcfile").await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    let typed = |cmd: &'static str| {
        let live = live.clone();
        async move { audit::as_typed(live.stream_cmd("cell1", cmd, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err) }
    };
    // No fence open: each is a change, so it's Fenced.
    for cmd in [
        "echo rm -f gemma-release | xargs docker",
        "echo reboot | xargs systemctl",
        "echo 'docker rm -f gemma-release' | bash --rcfile /dev/stdin -i -c true",
    ] {
        let e = typed(cmd).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Fenced, "`{cmd}`: {}", e.message);
    }
    // Look-only: refused before the node.
    live.set_write_policy(Policy::new(true));
    for cmd in [
        "echo run --rm --privileged -v /:/host alpine chroot /host reboot | xargs docker",
        "echo 'docker rm -f gemma-release' | bash --rcfile /dev/stdin -i -c true",
    ] {
        let e = typed(cmd).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::LookOnly, "`{cmd}`: {}", e.message);
    }
    assert!(node.ran("rm -f gemma-release").is_empty() && node.ran("reboot").is_empty() && node.ran("xargs").is_empty(), "{:?}", node.ran(""));
}

/// Check (d) catches a stop/kill/rm/exec/management verb on the other build's container however
/// it's wrapped, named by a short id prefix, or spelled `/name`.
#[tokio::test]
async fn check_d_catches_wrapped_and_short_id_forms() {
    let (node, live, base) = two_owners("exit-checkd-forms").await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell1"], &[FenceClass::Fleet, FenceClass::Shell, FenceClass::ContainerOwner], "dev"));
    let typed = |cmd: String| {
        let live = live.clone();
        async move { audit::as_typed(live.stream_cmd("cell1", &cmd, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err) }
    };
    let other = "gemma-release on cell1 is managed by the other SparkBox (release).";
    for cmd in [
        "/usr/bin/docker stop gemma-release",
        "env docker stop gemma-release",
        "env -S 'docker stop gemma-release'",
        "env -iS 'docker stop gemma-release'",
        "sudo env -iS 'timeout 5 docker stop gemma-release'",
        "timeout 5 docker stop gemma-release",
        "timeout --signal=TERM 5 docker stop gemma-release",
        "timeout -s TERM 5 docker stop gemma-release",
        "nice docker kill gemma-release",
        "exec docker rm -f gemma-release",
        "sudo -u root docker stop gemma-release",
        "bash -lc 'docker stop gemma-release'",
        "docker update --restart=no gemma-release",
        "docker rename gemma-release x",
        "docker network disconnect bridge gemma-release",
        "docker stop 0123",
        "docker stop /gemma-release",
        "docker container prune -f",
    ] {
        let e = typed(cmd.to_string()).await.unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, other), "`{cmd}`");
    }
    assert!(node.ran("docker stop").is_empty() && node.ran("docker kill").is_empty() && node.ran("docker rm").is_empty(), "{:?}", node.ran("docker"));
}

/// A call-site tag can't relabel a root-running command to a non-sudo class, nor inject an item
/// a sudo class's `only` list would need: the command's own shape decides there.
#[tokio::test]
async fn a_tag_cant_relabel_a_root_command_past_its_shape() {
    let (node, port) = fake_node(&temp("exit-tag-home"), docker()).await;
    let base = temp("exit-tag-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    // A process-test tag on a sudo kill stays `other` (process-test isn't a sudo class).
    f.open(&spec(&["cell2"], &[FenceClass::ProcessTest], "dev"));
    let e = as_class(FenceClass::ProcessTest, live.sudo("cell2", "kill -s TERM 9", Duration::from_secs(10))).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow this change.");
    // A service tag on `systemctl mask` (shape `other`) leaves it `other` (P1 r4): a service
    // fence, with an `only` list or without, refuses it rather than masking whatever it names.
    let mut s = spec(&["cell2"], &[FenceClass::Service], "dev");
    s.only.insert(FenceClass::Service, vec!["earlyoom.service".into()]);
    f.open(&s);
    let e = as_class(FenceClass::Service, live.sudo("cell2", "systemctl mask -- ssh.service", Duration::from_secs(10))).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow this change.");
    f.open(&spec(&["cell2"], &[FenceClass::Service], "dev"));
    let e = as_class(FenceClass::Service, live.sudo("cell2", "systemctl mask -- ssh.service", Duration::from_secs(10))).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow this change.");
    assert!(node.ran("systemctl mask").is_empty() && node.ran("kill").is_empty());
}

/// A typed Fleet power verb needs `power-one`, not just `fleet`: under a fleet fence with no
/// power-one (the Session A preset's shape), a typed reboot or shutdown is refused.
#[tokio::test]
async fn a_typed_fleet_power_verb_needs_power_one() {
    let (node, port) = fake_node(&temp("exit-typedpower-home"), docker()).await;
    let base = temp("exit-typedpower-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    // fleet, service and shell, as the Session A preset has them — but no power-one.
    f.open(&spec(&["cell2"], &[FenceClass::Fleet, FenceClass::Service, FenceClass::Shell], "release"));
    let typed = |cmd: &'static str| {
        let live = live.clone();
        async move { audit::as_typed(live.stream_cmd("cell2", cmd, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err) }
    };
    for cmd in ["sudo reboot", "sudo shutdown -h now", "sudo chroot / reboot"] {
        let e = typed(cmd).await.unwrap_err();
        assert_eq!(e.message, "The fence doesn't allow restarts and shutdowns.", "`{cmd}`");
    }
    // A non-power typed change still passes with just `fleet`, and reaches the node.
    audit::as_typed(live.stream_cmd("cell2", "systemctl restart -- 'earlyoom.service'", Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.unwrap();
    assert!(node.ran("reboot").is_empty() && node.ran("shutdown").is_empty(), "no power verb reached the node: {:?}", node.ran(""));
}

/// Closing a fence in look-only writes nothing on the node, even when Admin was already locked
/// (so no relock hook ran to drop the local lease first): `close_fence`'s `send = !look_only`
/// guard is the one that stops a `lease release`.
#[tokio::test]
async fn closing_a_fence_in_look_only_with_admin_locked_writes_nothing() {
    let (node, home, live, _f) = holding_a_lease("exit-locked-close").await;
    let (gate, _, _) = crate::admin::testing::gate();
    live.set_write_policy(gate.clone());
    assert!(!gate.state().unlocked, "Admin starts locked");
    // A fenced change claims the lease (the fence is open and look-only is off; Admin need not be
    // unlocked for the exit to admit a fenced change).
    restart(&live, "cell2").await.unwrap();
    assert_eq!(live.exit.leases.held(), ["cell2"]);
    assert!(home.join(".sparkbox/controller.json").exists());
    // Turning look-only on while Admin was locked runs no relock hook, so the local claim stays.
    let hooks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let h = hooks.clone();
    gate.on_relock(move |_, _| {
        h.fetch_add(1, Ordering::SeqCst);
    });
    gate.set_look_only(true).unwrap();
    assert_eq!(hooks.load(Ordering::SeqCst), 0, "no relock hook on look-only when Admin was locked");
    assert_eq!(live.exit.leases.held(), ["cell2"], "the local claim is still held");
    // Closing the fence in look-only must send no `lease release`, and leave controller.json for
    // the node's own 45 s TTL to free.
    close_fence(&live).await.unwrap();
    assert!(node.ran("lease release").is_empty(), "{:?}", node.ran("lease"));
    assert!(home.join(".sparkbox/controller.json").exists(), "nothing was written on the node");
}

/// A confirmation (`fence_confirm`) is tied to the exact spec, so it can't open a different one,
/// and the generic `admin_authenticate("fence_open")` doesn't satisfy `fence_open` either.
#[test]
fn a_fence_confirmation_is_bound_to_its_spec() {
    let (g, _, _) = crate::admin::testing::gate();
    let narrow = spec(&["cell2"], &[FenceClass::FilesTest], "release");
    let wide = spec(&["cell2", "cell3"], &[FenceClass::Any], "release");
    assert_ne!(fence_confirm_key(&narrow), fence_confirm_key(&wide));
    // Confirming the narrow fence doesn't let fence_open check out the wide one.
    g.confirm_with_words(&fence_confirm_key(&narrow), &open_reason(&narrow)).unwrap();
    assert!(g.check_confirmed(&fence_confirm_key(&wide)).is_err(), "a different spec isn't covered");
    // The narrow one it does cover (re-confirm, since the failed check took the token).
    g.confirm_with_words(&fence_confirm_key(&narrow), &open_reason(&narrow)).unwrap();
    g.check_confirmed(&fence_confirm_key(&narrow)).unwrap();
    // The generic unlock for action "fence_open" doesn't satisfy a bound key.
    g.authenticate(Some("fence_open"), true).unwrap();
    assert!(g.check_confirmed(&fence_confirm_key(&narrow)).is_err(), "the generic prompt can't stand in");
    // The digest covers every field: a spec that differs from the confirmed one in its nodes,
    // classes, only-list, minutes, instance or note alone isn't covered by its confirmation.
    let mut variants: Vec<(&str, Spec)> = Vec::new();
    let mut v = narrow.clone();
    v.nodes = vec!["cell3".into()];
    variants.push(("nodes", v));
    let mut v = narrow.clone();
    v.classes = vec![FenceClass::ContainerTest];
    variants.push(("classes", v));
    let mut v = narrow.clone();
    v.only.insert(FenceClass::FilesTest, vec!["x".into()]);
    variants.push(("only", v));
    let mut v = narrow.clone();
    v.minutes = 480;
    variants.push(("minutes", v));
    let mut v = narrow.clone();
    v.instance = "dev".into();
    variants.push(("instance", v));
    let mut v = narrow.clone();
    v.note = "different note".into();
    variants.push(("note", v));
    for (field, other) in &variants {
        assert_ne!(fence_confirm_key(&narrow), fence_confirm_key(other), "{field}");
        g.confirm_with_words(&fence_confirm_key(&narrow), &open_reason(&narrow)).unwrap();
        assert!(g.check_confirmed(&fence_confirm_key(other)).is_err(), "a confirmation of the narrow fence opened one differing in {field}");
    }
}

// ---- P1 r3 (R3-SAFE)

/// Look-only turning on while Admin is already locked runs no relock hook, so the hold hook the
/// exit installs (`hand_over`) lets go of this build's lease claim here; nothing is written on
/// the node, and the node's own 45 s TTL frees it.
#[tokio::test]
async fn look_only_with_admin_locked_lets_go_of_the_lease_here_only() {
    let (node, home, live, _f) = holding_a_lease("exit-hold-drop").await;
    let (gate, _, _) = crate::admin::testing::gate();
    hand_over(&gate, &live);
    assert!(!gate.state().unlocked, "Admin starts locked");
    restart(&live, "cell2").await.unwrap();
    assert_eq!(live.exit.leases.held(), ["cell2"]);
    gate.set_look_only(true).unwrap();
    assert!(live.exit.leases.held().is_empty(), "the claim is gone here");
    assert!(node.ran("lease release").is_empty(), "{:?}", node.ran("lease"));
    assert!(home.join(".sparkbox/controller.json").exists(), "nothing was written on the node");
    // Closing the fence in look-only sends nothing either.
    close_fence(&live).await.unwrap();
    assert!(node.ran("lease release").is_empty());
}

/// `sh -c` with its options in any order keeps its power verb (POSIX: the script is the first
/// operand after them all): typed, alone or in Fleet's wrapper, as the cluster user or as root,
/// each needs `power-one`, and the Session A shape of fence (fleet, service, shell; no power-one)
/// refuses it before anything reaches the node.
#[tokio::test]
async fn a_typed_sh_c_power_verb_needs_power_one_whatever_the_option_order() {
    let (node, port) = fake_node(&temp("exit-shc-home"), docker()).await;
    let base = temp("exit-shc-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Fleet, FenceClass::Service, FenceClass::Shell], "release"));
    for cmd in crate::ops::classify::contracts::SH_C_POWER {
        for (sent, root) in [(cmd.to_string(), false), (crate::ops::fleet::wrap(cmd, 30, false), false), (crate::ops::fleet::wrap(cmd, 30, true), true)] {
            let r = if root {
                audit::as_typed(live.sudo("cell2", &sent, Duration::from_secs(10))).await.map(drop)
            } else {
                audit::as_typed(live.stream_cmd("cell2", &sent, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map(drop)
            };
            let e = r.map_err(err).unwrap_err();
            assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow restarts and shutdowns."), "`{sent}`");
        }
    }
    // So does a shell that sources its rcfile or BASH_ENV from what it's fed.
    for cmd in ["echo reboot | bash --rcfile /dev/stdin -i -c true", "echo reboot | BASH_ENV=/dev/stdin bash -c true"] {
        let e = audit::as_typed(live.stream_cmd("cell2", cmd, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err).unwrap_err();
        assert_eq!(e.message, "The fence doesn't allow restarts and shutdowns.", "`{cmd}`");
    }
    assert!(node.ran("reboot").is_empty() && node.ran("shutdown").is_empty(), "{:?}", node.ran(""));
    assert_eq!(refusals(&live).len(), 38, "one Refused line each");
}

/// Typed Wi-Fi on or off, the whole command in Fleet's wrapper, needs `wifi`, not `fleet` (C9);
/// with anything more it's a Fleet command again.
#[tokio::test]
async fn typed_wifi_needs_wifi_not_fleet() {
    let (node, port) = fake_node(&temp("exit-wifi-home"), docker()).await;
    node.bin("nmcli", "echo \"wifi $4\"");
    let base = temp("exit-wifi-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    let typed = |sent: String, root: bool| {
        let live = live.clone();
        async move {
            if root {
                audit::as_typed(live.sudo("cell2", &sent, Duration::from_secs(10))).await.map(drop).map_err(err)
            } else {
                audit::as_typed(live.stream_cmd("cell2", &sent, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map(drop).map_err(err)
            }
        }
    };
    let off = |root: bool| crate::ops::fleet::wrap("nmcli radio wifi off", 30, root);
    f.open(&spec(&["cell2"], &[FenceClass::Fleet], "release"));
    for root in [false, true] {
        assert_eq!(typed(off(root), root).await.unwrap_err().message, "The fence doesn't allow Wi-Fi changes.");
    }
    assert!(node.ran("nmcli").is_empty());
    f.open(&spec(&["cell2"], &[FenceClass::Wifi], "release"));
    for root in [false, true] {
        typed(off(root), root).await.unwrap();
    }
    assert_eq!(node.ran("nmcli radio wifi off").len(), 2);
    let row = live.audit.entries().into_iter().find(|e| e.command == off(true) && e.title != "Refused").unwrap();
    assert_eq!(row.fence.as_deref(), Some("wifi"));
    // More than the one command: a Fleet command, which a wifi fence doesn't allow.
    let more = crate::ops::fleet::wrap("nmcli radio wifi off; uptime", 30, false);
    assert_eq!(typed(more, false).await.unwrap_err().message, "The fence doesn't allow Fleet commands.");
}

/// This round's reads at the exit: in look-only, as background checks, the update check and its
/// stamps (C5) and the checks before a restart (C6) are let through; the dry run (C2) runs as a
/// plain read too.
#[tokio::test]
async fn the_update_check_and_the_checks_before_a_restart_are_background_reads() {
    use crate::ops::classify::contracts::*;
    // The node's answers (the fake Mac has no apt, fwupd, snap or GRUB).
    let script: Script = Arc::new(|cmd: &str, _: &[u8]| {
        let ours = cmd.starts_with("echo '--- upgradable'") || cmd.starts_with("stat -c") || cmd.starts_with("readlink -f /boot/vmlinuz") || cmd.starts_with("echo '--- initrd'") || cmd.starts_with("apt-get -s");
        ours.then(|| ("ok\n".to_string(), 0))
    });
    let (node, port) = fake_node(&temp("exit-reads-home"), script).await;
    let base = temp("exit-reads-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    live.set_write_policy(Policy::new(true));
    for cmd in [check_script(), STAMPS.to_string(), C6_KERNEL.to_string(), C6_STEP_B.to_string()] {
        let out = live.probe("cell2", &cmd, Duration::from_secs(20)).await.unwrap();
        assert_eq!(out.stdout, "ok\n", "`{cmd}`");
    }
    for cmd in [C2_ONE, C2_EVERYTHING] {
        live.run("cell2", cmd, None, Duration::from_secs(20)).await.unwrap();
    }
    assert!(refusals(&live).is_empty(), "{:?}", refusals(&live));
    assert_eq!(node.ran("echo '--- upgradable'").len(), 1);
}

/// C1′ at the exit: an apt-one fence admits the pinned upgrade of the packages it names and
/// refuses one of another package, and C1′ with one more command; without an only-list, every
/// package pinned ("Upgrade everything") is admitted. Cancel, `other` by its shape, is admitted
/// where it's tagged apt-one.
#[tokio::test]
async fn an_apt_one_fence_admits_exactly_c1() {
    use crate::ops::classify::contracts::c1_pinned_in;
    let (node, port) = fake_node(&temp("exit-aptone-home"), docker()).await;
    let base = temp("exit-aptone-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let lib = temp("exit-aptone-lib").join("sparkbox").display().to_string();
    live.set_root_lib(&lib);
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    let logs = live.upgrade_log_dir();
    let (one, everything) = (c1_pinned_in(&logs, &["libfoo=2.0-1"]), c1_pinned_in(&logs, &["libfoo=2.0-1", "libbar=2.0-1"]));
    let mut s = spec(&["cell2"], &[FenceClass::AptOne], "release");
    s.only.insert(FenceClass::AptOne, vec!["libfoo".into()]);
    f.open(&s);
    let sudo = |cmd: String| {
        let live = live.clone();
        async move { live.sudo("cell2", &cmd, Duration::from_secs(20)).await.map(drop).map_err(err) }
    };
    sudo(one.clone()).await.unwrap();
    assert_eq!(sudo(everything.clone()).await.unwrap_err().message, "The fence doesn't allow package upgrades to libbar: it names only libfoo.");
    let more = one.replace("exit $code'", "exit $code; touch /tmp/sparkbox-r3-safe-never'");
    assert_eq!(sudo(more).await.unwrap_err().message, "The fence doesn't allow this change.");
    assert!(node.ran("libbar=").is_empty() && node.ran("sparkbox-r3-safe-never").is_empty());
    // Without an only-list, every package pinned is admitted too.
    f.open(&spec(&["cell2"], &[FenceClass::AptOne], "release"));
    sudo(everything).await.unwrap();
    // Cancel: `other` untagged, apt-one where Updates tags it.
    let cancel = crate::ops::updates::cancel_command("sparkbox-upgrade-t1");
    assert_eq!(sudo(cancel.clone()).await.unwrap_err().message, "The fence doesn't allow this change.");
    as_class(FenceClass::AptOne, sudo(cancel.clone())).await.unwrap();
    let row = live.audit.entries().into_iter().find(|e| e.command == cancel && e.title != "Refused").unwrap();
    assert_eq!(row.fence.as_deref(), Some("apt-one"));
}

// ---- P1 r3 round 3 (R3-SAFE): the verifier's defects

/// A typed power verb however a shell that sources an rcfile, `su`/`runuser`, `env -S` or
/// `find -exec` wraps it needs `power-one`, not `fleet`: under the Session A shape (fleet,
/// service, shell; no power-one) each is refused, plain and in Fleet's wrapper, and nothing
/// reaches the node. This is item (b)'s gap reached through other options, the sh -c/bash -c
/// operand gap closed for real.
#[tokio::test]
async fn a_typed_wrapped_power_verb_needs_power_one() {
    let (node, port) = fake_node(&temp("exit-wrapped-home"), docker()).await;
    let base = temp("exit-wrapped-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Fleet, FenceClass::Service, FenceClass::Shell], "release"));
    let wrapped = [
        "bash -ic reboot",
        "bash -i -c reboot",
        "sh -i -c reboot",
        "bash -lic reboot",
        "bash --rcfile /dev/null -c reboot",
        "bash --rcfile=/dev/null -c reboot",
        "bash --init-file /dev/null -c 'systemctl reboot'",
        "bash --rcfile /dev/null -c 'shutdown -h now'",
        "zsh -ic reboot",
        "su -lc reboot",
        "su --command=reboot",
        "runuser -u root --command=reboot",
        "env -S reboot",
        "env -S 'systemctl reboot'",
        "find / -maxdepth 0 -exec reboot ;",
    ];
    for cmd in wrapped {
        for (sent, root) in [(cmd.to_string(), false), (crate::ops::fleet::wrap(cmd, 30, false), false), (crate::ops::fleet::wrap(cmd, 30, true), true)] {
            let r = if root {
                audit::as_typed(live.sudo("cell2", &sent, Duration::from_secs(10))).await.map(drop)
            } else {
                audit::as_typed(live.stream_cmd("cell2", &sent, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map(drop)
            };
            let e = r.map_err(err).unwrap_err();
            assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow restarts and shutdowns."), "`{sent}`");
        }
    }
    assert!(node.ran("reboot").is_empty() && node.ran("shutdown").is_empty(), "{:?}", node.ran(""));
}

/// A typed command that only *contains* a power verb (`touch x; reboot`, an apt install then a
/// reboot) isn't power-one: it's a Fleet command, so under the Session B shape (power-one,
/// apt-*, shell; no fleet) it's refused and its other half never runs. A lone power verb under
/// that same fence is admitted and reaches the node.
#[tokio::test]
async fn a_compound_typed_power_verb_is_a_fleet_command_not_power_one() {
    let (node, port) = fake_node(&temp("exit-compound-home"), docker()).await;
    node.bin("reboot", "exit 0");
    let base = temp("exit-compound-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    // power-one, apt-*, shell — but no fleet (the Session B · cell2 shape).
    f.open(&spec(&["cell2"], &[FenceClass::PowerOne, FenceClass::AptRefresh, FenceClass::AptOne, FenceClass::AptHold, FenceClass::Shell], "release"));
    let marker = "/tmp/sparkbox-r3-compound-never";
    for cmd in [
        format!("touch '{marker}'; reboot"),
        "apt-get -y install nvidia-dkms-580-open; reboot".to_string(),
        "rm /boot/initrd.img-7.0.0-1019-nvidia; reboot".to_string(),
    ] {
        for sent in [cmd.clone(), crate::ops::fleet::wrap(&cmd, 30, true)] {
            let e = audit::as_typed(live.sudo("cell2", &sent, Duration::from_secs(10))).await.map_err(err).unwrap_err();
            assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow Fleet commands."), "`{sent}`");
        }
    }
    assert!(node.ran("touch").is_empty() && node.ran("apt-get").is_empty() && node.ran("initrd").is_empty(), "no first half ran: {:?}", node.ran(""));
    // A lone power verb under the same fence is power-one, and reaches the node.
    audit::as_typed(live.sudo("cell2", "reboot", Duration::from_secs(10))).await.unwrap();
    assert_eq!(node.ran("reboot").len(), 1);
}

/// Look-only turning on while a fenced change waits on its lease claim (both over SSH): the
/// claim that reaches the node isn't kept here, so `held()` stays empty, and the change is
/// refused — nothing runs on the node after look-only is on. The node's own 45 s TTL frees the
/// controller.json the in-flight write left.
#[tokio::test]
async fn look_only_turning_on_during_an_inflight_claim_refuses_and_keeps_no_lease() {
    let home = temp("exit-inflight-home");
    let (node, port) = fake_node(&home, docker()).await;
    // The lease claim sleeps 2 s on the node, so look-only can turn on while it's in flight. The
    // shim execs the real python3 by its absolute path (its own name is python3 on the node's PATH).
    let py = {
        let out = std::process::Command::new("/usr/bin/which").arg("python3").output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    node.bin("python3", &format!("case \"$*\" in *'lease claim'*) sleep 2;; esac\nexec {py} \"$@\""));
    let base = temp("exit-inflight-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Service], "release"));
    let (gate, _, _) = crate::admin::testing::gate();
    hand_over(&gate, &live);
    let l2 = live.clone();
    let job = tokio::spawn(async move { l2.sudo("cell2", RESTART, Duration::from_secs(30)).await.map(drop) });
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(live.exit.leases.held().is_empty(), "the claim hasn't been taken yet");
    gate.set_look_only(true).unwrap();
    let r = job.await.unwrap();
    assert_eq!(err(r.unwrap_err()).kind, ErrorKind::LookOnly, "the change is refused once look-only is on");
    assert!(live.exit.leases.held().is_empty(), "no lease is held here");
    assert!(node.ran("systemctl restart").is_empty(), "the change never ran on the node: {:?}", node.ran("systemctl"));
}

/// Under the T2 fence (container-test, process-test, files-test), a file op whose path carries an
/// unquoted `${IFS}`, `$HOME` or glob is `other` and refused before it reaches the node, so the
/// shell's word splitting can't reach a path under ~/sparkbox-test to a file in ~/models.
#[tokio::test]
async fn files_test_refuses_an_unquoted_expansion_that_would_escape_the_test_folder() {
    use std::os::unix::fs::PermissionsExt;
    let home = temp("exit-ifs-home");
    let (node, port) = fake_node(&home, docker()).await;
    let base = temp("exit-ifs-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::ContainerTest, FenceClass::ProcessTest, FenceClass::FilesTest], "dev"));
    std::fs::create_dir_all(home.join("sparkbox-test/x")).unwrap();
    std::fs::create_dir_all(home.join("models/big")).unwrap();
    std::fs::write(home.join("models/big/weights.bin"), "w").unwrap();
    std::fs::write(home.join("victim.txt"), "keep me").unwrap();
    std::fs::set_permissions(home.join("victim.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();

    // A chmod whose path splits on ${IFS} into a second path ($HOME/victim.txt).
    let e = err(live.run("cell2", "chmod -- 000 ~/sparkbox-test/x${IFS}$HOME/victim.txt", None, Duration::from_secs(10)).await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."));
    assert_eq!(std::fs::metadata(home.join("victim.txt")).unwrap().permissions().mode() & 0o777, 0o644, "victim untouched");

    // A sparkjob rm whose path splits into ~/models.
    let e = err(live.sparkjob("cell2", "rm ~/sparkbox-test/x${IFS}$HOME/models", None, Duration::from_secs(10)).await.unwrap_err());
    assert_eq!(e.kind, ErrorKind::Fenced);
    assert!(home.join("models/big/weights.bin").exists(), "~/models untouched");
    assert!(node.ran("sparkjob.py rm").is_empty(), "nothing reached the node: {:?}", node.ran(""));
}

// ---- P1 round 4 (R4-SAFE)

/// T2 blocker 1 at the exit: typed in Fleet, a command whose command word is a substitution or a
/// variable is no read, so look-only holds it before it reaches the node, and with a fence it
/// needs `fleet`; `uptime` still passes as a read.
#[tokio::test]
async fn a_typed_command_word_from_a_substitution_or_a_variable_is_held_like_a_change() {
    let (node, port) = fake_node(&temp("exit-hidden-home"), docker()).await;
    let base = temp("exit-hidden-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    let policy = Policy::new(true);
    live.set_write_policy(policy.clone());
    let typed = |cmd: &str| {
        let (live, sent) = (live.clone(), crate::ops::fleet::wrap(cmd, 30, false));
        async move { audit::as_typed(live.stream_cmd("cell2", &sent, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err) }
    };
    let hidden = ["$(echo uptime)", "`echo uptime`", "$X", "${X}", "$X/cat /etc/hostname", "c?t /etc/hostname", "echo $'\\x72'", "ls\u{a0}-la"];
    for cmd in hidden {
        assert_eq!(typed(cmd).await.unwrap_err().kind, ErrorKind::LookOnly, "`{cmd}`");
    }
    assert!(node.ran("timeout").is_empty(), "nothing reached the node: {:?}", node.ran(""));
    typed("uptime").await.unwrap();
    assert_eq!(node.ran("uptime").len(), 1);
    // Not look-only: a fence without `fleet` refuses them; with it they run as Fleet commands.
    policy.look_only.store(false, Ordering::SeqCst);
    f.open(&spec(&["cell2"], &[FenceClass::PowerOne, FenceClass::Shell], "dev"));
    for cmd in hidden {
        assert_eq!(typed(cmd).await.unwrap_err().message, "The fence doesn't allow Fleet commands.", "`{cmd}`");
    }
    // A power verb hidden that way is a Fleet command, never power-one (the named limit).
    for cmd in ["$(echo reboot)", "r?boot", "{reboot,}", "/tmp/x/reboot"] {
        assert_eq!(typed(cmd).await.unwrap_err().message, "The fence doesn't allow Fleet commands.", "`{cmd}`");
    }
    assert_eq!(node.ran("timeout").len(), 1, "only uptime ran");
}

/// C-X: Cancel, tagged apt-one as Updates sends it, passes an apt-one fence whatever its `only`
/// list names (stopping apt only reduces what installs), and reaches the node. A look-alike one
/// character off is `other` whatever its tag (P1 r4), and so is the exact text under another class.
#[tokio::test]
async fn cancel_passes_an_apt_one_only_list_and_a_look_alike_doesnt() {
    let (node, port) = fake_node(&temp("exit-cancel-home"), docker()).await;
    let base = temp("exit-cancel-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "release");
    live.enforce_writes(f.rules.clone());
    let mut s = spec(&["cell2"], &[FenceClass::AptOne, FenceClass::Service], "release");
    s.only.insert(FenceClass::AptOne, vec!["libbrotli1".into()]);
    s.only.insert(FenceClass::Service, vec!["earlyoom.service".into()]);
    f.open(&s);
    let sudo = |cmd: String, class: FenceClass| {
        let live = live.clone();
        async move { as_class(class, live.sudo("cell2", &cmd, Duration::from_secs(20))).await.map(drop).map_err(err) }
    };
    let cancel = crate::ops::updates::cancel_command("sparkbox-upgrade-t1");
    sudo(cancel.clone(), FenceClass::AptOne).await.unwrap();
    assert_eq!(node.ran("systemctl show -p MainPID --value -- 'sparkbox-upgrade-t1'").len(), 1, "it reached the node");
    let row = live.audit.entries().into_iter().find(|e| e.command == cancel && e.title != "Refused").unwrap();
    assert_eq!(row.fence.as_deref(), Some("apt-one"));
    for bad in [cancel.replace("exit 3", "exit 4"), cancel.replacen("pkill", "pkilL", 1), format!("{cancel} "), cancel.replace("sparkbox-upgrade-t1", "sparkbox-upgrade-t1;")] {
        assert_eq!(sudo(bad.clone(), FenceClass::AptOne).await.unwrap_err().message, "The fence doesn't allow this change.", "`{bad}`");
    }
    assert_eq!(sudo(cancel.clone(), FenceClass::Service).await.unwrap_err().message, "The fence doesn't allow this change.");
    assert_eq!(node.ran("systemctl show -p MainPID").len(), 1, "nothing refused reached the node");
}

/// C-R: the one-token reply and the 17+25 check run as inference (`as_use`): no fence needed,
/// look-only holds them. Untagged, the same request is no read.
#[tokio::test]
async fn the_readiness_token_and_sanity_check_are_use_held_in_look_only() {
    use crate::models::readiness::{sanity_command, thinking_command};
    use crate::ops::classify::contracts::{R_SANITY, R_SANITY_THINKING};
    // The contract literals are what readiness sends.
    assert_eq!(sanity_command("192.168.50.154", 8000, "gemma-3-27b-it", true), R_SANITY);
    assert_eq!(thinking_command("192.168.50.154", 8000, "gemma-3-27b-it", true), R_SANITY_THINKING);
    // The node answers curl itself: no request leaves this Mac.
    let script: Script = Arc::new(|cmd: &str, _: &[u8]| cmd.starts_with("curl ").then(|| ("{\"choices\":[{\"message\":{\"content\":\"42\"}}]}\n200".to_string(), 0)));
    let (node, port) = fake_node(&temp("exit-use-home"), script).await;
    let base = temp("exit-use-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    let policy = Policy::new(false);
    live.set_write_policy(policy.clone());
    let sanity = R_SANITY.replace("192.168.50.154", "127.0.0.1");
    let token = sanity.replace("What is 17+25? Reply with the number only.", "Hi").replace("\"max_tokens\":8", "\"max_tokens\":1");
    let thinking = R_SANITY_THINKING.replace("192.168.50.154", "127.0.0.1");
    for cmd in [&token, &sanity, &thinking] {
        let out = as_use(live.run("cell2", cmd, Some(b"Authorization: Bearer x\n"), Duration::from_secs(10))).await.unwrap();
        assert!(out.stdout.ends_with("200"), "{out:?}");
    }
    let admitted: Vec<_> = live.admits().into_iter().filter(|a| a.cmd.starts_with("curl ")).collect();
    assert!(admitted.iter().all(|a| a.verdict == Verdict::Use && a.admitted), "{admitted:?}");
    // Untagged, it's a change no fence names.
    assert_eq!(err(live.run("cell2", &sanity, None, Duration::from_secs(10)).await.unwrap_err()).message, "No fence is open, so changes on the cluster are refused. This test build needs one: open it in the release SparkBox (Settings › Fence).");
    policy.look_only.store(true, Ordering::SeqCst);
    for cmd in [&token, &sanity, &thinking] {
        assert_eq!(err(as_use(live.run("cell2", cmd, None, Duration::from_secs(10))).await.unwrap_err()).kind, ErrorKind::LookOnly);
    }
    assert_eq!(node.ran("curl").len(), 3, "only the three admitted ran");
}

/// Check (d) through what the shell builds at run time, and C-U2's label: a typed container name
/// from a variable or a glob, or one behind a block's words, could be the other build's, so it's
/// refused; an unload whose label filter names the other build is refused too.
#[tokio::test]
async fn check_d_covers_names_built_at_run_time_and_an_unloads_label() {
    let (node, live, base) = two_owners("exit-checkd-runtime").await;
    // The unload's own `docker ps`/`stop`/`rm` answer here, never the Mac's Docker.
    node.bin("docker", "exit 0");
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell1"], &[FenceClass::Fleet, FenceClass::ModelUnload], "dev"));
    let typed = |cmd: &str| {
        let (live, cmd) = (live.clone(), cmd.to_string());
        async move { audit::as_typed(live.stream_cmd("cell1", &cmd, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err) }
    };
    for cmd in ["X=gemma-release; docker rm -f $X", "docker stop $(docker ps -q)", "docker kill gemma-*", "{ docker rm gemma-release; }", "if true; then docker stop gemma-release; fi"] {
        let e = typed(cmd).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::OtherController, "`{cmd}`: {}", e.message);
    }
    let run = |cmd: String| {
        let live = live.clone();
        async move { live.run("cell1", &cmd, None, Duration::from_secs(10)).await.map(drop).map_err(err) }
    };
    use crate::ops::classify::contracts::C_U;
    let e = run(C_U.into()).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "This unload on cell1 would remove the other SparkBox's (release) containers."));
    run(C_U.replace("instance=release", "instance=dev")).await.unwrap();
    assert!(node.ran("docker rm").iter().all(|c| c.contains("instance=dev")), "{:?}", node.ran("docker rm"));
}

// ---- P1 round 4 fixes (R4-SAFE): the T2 blockers' re-test

/// A typed `bash -c` read whose value bash evaluates (`[[ $y -eq 0 ]]` with y read as
/// `a[$(touch M)]`) is no read: in look-only it's refused and runs nothing. Not look-only, under
/// a fleet fence, it's a Fleet command and the node's bash really runs the `$(…)` (this Mac's bash
/// 3.2 evaluates it as Ubuntu's 5.2 does), so the refusal is all that kept it from running.
#[tokio::test]
async fn a_typed_bash_arithmetic_read_runs_nothing_in_look_only() {
    let (node, port) = fake_node(&temp("exit-arith-home"), fake_node::real_shell()).await;
    let base = temp("exit-arith-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    let policy = Policy::new(true);
    live.set_write_policy(policy.clone());
    let typed = |cmd: String| {
        let (live, sent) = (live.clone(), crate::ops::fleet::wrap(&cmd, 30, false));
        async move { audit::as_typed(live.stream_cmd("cell2", &sent, Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err) }
    };
    let marker = base.join("ran");
    let m = marker.display();
    let reads = [
        format!("bash -c \"echo 'a[\\$(touch {m})]' | {{ read y; [[ \\$y -eq 0 ]]; }}\""),
        format!("bash -c \"echo 'a[\\$(touch {m})]' | {{ read y; x=1; echo \\${{x[y]}}; }}\""),
        format!("bash -c \"echo 'a[\\$(touch {m})]' | {{ read y; echo \\${{!y}}; }}\""),
    ];
    for cmd in &reads {
        assert_eq!(typed(cmd.clone()).await.unwrap_err().kind, ErrorKind::LookOnly, "`{cmd}`");
    }
    assert!(!marker.exists() && node.ran("bash").is_empty(), "nothing reached the node: {:?}", node.ran(""));
    typed("uptime".into()).await.unwrap();
    assert_eq!(node.ran("uptime").len(), 1, "a typed read still runs in look-only");
    // Under a fleet fence it's a Fleet command: it runs, and the `$(…)` in the value with it.
    policy.look_only.store(false, Ordering::SeqCst);
    f.open(&spec(&["cell2"], &[FenceClass::Fleet], "dev"));
    typed(reads[0].clone()).await.unwrap();
    assert!(marker.exists(), "bash evaluated y's value: {:?}", node.ran("bash"));
}

/// A redirection the shapes don't name opens a file whatever the shape (blocker 2): an exec_io
/// copy-out (a read) with `>victim` is refused in look-only, Files' New folder with a glued
/// `>victim2` and C7 with `>victim3` are refused under T2's fence, and none of the files changes.
/// The same copy-out without it is still a read.
#[tokio::test]
async fn a_shapes_unnamed_redirection_writes_nothing_in_look_only_or_under_t2() {
    let home = temp("exit-redirect-home");
    let (node, port) = fake_node(&home, fake_node::real_shell()).await;
    // C7's docker answers here, never this Mac's Docker.
    node.bin("docker", "exit 0");
    let base = temp("exit-redirect-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    let policy = Policy::new(true);
    live.set_write_policy(policy.clone());
    std::fs::write(home.join("x"), "file to pack\n").unwrap();
    for v in ["victim", "victim2", "victim3"] {
        std::fs::write(home.join(v), "precious\n").unwrap();
    }
    let h = home.display().to_string();
    let e = err(live.exec_io("cell2", &format!("tar -C '{h}' -cvf - -- 'x' >'{h}/victim'")).await.map(drop).unwrap_err());
    assert_eq!(e.kind, ErrorKind::LookOnly);
    let e = err(live.exec_io("cell2", &format!("tar -C / -cvf - -- etc/hostname >{h}/victim")).await.map(drop).unwrap_err());
    assert_eq!(e.kind, ErrorKind::LookOnly);
    assert!(live.exec_io("cell2", &format!("tar -C '{h}' -cvf - -- 'x'")).await.is_ok(), "the copy-out alone is a read");
    // Not look-only, T2's fence: test containers, process signals, files in ~/sparkbox-test.
    policy.look_only.store(false, Ordering::SeqCst);
    f.open(&spec(&["cell2"], &[FenceClass::ContainerTest, FenceClass::ProcessTest, FenceClass::FilesTest], "dev"));
    std::fs::create_dir_all(home.join("sparkbox-test")).unwrap();
    for cmd in [format!("mkdir -- ~/sparkbox-test/x>{h}/victim2"), format!("{} >'{h}/victim3'", crate::ops::classify::contracts::c7("dev")), format!("kill -s TERM 4242 >{h}/victim3")] {
        let e = err(live.run("cell2", &cmd, None, Duration::from_secs(10)).await.unwrap_err());
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."), "`{cmd}`");
    }
    for v in ["victim", "victim2", "victim3"] {
        assert_eq!(std::fs::read_to_string(home.join(v)).unwrap(), "precious\n", "{v}");
    }
    assert!(node.ran("mkdir -- ").is_empty() && node.ran("docker run").is_empty() && node.ran("kill -s").is_empty(), "{:?}", node.ran(""));
    assert!(!home.join("sparkbox-test/x").exists());
}

/// Safety review B: a call-site tag never relabels a command whose shape is `other` into its
/// class. Under a fence that names only the tag's class, each of these is refused and nothing
/// reaches the node — as the cluster user and as root. The commands each tagged call site really
/// sends (C7, C-L2, C-F, C-U2, C-S, C1′, C-X, the drop, `shutdown -c`, the PID-guarded signal,
/// Doctor's sshd check) are still admitted under their class.
#[tokio::test]
async fn a_tag_never_relabels_an_other_command() {
    use crate::ops::classify::contracts::{self, c1_pinned_in, c_l_no_key, C_S, C_U};
    let home = temp("exit-relabel-home");
    let (node, port) = fake_node(&home, fake_node::real_shell()).await;
    node.bin("docker", "exit 0");
    let base = temp("exit-relabel-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let lib = temp("exit-relabel-lib").join("sparkbox").display().to_string();
    live.set_root_lib(&lib);
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    std::fs::write(home.join("victim"), "precious\n").unwrap();
    let h = home.display().to_string();
    for (class, sudo, cmd) in [
        (FenceClass::ModelLoadOne, false, format!("touch '{h}/zz-load'")),
        (FenceClass::ModelLoad, false, format!("touch '{h}/zz-load-many'")),
        (FenceClass::ModelUnload, false, format!("rm -f '{h}/victim'")),
        (FenceClass::ContainerTest, false, format!("touch '{h}/zz-ct'")),
        (FenceClass::ProcessTest, false, format!("touch '{h}/zz-pt'")),
        (FenceClass::FilesTest, false, format!("touch '{h}/zz-ft'")),
        (FenceClass::AptOne, true, format!("touch '{h}/zz-apt'")),
        (FenceClass::DropCaches, true, format!("touch '{h}/zz-drop'")),
        (FenceClass::Wifi, true, format!("touch '{h}/zz-wifi'")),
        (FenceClass::Service, true, format!("touch '{h}/zz-svc'")),
        (FenceClass::SudoProbe, true, format!("touch '{h}/zz-probe'")),
    ] {
        f.open(&spec(&["cell2"], &[class], "dev"));
        let r = if sudo { as_class(class, live.sudo("cell2", &cmd, Duration::from_secs(10))).await } else { as_class(class, live.run("cell2", &cmd, None, Duration::from_secs(10))).await };
        let e = err(r.unwrap_err());
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."), "{class:?} `{cmd}`");
    }
    assert!(node.ran("touch").is_empty() && node.ran("rm -f").is_empty(), "{:?}", node.ran(""));
    assert_eq!(std::fs::read_to_string(home.join("victim")).unwrap(), "precious\n");
    // What the call sites send, under the class each tags it with: admitted (asked, not run).
    let logs = live.upgrade_log_dir();
    let rows: Vec<(FenceClass, ExecKind, String)> = vec![
        (FenceClass::ContainerTest, ExecKind::Run, contracts::c7("dev")),
        (FenceClass::ModelLoadOne, ExecKind::Run, c_l_no_key().replace("instance=release", "instance=dev")),
        (FenceClass::ModelLoadOne, ExecKind::Sparkjob, "deploy-files d1 single".into()),
        (FenceClass::ModelLoad, ExecKind::Run, contracts::C_L_HEAD.replace("instance=release", "instance=dev")),
        (FenceClass::ModelLoad, ExecKind::Run, contracts::C_L_WORKER.replace("instance=release", "instance=dev")),
        (FenceClass::ModelLoad, ExecKind::Sparkjob, "deploy-files d1 head".into()),
        (FenceClass::ModelLoad, ExecKind::Sparkjob, "deploy-files d1 worker".into()),
        (FenceClass::ModelUnload, ExecKind::Run, C_U.replace("instance=release", "instance=dev")),
        (FenceClass::ModelUnload, ExecKind::Run, C_S.replace("instance=release", "instance=dev")),
        (FenceClass::AptOne, ExecKind::Sudo, c1_pinned_in(&logs, &["libfoo=2.0-1"])),
        (FenceClass::AptOne, ExecKind::Sudo, crate::ops::updates::cancel_command("sparkbox-upgrade-t1")),
        (FenceClass::DropCaches, ExecKind::Sudo, crate::models::plan::DROP_CACHES.into()),
        (FenceClass::PowerOne, ExecKind::Sudo, "shutdown -c".into()),
        (FenceClass::PowerOne, ExecKind::Sudo, crate::ops::power_command("reboot", 5)),
        (FenceClass::ProcessTest, ExecKind::Run, crate::ops::pid_guard_cmd(4242, "TERM", Some(98_765), "/proc")),
        (FenceClass::ProcessTest, ExecKind::Run, "renice -n 5 -p 4242".into()),
        (FenceClass::SudoProbe, ExecKind::Sudo, crate::live::DOCTOR_SSHD_T.into()),
        (FenceClass::Service, ExecKind::Sudo, crate::ops::services::action_command("earlyoom.service", "restart")),
        (FenceClass::AptHold, ExecKind::Sudo, crate::ops::updates::hold_command(&["libfoo".into()], true)),
        (FenceClass::AptRefresh, ExecKind::Sudo, crate::ops::update_check::REFRESH.into()),
    ];
    for (class, kind, cmd) in &rows {
        f.open(&spec(&["cell2"], &[*class], "dev"));
        let admitted = as_class(*class, live.exit.admit(&live, "cell2", *kind, cmd)).await;
        assert_eq!(admitted.map_err(|e| e.message), Ok(Admitted { fence: Some(class.id().to_string()) }), "{class:?} `{cmd}`");
    }
}

/// Safety review E: a load labels its container for one build, and the exit refuses one labelled
/// for the other build (it would treat the container as its own: unload it, count it). The dev
/// build's load labelled dev, and the release build's labelled release, pass; nothing refused
/// reaches the node.
#[tokio::test]
async fn a_load_labelled_for_the_other_build_is_refused() {
    use crate::ops::classify::contracts::{c_l_no_key, C_L_HEAD, C_L_WORKER};
    let (node, port) = fake_node(&temp("exit-loadlabel-home"), fake_node::real_shell()).await;
    node.bin("docker", "exit 0");
    let base = temp("exit-loadlabel-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    for (class, release) in [(FenceClass::ModelLoadOne, c_l_no_key()), (FenceClass::ModelLoad, C_L_HEAD.to_string()), (FenceClass::ModelLoad, C_L_WORKER.to_string())] {
        let dev = release.replace("instance=release", "instance=dev");
        for (me, mine, theirs, owner) in [("dev", &dev, &release, "release"), ("release", &release, &dev, "dev")] {
            let f = TestFence::new(&temp(&format!("exit-loadlabel-{me}")), me);
            live.enforce_writes(f.rules.clone());
            f.open(&spec(&["cell2"], &[class], me));
            let before = node.ran("docker run").len();
            let e = err(as_class(class, live.run("cell2", theirs, None, Duration::from_secs(10))).await.unwrap_err());
            assert_eq!((e.kind, e.message), (ErrorKind::OtherController, format!("This load on cell2 would start a container labeled as the other SparkBox's ({owner}).")), "{me} `{theirs}`");
            assert_eq!(node.ran("docker run").len(), before, "{me}: refused before it left");
            as_class(class, live.run("cell2", mine, None, Duration::from_secs(10))).await.unwrap();
            assert_eq!(node.ran("docker run").len(), before + 1, "{me}: its own load ran");
        }
    }
}

// ---- P2 round 1 (R1-SAFE): a load on several nodes

/// The contracts as the dev build writes them: a rank's `docker run`, and its launch files.
fn dev_ranks() -> [(FenceClass, ExecKind, String); 6] {
    use crate::ops::classify::contracts::{c_l_no_key, C_L, C_L_HEAD, C_L_WORKER};
    let dev = |c: &str| c.replace("instance=release", "instance=dev");
    [
        (FenceClass::ModelLoadOne, ExecKind::Run, dev(C_L)),
        (FenceClass::ModelLoadOne, ExecKind::Run, dev(&c_l_no_key())),
        (FenceClass::ModelLoad, ExecKind::Run, dev(C_L_HEAD)),
        (FenceClass::ModelLoad, ExecKind::Run, dev(C_L_WORKER)),
        (FenceClass::ModelLoadOne, ExecKind::Sparkjob, "deploy-files d1 single".into()),
        (FenceClass::ModelLoad, ExecKind::Sparkjob, "deploy-files d1 worker".into()),
    ]
}

/// A rank at the exit, tagged with `tag` as a call site would.
async fn admit_rank(live: &Live, tag: FenceClass, kind: ExecKind, cmd: &str) -> Result<Admitted, SbError> {
    as_class(tag, live.exit.admit(live, "cell2", kind, cmd)).await
}

/// `model-load` admits a load of either size, so Session 1 needs one fence. `model-load-one` admits
/// a one-node load and refuses a head or a worker, whichever class the call site tags it with,
/// because the shape decides. With no fence, or one naming neither, the debug build refuses all of
/// them. A line in the log names the class that let it through.
#[tokio::test]
async fn a_load_on_several_nodes_needs_model_load_and_that_admits_one_node_too() {
    let (node, port) = fake_node(&temp("exit-modelload-home"), fake_node::real_shell()).await;
    node.bin("docker", "exit 0");
    let base = temp("exit-modelload-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());

    // No fence at all.
    for (class, kind, cmd) in dev_ranks() {
        let e = admit_rank(&live, class, kind, &cmd).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Fenced, "`{cmd}`: {}", e.message);
    }
    // A fence that names neither.
    f.open(&spec(&["cell2"], &[FenceClass::ModelUnload], "dev"));
    for (class, kind, cmd) in dev_ranks() {
        let e = admit_rank(&live, class, kind, &cmd).await.unwrap_err();
        let want = if class == FenceClass::ModelLoad { "The fence doesn't allow model loads on several nodes." } else { "The fence doesn't allow model loads." };
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, want), "`{cmd}`");
    }
    // model-load-one: one node passes, a head or a worker doesn't, however it's tagged.
    f.open(&spec(&["cell2"], &[FenceClass::ModelLoadOne], "dev"));
    for (class, kind, cmd) in dev_ranks() {
        for tag in [FenceClass::ModelLoadOne, class] {
            let got = admit_rank(&live, tag, kind, &cmd).await;
            if class == FenceClass::ModelLoadOne {
                assert_eq!(got.map_err(|e| e.message), Ok(Admitted { fence: Some("model-load-one".into()) }), "`{cmd}`");
            } else {
                let e = got.unwrap_err();
                assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow model loads on several nodes."), "tagged {tag:?}: `{cmd}`");
            }
        }
    }
    // model-load: every rank passes, and each line says which class it was.
    f.open(&spec(&["cell2"], &[FenceClass::ModelLoad], "dev"));
    for (class, kind, cmd) in dev_ranks() {
        let got = admit_rank(&live, class, kind, &cmd).await;
        assert_eq!(got.map_err(|e| e.message), Ok(Admitted { fence: Some(class.id().to_string()) }), "`{cmd}`");
    }
    // Through Live: what a fence refuses never reaches the node.
    f.open(&spec(&["cell2"], &[FenceClass::ModelLoadOne], "dev"));
    let worker = dev_ranks()[3].2.clone();
    assert!(as_class(FenceClass::ModelLoad, live.run("cell2", &worker, None, Duration::from_secs(10))).await.is_err());
    assert!(node.ran("docker run").is_empty(), "{:?}", node.ran("docker"));
    f.open(&spec(&["cell2"], &[FenceClass::ModelLoad], "dev"));
    as_class(FenceClass::ModelLoad, live.run("cell2", &worker, None, Duration::from_secs(10))).await.unwrap();
    assert_eq!(node.ran("docker run").len(), 1);
}

/// `model-load` standing in for `model-load-one` keeps its `only` list: a fence that names one
/// container admits that one-node load and no other. Nothing goes the other way: an `only` list on
/// `model-load-one` says nothing about a head or a worker.
#[tokio::test]
async fn model_loads_only_list_covers_a_one_node_load() {
    let (node, port) = fake_node(&temp("exit-modelonly-home"), fake_node::real_shell()).await;
    node.bin("docker", "exit 0");
    let base = temp("exit-modelonly-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    let mine = dev_ranks()[1].2.clone();
    let theirs = mine.replace("sparkbox-d1", "sparkbox-d2").replace("deployment=d1", "deployment=d2").replace("group=d1", "group=d2").replace("deploy/d1", "deploy/d2");
    let mut s = spec(&["cell2"], &[FenceClass::ModelLoad], "dev");
    s.only.insert(FenceClass::ModelLoad, vec!["sparkbox-d1".to_string()]);
    f.open(&s);
    assert!(admit_rank(&live, FenceClass::ModelLoadOne, ExecKind::Run, &mine).await.is_ok());
    let e = admit_rank(&live, FenceClass::ModelLoadOne, ExecKind::Run, &theirs).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow model loads to sparkbox-d2: it names only sparkbox-d1."));
}

/// Look-only refuses every rank and its launch files before they leave, and C-P2 and C-J, which
/// run while a load is being set up, pass it as reads. So does a C-P2 under `probe`.
#[tokio::test]
async fn look_only_refuses_every_rank_and_passes_the_preflight_and_the_jumbo_ping() {
    use crate::ops::classify::contracts::{C_J, C_P};
    let (node, port) = fake_node(&temp("exit-modellook-home"), fake_node::real_shell()).await;
    for bin in ["docker", "ping", "ss", "curl", "systemctl"] {
        node.bin(bin, "exit 0");
    }
    let base = temp("exit-modellook-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::ModelLoad, FenceClass::ModelLoadOne, FenceClass::ModelUnload], "dev"));
    live.set_write_policy(Policy::new(true));
    for (class, kind, cmd) in dev_ranks() {
        let e = admit_rank(&live, class, kind, &cmd).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::LookOnly, "`{cmd}`: {}", e.message);
    }
    let unload = crate::ops::classify::contracts::C_U.replace("instance=release", "instance=dev");
    assert_eq!(live.exit.admit(&live, "cell2", ExecKind::Run, &unload).await.unwrap_err().kind, ErrorKind::LookOnly);
    assert!(node.ran("docker run").is_empty(), "{:?}", node.ran("docker"));
    for cmd in [C_P, C_J] {
        for kind in [ExecKind::Run, ExecKind::Probe] {
            let got = live.exit.admit(&live, "cell2", kind, cmd).await;
            assert_eq!(got.map_err(|e| e.message), Ok(Admitted::default()), "{kind:?} `{cmd}`");
        }
    }
    let out = live.run("cell2", C_P, None, Duration::from_secs(10)).await.unwrap();
    assert!(out.stdout.contains("--- mem") && out.stdout.contains("--- ollama"), "{}", out.stdout);
    assert_eq!(node.ran("ping").len(), 0, "C-P2 has no ping");
    let out = live.run("cell2", C_J, None, Duration::from_secs(10)).await.unwrap();
    assert_eq!(out.stdout, "half1 ok\nhalf2 ok\n", "both halves ran");
    assert_eq!(node.ran("ping").len(), 1, "one command, sent as a read");
}

/// What the classifier couldn't read, each of which got through this exit as a read and did it, on
/// a real shell with look-only on: an option the shell supplies, a verb behind `git --namespace`, a
/// format from a file or stdin. Each is refused now, and with look-only off it does what it says on
/// the real tool, so the row proves something. The same tools only looking still run.
#[tokio::test]
async fn what_a_rule_cant_read_stays_behind_look_only() {
    let home = temp("exit-built-home");
    let (_node, port) = fake_node(&home, fake_node::real_shell()).await;
    let (live, _) = fake_node::connect(&temp("exit-built-app"), &[("cell1", port)]).await;
    let policy = Policy::new(true);
    live.set_write_policy(policy.clone());
    let sh = |cmd: &str| std::process::Command::new("/bin/sh").args(["-c", cmd]).current_dir(&home).output().unwrap();
    let setup = "rm -rf keep repo sorted curled fmt; mkdir keep && echo x > keep/precious && printf 'b\\na\\n' > y && printf '%%output{curled}hello' > fmt \
                 && git init -q repo && cd repo && echo a > a && git add a && git -c user.name=a -c user.email=a@b commit -qm one && git config user.name a && git config user.email a@b && echo b > a";
    let gone = "! test -e keep/precious";
    let two = "test \"$(git -C repo rev-list --count HEAD)\" = 2";
    let d = home.display();
    let rows = [
        ("find keep $(echo -delete)".to_string(), gone),
        ("find keep \"$(echo -delete)\"".to_string(), gone),
        ("for x in -delete; do find keep $x; done".to_string(), gone),
        ("echo -delete | { read x; find keep $x; }".to_string(), gone),
        ("sh -c 'find keep $1' x -delete".to_string(), gone),
        ("sort $(echo -o sorted) y".to_string(), "test -s sorted"),
        ("sort \"$(echo -osorted)\" y".to_string(), "test -s sorted"),
        (format!("cd {d} && printf '%%output{{curled}}hello' | curl -s -o /dev/null -w @- file://{d}/y"), "test -s curled"),
        (format!("cd {d} && curl -s -o /dev/null -w @fmt file://{d}/y"), "test -s curled"),
        (format!("cd {d} && curl -o /dev/null -sw@fmt file://{d}/y"), "test -s curled"),
        (format!("curl -4so {d}/curled file://{d}/y"), "test -s curled"),
        (format!("curl -s $(echo -o{d}/curled) file://{d}/y"), "test -s curled"),
        ("cd repo && git --namespace log commit --allow-empty -qm by-look-only".to_string(), two),
        ("cd repo && git --namespace status commit -qam two".to_string(), two),
        ("cd repo && git --namespace log reset -q --hard".to_string(), "grep -qx a repo/a"),
    ];
    for (cmd, did) in &rows {
        assert!(sh(setup).status.success(), "the setup");
        let e = err(live.run("cell1", cmd, None, Duration::from_secs(10)).await.unwrap_err());
        assert_eq!(e.kind, ErrorKind::LookOnly, "`{cmd}`: {}", e.message);
        assert!(!sh(did).status.success(), "`{cmd}` did something while refused");
        policy.look_only.store(false, Ordering::SeqCst);
        live.run("cell1", cmd, None, Duration::from_secs(10)).await.unwrap();
        policy.look_only.store(true, Ordering::SeqCst);
        assert!(sh(did).status.success(), "`{cmd}` didn't do it on the real tool, so its row proves nothing");
    }
    assert!(sh(setup).status.success(), "the setup");
    for (cmd, want) in [
        ("find keep -type f", "keep/precious\n"),
        ("sort y", "a\nb\n"),
        ("cat $(echo y)", "b\na\n"),
        ("for x in y; do cat $x; done", "b\na\n"),
        ("cd repo && git --no-pager log --format=%s", "one\n"),
    ] {
        let out = live.run("cell1", cmd, None, Duration::from_secs(10)).await.unwrap();
        assert_eq!((out.code, out.stdout.as_str()), (Some(0), want), "`{cmd}`");
    }
}

/// The same commands typed into Fleet, or sent by a call site, with Admin open: a read needs no
/// fence, and these aren't reads, so without the class the debug build refuses them.
#[tokio::test]
async fn what_a_rule_cant_read_needs_a_fence() {
    let home = temp("exit-built-fence-home");
    let (_node, port) = fake_node(&home, fake_node::real_shell()).await;
    let base = temp("exit-built-fence-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[FenceClass::Service], "dev"));
    std::fs::create_dir(home.join("keep")).unwrap();
    std::fs::write(home.join("keep/precious"), "x").unwrap();
    let token = crate::live::CancelToken::new;
    for cmd in ["find keep $(echo -delete)", "for x in -delete; do find keep $x; done", "git --namespace log commit -qm x", "curl -w @- file:///etc/hosts"] {
        let e = audit::as_typed(live.stream_cmd("cell2", cmd, Duration::from_secs(10), token(), |_, _| {})).await.map_err(err).unwrap_err();
        assert_eq!(e.message, "The fence doesn't allow Fleet commands.", "`{cmd}`");
        let e = err(live.run("cell2", cmd, None, Duration::from_secs(10)).await.unwrap_err());
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."), "`{cmd}`");
    }
    assert!(home.join("keep/precious").exists());
    audit::as_typed(live.stream_cmd("cell2", "find keep -type f", Duration::from_secs(10), token(), |_, _| {})).await.unwrap();
}
