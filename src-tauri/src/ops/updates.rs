//! Software updates (apt), node by node: what's pending, which of it is security or a
//! sensitive package (kernel, NVIDIA driver, CUDA, Docker, Mellanox/DOCA), whether a reboot
//! is waiting, what's held, and a dry run of the upgrade. Changing anything (refreshing the
//! lists, holding, upgrading) needs Admin mode and runs as a task.
//!
//! An upgrade runs detached on the node, as a transient systemd unit
//! (`systemd-run --unit sparkbox-upgrade-<id> --collect`) writing to
//! /var/log/sparkbox/upgrade-<id>.log, the way Cockpit does it: SparkBox disconnecting,
//! reconnecting or quitting doesn't stop apt halfway. SparkBox follows the log over the
//! node's bulk connection ("Unpacking 3 of 7"), picks it up again after a reconnect or a
//! restart (`systemctl show`), and afterwards runs `dpkg --audit` and checks for a restart.
//! Cancel stops apt (SIGTERM) only while it's still downloading; once dpkg is installing,
//! stopping would leave packages half-configured, so it says so and lets it finish.
//!
//! P1.24: the dry run is the real apt-get with `-s` ("everything" is `dist-upgrade`, a
//! selection `install --only-upgrade`), and the GPU-stack guard (gpu_guard.rs) refuses a plan
//! before anything runs as root. Before apt starts, the same root command saves
//! `dpkg-query -W` to /var/log/sparkbox/dpkg-before-<id>.txt (SparkBox copies it to the
//! evidence folder); after apt, inside the unit, `apt-get check` and `needrestart -b -r l`
//! list what's left (services still on old libraries), so a reattach finds them in the log.
//!
//! Round 4, decision (b): an upgrade installs exactly the plan its dry run showed. The Updates
//! window sends that plan back (`opts.plan`); every installed package in it is pinned as
//! `name=version` (C1′: `--no-remove install --only-upgrade`, new dependencies left to apt), and
//! just before the start the dry run of exactly that command (C2′) must show the same plan, or
//! nothing starts. A plan that removes anything is refused: one-node upgrades only upgrade.
//! Decision (d): the node is in a maintenance window from just before the unit starts, and
//! each time the follower sees the unit active the window runs 30 minutes from then; out of
//! contact it isn't extended, so its alerts page 30 minutes after apt was last seen running.
//!
//! One upgrade at a time on a node, and none while SparkBox restarts or shuts it down: the node
//! is claimed (its run recorded) before the dry run, in one step with those checks. The run is
//! saved before the start goes out, so a start whose reply is lost is looked for on the node
//! (its unit, its log) and followed if it's there, now or after a restart. "Upgrade everything"
//! still runs only with the GPU stack pinned (round 3's amendment, kept): the pins hold what was
//! shown, and the holds keep a new GPU-stack dependency out.
//!
//! A run whose node stays away past the follower's wait (30 minutes), or whose Live data stops,
//! is "lost", never forgotten: its record stays in upgrades.json, and SparkBox follows it again
//! when the node is back (or at the next Live start). A record leaves upgrades.json only once
//! apt's exit is in the log, or the start is proven never to have happened, or, when the unit
//! ended without an exit line (the node restarted under apt), once `dpkg --audit` has run after
//! it (the task fails with what it said).
//!
//! Rollback of one package is by hand, outside every guard here, and it isn't a sure undo:
//! `sudo apt-get install <pkg>=<old version>` (the old version is in the dpkg-before list), only
//! while `apt-cache policy <pkg>` still lists it, reading apt's plan and answering n if it
//! removes anything or moves the GPU stack; then `dpkg --audit`, and a hold if it must stay
//! there. `APT::Keep-Downloaded-Packages` keeps the .debs a run downloads, which are the new
//! versions, and apt installs only what its indexes list: once a pocket drops the old version
//! it answers "E: Version '<old>' for '<pkg>' was not found", even with that .deb in
//! /var/cache/apt/archives (tried on the staging node, 2026-10-31). Of the GPU-stack pin: Release
//! the GPU-stack pin
//! (`sudo apt-mark unhold` of the names the pin's task listed). A "lost" record may be taken out
//! of upgrades.json by hand only once `systemctl status sparkbox-upgrade-<id>` on the node shows
//! the unit has ended and, if its log has no `SPARKBOX-EXIT` line, `sudo dpkg --audit` there
//! lists nothing (else `sudo dpkg --configure -a` first).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{gpu_guard, maintenance_missing, Ops};
use crate::errors::{ErrorKind, SbError};
use crate::live::audit::{self, OpMeta};
use crate::live::exit::as_class;
use crate::live::fence_class::FenceClass;
use crate::live::{sh_quote as q, CancelToken};
use crate::tasks::{TaskHandle, TaskSpec};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPackage {
    pub name: String,
    pub from: String,
    pub to: String,
    /// The archive it comes from: noble-updates, noble-security, nvidia…
    pub origin: String,
    pub security: bool,
    /// Kernel, NVIDIA, CUDA, Docker or Mellanox: upgrade with care.
    pub sensitive: bool,
    pub held: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeUpdates {
    pub node: String,
    pub pending: Vec<PendingPackage>,
    pub reboot_required: bool,
    pub reboot_packages: Vec<String>,
    /// unattended-upgrades on (Some(true)), off, or unknown.
    pub unattended: Option<bool>,
    pub held: Vec<String>,
    pub driver: Option<String>,
    pub kernel: Option<String>,
    /// When the package lists were last refreshed (unix ms).
    pub checked_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulatedUpgrade {
    pub node: String,
    pub installs: Vec<String>,
    pub upgrades: Vec<String>,
    pub removals: Vec<String>,
    pub held_back: Vec<String>,
    pub text: String,
    /// The apt-get the upgrade runs, as root in its unit (`apt_command_opts`); empty when the
    /// plan has nothing to install.
    pub command: String,
    /// What this dry run ran: the same apt-get with `-s`, no env, no sudo.
    pub dry_run: String,
    /// Upgrades apt defers this time (phased updates): they stay pending.
    pub phased: Vec<String>,
    /// GPU-stack packages the plan would upgrade or install.
    pub sensitive: Vec<String>,
    /// Why this plan may not run on one node (P1.25's guard); None when it may.
    pub refused: Option<String>,
    /// The plan an upgrade of it is bound to (decision b): every `Inst` line, in apt's order.
    pub plan: Vec<PlannedPkg>,
    /// What it would remove (`Remv` lines, or apt's "will be REMOVED" list), without `:arch`.
    pub removes: Vec<String>,
}

impl SimulatedUpgrade {
    /// The plan as the Upgrade confirm sends it back (`opts.plan`).
    pub fn shown(&self) -> Vec<PlanEntry> {
        self.plan.iter().map(|p| PlanEntry { name: p.name.clone(), to: p.to.clone() }).collect()
    }
}

/// One package of a dry run's plan (an `Inst` line): the version apt installs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlannedPkg {
    /// Without apt's `:arch` suffix ("libfoo:arm64" is "libfoo").
    pub name: String,
    /// The version installed now; None for a package the plan adds.
    pub from: Option<String>,
    pub to: String,
    /// New dependency: routine updates let apt choose it; reviewed system updates pin it.
    pub new: bool,
}

/// A package of the plan the owner was shown, as Upgrade sends it back (`opts.plan`): SparkBox
/// installs exactly these versions, or nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlanEntry {
    pub name: String,
    pub to: String,
}

/// What an upgrade's command carries beyond its packages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpgradeOpts {
    /// Set only by the dedicated, fully validated system-update path, never by IPC.
    #[serde(skip)]
    pub(crate) protected: bool,
    /// apt's download limit (Acquire::http(s)::Dl-Limit), for the reattach test.
    pub dl_limit_kbps: Option<u32>,
}

/// What the Upgrade confirm asked for (the `updates_upgrade` command's `opts`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpgradeArgs {
    pub dl_limit_kbps: Option<u32>,
    /// The pending security fixes that aren't GPU-stack and aren't held; the packages named are ignored.
    pub security_only: bool,
    /// The plan the dry run showed (`SimulatedUpgrade::shown`). Required with Live data: SparkBox
    /// installs exactly it, or refuses. Demo without one binds to its simulated dry run.
    pub plan: Option<Vec<PlanEntry>>,
}

/// What an upgrade is bound to: what's pending (security only picks from it), the picks, the plan
/// shown, and the names dpkg has installed (the plan's installed packages are what C1′ pins).
type Bound = (Vec<PendingPackage>, Vec<String>, Vec<PlanEntry>, Vec<String>);

/// Upgrade with Live data and no plan to bind to.
pub const DRY_RUN_FIRST: &str = "Run the dry run first: SparkBox installs exactly what it showed.";

/// Upgrade with a plan that leaves out a package it names: the plan is another selection's.
pub const OTHER_SELECTION: &str = "That dry run was for another selection. Run it again.";

/// needrestart's batch listing after an upgrade (`needrestart -b -r l`, as root in the unit).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NeedRestart {
    /// NEEDRESTART-KSTA: 1 the running kernel is the newest; 2 or 3 a newer one is installed
    /// (ABI-compatible, or a new version); 0 needrestart couldn't tell.
    pub kernel_status: Option<u8>,
    pub kernel_current: Option<String>,
    pub kernel_expected: Option<String>,
    /// Services still running old libraries ("nginx.service"), each restarted only by hand.
    pub services: Vec<String>,
}

/// "Pin the GPU stack" (or its release) on one node: the packages it holds (or releases), and
/// its task once it runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldPreset {
    pub node: String,
    pub hold: bool,
    pub packages: Vec<String>,
    pub task_id: Option<String>,
}

/// Kernel, NVIDIA, CUDA, Docker, Mellanox/DOCA, the DGX OS updater: gpu_guard.rs decides.
pub fn sensitive(name: &str) -> bool {
    gpu_guard::sensitive(name)
}

/// `apt list --upgradable`:
/// "libnvidia-compute-580/noble-updates 580.95.05-0ubuntu1 arm64 [upgradable from: 580.82.07-0ubuntu1]"
pub fn parse_upgradable(text: &str) -> Vec<PendingPackage> {
    let mut out: Vec<PendingPackage> = text
        .lines()
        .filter_map(|l| {
            let (head, from) = l.split_once(" [upgradable from: ")?;
            let mut cols = head.split_whitespace();
            let (name, origin) = cols.next()?.split_once('/')?;
            let to = cols.next()?.to_string();
            // Two sources for one archive (Chrome's, on the nodes) list it twice: "stable,stable".
            let mut origins: Vec<&str> = Vec::new();
            for o in origin.split(',').map(str::trim) {
                if !origins.contains(&o) {
                    origins.push(o);
                }
            }
            let origin = origins;
            Some(PendingPackage {
                name: name.into(),
                from: from.trim_end_matches(']').trim().into(),
                to,
                security: origin.iter().any(|o| o.ends_with("-security")),
                origin: origin.join(", "),
                sensitive: sensitive(name),
                held: false,
            })
        })
        .collect();
    sort_packages(&mut out);
    out
}

/// Sensitive packages first, then security, then by name.
fn sort_packages(list: &mut [PendingPackage]) {
    list.sort_by(|a, b| (!a.sensitive, !a.security, &a.name).cmp(&(!b.sensitive, !b.security, &b.name)));
}

/// The one read-only script behind the Updates window, split into sections.
pub const STATUS_SCRIPT: &str = "echo '--- upgradable'; LC_ALL=C apt list --upgradable || exit $?; \
echo '--- held'; apt-mark showhold || exit $?; \
echo '--- reboot'; cat /var/run/reboot-required 2>/dev/null; cat /var/run/reboot-required.pkgs 2>/dev/null; \
echo '--- unattended'; apt-config dump APT::Periodic::Unattended-Upgrade 2>/dev/null; \
echo '--- checked'; stat -c %Y /var/lib/apt/periodic/update-success-stamp /var/lib/apt/lists 2>/dev/null | head -1";

fn sections(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current = String::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("--- ") {
            current = name.trim().to_string();
            out.entry(current.clone()).or_default();
        } else if !current.is_empty() {
            out.entry(current.clone()).or_default().push(line.to_string());
        }
    }
    out
}

pub fn parse_status(node: &str, text: &str) -> NodeUpdates {
    let s = sections(text);
    let get = |k: &str| s.get(k).cloned().unwrap_or_default();
    let held: Vec<String> = get("held").into_iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let mut pending = parse_upgradable(&get("upgradable").join("\n"));
    for p in &mut pending {
        p.held = held.contains(&p.name);
    }
    let reboot = get("reboot");
    let reboot_required = reboot.iter().any(|l| l.contains("System restart required"));
    let reboot_packages = reboot.iter().filter(|l| !l.contains("System restart required") && !l.trim().is_empty()).map(|l| l.trim().to_string()).collect();
    // APT::Periodic::Unattended-Upgrade "1";
    let unattended = get("unattended").iter().find_map(|l| l.split('"').nth(1).map(|v| v != "0"));
    let checked_ms = get("checked").first().and_then(|l| l.trim().parse::<u64>().ok()).map(|s| s * 1000);
    let driver = pending.iter().find(|p| p.name.starts_with("nvidia-driver-") || p.name.starts_with("libnvidia-compute-")).map(|p| format!("{} → {}", short_version(&p.from), short_version(&p.to)));
    let kernel = pending.iter().find(|p| p.name.starts_with("linux-image-")).map(|p| format!("{} → {}", p.from, p.to));
    NodeUpdates { node: node.into(), pending, reboot_required, reboot_packages, unattended, held, driver, kernel, checked_ms }
}

/// "580.95.05-0ubuntu1" → "580.95.05"
fn short_version(v: &str) -> String {
    v.split('-').next().unwrap_or(v).to_string()
}

/// `apt-get -s dist-upgrade`: "Inst pkg [old] (new origin [arch])", "Remv pkg [old]", the list
/// of what it would remove (all a `--no-remove` dry run prints before it stops), and the lists of
/// what's kept back (held) and what phasing defers this time.
pub fn parse_simulation(node: &str, text: &str) -> SimulatedUpgrade {
    let mut sim = SimulatedUpgrade { node: node.into(), text: text.to_string(), ..Default::default() };
    let mut removed: Vec<String> = Vec::new();
    // The list the indented lines that follow belong to.
    let mut list: Option<&mut Vec<String>> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Inst ") {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            let to = rest.split_once(" (").and_then(|(_, r)| r.split_whitespace().next()).unwrap_or("").to_string();
            // "Inst libfoo [1.0-1] (2.0-1 …)" upgrades what's installed; "Inst libnew (2.0 …)" adds it.
            let old = rest.strip_prefix(name.as_str()).and_then(|r| r.strip_prefix(" [")).and_then(|r| r.split(']').next());
            match old {
                Some(old) => sim.upgrades.push(format!("{name} {old} → {to}")),
                None => sim.installs.push(format!("{name} {to}")),
            }
            sim.plan.push(PlannedPkg { name: gpu_guard::bare(&name).to_string(), from: old.map(String::from), to, new: old.is_none() });
            list = None;
        } else if let Some(rest) = line.strip_prefix("Remv ") {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            removed.push(gpu_guard::bare(&name).to_string());
            sim.removals.push(name);
            list = None;
        } else if line.starts_with("The following packages will be REMOVED:") {
            list = Some(&mut removed);
        } else if line.starts_with("The following packages have been kept back:") {
            list = Some(&mut sim.held_back);
        } else if line.starts_with("The following upgrades have been deferred due to phasing:") {
            list = Some(&mut sim.phased);
        } else if let (true, Some(l)) = (line.starts_with("  "), list.as_mut()) {
            l.extend(line.split_whitespace().map(String::from));
        } else {
            list = None;
        }
    }
    // "docker-ce*" is purged; each name once.
    for name in removed.iter().map(|n| gpu_guard::bare(n.trim_end_matches('*'))) {
        if !sim.removes.iter().any(|r| r == name) {
            sim.removes.push(name.to_string());
        }
    }
    sim
}

/// Fills in what a dry run of `packages` (C2) stands for: the command an upgrade bound to its
/// plan runs (C1′, its installed packages pinned), the dry run's own, the GPU-stack packages in
/// it, and the verdict on the selection and on the plan. A plan with nothing to pin has no C1′
/// (`install --only-upgrade` of nothing isn't a command): an empty `command`.
fn judge(mut sim: SimulatedUpgrade, packages: &[String], opts: &UpgradeOpts) -> SimulatedUpgrade {
    let pins = pins_of(&sim.plan);
    sim.command = if pins.is_empty() { String::new() } else { apt_command_opts(&pins, opts) };
    sim.dry_run = simulate_command_opts(packages, opts);
    sim.sensitive = gpu_guard::sensitive_in(&sim);
    sim.refused = gpu_guard::check_selection(packages).and_then(|()| gpu_guard::check_plan(&sim)).and_then(|()| only_upgrades(&sim)).err();
    sim
}

/// What C2′ (the dry run of exactly the pinned command) stands for. The verdict is the caller's:
/// whether the plan still is the one shown comes first.
fn judge_pinned(mut sim: SimulatedUpgrade, pins: &[String], opts: &UpgradeOpts) -> SimulatedUpgrade {
    sim.command = apt_command_opts(pins, opts);
    sim.dry_run = pinned_dry_run_opts(pins, opts);
    sim.sensitive = gpu_guard::sensitive_in(&sim);
    sim
}

/// A plan that removes anything is refused (round 4's named amendment, hence C1′'s `--no-remove`).
fn only_upgrades(sim: &SimulatedUpgrade) -> Result<(), String> {
    if sim.removes.is_empty() {
        Ok(())
    } else {
        Err(format!("This plan removes {}. SparkBox's one-node upgrades only upgrade; run it in a terminal if you mean it.", sim.removes.join(", ")))
    }
}

/// The installed packages of a plan, pinned (`name=version`); new dependencies are left to apt.
fn pins_of(plan: &[PlannedPkg]) -> Vec<String> {
    plan.iter().filter(|p| !p.new).map(|p| format!("{}={}", p.name, p.to)).collect()
}

/// The shown plan's packages that are installed on the node (`installed`: dpkg's names), pinned.
fn pins_in(shown: &[PlanEntry], installed: &[String]) -> Vec<String> {
    shown.iter().filter(|p| installed.iter().any(|i| i == gpu_guard::bare(&p.name))).map(|p| format!("{}={}", gpu_guard::bare(&p.name), p.to)).collect()
}

/// "libfoo=1.2-3" → "libfoo".
fn pin_name(pin: &str) -> &str {
    pin.split_once('=').map(|(n, _)| n).unwrap_or(pin)
}

