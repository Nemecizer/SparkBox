//! The four machines as SparkBox found them in the Live tour on Oct 29: what every Demo world
//! runs on the hosts (the real daemons, users, ports and services), and what only the
//! as-found world has on top — nothing serving, the owner's failed 4-node GLM-5.3-Flash run
//! left as four exited glm53 containers, a stopped Traefik on cell3, gx10-0000's half-year-old
//! test containers, native Ollama on cell1 and cell3 bound to loopback, the pending updates,
//! the recabling in the kernel log and the images on disk.
//!
//! Every fact comes from that morning's doctor report, Live snapshots and window dumps. Nothing
//! secret was copied: the glm53 API key is a placeholder, as are the files that hold it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::ops::{DemoPackage, Unit};
use super::world::*;
use super::{DemoContainer, DemoNode, DemoProc, Kind, CORES};
use crate::model::{Container, KernelEvent};

/// `systemctl list-units` on each node that morning: 92 units, as Services showed them.
const SERVICES: &str = include_str!("fixtures/services-matrix.txt");
/// The 58 mlx5 events from the recabling on Oct 26 (2:16–2:29 PM), as the collector sent them.
const KERNEL_MLX5: &str = include_str!("fixtures/kernel-mlx5.jsonl");
/// The head's `docker logs glm53` (serving's fixture, unwrapped and redacted).
pub(super) const GLM53_LOG: &str = include_str!("../models/fixtures/logs/glm53-rank0.log");

pub const GLM53_IMAGE: &str = "vllm/vllm-openai:glm53-flash-arm64-cu130";
pub const GX10_VLLM_IMAGE: &str = "nvcr.io/nvidia/vllm:26.03.post1-py3";
pub const CUDA_IMAGE: &str = "nvcr.io/nvidia/cuda:13.0.1-devel-ubuntu24.04";
pub const TRAEFIK_IMAGE: &str = "docker.io/traefik:v3.6.1";
/// The value Inspect shows for glm53's API_KEY: a placeholder, never the owner's key.
pub const API_KEY_PLACEHOLDER: &str = "demo-placeholder-not-the-real-key";

// ---- Processes every world runs on the host

/// (comm, user, command, unit, rss MiB, threads, %CPU at idle, listening (port, address)).
type Daemon = (&'static str, &'static str, &'static str, Option<&'static str>, f64, u32, f64, &'static [(u16, &'static str)]);

