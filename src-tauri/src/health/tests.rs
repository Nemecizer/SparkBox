use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use super::incident::Incident;
use super::rules::*;
use super::*;
use crate::demo::DemoCluster;
use crate::inventory::Inventory;
use crate::model::{Container, DiskStats, LinkStatus, MacReach, MacReachState, NetRole, NetStats, NodeSnapshot, SwitchPort, SwitchSnapshot};

const GIB: u64 = 1024 * 1024 * 1024;

/// A quiet four-node cluster: everything healthy, so each test adds exactly one problem.
fn healthy() -> ClusterSnapshot {
    let node = |id: &str| NodeSnapshot {
        id: id.into(),
        name: id.into(),
        host: format!("{id}.local"),
        status: LinkStatus::Online,
        mem: crate::model::MemStats { total: 128 * GIB, used: 40 * GIB, ..Default::default() },
        disks: vec![DiskStats { mount: "/".into(), device: "/dev/nvme0n1p2".into(), fs_type: "ext4".into(), total: 4000 * GIB, used: 1000 * GIB, ..Default::default() }],
        nets: vec![
            NetStats { iface: "enp1s0f1np1".into(), role: NetRole::Fabric, up: true, speed_mbps: 200_000, ip: Some(format!("192.168.100.{}", id.len())), carrier_changes: Some(2), ..Default::default() },
            NetStats { iface: "enP7s7".into(), role: NetRole::Lan, up: true, speed_mbps: 10_000, ip: Some("192.168.50.10".into()), ..Default::default() },
        ],
        containers: vec![Container { id: "abc".into(), name: "vllm-head".into(), image: "vllm".into(), state: "running".into(), ..Default::default() }],
        ..Default::default()
    };
    ClusterSnapshot {
        versions: None,
        ts_ms: 1_800_000_000_000,
        demo: true,
        nodes: vec![node("cell1"), node("cell2"), node("cell3"), node("gx10-0000")],
        switches: vec![SwitchSnapshot {
            id: "crs804".into(),
            name: "CRS804 DDQ".into(),
            status: LinkStatus::Online,
            configured: true,
            ports: vec![SwitchPort { name: "qsfp56-dd-1-1".into(), running: true, node: Some("cell1".into()), lane_rx_power_dbm: vec![-2.1, -2.3, -2.0, -2.2], ..Default::default() }],
            ..Default::default()
        }],
        scenario: None,
        scenario_phase: None,
        mac_reach: None,
    }
}

/// Runs `secs` one-second steps, letting `f` change the snapshot each time.
fn run(engine: &mut Engine, snap: &mut ClusterSnapshot, secs: u64, mut f: impl FnMut(&mut ClusterSnapshot, u64)) -> Vec<AlertEvent> {
    let mut events = Vec::new();
    for i in 0..secs {
        snap.ts_ms += 1000;
        f(snap, i);
        events.extend(engine.step(snap, snap.ts_ms));
    }
    events
}

fn firing(e: &Engine, snap: &ClusterSnapshot) -> Vec<(String, Severity)> {
    e.view(snap.ts_ms, Some(snap)).alerts.iter().filter(|a| a.state == AlertState::Firing).map(|a| (a.rule.clone(), a.severity)).collect()
}

#[test]
fn a_healthy_cluster_has_no_alerts() {
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 120, |_, _| {});
    let v = e.view(s.ts_ms, Some(&s));
    assert!(v.alerts.is_empty(), "{:?}", v.alerts);
    assert_eq!(v.nodes["cell1"].level, Level::Ok);
}

#[test]
fn offline_fires_after_30_seconds_and_resolves_when_back() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 29, |s, _| {
        s.nodes[2].status = LinkStatus::Offline;
        s.nodes[2].error = Some("SSH timeout: no reply from 192.168.50.98 in 10 s. The node may be off or restarting, or this Mac isn't on its network.".into());
    });
    assert!(ev.is_empty(), "pending, not firing, before 30 s");
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!(v.alerts[0].state, AlertState::Pending);
    assert_eq!(v.summary.pending, 1);
    assert_eq!(v.nodes["cell3"].level, Level::Ok, "pending alerts don't colour anything");

    let ev = run(&mut e, &mut s, 2, |_, _| {});
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].kind, EventKind::Raised);
    assert_eq!(ev[0].alert.severity, Severity::Crit);
    // One word: a timeout is Offline, and the page says what happened (never "cell3 error").
    assert_eq!(ev[0].alert.summary, "cell3 offline · no SSH reply in 10 s");
    assert_eq!(ev[0].alert.message, "Offline · No SSH reply in 10 s");
    assert!(ev[0].alert.detail.as_deref().unwrap().contains("192.168.50.98"), "the detail keeps the address");
    assert_eq!(e.view(s.ts_ms, Some(&s)).nodes["cell3"].level, Level::Crit);

    // Trying again keeps it raised, as "Offline · trying again"; online resolves it.
    let ev = run(&mut e, &mut s, 3, |s, _| s.nodes[2].status = LinkStatus::Connecting);
    assert!(ev.is_empty());
    assert_eq!(e.view(s.ts_ms, Some(&s)).alerts[0].message, "Offline · trying again");
    let ev = run(&mut e, &mut s, 1, |s, _| {
        s.nodes[2].status = LinkStatus::Online;
        s.nodes[2].error = None;
    });
    assert_eq!(ev[0].kind, EventKind::Resolved);
    assert!(e.view(s.ts_ms, Some(&s)).alerts.is_empty());
}

#[test]
fn a_short_blip_never_fires() {
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 10, |s, _| s.nodes[0].status = LinkStatus::Offline);
    let ev = run(&mut e, &mut s, 5, |s, _| s.nodes[0].status = LinkStatus::Online);
    assert!(ev.is_empty());
    assert!(e.view(s.ts_ms, Some(&s)).alerts.is_empty());
}

/// A maintenance window on `node`, as maintenance::stamp fills it in.
fn maint(s: &mut ClusterSnapshot, node: usize, until_ms: Option<u64>) {
    let started = s.ts_ms;
    s.nodes[node].maintenance = Some(crate::model::MaintInfo { until_ms, reason: if until_ms.is_some() { "restarting".into() } else { "shut down".into() }, started_ms: started });
}

#[test]
fn a_reboot_from_sparkbox_is_planned_and_pages_nobody() {
    // cell3 restarts: offline, its switch port down, and the model with a stage there down.
    let mut e = Engine::default();
    let mut s = healthy();
    s.switches[0].ports.push(SwitchPort { name: "qsfp56-dd-2-1".into(), running: true, node: Some("cell3".into()), ..Default::default() });
    let until = s.ts_ms + 480_000;
    maint(&mut s, 2, Some(until));
    let llama = |health: &str| EndpointInfo {
        id: "cell1-vllm-8000".into(),
        label: "llama-3.3-70b on cell1–cell3".into(),
        model: "llama-3.3-70b".into(),
        head_node: "cell1".into(),
        health: health.into(),
        nodes: vec!["cell1".into(), "cell2".into(), "cell3".into()],
        missing_nodes: if health == "down" { vec!["cell3".into()] } else { vec![] },
        ..Default::default()
    };
    let mut events = Vec::new();
    for _ in 0..120 {
        s.ts_ms += 1000;
        s.nodes[2].status = LinkStatus::Offline;
        s.nodes[2].power_state = Some("rebooting".into());
        s.switches[0].ports[1].running = false;
        events.extend(e.step_with(&s, &[llama("down")], s.ts_ms));
    }
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!((v.summary.crit, v.summary.warn), (0, 0), "counted nowhere: {:?}", v.incidents);
    assert!(v.alerts.iter().filter(|a| a.state == AlertState::Firing).all(|a| a.planned.as_deref().is_some_and(|p| p.starts_with("cell3 restarting (SparkBox, "))), "{:?}", v.alerts);
    assert!(v.alerts.iter().any(|a| a.rule == "endpoint-down") && v.alerts.iter().any(|a| a.rule == "port-down"));
    assert_eq!(v.incidents.len(), 1, "one incident: {:?}", v.incidents);
    assert_eq!(v.incidents[0].state, incident::IncidentState::Planned);
    assert_eq!(v.nodes["cell3"].level, Level::Ok, "planned work doesn't colour the node");
    assert!(!events.is_empty() && events.iter().all(|ev| ev.alert.muted(ev.ts_ms)), "every event is planned, so nothing pages: {events:?}");
    // Back and serving; the window closes after Verify: everything clears quietly.
    s.ts_ms += 1000;
    s.nodes[2].status = LinkStatus::Online;
    s.nodes[2].power_state = None;
    s.switches[0].ports[1].running = true;
    let ev = e.step_with(&s, &[llama("ready")], s.ts_ms);
    s.nodes[2].maintenance = None;
    let ev2 = run(&mut e, &mut s, 31, |_, _| {});
    assert!(ev.iter().chain(&ev2).all(|e| e.kind == EventKind::Resolved), "{ev:?}");
    assert_eq!(e.view(s.ts_ms, Some(&s)).summary.crit, 0);
}

#[test]
fn a_restart_that_runs_late_is_one_critical() {
    let mut e = Engine::default();
    let mut s = healthy();
    let started = s.ts_ms;
    maint(&mut s, 2, Some(started + 480_000));
    let ev = run(&mut e, &mut s, 480 + 115, |s, _| {
        s.nodes[2].status = LinkStatus::Offline;
        s.nodes[2].power_state = Some("rebooting".into());
    });
    assert!(ev.iter().all(|e| e.alert.planned.is_some()), "planned until the deadline and its grace: {ev:?}");
    let ev = run(&mut e, &mut s, 10, |_, _| {});
    let late: Vec<_> = ev.iter().filter(|e| e.kind == EventKind::Raised && !e.alert.muted(e.ts_ms)).collect();
    assert_eq!(late.len(), 1, "{ev:?}");
    assert_eq!((late[0].alert.rule.as_str(), late[0].alert.severity), ("node-reboot", Severity::Crit));
    let at = maintenance::local_hhmm(started);
    assert_eq!(late[0].alert.summary, format!("cell3 didn't come back after the restart SparkBox started at {at}"));
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!((v.summary.crit, v.incidents.len()), (1, 1), "{:?}", v.incidents);
}

#[test]
fn a_shutdown_from_sparkbox_is_planned_not_critical() {
    // QA's case: cell3 shut down on purpose; its port goes dark with it.
    let mut e = Engine::default();
    let mut s = healthy();
    s.switches[0].ports.push(SwitchPort { name: "qsfp56-dd-2-1".into(), running: true, node: Some("cell3".into()), ..Default::default() });
    maint(&mut s, 2, None);
    let ev = run(&mut e, &mut s, 3600, |s, _| {
        s.nodes[2].status = LinkStatus::Offline;
        s.nodes[2].power_state = Some("off".into());
        s.nodes[2].nets[0].up = false;
        s.switches[0].ports[1].running = false;
    });
    assert!(ev.iter().all(|e| e.alert.muted(e.ts_ms)), "nothing pages: {ev:?}");
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!((v.summary.crit, v.summary.warn), (0, 0));
    let off = v.alerts.iter().find(|a| a.rule == "node-offline").unwrap();
    assert_eq!(off.message, "Off · shut down from SparkBox");
    assert!(off.planned.as_deref().unwrap().starts_with("cell3 off · shut down from SparkBox ("));
}

#[test]
fn stale_numbers_warn_after_5_seconds() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 7, |s, i| {
        s.nodes[0].stale = true;
        s.nodes[0].frame_age_ms = Some(3000 + i * 1000);
    });
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].alert.rule.as_str(), ev[0].alert.severity), ("node-stale", Severity::Warn));
    assert_eq!(ev[0].alert.summary, "cell1 not updating");
}

#[test]
fn stale_data_neither_confirms_nor_clears_other_alerts() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[3].failed_units = vec!["nvidia-dgx-telemetry.service".into()];
    run(&mut e, &mut s, 2, |_, _| {});
    // The node stops reporting and its frame no longer lists the unit: the alert stays.
    let ev = run(&mut e, &mut s, 20, |s, _| {
        s.nodes[3].stale = true;
        s.nodes[3].failed_units.clear();
    });
    assert!(ev.iter().all(|e| e.kind != EventKind::Resolved), "{ev:?}");
    assert!(firing(&e, &s).contains(&("unit-failed".into(), Severity::Warn)));
}

#[test]
fn memory_warns_at_90_escalates_at_95_and_clears_below_88() {
    let mut e = Engine::default();
    let mut s = healthy();
    let set = |s: &mut ClusterSnapshot, pct: f64| s.nodes[3].mem.used = (128.0 * GIB as f64 * pct / 100.0) as u64;
    let ev = run(&mut e, &mut s, 59, |s, _| set(s, 91.0));
    assert!(ev.is_empty(), "warn needs a minute");
    let ev = run(&mut e, &mut s, 2, |s, _| set(s, 91.0));
    assert_eq!((ev[0].kind, ev[0].alert.severity), (EventKind::Raised, Severity::Warn));
    assert_eq!(ev[0].alert.summary, "gx10-0000 memory 91%");
    let ev = run(&mut e, &mut s, 1, |s, _| set(s, 96.0));
    assert_eq!((ev[0].kind, ev[0].alert.severity), (EventKind::Escalated, Severity::Crit));
    // 89% is under the warning line but above the clear line: still raised.
    let ev = run(&mut e, &mut s, 5, |s, _| set(s, 89.0));
    assert_eq!(ev[0].kind, EventKind::Eased);
    assert_eq!(firing(&e, &s), vec![("mem-high".to_string(), Severity::Warn)]);
    let ev = run(&mut e, &mut s, 1, |s, _| set(s, 87.0));
    assert_eq!(ev[0].kind, EventKind::Resolved);
}

#[test]
fn a_brief_memory_spike_to_crit_fires_fast() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 3, |s, _| s.nodes[0].mem.used = (128.0 * GIB as f64 * 0.97) as u64);
    assert_eq!(ev[0].alert.severity, Severity::Crit);
}

#[test]
fn disk_thresholds_and_the_fill_rate() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[3].disks[0].used = 3400 * GIB; // 85%
    let ev = run(&mut e, &mut s, 1, |_, _| {});
    assert_eq!(ev[0].alert.summary, "gx10-0000 disk 85%");
    assert!(ev[0].alert.message.starts_with("Root disk 85% full (600 GiB free)"), "{}", ev[0].alert.message);

    // Filling at 20 GiB a minute: with about 2 h left it must be watched for 30 minutes, and have
    // eaten a quarter of the free space in them.
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 29 * 60, |s, i| s.nodes[1].disks[0].used = 1000 * GIB + i * GIB / 3);
    assert!(!ev.iter().any(|e| e.alert.rule == "disk-eta"), "too soon to speak for two hours");
    let ev = run(&mut e, &mut s, 5 * 60, |s, i| s.nodes[1].disks[0].used = 1000 * GIB + (i + 29 * 60) * GIB / 3);
    let eta = ev.iter().find(|e| e.alert.rule == "disk-eta").expect("fill-rate warning");
    assert_eq!(eta.alert.summary, "cell2 disk full in ~2 h", "{}", eta.alert.message);
    assert!(eta.alert.message.contains("if it keeps filling at this rate"));
}

const HOUR_MS: u64 = 3_600_000;
const WINDOW_MS: u64 = 4 * HOUR_MS;
/// Every burst below starts after this much quiet, so it's judged as it would be on a real disk:
/// in the middle of its look-back, not at the start of the samples.
const QUIET_S: u64 = 2 * 3600;
const GB: f64 = 1e9;
const FREE: f64 = 2400.0 * GB;

/// Samples every 10 s for `minutes`, of a disk that has gained `gained(second)` bytes, as the
/// tracker keeps them: only the window's worth.
fn disk_samples(minutes: u64, gained: impl Fn(u64) -> f64) -> VecDeque<(u64, f64, f64)> {
    let end = minutes * 60;
    (0..=end / 10).map(|i| i * 10).filter(|t| end - t <= WINDOW_MS / 1000).map(|t| (t * 1000, 1600.0 * GB + gained(t), 4e12)).collect()
}

/// What `fill_rate` says `minutes` in, on the 4 TB disk these tests use.
fn rate_at(minutes: u64, gained: impl Fn(u64) -> f64 + Copy) -> Option<f64> {
    fill_rate(&disk_samples(minutes, gained), FREE - gained(minutes * 60), WINDOW_MS)
}

/// `size` bytes written evenly over `secs`, after the quiet.
fn burst(size: f64, secs: u64) -> impl Fn(u64) -> f64 + Copy {
    move |t| size * (t.saturating_sub(QUIET_S).min(secs) as f64 / secs as f64)
}

/// `rate_at` would warn: full inside the rule's 24 hours.
fn warns(minutes: u64, gained: impl Fn(u64) -> f64 + Copy) -> bool {
    rate_at(minutes, gained).is_some_and(|r| (FREE - gained(minutes * 60)) / r < 24.0 * HOUR_MS as f64)
}

/// Every minute from the end of the quiet to a day after: no warning.
fn never_warns(what: &str, gained: impl Fn(u64) -> f64 + Copy) {
    for minutes in 120..=(120 + 24 * 60) {
        assert!(!warns(minutes, gained), "{what}, {} min after the quiet: {:?} B/ms", minutes - 120, rate_at(minutes, gained));
    }
}

#[test]
fn fill_rate_needs_a_look_back_in_proportion_to_what_it_predicts() {
    // 100 MB/s with 1 TB free would fill it in 2 h 47 min: an hour must have seen a quarter of it go.
    let steady = |s: u64| s as f64 * 100e6;
    assert_eq!(fill_rate(&disk_samples(59, steady), 1e12, WINDOW_MS), None, "not yet a quarter in the hour");
    let rate = fill_rate(&disk_samples(61, steady), 1e12, WINDOW_MS).expect("an hour can");
    assert!((rate - 100e3).abs() < 1.0, "{rate}");
    // A runaway log with 30 GB left: 5 minutes of it are enough to speak for the next 5.
    assert!(fill_rate(&disk_samples(5, steady), 30e9, WINDOW_MS).is_some());
    // The warning line takes all 4 hours: 10 MB/s with 800 GB free is 22 h away.
    let slow = |s: u64| s as f64 * 10e6;
    assert!(fill_rate(&disk_samples(240, slow), 800e9, WINDOW_MS).is_some());
    assert_eq!(fill_rate(&disk_samples(239, slow), 800e9, WINDOW_MS), None, "not with less");
    assert_eq!(fill_rate(&disk_samples(240, slow), 900e9, WINDOW_MS), None, "and 4 hours can't speak for 25: there's no free pass for the longest look-back");
    assert_eq!(fill_rate(&VecDeque::new(), 1e12, WINDOW_MS), None);
    assert_eq!(eta_text(6.2 * 3600.0), "~6 h");
    assert_eq!(eta_text(40.0 * 60.0), "~40 min");
}

#[test]
fn a_short_look_back_must_have_seen_a_quarter_of_the_free_space_go() {
    // 1.35 GB/s would fill 2.4 TB in 30 minutes, and 5 minutes of that could speak for 30. But a
    // 270 GB copy is an eighth of what's free, and it's over.
    never_warns("a 270 GB fabric copy", burst(270.0 * GB, 200));
    // The same speed for longer has eaten a quarter of it, and isn't a copy any more.
    let long = burst(1800.0 * GB, 1333);
    assert!((121..=(120 + 22)).any(|m| warns(m, long)), "a runaway at the same speed is caught");
}

#[test]
fn one_download_followed_by_nothing_is_no_trend() {
    never_warns("the owner's morning: 31 GB in nine minutes", burst(31.0 * GB, 540));
    for (gb, minutes) in [(100.0, 29), (150.0, 44), (200.0, 58), (70.0, 42)] {
        never_warns(&format!("{gb} GB over {minutes} min"), burst(gb * GB, minutes * 60));
    }
    for (gb, secs) in [(150.0, 75), (270.0, 200), (300.0, 200), (400.0, 240)] {
        never_warns(&format!("a fabric copy of {gb} GB in {secs} s"), burst(gb * GB, secs));
    }
}

#[test]
fn a_rate_held_for_half_a_look_back_is_not_a_trend_but_two_thirds_is() {
    // 57 MB/s for 66 minutes: three 20-minute slices of the two-hour look-back, not four.
    never_warns("three slices of six", burst(226.0 * GB, 66 * 60));
    // Twice over in that look-back, a slice apart.
    let two = |t: u64| burst(150.0 * GB, 44 * 60)(t) + burst(150.0 * GB, 44 * 60)(t.saturating_sub(64 * 60));
    assert!(!warns(120 + 110, two), "two bursts can't outvote the quiet between them");
    // The same speed through a fourth slice is a trend: 2.2 TB left, 10 h away, and it's seen.
    let held = burst(1000.0 * GB, 4 * 3600);
    assert!(warns(120 + 84, held), "four slices of six");
}

#[test]
fn writing_that_goes_on_at_a_trickle_after_a_burst_is_judged_by_the_trickle() {
    for (gb, secs) in [(31.0, 540), (150.0, 44 * 60), (270.0, 200), (226.0, 66 * 60)] {
        let trickle = move |t: u64| burst(gb * GB, secs)(t) + t.saturating_sub(QUIET_S) as f64 * 1e6;
        never_warns(&format!("{gb} GB, then 1 MB/s"), trickle);
    }
}

#[test]
fn a_trend_ends_soon_after_writing_stops_or_slows_to_a_trickle() {
    let stops_at_3h = |s: u64| s.min(3 * 3600) as f64 * 40e6;
    assert!(fill_rate(&disk_samples(180, stops_at_3h), 2.0e12 - 432.0 * GB, WINDOW_MS).is_some(), "3 hours at 144 GB/h, 11 h from full");
    let ends = |gained: fn(u64) -> f64| (181..300).find(|m| fill_rate(&disk_samples(*m, gained), 1.5e12, WINDOW_MS).is_none());
    // A look-back of 2 hours watches its last 20 minutes: it's gone once 3/4 of them are quiet.
    assert_eq!(ends(stops_at_3h), Some(196), "15 minutes after it stopped");
    // A trickle isn't filling: a hundredth of the rate is below the quarter it must keep.
    let trickle: fn(u64) -> f64 = |s| s.min(3 * 3600) as f64 * 40e6 + s.saturating_sub(3 * 3600) as f64 * 0.4e6;
    assert_eq!(ends(trickle), Some(196));
    // A fifth of the rate is under the quarter it must keep, and a third is over it.
    let fifth: fn(u64) -> f64 = |s| s.min(3 * 3600) as f64 * 40e6 + s.saturating_sub(3 * 3600) as f64 * 8e6;
    assert_eq!(ends(fifth), Some(199), "the fifth takes a little longer to fall under the bar");
    let third: fn(u64) -> f64 = |s| s.min(3 * 3600) as f64 * 40e6 + s.saturating_sub(3 * 3600) as f64 * 14e6;
    assert!(fill_rate(&disk_samples(205, third), 1.5e12, WINDOW_MS).is_some(), "still a fill at a third of the rate");
}

#[test]
fn a_fill_written_in_steps_is_still_a_fill() {
    // 30 GB a minute, every 20 minutes: 90 GB/h, on 2.4 TB, and at 4 hours it's measured at 90.
    let steps = |t: u64| (t / 1200) as f64 * 30.0 * GB + 30.0 * GB * ((t % 1200).min(60) as f64 / 60.0);
    let rate = fill_rate(&disk_samples(240, steps), 2000.0 * GB, WINDOW_MS).expect("a steady fill in steps");
    assert!((rate * HOUR_MS as f64 / GB - 90.0).abs() < 1.0, "{} GB/h", rate * HOUR_MS as f64 / GB);
    // Every 40 and every 60 minutes: seen too, though it may come and go between writes.
    for period in [2400, 3600] {
        let every = move |t: u64| (t / period) as f64 * (period as f64 / 40.0) * GB;
        assert!((200..=480).any(|m| fill_rate(&disk_samples(m, every), 1000.0 * GB, WINDOW_MS).is_some()), "every {} min", period / 60);
    }
}

/// `secs` of engine at 10-second ticks: the samples are 10 s apart anyway, and hours stay cheap.
fn run_slow(engine: &mut Engine, snap: &mut ClusterSnapshot, secs: u64, mut f: impl FnMut(&mut ClusterSnapshot, u64)) -> Vec<AlertEvent> {
    let mut events = Vec::new();
    for i in 0..secs / 10 {
        snap.ts_ms += 10_000;
        f(snap, i * 10);
        events.extend(engine.step(snap, snap.ts_ms));
    }
    events
}

#[test]
fn through_the_engine_one_off_writes_never_warn_on_a_disk_with_room() {
    for (gb, secs) in [(31u64, 540), (150, 2640), (70, 2520), (270, 200), (400, 240)] {
        let mut e = Engine::default();
        let mut s = healthy();
        let base = 1600 * GIB;
        s.nodes[0].disks[0].used = base;
        let ev = run_slow(&mut e, &mut s, QUIET_S + secs + 2 * 3600, |s, t| s.nodes[0].disks[0].used = base + gb * GIB * t.saturating_sub(QUIET_S).min(secs) / secs);
        assert!(!ev.iter().any(|e| e.alert.rule == "disk-eta"), "{gb} GB: {:?}", ev.iter().filter(|e| e.alert.rule == "disk-eta").map(|e| &e.alert.summary).collect::<Vec<_>>());
    }
}

#[test]
fn through_the_engine_a_real_fill_warns_then_clears_after_writing_stops() {
    let mut e = Engine::default();
    let mut s = healthy();
    let base = 1600 * GIB;
    s.nodes[0].disks[0].used = base;
    run_slow(&mut e, &mut s, QUIET_S, |_, _| {});
    // 170 GiB an hour with 2.4 TiB free: 14 h away, so it takes the 4-hour look-back.
    let rate = |t: u64| base + t * 170 * GIB / 3600;
    let ev = run_slow(&mut e, &mut s, 4 * 3600, |s, t| s.nodes[0].disks[0].used = rate(t));
    let raised = ev.iter().find(|e| e.alert.rule == "disk-eta").expect("warns");
    assert!(raised.alert.summary.starts_with("cell1 disk full in ~"), "{}", raised.alert.summary);
    // Writing stops: gone within a slice of the look-back and the rule's clear wait.
    let stopped = s.nodes[0].disks[0].used;
    let ev = run_slow(&mut e, &mut s, 36 * 60, |s, _| s.nodes[0].disks[0].used = stopped);
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved && e.alert.rule == "disk-eta"), "clears");
    assert!(!firing(&e, &s).iter().any(|(r, _)| r == "disk-eta"));
}

#[test]
fn a_checkpoint_every_20_minutes_warns_with_the_true_estimate() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[0].disks[0].total = 1000 * GIB;
    let start = 300 * GIB;
    // 90 GiB/h, 700 GiB free: full in about 7 h 45, and the warning must say so, not ~20 h.
    let ev = run_slow(&mut e, &mut s, 4 * 3600, |s, i| s.nodes[0].disks[0].used = start + (i / 1200) * 30 * GIB + 30 * GIB * (i % 1200).min(60) / 60);
    let raised = ev.iter().find(|e| e.alert.rule == "disk-eta" && e.kind == EventKind::Raised).expect("warns");
    let hours: f64 = raised.alert.summary.trim_start_matches("cell1 disk full in ~").trim_end_matches(" h").parse().unwrap();
    assert!((4.0..=8.0).contains(&hours), "{}", raised.alert.summary);
}

/// What happened to one disk: when disk-eta rose and fell, in seconds from the end of the quiet.
#[derive(Default)]
struct DiskRun {
    raised: Vec<(u64, String)>,
    resolved: Vec<u64>,
}

/// An unchanging disk for `quiet_s`, then `run_s` of `used(second)`, through the engine at 5 s steps.
fn disk_run(total: u64, quiet_s: u64, run_s: u64, edit: impl FnOnce(&mut RuleSet), used: impl Fn(u64) -> u64) -> DiskRun {
    let mut e = Engine::default();
    let mut rules = RuleSet::defaults();
    edit(&mut rules);
    e.set_rules(Arc::new(rules));
    let mut s = healthy();
    s.nodes[0].disks[0].total = total;
    let mut r = DiskRun::default();
    for step in 1..=(quiet_s + run_s) / 5 {
        let at = (step * 5).saturating_sub(quiet_s);
        s.ts_ms += 5000;
        s.nodes[0].disks[0].used = used(at).min(total);
        note_disk_eta(&e.step(&s, s.ts_ms), at, &mut r);
    }
    r
}

fn note_disk_eta(events: &[AlertEvent], at: u64, r: &mut DiskRun) {
    for ev in events.iter().filter(|e| e.alert.rule == "disk-eta") {
        match ev.kind {
            EventKind::Raised => r.raised.push((at, ev.alert.summary.clone())),
            EventKind::Resolved => r.resolved.push(at),
            _ => {}
        }
    }
}