/// Why C2′'s plan isn't the one the owner was shown (names without `:arch`, order ignored), or
/// None when it is: "apt's plan for cell2 changed since the dry run you saw: libfoo now 1.3 (was
/// 1.2); new: libbar 2.0. Look at the new dry run and start again."
pub fn plan_changed(name: &str, shown: &[PlanEntry], now: &[PlannedPkg]) -> Option<String> {
    let want: BTreeMap<&str, &str> = shown.iter().map(|p| (gpu_guard::bare(&p.name), p.to.as_str())).collect();
    let got: BTreeMap<&str, &str> = now.iter().map(|p| (gpu_guard::bare(&p.name), p.to.as_str())).collect();
    if want == got {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    let moved: Vec<String> = got.iter().filter_map(|(n, v)| want.get(n).filter(|w| *w != v).map(|w| format!("{n} now {v} (was {w})"))).collect();
    if !moved.is_empty() {
        parts.push(moved.join(", "));
    }
    let added: Vec<String> = got.iter().filter(|(n, _)| !want.contains_key(*n)).map(|(n, v)| format!("{n} {v}")).collect();
    if !added.is_empty() {
        parts.push(format!("new: {}", added.join(", ")));
    }
    let gone: Vec<&str> = want.keys().filter(|n| !got.contains_key(*n)).copied().collect();
    if !gone.is_empty() {
        parts.push(format!("no longer in it: {}", gone.join(", ")));
    }
    Some(format!("apt's plan for {name} changed since the dry run you saw: {}. Look at the new dry run and start again.", parts.join("; ")))
}

/// C2′ failed with apt's error (a pinned version it no longer offers): the plan moved.
fn plan_moved(name: &str, error: &str) -> String {
    format!("apt's plan for {name} changed since the dry run you saw (apt-get -s: {}). Look at the new dry run and start again.", error.trim())
}

/// "Upgrade everything" on a node whose GPU stack isn't all held: six names, then how many more
/// (a node has about 190; the task's log and the pin's confirm have them all).
fn needs_pin(free: &[String], name: &str) -> String {
    let names = match free.get(6..) {
        Some(rest) if !rest.is_empty() => format!("{} and {} more", free[..6].join(", "), rest.len()),
        _ => free.join(", "),
    };
    format!("Upgrade everything needs the GPU stack pinned first: {names} {} held on {name}", if free.len() == 1 { "isn't" } else { "aren't" })
}

/// Whether the plan shown has every selected package (names without `:arch`).
fn covers(plan: &[PlanEntry], packages: &[String]) -> bool {
    packages.iter().all(|p| plan.iter().any(|e| gpu_guard::bare(&e.name) == gpu_guard::bare(p)))
}

/// The plan's names and versions as the Upgrade confirm sends them, checked as apt gets them.
fn check_plan_entries(plan: &[PlanEntry]) -> Result<(), SbError> {
    check_names(&plan.iter().map(|p| p.name.clone()).collect::<Vec<_>>())?;
    let ok = |v: &str| !v.is_empty() && v.len() <= 128 && v.starts_with(|c: char| c.is_ascii_alphanumeric()) && v.chars().all(|c| c.is_ascii_alphanumeric() || "+-.:~_".contains(c));
    if plan.iter().all(|p| ok(&p.to)) {
        Ok(())
    } else {
        Err(SbError::new(ErrorKind::Invalid, "That version has characters SparkBox won't pass to apt."))
    }
}

/// Demo's dry run, from the simulator's pending list, in apt's words: what isn't held is upgraded
/// (a kernel brings its modules package), what is held is kept back.
fn demo_plan(node: &str, pending: &[PendingPackage], packages: &[String]) -> SimulatedUpgrade {
    let mut text = String::from("NOTE: This is only a simulation!\n      apt-get needs root privileges for real execution.\nReading package lists... Done\nBuilding dependency tree... Done\n");
    let (mut upgraded, mut added, mut held) = (0, 0, Vec::new());
    for p in pending.iter().filter(|p| packages.is_empty() || packages.contains(&p.name)) {
        if p.held {
            held.push(p.name.clone());
            continue;
        }
        upgraded += 1;
        text += &format!("Inst {} [{}] ({} Ubuntu:24.04/{} [arm64])\n", p.name, p.from, p.to, p.origin);
    }
    if pending.iter().any(|p| p.name.starts_with("linux-image-") && !p.held) {
        let k = pending.iter().find(|p| p.name.starts_with("linux-image-")).map(|p| p.to.clone()).unwrap_or_default();
        added += 1;
        text += &format!("Inst linux-modules-{k} ({k} Ubuntu:24.04/noble-updates [arm64])\n");
    }
    if !held.is_empty() {
        text += &format!("The following packages have been kept back:\n  {}\n", held.join(" "));
    }
    text += &format!("{upgraded} upgraded, {added} newly installed, 0 to remove and {} not upgraded.\n", held.len());
    parse_simulation(node, &text)
}

/// What needrestart found and `apt-get check` said, from the unit's log: the lines between
/// `SPARKBOX-AFTER` and `SPARKBOX-AFTER-END`. (None, None) when the log has no such section
/// (Demo, an upgrade started by round 2).
pub fn post_checks(lines: &[String]) -> (Option<String>, Option<NeedRestart>) {
    let Some(start) = lines.iter().position(|l| l.trim() == "SPARKBOX-AFTER") else { return (None, None) };
    let mut check = Vec::new();
    let mut nr = NeedRestart::default();
    let mut listed = false;
    for l in lines[start + 1..].iter().map(|l| l.trim_end()).take_while(|l| l.trim() != "SPARKBOX-AFTER-END" && !l.starts_with("SPARKBOX-EXIT ")) {
        match l.strip_prefix("NEEDRESTART-").and_then(|r| r.split_once(':')) {
            Some((key, value)) => {
                listed = true;
                let value = value.trim().to_string();
                match key {
                    "KSTA" => nr.kernel_status = value.parse().ok(),
                    "KCUR" => nr.kernel_current = Some(value),
                    "KEXP" => nr.kernel_expected = Some(value),
                    "SVC" if !value.is_empty() => nr.services.push(value),
                    _ => {}
                }
            }
            None if !l.trim().is_empty() => check.push(l.to_string()),
            None => {}
        }
    }
    (Some(check.join("\n")), listed.then_some(nr))
}

/// "1 service still uses old libraries: nginx.service" / "3 services still use …".
fn services_sentence(services: &[String]) -> String {
    match services.len() {
        1 => format!("1 service still uses old libraries: {}", services[0]),
        n => format!("{n} services still use old libraries: {}", services.join(", ")),
    }
}

/// needrestart's kernel line, when a newer kernel than the running one is installed.
fn kernel_sentence(nr: &NeedRestart) -> Option<String> {
    (nr.kernel_status.unwrap_or(0) >= 2).then(|| match &nr.kernel_expected {
        Some(k) => format!("A newer kernel is installed ({k}); it runs after the next restart"),
        None => "A newer kernel is installed; it runs after the next restart".to_string(),
    })
}

/// apt in Demo, layered over the simulator: holds and what Updates installed.
#[derive(Default)]
pub struct DemoOverlay {
    /// Set once the node's holds have been read from the simulator.
    holds: Option<BTreeSet<String>>,
    installed: BTreeSet<String>,
    checked_ms: Option<u64>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Ops {
    pub async fn updates_status(&self, demo: bool, node: &str) -> Result<NodeUpdates, SbError> {
        if demo {
            let d = self.demo.lock().unwrap().updates(node).map_err(|e| SbError::classify(&e).on(node))?;
            let mut overlays = self.demo_updates.lock().unwrap();
            let o = overlays.entry(node.to_string()).or_default();
            let holds = o.holds.get_or_insert_with(|| d.packages.iter().filter(|p| p.held).map(|p| p.name.clone()).collect()).clone();
            let mut pending: Vec<PendingPackage> = d
                .packages
                .iter()
                .filter(|p| !o.installed.contains(&p.name))
                .map(|p| PendingPackage {
                    name: p.name.clone(),
                    from: p.from.clone(),
                    to: p.to.clone(),
                    origin: p.origin.clone(),
                    security: p.security,
                    sensitive: sensitive(&p.name),
                    held: holds.contains(&p.name),
                })
                .collect();
            sort_packages(&mut pending);
            let reboot_packages: Vec<String> = d.packages.iter().filter(|p| p.reboot && o.installed.contains(&p.name)).map(|p| p.name.clone()).collect();
            let view = self.node_view(node);
            let driver = pending.iter().find(|p| p.name.starts_with("nvidia-driver-") || p.name.starts_with("libnvidia-compute-")).map(|p| format!("{} → {}", short_version(&p.from), short_version(&p.to)));
            let kernel = pending.iter().find(|p| p.name.starts_with("linux-image-")).map(|p| format!("{} → {}", p.from, p.to));
            return Ok(NodeUpdates {
                node: node.into(),
                pending,
                reboot_required: d.reboot_required || !reboot_packages.is_empty(),
                reboot_packages,
                unattended: Some(view.map(|v| v.name != "gx10-0000").unwrap_or(true)),
                held: holds.into_iter().collect(),
                driver,
                kernel,
                checked_ms: Some(o.checked_ms.unwrap_or(d.checked_at * 1000)),
            });
        }
        let out = self.live.run(node, STATUS_SCRIPT, None, Duration::from_secs(60)).await.map_err(|e| SbError::classify(&e).on(node))?;
        if out.code != Some(0) || !out.stdout.lines().any(|l| l == "--- upgradable") || !out.stdout.lines().any(|l| l == "--- held") {
            return Err(SbError::failed(format!("Couldn't check the remaining packages on {}. Package status is unknown.", self.name_of(node))).on(node).with_detail(out.stderr));
        }
        Ok(parse_status(node, &out.stdout))
    }

    /// The dry run (`simulate_command`: the upgrade's own apt-get with `-s`), nothing changes;
    /// with the guard's verdict on it.
    pub async fn updates_simulate(&self, demo: bool, node: &str, packages: &[String]) -> Result<SimulatedUpgrade, SbError> {
        self.updates_simulate_opts(demo, node, packages, &UpgradeOpts::default()).await
    }

    /// `updates_simulate` for an upgrade with these options (the download limit is in the dry run too).
    pub async fn updates_simulate_opts(&self, demo: bool, node: &str, packages: &[String], opts: &UpgradeOpts) -> Result<SimulatedUpgrade, SbError> {
        check_names(packages)?;
        if demo {
            let status = self.updates_status(true, node).await?;
            let sim = judge(demo_plan(node, &status.pending, packages), packages, opts);
            self.simulated(node, &format!("Simulate the upgrade on {}", self.name_of(node)), &sim.dry_run);
            return Ok(sim);
        }
        let dry = simulate_command_opts(packages, opts);
        let out = self.live.run(node, &dry, None, Duration::from_secs(120)).await.map_err(|e| SbError::classify(&e).on(node))?;
        if out.code != Some(0) {
            return Err(SbError::failed(format!("apt-get -s failed on {node}: {}", out.stderr.trim())).on(node).with_detail(out.stderr));
        }
        let mut sim = judge(parse_simulation(node, &out.stdout), packages, opts);
        // Everything needs the GPU stack held (round 3's amendment, kept with the pins): a new
        // GPU-stack dependency is left to apt, and only a hold keeps one out. Said before the
        // guard's verdict: holding the stack is what keeps its updates out of the plan.
        if packages.is_empty() {
            let free = self.unpinned(node, &self.read_lines(node, PRESET_READ).await?).await?;
            if !free.is_empty() {
                sim.refused = Some(needs_pin(&free, &self.name_of(node)));
            }
        }
        Ok(sim)
    }

    /// C2′, just before an upgrade starts: the dry run of exactly the pinned command, judged by
    /// the caller against the plan shown. Its `refused` is set only when apt couldn't plan it as
    /// pinned: a removal (`--no-remove` stops apt), or a pinned version apt no longer offers. Demo
    /// plans it on its simulated apt.
    async fn pinned_dry_run(&self, demo: bool, node: &str, pending: &[PendingPackage], pins: &[String], opts: &UpgradeOpts) -> Result<SimulatedUpgrade, SbError> {
        let name = self.name_of(node);
        if demo {
            let names: Vec<String> = pins.iter().map(|p| pin_name(p).to_string()).collect();
            let mut sim = judge_pinned(demo_plan(node, pending, &names), pins, opts);
            self.simulated(node, &format!("Simulate the upgrade on {name}"), &sim.dry_run);
            // A pinned version the simulator doesn't offer: what apt would say.
            if let Some(pin) = pins.iter().find(|p| !pending.iter().any(|x| format!("{}={}", x.name, x.to) == **p)) {
                let (n, v) = pin.split_once('=').unwrap_or((pin, ""));
                sim.refused = Some(plan_moved(&name, &format!("E: Version '{v}' for '{n}' was not found")));
            }
            return Ok(sim);
        }
        let dry = pinned_dry_run_opts(pins, opts);
        let out = self.live.run(node, &dry, None, Duration::from_secs(120)).await.map_err(|e| SbError::classify(&e).on(node))?;
        let mut sim = judge_pinned(parse_simulation(node, &out.stdout), pins, opts);
        if out.code != Some(0) {
            if !sim.removes.is_empty() {
                // The guard's words for a GPU-stack or 580 removal, as the dry run shown had them.
                let listed = SimulatedUpgrade { removals: sim.removes.clone(), ..sim.clone() };
                sim.refused = gpu_guard::check_plan(&listed).and_then(|()| only_upgrades(&sim)).err();
            } else if let Some(e) = out.stderr.lines().rev().find(|l| l.starts_with("E: ")) {
                sim.refused = Some(plan_moved(&name, e));
            } else {
                return Err(SbError::failed(format!("apt-get -s failed on {node}: {}", out.stderr.trim())).on(node).with_detail(out.stderr));
            }
        }
        Ok(sim)
    }

    /// `apt-mark hold` / `unhold` (`hold_command`), as a package-hold change.
    pub async fn updates_hold(&self, demo: bool, node: &str, packages: &[String], hold: bool) -> Result<(), SbError> {
        check_names(packages)?;
        if packages.is_empty() {
            return Err(SbError::invalid("Pick the packages to hold."));
        }
        let verb = if hold { "Hold" } else { "Release" };
        let title = format!("{verb} {} on {}", list_names(packages), self.name_of(node));
        let cmd = hold_command(packages, hold);
        let t = title.clone();
        self.one("updates", title, node, async move {
            if demo {
                self.updates_status(true, node).await?;
                self.demo.lock().unwrap().sudo_check(node).map_err(|e| SbError::classify(&e).on(node))?;
                let mut overlays = self.demo_updates.lock().unwrap();
                let holds = overlays.entry(node.to_string()).or_default().holds.get_or_insert_with(BTreeSet::new);
                for p in packages {
                    if hold {
                        holds.insert(p.clone());
                    } else {
                        holds.remove(p);
                    }
                }
                drop(overlays);
                self.simulated(node, &t, &format!("sudo {cmd}"));
                return Ok(((), format!("{} {}", if hold { "Held" } else { "Released" }, list_names(packages))));
            }
            let (code, _, err) = as_class(FenceClass::AptHold, self.exec(node, &cmd, true, Duration::from_secs(60))).await?;
            if code == Some(0) {
                Ok(((), format!("{} {}", if hold { "Held" } else { "Released" }, list_names(packages))))
            } else {
                Err(SbError::failed(format!("apt-mark failed on {node}: {}", err.trim())).on(node))
            }
        })
        .await
    }

    /// "Pin the GPU stack" (P1.27): holds every installed package of the GPU stack
    /// (`gpu_guard::preset_packages` over `dpkg-query -W`), or releases the ones of them that are
    /// held. With `preview` it only reads which packages that is, for the confirm. As a task whose
    /// log lists every package (what Release puts back), also filed in the evidence folder.
    pub async fn updates_hold_preset(&self, demo: bool, node: &str, hold: bool, preview: bool) -> Result<HoldPreset, SbError> {
        if demo {
            return Err(SbError::new(ErrorKind::DemoOnly, "Available with Live data."));
        }
        let name = self.name_of(node);
        if preview {
            let packages = self.preset_names(node, hold).await?;
            return Ok(HoldPreset { node: node.into(), hold, packages, task_id: None });
        }
        let title = if hold { format!("Pin the GPU stack on {name}") } else { format!("Release the GPU-stack pin on {name}") };
        let task = self.tasks.start(TaskSpec::new("updates", title.clone(), &[node.to_string()]));
        task.set_retry("updates_hold_preset", json!({ "node": node, "hold": hold }));
        let id = task.id().to_string();
        let op = OpMeta { title: title.clone(), source: "ui".into() };
        let result: Result<(Vec<String>, String), SbError> = audit::with_op_id(id.clone(), op, async {
            let head = self.live.audit.begin_op(node, &title, "", false);
            let result = async {
                task.step("Reading the installed packages (dpkg-query -W)");
                let names = self.preset_names(node, hold).await?;
                task.step_ok(Some(format!("{} GPU-stack package{}{}", names.len(), if names.len() == 1 { "" } else { "s" }, if hold { "" } else { " held" })));
                if names.is_empty() {
                    let none = if hold { format!("No GPU-stack packages are installed on {name}") } else { format!("Nothing of the GPU stack is held on {name}") };
                    return Ok((names, none));
                }
                let verb = if hold { "hold" } else { "unhold" };
                task.log(format!("apt-mark {verb} on {name}:\n{}", names.join("\n")));
                let undo = if hold { format!("Undo: Release the GPU-stack pin on {name} (sudo apt-mark unhold of these)") } else { format!("Undo: Pin the GPU stack on {name} again") };
                let filed = format!("# {title} (apt-mark {verb})\n# {undo}\n{}\n", names.join("\n"));
                match crate::evidence::file_text(&self.evidence_logs(), "updates", &format!("{node}-holds-{id}.txt"), &filed) {
                    Ok(path) => task.log(format!("{undo}. The list is also in the evidence folder: {}", path.display())),
                    Err(e) => task.log(format!("{undo}. Couldn't file the list in the evidence folder: {e}")),
                }
                task.step(format!("apt-mark {verb} on {} package{}", names.len(), if names.len() == 1 { "" } else { "s" }));
                let (code, out, err) = as_class(FenceClass::AptHold, self.exec(node, &hold_command(&names, hold), true, Duration::from_secs(60))).await?;
                if code != Some(0) {
                    task.step_failed(err.trim().to_string());
                    return Err(SbError::failed(format!("apt-mark failed on {name}: {}", err.trim())).on(node).with_detail(err));
                }
                task.log(out.trim().to_string());
                task.step_ok(None);
                let n = names.len();
                let done = if hold { format!("Pinned {n} GPU-stack package{} on {name}", if n == 1 { "" } else { "s" }) } else { format!("Released {n} GPU-stack package{} on {name}", if n == 1 { "" } else { "s" }) };
                Ok((names, done))
            }
            .await;
            self.end_head(&head, &result.as_ref().map(|(_, s)| s.clone()).map_err(Clone::clone));
            result
        })
        .await;
        match result {
            Ok((packages, summary)) => {
                task.finish_ok(summary);
                Ok(HoldPreset { node: node.into(), hold, packages, task_id: Some(id) })
            }
            Err(e) => {
                task.finish_err(e.message.clone());
                Err(e)
            }
        }
    }

    /// The preset's names on `node`: the GPU-stack packages installed (to hold), or the held
    /// ones among them (to release). Reads only.
    async fn preset_names(&self, node: &str, hold: bool) -> Result<Vec<String>, SbError> {
        let mut names = gpu_guard::preset_packages(&self.read_lines(node, PRESET_READ).await?);
        if !hold {
            let held = self.read_lines(node, "apt-mark showhold").await?;
            names.retain(|n| held.contains(n));
        }
        check_names(&names)?;
        Ok(names)
    }

    /// The GPU-stack packages of `installed` (`PRESET_READ` on `node`) that aren't held: what "Pin
    /// the GPU stack" would still hold. Reads only.
    async fn unpinned(&self, node: &str, installed: &[String]) -> Result<Vec<String>, SbError> {
        let mut names = gpu_guard::preset_packages(installed);
        let held = self.read_lines(node, "apt-mark showhold").await?;
        names.retain(|n| !held.contains(n));
        Ok(names)
    }

    /// A read's output lines, trimmed, without the empty ones; Err when it fails.
    async fn read_lines(&self, node: &str, cmd: &str) -> Result<Vec<String>, SbError> {
        let out = self.live.run(node, cmd, None, Duration::from_secs(30)).await.map_err(|e| SbError::classify(&e).on(node))?;
        if out.code != Some(0) {
            return Err(SbError::failed(format!("{cmd} failed on {node}: {}", out.stderr.trim())).on(node).with_detail(out.stderr));
        }
        Ok(out.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
    }

    /// Where evidence goes: ~/Library/Logs/SparkBox (next to the known hosts in tests).
    fn evidence_logs(&self) -> PathBuf {
        if cfg!(test) {
            self.live.data_path("logs")
        } else {
            crate::live::sshconfig::home().join("Library/Logs/SparkBox")
        }
    }

    /// Reads the package list the upgrade's root command saved before apt started
    /// (`dpkg-before-<id>.txt`, readable by the cluster user) and files a copy in the evidence
    /// folder; the run records both paths. What fails is said in the task and nothing stops.
    async fn keep_dpkg_before(&self, task: &TaskHandle, node: &str) {
        let Some(run) = self.runs.lock().unwrap().get(node).cloned() else { return };
        let path = run.dpkg_before.clone().unwrap_or_else(|| dpkg_before_path(run.log_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("/var/log/sparkbox"), &run.unit));
        let id = run.unit.strip_prefix("sparkbox-upgrade-").unwrap_or(&run.unit).to_string();
        let read = self.live.run(node, &format!("cat {}", q(&path)), None, Duration::from_secs(30)).await;
        let text = match read {
            Ok(o) if o.code == Some(0) => o.stdout,
            Ok(o) => return task.log(format!("Couldn't read the package list from before the upgrade ({path}): {}", o.stderr.trim())),
            Err(e) => return task.log(format!("Couldn't read the package list from before the upgrade ({path}): {e}")),
        };
        match crate::evidence::file_text(&self.evidence_logs(), "updates", &format!("{node}-dpkg-before-{id}.txt"), &text) {
            Ok(mac) => {
                let mac = mac.display().to_string();
                self.update_run(node, |r| (r.dpkg_before, r.evidence) = (Some(path.clone()), Some(mac.clone())));
                save_running(self);
                task.log(format!("Package list before the upgrade: {path} (also in the evidence folder: {mac})"));
            }
            Err(e) => {
                self.update_run(node, |r| r.dpkg_before = Some(path.clone()));
                task.log(format!("Package list before the upgrade: {path} (couldn't copy it to the evidence folder: {e})"));
            }
        }
    }

    /// `apt-get update` on each node, as one task.
    pub fn updates_refresh(self: &Arc<Self>, demo: bool, nodes: Vec<String>) -> Result<String, SbError> {
        if nodes.is_empty() {
            return Err(SbError::invalid("Pick at least one node."));
        }
        let title = format!("Check for updates on {} node{}", nodes.len(), if nodes.len() == 1 { "" } else { "s" });
        let task = self.tasks.start(TaskSpec::new("updates", title.clone(), &nodes));
        task.set_retry("updates_refresh", json!({ "nodes": nodes }));
        let id = task.id().to_string();
        let me = self.clone();
        tauri::async_runtime::spawn(audit::with_op_id(id.clone(), OpMeta { title, source: "ui".into() }, async move {
            let mut failed = Vec::new();
            for (i, node) in nodes.iter().enumerate() {
                let name = me.name_of(node);
                task.step(format!("apt-get update on {name}"));
                let result = if demo {
                    let r = me.demo.lock().unwrap().sudo_check(node).map_err(|e| SbError::classify(&e).on(node));
                    if r.is_ok() {
                        tokio::time::sleep(Duration::from_millis(700 + fastrand::u64(0..900))).await;
                        me.simulated(node, &format!("Check for updates on {name}"), "sudo apt-get update");
                        me.demo_updates.lock().unwrap().entry(node.clone()).or_default().checked_ms = Some(now_ms());
                        task.log(format!("{name}: Hit:1 http://ports.ubuntu.com/ubuntu-ports noble InRelease\n{name}: Reading package lists... Done"));
                    }
                    r
                } else {
                    match me.exec(node, "apt-get update", true, Duration::from_secs(300)).await {
                        Ok((Some(0), out, _)) => {
                            task.log(out.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().map(|l| format!("{name}: {l}")).collect::<Vec<_>>().join("\n"));
                            Ok(())
                        }
                        Ok((_, _, err)) => Err(SbError::failed(format!("apt-get update failed on {name}: {}", err.trim()))),
                        Err(e) => Err(e),
                    }
                };
                match result {
                    Ok(()) => task.step_ok(None),
                    Err(e) => {
                        task.step_failed(e.message.clone());
                        failed.push(name);
                    }
                }
                task.progress((i + 1) as f32 / nodes.len() as f32, None);
            }
            if failed.is_empty() {
                task.finish_ok("Package lists are up to date");
            } else {
                task.finish_err(format!("Couldn't check {}", failed.join(", ")));
            }
        }));
        Ok(id)
    }

    /// `updates_upgrade_opts` with no options (tests). With Live data it's refused: there is no plan.
    #[cfg(test)]
    pub fn updates_upgrade(self: &Arc<Self>, demo: bool, node: String, packages: Vec<String>) -> Result<String, SbError> {
        self.updates_upgrade_opts(demo, node, packages, UpgradeArgs::default())
    }

    /// Upgrades a node (all pending packages, or the named ones, or its security fixes) as a
    /// task, bound to the plan its dry run showed (`args.plan`, decision b): the installed
    /// packages in it pinned at the versions shown, and refused before anything runs as root
    /// when C2′ (the dry run of exactly that) shows another plan, a removal, or the GPU stack.
    /// Held packages stay as they are; everything only with the GPU stack pinned. Refused at once
    /// with no plan (Live), with a plan that leaves out a selected package, while another upgrade
    /// runs on the node or SparkBox restarts or shuts it down. Returns the task's id at once.
    pub fn updates_upgrade_opts(self: &Arc<Self>, demo: bool, node: String, packages: Vec<String>, args: UpgradeArgs) -> Result<String, SbError> {
        self.updates_upgrade_impl(demo, node, packages, args, false)
    }

    pub(super) fn updates_upgrade_impl(self: &Arc<Self>, demo: bool, node: String, packages: Vec<String>, args: UpgradeArgs, protected: bool) -> Result<String, SbError> {
        check_names(&packages)?;
        let UpgradeArgs { dl_limit_kbps, security_only, plan } = args;
        if dl_limit_kbps == Some(0) {
            return Err(SbError::invalid("A download limit of 0 KB/s never finishes; leave the limit off instead."));
        }
        if !demo && plan.is_none() {
            return Err(SbError::invalid(DRY_RUN_FIRST).on(&node));
        }
        if let Some(plan) = &plan {
            check_plan_entries(plan)?;
        }
        let packages = if security_only { Vec::new() } else { packages };
        if !protected {
            super::protected_updates::check_routine(&self.live, demo, &node)?;
            gpu_guard::check_selection(&packages).map_err(|why| SbError::invalid(why).on(&node))?;
        }
        if plan.as_ref().is_some_and(|plan| !covers(plan, &packages)) {
            return Err(SbError::invalid(OTHER_SELECTION).on(&node));
        }
        let name = self.name_of(&node);
        let title = if protected { format!("Install reviewed system updates on {name}") } else { match (security_only, packages.as_slice()) {
            (true, _) => format!("Upgrade security fixes on {name}"),
            (false, []) => format!("Upgrade {name}"),
            (false, named) => format!("Upgrade {} on {name}", list_names(named)),
        } };
        let previous = self.claim_upgrade(demo, &node, &name, &title)?;
        let task = self.tasks.start(TaskSpec::new("updates", title.clone(), std::slice::from_ref(&node)).cancellable());
        if !protected {
            task.set_retry("updates_upgrade", json!({ "node": node, "packages": packages, "opts": { "dlLimitKbps": dl_limit_kbps, "securityOnly": security_only } }));
        }
        let id = task.id().to_string();
        self.update_run(&node, |r| r.task_id = id.clone());
        let me = self.clone();
        let opts = UpgradeOpts { dl_limit_kbps, protected };
        tauri::async_runtime::spawn(audit::with_op_id(id.clone(), OpMeta { title: title.clone(), source: "ui".into() }, async move {
            // What's pending now (security only picks from it; Demo's apt is it), the plan shown,
            // and what's installed: the plan's installed packages are what C1′ pins.
            let inputs: Result<Bound, SbError> = async {
                let pending = if demo || security_only { me.updates_status(demo, &node).await?.pending } else { Vec::new() };
                let picks = if security_only { gpu_guard::security_only(&pending) } else { packages.clone() };
                let shown = match plan {
                    Some(plan) => plan,
                    None => demo_plan(&node, &pending, &picks).shown(),
                };
                let installed: Vec<String> = if demo { pending.iter().map(|p| p.name.clone()).collect() } else { me.read_lines(&node, PRESET_READ).await? };
                Ok((pending, picks, shown, installed))
            }
            .await;
            let pins: Vec<String> = inputs.as_ref().map(|(_, _, shown, installed)| if protected { shown.iter().map(|p| format!("{}={}", p.name, p.to)).collect() } else { pins_in(shown, installed) }).unwrap_or_default();
            // The head line has the apt-get that actually runs (C1′, detached, below), not a stand-in.
            let head = me.live.audit.begin_op(&node, &title, &inputs.as_ref().map(|_| apt_command_opts(&pins, &opts)).unwrap_or_default(), demo);
            // Open while the node upgrades: its alerts are planned and page nobody.
            let windowed = AtomicBool::new(false);
            // Set once apt's unit has (or may have) started: from then on this run is the node's record.
            let started = AtomicBool::new(false);
            // SparkBox lost the node (or Live stopped) while apt may still run there.
            let lost = AtomicBool::new(false);
            let result: Result<String, SbError> = async {
                let (pending, picks, shown, installed) = inputs?;
                if security_only && picks.is_empty() {
                    return Ok(format!("No security fixes SparkBox may install on {name} (it can't upgrade GPU-stack ones yet)"));
                }
                if shown.is_empty() {
                    return Ok(format!("{name} is already up to date"));
                }
                task.step("Checking what will change");
                task.log(format!("The plan you were shown: {}", shown.iter().map(|p| format!("{} {}", p.name, p.to)).collect::<Vec<_>>().join(", ")));
                let review = if protected { Some(me.system_prepare(demo, &node, task.id()).await?) } else { None };
                let sim = if protected && demo { SimulatedUpgrade { node: node.clone(), plan: review.expect("protected review").packages(), ..Default::default() } } else { me.pinned_dry_run(demo, &node, &pending, &pins, &opts).await? };
                task.log(format!("Dry run of exactly that: {}", sim.dry_run));
                task.log(format!("{} to upgrade, {} new, {} to remove, {} held back", sim.upgrades.len(), sim.installs.len(), sim.removes.len(), sim.held_back.len()));
                if !sim.phased.is_empty() {
                    task.log(format!("Deferred by apt's phasing, so not installed this time: {}", sim.phased.join(", ")));
                }
                // apt couldn't plan it as pinned; it isn't the plan shown any more; everything with
                // the GPU stack not held; the guard.
                let mut why = sim.refused.clone().or_else(|| plan_changed(&name, &shown, &sim.plan));
                if why.is_none() && !demo && packages.is_empty() && !security_only {
                    let free = me.unpinned(&node, &installed).await?;
                    if !free.is_empty() {
                        task.log(format!("Not held on {name}:\n{}", free.join("\n")));
                        why = Some(needs_pin(&free, &name));
                    }
                }
                let why = why.or_else(|| if protected { review.expect("protected review").check_plan(&sim).and_then(|()| if demo { Ok(()) } else { review.expect("protected review").check_origin(&sim) }).err() } else { gpu_guard::check_plan(&sim).and_then(|()| only_upgrades(&sim)).err() });
                if let Some(why) = why {
                    task.step_failed(why.clone());
                    task.log("Refused before anything ran as root; nothing changed on the node.".to_string());
                    return Err(SbError::invalid(why).on(&node));
                }
                task.step_ok(None);
                if sim.plan.is_empty() {
                    return Ok(format!("{name} is already up to date"));
                }
                if task.is_cancelled() {
                    return Ok(format!("Stopped before anything was installed on {name}"));
                }
                let n = sim.plan.len();
                task.step(format!("Installing {n} package{}", if n == 1 { "" } else { "s" }));
                if demo {
                    me.demo.lock().unwrap().sudo_check(&node).map_err(|e| SbError::classify(&e).on(&node))?;
                    let take: Vec<String> = pins.iter().map(|p| pin_name(p).to_string()).collect();
                    let simulator_held: BTreeSet<String> =
                        me.demo.lock().unwrap().updates(&node).map(|d| d.packages.iter().filter(|p| p.held).map(|p| p.name.clone()).collect()).unwrap_or_default();
                    let real: Vec<String> = take.iter().filter(|p| !simulator_held.contains(*p)).cloned().collect();
                    started.store(true, Ordering::SeqCst);
                    if me.demo_upgrade(&task, &node, &title, &pending, &take).await? {
                        return Ok(format!("Stopped before anything was installed on {name}"));
                    }
                    if !real.is_empty() {
                        me.demo.lock().unwrap().apply_updates(&node, &real).map_err(|e| SbError::classify(&e).on(&node))?;
                    }
                    me.demo_updates.lock().unwrap().entry(node.clone()).or_default().installed.extend(take.iter().cloned());
                    me.simulated(&node, &title, &format!("sudo {}", apt_command_opts(&pins, &opts)));
                } else {
                    // What the bound plan installs, for "n of m".
                    let names: Vec<String> = sim.plan.iter().map(|p| p.name.clone()).collect();
                    let id = unit_id(task.id());
                    let log_dir = me.live.upgrade_log_dir();
                    let unit = format!("sparkbox-upgrade-{id}");
                    let run = UpgradeRun {
                        node: node.clone(),
                        task_id: task.id().to_string(),
                        title: title.clone(),
                        dpkg_before: Some(dpkg_before_path(&log_dir, &unit)),
                        unit,
                        log_path: format!("{log_dir}/upgrade-{id}.log"),
                        total: names.len(),
                        packages: names,
                        phase: "starting".into(),
                        cancellable: true,
                        started_ms: now_ms(),
                        dl_limit_kbps: opts.dl_limit_kbps,
                        ..Default::default()
                    };
                    // Recorded (and saved) before the start goes out: a start whose reply is lost
                    // is looked for on the node, now or by the next SparkBox.
                    me.set_run(&run);
                    save_running(&me);
                    me.begin_window(&task, &node).await;
                    windowed.store(true, Ordering::SeqCst);
                    let start = detached_command_opts(&run.unit, &log_dir, &run.log_path, &pins, &opts);
                    match as_class(FenceClass::AptOne, me.exec(&node, &start, true, Duration::from_secs(60))).await {
                        Ok((Some(0), _, _)) => started.store(true, Ordering::SeqCst),
                        // The node said no: the && chain stops before systemd-run, or systemd-run failed.
                        Ok((Some(_), _, err)) => {
                            return Err(SbError::failed(format!("Couldn't start the upgrade on {name}: {}", err.lines().last().unwrap_or("").trim())).on(&node).with_detail(err));
                        }
                        // Refused before it was sent: nothing reached the node.
                        Err(e) if not_sent(&e) => return Err(e),
                        // Sent, but its reply was lost (the connection dropped, an sshd child died):
                        // apt may be running there. Look before saying anything.
                        reply => {
                            let why = match &reply {
                                Ok(_) => "no exit status".to_string(),
                                Err(e) => e.message.clone(),
                            };
                            task.log(format!("The start command's reply was lost ({why}); checking whether the upgrade started on {name}…"));
                            started.store(true, Ordering::SeqCst);
                            match me.upgrade_started(&node, &run).await {
                                Some(true) => task.log(format!("It did start on {name}.")),
                                Some(false) => {
                                    started.store(false, Ordering::SeqCst);
                                    return Err(SbError::failed(format!("Couldn't start the upgrade on {name}: the start command's reply was lost, and no upgrade is running there.")).on(&node));
                                }
                                // Gone before it could tell: the record stays, as "lost".
                                None => {
                                    lost.store(true, Ordering::SeqCst);
                                    return Ok(String::new());
                                }
                            }
                        }
                    }
                    me.update_run(&node, |r| {
                        if r.phase == "starting" {
                            r.phase = "preparing".into();
                        }
                    });
                    save_running(&me);
                    // apt runs on the node by itself from here: look-only can't stop it, and says so.
                    task.on_node();
                    task.log(format!("Running detached as {} (log: {})", run.unit, run.log_path));
                    me.keep_dpkg_before(&task, &node).await;
                    if me.follow_upgrade(&task, &node).await? == Followed::Lost {
                        lost.store(true, Ordering::SeqCst);
                        return Ok(String::new());
                    }
                }
                me.after_upgrade(&task, &node, &name, demo, format!("{n} package{} installed on {name}", if n == 1 { "" } else { "s" })).await
            }
            .await;
            if windowed.load(Ordering::SeqCst) {
                if let Some(p) = me.planned() {
                    p.end(&node);
                }
            }
            let lost = lost.load(Ordering::SeqCst);
            if protected && !started.load(Ordering::SeqCst) { me.system_not_started(demo, &node, task.id(), "No installer was sent. Review Tasks, then preview again."); }
            let stopped = matches!(&result, Ok(s) if s.starts_with("Stopped before"));
            if started.load(Ordering::SeqCst) {
                // Only this task's own run.
                me.update_run(&node, |r| {
                    if r.task_id != task.id() {
                        return;
                    }
                    if lost {
                        r.phase = "lost".into();
                    } else if result.is_err() {
                        // After a Stop that worked too: apt's end never reached the log.
                        r.phase = "failed".into();
                    } else if r.phase != "cancelled" {
                        r.phase = "done".into();
                    }
                    r.cancellable = false;
                });
            } else {
                // Nothing started (refused, up to date, stopped first): the node's last run is
                // what it was.
                let mut runs = me.runs.lock().unwrap();
                if runs.get(&node).is_some_and(|r| r.task_id == task.id()) {
                    match previous {
                        Some(before) => runs.insert(node.clone(), before),
                        None => runs.remove(&node),
                    };
                }
            }
            save_running(&me);
            if !demo {
                me.following.lock().unwrap().remove(&node);
            }
            match result {
                Ok(_) if lost => {
                    let log = me.runs.lock().unwrap().get(&node).filter(|r| r.task_id == task.id()).map(|r| r.log_path.clone()).unwrap_or_default();
                    let (summary, warning) = lost_words(&name, &log);
                    me.live.audit.end(&head, audit::Ending { stdout_tail: Some(summary.clone()), error: Some(warning.clone()), ..Default::default() });
                    task.finish_warn(summary, warning);
                    // Followed again once the node is back.
                    me.look_again(&node);
                }
                Ok(summary) if stopped => {
                    me.live.audit.end_op(&head, true, Some(summary.clone()), None);
                    task.finish_cancelled(summary);
                }
                Ok(summary) => {
                    me.live.audit.end_op(&head, true, Some(summary.clone()), None);
                    task.finish_ok(summary);
                }
                Err(e) => {
                    me.live.audit.end_op(&head, false, None, Some(e.message.clone()));
                    // Once apt ran an error is a failed upgrade, whatever Cancel asked for: Stop
                    // came too late or didn't work, so dpkg may have run. Only a run that never
                    // started can read Cancelled.
                    if started.load(Ordering::SeqCst) {
                        task.finish_failed(e.message);
                    } else {
                        task.finish_err(e.message);
                    }
                }
            }
        }));
        Ok(id)
    }

    /// Claims `node` for an upgrade, in one step with the checks: refused while SparkBox
    /// restarts or shuts it down (its declared power state) or while another upgrade runs there
    /// (or was lost there). The claim is a run of its own ("checking": the dry run is under way,
    /// nothing started) in the node's place; returns the run it replaced, to put back if nothing
    /// starts. With Live data this SparkBox then follows the node's upgrade (`following`).
    fn claim_upgrade(&self, demo: bool, node: &str, name: &str, title: &str) -> Result<Option<UpgradeRun>, SbError> {
        // The declared states first, then the runs, then the followers: one order everywhere.
        let declared = self.declared.lock().unwrap();
        if let Some(d) = declared.get(node).filter(|d| d.demo == demo && d.state != "off") {
            return Err(SbError::invalid(power_busy(name, d.state)).on(node));
        }
        let mut runs = self.runs.lock().unwrap();
        match runs.get(node).map(|r| r.phase.as_str()) {
            Some("lost") => return Err(SbError::invalid(format!("SparkBox lost {name} while it was upgrading; apt may still be running there. It looks again when {name} is back.")).on(node)),
            Some(phase) if !is_final(phase) => return Err(SbError::invalid(format!("An upgrade is already running on {name}.")).on(node)),
            _ => {}
        }
        if !demo && !self.following.lock().unwrap().insert(node.to_string()) {
            return Err(SbError::invalid(format!("An upgrade is already running on {name}.")).on(node));
        }
        let claim = UpgradeRun { node: node.into(), title: title.into(), phase: "checking".into(), cancellable: true, started_ms: now_ms(), simulated: demo, ..Default::default() };
        Ok(runs.insert(node.to_string(), claim))
    }

    /// The end of an upgrade, first run or picked up again: whether it was stopped, `dpkg
    /// --audit` (as the cluster user: exit 2 with nothing listed is no all clear), what the unit
    /// checked after apt (`apt-get check`'s E: lines fail it; needrestart's list), and whether
    /// the node needs a restart. Ok with the summary, which starts with `done`.
    async fn after_upgrade(&self, task: &TaskHandle, node: &str, name: &str, demo: bool, done: String) -> Result<String, SbError> {
        if !demo && self.runs.lock().unwrap().get(node).is_some_and(|r| r.task_id == task.id() && r.phase == "cancelled") {
            return Ok(format!("Stopped before anything was installed on {name}"));
        }
        task.step("Checking the package database (dpkg --audit)");
        let (code, audit, err) = if demo { (Some(0), String::new(), String::new()) } else { self.exec(node, "dpkg --audit", false, Duration::from_secs(60)).await? };
        if !audit.trim().is_empty() {
            task.log(audit.clone());
            task.step_failed("dpkg reports packages that aren't fully installed");
            return Err(SbError::failed(format!("The upgrade finished, but dpkg reports packages that aren't fully installed on {name}. Run sudo dpkg --configure -a there.")).on(node).with_detail(audit));
        }
        if code != Some(0) {
            // As the cluster user, dpkg 1.22 lists nothing and exits 2 when a package isn't
            // fully installed (staging finding 4): that is no "all clear".
            let why = err.lines().last().unwrap_or("").trim().to_string();
            task.log(err.clone());
            task.step_failed(format!("Couldn't check: {why}"));
            return Err(SbError::failed(format!("The upgrade finished, but SparkBox couldn't check the package database on {name}. Run sudo dpkg --audit there.")).on(node).with_detail(err));
        }
        task.step_ok(Some("No broken or half-installed packages".into()));
        // What the unit checked after apt (Live: in its log; Demo has no unit).
        let checked = if demo { None } else { self.runs.lock().unwrap().get(node).filter(|r| r.task_id == task.id()).cloned() };
        let needrestart = match &checked {
            Some(run) => after_checks(task, name, run)?,
            None => None,
        };
        task.step("Checking remaining packages and restart status");
        let after = self.updates_status(demo, node).await?;
        let verification = UpgradeVerification { checked_ms: now_ms(), remaining: after.pending.clone() };
        let package_result = verification.summary();
        self.update_run(node, |r| {
            if r.task_id == task.id() {
                r.verification = Some(verification);
            }
        });
        task.log(package_result.clone());
        if !after.pending.is_empty() {
            task.log(format!("Still pending: {}", after.pending.iter().map(|p| format!("{}{}", p.name, if p.held { " (held)" } else if p.sensitive { " (GPU stack)" } else { "" })).collect::<Vec<_>>().join(", ")));
        }
        if !demo { self.system_boot_ready(node).await.map_err(SbError::failed)?; }
        task.step_ok(Some(package_result.clone()));
        let mut said = vec![if after.reboot_required {
            format!("{done}. Restart it to finish ({})", if after.reboot_packages.is_empty() { "kernel or driver".into() } else { after.reboot_packages.join(", ") })
        } else {
            done
        }];
        said.push(package_result);
        if let Some(nr) = &needrestart {
            if !nr.services.is_empty() {
                said.push(services_sentence(&nr.services));
            }
            said.extend(kernel_sentence(nr));
        }
        Ok(said.join(". "))
    }

    /// Whether a detached upgrade whose start went unanswered is on the node: its unit is there,
    /// or its log is (a quick apt may be done and its unit collected already). Waits for the node
    /// first (`wait_back`), and looks three times, a moment apart: the start may still be on its
    /// way. None when the node is gone before it can tell (the run is then "lost"). Reads only.
    async fn upgrade_started(&self, node: &str, run: &UpgradeRun) -> Option<bool> {
        if !self.wait_back(node).await {
            return None;
        }
        for i in 0..3 {
            if i > 0 {
                tokio::time::sleep((self.poll * 2).clamp(Duration::from_millis(500), Duration::from_secs(2))).await;
            }
            let unit = self.live.probe(node, &format!("systemctl show -p ActiveState,Result,LoadState -- {}", q(&run.unit)), Duration::from_secs(10)).await.ok()?;
            if unit.stdout.lines().any(|l| matches!(l, "LoadState=loaded" | "ActiveState=active" | "ActiveState=activating")) {
                return Some(true);
            }
            let log = self.live.probe(node, &format!("tail -n 20 {}", q(&run.log_path)), Duration::from_secs(10)).await.ok()?;
            if log.code == Some(0) {
                return Some(true);
            }
        }
        Some(false)
    }

    /// Live has `node` in its loop (connected or not). Live stopping drops every node from it.
    fn live_follows(&self, node: &str) -> bool {
        self.live.all_channel_stats().iter().any(|(id, _)| id == node)
    }

    /// How often a follower looks whether its node is back (5 s; tests are quicker).
    fn reconnect_poll(&self) -> Duration {
        (self.poll * 50).clamp(Duration::from_millis(100), Duration::from_secs(5))
    }

    /// Waits for `node`'s connection while an upgrade's follower may (`upgrade_wait`, 30
    /// minutes): false when it isn't back by then, or once Live stops.
    async fn wait_back(&self, node: &str) -> bool {
        let until = Instant::now() + *self.upgrade_wait.lock().unwrap();
        loop {
            if self.live.is_connected(node) {
                return true;
            }
            if !self.live_follows(node) || Instant::now() >= until {
                return false;
            }
            tokio::time::sleep(self.reconnect_poll().min(until.saturating_duration_since(Instant::now()))).await;
        }
    }

    /// Waits for `node`'s connection with no limit (a lost upgrade is picked up whenever its node
    /// is back): true once it's connected, false once Live stops after following it. Before Live
    /// starts (SparkBox opening in Demo, or still starting) it waits for that too.
    async fn wait_for_node(&self, node: &str) -> bool {
        let mut followed = false;
        loop {
            if self.live.is_connected(node) {
                return true;
            }
            let follows = self.live_follows(node);
            if followed && !follows {
                return false;
            }
            followed |= follows;
            tokio::time::sleep(self.reconnect_poll()).await;
        }
    }

    /// Opens the node's maintenance window for an upgrade (`upgrade_window` from now; the follower
    /// extends it while the unit runs, a reattach opens it again), and tells the agents first, as
    /// a restart does: its alerts are planned meanwhile and page nobody. The task says which
    /// agents didn't get the note.
    async fn begin_window(&self, task: &TaskHandle, node: &str) {
        let Some(p) = self.planned() else { return };
        let until = now_ms() + self.upgrade_window.lock().unwrap().as_millis() as u64;
        p.begin(node, Some(until), "upgrading");
        task.step("Telling the nodes' agents about the planned work");
        let missing = p.notify_agents(task.id(), &[node.to_string()], Some(until), "upgrading").await;
        if missing.is_empty() {
            task.step_ok(None);
        } else {
            task.step_ok(Some(maintenance_missing(&missing, &|n: &str| self.name_of(n))));
        }
    }

    /// Follows a detached upgrade to its end: tails its log over the bulk connection (again
    /// after a reconnect, from where it got to), shows "Unpacking 3 of 7", stops apt on Cancel
    /// while that's still safe, and keeps the node's window open while the unit runs (decision
    /// d). Ends when apt's exit status is in the log, or (`interrupted`) when its unit is gone
    /// without one; `Lost` when the node stays away past `upgrade_wait`, or Live stops (apt may
    /// still be running there).
    ///
    /// A press of Cancel is dealt with here, between tails, not beside the tail: apt's exit line
    /// can beat Stop's answer, and losing that race would end the sudo (`exec_in`) and leave a
    /// stopped apt reading as a failed one. It counts as answered only once the node has answered:
    /// a Stop that failed can be pressed again (`TaskHandle::cancel_failed`), and one whose answer
    /// was lost is sent again once the node is back (C-X is safe to repeat), three tries in all.
    async fn follow_upgrade(&self, task: &TaskHandle, node: &str) -> Result<Followed, SbError> {
        let name = self.name_of(node);
        let run = self.runs.lock().unwrap().get(node).cloned().ok_or_else(|| SbError::failed("The upgrade's record went missing."))?;
        task.step(format!("Installing {} package{}", run.total, if run.total == 1 { "" } else { "s" }));
        let mut cancel_seen = false;
        // Tries at Stop whose answer was lost: any of them may have reached apt.
        let mut unanswered = 0;
        let mut quiet_since: Option<Instant> = None;
        // Each sighting of the unit active (at most every thirtieth of the window: a minute)
        // moves the window's end to a window-length from then. Out of contact nothing moves it.
        let window = *self.upgrade_window.lock().unwrap();
        let mut extended = Instant::now();
        loop {
            let ended = self.runs.lock().unwrap().get(node).and_then(|r| r.exit_code);
            if let Some(code) = ended {
                // apt failed before dpkg ran, after a Stop that went unanswered: that Stop got there.
                if unanswered > 0 && code != 0 {
                    self.update_run(node, |r| {
                        if before_dpkg(&r.log) {
                            r.phase = "cancelled".into();
                        }
                    });
                }
                return self.upgrade_ended(task, node, &name, code).await.map(|()| Followed::Ended);
            }
            if task.is_cancelled() && !cancel_seen {
                if self.runs.lock().unwrap().get(node).is_some_and(|r| r.cancellable) {
                    match self.try_stop(task, node, &run.unit).await {
                        StopTry::Stopped | StopTry::NothingLeft => cancel_seen = true,
                        StopTry::TooLate => {
                            cancel_seen = true;
                            unanswered = 0;
                        }
                        StopTry::Failed => task.cancel_failed(),
                        StopTry::NoAnswer => {
                            unanswered += 1;
                            if unanswered < 3 {
                                task.log(format!("Trying again when {name} answers…"));
                                tokio::time::sleep(self.reconnect_poll()).await;
                                if !self.wait_back(node).await {
                                    return Ok(Followed::Lost);
                                }
                                continue;
                            }
                            task.cancel_failed();
                        }
                    }
                } else {
                    cancel_seen = true;
                    task.log(DPKG_RUNS.to_string());
                }
            }
            // Follow the log from the line after the last one read; reconnects resume here.
            let from = self.runs.lock().unwrap().get(node).map(|r| r.lines_read).unwrap_or(0);
            let stop = CancelToken::new();
            let tail = format!("tail -n +{} -F {} 2>/dev/null", from + 1, q(&run.log_path));
            let mut partial = String::new();
            let stream = {
                let (node, task) = (node.to_string(), task.clone());
                let stop = stop.clone();
                let live = self.live.clone();
                async move {
                    live.stream_cmd(&node, &tail, Duration::from_secs(24 * 3600), stop.clone(), |_, chunk| {
                        partial.push_str(&String::from_utf8_lossy(chunk));
                        while let Some(i) = partial.find('\n') {
                            let line: String = partial.drain(..=i).collect();
                            let line = line.trim_end().to_string();
                            let updated = self.update_run(&node, |r| {
                                r.lines_read += 1;
                                r.log.push(line.clone());
                                if r.log.len() > LOG_KEEP {
                                    r.log.remove(0);
                                }
                                if let Some(code) = exit_of(std::slice::from_ref(&line)) {
                                    r.exit_code = Some(code);
                                }
                            });
                            if !is_marker(&line) {
                                task.log(line.clone());
                            }
                            if let Some(r) = updated {
                                let all = r.log.clone();
                                let (phase, done, current, can_stop) = progress_of(&all);
                                let r = self.update_run(&node, |r| {
                                    if r.phase != "cancelled" {
                                        r.phase = phase.into();
                                    }
                                    (r.done, r.current, r.cancellable) = (done, current, can_stop && r.cancellable);
                                })
                                .unwrap();
                                let steps = (r.total * 2).max(1) as f32;
                                let through = match phase {
                                    "configuring" => r.total + done,
                                    "unpacking" => done,
                                    "finishing" => r.total * 2,
                                    _ => 0,
                                };
                                task.progress(through as f32 / steps, Some(describe(&r)));
                                if r.exit_code.is_some() {
                                    stop.cancel();
                                }
                            }
                        }
                    })
                    .await
                }
            };
            // While it runs: is the unit still there, and did someone press Cancel?
            let watch = async {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(self.poll.max(Duration::from_millis(500)).min(Duration::from_secs(2))) => {}
                        // Dealt with at the top of the outer loop, once the tail is stopped.
                        _ = task.cancelled(), if !cancel_seen => return,
                    }
                    if self.runs.lock().unwrap().get(node).and_then(|r| r.exit_code).is_some() {
                        return;
                    }
                    // A unit that's gone without an exit line in the log: give the tail a moment, then read the end.
                    if let Ok(o) = self.live.probe(node, &format!("systemctl show -p ActiveState,Result,LoadState -- {}", q(&run.unit)), Duration::from_secs(10)).await {
                        let active = o.stdout.lines().any(|l| l == "ActiveState=active" || l == "ActiveState=activating");
                        if active {
                            quiet_since = None;
                            if extended.elapsed() >= window / 30 {
                                extended = Instant::now();
                                if let Some(p) = self.planned() {
                                    p.extend(node, now_ms() + window.as_millis() as u64);
                                }
                            }
                        } else if quiet_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(5) {
                            return;
                        }
                    }
                }
            };
            tokio::select! {
                result = stream => {
                    if let Err(e) = result {
                        // The node went away: wait for it, then pick the log up where it stopped.
                        task.log(format!("Lost the connection to {name} ({e}); the upgrade keeps running there. Waiting to reconnect…"));
                        tokio::time::sleep(self.reconnect_poll()).await;
                        if !self.wait_back(node).await {
                            return Ok(Followed::Lost);
                        }
                    }
                }
                _ = watch => stop.cancel(),
            }
            if self.runs.lock().unwrap().get(node).and_then(|r| r.exit_code).is_none() && quiet_since.is_some_and(|t| t.elapsed() > Duration::from_secs(5)) {
                // The unit ended: read the end of its log for the exit line the tail may have missed.
                // Gone meanwhile: the next tail finds that out, and waits for the node.
                let Ok(o) = self.live.probe(node, &format!("tail -n 20 {}", q(&run.log_path)), Duration::from_secs(10)).await else { continue };
                let lines: Vec<String> = o.stdout.lines().map(String::from).collect();
                match exit_of(&lines) {
                    Some(code) => {
                        self.update_run(node, |r| r.exit_code = Some(code));
                    }
                    None => return self.interrupted(task, node, &name, &run.log_path).await,
                }
            }
        }
    }

    /// One try at Stop: says what it came to in the task's log, and keeps it in the run for the
    /// window (`stop_failed`) until a try is answered. The log can lag: the node itself checks that
    /// no dpkg runs, in the same shell that stops apt, and the activity log has that command. A
    /// package-upgrade change, like the start: a fence that allows the upgrade allows its Cancel.
    async fn try_stop(&self, task: &TaskHandle, node: &str, unit: &str) -> StopTry {
        let tried = as_class(FenceClass::AptOne, self.exec(node, &cancel_command(unit), true, Duration::from_secs(30))).await;
        let said = cancel_said(&tried);
        let came_to = stop_came_to(&tried);
        self.update_run(node, |r| match came_to {
            StopTry::Stopped => {
                r.phase = "cancelled".into();
                r.stop_failed = None;
            }
            // No second press to offer: there's nothing left that Stop could stop.
            StopTry::TooLate | StopTry::NothingLeft => {
                r.cancellable = false;
                r.stop_failed = None;
            }
            // The node's words, and they're saved with the run: redacted, as in the task's log.
            StopTry::Failed | StopTry::NoAnswer => r.stop_failed = Some(audit::redact(&said)),
        });
        task.log(said);
        came_to
    }

    /// apt's unit ended with no exit status in its log: something killed it, most likely the
    /// node restarting under it. dpkg may have stopped halfway, so `dpkg --audit` runs as after
    /// any upgrade (as the cluster user: exit 2 with nothing listed is no all clear) and the
    /// task fails with what it said; the run's record goes only after that check. `Lost` when
    /// the node is away past the wait before it could check: the record stays, and the check
    /// runs when the node is back. Three tries without an answer while the node is there fail it
    /// unchecked.
    async fn interrupted(&self, task: &TaskHandle, node: &str, name: &str, log: &str) -> Result<Followed, SbError> {
        let stopped = format!("apt on {name} stopped without an exit status (the node may have restarted)");
        let unchecked = format!("{stopped}, and SparkBox couldn't check the package database. Run sudo dpkg --audit there, and sudo dpkg --configure -a if it lists packages (apt's log: {log}).");
        task.step_failed("apt stopped without an exit status");
        task.step("Checking the package database (dpkg --audit)");
        let mut tries = 0;
        let (code, audit, err) = loop {
            // An exit status is dpkg's answer; none (the channel closed) is the node going away again.
            let why = match self.exec(node, "dpkg --audit", false, Duration::from_secs(60)).await {
                Ok((Some(code), out, err)) => break (code, out, err),
                Ok((None, _, _)) => "no exit status".to_string(),
                Err(e) => e.message,
            };
            tries += 1;
            if tries == 3 {
                task.step_failed(format!("Couldn't check: {why}"));
                return Err(SbError::failed(unchecked).on(node).with_detail(why));
            }
            // Wait for it, as the log's tail does.
            tokio::time::sleep(self.reconnect_poll()).await;
            if !self.wait_back(node).await {
                task.step_failed(format!("Couldn't check: {why}"));
                task.log(format!("{stopped}; SparkBox lost {name} again before it could check the package database, and checks it when {name} is back."));
                return Ok(Followed::Lost);
            }
        };
        if !audit.trim().is_empty() {
            task.log(audit.clone());
            task.step_failed("dpkg reports packages that aren't fully installed");
            return Err(SbError::failed(format!("{stopped}, and dpkg reports packages that aren't fully installed. Run sudo dpkg --configure -a there (apt's log: {log}).")).on(node).with_detail(audit));
        }
        if code != 0 {
            let why = err.lines().last().unwrap_or("").trim().to_string();
            task.log(err.clone());
            task.step_failed(format!("Couldn't check: {why}"));
            return Err(SbError::failed(unchecked).on(node).with_detail(err));
        }
        task.step_ok(Some("No broken or half-installed packages".into()));
        Err(SbError::failed(format!("{stopped}. dpkg --audit lists no broken or half-installed packages; apt's log shows how far it got: {log}.")).on(node))
    }

    async fn upgrade_ended(&self, task: &TaskHandle, node: &str, name: &str, code: i32) -> Result<(), SbError> {
        // What the unit checked after apt (it does, whatever apt's code).
        self.update_run(node, |r| (r.apt_check, r.needrestart) = post_checks(&r.log));
        let cancelled = self.runs.lock().unwrap().get(node).is_some_and(|r| r.phase == "cancelled");
        if cancelled {
            task.step_ok(Some("Stopped before anything was installed".into()));
            return Ok(());
        }
        if code != 0 {
            let tail: Vec<String> = self.runs.lock().unwrap().get(node).map(|r| r.log.iter().rev().take(12).rev().cloned().collect()).unwrap_or_default();
            let why = tail.iter().rev().find(|l| l.starts_with("E: ")).cloned().unwrap_or_else(|| format!("apt-get exited with {code}"));
            task.step_failed(why.clone());
            return Err(SbError::failed(format!("apt-get failed on {name}: {why}")).on(node).with_detail(tail.join("\n")));
        }
        self.update_run(node, |r| r.phase = "finishing".into());
        task.step_ok(None);
        Ok(())
    }

    /// Picks up upgrades SparkBox started before it quit (or lost the node): every saved run is
    /// its node's record at once (another node's upgrade saving its own can't drop it), and each
    /// node gets one follower, which waits for the node with no limit (until Live stops: the
    /// record then stays for the next start), follows the unit to its end and ends as a first
    /// run does (`after_upgrade`). Safe to call each time Live starts: a node already followed,
    /// or waited for, gets no second follower.
    pub fn reattach_upgrades(self: &Arc<Self>) {
        let saved: Vec<UpgradeRun> = std::fs::read(saved_path(self)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        {
            let mut runs = self.runs.lock().unwrap();
            for run in &saved {
                // A record this process has newer word of (a follower updates it) stays.
                if !runs.get(&run.node).is_some_and(|r| !r.simulated && !is_final(&r.phase)) {
                    runs.insert(run.node.clone(), run.clone());
                }
            }
        }
        for run in saved {
            self.look_again(&run.node);
        }
    }

    /// Follows `node`'s upgrade again once the node is back, unless this SparkBox already
    /// follows it or waits for it.
    fn look_again(self: &Arc<Self>, node: &str) {
        if !self.following.lock().unwrap().insert(node.to_string()) {
            return;
        }
        let (me, node) = (self.clone(), node.to_string());
        tauri::async_runtime::spawn(async move {
            // Lost again: wait for it again.
            while me.wait_for_node(&node).await {
                let Some(run) = me.runs.lock().unwrap().get(&node).filter(|r| !r.simulated && !is_final(&r.phase)).cloned() else { break };
                if !me.follow_again(run).await {
                    break;
                }
            }
            me.following.lock().unwrap().remove(&node);
        });
    }

    /// One saved run, picked up on its connected node as a task of its own. One SparkBox was
    /// still starting, or lost, is looked for on the node first. True when it's lost again.
    async fn follow_again(&self, run: UpgradeRun) -> bool {
        let name = self.name_of(&run.node);
        let title = format!("{} (picked up again)", run.title);
        let task = self.tasks.start(TaskSpec::new("updates", title.clone(), std::slice::from_ref(&run.node)).cancellable().on_node());
        // Its rows carry the task's id and title, as the first run's did.
        let op = OpMeta { title, source: "ui".into() };
        let task_id = task.id().to_string();
        audit::with_op_id(task_id, op, async move {
            let copied = run.evidence.as_ref().is_some_and(|p| std::path::Path::new(p).exists());
            let look_first = matches!(run.phase.as_str(), "starting" | "lost");
            let mut run = UpgradeRun { task_id: task.id().to_string(), lines_read: 0, log: Vec::new(), exit_code: None, verification: None, ..run };
            // The cancel state starts over: what's in the log decides it.
            run.cancellable = true;
            run.stop_failed = None;
            self.set_run(&run);
            let node = run.node.clone();
            let windowed = AtomicBool::new(false);
            let never = AtomicBool::new(false);
            let lost = AtomicBool::new(false);
            let result: Result<String, SbError> = async {
                if look_first {
                    task.log(if run.phase == "starting" {
                        format!("SparkBox was starting {} on {name} when it stopped; checking whether it started", run.unit)
                    } else {
                        format!("SparkBox lost {name} while {} ran; checking that it's there", run.unit)
                    });
                    match self.upgrade_started(&node, &run).await {
                        Some(true) => {
                            self.update_run(&node, |r| r.phase = "preparing".into());
                        }
                        Some(false) => {
                            never.store(true, Ordering::SeqCst);
                            return Err(SbError::failed(if run.phase == "starting" {
                                format!("The upgrade on {name} never started (SparkBox stopped while starting it); apt didn't run there.")
                            } else {
                                format!("The upgrade on {name} isn't there: neither {} nor its log ({}) is on the node, so apt isn't running it.", run.unit, run.log_path)
                            })
                            .on(&node));
                        }
                        None => {
                            lost.store(true, Ordering::SeqCst);
                            return Ok(String::new());
                        }
                    }
                }
                task.log(format!("Following {} on {name} again", run.unit));
                // Still upgrading: its window opens again for the rest of the run.
                self.begin_window(&task, &node).await;
                windowed.store(true, Ordering::SeqCst);
                if !copied {
                    self.keep_dpkg_before(&task, &node).await;
                }
                if self.follow_upgrade(&task, &node).await? == Followed::Lost {
                    lost.store(true, Ordering::SeqCst);
                    return Ok(String::new());
                }
                self.after_upgrade(&task, &node, &name, false, format!("The upgrade on {name} finished")).await
            }
            .await;
            if windowed.load(Ordering::SeqCst) {
                if let Some(p) = self.planned() {
                    p.end(&node);
                }
            }
            let lost = lost.load(Ordering::SeqCst);
            let stopped = matches!(&result, Ok(s) if s.starts_with("Stopped before"));
            if never.load(Ordering::SeqCst) {
                // Nothing ran: no record of a run.
                let mut runs = self.runs.lock().unwrap();
                if runs.get(&node).is_some_and(|r| r.task_id == task.id()) {
                    runs.remove(&node);
                }
            }
            self.update_run(&node, |r| {
                if r.task_id != task.id() {
                    return;
                }
                if lost {
                    r.phase = "lost".into();
                } else if result.is_err() {
                    r.phase = "failed".into();
                } else if r.phase != "cancelled" {
                    r.phase = "done".into();
                }
                r.cancellable = false;
            });
            save_running(self);
            match result {
                Ok(_) if lost => {
                    let (summary, warning) = lost_words(&name, &run.log_path);
                    task.finish_warn(summary, warning);
                }
                Ok(summary) if stopped => task.finish_cancelled(summary),
                Ok(summary) => task.finish_ok(summary),
                // Only a run that was never there can read Cancelled: this one ran.
                Err(e) if never.load(Ordering::SeqCst) => task.finish_err(e.message),
                Err(e) => task.finish_failed(e.message),
            }
            lost
        })
        .await
    }

    /// Demo: apt's output, a package at a time, with the same phases and Cancel rules as Live.
    /// Returns true when it was stopped before anything was installed.
    async fn demo_upgrade(&self, task: &TaskHandle, node: &str, title: &str, pending: &[PendingPackage], take: &[String]) -> Result<bool, SbError> {
        let id = unit_id(task.id());
        let mut run = UpgradeRun {
            node: node.into(),
            task_id: task.id().into(),
            title: title.into(),
            unit: format!("sparkbox-upgrade-{id}"),
            log_path: format!("/var/log/sparkbox/upgrade-{id}.log"),
            packages: take.to_vec(),
            total: take.len(),
            phase: "preparing".into(),
            cancellable: true,
            started_ms: now_ms(),
            simulated: true,
            ..Default::default()
        };
        self.set_run(&run);
        task.step(format!("Installing {} package{}", take.len(), if take.len() == 1 { "" } else { "s" }));
        let version = |p: &str| pending.iter().find(|x| x.name == p).map(|x| (x.from.clone(), x.to.clone())).unwrap_or_default();
        let mut lines: Vec<String> = vec![
            "Reading package lists...".into(),
            "Building dependency tree...".into(),
            "Reading state information...".into(),
            "The following packages will be upgraded:".into(),
            format!("  {}", take.join(" ")),
            format!("{} upgraded, 0 newly installed, 0 to remove and 0 not upgraded.", take.len()),
        ];
        for (i, p) in take.iter().enumerate() {
            lines.push(format!("Get:{} http://ports.ubuntu.com/ubuntu-ports noble-updates/main arm64 {p} arm64 {} [{} kB]", i + 1, version(p).1, 180 + i * 97));
        }
        lines.push(format!("Fetched {:.1} MB in 3s (8,412 kB/s)", take.len() as f64 * 1.7));
        lines.push("(Reading database ... 214337 files and directories currently installed.)".into());
        for p in take {
            let (from, to) = version(p);
            lines.push(format!("Preparing to unpack .../{p}_{to}_arm64.deb ..."));
            lines.push(format!("Unpacking {p} ({to}) over ({from}) ..."));
        }
        for p in take {
            lines.push(format!("Setting up {p} ({}) ...", version(p).1));
        }
        lines.push("Processing triggers for libc-bin (2.39-0ubuntu8.6) ...".into());
        lines.push("SPARKBOX-EXIT 0".into());
        let mut cancel_seen = false;
        for line in lines {
            // About 0.4 s a package step at the real poll interval (1 s); tests run it faster.
            let step = if line.starts_with("Get:") || line.starts_with("Unpacking") || line.starts_with("Setting up") { 0.42 } else { 0.12 };
            tokio::select! {
                _ = tokio::time::sleep(self.poll.mul_f64(step)) => {}
                _ = task.cancelled(), if !cancel_seen => {
                    cancel_seen = true;
                    if run.cancellable {
                        task.log("Stopping apt before it installs anything…".to_string());
                        run.phase = "cancelled".into();
                        run.log.push("E: Sub-process /usr/bin/dpkg received a termination signal".into());
                        run.cancellable = false;
                        self.set_run(&run);
                        self.simulated(node, &format!("Stop the upgrade on {}", self.name_of(node)), &format!("sudo sh -c {}", q(&cancel_command(&run.unit))));
                        return Ok(true);
                    }
                    task.log(DPKG_RUNS.to_string());
                }
            }
            if !is_marker(&line) {
                task.log(line.clone());
            }
            run.log.push(line);
            let (phase, done, current, can_stop) = progress_of(&run.log);
            (run.phase, run.done, run.current, run.cancellable) = (phase.into(), done, current, can_stop && run.cancellable);
            let through = match phase {
                "configuring" => run.total + done,
                "unpacking" => done,
                "finishing" => run.total * 2,
                _ => 0,
            };
            task.progress(through as f32 / (run.total * 2).max(1) as f32, Some(describe(&run)));
            self.set_run(&run);
        }
        run.exit_code = Some(0);
        self.set_run(&run);
        Ok(false)
    }

    /// The Updates window's Cancel: stops apt while that's safe; says why not otherwise.
    pub fn updates_cancel(&self, node: &str) -> Result<String, SbError> {
        let run = self.runs.lock().unwrap().get(node).cloned().ok_or_else(|| SbError::invalid("No upgrade is running there."))?;
        if run.phase == "lost" {
            return Err(SbError::invalid("SparkBox lost the node while it was upgrading; Cancel works again once it's followed again."));
        }
        if !run.cancellable {
            return Err(SbError::invalid("Can't stop safely now: dpkg is installing packages. It will finish on its own."));
        }
        self.tasks.cancel(&run.task_id).map_err(SbError::invalid)?;
        Ok("Stopping apt before it installs anything".into())
    }
}

// ---- Detached upgrades

/// Stops a detached upgrade's apt-get, unless dpkg is already running on the node (checked in
/// the same root shell, just before): exits 3 and says so instead. Only apt-get gets TERM, and
/// only the one under this upgrade's own shell (its unit's main process): the shell lives on to
/// log apt's exit, and nothing else on the node is touched.
pub fn cancel_command(unit: &str) -> String {
    format!(
        "if pgrep -x dpkg >/dev/null; then echo 'dpkg is installing packages' >&2; exit 3; fi; \
p=$(systemctl show -p MainPID --value -- {}); [ \"${{p:-0}}\" -gt 0 ] && pkill -TERM -P \"$p\" -f '(^|/)apt-get( |$)'",
        q(unit)
    )
}

/// An upgrade in progress (or just finished) on one node, for the Updates window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpgradeRun {
    pub node: String,
    pub task_id: String,
    pub title: String,
    /// The transient unit and its log on the node.
    pub unit: String,
    pub log_path: String,
    pub packages: Vec<String>,
    /// checking (the dry run; nothing started) | starting (the start sent, not yet answered) |
    /// preparing | downloading | unpacking | configuring | finishing | done | failed | cancelled |
    /// lost (SparkBox lost the node while apt may still run: followed again when it's back)
    pub phase: String,
    /// Packages through the current phase, of `total`.
    pub done: usize,
    pub total: usize,
    pub current: Option<String>,
    /// The newest lines of apt's output (the last 400).
    pub log: Vec<String>,
    /// Lines of the log read so far (where a reattached tail starts).
    pub lines_read: usize,
    /// Cancel stops apt before anything is installed; false once dpkg runs, or the node says
    /// there's nothing left to stop.
    pub cancellable: bool,
    /// Why the last Stop didn't work (`cancel_said`'s words), until a later one is answered.
    pub stop_failed: Option<String>,
    pub started_ms: u64,
    pub exit_code: Option<i32>,
    pub simulated: bool,
    /// The package list saved on the node before apt started (`dpkg-query -W`).
    pub dpkg_before: Option<String>,
    /// Its copy in the evidence folder on this Mac.
    pub evidence: Option<String>,
    /// What needrestart listed after apt: services on old libraries, the kernel.
    pub needrestart: Option<NeedRestart>,
    /// What `apt-get check` printed after apt.
    pub apt_check: Option<String>,
    /// Fresh package status after apt and the package-database checks. None is not an all-clear.
    pub verification: Option<UpgradeVerification>,
    pub dl_limit_kbps: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeVerification {
    pub checked_ms: u64,
    pub remaining: Vec<PendingPackage>,
}

impl UpgradeVerification {
    fn summary(&self) -> String {
        if self.remaining.is_empty() {
            "Package check passed: no package updates remain".into()
        } else {
            let held = self.remaining.iter().filter(|p| p.held).count();
            let protected = self.remaining.iter().filter(|p| p.sensitive && !p.held).count();
            format!("Package check complete: {} {} ({held} held, {protected} GPU-stack packages)", self.remaining.len(), if self.remaining.len() == 1 { "update remains" } else { "updates remain" })
        }
    }
}

const LOG_KEEP: usize = 400;

/// An upgrade's maintenance window: 30 minutes from the start, and from each time its unit is
/// seen active (`Ops::upgrade_window`; tests shorten it).
pub(crate) const UPGRADE_WINDOW: Duration = Duration::from_secs(30 * 60);

/// How long a follower waits for its node before the run counts as lost (`Ops::upgrade_wait`).
pub(crate) const UPGRADE_WAIT: Duration = Duration::from_secs(30 * 60);

/// Cancel once dpkg runs (the log says so, or the node's own check does).
const DPKG_RUNS: &str = "Can't stop safely now: dpkg is installing packages, and stopping it would leave them half-configured. It will finish on its own.";

/// How following an upgrade ended.
#[derive(Debug, PartialEq, Eq)]
enum Followed {
    /// apt's exit is in the log.
    Ended,
    /// The node stayed away past the wait, or Live stopped: apt may still be running there.
    Lost,
}

/// done, failed and cancelled: nothing runs for it any more.
fn is_final(phase: &str) -> bool {
    matches!(phase, "done" | "failed" | "cancelled")
}

/// A lost run's task ends with a warning: (summary, warning).
fn lost_words(name: &str, log: &str) -> (String, String) {
    (
        format!("Lost {name} while it was upgrading"),
        format!("Lost {name} while it was upgrading; apt may still be running there. SparkBox looks again when {name} is back, and after a restart (log: {log})."),
    )
}

/// What Cancel came to (C-X, sent as root): stopped; dpkg already installing; nothing left to
/// stop (pkill matched nothing, or the unit has no main process); refused before it was sent,
/// in the refusal's words; or why it failed.
fn cancel_said(tried: &Result<(Option<u32>, String, String), SbError>) -> String {
    match tried {
        Ok((Some(0), _, _)) => "Stopping apt before it installs anything…".into(),
        Ok((Some(3), _, err)) if err.contains("dpkg is installing packages") => DPKG_RUNS.into(),
        Ok((Some(1), _, _)) => "apt had already finished; nothing to stop.".into(),
        Err(e) if e.kind == ErrorKind::LookOnly => "Look-only refused Cancel; the upgrade keeps running".into(),
        Err(e) if not_sent(e) => format!("Cancel was refused ({}); the upgrade keeps running", e.message),
        Ok((code, _, err)) => {
            let last = err.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(String::from);
            format!("Couldn't stop it: {}", last.unwrap_or_else(|| code.map(|c| format!("exit {c}")).unwrap_or_else(|| "no exit status".into())))
        }
        Err(e) => format!("Couldn't stop it: {}", e.message),
    }
}

/// What one try at Cancel came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopTry {
    /// apt got TERM.
    Stopped,
    /// Nothing left to stop: apt had ended (pkill matched nothing, or the unit has no main process).
    NothingLeft,
    /// dpkg already runs on the node, so it wasn't stopped.
    TooLate,
    /// The node couldn't stop it and said why, or it was refused before it was sent: apt runs on.
    Failed,
    /// No answer (the connection dropped, no exit status, no reply in time): it may have reached apt.
    NoAnswer,
}

/// `cancel_said`'s cases, for what happens next. Exit 3 is C-X's own only with its words: it's
/// also what pkill says when it fails.
fn stop_came_to(tried: &Result<(Option<u32>, String, String), SbError>) -> StopTry {
    match tried {
        Ok((Some(0), _, _)) => StopTry::Stopped,
        Ok((Some(1), _, _)) => StopTry::NothingLeft,
        Ok((Some(3), _, err)) if err.contains("dpkg is installing packages") => StopTry::TooLate,
        Ok((Some(_), _, _)) => StopTry::Failed,
        Err(e) if not_sent(e) => StopTry::Failed,
        Ok((None, _, _)) | Err(_) => StopTry::NoAnswer,
    }
}

/// The installed packages' names, for the GPU-stack pin (a read).
pub const PRESET_READ: &str = "dpkg-query -W -f='${Package}\\n'";

impl Ops {
    /// The upgrade running (or last run) on each node, as this SparkBox knows it.
    pub fn runs(&self) -> Vec<UpgradeRun> {
        self.runs.lock().unwrap().values().cloned().collect()
    }

    fn set_run(&self, run: &UpgradeRun) {
        self.runs.lock().unwrap().insert(run.node.clone(), run.clone());
    }

    fn update_run(&self, node: &str, f: impl FnOnce(&mut UpgradeRun)) -> Option<UpgradeRun> {
        let mut runs = self.runs.lock().unwrap();
        let r = runs.get_mut(node)?;
        f(r);
        Some(r.clone())
    }
}

/// Where SparkBox keeps the units it started, so it can pick them up after a restart.
fn saved_path(ops: &Ops) -> PathBuf {
    ops.live.data_path("upgrades.json")
}

/// Saves every run apt may still be running (a lost one too): all but a claim whose dry run is
/// under way (nothing has started for it) and the ones that ended.
fn save_running(ops: &Ops) {
    let running: Vec<UpgradeRun> = ops.runs().into_iter().filter(|r| !r.simulated && !matches!(r.phase.as_str(), "checking" | "done" | "failed" | "cancelled")).map(|r| UpgradeRun { log: Vec::new(), ..r }).collect();
    let _ = crate::util::save_json(&saved_path(ops), &running);
}

/// The lines the unit writes around apt for SparkBox, not apt's own output.
fn is_marker(line: &str) -> bool {
    line.starts_with("SPARKBOX-EXIT") || line == "SPARKBOX-AFTER" || line == "SPARKBOX-AFTER-END"
}

/// The task's steps for what the unit checked after apt: `apt-get check` (broken dependencies
/// fail the task, as `dpkg --audit` does) and needrestart's list. A run without the checks (one
/// round 2 started) has neither step.
fn after_checks(task: &TaskHandle, name: &str, run: &UpgradeRun) -> Result<Option<NeedRestart>, SbError> {
    let Some(check) = &run.apt_check else { return Ok(None) };
    task.step("Checking dependencies (apt-get check)");
    let errors: Vec<&str> = check.lines().filter(|l| l.starts_with("E:")).collect();
    if !errors.is_empty() {
        task.log(check.clone());
        task.step_failed(errors.join("\n"));
        return Err(SbError::failed(format!("The upgrade finished, but apt-get check reports broken dependencies on {name}: {}", errors[0])).on(&run.node).with_detail(check.clone()));
    }
    task.step_ok(Some("No broken dependencies".into()));
    task.step("Services still using old libraries (needrestart)");
    match &run.needrestart {
        None => task.step_ok(Some("needrestart listed nothing (it may not be installed)".into())),
        Some(nr) => {
            let mut said: Vec<String> = Vec::new();
            if nr.services.is_empty() {
                said.push("None".into());
            } else {
                said.push(nr.services.join(", "));
            }
            said.extend(kernel_sentence(nr));
            task.step_ok(Some(said.join(". ")));
        }
    }
    Ok(run.needrestart.clone())
}

/// What apt's output says so far: (phase, packages through it, the one in hand, can it still stop).
/// From `SPARKBOX-AFTER` on apt is done and the unit checks what's left: "finishing".
pub fn progress_of(lines: &[String]) -> (&'static str, usize, Option<String>, bool) {
    let name = |l: &str, prefix: &str| l.strip_prefix(prefix).and_then(|r| r.split_whitespace().next()).map(|n| n.split(':').next().unwrap_or(n).to_string());
    let (mut unpacked, mut set_up) = (BTreeSet::new(), BTreeSet::new());
    let (mut current, mut dpkg, mut fetching, mut ended) = (None, false, false, false);
    for l in lines {
        let l = l.trim();
        if l == "SPARKBOX-AFTER" || l.starts_with("SPARKBOX-EXIT ") {
            // apt-get check's "Reading package lists" after this isn't apt downloading again.
            ended = true;
            break;
        } else if let Some(n) = name(l, "Unpacking ") {
            unpacked.insert(n.clone());
            current = Some(n);
            dpkg = true;
        } else if let Some(n) = name(l, "Setting up ") {
            set_up.insert(n.clone());
            current = Some(n);
            dpkg = true;
        } else if l.starts_with("(Reading database") || l.starts_with("Preparing to unpack") {
            dpkg = true;
        } else if l.starts_with("Get:") || l.starts_with("Fetched ") || l.starts_with("Reading package lists") {
            fetching = true;
        }
    }
    let phase = match (ended, set_up.len(), unpacked.len(), dpkg, fetching) {
        (true, ..) => "finishing",
        (_, s, _, _, _) if s > 0 => "configuring",
        (_, _, u, _, _) if u > 0 => "unpacking",
        (_, _, _, true, _) => "unpacking",
        (_, _, _, _, true) => "downloading",
        _ => "preparing",
    };
    let done = if set_up.is_empty() { unpacked.len() } else { set_up.len() };
    (phase, done, current, !dpkg && !ended)
}

/// apt ended, or is still going, with no line from dpkg in its output.
fn before_dpkg(lines: &[String]) -> bool {
    let apt: Vec<String> = lines.iter().take_while(|l| l.trim() != "SPARKBOX-AFTER" && !l.trim().starts_with("SPARKBOX-EXIT ")).cloned().collect();
    progress_of(&apt).3
}

/// The exit status apt reported in the log (`SPARKBOX-EXIT <code>`).
pub fn exit_of(lines: &[String]) -> Option<i32> {
    lines.iter().rev().find_map(|l| l.trim().strip_prefix("SPARKBOX-EXIT ").and_then(|c| c.trim().parse().ok()))
}

/// "Unpacking 3 of 7 · libnvidia-compute-580".
pub fn describe(run: &UpgradeRun) -> String {
    let of = |verb: &str| format!("{verb} {} of {}{}", run.done.max(1).min(run.total.max(1)), run.total.max(1), run.current.as_deref().map(|c| format!(" · {c}")).unwrap_or_default());
    match run.phase.as_str() {
        "checking" => "Checking what will change".into(),
        "starting" | "preparing" => "Starting apt".into(),
        "downloading" => "Downloading packages".into(),
        "unpacking" => of("Unpacking"),
        "configuring" => of("Setting up"),
        "finishing" => "Finishing".into(),
        "cancelled" => "Stopped".into(),
        other => other.to_string(),
    }
}

/// A unit name and log file name part from a task id.
fn unit_id(task_id: &str) -> String {
    task_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect()
}

/// The apt-get options a run and its dry run share (P1.24): wait up to 5 minutes for the dpkg
/// lock (apt-daily, the DGX Dashboard's updater), keep the .debs this run downloads (the new
/// versions, so they are no way back: see the rollback note at the top of this file), and keep
/// config files the admin changed.
const APT_OPTS: &str = "-o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold";

/// The shared options and the download limit, as both dry runs and the run pass them.
fn apt_opts(opts: &UpgradeOpts) -> String {
    let limit = opts.dl_limit_kbps.map(|k| format!(" -o Acquire::http::Dl-Limit={k} -o Acquire::https::Dl-Limit={k}")).unwrap_or_default();
    format!("{APT_OPTS}{limit}")
}

/// The discovery dry run's arguments (C2): `dist-upgrade` for everything, `install
/// --only-upgrade <names>` for a selection.
fn apt_args(packages: &[String], opts: &UpgradeOpts) -> String {
    let verb = if packages.is_empty() { "dist-upgrade".to_string() } else { format!("install --only-upgrade {}", packages.iter().map(|p| q(p)).collect::<Vec<_>>().join(" ")) };
    format!("{} -y {verb}", apt_opts(opts))
}

/// The run's arguments and its last dry run's (C1′, C2′): exactly the pinned versions
/// (`name=version`), upgrades only, and nothing removed. There's no `dist-upgrade` run: every
/// installed package the shown plan upgrades is pinned, and new dependencies are left to apt.
fn pinned_args(pins: &[String], opts: &UpgradeOpts) -> String {
    format!("{} -y --no-remove install {}{}", apt_opts(opts), if opts.protected { "" } else { "--only-upgrade " }, pins.iter().map(|p| q(p)).collect::<Vec<_>>().join(" "))
}

/// The apt-get a detached upgrade runs (C1′, on `pins`): non-interactive, and with needrestart's
/// apt hook held (`NEEDRESTART_SUSPEND=1`, list mode if it runs at all), so needrestart restarts
/// nothing: on stock Ubuntu 24.04 it would restart what it flags, Docker's daemon and every
/// container with it (the staging test `staging_shipped_upgrade_and_needrestart` checks it on a
/// real needrestart). A package's own scripts may still restart its service. The activity log's
/// head line for an upgrade is this command.
pub fn apt_command_opts(pins: &[String], opts: &UpgradeOpts) -> String {
    format!("env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get {}", pinned_args(pins, opts))
}

/// C2′: the dry run of exactly the pinned command, run just before the start (no env, no sudo).
pub fn pinned_dry_run_opts(pins: &[String], opts: &UpgradeOpts) -> String {
    format!("apt-get -s {}", pinned_args(pins, opts))
}

/// `apt_command_opts` with no options.
#[cfg(test)]
pub fn apt_command(pins: &[String]) -> String {
    apt_command_opts(pins, &UpgradeOpts::default())
}

/// What the upgrade's head line in the activity log says it runs: `apt_command`.
#[cfg(test)]
pub fn head_command(pins: &[String]) -> String {
    apt_command(pins)
}

/// `simulate_command_opts` with no options.
#[cfg(test)]
pub fn simulate_command(packages: &[String]) -> String {
    simulate_command_opts(packages, &UpgradeOpts::default())
}

/// The discovery dry run (C2): what an upgrade of `packages` would do now (no env, no sudo).
/// Its plan is what Upgrade binds to.
pub fn simulate_command_opts(packages: &[String], opts: &UpgradeOpts) -> String {
    format!("apt-get -s {}", apt_args(packages, opts))
}

/// Where an upgrade's root command saves the package list before apt runs:
/// `<log dir>/dpkg-before-<id>.txt` for the unit `sparkbox-upgrade-<id>`.
pub fn dpkg_before_path(log_dir: &str, unit: &str) -> String {
    format!("{log_dir}/dpkg-before-{}.txt", unit.strip_prefix("sparkbox-upgrade-").unwrap_or(unit))
}

/// `detached_command_opts` with no options.
#[cfg(test)]
pub fn detached_command(unit: &str, log_dir: &str, log: &str, pins: &[String]) -> String {
    detached_command_opts(unit, log_dir, log, pins, &UpgradeOpts::default())
}

/// The command that starts the detached upgrade (run with sudo, one command). It makes the log
/// folder readable by the cluster user (0755), saves `dpkg-query -W` there first, then starts
/// the unit. In the unit, after apt: `apt-get check` and `needrestart -b -r l` between
/// `SPARKBOX-AFTER` and `SPARKBOX-AFTER-END`; the last line is `SPARKBOX-EXIT <apt's code>`,
/// since `--collect` forgets a unit's result once it ends. The shell around apt ignores TERM,
/// so whatever else signals the unit, it outlives apt long enough to write that line; apt
/// itself gets the default TERM. apt is C1′: `pins` are `name=version`, exactly the shown plan's
/// installed packages (the fence's `only` items are their names).
pub fn detached_command_opts(unit: &str, log_dir: &str, log: &str, pins: &[String], opts: &UpgradeOpts) -> String {
    let apt = apt_command_opts(pins, opts);
    let inner = format!("trap : TERM; {apt}; code=$?; echo SPARKBOX-AFTER; apt-get check; needrestart -b -r l; echo SPARKBOX-AFTER-END; echo \"SPARKBOX-EXIT $code\"; exit $code");
    format!(
        "mkdir -p {d} && chmod 0755 {d} && dpkg-query -W > {before} && systemd-run --unit {unit} --collect -p StandardOutput=append:{log} -p StandardError=append:{log} /bin/sh -c {}",
        q(&inner),
        d = q(log_dir),
        before = q(&dpkg_before_path(log_dir, unit))
    )
}

/// `apt-mark hold 'a' 'b'` / `apt-mark unhold 'a'` (run with sudo).
pub fn hold_command(packages: &[String], hold: bool) -> String {
    format!("apt-mark {} {}", if hold { "hold" } else { "unhold" }, packages.iter().map(|p| q(p)).collect::<Vec<_>>().join(" "))
}

/// Refused before it was sent (the write exit, sudo, the login): nothing reached the node.
fn not_sent(e: &SbError) -> bool {
    matches!(
        e.kind,
        ErrorKind::Fenced | ErrorKind::LookOnly | ErrorKind::OtherController | ErrorKind::WrongNode | ErrorKind::Locked | ErrorKind::NeedsAuth | ErrorKind::SudoRejected | ErrorKind::NoPassword | ErrorKind::AuthRejected | ErrorKind::Invalid | ErrorKind::DemoOnly
    )
}

/// Why no upgrade starts on a node SparkBox has declared in a power state (other than off).
fn power_busy(name: &str, state: &str) -> String {
    match state {
        "shutting-down" => format!("{name} is shutting down."),
        "shutdown-scheduled" => format!("{name} has a shutdown scheduled."),
        "restart-scheduled" => format!("{name} has a restart scheduled; upgrade it once it's back."),
        "waking" => format!("{name} is waking up; upgrade it once it's back."),
        _ => format!("{name} is restarting; upgrade it once it's back."),
    }
}

fn check_names(packages: &[String]) -> Result<(), SbError> {
    let ok = |p: &String| !p.is_empty() && p.len() <= 128 && !p.starts_with('-') && p.chars().all(|c| c.is_ascii_alphanumeric() || "+-.:_".contains(c));
    if packages.iter().all(ok) {
        Ok(())
    } else {
        Err(SbError::new(ErrorKind::Invalid, "That package name has characters SparkBox won't pass to apt."))
    }
}

fn list_names(packages: &[String]) -> String {
    match packages {
        [one] => one.clone(),
        many => format!("{} packages", many.len()),
    }
}

/// apt on the fake nodes as the real one takes C1′'s pins (fake_node.rs's shim takes names only).
#[cfg(test)]
pub(crate) mod fake {
    use crate::fake_node::Node;

    /// apt-get as the real one takes `name=version`: the version must be the one it would
    /// install (the shim's 2.0-1), else apt's own error and exit 100; then the shim runs with the
    /// names, as `apt-get` still (Cancel's pkill looks for that name). Words before the verb (`-o`
    /// and its setting) pass as they are.
    const PINNED_APT: &str = r#"#!/bin/sh
verb=""
for a in "$@"; do
  shift
  if [ -n "$verb" ]; then
    case "$a" in
      -*) ;;
      *=*)
        v=${a#*=}; a=${a%%=*}
        if [ "$v" != "2.0-1" ]; then echo "E: Version '$v' for '$a' was not found" >&2; exit 100; fi ;;
    esac
  else
    case "$a" in install|update|upgrade|dist-upgrade|check) verb=$a ;; esac
  fi
  set -- "$@" "$a"
