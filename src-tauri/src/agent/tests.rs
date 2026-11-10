//! The agent's install, settings pushes and removal against a fake Spark (real shell, the
//! sudo and systemctl shims, files under a temporary root), the settings hash, who sends
//! which alert, signed requests (maintenance windows, acks) and their answers, retries that
//! back off onto one activity row, what a test page proves, backfill merging and the Demo
//! agents.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::delivery::{Channel, Kind};
use crate::fake_node::{connect, fake_node, real_shell, temp, Node};
use crate::health::rule::RuleSet;
use crate::health::{AlertEvent, EventKind, Severity};
use crate::model::AgentBeat;
use crate::notify::{Member, NotifySettings, Scope, Why};
use crate::tasks::{TaskSpec, Tasks};

fn layout_under(root: &std::path::Path) -> Layout {
    std::fs::create_dir_all(root.join("root")).unwrap();
    Layout {
        opt: root.join("opt/sparkbox-agent").display().to_string(),
        etc: root.join("etc/sparkbox-agent").display().to_string(),
        unit: root.join("etc/systemd/system/sparkbox-agent.service").display().to_string(),
        state: root.join("var/lib/sparkbox-agent").display().to_string(),
        scratch: root.join("root").display().to_string(),
    }
}

/// A systemctl that records what it's asked and plays the agent: `restart` starts it and
/// `reload` re-reads agent.json and rules.json, each writing the heartbeat and a status
/// whose configHash is computed over the files as installed, as the agent does.
fn systemctl_that_starts(node: &Node, layout: &Layout) {
    let script = format!(
        r#"#!/usr/bin/env python3
import hashlib, json, os, sys, time
state = os.environ["SB_FAKE_STATE"]
with open(os.path.join(state, "systemctl.log"), "a") as f:
    f.write(" ".join(sys.argv[1:]) + "\n")
units = os.path.join(state, "units"); os.makedirs(units, exist_ok=True)
unit = os.path.join(units, "sparkbox-agent")
etc, var = {etc}, {var}
def play():
    parts = []
    for name in ("agent.json", "rules.json", "notify.json"):
        try:
            parts.append(open(os.path.join(etc, name), "rb").read())
        except OSError:
            parts.append(b"")
    now = int(time.time() * 1000)
    os.makedirs(var, exist_ok=True)
    json.dump({{"node": "cell1", "version": {version}, "lastBeatMs": now, "historyFromMs": now, "firing": 0}}, open(os.path.join(var, "beat.json"), "w"))
    json.dump({{"node": "cell1", "version": {version}, "uptimeS": 2, "lastSampleMs": now, "lastCycleMs": now, "firing": [], "lastDelivery": None,
               "configHash": hashlib.sha256(b"\0".join(parts)).hexdigest(), "missing": [], "endpointsProbed": [], "outboxPending": 0}},
              open(os.path.join(var, "status.json"), "w"))
verb = sys.argv[1] if len(sys.argv) > 1 else ""
if verb == "is-active":
    print("active" if os.path.exists(unit) else "inactive")
    sys.exit(0 if os.path.exists(unit) else 3)
if verb in ("restart", "start"):
    open(unit, "w").close()
    play()
elif verb == "reload":
    if not os.path.exists(unit):
        print("sparkbox-agent.service is not active, cannot reload.", file=sys.stderr)
        sys.exit(1)
    play()
elif verb == "disable":
    if os.path.exists(unit):
        os.remove(unit)
"#,
        etc = py_str(&layout.etc),
        var = py_str(&layout.state),
        version = py_str(version()),
    );
    node.bin("systemctl", &script);
}

fn py_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// Root's `install -o root -g root` and `useradd` under the fake's sudo (which has no root):
/// the owner is noted, the rest really runs.
fn root_tools(node: &Node) {
    node.bin(
        "install",
        r#"#!/bin/sh
n=$#; i=0
while [ $i -lt $n ]; do
  a=$1; shift; i=$((i + 1))
  case "$a" in
    -o|-g) echo "$a $1" >> "$SB_FAKE_STATE/install-owner.log"; shift; i=$((i + 1)) ;;
    *) set -- "$@" "$a" ;;
  esac
done
exec /usr/bin/install "$@"
"#,
    );
    node.bin("useradd", "#!/bin/sh\necho \"useradd $*\" >> \"$SB_FAKE_STATE/useradd.log\"\n");
}

/// A sudo that refuses while $SB_FAKE_STATE/sudo-refuses exists, as a node without the
/// password set up does, and is the fake's own sudo otherwise.
fn refusable_sudo(node: &Node) {
    std::fs::rename(node.bin_dir.join("sudo"), node.bin_dir.join("sudo-real")).unwrap();
    node.bin("sudo", "#!/bin/sh\nif [ -e \"$SB_FAKE_STATE/sudo-refuses\" ]; then echo 'sudo: a password is required' >&2; exit 1; fi\nexec \"$(dirname \"$0\")/sudo-real\" \"$@\"\n");
}

fn phone(saved: &[&str]) -> Channel {
    Channel { topic: Some("sparkbox-alerts".into()), saved: saved.iter().map(|s| s.to_string()).collect(), ..Channel::empty("phone", Kind::Ntfy, "Phone (ntfy)") }
}

fn settings_with(channels: Vec<Channel>) -> NotifySettings {
    NotifySettings { channels, quiet_hours: true, quiet_tz: "America/New_York".into(), ..Default::default() }
}

fn bundle(secret: &str, s: &NotifySettings) -> Bundle {
    let inv = crate::fake_node::inventory(&[("cell1", 1), ("cell2", 2)]);
    let secrets = |id: &str, f: &str| (id == "phone" && f == "token").then(|| secret.to_string());
    Bundle {
        agent: agent_json(&inv, "cell1", &BTreeMap::new(), &Extras::default(), s, &AgentSettings::default()),
        rules: rules_json(&RuleSet::defaults()),
        notify: notify_json(s, secrets, &BTreeMap::new(), None, Some("3f".repeat(32))),
    }
}

struct Rig {
    dir: std::path::PathBuf,
    home: std::path::PathBuf,
    node: Node,
    live: Arc<Live>,
    inv: Inventory,
    layout: Layout,
}

async fn rig(name: &str) -> Rig {
    let dir = temp(name);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (node, port) = fake_node(&home, real_shell()).await;
    let layout = layout_under(&dir.join("sys"));
    systemctl_that_starts(&node, &layout);
    root_tools(&node);
    let (live, inv) = connect(&dir, &[("cell1", port)]).await;
    Rig { dir, home, node, live, inv, layout }
}

async fn install(r: &Rig, b: &Bundle) -> Result<String, String> {
    let tasks = Tasks::new(None);
    let task = tasks.start(TaskSpec::new("agent-install", "Install the always-on agent on 1 node", &["cell1".to_string()]));
    install_node(&r.live, &r.inv.nodes[0], &r.layout, &b.files(Apply::Restart, true), &task).await
}

#[test]
fn config_hash_fixture() {
    let agent = include_bytes!("fixtures/config-hash/agent.json");
    let rules = include_bytes!("fixtures/config-hash/rules.json");
    let notify = include_bytes!("fixtures/config-hash/notify.json");
    let want = include_str!("fixtures/config-hash/expected.txt").trim();
    assert_eq!(config_hash(agent, rules, notify), want);
    let b = Bundle { agent: String::from_utf8(agent.to_vec()).unwrap(), rules: String::from_utf8(rules.to_vec()).unwrap(), notify: String::from_utf8(notify.to_vec()).unwrap() };
    assert_eq!(b.hash(), want);
    // One byte anywhere, or the separators moved, gives another hash.
    assert_ne!(config_hash(agent, rules, &notify[..notify.len() - 1]), want);
    assert_ne!(config_hash(&[agent.as_slice(), b"\0"].concat(), &rules[..], notify), want);
}

#[tokio::test]
async fn install_checks_every_file_as_root_installs_and_starts_it() {
    let r = rig("agent-install").await;
    let secret = "tk_SECRET_do_not_log";
    let b = bundle(secret, &settings_with(vec![phone(&["token"])]));
    let summary = install(&r, &b).await.unwrap();
    assert_eq!(summary, format!("agent {} running on cell1", version()));
    // Every file in place, byte for byte, with its mode; owners given as root.
    for f in b.files(Apply::Restart, true) {
        let (path, mode) = dest_path(&r.layout, &f);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), f.content, "{}", f.name);
        let got = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(format!("0{got:o}"), mode, "{}", f.name);
    }
    let owners = std::fs::read_to_string(r.node.state_dir.join("install-owner.log")).unwrap();
    assert!(owners.lines().all(|l| l == "-o root" || l == "-g root"), "{owners}");
    assert_eq!(std::fs::read_to_string(r.node.state_dir.join("useradd.log")).unwrap().trim(), format!("useradd --system --user-group --no-create-home --home-dir {} --shell /usr/sbin/nologin sparkbox-agent", r.layout.state));
    // The state folder belongs to the service user; requests/ to the login user's group, setgid.
    let chown = std::fs::read_to_string(r.node.state_dir.join("chown.log")).unwrap();
    assert!(chown.contains(&format!("chown -R -h sparkbox-agent:sparkbox-agent {}", r.layout.state)), "{chown}");
    assert!(chown.lines().any(|l| l.starts_with("chown sparkbox-agent:") && l.ends_with(&r.layout.requests()) && !l.contains("sparkbox-agent:sparkbox-agent")), "{chown}");
    assert_eq!(std::fs::metadata(r.layout.requests()).unwrap().permissions().mode() & 0o7777, 0o2770);
    let systemctl = std::fs::read_to_string(r.node.state_dir.join("systemctl.log")).unwrap();
    for want in ["daemon-reload", "enable sparkbox-agent", "restart sparkbox-agent", "is-active sparkbox-agent"] {
        assert!(systemctl.contains(want), "{want}: {systemctl}");
    }
    // One sudo command did it, with the password first on its stdin; the scratch folder is gone.
    let sudo = r.node.ran("sudo -k -S -p '' ");
    assert_eq!(sudo.len(), 1, "{sudo:?}");
    assert!(sudo[0].contains("sha256sum -c") && sudo[0].contains(&sha256_hex(UNIT.as_bytes())), "the hashes are in the root script's own text");
    assert!(std::fs::read_dir(&r.layout.scratch).unwrap().next().is_none(), "the folder under /root is removed");
    let st = read_status(&r.live, &r.inv.nodes[0], &r.layout).await;
    assert_eq!((st.installed, st.state.as_str(), st.config_hash.as_deref()), (true, "running", Some(b.hash().as_str())), "the agent reports the hash of what was installed");
    // Removing it takes everything away and stops the service.
    uninstall_node(&r.live, &r.inv.nodes[0], &r.layout).await.unwrap();
    for p in [&r.layout.opt, &r.layout.etc, &r.layout.unit, &r.layout.state] {
        assert!(!std::path::Path::new(p).exists(), "{p} is gone");
    }
    let st = read_status(&r.live, &r.inv.nodes[0], &r.layout).await;
    assert_eq!((st.installed, st.state.as_str()), (false, "not-installed"));
    r.live.stop();
}

