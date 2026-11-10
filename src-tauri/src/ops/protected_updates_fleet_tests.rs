// Synthetic direct plan, assembled from archived evidence. No real node is contacted.
const FLEET_OBSERVED: &str = include_str!("../../../src/test/fixtures/protected-updates-fleet1025.txt");
fn fleet_plan() -> Vec<PlanEntry> {
    reviewed_fleet_1025().into_iter().map(|p| PlanEntry { name: p.name, to: p.to }).collect()
}
fn verified_cell2() -> Record {
    Record { phase: "verified".into(), boot: BOOT_1024.into(), task: Some("verified-1025-task".into()),
        plan: continuation_plan(), previous: Some(Box::new(original_record(false))), ..original_record(false) }
}
#[test]
fn fleet_exact_plan_origins_and_every_pin_remain_bounded() {
    let review = Review::Fleet1025;
    let sim = updates::parse_simulation("cell1", FLEET_OBSERVED);
    review.check_plan(&sim).unwrap();
    review.check_origin(&sim).unwrap();
    let fixture: Vec<PlannedPkg> = serde_json::from_str(include_str!("../../../src/test/fixtures/protected-updates-fleet1025.json")).unwrap();
    assert_eq!(fixture, sim.plan);
    assert_eq!(sim.upgrades.len(), 16);
    assert_eq!(sim.installs.len(), 8);
    assert_eq!(review.installed().len(), 24);
    assert!(super::super::gpu_guard::check_plan(&sim).is_err());
    assert!(Review::Initial.check_plan(&sim).is_err());
    assert!(Review::Kernel1025.check_plan(&sim).is_err());
    for i in 0..sim.plan.len() {
        for field in 0..5 {
            let mut bad = sim.clone();
            match field {
                0 => bad.plan[i].name.push('x'), 1 => bad.plan[i].to.push('x'),
                2 => bad.plan[i].from = Some("other".into()), 3 => bad.plan[i].new = !bad.plan[i].new,
                _ => { bad.plan.remove(i); }
            }
            assert!(review.check_plan(&bad).is_err());
        }
        let name = &sim.plan[i].name;
        let line = FLEET_OBSERVED.lines().find(|l| l.starts_with(&format!("Inst {name} "))).unwrap();
        for replacement in [line.replace(" [", " foreign ["), line.replace("arm64", "amd64").replace("[all]", "[arm64]")] {
            let bad = updates::parse_simulation("cell1", &FLEET_OBSERVED.replace(line, &replacement));
            assert!(review.check_origin(&bad).is_err(), "{name}");
        }
    }
    for change in 0..5 {
        let mut bad = sim.clone();
        match change { 0 => bad.removes.push("old".into()), 1 => bad.removals.push("old".into()), 2 => bad.held_back.push("held".into()), 3 => bad.phased.push("later".into()), _ => bad.plan.push(bad.plan[0].clone()) }
        assert!(review.check_plan(&bad).is_err());
    }
    assert!(super::super::classify::read_only(RECOVERY_SETTING));
    assert!(!super::super::classify::escalates(RECOVERY_SETTING));
    let expected = super::super::postboot::Expected { boot: Some("current".into()), kernel: Some(OLD_KERNEL.into()), driver: Some(DRIVER.into()), ..Default::default() };
    assert!(!super::super::postboot::judge_running("", &expected).is_empty());
}
#[tokio::test]
async fn fleet_prerequisites_refuse_before_install_and_keep_verified_history() {
    for fault in [1, 2, 3, 4, 5, 7, 9, 10, 11, 12, 13, 14, 29, 31, 33, 34, 35, 36] {
        let (ops, node, _, _) = setup_1025(fault, "cell1", true).await;
        let old = verified_cell2();
        save(&ops.live, &old).unwrap();
        let id = ops.system_start(false, "cell1".into(), fleet_plan()).unwrap();
        let task = done(&ops, &id).await;
        assert_eq!(task.state, TaskState::Failed, "fault {fault}: {task:?}");
        assert!(node.ran("systemd-run").is_empty(), "fault {fault}");
        let saved = ops.system_status(false).unwrap().unwrap();
        assert_eq!(saved.phase, "refused");
        assert_eq!(saved.completed, vec![old]);
        assert!(check_load(&ops.live, false, &["cell1".into()]).is_ok());
    }
}
#[tokio::test]
async fn fleet_all_three_stable_ids_install_once_and_verify_only_after_healthy_new_boot() {
    for target in ["cell1", "cell3", "gx10-0000"] {
        let (ops, node, stage, fault) = setup_1025(0, target, true).await;
        let old = verified_cell2();
        save(&ops.live, &old).unwrap();
        let sim = ops.system_preview(false, target).await.unwrap();
        assert_eq!(ops.system_status(false).unwrap().unwrap(), old);
        let id = ops.system_start(false, target.into(), sim.shown()).unwrap();
        assert!(ops.system_start(false, target.into(), sim.shown()).is_err());
        let task = done(&ops, &id).await;
        assert_eq!(task.state, TaskState::Ok, "{target}: {task:?}");
        assert!(task.retry.is_none());
        let saved = ops.system_status(false).unwrap().unwrap();
        assert!(saved.fresh1025);
        assert!(saved.previous.is_none());
        assert_eq!(saved.completed, vec![old]);
        let recovered = Ops::with_poll(ops.live.clone(), ops.demo.clone(), Tasks::new(None), Duration::from_millis(5));
        assert!(recovered.system_verify(false).await.is_err());
        assert!(recovered.system_discard_review(false).is_err());
        assert!(recovered.system_preview(false, "cell3").await.is_err());
        assert!(check_load(&ops.live, false, &[target.into()]).is_err());
        assert!(check_routine(&ops.live, false, target).is_err());
        stage.store(2, Ordering::SeqCst);
        for f in [8, 9, 13, 16, 20, 22, 23, 24, 25, 26, 27, 28, 29, 30, 32, 33, 36] {
            fault.store(f, Ordering::SeqCst);
            assert!(recovered.system_verify(false).await.is_err(), "{target}, fault {f}");
            assert_eq!(recovered.system_status(false).unwrap().unwrap(), saved);
        }
        fault.store(0, Ordering::SeqCst);
        let verified = recovered.system_verify(false).await.unwrap();
        assert_eq!(verified.phase, "verified");
        assert_eq!(verified.completed, saved.completed);
        assert!(check_load(&ops.live, false, &[target.into()]).is_ok());
        let starts = node.ran("systemd-run");
        assert_eq!(starts.len(), 1);
        for p in reviewed_fleet_1025() { assert!(starts[0].contains(&format!("{}={}", p.name, p.to))); }
        assert!(!starts[0].contains("1024"));
        assert!(node.raw.lock().unwrap().iter().all(|c| super::super::classify::power_alone(c).is_none()));
    }
}
#[tokio::test]
async fn fleet_history_owner_source_stable_id_and_interrupted_review_guards() {
    let (ops, node, _, _) = setup_1025(0, "cell1", true).await;
    for id in ["cell2", "cell4", "unknown"] {
        assert!(ops.system_start(false, id.into(), fleet_plan()).is_err());
    }
    let old = verified_cell2();
    let checking = Record { node: "cell1".into(), instance: crate::identity::instance_name().into(), phase: "checking".into(), fresh1025: true, plan: fleet_plan(), completed: vec![old.clone()], ..Default::default() };
    save(&ops.live, &checking).unwrap();
    assert!(ops.system_start(false, "cell3".into(), fleet_plan()).is_err());
    assert!(ops.system_verify(false).await.is_err());
    let discarded = ops.system_discard_review(false).unwrap();
    assert_eq!(discarded.phase, "refused");
    assert_eq!(discarded.completed, vec![old]);
    for change in 0..6 {
        let mut bad = checking.clone();
        match change { 0 => bad.node = "cell4".into(), 1 => bad.plan[0].to.push('9'), 2 => bad.completed[0].demo = true, 3 => bad.completed[0].instance = "other".into(), 4 => bad.completed[0].phase = "installing".into(), _ => bad.previous = Some(Box::new(original_record(false))) }
        save(&ops.live, &bad).unwrap();
        assert!(ops.system_status(false).is_err());
        assert!(check_load(&ops.live, false, &["cell1".into()]).is_err());
    }
    assert!(node.ran("systemd-run").is_empty());
}
#[tokio::test]
async fn fleet_failed_install_and_lost_reply_never_repeat_or_release_reservation() {
    for failed in [false, true] {
        let (ops, node, _, _) = setup_1025(0, "cell3", true).await;
        save(&ops.live, &verified_cell2()).unwrap();
        if failed { node.bin("apt-under-test", "#!/bin/sh\necho interrupted\nexit 100\n"); }
        else { node.close_without_status("systemd-run --unit sparkbox-upgrade-"); }
        let id = ops.system_start(false, "cell3".into(), fleet_plan()).unwrap();
        let task = done(&ops, &id).await;
        assert_eq!(task.state, if failed { TaskState::Failed } else { TaskState::Ok });
        assert!(ops.system_discard_review(false).is_err());
        assert!(ops.system_start(false, "cell3".into(), fleet_plan()).is_err());
        assert_eq!(ops.system_status(false).unwrap().unwrap().completed, vec![verified_cell2()]);
        assert_eq!(node.ran("systemd-run").len(), 1);
    }
}