done
exec "$(dirname "$0")/shim/apt-get" "$@"
"#;

    /// Puts the pin-taking apt-get in front of the node's shim (moved to `bin/shim/apt-get`).
    pub(crate) fn pin_aware(node: &Node) {
        let shim = std::fs::read_to_string(node.bin_dir.join("apt-get")).unwrap();
        std::fs::create_dir_all(node.bin_dir.join("shim")).unwrap();
        node.bin("shim/apt-get", &shim);
        node.bin("apt-get", PINNED_APT);
    }

    /// The node goes away: sshd takes no new logins (as after `systemctl stop ssh`) and every
    /// connection drops. Its processes, apt's unit among them, run on.
    pub(crate) async fn away(node: &Node) {
        std::fs::write(node.state_dir.join("sshd-stopped"), "").unwrap();
        node.drop_connections().await;
    }

    /// It's back: logins work again.
    pub(crate) fn back(node: &Node) {
        let _ = std::fs::remove_file(node.state_dir.join("sshd-stopped"));
    }

    /// The node restarts under apt: its connections drop, every process of the upgrade's unit
    /// dies at once (SIGKILL to the unit's process group, as the power going does: no exit line
    /// in the log), systemd forgets the transient unit, and it's back. With `half`, dpkg's
    /// database is left as a real one is after a kill mid-unpack: that package half-installed
    /// (the fake's dpkg --audit lists what `apt/audit` says).
    pub(crate) async fn restart(node: &Node, half: Option<&str>) {
        away(node).await;
        let units = node.state_dir.join("units");
        let unit = std::fs::read_dir(&units).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("sparkbox-upgrade-"))).expect("the upgrade's unit");
        let pid: i32 = std::fs::read_to_string(&unit).unwrap().lines().find_map(|l| l.strip_prefix("MainPID=")).unwrap().trim().parse().unwrap();
        assert!(pid > 0, "the unit runs");
        assert!(std::process::Command::new("kill").args(["-KILL", &format!("-{pid}")]).status().unwrap().success());
        // --collect: systemd-run forgets the unit once its process is gone.
        for _ in 0..500 {
            if !unit.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!unit.exists(), "the unit is gone");
        if let Some(p) = half {
            let audit = format!("The following packages are only half installed, due to problems during\ninstallation.  The installation can probably be completed by retrying it;\nthe packages can be removed using dselect or dpkg --remove:\n {p:<20} a test library\n");
            std::fs::write(node.state_dir.join("apt/audit"), audit).unwrap();
        }
        back(node);
    }
}

