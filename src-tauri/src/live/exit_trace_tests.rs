//! The exit's admit trace (`Live::admits`, tests only) and the tests that need it. Several call
//! sites tag a command with the class its shape already gives (a tunnel's forward is `shell`
//! either way), so taking the tag away changes no fence outcome; these tests check the tag
//! itself where the tag is the thing under test, and the tag rule as a pure function. Also the
//! round-2 survivors that needed a test of their own: TERM on a dropped sudo, the wizard's reads
//! and sudo check, the root helper's install, a terminal's shell, a tag inside a typed run, and
//! the call-site Touch ID rule of a re-pin.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::tests::{docker, err, refusals, spec, TestFence};
use super::*;
use crate::errors::ErrorKind;
use crate::fake_node::{self, fake_node, real_shell, temp, Script};
use crate::live::fence_class::FenceClass::{self, *};

/// The admits of `kind` whose command is `cmd`.
fn admits_of(live: &Live, kind: ExecKind, cmd: &str) -> Vec<Admit> {
    live.admits().into_iter().filter(|a| a.kind == kind && a.cmd == cmd).collect()
}

async fn eventually(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let started = Instant::now();
    while !f() {
        assert!(started.elapsed() < Duration::from_secs(secs), "{what} didn't happen");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// cell2 on a fake node, the exit enforced with this build's (dev) rules and no fence yet.
async fn enforced(name: &str, script: Script) -> (fake_node::Node, Arc<Live>, TestFence, std::path::PathBuf) {
    let home = temp(&format!("{name}-home"));
    let (node, port) = fake_node(&home, script).await;
    let base = temp(&format!("{name}-app"));
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    (node, live, f, home)
}

/// T2's fence: cell2; test containers, process signals, files in ~/sparkbox-test.
const T2: [FenceClass; 3] = [ContainerTest, ProcessTest, FilesTest];

#[tokio::test]
async fn every_admit_is_traced_with_its_tag_and_verdict() {
    let (_node, live, f, _) = enforced("trace-basic", docker()).await;
    f.open(&spec(&["cell2"], &[Service], "dev"));
    live.run("cell2", "uptime", None, Duration::from_secs(10)).await.unwrap();
    as_class(Service, live.sudo("cell2", "systemctl restart -- 'earlyoom.service'", Duration::from_secs(10))).await.unwrap();
    as_use(live.run("cell2", "uptime", None, Duration::from_secs(10))).await.unwrap();
    let e = audit::as_typed(live.stream_cmd("cell2", "sudo reboot", Duration::from_secs(10), crate::live::CancelToken::new(), |_, _| {})).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow restarts and shutdowns.");
    let t: Vec<(ExecKind, String, Option<Tag>, bool, Verdict, bool)> = live.admits().into_iter().filter(|a| a.node == "cell2").map(|a| (a.kind, a.cmd, a.tag, a.typed, a.verdict, a.admitted)).collect();
    assert_eq!(
        t,
        [
            (ExecKind::Run, "uptime".to_string(), None, false, Verdict::Read, true),
            (ExecKind::Sudo, "systemctl restart -- 'earlyoom.service'".to_string(), Some(Tag::Class(Service)), false, Verdict::Change(Service), true),
            (ExecKind::Run, "uptime".to_string(), Some(Tag::Use), false, Verdict::Use, true),
            (ExecKind::Stream, "sudo reboot".to_string(), None, true, Verdict::Change(PowerOne), false),
        ]
    );
}

// ---- The tag rule as a pure function (as-class-items-any, and as_class's two limits, D8)

#[test]
fn a_tag_keeps_only_the_items_of_a_shape_of_its_own_class() {
    let item = || vec![vec!["x".to_string()]];
    let cmd = "systemctl restart -- 'x'";
    // A shape of the tag's class: its items stand.
    assert_eq!(tagged(Shape::Change(Service, item()), ExecKind::Sudo, cmd, Service), (Verdict::Change(Service), item()));
    // A read or bookkeeping shape, tagged: a change of the tag's class, with no items.
    assert_eq!(tagged(Shape::Read, ExecKind::Run, "uptime", ProcessTest), (Verdict::Change(ProcessTest), Vec::new()));
    assert_eq!(tagged(Shape::Bookkeeping, ExecKind::Run, "mkdir -p ~/.sparkbox", Shell), (Verdict::Change(Shell), Vec::new()));
    // `other` takes the tag only for the literals `tag_names` lists, and then with no items.
    let cancel = crate::ops::updates::cancel_command("sparkbox-upgrade-t1");
    assert_eq!(tagged(Shape::Change(Other, item()), ExecKind::Sudo, &cancel, AptOne), (Verdict::Change(AptOne), Vec::new()));
    assert_eq!(tagged(Shape::Change(Other, Vec::new()), ExecKind::Sudo, crate::live::DOCTOR_SSHD_T, SudoProbe), (Verdict::Change(SudoProbe), Vec::new()));
}

/// P1 r4 (safety review B): a tag never relabels an `other` shape into its class, except the
/// literals `fence_class::tag_names` lists, each only under its own class and way of running.
#[test]
fn a_tag_cant_relabel_root_work_or_another_class_as_a_pure_rule() {
    let item = || vec![vec!["glm53".to_string()]];
    let other = || Shape::Change(Other, Vec::new());
    // Limit A: a root-running kind keeps its shape under a non-root tag, `other` included.
    for kind in [ExecKind::Sudo, ExecKind::SudoInput, ExecKind::SparkjobRoot] {
        assert_eq!(tagged(other(), kind, "touch x", ProcessTest), (Verdict::Change(Other), Vec::new()), "{kind:?}");
        assert_eq!(tagged(other(), kind, "touch x", FilesTest), (Verdict::Change(Other), Vec::new()), "{kind:?}");
    }
    // Limit B: a shape that names another specific class decides, whatever the tag.
    assert_eq!(tagged(Shape::Change(ContainerOwner, item()), ExecKind::Run, "docker stop glm53", FilesTest), (Verdict::Change(ContainerOwner), item()));
    assert_eq!(tagged(Shape::Change(PowerOne, Vec::new()), ExecKind::Sudo, "reboot", Service), (Verdict::Change(PowerOne), Vec::new()));
    // Limit C: `other` stays `other` under every tag, root or not, unless it's a listed literal
    // under its own tag and kind.
    for c in FenceClass::OPENABLE {
        for kind in [ExecKind::Run, ExecKind::Sudo, ExecKind::Stream, ExecKind::Sparkjob, ExecKind::ExecIo] {
            assert_eq!(tagged(other(), kind, "touch x", c), (Verdict::Change(Other), Vec::new()), "{c:?} {kind:?}");
        }
    }
    let cancel = crate::ops::updates::cancel_command("sparkbox-upgrade-t1");
    let guard = crate::ops::pid_guard_cmd(4242, "TERM", Some(98_765), "/proc");
    for (c, kind, cmd) in [(AptOne, ExecKind::Run, cancel.as_str()), (Service, ExecKind::Sudo, cancel.as_str()), (ProcessTest, ExecKind::Sudo, guard.as_str()), (AptOne, ExecKind::Run, guard.as_str()), (Fleet, ExecKind::Sudo, crate::live::DOCTOR_SSHD_T), (SudoProbe, ExecKind::Run, crate::live::DOCTOR_SSHD_T)] {
        assert_eq!(tagged(other(), kind, cmd, c), (Verdict::Change(Other), Vec::new()), "{c:?} {kind:?} `{cmd}`");
    }
    assert_eq!(tagged(other(), ExecKind::Run, &guard, ProcessTest), (Verdict::Change(ProcessTest), Vec::new()));
}

/// Cancel's pass past an `only` list is apt-one's alone (C-X). At the exit today its text under
/// any other tag is `other`, which no `only` list reaches, so the rule is checked here: the exact
/// text, as root, under any other verdict acts on items like any other change.
#[test]
fn only_an_apt_one_cancel_skips_an_only_list() {
    let cancel = crate::ops::updates::cancel_command("sparkbox-upgrade-t1");
    assert!(whole_change(Verdict::Change(AptOne), ExecKind::Sudo, &cancel));
    for v in [Verdict::Change(Other), Verdict::Change(Service), Verdict::Change(AptRefresh)] {
        assert!(!whole_change(v, ExecKind::Sudo, &cancel), "{v:?}");
    }
    assert!(!whole_change(Verdict::Change(AptOne), ExecKind::Run, &cancel), "not as root");
    assert!(!whole_change(Verdict::Change(AptOne), ExecKind::Sudo, &format!("{cancel} ")), "a look-alike");
    assert!(whole_change(Verdict::Change(Service), ExecKind::Sudo, "systemctl daemon-reload"), "no item by nature, whatever the class");
}

/// Limit B at the exit: a files-test tag on a `docker stop` of someone's container stays a
/// container change, which T2's fence doesn't allow; nothing reaches the node.
#[tokio::test]
async fn a_tag_cant_turn_a_container_change_into_a_file_change() {
    let (node, live, f, _) = enforced("trace-limit-b", docker()).await;
    f.open(&spec(&["cell2"], &T2, "dev"));
    let e = as_class(FilesTest, live.run("cell2", "docker stop glm53", None, Duration::from_secs(10))).await.map_err(err).unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow container changes."));
    assert!(node.ran("docker stop").is_empty());
}

// ---- (a) 1: the process-test root retry (R2-OPS; D8 is a_tag_cant_relabel_a_root_command_past_its_shape)

/// util-linux renice as the cluster user: only root may lower a nice value.
const RENICE: &str = "n=\"$2\"; pid=\"$4\"\nif [ \"$SB_FAKE_ROOT\" != 1 ] && [ \"$n\" -lt 0 ]; then\n  echo \"renice: failed to set priority for $pid (process ID): Permission denied\" >&2\n  exit 1\nfi\necho \"$pid (process ID) old priority 0, new priority $n\"\n";

/// Under T2's fence, a renice below 0 goes as the cluster user tagged process-test, and the node
/// refuses it; the sudo retry is asked untagged (its shape, `other`, decides) and is refused
/// before any sudo reaches the node. A process-test tag on the retry is the thing this checks:
/// D8 would still refuse it, so the fence outcome alone can't tell.
#[tokio::test]
async fn the_root_retry_of_a_renice_goes_untagged() {
    let (node, live, f, _) = enforced("trace-renice", real_shell()).await;
    node.bin("renice", RENICE);
    f.open(&spec(&["cell2"], &T2, "dev"));
    let demo = Arc::new(Mutex::new(crate::demo::DemoCluster::new(&crate::inventory::Inventory::example())));
    let ops = crate::ops::Ops::new(live.clone(), demo, crate::tasks::Tasks::new(None));
    let e = ops.renice(false, "cell2", 4242, -5).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."));
    let user = admits_of(&live, ExecKind::Run, "renice -n -5 -p 4242");
    assert_eq!(user.iter().map(|a| (a.tag, a.verdict, a.admitted)).collect::<Vec<_>>(), [(Some(Tag::Class(ProcessTest)), Verdict::Change(ProcessTest), true)]);
    let root = admits_of(&live, ExecKind::Sudo, "renice -n -5 -p 4242");
    assert_eq!(root.iter().map(|a| (a.tag, a.verdict, a.admitted)).collect::<Vec<_>>(), [(None, Verdict::Change(Other), false)]);
    assert_eq!(node.ran("renice -n -5 -p 4242").len(), 1, "only the user-level try reached the node");
    assert!(node.ran("sudo ").is_empty(), "{:?}", node.ran("sudo "));
}

// ---- (a) 2: tunnels are `shell` by their tag and by their shape

/// "A page on a node": answers every request with its name.
async fn page() -> u16 {
    let page = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = page.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = page.accept().await {
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\nthe page\n").await;
        }
    });
    port
}

