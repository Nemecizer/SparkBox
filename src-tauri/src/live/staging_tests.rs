//! Staging tier (PLAN P1.S, §6.4): the command contracts SparkBox's write paths rely on, run on a
//! real Ubuntu 24.04 arm64 node, in the places fake_node can't reach: OpenSSH's MaxSessions, sudo
//! 1.9.15 with `Defaults use_pty`, systemd-run, real apt/dpkg over HTTP, needrestart, kmod. The
//! node is a container `scripts/staging/setup.sh` stands up (its dummy packages:
//! scripts/staging/image/build-repo.sh); these tests ensure it, then log in to its sshd on
//! 127.0.0.1 with SparkBox's own SSH client.
//!
//! They run SparkBox's code as shipped, not copies of it: `ssh::connect_to`, `sudo_exchange`, the
//! root helper's `root_install_command`/`root_sparkjob_command`; the Updates window's command
//! builders (`detached_command_opts`, `apt_command`, `simulate_command`, `cancel_command`,
//! `hold_command`, `PRESET_READ`, `STATUS_SCRIPT`) with their parsers (`parse_status`,
//! `parse_simulation`, `post_checks`, `progress_of`, `exit_of`); the GPU-stack guard
//! (`gpu_guard`); and, over a SparkBox `Live` logged in to the node as it logs in to a Spark,
//! `Ops::updates_upgrade_opts`, `reattach_upgrades`, `updates_hold_preset`, `updates_hold`,
//! `updates_simulate`, `prereboot::check` and the update check's poller round
//! (`UpdateCache::tick` with `CHECK_SCRIPT`/`STAMPS`). Change one and these tests run the changed
//! one. The only node commands built here are the tests' own setup and checks.
//!
//! They are `#[ignore]`d and named `staging_*`, so the normal suite skips them and
//! `cargo test --lib -- --ignored staging` runs them. They need Docker Desktop up (setup.sh starts
//! it). When it can't be reached each test prints a SKIP line (blocked-on-owner) and returns, so
//! "passed" next to SKIP lines means nothing ran; SPARKBOX_STAGING_REQUIRED=1 makes that a failure.
//! The four `staging_session_b_step*` tests are the real-Ubuntu half of Session B steps 19, 21, 22
//! and 24 (docs/acceptance/P1-session-B.md cites them).
//!
//! Round 5's tests run sparkjob.py's jobs (`Live::sparkjob`, `Models::cancel_job`,
//! `Models::library`): a real `rsync --daemon` and a real rsync client, real signals to their
//! process groups, and real Linux file modes. A download's `docker` is the stand-in, which behaves
//! as Docker's CLI and daemon do towards a signal (image/docker-standin.py).
//!
//! P2 round 1's tests run a load's new shapes the same way: C-F through the shipped `deploy-files`
//! under real Python 3.12, C-L2 and C-U2 under real dash into the stand-in, C-P2 on real Ubuntu
//! output, C-J with real ping, and a real rsync left behind by a real `kill -9`. Real Docker's half is
//! scripts/staging/launch-proof.sh, whose two `docker run` lines a plain test here holds to Rust's.
//!
//! One user of the container at a time: each test holds a lock on it, the same one check.sh
//! takes, so the tests run one by one and wait for a check.sh (or another cargo run) to finish.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use russh::client;
use russh::ChannelOpenFailure;

use super::exit::ExecKind;
use super::fence_class::{class_of, upgrade_cancel_unit, Ctx, FenceClass, Shape};
use super::ssh::{self, Session};
use super::{breaker_message, parse_sparkjob, root_install_command, root_sparkjob_command, sh_quote, sha256_hex, sudo_exchange, trust_host_key, wire, Live, ROOT_COPY_STALE, ROOT_LIB, SPARKJOB};
use crate::demo::DemoCluster;
use crate::inventory::{Inventory, NodeConfig};
use crate::model::LinkStatus;
use crate::models::hf;
use crate::models::plan::{self as load_plan, jumbo_failures, jumbo_script, launch_payload, parse_preflight, preflight_script, rank_argv, rank_command, rank_command_for, rank_env, single_command, stop_role_command, unload_command, DeploySpec, Earlyoom, NodeNet, DROP_CACHES};
use crate::models::{download_spec, Models};
use crate::ops::classify::{self, Kind};
use crate::ops::update_check::{self, FirmwareState, UpdateCache};
use crate::ops::updates::{
    apt_command, apt_command_opts, cancel_command, detached_command, detached_command_opts, dpkg_before_path, exit_of, hold_command, parse_simulation, parse_status, post_checks, progress_of, simulate_command, UpgradeArgs,
    UpgradeOpts, UpgradeRun, PRESET_READ, STATUS_SCRIPT,
};
use crate::ops::{gpu_guard, prereboot, Ops};
use crate::tasks::{Task, TaskState, Tasks};
use crate::vpn::Vpn;
use serde_json::{json, Value};

const CONTAINER: &str = "sparkbox-staging-spark1";
/// The node's name in SparkBox's messages (sudo_exchange's refusal names it).
const NODE: &str = "staging";
/// Where SparkBox's detached upgrades log on a real node (`Live::upgrade_log_dir` outside tests).
const LOG_DIR: &str = "/var/log/sparkbox";
const LIMIT: Duration = Duration::from_secs(120);

/// `docker` pinned to Docker Desktop, never the podman machine, DOCKER_HOST cleared (as setup.sh).
fn docker() -> Command {
    let mut c = Command::new("docker");
    c.env_remove("DOCKER_HOST").args(["--context", "desktop-linux"]);
    c
}

