//! Checks before every reboot (P1.26). Before SparkBox restarts a node it reads which kernel
//! boots next and whether that kernel can come up with its GPU: an initrd (in September 2026
//! 7.0.0-1019-nvidia machines were left without one and panicked at boot), an NVIDIA module
//! built for it at the version the userspace driver has, and no DKMS NVIDIA package (what left
//! those kernels without an initrd). If any of that is wrong the restart is refused before
//! anything is sent: no command, no maintenance window.
//!
//! Recorded, never blocking (the round plan's decision): which prebuilt
//! `linux-modules-nvidia-580-open-*` packages are installed (PLAN names the `-nvidia-hwe-24.04`
//! metapackage, but a wrong guess at a package name mustn't block every restart; the modinfo
//! match is the proof), the DGX OS 7.6.0 `kho=off` hotfix, and GRUB_DEFAULT (a `grub-reboot`
//! rollback works only with GRUB_DEFAULT=saved; a node without it reads "not set").
//!
//! Two reads as the cluster user, no sudo, as SparkBox's own probes: `KERNEL` (step A), then
//! `script(k)` (step B) for the kernel it names (contract C6′: the userspace driver read with its
//! dpkg status, so a package removed with its config kept (`rc`) blocks, and a trailing `true`, so
//! a node without GRUB_DEFAULT doesn't show a failed row). The node window's Restart
//! (`Ops::power`; a scheduled one is checked when it's scheduled and again a minute before its
//! time) and Fleet's restarts (`run_cutting_access`, per node) run them first. A shutdown isn't
//! checked: the node boots that kernel only when it's woken, and Wake doesn't check either.
//!
//! Demo has no node to read: the step says "Available with Live data".

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

use super::Ops;
use crate::errors::{ErrorKind, SbError};
use crate::live::audit::{self, OpMeta};
use crate::live::Live;
use crate::tasks::TaskHandle;

/// Step A: where /boot/vmlinuz points (the kernel GRUB boots by default on DGX OS).
pub const KERNEL: &str = "readlink -f /boot/vmlinuz";

/// The task step's label. In Demo it reads `DEMO_STEP`.
pub const STEP: &str = "Checks before the restart";
pub const DEMO_STEP: &str = "Checks before the restart: Available with Live data";

const LIMIT: Duration = Duration::from_secs(20);

/// The `dpkg -l` lines of packages whose files are on the node: any wanted state (`u`, `i`,
/// `h` for held, `r`, `p`) with installed, unpacked, half-configured, half-installed or
/// trigger-waiting files (`i U F H W t`). A package on hold reads `hi` (the GPU-stack pin holds
/// nvidia-dkms-*, the prebuilt modules and nvidia-spark-grub-kho); one removed with its config
/// kept (`rc`) or never installed (`un`) isn't there.
pub const INSTALLED_LINES: &str = "grep -E '^[uihrp][iUFHWt]'";

/// Step B for kernel `k` (already validated by `kernel_of`, so it goes in unquoted, as C6′ has it).
pub fn script(k: &str) -> String {
    format!(
        "echo '--- initrd'; test -s /boot/initrd.img-{k} && echo present; \
echo '--- modinfo'; modinfo -k {k} -F version nvidia 2>&1; \
echo '--- compute'; dpkg-query -W -f='${{db:Status-Abbrev}}${{Version}}\\n' libnvidia-compute-580 2>&1; \
echo '--- dkms'; dpkg -l 'nvidia-dkms-*' 2>/dev/null | {INSTALLED_LINES}; \
echo '--- modules'; dpkg -l 'linux-modules-nvidia-580-open-*' 2>/dev/null | {INSTALLED_LINES}; \
echo '--- kho'; grep -o kho=off /proc/cmdline; dpkg -l nvidia-spark-grub-kho 2>/dev/null | {INSTALLED_LINES}; \
echo '--- grub'; grep GRUB_DEFAULT /etc/default/grub; true"
    )
}

/// The compute line (`${db:Status-Abbrev}${Version}`: three status columns, the third a space
/// unless dpkg flags an error, then the version): (status, version). None for anything else
/// (dpkg-query's "no packages found", nothing at all).
fn status_and_version(line: &str) -> Option<(&str, &str)> {
    let status = line.get(..3)?;
    let mut cols = status.chars();
    let (want, have) = (cols.next()?, cols.next()?);
    ("uihrp".contains(want) && "ncHUFWti".contains(have)).then(|| (status.trim_end(), line[3..].trim()))
}

/// One line of the checks: ok, fail (it blocks the restart) or info (recorded only).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    pub label: String,
    /// ok | fail | info
    pub state: String,
    pub detail: String,
}

/// What the checks found on one node. `ok` exactly when nothing blocks the restart.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreReboot {
    pub node: String,
    /// The kernel that boots next ("7.0.0-1019-nvidia"); None when SparkBox can't tell.
    pub kernel: Option<String>,
    pub ok: bool,
    /// Why the restart is refused, one sentence each.
    pub blocking: Vec<String>,
    pub checks: Vec<Check>,
}

impl PreReboot {
    fn push(&mut self, id: &str, label: &str, state: &str, detail: String) {
        self.checks.push(Check { id: id.into(), label: label.into(), state: state.into(), detail });
    }

    fn block(&mut self, id: &str, label: &str, detail: String, why: String) {
        self.push(id, label, "fail", detail);
        self.blocking.push(why);
    }