async fn get(local: u16, path: &str, cookie: Option<&str>) -> String {
    let mut c = TcpStream::connect(("127.0.0.1", local)).await.unwrap();
    let cookie = cookie.map(|c| format!("Cookie: {c}\r\n")).unwrap_or_default();
    c.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{local}\r\n{cookie}\r\n").as_bytes()).await.unwrap();
    let mut body = String::new();
    c.read_to_string(&mut body).await.unwrap();
    body
}

/// Both of a tunnel's forwards, the open's check and each connection through it, are tagged
/// `shell` where they're made (tunnels.rs): the DirectTcpip shape gives `shell` too, so only the
/// tag in the trace tells whether the call sites still say so.
#[tokio::test]
async fn both_forwards_of_a_tunnel_are_tagged_shell() {
    let served = page().await;
    let home = temp("trace-tunnel-home");
    let (node, port) = fake_node(&home, real_shell()).await;
    let mut frame: serde_json::Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
    frame["listeners"] = serde_json::json!([{ "port": served, "addr": "127.0.0.1", "pid": null, "process": null }]);
    *node.knobs.frame.lock().unwrap() = Some(frame.to_string());
    let base = temp("trace-tunnel-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[Shell], "dev"));
    let tunnels = crate::tunnels::Tunnels::new(live.clone());
    let opened = tunnels.open("cell2", served).await.unwrap();
    let url = opened.url();
    let token = url.split("sbt=").nth(1).unwrap().to_string();
    let local = opened.local_port;
    assert!(get(local, &format!("/?sbt={token}"), None).await.starts_with("HTTP/1.1 302"));
    assert!(get(local, "/", Some(&format!("sbt_{local}={token}"))).await.contains("the page"));
    let forwards = admits_of(&live, ExecKind::DirectTcpip, &format!("127.0.0.1:{served}"));
    assert_eq!(forwards.len(), 2, "the open's check and the page's connection: {forwards:?}");
    for a in &forwards {
        assert_eq!((a.tag, a.verdict, a.admitted), (Some(Tag::Class(Shell)), Verdict::Change(Shell), true), "{a:?}");
    }
    tunnels.close_all();
    live.stop();
}

/// Untagged, a forward is still `shell` by its shape (fail closed): without a shell fence it's
/// refused before anything is forwarded.
#[tokio::test]
async fn an_untagged_forward_needs_a_shell_fence_too() {
    assert_eq!(class_of(ExecKind::DirectTcpip, "127.0.0.1:11000", &Ctx::default()), Shape::Change(Shell, Vec::new()));
    let served = page().await;
    let (node, live, f, _) = enforced("trace-forward", real_shell()).await;
    f.open(&spec(&["cell2"], &T2, "dev"));
    let e = live.direct_tcpip("cell2", "127.0.0.1", served, 0).await.map(drop).map_err(err).unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow shells."));
    assert!(node.ran("direct-tcpip").is_empty());
    f.open(&spec(&["cell2"], &[Shell], "dev"));
    live.direct_tcpip("cell2", "127.0.0.1", served, 0).await.map(drop).unwrap();
    assert_eq!(node.ran("direct-tcpip").len(), 1);
}

// ---- (c) 3: round 2's surviving mutants

/// A sudo SparkBox stops waiting for is ended with TERM on the node (sudo_exchange's
/// CloseOnDrop::term), not left running (the verifier's v_a_dropped_sudo_is_ended_with_term).
#[tokio::test]
async fn a_dropped_sudo_is_ended_with_term() {
    let home = temp("trace-sudo-drop");
    let (node, port) = fake_node(&home, real_shell()).await;
    let (live, _) = fake_node::connect(&temp("trace-sudo-drop-app"), &[("cell1", port)]).await;
    let pid_file = home.join("sudo.pid");
    let dropped = tokio::time::timeout(Duration::from_millis(1500), live.sudo("cell1", "echo $$ > sudo.pid; exec sleep 30", Duration::from_secs(60))).await;
    assert!(dropped.is_err(), "still running when dropped: {dropped:?}");
    let mut pid = None;
    eventually("its pid", 5, || {
        pid = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse::<i32>().ok());
        pid.is_some()
    })
    .await;
    let pid = pid.unwrap();
    eventually("TERM ended the sudo'd command", 5, || unsafe { libc::kill(pid, 0) } != 0).await;
    assert!(node.knobs.signals.lock().unwrap().iter().any(|s| s == "TERM"), "{:?}", node.knobs.signals.lock().unwrap());
    live.stop();
}