fn docker_out(args: &[&str]) -> Result<String, String> {
    let out = docker().args(args).output().map_err(|e| format!("docker {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!("docker {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("docker {}: {e}", args.join(" ")))
}

struct Staging {
    port: u16,
    password: String,
    /// The node's ed25519 host key, as SparkBox pins it.
    fingerprint: String,
    /// A known_hosts.json with the node's host key pinned, as SparkBox keeps one.
    known_hosts: PathBuf,
}

/// Brings the container up (setup.sh) and reads what a test needs to log in: the sshd port, the
/// throwaway password, and the host key to pin. `Ok(None)`: Docker Desktop isn't available.
fn ensure() -> Result<Option<Staging>, String> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let setup = repo.join("scripts/staging/setup.sh");
    let out = Command::new(&setup).current_dir(repo).output().map_err(|e| format!("couldn't run {}: {e}", setup.display()))?;
    match out.status.code() {
        Some(0) => {}
        // setup.sh's "Docker Desktop isn't available".
        Some(2) => return Ok(None),
        _ => return Err(format!("setup.sh failed:\n{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))),
    }
    let mapped = docker_out(&["port", CONTAINER, "22/tcp"])?;
    let first = mapped.lines().next().unwrap_or("").trim();
    let port: u16 = first.rsplit(':').next().and_then(|p| p.parse().ok()).ok_or_else(|| format!("no published port in {first:?}"))?;
    // The password comes back on a pipe, never on a command line or in a log.
    let password = docker_out(&["exec", CONTAINER, "cat", "/root/.sparkbox-staging-password"])?;
    if password.is_empty() {
        return Err("the staging password is empty".into());
    }
    // The key SparkBox's client negotiates with OpenSSH (ed25519), pinned the way a user's
    // "Trust" pins a node's key: a different key is refused.
    let key = docker_out(&["exec", CONTAINER, "ssh-keygen", "-lf", "/etc/ssh/ssh_host_ed25519_key.pub"])?;
    let fingerprint = key.split_whitespace().nth(1).ok_or_else(|| format!("no fingerprint in {key:?}"))?.to_string();
    // Next to the lock files, so teardown.sh removes it with them.
    let known_hosts = std::env::temp_dir().join(format!("{CONTAINER}.known_hosts.json"));
    trust_host_key(&known_hosts, &format!("127.0.0.1:{port}"), &fingerprint)?;
    Ok(Some(Staging { port, password, fingerprint, known_hosts }))
}

/// A line on the console that libtest doesn't capture (it captures `eprintln!`, not a direct
/// write to stderr), so it shows even when the test passes.
fn say(line: &str) {
    let _ = std::io::stderr().write_all(format!("{line}\n").as_bytes());
}

/// The staging node, set up once per test run, or None when Docker Desktop isn't up: the test
/// then says so and returns (SPARKBOX_STAGING_REQUIRED=1: fails instead).
fn staging() -> Option<&'static Staging> {
    static NODE_UP: OnceLock<Result<Option<Staging>, String>> = OnceLock::new();
    match NODE_UP.get_or_init(ensure) {
        Ok(Some(s)) => Some(s),
        Ok(None) => {
            let test = std::thread::current().name().unwrap_or("a staging test").to_string();
            if std::env::var("SPARKBOX_STAGING_REQUIRED").as_deref() == Ok("1") {
                panic!("Docker Desktop isn't up, and SPARKBOX_STAGING_REQUIRED=1: {test} can't run.");
            }
            say(&format!("SKIP (blocked-on-owner): Docker Desktop isn't up, so {test} ran nothing. Start it by hand and re-run."));
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// The upgrade record of task `id` once it reaches `phase` (`Ops::runs`), or panics after `secs`.
async fn run_in_phase(ops: &Ops, id: &str, phase: &str, secs: u64) -> UpgradeRun {
    for _ in 0..secs * 10 {
        if let Some(r) = ops.runs().into_iter().find(|r| r.task_id == id && r.phase == phase) {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the upgrade of task {id} never got to {phase}: {:?}", ops.runs().into_iter().find(|r| r.task_id == id).map(|r| (r.phase, r.log)));
}

/// The container's use lock, held while a test runs: a flock on the file check.sh locks, so a
/// check.sh, another cargo run and the other tests here wait their turn. The kernel drops it when
/// the holder exits, so a killed run leaves nothing behind. A test waits up to 45 minutes: the 28
/// tests of a run queue on it one by one (about 5 minutes in all, more while other builds load
/// this Mac).
fn use_lock() -> File {
    let path = std::env::temp_dir().join(format!("{CONTAINER}.lock"));
    let file = OpenOptions::new().create(true).append(true).open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    let deadline = Instant::now() + Duration::from_secs(45 * 60);
    loop {
        match file.try_lock() {
            Ok(()) => return file,
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => panic!("{CONTAINER} stayed in use (check.sh or another staging run) for 45 minutes: {e:?}"),
        }
    }
}

/// A test's data folder on this Mac (a Live's known hosts, ssh.log, upgrades.json, the evidence
/// copies): `$TMPDIR/sparkbox-<name>-<pid>`, emptied first; teardown.sh removes them. Not
/// `fake_node::temp`: these tests need none of fake_node, nor its sweep of $TMPDIR.
fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sparkbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("make {}: {e}", dir.display()));
    // macOS temp paths go through /var -> /private/var; the real one, as fake_node's.
    dir.canonicalize().unwrap()
}

/// The staging node and its use lock, or return (skipped) when Docker Desktop isn't up.
macro_rules! staging_or_skip {
    () => {
        match staging() {
            Some(s) => (s, use_lock()),
            None => return,
        }
    };
}

/// Logs in the way Live does: SparkBox's client, the pinned host key, the password.
async fn connect(s: &Staging) -> Session {
    let target = ssh::Target { addr: "127.0.0.1".into(), port: s.port, user: "sparkuser".into(), jump: None };
    let login = ssh::Login { password: Some(s.password.clone()), ..Default::default() };
    let c = ssh::connect_to(&target, &login, None, &s.known_hosts, ssh::Trust::Normal).await.unwrap_or_else(|e| panic!("log in to the staging sshd: {e}"));
    assert_eq!(c.login.method, ssh::AuthMethod::Password);
    c
}

/// A command as the cluster user, as `Live::run` sends it (`wire`).
async fn run(c: &Session, cmd: &str) -> ssh::Output {
    c.exec(&wire(cmd), None, LIMIT).await.unwrap_or_else(|e| panic!("`{cmd}`: {e}"))
}

/// A command under sudo, through SparkBox's own exchange; panics if sudo refused the password.
async fn sudo(c: &Session, s: &Staging, cmd: &str, input: Option<&[u8]>) -> ssh::Output {
    sudo_exchange(c, NODE, cmd, &s.password, input, LIMIT).await.unwrap_or_else(|e| panic!("sudo `{cmd}`: {e}"))
}

/// `sudo`, and the command must exit 0.
async fn sudo_ok(c: &Session, s: &Staging, cmd: &str) -> ssh::Output {
    let out = sudo(c, s, cmd, None).await;
    assert_eq!(out.code, Some(0), "`{cmd}` exited {:?}: {}", out.code, out.stderr.trim());
    out
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(String::from).collect()
}

/// The upgrade's log once apt has written its `SPARKBOX-EXIT` line (read the way follow_upgrade
/// reads the end of it), or what's there after 20 s.
async fn finished_log(c: &Session, log: &str) -> Vec<String> {
    finished_log_within(c, log, 20).await
}

/// `finished_log`, waiting up to `secs`.
async fn finished_log_within(c: &Session, log: &str, secs: u64) -> Vec<String> {
    let mut tail = Vec::new();
    for _ in 0..secs * 4 {
        tail = lines(&run(c, &format!("tail -n 20 {}", sh_quote(log))).await.stdout);
        if exit_of(&tail).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    tail
}

/// The dummy repo (scripts/staging/image/build-repo.sh): every package at the version the node
/// has installed is listed in `installed.list`; the newer ones are in the suites' pools.
const REPO: &str = "/srv/sparkbox-repo";
/// The kernel the staging node boots next (/boot/vmlinuz), as on the Sparks in October 2026.
const K: &str = "7.0.0-1019-nvidia";

/// The GPU stack the staging node has, as a Spark names it: what "Pin the GPU stack" must hold
/// (P1.27's pattern), sorted.
const STACK: [&str; 9] = [
    "containerd.io",
    "dgx-spark-ota-update-meta",
    "docker-ce",
    "libnvidia-compute-580",
    "linux-image-7.0.0-1019-nvidia",
    "linux-modules-nvidia-580-open-7.0.0-1019-nvidia",
    "nvidia-container-toolkit",
    "nvidia-driver-580-open",
    "nvidia-spark-grub-kho",
];

/// Restarts the dummy service. needrestart restarts it too, so systemd's start limit (5 in 10 s)
/// is cleared first, or a quick run of tests would leave it failed.
const RESTART_SVC: &str = "systemctl reset-failed sparkbox-dummy-svc; systemctl restart sparkbox-dummy-svc";

/// apt reuses its package cache while dpkg's status file keeps its size and its mtime, which apt
/// compares in whole seconds. A dpkg -i outside apt that swaps a version for one as long (1.1 for
/// 1.0) in the same second as the file's last change keeps both, and apt goes on planning against
/// what was installed before (an upgrade then finds nothing to do). So each dpkg change the tests
/// make themselves drops the cache after it.
const DROP_APT_CACHE: &str = "rm -f /var/cache/apt/pkgcache.bin /var/cache/apt/srcpkgcache.bin";

/// Installs `deb` (a path in the dummy repo) with dpkg itself, as a test's own setup, and drops
/// apt's cache after it.
async fn dpkg_install(c: &Session, s: &Staging, deb: &str) {
    sudo_ok(c, s, &format!("dpkg -i {deb} >/dev/null && {DROP_APT_CACHE}")).await;
}

/// The dummy repo's suites, and where apt keeps what it read of them.
const SUITES: &str = "noble noble-updates noble-security stable";
const LISTS: &str = "/var/lib/apt/lists/127.0.0.1:8080";

/// apt reads the repo again when what it holds isn't the repo as it is: a Release it cached
/// differs from the repo's. A repo put back from its copy has the older files' times, so a plain
/// `apt-get update` asks with If-Modified-Since, is told nothing changed, and keeps what a test
/// published (round 4 found apt still offering sparkbox-bind 3.0 that way): the staging lists go
/// first.
fn apt_lists_as_built() -> String {
    format!(
        "stale=; for s in {SUITES}; do cmp -s {LISTS}_dists_${{s}}_Release {REPO}/dists/$s/Release || stale=1; done; \
         if [ -n \"$stale\" ]; then rm -f {LISTS}_*; apt-get -q update >/dev/null 2>&1; fi"
    )
}

/// The dummy packages that are only in the archive (build-repo.sh): a test installs them, the
/// reset purges them.
const NOT_INSTALLED: &str = "nvidia-driver-590-open nvidia-dkms-580 sparkbox-bind sparkbox-bind-dep sparkbox-swap sparkbox-swap-old sparkbox-rm-docker sparkbox-rm-kernel";

/// Puts every dummy package back at its installed version, nothing held, and clears what an
/// earlier (maybe failed) test left: upgrade units and their logs and package lists, the
/// slow-apt and failing-postinst markers, the packages that are only in the archive
/// (`NOT_INSTALLED`). Reinstalling the kernel and its module package runs their postinsts again:
/// /boot/vmlinuz, the initrd and depmod's index come back. A repo a test published into is put
/// back from /srv/sparkbox-repo.orig and apt reads it again (`apt_lists_as_built`);
/// /etc/default/grub is put back; the container's /proc/sys is read-only again; the docker
/// stand-in ends any container it's running (a killed job's outlives it) and forgets them, its
/// images, its calls and its Hub. apt's cache is dropped (`DROP_APT_CACHE`).
/// The dummy service runs again. Then checks it all took (dpkg's states with dpkg-query: `dpkg
/// --audit` as the cluster user can't report a problem, finding 4), and that apt sees the
/// versions dpkg has.
async fn reset(c: &Session, s: &Staging) {
    // As the cluster user, the owner of the stand-in's files.
    run(c, "docker ps -aq | xargs -r docker rm -f >/dev/null 2>&1").await;
    sudo_ok(c, s, &format!(
        "systemctl stop 'sparkbox-upgrade-*' 2>/dev/null; dpkg --configure -a >/dev/null 2>&1; \
         rm -f /run/sparkbox-staging-slow-apt /run/sparkbox-staging-fail-postinst; \
         apt-mark showhold | xargs -r apt-mark unhold >/dev/null; \
         dpkg -P --force-depends {NOT_INSTALLED} >/dev/null 2>&1; \
         diff -rq {REPO} {REPO}.orig >/dev/null 2>&1 || rsync -a --delete {REPO}.orig/ {REPO}/; {}; \
         dpkg -i $(cat {REPO}/installed.list) >/dev/null 2>&1; {DROP_APT_CACHE}; \
         cmp -s /usr/share/sparkbox-staging/grub /etc/default/grub || cp /usr/share/sparkbox-staging/grub /etc/default/grub; \
         grep -q ' /proc/sys ro,' /proc/self/mountinfo || mount -o remount,bind,ro /proc/sys; \
         rm -f {DOCKER_STATE}/* {DOCKER_STATE}/.lock; rm -rf {DOCKER_STATE}/hub; \
         rm -f {LOG_DIR}/upgrade-*.log {LOG_DIR}/dpkg-before-*.txt; rm -rf {OPS_LIB}; {RESTART_SVC}; true",
        apt_lists_as_built()
    )).await;
    let off = run(c, &format!(
        "names=; for f in $(cat {REPO}/installed.list); do p=$(dpkg-deb -f \"$f\" Package); v=$(dpkg-deb -f \"$f\" Version); names=\"$names $p\"; \
         [ \"$(dpkg-query -W -f='${{Status}} ${{Version}}' \"$p\")\" = \"install ok installed $v\" ] || echo \"$p isn't at $v\"; done; \
         {{ dpkg-query -W -f='D ${{Package}} ${{Version}}\\n' $names; apt-cache policy $names; }} | awk '/^D /{{d[$2] = $3; next}} \
         /^[^ ]/{{p = $1; sub(/:$/, \"\", p)}} /^  Installed: /{{a[p] = $2}} \
         END{{for (p in d) if ((a[p] \"\") != (d[p] \"\")) print \"apt sees \" p \" at \" a[p] \", dpkg has \" d[p]}}'; \
         dpkg-query -W -f='${{db:Status-Abbrev}} ${{Package}}\\n' {NOT_INSTALLED} 2>/dev/null | grep -v '^un ' | sed 's/$/ is still installed/'; \
         apt-mark showhold | sed 's/$/ is still held/'; test -s /boot/initrd.img-{K} || echo 'no initrd'; \
         diff -rq {REPO} {REPO}.orig >/dev/null || echo 'the repo is not as built'; \
         for s in {SUITES}; do cmp -s {LISTS}_dists_${{s}}_Release {REPO}/dists/$s/Release || echo \"apt's lists for $s aren't the repo's\"; done; \
         cmp -s /usr/share/sparkbox-staging/grub /etc/default/grub || echo '/etc/default/grub changed'; \
         grep -q ' /proc/sys ro,' /proc/self/mountinfo || echo '/proc/sys is writable'; \
         dpkg-query -W -f='${{db:Status-Abbrev}} ${{Package}}\\n' | grep -v '^ii \\|^rc ' | sed 's/$/ (not fully installed)/'"
    )).await;
    assert_eq!(off.stdout.trim(), "", "the reset didn't put the node back:\n{}", off.stdout);
}

async fn installed_version(c: &Session, pkg: &str) -> String {
    run(c, &format!("dpkg-query -W -f='${{Version}}' {pkg}")).await.stdout.trim().to_string()
}

/// The pid of the dummy service that keeps sparkbox-dummy-lib loaded.
async fn service_pid(c: &Session) -> String {
    run(c, "systemctl show -p MainPID --value sparkbox-dummy-svc").await.stdout.trim().to_string()
}

/// Restarts the dummy service and waits until it has mapped the library (its python dlopen'd it),
/// so an upgrade replaces a file it already holds. A root process's maps need sudo.
async fn service_holding_lib(c: &Session, s: &Staging) -> String {
    sudo_ok(c, s, RESTART_SVC).await;
    let pid = service_pid(c).await;
    assert!(pid.parse::<i64>().unwrap_or(0) > 0, "the dummy service is running");
    for _ in 0..50 {
        let m = sudo_ok(c, s, &format!("grep -c 'libsparkbox-dummy.so$' /proc/{pid}/maps 2>/dev/null || true")).await;
        if m.stdout.trim().parse::<i32>().unwrap_or(0) > 0 {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the dummy service never mapped the library");
}

/// Upgrades sparkbox-dummy-lib with `cmd` (a detached upgrade under `unit`), returns its log.
async fn upgrade_lib(c: &Session, s: &Staging, unit: &str, cmd: &str, log: &str) -> Vec<String> {
    sudo_ok(c, s, cmd).await;
    let tail = finished_log(c, log).await;
    assert_eq!(exit_of(&tail), Some(0), "the library upgrade ({unit}) succeeded:\n{}", tail.join("\n"));
    tail
}

/// Where an `Ops` on the staging node keeps its root helper, and so its upgrades' logs and
/// package lists (`Live::upgrade_log_dir` is `<root lib>/logs` in tests; /var/log/sparkbox in the
/// app, where the tests that build the command themselves put theirs).
const OPS_LIB: &str = "/var/lib/sparkbox-staging-ops";

/// A SparkBox Live backend logged in to the staging node as it logs in to a Spark: the node `id`
/// at 127.0.0.1, user sparkuser, its host key pinned in `dir`'s known hosts (`dir` is its data
/// folder), the password as the vault would give it (the container's throwaway one), no
/// ~/.ssh/config, no agent. Waits until the node is online (its collector reports). Each test
/// names the node its own way: the upgrade records are per node id and shared by the process.
async fn staging_live(s: &Staging, dir: &Path, id: &str) -> (Arc<Live>, Inventory) {
    let vpn = Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let known = dir.join("known_hosts.json");
    trust_host_key(&known, &format!("127.0.0.1:{}", s.port), &s.fingerprint).unwrap();
    let password = s.password.clone();
    let live = Live::new(vpn, known, move || Ok(Some(password.clone())));
    live.set_ssh_config_path(Some(dir.join("no-ssh-config")));
    live.set_agent_socket(None);
    live.sshlog.set_path(dir.join("ssh.log"));
    live.set_root_lib(OPS_LIB);
    let inventory = Inventory {
        nodes: vec![NodeConfig {
            id: id.into(),
            name: id.into(),
            host: "sparkbox-staging.invalid".into(),
            ip: Some("127.0.0.1".into()),
            model: "MSI EdgeXpert".into(),
            port: s.port,
            user: "sparkuser".into(),
            fabric: None,
        }],
        switches: vec![],
    };
    live.start(&inventory.nodes);
    for _ in 0..600 {
        if live.snapshot(&inventory).nodes[0].status == LinkStatus::Online {
            return (live, inventory);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the staging node never came online in SparkBox: {:?}", live.snapshot(&inventory).nodes[0]);
}

/// SparkBox's operations on that Live, as the app has them (its snapshot is the Live's).
fn staging_ops(live: &Arc<Live>, inventory: &Inventory) -> Arc<Ops> {
    let demo = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
    let ops = Ops::with_poll(live.clone(), demo, Tasks::new(None), Duration::from_millis(200));
    let (l, inv) = (live.clone(), inventory.clone());
    ops.set_snapshot_source(move || Some(l.snapshot(&inv)));
    ops
}

async fn wait_task(ops: &Ops, id: &str, secs: u64) -> Task {
    for _ in 0..secs * 10 {
        let t = ops.tasks().get(id).unwrap();
        if t.finished() {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("still running after {secs} s: {:?}", ops.tasks().get(id));
}

// ── tests ────────────────────────────────────────────────────────────────────────────────────

/// sudo_exchange, no input, right password: the command runs as root, and the password never
/// shows in its output. SparkBox's password-to-the-prompt protocol against real sudo 1.9.15 with
/// `Defaults use_pty`.
#[tokio::test]
#[ignore]
async fn staging_sudo_exchange_without_input() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    let out = sudo_ok(&c, s, "id -u; id -un").await;
    assert_eq!(out.stdout.lines().collect::<Vec<_>>(), ["0", "root"], "uid and user under sudo");
    assert!(!out.stdout.contains(&s.password) && !out.stderr.contains(&s.password), "the password stayed out of the output");
}

/// sudo_exchange with binary input after the password line: the bytes reach the command intact.
/// A wrong password is refused with SparkBox's breaker message, and the command doesn't run.
#[tokio::test]
#[ignore]
async fn staging_sudo_exchange_with_input() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    // 8 KiB covering every byte value: hash what sudo's stdin delivered.
    let blob: Vec<u8> = (0u8..=255).cycle().take(8192).collect();
    let out = sudo(&c, s, "cat | sha256sum | cut -c1-64", Some(&blob)).await;
    assert_eq!(out.code, Some(0), "sudo cat exited {:?}: {}", out.code, out.stderr.trim());
    assert_eq!(out.stdout.trim(), sha256_hex(&blob), "binary stdin survived sudo intact");

    let marker = "/root/sparkbox-staging-wrong-password-ran";
    sudo_ok(&c, s, &format!("rm -f {marker}")).await;
    let refused = sudo_exchange(&c, NODE, &format!("touch {marker}"), "definitely-not-the-password", None, LIMIT).await;
    assert_eq!(refused.err(), Some(breaker_message(NODE)), "a wrong password reads as refused");
    let ran = sudo_ok(&c, s, &format!("test -e {marker} && echo ran || echo not-run")).await;
    assert_eq!(ran.stdout.trim(), "not-run", "nothing ran on the refused password");
}

/// The root helper, as `Live::sparkjob_root` uses it: with no copy the check refuses
/// (ROOT_COPY_STALE); the install writes sparkjob.py root-owned 0644 from stdin; the checked copy
/// runs; a damaged install replaces nothing; a tampered copy exits ROOT_COPY_STALE without running.
#[tokio::test]
#[ignore]
async fn staging_root_helper_install_and_run() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    let sha = sha256_hex(SPARKJOB.as_bytes());
    let file = format!("{ROOT_LIB}/sparkjob.py");
    sudo_ok(&c, s, &format!("rm -rf {}", sh_quote(ROOT_LIB))).await;

    let missing = sudo(&c, s, &root_sparkjob_command(ROOT_LIB, &sha, "df ~"), None).await;
    assert_eq!(missing.code, Some(ROOT_COPY_STALE), "no copy yet: refused before running");

    // The shipped sparkjob.py bytes go in on stdin after the password line.
    let install = sudo(&c, s, &root_install_command(ROOT_LIB, &sha), Some(SPARKJOB.as_bytes())).await;
    assert_eq!(install.failure("Installing sparkjob.py", NODE), None);
    let meta = run(&c, &format!("stat -c '%U %G %a' {} {}", sh_quote(ROOT_LIB), sh_quote(&file))).await;
    assert_eq!(meta.stdout.lines().collect::<Vec<_>>(), ["root root 755", "root root 644"], "root-owned folder 0755, file 0644");

    let df = sudo_ok(&c, s, &root_sparkjob_command(ROOT_LIB, &sha, "df ~")).await;
    let v = parse_sparkjob(NODE, &df).unwrap_or_else(|e| panic!("sparkjob df as root: {e}"));
    assert!(v.get("capacity").is_some(), "sparkjob df ran as root: {v}");

    // Bytes that don't match the checksum: refused, and the good copy is kept.
    let damaged = sudo(&c, s, &root_install_command(ROOT_LIB, &sha), Some(b"print('not sparkjob')\n")).await;
    assert_eq!(damaged.code, Some(1));
    assert!(damaged.stderr.contains("arrived damaged; nothing was replaced"), "{}", damaged.stderr.trim());
    assert_eq!(sudo_ok(&c, s, &format!("sha256sum < {} | cut -c1-64", sh_quote(&file))).await.stdout.trim(), sha, "the good copy is still there");

    sudo_ok(&c, s, &format!("echo tampered >> {}", sh_quote(&file))).await;
    let stale = sudo(&c, s, &root_sparkjob_command(ROOT_LIB, &sha, "df ~"), None).await;
    assert_eq!(stale.code, Some(ROOT_COPY_STALE), "a copy that isn't ours exits ROOT_COPY_STALE");

    sudo_ok(&c, s, &format!("rm -rf {}", sh_quote(ROOT_LIB))).await;
}

/// The detached upgrade and reattach: `detached_command` starts apt as a transient unit; the unit
/// is seen active while it runs (reattach's "still going"), and once it's gone (`--collect`) the
/// log's `SPARKBOX-EXIT 0` and apt's own lines are what's left (reattach's "finished while away").
#[tokio::test]
#[ignore]
async fn staging_systemd_run_upgrade_and_reattach() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    assert_eq!(installed_version(&c, "sparkbox-dummy").await, "1.0");

    let unit = "sparkbox-upgrade-stg-run";
    let log = format!("{LOG_DIR}/upgrade-stg-run.log");
    // sparkbox-dummy 1.1's postinst prints a line a second for 6 s: the unit is active a while.
    let start = sudo_ok(&c, s, &detached_command(unit, LOG_DIR, &log, &["sparkbox-dummy".into()])).await;
    assert!(start.stderr.contains(&format!("Running as unit: {unit}.service")), "systemd-run named the unit: {}", start.stderr.trim());

    // While it runs: the probe follow_upgrade polls reports it active.
    let show = format!("systemctl show -p ActiveState,Result,LoadState -- {}", sh_quote(unit));
    let mut saw_active = false;
    for _ in 0..40 {
        if run(&c, &show).await.stdout.lines().any(|l| l == "ActiveState=active" || l == "ActiveState=activating") {
            saw_active = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(saw_active, "the upgrade unit was seen active while running");

    let tail = finished_log(&c, &log).await;
    assert_eq!(exit_of(&tail), Some(0), "apt reported success in the log:\n{}", tail.join("\n"));
    // The whole log, as the follow reads it: SparkBox's progress parser on real apt output.
    let all = lines(&run(&c, &format!("cat {}", sh_quote(&log))).await.stdout);
    assert_eq!(progress_of(&all), ("finishing", 1, Some("sparkbox-dummy".to_string()), false), "progress of:\n{}", all.join("\n"));
    assert_eq!(installed_version(&c, "sparkbox-dummy").await, "1.1", "the package moved to 1.1");

    // The unit is gone (--collect forgets a finished unit): reattach relies on the log.
    let gone = run(&c, &show).await;
    assert!(gone.stdout.lines().any(|l| l == "LoadState=not-found"), "the collected unit is gone: {}", gone.stdout.trim());

    reset(&c, s).await;
}

/// Cancel while apt is still before dpkg: `cancel_command` TERMs apt-get under the unit's
/// MainPID, the log's exit is non-zero, nothing was installed.
#[tokio::test]
#[ignore]
async fn staging_upgrade_cancel_before_install() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    // apt waits 8 s before it calls dpkg (the image's DPkg::Pre-Invoke hook), so Cancel lands
    // while stopping is still safe.
    sudo_ok(&c, s, "touch /run/sparkbox-staging-slow-apt").await;

    let unit = "sparkbox-upgrade-stg-cancel";
    let log = format!("{LOG_DIR}/upgrade-stg-cancel.log");
    sudo_ok(&c, s, &detached_command(unit, LOG_DIR, &log, &["sparkbox-dummy-lib".into()])).await;

    // Wait until apt-get runs under the unit's shell.
    let under = format!("p=$(systemctl show -p MainPID --value -- {}); [ \"${{p:-0}}\" -gt 0 ] && pgrep -P \"$p\" -x apt-get", sh_quote(unit));
    let mut running = false;
    for _ in 0..40 {
        if run(&c, &under).await.code == Some(0) {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(running, "apt-get started under the upgrade unit");
    let so_far = lines(&run(&c, &format!("cat {}", sh_quote(&log))).await.stdout);
    assert!(progress_of(&so_far).3, "the log says Cancel is still safe:\n{}", so_far.join("\n"));

    // No dpkg yet, so the stop goes through (exit 0), not the dpkg-busy guard (exit 3).
    let cancel = sudo(&c, s, &cancel_command(unit), None).await;
    assert_eq!(cancel.code, Some(0), "cancel exited {:?}: {}", cancel.code, cancel.stderr.trim());

    let tail = finished_log(&c, &log).await;
    assert!(matches!(exit_of(&tail), Some(code) if code != 0), "apt was stopped (non-zero exit):\n{}", tail.join("\n"));
    assert_eq!(installed_version(&c, "sparkbox-dummy-lib").await, "1.0", "nothing was installed");

    reset(&c, s).await;
}

/// The reset puts apt back too, right after a dpkg change apt has read: sparkbox-dummy-lib 1.1
/// installed with dpkg itself (apt's cache left as it is), `apt-get check` reads it, and the
/// reset's dpkg puts 1.0 back, the status file as long and often in the same second. SparkBox's
/// dry run of the library must then plan 1.0 → 1.1 (with the cache left, apt said "already the
/// newest version (1.1)": the Cancel test above had nothing to cancel). Landing in the same second
/// is luck, and a loaded Mac can go twenty rounds without it, so each round makes it so: the status
/// file gets the time it had when apt read it, the one state in which a kept cache would fool apt.
#[tokio::test]
#[ignore]
async fn staging_reset_puts_apt_back_too() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let lib = ["sparkbox-dummy-lib".to_string()];
    let status = "/var/lib/dpkg/status";
    for _ in 0..3 {
        let changed = sudo_ok(&c, s, &format!("dpkg -i {REPO}/pool/noble-updates/sparkbox-dummy-lib_1.1_all.deb >/dev/null && apt-get check >/dev/null && stat -c %Y {status}")).await;
        reset(&c, s).await;
        sudo_ok(&c, s, &format!("touch -d @{} {status}", changed.stdout.trim())).await;
        let dry = run(&c, &simulate_command(&lib)).await;
        assert_eq!(parse_simulation(NODE, &dry.stdout).upgrades, ["sparkbox-dummy-lib 1.0 → 1.1"], "apt plans against what dpkg has:\n{}", dry.stdout);
    }
    say("staging_reset_puts_apt_back_too: 3 resets, each with dpkg's status file as long and as old as apt last read it; apt planned 1.0 → 1.1 after each.");
    reset(&c, s).await;
}

/// Holds: `hold_command` (C4, what `updates_hold` and the GPU-stack pin send) shows in the Updates
/// status, the dry run keeps the package back (SparkBox's parser reads it), a `-y` upgrade naming
/// it is refused (exit 100), and after the unhold the upgrade goes through.
#[tokio::test]
#[ignore]
async fn staging_holds() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let held = "sparkbox-dummy-held";

    sudo_ok(&c, s, &hold_command(&[held.into()], true)).await;
    let status = parse_status(NODE, &run(&c, STATUS_SCRIPT).await.stdout);
    assert_eq!(status.held, [held], "the status lists the hold");
    assert!(status.pending.iter().any(|p| p.name == held && p.held), "the pending package is marked held: {:?}", status.pending);

    let dry = run(&c, &simulate_command(&[])).await;
    assert_eq!(dry.code, Some(0), "the dry run: {}", dry.stderr.trim());
    let sim = parse_simulation(NODE, &dry.stdout);
    assert_eq!(sim.held_back, [held], "the dry run keeps it back:\n{}", dry.stdout);
    assert!(sim.upgrades.iter().any(|u| u == "sparkbox-dummy 1.0 → 1.1"), "and upgrades the rest: {:?}", sim.upgrades);

    let refused = sudo(&c, s, &apt_command(&[held.into()]), None).await;
    assert_eq!(refused.code, Some(100), "apt refuses to change a held package");
    assert!(format!("{}{}", refused.stdout, refused.stderr).contains("Held packages were changed"), "apt said why:\n{}\n{}", refused.stdout, refused.stderr);
    assert_eq!(installed_version(&c, held).await, "1.0", "still held at 1.0");

    sudo_ok(&c, s, &hold_command(&[held.into()], false)).await;
    assert!(run(&c, "apt-mark showhold").await.stdout.trim().is_empty(), "nothing held after unhold");
    sudo_ok(&c, s, &apt_command(&[held.into()])).await;
    assert_eq!(installed_version(&c, held).await, "1.1", "upgraded once released");

    reset(&c, s).await;
}

/// A bare SSH client with no channel budget, to see what OpenSSH itself allows on one connection.
struct AnyKey;
impl client::Handler for AnyKey {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// The channel budget against a real sshd: 10 session channels on one connection, the 11th
/// refused as OpenSSH's MaxSessions does it, and a closed channel's place given back.
#[tokio::test]
#[ignore]
async fn staging_channel_budget_maxsessions() {
    let (s, _lock) = staging_or_skip!();
    let mut handle = client::connect(std::sync::Arc::new(client::Config::default()), ("127.0.0.1", s.port), AnyKey).await.expect("connect to the staging sshd");
    assert!(handle.authenticate_password("sparkuser", &s.password).await.expect("auth").success(), "the staging password was rejected");
    let mut open = Vec::new();
    for _ in 0..10 {
        open.push(handle.channel_open_session().await.expect("ten session channels are allowed"));
    }
    // Real OpenSSH 9.6p1 (what the Sparks run) logs "error: no more sessions" past MaxSessions and
    // sends OPEN_FAILURE reason SSH_OPEN_CONNECT_FAILED (2), russh's `ConnectFailed`. fake_node
    // answers the same, and ssh.rs `open()` backs off on it.
    match handle.channel_open_session().await {
        Err(russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)) => {}
        other => panic!("the 11th channel should be refused with ConnectFailed, got {:?}", other.map(|ch| ch.id())),
    }
    open.pop().unwrap().close().await.unwrap();
    let mut reopened = false;
    for _ in 0..50 {
        if handle.channel_open_session().await.is_ok() {
            reopened = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reopened, "a closed channel's place is given back");
}

/// Check for updates and the Updates status against real apt: "Check all nodes"' refresh (C3,
/// `update_check::REFRESH`, under sudo), then `STATUS_SCRIPT` parsed by `parse_status`.
#[tokio::test]
#[ignore]
async fn staging_updates_status_and_refresh() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let refreshed = sudo_ok(&c, s, update_check::REFRESH).await;
    assert!(refreshed.stdout.contains("http://127.0.0.1:8080 noble-security"), "apt read the repo over HTTP:\n{}", refreshed.stdout);
    let status = parse_status(NODE, &run(&c, STATUS_SCRIPT).await.stdout);
    let dummy = status.pending.iter().find(|p| p.name == "sparkbox-dummy").unwrap_or_else(|| panic!("sparkbox-dummy is pending: {:?}", status.pending));
    assert_eq!((dummy.from.as_str(), dummy.to.as_str()), ("1.0", "1.1"));
    assert!(status.held.is_empty(), "nothing held: {:?}", status.held);
    assert!(status.checked_ms.is_some(), "the last refresh time is read");
}

/// The pre-reboot checks' detail line `id` (`prereboot::Check`).
fn detail(r: &prereboot::PreReboot, id: &str) -> String {
    r.checks.iter().find(|c| c.id == id).map(|c| c.detail.clone()).unwrap_or_else(|| panic!("no {id} check in {:?}", r.checks))
}

/// Session B step 19's real-Ubuntu half (P1.26, contract C6): `prereboot::check`, over SparkBox's
/// own connection, reads which kernel boots next and whether it can come up with its GPU, on a
/// node whose /boot, kernel modules and packages are the dummy GB10 stack. The NVIDIA module is a
/// crafted nvidia.ko (an aarch64 ELF with only a `.modinfo` section, `version=580.95.05`) shipped
/// by the dummy linux-modules-nvidia-580-open-K and indexed by real depmod; modinfo is kmod's own,
/// with no stand-in on PATH. /proc/cmdline is the Docker VM's, so kho=off is recorded as absent.
/// Then each blocking failure on the real node: the initrd removed, the userspace driver upgraded
/// without its module, a DKMS NVIDIA package installed. And the module and userspace moved
/// together match again (depmod and modinfo read the new module).
#[tokio::test]
#[ignore]
async fn staging_session_b_step19_prereboot() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let tools = run(&c, "command -v modinfo; readlink -f \"$(command -v modinfo)\"; dpkg -S /usr/sbin/modinfo").await.stdout;
    assert_eq!(tools.lines().collect::<Vec<_>>(), ["/usr/sbin/modinfo", "/usr/bin/kmod", "kmod: /usr/sbin/modinfo"], "kmod's own modinfo, nothing ahead of it on PATH");
    let ko = format!("/lib/modules/{K}/kernel/nvidia-580-open/nvidia.ko");
    let owner = run(&c, &format!("dpkg -S {ko}; grep -c nvidia.ko /lib/modules/{K}/modules.dep")).await.stdout;
    assert_eq!(owner.lines().collect::<Vec<_>>(), [format!("linux-modules-nvidia-580-open-{K}: {ko}").as_str(), "1"], "the crafted module is the package's, indexed by depmod");
    say(&format!("staging_session_b_step19_prereboot: modinfo is kmod's own, reading a crafted {ko} (an ELF with only a .modinfo section, indexed by depmod); no modinfo stand-in."));

    let dir = data_dir("staging-step19");
    let (live, _) = staging_live(s, &dir, NODE).await;
    let ok = prereboot::check(&live, NODE).await.unwrap();
    assert!(ok.ok, "a healthy staging GB10 passes: {:?}", ok.blocking);
    assert_eq!(ok.kernel.as_deref(), Some(K));
    assert_eq!(detail(&ok, "initrd"), format!("/boot/initrd.img-{K} is there"));
    assert_eq!(detail(&ok, "module"), "580.95.05", "modinfo read the crafted module's version");
    assert_eq!(detail(&ok, "userspace"), "580.95.05-0ubuntu1 (dpkg status ii)", "C6′ reads the status with the version");
    assert_eq!(detail(&ok, "match"), "both 580.95.05");
    assert_eq!(detail(&ok, "dkms"), "none installed");
    assert_eq!(detail(&ok, "modules"), format!("linux-modules-nvidia-580-open-{K} 7.0.0-1019.19; one for {K}: yes; the nvidia-hwe-24.04 metapackage: no"));
    assert_eq!(detail(&ok, "kho"), "kho=off isn't on the kernel command line; nvidia-spark-grub-kho 1.0 is installed", "the Docker VM's command line: recorded only");
    assert_eq!(detail(&ok, "grub"), "GRUB_DEFAULT=0 (a grub-reboot rollback needs GRUB_DEFAULT=saved)");
    // Both reads went as the cluster user, SparkBox's own probes: no sudo, nothing that changes.
    let rows: Vec<_> = live.audit.entries().into_iter().filter(|e| e.command == prereboot::KERNEL || e.command == prereboot::script(K)).collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|e| !e.escalated && !e.changes && e.kind == "command"), "{rows:?}");

    // The September 2026 panic: no initrd for the kernel that boots next.
    sudo_ok(&c, s, &format!("rm /boot/initrd.img-{K}")).await;
    let r = prereboot::check(&live, NODE).await.unwrap();
    assert_eq!(r.blocking, [format!("No initrd for {K}: the September 2026 panic. Not restarted.")]);
    reset(&c, s).await;

    // The userspace driver upgraded, the module for K left at the old version.
    dpkg_install(&c, s, &format!("{REPO}/pool/noble-updates/libnvidia-compute-580_580.105.08-0ubuntu1_all.deb")).await;
    let r = prereboot::check(&live, NODE).await.unwrap();
    assert_eq!(r.blocking, [format!("{K} would boot driver 580.95.05 against userspace 580.105.08. Not restarted.")]);
    // Its module package too: depmod indexes the new nvidia.ko, and they match again.
    dpkg_install(&c, s, &format!("{REPO}/pool/noble-updates/linux-modules-nvidia-580-open-{K}_7.0.0-1019.20_all.deb")).await;
    let r = prereboot::check(&live, NODE).await.unwrap();
    assert!(r.ok, "{:?}", r.blocking);
    assert_eq!(detail(&r, "match"), "both 580.105.08");
    reset(&c, s).await;

    // A DKMS NVIDIA package, as left those kernels without an initrd.
    dpkg_install(&c, s, &format!("{REPO}/pool/extra/nvidia-dkms-580_580.95.05-0ubuntu1_all.deb")).await;
    let r = prereboot::check(&live, NODE).await.unwrap();
    assert_eq!(r.blocking, ["nvidia-dkms-580 is installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted."]);
    reset(&c, s).await;
    live.stop();
}

/// The lines of `text` between `SPARKBOX-AFTER` and `SPARKBOX-AFTER-END`, which must come after
/// apt's own output and before `SPARKBOX-EXIT`.
fn after_section(all: &[String]) -> Vec<String> {
    let at = |m: &str| all.iter().position(|l| l == m).unwrap_or_else(|| panic!("no {m} line in the log:\n{}", all.join("\n")));
    let (after, end) = (at("SPARKBOX-AFTER"), at("SPARKBOX-AFTER-END"));
    let exit = all.iter().position(|l| l.starts_with("SPARKBOX-EXIT ")).expect("an exit line");
    assert!(after < end && end + 1 == exit, "AFTER, then AFTER-END, then the exit line:\n{}", all.join("\n"));
    all[after + 1..end].to_vec()
}

/// P1.24's acceptance test, on round 3's upgrade runner (contract C1) exactly as SparkBox builds
/// it, on real systemd, apt, dpkg and needrestart, sent through `sudo_exchange` under
/// `Defaults use_pty`: the root command opens /var/log/sparkbox (0755) and saves `dpkg-query -W`
/// there before the unit starts, readable by
/// the cluster user; after apt, inside the unit, `apt-get check` and `needrestart -b -r l` write
/// their section of the log, which `post_checks` reads: the dummy service still holding the
/// replaced library is listed and left running (same pid: needrestart only lists).
#[tokio::test]
#[ignore]
async fn staging_shipped_upgrade_and_needrestart() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    // A folder left 0700 (by hand, or an older tool): the command opens it to the cluster user.
    sudo_ok(&c, s, &format!("rm -rf {LOG_DIR} && mkdir -m 0700 {LOG_DIR}")).await;
    let pid = service_holding_lib(&c, s).await;
    let installed = run(&c, "dpkg-query -W").await.stdout;

    let unit = "sparkbox-upgrade-stg-r3";
    let log = format!("{LOG_DIR}/upgrade-stg-r3.log");
    let lib = ["sparkbox-dummy-lib".to_string()];
    let cmd = detached_command_opts(unit, LOG_DIR, &log, &lib, &UpgradeOpts::default());
    assert_eq!(cmd, detached_command(unit, LOG_DIR, &log, &lib), "no options: the plain form");
    let tail = upgrade_lib(&c, s, unit, &cmd, &log).await;

    let before = dpkg_before_path(LOG_DIR, unit);
    assert_eq!(before, "/var/log/sparkbox/dpkg-before-stg-r3.txt");
    let meta = run(&c, &format!("stat -c '%U %a' {LOG_DIR} {}", sh_quote(&before))).await.stdout;
    assert_eq!(meta.lines().collect::<Vec<_>>(), ["root 755", "root 644"], "the folder made 0755, the list root's and world-readable");
    // Read back as the cluster user, as SparkBox reads it for the evidence folder: the list from
    // before apt ran (the library still at 1.0).
    let saved = run(&c, &format!("cat {}", sh_quote(&before))).await;
    assert_eq!(saved.code, Some(0), "sparkuser reads it: {}", saved.stderr.trim());
    assert_eq!(saved.stdout, installed, "the package list from just before the upgrade");
    assert!(saved.stdout.lines().any(|l| l == "sparkbox-dummy-lib\t1.0"), "{}", saved.stdout);

    let all = lines(&run(&c, &format!("cat {}", sh_quote(&log))).await.stdout);
    assert_eq!(exit_of(&tail), Some(0));
    let section = after_section(&all);
    assert!(section.iter().any(|l| l.starts_with("NEEDRESTART-VER: ")), "needrestart ran in batch mode:\n{}", section.join("\n"));
    let (check, nr) = post_checks(&all);
    let check = check.expect("apt-get check's output");
    assert!(check.contains("Reading package lists") && !check.lines().any(|l| l.starts_with("E:")), "apt-get check ran, clean:\n{check}");
    let nr = nr.expect("needrestart's list");
    assert!(nr.services.iter().any(|v| v == "sparkbox-dummy-svc.service"), "the service on the old library is listed: {nr:?}\n{}", section.join("\n"));
    // A container has no kernel of its own: needrestart leaves out its kernel lines.
    assert_eq!((nr.kernel_status, nr.kernel_current.as_deref()), (None, None), "{nr:?}");
    assert_eq!(service_pid(&c).await, pid, "needrestart listed the service and left it running");
    assert_eq!(progress_of(&all), ("finishing", 1, Some("sparkbox-dummy-lib".to_string()), false), "from SPARKBOX-AFTER on it's finishing:\n{}", all.join("\n"));
    say(&format!("staging_shipped_upgrade_and_needrestart: needrestart listed {:?} (kernel lines: none in a container)", nr.services));
    reset(&c, s).await;
}

/// `SPARKBOX-EXIT` keeps apt's own code: a package whose postinst fails makes apt exit 100; the
/// unit still writes its after-section (needrestart and apt-get check run whatever apt said),
/// and the last line says 100.
#[tokio::test]
#[ignore]
async fn staging_r3_runner_keeps_apts_exit_code() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    sudo_ok(&c, s, "touch /run/sparkbox-staging-fail-postinst").await;
    let unit = "sparkbox-upgrade-stg-fail";
    let log = format!("{LOG_DIR}/upgrade-stg-fail.log");
    sudo_ok(&c, s, &detached_command(unit, LOG_DIR, &log, &["sparkbox-dummy-fail".into()])).await;
    let tail = finished_log(&c, &log).await;
    assert_eq!(exit_of(&tail), Some(100), "apt's own code:\n{}", tail.join("\n"));
    let all = lines(&run(&c, &format!("cat {}", sh_quote(&log))).await.stdout);
    assert!(all.iter().any(|l| l.contains("sparkbox-dummy-fail: failing on purpose")), "{}", all.join("\n"));
    assert!(all.iter().any(|l| l.starts_with("E: Sub-process /usr/bin/dpkg returned an error code")), "{}", all.join("\n"));
    let section = after_section(&all);
    assert!(section.iter().any(|l| l.starts_with("NEEDRESTART-VER: ")), "the checks ran after the failure:\n{}", section.join("\n"));
    let (check, nr) = post_checks(&all);
    assert!(check.is_some() && nr.is_some());
    let audit = sudo_ok(&c, s, "dpkg --audit").await.stdout;
    assert!(audit.contains("sparkbox-dummy-fail"), "dpkg --audit, as root, names the half-configured package:\n{audit}");
    // As the cluster user, the way the Updates task runs its audit after an upgrade, dpkg 1.22
    // can't say so: once it finds a problem it checks its lock file (root-only) and fails, with
    // no list. A clean database gives exit 0 and nothing. So the task takes a non-zero exit with
    // nothing listed as "couldn't check" (README.md, finding 4).
    let user = run(&c, "dpkg --audit").await;
    assert_eq!((user.code, user.stdout.as_str()), (Some(2), ""), "{}", user.stderr);
    assert!(user.stderr.contains("unable to check lock file for dpkg database directory /var/lib/dpkg: Permission denied"), "{}", user.stderr);
    say("README finding 4: `dpkg --audit` as the cluster user exits 2 with no list when a package is half-configured; as root it lists it.");
    reset(&c, s).await;
}

/// "Upgrade everything" with the GPU stack pinned, through SparkBox's `Ops`: the dry run (C2, run
/// as the cluster user) is the real apt-get dist-upgrade with -s; it passes the guard and keeps the
/// held stack back. Upgrade binds to that plan: C2′ pins the six routine packages at the versions
/// shown (no dist-upgrade runs any more), and the one root command, C1′, installs exactly those
/// and leaves the stack and the 580 driver where they were.
#[tokio::test]
#[ignore]
async fn staging_upgrade_everything_pinned_with_the_stack_held() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let stack: Vec<String> = STACK.iter().map(|n| n.to_string()).collect();
    sudo_ok(&c, s, &hold_command(&stack, true)).await;
    let node = "staging-everything";
    let dir = data_dir("staging-everything");
    let (live, inventory) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);

    let sim = ops.updates_simulate(false, node, &[]).await.unwrap();
    assert!(sim.dry_run.starts_with("apt-get -s ") && sim.dry_run.ends_with(" -y dist-upgrade"), "{}", sim.dry_run);
    assert_eq!((sim.refused.as_deref(), sim.removes.len()), (None, 0), "with the stack held, the plan passes the guard:\n{}", sim.text);
    let mut kept = sim.held_back.clone();
    kept.sort();
    let held_with_news: Vec<&str> = STACK.iter().copied().filter(|n| *n != "nvidia-spark-grub-kho").collect();
    assert_eq!(kept, held_with_news, "every held package with a newer version is kept back");
    let rest = [
        ("google-chrome-stable", "141.0.7390.65-1", "142.0.7444.59-1"),
        ("libbrotli1", "1.1.0-2build2", "1.1.0-2ubuntu0.24.04.1"),
        ("sparkbox-dummy", "1.0", "1.1"),
        ("sparkbox-dummy-fail", "1.0", "1.1"),
        ("sparkbox-dummy-held", "1.0", "1.1"),
        ("sparkbox-dummy-lib", "1.0", "1.1"),
    ];
    let want: Vec<(String, Option<String>, String, bool)> = rest.iter().map(|(n, f, t)| (n.to_string(), Some(f.to_string()), t.to_string(), false)).collect();
    assert_eq!(plan_of(&sim), want, "the six routine upgrades, nothing new");

    let id = ops.updates_upgrade_opts(false, node.into(), vec![], UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
    let t = wait_task(&ops, &id, 180).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}\n{}", t.error, t.log.join("\n"));
    let run_ = ops.runs().into_iter().find(|r| r.task_id == id).expect("the upgrade's record");
    let pins: Vec<String> = sim.plan.iter().map(|p| format!("{}={}", p.name, p.to)).collect();
    let pins: Vec<&str> = pins.iter().map(String::as_str).collect();
    let started: Vec<_> = live.audit.entries().into_iter().filter(|e| e.escalated).collect();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].command, c1_prime(&run_.unit, &format!("{OPS_LIB}/logs"), &run_.log_path, &pins), "C1′: each routine package pinned, no dist-upgrade");
    assert!(rows(&live, &c2_prime(&pins)).iter().any(|e| !e.escalated && e.code == Some(0)), "C2′ ran as the cluster user");
    let names: Vec<&str> = rest.iter().map(|r| r.0).collect();
    let after = run(&c, &format!("dpkg-query -W {} {}", names.join(" "), STACK.join(" "))).await.stdout;
    let version = |p: &str| after.lines().find_map(|l| l.strip_prefix(&format!("{p}\t"))).unwrap_or("").to_string();
    for (p, _, v) in rest {
        assert_eq!(version(p), v, "{p} upgraded to the version shown");
    }
    for (p, v) in [("nvidia-driver-580-open", "580.95.05-0ubuntu1"), ("libnvidia-compute-580", "580.95.05-0ubuntu1"), (&format!("linux-image-{K}") as &str, "7.0.0-1019.19"), ("dgx-spark-ota-update-meta", "7.6.0"), ("docker-ce", "5:29.0.0-1~ubuntu.24.04~noble")] {
        assert_eq!(version(p), v, "{p} held where it was");
    }
    assert_eq!(installed_version(&c, "nvidia-driver-590-open").await, "", "no 590 driver");
    live.stop();
    reset(&c, s).await;
}

/// The activity log rows of `live` whose command contains `part`.
fn rows(live: &Live, part: &str) -> Vec<crate::live::audit::Entry> {
    live.audit.entries().into_iter().filter(|e| e.command.contains(part)).collect()
}

/// Session B step 21's real-Ubuntu half: SparkBox's own Upgrade of one routine security fix
/// (libbrotli1), through `Ops` on a Live connection to the node, bound to the plan its dry run
/// showed (decision b). The dry run (C2, run as the cluster user) shows libbrotli1's plan and C1′'s
/// apt-get as the command; Upgrade sends that plan back; just before the start the dry run of
/// exactly the pinned command (C2′, as the cluster user) shows the same plan; the one root command
/// is C1′ (`libbrotli1=<version>` pinned, `--no-remove`), which saves the package list first, and
/// SparkBox files a copy in the evidence folder on this Mac; apt runs in its unit; the
/// after-section is read (apt-get check clean, needrestart's list), dpkg --audit is clean, and the
/// downloaded .deb is kept in apt's cache (the rollback copy).
#[tokio::test]
#[ignore]
async fn staging_session_b_step21_upgrade_one() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let node = "staging-step21";
    let dir = data_dir("staging-step21");
    let (live, inventory) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    let pkg = vec!["libbrotli1".to_string()];
    let fix = "libbrotli1=1.1.0-2ubuntu0.24.04.1";

    // What Upgrade's confirm shows: the dry run, the plan, and the command it binds to.
    let sim = ops.updates_simulate(false, node, &pkg).await.unwrap();
    assert_eq!(sim.dry_run, simulate_command(&pkg));
    assert_eq!(sim.command, apt_command_opts(&[fix.into()], &UpgradeOpts::default()), "C1′'s apt-get, pinned to the version shown");
    assert_eq!(sim.upgrades, ["libbrotli1 1.1.0-2build2 → 1.1.0-2ubuntu0.24.04.1"]);
    assert_eq!(plan_of(&sim), [("libbrotli1".into(), Some("1.1.0-2build2".into()), "1.1.0-2ubuntu0.24.04.1".into(), false)]);
    assert_eq!((sim.refused.as_deref(), sim.sensitive.len()), (None, 0));
    let dry_rows = rows(&live, &sim.dry_run);
    assert!(!dry_rows.is_empty() && dry_rows.iter().all(|e| !e.escalated), "the dry run ran as the cluster user: {dry_rows:?}");

    let id = ops.updates_upgrade_opts(false, node.into(), pkg.clone(), UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
    let t = wait_task(&ops, &id, 120).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}\n{:?}\n{}", t.error, t.steps, t.log.join("\n"));
    assert_eq!(t.summary.as_deref(), Some("1 package installed on staging-step21"));
    assert_eq!(installed_version(&c, "libbrotli1").await, "1.1.0-2ubuntu0.24.04.1");

    let run_ = ops.runs().into_iter().find(|r| r.task_id == id).expect("the upgrade's record");
    let log_dir = format!("{OPS_LIB}/logs");
    let before = run_.dpkg_before.clone().expect("the dpkg-before path");
    assert_eq!(before, dpkg_before_path(&log_dir, &run_.unit));
    let on_node = run(&c, &format!("cat {}", sh_quote(&before))).await.stdout;
    assert!(on_node.lines().any(|l| l == "libbrotli1\t1.1.0-2build2"), "the list from before apt ran:\n{on_node}");
    let copy = std::fs::read_to_string(run_.evidence.as_deref().expect("an evidence copy")).unwrap();
    assert!(copy.lines().any(|l| l == "libbrotli1\t1.1.0-2build2") && copy.lines().count() == on_node.lines().count(), "the evidence copy on this Mac");
    assert!(t.log.iter().any(|l| l.starts_with(&format!("Package list before the upgrade: {before} (also in the evidence folder: "))), "{}", t.log.join("\n"));
    let check = run_.apt_check.clone().expect("apt-get check's output");
    assert!(!check.lines().any(|l| l.starts_with("E:")), "{check}");
    assert!(run_.needrestart.is_some(), "needrestart's list was read from the log");

    // C2′ as the cluster user, then one root command, C1′; dpkg --audit afterwards as the cluster
    // user. (The activity log lists newest first; oldest first here.)
    let entries: Vec<_> = live.audit.entries().into_iter().rev().collect();
    let c2 = entries.iter().position(|e| e.command == c2_prime(&[fix])).expect("C2′ ran");
    assert_eq!(sim.command.replacen("env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get ", "apt-get -s ", 1), entries[c2].command, "C2′ is C1′'s apt-get with -s");
    let started: Vec<_> = entries.iter().filter(|e| e.escalated).collect();
    assert_eq!(started.len(), 1, "one sudo command: {:?}", started.iter().map(|e| &e.command).collect::<Vec<_>>());
    assert_eq!(started[0].command, c1_prime(&run_.unit, &log_dir, &run_.log_path, &[fix]), "C1′ byte for byte");
    assert!(c2 < entries.iter().position(|e| e.escalated).unwrap() && !entries[c2].escalated && entries[c2].code == Some(0), "{:?}", entries[c2]);
    let audit = rows(&live, "dpkg --audit");
    assert_eq!(audit.len(), 1, "dpkg --audit ran once: {audit:?}");
    assert!(audit[0].command == "dpkg --audit" && !audit[0].escalated && audit[0].code == Some(0), "{audit:?}");
    // apt kept the .deb (APT::Keep-Downloaded-Packages): what a rollback installs from.
    assert_eq!(run(&c, "ls /var/cache/apt/archives/ | grep '^libbrotli1_'").await.stdout.trim(), "libbrotli1_1.1.0-2ubuntu0.24.04.1_all.deb");
    live.stop();
    reset(&c, s).await;
}