#[tokio::test]
async fn installer_ignores_planted_stage() {
    let r = rig("agent-planted").await;
    // An older version's staging folder, with a unit someone changed to run as root…
    let stage = r.home.join(".sparkbox/agent-stage");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::write(stage.join("sparkbox-agent.service"), UNIT.replace("User=sparkbox-agent", "User=root")).unwrap();
    std::fs::write(stage.join("sparkbox_agent.py"), "import os; os.system('id > /tmp/pwned')\n").unwrap();
    let b = bundle("x", &settings_with(vec![phone(&["token"])]));
    install(&r, &b).await.unwrap();
    // …changes nothing: what's installed is what SparkBox sent, and the folder is gone.
    assert_eq!(std::fs::read_to_string(&r.layout.unit).unwrap(), UNIT, "the unit, byte for byte");
    assert!(UNIT.contains("User=sparkbox-agent\n") && !UNIT.contains("@USER@"));
    assert_eq!(std::fs::read_to_string(format!("{}/sparkbox_agent.py", r.layout.opt)).unwrap(), AGENT_PY);
    assert!(!stage.exists());
    // The root step never names anything under the login user's home.
    for c in r.node.ran("sudo -k -S -p '' ") {
        assert!(!c.contains(&r.home.display().to_string()) && !c.contains("agent-stage"), "{c}");
    }
    // A stage that's a symlink is removed as a link; what it points at is left alone.
    let elsewhere = r.dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("keep"), "mine").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &stage).unwrap();
    install(&r, &b).await.unwrap();
    assert!(std::fs::symlink_metadata(&stage).is_err(), "the link is gone");
    assert_eq!(std::fs::read_to_string(elsewhere.join("keep")).unwrap(), "mine");
    r.live.stop();
}

#[tokio::test]
async fn installer_refuses_bad_hash() {
    let r = rig("agent-damaged").await;
    // From now on `cat` on the node changes a letter of what it copies: same size, wrong bytes.
    r.node.bin("cat", "#!/bin/sh\n/bin/cat \"$@\" | tr 'q' 'Q'\n");
    let b = bundle("x", &settings_with(vec![phone(&["token"])]));
    let err = install(&r, &b).await.unwrap_err();
    assert!(err.contains("A file SparkBox sent arrived damaged; nothing was installed."), "{err}");
    for p in [&r.layout.opt, &r.layout.unit] {
        assert!(!std::path::Path::new(p).exists(), "nothing installed at {p}");
    }
    assert!(!r.node.state_dir.join("useradd.log").exists(), "no user created");
    assert!(std::fs::read_to_string(r.node.state_dir.join("systemctl.log")).unwrap_or_default().lines().all(|l| l.starts_with("is-active")), "nothing started");
    // A stream cut short is refused too.
    r.node.bin("cat", "#!/bin/sh\n/bin/cat \"$@\" | head -c 1000\n");
    let err = install(&r, &b).await.unwrap_err();
    assert!(err.contains("arrived incomplete"), "{err}");
    r.live.stop();
}

#[tokio::test]
async fn secrets_never_on_command_line() {
    let r = rig("agent-secrets").await;
    let secret = "tk_SECRET_do_not_log";
    let b = bundle(secret, &settings_with(vec![phone(&["token"])]));
    install(&r, &b).await.unwrap();
    let agents = Agents::new(Some(r.dir.join("agents.json")));
    push_one(&r.live, &agents, &r.inv.nodes[0], &r.layout, &b, "fp", crate::history::now_ms()).await.unwrap();
    // Everything the client sent as a command (the exec log), and the activity log.
    let raw = r.node.raw.lock().unwrap().clone();
    assert!(!raw.is_empty());
    for c in &raw {
        assert!(!c.contains(secret) && !c.contains("pw\n"), "a secret on a command line: {c}");
    }
    for e in r.live.audit.entries() {
        assert!(!e.command.contains(secret) && !e.stdout_tail.as_deref().unwrap_or("").contains(secret) && !e.stderr_tail.as_deref().unwrap_or("").contains(secret), "{e:?}");
    }
    // It travels on stdin, after sudo's password line, and lands only in the 0600 file.
    let stdin = r.node.stdin.lock().unwrap().clone();
    assert!(stdin.iter().any(|(c, i)| c.starts_with("sudo ") && i.starts_with(b"pw\n") && String::from_utf8_lossy(i).contains(secret)));
    let notify = format!("{}/notify.json", r.layout.etc);
    assert!(std::fs::read_to_string(&notify).unwrap().contains(secret));
    assert_eq!(std::fs::metadata(&notify).unwrap().permissions().mode() & 0o777, 0o600);
    r.live.stop();
}

#[tokio::test]
async fn push_failure_is_persisted_and_retried() {
    let r = rig("agent-retry").await;
    let s = settings_with(vec![phone(&["token"])]);
    let first = bundle("tk_1", &s);
    install(&r, &first).await.unwrap();
    refusable_sudo(&r.node);
    let path = r.dir.join("agents.json");
    let agents = Agents::new(Some(path.clone()));
    agents.update("cell1", |rec| rec.installed = true);
    // The owner changes the quiet hours; the node refuses sudo.
    std::fs::write(r.node.state_dir.join("sudo-refuses"), "").unwrap();
    let next = bundle("tk_1", &NotifySettings { quiet_start: "23:00".into(), ..s.clone() });
    let err = push_one(&r.live, &agents, &r.inv.nodes[0], &r.layout, &next, "fp2", crate::history::now_ms()).await.unwrap_err();
    assert!(err.contains("sudo: a password is required"), "{err}");
    // Kept on disk, with what's expected there now; a restart of SparkBox keeps it.
    let again = Agents::new(Some(path.clone()));
    let rec = again.record("cell1");
    let e = rec.push_error.clone().expect("the failure is kept");
    assert_eq!((e.message.as_str(), e.attempts), ("sudo: a password is required", 1));
    assert_eq!(rec.expected_hash.as_deref(), Some(next.hash().as_str()));
    again.set_live(read_status(&r.live, &r.inv.nodes[0], &r.layout).await);
    let st = again.statuses(false, &r.inv, crate::history::now_ms()).remove(0);
    assert_eq!(st.config_current, Some(false), "the node still runs the old settings");
    let ws = watch_state(vec![st], true, crate::history::now_ms());
    assert_eq!((ws.drift[0].reason.as_str(), ws.push_failures.len()), ("push failed: sudo: a password is required", 1));
    // Tried again at once when the node comes back, else after 5 minutes, then 15, 60 and 6 hours.
    let now = e.at_ms;
    assert!(retry_due(&rec, true, now + 1_000), "on reconnect");
    assert!(!retry_due(&rec, false, now + 60_000));
    assert!(retry_due(&rec, false, now + 5 * 60_000));
    assert_eq!((1..=6).map(backoff_ms).map(|ms| ms / 60_000).collect::<Vec<_>>(), [5, 15, 60, 360, 360, 360]);
    let err = auto_push(&r.live, &again, &r.inv.nodes[0], &r.layout, &next, "fp2", PushReason::Retry, now + 5 * 60_000).await.unwrap_err();
    assert!(err.contains("password is required"));
    assert_eq!(again.record("cell1").push_error.unwrap().attempts, 2);
    // sudo works again: the retry goes through and the node reports the expected hash.
    std::fs::remove_file(r.node.state_dir.join("sudo-refuses")).unwrap();
    assert_eq!(auto_push(&r.live, &again, &r.inv.nodes[0], &r.layout, &next, "fp2", PushReason::Retry, now + 20 * 60_000).await.unwrap(), Apply::Restart);
    assert_eq!((again.record("cell1").push_error, again.record("cell1").retry_row), (None, None));
    again.set_live(read_status(&r.live, &r.inv.nodes[0], &r.layout).await);
    assert_eq!(again.statuses(false, &r.inv, crate::history::now_ms())[0].config_current, Some(true));
    assert!(std::fs::read_to_string(format!("{}/agent.json", r.layout.etc)).unwrap().contains("\"start\": \"23:00\""));
    // The automatic push is one row of source SparkBox, which ends ok on the try that went through.
    let heads: Vec<crate::live::audit::Entry> = r.live.audit.entries().into_iter().filter(|e| e.kind == "op").collect();
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert_eq!((heads[0].title.as_str(), heads[0].source.as_str(), heads[0].actor.as_str(), heads[0].state.as_str()), ("Push alert settings to cell1", "system", "system", "ok"));
    assert_eq!(heads[0].stdout_tail.as_deref(), Some("restarted · went through on try 3"));
    r.live.stop();
}