/// A GB10's docker and nvidia-smi, which the fake Mac doesn't have.
fn gb10() -> Script {
    Arc::new(|cmd: &str, _: &[u8]| {
        if cmd.starts_with("docker ") {
            Some(("29.0.0\n".into(), 0))
        } else if cmd.starts_with("nvidia-smi") {
            Some(("NVIDIA GB10, 580.95.05\n".into(), 0))
        } else {
            None
        }
    })
}

fn cell2_cfg(port: u16) -> crate::inventory::NodeConfig {
    let mut c = fake_node::inventory(&[("cell2", port)]).nodes.remove(0);
    c.name = "cell2".into();
    c
}

/// The Connect wizard's reads reach the exit as reads of their own (`check_on`), and its sudo
/// `true` is admitted as tagged: a sudo check (`sudo-probe`). The shape of `sudo true` is a sudo
/// check too, so the tag is what the trace checks.
#[tokio::test]
async fn the_wizards_reads_and_sudo_check_go_through_the_exit_as_what_they_are() {
    let (node, port) = fake_node(&temp("trace-wizard-home"), gb10()).await;
    let base = temp("trace-wizard-app");
    let (live, _) = fake_node::connect(&base, &[("cell2", port)]).await;
    let f = TestFence::new(&base, "dev");
    live.enforce_writes(f.rules.clone());
    f.open(&spec(&["cell2"], &[SudoProbe], "dev"));
    let steps = crate::onboard::probe_node(&live, &cell2_cfg(port), Some("pw".into()), None, true).await;
    assert_eq!(steps.iter().find(|s| s.id == "sudo").map(|s| s.status.as_str()), Some("ok"), "{steps:?}");
    for cmd in ["python3 --version", "docker version --format '{{.Server.Version}}'", "nvidia-smi --query-gpu=name,driver_version --format=csv,noheader", "ip -br -4 addr"] {
        let a = admits_of(&live, ExecKind::Run, cmd);
        assert_eq!(a.iter().map(|a| (a.tag, a.verdict, a.admitted)).collect::<Vec<_>>(), [(None, Verdict::Read, true)], "`{cmd}`");
    }
    let sudo = admits_of(&live, ExecKind::Sudo, "true");
    assert_eq!(sudo.iter().map(|a| (a.tag, a.verdict, a.admitted)).collect::<Vec<_>>(), [(Some(Tag::Class(SudoProbe)), Verdict::Change(SudoProbe), true)]);
    assert_eq!(node.ran("sudo -k -S -p '' true").len(), 1);
    live.stop();
}