    /// The task step's lines: "Its initrd: /boot/initrd.img-… is there".
    pub fn lines(&self) -> Vec<String> {
        self.checks
            .iter()
            .map(|c| match c.state.as_str() {
                "info" => format!("{} (recorded): {}", c.label, c.detail),
                _ => format!("{}: {}", c.label, c.detail),
            })
            .collect()
    }
}

/// A kernel name SparkBox passes to the node unquoted: `^[A-Za-z0-9._+~-]+$`, starting with a
/// letter or digit (so no option, and no `~` where the shell would expand it).
pub fn valid_kernel(k: &str) -> bool {
    !k.is_empty() && k.len() <= 100 && k.starts_with(|c: char| c.is_ascii_alphanumeric()) && k.chars().all(|c| c.is_ascii_alphanumeric() || "._+~-".contains(c))
}

/// The kernel step A names ("/boot/vmlinuz-7.0.0-1019-nvidia" → "7.0.0-1019-nvidia"), or why
/// it names none.
pub fn kernel_of(readlink: &str) -> Result<String, String> {
    let path = readlink.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let base = path.rsplit('/').next().unwrap_or("");
    match base.strip_prefix("vmlinuz-") {
        _ if path.is_empty() || base == "vmlinuz" => Err("/boot/vmlinuz isn't there".into()),
        Some(k) if valid_kernel(k) => Ok(k.to_string()),
        Some(_) => Err(format!("/boot/vmlinuz points at a kernel name SparkBox won't pass to the node ({})", path.escape_debug())),
        None => Err(format!("/boot/vmlinuz points at {}", path.escape_debug())),
    }
}

/// The result when step A names no kernel: nothing else is read.
pub fn unknown_kernel(node: &str, why: &str) -> PreReboot {
    let mut r = PreReboot { node: node.into(), ..Default::default() };
    r.block("kernel", "Kernel that boots next", why.to_string(), format!("Can't tell which kernel boots next: {why}. Not restarted."));
    r
}

/// Step B's output, split at its `--- name` lines (None for a section that never came).
fn sections(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("--- ") {
            current = Some(name.trim().to_string());
            out.entry(name.trim().to_string()).or_default();
        } else if let Some(c) = &current {
            if !line.trim().is_empty() {
                out.entry(c.clone()).or_default().push(line.trim().to_string());
            }
        }
    }
    out
}

/// `dpkg -l … | INSTALLED_LINES`: (package, version) per line whose status says its files are
/// there (held or not, fully configured or not; `R` after it, reinstall required, too).
fn installed(lines: &[String]) -> Vec<(String, String)> {
    lines
        .iter()
        .filter_map(|l| {
            let mut cols = l.split_whitespace();
            let status: Vec<char> = cols.next()?.chars().collect();
            let there = matches!(status.as_slice(), [want, have] | [want, have, 'R'] if "uihrp".contains(*want) && "iUFHWt".contains(*have));
            there.then_some(())?;
            let name = cols.next()?.split(':').next()?.to_string();
            Some((name, cols.next().unwrap_or("").to_string()))
        })
        .collect()
}

fn names(pkgs: &[(String, String)]) -> String {
    pkgs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
}

/// A version modinfo prints ("580.178.04"), not its error.
fn is_version(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_digit()) && s.chars().all(|c| c.is_ascii_alphanumeric() || ".~+".contains(c))
}

const SECTIONS: [&str; 7] = ["initrd", "modinfo", "compute", "dkms", "modules", "kho", "grub"];