/// The runs below use a 1000 GiB disk with 700 GiB free.
const RUN_BASE: u64 = 300 * GIB;
const LONG_QUIET_S: u64 = 5 * 3600;

/// Quiet, then `hours` at `gib_h`, with the warning up: the engine, the snapshot and the writer.
fn raised_fill(gib_h: u64, hours: u64) -> (Engine, ClusterSnapshot, impl Fn(u64) -> u64) {
    let (mut e, mut s) = (Engine::default(), healthy());
    s.nodes[0].disks[0].total = 1000 * GIB;
    let write = move |t: u64| RUN_BASE + t.saturating_sub(LONG_QUIET_S) * gib_h * GIB / 3600;
    let mut t = 0;
    while t < LONG_QUIET_S + hours * 3600 {
        t += 5;
        s.ts_ms += 5000;
        s.nodes[0].disks[0].used = write(t);
        e.step(&s, s.ts_ms);
    }
    assert!(firing(&e, &s).iter().any(|(r, _)| r == "disk-eta"), "the warning is up before anything happens to it");
    (e, s, write)
}

/// What a restart keeps: the alerts, through `set_source`. The samples go.
fn restarted(e: &Engine, s: &ClusterSnapshot) -> Engine {
    let saved: Vec<Saved> = e.active.values().map(|a| Saved { alert: a.clone(), for_ms: a.for_ms, clear_ms: a.clear_ms }).collect();
    let mut after = Engine::default();
    after.set_source(true, saved, Vec::new(), s.ts_ms);
    after
}

#[test]
fn a_restart_during_a_real_fill_keeps_the_warning_until_the_disk_is_nearly_full() {
    for (gib_h, hours) in [(40u64, 5u64), (90, 3)] {
        let (e, mut s, write) = raised_fill(gib_h, hours);
        let mut after = restarted(&e, &s);
        let mut t = LONG_QUIET_S + hours * 3600 + 30;
        s.ts_ms += 30_000;
        let mut r = DiskRun::default();
        for _ in 0..3 * 3600 / 5 {
            t += 5;
            s.ts_ms += 5000;
            s.nodes[0].disks[0].used = write(t);
            note_disk_eta(&after.step(&s, s.ts_ms), t, &mut r);
        }
        assert_eq!(r.resolved, Vec::<u64>::new(), "{gib_h} GiB/h: the fill never stopped, so nothing was resolved");
        assert!(firing(&after, &s).iter().any(|(r, _)| r == "disk-eta"), "and it's still firing three hours on");
    }
}

#[test]
fn a_restart_after_writing_stopped_still_clears() {
    let (e, mut s, _) = raised_fill(90, 3);
    let stopped = s.nodes[0].disks[0].used;
    let mut after = restarted(&e, &s);
    let mut r = DiskRun::default();
    for step in 0..20 * 60 / 5 {
        s.ts_ms += 5000;
        s.nodes[0].disks[0].used = stopped;
        note_disk_eta(&after.step(&s, s.ts_ms), step * 5, &mut r);
    }
    assert_eq!(r.resolved.len(), 1, "a disk that isn't gaining has no warning to hold");
    assert!(r.resolved[0] <= 10 * 60, "within 3 quiet minutes and the clear wait: {:?}", r.resolved);
}

#[test]
fn a_mac_that_sleeps_through_a_real_fill_keeps_the_warning() {
    let (mut e, mut s, write) = raised_fill(40, 5);
    let mut t = LONG_QUIET_S + 5 * 3600 + 8 * 3600;
    s.ts_ms += 8 * 3_600_000;
    let mut r = DiskRun::default();
    for _ in 0..3 * 3600 / 5 {
        t += 5;
        s.ts_ms += 5000;
        s.nodes[0].disks[0].used = write(t);
        note_disk_eta(&e.step(&s, s.ts_ms), t, &mut r);
    }
    assert_eq!(r.resolved, Vec::<u64>::new(), "eight hours asleep lost the samples, not the fill");
    assert!(firing(&e, &s).iter().any(|(r, _)| r == "disk-eta"));
}

#[test]
fn a_warning_line_the_owner_raised_reaches_past_24_hours() {
    // 700 GiB free at 20 GiB/h: full in 35 h, inside a 48-hour line, watched for 10 hours.
    let raised = |rules: &mut RuleSet| {
        let d = rules.rules.iter_mut().find(|r| r.id == "disk-eta").unwrap();
        (d.warn, d.clear) = (Some(48.0), Some(54.0));
    };
    let r = disk_run(1000 * GIB, LONG_QUIET_S, 10 * 3600, raised, |s| RUN_BASE + s * 20 * GIB / 3600);
    assert_eq!(r.raised.len(), 1, "full in 35 h, warning line 48 h: {:?}", r.raised);
    // The same fill under the shipped 24-hour line says nothing.
    assert!(disk_run(1000 * GIB, LONG_QUIET_S, 10 * 3600, |_| {}, |s| RUN_BASE + s * 20 * GIB / 3600).raised.is_empty());
}

#[test]
fn a_fill_that_slows_past_24_hours_holds_until_the_30_hour_line() {
    // 30 GiB/h from 700 GiB free is 23 h out; slowing by a fifth puts it about 27 h out, in the
    // band: past the warning line, short of the clear line.
    let fast = 8 * 3600;
    let at_fast = move |s: u64| RUN_BASE + s * 30 * GIB / 3600;
    let r = disk_run(1000 * GIB, LONG_QUIET_S, fast + 6 * 3600, |_| {}, move |s| if s <= fast { at_fast(s) } else { at_fast(fast) + (s - fast) * 16 * GIB / 3600 });
    assert_eq!(r.raised.len(), 1, "{:?}", r.raised);
    assert_eq!(r.resolved, Vec::<u64>::new(), "27 h out is under the 30 h clear line");
}

#[test]
fn clearing_takes_about_35_minutes_and_longer_after_a_last_burst() {
    let (write, hours) = (5 * 3600u64, 5);
    let stops = move |s: u64| RUN_BASE + s.min(write) * 35 * GIB / 3600;
    let r = disk_run(1000 * GIB, LONG_QUIET_S, write + 2 * 3600, |_| {}, stops);
    assert_eq!(r.raised.len(), 1, "{hours} hours at 35 GiB/h: {:?}", r.raised);
    let after = (r.resolved[0] - write) / 60;
    assert!((30..=36).contains(&after), "{after} min after writing stopped");
    // A last burst keeps clearing the still-filling bar a while longer.
    let burst = move |s: u64| stops(s) + if s >= write - 60 { 40 * GIB * (s - (write - 60)).min(60) / 60 } else { 0 };
    let r = disk_run(1000 * GIB, LONG_QUIET_S, write + 3 * 3600, |_| {}, burst);
    let after = (r.resolved[0] - write) / 60;
    assert!((36..=45).contains(&after), "{after} min after writing stopped");
}

/// Help says so, in as many words, so this keeps it honest.
#[test]
fn an_hourly_writer_can_raise_and_clear_while_the_disk_is_still_filling() {
    let (period, each) = (3600u64, 90 * GIB);
    let r = disk_run(1000 * GIB, LONG_QUIET_S, 7 * 3600, |_| {}, move |i| RUN_BASE + (i / period) * each + each * (i % period).min(60) / 60);
    assert!(r.raised.len() >= 2 && !r.resolved.is_empty(), "raised {:?}, resolved {:?}", r.raised, r.resolved);
}

#[test]
fn a_real_fast_fill_is_caught_with_time_to_act() {
    for minutes in [10u64, 30, 120, 360] {
        let (t, free) = (minutes * 60, 700 * GIB);
        let r = disk_run(1000 * GIB, LONG_QUIET_S, t, |_| {}, move |s| RUN_BASE + free / t * s.min(t));
        let first = r.raised.first().unwrap_or_else(|| panic!("full in {minutes} min, and no warning"));
        assert!(first.0 < t * 3 / 4, "full in {minutes} min, warned at {} min", first.0 / 60);
    }
}

#[test]
fn saved_rules_take_the_new_look_back_and_clear_wait() {
    let mut saved = RuleSet::defaults();
    let old = saved.rules.iter_mut().find(|r| r.id == "disk-eta").unwrap();
    (old.window_s, old.clear_s) = (Some(3600), 0);
    let merged = RuleSet::merged_onto_defaults(&saved);
    let rule = merged.get("disk-eta").unwrap();
    assert_eq!((rule.window_s, rule.clear_s), (Some(14_400), 300), "the old shipped values move");
    // A choice of the owner's stays.
    let mine = saved.rules.iter_mut().find(|r| r.id == "disk-eta").unwrap();
    (mine.window_s, mine.clear_s) = (Some(7200), 60);
    let merged = RuleSet::merged_onto_defaults(&saved);
    let rule = merged.get("disk-eta").unwrap();
    assert_eq!((rule.window_s, rule.clear_s), (Some(7200), 60));
    assert_eq!(RuleSet::defaults().th("disk-eta", None).window_ms, WINDOW_MS);
}

#[test]
fn hot_gpu_warns_after_two_minutes_and_throttling_after_a_minute() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 119, |s, _| s.nodes[0].gpu.temp_c = 89.0);
    assert!(ev.is_empty());
    let ev = run(&mut e, &mut s, 2, |_, _| {});
    assert_eq!(ev[0].alert.summary, "cell1 GPU 89 °C");

    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[1].gpu.throttle = vec!["GPU idle".into()];
    assert!(run(&mut e, &mut s, 60, |_, _| {}).is_empty(), "idle isn't throttling");
    let busy = |s: &mut ClusterSnapshot| {
        let g = &mut s.nodes[1].gpu;
        (g.util_pct, g.sm_clock_mhz, g.clock_max_mhz) = (97.0, 2010, Some(3003));
        g.throttle = vec!["HW thermal slowdown".into(), "SW power cap".into()];
    };
    assert!(run(&mut e, &mut s, 60, |s, _| busy(s)).is_empty(), "a minute before it's an alert");
    let ev = run(&mut e, &mut s, 1, |s, _| busy(s));
    assert_eq!(ev[0].alert.message, "GPU throttled: HW thermal slowdown, SW power cap at 97% busy, 2.0 of 3.0 GHz");
    assert_eq!(ev[0].alert.short, "GPU throttled");
}

/// health/fixtures/gpu-throttle-cases.json: the readings and sequences the node agent and the
/// UI's throttleVerdict are checked against too.
fn gpu_from(v: &serde_json::Value) -> crate::model::GpuStats {
    let mut g: crate::model::GpuStats = serde_json::from_value(serde_json::json!({
        "utilPct": v["utilPct"], "tempC": v["tempC"], "powerW": v["powerW"], "smClockMhz": v["smClockMhz"], "procs": [],
        "clockMaxMhz": v["clockMaxMhz"], "powerLimitW": v["powerLimitW"], "pstate": v["pstate"], "throttle": v["throttle"],
    }))
    .unwrap();
    g.readable = v["readable"].as_bool().unwrap_or(true);
    g
}

#[test]
fn gpu_throttling_follows_the_shared_fixture() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/health/fixtures/gpu-throttle-cases.json");
    let f: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let th = RuleSet::defaults().th("gpu-throttle", None);
    assert_eq!((th.for_ms / 1000, th.clear_ms / 1000), (f["rule"]["forS"].as_u64().unwrap(), f["rule"]["clearS"].as_u64().unwrap()));
    assert_eq!(THROTTLE_BUSY_UTIL_PCT as f64, f["rule"]["busyUtilPct"].as_f64().unwrap());
    let words = |k: &str| f["rule"][k].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_ascii_lowercase()).collect::<Vec<_>>();
    assert_eq!(words("always"), THROTTLE_ALWAYS);
    assert_eq!(words("whenBusy"), THROTTLE_IF_BUSY);
    assert!(words("never").iter().all(|w| THROTTLE_NEVER.contains(&w.as_str())));
    for v in f["verdicts"].as_array().unwrap() {
        let got = throttle_verdict(&gpu_from(&v["gpu"])).map(|(_, text)| text);
        assert_eq!(got.as_deref(), v["text"].as_str(), "{}", v["name"]);
        assert_eq!(got.is_some(), v["throttled"].as_bool().unwrap(), "{}", v["name"]);
    }
    for case in f["cases"].as_array().unwrap() {
        let mut e = Engine::default();
        let mut s = healthy();
        let (mut raised, mut resolved, mut t) = (None, None, 0u64);
        let start = s.ts_ms;
        for seg in case["segments"].as_array().unwrap() {
            let cycle: Vec<crate::model::GpuStats> = seg["cycle"].as_array().unwrap().iter().map(gpu_from).collect();
            for i in 0..seg["secs"].as_u64().unwrap() {
                s.ts_ms = start + t * 1000;
                s.nodes[0].gpu = cycle[i as usize % cycle.len()].clone();
                for ev in e.step(&s, s.ts_ms).iter().filter(|ev| ev.alert.rule == "gpu-throttle") {
                    match ev.kind {
                        EventKind::Raised => raised = raised.or(Some(t)),
                        EventKind::Resolved => resolved = resolved.or(Some(t)),
                        _ => {}
                    }
                }
                t += 1;
            }
        }
        assert_eq!((raised, resolved), (case["raisedAtS"].as_u64(), case["resolvedAtS"].as_u64()), "{}", case["name"]);
    }
}

#[test]
fn a_fabric_link_down_is_critical_and_flapping_warns() {
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 2, |_, _| {});
    // The collector drops the address with the link; the engine remembers it.
    let ev = run(&mut e, &mut s, 4, |s, _| {
        let x = &mut s.nodes[1].nets[0];
        x.up = false;
        x.ip = None;
        x.carrier_changes = Some(3);
    });
    let rules: Vec<_> = ev.iter().map(|e| (e.alert.rule.as_str(), e.alert.severity)).collect();
    assert_eq!(rules, [("fabric-down", Severity::Crit)], "a link that's down is down; its flap counts once it's back");
    let down = ev.iter().find(|e| e.alert.rule == "fabric-down").unwrap();
    assert_eq!(down.alert.summary, "enp1s0f1np1 down on cell2");
    assert_eq!(down.alert.entity.node.as_deref(), Some("cell2"));

    // Back up: "down" resolves after 30 s of link; the drop (two carrier changes) is one flap, on
    // this interface only, and "flapping" lasts 5 minutes from it.
    let ev = run(&mut e, &mut s, 31, |s, _| {
        s.nodes[1].nets[0].up = true;
        s.nodes[1].nets[0].carrier_changes = Some(4);
    });
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved && e.alert.rule == "fabric-down"));
    assert!(firing(&e, &s).contains(&("fabric-flap".into(), Severity::Warn)));
    let flap = ev.iter().find(|e| e.alert.rule == "fabric-flap").expect("raised once it's back");
    assert_eq!((flap.alert.entity.id.as_str(), flap.alert.value), ("enp1s0f1np1", Some(1.0)));
    assert_eq!(flap.alert.message, "enp1s0f1np1 dropped and came back in the last 5 min");
    assert_eq!(e.view(s.ts_ms, Some(&s)).alerts.iter().filter(|a| a.rule == "fabric-flap").count(), 1, "the other interfaces didn't drop");
    let ev = run(&mut e, &mut s, 300, |_, _| {});
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved && e.alert.rule == "fabric-flap"));
}

#[test]
fn an_unused_port_without_an_address_is_not_an_alert() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[0].nets.push(NetStats { iface: "enp1s0f0np0".into(), role: NetRole::Fabric, up: false, ..Default::default() });
    assert!(run(&mut e, &mut s, 10, |_, _| {}).is_empty());
}

#[test]
fn rising_interface_errors_warn_until_they_stop() {
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 1, |_, _| {});
    let ev = run(&mut e, &mut s, 3, |s, i| s.nodes[2].nets[0].rx_errors = 10 * (i + 1));
    assert_eq!(ev[0].alert.rule, "net-errors");
    assert!(ev[0].alert.message.starts_with("enp1s0f1np1 errors rising (+"), "{}", ev[0].alert.message);
    let ev = run(&mut e, &mut s, 121, |_, _| {});
    assert_eq!(ev[0].kind, EventKind::Resolved);
}

#[test]
fn containers_exited_oom_and_crash_loops() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[3].containers.push(Container {
        name: "ollama".into(),
        state: "exited".into(),
        exit_code: 137,
        oom_killed: true,
        finished_at: s.ts_ms / 1000 - 120,
        ..Default::default()
    });
    s.nodes[3].containers.push(Container { name: "sglang-test".into(), state: "exited".into(), exit_code: 1, ..Default::default() });
    let long_ago = s.ts_ms / 1000 - 3 * 86_400;
    s.nodes[3].containers.push(Container { name: "old-test".into(), state: "exited".into(), exit_code: 1, finished_at: long_ago, ..Default::default() });
    s.nodes[3].containers.push(Container { name: "done-job".into(), state: "exited".into(), exit_code: 0, ..Default::default() });
    let ev = run(&mut e, &mut s, 1, |_, _| {});
    let oom = ev.iter().find(|e| e.alert.entity.id == "ollama").unwrap();
    assert_eq!(oom.alert.severity, Severity::Crit);
    assert_eq!(oom.alert.message, "ollama exited 137 · OOM-killed · 2 min ago");
    let plain = ev.iter().find(|e| e.alert.entity.id == "sglang-test").unwrap();
    assert_eq!((plain.alert.severity, plain.alert.message.as_str()), (Severity::Warn, "sglang-test exited 1"));
    assert!(ev.iter().all(|e| e.alert.entity.id != "done-job"), "a clean exit isn't a failure");
    assert!(ev.iter().all(|e| e.alert.entity.id != "old-test"), "a failure from days ago is history, not an incident");

    // Docker restarting it again and again is a crash loop.
    let ev = run(&mut e, &mut s, 40, |s, i| {
        let c = &mut s.nodes[1].containers[0];
        c.restart_count = (i / 15) as u32;
        c.state = if i % 15 < 3 { "restarting".into() } else { "running".into() };
    });
    let crit: Vec<_> = ev.iter().filter(|e| e.alert.rule == "crash-loop" && e.alert.severity == Severity::Crit).collect();
    assert!(!crit.is_empty(), "{ev:?}");
    assert_eq!(crit[0].alert.summary, "vllm-head crash-looping on cell2");
}

#[test]
fn each_failed_unit_is_its_own_warning() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[0].failed_units = vec!["nvidia-dgx-telemetry.service".into(), "fwupd-refresh.service".into()];
    let ev = run(&mut e, &mut s, 1, |_, _| {});
    assert_eq!(ev.len(), 2);
    assert!(ev.iter().any(|e| e.alert.summary == "fwupd-refresh failed on cell1"));
    assert!(ev.iter().any(|e| e.alert.summary == "nvidia-dgx-telemetry failed on cell1"));
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!(v.nodes["cell1"].level, Level::Warn);
    assert_eq!(v.nodes["cell1"].issues.len(), 2);
    assert_eq!(v.summary.warn, 2);
}

#[test]
fn switch_rules() {
    // Unreachable (only when configured).
    let mut e = Engine::default();
    let mut s = healthy();
    s.switches.push(SwitchSnapshot { id: "gold".into(), name: "Firewalla Gold".into(), configured: false, ..Default::default() });
    let ev = run(&mut e, &mut s, 11, |s, _| {
        s.switches[0].status = LinkStatus::Offline;
        s.switches[0].error = Some("Timed out after 5 s".into());
    });
    assert_eq!(ev.len(), 1, "an unconfigured gateway is 'Not set up', not an alert: {ev:?}");
    assert_eq!(ev[0].alert.summary, "CRS804 DDQ unreachable");
    assert_eq!(e.view(s.ts_ms, Some(&s)).switches["crs804"].level, Level::Crit);

    // A node's port down is critical and colours both the switch and the node.
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 4, |s, _| s.switches[0].ports[0].running = false);
    assert_eq!(ev[0].alert.summary, "qsfp56-dd-1-1 down (cell1)");
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!(v.nodes["cell1"].level, Level::Crit);
    assert_eq!(v.switches["crs804"].level, Level::Crit);

    // Weak optics, by lane.
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 6, |s, _| s.switches[0].ports[0].lane_rx_power_dbm[2] = -9.5);
    assert_eq!(ev[0].alert.severity, Severity::Warn);
    assert_eq!(ev[0].alert.message, "Lane 3 receive power -9.5 dBm (weak below -7 dBm)");
    let ev = run(&mut e, &mut s, 1, |s, _| s.switches[0].ports[0].lane_rx_power_dbm[2] = -10.4);
    assert_eq!(ev[0].kind, EventKind::Escalated);

    // The switch's own health: a failed power supply is critical, a stopped fan warns.
    let mut e = Engine::default();
    let mut s = healthy();
    s.switches[0].sensors = vec![
        crate::model::Sensor { name: "PSU2".into(), value: 0.0, unit: String::new(), ok: false },
        crate::model::Sensor { name: "Fan 1".into(), value: 0.0, unit: "RPM".into(), ok: false },
        crate::model::Sensor { name: "Switch chip temperature".into(), value: 61.0, unit: "°C".into(), ok: true },
    ];
    let ev = run(&mut e, &mut s, 31, |_, _| {});
    let got: Vec<(&str, Severity)> = ev.iter().map(|e| (e.alert.summary.as_str(), e.alert.severity)).collect();
    assert!(got.contains(&("CRS804 DDQ PSU2 failed: the switch runs on its other supply", Severity::Crit)), "{got:?}");
    assert!(got.contains(&("CRS804 DDQ fan 1 at 0 RPM: it has stopped or is failing", Severity::Warn)), "{got:?}");
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!(v.incidents.len(), 1, "one switch, one incident: {:?}", v.incidents);
    assert_eq!(v.switches["crs804"].level, Level::Crit);

    // Port errors increasing.
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 1, |_, _| {});
    let ev = run(&mut e, &mut s, 2, |s, i| s.switches[0].ports[0].fcs_errors = 5 * (i + 1));
    assert_eq!(ev[0].alert.rule, "port-errors");
}

#[test]
fn acknowledged_and_silenced_alerts_are_known_issues_that_never_count() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[0].failed_units = vec!["x.service".into()];
    s.nodes[3].disks[0].used = 3440 * GIB; // 86%
    run(&mut e, &mut s, 1, |_, _| {});
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!(v.summary.warn, 2);
    let unit = v.alerts.iter().find(|a| a.rule == "unit-failed").unwrap().clone();
    let disk = v.alerts.iter().find(|a| a.rule == "disk-full").unwrap().clone();
    // Acknowledged: no repeats, and it's known, not counted.
    e.ack(&unit.id, s.ts_ms);
    // Silenced for a week, with a reason.
    let m = silence::Matcher { rule: Some(disk.rule.clone()), entity: Some(silence::entity_key(&disk)), incident: None };
    e.add_silence(silence::make(m, "1w", "expanding disk Friday", "Root disk on gx10-0000", s.ts_ms, true).unwrap(), s.ts_ms);
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!((v.summary.warn, v.summary.crit), (0, 0), "known issues never count");
    assert_eq!(v.summary.known.len(), 2);
    let k = v.summary.known.iter().find(|k| k.why == "silenced").unwrap();
    assert_eq!((k.reason.as_deref(), k.until_ms), (Some("expanding disk Friday"), Some(s.ts_ms + 7 * 86_400_000)));
    assert_eq!(v.nodes["cell1"].level, Level::Ok);
    assert_eq!(v.nodes["gx10-0000"].level, Level::Ok);
    assert!(v.incidents.iter().all(|i| !i.counted));
    // The silence outlives a blip: it clears and comes back within the week, still silenced.
    run(&mut e, &mut s, 1, |s, _| s.nodes[3].disks[0].used = 1000 * GIB);
    let ev = run(&mut e, &mut s, 1, |s, _| s.nodes[3].disks[0].used = 3440 * GIB);
    assert!(ev.iter().filter(|x| x.alert.rule == "disk-full").all(|x| x.alert.muted(x.ts_ms)), "{ev:?}");
    // Worse than what was acknowledged: it counts again.
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 1, |s, _| s.nodes[3].disks[0].used = 3440 * GIB);
    let id = e.view(s.ts_ms, Some(&s)).alerts[0].id.clone();
    e.ack(&id, s.ts_ms);
    let ev = run(&mut e, &mut s, 1, |s, _| s.nodes[3].disks[0].used = 3900 * GIB);
    assert_eq!(ev[0].kind, EventKind::Escalated);
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!((v.summary.crit, v.summary.known.len()), (1, 0), "an escalation un-acknowledges");
}

#[test]
fn a_silence_that_ends_while_its_alert_fires_says_so() {
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 1, |s, _| s.nodes[0].failed_units = vec!["x.service".into()]);
    let a = e.view(s.ts_ms, Some(&s)).alerts[0].clone();
    let m = silence::Matcher { incident: a.incident_id.clone(), ..Default::default() };
    e.add_silence(silence::make(m, "1h", "", "x.service on cell1", s.ts_ms, true).unwrap(), s.ts_ms);
    assert_eq!(e.view(s.ts_ms, Some(&s)).summary.warn, 0);
    s.ts_ms += 3_600_000;
    let ev = run(&mut e, &mut s, 1, |_, _| {});
    assert_eq!(ev.iter().map(|e| e.kind).collect::<Vec<_>>(), vec![EventKind::SilenceEnded]);
    assert!(!ev[0].alert.muted(ev[0].ts_ms), "it pages again");
    assert_eq!(e.view(s.ts_ms, Some(&s)).summary.warn, 1);
}

#[test]
fn silences_take_only_the_listed_durations() {
    let m = || silence::Matcher { rule: Some("disk-full".into()), entity: Some("disk:gx10-0000:/".into()), ..Default::default() };
    for d in silence::DURATIONS {
        let reason = if matches!(d, "1w" | "never" | "resolved") { "expanding disk Friday" } else { "" };
        assert!(silence::make(m(), d, reason, "x", 0, false).is_ok(), "{d}");
    }
    for bad in ["30m", "2h", "1y", "", "forever", "60"] {
        assert!(silence::make(m(), bad, "why", "x", 0, false).unwrap_err().starts_with("A silence lasts 1 hour, 1 day, 1 week"), "{bad}");
    }
    assert!(silence::make(m(), "1w", "  ", "x", 0, false).unwrap_err().contains("needs a reason"));
    assert!(silence::make(m(), "never", "", "x", 0, false).is_err());
    assert!(silence::make(silence::Matcher::default(), "1h", "", "x", 0, false).is_err(), "it has to name something");
    let s = silence::make(m(), "1d", "", "x", 1_000, true).unwrap();
    assert_eq!((s.until.clone(), s.source.as_str()), (silence::Until::At { ms: 1_000 + 86_400_000 }, "demo"));
}