/// Root's copy of sparkjob.py, put in place when it's missing, is admitted as tagged:
/// `root-helper` (its exact install command is root-helper by shape as well).
#[tokio::test]
async fn the_root_helpers_install_is_tagged_root_helper() {
    let home = temp("trace-root-helper-home");
    let (_node, port) = fake_node(&home, real_shell()).await;
    let (live, _) = fake_node::connect(&temp("trace-root-helper-app"), &[("cell2", port)]).await;
    let lib = temp("trace-root-helper-lib");
    live.set_root_lib(&lib.join("sparkbox").display().to_string());
    let _ = live.sparkjob_root("cell2", "df /", Duration::from_secs(30)).await;
    let install = super::super::root_install_command(&live.root_lib(), sparkjob_sha());
    let a = admits_of(&live, ExecKind::SudoInput, &install);
    assert_eq!(a.iter().map(|a| (a.tag, a.verdict)).collect::<Vec<_>>(), [(Some(Tag::Class(RootHelper)), Verdict::Change(RootHelper))]);
    live.stop();
}

/// A terminal's login shell and a shell in a container leave as tagged: `shell` (term/mod.rs).
/// Both are `shell` by their Pty shape too, so the tag is what this checks.
#[tokio::test]
async fn a_terminals_shells_are_tagged_shell() {
    let (node, live, f, _) = enforced("trace-term", real_shell()).await;
    node.bin("docker", "echo in-the-container; exec sleep 5");
    f.open(&spec(&["cell2"], &[Shell], "dev"));
    let terms = crate::term::Terminals::new(live.clone());
    terms.open(false, "cell2", crate::term::Target::Shell, 80, 24, Box::new(|_| {})).await.unwrap();
    terms.open(false, "cell2", crate::term::Target::ContainerShell { container: "sparkbox-test-x".into() }, 80, 24, Box::new(|_| {})).await.unwrap();
    let ptys: Vec<(String, Option<Tag>, Verdict, bool)> = live.admits().into_iter().filter(|a| a.kind == ExecKind::Pty).map(|a| (a.cmd, a.tag, a.verdict, a.admitted)).collect();
    let exec = "docker exec -it -e TERM=xterm-256color sparkbox-test-x sh -c 'if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi'";
    assert_eq!(
        ptys,
        [
            (String::new(), Some(Tag::Class(Shell)), Verdict::Change(Shell), true),
            (exec.to_string(), Some(Tag::Class(Shell)), Verdict::Change(Shell), true),
        ]
    );
    live.stop();
}