/// The size of apt's partial download of `pkg`, or 0 (apt's partial folder is root's, 0700).
async fn partial_size(c: &Session, s: &Staging, pkg: &str) -> u64 {
    sudo_ok(c, s, &format!("stat -c %s /var/cache/apt/archives/partial/{pkg}_* 2>/dev/null || true")).await.stdout.trim().parse().unwrap_or(0)
}

/// Session B step 22's real-Ubuntu half, the reattach with the download limit: SparkBox upgrades
/// google-chrome-stable (its new package is 20 MiB) from the node's HTTP repo with
/// `UpgradeArgs { dl_limit_kbps: Some(500) }` (apt's Acquire::http::Dl-Limit, so the download
/// takes about 40 s). SparkBox quits while apt downloads (its Live stops, and every connection
/// with it, the follower's tail too); apt goes on in its unit; a new SparkBox on the same data
/// folder picks it up again (`reattach_upgrades`) and follows it to its end.
#[tokio::test]
#[ignore]
async fn staging_session_b_step22_reattach() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    sudo_ok(&c, s, "rm -f /var/cache/apt/archives/google-chrome-stable_* /var/cache/apt/archives/partial/google-chrome-stable_*").await;
    let node = "staging-step22";
    let dir = data_dir("staging-step22");
    let (live, inventory) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    let chrome = vec!["google-chrome-stable".to_string()];
    let started = Instant::now();
    let limited = UpgradeOpts { dl_limit_kbps: Some(500), ..Default::default() };
    let sim = ops.updates_simulate_opts(false, node, &chrome, &limited).await.unwrap();
    let pin = "google-chrome-stable=142.0.7444.59-1".to_string();
    let id = ops.updates_upgrade_opts(false, node.into(), chrome.clone(), UpgradeArgs { dl_limit_kbps: Some(500), security_only: false, plan: Some(sim.shown()) }).unwrap();
    let first = run_in_phase(&ops, &id, "downloading", 60).await;
    assert_eq!(first.dl_limit_kbps, Some(500));
    let start = live.audit.entries().into_iter().find(|e| e.escalated).expect("the start command");
    assert_eq!(start.command, detached_command_opts(&first.unit, &format!("{OPS_LIB}/logs"), &first.log_path, std::slice::from_ref(&pin), &limited));
    assert!(
        start.command.contains("-o Dpkg::Options::=--force-confold -o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500 -y --no-remove install --only-upgrade '\\''google-chrome-stable=142.0.7444.59-1'\\''; code=$?"),
        "C1′ with the limit right after --force-confold: {}",
        start.command
    );
    // Kept for a relaunch.
    let saved = std::fs::read_to_string(dir.join("upgrades.json")).unwrap();
    assert!(saved.contains(&first.unit), "{saved}");

    // SparkBox quits in the middle of the download.
    let mut got = 0;
    for _ in 0..100 {
        got = partial_size(&c, s, "google-chrome-stable").await;
        if got > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(got > 0 && got < 20 << 20, "mid-download ({got} bytes)");
    live.stop();
    // apt goes on by itself in its unit.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let later = partial_size(&c, s, "google-chrome-stable").await;
    assert!(later > got && later < 20 << 20, "still downloading with SparkBox gone ({got} -> {later} bytes)");
    let show = run(&c, &format!("systemctl show -p ActiveState -- {}", sh_quote(&first.unit))).await.stdout;
    assert_eq!(show.trim(), "ActiveState=active");

    // A new SparkBox on the same data folder.
    let (live2, _) = staging_live(s, &dir, node).await;
    let ops2 = staging_ops(&live2, &inventory);
    ops2.reattach_upgrades();
    let mut task = None;
    for _ in 0..100 {
        task = ops2.tasks().list().into_iter().find(|t| t.title == "Upgrade google-chrome-stable on staging-step22 (picked up again)");
        if task.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let t = wait_task(&ops2, &task.expect("the picked-up task").id, 180).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}\n{}", t.error, t.log.join("\n"));
    assert_eq!(t.summary.as_deref(), Some("The upgrade on staging-step22 finished"));
    assert_eq!(installed_version(&c, "google-chrome-stable").await, "142.0.7444.59-1");
    let all = lines(&run(&c, &format!("cat {}", sh_quote(&first.log_path))).await.stdout);
    assert_eq!(exit_of(&all), Some(0));
    // The limit held: apt's own count of the download time ("Fetched 21.0 MB in 41s (510 kB/s)").
    let fetched = all.iter().find(|l| l.starts_with("Fetched ")).cloned().unwrap_or_default();
    let secs: u64 = fetched.split(" in ").nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(secs >= 20, "a 20 MiB download at 500 KB/s takes about 40 s: {fetched:?}");
    assert!(started.elapsed() >= Duration::from_secs(20));
    say(&format!("staging_session_b_step22_reattach: {fetched} (SparkBox quit at {got} bytes, apt went on)"));
    live2.stop();
    reset(&c, s).await;
}

/// The held packages, sorted.
async fn showhold(c: &Session) -> Vec<String> {
    let mut held = lines(&run(c, "apt-mark showhold").await.stdout);
    held.sort();
    held
}

/// Session B step 24's real-Ubuntu half, the holds (C4): "Pin the GPU stack"
/// (`Ops::updates_hold_preset`) reads the installed packages as the cluster user
/// (`PRESET_READ`), takes the GPU-stack ones (`gpu_guard::preset_packages`) and holds exactly
/// those with one `hold_command`; `apt-mark showhold` lists them; a dry run of everything then
/// contains none of them (kept back) and passes the guard; Hold and Release of
/// google-chrome-stable show in showhold and in the Updates status; Release the GPU-stack pin
/// unholds them all.
#[tokio::test]
#[ignore]
async fn staging_session_b_step24_holds() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let node = "staging-step24";
    let dir = data_dir("staging-step24");
    let (live, inventory) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);

    let listed = run(&c, PRESET_READ).await;
    assert_eq!(listed.code, Some(0));
    let mut preset = gpu_guard::preset_packages(&lines(&listed.stdout));
    preset.sort();
    assert_eq!(preset, STACK, "of everything installed, exactly the stack");

    let pin = ops.updates_hold_preset(false, node, true, false).await.unwrap();
    let mut pinned = pin.packages.clone();
    pinned.sort();
    assert_eq!(pinned, STACK);
    let t = ops.tasks().get(pin.task_id.as_deref().unwrap()).unwrap();
    assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("Pinned 9 GPU-stack packages on staging-step24")), "{:?}", t.error);
    let sudo_rows: Vec<_> = live.audit.entries().into_iter().filter(|e| e.escalated).collect();
    assert_eq!(sudo_rows.len(), 1);
    assert_eq!(sudo_rows[0].command, hold_command(&pin.packages, true));
    let read = rows(&live, PRESET_READ);
    assert_eq!(read.len(), 1, "the pin read the installed packages once: {read:?}");
    assert!(!read[0].escalated && read[0].code == Some(0), "the read ran as the cluster user: {read:?}");
    assert_eq!(showhold(&c).await, STACK);

    let sim = ops.updates_simulate(false, node, &[]).await.unwrap();
    assert_eq!((sim.refused.as_deref(), sim.sensitive.len()), (None, 0), "the pinned stack passes the guard: {sim:?}");
    for name in STACK {
        assert!(!sim.upgrades.iter().chain(&sim.installs).chain(&sim.removals).any(|l| l.split_whitespace().next() == Some(name)), "{name} isn't in the plan: {sim:?}");
    }
    let mut kept = sim.held_back.clone();
    kept.sort();
    assert_eq!(kept, STACK.iter().copied().filter(|n| *n != "nvidia-spark-grub-kho").collect::<Vec<_>>(), "kept back");

    let chrome = ["google-chrome-stable".to_string()];
    ops.updates_hold(false, node, &chrome, true).await.unwrap();
    assert!(showhold(&c).await.contains(&chrome[0]));
    let status = parse_status(NODE, &run(&c, STATUS_SCRIPT).await.stdout);
    assert!(status.pending.iter().any(|p| p.name == chrome[0] && p.held), "{:?}", status.pending);
    ops.updates_hold(false, node, &chrome, false).await.unwrap();
    assert_eq!(showhold(&c).await, STACK, "released: the stack's holds are left");

    let release = ops.updates_hold_preset(false, node, false, false).await.unwrap();
    let mut released = release.packages.clone();
    released.sort();
    assert_eq!(released, STACK);
    assert!(showhold(&c).await.is_empty(), "nothing held after Release the GPU-stack pin");
    live.stop();
    reset(&c, s).await;
}

/// P1.25's guards on real apt output. The node's apt lists the kernel's fix from noble-security
/// (marked security and GPU-stack) next to libbrotli1's; "Upgrade security only" picks libbrotli1
/// and never the kernel. With the stack unpinned, the real dry run of everything would move the
/// stack, install the dummy nvidia-driver-590-open (dgx-spark-ota-update-meta 7.7.0 depends on it)
/// and remove nvidia-driver-580-open (590 Conflicts/Replaces it): `check_plan` refuses it, and
/// each rule alone refuses its part of that same plan. SparkBox's own dry run of everything is
/// refused first, the stack not being pinned, and Upgrade everything bound to it stops at C2′ (the
/// 580 removal), before any root command or unit; a selection naming the kernel is refused before
/// even the dry run; security only installs libbrotli1 and leaves the kernel.
#[tokio::test]
#[ignore]
async fn staging_gpu_stack_and_590_guards() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let status = parse_status(NODE, &run(&c, STATUS_SCRIPT).await.stdout);
    let pkg = |n: &str| status.pending.iter().find(|p| p.name == n).cloned().unwrap_or_else(|| panic!("{n} pending: {:?}", status.pending));
    let kernel = pkg(&format!("linux-image-{K}"));
    assert!(kernel.security && kernel.sensitive, "{kernel:?}");
    assert_eq!(kernel.origin, "noble-updates, noble-security");
    let brotli = pkg("libbrotli1");
    assert!(brotli.security && !brotli.sensitive, "{brotli:?}");
    assert_eq!(gpu_guard::security_only(&status.pending), ["libbrotli1"], "never the security kernel");

    let dry = run(&c, &simulate_command(&[])).await;
    assert_eq!(dry.code, Some(0));
    let sim = parse_simulation(NODE, &dry.stdout);
    assert_eq!(sim.removals, ["nvidia-driver-580-open"], "{}", dry.stdout);
    assert!(sim.installs.iter().any(|i| i.starts_with("nvidia-driver-590-open ")), "{:?}", sim.installs);
    assert_eq!(gpu_guard::check_plan(&sim), Err("590 deadlocks CUDA graph capture on GB10: nvidia-driver-590-open".into()));
    let mut no_590 = sim.clone();
    no_590.installs.retain(|i| !i.starts_with("nvidia-driver-59"));
    assert_eq!(gpu_guard::check_plan(&no_590), Err("This plan removes nvidia-driver-580-open (a 580 package)".into()));
    no_590.removals.clear();
    let stack = gpu_guard::check_plan(&no_590).unwrap_err();
    assert!(stack.starts_with("SparkBox can't upgrade GPU-stack packages yet: ") && stack.contains(&format!("linux-image-{K}")), "{stack}");

    let node = "staging-guards";
    let dir = data_dir("staging-guards");
    let (live, inventory) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    // SparkBox's own dry run of everything is refused before the guard (the 590 words are
    // `check_plan`'s, above): the stack isn't pinned. Upgrade without a plan is refused.
    let everything = ops.updates_simulate(false, node, &[]).await.unwrap();
    assert_eq!(everything.refused, Some(format!("Upgrade everything needs the GPU stack pinned first: {} and {} more aren't held on {node}", STACK[..6].join(", "), STACK.len() - 6)));
    assert_eq!(ops.updates_upgrade(false, node.into(), vec![]).unwrap_err().message, crate::ops::updates::DRY_RUN_FIRST);
    // Upgrade everything bound to that plan: C2′ pins it, and apt can't keep 580 without removing
    // it (590 conflicts), which --no-remove stops; the guard reads the removal: refused, nothing
    // as root.
    let id = ops.updates_upgrade_opts(false, node.into(), vec![], UpgradeArgs { plan: Some(everything.shown()), ..Default::default() }).unwrap();
    let t = wait_task(&ops, &id, 60).await;
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("This plan removes nvidia-driver-580-open (a 580 package)")), "{:?}", t.log);
    nothing_started(&c, &live).await;

    let refused = ops.updates_upgrade_opts(false, node.into(), vec![format!("linux-image-{K}")], UpgradeArgs { plan: Some(Vec::new()), ..Default::default() }).unwrap_err();
    assert_eq!(refused.message, format!("SparkBox can't upgrade GPU-stack packages yet: linux-image-{K}"));

    let rows_before = live.audit.entries().len();
    let fixes = ops.updates_simulate(false, node, &["libbrotli1".to_string()]).await.unwrap();
    let id = ops.updates_upgrade_opts(false, node.into(), vec![], UpgradeArgs { dl_limit_kbps: None, security_only: true, plan: Some(fixes.shown()) }).unwrap();
    let t = wait_task(&ops, &id, 120).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}\n{}", t.error, t.log.join("\n"));
    assert_eq!(installed_version(&c, "libbrotli1").await, "1.1.0-2ubuntu0.24.04.1");
    assert_eq!(installed_version(&c, &format!("linux-image-{K}")).await, "7.0.0-1019.19", "the security kernel stayed");
    // The rows since `rows_before` (the activity log lists newest first).
    let all = live.audit.entries();
    let sudo_rows: Vec<_> = all.iter().take(all.len() - rows_before).filter(|e| e.escalated).collect();
    assert_eq!(sudo_rows.len(), 1);
    assert!(sudo_rows[0].command.contains("-y --no-remove install --only-upgrade '\\''libbrotli1=1.1.0-2ubuntu0.24.04.1'\\''; code=$?"), "{}", sudo_rows[0].command);
    live.stop();
    reset(&c, s).await;
}

/// The update check (P1.20, contract C5) on real Ubuntu, through SparkBox's own cache and poller
/// round (`UpdateCache::tick` over a Live connection): `CHECK_SCRIPT` as the cluster user, read
/// by `update_check::parse_check`. The node has no fwupdmgr (firmware unknown, and why), no snap
/// and no Ollama or Docker; /etc/dgx-release is a Spark's (build 7.2.3, OTA 7.6.0) and
/// dgx-spark-ota-update-meta 7.7.0 is pending; the stamps are real `stat` times; the
/// -security pocket is counted. A round with nothing moved reads only `STAMPS`; a dpkg change
/// (libbrotli1 installed by hand, as Session B step 23 does) moves dpkg's stamp, and the next
/// round checks in full again.
#[tokio::test]
#[ignore]
async fn staging_update_check_on_real_ubuntu() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    sudo_ok(&c, s, update_check::REFRESH).await;
    let node = "staging-check";
    let dir = data_dir("staging-check");
    let (live, inventory) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    let cache = UpdateCache::new(None);
    assert_eq!(cache.tick(&ops, false).await, [node], "a node seen for the first time is checked in full");
    let first = cache.summary(node).unwrap();
    assert_eq!(first.error, None);
    let apt = first.apt.clone().unwrap();
    let mut security: Vec<&str> = apt.pending.iter().filter(|p| p.security).map(|p| p.name.as_str()).collect();
    security.sort();
    assert_eq!(security, ["libbrotli1", &format!("linux-image-{K}") as &str]);
    let firmware = first.firmware.clone().unwrap();
    assert_eq!((firmware.state, firmware.detail.as_deref()), (FirmwareState::Unknown, Some("fwupdmgr isn't installed")));
    assert_eq!((first.snaps.as_ref(), first.ollama.as_ref(), first.docker_live_restore), (None, None, None));
    let dgx = first.dgx.clone().unwrap();
    assert_eq!(
        (dgx.build.as_deref(), dgx.ota.as_deref(), dgx.ota_date.as_deref(), dgx.ota_pending.as_deref()),
        (Some("7.2.3"), Some("7.6.0"), Some("Sun Oct  4 11:41:51 EDT 2026"), Some("7.7.0"))
    );
    let stat = |path: &str| {
        let path = path.to_string();
        let c = &c;
        async move { run(c, &format!("stat -c %Y {path}")).await.stdout.trim().parse::<u64>().unwrap() * 1000 }
    };
    assert_eq!(first.stamps.lists_ms, Some(stat("/var/lib/apt/periodic/update-success-stamp").await), "the refresh's stamp");
    assert_eq!(first.stamps.dpkg_ms, Some(stat("/var/lib/dpkg/status").await));
    assert_eq!(first.stamps.reboot_required_ms, None, "no /var/run/reboot-required");
    assert_eq!(first.security_since_ms, first.checked_ms, "security fixes are waiting since this check");
    let facts = first.facts().unwrap();
    assert_eq!((facts.pending as usize, facts.security, facts.ota_pending.as_deref(), facts.firmware), (apt.pending.len(), 2, Some("7.7.0"), None));

    assert!(cache.tick(&ops, false).await.is_empty(), "nothing moved: no full check");
    assert_eq!(rows(&live, update_check::CHECK_SCRIPT).len(), 1);
    assert!(!rows(&live, update_check::STAMPS).is_empty(), "the round read the stamps");

    // dpkg's status file changes (mtime has whole seconds): the next round checks in full.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    dpkg_install(&c, s, &format!("{REPO}/pool/noble-security/libbrotli1_1.1.0-2ubuntu0.24.04.1_all.deb")).await;
    assert_eq!(cache.tick(&ops, false).await, [node]);
    let second = cache.summary(node).unwrap();
    assert!(second.stamps.dpkg_ms > first.stamps.dpkg_ms, "{:?} -> {:?}", first.stamps, second.stamps);
    assert_eq!(second.stamps.dpkg_ms, Some(stat("/var/lib/dpkg/status").await));
    let still: Vec<String> = second.apt.unwrap().pending.iter().filter(|p| p.security).map(|p| p.name.clone()).collect();
    assert_eq!(still, [format!("linux-image-{K}")], "libbrotli1's fix is gone from the list");
    assert_eq!(second.security_since_ms, first.security_since_ms, "the kernel's fix still waits since the first check");
    assert!(live.audit.entries().iter().all(|e| !e.escalated && !e.changes), "the check is reads only");
    live.stop();
    reset(&c, s).await;
}

// ── round 4: shell conformance (T2 blocker 2's real proof) ──────────────────────────────────

/// What a conformance row must be.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Must {
    /// A command hidden in shell syntax: real dash or real bash runs it (else the row proves
    /// nothing), and SparkBox calls it neither a read nor any class but `other`.
    Hidden,
    /// A read SparkBox sends, or a positive control: Read to both parsers.
    Read,
    /// One of this round's change literals: its own class under the way Live sends it. Not run.
    Class(ExecKind, FenceClass),
    /// C-X: no shape (its `if … fi` and `${p:-0}` are syntax the shape parser refuses), so `other`
    /// to it; `fence_class::upgrade_cancel_unit` recognises its exact text, and its call site's
    /// `as_class(AptOne)` makes it apt-one. Not run.
    Cancel(&'static str),
}

struct Row {
    form: &'static str,
    cmd: String,
    must: Must,
    /// The test's own setup for the row (a planted file), as the cluster user, before each shell
    /// runs it; `N` is the row.
    plant: Option<&'static str>,
    /// The hidden command deletes /tmp/sbconf-N (a folder `plant` makes) rather than making it:
    /// it ran when the folder is gone.
    removes: bool,
}

fn hidden(form: &'static str, cmd: &str) -> Row {
    Row { form, cmd: cmd.into(), must: Must::Hidden, plant: None, removes: false }
}

fn read_row(form: &'static str, cmd: &str) -> Row {
    Row { form, cmd: cmd.into(), must: Must::Read, plant: None, removes: false }
}

/// A folder of files, as ~/models is, for a hidden `rm -rf` to delete.
const PLANT_FOLDER: &str = "mkdir -p /tmp/sbconf-N/models/m && printf x > /tmp/sbconf-N/models/m/weights";

/// A command hiding `rm -rf /tmp/sbconf-N`, the folder planted before each shell runs it.
fn removing(form: &'static str, cmd: &str) -> Row {
    Row { form, cmd: cmd.into(), must: Must::Hidden, plant: Some(PLANT_FOLDER), removes: true }
}

/// The table: R4-SAFE's fail-closed constructs and command-word rules, each hiding `touch
/// /tmp/sbconf-N` in its syntax; the round 4 acceptance tester's forms (a wrapper's option read
/// as the program reads it, watch re-joining its words, bash's `>&word`, a `${a[x]}` subscript),
/// the wrappers' again hiding `rm -rf` of a planted folder; the reads SparkBox sends and the
/// positive controls; this round's change literals (their class) and those literals with a hidden
/// command added.
fn conformance_rows() -> Vec<Row> {
    let mut rows = vec![
        // The command word is a substitution, a variable, a glob, a brace or a path SparkBox can't vouch for.
        hidden("substitution as the command word", "$(echo touch) /tmp/sbconf-N"),
        hidden("backtick as the command word", "`echo touch` /tmp/sbconf-N"),
        hidden("variable as the command word", "X=touch; $X /tmp/sbconf-N"),
        hidden("${X:-…} as the command word", "${X:-touch} /tmp/sbconf-N"),
        hidden("glob in the command word", "/usr/bin/tou?h /tmp/sbconf-N"),
        hidden("assignment with a substitution", "X=$(touch /tmp/sbconf-N) true"),
        Row { form: "a ~ path named cat", cmd: "~/sbconf-bin-N/cat /etc/hostname".into(), must: Must::Hidden, plant: Some("mkdir -p ~/sbconf-bin-N && printf '#!/bin/sh\\ntouch /tmp/sbconf-N\\n' > ~/sbconf-bin-N/cat && chmod +x ~/sbconf-bin-N/cat"), removes: false },
        Row { form: "a /tmp path named cat", cmd: "/tmp/sbconf-bin-N/cat /etc/hostname".into(), must: Must::Hidden, plant: Some("mkdir -p /tmp/sbconf-bin-N && printf '#!/bin/sh\\ntouch /tmp/sbconf-N\\n' > /tmp/sbconf-bin-N/cat && chmod +x /tmp/sbconf-bin-N/cat"), removes: false },
        Row { form: "$X/cat", cmd: "X=/tmp/sbconf-bin-N; $X/cat /etc/hostname".into(), must: Must::Hidden, plant: Some("mkdir -p /tmp/sbconf-bin-N && printf '#!/bin/sh\\ntouch /tmp/sbconf-N\\n' > /tmp/sbconf-bin-N/cat && chmod +x /tmp/sbconf-bin-N/cat"), removes: false },
        // Arithmetic and array subscripts.
        hidden("$((a[$(…)]))", "echo $((a[$(touch /tmp/sbconf-N)]))"),
        hidden("$((x)) re-evaluating a[$(…)]", "x='a[$(touch /tmp/sbconf-N)]'; echo $((x))"),
        hidden("$[x] re-evaluating a[$(…)]", "x='a[$(touch /tmp/sbconf-N)]'; echo $[x]"),
        hidden("[[ -eq ]] re-evaluating a[$(…)]", "[[ 1 -eq 'a[$(touch /tmp/sbconf-N)]' ]]"),
        hidden("test -v 'a[$(…)]'", "test -v 'a[$(touch /tmp/sbconf-N)]'"),
        hidden("printf -v 'a[$(…)]'", "printf -v 'a[$(touch /tmp/sbconf-N)]' x"),
        hidden("read 'a[$(…)]'", "read 'a[$(touch /tmp/sbconf-N)]' </dev/null"),
        hidden("declare -i re-evaluating a[$(…)]", "declare -i x='a[$(touch /tmp/sbconf-N)]'"),
        hidden("let 'a[$(…)]'", "let 'a[$(touch /tmp/sbconf-N)]'"),
        hidden("${x:=…} then ${a[x]}", "echo ${x:='a[$(touch /tmp/sbconf-N)]'} ${a[x]}"),
        // Quoting forms dash and bash read differently.
        hidden("brace expansion", "{touch,/tmp/sbconf-N}"),
        hidden("$'\\x74ouch'", "$'\\x74ouch' /tmp/sbconf-N"),
        hidden("$\"…\"", "$\"touch\" /tmp/sbconf-N"),
        // Heredocs, here-strings, process substitution.
        hidden("a heredoc with a substitution", "cat <<EOF\n$(touch /tmp/sbconf-N)\nEOF"),
        hidden("a <<- heredoc ending early", "cat <<-EOF\n\tEOF\ntouch /tmp/sbconf-N\nEOF"),
        hidden("a here-string", "cat <<< $(touch /tmp/sbconf-N)"),
        hidden("<(…)", "cat <(touch /tmp/sbconf-N)"),
        hidden(">(…)", "uptime > >(touch /tmp/sbconf-N)"),
        hidden(". with a heredoc", ". /dev/stdin <<EOF\ntouch /tmp/sbconf-N\nEOF"),
        // Builtins that run or redefine commands.
        hidden("an alias across a newline", "alias uptime='touch /tmp/sbconf-N'\nuptime"),
        hidden("hash -p", "hash -p /usr/bin/touch cat; cat /tmp/sbconf-N"),
        hidden("trap", "trap 'touch /tmp/sbconf-N' EXIT; uptime"),
        hidden("eval", "eval 'touch /tmp/sbconf-N'"),
        hidden("export -f", "f() { touch /tmp/sbconf-N; }; export -f f; bash -c f"),
        Row { form: "ENV= sh -i", cmd: "ENV=/tmp/sbconf-env-N sh -i -c true </dev/null".into(), must: Must::Hidden, plant: Some("printf 'touch /tmp/sbconf-N\\n' > /tmp/sbconf-env-N"), removes: false },
        // IFS.
        hidden("${IFS} in the command word", "touch${IFS}/tmp/sbconf-N"),
        hidden("IFS set, then $x", "IFS=,; x=touch,/tmp/sbconf-N; $x"),
        // Whitespace and control characters SparkBox's parsers might split on and the shells don't.
        hidden("NBSP before a #", "uptime\u{a0}#;touch /tmp/sbconf-N"),
        hidden("U+2028 before a #", "uptime\u{2028}#;touch /tmp/sbconf-N"),
        hidden("CR before a #", "uptime\r#;touch /tmp/sbconf-N"),
        hidden("VT before a #", "uptime\u{0b}#;touch /tmp/sbconf-N"),
        hidden("a comment ending in a backslash", "uptime # see \\\ntouch /tmp/sbconf-N"),
        // Redirections the shape parser doesn't model.
        hidden("<> opens for writing", "cat <> /tmp/sbconf-N"),
        hidden(">|", "uptime >| /tmp/sbconf-N"),
        hidden("&>", "uptime &> /tmp/sbconf-N"),
        hidden("|&", "uptime |& touch /tmp/sbconf-N"),
        hidden(">&word", "uptime >&/tmp/sbconf-N"),
        // Reserved words and grouping.
        hidden("( … )", "(touch /tmp/sbconf-N)"),
        hidden("{ …; }", "{ touch /tmp/sbconf-N; }"),
        hidden("if", "if true; then touch /tmp/sbconf-N; fi"),
        hidden("for", "for f in /tmp/sbconf-N; do touch \"$f\"; done"),
        hidden("while", "while touch /tmp/sbconf-N; do break; done"),
        hidden("case … ;;", "case a in a) touch /tmp/sbconf-N;; esac"),
        hidden("case … ;&", "case a in a) true;& b) touch /tmp/sbconf-N;; esac"),
        hidden("case … ;;&", "case a in a) true;;& a) touch /tmp/sbconf-N;; esac"),
        hidden("!", "! touch /tmp/sbconf-N"),
        hidden("time", "time touch /tmp/sbconf-N"),
        hidden("coproc", "coproc touch /tmp/sbconf-N"),
        hidden("function", "function uptime { touch /tmp/sbconf-N; }; uptime"),
        hidden("select", "select f in /tmp/sbconf-N; do touch \"$f\"; break; done <<< 1"),
        // Wrappers: an option read as a value it isn't (or the other way round) makes the next
        // word, cat, look like the command; watch joins its words and hands them to sh -c.
        hidden("watch re-joins its words", "TERM=xterm timeout 3 watch -n 1 uptime ';' touch /tmp/sbconf-N"),
        hidden("watch, a quoted $(…)", "TERM=xterm timeout 3 watch -n 1 echo '$(touch /tmp/sbconf-N)'"),
        hidden("watch -d is a flag", "TERM=xterm timeout 3 watch -d touch cat /tmp/sbconf-N"),
        hidden("xargs -i's value is optional", "echo x | xargs -i touch cat /tmp/sbconf-N"),
        hidden("xargs --replace's value is optional", "echo x | xargs --replace touch cat /tmp/sbconf-N"),
        hidden("taskset -c is a flag", "taskset -c 0 touch cat /tmp/sbconf-N"),
        hidden("chrt -i is a flag", "chrt -i 0 touch cat /tmp/sbconf-N"),
        hidden("command -p is a flag", "command -p touch cat /tmp/sbconf-N"),
        hidden("exec -c is a flag (bash)", "exec -c touch cat /tmp/sbconf-N"),
        hidden("exec -a takes a name (bash)", "exec -a cat touch /tmp/sbconf-N"),
        // The same wrappers deleting a folder of files (rm takes -rf after its operands).
        removing("watch re-joins its words, rm -rf", "TERM=xterm timeout 3 watch -n 1 ls ';' rm -rf /tmp/sbconf-N"),
        removing("watch -d, rm cat -rf", "TERM=xterm timeout 3 watch -d rm cat -rf /tmp/sbconf-N"),
        removing("xargs -i, rm cat -rf", "echo x | xargs -i rm cat -rf /tmp/sbconf-N"),
        removing("xargs --replace, rm cat -rf", "echo x | xargs --replace rm cat -rf /tmp/sbconf-N"),
        removing("taskset -c, rm cat -rf", "taskset -c 0 rm cat -rf /tmp/sbconf-N"),
        removing("chrt -i, rm cat -rf", "chrt -i 0 rm cat -rf /tmp/sbconf-N"),
        removing("command -p, rm cat -rf", "command -p rm cat -rf /tmp/sbconf-N"),
        removing("exec -c, rm cat -rf (bash)", "exec -c rm cat -rf /tmp/sbconf-N"),
        removing("exec -a cat, rm -rf (bash)", "exec -a cat rm -rf /tmp/sbconf-N"),
        // Positive controls (R4-SAFE's): they stay reads.
        read_row("uptime", "uptime"),
        read_row("cat /etc/hostname", "cat /etc/hostname"),
        read_row("/usr/bin/cat /etc/hostname", "/usr/bin/cat /etc/hostname"),
        read_row("~ in an argument", "ls -la ~/models"),
        read_row("an expansion in an argument", "echo \"$HOME\""),
        read_row("grep -E with ( | )", "grep -E '^(a|b)' /proc/meminfo"),
        read_row("ls -la", "ls -la"),
        // The reads SparkBox sends.
        read_row("C5 the update check", update_check::CHECK_SCRIPT),
        read_row("C5 the stamps", update_check::STAMPS),
        read_row("C6′ the checks before a restart, step B", &prereboot::script(K)),
        read_row("C-SH the shell facts", crate::evidence::STEPS.iter().find(|(name, _)| *name == "shell").map(|(_, cmd)| *cmd).expect("the evidence capture's shell step")),
        read_row("C-P2 the load's pre-flight", &preflight_script(&first_light_spec())),
        read_row("C-P2 on two nodes", &preflight_script(&multi_spec())),
        read_row("C-J the jumbo-frame path", &jumbo_script(&[("enp1s0f1np1".into(), "192.168.100.1".into()), ("enP2p1s0f1np1".into(), "192.168.101.1".into())])),
    ];
    let spec = first_light_spec();
    let several = multi_spec();
    // This round's change literals keep their class.
    rows.extend([
        Row { form: "C-L2 the single-node load", cmd: single_command(&spec), must: Must::Class(ExecKind::Run, FenceClass::ModelLoadOne), plant: None, removes: false },
        Row { form: "C-L2 the head", cmd: rank_command(&several, 0), must: Must::Class(ExecKind::Run, FenceClass::ModelLoad), plant: None, removes: false },
        Row { form: "C-L2 a worker", cmd: rank_command(&several, 1), must: Must::Class(ExecKind::Run, FenceClass::ModelLoad), plant: None, removes: false },
        Row { form: "C-F one node's files", cmd: "deploy-files d1 single".into(), must: Must::Class(ExecKind::Sparkjob, FenceClass::ModelLoadOne), plant: None, removes: false },
        Row { form: "C-F the head's files", cmd: "deploy-files d1 head".into(), must: Must::Class(ExecKind::Sparkjob, FenceClass::ModelLoad), plant: None, removes: false },
        Row { form: "C-F a worker's files", cmd: "deploy-files d1 worker".into(), must: Must::Class(ExecKind::Sparkjob, FenceClass::ModelLoad), plant: None, removes: false },
        Row { form: "C-U2 the unload", cmd: unload_command("d1"), must: Must::Class(ExecKind::Run, FenceClass::ModelUnload), plant: None, removes: false },
        Row { form: "C-S stop one role", cmd: stop_role_command("d1", "head"), must: Must::Class(ExecKind::Run, FenceClass::ModelUnload), plant: None, removes: false },
        Row { form: "C-D the page-cache drop", cmd: DROP_CACHES.into(), must: Must::Class(ExecKind::Sudo, FenceClass::DropCaches), plant: None, removes: false },
        Row { form: "C-X Cancel", cmd: cancel_command("sparkbox-upgrade-t1"), must: Must::Cancel("sparkbox-upgrade-t1"), plant: None, removes: false },
    ]);
    // The same literals with a command hidden after them.
    rows.extend([
        hidden("C-L2, then a substitution as a command word", &format!("{}\n$(echo touch) /tmp/sbconf-N", single_command(&spec))),
        hidden("C-L2 a worker, then a substitution", &format!("{}\n$(echo touch) /tmp/sbconf-N", rank_command(&several, 1))),
        hidden("C-U2, then brace expansion", &format!("{}; {{touch,/tmp/sbconf-N}}", unload_command("d1"))),
        hidden("C-P2, then a <<- heredoc", &format!("{}; cat <<-EOF\n\tEOF\ntouch /tmp/sbconf-N\nEOF", preflight_script(&spec))),
        hidden("C-SH, then an alias", &format!("{}\nalias uptime='touch /tmp/sbconf-N'\nuptime", crate::evidence::STEPS.iter().find(|(name, _)| *name == "shell").unwrap().1)),
        hidden("C6′, then ${IFS}", &format!("{}; touch${{IFS}}/tmp/sbconf-N", prereboot::script(K))),
        hidden("C-X, then a substitution as a command word", &format!("{}; $(echo touch) /tmp/sbconf-N", cancel_command("sparkbox-upgrade-t1"))),
    ]);
    // Round 4's fix phase (R4-SAFE): bash evaluating a value read in, and a quote that spans two heredocs.
    rows.extend([
        hidden("[[ on a value read in (bash)", "echo 'a[$(touch /tmp/sbconf-N)]' | { read y; [[ $y -eq 0 ]]; }"),
        hidden("${x[y]} on a value read in (bash)", "echo 'a[$(touch /tmp/sbconf-N)]' | { read y; x=1; echo ${x[y]}; }"),
        hidden("${!y} (bash)", "echo 'a[$(touch /tmp/sbconf-N)]' | { read y; echo ${!y}; }"),
        hidden("${y@P} (bash)", "echo '$(touch /tmp/sbconf-N)' | { read y; echo ${y@P}; }"),
        hidden("test -v on a value (bash)", "echo 'a[$(touch /tmp/sbconf-N)]' | { read y; test -v \"$y\"; }"),
        hidden("two heredocs, odd quote", "cat <<A\n'\nA\ntouch /tmp/sbconf-N\ncat <<B\n'\nB"),
    ]);
    rows
}