#[test]
fn a_demo_silence_never_affects_live() {
    let dir = std::env::temp_dir().join(format!("sb-silence-{}-{}", std::process::id(), fastrand::u32(..)));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("alert-mutes.json"), r#"{"disk-full:disk:gx10-0000:/": 99999999999999}"#).unwrap();
    let h = Health::new(Some(dir.clone()));
    assert!(!dir.join("alert-mutes.json").exists(), "the round-2 mutes are discarded");
    let mut s = healthy();
    s.nodes[3].disks[0].used = 3440 * GIB;
    s.ts_ms = crate::history::now_ms();
    h.evaluate(&s);
    let a = h.view().alerts.iter().find(|a| a.rule == "disk-full").unwrap().clone();
    let m = silence::Matcher { rule: Some(a.rule.clone()), entity: Some(silence::entity_key(&a)), incident: None };
    h.silence(m, "never", "Demo only", "Root disk on gx10-0000").unwrap();
    assert_eq!(h.view().summary.warn, 0);
    // Switch to Live: the same alert there is not silenced.
    let mut live = s.clone();
    live.demo = false;
    live.ts_ms += 1000;
    h.evaluate(&live);
    let v = h.view();
    assert!(!v.demo && v.silences.is_empty());
    assert_eq!(v.summary.warn, 1, "{:?}", v.alerts);
    assert!(h.silences().is_empty(), "the agents get no Demo silence");
    assert!(std::fs::read_to_string(dir.join("alert-silences.demo.json")).unwrap().contains("Demo only"));
    assert!(!dir.join("alert-silences.live.json").exists() || !std::fs::read_to_string(dir.join("alert-silences.live.json")).unwrap().contains("Demo only"));
    // And back to Demo: its silence is still there.
    s.ts_ms = live.ts_ms + 1000;
    h.evaluate(&s);
    assert_eq!((h.view().summary.warn, h.view().silences.len()), (0, 1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn demo_events_never_reach_the_live_log() {
    let dir = std::env::temp_dir().join(format!("sb-logs-{}-{}", std::process::id(), fastrand::u32(..)));
    let h = Health::new(Some(dir.clone()));
    let mut s = healthy();
    s.nodes[0].failed_units = vec!["x.service".into()];
    h.evaluate(&s);
    // The Demo agents' "while away" events go to the Demo log too.
    let mut away = h.history(1)[0].clone();
    away.ts_ms -= 3_600_000;
    away.away = true;
    away.demo = false; // as the simulated agent builds it
    h.import(vec![away]);
    let mut live = healthy();
    live.demo = false;
    live.ts_ms = s.ts_ms + 1000;
    live.nodes[1].failed_units = vec!["y.service".into()];
    h.evaluate(&live);
    let live_log = std::fs::read_to_string(dir.join("alerts-live.jsonl")).unwrap();
    let demo_log = std::fs::read_to_string(dir.join("alerts-demo.jsonl")).unwrap();
    assert!(live_log.lines().count() == 1 && live_log.contains("y.service") && !live_log.contains("x.service"), "{live_log}");
    assert_eq!(demo_log.lines().count(), 2, "{demo_log}");
    assert!(demo_log.lines().all(|l| l.contains("\"demo\":true")));
    assert!(h.history(10).iter().all(|e| !e.demo), "History shows the source in use");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn active_alerts_come_back_at_launch_without_being_raised_again() {
    let dir = std::env::temp_dir().join(format!("sb-active-{}-{}", std::process::id(), fastrand::u32(..)));
    let mut s = healthy();
    s.nodes[3].disks[0].used = 3440 * GIB;
    s.ts_ms = 1_800_000_000_000;
    let h = Health::new(Some(dir.clone()));
    assert_eq!(h.evaluate(&s).events.len(), 1);
    drop(h);
    // The next launch: the warning is still there and nothing is raised (or paged) again.
    let h = Health::new(Some(dir.clone()));
    let mut events = Vec::new();
    for _ in 0..90 {
        s.ts_ms += 1000;
        events.extend(h.evaluate(&s).events);
    }
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(h.view().summary.warn, 1);
    // Restored but its data never comes back within the grace: then it resolves like any other.
    drop(h);
    let h = Health::new(Some(dir.clone()));
    s.nodes[3].disks[0].used = 1000 * GIB;
    let mut events = Vec::new();
    for _ in 0..62 {
        s.ts_ms += 1000;
        events.extend(h.evaluate(&s).events);
    }
    assert_eq!(events.iter().map(|e| e.kind).collect::<Vec<_>>(), vec![EventKind::Resolved]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switching_between_demo_and_live_starts_over_quietly() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[0].failed_units = vec!["x.service".into()];
    run(&mut e, &mut s, 1, |_, _| {});
    let mut live = healthy();
    live.demo = false;
    live.ts_ms = s.ts_ms + 1000;
    assert!(e.step(&live, live.ts_ms).is_empty(), "no 'resolved' for alerts of the other data source");
    assert!(e.view(live.ts_ms, Some(&live)).alerts.is_empty());
}

#[test]
fn the_alert_log_keeps_the_last_thousand_and_survives_a_restart() {
    let dir = std::env::temp_dir().join(format!("sb-health-{}-{}", std::process::id(), fastrand::u32(..)));
    let h = Health::new(Some(dir.clone()));
    let mut s = healthy();
    for i in 0..1100u64 {
        s.ts_ms += 1000;
        // A different unit each time: the same one failing every other second would be one flapping alert.
        s.nodes[0].failed_units = if i % 2 == 0 { vec![format!("x{i}.service")] } else { vec![] };
        h.evaluate(&s);
    }
    let lines = std::fs::read_to_string(dir.join("alerts-demo.jsonl")).unwrap().lines().count();
    assert!(lines <= 2 * LOG_KEEP + 1, "{lines}");
    let again = Health::new(Some(dir.clone()));
    let events = again.history(5000);
    assert_eq!(events.len(), LOG_KEEP);
    assert!(events[0].ts_ms >= events[1].ts_ms, "newest first");
    assert!(again.view().last_alert_ms.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_evaluation_only_reports_a_new_view_when_something_changed() {
    let h = Health::new(None);
    let mut s = healthy();
    assert!(h.evaluate(&s).view.is_some(), "the first view is always new");
    s.ts_ms += 1000;
    assert!(h.evaluate(&s).view.is_none());
    s.ts_ms += 1000;
    s.nodes[0].failed_units = vec!["x.service".into()];
    let ev = h.evaluate(&s);
    assert_eq!(ev.events.len(), 1);
    assert!(ev.view.is_some());
}

#[test]
fn the_demo_baseline_shows_its_known_warnings() {
    let mut demo = DemoCluster::new(&Inventory::example());
    let mut e = Engine::default();
    let mut events = Vec::new();
    for _ in 0..90 {
        demo.advance();
        let s = demo.snapshot();
        events.extend(e.step(&s, s.ts_ms));
    }
    let s = demo.snapshot();
    let v = e.view(s.ts_ms, Some(&s));
    let rules: Vec<(&str, &str)> = v.alerts.iter().map(|a| (a.rule.as_str(), a.entity.node.as_deref().unwrap_or(""))).collect();
    assert!(rules.contains(&("unit-failed", "cell1")), "{rules:?}");
    // gx10-0000's disk warns when the world puts it on the 85% line (round 4's world reads the real 38%).
    let gx = s.nodes.iter().find(|n| n.id == "gx10-0000").and_then(root_disk).map(|d| (d.used as f64 / d.total as f64 * 100.0).round()).unwrap_or(0.0);
    assert_eq!(rules.contains(&("disk-full", "gx10-0000")), gx >= 85.0, "the disk warns exactly when it reads 85% or more ({gx}%): {rules:?}");
    assert!(v.summary.crit == 0, "the baseline has nothing critical: {rules:?}");
    assert!(v.alerts.iter().all(|a| a.severity == Severity::Warn));
}

// ---- The demo's scripted faults, played tick by tick through the engine.

/// The Demo world as the app runs it: the simulator, the fabric monitor (switch-port mapping,
/// cabling, counter rates) and the engine, one snapshot a second.
pub(crate) struct Sim {
    pub demo: DemoCluster,
    pub mon: crate::fabric::FabricMonitor,
    pub engine: Engine,
    pub t: u64,
    base: u64,
    /// Whether to judge the simulator's two models (from what its containers do).
    pub models: bool,
    pub fabric: Option<crate::fabric::FabricState>,
}

impl Sim {
    pub fn new(id: Option<&str>) -> Sim {
        let mut demo = DemoCluster::new(&Inventory::example());
        // Simulated seconds start now, so the demo's own timestamps (exit times) line up.
        let base = crate::history::now_ms();
        // The monitor has watched the everyday world for a few seconds first, as in the app: it
        // knows who's on each switch port before a scenario takes a link down.
        let mon = crate::fabric::FabricMonitor::new(None);
        for k in (1..=3).rev() {
            demo.advance();
            let mut snap = demo.snapshot();
            snap.ts_ms = base - k * 1000;
            let tables = [("crs804".to_string(), demo.switch_tables("crs804"))].into();
            let world = crate::fabric::demo_world_cabling(&snap);
            mon.assess(&mut snap, &tables, Some(&world));
        }
        demo.set_scenario(id).unwrap();
        Sim { demo, mon, engine: Engine::default(), t: 0, base, models: false, fabric: None }
    }

    /// Plays `ticks` seconds; returns each event with the tick it happened at.
    pub fn run(&mut self, ticks: u64) -> (Vec<(u64, AlertEvent)>, ClusterSnapshot) {
        let mut events = Vec::new();
        let mut snap = self.demo.snapshot();
        for _ in 0..ticks {
            self.demo.advance();
            snap = self.demo.snapshot();
            snap.ts_ms = self.base + self.t * 1000;
            let tables = [("crs804".to_string(), self.demo.switch_tables("crs804"))].into();
            let world = crate::fabric::demo_world_cabling(&snap);
            self.fabric = Some(self.mon.assess(&mut snap, &tables, Some(&world)));
            self.engine.set_rates(self.mon.rates());
            let endpoints = if self.models { demo_models(&snap) } else { Vec::new() };
            events.extend(self.engine.step_with(&snap, &endpoints, snap.ts_ms).into_iter().map(|ev| (self.t, ev)));
            self.t += 1;
        }
        (events, snap)
    }

    pub fn view(&self, snap: &ClusterSnapshot) -> HealthView {
        self.engine.view(snap.ts_ms, Some(snap))
    }
}

/// The simulator's two models as the endpoint list would judge them: Llama across the three Ray
/// nodes (down when a stage is offline, cut off from the fabric or not running), Ollama on the
/// last node (down when its container isn't running or the GPU is gone).
fn demo_models(s: &ClusterSnapshot) -> Vec<EndpointInfo> {
    let ray: Vec<&NodeSnapshot> = s.nodes.iter().take(3).collect();
    let up = |n: &NodeSnapshot| reporting(n) && n.nets.iter().filter(|x| x.role == NetRole::Fabric && x.ip.is_some()).all(|x| x.up) && n.containers.iter().any(|c| c.name.starts_with("vllm") && c.state == "running");
    let missing: Vec<String> = ray.iter().filter(|n| !up(n)).map(|n| n.id.clone()).collect();
    let llama = EndpointInfo {
        id: "cell1-vllm-8000".into(),
        label: "llama-3.3-70b on cell1–cell3".into(),
        model: "llama-3.3-70b".into(),
        head_node: "cell1".into(),
        health: if missing.is_empty() { "ready" } else { "down" }.into(),
        reason: missing.first().map(|n| format!("Pipeline stage on {n} is offline")),
        nodes: ray.iter().map(|n| n.id.clone()).collect(),
        containers: ray.iter().flat_map(|n| n.containers.iter().filter(|c| c.name.starts_with("vllm")).map(|c| c.name.clone())).collect(),
        missing_nodes: missing,
        ..Default::default()
    };
    let gx = &s.nodes[3];
    let ollama_up = reporting(gx) && gpu_error(gx).is_none() && gx.containers.iter().any(|c| c.name == "ollama" && c.state == "running");
    let ollama = EndpointInfo {
        id: "gx10-0000-ollama-11434".into(),
        label: "Ollama on gx10-0000".into(),
        model: "Ollama".into(),
        head_node: gx.id.clone(),
        health: if ollama_up { "ready" } else { "down" }.into(),
        nodes: vec![gx.id.clone()],
        containers: vec!["ollama".into()],
        ..Default::default()
    };
    vec![llama, ollama]
}

/// Plays a scenario for `ticks` seconds of simulated time; returns every event and the engine.
fn play(id: &str, ticks: u64) -> (Engine, Vec<(u64, AlertEvent)>, ClusterSnapshot) {
    let mut sim = Sim::new(Some(id));
    let (events, snap) = sim.run(ticks);
    (sim.engine, events, snap)
}

fn find<'a>(events: &'a [(u64, AlertEvent)], kind: EventKind, rule: &str, node: &str) -> Option<&'a (u64, AlertEvent)> {
    events.iter().find(|(_, e)| e.kind == kind && e.alert.rule == rule && e.alert.entity.node.as_deref() == Some(node))
}

#[test]
fn scenario_node_offline_raises_and_clears() {
    let (_, ev, _) = play("node-offline", 120);
    let (at, raised) = find(&ev, EventKind::Raised, "node-offline", "cell3").expect("cell3 offline fires");
    assert_eq!(raised.alert.severity, Severity::Crit);
    assert!((30..40).contains(at), "fires once offline for 30 s (tick {at})");
    assert!(raised.alert.message.starts_with("Offline · "), "{}", raised.alert.message);
    let (cleared, _) = find(&ev, EventKind::Resolved, "node-offline", "cell3").expect("and clears when it's back");
    assert!(cleared > at);
}

#[test]
fn scenario_disk_filling_warns_escalates_and_predicts() {
    let (_, ev, _) = play("disk-filling", 700);
    let warn = find(&ev, EventKind::Raised, "disk-full", "gx10-0000").map(|(_, e)| e.alert.severity);
    let crit = find(&ev, EventKind::Escalated, "disk-full", "gx10-0000").map(|(_, e)| e.alert.severity);
    assert!(warn.is_some() && (warn == Some(Severity::Crit) || crit == Some(Severity::Crit)), "{warn:?} {crit:?}");
    let eta = find(&ev, EventKind::Raised, "disk-eta", "gx10-0000").expect("a fill-rate warning before it's full");
    assert!(eta.1.alert.summary.starts_with("gx10-0000 disk full in ~"), "{}", eta.1.alert.summary);
    assert!(find(&ev, EventKind::Resolved, "disk-full", "gx10-0000").is_some() || find(&ev, EventKind::Eased, "disk-full", "gx10-0000").is_some(), "cleanup brings it back down");
}

#[test]
fn scenario_thermal_throttle_warns() {
    let mut sim = Sim::new(Some("thermal-throttle"));
    let (ev, _) = sim.run(180);
    let (_, t) = find(&ev, EventKind::Raised, "gpu-throttle", "cell1").expect("throttling warns");
    assert!(t.alert.message.contains("HW thermal slowdown"), "{}", t.alert.message);
    // The scenario heats it again every 3 minutes; once it stops, the GPU cools and it clears
    // after its 2-minute clear time.
    sim.demo.set_scenario(None).unwrap();
    let (ev, _) = sim.run(300);
    assert!(find(&ev, EventKind::Resolved, "gpu-throttle", "cell1").is_some(), "and clears when it cools");
}

#[test]
fn scenario_oom_kill_is_critical() {
    let (_, ev, _) = play("oom-kill", 70);
    assert!(find(&ev, EventKind::Raised, "mem-high", "gx10-0000").is_none(), "GPU reservation alone isn't pressure");
    let (_, kill) = find(&ev, EventKind::Raised, "kernel-oom", "gx10-0000").expect("the kernel kill is still reported immediately");
    assert_eq!(kill.alert.severity, Severity::Crit);
    let ollama = |kind: EventKind| ev.iter().find(|(_, e)| e.kind == kind && e.alert.rule == "container-exited" && e.alert.entity.id == "ollama");
    let (_, oom) = ollama(EventKind::Raised).expect("the OOM kill is an alert");
    assert_eq!(oom.alert.severity, Severity::Crit);
    assert!(oom.alert.message.starts_with("ollama exited 137 · OOM-killed"), "{}", oom.alert.message);
    assert!(ollama(EventKind::Resolved).is_some(), "Docker restarts it");
}

#[test]
fn scenario_crash_loop_is_critical() {
    let (_, ev, _) = play("crash-loop", 60);
    let (_, c) = ev.iter().find(|(_, e)| e.alert.rule == "crash-loop" && e.alert.severity == Severity::Crit).expect("a crash loop");
    assert_eq!(c.alert.entity.node.as_deref(), Some("cell2"));
    assert_eq!(c.alert.entity.id, "vllm-worker");
}

#[test]
fn scenario_fabric_flap_shows_down_flapping_and_errors() {
    let (_, ev, _) = play("fabric-flap", 60);
    assert!(find(&ev, EventKind::Raised, "fabric-down", "cell2").is_some(), "the link going down is critical");
    assert!(find(&ev, EventKind::Raised, "fabric-flap", "cell2").is_some());
    assert!(find(&ev, EventKind::Raised, "net-errors", "cell2").is_some());
    assert!(ev.iter().any(|(_, e)| e.kind == EventKind::Raised && e.alert.rule == "port-errors"), "the switch sees FCS errors");
}

#[test]
fn scenario_collector_stall_goes_stale_and_recovers() {
    let (_, ev, _) = play("collector-stall", 60);
    assert!(find(&ev, EventKind::Raised, "node-stale", "cell1").is_some());
    assert!(find(&ev, EventKind::Resolved, "node-stale", "cell1").is_some());
}

#[test]
fn scenario_optic_degraded_warns_on_the_port() {
    let (e, ev, s) = play("optic-degraded", 60);
    let (_, o) = ev.iter().find(|(_, e)| e.kind == EventKind::Raised && e.alert.rule == "optic-rx-low").expect("weak lane");
    assert_eq!(o.alert.entity.node.as_deref(), Some("cell2"));
    assert!(o.alert.message.starts_with("Lane 3 receive power"), "{}", o.alert.message);
    assert_ne!(e.view(s.ts_ms, Some(&s)).switches["crs804"].level, Level::Ok);
}

#[test]
fn scenario_switch_port_down_cuts_off_cell3() {
    let (e, ev, s) = play("switch-port-down", 30);
    let (_, p) = ev.iter().find(|(_, e)| e.kind == EventKind::Raised && e.alert.rule == "port-down").expect("port down");
    assert_eq!(p.alert.entity.id, "qsfp56-dd-3-1");
    assert_eq!(e.view(s.ts_ms, Some(&s)).nodes["cell3"].level, Level::Crit);
}

/// The RoCE alerts in a set of events (raised, still active).
fn roce_active(v: &HealthView) -> Vec<&Alert> {
    v.alerts.iter().filter(|a| a.rule == "roce-iface" && a.state == AlertState::Firing).collect()
}

#[test]
fn roce_alerts_clear_after_roce_loss_ends() {
    // QA's repro: roce-loss, wait 60 s, back to the everyday world, then read health.
    let mut sim = Sim::new(Some("roce-loss"));
    let (ev, snap) = sim.run(60);
    let raised: Vec<&AlertEvent> = ev.iter().map(|(_, e)| e).filter(|e| e.kind == EventKind::Raised && e.alert.rule == "roce-iface").collect();
    assert_eq!(raised.len(), 1, "one RoCE alert, on the one interface that loses packets: {raised:?}");
    let a = &raised[0].alert;
    assert_eq!((a.entity.node.as_deref(), a.entity.label.as_str()), (Some("cell2"), "cell2 half 2"));
    // Critical once CRC errors pass 10/s: at once, or by escalation when the simulator's first
    // reading lands just under the line (it does under a loaded full-crate run).
    let crit = a.severity == Severity::Crit || ev.iter().any(|(_, e)| e.kind == EventKind::Escalated && e.alert.rule == "roce-iface" && e.alert.severity == Severity::Crit);
    assert!(crit, "{ev:?}");
    // CRC errors count at once; the retransmits join once they've lasted a minute.
    assert_eq!(a.message, "CRC and symbol errors rising · check the optic or cable");
    let v = sim.view(&snap);
    let now = roce_active(&v);
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].summary, "cell2 half 2: RoCE retransmitting · CRC and symbol errors rising · check the optic or cable");
    assert!(now[0].detail.as_deref().unwrap().starts_with("Out of sequence "), "the evidence: {:?}", now[0].detail);
    sim.demo.set_scenario(None).unwrap();
    let ended = sim.t;
    let clear_s = RuleSet::defaults().th("roce-iface", None).clear_ms / 1000;
    let (ev, snap) = sim.run(clear_s + 30);
    let resolved: Vec<&(u64, AlertEvent)> = ev.iter().filter(|(_, e)| e.kind == EventKind::Resolved && e.alert.rule == "roce-iface").collect();
    assert_eq!(resolved.len(), 1, "{ev:?}");
    assert!(resolved[0].0 <= ended + clear_s + 30, "resolved {} s after the end", resolved[0].0 - ended);
    let v = sim.view(&snap);
    assert!(roce_active(&v).is_empty(), "{:?}", roce_active(&v));
    assert!(ev.iter().all(|(_, e)| e.kind != EventKind::Raised), "nothing raised after the end: {ev:?}");
    // Its switch port's errors were part of the same incident and are gone too.
    assert!(v.alerts.iter().all(|a| a.entity.node.as_deref() != Some("cell2")), "{:?}", v.alerts);
}

#[test]
fn roce_loss_is_one_incident() {
    let mut sim = Sim::new(Some("roce-loss"));
    let (_, snap) = sim.run(75);
    let v = sim.view(&snap);
    let counted: Vec<&Incident> = v.incidents.iter().filter(|i| i.counted && i.id.starts_with("link:")).collect();
    assert_eq!(counted.len(), 1, "{:?}", v.incidents);
    let i = counted[0];
    assert!(i.id.starts_with("link:cell2/"), "{}", i.id);
    assert_eq!(i.title, "cell2 half 2: RoCE retransmitting · CRC and symbol errors rising · check the optic or cable");
    assert_eq!((i.severity, i.affects.as_str(), i.cause.as_deref()), (Severity::Crit, "cell2 half 2", Some("check the optic or cable")));
    let kids: Vec<&str> = i.children.iter().map(|id| v.alerts.iter().find(|a| &a.id == id).unwrap().rule.as_str()).collect();
    assert_eq!(kids[0], "roce-iface");
    assert!(kids.contains(&"port-errors"), "the CRS804 port's errors are a child: {kids:?}");
    // The top bar's count: one critical, and the baseline's warnings (cell1's failed unit…), none about cell2's fabric.
    assert_eq!(v.summary.crit, 1, "{:?}", v.incidents.iter().map(|i| (&i.id, i.severity, i.state)).collect::<Vec<_>>());
    assert!(v.incidents.iter().filter(|i| i.counted && i.severity == Severity::Warn).all(|i| !i.id.contains("cell2/en")), "{:?}", v.incidents);
    assert!(v.summary.alerts >= 2);
}

#[test]
fn node_offline_is_one_incident_with_its_children() {
    let mut sim = Sim::new(Some("node-offline"));
    sim.models = true;
    let (_, snap) = sim.run(50);
    let v = sim.view(&snap);
    let crit: Vec<&Incident> = v.incidents.iter().filter(|i| i.counted && i.severity == Severity::Crit).collect();
    assert_eq!(crit.len(), 1, "{:?}", v.incidents);
    let i = crit[0];
    assert_eq!((i.id.as_str(), i.title.as_str(), i.affects.as_str()), ("node:cell3", "cell3 offline · no SSH reply in 10 s", "cell3"));
    let kids: Vec<&str> = i.children.iter().map(|id| v.alerts.iter().find(|a| &a.id == id).unwrap().rule.as_str()).collect();
    assert_eq!(kids[0], "node-offline");
    assert!(kids.contains(&"endpoint-down"), "the model down because of it is a child: {kids:?}");
    assert_eq!(v.summary.crit, 1, "one critical incident, not one per alert");
    assert_ne!(v.nodes["cell2"].level, Level::Crit, "a model's outage doesn't colour its other stages");
    assert_ne!(v.nodes["cell1"].level, Level::Crit);
    // Back: no critical within one poll of Ready.
    let (ev, snap) = sim.run(30);
    let back = ev.iter().find(|(_, e)| e.kind == EventKind::Resolved && e.alert.rule == "node-offline").map(|(t, _)| *t).expect("it comes back");
    let model = ev.iter().find(|(_, e)| e.kind == EventKind::Resolved && e.alert.rule == "endpoint-down").map(|(t, _)| *t).expect("and the model answers");
    assert!(model <= back + 1, "the model clears on its first Ready ({model} vs back at {back})");
    assert_eq!(sim.view(&snap).summary.crit, 0);
}

/// Seen in Live on Oct 29: cell3 reconnected at 6:09:23 PM and its Ollama's "Down · cell3 is
/// offline" cleared two seconds later as an incident of its own, so it would have sent its own
/// resolve. Clearing after its node, a child keeps the node's incident: one resolve.
#[test]
fn a_child_that_clears_after_its_node_stays_in_the_nodes_incident() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ollama = |health: &str, reason: Option<&str>| vec![EndpointInfo { id: "cell3-ollama-11434".into(), label: "Ollama on cell3".into(), model: "gemma4:latest".into(), head_node: "cell3".into(), health: health.into(), reason: reason.map(String::from), nodes: vec!["cell3".into()], ..Default::default() }];
    let mut events = Vec::new();
    let mut step = |e: &mut Engine, s: &mut ClusterSnapshot, secs: u64, offline: bool, eps: &[EndpointInfo]| {
        for _ in 0..secs {
            s.ts_ms += 1000;
            s.nodes[2].status = if offline { LinkStatus::Error } else { LinkStatus::Online };
            s.nodes[2].error = offline.then(|| "Reconnecting".to_string());
            if !offline {
                s.nodes[2].last_seen_ms = Some(s.ts_ms);
            }
            events.extend(e.step_with(s, eps, s.ts_ms));
        }
    };
    step(&mut e, &mut s, 5, false, &ollama("idle", None));
    step(&mut e, &mut s, 60, true, &ollama("down", Some("cell3 is offline")));
    // Back: the node first, its Ollama found again a few seconds later.
    step(&mut e, &mut s, 3, false, &ollama("down", Some("cell3 is offline")));
    step(&mut e, &mut s, 120, false, &ollama("idle", None));
    let of = |rule: &str, kind: EventKind| events.iter().filter(|x| x.alert.rule == rule && x.kind == kind).map(|x| x.incident_id.clone().unwrap_or_default()).collect::<Vec<_>>();
    assert_eq!(of("endpoint-down", EventKind::Raised), ["node:cell3"], "{events:?}");
    assert_eq!(of("node-offline", EventKind::Resolved), ["node:cell3"]);
    assert_eq!(of("endpoint-down", EventKind::Resolved), ["node:cell3"], "its clear goes with cell3's");
}

#[test]
fn recovery_never_flaps_between_critical_and_warning() {
    // Memory hovering around the critical line: it eases only after its clear time below it.
    let mut rules = RuleSet::defaults();
    let mem = rules.rules.iter_mut().find(|r| r.id == "mem-high").unwrap();
    mem.clear_s = 60;
    let mut e = engine_with(rules);
    let mut s = healthy();
    let set = |s: &mut ClusterSnapshot, pct: f64| s.nodes[3].mem.used = (128.0 * GIB as f64 * pct / 100.0) as u64;
    run(&mut e, &mut s, 70, |s, _| set(s, 96.0));
    let ev = run(&mut e, &mut s, 120, |s, i| set(s, if i % 20 < 10 { 93.0 } else { 96.0 }));
    assert!(ev.iter().all(|e| e.kind != EventKind::Eased), "no critical → warning → critical: {ev:?}");
    let ev = run(&mut e, &mut s, 61, |s, _| set(s, 93.0));
    assert_eq!(ev.iter().map(|e| e.kind).collect::<Vec<_>>(), vec![EventKind::Eased]);
}

#[test]
fn a_condition_that_stops_is_recovering_and_not_counted() {
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 121, |s, _| s.nodes[0].gpu.temp_c = 89.0);
    assert_eq!(e.view(s.ts_ms, Some(&s)).summary.warn, 1);
    run(&mut e, &mut s, 5, |s, _| s.nodes[0].gpu.temp_c = 70.0);
    let v = e.view(s.ts_ms, Some(&s));
    let a = &v.alerts[0];
    assert_eq!((a.state, a.counted), (AlertState::Firing, false));
    assert_eq!(a.recovers_at_ms.unwrap() - s.ts_ms, 26_000, "clears 30 s after it went back under 82 °C");
    assert_eq!((v.summary.warn, v.summary.recovering), (0, 1));
    assert_eq!(v.incidents[0].state, incident::IncidentState::Recovering);
    assert_eq!(v.nodes["cell1"].level, Level::Ok);
}

#[test]
fn fabric_partition_is_one_incident_and_the_node_stays_online() {
    let mut sim = Sim::new(Some("fabric-partition"));
    sim.models = true;
    let (ev, snap) = sim.run(45);
    let v = sim.view(&snap);
    assert_eq!(snap.nodes[1].status, LinkStatus::Online, "the LAN still answers");
    assert!(ev.iter().all(|(_, e)| e.alert.rule != "node-offline"), "no node-unreachable: {ev:?}");
    let crit: Vec<&Incident> = v.incidents.iter().filter(|i| i.counted && i.severity == Severity::Crit).collect();
    assert_eq!(crit.len(), 1, "{:?}", v.incidents.iter().map(|i| (&i.id, &i.title)).collect::<Vec<_>>());
    assert_eq!((crit[0].id.as_str(), crit[0].title.as_str()), ("node:cell2", "cell2 lost the fabric"));
    let kids: Vec<&str> = crit[0].children.iter().map(|id| v.alerts.iter().find(|a| &a.id == id).unwrap().rule.as_str()).collect();
    assert_eq!(kids.iter().filter(|r| **r == "fabric-down").count(), 2, "both halves: {kids:?}");
    assert!(kids.contains(&"port-down") && kids.contains(&"endpoint-down"), "{kids:?}");
}

#[test]
fn gpu_xid_stays_latched_after_an_hour_of_demo_time() {
    let mut sim = Sim::new(Some("gpu-xid"));
    sim.models = true;
    let (_, snap) = sim.run(3_700);
    let v = sim.view(&snap);
    let xid = v.alerts.iter().find(|a| a.rule == "kernel-xid").expect("still firing an hour later");
    assert_eq!((xid.state, xid.severity, xid.counted), (AlertState::Firing, Severity::Crit, true));
    assert!(v.alerts.iter().any(|a| a.rule == "gpu-unreadable"), "the GPU's readings are gone too");
    assert_eq!(snap.nodes[3].error.as_deref(), Some("GPU has fallen off the bus (Xid 79)"));
    let gpu: Vec<&Incident> = v.incidents.iter().filter(|i| i.counted && i.severity == Severity::Crit).collect();
    assert_eq!(gpu.len(), 1, "the Xid, the unreadable GPU and Ollama down are one incident: {:?}", v.incidents);
    assert_eq!(gpu[0].id, "gpu:gx10-0000");
    // The owner restarts gx10-0000: a new boot clears it.
    sim.demo.power("gx10-0000", "reboot").unwrap();
    let (ev, snap) = sim.run(60);
    assert!(ev.iter().any(|(_, e)| e.kind == EventKind::Resolved && e.alert.rule == "kernel-xid"), "{ev:?}");
    assert!(sim.view(&snap).alerts.iter().all(|a| a.rule != "kernel-xid" && a.rule != "gpu-unreadable"));
}

#[test]
fn scenario_healthy_raises_nothing() {
    // Not even gx10-0000's disk, which sits on the 85% line in the everyday world.
    let (e, ev, s) = play("healthy", 120);
    let rules: Vec<_> = ev.iter().map(|(_, e)| (e.alert.rule.as_str(), e.alert.entity.node.as_deref())).collect();
    assert!(rules.is_empty(), "{rules:?}");
    let summary = e.view(s.ts_ms, Some(&s)).summary;
    assert_eq!((summary.crit, summary.warn), (0, 0));
}

#[test]
fn an_inference_server_that_stops_answering_is_critical() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ep = |health: &str| vec![EndpointInfo { id: "cell1-vllm-8000".into(), label: "llama-3.3-70b on cell1–cell3".into(), model: "llama-3.3-70b".into(), head_node: "cell1".into(), health: health.into(), ..Default::default() }];
    let mut events = Vec::new();
    for _ in 0..31 {
        s.ts_ms += 1000;
        events.extend(e.step_with(&s, &ep("down"), s.ts_ms));
    }
    let raised = events.iter().find(|ev| ev.alert.rule == "endpoint-down").expect("down for 30 s");
    assert_eq!(raised.alert.severity, Severity::Crit);
    assert_eq!(raised.alert.summary, "llama-3.3-70b is down: it isn't answering requests");
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!(v.nodes["cell1"].level, Level::Ok, "a model's outage doesn't paint its healthy head node");
    assert_eq!(v.summary.crit, 1);
    // The first Ready clears it: the verdict is evidence (an answer), so there's no hold.
    s.ts_ms += 1000;
    let events = e.step_with(&s, &ep("ready"), s.ts_ms);
    assert!(events.iter().any(|ev| ev.kind == EventKind::Resolved && ev.alert.rule == "endpoint-down"), "{events:?}");
    assert_eq!(e.view(s.ts_ms, Some(&s)).summary.crit, 0);
}

