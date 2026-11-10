//! The GPU-stack guard (P1.25) and the holds preset (P1.27). Until the P6 lockstep flow exists,
//! an upgrade on one node never moves the kernel, the NVIDIA driver, CUDA, Docker, the network
//! stack or the DGX OS update metapackage: a selection that names one is refused before anything
//! runs, and a plan (apt's own dry run) that would upgrade, install or remove one is refused
//! before the upgrade starts. Nothing ever installs a 590 driver (it deadlocks CUDA graph capture
//! on GB10) or removes a 580 package. "Pin the GPU stack" holds every installed package the guard
//! names except fwupd, so that "Upgrade everything" passes the guard; fwupd isn't held, and
//! SparkBox doesn't upgrade it either yet.

use super::updates::{PendingPackage, SimulatedUpgrade};

/// SparkBox can't upgrade names that start with one of these yet. PLAN P1.25's anchored
/// patterns, plus (kept from round 2's list, so the guard doesn't get weaker) libcuda, mft and
/// libibverbs. libxnvctrl is part of NVIDIA's settings package family; upgrading it can
/// require nvidia-settings to move too, so routine selections must leave both pending.
const PREFIXES: &[&str] = &["linux-", "nvidia-", "libnvidia-", "libxnvctrl", "cuda-", "docker", "containerd", "moby-", "mlnx-", "doca-", "libcuda", "mft", "libibverbs"];

/// And these, exactly (rdma-core kept from round 2's list).
const EXACT: &[&str] = &["dgx-spark-ota-update-meta", "nvidia-spark-grub-kho", "fwupd", "rdma-core"];

/// What "Pin the GPU stack" leaves free of what the guard names: fwupd (P1.27).
const NOT_HELD: &[&str] = &["fwupd"];

/// Kernel, NVIDIA, CUDA, Docker, Mellanox/DOCA or the DGX OS updater: refused on one node.
/// Judged without apt's `:arch` suffix ("fwupd:arm64" is fwupd).
pub fn sensitive(name: &str) -> bool {
    let name = bare(name);
    PREFIXES.iter().any(|p| name.starts_with(p)) || EXACT.contains(&name)
}

/// A package name without apt's architecture suffix: "libfoo:arm64" → "libfoo".
pub fn bare(name: &str) -> &str {
    name.split_once(':').map(|(n, _)| n).unwrap_or(name)
}

/// "libfoo:arm64 1.0-1 → 2.0-1" (an upgrade line of a dry run) → "libfoo".
fn name_of(line: &str) -> &str {
    bare(line.split_whitespace().next().unwrap_or(""))
}

/// The packages a dry run would upgrade, install or remove that SparkBox can't upgrade yet.
pub fn sensitive_in(sim: &SimulatedUpgrade) -> Vec<String> {
    sim.upgrades.iter().chain(&sim.installs).chain(&sim.removals).map(|l| name_of(l)).filter(|n| sensitive(n)).map(String::from).collect()
}

/// Whether a plan (apt's dry run) may run on one node; Err with why not.
pub fn check_plan(sim: &SimulatedUpgrade) -> Result<(), String> {
    let installs: Vec<&str> = sim.upgrades.iter().chain(&sim.installs).map(|l| name_of(l)).collect();
    let driver_590: Vec<&str> = installs.iter().copied().filter(|n| n.starts_with("nvidia-driver-59")).collect();
    if !driver_590.is_empty() {
        return Err(format!("590 deadlocks CUDA graph capture on GB10: {}", driver_590.join(", ")));
    }
    if let Some(gone) = sim.removals.iter().map(|l| name_of(l)).find(|n| n.contains("-580")) {
        return Err(format!("This plan removes {gone} (a 580 package)"));
    }
    // A removal moves the stack as much as an upgrade does (a dist-upgrade that drops docker-ce
    // takes every container with it).
    let gone: Vec<&str> = sim.removals.iter().map(|l| name_of(l)).filter(|n| sensitive(n)).collect();
    if !gone.is_empty() {
        return Err(format!("SparkBox can't upgrade GPU-stack packages yet: this plan removes {}", gone.join(", ")));
    }
    check_selection(&installs)
}