#[cfg(test)]
mod detached_tests {
    use super::*;
    use std::sync::Mutex;

    use crate::demo::DemoCluster;
    use crate::fake_node::{connect, fake_node, temp, Node, Script};
    use crate::inventory::Inventory;
    use crate::tasks::{Task, TaskState, Tasks};

    /// A node with two upgradable packages: apt's dry run (of the packages it names, or both for
    /// dist-upgrade) and the status script are answered; sudo, systemd-run, apt-get, systemctl,
    /// dpkg, dpkg-query and needrestart are the fake node's shims, really run.
    fn apt() -> Script {
        Arc::new(|cmd, _| {
            if cmd.starts_with("apt-get -s ") {
                // C2 names packages, C2′ pins them (`'libfoo=2.0-1'`): the same plan either way.
                let named = cmd.split_once(" install --only-upgrade ").map(|(_, p)| p.split_whitespace().map(|w| w.trim_matches('\'').split('=').next().unwrap_or("").to_string()).collect::<Vec<_>>());
                let pkgs = named.unwrap_or_else(|| vec!["libfoo".into(), "libbar".into()]);
                return Some((pkgs.iter().map(|p| format!("Inst {p} [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])\n")).collect(), 0));
            }
            cmd.contains("apt list --upgradable").then(|| ("--- upgradable\n--- held\n--- reboot\n--- unattended\n--- checked\n".to_string(), 0))
        })
    }

    /// Each test has its own node and its own SparkBox.
    async fn setup(name: &str, delay: &str) -> (Arc<Ops>, Node) {
        let (node, port) = fake_node(&temp(&format!("upg-{name}-home")), apt()).await;
        fake::pin_aware(&node);
        std::fs::create_dir_all(node.state_dir.join("apt")).unwrap();
        std::fs::write(node.state_dir.join("apt/delay"), delay).unwrap();
        let id = format!("upg-{name}");
        let (live, _) = connect(&temp(&format!("upg-{name}-app")), &[(id.as_str(), port)]).await;
        let demo = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
        (Ops::with_poll(live, demo, Tasks::new(None), Duration::from_millis(5)), node)
    }