// ---- Rules as data

use super::rule::{Op, Override, Rule, RuleSet};

#[test]
fn the_default_rules_fixture_is_current() {
    // agent_test.py evaluates this file; it must be exactly RuleSet::defaults().
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/health/fixtures/default-rules.json");
    let now = serde_json::to_value(RuleSet::defaults()).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default()).unwrap_or_default();
    if saved != now {
        std::fs::write(&path, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
        panic!("{} was out of date and has been rewritten; run the tests again", path.display());
    }
}

#[test]
fn the_defaults_carry_the_shipped_numbers() {
    let r = RuleSet::defaults();
    let th = |id: &str| r.th(id, None);
    let mem = th("mem-high");
    assert_eq!((mem.warn, mem.crit, mem.clear, mem.for_ms, mem.crit_for_ms), (Some(90.0), Some(95.0), Some(88.0), 60_000, 2_000));
    assert_eq!((th("disk-full").warn, th("disk-full").crit, th("disk-full").clear), (Some(85.0), Some(95.0), Some(83.0)));
    assert_eq!((th("gpu-hot").warn, th("gpu-hot").for_ms, th("gpu-hot").clear_ms), (Some(85.0), 120_000, 30_000));
    assert_eq!((th("node-offline").for_ms, th("node-reboot").for_ms, th("node-stale").for_ms), (30_000, 120_000, 5_000));
    assert_eq!((th("optic-rx-low").warn, th("optic-rx-low").crit, th("optic-rx-low").op), (Some(-7.0), Some(-10.0), Op::Lt));
    assert_eq!((th("ntp-offset").warn, th("ntp-offset").crit, th("ntp-offset").op), (Some(100.0), Some(1000.0), Op::AbsGt));
    assert_eq!((th("endpoint-ttft").warn, th("endpoint-ttft").for_ms), (Some(2000.0), 300_000));
    assert_eq!((th("endpoint-down").for_ms, th("endpoint-down").clear_ms), (30_000, 0), "a model clears on its first Ready");
    assert_eq!((th("crash-loop").crit, th("crash-loop").clear_ms, th("crash-loop").window_ms), (Some(1.0), 300_000, CRASH_WINDOW_S * 1000));
    assert_eq!((th("gpu-throttle").for_ms, th("gpu-throttle").clear_ms), (60_000, 120_000));
    assert_eq!((CRASH_RESTARTS, CRASH_WINDOW_S, CRASH_RECENT_S), (2, 600, 60));
    assert_eq!((th("gpu-unreadable").for_ms, th("node-reboot").for_ms), (30_000, 120_000));
    assert!(!r.get("endpoint-idle").unwrap().notify);
    assert_eq!(super::rule::MEM_WARN_PCT, 90.0);
    let mut ids: Vec<&str> = r.rules.iter().map(|x| x.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), r.rules.len(), "ids are unique");
}

#[test]
fn the_parity_fixture_runs_the_same_in_the_app() {
    // agent_test.py runs the same file through the node agent's engine.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/health/fixtures/parity.json");
    let fixture: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let (rule, metric) = (case["rule"].as_str().unwrap(), case["metric"].as_str().unwrap());
        let mut e = Engine::default();
        let mut s = healthy();
        let mon = crate::fabric::FabricMonitor::new(None);
        let mut counters: BTreeMap<String, u64> = BTreeMap::new();
        let mut got: Vec<(u64, String, String)> = Vec::new();
        let mut step = 0u64;
        for seg in case["segments"].as_array().unwrap() {
            for _ in 0..seg["secs"].as_u64().unwrap() {
                step += 1;
                s.ts_ms += 1000;
                let n = &mut s.nodes[0];
                if metric == "roce" {
                    for (k, v) in seg["perS"].as_object().unwrap() {
                        *counters.entry(k.clone()).or_default() += v.as_u64().unwrap();
                    }
                    let mut c = serde_json::Map::new();
                    for key in super::roce_rules::table().counters.iter().map(|c| c.key.clone()) {
                        // The pause counters are sums of per-priority ones; the fixture sets the totals.
                        let field = match key.as_str() {
                            "txPause" => "txPauseCtrl".to_string(),
                            "rxPause" => "rxPauseCtrl".to_string(),
                            _ => key.clone(),
                        };
                        c.insert(field, counters.get(&key).copied().unwrap_or(0).into());
                    }
                    n.nets[0].roce = Some(serde_json::from_value(serde_json::Value::Object(c)).unwrap());
                    mon.assess(&mut s, &Default::default(), None);
                    e.set_rates(mon.rates());
                } else if metric == "macReach" {
                    // This Mac's verdict, as reach.rs stamps it.
                    s.mac_reach = Some(MacReach { state: serde_json::from_value(seg["state"].clone()).unwrap(), title: "This Mac can't reach any node; the LAN between them?".into(), ..Default::default() });
                } else if metric == "errorKind" {
                    // The first node's last connection failure (null: online).
                    let kind = seg["state"].as_str();
                    n.status = if kind.is_some() { LinkStatus::Error } else { LinkStatus::Online };
                    n.error_kind = kind.map(String::from);
                    n.error = kind.map(|k| format!("{k} on {}", n.name));
                } else {
                    let v = seg["value"].as_f64().unwrap();
                    match metric {
                        "memPct" => n.mem.used = (n.mem.total as f64 * v / 100.0) as u64,
                        "diskPct" => n.disks[0].used = (n.disks[0].total as f64 * v / 100.0) as u64,
                        "gpuTemp" => n.gpu.temp_c = v as f32,
                        "clockOffsetMs" => n.system.clock_offset_ms = Some(v as f32),
                        other => panic!("no setter for {other}"),
                    }
                }
                for ev in e.step(&s, s.ts_ms).into_iter().filter(|ev| ev.alert.rule == rule) {
                    let kind = serde_json::to_value(ev.kind).unwrap().as_str().unwrap().to_string();
                    let sev = serde_json::to_value(ev.alert.severity).unwrap().as_str().unwrap().to_string();
                    got.push((step, kind, sev));
                }
            }
        }
        let want: Vec<(u64, String, String)> =
            case["expect"].as_array().unwrap().iter().map(|x| (x[0].as_u64().unwrap(), x[1].as_str().unwrap().into(), x[2].as_str().unwrap().into())).collect();
        assert_eq!(got, want, "{}", case["name"]);
    }
}

fn engine_with(rules: RuleSet) -> Engine {
    let mut e = Engine::default();
    e.set_rules(std::sync::Arc::new(rules));
    e
}

#[test]
fn edited_lines_overrides_and_switches_apply() {
    let mut rules = RuleSet::defaults();
    let mem = rules.rules.iter_mut().find(|r| r.id == "mem-high").unwrap();
    mem.warn = Some(70.0);
    mem.for_s = 5;
    mem.overrides.insert("gx10-0000".into(), Override { warn: Some(60.0), crit: None, enabled: None });
    mem.overrides.insert("cell2".into(), Override { enabled: Some(false), ..Default::default() });
    let mut e = engine_with(rules);
    let mut s = healthy();
    let set = |s: &mut ClusterSnapshot, i: usize, pct: f64| s.nodes[i].mem.used = (128.0 * GIB as f64 * pct / 100.0) as u64;
    let ev = run(&mut e, &mut s, 6, |s, _| {
        set(s, 0, 75.0); // cell1: over the edited line
        set(s, 1, 99.0); // cell2: switched off for this node
        set(s, 3, 65.0); // gx10-0000: over its own override
    });
    let raised: Vec<_> = ev.iter().map(|e| (e.alert.entity.id.as_str(), e.alert.severity)).collect();
    assert!(raised.contains(&("cell1", Severity::Warn)) && raised.contains(&("gx10-0000", Severity::Warn)), "{raised:?}");
    assert!(!raised.iter().any(|(n, _)| *n == "cell2"), "{raised:?}");
}

#[test]
fn a_bad_candidate_is_refused_and_a_bad_file_is_set_aside() {
    let mut rules = RuleSet::defaults();
    rules.rules.iter_mut().find(|r| r.id == "disk-full").unwrap().crit = Some(80.0);
    assert!(rules.validate().unwrap_err().contains("critical line has to be past the warning line"));
    let mut rules = RuleSet::defaults();
    rules.rules.iter_mut().find(|r| r.id == "optic-rx-low").unwrap().crit = Some(-5.0);
    assert!(rules.validate().is_err(), "for a '<' rule critical is below the warning");

    let dir = std::env::temp_dir().join(format!("sb-rules-{}-{}", std::process::id(), fastrand::u32(..)));
    let h = Health::new(Some(dir.clone()));
    let mut edited = RuleSet::defaults();
    edited.rules.iter_mut().find(|r| r.id == "gpu-hot").unwrap().warn = Some(80.0);
    h.set_rules(edited).unwrap();
    assert_eq!(Health::new(Some(dir.clone())).rules().get("gpu-hot").unwrap().warn, Some(80.0), "saved and read back");
    std::fs::write(dir.join("rules.json"), "{not json").unwrap();
    let again = Health::new(Some(dir.clone()));
    assert_eq!(again.rules().get("gpu-hot").unwrap().warn, Some(85.0), "the defaults apply");
    assert!(std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("rules.json.corrupt")), "the bad file is kept aside");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saved_rules_keep_the_owners_numbers_over_new_defaults() {
    let mut saved = RuleSet::defaults();
    saved.rules.retain(|r| r.id != "endpoint-kv"); // an older file, before this rule existed
    let r = saved.rules.iter_mut().find(|r| r.id == "disk-full").unwrap();
    r.warn = Some(80.0);
    r.title = "Old wording".into();
    saved.rules.push(Rule { id: "gone-rule".into(), ..RuleSet::defaults().rules[0].clone() });
    let merged = RuleSet::merged_onto_defaults(&saved);
    assert_eq!(merged.get("disk-full").unwrap().warn, Some(80.0));
    assert_eq!(merged.get("disk-full").unwrap().title, "Root disk full", "wording comes from this version");
    assert!(merged.get("endpoint-kv").is_some() && merged.get("gone-rule").is_none());
}

#[test]
fn serving_memory_rules_arrive_without_overwriting_saved_memory_preferences() {
    let mut saved = RuleSet::defaults();
    saved.rules.retain(|r| !matches!(r.id.as_str(), "mem-available" | "mem-swap-in"));
    let r = saved.rules.iter_mut().find(|r| r.id == "mem-high").unwrap();
    r.warn = Some(91.0);
    r.for_s = 90;
    r.enabled = false;
    let merged = RuleSet::merged_onto_defaults(&saved);
    let old = merged.get("mem-high").unwrap();
    assert_eq!((old.warn, old.for_s, old.enabled), (Some(91.0), 90, false));
    let headroom = merged.get("mem-available").unwrap();
    assert_eq!((headroom.warn, headroom.crit, headroom.for_s, headroom.enabled), (Some(4.0), Some(2.0), 60, true));
    assert!(merged.get("mem-swap-in").unwrap().enabled);
    assert!(merged.validate().is_ok());
}

#[test]
fn would_fire_now_previews_a_candidate_without_changing_anything() {
    let h = Health::new(None);
    let mut s = healthy();
    s.nodes[3].mem.used = 100 * GIB; // 78%
    for _ in 0..3 {
        s.ts_ms += 1000;
        h.evaluate(&s);
    }
    let mut candidate = RuleSet::defaults();
    candidate.rules.iter_mut().find(|r| r.id == "mem-high").unwrap().warn = Some(75.0);
    let rows = h.preview(&candidate);
    let mem = rows.iter().find(|r| r.id == "mem-high").unwrap();
    assert_eq!(mem.matches.len(), 1);
    assert_eq!((mem.matches[0].label.as_str(), mem.matches[0].severity), ("gx10-0000", Severity::Warn));
    assert_eq!(mem.matches[0].fires_in_s, 60, "it would still have to hold for a minute");
    assert!(rows.iter().find(|r| r.id == "disk-full").unwrap().matches.is_empty());
    assert!(h.view().alerts.is_empty(), "previewing raised nothing");
    assert_eq!(h.rules().get("mem-high").unwrap().warn, Some(90.0), "and saved nothing");
}

// ---- The rules added in round 2

#[test]
fn clock_offset_and_ntp_sync() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 61, |s, _| s.nodes[1].system.clock_offset_ms = Some(-230.0));
    let a = &ev.iter().find(|e| e.alert.rule == "ntp-offset").unwrap().alert;
    assert_eq!((a.severity, a.summary.as_str(), a.message.as_str()), (Severity::Warn, "cell2 clock offset -230 ms", "Clock -230 ms off NTP time"));
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 601, |s, _| s.nodes[2].system.ntp_synced = Some(false));
    assert_eq!(ev.iter().filter(|e| e.alert.rule == "ntp-unsynced").count(), 1, "after ten minutes unsynchronized");
}

#[test]
fn fabric_mtu_off_the_expected_value_or_its_peers_warns() {
    let mut e = Engine::default();
    let mut s = healthy();
    for n in &mut s.nodes {
        n.nets[0].mtu = Some(9000);
    }
    let ev = run(&mut e, &mut s, 31, |s, _| s.nodes[3].nets[0].mtu = Some(1500));
    let a = &ev.iter().find(|e| e.alert.rule == "fabric-mtu").expect("mtu").alert;
    assert_eq!((a.entity.node.as_deref(), a.message.as_str()), (Some("gx10-0000"), "enp1s0f1np1 MTU 1500, expected 9000"));
    assert_eq!(ev.iter().filter(|e| e.alert.rule == "fabric-mtu").count(), 1, "only the odd one out");
}

fn endpoint(health: &str, metrics: &[(&str, f64)]) -> EndpointInfo {
    EndpointInfo {
        id: "cell1-vllm-8000".into(),
        label: "llama-3.3-70b on cell1–cell3".into(),
        model: "llama-3.3-70b".into(),
        head_node: "cell1".into(),
        health: health.into(),
        reason: (health == "down").then(|| "Pipeline stage 3 of 3 on cell3 is offline".to_string()),
        message: None,
        metrics: metrics.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        ..Default::default()
    }
}

fn run_ep(e: &mut Engine, s: &mut ClusterSnapshot, secs: u64, ep: impl Fn(u64) -> EndpointInfo) -> Vec<AlertEvent> {
    let mut events = Vec::new();
    for i in 0..secs {
        s.ts_ms += 1000;
        events.extend(e.step_with(s, &[ep(i)], s.ts_ms));
    }
    events
}

#[test]
fn a_down_model_says_why_in_its_alert() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[2].status = LinkStatus::Offline;
    let ev = run_ep(&mut e, &mut s, 31, |_| endpoint("down", &[]));
    let a = &ev.iter().find(|e| e.alert.rule == "endpoint-down").expect("fires even though cell3 is offline too").alert;
    assert_eq!(a.severity, Severity::Crit);
    assert_eq!(a.summary, "llama-3.3-70b is down: pipeline stage 3 of 3 on cell3 is offline");
    assert_eq!(a.message, "Down · Pipeline stage 3 of 3 on cell3 is offline");
}

#[test]
fn a_structured_error_in_a_models_reason_shows_its_message_not_its_json() {
    let mut e = Engine::default();
    let mut s = healthy();
    let refused = crate::errors::SbError::new(crate::errors::ErrorKind::Fenced, "No fence is open, so changes on the cluster are refused.").on("cell1").to_json();
    let ep = EndpointInfo { id: "cell1-vllm-8000".into(), label: "qwen3-1.7b on cell1".into(), model: "qwen3-1.7b".into(), head_node: "cell1".into(), health: "down".into(), reason: Some(refused), ..Default::default() };
    let mut events = Vec::new();
    for _ in 0..31 {
        s.ts_ms += 1000;
        events.extend(e.step_with(&s, std::slice::from_ref(&ep), s.ts_ms));
    }
    let a = &events.iter().find(|ev| ev.alert.rule == "endpoint-down").expect("fires").alert;
    assert_eq!(a.summary, "qwen3-1.7b is down: no fence is open, so changes on the cluster are refused.");
    assert_eq!(a.message, "Down · No fence is open, so changes on the cluster are refused.");
    assert!(!a.summary.contains('{') && !a.message.contains('{'));
}

/// On a two-node load the refusal comes back with its node in front of it.
#[test]
fn a_structured_error_after_a_node_name_shows_its_message_too() {
    let mut e = Engine::default();
    let mut s = healthy();
    let refused = crate::errors::SbError::new(crate::errors::ErrorKind::Fenced, "No fence is open, so changes on the cluster are refused.").on("cell2").to_json();
    let ep = EndpointInfo { id: "cell1-vllm-8000".into(), label: "qwen3-1.7b on cell1".into(), model: "qwen3-1.7b".into(), head_node: "cell1".into(), health: "down".into(), reason: Some(format!("cell2: {refused}")), ..Default::default() };
    let mut events = Vec::new();
    for _ in 0..31 {
        s.ts_ms += 1000;
        events.extend(e.step_with(&s, std::slice::from_ref(&ep), s.ts_ms));
    }
    let a = &events.iter().find(|ev| ev.alert.rule == "endpoint-down").expect("fires").alert;
    assert_eq!(a.summary, "qwen3-1.7b is down: cell2: No fence is open, so changes on the cluster are refused.");
    assert!(!a.summary.contains('{') && !a.message.contains('{'));
}

#[test]
fn serving_slos_warn_after_their_durations() {
    // TTFT p95 over 2 s for 5 minutes.
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run_ep(&mut e, &mut s, 299, |_| endpoint("ready", &[("ttftP95", 2400.0), ("genTps", 30.0)]));
    assert!(ev.iter().all(|e| e.alert.rule != "endpoint-ttft"));
    let ev = run_ep(&mut e, &mut s, 2, |_| endpoint("ready", &[("ttftP95", 2400.0), ("genTps", 30.0)]));
    let a = &ev.iter().find(|e| e.alert.rule == "endpoint-ttft").unwrap().alert;
    assert_eq!((a.severity, a.message.as_str()), (Severity::Warn, "TTFT p95 2.4 s"));
    // Requests waiting for a minute, KV cache over 90% for two.
    let mut e = Engine::default();
    let ev = run_ep(&mut e, &mut s, 121, |_| endpoint("ready", &[("waiting", 3.0), ("kvPct", 94.0), ("genTps", 10.0)]));
    let rules: Vec<_> = ev.iter().map(|e| e.alert.rule.as_str()).collect();
    assert!(rules.contains(&"endpoint-waiting") && rules.contains(&"endpoint-kv"), "{rules:?}");
    assert_eq!(ev.iter().find(|e| e.alert.rule == "endpoint-waiting").unwrap().alert.message, "3 requests waiting");
    // Preemptions rising.
    let mut e = Engine::default();
    let ev = run_ep(&mut e, &mut s, 5, |i| endpoint("ready", &[("preemptions", 10.0 + i as f64 * 2.0), ("genTps", 10.0)]));
    assert!(ev.iter().any(|e| e.alert.rule == "endpoint-preempt"), "{ev:?}");
}

#[test]
fn an_idle_model_is_information_that_never_counts() {
    let mut e = Engine::default();
    let mut s = healthy();
    run_ep(&mut e, &mut s, 1, |_| endpoint("ready", &[("genTps", 0.0), ("running", 0.0)]));
    s.ts_ms += 24 * 3_600_000;
    let ev = run_ep(&mut e, &mut s, 1, |_| endpoint("ready", &[("genTps", 0.0), ("running", 0.0)]));
    let a = &ev.iter().find(|e| e.alert.rule == "endpoint-idle").expect("after a day idle").alert;
    assert_eq!((a.severity, a.notify), (Severity::Info, false));
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!((v.summary.info, v.summary.warn, v.summary.crit), (1, 0, 0));
    assert_eq!(v.nodes["cell1"].level, Level::Ok, "information doesn't colour the node");
}

#[test]
fn a_crash_loop_clears_once_the_container_stays_up_a_minute() {
    let mut e = Engine::default();
    let mut s = healthy();
    let start_s = s.ts_ms / 1000;
    let ev = run(&mut e, &mut s, 20, |s, i| {
        let c = &mut s.nodes[1].containers[0];
        c.restart_count = (i / 5) as u32;
        c.state = if i % 5 < 2 { "restarting".into() } else { "running".into() };
        c.started_at = start_s + i;
    });
    let raised: Vec<_> = ev.iter().filter(|e| e.alert.rule == "crash-loop" && e.kind == EventKind::Raised).collect();
    assert_eq!(raised.len(), 1, "one raise, however often it restarted: {ev:?}");
    assert_eq!(raised[0].alert.severity, Severity::Crit);
    assert!(ev.iter().all(|e| e.kind != EventKind::Eased && e.kind != EventKind::Resolved), "the running seconds between restarts don't ease it: {ev:?}");
    // Now it stays up: a minute up ends the condition (the window still counts the restarts), and
    // it resolves after its clear time.
    let up_since = s.ts_ms / 1000;
    let clear_s = RuleSet::defaults().th("crash-loop", None).clear_ms / 1000;
    assert_eq!(clear_s, 300);
    let ev = run(&mut e, &mut s, CRASH_RECENT_S + clear_s - 2, |s, _| s.nodes[1].containers[0].started_at = up_since);
    assert!(ev.iter().all(|e| e.kind != EventKind::Resolved && e.kind != EventKind::Eased), "not yet: {ev:?}");
    let ev = run(&mut e, &mut s, 3, |s, _| s.nodes[1].containers[0].started_at = up_since);
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved && e.alert.rule == "crash-loop"), "{ev:?}");
}

#[test]
fn verified_restart_clears_within_one_poll() {
    // A crash loop, then the ordered restart SparkBox ran; its Verify calls resolve_now.
    let h = Health::new(None);
    let mut s = healthy();
    s.ts_ms = crate::history::now_ms();
    let start_s = s.ts_ms / 1000;
    for i in 0..20u64 {
        s.ts_ms += 1000;
        let c = &mut s.nodes[1].containers[0];
        c.restart_count = (i / 5) as u32;
        c.state = if i % 5 < 2 { "restarting".into() } else { "running".into() };
        c.started_at = start_s + i;
        h.evaluate(&s);
    }
    assert_eq!(h.view().summary.crit, 1);
    let up = s.ts_ms / 1000;
    s.nodes[1].containers[0].state = "running".into();
    s.nodes[1].containers[0].started_at = up;
    h.resolve_now("crash-loop", "vllm-head", "Verified: llama-3.3-70b answered after 26 s");
    assert_eq!(h.view().summary.crit, 0, "at once, before the next poll");
    // The next polls (within 5 s): it stays clear although the window still counts the old restarts.
    let mut events = Vec::new();
    for _ in 0..5 {
        s.ts_ms += 1000;
        events.extend(h.evaluate(&s).events);
    }
    assert_eq!(h.view().summary.crit, 0);
    let r = events.iter().find(|e| e.kind == EventKind::Resolved).expect("the resolution is notified with the next poll");
    assert_eq!(r.alert.reason.as_deref(), Some("Verified: llama-3.3-70b answered after 26 s"));
    assert!(events.iter().all(|e| e.kind != EventKind::Raised), "{events:?}");
    assert_eq!(h.history(100).iter().filter(|e| e.alert.reason.as_deref() == Some("Verified: llama-3.3-70b answered after 26 s")).count(), 1, "logged once");
    // A new crash after the verify is a new problem.
    let mut events = Vec::new();
    for i in 0..3u64 {
        s.ts_ms += 1000;
        s.nodes[1].containers[0].restart_count = 10 + i as u32;
        s.nodes[1].containers[0].state = "restarting".into();
        events.extend(h.evaluate(&s).events);
    }
    assert!(events.iter().any(|e| e.kind == EventKind::Raised && e.alert.rule == "crash-loop"));
}

#[test]
fn a_crash_loop_clears_when_its_model_answers() {
    let mut e = Engine::default();
    let mut s = healthy();
    let start_s = s.ts_ms / 1000;
    let ep = |health: &str| EndpointInfo { id: "cell1-vllm-8000".into(), health: health.into(), head_node: "cell1".into(), nodes: vec!["cell1".into(), "cell2".into()], containers: vec!["vllm-head".into()], ..Default::default() };
    let mut raised = false;
    for i in 0..20u64 {
        s.ts_ms += 1000;
        let c = &mut s.nodes[1].containers[0];
        c.restart_count = (i / 5) as u32;
        c.state = if i % 5 < 2 { "restarting".into() } else { "running".into() };
        c.started_at = start_s + i;
        raised |= e.step_with(&s, &[ep("down")], s.ts_ms).iter().any(|x| x.alert.rule == "crash-loop");
    }
    assert!(raised);
    // It's running again, a few seconds up, and the model answers: the crash loop is over, and it
    // clears after its clear time without easing on the way.
    let mut ev = Vec::new();
    for _ in 0..301 {
        s.ts_ms += 1000;
        s.nodes[1].containers[0].state = "running".into();
        ev.extend(e.step_with(&s, &[ep("ready")], s.ts_ms));
    }
    assert_eq!(ev.iter().map(|x| x.kind).collect::<Vec<_>>(), [EventKind::Resolved], "{ev:?}");
}