/// A call-site tag inside a typed run stands: a `docker stop` of a test container that Fleet's
/// own code tags `container-test` is that, not a Fleet command, and a container-test fence
/// admits it.
#[tokio::test]
async fn a_tag_inside_a_typed_run_stands() {
    let (node, live, f, _) = enforced("trace-typed-tag", docker()).await;
    f.open(&spec(&["cell2"], &[ContainerTest], "dev"));
    audit::as_typed(as_class(ContainerTest, live.run("cell2", "docker stop sparkbox-test-x", None, Duration::from_secs(10)))).await.unwrap();
    let a = admits_of(&live, ExecKind::Run, "docker stop sparkbox-test-x");
    assert_eq!(a.iter().map(|a| (a.tag, a.typed, a.verdict, a.admitted)).collect::<Vec<_>>(), [(Some(Tag::Class(ContainerTest)), true, Verdict::Change(ContainerTest), true)]);
    assert_eq!(node.ran("docker stop sparkbox-test-x").len(), 1);
    // Typed and untagged, it's a Fleet command, which this fence doesn't allow.
    let e = audit::as_typed(live.run("cell2", "docker stop sparkbox-test-x", None, Duration::from_secs(10))).await.map_err(err).unwrap_err();
    assert_eq!(e.message, "The fence doesn't allow Fleet commands.");
}

