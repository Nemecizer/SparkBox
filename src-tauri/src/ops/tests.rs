//! Node actions against fake nodes (real SSH, real shell) and the simulator: what runs,
//! when sudo is used and what it's given, how Fleet's strategies and time limits behave,
//! the classifier, the parsers, the PID guard and the confirmation copy.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::classify::{classify, Kind};
use super::fleet::{FleetSpec, Strategy};
use super::services::{service_impact, NodeCtx};
use super::*;
use crate::demo::DemoCluster;
use crate::fake_node::{connect, fake_node, temp, Script};
use crate::inventory::Inventory;
use crate::tasks::{StepState, TaskState};

/// A fake GB10: kill/renice of root's processes need sudo; systemctl answers in JSON.
fn script() -> Script {
    Arc::new(|cmd, stdin| {
        let cmd = cmd.strip_prefix("LC_ALL=C ").unwrap_or(cmd);
        let sudo = cmd.starts_with("sudo -k -S -p '' ");
        if sudo && stdin != b"pw\n" {
            return Some(("!stderr:Sorry, try again.".into(), 1));
        }
        let cmd = cmd.trim_start_matches("sudo -k -S -p '' ");
        Some(match cmd {
            c if c.starts_with("kill -s TERM 4242") || c.starts_with("renice -n 10 -p 4242") => ("".into(), 0),
            c if c.starts_with("kill ") || c.starts_with("renice ") => {
                if sudo { ("".into(), 0) } else { ("!stderr:kill: (900) - Operation not permitted".into(), 1) }
            }
            "systemctl list-units --type=service --all --no-pager --output=json" => (
                r#"[{"unit":"docker.service","load":"loaded","active":"active","sub":"running","description":"Docker"},
                    {"unit":"ollama.service","load":"loaded","active":"failed","sub":"failed","description":"Ollama Service"}]"#
                    .into(),
                0,
            ),
            "systemctl list-unit-files --type=service --no-pager --output=json" => (
                r#"[{"unit_file":"docker.service","state":"enabled","preset":"enabled"},
                    {"unit_file":"ollama.service","state":"disabled","preset":"enabled"},
                    {"unit_file":"fwupd.service","state":"static","preset":null},
                    {"unit_file":"getty@.service","state":"enabled","preset":"enabled"}]"#
                    .into(),
                0,
            ),
            c if c.starts_with("systemctl restart -- 'ollama.service'") && sudo => ("".into(), 0),
            c if c.starts_with("systemctl restart -- 'nvidia-dgx-telemetry.service'") && sudo => (
                "!stderr:Job for nvidia-dgx-telemetry.service failed because the control process exited with error code.\nSee \"systemctl status nvidia-dgx-telemetry.service\" for details.".into(),
                1,
            ),
            c if c.starts_with("systemctl reboot") && sudo => ("".into(), 0),
            c if c.starts_with("docker restart ") || c.starts_with("docker rm -v ") => ("".into(), 0),
            // The checks before a restart (C6′): the next kernel has its initrd and a matching
            // module, and the compute package is installed (`ii`, then its version).
            "readlink -f /boot/vmlinuz" => ("/boot/vmlinuz-7.0.0-1019-nvidia\n".into(), 0),
            c if c.starts_with("echo '--- initrd'") => ("--- initrd\npresent\n--- modinfo\n580.178.04\n--- compute\nii 580.178.04-1\n--- dkms\n--- modules\n--- kho\n--- grub\nGRUB_DEFAULT=0\n".into(), 0),
            _ => return None,
        })
    })
}

fn demo() -> Arc<Mutex<DemoCluster>> {
    Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())))
}

async fn setup(name: &str) -> (Arc<Ops>, crate::fake_node::Node) {
    let (node, port) = fake_node(&temp(&format!("ops-{name}-home")), script()).await;
    let (live, inventory) = connect(&temp(&format!("ops-{name}-app")), &[("cell1", port)]).await;
    let ops = Ops::with_poll(live.clone(), demo(), Tasks::new(None), Duration::from_millis(5));
    ops.set_snapshot_source(move || Some(live.snapshot(&inventory)));
    (ops, node)
}

/// Ops over the simulator; every look at the snapshot is one tick of simulated time.
fn demo_ops(name: &str) -> (Arc<Ops>, Arc<Mutex<DemoCluster>>) {
    let d = demo();
    let dir = temp(&format!("ops-demo-{name}"));
    let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let live = Live::new(vpn, dir.join("kh.json"), || Ok(None));
    let ops = Ops::with_poll(live, d.clone(), Tasks::new(None), Duration::from_millis(1));
    let src = d.clone();
    ops.set_snapshot_source(move || {
        let mut d = src.lock().unwrap();
        d.advance();
        Some(d.snapshot())
    });
    (ops, d)
}

async fn wait_task(ops: &Ops, id: &str, limit: Duration) -> crate::tasks::Task {
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

#[tokio::test]
async fn processes_are_stopped_with_sudo_only_when_needed() {
    let (ops, node) = setup("signal").await;
    ops.signal(false, "cell1", 4242, "TERM", None).await.unwrap();
    ops.signal(false, "cell1", 900, "KILL", None).await.unwrap();
    ops.renice(false, "cell1", 4242, 10).await.unwrap();
    let stdins = node.stdin.lock().unwrap().clone();
    let cmds: Vec<String> = stdins.iter().map(|(c, _)| c.trim_start_matches("LC_ALL=C ").to_string()).collect();
    assert!(cmds.iter().any(|c| c == "kill -s TERM 4242"));
    assert!(!cmds.iter().any(|c| c.starts_with("sudo") && c.contains("4242")), "own process: no sudo");
    let (_, password) = stdins.iter().find(|(c, _)| c.contains("sudo -k -S -p '' kill -s KILL 900")).unwrap();
    assert_eq!(password, b"pw\n");

    assert!(ops.signal(false, "cell1", 1, "KILL", None).await.unwrap_err().message.contains("process 1"));
    assert!(ops.signal(false, "cell1", 42, "STOP; reboot", None).await.is_err());
    assert!(ops.renice(false, "cell1", 42, -21).await.is_err());

    // Each one was a task, logged under the task's id.
    let tasks = ops.tasks().list();
    assert_eq!(tasks.len(), 3);
    assert!(tasks.iter().all(|t| t.state == TaskState::Ok && t.kind == "process"));
    let log = ops.live.audit.entries();
    let kill = log.iter().find(|e| e.command.contains("kill -s KILL 900") && e.escalated).unwrap();
    assert!(tasks.iter().any(|t| Some(&t.id) == kill.op_id.as_ref()));
}

/// The PID guard, run by a real shell against a fake /proc: the right start time signals
/// the process, a different one (the PID was reused) leaves it alone and exits 3.
#[test]
fn the_pid_guard_only_signals_the_same_process() {
    let root = temp("ops-proc");
    let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    std::fs::create_dir_all(root.join(pid.to_string())).unwrap();
    // Field 2 has a space in it, as some process names do.
    let stat = format!("{pid} (sleep tight) S 1 {pid} {pid} 0 -1 4194304 101 0 0 0 0 0 0 0 20 0 1 0 987654 5926912 120 18446744073709551615");
    std::fs::write(root.join(format!("{pid}/stat")), stat).unwrap();
    let run = |start: u64| {
        let cmd = pid_guard_cmd(pid, "TERM", Some(start), root.to_str().unwrap());
        std::process::Command::new("sh").arg("-c").arg(cmd).output().unwrap()
    };
    let wrong = run(111);
    assert_eq!(wrong.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&wrong.stderr).trim(), "gone");
    assert!(child.try_wait().unwrap().is_none(), "a reused PID isn't touched");

    let right = run(987654);
    assert_eq!(right.status.code(), Some(0));
    let status = child.wait().unwrap();
    assert!(!status.success(), "the process got SIGTERM");
    assert_eq!(pid_guard_cmd(7, "KILL", None, "/proc"), "kill -s KILL 7");
}

#[tokio::test]
async fn services_are_listed_and_changed_with_sudo() {
    let (ops, node) = setup("services").await;
    let list = ops.services(false, "cell1").await.unwrap();
    let names: Vec<&str> = list.iter().map(|s| s.unit.as_str()).collect();
    assert_eq!(names, ["docker.service", "fwupd.service", "ollama.service"], "template units left out");
    let ollama = list.iter().find(|s| s.unit == "ollama.service").unwrap();
    assert_eq!((ollama.active.as_str(), ollama.enabled.as_str()), ("failed", "disabled"));
    assert_eq!(list.iter().find(|s| s.unit == "fwupd.service").unwrap().load, "not-loaded");

    ops.service_action(false, "cell1", "ollama.service", "restart").await.unwrap();
    assert!(node.stdin.lock().unwrap().iter().any(|(c, s)| c.ends_with("sudo -k -S -p '' systemctl restart -- 'ollama.service'") && s == b"pw\n"));
    assert!(ops.service_action(false, "cell1", "x; reboot", "stop").await.is_err());
    assert!(ops.service_action(false, "cell1", "ollama.service", "explode").await.is_err());
    for a in ["reload", "reset-failed", "mask", "unmask"] {
        assert!(services::check_action("ollama.service", a).is_ok(), "{a}");
    }
}

#[tokio::test]
async fn a_reboot_that_never_happens_is_reported() {
    let (ops, node) = setup("power").await;
    // This legacy collector fixture predates boot ids. Give the restart a real baseline.
    let mut snap = ops.cluster().unwrap();
    snap.nodes[0].boot_id = Some("boot-before".into());
    ops.set_snapshot_source(move || Some(snap.clone()));
    let id = ops.power(false, "cell1", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert!(node.log.lock().unwrap().iter().any(|c| c.ends_with("sudo -k -S -p '' systemctl reboot")));
    assert_eq!(t.state, TaskState::Failed);
    assert!(t.error.unwrap().contains("The restart didn't happen"), "the fake node never goes down");
    assert!(ops.power(false, "cell1", "halt; rm -rf /", 0).is_err());
    assert_eq!(power_command("reboot", 5), "shutdown -r +5 'SparkBox: restarting in 5 min'");
}

#[tokio::test]
async fn a_demo_reboot_goes_offline_comes_back_and_starts_its_uptime_over() {
    let (ops, d) = demo_ops("d1");
    let uptime_before = d.lock().unwrap().snapshot().nodes.iter().find(|n| n.id == "cell3").unwrap().system.uptime_s;
    let id = audit::with_op(OpMeta::default(), async { ops.power(true, "cell3", "reboot", 0).unwrap() }).await;
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    // (The checks before a restart are a step of their own: ops/prereboot.rs tests them.)
    let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).filter(|l| !l.starts_with("Checks before the restart") && !l.starts_with("Checks after the restart")).collect();
    assert_eq!(labels, ["Sending the restart to cell3", "Waiting for cell3 to go offline", "Waiting for cell3 to come back (up to 5 min)", "Checking that it restarted"]);
    assert!(t.steps.iter().filter(|s| !s.label.starts_with("Checks before the restart") && !s.label.starts_with("Checks after the restart")).all(|s| s.state == StepState::Ok));
    assert!(t.steps.iter().any(|s| s.label == "Checks after the restart" && s.state == StepState::Skipped), "Demo does not claim hardware checks ran");
    assert!(t.summary.unwrap().starts_with("cell3 is back after "));
    let after = d.lock().unwrap().snapshot().nodes.iter().find(|n| n.id == "cell3").unwrap().system.uptime_s;
    assert!(after < uptime_before);

    // The activity log has the operation (marked simulated) with the command it stood for.
    let log = ops.live.audit.entries();
    let mine: Vec<_> = log.iter().filter(|e| e.op_id.as_deref() == Some(id.as_str())).collect();
    assert!(mine.iter().any(|e| e.kind == "op" && e.state == "ok" && e.simulated));
    assert!(mine.iter().any(|e| e.command == "sudo systemctl reboot" && e.simulated && e.escalated));
}

#[tokio::test]
async fn a_scheduled_restart_can_be_called_off() {
    let (ops, _) = demo_ops("d2");
    let id = ops.power(true, "cell2", "reboot", 1).unwrap();
    // Once it's scheduled on the node: Cancel before that has nothing to call off there.
    let sent = std::time::Instant::now();
    while !ops.live.audit.entries().iter().any(|e| e.command.starts_with("sudo shutdown -r +1 ")) {
        assert!(sent.elapsed() < Duration::from_secs(10), "never scheduled: {:?}", ops.tasks().get(&id));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    ops.tasks().cancel(&id).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(5)).await;
    assert_eq!(t.state, TaskState::Cancelled);
    assert!(ops.live.audit.entries().iter().any(|e| e.command == "sudo shutdown -c"));
    assert!(ops.live.audit.entries().iter().any(|e| e.command.starts_with("sudo shutdown -r +1 ")));
}

#[tokio::test]
async fn fleet_stops_a_command_at_its_time_limit_on_the_node() {
    let (ops, _node) = setup("fleet-timeout").await;
    let started = Instant::now();
    // Its own text: pgrep sees the whole Mac, and another copy of this suite may be running this.
    let nap = format!("sleep 30.{}", std::process::id());
    let results = ops.fleet(false, &["cell1".into()], &nap, false, 1).await;
    assert!(started.elapsed() < Duration::from_secs(7), "took {:?}", started.elapsed());
    assert!(results[0].timed_out, "{:?}", results[0]);
    assert_eq!(results[0].state, "timedOut");
    assert_eq!(results[0].code, Some(124));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let left = std::process::Command::new("pgrep").args(["-f", &nap]).output().unwrap();
    assert!(String::from_utf8_lossy(&left.stdout).trim().is_empty(), "the node killed it");
}

#[tokio::test]
async fn fleet_wraps_commands_and_reports_each_node() {
    let (ops, node) = setup("fleet").await;
    let results = ops.fleet(false, &["cell1".into(), "cell9".into()], "echo hi; echo oops >&2; exit 2", false, 30).await;
    assert_eq!(results[0].stdout, "hi\n");
    assert_eq!(results[0].stderr, "oops\n");
    assert_eq!((results[0].code, results[0].state.as_str()), (Some(2), "failed"));
    assert!(results[1].error.as_deref().unwrap().contains("isn't connected"));
    assert!(node.log.lock().unwrap().iter().any(|c| c == "timeout --signal=TERM --kill-after=5s 30s sh -c 'echo hi; echo oops >&2; exit 2'"));

    let results = ops.fleet(false, &["cell1".into()], "id -u", true, 30).await;
    assert!(node.log.lock().unwrap().iter().any(|c| c == "sudo -k -S -p '' env LC_ALL=C timeout --signal=TERM --kill-after=5s 30s sh -c 'id -u'"));
    assert_eq!(results[0].code, Some(0));

    // Output past 1 MiB is cut, and the result says so.
    let results = ops.fleet(false, &["cell1".into()], "head -c 1100000 /dev/zero | tr '\\0' x", false, 30).await;
    assert!(results[0].truncated);
    assert_eq!(results[0].stdout.len(), fleet::OUTPUT_CAP);
}

#[tokio::test]
async fn one_at_a_time_runs_in_order_and_stops_at_a_failure() {
    let (ops, _) = demo_ops("d3");
    let events = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let e = events.clone();
    ops.set_emitter(move |name, v| {
        if name == "fleet://node" {
            e.lock().unwrap().push((v["node"].as_str().unwrap().into(), v["state"].as_str().unwrap().into()));
        }
    });
    let nodes: Vec<String> = ["cell1", "cell2", "cell3", "gx10-0000"].map(String::from).to_vec();
    // `false` isn't something the simulator knows, so it fails on the first node.
    let spec = FleetSpec { nodes: nodes.clone(), command: "frobnicate".into(), sudo: false, timeout_s: 10, strategy: Strategy::OneByOne, stop_on_failure: true, title: None, cuts_access: false };
    let id = ops.fleet_start(true, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.state, TaskState::Failed);
    let results = ops.fleet_results(&id).unwrap();
    assert_eq!(results.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(), ["failed", "skipped", "skipped", "skipped"]);
    let started: Vec<String> = events.lock().unwrap().iter().filter(|(_, s)| s == "running").map(|(n, _)| n.clone()).collect();
    assert_eq!(started, ["cell1"]);

    // Without stop-on-failure they all run, in order.
    events.lock().unwrap().clear();
    let spec = FleetSpec { nodes: nodes.clone(), command: "hostname".into(), sudo: false, timeout_s: 10, strategy: Strategy::OneByOne, stop_on_failure: true, title: None, cuts_access: false };
    let id = ops.fleet_start(true, spec).unwrap();
    assert_eq!(wait_task(&ops, &id, Duration::from_secs(10)).await.state, TaskState::Ok);
    let started: Vec<String> = events.lock().unwrap().iter().filter(|(_, s)| s == "running").map(|(n, _)| n.clone()).collect();
    assert_eq!(started, nodes);
    assert!(ops.fleet_results(&id).unwrap()[1].stdout.contains("cell2"));

    // Canary: a failing first node leaves the rest untouched.
    let spec = FleetSpec { nodes: nodes.clone(), command: "frobnicate".into(), sudo: false, timeout_s: 10, strategy: Strategy::Canary, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(true, spec).unwrap();
    wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(ops.fleet_results(&id).unwrap().iter().filter(|r| r.state == "skipped").count(), 3);
}

#[tokio::test]
async fn every_built_in_snippet_works_in_demo_and_differs_per_node() {
    const SNIPPETS: &[&str] = &[
        "nvidia-smi --query-gpu=name,driver_version,utilization.gpu,temperature.gpu,power.draw --format=csv,noheader",
        "uptime",
        "df -h / /home 2>/dev/null",
        "free -h",
        "docker ps --format 'table {{.Names}}\\t{{.Image}}\\t{{.Status}}'",
        "apt list --upgradable 2>/dev/null",
        "uname -srm",
        "for i in /sys/class/net/enp1s0f*np* /sys/class/net/enP2p1s0f*np*; do [ -e \"$i\" ] && echo \"$(basename $i) $(cat $i/operstate) $(cat $i/speed 2>/dev/null)Mb/s\"; done",
    ];
    let (ops, _) = demo_ops("d4");
    let nodes: Vec<String> = ["cell1", "cell2", "cell3", "gx10-0000"].map(String::from).to_vec();
    for s in SNIPPETS {
        assert_eq!(classify(s), Kind::Read, "built-in snippets only read: {s}");
        let results = ops.fleet(true, &nodes, s, false, 30).await;
        assert!(results.iter().all(|r| r.code == Some(0)), "{s}: {results:?}");
    }
    let gpu = ops.fleet(true, &nodes, SNIPPETS[0], false, 30).await;
    assert_ne!(gpu[0].stdout, gpu[3].stdout, "each node answers for itself");
}

#[tokio::test]
async fn a_bulk_container_restart_is_one_task_with_a_step_each() {
    let (ops, d) = demo_ops("d5");
    let snap = d.lock().unwrap().snapshot();
    let mut items = Vec::new();
    for n in &snap.nodes {
        for c in n.containers.iter().filter(|c| c.project.as_deref() == Some("llama-70b")) {
            items.push(containers::Target { node: n.id.clone(), id: c.id.clone(), name: c.name.clone() });
        }
    }
    assert_eq!(items.len(), 3, "the llama deployment spans three nodes");
    let id = ops.containers_bulk(true, items, "restart".into(), false, Some("Restart 3 containers".into())).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(5)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    assert_eq!(t.steps.len(), 3);
    assert_eq!(t.summary.as_deref(), Some("3 of 3 restarted"));
    assert_eq!(ops.tasks().list().len(), 1, "one parent task");
    let logged: Vec<_> = ops.live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(id.as_str()) && e.simulated).collect();
    assert_eq!(logged.iter().filter(|e| e.kind != "op").count(), 3);
    // The group's own line carries the operation's title and outcome (not "Restart vllm-worker on cell2").
    let head = logged.iter().find(|e| e.kind == "op").expect("the op's head line");
    assert_eq!((head.title.as_str(), head.state.as_str(), head.stdout_tail.as_deref()), ("Restart 3 containers", "ok", Some("3 of 3 restarted")));

    // Removing a running container is refused; an exited one goes.
    let sglang = snap.nodes.iter().flat_map(|n| n.containers.iter().map(move |c| (n, c))).find(|(_, c)| c.name == "sglang-test").unwrap();
    assert_eq!(sglang.1.exit_code, 137);
    let ollama = snap.nodes.iter().flat_map(|n| n.containers.iter().map(move |c| (n, c))).find(|(_, c)| c.name == "ollama").unwrap();
    assert!(ops.container_action(true, &ollama.0.id, &ollama.1.id, "remove", false).await.unwrap_err().message.contains("stop it"));
    ops.container_action(true, &sglang.0.id, &sglang.1.id, "remove", true).await.unwrap();
    assert!(ops.live.audit.entries().iter().any(|e| e.command == format!("docker rm -v {}", sglang.1.id)));
}

#[tokio::test]
async fn container_actions_on_a_live_node_go_through_the_log() {
    let (ops, node) = setup("containers").await;
    let id = "0123456789ab";
    ops.container_action(false, "cell1", id, "restart", false).await.unwrap();
    assert!(node.log.lock().unwrap().iter().any(|c| c.ends_with(&format!("docker restart {id}"))));
    assert!(ops.live.audit.entries().iter().any(|e| e.command.ends_with(&format!("docker restart {id}")) && e.state == "ok"));
    assert!(ops.container_action(false, "cell1", "abc; reboot", "stop", false).await.is_err());
    assert!(ops.container_action(false, "cell1", id, "exec", false).await.is_err());
}

#[test]
fn the_classifier_knows_what_only_reads_what_changes_and_what_destroys() {
    use Kind::*;
    let table: &[(&str, Kind)] = &[
        // Probes are tagged where SparkBox runs them, never guessed from the text.
        ("true", Read),
        ("sha256sum ~/.sparkbox/collector.py 2>/dev/null", Read),
        ("python3 ~/.sparkbox/sparkjob.py list", Read),
        ("python3 ~/.sparkbox/sparkjob.py read abc", Read),
        // The whole model folder is data loss (r4); one model in it is a change.
        ("python3 ~/.sparkbox/sparkjob.py list; rm -rf ~/models", Destructive),
        ("curl -s -o /dev/null -m 3 http://x; dd if=/dev/zero of=/dev/nvme0n1", Destructive),
        ("python3 ~/.sparkbox/sparkjob.py start", Change),
        // Reads.
        ("uptime", Read),
        ("cat /proc/loadavg", Read),
        ("head -25 /proc/meminfo", Read),
        ("tail -n 5 /sys/class/net/enp1s0f1np1/statistics/rx_bytes", Read),
        ("ls -la /models", Read),
        ("df -h / /home 2>/dev/null", Read),
        ("free -h", Read),
        ("uname -srm", Read),
        ("hostname", Read),
        ("hostname newname", Change),
        ("nvidia-smi", Read),
        ("nvidia-smi --query-gpu=name,driver_version --format=csv,noheader", Read),
        ("nvidia-smi -q -d CLOCK", Read),
        ("nvidia-smi -pl 100", Change),
        // R1-K (P1.42): a GPU reset tears down whatever is on the GPU — Destructive now.
        ("nvidia-smi -r", Destructive),
        ("nvidia-smi -i 0 -ac 2619,1980", Change),
        ("docker ps --format 'table {{.Names}}\\t{{.Status}}'", Read),
        ("docker inspect vllm-head", Read),
        ("docker logs --tail 50 vllm-head", Read),
        ("docker images", Read),
        ("docker version --format '{{.Server.Version}}'", Read),
        ("docker compose ps", Read),
        ("docker compose down", Change),
        ("docker restart vllm-head", Change),
        ("systemctl status docker", Read),
        ("systemctl list-units --failed", Read),
        ("systemctl is-active ssh", Read),
        ("systemctl --failed", Read),
        ("systemctl restart docker", Change),
        ("journalctl -u docker -n 50 --no-pager", Read),
        ("journalctl --vacuum-time=1s", Change),
        ("journalctl --rotate", Change),
        ("ip -br addr", Read),
        ("ip addr show enp1s0f1np1", Read),
        ("ip -s link", Read),
        ("ip link set x down", Destructive),
        ("ip route add 10.0.0.0/8 via 192.168.50.1", Change),
        ("ss -tlnp", Read),
        ("apt list --upgradable 2>/dev/null", Read),
        ("apt-get -s dist-upgrade", Read),
        ("apt-get update", Change),
        ("dpkg -l | grep nvidia", Read),
        // An upgrade's check afterwards (ops/updates.rs): it reports, and changes nothing.
        ("dpkg --audit", Read),
        ("id", Read),
        ("whoami", Read),
        ("cat /dev/null > /etc/hosts", Change),
        ("echo hi >> ~/.bashrc", Change),
        ("cat /proc/cpuinfo > /dev/null 2>&1", Read),
        ("echo x > /dev/null", Read),
        ("free -h | tee /tmp/x", Change),
        ("sed -i 's/a/b/' /etc/hosts", Change),
        ("sed 's/a/b/' /etc/hosts", Read),
        ("echo $(cat /etc/hostname)", Read),
        ("echo $(rm -rf /tmp/x)", Change),
        ("sudo -n journalctl -n 20", Read),
        ("ps aux | grep vllm | head -5", Read),
        ("find /models -name '*.json'", Read),
        ("find /tmp -delete", Change),
        ("curl -s http://localhost:8000/v1/models", Read),
        ("curl -X POST http://localhost:8000/v1/completions -d '{}'", Change),
        ("bash -c 'uptime'", Read),
        ("for i in /sys/class/net/*; do echo $(basename $i) $(cat $i/operstate); done", Read),
        ("timeout 5 nvidia-smi", Read),
        ("LC_ALL=C df -h", Read),
        ("rm -rf /tmp/cache", Change),
        ("rm -rf ~/models/old-model", Change),
        ("rm /etc/hosts", Change),
        ("kill -9 1234", Change),
        ("kill -1 1234", Change),
        ("shutdown -c", Change),
        ("docker image prune -f", Change),
        ("chmod -R 755 ~/models", Change),
        ("dd if=/dev/zero of=/tmp/test.img bs=1M count=10", Change),
        ("uniq a.txt", Read),
        ("sort -u x | uniq", Read),
        ("uniq a.txt b.txt", Change),
        ("uniq -c a.txt", Read),
        ("fdisk -l", Read),
        ("parted -l", Read),
        ("parted /dev/nvme0n1 print", Read),
        ("sgdisk -p /dev/nvme0n1", Read),
        ("sfdisk --dump /dev/sda", Read),
        // Destructive: disks.
        ("dd if=/dev/zero of=/dev/nvme0n1", Destructive),
        ("sudo dd if=/dev/zero of=/dev/nvme0n1 bs=1M status=progress", Destructive),
        ("dd if=image.iso of=/dev/sdb", Destructive),
        ("mkfs.ext4 /dev/nvme0n1p1", Destructive),
        ("sudo mkfs -t xfs /dev/sdb", Destructive),
        ("mkswap /dev/nvme0n1p3", Destructive),
        ("wipefs -a /dev/sda", Destructive),
        ("parted /dev/nvme0n1 mklabel gpt", Destructive),
        ("sudo fdisk /dev/sda", Destructive),
        ("sgdisk --zap-all /dev/nvme0n1", Destructive),
        ("blkdiscard /dev/nvme0n1", Destructive),
        ("shred -n 1 /dev/sda", Destructive),
        ("cat /dev/zero > /dev/sda", Destructive),
        ("echo x > /dev/nvme0n1", Destructive),
        ("uptime | sudo tee /dev/sda", Destructive),
        // Destructive: deletes.
        ("rm -rf /", Destructive),
        ("rm -rf /*", Destructive),
        ("rm -rf ~", Destructive),
        ("rm -rf ~/", Destructive),
        ("rm -fr $HOME", Destructive),
        ("rm -rf \"$HOME\"/*", Destructive),
        ("rm -r /home", Destructive),
        ("sudo rm -Rf /etc", Destructive),
        ("rm -rf /var/", Destructive),
        ("rm -rf /boot/*", Destructive),
        ("rm --recursive --force /usr", Destructive),
        ("rm -rf --no-preserve-root /", Destructive),
        ("find / -name '*.log' -delete", Destructive),
        ("find ~ -type f -exec rm {} +", Destructive),
        ("chmod -R 777 /", Destructive),
        ("chown -R nobody:nogroup /", Destructive),
        ("docker system prune -af", Destructive),
        ("docker volume rm models", Destructive),
        ("docker volume prune -f", Destructive),
        // Destructive: taking the node down.
        ("shutdown -h now", Destructive),
        ("sudo reboot", Destructive),
        ("poweroff", Destructive),
        ("halt", Destructive),
        ("init 0", Destructive),
        ("sudo init 6", Destructive),
        ("systemctl reboot", Destructive),
        ("systemctl poweroff", Destructive),
        ("sudo systemctl kexec", Destructive),
        ("kill -9 1", Destructive),
        ("kill -KILL 1", Destructive),
        ("kill -9 -1", Destructive),
        ("kill -s KILL 1", Destructive),
        (":(){ :|:& };:", Destructive),
        ("bomb() { bomb | bomb & }; bomb", Destructive),
        // Hidden in a list, a pipe, a substitution, a subshell or a wrapper.
        ("true; reboot", Destructive),
        ("uptime && reboot", Destructive),
        ("uptime; dd if=/dev/zero of=/dev/nvme0n1", Destructive),
        ("uptime && rm -rf /", Destructive),
        ("uptime || reboot", Destructive),
        ("echo $(reboot)", Destructive),
        ("echo `poweroff`", Destructive),
        ("(cd /tmp && rm -rf /)", Destructive),
        ("bash -c 'dd if=/dev/zero of=/dev/sda'", Destructive),
        ("sh -c \"rm -rf ~\"", Destructive),
        ("sh -c 'rm -rf /'", Destructive),
        ("nohup reboot &", Destructive),
        ("timeout 5 reboot", Destructive),
        ("env FOO=1 reboot", Destructive),
        ("FOO=1 reboot", Destructive),
        ("xargs rm -rf / < list", Destructive),
        ("watch reboot", Destructive),
        ("doas reboot", Destructive),
        ("pkexec rm -rf /etc", Destructive),
        ("su -c 'reboot'", Destructive),
        ("if true; then reboot; fi", Destructive),
        ("uptime\nreboot", Destructive),
        // Quoting SparkBox can't read: bash would still run the lines before it.
        ("echo 'unbalanced", Destructive),
        ("dd if=/dev/zero of=/dev/sda\necho 'oops", Destructive),
    ];
    assert!(table.len() >= 80, "{} cases", table.len());
    let wrong: Vec<String> = table.iter().filter(|(c, k)| classify(c) != *k).map(|(c, k)| format!("{c:?}: expected {k:?}, got {:?}", classify(c))).collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // Destructive ones say why, for the banner.
    let why = super::classify::verdict("uptime; dd if=/dev/zero of=/dev/nvme0n1").reason;
    assert_eq!(why.as_deref(), Some("Writes straight to the disk device /dev/nvme0n1 (dd)."));
    assert_eq!(super::classify::verdict("rm -rf ~").reason.as_deref(), Some("Deletes ~ and everything in it (rm -r): the node stops working."));
}

