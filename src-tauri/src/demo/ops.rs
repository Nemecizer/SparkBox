//! What Operations do to the simulated cluster: systemd services and their
//! journals, signals, reboots and shutdowns, apt updates, Fleet Command's
//! output, and containers added by Models. Every reply is generated from the
//! node's current state, so Fleet's `docker ps` lists exactly what the
//! Containers window shows and `df` matches the disk bars.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::world::*;
use super::{base_procs, make_container, unix_now, DemoCluster, DemoContainer, DemoNode, Kind, Power, Role, GIB};

/// One systemd service, shaped like `systemctl list-units`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoUnit {
    pub unit: String,
    pub description: String,
    /// loaded | not-found | masked
    pub load: String,
    /// active | inactive | failed | activating
    pub active: String,
    /// running | exited | dead | failed
    pub sub: String,
    /// enabled | disabled | static | masked
    pub enabled: String,
}

/// A service as the simulator keeps it: its state, what it runs and its journal.
#[derive(Clone)]
pub(super) struct Unit {
    pub unit: String,
    pub description: String,
    pub active: String,
    pub sub: String,
    pub enabled: String,
    /// Fails every time it starts (nvidia-dgx-telemetry on cell1, in the default world).
    pub broken: bool,
    pub exec: String,
    pub since: u64,
    pub main_pid: Option<u32>,
    /// The journal: (unix time, "ident[pid]: message").
    pub log: Vec<(u64, String)>,
    /// Its process while stopped, so starting it brings the same program back.
    parked: Option<super::DemoProc>,
    /// Runs once and stays "active (exited)" (apparmor, systemd-sysctl…).
    pub oneshot: bool,
    /// Started by a timer and gone again a minute later (anacron).
    pub timer: bool,
}

/// A pending package upgrade, as `apt list --upgradable` shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoPackage {
    pub name: String,
    pub from: String,
    pub to: String,
    /// noble-updates | noble-security | nvidia | docker
    pub origin: String,
    pub security: bool,
    /// `apt-mark hold`: apt won't upgrade it.
    pub held: bool,
    /// Takes effect only after a reboot (kernel, driver).
    pub reboot: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoUpdates {
    pub node: String,
    pub packages: Vec<DemoPackage>,
    /// Updates already installed are waiting for a reboot.
    pub reboot_required: bool,
    /// Unix seconds of the last `apt update`.
    pub checked_at: u64,
    /// Unattended upgrades are on (APT::Periodic::Unattended-Upgrade): off on all four, as found.
    pub unattended: bool,
}

/// A container Models starts in the simulator (a vLLM deployment's head or worker…).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoContainerSpec {
    pub name: String,
    pub image: String,
    /// Host ports it listens on (vLLM uses host networking, so these are the node's ports).
    pub ports: Vec<u16>,
    /// Bytes of GPU memory once loaded.
    pub gpu_mem: u64,
    /// Bytes of CPU memory.
    pub rss: u64,
    pub labels: BTreeMap<String, String>,
    pub command: String,
    pub env: Vec<String>,
    /// Defaults to "no", like `docker run`.
    pub restart_policy: Option<String>,
}

// ---- Time formatting (UTC, without a date library)

/// (year, month, day, hour, minute, second, weekday with 0 = Sunday).
pub(crate) fn civil(unix: u64) -> (i64, u32, u32, u32, u32, u32, usize) {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let wd = (days + 4).rem_euclid(7) as usize; // 1970-01-01 was a Thursday
    (y, m, d, (secs / 3_600) as u32, (secs / 60 % 60) as u32, (secs % 60) as u32, wd)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Docker's timestamps: 2026-10-26T09:14:02.000000000Z
pub(crate) fn iso(unix: u64) -> String {
    let (y, m, d, h, mi, s, _) = civil(unix);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.000000000Z")
}

/// systemctl's: Mon 2026-10-26 09:14:02 UTC
fn systemd_time(unix: u64) -> String {
    let (y, m, d, h, mi, s, wd) = civil(unix);
    format!("{} {y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC", DAYS[wd])
}

/// journalctl's: Oct 26 09:14:02
fn journal_time(unix: u64) -> String {
    let (_, m, d, h, mi, s, _) = civil(unix);
    format!("{} {d:02} {h:02}:{mi:02}:{s:02}", MONTHS[m as usize - 1])
}

/// systemd's "2 days ago", "3h 12min ago", "41s ago".
fn systemd_ago(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3_599 => format!("{}min {}s ago", secs / 60, secs % 60),
        3_600..=86_399 => format!("{}h {}min ago", secs / 3_600, secs / 60 % 60),
        _ => format!("{} day{} ago", secs / 86_400, if secs / 86_400 == 1 { "" } else { "s" }),
    }
}

/// Docker's HumanDuration, for `docker ps` STATUS.
fn docker_duration(secs: u64) -> String {
    let hours = (secs as f64 / 3_600.0 + 0.5) as u64;
    match secs {
        0 => "Less than a second".into(),
        1 => "1 second".into(),
        2..=59 => format!("{secs} seconds"),
        60..=119 => "About a minute".into(),
        120..=3_599 => format!("{} minutes", secs / 60),
        _ if hours == 1 => "About an hour".into(),
        _ if hours < 48 => format!("{hours} hours"),
        _ if hours < 24 * 14 => format!("{} days", hours / 24),
        _ if hours < 24 * 60 => format!("{} weeks", hours / 24 / 7),
        _ => format!("{} months", hours / 24 / 30),
    }
}

/// The same, in the owner's local time (the nodes keep the Mac's time zone, America/New_York).
pub(crate) fn civil_local(unix: u64) -> (i64, u32, u32, u32, u32, u32, usize) {
    civil((unix as i64 + crate::health::maintenance::local_offset_s()).max(0) as u64)
}

/// Go's RFC 3339 with milliseconds and the local offset, as Ollama logs: 2026-10-25T17:08:11.989-04:00
pub(crate) fn rfc3339_local(unix: u64) -> String {
    let off = crate::health::maintenance::local_offset_s();
    let (y, m, d, h, mi, s, _) = civil_local(unix);
    let ms = (unix.wrapping_mul(7_919) % 1000) as u32;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}{}{:02}:{:02}", if off < 0 { '-' } else { '+' }, off.abs() / 3600, off.abs() / 60 % 60)
}

/// A Go duration as gin prints it: "33.408247ms", "863.424µs".
pub(crate) fn micros(us: u64) -> String {
    if us >= 1000 {
        format!("{}.{:06}ms", us / 1000, (us % 1000) * 1000 + us % 997)
    } else {
        format!("{us}.{:03}µs", us.wrapping_mul(31) % 1000)
    }
}