/// Step B's answer for kernel `k`, judged.
pub fn parse(node: &str, k: &str, out: &str) -> PreReboot {
    let s = sections(out);
    let mut r = PreReboot { node: node.into(), kernel: Some(k.into()), ..Default::default() };
    r.push("kernel", "Kernel that boots next", "ok", format!("{k} (where /boot/vmlinuz points)"));
    let missing: Vec<&str> = SECTIONS.iter().copied().filter(|n| !s.contains_key(*n)).collect();
    if !missing.is_empty() {
        let what = missing.join(", ");
        r.block("answer", "The node's answer", format!("cut off: nothing came back for {what}"), format!("The checks didn't finish (nothing came back for {what}). Not restarted."));
        return r;
    }
    let get = |n: &str| s.get(n).cloned().unwrap_or_default();

    // The September 2026 panic: a kernel with no initrd doesn't mount its root file system.
    if get("initrd").iter().any(|l| l == "present") {
        r.push("initrd", "Its initrd", "ok", format!("/boot/initrd.img-{k} is there"));
    } else {
        r.block("initrd", "Its initrd", format!("/boot/initrd.img-{k} is missing or empty"), format!("No initrd for {k}: the September 2026 panic. Not restarted."));
    }

    // The NVIDIA module built for that kernel, and the userspace driver it must match.
    let modinfo = get("modinfo").into_iter().next().unwrap_or_default();
    let module = is_version(&modinfo).then(|| modinfo.clone());
    match &module {
        Some(v) => r.push("module", "Its NVIDIA module", "ok", v.clone()),
        None => {
            let said = if modinfo.is_empty() { "modinfo printed nothing".to_string() } else { modinfo.clone() };
            r.block("module", "Its NVIDIA module", format!("none for {k} ({said})"), format!("No NVIDIA module for {k}: it would boot without its GPU. Not restarted."));
        }
    }
    // Installed means dpkg says so (`ii`, or `hi` when the pin holds it), not that it has a version:
    // one removed with its config kept (`rc`) still reports the version it had.
    let compute = get("compute").into_iter().next().unwrap_or_default();
    const USERSPACE: &str = "NVIDIA userspace (libnvidia-compute-580)";
    let userspace = match status_and_version(&compute) {
        Some((status @ ("ii" | "hi"), v)) if !v.is_empty() => {
            r.push("userspace", USERSPACE, "ok", format!("{v} (dpkg status {status})"));
            Some(v.to_string())
        }
        Some(("rc", v)) => {
            r.block("userspace", USERSPACE, format!("removed, only its config is left (dpkg status rc, {v})"), "libnvidia-compute-580 is removed (only its config is left): the driver's userspace isn't installed. Not restarted.".into());
            None
        }
        Some((status, v)) if !matches!(status.get(1..2), Some("n")) => {
            let had = if v.is_empty() { String::new() } else { format!(", {v}") };
            r.block("userspace", USERSPACE, format!("not fully installed (dpkg status {status}{had})"), format!("libnvidia-compute-580 isn't fully installed (dpkg status {status}): the driver's userspace may be broken. Not restarted."));
            None
        }
        _ => {
            let said = if compute.is_empty() { "dpkg-query printed nothing".to_string() } else { compute.clone() };
            r.block("userspace", USERSPACE, format!("not installed ({said})"), "libnvidia-compute-580 isn't installed, so SparkBox can't tell which driver the GPU programs expect. Not restarted.".into());
            None
        }
    };
    if let (Some(m), Some(u)) = (&module, &userspace) {
        let base = u.split('-').next().unwrap_or(u);
        if m == base {
            r.push("match", "Module and userspace match", "ok", format!("both {m}"));
        } else {
            r.block("match", "Module and userspace match", format!("module {m}, userspace {base}"), format!("{k} would boot driver {m} against userspace {base}. Not restarted."));
        }
    }

    // DKMS NVIDIA packages: how those kernels lost their initrd in September 2026.
    let dkms = installed(&get("dkms"));
    if dkms.is_empty() {
        r.push("dkms", "No DKMS NVIDIA packages", "ok", "none installed".into());
    } else {
        let list = names(&dkms);
        r.block("dkms", "No DKMS NVIDIA packages", format!("{list} installed"), format!("{list} {} installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted.", if dkms.len() == 1 { "is" } else { "are" }));
    }

    // Recorded only.
    let modules = installed(&get("modules"));
    let detail = if modules.is_empty() {
        "none installed (the module check above is what counts)".to_string()
    } else {
        let yes = |b: bool| if b { "yes" } else { "no" };
        let list = modules.iter().map(|(n, v)| format!("{n} {v}")).collect::<Vec<_>>().join(", ");
        format!(
            "{list}; one for {k}: {}; the nvidia-hwe-24.04 metapackage: {}",
            yes(modules.iter().any(|(n, _)| n.ends_with(&format!("-{k}")))),
            yes(modules.iter().any(|(n, _)| n == "linux-modules-nvidia-580-open-nvidia-hwe-24.04"))
        )
    };
    r.push("modules", "Prebuilt NVIDIA module packages", "info", detail);

    let kho = get("kho");
    let on = kho.iter().any(|l| l == "kho=off");
    let pkg = installed(&kho).into_iter().find(|(n, _)| n == "nvidia-spark-grub-kho");
    r.push(
        "kho",
        "kho=off hotfix (DGX OS 7.6.0)",
        "info",
        format!(
            "kho=off {} on the kernel command line; nvidia-spark-grub-kho {}",
            if on { "is" } else { "isn't" },
            pkg.map(|(_, v)| format!("{v} is installed")).unwrap_or_else(|| "isn't installed".into())
        ),
    );

    let grub: Vec<String> = get("grub");
    let saved = grub.iter().any(|l| l.trim_start().strip_prefix("GRUB_DEFAULT=").is_some_and(|v| v.trim_matches(|c| c == '"' || c == '\'') == "saved"));
    let rollback = if saved { "a grub-reboot rollback works" } else { "a grub-reboot rollback needs GRUB_DEFAULT=saved" };
    let detail = if grub.is_empty() { format!("not set (GRUB boots its first entry; {rollback})") } else { format!("{} ({rollback})", grub.join("; ")) };
    r.push("grub", "GRUB_DEFAULT", "info", detail);

    r.ok = r.blocking.is_empty();
    r
}

/// Runs the checks on `node`: step A, then step B for the kernel it names. Reads only, as the
/// cluster user, each as SparkBox's own probe. Err when the node can't be asked at all.
pub async fn check(live: &Live, node: &str) -> Result<PreReboot, SbError> {
    let a = audit::as_probe(live.run(node, KERNEL, None, LIMIT)).await.map_err(|e| SbError::classify(&e).on(node))?;
    let k = match kernel_of(&a.stdout) {
        Ok(k) => k,
        Err(why) => return Ok(unknown_kernel(node, &why)),
    };
    let b = audit::as_probe(live.run(node, &script(&k), None, LIMIT)).await.map_err(|e| SbError::classify(&e).on(node))?;
    Ok(parse(node, &k, &b.stdout))
}

