//! P1 Session A and the T2 fence, rehearsed on fake nodes (docs/acceptance/P1-session-A.md).
//! Each test walks the playbook's steps that send a command to a node, through the entry
//! points the windows use (Live, Ops, Files, Terminals), with the write exit enforcing a fence
//! opened the way Settings › Fence opens one: a record signed with the release build's key, in
//! the one fence folder both builds share, trusted by each build's own config folder. It
//! asserts what the exit admitted, under which class, and the activity row it wrote, and that
//! the same step is refused when the fence leaves that class out.
//!
//! Session A runs on the release build under the "P1 Session A" preset (all four nodes;
//! sudo-probe, root-helper, service, fleet, shell, container-owner; service only for
//! earlyoom.service and cups.service; 3 h). T2 runs on the dev build under the "Agent tests on
//! cell2" preset (container-test, process-test, files-test; 8 h), opened at step 17.
//!
//! Since 2026-10-30 the release build needs no fence, so Settings no longer offers the Session A
//! preset. `cargo test` is a debug build, whose exit asks for a fence whatever instance its rules
//! name (exit.rs `BUILD_NEEDS_FENCE`), so these walks keep proving what they always did: each
//! step's class, the rows it writes, and the refusal when a fence leaves that class out. The
//! preset lives on here as a fixture. live/exit_release_tests.rs walks the release build's path.
//!
//! What a fake can't show (real root, the Mac's own network and sleep, the windows) is the
//! owner's, in the session; the playbook marks each such step. Where a T2 action is refused
//! today although the plan expects it to pass, the test says so (`GAP`) and asserts what
//! SparkBox does now, so the gap stays visible until its fix lands.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::exit::{as_class, Enforce, Verdict, WritePolicy, KEY_KIND};
use super::fence::{self, Record, Spec};
use super::fence_class::{class_of, Ctx, FenceClass, Shape};
use super::{audit, now_ms, root_install_command, root_sparkjob_command, sh_quote, sha256_hex, CancelToken, Live, SPARKJOB};
use crate::errors::{ErrorKind, SbError};
use crate::fake_node::{self, fake_node, fake_spark, temp, Node, Script};
use crate::files::Files;
use crate::inventory::{Inventory, NodeConfig};
use crate::model::LinkStatus;
use crate::ops::fleet::FleetSpec;
use crate::ops::Ops;
use crate::tasks::Tasks;
use crate::term::{Target, TermEvent, Terminals};

use FenceClass::{ContainerOwner, ContainerTest, FilesTest, Fleet, ProcessTest, RootHelper, Service, Shell, SudoProbe};

const MAC: &str = "Studio";
const TAG: &str = "p1-r2-rehearsal";
const FOUR: [&str; 4] = ["cell1", "cell2", "cell3", "gx10-0000"];
/// The "P1 Session A" preset, in the order src/lib/fence.ts had it.
const SESSION_A: [FenceClass; 6] = [SudoProbe, RootHelper, Service, Fleet, Shell, ContainerOwner];
/// src/lib/fence.ts "Agent tests on cell2 (test build)": the one preset Settings still offers, so
/// this copy is held to fence.ts's text (`the_t2_preset_is_fence_ts_text`).
const T2: [FenceClass; 3] = [ContainerTest, ProcessTest, FilesTest];

/// The "P1 Session A" preset with `classes` (service only for earlyoom and cups, when it's in).
fn session_a(classes: &[FenceClass]) -> Spec {
    let mut only = BTreeMap::new();
    if classes.contains(&Service) {
        only.insert(Service, vec!["earlyoom.service".to_string(), "cups.service".to_string()]);
    }
    Spec { nodes: FOUR.map(String::from).to_vec(), classes: classes.to_vec(), only, minutes: 180, instance: "release".into(), note: "P1 Session A".into() }
}

/// The "Agent tests on cell2" preset with `classes`: the dev build, cell2, 8 h.
fn t2(classes: &[FenceClass]) -> Spec {
    Spec { nodes: vec!["cell2".into()], classes: classes.to_vec(), minutes: 480, instance: "dev".into(), note: "Agent tests on cell2".into(), ..Default::default() }
}

/// `all` less `c`: the fence that must refuse the step `c` lets through.
fn without(all: &[FenceClass], c: FenceClass) -> Vec<FenceClass> {
    all.iter().copied().filter(|x| *x != c).collect()
}

/// The one fence folder both builds share, the release build's signing key, and each build's
/// own config folder trusting that key (the dev build by hand: Settings › Fence › Trust).
struct Fences {
    base: PathBuf,
    dir: PathBuf,
    key: p256::ecdsa::SigningKey,
    file: fence::KeyFile,
}

impl Fences {
    fn new(base: &Path) -> Fences {
        let dir = base.join("SparkBox Fence");
        std::fs::create_dir_all(&dir).unwrap();
        let key = fence::new_key().unwrap();
        let (key_id, public_key) = fence::public_of(&key).unwrap();
        let f = Fences { base: base.into(), dir, key, file: fence::KeyFile { key_id, public_key, kind: KEY_KIND.into() } };
        for instance in ["release", "dev"] {
            fence::save_trust(&f.config(instance), &f.file).unwrap();
        }
        f
    }

    fn config(&self, instance: &str) -> PathBuf {
        self.base.join(format!("config-{instance}"))
    }

    /// The exit's rules for one build, as `Enforce::app` makes them, with these folders.
    fn rules(&self, instance: &str) -> Enforce {
        Enforce { instance: instance.into(), fence_dir: self.dir.clone(), config_dir: Some(self.config(instance)), noauth: false, tag: TAG.into(), mac: Some(MAC.into()) }
    }

    /// Opens `spec` as fence_open does (signed, written next to the key file), a moment after
    /// the last one so it's the newer.
    fn open(&self, spec: &Spec) -> Record {
        std::thread::sleep(Duration::from_millis(3));
        let r = fence::record_for(spec, &self.key, now_ms(), MAC).unwrap();
        fence::write(&self.dir, &self.config("release"), &self.file, &r).unwrap();
        r
    }
}

fn err(e: String) -> SbError {
    SbError::from(e)
}

/// The newest activity row on `node` whose command is exactly `command`.
fn row(live: &Live, node: &str, command: &str) -> audit::Entry {
    live.audit.entries().into_iter().find(|e| e.node == node && e.command == command).unwrap_or_else(|| panic!("no row on {node} for `{command}`: {:#?}", live.audit.entries().iter().map(|e| (&e.node, &e.command)).collect::<Vec<_>>()))
}

fn refusals(live: &Live) -> Vec<audit::Entry> {
    live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect()
}

/// The lease claim the exit writes to the log the first time this build takes a node.
fn claim(instance: &str) -> String {
    format!("python3 ~/.sparkbox/sparkjob.py lease claim '{instance}' '{TAG}' '{MAC}' 45")
}

/// Points `node` at another fake's port, keeping every other node as it is (Settings › Nodes).
fn moved(inv: &Inventory, node: &str, port: u16) -> Vec<NodeConfig> {
    inv.nodes.iter().map(|n| if n.id == node { NodeConfig { port, ..n.clone() } } else { n.clone() }).collect()
}