/// The long-lived processes of a DGX Spark at rest, in pid order (doctor.txt and the tour's
/// Workloads list). Names are argv[0]'s, so "dashboard-service" is whole (its comm is cut to 15).
const DAEMONS: &[Daemon] = &[
    ("systemd-journald", "root", "/usr/lib/systemd/systemd-journald", Some("systemd-journald.service"), 26.4, 1, 0.0, &[]),
    ("multipathd", "root", "/sbin/multipathd -d -s", Some("multipathd.service"), 27.1, 7, 0.0, &[]),
    ("systemd-udevd", "root", "/usr/lib/systemd/systemd-udevd", Some("systemd-udevd.service"), 9.8, 1, 0.0, &[]),
    ("systemd-resolved", "systemd-resolve", "/usr/lib/systemd/systemd-resolved", Some("systemd-resolved.service"), 13.2, 1, 0.0, &[(53, "127.0.0.53"), (53, "127.0.0.54")]),
    ("systemd-timesyncd", "systemd-timesync", "/usr/lib/systemd/systemd-timesyncd", Some("systemd-timesyncd.service"), 7.1, 2, 0.0, &[]),
    ("accounts-daemon", "root", "/usr/libexec/accounts-daemon", Some("accounts-daemon.service"), 8.9, 3, 0.0, &[]),
    ("avahi-daemon", "avahi", "avahi-daemon: running [{host}.local]", Some("avahi-daemon.service"), 5.1, 1, 0.4, &[]),
    ("dbus-daemon", "messagebus", "@dbus-daemon --system --address=systemd: --nofork --nopidfile --systemd-activation --syslog-only", Some("dbus.service"), 6.0, 1, 0.2, &[]),
    ("dashboard-service", "dgx-dashboard-service-user", "/opt/nvidia/dgx-dashboard-service/dashboard-service -port 11000 serve", Some("dgx-dashboard.service"), 16.2, 22, 0.4, &[(11000, "127.0.0.1")]),
    ("cron", "root", "/usr/sbin/cron -f -P", Some("cron.service"), 2.8, 1, 0.0, &[]),
    ("earlyoom", "root", "/usr/bin/earlyoom -r 3600", Some("earlyoom.service"), 3.4, 1, 0.0, &[]),
    ("polkitd", "polkitd", "/usr/lib/polkit-1/polkitd --no-debug", Some("polkit.service"), 9.2, 4, 0.0, &[]),
    ("lldpd", "_lldpd", "lldpd: monitor.", Some("lldpd.service"), 2.6, 1, 0.0, &[]),
    ("rasdaemon", "root", "/usr/sbin/rasdaemon -f -r", Some("rasdaemon.service"), 5.7, 1, 0.0, &[]),
    ("smartd", "root", "/usr/sbin/smartd -n", Some("smartmontools.service"), 5.3, 1, 0.0, &[]),
    ("switcheroo-control", "root", "/usr/libexec/switcheroo-control", Some("switcheroo-control.service"), 6.4, 3, 0.0, &[]),
    ("systemd-logind", "root", "/usr/lib/systemd/systemd-logind", Some("systemd-logind.service"), 8.0, 1, 0.0, &[]),
    ("udisksd", "root", "/usr/libexec/udisks2/udisksd", Some("udisks2.service"), 13.6, 6, 0.0, &[]),
    ("networkd-dispatcher", "root", "/usr/bin/python3 /usr/bin/networkd-dispatcher --run-startup-triggers", Some("networkd-dispatcher.service"), 21.3, 1, 0.0, &[]),
    ("rsyslogd", "syslog", "/usr/sbin/rsyslogd -n -iNONE", Some("rsyslog.service"), 6.2, 4, 0.0, &[]),
    ("ModemManager", "root", "/usr/sbin/ModemManager", Some("ModemManager.service"), 11.7, 4, 0.0, &[]),
    ("wpa_supplicant", "root", "/usr/sbin/wpa_supplicant -u -s -O DIR=/run/wpa_supplicant GROUP=netdev", Some("wpa_supplicant.service"), 7.4, 1, 0.0, &[]),
    ("bluetoothd", "root", "/usr/libexec/bluetooth/bluetoothd", Some("bluetooth.service"), 6.1, 1, 0.0, &[]),
    ("nvidia-persistenced", "nvidia-persistenced", "/usr/bin/nvidia-persistenced --user nvidia-persistenced --verbose", Some("nvidia-persistenced.service"), 3.9, 1, 0.0, &[]),
    ("nvidia-dgx-telemetry", "root", "/usr/bin/nvidia-dgx-telemetry --config /etc/nvidia-dgx-telemetry/config.yaml", Some("nvidia-dgx-telemetry.service"), 19.6, 6, 0.1, &[]),
    ("cupsd", "root", "/usr/sbin/cupsd -l", Some("cups.service"), 7.8, 1, 0.0, &[(631, "127.0.0.1"), (631, "::1")]),
    ("cups-browsed", "cups-browsed", "/usr/sbin/cups-browsed", Some("cups-browsed.service"), 10.2, 3, 0.0, &[]),
    ("snapd", "root", "/usr/lib/snapd/snapd", Some("snapd.service"), 31.8, 18, 0.0, &[]),
    ("containerd", "root", "/usr/bin/containerd", Some("containerd.service"), 26.3, 22, 0.2, &[]),
    ("sshd", "root", "sshd: /usr/sbin/sshd -D [listener] 0 of 10-100 startups", Some("ssh.service"), 8.6, 1, 0.0, &[(22, "0.0.0.0"), (22, "::")]),
    ("xrdp-sesman", "root", "/usr/sbin/xrdp-sesman", Some("xrdp-sesman.service"), 4.9, 1, 0.0, &[(3350, "::1")]),
    ("xrdp", "xrdp", "/usr/sbin/xrdp", Some("xrdp.service"), 3.3, 1, 0.0, &[(3389, "::")]),
    ("dashboard-admin", "root", "/opt/nvidia/dgx-dashboard/dashboard-admin", Some("dgx-dashboard-admin.service"), 14.6, 17, 0.2, &[]),
    ("master", "root", "/usr/lib/postfix/sbin/master -w", Some("postfix@-.service"), 4.6, 1, 0.0, &[(25, "0.0.0.0"), (25, "::")]),
    ("qmgr", "postfix", "qmgr -l -t unix -u", Some("postfix@-.service"), 6.9, 1, 0.0, &[]),
    ("dockerd", "root", "/usr/bin/dockerd -H fd:// --containerd=/run/containerd/containerd.sock", Some("docker.service"), 55.3, 19, 0.2, &[]),
    ("ollama", "ollama", "/usr/local/bin/ollama serve", Some("ollama.service"), 20.1, 12, 0.0, &[(11434, "127.0.0.1")]),
    ("nginx", "root", "nginx: master process /usr/sbin/nginx -g daemon on; master_process on;", Some("nginx.service"), 1.9, 1, 0.0, &[(8080, "0.0.0.0")]),
    ("gdm3", "root", "/usr/sbin/gdm3", Some("gdm.service"), 9.0, 3, 0.0, &[]),
    ("gnome-remote-desktop-daemon", "gnome-remote-desktop", "/usr/libexec/gnome-remote-desktop-daemon --system", Some("gnome-remote-desktop.service"), 21.4, 9, 0.0, &[]),
    ("gjs", "gdm", "/usr/bin/gjs -m /usr/share/gnome-shell/org.gnome.Shell.Notifications", None, 25.8, 9, 0.0, &[]),
    ("colord", "colord", "/usr/libexec/colord", Some("colord.service"), 12.3, 3, 0.0, &[]),
    ("upowerd", "root", "/usr/libexec/upowerd", Some("upower.service"), 8.1, 3, 0.0, &[]),
    ("rtkit-daemon", "rtkit", "/usr/libexec/rtkit-daemon", Some("rtkit-daemon.service"), 3.0, 3, 0.0, &[]),
    ("systemd", "{user}", "/usr/lib/systemd/systemd --user", Some("user@1000.service"), 10.4, 1, 0.0, &[]),
];