/// The contracts' model folder, image and local id (C-L2).
const MODEL: &str = "/home/sparkuser/models/google/gemma-3-27b-it";
const IMAGE_REF: &str = "vllm/vllm-openai:glm53-flash-arm64-cu130";

fn image_id() -> String {
    format!("sha256:{}", "a".repeat(64))
}

/// The first-light load of contract C-L: gemma-3-27b-it on cell2 from the local image id.
fn first_light_spec() -> DeploySpec {
    DeploySpec {
        id: "d1".into(),
        served_name: "gemma-3-27b-it".into(),
        model_path: MODEL.into(),
        nodes: vec!["cell2".into()],
        host: "192.168.50.154".into(),
        port: 8000,
        gpu_memory_utilization: 0.75,
        max_model_len: Some(32768),
        image: IMAGE_REF.into(),
        image_ref: IMAGE_REF.into(),
        image_id: image_id(),
        ..Default::default()
    }
}

/// The same load on two nodes: rank 0 is the head, rank 1 a worker, with the owner's first-run
/// flags (C-L2's `--privileged`).
fn multi_spec() -> DeploySpec {
    DeploySpec { nodes: vec!["cell1".into(), "cell2".into()], tensor_parallel: 2, docker_access: "privileged".into(), recipe: "gemma-3-27b-it".into(), ..first_light_spec() }
}

/// Each rank's fabric identity, as `pick_fabric` makes it from a node's own `sparkjob.py check`.
fn multi_nets() -> Vec<NodeNet> {
    [("cell1", "192.168.100.1"), ("cell2", "192.168.100.2")]
        .map(|(node, ip)| NodeNet { node_id: node.into(), iface: "enp1s0f1np1".into(), ip: ip.into(), hcas: vec!["rocep1s0f1".into(), "roceP2p1s0f1".into()] })
        .to_vec()
}

/// docker's argv for a rank of C-L2 under real dash, word for word. `$HOME` is the cluster user's,
/// and the options the rank has or hasn't are the arguments.
struct Words<'a> {
    name: &'a str,
    role: &'a str,
    rank: usize,
    recipe: &'a str,
    parallel: &'a str,
    listen: bool,
    privileged: bool,
    key: bool,
}

fn c_l2_words(w: &Words) -> Vec<String> {
    let instance = crate::identity::instance_name();
    let mut v: Vec<String> = ["run", "-d", "--name", w.name].map(String::from).to_vec();
    let mut labels = vec![
        format!("sparkbox.instance={instance}"),
        "sparkbox.deployment=d1".to_string(),
        "sparkbox.group=d1".to_string(),
        format!("sparkbox.role={}", w.role),
        format!("sparkbox.rank={}", w.rank),
        "sparkbox.model=gemma-3-27b-it".to_string(),
        format!("sparkbox.recipe={}", w.recipe),
        format!("sparkbox.image={IMAGE_REF}"),
        format!("sparkbox.path={MODEL}"),
        format!("sparkbox.parallel={}", w.parallel),
    ];
    if w.listen {
        labels.extend(["sparkbox.port=8000".to_string(), "sparkbox.listen=192.168.50.154".to_string()]);
    }
    labels.push("sparkbox.maxlen=32768".to_string());
    for label in labels {
        v.extend(["--label".to_string(), label]);
    }
    v.extend(["--network", "host", "--ipc", "host", "--gpus", "all", "--ulimit", "memlock=-1", "--ulimit", "stack=67108864", "--ulimit", "nofile=1048576:1048576"].map(String::from));
    if w.privileged {
        v.push("--privileged".into());
    }
    v.extend(["--restart", "no", "-v", &format!("{MODEL}:{MODEL}:ro")].map(String::from));
    v.extend(
        ["/home/sparkuser/.cache/vllm:/root/.cache/vllm", "/home/sparkuser/.cache/flashinfer:/root/.cache/flashinfer", "/home/sparkuser/.triton:/root/.triton", "/home/sparkuser/.sparkbox/deploy/d1:/run/sparkbox/deploy:ro", &format!("/home/sparkuser/.sparkbox/{}:/run/sparkbox/launch.py:ro", crate::models::plan::launcher_name())]
            .iter()
            .flat_map(|m| ["-v".to_string(), m.to_string()]),
    );
    if w.key {
        v.extend(["-v".to_string(), "/home/sparkuser/.sparkbox/keys/d1:/run/sparkbox/key:ro".to_string()]);
    }
    v.extend(["--entrypoint".to_string(), "python3".to_string(), image_id(), "/run/sparkbox/launch.py".to_string()]);
    if w.key {
        v.push("--key".into());
    }
    v
}

fn shape_id(s: &Shape) -> &'static str {
    match s {
        Shape::Read => "read",
        Shape::Bookkeeping => "bookkeeping",
        Shape::Change(c, _) => c.id(),
    }
}

/// Row `n`'s text with its own paths: /tmp/sbconf-<n> and the planted files'.
fn numbered(text: &str, n: usize) -> String {
    text.replace("sbconf-N", &format!("sbconf-{n}")).replace("sbconf-bin-N", &format!("sbconf-bin-{n}")).replace("sbconf-env-N", &format!("sbconf-env-{n}"))
}

/// Whether row `n`'s hidden command ran (a background job of the row gets half a second):
/// /tmp/sbconf-<n> is there, or for a removing row it's gone. Then removes it.
async fn sentinel(c: &Session, n: usize, removes: bool) -> bool {
    let probe = format!("test -e /tmp/sbconf-{n} && echo there");
    let mut ran = false;
    for _ in 0..5 {
        if (run(c, &probe).await.stdout.trim() == "there") != removes {
            ran = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    run(c, &format!("rm -rf /tmp/sbconf-{n}")).await;
    ran
}

/// Row `n`'s setup before a shell runs it: its planted files (a removing row's folder), and
/// /tmp/sbconf-<n> cleared for a row that makes it.
async fn prepare(c: &Session, n: usize, row: &Row) {
    if let Some(plant) = row.plant {
        let plant = numbered(plant, n);
        assert_eq!(run(c, &plant).await.code, Some(0), "row {n}'s setup: {plant}");
    }
    if !row.removes {
        run(c, &format!("rm -rf /tmp/sbconf-{n}")).await;
    }
}

/// T2 blocker 2's real proof. Every row of `conformance_rows` is classified by both of SparkBox's
/// parsers (`classify::verdict`, and `fence_class::class_of` as Live would send it), then run on
/// the staging node through SparkBox's own `Live::run` as the cluster user: plainly (Live's
/// `env LC_ALL=C sh -c`, real dash 0.5.12) and as `bash -c '<row>'` (real bash 5.2.21). After each
/// run the test looks at the row's sentinel: made, or for a removing row (its folder planted before
/// each run) gone. Asserts: (i) a row either parser calls a read never ran its hidden command in
/// either shell (the bash-wrapped form is classified too); (ii) every
/// hidden-command row is `other` to the shape parser and ran in at least one shell (or it would
/// prove nothing); (iii) the reads stay reads; this round's change literals keep their class, C-X
/// is recognised by its exact text, and no other row passes as C-X.
/// Prints the whole table. The container's /bin/sh, dash and bash are recorded and checked against
/// what scripts/staging/README.md compares with the real nodes' shell facts.
#[tokio::test]
#[ignore]
async fn staging_shell_conformance() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    let node = "staging-conformance";
    let dir = data_dir("staging-conformance");
    let (live, _) = staging_live(s, &dir, node).await;

    // The shell facts, through the evidence capture's own step.
    let shell_step = crate::evidence::STEPS.iter().find(|(name, _)| *name == "shell").unwrap().1;
    let facts = live.run(node, shell_step, None, LIMIT).await.unwrap();
    let bash = live.run(node, "bash --version | head -1", None, LIMIT).await.unwrap();
    say(&format!("staging_shell_conformance: the staging node's shell facts (C-SH):\n{}{}", facts.stdout, bash.stdout));
    let fact_lines = lines(&facts.stdout);
    assert!(fact_lines.iter().any(|l| l == "/usr/bin/dash"), "/bin/sh is dash: {fact_lines:?}");
    assert!(fact_lines.iter().any(|l| l == "dash 0.5.12-6ubuntu5") && fact_lines.iter().any(|l| l == "bash 5.2.21-2ubuntu4"), "{fact_lines:?}");
    assert!(bash.stdout.starts_with("GNU bash, version 5.2.21(1)-release"), "{}", bash.stdout);

    let ctx = Ctx::default();
    let mut table = Vec::new();
    let mut wrong = Vec::new();
    for (i, row) in conformance_rows().into_iter().enumerate() {
        let n = i + 1;
        let cmd = numbered(&row.cmd, n);
        let wrapped = format!("bash -c {}", sh_quote(&cmd));
        let kind = classify::verdict(&cmd).kind;
        let wrapped_kind = classify::verdict(&wrapped).kind;
        let exec = match row.must {
            Must::Class(e, _) => e,
            Must::Cancel(_) => ExecKind::Sudo,
            _ => ExecKind::Run,
        };
        let not_run = matches!(row.must, Must::Class(..) | Must::Cancel(_));
        let shape = class_of(exec, &cmd, &ctx);
        let wrapped_shape = class_of(exec, &wrapped, &ctx);
        let (mut dash, mut bash) = (false, false);
        if !not_run {
            prepare(&c, n, &row).await;
            live.run(node, &cmd, None, Duration::from_secs(20)).await.unwrap_or_else(|e| panic!("row {n} under dash: {e}"));
            dash = sentinel(&c, n, row.removes).await;
            prepare(&c, n, &row).await;
            live.run(node, &wrapped, None, Duration::from_secs(20)).await.unwrap_or_else(|e| panic!("row {n} under bash: {e}"));
            bash = sentinel(&c, n, row.removes).await;
            // What a row may leave: its planted files, and the file named cat a wrapper row's
            // `touch cat …` makes where it runs.
            run(&c, &format!("rm -rf ~/sbconf-bin-{n} /tmp/sbconf-bin-{n} /tmp/sbconf-env-{n} /tmp/sbconf-{n} ~/cat /tmp/cat")).await;
        }
        let mut why = Vec::new();
        // (i) No read ran a hidden command, in the plain form or the bash-wrapped one.
        if (kind == Kind::Read || shape == Shape::Read) && (dash || bash) {
            why.push("a read that ran its hidden command");
        }
        if (wrapped_kind == Kind::Read || wrapped_shape == Shape::Read) && bash {
            why.push("bash -c of it is a read, and bash ran its hidden command");
        }
        match row.must {
            Must::Hidden => {
                if !dash && !bash {
                    why.push("neither shell ran the hidden command: the row proves nothing");
                }
                if !matches!(shape, Shape::Change(FenceClass::Other, _)) {
                    why.push("not `other` to the shape parser");
                }
                if kind == Kind::Read {
                    why.push("a read to the classifier");
                }
            }
            Must::Read => {
                if kind != Kind::Read || shape != Shape::Read {
                    why.push("no longer a read");
                }
                if dash || bash {
                    why.push("created a sentinel");
                }
            }
            Must::Class(_, class) => {
                if !matches!(&shape, Shape::Change(c, _) if *c == class) {
                    why.push("lost its class");
                }
                if kind == Kind::Read {
                    why.push("a read to the classifier");
                }
            }
            Must::Cancel(unit) => {
                if upgrade_cancel_unit(&cmd).as_deref() != Some(unit) {
                    why.push("not recognised as Cancel");
                }
                if shape == Shape::Read || kind == Kind::Read {
                    why.push("a read");
                }
            }
        }
        // Only C-X itself passes as C-X.
        if !matches!(row.must, Must::Cancel(_)) && upgrade_cancel_unit(&cmd).is_some() {
            why.push("passes as Cancel");
        }
        let fired = |b: bool| if b { "RAN" } else { "-" };
        let line = format!(
            "{n:>2} {:<44} classify={:<11} bash-c={:<11} shape={:<15} bash-c={:<15} dash:{:<3} bash:{:<3} {}",
            row.form,
            kind.as_str(),
            wrapped_kind.as_str(),
            shape_id(&shape),
            shape_id(&wrapped_shape),
            if not_run { "n/a" } else { fired(dash) },
            if not_run { "n/a" } else { fired(bash) },
            if why.is_empty() { "ok".to_string() } else { format!("WRONG: {}", why.join("; ")) }
        );
        if !why.is_empty() {
            wrong.push(line.clone());
        }
        table.push(line);
    }
    say(&format!("staging_shell_conformance: {} rows (dash: env LC_ALL=C sh -c, as Live sends; bash: bash -c '<row>' inside it)\n{}", table.len(), table.join("\n")));
    live.stop();
    assert!(wrong.is_empty(), "{} rows are wrong:\n{}", wrong.len(), wrong.join("\n"));
}

// ── round 5: what the read tier assumes of the real tools ───────────────────────────────────

/// Round 5's safety review, on the real tools: env 9.4, sed 4.9, sort 9.4, tar 1.35 and mawk.
/// Each row wrote a file or ran a program through a long option cut short, an option at the end of
/// a cluster or one a denylist never named. The test runs each through Live as the cluster user and
/// looks at what it did (a row whose tool did nothing proves nothing), and asserts both parsers
/// refuse it as a read. The controls use the same tools and leave everything as it was.
#[tokio::test]
#[ignore]
async fn staging_read_tier_options() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    let node = "staging-read-tier";
    let dir = data_dir("staging-read-tier");
    let (live, _) = staging_live(s, &dir, node).await;
    const SETUP: &str = "rm -rf /tmp/sbtier && mkdir -p /tmp/sbtier && cd /tmp/sbtier && printf 'b\\na\\n' > f && printf 'aaa\\n' > s && tar -cf a.tar f && seq 1 200000 > big && printf keep > victim \
        && printf '#!/bin/sh\\ntouch /tmp/sbtier/ran\\nexec cat \"$@\"\\n' > prog && chmod +x prog && printf 'BEGIN { system(\"touch /tmp/sbtier/ran\") }\\n' > p.awk";
    const RAN: &str = "test -e /tmp/sbtier/ran";
    const EDITED: &str = "grep -q baa /tmp/sbtier/s";
    let rows: &[(&str, &str, &str)] = &[
        ("env -iS", "env -iS '/usr/bin/touch /tmp/sbtier/ran'", RAN),
        ("env -vS", "env -vS '/usr/bin/touch /tmp/sbtier/ran'", RAN),
        ("env -iSstring", "env -iS'/usr/bin/touch /tmp/sbtier/ran'", RAN),
        ("sed --in", "sed --in s/a/b/ /tmp/sbtier/s", EDITED),
        ("sed --i", "sed --i s/a/b/ /tmp/sbtier/s", EDITED),
        ("sed after its operands", "sed s/a/b/ /tmp/sbtier/s --in-pl", EDITED),
        ("sort --o=", "sort --o=/tmp/sbtier/out /tmp/sbtier/f", "test -s /tmp/sbtier/out"),
        ("sort --outp", "sort --outp /tmp/sbtier/out /tmp/sbtier/f", "test -s /tmp/sbtier/out"),
        ("sort --comp=", "sort -S 64k -T /tmp/sbtier --comp=/tmp/sbtier/prog /tmp/sbtier/big >/dev/null", RAN),
        ("tar --use=", "tar -tf /tmp/sbtier/a.tar --use=/tmp/sbtier/prog >/dev/null", RAN),
        ("tar --checkpoint-a=exec=", "tar -tf /tmp/sbtier/a.tar --checkpoint=1 --checkpoint-a=exec='touch /tmp/sbtier/ran' >/dev/null", RAN),
        ("tar --index-file=, spelled out", "tar -tvf /tmp/sbtier/a.tar --index-file=/tmp/sbtier/victim", "! grep -q keep /tmp/sbtier/victim"),
        ("awk -W exec", "awk -W exec /tmp/sbtier/p.awk", RAN),
    ];
    let ctx = Ctx::default();
    let mut wrong = Vec::new();
    for (form, cmd, did) in rows {
        assert_eq!(run(&c, SETUP).await.code, Some(0), "the setup");
        let refused = classify::verdict(cmd).kind != Kind::Read && !classify::read_only(cmd) && matches!(class_of(ExecKind::Run, cmd, &ctx), Shape::Change(FenceClass::Other, _));
        live.run(node, cmd, None, Duration::from_secs(20)).await.unwrap_or_else(|e| panic!("{form}: {e}"));
        let worked = run(&c, did).await.code == Some(0);
        say(&format!("staging_read_tier_options: {form:<32} wrote or ran: {worked:<5} refused as a read: {refused}"));
        if !worked || !refused {
            wrong.push((*form).to_string());
        }
    }
    // The same tools only reading: reads to both parsers, and nothing moved.
    for cmd in ["sed -n p /tmp/sbtier/s", "sed --quiet --regexp-extended p /tmp/sbtier/s", "sort --reverse /tmp/sbtier/f", "tar -tvf /tmp/sbtier/a.tar", "tar --list --file=/tmp/sbtier/a.tar", "awk -F: '{print $1}' /tmp/sbtier/s"] {
        assert_eq!(run(&c, SETUP).await.code, Some(0), "the setup");
        let before = run(&c, "ls -lA --time-style=full-iso /tmp/sbtier | grep -v big; cat /tmp/sbtier/s /tmp/sbtier/victim").await.stdout;
        assert_eq!(classify::verdict(cmd).kind, Kind::Read, "{cmd}");
        assert_eq!(class_of(ExecKind::Run, cmd, &ctx), Shape::Read, "{cmd}");
        assert_eq!(live.run(node, cmd, None, Duration::from_secs(20)).await.unwrap().code, Some(0), "{cmd}");
        assert_eq!(run(&c, "ls -lA --time-style=full-iso /tmp/sbtier | grep -v big; cat /tmp/sbtier/s /tmp/sbtier/victim").await.stdout, before, "{cmd} changed something");
    }
    // P2 round 1's review, on the same node. Each row wrote or ran on the real tool (Ubuntu 24.04's
    // util-linux 2.39, apt 2.8, glibc 2.39, iproute2 6.1, systemd 255) and each was a read until
    // this round. The second word of a row is what's classified, the third what's run.
    const SETUP2: &str = "rm -rf /tmp/sbtier && mkdir -p /tmp/sbtier/libnss_x /tmp/sbtier/d/e && cd /tmp/sbtier && printf 'hello\\n' > a.txt && truncate -s 8M img && mkfs.ext4 -q -F img \
        && printf '#!/bin/sh\\ntouch /tmp/sbtier/ran\\nexec cat \"$@\"\\n' > prog && chmod +x prog \
        && printf 'Dir::Cache::pkgcache \"/tmp/sbtier/conf-cache\";\\n' > cache.conf && printf 'Dir::Bin::dpkg \"/tmp/sbtier/prog\";\\n' > run.conf \
        && printf '0 string hello HELLO\\n' > m && printf -- '-C -m m\\n' > list && printf 'kernel.domainname = sbtier\\n' > sysctl.conf && printf 'Foo \"x\";\\n' > showhold && printf 'qdisc add dev lo root handle 1: prio\\n' > batch && printf '#old\\n' > pci \
        && printf '%s\\n' .text .global\\ sb_init sb_init: 'mov x8, #56' 'mov x0, #-100' 'adr x1, sb_path' 'mov x2, #0x41' 'mov x3, #0x1a4' 'svc #0' ret 'sb_path: .asciz \"/tmp/sbtier/ran\"' '.section .init_array,\"aw\"' '.xword sb_init' '.section .note.GNU-stack,\"\",@progbits' > evil.s \
        && as -o evil.o evil.s && ld -shared -o evil.so.2 evil.o";
    const WROTE: fn(&str) -> String = |f| format!("test -s /tmp/sbtier/{f}");
    // As the cluster user, as a Fleet read would run.
    let found: Vec<(&str, String, String)> = vec![
        ("blkid -c", "blkid -c /tmp/sbtier/cache /tmp/sbtier/img".into(), WROTE("cache")),
        ("blkid --cache=, shortened", "blkid --cache=/tmp/sbtier/cache /tmp/sbtier/img".into(), WROTE("cache")),
        ("blkid --c=, shortened", "blkid --c=/tmp/sbtier/cache /tmp/sbtier/img".into(), WROTE("cache")),
        ("blkid -dc, in a cluster", "blkid -dc /tmp/sbtier/cache /tmp/sbtier/img".into(), WROTE("cache")),
        ("apt-cache -p, any verb", "apt-cache -p /tmp/sbtier/pkg stats".into(), WROTE("pkg")),
        ("apt-cache --pkg-cache=", "apt-cache --pkg-cache=/tmp/sbtier/pkg stats".into(), WROTE("pkg")),
        ("apt-cache -s", "apt-cache -p /tmp/sbtier/pkg -s /tmp/sbtier/src stats".into(), WROTE("src")),
        ("apt-cache -o Dir::Cache::pkgcache", "apt-cache -o Dir::Cache::pkgcache=/tmp/sbtier/pkg policy".into(), WROTE("pkg")),
        ("apt-cache -oDir::Cache, attached", "apt-cache -oDir::Cache::pkgcache=/tmp/sbtier/pkg policy".into(), WROTE("pkg")),
        ("apt-cache -c", "apt-cache -c /tmp/sbtier/cache.conf policy".into(), WROTE("conf-cache")),
        ("apt-cache -o Dir::Bin::dpkg", "apt-cache -o Dir::Bin::dpkg=/tmp/sbtier/prog policy".into(), RAN.into()),
        ("apt-config -o Dir::Bin::dpkg", "apt-config -o Dir::Bin::dpkg=/tmp/sbtier/prog dump".into(), RAN.into()),
        ("apt-config -c", "apt-config -c /tmp/sbtier/run.conf dump".into(), RAN.into()),
        ("getent -s", "cd /tmp/sbtier && getent -s x/../evil passwd root".into(), RAN.into()),
        ("getent --service=", "cd /tmp/sbtier && getent --service=passwd:x/../evil passwd root".into(), RAN.into()),
        ("getent --serv=, shortened", "cd /tmp/sbtier && getent --serv=x/../evil passwd root".into(), RAN.into()),
        ("iconv -o", "iconv -f utf-8 -t utf-8 -o /tmp/sbtier/out /tmp/sbtier/a.txt".into(), WROTE("out")),
        ("uniq, a second operand", "uniq /tmp/sbtier/a.txt /tmp/sbtier/out".into(), WROTE("out")),
        ("ss -D", "ss -D /tmp/sbtier/out".into(), "test -e /tmp/sbtier/out".into()),
        // A rule reads words, and these come from the shell: a substitution, a variable, a glob.
        ("find $(echo -delete)", "find /tmp/sbtier/d $(echo -delete)".into(), "! test -e /tmp/sbtier/d".into()),
        ("find \"$(echo -delete)\"", "find /tmp/sbtier/d \"$(echo -delete)\"".into(), "! test -e /tmp/sbtier/d".into()),
        ("find, a variable a for set", "for x in -delete; do find /tmp/sbtier/d $x; done".into(), "! test -e /tmp/sbtier/d".into()),
        ("find, a variable read set", "echo -delete | { read x; find /tmp/sbtier/d $x; }".into(), "! test -e /tmp/sbtier/d".into()),
        ("find, a script's $1", "sh -c 'find /tmp/sbtier/d $1' x -delete".into(), "! test -e /tmp/sbtier/d".into()),
        ("sort $(echo -o …)", "sort $(echo -o /tmp/sbtier/out) /tmp/sbtier/a.txt".into(), WROTE("out")),
        ("sort \"$(echo -o…)\"", "sort \"$(echo -o/tmp/sbtier/out)\" /tmp/sbtier/a.txt".into(), WROTE("out")),
        ("iconv $(echo -o …)", "iconv $(echo -o /tmp/sbtier/out) -f utf-8 -t utf-8 /tmp/sbtier/a.txt".into(), WROTE("out")),
        ("sed \"$(echo 1w …)\"", "sed -n \"$(echo 1w /tmp/sbtier/out)\" /tmp/sbtier/a.txt".into(), WROTE("out")),
    ];
    // apt-cache writes where it's told only once root's caches are there, as on a node that has run
    // apt; a test's reset drops them.
    sudo_ok(&c, s, "apt-cache gencaches").await;
    for (form, cmd, did) in &found {
        assert_eq!(run(&c, SETUP2).await.code, Some(0), "the setup");
        let refused = classify::verdict(cmd).kind != Kind::Read && !classify::read_only(cmd) && matches!(class_of(ExecKind::Run, cmd, &ctx), Shape::Change(FenceClass::Other, _));
        live.run(node, cmd, None, Duration::from_secs(20)).await.unwrap_or_else(|e| panic!("{form}: {e}"));
        let worked = run(&c, did).await.code == Some(0);
        say(&format!("staging_read_tier_options: {form:<36} wrote or ran: {worked:<5} refused as a read: {refused}"));
        if !worked || !refused {
            wrong.push((*form).to_string());
        }
    }
    // Tools the staging image may not carry. A row that can't run proves nothing, so it's named,
    // and with SPARKBOX_STAGING_REQUIRED=1 it fails, like a test that can't reach Docker.
    const DISK: &str = "/tmp/sbtier/disk";
    const TABLE: &str = "truncate -s 16M /tmp/sbtier/disk && printf 'label: gpt\\n,1M\\n,1M\\n' | sfdisk -q /tmp/sbtier/disk";
    const LABEL: &str = "truncate -s 16M /tmp/sbtier/disk && parted -s /tmp/sbtier/disk mklabel gpt";
    let needs: Vec<(&str, &str, String, String)> = vec![
        ("pidstat", "pidstat -e", "pidstat -e touch /tmp/sbtier/ran".into(), RAN.into()),
        ("pidstat", "pidstat -u -e", "pidstat -u -e touch /tmp/sbtier/ran".into(), RAN.into()),
        ("tree", "tree -o", "tree -o /tmp/sbtier/out /tmp/sbtier/d".into(), WROTE("out")),
        ("tree", "tree -R -H", "cd /tmp/sbtier && tree -R -L 1 -H . d".into(), WROTE("d/e/00Tree.html")),
        ("tree", "tree -RL 1 -H, in a cluster", "cd /tmp/sbtier && tree -RL 1 -H . d".into(), WROTE("d/e/00Tree.html")),
        ("tree", "tree -LR 1 -H, its value after", "cd /tmp/sbtier && tree -LR 1 -H . d".into(), WROTE("d/e/00Tree.html")),
        ("sar", "sar -o", "sar -o /tmp/sbtier/out 1 1".into(), WROTE("out")),
        ("rg", "rg --pre", "rg --pre /tmp/sbtier/prog hello /tmp/sbtier/a.txt".into(), RAN.into()),
        ("rg", "rg --hostname-bin", "rg --hostname-bin /tmp/sbtier/prog hello /tmp/sbtier/a.txt".into(), RAN.into()),
        ("lspci", "lspci -qq -O net.cache_name", "lspci -qq -O net.domain=invalid.invalid -O net.cache_name=/tmp/sbtier/pci".into(), "grep -q PCI-CACHE /tmp/sbtier/pci".into()),
        ("file", "file -C", "cd /tmp/sbtier && file -C -m m".into(), WROTE("m.mgc")),
        ("file", "file --comp, shortened", "cd /tmp/sbtier && file --comp -m m".into(), WROTE("m.mgc")),
        ("file", "file -Cm, in a cluster", "cd /tmp/sbtier && file -Cm m".into(), WROTE("m.mgc")),
        ("file", "xargs file, fed -C", "cd /tmp/sbtier && cat list | xargs file".into(), WROTE("m.mgc")),
        ("rg", "rg $(echo --pre=…)", "rg $(echo --pre=/tmp/sbtier/prog) hello /tmp/sbtier/a.txt".into(), RAN.into()),
        ("sar", "sar $(echo -o …)", "sar $(echo -o /tmp/sbtier/out) 1 1".into(), WROTE("out")),
        ("tree", "tree $(echo -o …)", "tree $(echo -o /tmp/sbtier/out) /tmp/sbtier/d".into(), WROTE("out")),
        // sfdisk and parted past a listing word, on an image file the row makes. git and curl aren't
        // on this image (each needs the real libbrotli1, a dummy here), so theirs are in
        // `what_a_rule_cant_read_stays_behind_look_only`, on a real shell.
        ("sfdisk", "sfdisk -l --delete", format!("{TABLE}; sfdisk -l --delete {DISK} 1"), format!("test \"$(sfdisk -d {DISK} | grep -c start=)\" = 1")),
        ("sfdisk", "sfdisk -s --delete", format!("{TABLE}; sfdisk -s --delete {DISK}"), format!("test \"$(sfdisk -d {DISK} | grep -c start=)\" = 0")),
        ("sfdisk", "sfdisk -d --part-label", format!("{TABLE}; sfdisk -d --part-label {DISK} 1 VERIFYX"), format!("sfdisk -d {DISK} | grep -q VERIFYX")),
        ("parted", "parted print mkl", format!("{LABEL}; parted -s {DISK} print mkl msdos"), format!("parted -s {DISK} print | grep -q msdos")),
        ("parted", "parted print mkp", format!("{LABEL}; parted -s {DISK} print mkp a 1MiB 2MiB"), format!("parted -s {DISK} print | grep -q '^ 1 '")),
        ("parted", "parted mkl print", format!("{LABEL}; parted -s {DISK} mkl msdos print"), format!("parted -s {DISK} print | grep -q msdos")),
    ];
    let mut absent: Vec<&str> = Vec::new();
    for (tool, form, cmd, did) in &needs {
        if run(&c, &format!("command -v {tool}")).await.code != Some(0) {
            say(&format!("staging_read_tier_options: {form:<36} NOT RUN: {tool} isn't on this staging image"));
            if !absent.contains(tool) {
                absent.push(tool);
            }
            continue;
        }
        assert_eq!(run(&c, SETUP2).await.code, Some(0), "the setup");
        // A row's own set-up comes before what it proves, so only that part is classified.
        let proved = cmd.rsplit("; ").next().unwrap_or(cmd);
        let refused = classify::verdict(proved).kind != Kind::Read && !classify::read_only(proved) && matches!(class_of(ExecKind::Run, proved, &ctx), Shape::Change(FenceClass::Other, _));
        live.run(node, cmd, None, Duration::from_secs(20)).await.unwrap_or_else(|e| panic!("{form}: {e}"));
        let worked = run(&c, did).await.code == Some(0);
        say(&format!("staging_read_tier_options: {form:<36} wrote or ran: {worked:<5} refused as a read: {refused}"));
        if !worked || !refused {
            wrong.push((*form).to_string());
        }
    }
    // As root, what sudo would run: the node's own service. What would take the staging node's
    // routes with it runs as the cluster user, in a user and network namespace of its own (the
    // container has no NET_ADMIN). (form, what's classified, what's run, did it.)
    const UNIT: &str = "sparkbox-dummy-svc.service";
    let stopped = format!("! systemctl is-active -q {UNIT}");
    // `cmd` on a dummy link that's up, or an xfrm state that's there (the marker says so, or the
    // row would pass on nothing), and what's left of it.
    const DOWN: &str = "test -e /tmp/sbtier/ran && test -s /tmp/sbtier/said && ! grep -q ,UP /tmp/sbtier/said";
    const GONE: &str = "test -e /tmp/sbtier/ran && ! test -s /tmp/sbtier/said";
    let ip_link = |cmd: &str| format!("unshare -Urn sh -c 'ip link add d1 type dummy && ip link set d1 up && ip -o link show d1 | grep -q ,UP && touch /tmp/sbtier/ran; {cmd}; ip -o link show d1 > /tmp/sbtier/said 2>/dev/null'");
    let ip_state = |cmd: &str| format!("unshare -Urn sh -c 'ip xfrm state add src 10.0.0.1 dst 10.0.0.2 proto esp spi 1 mode tunnel enc aes 0x00112233445566778899aabbccddeeff && ip xfrm state list | grep -q 10.0.0.1 && touch /tmp/sbtier/ran; {cmd}; ip xfrm state list > /tmp/sbtier/said'");
    let as_root: Vec<(&str, String, String, String)> = vec![
        ("apt-cache gencaches", "apt-cache gencaches".into(), "rm -f /var/cache/apt/pkgcache.bin /var/cache/apt/srcpkgcache.bin; apt-cache gencaches".into(), "test -s /var/cache/apt/pkgcache.bin".into()),
        ("systemctl -p status stop", format!("systemctl -p status stop {UNIT}"), format!("systemctl -p status stop {UNIT}"), stopped.clone()),
        ("systemctl --state status stop", format!("systemctl --state status stop {UNIT}"), format!("systemctl --state status stop {UNIT}"), stopped.clone()),
        (
            "busctl --match status call",
            format!("busctl --match status call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager StopUnit ss {UNIT} replace"),
            format!("busctl --match status call org.freedesktop.systemd1 /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager StopUnit ss {UNIT} replace"),
            format!("sleep 1; {stopped}"),
        ),
        ("timedatectl -p status set-ntp", "timedatectl -p status set-ntp false".into(), "timedatectl -p status set-ntp false 2>/tmp/sbtier/said; grep -q 'set ntp' /tmp/sbtier/said && touch /tmp/sbtier/ran".into(), RAN.into()),
        ("apt-mark -c showhold hold", "cd /tmp/sbtier && apt-mark -c showhold hold sparkbox-dummy".into(), "cd /tmp/sbtier && apt-mark -c showhold hold sparkbox-dummy".into(), "apt-mark showhold | grep -q sparkbox-dummy".into()),
        ("sysctl -f", "sysctl -f /tmp/sbtier/sysctl.conf".into(), "unshare -u sh -c 'mount -o remount,rw /proc/sys; sysctl -f /tmp/sbtier/sysctl.conf; cat /proc/sys/kernel/domainname > /tmp/sbtier/said'".into(), "grep -q sbtier /tmp/sbtier/said".into()),
        ("ip rule f", "ip rule f".into(), "unshare -Urn sh -c 'ip rule add from 10.9.9.9 lookup 100; ip rule f; ip rule | wc -l > /tmp/sbtier/said'".into(), "grep -qx 1 /tmp/sbtier/said".into()),
        ("tc -batch f show", "tc -batch /tmp/sbtier/batch show".into(), "unshare -Urn sh -c 'tc -batch /tmp/sbtier/batch show; tc qdisc show dev lo > /tmp/sbtier/said'".into(), "grep -q prio /tmp/sbtier/said".into()),
        // ip's options cut short: the word after one is its value, so the next is the object.
        ("ip -fa inet l set down", "ip -fa inet l set d1 down".into(), ip_link("ip -fa inet l set d1 down"), DOWN.into()),
        ("ip --fa inet l set down", "ip --fa inet l set d1 down".into(), ip_link("ip --fa inet l set d1 down"), DOWN.into()),
        ("ip -lo 1 l set down", "ip -lo 1 l set d1 down".into(), ip_link("ip -lo 1 l set d1 down"), DOWN.into()),
        ("ip -rcv 100000 l del", "ip -rcv 100000 l del d1".into(), ip_link("ip -rcv 100000 l del d1"), GONE.into()),
        // A sub-object isn't a verb.
        ("ip xfrm s flush", "ip xfrm s flush".into(), ip_state("ip xfrm s flush"), GONE.into()),
        ("ip x s f", "ip x s f".into(), ip_state("ip x s f"), GONE.into()),
        // The journal, rotated by an option the shell gives it.
        (
            "journalctl $(echo --rotate)",
            "journalctl $(echo --rotate)".into(),
            "ls /run/log/journal/*/ /var/log/journal/*/ 2>/dev/null | wc -l > /tmp/sbtier/said; journalctl $(echo --rotate); sleep 1; test \"$(ls /run/log/journal/*/ /var/log/journal/*/ 2>/dev/null | wc -l)\" -gt \"$(cat /tmp/sbtier/said)\" && touch /tmp/sbtier/ran".into(),
            RAN.into(),
        ),
    ];
    for (form, cmd, root, did) in &as_root {
        assert_eq!(run(&c, SETUP2).await.code, Some(0), "the setup");
        let refused = classify::verdict(cmd).kind != Kind::Read && !classify::read_only(cmd) && matches!(class_of(ExecKind::Run, cmd, &ctx), Shape::Change(FenceClass::Other, _));
        if root.starts_with("unshare -Ur") {
            run(&c, root).await;
        } else {
            sudo(&c, s, root, None).await;
        }
        let worked = run(&c, did).await.code == Some(0);
        say(&format!("staging_read_tier_options: {form:<36} wrote or ran: {worked:<5} refused as a read: {refused}"));
        sudo(&c, s, &format!("systemctl start {UNIT}; apt-mark unhold sparkbox-dummy >/dev/null"), None).await;
        if !worked || !refused {
            wrong.push((*form).to_string());
        }
    }
    // What the review left alone, and the same tools only looking: reads to both parsers, and
    // nothing moved. `diff --output` and lsof's -D aren't options on these versions; `blkid -c
    // /dev/null` stays a read.
    let before_setup = run(&c, SETUP2).await;
    assert_eq!(before_setup.code, Some(0), "the setup");
    for cmd in [
        "blkid -c /dev/null /tmp/sbtier/img",
        "apt-cache policy",
        "apt-cache stats",
        "apt-config dump APT::Periodic",
        "apt-mark showhold",
        "getent passwd root",
        "ip rule list",
        "ip route get 127.0.0.1",
        "tc qdisc show dev lo",
        "ip -o link",
        "ip -br -4 addr",
        "ip link show",
        "cat $(echo /tmp/sbtier/a.txt)",
        "for x in /tmp/sbtier/a.txt; do cat $x; done",
        "find /tmp/sbtier/d -type d",
        "sort /tmp/sbtier/a.txt",
        "systemctl is-active sparkbox-dummy-svc.service",
        "systemctl show -p ActiveState --value sparkbox-dummy-svc.service",
        "timedatectl show -p NTP",
        "busctl status",
    ] {
        let snapshot = "ls -lA --time-style=full-iso /tmp/sbtier | grep -v -E ' (evil|img)'; apt-mark showhold; systemctl is-active sparkbox-dummy-svc.service";
        let before = run(&c, snapshot).await.stdout;
        assert_eq!(classify::verdict(cmd).kind, Kind::Read, "{cmd}");
        assert_eq!(class_of(ExecKind::Run, cmd, &ctx), Shape::Read, "{cmd}");
        live.run(node, cmd, None, Duration::from_secs(20)).await.unwrap_or_else(|e| panic!("{cmd}: {e}"));
        assert_eq!(run(&c, snapshot).await.stdout, before, "{cmd} changed something");
    }
    run(&c, "rm -rf /tmp/sbtier").await;
    live.stop();
    if !absent.is_empty() {
        say(&format!("staging_read_tier_options: SKIP rows that need {}: add them to scripts/staging/image/Dockerfile", absent.join(", ")));
    }
    assert!(wrong.is_empty(), "rows that didn't hold: {wrong:?}");
    assert!(absent.is_empty() || std::env::var_os("SPARKBOX_STAGING_REQUIRED").is_none(), "these tools aren't on the staging image, so their rows didn't run: {absent:?}");
}