/// vLLM's log stamp (the container runs on UTC): "10-26 22:20:41".
pub(crate) fn log_stamp(unix: u64) -> String {
    let (_, m, d, h, mi, s, _) = civil(unix);
    format!("{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// RFC 3339 to the second, UTC: 2026-10-13T09:28:23Z
pub(crate) fn iso_seconds(unix: u64) -> String {
    let (y, m, d, h, mi, s, _) = civil(unix);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `docker image ls`'s CreatedAt, in the node's zone: 2026-10-26 14:31:51 -0400 EDT
pub(crate) fn docker_created(unix: u64) -> String {
    let off = crate::health::maintenance::local_offset_s();
    let zone = match off {
        -14_400 => "EDT",
        -18_000 => "EST",
        0 => "UTC",
        _ => "",
    };
    let (y, m, d, h, mi, s, _) = civil_local(unix);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} {}{:02}{:02} {zone}", if off < 0 { '-' } else { '+' }, off.abs() / 3600, off.abs() / 60 % 60).trim_end().to_string()
}

/// `df -h` / `free -h` style sizes: 3.7T, 952G, 119Gi…
fn human(bytes: f64, suffix: &str) -> String {
    let units = ["B", "K", "M", "G", "T", "P"];
    let mut v = bytes;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < units.len() {
        v /= 1024.0;
        i += 1;
    }
    let unit = if i == 0 { "B".to_string() } else { format!("{}{suffix}", units[i]) };
    if v < 10.0 && i > 0 {
        format!("{:.1}{unit}", (v * 10.0).ceil() / 10.0)
    } else {
        format!("{:.0}{unit}", v.ceil())
    }
}

// ---- Baselines

impl Unit {
    /// A unit as the services matrix of the Live tour lists it.
    pub(super) fn found(unit: &str, description: &str, active: &str, sub: &str, enabled: &str, exec: String) -> Unit {
        Unit {
            unit: unit.into(),
            description: description.into(),
            active: active.into(),
            sub: sub.into(),
            enabled: enabled.into(),
            broken: false,
            exec,
            since: 0,
            main_pid: None,
            log: Vec::new(),
            parked: None,
            oneshot: sub == "exited",
            timer: unit == "anacron.service",
        }
    }
}

/// The services a node has: the 92 units of the real machines (the everyday world without the
/// native Ollama and nginx). `telemetry_fails`: nvidia-dgx-telemetry can't open its state file.
pub(super) fn base_units(node: &str, world: World, telemetry_fails: bool) -> Vec<Unit> {
    let mut units = super::found::units(node, world);
    if telemetry_fails {
        if let Some(u) = units.iter_mut().find(|u| u.unit == "nvidia-dgx-telemetry.service") {
            u.broken = true;
        }
    }
    units
}

/// Pending upgrades: the real ones (apt list --upgradable on Oct 29), the same in every world.
pub(super) fn base_updates(node: &str) -> Vec<DemoPackage> {
    super::found::updates(node)
}

fn unit_ok(unit: &str) -> bool {
    !unit.is_empty() && unit.len() <= 200 && !unit.starts_with('-') && unit.chars().all(|c| c.is_ascii_alphanumeric() || "_.-@:\\".contains(c))
}

fn hostname(n: &DemoNode) -> String {
    n.cfg.host.trim_end_matches(".local").to_string()
}

impl Unit {
    fn view(&self) -> DemoUnit {
        DemoUnit {
            unit: self.unit.clone(),
            description: self.description.clone(),
            load: "loaded".into(),
            active: self.active.clone(),
            sub: self.sub.clone(),
            enabled: self.enabled.clone(),
        }
    }

    fn short(&self) -> &str {
        self.unit.trim_end_matches(".service")
    }

    fn push(&mut self, at: u64, line: String) {
        self.log.push((at, line));
        if self.log.len() > 400 {
            self.log.drain(..100);
        }
    }
}

/// Records a unit starting (or failing to) at `at`, with the lines its journal would get.
fn start_unit(n: &mut DemoNode, idx: usize, at: u64) -> Result<(), String> {
    let pid = n.next_pid + 17;
    n.next_pid = pid;
    let u = &mut n.units[idx];
    let ident = u.short().to_string();
    u.push(at, format!("systemd[1]: Starting {} - {}...", u.unit, u.description));
    if u.broken {
        u.push(at, format!("systemd[1]: Started {} - {}.", u.unit, u.description));
        u.push(at, format!("{ident}[{pid}]: telemetry: loading /etc/nvidia-dgx-telemetry/config.yaml"));
        u.push(at, format!("{ident}[{pid}]: telemetry: cannot open /var/lib/nvidia-dgx-telemetry/state.db: Permission denied"));
        u.push(at, format!("systemd[1]: {}: Main process exited, code=exited, status=1/FAILURE", u.unit));
        u.push(at, format!("systemd[1]: {}: Failed with result 'exit-code'.", u.unit));
        u.active = "failed".into();
        u.sub = "failed".into();
        u.since = at;
        u.main_pid = Some(pid);
        if let Some(p) = n.procs.iter().position(|p| p.unit.as_deref() == Some(n.units[idx].unit.as_str())) {
            n.units[idx].parked = Some(n.procs.remove(p));
        }
        let u = &n.units[idx];
        return Err(format!(
            "Job for {0} failed because the control process exited with error code.\nSee \"systemctl status {0}\" and \"journalctl -xeu {0}\" for details.",
            u.unit
        ));
    }
    u.push(at, format!("systemd[1]: {} {} - {}.", if u.oneshot { "Finished" } else { "Started" }, u.unit, u.description));
    u.active = "active".into();
    u.sub = if u.oneshot { "exited" } else { "running" }.into();
    u.since = at;
    u.main_pid = Some(pid);
    let name = u.unit.clone();
    if let Some(mut p) = u.parked.take() {
        p.pid = pid;
        p.started_s = at.saturating_sub(n.boot_unix);
        p.cpu_time = 0.0;
        p.stopped = false;
        n.procs.push(p);
    } else if let Some(p) = n.procs.iter_mut().find(|p| p.unit.as_deref() == Some(name.as_str())) {
        n.units[idx].main_pid = Some(p.pid);
    }
    Ok(())
}

fn stop_unit(n: &mut DemoNode, idx: usize, at: u64, failed: bool) {
    let name = n.units[idx].unit.clone();
    if let Some(p) = n.procs.iter().position(|p| p.unit.as_deref() == Some(name.as_str())) {
        n.units[idx].parked = Some(n.procs.remove(p));
    }
    let u = &mut n.units[idx];
    if failed {
        u.push(at, format!("systemd[1]: {}: Main process exited, code=killed, status=9/KILL", u.unit));
        u.push(at, format!("systemd[1]: {}: Failed with result 'signal'.", u.unit));
        u.active = "failed".into();
        u.sub = "failed".into();
    } else {
        u.push(at, format!("systemd[1]: Stopping {} - {}...", u.unit, u.description));
        u.push(at, format!("systemd[1]: {}: Deactivated successfully.", u.unit));
        u.push(at, format!("systemd[1]: Stopped {} - {}.", u.unit, u.description));
        u.active = "inactive".into();
        u.sub = "dead".into();
    }
    u.since = at;
    u.main_pid = None;
}

/// Brings a node up after a reboot or wake: fresh processes, enabled services started,
/// containers back according to their restart policy.
fn boot(n: &mut DemoNode, rng: &mut fastrand::Rng, now: u64) {
    n.boot_unix = now;
    n.boots += 1;
    // A new boot: a new boot_id, and the kernel log starts over (a latched Xid clears with it).
    n.kernel_events.clear();
    if let Some((driver, kernel)) = n.pending_boot.take() {
        if let Some(d) = driver {
            n.driver = d;
        }
        if let Some(k) = kernel {
            n.kernel = k;
        }
    }
    n.next_pid = 900;
    n.procs = base_procs(n, rng);
    for i in 0..n.units.len() {
        n.units[i].log.clear();
        n.units[i].parked = None;
        let at = now + 3 + i as u64 % 5;
        if n.units[i].enabled == "enabled" {
            let _ = start_unit(n, i, at);
        } else {
            let name = n.units[i].unit.clone();
            if let Some(p) = n.procs.iter().position(|p| p.unit.as_deref() == Some(name.as_str())) {
                n.units[i].parked = Some(n.procs.remove(p));
            }
            let u = &mut n.units[i];
            u.active = "inactive".into();
            u.sub = "dead".into();
            u.since = now;
            u.main_pid = None;
        }
    }
    n.counters = [super::Counters { carrier_changes: 2, ..Default::default() }; 2];
    n.carrier_was = true;
    for c in n.containers.iter_mut() {
        let policy = c.c.restart_policy.as_deref().unwrap_or("no");
        if c.c.state == "exited" && !c.manual_stop && matches!(policy, "always" | "unless-stopped") {
            c.c.state = "running".into();
            c.c.started_at = now + 9;
            c.c.exit_code = 0;
            c.c.oom_killed = false;
            n.next_pid += rng.u32(40..400);
            c.pid = n.next_pid;
            c.cpu_time = 0.0;
        }
    }
}

/// Advances reboots, shutdowns and wakes by one tick.
pub(super) fn power_step(d: &mut DemoCluster, now: u64) {
    let rng = &mut d.rng;
    for n in d.nodes.iter_mut() {
        n.power = match n.power {
            Power::On | Power::Off => n.power,
            Power::Rebooting { down, off, up } if down > 0 => Power::Rebooting { down: down - 1, off, up },
            Power::Rebooting { down, off, up } if off > 0 => Power::Rebooting { down, off: off - 1, up },
            Power::Rebooting { up, .. } if up > 1 => Power::Rebooting { down: 0, off: 0, up: up - 1 },
            Power::Rebooting { .. } => {
                boot(n, rng, now);
                Power::On
            }
            Power::ShuttingDown { left } if left > 1 => Power::ShuttingDown { left: left - 1 },
            Power::ShuttingDown { .. } => Power::Off,
            Power::Waking { off, up } if off > 0 => Power::Waking { off: off - 1, up },
            Power::Waking { up, .. } if up > 1 => Power::Waking { off: 0, up: up - 1 },
            Power::Waking { .. } => {
                boot(n, rng, now);
                Power::On
            }
        };
    }
}

/// Timer-driven units come and go: anacron runs for a few minutes out of every five, at its own
/// moment on each node, and is "inactive (dead)" in between, which is how a healthy one looks.
pub(super) fn timers(d: &mut DemoCluster, now: u64) {
    let minute = d.sim_ms() / 60_000;
    for n in d.nodes.iter_mut().filter(|n| n.power == Power::On) {
        let on = (minute + 2 * n.idx as u64) % 5 < 3;
        for i in 0..n.units.len() {
            if !n.units[i].timer || (n.units[i].active == "active") == on || n.units[i].active == "failed" {
                continue;
            }
            if on {
                let _ = start_unit(n, i, now);
            } else {
                let u = &mut n.units[i];
                u.push(now, format!("anacron[{}]: Normal exit (0 jobs run)", u.main_pid.unwrap_or(0)));
                u.push(now, format!("systemd[1]: {}: Deactivated successfully.", u.unit));
                u.active = "inactive".into();
                u.sub = "dead".into();
                u.since = now;
                u.main_pid = None;
            }
        }
    }
}

/// Columns aligned the way Go's tabwriter does for `docker ps --format table` (min width 10, padding 3).
fn table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| s.chars().count()).max().unwrap_or(0).saturating_add(3).max(10))
        .collect();
    let mut out = String::new();
    for r in rows {
        for (c, cell) in r.iter().enumerate() {
            if c + 1 == r.len() {
                out.push_str(cell);
            } else {
                out.push_str(&format!("{cell:<w$}", w = widths[c]));
            }
        }
        out.push('\n');
    }
    out
}