// ---- Round 1's two survivors: the call-site Touch ID rule of a re-pin, and NOAUTH_NEVER's list

/// node_identity_repin's own rule, apart from the gate's: a scripted unlock never re-pins (and
/// nothing is written), the node's name must be typed, and with both it re-pins.
#[tokio::test]
async fn a_repin_refuses_a_scripted_unlock_itself() {
    let (_node, port) = fake_node::fake_spark(&temp("trace-repin-home"), "cell2").await;
    let base = temp("trace-repin-app");
    let (live, _) = fake_node::connect_sparks(&base, &[("cell2", port)]).await;
    let pins = live.exit_config_dir().join(identity_pin::FILE);
    let e = repin(&live, "cell2", Some("cell2".into()), "cell2", true).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Invalid, "Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts."));
    assert!(!pins.exists(), "nothing was pinned");
    assert_eq!(repin(&live, "cell2", Some("cell2".into()), "cell3", false).await.unwrap_err().message, "Type cell2 to re-pin it.");
    assert_eq!(repin(&live, "cell9", None, "cell9", false).await.unwrap_err().message, "cell9 isn't one of your nodes.");
    let pin = repin(&live, "cell2", Some("cell2".into()), "cell2", false).await.unwrap();
    assert_eq!(pin.hostname, "cell2-example");
    assert!(pins.exists());
    live.stop();
}