// ── round 4: first light under real dash (Session B step 27) ────────────────────────────────

/// Where the docker stand-in keeps its state and its record of calls (image/docker-standin.py).
const DOCKER_STATE: &str = "/var/lib/sparkbox-staging-docker";

/// The docker stand-in's calls so far, in order (argv, environment, every process's command line
/// and the environments the caller may read, at the moment of the call).
async fn docker_calls(c: &Session) -> Vec<serde_json::Value> {
    let out = run(c, &format!("cat {DOCKER_STATE}/calls.jsonl 2>/dev/null")).await.stdout;
    out.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("a calls.jsonl line: {e}: {l}"))).collect()
}

fn argv(call: &serde_json::Value) -> Vec<String> {
    call["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect()
}

/// Clears what a load leaves in the cluster user's home and the stand-in's state, then gives the
/// stand-in the contracts' image and makes the model folder (the test's own setup).
async fn first_light_setup(c: &Session) {
    let setup = format!(
        "rm -rf ~/.cache/vllm ~/.cache/flashinfer ~/.triton ~/.sparkbox/keys ~/.sparkbox/deploy ~/.sparkbox/{launcher} ~/models/google /dev/shm/psm_sbstage && rm -f {DOCKER_STATE}/*.json {DOCKER_STATE}/calls.jsonl \
         && printf '{{\"{IMAGE_REF}\": \"{}\"}}' > {DOCKER_STATE}/images.json && mkdir -p {MODEL} && printf '{{\"architectures\": [\"Gemma3ForConditionalGeneration\"]}}' > {MODEL}/config.json",
        image_id(), launcher = crate::models::plan::launcher_name()
    );
    let out = run(c, &setup).await;
    assert_eq!(out.code, Some(0), "first light's setup: {}", out.stderr);
}

/// The pre-flight's raw output, indented for the console.
fn preflight_raw(out: &str) -> String {
    out.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

/// Session B step 27's command shapes under real dash, as SparkBox's Live sends them on the
/// staging node (its docker is the stand-in, so nothing starts):
/// - C-P2 (`plan::preflight_script`) on real Ubuntu output, read by `plan::parse_preflight`: the
///   meminfo figures, earlyoom not installed, the image's local id, Docker's store, the model
///   folder, `ss` with nothing on 8000, the real /dev/shm (empty, then with a file in it), the cache
///   folders missing and then there (the cluster user's), no leftovers, then the load as one;
/// - C-F (`deploy-files`, the shipped sparkjob.py under real Python 3.12): the launch files and the
///   key's modes on real ext4-backed Linux, the three cache folders made by the cluster user, and
///   only the folder printed;
/// - C-L2 (`plan::rank_command`) through `Live::run`: one command with nothing on stdin. docker's argv
///   is the contract's vector word for word (each word intact, $HOME expanded), and the key is in
///   none of argv, docker's environment, any process's command line or readable environment, the
///   activity log or ssh.log;
/// - C-U2 (`plan::unload_command`): `ps -aq` with the instance and deployment labels, then
///   `stop -t 30`, then `rm` (never `rm -f`); the container, the key and the launch files are gone.
#[tokio::test]
#[ignore]
async fn staging_session_b_step27_first_light_shapes() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    first_light_setup(&c).await;
    let node = "staging-step27";
    let dir = data_dir("staging-step27");
    let (live, _) = staging_live(s, &dir, node).await;
    let spec = first_light_spec();

    // C-P2 before the load.
    let mem = run(&c, "grep -E '^(MemTotal|MemFree|MemAvailable|Cached):' /proc/meminfo").await.stdout;
    let total_kb: u64 = mem.lines().find_map(|l| l.strip_prefix("MemTotal:")).unwrap().trim().trim_end_matches(" kB").trim().parse().unwrap();
    let before = live.run(node, &preflight_script(&spec), None, LIMIT).await.unwrap();
    assert_eq!(before.code, Some(0), "{}", before.stderr);
    let p = parse_preflight(node, &spec, &before.stdout);
    say(&format!("staging_session_b_step27_first_light_shapes: C-P2 on the staging node, before the load:\n{}", preflight_raw(&before.stdout)));
    let m = p.mem.clone().expect("the meminfo figures");
    assert_eq!(m.total, total_kb * 1024, "MemTotal");
    assert!(m.free > 0 && m.available >= m.free && m.cached > 0 && m.available < m.total, "{m:?}");
    assert_eq!(p.need_free, (m.total as f64 * 0.75) as u64 + load_plan::FREE_HEADROOM);
    assert_eq!(p.drop_needed, m.free < p.need_free);
    assert_eq!(p.earlyoom, Some(Earlyoom { active: false, state: "inactive".into(), config: None }), "earlyoom isn't installed");
    assert!(before.stdout.contains("cat: /etc/default/earlyoom: No such file or directory"), "{}", before.stdout);
    assert_eq!(p.image_id.as_deref(), Some(image_id().as_str()));
    assert_eq!(p.store.as_deref(), Some("containerd"), "Docker Desktop's words for its containerd store, as the stand-in answers");
    assert!(p.model_present && p.port_free && p.port_owner.is_none(), "{p:?}");
    assert!(p.shm.is_empty(), "the container's /dev/shm is empty: {:?}", p.shm);
    assert!(p.caches.iter().all(|d| !d.exists && d.owner.is_none()), "no cache folders yet: {:?}", p.caches);
    assert!(p.leftovers.is_empty() && p.ray_leftovers.is_empty() && p.ollama_resident.is_empty(), "{p:?}");
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    assert!(rows(&live, "echo '--- mem'").iter().all(|e| !e.escalated && !e.changes), "the pre-flight is a read");

    // C-F. Quotes, a backslash, a $( ) and a backtick in the key: no shell ever reads it.
    let key = format!("sbk-{}-'\"\\$(id)`id`", sha256_hex(b"staging first light"));
    live.ensure_file(node, "launch.py", load_plan::LAUNCH).await.unwrap();
    let python = run(&c, "python3 --version").await.stdout;
    assert!(python.starts_with("Python 3.12."), "{python}");
    let payload = launch_payload(&spec, 0, &[], Some(&key));
    let printed = live.sparkjob(node, "deploy-files d1 single", Some(payload.to_string().as_bytes()), LIMIT).await.unwrap();
    assert_eq!(printed, json!({ "dir": "/home/sparkuser/.sparkbox/deploy/d1" }), "only the folder comes back");
    let modes = run(&c, "stat -c '%a %U %n' ~/.sparkbox/deploy/d1 ~/.sparkbox/deploy/d1/argv.json ~/.sparkbox/deploy/d1/env.json").await.stdout;
    assert_eq!(lines(&modes), ["700 sparkuser /home/sparkuser/.sparkbox/deploy/d1", "600 sparkuser /home/sparkuser/.sparkbox/deploy/d1/argv.json", "600 sparkuser /home/sparkuser/.sparkbox/deploy/d1/env.json"]);
    let files: Value = serde_json::from_str(&run(&c, "cat ~/.sparkbox/deploy/d1/argv.json").await.stdout).unwrap();
    assert_eq!(files, json!(rank_argv(&spec, 0, &[])), "argv.json is C-V's");
    let files: Value = serde_json::from_str(&run(&c, "cat ~/.sparkbox/deploy/d1/env.json").await.stdout).unwrap();
    assert_eq!(files, json!(rank_env(&spec, 0, &[])), "env.json is C-E's");

    // C-L2: one command, nothing on stdin.
    let cmd = rank_command(&spec, 0);
    assert_eq!(cmd, single_command(&spec));
    let started = live.run(node, &cmd, None, LIMIT).await.unwrap();
    assert_eq!(started.code, Some(0), "C-L2 under dash: {}", started.stderr);
    let id = started.stdout.trim().to_string();
    assert!(id.len() == 64 && id.chars().all(|ch| ch.is_ascii_hexdigit()), "docker run -d printed the container id: {:?}", started.stdout);

    let calls = docker_calls(&c).await;
    let run_call = calls.iter().find(|call| argv(call).first().map(String::as_str) == Some("run")).expect("docker run was called");
    let instance = crate::identity::instance_name();
    let expected = c_l2_words(&Words { name: "sparkbox-d1", role: "single", rank: 0, recipe: "generic", parallel: "1x1", listen: true, privileged: false, key: true });
    let got = argv(run_call);
    say(&format!("staging_session_b_step27_first_light_shapes: docker's argv under dash ({} words):\n{}", got.len(), got.iter().enumerate().map(|(i, w)| format!("    [{i:>2}] {w}")).collect::<Vec<_>>().join("\n")));
    assert_eq!(got, expected, "docker's argv under real dash");
    assert_eq!(run_call["uid"], 1000, "docker ran as the cluster user");

    // The key reached the file and nothing else.
    let seen = serde_json::to_string(&calls).unwrap();
    assert!(!seen.contains(&key), "the key isn't in docker's argv or environment, nor in any process's command line or readable environment");
    let env: &serde_json::Map<String, serde_json::Value> = run_call["env"].as_object().unwrap();
    assert!(!env.contains_key("k") && !env.contains_key("VLLM_API_KEY"), "no key variable reached docker: {:?}", env.keys().collect::<Vec<_>>());
    assert!(run_call["procs"].as_array().unwrap().len() > 3, "the process list was read: {}", run_call["procs"]);
    let file = run(&c, "stat -c '%a %U %s' ~/.sparkbox/keys/d1; cat ~/.sparkbox/keys/d1").await.stdout;
    assert_eq!(file, format!("400 sparkuser {}\n{key}", key.len()), "the key file is 0400, the cluster user's, and holds exactly the key");
    let caches = run(&c, "stat -c '%U %a %n' ~/.cache/vllm ~/.cache/flashinfer ~/.triton").await.stdout;
    assert_eq!(lines(&caches), ["sparkuser 700 /home/sparkuser/.cache/vllm", "sparkuser 700 /home/sparkuser/.cache/flashinfer", "sparkuser 700 /home/sparkuser/.triton"], "made by the cluster user");
    assert!(live.audit.entries().iter().all(|e| !e.command.contains(&key)), "the key isn't in the activity log");
    assert!(!std::fs::read_to_string(dir.join("ssh.log")).unwrap_or_default().contains(&key), "the key isn't in ssh.log");

    // C-P2 after: the caches are there, a file in /dev/shm is listed, and the load's container shows.
    run(&c, "printf x > /dev/shm/psm_sbstage").await;
    let after = live.run(node, &preflight_script(&spec), None, LIMIT).await.unwrap();
    let q = parse_preflight(node, &spec, &after.stdout);
    say(&format!("staging_session_b_step27_first_light_shapes: C-P2 after the load:\n{}", preflight_raw(&after.stdout)));
    assert_eq!(q.shm, ["psm_sbstage"]);
    assert!(q.caches.iter().all(|d| d.exists && d.owner.as_deref() == Some("sparkuser")), "{:?}", q.caches);
    assert_eq!(q.leftovers.iter().map(|l| (l.name.as_str(), l.deployment.as_str(), l.instance.as_str(), l.ours)).collect::<Vec<_>>(), [("sparkbox-d1", "d1", instance, true)], "{:?}", q.leftovers);
    assert!(q.leftovers[0].status.starts_with("Up "), "{:?}", q.leftovers);
    run(&c, "rm -f /dev/shm/psm_sbstage").await;

    // C-U2: stop with 30 s, then remove; never rm -f. The launch files and the key go with them.
    let from = docker_calls(&c).await.len();
    let unload = live.run(node, &unload_command("d1"), None, LIMIT).await.unwrap();
    assert_eq!((unload.code, unload.stdout.trim()), (Some(0), "ok"), "{}", unload.stderr);
    let unload_calls: Vec<Vec<String>> = docker_calls(&c).await[from..].iter().map(argv).collect();
    let short = &id[..12];
    let words = |v: &[&str]| v.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        unload_calls,
        [
            words(&["ps", "-aq", "--filter", &format!("label=sparkbox.instance={instance}"), "--filter", "label=sparkbox.deployment=d1"]),
            words(&["stop", "-t", "30", short]),
            words(&["rm", short]),
        ],
        "C-U2's docker calls under dash"
    );
    let removes: Vec<Vec<String>> = docker_calls(&c).await.iter().map(argv).filter(|a| a.first().map(String::as_str) == Some("rm")).collect();
    assert!(!removes.is_empty() && removes.iter().all(|a| !a.iter().any(|w| w == "-f" || w == "--force" || (w.starts_with('-') && !w.starts_with("--") && w.contains('f')))), "never rm -f: {removes:?}");
    assert_eq!(run(&c, &format!("cat {DOCKER_STATE}/containers.json")).await.stdout.trim(), "[]", "the container is gone");
    assert_eq!(run(&c, "test -e ~/.sparkbox/keys/d1 && echo there || echo gone").await.stdout.trim(), "gone", "the key file is gone");
    assert_eq!(run(&c, "test -e ~/.sparkbox/deploy/d1 && echo there || echo gone").await.stdout.trim(), "gone", "the launch files are gone");
    live.stop();
    reset(&c, s).await;
}

/// `launch-proof.sh` runs C-L2 on real Docker Desktop, so what it sends must be what Rust writes,
/// minus what Docker Desktop can't do (`--gpus`) and the plan leaves out (`--privileged`). The
/// script's two lines keep `$MODEL` and `$IMAGE_ID` as shell words, the spec's folder and image.
#[test]
fn launch_proof_sends_what_rust_writes() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("scripts/staging/launch-proof.sh");
    let script = std::fs::read_to_string(&script).unwrap_or_else(|e| panic!("{}: {e}", script.display()));
    let spec = DeploySpec {
        id: "staging-launch".into(),
        served_name: "sbproof".into(),
        model_path: "$MODEL".into(),
        nodes: vec!["local".into()],
        port: 18999,
        max_model_len: Some(32768),
        image: "sparkbox-staging-launch:throwaway".into(),
        image_ref: "sparkbox-staging-launch:throwaway".into(),
        image_id: "$IMAGE_ID".into(),
        ..Default::default()
    };
    let several = DeploySpec { nodes: vec!["local".into(), "other".into()], tensor_parallel: 2, docker_access: "privileged".into(), ..spec.clone() };
    let lines: Vec<&str> = script.lines().map(str::trim).collect();
    for (what, line) in [("the single rank", rank_command_for(&spec, 0, "dev")), ("the worker", rank_command_for(&several, 1, "dev"))] {
        let line = line.replace(" --gpus all", "").replace(" --privileged", "").replace(&crate::models::plan::launcher_name(), "$LAUNCH_NAME");
        assert!(lines.contains(&line.as_str()), "{what}'s line in launch-proof.sh isn't Rust's. Rust writes:\n{line}");
    }
}