fn docker_status(c: &DemoContainer, now: u64) -> String {
    let up = docker_duration(now.saturating_sub(c.c.started_at));
    match c.c.state.as_str() {
        "paused" => format!("Up {up} (Paused)"),
        "restarting" => format!("Restarting ({}) {} ago", c.c.exit_code, docker_duration(now.saturating_sub(c.c.finished_at))),
        "running" => match c.c.health.as_deref() {
            Some(h) if c.is_ray() || h != "healthy" => format!("Up {up} ({})", if h == "starting" { "health: starting" } else { h }),
            Some(h) => format!("Up {up} ({h})"),
            None => format!("Up {up}"),
        },
        _ => format!("Exited ({}) {} ago", c.c.exit_code, docker_duration(now.saturating_sub(c.c.finished_at))),
    }
}

/// One line of `docker system df --format '{{json .}}'`.
fn json_line(kind: &str, total: usize, active: usize, size: &str, reclaimable: &str) -> String {
    format!("{{\"Active\":\"{active}\",\"Reclaimable\":\"{reclaimable}\",\"Size\":\"{size}\",\"TotalCount\":\"{total}\",\"Type\":\"{kind}\"}}\n")
}

impl DemoCluster {
    fn node(&self, id: &str) -> Result<&DemoNode, String> {
        self.nodes.iter().find(|n| n.cfg.id == id || n.cfg.name == id).ok_or_else(|| format!("unknown node {id}"))
    }

    fn reachable(&self, id: &str) -> Result<usize, String> {
        let i = self.node_index(id)?;
        if !self.nodes[i].online() {
            return Err(format!("{} isn't connected.", self.nodes[i].cfg.name));
        }
        Ok(i)
    }

    /// Fails the way sudo does while the "sudo-rejected" scenario plays.
    pub fn sudo_check(&self, node: &str) -> Result<(), String> {
        if self.scenario().as_deref() == Some("sudo-rejected") {
            return Err(format!("sudo: incorrect password (on {node}). The saved cluster password was rejected; save the correct one in Settings."));
        }
        Ok(())
    }

    // ---- Services

    pub fn services(&self, node: &str) -> Result<Vec<DemoUnit>, String> {
        let i = self.reachable(node)?;
        Ok(self.nodes[i].units.iter().map(Unit::view).collect())
    }