/// The four commands that change what SparkBox trusts are each on NOAUTH_NEVER, which the gate
/// refuses a scripted unlock for (admin's a_scripted_unlock_confirms_nothing_sparkbox_trusts).
#[test]
fn every_command_that_changes_trust_is_never_scripted() {
    for cmd in ["fence_open", "fence_trust_key", "node_identity_repin", "known_hosts_trust"] {
        assert!(crate::admin::NOAUTH_NEVER.contains(&cmd), "{cmd}");
    }
}

// ---- (c) 2 and (c) 5 at the exit: the test folder's Trash, and a refused Files change in words

/// T2's fence: trashing ~/sparkbox-test itself, or something already in its Trash, is `other`,
/// refused before it reaches the node; nothing lands in ~/.local/share/Trash. Without a fence
/// (the exit permissive), sparkjob.py refuses both on the node.
#[tokio::test]
async fn the_test_folder_and_its_trash_never_go_to_the_home_trash() {
    let (node, live, f, home) = enforced("trace-trash", real_shell()).await;
    f.open(&spec(&["cell2"], &T2, "dev"));
    std::fs::create_dir_all(home.join("sparkbox-test/.sparkbox-trash/files")).unwrap();
    std::fs::write(home.join("sparkbox-test/.sparkbox-trash/files/a.txt"), "a").unwrap();
    let q = |p: &str| crate::live::sh_quote(p);
    let folder = home.join("sparkbox-test").display().to_string();
    let trashed = home.join("sparkbox-test/.sparkbox-trash/files/a.txt").display().to_string();
    for p in [folder.as_str(), "~/sparkbox-test", "~/sparkbox-test/", trashed.as_str(), "~/sparkbox-test/.sparkbox-trash"] {
        let e = err(live.sparkjob("cell2", &format!("trash {}", q(p)), None, Duration::from_secs(20)).await.unwrap_err());
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow this change."), "{p}");
    }
    assert!(node.ran("sparkjob.py trash").is_empty(), "{:?}", node.ran("sparkjob"));
    // An item in the folder is still files-test, and goes to the folder's own Trash.
    std::fs::write(home.join("sparkbox-test/b.txt"), "b").unwrap();
    live.sparkjob("cell2", &format!("trash {}", q("~/sparkbox-test/b.txt")), None, Duration::from_secs(20)).await.unwrap();
    assert!(home.join("sparkbox-test/.sparkbox-trash/files/b.txt").exists());

    // On the node: sparkjob.py refuses them too, whatever let the command through.
    let (_n, port) = fake_node(&home, real_shell()).await;
    let (open, _) = fake_node::connect(&temp("trace-trash-open-app"), &[("cell2", port)]).await;
    let e = open.sparkjob("cell2", &format!("trash {}", q("~/sparkbox-test")), None, Duration::from_secs(20)).await.unwrap_err();
    assert!(e.contains("SparkBox won't put ~/sparkbox-test itself in the Trash; delete what's in it instead."), "{e}");
    let e = open.sparkjob("cell2", &format!("trash {}", q(&trashed)), None, Duration::from_secs(20)).await.unwrap_err();
    assert!(e.contains("is already in the Trash; Delete removes it for good."), "{e}");
    assert!(home.join("sparkbox-test").is_dir() && Path::new(&trashed).exists());
    assert!(!home.join(".local/share/Trash").exists(), "nothing in the home Trash");
    live.stop();
    open.stop();
}

