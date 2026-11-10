//! Bounded November 2026 system-update transaction. This is not a generic GPU guard override.
//! Only reviewed 1024/1025 plans are admitted, with separate fresh and continuation paths. Versions
//! are pinned, the 580 driver stays put, and one durable reservation survives until a healthy
//! new boot is proved. No reboot, hold release, autoremove, firmware flashing or downgrade.
use super::{
    updates::{self, PlanEntry, PlannedPkg, SimulatedUpgrade, UpgradeArgs, UpgradeOpts},
    Ops,
};
use crate::{errors::SbError, live::Live};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

pub const KERNEL: &str = "7.0.0-1024-nvidia";
pub const OLD_KERNEL: &str = "7.0.0-1019-nvidia";
pub const DRIVER: &str = "580.178.04";
const KV: &str = "7.0.0-1024.25~24.04.1";
const OLD_KV: &str = "7.0.0-1019.19~24.04.2";
static ADMISSION: Mutex<()> = Mutex::new(());
pub const CONTINUED_KERNEL: &str = "7.0.0-1025-nvidia";
const CONTINUED_KV: &str = "7.0.0-1025.26~24.04.1";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Review {
    Initial,
    Kernel1025,
    Fleet1025,
}
impl Review {
    fn kernel(self) -> &'static str {
        if self == Self::Initial {
            KERNEL
        } else {
            CONTINUED_KERNEL
        }
    }
    pub(super) fn packages(self) -> Vec<PlannedPkg> {
        match self {
            Self::Initial => reviewed(),
            Self::Kernel1025 => reviewed_1025(),
            Self::Fleet1025 => reviewed_fleet_1025(),
        }
    }
    fn pins(self) -> Vec<String> {
        self.packages().iter().map(|p| format!("{}={}", p.name, p.to)).collect()
    }
    fn installed(self) -> Vec<PlannedPkg> {
        if self == Self::Fleet1025 { return self.packages(); }
        let mut all = reviewed();
        if self == Self::Kernel1025 {
            for next in reviewed_1025() {
                all.retain(|old| old.name != next.name);
                all.push(next);
            }
        }
        all
    }
    pub(super) fn check_plan(self, sim: &SimulatedUpgrade) -> Result<(), String> {
        check_plan_for(sim, self)
    }
    pub(super) fn check_origin(self, sim: &SimulatedUpgrade) -> Result<(), String> {
        if self != Self::Initial {
            let rows: Vec<_> = sim.text.lines().filter(|l| l.starts_with("Inst ")).collect();
            if rows.len() != self.packages().len()
                || rows.iter().any(|line| {
                    let name = line.split_whitespace().nth(1).unwrap_or("");
                    let arch = if matches!(name, "linux-tools-common" | "linux-nvidia-7.0-headers-7.0.0-1025" | "cuda-toolkit-13-config-common" | "cuda-toolkit-config-common" | "linux-firmware-amd-graphics") { "all" } else { "arm64" };
                    let archive = match name {
                        "cuda-toolkit-13-config-common" | "cuda-toolkit-config-common" | "libxnvctrl0" | "nvidia-modprobe" | "nvidia-settings" => "NVIDIA CUDA:developer.download.nvidia.com",
                        "nvidia-spark-initcall-bl" => "NVIDIA Spark Updates Repository:noble-updates",
                        _ => "Ubuntu:24.04/noble-updates",
                    };
                    let origin = line.split_once(" (").and_then(|(_, tail)| tail.split_once(')')).map(|(value, _)| value);
                    let expected = self.packages().into_iter().find(|p| p.name == name).map(|p| format!("{} {archive} [{arch}]", p.to));
                    origin != expected.as_deref()
                })
            {
                return Err("The 1025 package origins differ from the reviewed archives and architectures.".into());
            }
        }
        Ok(())
    }
}
fn fleet_node(node: &str) -> bool {
    matches!(node, "cell1" | "cell3" | "gx10-0000")
}
pub fn reviewed_fleet_1025() -> Vec<PlannedPkg> {
    let mut plan = reviewed();
    for p in &mut plan {
        if p.to == KV {
            p.name = p.name.replace("1024", "1025");
            p.to = CONTINUED_KV.into();
        }
    }
    plan.extend(reviewed_1025().into_iter().filter(|p| p.to != CONTINUED_KV));
    plan
}
pub fn reviewed_1025() -> Vec<PlannedPkg> {
    let mut p: Vec<_> = reviewed()
        .into_iter()
        .filter(|p| p.to == KV)
        .map(|p| PlannedPkg { name: p.name.replace("1024", "1025"), from: if p.new { None } else { Some(KV.into()) }, to: CONTINUED_KV.into(), new: p.new })
        .collect();
    for name in ["linux-crashdump", "linux-libc-dev", "linux-tools-common"] {
        p.push(PlannedPkg { name: name.into(), from: Some("6.8.0-147.147".into()), to: "6.8.0-149.149".into(), new: false });
    }
    p
}
fn shown_matches(plan: &[PlanEntry], review: Review) -> bool {
    let want = review.packages();
    plan.len() == want.len() && want.iter().all(|p| plan.iter().filter(|x| x.name == p.name && x.to == p.to).count() == 1)
}

