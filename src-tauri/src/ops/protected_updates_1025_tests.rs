// Included in protected_updates::tests: all connections terminate at local fake nodes.
const CONTINUED_OBSERVED: &str = include_str!("../../../src/test/fixtures/protected-updates-1025.txt");
const BOOT_1024: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const BOOT_1025: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
fn original_record(demo: bool) -> Record {
    Record {
        node: "cell2".into(),
        demo,
        instance: crate::identity::instance_name().into(),
        phase: "installing".into(),
        boot: BOOT.into(),
        task: Some("original-1024-task".into()),
        plan: fixture().into_iter().map(|p| PlanEntry { name: p.name, to: p.to }).collect(),
        message: "Original update awaiting verification".into(),
        previous: None,
        ..Default::default()
    }
}
fn continuation_plan() -> Vec<PlanEntry> {
    reviewed_1025().into_iter().map(|p| PlanEntry { name: p.name, to: p.to }).collect()
}
#[test]
fn continuation_matches_actual_apt_output_and_rejects_every_changed_row() {
    let s = updates::parse_simulation("cell2", CONTINUED_OBSERVED);
    let review = Review::Kernel1025;
    review.check_plan(&s).unwrap();
    review.check_origin(&s).unwrap();
    let actual: serde_json::Value = serde_json::from_str(include_str!("../../../docs/fixtures/kernel1025-plan.json")).unwrap();
    let actual: Vec<PlannedPkg> = serde_json::from_value(actual["packages"].clone()).unwrap();
    assert_eq!(s.plan, actual);
    assert_eq!(s.upgrades.len(), 9);
    assert_eq!(s.installs.len(), 8);
    assert!(super::super::gpu_guard::check_plan(&s).is_err());
    assert!(check_plan(&s).is_err(), "1025 is never a fresh 1019 update");
    for i in 0..s.plan.len() {
        for field in 0..5 {
            let mut bad = s.clone();
            match field {
                0 => bad.plan[i].name.push('x'),
                1 => bad.plan[i].to.push('1'),
                2 => bad.plan[i].from = Some("other".into()),
                3 => bad.plan[i].new = !bad.plan[i].new,
                _ => {
                    bad.plan.remove(i);
                }
            }
            assert!(review.check_plan(&bad).is_err(), "{i}/{field}");
        }
    }
    for change in 0..5 {
        let mut bad = s.clone();
        match change {
            0 => bad.removes.push("old-kernel".into()),
            1 => bad.removals.push("old".into()),
            2 => bad.held_back.push("held".into()),
            3 => bad.phased.push("deferred".into()),
            _ => bad.plan.push(bad.plan[0].clone()),
        }
        assert!(review.check_plan(&bad).is_err());
    }
    for (from, to) in [("Ubuntu:24.04/noble-updates", "PPA:unknown"), ("[all]", "[arm64]"), ("[arm64]", "[amd64]")] {
        let bad = updates::parse_simulation("cell2", &CONTINUED_OBSERVED.replacen(from, to, 1));
        assert!(review.check_origin(&bad).is_err());
    }
    let all = review.installed();
    assert_eq!(all.len(), 32);
    let installed = all.iter().map(|p| format!("{} ii {}\n", p.name, p.to)).collect::<String>();
    assert!(installed_ok_for(&installed, review));
    for p in &all {
        assert!(!installed_ok_for(&installed.replace(&format!("{} ii {}\n", p.name, p.to), ""), review));
    }
}
async fn setup_continuation(fault: usize) -> (Arc<Ops>, Node, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    setup_1025(fault, "cell2", false).await
}
async fn setup_1025(fault: usize, id: &str, fresh: bool) -> (Arc<Ops>, Node, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let serial = SERIAL.fetch_add(1, Ordering::SeqCst);
    let stage = Arc::new(AtomicUsize::new(0));
    let phase = stage.clone();
    let faults = Arc::new(AtomicUsize::new(fault));
    let m = faults.clone();
    let dry_count = AtomicUsize::new(0);
    let script: Script = Arc::new(move |cmd, _| {
        let fault = m.load(Ordering::SeqCst);
        let stage = phase.load(Ordering::SeqCst);
        let root = cmd.starts_with("sudo -k -S -p '' ");
        let cmd = cmd.strip_prefix("sudo -k -S -p '' ").unwrap_or(cmd);
        if cmd.contains("systemd-run --unit sparkbox-upgrade-") {
            phase.store(1, Ordering::SeqCst);
            return None;
        }
        if cmd.contains("/boot/grub/grub.cfg") && (!root || fault == 9) {
            return Some(("permission denied".into(), 2));
        }
        if cmd == BEFORE {
            let mut b = if fresh { good_before() } else { good_before().replace(OLD_KERNEL, KERNEL).replace(BOOT, BOOT_1024) };
            match fault {
                1 => b = b.replace("--- jobs\n", "--- jobs\n123\n"),
                7 => b = b.replace(if fresh { OLD_KERNEL } else { KERNEL }, "unreviewed-kernel"),
                11 => b = b.replace("30000000", "50"),
                12 => b = b.replace(if fresh { BOOT } else { BOOT_1024 }, "changed-boot"),
                _ => {}
            }
            return Some((b, 0));
        }
        if cmd == DEPENDENCIES {
            return Some((if fault == 30 { "E: broken" } else { "Reading package lists..." }.into(), if fault == 30 { 100 } else { 0 }));
        }
        if cmd.starts_with("apt-get -s ") {
            let count = dry_count.fetch_add(1, Ordering::SeqCst);
            let mut s = if fresh { FLEET_OBSERVED } else { CONTINUED_OBSERVED }.to_string();
            if fault == 2 || (fault == 14 && count > 0) {
                s = s.replacen(CONTINUED_KV, "7.0.0-1026.27~24.04.1", 1);
            }
            if fault == 10 {
                s = s.replacen("Ubuntu:24.04/noble-updates", "PPA:unknown", 1);
            }
            return Some((s, 0));
        }
        if cmd == updates::STATUS_SCRIPT {
            let held = if fault == 3 {
                "linux-crashdump\n"
            } else if fault == 31 {
                "nvidia-spark-initcall-bl\n"
            } else {
                ""
            };
            let pending =
                if fault == 24 { "linux-nvidia-hwe-24.04/noble-updates 7.0.0-1026.27~24.04.1 arm64 [upgradable from: 7.0.0-1025.26~24.04.1]\n" } else { "" };
            return Some((format!("--- upgradable\n{pending}--- held\n{held}--- reboot\n--- unattended\n--- checked\n"), 0));
        }
        if cmd.starts_with("echo '--- initrd'") {
            let bad = fault == 5
                || (fault == 20 && cmd.contains(CONTINUED_KERNEL))
                || (fault == 21 && cmd.contains(KERNEL))
                || (fault == 22 && cmd.contains(OLD_KERNEL));
            return Some((if bad { PRE.replace("present", "") } else { PRE.into() }, 0));
        }
        if cmd == RECOVERY_SETTING {
            return Some((if fault == 34 { "changed setting" } else { RECOVERY_SETTING_TEXT }.into(), if fault == 35 { 1 } else { 0 }));
        }
        if cmd == RECOVERY_GRUB {
            if !fresh { return Some((OLD_KERNEL.into(), 0)); }
            let fix = if fault == 32 { "" } else { "initcall_blacklist=tegra234_cbb_init,power_wrap_init" };
            let recovery = if fault == 33 { "".into() } else { format!("linux /boot/vmlinuz-{OLD_KERNEL} recovery {fix}\n") };
            return Some((format!("linux /boot/vmlinuz-{OLD_KERNEL} {fix}\n{recovery}"), if fault == 36 { 1 } else { 0 }));
        }
        if cmd == NEXT_GRUB || cmd == CONTINUED_GRUB || cmd == RECOVERY_1024_GRUB {
            if (cmd == RECOVERY_1024_GRUB && fault == 15) || (cmd == CONTINUED_GRUB && fault == 16) {
                return Some((String::new(), 1));
            }
            let kernel = if cmd != CONTINUED_GRUB { KERNEL } else { CONTINUED_KERNEL };
            let fix = if fault == 23 && cmd == CONTINUED_GRUB { "" } else { "initcall_blacklist=tegra234_cbb_init,power_wrap_init" };
            return Some((format!("linux /boot/vmlinuz-{kernel} {fix}\nlinux /boot/vmlinuz-{kernel} recovery {fix}\n"), 0));
        }
        if cmd == super::super::prereboot::KERNEL {
            return Some((format!("/boot/vmlinuz-{}", if stage == 0 { if fresh { OLD_KERNEL } else { KERNEL } } else { CONTINUED_KERNEL }), 0));
        }
        if cmd == installed_command_for(Review::Initial) || cmd == installed_command_for(Review::Kernel1025) || cmd == installed_command_for(Review::Fleet1025) {
            let all = if fresh { Review::Fleet1025.installed() } else if stage == 0 { Review::Initial.installed() } else { Review::Kernel1025.installed() };
            return Some((
                all.iter()
                    .enumerate()
                    .map(|(i, p)| format!("{} {} {}\n", p.name, if (fault == 6 || fault == 25) && i == 0 { "iU" } else { "ii" }, p.to))
                    .collect(),
                0,
            ));
        }
        if cmd == super::super::postboot::READ {
            let boot = if fault == 12 {
                BOOT
            } else if stage < 2 || fault == 27 {
                if fresh { BOOT } else { BOOT_1024 }
            } else {
                BOOT_1025
            };
            let kernel = if stage < 2 || fault == 26 { if fresh { OLD_KERNEL } else { KERNEL } } else { CONTINUED_KERNEL };
            let state = if fault == 4 || fault == 28 { "starting" } else { "running" };
            let fabric = if fault == 13 { "enp1s0f1np1 down\n" } else { "enp1s0f1np1 200000 9000\nenP2p1s0f1np1 200000 9000\n" };
            let failed = if fault == 29 { "bad.service\n" } else { "" };
            return Some((format!("--- boot\n{boot}\n--- kernel\n{kernel}\n--- gpu\nNVIDIA GB10, {DRIVER}\n--- fabric\n{fabric}--- docker\n29.6.2\n--- system\n{state}\n--- failed\n{failed}--- end\n"),0));
        }
        if cmd == "cat /proc/cmdline" {
            return Some((if fault == 8 { "kho=off" } else { "kho=off initcall_blacklist=tegra234_cbb_init,power_wrap_init" }.into(), 0));
        }
        None
    });
    let (node, port) = fake_node(&temp(&format!("continuation-home-{serial}")), script).await;
    let shim = std::fs::read_to_string(node.bin_dir.join("apt-get")).unwrap();
    node.bin("apt-under-test", &shim);
    node.bin(
        "apt-get",
        r#"#!/bin/sh
for a in "$@"; do shift; case "$a" in *=*) a=${a%%=*};; esac; set -- "$@" "$a"; done
exec "$(dirname "$0")/apt-under-test" "$@"
"#,
    );
    let (live, _) = connect(&temp(&format!("continuation-app-{serial}")), &[(id, port)]).await;
    let ops = Ops::with_poll(live, Arc::new(Mutex::new(DemoCluster::new(&Inventory::example()))), Tasks::new(None), Duration::from_millis(5));
    if !fresh { save(&ops.live, &original_record(false)).unwrap(); }
    (ops, node, stage, faults)
}
#[tokio::test]
async fn continuation_refusals_restore_original_reservation_without_root_install() {
    for case in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 31] {
        let (ops, node, _, _) = setup_continuation(case).await;
        let before = ops.system_status(false).unwrap().unwrap();
        let id = ops.system_start(false, "cell2".into(), continuation_plan()).unwrap();
        let t = done(&ops, &id).await;
        assert_eq!(t.state, TaskState::Failed, "case {case}: {t:?}");
        assert!(node.ran("systemd-run").is_empty(), "case {case}");
        assert_eq!(ops.system_status(false).unwrap().unwrap(), before, "case {case}");
        assert!(check_load(&ops.live, false, &["cell2".into()]).is_err());
        assert!(ops.system_start(false, "cell1".into(), fixture().iter().map(|p| PlanEntry { name: p.name.clone(), to: p.to.clone() }).collect()).is_err());
    }
}
#[tokio::test]
async fn continuation_keeps_history_and_requires_new_kernel_boot_and_all_health_checks() {
    let (ops, node, stage, fault) = setup_continuation(0).await;
    let original = ops.system_status(false).unwrap().unwrap();
    let sim = ops.system_preview(false, "cell2").await.unwrap();
    assert_eq!(ops.system_status(false).unwrap().unwrap(), original, "preview cannot alter checkpoint");
    let id = ops.system_start(false, "cell2".into(), sim.shown()).unwrap();
    assert!(ops.system_start(false, "cell2".into(), sim.shown()).is_err());
    let t = done(&ops, &id).await;
    assert_eq!(t.state, TaskState::Ok, "{t:?}");
    assert!(t.retry.is_none());
    let r = ops.system_status(false).unwrap().unwrap();
    assert_eq!(r.previous.as_deref(), Some(&original));
    assert_eq!(r.boot, BOOT_1024);
    assert_eq!(r.task.as_deref(), Some(id.as_str()));
    assert!(ops.system_discard_review(false).is_err());
    let starts = node.ran("systemd-run");
    assert_eq!(starts.len(), 1);
    for p in reviewed_1025() {
        assert!(starts[0].contains(&format!("{}={}", p.name, p.to)));
    }
    assert!(!starts[0].contains("--only-upgrade"));
    let recovered = Ops::with_poll(ops.live.clone(), ops.demo.clone(), Tasks::new(None), Duration::from_millis(5));
    assert!(recovered.system_verify(false).await.is_err(), "installed kernel without restart is insufficient");
    stage.store(2, Ordering::SeqCst);
    for f in [8, 9, 13, 15, 16, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30] {
        fault.store(f, Ordering::SeqCst);
        assert!(recovered.system_verify(false).await.is_err(), "fault {f}");
        assert_eq!(recovered.system_status(false).unwrap().unwrap(), r, "fault {f} must retain checkpoint");
        assert!(check_load(&ops.live, false, &["cell2".into()]).is_err());
    }
    fault.store(0, Ordering::SeqCst);
    let verified = recovered.system_verify(false).await.unwrap();
    assert_eq!(verified.phase, "verified");
    assert_eq!(verified.previous.as_deref(), Some(&original));
    assert!(check_load(&ops.live, false, &["cell2".into()]).is_ok());
    assert_eq!(node.ran("systemd-run").len(), 1, "reattachment never repeats installation");
    assert!(node.raw.lock().unwrap().iter().all(|c| super::super::classify::power_alone(c).is_none()));
}
#[tokio::test]
async fn continuation_legacy_owner_plan_and_interruption_guards() {
    let (ops, node, _, _) = setup_continuation(0).await;
    let old = original_record(false);
    assert!(!serde_json::to_string(&old).unwrap().contains("previous"), "legacy schema stays readable");
    for change in 0..7 {
        let mut bad = old.clone();
        match change {
            0 => bad.node = "cell1".into(),
            1 => bad.boot.clear(),
            2 => bad.task = None,
            3 => bad.phase = "verified".into(),
            4 => bad.plan[0].to.push('9'),
            5 => bad.instance = "another".into(),
            _ => bad.demo = true,
        }
        save(&ops.live, &bad).unwrap();
        assert!(ops.system_start(false, "cell2".into(), continuation_plan()).is_err(), "case {change}");
    }
    save(&ops.live, &old).unwrap();
    let checking =
        Record { phase: "checking".into(), boot: String::new(), task: None, plan: continuation_plan(), previous: Some(Box::new(old.clone())), ..old.clone() };
    save(&ops.live, &checking).unwrap();
    let recovered = Ops::with_poll(ops.live.clone(), ops.demo.clone(), Tasks::new(None), Duration::from_millis(5));
    assert!(recovered.system_start(false, "cell2".into(), continuation_plan()).is_err());
    assert!(recovered.system_verify(false).await.is_err());
    assert_eq!(recovered.system_discard_review(false).unwrap(), old, "discard restores, never releases, the original");
    let installed = Record { phase: "installing".into(), boot: BOOT_1024.into(), task: Some("possible-start".into()), ..checking };
    save(&ops.live, &installed).unwrap();
    recovered.system_not_started(false, "cell2", "other-task", "late failure");
    assert_eq!(recovered.system_status(false).unwrap().unwrap(), installed);
    assert!(recovered.system_discard_review(false).is_err());
    assert!(recovered.system_start(false, "cell2".into(), continuation_plan()).is_err());
    assert!(node.ran("systemd-run").is_empty());
    let mut wrong_owner = installed;
    wrong_owner.previous.as_mut().unwrap().instance = "another".into();
    save(&ops.live, &wrong_owner).unwrap();
    assert!(recovered.system_status(false).is_err());
}
#[tokio::test]
async fn continuation_lost_start_reply_is_followed_once_with_history_retained() {
    let (ops, node, _, _) = setup_continuation(0).await;
    node.close_without_status("systemd-run --unit sparkbox-upgrade-");
    let id = ops.system_start(false, "cell2".into(), continuation_plan()).unwrap();
    let task = done(&ops, &id).await;
    assert_eq!(task.state, TaskState::Ok, "{task:?}");
    assert!(task.log.iter().any(|line| line.contains("start command's reply was lost")));
    assert_eq!(node.ran("systemd-run").len(), 1);
    assert_eq!(ops.system_status(false).unwrap().unwrap().previous.as_deref(), Some(&original_record(false)));
    assert!(ops.system_discard_review(false).is_err());
}
#[tokio::test]
async fn continuation_failed_installer_stays_reserved_and_cannot_be_repeated() {
    let (ops, node, _, _) = setup_continuation(0).await;
    node.bin("apt-under-test", "#!/bin/sh\necho 'installation interrupted'\nexit 100\n");
    let id = ops.system_start(false, "cell2".into(), continuation_plan()).unwrap();
    let task = done(&ops, &id).await;
    assert_eq!(task.state, TaskState::Failed, "{task:?}");
    let saved = ops.system_status(false).unwrap().unwrap();
    assert_eq!(saved.phase, "installing");
    assert_eq!(saved.previous.as_deref(), Some(&original_record(false)));
    let recovered = Ops::with_poll(ops.live.clone(), ops.demo.clone(), Tasks::new(None), Duration::from_millis(5));
    assert!(recovered.system_discard_review(false).is_err());
    assert!(recovered.system_start(false, "cell2".into(), continuation_plan()).is_err());
    assert!(check_load(&ops.live, false, &["cell2".into()]).is_err());
    assert_eq!(node.ran("systemd-run").len(), 1);
}