/// The security critic's round-2 cases: tools on the read-only list used in ways that write a
/// file or run a command are changes (Fleet asks first, the activity log shows them).
#[test]
fn the_read_tier_trusts_no_tool_that_writes_or_runs() {
    use Kind::*;
    let table: &[(&str, Kind)] = &[
        // sed: w, W, e and the s///e and s///w flags; -i and script files.
        ("sudo sed -n 'w /etc/shadow' /etc/hosts", Change),
        ("sudo sed '1e rm -rf /tmp/x' /etc/hostname", Change),
        ("sed 's/.*/reboot/e' f", Change),
        ("sed 's/a/b/w /tmp/out' f", Change),
        ("sed -n '/x/W /tmp/out' f", Change),
        ("sed -e 's/a/b/' -e '$e reboot' f", Change),
        ("sed -ne 's/a/b/gw out' f", Change),
        ("sed -i.bak 's/a/b/' f", Change),
        ("sed -f script.sed f", Change),
        ("sed ':a;w /tmp/x' f", Change),
        ("sed -n '/start/,/end/p' f", Read),
        ("sed 's/a/b/g; s|x|y|2' f", Read),
        ("sed -E 's/([0-9]+)/<\\1>/gI' f", Read),
        ("sed '$d' f", Read),
        ("sed ':a;N;$!ba;s/\\n/ /g' f", Read),
        ("sed -n '5q;p' f", Read),
        // awk: system with any spacing, pipes both ways, print redirects, program files.
        // The command inside system() is judged by the same rules (r4): a reboot is destructive.
        ("awk 'BEGIN{system (\"reboot\")}'", Destructive),
        ("awk 'BEGIN{system(\"reboot\")}'", Destructive),
        ("awk 'BEGIN{system(\"date\")}'", Change),
        ("awk 'BEGIN { system\t(\"id\") }'", Change),
        ("sudo awk 'BEGIN{f=\"/etc/shadow\"; print \"x\" > f}'", Change),
        ("awk '{print $1 >> \"/tmp/x\"}' f", Change),
        ("awk '{print $1 > 50}' f", Change),
        ("awk 'BEGIN{\"date\" | getline d; print d}'", Change),
        ("awk '{print | \"sh\"}' f", Change),
        ("gawk 'BEGIN{\"sh\" |& getline}'", Change),
        ("awk -f prog.awk f", Change),
        ("gawk -i inplace '{print}' f", Change),
        ("awk '{print $1 / 2 > \"f\"}' x", Change),
        ("awk -F: '$3 > 999 {print $1}' /etc/passwd", Read),
        ("awk '{print ($1 > 50)}' f", Read),
        ("awk '/a|b/ {print $2}' f", Read),
        ("awk '$1 == \"x|y>z\" || NR > 3 {print}' f", Read),
        ("awk '{s += $2} END {printf \"%d\\n\", s}' f", Read),
        // Interpreters with a program on the command line.
        ("perl -e 'print 1'", Change),
        ("python3 -c 'import os; os.system(\"id\")'", Change),
        ("ruby -e 'puts 1'", Change),
        ("node -e 'console.log(1)'", Change),
        ("lua -e 'print(1)'", Change),
        ("python3 --version", Read),
        // Setters and writers on otherwise read-only tools.
        // Writing over /etc/passwd is worse than a change: it cuts access.
        ("sudo nvidia-smi -q -f /etc/passwd", Destructive),
        ("nvidia-smi -q -f /tmp/gpu.txt", Change),
        ("nvidia-smi -q --filename=/tmp/x", Change),
        // R1-K (P1.42): a GPU reset tears down whatever is on the GPU — Destructive now.
        ("nvidia-smi --gpu-reset -i 0", Destructive),
        ("sudo ss -K dport = 22", Change),
        ("ss -tK", Change),
        ("ss -tnp", Read),
        ("sudo ip -batch /tmp/cmds", Change),
        ("ip -b /tmp/cmds", Change),
        ("ip -force -batch /tmp/cmds", Change),
        ("git log --output=/etc/passwd", Destructive),
        ("git log --output=/tmp/log.txt", Change),
        ("git -c core.pager='sh -c reboot' log", Change),
        ("git log -o /tmp/x", Change),
        ("git log --oneline -5", Read),
        ("xxd dump.bin /etc/hosts", Change),
        ("iconv -f utf-8 -t ascii -o /etc/hosts x", Change),
        ("tar -tvf x.tar --checkpoint-action=exec=reboot", Change),
        ("tar -tf x.tar", Read),
        ("tar tvzf x.tgz", Read),
        ("tar -tIf sh x.tar", Change),
        ("tar -xf x.tar", Change),
        ("rg --pre ./run.sh x", Change),
        ("sort --compress-program=sh x", Change),
        ("curl -c /tmp/jar http://x", Change),
        ("curl -D /etc/hosts http://x", Change),
        ("curl -sD - http://x", Read),
        ("wget -O- -o /tmp/log http://x", Change),
        ("journalctl --cursor-file=/etc/hosts", Change),
        ("dpkg -l -i x.deb", Change),
        ("apt-get -s -o APT::Update::Pre-Invoke::=reboot update", Change),
        ("apt-get -s update", Change),
        ("nvcc -o x x.cu", Change),
        ("nvidia-ctk runtime configure --runtime=docker", Change),
        ("less +!reboot f", Change),
        ("env -S 'rm -rf /tmp/x'", Change),
        // Assignments that make the next program load or run something.
        ("env LD_PRELOAD=/tmp/x.so ls", Change),
        ("sudo env LD_PRELOAD=/tmp/x.so ls", Change),
        ("LD_PRELOAD=/tmp/x.so ls", Change),
        ("LD_LIBRARY_PATH=/tmp ls", Change),
        ("PATH=/tmp/evil:$PATH ls", Change),
        ("BASH_ENV=/tmp/x bash -c 'uptime'", Change),
        ("ENV=/tmp/x sh -c 'uptime'", Change),
        ("PYTHONPATH=/tmp python3 --version", Change),
        ("PERL5OPT=-Mx cat f", Change),
        ("NODE_OPTIONS='--require /tmp/x' cat f", Change),
        ("export PATH=/tmp:$PATH; ls", Change),
        // Assignments are an allowlist (r4): anything but the locale, TZ, COLUMNS, TERM and NO_COLOR.
        ("FOO=1 ls", Change),
        ("export FOO=1; ls", Change),
        ("LC_ALL=C ls", Read),
    ];
    let wrong: Vec<String> = table.iter().filter(|(c, k)| classify(c) != *k).map(|(c, k)| format!("{c:?}: expected {k:?}, got {:?}", classify(c))).collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Commands that may leave SparkBox unable to log in again: destructive, flagged as cutting
/// access (Fleet: canary, then reach the node over SSH again, then one at a time), with why.
#[test]
fn commands_that_cut_access_are_destructive_and_say_so() {
    use super::classify::verdict;
    let cuts: &[(&str, &str)] = &[
        ("sudo systemctl stop ssh", "Stops ssh"),
        ("systemctl disable --now sshd.service", "Stops sshd.service"),
        ("sudo systemctl mask ssh.socket", "Stops ssh.socket"),
        ("systemctl kill sshd", "Stops sshd"),
        ("sudo systemctl stop NetworkManager", "Stops NetworkManager"),
        ("sudo service ssh stop", "Stops ssh"),
        ("sudo /etc/init.d/ssh stop", "Stops ssh"),
        ("sudo killall sshd", "Kills the SSH server"),
        ("pkill -9 sshd", "Kills the SSH server"),
        ("sudo apt-get purge -y openssh-server", "Removes the SSH server"),
        ("apt remove openssh-server", "Removes the SSH server"),
        ("sudo dpkg --purge openssh-server", "Removes the SSH server"),
        ("sudo ip link set enp1s0f1np1 down", "Takes a network interface down"),
        ("ip l s eth0 down", "Takes a network interface down"),
        ("ip addr flush dev eth0", "Removes network addresses"),
        ("ip route del default", "Removes network addresses or routes"),
        ("sudo ifdown eth0", "Takes a network interface down"),
        ("ifconfig eth0 down", "Takes a network interface down"),
        ("nmcli networking off", "Takes the network down"),
        ("nmcli con down 'Wired connection 1'", "Takes the network down"),
        ("sudo iptables -F", "Flushes the firewall"),
        ("iptables -X", "Flushes the firewall"),
        ("sudo iptables -P INPUT DROP", "Sets a firewall chain to drop everything"),
        ("iptables -A INPUT -p tcp --dport 22 -j DROP", "Adds a firewall rule that drops traffic"),
        ("sudo nft flush ruleset", "Replaces or flushes the firewall"),
        ("sudo ufw enable", "Changes the firewall (ufw enable)"),
        ("ufw default deny incoming", "Changes the firewall"),
        ("ufw reset", "Changes the firewall"),
        ("ufw deny ssh", "Changes the firewall"),
        ("echo b > /proc/sysrq-trigger", "Writes to /proc/sysrq-trigger"),
        ("echo c | sudo tee /proc/sysrq-trigger", "Writes to /proc/sysrq-trigger"),
        ("truncate -s 0 /etc/passwd", "Changes /etc/passwd"),
        ("> /etc/fstab", "Changes /etc/fstab"),
        ("echo 'x ALL=(ALL) NOPASSWD:ALL' | sudo tee -a /etc/sudoers.d/x", "Changes /etc/sudoers.d/x"),
        ("sudo sed -i 's/^PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config", "Changes /etc/ssh/sshd_config"),
        ("sudo cp /tmp/x /etc//shadow", "Changes /etc/shadow"),
        ("sudo chmod 000 /etc/group", "Changes /etc/group"),
        ("sudo vim /etc/ssh/sshd_config.d/10-x.conf", "Changes /etc/ssh/sshd_config.d/10-x.conf"),
        ("sudo passwd sparkuser", "Changes a password"),
        ("echo 'sparkuser:x' | sudo chpasswd", "Sets passwords"),
        ("sudo usermod -L sparkuser", "Locks an account"),
        ("sudo chage -E 0 sparkuser", "Changes when a password or account expires"),
        ("sudo mv /usr /usr.bak", "Moves /usr"),
        ("rm -r /home/sparkuser", "Deletes /home/sparkuser"),
        ("rm -rf /", "Deletes / and everything"),
        ("sudo chown -R nobody /etc", "Changes the owner or permissions of every file under /etc"),
        ("sudo systemctl isolate rescue.target", "Switches the node to another target"),
        ("sudo sh -c 'systemctl stop ssh'", "Stops ssh"),
    ];
    let wrong: Vec<String> = cuts
        .iter()
        .filter_map(|(c, why)| {
            let v = verdict(c);
            let ok = v.kind == Kind::Destructive && v.cuts_access && v.reason.as_deref().is_some_and(|r| r.starts_with(why));
            (!ok).then(|| format!("{c:?}: {v:?} (wanted it to start {why:?})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // Destructive but not a lockout; power verbs are flagged so several nodes at once cut access.
    use super::classify::Power;
    for (c, power) in [("mkfs.ext4 /dev/nvme0n1p1", None), ("shutdown -h now", Some(Power::Off)), ("sudo reboot", Some(Power::Restart)), ("shutdown -r +1", Some(Power::Restart)), ("systemctl poweroff", Some(Power::Off)), ("uptime && sudo systemctl reboot", Some(Power::Restart)), ("dd if=/dev/zero of=/dev/sda", None)] {
        let v = verdict(c);
        assert!(v.kind == Kind::Destructive && !v.cuts_access && v.power == power, "{c}: {v:?}");
    }
    // Reads of the same files, and copies away from them, don't.
    for c in ["cat /etc/passwd", "grep sparkuser /etc/shadow", "cp /etc/passwd /tmp/p", "sudo sshd -T", "systemctl status ssh", "systemctl restart ssh", "mv ~/x /etc/", "ip link show", "iptables -L -n", "ufw status", "chage -l sparkuser", "ls /etc/sudoers.d"] {
        let v = verdict(c);
        assert!(!v.cuts_access, "{c}: {v:?}");
    }
}

/// r4 F3: every Fleet classifier case the round-3 security, PE and QA critics raised
/// (feedback.md), as a fixture. Read means Fleet runs it on every node at once with no dialog,
/// so each Change here is one the Read tier used to let through.
#[test]
fn every_round_3_classifier_case_is_a_fixture() {
    use super::classify::verdict;
    use Kind::*;
    // (command, class, cuts access)
    let table: &[(&str, Kind, bool)] = &[
        // PE: an option's value attached to it, or short flags run together (getopt).
        ("sudo date -s2030-01-01", Change, false),
        ("sort -o/etc/hosts /etc/hosts", Change, false),
        ("sort -o/etc/passwd /dev/null", Destructive, true),
        ("sort -o/home/sparkuser/.bashrc /dev/null", Change, false),
        ("wget -o/tmp/log -O- http://x", Change, false),
        ("sudo dmesg -Hc", Change, false),
        ("sudo dmesg -Tc", Change, false),
        ("iconv -o/tmp/x -f utf8 -t ascii f", Change, false),
        ("sar -o/tmp/sa 1 1", Change, false),
        ("sudo smartctl -tlong /dev/nvme0", Change, false),
        ("dmesg -T", Read, false),
        ("sort -k2 -t: /etc/passwd", Read, false),
        ("sort -rn x", Read, false),
        ("date +%s", Read, false),
        ("date -d '1 day ago'", Read, false),
        ("smartctl -a /dev/nvme0", Read, false),
        // Security: assignments that point a program at other code or config, and sudo -E.
        ("GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.pager GIT_CONFIG_VALUE_0=sh git log", Change, false),
        ("GIT_DIR=/tmp/x git status", Change, false),
        ("HOME=/tmp git status", Change, false),
        ("XDG_CONFIG_HOME=/tmp git log", Change, false),
        ("SYSTEMD_PAGER=sh systemctl status", Change, false),
        ("GCONV_PATH=/tmp iconv -f x -t y f", Change, false),
        ("CUDA_INJECTION64_PATH=/tmp/x.so nvidia-smi", Change, false),
        ("sudo -E ls", Change, false),
        ("sudo -nE ls", Change, false),
        ("sudo --preserve-env=PATH ls", Change, false),
        ("TZ=UTC date", Read, false),
        ("COLUMNS=200 ps aux", Read, false),
        ("NO_COLOR=1 TERM=dumb ls", Read, false),
        ("LANG=C.UTF-8 ls", Read, false),
        // Security: gawk directives and indirect calls.
        ("gawk '@load \"filefuncs\"; BEGIN { print 1 }'", Change, false),
        ("awk '@include \"x.awk\"'", Change, false),
        ("gawk 'function f(x) { print x } BEGIN { g = \"f\"; @g(1) }'", Change, false),
        ("awk '{ print \"a@b\" }' f", Read, false),
        // Security: git verbs that move refs, and *ctl verbs that set.
        ("git branch evil", Change, false),
        ("git branch -D main", Change, false),
        ("git tag -f v1", Change, false),
        ("git tag v2 HEAD", Change, false),
        ("git remote rename origin x", Change, false),
        ("git remote update", Change, false),
        ("git remote prune origin", Change, false),
        ("git -C /srv/repo status", Change, false),
        ("git branch", Read, false),
        ("git branch -a -v", Read, false),
        ("git tag", Read, false),
        ("git tag -l 'v*'", Read, false),
        ("git remote -v", Read, false),
        ("git log --oneline -5", Read, false),
        ("resolvectl dns eth0 1.1.1.1", Change, false),
        ("resolvectl domain eth0 example.com", Change, false),
        ("hostnamectl hostname evil", Change, false),
        ("resolvectl dns", Read, false),
        ("resolvectl status", Read, false),
        ("hostnamectl", Read, false),
        ("timedatectl status", Read, false),
        ("networkctl status enP7s7", Read, false),
        ("nmcli -g GENERAL.CONNECTION device show enP7s7", Read, false),
        ("nmcli -g 802-3-ethernet.wake-on-lan connection show 'Wired connection 1'", Read, false),
        ("nmcli d s", Read, false),
        ("nmcli general", Read, false),
        ("nmcli radio wifi", Read, false),
        ("nmcli radio wifi off", Destructive, true),
        ("nmcli connection modify x 802-3-ethernet.wake-on-lan magic", Change, false),
        ("nmcli device disconnect enP7s7", Destructive, true),
        ("nmcli general hostname evil", Change, false),
        ("nmcli networking off", Destructive, true),
        // Security: options that write a file.
        ("curl -s -w '%output{/etc/hosts}x' http://x", Change, false),
        ("curl -sw%output{/tmp/x} http://x", Change, false),
        ("curl -s -w '%{http_code}' http://x", Read, false),
        ("less -o /tmp/x f", Change, false),
        ("less -o/tmp/x f", Change, false),
        ("less --log-file=/tmp/x f", Change, false),
        ("less f", Read, false),
        // QA: lockouts the Cuts access tier missed.
        ("rm ~/.ssh/authorized_keys", Destructive, true),
        (": > ~/.ssh/authorized_keys", Destructive, true),
        ("sudo rm -f /home/sparkuser/.ssh/authorized_keys", Destructive, true),
        ("echo ssh-ed25519 AAAA >> ~/.ssh/authorized_keys", Destructive, true),
        ("sed -i '/sparkuser/d' ~/.ssh/authorized_keys", Destructive, true),
        ("tee ~/.ssh/authorized_keys < /dev/null", Destructive, true),
        ("sudo rm -rf /root/.ssh", Destructive, true),
        ("mv ~/.ssh ~/.ssh.old", Destructive, true),
        ("chmod 777 ~/.ssh", Destructive, true),
        ("sudo truncate -s 0 /etc/ssh/ssh_host_ed25519_key", Destructive, true),
        ("sudo chmod 000 /", Destructive, true),
        ("sudo chmod 777 /etc", Destructive, true),
        ("sudo chown nobody /usr", Destructive, true),
        ("sudo passwd sparkuser", Destructive, true),
        ("sudo usermod -L sparkuser", Destructive, true),
        ("python3 -c \"import subprocess; subprocess.run(['systemctl', 'stop', 'ssh'])\"", Destructive, true),
        ("perl -e 'system(\"rm -rf ~/.ssh\")'", Destructive, true),
        // QA: data loss that was an ordinary change.
        ("sudo rm -rf /var/lib/docker", Destructive, false),
        ("sudo rm -rf /var/lib/containerd/", Destructive, false),
        ("sudo rm -rf /opt/models", Destructive, false),
        ("rm -rf ~/models", Destructive, false),
        ("rm -r /home/sparkuser/models", Destructive, false),
        ("rm -r /home/sparkuser/models/Llama-3.3-70B-Instruct", Change, false),
        ("rm -rf ~/.cache/huggingface/hub", Destructive, false),
        ("docker rm -f $(docker ps -aq)", Destructive, false),
        ("docker ps -aq | xargs docker rm -f", Destructive, false),
        ("docker container prune -f", Destructive, false),
        ("pkill -u sparkuser", Destructive, false),
        ("sudo pkill -KILL -usparkuser", Destructive, false),
        ("sudo crontab -r", Destructive, false),
        ("mkfs.ext4 /tmp/disk.img", Destructive, false),
        ("sudo iptables -P INPUT ACCEPT", Destructive, false),
        ("sudo ufw default allow incoming", Destructive, false),
        ("python3 -c \"import os; os.system('reboot')\"", Destructive, false),
        ("awk 'BEGIN { system(\"reboot\") }'", Destructive, false),
        // ... and their ordinary neighbours stay what they were.
        ("docker rm -f vllm-head", Change, false),
        ("rm -rf ~/models.tmp", Change, false),
        ("rm ~/models/notes.txt", Change, false),
        ("pkill vllm", Change, false),
        ("crontab -l", Read, false),
        ("cat ~/.ssh/authorized_keys", Read, false),
        ("ls -la ~/.ssh", Read, false),
        ("cp ~/.ssh/id_ed25519.pub /tmp/", Change, false),
        ("python3 -c \"print('it is fine')\"", Change, false),
        ("python3 -c \"print(\\\"don't\\\")\"", Change, false),
    ];
    let wrong: Vec<String> = table
        .iter()
        .filter_map(|(c, kind, cuts)| {
            let v = verdict(c);
            (v.kind != *kind || v.cuts_access != *cuts).then(|| format!("{c:?}: expected {kind:?}{}, got {v:?}", if *cuts { " · cuts access" } else { "" }))
        })
        .collect();
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
    // The power verb inside an interpreter string is still a power verb (several nodes cut access).
    assert_eq!(verdict("python3 -c \"import os; os.system('reboot')\"").power, Some(super::classify::Power::Restart));
}

#[test]
fn sudo_doas_and_pkexec_are_found_in_every_part() {
    use super::classify::escalates;
    for c in ["sudo reboot", "uptime && sudo reboot", "uptime | sudo tee /x", "doas id", "pkexec id", "bash -c 'sudo id'", "xargs sudo rm", "su -c id", "env A=1 sudo id", "echo $(sudo cat /etc/shadow)"] {
        assert!(escalates(c), "{c}");
    }
    for c in ["uptime", "echo sudo", "grep sudo /var/log/auth.log", "cat /etc/sudoers.d/x"] {
        assert!(!escalates(c), "{c}");
    }
}

#[test]
fn confirmations_say_what_an_action_breaks() {
    let cell1 = NodeCtx { name: "cell1".into(), ..Default::default() };
    let im = service_impact("ssh.service", "stop", std::slice::from_ref(&cell1), false);
    assert_eq!(im.tier, "high");
    assert_eq!(im.lines, ["SparkBox and new SSH sessions can't reach cell1 until it's started from a console."]);
    assert_eq!(im.require_typing.as_deref(), Some("cell1"));

    let im = service_impact("ssh.service", "disable", std::slice::from_ref(&cell1), true);
    assert_eq!(im.lines, ["After the next reboot nobody can SSH in — you're remote."]);
    assert_eq!(im.require_typing.as_deref(), Some("cell1"));

    let busy = NodeCtx {
        name: "cell1".into(),
        containers: vec!["vllm-head".into(), "open-webui".into()],
        serving: vec!["Llama-3.3-70B-Instruct (llama-70b on cell1, cell2, cell3)".into()],
    };
    let im = service_impact("docker.service", "stop", &[busy], false);
    assert_eq!(im.tier, "high");
    assert_eq!(im.lines[0], "2 running containers on cell1 stop: vllm-head, open-webui.");
    assert_eq!(im.lines[1], "Llama-3.3-70B-Instruct (llama-70b on cell1, cell2, cell3) is unavailable until its containers are back.");

    let im = service_impact("docker.service", "restart", &[NodeCtx { name: "gx10-0000".into(), containers: vec!["ollama".into()], serving: vec![] }], false);
    assert_eq!(im.tier, "medium");

    for unit in ["ssh.socket", "nvidia-persistenced.service", "dgx-dashboard.service"] {
        assert!(services::CRITICAL.contains(&unit));
        assert_ne!(service_impact(unit, "stop", std::slice::from_ref(&cell1), false).tier, "low", "{unit}");
    }
    let im = service_impact("ollama.service", "mask", std::slice::from_ref(&cell1), false);
    assert_eq!((im.tier, im.require_typing.as_deref()), ("high", Some("cell1")));
    assert_eq!(service_impact("ollama.service", "start", std::slice::from_ref(&cell1), false).tier, "low");

    let two = [cell1.clone(), NodeCtx { name: "cell2".into(), ..Default::default() }];
    let im = service_impact("cron.service", "restart", &two, false);
    assert!(im.lines.last().unwrap().contains("one node at a time"));
    assert_eq!(services::list(&["cell1", "cell2", "cell3"]), "cell1, cell2 and cell3");
}

#[test]
fn the_models_a_node_serves_come_from_its_deployment() {
    let mut d = DemoCluster::new(&Inventory::example());
    let snap = d.snapshot();
    let cell2 = snap.nodes.iter().find(|n| n.id == "cell2").unwrap();
    assert_eq!(services::serving(&snap.nodes, cell2), ["Llama-3.3-70B-Instruct (llama-70b on cell1, cell2, cell3)"]);
}

#[test]
fn service_status_and_journal_are_parsed() {
    let text = "× nvidia-dgx-telemetry.service - NVIDIA DGX telemetry
     Loaded: loaded (/usr/lib/systemd/system/nvidia-dgx-telemetry.service; enabled; preset: enabled)
     Active: failed (Result: exit-code) since Tue 2026-10-28 09:14:02 UTC; 1h 3min ago
   Duration: 2.112s
    Process: 2211 ExecStart=/usr/bin/nvidia-dgx-telemetry (code=exited, status=1/FAILURE)
   Main PID: 2211 (code=exited, status=1/FAILURE)
        CPU: 41ms
";
    let d = services::parse_status("nvidia-dgx-telemetry.service", text, vec![]);
    assert_eq!(d.description, "NVIDIA DGX telemetry");
    assert_eq!(d.active.as_deref(), Some("failed (Result: exit-code)"));
    assert_eq!(d.active_since.as_deref(), Some("Tue 2026-10-28 09:14:02 UTC; 1h 3min ago"));
    assert_eq!(d.main_pid, Some(2211));
    assert_eq!(d.result.as_deref(), Some("exit-code · status 1/FAILURE"));
    assert_eq!(d.cpu.as_deref(), Some("41ms"));

    let running = "● docker.service - Docker Application Container Engine
     Active: active (running) since Mon 2026-10-26 09:14:02 UTC; 2 days ago
   Main PID: 1342 (dockerd)
      Tasks: 41 (limit: 146789)
     Memory: 312.4M (peak: 1.1G)
";
    let d = services::parse_status("docker.service", running, vec![]);
    assert_eq!((d.main_pid, d.memory.as_deref(), d.tasks.as_deref(), d.result), (Some(1342), Some("312.4M (peak: 1.1G)"), Some("41"), None));

    let journal = services::parse_journal_text(
        "Oct 28 09:14:02 cell1 nvidia-dgx-telemetry[2211]: telemetry: cannot open /var/lib/nvidia-dgx-telemetry/state.db: Permission denied\n\
         Oct 28 09:14:02 cell1 systemd[1]: nvidia-dgx-telemetry.service: Failed with result 'exit-code'.\n\
         Oct 28 09:14:00 cell1 systemd[1]: Started nvidia-dgx-telemetry.service - NVIDIA DGX telemetry.\n",
    );
    assert_eq!(journal.iter().map(|j| j.priority).collect::<Vec<_>>(), [3, 3, 6]);
    assert_eq!(journal[0].time, "Oct 28 09:14:02");
    assert!(journal[1].text.starts_with("systemd[1]: "));

    let json = r#"{"__REALTIME_TIMESTAMP":"1761642842000000","PRIORITY":"4","SYSLOG_IDENTIFIER":"dockerd","_PID":"1342","MESSAGE":"retrying in 5s"}"#;
    let j = services::parse_journal_json(json);
    assert_eq!((j[0].priority, j[0].ts_ms, j[0].text.as_str()), (4, Some(1_761_642_842_000), "dockerd[1342]: retrying in 5s"));
}

#[test]
fn ss_output_gives_every_port_its_owner() {
    let text = "LISTEN 0 4096 0.0.0.0:8000 0.0.0.0:* users:((\"python3\",pid=2211,fd=7))\n\
                LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1201,fd=3))\n\
                LISTEN 0 128 [::]:22 [::]:* users:((\"sshd\",pid=1201,fd=4))\n\
                LISTEN 0 4096 127.0.0.53%lo:53 0.0.0.0:* users:((\"systemd-resolve\",pid=812,fd=14))\n";
    let l = services::parse_ss(text);
    assert_eq!(l.iter().map(|l| l.port).collect::<Vec<_>>(), [22, 22, 53, 8000]);
    let api = l.iter().find(|l| l.port == 8000).unwrap();
    assert_eq!((api.process.as_deref(), api.pid), (Some("python3"), Some(2211)));
}

#[test]
fn apt_output_is_parsed() {
    let status = "--- upgradable
Listing...
libnvidia-compute-580/noble-updates 580.95.05-0ubuntu1 arm64 [upgradable from: 580.82.07-0ubuntu1]
openssl/noble-updates,noble-security 3.0.13-0ubuntu3.6 arm64 [upgradable from: 3.0.13-0ubuntu3.5]
linux-image-6.11.0-1016-nvidia/noble-updates 6.11.0-1016.16 arm64 [upgradable from: 6.11.0-1015.15]
curl/noble-updates 8.5.0-2ubuntu10.7 arm64 [upgradable from: 8.5.0-2ubuntu10.6]
--- held
docker-ce
--- reboot
*** System restart required ***
linux-image-6.11.0-1015-nvidia
--- unattended
APT::Periodic::Unattended-Upgrade \"1\";
--- checked
1761642842
";
    let u = updates::parse_status("cell1", status);
    assert_eq!(u.pending.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["libnvidia-compute-580", "linux-image-6.11.0-1016-nvidia", "openssl", "curl"], "sensitive first, then security");
    let openssl = u.pending.iter().find(|p| p.name == "openssl").unwrap();
    assert!(openssl.security && !openssl.sensitive);
    assert_eq!((openssl.from.as_str(), openssl.to.as_str(), openssl.origin.as_str()), ("3.0.13-0ubuntu3.5", "3.0.13-0ubuntu3.6", "noble-updates, noble-security"));
    assert!(u.reboot_required);
    assert_eq!(u.reboot_packages, ["linux-image-6.11.0-1015-nvidia"]);
    assert_eq!(u.held, ["docker-ce"]);
    assert_eq!(u.unattended, Some(true));
    assert_eq!(u.driver.as_deref(), Some("580.82.07 → 580.95.05"));
    assert_eq!(u.checked_ms, Some(1_761_642_842_000));
    assert_eq!(super::classify::classify(updates::STATUS_SCRIPT), Kind::Read, "looking at updates changes nothing");

    let sim = updates::parse_simulation(
        "cell1",
        "Inst libnvidia-compute-580 [580.82.07-0ubuntu1] (580.95.05-0ubuntu1 Ubuntu:24.04/noble-updates [arm64])
Inst linux-modules-6.11.0-1016-nvidia (6.11.0-1016.16 Ubuntu:24.04/noble-updates [arm64])
Remv nvidia-dkms-580 [580.82.07-0ubuntu1]
Conf libnvidia-compute-580 (580.95.05-0ubuntu1 Ubuntu:24.04/noble-updates [arm64])
The following packages have been kept back:
  docker-ce
",
    );
    assert_eq!(sim.upgrades, ["libnvidia-compute-580 580.82.07-0ubuntu1 → 580.95.05-0ubuntu1"]);
    assert_eq!(sim.installs, ["linux-modules-6.11.0-1016-nvidia 6.11.0-1016.16"]);
    assert_eq!(sim.removals, ["nvidia-dkms-580"]);
    assert_eq!(sim.held_back, ["docker-ce"]);
}

#[tokio::test]
async fn demo_updates_show_drift_and_hold_and_simulate() {
    let (ops, _) = demo_ops("d6");
    let gx = ops.updates_status(true, "gx10-0000").await.unwrap();
    // The real Sparks all run one driver (the demo world matches them since r4): no drift.
    let cell1 = ops.updates_status(true, "cell1").await.unwrap();
    assert!(cell1.driver.is_none() && gx.driver.is_none(), "{:?} {:?}", cell1.driver, gx.driver);

    let pkg = gx.pending.iter().find(|p| !p.held).unwrap().name.clone();
    ops.updates_hold(true, "gx10-0000", std::slice::from_ref(&pkg), true).await.unwrap();
    let after = ops.updates_status(true, "gx10-0000").await.unwrap();
    assert!(after.pending.iter().find(|p| p.name == pkg).unwrap().held);
    let sim = ops.updates_simulate(true, "gx10-0000", &[]).await.unwrap();
    assert!(sim.held_back.contains(&pkg));
    assert!(!sim.upgrades.iter().any(|u| u.starts_with(&format!("{pkg} "))));
    assert!(sim.text.contains("NOTE: This is only a simulation!"));

    let id = ops.updates_upgrade(true, "gx10-0000".into(), vec![]).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    let left = ops.updates_status(true, "gx10-0000").await.unwrap();
    assert_eq!(left.pending.iter().map(|p| p.name.clone()).collect::<Vec<_>>(), [pkg.clone()], "only the held package is left");
    // A restart is waiting only when a kernel, NVIDIA or other sensitive package went in.
    let sensitive = gx.pending.iter().any(|p| p.sensitive && p.name != pkg);
    assert_eq!(left.reboot_required, sensitive, "{:?}", gx.pending);
    assert!(ops.updates_hold(true, "cell1", &["x; rm -rf /".into()], true).await.is_err());
}

#[tokio::test]
async fn service_rollout_runs_one_node_at_a_time_and_stops_at_a_failure() {
    let (ops, _) = demo_ops("d7");
    let nodes: Vec<String> = ["cell2", "cell1", "cell3"].map(String::from).to_vec();
    let id = ops.services_apply(true, nodes.clone(), "nvidia-dgx-telemetry.service".into(), "restart".into()).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(5)).await;
    // It's broken on cell1 in the default world, so the rollout stops there.
    assert_eq!(t.state, TaskState::Failed, "{:?}", t.steps);
    assert_eq!(t.steps.iter().map(|s| s.state).collect::<Vec<_>>(), [StepState::Ok, StepState::Failed, StepState::Skipped]);
}

// ---- r2: failures in the log, Verify steps, Wake, Fleet's Destructive tier, masked inspect

#[tokio::test]
async fn a_failed_telemetry_restart_is_in_the_log_with_its_stderr_in_demo_and_live() {
    // Demo: in the default world nvidia-dgx-telemetry on cell1 fails every time it starts.
    let (ops, _) = demo_ops("telemetry");
    let e = ops.service_action(true, "cell1", "nvidia-dgx-telemetry.service", "restart").await.unwrap_err();
    assert!(e.message.contains("Job for nvidia-dgx-telemetry.service failed"), "{}", e.message);
    let log = ops.live.audit.entries();
    let cmd = log.iter().find(|e| e.kind == "sudo" && e.command.contains("systemctl restart")).expect("the simulated command is logged");
    assert_eq!((cmd.state.as_str(), cmd.code, cmd.simulated, cmd.class.as_str()), ("failed", Some(1), true, "change"));
    assert!(cmd.stderr_tail.as_deref().unwrap().contains("Job for nvidia-dgx-telemetry.service failed"));
    let head = log.iter().find(|e| e.kind == "op").expect("the operation's own line");
    assert_eq!((head.title.as_str(), head.state.as_str()), ("Restart nvidia-dgx-telemetry.service on cell1", "failed"));

    // Live, through sudo on a fake node: the command's exit code and stderr.
    let (ops, node) = setup("telemetry").await;
    ops.service_action(false, "cell1", "nvidia-dgx-telemetry.service", "restart").await.unwrap_err();
    assert!(node.log.lock().unwrap().iter().any(|c| c.ends_with("systemctl restart -- 'nvidia-dgx-telemetry.service'")));
    let log = ops.live.audit.entries();
    let cmd = log.iter().find(|e| e.kind == "sudo" && e.command.contains("nvidia-dgx-telemetry")).expect("logged");
    assert_eq!((cmd.state.as_str(), cmd.code), ("failed", Some(1)));
    assert!(cmd.stderr_tail.as_deref().unwrap().contains("Job for nvidia-dgx-telemetry.service failed"));

    // Failing before anything runs (the node isn't connected) is logged too.
    ops.service_action(false, "cell9", "docker.service", "restart").await.unwrap_err();
    let head = ops.live.audit.entries().into_iter().find(|e| e.kind == "op" && e.title == "Restart docker.service on cell9").expect("logged");
    assert_eq!(head.state, "failed");
    assert!(head.error.unwrap().contains("isn't connected"));
}

/// A scripted Verify: names one endpoint and says how it came back.
struct FakeVerifier {
    outcome: verify::VerifyOutcome,
    asked: Mutex<Vec<verify::Scope>>,
}

impl Verifier for FakeVerifier {
    fn affected(&self, scope: &verify::Scope) -> Vec<verify::EndpointRef> {
        self.asked.lock().unwrap().push(scope.clone());
        vec![verify::EndpointRef { id: "cell1-vllm-8000".into(), model: "llama-3.3-70b".into(), nodes: vec!["cell1".into(), "cell2".into(), "cell3".into()] }]
    }

    fn wait<'a>(&'a self, refs: &'a [verify::EndpointRef], _timeout: Duration, progress: Box<dyn Fn(&verify::EndpointRef, String) + Send + Sync + 'a>) -> BoxFuture<'a, Vec<(verify::EndpointRef, verify::VerifyOutcome)>> {
        progress(&refs[0], "llama-3.3-70b: loading 42%".into());
        let out = refs.iter().map(|r| (r.clone(), self.outcome.clone())).collect();
        Box::pin(async move { out })
    }
}

fn verifier(outcome: verify::VerifyOutcome) -> Arc<FakeVerifier> {
    Arc::new(FakeVerifier { outcome, asked: Mutex::new(Vec::new()) })
}

fn llama_items(d: &Arc<Mutex<DemoCluster>>) -> Vec<containers::Target> {
    let snap = d.lock().unwrap().snapshot();
    snap.nodes
        .iter()
        .flat_map(|n| n.containers.iter().filter(|c| c.project.as_deref() == Some("llama-70b")).map(move |c| containers::Target { node: n.id.clone(), id: c.id.clone(), name: c.name.clone() }))
        .collect()
}

#[tokio::test]
async fn a_container_restart_ends_by_checking_the_model_answers() {
    let (ops, d) = demo_ops("verify-ok");
    let v = verifier(verify::VerifyOutcome::Ready { after_s: 130, untested: None });
    ops.set_verifier(v.clone());
    let items = llama_items(&d);
    let id = ops.containers_bulk(true, items.clone(), "restart".into(), false, None).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(5)).await;
    assert_eq!((t.state, t.warning.as_deref()), (TaskState::Ok, None));
    assert_eq!(t.summary.as_deref(), Some("3 of 3 restarted. llama-3.3-70b answered after 2m 10s"));
    let last = t.steps.last().unwrap();
    assert_eq!((last.label.as_str(), last.detail.as_deref()), ("Waiting for llama-3.3-70b to answer…", Some("ready after 2m 10s")));
    assert_eq!(*v.asked.lock().unwrap(), [verify::Scope::Containers(items.iter().map(|i| (i.node.clone(), i.id.clone())).collect())]);

    // A stop isn't followed by a check.
    let id = ops.containers_bulk(true, llama_items(&d), "stop".into(), false, None).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(5)).await;
    assert!(!t.steps.iter().any(|s| s.label.starts_with("Waiting for llama")));
}