async fn wait_hostname(live: &Live, inv: &Inventory, node: &str, want: &str) {
    for _ in 0..200 {
        if live.snapshot(inv).nodes.iter().any(|n| n.id == node && n.status == LinkStatus::Online && n.system.hostname == want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{node} never answered as {want}");
}

/// Look-only and Admin mode as the test sets them (Settings › Security's look-only switch).
struct Policy {
    look_only: AtomicBool,
}

impl WritePolicy for Policy {
    fn look_only(&self) -> bool {
        self.look_only.load(Ordering::SeqCst)
    }

    fn admin_unlocked(&self) -> bool {
        true
    }
}

fn ops_for(live: &Arc<Live>, inv: &Inventory) -> Arc<Ops> {
    let ops = Ops::new(live.clone(), Arc::new(Mutex::new(crate::demo::DemoCluster::new(&Inventory::example()))), Tasks::new(None));
    let (l, i) = (live.clone(), inv.clone());
    ops.set_snapshot_source(move || Some(l.snapshot(&i)));
    ops
}

async fn fleet(ops: &Arc<Ops>, spec: FleetSpec) -> (String, Vec<crate::ops::FleetResult>) {
    let id = ops.fleet_start(false, spec).unwrap();
    for _ in 0..600 {
        if let Some(r) = ops.fleet_results(&id) {
            return (id, r);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the Fleet run {id} never finished");
}

fn spec(nodes: &[&str], command: &str, sudo: bool, timeout_s: u64) -> FleetSpec {
    FleetSpec { nodes: nodes.iter().map(|n| n.to_string()).collect(), command: command.into(), sudo, timeout_s, ..Default::default() }
}

/// Digits for this test run alone (its process and the moment it asks), so a process the fake
/// node starts on this Mac can't be confused with another run's.
fn run_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    format!("{}{nanos:09}", std::process::id())
}

/// The PID of the process on this Mac whose command line is exactly `cmd` (digits and spaces).
fn pgrep(cmd: &str) -> Option<u32> {
    let out = std::process::Command::new("pgrep").args(["-f", &format!("^{}$", cmd.replace('.', "\\."))]).output().unwrap();
    let pids: Vec<u32> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect();
    assert!(pids.len() <= 1, "more than one `{cmd}`: {pids:?}");
    pids.first().copied()
}

// ---- The two presets (step 17's and Session A's)

#[test]
fn the_two_presets_are_what_settings_opens() {
    let ids: Vec<String> = FOUR.map(String::from).to_vec();
    assert_eq!(SESSION_A.map(FenceClass::id), ["sudo-probe", "root-helper", "service", "fleet", "shell", "container-owner"]);
    assert_eq!(T2.map(FenceClass::id), ["container-test", "process-test", "files-test"]);
    fence::validate(&session_a(&SESSION_A), &ids).unwrap();
    fence::validate(&t2(&T2), &ids).unwrap();
    // The Touch ID sheet's words for each (Record::summary).
    let f = Fences::new(&temp("rehearsal-presets"));
    let now = now_ms();
    assert_eq!(fence::record_for(&session_a(&SESSION_A), &f.key, now, MAC).unwrap().summary(now), "cell1, cell2, cell3, gx10-0000 · 6 classes · 3 h");
    assert_eq!(fence::record_for(&t2(&T2), &f.key, now, MAC).unwrap().summary(now), "cell2 · 3 classes · 8 h");
    // The dev build only gets named classes.
    assert!(fence::validate(&t2(&[FenceClass::Any]), &ids).is_err());
}

/// Settings › Fence opens what src/lib/fence.ts says: the copy above can't drift from its text.
#[test]
fn the_t2_preset_is_fence_ts_text() {
    let ts = include_str!("../../../src/lib/fence.ts");
    let line = ts.lines().find(|l| l.contains("spec: { nodes: nodes.filter((n) => n === \"cell2\")")).expect("the cell2 preset in fence.ts");
    let spec = t2(&T2);
    let classes = T2.map(|c| format!("\"{}\"", c.id())).join(", ");
    for want in [format!("classes: [{classes}]"), "only: {}".to_string(), format!("minutes: {}", spec.minutes), format!("instance: \"{}\"", spec.instance), format!("note: \"{}\"", spec.note)] {
        assert!(line.contains(&want), "{want} isn't in fence.ts's preset: {line}");
    }
}

// ---- Before you start: what look-only wrote in ~/.sparkbox (owner-only, before the fence)

/// The listing Before you start asks Fleet for on the four nodes.
const FOOTPRINT: &str = "ls -laR --time-style=full-iso ~/.sparkbox";

#[tokio::test]
async fn before_you_start_sparkbox_s_folder_is_listed_with_no_fence_open() {
    let home = temp("rehearsal-footprint-home");
    let (node, port) = fake_spark(&home, "cell2").await;
    let app = temp("rehearsal-footprint-app");
    let (live, inv) = fake_node::connect_sparks(&app, &[("cell2", port)]).await;
    let ops = ops_for(&live, &inv);
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("release"));

    // No fence open: the exit lets the listing through as a typed read, with no fence class, and
    // claims no lease for it (a claim would write ~/.sparkbox/controller.json itself).
    let (id, _) = fleet(&ops, spec(&["cell2"], FOOTPRINT, false, 30)).await;
    let sent = crate::ops::fleet::wrap(FOOTPRINT, 30, false);
    let asked: Vec<(Verdict, bool, bool)> = live.admits().into_iter().filter(|a| a.cmd == sent).map(|a| (a.verdict, a.admitted, a.typed)).collect();
    assert_eq!(asked, [(Verdict::Read, true, true)]);
    assert_eq!(node.ran(FOOTPRINT).len(), 1);
    let r = row(&live, "cell2", &sent);
    assert_eq!((r.kind.as_str(), r.fence.as_deref(), r.class.as_str(), r.title.as_str(), r.op_id.as_deref()), ("stream", None, "change", "Fleet: ls -laR --time-style=full-iso ~/.sparkbox", Some(id.as_str())));
    assert!(refusals(&live).is_empty(), "{:#?}", refusals(&live));
    assert!(live.audit.entries().iter().all(|e| !e.command.contains("lease claim")));
    // (This Mac's BSD ls has no --time-style, so the fake can't print the listing; that's yours.)
    live.stop();
}

// ---- Step 4: the Connect check's sudo on cell2 first, then the root helper

#[tokio::test]
async fn step4_sudo_check_and_the_root_helper_on_cell2() {
    let home = temp("rehearsal-s4-home");
    let (node, port) = fake_spark(&home, "cell2").await;
    let app = temp("rehearsal-s4-app");
    let (live, inv) = fake_node::connect_sparks(&app, &[("cell2", port)]).await;
    let lib = app.join("rootlib").display().to_string();
    live.set_root_lib(&lib);
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("release"));
    // Settings › Nodes › cell2 › Test: the Connect wizard's checks on cell2 alone, with the saved
    // password (node_probe). Its sudo step is `sudo -k -S -p '' true` on the check's own connection.
    let cfg = inv.nodes[0].clone();
    let test = || async { crate::onboard::probe_node(&live, &cfg, Some("pw".into()), None, true).await.into_iter().find(|s| s.id == "sudo").unwrap() };
    let checks = || live.audit.entries().into_iter().filter(|e| e.node == "cell2" && e.command == "true").map(|e| (e.kind, e.title, e.state, e.fence)).collect::<Vec<_>>();
    let refused = || ("op".to_string(), "Refused".to_string(), "failed".to_string(), None);

    // No fence yet: the step isn't checked and says why, and nothing reached the node.
    let s = test().await;
    assert_eq!((s.status.as_str(), s.detail.as_str()), ("skip", "Not checked: no open fence allows sudo checks on cell2"));
    assert_eq!(s.hint.as_deref(), Some("No fence is open, so changes on the cluster are refused. This test build needs one: open it in the release SparkBox (Settings › Fence)."));
    f.open(&session_a(&without(&SESSION_A, SudoProbe)));
    assert_eq!(test().await.hint.as_deref(), Some("The fence doesn't allow sudo checks."));
    assert!(node.ran("sudo").is_empty(), "{:?}", node.ran("sudo"));
    assert_eq!(checks(), [refused(), refused()]);

    // The preset: the check is a sudo-probe, the password goes on sudo's stdin only, and it
    // writes its own row ("Connect check: sudo", `true`: the log's rule that leaves out `true`,
    // the latency ping, isn't how this row is written). Its admission takes cell2's lease first.
    f.open(&session_a(&SESSION_A));
    let s = test().await;
    assert_eq!((s.status.as_str(), s.detail.as_str()), ("ok", "Works with the cluster password"));
    let stdins = node.stdin.lock().unwrap().clone();
    let (cmd, input) = stdins.iter().find(|(c, _)| c.starts_with("sudo -k -S -p ''")).unwrap();
    assert_eq!((cmd.as_str(), input.as_slice()), ("sudo -k -S -p '' true", b"pw\n".as_slice()));
    let ok = || ("sudo".to_string(), "Connect check: sudo".to_string(), "ok".to_string(), Some("sudo-probe".to_string()));
    assert_eq!(checks(), [ok(), refused(), refused()]);
    let r = row(&live, "cell2", "true");
    assert_eq!((r.class.as_str(), r.escalated, r.changes), ("change", true, true));
    let lease = row(&live, "cell2", &claim("release"));
    assert_eq!((lease.kind.as_str(), lease.title.as_str(), lease.state.as_str(), lease.class.as_str(), lease.fence.as_deref()), ("command", "", "ok", "change", None));
    // Newest first: the check's row, then the claim its admission made.
    let at = |cmd: &str| live.audit.entries().iter().position(|e| e.command == cmd).unwrap();
    assert!(at("true") < at(&claim("release")), "the claim comes first");
    // The breaker stays quiet: a second check passes, one more row, and no new lease row.
    assert_eq!(test().await.status, "ok");
    assert_eq!(checks(), [ok(), ok(), refused(), refused()]);
    assert!(live.audit.entries().iter().all(|e| !e.error.as_deref().unwrap_or_default().contains("rejected")));
    assert_eq!(live.audit.entries().iter().filter(|e| e.command == claim("release")).count(), 1);

    // Files › Read with sudo… of the fabric netplan file (root's and 600). The fake's /etc is a
    // folder in its home, and its sudo has no real root, so the file is the cluster user's.
    let netplan = home.join("etc/netplan/40-cx7.yaml");
    std::fs::create_dir_all(netplan.parent().unwrap()).unwrap();
    std::fs::write(&netplan, "network:\n  version: 2\n  ethernets: {}\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&netplan, std::fs::Permissions::from_mode(0o600)).unwrap();
    let path = netplan.display().to_string();
    let files = Files::new(live.clone(), app.join("backups"));
    let sha = sha256_hex(SPARKJOB.as_bytes());
    let read = root_sparkjob_command(&lib, &sha, &format!("read {}", sh_quote(&path)));
    let install = root_install_command(&lib, &sha);
    let rows = |cmd: &str| live.audit.entries().into_iter().filter(|e| e.node == "cell2" && e.command == cmd && e.kind == "sudo").collect::<Vec<_>>();

    // Without root-helper the read is tried (root's copy is missing: exit 97) and the copy
    // into place is refused, so nothing is read.
    f.open(&session_a(&without(&SESSION_A, RootHelper)));
    let e = files.read_sudo(false, "cell2", &path).await.unwrap_err();
    assert!(e.contains("The fence doesn't allow installing SparkBox's root helper."), "{e}");
    assert!(!Path::new(&lib).join("sparkjob.py").exists());
    assert_eq!(refusals(&live).iter().filter(|r| r.command == install).count(), 1);

    // The preset: the read as root (sudo-probe) finds no copy, the copy goes in (root-helper),
    // the read runs again; root's copy is SparkBox's own, byte for byte.
    f.open(&session_a(&SESSION_A));
    let file = files.read_sudo(false, "cell2", &path).await.unwrap();
    assert_eq!((file.content.as_str(), file.writable), ("network:\n  version: 2\n  ethernets: {}\n", false));
    assert_eq!(sha256_hex(&std::fs::read(Path::new(&lib).join("sparkjob.py")).unwrap()), sha);
    let reads = rows(&read);
    let shown: Vec<(Option<&str>, &str, Option<u32>)> = reads.iter().map(|r| (r.fence.as_deref(), r.state.as_str(), r.code)).collect();
    assert_eq!(shown, [(Some("sudo-probe"), "ok", Some(0)), (Some("sudo-probe"), "failed", Some(97)), (Some("sudo-probe"), "failed", Some(97))], "newest first");
    let installed = rows(&install);
    assert_eq!(installed.len(), 1);
    assert_eq!((installed[0].fence.as_deref(), installed[0].state.as_str(), installed[0].escalated), (Some("root-helper"), "ok", true));
    assert!(reads.iter().chain(&installed).all(|r| r.title.is_empty() && r.class == "change" && r.escalated), "{reads:#?}");
    // Only the password went to sudo's stdin for the read; the copy followed it for the install.
    let stdins = node.stdin.lock().unwrap().clone();
    assert!(stdins.iter().filter(|(c, _)| c == "sudo -k -S -p '' true").all(|(_, s)| s == b"pw\n"));
    assert!(stdins.iter().filter(|(c, _)| c.contains("sparkjob.py' read")).all(|(_, s)| s == b"pw\n"));
    assert!(stdins.iter().any(|(c, s)| c.contains("mktemp") && s.starts_with(b"pw\n") && s.len() == 3 + SPARKJOB.len()));
    live.stop();
}

// ---- Steps 6 and 10: services on cell2

#[tokio::test]
async fn step6_and_step10_services_on_cell2() {
    let home = temp("rehearsal-s6-home");
    let (node, port) = fake_spark(&home, "cell2").await;
    let app = temp("rehearsal-s6-app");
    let (live, inv) = fake_node::connect_sparks(&app, &[("cell2", port)]).await;
    let ops = ops_for(&live, &inv);
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("release"));
    let act = |unit: &'static str, action: &'static str| {
        let ops = ops.clone();
        async move { ops.service_action(false, "cell2", unit, action).await }
    };

    f.open(&session_a(&without(&SESSION_A, Service)));
    assert_eq!(act("earlyoom.service", "restart").await.unwrap_err().message, "The fence doesn't allow service changes.");

    // The preset: restart earlyoom, disable and enable cups, daemon-reload; step 10's stop and
    // start of earlyoom. Each one is a sudo line its fence class names, under its task's title.
    f.open(&session_a(&SESSION_A));
    for (unit, action, title) in [
        ("earlyoom.service", "restart", "Restart earlyoom.service on cell2"),
        ("cups.service", "disable", "Disable cups.service on cell2"),
        ("cups.service", "enable", "Enable cups.service on cell2"),
        ("earlyoom.service", "stop", "Stop earlyoom.service on cell2"),
        ("earlyoom.service", "start", "Start earlyoom.service on cell2"),
    ] {
        act(unit, action).await.unwrap_or_else(|e| panic!("{action} {unit}: {}", e.message));
        let r = row(&live, "cell2", &format!("systemctl {action} -- '{unit}'"));
        assert_eq!((r.kind.as_str(), r.fence.as_deref(), r.title.as_str(), r.state.as_str(), r.escalated, r.class.as_str()), ("sudo", Some("service"), title, "ok", true, "change"), "{action} {unit}");
        // The task's own head line, with the same title and op, ends with its outcome.
        let head = live.audit.entries().into_iter().find(|e| e.kind == "op" && e.op_id == r.op_id).unwrap();
        assert_eq!((head.title.as_str(), head.command.as_str(), head.state.as_str()), (title, "", "ok"), "{action} {unit}");
    }
    // daemon-reload names no unit, so the `only` list doesn't narrow it.
    act("", "daemon-reload").await.unwrap();
    let r = row(&live, "cell2", "systemctl daemon-reload");
    assert_eq!((r.kind.as_str(), r.fence.as_deref(), r.title.as_str(), r.state.as_str()), ("sudo", Some("service"), "Reload the unit files on cell2", "ok"));
    assert!(live.audit.entries().iter().any(|e| e.kind == "op" && e.op_id == r.op_id && e.title == "Reload the unit files on cell2" && e.state == "ok"));
    let ran = std::fs::read_to_string(node.state_dir.join("systemctl.log")).unwrap();
    assert_eq!(ran.lines().collect::<Vec<_>>(), ["restart -- earlyoom.service", "disable -- cups.service", "enable -- cups.service", "start -- earlyoom.service", "daemon-reload"]);

    // Any other unit is outside the preset, and never reaches the node.
    let e = act("sshd.service", "restart").await.unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow service changes to sshd.service: it names only earlyoom.service, cups.service.");
    assert!(node.ran("sshd.service").is_empty());
    live.stop();
}

// ---- Steps 5, 7, 8, 8a and 9: sudo reads, ports, Fleet and shells on all four nodes

#[tokio::test]
async fn steps_5_7_8_and_9_on_all_four_nodes() {
    let mut nodes = Vec::new();
    for n in FOUR {
        nodes.push((n, fake_spark(&temp(&format!("rehearsal-four-{n}")), n).await));
    }
    let app = temp("rehearsal-four-app");
    let ports: Vec<(&str, u16)> = nodes.iter().map(|(n, (_, p))| (*n, *p)).collect();
    let (live, inv) = fake_node::connect_sparks(&app, &ports).await;
    let ops = ops_for(&live, &inv);
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("release"));
    let cell2 = &nodes[1].1 .0;
    // The fake's sudo has no real root: `id` answers as root's does under it.
    cell2.bin("id", "if [ \"$SB_FAKE_ROOT\" = 1 ]; then echo 'uid=0(root) gid=0(root) groups=0(root)'; else exec /usr/bin/id \"$@\"; fi\n");
    let terminals = Terminals::new(live.clone());
    type Seen = Arc<Mutex<String>>;
    let tab = |seen: &Seen| -> crate::term::Emit {
        let s = seen.clone();
        Box::new(move |e| {
            if let TermEvent::Data { data } = e {
                use base64::Engine;
                s.lock().unwrap().push_str(&String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(data).unwrap()));
            }
        })
    };

    // Each class refused without it: Fleet as root, a shell.
    f.open(&session_a(&without(&SESSION_A, Fleet)));
    let (_, r) = fleet(&ops, spec(&["cell2"], "id", true, 30)).await;
    assert!(r[0].error.as_deref().is_some_and(|e| e.contains("The fence doesn't allow Fleet commands.")), "{r:?}");
    f.open(&session_a(&without(&SESSION_A, Shell)));
    let e = terminals.open(false, "cell2", Target::Shell, 80, 24, tab(&Seen::default())).await.unwrap_err();
    assert_eq!(err(e).message, "The fence doesn't allow shells.");
    let failed = live.audit.entries().into_iter().find(|e| e.kind == "terminal").unwrap();
    assert_eq!((failed.title.as_str(), failed.state.as_str()), ("Open a shell on cell2", "failed"));

    f.open(&session_a(&SESSION_A));

    // Step 5: a read with sudo runs on every node (the preset names all four for sudo-probe).
    let journal = "journalctl -u ollama.service -n 25 --no-pager";
    for n in FOUR {
        let out = live.sudo(n, journal, Duration::from_secs(20)).await.unwrap();
        assert_eq!(out.code, Some(0), "{n}: {out:?}");
        let r = row(&live, n, journal);
        assert_eq!((r.kind.as_str(), r.fence.as_deref(), r.escalated), ("sudo", Some("sudo-probe"), true), "{n}");
    }
    // The doctor's sudo line, `sshd -T`, isn't a read the classifier knows, so on its own it's
    // `other`: the doctor's call has to carry `sudo-probe` itself (R2-CONN).
    let e = err(live.sudo("cell1", "sshd -T", Duration::from_secs(20)).await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow this change.");

    // Step 7: Reveal with sudo on cell1's ports (`sudo ss -tlnpH`, a read run as root).
    let listeners = ops.reveal_ports(false, "cell1").await.unwrap();
    assert!(listeners.iter().any(|l| l.port == 8080), "{listeners:?}");
    let r = row(&live, "cell1", "ss -tlnpH");
    assert_eq!((r.kind.as_str(), r.fence.as_deref()), ("sudo", Some("sudo-probe")));

    // Step 8: `uptime` on all four is a read (no fence class), still in the Changes view as typed.
    let (id, results) = fleet(&ops, spec(&FOUR, "uptime", false, 30)).await;
    assert_eq!(results.iter().map(|r| (r.node.as_str(), r.state.as_str())).collect::<Vec<_>>(), FOUR.map(|n| (n, "ok")));
    for n in FOUR {
        let r = row(&live, n, "timeout --signal=TERM --kill-after=5s 30s sh -c 'uptime'");
        assert_eq!((r.kind.as_str(), r.fence.as_deref(), r.class.as_str(), r.title.as_str(), r.op_id.as_deref(), r.state.as_str()), ("stream", None, "change", "Fleet: uptime", Some(id.as_str()), "ok"), "{n}");
    }
    // Step 3's `df -B1 /` on gx10-0000 is a read the same way: it runs with no fence class.
    // (This Mac's BSD df has no -B, so the fake can't give the number; that reading is yours.)
    fleet(&ops, spec(&["gx10-0000"], "df -B1 /", false, 30)).await;
    assert_eq!(nodes[3].1 .0.ran("df -B1 /").len(), 1);
    assert_eq!(row(&live, "gx10-0000", "timeout --signal=TERM --kill-after=5s 30s sh -c 'df -B1 /'").fence, None);
    // `id` as root on cell2: a Fleet change, run through sudo.
    let (id, results) = fleet(&ops, spec(&["cell2"], "id", true, 30)).await;
    assert!(results[0].state == "ok" && results[0].stdout.starts_with("uid=0(root)"), "{results:?}");
    let r = row(&live, "cell2", "env LC_ALL=C timeout --signal=TERM --kill-after=5s 30s sh -c 'id'");
    assert_eq!((r.kind.as_str(), r.fence.as_deref(), r.escalated, r.title.as_str(), r.op_id.as_deref(), r.state.as_str()), ("sudo", Some("fleet"), true, "Fleet: id (as root)", Some(id.as_str()), "ok"));
    // `sleep 120` with a 5 s limit: stopped on the node at the limit, nothing left running. (The
    // fake nodes are this Mac, so this run's sleep carries a fraction of its own: pgrep finds
    // it and no other run's.)
    let sleep = format!("sleep 120.{}", run_id());
    let (_, results) = fleet(&ops, spec(&["cell2"], &sleep, false, 5)).await;
    assert_eq!((results[0].state.as_str(), results[0].code, results[0].timed_out), ("timedOut", Some(124), true), "{results:?}");
    assert_eq!(cell2.ran(&sleep).len(), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(pgrep(&sleep), None, "the node stopped it");

    // Step 8a, typed but not run: what Fleet's confirmation is built from.
    use crate::ops::classify::{verdict, Kind, Power};
    let v = verdict("docker run --rm -v /:/host alpine true");
    assert!(v.root && v.kind == Kind::Change, "{v:?}");
    let v = verdict("sudo chroot / reboot");
    assert!(v.power == Some(Power::Restart) && v.reason.as_deref().is_some_and(|r| r.starts_with("Takes the node down")), "{v:?}");
    assert_eq!(verdict("curl -o/tmp/x http://127.0.0.1").kind, Kind::Change);

    // Step 9: a shell on each node, `hostname` typed into all four at once.
    let mut tabs = Vec::new();
    for n in FOUR {
        let seen = Seen::default();
        let id = terminals.open(false, n, Target::Shell, 80, 24, tab(&seen)).await.unwrap();
        terminals.authorize_shell(&id).unwrap();
        let r = live.audit.entries().into_iter().find(|e| e.kind == "terminal" && e.node == n && e.state == "running").unwrap();
        assert_eq!((r.title, r.command, r.class, r.fence, r.changes), (format!("Open a shell on {n}"), format!("ssh {n} login shell"), "session".to_string(), None, false));
        tabs.push((n, id, seen));
    }
    let ids: Vec<String> = tabs.iter().map(|(_, id, _)| id.clone()).collect();
    terminals.note_broadcast(false, true, &ids);
    let b = live.audit.entries().into_iter().find(|e| e.title.starts_with("Broadcast")).unwrap();
    assert_eq!((b.node.as_str(), b.kind.as_str(), b.title.as_str(), b.command.as_str(), b.class.as_str(), b.changes), ("mac", "app", "Broadcast typing to 4 shells", "cell1, cell2, cell3, gx10-0000", "session", false));
    for (_, id, _) in &tabs {
        terminals.input(id, b"hostname\r".to_vec()).unwrap();
    }
    for (n, _, seen) in &tabs {
        let want = if *n == "gx10-0000" { "gx10-0000".to_string() } else { format!("{n}-example") };
        let mut found = false;
        for _ in 0..200 {
            // The shell echoes what was typed, then prints the answer (after its prompt).
            if seen.lock().unwrap().lines().any(|l| l.trim().trim_start_matches("$ ") == want) {
                found = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(found, "{n}: {:?}", seen.lock().unwrap());
    }
    terminals.note_broadcast(false, false, &ids);
    let stop = live.audit.entries().remove(0);
    assert_eq!((stop.node.as_str(), stop.kind.as_str(), stop.title.as_str(), stop.command.as_str(), stop.class.as_str()), ("mac", "app", "Stop broadcast typing", "cell1, cell2, cell3, gx10-0000", "session"));
    for id in &ids {
        terminals.close(id);
    }
    live.stop();
}

// ---- Step 11: cell2's entry pointed at cell3

#[tokio::test]
async fn step11_cell2_answering_as_cell3_is_refused_until_reverted() {
    step11("s11", true).await;
}

/// Step 11 again with a machine-id of each node's own, as gx10-0000 has: the pin holds it, and
/// the revert is admitted. (Round 2 once saw this revert refused, while the identity caches were
/// keyed by the connection's address; that address isn't reused on every run, so this can pass
/// with such caches too.)
#[tokio::test]
async fn step11_with_their_own_machine_ids_the_revert_is_admitted_too() {
    step11("s11-own", false).await;
}

async fn step11(name: &str, shared_machine_id: bool) {
    let (h2, h3) = (temp(&format!("rehearsal-{name}-cell2")), temp(&format!("rehearsal-{name}-cell3")));
    let (_n2, p2) = fake_spark(&h2, "cell2").await;
    let (n3, p3) = fake_spark(&h3, "cell3").await;
    // cell1–cell3 share one machine-id on the cluster (round 1's Live pass), so their pins hold
    // hostname and LAN MAC only; the fake's stand-in gives each Spark its own.
    if shared_machine_id {
        for h in [&h2, &h3] {
            std::fs::write(PathBuf::from(format!("{}.spark/root/etc/machine-id", h.display())), "295f5139c0de4e2a9b7d1f3a5c6e8b90\n").unwrap();
        }
    }
    let app = temp(&format!("rehearsal-{name}-app"));
    let (live, inv) = fake_node::connect_sparks(&app, &[("cell2", p2), ("cell3", p3)]).await;
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("release"));
    f.open(&session_a(&SESSION_A));
    let restart = || live.sudo("cell2", "systemctl restart -- 'earlyoom.service'", Duration::from_secs(30));
    // The first change pins cell2 (hostname and LAN MAC from its frame, and its machine-id when
    // no other node shares it).
    restart().await.unwrap();
    let pin = super::identity_pin::load(&f.config("release")).remove("cell2").unwrap();
    assert_eq!((pin.hostname.as_str(), pin.machine_id.is_some(), pin.cloned_machine_id), ("cell2-example", !shared_machine_id, shared_machine_id));

    // cell3 answers on cell2's address, with the host key cell1–cell3 share.
    live.start(&moved(&inv, "cell2", p3));
    wait_hostname(&live, &inv, "cell2", "cell3-example").await;
    let e = err(restart().await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::WrongNode, "cell2 answers as cell3-example: changes refused. If you moved or rebuilt it, re-pin it in Settings › Fence."));
    assert!(n3.ran("systemctl restart").is_empty(), "nothing reached cell3");
    let refused = refusals(&live).into_iter().find(|r| r.node == "cell2").unwrap();
    assert_eq!((refused.kind.as_str(), refused.state.as_str(), refused.command.as_str(), refused.fence.as_deref(), refused.changes), ("op", "failed", "systemctl restart -- 'earlyoom.service'", None, true));
    // Settings › Fence shows cell2 not matching its pin; reads keep working.
    let view = live.exit.pins.view(&live, &f.config("release"), &[("cell2".into(), "cell2".into())]).await;
    assert_eq!((view[0].matches, view[0].now.as_ref().map(|n| n.hostname.as_str())), (Some(false), Some("cell3-example")));
    assert_eq!(live.run("cell2", "uptime", None, Duration::from_secs(10)).await.unwrap().code, Some(0));

    // Reverted: cell2 answers as itself, and changes go through again without a re-pin.
    live.start(&inv.nodes);
    wait_hostname(&live, &inv, "cell2", "cell2-example").await;
    restart().await.unwrap();
    live.stop();
}

// ---- Containers on the fake nodes (steps 16 and T2)

/// A GB10's Docker, which the fake Mac doesn't have: `run` prints the new container's id,
/// `start|stop|restart|kill|rm` print what they were given, as docker does.
fn docker() -> Script {
    Arc::new(|cmd: &str, _: &[u8]| {
        let w: Vec<&str> = cmd.split_whitespace().collect();
        match w.as_slice() {
            ["docker", "run", ..] => Some((format!("{BUSYBOX}\n"), 0)),
            ["docker", "start" | "stop" | "restart" | "kill" | "rm", .., last] => Some((format!("{last}\n"), 0)),
            _ => None,
        }
    })
}

const BUSYBOX: &str = "5b0c7e1f2a3d4c5b6a7980f1e2d3c4b5a69788776655443322110ffeeddccbbaa";
const GLM53: &str = "2e35a6ef41260f6d0b8c3f6a5e4d3c2b1a09f8e7d6c5b4a3928170615243f5e6";
const GEMMA_RELEASE: &str = "0123456789abcdef00112233445566778899aabbccddeeff0011223344556677";
const TEST_RELEASE: &str = "fedcba9876543210ffeeddccbbaa99887766554433221100ffeeddccbbaa9988";
/// The T2 test container, as T2 runs it on cell2.
const RUN_BUSYBOX: &str = "docker run -d --name sparkbox-test-busybox --label sparkbox.test=1 --label sparkbox.instance=dev busybox sleep 600";

fn container(id: &str, name: &str, state: &str, labels: Value) -> Value {
    json!({ "id": id, "name": name, "image": "busybox", "state": state, "startedAt": 1, "finishedAt": 0, "exitCode": 0,
            "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "sleep 600", "labels": labels })
}

/// What the node's collector reports: these containers in every frame from now on.
fn show(node: &Node, containers: Vec<Value>) {
    let mut frame: Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    frame["containers"] = Value::Array(containers);
    *node.knobs.frame.lock().unwrap() = Some(frame.to_string());
}

async fn wait_container(live: &Live, inv: &Inventory, node: &str, name: &str, present: bool) {
    for _ in 0..200 {
        let snap = live.snapshot(inv);
        if snap.nodes.iter().any(|n| n.id == node && n.containers.iter().any(|c| c.name == name) == present) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{name} on {node}: never {}", if present { "there" } else { "gone" });
}

// ---- Steps 13 and 14 (and 17): the two builds, one fence folder, the lease

/// The debug build's Files › cell2 › ~/sparkbox-test › notes.txt › Edit, then Save over what it
/// opened (the editor's save: `sparkjob.py commit … --expect <the opened sha256>`).
async fn edit_and_save(files: &Files, path: &str, text: &str) -> Result<(), SbError> {
    let opened = files.read(false, "cell2", path).await.unwrap();
    files.write(false, "cell2", path, text, false, Some(&opened.sha256)).await.map(drop).map_err(|e| match e {
        crate::files::SaveError::Failed { message } => err(message),
        other => panic!("{other:?}"),
    })
}

#[tokio::test]
async fn steps_13_14_and_17_two_builds_share_one_fence_folder() {
    let home = temp("rehearsal-s13-home");
    let (node, port) = fake_node(&home, fake_node::real_shell()).await;
    // ~/sparkbox-test/notes.txt, as step 9 makes it in cell2's shell.
    std::fs::create_dir_all(home.join("sparkbox-test")).unwrap();
    let notes = home.join("sparkbox-test/notes.txt");
    std::fs::write(&notes, "made in step 9\n").unwrap();
    let path = notes.display().to_string();
    let app = temp("rehearsal-s13-app");
    let (release, _) = fake_node::connect(&app.join("release"), &[("cell2", port)]).await;
    let (dev, _) = fake_node::connect(&app.join("dev"), &[("cell2", port)]).await;
    let f = Fences::new(&app);
    release.enforce_writes(f.rules("release"));
    dev.enforce_writes(f.rules("dev"));
    let files = Files::new(dev.clone(), app.join("dev-backups"));
    let text = || std::fs::read_to_string(&notes).unwrap();
    // The claim exit::start's loop makes every 15 s while the release build is unlocked or its
    // own fence is open (the loop is in the Tauri start hook, so the test makes the call itself).
    let who = super::lease::Who { instance: "release".into(), tag: TAG.into(), mac: MAC.into() };
    let renew = || release.exit.leases.claim(&release, "cell2", &who);
    let claims = |live: &Live, instance: &str| live.audit.entries().iter().filter(|e| e.command == claim(instance)).count();

    // Step 13a, as Session A runs it: the release build's fence is open and it holds cell2's
    // lease. The debug build's save is refused by the fence before the lease is asked.
    f.open(&session_a(&SESSION_A));
    renew().await.unwrap();
    assert_eq!(row(&release, "cell2", &claim("release")).kind, "command");
    let e = edit_and_save(&files, &path, "13a\n").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "This fence is for the release build."));
    let refused: Vec<(String, String, Option<String>)> = refusals(&dev).into_iter().map(|r| (r.kind, r.state, r.fence)).collect();
    assert_eq!(refused, [("op".to_string(), "failed".to_string(), None)]);
    assert!(refusals(&dev)[0].command.starts_with(&format!("python3 ~/.sparkbox/sparkjob.py commit {} --sha256 ", sh_quote(&path))), "{}", refusals(&dev)[0].command);
    assert_eq!(refusals(&dev)[0].error.as_deref(), Some("Save notes.txt: This fence is for the release build."));
    assert_eq!(text(), "made in step 9\n");
    // A relock lets go of the lease here only (no command to the node), so the next claim, which
    // the loop makes within 15 s while the fence stays open, writes its row again.
    super::exit::release_on_relock(&release, crate::admin::RelockReason::Explicit).await;
    assert!(release.exit.leases.held().is_empty());
    renew().await.unwrap();
    assert_eq!(claims(&release, "release"), 2);

    // Step 17c: the T2 fence is the dev build's. While the release build holds cell2's lease
    // (unlocked, it renews it every 15 s), the debug build's save names it.
    f.open(&t2(&T2));
    let view = dev.exit.fence_view(&dev);
    assert!(view.state == fence::State::Valid && view.mine && view.summary.as_deref().is_some_and(|s| s.starts_with("cell2 · 3 classes · ")), "{view:?}");
    // A renewal, once the 5 s answer is stale, asks the node again and writes no row.
    tokio::time::sleep(Duration::from_millis(5200)).await;
    renew().await.unwrap();
    assert_eq!(claims(&release, "release"), 2);
    // Step 13b.
    let e = edit_and_save(&files, &path, "13b\n").await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "SparkBox (release) is controlling cell2 right now."));
    assert!(refusals(&dev)[0].command.starts_with("python3 ~/.sparkbox/sparkjob.py commit "));
    assert_eq!(text(), "made in step 9\n", "nothing reached the node");
    assert_eq!(claims(&dev, "dev"), 0);
    // The release build locks: it stops renewing, its heartbeat ages past 45 s, and once the debug
    // build's 5 s answer is stale the lease is free.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    std::fs::write(home.join(".sparkbox/controller.json"), json!({ "instance": "release", "tag": TAG, "mac": MAC, "heartbeat": now - 60.0 }).to_string()).unwrap();
    dev.exit.leases.forget();
    edit_and_save(&files, &path, "13b\n").await.unwrap();
    assert_eq!(text(), "13b\n");
    let saved = dev.audit.entries().into_iter().find(|e| e.command.starts_with(&format!("python3 ~/.sparkbox/sparkjob.py commit {} --sha256 ", sh_quote(&path)))).unwrap();
    assert_eq!((saved.kind.as_str(), saved.title.as_str(), saved.fence.as_deref(), saved.state.as_str(), saved.class.as_str()), ("command", "Save notes.txt on cell2", Some("files-test"), "ok", "change"));
    let lease = row(&dev, "cell2", &claim("dev"));
    assert_eq!((lease.kind.as_str(), lease.state.as_str(), lease.class.as_str()), ("command", "ok", "change"));
    assert_eq!(claims(&dev, "dev"), 1);
    // The release build is refused now: the fence is the dev build's.
    let e = err(release.sudo("cell2", "systemctl restart -- 'earlyoom.service'", Duration::from_secs(10)).await.unwrap_err());
    assert_eq!(e.message, "This fence is for the dev build.");

    // Step 14: the T2 record edited by hand to add gx10-0000. Both builds refuse, and say so.
    let record = f.dir.join(fence::RECORD);
    let original = std::fs::read(&record).unwrap();
    let mut edited: Value = serde_json::from_slice(&original).unwrap();
    edited["nodes"] = json!(["cell2", "gx10-0000"]);
    std::fs::write(&record, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
    // A refused Files save's line starts with what it would have done.
    let failed = |live: &Live, e: SbError, words: &str| {
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence record failed its check: open a new one."));
        assert_eq!(live.exit.fence_view(live).state, fence::State::Failed);
        assert_eq!(refusals(live)[0].error.as_deref(), Some(format!("{words}The fence record failed its check: open a new one.").as_str()));
    };
    failed(&release, err(release.sudo("cell2", "systemctl restart -- 'earlyoom.service'", Duration::from_secs(10)).await.unwrap_err()), "");
    failed(&dev, edit_and_save(&files, &path, "14\n").await.unwrap_err(), "Save notes.txt: ");
    // Putting the old file back stays refused: each build remembers the failed record.
    std::fs::write(&record, &original).unwrap();
    failed(&release, err(release.sudo("cell2", "systemctl restart -- 'earlyoom.service'", Duration::from_secs(10)).await.unwrap_err()), "");
    failed(&dev, edit_and_save(&files, &path, "14\n").await.unwrap_err(), "Save notes.txt: ");
    assert_eq!(text(), "13b\n");
    assert!(node.ran("systemctl").is_empty() && node.ran("sudo").is_empty());
    // Rollback: a new fence (Agent tests on cell2 again) opens it.
    f.open(&t2(&T2));
    edit_and_save(&files, &path, "after 14\n").await.unwrap();
    assert_eq!(text(), "after 14\n");
    release.stop();
    dev.stop();
}

// ---- Step 14a: look-only

#[tokio::test]
async fn step14a_look_only_holds_every_change() {
    let home = temp("rehearsal-s14a-home");
    let (node, port) = fake_spark(&home, "cell2").await;
    let app = temp("rehearsal-s14a-app");
    let (live, inv) = fake_node::connect_sparks(&app, &[("cell2", port)]).await;
    let ops = ops_for(&live, &inv);
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("release"));
    f.open(&session_a(&SESSION_A));
    let policy = Arc::new(Policy { look_only: AtomicBool::new(true) });
    live.set_write_policy(policy.clone());
    let before = live.audit.entries().len();

    // The window never sends Fleet's Run in look-only (src/test/look-only.test.tsx), and the
    // command itself is refused before it runs: fleet_start isn't one of the reads.
    let passes = |c: &str| (crate::admin::READ_ONLY.iter().any(|(n, _)| *n == c) && !crate::admin::LOOK_ONLY_HOLDS.iter().any(|(n, _)| *n == c)) || crate::admin::LOOK_ONLY_CONDITIONAL.iter().any(|(n, _)| *n == c);
    for refused in ["fleet_start", "services_apply", "save_inventory", "containers_bulk", "files_write"] {
        assert!(!passes(refused), "{refused}");
    }
    // Every change is refused at the exit too: as root, a typed change, a service, a shell, a
    // container, a read with sudo, a file.
    let (_, r) = fleet(&ops, spec(&["cell2"], "id", true, 30)).await;
    assert!(r[0].error.as_deref().is_some_and(|e| e.contains("look-only")), "{r:?}");
    let (_, r) = fleet(&ops, spec(&["cell2"], "touch /tmp/sparkbox-rehearsal-look-only", false, 30)).await;
    assert!(r[0].error.as_deref().is_some_and(|e| e.contains("look-only")), "{r:?}");
    assert_eq!(ops.service_action(false, "cell2", "earlyoom.service", "restart").await.unwrap_err().kind, ErrorKind::LookOnly);
    assert!(live.pty("cell2", 80, 24, None).await.is_err());
    assert_eq!(err(live.container_action("cell2", "2e35a6ef4126", "restart").await.unwrap_err()).kind, ErrorKind::LookOnly);
    assert_eq!(err(live.sudo("cell2", "sshd -T", Duration::from_secs(10)).await.unwrap_err()).kind, ErrorKind::LookOnly);
    assert_eq!(err(live.sparkjob("cell2", "rm '~/sparkbox-test/x'", None, Duration::from_secs(10)).await.unwrap_err()).kind, ErrorKind::LookOnly);
    // A typed read passes the exit (it changes nothing); only the window and the command gate
    // keep Run off.
    let (_, r) = fleet(&ops, spec(&["cell2"], "uptime", false, 30)).await;
    assert_eq!(r[0].state, "ok");

    // Nothing that changes anything ran: every new change row is a refusal, failed.
    let new: Vec<audit::Entry> = live.audit.entries().into_iter().take(live.audit.entries().len() - before).collect();
    let ran: Vec<&audit::Entry> = new.iter().filter(|e| e.changes && e.title != "Refused" && e.kind != "op").filter(|e| e.command != "timeout --signal=TERM --kill-after=5s 30s sh -c 'uptime'").collect();
    assert!(ran.is_empty(), "{ran:#?}");
    assert_eq!(refusals(&live).len(), 7, "{:#?}", refusals(&live).iter().map(|r| &r.command).collect::<Vec<_>>());
    assert!(refusals(&live).iter().all(|r| r.state == "failed" && r.error.as_deref().is_some_and(|e| e.contains("look-only"))));
    for cmd in ["sudo", "systemctl", "touch", "docker", "sparkjob.py rm"] {
        assert!(node.ran(cmd).is_empty(), "{cmd}: {:?}", node.ran(cmd));
    }
    live.stop();
}