#[test]
fn kernel_events_become_alerts_that_clear_after_their_window() {
    let mut e = Engine::default();
    let mut s = healthy();
    let now = s.ts_ms;
    s.nodes[3].kernel_events = vec![
        crate::model::KernelEvent { ts_ms: now, kind: "xid".into(), code: Some(79), text: "NVRM: Xid (PCI:000f:01:00): 79, GPU has fallen off the bus.".into(), severity: "crit".into(), process: None },
        crate::model::KernelEvent { ts_ms: now, kind: "oom".into(), code: None, text: "Out of memory: Killed process 4242 (ollama)".into(), severity: "crit".into(), process: Some("ollama".into()) },
        crate::model::KernelEvent { ts_ms: now, kind: "mlx5".into(), code: None, text: "mlx5_core 0000:01:00.1 enp1s0f1np1: health compromised - reached miss count".into(), severity: "warn".into(), process: None },
        crate::model::KernelEvent { ts_ms: now, kind: "mlx5".into(), code: None, text: "mlx5_core 0000:01:00.1 enp1s0f1np1: Link down".into(), severity: "warn".into(), process: None },
        crate::model::KernelEvent { ts_ms: now, kind: "hung".into(), code: None, text: "INFO: task jbd2/nvme0n1p2-8:612 blocked for more than 122 seconds.".into(), severity: "warn".into(), process: Some("jbd2/nvme0n1p2-8".into()) },
    ];
    let ev = run(&mut e, &mut s, 1, |_, _| {});
    let get = |rule: &str| ev.iter().find(|e| e.alert.rule == rule).map(|e| e.alert.clone()).unwrap_or_else(|| panic!("{rule}: {ev:?}"));
    let xid = get("kernel-xid");
    assert_eq!((xid.severity, xid.summary.as_str()), (Severity::Crit, "gx10-0000 GPU Xid 79"));
    assert_eq!(xid.message, "Xid 79 · the GPU stopped answering on PCIe");
    assert!(xid.detail.as_deref().unwrap().starts_with("Restart the node"));
    assert_eq!(get("kernel-oom").summary, "ollama OOM-killed on gx10-0000");
    assert_eq!((get("kernel-mlx5").entity.id.as_str(), get("kernel-mlx5").message.as_str()), ("enp1s0f1np1", "mlx5_core: health compromised - reached miss count"));
    assert_eq!(ev.iter().filter(|e| e.alert.rule == "kernel-mlx5").count(), 1, "the link going down is information");
    assert_eq!(get("kernel-hung").severity, Severity::Warn);
    // An hour later a warning Xid is history; a fatal one isn't.
    let mut e = Engine::default();
    let mut s = healthy();
    let now = s.ts_ms;
    s.nodes[3].kernel_events = vec![crate::model::KernelEvent { ts_ms: now, kind: "xid".into(), code: Some(13), text: "NVRM: Xid (PCI:000f:01:00): 13, Graphics Exception".into(), severity: "warn".into(), process: None }];
    run(&mut e, &mut s, 1, |_, _| {});
    s.ts_ms += 3_601_000;
    let ev = run(&mut e, &mut s, 1, |_, _| {});
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved && e.alert.rule == "kernel-xid"));
}

#[test]
fn a_fatal_xid_stays_open_until_the_node_boots_again() {
    let mut e = Engine::default();
    let mut s = healthy();
    for n in &mut s.nodes {
        n.boot_id = Some(format!("{}-boot-1", n.id));
    }
    let now = s.ts_ms;
    s.nodes[3].kernel_events = vec![crate::model::KernelEvent { ts_ms: now, kind: "xid".into(), code: Some(79), text: "NVRM: Xid (PCI:000f:01:00): 79, GPU has fallen off the bus.".into(), severity: "crit".into(), process: Some("ollama".into()) }];
    let ev = run(&mut e, &mut s, 1, |_, _| {});
    let xid = ev.iter().find(|e| e.alert.rule == "kernel-xid").expect("raised");
    assert!(xid.alert.detail.as_deref().unwrap().ends_with("It stays open until the node restarts."));
    // Two hours later, with no event in the log any more (the journal rolled): still firing.
    s.ts_ms += 2 * 3_600_000;
    s.nodes[3].kernel_events.clear();
    let ev = run(&mut e, &mut s, 120, |_, _| {});
    assert!(ev.iter().all(|e| e.kind != EventKind::Resolved), "{ev:?}");
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!((v.summary.crit, v.alerts[0].rule.as_str()), (1, "kernel-xid"));
    // The node restarted: a new boot_id clears it.
    let ev = run(&mut e, &mut s, 1, |s, _| s.nodes[3].boot_id = Some("gx10-0000-boot-2".into()));
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved && e.alert.rule == "kernel-xid"), "{ev:?}");
}

#[test]
fn an_unreadable_gpu_is_critical_after_30_seconds() {
    let mut e = Engine::default();
    let mut s = healthy();
    let unreadable = |s: &mut ClusterSnapshot| {
        let g = &mut s.nodes[3].gpu;
        (g.readable, g.error) = (false, Some("nvidia-smi didn't answer".into()));
    };
    let ev = run(&mut e, &mut s, 29, |s, _| unreadable(s));
    assert!(ev.is_empty(), "{ev:?}");
    let ev = run(&mut e, &mut s, 2, |_, _| {});
    let a = &ev.iter().find(|e| e.alert.rule == "gpu-unreadable").expect("fires").alert;
    assert_eq!((a.severity, a.summary.as_str(), a.message.as_str()), (Severity::Crit, "gx10-0000 GPU not answering", "GPU readings unavailable · nvidia-smi didn't answer"));
    let ev = run(&mut e, &mut s, 1, |s, _| (s.nodes[3].gpu.readable, s.nodes[3].gpu.error) = (true, None));
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved));
    // The node's own error text never makes a GPU unreadable (round 3 matched "nvidia-smi" in it).
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 60, |s, _| s.nodes[3].error = Some("nvidia-smi: GPU idle; NVML answered".into()));
    assert!(ev.iter().all(|e| e.alert.rule != "gpu-unreadable"), "{ev:?}");
    // And an unreadable GPU's zeros are no reading for a threshold rule.
    let mut n = s.nodes[3].clone();
    n.gpu.readable = false;
    assert_eq!((node_metric(&n, "gpuTemp"), node_metric(&n, "gpuUtil"), node_metric(&n, "gpuPowerW")), (None, None, None));
    assert_eq!(node_metric(&s.nodes[3], "gpuUtil"), Some(0.0));
}

#[test]
fn a_roce_rate_rule_needs_no_code() {
    // A rule like the ones health::roce_rules adds: data only, judged generically.
    let mut rules = RuleSet::defaults();
    rules.rules.push(Rule {
        id: "roce-oos".into(),
        area: "fabric".into(),
        title: "Out-of-sequence packets".into(),
        entity: super::rule::EntityKind::Net,
        kind: super::rule::RuleKind::Rate,
        metric: "oos".into(),
        op: Op::Gt,
        warn: Some(10.0),
        crit: Some(100.0),
        window_s: Some(30),
        unit: "/s".into(),
        message: "{value} out of sequence".into(),
        for_s: 0,
        clear_s: 0,
        ..RuleSet::defaults().rules[0].clone()
    });
    let mut e = engine_with(rules);
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 5, |s, i| s.nodes[1].nets[0].roce = Some(crate::model::RoceCounters { out_of_sequence: Some(1000 + i * 50), ..Default::default() }));
    let a = &ev.iter().find(|e| e.alert.rule == "roce-oos").expect("rate").alert;
    assert_eq!((a.severity, a.message.as_str(), a.category), (Severity::Warn, "50/s out of sequence", Category::Fabric));
    let ev = run(&mut e, &mut s, 40, |s, i| s.nodes[1].nets[0].roce = Some(crate::model::RoceCounters { out_of_sequence: Some(1250 + i * 500), ..Default::default() }));
    assert!(ev.iter().any(|e| e.kind == EventKind::Escalated && e.alert.rule == "roce-oos"));
    // A counter the driver doesn't have: no reading, no alert, and the preview says so.
    let h = Health::new(None);
    h.evaluate(&healthy());
    let mut candidate = RuleSet::defaults();
    candidate.rules.push(Rule { id: "roce-x".into(), entity: super::rule::EntityKind::Net, kind: super::rule::RuleKind::Rate, metric: "cnpSent".into(), ..RuleSet::defaults().rules[0].clone() });
    assert!(!h.preview(&candidate).iter().find(|r| r.id == "roce-x").unwrap().known);
}

#[test]
fn imported_agent_events_land_in_the_log_once_and_in_order() {
    let h = Health::new(None);
    let mut s = healthy();
    s.nodes[0].failed_units = vec!["x.service".into()];
    s.ts_ms += 1000;
    h.evaluate(&s);
    let mut away = h.history(1)[0].clone();
    away.ts_ms -= 3_600_000;
    away.away = true;
    away.source = Some("agent:cell1".into());
    assert_eq!(h.import(vec![away.clone()]), 1);
    assert_eq!(h.import(vec![away]), 0);
    let log = h.history(10);
    assert!(log[0].ts_ms > log[1].ts_ms && log[1].away, "newest first, the away one older");
}

#[test]
fn summaries_keep_acronyms_and_say_which_node() {
    assert_eq!(first_lower("RoCE ACK timeouts"), "RoCE ACK timeouts");
    assert_eq!(first_lower("CRC errors on the fabric"), "CRC errors on the fabric");
    assert_eq!(first_lower("Unified memory"), "unified memory");
    let iface = Entity { kind: "interface".into(), id: "enP2p1s0f1np1".into(), node: Some("cell2".into()), switch: None, label: "enP2p1s0f1np1 on cell2".into() };
    assert_eq!(short_label(&iface), "cell2 enP2p1s0f1np1", "every node has that interface, so the node leads");
    let port = Entity { kind: "port".into(), id: "qsfp56-dd-1-2".into(), node: Some("cell2".into()), switch: Some("crs804".into()), label: "qsfp56-dd-1-2 (cell2) on CRS804 DDQ".into() };
    assert_eq!(short_label(&port), "qsfp56-dd-1-2 (cell2)");
}

// ---- Incidents

/// An active alert as the incident-roots fixture names one.
fn fixture_alert(v: &serde_json::Value) -> Alert {
    let s = |k: &str| v[k].as_str().map(String::from);
    let entity = Entity { kind: s("kind").unwrap(), id: s("id").unwrap(), node: s("node"), switch: s("switch"), label: s("id").unwrap() };
    let c = Condition {
        key: format!("{}:{}", s("rule").unwrap(), s("id").unwrap()),
        rule: s("rule").unwrap(),
        category: Category::Node,
        severity: Severity::Crit,
        entity,
        message: String::new(),
        summary: String::new(),
        detail: None,
        value: None,
        for_ms: 0,
        clear_ms: 0,
        notify: true,
        cause: None,
        short: None,
    };
    Alert { state: AlertState::Firing, ..Alert::from(&c, 0) }
}

#[test]
fn incident_roots_follow_the_fixture() {
    // agent_test.py runs the same file through the node agent's grouping.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/health/fixtures/incident-roots.json");
    let f: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut snap = healthy();
    for n in &mut snap.nodes {
        n.nets.retain(|x| x.role != NetRole::Fabric);
        n.gpu_fault = f["gpuFault"].as_array().unwrap().iter().any(|g| g == n.id.as_str()).then(|| crate::model::GpuFault { code: Some(79), text: "Xid 79".into(), since_ms: 0, source: "xid".into() });
    }
    for x in f["ifaces"].as_array().unwrap() {
        let n = snap.nodes.iter_mut().find(|n| x["node"] == n.id.as_str()).unwrap();
        n.nets.push(NetStats {
            iface: x["iface"].as_str().unwrap().into(),
            role: NetRole::Fabric,
            up: true,
            ip: x["ip"].as_str().map(String::from),
            switch_port: x["switchPort"].as_str().map(String::from),
            ..Default::default()
        });
    }
    let list = |v: &serde_json::Value| v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect::<Vec<_>>();
    let endpoints: Vec<EndpointInfo> = f["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| EndpointInfo {
            id: e["id"].as_str().unwrap().into(),
            head_node: e["headNode"].as_str().unwrap().into(),
            nodes: list(&e["nodes"]),
            containers: list(&e["containers"]),
            missing_nodes: list(&e["missingNodes"]),
            health: "down".into(),
            ..Default::default()
        })
        .collect();
    let mut mac_cases = 0;
    for case in f["cases"].as_array().unwrap() {
        let active: Vec<Alert> = case["active"].as_array().unwrap().iter().map(fixture_alert).collect();
        let refs: Vec<&Alert> = active.iter().collect();
        // A macOnly case: this Mac's verdict, and the nodes timing out.
        let mut snap = snap.clone();
        if let Some(state) = case.get("macReach") {
            mac_cases += 1;
            snap.mac_reach = Some(MacReach { state: serde_json::from_value(state.clone()).unwrap(), ..Default::default() });
            for n in snap.nodes.iter_mut().filter(|n| case["unreachable"].as_array().unwrap().iter().any(|u| u == n.id.as_str())) {
                (n.status, n.error_kind) = (LinkStatus::Offline, Some("timeout".into()));
            }
        }
        let roots = incident::RootCtx::new(&snap, &endpoints, &refs);
        assert_eq!(roots.root_of(&fixture_alert(&case["alert"])), case["root"].as_str().unwrap(), "{}", case["name"]);
    }
    for state in ["lostHomeNetwork", "lanBetween", "wiredSwitch", "vpnProblem", "noRoute"] {
        assert!(f["cases"].as_array().unwrap().iter().any(|c| c["macReach"] == state && c["root"] == incident::MAC_ROOT), "a case for {state}");
    }
    assert!(mac_cases >= 5);
}

/// Seen in Live on Oct 29 with the whole cluster unreachable: cell1's Ollama, no longer listed
/// once its node stopped answering, kept its held "Down" alert as an incident of its own beside
/// "cell1 offline". It's cell1's. (App-only: the agents never hold a model alert for a node
/// that's gone, so this isn't in incident-roots.json.)
#[test]
fn a_model_no_longer_listed_is_its_down_nodes() {
    let snap = healthy();
    let down = fixture_alert(&serde_json::json!({"rule": "endpoint-down", "kind": "endpoint", "id": "cell1-ollama-11434", "node": "cell1"}));
    let offline = fixture_alert(&serde_json::json!({"rule": "node-offline", "kind": "node", "id": "cell1", "node": "cell1"}));
    let roots = incident::RootCtx::new(&snap, &[], &[&offline]);
    assert_eq!(roots.root_of(&down), "node:cell1");
    // Its node answering, it stays its own.
    let roots = incident::RootCtx::new(&snap, &[], &[]);
    assert_eq!(roots.root_of(&down), "endpoint:cell1-ollama-11434");
}


#[test]
fn a_rate_rule_warning_above_zero_clears_once_its_counter_stops() {
    // The round-2 bug behind the 23-minute RoCE alerts: a warning line of 0/s with no clear line
    // of its own held the alert while the rate merely *reached* 0, so it never cleared.
    let mut rules = RuleSet::defaults();
    rules.rules.push(Rule {
        id: "oos-rate".into(),
        entity: super::rule::EntityKind::Net,
        kind: super::rule::RuleKind::Rate,
        metric: "oos".into(),
        warn: Some(0.0),
        crit: None,
        clear: None,
        window_s: Some(10),
        unit: "/s".into(),
        for_s: 0,
        clear_s: 30,
        ..RuleSet::defaults().rules[0].clone()
    });
    let mut e = engine_with(rules);
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 20, |s, i| s.nodes[1].nets[0].roce = Some(crate::model::RoceCounters { out_of_sequence: Some(100 + i * 40), ..Default::default() }));
    assert!(ev.iter().any(|e| e.kind == EventKind::Raised && e.alert.rule == "oos-rate"));
    // The counter stops: 0/s from the next window on, then the clear time.
    let ev = run(&mut e, &mut s, 45, |s, _| s.nodes[1].nets[0].roce = Some(crate::model::RoceCounters { out_of_sequence: Some(860), ..Default::default() }));
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved && e.alert.rule == "oos-rate"), "{ev:?}");
}

#[test]
fn agent_settings_that_didnt_arrive_warn() {
    let mut e = Engine::default();
    let mut s = healthy();
    let beat = |now: u64| crate::model::AgentBeat {
        version: "1.0.0".into(),
        last_beat_ms: now,
        config_current: Some(false),
        push_error: Some("sudo refused".into()),
        ..Default::default()
    };
    let ev = run(&mut e, &mut s, 61, |s, _| {
        let now = s.ts_ms;
        s.nodes[2].agent = Some(beat(now));
    });
    let a = &ev.iter().find(|e| e.alert.rule == "agent-config-stale").expect("after a minute").alert;
    assert_eq!((a.severity, a.message.as_str()), (Severity::Warn, "Alert settings didn't reach cell3 (push failed: sudo refused)"));
    assert_eq!(a.incident_id.as_deref(), Some("node:cell3"));
    // With the time it failed, when the beat has it.
    let at = s.ts_ms - 60_000;
    run(&mut e, &mut s, 1, |s, _| {
        let now = s.ts_ms;
        s.nodes[2].agent = Some(crate::model::AgentBeat { push_error_ms: Some(at), ..beat(now) });
    });
    let a = e.view(s.ts_ms, Some(&s)).alerts.into_iter().find(|a| a.rule == "agent-config-stale").unwrap();
    assert_eq!(a.message, format!("Alert settings didn't reach cell3 (push failed {}: sudo refused)", crate::health::maintenance::local_hhmm(at)));
    // The push goes through: it clears at once.
    let ev = run(&mut e, &mut s, 1, |s, _| {
        let now = s.ts_ms;
        s.nodes[2].agent = Some(crate::model::AgentBeat { config_current: Some(true), push_error: None, ..beat(now) });
    });
    assert!(ev.iter().any(|e| e.kind == EventKind::Resolved && e.alert.rule == "agent-config-stale"));
}

// ---- r4: the real cluster at rest, recabling, flapping

/// One of the real Live snapshots of Oct 29 (snapshot-tour-1/2.json): four idle GB10s, Ollama
/// on cell1 and cell3 bound to 127.0.0.1 with nothing in memory, the failed glm53 run two days
/// old, 58 mlx5 link events from recabling, PFC off.
fn live_idle(n: u8) -> ClusterSnapshot {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("src/health/fixtures/live-idle-{n}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The two idle Ollamas, as the endpoint list has them: idle (nothing in memory), or for a
/// moment gone (the process missed a sample).
fn idle_ollamas(health: &str) -> Vec<EndpointInfo> {
    ["cell1", "cell3"]
        .iter()
        .map(|n| EndpointInfo { id: format!("{n}-ollama-11434"), label: format!("Ollama on {n}"), model: String::new(), head_node: n.to_string(), health: health.into(), nodes: vec![n.to_string()], ..Default::default() })
        .collect()
}

#[test]
fn the_real_cluster_at_rest_raises_nothing() {
    let h = Health::new(None);
    let snaps = [live_idle(1), live_idle(2)];
    assert!(snaps.iter().all(|s| !s.demo && s.nodes.len() == 4));
    let start = snaps[1].ts_ms;
    // This Mac's path is fine: its router answers.
    h.reach.set_probes(router_up());
    // The gate as the day left it: the 17:32 outage paged (four nodes and cell1's Ollama), then
    // cleared; and a page whose clear never came (SparkBox quit while cell3 was away).
    let settings = crate::notify::NotifySettings { repeat_minutes: 15, ..Default::default() };
    let mut gate = crate::notify::Gate::default();
    let outage = start - 3_600_000;
    let mut earlier = Vec::new();
    for n in ["cell1", "cell2", "cell3", "gx10-0000"] {
        earlier.push(gate_event(EventKind::Raised, "node-offline", Severity::Crit, n, &format!("{n} offline · no SSH reply in 10 s")));
    }
    earlier.push(gate_event(EventKind::Raised, "endpoint-down", Severity::Crit, "cell1", "Ollama on cell1 is down: cell1 is offline"));
    let (_, sent) = crate::notify::decide_events(&mut gate, &earlier, &settings, outage);
    assert_eq!(sent.len(), 1, "one grouped message then");
    let cleared: Vec<AlertEvent> = earlier.iter().filter(|e| e.alert.entity.id != "cell3").map(|e| AlertEvent { kind: EventKind::Resolved, ..e.clone() }).collect();
    let (_, back) = crate::notify::decide_events(&mut gate, &cleared, &settings, outage + 120_000);
    assert!(!back.is_empty(), "the nodes' return was sent at once");
    for i in 0..40 {
        gate.tick(&settings.gate(), outage + 120_000 + i * 30_000);
        gate.flush(outage + 130_000 + i * 30_000);
    }
    let mut events = Vec::new();
    let mut delivered = Vec::new();
    for t in 0..3600u64 {
        // A snapshot a second for an hour, the two real ones in turn, as the tour saw them.
        let mut s = snaps[(t / 7 % 2) as usize].clone();
        s.ts_ms = start + t * 1000;
        for (i, n) in s.nodes.iter_mut().enumerate() {
            n.last_seen_ms = Some(s.ts_ms);
            // The idle alternation: "SW power cap" on every other reading, at P8 and 4 W.
            n.gpu.throttle = if (t + i as u64) % 2 == 0 { vec!["SW power cap".into()] } else { vec![] };
            // Ollama now and then missing from the process sample.
            if t % 97 < 3 {
                n.processes.retain(|p| !p.name.contains("ollama"));
            }
        }
        super::kernel::stamp(&mut s);
        h.reach.stamp(&mut s);
        assert_eq!(s.mac_reach.as_ref().map(|r| r.state), Some(MacReachState::Ok), "at rest this Mac reaches the cluster");
        let endpoints = idle_ollamas(if t % 97 < 3 { "gone" } else { "idle" });
        let ev = h.evaluate_with(&s, &endpoints).events;
        // Through the real gate, as on_alerts and notify::start drive it: decide, then every
        // 30 s the repeat timer on what's firing, and the group window.
        delivered.extend(crate::notify::decide_events(&mut gate, &ev, &settings, s.ts_ms).1);
        if t % 30 == 29 {
            gate.sync(&crate::notify::firing_in(&h.view(), s.ts_ms));
            gate.tick(&settings.gate(), s.ts_ms);
        }
        delivered.extend(gate.flush(s.ts_ms));
        events.extend(ev);
    }
    assert!(events.is_empty(), "{:?}", events.iter().map(|e| (e.kind, e.alert.summary.clone())).collect::<Vec<_>>());
    assert!(delivered.is_empty(), "nothing delivered in an hour at rest: {:?}", delivered.iter().map(|n| &n.title).collect::<Vec<_>>());
    let v = h.view();
    assert!(v.alerts.is_empty(), "nothing even pending: {:?}", v.alerts.iter().map(|a| &a.summary).collect::<Vec<_>>());
    assert_eq!((v.summary.warn, v.summary.crit, v.summary.info, v.summary.pending), (0, 0, 0, 0));
    assert!(!v.alerts.iter().any(|a| a.rule == "mac-reach" || a.rule == "node-needs-you"));
    assert!(v.nodes.values().all(|n| n.level == Level::Ok));
    // Settings › Alerts "Would fire now": nothing, for every rule.
    let preview = h.preview(&RuleSet::defaults());
    assert!(preview.iter().all(|r| r.matches.is_empty()), "{:?}", preview.iter().filter(|r| !r.matches.is_empty()).map(|r| &r.id).collect::<Vec<_>>());
    // The real nodes don't report a clock offset: "Not reported by this node", by each of them.
    let clock = preview.iter().find(|r| r.id == "ntp-offset").unwrap();
    assert_eq!((clock.known, clock.unreported.clone()), (false, vec!["cell1".to_string(), "cell2".into(), "cell3".into(), "gx10-0000".into()]));
    assert!(preview.iter().find(|r| r.id == "mem-high").unwrap().unreported.is_empty(), "memory is reported");
}

#[test]
fn the_idle_and_gone_endpoints_raise_nothing() {
    let mut e = Engine::default();
    let mut s = healthy();
    for health in ["idle", "gone", "paused"] {
        let ev = run(&mut e, &mut s, 120, |_, _| {});
        assert!(ev.is_empty());
        let mut events = Vec::new();
        for _ in 0..120 {
            s.ts_ms += 1000;
            events.extend(e.step_with(&s, &idle_ollamas(health), s.ts_ms));
        }
        assert!(events.is_empty(), "{health}: {events:?}");
        assert!(e.view(s.ts_ms, Some(&s)).alerts.is_empty(), "{health}");
    }
    // Down still is.
    let mut events = Vec::new();
    for _ in 0..31 {
        s.ts_ms += 1000;
        events.extend(e.step_with(&s, &idle_ollamas("down"), s.ts_ms));
    }
    assert_eq!(events.iter().filter(|e| e.alert.rule == "endpoint-down").count(), 2);
}

/// The first and last of the 42 link events of Oct 26 (2:16:21 PM and 2:29:07 PM on the Mac).
const FIRST_OCT26: u64 = 1_793_038_581_337;
const LAST_OCT26: u64 = 1_793_039_347_731;

#[test]
fn recabling_is_one_group_of_information_and_never_an_alert() {
    // The 58 real mlx5 lines from the four nodes (Oct 25–26), as the collector sent them.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/health/fixtures/kernel-mlx5.jsonl");
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 58);
    let mut s = live_idle(2);
    for n in s.nodes.iter_mut() {
        n.kernel_events = lines.iter().filter(|l| l["node"] == n.id.as_str()).map(|l| serde_json::from_value(l.clone()).unwrap()).collect();
    }
    super::kernel::stamp(&mut s);
    let all: Vec<&crate::model::KernelEvent> = s.nodes.iter().flat_map(|n| n.kernel_events.iter()).collect();
    assert!(all.iter().all(|e| e.kind == "link" && e.severity == "info"), "{:?}", all.iter().filter(|e| e.kind != "link").collect::<Vec<_>>());
    // The Events tab: each recabling is one row. The real log holds three: gx10-0000 alone on
    // Oct 25 at 1:47 PM, the three cells at 5:08 PM, and all four nodes on Oct 26, 2:16–2:29 PM.
    let names = s.nodes.iter().map(|n| (n.id.clone(), n.name.clone())).collect();
    let decoded: Vec<_> = s.nodes.iter().flat_map(|n| n.kernel_events.iter().map(|e| super::kernel::decode(&n.id, e))).collect();
    let rows = super::kernel::rows(decoded, &names);
    assert_eq!(rows.iter().map(|r| r.events.len()).sum::<usize>(), 58, "every event is in a group");
    assert_eq!(rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), [
        "42 link events on 4 nodes (links down)",
        "12 link events on 3 nodes (cables unplugged and links down)",
        "4 link events on gx10-0000 (cables unplugged and links down)",
    ]);
    assert!(rows.iter().all(|r| r.severity == "info" && r.kind == "link"));
    let oct26 = &rows[0];
    assert_eq!(oct26.nodes, ["cell1", "cell2", "cell3", "gx10-0000"]);
    assert_eq!((oct26.ts_ms, oct26.to_ms), (FIRST_OCT26, Some(LAST_OCT26)), "2:16:21 to 2:29:07 PM");
    assert!(oct26.events.iter().any(|e| e.title == "Link down on enp1s0f1np1"));
    assert!(oct26.events.iter().any(|e| e.title == "RoCE port down on rocep1s0f1"));
    assert!(rows[1].events.iter().any(|e| e.title == "ConnectX-7 cable unplugged (0000:01:00.0)"));
    // No alert, then or ever.
    let mut e = Engine::default();
    for n in s.nodes.iter_mut() {
        for k in n.kernel_events.iter_mut() {
            k.ts_ms = s.ts_ms; // as if it had just happened
        }
    }
    let mut events = Vec::new();
    for _ in 0..30 {
        s.ts_ms += 1000;
        events.extend(e.step(&s, s.ts_ms));
    }
    assert!(events.iter().all(|e| e.alert.rule != "kernel-mlx5"), "{events:?}");
    // A real driver fault still is a warning.
    assert_eq!(super::kernel::classify_mlx5("mlx5_core 0000:01:00.1: wait_func_handle_exec_timeout:1171:(pid 2201): cmd[0]: ACCESS_REG(0x805) No done completion"), super::kernel::Mlx5::Fault);
    assert_eq!(super::kernel::classify_mlx5("mlx5_core 0000:01:00.0: mlx5_cmd_out_err:838:(pid 1234): QUERY_VPORT_COUNTER(0x770) op_mod(0x0) failed, status bad parameter(0x3), syndrome (0x67e4f9), err(-22)"), super::kernel::Mlx5::Fault);
    assert_eq!(super::kernel::classify_mlx5("mlx5_core 0000:01:00.0: health compromised - reached miss count"), super::kernel::Mlx5::Fault);
    assert_eq!(super::kernel::classify_mlx5("mlx5_core 0002:01:00.1 roceP2p1s0f1: Port: 1 Link ACTIVE"), super::kernel::Mlx5::Link);
}