/// A node that refuses sudo for a whole day: the retries back off (5, 15, 60 minutes, then
/// every 6 hours) and stay on one row in Changes, which says how often it was tried.
#[tokio::test]
async fn a_day_of_failed_retries_is_one_changes_row() {
    let r = rig("agent-flood").await;
    let s = settings_with(vec![phone(&["token"])]);
    install(&r, &bundle("tk_1", &s)).await.unwrap();
    refusable_sudo(&r.node);
    std::fs::write(r.node.state_dir.join("sudo-refuses"), "").unwrap();
    let agents = Agents::new(None);
    agents.update("cell1", |rec| rec.installed = true);
    let next = bundle("tk_1", &NotifySettings { quiet_start: "23:00".into(), ..s.clone() });
    let start = 1_800_000_000_000u64;
    // The settings changed by themselves (an endpoint moved): pushed once, refused.
    auto_push(&r.live, &agents, &r.inv.nodes[0], &r.layout, &next, "fp2", PushReason::Changed, start).await.unwrap_err();
    let mut tries = vec![0u64];
    for minute in 1..=24 * 60u64 {
        let now = start + minute * 60_000;
        if retry_due(&agents.record("cell1"), false, now) {
            auto_push(&r.live, &agents, &r.inv.nodes[0], &r.layout, &next, "fp2", PushReason::Retry, now).await.unwrap_err();
            tries.push(minute);
        }
    }
    assert_eq!(tries, [0, 5, 20, 80, 440, 800, 1160], "backed off, not every 5 minutes (288 a day)");
    let changes: Vec<crate::live::audit::Entry> = r.live.audit.entries().into_iter().filter(|e| e.changes && e.node == "cell1" && e.ts_ms > 0).collect();
    let rows: std::collections::HashSet<String> = changes.iter().filter(|e| e.title.starts_with("Push alert settings")).map(|e| e.op_id.clone().unwrap_or_else(|| e.id.clone())).collect();
    assert_eq!(rows.len(), 1, "one row in Changes for the whole day: {rows:?}");
    let head = changes.iter().find(|e| e.kind == "op" && e.title.starts_with("Push alert settings")).unwrap();
    assert_eq!(head.state, "failed");
    assert_eq!(head.error.as_deref(), Some(format!("failed · retried 6 times, last {}: sudo: a password is required", crate::notify::clock_in("", start + 1160 * 60_000)).as_str()));
    assert_eq!(agents.record("cell1").push_error.unwrap().attempts, 7);
    r.live.stop();
}

#[tokio::test]
async fn push_reloads_or_restarts_as_the_change_needs() {
    let r = rig("agent-reload").await;
    let secret = "tk_SECRET_reload";
    let s = settings_with(vec![phone(&["token"])]);
    let b = bundle(secret, &s);
    install(&r, &b).await.unwrap();
    let agents = Agents::new(None);
    // Nothing known about the node yet: every file and a restart.
    assert_eq!(push_one(&r.live, &agents, &r.inv.nodes[0], &r.layout, &b, "fp", crate::history::now_ms()).await.unwrap(), Apply::Restart);
    agents.set_live(read_status(&r.live, &r.inv.nodes[0], &r.layout).await);
    // Only the rules change: agent.json and rules.json go, the agent reloads, no secret travels.
    let rules = RuleSet::defaults();
    let mut fewer = rules.clone();
    fewer.rules.retain(|x| x.id != "gpu-hot");
    let b2 = Bundle { rules: rules_json(&fewer), ..b.clone() };
    let before = r.node.stdin.lock().unwrap().len();
    assert_eq!(apply_for(&agents.record("cell1"), &b2), Apply::Reload);
    assert_eq!(push_one(&r.live, &agents, &r.inv.nodes[0], &r.layout, &b2, "fp", crate::history::now_ms()).await.unwrap(), Apply::Reload);
    let sent: Vec<(String, Vec<u8>)> = r.node.stdin.lock().unwrap()[before..].to_vec();
    let (_, input) = sent.iter().find(|(c, _)| c.starts_with("sudo ")).unwrap();
    assert!(!String::from_utf8_lossy(input).contains(secret), "notify.json isn't sent for a reload");
    let log = std::fs::read_to_string(r.node.state_dir.join("systemctl.log")).unwrap();
    assert_eq!(log.lines().last(), Some("reload sparkbox-agent"), "{log}");
    agents.set_live(read_status(&r.live, &r.inv.nodes[0], &r.layout).await);
    assert_eq!(agents.statuses(false, &r.inv, crate::history::now_ms())[0].config_current, Some(true), "the reloaded agent reports the new hash");
    // A channel changes: notify.json goes and the agent restarts (it reads the credential at start).
    let b3 = bundle(secret, &settings_with(vec![Channel { critical_only: true, ..phone(&["token"]) }]));
    assert_eq!(push_one(&r.live, &agents, &r.inv.nodes[0], &r.layout, &b3, "fp", crate::history::now_ms()).await.unwrap(), Apply::Restart);
    let log = std::fs::read_to_string(r.node.state_dir.join("systemctl.log")).unwrap();
    assert_eq!(log.lines().last(), Some("restart sparkbox-agent"));
    // A node that isn't running what this Mac pushed last gets everything again.
    let stale = NodeRecord { reported_hash: Some("other".into()), ..agents.record("cell1") };
    assert_eq!(apply_for(&stale, &b3), Apply::Restart);
    r.live.stop();
}