// ---- Step 16 (optional): gx10-0000's four old containers

#[tokio::test]
async fn step16_the_owners_old_containers_on_gx10() {
    let (node, port) = fake_node(&temp("rehearsal-s16-home"), docker()).await;
    // gx10-0000's four exited containers from the tour (live/fixtures/doctor/gx10-0000).
    let old = [
        ("22e0f93fdd26b1b2c3d4e5f60718293a4b5c6d7e8f9011223344556677889900", "vigilant_hofstadter"),
        ("be055f9ffc73a1b2c3d4e5f60718293a4b5c6d7e8f9011223344556677889900", "adoring_northcutt"),
        ("0f42a56925baa1b2c3d4e5f60718293a4b5c6d7e8f9011223344556677889900", "jovial_elgamal"),
        ("40a05cea602da1b2c3d4e5f60718293a4b5c6d7e8f9011223344556677889900", "gracious_babbage"),
    ];
    show(&node, old.iter().map(|(id, name)| container(id, name, "exited", json!({}))).collect());
    let app = temp("rehearsal-s16-app");
    let (live, inv) = fake_node::connect(&app, &[("gx10-0000", port)]).await;
    wait_container(&live, &inv, "gx10-0000", "gracious_babbage", true).await;
    let ops = ops_for(&live, &inv);
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("release"));
    f.open(&session_a(&without(&SESSION_A, ContainerOwner)));
    assert_eq!(ops.container_action(false, "gx10-0000", old[0].0, "remove", false).await.unwrap_err().message, "The fence doesn't allow container changes.");

    f.open(&session_a(&SESSION_A));
    for (id, name) in old {
        ops.container_action(false, "gx10-0000", id, "remove", false).await.unwrap_or_else(|e| panic!("{name}: {}", e.message));
        let r = row(&live, "gx10-0000", &format!("docker rm {id}"));
        assert_eq!((r.kind.as_str(), r.fence.as_deref(), r.state.as_str(), r.title.clone()), ("command", Some("container-owner"), "ok", format!("Remove {name} on gx10-0000")), "{name}");
        let head = live.audit.entries().into_iter().find(|e| e.kind == "op" && e.op_id == r.op_id).unwrap();
        assert_eq!((head.title, head.state), (format!("Remove {name} on gx10-0000"), "ok".to_string()), "{name}");
    }
    assert_eq!(node.ran("docker rm").len(), 4, "one at a time, one each");
    live.stop();
}