    pub fn service_action(&mut self, node: &str, unit: &str, action: &str) -> Result<(), String> {
        if !unit_ok(unit) {
            return Err("That service name has characters SparkBox won't pass to a shell.".into());
        }
        let i = self.reachable(node)?;
        self.sudo_check(node)?;
        let now = unix_now();
        let n = &mut self.nodes[i];
        let idx = n.units.iter().position(|u| u.unit == unit).ok_or_else(|| format!("Failed to {action} {unit}: Unit {unit} not found."))?;
        match action {
            "start" => {
                if n.units[idx].active == "active" {
                    return Ok(());
                }
                start_unit(n, idx, now)?;
            }
            "restart" => {
                if n.units[idx].active == "active" {
                    stop_unit(n, idx, now, false);
                }
                start_unit(n, idx, now)?;
            }
            "stop" => {
                if n.units[idx].active != "inactive" {
                    stop_unit(n, idx, now, false);
                }
            }
            "reload" => {
                if n.units[idx].active != "active" {
                    return Err(format!("Job for {unit} failed: {unit} is not active, cannot reload."));
                }
                let u = &mut n.units[idx];
                u.push(now, format!("systemd[1]: Reloading {} - {}...", u.unit, u.description));
                u.push(now, format!("systemd[1]: Reloaded {} - {}.", u.unit, u.description));
            }
            "enable" | "disable" | "mask" | "unmask" => {
                let u = &mut n.units[idx];
                if u.enabled == "static" && action != "mask" {
                    return Err(format!("The unit files have no installation config (WantedBy=, RequiredBy=…). {unit} is static and can't be {action}d."));
                }
                u.enabled = match action {
                    "enable" => "enabled",
                    "mask" => "masked",
                    _ => "disabled",
                }
                .into();
                u.push(now, "systemd[1]: Reloading requested from client PID 48211 ('systemctl') (unit session-41.scope)...".into());
                u.push(now, "systemd[1]: Reloading finished in 312 ms.".into());
            }
            other => return Err(format!("unsupported action {other}")),
        }
        // Docker going away takes every container with it; coming back restarts those that should.
        if unit == "docker.service" {
            if n.units[idx].active != "active" {
                let mut ray_lost = None;
                for c in n.containers.iter_mut().filter(|c| c.running() || c.c.state == "paused") {
                    c.c.state = "exited".into();
                    c.c.exit_code = 0;
                    c.c.finished_at = now;
                    c.frozen = false;
                    if c.is_ray() {
                        ray_lost = Some(c.kind == Kind::RayHead);
                    }
                }
                if let Some(head) = ray_lost {
                    self.ray_cascade(head);
                }
            } else {
                for c in n.containers.iter_mut() {
                    if c.c.state == "exited" && !c.manual_stop && matches!(c.c.restart_policy.as_deref(), Some("always" | "unless-stopped")) {
                        c.c.state = "running".into();
                        c.c.started_at = now;
                    }
                }
            }
        }
        Ok(())
    }

    /// `systemctl status` for one unit.
    pub fn service_status(&self, node: &str, unit: &str) -> Result<String, String> {
        if !unit_ok(unit) {
            return Err("bad service name".into());
        }
        let n = &self.nodes[self.reachable(node)?];
        let Some(u) = n.units.iter().find(|u| u.unit == unit) else {
            return Ok(format!("Unit {unit} could not be found.\n"));
        };
        let now = unix_now();
        let since = if u.since == 0 { n.boot_unix + 6 } else { u.since };
        let dot = match u.active.as_str() {
            "active" => "●",
            "failed" => "×",
            _ => "○",
        };
        let preset = if u.enabled == "disabled" { "disabled" } else { "enabled" };
        // Units the owner installed by hand live in /etc (Ollama's installer writes its own).
        let dir = if u.unit == "ollama.service" { "/etc/systemd/system" } else { "/usr/lib/systemd/system" };
        let state = if u.enabled.is_empty() { "static".to_string() } else { u.enabled.clone() };
        let mut s = format!("{dot} {} - {}\n     Loaded: loaded ({dir}/{}; {state}; preset: {preset})\n", u.unit, u.description, u.unit);
        match u.active.as_str() {
            "active" => {
                s += &format!("     Active: active ({}) since {}; {}\n", u.sub, systemd_time(since), systemd_ago(now.saturating_sub(since)));
                if let Some(p) = n.procs.iter().find(|p| p.unit.as_deref() == Some(unit)) {
                    let mem = p.rss / 1_048_576.0;
                    let cpu = p.cpu_time + now.saturating_sub(n.boot_unix + p.started_s) as f64 * p.base_cpu / 100.0;
                    s += &format!("   Main PID: {} ({})\n", p.pid, p.name);
                    s += &format!("      Tasks: {} (limit: 154185)\n", p.threads);
                    // Ollama's runners took half a gigabyte while it probed the GPU at boot.
                    let peak = if p.name == "ollama" { 554.8 } else { mem * 1.18 };
                    s += &format!("     Memory: {mem:.1}M (peak: {peak:.1}M)\n");
                    s += &format!("        CPU: {}min {:.3}s\n", (cpu / 60.0) as u64, cpu % 60.0);
                    s += &format!("     CGroup: /system.slice/{}\n             └─{} {}\n", u.unit, p.pid, p.cmd);
                }
            }
            "failed" => {
                s += &format!("     Active: failed (Result: exit-code) since {}; {}\n", systemd_time(since), systemd_ago(now.saturating_sub(since)));
                s += "   Duration: 412ms\n";
                if let Some(pid) = u.main_pid {
                    s += &format!("    Process: {pid} ExecStart={} (code=exited, status=1/FAILURE)\n", u.exec);
                    s += &format!("   Main PID: {pid} (code=exited, status=1/FAILURE)\n");
                }
                s += "        CPU: 118ms\n";
            }
            _ => {
                s += &format!("     Active: inactive (dead) since {}; {}\n", systemd_time(since), systemd_ago(now.saturating_sub(since)));
            }
        }
        let tail = self.journal_lines(n, u, 10);
        if !tail.is_empty() {
            s += "\n";
            s += &tail;
        }
        Ok(s)
    }

    fn journal_lines(&self, n: &DemoNode, u: &Unit, lines: usize) -> String {
        let host = hostname(n);
        let mut log = u.log.clone();
        if u.unit == "ollama.service" && u.active == "active" {
            // SparkBox reads a native Ollama through the node: /api/tags and /api/ps from 127.0.0.1.
            let pid = u.main_pid.unwrap_or(0);
            let now = unix_now();
            for k in (1..=6u64).rev() {
                let at = now - now % 60 - k * 60 + 44;
                log.push((at, super::found::gin_line(pid, at, "/api/tags", 88 + k * 7)));
                log.push((at, super::found::gin_line(pid, at, "/api/ps", 16 + k * 3)));
            }
        }
        let since = if log.is_empty() { Vec::new() } else { log[log.len().saturating_sub(lines)..].to_vec() };
        since.iter().map(|(t, line)| format!("{} {host} {line}\n", journal_time(*t))).collect()
    }

    /// `journalctl -u <unit> -n <lines>`.
    pub fn journal(&self, node: &str, unit: &str, lines: usize) -> Result<String, String> {
        let n = &self.nodes[self.reachable(node)?];
        let Some(u) = n.units.iter().find(|u| u.unit == unit) else {
            return Ok("-- No entries --\n".into());
        };
        let body = self.journal_lines(n, u, lines.clamp(1, 1000));
        Ok(if body.is_empty() { "-- No entries --\n".into() } else { body })
    }

    // ---- Processes