/// A selection (or a plan's packages) that names GPU-stack packages: refused, naming them.
pub fn check_selection<S: AsRef<str>>(names: &[S]) -> Result<(), String> {
    let hit: Vec<&str> = names.iter().map(AsRef::as_ref).filter(|n| sensitive(n)).collect();
    if hit.is_empty() {
        Ok(())
    } else {
        Err(format!("SparkBox can't upgrade GPU-stack packages yet: {}", hit.join(", ")))
    }
}

/// "Upgrade security only": the pending security fixes that aren't GPU-stack and aren't held.
pub fn security_only(pending: &[PendingPackage]) -> Vec<String> {
    pending.iter().filter(|p| p.security && !sensitive(&p.name) && !p.held).map(|p| p.name.clone()).collect()
}

/// The installed packages "Pin the GPU stack" holds, in the order given, each once: every one
/// the guard names but fwupd. (A named amendment to PLAN P1.27, whose pattern left ten kinds of
/// guarded packages unheld, linux-firmware, docker-compose-plugin, libcuda1 and rdma-core among
/// them, so that any update of one refused "Upgrade everything" even after the pin.)
pub fn preset_packages(installed: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in installed.iter().map(|n| n.trim()) {
        let hit = sensitive(name) && !NOT_HELD.contains(&bare(name));
        if hit && !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::updates::{parse_simulation, parse_upgradable};

    #[test]
    fn the_guard_is_anchored_and_keeps_round_twos_names() {
        for n in [
            "linux-image-7.0.0-1020-nvidia",
            "linux-modules-nvidia-580-open-7.0.0-1019-nvidia",
            "nvidia-driver-580-open",
            "nvidia-container-toolkit",
            "libnvidia-compute-580",
            "libxnvctrl0",
            "libxnvctrl-dev",
            "cuda-toolkit-13-0",
            "docker-ce",
            "docker-buildx-plugin",
            "docker.io",
            "containerd.io",
            "moby-engine",
            "mlnx-ofed-kernel-utils",
            "doca-runtime",
            "dgx-spark-ota-update-meta",
            "nvidia-spark-grub-kho",
            "fwupd",
            "libcuda1",
            "mft",
            "libibverbs1",
            "rdma-core",
        ] {
            assert!(sensitive(n), "{n}");
        }
        // Anchored: a name that merely contains one isn't caught, and exact names are exact.
        for n in ["libbrotli1", "nginx", "google-chrome-stable", "python3-nvidia-ml", "python3-libxnvctrl", "fwupd-signed", "dgx-spark-ota-update-meta-doc", "rdma-core-dev", "openssl"] {
            assert!(!sensitive(n), "{n}");
        }
        // updates.rs keeps its own name for it.
        assert!(crate::ops::updates::sensitive("linux-headers-7.0.0-1020-nvidia"));
        assert!(!crate::ops::updates::sensitive("libbrotli1"));
    }

    /// apt names a package with its architecture at times ("fwupd:arm64"): judged without it, so
    /// an arch suffix doesn't slip a GPU-stack package past a selection, a plan or the pin.
    #[test]
    fn names_are_judged_without_their_arch_suffix() {
        assert_eq!((bare("fwupd:arm64"), bare("libfoo"), bare("libc6:armhf")), ("fwupd", "libfoo", "libc6"));
        for n in ["fwupd:arm64", "linux-image-7.0.0-1020-nvidia:arm64", "docker-ce:arm64", "rdma-core:arm64", "dgx-spark-ota-update-meta:arm64"] {
            assert!(sensitive(n), "{n}");
        }
        assert!(!sensitive("libbrotli1:arm64") && !sensitive("fwupd-signed:arm64"));
        assert_eq!(check_selection(&["libbrotli1:arm64", "fwupd:arm64"]), Err("SparkBox can't upgrade GPU-stack packages yet: fwupd:arm64".into()));
        let plan = parse_simulation("cell2", "Inst fwupd:arm64 [1.9.30-0ubuntu1~24.04.1] (1.9.31-0ubuntu1~24.04.1 Ubuntu:24.04/noble-updates [arm64])\n");
        assert_eq!(check_plan(&plan), Err("SparkBox can't upgrade GPU-stack packages yet: fwupd".into()));
        assert_eq!(sensitive_in(&plan), ["fwupd"]);
        let removes = parse_simulation("cell2", "Remv libnvidia-compute-580:arm64 [580.178.04-0ubuntu1]\n");
        assert_eq!(check_plan(&removes), Err("This plan removes libnvidia-compute-580 (a 580 package)".into()));
        assert_eq!(preset_packages(&["fwupd:arm64".into(), "docker-ce".into()]), ["docker-ce"], "fwupd isn't held, with or without its arch");
        let pending = parse_upgradable("fwupd:arm64/noble-security 1.9.31 arm64 [upgradable from: 1.9.30]\nlibbrotli1/noble-security 1.1.0-2ubuntu0.1 arm64 [upgradable from: 1.1.0-2build2]");
        assert_eq!(security_only(&pending), ["libbrotli1"]);
    }

    #[test]
    fn a_plan_with_the_gpu_stack_a_590_driver_or_a_580_removal_is_refused() {
        let plan = |text: &str| parse_simulation("cell2", text);
        assert_eq!(check_plan(&plan("Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-updates, Ubuntu:24.04/noble-security [arm64])\n")), Ok(()));
        let kernel = plan("Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-security [arm64])\nInst linux-image-7.0.0-1020-nvidia (7.0.0-1020.20 Ubuntu:24.04/noble-updates [arm64])\n");
        assert_eq!(check_plan(&kernel), Err("SparkBox can't upgrade GPU-stack packages yet: linux-image-7.0.0-1020-nvidia".into()));
        assert_eq!(sensitive_in(&kernel), ["linux-image-7.0.0-1020-nvidia"]);
        let driver = plan("Inst nvidia-driver-590-open (590.44.01-0ubuntu1 Ubuntu:24.04/noble-updates [arm64])\nRemv nvidia-driver-580-open [580.178.04-0ubuntu1]\n");
        assert_eq!(check_plan(&driver), Err("590 deadlocks CUDA graph capture on GB10: nvidia-driver-590-open".into()));
        let removal = plan("Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-security [arm64])\nRemv libnvidia-compute-580 [580.178.04-0ubuntu1]\n");
        assert_eq!(check_plan(&removal), Err("This plan removes libnvidia-compute-580 (a 580 package)".into()));
        // Any other GPU-stack removal too: Docker, the container toolkit, a kernel's NVIDIA modules.
        let gone = plan(
            "Inst libbrotli1 [1.1.0-2build2] (1.1.0-2ubuntu0.1 Ubuntu:24.04/noble-security [arm64])
Remv docker-ce [5:29.0.0-1~ubuntu.24.04~noble]
Remv nvidia-container-toolkit [1.17.8-1]
Remv linux-modules-nvidia-open-7.0.0-1019-nvidia [7.0.0-1019.19]
Remv libfoo-doc [1.0-1]
",
        );
        assert_eq!(
            check_plan(&gone),
            Err("SparkBox can't upgrade GPU-stack packages yet: this plan removes docker-ce, nvidia-container-toolkit, linux-modules-nvidia-open-7.0.0-1019-nvidia".into())
        );
        assert_eq!(sensitive_in(&gone), ["docker-ce", "nvidia-container-toolkit", "linux-modules-nvidia-open-7.0.0-1019-nvidia"]);
        assert_eq!(check_plan(&plan("Remv libfoo-doc [1.0-1]\n")), Ok(()), "a routine removal passes");
        assert_eq!(check_plan(&plan("Remv linux-image-7.0.0-1019-nvidia [7.0.0-1019.19]\n")), Err("SparkBox can't upgrade GPU-stack packages yet: this plan removes linux-image-7.0.0-1019-nvidia".into()));
        // An upgrade of a GPU-stack package already installed counts as much as a new one.
        let upgrade = plan("Inst libnvidia-compute-580 [580.95.05-0ubuntu1] (580.178.04-0ubuntu1 Ubuntu:24.04/noble-updates [arm64])\n");
        assert_eq!(check_plan(&upgrade), Err("SparkBox can't upgrade GPU-stack packages yet: libnvidia-compute-580".into()));
        // A held package is only kept back: the plan passes.
        assert_eq!(check_plan(&plan("The following packages have been kept back:\n  linux-image-nvidia\n0 upgraded, 0 newly installed, 0 to remove and 1 not upgraded.\n")), Ok(()));
        assert_eq!(check_selection(&["libbrotli1", "docker-ce"]), Err("SparkBox can't upgrade GPU-stack packages yet: docker-ce".into()));
        assert_eq!(check_selection(&["libbrotli1"]), Ok(()));
    }

    #[test]
    fn security_only_never_picks_a_security_kernel() {
        let pending = parse_upgradable(
            "linux-image-7.0.0-1020-nvidia/noble-security 7.0.0-1020.20 arm64 [upgradable from: 7.0.0-1019.19]
libbrotli1/noble-updates,noble-security 1.1.0-2ubuntu0.1 arm64 [upgradable from: 1.1.0-2build2]
nginx/noble-updates,noble-security 1.24.0-2ubuntu7.19 arm64 [upgradable from: 1.24.0-2ubuntu7.18]
google-chrome-stable/stable,stable 156.0.8078.73-1 arm64 [upgradable from: 156.0.8078.47-1]",
        );
        let kernel = pending.iter().find(|p| p.name.starts_with("linux-image-")).unwrap();
        assert!(kernel.security && kernel.sensitive, "the kernel comes from noble-security");
        let mut held = pending.clone();
        held.iter_mut().find(|p| p.name == "nginx").unwrap().held = true;
        assert_eq!(security_only(&pending), ["libbrotli1", "nginx"]);
        assert_eq!(security_only(&held), ["libbrotli1"], "held stays put");
    }

    #[test]
    fn nvidia_control_libraries_stay_out_of_routine_updates_and_in_the_pin() {
        let pending = parse_upgradable(
            "libxnvctrl0/noble-security 620.70.06-1ubuntu1 arm64 [upgradable from: 615.71.10-1ubuntu1]\nnvidia-settings/unknown 620.70.06-1ubuntu1 arm64 [upgradable from: 615.71.10-1ubuntu1]\nlibsqlite3-0/noble-security 3.45.1-1ubuntu2.9 arm64 [upgradable from: 3.45.1-1ubuntu2.8]\n",
        );
        let routine: Vec<_> = pending.iter().filter(|p| !p.sensitive && !p.held).map(|p| p.name.as_str()).collect();
        assert_eq!(routine, ["libsqlite3-0"]);
        assert_eq!(security_only(&pending), ["libsqlite3-0"]);
        assert_eq!(preset_packages(&["libxnvctrl0:arm64".into(), "libxnvctrl-dev".into(), "libsqlite3-0".into()]), ["libxnvctrl0:arm64", "libxnvctrl-dev"]);
        assert!(check_selection(&["libxnvctrl0:arm64"]).is_err());
        for action in ["Inst libxnvctrl0:arm64 [615] (620 NVIDIA [arm64])", "Inst libxnvctrl-dev (620 NVIDIA [arm64])", "Remv libxnvctrl0 [615]"] {
            let plan = parse_simulation("cell2", action);
            assert!(check_plan(&plan).is_err(), "{action}");
            assert_eq!(sensitive_in(&plan).len(), 1, "{action}");
        }
    }

    #[test]
    fn the_preset_is_the_gpu_stack_and_nothing_else() {
        let installed: Vec<String> = [
            "containerd.io",
            "cuda-toolkit-13-0",
            "dgx-spark-ota-update-meta",
            "dgx-spark-ota-update-meta-doc",
            "docker-ce",
            "docker-ce-cli",
            "fwupd",
            "google-chrome-stable",
            "libbrotli1",
            "libcuda1",
            "libnvidia-compute-580",
            "linux-headers-7.0.0-1019-nvidia",
            "linux-image-7.0.0-1019-nvidia",
            "linux-modules-nvidia-580-open-7.0.0-1019-nvidia",
            "linux-nvidia-hwe-24.04",
            "linux-tools-common",
            "mlnx-ofed-kernel-utils",
            "nginx",
            "nvidia-container-toolkit",
            "nvidia-driver-580-open",
            "nvidia-driver-580-open",
            "rdma-core",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            preset_packages(&installed),
            [
                "containerd.io",
                "cuda-toolkit-13-0",
                "dgx-spark-ota-update-meta",
                "docker-ce",
                "docker-ce-cli",
                "libcuda1",
                "libnvidia-compute-580",
                "linux-headers-7.0.0-1019-nvidia",
                "linux-image-7.0.0-1019-nvidia",
                "linux-modules-nvidia-580-open-7.0.0-1019-nvidia",
                "linux-nvidia-hwe-24.04",
                "linux-tools-common",
                "mlnx-ofed-kernel-utils",
                "nvidia-container-toolkit",
                "nvidia-driver-580-open",
                "rdma-core",
            ],
            "every guarded package but fwupd"
        );
    }

    /// After the pin, nothing the guard names is left free but fwupd: a dry run of everything
    /// that upgrades the rest of what's installed passes the guard. (Round 3's pin left ten kinds
    /// of guarded packages unheld, so their updates refused "Upgrade everything" for good.)
    #[test]
    fn after_the_pin_only_fwupd_of_the_guarded_packages_is_free() {
        let installed: Vec<String> = [
            "docker-ce",
            "docker-ce-cli",
            "docker-ce-rootless-extras",
            "docker-compose-plugin",
            "docker-buildx-plugin",
            "containerd.io",
            "linux-image-7.0.0-1019-nvidia",
            "linux-firmware",
            "linux-libc-dev",
            "linux-tools-common",
            "linux-base",
            "libcuda1",
            "libibverbs1",
            "rdma-core",
            "mft",
            "fwupd",
            "nvidia-spark-grub-kho",
            "libbrotli1",
            "nginx",
        ]
        .map(String::from)
        .to_vec();
        let held = preset_packages(&installed);
        let free: Vec<&String> = installed.iter().filter(|n| sensitive(n) && !held.contains(n)).collect();
        assert_eq!(free, ["fwupd"]);
        // What apt may still move with the stack held: the packages outside it.
        let rest: String = installed.iter().filter(|n| !held.contains(n) && *n != "fwupd").map(|n| format!("Inst {n} [1] (2 Ubuntu:24.04/noble-updates [arm64])\n")).collect();
        let kept = format!("The following packages have been kept back:\n  {}\n{rest}", held.join(" "));
        let sim = crate::ops::updates::parse_simulation("cell2", &kept);
        assert_eq!((sim.upgrades.len(), sim.held_back.len()), (2, held.len()));
        assert_eq!(check_plan(&sim), Ok(()));
        // fwupd isn't held, and isn't upgraded on one node either.
        assert_eq!(check_plan(&parse_simulation("cell2", "Inst fwupd [1.9.30-0ubuntu1~24.04.1] (1.9.31-0ubuntu1~24.04.1 Ubuntu:24.04/noble-updates [arm64])\n")), Err("SparkBox can't upgrade GPU-stack packages yet: fwupd".into()));
    }
}