/// A refused Files change is written with its whole command, as the admitted one is, and its
/// error says what it would have done; the error SparkBox returns is the exit's own.
#[tokio::test]
async fn a_refused_files_change_says_what_it_would_have_done() {
    let (node, live, f, home) = enforced("trace-files-words", real_shell()).await;
    // A fence without files-test.
    f.open(&spec(&["cell2"], &[ContainerTest], "dev"));
    let h = home.display().to_string();
    let q = |p: &str| crate::live::sh_quote(p);
    let rows = [
        (format!("commit {} --sha256 ab --expect '-' --nonce 7", q(&format!("{h}/sparkbox-test/notes.txt"))), "Save notes.txt"),
        (format!("commit {} --sha256 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 --expect absent", q(&format!("{h}/sparkbox-test/new.txt"))), "New file new.txt"),
        (format!("trash {}", q(&format!("{h}/sparkbox-test/notes.txt"))), "Move notes.txt to the Trash"),
        (format!("trash {} {}", q("~/sparkbox-test/a"), q("~/sparkbox-test/b")), "Move 2 items to the Trash"),
        (format!("untrash {}", q("~/sparkbox-test/.sparkbox-trash/files/notes.txt")), "Put back notes.txt"),
        (format!("rm {}", q("~/sparkbox-test/old")), "Delete old"),
        (format!("delete {} {}", q("~/sparkbox-test"), q("m1")), "Delete m1"),
        (format!("place {} {}", q("~/sparkbox-test/.sparkbox-incoming-1"), q("~/sparkbox-test/dest")), "Copy files to dest"),
    ];
    for (args, words) in &rows {
        let e = err(live.sparkjob("cell2", args, None, Duration::from_secs(20)).await.unwrap_err());
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Fenced, "The fence doesn't allow file changes."), "{args}");
        let row = refusals(&live).into_iter().next().unwrap();
        assert_eq!(row.command, format!("python3 ~/.sparkbox/sparkjob.py {args}"));
        assert_eq!(row.error.as_deref(), Some(format!("{words}: The fence doesn't allow file changes.").as_str()));
        assert_eq!((row.title.as_str(), row.state.as_str()), ("Refused", "failed"));
    }
    // A copy within the node (over exec_io) and a rename (a shell command) say so too.
    let cp = format!("python3 ~/.sparkbox/sparkjob.py cp {} {} 7 'a.txt'", q("~/sparkbox-test"), q("~/sparkbox-test/copy"));
    assert!(live.exec_io("cell2", &cp).await.is_err());
    assert_eq!(refusals(&live).into_iter().next().unwrap().error.as_deref(), Some("Copy a.txt to copy: The fence doesn't allow file changes."));
    let rename = crate::files::rename_command(&format!("{h}/sparkbox-test/a.txt"), &format!("{h}/sparkbox-test/b.txt"));
    let e = err(live.run("cell2", &rename, None, Duration::from_secs(10)).await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow file changes.");
    let row = refusals(&live).into_iter().next().unwrap();
    assert_eq!((row.command.as_str(), row.error.as_deref()), (rename.as_str(), Some("Rename a.txt to b.txt: The fence doesn't allow file changes.")));
    // A sudo save is root's checked copy of sparkjob.py, as its admitted line shows it.
    let lib = temp("trace-files-words-lib").join("sparkbox").display().to_string();
    live.set_root_lib(&lib);
    let args = "commit '/etc/hosts' --sha256 ab --expect '-' --from '~/.sparkbox/tmp/x'";
    let e = err(live.sparkjob_root("cell2", args, Duration::from_secs(20)).await.unwrap_err());
    assert_eq!(e.message, "The fence doesn't allow this change.");
    let row = refusals(&live).into_iter().next().unwrap();
    assert_eq!(row.command, crate::live::root_sparkjob_command(&lib, sparkjob_sha(), args));
    assert_eq!(row.error.as_deref(), Some("Save hosts: The fence doesn't allow this change."));
    assert!(node.ran("sparkjob.py commit").is_empty() && node.ran("mv --").is_empty() && node.ran("sudo").is_empty());
    live.stop();
}