/// Plays the agent's side of requests/ on a fake node: every signed request that verifies
/// with `key` gets {ok: true}, anything else the agent's refusal, as sparkbox_agent.py answers.
fn answer_requests(dir: std::path::PathBuf, key: &'static str) -> tokio::task::JoinHandle<Vec<Value>> {
    tokio::spawn(async move {
        let mut taken = Vec::new();
        for _ in 0..80 {
            if let Ok(list) = std::fs::read_dir(&dir) {
                for e in list.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if !name.ends_with(".json") || name.ends_with(".result.json") {
                        continue;
                    }
                    let req: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap_or_default()).unwrap_or_default();
                    std::fs::remove_file(e.path()).ok();
                    let (kind, body) = (req["kind"].as_str().unwrap_or(""), req["body"].as_str().unwrap_or(""));
                    let good = req["mac"].as_str() == Some(hmac_sha256_hex(key.as_bytes(), format!("{kind}\n{body}").as_bytes()).as_str());
                    let answer = if good { json!({"ok": true, "error": null}) } else { json!({"ok": false, "error": "its signature doesn't match"}) };
                    std::fs::write(dir.join(name.replace(".json", ".result.json")), answer.to_string()).unwrap();
                    if good {
                        taken.push(serde_json::from_str(body).unwrap());
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        taken
    })
}

#[tokio::test]
async fn maintenance_notes_are_signed_answered_and_bounded_in_time() {
    let dir = temp("agent-maint");
    let (mut nodes, mut ports) = (Vec::new(), Vec::new());
    for id in ["cell1", "cell2"] {
        let home = dir.join(id);
        std::fs::create_dir_all(&home).unwrap();
        let (node, port) = fake_node(&home, real_shell()).await;
        nodes.push(node);
        ports.push((id, port));
    }
    let (live, inv) = connect(&dir, &ports).await;
    let (gate, _, _) = crate::admin::testing::gate();
    // Each fake node has its own /var/lib/sparkbox-agent (they share this Mac's disk).
    let layouts: Vec<Layout> = ["cell1", "cell2"].iter().map(|n| layout_under(&dir.join("sys").join(n))).collect();
    for l in &layouts {
        std::fs::create_dir_all(l.requests()).unwrap();
    }
    let layout = layouts[0].clone();
    const KEY: &str = "3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f";
    let body = maintenance_note(&["cell3".into()], Some(1_000_600_000), "restarting", "SparkBox", 1_000_000_000, "t-42", false).unwrap();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v, json!({ "nodes": ["cell3"], "untilMs": 1_000_600_000u64, "reason": "restarting", "by": "SparkBox", "startedMs": 1_000_000_000u64, "op": "t-42" }));
    // Both agents take it and answer, at the same time; the Mac reads each answer within the 5 s.
    let agent_side: Vec<_> = layouts.iter().map(|l| answer_requests(std::path::PathBuf::from(l.requests()), KEY)).collect();
    let started = std::time::Instant::now();
    let content = signed_request(KEY, "maintenance", &body);
    let wait = NOTE_TIMEOUT - Duration::from_millis(500);
    let (a, b) = tokio::join!(
        deliver_all(&live, &gate, &inv.nodes[..1], &layouts[0], "maintenance-1-abc", &content, wait),
        deliver_all(&live, &gate, &inv.nodes[1..], &layouts[1], "maintenance-1-abc", &content, wait)
    );
    let results: Vec<_> = a.into_iter().chain(b).collect();
    assert!(results.iter().all(|(_, r)| r.is_ok()), "{results:?}");
    assert!(started.elapsed() < NOTE_TIMEOUT, "{:?}", started.elapsed());
    for n in &nodes {
        assert!(n.ran("sudo").is_empty(), "written as the login user");
        assert_eq!(n.ran("maintenance-1-abc.json").len(), 1);
    }
    // Signed with another key: refused, and the refusal comes back.
    let forged = signed_request(&"00".repeat(32), "maintenance", &body);
    let results = deliver_all(&live, &gate, &inv.nodes[..1], &layout, "maintenance-2-abc", &forged, Duration::from_secs(3)).await;
    assert_eq!(results[0].1, Err("the agent refused it: its signature doesn't match".into()));
    let mut taken = 0;
    for t in agent_side {
        taken += t.await.unwrap().len();
    }
    assert_eq!(taken, 2, "only the signed note was taken, once per node");
    // No agent answering: each node gives up when the wait is over, not later.
    let started = std::time::Instant::now();
    let results = deliver_all(&live, &gate, &inv.nodes, &layout, "maintenance-3-abc", &signed_request(KEY, "maintenance", &body), Duration::from_millis(1500)).await;
    assert!(results.iter().all(|(_, r)| r.as_ref().unwrap_err() == "its agent didn't answer in 1 s"), "{results:?}");
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    // The end record, the 4-hour cap, and the limits.
    let end: Value = serde_json::from_str(&maintenance_note(&["cell3".into()], None, "ended", "SparkBox", 5, "t-42", true).unwrap()).unwrap();
    assert_eq!((end["untilMs"].as_u64(), end["end"].as_bool()), (Some(5), Some(true)));
    let long: Value = serde_json::from_str(&maintenance_note(&["cell3".into()], Some(5 + 9 * 3_600_000), "x", "SparkBox", 5, "", false).unwrap()).unwrap();
    assert_eq!(long["untilMs"].as_u64(), Some(5 + MAINT_MAX_MS), "never longer than 4 hours");
    assert!(maintenance_note(&["cell3; rm -rf /".into()], None, "x", "SparkBox", 5, "", false).is_err());
    assert!(maintenance_note(&["cell3".into()], None, &"x".repeat(10_000), "SparkBox", 5, "", false).unwrap().len() <= MAINT_MAX);
    live.stop();
}

#[test]
fn requests_are_signed_as_the_agent_checks_them() {
    // fixtures/signed-request.json: agent_test.py checks the same signature with sparkbox_agent.sign.
    let f: Value = serde_json::from_str(include_str!("fixtures/signed-request.json")).unwrap();
    let (key, kind, body) = (f["key"].as_str().unwrap(), f["kind"].as_str().unwrap(), f["body"].as_str().unwrap());
    let req: Value = serde_json::from_str(&signed_request(key, kind, body)).unwrap();
    assert_eq!(req, json!({ "v": 1, "kind": kind, "body": body, "mac": f["mac"] }));
    // RFC 4231 test case 2.
    assert_eq!(hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
}

#[test]
fn acks_are_spelled_the_way_the_agents_gate_keys_them() {
    let a: crate::health::Alert = serde_json::from_value(json!({
        "id": "disk-full:disk:gx10-0000:/", "rule": "disk-full", "category": "disk", "severity": "warn", "state": "firing",
        "entity": { "kind": "disk", "id": "/", "node": "gx10-0000", "switch": null, "label": "Root disk on gx10-0000" },
        "message": "Root disk 91% full", "summary": "gx10-0000 disk 91%", "detail": null, "value": 91.0, "sinceMs": 1, "firingSinceMs": 1,
        "resolvedMs": null, "ackedMs": 1_800_000_000_000u64, "mutedUntilMs": null, "incidentId": "disk:gx10-0000:/"
    }))
    .unwrap();
    let mut unacked = a.clone();
    unacked.acked_ms = None;
    let acks = ack_bodies(&[a.clone(), unacked]);
    assert_eq!(acks.len(), 1, "only acknowledged, firing alerts");
    let body: Value = serde_json::from_str(&acks[0].1).unwrap();
    assert_eq!(body, json!({ "rule": "disk-full", "entity": "disk:gx10-0000:/", "incident": "disk:gx10-0000:/", "untilEscalated": true, "ackedMs": 1_800_000_000_000u64, "by": "SparkBox on this Mac" }));
    assert!(acks[0].0.starts_with("ack-") && acks[0].0.len() == 20);
    let e = |kind: &str, id: &str, node: Option<&str>, switch: Option<&str>| crate::health::Entity { kind: kind.into(), id: id.into(), node: node.map(String::from), switch: switch.map(String::from), label: String::new() };
    assert_eq!(gate_entity(&e("node", "cell3", Some("cell3"), None)), "node:cell3");
    assert_eq!(gate_entity(&e("container", "vllm-head", Some("cell1"), None)), "container:cell1/vllm-head");
    assert_eq!(gate_entity(&e("interface", "enP2p1s0f1np1", Some("cell2"), None)), "net:cell2/enP2p1s0f1np1");
    assert_eq!(gate_entity(&e("port", "qsfp28-2-1", None, Some("crs804"))), "port:crs804/qsfp28-2-1");
    let mut cleared = a;
    cleared.state = crate::health::AlertState::Resolved;
    assert!(ack_bodies(&[cleared]).is_empty());
}

#[test]
fn a_test_page_proves_the_channels_not_the_silences() {
    let inv = crate::fake_node::inventory(&[("cell1", 1)]);
    let s = settings_with(vec![phone(&["token"])]);
    let secret = |id: &str, f: &str| (id == "phone" && f == "token").then(|| "tk_1".to_string());
    let notify = |s: &NotifySettings, tk: &str| notify_json(s, |id, f| secret(id, f).map(|_| tk.to_string()), &BTreeMap::new(), None, Some("k".into()));
    let tested = paging_hash(&notify(&s, "tk_1"));
    // A silence, quiet hours, repeats and rules are agent.json and rules.json: same paging hash.
    let quieter = NotifySettings { quiet_start: "21:00".into(), repeat_minutes: 15, critical_only: true, ..s.clone() };
    assert_eq!(paging_hash(&notify(&quieter, "tk_1")), tested);
    let silenced = Extras { silences: vec![json!({"id": "s1", "rule": "disk-full"})], ..Default::default() };
    assert_ne!(agent_json(&inv, "cell1", &BTreeMap::new(), &silenced, &s, &AgentSettings::default()), agent_json(&inv, "cell1", &BTreeMap::new(), &Extras::default(), &s, &AgentSettings::default()));
    // A new token, another topic, or the channel kept to this Mac: a different hash.
    assert_ne!(paging_hash(&notify(&s, "tk_2")), tested);
    assert_ne!(paging_hash(&notify(&settings_with(vec![Channel { topic: Some("other".into()), ..phone(&["token"]) }]), "tk_1")), tested);
    assert_ne!(paging_hash(&notify(&settings_with(vec![Channel { mac_only: true, ..phone(&["token"]) }]), "tk_1")), tested);
    // The watch state: proven while what the node runs is what was tested, again when it's put back.
    let ch = vec![WatchChannel { id: "phone".into(), name: "Phone (ntfy)".into(), remote: true, enabled: true, ..Default::default() }];
    let run = TestRun { at_ms: 100, results: [("phone".to_string(), TestCell { ok: true, ..Default::default() })].into(), paging: Some(tested.clone()) };
    let node = |paging: &str| AgentStatus { node: "cell1".into(), installed: true, last_test: Some(run.clone()), paging: Some(paging.into()), ..Default::default() };
    assert!(end_to_end(&[node(&tested)], &ch, 5_000).ok, "a silence after the test leaves it standing");
    assert!(!end_to_end(&[node("other")], &ch, 50).ok, "a channel changed: untested");
    assert!(end_to_end(&[node(&tested)], &ch, 9_999).ok, "changed back: proven again");
}

#[test]
fn copying_the_owners_secrets_needs_mac_scope() {
    assert!(copies_secrets(&settings_with(vec![phone(&["token"])]), false, false));
    assert!(!copies_secrets(&settings_with(vec![phone(&[])]), false, false), "a public topic has nothing secret");
    assert!(!copies_secrets(&settings_with(vec![Channel { mac_only: true, ..phone(&["token"]) }]), false, false), "kept to this Mac");
    assert!(copies_secrets(&settings_with(vec![]), true, false), "an endpoint's API key");
    assert!(copies_secrets(&settings_with(vec![]), false, true), "the metrics token");
    // The request key is SparkBox's own, made for the agents.
    let n: Value = serde_json::from_str(&notify_json(&settings_with(vec![]), |_, _| None, &BTreeMap::new(), None, Some("k1".into()))).unwrap();
    assert_eq!(n["requestKey"], "k1");
}

#[test]
fn root_script_embeds_each_files_size_and_hash_and_runs_nothing_from_home() {
    let layout = Layout::system();
    let b = bundle("x", &settings_with(vec![phone(&["token"])]));
    let files = b.files(Apply::Restart, true);
    let s = root_script(&layout, &files, "sparkuser", true, Apply::Restart);
    assert!(s.starts_with("set -eu\numask 077\nw=$(mktemp -d '/root/sparkbox-agent.XXXXXXXX')\n"), "{s}");
    for f in &files {
        assert!(s.contains(&format!("{}  f", sha256_hex(f.content.as_bytes()))), "{}", f.name);
        assert!(s.contains(&format!("| head -c {} >", f.content.len())));
    }
    let at = |name: &str| files.iter().position(|f| f.name == name).unwrap();
    assert!(s.contains(&format!("install -o root -g root -m 0600 \"$w/f{}\" '/etc/sparkbox-agent/notify.json'", at("notify.json"))), "{s}");
    assert!(s.contains(&format!("install -o root -g root -m 0644 \"$w/f{}\" '/etc/systemd/system/sparkbox-agent.service'", at("sparkbox-agent.service"))), "{s}");
    assert!(s.contains(&format!("install -o root -g root -m 0644 \"$w/f{}\" '/opt/sparkbox-agent/gate.py'", at("gate.py"))), "{s}");
    assert!(s.contains("chown sparkbox-agent:\"$(id -gn 'sparkuser')\" '/var/lib/sparkbox-agent/requests'"));
    assert!(!s.contains("$HOME") && !s.contains("~/"), "nothing from the login user's home");
    assert!(s.ends_with("systemctl daemon-reload\nsystemctl enable sparkbox-agent\nsystemctl restart sparkbox-agent\n"));
    // A push: settings only, then reload or restart.
    let push = root_script(&layout, &b.files(Apply::Reload, false), "sparkuser", false, Apply::Reload);
    assert!(!push.contains("useradd") && !push.contains("notify.json") && push.ends_with("systemctl reload sparkbox-agent\n"), "{push}");
    let u = uninstall_script(&layout);
    assert!(u.contains("rm -rf '/opt/sparkbox-agent' '/etc/sparkbox-agent' '/var/lib/sparkbox-agent'"));
    // The unit the README and the install dialog describe.
    for line in [
        "User=sparkbox-agent", "Group=sparkbox-agent", "SupplementaryGroups=systemd-journal docker", "StateDirectory=sparkbox-agent",
        "LoadCredential=notify:/etc/sparkbox-agent/notify.json", "Type=notify", "NotifyAccess=main", "WatchdogSec=30",
        "ExecReload=/bin/kill -HUP $MAINPID", "Restart=always", "RestartSec=5", "ProtectSystem=strict", "ProtectHome=yes", "PrivateTmp=yes",
        "NoNewPrivileges=yes", "ReadWritePaths=/var/lib/sparkbox-agent",
    ] {
        assert!(UNIT.lines().any(|l| l == line), "{line}");
    }
}

#[test]
fn status_lines_read_as_states() {
    let n = crate::fake_node::inventory(&[("cell1", 1)]).nodes.remove(0);
    let st = parse_status(&n, "active\n{\"version\":\"1.0.0\",\"lastBeatMs\":5,\"historyFromMs\":1,\"uptimeS\":40,\"firing\":[{\"id\":\"a\"}],\"lastDelivery\":{\"tsMs\":4,\"ok\":false},\"metrics\":{\"enabled\":true,\"port\":9470}}");
    assert_eq!((st.state.as_str(), st.installed, st.firing, st.last_delivery_ok, st.metrics_port), ("running", true, 1, Some(false), Some(9470)));
    assert_eq!(parse_status(&n, "failed\n{\"version\":\"1.0.0\"}").state, "failed");
    assert_eq!(parse_status(&n, "inactive\n").state, "not-installed");
    assert_eq!(parse_status(&n, "inactive\n{\"version\":\"1.0.0\"}").state, "stopped");
    // The round-three status (C-agent).
    let st = parse_status(&n, "active\n{\"version\":\"1.1.0\",\"lastBeatMs\":5,\"configHash\":\"abc\",\"lastCycleMs\":4,\"missing\":[\"cell3\"],\"isolated\":null,\"leader\":\"cell1\",\"outboxPending\":2,\"lastDelivery\":{\"ms\":3,\"ok\":true,\"channel\":\"phone\"},\"heartbeatOkMs\":2,\"endpointsProbed\":[\"llama\"],\"quietTz\":\"America/New_York\"}");
    assert_eq!(st.config_hash.as_deref(), Some("abc"));
    assert_eq!((st.peers_missing.clone(), st.endpoints_probed.clone(), st.outbox_pending, st.leader), (vec!["cell3".to_string()], vec!["llama".to_string()], 2, Some(true)));
    assert_eq!((st.last_delivery_ms, st.last_delivery_channel.as_deref(), st.heartbeat_ok_ms), (Some(3), Some("phone"), Some(2)));
}

/// Every module sparkbox_agent.py and the collector import from their own folder is installed.
#[test]
fn every_module_the_agent_imports_is_shipped() {
    let shipped: Vec<&str> = program_files().iter().map(|(n, _)| *n).collect();
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let local: Vec<String> = ["agent", "live"]
        .iter()
        .flat_map(|d| std::fs::read_dir(here.join(d)).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.ends_with(".py") && !n.ends_with("_test.py"))
        .map(|n| n.trim_end_matches(".py").to_string())
        .collect();
    for (name, text) in program_files() {
        for line in text.lines().map(str::trim) {
            let Some(module) = line.strip_prefix("import ").or_else(|| line.strip_prefix("from ")).and_then(|r| r.split([' ', '.', ',']).next()) else { continue };
            if local.iter().any(|l| l == module) {
                assert!(shipped.contains(&format!("{module}.py").as_str()), "{name} imports {module}, which isn't installed");
            }
        }
    }
}

#[test]
fn the_versions_agree() {
    assert!(!version().is_empty());
    assert!(AGENT_PY.contains(&format!("VERSION = \"{}\"", version())));
}

#[test]
fn agent_json_carries_peers_quiet_hours_and_the_rest() {
    let inv = crate::fake_node::inventory(&[("cell1", 1), ("cell2", 2), ("cell3", 3), ("gx10-0000", 4)]);
    let addrs: BTreeMap<String, Addrs> = [("cell2".to_string(), Addrs { lan: Some("192.168.1.102".into()), fabric: vec!["192.168.101.12".into(), "192.168.100.12".into()] })].into();
    let extras = Extras {
        switch_mgmt: Some(json!({ "id": "crs804", "name": "CRS804 DDQ", "host": "192.168.88.1" })),
        endpoints: vec![json!({"id": "llama", "headNode": "cell3"})],
        silences: vec![json!({"id": "s1", "rule": null, "entity": "node:gx10-0000", "untilMs": null, "reason": "rack move"})],
    };
    let n = NotifySettings { quiet_hours: true, quiet_tz: "Europe/Paris".into(), repeat_minutes: 30, disk: false, ..Default::default() };
    let v: Value = serde_json::from_str(&agent_json(&inv, "cell3", &addrs, &extras, &n, &AgentSettings { metrics_enabled: true, ..Default::default() })).unwrap();
    assert_eq!((v["schema"].as_u64(), v["index"].as_u64(), v["node"].as_str()), (Some(2), Some(2), Some("cell3")));
    assert_eq!(v["peers"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect::<Vec<_>>(), ["cell1", "cell2", "cell3", "gx10-0000"]);
    assert_eq!(v["peers"][1]["fabric"], json!(["192.168.100.12", "192.168.101.12"]));
    assert_eq!((v["peers"][1]["lan"].as_str(), v["peers"][1]["addr"].as_str()), (Some("192.168.1.102"), Some("192.168.100.12")));
    assert_eq!(v["peers"][0]["lan"], "127.0.0.1", "the inventory address when the snapshot has none");
    assert_eq!(v["quiet"], json!({ "start": "22:00", "end": "07:00", "tz": "Europe/Paris" }));
    assert_eq!((v["repeatMinutes"].as_u64(), v["categories"]["disk"].as_bool(), v["switchMgmt"]["host"].as_str()), (Some(30), Some(false), Some("192.168.88.1")));
    assert_eq!((v["endpoints"][0]["id"].as_str(), v["silences"][0]["reason"].as_str()), (Some("llama"), Some("rack move")));
    assert_eq!((v["port"].as_u64(), v["metrics"]["enabled"].as_bool(), v["metrics"]["port"].as_u64()), (Some(9469), Some(true), Some(9470)));
    // notify.json: channels with their secrets, except Mac-only and switched-off ones.
    let s = NotifySettings {
        channels: vec![phone(&["token"]), Channel { mac_only: true, ..Channel::empty("mail", Kind::Email, "Email") }, Channel { enabled: false, ..Channel::empty("hook", Kind::Webhook, "Hook") }],
        ..Default::default()
    };
    let notify: Value = serde_json::from_str(&notify_json(&s, |_, _| Some("tk_secret".into()), &[("llama".to_string(), "sk-1".to_string())].into(), None, None)).unwrap();
    assert_eq!(notify["channels"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect::<Vec<_>>(), ["phone"]);
    assert_eq!((notify["channels"][0]["secrets"]["token"].as_str(), notify["endpointKeys"]["llama"].as_str()), (Some("tk_secret"), Some("sk-1")));
    assert_eq!(secret_items(&s, &["llama".to_string()]), ["Phone (ntfy): token", "llama: API key"]);
    // The crash-loop timing the agents use is the Mac's.
    assert_eq!((v["crash"]["recentS"].as_u64(), v["crash"]["windowS"].as_u64()), (Some(crate::health::rules::CRASH_RECENT_S), Some(crate::health::rules::CRASH_WINDOW_S)));
    assert_eq!(v["tz"], "Europe/Paris", "times in the agents' messages follow the zone quiet hours were set in");
}

#[test]
fn the_test_page_result_reads_as_a_list_or_a_map() {
    let run = parse_test_all("starting\n{\"ok\":true,\"results\":[{\"channel\":\"phone\",\"name\":\"Phone (ntfy)\",\"ok\":true,\"ms\":812},{\"channel\":\"mail\",\"ok\":false,\"error\":\"SMTP 535 — check the user and password\"}]}", 7).unwrap();
    assert_eq!((run.at_ms, run.results["phone"].ms, run.results["mail"].ok), (7, Some(812), false));
    let run = parse_test_all("{\"results\":{\"phone\":{\"ok\":true,\"ms\":500}}}", 7).unwrap();
    assert!(run.results["phone"].ok);
    assert_eq!(parse_test_all("{\"ok\":false,\"message\":\"The agent didn't answer in 20 s.\"}", 7).unwrap_err(), "The agent didn't answer in 20 s.");
    // The agent's own answer when it couldn't send anything.
    assert_eq!(parse_test_all("{\"node\":\"cell1\",\"ok\":false,\"message\":\"Couldn't ask the agent: Permission denied\",\"results\":[]}", 7).unwrap_err(), "Couldn't ask the agent: Permission denied");
    let rows = test_rows("cell1", "cell1", &parse_test_all("{\"results\":{\"phone\":{\"ok\":true,\"ms\":812}}}", 7).unwrap(), &[phone(&[])], false);
    assert_eq!(rows[0].body, "Test from cell1 via Phone (ntfy): delivered in 0.8 s");
    assert_eq!((rows[0].via.as_str(), rows[0].test, rows[0].outcome), ("agent:cell1", true, crate::delivery::Outcome::Sent));
}

#[test]
fn backfill_merges_into_history_and_marks_the_stretch() {
    let h = crate::history::History::new(None);
    let now = crate::history::now_ms();
    let base = now - 3_600_000;
    let lines: String = (0..30)
        .map(|i| format!("{{\"e\":\"node:cell1\",\"t\":{},\"s\":10,\"v\":{{\"cpu\":{}.0,\"gpu\":50.0}}}}\n", base + i * 10_000, i))
        .chain(["{\"e\":\"container:cell1/vllm-head\",\"t\":".to_string() + &base.to_string() + ",\"s\":10,\"v\":{\"cpu\":3.0}}\n", "not json\n".to_string()])
        .collect();
    assert_eq!(merge_export(&h, false, &lines), 31);
    let r = h.query("node:cell1", &["cpu".into()], base, base + 400_000, 1000).unwrap();
    assert_eq!(r.tier, "10s");
    assert!(r.series["cpu"].iter().flatten().any(|v| *v == 29.0));
    assert_eq!(r.agent_ranges, vec![[base, base + 290_000]]);
    // A second merge of the same export adds nothing (SparkBox already has that time).
    assert_eq!(merge_export(&h, false, &lines), 0);
    assert!(h.query("container:cell1/vllm-head", &["cpu".into()], base - 60_000, base + 60_000, 100).unwrap().series["cpu"].iter().flatten().count() > 0);
}

fn alert_line(ts: u64, rule: &str, node: &str) -> String {
    format!(
        r#"{{"tsMs":{ts},"kind":"raised","alert":{{"id":"{rule}:node:{node}","rule":"{rule}","category":"node","severity":"crit","state":"firing","entity":{{"kind":"node","id":"{node}","node":"{node}","switch":null,"label":"{node}"}},"message":"m","summary":"s","detail":null,"value":null,"sinceMs":{ts},"firingSinceMs":{ts},"resolvedMs":null,"ackedMs":null,"mutedUntilMs":null,"notify":true}}}}"#
    )
}

#[test]
fn alerts_from_while_sparkbox_was_closed_are_marked_away() {
    let text = format!("{}\n{}\n", alert_line(1_000, "peer-unreachable", "cell3"), alert_line(9_000, "mem-high", "cell1"));
    let ev = away_events(&text, "cell1", 5_000);
    assert_eq!(ev.len(), 1, "the later one SparkBox saw for itself");
    assert!(ev[0].away);
    assert_eq!(ev[0].source.as_deref(), Some("agent:cell1"));
    let h = crate::health::Health::new(None);
    assert_eq!(h.import(ev.clone()), 1);
    assert_eq!(h.import(ev), 0, "imported once");
    assert!(h.history(10)[0].away);
}

// ---- Who sends

fn scope(rule: &str, kind: &str, id: &str, node: Option<&str>, switch: Option<&str>) -> Scope {
    Scope { rule: rule.into(), kind: kind.into(), id: id.into(), node: node.map(String::from), switch: switch.map(String::from) }
}

/// A snapshot where each listed node has an agent beat with these traits.
fn ctx_with(beats: &[(&str, u64, Option<bool>, &[&str], &[&str])]) -> PagingCtx {
    let mut snap = crate::demo::DemoCluster::new(&crate::inventory::Inventory::example()).snapshot();
    let now = snap.ts_ms;
    for n in &mut snap.nodes {
        n.agent = beats.iter().find(|b| b.0 == n.id).map(|(_, age, current, missing, probed)| AgentBeat {
            version: version().into(),
            last_beat_ms: now - age,
            config_current: *current,
            peers_missing: missing.iter().map(|s| s.to_string()).collect(),
            endpoints_probed: probed.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        });
    }
    PagingCtx::new(&snap, &RuleSet::defaults())
}

#[test]
fn old_agents_do_not_take_paging_for_memory_metrics_they_cannot_read() {
    let mut snap = crate::demo::DemoCluster::new(&crate::inventory::Inventory::example()).snapshot();
    for v in ["1.2.0", "", "unknown", version()] {
        snap.nodes[0].agent = Some(AgentBeat {
            version: v.into(), last_beat_ms: snap.ts_ms, config_current: Some(true), ..Default::default()
        });
        let node = &snap.nodes[0].id;
        let ctx = PagingCtx::new(&snap, &RuleSet::defaults());
        for rule in ["mem-available", "mem-pressure", "mem-swap-in"] {
            assert_eq!(agent_sends(&scope(rule, "node", node, Some(node), None), &ctx),
                (v == version()).then(|| node.clone()), "{rule} on agent {v}");
        }
        assert_eq!(agent_sends(&scope("disk-full", "node", node, Some(node), None), &ctx), Some(node.clone()));
    }
}

#[test]
fn stale_agent_doesnt_take_paging() {
    let mem = scope("mem-high", "node", "cell1", Some("cell1"), None);
    assert_eq!(agent_sends(&mem, &ctx_with(&[("cell1", 2_000, Some(true), &[], &[])])), Some("cell1".into()), "healthy and current: the agent pages");
    assert_eq!(agent_sends(&mem, &ctx_with(&[("cell1", 2_000, Some(false), &[], &[])])), None, "old settings: this Mac pages");
    assert_eq!(agent_sends(&mem, &ctx_with(&[("cell1", 2_000, None, &[], &[])])), None, "settings unknown (an older agent): this Mac pages");
    assert_eq!(agent_sends(&mem, &ctx_with(&[("cell1", 61_000, Some(true), &[], &[])])), None, "no heartbeat for a minute");
    // A beating agent whose cycle stopped (wedged) doesn't page either.
    let mut c = ctx_with(&[("cell1", 2_000, Some(true), &[], &[])]);
    let snap = crate::demo::DemoCluster::new(&crate::inventory::Inventory::example()).snapshot();
    let mut wedged = snap.clone();
    wedged.nodes[0].agent = Some(AgentBeat { last_beat_ms: snap.ts_ms - 1_000, last_cycle_ms: Some(snap.ts_ms - 90_000), config_current: Some(true), ..Default::default() });
    c.agents = PagingCtx::new(&wedged, &RuleSet::defaults()).agents;
    assert_eq!(agent_sends(&mem, &c), None);
    // Rules the agent doesn't evaluate, and switches, stay with the Mac.
    let ok = ctx_with(&[("cell1", 2_000, Some(true), &[], &[])]);
    assert_eq!(agent_sends(&scope("agent-silent", "node", "cell1", Some("cell1"), None), &ok), None);
    assert_eq!(agent_sends(&scope("port-down", "port", "qsfp28-1-1", Some("cell1"), Some("crs804")), &ok), None);
    assert_eq!(agent_sends(&scope("switch-unreachable", "switch", "crs804", None, Some("crs804")), &ok), None);
    // Generic rules on an entity kind the agent judges are its too.
    assert_eq!(agent_sends(&scope("ntp-offset", "node", "cell1", Some("cell1"), None), &ok), Some("cell1".into()));
}

#[test]
fn node_offline_deferred_only_when_reported_missing() {
    let offline = scope("node-offline", "node", "cell3", Some("cell3"), None);
    assert_eq!(agent_sends(&offline, &ctx_with(&[("cell1", 2_000, Some(true), &[], &[])])), None, "a healthy agent that doesn't report cell3 missing: this Mac pages");
    assert_eq!(agent_sends(&offline, &ctx_with(&[("cell1", 2_000, Some(true), &["cell3"], &[])])), Some("cell1".into()));
    assert_eq!(agent_sends(&offline, &ctx_with(&[("cell2", 2_000, Some(false), &["cell3"], &[])])), None, "reported by an agent with old settings");
    assert_eq!(agent_sends(&offline, &ctx_with(&[("cell3", 2_000, Some(true), &["cell3"], &[])])), None, "never by its own agent");
    assert_eq!(agent_sends(&offline, &ctx_with(&[("cell2", 2_000, Some(true), &["cell3"], &[]), ("cell1", 2_000, Some(true), &["cell3"], &[])])), Some("cell1".into()), "the lowest that sees it");
}

#[test]
fn the_fabric_switch_and_a_late_restart_go_to_the_agent_that_watches_them() {
    let sw = scope("switch-unreachable", "switch", "crs804", None, Some("crs804"));
    let mut c = ctx_with(&[("cell1", 2_000, Some(true), &[], &[]), ("cell2", 2_000, Some(true), &[], &[])]);
    assert_eq!(c.fabric_switch.as_deref(), Some("crs804"));
    assert_eq!(agent_sends(&sw, &c), None, "no agent checks it: this Mac pages");
    c.agents.get_mut("cell2").unwrap().switch = true;
    assert_eq!(agent_sends(&sw, &c), Some("cell2".into()), "the leader that checks it");
    assert_eq!(agent_sends(&scope("switch-unreachable", "switch", "firewalla", None, Some("firewalla")), &c), None, "the Firewallas are this Mac's");
    assert_eq!(agent_sends(&scope("port-down", "port", "qsfp56-dd-1-1", None, Some("crs804")), &c), None, "its ports too");
    c.agents.get_mut("cell2").unwrap().current = false;
    assert_eq!(agent_sends(&sw, &c), None, "checking it on old settings");
    // A restart that ran late: the agent that reports the node missing raises node-reboot.
    let late = scope("node-reboot", "node", "cell3", Some("cell3"), None);
    assert_eq!(agent_sends(&late, &ctx_with(&[("cell1", 2_000, Some(true), &[], &[])])), None);
    assert_eq!(agent_sends(&late, &ctx_with(&[("cell1", 2_000, Some(true), &["cell3"], &[])])), Some("cell1".into()));
    // What the agent reports in `status`, as the beat carries it.
    let st = parse_status(&crate::fake_node::inventory(&[("cell2", 2)]).nodes[0], "active\n{\"lastBeatMs\": 1, \"leader\": true, \"switchWatched\": true}\n");
    assert!(st.switch_watched && st.leader == Some(true));
}

#[test]
fn endpoint_rules_deferred_to_probing_head() {
    let down = scope("endpoint-down", "endpoint", "llama-3.3-70b", Some("cell1"), None);
    assert_eq!(agent_sends(&down, &ctx_with(&[("cell1", 2_000, Some(true), &[], &[])])), None, "the head's agent doesn't probe it");
    assert_eq!(agent_sends(&down, &ctx_with(&[("cell1", 2_000, Some(true), &[], &["llama-3.3-70b"])])), Some("cell1".into()));
    assert_eq!(agent_sends(&down, &ctx_with(&[("cell1", 2_000, Some(false), &[], &["llama-3.3-70b"])])), None, "probing with old settings");
    assert_eq!(agent_sends(&down, &ctx_with(&[("cell2", 2_000, Some(true), &[], &["llama-3.3-70b"])])), None, "only the head's agent counts");
    let ttft = scope("endpoint-ttft", "endpoint", "llama-3.3-70b", Some("cell1"), None);
    assert_eq!(agent_sends(&ttft, &ctx_with(&[("cell1", 2_000, Some(true), &[], &["llama-3.3-70b"])])), Some("cell1".into()));
}

fn member(rule: &str, node: &str, sev: Severity) -> Member {
    Member {
        why: Why::Raised,
        rule: rule.into(),
        entity: format!("{rule}:{node}"),
        severity: sev,
        incident: format!("node:{node}"),
        title: format!("{node} {rule}"),
        body: String::new(),
        scope: scope(rule, "node", node, Some(node), None),
        alert_id: None,
        open: None,
    }
}

#[test]
fn mac_only_channel_sent_from_mac() {
    let ctx = ctx_with(&[("cell1", 2_000, Some(true), &[], &[])]);
    let notice = |members: Vec<Member>| crate::notify::Notice { members, ..crate::notify::Notice::plain("x", "", None) };
    let n = notice(vec![member("mem-high", "cell1", Severity::Warn), member("mem-high", "cell3", Severity::Crit)]);
    // A channel the agents have: cell1's alert is its agent's, cell3's (no agent) this Mac's.
    let plan = crate::notify::plan_channel(&n, &phone(&[]), &ctx);
    assert_eq!(plan.agents.iter().map(|(m, by)| (m.scope.node.clone().unwrap(), by.clone())).collect::<Vec<_>>(), [("cell1".to_string(), "cell1".to_string())]);
    let mine = plan.mac.unwrap();
    assert_eq!(mine.members.len(), 1);
    assert_eq!((mine.title.as_str(), mine.message_id.as_str()), ("cell3 mem-high", n.message_id.as_str()), "the same message id");
    // "Send from this Mac only": everything goes from here.
    let plan = crate::notify::plan_channel(&n, &Channel { mac_only: true, ..phone(&[]) }, &ctx);
    assert!(plan.agents.is_empty());
    assert_eq!(plan.mac.unwrap().members.len(), 2);
    // A critical-only channel gets the critical one only.
    let plan = crate::notify::plan_channel(&n, &Channel { critical_only: true, ..phone(&[]) }, &ctx);
    assert_eq!((plan.agents.len(), plan.mac.unwrap().members.len()), (0, 1));
    // And a Mac-only channel is never in what the agents get.
    let s = NotifySettings { channels: vec![Channel { mac_only: true, ..phone(&["token"]) }], ..Default::default() };
    assert!(!notify_json(&s, |_, _| Some("tk".into()), &BTreeMap::new(), None, None).contains("phone"));
}

#[test]
fn the_watch_state_says_what_24_7_rests_on() {
    let now = crate::history::now_ms();
    let a = |node: &str, current: Option<bool>, err: Option<&str>| AgentStatus {
        node: node.into(),
        name: node.into(),
        installed: true,
        state: "running".into(),
        version: Some(version().into()),
        last_beat_ms: Some(now - 2_000),
        config_hash: Some("h".into()),
        expected_hash: Some("h".into()),
        config_current: current,
        push_error: err.map(|m| PushError { at_ms: now, message: m.into(), attempts: 1 }),
        ..Default::default()
    };
    let ws = watch_state(vec![a("cell1", Some(true), None), a("cell2", Some(true), None)], true, now);
    assert_eq!((ws.mode.as_str(), ws.delivery_owner.as_str(), ws.drift.len()), ("agents", "agents", 0));
    let ws = watch_state(vec![a("cell1", Some(true), None), a("cell3", Some(false), Some("sudo: a password is required"))], true, now);
    assert_eq!((ws.mode.as_str(), ws.delivery_owner.as_str()), ("agents", "mac"), "a stale agent: this Mac pages too");
    assert_eq!((ws.drift[0].node.as_str(), ws.drift[0].reason.as_str()), ("cell3", "push failed: sudo: a password is required"));
    let old = AgentStatus { config_hash: None, version: Some("1.0.0".into()), ..a("cell2", Some(false), None) };
    assert_eq!(watch_state(vec![old], true, now).drift[0].reason, "agent 1.0.0 doesn't report its settings: reinstall it");
    // An end-to-end test counts only if every node reached every channel after the last change.
    let tested = |at: u64, ok: bool| TestRun { at_ms: at, results: [("phone".to_string(), TestCell { ok, ..Default::default() })].into(), paging: None };
    let ch = vec![WatchChannel { id: "phone".into(), name: "Phone (ntfy)".into(), remote: true, enabled: true, ..Default::default() }];
    let agents = vec![AgentStatus { last_test: Some(tested(100, true)), ..a("cell1", Some(true), None) }, AgentStatus { last_test: Some(tested(200, true)), ..a("cell2", Some(true), None) }];
    let e = end_to_end(&agents, &ch, 50);
    assert_eq!((e.ok, e.tested_at_ms, e.cells.len()), (true, Some(100), 2));
    assert!(!end_to_end(&agents, &ch, 150).ok, "changed after cell1's test");
    let failed = vec![agents[0].clone(), AgentStatus { last_test: Some(tested(200, false)), ..a("cell2", Some(true), None) }];
    assert!(!end_to_end(&failed, &ch, 50).ok);
    let untested = vec![agents[0].clone(), a("cell2", Some(true), None)];
    assert!(!end_to_end(&untested, &ch, 50).ok);
}

#[test]
fn a_heartbeat_older_than_a_minute_isnt_healthy() {
    let mut snap = crate::demo::DemoCluster::new(&crate::inventory::Inventory::example()).snapshot();
    let now = snap.ts_ms;
    snap.nodes[0].agent = Some(AgentBeat { version: version().into(), last_beat_ms: now - 3_000, ..Default::default() });
    snap.nodes[1].agent = Some(AgentBeat { version: version().into(), last_beat_ms: now - 61_000, ..Default::default() });
    assert_eq!(healthy_agents(&snap), [snap.nodes[0].id.clone()].into());
}

// ---- Demo

fn demo_ws(agents: &demo::DemoAgents, inv: &Inventory, now: u64) -> WatchState {
    let mut ws = watch_state(agents.statuses(inv, now), true, now);
    ws.end_to_end = agents.end_to_end(inv, now);
    ws.channels = vec![agents.channel(now), agents.deadman_channel(now)];
    let d = agents.deadman(now);
    ws.deadman = deadman(&ws.channels, &NotifySettings::default(), d.last_ping_ms, d.by);
    ws
}

#[test]
fn demo_agents_beat_on_every_node_until_agent_offline_stops_cell2() {
    let inv = crate::inventory::Inventory::example();
    let mut cluster = crate::demo::DemoCluster::new(&inv);
    let mut agents = demo::DemoAgents::new(0);
    let mut snap = cluster.snapshot();
    agents.overlay(&mut snap, &[("llama-3.3-70b".into(), "cell1".into())]);
    assert_eq!(demo::beating(&snap).len(), snap.nodes.len(), "all four nodes have a fresh heartbeat");
    let ws = demo_ws(&agents, &inv, snap.ts_ms);
    assert_eq!((ws.mode.as_str(), ws.delivery_owner.as_str()), ("agents", "agents"));
    assert!(ws.agents.iter().all(|a| a.simulated && a.version.as_deref() == Some(version()) && a.config_current == Some(true)));
    assert!(ws.end_to_end.ok && ws.end_to_end.cells.len() == 4, "a simulated test page from every node: {:?}", ws.end_to_end);
    assert_eq!((ws.deadman.configured, ws.deadman.service.as_deref(), ws.deadman.by.as_deref()), (true, Some("healthchecks.io"), Some("cell1")));
    let row = ws.channels.iter().find(|c| c.id == "demo-deadman").unwrap();
    assert_eq!((row.name.as_str(), row.kind.as_str(), row.simulated), ("Dead-man's switch (Demo)", "Dead-man's switch", true), "the checklist's dead-man's switch is a row in the channel list, marked Demo");
    assert_eq!(ws.agents[0].endpoints_probed, ["llama-3.3-70b"]);
    // In a Demo snapshot the agents are healthy and current, so they page for their nodes.
    let ctx = PagingCtx::new(&snap, &RuleSet::defaults());
    assert_eq!(agent_sends(&scope("mem-high", "node", "cell2", Some("cell2"), None), &ctx), Some("cell2".into()));

    cluster.set_scenario(Some("agent-offline")).unwrap();
    let mut snap = cluster.snapshot();
    agents.overlay(&mut snap, &[]);
    for _ in 0..70 {
        cluster.advance();
        let mut s = cluster.snapshot();
        s.ts_ms = snap.ts_ms + 70_000;
        agents.overlay(&mut s, &[]);
        snap = s;
    }
    let beating = demo::beating(&snap);
    assert!(!beating.contains("cell2") && beating.len() == snap.nodes.len() - 1, "{beating:?}");
    let ws = demo_ws(&agents, &inv, snap.ts_ms);
    assert_eq!((ws.mode.as_str(), ws.delivery_owner.as_str()), ("partial", "mac"));
    let cell2 = ws.agents.iter().find(|a| a.node == "cell2").unwrap();
    assert_eq!(cell2.state, "failed");
    assert!(cell2.error.as_deref().unwrap().starts_with("Agent on cell2 not answering"));
    // And the health engine says so after a minute.
    let mut e = crate::health::Engine::default();
    let mut events = Vec::new();
    for i in 0..65 {
        let mut s = snap.clone();
        s.ts_ms += i * 1000;
        events.extend(e.step(&s, s.ts_ms));
    }
    assert!(events.iter().any(|ev| ev.kind == EventKind::Raised && ev.alert.rule == "agent-silent" && ev.alert.entity.node.as_deref() == Some("cell2")), "{events:?}");
}

#[test]
fn a_demo_node_that_goes_offline_takes_its_agent_with_it() {
    let inv = crate::inventory::Inventory::example();
    let mut agents = demo::DemoAgents::new(0);
    let mut snap = crate::demo::DemoCluster::new(&inv).snapshot();
    snap.nodes[2].status = crate::model::LinkStatus::Offline;
    assert!(agents.overlay(&mut snap, &[]), "the change is sent at once");
    let later = snap.ts_ms + 90_000;
    let ws = demo_ws(&agents, &inv, later);
    let cell3 = ws.agents.iter().find(|a| a.node == "cell3").unwrap();
    assert!(!cell3.healthy(later) && cell3.error.as_deref() == Some("cell3 is offline"));
    assert_eq!(ws.mode, "partial");
    // Its peers report it missing, so they page node-offline for it.
    let ctx = PagingCtx::new(&snap, &RuleSet::defaults());
    assert_eq!(agent_sends(&scope("node-offline", "node", "cell3", Some("cell3"), None), &ctx), Some("cell1".into()));
}

#[test]
fn demo_paging_drift_keeps_cell3_on_old_settings_and_this_mac_pages_for_it() {
    let inv = crate::inventory::Inventory::example();
    let mut cluster = crate::demo::DemoCluster::new(&inv);
    let mut agents = demo::DemoAgents::new(0);
    let mut snap = cluster.snapshot();
    agents.overlay(&mut snap, &[]);
    // The scenario id comes from the demo world; the agents react to it.
    snap.scenario = Some("paging-drift".into());
    agents.overlay(&mut snap, &[]);
    let ws = demo_ws(&agents, &inv, snap.ts_ms);
    let cell3 = ws.agents.iter().find(|a| a.node == "cell3").unwrap();
    assert_eq!((cell3.config_current, cell3.push_error.as_ref().map(|e| e.message.as_str())), (Some(false), Some(demo::DRIFT_ERROR)));
    assert_eq!(ws.drift.iter().map(|d| (d.node.as_str(), d.reason.as_str())).collect::<Vec<_>>(), [("cell3", "push failed: sudo: a password is required")]);
    assert_eq!(ws.delivery_owner, "mac");
    let ctx = PagingCtx::new(&snap, &RuleSet::defaults());
    assert_eq!(agent_sends(&scope("mem-high", "node", "cell3", Some("cell3"), None), &ctx), None, "this Mac pages for cell3");
    assert_eq!(agent_sends(&scope("mem-high", "node", "cell1", Some("cell1"), None), &ctx), Some("cell1".into()));
    // Push again fails while the scenario plays, and goes through once it's over.
    assert!(agents.push("cell3", snap.ts_ms + 1).is_err());
    assert_eq!(agents.statuses(&inv, snap.ts_ms)[2].push_error.as_ref().unwrap().attempts, 2);
    snap.scenario = None;
    agents.overlay(&mut snap, &[]);
    assert!(demo_ws(&agents, &inv, snap.ts_ms).drift.is_empty());
}

#[test]
fn a_demo_push_that_fails_on_cell3_fails_cell3s_own_row() {
    let inv = crate::inventory::Inventory::example();
    let mut cluster = crate::demo::DemoCluster::new(&inv);
    let agents = Agents::new(None);
    let mut snap = cluster.snapshot();
    snap.scenario = Some("paging-drift".into());
    agents.demo.lock().unwrap().overlay(&mut snap, &[]);
    let audit = crate::live::audit::Log::new();
    let results: Vec<(String, bool)> = ["cell1", "cell2", "cell3", "gx10-0000"]
        .iter()
        .map(|n| (n.to_string(), commands::simulate_step(&audit, Some(&*agents), n, &format!("Push the settings to {n}"), "sudo sh -c '…'", snap.ts_ms + 1).is_ok()))
        .collect();
    assert_eq!(results.iter().filter(|(_, ok)| *ok).count(), 3);
    let rows = audit.entries();
    let cell3 = rows.iter().find(|e| e.node == "cell3").unwrap();
    assert_eq!((cell3.state.as_str(), cell3.code, cell3.error.as_deref()), ("failed", Some(1), Some(demo::DRIFT_ERROR)), "not \"ok\" under a failed push");
    assert!(rows.iter().filter(|e| e.node != "cell3").all(|e| e.state == "ok"));
    // An install step has no push to fail.
    commands::simulate_step(&audit, None, "cell3", "Install the agent on cell3", "sudo sh -c '…'", snap.ts_ms + 2).unwrap();
}

#[test]
fn demo_channels_changed_this_session_need_a_new_test_page_and_a_silence_doesnt() {
    let inv = crate::inventory::Inventory::example();
    let mut agents = demo::DemoAgents::new(10_000_000);
    let mut snap = crate::demo::DemoCluster::new(&inv).snapshot();
    agents.overlay(&mut snap, &[]);
    let s = settings_with(vec![]);
    agents.note_channels(&s, snap.ts_ms);
    assert!(agents.end_to_end(&inv, snap.ts_ms).ok);
    // A silence, or quiet hours: pushed to the agents, but the channels a test proves are the same.
    assert_eq!(agents.pushed(snap.ts_ms).len(), 4);
    agents.note_channels(&NotifySettings { quiet_start: "21:00".into(), ..s.clone() }, snap.ts_ms);
    assert!(agents.end_to_end(&inv, snap.ts_ms).ok, "still 24/7");
    // A new phone channel: untested until the next test page.
    agents.note_channels(&settings_with(vec![phone(&["token"])]), snap.ts_ms + 1);
    assert!(!agents.end_to_end(&inv, snap.ts_ms + 1).ok, "untested since the channel change");
    agents.test_page(&[crate::notify::demo_channel()], snap.ts_ms + 2);
    let e = agents.end_to_end(&inv, snap.ts_ms + 3);
    assert!(e.ok && e.cells.iter().all(|c| c.ms.is_some()), "{e:?}");
}

#[test]
fn demo_agents_logged_cell2s_restart_while_sparkbox_was_closed() {
    let snap = crate::demo::DemoCluster::new(&crate::inventory::Inventory::example()).snapshot();
    let ev: Vec<AlertEvent> = demo::away_events(&snap);
    assert_eq!(ev.iter().map(|e| e.kind).collect::<Vec<_>>(), [EventKind::Raised, EventKind::Resolved]);
    assert!(ev.iter().all(|e| e.away && e.demo && e.alert.rule == "peer-unreachable"), "tagged Demo");
    assert_eq!(ev[0].alert.summary, format!("{} is unreachable from the other agents", snap.nodes[1].name));
    let gap = ev[1].ts_ms - ev[0].ts_ms;
    assert!(gap < crate::history::REBOOT_GAP_MS && gap > 60_000, "inside the restart ({gap} ms)");
}

#[test]
fn the_as_found_world_has_no_agents_so_paging_is_this_mac_only() {
    // The real cluster has no agent installed (the Live tour, Oct 29): its Demo twin neither.
    let inv = crate::inventory::Inventory::example();
    let mut snap = crate::demo::DemoCluster::as_found().snapshot();
    let mut agents = demo::DemoAgents::new(0);
    agents.overlay(&mut snap, &[]);
    assert!(snap.nodes.iter().all(|n| n.agent.is_none()), "no heartbeat on any node");
    let ws = watch_state(agents.statuses(&inv, snap.ts_ms), true, snap.ts_ms);
    assert_eq!((ws.mode.as_str(), ws.agents.iter().filter(|a| a.installed).count()), ("mac-only", 0));
    assert!(agents.installed().is_empty() && agents.pushed(snap.ts_ms).is_empty(), "nothing to push to");
    let ctx = PagingCtx::new(&snap, &RuleSet::defaults());
    assert_eq!(agent_sends(&scope("mem-high", "node", "cell1", Some("cell1"), None), &ctx), None, "this Mac pages for every node");
}

/// An acknowledgement that ends (or is made) while look-only is on writes nothing on the
/// node: the fake agent takes no request and the ack stays not-taken. Once look-only is off
/// its node is due at once, and it goes out. Maintenance notes are held the same way.
#[tokio::test]
async fn an_ack_under_look_only_sends_nothing_and_goes_out_after() {
    let r = rig("agent-ack-held").await;
    std::fs::create_dir_all(r.layout.requests()).unwrap();
    const KEY: &str = "3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f";
    let agent_side = answer_requests(std::path::PathBuf::from(r.layout.requests()), KEY);
    let (gate, _, _) = crate::admin::testing::gate();
    let agents = Agents::new(None);
    let body = json!({ "rule": "disk-full", "entity": "disk:cell1:/", "incident": "disk:cell1:/", "untilEscalated": true, "ackedMs": 1, "by": "SparkBox on this Mac" }).to_string();
    let acks = vec![(format!("ack-{}", &sha256_hex(body.as_bytes())[..16]), body)];
    let now = crate::history::now_ms();
    agents.ack_tried("cell1", now - 60_000);
    let run = || AckRun { demo: false, source: "system", layout: &r.layout, now };

    gate.set_look_only(true).unwrap();
    assert_eq!(deliver_acks(&r.live, &gate, &agents, &r.inv.nodes, &acks, KEY, run()).await, 0);
    let note = deliver_all(&r.live, &gate, &r.inv.nodes, &r.layout, "maintenance-9-abc", &signed_request(KEY, "maintenance", "{}"), Duration::from_secs(1)).await;
    assert_eq!(note[0].1, Err(HELD_LOOK_ONLY.to_string()));
    assert!(r.node.ran("requests").is_empty(), "nothing written on the node: {:?}", r.node.ran(""));
    assert_eq!(std::fs::read_dir(r.layout.requests()).unwrap().count(), 0, "no signed file");
    assert!(!agents.ack_taken("cell1", &acks[0].0), "kept as not taken");
    assert!(agents.acks_due("cell1", false, now), "due again as soon as look-only ends, not 15 minutes after the last try");

    gate.set_look_only(false).unwrap();
    assert_eq!(deliver_acks(&r.live, &gate, &agents, &r.inv.nodes, &acks, KEY, run()).await, 1);
    assert!(agents.ack_taken("cell1", &acks[0].0));
    assert_eq!(r.node.ran("requests").len(), 1, "one request, after look-only");
    let taken = agent_side.await.unwrap();
    assert_eq!(taken.len(), 1, "the agent took the ack once: {taken:?}");
    assert_eq!(taken[0]["rule"], "disk-full");
    r.live.stop();
}

/// A push SparkBox starts by itself is a root change that copies the owner's secrets as they are at
/// that moment: the release build has no fence to hold it (live/exit.rs), so it waits for Admin
/// mode and stays due. A retry is no exception, since a failed push nobody asked for sets
/// `push_error` as well.
#[test]
fn a_push_sparkbox_starts_by_itself_waits_for_admin_and_stays_due() {
    let now = 10 * 60_000;
    let changed = NodeRecord { installed: true, fingerprint: Some("before".into()), pushed_at_ms: Some(0), ..Default::default() };
    assert_eq!(auto_push_due(&changed, "after", false, false, now), None, "locked: it waits");
    assert_eq!(auto_push_due(&changed, "after", false, true, now), Some(PushReason::Changed), "unlocked: it goes out");
    assert_eq!(auto_push_due(&changed, "after", false, false, now), None, "and it's still due, not dropped");
    assert_eq!(auto_push_due(&changed, "before", false, true, now), None, "nothing changed");

    let failed = NodeRecord { push_error: Some(PushError { at_ms: 0, attempts: 1, ..Default::default() }), ..changed };
    assert_eq!(auto_push_due(&failed, "after", false, false, now), None, "locked: the retry waits");
    assert_eq!(auto_push_due(&failed, "after", true, false, 1_000), None, "on reconnect too");
    assert_eq!(auto_push_due(&failed, "after", false, true, now), Some(PushReason::Retry), "unlocked: it goes out");
    assert_eq!(auto_push_due(&failed, "after", true, true, 1_000), Some(PushReason::Retry), "and at once on reconnect");
    assert_eq!(auto_push_due(&failed, "after", false, true, 1_000), None, "not before its backoff");
}

/// The round asks the gate each time, through `push_due_now`, and has no other way to a push.
#[test]
fn the_agents_round_asks_the_gate_before_a_push_by_itself() {
    let round = include_str!("mod.rs").split("pub fn start(app: &AppHandle) {").nth(1).unwrap();
    assert!(round.contains("push_due_now(&mut back, &node.id, &rec, &fingerprint, gate.state().unlocked, now)"));
    assert_eq!(round.matches("auto_push(").count(), 1, "one push, behind that check");
    assert!(!round.contains("auto_push_due(") && !round.contains("retry_due(") && !round.contains("change_due("));
}

/// A node that came back while Admin was locked keeps its retry: it goes out with the first round
/// Admin is unlocked, not at the end of a backoff that can be 6 hours.
#[test]
fn a_retry_due_because_the_node_is_back_stays_due_until_admin_is_unlocked() {
    let hour = 3_600_000;
    let rec = NodeRecord { installed: true, push_error: Some(PushError { at_ms: 0, attempts: 4, ..Default::default() }), ..Default::default() };
    assert_eq!(backoff_ms(4), 6 * hour, "its backoff has hours to go");
    let mut back: std::collections::HashSet<String> = ["cell1".to_string()].into();
    for round in 1..=3 {
        assert_eq!(push_due_now(&mut back, "cell1", &rec, "fp", false, hour + round * 15_000), None, "locked");
    }
    assert_eq!(push_due_now(&mut back, "cell1", &rec, "fp", true, 2 * hour), Some(PushReason::Retry), "the first unlocked round");
    assert_eq!(push_due_now(&mut back, "cell1", &rec, "fp", true, 2 * hour + 15_000), None, "once: then its backoff again");
    assert!(back.is_empty());
    // A node that came back with nothing to retry is off the list once Admin was unlocked.
    let fine = NodeRecord { installed: true, ..Default::default() };
    back.insert("cell2".into());
    assert_eq!(push_due_now(&mut back, "cell2", &fine, "fp", true, 2 * hour), None);
    assert!(back.is_empty());
}