#[test]
fn an_alert_back_within_ten_minutes_of_its_resolve_is_flapping_on_one_incident() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ep = |health: &str| vec![EndpointInfo { id: "cell3-ollama-11434".into(), label: "Ollama on cell3".into(), model: "ollama".into(), head_node: "cell3".into(), health: health.into(), nodes: vec!["cell3".into()], ..Default::default() }];
    // Down 31 s, back, down again two minutes later, twice: the live tour's flapping "Ollama Down".
    let mut events = Vec::new();
    let step = |e: &mut Engine, s: &mut ClusterSnapshot, secs: u64, health: &str, events: &mut Vec<AlertEvent>| {
        for _ in 0..secs {
            s.ts_ms += 1000;
            events.extend(e.step_with(s, &ep(health), s.ts_ms));
        }
    };
    let first_raise = s.ts_ms + 31_000;
    step(&mut e, &mut s, 31, "down", &mut events);
    step(&mut e, &mut s, 120, "ready", &mut events);
    let first: Vec<EventKind> = events.iter().map(|e| e.kind).collect();
    assert_eq!(first, [EventKind::Raised, EventKind::Resolved], "the first time is an ordinary alert");
    assert!(events[0].alert.flapping.is_none());
    // One resolve and one re-raise isn't flapping (round 4 said "Flapping · 1 times in 153 min").
    events.clear();
    step(&mut e, &mut s, 31, "down", &mut events);
    step(&mut e, &mut s, 120, "ready", &mut events);
    assert_eq!(events.iter().map(|e| e.kind).collect::<Vec<_>>(), [EventKind::Raised, EventKind::Resolved]);
    assert!(events.iter().all(|e| e.alert.flapping.is_none()), "one re-raise isn't flapping");
    // The second re-raise in the window is.
    events.clear();
    step(&mut e, &mut s, 31, "down", &mut events);
    let again = events.iter().find(|e| e.kind == EventKind::Raised).expect("raised again");
    let f = again.alert.flapping.clone().expect("flapping");
    assert_eq!((f.count, f.since_ms), (3, first_raise), "raised 3 times, counted from the first");
    assert_eq!(f.words(s.ts_ms), "Flapping · raised 3 times in 5 min");
    assert_eq!(again.alert.incident_id.as_deref(), Some("endpoint:cell3-ollama-11434"), "the same incident");
    // Now it keeps flipping every minute: it stays one firing alert and counts the flips.
    events.clear();
    for _ in 0..3 {
        step(&mut e, &mut s, 30, "ready", &mut events);
        step(&mut e, &mut s, 30, "down", &mut events);
    }
    assert!(events.is_empty(), "no more raises or resolves while it flaps: {events:?}");
    let v = e.view(s.ts_ms, Some(&s));
    let a = v.alerts.iter().find(|a| a.rule == "endpoint-down").unwrap();
    assert_eq!(a.flapping.as_ref().map(|f| f.count), Some(6));
    assert_eq!(v.summary.crit, 1, "counted once");
    let inc = v.incidents.iter().find(|i| i.id == "endpoint:cell3-ollama-11434").unwrap();
    assert_eq!(inc.flapping.as_ref().map(|f| f.count), Some(6));
    // It clears only after ten minutes clear.
    step(&mut e, &mut s, 599, "ready", &mut events);
    assert!(events.is_empty(), "{events:?}");
    step(&mut e, &mut s, 2, "ready", &mut events);
    assert_eq!(events.iter().map(|e| e.kind).collect::<Vec<_>>(), [EventKind::Resolved]);
    assert!(events[0].alert.flapping.is_some(), "the resolve says it was flapping");
    // Back within 10 minutes of that clear, it's the same run: still flapping, counted on (round 1
    // started the count over at the clear, so this raise wasn't flapping at all).
    events.clear();
    step(&mut e, &mut s, 31, "down", &mut events);
    let again = events.iter().find(|e| e.kind == EventKind::Raised).expect("raised again");
    assert_eq!(again.alert.flapping.as_ref().map(|f| f.count), Some(7));
}

/// An alert restored at launch isn't a re-raise: its resolve after the restart (never seen
/// again) and a raise after it start a count of their own, and one that is seen again counts
/// as raised once, not twice.
#[test]
fn an_alert_restored_at_launch_is_not_a_re_raise() {
    let h = Health::new(None);
    let mut s = healthy();
    s.demo = false;
    s.nodes[3].disks[0].used = 3440 * GIB;
    for _ in 0..2 {
        s.ts_ms += 1000;
        h.evaluate(&s);
    }
    let saved = h.engine.lock().unwrap().saved();
    assert_eq!(saved.len(), 1, "disk-full fired");
    // A restart: the alert comes back from disk, already flapping in the last run.
    let restored = |s: &ClusterSnapshot, fired: u64| {
        let mut e = Engine::default();
        let mut saved = saved.clone();
        saved[0].alert.flapping = Some(FlapInfo { count: 4, since_ms: 0 });
        saved[0].alert.firing_since_ms = Some(fired);
        e.set_source(false, saved, Vec::new(), s.ts_ms);
        e
    };
    // Gone by the time its data is in: it resolves, and a raise five minutes later isn't flapping.
    s.ts_ms += 600_000;
    let mut e = restored(&s, s.ts_ms - 120_000);
    let mut ev = run(&mut e, &mut s, 65, |s, _| s.nodes[3].disks[0].used = 1000 * GIB);
    assert_eq!(ev.iter().map(|e| e.kind).collect::<Vec<_>>(), [EventKind::Resolved]);
    assert!(ev[0].alert.flapping.is_none(), "its flaps were the last run's");
    ev = run(&mut e, &mut s, 300, |s, _| s.nodes[3].disks[0].used = 3440 * GIB);
    assert!(ev.iter().filter(|e| e.kind == EventKind::Raised).all(|e| e.alert.flapping.is_none()), "{ev:?}");
    // Seen again at once: raised once. Two real re-raises later it's flapping, counting it: 3.
    s.ts_ms += 3_600_000;
    let mut e = restored(&s, s.ts_ms - 60_000);
    let mut flaps = Vec::new();
    run(&mut e, &mut s, 2, |s, _| s.nodes[3].disks[0].used = 3440 * GIB);
    for _ in 0..2 {
        flaps.extend(run(&mut e, &mut s, 2, |s, _| s.nodes[3].disks[0].used = 1000 * GIB));
        flaps.extend(run(&mut e, &mut s, 60, |s, _| s.nodes[3].disks[0].used = 3440 * GIB));
    }
    let raised: Vec<Option<u32>> = flaps.iter().filter(|e| e.kind == EventKind::Raised).map(|e| e.alert.flapping.as_ref().map(|f| f.count)).collect();
    assert_eq!(raised, [None, Some(3)], "{flaps:?}");
}

#[test]
fn every_rule_has_a_short_label_that_fits() {
    for r in RuleSet::defaults().rules {
        assert!(!r.short.is_empty(), "{} has no short label", r.id);
        assert!(r.short.chars().count() <= super::rule::SHORT_MAX, "{}: \"{}\" is over {} characters", r.id, r.short, super::rule::SHORT_MAX);
    }
    // Alerts carry it.
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run(&mut e, &mut s, 31, |s, _| s.nodes[2].status = LinkStatus::Offline);
    assert_eq!(ev[0].alert.short, "Offline");
    let mut s = healthy();
    let until = s.ts_ms + 600_000;
    maint(&mut s, 2, Some(until));
    let mut e = Engine::default();
    let ev = run(&mut e, &mut s, 31, |s, _| s.nodes[2].status = LinkStatus::Offline);
    assert_eq!(ev[0].alert.short, "Restarting (planned)");
}

#[test]
fn two_silence_calls_make_one_silence() {
    let h = Health::new(None);
    let mut s = healthy();
    s.ts_ms = crate::history::now_ms();
    s.nodes[0].failed_units = vec!["nvidia-dgx-telemetry.service".into()];
    h.evaluate(&s);
    let a = h.view().alerts.into_iter().find(|a| a.rule == "unit-failed").expect("firing");
    let incident = silence::Matcher { incident: a.incident_id.clone(), ..Default::default() };
    // A double click: two calls, one silence, the same id.
    let (_, first) = h.silence(incident.clone(), "1h", "", "nvidia-dgx-telemetry on cell1").unwrap();
    let (v, second) = h.silence(incident.clone(), "1d", "replacing the unit", "nvidia-dgx-telemetry on cell1").unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(v.silences.len(), 1);
    assert_eq!((second.duration.as_str(), second.reason.as_str()), ("1d", "replacing the unit"), "the second call replaces the first");
    // The same alert silenced by rule and entity is a different matcher; ending one ends both.
    let by_alert = silence::Matcher { rule: Some(a.rule.clone()), entity: Some(silence::entity_key(&a)), incident: None };
    let (v, other) = h.silence(by_alert, "1h", "", "nvidia-dgx-telemetry.service on cell1").unwrap();
    assert_ne!(other.id, first.id);
    assert_eq!(v.silences.len(), 2);
    let (v, ended) = h.unsilence(&first.id).unwrap();
    assert_eq!(ended.id, first.id);
    assert!(v.silences.is_empty(), "End silence ends every silence on it: {:?}", v.silences);
    assert!(v.alerts.iter().find(|x| x.id == a.id).is_some_and(|x| x.silence_id.is_none() && x.counted), "paging resumes");
    assert!(h.unsilence(&other.id).is_err(), "already ended");
}

#[test]
fn an_ordered_model_restart_makes_its_alerts_planned() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.ts_ms = 4_100_000_000_000; // far ahead: the windows are process-wide
    let ep = |health: &str| vec![EndpointInfo { id: "cell1-glm-8000".into(), label: "glm-5.3-flash on cell1–cell3".into(), model: "glm-5.3-flash".into(), head_node: "cell1".into(), health: health.into(), nodes: vec!["cell1".into(), "cell2".into(), "cell3".into()], containers: vec!["vllm-head".into()], ..Default::default() }];
    super::maintenance::begin_endpoint_at(true, "cell1-glm-8000", &["cell1".into(), "cell2".into(), "cell3".into()], None, "restarting", s.ts_ms);
    let mut events = Vec::new();
    for _ in 0..40 {
        s.ts_ms += 1000;
        events.extend(e.step_with(&s, &ep("down"), s.ts_ms));
    }
    let down = events.iter().find(|e| e.alert.rule == "endpoint-down").expect("it's down while it restarts");
    assert!(down.alert.planned.as_deref().is_some_and(|p| p.starts_with("glm-5.3-flash restarting (SparkBox, ")), "{:?}", down.alert.planned);
    let v = e.view(s.ts_ms, Some(&s));
    assert_eq!((v.summary.crit, v.summary.planned), (0, 1), "planned, not counted");
    // It answers: the window closes.
    let mut snap = s.clone();
    super::maintenance::stamp(&mut snap, &ep("ready"), 120_000);
    assert!(super::maintenance::endpoint_windows(true).get("cell1-glm-8000").is_none());
}

#[test]
fn a_durable_rolling_hold_keeps_endpoint_alerts_planned_without_a_timer() {
    let mut e = Engine::default();
    let mut s = healthy();
    let ep = EndpointInfo { id: "rolling-held".into(), label: "Held model".into(), head_node: "cell1".into(), health: "down".into(), nodes: vec!["cell1".into()], maintenance_hold: true, ..Default::default() };
    let mut events = Vec::new();
    for _ in 0..40 {
        s.ts_ms += 1000;
        events.extend(e.step_with(&s, std::slice::from_ref(&ep), s.ts_ms));
    }
    let down = events.iter().find(|e| e.alert.rule == "endpoint-down").expect("the held endpoint is observed down");
    assert!(down.alert.planned.as_deref().is_some_and(|p| p.contains("rolling maintenance")));
    assert!(down.alert.muted(down.ts_ms));
    assert_eq!(e.view(s.ts_ms, Some(&s)).summary.crit, 0);
}

#[test]
fn both_halves_of_one_node_are_one_fabric_incident() {
    let mut e = Engine::default();
    let mut s = healthy();
    for n in s.nodes.iter_mut() {
        let mut half2 = n.nets[0].clone();
        half2.iface = "enP2p1s0f1np1".into();
        n.nets.push(half2);
    }
    run(&mut e, &mut s, 2, |_, _| {});
    // Both halves of cell2 drop at once (one optic behind port 2), and come back.
    run(&mut e, &mut s, 2, |s, _| {
        for x in s.nodes[1].nets.iter_mut().filter(|x| x.role == NetRole::Fabric) {
            x.carrier_changes = Some(3);
            x.up = false;
        }
    });
    run(&mut e, &mut s, 31, |s, _| {
        for x in s.nodes[1].nets.iter_mut().filter(|x| x.role == NetRole::Fabric) {
            x.carrier_changes = Some(4);
            x.up = true;
        }
    });
    let v = e.view(s.ts_ms, Some(&s));
    let flaps: Vec<&Alert> = v.alerts.iter().filter(|a| a.rule == "fabric-flap").collect();
    assert_eq!(flaps.len(), 2, "one flap per half");
    assert!(flaps.iter().all(|a| a.incident_id.as_deref() == Some("node:cell2")));
    let i = v.incidents.iter().find(|i| i.id == "node:cell2").expect("one incident");
    assert_eq!((i.title.as_str(), i.affects.as_str()), ("cell2 fabric (both halves)", "cell2 fabric"));
    assert_eq!(v.incidents.iter().filter(|i| i.counted).count(), 1);
}

#[test]
fn memory_pressure_warns_past_5_percent_and_is_critical_past_20() {
    let mut e = Engine::default();
    let mut s = healthy();
    // No PSI from the node: nothing to judge.
    assert!(run(&mut e, &mut s, 40, |_, _| {}).is_empty());
    let ev = run(&mut e, &mut s, 31, |s, _| s.nodes[2].mem.psi_full10 = Some(7.5));
    let a = &ev.iter().find(|e| e.alert.rule == "mem-pressure").expect("after 30 s").alert;
    assert_eq!((a.severity, a.message.as_str(), a.short.as_str()), (Severity::Warn, "Every task waited on memory 8% of the last 10 s", "Memory pressure"));
    let ev = run(&mut e, &mut s, 31, |s, _| s.nodes[2].mem.psi_full10 = Some(24.0));
    assert!(ev.iter().any(|e| e.kind == EventKind::Escalated && e.alert.rule == "mem-pressure"), "{ev:?}");
}

/// Demo's "As found" world (the cluster as the Live tour found it on Oct 29) is as quiet as the
/// real one: idle GPUs at their power cap, two idle Ollamas, the failed glm53 run, the recabling
/// in the kernel log and PFC off raise nothing; Fabric has its one advisory; Events, info rows only.
#[test]
fn the_as_found_world_raises_nothing() {
    let mut sim = Sim::new(Some("as-found"));
    let mut events = Vec::new();
    let mut snap = sim.demo.snapshot();
    for t in 0..600u64 {
        sim.demo.advance();
        snap = sim.demo.snapshot();
        snap.ts_ms = sim.base + t * 1000;
        let tables = [("crs804".to_string(), sim.demo.switch_tables("crs804"))].into();
        let world = crate::fabric::demo_world_cabling(&snap);
        sim.fabric = Some(sim.mon.assess(&mut snap, &tables, Some(&world)));
        sim.engine.set_rates(sim.mon.rates());
        super::kernel::stamp(&mut snap);
        let endpoints = idle_ollamas(if t % 97 < 3 { "gone" } else { "idle" });
        events.extend(sim.engine.step_with(&snap, &endpoints, snap.ts_ms));
    }
    assert!(events.is_empty(), "{:?}", events.iter().map(|e| (e.kind, e.alert.summary.clone())).collect::<Vec<_>>());
    assert!(snap.nodes.iter().any(|n| n.gpu.throttle.iter().any(|r| r == "SW power cap")), "the idle GPUs report their power cap");
    let fabric = sim.fabric.expect("assessed");
    let advisory = fabric.advisory.expect("PFC off is an advisory");
    assert_eq!((advisory.short.as_str(), advisory.title.as_str()), ("PFC protection unverified", "Fabric PFC needs verification."));
    assert!(advisory.body.starts_with("PFC protection is absent or unknown on all 8 fabric ports (trust pcp, node-side evidence only)."), "{}", advisory.body);
    assert!(!matches!(fabric.verdict.level, crate::fabric::Level::Warn | crate::fabric::Level::Crit), "{:?}", fabric.verdict);
    // The kernel log's ConnectX-7 lines are recabling: link rows, information only.
    let names = snap.nodes.iter().map(|n| (n.id.clone(), n.name.clone())).collect();
    let decoded: Vec<_> = snap.nodes.iter().flat_map(|n| n.kernel_events.iter().map(|e| super::kernel::decode(&n.id, e))).collect();
    assert!(!decoded.is_empty(), "the as-found world has the real kernel lines");
    let rows = super::kernel::rows(decoded, &names);
    assert!(rows.iter().all(|r| r.severity == "info" && r.kind == "link"), "{:?}", rows.iter().map(|r| &r.title).collect::<Vec<_>>());
    assert!(rows.iter().any(|r| r.title.starts_with("42 link events on 4 nodes")), "{:?}", rows.iter().map(|r| &r.title).collect::<Vec<_>>());
}

// ---- r1-h: this Mac's path to the cluster (P1.10), flapping and damping (P1.44), silences (P1.45)

/// The probes of a Mac whose router (192.168.50.1, on en0) answers, with the Switch X set up.
fn router_up() -> reach::Probes {
    reach::Probes {
        read: true,
        route: Some(reach::Route { gateway: Some("192.168.50.1".into()), interface: Some("en0".into()) }),
        gateway_up: Some(true),
        gateway_ms: Some(2.1),
        gateway_answers: true,
        vpn: Some("off".into()),
        switch_x_set_up: true,
        ..Default::default()
    }
}

/// An alert event about a node, for driving the gate directly.
fn gate_event(kind: EventKind, rule: &str, severity: Severity, node: &str, summary: &str) -> AlertEvent {
    let c = Condition {
        key: format!("{rule}:node:{node}"),
        rule: rule.into(),
        category: if rule.starts_with("endpoint") { Category::Containers } else { Category::Node },
        severity,
        entity: Entity { kind: "node".into(), id: node.into(), node: Some(node.into()), switch: None, label: node.into() },
        message: String::new(),
        summary: summary.into(),
        detail: None,
        value: None,
        for_ms: 0,
        clear_ms: 0,
        notify: true,
        cause: None,
        short: None,
    };
    let a = Alert { state: AlertState::Firing, incident_id: Some(format!("node:{node}")), ..Alert::from(&c, 0) };
    AlertEvent::new(0, kind, a)
}

/// All four nodes timing out, as on Oct 29 at 17:32 (status Error as round 4 had it, or Offline
/// as R1-N maps a timeout), with their retries (Connecting) between.
fn all_timing_out(s: &mut ClusterSnapshot, t: u64, status: LinkStatus) {
    for n in s.nodes.iter_mut() {
        n.status = if t % 12 < 2 { LinkStatus::Connecting } else { status };
        n.error = Some(format!("SSH timeout: no reply from {} in 10 s. The node may be off or restarting, or this Mac isn't on its network.", n.host));
        n.error_kind = Some("timeout".into());
        n.latency_ms = None;
    }
}

/// Every word an alert or incident shows: messages, summaries, details, short labels, titles.
fn texts(v: &HealthView) -> Vec<String> {
    let mut out = Vec::new();
    for a in &v.alerts {
        out.extend([a.message.clone(), a.summary.clone(), a.short.clone()]);
        out.extend(a.detail.clone());
    }
    for i in &v.incidents {
        out.extend([i.title.clone(), i.affects.clone()]);
        out.extend(i.cause.clone());
    }
    out
}

/// Live snapshots through Health with this Mac's verdict stamped and the gate run on what they
/// raise (decide, the 30 s repeat timer, the group window), as on_publish and notify::start do.
struct Live {
    h: Health,
    s: ClusterSnapshot,
    gate: crate::notify::Gate,
    settings: crate::notify::NotifySettings,
    events: Vec<AlertEvent>,
    sent: Vec<crate::notify::Notice>,
}

impl Live {
    fn new() -> Live {
        let mut s = healthy();
        s.demo = false;
        let h = Health::new(None);
        h.reach.set_probes(router_up());
        Live { h, s, gate: crate::notify::Gate::default(), settings: crate::notify::NotifySettings { repeat_minutes: 15, ..Default::default() }, events: Vec::new(), sent: Vec::new() }
    }

    fn run(&mut self, secs: u64, mut f: impl FnMut(&mut ClusterSnapshot, u64)) {
        for t in 0..secs {
            self.s.ts_ms += 1000;
            f(&mut self.s, t);
            self.h.reach.stamp(&mut self.s);
            let ev = self.h.evaluate(&self.s).events;
            let now = self.s.ts_ms;
            self.sent.extend(crate::notify::decide_events(&mut self.gate, &ev, &self.settings, now).1);
            if (now / 1000).is_multiple_of(30) {
                self.gate.sync(&crate::notify::firing_in(&self.h.view(), now));
                self.gate.tick(&self.settings.gate(), now);
            }
            self.sent.extend(self.gate.flush(now));
            self.events.extend(ev);
        }
    }

    /// Messages that page (not the clears).
    fn pages(&self) -> Vec<&str> {
        self.sent.iter().filter(|n| !n.resolved).map(|n| n.title.as_str()).collect()
    }
}

#[test]
fn four_nodes_timing_out_with_the_router_up_are_one_mac_incident_and_at_most_one_page() {
    for status in [LinkStatus::Error, LinkStatus::Offline] {
        for paging in [false, true] {
            let mut l = Live::new();
            if paging {
                // The owner turned mac-reach's paging on (rules.json keeps it).
                let mut rules = RuleSet::defaults();
                rules.rules.iter_mut().find(|r| r.id == "mac-reach").unwrap().notify = true;
                l.h.set_rules(rules).unwrap();
            }
            l.run(5, |_, _| {});
            let mut seen_texts = Vec::new();
            for _ in 0..6 {
                l.run(30, |s, t| all_timing_out(s, t, status));
                seen_texts.extend(texts(&l.h.view()));
            }
            let v = l.h.view();
            let shown: Vec<(&str, IncidentState)> = v.incidents.iter().map(|i| (i.id.as_str(), i.state)).collect();
            assert_eq!(shown, [(incident::MAC_ROOT, IncidentState::Firing)], "{status:?}: one incident");
            let i = &v.incidents[0];
            assert_eq!(i.title, "This Mac can't reach any node; the LAN between them?");
            assert_eq!(i.affects, "This Mac · 4 nodes not confirmed down");
            assert!(i.cause.as_deref().unwrap().starts_with("The router (192.168.50.1) answers, but no node does over the wired LAN."), "{:?}", i.cause);
            assert_eq!((v.summary.crit, v.summary.warn), (0, 1), "a warning, not four criticals");
            let rows: Vec<&Alert> = v.alerts.iter().filter(|a| a.rule == "node-offline").collect();
            assert_eq!(rows.len(), 4);
            assert!(rows.iter().all(|a| a.message == "Offline · not confirmed down" && a.severity == Severity::Info && !a.counted && a.incident_id.as_deref() == Some(incident::MAC_ROOT)), "{rows:?}");
            assert!(v.nodes.values().all(|n| n.level == Level::Ok), "no node painted critical");
            for t in &seen_texts {
                assert!(!t.contains("Error") && !t.to_lowercase().contains("reconnecting"), "{t}");
            }
            let pages: Vec<String> = l.pages().into_iter().map(String::from).collect();
            if paging {
                assert_eq!(pages, ["This Mac can't reach any node; the LAN between them?"], "{status:?}");
            } else {
                assert!(l.sent.is_empty(), "not paged by default: {:?}", l.sent.iter().map(|n| &n.title).collect::<Vec<_>>());
            }
            // The LAN is back: cell1 first, the others within 40 s. No node is paged on the way.
            let back_at = [0, 10, 25, 40];
            l.run(60, |s, t| {
                for (i, n) in s.nodes.iter_mut().enumerate() {
                    if t >= back_at[i] {
                        (n.status, n.error, n.error_kind) = (LinkStatus::Online, None, None);
                    } else {
                        n.status = if t % 12 < 2 { LinkStatus::Connecting } else { status };
                    }
                }
            });
            assert_eq!(l.pages().len(), pages.len(), "{:?}", l.sent.iter().map(|n| &n.title).collect::<Vec<_>>());
            assert!(!l.events.iter().any(|e| e.alert.rule == "node-offline" && e.alert.severity == Severity::Crit), "no node went critical");
            assert!(l.h.view().alerts.is_empty(), "{:?}", l.h.view().alerts.iter().map(|a| &a.summary).collect::<Vec<_>>());
        }
    }
}

#[test]
fn a_node_that_alone_stops_answering_is_still_paged() {
    // The Mac reaches the others: cell3 alone is cell3's outage.
    let mut l = Live::new();
    l.run(40, |s, _| {
        s.nodes[2].status = LinkStatus::Offline;
        (s.nodes[2].error, s.nodes[2].error_kind) = (Some("No SSH reply in 10 s".into()), Some("timeout".into()));
    });
    assert_eq!(l.s.mac_reach.as_ref().map(|r| r.state), Some(MacReachState::Ok));
    assert_eq!(l.pages(), ["cell3 offline · no SSH reply in 10 s"]);
}

#[test]
fn the_other_roots_name_what_this_mac_lost() {
    let mut cases = Vec::new();
    let no_route = reach::Probes { route: None, ..router_up() };
    cases.push((no_route, MacReachState::NoRoute, "This Mac lost the home network"));
    let router_down = reach::Probes { gateway_up: Some(false), ..router_up() };
    cases.push((router_down, MacReachState::LostHomeNetwork, "This Mac lost the home network"));
    let vpn = reach::Probes { vpn: Some("error".into()), vpn_message: Some("handshake timed out".into()), ..router_up() };
    cases.push((vpn, MacReachState::VpnProblem, "This Mac can't reach the cluster: VPN problem"));
    let wifi = reach::Probes {
        second: vec![reach::PathProbe { label: "cell3's Wi-Fi (wlP9s9, 192.168.50.229)".into(), node: Some("cell3".into()), addr: "192.168.50.229:22".into(), ok: true, error: None, at_ms: 0 }],
        switch_x: Some(reach::PathProbe { label: "Firewalla Switch X".into(), ok: false, error: Some("no answer within 1500 ms".into()), ..Default::default() }),
        ..router_up()
    };
    cases.push((wifi, MacReachState::WiredSwitch, "Only cell3's Wi-Fi answers: wired LAN switch?"));
    for (probes, state, title) in cases {
        let mut l = Live::new();
        l.h.reach.set_probes(probes);
        l.run(60, |s, t| all_timing_out(s, t, LinkStatus::Offline));
        let v = l.h.view();
        assert_eq!(l.s.mac_reach.as_ref().map(|r| r.state), Some(state));
        assert_eq!(v.incidents.iter().map(|i| (i.id.as_str(), i.title.as_str())).collect::<Vec<_>>(), [(incident::MAC_ROOT, title)], "{state:?}");
        assert!(v.alerts.iter().filter(|a| a.rule == "node-offline").all(|a| a.message == "Offline · not confirmed down"));
        assert!(l.sent.is_empty(), "{state:?}");
    }
}

#[test]
fn a_router_that_never_answers_ping_says_nothing() {
    // Some routers drop ICMP: its silence isn't "lost the home network", and with no node
    // answering it's still the LAN between.
    let mut l = Live::new();
    l.h.reach.set_probes(reach::Probes { gateway_up: Some(false), gateway_answers: false, ..router_up() });
    l.run(40, |s, t| all_timing_out(s, t, LinkStatus::Offline));
    assert_eq!(l.s.mac_reach.as_ref().map(|r| r.state), Some(MacReachState::LanBetween));
}

#[test]
fn a_host_key_to_trust_or_a_refused_password_needs_you_and_isnt_an_outage() {
    for (kind, error, summary) in [
        ("hostKeyUnknown", "SparkBox hasn't connected to cell2 before. Its host key is SHA256:abc; trust it in Settings › Nodes.", "cell2's host key isn't trusted yet · Trust it"),
        ("authFailed", "cell2 rejected the saved password for sparkuser.", "cell2 refused the saved password"),
    ] {
        let mut l = Live::new();
        l.run(120, |s, _| {
            s.nodes[1].status = LinkStatus::Error;
            (s.nodes[1].error, s.nodes[1].error_kind) = (Some(error.into()), Some(kind.into()));
        });
        let v = l.h.view();
        assert!(!v.alerts.iter().any(|a| a.rule == "node-offline"), "{kind}: not an outage");
        let a = v.alerts.iter().find(|a| a.rule == "node-needs-you").expect(kind);
        assert_eq!((a.summary.as_str(), a.severity, a.counted, a.notify), (summary, Severity::Info, false, false));
        assert_eq!(a.incident_id.as_deref(), Some("node:cell2"));
        assert_eq!((v.summary.crit, v.summary.warn, v.summary.info), (0, 0, 1));
        assert!(l.events.iter().any(|e| e.alert.rule == "node-needs-you" && e.kind == EventKind::Raised));
        assert!(l.sent.is_empty(), "{kind}: never paged");
        assert_eq!(l.s.mac_reach.as_ref().map(|r| r.state), Some(MacReachState::Ok), "the node answered: the path works");
    }
}