pub fn reviewed() -> Vec<PlannedPkg> {
    let mut p = Vec::new();
    for n in [
        "linux-headers-nvidia-hwe-24.04",
        "linux-image-nvidia-hwe-24.04",
        "linux-modules-nvidia-580-open-nvidia-hwe-24.04",
        "linux-modules-nvidia-fs-nvidia-hwe-24.04",
        "linux-nvidia-hwe-24.04",
        "linux-tools-nvidia-hwe-24.04",
    ] {
        p.push(PlannedPkg {
            name: n.into(),
            from: Some(if n.contains("580-open") { format!("{OLD_KV}+2") } else { OLD_KV.into() }),
            to: KV.into(),
            new: false,
        });
    }
    for (n, from, to) in [
        ("cuda-toolkit-13-config-common", "13.4.92-1", "13.5.37-1"),
        ("cuda-toolkit-config-common", "13.4.92-1", "13.5.37-1"),
        ("libxnvctrl0", "615.71.10-1ubuntu1", "620.70.06-1ubuntu1"),
        ("nvidia-modprobe", "615.71.10-1ubuntu1", "620.70.06-1ubuntu1"),
        ("nvidia-settings", "615.71.10-1ubuntu1", "620.70.06-1ubuntu1"),
        ("nvidia-spark-initcall-bl", "1.0-1", "1.0-2"),
        ("linux-firmware-amd-graphics", "20240318.git3b128b60-0ubuntu3.3", "20240318.git3b128b60-0ubuntu3.4"),
    ] {
        p.push(PlannedPkg { name: n.into(), from: Some(from.into()), to: to.into(), new: false });
    }
    for n in [
        "linux-nvidia-7.0-headers-7.0.0-1024",
        "linux-headers-7.0.0-1024-nvidia",
        "linux-modules-7.0.0-1024-nvidia",
        "linux-image-7.0.0-1024-nvidia",
        "linux-modules-nvidia-580-open-7.0.0-1024-nvidia",
        "linux-modules-nvidia-fs-7.0.0-1024-nvidia",
        "linux-nvidia-7.0-tools-7.0.0-1024",
        "linux-tools-7.0.0-1024-nvidia",
    ] {
        p.push(PlannedPkg { name: n.into(), from: None, to: KV.into(), new: true });
    }
    p
}
#[cfg(test)]
fn pins() -> Vec<String> {
    Review::Initial.pins()
}
/// Also used by the write fence: this new install shape cannot become an arbitrary installer.
pub fn allowed_pins(p: &[&str]) -> bool {
    [Review::Initial, Review::Kernel1025, Review::Fleet1025].iter().any(|review| {
        let want = review.pins();
        p.len() == want.len() && want.iter().all(|w| p.iter().filter(|x| **x == w).count() == 1)
    })
}
#[cfg(test)]
pub fn check_plan(sim: &SimulatedUpgrade) -> Result<(), String> {
    check_plan_for(sim, Review::Initial)
}
fn check_plan_for(sim: &SimulatedUpgrade, review: Review) -> Result<(), String> {
    if !sim.removes.is_empty() || !sim.removals.is_empty() || !sim.held_back.is_empty() || !sim.phased.is_empty() {
        return Err("System updates cannot remove, hold back or defer packages.".into());
    }
    let want = review.packages();
    if sim.plan.len() != want.len() || want.iter().any(|p| sim.plan.iter().filter(|x| *x == p).count() != 1) {
        return Err("The system-update plan differs from the exact reviewed versions. Nothing will be installed. Refresh package lists and review the plan; a changed kernel or driver needs a new compatibility review.".into());
    }
    Ok(())
}
fn opts() -> UpgradeOpts {
    UpgradeOpts { protected: true, ..Default::default() }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub node: String,
    pub demo: bool,
    pub instance: String,
    pub phase: String,
    pub boot: String,
    pub task: Option<String>,
    pub plan: Vec<PlanEntry>,
    pub message: String,
    /// Entire prior checkpoint, including its task, baseline and exact original plan.
    /// Absent on legacy records. Older binaries reject this field rather than losing history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<Record>>,
    /// Separate from cell2's continuation: direct 1019-to-1025 on the remaining stable IDs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fresh1025: bool,
    /// Flattened, complete verified records. New admissions never erase completed evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed: Vec<Record>,
}
impl Record {
    fn review(&self) -> Review {
        if self.fresh1025 {
            Review::Fleet1025
        } else if self.previous.is_some() {
            Review::Kernel1025
        } else {
            Review::Initial
        }
    }
    fn admits_continuation(&self, node: &str) -> Result<(), String> {
        if node != "cell2"
            || self.node != node
            || self.previous.is_some()
            || self.fresh1025
            || self.phase != "installing"
            || self.boot.is_empty()
            || self.task.as_ref().is_none_or(|t| t.is_empty())
            || !shown_matches(&self.plan, Review::Initial)
        {
            return Err("The 1025 continuation requires cell2's original installed 1024 checkpoint. Keep the reservation and inspect Tasks.".into());
        }
        Ok(())
    }
    fn active(&self) -> bool {
        !matches!(self.phase.as_str(), "verified" | "refused")
    }
}
fn path(live: &Live, demo: bool) -> std::path::PathBuf {
    live.data_path(if demo { "system-update-demo.json" } else { "system-update.json" })
}
fn read(live: &Live, demo: bool) -> Result<Option<Record>, String> {
    let p = path(live, demo);
    if !p.exists() {
        return Ok(None);
    }
    let r: Record = serde_json::from_slice(&std::fs::read(p).map_err(|e| e.to_string())?)
        .map_err(|_| "The system-update checkpoint is unreadable; review it before maintenance.")?;
    validate_record(&r, demo, 0)?;
    Ok(Some(r))
}
fn validate_record(r: &Record, demo: bool, depth: usize) -> Result<(), String> {
    if depth > 8 || r.demo != demo || r.instance != crate::identity::instance_name() {
        return Err("The system-update checkpoint belongs to another source or instance.".into());
    }
    if r.fresh1025 && (!fleet_node(&r.node) || r.previous.is_some() || !shown_matches(&r.plan, Review::Fleet1025)) {
        return Err("The fresh 1025 checkpoint does not match its reviewed node or plan.".into());
    }
    if let Some(old) = &r.previous {
        old.admits_continuation(&r.node)?;
        if old.demo != r.demo || old.instance != r.instance || !shown_matches(&r.plan, Review::Kernel1025) {
            return Err("The continuation checkpoint does not match its original owner, source or reviewed plan.".into());
        }
        validate_record(old, demo, depth + 1)?;
    }
    for completed in &r.completed {
        if completed.phase != "verified" || !completed.completed.is_empty() {
            return Err("Completed system-update history is inconsistent; keep the checkpoint for review.".into());
        }
        validate_record(completed, demo, depth + 1)?;
    }
    Ok(())
}
fn save(live: &Live, r: &Record) -> Result<(), String> {
    crate::util::save_json(&path(live, r.demo), r).map_err(|e| format!("Couldn't save the system-update checkpoint: {e}"))
}
pub fn check_load(live: &Live, demo: bool, nodes: &[String]) -> Result<(), String> {
    if read(live, demo)?.is_some_and(|r| r.active() && nodes.contains(&r.node)) {
        return Err("System updates reserve this node until its new boot passes verification in Software Updates.".into());
    }
    Ok(())
}
pub fn check_routine(live: &Live, demo: bool, node: &str) -> Result<(), SbError> {
    check_load(live, demo, &[node.into()]).map_err(SbError::invalid)
}