    /// `kill -s <sig> <pid>`. TERM, INT and KILL end a process (and a container whose main
    /// process it is); STOP and CONT pause and resume it.
    pub fn signal(&mut self, node: &str, pid: u32, sig: &str) -> Result<(), String> {
        if !matches!(sig, "TERM" | "INT" | "KILL" | "HUP" | "STOP" | "CONT") {
            return Err(format!("unsupported signal {sig}"));
        }
        if pid <= 1 {
            return Err("SparkBox won't signal process 1.".into());
        }
        let i = self.reachable(node)?;
        let now = unix_now();
        let user = self.nodes[i].cfg.user.clone();
        let root = self.nodes[i].procs.iter().find(|p| p.pid == pid).map(|p| p.user != user).unwrap_or(true);
        if root {
            self.sudo_check(node)?;
        }
        let n = &mut self.nodes[i];
        if let Some(c) = n.containers.iter_mut().find(|c| c.pid == pid && (c.running() || c.c.state == "paused")) {
            let ray = c.is_ray().then_some(c.kind == Kind::RayHead);
            match sig {
                "STOP" => c.frozen = true,
                "CONT" => c.frozen = false,
                "HUP" => {}
                _ => {
                    c.c.state = "exited".into();
                    c.c.finished_at = now;
                    c.c.exit_code = match sig {
                        "KILL" => 137,
                        "INT" => 130,
                        _ => 143,
                    };
                    c.frozen = false;
                    c.manual_stop = false;
                    let policy = c.c.restart_policy.clone().unwrap_or_default();
                    // Docker restarts it if its policy says so (not "no").
                    if matches!(policy.as_str(), "always" | "unless-stopped") || (policy == "on-failure" && c.c.exit_code != 0) {
                        c.c.state = "running".into();
                        c.c.started_at = now + 1;
                        c.c.restart_count += 1;
                        c.pid += 311;
                    }
                }
            }
            if matches!(sig, "TERM" | "INT" | "KILL" | "STOP") {
                if let Some(head) = ray {
                    self.ray_cascade(head);
                }
            }
            return Ok(());
        }
        let Some(p) = n.procs.iter().position(|p| p.pid == pid) else {
            return Err(format!("kill: ({pid}) - No such process"));
        };
        match sig {
            "STOP" => n.procs[p].stopped = true,
            "CONT" => n.procs[p].stopped = false,
            "HUP" => {}
            _ => {
                let proc = n.procs.remove(p);
                if proc.pid == n.collector_pid {
                    // SparkBox's own collector: the connection restarts it a moment later.
                    let mut again = proc.clone();
                    n.next_pid += 523;
                    again.pid = n.next_pid;
                    again.started_s = now.saturating_sub(n.boot_unix);
                    n.collector_pid = again.pid;
                    n.procs.push(again);
                } else if let Some(unit) = &proc.unit {
                    if let Some(idx) = n.units.iter().position(|u| &u.unit == unit) {
                        n.procs.push(proc.clone());
                        stop_unit(n, idx, now, sig == "KILL");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn renice(&mut self, node: &str, pid: u32, nice: i32) -> Result<(), String> {
        if !(-20..=19).contains(&nice) {
            return Err("Priority (nice) goes from -20 (highest) to 19 (lowest).".into());
        }
        let i = self.reachable(node)?;
        let user = self.nodes[i].cfg.user.clone();
        let current = self.nodes[i].procs.iter().find(|p| p.pid == pid).map(|p| (p.user == user, p.nice));
        // Raising priority, or touching someone else's process, needs root.
        if current.map(|(own, old)| !own || nice < old).unwrap_or(true) {
            self.sudo_check(node)?;
        }
        let n = &mut self.nodes[i];
        if let Some(c) = n.containers.iter_mut().find(|c| c.pid == pid && c.running()) {
            c.nice = nice;
            return Ok(());
        }
        let p = n.procs.iter_mut().find(|p| p.pid == pid).ok_or_else(|| format!("renice: failed to set priority for {pid} (process ID): No such process"))?;
        p.nice = nice;
        Ok(())
    }

    // ---- Power

    /// reboot | poweroff | wake. The lifecycle plays out over the following ticks.
    pub fn power(&mut self, node: &str, action: &str) -> Result<(), String> {
        let i = self.node_index(node)?;
        let name = self.nodes[i].cfg.name.clone();
        match (action, self.nodes[i].power) {
            ("wake", Power::Off) => {
                self.nodes[i].power = Power::Waking { off: 20, up: 3 };
                return Ok(());
            }
            ("wake", Power::On) => return Err(format!("{name} is already on.")),
            ("wake", _) => return Err(format!("{name} is busy restarting; wait a moment.")),
            ("reboot" | "poweroff", Power::On) => {}
            ("reboot" | "poweroff", Power::Off) => return Err(format!("{name} is powered off. Use Wake to start it.")),
            ("reboot" | "poweroff", _) => return Err(format!("{name} is already restarting or shutting down.")),
            (other, _) => return Err(format!("unsupported power action {other}")),
        }
        if !self.nodes[i].online() {
            return Err(format!("{name} isn't connected."));
        }
        self.sudo_check(node)?;
        let now = unix_now();
        let n = &mut self.nodes[i];
        let ray = n.role != Role::Ollama;
        for c in n.containers.iter_mut().filter(|c| c.running() || c.c.state == "paused" || c.c.state == "restarting") {
            c.c.state = "exited".into();
            c.c.exit_code = 0;
            c.c.finished_at = now;
            c.frozen = false;
            c.exit_at = None;
        }
        n.power = if action == "reboot" { Power::Rebooting { down: 3, off: 20, up: 3 } } else { Power::ShuttingDown { left: 3 } };
        if ray {
            self.ray_cascade(self.nodes[i].role == Role::RayHead);
        }
        Ok(())
    }

    // ---- Updates

    pub fn updates(&self, node: &str) -> Result<DemoUpdates, String> {
        let i = self.reachable(node)?;
        let n = &self.nodes[i];
        Ok(DemoUpdates {
            node: n.cfg.id.clone(),
            packages: n.updates.clone(),
            reboot_required: n.pending_boot.is_some(),
            checked_at: unix_now() - super::found::checked_ago_s(&n.cfg.id),
            unattended: false,
        })
    }

    /// Installs the named upgrades (all of them when `packages` is empty). Held packages refuse,
    /// as apt does; kernel and driver updates wait for a reboot.
    pub fn apply_updates(&mut self, node: &str, packages: &[String]) -> Result<(), String> {
        let i = self.reachable(node)?;
        self.sudo_check(node)?;
        let n = &mut self.nodes[i];
        for name in packages {
            match n.updates.iter().find(|p| &p.name == name) {
                None => return Err(format!("E: Unable to locate package {name} (or it's already up to date)")),
                Some(p) if p.held => {
                    return Err(format!("E: Held packages were changed and -y was used without --allow-change-held-packages. ({name} is held on {})", n.cfg.name))
                }
                _ => {}
            }
        }
        let take: Vec<DemoPackage> = n.updates.iter().filter(|p| !p.held && (packages.is_empty() || packages.contains(&p.name))).cloned().collect();
        n.updates.retain(|p| !take.iter().any(|t| t.name == p.name));
        let driver = take.iter().any(|p| p.name.starts_with("nvidia-driver")).then(|| DRIVER.to_string());
        let kernel = take.iter().any(|p| p.name.starts_with("linux-image-6")).then(|| KERNEL.to_string());
        if driver.is_some() || kernel.is_some() || take.iter().any(|p| p.reboot) {
            let (d0, k0) = n.pending_boot.take().unwrap_or((None, None));
            n.pending_boot = Some((driver.or(d0), kernel.or(k0)));
        }
        Ok(())
    }

    // ---- Containers from Models

    /// Starts a container in the simulator; returns its id. Its memory, GPU memory and
    /// listening ports count on the node from the next snapshot.
    pub fn add_container(&mut self, node: &str, spec: DemoContainerSpec) -> Result<String, String> {
        let i = self.reachable(node)?;
        if spec.name.is_empty() || !spec.name.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) {
            return Err(format!("invalid container name {:?}", spec.name));
        }
        let now = unix_now();
        let n = &mut self.nodes[i];
        if n.containers.iter().any(|c| c.c.name == spec.name) {
            return Err(format!(
                "docker: Error response from daemon: Conflict. The container name \"/{}\" is already in use. You have to remove (or rename) that container to be able to reuse that name.",
                spec.name
            ));
        }
        let pid = n.alloc_pid(&mut self.rng);
        let labels: Vec<(&str, &str)> = spec.labels.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mut c = make_container(
            n,
            pid,
            Kind::Injected,
            &spec.name,
            &spec.image,
            &[],
            &spec.ports,
            &spec.command,
            now,
            spec.restart_policy.as_deref().unwrap_or("no"),
            &labels,
            spec.env.clone(),
        );
        c.gpu_mem = spec.gpu_mem as f64;
        c.rss = if spec.rss > 0 { spec.rss as f64 } else { 0.75 * GIB };
        if spec.gpu_mem > 0 {
            c.idle_cpu = 3.0;
            c.busy_cpu = 110.0;
            c.proc_name = "python3".into();
            c.threads = 160;
        }
        let id = c.c.id.clone();
        n.containers.push(c);
        Ok(id)
    }

    /// Removes every container (on any node) with this label; returns how many went.
    pub fn remove_containers(&mut self, label_key: &str, label_value: &str) -> usize {
        let mut gone = 0;
        for n in &mut self.nodes {
            let before = n.containers.len();
            n.containers.retain(|c| c.c.labels.get(label_key).map(String::as_str) != Some(label_value));
            gone += before - n.containers.len();
        }
        gone
    }

    /// Copies and weight loads Models runs over the fabric (bits per second); zero when done.
    pub fn set_external_fabric_load(&mut self, node: &str, rx_bps: f64, tx_bps: f64) -> Result<(), String> {
        let i = self.node_index(node)?;
        self.nodes[i].ext_rx = rx_bps.max(0.0);
        self.nodes[i].ext_tx = tx_bps.max(0.0);
        Ok(())
    }

    // ---- Fleet Command

    /// What a command prints on a simulated node: (exit code, stdout, stderr), generated from
    /// the node's state. Unknown commands exit 127.
    pub fn fleet_reply(&self, node: &str, cmd: &str) -> (u32, String, String) {
        let n = match self.node(node) {
            Ok(n) => n,
            Err(e) => return (255, String::new(), format!("{e}\n")),
        };
        if !n.online() {
            let ip = n.cfg.ip.clone().unwrap_or_else(|| n.cfg.host.clone());
            return (255, String::new(), format!("ssh: connect to host {ip} port 22: Operation timed out\n"));
        }
        let mut c = cmd.trim();
        for prefix in ["LC_ALL=C ", "sudo ", "sudo -n "] {
            c = c.strip_prefix(prefix).unwrap_or(c).trim_start();
        }
        if cmd.trim_start().starts_with("sudo ") {
            if let Err(e) = self.sudo_check(node) {
                return (1, String::new(), format!("{e}\n"));
            }
        }
        let now = unix_now();
        let ok = |s: String| (0, s, String::new());
        let snap_of = || {
            // A throwaway copy so replies can read the same numbers the snapshot shows.
            let mut rng = fastrand::Rng::with_seed(self.tick);
            super::node_snapshot(n, now, now * 1000, self.ray_serving(), self.ray_load, &mut rng)
        };

        if c.starts_with("nvidia-smi --query-gpu") {
            let s = snap_of();
            return ok(format!(
                "NVIDIA GB10, {}, {:.0} %, {:.0}, {:.2} W\n",
                n.driver, s.gpu.util_pct, s.gpu.temp_c, s.gpu.power_w
            ));
        }
        if c == "nvidia-smi" || c.starts_with("nvidia-smi ") && !c.contains("--query") {
            return ok(self.nvidia_smi(n, &snap_of(), now));
        }
        if c == "uptime" || c.starts_with("uptime ") {
            let up = now.saturating_sub(n.boot_unix);
            let (_, _, _, h, m, s, _) = super::ops::civil(now);
            let days = up / 86_400;
            let hm = format!("{}:{:02}", up % 86_400 / 3_600, up % 3_600 / 60);
            let span = match days {
                0 if up < 3_600 => format!("{} min", up / 60),
                0 => format!(" {hm}"),
                1 => format!("1 day, {hm:>5}"),
                d => format!("{d} days, {hm:>5}"),
            };
            return ok(format!(
                " {h:02}:{m:02}:{s:02} up {span},  1 user,  load average: {:.2}, {:.2}, {:.2}\n",
                n.load[0], n.load[1], n.load[2]
            ));
        }
        if c.starts_with("hostname") || c == "cat /etc/hostname" {
            return ok(format!("{}\n", hostname(n)));
        }
        if c.starts_with("df") {
            let s = snap_of();
            let root = &s.disks[0];
            let line = |mount: &str| {
                let total = root.total as f64;
                let used = root.used as f64;
                format!(
                    "{:<15} {:>4} {:>5} {:>5} {:>3}% {mount}\n",
                    root.device,
                    human(total, ""),
                    human(used, ""),
                    human(total - used, ""),
                    ((used / total) * 100.0).ceil() as u32
                )
            };
            let mut out = "Filesystem      Size  Used Avail Use% Mounted on\n".to_string();
            let args: Vec<&str> = c.split_whitespace().filter(|a| a.starts_with('/')).collect();
            if args.is_empty() {
                out += &line("/");
                let efi = n.real.efi_used as f64;
                out += &format!("{:<15} {:>4} {:>5} {:>5} {:>3}% /boot/efi\n", "/dev/nvme0n1p1", "511M", human(efi, ""), human(535_805_952.0 - efi, ""), 2);
            } else {
                for _ in args {
                    out += &line("/");
                }
            }
            return ok(out);
        }
        if c.starts_with("free") {
            let s = snap_of();
            let m = &s.mem;
            let shared = 118.0 * 1_048_576.0;
            let free = (m.available as f64 - m.cached as f64).max(0.4 * GIB);
            let h = |v: f64| human(v, "i");
            return ok(format!(
                "               total        used        free      shared  buff/cache   available\nMem:     {:>11} {:>11} {:>11} {:>11} {:>11} {:>11}\nSwap:    {:>11} {:>11} {:>11}\n",
                h(m.total as f64),
                h(m.used as f64),
                h(free),
                h(shared),
                h(m.cached as f64),
                h(m.available as f64),
                h(m.swap_total as f64),
                h(m.swap_used as f64),
                h((m.swap_total - m.swap_used) as f64)
            ));
        }
        if c.starts_with("docker ps") {
            let all = c.contains(" -a") || c.contains("--all");
            let list: Vec<&DemoContainer> = n.containers.iter().filter(|c| all || c.c.state != "exited" && c.c.state != "created").collect();
            if c.contains("{{.Names}}") {
                let mut rows = vec![vec!["NAMES".to_string(), "IMAGE".into(), "STATUS".into()]];
                rows.extend(list.iter().map(|x| vec![x.c.name.clone(), x.c.image.clone(), docker_status(x, now)]));
                return ok(table(&rows));
            }
            // Docker's own columns, CREATED and PORTS included (host-network containers have none).
            let mut rows = vec![vec!["CONTAINER ID".to_string(), "IMAGE".into(), "COMMAND".into(), "CREATED".into(), "STATUS".into(), "PORTS".into(), "NAMES".into()]];
            rows.extend(list.iter().map(|x| {
                let cmd: String = x.c.command.chars().take(20).collect();
                let created = format!("{} ago", docker_duration(now.saturating_sub(x.c.started_at)));
                vec![x.c.id.clone(), x.c.image.clone(), format!("\"{cmd}…\""), created, docker_status(x, now), x.c.ports.join(", "), x.c.name.clone()]
            }));
            return ok(table(&rows));
        }
        if c.starts_with("docker image ls") || c.starts_with("docker images") {
            let list = self.images(node).unwrap_or_default();
            if c.contains("{{json .}}") {
                return ok(list.iter().map(|i| format!("{}\n", serde_json::to_string(i).unwrap_or_default())).collect());
            }
            let mut rows = vec![vec!["REPOSITORY".to_string(), "TAG".into(), "IMAGE ID".into(), "CREATED".into(), "SIZE".into()]];
            rows.extend(list.iter().map(|i| vec![i.repository.clone(), i.tag.clone(), i.id.clone(), i.created_at.clone(), i.size.clone()]));
            return ok(table(&rows));
        }
        if c.starts_with("docker system df") {
            let list = self.images(node).unwrap_or_default();
            let total: u64 = list.iter().map(|i| i.bytes).sum();
            let used: u64 = list.iter().filter(|i| n.containers.iter().any(|x| x.c.image == format!("{}:{}", i.repository, i.tag) && x.running())).map(|i| i.bytes).sum();
            let gb = |b: u64| format!("{:.2}GB", b as f64 / 1e9);
            let pct = if total > 0 { (total - used) * 100 / total } else { 0 };
            let rows = [
                json_line("Images", list.len(), list.len(), &gb(total), &format!("{} ({pct}%)", gb(total - used))),
                json_line("Containers", n.containers.len(), n.containers.iter().filter(|c| c.running()).count(), "0B", "0B (0%)"),
                json_line("Local Volumes", 0, 0, "0B", "0B"),
                json_line("Build Cache", 0, 0, "0B", "0B"),
            ];
            return ok(rows.concat());
        }
        if c.starts_with("docker version") {
            return ok(format!(
                "Client: Docker Engine - Community\n Version:           {DOCKER}\n API version:       1.51\n Go version:        go1.24.5\n OS/Arch:           linux/arm64\n\nServer: Docker Engine - Community\n Engine:\n  Version:          {DOCKER}\n  API version:      1.51 (minimum version 1.24)\n  OS/Arch:          linux/arm64\n containerd:\n  Version:          1.7.27\n nvidia:\n  Version:          1.3.0\n"
            ));
        }
        if c.contains("apt list --upgradable") {
            let mut out = "Listing...\n".to_string();
            for p in &n.updates {
                out += &format!("{}/{} {} arm64 [upgradable from: {}]\n", p.name, p.origin.replace("nvidia", "noble").replace("docker", "noble"), p.to, p.from);
            }
            return ok(out);
        }
        if c.starts_with("uname") {
            let out = if c.contains("-a") {
                format!("Linux {} {} #19~24.04.2-Ubuntu SMP PREEMPT_DYNAMIC Sun Sep  6 18:35:11 UTC 2026 aarch64 aarch64 aarch64 GNU/Linux\n", hostname(n), n.kernel)
            } else {
                format!("Linux {} aarch64\n", n.kernel)
            };
            return ok(out);
        }
        if c == "true" || c == ":" {
            return ok(String::new());
        }
        if c == "false" {
            return (1, String::new(), String::new());
        }
        if c == "python3 --version" || c == "python3 -V" {
            return ok(format!("Python {PYTHON}\n"));
        }
        if c == "cat /etc/dgx-release" {
            return ok(format!(
                "DGX_NAME=\"DGX Spark\"\nDGX_PRETTY_NAME=\"NVIDIA DGX Spark\"\nDGX_SWBUILD_DATE=\"2025-09-10-13-50-03\"\nDGX_SWBUILD_VERSION=\"7.2.3\"\nDGX_COMMIT_ID=\"833b4a7\"\nDGX_PLATFORM=\"{}\"\nDGX_SERIAL_NUMBER=\"{}\"\n\nDGX_OTA_VERSION=\"7.5.0\"\nDGX_OTA_DATE=\"Sun Apr 19 10:38:20 EDT 2026\"\n\nDGX_OTA_VERSION=\"7.6.0\"\nDGX_OTA_DATE=\"Sun Oct  4 11:41:51 EDT 2026\"\n",
                n.real.platform, n.real.serial
            ));
        }
        if c.starts_with("nproc") {
            return ok("20\n".into());
        }
        if c.contains("/sys/class/net/enp1s0f") {
            // The built-in "Fabric links" snippet: every CX-7 netdev, in glob order; a speed only
            // for a link that's up (a down port has none to read).
            let up = n.carrier();
            let mut out = String::new();
            for iface in ["enp1s0f0np0", "enp1s0f1np1", "enP2p1s0f0np0", "enP2p1s0f1np1"] {
                let cabled = FABRIC.iter().any(|(f, _, _)| *f == iface);
                if cabled && up {
                    out += &format!("{iface} up {FABRIC_SPEED_MBPS}Mb/s\n");
                } else {
                    out += &format!("{iface} down\n");
                }
            }
            return ok(out);
        }
        if c.starts_with("ip -br") && (c.contains("addr") || c.contains(" a")) {
            let s = snap_of();
            let mut out = format!("{:<16} {:<14} 127.0.0.1/8 ::1/128 \n", "lo", "UNKNOWN");
            for x in &s.nets {
                let state = if x.up { "UP" } else { "DOWN" };
                let addr = match (&x.ip, x.role) {
                    (Some(ip), crate::model::NetRole::Lan) => format!("{ip}/24 fe80::{}/64 ", &x.mac.as_deref().unwrap_or("").replace(':', "")[6..]),
                    (Some(ip), _) => format!("{ip}/{FABRIC_PREFIX} "),
                    (None, _) => String::new(),
                };
                out += &format!("{:<16} {:<14} {addr}\n", x.iface, state);
            }
            let bridge = n.containers.iter().any(|c| c.running() && !c.is_ray() && c.kind != Kind::Injected);
            out += &format!("{:<16} {:<14} 172.17.0.1/16 \n", "docker0", if bridge { "UP" } else { "DOWN" });
            return ok(out);
        }
        if c.starts_with("ip -br link") || c.starts_with("ip -br l") {
            let s = snap_of();
            let mut out = format!("{:<16} {:<14} 00:00:00:00:00:00 <LOOPBACK,UP,LOWER_UP> \n", "lo", "UNKNOWN");
            for x in &s.nets {
                let (state, flags) = if x.up { ("UP", "<BROADCAST,MULTICAST,UP,LOWER_UP>") } else { ("DOWN", "<NO-CARRIER,BROADCAST,MULTICAST,UP>") };
                out += &format!("{:<16} {:<14} {} {flags} \n", x.iface, state, x.mac.as_deref().unwrap_or(""));
            }
            return ok(out);
        }
        if c.starts_with("systemctl --failed") || c.starts_with("systemctl list-units --failed") || c.starts_with("systemctl --state=failed") {
            let failed: Vec<&Unit> = n.units.iter().filter(|u| u.active == "failed").collect();
            if failed.is_empty() {
                return ok("  UNIT LOAD ACTIVE SUB DESCRIPTION\n\n0 loaded units listed.\n".into());
            }
            let w = failed.iter().map(|u| u.unit.len()).max().unwrap_or(4);
            let mut out = format!("  {:<w$} LOAD   ACTIVE SUB    DESCRIPTION\n", "UNIT");
            for u in &failed {
                out += &format!("● {:<w$} loaded failed failed {}\n", u.unit, u.description);
            }
            out += "\nLegend: LOAD   → Reflects whether the unit definition was properly loaded.\n        ACTIVE → The high-level unit activation state, i.e. generalization of SUB.\n        SUB    → The low-level unit activation state, values depend on unit type.\n\n";
            out += &format!("{} loaded unit{} listed.\n", failed.len(), if failed.len() == 1 { "" } else { "s" });
            return ok(out);
        }
        if let Some(rest) = c.strip_prefix("systemctl status") {
            let unit = rest.split_whitespace().filter(|a| !a.starts_with('-')).map(|a| a.trim_matches('\'')).next_back().unwrap_or("");
            let unit = if unit.contains('.') { unit.to_string() } else { format!("{unit}.service") };
            return match self.service_status(node, &unit) {
                Ok(text) if text.starts_with("Unit ") => (4, String::new(), text),
                Ok(text) => {
                    let code = n.units.iter().find(|u| u.unit == unit).map(|u| if u.active == "active" { 0 } else { 3 }).unwrap_or(4);
                    (code, text, String::new())
                }
                Err(e) => (1, String::new(), e),
            };
        }
        if c.starts_with("journalctl") {
            let args: Vec<&str> = c.split_whitespace().collect();
            let mut unit = None;
            let mut lines = 10usize;
            for (i, a) in args.iter().enumerate() {
                match *a {
                    "-u" | "--unit" => unit = args.get(i + 1).map(|u| u.trim_matches('\'').to_string()),
                    "-n" | "--lines" => lines = args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(10),
                    _ => {}
                }
            }
            let Some(unit) = unit else {
                return (1, String::new(), "SparkBox's demo journal answers `journalctl -u <unit> -n <lines>`.\n".into());
            };
            let unit = if unit.contains('.') { unit } else { format!("{unit}.service") };
            return match self.journal(node, &unit, lines) {
                Ok(text) => ok(text),
                Err(e) => (1, String::new(), e),
            };
        }
        if let Some(reply) = super::fabric::reply(self, n.idx, c) {
            return reply;
        }
        (127, String::new(), format!("(demo mode) {c}: no simulated output for this command; switch to Live data to run it\n"))
    }

    fn nvidia_smi(&self, n: &DemoNode, s: &crate::model::NodeSnapshot, now: u64) -> String {
        let (y, m, d, h, mi, sec, wd) = civil(now);
        let mut out = format!("{} {} {d:>2} {h:02}:{mi:02}:{sec:02} {y}       \n", DAYS[wd], MONTHS[m as usize - 1]);
        out += "+-----------------------------------------------------------------------------------------+\n";
        out += &format!("| NVIDIA-SMI {:<22} Driver Version: {:<14} CUDA Version: {:<9}|\n", n.driver, n.driver, CUDA);
        out += "+-----------------------------------------+------------------------+----------------------+\n";
        out += "| GPU  Name                 Persistence-M | Bus-Id          Disp.A | Volatile Uncorr. ECC |\n";
        out += "| Fan  Temp   Perf          Pwr:Usage/Cap |           Memory-Usage | GPU-Util  Compute M. |\n";
        out += "|                                         |                        |               MIG M. |\n";
        out += "|=========================================+========================+======================|\n";
        out += "|   0  NVIDIA GB10                    On  |   0000000F:01:00.0 Off |                  N/A |\n";
        out += &format!(
            "| N/A   {:>2.0}C    {:<4}         {:>3.0}W /  N/A  | Not Supported          | {:>6.0}%      Default |\n",
            s.gpu.temp_c,
            s.gpu.pstate.as_deref().unwrap_or("P8"),
            s.gpu.power_w,
            s.gpu.util_pct
        );
        out += "|                                         |                        |                  N/A |\n";
        out += "+-----------------------------------------+------------------------+----------------------+\n\n";
        out += "+-----------------------------------------------------------------------------------------+\n";
        out += "| Processes:                                                                              |\n";
        out += "|  GPU   GI   CI              PID   Type   Process name                        GPU Memory |\n";
        out += "|        ID   ID                                                               Usage      |\n";
        out += "|=========================================================================================|\n";
        if s.gpu.procs.is_empty() {
            out += "|  No running processes found                                                             |\n";
        }
        for p in &s.gpu.procs {
            out += &format!("|    0   N/A  N/A    {:>12}      C   {:<34} {:>8}MiB |\n", p.pid, p.name, p.mem / 1_048_576);
        }
        out += "+-----------------------------------------------------------------------------------------+\n";
        out
    }
}

/// Seeds each unit's journal and process table from the node's boot, and parks what isn't running.
pub(super) fn settle(n: &mut DemoNode) {
    let boot = n.boot_unix;
    for i in 0..n.units.len() {
        let at = boot + 3 + (i as u64 % 6);
        if n.units[i].active == "active" {
            let _ = start_unit(n, i, at);
        } else {
            let name = n.units[i].unit.clone();
            if let Some(p) = n.procs.iter().position(|p| p.unit.as_deref() == Some(name.as_str())) {
                n.units[i].parked = Some(n.procs.remove(p));
            }
            n.units[i].since = boot + 4;
        }
    }
    // Daemons keep the pid they booted with.
    for u in n.units.iter_mut() {
        if let Some(pid) = n.procs.iter().find(|p| p.unit.as_deref() == Some(u.unit.as_str())).map(|p| p.pid) {
            u.main_pid = Some(pid);
        }
        if u.unit == "ollama.service" && u.active == "active" {
            u.log.extend(super::found::ollama_journal(boot, u.main_pid.unwrap_or(0)));
        }
    }
}

/// For scenarios: fix or break the telemetry service in place.
pub(super) fn set_telemetry_broken(n: &mut DemoNode, broken: bool, now: u64) {
    if let Some(idx) = n.units.iter().position(|u| u.unit == "nvidia-dgx-telemetry.service") {
        n.units[idx].broken = broken;
        let active = n.units[idx].active == "active";
        if broken && active {
            stop_unit(n, idx, now, false);
            let _ = start_unit(n, idx, now);
        } else if !broken && !active {
            let _ = start_unit(n, idx, now);
        }
    }
}