/// A load on two nodes under real dash and real Python 3.12, through `Live`. One node stands for
/// both, so its two ranks share a home; docker is the stand-in.
/// - C-P2 for two nodes on real `ss`, with the rendezvous port in its filter: a listener on 29501 is
///   a problem, and a Ray worker's `ray start --address` command is found by real grep;
/// - C-F: a worker's files have no key and refuse one, each rank's argv and env are its own (C-V,
///   C-E), and a refusal writes nothing;
/// - C-L2: the worker, then the head, each docker's argv word for word (`--privileged`, no key mount
///   on the worker, `--key` exactly with the mount on the head);
/// - C-J: real ping with its flags, through `jumbo_script` and `jumbo_failures`. Loopback's 64 KiB
///   MTU carries 8972 bytes with don't-fragment set; an interface this node doesn't have, and an
///   address that doesn't answer, read as failed. The container has no fabric and every link here
///   carries jumbo frames: this proves the command and how it's read, not a small MTU's refusal, and
///   not the Sparks' path;
/// - C-U2 takes both ranks and their files, and only this load's.
#[tokio::test]
#[ignore]
async fn staging_multi_node_shapes() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    first_light_setup(&c).await;
    let node = "staging-multi";
    let dir = data_dir("staging-multi");
    let (live, _) = staging_live(s, &dir, node).await;
    let (spec, nets) = (multi_spec(), multi_nets());
    live.ensure_file(node, "launch.py", load_plan::LAUNCH).await.unwrap();

    // C-P2 before: nothing in the way. On two nodes it also reads the rendezvous port.
    let read = preflight_script(&spec);
    assert!(read.contains("( sport = :8000 or sport = :29501 )"), "{read}");
    let before = live.run(node, &read, None, LIMIT).await.unwrap();
    assert_eq!(before.code, Some(0), "{}", before.stderr);
    say(&format!("staging_multi_node_shapes: C-P2 for two nodes on the staging node:\n{}", preflight_raw(&before.stdout)));
    let p = parse_preflight("cell1", &spec, &before.stdout);
    assert!(p.problems.is_empty() && p.port_free && p.store.is_some(), "{p:?}");
    let held = "nohup python3 -c 'import socket, time; s = socket.socket(); s.bind((\"127.0.0.1\", 29501)); s.listen(1); time.sleep(30)' >/dev/null 2>&1 &";
    run(&c, held).await;
    let mut busy = false;
    for _ in 0..50 {
        let out = live.run(node, &read, None, LIMIT).await.unwrap();
        busy = !parse_preflight("cell1", &spec, &out.stdout).port_free;
        if busy {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(busy, "real ss found the listener on 29501");
    run(&c, "pkill -f '[s].bind' ; true").await;

    // C-F: the worker's files. A key is refused before anything is written.
    let refused = live.sparkjob(node, "deploy-files d1 worker", Some(json!({ "argv": rank_argv(&spec, 1, &nets), "env": rank_env(&spec, 1, &nets), "key": "sbk-x" }).to_string().as_bytes()), LIMIT).await;
    assert!(refused.as_ref().is_err_and(|e| e.contains("A worker takes no API key")), "{refused:?}");
    let out_of_line = live.sparkjob(node, "deploy-files d1 worker", Some(json!({ "argv": ["sh", "-c", "id"], "env": {}, "key": null }).to_string().as_bytes()), LIMIT).await;
    assert!(out_of_line.as_ref().is_err_and(|e| e.contains("vllm serve and nothing else")), "{out_of_line:?}");
    assert_eq!(run(&c, "test -e ~/.sparkbox/deploy/d1 && echo there || echo none").await.stdout.trim(), "none", "a refusal writes nothing");
    let printed = live.sparkjob(node, "deploy-files d1 worker", Some(launch_payload(&spec, 1, &nets, Some("sbk-only-rank-0")).to_string().as_bytes()), LIMIT).await.unwrap();
    assert_eq!(printed, json!({ "dir": "/home/sparkuser/.sparkbox/deploy/d1" }));
    assert_eq!(run(&c, "test -e ~/.sparkbox/keys/d1 && echo there || echo none").await.stdout.trim(), "none", "launch_payload gives a worker no key");
    let env: Value = serde_json::from_str(&run(&c, "cat ~/.sparkbox/deploy/d1/env.json").await.stdout).unwrap();
    assert_eq!((&env["VLLM_HOST_IP"], &env["NCCL_IB_HCA"], &env["GLOO_SOCKET_IFNAME"]), (&json!("192.168.100.2"), &json!("rocep1s0f1,roceP2p1s0f1"), &json!("enp1s0f1np1")), "the rank's own fabric: {env}");
    let words: Vec<String> = serde_json::from_str(&run(&c, "cat ~/.sparkbox/deploy/d1/argv.json").await.stdout).unwrap();
    assert_eq!(words, rank_argv(&spec, 1, &nets));
    assert!(words.iter().any(|w| w == "--headless") && !words.iter().any(|w| w == "--port"), "{words:?}");

    // C-L2: the worker first, then the head.
    let worker = live.run(node, &rank_command(&spec, 1), None, LIMIT).await.unwrap();
    assert_eq!(worker.code, Some(0), "the worker under dash: {}", worker.stderr);
    let key = format!("sbk-{}", sha256_hex(b"staging several nodes"));
    let printed = live.sparkjob(node, "deploy-files d1 head", Some(launch_payload(&spec, 0, &nets, Some(&key)).to_string().as_bytes()), LIMIT).await.unwrap();
    assert_eq!(printed, json!({ "dir": "/home/sparkuser/.sparkbox/deploy/d1" }));
    assert_eq!(run(&c, "stat -c '%a' ~/.sparkbox/keys/d1").await.stdout.trim(), "400");
    let head = live.run(node, &rank_command(&spec, 0), None, LIMIT).await.unwrap();
    assert_eq!(head.code, Some(0), "the head under dash: {}", head.stderr);
    let calls = docker_calls(&c).await;
    let started: Vec<Vec<String>> = calls.iter().map(argv).filter(|a| a.first().map(String::as_str) == Some("run")).collect();
    let want = [
        c_l2_words(&Words { name: "sparkbox-d1-r1", role: "worker", rank: 1, recipe: "gemma-3-27b-it", parallel: "2x1", listen: false, privileged: true, key: false }),
        c_l2_words(&Words { name: "sparkbox-d1-r0", role: "head", rank: 0, recipe: "gemma-3-27b-it", parallel: "2x1", listen: true, privileged: true, key: true }),
    ];
    assert_eq!(started, want, "docker's argv under real dash, worker then head");
    let seen = serde_json::to_string(&calls).unwrap();
    assert!(!seen.contains(&key) && live.audit.entries().iter().all(|e| !e.command.contains(&key)), "the key reached the file and nothing else");

    // C-P2 with the load up: both ranks are listed, and a Ray worker's command is found.
    let plant = format!(
        "docker run -d --name raylet --entrypoint /bin/sh {} -c 'ray start --address=192.168.100.1:6379 --block' >/dev/null",
        image_id()
    );
    assert_eq!(run(&c, &plant).await.code, Some(0));
    let during = live.run(node, &read, None, LIMIT).await.unwrap();
    let q = parse_preflight("cell1", &spec, &during.stdout);
    let mut names: Vec<&str> = q.leftovers.iter().map(|l| l.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["sparkbox-d1-r0", "sparkbox-d1-r1"], "{:?}", q.leftovers);
    assert_eq!(q.ray_leftovers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["raylet"], "{}", during.stdout);

    // C-J, on real ping. Loopback's MTU is 65536, and nothing answers on TEST-NET-1.
    let script = jumbo_script(&[("lo".into(), "127.0.0.1".into()), ("lo".into(), "192.0.2.1".into()), ("enp1s0f1np1".into(), "192.168.100.1".into())]);
    assert!(script.starts_with("ping -c 2 -W 2 -M do -s 8972 -I lo 127.0.0.1 >/dev/null 2>&1 && echo half1 ok || echo half1 failed; "), "{script}");
    let jumbo = live.run(node, &script, None, LIMIT).await.unwrap();
    say(&format!("staging_multi_node_shapes: C-J on real ping: {:?}", jumbo.stdout));
    assert_eq!(lines(&jumbo.stdout), ["half1 ok", "half2 failed", "half3 failed"], "{}", jumbo.stderr);
    assert_eq!(jumbo_failures(&jumbo.stdout, 3), [1, 2]);

    // C-U2 takes this load and leaves another's.
    let other = rank_command(&DeploySpec { id: "d2".into(), ..first_light_spec() }, 0);
    assert_eq!(run(&c, &other).await.code, Some(0));
    let unload = live.run(node, &unload_command("d1"), None, LIMIT).await.unwrap();
    assert_eq!((unload.code, unload.stdout.trim()), (Some(0), "ok"), "{}", unload.stderr);
    let left = lines(&run(&c, "docker ps -a --format '{{.Names}}'").await.stdout);
    assert_eq!(left, ["raylet", "sparkbox-d2"], "only this load's containers went");
    assert_eq!(run(&c, "ls -A ~/.sparkbox/keys ~/.sparkbox/deploy 2>/dev/null | grep -c d1").await.stdout.trim(), "0", "its key and files went");
    live.stop();
    reset(&c, s).await;
}

/// Session B step 27's page-cache drop (C-D, `plan::DROP_CACHES`) on real sudo 1.9.15 with
/// `Defaults use_pty`, through SparkBox's `Live::sudo` (the password goes to sudo on stdin, never
/// in a command line, the activity log or ssh.log) and through `Models::deploy` with the owner's
/// consent (`drop_caches`), which runs the drop when the pre-flight says vLLM would refuse to
/// start. Docker mounts the container's /proc/sys read-only: the drop then fails, and SparkBox
/// says so in plain words, before anything starts. With /proc/sys made writable for the
/// container (the test's own `mount -o remount,bind,rw`), it is a real write: the kernel drops
/// the page cache (Docker Desktop's VM's: harmless, the kernel refills it), and deploy reads the
/// node again and refuses with what vLLM would still need (a Docker VM never has room for share
/// 0.95). No load starts either way: docker is never called with `run`.
#[tokio::test]
#[ignore]
async fn staging_session_b_step27_drop_caches() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    first_light_setup(&c).await;
    let node = "staging-step27-drop";
    let dir = data_dir("staging-step27-drop");
    let (live, inventory) = staging_live(s, &dir, node).await;

    // Read-only, as Docker leaves it.
    let ro = live.sudo(node, DROP_CACHES, LIMIT).await.expect("sudo ran it");
    assert_eq!(ro.code, Some(2), "{ro:?}");
    assert_eq!(ro.stderr.trim(), "sh: 1: cannot create /proc/sys/vm/drop_caches: Read-only file system");
    let row = rows(&live, DROP_CACHES);
    assert!(row.len() == 1 && row[0].escalated && row[0].command == DROP_CACHES, "one sudo row, the command as written: {row:?}");

    // Through the Load flow: the owner ticked the drop; the read-only /proc/sys stops it in plain words.
    let llm = crate::llm::Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
    let models = crate::models::Models::new(dir.clone(), live.clone(), llm, None);
    let ctx = || crate::models::Ctx {
        live: true,
        settings: crate::models::ModelSettings { model_root: "~/models".into(), ..Default::default() },
        inventory: inventory.clone(),
        snapshot: Some(live.snapshot(&inventory)),
    };
    let spec = DeploySpec { nodes: vec![node.into()], host: String::new(), gpu_memory_utilization: 0.95, drop_caches: true, override_memory: true, ..first_light_spec() };
    let total: u64 = run(&c, "grep MemTotal /proc/meminfo").await.stdout.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() * 1024;
    assert!((total as f64 * 0.95) as u64 + load_plan::FREE_HEADROOM > total, "the Docker VM ({total} bytes) can never fit share 0.95, so no load can start");
    let refused = models.deploy(&ctx(), spec.clone()).await.unwrap_err();
    assert_eq!(refused, format!("{node}: couldn't drop the page cache: sh: 1: cannot create /proc/sys/vm/drop_caches: Read-only file system"));

    // Writable: a real drop.
    sudo_ok(&c, s, "mount -o remount,bind,rw /proc/sys").await;
    let cached = |out: &str| out.lines().find_map(|l| l.strip_prefix("Cached:")).and_then(|v| v.trim().trim_end_matches(" kB").trim().parse::<u64>().ok()).unwrap();
    // Something in the page cache to drop: the dummy repo's 20 MiB package, read once.
    run(&c, &format!("cat {REPO}/pool/stable/google-chrome-stable_142.0.7444.59-1_all.deb > /dev/null")).await;
    let was = cached(&run(&c, "cat /proc/meminfo").await.stdout);
    let rw = live.sudo(node, DROP_CACHES, LIMIT).await.expect("sudo ran it");
    assert_eq!((rw.code, rw.stderr.trim()), (Some(0), ""), "{rw:?}");
    let now = cached(&run(&c, "cat /proc/meminfo").await.stdout);
    say(&format!("staging_session_b_step27_drop_caches: a real drop in Docker Desktop's VM: Cached {was} kB -> {now} kB"));
    assert!(now < was, "the page cache was dropped: Cached {was} kB -> {now} kB");
    let refused = models.deploy(&ctx(), spec).await.unwrap_err();
    assert!(refused.starts_with(&format!("{node} has ")) && refused.contains(" free after dropping the page cache; vLLM at share 0.95 needs ") && refused.ends_with("free (4 GiB of it headroom). Lower the share or free memory."), "{refused}");

    // Every drop was one sudo row; the password is nowhere SparkBox writes.
    let drops = rows(&live, DROP_CACHES);
    assert_eq!(drops.len(), 4, "{drops:?}");
    assert!(drops.iter().all(|e| e.escalated && e.command == DROP_CACHES), "{drops:?}");
    assert!(live.audit.entries().iter().all(|e| !e.command.contains(&s.password) && e.error.as_deref().is_none_or(|x| !x.contains(&s.password))), "the password isn't in the activity log");
    assert!(!std::fs::read_to_string(dir.join("ssh.log")).unwrap().contains(&s.password), "the password isn't in ssh.log");
    assert!(docker_calls(&c).await.iter().all(|call| argv(call).first().map(String::as_str) != Some("run")), "nothing was started");
    live.stop();
    reset(&c, s).await;
}

// ── round 4: the plan binding (decision b) and the restart variants ────────────────────────

/// C1′'s apt options, as contract C1′ writes them (no download limit).
const APT_OPTS: &str = "-o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold";

/// Contract C2′ for `pins` (`name=version`), byte for byte.
fn c2_prime(pins: &[&str]) -> String {
    let quoted: Vec<String> = pins.iter().map(|p| sh_quote(p)).collect();
    format!("apt-get -s {APT_OPTS} -y --no-remove install --only-upgrade {}", quoted.join(" "))
}

/// Contract C1′ for `unit` logging to `log` under `log_dir`, `pins` installed, byte for byte.
fn c1_prime(unit: &str, log_dir: &str, log: &str, pins: &[&str]) -> String {
    let quoted: Vec<String> = pins.iter().map(|p| sh_quote(p)).collect();
    let inner = format!(
        "trap : TERM; env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get {APT_OPTS} -y --no-remove install --only-upgrade {}; code=$?; echo SPARKBOX-AFTER; apt-get check; needrestart -b -r l; echo SPARKBOX-AFTER-END; echo \"SPARKBOX-EXIT $code\"; exit $code",
        quoted.join(" ")
    );
    format!(
        "mkdir -p {d} && chmod 0755 {d} && dpkg-query -W > {before} && systemd-run --unit {unit} --collect -p StandardOutput=append:{log} -p StandardError=append:{log} /bin/sh -c {}",
        sh_quote(&inner),
        d = sh_quote(log_dir),
        before = sh_quote(&dpkg_before_path(log_dir, unit))
    )
}

/// A dry run's plan as (name, from, to, new), sorted by name.
fn plan_of(sim: &crate::ops::updates::SimulatedUpgrade) -> Vec<(String, Option<String>, String, bool)> {
    let mut plan: Vec<_> = sim.plan.iter().map(|p| (p.name.clone(), p.from.clone(), p.to.clone(), p.new)).collect();
    plan.sort();
    plan
}

/// Publishes into the dummy repo the way an archive does it: `to_pool` from pool/later replaces
/// `from_pool` in noble-updates (an Ubuntu pocket lists one version of a package), the indexes
/// are made again and apt reads them (`apt-get update`). The reset puts the repo back.
async fn publish(c: &Session, s: &Staging, newer: &str, older: &str) {
    sudo_ok(c, s, &format!("mv {REPO}/pool/later/{newer} {REPO}/pool/noble-updates/ && mv {REPO}/pool/noble-updates/{older} {REPO}/pool/later/ && sparkbox-staging-index && apt-get -q update >/dev/null")).await;
}

/// What an upgrade that was refused left behind on the node: nothing (no unit, no log, no package list).
async fn nothing_started(c: &Session, live: &Live) {
    assert!(live.audit.entries().iter().all(|e| !e.escalated), "nothing ran as root: {:?}", live.audit.entries().iter().filter(|e| e.escalated).map(|e| &e.command).collect::<Vec<_>>());
    let left = run(c, &format!("systemctl list-units --all --no-legend 'sparkbox-upgrade-*'; ls {OPS_LIB}/logs 2>/dev/null")).await.stdout;
    assert_eq!(left.trim(), "", "no unit, no log, no package list");
}

/// Decision (b) on real apt, through SparkBox's `Ops`: an upgrade installs exactly the plan its
/// dry run showed. The dummy sparkbox-bind 2.0 needs a new package (sparkbox-bind-dep): the plan
/// shows both; C1′ pins only the installed one (`sparkbox-bind=2.0`), new dependencies left to apt;
/// just before the start the dry run of exactly that (C2′, as the cluster user) shows the same plan,
/// and the upgrade installs those versions and nothing else. Then the repo moves between the dry
/// run and Upgrade, as a real archive does: sparkbox-bind 3.0 published in place of 2.0 (apt no
/// longer offers 2.0), or the dependency's 1.1 in place of 1.0 (same pin, another plan): each time
/// SparkBox refuses in plain words and starts nothing. A plan with a removal is refused before
/// anything runs as root; C1′ forced through on such a plan stops on `--no-remove` (apt exits 100,
/// "E: Packages need to be removed but remove is disabled.") and changes nothing.
#[tokio::test]
#[ignore]
async fn staging_plan_binding() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let node = "staging-bind";
    let dir = data_dir("staging-bind");
    let bind = vec!["sparkbox-bind".to_string()];
    let extra = |name: &str| format!("{REPO}/pool/extra/{name}_1.0_all.deb");

    // The plan as shown, then installed exactly.
    dpkg_install(&c, s, &extra("sparkbox-bind")).await;
    let (live, inventory) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    let sim = ops.updates_simulate(false, node, &bind).await.unwrap();
    assert_eq!(plan_of(&sim), [("sparkbox-bind".into(), Some("1.0".into()), "2.0".into(), false), ("sparkbox-bind-dep".into(), None, "1.0".into(), true)], "{}", sim.text);
    assert!(sim.removes.is_empty() && sim.refused.is_none(), "{sim:?}");
    let id = ops.updates_upgrade_opts(false, node.into(), bind.clone(), UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
    let t = wait_task(&ops, &id, 120).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}\n{}", t.error, t.log.join("\n"));
    assert_eq!((installed_version(&c, "sparkbox-bind").await, installed_version(&c, "sparkbox-bind-dep").await), ("2.0".into(), "1.0".into()), "the shown versions, and the new dependency");
    let run_ = ops.runs().into_iter().find(|r| r.task_id == id).expect("the upgrade's record");
    let log_dir = format!("{OPS_LIB}/logs");
    let started: Vec<_> = live.audit.entries().into_iter().filter(|e| e.escalated).collect();
    assert_eq!(started.len(), 1, "one root command: {:?}", started.iter().map(|e| &e.command).collect::<Vec<_>>());
    assert_eq!(started[0].command, c1_prime(&run_.unit, &log_dir, &run_.log_path, &["sparkbox-bind=2.0"]), "C1′, the installed package pinned, the new dependency left to apt");
    assert_eq!(started[0].command, detached_command_opts(&run_.unit, &log_dir, &run_.log_path, &["sparkbox-bind=2.0".into()], &UpgradeOpts::default()));
    let entries: Vec<_> = live.audit.entries().into_iter().rev().collect();
    let c2 = entries.iter().position(|e| e.command == c2_prime(&["sparkbox-bind=2.0"])).expect("C2′ ran");
    let start = entries.iter().position(|e| e.escalated).unwrap();
    assert!(c2 < start && !entries[c2].escalated && entries[c2].code == Some(0), "C2′ ran as the cluster user just before the start: {:?}", entries[c2]);
    live.stop();
    reset(&c, s).await;

    // sparkbox-bind 3.0 published between the dry run and Upgrade: apt no longer offers 2.0.
    dpkg_install(&c, s, &extra("sparkbox-bind")).await;
    let (live, _) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    let sim = ops.updates_simulate(false, node, &bind).await.unwrap();
    publish(&c, s, "sparkbox-bind_3.0_all.deb", "sparkbox-bind_2.0_all.deb").await;
    let id = ops.updates_upgrade_opts(false, node.into(), bind.clone(), UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
    let t = wait_task(&ops, &id, 60).await;
    assert_eq!(
        (t.state, t.error.as_deref()),
        (TaskState::Failed, Some("apt's plan for staging-bind changed since the dry run you saw (apt-get -s: E: Version '2.0' for 'sparkbox-bind' was not found). Look at the new dry run and start again.")),
        "{}",
        t.log.join("\n")
    );
    nothing_started(&c, &live).await;
    assert_eq!(installed_version(&c, "sparkbox-bind").await, "1.0");
    live.stop();
    reset(&c, s).await;

    // The dependency's 1.1 published in its place: the same pin, another plan.
    dpkg_install(&c, s, &extra("sparkbox-bind")).await;
    let (live, _) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    let sim = ops.updates_simulate(false, node, &bind).await.unwrap();
    publish(&c, s, "sparkbox-bind-dep_1.1_all.deb", "sparkbox-bind-dep_1.0_all.deb").await;
    let id = ops.updates_upgrade_opts(false, node.into(), bind.clone(), UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
    let t = wait_task(&ops, &id, 60).await;
    assert_eq!(
        (t.state, t.error.as_deref()),
        (TaskState::Failed, Some("apt's plan for staging-bind changed since the dry run you saw: sparkbox-bind-dep now 1.1 (was 1.0). Look at the new dry run and start again.")),
        "{}",
        t.log.join("\n")
    );
    nothing_started(&c, &live).await;
    assert_eq!((installed_version(&c, "sparkbox-bind").await, installed_version(&c, "sparkbox-bind-dep").await), ("1.0".into(), String::new()));
    live.stop();
    reset(&c, s).await;

    // A plan that removes a package: refused; C1′ forced through stops on --no-remove.
    dpkg_install(&c, s, &format!("{} {}", extra("sparkbox-swap"), extra("sparkbox-swap-old"))).await;
    let (live, _) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    let swap = vec!["sparkbox-swap".to_string()];
    let sim = ops.updates_simulate(false, node, &swap).await.unwrap();
    let why = "This plan removes sparkbox-swap-old. SparkBox's one-node upgrades only upgrade; run it in a terminal if you mean it.";
    assert_eq!((sim.removes.clone(), sim.refused.as_deref()), (vec!["sparkbox-swap-old".to_string()], Some(why)), "{}", sim.text);
    let id = ops.updates_upgrade_opts(false, node.into(), swap.clone(), UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
    let t = wait_task(&ops, &id, 60).await;
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(why)), "{}", t.log.join("\n"));
    nothing_started(&c, &live).await;
    let unit = "sparkbox-upgrade-stg-noremove";
    let log = format!("{LOG_DIR}/upgrade-stg-noremove.log");
    sudo_ok(&c, s, &c1_prime(unit, LOG_DIR, &log, &["sparkbox-swap=2.0"])).await;
    let all = finished_log(&c, &log).await;
    assert_eq!(exit_of(&all), Some(100), "{}", all.join("\n"));
    assert!(all.iter().any(|l| l == "E: Packages need to be removed but remove is disabled."), "apt's own words:\n{}", all.join("\n"));
    assert_eq!((installed_version(&c, "sparkbox-swap").await, installed_version(&c, "sparkbox-swap-old").await), ("1.0".into(), "1.0".into()), "nothing changed");
    live.stop();
    reset(&c, s).await;
}

/// Session B step 19's variants on the real node, through `prereboot::check` over a Live (C6′):
/// a held DKMS package (`hi`) blocks; libnvidia-compute-580 removed with its config kept (`rc`)
/// blocks; /etc/default/grub without GRUB_DEFAULT is an ok step B (its row exits 0) with "not set"
/// as information. And a plan that would remove a GPU-stack package is refused before anything
/// runs as root: dummy upgrades that conflict with docker-ce and with the kernel that boots next.
#[tokio::test]
#[ignore]
async fn staging_prereboot_variants() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    let node = "staging-variants";
    let dir = data_dir("staging-variants");
    let (live, inventory) = staging_live(s, &dir, node).await;

    // A held DKMS package.
    dpkg_install(&c, s, &format!("{REPO}/pool/extra/nvidia-dkms-580_580.95.05-0ubuntu1_all.deb")).await;
    sudo_ok(&c, s, &hold_command(&["nvidia-dkms-580".into()], true)).await;
    assert!(run(&c, "dpkg -l nvidia-dkms-580 | tail -1").await.stdout.starts_with("hi  nvidia-dkms-580 "), "held and installed");
    let r = prereboot::check(&live, node).await.unwrap();
    assert_eq!(r.blocking, ["nvidia-dkms-580 is installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted."]);
    reset(&c, s).await;

    // libnvidia-compute-580 removed, its config kept: rc.
    sudo_ok(&c, s, &format!("dpkg -r libnvidia-compute-580 >/dev/null && {DROP_APT_CACHE}")).await;
    assert_eq!(run(&c, "dpkg-query -W -f='${db:Status-Abbrev}${Version}' libnvidia-compute-580").await.stdout, "rc 580.95.05-0ubuntu1");
    let r = prereboot::check(&live, node).await.unwrap();
    assert!(!r.ok, "{r:?}");
    assert_eq!(r.blocking, ["libnvidia-compute-580 is removed (only its config is left): the driver's userspace isn't installed. Not restarted."]);
    assert_eq!(detail(&r, "userspace"), "removed, only its config is left (dpkg status rc, 580.95.05-0ubuntu1)");
    reset(&c, s).await;

    // No GRUB_DEFAULT: step B's grep finds nothing, and its row still exits 0 (C6′'s `; true`).
    let step_b_before = rows(&live, "echo '--- initrd'").len();
    sudo_ok(&c, s, "sed -i '/^GRUB_DEFAULT=/d' /etc/default/grub").await;
    let r = prereboot::check(&live, node).await.unwrap();
    assert!(r.ok, "{:?}", r.blocking);
    assert!(detail(&r, "grub").starts_with("not set"), "{}", detail(&r, "grub"));
    // The activity log lists newest first: this check's row is the first, and the only new one.
    let step_b = rows(&live, "echo '--- initrd'");
    assert_eq!(step_b.len(), step_b_before + 1, "one step B row for this check: {step_b:?}");
    assert_eq!(step_b[0].code, Some(0), "step B's row didn't fail: {:?}", step_b[0]);
    reset(&c, s).await;

    // A GPU-stack removal: the dry run (C2) is refused by the GPU-stack guard, and so is Upgrade
    // bound to that plan, at C2′ (--no-remove stops apt before its plan lines; the guard reads
    // apt's list of what it would remove). Nothing runs as root.
    let ops = staging_ops(&live, &inventory);
    for (pkg, gone) in [("sparkbox-rm-docker", "docker-ce".to_string()), ("sparkbox-rm-kernel", format!("linux-image-{K}"))] {
        dpkg_install(&c, s, &format!("{REPO}/pool/extra/{pkg}_1.0_all.deb")).await;
        let sel = vec![pkg.to_string()];
        let sim = ops.updates_simulate(false, node, &sel).await.unwrap();
        let guard = format!("SparkBox can't upgrade GPU-stack packages yet: this plan removes {gone}");
        assert_eq!((sim.removes.clone(), sim.refused.as_deref()), (vec![gone.clone()], Some(guard.as_str())), "{}", sim.text);
        let id = ops.updates_upgrade_opts(false, node.into(), sel, UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
        let t = wait_task(&ops, &id, 60).await;
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(guard.as_str())), "{}", t.log.join("\n"));
        nothing_started(&c, &live).await;
        assert!(!installed_version(&c, &gone).await.is_empty(), "{gone} is still installed");
        reset(&c, s).await;
    }
    live.stop();
}