// ---- T2 (step 17's fence, the dev build): test containers, processes, files on cell2

#[tokio::test]
async fn t2_the_test_container_on_cell2() {
    let (node, port) = fake_node(&temp("rehearsal-t2c-home"), docker()).await;
    let others = || {
        vec![
            container(GLM53, "glm53", "exited", json!({})),
            container(GEMMA_RELEASE, "gemma-release", "running", json!({ "sparkbox.instance": "release", "sparkbox.deployment": "d1" })),
            container(TEST_RELEASE, "sparkbox-test-release", "running", json!({ "sparkbox.test": "1", "sparkbox.instance": "release" })),
        ]
    };
    show(&node, others());
    let app = temp("rehearsal-t2c-app");
    let (live, inv) = fake_node::connect(&app, &[("cell2", port)]).await;
    wait_container(&live, &inv, "cell2", "gemma-release", true).await;
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("dev"));

    f.open(&t2(&without(&T2, ContainerTest)));
    assert_eq!(err(live.run("cell2", RUN_BUSYBOX, None, Duration::from_secs(30)).await.unwrap_err()).message, "The fence doesn't allow test containers.");
    assert!(node.ran("docker run").is_empty());

    // Run, stop and remove sparkbox-test-busybox: each a container-test change.
    f.open(&t2(&T2));
    let out = live.run("cell2", RUN_BUSYBOX, None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(out.stdout.trim(), BUSYBOX);
    assert_eq!(row(&live, "cell2", RUN_BUSYBOX).fence.as_deref(), Some("container-test"));
    assert_eq!(row(&live, "cell2", &claim("dev")).kind, "command");
    show(&node, [others(), vec![container(BUSYBOX, "sparkbox-test-busybox", "running", json!({ "sparkbox.test": "1", "sparkbox.instance": "dev" }))]].concat());
    wait_container(&live, &inv, "cell2", "sparkbox-test-busybox", true).await;
    for action in ["stop", "remove"] {
        live.container_action("cell2", BUSYBOX, action).await.unwrap();
        let verb = if action == "remove" { "rm" } else { action };
        assert_eq!(row(&live, "cell2", &format!("docker {verb} {BUSYBOX}")).fence.as_deref(), Some("container-test"), "{action}");
    }
    assert_eq!(node.ran("docker").len(), 3, "{:?}", node.ran("docker"));

    // Not under T2: the owner's glm53, a load, and the release build's own test container.
    let e = err(live.container_action("cell2", GLM53, "restart").await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow container changes.");
    let e = err(live.container_action("cell2", GEMMA_RELEASE, "stop").await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow model unloads.");
    let e = err(live.container_action("cell2", TEST_RELEASE, "stop").await.unwrap_err());
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::OtherController, "sparkbox-test-release on cell2 is managed by the other SparkBox (release)."));
    // GAP: the same `docker run` typed into Fleet is a Fleet command, which T2 doesn't allow,
    // and no window sends it untyped (P1 has no "run a container"): T2 can't start the
    // container through SparkBox until a typed test-container shape keeps its class.
    let e = err(audit::as_typed(live.stream_cmd("cell2", RUN_BUSYBOX, Duration::from_secs(30), CancelToken::new(), |_, _| {})).await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow Fleet commands.");
    assert_eq!(node.ran("docker run").len(), 1);
    live.stop();
}

/// A `/proc/<pid>/stat` line as Linux writes it for `sleep`, with `start` as field 22.
fn stat_line(pid: u32, start: u64) -> String {
    let mut fields: Vec<String> = vec![pid.to_string(), "(sleep)".into(), "S".into()];
    fields.extend((4..=52).map(|i| match i {
        4..=6 => "1".to_string(),
        18 => "20".into(),
        19 => "10".into(),
        20 => "1".into(),
        22 => start.to_string(),
        23 => "5652480".into(),
        24 => "230".into(),
        _ => "0".into(),
    }));
    fields.join(" ") + "\n"
}

#[tokio::test]
async fn t2_a_process_on_cell2() {
    let home = temp("rehearsal-t2p-home");
    let (_node, port) = fake_node(&home, fake_node::real_shell()).await;
    let app = temp("rehearsal-t2p-app");
    let (live, inv) = fake_node::connect(&app, &[("cell2", port)]).await;
    let ops = ops_for(&live, &inv);
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("dev"));
    f.open(&t2(&T2));

    // `sleep 600` through Fleet: a read, so T2 runs it. (This one sleeps 600 s and a fraction of
    // this run's own, so it can't be mistaken for anything else on this Mac, another run of this
    // test included.)
    let sleep = format!("sleep 600.{}", run_id());
    let id = ops.fleet_start(false, spec(&["cell2"], &sleep, false, 900)).unwrap();
    let mut pid = None;
    for _ in 0..100 {
        pid = pgrep(&sleep);
        if pid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid = pid.expect("the Fleet run's sleep started");
    let alive = || std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().unwrap().success();
    let nice = || String::from_utf8_lossy(&std::process::Command::new("ps").args(["-o", "nice=", "-p", &pid.to_string()]).output().unwrap().stdout).trim().parse::<i32>().unwrap();
    // What the sleep started with: 0, or 5 when the test itself runs as a zsh background job.
    let before = nice();

    // Renice to 10: process-test, refused without it.
    f.open(&t2(&without(&T2, ProcessTest)));
    assert_eq!(ops.renice(false, "cell2", pid, 10).await.unwrap_err().message, "The fence doesn't allow process signals.");
    assert_eq!(nice(), before);
    f.open(&t2(&T2));
    ops.renice(false, "cell2", pid, 10).await.unwrap();
    // This Mac's `renice -n` adds to the nice value (at most 20).
    assert_eq!(nice(), (before + 10).min(20));
    let r = row(&live, "cell2", &format!("renice -n 10 -p {pid}"));
    assert_eq!((r.fence.as_deref(), r.title.clone(), r.state.as_str()), (Some("process-test"), format!("Set the priority of process {pid} on cell2 to 10"), "ok"));

    // TERM from Workloads (Ops::signal) goes through the PID guard. The guard is several
    // commands, so its shape alone is `other`; Ops::signal tags its call `process-test`. This
    // Mac has no /proc, so there the guard finds no such process and leaves it alone.
    let start = 98_765_432;
    let e = ops.signal(false, "cell2", pid, "TERM", Some(start)).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Gone, "{}", e.message);
    let r = row(&live, "cell2", &crate::ops::pid_guard_cmd(pid, "TERM", Some(start), "/proc"));
    // The fake's frame doesn't list the sleep, so the title names it by its PID.
    assert_eq!((r.fence.as_deref(), r.title.clone()), (Some("process-test"), format!("Send SIGTERM to process {pid} ({pid}) on cell2")));
    assert!(alive(), "left alone");
    // With a copy of /proc/<pid>/stat in the real format, the start time matches and TERM goes.
    let proc_root = app.join("proc");
    std::fs::create_dir_all(proc_root.join(pid.to_string())).unwrap();
    std::fs::write(proc_root.join(format!("{pid}/stat")), stat_line(pid, start)).unwrap();
    let guard = crate::ops::pid_guard_cmd(pid, "TERM", Some(start), &proc_root.display().to_string());
    assert_eq!(err(live.run("cell2", &guard, None, Duration::from_secs(10)).await.unwrap_err()).message, "The fence doesn't allow this change.");
    assert!(alive(), "still running");
    let out = as_class(ProcessTest, live.run("cell2", &guard, None, Duration::from_secs(10))).await.unwrap();
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(row(&live, "cell2", &guard).fence.as_deref(), Some("process-test"));
    let mut ended = None;
    for _ in 0..100 {
        if let Some(r) = ops.fleet_results(&id) {
            ended = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ended = ended.expect("the Fleet run ended when its sleep did");
    assert!(ended[0].state == "failed" && !ended[0].timed_out, "{ended:?}");
    assert!(!alive(), "TERM ended it");
    live.stop();
}

#[tokio::test]
async fn t2_files_in_sparkbox_test_on_cell2() {
    let home = temp("rehearsal-t2f-home");
    let (_node, port) = fake_node(&home, fake_node::real_shell()).await;
    let app = temp("rehearsal-t2f-app");
    let (live, _) = fake_node::connect(&app, &[("cell2", port)]).await;
    let files = Files::new(live.clone(), app.join("backups"));
    let f = Fences::new(&app);
    live.enforce_writes(f.rules("dev"));
    // ~/sparkbox-test exists: the owner makes it in Session A (a shell on cell2, step 9).
    std::fs::create_dir_all(home.join("sparkbox-test")).unwrap();
    std::fs::create_dir_all(home.join("models")).unwrap();
    let dir = home.join("sparkbox-test").display().to_string();
    let notes = format!("{dir}/notes.txt");
    let fence_of = |verb: &str| live.audit.entries().into_iter().find(|e| e.command.starts_with(&format!("python3 ~/.sparkbox/sparkjob.py {verb} "))).and_then(|e| e.fence);

    f.open(&t2(&without(&T2, FilesTest)));
    let e = files.write(false, "cell2", &notes, "v1\n", false, None).await.unwrap_err();
    assert!(format!("{e:?}").contains("The fence doesn't allow file changes."), "{e:?}");
    assert!(!Path::new(&notes).exists());

    f.open(&t2(&T2));
    // Create, then edit and save over what was opened.
    let v1 = files.write(false, "cell2", &notes, "v1\n", false, None).await.unwrap();
    files.write(false, "cell2", &notes, "v2\n", false, Some(&v1.sha256)).await.unwrap();
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "v2\n");
    assert_eq!(fence_of("commit").as_deref(), Some("files-test"));
    // The previous version is kept, and reading it needs no fence class.
    let backups = files.backups(false, "cell2", &notes).await.unwrap();
    assert_eq!(backups.len(), 1, "{backups:?}");
    // Trash, and Undo.
    let trashed = files.trash(false, "cell2", std::slice::from_ref(&notes)).await.unwrap();
    assert!(!Path::new(&notes).exists());
    // Into the folder's own Trash, so Undo stays inside it.
    assert!(trashed[0].trashed.as_deref().is_some_and(|t| t.starts_with(&format!("{dir}/.sparkbox-trash/"))), "{trashed:?}");
    assert_eq!(fence_of("trash").as_deref(), Some("files-test"));
    files.untrash(false, "cell2", &trashed).await.unwrap();
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "v2\n");
    assert_eq!(fence_of("untrash").as_deref(), Some("files-test"));
    // chmod, rename and New folder now have the files-test shape when they stay in the folder,
    // so T2 admits them (P1 r2).
    use std::os::unix::fs::PermissionsExt;
    let mode = || std::fs::metadata(&notes).unwrap().permissions().mode() & 0o777;
    files.chmod(false, "cell2", &notes, 0o600).await.unwrap();
    assert_eq!(mode(), 0o600);
    files.rename(false, "cell2", &notes, "renamed.txt").await.unwrap();
    let renamed = format!("{dir}/renamed.txt");
    assert!(!Path::new(&notes).exists() && Path::new(&renamed).exists());
    // Delete.
    files.delete(false, "cell2", std::slice::from_ref(&renamed)).await.unwrap();
    assert!(!Path::new(&renamed).exists());
    assert_eq!(fence_of("rm").as_deref(), Some("files-test"));
    // Anywhere outside ~/sparkbox-test is refused.
    let outside = home.join("models/x.txt").display().to_string();
    let e = files.write(false, "cell2", &outside, "x\n", false, None).await.unwrap_err();
    assert!(format!("{e:?}").contains("The fence doesn't allow this change."), "{e:?}");
    assert!(!Path::new(&outside).exists());
    live.stop();
}