    async fn wait_task(ops: &Ops, id: &str, secs: u64) -> Task {
        for _ in 0..secs * 50 {
            let t = ops.tasks.get(id).unwrap();
            if t.finished() {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("still running: {:?}", ops.tasks.get(id));
    }

    async fn wait_phase(ops: &Ops, node: &str, phase: &str) {
        for _ in 0..500 {
            if ops.runs.lock().unwrap().get(node).is_some_and(|r| r.phase == phase) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never {phase}: {:?}", ops.runs.lock().unwrap().get(node).map(|r| (&r.phase, &r.log)));
    }

    /// What the Updates window does: the dry run (C2) first, then Upgrade bound to the plan it
    /// showed. Security only dry-runs the fixes it takes.
    async fn upgrade_as_shown(ops: &Arc<Ops>, node: &str, packages: Vec<String>, args: UpgradeArgs) -> String {
        let shown = if args.security_only { gpu_guard::security_only(&ops.updates_status(false, node).await.unwrap().pending) } else { packages.clone() };
        let sim = ops.updates_simulate_opts(false, node, &shown, &UpgradeOpts { dl_limit_kbps: args.dl_limit_kbps, ..Default::default() }).await.unwrap();
        ops.updates_upgrade_opts(false, node.into(), packages, UpgradeArgs { plan: Some(sim.shown()), ..args }).unwrap()
    }

    async fn upgrade(ops: &Arc<Ops>, node: &str, packages: Vec<String>) -> String {
        upgrade_as_shown(ops, node, packages, UpgradeArgs::default()).await
    }

    /// The saved runs (upgrades.json) of this SparkBox's data folder, by node: their phases.
    fn saved_phases(ops: &Ops) -> BTreeMap<String, String> {
        let saved: Vec<UpgradeRun> = serde_json::from_slice(&std::fs::read(saved_path(ops)).unwrap_or_default()).unwrap_or_default();
        saved.into_iter().map(|r| (r.node, r.phase)).collect()
    }

    /// The task's line while apt stops says what the window says, not the raw state.
    #[test]
    fn a_stopped_run_is_described_in_words() {
        assert_eq!(describe(&UpgradeRun { phase: "cancelled".into(), ..Default::default() }), "Stopped");
    }

    #[test]
    fn apt_output_gives_the_phase_and_package_n_of_m() {
        let lines = |t: &str| t.lines().map(String::from).collect::<Vec<_>>();
        assert_eq!(progress_of(&lines("Reading package lists...\nGet:1 http://x libfoo")), ("downloading", 0, None, true));
        let mid = lines("Fetched 2 kB\n(Reading database ... 5 files)\nPreparing to unpack .../libfoo.deb ...\nUnpacking libfoo (2.0-1) over (1.0-1) ...\nUnpacking libbar:arm64 (2.0-1) over (1.0-1) ...");
        assert_eq!(progress_of(&mid), ("unpacking", 2, Some("libbar".into()), false), "dpkg is running: no stopping now");
        let late = lines("Unpacking libfoo (2) over (1) ...\nUnpacking libbar (2) over (1) ...\nSetting up libfoo (2) ...");
        assert_eq!(progress_of(&late).0, "configuring");
        let run = UpgradeRun { phase: "configuring".into(), done: 1, total: 2, current: Some("libfoo".into()), ..Default::default() };
        assert_eq!(describe(&run), "Setting up 1 of 2 · libfoo");
        assert_eq!(exit_of(&lines("x\nSPARKBOX-EXIT 100")), Some(100));
        // From SPARKBOX-AFTER on apt is done: apt-get check's "Reading package lists" isn't a download.
        let after = lines("Setting up libfoo (2) ...\nSPARKBOX-AFTER\nReading package lists... Done\nNEEDRESTART-KSTA: 1\nSPARKBOX-AFTER-END");
        assert_eq!(progress_of(&after), ("finishing", 1, Some("libfoo".into()), false));
        // Whether apt ended before dpkg ran.
        assert!(before_dpkg(&lines("Reading package lists...\nGet:1 http://x libfoo\nE: Failed to fetch\nSPARKBOX-AFTER\nSPARKBOX-AFTER-END\nSPARKBOX-EXIT 100")));
        assert!(!before_dpkg(&lines("Fetched 2 kB\n(Reading database ... 5 files)\nSPARKBOX-AFTER\nSPARKBOX-EXIT 100")), "dpkg had started");
        assert!(!before_dpkg(&lines("Unpacking libfoo (2) over (1) ...\nSPARKBOX-EXIT 143")));
    }

    /// Contracts C1′, C2 and C2′, byte for byte: the detached upgrade of pinned versions (with and
    /// without the download limit), the discovery dry runs (one package, everything) and the dry
    /// run of exactly the pinned command; the head line and the hold command (C4).
    #[test]
    fn the_upgrade_commands_are_the_contracts_literals() {
        let pinned = ["libfoo=1.2-3".to_string()];
        let (unit, dir, log) = ("sparkbox-upgrade-t1", "/var/log/sparkbox", "/var/log/sparkbox/upgrade-t1.log");
        assert_eq!(
            detached_command(unit, dir, log, &pinned),
            r#"mkdir -p '/var/log/sparkbox' && chmod 0755 '/var/log/sparkbox' && dpkg-query -W > '/var/log/sparkbox/dpkg-before-t1.txt' && systemd-run --unit sparkbox-upgrade-t1 --collect -p StandardOutput=append:/var/log/sparkbox/upgrade-t1.log -p StandardError=append:/var/log/sparkbox/upgrade-t1.log /bin/sh -c 'trap : TERM; env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y --no-remove install --only-upgrade '\''libfoo=1.2-3'\''; code=$?; echo SPARKBOX-AFTER; apt-get check; needrestart -b -r l; echo SPARKBOX-AFTER-END; echo "SPARKBOX-EXIT $code"; exit $code'"#
        );
        let limited = UpgradeOpts { dl_limit_kbps: Some(500), ..Default::default() };
        assert_eq!(
            detached_command_opts(unit, dir, log, &pinned, &limited),
            r#"mkdir -p '/var/log/sparkbox' && chmod 0755 '/var/log/sparkbox' && dpkg-query -W > '/var/log/sparkbox/dpkg-before-t1.txt' && systemd-run --unit sparkbox-upgrade-t1 --collect -p StandardOutput=append:/var/log/sparkbox/upgrade-t1.log -p StandardError=append:/var/log/sparkbox/upgrade-t1.log /bin/sh -c 'trap : TERM; env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500 -y --no-remove install --only-upgrade '\''libfoo=1.2-3'\''; code=$?; echo SPARKBOX-AFTER; apt-get check; needrestart -b -r l; echo SPARKBOX-AFTER-END; echo "SPARKBOX-EXIT $code"; exit $code'"#
        );
        assert_eq!(detached_command(unit, dir, log, &pinned), detached_command_opts(unit, dir, log, &pinned, &UpgradeOpts::default()));
        assert!(!detached_command(unit, dir, log, &pinned).contains("dist-upgrade"), "no dist-upgrade run any more");
        // C2, the discovery dry run: unchanged.
        let one = ["libfoo".to_string()];
        assert_eq!(
            simulate_command(&one),
            "apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y install --only-upgrade 'libfoo'"
        );
        assert_eq!(
            simulate_command(&[]),
            "apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y dist-upgrade"
        );
        assert_eq!(
            simulate_command_opts(&one, &limited),
            "apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500 -y install --only-upgrade 'libfoo'"
        );
        // C2′: the dry run of exactly the pinned command.
        assert_eq!(
            pinned_dry_run_opts(&pinned, &UpgradeOpts::default()),
            "apt-get -s -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y --no-remove install --only-upgrade 'libfoo=1.2-3'"
        );
        assert_eq!(pinned_dry_run_opts(&pinned, &limited), format!("apt-get -s {}", apt_command_opts(&pinned, &limited).split_once(" apt-get ").unwrap().1));
        // The head line is the apt-get inside the unit, without the trap and the checks after it.
        assert_eq!(
            apt_command(&pinned),
            "env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get -o DPkg::Lock::Timeout=300 -o APT::Keep-Downloaded-Packages=true -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -y --no-remove install --only-upgrade 'libfoo=1.2-3'"
        );
        assert_eq!(head_command(&pinned), apt_command(&pinned));
        assert_eq!(dpkg_before_path(dir, unit), "/var/log/sparkbox/dpkg-before-t1.txt");
        assert_eq!(hold_command(&["a".into(), "b".into()], true), "apt-mark hold 'a' 'b'");
        assert_eq!(hold_command(&["a".into()], false), "apt-mark unhold 'a'");
        assert_eq!(PRESET_READ, r"dpkg-query -W -f='${Package}\n'");
    }

    #[test]
    fn the_post_checks_read_needrestart_and_apt_get_check() {
        let lines = |t: &str| t.lines().map(String::from).collect::<Vec<_>>();
        let log = lines(
            "Setting up libbrotli1:arm64 (1.1.0-2ubuntu0.1) ...
SPARKBOX-AFTER
Reading package lists... Done
Building dependency tree... Done
Reading state information... Done
NEEDRESTART-VER: 3.6
NEEDRESTART-KCUR: 7.0.0-1019-nvidia
NEEDRESTART-KEXP: 7.0.0-1020-nvidia
NEEDRESTART-KSTA: 3
NEEDRESTART-SVC: nginx.service
NEEDRESTART-SVC: ollama.service
SPARKBOX-AFTER-END
SPARKBOX-EXIT 0",
        );
        let (check, nr) = post_checks(&log);
        assert_eq!(check.as_deref(), Some("Reading package lists... Done\nBuilding dependency tree... Done\nReading state information... Done"));
        let nr = nr.unwrap();
        assert_eq!(
            nr,
            NeedRestart { kernel_status: Some(3), kernel_current: Some("7.0.0-1019-nvidia".into()), kernel_expected: Some("7.0.0-1020-nvidia".into()), services: vec!["nginx.service".into(), "ollama.service".into()] }
        );
        assert_eq!(services_sentence(&nr.services), "2 services still use old libraries: nginx.service, ollama.service");
        assert_eq!(services_sentence(&nr.services[..1]), "1 service still uses old libraries: nginx.service");
        assert_eq!(kernel_sentence(&nr).as_deref(), Some("A newer kernel is installed (7.0.0-1020-nvidia); it runs after the next restart"));
        assert_eq!(kernel_sentence(&NeedRestart { kernel_status: Some(1), ..Default::default() }), None);
        // needrestart missing on the node: the check's output only, no listing.
        let (check, nr) = post_checks(&lines("SPARKBOX-AFTER\nReading package lists... Done\n/bin/sh: 1: needrestart: not found\nSPARKBOX-AFTER-END\nSPARKBOX-EXIT 0"));
        assert_eq!((check.as_deref(), nr), (Some("Reading package lists... Done\n/bin/sh: 1: needrestart: not found"), None));
        // An upgrade round 2 started has no such section.
        assert_eq!(post_checks(&lines("Setting up libfoo (2) ...\nSPARKBOX-EXIT 0")), (None, None));
    }

    #[test]
    fn the_dry_run_lists_phased_upgrades_and_origins_are_folded() {
        let sim = parse_simulation(
            "cell2",
            "Reading package lists...
The following upgrades have been deferred due to phasing:
  libsystemd0 systemd
The following packages have been kept back:
  linux-image-nvidia
Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [arm64])
Conf libbrotli1 (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [arm64])
1 upgraded, 0 newly installed, 0 to remove and 3 not upgraded.
",
        );
        assert_eq!(sim.phased, ["libsystemd0", "systemd"]);
        assert_eq!(sim.held_back, ["linux-image-nvidia"]);
        assert_eq!(sim.upgrades, ["libbrotli1 1.1.0-2build2 → 1.1.0-2ubuntu0.1"]);
        let chrome = parse_upgradable("google-chrome-stable/stable,stable 156.0.8078.73-1 arm64 [upgradable from: 156.0.8078.47-1]");
        assert_eq!(chrome[0].origin, "stable");
        let both = parse_upgradable("openssl/noble-updates,noble-security 3.0.13-0ubuntu3.6 arm64 [upgradable from: 3.0.13-0ubuntu3.5]");
        assert_eq!((both[0].origin.as_str(), both[0].security), ("noble-updates, noble-security", true));
        // What a dry run of these packages stands for.
        let judged = judge(sim, &["libbrotli1".into()], &UpgradeOpts::default());
        assert_eq!(
            (judged.command, judged.dry_run, judged.refused),
            (apt_command(&["libbrotli1=1.1.0-2ubuntu0.1".into()]), simulate_command(&["libbrotli1".into()]), None),
            "the upgrade it shows is C1′, its installed package pinned"
        );
        assert_eq!(judged.plan, [PlannedPkg { name: "libbrotli1".into(), from: Some("1.1.0-2build2".into()), to: "1.1.0-2ubuntu0.1".into(), new: false }]);
    }

    /// The plan an upgrade binds to: every Inst line (an upgrade, or a new package, never
    /// pinned), names without apt's `:arch`; removals from Remv lines or apt's "will be REMOVED"
    /// list (all a `--no-remove` dry run prints before it stops), and a plan that removes is refused.
    #[test]
    fn a_dry_run_gives_the_plan_and_what_it_removes() {
        let sim = parse_simulation(
            "cell2",
            "Inst libfoo:arm64 [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])
Inst libnew (2.0-1 Ubuntu:24.04/noble-updates [arm64])
Inst fwupd:arm64 [1.9.30-0ubuntu1~24.04.1] (1.9.31-0ubuntu1~24.04.1 Ubuntu:24.04/noble-updates [arm64])
Remv libold:arm64 [1.0-1]
",
        );
        let names: Vec<(&str, Option<&str>, &str, bool)> = sim.plan.iter().map(|p| (p.name.as_str(), p.from.as_deref(), p.to.as_str(), p.new)).collect();
        assert_eq!(names, [("libfoo", Some("1.0-1"), "2.0-1", false), ("libnew", None, "2.0-1", true), ("fwupd", Some("1.9.30-0ubuntu1~24.04.1"), "1.9.31-0ubuntu1~24.04.1", false)]);
        assert_eq!(sim.removes, ["libold"]);
        assert_eq!(pins_of(&sim.plan), ["libfoo=2.0-1", "fwupd=1.9.31-0ubuntu1~24.04.1"], "the new dependency isn't pinned");
        // fwupd:arm64 is the guard's fwupd.
        let judged = judge(sim.clone(), &[], &UpgradeOpts::default());
        assert_eq!(judged.refused.as_deref(), Some("SparkBox can't upgrade GPU-stack packages yet: fwupd"));
        assert_eq!(judged.sensitive, ["fwupd"]);
        // Without it, the removal refuses the plan (round 4's amendment).
        let routine = parse_simulation("cell2", "Inst libfoo [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])\nRemv libfoo-doc [1.0-1]\n");
        assert_eq!(
            judge(routine, &[], &UpgradeOpts::default()).refused.as_deref(),
            Some("This plan removes libfoo-doc. SparkBox's one-node upgrades only upgrade; run it in a terminal if you mean it.")
        );
        // apt's list when --no-remove stops it: purged ones end in "*".
        let stopped = parse_simulation("cell2", "Reading package lists...\nThe following packages will be REMOVED:\n  docker-ce* libold\nThe following NEW packages will be installed:\n  libnew\n");
        assert_eq!((stopped.removes.clone(), stopped.plan.len(), stopped.removals.len()), (vec!["docker-ce".to_string(), "libold".into()], 0, 0));
        // The shown plan, as Upgrade sends it back.
        assert_eq!(parse_simulation("cell2", "Inst libfoo:arm64 [1.0-1] (2.0-1 x [arm64])\n").shown(), [PlanEntry { name: "libfoo".into(), to: "2.0-1".into() }]);
    }

    /// Decision (b)'s comparison: the same packages at the same versions, in any order and with or
    /// without `:arch`, is the same plan; anything else says what moved.
    #[test]
    fn the_bound_plan_is_compared_without_order_or_arch() {
        let entry = |n: &str, v: &str| PlanEntry { name: n.into(), to: v.into() };
        let now = |n: &str, v: &str, new: bool| PlannedPkg { name: n.into(), from: (!new).then(|| "1.0-1".into()), to: v.into(), new };
        let shown = [entry("libbrotli1", "1.1.0-2ubuntu0.1"), entry("libfoo:arm64", "1.2")];
        assert_eq!(plan_changed("cell2", &shown, &[now("libfoo", "1.2", true), now("libbrotli1", "1.1.0-2ubuntu0.1", false)]), None);
        assert_eq!(
            plan_changed("cell2", &shown, &[now("libbrotli1", "1.1.0-2ubuntu0.1", false), now("libfoo", "1.3", true), now("libbar", "2.0", true)]).as_deref(),
            Some("apt's plan for cell2 changed since the dry run you saw: libfoo now 1.3 (was 1.2); new: libbar 2.0. Look at the new dry run and start again.")
        );
        assert_eq!(
            plan_changed("cell2", &shown, &[now("libbrotli1", "1.1.0-2ubuntu0.1", false)]).as_deref(),
            Some("apt's plan for cell2 changed since the dry run you saw: no longer in it: libfoo. Look at the new dry run and start again.")
        );
        // The installed ones are pinned, by their bare names; a new dependency isn't.
        assert_eq!(pins_in(&shown, &["libbrotli1".into(), "nginx".into()]), ["libbrotli1=1.1.0-2ubuntu0.1"]);
        assert_eq!(pins_in(&[entry("libfoo:arm64", "2.0-1")], &["libfoo".into()]), ["libfoo=2.0-1"]);
        // What Upgrade sends is checked as apt gets it.
        assert!(check_plan_entries(&[entry("docker-ce", "5:29.0.0-1~ubuntu.24.04~noble")]).is_ok());
        for bad in [entry("libfoo", "1.0; reboot"), entry("libfoo", ""), entry("libfoo", "-1"), entry("lib foo", "1.0"), entry("libfoo", "$(id)")] {
            assert_eq!(check_plan_entries(std::slice::from_ref(&bad)).unwrap_err().kind, ErrorKind::Invalid, "{bad:?}");
        }
    }

    #[tokio::test]
    async fn an_upgrade_runs_detached_and_is_followed_to_its_end() {
        let (ops, node) = setup("ok", "0.05").await;
        let id = upgrade(&ops, "upg-ok", vec!["libfoo".into(), "libbar".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
        let started = node.ran("systemd-run --unit sparkbox-upgrade-");
        assert_eq!(started.len(), 1);
        assert!(started[0].starts_with("sudo -k -S -p '' mkdir -p ") && started[0].contains(" --collect -p StandardOutput=append:"), "{}", started[0]);
        let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
        assert_eq!((run.phase.as_str(), run.done, run.total, run.exit_code), ("done", 2, 2, Some(0)));
        assert!(run.verification.as_ref().is_some_and(|v| v.checked_ms >= run.started_ms && v.remaining.is_empty()));
        assert!(t.summary.as_deref().unwrap().contains("no package updates remain"));
        assert!(run.log.iter().any(|l| l == "Setting up libbar (2.0-1) ..."), "{:?}", run.log);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\nlibbar\n");
        assert!(!node.ran("dpkg --audit").is_empty(), "checked afterwards");
        assert!(!node.state_dir.join(format!("units/{}.service", run.unit)).exists(), "--collect: the unit is gone");
        assert!(node.ran("tail -n +1 -F ").len() == 1, "followed over the bulk connection");
    }

    /// `pgrep -x dpkg` would see every process on this Mac (the fake nodes' "node"); the fake's
    /// pgrep counts only the node's own, and the tests that cancel take turns as well.
    static PGREP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn cancel_stops_apt_while_it_downloads_but_not_once_dpkg_runs() {
        let _turn = PGREP.lock().await;
        // apt-get waits 4 s after downloading: room for Cancel to arrive over SSH on a loaded machine.
        let (ops, node) = setup("cancel", "4").await;
        // A dpkg somewhere else on this Mac (another test run's stand-in): not this node's.
        let stray = temp("upg-cancel-stray").join("dpkg");
        std::os::unix::fs::symlink("/bin/sleep", &stray).unwrap();
        let mut stray = std::process::Command::new(&stray).arg("30").spawn().unwrap();
        let id = upgrade(&ops, "upg-cancel", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-cancel", "downloading").await;
        assert_eq!(ops.updates_cancel("upg-cancel").unwrap(), "Stopping apt before it installs anything");
        let t = wait_task(&ops, &id, 30).await;
        let _ = stray.kill();
        let _ = stray.wait();
        assert_eq!(t.state, TaskState::Cancelled, "{:?} {:?} {:?}", t.error, t.log, node.ran("pkill"));
        assert!(!node.state_dir.join("apt/unpacked").exists(), "nothing was installed");
        assert!(!node.ran("pkill -TERM -P").is_empty(), "only apt-get is signalled");
        assert!(node.ran("systemctl kill").is_empty(), "never the whole unit");
        let logged = ops.live.audit.entries().into_iter().find(|e| e.command.contains("pkill -TERM -P")).expect("logged");
        assert!(logged.command.contains("pgrep -x dpkg"), "the log has the command that ran: {}", logged.command);
        let head = ops.live.audit.entries().into_iter().find(|e| e.kind == "op" && e.command.starts_with("env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get")).expect("a head line");
        assert_eq!(head.command, apt_command(&["libfoo=2.0-1".into(), "libbar=2.0-1".into()]), "the apt-get that ran, not a stand-in");

        // Once dpkg runs, Cancel says so and the upgrade finishes.
        let (ops, node) = setup("nocancel", "0.4").await;
        let id = upgrade(&ops, "upg-nocancel", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-nocancel", "unpacking").await;
        let refused = ops.updates_cancel("upg-nocancel").unwrap_err();
        assert!(refused.message.starts_with("Can't stop safely now"), "{}", refused.message);
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\nlibbar\n");
    }

    /// The log says it's still downloading, but dpkg already runs on the node: the node's own
    /// check wins, apt isn't stopped, and the upgrade finishes.
    #[tokio::test]
    async fn cancel_checks_for_dpkg_on_the_node_itself() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("dpkgrace", "2").await;
        // A process called dpkg, as the node would have mid-install (a link: macOS kills a copied system binary).
        let fake_dpkg = node.home.join("dpkg");
        std::os::unix::fs::symlink("/bin/sleep", &fake_dpkg).unwrap();
        let mut dpkg = std::process::Command::new(&fake_dpkg).arg("30").spawn().unwrap();
        let id = upgrade(&ops, "upg-dpkgrace", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-dpkgrace", "downloading").await;
        ops.updates_cancel("upg-dpkgrace").unwrap();
        let t = wait_task(&ops, &id, 30).await;
        let _ = dpkg.kill();
        assert_eq!(t.state, TaskState::Ok, "not stopped: {:?} {:?}", t.error, t.log);
        assert!(t.log.iter().any(|l| l.starts_with("Can't stop safely now")), "{:?}", t.log);
        assert!(node.ran("pkill -TERM -P").iter().all(|c| c.contains("pgrep -x dpkg")));
    }

    /// Look-only comes on while apt runs in its unit: the upgrade isn't counted as stopped, its
    /// task says apt keeps running on the node, a Cancel that reaches it anyway is refused at the
    /// write exit with its own words (nothing reaches the node), and apt finishes there.
    #[tokio::test]
    async fn look_only_says_a_running_upgrade_keeps_running_on_its_node() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("lookonly", "4").await;
        let (gate, _, _) = crate::admin::testing::gate();
        ops.live.set_write_policy(gate.clone());
        let hook = ops.tasks.clone();
        gate.on_hold(move |why| hook.hold_non_read(why));
        let id = upgrade(&ops, "upg-lookonly", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-lookonly", "downloading").await;

        let v = gate.set_look_only(true).unwrap();
        assert_eq!(v.held, crate::admin::HeldTasks { stopped: 0, stopping: 0, on_node: 1 });
        let t = ops.tasks.get(&id).unwrap();
        assert_eq!(t.held.as_deref(), Some("Look-only turned on: what it started on upg-lookonly keeps running; SparkBox can't stop it in look-only"));
        assert!(!t.cancel_requested, "not cancelled by the hold");
        // updates_cancel is refused before it runs in look-only; this is the task's own Cancel path.
        ops.tasks.cancel(&id).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert!(t.log.iter().any(|l| l == "Look-only refused Cancel; the upgrade keeps running"), "{:?}", t.log);
        assert!(!t.log.iter().any(|l| l.starts_with("Can't stop safely now")), "not the dpkg reason: {:?}", t.log);
        assert!(node.ran("pkill").is_empty(), "the stop never reached the node");
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("2 packages installed on upg-lookonly. Package check passed: no package updates remain")), "{:?} {:?}", t.error, t.log);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\nlibbar\n", "apt finished on the node");
    }

    #[tokio::test]
    async fn an_upgrade_is_picked_up_again_after_a_restart() {
        let (ops, node) = setup("reattach", "0.3").await;
        let dir = ops.live.upgrade_log_dir();
        let run = UpgradeRun {
            node: "upg-reattach".into(),
            title: "Upgrade upg-reattach".into(),
            unit: "sparkbox-upgrade-old1".into(),
            log_path: format!("{dir}/upgrade-old1.log"),
            packages: vec!["libfoo".into()],
            total: 1,
            phase: "unpacking".into(),
            ..Default::default()
        };
        // What an earlier SparkBox started before it quit.
        let out = ops.live.sudo("upg-reattach", &detached_command(&run.unit, &dir, &run.log_path, &run.packages), Duration::from_secs(20)).await.unwrap();
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        crate::util::save_json(&saved_path(&ops), &vec![run]).unwrap();
        ops.runs.lock().unwrap().remove("upg-reattach");
        let window = Window::on(&ops, &node);
        ops.reattach_upgrades();
        let mut task = None;
        for _ in 0..200 {
            task = ops.tasks.list().into_iter().find(|t| t.title.ends_with("(picked up again)"));
            if task.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let t = wait_task(&ops, &task.expect("a task for the reattached upgrade").id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\n");
        // The earlier SparkBox never copied the package list: the reattach does.
        let back = ops.runs().into_iter().find(|r| r.task_id == t.id).unwrap();
        let copy = back.evidence.clone().expect("the evidence copy");
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "libbar\t1.0-1\nlibfoo\t1.0-1\n");
        assert_eq!(back.dpkg_before.as_deref(), Some(format!("{dir}/dpkg-before-old1.txt").as_str()));
        assert!(t.log.iter().any(|l| l.starts_with(&format!("Package list before the upgrade: {dir}/dpkg-before-old1.txt (also in the evidence folder: "))), "{:?}", t.log);
        assert_eq!(node.ran("cat '").len(), 1);
        // Its maintenance window opens again for the rest of the run.
        // It ends as a first run does: dpkg --audit before the window closes.
        assert_eq!(window.said(), ["begin upg-reattach upgrading 30 min, after the start", "agents told about upg-reattach", "end upg-reattach, after dpkg --audit"]);
    }

    /// The task for a run picked up again, once `reattach_upgrades` has made it.
    async fn picked_up(ops: &Ops) -> Task {
        let mut task = None;
        for _ in 0..300 {
            task = ops.tasks.list().into_iter().find(|t| t.title.ends_with("(picked up again)"));
            if task.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        wait_task(ops, &task.expect("a task for the reattached upgrade").id, 30).await
    }

    /// What an earlier SparkBox recorded for an upgrade of libfoo on `node` (unit `id`), saved
    /// as it saves its runs; with `start`, the unit really runs on the node.
    async fn earlier_run(ops: &Ops, node: &str, id: &str, phase: &str, start: bool) -> UpgradeRun {
        let dir = ops.live.upgrade_log_dir();
        let run = UpgradeRun {
            node: node.into(),
            title: format!("Upgrade {node}"),
            unit: format!("sparkbox-upgrade-{id}"),
            log_path: format!("{dir}/upgrade-{id}.log"),
            packages: vec!["libfoo".into()],
            total: 1,
            phase: phase.into(),
            ..Default::default()
        };
        if start {
            let out = ops.live.sudo(node, &detached_command(&run.unit, &dir, &run.log_path, &run.packages), Duration::from_secs(20)).await.unwrap();
            assert_eq!(out.code, Some(0), "{}", out.stderr);
        }
        crate::util::save_json(&saved_path(ops), &vec![run.clone()]).unwrap();
        ops.runs.lock().unwrap().remove(node);
        run
    }

    /// A run picked up again ends as a first run does: `apt-get check`'s E: lines (in the unit's
    /// log) fail it, and `dpkg --audit` runs before its window closes.
    #[tokio::test]
    async fn a_picked_up_upgrade_with_broken_dependencies_fails_and_is_audited() {
        let (ops, node) = setup("reattach-check", "0.05").await;
        let shim = std::fs::read_to_string(node.bin_dir.join("shim/apt-get")).unwrap();
        let broken = shim.replace(
            r#"check) echo "Reading package lists... Done"; echo "Building dependency tree... Done"; echo "Reading state information... Done"; exit 0 ;;"#,
            r#"check) echo "Reading package lists... Done"; echo "E: Unmet dependencies. Try 'apt --fix-broken install' with no packages (or specify a solution)."; exit 100 ;;"#,
        );
        assert_ne!(shim, broken);
        node.bin("shim/apt-get", &broken);
        let mut saved = earlier_run(&ops, "upg-reattach-check", "old2", "unpacking", true).await;
        // A saved result cannot stand in for this follower's checks, which may fail before apt status.
        saved.verification = Some(UpgradeVerification { checked_ms: 1, remaining: vec![] });
        crate::util::save_json(&saved_path(&ops), &vec![saved]).unwrap();
        let window = Window::on(&ops, &node);
        ops.reattach_upgrades();
        let t = picked_up(&ops).await;
        assert_eq!(t.state, TaskState::Failed, "{:?} {:?}", t.summary, t.steps);
        assert!(ops.runs().iter().find(|r| r.node == "upg-reattach-check").unwrap().verification.is_none(), "old check must not survive a failed recheck");
        assert_eq!(
            t.error.as_deref(),
            Some("The upgrade finished, but apt-get check reports broken dependencies on upg-reattach-check: E: Unmet dependencies. Try 'apt --fix-broken install' with no packages (or specify a solution).")
        );
        assert_eq!(node.ran("dpkg --audit"), ["dpkg --audit"], "audited, as the cluster user");
        assert_eq!(window.said().last().map(String::as_str), Some("end upg-reattach-check, after dpkg --audit"));
        assert_eq!(ops.runs().into_iter().find(|r| r.task_id == t.id).map(|r| r.phase), Some("failed".to_string()));
    }

    /// A run picked up again on a node that now needs a restart says so, as a first run does.
    #[tokio::test]
    async fn a_picked_up_upgrade_says_when_the_node_needs_a_restart() {
        let script: Script = Arc::new(|cmd, _| {
            cmd.contains("apt list --upgradable").then(|| ("--- upgradable\n--- held\n--- reboot\n*** System restart required ***\nlibfoo\n--- unattended\n--- checked\n".to_string(), 0))
        });
        let (ops, _node, _) = setup_with("reattach-reboot", script).await;
        std::fs::write(_node.state_dir.join("apt/delay"), "0.05").unwrap();
        earlier_run(&ops, "upg-reattach-reboot", "old3", "downloading", true).await;
        ops.reattach_upgrades();
        let t = picked_up(&ops).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
        assert_eq!(t.summary.as_deref(), Some("The upgrade on upg-reattach-reboot finished. Restart it to finish (libfoo). Package check passed: no package updates remain"));
    }

    /// SparkBox quit while it was starting an upgrade (the record says "starting"): the next one
    /// looks for the unit on the node. Found, it's followed to its end; not there, the task says
    /// it never started and no record is kept.
    #[tokio::test]
    async fn a_run_saved_while_starting_is_looked_for_on_the_node() {
        let (ops, node) = setup("reattach-start", "0.05").await;
        earlier_run(&ops, "upg-reattach-start", "old4", "starting", false).await;
        ops.reattach_upgrades();
        let t = picked_up(&ops).await;
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("The upgrade on upg-reattach-start never started (SparkBox stopped while starting it); apt didn't run there.")));
        assert!(ops.runs().iter().all(|r| r.node != "upg-reattach-start"), "no record of a run that never was");
        assert!(node.ran("dpkg --audit").is_empty() && !node.state_dir.join("apt/upgraded").exists());
        // (The runs are the process's: other tests' running upgrades are saved here too.)
        let saved: Vec<UpgradeRun> = serde_json::from_slice(&std::fs::read(saved_path(&ops)).unwrap()).unwrap();
        assert!(saved.iter().all(|r| r.node != "upg-reattach-start"), "{saved:?}");

        let (ops, node) = setup("reattach-started", "0.05").await;
        earlier_run(&ops, "upg-reattach-started", "old5", "starting", true).await;
        ops.reattach_upgrades();
        let t = picked_up(&ops).await;
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("The upgrade on upg-reattach-started finished. Package check passed: no package updates remain")), "{:?} {:?}", t.error, t.log);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\n");
    }

    /// The start command's reply is lost (its channel closes with no exit status) after the
    /// unit started: SparkBox finds the unit and follows it to its end. When nothing started
    /// (the command never got to systemd-run), the task says so and no record is left.
    #[tokio::test]
    async fn a_lost_reply_to_the_start_is_looked_for_on_the_node() {
        let (ops, node) = setup("lost", "0.2").await;
        node.close_without_status("systemd-run --unit sparkbox-upgrade-");
        let window = Window::on(&ops, &node);
        let id = upgrade(&ops, "upg-lost", vec!["libfoo".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("1 package installed on upg-lost. Package check passed: no package updates remain")), "{:?} {:?}", t.error, t.log);
        assert!(t.log.iter().any(|l| l == "The start command's reply was lost (no exit status); checking whether the upgrade started on upg-lost…"), "{:?}", t.log);
        assert_eq!(node.ran("systemd-run --unit sparkbox-upgrade-").len(), 1, "started once");
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\n");
        assert_eq!(ops.runs().into_iter().find(|r| r.node == "upg-lost").map(|r| (r.phase, r.task_id)), Some(("done".to_string(), id)));
        assert_eq!(window.said().last().map(String::as_str), Some("end upg-lost, after dpkg --audit"));

        let (ops, node) = setup("lost-none", "0.05").await;
        // The start's chain stops before the unit (as if systemd-run never ran), and its reply is lost.
        node.bin("systemd-run", "exit 0");
        node.close_without_status("systemd-run --unit sparkbox-upgrade-");
        let id = upgrade(&ops, "upg-lost-none", vec!["libfoo".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(
            (t.state, t.error.as_deref()),
            (TaskState::Failed, Some("Couldn't start the upgrade on upg-lost-none: the start command's reply was lost, and no upgrade is running there."))
        );
        assert!(ops.runs().iter().all(|r| r.node != "upg-lost-none"), "no record of a run that never was");
        assert!(!node.state_dir.join("apt/upgraded").exists() && node.ran("dpkg --audit").is_empty());
        // It looked three times, reads only.
        assert_eq!(node.ran("systemctl show -p ActiveState,Result,LoadState -- 'sparkbox-upgrade-").len(), 3);
        assert!(node.ran("sudo").iter().all(|c| c.contains("systemd-run --unit sparkbox-upgrade-")), "{:?}", node.ran("sudo"));
    }

    /// Two upgrades of one node sent back to back (Retry, a second window, Upgrade then security
    /// only): the second is refused at once, while the first is still in its dry run, and one
    /// unit starts. The node's last run is kept when an upgrade is refused or finds nothing to do.
    #[tokio::test]
    async fn one_upgrade_at_a_time_on_a_node() {
        let (ops, node) = setup("two", "0.3").await;
        let shown = |sim: SimulatedUpgrade| UpgradeArgs { plan: Some(sim.shown()), ..Default::default() };
        let foo = ops.updates_simulate(false, "upg-two", &["libfoo".into()]).await.unwrap();
        let bar = ops.updates_simulate(false, "upg-two", &["libbar".into()]).await.unwrap();
        let first = ops.updates_upgrade_opts(false, "upg-two".into(), vec!["libfoo".into()], shown(foo)).unwrap();
        let second = ops.updates_upgrade_opts(false, "upg-two".into(), vec!["libbar".into()], shown(bar.clone())).unwrap_err();
        assert_eq!((second.kind, second.message.as_str()), (ErrorKind::Invalid, "An upgrade is already running on upg-two."));
        let also = ops.updates_upgrade_opts(false, "upg-two".into(), vec![], UpgradeArgs { security_only: true, ..shown(bar) }).unwrap_err();
        assert_eq!(also.message, "An upgrade is already running on upg-two.");
        let t = wait_task(&ops, &first, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
        assert_eq!(node.ran("systemd-run --unit sparkbox-upgrade-").len(), 1);
        let pinned_dry_runs = node.ran("apt-get -s ").into_iter().filter(|c| c.contains(" --no-remove ")).count();
        assert_eq!(pinned_dry_runs, 1, "the second never got to its dry run");
        // Over: the next one may start. A GPU-stack plan is refused, and the last run stays.
        let done = ops.runs().into_iter().find(|r| r.node == "upg-two").unwrap();
        assert_eq!((done.phase.as_str(), done.task_id.as_str()), ("done", first.as_str()));
        let refused = ops.updates_upgrade_opts(false, "upg-two".into(), vec!["linux-image-7.0.0-1020-nvidia".into()], UpgradeArgs { plan: Some(vec![]), ..Default::default() }).unwrap_err();
        assert!(refused.message.starts_with("SparkBox can't upgrade GPU-stack packages yet"));
        assert_eq!(ops.runs().into_iter().find(|r| r.node == "upg-two"), Some(done.clone()));
    }

    /// Two SparkBoxes in one process (a parallel test's, a second window of the rehearsals'): each
    /// sees and saves only its own runs. With the records shared, the second one saved the first
    /// one's run to its own upgrades.json, its next start put that old record back over the run's
    /// end, and the first one's next upgrade read "An upgrade is already running".
    #[tokio::test]
    async fn two_sparkboxes_in_one_process_keep_their_own_runs() {
        let (ops, _node) = setup("own", "0.2").await;
        let (other, _other_node) = setup("own-other", "0").await;
        let first = upgrade(&ops, "upg-own", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-own", "downloading").await;
        assert_eq!(ops.runs().iter().map(|r| r.node.as_str()).collect::<Vec<_>>(), ["upg-own"]);
        assert!(other.runs().is_empty(), "{:?}", other.runs());
        save_running(&other);
        assert!(saved_phases(&other).is_empty(), "the other's file holds only its own runs");
        assert!(saved_phases(&ops).contains_key("upg-own"), "its own file holds it");

        let t = wait_task(&ops, &first, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
        let again = Ops::with_poll(other.live.clone(), Arc::new(Mutex::new(DemoCluster::new(&Inventory::example()))), Tasks::new(None), Duration::from_millis(5));
        again.reattach_upgrades();
        assert!(again.runs().is_empty(), "{:?}", again.runs());

        let sim = ops.updates_simulate(false, "upg-own", &["libbar".into()]).await.unwrap();
        let second = ops.updates_upgrade_opts(false, "upg-own".into(), vec!["libbar".into()], UpgradeArgs { plan: Some(sim.shown()), ..Default::default() });
        let second = second.map_err(|e| e.message).expect("its run ended: the next upgrade may start");
        assert_eq!(wait_task(&ops, &second, 30).await.state, TaskState::Ok);
    }

    /// No upgrade starts while SparkBox restarts the node, has a restart or shutdown scheduled,
    /// or is shutting it down: a reboot mid-dpkg leaves packages half-configured.
    #[tokio::test]
    async fn no_upgrade_starts_on_a_node_sparkbox_is_restarting() {
        let (ops, node) = setup("declared", "0.05").await;
        for (state, said) in [
            ("rebooting", "upg-declared is restarting; upgrade it once it's back."),
            ("restart-checking", "upg-declared is restarting; upgrade it once it's back."),
            ("restart-scheduled", "upg-declared has a restart scheduled; upgrade it once it's back."),
            ("shutdown-scheduled", "upg-declared has a shutdown scheduled."),
            ("shutting-down", "upg-declared is shutting down."),
        ] {
            ops.declare(false, "upg-declared", state);
            let e = ops.updates_upgrade_opts(false, "upg-declared".into(), vec!["libfoo".into()], UpgradeArgs { plan: Some(vec![PlanEntry { name: "libfoo".into(), to: "2.0-1".into() }]), ..Default::default() }).unwrap_err();
            assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Invalid, said), "{state}");
        }
        assert!(node.ran("apt-get").is_empty() && ops.tasks.list().is_empty(), "nothing ran, no task");
        // Demo's declared states don't hold Live, and once it's back the upgrade runs.
        ops.declare(true, "upg-declared", "rebooting");
        let id = upgrade(&ops, "upg-declared", vec!["libfoo".into()]).await;
        assert_eq!(wait_task(&ops, &id, 30).await.state, TaskState::Ok);
        ops.undeclare("upg-declared");
    }

    /// "Upgrade everything" binds to the dry run of dist-upgrade it showed: its installed packages
    /// pinned, so a kernel the lists brought after the dry run isn't in the run. It still needs the
    /// GPU stack pinned (round 3's amendment, kept): unpinned, it's refused before anything runs
    /// as root, in the dry run and in the task.
    #[tokio::test]
    async fn everything_runs_only_with_the_gpu_stack_pinned() {
        // The dry run was made before apt-daily refreshed the lists: it shows libfoo only.
        let script: Script = Arc::new(|cmd, _| {
            if cmd.starts_with("apt-get -s ") {
                return Some(("Inst libfoo [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])\n".into(), 0));
            }
            cmd.contains("apt list --upgradable").then(|| ("--- upgradable\n--- held\n--- reboot\n--- unattended\n--- checked\n".to_string(), 0))
        });
        let (ops, node, _) = setup_with("pinfirst", script).await;
        std::fs::write(node.state_dir.join("apt/installed"), "docker-ce\t5:29.0.0-1~ubuntu.24.04~noble\nfwupd\t1.9.30-0ubuntu1~24.04.1\nlibfoo\t1.0-1\nlinux-image-nvidia-hwe-24.04\t7.0.0-1019.19\nnvidia-driver-580-open\t580.178.04-0ubuntu1\n").unwrap();
        std::fs::write(node.state_dir.join("apt/pending"), "libfoo\nlinux-image-nvidia-hwe-24.04\n").unwrap();
        let why = "Upgrade everything needs the GPU stack pinned first: docker-ce, linux-image-nvidia-hwe-24.04, nvidia-driver-580-open aren't held on upg-pinfirst";
        let sim = ops.updates_simulate(false, "upg-pinfirst", &[]).await.unwrap();
        assert_eq!((sim.refused.as_deref(), sim.sensitive.len()), (Some(why), 0), "the dry run itself has nothing sensitive");
        let id = ops.updates_upgrade_opts(false, "upg-pinfirst".into(), vec![], UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(why)));
        assert!(node.ran("sudo").is_empty(), "nothing ran as root");
        // A selection needs no pin.
        assert_eq!(ops.updates_simulate(false, "upg-pinfirst", &["libfoo".into()]).await.unwrap().refused, None);

        let pinned = ops.updates_hold_preset(false, "upg-pinfirst", true, false).await.unwrap();
        assert_eq!(pinned.packages, ["docker-ce", "linux-image-nvidia-hwe-24.04", "nvidia-driver-580-open"], "fwupd isn't held");
        let id = upgrade(&ops, "upg-pinfirst", vec![]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("1 package installed on upg-pinfirst. Package check passed: no package updates remain")), "{:?} {:?}", t.error, t.steps);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\n", "only what was shown");
        let started = node.ran("systemd-run --unit sparkbox-upgrade-");
        assert!(started[0].contains(r#" -y --no-remove install --only-upgrade '\''libfoo=2.0-1'\''; code=$?"#) && !started[0].contains("dist-upgrade"), "{}", started[0]);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/pending")).unwrap(), "linux-image-nvidia-hwe-24.04\n", "still pending");
    }

    /// R11a: a plan that leaves out a selected package is another selection's dry run. Upgrade would
    /// say one thing and install another, so it's refused at once: no task, no run, nothing sent.
    #[tokio::test]
    async fn a_plan_that_leaves_out_a_selected_package_is_refused() {
        let (ops, node) = setup("other-pick", "0").await;
        let foo = ops.updates_simulate(false, "upg-other-pick", &["libfoo".into()]).await.unwrap().shown();
        let (tasks, sent) = (ops.tasks.list().len(), node.log.lock().unwrap().len());
        let args = |plan: &[PlanEntry]| UpgradeArgs { plan: Some(plan.to_vec()), ..Default::default() };
        let e = ops.updates_upgrade_opts(false, "upg-other-pick".into(), vec!["libbar".into()], args(&foo)).unwrap_err();
        assert_eq!((e.kind, e.message.as_str(), e.node.as_deref()), (ErrorKind::Invalid, OTHER_SELECTION, Some("upg-other-pick")));
        assert_eq!(e.message, "That dry run was for another selection. Run it again.");
        let e = ops.updates_upgrade_opts(false, "upg-other-pick".into(), vec!["libfoo".into(), "libbar".into()], args(&foo)).unwrap_err();
        assert_eq!(e.message, OTHER_SELECTION, "every selected package, not just one");
        assert_eq!((ops.tasks.list().len(), node.log.lock().unwrap().len()), (tasks, sent));
        assert!(ops.runs().is_empty(), "no claim stayed behind: {:?}", ops.runs());
        // An arch suffix doesn't make it another package, and a plan may hold more than was picked
        // (a new dependency).
        let more = [foo.clone(), vec![PlanEntry { name: "libnew".into(), to: "2.0-1".into() }]].concat();
        assert!(covers(&more, &["libfoo:arm64".to_string()]));
        // The plan it was for still runs.
        let id = ops.updates_upgrade_opts(false, "upg-other-pick".into(), vec!["libfoo".into()], args(&foo)).unwrap();
        assert_eq!(wait_task(&ops, &id, 30).await.state, TaskState::Ok);
    }

    /// R11b: the refusal names six packages and counts the rest; the task's log has them all.
    #[tokio::test]
    async fn the_pin_refusal_names_six_and_the_log_has_them_all() {
        let name = |n: usize| (0..n).map(|i| format!("nvidia-pkg-{i}")).collect::<Vec<_>>();
        assert_eq!(needs_pin(&name(1), "cell2"), "Upgrade everything needs the GPU stack pinned first: nvidia-pkg-0 isn't held on cell2");
        assert_eq!(
            needs_pin(&name(6), "cell2"),
            "Upgrade everything needs the GPU stack pinned first: nvidia-pkg-0, nvidia-pkg-1, nvidia-pkg-2, nvidia-pkg-3, nvidia-pkg-4, nvidia-pkg-5 aren't held on cell2"
        );
        assert_eq!(
            needs_pin(&name(190), "cell2"),
            "Upgrade everything needs the GPU stack pinned first: nvidia-pkg-0, nvidia-pkg-1, nvidia-pkg-2, nvidia-pkg-3, nvidia-pkg-4, nvidia-pkg-5 and 184 more aren't held on cell2"
        );

        let script: Script = Arc::new(|cmd, _| {
            if cmd.starts_with("apt-get -s ") {
                return Some(("Inst libfoo [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])\n".into(), 0));
            }
            cmd.contains("apt list --upgradable").then(|| ("--- upgradable\n--- held\n--- reboot\n--- unattended\n--- checked\n".to_string(), 0))
        });
        let (ops, node, _) = setup_with("pinlist", script).await;
        let stack = name(8);
        let installed: String = stack.iter().map(|p| format!("{p}\t1.0-1\n")).collect();
        std::fs::write(node.state_dir.join("apt/installed"), format!("libfoo\t1.0-1\n{installed}")).unwrap();
        let sim = ops.updates_simulate(false, "upg-pinlist", &[]).await.unwrap();
        let id = ops.updates_upgrade_opts(false, "upg-pinlist".into(), vec![], UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Failed, "{:?}", t.error);
        assert!(t.error.as_deref().is_some_and(|e| e.ends_with(" and 2 more aren't held on upg-pinlist")), "{:?}", t.error);
        let listed: Vec<&str> = t.log.iter().flat_map(|l| l.split('\n')).collect();
        for p in &stack {
            assert!(listed.contains(&p.as_str()), "{p} is in the task's log: {:?}", t.log);
        }
    }

    /// R11c: a dry run with nothing to install has no upgrade command to show, and the commands
    /// that do run are unchanged.
    #[tokio::test]
    async fn an_empty_plan_has_no_upgrade_command() {
        let script: Script = Arc::new(|cmd, _| {
            if cmd.starts_with("apt-get -s ") {
                return Some(("Reading package lists... Done\n0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\n".into(), 0));
            }
            cmd.contains("apt list --upgradable").then(|| ("--- upgradable\n--- held\n--- reboot\n--- unattended\n--- checked\n".to_string(), 0))
        });
        let (ops, _node, _) = setup_with("noplan", script).await;
        let sim = ops.updates_simulate(false, "upg-noplan", &["libfoo".into()]).await.unwrap();
        assert!(sim.plan.is_empty());
        assert_eq!((sim.command.as_str(), sim.dry_run.as_str()), ("", simulate_command(&["libfoo".to_string()]).as_str()));
        let some = judge(parse_simulation("n", "Inst libfoo [1.0-1] (2.0-1 Ubuntu:24.04/noble-updates [arm64])\n"), &["libfoo".to_string()], &UpgradeOpts::default());
        assert_eq!(some.command, apt_command(&["libfoo=2.0-1".to_string()]));
    }

    /// What the alerting was told, in order: when the window opened, whether the upgrade's unit
    /// had started yet; when it ended, whether `dpkg --audit` had run.
    struct Window {
        said: Mutex<Vec<String>>,
        node: Arc<Mutex<Vec<String>>>,
    }

    impl Window {
        fn on(ops: &Ops, node: &Node) -> Arc<Window> {
            let w = Arc::new(Window { said: Mutex::new(Vec::new()), node: node.log.clone() });
            ops.set_planned(w.clone());
            w
        }

        fn said(&self) -> Vec<String> {
            self.said.lock().unwrap().clone()
        }

        fn ran(&self, what: &str) -> bool {
            self.node.lock().unwrap().iter().any(|c| c.contains(what))
        }
    }

    impl crate::ops::Planned for Window {
        fn begin(&self, node: &str, until_ms: Option<u64>, reason: &str) {
            let minutes = until_ms.map(|u| (u.saturating_sub(now_ms()) + 30_000) / 60_000).unwrap_or(0);
            let started = if self.ran("systemd-run") { "after the start" } else { "before the start" };
            self.said.lock().unwrap().push(format!("begin {node} {reason} {minutes} min, {started}"));
        }

        fn notify_agents<'a>(&'a self, _op: &'a str, nodes: &'a [String], _until: Option<u64>, _why: &'a str) -> crate::ops::BoxFuture<'a, Vec<(String, String)>> {
            self.said.lock().unwrap().push(format!("agents told about {}", nodes.join(",")));
            Box::pin(async { Vec::new() })
        }

        fn extend(&self, node: &str, _until_ms: u64) {
            self.said.lock().unwrap().push(format!("extend {node}"));
        }

        fn end(&self, node: &str) {
            let audited = if self.ran("dpkg --audit") { "after dpkg --audit" } else { "before dpkg --audit" };
            self.said.lock().unwrap().push(format!("end {node}, {audited}"));
        }

        fn resolved(&self, _rule: &str, _alert: &str, _reason: &str) {}
    }

    /// A node where apt-get really runs (the shim): its dry run too. Only the status script is answered.
    fn status_only(upgradable: &'static str) -> Script {
        Arc::new(move |cmd, _| cmd.contains("apt list --upgradable").then(|| (format!("--- upgradable\n{upgradable}\n--- held\n--- reboot\n--- unattended\n--- checked\n"), 0)))
    }

    async fn setup_with(name: &str, script: Script) -> (Arc<Ops>, Node, std::path::PathBuf) {
        let (ops, node, app, _) = setup_port(name, script).await;
        (ops, node, app)
    }

    #[tokio::test]
    async fn finished_upgrade_reports_remaining_packages_instead_of_an_all_clear() {
        let (ops, _node, _) = setup_with("remaining", status_only("libfoo/noble-updates 2.0-1 arm64 [upgradable from: 1.0-1]\nlinux-image-test/noble-updates 2.0-1 arm64 [upgradable from: 1.0-1]")).await;
        let id = upgrade(&ops, "upg-remaining", vec!["libfoo".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
        let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
        let v = run.verification.unwrap();
        assert_eq!(v.remaining.len(), 2);
        assert!(v.summary().contains("2 updates remain (0 held, 1 GPU-stack packages)"));
        assert!(!t.summary.unwrap().contains("no package updates remain"));
    }

    #[tokio::test]
    async fn unreadable_post_upgrade_status_cannot_report_success() {
        let after = Arc::new(AtomicBool::new(false));
        let flag = after.clone();
        let normal = apt();
        let script: Script = Arc::new(move |cmd, input| {
            if cmd == "dpkg --audit" { flag.store(true, Ordering::SeqCst); }
            if flag.load(Ordering::SeqCst) && cmd.contains("apt list --upgradable") {
                return Some(("--- upgradable\n".into(), 100));
            }
            normal(cmd, input)
        });
        let (ops, _node, _) = setup_with("remaining-error", script).await;
        let id = upgrade(&ops, "upg-remaining-error", vec!["libfoo".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Failed, "{:?}", t.error);
        assert!(t.error.unwrap().contains("Package status is unknown"));
        assert!(ops.runs().iter().find(|r| r.task_id == id).unwrap().verification.is_none());
    }

    #[tokio::test]
    async fn missing_status_sections_cannot_look_up_to_date() {
        let (ops, _node, _) = setup_with("status-missing", Arc::new(|cmd, _| cmd.contains("apt list --upgradable").then(|| (String::new(), 0)))).await;
        assert!(ops.updates_status(false, "upg-status-missing").await.unwrap_err().message.contains("Package status is unknown"));
    }

    /// `setup_with`, and the node's port (for a second SparkBox on the same data folder).
    async fn setup_port(name: &str, script: Script) -> (Arc<Ops>, Node, std::path::PathBuf, u16) {
        let (node, port) = fake_node(&temp(&format!("upg-{name}-home")), script).await;
        fake::pin_aware(&node);
        std::fs::create_dir_all(node.state_dir.join("apt")).unwrap();
        std::fs::write(node.state_dir.join("apt/delay"), "0.05").unwrap();
        let id = format!("upg-{name}");
        let app = temp(&format!("upg-{name}-app"));
        let (live, _) = connect(&app, &[(id.as_str(), port)]).await;
        let demo = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
        (Ops::with_poll(live, demo, Tasks::new(None), Duration::from_millis(5)), node, app, port)
    }

    /// The admitted sudo rows (not the exit's "Refused" ones) whose command contains this.
    fn sudo_rows(ops: &Ops, containing: &str) -> Vec<crate::live::audit::Entry> {
        ops.live.audit.entries().into_iter().filter(|e| e.kind == "sudo" && e.title != "Refused" && e.command.contains(containing)).collect()
    }

    /// P1.24 on a fake node: the root command saves the package list before apt starts and
    /// SparkBox files a copy in the evidence folder; the node is in a maintenance window from
    /// before the unit starts until after the checks; needrestart's list ends up in the run and
    /// the task's summary, and the dry run was the upgrade's own apt-get with -s.
    #[tokio::test]
    async fn an_upgrade_saves_the_package_list_first_and_lists_the_services_left_on_old_libraries() {
        let (ops, node) = setup("record", "0.05").await;
        std::fs::write(node.state_dir.join("apt/installed"), "libbar\t1.0-1\nlibfoo\t1.0-1\nnginx\t1.24.0-2ubuntu7.18\n").unwrap();
        std::fs::write(node.state_dir.join("needrestart"), "NEEDRESTART-VER: 3.6\nNEEDRESTART-KCUR: 7.0.0-1019-nvidia\nNEEDRESTART-KEXP: 7.0.0-1019-nvidia\nNEEDRESTART-KSTA: 1\nNEEDRESTART-SVC: nginx.service\n").unwrap();
        let window = Window::on(&ops, &node);
        let id = upgrade(&ops, "upg-record", vec!["libfoo".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
        assert_eq!(t.summary.as_deref(), Some("1 package installed on upg-record. Package check passed: no package updates remain. 1 service still uses old libraries: nginx.service"));
        assert_eq!(window.said(), ["begin upg-record upgrading 30 min, before the start", "agents told about upg-record", "end upg-record, after dpkg --audit"]);

        // The dry run shown (C2), the dry run of exactly the pinned command (C2′), then one root
        // command that saves the list and starts the unit (C1′).
        let pins = ["libfoo=2.0-1".to_string()];
        assert_eq!(node.ran("apt-get -s "), [simulate_command(&["libfoo".into()]), pinned_dry_run_opts(&pins, &UpgradeOpts::default())]);
        let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
        let dir = ops.live.upgrade_log_dir();
        let started = node.ran("systemd-run --unit sparkbox-upgrade-");
        assert_eq!(started, [format!("sudo -k -S -p '' {}", detached_command(&run.unit, &dir, &run.log_path, &pins))]);
        let before = run.dpkg_before.clone().unwrap();
        assert_eq!(before, format!("{dir}/dpkg-before-{}.txt", run.unit.trim_start_matches("sparkbox-upgrade-")));
        assert_eq!(std::fs::read_to_string(&before).unwrap(), "libbar\t1.0-1\nlibfoo\t1.0-1\nnginx\t1.24.0-2ubuntu7.18\n", "saved before apt ran");
        // Read back without sudo, and filed on the Mac, readable by you only.
        assert_eq!(node.ran("cat '"), [format!("cat '{before}'")]);
        let copy = std::path::PathBuf::from(run.evidence.clone().unwrap());
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), std::fs::read_to_string(&before).unwrap());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777, 0o600);
        let folder = copy.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        assert!(folder.ends_with("-updates") && copy.starts_with(ops.live.data_path("logs").join("evidence")), "{}", copy.display());
        assert_eq!(copy.file_name().unwrap().to_string_lossy(), format!("upg-record-dpkg-before-{}.txt", run.unit.trim_start_matches("sparkbox-upgrade-")));
        assert!(t.log.iter().any(|l| l == &format!("Package list before the upgrade: {before} (also in the evidence folder: {})", copy.display())), "{:?}", t.log);

        // After apt, in the unit: apt-get check and needrestart's list, parsed into the run.
        assert_eq!(run.needrestart.as_ref().map(|n| n.services.clone()), Some(vec!["nginx.service".to_string()]));
        assert_eq!(run.needrestart.as_ref().and_then(|n| n.kernel_status), Some(1));
        assert_eq!(run.apt_check.as_deref(), Some("Reading package lists... Done\nBuilding dependency tree... Done\nReading state information... Done"));
        assert!(t.steps.iter().any(|s| s.label == "Services still using old libraries (needrestart)" && s.detail.as_deref() == Some("nginx.service")), "{:?}", t.steps);
        assert!(!t.log.iter().any(|l| l.starts_with("SPARKBOX-")), "the markers aren't apt's output: {:?}", t.log);
        // The head line is the apt-get that ran.
        let head = ops.live.audit.entries().into_iter().find(|e| e.kind == "op" && e.op_id.as_deref() == Some(id.as_str())).unwrap();
        assert_eq!(head.command, apt_command(&pins));
    }

    /// A half-configured package after apt: dpkg --audit, run as the cluster user, lists nothing
    /// and exits 2, so the upgrade can't say the database is fine (staging finding 4).
    #[tokio::test]
    async fn an_audit_that_couldnt_read_the_database_is_no_all_clear() {
        let (ops, node) = setup("audit", "0.05").await;
        std::fs::write(node.state_dir.join("apt/audit"), "The following packages are only half configured:\n libfoo  a test library\n").unwrap();
        let id = upgrade(&ops, "upg-audit", vec!["libfoo".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Failed, "{:?} {:?}", t.summary, t.steps);
        assert_eq!(t.error.as_deref(), Some("The upgrade finished, but SparkBox couldn't check the package database on upg-audit. Run sudo dpkg --audit there."));
        let step = t.steps.iter().find(|s| s.label == "Checking the package database (dpkg --audit)").unwrap();
        assert_eq!(step.detail.as_deref(), Some("Couldn't check: dpkg: error: unable to check lock file for dpkg database directory /var/lib/dpkg: Permission denied"));
        assert!(!t.steps.iter().any(|s| s.detail.as_deref() == Some("No broken or half-installed packages")), "{:?}", t.steps);
        assert_eq!(node.ran("dpkg --audit"), ["dpkg --audit"], "read as the cluster user, never with sudo");
    }

    /// "Everything" is dist-upgrade in the dry run shown (C2), and in the run every package that
    /// plan upgrades, pinned at the version shown (C1′); the shim's real dry runs plan both. The
    /// download limit is in all three.
    #[tokio::test]
    async fn everything_is_the_shown_dist_upgrade_pinned() {
        let (ops, node, _) = setup_with("all", status_only("")).await;
        std::fs::write(node.state_dir.join("apt/pending"), "libfoo\nlibbar\nnginx\n").unwrap();
        std::fs::write(node.state_dir.join("apt/installed"), "libbar\t1.0-1\nlibfoo\t1.0-1\nnginx\t1.0-1\n").unwrap();
        let id = upgrade(&ops, "upg-all", vec![]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
        let pins: Vec<String> = ["libfoo=2.0-1", "libbar=2.0-1", "nginx=2.0-1"].map(String::from).to_vec();
        assert_eq!(node.ran("apt-get -s "), [simulate_command(&[]), pinned_dry_run_opts(&pins, &UpgradeOpts::default())], "the shim's own dry runs");
        let started = node.ran("systemd-run --unit sparkbox-upgrade-");
        assert!(started[0].contains(r#" -y --no-remove install --only-upgrade '\''libfoo=2.0-1'\'' '\''libbar=2.0-1'\'' '\''nginx=2.0-1'\''; code=$?"#) && !started[0].contains("dist-upgrade"), "{}", started[0]);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\nlibbar\nnginx\n");
        assert_eq!(t.summary.as_deref(), Some("3 packages installed on upg-all. Package check passed: no package updates remain"));
        assert_eq!(ops.runs().into_iter().find(|r| r.task_id == id).map(|r| (r.total, r.packages)), Some((3, vec!["libfoo".to_string(), "libbar".into(), "nginx".into()])));

        // With the download limit (the reattach test's): in both dry runs and in the sudo row.
        std::fs::write(node.state_dir.join("apt/pending"), "libfoo\n").unwrap();
        let id = upgrade_as_shown(&ops, "upg-all", vec!["libfoo".into()], UpgradeArgs { dl_limit_kbps: Some(500), ..Default::default() }).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
        let limited = UpgradeOpts { dl_limit_kbps: Some(500), ..Default::default() };
        let dry = node.ran("apt-get -s ");
        assert_eq!(dry[dry.len() - 2..], [simulate_command_opts(&["libfoo".into()], &limited), pinned_dry_run_opts(&["libfoo=2.0-1".into()], &limited)]);
        let rows = sudo_rows(&ops, "-o Dpkg::Options::=--force-confold -o Acquire::http::Dl-Limit=500 -o Acquire::https::Dl-Limit=500 -y --no-remove install --only-upgrade ");
        assert_eq!(rows.len(), 1, "the sudo row has the limit");
        assert_eq!(ops.runs().into_iter().find(|r| r.task_id == id).and_then(|r| r.dl_limit_kbps), Some(500));
        assert!(ops.updates_upgrade_opts(false, "upg-all".into(), vec![], UpgradeArgs { dl_limit_kbps: Some(0), ..Default::default() }).is_err());
        // With Live data, no plan: refused at once, no task.
        let before = ops.tasks.list().len();
        let e = ops.updates_upgrade(false, "upg-all".into(), vec!["libfoo".into()]).unwrap_err();
        assert_eq!((e.kind, e.message.as_str(), ops.tasks.list().len()), (ErrorKind::Invalid, DRY_RUN_FIRST, before));
    }

    /// P1.25: a GPU-stack selection is refused at once, and a plan with a kernel, a 590 driver, a
    /// GPU-stack removal or a 580 removal, in its dry run and again in C2′ before the start (apt
    /// stops C2′ on a removal: `--no-remove`); nothing runs as root and no window opens.
    #[tokio::test]
    async fn gpu_stack_selections_and_plans_are_refused_before_anything_runs_as_root() {
        let plan = Arc::new(Mutex::new(String::new()));
        let p = plan.clone();
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.starts_with("apt-get -s ") {
                let text = p.lock().unwrap().clone();
                // As apt does with --no-remove: it lists what it would remove, then stops (exit 100).
                let removed: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("Remv ")).filter_map(|r| r.split_whitespace().next()).collect();
                if cmd.contains(" --no-remove ") && !removed.is_empty() {
                    return Some((format!("Reading package lists...\nThe following packages will be REMOVED:\n  {}\n", removed.join(" ")), 100));
                }
                return Some((text, 0));
            }
            cmd.contains("apt list --upgradable").then(|| ("--- upgradable\n--- held\n--- reboot\n--- unattended\n--- checked\n".to_string(), 0))
        });
        let (ops, node, _) = setup_with("guard", script).await;
        std::fs::write(node.state_dir.join("apt/installed"), "libbrotli1\t1.1.0-2build2\nlibnvidia-compute-580\t580.178.04-0ubuntu1\nnvidia-driver-580-open\t580.178.04-0ubuntu1\n").unwrap();
        // The GPU stack is pinned: "everything" gets past "pin it first" to the guard.
        std::fs::write(node.state_dir.join("apt/holds"), "libnvidia-compute-580\nnvidia-driver-580-open\n").unwrap();
        let window = Window::on(&ops, &node);
        let any_plan = UpgradeArgs { plan: Some(vec![]), ..Default::default() };
        let e = ops.updates_upgrade_opts(false, "upg-guard".into(), vec!["libbrotli1".into(), "linux-image-7.0.0-1020-nvidia".into()], any_plan.clone()).unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Invalid, "SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia"));
        let e = ops.updates_upgrade_opts(false, "upg-guard".into(), vec!["libxnvctrl0:arm64".into()], any_plan.clone()).unwrap_err();
        assert_eq!(e.message, "SparkBox can't upgrade GPU-stack packages yet: libxnvctrl0:arm64");
        let e = ops.updates_upgrade_opts(false, "upg-guard".into(), vec!["fwupd:arm64".into()], any_plan).unwrap_err();
        assert_eq!(e.message, "SparkBox can't upgrade GPU-stack packages yet: fwupd:arm64", "an arch suffix doesn't slip past");
        assert!(node.ran("apt-get").is_empty(), "not even the dry run");
        // (plan, what its dry run says, what C2′ says before the start)
        let removes_580 = "This plan removes nvidia-driver-580-open (a 580 package)";
        for (text, shown, bound) in [
            (
                "Inst libxnvctrl0 [615.71.10-1ubuntu1] (620.70.06-1ubuntu1 NVIDIA [arm64])\nInst nvidia-settings [615.71.10-1ubuntu1] (620.70.06-1ubuntu1 NVIDIA [arm64])\n",
                "SparkBox can't upgrade GPU-stack packages yet: libxnvctrl0, nvidia-settings",
                "SparkBox can't upgrade GPU-stack packages yet: libxnvctrl0, nvidia-settings",
            ),
            (
                "Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-security [arm64])\nInst linux-image-7.0.0-1020-nvidia (7.0.0-1020.20 Ubuntu:24.04/noble-updates [arm64])\n",
                "SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia",
                "SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia",
            ),
            ("Inst nvidia-driver-590-open (590.44.01-0ubuntu1 Ubuntu:24.04/noble-updates [arm64])\nRemv nvidia-driver-580-open [580.178.04-0ubuntu1]\n", "590 deadlocks CUDA graph capture on GB10: nvidia-driver-590-open", removes_580),
            (
                "Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-security [arm64])\nRemv docker-ce [5:29.0.0-1~ubuntu.24.04~noble]\nRemv nvidia-container-toolkit [1.17.8-1]\nRemv linux-modules-nvidia-open-7.0.0-1019-nvidia [7.0.0-1019.19]\n",
                "SparkBox can't upgrade GPU-stack packages yet: this plan removes docker-ce, nvidia-container-toolkit, linux-modules-nvidia-open-7.0.0-1019-nvidia",
                "SparkBox can't upgrade GPU-stack packages yet: this plan removes docker-ce, nvidia-container-toolkit, linux-modules-nvidia-open-7.0.0-1019-nvidia",
            ),
            (
                "Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-security [arm64])\nRemv libnvidia-compute-580 [580.178.04-0ubuntu1]\n",
                "This plan removes libnvidia-compute-580 (a 580 package)",
                "This plan removes libnvidia-compute-580 (a 580 package)",
            ),
        ] {
            *plan.lock().unwrap() = text.to_string();
            let sim = ops.updates_simulate(false, "upg-guard", &[]).await.unwrap();
            assert_eq!(sim.refused.as_deref(), Some(shown));
            // Upgrade with that plan anyway (the window doesn't offer it): C2′ refuses it too.
            let id = ops.updates_upgrade_opts(false, "upg-guard".into(), vec![], UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
            let t = wait_task(&ops, &id, 30).await;
            assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(bound)), "{:?}", t.steps);
            assert!(t.log.iter().any(|l| l == "Refused before anything ran as root; nothing changed on the node."), "{:?}", t.log);
        }
        assert_eq!(node.ran("apt-get -s ").iter().filter(|c| c.contains(" --no-remove ")).count(), 5, "each plan's C2′ ran");
        assert!(node.ran("sudo").is_empty(), "nothing ran as root");
        assert!(window.said().is_empty(), "no window opened: {:?}", window.said());
        assert!(ops.runs().iter().all(|r| r.node != "upg-guard"), "no upgrade record");
        // The dry run on its own shows the exact commands: C2 it ran, C1′ the upgrade would run.
        let sim = ops.updates_simulate(false, "upg-guard", &[]).await.unwrap();
        assert_eq!((sim.command.as_str(), sim.dry_run.as_str()), (apt_command(&["libbrotli1=1.1.0-2ubuntu0.1".into()]).as_str(), simulate_command(&[]).as_str()));
        // Not pinned: "everything" says so before the guard's verdict, naming what isn't held (the
        // real nodes' case: a kernel update pending and nothing held).
        std::fs::remove_file(node.state_dir.join("apt/holds")).unwrap();
        *plan.lock().unwrap() = "Inst linux-image-7.0.0-1020-nvidia (7.0.0-1020.20 Ubuntu:24.04/noble-updates [arm64])\n".into();
        let sim = ops.updates_simulate(false, "upg-guard", &[]).await.unwrap();
        assert_eq!(sim.refused.as_deref(), Some("Upgrade everything needs the GPU stack pinned first: libnvidia-compute-580, nvidia-driver-580-open aren't held on upg-guard"));
        assert!(node.ran("sudo").is_empty(), "nothing ran as root");
    }

    /// Decision (b) on a fake node: the plan apt makes between the dry run shown and Upgrade
    /// differs (a new dependency's version moved and another joined it; a pinned version is gone;
    /// a routine package would be removed): refused with what changed, nothing run as root, no
    /// window. A plan that only adds a new dependency is allowed, and the dependency isn't pinned.
    #[tokio::test]
    async fn an_upgrade_installs_exactly_the_plan_shown_or_nothing() {
        let plan = Arc::new(Mutex::new(String::new()));
        let p = plan.clone();
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.starts_with("apt-get -s ") {
                let text = p.lock().unwrap().clone();
                let removed: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("Remv ")).filter_map(|r| r.split_whitespace().next()).collect();
                if cmd.contains(" --no-remove ") && !removed.is_empty() {
                    return Some((format!("The following packages will be REMOVED:\n  {}\n", removed.join(" ")), 100));
                }
                // apt no longer offers the pinned version.
                if cmd.contains("'libbrotli1=1.1.0-2ubuntu0.1'") && text.contains("1.1.0-2ubuntu0.2") {
                    return Some(("!stderr:E: Version '1.1.0-2ubuntu0.1' for 'libbrotli1' was not found".into(), 100));
                }
                return Some((text, 0));
            }
            cmd.contains("apt list --upgradable").then(|| ("--- upgradable\n--- held\n--- reboot\n--- unattended\n--- checked\n".to_string(), 0))
        });
        let (ops, node, _) = setup_with("bound", script).await;
        std::fs::write(node.state_dir.join("apt/installed"), "libbrotli1\t1.1.0-2build2\nlibfoo-doc\t1.0-1\n").unwrap();
        let window = Window::on(&ops, &node);
        let brotli = "Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-security [arm64])\n";
        let shown_with = |text: &str| {
            *plan.lock().unwrap() = text.to_string();
            let ops = ops.clone();
            async move { ops.updates_simulate(false, "upg-bound", &["libbrotli1".into()]).await.unwrap() }
        };
        let upgrade_after = |sim: SimulatedUpgrade, now: &str| {
            *plan.lock().unwrap() = now.to_string();
            ops.updates_upgrade_opts(false, "upg-bound".into(), vec!["libbrotli1".into()], UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap()
        };
        for (shown, now, why) in [
            (
                format!("{brotli}Inst libfoo (1.2 Ubuntu:24.04/noble-updates [arm64])\n"),
                format!("{brotli}Inst libfoo (1.3 Ubuntu:24.04/noble-updates [arm64])\nInst libbar (2.0 Ubuntu:24.04/noble-updates [arm64])\n"),
                "apt's plan for upg-bound changed since the dry run you saw: libfoo now 1.3 (was 1.2); new: libbar 2.0. Look at the new dry run and start again.",
            ),
            (
                brotli.to_string(),
                "Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.2 Ubuntu:24.04/noble-security [arm64])\n".to_string(),
                "apt's plan for upg-bound changed since the dry run you saw (apt-get -s: E: Version '1.1.0-2ubuntu0.1' for 'libbrotli1' was not found). Look at the new dry run and start again.",
            ),
            (
                brotli.to_string(),
                format!("{brotli}Remv libfoo-doc [1.0-1]\n"),
                "This plan removes libfoo-doc. SparkBox's one-node upgrades only upgrade; run it in a terminal if you mean it.",
            ),
        ] {
            let sim = shown_with(&shown).await;
            assert_eq!(sim.refused, None);
            let id = upgrade_after(sim, &now);
            let t = wait_task(&ops, &id, 30).await;
            assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(why)), "{:?}", t.log);
            assert!(t.log.iter().any(|l| l == "Refused before anything ran as root; nothing changed on the node."), "{:?}", t.log);
        }
        // A removal in the dry run shown is refused there already.
        let sim = shown_with(&format!("{brotli}Remv libfoo-doc [1.0-1]\n")).await;
        assert_eq!(sim.refused.as_deref(), Some("This plan removes libfoo-doc. SparkBox's one-node upgrades only upgrade; run it in a terminal if you mean it."));
        assert!(node.ran("sudo").is_empty() && window.said().is_empty(), "nothing ran as root, no window: {:?}", window.said());
        assert!(ops.runs().iter().all(|r| r.node != "upg-bound"), "no upgrade record");

        // A new dependency, in the plan shown and in C2′: allowed, left to apt (not pinned).
        let with_dep = "Inst libbrotli1 [1.1.0-2build2] (2.0-1 Ubuntu:24.04/noble-security [arm64])\nInst libnew (2.0-1 Ubuntu:24.04/noble-updates [arm64])\n";
        let sim = shown_with(with_dep).await;
        assert_eq!((sim.refused.as_deref(), sim.command.clone()), (None, apt_command(&["libbrotli1=2.0-1".into()])));
        let id = upgrade_after(sim, with_dep);
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
        assert!(t.log.iter().any(|l| l == "The plan you were shown: libbrotli1 2.0-1, libnew 2.0-1"), "{:?}", t.log);
        let started = node.ran("systemd-run --unit sparkbox-upgrade-");
        assert_eq!(started.len(), 1);
        assert!(started[0].contains(r#" -y --no-remove install --only-upgrade '\''libbrotli1=2.0-1'\''; code=$?"#), "only the installed package is pinned: {}", started[0]);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libbrotli1\n");
    }

    /// "Upgrade security only": the security fixes pending now, less the GPU-stack ones and the held.
    #[tokio::test]
    async fn security_only_upgrades_the_fixes_the_guard_and_the_holds_let_through() {
        let listed = "linux-image-7.0.0-1020-nvidia/noble-security 7.0.0-1020.20 arm64 [upgradable from: 7.0.0-1019.19]
libbrotli1/noble-updates,noble-security 1.1.0-2ubuntu0.1 arm64 [upgradable from: 1.1.0-2build2]
google-chrome-stable/stable,stable 156.0.8078.73-1 arm64 [upgradable from: 156.0.8078.47-1]";
        let (ops, node, _) = setup_with("security", status_only(listed)).await;
        std::fs::write(node.state_dir.join("apt/installed"), "google-chrome-stable\t156.0.8078.47-1\nlibbrotli1\t1.1.0-2build2\nlinux-image-7.0.0-1019-nvidia\t7.0.0-1019.19\n").unwrap();
        // The packages named are ignored, a GPU-stack one included (it isn't refused: it isn't taken).
        let named = vec!["google-chrome-stable".to_string(), "linux-image-7.0.0-1020-nvidia".into()];
        let id = upgrade_as_shown(&ops, "upg-security", named, UpgradeArgs { security_only: true, ..Default::default() }).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.steps);
        assert_eq!(t.title, "Upgrade security fixes on upg-security");
        assert_eq!(
            node.ran("apt-get -s "),
            [simulate_command(&["libbrotli1".into()]), pinned_dry_run_opts(&["libbrotli1=2.0-1".into()], &UpgradeOpts::default())],
            "the packages named are ignored"
        );
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libbrotli1\n");
        assert!(node.ran("sudo").iter().all(|c| !c.contains("linux-image") && !c.contains("google-chrome")));
    }

    /// Under an enforced fence of `apt-one` on the node: the upgrade's start and its Cancel are
    /// admitted (rows with fence apt-one) and reach the node; the dry run and the reads need no
    /// class. A fence without `apt-one` refuses the start, and nothing reaches the node.
    #[tokio::test]
    async fn under_an_apt_one_fence_the_upgrade_and_its_cancel_reach_the_node() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("fence", "4").await;
        let fence = TestFence::new(&ops, &temp("upg-fence-rules"));
        fence.open(&["upg-fence"], &[FenceClass::AptOne], BTreeMap::new());
        let id = upgrade(&ops, "upg-fence", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-fence", "downloading").await;
        ops.updates_cancel("upg-fence").unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Cancelled, "{:?} {:?} {:?}", t.error, t.log, ops.live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect::<Vec<_>>());
        assert!(!node.ran("pkill -TERM -P").is_empty(), "Cancel reached the node");
        assert!(!node.state_dir.join("apt/unpacked").exists(), "nothing was installed");
        let start = sudo_rows(&ops, "systemd-run --unit sparkbox-upgrade-");
        let cancel = sudo_rows(&ops, "pkill -TERM -P");
        assert_eq!((start.len(), cancel.len()), (1, 1));
        assert_eq!((start[0].fence.as_deref(), cancel[0].fence.as_deref()), (Some("apt-one"), Some("apt-one")));
        // Both carry the call site's tag (the start's shape says apt-one as well; Cancel's doesn't).
        let tags: Vec<_> = ops.live.admits().into_iter().filter(|a| a.admitted && (a.cmd.contains("systemd-run --unit sparkbox-upgrade-") || a.cmd.contains("pkill -TERM -P"))).map(|a| a.tag).collect();
        assert_eq!(tags, [Some(crate::live::exit::Tag::Class(FenceClass::AptOne)); 2]);

        // A fence for holds only: the start is refused at the exit, nothing ran as root.
        let (ops, node) = setup("fenced", "0.05").await;
        let fence = TestFence::new(&ops, &temp("upg-fenced-rules"));
        fence.open(&["upg-fenced"], &[FenceClass::AptHold], BTreeMap::new());
        let window = Window::on(&ops, &node);
        let id = upgrade(&ops, "upg-fenced", vec!["libfoo".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Failed);
        assert!(t.error.as_deref().unwrap_or_default().contains("The fence doesn't allow package upgrades"), "{:?}", t.error);
        assert!(node.ran("systemd-run").is_empty() && node.ran("sudo").is_empty(), "only reads ran: {:?}", node.ran(""));
        assert_eq!(window.said().last().map(String::as_str), Some("end upg-fenced, before dpkg --audit"), "the window opened for it is closed again");
    }

    /// Under a fence whose `apt-one` names only libbrotli1 (Session B's step 21): the pinned
    /// upgrade of libbrotli1 is admitted (its item is the name, not `name=version`), and so is its
    /// Cancel, which only stops apt (R4-SAFE's exemption), and both reach the node.
    #[tokio::test]
    async fn under_an_apt_one_fence_naming_only_libbrotli1_the_upgrade_and_its_cancel_reach_the_node() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("only", "4").await;
        std::fs::write(node.state_dir.join("apt/installed"), "libbrotli1\t1.1.0-2build2\n").unwrap();
        let fence = TestFence::new(&ops, &temp("upg-only-rules"));
        fence.open(&["upg-only"], &[FenceClass::AptOne], BTreeMap::from([(FenceClass::AptOne, vec!["libbrotli1".to_string()])]));
        let id = upgrade(&ops, "upg-only", vec!["libbrotli1".into()]).await;
        wait_phase(&ops, "upg-only", "downloading").await;
        ops.updates_cancel("upg-only").unwrap();
        let t = wait_task(&ops, &id, 30).await;
        let refused: Vec<_> = ops.live.audit.entries().into_iter().filter(|e| e.title == "Refused").map(|e| (e.command, e.error)).collect();
        assert_eq!(t.state, TaskState::Cancelled, "{:?} {:?} {refused:?}", t.error, t.log);
        assert!(t.log.iter().any(|l| l == "Stopping apt before it installs anything…"), "{:?}", t.log);
        let start = sudo_rows(&ops, "systemd-run --unit sparkbox-upgrade-");
        let cancel = sudo_rows(&ops, "pkill -TERM -P");
        assert_eq!((start.len(), cancel.len()), (1, 1), "{refused:?}");
        assert!(start[0].command.contains("'\\''libbrotli1=2.0-1'\\''"), "{}", start[0].command);
        assert_eq!((start[0].fence.as_deref(), cancel[0].fence.as_deref()), (Some("apt-one"), Some("apt-one")));
        assert!(!node.state_dir.join("apt/unpacked").exists(), "nothing was installed");
    }

    /// P1.27 on a fake node whose package database has the GPU stack and other packages: the pin
    /// holds exactly the stack (the sudo row's items are those names: a fence whose `only` lists
    /// them admits it, one that lists fewer refuses it), lists them in its task and the evidence
    /// folder, and the release puts back exactly the held ones.
    #[tokio::test]
    async fn the_gpu_stack_pin_holds_exactly_the_stack() {
        let (ops, node) = setup("pin", "0.05").await;
        let installed = "containerd.io\t1.7.27-1\ndocker-ce\t5:29.0.0-1~ubuntu.24.04~noble\nfwupd\t1.9.30-0ubuntu1~24.04.1\ngoogle-chrome-stable\t156.0.8078.47-1\nlibbrotli1\t1.1.0-2build2\nlibnvidia-compute-580\t580.178.04-0ubuntu1\nlinux-image-7.0.0-1019-nvidia\t7.0.0-1019.19\nnginx\t1.24.0-2ubuntu7.18\nnvidia-driver-580-open\t580.178.04-0ubuntu1\n";
        std::fs::write(node.state_dir.join("apt/installed"), installed).unwrap();
        let stack: Vec<String> = ["containerd.io", "docker-ce", "libnvidia-compute-580", "linux-image-7.0.0-1019-nvidia", "nvidia-driver-580-open"].map(String::from).to_vec();
        let fence = TestFence::new(&ops, &temp("upg-pin-rules"));

        // Fewer names in the fence's `only` than the pin holds: refused, nothing held.
        fence.open(&["upg-pin"], &[FenceClass::AptHold], BTreeMap::from([(FenceClass::AptHold, vec!["docker-ce".to_string()])]));
        let e = ops.updates_hold_preset(false, "upg-pin", true, false).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Fenced, "{}", e.message);
        assert!(node.ran("apt-mark hold").is_empty());

        fence.open(&["upg-pin"], &[FenceClass::AptHold], BTreeMap::from([(FenceClass::AptHold, stack.clone())]));
        let preview = ops.updates_hold_preset(false, "upg-pin", true, true).await.unwrap();
        assert_eq!((preview.packages.clone(), preview.task_id.clone()), (stack.clone(), None));
        assert!(node.ran("sudo").is_empty(), "the preview only reads");
        assert_eq!(node.ran("dpkg-query"), [PRESET_READ, PRESET_READ], "the refused pin and the preview read it");

        let pinned = ops.updates_hold_preset(false, "upg-pin", true, false).await.unwrap();
        assert_eq!(pinned.packages, stack);
        let quoted = stack.iter().map(|p| q(p)).collect::<Vec<_>>().join(" ");
        assert_eq!(node.ran("apt-mark hold"), [format!("sudo -k -S -p '' apt-mark hold {quoted}")]);
        let rows = sudo_rows(&ops, "apt-mark hold");
        assert_eq!((rows.len(), rows[0].fence.as_deref()), (1, Some("apt-hold")));
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/holds")).unwrap(), stack.iter().map(|s| format!("{s}\n")).collect::<String>());
        let t = ops.tasks.get(pinned.task_id.as_deref().unwrap()).unwrap();
        assert_eq!((t.title.as_str(), t.state, t.summary.as_deref()), ("Pin the GPU stack on upg-pin", TaskState::Ok, Some("Pinned 5 GPU-stack packages on upg-pin")));
        let at = t.log.iter().position(|l| l == "apt-mark hold on upg-pin:").expect("the list");
        assert_eq!(t.log[at + 1..at + 1 + stack.len()], stack[..], "every package, the rollback list: {:?}", t.log);
        let filed = std::fs::read_dir(ops.live.data_path("logs").join("evidence")).unwrap().flatten().flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten()).map(|f| f.path()).find(|p| p.to_string_lossy().contains(&format!("upg-pin-holds-{}", t.id))).expect("filed");
        assert!(std::fs::read_to_string(&filed).unwrap().ends_with(&format!("{}\n", stack.join("\n"))));