impl Ops {
    /// The task step `label` ("Checks before the restart") for `node`: Ok when it may restart,
    /// Err with why not (every blocking reason, one per line). The reads leave from a task of
    /// their own under the task's op, outside a Fleet run's typed scope: they're SparkBox's own
    /// reads, not the command that was typed. Cancel (or look-only coming on) stops the wait at
    /// once: Err, and the caller, seeing the task cancelled, sends nothing.
    pub(super) async fn checks_before_restart(&self, demo: bool, task: &TaskHandle, node: &str, label: &str) -> Result<(), String> {
        if demo {
            task.step_skipped(DEMO_STEP, "Not checked in Demo");
            return Ok(());
        }
        self.system_boot_ready(node).await?;
        task.step(label.to_string());
        let name = self.name_of(node);
        let title = self.tasks.get(task.id()).map(|t| t.title).unwrap_or_default();
        let (live, n, op) = (self.live.clone(), node.to_string(), task.id().to_string());
        let reads = tauri::async_runtime::spawn(audit::with_op_id(op, OpMeta { title, source: "ui".into() }, async move { check(&live, &n).await }));
        let ran = tokio::select! {
            ran = reads => ran,
            _ = task.cancelled() => return Err(format!("Called off during the checks before the restart of {name}. Not restarted.")),
        };
        match ran {
            Ok(Ok(r)) if r.ok => {
                let before = self.node_view(node);
                let peers = self.cluster().unwrap_or_default().nodes.iter().filter(|n| n.id != node && n.status == crate::model::LinkStatus::Online && !n.stale && !n.system.driver.is_empty())
                    .map(|n| (n.id.clone(), n.system.driver.clone())).collect();
                self.postboot.lock().unwrap().insert(node.into(), super::postboot::Expected {
                    boot: before.as_ref().and_then(|n| n.boot_id.clone()), kernel: r.kernel.clone(),
                    driver: before.as_ref().map(|n| n.system.driver.clone()).filter(|s| !s.is_empty()), peers,
                });
                task.step_ok(Some(r.lines().join("\n")));
                Ok(())
            }
            Ok(Ok(r)) => {
                task.step_failed(r.lines().join("\n"));
                Err(r.blocking.join("\n"))
            }
            Ok(Err(e)) => {
                let why = format!("Couldn't run the checks before the restart on {name}: {}. Not restarted.", e.message.trim_end_matches('.'));
                task.step_failed(why.clone());
                Err(why)
            }
            Err(e) => {
                let why = format!("Couldn't run the checks before the restart on {name}: {e}. Not restarted.");
                task.step_failed(why.clone());
                Err(why)
            }
        }
    }
}

/// What the restart confirmation shows (src/lib/power.ts): the checks on `node` now. Reads
/// only; Demo has no node to read.
#[tauri::command]
pub async fn node_prereboot_check(state: tauri::State<'_, crate::AppState>, node: String) -> Result<PreReboot, SbError> {
    if !crate::is_live(&state) {
        return Err(SbError::new(ErrorKind::DemoOnly, "Available with Live data.").on(&node));
    }
    let title = format!("{STEP} of {}", state.ops.name_of(&node));
    audit::with_op_id(format!("prereboot-{}", super::now_ms()), OpMeta { title, source: "ui".into() }, check(&state.live, &node)).await
}

/// A GB10 for the fake nodes: how it answers the two reads, healthy or with one thing wrong.
#[cfg(test)]
pub mod fake {
    use super::{script, KERNEL};

    pub const K: &str = "7.0.0-1019-nvidia";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Gb10 {
        Healthy,
        /// /boot/vmlinuz is gone (readlink -f prints the path itself).
        NoVmlinuz,
        /// No /boot/initrd.img-K (the September 2026 panic).
        NoInitrd,
        /// The module on disk is 580.95.05; the userspace 580.178.04.
        Mismatch,
        /// No nvidia.ko for K.
        NoModule,
        NoUserspace,
        /// libnvidia-compute-580 removed with its config kept (`rc`): it still reports a version.
        RemovedUserspace,
        /// nvidia-dkms-580-open installed.
        Dkms,
        /// nvidia-dkms-580-open installed and on hold (`hi`).
        HeldDkms,
        /// Healthy after "Pin the GPU stack": the prebuilt modules and nvidia-spark-grub-kho on hold.
        Pinned,
    }

    /// Step B's answer, in the format the real commands print (`dpkg -l` through
    /// `INSTALLED_LINES`, as dpkg 1.22 prints its lines).
    pub fn output(state: Gb10) -> String {
        let initrd = if state == Gb10::NoInitrd { "" } else { "present\n" };
        let modinfo = match state {
            Gb10::Mismatch => "580.95.05\n",
            Gb10::NoModule => "modinfo: ERROR: Module nvidia not found.\n",
            _ => "580.178.04\n",
        };
        let compute = match state {
            Gb10::NoUserspace => "dpkg-query: no packages found matching libnvidia-compute-580\n",
            Gb10::RemovedUserspace => "rc 580.178.04-0ubuntu0.24.04.1\n",
            _ => "ii 580.178.04-0ubuntu0.24.04.1\n",
        };
        let dkms = match state {
            Gb10::Dkms => "ii  nvidia-dkms-580-open                 580.178.04-0ubuntu0.24.04.1        arm64        NVIDIA DKMS package\n",
            Gb10::HeldDkms => "hi  nvidia-dkms-580-open                 580.178.04-0ubuntu0.24.04.1        arm64        NVIDIA DKMS package\n",
            _ => "",
        };
        let held = if state == Gb10::Pinned { "hi" } else { "ii" };
        format!(
            "--- initrd\n{initrd}--- modinfo\n{modinfo}--- compute\n{compute}--- dkms\n{dkms}--- modules\n\
{held}  linux-modules-nvidia-580-open-{K}   7.0.0-1019.19+1   arm64   Linux kernel nvidia modules for version 7.0.0-1019\n\
{held}  linux-modules-nvidia-580-open-nvidia-hwe-24.04   7.0.0-1019.19+1   arm64   Extra drivers for nvidia-580-open for the nvidia-hwe-24.04 flavour\n\
--- kho\nkho=off\n{held}  nvidia-spark-grub-kho   1.0.0   all   Adds kho=off to the kernel command line\n--- grub\nGRUB_DEFAULT=0\n"
        )
    }

