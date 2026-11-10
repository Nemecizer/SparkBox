//! Terminal targets, the demo shell, read-only followers, and output batching with
//! backpressure against a fake Spark. The live PTY path is tested in live/tests.rs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;

use super::*;
use crate::fake_node::{connect, fake_node, real_shell, temp};
#[allow(unused_imports)]
use base64::Engine as _;
use crate::inventory::Inventory;
use crate::vpn::Vpn;

#[test]
fn targets_become_safe_commands() {
    assert_eq!(command_for(&Target::Shell).unwrap(), None);
    let here = command_for(&Target::ShellIn { cwd: "/home/u/my models".into() }).unwrap().unwrap();
    assert_eq!(here, "cd -- '/home/u/my models' && exec \"${SHELL:-/bin/bash}\" -l");
    assert!(command_for(&Target::ShellIn { cwd: "~/x'y".into() }).unwrap().unwrap().starts_with("cd -- \"$HOME\"'/x'\\''y'"));
    assert!(command_for(&Target::ShellIn { cwd: "relative".into() }).is_err());
    let shell = command_for(&Target::ContainerShell { container: "vllm-head".into() }).unwrap().unwrap();
    assert!(shell.starts_with("docker exec -it -e TERM=xterm-256color vllm-head sh -c"), "{shell}");
    assert_eq!(
        command_for(&Target::ContainerLogs { container: "0123456789ab".into(), since: None }).unwrap().as_deref(),
        Some("docker logs -f --tail 500 0123456789ab")
    );
    assert!(command_for(&Target::Journal { unit: "nvidia-dgx-telemetry@1.service".into(), since: None }).unwrap().unwrap().ends_with("-u nvidia-dgx-telemetry@1.service"));
    for bad in ["x; rm -rf ~", "$(reboot)", "a b", "", "--privileged"] {
        assert!(command_for(&Target::ContainerShell { container: bad.into() }).is_err(), "{bad}");
        assert!(command_for(&Target::Journal { unit: bad.into(), since: None }).is_err(), "{bad}");
    }
    // The UI writes {kind: "shell", cwd?}.
    assert_eq!(serde_json::from_str::<Target>(r#"{"kind":"shell"}"#).unwrap(), Target::Shell);
    assert_eq!(serde_json::from_str::<Target>(r#"{"kind":"shell","cwd":"/srv"}"#).unwrap(), Target::ShellIn { cwd: "/srv".into() });
    assert_eq!(serde_json::to_string(&Target::ShellIn { cwd: "/srv".into() }).unwrap(), r#"{"kind":"shell","cwd":"/srv"}"#);
    assert!(Target::Shell.is_shell() && !Target::Journal { unit: "x".into(), since: None }.is_shell());
    // Log tabs can start further back, but only from the fixed choices.
    let since = |s: &str| Some(s.to_string());
    assert_eq!(command_for(&Target::ContainerLogs { container: "vllm-head".into(), since: since("1h") }).unwrap().as_deref(), Some("docker logs -f --since 1h vllm-head"));
    assert_eq!(command_for(&Target::Journal { unit: "docker.service".into(), since: since("boot") }).unwrap().as_deref(), Some("journalctl -f -b --no-hostname -u docker.service"));
    assert_eq!(command_for(&Target::Journal { unit: "docker.service".into(), since: since("10m") }).unwrap().as_deref(), Some("journalctl -f --since -10min --no-hostname -u docker.service"));
    assert!(command_for(&Target::Journal { unit: "docker.service".into(), since: since("1h; reboot") }).is_err());
    assert_eq!(serde_json::from_str::<Target>(r#"{"kind":"journal","unit":"x"}"#).unwrap(), Target::Journal { unit: "x".into(), since: None });
}

type Seen = Arc<Mutex<(String, Option<Option<u32>>)>>;

fn collector() -> (Emit, Seen) {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    let emit: Emit = Box::new(move |e| match e {
        TermEvent::Data { data } => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
            s.lock().unwrap().0.push_str(&String::from_utf8_lossy(&bytes));
        }
        TermEvent::Exit { code, .. } => s.lock().unwrap().1 = Some(code),
        _ => {}
    });
    (emit, seen)
}

async fn wait_for(seen: &Seen, f: impl Fn(&(String, Option<Option<u32>>)) -> bool) {
    for _ in 0..150 {
        if f(&seen.lock().unwrap()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out; terminal so far: {:?}", seen.lock().unwrap());
}

fn terminals() -> (Arc<Terminals>, Arc<Mutex<DemoCluster>>) {
    let dir = std::env::temp_dir().join(format!("sparkbox-term-{}", std::process::id()));
    let vpn = Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let cluster = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
    let fs = Arc::new(Mutex::new(DemoFs::new()));
    (Terminals::with_demo(Live::new(vpn, dir.join("known_hosts.json"), || Ok(None)), cluster.clone(), fs), cluster)
}

#[tokio::test]
async fn the_demo_shell_uses_the_demo_filesystem_and_cluster() {
    let (t, _) = terminals();
    let (emit, seen) = collector();
    let id = t.open(true, "cell2", Target::Shell, 80, 24, emit).await.unwrap();
    // terminal_open authorises a shell once it's open in Admin mode (open_gated).
    t.authorize_shell(&id).unwrap();
    wait_for(&seen, |s| s.0.contains("@cell2") && s.0.contains("$ ")).await;
    assert!(seen.lock().unwrap().0.contains("\x1b]0;"), "the prompt sets the title (OSC 0)");

    t.input(&id, b"hostnamx\x7fe\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("\r\ncell2")).await;
    assert!(seen.lock().unwrap().0.contains("\x08 \x08"), "backspace erases on screen");
    t.input(&id, b"cd models && ls\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("meta-llama")).await;
    t.input(&id, b"pwd; cat /etc/hosts | grep fabric2 | grep cell3\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("/models\r\n") && s.0.contains("192.168.101.3   cell3-fabric2")).await;
    t.input(&id, b"echo first > ~/log.txt; echo second >> ~/log.txt; cat ~/log.txt\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("first\r\nsecond\r\n")).await;
    t.input(&id, b"nvidia-smi\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("NVIDIA GB10")).await;
    t.input(&id, b"false && echo never; echo $?\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("\r\n1\r\n")).await;
    assert!(!seen.lock().unwrap().0.contains("\r\nnever\r\n"), "&& stops after a failure");
    t.input(&id, b"ping cell1-fabric\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("from cell1-fabric (192.168.100.1): icmp_seq=1")).await;
    t.input(&id, vec![3]).unwrap();
    wait_for(&seen, |s| s.0.contains("packets transmitted")).await;
    t.input(&id, b"exit\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.1 == Some(Some(0))).await;
    // Closed terminals stop taking input.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(t.input(&id, b"x".to_vec()).is_err());
    assert_eq!(t.count(), 0);
}

#[tokio::test]
async fn a_shell_can_start_in_a_folder() {
    let (t, _) = terminals();
    let (emit, seen) = collector();
    let home = crate::demo::world::home();
    let id = t.open(true, "cell1", Target::ShellIn { cwd: format!("{home}/projects/bench") }, 80, 24, emit).await.unwrap();
    t.authorize_shell(&id).unwrap();
    wait_for(&seen, |s| s.0.contains("~/projects/bench")).await;
    t.input(&id, b"ls\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("sweep.yaml")).await;
}

#[tokio::test]
async fn log_followers_are_read_only_so_ctrl_c_cant_stop_them() {
    let (t, _) = terminals();
    let (emit, seen) = collector();
    // Reproduce a recent, cached stats line with no Models watcher producing more.
    // A distinct container keeps other concurrently running Demo tests out of this fixture.
    let container = "vllm-read-only-test";
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    crate::models::demo_engine_log_push("cell1", container, at.saturating_sub(5), "INFO Engine 000: Avg generation throughput: 1.0 tokens/s");
    let id = t.open(true, "cell1", Target::ContainerLogs { container: container.into(), since: None }, 80, 24, emit).await.unwrap();
    wait_for(&seen, |s| s.0.contains("Avg generation throughput")).await;
    assert!(!t.is_shell(&id));
    assert!(t.input(&id, vec![3]).unwrap_err().contains("read-only"));
    // Fresh request logs prove the follower still runs. Other Demo tests may have
    // populated the shared engine-log cache; without a Models watcher here, those
    // cached statistics need not advance. Their cadence is not what this tests.
    let before = seen.lock().unwrap().0.len();
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    while seen.lock().unwrap().0.len() == before {
        assert!(std::time::Instant::now() < deadline, "no new log output in 12 s");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(seen.lock().unwrap().1, None, "still following");
    t.close(&id);
    wait_for(&seen, |s| s.1.is_some()).await;
}

#[tokio::test]
async fn live_terminals_need_a_connected_node() {
    let (t, _) = terminals();
    let (emit, _) = collector();
    let err = t.open(false, "cell1", Target::Shell, 80, 24, emit).await.unwrap_err();
    assert!(err.contains("isn't connected"), "{err}");
}

/// 50 MB from a fake node: output arrives in batches of at most 64 KB and stops at the
/// high-water mark while nothing is acknowledged, but reading from the node doesn't stop:
/// the command finishes, the newest 2 MiB wait, and the window is told what it skipped.
#[tokio::test]
async fn output_is_batched_paced_and_never_holds_up_the_node() {
    const TOTAL: usize = 50 * 1024 * 1024;
    let home = temp("term-flow");
    let (_node, port) = fake_node(&home, real_shell()).await;
    let (live, _) = connect(&temp("term-flow-app"), &[("cell1", port)]).await;
    let exec = live.exec_io("cell1", &format!("head -c {TOTAL} /dev/zero | tr '\\0' x; printf END")).await.unwrap();
    exec.write.eof().await.unwrap();

    let flow = Arc::new(Flow::default());
    let (_in_tx, in_rx) = mpsc::channel(8);
    let (out_tx, mut out_rx) = mpsc::channel(4);
    let (read, write, _hold) = exec.into_parts();
    let task = tokio::spawn(pump(read, write, in_rx, flow.clone(), out_tx));

    // Nothing acknowledged: delivery stops at the high-water mark.
    let mut got = 0usize;
    loop {
        match tokio::time::timeout(Duration::from_millis(700), out_rx.recv()).await {
            Ok(Some(Out::Data(d))) => {
                assert!(d.len() <= BATCH_MAX);
                got += d.len();
            }
            Ok(_) => panic!("ended early"),
            Err(_) => break,
        }
    }
    assert!(got as u64 >= HIGH_WATER && got as u64 <= HIGH_WATER + BATCH_MAX as u64, "paused at {got} unacknowledged bytes");
    assert_eq!(flow.unacked(), got as u64);
    // Meanwhile the command finished on the node: SparkBox kept reading its output.
    let done = || _node.stdin.lock().unwrap().iter().any(|(c, _)| c.contains("head -c"));
    for _ in 0..600 {
        if done() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(done(), "the node was held up by an unread channel");
    assert_eq!(flow.unacked(), got as u64, "and still nothing more was sent to the window");

    // Acknowledge as xterm would: first how much was skipped, then the newest bytes, then the end.
    flow.ack(got as u64);
    let mut skipped = 0u64;
    let mut tail = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(60), out_rx.recv()).await.expect("stalled") {
            Some(Out::Skipped(n)) => {
                assert!(tail.is_empty(), "the skip comes before what's left");
                skipped += n;
            }
            Some(Out::Data(d)) => {
                flow.ack(d.len() as u64);
                tail.extend_from_slice(&d);
            }
            Some(Out::Exit { .. }) | None => break,
            Some(_) => {}
        }
        if tail.ends_with(b"END") {
            break;
        }
    }
    let (code, _) = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
    assert_eq!(code, Some(0));
    assert!(tail.ends_with(b"END") && tail.len() <= RING_MAX + BATCH_MAX, "the newest output: {} bytes", tail.len());
    assert_eq!(got as u64 + skipped + tail.len() as u64, TOTAL as u64 + 3, "every byte is either shown or counted as skipped");
    live.stop();
}

/// The round-1 stall: one paused tab that never acknowledges, while 50 MB pours into it,
/// must not hold up anything else: short commands, the collector, or another tab on the
/// same connection. When it resumes it shows what it skipped.
#[tokio::test]
async fn a_paused_tab_cant_stall_the_node() {
    let home = temp("term-stall");
    let (node, port) = fake_node(&home, real_shell()).await;
    let (live, inv) = connect(&temp("term-stall-app"), &[("cell1", port)]).await;
    let t = Terminals::new(live.clone());

    let paused: Arc<Mutex<(String, u64)>> = Arc::default();
    let p = paused.clone();
    let a = t
        .open(false, "cell1", Target::Shell, 80, 24, Box::new(move |e| match e {
            TermEvent::Data { data } => p.lock().unwrap().0.push_str(&String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(data).unwrap())),
            TermEvent::Skipped { bytes } => p.lock().unwrap().1 += bytes,
            _ => {}
        }))
        .await
        .unwrap();
    t.authorize_shell(&a).unwrap();
    let (emit_b, seen_b) = collector();
    let b = t.open(false, "cell1", Target::Shell, 80, 24, emit_b).await.unwrap();
    t.authorize_shell(&b).unwrap();
    wait_for(&seen_b, |s| s.0.contains("$ ")).await;
    // Tab B acknowledges everything, like a visible xterm.
    let (t2, b2, seen2) = (t.clone(), b.clone(), seen_b.clone());
    let acker = tokio::spawn(async move {
        loop {
            let n = seen2.lock().unwrap().0.len() as u64;
            t2.ack(&b2, n);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    // Tab A floods and never acknowledges (its window is paused). The node says when it has sent
    // the lot: on a busy machine that takes longer than the 8 s the checks below run for, and a
    // resume before then would have skipped less.
    let flooded = home.join("flooded");
    t.input(&a, format!("head -c 50000000 /dev/zero | tr '\\0' x; echo; echo FLOOD-DONE; touch {}\r", flooded.display()).into_bytes()).unwrap();
    let started = std::time::Instant::now();
    let mut checks = 0;
    while started.elapsed() < Duration::from_secs(8) || !flooded.exists() {
        assert!(started.elapsed() < Duration::from_secs(120), "the flood never finished");
        // A short command on the node answers at once.
        let t0 = std::time::Instant::now();
        assert_eq!(live.run("cell1", "echo hi", None, Duration::from_secs(5)).await.unwrap().stdout, "hi\n");
        assert!(t0.elapsed() < Duration::from_secs(1), "echo hi took {:?}", t0.elapsed());
        // The other tab on the same connection keeps answering.
        let marker = format!("tick{checks}");
        t.input(&b, format!("echo {marker}\r").into_bytes()).unwrap();
        wait_for(&seen_b, |s| s.0.contains(&format!("\r\n{marker}\r\n"))).await;
        checks += 1;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // The collector's frames kept coming.
    let age = live.snapshot(&inv).nodes[0].frame_age_ms.unwrap();
    assert!(age < 2500, "frames stopped: {age} ms old");
    assert!(checks >= 5);
    let unacked = t.open.lock().unwrap().get(&a).map(|s| s.flow.unacked()).unwrap();
    assert!(unacked <= HIGH_WATER + BATCH_MAX as u64, "the paused tab got only what the window allows: {unacked}");

    // Resume: the tab shows how much it skipped, then the newest output, ending with the prompt.
    let before = paused.lock().unwrap().0.len();
    t.ack(&a, unacked);
    let (t3, a3, p3) = (t.clone(), a.clone(), paused.clone());
    let acker_a = tokio::spawn(async move {
        loop {
            let n = p3.lock().unwrap().0.len() as u64;
            t3.ack(&a3, n);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    for _ in 0..500 {
        if paused.lock().unwrap().0[before..].contains("FLOOD-DONE") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (text, skipped) = paused.lock().unwrap().clone();
    assert!(text[before..].contains("FLOOD-DONE"), "the end arrived");
    assert!(skipped > 40_000_000, "told what it skipped: {skipped}");
    assert!(node.total_connections() <= 2, "control and bulk, nothing more: {}", node.total_connections());
    acker.abort();
    acker_a.abort();
    live.stop();
}

#[test]
fn saved_logs_get_a_new_safe_name_each_time() {
    let dir = temp("term-save");
    let a = save_text(&dir, "vllm-head / cell1", "one\n").unwrap();
    let b = save_text(&dir, "vllm-head / cell1", "two\n").unwrap();
    assert_ne!(a, b, "never replaces an earlier save");
    let name = a.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.starts_with("vllm-head - cell1 ") && name.ends_with(".log"), "{name}");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "two\n");
    assert!(save_text(&dir, "../../etc", "x").unwrap().starts_with(&dir));
}

#[tokio::test]
async fn the_demo_shell_has_tmp_pipes_and_gpu_queries() {
    let (t, _) = terminals();
    let (emit, seen) = collector();
    let id = t.open(true, "cell1", Target::Shell, 120, 24, emit).await.unwrap();
    t.authorize_shell(&id).unwrap();
    wait_for(&seen, |s| s.0.contains("$ ")).await;
    t.input(&id, b"echo hello > /tmp/x.txt; cat /tmp/x.txt | wc -l\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("hello") || s.0.contains("\r\n1\r\n")).await;
    wait_for(&seen, |s| s.0.contains("\r\n1\r\n")).await;
    assert!(!seen.lock().unwrap().0.contains("Permission denied"), "/tmp is writable: {:?}", seen.lock().unwrap().0);
    t.input(&id, b"cat /etc/hosts | grep fabric | grep -v '#' | sort | head -n 2 | cut -d ' ' -f 1\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("192.168.100.1\r\n")).await;
    t.input(&id, b"printf x; cat /etc/hosts | tail -n 1 | wc -w\r".to_vec()).unwrap();
    t.input(&id, b"nvidia-smi --query-gpu=name,temperature.gpu,power.draw,memory.used --format=csv\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("name, temperature.gpu, power.draw [W], memory.used\r\nNVIDIA GB10, ")).await;
    t.input(&id, b"nvidia-smi --query-gpu=utilization.gpu,driver_version --format=csv,noheader,nounits\r".to_vec()).unwrap();
    // The driver the real Sparks run (demo world, r4).
    let driver = format!(", {}", crate::demo::world::DRIVER);
    wait_for(&seen, |s| s.0.lines().any(|l| l.trim_end().ends_with(&driver) && !l.contains('%'))).await;
    t.input(&id, b"echo a; echo a | uniq -c\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("      1 a")).await;
    // The filters also take a file, as they do on a real node.
    t.input(&id, b"grep 127.0.1.1 /etc/hosts; wc -l /etc/hostname; cut -d ' ' -f 2 /etc/hosts | head -n 1\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("127.0.1.1 cell1-example\r\n") && s.0.contains("\r\n1 /etc/hostname\r\n") && s.0.contains("\r\nlocalhost\r\n")).await;
    t.input(&id, b"grep x\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("grep: reads a file or a pipe in demo mode")).await;
    t.input(&id, b"exit\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.1.is_some()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(t.input(&id, b"x".to_vec()).unwrap_err(), "This tab has closed.");
    // Demo sessions are in the activity log: opened and closed, nothing typed.
    let log = t.live.audit.entries();
    // One entry from open to close: a session (not a change), ended with how it closed.
    let shells: Vec<_> = log.iter().filter(|e| e.title == "Open a shell on cell1").collect();
    assert_eq!(shells.len(), 1, "{log:?}");
    let e = shells[0];
    assert!(e.simulated && e.class == "session" && !e.changes && e.kind == "terminal", "{e:?}");
    assert_eq!(e.state, "ok");
    assert!(e.stdout_tail.as_deref().is_some_and(|t| t.starts_with("Closed: ")), "{e:?}");
    assert!(!log.iter().any(|e| e.command.contains("nvidia-smi")), "keystrokes are never logged");
}

#[tokio::test]
async fn a_revoked_shell_takes_no_input_until_it_is_authorized_again() {
    let (t, _) = terminals();
    let (emit, seen) = collector();
    let id = t.open(true, "cell2", Target::Shell, 80, 24, emit).await.unwrap();
    wait_for(&seen, |s| s.0.contains("$ ")).await;
    assert!(!t.shell_authorized(&id), "a shell opens unauthorised");
    t.authorize_shell(&id).unwrap();
    assert!(t.shell_authorized(&id));
    assert_eq!(t.revoke_shells("Locked from the top bar"), 1);
    assert!(!t.shell_authorized(&id));
    assert_eq!(t.input(&id, b"hostname\r".to_vec()).unwrap_err(), LOCKED_INPUT);
    t.authorize_shell(&id).unwrap();
    t.input(&id, b"hostname\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("\r\ncell2-example\r\n")).await;
    // Log followers aren't shells: nothing to revoke or authorize.
    let (emit, _) = collector();
    let logs = t.open(true, "cell2", Target::ContainerLogs { container: "vllm-worker".into(), since: None }, 80, 24, emit).await.unwrap();
    assert!(t.authorize_shell(&logs).is_err() && !t.shell_authorized(&logs));
}

// ---- Shells keep working: authorised once, locked only by the owner or their own idle time

#[derive(Clone)]
struct FakeClock(Arc<AtomicU64>);

impl crate::admin::Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl FakeClock {
    fn advance_s(&self, s: u64) {
        self.0.fetch_add(s * 1000, Ordering::SeqCst);
    }
}

struct Yes;
impl crate::admin::Authenticator for Yes {
    fn authenticate(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Terminals and an Admin gate on one fake clock, wired as the app wires them.
fn gated() -> (Arc<Terminals>, Arc<crate::admin::AdminGate>, FakeClock) {
    let (t, _) = terminals();
    let clock = FakeClock(Arc::new(AtomicU64::new(1_000_000)));
    let gate = crate::admin::AdminGate::new(Box::new(clock.clone()), Box::new(Yes), None);
    t.set_clock(gate.clock());
    let hooks = t.clone();
    gate.on_relock(move |why, text| {
        hooks.on_relock(why, text);
    });
    (t, gate, clock)
}

/// Every notice a tab got (locked, unlocked, warnings), in order.
type Notices = Arc<Mutex<Vec<TermEvent>>>;

async fn open_shell(t: &Arc<Terminals>, gate: &crate::admin::AdminGate) -> (String, Seen, Notices) {
    let seen: Seen = Arc::default();
    let notices: Notices = Arc::default();
    let (s, n) = (seen.clone(), notices.clone());
    let emit: Emit = Box::new(move |e| match e {
        TermEvent::Data { data } => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
            s.lock().unwrap().0.push_str(&String::from_utf8_lossy(&bytes));
        }
        TermEvent::Exit { code, .. } => s.lock().unwrap().1 = Some(code),
        other => n.lock().unwrap().push(other),
    });
    let id = t.open_gated(gate, true, "cell2", Target::Shell, 80, 24, emit).await.unwrap();
    wait_for(&seen, |s| s.0.contains("$ ")).await;
    (id, seen, notices)
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(30)).await;
}

#[tokio::test]
async fn typing_keeps_a_shell_open_for_20_minutes() {
    let (t, gate, clock) = gated();
    gate.unlock(None).unwrap();
    let (id, _, notices) = open_shell(&t, &gate).await;
    assert!(t.shell_authorized(&id), "authorised once, at open");
    // Twenty minutes of typing every 30 s, with no other change: Admin mode relocks at 15
    // minutes (its own idle time), and the shell carries on.
    for i in 1..=40 {
        clock.advance_s(30);
        gate.tick();
        t.tick();
        t.input(&id, b"x".to_vec()).unwrap_or_else(|e| panic!("keystroke {i} refused: {e}"));
    }
    assert!(!gate.state().unlocked, "Admin mode relocked after 15 minutes without a change");
    assert!(t.shell_authorized(&id));
    settle().await;
    assert!(notices.lock().unwrap().is_empty(), "no lock and no warning: {:?}", notices.lock().unwrap());
}

#[tokio::test]
async fn admin_idle_relock_leaves_shells_open() {
    let (t, gate, clock) = gated();
    gate.unlock(None).unwrap();
    let (id, seen, notices) = open_shell(&t, &gate).await;
    clock.advance_s(16 * 60);
    assert!(gate.tick(), "idle relock");
    t.tick();
    // Closing the window to the menu bar doesn't end it either.
    gate.unlock(None).unwrap();
    gate.lock_with(crate::admin::RelockReason::WindowClosed, "The window was closed");
    assert!(t.shell_authorized(&id));
    t.input(&id, b"hostname\r".to_vec()).unwrap();
    wait_for(&seen, |s| s.0.contains("\r\ncell2-example\r\n")).await;
    settle().await;
    assert!(notices.lock().unwrap().iter().all(|e| !matches!(e, TermEvent::Locked { .. })));
}

#[tokio::test]
async fn shell_idle_timer_locks_with_a_warning() {
    let (t, gate, clock) = gated();
    gate.unlock(None).unwrap();
    let (id, _, notices) = open_shell(&t, &gate).await;
    // Default: 60 minutes. A minute before, the tab says so.
    clock.advance_s(58 * 60);
    t.tick();
    settle().await;
    assert!(notices.lock().unwrap().is_empty(), "nothing at 58 minutes");
    clock.advance_s(60);
    t.tick();
    t.tick();
    settle().await;
    let warned: Vec<String> = notices.lock().unwrap().iter().filter_map(|e| match e {
        TermEvent::IdleWarning { text } => Some(text.clone()),
        _ => None,
    }).collect();
    assert_eq!(warned, ["This shell locks in 60 s: nothing typed for 59 min. Type anything to keep it open."], "once");
    // Typing resets it; then a full hour of nothing locks it.
    t.input(&id, b"x".to_vec()).unwrap();
    clock.advance_s(59 * 60 + 30);
    t.tick();
    clock.advance_s(31);
    t.tick();
    settle().await;
    assert!(!t.shell_authorized(&id));
    assert_eq!(t.input(&id, b"y".to_vec()).unwrap_err(), LOCKED_INPUT);
    assert!(notices.lock().unwrap().iter().any(|e| matches!(e, TermEvent::Locked { reason } if reason == "Locked: nothing typed for 60 min.")));
    // 15 minutes and Never.
    t.authorize_shell(&id).unwrap();
    t.set_idle_minutes(15);
    clock.advance_s(15 * 60);
    t.tick();
    assert!(!t.shell_authorized(&id));
    t.authorize_shell(&id).unwrap();
    t.set_idle_minutes(0);
    clock.advance_s(5 * 3600);
    t.tick();
    assert!(t.shell_authorized(&id), "never");
}

/// r4 F11: "Never" holds only in Demo; a Live shell locks after 8 hours with nothing typed,
/// and the setting is capped there too.
#[test]
fn never_is_demo_only_and_live_shells_cap_at_8_hours() {
    assert_eq!(super::idle_limit_minutes(0, false), 0, "Demo: never");
    assert_eq!(super::idle_limit_minutes(0, true), 480, "Live: 8 h");
    assert_eq!(super::idle_limit_minutes(240, true), 240);
    assert_eq!(super::idle_limit_minutes(60, false), 60);
    assert_eq!(super::idle_limit_minutes(24 * 60, true), 480);
}

/// r4 F11: a shell in a container runs as root, so even an idle relock of Admin mode locks it;
/// an ordinary shell carries on.
#[tokio::test]
async fn root_shells_lock_with_admin_mode() {
    let (t, gate, clock) = gated();
    gate.unlock(None).unwrap();
    let (plain, _, _) = open_shell(&t, &gate).await;
    let notices: Notices = Arc::default();
    let n = notices.clone();
    let emit: Emit = Box::new(move |e| {
        if !matches!(e, TermEvent::Data { .. }) {
            n.lock().unwrap().push(e);
        }
    });
    let root = t.open_gated(&gate, true, "cell1", Target::ContainerShell { container: "vllm-head".into() }, 80, 24, emit).await.unwrap();
    assert!(t.shell_authorized(&root));
    clock.advance_s(16 * 60);
    assert!(gate.tick(), "idle relock");
    assert!(!t.shell_authorized(&root), "the root shell locked with Admin mode");
    assert!(t.shell_authorized(&plain), "the ordinary shell didn't");
    settle().await;
    assert!(notices.lock().unwrap().iter().any(|e| matches!(e, TermEvent::Locked { reason } if reason.contains("runs as root"))), "{:?}", notices.lock().unwrap());
}

/// A shell's lock reason is the relock's own sentence, one period, then why it locks too.
#[tokio::test]
async fn a_lock_reason_reads_as_two_sentences() {
    use crate::admin::RelockReason;
    for (text, want) in [
        ("No changes for 15 minutes, so SparkBox is read-only again.", "No changes for 15 minutes, so SparkBox is read-only again. A shell in a container runs as root, so it locks with Admin mode."),
        ("The window was closed", "The window was closed. A shell in a container runs as root, so it locks with Admin mode."),
    ] {
        let (t, gate, _) = gated();
        gate.unlock(None).unwrap();
        let notices: Notices = Arc::default();
        let n = notices.clone();
        let emit: Emit = Box::new(move |e| {
            if !matches!(e, TermEvent::Data { .. }) {
                n.lock().unwrap().push(e);
            }
        });
        let root = t.open_gated(&gate, true, "cell1", Target::ContainerShell { container: "vllm-head".into() }, 80, 24, emit).await.unwrap();
        assert_eq!(t.on_relock(RelockReason::WindowClosed, text), 1);
        assert!(!t.shell_authorized(&root));
        settle().await;
        assert!(notices.lock().unwrap().iter().any(|e| matches!(e, TermEvent::Locked { reason } if reason == want)), "{:?}", notices.lock().unwrap());
        t.close(&root);
    }
}

#[tokio::test]
async fn explicit_lock_and_screen_lock_revoke() {
    use crate::admin::RelockReason;
    let (t, gate, _) = gated();
    for (why, text) in [(RelockReason::Explicit, "Locked from the top bar"), (RelockReason::ScreenLock, "The screen was locked"), (RelockReason::Sleep, "The Mac went to sleep"), (RelockReason::Quit, "SparkBox quit")] {
        gate.unlock(None).unwrap();
        let (id, _, notices) = open_shell(&t, &gate).await;
        assert!(gate.lock_with(why, text));
        assert!(!t.shell_authorized(&id), "{why:?}");
        assert_eq!(t.input(&id, b"x".to_vec()).unwrap_err(), LOCKED_INPUT);
        settle().await;
        assert!(notices.lock().unwrap().iter().any(|e| matches!(e, TermEvent::Locked { reason } if reason == text)), "{why:?}: {:?}", notices.lock().unwrap());
        // Authenticating lets the same session type again.
        gate.unlock(None).unwrap();
        t.authorize_shell(&id).unwrap();
        t.input(&id, b"x".to_vec()).unwrap();
        t.close(&id);
    }
}

#[tokio::test]
async fn relock_during_slow_open_leaves_it_locked() {
    let (t, gate, _) = gated();
    gate.unlock(None).unwrap();
    *t.slow_open.lock().unwrap() = Duration::from_millis(300);
    let (t2, g2) = (t.clone(), gate.clone());
    let notices: Notices = Arc::default();
    let n = notices.clone();
    let opening = tokio::spawn(async move {
        let emit: Emit = Box::new(move |e| {
            if !matches!(e, TermEvent::Data { .. }) {
                n.lock().unwrap().push(e);
            }
        });
        t2.open_gated(&g2, true, "cell2", Target::Shell, 80, 24, emit).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Even an idle relock while it connects: the shell waits for the owner.
    assert!(gate.lock_with(crate::admin::RelockReason::Idle, "No changes for 15 minutes"));
    let id = opening.await.unwrap().unwrap();
    assert!(!t.shell_authorized(&id));
    assert_eq!(t.input(&id, b"x".to_vec()).unwrap_err(), LOCKED_INPUT);
    settle().await;
    assert!(notices.lock().unwrap().iter().any(|e| matches!(e, TermEvent::Locked { reason } if reason == LOCKED_WHILE_OPENING)));
    // Locked from the start: refused before anything connects.
    let emit: Emit = Box::new(|_| {});
    *t.slow_open.lock().unwrap() = Duration::ZERO;
    let refused = t.open_gated(&gate, true, "cell2", Target::Shell, 80, 24, emit).await.unwrap_err();
    assert_eq!(crate::errors::SbError::classify(&refused).kind, crate::errors::ErrorKind::Locked);
    // Log followers need nothing.
    let emit: Emit = Box::new(|_| {});
    assert!(t.open_gated(&gate, true, "cell2", Target::ContainerLogs { container: "vllm-worker".into(), since: None }, 80, 24, emit).await.is_ok());
}

/// R1-K (P1.42): the cluster user is in the docker group (root-equivalent), so a Live login
/// shell locks with Admin mode on an idle relock — a Demo one carries on (that path is
/// `admin_idle_relock_leaves_shells_open`).
#[tokio::test]
async fn a_live_login_shell_relocks_on_an_idle_relock() {
    let (_spark, port) = fake_node(&temp("term-live-relock-node"), real_shell()).await;
    let (live, _inv) = connect(&temp("term-live-relock-app"), &[("cell1", port)]).await;
    let clock = FakeClock(Arc::new(AtomicU64::new(1_000_000)));
    let gate = crate::admin::AdminGate::new(Box::new(clock.clone()), Box::new(Yes), None);
    let t = Terminals::new(live);
    t.set_clock(gate.clock());
    let hooks = t.clone();
    gate.on_relock(move |why, text| {
        hooks.on_relock(why, text);
    });
    gate.unlock(None).unwrap();

    let notices: Notices = Arc::default();
    let n = notices.clone();
    let emit: Emit = Box::new(move |e| {
        if !matches!(e, TermEvent::Data { .. }) {
            n.lock().unwrap().push(e);
        }
    });
    let id = t.open_gated(&gate, false, "cell1", Target::Shell, 80, 24, emit).await.unwrap();
    assert!(t.shell_authorized(&id), "a live shell opens authorised in Admin mode");
    clock.advance_s(16 * 60);
    assert!(gate.tick(), "idle relock");
    assert!(!t.shell_authorized(&id), "a Live login shell locks on an idle relock");
    assert_eq!(t.input(&id, b"x".to_vec()).unwrap_err(), LOCKED_INPUT);
    settle().await;
    assert!(
        notices.lock().unwrap().iter().any(|e| matches!(e, TermEvent::Locked { reason } if reason.contains("docker group"))),
        "{:?}",
        notices.lock().unwrap()
    );
}