/// The upgrade follower against a real sshd that goes away (round 4, R4-UPD's follower fix): an
/// upgrade whose node stays away past the follower's wait is "lost", never forgotten. SparkBox's
/// `Ops` upgrades google-chrome-stable at 500 KB/s (about 35 s, bound to its dry run's plan);
/// while apt downloads, the staging node's sshd stops and every SSH session to it is cut (the
/// test's own `docker exec`, as root in the container). The follower waits its injected 3 s, the
/// task ends with the "Lost …" warning, and upgrades.json keeps the record as "lost", while apt
/// goes on in its unit. sshd comes back: the same `Ops` follows the run again ("… (picked up
/// again)") to apt's exit, Chrome is at the version shown, and the record leaves upgrades.json.
#[tokio::test]
#[ignore]
async fn staging_upgrade_lost_and_picked_up_again() {
    let (s, _lock) = staging_or_skip!();
    let c = connect(s).await;
    reset(&c, s).await;
    sudo_ok(&c, s, "rm -f /var/cache/apt/archives/google-chrome-stable_* /var/cache/apt/archives/partial/google-chrome-stable_*").await;
    let node = "staging-lost";
    let dir = data_dir("staging-lost");
    let (live, inventory) = staging_live(s, &dir, node).await;
    let ops = staging_ops(&live, &inventory);
    ops.set_upgrade_timing(Duration::from_secs(3), Duration::from_secs(1800));
    let chrome = vec!["google-chrome-stable".to_string()];
    let limited = UpgradeOpts { dl_limit_kbps: Some(500), ..Default::default() };
    let sim = ops.updates_simulate_opts(false, node, &chrome, &limited).await.unwrap();
    let id = ops.updates_upgrade_opts(false, node.into(), chrome.clone(), UpgradeArgs { dl_limit_kbps: Some(500), security_only: false, plan: Some(sim.shown()) }).unwrap();
    let first = run_in_phase(&ops, &id, "downloading", 60).await;

    // The node goes away: sshd stops listening, and every session to it (this test's too) is cut.
    // sshd comes back however the test ends.
    struct SshdBack;
    impl Drop for SshdBack {
        fn drop(&mut self) {
            let _ = docker_out(&["exec", CONTAINER, "systemctl", "start", "ssh.socket"]);
        }
    }
    let _back = SshdBack;
    drop(c);
    docker_out(&["exec", CONTAINER, "sh", "-c", "systemctl stop ssh.socket ssh.service; pkill -f '^sshd: sparkuser'; true"]).unwrap();
    let t = wait_task(&ops, &id, 60).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}\n{}", t.error, t.log.join("\n"));
    assert_eq!(t.summary.as_deref(), Some("Lost staging-lost while it was upgrading"));
    assert_eq!(
        t.warning.as_deref(),
        Some(format!("Lost staging-lost while it was upgrading; apt may still be running there. SparkBox looks again when staging-lost is back, and after a restart (log: {}).", first.log_path).as_str())
    );
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("upgrades.json")).unwrap()).unwrap();
    let kept = saved.as_array().and_then(|runs| runs.iter().find(|r| r["unit"] == first.unit.as_str())).unwrap_or_else(|| panic!("the record stays in upgrades.json: {saved}"));
    assert_eq!(kept["phase"], "lost", "{kept}");
    let unit = docker_out(&["exec", CONTAINER, "systemctl", "show", "-p", "ActiveState", "--", &first.unit]).unwrap();
    assert_eq!(unit.trim(), "ActiveState=active", "apt goes on in its unit");

    // sshd is back: the same SparkBox follows it again, to apt's end.
    docker_out(&["exec", CONTAINER, "systemctl", "start", "ssh.socket"]).unwrap();
    let title = "Upgrade google-chrome-stable on staging-lost (picked up again)";
    let mut again = None;
    for _ in 0..1200 {
        again = ops.tasks().list().into_iter().find(|t| t.title == title);
        if again.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let t = wait_task(&ops, &again.unwrap_or_else(|| panic!("no '{title}' task: {:?}", ops.tasks().list().iter().map(|t| &t.title).collect::<Vec<_>>())).id, 180).await;
    assert_eq!(t.state, TaskState::Ok, "{:?}\n{}", t.error, t.log.join("\n"));
    assert_eq!(t.summary.as_deref(), Some("The upgrade on staging-lost finished"));
    let c = connect(s).await;
    assert_eq!(installed_version(&c, "google-chrome-stable").await, "142.0.7444.59-1");
    let left = std::fs::read_to_string(dir.join("upgrades.json")).unwrap_or_default();
    assert!(!left.contains(&first.unit), "the record left upgrades.json once apt's exit was known: {left}");
    live.stop();
    reset(&c, s).await;
}

// ── round 5: node jobs on real rsync, real signals and real Linux modes ─────────────────────

/// Where these tests keep model folders: under the Library's root, so `Models::library` scans them.
const JOBS_ROOT: &str = "/home/sparkuser/models/sbstage";
/// What a fabric copy leaves out, as `Models::share` sends it (`models::COPY_EXCLUDE`).
const COPY_EXCLUDE: [&str; 3] = ["original/", "metal/", "*.pth"];
const RSYNC_SECRET: &str = "sbstage-rsync-throwaway";
const JOB_REPO: &str = "sbstage/model";
const JOB_REV: &str = "0123456789abcdef0123456789abcdef01234567";
/// The weights file a job is stopped partway through.
const BIG: &str = "model-00002-of-00002.safetensors";
/// Its size in a copy's source: sparse, so it's quick to make, and long enough that the pull is
/// still inside it when the freezer stops it.
const BIG_BYTES: u64 = 256 << 20;
const TOKEN_MOUNT: &str = "/run/sparkbox/hf_token";

/// The serve job's spec, as `Models::share` sends it, on 127.0.0.1.
fn serve_spec(path: &str, port: u16) -> Value {
    json!({ "path": path, "address": "127.0.0.1", "port": port, "password": RSYNC_SECRET, "allow": "127.0.0.1/32",
            "exclude": COPY_EXCLUDE, "ttl": 3600, "label": "Share sbstage/model" })
}

/// The pull job's spec, as `Models::share` sends it. The one node here is both ends, so the copy
/// goes to a folder of its own, where a real one keeps the source's path. `share` itself can't
/// run here: it needs a ConnectX-7 address and two nodes.
fn pull_spec(port: u16, dest: &str, files: &Value) -> Value {
    json!({ "src": format!("rsync://sparkbox@127.0.0.1:{port}/model"), "dest": dest, "password": RSYNC_SECRET, "files": files,
            "exclude": COPY_EXCLUDE, "checksums": false, "label": "Copy sbstage/model", "repo": JOB_REPO, "revision": JOB_REV,
            "instance": crate::identity::instance_name() })
}

/// One test's SparkBox on the staging node, and its own connection for looking.
struct JobRig {
    c: Session,
    live: Arc<Live>,
    inventory: Inventory,
    models: Arc<Models>,
    node: String,
}

/// Logs in, puts the node back and clears what a job test leaves in the cluster user's home: any
/// job, rsync, secret and model folder. A SIGKILL also ends a sender a test left frozen.
async fn job_rig(s: &Staging, node: &str) -> JobRig {
    let c = connect(s).await;
    reset(&c, s).await;
    clear_jobs(&c).await;
    let dir = data_dir(node);
    let (live, inventory) = staging_live(s, &dir, node).await;
    let llm = crate::llm::Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
    let models = Models::new(dir.clone(), live.clone(), llm, None);
    JobRig { c, live, inventory, models, node: node.into() }
}

async fn clear_jobs(c: &Session) {
    let clear = format!("pkill -9 -f '[s]parkjob.py'; pkill -9 -x rsync; rm -rf ~/.sparkbox/jobs ~/.sparkbox/rsyncd ~/.sparkbox/job-secrets ~/.cache/huggingface {JOBS_ROOT} /tmp/sbfreeze.*; true");
    assert_eq!(run(c, &clear).await.code, Some(0));
}

impl JobRig {
    async fn sparkjob(&self, args: &str, stdin: Option<&[u8]>) -> Value {
        self.live.sparkjob(&self.node, args, stdin, LIMIT).await.unwrap_or_else(|e| panic!("sparkjob {args}: {e}"))
    }

    async fn start(&self, kind: &str, id: &str, spec: &Value) {
        self.sparkjob(&format!("start {kind} {id}"), Some(spec.to_string().as_bytes())).await;
    }

    /// `sparkjob.py list`, which marks a dead job and sweeps its secrets.
    async fn list(&self) -> Vec<Value> {
        self.sparkjob("list", None).await.as_array().cloned().unwrap_or_default()
    }

    /// A job's status file, read without `list`, so it changes nothing.
    async fn job(&self, id: &str) -> Value {
        let raw = run(&self.c, &format!("cat ~/.sparkbox/jobs/{id}.json")).await.stdout;
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{id}'s status: {e}: {raw:?}"))
    }

    async fn until_state(&self, id: &str, state: &str, secs: u64) -> Value {
        for _ in 0..secs * 10 {
            let st = self.job(id).await;
            if st["state"] == state {
                return st;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{id} never got to {state}: {}", self.job(id).await);
    }

    /// Cancel, as the Jobs tab sends it.
    async fn cancel(&self, id: &str) {
        let ctx = self.ctx();
        self.models.cancel_job(&ctx, &self.node, id).await.unwrap_or_else(|e| panic!("cancel {id}: {e}"));
    }

    fn ctx(&self) -> crate::models::Ctx {
        crate::models::Ctx {
            live: true,
            settings: crate::models::ModelSettings { model_root: "~/models".into(), ..Default::default() },
            inventory: self.inventory.clone(),
            snapshot: Some(self.live.snapshot(&self.inventory)),
        }
    }

    /// This node's copy of a folder in the Library, as the app reads it (`scan` over SSH).
    async fn copy(&self, key: &str) -> crate::models::Copy {
        let library = self.models.library(&self.ctx()).await.unwrap();
        let entry = library.into_iter().find(|e| e.key == key).unwrap_or_else(|| panic!("{key} isn't in the Library"));
        entry.copies.get(&self.node).cloned().unwrap_or_else(|| panic!("{key} has no copy on {}", self.node))
    }

    async fn exists(&self, path: &str) -> bool {
        run(&self.c, &format!("test -e {path} && echo there")).await.stdout.trim() == "there"
    }

    /// SIGKILL to a job's whole process group: the job and what it started, with no chance to
    /// clean up, as the OOM killer or a lost node leaves it.
    async fn kill_group(&self, st: &Value) {
        let pid = st["pid"].as_u64().unwrap_or_else(|| panic!("no pid in {st}"));
        assert_eq!(run(&self.c, &format!("kill -9 -{pid}")).await.code, Some(0));
        assert!(!self.running(pid).await, "the job's process {pid} is gone");
    }

    /// Whether `pid` is a live process (a zombie isn't, and goes when its parent reaps it).
    async fn running(&self, pid: u64) -> bool {
        for _ in 0..50 {
            let state = run(&self.c, &format!("ps -o state= -p {pid}")).await.stdout;
            if state.trim().is_empty() || state.trim() == "Z" {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        true
    }

    /// Whether something listens on the port (`ss`, as a load's pre-flight reads it).
    async fn listening(&self, port: u16) -> bool {
        !run(&self.c, &format!("ss -ltnH 'sport = :{port}'")).await.stdout.trim().is_empty()
    }

    /// Starts a serve job and waits until its rsync answers. It's the real one: the job's child is
    /// Ubuntu's /usr/bin/rsync as a daemon, with a stand-in nowhere on the path. Returns its pid.
    async fn serve(&self, id: &str, path: &str, port: u16) -> u64 {
        self.start("serve", id, &serve_spec(path, port)).await;
        for _ in 0..100 {
            if let Some(pid) = self.job(id).await["childPid"].as_u64() {
                if self.listening(port).await {
                    let exe = run(&self.c, &format!("readlink /proc/{pid}/exe; command -v rsync; dpkg -S /usr/bin/rsync")).await.stdout;
                    assert_eq!(lines(&exe), ["/usr/bin/rsync", "/usr/bin/rsync", "rsync: /usr/bin/rsync"], "the distribution's rsync");
                    assert!(argv_of(&self.c, pid).await.iter().any(|w| w == "--daemon"), "a daemon");
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{id} never served port {port}: {}", self.job(id).await);
    }

    /// The files of a folder, as `Models::share` reads them for a pull's check.
    async fn manifest(&self, path: &str) -> Value {
        self.sparkjob(&format!("manifest {}", sh_quote(path)), None).await["files"].clone()
    }
}

/// A process's arguments, one a line in /proc: what really runs, not what a test thinks it sent.
async fn argv_of(c: &Session, pid: u64) -> Vec<String> {
    lines(&run(c, &format!("tr '\\0' '\\n' < /proc/{pid}/cmdline")).await.stdout)
}

/// A model folder as the Library knows one: a source's own record, a config, a small weights file,
/// the big one `BIG`, and a checkpoint a copy leaves out.
async fn make_model(c: &Session, path: &str, big: u64) {
    let out = run(c, &format!(
        "mkdir -p {path}/original && printf '{{\"architectures\": [\"Sbstage\"]}}' > {path}/config.json \
         && head -c 1048576 /dev/urandom > {path}/model-00001-of-00002.safetensors && truncate -s {big} {path}/{BIG} && printf x > {path}/original/skipped.bin \
         && printf '{{\"repo\": \"{JOB_REPO}\", \"revision\": \"source\", \"files\": [], \"checked\": \"sha256\", \"verifiedAt\": 1}}' > {path}/.sparkbox.json"
    )).await;
    assert_eq!(out.code, Some(0), "{}", out.stderr);
}

/// Stops a pull partway on any machine. On the node, this watches the target for rsync's temporary
/// copy of `BIG` (leaving alone one a killed pull left) and, at its first bytes, sends SIGSTOP to
/// the rsync server's connection processes: the pull stands still in the middle of that file, for
/// as long as a test wants to look. It's ready before the pull starts, and says which it stopped.
const FREEZER: &str = r#"
import glob, os, signal, subprocess, sys, time
dest, name, server = sys.argv[1:4]
pattern = glob.escape(dest) + "/." + glob.escape(name) + ".*"
old = set(glob.glob(pattern))
open("/tmp/sbfreeze.ready", "w").close()
deadline = time.time() + 90
while time.time() < deadline:
    for part in set(glob.glob(pattern)) - old:
        try:
            if os.path.getsize(part) == 0:
                continue
        except OSError:
            continue
        pids = subprocess.run(["pgrep", "-P", server], capture_output=True, text=True).stdout.split()
        for pid in pids:
            os.kill(int(pid), signal.SIGSTOP)
        with open("/tmp/sbfreeze.tmp", "w") as f:
            f.write(" ".join(pids))
        os.rename("/tmp/sbfreeze.tmp", "/tmp/sbfreeze.pids")
        sys.exit(0)
    time.sleep(0.001)
"#;

/// Starts a pull with the freezer armed, and returns once the sender is stopped mid-`BIG`: its pids.
async fn pull_frozen(r: &JobRig, id: &str, port: u16, dest: &str, files: &Value, server: u64) -> String {
    let c = &r.c;
    assert_eq!(c.exec(&wire("rm -f /tmp/sbfreeze.*; cat > /tmp/sbfreeze.py"), Some(FREEZER.as_bytes()), LIMIT).await.unwrap().code, Some(0));
    run(c, &format!("nohup python3 /tmp/sbfreeze.py {dest} {BIG} {server} >/tmp/sbfreeze.log 2>&1 &")).await;
    for _ in 0..100 {
        if r.exists("/tmp/sbfreeze.ready").await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    r.start("pull", id, &pull_spec(port, dest, files)).await;
    for _ in 0..1200 {
        let pids = run(c, "cat /tmp/sbfreeze.pids 2>/dev/null").await.stdout;
        if !pids.trim().is_empty() {
            return pids.trim().to_string();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the pull never got into {BIG}: {} | {}", r.job(id).await, run(c, "cat /tmp/sbfreeze.log").await.stdout);
}

/// Lets the frozen sender go: its client is gone, so it ends. Waits until the server has no
/// connection left.
async fn thaw(r: &JobRig, pids: &str, server: u64) {
    run(&r.c, &format!("kill -CONT {pids}")).await;
    for _ in 0..200 {
        if run(&r.c, &format!("pgrep -P {server}")).await.stdout.trim().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the rsync server still has a connection process");
}

/// D1 under real signals. A serve job on a real `rsync --daemon`: the password file is 0600 and
/// works (the right password lists the folder, a wrong one is refused). Cancel, as the Jobs tab
/// sends it: when it returns, the secrets folder is gone, the port is closed, a client is refused,
/// and the job and its rsync have really stopped. A second serve job gets SIGKILL to its whole
/// process group, so no `finally` runs: its folder outlives it until the next `list`, which
/// marks the job failed and removes it.
#[tokio::test]
#[ignore]
async fn staging_cancel_leaves_no_rsync_password() {
    let (s, _lock) = staging_or_skip!();
    let r = job_rig(s, "staging-serve-cancel").await;
    let folder = format!("{JOBS_ROOT}/served");
    make_model(&r.c, &folder, 1 << 20).await;

    let (id, port) = ("share-sbcancel-serve", 8741);
    let secrets = format!("~/.sparkbox/rsyncd/{id}");
    let server = r.serve(id, &folder, port).await;
    let st = r.job(id).await;
    let job_pid = st["pid"].as_u64().unwrap();
    assert_eq!(run(&r.c, &format!("stat -c '%a %U' {secrets}/secrets")).await.stdout.trim(), "600 sparkuser");
    assert_eq!(run(&r.c, &format!("cat {secrets}/secrets")).await.stdout, format!("sparkbox:{RSYNC_SECRET}\n"));
    let list_with = |password: &str| format!("RSYNC_PASSWORD={} rsync rsync://sparkbox@127.0.0.1:{port}/model/", sh_quote(password));
    let good = run(&r.c, &list_with(RSYNC_SECRET)).await;
    assert_eq!(good.code, Some(0), "{}", good.stderr);
    assert!(good.stdout.contains("config.json"), "{}", good.stdout);
    let bad = run(&r.c, &list_with("not-the-password")).await;
    assert_eq!(bad.code, Some(5), "{}", bad.stderr);
    assert!(bad.stderr.contains("auth failed on module model"), "{}", bad.stderr);

    r.cancel(id).await;
    assert_eq!(r.job(id).await["state"], "cancelled");
    assert!(!r.exists(&secrets).await, "no password file outlives Cancel: {}", run(&r.c, "ls -la ~/.sparkbox/rsyncd").await.stdout);
    assert!(!r.listening(port).await, "the port is closed");
    let refused = run(&r.c, &list_with(RSYNC_SECRET)).await;
    assert_eq!(refused.code, Some(10), "{}", refused.stderr);
    assert!(refused.stderr.contains("Connection refused"), "{}", refused.stderr);
    assert!(!r.running(server).await, "the rsync server stopped");
    assert!(!r.running(job_pid).await, "the job stopped");

    // SIGKILL: nothing runs in the job, so the file stays until `list` finds the job dead.
    let (id, port) = ("share-sbkilled-serve", 8751);
    let secrets = format!("~/.sparkbox/rsyncd/{id}");
    let server = r.serve(id, &folder, port).await;
    assert!(r.exists(&format!("{secrets}/secrets")).await);
    r.kill_group(&r.job(id).await).await;
    assert!(!r.running(server).await, "SIGKILL reached the rsync server: it's in the job's group");
    assert!(!r.listening(port).await);
    assert!(r.exists(&secrets).await, "no code ran after SIGKILL, so the folder is still there");
    let listed = r.list().await;
    assert_eq!(listed.iter().find(|j| j["id"] == id).unwrap()["state"], "failed", "{listed:?}");
    assert!(!r.exists(&secrets).await, "the next list removed a dead job's password file");
    say("staging_cancel_leaves_no_rsync_password: real rsync --daemon; Cancel (SIGTERM) left no folder, closed the port and stopped both processes; SIGKILL's folder went at the next list.");

    clear_jobs(&r.c).await;
    r.live.stop();
    reset(&r.c, s).await;
}

/// D3 and D4 with real rsync and real signals, on 127.0.0.1. The target starts with a stale record
/// from an earlier copy. A pull is stopped partway through a file, at a place the test holds it,
/// and it reads `copying` while its job lives. Its rsync's arguments, from /proc, are C-P's. SIGKILL
/// to its group: the Library says stopped and unverified, and neither the stale record nor the
/// source's is on the target. A second pull, frozen the same way, is Canceled: real rsync's
/// `--partial` keeps what it had under the file's own name, and it still reads stopped. A third
/// pull's file check fails on purpose (one size is wrong), which shows the order: rsync ends
/// well, `--delete` takes a stray file and leaves the marker, and so the folder still can't be
/// used. A fourth finishes: the bytes match, the marker's gone, and the target's own record says
/// `checked: "sizes"`.
#[tokio::test]
#[ignore]
async fn staging_partial_copy_retries() {
    let (s, _lock) = staging_or_skip!();
    let r = job_rig(s, "staging-partial").await;
    let source = format!("{JOBS_ROOT}/source");
    let dest = format!("{JOBS_ROOT}/target");
    make_model(&r.c, &source, BIG_BYTES).await;
    let stale = format!("mkdir -p {dest} && printf '{{\"repo\": \"sbstage/stale\", \"verifiedAt\": 2}}' > {dest}/.sparkbox.json");
    assert_eq!(run(&r.c, &stale).await.code, Some(0));
    let (record, marker) = (format!("{dest}/.sparkbox.json"), format!("{dest}/.sparkbox.partial"));

    let (serve_id, port) = ("share-sbpartial-serve", 8742);
    let server = r.serve(serve_id, &source, port).await;
    let files = r.manifest(&source).await;
    assert!(files.as_array().unwrap().iter().all(|f| f["path"] != ".sparkbox.json"), "{files}");

    // SIGKILL partway.
    let first = "share-sbpartial-a";
    let frozen = pull_frozen(&r, first, port, &dest, &files, server).await;
    let st = r.job(first).await;
    assert_eq!((&st["state"], &st["kind"], st["dest"].as_str()), (&json!("running"), &json!("pull"), Some(dest.as_str())), "{st}");
    let c_p: Vec<String> = [
        "rsync", "-aL", "--partial", "--delete", "--exclude=original/", "--exclude=metal/", "--exclude=*.pth", "--exclude=/.sparkbox.json", "--exclude=/.sparkbox.partial", "--exclude=/.cache/",
        &format!("rsync://sparkbox@127.0.0.1:{port}/model/"), &format!("{dest}/"),
    ]
    .iter()
    .map(|w| w.to_string())
    .collect();
    assert_eq!(argv_of(&r.c, st["childPid"].as_u64().unwrap()).await, c_p, "C-P's pull, as the kernel has it");
    let mark: Value = serde_json::from_str(&run(&r.c, &format!("cat {marker}")).await.stdout).unwrap();
    assert_eq!((&mark["job"], &mark["kind"]), (&json!(first), &json!("pull")), "{mark}");
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified, copy.check.as_str()), (Some("copying"), false, ""), "{copy:?}");
    assert!(!r.exists(&record).await, "the stale record went before rsync ran, and the source's isn't copied");
    let part: u64 = run(&r.c, &format!("stat -c %s {dest}/.{BIG}.*")).await.stdout.trim().parse().unwrap();
    assert!(part > 0 && part < BIG_BYTES, "stopped partway: {part} of {BIG_BYTES} bytes");
    say(&format!("staging_partial_copy_retries: SIGKILL at {part} of {BIG_BYTES} bytes of {BIG}"));

    r.kill_group(&st).await;
    thaw(&r, &frozen, server).await;
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified, copy.check.as_str()), (Some("stopped"), false, ""), "{copy:?}");
    assert!(!r.exists(&record).await, "no record on a copy killed partway");
    assert!(r.exists(&marker).await);
    let listed = r.list().await;
    assert_eq!(listed.iter().find(|j| j["id"] == first).unwrap()["state"], "failed", "{listed:?}");

    // Cancel partway: rsync gets SIGTERM with the job.
    let second = "share-sbpartial-b";
    let frozen = pull_frozen(&r, second, port, &dest, &files, server).await;
    r.cancel(second).await;
    assert_eq!(r.job(second).await["state"], "cancelled");
    thaw(&r, &frozen, server).await;
    let kept: u64 = run(&r.c, &format!("stat -c %s {dest}/{BIG}")).await.stdout.trim().parse().unwrap();
    assert!(kept > 0 && kept < BIG_BYTES, "--partial kept {kept} of {BIG_BYTES} bytes under the file's name");
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified), (Some("stopped"), false), "{copy:?}");
    assert!(r.exists(&marker).await && !r.exists(&record).await);
    say(&format!("staging_partial_copy_retries: Cancel left {kept} of {BIG_BYTES} bytes of {BIG} under its own name"));

    // A check that fails: rsync finishes, and --delete takes a stray file but not the marker.
    let third = "share-sbpartial-c";
    assert_eq!(run(&r.c, &format!("printf x > {dest}/stray.bin")).await.code, Some(0));
    let mut wrong = files.clone();
    let big = wrong.as_array_mut().unwrap().iter_mut().find(|f| f["path"] == BIG).unwrap();
    big["size"] = json!(BIG_BYTES + 1);
    r.start("pull", third, &pull_spec(port, &dest, &wrong)).await;
    let st = r.until_state(third, "failed", 120).await;
    assert!(st["message"].as_str().unwrap().starts_with(&format!("Copy incomplete: {BIG} is {BIG_BYTES} bytes")), "rsync ended well, and the check failed: {st}");
    assert!(!r.exists(&format!("{dest}/stray.bin")).await, "--delete ran");
    let mark: Value = serde_json::from_str(&run(&r.c, &format!("cat {marker}")).await.stdout).unwrap();
    assert_eq!(mark["job"], third, "--delete kept this pull's marker: {mark}");
    assert!(!r.exists(&record).await, "no record before the check passes");
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified), (Some("stopped"), false), "{copy:?}");

    // Then it finishes.
    let fourth = "share-sbpartial-d";
    r.start("pull", fourth, &pull_spec(port, &dest, &files)).await;
    r.until_state(fourth, "done", 120).await;
    assert!(!r.exists(&marker).await, "the marker went once the check passed");
    let own: Value = serde_json::from_str(&run(&r.c, &format!("cat {record}")).await.stdout).unwrap();
    assert_eq!((&own["checked"], &own["repo"], &own["revision"]), (&json!("sizes"), &json!(JOB_REPO), &json!(JOB_REV)), "the target's own record: {own}");
    assert_ne!(own["verifiedAt"], json!(1), "not the source's: {own}");
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified, copy.check.as_str()), (None, true, "sizes"), "{copy:?}");
    let left = run(&r.c, &format!("cd {dest} && ls -A")).await.stdout;
    assert_eq!(lines(&left), [".sparkbox.json", "config.json", "model-00001-of-00002.safetensors", BIG], "nothing else: no temporary file, no original/");
    let sums = run(&r.c, &format!("cd {source} && sha256sum config.json model-*.safetensors > /tmp/sbfreeze.sums && cd {dest} && sha256sum -c /tmp/sbfreeze.sums")).await;
    assert_eq!(sums.code, Some(0), "the bytes match:\n{}{}", sums.stdout, sums.stderr);

    r.cancel(serve_id).await;
    clear_jobs(&r.c).await;
    r.live.stop();
    reset(&r.c, s).await;
}

/// D10, what real rsync does with a Hugging Face cache folder. A `snapshots/<rev>` folder of
/// relative links into `../../blobs`, as `hf` leaves one, served by the shipped serve job (`use
/// chroot = no`) and pulled by the shipped pull job (`-aL`): the sender can't open a link that
/// leaves its module (`Invalid cross-device link`, rsync exit 23). The job fails, nothing arrives,
/// and the folder reads stopped. As a control, the same jobs copy a link that stays inside the
/// module, so links as such aren't the problem. P5.T2 is the real fix.
#[tokio::test]
#[ignore]
async fn staging_hf_snapshot_copy() {
    let (s, _lock) = staging_or_skip!();
    let r = job_rig(s, "staging-hf-copy").await;
    let hub = "/home/sparkuser/.cache/huggingface/hub/models--sbstage--hf";
    let snapshot = format!("{hub}/snapshots/{JOB_REV}");
    let setup = format!(
        "mkdir -p {hub}/blobs {hub}/refs {snapshot}/sub {JOBS_ROOT} && printf '{{\"architectures\": [\"Sbstage\"]}}' > {hub}/blobs/aaaa && head -c 3000000 /dev/urandom > {hub}/blobs/bbbb \
         && printf tok > {hub}/blobs/cccc && printf {JOB_REV} > {hub}/refs/main && ln -s ../../blobs/aaaa {snapshot}/config.json && ln -s ../../blobs/bbbb {snapshot}/model.safetensors \
         && ln -s ../../../blobs/cccc {snapshot}/sub/tokenizer.json"
    );
    assert_eq!(run(&r.c, &setup).await.code, Some(0));

    let (serve_id, port) = ("share-sbhf-serve", 8743);
    r.serve(serve_id, &snapshot, port).await;
    let conf = run(&r.c, &format!("cat ~/.sparkbox/rsyncd/{serve_id}/rsyncd.conf")).await.stdout;
    assert!(conf.starts_with("use chroot = no\n") && conf.contains(&format!("  path = {snapshot}\n")), "{conf}");
    // SparkBox reads the folder through its links, so it knows what to expect.
    let files = r.manifest(&snapshot).await;
    let listed: Vec<(&str, u64)> = files.as_array().unwrap().iter().map(|f| (f["path"].as_str().unwrap(), f["size"].as_u64().unwrap())).collect();
    assert_eq!(listed, [("config.json", 30), ("model.safetensors", 3000000), ("sub/tokenizer.json", 3)]);

    let dest = format!("{JOBS_ROOT}/hf-copy");
    let pull = "share-sbhf-pull";
    r.start("pull", pull, &pull_spec(port, &dest, &files)).await;
    let st = r.until_state(pull, "failed", 60).await;
    let message = st["message"].as_str().unwrap();
    assert!(message.starts_with("copy failed (rsync exit 23): "), "{message}");
    assert!(message.contains("Invalid cross-device link (18)"), "the sender refused the links: {message}");
    let log = run(&r.c, &format!("cat ~/.sparkbox/jobs/{pull}.log")).await.stdout;
    for name in ["config.json", "model.safetensors", "sub/tokenizer.json"] {
        assert!(log.contains(&format!("send_files failed to open \"{name}\" (in model): Invalid cross-device link (18)")), "{log}");
    }
    let arrived = run(&r.c, &format!("find {dest} ! -type d ! -name .sparkbox.partial")).await.stdout;
    assert_eq!(arrived.trim(), "", "nothing arrived");
    assert!(r.exists(&format!("{dest}/.sparkbox.partial")).await, "the marker stays");
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified), (Some("stopped"), false), "{copy:?}");

    // The control: a link inside the module, the same jobs. -L follows it.
    let plain = format!("{JOBS_ROOT}/with-link");
    make_model(&r.c, &plain, 4096).await;
    assert_eq!(run(&r.c, &format!("ln -s model-00001-of-00002.safetensors {plain}/model-alias.safetensors")).await.code, Some(0));
    let (control_serve, control_port) = ("share-sbhf-control", 8744);
    r.serve(control_serve, &plain, control_port).await;
    let control_files = r.manifest(&plain).await;
    let control_dest = format!("{JOBS_ROOT}/with-link-copy");
    let control = "share-sbhf-control-pull";
    r.start("pull", control, &pull_spec(control_port, &control_dest, &control_files)).await;
    r.until_state(control, "done", 60).await;
    let link = run(&r.c, &format!("test -L {control_dest}/model-alias.safetensors && echo link || stat -c %s {control_dest}/model-alias.safetensors")).await.stdout;
    assert_eq!(link.trim(), "1048576", "the link inside the module arrived as a whole file");

    let rsync = run(&r.c, "dpkg-query -W -f='${Version}' rsync").await.stdout;
    say(&format!("staging_hf_snapshot_copy: D10's answer on rsync {rsync}: a snapshots/<rev> copy with -aL and use chroot = no FAILS (exit 23, Invalid cross-device link): nothing arrives and the folder reads stopped. A link inside the module copies."));
    r.cancel(serve_id).await;
    r.cancel(control_serve).await;
    clear_jobs(&r.c).await;
    r.live.stop();
    reset(&r.c, s).await;
}