    /// The fake's answer to `cmd` (the logical command a Script sees), or None for anything else.
    pub fn answer(state: Gb10, cmd: &str) -> Option<(String, u32)> {
        if cmd == crate::ops::postboot::READ {
            return Some((format!("--- boot\nfixture-new-boot\n--- kernel\n{K}\n--- gpu\nNVIDIA GB10, 580.178.04\n--- fabric\nenp1s0f1np1 200000 9000\nenP2p1s0f1np1 200000 9000\n--- docker\n28.0.0\n--- system\nrunning\n--- failed\n--- end\n"), 0));
        }
        if cmd == KERNEL {
            return Some((if state == Gb10::NoVmlinuz { "/boot/vmlinuz\n".into() } else { format!("/boot/vmlinuz-{K}\n") }, 0));
        }
        (cmd == script(K)).then(|| (output(state), 0))
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{output, Gb10, K};
    use super::*;

    #[test]
    fn step_b_is_the_contract_c6_literal() {
        assert_eq!(KERNEL, "readlink -f /boot/vmlinuz");
        assert_eq!(
            script("7.0.0-1019-nvidia"),
            "echo '--- initrd'; test -s /boot/initrd.img-7.0.0-1019-nvidia && echo present; echo '--- modinfo'; modinfo -k 7.0.0-1019-nvidia -F version nvidia 2>&1; echo '--- compute'; dpkg-query -W -f='${db:Status-Abbrev}${Version}\\n' libnvidia-compute-580 2>&1; echo '--- dkms'; dpkg -l 'nvidia-dkms-*' 2>/dev/null | grep -E '^[uihrp][iUFHWt]'; echo '--- modules'; dpkg -l 'linux-modules-nvidia-580-open-*' 2>/dev/null | grep -E '^[uihrp][iUFHWt]'; echo '--- kho'; grep -o kho=off /proc/cmdline; dpkg -l nvidia-spark-grub-kho 2>/dev/null | grep -E '^[uihrp][iUFHWt]'; echo '--- grub'; grep GRUB_DEFAULT /etc/default/grub; true"
        );
        // The classifier's and the exit's contract tests check this same text.
        assert_eq!(script("7.0.0-1019-nvidia"), crate::ops::classify::contracts::C6_STEP_B);
        assert_eq!(KERNEL, crate::ops::classify::contracts::C6_KERNEL);
        // A background read at the write exit: Read, no sudo (a probe that isn't is refused).
        let b = script(K);
        assert_eq!(crate::ops::classify::classify(&b), crate::ops::classify::Kind::Read, "{b}");
        assert!(crate::ops::classify::read_only(&b) && !crate::ops::classify::escalates(&b));
    }

    /// What `dpkg -l` prints on Ubuntu 24.04 (dpkg 1.22.6), captured in an ubuntu:24.04
    /// container from small packages named like the GPU stack: one held (`hi`, as after "Pin the
    /// GPU stack"), one whose postinst failed (`iF`), one only unpacked (`iU`); and procps removed
    /// with its config kept (`rc`).
    const DPKG_DKMS: &str = "Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name                 Version                     Architecture Description
+++-====================-===========================-============-=================================
iF  nvidia-dkms-580      580.95.05-0ubuntu1          arm64        test nvidia-dkms-580
hi  nvidia-dkms-580-open 580.178.04-0ubuntu0.24.04.1 arm64        test nvidia-dkms-580-open
";
    const DPKG_MODULES: &str = "Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name                                            Version         Architecture Description
+++-===============================================-===============-============-====================================================
hi  linux-modules-nvidia-580-open-7.0.0-1019-nvidia 7.0.0-1019.19+1 arm64        test linux-modules-nvidia-580-open-7.0.0-1019-nvidia
iU  linux-modules-nvidia-580-open-nvidia-hwe-24.04  7.0.0-1019.19+1 arm64        test linux-modules-nvidia-580-open-nvidia-hwe-24.04
";
    const DPKG_RC: &str = "Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name           Version            Architecture Description
+++-==============-==================-============-=================================
rc  procps         2:4.0.4-4ubuntu3.3 arm64        /proc file system utilities
";

    /// `dpkg -l` output through the step's own filter, run by a real shell.
    fn filtered(dpkg_l: &str) -> String {
        let out = std::process::Command::new("/bin/sh").args(["-c", &format!("printf '%s' \"$DPKG_L\" | LC_ALL=C {INSTALLED_LINES}")]).env("DPKG_L", dpkg_l).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn held_and_half_installed_packages_count_and_removed_ones_dont() {
        // The node-side filter keeps the held and half-installed lines and drops the header and rc.
        assert_eq!(filtered(DPKG_DKMS).lines().map(|l| &l[..2]).collect::<Vec<_>>(), ["iF", "hi"]);
        assert_eq!(filtered(DPKG_MODULES).lines().map(|l| &l[..2]).collect::<Vec<_>>(), ["hi", "iU"]);
        assert_eq!(filtered(DPKG_RC), "");
        // A held DKMS package blocks, as an installed one does; so does a half-configured one.
        let with = |dkms: &str, modules: &str| format!("--- initrd\npresent\n--- modinfo\n580.178.04\n--- compute\nii 580.178.04-1\n--- dkms\n{dkms}--- modules\n{modules}--- kho\n--- grub\n");
        let r = parse("cell2", K, &with(&filtered(DPKG_DKMS), ""));
        assert!(!r.ok);
        assert_eq!(r.blocking, ["nvidia-dkms-580, nvidia-dkms-580-open are installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted."]);
        let r = parse("cell2", K, &output(Gb10::HeldDkms));
        assert_eq!(r.blocking, ["nvidia-dkms-580-open is installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted."]);
        // Pinned (held) prebuilt modules and kho package are recorded as installed.
        let r = parse("cell2", K, &with("", &filtered(DPKG_MODULES)));
        assert!(r.ok, "{:?}", r.blocking);
        let modules = r.checks.iter().find(|c| c.id == "modules").unwrap();
        assert_eq!(modules.detail, format!("linux-modules-nvidia-580-open-{K} 7.0.0-1019.19+1, linux-modules-nvidia-580-open-nvidia-hwe-24.04 7.0.0-1019.19+1; one for {K}: yes; the nvidia-hwe-24.04 metapackage: yes"));
        let r = parse("cell2", K, &output(Gb10::Pinned));
        assert!(r.ok, "{:?}", r.blocking);
        assert_eq!(r.lines(), parse("cell2", K, &output(Gb10::Healthy)).lines(), "pinned reads as healthy");
        assert!(r.lines().iter().any(|l| l == "kho=off hotfix (DGX OS 7.6.0) (recorded): kho=off is on the kernel command line; nvidia-spark-grub-kho 1.0.0 is installed"));
        // The status column, one line at a time: wanted any way, files there; reinstall-required too.
        for (line, there) in [("hi  p 1", true), ("ii  p 1", true), ("ri  p 1", true), ("iU  p 1", true), ("iHR p 1", true), ("it  p 1", true), ("rc  p 1", false), ("un  p 1", false), ("pn  p 1", false), ("||/ Name", false), ("+++-===", false)] {
            assert_eq!(!installed(&[line.to_string()]).is_empty(), there, "{line}");
        }
    }

    #[test]
    fn the_kernel_that_boots_next_is_validated_before_it_reaches_a_command() {
        assert_eq!(kernel_of("/boot/vmlinuz-7.0.0-1019-nvidia\n"), Ok("7.0.0-1019-nvidia".to_string()));
        assert_eq!(kernel_of("/boot/vmlinuz-6.11.0-1016-nvidia+1~rc\n"), Ok("6.11.0-1016-nvidia+1~rc".to_string()));
        // readlink -f prints the path itself when the link is gone, and nothing when /boot is.
        assert_eq!(kernel_of("/boot/vmlinuz\n"), Err("/boot/vmlinuz isn't there".to_string()));
        assert_eq!(kernel_of(""), Err("/boot/vmlinuz isn't there".to_string()));
        assert_eq!(kernel_of("/boot/Image\n"), Err("/boot/vmlinuz points at /boot/Image".to_string()));
        for bad in ["/boot/vmlinuz-7.0.0;reboot", "/boot/vmlinuz-$(id)", "/boot/vmlinuz-a b", "/boot/vmlinuz--k", "/boot/vmlinuz-~root", "/boot/vmlinuz-", "/boot/vmlinuz-x'y"] {
            let e = kernel_of(bad).unwrap_err();
            assert!(e.starts_with("/boot/vmlinuz points at a kernel name SparkBox won't pass to the node") || bad.ends_with("vmlinuz-"), "{bad}: {e}");
            assert!(!valid_kernel(bad.trim_start_matches("/boot/vmlinuz-")), "{bad}");
        }
        assert!(!valid_kernel(&"7".repeat(101)));
        let r = unknown_kernel("cell2", "/boot/vmlinuz isn't there");
        assert_eq!((r.ok, r.kernel.as_deref()), (false, None));
        assert_eq!(r.blocking, ["Can't tell which kernel boots next: /boot/vmlinuz isn't there. Not restarted."]);
        assert_eq!(r.checks[0].state, "fail");
    }

    fn states(r: &PreReboot) -> Vec<(&str, &str)> {
        r.checks.iter().map(|c| (c.id.as_str(), c.state.as_str())).collect()
    }

    #[test]
    fn a_healthy_gb10_passes_and_the_recorded_lines_block_nothing() {
        let r = parse("cell2", K, &output(Gb10::Healthy));
        assert!(r.ok, "{:?}", r.blocking);
        assert!(r.blocking.is_empty());
        assert_eq!(r.kernel.as_deref(), Some(K));
        assert_eq!(
            states(&r),
            [("kernel", "ok"), ("initrd", "ok"), ("module", "ok"), ("userspace", "ok"), ("match", "ok"), ("dkms", "ok"), ("modules", "info"), ("kho", "info"), ("grub", "info")]
        );
        assert_eq!(
            r.lines(),
            [
                "Kernel that boots next: 7.0.0-1019-nvidia (where /boot/vmlinuz points)",
                "Its initrd: /boot/initrd.img-7.0.0-1019-nvidia is there",
                "Its NVIDIA module: 580.178.04",
                "NVIDIA userspace (libnvidia-compute-580): 580.178.04-0ubuntu0.24.04.1 (dpkg status ii)",
                "Module and userspace match: both 580.178.04",
                "No DKMS NVIDIA packages: none installed",
                "Prebuilt NVIDIA module packages (recorded): linux-modules-nvidia-580-open-7.0.0-1019-nvidia 7.0.0-1019.19+1, linux-modules-nvidia-580-open-nvidia-hwe-24.04 7.0.0-1019.19+1; one for 7.0.0-1019-nvidia: yes; the nvidia-hwe-24.04 metapackage: yes",
                "kho=off hotfix (DGX OS 7.6.0) (recorded): kho=off is on the kernel command line; nvidia-spark-grub-kho 1.0.0 is installed",
                "GRUB_DEFAULT (recorded): GRUB_DEFAULT=0 (a grub-reboot rollback needs GRUB_DEFAULT=saved)",
            ]
        );
    }

    #[test]
    fn each_blocking_failure_refuses_with_its_own_reason() {
        let table: [(Gb10, &str, &str); 6] = [
            (Gb10::NoInitrd, "initrd", "No initrd for 7.0.0-1019-nvidia: the September 2026 panic. Not restarted."),
            (Gb10::NoModule, "module", "No NVIDIA module for 7.0.0-1019-nvidia: it would boot without its GPU. Not restarted."),
            (Gb10::Mismatch, "match", "7.0.0-1019-nvidia would boot driver 580.95.05 against userspace 580.178.04. Not restarted."),
            (Gb10::NoUserspace, "userspace", "libnvidia-compute-580 isn't installed, so SparkBox can't tell which driver the GPU programs expect. Not restarted."),
            (Gb10::RemovedUserspace, "userspace", "libnvidia-compute-580 is removed (only its config is left): the driver's userspace isn't installed. Not restarted."),
            (Gb10::Dkms, "dkms", "nvidia-dkms-580-open is installed: a DKMS NVIDIA package is how kernels lost their initrd in the September 2026 panic. Not restarted."),
        ];
        for (state, id, why) in table {
            let r = parse("cell2", K, &output(state));
            assert!(!r.ok, "{state:?}");
            assert_eq!(r.blocking, [why], "{state:?}");
            let failed: Vec<&str> = r.checks.iter().filter(|c| c.state == "fail").map(|c| c.id.as_str()).collect();
            assert_eq!(failed, [id], "{state:?}: {:?}", r.checks);
        }
        // Several at once: each its own line.
        let both = output(Gb10::NoInitrd).replace("--- dkms\n", "--- dkms\nii  nvidia-dkms-580-open  580.178.04-0ubuntu1  arm64  NVIDIA DKMS package\nii  nvidia-dkms-580  580.178.04-0ubuntu1  arm64  x\n");
        let r = parse("cell2", K, &both);
        assert_eq!(r.blocking.len(), 2, "{:?}", r.blocking);
        assert!(r.blocking[1].starts_with("nvidia-dkms-580-open, nvidia-dkms-580 are installed: "), "{}", r.blocking[1]);
        // A package that's only remembered (rc), not installed, doesn't block.
        let rc = output(Gb10::Healthy).replace("--- dkms\n", "--- dkms\nrc  nvidia-dkms-580-open  580.95.05-0ubuntu1  arm64  NVIDIA DKMS package\n");
        assert!(parse("cell2", K, &rc).ok);
    }

    #[test]
    fn recorded_lines_never_block_whatever_they_say() {
        // No prebuilt module packages, no hotfix, GRUB_DEFAULT unset: recorded, still ok.
        let bare = "--- initrd\npresent\n--- modinfo\n580.178.04\n--- compute\nii 580.178.04-1\n--- dkms\n--- modules\n--- kho\n--- grub\n";
        let r = parse("gx10-0000", K, bare);
        assert!(r.ok, "{:?}", r.blocking);
        let detail = |id: &str| r.checks.iter().find(|c| c.id == id).unwrap().detail.clone();
        assert_eq!(detail("modules"), "none installed (the module check above is what counts)");
        assert_eq!(detail("kho"), "kho=off isn't on the kernel command line; nvidia-spark-grub-kho isn't installed");
        assert_eq!(detail("grub"), "not set (GRUB boots its first entry; a grub-reboot rollback needs GRUB_DEFAULT=saved)");
        // Only the kernel's own package, not the metapackage; GRUB_DEFAULT=saved (quoted, with a comment line).
        let some = bare
            .replace("--- modules\n", &format!("--- modules\nii  linux-modules-nvidia-580-open-{K}:arm64  7.0.0-1019.19+1  arm64  x\n"))
            .replace("--- grub\n", "--- grub\n#GRUB_DEFAULT=0\nGRUB_DEFAULT=\"saved\"\n");
        let r = parse("gx10-0000", K, &some);
        assert!(r.ok);
        let detail = |id: &str| r.checks.iter().find(|c| c.id == id).unwrap().detail.clone();
        assert_eq!(detail("modules"), format!("linux-modules-nvidia-580-open-{K} 7.0.0-1019.19+1; one for {K}: yes; the nvidia-hwe-24.04 metapackage: no"));
        assert_eq!(detail("grub"), "#GRUB_DEFAULT=0; GRUB_DEFAULT=\"saved\" (a grub-reboot rollback works)");
        // A different kernel's package isn't "one for K".
        let other = bare.replace("--- modules\n", "--- modules\nii  linux-modules-nvidia-580-open-7.0.0-1020-nvidia  7.0.0-1020.20+1  arm64  x\n");
        assert!(parse("cell1", K, &other).checks.iter().any(|c| c.id == "modules" && c.detail.contains("one for 7.0.0-1019-nvidia: no")));
    }

    #[test]
    fn an_answer_cut_off_before_its_end_blocks() {
        let full = output(Gb10::Healthy);
        let cut = &full[..full.find("--- kho").unwrap()];
        let r = parse("cell2", K, cut);
        assert!(!r.ok);
        assert_eq!(r.blocking, ["The checks didn't finish (nothing came back for kho, grub). Not restarted."]);
        assert!(!parse("cell2", K, "").ok);
    }

    /// C6′ reads the userspace driver's dpkg status with its version: only `ii` (or `hi`, held by
    /// the pin) counts as installed. Removed with its config kept (`rc`), it still has a version,
    /// and blocks; half-installed blocks too; a package dpkg doesn't know is "not installed".
    #[test]
    fn the_userspace_driver_counts_only_when_dpkg_says_it_is_installed() {
        let with = |compute: &str| parse("cell2", K, &output(Gb10::Healthy).replace("ii 580.178.04-0ubuntu0.24.04.1\n", &format!("{compute}\n")));
        let r = with("hi 580.178.04-0ubuntu0.24.04.1");
        assert!(r.ok, "held by the pin: {:?}", r.blocking);
        assert_eq!(r.checks.iter().find(|c| c.id == "userspace").unwrap().detail, "580.178.04-0ubuntu0.24.04.1 (dpkg status hi)");
        let r = with("rc 580.178.04-0ubuntu0.24.04.1");
        assert_eq!(r.blocking, ["libnvidia-compute-580 is removed (only its config is left): the driver's userspace isn't installed. Not restarted."]);
        assert_eq!(r.checks.iter().find(|c| c.id == "userspace").map(|c| (c.state.as_str(), c.detail.as_str())), Some(("fail", "removed, only its config is left (dpkg status rc, 580.178.04-0ubuntu0.24.04.1)")));
        assert!(!r.checks.iter().any(|c| c.id == "match"), "no match judged against a removed package");
        let r = with("iF 580.178.04-0ubuntu0.24.04.1");
        assert_eq!(r.blocking, ["libnvidia-compute-580 isn't fully installed (dpkg status iF): the driver's userspace may be broken. Not restarted."]);
        let r = with("iUR580.178.04-0ubuntu0.24.04.1");
        assert_eq!(r.blocking, ["libnvidia-compute-580 isn't fully installed (dpkg status iUR): the driver's userspace may be broken. Not restarted."]);
        for unknown in ["un ", "pn ", "dpkg-query: no packages found matching libnvidia-compute-580", "580.178.04-1", ""] {
            let r = with(unknown);
            assert_eq!(r.blocking, ["libnvidia-compute-580 isn't installed, so SparkBox can't tell which driver the GPU programs expect. Not restarted."], "{unknown:?}");
        }
    }

    /// C6′ ends with `true`: a node without GRUB_DEFAULT (or without /etc/default/grub) answers
    /// with exit 0, so its row isn't a failure, and GRUB_DEFAULT reads "not set" (recorded). Run
    /// by a real shell on this Mac, where most of step B's programs and files don't exist.
    #[test]
    fn a_node_without_grub_default_answers_ok_and_reads_not_set() {
        let out = std::process::Command::new("/bin/sh").args(["-c", &script(K)]).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        let r = parse("cell2", K, &String::from_utf8_lossy(&out.stdout));
        assert!(!r.checks.iter().any(|c| c.id == "answer"), "every section came: {:?}", r.checks);
        let grub = r.checks.iter().find(|c| c.id == "grub").unwrap();
        assert_eq!((grub.state.as_str(), grub.detail.as_str()), ("info", "not set (GRUB boots its first entry; a grub-reboot rollback needs GRUB_DEFAULT=saved)"));
        // Without the trailing `true`, grep's exit is the answer's: 2 for a missing file.
        let old = script(K).trim_end_matches("; true").to_string();
        let out = std::process::Command::new("/bin/sh").args(["-c", &old]).output().unwrap();
        assert_ne!(out.status.code(), Some(0));
    }

    #[test]
    fn the_fake_gb10_answers_exactly_the_two_reads() {
        assert_eq!(fake::answer(Gb10::Healthy, KERNEL).unwrap().0, format!("/boot/vmlinuz-{K}\n"));
        assert!(fake::answer(Gb10::Healthy, &script(K)).is_some());
        assert!(fake::answer(Gb10::Healthy, &script("7.0.0-1020-nvidia")).is_none());
        assert!(fake::answer(Gb10::Healthy, "systemctl reboot").is_none());
        assert_eq!(kernel_of(&fake::answer(Gb10::NoVmlinuz, KERNEL).unwrap().0), Err("/boot/vmlinuz isn't there".into()));
    }
}