#[test]
fn after_a_sleep_the_nodes_get_a_minute_to_reconnect() {
    let mut l = Live::new();
    l.run(5, |_, _| {});
    // The Mac slept: the monitor saw the wall clock jump. All four reconnect within 20 s, cell3 in 45.
    l.h.reach.woke(l.s.ts_ms);
    l.run(45, |s, t| {
        for (i, n) in s.nodes.iter_mut().enumerate() {
            let away = if i == 2 { 45 } else { 20 };
            if t < away {
                n.status = if t % 12 < 2 { LinkStatus::Connecting } else { LinkStatus::Offline };
                (n.error, n.error_kind) = (Some("No SSH reply in 10 s".into()), Some("timeout".into()));
            } else {
                (n.status, n.error, n.error_kind) = (LinkStatus::Online, None, None);
            }
        }
        // (What the last step stamped: the first is from before the sleep.)
        assert_eq!(s.mac_reach.as_ref().map(|r| r.state), Some(if t == 0 { MacReachState::Ok } else { MacReachState::Waking }));
    });
    l.run(1, |s, _| (s.nodes[2].status, s.nodes[2].error, s.nodes[2].error_kind) = (LinkStatus::Online, None, None));
    assert!(l.events.is_empty(), "no node-offline (nor anything else) in the grace: {:?}", l.events.iter().map(|e| &e.alert.summary).collect::<Vec<_>>());
    assert!(l.h.view().alerts.is_empty());
    // After the minute, a node that doesn't answer is raised as usual.
    l.run(20, |_, _| {});
    l.run(31, |s, _| {
        s.nodes[2].status = LinkStatus::Offline;
        (s.nodes[2].error, s.nodes[2].error_kind) = (Some("No SSH reply in 10 s".into()), Some("timeout".into()));
    });
    assert_eq!(l.pages(), ["cell3 offline · no SSH reply in 10 s"]);
}

#[test]
fn a_node_back_with_a_new_boot_id_rebooted() {
    let mut l = Live::new();
    for n in l.s.nodes.iter_mut() {
        n.boot_id = Some(format!("{}-boot-1", n.id));
    }
    l.run(5, |_, _| {});
    let away = |s: &mut ClusterSnapshot, _: u64| {
        s.nodes[2].status = LinkStatus::Offline;
        (s.nodes[2].error, s.nodes[2].error_kind) = (Some("No SSH reply in 10 s".into()), Some("timeout".into()));
    };
    // Away 40 s, back on the same boot: "back online".
    l.run(40, away);
    l.run(2, |s, _| (s.nodes[2].status, s.nodes[2].error, s.nodes[2].error_kind) = (LinkStatus::Online, None, None));
    // Away again, back with a new boot_id (cell3 at 17:45 and 18:08 on Oct 29): "it rebooted".
    l.run(40, away);
    l.run(2, |s, _| {
        (s.nodes[2].status, s.nodes[2].error, s.nodes[2].error_kind) = (LinkStatus::Online, None, None);
        s.nodes[2].boot_id = Some("cell3-boot-2".into());
    });
    let resolved: Vec<Option<&str>> = l.events.iter().filter(|e| e.kind == EventKind::Resolved).map(|e| e.alert.reason.as_deref()).collect();
    assert_eq!(resolved, [None, Some(REBOOTED)]);
    let titles: Vec<&str> = l.sent.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, ["cell3 offline · no SSH reply in 10 s", "cell3 is back online", "cell3 offline · no SSH reply in 10 s", "cell3 is back · it rebooted"]);
    // A boot_id first seen (SparkBox just started) isn't a reboot.
    let mut l = Live::new();
    l.run(40, away);
    l.run(2, |s, _| {
        (s.nodes[2].status, s.nodes[2].error, s.nodes[2].error_kind) = (LinkStatus::Online, None, None);
        s.nodes[2].boot_id = Some("cell3-boot-1".into());
    });
    assert_eq!(l.sent.last().map(|n| n.title.as_str()), Some("cell3 is back online"));
}

#[test]
fn no_alert_ever_says_reconnecting() {
    // Every message live/mod.rs leaves on a node that went away, trying again, timing out, closed.
    let errors = ["Connection closed; reconnecting…", "No reply from cell3 in 10 s; reconnecting…", "Reconnecting", "SSH handshake with 192.168.50.98 timed out", "No SSH reply in 10 s"];
    for probes in [router_up(), reach::Probes { route: None, ..router_up() }] {
        let mut l = Live::new();
        l.h.reach.set_probes(probes.clone());
        let mut seen = Vec::new();
        for (i, e) in errors.iter().enumerate() {
            l.run(45, |s, t| {
                for n in s.nodes.iter_mut().filter(|n| probes.route.is_none() || n.id == "cell3") {
                    n.status = match t % 9 {
                        0..=2 => LinkStatus::Connecting,
                        3..=5 => LinkStatus::Offline,
                        _ => LinkStatus::Error,
                    };
                    (n.error, n.error_kind) = (Some(e.to_string()), if i % 2 == 0 { None } else { Some("timeout".into()) });
                }
            });
            seen.extend(texts(&l.h.view()));
        }
        // Trying again: the row keeps "Offline · trying again" (or, while this Mac can't reach
        // the cluster, "not confirmed down").
        l.run(1, |s, _| {
            for n in s.nodes.iter_mut().filter(|n| n.status != LinkStatus::Online) {
                (n.status, n.error_kind) = (LinkStatus::Connecting, Some("timeout".into()));
            }
        });
        let row = l.h.view().alerts.into_iter().find(|a| a.rule == "node-offline" && a.entity.id == "cell3").unwrap();
        assert_eq!(row.message, if probes.route.is_some() { "Offline · trying again" } else { "Offline · not confirmed down" });
        seen.extend(texts(&l.h.view()));
        seen.extend(l.events.iter().flat_map(|e| [e.alert.message.clone(), e.alert.summary.clone(), e.alert.detail.clone().unwrap_or_default(), e.incident_title.clone().unwrap_or_default()]));
        seen.extend(l.sent.iter().flat_map(|n| [n.title.clone(), n.body.clone()]));
        assert!(!seen.is_empty());
        for t in &seen {
            assert!(!t.to_lowercase().contains("reconnecting"), "{t}");
        }
    }
}

#[test]
fn a_models_title_is_what_the_ui_calls_it() {
    let ep = |label: &str, engine: &str| super::endpoint_info(&serde_json::json!({"id": "gx10-0000-ollama-11434", "engine": engine, "models": [], "headNode": "gx10-0000", "nodes": ["gx10-0000"], "health": "down", "label": label}));
    let mut e = Engine::default();
    let mut s = healthy();
    let mut ev = Vec::new();
    for _ in 0..31 {
        s.ts_ms += 1000;
        ev.extend(e.step_with(&s, &[ep("Ollama on gx10-0000", "ollama")], s.ts_ms));
    }
    let a = &ev.iter().find(|e| e.kind == EventKind::Raised).unwrap().alert;
    assert_eq!(a.summary, "Ollama on gx10-0000 is down: it isn't answering requests", "not \"ollama is down\"");
    assert_eq!(a.entity.label, "Ollama on gx10-0000");
    // A served name keeps its nodes in what it affects.
    let info = super::endpoint_info(&serde_json::json!({"id": "x", "models": ["/models/glm-5.3"], "headNode": "cell1", "nodes": ["cell1", "cell2"], "label": "glm-5.3"}));
    assert_eq!((info.name.as_str(), info.label.as_str()), ("glm-5.3", "glm-5.3 on cell1–cell2"));
}

#[test]
fn a_silence_wider_than_one_alert_or_until_it_clears_needs_mac_scope_and_a_reason() {
    use silence::{make, needs_mac, Matcher};
    let one = Matcher { rule: Some("node-offline".into()), entity: Some("node::cell3".into()), incident: None };
    let rule_wide = Matcher { rule: Some("node-offline".into()), ..Default::default() };
    let entity_wide = Matcher { entity: Some("node::cell3".into()), ..Default::default() };
    let incident = Matcher { incident: Some("node:cell3".into()), ..Default::default() };
    // Node scope: one alert or one incident, for an hour or a day.
    for m in [&one, &incident] {
        for d in ["1h", "1d"] {
            assert!(!needs_mac(m, d), "{m:?} {d}");
            assert!(make(m.clone(), d, "", "x", 0, false).is_ok());
        }
        for d in ["resolved", "1w", "never"] {
            assert!(needs_mac(m, d), "{m:?} {d}");
        }
    }
    // Everything else: Mac scope, and refused without a reason.
    for m in [&rule_wide, &entity_wide] {
        for d in silence::DURATIONS {
            assert!(needs_mac(m, d), "{m:?} {d}");
            assert!(make(m.clone(), d, " ", "x", 0, false).unwrap_err().starts_with("Say why"), "{m:?} {d}");
            assert!(make(m.clone(), d, "moving the rack", "x", 0, false).is_ok());
        }
    }
    assert_eq!(make(rule_wide.clone(), "resolved", "", "x", 0, false).unwrap_err(), "Say why: a silence until it clears needs a reason.");
    assert_eq!(make(rule_wide.clone(), "1h", "", "x", 0, false).unwrap_err(), "Say why: a silence of more than one alert needs a reason.");
    // The engine side (Health::silence, which alert_silence calls after its gate) refuses too.
    let h = Health::new(None);
    assert!(h.silence(rule_wide, "resolved", "", "Every node offline").unwrap_err().contains("needs a reason"));
    assert_eq!(silence::duration_words("resolved"), "until it clears (at most 7 days)");
}

#[test]
fn until_it_clears_ends_when_it_clears_or_after_seven_days() {
    let mut e = Engine::default();
    let mut s = healthy();
    run(&mut e, &mut s, 1, |s, _| s.nodes[3].disks[0].used = 3440 * GIB);
    let a = e.view(s.ts_ms, Some(&s)).alerts[0].clone();
    let m = silence::Matcher { incident: a.incident_id.clone(), ..Default::default() };
    let sil = silence::make(m, "resolved", "expanding the disk Friday", "Root disk on gx10-0000", s.ts_ms, true).unwrap();
    assert_eq!((sil.max_ms, sil.ends_ms()), (Some(s.ts_ms + 7 * 86_400_000), Some(s.ts_ms + 7 * 86_400_000)));
    e.add_silence(sil, s.ts_ms);
    assert_eq!(e.view(s.ts_ms, Some(&s)).summary.warn, 0, "silenced");
    // Six days later it still holds; at seven it ends, though the disk is still full.
    s.ts_ms += 6 * 86_400_000;
    assert!(run(&mut e, &mut s, 1, |_, _| {}).is_empty());
    assert_eq!(e.view(s.ts_ms, Some(&s)).silences.len(), 1);
    s.ts_ms += 86_400_000;
    let ev = run(&mut e, &mut s, 1, |_, _| {});
    assert_eq!(ev.iter().map(|e| e.kind).collect::<Vec<_>>(), [EventKind::SilenceEnded]);
    let v = e.view(s.ts_ms, Some(&s));
    assert!(v.silences.is_empty());
    assert_eq!(v.summary.warn, 1, "it counts (and pages) again");
    // One that clears first ends then.
    let m = silence::Matcher { incident: a.incident_id.clone(), ..Default::default() };
    e.add_silence(silence::make(m, "resolved", "expanding the disk Friday", "Root disk on gx10-0000", s.ts_ms, true).unwrap(), s.ts_ms);
    run(&mut e, &mut s, 1, |s, _| s.nodes[3].disks[0].used = 1000 * GIB);
    assert!(e.view(s.ts_ms, Some(&s)).silences.is_empty());
    // A silence saved before the cap (no maxMs) ends seven days after it was made.
    let old: silence::Silence = serde_json::from_value(serde_json::json!({"id": "s", "matcher": {"rule": "disk-full"}, "until": {"kind": "resolved"}, "reason": "", "createdMs": 1000, "createdBy": "Admin on this Mac", "source": "live"})).unwrap();
    assert_eq!(old.ends_ms(), Some(1000 + 7 * 86_400_000));
    assert!(!old.expired(7 * 86_400_000) && old.expired(1000 + 7 * 86_400_000));
}

/// The node rows of this Mac's reach problem: (severity, message), in node order.
fn node_rows(v: &HealthView) -> Vec<(Severity, String)> {
    v.alerts.iter().filter(|a| a.rule == "node-offline").map(|a| (a.severity, a.message.clone())).collect()
}

fn not_confirmed_down() -> Vec<(Severity, String)> {
    vec![(Severity::Info, "Offline · not confirmed down".to_string()); 4]
}

/// What live/mod.rs leaves on every node after a wake or at a start: Connecting with no error
/// kind ("Reconnecting…", or nothing yet) for 10 s, then the reconnects time out (Offline 2 s,
/// Connecting 10 s, kind kept).
fn reconnecting_into_the_outage(s: &mut ClusterSnapshot, t: u64, kick: Option<&str>) {
    for n in s.nodes.iter_mut() {
        if t < 10 {
            (n.status, n.error, n.error_kind) = (LinkStatus::Connecting, kick.map(String::from), None);
        } else {
            n.status = if (t - 10) % 12 < 2 { LinkStatus::Offline } else { LinkStatus::Connecting };
            (n.error, n.error_kind) = (Some("No SSH reply in 10 s".into()), Some("timeout".into()));
        }
    }
}

#[test]
fn a_sleep_in_the_middle_of_a_mac_outage_pages_nobody() {
    for probes in [router_up(), reach::Probes { route: None, ..router_up() }] {
        let mut l = Live::new();
        l.h.reach.set_probes(probes.clone());
        l.run(5, |_, _| {});
        l.run(180, |s, t| all_timing_out(s, t, LinkStatus::Offline));
        let state = l.s.mac_reach.as_ref().map(|r| r.state);
        assert!(state.is_some_and(reach::problem), "{state:?}");
        assert_eq!(node_rows(&l.h.view()), not_confirmed_down());
        l.events.clear();
        l.sent.clear();
        // The lid closes and opens: the monitor sees the wall clock jump, live kicks every
        // session ("Reconnecting…", no kind), then the reconnects time out again.
        l.h.reach.woke(l.s.ts_ms);
        l.run(90, |s, t| {
            reconnecting_into_the_outage(s, t, Some("Reconnecting…"));
            let r = s.mac_reach.as_ref().unwrap();
            assert_eq!(Some(r.state), state, "the problem stands through the wake grace");
            assert!(!r.detail.contains("answering again"), "no node answers: {}", r.detail);
        });
        let events: Vec<String> = l.events.iter().map(|e| format!("{:?} {} {:?} {}", e.kind, e.alert.rule, e.alert.severity, e.alert.summary)).collect();
        assert!(events.is_empty(), "{state:?}: no escalation, and mac-reach neither resolves nor comes back: {events:?}");
        assert!(l.sent.is_empty(), "{:?}", l.sent.iter().map(|n| &n.title).collect::<Vec<_>>());
        assert_eq!(node_rows(&l.h.view()), not_confirmed_down());
    }
}

#[test]
fn waking_at_home_after_a_mac_outage_it_ends_when_the_last_node_is_back() {
    let mut l = Live::new();
    l.run(5, |_, _| {});
    l.run(60, |s, t| all_timing_out(s, t, LinkStatus::Offline));
    l.events.clear();
    // The network is there on waking: cell1 is back at once, the others' reconnects take 20 s.
    l.h.reach.woke(l.s.ts_ms);
    l.run(40, |s, t| {
        for (i, n) in s.nodes.iter_mut().enumerate() {
            if i == 0 || t >= 20 {
                (n.status, n.error, n.error_kind) = (LinkStatus::Online, None, None);
            } else {
                (n.status, n.error, n.error_kind) = (LinkStatus::Connecting, Some("Reconnecting…".into()), None);
            }
        }
    });
    let at = |rule: &str| l.events.iter().filter(|e| e.alert.rule == rule && e.kind == EventKind::Resolved).map(|e| e.ts_ms).max().unwrap();
    // One incident to the end: the rows don't outlive the problem they're part of.
    assert!(at("mac-reach") > at("node-offline"), "{:?}", l.events.iter().map(|e| (e.kind, e.alert.rule.as_str(), e.ts_ms)).collect::<Vec<_>>());
    assert!(l.events.iter().all(|e| e.kind == EventKind::Resolved));
    assert!(l.sent.is_empty() && l.h.view().alerts.is_empty());
}