        // The tag on the root command is the class the fence is asked for (the shape says it too).
        let tagged = |what: &str| ops.live.admits().into_iter().filter(|a| a.admitted && a.cmd.contains(what)).map(|a| a.tag).collect::<Vec<_>>();
        assert_eq!(tagged("apt-mark hold"), [Some(crate::live::exit::Tag::Class(FenceClass::AptHold))]);

        // Release: the held ones of the stack only (docker-ce was released by hand already, and
        // google-chrome-stable, held by hand, stays held).
        let held: Vec<&String> = stack.iter().filter(|s| *s != "docker-ce").collect();
        std::fs::write(node.state_dir.join("apt/holds"), format!("{}google-chrome-stable\n", held.iter().map(|s| format!("{s}\n")).collect::<String>())).unwrap();
        let released = ops.updates_hold_preset(false, "upg-pin", false, false).await.unwrap();
        assert_eq!(released.packages, held.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(node.ran("apt-mark unhold"), [format!("sudo -k -S -p '' apt-mark unhold {}", held.iter().map(|p| q(p)).collect::<Vec<_>>().join(" "))]);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/holds")).unwrap(), "google-chrome-stable\n");
        let t = ops.tasks.get(released.task_id.as_deref().unwrap()).unwrap();
        assert_eq!((t.title.as_str(), t.summary.as_deref()), ("Release the GPU-stack pin on upg-pin", Some("Released 4 GPU-stack packages on upg-pin")));

        // A plain hold is a package-hold change too.
        ops.updates_hold(false, "upg-pin", &["libbrotli1".into()], true).await.unwrap_err();
        fence.open(&["upg-pin"], &[FenceClass::AptHold], BTreeMap::new());
        ops.updates_hold(false, "upg-pin", &["libbrotli1".into()], true).await.unwrap();
        assert_eq!(sudo_rows(&ops, "apt-mark hold 'libbrotli1'").last().and_then(|r| r.fence.clone()).as_deref(), Some("apt-hold"));
        assert_eq!(tagged("apt-mark hold 'libbrotli1'"), [Some(crate::live::exit::Tag::Class(FenceClass::AptHold))]);
    }