/// Watches who writes to a folder, from /proc: any rsync whose last word is `<dest>/`. It logs, each
/// once and in order, `gone` when the processes it's given have all ended, `new` when another rsync
/// appears, and `beside` if it ever sees both at once. It's ready before the new job starts.
const WRITERS: &str = r#"
import os, sys, time
dest, old = sys.argv[1] + "/", set(sys.argv[2:])
said = []
def say(word):
    if word not in said:
        said.append(word)
        with open("/tmp/sbfreeze.writers", "a") as f:
            f.write(word + "\n")
open("/tmp/sbfreeze.watching", "w").close()
deadline = time.time() + 90
while time.time() < deadline and "new" not in said:
    now = set()
    for pid in os.listdir("/proc"):
        if pid.isdigit():
            try:
                with open("/proc/%s/cmdline" % pid, "rb") as f:
                    argv = f.read().decode("utf-8", "replace").split("\0")[:-1]
            except OSError:
                continue
            if len(argv) > 2 and os.path.basename(argv[0]) == "rsync" and argv[-1] == dest:
                now.add(pid)
    if now & old and now - old:
        say("beside")
    if not now & old:
        say("gone")
    if now - old:
        say("new")
    time.sleep(0.002)
"#;

/// Starts `WRITERS` on `dest` with `old` as the processes that must end first.
async fn watch_writers(r: &JobRig, dest: &str, old: &[u64]) {
    assert_eq!(r.c.exec(&wire("rm -f /tmp/sbfreeze.writers /tmp/sbfreeze.watching; cat > /tmp/sbfreeze-writers.py"), Some(WRITERS.as_bytes()), LIMIT).await.unwrap().code, Some(0));
    let pids: Vec<String> = old.iter().map(u64::to_string).collect();
    run(&r.c, &format!("nohup python3 /tmp/sbfreeze-writers.py {dest} {} >/tmp/sbfreeze.log 2>&1 &", pids.join(" "))).await;
    for _ in 0..100 {
        if r.exists("/tmp/sbfreeze.watching").await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the watcher never started: {}", run(&r.c, "cat /tmp/sbfreeze.log").await.stdout);
}

/// The real rsync of a pull stopped by `pull_frozen`: its process and the one it forked, as a real
/// client is two (generator and receiver), each with the same command line.
async fn rsync_group(r: &JobRig, job_pid: u64, child: u64) -> Vec<u64> {
    let found: Vec<u64> = lines(&run(&r.c, &format!("pgrep -g {job_pid} -x rsync")).await.stdout).iter().map(|p| p.parse().unwrap()).collect();
    assert!(found.len() >= 2 && found.contains(&child), "a real rsync client is two processes: {found:?}");
    let first = argv_of(&r.c, found[0]).await;
    for pid in &found {
        assert_eq!(argv_of(&r.c, *pid).await, first, "one command line");
    }
    found
}

/// A job's status and marker as a dead job leaves them, written by hand: the test names its child.
async fn dead_job_record(r: &JobRig, job: &str, dest: &str, job_pid: u64, child: u64, start: u64) {
    write_job_record(r, job, dest, &json!({ "id": job, "kind": "pull", "state": "failed", "pid": job_pid, "childPid": child, "childStart": start, "dest": dest, "instance": crate::identity::instance_name() })).await;
}

async fn write_job_record(r: &JobRig, job: &str, dest: &str, status: &Value) {
    let marker = json!({ "job": job, "kind": "pull", "at": 1 });
    let write = format!(
        "mkdir -p ~/.sparkbox/jobs {dest} && printf '%s' {} > ~/.sparkbox/jobs/{job}.json && printf '%s' {} > {dest}/.sparkbox.partial",
        sh_quote(&status.to_string()),
        sh_quote(&marker.to_string())
    );
    assert_eq!(run(&r.c, &write).await.code, Some(0));
}

/// Round 5's orphan on real rsync, a real `kill -9` and real /proc. A pull stands still inside `BIG`,
/// and its job alone is killed, as the OOM killer would: both of real rsync's processes run on.
/// `childStart` and `pidStart` are /proc's field 22, read independently. `list` and `list --look` say
/// "orphan", say so in the job's message, and stop nothing. Delete refuses in a new job's words. A new
/// pull on the folder ends them first: a watcher reading /proc sees them gone before any new rsync,
/// and never both. It finishes from what's there, and the bytes match.
/// Then each way the record can't be trusted, on real processes: a pid used again by something else
/// is left alone, the job's own pid as well as its child's, and holds no folder; a real rsync on this
/// folder with another start time, and one with no marker, refuse and stay; another user's process
/// with this folder as its last word, recorded as the child or not, refuses and is never signalled.
#[tokio::test]
#[ignore]
async fn staging_orphan_rsync_after_sigkill() {
    let (s, _lock) = staging_or_skip!();
    let r = job_rig(s, "staging-orphan").await;
    let source = format!("{JOBS_ROOT}/source");
    let dest = format!("{JOBS_ROOT}/target");
    make_model(&r.c, &source, BIG_BYTES).await;
    let (serve_id, port) = ("share-sborphan-serve", 8745);
    let server = r.serve(serve_id, &source, port).await;
    let files = r.manifest(&source).await;

    // The pull stands still. Its job's start time for its child is /proc's own.
    let first = "share-sborphan-a";
    let frozen = pull_frozen(&r, first, port, &dest, &files, server).await;
    let st = r.job(first).await;
    let (job_pid, child) = (st["pid"].as_u64().unwrap(), st["childPid"].as_u64().unwrap());
    let field22: u64 = run(&r.c, &format!("cut -d' ' -f22 /proc/{child}/stat")).await.stdout.trim().parse().unwrap();
    assert_eq!(st["childStart"], json!(field22), "childStart is /proc/<pid>/stat's field 22: {st}");
    let field22: u64 = run(&r.c, &format!("cut -d' ' -f22 /proc/{job_pid}/stat")).await.stdout.trim().parse().unwrap();
    assert_eq!(st["pidStart"], json!(field22), "so is the job's own: {st}");
    let old = rsync_group(&r, job_pid, child).await;
    let rsync = run(&r.c, &format!("readlink /proc/{child}/exe")).await.stdout;
    assert_eq!(rsync.trim(), "/usr/bin/rsync", "the distribution's rsync");

    // SIGKILL to the job alone.
    assert_eq!(run(&r.c, &format!("kill -9 {job_pid}")).await.code, Some(0));
    assert!(!r.running(job_pid).await, "the job is gone");
    for pid in &old {
        assert!(r.running(*pid).await, "rsync {pid} outlives its job");
    }

    // list and list --look say so, and stop nothing.
    let says = "The job stopped unexpectedly (node restarted?). Its copy is still running: a new copy stops it first.";
    let looked = r.sparkjob("list --look", None).await;
    let job = looked.as_array().unwrap().iter().find(|j| j["id"] == first).unwrap().clone();
    assert_eq!((&job["state"], &job["orphan"], job["message"].as_str()), (&json!("failed"), &json!(true), Some(says)), "{job}");
    assert_eq!(r.job(first).await["state"], "running", "--look wrote nothing");
    let listed = r.list().await;
    let job = listed.iter().find(|j| j["id"] == first).unwrap();
    assert_eq!((&job["state"], &job["orphan"], job["message"].as_str()), (&json!("failed"), &json!(true), Some(says)), "{job}");
    assert!(r.job(first).await.get("orphan").is_none(), "a fact of the moment isn't kept");
    assert_eq!(r.job(first).await["message"], "The job stopped unexpectedly (node restarted?).", "a fact of the moment isn't kept");
    for pid in &old {
        assert!(r.running(*pid).await, "list stopped nothing");
    }

    // The folder reads as stopped, and delete still won't take it from under them.
    assert_eq!(r.copy(&dest).await.partial.as_deref(), Some("stopped"));
    let refused = r.live.sparkjob(&r.node, &format!("delete {JOBS_ROOT} {dest}"), None, LIMIT).await.unwrap_err();
    assert!(refused.contains(&format!("An earlier copy may still be writing to {dest}.")), "{refused}");
    assert!(r.exists(&format!("{dest}/.sparkbox.partial")).await, "the folder stays");
    for pid in &old {
        assert!(r.running(*pid).await, "delete stopped nothing");
    }

    // A new pull ends them first.
    watch_writers(&r, &dest, &old).await;
    let second = "share-sborphan-b";
    let began = Instant::now();
    r.start("pull", second, &pull_spec(port, &dest, &files)).await;
    let done = r.until_state(second, "done", 120).await;
    let took = began.elapsed();
    let order = lines(&run(&r.c, "cat /tmp/sbfreeze.writers").await.stdout);
    assert_eq!(order, ["gone", "new"], "the old copy was gone before the new one started, and they never ran together: {done}");
    for pid in &old {
        assert!(!r.running(*pid).await, "rsync {pid} is gone");
    }
    let reasons = r.job(first).await;
    assert_eq!(reasons["state"], "failed", "{reasons}");
    say(&format!("staging_orphan_rsync_after_sigkill: {} rsync processes outlived kill -9 of the job; a new pull ended them first and finished in {took:?}", old.len()));
    thaw(&r, &frozen, server).await;
    let sums = run(&r.c, &format!("cd {source} && sha256sum config.json model-*.safetensors > /tmp/sbfreeze.sums && cd {dest} && sha256sum -c /tmp/sbfreeze.sums")).await;
    assert_eq!(sums.code, Some(0), "the bytes match:\n{}{}", sums.stdout, sums.stderr);
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified, copy.check.as_str()), (None, true, "sizes"), "{copy:?}");

    // A pid used again: a real process, a real start time that isn't the record's.
    let other: u64 = run(&r.c, "nohup sleep 300 >/dev/null 2>&1 & echo $!").await.stdout.trim().parse().unwrap();
    let recycled = format!("{JOBS_ROOT}/recycled");
    dead_job_record(&r, "share-sbrecycled", &recycled, 999_999, other, 7).await;
    assert!(r.list().await.iter().find(|j| j["id"] == "share-sbrecycled").unwrap().get("orphan").is_none(), "a pid used again isn't an orphan");
    let third = "share-sborphan-c";
    r.start("pull", third, &pull_spec(port, &recycled, &files)).await;
    r.until_state(third, "done", 120).await;
    assert!(r.running(other).await, "the process with that pid now was left alone");
    run(&r.c, &format!("kill {other}")).await;

    // The job's own pid used again, by a real group leader: a job that died at running, as a power
    // cut leaves it. It holds no folder, and Cancel doesn't signal it.
    let other: u64 = run(&r.c, "setsid nohup sleep 300 >/dev/null 2>&1 & echo $!").await.stdout.trim().parse().unwrap();
    let reused = format!("{JOBS_ROOT}/reused");
    let dead = "share-sbreused";
    write_job_record(&r, dead, &reused, &json!({ "id": dead, "kind": "pull", "state": "running", "pid": other, "pidStart": 7, "dest": reused, "instance": crate::identity::instance_name() })).await;
    let next = "share-sborphan-i";
    r.start("pull", next, &pull_spec(port, &reused, &files)).await;
    r.until_state(next, "done", 120).await;
    assert_eq!(r.sparkjob(&format!("cancel {dead}"), None).await["state"], "cancelled");
    assert!(r.running(other).await, "Cancel left the process with the job's pid alone");
    assert_eq!(run(&r.c, &format!("kill {other}")).await.code, Some(0));

    // A real rsync on this folder whose start time isn't the record's could be anyone's: it stays,
    // and the new pull refuses. So does one no record names, with the marker gone.
    let lost = format!("{JOBS_ROOT}/lost");
    let fourth = "share-sborphan-d";
    let frozen = pull_frozen(&r, fourth, port, &lost, &files, server).await;
    let st = r.job(fourth).await;
    let (job_pid, child) = (st["pid"].as_u64().unwrap(), st["childPid"].as_u64().unwrap());
    let start = st["childStart"].as_u64().unwrap();
    let old = rsync_group(&r, job_pid, child).await;
    assert_eq!(run(&r.c, &format!("kill -9 {job_pid}")).await.code, Some(0));
    assert!(!r.running(job_pid).await);
    dead_job_record(&r, fourth, &lost, job_pid, child, start + 1).await;
    for (job, unmarked) in [("share-sborphan-e", false), ("share-sborphan-h", true)] {
        if unmarked {
            assert_eq!(run(&r.c, &format!("rm {lost}/.sparkbox.partial")).await.code, Some(0));
        }
        r.start("pull", job, &pull_spec(port, &lost, &files)).await;
        let st = r.until_state(job, "failed", 30).await;
        assert_eq!(st["message"], format!("An earlier copy may still be writing to {lost}."), "{st}");
        for pid in &old {
            assert!(r.running(*pid).await, "nothing unvouched-for is signalled");
        }
    }
    assert_eq!(run(&r.c, &format!("kill -9 {}", old.iter().map(u64::to_string).collect::<Vec<_>>().join(" "))).await.code, Some(0));
    thaw(&r, &frozen, server).await;

    // Another user's process, with this folder as its last word. Started by root as uid 65534.
    let theirs = format!("{JOBS_ROOT}/theirs");
    assert_eq!(run(&r.c, &format!("mkdir -p {theirs}")).await.code, Some(0));
    let start_as_nobody = format!("setsid nohup setpriv --reuid=65534 --regid=65534 --clear-groups /bin/bash -c 'exec -a rsync /usr/bin/python3 -c \"import time; time.sleep(120)\" {theirs}/' >/dev/null 2>&1 < /dev/null & echo $!");
    let held: u64 = sudo_ok(&r.c, s, &start_as_nobody).await.stdout.trim().parse().unwrap();
    let mut facts = String::new();
    for _ in 0..100 {
        facts = run(&r.c, &format!("stat -c %u /proc/{held}; tr '\\0' ' ' < /proc/{held}/cmdline")).await.stdout;
        if facts.contains("rsync -c") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(facts.starts_with("65534\nrsync -c") && facts.trim_end().ends_with(&format!("{theirs}/")), "{facts:?}");
    let field22: u64 = run(&r.c, &format!("cut -d' ' -f22 /proc/{held}/stat")).await.stdout.trim().parse().unwrap();
    for (record, job) in [(false, "share-sborphan-f"), (true, "share-sborphan-g")] {
        if record {
            dead_job_record(&r, "share-sbtheirs", &theirs, 999_999, held, field22).await;
            assert_eq!(r.list().await.iter().find(|j| j["id"] == "share-sbtheirs").unwrap()["orphan"], true, "the record's child, confirmed");
        }
        r.start("pull", job, &pull_spec(port, &theirs, &files)).await;
        let st = r.until_state(job, "failed", 30).await;
        assert_eq!(st["message"], format!("An earlier copy may still be writing to {theirs}."), "{st}");
        assert!(r.running(held).await, "another user's process is never touched");
    }
    assert!(!r.exists(&format!("{theirs}/config.json")).await, "nothing was copied beside it");
    sudo_ok(&r.c, s, &format!("kill -9 {held}")).await;

    r.cancel(serve_id).await;
    clear_jobs(&r.c).await;
    r.live.stop();
    reset(&r.c, s).await;
}

/// A file of the docker stand-in's Hub, with what Hugging Face says of it.
async fn hub_file(c: &Session, name: &str, bytes: &[u8]) -> Value {
    let put = wire(&format!("mkdir -p {DOCKER_STATE}/hub && cat > {DOCKER_STATE}/hub/{name}"));
    assert_eq!(c.exec(&put, Some(bytes), LIMIT).await.unwrap().code, Some(0));
    json!({ "rfilename": name, "size": bytes.len(), "lfs": { "size": bytes.len(), "sha256": sha256_hex(bytes) } })
}

/// Starts a download through `download_spec` and returns once hf stands halfway through `BIG`
/// (`hf-hold`) with its unfinished file on disk, as a real one has it.
async fn download_held(r: &JobRig, model: &hf::HfModel, id: &str, dest: &str, token: &str) {
    assert_eq!(run(&r.c, &format!("printf %s {BIG} > {DOCKER_STATE}/hf-hold")).await.code, Some(0));
    r.start("download", id, &download_spec(model, dest, IMAGE_REF, token.to_string())).await;
    for _ in 0..200 {
        if !run(&r.c, &format!("ls {dest}/.cache/huggingface/download/ 2>/dev/null | grep '.incomplete$'")).await.stdout.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("hf never got to {BIG}: {} {}", r.job(id).await, run(&r.c, &format!("cat ~/.sparkbox/jobs/{id}.log")).await.stdout);
}

/// Lets a held hf go on.
async fn release_hold(c: &Session) {
    assert_eq!(run(c, &format!("rm -f {DOCKER_STATE}/hf-hold")).await.code, Some(0));
}

/// The running stand-in container of a download: its id.
async fn only_container(c: &Session) -> String {
    let ids = lines(&run(c, "docker ps -q").await.stdout);
    assert_eq!(ids.len(), 1, "one container: {ids:?}");
    ids[0].clone()
}

/// What the job sent to stop its download: the `ps` that finds it, by both labels, and the `stop`.
fn assert_stopped_by_labels(calls: &[Value], job: &str, container: &str) {
    let instance = crate::identity::instance_name();
    let ps = ["ps", "-q", "--filter", &format!("label=sparkbox.deployment={job}"), "--filter", &format!("label=sparkbox.instance={instance}")].map(String::from);
    assert!(calls.iter().any(|call| argv(call) == ps), "SparkBox found it by both labels: {ps:?}");
    let stop = ["stop", "-t", "5", container].map(String::from);
    assert!(calls.iter().any(|call| argv(call) == stop), "SparkBox stopped it: {stop:?}");
}

/// Waits for the retry's container, and shows it's the only one: the old one stopped first. Returns
/// its id. hf stands at its hold again, so it can be looked at.
async fn second_writer(r: &JobRig, job: &str, old: &str) -> String {
    let mine = format!("docker ps -q --filter label=sparkbox.deployment={job}");
    for _ in 0..300 {
        let ids = lines(&run(&r.c, &mine).await.stdout);
        if let [id] = &ids[..] {
            assert_ne!(id, old, "a new container");
            assert_eq!(only_container(&r.c).await, *id, "one writer");
            return id.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the retry {job} never started its container: {} | {}", r.job(job).await, run(&r.c, "docker ps").await.stdout);
}

/// D2, C-S and Cancel under real Linux modes, through `download_spec` and the shipped job, with
/// `docker` the stand-in. While a download runs: the folder is 0700 and the file 0400, the cluster
/// user's, another user can't read it, and the process in the container can. The token is in no
/// argv, environment, process, `docker inspect`, job status or job log. The folder is gone after a
/// normal end, after Cancel, and after SIGKILL followed by `list`. Cancel ends the container, whose
/// PID 1 ignores SIGTERM and which outlives its CLI, by both labels, and `list` stops nothing. A
/// retry on the folder stops a killed job's container first, found through the marker, whether or
/// not `list` ran in between, so there's one writer. With no token there's neither the `-e` nor the
/// mount. Nothing here runs `docker rm` or `docker stop` by hand: the calls are SparkBox's.
#[tokio::test]
#[ignore]
async fn staging_download_token_files() {
    let (s, _lock) = staging_or_skip!();
    let r = job_rig(s, "staging-token").await;
    let images = format!("printf '{{\"{IMAGE_REF}\": \"{}\"}}' > {DOCKER_STATE}/images.json", image_id());
    assert_eq!(run(&r.c, &images).await.code, Some(0));
    let one: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let two: Vec<u8> = (0..1024 * 1024).map(|i| (i % 241) as u8).collect();
    let siblings = vec![
        hub_file(&r.c, "config.json", b"{\"architectures\": [\"Sbstage\"]}").await,
        hub_file(&r.c, "model-00001-of-00002.safetensors", &one).await,
        hub_file(&r.c, BIG, &two).await,
    ];
    let model = hf::parse_model(&json!({ "id": JOB_REPO, "sha": JOB_REV, "siblings": siblings }), Value::Null, None, "");
    assert_eq!(model.files.len(), 3, "{:?}", model.files);
    let token = format!("hf_sbfake{}", &sha256_hex(b"staging_download_token_files")[..32]);
    let home = "/home/sparkuser";
    let secrets_of = |id: &str| format!("{home}/.sparkbox/job-secrets/{id}");

    // A download that runs to its end.
    let (id, dest) = ("dl-sbend", format!("{JOBS_ROOT}/end"));
    download_held(&r, &model, id, &dest, &token).await;
    let folder = secrets_of(id);
    let modes = run(&r.c, &format!("stat -c '%a %U %n' {home}/.sparkbox/job-secrets {folder} {folder}/hf_token")).await.stdout;
    assert_eq!(lines(&modes), [format!("700 sparkuser {home}/.sparkbox/job-secrets"), format!("700 sparkuser {folder}"), format!("400 sparkuser {folder}/hf_token")]);
    assert_eq!(run(&r.c, &format!("cat {folder}/hf_token")).await.stdout, token, "the file holds exactly the token");
    let other = sudo(&r.c, s, &format!("setpriv --reuid=65534 --regid=65534 --clear-groups cat {folder}/hf_token"), None).await;
    assert_ne!(other.code, Some(0), "another user read it: {}", other.stdout);
    assert!(other.stderr.contains("Permission denied"), "{}", other.stderr);
    let read = run(&r.c, &format!("tail -1 {DOCKER_STATE}/hf.jsonl")).await.stdout;
    let read: Value = serde_json::from_str(&read).unwrap();
    assert_eq!((&read["source"], read["sha256"].as_str()), (&json!("file"), Some(sha256_hex(token.as_bytes()).as_str())), "hf read it from the mounted file");

    let calls = docker_calls(&r.c).await;
    let run_call = calls.iter().rev().find(|call| argv(call).first().map(String::as_str) == Some("run")).expect("docker run was called");
    let instance = crate::identity::instance_name();
    let mut want: Vec<String> = [
        "run", "--rm", "--name", &format!("sparkbox-dl-{id}"), "--label", &format!("sparkbox.instance={instance}"), "--label", &format!("sparkbox.deployment={id}"),
        "--user", "1000:1000", "-e", &format!("HF_TOKEN_PATH={TOKEN_MOUNT}"), "-e", "HF_HOME=/tmp/hf", "-e", "HF_HUB_DISABLE_PROGRESS_BARS=1",
    ]
    .iter()
    .map(|w| w.to_string())
    .collect();
    want.extend(["-v".to_string(), format!("{folder}/hf_token:{TOKEN_MOUNT}:ro"), "-v".into(), format!("{dest}:/dest")]);
    want.extend(["--entrypoint", "hf", IMAGE_REF, "download", JOB_REPO, "--revision", JOB_REV, "--local-dir", "/dest", "--include", "config.json", "--include", "model-00001-of-00002.safetensors", "--include", BIG].map(String::from));
    assert_eq!(argv(run_call), want, "C-T's command under real dash");
    assert!(!run_call["env"].as_object().unwrap().contains_key("HF_TOKEN"), "the job's environment has no HF_TOKEN");
    assert!(run_call["procs"].as_array().unwrap().len() > 3, "the process list was read");
    let seen = serde_json::to_string(&calls).unwrap();
    assert!(!seen.contains(&token), "the token isn't in docker's argv or environment, nor in any process's command line or readable environment");
    let inspect = run(&r.c, &format!("docker inspect {}", only_container(&r.c).await)).await.stdout;
    assert!(!inspect.contains(&token), "docker inspect doesn't show the token");
    let shown: Value = serde_json::from_str(&inspect).unwrap();
    assert!(shown[0]["Config"]["Env"].as_array().unwrap().iter().any(|e| *e == format!("HF_TOKEN_PATH={TOKEN_MOUNT}")), "{shown}");
    let job_files = run(&r.c, &format!("cat ~/.sparkbox/jobs/{id}.json ~/.sparkbox/jobs/{id}.log")).await.stdout;
    assert!(!job_files.is_empty() && !job_files.contains(&token), "the job's status and log don't hold it");
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified), (Some("downloading"), false), "{copy:?}");

    release_hold(&r.c).await;
    r.until_state(id, "done", 60).await;
    assert!(!r.exists(&folder).await, "gone after a normal end");
    let record: Value = serde_json::from_str(&run(&r.c, &format!("cat {dest}/.sparkbox.json")).await.stdout).unwrap();
    assert_eq!((&record["checked"], &record["revision"]), (&json!("sha256"), &json!(JOB_REV)), "{record}");
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified, copy.check.as_str()), (None, true, "sha256"), "{copy:?}");
    assert_eq!(run(&r.c, "docker ps -aq").await.stdout.trim(), "", "--rm: no container is left");

    // Cancel, while hf stands halfway.
    let (id, dest) = ("dl-sbcancel", format!("{JOBS_ROOT}/cancel"));
    download_held(&r, &model, id, &dest, &token).await;
    let folder = secrets_of(id);
    assert!(r.exists(&format!("{folder}/hf_token")).await);
    let container = only_container(&r.c).await;
    let began = Instant::now();
    r.cancel(id).await;
    assert_eq!(r.job(id).await["state"], "cancelled");
    assert!(!r.exists(&folder).await, "gone after Cancel");
    assert_eq!(run(&r.c, "docker ps -aq").await.stdout.trim(), "", "Cancel stops the download itself, not only its docker CLI");
    assert_stopped_by_labels(&docker_calls(&r.c).await, id, &container);
    assert!(began.elapsed() >= Duration::from_secs(5), "hf ignored SIGTERM and went at docker stop's SIGKILL: {:?}", began.elapsed());
    assert!(began.elapsed() < Duration::from_secs(20), "inside Cancel's time limit: {:?}", began.elapsed());
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified), (Some("stopped"), false), "{copy:?}");
    release_hold(&r.c).await;

    // SIGKILL to the job's group. Nothing runs in the job; `list` finds it dead. The container is
    // the daemon's, so it outlives its CLI, as on real Docker (scripts/staging/token-proof.sh), and
    // `list`, a read, leaves it running and says so.
    let (id, dest) = ("dl-sbkill", format!("{JOBS_ROOT}/kill"));
    download_held(&r, &model, id, &dest, &token).await;
    let folder = secrets_of(id);
    r.kill_group(&r.job(id).await).await;
    assert!(r.exists(&format!("{folder}/hf_token")).await, "no code ran after SIGKILL, so the file is still there");
    let survivor = only_container(&r.c).await;
    let calls_before = docker_calls(&r.c).await.len();
    let listed = r.list().await;
    let dead = listed.iter().find(|j| j["id"] == id).unwrap();
    assert_eq!(dead["state"], "failed", "{listed:?}");
    assert_eq!(dead["message"], "The job stopped unexpectedly (node restarted?). Its container may still be running: Resume download stops it first.");
    assert!(!r.exists(&folder).await, "the next list removed a dead job's token");
    assert_eq!(docker_calls(&r.c).await.len(), calls_before, "list doesn't call docker at all");
    assert_eq!(only_container(&r.c).await, survivor, "list stops nothing");
    say("staging_download_token_files: after SIGKILL to the job's group the container is still running, as on real Docker; the token file went at the next list, which stopped nothing.");

    // A retry, as Resume sends it, after that `list`: a new id, on the same folder.
    let retried = "dl-sbretry";
    r.start("download", retried, &download_spec(&model, &dest, IMAGE_REF, token.clone())).await;
    let second = second_writer(&r, retried, &survivor).await;
    assert_stopped_by_labels(&docker_calls(&r.c).await, id, &survivor);
    release_hold(&r.c).await;
    r.until_state(retried, "done", 60).await;
    assert_eq!(run(&r.c, "docker ps -aq").await.stdout.trim(), "", "{second} ended, with --rm");
    let copy = r.copy(&dest).await;
    assert_eq!((copy.partial.as_deref(), copy.verified, copy.check.as_str()), (None, true, "sha256"), "{copy:?}");

    // The same with no `list` in between: only the marker names the old job.
    let (id, dest) = ("dl-sbkill2", format!("{JOBS_ROOT}/kill2"));
    download_held(&r, &model, id, &dest, &token).await;
    r.kill_group(&r.job(id).await).await;
    let survivor = only_container(&r.c).await;
    let retried = "dl-sbretry2";
    r.start("download", retried, &download_spec(&model, &dest, IMAGE_REF, token.clone())).await;
    second_writer(&r, retried, &survivor).await;
    assert_stopped_by_labels(&docker_calls(&r.c).await, id, &survivor);
    let marker: Value = serde_json::from_str(&run(&r.c, &format!("cat {dest}/.sparkbox.partial")).await.stdout).unwrap();
    assert_eq!(marker["job"], retried, "{marker}");
    release_hold(&r.c).await;
    r.until_state(retried, "done", 60).await;
    assert!(!r.exists(&secrets_of(retried)).await, "gone after a normal end");
    r.list().await;
    assert!(!r.exists(&secrets_of(id)).await, "the dead job's folder goes at the next list");

    // No token: neither the -e nor the mount, and no secrets folder.
    let (id, dest) = ("dl-sbnone", format!("{JOBS_ROOT}/none"));
    download_held(&r, &model, id, &dest, "").await;
    let call = docker_calls(&r.c).await.into_iter().rev().find(|call| argv(call).first().map(String::as_str) == Some("run")).expect("docker run was called");
    let words = argv(&call);
    assert!(!words.iter().any(|w| w.contains("HF_TOKEN") || w.contains("hf_token") || w.contains("job-secrets")), "{words:?}");
    assert!(!r.exists(&secrets_of(id)).await, "no token, no folder");
    let read: Value = serde_json::from_str(&run(&r.c, &format!("tail -1 {DOCKER_STATE}/hf.jsonl")).await.stdout).unwrap();
    assert_eq!(read["source"], "none");
    release_hold(&r.c).await;
    r.until_state(id, "done", 60).await;

    clear_jobs(&r.c).await;
    r.live.stop();
    reset(&r.c, s).await;
}