#[tokio::test]
async fn a_restart_whose_model_doesnt_come_back_ends_with_a_warning() {
    let (ops, d) = demo_ops("verify-down");
    ops.set_verifier(verifier(verify::VerifyOutcome::Down { reason: "CUDA out of memory while loading".into() }));
    let id = ops.containers_bulk(true, llama_items(&d), "restart".into(), false, None).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(5)).await;
    assert_eq!(t.state, TaskState::Ok, "the containers did restart");
    assert_eq!(t.warning.as_deref(), Some("Restarted, but llama-3.3-70b didn't come back: CUDA out of memory while loading"));
    assert_eq!(t.steps.last().unwrap().state, StepState::Failed);
    let head = ops.live.audit.entries().into_iter().find(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind == "op").unwrap();
    // The head line says what Tasks and the toast say: done, with a warning.
    assert_eq!((head.state.as_str(), t.outcome), ("warning", Some(crate::tasks::Outcome::Warning)), "the log and Tasks agree");
    assert_eq!(head.error.as_deref(), t.warning.as_deref(), "and it says why");
}

/// Records what the operations tell the alerting (the app wires health::maintenance and the agents).
#[derive(Default)]
struct PlannedLog(Mutex<Vec<String>>);

impl PlannedLog {
    /// Its lines once there are `n`: a task reads finished a moment before its window ends.
    async fn lines(&self, n: usize) -> Vec<String> {
        let started = Instant::now();
        loop {
            let got = self.0.lock().unwrap().clone();
            if got.len() >= n {
                return got;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "only {got:?}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

impl Planned for PlannedLog {
    fn begin(&self, node: &str, until_ms: Option<u64>, reason: &str) {
        self.0.lock().unwrap().push(format!("begin {node} {reason} {}", if until_ms.is_some() { "until" } else { "open-ended" }));
    }
    fn end(&self, node: &str) {
        self.0.lock().unwrap().push(format!("end {node}"));
    }
    fn resolved(&self, rule: &str, alert: &str, reason: &str) {
        self.0.lock().unwrap().push(format!("resolved {rule} {alert} · {reason}"));
    }
}

#[tokio::test]
async fn restarts_shutdowns_and_wakes_are_planned_work() {
    let (ops, d) = demo_ops("planned");
    ops.set_verifier(verifier(verify::VerifyOutcome::Ready { after_s: 30, untested: None }));
    let log = Arc::new(PlannedLog::default());
    ops.set_planned(log.clone());
    let id = ops.power(true, "cell3", "reboot", 0).unwrap();
    assert_eq!(wait_task(&ops, &id, Duration::from_secs(10)).await.state, TaskState::Ok);
    let id = ops.power(true, "cell3", "poweroff", 0).unwrap();
    assert_eq!(wait_task(&ops, &id, Duration::from_secs(10)).await.state, TaskState::Ok);
    assert_eq!(log.lines(3).await, ["begin cell3 restarting until", "end cell3", "begin cell3 shut down open-ended"], "a node that's off stays planned");
    // Let the simulator finish switching it off.
    for _ in 0..5 {
        d.lock().unwrap().advance();
    }
    let sim = d.clone();
    let id = ops.wake(true, "cell3", Box::pin(async move {
        sim.lock().unwrap().power("cell3", "wake")?;
        Ok("from cell1".to_string())
    })).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    assert_eq!(log.lines(5).await[3..], ["begin cell3 starting up until", "end cell3"]);
    // A scheduled restart called off ends its window too.
    log.0.lock().unwrap().clear();
    let id = ops.power(true, "cell2", "reboot", 1).unwrap();
    // Once its window is open: Cancel before that calls it off with no window at all.
    let opened = std::time::Instant::now();
    while !log.0.lock().unwrap().iter().any(|l| l.starts_with("begin cell2")) {
        assert!(opened.elapsed() < Duration::from_secs(10), "the window never opened: {:?}", log.0.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    ops.tasks().cancel(&id).unwrap();
    assert_eq!(wait_task(&ops, &id, Duration::from_secs(10)).await.state, TaskState::Cancelled);
    assert_eq!(log.lines(2).await, ["begin cell2 restarting until", "end cell2"]);
    // A container restart that verifies resolves its crash-loop alerts at once.
    log.0.lock().unwrap().clear();
    let items = llama_items(&d);
    let id = ops.containers_bulk(true, items.clone(), "restart".into(), false, None).unwrap();
    assert_eq!(wait_task(&ops, &id, Duration::from_secs(10)).await.state, TaskState::Ok);
    let got = log.0.lock().unwrap().clone();
    assert_eq!(got.len(), items.len(), "{got:?}");
    assert!(got[0].starts_with(&format!("resolved crash-loop crash-loop:container:{}:{} · Verified: llama-3.3-70b answered after", items[0].node, items[0].name)), "{got:?}");
}

/// Agents that don't take the maintenance note: cell2's is unreachable. The power task waits
/// for them before the command, then says which one may page.
struct DeafAgents {
    order: Mutex<Vec<String>>,
}

impl Planned for DeafAgents {
    fn begin(&self, node: &str, _: Option<u64>, _: &str) {
        self.order.lock().unwrap().push(format!("begin {node}"));
    }
    fn notify_agents<'a>(&'a self, op_id: &'a str, nodes: &'a [String], _: Option<u64>, _: &'a str) -> BoxFuture<'a, Vec<(String, String)>> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            self.order.lock().unwrap().push(format!("agents told about {} under {op_id}", nodes.join(",")));
            vec![("cell2".to_string(), "connection refused".to_string())]
        })
    }
    fn end(&self, node: &str) {
        self.order.lock().unwrap().push(format!("end {node}"));
    }
    fn resolved(&self, _: &str, _: &str, _: &str) {}
}

#[tokio::test]
async fn the_agents_hear_before_the_power_command_and_a_missed_one_is_named() {
    let (ops, _d) = demo_ops("maint-first");
    ops.set_verifier(verifier(verify::VerifyOutcome::Ready { after_s: 30, untested: None }));
    let planned = Arc::new(DeafAgents { order: Mutex::new(Vec::new()) });
    ops.set_planned(planned.clone());
    let id = ops.power(true, "cell3", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    let order = planned.order.lock().unwrap().clone();
    assert_eq!(order[..2], ["begin cell3".to_string(), format!("agents told about cell3 under {id}")], "awaited, under the task's operation");
    let told = t.steps.iter().find(|s| s.label == "Telling the nodes' agents about the planned work").expect("a step of its own");
    assert_eq!(told.detail.as_deref(), Some("cell2's agent didn't get the maintenance note: it may page (connection refused)."));
    // The power command was sent only after the agents step finished.
    let sent = t.steps.iter().position(|s| s.label.starts_with("Sending the restart")).unwrap();
    let agents = t.steps.iter().position(|s| s.label.starts_with("Telling the nodes' agents")).unwrap();
    assert!(agents < sent);
    let restart = ops.live.audit.entries().into_iter().find(|e| e.op_id.as_deref() == Some(id.as_str()) && e.command.contains("systemctl reboot")).expect("the simulated restart");
    assert!(restart.ts_ms >= t.steps[agents].finished_ms.unwrap());
}

#[tokio::test]
async fn a_demo_reboot_and_a_wake_are_verified() {
    let (ops, d) = demo_ops("verify-power");
    let v = verifier(verify::VerifyOutcome::Ready { after_s: 45, untested: None });
    ops.set_verifier(v.clone());
    let id = ops.power(true, "cell3", "reboot", 0).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    assert!(t.summary.as_deref().unwrap().ends_with("llama-3.3-70b answered after 45s"), "{:?}", t.summary);
    assert_eq!(v.asked.lock().unwrap()[0], verify::Scope::Nodes(vec!["cell3".into()]));

    // Shut it down, then wake it: no typed name, a packet, back online, then the check.
    let id = ops.power(true, "cell3", "poweroff", 0).unwrap();
    assert_eq!(wait_task(&ops, &id, Duration::from_secs(10)).await.state, TaskState::Ok);
    assert!(ops.wake(true, "cell1", Box::pin(async { Ok("from cell2".to_string()) })).unwrap_err().message.contains("already on"));
    let sim = d.clone();
    let send = Box::pin(async move {
        sim.lock().unwrap().power("cell3", "wake")?;
        Ok("from cell1".to_string())
    });
    let id = ops.wake(true, "cell3", send).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Sending the wake packet to cell3", "Waiting for cell3 to come online (up to 5 min)", "Waiting for llama-3.3-70b to answer…"]);
    assert_eq!(t.steps[0].detail.as_deref(), Some("Sent from cell1"));
    assert!(t.summary.unwrap().starts_with("cell3 is on after "));
    assert_eq!(t.retry.unwrap().args["action"], "wake");

    // A packet that can't be sent fails the task, and the log says so.
    let id = ops.wake(true, "cell3", Box::pin(async { Err("No other node is on to send the wake packet.".to_string()) })).err();
    assert!(id.is_some(), "cell3 is on again, so there's nothing to wake");
}

#[tokio::test]
async fn a_destructive_fleet_command_never_runs_on_several_nodes_at_once() {
    let (ops, _) = demo_ops("fleet-destructive");
    let nodes: Vec<String> = ["cell1", "cell2", "cell3", "gx10-0000"].map(String::from).to_vec();
    let spec = FleetSpec { nodes: nodes.clone(), command: "dd if=/dev/zero of=/dev/nvme0n1 bs=1M count=1".into(), sudo: true, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(true, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    let retry = t.retry.unwrap();
    assert_eq!((retry.args["strategy"].as_str(), retry.args["stopOnFailure"].as_bool()), (Some("oneByOne"), Some(true)), "forced by the backend, whatever the window asked");
    // The simulator has no reply for dd: the first node fails (exit 127), the log says so with
    // what the node printed, and the other nodes are never tried.
    let log: Vec<_> = ops.live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind != "op").collect();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!((log[0].state.as_str(), log[0].code, log[0].class.as_str(), log[0].simulated), ("failed", Some(127), "destructive", true));
    assert!(log[0].stderr_tail.as_deref().unwrap().contains("no simulated output"));
    assert_eq!(t.steps.iter().filter(|s| s.state == StepState::Skipped).count(), 3);

    // A snippet's run is titled with its name, the command underneath.
    let spec = FleetSpec { nodes, command: "uptime".into(), sudo: false, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: Some("Uptime".into()), cuts_access: false };
    let id = ops.fleet_start(true, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!((t.title.as_str(), t.subtitle.as_deref()), ("Fleet: Uptime", Some("uptime")));
}

#[tokio::test]
async fn stopping_ssh_runs_on_one_node_and_stops_when_sparkbox_cant_log_in_again() {
    // Two fake nodes whose `systemctl stop ssh` really stops new logins (open sessions go on).
    let (a, pa) = fake_node(&temp("ops-cut-a"), script()).await;
    let (_b, pb) = fake_node(&temp("ops-cut-b"), script()).await;
    let (live, inventory) = connect(&temp("ops-cut-app"), &[("cell1", pa), ("cell2", pb)]).await;
    let ops = Ops::with_poll(live.clone(), demo(), Tasks::new(None), Duration::from_millis(5));
    ops.set_snapshot_source(move || Some(live.snapshot(&inventory)));
    let nodes: Vec<String> = ["cell1", "cell2"].map(String::from).to_vec();
    let spec = FleetSpec { nodes, command: "systemctl stop ssh".into(), sudo: true, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(false, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(20)).await;
    assert_eq!(t.state, TaskState::Failed, "{t:?}");
    let results = ops.fleet_results(&id).unwrap();
    assert_eq!(results[0].state, "failed", "the command ran, but SparkBox can't log in again");
    assert!(results[0].error.as_deref().unwrap().starts_with("SparkBox can't log in to cell1 over SSH any more"), "{:?}", results[0].error);
    assert_eq!(results[1].state, "skipped", "cell2 was never touched");
    assert!(a.log.lock().unwrap().iter().any(|c| c.contains("systemctl stop ssh")));
    let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["cell1 (1 of 2)", "Logging in to cell1 again over SSH", "cell2"], "{labels:?}");
    assert_eq!(t.retry.unwrap().args["cutsAccess"], true);
    // In the activity log the run is a change (it's in the Changes view).
    let log: Vec<_> = ops.live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(id.as_str())).collect();
    assert!(log.iter().all(|e| e.changes && e.class != "read"), "{log:?}");
}

#[tokio::test]
async fn cutting_access_waits_for_each_node_before_the_next_in_demo() {
    let (ops, d) = demo_ops("fleet-cut");
    let nodes: Vec<String> = ["cell1", "cell2", "cell3"].map(String::from).to_vec();
    // stop ssh: the simulated nodes stay reachable, and each is checked before the next.
    let spec = FleetSpec { nodes: nodes.clone(), command: "systemctl stop ssh".into(), sudo: true, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let t = wait_task(&ops, &ops.fleet_start(true, spec).unwrap(), Duration::from_secs(20)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["cell1 (1 of 3)", "Logging in to cell1 again over SSH", "cell2 (2 of 3)", "Logging in to cell2 again over SSH", "cell3 (3 of 3)", "Logging in to cell3 again over SSH"]);
    // A restart on several nodes is a rolling restart: each goes away and comes back first.
    let spec = FleetSpec { nodes: nodes[..2].to_vec(), command: "reboot".into(), sudo: true, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let t = wait_task(&ops, &ops.fleet_start(true, spec).unwrap(), Duration::from_secs(30)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).filter(|l| !l.starts_with("Checks before the restart") && !l.starts_with("Checks after the restart")).collect();
    assert_eq!(labels[..4], ["cell1 (1 of 2)", "Waiting for cell1 to go away", "Waiting for cell1 to come back (up to 6 min)", "Logging in to cell1 again over SSH"]);
    // Shutting down every node goes one at a time too: each is through once it's off (there's
    // nothing to log in to), and only then is the next one sent it.
    let spec = FleetSpec { nodes: nodes.clone(), command: "shutdown -h now".into(), sudo: true, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(true, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
    let results = ops.fleet_results(&id).unwrap();
    assert_eq!(results.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(), ["ok", "ok", "ok"]);
    let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["cell1 (1 of 3)", "Waiting for cell1 to go away", "cell2 (2 of 3)", "Waiting for cell2 to go away", "cell3 (3 of 3)", "Waiting for cell3 to go away"]);
    assert_eq!(t.steps[1].detail.as_deref(), Some("cell1 is off (planned until it's woken)"));
    let off: Vec<String> = d.lock().unwrap().snapshot().nodes.iter().filter(|n| n.status != crate::model::LinkStatus::Online).map(|n| n.id.clone()).collect();
    assert_eq!(off, ["cell1", "cell2", "cell3"], "the three it was sent to are off");
    // On one node a power verb is only destructive (a canary run says it cuts access).
    let spec = FleetSpec { nodes: vec!["gx10-0000".into()], command: "uptime".into(), sudo: false, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let t = wait_task(&ops, &ops.fleet_start(true, spec).unwrap(), Duration::from_secs(10)).await;
    assert!(t.read_only, "a read-only run is a read-only task: anyone may stop it");
}

#[test]
fn inspect_masks_secret_environment_values_and_reveal_reads_one() {
    let mut v = serde_json::json!([{
        "Name": "/vllm-head",
        "Config": {
            "Env": ["HF_TOKEN=hf_AbCdEf0123456789AbCdEf0123456789ab", "VLLM_API_KEY=abc", "DB_PASS=x", "AWS_CREDENTIAL_FILE=/x", "NCCL_SOCKET_IFNAME=enp1s0f1np1", "PATH=/usr/bin"],
            "Cmd": ["vllm", "serve", "--api-key", "sk-live-abcdefgh12345"]
        }
    }]);
    let raw = v.clone();
    containers::mask_env(&mut v);
    let env: Vec<&str> = v[0]["Config"]["Env"].as_array().unwrap().iter().map(|e| e.as_str().unwrap()).collect();
    assert_eq!(env, ["HF_TOKEN=‹redacted›", "VLLM_API_KEY=‹redacted›", "DB_PASS=‹redacted›", "AWS_CREDENTIAL_FILE=‹redacted›", "NCCL_SOCKET_IFNAME=enp1s0f1np1", "PATH=/usr/bin"]);
    assert_eq!(v[0]["Config"]["Cmd"][3], "‹redacted›");
    assert_eq!(containers::env_value(&raw, "HF_TOKEN").as_deref(), Some("hf_AbCdEf0123456789AbCdEf0123456789ab"));
    assert_eq!(containers::inspect_name(&raw).as_deref(), Some("vllm-head"));
}


// ---- R1-K P1.42: Docker is root, wrappers, logind power, the Read tier and the promotions.

/// Docker is root-equivalent (P1.42): `docker run`/`create`/`exec` with host access is tagged
/// root, its own command (host bind paths mapped back) is classified, and the wrappers,
/// logind power over the bus and the Read-tier and access promotions are covered.
#[test]
fn docker_is_root_and_the_round1_classifier_cases() {
    use super::classify::{verdict, Power};
    use Kind::*;
    // (command, kind, cuts access, powers (None unless a power verb), root)
    let table: &[(&str, Kind, bool, Option<Power>, bool)] = &[
        // Docker as root: the flags that give the container the node's root.
        ("docker run --rm -v /:/host alpine true", Change, false, None, true),
        ("docker run --privileged alpine true", Change, false, None, true),
        ("docker run --pid=host alpine true", Change, false, None, true),
        ("docker run --pid host alpine true", Change, false, None, true),
        ("docker run --net=host alpine true", Change, false, None, true),
        ("docker run --network host alpine true", Change, false, None, true),
        ("docker run --cap-add SYS_ADMIN alpine true", Change, false, None, true),
        ("docker run --device /dev/nvme0n1 alpine true", Change, false, None, true),
        ("docker run -u 0 alpine true", Change, false, None, true),
        ("docker run --user root alpine true", Change, false, None, true),
        ("docker run --mount type=bind,source=/,destination=/host alpine true", Change, false, None, true),
        ("docker exec -u 0 vllm-head id", Change, false, None, true),
        ("docker exec --user root vllm-head id", Change, false, None, true),
        ("docker exec --privileged vllm-head id", Change, false, None, true),
        ("sudo docker run -v /:/host alpine true", Change, false, None, true),
        // An ordinary container, isolated from the node, isn't root and its command stays inside it.
        ("docker run --rm alpine true", Change, false, None, false),
        ("docker run -d --name x -p 8000:8000 vllm serve /models/m", Change, false, None, false),
        ("docker run alpine reboot", Change, false, None, false),
        ("docker ps -a --format '{{json .}}'", Read, false, None, false),
        // The container's own command is classified, with the host bind mapped back.
        ("docker run --rm -v /:/host alpine chroot /host shutdown -h now", Destructive, false, Some(Power::Off), true),
        ("docker run --rm -v /:/host alpine rm -rf /host/var/lib/docker", Destructive, false, None, true),
        ("docker exec -u 0 vllm-head reboot", Destructive, false, Some(Power::Restart), true),
        // Wrappers unwrap to the real verb.
        ("sudo chroot / reboot", Destructive, false, Some(Power::Restart), false),
        ("nsenter -t 1 -m -p -- reboot", Destructive, false, Some(Power::Restart), false),
        ("unshare -m reboot", Destructive, false, Some(Power::Restart), false),
        ("systemd-run --unit sb-x reboot", Destructive, false, Some(Power::Restart), false),
        ("setsid reboot", Destructive, false, Some(Power::Restart), false),
        ("flock -n /tmp/l shutdown -h now", Destructive, false, Some(Power::Off), false),
        // logind power over the bus is a power verb.
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager PowerOff b true", Destructive, false, Some(Power::Off), false),
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager Reboot b true", Destructive, false, Some(Power::Restart), false),
        ("gdbus call --system --dest org.freedesktop.login1 --object-path /org/freedesktop/login1 --method org.freedesktop.login1.Manager.PowerOff true", Destructive, false, Some(Power::Off), false),
        ("dbus-send --system --dest=org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager.Halt boolean:true", Destructive, false, Some(Power::Off), false),
        // Promotions that cut access.
        ("sudo netplan apply", Destructive, true, None, false),
        ("sudo nmcli con up 'Wired connection 1'", Destructive, true, None, false),
        ("sudo networkctl down enP7s7", Destructive, true, None, false),
        ("sudo tc qdisc add dev enP7s7 root netem loss 100%", Destructive, true, None, false),
        ("sudo mlxlink -d /dev/mst/x -a DN", Destructive, true, None, false),
        ("sudo rmmod mlx5_core", Destructive, true, None, false),
        ("sudo modprobe -r mlx5_core", Destructive, true, None, false),
        // Promotions to Destructive (not a lockout of their own).
        ("sudo nvidia-smi -r", Destructive, false, None, false),
        ("nvidia-smi --gpu-reset -i 0", Destructive, false, None, false),
        ("sudo loginctl terminate-user sparkuser", Destructive, false, None, false),
        ("sudo loginctl kill-user sparkuser", Destructive, false, None, false),
        // curl with an attached value, git grep -O, the helper path, a remote tar: not Read.
        ("curl -o/tmp/x http://127.0.0.1", Change, false, None, false),
        ("sudo curl -o/etc/sudoers.d/x http://127.0.0.1", Change, false, None, false),
        ("git grep -O vllm", Change, false, None, false),
        ("git grep --open-files-in-pager x", Change, false, None, false),
        ("python3 /tmp/.sparkbox/sparkjob.py list", Change, false, None, false),
        ("tar -tf host:/backup.tar", Change, false, None, false),
        ("tar --file=user@host:x.tar -t", Change, false, None, false),
        // P1 r1 fixes. The `container` management verb, and docker's global options with values.
        ("docker container run --rm -v /:/host alpine chroot /host shutdown -h now", Destructive, false, Some(Power::Off), true),
        ("docker container run --privileged alpine true", Change, false, None, true),
        ("docker container create --privileged alpine", Change, false, None, true),
        ("docker container create -v /:/h alpine", Change, false, None, true),
        ("docker container exec -u 0 c id", Change, false, None, true),
        ("docker container exec -u 0 vllm-head reboot", Destructive, false, Some(Power::Restart), true),
        ("docker --tlsverify --tlscacert /x run --privileged alpine true", Change, false, None, true),
        ("docker --tlscert /c --tlskey /k container run -v /:/host alpine true", Change, false, None, true),
        ("docker -H unix:///var/run/docker.sock --config /tmp/c -l debug run --privileged alpine true", Change, false, None, true),
        ("docker container ls -a", Read, false, None, false),
        // Short options as docker's CLI (pflag) reads them: attached values and clusters.
        ("docker run -v/:/host alpine true", Change, false, None, true),
        ("docker run --rm -v/:/host alpine chroot /host shutdown -h now", Destructive, false, Some(Power::Off), true),
        ("docker run -itv /:/host alpine sh", Change, false, None, true),
        ("docker run -v=/:/host alpine true", Change, false, None, true),
        ("docker run -u0 alpine true", Change, false, None, true),
        ("docker run -uroot alpine true", Change, false, None, true),
        ("docker run -u=0 alpine true", Change, false, None, true),
        ("docker run -itu 0 alpine sh", Change, false, None, true),
        ("docker run --user=root:root alpine true", Change, false, None, true),
        ("docker run -u root:root alpine true", Change, false, None, true),
        ("docker run -u 0:100 alpine true", Change, false, None, true),
        ("docker exec -u0 c id", Change, false, None, true),
        ("docker exec -itu 0 c sh", Change, false, None, true),
        ("docker exec -u root:root c id", Change, false, None, true),
        ("docker exec -u0 vllm-head reboot", Destructive, false, Some(Power::Restart), true),
        ("docker run --entrypoint chroot -v /:/host alpine /host reboot", Destructive, false, Some(Power::Restart), true),
        // Not root: an ordinary user, a group of 0 alone, and clusters of flags without host access.
        ("docker run -u 1000:0 alpine true", Change, false, None, false),
        ("docker run -itd --name x alpine sh", Change, false, None, false),
        ("docker run -dp 8000:8000 vllm serve /models/m", Change, false, None, false),
        ("docker run -e A=-v/:/host alpine true", Change, false, None, false),
        ("docker exec -it vllm-head sh", Change, false, None, false),
        ("docker run --privileged=false alpine true", Change, false, None, false),
        // at and batch: the job is classified; one SparkBox can't read fails closed.
        ("echo reboot | at now", Destructive, false, Some(Power::Restart), false),
        ("echo 'shutdown -h now' | at now", Destructive, false, Some(Power::Off), false),
        ("at now <<< reboot", Destructive, false, Some(Power::Restart), false),
        ("echo 'shutdown -h now' | batch", Destructive, false, Some(Power::Off), false),
        ("printf 'uptime\\nreboot\\n' | sudo at now + 1 minute", Destructive, false, Some(Power::Restart), false),
        ("echo 'docker run -v /:/host alpine true' | at now", Change, false, None, true),
        ("echo uptime | at now", Change, false, None, false),
        ("at -f /tmp/x now", Destructive, false, None, false),
        ("at now", Destructive, false, None, false),
        ("cat /tmp/job | at now", Destructive, false, None, false),
        ("echo reboot; at now", Destructive, false, None, false),
        ("at -l", Change, false, None, false),
        ("at -c 3", Change, false, None, false),
        // Wrappers: flock -c, and the value options of unshare and systemd-run.
        ("flock /tmp/l -c reboot", Destructive, false, Some(Power::Restart), false),
        ("flock -n /tmp/l --command 'shutdown -h now'", Destructive, false, Some(Power::Off), false),
        ("unshare --propagation private -m reboot", Destructive, false, Some(Power::Restart), false),
        ("unshare -S 0 reboot", Destructive, false, Some(Power::Restart), false),
        ("unshare -G 0 --setuid 0 reboot", Destructive, false, Some(Power::Restart), false),
        ("unshare -w /tmp reboot", Destructive, false, Some(Power::Restart), false),
        ("systemd-run --service-type oneshot reboot", Destructive, false, Some(Power::Restart), false),
        ("systemd-run -C x reboot", Destructive, false, Some(Power::Restart), false),
        ("systemd-run --capsule x reboot", Destructive, false, Some(Power::Restart), false),
        ("runuser -u root -- reboot", Destructive, false, Some(Power::Restart), false),
        ("runuser -l root -c 'shutdown -h now'", Destructive, false, Some(Power::Off), false),
        ("setpriv --reuid=0 reboot", Destructive, false, Some(Power::Restart), false),
        ("setpriv --reuid 0 --regid 0 --init-groups reboot", Destructive, false, Some(Power::Restart), false),
        ("busybox reboot", Destructive, false, Some(Power::Restart), false),
        ("busybox poweroff", Destructive, false, Some(Power::Off), false),
        // A script inside a root container sees the host through the bind too.
        ("docker run -v /:/host alpine sh -c 'rm -rf /host/var/lib/docker'", Destructive, false, None, true),
        ("docker run -v /:/host alpine sh -c 'chroot /host reboot'", Destructive, false, Some(Power::Restart), true),
        // systemd-run -H runs on another machine: SparkBox can't follow it there.
        ("systemd-run -H cell2 reboot", Destructive, false, None, false),
        ("systemd-run --host=cell2 uptime", Destructive, false, None, false),
        // Power over the bus: the …WithFlags forms, ScheduleShutdown, power targets started.
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager RebootWithFlags t 0", Destructive, false, Some(Power::Restart), false),
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager PowerOffWithFlags t 0", Destructive, false, Some(Power::Off), false),
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager HaltWithFlags t 0", Destructive, false, Some(Power::Off), false),
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager ScheduleShutdown st poweroff 0", Destructive, false, Some(Power::Off), false),
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager ScheduleShutdown st reboot 0", Destructive, false, Some(Power::Restart), false),
        ("busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager StartUnit ss reboot.target replace-irreversibly", Destructive, false, Some(Power::Restart), false),
        ("busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager KExec", Destructive, false, Some(Power::Restart), false),
        ("busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager SoftReboot s ''", Destructive, false, Some(Power::Restart), false),
        ("busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1/unit/reboot_2etarget org.freedesktop.systemd1.Unit Start s replace", Destructive, false, Some(Power::Restart), false),
        ("gdbus call --system --dest org.freedesktop.systemd1 --object-path /org/freedesktop/systemd1 --method org.freedesktop.systemd1.Manager.StartUnit \"'poweroff.target'\" \"'replace'\"", Destructive, false, Some(Power::Off), false),
        ("dbus-send --system --print-reply --dest=org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager.StartUnit string:reboot.target string:replace", Destructive, false, Some(Power::Restart), false),
        // …but not the dry runs, the questions, or a property that only shares the prefix.
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager ScheduleShutdown st dry-poweroff 0", Change, false, None, false),
        ("busctl call org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager CanReboot", Change, false, None, false),
        ("busctl get-property org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager RebootToFirmwareSetup", Change, false, None, false),
        ("busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager StartUnit ss nginx.service replace", Change, false, None, false),
        ("systemctl start reboot.target", Destructive, false, Some(Power::Restart), false),
        ("sudo systemctl start poweroff.target", Destructive, false, Some(Power::Off), false),
        ("systemctl isolate poweroff.target", Destructive, false, Some(Power::Off), false),
        ("systemctl restart halt.target", Destructive, false, Some(Power::Off), false),
        ("systemctl start systemd-reboot", Destructive, false, Some(Power::Restart), false),
        ("systemctl enable --now kexec.target", Destructive, false, Some(Power::Restart), false),
        ("systemctl exit", Destructive, false, Some(Power::Off), false),
        ("systemctl --user exit", Change, false, None, false),
        ("systemctl isolate multi-user.target", Destructive, true, None, false),
        ("systemctl enable reboot.target", Change, false, None, false),
        ("systemctl start nginx", Change, false, None, false),
        // git grep's pager option however git accepts it; its other options stay Read.
        ("git grep --open-files-in-pager=vim x", Change, false, None, false),
        ("git grep --open-files-in-pager='rm -rf ~/models' x", Change, false, None, false),
        ("git grep --open-files x", Change, false, None, false),
        ("git grep --open x", Change, false, None, false),
        ("git grep --op=vim x", Change, false, None, false),
        ("git grep -nOvim x", Change, false, None, false),
        ("git grep -n x", Read, false, None, false),
        ("git grep --only-matching x", Read, false, None, false),
        ("git grep -e a --or -e b", Read, false, None, false),
        // The fabric port by its long name, and modprobe -r in a cluster.
        ("sudo mlxlink -d /dev/mst/x --port_state DN", Destructive, true, None, false),
        ("sudo mlxlink -d /dev/mst/x --port_state=dn", Destructive, true, None, false),
        ("sudo mlxlink -d /dev/mst/x --port_state UP", Change, false, None, false),
        ("sudo modprobe -rv mlx5_core", Destructive, true, None, false),
        ("sudo modprobe -v mlx5_core", Change, false, None, false),
        // sparkjob.py check runs a container to read vLLM's version; only the quick one only looks.
        ("python3 ~/.sparkbox/sparkjob.py check ~/models vllm:img vllm:img-ray", Change, false, None, false),
        ("python3 ~/.sparkbox/sparkjob.py check ~/models vllm:img vllm:img-ray quick", Read, false, None, false),
    ];
    let wrong: Vec<String> = table
        .iter()
        .filter_map(|(c, kind, cuts, power, root)| {
            let v = verdict(c);
            (v.kind != *kind || v.cuts_access != *cuts || v.power != *power || v.root != *root)
                .then(|| format!("{c:?}: wanted {kind:?} cuts={cuts} power={power:?} root={root}, got {v:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));

    // Every read this round's packages rely on must classify Read.
    let reads = [
        "docker ps -a --no-trunc --format '{{json .}}'",
        "docker images --digests --no-trunc --format '{{json .}}'",
        "systemctl --failed --no-legend --plain",
        "df -B1 /",
        "ls -la ~/models ~/.cache/huggingface/hub",
        "apt list --upgradable",
        "apt-mark showhold",
        "dpkg-query -W",
        "nvidia-smi --query-gpu=driver_version,pstate,power.draw --format=csv",
        "ip -br addr",
        "ip -br link",
        "ss -Hltn",
        "grep MemAvailable /proc/meminfo",
        "cat /etc/machine-id",
        "hostname",
        "cat /sys/class/net/enP7s7/address",
        "journalctl --list-boots -o json --no-pager",
        "journalctl -b bb600f35-a1aa-4286-a65e-6b2f30f33b9c -n 200 -p warning -o json --no-pager --output-fields=MESSAGE _PID=1",
        "journalctl -b -1 -n 50 -o json --no-pager -u systemd-journald",
        "cat /proc/1234/cmdline",
        "cat /proc/1234/cmdline | tr '\\0' ' '",
        "curl -s http://127.0.0.1:8000/health",
    ];
    let not_read: Vec<&str> = reads.iter().copied().filter(|c| classify(c) != Read).collect();
    assert!(not_read.is_empty(), "these must be Read: {not_read:?}");
    // The helper only at its own path.
    assert_eq!(classify("python3 ~/.sparkbox/sparkjob.py list"), Read);
    assert_eq!(classify("python3 ~/.sparkbox/sparkjob.py read /etc/netplan/x"), Read);

    // escalates() is true for docker-root (so Fleet's as-root path applies), false otherwise.
    use super::classify::escalates;
    for c in [
        "docker run -v /:/host alpine true", "docker exec -u 0 c sh", "docker run --privileged a b",
        "docker container run --privileged alpine true", "docker run -v/:/host alpine true", "docker exec -itu 0 c sh",
        "docker --tlscacert /x run --privileged alpine true", "echo 'sudo reboot' | at now",
    ] {
        assert!(escalates(c), "{c}");
    }
    for c in ["docker run --rm alpine true", "docker ps", "uptime"] {
        assert!(!escalates(c), "{c}");
    }
}

/// Fleet treats a root docker command as root (one node at a time, stopping at the first) and
/// tags its activity rows root; `sudo chroot / reboot` reads "takes nodes down".
#[tokio::test]
async fn fleet_runs_a_root_docker_command_one_at_a_time_and_tags_it_root() {
    let (ops, _) = demo_ops("fleet-docker-root");
    let nodes: Vec<String> = ["cell1", "cell2", "cell3", "gx10-0000"].map(String::from).to_vec();
    let spec = FleetSpec { nodes: nodes.clone(), command: "docker run --rm -v /:/host alpine true".into(), sudo: false, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(true, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    let retry = t.retry.unwrap();
    assert_eq!((retry.args["strategy"].as_str(), retry.args["stopOnFailure"].as_bool()), (Some("oneByOne"), Some(true)), "a root docker command runs one node at a time");
    let log: Vec<_> = ops.live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind != "op").collect();
    assert!(!log.is_empty() && log.iter().all(|e| e.root), "the rows are tagged root: {log:?}");

    // `sudo chroot / reboot` reads "takes nodes down", and runs one at a time behind the canary.
    let v = super::classify::verdict("sudo chroot / reboot");
    assert_eq!(v.power, Some(super::classify::Power::Restart));
    assert!(v.reason.as_deref().is_some_and(|r| r.starts_with("Takes the node down")), "{:?}", v.reason);
    let spec = FleetSpec { nodes: nodes.clone(), command: "sudo chroot / reboot".into(), sudo: false, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
    let id = ops.fleet_start(true, spec).unwrap();
    let t = wait_task(&ops, &id, Duration::from_secs(10)).await;
    assert_eq!(t.retry.unwrap().args["strategy"].as_str(), Some("oneByOne"), "a power verb runs one node at a time");
}

/// The four P1.42 Fleet examples the acceptance names classify as the plan says.
#[test]
fn the_fleet_acceptance_commands_classify_as_stated() {
    use super::classify::{verdict, Power};
    let v = verdict("docker run --rm -v /:/host alpine true");
    assert!(v.root && v.kind == Kind::Change, "root, as-root tier: {v:?}");
    let v = verdict("sudo chroot / reboot");
    assert_eq!(v.power, Some(Power::Restart));
    assert!(v.reason.as_deref().unwrap().starts_with("Takes the node down"));
    assert_eq!(verdict("curl -o/tmp/x http://127.0.0.1").kind, Kind::Change);
    assert_eq!(verdict("uptime").kind, Kind::Read);
    // The 4-node chroot /host shutdown example: root + a power Off (one by one with the canary).
    let v = verdict("docker run --rm -v /:/host alpine chroot /host shutdown -h now");
    assert!(v.root && v.kind == Kind::Destructive && v.power == Some(Power::Off), "{v:?}");
}


/// P1 r1 fix: the spellings round 1 missed run on four nodes one at a time behind the canary,
/// not in parallel: `docker container run … chroot /host shutdown -h now` and `-v/:/host`
/// (Docker as root, its rows tagged root), and a power verb `at` schedules.
#[tokio::test]
async fn fleet_holds_docker_container_run_attached_volumes_and_at_jobs_to_one_node_at_a_time() {
    let nodes: Vec<String> = ["cell1", "cell2", "cell3", "gx10-0000"].map(String::from).to_vec();
    for (name, command, root) in [
        ("fleet-container-run", "docker container run --rm -v /:/host alpine chroot /host shutdown -h now", true),
        ("fleet-attached-v", "docker run --rm -v/:/host alpine chroot /host shutdown -h now", true),
        ("fleet-at-job", "echo 'shutdown -h now' | at now", false),
    ] {
        let (ops, d) = demo_ops(name);
        let spec = FleetSpec { nodes: nodes.clone(), command: command.into(), sudo: false, timeout_s: 10, strategy: Strategy::Parallel, stop_on_failure: false, title: None, cuts_access: false };
        let id = ops.fleet_start(true, spec).unwrap();
        let t = wait_task(&ops, &id, Duration::from_secs(30)).await;
        let retry = t.retry.unwrap();
        assert_eq!((retry.args["strategy"].as_str(), retry.args["stopOnFailure"].as_bool(), retry.args["cutsAccess"].as_bool()), (Some("oneByOne"), Some(true), Some(true)), "{command}");
        // Each node goes off in turn, and the next is sent it only once the one before is seen gone.
        let results = ops.fleet_results(&id).unwrap();
        assert_eq!(results.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(), ["ok", "ok", "ok", "ok"], "{command}");
        let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
        let order: Vec<String> = nodes.iter().enumerate().flat_map(|(i, n)| [format!("{n} ({} of 4)", i + 1), format!("Waiting for {n} to go away")]).collect();
        assert_eq!(labels, order, "{command}");
        assert_eq!(d.lock().unwrap().snapshot().nodes.iter().filter(|n| n.status == crate::model::LinkStatus::Online).count(), 0, "{command}: all four went off");
        let log: Vec<_> = ops.live.audit.entries().into_iter().filter(|e| e.op_id.as_deref() == Some(id.as_str()) && e.kind != "op").collect();
        assert_eq!(log.len(), 4, "{command}: {log:?}");
        assert!(log.iter().all(|e| (e.root, e.class.as_str()) == (root, "destructive")), "{command}: {log:?}");
    }
}

/// P1 r2 (round 1's spill 1 and 9): `docker compose run|exec` (and up/create) is root, since the
/// compose file SparkBox can't see may mount the host or run privileged; a shell fed its script
/// on stdin runs that script (`echo reboot | sh`, `bash <<< reboot`), and one fed text SparkBox
/// can't read is a change; `ssh <host> <cmd>` runs the command on another machine: never Read,
/// with its power, severity and Docker root carried through.
#[test]
fn classify_compose_as_root_and_what_stdin_shells_and_ssh_run() {
    use super::classify::{escalates, verdict, Power};
    use Kind::*;
    // (command, kind, cuts access, powers, root)
    let table: &[(&str, Kind, bool, Option<Power>, bool)] = &[
        // docker compose run|exec: root, and the command given is classified like a root container's.
        ("docker compose run --rm -v /:/host svc true", Change, false, None, true),
        ("docker compose run svc true", Change, false, None, true),
        ("docker compose exec svc sh", Change, false, None, true),
        ("docker-compose run --rm svc true", Change, false, None, true),
        ("docker-compose exec -T svc id", Change, false, None, true),
        ("/usr/local/bin/docker-compose exec svc id", Change, false, None, true),
        ("docker compose -f /srv/x/compose.yml -p x run -e A=1 -u 0 svc true", Change, false, None, true),
        ("docker --context x compose run svc true", Change, false, None, true),
        ("docker compose exec -u0 svc reboot", Destructive, false, Some(Power::Restart), true),
        ("docker compose run --rm svc chroot /host shutdown -h now", Destructive, false, Some(Power::Off), true),
        ("sudo docker compose up -d", Change, false, None, true),
        ("docker compose create", Change, false, None, true),
        // The compose verbs that only look stay Read; the rest are ordinary changes.
        ("docker compose ps", Read, false, None, false),
        ("docker compose logs -f svc", Read, false, None, false),
        ("docker compose down", Change, false, None, false),
        // A shell fed its script on stdin, by echo, printf or a here-string, runs it.
        ("echo reboot | sh", Destructive, false, Some(Power::Restart), false),
        ("printf 'reboot\\n' | sh", Destructive, false, Some(Power::Restart), false),
        ("echo 'shutdown -h now' | bash -s", Destructive, false, Some(Power::Off), false),
        ("echo reboot | dash -x", Destructive, false, Some(Power::Restart), false),
        ("sh <<< reboot", Destructive, false, Some(Power::Restart), false),
        ("bash <<< 'reboot'", Destructive, false, Some(Power::Restart), false),
        ("echo reboot | sudo sh", Destructive, false, Some(Power::Restart), false),
        ("echo 'systemctl stop ssh' | bash", Destructive, true, None, false),
        ("echo 'docker run --rm -v /:/host alpine true' | sh", Change, false, None, true),
        // …and a shell is never Read, whatever it's fed.
        ("echo uptime | sh", Change, false, None, false),
        ("bash <<< uptime", Change, false, None, false),
        // Fed text SparkBox can't read, or a script file: a change.
        ("curl -s http://x | sh", Change, false, None, false),
        ("cat /tmp/x.sh | bash", Change, false, None, false),
        ("bash -s < /tmp/x.sh", Change, false, None, false),
        ("echo reboot | sh /tmp/x.sh", Change, false, None, false),
        ("sh /tmp/x.sh", Change, false, None, false),
        // `-c` in a cluster of options is still the script.
        ("bash -lc 'reboot'", Destructive, false, Some(Power::Restart), false),
        ("sh -ec 'shutdown -h now'", Destructive, false, Some(Power::Off), false),
        // ssh <host> <cmd>: the command, on another machine; never Read.
        ("ssh cell3 reboot", Destructive, false, Some(Power::Restart), false),
        ("ssh -o StrictHostKeyChecking=no -p 22 sparkuser@cell3 'sudo systemctl poweroff'", Destructive, false, Some(Power::Off), false),
        ("ssh -t cell3 sudo reboot", Destructive, false, Some(Power::Restart), false),
        ("ssh -luser -p2222 cell3 rm -rf /var/lib/docker", Destructive, false, None, false),
        ("ssh -J cell1 cell3 \"echo hi; shutdown -r now\"", Destructive, false, Some(Power::Restart), false),
        ("ssh cell3 uptime", Change, false, None, false),
        ("ssh -i ~/.ssh/id_ed25519 cell3 'cat /etc/hostname'", Change, false, None, false),
        ("ssh cell3 docker run --rm -v /:/host alpine true", Change, false, None, true),
        ("ssh cell3", Change, false, None, false),
        ("ssh -N -L 8000:127.0.0.1:8000 cell3", Change, false, None, false),
        // A login shell there, fed its script.
        ("echo reboot | ssh cell3", Destructive, false, Some(Power::Restart), false),
        ("ssh cell3 <<< 'shutdown -h now'", Destructive, false, Some(Power::Off), false),
        // The init script called ssh isn't the client.
        ("/etc/init.d/ssh stop", Destructive, true, None, false),
        // xargs appends the words it's fed as operands: `CMD ARGS <fed>` carries power and severity.
        ("echo reboot | xargs systemctl", Destructive, false, Some(Power::Restart), false),
        ("echo 'stop ssh' | xargs systemctl", Destructive, true, None, false),
        ("echo reboot | xargs env", Destructive, false, Some(Power::Restart), false),
        ("echo reboot | xargs timeout 5", Destructive, false, Some(Power::Restart), false),
        ("echo /home/sparkuser/models | xargs rm -rf", Destructive, false, None, false),
        ("echo 1 | xargs kill -9", Destructive, false, None, false),
        ("echo run --rm --privileged -v /:/host alpine chroot /host reboot | xargs docker", Destructive, false, Some(Power::Restart), true),
        ("echo rm -f gemma-release | xargs docker", Change, false, None, false),
        ("echo gemma-release | xargs docker stop", Change, false, None, false),
        ("ls | xargs rm", Change, false, None, false),
        ("find / -name x | xargs rm", Change, false, None, false),
        // xargs with no command runs echo; with a reader it stays Read whatever it's fed.
        ("echo reboot | xargs", Read, false, None, false),
        ("echo x | xargs cat", Read, false, None, false),
        ("echo x | xargs ls", Read, false, None, false),
        ("echo x | xargs docker logs", Read, false, None, false),
        // A shell that sources an rcfile (from /dev/stdin too) with -i/--rcfile/--init-file isn't a
        // transparent `-c` wrapper: a change, not a read, whatever the -c script is.
        ("bash --rcfile /dev/stdin -i -c true", Change, false, None, false),
        ("echo 'docker rm -f gemma-release' | bash --rcfile /dev/stdin -i -c true", Change, false, None, false),
        ("bash --init-file /tmp/x -i -c uptime", Change, false, None, false),
        ("bash -ic uptime", Change, false, None, false),
        // A root shell with no command reads stdin, so the fed text runs as root: power carries.
        ("echo reboot | sudo -s", Destructive, false, Some(Power::Restart), false),
        ("echo reboot | sudo -i", Destructive, false, Some(Power::Restart), false),
        ("echo reboot | su", Destructive, false, Some(Power::Restart), false),
        ("echo reboot | su -", Destructive, false, Some(Power::Restart), false),
        ("echo reboot | su root", Destructive, false, Some(Power::Restart), false),
        ("sudo -s <<< reboot", Destructive, false, Some(Power::Restart), false),
        ("su <<< reboot", Destructive, false, Some(Power::Restart), false),
        // ssh reads its own options after the destination too (it permutes), unless `--` ended them.
        ("ssh cell3 -- reboot", Destructive, false, Some(Power::Restart), false),
        ("ssh cell3 -t reboot", Destructive, false, Some(Power::Restart), false),
        ("ssh cell3 -o StrictHostKeyChecking=no reboot", Destructive, false, Some(Power::Restart), false),
    ];
    let wrong: Vec<String> = table
        .iter()
        .filter_map(|(c, kind, cuts, power, root)| {
            let v = verdict(c);
            (v.kind != *kind || v.cuts_access != *cuts || v.power != *power || v.root != *root)
                .then(|| format!("{c:?}: wanted {kind:?} cuts={cuts} power={power:?} root={root}, got {v:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // Root without sudo, or sudo in what a shell or ssh runs: Fleet's as-root path.
    for c in ["docker compose run svc true", "docker-compose exec svc sh", "echo 'sudo reboot' | sh", "sh <<< 'sudo reboot'", "ssh cell3 sudo reboot", "echo 'sudo reboot' | ssh cell3"] {
        assert!(escalates(c), "{c}");
    }
    for c in ["echo uptime | sh", "ssh cell3 uptime", "docker compose ps"] {
        assert!(!escalates(c), "{c}");
    }
}

/// `sh -c`'s script is the first operand after all the shell's options, wherever `-c` sits among
/// them (POSIX `sh -c [options] command_string [command_name [argument…]]`): each of these keeps
/// its power verb. A script that expands anything runs its operands too (`"$@"`, `$0`).
#[test]
fn sh_c_keeps_its_power_verb_whatever_the_option_order() {
    use super::classify::contracts::SH_C_POWER;
    use super::classify::{escalates, verdict, Power};
    use Kind::*;
    let restart = |c: &str| (c.to_string(), Destructive, Some(Power::Restart));
    let mut table: Vec<(String, Kind, Option<Power>)> = SH_C_POWER.iter().filter(|c| !c.contains("shutdown")).map(|c| restart(c)).collect();
    table.push(("timeout 5s sh -c -e 'shutdown -h now'".into(), Destructive, Some(Power::Off)));
    for (c, k, p) in [
        // More of the same: -o/-O take a value, `-` and `--` end the options, clusters anywhere.
        ("bash -O extglob -c reboot", Destructive, Some(Power::Restart)),
        ("sh -eo pipefail -c reboot", Destructive, Some(Power::Restart)),
        ("sh -c - reboot", Destructive, Some(Power::Restart)),
        ("sh -c + reboot", Destructive, Some(Power::Restart)),
        ("bash --norc --noprofile -c reboot", Destructive, Some(Power::Restart)),
        ("bash --posix -c -e 'systemctl poweroff'", Destructive, Some(Power::Off)),
        ("dash -xc -e reboot", Destructive, Some(Power::Restart)),
        ("zsh -c -- reboot", Destructive, Some(Power::Restart)),
        // A script that expands anything may run its operands: `$@`, `$0`, `$1`.
        ("sh -c '\"$@\"' sh reboot", Destructive, Some(Power::Restart)),
        ("sh -c '$0' reboot", Destructive, Some(Power::Restart)),
        ("bash -c 'exec \"$1\"' x shutdown -h now", Destructive, Some(Power::Off)),
        // Reads stay reads; a script with no expansion leaves its $0 alone.
        ("sh -c -e uptime", Read, None),
        ("bash -c -- 'cat /etc/hostname'", Read, None),
        ("sh -ec 'uptime; df -h'", Read, None),
        ("sh -c uptime x", Read, None),
        // No script, a script file, an rcfile or -i: a change, never a read.
        ("sh -c", Change, None),
        ("sh -c -e", Change, None),
        ("sh -- -c reboot", Change, None),
        ("bash -ci uptime", Change, None),
        ("bash -c -i uptime", Change, None),
        ("bash -c --rcfile /tmp/x uptime", Change, None),
        // An rcfile or BASH_ENV/ENV read from stdin runs what it's fed, whatever the -c script.
        ("echo reboot | bash --rcfile /dev/stdin -i -c true", Destructive, Some(Power::Restart)),
        ("echo reboot | bash --init-file=/dev/fd/0 -c true", Destructive, Some(Power::Restart)),
        ("bash --rcfile /dev/stdin -i -c true <<< 'shutdown -h now'", Destructive, Some(Power::Off)),
        ("echo reboot | BASH_ENV=/dev/stdin bash -c true", Destructive, Some(Power::Restart)),
        ("echo reboot | env ENV=/dev/stdin sh -c true", Destructive, Some(Power::Restart)),
        ("echo uptime | BASH_ENV=/dev/stdin bash -c true", Change, None),
        ("BASH_ENV=/dev/stdin bash -c true", Change, None),
    ] {
        table.push((c.to_string(), k, p));
    }
    let wrong: Vec<String> = table
        .iter()
        .filter_map(|(c, kind, power)| {
            let v = verdict(c);
            (v.kind != *kind || v.power != *power).then(|| format!("{c:?}: wanted {kind:?} power={power:?}, got {v:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // sudo in front still escalates; the shell's own options don't.
    assert!(escalates("sudo sh -c -e reboot") && escalates("sh -c -e 'sudo reboot'"));
    assert!(escalates("echo 'sudo reboot' | bash --rcfile /dev/stdin -i -c true") && escalates("echo 'sudo reboot' | BASH_ENV=/dev/stdin bash -c true"));
    assert!(!escalates("sh -c -e reboot"));
    // read_only, the activity log's own read test, reads the script the same way.
    assert!(super::classify::read_only("sh -c -e uptime") && super::classify::read_only("bash -c -- 'cat /etc/hostname'"));
    assert!(!super::classify::read_only("sh -c -e reboot") && !super::classify::read_only("sh -c") && !super::classify::read_only("bash -ci uptime"));
}

/// A power verb however a shell that sources an rcfile, `su`/`runuser`, `env -S`, `eval`, `exec`
/// or `find -exec` wraps it is still a power verb, and still power-*alone* (Fleet gives it
/// `power-one`): the R3-SAFE gaps (b) reached through other options, and the other wrappers. A
/// command that also changes the node some other way (`touch x; reboot`) keeps its power but is
/// no longer power-alone, so it stays a Fleet command.
#[test]
fn a_power_verb_however_wrapped_is_power_and_power_alone() {
    use super::classify::{power_alone, verdict, Power};
    // Pure power, through each wrapper: a power verb, and power-alone.
    let alone = [
        ("bash -ic reboot", Power::Restart),
        ("bash -i -c reboot", Power::Restart),
        ("sh -i -c reboot", Power::Restart),
        ("bash -lic reboot", Power::Restart),
        ("bash --rcfile /dev/null -c reboot", Power::Restart),
        ("bash --rcfile=/dev/null -c reboot", Power::Restart),
        ("bash --init-file /dev/null -c 'systemctl reboot'", Power::Restart),
        ("bash --rcfile /dev/null -c 'shutdown -h now'", Power::Off),
        ("zsh -ic reboot", Power::Restart),
        ("sudo bash -ic reboot", Power::Restart),
        ("su -lc reboot", Power::Restart),
        ("su --command=reboot", Power::Restart),
        ("su - root -c reboot", Power::Restart),
        ("runuser -u root --command=reboot", Power::Restart),
        ("env -S reboot", Power::Restart),
        ("env -S 'systemctl reboot'", Power::Restart),
        ("find / -maxdepth 0 -exec reboot ;", Power::Restart),
        ("eval reboot", Power::Restart),
        ("eval 'systemctl reboot'", Power::Restart),
        ("exec -a x reboot", Power::Restart),
        ("bash -c 'exec -a x reboot'", Power::Restart),
    ];
    for (c, p) in alone {
        assert_eq!(verdict(c).power, Some(p), "power `{c}`");
        assert_eq!(power_alone(c), Some(p), "power_alone `{c}`");
    }
    // A power verb riding with another change: still power, but not alone (a Fleet command).
    for c in [
        "touch /tmp/x; reboot",
        "apt-get -y install nvidia-dkms-580-open; reboot",
        "rm /boot/initrd.img-7.0.0-1019-nvidia; reboot",
        "reboot && docker run --rm -v /:/host alpine true",
        "find / -delete -exec reboot ;",
    ] {
        assert!(verdict(c).power.is_some(), "power `{c}`");
        assert_eq!(power_alone(c), None, "power_alone `{c}`");
    }
    // Not a power verb at all, and a lone power verb.
    assert_eq!(power_alone("uptime"), None);
    assert_eq!(power_alone("sudo reboot"), Some(Power::Restart));
    assert_eq!(power_alone("systemctl poweroff"), Some(Power::Off));
}

/// A read-tier builtin bash re-evaluates its operands for (`[[`, `test`, `[`, `printf -v`,
/// `read`, `ulimit`) runs a `$(…)` in an array subscript or arithmetic context even when it's
/// single-quoted, so one there isn't a read. An ordinary (unquoted) substitution stays its own
/// checked segment, so a genuine read with one is still a read.
#[test]
fn a_reevaluating_builtin_with_a_quoted_substitution_isnt_a_read() {
    for cmd in [
        "[[ 'a[$(touch X)]' -eq 0 ]]",
        "printf -v 'a[$(touch X)]' x",
        "test -v 'a[$(touch X)]'",
        "[ -v 'a[$(touch X)]' ]",
        "ulimit -n 'a[$(touch X)]'",
        "bash -c \"[[ 'a[\\$(touch X)]' -eq 0 ]]\"",
        "[[ 'a[`touch X`]' -eq 0 ]]",
        // Round 4: a `[` (an array subscript) or a substitution in any operand, quoted or not.
        "[ \"$(id -u)\" = 0 ]",
        "test -v 'a[0]'",
        "read 'a[1]' </dev/null",
        // A subscript alone: bash evaluates `y`, whose value can hold a `$(…)`.
        "ulimit -n 'a[y]'",
        // A builtin that sets the shell's own variable changes what later commands run.
        "read PATH </tmp/p; cat /etc/hostname",
        "printf -v PATH /tmp/x",
        "for PATH in /tmp/x; do cat /etc/hostname; done",
        // Round 4 fixes: `[[` is never a read (its -eq… and -v evaluate a variable's value).
        "[[ -d /tmp ]]",
    ] {
        assert_ne!(classify(cmd), Kind::Read, "`{cmd}`");
    }
    // A plain test stays a read, and so does printf without -v (it re-evaluates nothing).
    for cmd in ["[ -f /etc/hosts ]", "test -n \"$HOME\"", "printf '%s\\n' hi", "printf '%s' \"$(id -u)\"", "read -r line </dev/null", "printf -v x %s hi", "for f in a b; do cat \"$f\"; done"] {
        assert_eq!(classify(cmd), Kind::Read, "`{cmd}`");
    }
}

/// This round's reads (Contracts C2, C4's preset read, C5, C6, the dpkg-before list) only read,
/// and the commands next to them that don't are changes: an -o setting outside the dry run's
/// allowlist, `update` even with -s, a plain `snap refresh`, needrestart in any mode but `-r l`.
#[test]
fn this_rounds_update_and_restart_reads_are_reads() {
    use super::classify::contracts::*;
    use super::classify::{escalates, read_only};
    let reads: Vec<String> = vec![
        C2_ONE.into(),
        C2_EVERYTHING.into(),
        c2_dl_limit(),
        C4_PRESET_READ.into(),
        check_script(),
        STAMPS.into(),
        C5_SUDO_FWUPD.into(),
        "fwupdmgr get-devices --json </dev/null 2>&1".into(),
        "fwupdmgr get-history".into(),
        "fwupdmgr get-updates".into(),
        "snap refresh --list".into(),
        "snap list".into(),
        "ollama --version".into(),
        "ollama -v".into(),
        "docker info --format '{{.LiveRestoreEnabled}}'".into(),
        "dpkg-query -W".into(),
        "dpkg-query -W -f='${Version}\\n' libnvidia-compute-580".into(),
        "cat '/var/log/sparkbox/dpkg-before-t1.txt'".into(),
        C6_KERNEL.into(),
        C6_STEP_B.into(),
        "needrestart -b -r l".into(),
        "modinfo -k 7.0.0-1019-nvidia -F version nvidia".into(),
    ];
    for c in &reads {
        assert_eq!(classify(c), Kind::Read, "`{c}`");
        assert!(read_only(c) && !escalates(c), "`{c}`");
    }
    let changes = [
        "apt-get -s -o APT::Update::Pre-Invoke::=reboot install --only-upgrade 'libfoo'",
        "apt-get -s -o DPkg::Pre-Invoke::=reboot -y dist-upgrade",
        "apt-get -s -o Dir::Etc::sourcelist=/tmp/x -y dist-upgrade",
        "apt-get -s -oDPkg::Post-Invoke::=reboot -y dist-upgrade",
        "apt-get -s --option=DPkg::Pre-Invoke::=reboot -y dist-upgrade",
        "apt-get -s -o DPkg::Lock::Timeout=abc -y dist-upgrade",
        "apt-get -s -c /tmp/apt.conf -y dist-upgrade",
        "apt-get -s -o DPkg::Lock::Timeout=300 update",
        "apt-get -t noble-backports -s update",
        "apt-get -o DPkg::Lock::Timeout=300 update",
        "apt-get -o DPkg::Lock::Timeout=300 -y install --only-upgrade 'libfoo'",
        "snap refresh",
        "snap refresh --list x",
        "snap install x",
        "ollama pull llama3",
        "ollama run llama3",
        "fwupdmgr update",
        "fwupdmgr refresh",
        "fwupdmgr get-updates --force",
        "needrestart",
        "needrestart -b",
        "needrestart -b -r a",
        "needrestart -r a -b",
        "needrestart -b -r l -c /tmp/x.conf",
        "dpkg-query -W > /tmp/x",
    ];
    for c in changes {
        assert_ne!(classify(c), Kind::Read, "`{c}`");
    }
    // The upgrade itself (C1) and the refresh (C3) are changes, never reads.
    for c in [C1_ONE.to_string(), c1_everything(), c1_dl_limit(), C3.to_string(), C4_HOLD.to_string(), C4_UNHOLD.to_string()] {
        assert_ne!(classify(&c), Kind::Read, "`{c}`");
    }
}

// ---- P1 round 4 (R4-SAFE): T2 blocker 1, and Fleet's docker root count

/// T2 blocker 1: the command word decides what runs. When the node makes it after SparkBox has
/// looked (a substitution, a variable, a glob, a brace, a `~` path), or it is a path outside the
/// system's folders, or the line uses shell syntax SparkBox doesn't follow, the command is never
/// a read: destructive with the reason, or a change. The reads beside them stay reads.
#[test]
fn a_command_word_from_a_substitution_or_a_variable_is_never_a_read() {
    use super::classify::contracts::*;
    use super::classify::{power_alone, verdict, Power};
    use Kind::*;
    const HIDDEN: &str = "The command it runs comes from a substitution or a variable, so SparkBox can't tell what it is; it treats it as destructive.";
    let syntax = |what: &str| format!("This command uses shell syntax SparkBox doesn't follow ({what}), so it treats it as destructive.");
    let rows: Vec<(&str, Kind, Option<Power>, Option<String>)> = vec![
        ("$(echo uptime)", Destructive, None, Some(HIDDEN.into())),
        ("`echo uptime`", Destructive, None, Some(HIDDEN.into())),
        ("$X", Destructive, None, Some(HIDDEN.into())),
        ("${X}", Destructive, None, Some(HIDDEN.into())),
        ("\"$X\"", Destructive, None, Some(HIDDEN.into())),
        ("$X/cat /etc/hostname", Destructive, None, Some(HIDDEN.into())),
        ("X=reboot; $X", Destructive, None, Some(HIDDEN.into())),
        ("X=$(reboot) true", Destructive, Some(Power::Restart), Some("Takes the node down (reboot).".into())),
        ("cat$IFS/etc/passwd", Destructive, None, Some(HIDDEN.into())),
        ("~/bin/cat x", Destructive, None, Some(HIDDEN.into())),
        ("/tmp/x/cat y", Change, None, None),
        ("c?t /etc/hostname", Destructive, None, Some(HIDDEN.into())),
        ("{cat,/etc/hostname}", Destructive, None, Some(syntax("{…} brace expansion"))),
        ("echo $((1+1))", Destructive, None, Some(syntax("$((…)) arithmetic"))),
        ("echo $'\\x72'", Destructive, None, Some(syntax("$'…' quoting"))),
        ("cat <(reboot)", Destructive, None, Some(syntax("<(…) process substitution"))),
        ("echo hi |& cat", Destructive, None, Some(syntax("|& pipe"))),
        ("alias ls=reboot\nls", Destructive, Some(Power::Restart), Some("Takes the node down (reboot).".into())),
        ("hash -p /tmp/x cat; cat", Change, None, None),
        ("test -v 'a[$(reboot)]'", Destructive, Some(Power::Restart), Some("Takes the node down (reboot).".into())),
        ("printf -v 'a[$(reboot)]' x", Destructive, Some(Power::Restart), Some("Takes the node down (reboot).".into())),
        ("read 'a[$(reboot)]' </dev/null", Destructive, Some(Power::Restart), Some("Takes the node down (reboot).".into())),
        ("ls\u{a0}-la", Destructive, None, Some(syntax("a non-ASCII space"))),
        ("ls\r", Destructive, None, Some(syntax("a control character"))),
    ];
    for (cmd, kind, power, reason) in &rows {
        let v = verdict(cmd);
        assert_eq!((v.kind, v.power, v.reason.as_deref()), (*kind, *power, reason.as_deref()), "`{cmd}`");
        // Fleet's typed copy (`fleet::wrap`) is never a read either: the exit admits a typed command
        // without a fence only when it reads and nothing escalates.
        let typed = super::fleet::wrap(cmd, 30, false);
        assert_ne!(classify(&typed), Read, "`{typed}`");
    }
    // The other forms of each: in a script, under sudo or env, behind a keyword, quoted, as an
    // argument of a wrapper.
    for cmd in [
        "sh -c '$X'",
        "bash -c '`echo uptime`'",
        "sudo $X",
        "env A=1 $X",
        "timeout 5 \"$@\"",
        "if true; then $X; fi",
        "xargs $X < /dev/null",
        "exec \"$0\"",
        "${X:-uptime}",
        "nice -n 5 c*t /etc/hostname",
        "/bin/../tmp/cat x",
        "./cat x",
        "usr/bin/cat x",
        "echo {1..3}",
        "echo $[1+1]",
        "echo $\"x\"",
        "echo \"$((1+1))\"",
        "echo x > >(cat)",
        "case x in x) uptime ;& esac",
        "coproc uptime",
        "function f { uptime; }",
        "select x in a; do uptime; done",
        "ls\u{b}-la",
        "ls\u{0}",
        "ls\u{85}-la",
        "ls\u{2007}-la",
        "ls\u{3000}-la",
        "ls\u{feff}-la",
        "alias ll='ls -l'",
        "unalias ls",
        "trap 'echo bye' EXIT",
        "eval uptime",
        ". /tmp/x",
        "source /tmp/x",
        "set -e",
        "shopt -s expand_aliases",
        "enable -n echo",
        "builtin cat /etc/hostname",
        "declare -x A=1",
        "typeset A=1",
        "let x=1",
        "export -f f",
        "IFS=/ cat /etc/hostname",
        "ENV=/tmp/x cat /etc/hostname",
        "BASH_FUNC_cat%%=x cat /etc/hostname",
        "SHELLOPTS=xtrace cat /etc/hostname",
        "PS4='$(reboot)' cat /etc/hostname",
    ] {
        assert_ne!(classify(cmd), Read, "`{cmd}`");
    }
    // A power verb hidden in syntax SparkBox can't follow is destructive, never power-one: a red,
    // typed-count Fleet command (the named limit).
    for cmd in ["$(echo reboot)", "echo $'\\x72eboot' | sh", "{reboot,}", "r?boot", "$X reboot", "/tmp/x/reboot"] {
        assert_eq!((classify(cmd) == Read, power_alone(cmd)), (false, None), "`{cmd}`");
    }
    // A trap's action and an alias's text are judged as scripts: their power verb counts.
    assert_eq!(verdict("trap reboot EXIT").power, Some(Power::Restart));
    assert_eq!(verdict("alias ls='systemctl poweroff'").power, Some(Power::Off));
    // Reads that stay reads: a `~` or an expansion in an argument, a system path, quoted parentheses,
    // this round's reads (C5, C6′, C-P2, C-SH, C-R's models list and protocol probe, C2′).
    let reads: Vec<String> = vec![
        "uptime".into(),
        "cat /etc/hostname".into(),
        "/usr/bin/cat /etc/hostname".into(),
        "/bin/ls -la".into(),
        "ls -la ~/models".into(),
        "echo \"$HOME\"".into(),
        "grep -E '^(a|b)' /proc/meminfo".into(),
        "find /tmp -name '*.log' -print".into(),
        "echo 'x{a,b}' \"$'x'\" '$((1))'".into(),
        "ls ~/.cache/huggingface/hub/models--*".into(),
        check_script(),
        C6_STEP_B.into(),
        C_P.into(),
        C_SH.into(),
        R_MODELS.into(),
        R_OPENAPI.into(),
        C2_PINNED.into(),
    ];
    for c in &reads {
        assert_eq!(classify(c), Read, "`{c}`");
    }
    for c in [C_L.to_string(), c_l_no_key(), C_U.into(), C_S.into(), C1_PINNED.into(), R_SANITY.into(), R_SANITY_THINKING.into()] {
        assert_ne!(classify(&c), Read, "`{c}`");
    }
    // C2′'s dry run reads only named packages, pinned or not; a path or a pattern isn't one.
    for c in ["apt-get -s install ./x.deb", "apt-get -s install '/tmp/x.deb'", "apt-get -s -y install --only-upgrade 'lib*'", "apt-get -s install 'Libfoo'", "apt-get -s install 'libfoo='"] {
        assert_ne!(classify(c), Read, "`{c}`");
    }
}

/// Fleet's docker root count (P1.42, round 4): each `docker run`/`create`/`compose run` option
/// that reaches the host's namespaces, devices, kernel settings or other containers' data makes
/// it root (the typed count, the canary, one node at a time), in its `=` and space forms.
#[test]
fn every_docker_option_that_reaches_the_host_is_root() {
    use super::classify::verdict;
    let options = [
        "--userns=host", "--userns host", "--cgroupns=host", "--cgroupns host", "--uts=host", "--uts host", "--ipc=host", "--ipc host",
        "--pid=container:vllm", "--pid container:vllm", "--network=container:vllm", "--network container:vllm", "--net=container:vllm", "--net container:vllm",
        "--security-opt=seccomp=unconfined", "--security-opt seccomp=unconfined", "--security-opt=no-new-privileges", "--sysctl=net.ipv4.ip_forward=1", "--sysctl net.ipv4.ip_forward=1",
        "--volumes-from=vllm", "--volumes-from vllm", "-v hostroot:/h", "-vhostroot:/h", "--volume=hostroot:/h", "--volume hostroot:/h:ro",
        "--mount=source=v,target=/h", "--mount source=v,target=/h", "--mount type=volume,source=v,target=/h", "--mount=type=bind,source=/,target=/h",
        "--device-cgroup-rule='c 1:1 rwm'", "--device-cgroup-rule 'c 1:1 rwm'", "--cgroup-parent=/x", "--cgroup-parent /x",
        "--runtime=runsc", "--runtime runsc",
    ];
    for o in options {
        for cmd in [format!("docker run -d {o} busybox true"), format!("docker create {o} busybox true"), format!("docker container run --rm {o} busybox true"), format!("docker compose run {o} svc true")] {
            let v = verdict(&cmd);
            assert!(v.root && v.kind != Kind::Read, "`{cmd}`: {v:?}");
        }
    }
    // Not root: the default runtimes, a tmpfs, an anonymous volume, a private namespace.
    for o in ["--runtime=runc", "--runtime nvidia", "--mount type=tmpfs,target=/t", "-v /data", "--ipc=private", "--network bridge", "--pid=private", "--userns=", "--uts=private"] {
        let cmd = format!("docker run -d {o} busybox true");
        assert!(!verdict(&cmd).root, "`{cmd}`");
    }
}

// ---- P1 round 4 fixes (R4-SAFE): what bash reads that dash doesn't, and each wrapper's options

/// Each command, its `bash -c` copy and Fleet's typed copy (`fleet::wrap`).
fn three_ways(cmd: &str) -> [String; 3] {
    [cmd.to_string(), format!("bash -c {}", crate::live::sh_quote(cmd)), super::fleet::wrap(cmd, 30, false)]
}

/// bash evaluates a variable's *value* as an expression in `[[ … -eq … ]]` and `[[ -v … ]]`,
/// `${x[…]}`, `${x:n}`, `${!x}`, `test -v` and expands it again in `${x@P}`, so a `$(…)` read into
/// the value runs though no word shows it (`read y </tmp/p; [[ $y -eq 0 ]]`). None of these is a
/// read, as typed, through `bash -c` or in Fleet's wrapper; bash's expansions are syntax SparkBox
/// doesn't follow (dash refuses them), and `${x=…}`/`${x:=…}` set x: a change. Ubuntu 24.04's bash
/// 5.2.21 ran the `$(…)` for every row (`y='a[$(touch M)]'`, docker ubuntu:24.04).
#[test]
fn bash_evaluating_a_value_is_never_a_read() {
    use super::classify::{power_alone, verdict};
    let bash_only = "This command uses shell syntax SparkBox doesn't follow (a bash-only ${…} expansion), so it treats it as destructive.";
    for cmd in [
        // The safety re-test's typed reads.
        "bash -c 'read y </tmp/p; [[ $y -eq 0 ]]'",
        "bash -c 'read y </tmp/p; echo ${x[y]}'",
        "bash -c 'read y </tmp/p; echo ${x:y}'",
        "bash -c 'read y </tmp/p; echo ${!y}'",
        "bash -c \"echo 'a[\\$(touch /tmp/M)]' | { read y; [[ \\$y -eq 0 ]]; }\"",
        "[[ y -eq 0 ]]",
        "[[ $y -eq 0 ]]",
        "read y </tmp/p; [[ $y -gt 1 ]]",
        "[[ -v $y ]]",
        "echo ${x[y]}",
        "echo \"${x[y]}\"",
        "echo ${x[$y]}",
        "echo ${#x[y]}",
        "echo ${x:y}",
        "echo ${x: y}",
        "echo \"${x:y}\"",
        "set -- 1; echo ${@:y}",
        "echo ${!y}",
        "test -n \"${!y}\"",
        "echo ${y@P}",
        "echo \"${y@P}\"",
        "test -v \"$y\"",
        "[ -v \"$y\" ]",
        "test -v y",
        // The staging re-test's row 106 and its sibling.
        "echo ${x:='a[$(touch /tmp/sbconf-N)]'} ${a[x]}",
        "echo ${x:='a[$(touch /tmp/sbconf-N)]'} ${y:x}",
        // Assignments as they expand.
        "echo ${x=1}",
        "echo ${x:=1}",
        "echo \"${x:=1}\"",
        "cat ${f=/etc/hostname}",
    ] {
        for c in three_ways(cmd) {
            assert_ne!(classify(&c), Kind::Read, "`{c}`");
            assert_eq!(power_alone(&c), None, "`{c}`");
        }
    }
    for cmd in ["echo ${x[y]}", "echo \"${x:y}\"", "echo ${!y}", "echo \"${y@P}\"", "echo ${@:2}", "cat ${x:-${y[1]}}"] {
        let v = verdict(cmd);
        assert_eq!((v.kind, v.reason.as_deref()), (Kind::Destructive, Some(bash_only)), "`{cmd}`");
    }
    assert_eq!(classify("echo ${x:=1}"), Kind::Change);
    // dash's own expansions stay reads, and so does any of these single-quoted.
    for cmd in [
        "echo ${x:-a} \"${x:-a}\" ${x-a} ${x:+a} ${x:?no}",
        "echo ${#x} ${x#y} ${x%%.*} ${x/a/b} ${x^^}",
        "echo ${!} ${@} ${#} ${*} ${1} ${10}",
        "echo '${x[y]}' '${!y}' '[[ y -eq 0 ]]' '${x:=1}'",
        "[ -f /etc/hosts ] && test -n \"$HOME\"",
    ] {
        assert_eq!(classify(cmd), Kind::Read, "`{cmd}`");
    }
}

/// bash reads `>&word` and `N>&word` as a write to the file `word` (`>&file` is `>file 2>&1`)
/// unless `word` is a stream number or `-`; dash refuses it. So it's a write: never a read, typed
/// or through `bash -c`, and one into an account file cuts access. Ubuntu 24.04's bash created the
/// file for each form (`pwd >&/tmp/G2`, `1>&`, `>& /tmp/G4`, `>&2x`).
#[test]
fn a_redirect_to_a_file_after_and_is_a_write() {
    use super::classify::verdict;
    for cmd in [
        "uptime >&/tmp/sbconf-N",
        "uptime >& /tmp/x",
        "uptime 1>&/tmp/x",
        "uptime 2>&/tmp/x",
        "uptime >&2x",
        "uptime >&-x",
        "cat /etc/hostname >&/home/sparkuser/.bashrc",
        "ls >&/dev/tcp/host/port",
    ] {
        for c in three_ways(cmd) {
            assert_ne!(classify(&c), Kind::Read, "`{c}`");
        }
    }
    assert!(verdict("cat /etc/hostname >&/etc/passwd").cuts_access);
    assert!(verdict("bash -c 'cat /etc/hostname >& /etc/shadow'").cuts_access);
    // Only one unquoted digit names a stream: dash reads `'1'>f`, `\1>f` and `01>f` as an argument
    // and `>f`, so `kill -9 '1'>/dev/null` and `kill -9 01>/dev/null` kill PID 1.
    for cmd in ["kill -9 '1'>/dev/null", "kill -9 \\1>/dev/null", "kill -9 \"1\">/dev/null", "kill -9 01>/dev/null", "kill -9 01"] {
        assert_eq!(classify(cmd), Kind::Destructive, "`{cmd}`");
    }
    assert_eq!(classify("kill -9 1>/dev/null"), Kind::Change, "fd 1 to /dev/null: kill with no PID");
    // A stream copied or closed writes nothing.
    for cmd in ["uptime >&2", "uptime 2>&1", "uptime 1>&2", "uptime >& 2", "uptime >&-", "uptime 2>&1 | cat", "uptime 2>&1;uptime", "uptime >&2 </dev/null"] {
        assert_eq!(classify(cmd), Kind::Read, "`{cmd}`");
    }
}

/// A here-document's body is text, and an odd quote in one carried the parser past its delimiter
/// onto the commands after it (`cat <<A` … `reboot` … `cat <<B`, a plain change before). Now any
/// here-document is syntax SparkBox doesn't follow: destructive, so a power verb hidden that way is
/// a red, typed-count Fleet command, never a plain change or power-one. dash ran both rows' hidden
/// command.
#[test]
fn a_here_document_is_syntax_sparkbox_doesnt_follow() {
    use super::classify::{power_alone, verdict};
    let why = "This command uses shell syntax SparkBox doesn't follow (a here-document), so it treats it as destructive.";
    for cmd in ["cat <<A\n'\nA\nreboot\ncat <<B\n'\nB", "cat <<A\n\"\nA\nreboot\ncat <<B\n\"\nB", "cat <<EOF\nhello\nEOF", "sh <<'EOF'\nreboot\nEOF", "cat <<-EOF\n\tx\n\tEOF"] {
        let v = verdict(cmd);
        assert_eq!((v.kind, v.reason.as_deref()), (Kind::Destructive, Some(why)), "`{cmd}`");
        for c in three_ways(cmd) {
            assert_eq!((classify(&c), power_alone(&c)), (Kind::Destructive, None), "`{c}`");
        }
    }
    // A here-string is still followed (its text is the command's stdin): a change.
    assert_eq!(classify("cat <<< hello"), Kind::Change);
}

/// Each wrapper's options are read the way the wrapper reads them (`classify::wrapped`, `XARGS`,
/// `WATCH`, `ENV`): a flag takes no word (`command -p`, `exec -c`, `chrt -i`, `taskset -c`, `xargs
/// -i`, `watch -d`), a value-taking option takes one (`exec -a NAME`, `nice -n 1`, `stdbuf -o L`),
/// and watch without -x joins its words and runs them with sh. The staging re-test's twelve forms
/// (rows 95, 106, 143-145, 147-153: each a read before, each ran its hidden `touch` on Ubuntu
/// 24.04), the three that deleted a folder, and the rest of its wrapper rows are no reads.
#[test]
fn each_wrapper_is_read_with_its_own_options() {
    use super::classify::{verdict, Kind::*};
    for cmd in [
        "uptime >&/tmp/sbconf-N",
        "echo ${x:='a[$(touch /tmp/sbconf-N)]'} ${a[x]}",
        "TERM=xterm timeout 3 watch -n 1 uptime ';' touch /tmp/sbconf-N",
        "TERM=xterm timeout 3 watch -n 1 echo '$(touch /tmp/sbconf-N)'",
        "TERM=xterm timeout 3 watch -d touch cat /tmp/sbconf-N",
        "echo x | xargs -i touch cat /tmp/sbconf-N",
        "echo x | xargs --replace touch cat /tmp/sbconf-N",
        "taskset -c 0 touch cat /tmp/sbconf-N",
        "chrt -i 0 touch cat /tmp/sbconf-N",
        "command -p touch cat /tmp/sbconf-N",
        "exec -c touch cat /tmp/sbconf-N",
        "exec -a cat touch /tmp/sbconf-N",
        "env -a cat touch /tmp/sbconf-N",
        "nice -n 1 touch cat /tmp/sbconf-N",
        "stdbuf -o L touch cat /tmp/sbconf-N",
        "ionice -c 3 -t touch cat /tmp/sbconf-N",
        "timeout --foreground 5 touch cat /tmp/sbconf-N",
        "numactl -C 0 touch cat /tmp/sbconf-N",
        "/usr/bin/time -o /tmp/sbconf-N cat /etc/hostname",
        "bash -c 'cat /etc/hostname >&/home/sparkuser/.bashrc'",
    ] {
        for c in three_ways(cmd) {
            assert_ne!(classify(&c), Read, "`{c}`");
        }
    }
    // The three that deleted a folder in staging: destructive, as `rm -rf ~/models` is.
    for cmd in ["command -p rm cat -rf ~/models", "echo x | xargs -i rm cat -rf ~/models", "TERM=xterm timeout 5 watch -n 9 ls ';' rm -rf ~/models"] {
        for c in three_ways(cmd) {
            assert_eq!(classify(&c), Destructive, "`{c}`");
        }
    }
    // An option a wrapper's table doesn't list (or a long one abbreviated): what runs can't be told.
    for (cmd, name) in [("nice --frobnicate 5 cat /etc/hostname", "nice"), ("nice --adj 5 reboot", "nice"), ("xargs --foo cat", "xargs"), ("watch -z ls", "watch"), ("taskset --cpu 0 cat x", "taskset"), ("chrt -Z 1 cat x", "chrt"), ("env --foo cat x", "env"), ("sudo command -x cat x", "command")] {
        let v = verdict(cmd);
        let why = format!("This command uses shell syntax SparkBox doesn't follow ({name} with an option it doesn't know), so it treats it as destructive.");
        assert_eq!((v.kind, v.reason.as_deref()), (Destructive, Some(why.as_str())), "`{cmd}`");
    }
    // The same wrappers used as written stay reads, and so do the ones that only show a setting.
    for cmd in [
        "nice -n 5 cat /etc/hostname",
        "nice -5 cat /etc/hostname",
        "nice --adjustment=5 cat /etc/hostname",
        "nice --adjustment 5 cat /etc/hostname",
        "nohup cat /etc/hostname",
        "ionice -c 3 cat /etc/hostname",
        "ionice -c3 -n7 cat /etc/hostname",
        "stdbuf -oL cat /etc/hostname",
        "command -v cat",
        "command -p cat /etc/hostname",
        "exec -a x cat /etc/hostname",
        "taskset -c 0-3 cat /etc/hostname",
        "taskset 0x3 cat /etc/hostname",
        "chrt -o 0 cat /etc/hostname",
        "chrt -i 0 cat /etc/hostname",
        "numactl -N 0 cat /etc/hostname",
        "numactl --cpunodebind=0 cat /etc/hostname",
        "numactl -H",
        "echo a | xargs -0 -n 1 cat",
        "echo a | xargs -r -I{} cat {}",
        "echo a | xargs -i cat {}",
        "echo a | xargs --replace={} cat {}",
        "watch -n 5 uptime",
        "watch -d -n 5 uptime",
        "watch -x cat /etc/hostname",
        "watch 'uptime; df -h'",
        "env -i cat /etc/hostname",
        "env -u FOO cat /etc/hostname",
        "env LANG=C cat /etc/hostname",
        "taskset -p 4242",
        "taskset -cp 4242",
        "chrt -p 4242",
        "chrt -m",
        "ionice -p 4242",
        "ionice -p 4242 4243",
    ] {
        assert_eq!(classify(cmd), Read, "`{cmd}`");
    }
}

/// Round 5's safety review, the reason p1-t2 stayed untagged. getopt_long takes any unique start of
/// a long option and env's -S may end a cluster, so every command here wrote or ran something on
/// real GNU tools (sed 4.9, sort 9.4, tar 1.35, mawk, env 9.4: `staging_read_tier_options`) while
/// exact-match rules called it a read. Each is a change now, typed, through `bash -c` and in Fleet's
/// wrapper; curl's and nvidia-smi's are unconfirmed on the real versions and barred all the same.
#[test]
fn a_shortened_option_or_one_the_rules_dont_know_is_no_read() {
    use super::classify::{escalates, read_only, verdict, Kind::*, Power};
    for cmd in [
        // env -S at the end of a cluster.
        "env -iS 'rm x'",
        "env -vS 'rm x'",
        "env -0S 'rm x'",
        "env -iS'rm x'",
        "env -iu FOO -S 'rm x'",
        "env -i -S 'rm x'",
        "env --split-string='rm x'",
        "env --split-string 'rm x'",
        "env --split='rm x'",
        // sed
        "sed --in s/a/b/ f",
        "sed --i s/a/b/ f",
        "sed --in-pl s/a/b/ f",
        "sed --in-place=.bak s/a/b/ f",
        "sed -n p f --in-pl",
        "sed s/a/b/ --in f",
        "sed -n -e p --in f",
        "sed --fi x f",
        "sed --expr='1e reboot' f",
        "sed -y p f",
        // sort
        "sort --o=f g",
        "sort --ou=f g",
        "sort --outp=/tmp/x f",
        "sort --outpu /tmp/x f",
        "sort --comp=PROG f",
        "sort --compress=/tmp/x f",
        "sort --compress-prog=/tmp/x f",
        // tar: the list of what may sit beside -t, spelled out or shortened.
        "tar -tf a --use=PROG",
        "tar -tf a --use-comp=/tmp/x",
        "tar -tf a --checkpoint=1 --checkpoint-a=exec=CMD",
        "tar -tf a --checkpoint-action=exec=CMD",
        "tar -tvf a --index-file=f",
        "tar tvf a --index=/tmp/x",
        "tar tvfz a --index-f=/tmp/x",
        "tar -tf a --to-comm='rm x'",
        "tar -tf a --to-c='rm x'",
        "tar -tf a --rsh-c=/tmp/x",
        "tar -tf a --rmt-command=/tmp/x",
        "tar -tf a --volno-file=/tmp/x",
        "tar -tf a --extract",
        "tar -tf a --extract -C /tmp",
        "tar -tf a -x",
        "tar -t -x -f f",
        "tar -tf a --create",
        "tar -tf a --delete x",
        "tar -tf a --append",
        "tar -tf a --update",
        "tar -tf a --catenate",
        "tar -tf a --remove-files",
        "tar -tf a -T /tmp/x",
        "tar -tf a -g /tmp/x",
        "tar -tf a -C /tmp",
        // awk: only -F, -v and one program text.
        "awk -W exec file",
        "awk -Wexec=/tmp/x",
        "awk -W interactive 'BEGIN{}'",
        "mawk -W exec /tmp/x",
        "gawk -o/x 'BEGIN{}'",
        "gawk -d/x 'BEGIN{}'",
        "gawk -p/x 'BEGIN{}'",
        "gawk -D/x 'BEGIN{}'",
        "gawk --pretty-print=/x 'BEGIN{}'",
        "gawk --dump-variables=/x 'BEGIN{}'",
        "gawk --profile=/x 'BEGIN{}'",
        "gawk --load=filefuncs 'BEGIN{}'",
        "gawk --incl=x 'BEGIN{}'",
        "awk '{print}' f -W exec",
        // curl
        "curl --outp /tmp/x http://a",
        "curl --out /tmp/x http://a",
        "curl --reque POST http://a",
        "curl --req DELETE http://a",
        "curl --dat x http://a",
        "curl --upl /tmp/x http://a",
        "curl --upload-f /tmp/x http://a",
        "curl --conf /tmp/x http://a",
        "curl --expand-output /tmp/x http://a",
        "curl --expand-url http://a",
        "curl --expand-data x http://a",
        "curl --variable a=b --expand-output /tmp/x http://a",
        "curl --dump-h /tmp/x http://a",
        "curl --cookie-j /tmp/x http://a",
        "curl --for a=b http://a",
        "curl --js '{}' http://a",
        "curl --remote-n http://a/x",
        "curl --stde /tmp/x http://a",
        "curl --trace-a /tmp/x http://a",
        "curl --tr /tmp/x http://a",
        "curl --lib /tmp/x http://a",
        "curl --etag-s /tmp/x http://a",
        "curl --hst /tmp/x http://a",
        "curl --alt /tmp/x http://a",
        "curl --quo 'DELE x' ftp://a",
        "curl --quote 'DELE x' ftp://a",
        "curl -Q 'DELE x' ftp://a",
        "curl -P x ftp://a",
        "curl -Q 'DELE x' http://a",
        "curl -P x http://a",
        "curl -sQ x http://a",
        "curl --ftp-create-dirs ftp://a/b",
        "curl --output=/tmp/x http://a",
        "curl gopher://127.0.0.1:6379/_FLUSHALL",
        "curl dict://a:6379/flushall",
        "curl smb://a/b",
        "curl sftp://a/b",
        "curl scp://a/b",
        "curl telnet://a",
        "curl ftp://a/b",
        // wget
        "wget --spider --method=DELETE --body-data=x http://a",
        "wget --spider --post-data=x http://a",
        "wget --spider --delete-after http://a",
        "wget --spider --output-f=/tmp/x http://a",
        "wget --spider -a /tmp/x http://a",
        "wget --output-d=/tmp/x http://a",
        "wget -qO/tmp/x http://a",
        "wget -O- -o /tmp/log http://a",
        // journalctl
        "journalctl --rota",
        "journalctl --flu",
        "journalctl --sy",
        "journalctl --setup",
        "journalctl --update",
        "journalctl --vacu=1",
        "journalctl --vacuum-s=1",
        "journalctl --relin",
        "journalctl --smart",
        "journalctl --interval=1 --setup-k",
        "journalctl --cursor-f=/tmp/x",
        // nvidia-smi
        "nvidia-smi --gpu-r",
        "nvidia-smi --power=100",
        "nvidia-smi --compute=0",
        "nvidia-smi --gom=COMPUTE",
        "nvidia-smi --filen=/tmp/x",
        // The same bars on the tools still kept as denylists, shortened.
        "hostname --fi /tmp/x",
        "date --se 2030-01-01",
        "iconv --outp=/tmp/x f",
        "dmesg --cl",
        "smartctl --sm=on /dev/nvme0",
        "sudo --log cat f",
        "sudo --sh cat f",
        "sudo --pres cat f",
        "sudo --chroot=/tmp/x cat f",
        // ip takes the start of a word too, and `s` on a link is set.
        "ip -bat /tmp/cmds",
        "ip --ba /tmp/cmds",
        "ip -fo -b /tmp/cmds",
        "ip link s eth0 mtu 1",
        "ip l s eth0 up",
    ] {
        for c in three_ways(cmd) {
            assert_ne!(classify(&c), Read, "`{c}`");
        }
        assert!(!read_only(cmd), "`{cmd}`");
    }
    // Every wrapper passes them on, sudo's root write included.
    for cmd in ["nice sed --in s/a/b/ f", "nohup sed --in s/a/b/ f", "command -p sed --in s/a/b/ f", "timeout 5 sed --in s/a/b/ f", "time sed --in s/a/b/ f", "stdbuf -o0 sed --in s/a/b/ f", "env sed --in s/a/b/ f", "env -i sed --in s/a/b/ f", "watch -x sed --in s/a/b/ f", "sh -c 'sed --in s/a/b/ f'", "bash -c 'sort --o=x f'", "ls | sed --in s/a/b/ f", "xargs sort --o=x", "sudo sed --in s/a/b/ /etc/hosts", "sudo sort --o=/tmp/x f"] {
        assert_ne!(classify(cmd), Read, "`{cmd}`");
    }
    assert!(escalates("sudo sed --in s/a/b/ /etc/hosts"));
    // On the files logins depend on it's worse than a change, however it's spelled.
    for cmd in ["sudo sed --in 's/a/b/' /etc/ssh/sshd_config", "sudo sort --o=/etc/passwd f", "sudo env -iS 'sed -i s/a/b/ /etc/shadow'"] {
        let v = verdict(cmd);
        assert!(v.kind == Destructive && v.cuts_access, "`{cmd}`: {v:?}");
    }
    // What the string runs counts as if it were typed, whichever way env is given it.
    for cmd in ["env -iS reboot", "env -vS reboot", "env -iSreboot", "env -iS 'sudo reboot'", "sudo env -iS reboot", "env -S '-i reboot'", "env -i -S 'LANG=C reboot'", "env --split-string='-i reboot'", "env -iS 'systemctl' reboot"] {
        let v = verdict(cmd);
        assert_eq!((v.kind, v.power), (Destructive, Some(Power::Restart)), "`{cmd}`");
        assert_eq!(super::classify::power_alone(cmd), Some(Power::Restart), "`{cmd}`");
    }
    assert!(escalates("env -iS 'sudo reboot'"));
    assert!(!escalates("env -iS 'ls'"));
    assert_eq!(classify("env -iS 'rm -rf ~/models'"), Destructive);
    assert_eq!(classify("env -iS 'docker stop victim'"), Change);
    // env's own escapes aren't the shell's, so what runs can't be told.
    for cmd in ["env -S 'rm\\_-rf\\_~/models'", "env -iS '${X} y'"] {
        assert_eq!(classify(cmd), Destructive, "`{cmd}`");
    }
    // A wrapper's option shortened may take the next word as its value: what runs can't be told.
    for (cmd, name) in [("timeout --sig KILL 5 rm -rf x", "timeout"), ("timeout --k 5 10 reboot", "timeout"), ("sudo --us root reboot", "sudo"), ("env --split='rm x' y", "env")] {
        let v = verdict(cmd);
        let why = format!("This command uses shell syntax SparkBox doesn't follow ({name} with an option it doesn't know), so it treats it as destructive.");
        assert_eq!((v.kind, v.reason.as_deref()), (Destructive, Some(why.as_str())), "`{cmd}`");
    }
    // What SparkBox sends, and the plain looks, stay reads with the options spelled out.
    for cmd in [
        "sed -n '/start/,/end/p' f",
        "sed --quiet --regexp-extended 's/a/b/p' f",
        "sed --expression='s/a/b/' f",
        "sed --line-length=1 p f",
        "sed --line-length 1 p f",
        "sed --null-data p f",
        "sed -Ez p f",
        "sort --human-numeric-sort --reverse --key=2 f",
        "sort -k2 -n -r f",
        "tar -tf a",
        "tar -tvzf a",
        "tar tvzf a",
        "tar -tjf a",
        "tar --list --file=a",
        "tar --list --file a --gzip --verbose",
        "tar -tf a -- member",
        "awk -F: '{print $1}' f",
        "awk -F : -v x=1 '{print x}' f",
        "awk -vx=1 '{print x}' f -",
        "awk -- '{print}' f",
        "curl -s -m 2 http://127.0.0.1:8000/metrics",
        "curl -s -o /dev/null -m 3 -w '%{http_code}\n' http://h:8000/health",
        "curl -s -m 10 -H @- -w '\n%{http_code}' http://h:8000/v1/models",
        "curl -sD - http://x",
        "curl -sS --fail-with-body --max-time 5 --location https://x",
        "curl -X GET http://x",
        "curl --request HEAD http://x",
        "curl --url http://a",
        "curl file:///tmp/x",
        "wget -qO- http://a",
        "wget -O /dev/null http://a",
        "wget --spider --timeout=5 --tries=1 http://a",
        "journalctl --list-boots -o json --no-pager",
        "journalctl -f --since -10min --no-hostname -u docker.service",
        "journalctl -k -b -f --no-tail -o json",
        "journalctl -b '-1' -n 50 -o json --no-pager _PID=1 --output-fields=MESSAGE,__REALTIME_TIMESTAMP",
        "journalctl --unit=x --output=json",
        "journalctl --disk-usage",
        "nvidia-smi --query-gpu=name,driver_version --format=csv,noheader",
        "nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits",
        "nvidia-smi -q -d CLOCK",
        "nvidia-smi topo --matrix",
        "hostname --fqdn",
        "ip -s link",
        "ip a s",
        "ip addr s dev eth0",
        "ip route s",
        "ip -br a",
        "sudo -n cat f",
        "sudo --non-interactive cat f",
        "sudo --user=root cat f",
        "env -i cat f",
        "timeout --foreground 5 cat f",
        "timeout --signal=TERM --kill-after=5s 30s cat f",
        "timeout 5",
        "python3 ~/.sparkbox/sparkjob.py list --look",
    ] {
        assert_eq!(classify(cmd), Read, "`{cmd}`");
    }
    // A word only starting like an option isn't one that passes: apt's and wget's own reads.
    for cmd in ["apt-get --s install x", "apt --no install x", "wget --sp http://a"] {
        assert_ne!(classify(cmd), Read, "`{cmd}`");
    }
}

/// Setting a running process's affinity, scheduling policy or I/O class changes the node (the
/// staging re-test moved a process's affinity from 3ffff to 1, its I/O class to idle and its policy
/// to SCHED_BATCH): a change, as renice is, never a read. Only showing one stays a read (above).
#[test]
fn setting_a_processs_affinity_policy_or_io_class_is_a_change() {
    for cmd in [
        "taskset -p 0x1 4242",
        "taskset -cp 0 4242",
        "taskset -p -c 0-3 4242",
        "taskset --pid 1 4242",
        "ionice -c 3 -p 4242",
        "ionice -n 7 -p 4242",
        "ionice -c3 -P 100",
        "ionice -c 2 -u 1000",
        "chrt -o -p 0 4242",
        "chrt -b -p 0 4242",
        "chrt -p 5 4242",
        "chrt --pid 5 4242",
    ] {
        for c in three_ways(cmd) {
            assert_eq!(classify(&c), Kind::Change, "`{c}`");
        }
    }
}

// ── P2 round 1: the read tier's fresh review ───────────────────────────────────────────────────

/// How a READ_ANY tool was cleared. Walked with its man page and the real binary on Ubuntu 24.04
/// arm64 (util-linux 2.39.3, procps 4.0.4, apt 2.8.3, ripgrep 14.1.0, sysstat 12.6.1, tree 2.1.1,
/// lsof 4.95.0, diffutils 3.10, gzip 1.12, bind9 9.18.39, jq 1.7.1, file 5.45, glibc 2.39), with
/// strace where the tool might run something. `staging_read_tier_options` is the real half.
enum Cleared {
    /// No writing option, and no way to run anything.
    NoWriter(&'static str),
    /// A shell builtin or keyword: there's no man page to walk.
    Builtin(&'static str),
    /// Only a rule keeps it a read: the rule, and commands that wrote or ran on the real tool (or,
    /// where the row says so, that its manual says would). Each must not be a read.
    Rule(&'static str, &'static [&'static str]),
}
use Cleared::{Builtin, NoWriter, Rule};

const NO_WRITER: &str = "no writing option (man page, real binary)";

/// One row per READ_ANY tool. A new tool needs its row, so nothing joins the read tier unasked.
const READ_TIER: &[(&str, Cleared)] = &[
    ("cat", NoWriter(NO_WRITER)),
    ("head", NoWriter(NO_WRITER)),
    ("tail", NoWriter(NO_WRITER)),
    ("less", Rule("+cmd, -o, -O, --log-file, --LOG-FILE, -k and --lesskey-*; in a terminal its keys run commands, and a pty reads only a follower", &["less -o /tmp/x f", "less -O/tmp/x f", "less --log-file=/tmp/x f", "less --LOG=/tmp/x f", "less -k /tmp/keys f", "less +!reboot f"])),
    ("more", Rule("as less: +cmd", &["more +!reboot f"])),
    ("ls", NoWriter(NO_WRITER)),
    ("df", NoWriter("no writing option (--sync only flushes)")),
    ("du", NoWriter(NO_WRITER)),
    ("free", NoWriter(NO_WRITER)),
    ("uptime", NoWriter(NO_WRITER)),
    ("uname", NoWriter(NO_WRITER)),
    ("id", NoWriter(NO_WRITER)),
    ("whoami", NoWriter(NO_WRITER)),
    ("groups", NoWriter(NO_WRITER)),
    ("nproc", NoWriter(NO_WRITER)),
    ("lscpu", NoWriter(NO_WRITER)),
    ("lsblk", NoWriter(NO_WRITER)),
    ("lspci", Rule("an allowlist: -q and -Q cache what they look up (~/.pciids-cache), and -O net.cache_name moves that file", &["lspci -Q -O net.cache_name=/tmp/x", "lspci -q", "lspci -qq", "lspci -vQ", "lspci -Onet.cache_name=/tmp/x"])),
    ("lsusb", NoWriter(NO_WRITER)),
    ("lsmod", NoWriter(NO_WRITER)),
    ("lsof", NoWriter("no writing option: -D isn't in this build (\"unsupported option\"), +D only scans")),
    ("lshw", Rule("-dump, though Ubuntu's build has no SQLite and refuses it", &["lshw -dump /tmp/x", "lshw --dump /tmp/x"])),
    ("lsns", NoWriter(NO_WRITER)),
    ("ps", NoWriter(NO_WRITER)),
    ("pgrep", NoWriter("no writing option; unlike pkill, --signal only reads a name")),
    ("pidof", NoWriter(NO_WRITER)),
    ("top", NoWriter("no writing option: keys on a pipe write nothing, and in a terminal it's no read (a pty reads only a follower). It makes an empty ~/.config/procps")),
    ("w", NoWriter(NO_WRITER)),
    ("who", NoWriter(NO_WRITER)),
    ("last", NoWriter(NO_WRITER)),
    ("ss", Rule("-K closes sockets, -D dumps into a file", &["ss -K dst 1.2.3.4", "ss -D /tmp/x", "ss -tD/tmp/x", "ss --kill", "ss --diag=/tmp/x"])),
    ("netstat", NoWriter(NO_WRITER)),
    ("echo", Builtin("a builtin: writes only where it's redirected, which `parse` sees")),
    ("printf", Rule("-v sets a variable: only a script's own, and no `[` or substitution in it", &["printf -v PATH /tmp", "printf -v 'a[$(touch x)]' 1"])),
    ("true", Builtin("does nothing")),
    ("false", Builtin("does nothing")),
    ("pwd", Builtin("a builtin that only prints")),
    ("stat", NoWriter(NO_WRITER)),
    ("file", Rule("-C writes a .mgc in the folder it runs in; -m only reads a magic file", &["file -C -m m", "file --comp -m m", "file -Cm m"])),
    ("readlink", NoWriter(NO_WRITER)),
    ("realpath", NoWriter(NO_WRITER)),
    ("basename", NoWriter(NO_WRITER)),
    ("dirname", NoWriter(NO_WRITER)),
    ("which", NoWriter(NO_WRITER)),
    ("whereis", NoWriter(NO_WRITER)),
    ("type", Builtin("a builtin that only prints")),
    ("getent", Rule("-s names a service, a library glibc loads and runs: `-s x/../evil` opens libnss_x/../evil.so.2", &["getent -s x/../evil passwd root", "getent -sx/../evil passwd root", "getent --service=x/../evil passwd root", "getent --serv=x/../evil passwd root"])),
    ("wc", NoWriter(NO_WRITER)),
    ("cut", NoWriter(NO_WRITER)),
    ("tr", NoWriter(NO_WRITER)),
    ("column", NoWriter(NO_WRITER)),
    ("grep", NoWriter(NO_WRITER)),
    ("egrep", NoWriter(NO_WRITER)),
    ("fgrep", NoWriter(NO_WRITER)),
    ("zgrep", NoWriter("no writing option: a script that quotes every word it evals (ten injections tried, none ran); its -f copy is a temp file it removes")),
    ("rg", Rule("an allowlist of long options: --pre and --hostname-bin run a program; --search-zip runs gzip from PATH and only reads", &["rg --pre /tmp/x hello f", "rg --pre=/tmp/x hello f", "rg --hostname-bin /tmp/x hello f", "rg --hostname-bin=/tmp/x hello f", "rg $(echo --pre=/tmp/x) hello f"])),
    ("jq", NoWriter("no writing option: a jq program can't write or run anything, so -f only reads one")),
    ("sha256sum", NoWriter(NO_WRITER)),
    ("sha1sum", NoWriter(NO_WRITER)),
    ("md5sum", NoWriter(NO_WRITER)),
    ("cksum", NoWriter(NO_WRITER)),
    ("xxd", Rule("a second operand is a file it writes, `-` first or not", &["xxd f /tmp/x", "xxd - /tmp/x", "xxd -r - /tmp/x"])),
    ("hexdump", NoWriter(NO_WRITER)),
    ("od", NoWriter(NO_WRITER)),
    ("strings", NoWriter("no writing option; @file only reads options")),
    ("diff", NoWriter("no writing option: --output is sdiff's, and diff 3.10 refuses it")),
    ("cmp", NoWriter(NO_WRITER)),
    ("comm", NoWriter(NO_WRITER)),
    ("locale", NoWriter(NO_WRITER)),
    ("lsb_release", NoWriter(NO_WRITER)),
    ("nvcc", Rule("only --version: a compiler (not on staging)", &["nvcc x.cu"])),
    ("sensors", Rule("-s sets limits (root); -c only reads a config", &["sensors -s", "sensors --set"])),
    ("vmstat", NoWriter(NO_WRITER)),
    ("iostat", NoWriter(NO_WRITER)),
    ("mpstat", NoWriter(NO_WRITER)),
    ("pidstat", Rule("-e runs a program", &["pidstat -e touch /tmp/x", "pidstat -u -e touch /tmp/x", "pidstat 1 1 -e touch /tmp/x"])),
    ("sar", Rule("-o saves to a file", &["sar -o /tmp/x 1 1", "sar -uo /tmp/x 1 1", "sar -o/tmp/x 1 1"])),
    ("ibstat", NoWriter(NO_WRITER)),
    ("ibstatus", NoWriter(NO_WRITER)),
    ("ibv_devinfo", NoWriter(NO_WRITER)),
    ("ibv_devices", NoWriter(NO_WRITER)),
    ("dig", NoWriter("no writing option: -f only reads a list of lookups (strace: nothing opened to write)")),
    ("nslookup", NoWriter("no writing option: on stdin ls, view and finger are \"not implemented\", and `!` is no shell")),
    ("host", NoWriter(NO_WRITER)),
    ("ping", NoWriter("no writing option: it sends packets, as C-J does")),
    ("traceroute", NoWriter("no writing option: -M says \"builtin or external\", but this build knows only its own (\"Unknown traceroute module\", and strace shows no library loaded)")),
    ("tracepath", NoWriter(NO_WRITER)),
    ("findmnt", NoWriter(NO_WRITER)),
    ("blkid", Rule("an allowlist: -c only /dev/null (a cache is written back, by the cluster user too). -g is meant to rewrite one and -w is accepted and ignored: neither wrote here, and neither is a look", &["blkid -c /tmp/x", "blkid -c /tmp/x /tmp/img", "blkid -c/tmp/x", "blkid -dc /tmp/x", "blkid --cache-file=/tmp/x", "blkid --cache=/tmp/x", "blkid --c=/tmp/x", "blkid -g", "blkid --garbage-collect", "blkid -w /tmp/x"])),
    ("test", Rule("-v, and a `[` or substitution in an operand, run in bash", &["test -v 'a[$(touch x)]'"])),
    ("[", Rule("as test", &["[ -v 'a[$(touch x)]' ]"])),
    ("[[", Rule("never a read: its -eq and -v evaluate a variable's value (NEVER_READ)", &["[[ y -eq 0 ]]"])),
    ("]]", Builtin("the end of `[[`, which never reads")),
    ("]", Builtin("the end of `[`")),
    ("sleep", NoWriter(NO_WRITER)),
    ("wait", Builtin("a builtin")),
    ("read", Rule("sets only a script's own variable", &["read PATH"])),
    ("seq", NoWriter(NO_WRITER)),
    ("cd", Builtin("moves the shell, which ends with the command")),
    ("exit", Builtin("ends the shell")),
    ("apt-cache", Rule("its looking verbs, no -c, -o, -p or -s: any verb writes the cache they name, and Dir::Bin::dpkg is a program it runs", &["apt-cache gencaches", "apt-cache -p /tmp/x stats", "apt-cache -p/tmp/x stats", "apt-cache --pkg-cache=/tmp/x stats", "apt-cache -s /tmp/x stats", "apt-cache --src-cache=/tmp/x stats", "apt-cache -c /tmp/c policy", "apt-cache -o Dir::Cache::pkgcache=/tmp/x policy", "apt-cache -oDir::Bin::dpkg=/tmp/x policy", "apt-cache --option Dir::Bin::dpkg=/tmp/x policy"])),
    ("dpkg-query", NoWriter("no writing option; its pager needs a terminal and DPKG_PAGER, an assignment that's no read")),
    ("getconf", NoWriter(NO_WRITER)),
    ("ulimit", Rule("no `[` or substitution in an operand; a limit lasts as long as the shell", &["ulimit 'a[$(touch x)]'"])),
    ("dmidecode", Rule("--dump-bin writes a file (no SMBIOS on staging to prove it; its manual says so)", &["dmidecode --dump-bin /tmp/x", "dmidecode --dump-b=/tmp/x"])),
    ("nl", NoWriter(NO_WRITER)),
    ("fold", NoWriter(NO_WRITER)),
    ("paste", NoWriter(NO_WRITER)),
    ("join", NoWriter(NO_WRITER)),
    ("expand", NoWriter(NO_WRITER)),
    ("tac", NoWriter(NO_WRITER)),
    ("rev", NoWriter(NO_WRITER)),
    ("zcat", NoWriter("no writing option: gzip -c, and nothing turns -c off")),
    ("bzcat", NoWriter("no writing option: bzip2 -c, and nothing turns -c off")),
    ("xzcat", NoWriter("no writing option: xz -c, and nothing turns -c off")),
    ("numfmt", NoWriter(NO_WRITER)),
    ("iconv", Rule("-o writes a file", &["iconv -o /tmp/x f", "iconv -o/tmp/x f", "iconv --output=/tmp/x f", "iconv --out=/tmp/x f", "iconv -co /tmp/x f"])),
    ("base64", NoWriter(NO_WRITER)),
    ("lslogins", NoWriter(NO_WRITER)),
    ("lsipc", NoWriter(NO_WRITER)),
    ("tree", Rule("an allowlist: -o writes a file, and -R with -H writes 00Tree.html into every folder", &["tree -o /tmp/x .", "tree -ox .", "tree -R -L 1 -H . d", "tree -RL 1 -H . d", "tree -LR 1 -H . d"])),
    ("printenv", NoWriter(NO_WRITER)),
    ("arch", NoWriter(NO_WRITER)),
    ("nvidia-ctk", Rule("only --version and `cdi list`: it writes specs and runtime config (not on staging)", &["nvidia-ctk cdi generate", "nvidia-ctk runtime configure"])),
    ("apt-config", Rule("dump and shell, no -c or -o: Dir::Bin::dpkg is a program it runs as it starts", &["apt-config -o Dir::Bin::dpkg=/tmp/x dump", "apt-config -oDir::Bin::dpkg=/tmp/x dump", "apt-config --option Dir::Bin::dpkg=/tmp/x shell X Y", "apt-config -c /tmp/c dump"])),
    ("modinfo", NoWriter(NO_WRITER)),
];

/// Every READ_ANY tool has its row, and nothing has a row that isn't in READ_ANY. A row with a rule
/// lists what the rule stops: each wrote or ran on the real tool, and none may be a read.
#[test]
fn read_tier_every_tool_is_accounted_for() {
    use super::classify::{read_only, verdict, Kind, READ_ANY};
    let table: Vec<&str> = READ_TIER.iter().map(|(t, _)| *t).collect();
    let missing: Vec<&&str> = READ_ANY.iter().filter(|t| !table.contains(t)).collect();
    assert!(missing.is_empty(), "READ_ANY tools with no row in READ_TIER: {missing:?}. Walk its man page and the real binary, then add its row");
    let stale: Vec<&&str> = table.iter().filter(|t| !READ_ANY.contains(t)).collect();
    assert!(stale.is_empty(), "rows for tools not in READ_ANY: {stale:?}");
    assert_eq!(table.iter().collect::<std::collections::HashSet<_>>().len(), table.len(), "a tool has two rows");
    for (tool, how) in READ_TIER {
        match how {
            NoWriter(why) | Builtin(why) => assert!(!why.is_empty(), "{tool}"),
            Rule(rule, writers) => {
                assert!(!rule.is_empty() && !writers.is_empty(), "{tool}: a rule needs what it stops");
                for cmd in *writers {
                    assert!(verdict(cmd).kind != Kind::Read && !read_only(cmd), "{tool}: `{cmd}` is a read");
                }
            }
        }
    }
}

/// A rule reads words, so a tool with one isn't a read with a word the shell builds; a tool with
/// no writer stays one. The table and the classifier's own list must agree.
#[test]
fn a_tool_with_a_rule_takes_no_built_word() {
    use super::classify::{read_only, OPTIONS_MATTER};
    let ruled: Vec<&str> = READ_TIER.iter().filter(|(_, how)| matches!(how, Rule(..))).map(|(t, _)| *t).collect();
    let missing: Vec<&&str> = ruled.iter().filter(|t| !OPTIONS_MATTER.contains(t)).collect();
    assert!(missing.is_empty(), "Rule rows OPTIONS_MATTER doesn't name: {missing:?}");
    let extra: Vec<&&str> = OPTIONS_MATTER.iter().filter(|t| !ruled.contains(t)).collect();
    assert!(extra.is_empty(), "OPTIONS_MATTER names tools with no Rule row: {extra:?}");
    for (tool, how) in READ_TIER {
        let cmd = |word: &str| if *tool == "[" { format!("[ {word} ]") } else { format!("{tool} {word}") };
        for word in ["$(echo x)", "`echo x`", "\"$(echo x)\"", "$x", "\"$x\"", "*"] {
            let command = cmd(word);
            match how {
                Rule(..) if matches!(*tool, "[" | "test") && word == "\"$x\"" => assert!(read_only(&command), "`{command}`: one word, and test reads it as text"),
                Rule(..) => assert!(!read_only(&command), "`{command}` is a read"),
                _ if matches!(*tool, "]" | "]]") => {}
                _ => assert!(read_only(&command), "`{command}` isn't a read"),
            }
        }
    }
}

/// Each writes or runs something where the shell puts its option in: on real find, sort and curl
/// (`staging_read_tier_options`, and a shell through the exit in `a_built_option_is_no_read_at_the_exit`).
/// Every option a rule bars is one a variable, a substitution or a glob can bring.
#[test]
fn a_word_the_shell_builds_can_be_the_option_a_rule_bars() {
    use super::classify::{read_only, verdict, Kind};
    let tools = [
        "find . {}", "sort {} y", "curl {} http://x", "curl -w {} http://x", "journalctl {}", "dmesg {}", "sysctl {}", "iconv {} y", "rg {} foo", "tree {}", "sar {} 1 1", "file {}", "blkid {}", "lspci {}", "smartctl {} /dev/x",
        "sed -n {} y", "awk {} y", "tar -tf y {}", "tar {} -tf y", "git log {}", "docker ps {}", "docker {} ps", "systemctl {}", "nmcli {}", "ip {}", "ip link {}", "nvidia-smi {}", "apt-cache {}", "less {}", "date {}",
    ];
    for tool in tools {
        for word in ["$(echo -delete)", "\"$(echo -delete)\"", "`echo -delete`", "\"`echo -delete`\"", "$x", "\"$x\"", "\"-$x\"", "\"${x}\"", "$1", "\"$@\"", "*", "-*", "?x", "[a]*", "[-]delete", "a$x", "\"a$x\"", "/tmp/$(echo a -delete)", "\"/tmp/$x\""] {
            for prefix in ["", "for x in -delete; do ", "LANG=-delete; ", "sudo ", "timeout 5 ", "nice ", "env "] {
                let cmd = format!("{prefix}{}", tool.replace("{}", word));
                let cmd = if prefix.starts_with("for") { format!("{cmd}; done") } else { cmd };
                assert!(verdict(&cmd).kind != Kind::Read && !read_only(&cmd), "`{cmd}` is a read");
            }
        }
    }
    // A wrapper's own word, a `-c` script's, and what a script is given as $1.
    for cmd in ["timeout $x cat y", "nice -n $x cat y", "sh -c \"cat $x\"", "bash -c \"cat /tmp/$x\"", "sh -c 'find . $1' x -delete", "sh -c \"$x\"", "echo -delete | { read x; find . $x; }", "echo --pre=/tmp/p | { read x; rg \"$x\" foo; }", "read x </tmp/o; sort $x y", "printf $x", "printf \"$x\" y", "printf -- \"$x\"", "[ $x ]", "[ \"$x\" \"$y\" ]", "[ ! \"$x\" ]", "[ \"$x\" \"$y\" \"$z\" ]", "test $x", "ethtool $x", "ethtool \"$x\" eth0", "ethtool -w \"$x\" data f", "nmcli -f $x connection show", "nmcli -f \"$x\" connection show", "${HOME:+-delete}", "find . ${HOME:+-delete}", "find . \"${HOME:+-delete}\"", "find . $USER$x", "find . \"$HOMEx\""] {
        assert!(verdict(cmd).kind != Kind::Read && !read_only(cmd), "`{cmd}` is a read");
    }
    // What the rules still read: text the shell doesn't build, tools with no rule, and the words
    // the read tier can't set.
    for cmd in [
        "sort '$x' y",
        "sort \\$x y",
        "awk '{print $1}' y",
        "sed -n '$p' y",
        "grep -E '^a$' y",
        "curl -s -m 3 -w '%{http_code}\\n' http://x",
        "docker ps --format '{{.Names}}'",
        "journalctl -u x --since '-1h' -o json",
        "cat $x",
        "cat \"$x\"",
        "ls /tmp/*",
        "ls -l $(ls /tmp)",
        "grep foo /etc/*.conf",
        "echo \"$(cat /etc/hostname) $x\"",
        "for z in /sys/class/thermal/thermal_zone*; do echo \"$(cat $z/type 2>/dev/null)\"; done",
        "for i in $(ls); do cat \"$i\"; done",
        "sudo cat $x",
        "timeout 5 cat \"$x\"",
        "printf %s \"$HOME\"",
        "printf '%s\\n' \"$x\"",
        "getent passwd \"$USER\"",
        "find \"$HOME\" -name x",
        "find ${HOME} -name x",
        "sed -n p \"$HOME/x\"",
        "ethtool \"$i\"",
        "nmcli -g GENERAL.CONNECTION connection show \"$x\"",
        "[ \"$x\" ]",
        "[ -z \"$x\" ]",
        "[ -e \"$i/wireless\" ]",
        "[ \"$x\" = y ]",
        "test -n \"$x\"",
    ] {
        assert_eq!(verdict(cmd).kind, Kind::Read, "`{cmd}`");
        assert!(read_only(cmd), "`{cmd}`");
    }
}

/// What the review found outside READ_ANY, in the per-tool rules: each wrote, ran or changed
/// something on the real tool (`ip`, `tc`, `sysctl`, `apt-mark`) or on real systemd 255, and each was
/// a read. A value-taking option put a word where the rule looked for the verb, a start of a verb
/// passed for "anything else", or a looking word let the whole line through.
#[test]
fn the_per_tool_rules_read_what_the_tool_reads() {
    use super::classify::{read_only, verdict, Kind};
    for cmd in [
        // ip takes the start of a verb: this one flushed the box's routing rules.
        "ip rule f",
        "ip rule fl",
        "ip ru flus",
        "ip rule res",
        "ip tcpm f",
        "ip link -s set eth0 down",
        "ip xfrm state",
        // tc and rdma run a batch file, whatever follows.
        "tc -batch /tmp/b show",
        "tc -b /tmp/b ls",
        "tc -force -batch /tmp/b show",
        "tc -ne qdisc show dev lo",
        "tc qdisc add dev lo root netem delay 1ms show",
        "tc exec bpf show",
        "rdma -b /tmp/b show",
        "rdma -batch /tmp/b list",
        "rdma dev set mlx5_0 name show",
        "rdma link add rxe0 type rxe netdev list",
        // systemctl: -p and --state take the next word, so `stop` is the verb.
        "systemctl -p status stop x",
        "systemctl -P status stop x",
        "systemctl --state status stop x",
        "systemctl --property status stop x",
        "systemctl -p status restart x",
        "systemctl --type status disable x",
        "systemctl --user -p status stop x",
        // An option SparkBox doesn't know might take a value, or go elsewhere.
        "systemctl -H status status",
        "systemctl -M status status",
        "systemctl --host status status",
        "systemctl --root=/tmp status",
        // The family.
        "timedatectl -p status set-ntp false",
        "loginctl -p status terminate-user sparkuser",
        "loginctl --property status kill-session 1",
        "hostnamectl -H status hostname x",
        "hostnamectl -M status hostname x",
        "busctl --match status call a /b c d",
        "busctl --timeout status call a /b c d",
        "busctl --address status call a /b c d",
        "networkctl -n status reload",
        "localectl -H status set-locale x",
        "resolvectl -H status flush-caches",
        // apt-mark -c takes a value.
        "apt-mark -c showhold hold jq",
        "apt-mark --config-file showhold hold jq",
        "apt-mark -o Foo=1 showhold",
        "apt-mark --file=/tmp/x showhold",
        // sysctl -f is -p.
        "sysctl -f /tmp/s",
        "sysctl -fp /tmp/s",
        "sysctl -pf /tmp/s",
        // ethtool writes a dump, and sets what the old list didn't name.
        "ethtool -w eth0 data /tmp/x",
        "ethtool --get-dump eth0 data /tmp/x",
        "ethtool --set-phy-tunable eth0 downshift on",
        "ethtool --set-tunable eth0 rx-copybreak 1",
        "ethtool --cable-test eth0",
        "ethtool --set-module eth0 power-mode-policy high",
        "ethtool -Q eth0 queue_mask 1",
        "ethtool --per-queue eth0 queue_mask 1",
        "ethtool --set-mm eth0 tx-enabled on",
        "ethtool --set-pse eth0 podl-pse-admin-control enable",
        "ethtool --set-plca-cfg eth0 enable on",
        "ethtool --rxfh eth0 equal 1",
        "ethtool -p eth0",
        "ethtool -t eth0",
        // smartctl sets through -l (ATA only).
        "smartctl -l scterc,70,70 /dev/sda",
        "smartctl -lscterc,70,70 /dev/sda",
        "smartctl -l scttempint,10 /dev/sda",
        "smartctl --log=scterc,reset /dev/sda",
        "smartctl --lo=scttempint,5 /dev/sda",
        "smartctl --log scterc,70,70 /dev/sda",
        // The mlx tools: having -q in it doesn't make a line a read.
        "mlxlink -d /dev/mst/x -q --port_state TG",
        "mlxlink -d /dev/mst/x -q --pcie_reset",
        "mlxlink -d /dev/mst/x -q --fec RS",
        "mlxconfig -d /dev/mst/x -y set X=1 query",
        "mlxconfig -d /dev/mst/x reset query",
        "mlxfwmanager -u -q",
        "mlxfwmanager --query -i /tmp/fw.bin -y",
        // docker compose config writes where -o says (Docker's compose, by hand); buildx boots its builder.
        "docker compose config -o /tmp/x",
        "docker compose config --output=/tmp/x",
        "docker compose config --out /tmp/x",
        "docker compose config -qo /tmp/x",
        "docker compose -f /tmp/c.yml config -o /tmp/x",
        "docker buildx inspect --bootstrap",
        // sudo -e edits, the way -s starts a shell.
        "sudo -e cat",
        "sudo -ue root cat",
        // nvidia-smi isn't on staging: what isn't known only to show isn't a read.
        "nvidia-smi mig -lgi",
        "nvidia-smi conf-compute -srs 1",
        "nvidia-smi vgpu -q",
        "nvidia-smi -gtt 80",
        "nvidia-smi -vm 1",
        // xargs fed a `-C` makes `file` write.
        "xargs file",
        // git before its verb: --namespace takes the next word, so each of these really ran. Any
        // other option before the verb isn't a read either.
        "git --namespace log commit -qam two",
        "git --namespace status reset --hard",
        "git --namespace log clean -fd",
        "git --namespace log push",
        "git --namespace log config user.name x",
        "git --namespace=x log",
        "git --namespace=x status",
        "git --super-prefix log commit -qm x",
        "git --attr-source log commit -qm x",
        "git --list-cmds log commit -qm x",
        "git --bare log",
        "git -p log",
        "git --paginate log",
        "git -- log",
        "git -x log",
        "git -Pc core.pager=x log",
        // docker compose before its verb: each of these removed or reached the real verb.
        "docker compose --workdir ps rm -f",
        "docker compose --workdir ps down",
        "docker compose --workdir=/tmp rm -f",
        "docker compose --ansi ps rm -f",
        "docker compose --ansi=never rm -f",
        "docker compose --foo ps rm -f",
        "docker compose -x ps rm -f",
        // buildx, and the management verbs: the word after an option is no verb.
        "docker buildx --builder ls rm foo",
        "docker buildx --builder=ls rm foo",
        "docker buildx -x ls rm foo",
        "docker buildx --boot inspect",
        "docker image -x ls rm foo",
        "docker system -x df prune",
        // docker's own options: --config runs what's in its cli-plugins, -H and --context reach
        // somewhere else.
        "docker --config /tmp/evil info",
        "docker --config=/tmp/evil info",
        "docker --config /tmp/evil compose ps",
        "docker --config /tmp/evil ps",
        "docker -H ssh://x ps",
        "docker -Hssh://x ps",
        "docker -H=ssh://x ps",
        "docker --host ssh://x ps",
        "docker --host=ssh://x ps",
        "docker -H tcp://x:2375 ps",
        "docker -H unix:///tmp/x.sock ps",
        "docker --context x ps",
        "docker --context=x ps",
        "docker -c x ps",
        "docker -cx ps",
        "docker --foo ps",
        // A listing word doesn't pass the rest: sfdisk's last action wins, and parted runs each
        // word after the device, shortened or not.
        "sfdisk -l --delete /dev/nvme0n1",
        "sfdisk -l --delete disk.img 1",
        "sfdisk -d --delete /dev/nvme0n1 1",
        "sfdisk -d --part-label disk.img 1 X",
        "sfdisk -J --delete disk.img 1",
        "sfdisk -s --delete disk.img",
        "sfdisk -V --part-type disk.img 1 83",
        "sfdisk --list --del disk.img",
        "sfdisk -l --no-act disk.img",
        "sfdisk -lq disk.img",
        "sfdisk --dump --reorder disk.img",
        "sfdisk disk.img",
        "parted -s /dev/nvme0n1 print mkl gpt",
        "parted -s disk.img print mkl msdos",
        "parted -s disk.img print mkt msdos",
        "parted -s disk.img print mkp primary 1 2",
        "parted -s disk.img print r 1",
        "parted -s disk.img print rm 1",
        "parted -s disk.img print res 1 2",
        "parted -s disk.img print se 1 boot on",
        "parted -s disk.img print t 1 boot",
        "parted -s disk.img print na 1 x",
        "parted -s disk.img mkl gpt print",
        "parted -s disk.img print free mkl gpt",
        "parted -s disk.img unit s print",
        "parted -s disk.img print 1",
        "parted -a opt disk.img print",
        "parted -l disk.img mkl gpt",
        "parted --list disk.img mkl gpt",
        "parted disk.img",
        "sgdisk -p -Z disk.img",
        // ip takes the start of an option's name, and `--` for `-`: each of these took a link down
        // or deleted one, or would.
        "ip -fa inet l set dummy0 down",
        "ip --fa inet l set dummy0 down",
        "ip -fam inet l set dummy0 down",
        "ip -lo 1 l set dummy0 down",
        "ip -rcv 100000 l del dummy0",
        "ip -rc 100000 l del dummy0",
        "ip -ne foo l del d1",
        "ip -net foo l del d1",
        "ip --net foo l del d1",
        "ip -statis l set dummy0 down",
        "ip -e l set dummy0 down",
        "ip -foo l",
        "ip -batch /tmp/cmds",
        "ip -force -batch /tmp/cmds",
        "ip -force link set dummy0 down",
        "ip -- l set dummy0 down",
        "ip - inet l del dummy0",
        // ...and of a verb, on an object the rule has no reason to think it knows.
        "ip rule f",
        "ip neigh f",
        "ip ntable s",
        "ip tunnel s",
        "ip stats s",
        // A sub-object is no verb: s is state and f flush, l limits.
        "ip xfrm s flush",
        "ip x s f",
        "ip xf p f",
        "ip xfrm state flush",
        "ip xfrm state",
        "ip mptcp l set subflows 3",
        "ip mp l set subflows 3",
        "ip mptcp endpoint add 10.0.0.1",
        "ip sr tunsrc set ::1",
        "ip sr t s ::1",
        "ip s t s ::1",
        "ip ioam namespace add 1",
        "ip io n s 1",
        "ip xfrm",
        // nmcli takes the start of an option's name too, so the option ate `radio`.
        "nmcli -fi radio connection down eth0",
        "nmcli --fi radio connection delete x",
        "nmcli -fie radio connection delete x",
        "nmcli -ge radio device disconnect eth0",
        "nmcli -get radio connection delete x",
        "nmcli -mo radio connection delete x",
        "nmcli -co radio connection delete x",
        "nmcli -es radio connection delete x",
        "nmcli -wa radio connection delete x",
        "nmcli -foo radio connection delete x",
        // A reader's option off the list, even where nothing moves: -s puts secrets on the screen.
        "nmcli -s connection show x",
        "nmcli --show-secrets connection show x",
        "nmcli -a connection show x",
        "nmcli -- radio connection delete x",
        // Radio: one more word at most, and not a verb.
        "nmcli radio connection delete x",
        "nmcli radio wifi on",
        "nmcli radio wifi off",
        "nmcli radio wifi of",
        "nmcli r w OFF",
        "nmcli radio all off",
        "nmcli radio foo",
        // apt changelog -d writes <package>.changelog here (apt 2.8.3, as the cluster user).
        "apt-get changelog -d jq",
        "apt changelog -d jq",
        "apt changelog --download-only jq",
        "apt-get changelog --download-only jq",
        "apt-get -d changelog jq",
        "apt -d changelog jq",
        "apt-get -qd changelog jq",
        "apt-get changelog -dq jq",
        "apt changelog --foo jq",
        // curl takes the format from a file or stdin, so no word holds %output.
        "curl -w @- http://x",
        "curl -w @/tmp/fmt http://x",
        "curl -w@- http://x",
        "curl -w@/tmp/fmt http://x",
        "curl -sw@/tmp/fmt http://x",
        "curl -sSw @- http://x",
        "curl -4w@/tmp/fmt http://x",
        "curl -#w@- http://x",
        "curl --write-out @- http://x",
        "curl --write-out @/tmp/fmt http://x",
        "curl --write-out=@/tmp/fmt http://x",
        "curl --write-out=@- http://x",
        "printf '%%output{/tmp/x}y' | curl -s -o /dev/null -w @- file:///etc/hostname",
        // A cluster that starts with a digit or # is a cluster all the same.
        "curl -4o/tmp/x http://x",
        "curl -1o /tmp/x http://x",
        "curl -#o/tmp/x http://x",
        "curl -0D/tmp/x http://x",
        "curl -:o/tmp/x http://x",
    ] {
        assert!(verdict(cmd).kind != Kind::Read && !read_only(cmd), "`{cmd}` is a read");
    }
}

/// A root run of the commands above is worse than a change: what the rules used to call a look
/// wipes the partition table or takes the node off the network, however the option is shortened.
#[test]
fn what_the_read_rules_let_through_is_as_severe_as_it_is() {
    use super::classify::{verdict, Kind};
    for cmd in ["sfdisk -l --delete /dev/nvme0n1", "sudo sfdisk -d --delete /dev/nvme0n1 1", "parted -s /dev/nvme0n1 print mkl gpt", "sudo parted -s /dev/sda print mkp primary 1 2"] {
        assert_eq!(verdict(cmd).kind, Kind::Destructive, "`{cmd}`");
    }
    // The listings of the same disks aren't.
    for cmd in ["sfdisk -l /dev/nvme0n1", "sfdisk --dump /dev/nvme0n1", "parted -s /dev/nvme0n1 print", "fdisk -l /dev/nvme0n1", "sgdisk -p /dev/nvme0n1"] {
        assert_eq!(verdict(cmd).kind, Kind::Read, "`{cmd}`");
    }
    for cmd in ["sudo ip -fa inet l set eth0 down", "ip --fa inet l del d1", "ip -lo 1 link set eth0 down", "ip -ne foo link del d1", "ip -rcv 1000 a flush dev eth0", "ip -r link del d1", "nmcli -fi radio connection down eth0", "nmcli --fi radio connection delete x", "nmcli -ge radio device disconnect eth0", "sudo nmcli -wa 5 device disconnect eth0"] {
        let v = verdict(cmd);
        assert!(v.kind == Kind::Destructive && v.cuts_access, "`{cmd}`: {v:?}");
    }
    for cmd in ["ip link show", "ip -br a", "nmcli -f NAME connection show", "nmcli --fields=NAME connection show"] {
        assert!(!verdict(cmd).cuts_access, "`{cmd}`");
    }
    // --workdir takes a word, so compose's `up` behind it still runs as root.
    assert!(verdict("docker compose --workdir ps up -d").root);
}

/// The same tools only looking: what SparkBox sends, and what the review's controls did on the
/// real tools, stay reads.
#[test]
fn the_read_tier_keeps_what_only_looks() {
    use super::classify::{read_only, verdict, Kind};
    for cmd in [
        "blkid",
        "blkid /dev/nvme0n1p1",
        "blkid -c /dev/null",
        "blkid -c /dev/null /dev/nvme0n1p1",
        "blkid -c/dev/null /dev/nvme0n1p1",
        "blkid --cache-file=/dev/null /dev/nvme0n1p1",
        "blkid --cache-file /dev/null -o value -s UUID /dev/nvme0n1p1",
        "blkid -o export /dev/nvme0n1p1",
        "blkid -p /dev/nvme0n1p1",
        "blkid -L root",
        "apt-cache policy",
        "apt-cache policy docker-ce",
        "apt-cache show docker-ce",
        "apt-cache search vllm",
        "apt-cache depends docker-ce",
        "apt-cache rdepends --installed docker-ce",
        "apt-cache madison docker-ce",
        "apt-cache pkgnames",
        "apt-cache stats",
        "apt-cache -q policy",
        "apt-cache --version",
        "apt-config dump",
        "apt-config dump APT::Periodic",
        "apt-config dump APT::Periodic::Unattended-Upgrade",
        "apt-config shell X Y",
        "apt-mark showhold",
        "apt-mark showauto",
        "apt-mark showmanual",
        "rg hello f",
        "rg -z hello f.gz",
        "rg --search-zip hello f.gz",
        "rg --pre-glob '*.txt' hello f",
        "rg -n --hidden --glob '!x' hello .",
        "rg --no-pre hello f",
        "rg --json hello f",
        "getent passwd root",
        "getent hosts cell2",
        "getent ahosts cell2",
        "pidstat",
        "pidstat -u 1 1",
        "pidstat -G python",
        "tree",
        "tree -L 2 /tmp",
        "tree -L2 /tmp",
        "tree -a -I node_modules .",
        "tree -H . d",
        "tree --du -h .",
        "tree --dirsfirst .",
        "lspci",
        "lspci -v",
        "lspci -vv",
        "lspci -nn",
        "lspci -k -s 00:01.0",
        "lspci -d 15b3:",
        "lspci -tv",
        "sysctl -a",
        "sysctl kernel.hostname",
        "sysctl -n vm.swappiness",
        "ip -br addr",
        "ip -br link",
        "ip -s link",
        "ip a",
        "ip a s",
        "ip addr show enp1s0f1np1",
        "ip link show",
        "ip l sh",
        "ip route",
        "ip r",
        "ip route show",
        "ip r l",
        "ip route ls",
        "ip route get 1.1.1.1",
        "ip r g 1.1.1.1",
        "ip neigh",
        "ip rule",
        "ip rule list",
        "ip rule show",
        "ip netns ls",
        "ip monitor",
        "tc qdisc show",
        "tc -s qdisc ls",
        "tc -j qdisc show dev eth0",
        "tc class list dev eth0",
        "tc filter show dev eth0",
        "rdma",
        "rdma dev",
        "rdma link",
        "rdma link show",
        "rdma -j dev show",
        "rdma resource show qp",
        "systemctl",
        "systemctl --failed",
        "systemctl --failed --no-legend --plain",
        "systemctl status docker",
        "systemctl is-active ssh",
        "systemctl is-enabled ssh",
        "systemctl is-active sparkbox-agent",
        "systemctl list-units --failed",
        "systemctl list-units --type=service --all --no-pager --output=json",
        "systemctl list-unit-files --type=service --no-pager --output=json",
        "systemctl list-timers --all",
        "systemctl show -p ActiveState --value ollama.service",
        "systemctl show -p MainPID --value -- sparkbox-upgrade-x",
        "systemctl show -p A,B --value x",
        "systemctl cat ssh",
        "systemctl get-default",
        "systemctl --user status x",
        "hostnamectl",
        "hostnamectl status",
        "hostnamectl --static",
        "hostnamectl hostname",
        "timedatectl",
        "timedatectl status",
        "timedatectl show -p NTP",
        "timedatectl show -p NTPSynchronized --value",
        "timedatectl show-timesync --all",
        "timedatectl timesync-status",
        "loginctl",
        "loginctl list-sessions",
        "loginctl show-session 1 -p Id",
        "networkctl",
        "networkctl list",
        "networkctl status enP7s7",
        "resolvectl",
        "resolvectl status",
        "resolvectl dns",
        "resolvectl query cell2 -t A",
        "busctl list",
        "busctl status",
        "localectl",
        "ethtool eth0",
        "ethtool -i eth0",
        "ethtool -S eth0",
        "ethtool --statistics eth0",
        "ethtool -k eth0",
        "ethtool -m eth0",
        "ethtool --json -S eth0",
        "nmcli",
        "nmcli radio",
        "nmcli radio wifi",
        "nmcli r all",
        "nmcli d s",
        "nmcli general",
        "smartctl -a /dev/nvme0",
        "smartctl -l error /dev/sda",
        "smartctl -l scterc /dev/sda",
        "smartctl --log=xerror /dev/sda",
        "mlxlink -d /dev/mst/x -q",
        "mlxlink -d /dev/mst/x --query --json",
        "mlxconfig -d /dev/mst/x query",
        "mlxconfig -d /dev/mst/x query LINK_TYPE_P1",
        "mlxfwmanager --query",
        "mlxfwmanager -d /dev/mst/x --query",
        "docker compose config",
        "docker compose -f /tmp/c.yml config --services",
        "docker compose config -q",
        "docker compose ps",
        "docker buildx inspect",
        "docker buildx ls",
        "nvidia-smi",
        "nvidia-smi -L",
        "nvidia-smi -q",
        "nvidia-smi -q -d CLOCK",
        "nvidia-smi -i 0 -q",
        "nvidia-smi --query-gpu=name --format=csv,noheader",
        "nvidia-smi dmon",
        "nvidia-smi pmon",
        "nvidia-smi topo -m",
        "file /tmp/x",
        "file -m /tmp/m /tmp/x",
        "file -z /tmp/x.gz",
        "diff /tmp/a /tmp/b",
        "diff -u /tmp/a /tmp/b",
        "lsof -i :8000",
        "lsof +D /tmp",
        "dig -f /tmp/n",
        "dig +short cell2",
        "jq -f /tmp/j /tmp/x",
        "jq .a /tmp/x",
        "nslookup cell2",
        "zgrep hello /tmp/x.gz",
        "xxd /tmp/x",
        "top -b -n 1",
        // git before its verb, spelled out.
        "git log -1",
        "git -P log",
        "git --no-pager log -5 --oneline",
        "git --no-optional-locks status",
        "git --no-pager --no-replace-objects diff",
        "git --version",
        "git --help",
        "git status --porcelain",
        // docker, compose and buildx with their own options before the verb.
        "docker ps -a --format '{{.Names}}'",
        "docker --debug ps",
        "docker -D info",
        "docker -l debug ps",
        "docker --log-level=info ps",
        "docker --version",
        "docker image ls",
        "docker image inspect x",
        "docker network ls",
        "docker system df",
        "docker compose -f /tmp/c.yml ps",
        "docker compose --file /tmp/c.yml --project-name x ps",
        "docker compose --project-name=x logs --tail 20 -f web",
        "docker compose -p x --env-file e --profile p --project-directory d ps",
        "docker compose --ansi never --progress plain ps",
        "docker compose --workdir /tmp ps",
        "docker compose --compatibility ps",
        "docker compose version",
        "docker buildx du",
        "docker buildx --builder x ls",
        "docker buildx --builder=x inspect",
        "docker buildx version",
        // The listings.
        "sfdisk -l /dev/nvme0n1",
        "sfdisk --list /dev/nvme0n1",
        "sfdisk -d /dev/nvme0n1",
        "sfdisk --dump /dev/nvme0n1",
        "sfdisk -J /dev/nvme0n1",
        "sfdisk --json /dev/nvme0n1",
        "sfdisk -s /dev/nvme0n1",
        "sfdisk -V /dev/nvme0n1",
        "fdisk -l",
        "fdisk -l /dev/nvme0n1",
        "fdisk --list -w always /dev/nvme0n1",
        "parted -l",
        "parted --list",
        "parted /dev/nvme0n1 print",
        "parted -s /dev/nvme0n1 print",
        "parted -m /dev/nvme0n1 print",
        "parted --script /dev/nvme0n1 print free",
        "parted -ms /dev/nvme0n1 print",
        "parted -s -m /dev/nvme0n1 print free",
        "sgdisk -p /dev/nvme0n1",
        "sgdisk --print /dev/nvme0n1",
        // ip's options spelled out, with what each takes, and the three objects it shortens.
        "ip -4 a",
        "ip -6 route",
        "ip -o link",
        "ip -oneline link",
        "ip -j -4 addr",
        "ip -json addr",
        "ip -d link show",
        "ip -details link",
        "ip -p -j link",
        "ip -c a",
        "ip -h -s link",
        "ip -iec -s link",
        "ip -br -4 addr",
        "ip -brief link",
        "ip -f inet a",
        "ip -family inet6 route",
        "ip -n x link",
        "ip -netns x addr",
        "ip -l 1 route",
        "ip -rc 100000 link",
        "ip -o -f inet addr show dev eth0",
        "ip -o route get 1.1.1.1",
        "ip -o route show default",
        "ip link l",
        "ip l",
        "ip route g 1.1.1.1",
        "ip route l",
        "ip neigh show",
        "ip nexthop list",
        "ip netns list",
        "ip tunnel show",
        "ip vrf show",
        "ip token list",
        "ip mon",
        "ip xfrm state show",
        "ip xfrm state list",
        "ip xfrm policy list",
        "ip mptcp endpoint show",
        "ip mptcp limits show",
        "ip sr tunsrc show",
        "ip ioam namespace show",
        // nmcli's options spelled out, the Connect wizard's read among them.
        "nmcli -g GENERAL.CONNECTION device show eth0",
        "nmcli -g 802-3-ethernet.wake-on-lan connection show x",
        "nmcli --get-values GENERAL.CONNECTION device show eth0",
        "nmcli -t -f NAME,DEVICE connection show",
        "nmcli --terse --fields=NAME connection show",
        "nmcli --mode tabular device status",
        "nmcli -c no general",
        "nmcli -w 5 device status",
        "nmcli radio all",
        "nmcli radio wwan",
        "nmcli r w",
        "nmcli r wi",
        "nmcli r a",
        "nmcli radio",
        "nmcli d",
        "nmcli g",
        // apt's changelog as a read.
        "apt-get changelog jq",
        "apt changelog jq",
        "apt-get changelog -q jq",
        "apt-get -q changelog jq",
        "apt-get changelog --print-uris jq",
        "apt changelog --print-uris jq",
        "apt-get -o DPkg::Lock::Timeout=300 changelog jq",
        // curl's format, inline.
        "curl -w '%{http_code}' http://x",
        "curl -sw '%{http_code}' http://x",
        "curl --write-out '%{http_code}' http://x",
        "curl --write-out='%{http_code}' http://x",
        "curl -4 http://x",
        "curl -4s http://x",
        "curl -#s http://x",
        "curl -4so /dev/null http://x",
        "curl -s -m 10 -H @- -w '\n%{http_code}' http://192.168.50.154:8000/v1/models",
    ] {
        assert_eq!(verdict(cmd).kind, Kind::Read, "`{cmd}`");
        assert!(read_only(cmd), "`{cmd}`");
    }
}

#[test]
fn expanded_test_operands_cannot_change_the_operators() {
    for cmd in ["test -e $x", "[ -n $x ]", "[ $x = y ]", "[ x = $y ]", "[ ! $x ]", "[ -e /tmp/* ]"] {
        assert_ne!(classify(cmd), Kind::Read, "{cmd}");
    }
    for cmd in [r#"[ -e "$x" ]"#, r#"[ "$x" = "$y" ]"#, r#"test -n "$x""#] {
        assert_eq!(classify(cmd), Kind::Read, "{cmd}");
    }
}