/// Every T2 action's command as the fence reads it today (the shapes fence_class.rs knows),
/// on cell2 whose home is /home/sparkuser. The file ops confined to ~/sparkbox-test (New folder,
/// rename, permission change, the tar copies) carry the files-test shape, so T2 runs them (P1 r2).
#[test]
fn t2_actions_as_the_fence_reads_them_today() {
    use super::exit::ExecKind::{self, *};
    let h = "/home/sparkuser/sparkbox-test";
    let containers = [crate::model::Container { id: BUSYBOX.into(), name: "sparkbox-test-busybox".into(), labels: [("sparkbox.test".to_string(), "1".to_string())].into(), ..Default::default() }];
    let ctx = Ctx { containers: &containers, home: Some("/home/sparkuser"), root_lib: super::ROOT_LIB, sparkjob_sha: "0" };
    let class = |kind: ExecKind, cmd: &str| match class_of(kind, cmd, &ctx) {
        Shape::Read => "read",
        Shape::Bookkeeping => "bookkeeping",
        Shape::Change(c, _) => c.id(),
    };
    let rows: Vec<(ExecKind, String, &str)> = vec![
        // Containers.
        (Run, RUN_BUSYBOX.into(), "container-test"),
        (ContainerAction, format!("docker stop {BUSYBOX}"), "container-test"),
        (ContainerAction, format!("docker rm {BUSYBOX}"), "container-test"),
        // Processes: Fleet's sleep, renice, an unguarded signal; the guard needs its call-site tag.
        (Stream, "timeout --signal=TERM --kill-after=5s 900s sh -c 'sleep 600'".into(), "read"),
        (Run, "renice -n 10 -p 4242".into(), "process-test"),
        (Run, "kill -s TERM 4242".into(), "process-test"),
        (Run, crate::ops::pid_guard_cmd(4242, "TERM", Some(98_765_432), "/proc"), "other"),
        // Files under ~/sparkbox-test.
        (Sparkjob, format!("commit '{h}/notes.txt' --sha256 ab --expect '-' --backups '/home/sparkuser/.sparkbox/backups' --nonce 7"), "files-test"),
        (Sparkjob, format!("backups ~/.sparkbox/backups '{h}/notes.txt'"), "read"),
        (Sparkjob, format!("trash '{h}/notes.txt'"), "files-test"),
        (Sparkjob, format!("untrash '{h}/.sparkbox-trash/files/notes.txt'"), "files-test"),
        // The home Trash puts things back wherever they came from: `other`, as meant.
        (Sparkjob, "untrash '/home/sparkuser/.local/share/Trash/files/notes.txt'".into(), "other"),
        (Sparkjob, format!("rm '{h}/notes.txt'"), "files-test"),
        (ExecIo, format!("python3 ~/.sparkbox/sparkjob.py cp '{h}' '{h}/copy' 7abc 'notes.txt'"), "files-test"),
        // A copy off the node packs with tar over a stream: a read (it changes nothing there).
        (ExecIo, format!("tar -C '{h}' -cvf - -- 'notes.txt'"), "read"),
        // New folder, rename and permission change inside ~/sparkbox-test: files-test (P1 r2).
        (Run, crate::files::rename_command(&format!("{h}/notes.txt"), &format!("{h}/renamed.txt")), "files-test"),
        (Run, format!("chmod -- 600 '{h}/notes.txt'"), "files-test"),
        (Run, format!("mkdir -- '{h}/new'"), "files-test"),
        // A copy into a staging folder under ~/sparkbox-test unpacks with tar over a stream: files-test.
        (ExecIo, format!("mkdir -p -- '{h}/.sparkbox-incoming-1' && tar -C '{h}/.sparkbox-incoming-1' -xf -"), "files-test"),
    ];
    for (kind, cmd, want) in &rows {
        assert_eq!(class(*kind, cmd), *want, "{kind:?} `{cmd}`");
    }
}