/// The units a world doesn't have on a node: earlyoom isn't installed on cell3 or gx10-0000,
/// nginx is cell1's, multipathd runs only on gx10-0000; native Ollama is the as-found world's.
fn runs(node: &str, comm: &str, world: World) -> bool {
    match comm {
        "earlyoom" => matches!(node, "cell1" | "cell2"),
        "multipathd" => node == "gx10-0000",
        "nginx" => world == World::AsFound && node == "cell1",
        "ollama" => world == World::AsFound && matches!(node, "cell1" | "cell3"),
        _ => true,
    }
}

/// A few kernel threads with CPU time to show (Workloads hides them unless asked).
const KTHREADS: &[(u32, &str, f64)] = &[
    (2, "kthreadd", 0.0),
    (15, "rcu_preempt", 0.2),
    (16, "rcu_exp_par_gp_kthread_worker/0", 0.0),
    (24, "migration/0", 0.0),
    (25, "ksoftirqd/0", 0.0),
    (98, "migration/9", 0.0),
];

/// The host's processes for a world, with their real users, units and ports. Pids follow the
/// node's boot order; `collector` is SparkBox's own (and its nvidia-smi child).
pub(super) fn host_procs(node: &mut DemoNode, world: World, rng: &mut fastrand::Rng) -> Vec<DemoProc> {
    let id = node.cfg.id.clone();
    let user = node.cfg.user.clone();
    let host = node.cfg.host.trim_end_matches(".local").to_string();
    let base = 1_560 + 21 * node.idx as u32;
    let mut out = Vec::new();
    let p = |pid: u32, name: &str, user: &str, cmd: &str, unit: Option<&str>, rss_mib: f64, threads: u32, cpu: f64, ports: &[(u16, &'static str)], started_s: u64| DemoProc {
        pid,
        name: name.into(),
        user: user.into(),
        cmd: cmd.into(),
        base_cpu: cpu,
        rss: rss_mib * 1_048_576.0,
        nice: 0,
        unit: unit.map(Into::into),
        ports: ports.to_vec(),
        ppid: 1,
        threads,
        started_s,
        stopped: false,
        cpu_time: 0.0,
        kernel: false,
        sparkbox: false,
    };
    out.push(p(1, "systemd", "root", "/sbin/init splash", Some("init.scope"), 12.3, 1, 0.0, &[], 0));
    for (k, (pid, name, cpu)) in KTHREADS.iter().enumerate() {
        out.push(DemoProc { kernel: true, ppid: if *pid == 2 { 0 } else { 2 }, cmd: format!("[{name}]"), ..p(*pid, name, "root", "", None, 0.0, 1, *cpu, &[], k as u64 / 3) });
    }
    let mut pid = base;
    for (comm, u, cmd, unit, rss, threads, cpu, ports) in DAEMONS {
        if !runs(&id, comm, world) {
            continue;
        }
        pid += 1 + rng.u32(0..6);
        let u = if *u == "{user}" { user.as_str() } else { u };
        let cmd = cmd.replace("{host}", &host);
        // The ones that came up late (or were restarted) have higher pids.
        let (pid_now, started) = match *comm {
            "ollama" => (2_175 - 7 * node.idx as u32, 18),
            "nginx" => (2_241, 19),
            "systemd" => (3_102 + 11 * node.idx as u32, 61),
            _ => (pid, 7 + (pid - base) as u64 / 9),
        };
        // systemd-resolved's two stub listeners come out of ss in each node's own order.
        let ports: &[(u16, &'static str)] = if *comm == "systemd-resolved" && matches!(id.as_str(), "cell2" | "cell3") { &[(53, "127.0.0.54"), (53, "127.0.0.53")] } else { ports };
        out.push(p(pid_now, comm, u, &cmd, *unit, *rss, *threads, *cpu, ports, started));
        if *comm == "nginx" {
            // One worker per core (worker_processes auto), as www-data.
            for w in 0..CORES as u32 {
                out.push(DemoProc { ppid: pid_now, ..p(pid_now + 1 + w, "nginx", "www-data", "nginx: worker process", Some("nginx.service"), 5.6, 1, 0.0, &[], 19) });
            }
        }
        if *comm == "lldpd" {
            out.push(DemoProc { ppid: pid, ..p(pid + 1, "lldpd", "_lldpd", "lldpd: no neighbor.", Some("lldpd.service"), 2.1, 1, 0.0, &[], 7) });
            pid += 1;
        }
        if *comm == "master" {
            out.push(DemoProc { ppid: pid, ..p(pid + 2, "pickup", "postfix", "pickup -l -t unix -u -c", Some("postfix@-.service"), 6.8, 1, 0.0, &[], 900) });
            pid += 2;
        }
    }
    // SparkBox's own: the collector its SSH session started, and the nvidia-smi it keeps running.
    let collector = node.alloc_pid(rng);
    node.collector_pid = collector;
    out.push(DemoProc {
        ppid: collector - 6,
        sparkbox: true,
        ..p(collector, "python3", &user, &format!("python3 -u {}/.sparkbox/collector.py", home()), None, 22.6, 5, 2.4, &[], 60)
    });
    out.push(DemoProc {
        ppid: collector,
        sparkbox: true,
        ..p(collector + 7, "nvidia-smi", &user, "nvidia-smi --query-gpu=utilization.gpu,temperature.gpu,power.draw,clocks.sm,driver_version,clocks.max.sm,power.limit,pstate,clocks_event_reasons.active --format=csv,noheader,nounits -lms 2000", None, 20.7, 2, 0.4, &[], 60)
    });
    node.next_pid = node.next_pid.max(collector + 8);
    out
}

/// What the node listens on that no process in the simulator owns: gx10-0000's loopback port
/// 43179 (nobody could tell whose without root).
pub(super) fn orphan_ports(node: &str) -> Vec<(u16, &'static str)> {
    if node == "gx10-0000" {
        vec![(43_179, "127.0.0.1")]
    } else {
        Vec::new()
    }
}

/// Postfix on gx10-0000 listens on loopback only (its main.cf says inet_interfaces = loopback-only).
pub(super) fn port_address(node: &str, port: u16, addr: &'static str) -> Option<&'static str> {
    match (node, port, addr) {
        ("gx10-0000", 25, "0.0.0.0") => Some("127.0.0.1"),
        ("gx10-0000", 25, "::") => Some("::1"),
        _ => Some(addr),
    }
}

// ---- Services

/// The 92 units the matrix lists, as this node had them (a unit "isn't installed" is absent).
/// `world`: the everyday world has no native Ollama or nginx.
pub(super) fn units(node: &str, world: World) -> Vec<Unit> {
    let mut out = Vec::new();
    for line in SERVICES.lines() {
        let parts: Vec<&str> = line.split(" | ").collect();
        if parts.len() < 3 {
            continue;
        }
        let description = parts[parts.len() - 1];
        let Some(cell) = parts[1..parts.len() - 1].iter().find(|c| c.contains(&format!(" on {node}: "))) else { continue };
        let Some((unit, state)) = cell.split_once(&format!(" on {node}: ")) else { continue };
        if world == World::Everyday && matches!(unit, "ollama.service" | "nginx.service") {
            continue;
        }
        // "active (running) · enabled at boot"
        let (status, boot) = state.split_once(" · ").unwrap_or((state, ""));
        let (active, sub) = status.split_once(" (").map(|(a, s)| (a, s.trim_end_matches(')'))).unwrap_or((status, ""));
        let enabled = boot.trim_end_matches(" at boot");
        out.push(Unit::found(unit, description, active, sub, if enabled == "?" { "" } else { enabled }, exec_of(unit)));
    }
    out
}

/// What a unit runs, for `systemctl status` (the daemons above; oneshots have their ExecStart).
fn exec_of(unit: &str) -> String {
    DAEMONS
        .iter()
        .find(|d| d.3 == Some(unit))
        .map(|d| d.2.split_whitespace().next().unwrap_or("").trim_start_matches('@').to_string())
        .unwrap_or_else(|| format!("/usr/lib/systemd/{}", unit.trim_end_matches(".service")))
}

/// Ollama's journal at boot on cell1 and cell3: the real lines from the tour's Services detail.
pub(super) fn ollama_journal(boot: u64, pid: u32) -> Vec<(u64, String)> {
    let t = |s: u64| super::ops::rfc3339_local(boot + s);
    let at = boot + 18;
    vec![
        (at, format!("ollama[{pid}]: time={} level=INFO source=routes.go:1810 msg=\"Listening on 127.0.0.1:11434 (version {OLLAMA_VERSION})\"", t(18))),
        (at, format!("ollama[{pid}]: time={} level=INFO source=runner.go:67 msg=\"discovering available GPUs...\"", t(18))),
        (at, format!("ollama[{pid}]: time={} level=INFO source=server.go:444 msg=\"starting runner\" cmd=\"/usr/local/bin/ollama runner --ollama-engine --port 34089\"", t(18))),
        (at + 4, format!("ollama[{pid}]: time={} level=INFO source=server.go:444 msg=\"starting runner\" cmd=\"/usr/local/bin/ollama runner --ollama-engine --port 37859\"", t(22))),
        (at + 4, format!("ollama[{pid}]: time={} level=INFO source=server.go:444 msg=\"starting runner\" cmd=\"/usr/local/bin/ollama runner --ollama-engine --port 37803\"", t(22))),
        (at + 5, format!("ollama[{pid}]: time={} level=INFO source=types.go:42 msg=\"inference compute\" id=GPU-a1f43b1d-b9ea-0187-31bc-4a654b409f02 filter_id=\"\" library=CUDA compute=12.1 name=CUDA0 description=\"NVIDIA GB10\" libdirs=ollama,cuda_v13 driver=13.0 pci_id=000f:01:00.0 type=iGPU total=\"121.7 GiB\" available=\"117.8 GiB\"", t(23))),
        (at + 5, format!("ollama[{pid}]: time={} level=INFO source=routes.go:1860 msg=\"vram-based default context\" total_vram=\"121.7 GiB\" default_num_ctx=262144", t(23))),
    ]
}

/// A `[GIN]` access line, as Ollama logs SparkBox's polls.
pub(super) fn gin_line(pid: u32, at: u64, path: &str, micros: u64) -> String {
    let (y, m, d, h, mi, s, _) = super::ops::civil_local(at);
    format!("ollama[{pid}]: [GIN] {y:04}/{m:02}/{d:02} - {h:02}:{mi:02}:{s:02} | 200 | {:>12} |       127.0.0.1 | GET      \"{path}\"", super::ops::micros(micros))
}

// ---- Updates

/// `apt list --upgradable` that morning: cell1 also has nginx; every node has Chrome and brotli.
pub(super) fn updates(node: &str) -> Vec<DemoPackage> {
    let pkg = |name: &str, from: &str, to: &str, origin: &str, security: bool| DemoPackage {
        name: name.into(),
        from: from.into(),
        to: to.into(),
        origin: origin.into(),
        security,
        held: false,
        reboot: false,
    };
    let mut out = vec![pkg("libbrotli1", "1.1.0-2build2", "1.1.0-2ubuntu0.1", "noble-updates,noble-security", true)];
    if node == "cell1" {
        out.push(pkg("nginx", "1.24.0-2ubuntu7.18", "1.24.0-2ubuntu7.19", "noble-updates,noble-security", true));
        out.push(pkg("nginx-common", "1.24.0-2ubuntu7.18", "1.24.0-2ubuntu7.19", "noble-updates,noble-security", true));
    }
    out.push(pkg("google-chrome-stable", "156.0.8078.47-1", "156.0.8078.73-1", "stable", false));
    out
}

/// When each node's apt lists were last refreshed (the periodic stamp): 29–50 minutes before.
pub(super) fn checked_ago_s(node: &str) -> u64 {
    match node {
        "cell1" => 30 * 60,
        "cell2" | "cell3" => 29 * 60,
        _ => 50 * 60,
    }
}

// ---- Kernel log

#[derive(Deserialize)]
struct Mlx5Line {
    node: String,
    #[serde(rename = "tsMs")]
    ts_ms: u64,
    kind: String,
    text: String,
    severity: String,
}

/// The recabling on Oct 26: 14 mlx5 lines per cell, 16 on gx10-0000, at the same time of day
/// three days before today.
pub(super) fn kernel_events(node: &str, now: u64) -> Vec<KernelEvent> {
    KERNEL_MLX5
        .lines()
        .filter_map(|l| serde_json::from_str::<Mlx5Line>(l).ok())
        .filter(|l| l.node == node)
        .map(|l| KernelEvent {
            ts_ms: found(l.ts_ms / 1000, now) * 1000 + l.ts_ms % 1000,
            kind: l.kind,
            code: None,
            text: l.text,
            severity: l.severity,
            process: None,
            ..Default::default()
        })
        .collect()
}

// ---- Containers

/// The real ids (docker's 64 hex), names and times of the containers on each node.
struct Real {
    node: &'static str,
    id: &'static str,
    name: &'static str,
    image: &'static str,
    command: &'static str,
    started: u64,
    finished: u64,
    exit: i32,
}

const CONTAINERS: &[Real] = &[
    Real { node: "cell1", id: "7da4d848a41aefe02d3543e6c8cc46326426cadaa673f0c6cd707e95b0ceb135", name: "glm53", image: GLM53_IMAGE, command: "/bin/bash /glm53/inner.sh 0", started: 1_793_053_226, finished: 1_793_053_280, exit: 1 },
    Real { node: "cell2", id: "2e35a6ef4126ae096e64798c7a2b301a8accea2d308dca36a073991974046ae5", name: "glm53", image: GLM53_IMAGE, command: "/bin/bash /glm53/inner.sh 1", started: 1_793_053_226, finished: 1_793_053_279, exit: 1 },
    Real { node: "cell3", id: "015bcb21d2bdae24150ac8921a1f0ed545a14f05b3f192a407264903c299e1d2", name: "glm53", image: GLM53_IMAGE, command: "/bin/bash /glm53/inner.sh 2", started: 1_793_053_225, finished: 1_793_053_280, exit: 1 },
    Real { node: "cell3", id: "2dd50c8247bbc5c88518fe4a2e1fd16b7ee322e7ac6464e7aab16c1e60e4e1f2", name: "workbench-proxy", image: TRAEFIK_IMAGE, command: "/entrypoint.sh traefik", started: 1_792_080_503, finished: 1_792_178_828, exit: 0 },
    Real { node: "gx10-0000", id: "9eadeae9efaf0cdd942d04cb364cb0c5b9da08bf4623338d6b17b4dffbb13705", name: "glm53", image: GLM53_IMAGE, command: "/bin/bash /glm53/inner.sh 3", started: 1_793_053_224, finished: 1_793_053_279, exit: 1 },
    Real { node: "gx10-0000", id: "22e0f93fdd264d70dec7b4a081e0103fdac73bb71ed522b42b42dbccc89dd154", name: "vigilant_hofstadter", image: GX10_VLLM_IMAGE, command: "/opt/nvidia/nvidia_entrypoint.sh vllm serve openai/gpt-oss-20b", started: 1_777_186_384, finished: 1_777_186_771, exit: 0 },
    Real { node: "gx10-0000", id: "be055f9ffc733ec0bb64780848c06e4793f0bfb13a4a0d9cc21d647cd6446127", name: "adoring_northcutt", image: CUDA_IMAGE, command: "/opt/nvidia/nvidia_entrypoint.sh nvidia-smi", started: 1_777_185_292, finished: 1_777_185_292, exit: 0 },
    Real { node: "gx10-0000", id: "0f42a56925ba44b8af5620cc3e86395a5238d63083d84c2d212a02419948961c", name: "jovial_elgamal", image: CUDA_IMAGE, command: "/opt/nvidia/nvidia_entrypoint.sh nvidia-smi", started: 1_777_185_282, finished: 1_777_185_282, exit: 0 },
    Real { node: "gx10-0000", id: "40a05cea602d1cc22da438caba1985c1d5ff3f89a85f8076940c4ded2bb67e3f", name: "gracious_babbage", image: CUDA_IMAGE, command: "/opt/nvidia/nvidia_entrypoint.sh nvidia-smi", started: 1_777_185_261, finished: 1_777_185_262, exit: 0 },
];

fn labels(image: &str) -> BTreeMap<String, String> {
    let pairs: &[(&str, &str)] = match image {
        GLM53_IMAGE => &[
            ("ai.vllm.build.commit", "385dce36bcee42309924a5ece951a96db3dce7f2"),
            ("ai.vllm.build.pipeline", "019d130e-464e-4ff7-b84b-492992c0c06b"),
            ("ai.vllm.build.url", "https://buildkite.com/vllm/release-v2/builds/6303"),
            ("ai.vllm.image.tag", "vllm/vllm-openai:nightly-385dce36bcee42309924a5ece951a96db3dce7f2"),
            ("maintainer", "NVIDIA CORPORATION <cudatools@nvidia.com>"),
            ("org.opencontainers.image.ref.name", "ubuntu"),
            ("org.opencontainers.image.revision", "385dce36bcee42309924a5ece951a96db3dce7f2"),
            ("org.opencontainers.image.source", "https://github.com/vllm-project/vllm"),
            ("org.opencontainers.image.url", "https://buildkite.com/vllm/release-v2/builds/6303"),
            ("org.opencontainers.image.version", "vllm/vllm-openai:nightly-385dce36bcee42309924a5ece951a96db3dce7f2"),
        ],
        GX10_VLLM_IMAGE => &[
            ("com.nvidia.build.id", "295860572"),
            ("com.nvidia.build.ref", "c9cbd3a6f46587fcce010740e6c055da7ddc2506"),
            ("com.nvidia.cublas.version", "13.3.0.5"),
            ("com.nvidia.cuda.version", "9.0"),
            ("com.nvidia.cudnn.version", "9.20.0.48"),
            ("com.nvidia.cufft.version", "12.2.0.37"),
            ("com.nvidia.curand.version", "10.4.2.51"),
            ("com.nvidia.cusolver.version", "12.1.0.51"),
            ("com.nvidia.cusparse.version", "12.7.9.17"),
            ("com.nvidia.cusparselt.version", "0.9.0.3"),
            ("com.nvidia.nccl.version", "2.29.7"),
            ("com.nvidia.npp.version", "13.1.0.44"),
            ("com.nvidia.nvjpeg.version", "13.0.4.44"),
            ("com.nvidia.vllm.version", "0.17.1+bd67d66a"),
            ("com.nvidia.volumes.needed", "nvidia_driver"),
            ("org.opencontainers.image.ref.name", "ubuntu"),
            ("org.opencontainers.image.version", "24.04"),
        ],
        CUDA_IMAGE => &[("maintainer", "NVIDIA CORPORATION <cudatools@nvidia.com>"), ("org.opencontainers.image.ref.name", "ubuntu"), ("org.opencontainers.image.version", "24.04")],
        TRAEFIK_IMAGE => &[
            ("org.opencontainers.image.description", "A modern reverse-proxy"),
            ("org.opencontainers.image.documentation", "https://docs.traefik.io"),
            ("org.opencontainers.image.source", "https://github.com/traefik/traefik"),
            ("org.opencontainers.image.title", "Traefik"),
            ("org.opencontainers.image.url", "https://traefik.io"),
            ("org.opencontainers.image.vendor", "Traefik Labs"),
            ("org.opencontainers.image.version", "v3.6.1"),
        ],
        _ => &[],
    };
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The CUDA base image's driver requirement: every branch it supports, on one 3,000-character line.
fn require_cuda() -> String {
    let brands = ["unknown", "grid", "tesla", "nvidia", "quadro", "quadrortx", "nvidiartx", "vapps", "vpc", "vcs", "vws", "cloudgaming"];
    let mut s = "cuda>=13.0".to_string();
    for d in [535, 550, 565, 570, 575, 580] {
        for b in brands {
            s += &format!(" brand={b},driver>={d},driver<{}", d + 1);
        }
    }
    s
}

/// glm53's environment as Inspect showed it (the same on all four ranks). API_KEY is a
/// placeholder; MTP_TOKENS is the 5 the log prints.
fn glm53_env() -> Vec<String> {
    let mut env: Vec<String> = [
        "NCCL_IGNORE_CPU_AFFINITY=1",
        "PORT=8000",
        "GPU_MEM_UTIL=0.87",
        "MN_IF_NAME=enp1s0f1np1",
        "TP_SOCKET_IFNAME=enp1s0f1np1",
        "HEAD_IP=192.168.100.1",
        "SERVED_NAME=glm-5.3-flash",
        "MTP_TOKENS=5",
        "MOE_BACKEND=",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    env.push(format!("API_KEY={API_KEY_PLACEHOLDER}"));
    env.extend(
        [
            "UCX_NET_DEVICES=enp1s0f1np1",
            "NCCL_IB_HCA=rocep1s0f1,roceP2p1s0f1",
            "OMPI_MCA_btl_tcp_if_include=enp1s0f1np1",
            "MODEL_LOCAL_NAME=GLM-5.3-Flash-NVFP4",
            "NCCL_SOCKET_IFNAME=enp1s0f1np1",
            "NCCL_IB_DISABLE=0",
            "GLOO_SOCKET_IFNAME=enp1s0f1np1",
            "MASTER_PORT=29501",
            "MAX_MODEL_LEN=524288",
            "MAX_NUM_SEQS=8",
            "NNODES=4",
            "PATH=/usr/local/cuda/bin:/usr/local/nvidia/bin:/usr/local/cuda/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            "NVARCH=sbsa",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    env.push(format!("NVIDIA_REQUIRE_CUDA={}", require_cuda()));
    env.extend(
        ["CUDA_VERSION=13.0.1", "LD_LIBRARY_PATH=/usr/local/nvidia/lib:/usr/local/nvidia/lib64", "NVIDIA_VISIBLE_DEVICES=all", "NVIDIA_DRIVER_CAPABILITIES=compute,utility", "VLLM_USAGE_SOURCE=production-docker-image"]
            .iter()
            .map(|s| s.to_string()),
    );
    env
}

/// The parts of `docker inspect` the simulator can't derive from the kind: glm53's mounts,
/// host networking and 16 GiB of shared memory; Traefik on AI Workbench's bridge network.
fn inspect_extra(image: &str, full_id: &str, started: u64) -> Value {
    let created = super::ops::iso(started.saturating_sub(2));
    let home = home();
    let bind = |src: &str, dst: &str, rw: bool| json!({ "Type": "bind", "Source": src, "Destination": dst, "Mode": if rw { "" } else { "ro" }, "RW": rw, "Propagation": "rprivate" });
    match image {
        GLM53_IMAGE => json!({
            "Id": full_id,
            "Created": created,
            "HostConfig": { "NetworkMode": "host", "IpcMode": "host", "ShmSize": 17_179_869_184u64, "DeviceRequests": [{ "Driver": "", "Count": -1, "Capabilities": [["gpu"]] }], "RestartPolicy": { "Name": "no", "MaximumRetryCount": 0 } },
            "Mounts": [
                bind(&format!("{home}/.cache/flashinfer"), "/root/.cache/flashinfer", true),
                bind(&format!("{home}/.triton"), "/root/.triton", true),
                bind(&format!("{home}/glm53"), "/glm53", false),
                bind(&format!("{home}/models"), "/models", false),
                bind(&format!("{home}/.cache/vllm"), "/root/.cache/vllm", true),
            ],
            "Config": { "Entrypoint": [], "WorkingDir": "/vllm-workspace" },
            "NetworkSettings": { "Ports": {}, "Networks": { "host": { "IPAddress": "" } } },
        }),
        GX10_VLLM_IMAGE | CUDA_IMAGE => json!({
            "Id": full_id,
            "Created": created,
            "HostConfig": { "NetworkMode": "bridge", "IpcMode": "private", "ShmSize": 67_108_864, "DeviceRequests": [{ "Driver": "", "Count": -1, "Capabilities": [["gpu"]] }], "RestartPolicy": { "Name": "no", "MaximumRetryCount": 0 } },
            "Config": { "Entrypoint": ["/opt/nvidia/nvidia_entrypoint.sh"] },
        }),
        TRAEFIK_IMAGE => json!({
            "Id": full_id,
            "Created": created,
            "HostConfig": { "NetworkMode": "workbench", "RestartPolicy": { "Name": "no", "MaximumRetryCount": 0 } },
            "Mounts": [{ "Type": "bind", "Source": "/var/run/docker.sock", "Destination": "/var/run/docker.sock", "Mode": "ro", "RW": false, "Propagation": "rprivate" }],
            "NetworkSettings": { "Ports": {}, "Networks": { "workbench": { "IPAddress": "", "Gateway": "" } } },
        }),
        _ => Value::Null,
    }
}

/// The containers on a node in the as-found world: every one exited.
pub(super) fn containers(node: &DemoNode, now: u64) -> Vec<DemoContainer> {
    CONTAINERS
        .iter()
        .filter(|r| r.node == node.cfg.id)
        .map(|r| {
            let (started, finished) = (found(r.started, now), found(r.finished, now));
            let env = if r.image == GLM53_IMAGE { glm53_env() } else if r.image == TRAEFIK_IMAGE { vec!["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into()] } else { vec!["NCCL_VERSION=".to_string() + if r.image == CUDA_IMAGE { "2.28.3-1" } else { "2.29.7" }] };
            DemoContainer {
                c: Container {
                    id: r.id[..12].to_string(),
                    name: r.name.into(),
                    image: r.image.into(),
                    state: "exited".into(),
                    started_at: started,
                    finished_at: finished,
                    exit_code: r.exit,
                    command: r.command.into(),
                    restart_policy: Some("no".into()),
                    labels: labels(r.image),
                    ..Default::default()
                },
                kind: Kind::Plain,
                pid: 0,
                proc_name: if r.image == TRAEFIK_IMAGE { "traefik".into() } else { "python3".into() },
                gpu_mem: 0.0,
                rss: 0.0,
                idle_cpu: 0.0,
                busy_cpu: 0.0,
                nice: 0,
                threads: 0,
                frozen: false,
                manual_stop: false,
                env,
                listen: Vec::new(),
                exit_at: None,
                cpu_time: 0.0,
                inspect: inspect_extra(r.image, r.id, started),
                log: Some(log_of(r, node.idx)),
            }
        })
        .collect()
}

/// What `docker logs` prints for a container that has stopped.
fn log_of(r: &Real, _index: usize) -> String {
    match r.image {
        GLM53_IMAGE => {
            let rank = r.command.rsplit(' ').next().unwrap_or("0");
            if rank == "0" {
                GLM53_LOG.to_string()
            } else {
                // The workers start headless: the same engine and the same failure, under their own rank.
                GLM53_LOG.replace("from node_rank 0", &format!("from node_rank {rank}")).replace("data_parallel_rank 0", &format!("data_parallel_rank {rank}"))
            }
        }
        GX10_VLLM_IMAGE => {
            let d = |s: u64| super::ops::log_stamp(r.started + s);
            format!(
                "\n==========\n== vLLM ==\n==========\n\nNVIDIA Release 26.03.post1 (build 295860572)\nvLLM Version 0.17.1+bd67d66a\n\n\
                 INFO {} [api_server.py:1880] vLLM API server version 0.17.1+bd67d66a\n\
                 INFO {} [utils.py:326] non-default args: {{'model_tag': 'openai/gpt-oss-20b', 'model': 'openai/gpt-oss-20b'}}\n\
                 INFO {} [model.py:547] Resolved architecture: GptOssForCausalLM\n\
                 INFO {} [weight_utils.py:392] Using model weights format ['*.safetensors']\n\
                 INFO {} [default_loader.py:308] Loading weights took 41.83 seconds\n\
                 INFO {} [gpu_model_runner.py:2620] Model loading took 13.7194 GiB and 186.40 seconds\n\
                 INFO {} [gpu_worker.py:298] Available KV cache memory: 89.02 GiB\n\
                 INFO {} [api_server.py:1955] Starting vLLM API server 0 on http://0.0.0.0:8000\n\
                 INFO:     Started server process [1]\n\
                 INFO:     Application startup complete.\n\
                 INFO:     Shutting down\n\
                 INFO:     Waiting for application shutdown.\n\
                 INFO:     Application shutdown complete.\n\
                 INFO:     Finished server process [1]\n",
                d(3), d(3), d(9), d(14), d(58), d(204), d(262), d(301)
            )
        }
        CUDA_IMAGE => "\n==========\n== CUDA ==\n==========\n\nCUDA Version 13.0.1\n\nContainer image Copyright (c) 2016-2025, NVIDIA CORPORATION & AFFILIATES. All rights reserved.\n\n\
             This container image and its contents are governed by the NVIDIA Deep Learning Container License.\n\
             By pulling and using the container, you accept the terms and conditions of this license:\n\
             https://developer.nvidia.com/ngc/nvidia-deep-learning-container-license\n\n\
             +-----------------------------------------------------------------------------------------+\n\
             | NVIDIA-SMI 580.95.05              Driver Version: 580.95.05      CUDA Version: 13.0     |\n\
             +-----------------------------------------+------------------------+----------------------+\n\
             |   0  NVIDIA GB10                    On  |   0000000F:01:00.0 Off |                  N/A |\n\
             | N/A   41C    P8              4W /  N/A  | Not Supported          |      0%      Default |\n\
             +-----------------------------------------+------------------------+----------------------+\n"
            .to_string(),
        TRAEFIK_IMAGE => {
            let d = |s: u64| super::ops::iso_seconds(r.started + s);
            let e = |s: u64| super::ops::iso_seconds(r.finished.saturating_sub(s));
            format!(
                "{} INF Traefik version 3.6.1 built on 2025-11-19T14:02:08Z version=3.6.1\n\
                 {} INF Starting provider aggregator *aggregator.ProviderAggregator\n\
                 {} INF Starting provider *docker.Provider\n\
                 {} INF I have to go...\n\
                 {} INF Stopping server gracefully\n\
                 {} INF Server stopped\n\
                 {} INF Shutting down\n",
                d(0), d(0), d(1), e(1), e(1), e(0), e(0)
            )
        }
        _ => String::new(),
    }
}

// ---- Images

/// One line of `docker image ls --format '{{json .}}'`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DemoImage {
    pub repository: String,
    pub tag: String,
    #[serde(rename = "ID")]
    pub id: String,
    pub created_at: String,
    /// Bytes on disk.
    #[serde(skip)]
    pub bytes: u64,
    /// "21.3GB", as Docker prints it.
    pub size: String,
}

/// The images each node holds: the glm53 build on all four; Traefik on cell3; gx10-0000's
/// NGC vLLM and CUDA images from April.
pub(super) fn images(node: &str, now: u64) -> Vec<DemoImage> {
    let img = |repo_tag: &str, id: &str, created: u64, bytes: u64| {
        let (repository, tag) = repo_tag.rsplit_once(':').unwrap_or((repo_tag, "latest"));
        DemoImage { repository: repository.into(), tag: tag.into(), id: id.into(), created_at: super::ops::docker_created(created), bytes, size: format!("{:.1}GB", bytes as f64 / 1e9).replace("0.2GB", "184MB") }
    };
    let mut out = vec![img(GLM53_IMAGE, "4b1f2d7e9a03", found(1_792_984_311, now), 21_304_118_272)];
    if node == "cell3" {
        out.push(img(TRAEFIK_IMAGE, "8c7e5f0a91d2", found(1_791_566_528, now), 184_338_432));
    }
    if node == "gx10-0000" {
        out.push(img(GX10_VLLM_IMAGE, "f3a8b61c0e47", found(1_774_014_220, now), 13_812_240_384));
        out.push(img(CUDA_IMAGE, "a90c4e21b7d5", found(1_757_400_812, now), 5_934_862_336));
    }
    out
}