#[test]
fn a_restart_of_sparkbox_in_the_middle_of_a_mac_outage_pages_nobody() {
    // How the second run starts: the path monitor's first round 5 s in; its first round after the
    // first connects time out (a slow probe of a router that drops pings); a switch to Demo and
    // back to Live in the same run.
    for how in ["restart", "slow first probe", "demo and back"] {
        let dir = std::env::temp_dir().join(format!("sb-health-restart-{}-{}", std::process::id(), fastrand::u32(..)));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = healthy();
        s.demo = false;
        let first = Health::new(Some(dir.clone()));
        first.reach.set_probes(router_up());
        for t in 0..200 {
            s.ts_ms += 1000;
            all_timing_out(&mut s, t, LinkStatus::Offline);
            first.reach.stamp(&mut s);
            first.evaluate(&s);
        }
        let v = first.view();
        assert_eq!(v.alerts.iter().filter(|a| a.state == AlertState::Firing).map(|a| (a.rule.as_str(), a.severity)).collect::<Vec<_>>(), [("mac-reach", Severity::Warn), ("node-offline", Severity::Info), ("node-offline", Severity::Info), ("node-offline", Severity::Info), ("node-offline", Severity::Info)]);
        let h = if how == "demo and back" {
            // Demo for a while (the monitor clears itself), then Live again.
            let mut demo = healthy();
            demo.ts_ms = s.ts_ms;
            for _ in 0..30 {
                demo.ts_ms += 1000;
                first.reach.stamp(&mut demo);
                first.evaluate(&demo);
            }
            first.reach.clear();
            s.ts_ms = demo.ts_ms;
            first
        } else {
            drop(first);
            Health::new(Some(dir.clone()))
        };
        let probed_at = if how == "slow first probe" { 12 } else { 6 };
        let (mut gate, settings) = (crate::notify::Gate::default(), crate::notify::NotifySettings { repeat_minutes: 15, ..Default::default() });
        let (mut events, mut sent) = (Vec::new(), Vec::new());
        for t in 0..90 {
            s.ts_ms += 1000;
            if t == probed_at {
                h.reach.set_probes(router_up());
            }
            reconnecting_into_the_outage(&mut s, t, None);
            h.reach.stamp(&mut s);
            let ev = h.evaluate(&s).events;
            sent.extend(crate::notify::decide_events(&mut gate, &ev, &settings, s.ts_ms).1);
            sent.extend(gate.flush(s.ts_ms));
            events.extend(ev);
        }
        let shown: Vec<String> = events.iter().map(|e| format!("{:?} {} {:?} {}", e.kind, e.alert.rule, e.alert.severity, e.alert.summary)).collect();
        assert!(events.iter().all(|e| e.kind != EventKind::Escalated && e.alert.severity != Severity::Crit), "{how}: {shown:?}");
        assert!(sent.is_empty(), "{how}: {:?}", sent.iter().map(|n| &n.title).collect::<Vec<_>>());
        let v = h.view();
        assert_eq!(node_rows(&v), not_confirmed_down(), "{how}");
        assert!(v.alerts.iter().any(|a| a.rule == "mac-reach" && a.state == AlertState::Firing), "{how}");
        assert_eq!((v.summary.crit, v.summary.warn), (0, 1), "{how}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_node_still_reconnecting_when_the_mac_outage_ends_isnt_paged() {
    let mut l = Live::new();
    l.run(5, |_, _| {});
    l.run(60, |s, t| all_timing_out(s, t, LinkStatus::Offline));
    assert_eq!(node_rows(&l.h.view()), not_confirmed_down());
    l.events.clear();
    // The LAN is back: three nodes answer at once. gx10-0000's retries time out for most of the
    // minute the problem holds for it; its next try (no kind yet) is under way as the hold ends,
    // and it connects.
    l.run(75, |s, t| {
        for (i, n) in s.nodes.iter_mut().enumerate() {
            if i < 3 || t >= 68 {
                (n.status, n.error, n.error_kind) = (LinkStatus::Online, None, None);
            } else if t >= 58 {
                (n.status, n.error, n.error_kind) = (LinkStatus::Connecting, Some("No reply from gx10-0000 in 10 s; reconnecting…".into()), None);
            } else {
                n.status = if t % 12 < 2 { LinkStatus::Connecting } else { LinkStatus::Offline };
            }
        }
    });
    let shown: Vec<String> = l.events.iter().map(|e| format!("{:?} {} {:?} {}", e.kind, e.alert.rule, e.alert.severity, e.alert.summary)).collect();
    assert!(l.events.iter().all(|e| e.kind == EventKind::Resolved), "{shown:?}");
    assert!(l.sent.is_empty(), "{:?}", l.sent.iter().map(|n| &n.title).collect::<Vec<_>>());
    assert!(l.h.view().alerts.is_empty());
}

/// One node through a LAN-path outage as live/mod.rs moves it, `t` seconds in: frames stop at
/// `stop` (stale after 3 s); the session ends at `ends`, by the stall watch ("No reply from X in
/// 10 s; reconnecting…", Connecting) or by the 15 s pinger ("Connection closed; reconnecting…",
/// Offline 2 s, then Connecting), neither with a kind; the reconnect times out 10 s later ("No
/// SSH reply in 10 s", kind timeout), then retries (Offline 2 s, Connecting 10 s); back at `back`.
fn through_a_lan_outage(n: &mut NodeSnapshot, t: u64, stop: u64, ends: u64, by_pinger: bool, back: u64) {
    if t < stop || t >= back {
        (n.status, n.error, n.error_kind, n.stale, n.frame_age_ms) = (LinkStatus::Online, None, None, false, Some(500));
    } else if t < ends {
        let age = (t - stop) * 1000;
        (n.status, n.error, n.error_kind, n.stale, n.frame_age_ms) = (LinkStatus::Online, None, None, age > 3000, Some(age));
    } else {
        (n.stale, n.frame_age_ms) = (false, None);
        let timed_out = ends + if by_pinger { 12 } else { 10 };
        if t < timed_out {
            let (status, error) = match by_pinger {
                true => (if t < ends + 2 { LinkStatus::Offline } else { LinkStatus::Connecting }, "Connection closed; reconnecting…".to_string()),
                false => (LinkStatus::Connecting, format!("No reply from {} in 10 s; reconnecting…", n.name)),
            };
            (n.status, n.error, n.error_kind) = (status, Some(error), None);
        } else {
            n.status = if (t - timed_out) % 12 < 2 { LinkStatus::Offline } else { LinkStatus::Connecting };
            (n.error, n.error_kind) = (Some("No SSH reply in 10 s".into()), Some("timeout".into()));
        }
    }
}

#[test]
fn a_lan_outage_as_live_moves_the_nodes_is_one_steady_mac_problem() {
    for paging in [false, true] {
        let mut l = Live::new();
        if paging {
            let mut rules = RuleSet::defaults();
            rules.rules.iter_mut().find(|r| r.id == "mac-reach").unwrap().notify = true;
            l.h.set_rules(rules).unwrap();
        }
        l.run(5, |_, _| {});
        // cell1 and cell3 end by the stall watch, cell2 and gx10-0000 by the pinger; all four
        // are in the kind-less stretch at once around 35–41 s.
        let (stop, ends, pinger, back) = ([10, 10, 11, 12], [30, 25, 31, 34], [false, true, false, true], [200, 205, 215, 230]);
        let mut states: Vec<(u64, Option<MacReachState>)> = Vec::new();
        l.run(260, |s, t| {
            for (i, n) in s.nodes.iter_mut().enumerate() {
                through_a_lan_outage(n, t, stop[i], ends[i], pinger[i], back[i]);
            }
            let st = s.mac_reach.as_ref().map(|r| r.state);
            if states.last().map(|x| x.1) != Some(st) {
                states.push((t, st));
            }
        });
        // (Each state is what the step before stamped.)
        let shown: Vec<Option<MacReachState>> = states.iter().map(|x| x.1).collect();
        assert_eq!(shown, [Some(MacReachState::Ok), Some(MacReachState::LanBetween), Some(MacReachState::Ok)], "{states:?}");
        assert!(states[2].0 >= 230, "it ends once every node answers again: {states:?}");
        let mac: Vec<EventKind> = l.events.iter().filter(|e| e.alert.rule == "mac-reach").map(|e| e.kind).collect();
        assert_eq!(mac, [EventKind::Raised, EventKind::Resolved]);
        assert!(l.events.iter().filter(|e| e.alert.rule != "mac-reach").all(|e| e.alert.severity == Severity::Info), "only rows not confirmed down");
        let pages = l.pages();
        assert_eq!(pages, if paging { vec!["This Mac can't reach any node; the LAN between them?"] } else { vec![] }, "paging {paging}");
    }
}

/// cell3 coming and going: offline 40 s, back 60 s, six times in 10 minutes (round 4 paged each).
#[test]
fn a_flapping_node_is_paged_as_flapping_once_then_only_if_it_stays_away() {
    let mut l = Live::new();
    l.settings.repeat_minutes = 60;
    l.run(5, |_, _| {});
    let away = |s: &mut ClusterSnapshot, _: u64| {
        s.nodes[2].status = LinkStatus::Offline;
        (s.nodes[2].error, s.nodes[2].error_kind) = (Some("No SSH reply in 10 s".into()), Some("timeout".into()));
    };
    let back = |s: &mut ClusterSnapshot, _: u64| (s.nodes[2].status, s.nodes[2].error, s.nodes[2].error_kind) = (LinkStatus::Online, None, None);
    for _ in 0..6 {
        l.run(40, away);
        l.run(60, back);
    }
    let titles = |l: &Live| l.sent.iter().map(|n| n.title.clone()).collect::<Vec<_>>();
    let off = "cell3 offline · no SSH reply in 10 s";
    let flapping = format!("{off} · flapping");
    // Each return at once; gone again it's paged, the second time as flapping; then nothing.
    assert_eq!(titles(&l), [off, "cell3 is back online", off, "cell3 is back online", flapping.as_str(), "cell3 is back online"]);
    // The count runs through the whole run of them (round 1 restarted it at each resolve).
    let counts: Vec<Option<u32>> = l.events.iter().filter(|e| e.kind == EventKind::Raised).map(|e| e.alert.flapping.as_ref().map(|f| f.count)).collect();
    assert_eq!(counts, [None, None, Some(3), Some(4), Some(5), Some(6)]);
    // Gone again, and this time it stays away: paged once it has been away 10 minutes, and its
    // return is told.
    l.sent.clear();
    l.run(12 * 60, away);
    assert_eq!(titles(&l), [format!("Still flapping: {off}")]);
    l.run(5, back);
    assert_eq!(titles(&l), [format!("Still flapping: {off}"), "cell3 is back online".to_string()]);
    // Up for more than 10 minutes, the next time it goes is a page of its own and a new count.
    l.run(11 * 60, back);
    l.sent.clear();
    l.events.clear();
    l.run(40, away);
    assert_eq!(titles(&l), [off]);
    assert!(l.events.iter().filter(|e| e.kind == EventKind::Raised).all(|e| e.alert.flapping.is_none()), "{:?}", l.events);
}

#[test]
fn a_blank_entity_or_incident_names_nothing() {
    use silence::{make, needs_mac, Matcher};
    let rule_wide = Matcher { rule: Some("node-offline".into()), ..Default::default() };
    let blank_entity = Matcher { entity: Some("".into()), ..rule_wide.clone() };
    let blank_incident = Matcher { incident: Some(" ".into()), ..rule_wide.clone() };
    for m in [&blank_entity, &blank_incident] {
        assert_eq!(m.clone().normalized(), rule_wide, "{m:?}");
        // Every node's node-offline: Mac scope and a reason, as alert_silence judges it.
        for d in silence::DURATIONS {
            assert!(needs_mac(m, d) && needs_mac(&m.clone().normalized(), d), "{m:?} {d}");
            assert!(make(m.clone(), d, "", "x", 0, false).unwrap_err().starts_with("Say why"), "{m:?} {d}");
        }
        // Made with a reason, it's saved rule-wide: what the node agents get says what it covers.
        assert_eq!(make(m.clone(), "1d", "moving the rack", "x", 0, false).unwrap().matcher, rule_wide);
    }
    let nothing = Matcher { rule: Some(" ".into()), entity: Some(String::new()), incident: Some(String::new()) };
    assert_eq!(make(nothing, "1h", "because", "x", 0, false).unwrap_err(), "A silence has to name an alert or an incident.");
    assert!(Health::new(None).silence(blank_entity.clone(), "1h", "", "cell1 offline").unwrap_err().starts_with("Say why"));
    // One already saved with a blank field (it couldn't be made now) is dropped when they load.
    let dir = std::env::temp_dir().join(format!("sb-silences-blank-{}-{}", std::process::id(), fastrand::u32(..)));
    std::fs::create_dir_all(&dir).unwrap();
    let saved = serde_json::json!([
        {"id": "sil-blank", "matcher": {"rule": "node-offline", "entity": ""}, "until": {"kind": "at", "ms": 9_999_999_999_999u64}, "reason": "", "createdMs": 0, "createdBy": "Admin on this Mac", "source": "live"},
        {"id": "sil-one", "matcher": {"rule": "node-offline", "entity": "node::cell3"}, "until": {"kind": "at", "ms": 9_999_999_999_999u64}, "reason": "", "createdMs": 0, "createdBy": "Admin on this Mac", "source": "live"}
    ]);
    std::fs::write(dir.join("alert-silences.live.json"), serde_json::to_vec(&saved).unwrap()).unwrap();
    let kept: Vec<String> = silence::Store::new(Some(dir.clone())).load(false).into_iter().map(|s| s.id).collect();
    assert_eq!(kept, ["sil-one"]);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── r1-c ──

#[test]
fn disk_full_judges_free_space_the_way_df_does() {
    // gx10-0000's root disk as `df -B1 /` printed it in the Live tour (live/fixtures/disks-doctor.json).
    let fixture: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/live/fixtures/disks-doctor.json")).unwrap()).unwrap();
    let gx10 = fixture["disks"].as_array().unwrap().iter().find(|d| d["node"] == "gx10-0000").unwrap()["df"].clone();
    let d = DiskStats { mount: "/".into(), total: gx10["total"].as_u64().unwrap(), used: gx10["used"].as_u64().unwrap(), avail: gx10["avail"].as_u64(), ..Default::default() };
    assert_eq!(d.free(), 558_916_337_664, "about 520.5 GiB, not the 567 GiB of total − used");
    assert_eq!((d.used_pct().ceil(), (d.used as f64 / d.total as f64 * 100.0).round()), (41.0, 38.0), "df's 41%, not 38%");
    // From an older collector (no avail) it's total − used, as before.
    assert_eq!(DiskStats { avail: None, ..d.clone() }.free(), d.total - d.used);
    // 84% of the total is used but ext4 keeps 5% back: df says 88%, past the 85% line.
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[3].disks[0] = DiskStats { mount: "/".into(), total: 1000 * GIB, used: 840 * GIB, avail: Some(110 * GIB), ..Default::default() };
    let ev = run(&mut e, &mut s, 2, |_, _| {});
    let a = &ev.iter().find(|x| x.alert.rule == "disk-full").expect("past the 85% line, df-wise").alert;
    assert_eq!((a.severity, a.message.as_str()), (Severity::Warn, format!("Root disk 88% full ({} free)", super::fmt_bytes(110 * GIB)).as_str()));
    // Full for everyone but root: f_bavail 0, the 5% reserve still in f_bfree. df says Available
    // 0 and Use% 100%, so it's critical with nothing free, not 95% with the reserve as free space.
    let (frsize, blocks, bfree) = (4096u64, 239_946_252u64, 11_997_312u64);
    let full = DiskStats { mount: "/".into(), total: blocks * frsize, used: (blocks - bfree) * frsize, avail: Some(0), ..Default::default() };
    assert_eq!((full.free(), full.used_pct().round()), (0, 100.0));
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[3].disks[0] = full;
    let ev = run(&mut e, &mut s, 2, |_, _| {});
    let a = &ev.iter().find(|x| x.alert.rule == "disk-full").expect("a full disk").alert;
    assert_eq!((a.severity, a.message.as_str()), (Severity::Crit, format!("Root disk 100% full ({} free)", super::fmt_bytes(0)).as_str()));
}

#[test]
fn frame_6_disks_and_lldp_read_into_the_snapshot() {
    // As collector.py (frame 6) sends them; an older collector's lack the new fields.
    let disk: DiskStats = serde_json::from_str(r#"{"mount":"/","device":"/dev/nvme0n1p2","fsType":"ext4","total":982819848192,"used":373903474688,"avail":558916337664,"readBps":0,"writeBps":0}"#).unwrap();
    assert_eq!(disk.avail, Some(558_916_337_664));
    let old: DiskStats = serde_json::from_str(r#"{"mount":"/","device":"/dev/nvme0n1p2","fsType":"ext4","total":100,"used":40,"readBps":0,"writeBps":0}"#).unwrap();
    assert_eq!((old.avail, old.free()), (None, 60));
    let half: NetStats = serde_json::from_str(r#"{"iface":"enp1s0f1np1","role":"fabric","up":true,"speedMbps":200000,"rxBps":0,"txBps":0,"ip":"192.168.100.1","lldpPeers":[],"lldpSelfOnly":true}"#).unwrap();
    assert!(half.lldp_self_only && half.lldp_peers.is_empty() && half.lldp.is_none());
    let lan: NetStats = serde_json::from_str(r#"{"iface":"enP7s7","role":"lan","up":true,"speedMbps":1000,"rxBps":0,"txBps":0,"ip":"192.168.50.51","lldpPeers":[{"system":"cell3-example","port":"enP7s7","chassisId":"fc:9d:05:5e:bd:9c","portId":"fc:9d:05:5e:bd:9c"}],"lldp":{"system":"cell3-example","port":"enP7s7"}}"#).unwrap();
    assert_eq!((lan.lldp_peers[0].chassis_id.as_deref(), lan.lldp_self_only), (Some("fc:9d:05:5e:bd:9c"), false));
}

// ---- p1r3-check: the update rules (P1.21), on the update check's facts

mod update_rules {
    use super::*;
    use crate::model::UpdateFacts;

    const HOUR: u64 = 3_600_000;
    const DAY: u64 = 24 * HOUR;

    /// cell1's facts as the update check leaves them: lists fresh, nothing else waiting.
    fn facts(now: u64) -> UpdateFacts {
        UpdateFacts { pending: 4, lists_ms: Some(now), checked_ms: now, ..Default::default() }
    }

    fn alert(e: &Engine, s: &ClusterSnapshot, rule: &str) -> Option<Alert> {
        e.view(s.ts_ms, Some(s)).alerts.into_iter().find(|a| a.rule == rule && a.state == AlertState::Firing)
    }

    /// One step `ms` after the last, with cell1's facts as `f` makes them.
    fn at(e: &mut Engine, s: &mut ClusterSnapshot, ms: u64, f: impl FnOnce(&mut UpdateFacts, u64)) -> Vec<AlertEvent> {
        s.ts_ms += ms;
        let now = s.ts_ms;
        let mut u = s.nodes[0].updates.clone().unwrap_or_else(|| facts(now));
        u.lists_ms = Some(now);
        f(&mut u, now);
        s.nodes[0].updates = Some(u);
        e.step(s, now)
    }

    #[test]
    fn security_updates_are_information_then_a_warning_after_seven_days() {
        let mut e = Engine::default();
        let mut s = healthy();
        let t0 = s.ts_ms + 1000;
        at(&mut e, &mut s, 1000, |u, now| (u.security, u.security_since_ms) = (3, Some(now)));
        let a = alert(&e, &s, "security-updates-pending").expect("information at once");
        assert_eq!((a.severity, a.message.as_str(), a.summary.as_str(), a.notify, a.counted), (Severity::Info, "3 security updates pending", "cell1: 3 security updates pending", false, false));
        // A second short of a week: still information.
        at(&mut e, &mut s, SECURITY_WAIT_MS - 1000, |_, _| {});
        assert_eq!(alert(&e, &s, "security-updates-pending").unwrap().severity, Severity::Info);
        // A week since SparkBox first saw them: a warning, notified.
        let ev = at(&mut e, &mut s, 1000, |_, _| {});
        assert_eq!(s.ts_ms - t0, SECURITY_WAIT_MS);
        let a = alert(&e, &s, "security-updates-pending").unwrap();
        assert_eq!((a.severity, a.message.as_str(), a.notify), (Severity::Warn, "3 security updates pending for 7 days", true));
        assert!(ev.iter().any(|x| x.alert.rule == "security-updates-pending" && x.alert.severity == Severity::Warn), "{ev:?}");
        at(&mut e, &mut s, DAY, |_, _| {});
        assert_eq!(alert(&e, &s, "security-updates-pending").unwrap().message, "3 security updates pending for 8 days");
        // The next check finds none: it clears at once (clearS 0).
        let ev = at(&mut e, &mut s, 1000, |u, _| (u.security, u.security_since_ms) = (0, None));
        assert!(ev.iter().any(|x| x.kind == EventKind::Resolved && x.alert.rule == "security-updates-pending"), "{ev:?}");
        assert!(alert(&e, &s, "security-updates-pending").is_none());
        // One update reads in the singular.
        at(&mut e, &mut s, 1000, |u, now| (u.security, u.security_since_ms) = (1, Some(now)));
        assert_eq!(alert(&e, &s, "security-updates-pending").unwrap().message, "1 security update pending");
    }

    /// The security alert says when fixes are in the GPU stack, and never "install them from
    /// there" for them: SparkBox can't upgrade them yet.
    #[test]
    fn the_security_alert_never_says_install_from_there_for_the_gpu_stack() {
        use crate::health::rules::security_words;
        let (message, detail) = security_words(4, Some(1), None);
        assert_eq!(message, "4 security updates pending (1 is in the GPU stack: SparkBox can't upgrade it yet)");
        assert_eq!(detail, "Updates lists them; Upgrade security only installs the 3 outside the GPU stack. SparkBox can't upgrade GPU-stack ones yet.");
        assert_eq!(security_words(3, Some(2), Some(8)).0, "3 security updates pending for 8 days (2 are in the GPU stack: SparkBox can't upgrade them yet)");
        let (message, detail) = security_words(1, Some(1), None);
        assert_eq!(message, "1 security update pending (1 is in the GPU stack: SparkBox can't upgrade it yet)");
        assert!(!detail.contains("install them from there"), "{detail}");
        assert_eq!(security_words(2, Some(0), None), ("2 security updates pending".to_string(), "Updates lists them; install them from there (Upgrade security only).".to_string()));
        // Not known (what the facts carry today): no claim either way, and no "install from there".
        let (message, detail) = security_words(4, None, Some(7));
        assert_eq!(message, "4 security updates pending for 7 days");
        assert!(!detail.contains("install them from there") && detail.contains("SparkBox can't upgrade GPU-stack ones (kernel, NVIDIA, CUDA, Docker, network) yet"), "{detail}");
        for gpu in [None, Some(1), Some(4)] {
            assert!(!security_words(4, gpu, None).1.contains("install them from there"), "{gpu:?}");
        }
        // The rule's detail as it fires.
        let mut e = Engine::default();
        let mut s = healthy();
        at(&mut e, &mut s, 1000, |u, now| (u.security, u.security_since_ms) = (4, Some(now)));
        let a = alert(&e, &s, "security-updates-pending").unwrap();
        assert!(!a.detail.as_deref().unwrap_or_default().contains("install them from there"), "{:?}", a.detail);
        // With the update check's GPU-stack count (UpdateFacts.security_gpu) the alert names it.
        let mut e = Engine::default();
        let mut s = healthy();
        at(&mut e, &mut s, 1000, |u, now| (u.security, u.security_since_ms, u.security_gpu) = (4, Some(now), Some(1)));
        let a = alert(&e, &s, "security-updates-pending").unwrap();
        assert_eq!(a.message, "4 security updates pending (1 is in the GPU stack: SparkBox can't upgrade it yet)");
    }

    /// Decision (d)'s window move: an upgrade's own window gets a later end and keeps when it
    /// opened and why; the owner's maintenance mode doesn't move, no window is opened by it, and
    /// past its end (and the grace) the node's alerts count again.
    #[test]
    fn an_upgrades_window_extends_but_the_owners_maintenance_mode_doesnt_move() {
        use crate::health::maintenance::{begin_at, begin_manual_at, end_at, end_manual_at, extend_at, planned_now, windows};
        let _live = crate::health::maintenance::live_test_lock();
        const T: u64 = 4_600_000_000_000;
        begin_at(false, "r4upd-a", Some(T + 1_800_000), "upgrading", T);
        assert!(extend_at(false, "r4upd-a", T + 3_600_000));
        let m = windows(false).get("r4upd-a").cloned().unwrap();
        assert_eq!((m.until_ms, m.started_ms, m.reason.as_str(), m.manual), (Some(T + 3_600_000), T, "upgrading", false));
        assert!(!extend_at(false, "r4upd-none", T + 1), "no window to move");
        assert!(!windows(false).contains_key("r4upd-none"), "and none opened");
        begin_manual_at(false, "r4upd-m", Some(T + 3_600_000), T).unwrap();
        assert!(!extend_at(false, "r4upd-m", T + 9_999_999));
        assert_eq!(windows(false).get("r4upd-m").unwrap().until_ms, Some(T + 3_600_000), "maintenance mode is the owner's");
        let info = crate::model::MaintInfo { until_ms: m.until_ms, reason: m.reason.clone(), started_ms: m.started_ms };
        assert!(planned_now(&info, T + 3_600_000 + 1_000, 5_000));
        assert!(!planned_now(&info, T + 3_600_000 + 6_000, 5_000), "past its end and the grace, alerts count again");
        assert!(end_at(false, "r4upd-a"));
        end_manual_at(false, "r4upd-m").unwrap();
    }

    #[test]
    fn a_restart_waiting_three_days_is_a_warning() {
        let mut e = Engine::default();
        let mut s = healthy();
        let t0 = s.ts_ms;
        at(&mut e, &mut s, 1000, |u, _| (u.reboot_required, u.reboot_since_ms) = (true, Some(t0)));
        assert!(alert(&e, &s, "restart-pending").is_none(), "not before 3 days");
        at(&mut e, &mut s, RESTART_WAIT_MS - 2000, |_, _| {});
        assert!(alert(&e, &s, "restart-pending").is_none());
        at(&mut e, &mut s, 1000, |_, _| {});
        let a = alert(&e, &s, "restart-pending").expect("at 3 days");
        assert_eq!((a.severity, a.message.as_str(), a.summary.as_str()), (Severity::Warn, "Restart pending for 3 days", "cell1: restart pending for 3 days"));
        // Without the file's time there's no telling how long it has waited: no alert.
        let ev = at(&mut e, &mut s, 1000, |u, _| u.reboot_since_ms = None);
        assert!(ev.iter().any(|x| x.kind == EventKind::Resolved && x.alert.rule == "restart-pending"), "{ev:?}");
        // Restarted: the flag is gone, and so is the alert.
        at(&mut e, &mut s, 1000, |u, _| u.reboot_since_ms = Some(t0));
        assert!(alert(&e, &s, "restart-pending").is_some());
        at(&mut e, &mut s, 1000, |u, _| (u.reboot_required, u.reboot_since_ms) = (false, None));
        assert!(alert(&e, &s, "restart-pending").is_none());
    }

    #[test]
    fn package_lists_older_than_48_hours_are_stale() {
        let mut e = Engine::default();
        let mut s = healthy();
        let t0 = s.ts_ms;
        // `at` refreshes the lists each step; these steps keep them where they were.
        at(&mut e, &mut s, 1000, |u, _| u.lists_ms = Some(t0));
        at(&mut e, &mut s, LISTS_STALE_MS - 2000, |u, _| u.lists_ms = Some(t0));
        assert!(alert(&e, &s, "package-lists-stale").is_none(), "a second short of 48 h");
        at(&mut e, &mut s, 1000, |u, _| u.lists_ms = Some(t0));
        let a = alert(&e, &s, "package-lists-stale").expect("at 48 h");
        assert_eq!(
            (a.severity, a.message.as_str(), a.summary.as_str()),
            (Severity::Warn, "Package lists last refreshed 2 days ago; apt-daily may not be running", "cell1: package lists last refreshed 2 days ago")
        );
        at(&mut e, &mut s, DAY, |u, _| u.lists_ms = Some(t0));
        assert_eq!(alert(&e, &s, "package-lists-stale").unwrap().message, "Package lists last refreshed 3 days ago; apt-daily may not be running");
        // Refreshed: clears at the next check.
        let ev = at(&mut e, &mut s, 1000, |_, _| {});
        assert!(ev.iter().any(|x| x.kind == EventKind::Resolved && x.alert.rule == "package-lists-stale"), "{ev:?}");
    }

    #[test]
    fn no_facts_no_update_alerts() {
        // A node SparkBox hasn't checked (and every node in Demo) has no `updates`.
        let mut e = Engine::default();
        let mut s = healthy();
        s.ts_ms += 30 * DAY;
        e.step(&s, s.ts_ms);
        let rules = ["security-updates-pending", "restart-pending", "package-lists-stale"];
        assert!(e.view(s.ts_ms, Some(&s)).alerts.iter().all(|a| !rules.contains(&a.rule.as_str())));
        // Facts on a node that isn't reporting are left as they were, not judged.
        s.nodes[1].status = LinkStatus::Offline;
        s.nodes[1].updates = Some(UpdateFacts { security: 2, security_since_ms: Some(0), lists_ms: Some(0), ..Default::default() });
        e.step(&s, s.ts_ms + 1000);
        assert!(e.view(s.ts_ms + 1000, Some(&s)).alerts.iter().all(|a| !rules.contains(&a.rule.as_str())));
    }

    #[test]
    fn unconfirmed_facts_leave_the_update_alerts_as_they_are() {
        let rules = ["security-updates-pending", "restart-pending", "package-lists-stale"];
        let mut e = Engine::default();
        let mut s = healthy();
        let ours = |ev: &[AlertEvent]| ev.iter().filter(|x| rules.contains(&x.alert.rule.as_str())).map(|x| (x.kind, x.alert.rule.clone())).collect::<Vec<_>>();
        // Kept facts that would raise all three (from the last launch, or before a reconnect): none is.
        let ev = at(&mut e, &mut s, 1000, |u, now| {
            u.unconfirmed = true;
            (u.security, u.security_since_ms) = (3, Some(now - 8 * DAY));
            (u.reboot_required, u.reboot_since_ms) = (true, Some(now - 4 * DAY));
            u.lists_ms = Some(now - 3 * DAY);
        });
        assert_eq!(ours(&ev), []);
        assert!(e.view(s.ts_ms, Some(&s)).alerts.iter().all(|a| !rules.contains(&a.rule.as_str())));
        // A check confirms them: judged.
        let ev = at(&mut e, &mut s, 1000, |u, now| {
            u.unconfirmed = false;
            u.security_since_ms = Some(now - DAY);
            u.lists_ms = Some(now - 3 * DAY);
        });
        assert_eq!(ours(&ev).len(), 3, "{ev:?}");
        let a = alert(&e, &s, "security-updates-pending").unwrap();
        assert_eq!(a.severity, Severity::Info);
        // Unconfirmed again: what they'd say now neither worsens nor clears what's up.
        let ev = at(&mut e, &mut s, 1000, |u, now| {
            u.unconfirmed = true;
            u.security_since_ms = Some(now - 8 * DAY);
        });
        assert_eq!(ours(&ev), []);
        let ev = at(&mut e, &mut s, 1000, |u, _| (u.security, u.security_since_ms, u.reboot_required, u.reboot_since_ms) = (0, None, false, None));
        assert_eq!(ours(&ev), []);
        for rule in rules {
            assert!(alert(&e, &s, rule).is_some(), "{rule}");
        }
        assert_eq!(alert(&e, &s, "security-updates-pending").map(|b| (b.severity, b.firing_since_ms)), Some((Severity::Info, a.firing_since_ms)));
        // Confirmed (security and restart gone, the lists refreshed by `at`): they clear.
        let ev = at(&mut e, &mut s, 1000, |u, _| u.unconfirmed = false);
        let mut cleared = ours(&ev);
        cleared.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(cleared, ["package-lists-stale", "restart-pending", "security-updates-pending"].map(|r| (EventKind::Resolved, r.to_string())));
    }

    #[test]
    fn the_update_rules_ship_coded_on_this_mac_only() {
        let defaults = RuleSet::defaults();
        let agent: serde_json::Value = serde_json::from_str(include_str!("../agent/fixtures/agent-rules.json")).unwrap();
        for id in ["security-updates-pending", "restart-pending", "package-lists-stale"] {
            let r = defaults.get(id).unwrap_or_else(|| panic!("{id} isn't in the defaults"));
            assert!(CODED.contains(&id), "{id}");
            assert_eq!((r.kind, r.mac_only, r.clear_s, r.for_s, r.enabled), (super::super::rule::RuleKind::Coded, true, 0, 0, true), "{id}");
            assert!(!r.short.is_empty() && r.short.chars().count() <= super::super::rule::SHORT_MAX, "{id}");
            assert!(!agent["coded"].as_array().unwrap().iter().any(|c| c == id), "the agents don't evaluate {id}");
        }
        assert_eq!((SECURITY_WAIT_MS, RESTART_WAIT_MS, LISTS_STALE_MS), (7 * DAY, 3 * DAY, 48 * HOUR));
    }
}

/// vLLM's count of requests with NaNs in their logits: one warning while it rises, none for a count
/// it already had, gone when it stops rising or the server's restart resets it.
#[test]
fn corrupted_requests_warn_once_while_they_rise_and_clear_on_a_reset() {
    let of = |ev: &[AlertEvent], kind: EventKind| ev.iter().filter(|e| e.alert.rule == "endpoint-corrupted" && e.kind == kind).count();
    let mut e = Engine::default();
    let mut s = healthy();
    // It reads 3 from the first look on: no news, whatever happened before SparkBox looked.
    let ev = run_ep(&mut e, &mut s, 30, |_| endpoint("ready", &[("corrupted", 3.0), ("genTps", 10.0)]));
    assert_eq!(of(&ev, EventKind::Raised), 0, "{ev:?}");
    // Rising: one warning, however often it rises.
    let ev = run_ep(&mut e, &mut s, 20, |i| endpoint("ready", &[("corrupted", 3.0 + (i / 4) as f64), ("genTps", 10.0)]));
    assert_eq!(of(&ev, EventKind::Raised), 1, "{ev:?}");
    let a = &ev.iter().find(|e| e.alert.rule == "endpoint-corrupted").unwrap().alert;
    assert_eq!((a.severity, a.entity.kind.as_str(), a.notify), (Severity::Warn, "endpoint", true));
    assert!(a.message.ends_with("corrupted requests in 5 min"), "{}", a.message);
    assert!(e.view(s.ts_ms, Some(&s)).alerts.iter().any(|a| a.rule == "endpoint-corrupted"));
    // The server restarted: the counter starts again, which is no news, and the alert clears.
    let ev = run_ep(&mut e, &mut s, 5, |_| endpoint("ready", &[("corrupted", 0.0), ("genTps", 10.0)]));
    assert_eq!((of(&ev, EventKind::Raised), of(&ev, EventKind::Resolved)), (0, 1), "{ev:?}");

    // Not rising any more clears it too, once the window has passed.
    let mut e = Engine::default();
    let mut s = healthy();
    run_ep(&mut e, &mut s, 10, |i| endpoint("ready", &[("corrupted", i as f64), ("genTps", 10.0)]));
    assert!(e.view(s.ts_ms, Some(&s)).alerts.iter().any(|a| a.rule == "endpoint-corrupted"));
    let ev = run_ep(&mut e, &mut s, 301, |_| endpoint("ready", &[("corrupted", 9.0), ("genTps", 10.0)]));
    assert_eq!(of(&ev, EventKind::Resolved), 1, "{ev:?}");

    // A server that doesn't publish the counter (no VLLM_COMPUTE_NANS_IN_LOGITS) is never judged.
    let mut e = Engine::default();
    let mut s = healthy();
    let ev = run_ep(&mut e, &mut s, 30, |_| endpoint("ready", &[("genTps", 10.0)]));
    assert_eq!(of(&ev, EventKind::Raised), 0, "{ev:?}");
}

/// The whole way: /metrics text, the endpoint's JSON, the rule. endpoint_info keeps a fixed list,
/// so a metric left off it reaches every test above and never reaches an alert.
#[test]
fn a_corrupted_counter_on_metrics_reaches_the_rule() {
    use crate::models::endpoints::{Endpoint, Health};
    use crate::models::metrics::{Ring, Scrape};
    let mut e = Engine::default();
    let mut s = healthy();
    let mut ring = Ring::default();
    let mut events = Vec::new();
    for i in 0..10 {
        s.ts_ms += 1000;
        let text = format!("vllm:generation_tokens_total 10\nvllm:corrupted_requests_total{{engine=\"0\"}} {i}.0\n");
        let metrics = ring.push(Scrape::from_text(&text, s.ts_ms)).clone();
        let served = Endpoint { id: "cell1-vllm-8000".into(), health: Health::Ready, metrics: Some(metrics), ..Default::default() };
        events.extend(e.step_with(&s, &endpoint_infos(&[served]), s.ts_ms));
    }
    let raised = events.iter().filter(|e| e.alert.rule == "endpoint-corrupted" && e.kind == EventKind::Raised).count();
    assert_eq!(raised, 1, "{events:?}");
}

/// Planned work on its model: Planned, as every endpoint-* rule, not an alert of its own.
#[test]
fn corrupted_requests_are_plannable_like_any_endpoint_rule() {
    let mut e = Engine::default();
    let mut s = healthy();
    s.nodes[0].maintenance = Some(crate::model::MaintInfo { until_ms: Some(s.ts_ms + 3_600_000), reason: "restarting".into(), started_ms: s.ts_ms });
    run_ep(&mut e, &mut s, 6, |i| endpoint("ready", &[("corrupted", i as f64), ("genTps", 10.0)]));
    let v = e.view(s.ts_ms, Some(&s));
    let a = v.alerts.iter().find(|a| a.rule == "endpoint-corrupted").expect("it's there");
    assert!(a.planned.is_some(), "{a:?}");
}


#[test]
fn serving_memory_streams_match_the_agent_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("fixtures/serving-memory.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut e = Engine::default();
        if case["headroomEnabled"] == false {
            Arc::make_mut(&mut e.rules).rules.iter_mut().find(|r| r.id == "mem-available").unwrap().enabled = false;
        }
        let mut s = healthy();
        let start = s.ts_ms;
        let mut got = Vec::new();
        for segment in case["segments"].as_array().unwrap() {
            for _ in 0..segment["secs"].as_u64().unwrap() {
                s.ts_ms += 1000;
                let n = &mut s.nodes[0];
                let available = (segment["availableGiB"].as_f64().unwrap_or(6.0) * GIB as f64) as u64;
                n.mem = crate::model::MemStats {
                    total: 256 * GIB, used: 256 * GIB - available,
                    available: if segment["invalidAvailable"] == true { 0 } else { available },
                    gpu_alloc: (segment["gpuGiB"].as_f64().unwrap_or(224.0) * GIB as f64) as u64,
                    psi_full10: Some(segment["psi"].as_f64().unwrap_or(0.0) as f32),
                    swap_in_bps: if segment.get("swapMiBs").is_some_and(|v| v.is_null()) { None } else { Some(segment["swapMiBs"].as_f64().unwrap_or(0.0) * 1_048_576.0) },
                    swap_total: 16 * GIB, swap_used: (segment["swapUsedGiB"].as_f64().unwrap_or(0.0) * GIB as f64) as u64,
                    ..Default::default()
                };
                n.gpu.readable = segment["gpuReadable"].as_bool().unwrap_or(true);
                n.containers.clear();
                if let Some(share) = segment["share"].as_str() {
                    n.containers.push(Container { name: "model-rank".into(), state: segment["state"].as_str().unwrap_or("running").into(), labels: [
                        ("sparkbox.instance", "release"), ("sparkbox.deployment", "d"), ("sparkbox.path", "/model"),
                        ("sparkbox.role", segment["role"].as_str().unwrap_or("single")), ("sparkbox.gpu-share", share),
                    ].into_iter().map(|(k,v)| (k.into(),v.into())).collect(), ..Default::default() });
                }
                n.kernel_events = if segment["oom"] == true { vec![crate::model::KernelEvent { ts_ms: start + 1000, kind: "oom".into(), severity: "crit".into(), process: Some("vllm".into()), text: "Out of memory: Killed process vllm".into(), ..Default::default() }] } else { vec![] };
                for ev in e.step(&s, s.ts_ms) {
                    let selected = case["focus"].as_array().map(|ids| ids.iter().any(|id| id.as_str() == Some(ev.alert.rule.as_str())))
                        .unwrap_or_else(|| matches!(ev.alert.rule.as_str(), "mem-high" | "mem-available" | "mem-pressure" | "mem-swap-in" | "kernel-oom"));
                    if selected { got.push(serde_json::json!([(s.ts_ms-start)/1000, ev.alert.rule, ev.kind, ev.alert.severity])); }
                }
            }
        }
        assert_eq!(serde_json::json!(got), case["expect"], "{}", case["name"]);
    }
}

#[test]
fn stale_memory_cannot_confirm_pending_or_clear_firing_pressure() {
    let mut e = Engine::default();
    let mut s = healthy();
    let low = |s: &mut ClusterSnapshot| { s.nodes[0].mem.used = 125 * GIB; s.nodes[0].mem.available = 3 * GIB; s.nodes[0].mem.gpu_alloc = 110 * GIB; };
    run(&mut e, &mut s, 40, |s,_| low(s));
    run(&mut e, &mut s, 30, |s,_| s.nodes[0].stale = true);
    let ev = run(&mut e, &mut s, 60, |s,_| { s.nodes[0].stale = false; low(s); });
    assert!(!ev.iter().any(|e| e.alert.rule == "mem-available"));
    let ev = run(&mut e, &mut s, 1, |s,_| low(s));
    assert!(ev.iter().any(|e| e.alert.rule == "mem-available" && e.kind == EventKind::Raised));
    let ev = run(&mut e, &mut s, 90, |s,_| { s.nodes[0].stale = true; s.nodes[0].mem.available = 20 * GIB; });
    assert!(!ev.iter().any(|e| e.alert.rule == "mem-available" && e.kind == EventKind::Resolved));
}