    /// The fence the tests open: signed with a key this build trusts, in a folder of the test's own.
    struct TestFence {
        rules: crate::live::exit::Enforce,
        key: p256::ecdsa::SigningKey,
        file: crate::live::fence::KeyFile,
    }

    /// Admin unlocked, look-only off: only the fence decides.
    struct Unlocked;

    impl crate::live::exit::WritePolicy for Unlocked {
        fn look_only(&self) -> bool {
            false
        }

        fn admin_unlocked(&self) -> bool {
            true
        }
    }

    impl TestFence {
        fn new(ops: &Ops, base: &std::path::Path) -> TestFence {
            use crate::live::fence;
            let instance = crate::identity::instance_name();
            let rules = crate::live::exit::Enforce {
                instance: instance.into(),
                fence_dir: base.join("SparkBox Fence"),
                config_dir: Some(base.join(format!("config-{instance}"))),
                noauth: false,
                tag: "p1-r3-test".into(),
                mac: Some("Studio".into()),
            };
            std::fs::create_dir_all(&rules.fence_dir).unwrap();
            let key = fence::new_key().unwrap();
            let (key_id, public_key) = fence::public_of(&key).unwrap();
            let file = fence::KeyFile { key_id, public_key, kind: "keychain".into() };
            fence::save_trust(rules.config_dir.as_ref().unwrap(), &file).unwrap();
            ops.live.enforce_writes(rules.clone());
            ops.live.set_write_policy(Arc::new(Unlocked));
            TestFence { rules, key, file }
        }

        fn open(&self, nodes: &[&str], classes: &[FenceClass], only: BTreeMap<FenceClass, Vec<String>>) {
            use crate::live::fence;
            let spec = fence::Spec { nodes: nodes.iter().map(|n| n.to_string()).collect(), classes: classes.to_vec(), only, minutes: 180, instance: self.rules.instance.clone(), ..Default::default() };
            let record = fence::record_for(&spec, &self.key, now_ms(), "Studio").unwrap();
            fence::write(&self.rules.fence_dir, self.rules.config_dir.as_ref().unwrap(), &self.file, &record).unwrap();
        }
    }

    // ---- Demo: the guard holds there too; the new functions say they need Live data.

    fn demo_ops(name: &str) -> Arc<Ops> {
        let dir = temp(&format!("upg-demo-{name}"));
        let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
        let live = crate::live::Live::new(vpn, dir.join("kh.json"), || Ok(None));
        let d = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
        let ops = Ops::with_poll(live, d.clone(), Tasks::new(None), Duration::from_millis(1));
        ops.set_snapshot_source(move || {
            let mut d = d.lock().unwrap();
            d.advance();
            Some(d.snapshot())
        });
        ops
    }