// A fixed read-only probe. Each fallible read must succeed; blank output is not healthy.
pub const BEFORE: &str = "echo '--- os'; cat /etc/os-release || exit 1; echo '--- arch'; dpkg --print-architecture || exit 1; echo '--- boot'; cat /proc/sys/kernel/random/boot_id || exit 1; echo '--- kernel'; uname -r || exit 1; echo '--- gpu'; nvidia-smi --query-gpu=name,driver_version --format=csv,noheader || exit 1; echo '--- jobs'; nvidia-smi --query-compute-apps=pid --format=csv,noheader || exit 1; echo '--- containers'; docker ps -q || exit 1; echo '--- space'; df -Pk / /boot || exit 1; echo '--- audit'; dpkg --audit || exit 1; echo '--- dependencies'; apt-get -s check || exit 1; echo '--- end'";
// Unprivileged checks must not acquire apt's root-only lock. The installer still locks and
// runs the real apt-get check as root. GRUB itself is root:root 0600 on the live nodes;
// these fixed reads use the existing authenticated sudo-probe path, never relaxed modes.
const DEPENDENCIES: &str = "apt-get -s check";
pub(crate) const RECOVERY_GRUB: &str = "test -s /boot/vmlinuz-7.0.0-1019-nvidia && grep -F -- 7.0.0-1019-nvidia /boot/grub/grub.cfg";
pub(crate) const NEXT_GRUB: &str = "grep -F -- vmlinuz-7.0.0-1024-nvidia /boot/grub/grub.cfg";
pub(crate) const RECOVERY_1024_GRUB: &str = "test -s /boot/vmlinuz-7.0.0-1024-nvidia && grep -F -- vmlinuz-7.0.0-1024-nvidia /boot/grub/grub.cfg";
pub(crate) const CONTINUED_GRUB: &str = "test -s /boot/vmlinuz-7.0.0-1025-nvidia && grep -F -- vmlinuz-7.0.0-1025-nvidia /boot/grub/grub.cfg";
pub(crate) const RECOVERY_SETTING: &str = "cat /etc/default/grub.d/zzzz-sparkbox-recovery-initcall.cfg";
const RECOVERY_SETTING_TEXT: &str = include_str!("protected_recovery.cfg");
fn recovery_entries(text: &str, kernel: &str, require_fix: bool) -> bool {
    let lines: Vec<Vec<_>> = text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|v| v.first().is_some_and(|w| matches!(*w, "linux" | "linuxefi"))).collect();
    let recovery = |v: &Vec<&str>| v.iter().any(|w| matches!(*w, "recovery" | "single"));
    !lines.is_empty() && lines.iter().any(recovery) && lines.iter().any(|v| !recovery(v))
        && lines.iter().all(|v| v.get(1).is_some_and(|p| p.ends_with(&format!("/vmlinuz-{kernel}")))
            && (!require_fix || v.iter().any(|w| w.strip_prefix("initcall_blacklist=").is_some_and(|s| s.split(',').any(|s| s == "power_wrap_init")))))
}
fn sections(s: &str) -> std::collections::BTreeMap<&str, String> {
    let mut m = std::collections::BTreeMap::new();
    let mut k = "";
    for l in s.lines() {
        if let Some(next) = l.strip_prefix("--- ") {
            k = next;
            m.entry(k).or_insert_with(String::new);
        } else {
            let v = m.entry(k).or_insert_with(String::new);
            v.push_str(l);
            v.push('\n');
        }
    }
    m
}
#[cfg(test)]
fn baseline(s: &str) -> Result<String, String> {
    baseline_for(s, OLD_KERNEL)
}
fn baseline_for(s: &str, kernel: &str) -> Result<String, String> {
    let m = sections(s);
    let get = |k| m.get(k).map(|s| s.trim()).unwrap_or("");
    if !m.contains_key("end")
        || !get("os").lines().any(|l| l == "ID=ubuntu")
        || !get("os").lines().any(|l| l == "VERSION_ID=\"24.04\"")
        || get("arch") != "arm64"
        || get("kernel") != kernel
        || get("gpu") != format!("NVIDIA GB10, {DRIVER}")
    {
        return Err(format!(
            "This reviewed update requires Ubuntu 24.04 arm64, a GB10, kernel {kernel} and driver 580.178.04. Missing or different evidence blocks it."
        ));
    }
    if get("boot").len() != 36 || !get("boot").bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err("Could not read the current boot identity.".into());
    }
    for k in ["jobs", "containers", "audit"] {
        if !m.contains_key(k) || !get(k).is_empty() {
            return Err(format!("System updates require an idle node and a clean package database ({k}). Stop workloads and resolve the finding first."));
        }
    }
    if !m.contains_key("dependencies") || get("dependencies").lines().any(|l| l.starts_with("E:")) {
        return Err("Package dependencies could not be verified.".into());
    }
    let space: Vec<_> = get("space")
        .lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<_> = l.split_whitespace().collect();
            Some((*c.last()?, c.get(3)?.parse::<u64>().ok()?))
        })
        .collect();
    if space.len() != 2 || space[0].1 < 8 * 1024 * 1024 || space[1].1 < 1024 * 1024 {
        return Err("System updates need at least 8 GiB free on / and 1 GiB on the filesystem containing /boot.".into());
    }
    Ok(get("boot").into())
}
#[cfg(test)]
fn installed_command() -> String {
    installed_command_for(Review::Initial)
}
fn installed_command_for(review: Review) -> String {
    format!(
        "dpkg-query -W -f='${{Package}} ${{db:Status-Abbrev}} ${{Version}}\\n' {}",
        review.installed().iter().map(|p| crate::live::sh_quote(&p.name)).collect::<Vec<_>>().join(" ")
    )
}
#[cfg(test)]
fn installed_ok(s: &str) -> bool {
    installed_ok_for(s, Review::Initial)
}
fn installed_ok_for(s: &str, review: Review) -> bool {
    let rows: Vec<_> = s.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).collect();
    let want = review.installed();
    rows.len() == want.len() && want.iter().all(|p| rows.iter().filter(|r| r.as_slice() == [p.name.as_str(), "ii", p.to.as_str()]).count() == 1)
}
impl Ops {
    pub fn system_discard_review(&self, demo: bool) -> Result<Record, String> {
        let _guard = ADMISSION.lock().unwrap();
        let mut r = read(&self.live, demo)?.ok_or("No interrupted review to discard.")?;
        if r.phase != "checking"
            || !r.boot.is_empty()
            || r.task.is_some()
            || self.runs().iter().any(|run| run.node == r.node && run.simulated == demo && !matches!(run.phase.as_str(), "done" | "failed" | "cancelled"))
        {
            return Err(
                "This review is running or an installer may have been sent. Its reservation cannot be discarded; inspect Tasks and verify the node.".into()
            );
        }
        if let Some(old) = r.previous.take() {
            save(&self.live, &old)?;
            return Ok(*old);
        }
        r.phase = "refused".into();
        r.message = "Discarded an interrupted review before any installer could be sent. Review the plan again.".into();
        save(&self.live, &r)?;
        Ok(r)
    }
    pub fn system_status(&self, demo: bool) -> Result<Option<Record>, String> {
        read(&self.live, demo)
    }
    pub async fn system_preview(&self, demo: bool, node: &str) -> Result<SimulatedUpgrade, SbError> {
        self.system_idle_run(demo, node, true).map_err(SbError::invalid)?;
        if let Some(r) = read(&self.live, demo).map_err(SbError::invalid)?.filter(Record::active) {
            r.admits_continuation(node).map_err(SbError::invalid)?;
            let sim = self.system_plan_for(demo, node, Review::Kernel1025, Some(&r)).await?;
            if read(&self.live, demo).map_err(SbError::invalid)?.as_ref() != Some(&r) {
                return Err(SbError::invalid("The checkpoint changed during review. Review it again."));
            }
            return Ok(sim);
        }
        self.system_plan(demo, node).await
    }
    async fn system_plan(&self, demo: bool, node: &str) -> Result<SimulatedUpgrade, SbError> {
        let review = if fleet_node(node) { Review::Fleet1025 } else if node == "cell2" { Review::Initial } else { return Err(SbError::invalid("This node has no reviewed protected-update plan.")); };
        self.system_plan_for(demo, node, review, None).await
    }
    async fn system_plan_for(&self, demo: bool, node: &str, review: Review, original: Option<&Record>) -> Result<SimulatedUpgrade, SbError> {
        self.system_idle_run(demo, node, false).map_err(SbError::invalid)?;
        let dry = updates::pinned_dry_run_opts(&review.pins(), &opts());
        let mut sim = if demo {
            let mut s = SimulatedUpgrade { node: node.into(), plan: review.packages(), ..Default::default() };
            s.upgrades = s.plan.iter().filter(|p| !p.new).map(|p| p.name.clone()).collect();
            s.installs = s.plan.iter().filter(|p| p.new).map(|p| p.name.clone()).collect();
            s
        } else {
            let status = self.updates_status(false, node).await?;
            if status.held.iter().any(|n| review.installed().iter().any(|p| p.name == *n)) {
                return Err(SbError::invalid("A reviewed system package is held. SparkBox does not release holds for this update."));
            }
            let out = self.live.run(node, &dry, None, Duration::from_secs(120)).await.map_err(|e| SbError::classify(&e))?;
            if out.code != Some(0) {
                return Err(SbError::invalid("The exact system-update dry run failed; nothing was installed.").with_detail(out.stderr));
            }
            updates::parse_simulation(node, &out.stdout)
        };
        review.check_plan(&sim).map_err(SbError::invalid)?;
        if !demo {
            review.check_origin(&sim).map_err(SbError::invalid)?;
            if review == Review::Kernel1025 {
                self.continuation_baseline(node, original.ok_or_else(|| SbError::invalid("Original checkpoint is missing."))?).await?;
            } else if review == Review::Fleet1025 {
                self.fleet_baseline(node).await?;
            } else {
                self.system_baseline(node).await?;
            }
        }
        sim.dry_run = dry;
        sim.command = updates::apt_command_opts(&review.pins(), &opts());
        Ok(sim)
    }
    async fn system_baseline(&self, node: &str) -> Result<String, SbError> {
        let o = self.live.run(node, BEFORE, None, Duration::from_secs(60)).await.map_err(|e| SbError::classify(&e))?;
        if o.code != Some(0) {
            return Err(SbError::invalid("The system-update safety read failed; nothing was installed.").with_detail(o.stderr));
        }
        let boot = baseline_for(&o.stdout, OLD_KERNEL).map_err(SbError::invalid)?;
        let old = self.live.run(node, &super::prereboot::script(OLD_KERNEL), None, Duration::from_secs(30)).await.map_err(|e| SbError::classify(&e))?;
        let checked = super::prereboot::parse(node, OLD_KERNEL, &old.stdout);
        if old.code != Some(0) || !checked.ok {
            return Err(SbError::invalid("The previous kernel's initrd, matching GPU module and userspace must be healthy before updating."));
        }
        let out = self.live.sudo(node, RECOVERY_GRUB, Duration::from_secs(20)).await.map_err(|e| SbError::classify(&e))?;
        if out.code != Some(0) || !out.stdout.contains(OLD_KERNEL) {
            return Err(SbError::invalid("The previous kernel and its GRUB recovery entry could not be confirmed. Arrange local boot recovery first."));
        }
        Ok(boot)
    }
    pub fn system_start(self: &Arc<Self>, demo: bool, node: String, plan: Vec<PlanEntry>) -> Result<String, SbError> {
        let _guard = ADMISSION.lock().unwrap();
        let prior = read(&self.live, demo).map_err(SbError::invalid)?;
        let review = if shown_matches(&plan, Review::Kernel1025) { Review::Kernel1025 } else if shown_matches(&plan, Review::Fleet1025) { Review::Fleet1025 } else { Review::Initial };
        if !shown_matches(&plan, review) {
            return Err(SbError::invalid(updates::DRY_RUN_FIRST));
        }
        if (review == Review::Fleet1025 && !fleet_node(&node)) || (review == Review::Initial && node != "cell2") {
            return Err(SbError::invalid("This exact plan is not reviewed for the selected node."));
        }
        let mut completed = prior.as_ref().map(|r| r.completed.clone()).unwrap_or_default();
        if let Some(old) = prior.as_ref().filter(|r| r.phase == "verified") {
            let mut old = old.clone();
            old.completed.clear();
            completed.push(old);
        }
        let previous = if review == Review::Kernel1025 {
            let old = prior.ok_or_else(|| SbError::invalid("The original 1024 checkpoint is required."))?;
            old.admits_continuation(&node).map_err(SbError::invalid)?;
            Some(Box::new(old))
        } else {
            if prior.is_some_and(|r| r.active()) {
                return Err(SbError::invalid("Finish the reserved node's system-update verification first."));
            }
            None
        };
        let mut record = Record {
            node: node.clone(),
            demo,
            instance: crate::identity::instance_name().into(),
            phase: "checking".into(),
            plan: plan.clone(),
            previous,
            fresh1025: review == Review::Fleet1025,
            completed,
            message: "Checking the reviewed system update. No restart is automatic.".into(),
            ..Default::default()
        };
        save(&self.live, &record).map_err(SbError::invalid)?;
        match self.updates_upgrade_impl(
            demo,
            node,
            review.packages().iter().filter(|p| !p.new).map(|p| p.name.clone()).collect(),
            UpgradeArgs { plan: Some(plan), ..Default::default() },
            true,
        ) {
            Ok(id) => Ok(id),
            Err(e) => {
                if let Some(old) = record.previous.take() {
                    record = *old;
                } else {
                    record.phase = "refused".into();
                    record.message = e.message.clone();
                }
                save(&self.live, &record).map_err(SbError::invalid)?;
                Err(e)
            }
        }
    }
    /// Called again inside the admitted task, immediately before the exact dry run/root start.
    pub(super) async fn system_prepare(&self, demo: bool, node: &str, task: &str) -> Result<Review, SbError> {
        let before = read(&self.live, demo).map_err(SbError::invalid)?.ok_or_else(|| SbError::invalid("System-update reservation disappeared."))?;
        if before.node != node || before.phase != "checking" {
            return Err(SbError::invalid("System-update reservation changed."));
        }
        let review = before.review();
        self.system_plan_for(demo, node, review, before.previous.as_deref()).await?;
        let boot = if demo {
            "demo-before".into()
        } else if let Some(old) = before.previous.as_deref() {
            self.continuation_baseline(node, old).await?
        } else if review == Review::Fleet1025 {
            self.fleet_baseline(node).await?
        } else {
            self.system_baseline(node).await?
        };
        let _guard = ADMISSION.lock().unwrap();
        if read(&self.live, demo).map_err(SbError::invalid)?.as_ref() != Some(&before) {
            return Err(SbError::invalid("System-update reservation changed."));
        }
        let mut r = before;
        r.boot = boot;
        r.task = Some(task.into());
        r.phase = "installing".into();
        r.message = "Follow the package task. Then restart this node explicitly and Check system update. Other system updates and model loads on this node wait for verification.".into();
        save(&self.live, &r).map_err(SbError::invalid)?;
        Ok(review)
    }
    pub(super) fn system_not_started(&self, demo: bool, node: &str, task: &str, message: &str) {
        let _guard = ADMISSION.lock().unwrap();
        if let Ok(Some(mut r)) = read(&self.live, demo) {
            if r.node == node && r.active() && r.task.as_deref().is_none_or(|id| id == task) {
                if let Some(old) = r.previous.take() {
                    r = *old;
                } else {
                    r.phase = "refused".into();
                    r.message = message.into();
                }
                let _ = save(&self.live, &r);
            }
        }
    }
    fn system_idle_run(&self, demo: bool, node: &str, include_checking: bool) -> Result<(), String> {
        if self.runs().iter().any(|run| {
            run.node == node
                && run.simulated == demo
                && !matches!(run.phase.as_str(), "done" | "failed" | "cancelled")
                && (include_checking || run.phase != "checking")
        }) {
            return Err("The package task has not finished. Follow it in Tasks; do not restart yet.".into());
        }
        Ok(())
    }
    async fn continuation_baseline(&self, node: &str, old: &Record) -> Result<String, SbError> {
        old.admits_continuation(node).map_err(SbError::invalid)?;
        let o = self.live.run(node, BEFORE, None, Duration::from_secs(60)).await.map_err(|e| SbError::classify(&e))?;
        if o.code != Some(0) {
            return Err(SbError::invalid("The continuation safety read failed."));
        }
        let boot = baseline_for(&o.stdout, KERNEL).map_err(SbError::invalid)?;
        self.boot_ready_for(node, Review::Initial).await.map_err(SbError::invalid)?;
        self.check_grub_fix(node, RECOVERY_1024_GRUB).await.map_err(SbError::invalid)?;
        let health_boot = self.system_health(node, KERNEL, &old.boot).await.map_err(SbError::invalid)?;
        if health_boot != boot {
            return Err(SbError::invalid("Cell2 restarted during review. Wait for fresh readings and review again."));
        }
        Ok(boot)
    }
    async fn fleet_baseline(&self, node: &str) -> Result<String, SbError> {
        if !fleet_node(node) { return Err(SbError::invalid("This node has no fresh 1025 plan.")); }
        let boot = self.system_baseline(node).await?;
        let health = self.live.run(node, super::postboot::READ, None, Duration::from_secs(60)).await.map_err(|e| SbError::classify(&e))?;
        let warnings = super::postboot::judge_running(&health.stdout, &super::postboot::Expected {
            boot: Some(boot.clone()), kernel: Some(OLD_KERNEL.into()), driver: Some(DRIVER.into()), ..Default::default()
        });
        if health.code != Some(0) || !warnings.is_empty() {
            return Err(SbError::invalid(format!("Startup and health must pass before installation: {}. Resolve startup blockers such as Plymouth first; no packages were installed.", warnings.join("; "))));
        }
        let setting = self.live.sudo(node, RECOVERY_SETTING, Duration::from_secs(20)).await.map_err(|e| SbError::classify(&e))?;
        if setting.code != Some(0) || setting.stdout != RECOVERY_SETTING_TEXT {
            return Err(SbError::invalid("The reviewed persistent recovery-boot setting is missing or changed. A separately reviewed boot-setting repair is required before installing; SparkBox has not changed it."));
        }
        let grub = self.live.sudo(node, RECOVERY_GRUB, Duration::from_secs(20)).await.map_err(|e| SbError::classify(&e))?;
        if grub.code != Some(0) || !recovery_entries(&grub.stdout, OLD_KERNEL, false) {
            return Err(SbError::invalid("Both normal and recovery entries for kernel 1019 must exist before installing."));
        }
        Ok(boot)
    }
    pub(super) async fn system_boot_ready(&self, node: &str) -> Result<(), String> {
        let Some(r) = read(&self.live, false)?.filter(|r| r.active() && r.node == node) else {
            return Ok(());
        };
        self.boot_ready_for(node, r.review()).await
    }
    async fn boot_ready_for(&self, node: &str, review: Review) -> Result<(), String> {
        let kernel = review.kernel();
        let out = self.live.run(node, &installed_command_for(review), None, Duration::from_secs(30)).await?;
        if out.code != Some(0) || !installed_ok_for(&out.stdout, review) {
            return Err("All reviewed and retained packages, including NVIDIA's boot fix, must be fully installed before restarting.".into());
        }
        let next = self.live.run(node, super::prereboot::KERNEL, None, Duration::from_secs(20)).await?;
        if next.code != Some(0) || next.stdout.trim() != format!("/boot/vmlinuz-{kernel}") {
            return Err("The next boot kernel could not be confirmed. Do not restart.".into());
        }
        let module = self.live.run(node, &super::prereboot::script(kernel), None, Duration::from_secs(30)).await?;
        if module.code != Some(0) || !super::prereboot::parse(node, kernel, &module.stdout).ok {
            return Err("The expected new kernel, initrd and matching NVIDIA module are not ready. Do not restart.".into());
        }
        let old = self.live.run(node, &super::prereboot::script(OLD_KERNEL), None, Duration::from_secs(30)).await?;
        if old.code != Some(0) || !super::prereboot::parse(node, OLD_KERNEL, &old.stdout).ok {
            return Err("The retained recovery kernel is no longer healthy. Do not restart.".into());
        }
        let recovery = self.live.sudo(node, RECOVERY_GRUB, Duration::from_secs(20)).await?;
        if recovery.code != Some(0) || !recovery.stdout.contains(OLD_KERNEL) {
            return Err("The previous kernel or its GRUB recovery entry is missing. Do not restart.".into());
        }
        if review == Review::Kernel1025 {
            let retained = self.live.run(node, &super::prereboot::script(KERNEL), None, Duration::from_secs(30)).await?;
            if retained.code != Some(0) || !super::prereboot::parse(node, KERNEL, &retained.stdout).ok {
                return Err("The retained 1024 recovery kernel is no longer healthy. Do not restart.".into());
            }
            self.check_grub_fix(node, RECOVERY_1024_GRUB).await?;
        }
        if review == Review::Fleet1025 {
            if !recovery_entries(&recovery.stdout, OLD_KERNEL, true) {
                return Err("The retained 1019 normal and recovery entries must both include NVIDIA's boot fix. Do not restart.".into());
            }
            let next = self.live.sudo(node, CONTINUED_GRUB, Duration::from_secs(20)).await?;
            if next.code != Some(0) || !recovery_entries(&next.stdout, CONTINUED_KERNEL, true) {
                return Err("The 1025 normal and recovery entries must both include NVIDIA's boot fix. Do not restart.".into());
            }
        }
        self.check_grub_fix(node, if review == Review::Initial { NEXT_GRUB } else { CONTINUED_GRUB }).await
    }
    async fn check_grub_fix(&self, node: &str, command: &str) -> Result<(), String> {
        let out = self.live.sudo(node, command, Duration::from_secs(30)).await?;
        let lines: Vec<_> = out.stdout.lines().filter(|l| l.split_whitespace().next().is_some_and(|w| matches!(w, "linux" | "linuxefi"))).collect();
        if out.code != Some(0)
            || lines.is_empty()
            || lines
                .iter()
                .any(|l| !l.split_whitespace().any(|w| w.strip_prefix("initcall_blacklist=").is_some_and(|v| v.split(',').any(|x| x == "power_wrap_init"))))
        {
            return Err("The generated GRUB entries do not all include NVIDIA's boot fix for the new kernel. Do not restart; review the package task.".into());
        }
        Ok(())
    }
    async fn system_health(&self, node: &str, kernel: &str, old_boot: &str) -> Result<String, String> {
        let read_cmd = |cmd| async move {
            let out = self.live.run(node, cmd, None, Duration::from_secs(60)).await?;
            if out.code != Some(0) {
                return Err("Verification read failed; the node remains reserved.".into());
            }
            Ok::<_, String>(out.stdout)
        };
        let health = read_cmd(super::postboot::READ).await?;
        let warnings = super::postboot::judge(
            &health,
            &super::postboot::Expected { boot: Some(old_boot.into()), kernel: Some(kernel.into()), driver: Some(DRIVER.into()), ..Default::default() },
        );
        let cmdline = read_cmd("cat /proc/cmdline").await?;
        if !cmdline.split_whitespace().any(|w| w.strip_prefix("initcall_blacklist=").is_some_and(|v| v.split(',').any(|x| x == "power_wrap_init"))) {
            return Err("The NVIDIA boot fix is not active. Restart only after the package task and boot checks pass; verification remains pending.".into());
        }
        if !warnings.is_empty() {
            return Err(format!("Verification remains pending: {}. Check again after startup settles; do not update another node.", warnings.join("; ")));
        }
        Ok(sections(&health).get("boot").map(|s| s.trim().to_string()).unwrap_or_default())
    }
    pub async fn system_verify(&self, demo: bool) -> Result<Record, String> {
        let mut r = read(&self.live, demo)?.ok_or("No system update to verify.")?;
        if !r.active() {
            return Ok(r);
        }
        if self.runs().iter().any(|run| run.node == r.node && run.simulated == demo && !matches!(run.phase.as_str(), "done" | "failed" | "cancelled")) {
            return Err("The package task has not finished. Follow it in Tasks; do not restart yet.".into());
        }
        if r.boot.is_empty() {
            return Err("Admission was interrupted before a boot baseline was saved. No verified update is recorded; review Tasks and the checkpoint.".into());
        }
        if demo {
            if !self.runs().iter().any(|run| run.node == r.node && Some(run.task_id.as_str()) == r.task.as_deref() && run.simulated && run.phase == "done") {
                return Err("The Demo package task has not completed successfully.".into());
            }
            r.phase = "verified".into();
            r.message = "Demo verification simulated. No real packages, boot or hardware were checked.".into();
            save(&self.live, &r)?;
            return Ok(r);
        }
        let review = r.review();
        let node = &r.node;
        let read_cmd = |cmd: String| async move {
            let o = self.live.run(node, &cmd, None, Duration::from_secs(60)).await?;
            if o.code != Some(0) {
                return Err("Verification read failed; the node remains reserved.".into());
            }
            Ok::<_, String>(o.stdout)
        };
        if !installed_ok_for(&read_cmd(installed_command_for(review)).await?, review) {
            return Err(
                "Not all reviewed and retained package versions are fully installed. The node remains reserved; inspect the package task before retrying."
                    .into(),
            );
        }
        let audit = read_cmd("dpkg --audit".into()).await?;
        let deps = read_cmd(DEPENDENCIES.into()).await?;
        if !audit.trim().is_empty() || deps.lines().any(|l| l.starts_with("E:")) {
            return Err("The package database or dependencies need repair; verification did not pass.".into());
        }
        self.system_boot_ready(&r.node).await?;
        self.system_health(node, review.kernel(), &r.boot).await?;
        let status = self.updates_status(false, &r.node).await.map_err(|e| e.message)?;
        if status.pending.iter().any(|p| review.installed().iter().any(|x| x.name == p.name)) {
            return Err("A reviewed system package still has an update pending. Refresh and review it before continuing.".into());
        }
        let _guard = ADMISSION.lock().unwrap();
        let current = read(&self.live, demo)?.ok_or("System-update checkpoint disappeared.")?;
        if current != r {
            return Err("System-update checkpoint changed during verification.".into());
        }
        r.phase = "verified".into();
        r.message = "All reviewed and retained versions, clean packages, new boot, kernel, NVIDIA fix, GB10/580 driver, both fabric links, Docker and systemd passed. The next node needs its own supported plan.".into();
        save(&self.live, &r)?;
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        demo::DemoCluster,
        fake_node::{connect, fake_node, temp, Node, Script},
        inventory::Inventory,
        tasks::{TaskState, Tasks},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    const OBSERVED: &str = include_str!("../../../src/test/fixtures/protected-updates.json");
    const BOOT: &str = "11111111-2222-3333-4444-555555555555";
    const PRE: &str = "--- initrd\npresent\n--- modinfo\n580.178.04\n--- compute\nii 580.178.04-1\n--- dkms\n--- modules\n--- kho\n--- grub\n";
    fn good_before() -> String {
        format!("--- os\nID=ubuntu\nVERSION_ID=\"24.04\"\n--- arch\narm64\n--- boot\n{BOOT}\n--- kernel\n{OLD_KERNEL}\n--- gpu\nNVIDIA GB10, {DRIVER}\n--- jobs\n--- containers\n--- space\nFilesystem 1024-blocks Used Available Capacity Mounted\n/dev/x 40000000 1000 30000000 1% /\n/dev/x 40000000 1000 30000000 1% /\n--- audit\n--- dependencies\nReading package lists...\n--- end\n")
    }
    fn fixture() -> Vec<PlannedPkg> {
        serde_json::from_str(OBSERVED).unwrap()
    }
    fn sim_text() -> String {
        fixture()
            .iter()
            .map(|p| {
                format!("Inst {}{} ({} Ubuntu:24.04/noble-updates [arm64])\n", p.name, p.from.as_ref().map(|f| format!(" [{f}]")).unwrap_or_default(), p.to)
            })
            .collect()
    }
    #[test]
    fn observed_plan_is_the_only_exception() {
        let forged: UpgradeOpts = serde_json::from_str(r#"{"protected":true}"#).unwrap();
        assert!(!forged.protected, "IPC cannot opt into the private installer mode");
        let s = updates::parse_simulation("cell2", &sim_text());
        assert!(check_plan(&s).is_ok());
        assert_eq!(s.upgrades.len(), 13);
        assert_eq!(s.installs.len(), 8);
        assert!(super::super::gpu_guard::check_plan(&s).is_err(), "routine guard remains strict");
        for i in 0..s.plan.len() {
            for field in 0..4 {
                let mut bad = s.clone();
                match field {
                    0 => bad.plan[i].to.push('1'),
                    1 => bad.plan[i].name.push('x'),
                    2 => bad.plan[i].from = Some("9999".into()),
                    _ => bad.plan[i].new = !bad.plan[i].new,
                };
                assert!(check_plan(&bad).is_err(), "{i}:{field}");
            }
            let mut bad = s.clone();
            bad.plan.remove(i);
            assert!(check_plan(&bad).is_err());
        }
        let mut bad = s.clone();
        bad.plan.push(s.plan[0].clone());
        assert!(check_plan(&bad).is_err());
        for field in 0..4 {
            let mut bad = s.clone();
            match field {
                0 => bad.removes.push("foo".into()),
                1 => bad.removals.push("foo".into()),
                2 => bad.held_back.push("foo".into()),
                _ => bad.phased.push("foo".into()),
            };
            assert!(check_plan(&bad).is_err());
        }
        let p = pins();
        let refs: Vec<_> = p.iter().map(String::as_str).collect();
        assert!(allowed_pins(&refs));
        assert!(!allowed_pins(&refs[..20]));
        let mut duplicate = refs.clone();
        duplicate[0] = duplicate[1];
        assert!(!allowed_pins(&duplicate));
        let installed = fixture().iter().map(|p| format!("{} ii {}\n", p.name, p.to)).collect::<String>();
        assert!(installed_ok(&installed));
        assert!(!installed_ok(&installed.replacen(" ii ", " iU ", 1)));
        assert!(!installed_ok(&(installed.clone() + &installed)));
    }
    #[test]
    fn missing_or_busy_preflight_never_passes() {
        assert_eq!(baseline(&good_before()).unwrap(), BOOT);
        for (from, to) in [
            ("--- end", ""),
            ("arm64", "amd64"),
            (OLD_KERNEL, KERNEL),
            (DRIVER, "590.1"),
            ("--- jobs\n", "--- jobs\n123\n"),
            ("--- containers\n", "--- containers\ncontainer\n"),
            ("--- audit\n", "--- audit\nbroken\n"),
            ("--- dependencies\n", "--- dependencies\nE: broken\n"),
            ("30000000", "100"),
            ("ID=ubuntu", "ID=other"),
        ] {
            assert!(baseline(&good_before().replace(from, to)).is_err(), "{from}");
        }
        assert!(baseline("").is_err());
        assert!(super::super::classify::read_only(BEFORE));
        assert!(!super::super::classify::read_only(&format!("{BEFORE}; touch /tmp/no")));
        assert!(super::super::classify::read_only(&installed_command()));
    }
    async fn setup(case: usize) -> (Arc<Ops>, Node, Arc<AtomicUsize>) {
        static SERIAL: AtomicUsize = AtomicUsize::new(0);
        let serial = SERIAL.fetch_add(1, Ordering::SeqCst);
        let mode = Arc::new(AtomicUsize::new(case));
        let m = mode.clone();
        let script: Script = Arc::new(move |cmd, _| {
            let mode = m.load(Ordering::SeqCst);
            let root = cmd.starts_with("sudo -k -S -p '' ");
            let cmd = cmd.strip_prefix("sudo -k -S -p '' ").unwrap_or(cmd);
            if cmd.contains("/boot/grub/grub.cfg") && (!root || mode == 11) {
                return Some(("grep: /boot/grub/grub.cfg: Permission denied\n".into(), 2));
            }
            if cmd == DEPENDENCIES {
                return Some((
                    if mode == 13 { "E: Unmet dependencies\n" } else { "Reading package lists...\nBuilding dependency tree...\n" }.into(),
                    if mode == 13 { 100 } else { 0 },
                ));
            }
            if cmd == BEFORE {
                if mode == 12 || !cmd.contains("apt-get -s check") {
                    return Some(("E: Could not open lock file /var/lib/dpkg/lock-frontend - open (13: Permission denied)\n".into(), 1));
                }
                return Some((if mode == 1 { good_before().replace("--- jobs\n", "--- jobs\n123\n") } else { good_before() }, 0));
            }
            if cmd.starts_with("apt-get -s ") {
                return Some((if mode == 2 { sim_text().replace("1.0-2", "1.0-3") } else { sim_text() }, 0));
            }
            if cmd == updates::STATUS_SCRIPT {
                return Some((
                    format!("--- upgradable\n--- held\n{}--- reboot\n--- unattended\n--- checked\n", if mode == 3 { "nvidia-spark-initcall-bl\n" } else { "" }),
                    0,
                ));
            }
            if cmd.starts_with("echo '--- initrd'") {
                return Some((if mode == 4 { PRE.replace("present", "") } else { PRE.into() }, 0));
            }
            if cmd.starts_with("test -s /boot/vmlinuz-") {
                return Some((OLD_KERNEL.into(), 0));
            }
            if cmd == super::super::prereboot::KERNEL {
                return Some((format!("/boot/vmlinuz-{KERNEL}"), if mode == 10 { 1 } else { 0 }));
            }
            if cmd == installed_command() {
                return Some((fixture().iter().map(|p| format!("{} {} {}\n", p.name, if mode == 8 { "iU" } else { "ii" }, p.to)).collect(), 0));
            }
            if cmd.starts_with("grep -F -- vmlinuz-") && mode == 9 {
                return Some((format!("linux /boot/vmlinuz-{KERNEL} kho=off\n"), 0));
            }
            if cmd.starts_with("grep -F -- vmlinuz-") {
                return Some((format!("linux /boot/vmlinuz-{KERNEL} initcall_blacklist=tegra234_cbb_init,power_wrap_init\n"), 0));
            }
            if cmd == super::super::postboot::READ {
                return Some((format!("--- boot\n{}\n--- kernel\n{KERNEL}\n--- gpu\nNVIDIA GB10, {DRIVER}\n--- fabric\nenp1s0f1np1 200000 9000\nenP2p1s0f1np1 200000 9000\n--- docker\n28.5.0\n--- system\n{}\n--- failed\n--- end\n",if mode==6 {BOOT}else{"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"},if mode==5 {"starting"}else{"running"}),0));
            }
            if cmd == "cat /proc/cmdline" {
                return Some((if mode == 7 { "kho=off" } else { "kho=off initcall_blacklist=tegra234_cbb_init,power_wrap_init" }.into(), 0));
            }
            None
        });
        let (node, port) = fake_node(&temp(&format!("system-home-{serial}")), script).await;
        // The stock shim executes the detached lifecycle; this adapter accepts version pins.
        let shim = std::fs::read_to_string(node.bin_dir.join("apt-get")).unwrap();
        node.bin("apt-under-test", &shim);
        node.bin(
            "apt-get",
            r#"#!/bin/sh
for a in "$@"; do shift; case "$a" in *=*) a=${a%%=*};; esac; set -- "$@" "$a"; done
exec "$(dirname "$0")/apt-under-test" "$@"
"#,
        );
        let (live, _) = connect(&temp(&format!("system-app-{serial}")), &[("cell2", port)]).await;
        let ops = Ops::with_poll(live, Arc::new(Mutex::new(DemoCluster::new(&Inventory::example()))), Tasks::new(None), Duration::from_millis(5));
        (ops, node, mode)
    }
    async fn done(ops: &Ops, id: &str) -> crate::tasks::Task {
        for _ in 0..2000 {
            let t = ops.tasks.get(id).unwrap();
            if t.finished() {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("task did not finish")
    }
    #[tokio::test]
    async fn protected_refusals_send_no_root_installer() {
        for case in [1, 2, 3, 4, 11, 12] {
            let (ops, node, _) = setup(case).await;
            assert!(ops.system_preview(false, "cell2").await.is_err());
            let plan = fixture().iter().map(|p| PlanEntry { name: p.name.clone(), to: p.to.clone() }).collect();
            let id = ops.system_start(false, "cell2".into(), plan).unwrap();
            assert_eq!(done(&ops, &id).await.state, TaskState::Failed);
            assert!(node.ran("systemd-run").is_empty());
            assert_eq!(ops.system_status(false).unwrap().unwrap().phase, "refused");
        }
    }
    #[tokio::test]
    async fn detached_install_requires_healthy_new_boot_and_survives_app_recreation() {
        let (ops, node, mode) = setup(0).await;
        let sim = ops.system_preview(false, "cell2").await.unwrap();
        assert!(!node.ran("sudo -k -S -p '' test -s /boot/vmlinuz-").is_empty(), "root-only GRUB must use authenticated sudo");
        let id = ops.system_start(false, "cell2".into(), sim.shown()).unwrap();
        assert!(ops.system_start(false, "cell1".into(), sim.shown()).is_err());
        assert!(ops.updates_upgrade_opts(false, "cell2".into(), vec!["curl".into()], UpgradeArgs { plan: Some(vec![]), ..Default::default() }).is_err());
        assert!(check_load(&ops.live, false, &["cell2".into()]).is_err());
        assert!(check_load(&ops.live, true, &["cell2".into()]).is_ok());
        let t = done(&ops, &id).await;
        assert_eq!(t.state, TaskState::Ok, "{:?}", t);
        assert!(t.retry.is_none(), "protected maintenance must not offer a routine installer retry");
        let starts = node.ran("systemd-run");
        assert_eq!(starts.len(), 1);
        assert!(!starts[0].contains("--only-upgrade"));
        for p in fixture() {
            assert!(starts[0].contains(&format!("{}={}", p.name, p.to)));
        }
        let recovered = Ops::with_poll(ops.live.clone(), ops.demo.clone(), Tasks::new(None), Duration::from_millis(5));
        assert_eq!(recovered.system_status(false).unwrap().unwrap().task.as_deref(), Some(id.as_str()));
        for case in [5, 6, 7, 8, 9, 10, 11, 13] {
            mode.store(case, Ordering::SeqCst);
            assert!(recovered.system_verify(false).await.is_err());
            assert!(check_load(&ops.live, false, &["cell2".into()]).is_err());
        }
        mode.store(0, Ordering::SeqCst);
        assert_eq!(recovered.system_verify(false).await.unwrap().phase, "verified");
        assert!(check_load(&ops.live, false, &["cell2".into()]).is_ok());
        assert_eq!(node.ran("systemd-run").len(), 1);
        assert!(node.raw.lock().unwrap().iter().all(|c| super::super::classify::power_alone(c).is_none()));
    }
    #[tokio::test]
    async fn malformed_or_other_owner_checkpoint_fails_closed() {
        let (ops, _, _) = setup(0).await;
        let p = path(&ops.live, false);
        std::fs::write(&p, "broken").unwrap();
        assert!(ops.system_status(false).is_err());
        assert!(check_load(&ops.live, false, &["cell2".into()]).is_err());
        let mut r = Record { node: "cell2".into(), phase: "installing".into(), instance: "someone-else".into(), ..Default::default() };
        save(&ops.live, &r).unwrap();
        assert!(ops.system_status(false).is_err());
        r.instance = crate::identity::instance_name().into();
        save(&ops.live, &r).unwrap();
        assert!(ops.system_discard_review(false).is_err());
        r.phase = "checking".into();
        save(&ops.live, &r).unwrap();
        assert_eq!(ops.system_discard_review(false).unwrap().phase, "refused");
    }
    include!("protected_updates_1025_tests.rs");
    include!("protected_updates_fleet_tests.rs");
}