    /// The Demo world has no GPU-stack package pending (as on the real nodes), so the Demo dry run
    /// is checked with one added: the same guard refuses it with the same words. A GPU-stack
    /// selection is refused in Demo before anything is simulated, and security only works there.
    #[tokio::test]
    async fn the_guard_holds_in_demo_and_the_pin_needs_live_data() {
        let pending = parse_upgradable(
            "libbrotli1/noble-updates,noble-security 1.1.0-2ubuntu0.1 arm64 [upgradable from: 1.1.0-2build2]
linux-image-7.0.0-1020-nvidia/noble-updates 7.0.0-1020.20 arm64 [upgradable from: 7.0.0-1019.19]",
        );
        let sim = judge(demo_plan("cell2", &pending, &[]), &[], &UpgradeOpts::default());
        assert_eq!(sim.refused.as_deref(), Some("SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia, linux-modules-7.0.0-1020.20"));
        assert_eq!(sim.sensitive, ["linux-image-7.0.0-1020-nvidia", "linux-modules-7.0.0-1020.20"]);
        let mut held = pending.clone();
        held.iter_mut().find(|p| p.name.starts_with("linux-image-")).unwrap().held = true;
        assert_eq!(judge(demo_plan("cell2", &held, &[]), &[], &UpgradeOpts::default()).refused, None, "held: kept back, the plan passes");

        let ops = demo_ops("guard");
        let before = ops.live.audit.entries().len();
        let e = ops.updates_upgrade(true, "cell2".into(), vec!["nvidia-driver-580-open".into()]).unwrap_err();
        assert_eq!(e.message, "SparkBox can't upgrade GPU-stack packages yet: nvidia-driver-580-open");
        assert_eq!(ops.live.audit.entries().len(), before, "nothing simulated");
        // The real Demo world: every Demo dry run passes the guard.
        for n in ["cell1", "cell2", "cell3", "gx10-0000"] {
            let sim = ops.updates_simulate(true, n, &[]).await.unwrap();
            assert_eq!((sim.refused.as_deref(), sim.sensitive.len()), (None, 0), "{n}");
            assert_eq!(sim.dry_run, simulate_command(&[]));
        }
        let id = ops.updates_upgrade_opts(true, "cell1".into(), vec![], UpgradeArgs { security_only: true, ..Default::default() }).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
        let left: Vec<String> = ops.updates_status(true, "cell1").await.unwrap().pending.into_iter().map(|p| p.name).collect();
        assert_eq!(left, ["google-chrome-stable"], "the three security fixes went in");
        let e = ops.updates_hold_preset(true, "cell2", true, true).await.unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::DemoOnly, "Available with Live data."));
    }

    /// Demo runs decision (b)'s flow on its simulated apt: the dry run's plan (pinned in the
    /// command it shows) bound to the upgrade, a plan whose version the simulator doesn't offer
    /// refused with apt's words and nothing simulated as root, and without a plan Demo binds to
    /// its own dry run.
    #[tokio::test]
    async fn demo_runs_the_same_flow_on_its_simulated_apt() {
        let ops = demo_ops("bound");
        let sim = ops.updates_simulate(true, "cell2", &[]).await.unwrap();
        assert!(!sim.plan.is_empty() && sim.refused.is_none(), "{sim:?}");
        let pins: Vec<String> = sim.plan.iter().filter(|p| !p.new).map(|p| format!("{}={}", p.name, p.to)).collect();
        assert_eq!(sim.command, apt_command(&pins));
        // A version the simulator doesn't have: refused, no simulated sudo.
        let mut moved = sim.shown();
        moved[0].to = format!("{}+moved", moved[0].to);
        let id = ops.updates_upgrade_opts(true, "cell2".into(), vec![], UpgradeArgs { plan: Some(moved.clone()), ..Default::default() }).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        let want = format!("apt's plan for cell2 changed since the dry run you saw (apt-get -s: E: Version '{}' for '{}' was not found). Look at the new dry run and start again.", moved[0].to, moved[0].name);
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(want.as_str())));
        assert!(ops.live.audit.entries().iter().all(|e| !(e.simulated && e.command.starts_with("sudo env "))), "nothing simulated as root");
        // The plan shown: installed, in the simulator too.
        let id = ops.updates_upgrade_opts(true, "cell2".into(), vec![], UpgradeArgs { plan: Some(sim.shown()), ..Default::default() }).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
        assert!(ops.live.audit.entries().iter().any(|e| e.simulated && e.command == format!("sudo {}", apt_command(&pins))), "the pinned command, simulated");
        assert!(ops.updates_status(true, "cell2").await.unwrap().pending.iter().all(|p| p.held), "what was shown went in");
    }

    // ---- Round 4: a run whose node is lost is never forgotten (the follower), and its window
    // lasts while apt runs (decision d).

    /// The task picked up again on this SparkBox, once there is one (up to `secs`), then its end.
    async fn picked_up_within(ops: &Ops, secs: u64) -> Task {
        for _ in 0..secs * 50 {
            if let Some(t) = ops.tasks.list().into_iter().find(|t| t.title.ends_with("(picked up again)")) {
                return wait_task(ops, &t.id, 60).await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("never picked up again: {:?}", ops.tasks.list().iter().map(|t| &t.title).collect::<Vec<_>>());
    }

    /// The node goes away mid-run for longer than the follower waits: the task warns, the run is
    /// "lost" and stays in upgrades.json (apt keeps running on the node). Back, it's picked up,
    /// followed to its end and leaves upgrades.json.
    #[tokio::test]
    async fn a_node_away_past_the_wait_leaves_a_lost_run_that_completes_when_it_is_back() {
        let (ops, node) = setup("away", "0.8").await;
        ops.set_upgrade_timing(Duration::from_millis(1500), UPGRADE_WINDOW);
        let id = upgrade(&ops, "upg-away", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-away", "downloading").await;
        fake::away(&node).await;
        let t = wait_task(&ops, &id, 30).await;
        let log = ops.runs().into_iter().find(|r| r.task_id == id).unwrap().log_path;
        assert_eq!(
            (t.state, t.summary.as_deref(), t.warning.as_deref()),
            (
                TaskState::Ok,
                Some("Lost upg-away while it was upgrading"),
                Some(format!("Lost upg-away while it was upgrading; apt may still be running there. SparkBox looks again when upg-away is back, and after a restart (log: {log}).").as_str())
            ),
            "{:?}",
            t.log
        );
        assert_eq!(saved_phases(&ops).get("upg-away").map(String::as_str), Some("lost"));
        assert_eq!(ops.runs().into_iter().find(|r| r.node == "upg-away").map(|r| r.phase), Some("lost".to_string()));
        // Lost, it holds the node: no second upgrade, and no restart (installing()).
        let e = ops.updates_upgrade_opts(false, "upg-away".into(), vec!["libfoo".into()], UpgradeArgs { plan: Some(vec![PlanEntry { name: "libfoo".into(), to: "2.0-1".into() }]), ..Default::default() }).unwrap_err();
        assert_eq!(e.message, "SparkBox lost upg-away while it was upgrading; apt may still be running there. It looks again when upg-away is back.");
        assert!(ops.power_busy("upg-away", "restart it").is_some_and(|w| w.starts_with("upg-away may still be installing packages (Upgrade 2 packages on upg-away)")));

        fake::back(&node);
        let t = picked_up_within(&ops, 60).await;
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("The upgrade on upg-away finished. Package check passed: no package updates remain")), "{:?} {:?}", t.error, t.log);
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\nlibbar\n", "apt finished on the node meanwhile");
        assert_eq!(saved_phases(&ops).get("upg-away"), None, "its exit is known: the record is gone");
        assert_eq!(node.ran("systemd-run --unit sparkbox-upgrade-").len(), 1, "started once");
    }

    /// The node restarts under apt mid-unpack (cell3 and gx10-0000 restarted on their own on
    /// 2026-10-29): apt dies with no exit line in its log and the unit is gone. dpkg --audit
    /// still runs, as the cluster user, before the window closes; the task says what happened
    /// and what to run there; the record leaves upgrades.json only after that check.
    #[tokio::test]
    async fn a_node_that_restarts_mid_unpack_is_audited_before_its_record_goes() {
        let (ops, node) = setup("restart", "0.6").await;
        ops.set_upgrade_timing(Duration::from_secs(20), UPGRADE_WINDOW);
        let window = Window::on(&ops, &node);
        let id = upgrade(&ops, "upg-restart", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-restart", "unpacking").await;
        fake::restart(&node, Some("libfoo")).await;
        let t = wait_task(&ops, &id, 60).await;
        let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
        assert_eq!(
            (t.state, t.error.as_deref()),
            (
                TaskState::Failed,
                Some(format!("apt on upg-restart stopped without an exit status (the node may have restarted), and SparkBox couldn't check the package database. Run sudo dpkg --audit there, and sudo dpkg --configure -a if it lists packages (apt's log: {}).", run.log_path).as_str())
            ),
            "{:?}",
            t.log
        );
        // The follower's step (the task's first has the same words).
        let installing = t.steps.iter().rev().find(|s| s.label == "Installing 2 packages").unwrap();
        assert_eq!((installing.state, installing.detail.as_deref()), (crate::tasks::StepState::Failed, Some("apt stopped without an exit status")), "{:?}", t.steps);
        let step = t.steps.iter().find(|s| s.label == "Checking the package database (dpkg --audit)").unwrap();
        assert_eq!(step.detail.as_deref(), Some("Couldn't check: dpkg: error: unable to check lock file for dpkg database directory /var/lib/dpkg: Permission denied"));
        assert_eq!(node.ran("dpkg --audit"), ["dpkg --audit"], "read as the cluster user, never with sudo");
        assert_eq!(window.said().last().map(String::as_str), Some("end upg-restart, after dpkg --audit"));
        assert_eq!((run.phase.as_str(), run.exit_code), ("failed", None));
        assert_eq!(saved_phases(&ops).get("upg-restart"), None, "checked: the record is gone");
        assert!(!node.state_dir.join("apt/upgraded").exists(), "nothing was set up");
    }

    /// R11d: `interrupted`, when dpkg lists packages. Real dpkg 1.22 won't list them to the cluster
    /// user (the test above, staging finding 4); this stand-in does, as a dpkg that can read its
    /// lock would. The task fails with what to run, and the run is checked, so its record goes.
    #[tokio::test]
    async fn a_restart_that_left_packages_half_installed_says_what_to_run() {
        let (ops, node) = setup("restart-listed", "0.6").await;
        ops.set_upgrade_timing(Duration::from_secs(20), UPGRADE_WINDOW);
        node.bin("dpkg", "#!/bin/dash\ncat \"$SB_FAKE_STATE/apt/audit\"\nexit 1\n");
        let id = upgrade(&ops, "upg-restart-listed", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-restart-listed", "unpacking").await;
        fake::restart(&node, Some("libfoo")).await;
        let t = wait_task(&ops, &id, 60).await;
        let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
        assert_eq!(
            (t.state, t.error.as_deref()),
            (
                TaskState::Failed,
                Some(format!("apt on upg-restart-listed stopped without an exit status (the node may have restarted), and dpkg reports packages that aren't fully installed. Run sudo dpkg --configure -a there (apt's log: {}).", run.log_path).as_str())
            ),
            "{:?}",
            t.log
        );
        assert!(t.log.iter().any(|l| l.contains("only half installed")) && t.log.iter().any(|l| l.trim().starts_with("libfoo ")), "dpkg's list is in the task's log: {:?}", t.log);
        let step = t.steps.iter().find(|s| s.label == "Checking the package database (dpkg --audit)").unwrap();
        assert_eq!((step.state, step.detail.as_deref()), (crate::tasks::StepState::Failed, Some("dpkg reports packages that aren't fully installed")));
        assert_eq!(node.ran("dpkg --audit").len(), 1);
        assert_eq!((run.phase.as_str(), saved_phases(&ops).get("upg-restart-listed")), ("failed", None), "checked: the record is gone");
    }

    /// R11d: `interrupted`, when dpkg never answers though the node is there (it dies by a signal
    /// each time, so the channel closes with no exit status): three tries, then it fails unchecked
    /// and says what to run by hand.
    #[tokio::test]
    async fn three_tries_without_an_answer_fail_the_check_unchecked() {
        let (ops, node) = setup("restart-mute", "0.6").await;
        ops.set_upgrade_timing(Duration::from_secs(20), UPGRADE_WINDOW);
        node.bin("dpkg", "#!/bin/dash\nkill -KILL $$\n");
        let id = upgrade(&ops, "upg-restart-mute", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-restart-mute", "unpacking").await;
        fake::restart(&node, None).await;
        let t = wait_task(&ops, &id, 60).await;
        let run = ops.runs().into_iter().find(|r| r.task_id == id).unwrap();
        assert_eq!(
            (t.state, t.error.as_deref()),
            (
                TaskState::Failed,
                Some(format!("apt on upg-restart-mute stopped without an exit status (the node may have restarted), and SparkBox couldn't check the package database. Run sudo dpkg --audit there, and sudo dpkg --configure -a if it lists packages (apt's log: {}).", run.log_path).as_str())
            ),
            "{:?}",
            t.log
        );
        assert_eq!(node.ran("dpkg --audit").len(), 3, "three tries, and no fourth");
        let step = t.steps.iter().find(|s| s.label == "Checking the package database (dpkg --audit)").unwrap();
        assert_eq!((step.state, step.detail.as_deref()), (crate::tasks::StepState::Failed, Some("Couldn't check: no exit status")));
        assert_eq!(run.phase, "failed");
    }

    /// apt stopped without an exit status, and the node goes away again while dpkg --audit runs:
    /// the run is lost, not dropped (upgrades.json keeps it). Back, it's picked up again, checked,
    /// and only then does its record go.
    #[tokio::test]
    async fn a_node_lost_again_before_the_check_keeps_its_record_until_it_is_checked() {
        let (ops, node) = setup("restart-again", "0.6").await;
        ops.set_upgrade_timing(Duration::from_secs(20), UPGRADE_WINDOW);
        // dpkg --audit waits while apt/audit-waits is there: the node goes away under it.
        let dpkg = std::fs::read_to_string(node.bin_dir.join("dpkg")).unwrap();
        node.bin("shim/dpkg", &dpkg);
        node.bin("dpkg", "#!/bin/dash\nwhile [ -f \"$SB_FAKE_STATE/apt/audit-waits\" ]; do sleep 0.05; done\nexec \"$(dirname \"$0\")/shim/dpkg\" \"$@\"\n");
        std::fs::write(node.state_dir.join("apt/audit-waits"), "").unwrap();
        let id = upgrade(&ops, "upg-restart-again", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-restart-again", "downloading").await;
        fake::restart(&node, None).await;
        for _ in 0..1500 {
            if !node.ran("dpkg --audit").is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(node.ran("dpkg --audit").len(), 1, "checking");
        ops.set_upgrade_timing(Duration::from_millis(1500), UPGRADE_WINDOW);
        fake::away(&node).await;
        std::fs::remove_file(node.state_dir.join("apt/audit-waits")).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("Lost upg-restart-again while it was upgrading")), "{:?} {:?} {:?}", t.error, t.log, t.steps);
        assert!(
            t.log.iter().any(|l| l == "apt on upg-restart-again stopped without an exit status (the node may have restarted); SparkBox lost upg-restart-again again before it could check the package database, and checks it when upg-restart-again is back."),
            "{:?}",
            t.log
        );
        assert_eq!(saved_phases(&ops).get("upg-restart-again").map(String::as_str), Some("lost"), "not checked yet: kept");

        fake::back(&node);
        let t = picked_up_within(&ops, 60).await;
        let log = ops.runs().into_iter().find(|r| r.task_id == t.id).unwrap().log_path;
        assert_eq!(
            (t.state, t.error.as_deref()),
            (
                TaskState::Failed,
                Some(format!("apt on upg-restart-again stopped without an exit status (the node may have restarted). dpkg --audit lists no broken or half-installed packages; apt's log shows how far it got: {log}.").as_str())
            ),
            "{:?}",
            t.log
        );
        assert_eq!(node.ran("dpkg --audit").len(), 2, "checked when it was back");
        assert_eq!(saved_phases(&ops).get("upg-restart-again"), None, "checked: the record is gone");
        assert!(!node.state_dir.join("apt/unpacked").exists(), "stopped while it downloaded");
    }

    /// Live stops mid-run (Live → Demo, or SparkBox quitting): the record is kept as lost, and a
    /// fresh SparkBox on the same data folder picks it up and follows it to its end.
    #[tokio::test]
    async fn live_stopping_mid_run_keeps_the_record_and_a_fresh_sparkbox_picks_it_up() {
        let (ops, node, app, port) = setup_port("stop", apt()).await;
        std::fs::write(node.state_dir.join("apt/delay"), "0.6").unwrap();
        let id = upgrade(&ops, "upg-stop", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-stop", "downloading").await;
        ops.live.stop();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("Lost upg-stop while it was upgrading")), "{:?}", t.log);
        assert!(t.warning.is_some());
        assert_eq!(saved_phases(&ops).get("upg-stop").map(String::as_str), Some("lost"));

        // The app opens again: a new Live and Ops over the same data folder.
        let (live, _) = connect(&app, &[("upg-stop", port)]).await;
        let demo = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
        let again = Ops::with_poll(live, demo, Tasks::new(None), Duration::from_millis(5));
        again.reattach_upgrades();
        again.reattach_upgrades();
        let t = picked_up_within(&again, 30).await;
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("The upgrade on upg-stop finished. Package check passed: no package updates remain")), "{:?} {:?}", t.error, t.log);
        assert_eq!(again.tasks.list().iter().filter(|t| t.title.ends_with("(picked up again)")).count(), 1, "one follower, however often it's called");
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\nlibbar\n");
        assert_eq!(saved_phases(&again).get("upg-stop"), None);
    }

    /// Two nodes on one SparkBox: one's run was lost and its node never comes back; the other
    /// upgrades meanwhile. Saving the second's run keeps the first's record, and it's still there
    /// (lost) after the second is done and some waiting.
    #[tokio::test]
    async fn another_nodes_upgrade_keeps_a_lost_run_and_a_node_that_never_returns_keeps_its_record() {
        let (away, pa) = fake_node(&temp("upg-never-home"), apt()).await;
        let (busy, pb) = fake_node(&temp("upg-other-home"), apt()).await;
        fake::pin_aware(&busy);
        std::fs::create_dir_all(busy.state_dir.join("apt")).unwrap();
        std::fs::write(busy.state_dir.join("apt/delay"), "0.05").unwrap();
        let app = temp("upg-never-app");
        let (live, _) = connect(&app, &[("upg-never", pa), ("upg-other", pb)]).await;
        let demo = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
        let ops = Ops::with_poll(live, demo, Tasks::new(None), Duration::from_millis(5));
        fake::away(&away).await;
        for _ in 0..500 {
            if !ops.live.is_connected("upg-never") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!ops.live.is_connected("upg-never"), "gone");
        // What an earlier SparkBox saved when it lost upg-never.
        let dir = ops.live.upgrade_log_dir();
        let lost = UpgradeRun { node: "upg-never".into(), title: "Upgrade upg-never".into(), unit: "sparkbox-upgrade-gone1".into(), log_path: format!("{dir}/upgrade-gone1.log"), phase: "lost".into(), total: 1, ..Default::default() };
        crate::util::save_json(&saved_path(&ops), &vec![lost]).unwrap();
        ops.runs.lock().unwrap().remove("upg-never");
        ops.reattach_upgrades();

        let id = upgrade(&ops, "upg-other", vec!["libfoo".into()]).await;
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
        assert_eq!(saved_phases(&ops).get("upg-never").map(String::as_str), Some("lost"), "the other node's save kept it");
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(saved_phases(&ops).get("upg-never").map(String::as_str), Some("lost"), "never back: still there");
        assert!(ops.tasks.list().iter().all(|t| !t.title.starts_with("Upgrade upg-never")), "nothing picked it up");
        assert_eq!(ops.runs().into_iter().find(|r| r.node == "upg-never").map(|r| r.phase), Some("lost".to_string()));
    }

    /// When the planned-work calls came, and the window's end each set (ms since the epoch).
    #[derive(Default)]
    struct Clocked {
        calls: Mutex<Vec<(u64, &'static str, Option<u64>)>>,
    }

    impl crate::ops::Planned for Clocked {
        fn begin(&self, _node: &str, until_ms: Option<u64>, _reason: &str) {
            self.calls.lock().unwrap().push((now_ms(), "begin", until_ms));
        }

        fn extend(&self, _node: &str, until_ms: u64) {
            self.calls.lock().unwrap().push((now_ms(), "extend", Some(until_ms)));
        }

        fn end(&self, _node: &str) {
            self.calls.lock().unwrap().push((now_ms(), "end", None));
        }

        fn resolved(&self, _rule: &str, _alert: &str, _reason: &str) {}
    }

    impl Clocked {
        /// The window's end in force at each call, and whether it had lapsed before the call came.
        fn lapses(&self) -> Vec<(&'static str, bool)> {
            let calls = self.calls.lock().unwrap().clone();
            let mut until: Option<u64> = None;
            calls
                .into_iter()
                .map(|(at, what, set)| {
                    let lapsed = until.is_some_and(|u| at > u);
                    if set.is_some() {
                        until = set;
                    }
                    (what, lapsed)
                })
                .collect()
        }
    }

    /// Decision (d) with a 2 s window: an upgrade that runs about five windows long keeps its
    /// window open throughout (each sighting of the unit active moves its end), and it closes at
    /// the end of the task.
    #[tokio::test]
    async fn the_window_stays_open_while_apt_runs_and_closes_at_the_end() {
        let (ops, _node) = setup("window", "2").await;
        let window = Duration::from_secs(2);
        ops.set_upgrade_timing(UPGRADE_WAIT, window);
        let clock = Arc::new(Clocked::default());
        ops.set_planned(clock.clone());
        let t0 = now_ms();
        let id = upgrade(&ops, "upg-window", vec!["libfoo".into(), "libbar".into()]).await;
        let t = wait_task(&ops, &id, 60).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
        let lapses = clock.lapses();
        assert!(now_ms() - t0 >= 4 * window.as_millis() as u64, "it ran several windows long");
        assert_eq!(lapses.first().map(|l| l.0), Some("begin"));
        assert_eq!(lapses.last().map(|l| l.0), Some("end"));
        assert!(lapses.iter().filter(|l| l.0 == "extend").count() >= 4, "{lapses:?}");
        assert!(lapses.iter().all(|(_, lapsed)| !lapsed), "open throughout: {lapses:?}");
    }

    /// Out of contact the window isn't extended: it lapses one window-length after the unit was
    /// last seen active, well before the follower gives up; the task's end closes it; when the
    /// node is back the reattach opens one again.
    #[tokio::test]
    async fn out_of_contact_the_window_lapses_a_window_after_the_last_sighting() {
        let (ops, node) = setup("dark", "1.5").await;
        let window = Duration::from_secs(2);
        ops.set_upgrade_timing(Duration::from_secs(6), window);
        let clock = Arc::new(Clocked::default());
        ops.set_planned(clock.clone());
        let id = upgrade(&ops, "upg-dark", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-dark", "downloading").await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        fake::away(&node).await;
        let dark = now_ms();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(t.summary.as_deref(), Some("Lost upg-dark while it was upgrading"), "{:?}", t.log);
        let calls = clock.calls.lock().unwrap().clone();
        let last = calls.iter().rev().find(|c| c.1 != "end").cloned().unwrap();
        assert_eq!(last.1, "extend", "{calls:?}");
        assert!(last.0 <= dark + 200, "no extension out of contact: {calls:?}");
        let until = last.2.unwrap();
        assert!(until + 1500 >= dark + window.as_millis() as u64 && until <= dark + window.as_millis() as u64 + 200, "the window ends a window-length after the last sighting: {until} vs {dark}");
        let end = calls.iter().find(|c| c.1 == "end").unwrap().0;
        assert!(end > until + 2000, "it lapsed by itself, before the follower gave up at the task's end: {calls:?}");

        fake::back(&node);
        let t = picked_up_within(&ops, 60).await;
        assert_eq!(t.state, TaskState::Ok, "{:?} {:?}", t.error, t.log);
        let calls = clock.calls.lock().unwrap().clone();
        assert_eq!(calls.iter().filter(|c| c.1 == "begin").count(), 2, "the reattach opened one again: {calls:?}");
        assert_eq!(calls.last().map(|c| c.1), Some("end"));
    }

    /// Cancel's words for each outcome of C-X on the node.
    #[test]
    fn cancel_says_what_happened() {
        let ok = |code: u32, err: &str| -> Result<(Option<u32>, String, String), SbError> { Ok((Some(code), String::new(), err.to_string())) };
        assert_eq!(cancel_said(&ok(0, "")), "Stopping apt before it installs anything…");
        assert_eq!(cancel_said(&ok(3, "dpkg is installing packages\n")), DPKG_RUNS);
        assert_eq!(cancel_said(&ok(1, "")), "apt had already finished; nothing to stop.");
        assert_eq!(cancel_said(&ok(3, "pkill: cannot allocate memory\n")), "Couldn't stop it: pkill: cannot allocate memory");
        assert_eq!(cancel_said(&ok(2, "")), "Couldn't stop it: exit 2");
        assert_eq!(cancel_said(&Err(SbError::new(ErrorKind::LookOnly, "Look-only mode is on."))), "Look-only refused Cancel; the upgrade keeps running");
        assert_eq!(cancel_said(&Err(SbError::new(ErrorKind::Fenced, "The fence doesn't allow package upgrades on cell2."))), "Cancel was refused (The fence doesn't allow package upgrades on cell2.); the upgrade keeps running");
        assert_eq!(cancel_said(&Err(SbError::failed("the connection dropped"))), "Couldn't stop it: the connection dropped");

        // And what each comes to: only the node's own answers settle it, and exit 3 is C-X's own
        // only with its words (pkill says 3 too when it fails).
        assert_eq!(stop_came_to(&ok(0, "")), StopTry::Stopped);
        assert_eq!(stop_came_to(&ok(1, "")), StopTry::NothingLeft);
        assert_eq!(stop_came_to(&ok(3, "dpkg is installing packages\n")), StopTry::TooLate);
        assert_eq!(stop_came_to(&ok(3, "pkill: cannot allocate memory\n")), StopTry::Failed);
        assert_eq!(stop_came_to(&ok(2, "")), StopTry::Failed);
        assert_eq!(stop_came_to(&Err(SbError::new(ErrorKind::LookOnly, "Look-only mode is on."))), StopTry::Failed);
        assert_eq!(stop_came_to(&Err(SbError::new(ErrorKind::Fenced, "The fence doesn't allow package upgrades on cell2."))), StopTry::Failed);
        assert_eq!(stop_came_to(&Err(SbError::failed("the connection dropped"))), StopTry::NoAnswer);
        assert_eq!(stop_came_to(&Err(SbError::new(ErrorKind::Unreachable, "cell2 isn't connected"))), StopTry::NoAnswer);
        assert_eq!(stop_came_to(&Ok((None, String::new(), String::new()))), StopTry::NoAnswer);
    }

    /// apt ended before Cancel reached it (pkill matched nothing, exit 1): Cancel says so, and the
    /// upgrade ends as it would have.
    #[tokio::test]
    async fn cancel_after_apt_ended_says_there_was_nothing_to_stop() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("nothing", "2").await;
        node.bin("pkill", "exit 1");
        let id = upgrade(&ops, "upg-nothing", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-nothing", "downloading").await;
        ops.updates_cancel("upg-nothing").unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert!(t.log.iter().any(|l| l == "apt had already finished; nothing to stop."), "{:?}", t.log);
        assert_eq!(t.state, TaskState::Ok, "{:?}", t.error);
        assert_eq!(node.ran("pkill -TERM -P").len(), 1);
    }

    async fn until(what: &str, mut f: impl FnMut() -> bool) {
        for _ in 0..3000 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never: {what}");
    }

    async fn until_logged(ops: &Ops, id: &str, line: &str) {
        until(line, || ops.tasks.get(id).is_some_and(|t| t.log.iter().any(|l| l == line))).await;
    }

    fn run_of(ops: &Ops, task: &str) -> UpgradeRun {
        ops.runs().into_iter().find(|r| r.task_id == task).expect("its run")
    }

    /// A process called dpkg, as the node would have mid-install.
    fn dpkg_running(node: &Node) -> std::process::Child {
        let link = node.home.join("dpkg");
        std::os::unix::fs::symlink("/bin/sleep", &link).unwrap();
        std::process::Command::new(&link).arg("60").spawn().unwrap()
    }

    /// Stop arrives too late (the node says dpkg runs) and then apt fails. dpkg did run, so it's a
    /// failed upgrade, not a stopped one, in the task as in the run; a second press is told why.
    #[tokio::test]
    async fn a_stop_too_late_then_a_failed_apt_reads_failed() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("late", "0.05").await;
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        std::fs::write(node.state_dir.join("apt/fail"), "E: Sub-process /usr/bin/dpkg returned an error code (1)\n").unwrap();
        let mut dpkg = dpkg_running(&node);
        let id = upgrade(&ops, "upg-late", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-late", "downloading").await;
        ops.updates_cancel("upg-late").unwrap();
        until_logged(&ops, &id, DPKG_RUNS).await;
        let again = ops.updates_cancel("upg-late").unwrap_err();
        assert!(again.message.starts_with("Can't stop safely now"), "{}", again.message);
        assert_eq!(node.ran("pkill -TERM -P").len(), 1);
        std::fs::remove_file(node.state_dir.join("apt/hold-download")).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        let _ = dpkg.kill();
        let _ = dpkg.wait();
        assert_eq!(std::fs::read_to_string(node.state_dir.join("apt/upgraded")).unwrap(), "libfoo\n", "dpkg did run");
        assert_eq!((t.state, t.outcome, t.error.as_deref()), (TaskState::Failed, Some(crate::tasks::Outcome::Failed), Some("apt-get failed on upg-late: E: Sub-process /usr/bin/dpkg returned an error code (1)")));
        assert_eq!(run_of(&ops, &id).phase, "failed");
    }

    /// Cancel pressed in Tasks once the log shows dpkg: nothing goes to the node, the task says why,
    /// and when apt then fails it reads Failed.
    #[tokio::test]
    async fn cancel_in_tasks_during_dpkg_then_a_failed_apt_reads_failed() {
        let (ops, node) = setup("tasks-late", "0.4").await;
        std::fs::write(node.state_dir.join("apt/fail"), "E: Sub-process /usr/bin/dpkg returned an error code (1)\n").unwrap();
        let id = upgrade(&ops, "upg-tasks-late", vec!["libfoo".into(), "libbar".into()]).await;
        wait_phase(&ops, "upg-tasks-late", "unpacking").await;
        ops.tasks.cancel(&id).unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert!(t.log.iter().any(|l| l == DPKG_RUNS), "{:?}", t.log);
        assert!(node.ran("pkill -TERM -P").is_empty());
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("apt-get failed on upg-tasks-late: E: Sub-process /usr/bin/dpkg returned an error code (1)")), "{:?}", t.log);
        assert_eq!(run_of(&ops, &id).phase, "failed");
    }

    /// Picked up again it ends the same way: Failed, once the run was there at all.
    #[tokio::test]
    async fn a_picked_up_upgrade_with_a_stop_too_late_reads_failed() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("late-again", "0.05").await;
        ops.set_upgrade_timing(Duration::from_secs(1), UPGRADE_WINDOW);
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        std::fs::write(node.state_dir.join("apt/fail"), "E: Sub-process /usr/bin/dpkg returned an error code (1)\n").unwrap();
        let id = upgrade(&ops, "upg-late-again", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-late-again", "downloading").await;
        fake::away(&node).await;
        let t = wait_task(&ops, &id, 60).await;
        assert_eq!(t.summary.as_deref(), Some("Lost upg-late-again while it was upgrading"), "{:?}", t.log);
        let mut dpkg = dpkg_running(&node);
        fake::back(&node);
        until("the run picked up again", || ops.runs().iter().any(|r| r.task_id != id && r.phase == "downloading")).await;
        let picked = ops.runs().into_iter().find(|r| r.node == "upg-late-again").unwrap().task_id;
        ops.updates_cancel("upg-late-again").unwrap();
        until_logged(&ops, &picked, DPKG_RUNS).await;
        std::fs::remove_file(node.state_dir.join("apt/hold-download")).unwrap();
        let t = wait_task(&ops, &picked, 30).await;
        let _ = dpkg.kill();
        let _ = dpkg.wait();
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("apt-get failed on upg-late-again: E: Sub-process /usr/bin/dpkg returned an error code (1)")), "{:?}", t.log);
    }

    /// Stop fails on the node (pkill errors, as it does out of memory). apt keeps going, the run
    /// and Tasks say so and offer it again, and the second press really goes out.
    #[tokio::test]
    async fn a_stop_that_failed_can_be_pressed_again() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("again", "0.05").await;
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        node.bin("pkill", "#!/bin/sh\necho 'pkill: cannot allocate memory' >&2\nexit 3\n");
        let id = upgrade(&ops, "upg-again", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-again", "downloading").await;
        ops.updates_cancel("upg-again").unwrap();
        let failed = "Couldn't stop it: pkill: cannot allocate memory";
        until_logged(&ops, &id, failed).await;
        until("the press handed back", || !ops.tasks.get(&id).unwrap().cancel_requested).await;
        let run = run_of(&ops, &id);
        assert_eq!((run.phase.as_str(), run.cancellable, run.stop_failed.as_deref()), ("downloading", true, Some(failed)));

        node.bin("pkill", "#!/bin/sh\nexec /usr/bin/pkill \"$@\"\n");
        assert_eq!(ops.updates_cancel("upg-again").unwrap(), "Stopping apt before it installs anything");
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!(node.ran("pkill -TERM -P").len(), 2, "the second press reached the node");
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Cancelled, Some("Stopped before anything was installed on upg-again")), "{:?} {:?}", t.error, t.log);
        assert!(!node.state_dir.join("apt/unpacked").exists(), "nothing was installed");
        let run = run_of(&ops, &id);
        assert_eq!((run.phase.as_str(), run.stop_failed), ("cancelled", None));
    }

    /// The connection drops while Stop is on its way, and apt didn't get it: the task says so, it's
    /// sent again once the node is back, and apt stops. Nobody presses anything twice.
    #[tokio::test]
    async fn a_stop_lost_with_the_connection_is_sent_again_when_the_node_is_back() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("drop", "0.05").await;
        ops.set_upgrade_timing(Duration::from_secs(30), UPGRADE_WINDOW);
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        // The first time it hangs, as a stop that never arrives; then it's the real one.
        let slow = node.state_dir.join("apt/pkill-slow");
        node.bin("pkill", &format!("#!/bin/sh\nif [ ! -f '{s}' ]; then : > '{s}'; sleep 3; exit 0; fi\nexec /usr/bin/pkill \"$@\"\n", s = slow.display()));
        let id = upgrade(&ops, "upg-drop", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-drop", "downloading").await;
        ops.updates_cancel("upg-drop").unwrap();
        until("Stop on the node", || slow.exists()).await;
        fake::away(&node).await;
        until("the lost answer in the log", || ops.tasks.get(&id).unwrap().log.iter().any(|l| l.starts_with("Couldn't stop it:"))).await;
        assert!(run_of(&ops, &id).stop_failed.is_some_and(|w| w.starts_with("Couldn't stop it:")));
        assert!(ops.tasks.get(&id).unwrap().cancel_requested, "still asked for");
        tokio::time::sleep(Duration::from_millis(300)).await;
        fake::back(&node);
        let t = wait_task(&ops, &id, 60).await;
        assert_eq!(node.ran("pkill -TERM -P").len(), 2, "{:?}", t.log);
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Cancelled, Some("Stopped before anything was installed on upg-drop")), "{:?} {:?}", t.error, t.log);
        assert!(!node.state_dir.join("apt/unpacked").exists(), "nothing was installed");
        assert_eq!(run_of(&ops, &id).stop_failed, None);
    }

    /// Stop reached apt and only its answer was lost: the next try finds nothing to stop, and apt
    /// failed with no line from dpkg, so the first one got there. It reads stopped, not failed.
    #[tokio::test]
    async fn a_stop_that_reached_apt_with_its_answer_lost_reads_stopped() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("reached", "0.05").await;
        ops.set_upgrade_timing(Duration::from_secs(30), UPGRADE_WINDOW);
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        let slow = node.state_dir.join("apt/pkill-slow");
        node.bin("pkill", &format!("#!/bin/sh\n/usr/bin/pkill \"$@\"; c=$?\nif [ ! -f '{s}' ]; then : > '{s}'; sleep 3; fi\nexit $c\n", s = slow.display()));
        let id = upgrade(&ops, "upg-reached", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-reached", "downloading").await;
        ops.updates_cancel("upg-reached").unwrap();
        until("Stop on the node", || slow.exists()).await;
        fake::away(&node).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        fake::back(&node);
        let t = wait_task(&ops, &id, 60).await;
        assert!(t.log.iter().any(|l| l == "apt had already finished; nothing to stop."), "{:?}", t.log);
        assert_eq!((t.state, t.summary.as_deref(), t.error.as_deref()), (TaskState::Cancelled, Some("Stopped before anything was installed on upg-reached"), None), "{:?}", t.log);
        assert_eq!(run_of(&ops, &id).phase, "cancelled");
        assert!(!node.state_dir.join("apt/unpacked").exists(), "nothing was installed");
    }

    /// The node stays connected and Stop never gets an answer (the command dies on a signal):
    /// three tries, then the press is handed back and the window says so. It can be pressed again.
    #[tokio::test]
    async fn a_stop_with_no_answer_is_tried_three_times_then_handed_back() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("noanswer", "0.05").await;
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        node.bin("pkill", "#!/bin/sh\nkill -KILL $$\n");
        let id = upgrade(&ops, "upg-noanswer", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-noanswer", "downloading").await;
        ops.updates_cancel("upg-noanswer").unwrap();
        until("the press handed back", || !ops.tasks.get(&id).unwrap().cancel_requested).await;
        let t = ops.tasks.get(&id).unwrap();
        let tries: Vec<&String> = t.log.iter().filter(|l| l.starts_with("Couldn't stop it:")).collect();
        assert_eq!(tries.len(), 3, "{:?}", t.log);
        assert_eq!(t.log.iter().filter(|l| l.starts_with("Trying again when")).count(), 2, "{:?}", t.log);
        assert_eq!(run_of(&ops, &id).stop_failed.as_ref(), Some(tries[2]));
        assert_eq!(node.ran("pkill -TERM -P").len(), 3);

        node.bin("pkill", "#!/bin/sh\nexec /usr/bin/pkill \"$@\"\n");
        ops.updates_cancel("upg-noanswer").unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Cancelled, Some("Stopped before anything was installed on upg-noanswer")), "{:?} {:?}", t.error, t.log);
        assert_eq!(run_of(&ops, &id).stop_failed, None);
    }

    /// A Stop that went unanswered doesn't excuse a failed install: apt went on to dpkg, so the
    /// upgrade reads Failed.
    #[tokio::test]
    async fn a_lost_stop_does_not_hide_a_failed_install() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("lost-install", "0.05").await;
        ops.set_upgrade_timing(Duration::from_secs(30), UPGRADE_WINDOW);
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        std::fs::write(node.state_dir.join("apt/fail"), "E: Sub-process /usr/bin/dpkg returned an error code (1)\n").unwrap();
        let slow = node.state_dir.join("apt/pkill-slow");
        node.bin("pkill", &format!("#!/bin/sh\nif [ ! -f '{s}' ]; then : > '{s}'; sleep 3; exit 0; fi\nexit 1\n", s = slow.display()));
        let id = upgrade(&ops, "upg-lost-install", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-lost-install", "downloading").await;
        ops.updates_cancel("upg-lost-install").unwrap();
        until("Stop on the node", || slow.exists()).await;
        fake::away(&node).await;
        // apt goes on to dpkg, and fails, while the node is away.
        std::fs::remove_file(node.state_dir.join("apt/hold-download")).unwrap();
        until("apt's end", || std::fs::read_dir(node.state_dir.join("units")).unwrap().next().is_none()).await;
        fake::back(&node);
        let t = wait_task(&ops, &id, 60).await;
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some("apt-get failed on upg-lost-install: E: Sub-process /usr/bin/dpkg returned an error code (1)")), "{:?}", t.log);
        assert_eq!(run_of(&ops, &id).phase, "failed");
    }

    /// apt's exit line can reach the tail before Stop's answer reaches SparkBox. The answer still
    /// counts: the stopped apt doesn't read as a failed one.
    #[tokio::test]
    async fn apt_s_exit_line_beating_stop_s_answer_still_reads_stopped() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("beaten", "0.05").await;
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        node.bin("pkill", "#!/bin/sh\n/usr/bin/pkill \"$@\"; c=$?\nsleep 1.5\nexit $c\n");
        let id = upgrade(&ops, "upg-beaten", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-beaten", "downloading").await;
        ops.updates_cancel("upg-beaten").unwrap();
        let t = wait_task(&ops, &id, 30).await;
        assert_eq!((t.state, t.summary.as_deref(), t.error.as_deref()), (TaskState::Cancelled, Some("Stopped before anything was installed on upg-beaten"), None), "{:?}", t.log);
        assert_eq!(run_of(&ops, &id).phase, "cancelled");
        assert_eq!(node.ran("pkill -TERM -P").len(), 1);
        assert!(t.log.iter().any(|l| l == "Stopping apt before it installs anything…"), "{:?}", t.log);
    }

    /// A Stop that worked, then the node restarts before apt's end reaches the log: the task fails
    /// with what the audit found, and the run says the same.
    #[tokio::test]
    async fn a_failure_after_a_stop_that_worked_reads_failed_in_the_task_and_the_run() {
        let _turn = PGREP.lock().await;
        let (ops, node) = setup("stop-restart", "0.05").await;
        ops.set_upgrade_timing(Duration::from_secs(20), UPGRADE_WINDOW);
        std::fs::write(node.state_dir.join("apt/hold-download"), "").unwrap();
        // The stop is under way: pkill has answered and apt hasn't ended yet.
        node.bin("pkill", "exit 0");
        let id = upgrade(&ops, "upg-stop-restart", vec!["libfoo".into()]).await;
        wait_phase(&ops, "upg-stop-restart", "downloading").await;
        ops.updates_cancel("upg-stop-restart").unwrap();
        wait_phase(&ops, "upg-stop-restart", "cancelled").await;
        fake::restart(&node, None).await;
        let t = wait_task(&ops, &id, 60).await;
        assert_eq!(t.state, TaskState::Failed, "{:?} {:?}", t.error, t.log);
        assert!(t.error.as_deref().unwrap_or_default().starts_with("apt on upg-stop-restart stopped without an exit status"), "{:?}", t.error);
        assert_eq!(run_of(&ops, &id).phase, "failed");
    }
}
