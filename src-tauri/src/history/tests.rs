use std::collections::BTreeMap;

use super::backfill::{fill_at, REBOOT_AGO_S, REBOOT_GAP_S};
use super::store::{enforce_budget, file_name, load_dir, save_dir, segment_name, utc_day, TIERS};
use super::*;
use crate::util::{Flush, OsSync, Syncer};
use crate::demo::DemoCluster;
use crate::inventory::Inventory;

const T0: i64 = 1_800_000_000; // a fixed "now" (2027), on a 15-minute boundary

fn reading(x: f32) -> [f32; N] {
    [x; N]
}

fn names(m: &[&str]) -> (Vec<String>, Vec<usize>) {
    (m.iter().map(|s| s.to_string()).collect(), m.iter().map(|s| METRICS.iter().position(|k| k == s).unwrap()).collect())
}

fn q(s: &Series, m: &[&str], from_s: i64, to_s: i64, max: usize, now_s: i64) -> QueryResult {
    let (n, i) = names(m);
    s.query(&n, &i, from_s as u64 * 1000, to_s as u64 * 1000, max, now_s as u64 * 1000)
}

#[test]
fn buckets_keep_min_average_and_max() {
    let mut s = Series::new();
    for (i, v) in [10.0, 20.0, 60.0, 30.0, 30.0, 50.0].iter().enumerate() {
        s.push(T0 + i as i64, &reading(*v));
    }
    // Asking for 10 minutes from the 10 s tier: the first bucket holds all six readings.
    let r = q(&s, &["cpu"], T0 - 600 + 6, T0 + 6, 1000, T0 + 7200);
    assert_eq!(r.tier, "10s");
    let first = r.ts.iter().position(|&t| t == T0 as u64 * 1000).unwrap();
    assert_eq!(r.series["cpu"][first], Some(200.0 / 6.0));
    assert_eq!(r.min.as_ref().unwrap()["cpu"][first], Some(10.0));
    assert_eq!(r.max.as_ref().unwrap()["cpu"][first], Some(60.0));
}

#[test]
fn missing_and_offline_seconds_come_back_as_gaps() {
    let mut s = Series::new();
    s.push(T0, &reading(1.0));
    s.push(T0 + 1, &[f32::NAN; N]); // offline
    s.push(T0 + 4, &reading(4.0)); // seconds 2 and 3 never arrived
    let r = q(&s, &["gpu"], T0, T0 + 5, 100, T0 + 5);
    assert_eq!(r.tier, "1s");
    assert_eq!(r.series["gpu"], vec![Some(1.0), None, None, None, Some(4.0)]);
    assert!(r.min.is_none(), "the 1 s tier has no min/max bands");
}

#[test]
fn a_second_reading_in_the_same_second_is_ignored() {
    let mut s = Series::new();
    s.push(T0, &reading(1.0));
    s.push(T0, &reading(99.0));
    assert_eq!(q(&s, &["cpu"], T0, T0 + 1, 10, T0 + 1).series["cpu"], vec![Some(1.0)]);
}

#[test]
fn old_buckets_fall_out_of_retention() {
    let mut s = Series::new();
    s.push(T0, &reading(5.0));
    // Two hours later the 1 s ring has lapped; the 10 s tier still remembers.
    s.push(T0 + 7200, &reading(6.0));
    let r = q(&s, &["cpu"], T0 - 10, T0 + 7201, 2000, T0 + 7201);
    assert_eq!(r.tier, "10s");
    assert!(r.series["cpu"].contains(&Some(5.0)));
    // A day and a bit later the 10 s tier has lapped too; the 1 min tier keeps it for a week.
    s.push(T0 + 90_000, &reading(7.0));
    let r = q(&s, &["cpu"], T0 - 60, T0 + 90_001, 4000, T0 + 90_001);
    assert_eq!(r.tier, "1m");
    assert!(r.series["cpu"].contains(&Some(5.0)));
    // After 31 days only the 15 min tier is left, and it has lapped as well.
    s.push(T0 + 31 * 86_400, &reading(8.0));
    let r = q(&s, &["cpu"], T0 - 900, T0 + 31 * 86_400 + 1, 4000, T0 + 31 * 86_400 + 1);
    assert_eq!(r.tier, "15m");
    assert!(!r.series["cpu"].contains(&Some(5.0)));
    assert_eq!(r.series["cpu"].last().copied().flatten(), Some(8.0));
}

#[test]
fn the_finest_tier_that_covers_the_range_wins() {
    let mut s = Series::new();
    for t in T0 - 7200..=T0 {
        s.push(t, &reading((t % 100) as f32));
    }
    assert_eq!(q(&s, &["cpu"], T0 - 300, T0, 1000, T0).tier, "1s");
    assert_eq!(q(&s, &["cpu"], T0 - 3600, T0, 4000, T0).tier, "1s");
    assert_eq!(q(&s, &["cpu"], T0 - 6 * 3600, T0, 1000, T0).tier, "10s");
    assert_eq!(q(&s, &["cpu"], T0 - 7 * 86_400, T0, 1000, T0).tier, "1m");
    assert_eq!(q(&s, &["cpu"], T0 - 30 * 86_400, T0, 1000, T0).tier, "15m");
}

#[test]
fn a_restarted_1s_ring_defers_to_the_10s_tier() {
    let mut s = Series::new();
    // An hour in the bucket tiers only (as if loaded from disk), then a minute of live seconds.
    for t in (T0 - 3600..T0 - 60).step_by(10) {
        s.push_buckets(t, &reading(3.0));
    }
    for t in T0 - 60..T0 {
        s.push(t, &reading(4.0));
    }
    let r = q(&s, &["cpu"], T0 - 3600, T0, 4000, T0);
    assert_eq!(r.tier, "10s", "the 1 s ring only covers the last minute");
    let r = q(&s, &["cpu"], T0 - 50, T0, 100, T0);
    assert_eq!(r.tier, "1s");
}

#[test]
fn merged_buckets_align_to_fixed_boundaries() {
    let mut s = Series::new();
    for t in T0 - 6 * 3600..T0 {
        s.push(t, &reading(1.0));
    }
    let a = q(&s, &["cpu"], T0 - 6 * 3600, T0, 360, T0);
    let b = q(&s, &["cpu"], T0 - 6 * 3600 + 7, T0 + 7, 360, T0 + 7);
    assert_eq!(a.step_ms, 60_000, "2160 buckets of 10 s merged six at a time");
    assert!(a.ts.len() <= 361);
    for t in a.ts.iter().chain(&b.ts) {
        assert_eq!(t % 60_000, 0, "every point starts on a whole minute");
    }
    assert_eq!(a.ts[..], b.ts[..a.ts.len()], "a later query lands on the same boundaries");
}

#[test]
fn history_starts_at_the_first_recorded_bucket() {
    let mut s = Series::new();
    for t in T0 - 1800..T0 {
        s.push(t, &reading(1.0));
    }
    let r = q(&s, &["cpu"], T0 - 6 * 3600, T0, 720, T0);
    assert_eq!(r.starts_at_ms, Some((T0 - 1800) as u64 * 1000));
    assert_eq!(r.series["cpu"][0], None, "before the start is a gap, not a zero");
}

#[test]
fn files_round_trip_and_unknown_metrics_are_skipped() {
    let dir = std::env::temp_dir().join(format!("sb-history-{}-{}", std::process::id(), fastrand::u32(..)));
    let now = (now_ms() / 1000) as i64;
    let mut nodes = BTreeMap::new();
    let mut s = Series::new();
    for t in now - 7200..now {
        let v = if (now - 4000..now - 3000).contains(&t) { [f32::NAN; N] } else { reading((t % 50) as f32) };
        s.push(t, &v);
    }
    nodes.insert("node:gx10-0000".to_string(), s);
    let written = save_dir(&dir, &mut nodes, &OsSync, false);
    assert_eq!(written.entities, vec!["node:gx10-0000".to_string()]);
    assert_eq!(written.bytes, std::fs::metadata(dir.join(file_name("node:gx10-0000"))).unwrap().len());
    assert_eq!(nodes["node:gx10-0000"].encoded_len("node:gx10-0000", now), nodes["node:gx10-0000"].encode("node:gx10-0000", now).len());
    assert_eq!(save_dir(&dir, &mut nodes, &OsSync, false), Written::default(), "nothing new: nothing written");

    let mut loaded = load_dir(&dir, metric_names);
    assert!(!loaded["node:gx10-0000"].changed(), "a file as loaded isn't written again");
    assert!(save_dir(&dir, &mut loaded, &OsSync, false).entities.is_empty());
    let back = &loaded["node:gx10-0000"];
    for m in [["cpu"], ["diskWrite"]] {
        let a = q(&nodes["node:gx10-0000"], &m, now - 7200, now, 5000, now);
        let b = q(back, &m, now - 7200, now, 5000, now);
        assert_eq!(a.tier, "10s");
        assert_eq!(a.ts, b.ts);
        assert_eq!(a.series, b.series);
        assert_eq!(a.min, b.min);
        assert_eq!(a.max, b.max);
    }
    let gap = q(back, &["cpu"], now - 3900, now - 3100, 1000, now);
    assert!(gap.series["cpu"].iter().all(|v| v.is_none()), "the outage is still a gap after loading");

    // A damaged file is set aside, not deleted, and history starts over for that node.
    let path = dir.join(file_name("node:gx10-0000"));
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&path, bytes).unwrap();
    assert!(load_dir(&dir, metric_names).is_empty());
    assert!(std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".corrupt-")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn demo_and_live_histories_stay_apart() {
    let h = History::new(None);
    let mut demo = DemoCluster::new(&Inventory::example());
    let snap = demo.snapshot();
    h.record(&snap);
    assert!(!h.node_ids().is_empty());
    let mut live = snap.clone();
    live.demo = false;
    live.ts_ms += 1000;
    live.nodes.truncate(1);
    h.record(&live);
    assert_eq!(h.node_ids(), vec![live.nodes[0].id.clone()], "switching to Live starts from Live's own history");
}

#[test]
fn demo_backfill_covers_a_day_with_a_reboot_gap() {
    let mut demo = DemoCluster::new(&Inventory::example());
    let mut snap = demo.snapshot();
    snap.ts_ms = T0 as u64 * 1000;
    let mut nodes = BTreeMap::new();
    fill_at(&mut nodes, &snap, 0);
    assert_eq!(nodes.len(), snap.nodes.len());

    let first = &snap.nodes[0];
    let key = |id: &str| format!("node:{id}");
    let day = q(&nodes[&key(&first.id)], &["gpu", "memUsed", "fabricTx"], T0 - 86_400, T0, 720, T0);
    assert_eq!(day.tier, "10s");
    let gpu: Vec<f32> = day.series["gpu"].iter().flatten().copied().collect();
    assert!(gpu.len() >= 700, "a full day of readings ({} points)", gpu.len());
    let (lo, hi) = (gpu.iter().cloned().fold(f32::MAX, f32::min), gpu.iter().cloned().fold(0.0, f32::max));
    assert!(hi - lo > 30.0, "load follows the day: {lo}..{hi}");
    assert!(day.series["fabricTx"].iter().flatten().all(|v| *v >= 0.0));

    // The second node was gone for four minutes.
    let second = &snap.nodes[1].id;
    let at = T0 - REBOOT_AGO_S;
    let gap = q(&nodes[&key(second)], &["cpu"], at + 30, at + REBOOT_GAP_S - 30, 100, T0);
    assert!(gap.series["cpu"].iter().all(|v| v.is_none()), "{:?}", gap.series["cpu"]);
    let after = q(&nodes[&key(second)], &["cpu"], at + REBOOT_GAP_S + 60, at + REBOOT_GAP_S + 600, 100, T0);
    assert!(after.series["cpu"].iter().all(|v| v.is_some()));

    // The last 15 minutes are there at 1 s, and they end at the live values.
    let recent = q(&nodes[&key(&first.id)], &["memUsed"], T0 - 900, T0, 4000, T0);
    assert_eq!(recent.tier, "1s");
    assert!(recent.series["memUsed"].iter().all(|v| v.is_some()), "every second of the last 15 minutes");
    let last = recent.series["memUsed"].iter().rev().flatten().next().copied().unwrap();
    let live = first.mem.used as f32;
    assert!((last - live).abs() / live < 0.05, "history ends near the live value: {last} vs {live}");
}

#[test]
fn an_as_found_history_reads_like_the_idle_gb10s() {
    // Started in "Your cluster at rest": the history before now is the simulator's own at-rest
    // readings, so the GPU power chart doesn't step down at "now".
    let mut demo = DemoCluster::as_found();
    let mut snap = demo.snapshot();
    snap.ts_ms = T0 as u64 * 1000;
    let mut nodes = BTreeMap::new();
    fill_at(&mut nodes, &snap, 0);
    for n in &snap.nodes {
        let h = q(&nodes[&format!("node:{}", n.id)], &["power", "gpuTemp", "smClock", "cpu"], T0 - 6 * 3600, T0 - 1800, 500, T0);
        let all = |m: &str| h.series[m].iter().flatten().copied().collect::<Vec<f32>>();
        assert!(all("power").iter().all(|w| (3.7..=5.4).contains(w)), "{}: {:?}", n.id, &all("power")[..5]);
        assert!(all("gpuTemp").iter().all(|c| (42.0..=50.0).contains(c)), "{}: {:?}", n.id, &all("gpuTemp")[..5]);
        assert!(all("smClock").iter().all(|mhz| *mhz == 208.0), "{}", n.id);
        assert!(all("cpu").iter().all(|pct| (0.3..=1.0).contains(pct)), "{}: {:?}", n.id, &all("cpu")[..5]);
    }
}

#[test]
fn demo_prefill_bursts_are_rare_and_shared_by_the_pipeline() {
    let mut demo = DemoCluster::new(&Inventory::example());
    let mut snap = demo.snapshot();
    snap.ts_ms = T0 as u64 * 1000;
    let mut nodes = BTreeMap::new();
    fill_at(&mut nodes, &snap, 0);
    let fab = |id: &str| q(&nodes[&format!("node:{id}")], &["fabricTx"], T0 - 6 * 3600, T0 - 3600, 2000, T0).series["fabricTx"].clone();
    let (a, b) = (fab(&snap.nodes[0].id), fab(&snap.nodes[1].id));
    let burst = |v: &Option<f32>| v.is_some_and(|x| x > 4e8);
    let share = a.iter().filter(|v| burst(v)).count() as f64 / a.len() as f64;
    // Like the live simulator's prefills: a few in a hundred buckets, so a chart's robust axis
    // fits the steady decode traffic and marks the bursts with ▲.
    assert!(share > 0.005 && share < 0.03, "{share}");
    // A prompt flows through every stage at once.
    assert!(a.iter().zip(&b).filter(|(x, _)| burst(x)).all(|(_, y)| burst(y)));
}

#[test]
fn backfill_fills_every_gap_and_keeps_what_was_recorded() {
    let mut demo = DemoCluster::new(&Inventory::example());
    let mut snap = demo.snapshot();
    snap.ts_ms = T0 as u64 * 1000;
    let id = format!("node:{}", snap.nodes[0].id);
    let mut nodes = BTreeMap::new();
    let mut s = Series::new();
    // A recording from an earlier run that ended two hours ago.
    for t in T0 - 3 * 3600..T0 - 2 * 3600 {
        s.push(t, &reading(42.0));
    }
    nodes.insert(id.clone(), s);
    let filled = fill_at(&mut nodes, &snap, 0);
    assert!(filled.iter().any(|(e, ..)| e == &id));
    // The Demo agents kept recording while the app was closed: those two hours are there.
    let closed = q(&nodes[&id], &["cpu"], T0 - 2 * 3600 + 60, T0 - 60, 1000, T0);
    assert_eq!(closed.tier, "10s");
    assert!(closed.series["cpu"].iter().all(|v| v.is_some()), "the time the app was closed is filled");
    // What SparkBox recorded itself is untouched.
    let kept = q(&nodes[&id], &["cpu"], T0 - 3 * 3600 + 30, T0 - 2 * 3600 - 30, 1000, T0);
    assert!(kept.series["cpu"].iter().flatten().all(|v| *v == 42.0), "{:?}", kept.series["cpu"]);
    // Time before the recording, and the day before that, are synthesized too.
    let before = q(&nodes[&id], &["cpu"], T0 - 20 * 3600, T0 - 19 * 3600, 1000, T0);
    assert!(before.series["cpu"].iter().all(|v| v.is_some()), "time before the recording is synthesized");
    let yesterday = q(&nodes[&id], &["cpu"], T0 - 47 * 3600, T0 - 46 * 3600, 100, T0);
    assert_eq!(yesterday.tier, "1m");
    assert!(yesterday.series["cpu"].iter().all(|v| v.is_some()), "48 hours back");
}

// ---- C5: charts open full

#[test]
fn demo_restart_has_no_gap() {
    let dir = std::env::temp_dir().join(format!("sb-history-restart-{}-{}", std::process::id(), fastrand::u32(..)));
    let mut demo = DemoCluster::new(&Inventory::example());
    let mut snap = demo.snapshot();
    let now = (now_ms() / 1000) * 1000;
    // The first run: three minutes, saved two hours ago when SparkBox quit.
    snap.ts_ms = now - 2 * 3_600_000 - 180_000;
    let h = History::new(Some(dir.clone()));
    for _ in 0..180 {
        h.record(&snap);
        snap.ts_ms += 1000;
    }
    h.save();
    drop(h);
    // Relaunch now: the Demo agents recorded the two hours SparkBox was closed.
    let h = History::new(Some(dir.clone()));
    let filled = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = filled.clone();
    h.on_filled(move |e| seen.lock().unwrap().extend(e));
    snap.ts_ms = now;
    h.record(&snap);
    let entity = format!("node:{}", snap.nodes[0].id);
    assert!(filled.lock().unwrap().contains(&entity), "the windows hear history://filled for {entity}");
    for (from, to, tier) in [(now - 3 * 3_600_000, now - 10_000, "10s"), (now - 900_000, now, "1s")] {
        let r = h.query(&entity, &["cpu".into(), "gpu".into(), "memUsed".into()], from, to, 2000).unwrap();
        assert_eq!(r.tier, tier);
        for m in ["cpu", "gpu", "memUsed"] {
            let gaps = r.series[m].iter().filter(|v| v.is_none()).count();
            assert_eq!(gaps, 0, "{m}: {gaps} empty buckets in the {tier} tier between the runs");
        }
    }
    // No jump at the join: the last filled second is close to the live reading.
    let r = h.query(&entity, &["memUsed".into()], now - 5_000, now + 1000, 100).unwrap();
    let vals: Vec<f32> = r.series["memUsed"].iter().flatten().copied().collect();
    let live = snap.nodes[0].mem.used as f32;
    assert!(vals.iter().all(|v| (v - live).abs() / live < 0.03), "{vals:?} vs {live}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn demo_endpoints_and_interfaces_open_full() {
    let h = History::new(None);
    let mut s = entity_snapshot();
    s.demo = true;
    let ep = crate::health::EndpointInfo {
        id: "cell1-vllm-8000".into(),
        health: "ready".into(),
        metrics: [("genTps".to_string(), 12.0), ("ttftP95".to_string(), 900.0), ("kvPct".to_string(), 8.0)].into(),
        ..Default::default()
    };
    h.record(&s);
    h.record_entities(&s, std::slice::from_ref(&ep));
    let now = s.ts_ms;
    let e = h.query("endpoint:cell1-vllm-8000", &["genTps".into(), "ttftP95".into()], now - 6 * 3_600_000, now - 10_000, 360).unwrap();
    assert!(e.series["genTps"].iter().all(|v| v.is_some()), "six hours of serving history");
    // A test token a minute, as the live canary runs: every minute of the hour has one.
    let c = h.query("endpoint:cell1-vllm-8000", &["canaryMs".into()], now - 3_600_000, now - 60_000, 59).unwrap();
    let tokens: Vec<f32> = c.series["canaryMs"].iter().flatten().copied().collect();
    assert!(tokens.len() >= 55 && tokens.iter().all(|ms| (500.0..1200.0).contains(ms)), "{tokens:?}");
    let iface = s.nodes[0].nets.iter().find(|x| x.roce.is_some()).unwrap().iface.clone();
    let n = h.query(&format!("net:{}/{iface}", s.nodes[0].id), &["ecn".into(), "crc".into()], now - 900_000, now - 1000, 900).unwrap();
    assert_eq!(n.tier, "1s");
    assert!(n.series["crc"].iter().all(|v| *v == Some(0.0)), "a healthy fabric: no CRC errors, and no gaps");
}

/// The agent's own `export`, run for real on a fake node over SSH: the last hour arrives
/// first and lands in the 10 s tier, then the hour before it.
#[tokio::test]
async fn agent_backfill_fills_the_10s_tier() {
    use crate::fake_node::{connect, fake_node, real_shell, temp};
    let dir = temp("history-agent-pull");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let root = dir.join("root");
    // The agent's install layout, under the fake node's temporary root.
    struct Dirs {
        opt: String,
        etc: String,
        state: String,
    }
    let layout = Dirs { opt: root.join("opt").display().to_string(), etc: root.join("etc").display().to_string(), state: root.join("state").display().to_string() };
    let cli = |args: &str| {
        let q = crate::live::sh_quote;
        format!("SPARKBOX_AGENT_STATE={} SPARKBOX_AGENT_CONFIG={} python3 -I {}/sparkbox_agent.py {args}", q(&layout.state), q(&layout.etc), q(&layout.opt))
    };
    for d in [&layout.opt, &layout.etc, &layout.state] {
        std::fs::create_dir_all(d).unwrap();
    }
    for (name, content) in crate::agent::program_files() {
        std::fs::write(format!("{}/{}", layout.opt, name), content).unwrap();
    }
    // Two hours the agent recorded while SparkBox was closed, every 10 s, with the agent's own History.
    let now = (now_ms() / 10_000) * 10_000;
    let seed = format!(
        r#"
import sys
sys.path.insert(0, {opt:?})
import sparkbox_agent as a
h = a.History({db:?})
for i in range(720, 0, -1):
    t = {now} - i * 10000
    h.add(t, "node:cell1", {{"cpu": 10.0 + i % 7, "gpu": 50.0, "fabricTx": 1e8}})
    h.add(t, "net:cell1/enp1s0f1np1", {{"ecn": 3.0, "crc": 0.0}})
    h.add(t, "container:cell1/vllm-head", {{"cpu": 4.0, "mem": 2e9}})
    h.add(t, "endpoint:cell1-vllm-8000", {{"genTps": 40.0, "ttftP95": 800.0}})
h.flush()
"#,
        opt = layout.opt,
        db = format!("{}/history.db", layout.state),
    );
    let status = std::process::Command::new("python3").arg("-I").arg("-c").arg(&seed).status().unwrap();
    assert!(status.success(), "seeding the agent's history");

    let (node, port) = fake_node(&home, real_shell()).await;
    let (live, _inv) = connect(&dir, &[("cell1", port)]).await;
    let h = History::new(None);
    // SparkBox's own recording of cell1 stopped two hours ago.
    h.record_entity("node:cell1", "cpu", (now - 2 * 3_600_000 - 30_000) as i64, 5.0).unwrap();
    let mut first: Vec<Vec<String>> = Vec::new();
    let run = |cmd: String| {
        let live = live.clone();
        async move { live.probe("cell1", &cmd, std::time::Duration::from_secs(60)).await.map(|o| o.stdout) }
    };
    let entities = super::backfill::pull_agent(&h, "cell1", now, cli, run, |e| first.push(e.to_vec())).await.unwrap();

    // The hour came first, on its own, then the rest up to it.
    let exports = node.ran("export");
    assert_eq!(exports.len(), 2, "{exports:?}");
    assert!(exports[0].contains(&format!("export --since {}", now - 3_600_000)), "{}", exports[0]);
    assert!(exports[1].contains(&format!("--until {}", now - 3_600_000)), "{}", exports[1]);
    assert_eq!(first.len(), 1);
    for kind in ["node:cell1", "net:cell1/enp1s0f1np1", "container:cell1/vllm-head", "endpoint:cell1-vllm-8000"] {
        assert!(first[0].iter().any(|e| e == kind), "{kind} came with the hour: {:?}", first[0]);
        assert!(entities.iter().any(|e| e == kind), "{kind}");
    }

    // A 15-minute chart has every 10 s bucket the agent covered.
    let r = h.query("node:cell1", &["cpu".into(), "gpu".into()], now - 900_000, now - 10_000, 900).unwrap();
    assert_eq!(r.tier, "10s");
    assert!(r.series["gpu"].iter().all(|v| *v == Some(50.0)), "{:?}", r.series["gpu"]);
    assert!(!r.agent_ranges.is_empty(), "marked as the agent's own data, not a closed gap");
    for (entity, metric) in [("net:cell1/enp1s0f1np1", "ecn"), ("container:cell1/vllm-head", "mem"), ("endpoint:cell1-vllm-8000", "genTps")] {
        let r = h.query(entity, &[metric.into()], now - 900_000, now - 10_000, 900).unwrap();
        assert!(r.series[metric].iter().all(|v| v.is_some()), "{entity} {metric}");
    }
    // And the hour before it, from the second export.
    let earlier = h.query("node:cell1", &["gpu".into()], now - 2 * 3_600_000 + 60_000, now - 3_600_000 - 10_000, 1000).unwrap();
    assert!(earlier.series["gpu"].iter().all(|v| v.is_some()));
    live.stop();
}

#[test]
fn round_one_node_files_migrate_to_entity_files() {
    let dir = std::env::temp_dir().join(format!("sb-history-v1-{}-{}", std::process::id(), fastrand::u32(..)));
    std::fs::create_dir_all(&dir).unwrap();
    let now = (now_ms() / 1000) as i64;
    let first = (now - 3600).div_euclid(10);
    // A version 1 file as round 1 wrote it: {"node": …}, 16 metrics, one 10 s bucket.
    let header = serde_json::json!({
        "format": "sparkbox-history", "version": 1, "node": "cell1", "metrics": METRICS, "savedAtMs": now_ms(),
        "tiers": [{ "name": "10s", "stepS": 10, "firstBucket": first, "count": 1 }]
    });
    let mut bytes = serde_json::to_vec(&header).unwrap();
    bytes.push(b'\n');
    for k in 0..N {
        for v in [k as f32, k as f32 + 0.5, k as f32 + 1.0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    std::fs::write(dir.join("cell1.sbh"), bytes).unwrap();
    let mut loaded = load_dir(&dir, metric_names);
    let s = &loaded["node:cell1"];
    let r = q(s, &["gpu"], first * 10, first * 10 + 10, 10, now);
    assert_eq!(r.series["gpu"], vec![Some(1.5)], "metric 1 (gpu): min 1, avg 1.5, max 2");
    assert!(s.changed(), "a version 1 file is written again as version 2");
    save_dir(&dir, &mut loaded, &OsSync, false);
    assert!(dir.join(file_name("node:cell1")).exists());
    assert!(!dir.join("cell1.sbh").exists(), "the round-one file is replaced");
    assert!(load_dir(&dir, metric_names).contains_key("node:cell1"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn entity_snapshot() -> crate::model::ClusterSnapshot {
    let mut demo = DemoCluster::new(&Inventory::example());
    let mut s = demo.snapshot();
    s.ts_ms = (now_ms() / 1000) * 1000;
    let x = s.nodes[0].nets.iter_mut().find(|x| x.role == crate::model::NetRole::Fabric).unwrap();
    x.roce = Some(crate::model::RoceCounters { ecn_marked: Some(1000), out_of_sequence: Some(10), cnp_sent: None, ..Default::default() });
    s
}

#[test]
fn every_entity_kind_records_and_answers_queries() {
    let h = History::new(None);
    let mut s = entity_snapshot();
    let node = s.nodes[0].clone();
    let iface = node.nets.iter().find(|x| x.roce.is_some()).unwrap().iface.clone();
    let ep = crate::health::EndpointInfo {
        id: "cell1-vllm-8000".into(),
        health: "ready".into(),
        metrics: [("genTps".to_string(), 42.0), ("ttftP95".to_string(), 850.0), ("kvPct".to_string(), 31.0)].into(),
        ..Default::default()
    };
    let base = s.ts_ms;
    for i in 0..5 {
        if let Some(r) = s.nodes[0].nets.iter_mut().find(|x| x.iface == iface).and_then(|x| x.roce.as_mut()) {
            r.ecn_marked = Some(1000 + i * 200);
            r.out_of_sequence = Some(10);
        }
        h.record(&s);
        h.record_entities(&s, std::slice::from_ref(&ep));
        s.ts_ms += 1000;
    }
    let (from, to) = (base, s.ts_ms);
    // Rates start with the second reading (they need a "before").
    let net = h.query(&format!("net:{}/{iface}", node.id), &["ecn".into(), "oos".into(), "cnpSent".into()], base + 1000, to, 100).unwrap();
    assert_eq!(net.tier, "1s");
    assert_eq!(net.series["ecn"].iter().flatten().copied().collect::<Vec<_>>(), vec![200.0; 4], "ECN marks per second from the counter");
    assert_eq!(net.series["oos"].iter().flatten().copied().collect::<Vec<_>>(), vec![0.0; 4]);
    assert!(net.series["cnpSent"].iter().all(|v| v.is_none()), "a counter the driver doesn't have stays a gap, not 0");
    let e = h.query("endpoint:cell1-vllm-8000", &["genTps".into(), "ttftP95".into(), "waiting".into()], from, to, 100).unwrap();
    assert_eq!(e.series["genTps"].iter().flatten().copied().collect::<Vec<_>>(), vec![42.0; 5]);
    assert!(e.series["waiting"].iter().all(|v| v.is_none()));
    let c = node.containers.iter().find(|c| c.state == "running").unwrap();
    let cq = h.query(&format!("container:{}/{}", node.id, c.name), &["cpu".into(), "mem".into()], from, to, 100).unwrap();
    assert_eq!(cq.series["mem"].iter().flatten().count(), 5);
    let sw = &s.switches[0];
    let p = sw.ports.iter().find(|p| p.running).unwrap();
    let pq = h.query(&format!("port:{}/{}", sw.id, p.name), &["rxBps".into(), "errors".into(), "rxPowerDbm".into()], from, to, 100).unwrap();
    assert_eq!(pq.series["rxBps"].iter().flatten().count(), 5);
    assert!(h.entities().iter().any(|e| e.starts_with("port:")));
    // Unknown kinds and metrics are refused, by name.
    assert!(h.query("gpu:cell1", &["x".into()], from, to, 10).unwrap_err().contains("not \"gpu:cell1\""));
    assert!(h.query("endpoint:x", &["cpu".into()], from, to, 10).unwrap_err().contains("Unknown metric \"cpu\""));
    // record_entity, the contract's single-value door.
    h.record_entity("port:crs804/qsfp56-dd-1-1", "rxPowerDbm", (s.ts_ms + 1000) as i64, -2.5).unwrap();
    assert!(h.record_entity("port:crs804/x", "nope", 0, 1.0).is_err());
}

#[test]
fn a_test_token_shares_its_second_with_the_endpoints_row() {
    let h = History::new(None);
    let mut s = entity_snapshot();
    let id = "cell1-vllm-8000";
    let ep = crate::health::EndpointInfo { id: id.into(), health: "ready".into(), metrics: [("genTps".to_string(), 42.0)].into(), ..Default::default() };
    h.record_entities(&s, std::slice::from_ref(&ep));
    s.ts_ms += 1000;
    let from = s.ts_ms;
    for i in 0..3 {
        // The one-token check's time lands first, in the second the endpoint's row is for.
        h.record_entity(&format!("endpoint:{id}"), "canaryMs", s.ts_ms as i64, 640.0 + i as f32).unwrap();
        h.record_entities(&s, std::slice::from_ref(&ep));
        s.ts_ms += 1000;
    }
    let q = h.query(&format!("endpoint:{id}"), &["genTps".into(), "canaryMs".into()], from, s.ts_ms, 100).unwrap();
    assert_eq!(q.series["genTps"].iter().flatten().count(), 3, "no second of serving numbers lost: {:?}", q.series);
    assert_eq!(q.series["canaryMs"].iter().flatten().copied().collect::<Vec<_>>(), vec![640.0, 641.0, 642.0]);
}

#[test]
fn the_budget_drops_idle_entities_then_the_oldest_data() {
    let now = (now_ms() / 1000) as i64;
    let mut series = BTreeMap::new();
    let mut busy = Series::new();
    for t in (now - 20 * 86_400..now).step_by(60) {
        busy.push(t, &reading(1.0));
    }
    let mut gone = Series::with_metrics(CONTAINER_METRICS.iter().map(|m| m.to_string()).collect());
    for t in (now - 6 * 86_400..now - 5 * 86_400).step_by(60) {
        gone.push(t, &[1.0, 2.0]);
    }
    series.insert("node:cell1".to_string(), busy);
    series.insert("container:cell1/old-job".to_string(), gone);
    let size = |s: &BTreeMap<String, Series>| s.iter().map(|(e, x)| x.encode(e, now).len()).sum::<usize>();
    assert!(series.iter().all(|(e, x)| x.encoded_len(e, now) == x.encode(e, now).len()), "the budget's sizes are the files' sizes");
    let total = size(&series);
    let gone_size = series["container:cell1/old-job"].encode("container:cell1/old-job", now).len();
    let dropped = enforce_budget(&mut series, total - gone_size / 2, now);
    assert_eq!(dropped, vec!["container:cell1/old-job".to_string()], "the entity that stopped reporting goes first");
    assert!(series.contains_key("node:cell1"));
    // Still over: the oldest data of what's left goes, the newest stays.
    let before = series["node:cell1"].oldest_s(now).unwrap();
    let budget = size(&series) / 2;
    enforce_budget(&mut series, budget, now);
    assert!(size(&series) <= budget, "{} > {budget}", size(&series));
    assert!(series["node:cell1"].oldest_s(now).unwrap() > before, "the oldest buckets went");
    let recent = q(&series["node:cell1"], &["cpu"], now - 3600, now, 100, now);
    assert!(recent.series["cpu"].iter().flatten().count() > 0, "the last hour is kept");
}

#[test]
fn a_reading_too_old_for_a_ring_never_overwrites_newer_data() {
    let mut s = Series::new();
    s.push(T0, &reading(5.0));
    // Exactly one lap of the 10 s ring earlier: the same slot.
    s.push_buckets(T0 - 8640 * 10, &reading(9.0));
    let r = q(&s, &["cpu"], T0, T0 + 10, 10, T0 + 10);
    assert!(r.series["cpu"].contains(&Some(5.0)), "{:?}", r.series["cpu"]);
}

// ── r4-craft ──

/// A Live snapshot of the Demo cluster's shape at `ts_ms`, every node online.
fn live_snapshot(ts_ms: u64) -> crate::model::ClusterSnapshot {
    let mut demo = DemoCluster::new(&Inventory::example());
    let mut s = demo.snapshot();
    s.demo = false;
    s.ts_ms = ts_ms;
    s
}

#[test]
fn a_restart_keeps_the_minutes_before_it_for_the_15_minute_charts() {
    let dir = std::env::temp_dir().join(format!("sb-history-restart-{}", now_ms()));
    let now = (now_ms() / 1000) as i64;
    let restart = now - 20;
    {
        // Ten minutes of Live, then SparkBox goes (Quit, or SIGTERM: both save).
        let h = History::new(Some(dir.clone()));
        for t in (restart - 600)..restart {
            h.record(&live_snapshot(t as u64 * 1000));
        }
        h.save();
    }
    // Back after 20 s: the 1 s ring starts empty, the 10 s tier comes from disk.
    let h = History::new(Some(dir.clone()));
    for t in now..=now {
        h.record(&live_snapshot(t as u64 * 1000));
    }
    let node = format!("node:{}", live_snapshot(0).nodes[0].id);
    let r = h.query(&node, &["cpu".into()], (now - 900) as u64 * 1000, now as u64 * 1000 + 1000, 900).unwrap();
    let before: Vec<u64> = r.ts.iter().zip(&r.series["cpu"]).filter(|(t, v)| v.is_some() && (**t / 1000) as i64 <= restart).map(|(t, _)| *t).collect();
    let covered = before.len() as u64 * r.step_ms / 1000;
    assert!(covered >= 9 * 60, "the 15-minute query has {covered} s from before the restart ({} tier)", r.tier);
    // And the recording since the restart is in the same answer.
    assert!(r.ts.iter().zip(&r.series["cpu"]).any(|(t, v)| v.is_some() && (*t / 1000) as i64 >= now - 10), "{:?}", r.series["cpu"].iter().rev().take(4).collect::<Vec<_>>());
    // One "History starts" for every entity of this source: when the first node was recorded
    // (to the 10 s the saved tier keeps), not the 15-minute bucket it fell in.
    let starts = r.starts_at_ms.expect("a start") as i64 / 1000;
    assert!((restart - 600 - starts).abs() < 10, "history starts {} s off", restart - 600 - starts);
    let other = h.query("endpoint:never-seen", &["genTps".into()], (now - 900) as u64 * 1000, now as u64 * 1000, 90).unwrap();
    assert_eq!(other.starts_at_ms, r.starts_at_ms, "every window says the same start");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn history_is_saved_every_minute() {
    assert_eq!(SAVE_EVERY_S, 60);
}

#[test]
fn a_reading_from_the_other_source_is_refused() {
    let h = History::new(None);
    let now = now_ms();
    h.record(&live_snapshot(now / 1000 * 1000));
    // A Demo test token that arrives after the switch to Live never lands in Live history.
    let err = h.record_entity_in(true, "endpoint:cell1-vllm-8000", "canaryMs", now as i64, 640.0).unwrap_err();
    assert!(err.contains("demo"), "{err}");
    h.record_entity_in(false, "endpoint:cell1-vllm-8000", "canaryMs", now as i64, 640.0).unwrap();
    // The one-source form records into whatever is recording now.
    h.record_entity("endpoint:cell1-vllm-8000", "canaryMs", now as i64 + 1000, 650.0).unwrap();
    let r = h.query("endpoint:cell1-vllm-8000", &["canaryMs".into()], now - 60_000, now + 60_000, 120).unwrap();
    // Only the two Live readings (the refused Demo one would have been 640 in a third second).
    let got: Vec<f32> = r.series["canaryMs"].iter().flatten().copied().collect();
    assert!(!got.is_empty() && got.iter().all(|v| (640.0..=650.0).contains(v)), "{got:?}");
    let raw = h.query("endpoint:cell1-vllm-8000", &["canaryMs".into()], now - 2_000, now + 2_000, 10).unwrap();
    assert!(raw.series["canaryMs"].iter().flatten().count() <= 2, "{:?}", raw.series["canaryMs"]);
}

#[test]
fn ten_saved_minutes_answer_a_15_minute_chart_from_the_10s_tier() {
    // Ten minutes in the bucket tiers only (as loaded after a restart), then a few live seconds:
    // the 15 min tier's first bucket starts before the chart does, but it holds only those ten
    // minutes, so the chart comes from the 10 s tier, finely, not as one or two 15-minute points.
    let mut s = Series::new();
    for t in (T0 - 620..T0 - 20).step_by(10) {
        s.push_buckets(t, &reading(5.0));
    }
    for t in T0 - 3..=T0 {
        s.push(t, &reading(6.0));
    }
    let r = q(&s, &["cpu"], T0 - 900, T0, 900, T0);
    assert_eq!(r.tier, "10s");
    assert!(r.series["cpu"].iter().flatten().count() >= 58, "{:?}", r.series["cpu"]);
}

// ---- R3-FOOT: a save copies under the lock and writes outside it, only what changed

/// Every sync a save asked for, by kind.
#[derive(Default)]
struct Syncs(std::sync::Mutex<Vec<Flush>>);

impl Syncs {
    fn count(&self, how: Flush) -> usize {
        self.0.lock().unwrap().iter().filter(|h| **h == how).count()
    }
}

impl Syncer for Syncs {
    fn sync(&self, _file: &std::fs::File, how: Flush) -> std::io::Result<()> {
        self.0.lock().unwrap().push(how);
        Ok(())
    }
}

/// A disk whose first file write waits (in its fsync) until the test lets it go.
struct Held {
    writing: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Syncer for Held {
    fn sync(&self, _file: &std::fs::File, how: Flush) -> std::io::Result<()> {
        if how == Flush::Plain {
            if let Some(tx) = self.writing.lock().unwrap().take() {
                let _ = tx.send(());
                let _ = self.release.lock().unwrap().recv();
            }
        }
        Ok(())
    }
}

fn history_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sb-history-{name}-{}-{}", std::process::id(), fastrand::u32(..)));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `t`, moved back when it's near midnight UTC: the minutes a test saves around it then land in
/// one day's segment, whatever time the test runs.
fn clear_of_midnight(t: i64) -> i64 {
    let into_day = t.rem_euclid(86_400);
    if into_day < 1800 {
        t - into_day - 3600
    } else if into_day > 84_600 {
        t - 3600
    } else {
        t
    }
}

/// Seconds `from..to` of `snap` (every node online unless the snapshot says otherwise) through
/// publish's path: `record` and `record_entities`.
fn record_span(h: &History, snap: &crate::model::ClusterSnapshot, from: i64, to: i64) {
    let mut s = snap.clone();
    for t in from..to {
        s.ts_ms = t as u64 * 1000;
        h.record(&s);
        h.record_entities(&s, &[]);
    }
}

#[test]
fn a_save_takes_only_the_buckets_a_reading_changed() {
    let mut s = Series::new();
    assert!(s.take_delta().is_none(), "nothing recorded");
    for t in T0..T0 + 25 {
        s.push(t, &reading(1.0));
    }
    let delta = s.take_delta().expect("readings changed it");
    // 25 s from a 15-minute boundary: three 10 s buckets, one minute, one quarter hour.
    assert_eq!(delta.counts(), [3, 1, 1]);
    assert!(s.take_delta().is_none(), "taken once until the next reading");
    s.push(T0 + 25, &[f32::NAN; N]);
    assert!(s.take_delta().is_none(), "an offline second isn't a reading");
    s.push(T0 + 26, &reading(2.0));
    let next = s.take_delta().unwrap();
    assert_eq!(next.counts(), [1, 1, 1], "the bucket the new reading went to, in each tier");
    s.put_back(next.bits());
    assert!(s.take_delta().is_some(), "a delta that couldn't be appended is taken again");
    // A compaction's copy holds every bucket whole, takes the changed bits, and is the next epoch.
    s.push(T0 + 40, &reading(3.0));
    let (copy, bits) = s.copy_for_base();
    assert!(!s.dirty() && copy.epoch == s.epoch + 1);
    let body = |b: Vec<u8>| b[b.iter().position(|&c| c == b'\n').unwrap()..].to_vec();
    assert_eq!(body(copy.encode("node:x", T0 + 41)), body(s.encode("node:x", T0 + 41)), "the copy writes the same buckets");
    s.put_back(&bits);
    assert!(s.dirty(), "a base that couldn't be written leaves its buckets to be appended");
}

#[test]
fn publishing_isnt_held_up_while_a_save_is_writing() {
    let dir = history_dir("held");
    let h = std::sync::Arc::new(History::new(Some(dir.clone())));
    let now = (now_ms() / 1000) as i64;
    let snap = live_snapshot(0);
    record_span(&h, &snap, now - 120, now - 60);
    let (writing_tx, writing_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    h.set_syncer(std::sync::Arc::new(Held { writing: std::sync::Mutex::new(Some(writing_tx)), release: std::sync::Mutex::new(release_rx) }));
    let saver = {
        let h = h.clone();
        std::thread::spawn(move || h.save())
    };
    writing_rx.recv_timeout(std::time::Duration::from_secs(20)).expect("the save is writing its first file");
    // Publish's path, while that file is still being written.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let recorder = {
        let (h, snap) = (h.clone(), snap.clone());
        std::thread::spawn(move || {
            record_span(&h, &snap, now - 60, now - 50);
            let _ = done_tx.send(());
        })
    };
    let recorded = done_rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok();
    release_tx.send(()).unwrap();
    let written = saver.join().unwrap();
    recorder.join().unwrap();
    assert!(recorded, "record() and record_entities() waited for a save that was writing");
    assert!(written.entities.iter().any(|e| e.starts_with("node:")), "{written:?}");
    // Those ten seconds reach the disk too: with that save (entities it hadn't copied yet) or the next.
    h.save();
    let node = format!("node:{}", snap.nodes[0].id);
    let saved = load_dir(&dir.join("live"), metric_names);
    let r = q(&saved[&node], &["cpu"], now - 60, now - 50, 100, now);
    assert_eq!(r.tier, "10s");
    assert!(!r.series["cpu"].is_empty() && r.series["cpu"].iter().all(Option::is_some), "the 10 s buckets of those seconds: {:?}", r.series["cpu"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_minute_save_appends_only_for_the_entities_with_new_readings() {
    let dir = history_dir("changed");
    let h = History::new(Some(dir.clone()));
    let now = clear_of_midnight((now_ms() / 1000) as i64);
    let mut snap = live_snapshot(0);
    let (a, b) = (format!("node:{}", snap.nodes[0].id), format!("node:{}", snap.nodes[1].id));
    let gone = snap.nodes[1].containers.iter().find(|c| c.state == "running").map(|c| format!("container:{}/{}", snap.nodes[1].id, c.name)).expect("a running container");
    record_span(&h, &snap, now - 120, now - 90);
    let first = h.save_at(false, now - 90);
    for e in [&a, &b, &gone] {
        assert!(first.appended.contains(e), "{e} in {:?}", first.appended);
    }
    assert!(first.compacted.is_empty(), "the minute save writes no base");
    let live = dir.join("live");
    let segment = |e: &str| live.join(segment_name(e, 0, &utc_day(now - 90)));
    let size = |e: &str| std::fs::metadata(segment(e)).map(|m| m.len()).unwrap_or(0);
    assert!(!live.join(file_name(&a)).exists(), "no base file until a compaction");
    let (a_before, b_before, gone_before) = (size(&a), size(&b), size(&gone));
    assert!(a_before > 0 && b_before > 0 && gone_before > 0);

    // The second node goes offline: no readings from it (nor from its containers) for the next half minute.
    snap.nodes[1].status = crate::model::LinkStatus::Offline;
    record_span(&h, &snap, now - 90, now - 60);
    let second = h.save_at(false, now - 60);
    assert!(second.appended.contains(&a));
    assert!(!second.entities.contains(&b) && !second.entities.contains(&gone), "{:?}", second.entities);
    assert!(size(&a) > a_before, "a node with new readings is appended to");
    assert_eq!((size(&b), size(&gone)), (b_before, gone_before), "an entity with no new readings keeps its file as it was");
    // Half a minute of a node is three or four 10 s buckets, a minute or two and a quarter hour:
    // a few kilobytes, not the node's whole history.
    assert!(size(&a) - a_before < 2000, "{} bytes appended", size(&a) - a_before);
    let grown: u64 = std::fs::read_dir(&live).unwrap().flatten().map(|e| e.metadata().unwrap().len()).sum::<u64>();
    assert_eq!(first.bytes + second.bytes, grown, "every byte written is counted");

    // Nothing new at all: nothing written.
    assert_eq!(h.save_at(false, now - 30), Written::default());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_minute_save_has_one_barrier_and_only_the_final_save_flushes_fully() {
    let dir = history_dir("syncs");
    let h = History::new(Some(dir.clone()));
    let syncs = std::sync::Arc::new(Syncs::default());
    h.set_syncer(syncs.clone());
    let now = (now_ms() / 1000) as i64;
    let snap = live_snapshot(0);
    record_span(&h, &snap, now - 60, now - 50);
    let minute = h.save();
    assert!(minute.entities.len() > 4, "{minute:?}");
    assert_eq!((syncs.count(Flush::Plain), syncs.count(Flush::Barrier), syncs.count(Flush::Full)), (minute.entities.len(), 1, 0), "an fsync per file, one barrier, no F_FULLFSYNC");

    syncs.0.lock().unwrap().clear();
    record_span(&h, &snap, now - 50, now - 40);
    let last = h.save_final();
    assert_eq!((syncs.count(Flush::Plain), syncs.count(Flush::Barrier), syncs.count(Flush::Full)), (last.entities.len(), 1, 1), "Quit and SIGTERM flush the drive's cache once");

    // Nothing new: the minute save does nothing, the final save still flushes what's behind the barriers.
    syncs.0.lock().unwrap().clear();
    h.save();
    assert!(syncs.0.lock().unwrap().is_empty());
    h.save_final();
    assert_eq!(*syncs.0.lock().unwrap(), vec![Flush::Full]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sleep_and_stale_reconnect_gaps_survive_save_and_reopen_without_invented_readings() {
    let dir = history_dir("sleep-reconnect");
    let start = clear_of_midnight((now_ms() / 1000) as i64 / 10 * 10 - 7200);
    let mut snap = live_snapshot(0);
    assert_eq!(snap.nodes.len(), 4);
    let ask = |h: &History, id: &str| h.query(&format!("node:{id}"), &["cpu".into(), "memUsed".into()], start as u64 * 1000, (start + 80) as u64 * 1000, 80).unwrap();
    let before = {
        let h = History::new(Some(dir.clone()));
        for offset in 0..80 {
            // No Mac collection during 26 s asleep. On wake, SSH reconnects;
            // the fourth node's retry takes longer. Old snapshots remain stale.
            if (10..36).contains(&offset) { continue; }
            snap.ts_ms = (start + offset) as u64 * 1000;
            for (i, n) in snap.nodes.iter_mut().enumerate() {
                n.stale = offset >= 36 && offset < if i == 3 { 41 } else { 38 };
                n.cpu.total_pct = if offset < 10 { 20.0 } else { 40.0 };
            }
            h.record(&snap);
        }
        h.save_final();
        snap.nodes.iter().map(|n| ask(&h, &n.id)).collect::<Vec<_>>()
    };
    let h = History::new(Some(dir.clone()));
    snap.ts_ms = (start + 90) as u64 * 1000;
    snap.nodes.iter_mut().for_each(|n| n.stale = true);
    h.record(&snap); // Load the saved Live history without adding valid readings.
    for (i, n) in snap.nodes.iter().enumerate() {
        let after = ask(&h, &n.id);
        assert_eq!(after.tier, "10s");
        assert!(after.agent_ranges.is_empty(), "no agent data was supplied");
        assert_eq!((&after.ts, &after.series, &after.min, &after.max), (&before[i].ts, &before[i].series, &before[i].min, &before[i].max));
        let missing: Vec<_> = after.ts.iter().zip(&after.series["cpu"]).filter_map(|(t, v)| v.is_none().then_some(*t / 1000 - start as u64)).collect();
        assert_eq!(missing, if i == 3 { vec![10, 20, 30] } else { vec![10, 20] });
        assert_eq!(after.series["cpu"].first(), Some(&Some(20.0)));
        assert_eq!(after.series["cpu"].last(), Some(&Some(40.0)));
    }
    assert_eq!(h.save(), Written::default(), "stale reconnects never rewrite historical readings");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_saved_history_loads_back_the_same_and_isnt_rewritten() {
    let dir = history_dir("round-trip");
    let now = (now_ms() / 1000) as i64;
    let snap = live_snapshot(0);
    let node = format!("node:{}", snap.nodes[0].id);
    // Six hours: past the 1 s ring, so both answers come from the saved 10 s tier.
    let ask = |h: &History| h.query(&node, &["cpu".into(), "memUsed".into()], (now - 6 * 3600) as u64 * 1000, (now - 30) as u64 * 1000, 2200).unwrap();
    let before = {
        let h = History::new(Some(dir.clone()));
        record_span(&h, &snap, now - 600, now - 30);
        h.save_final();
        ask(&h)
    };
    let h = History::new(Some(dir.clone()));
    // The first Live snapshot loads Live's files.
    let mut s = snap.clone();
    s.ts_ms = (now as u64 - 20) * 1000;
    for n in &mut s.nodes {
        n.status = crate::model::LinkStatus::Offline;
    }
    h.record(&s);
    let after = ask(&h);
    assert_eq!((before.tier.as_str(), after.tier.as_str()), ("10s", "10s"));
    assert!(after.series["cpu"].iter().flatten().count() >= 55, "{:?}", after.series["cpu"].iter().flatten().count());
    assert_eq!((after.ts.clone(), after.series.clone()), (before.ts.clone(), before.series.clone()));
    assert_eq!((after.min, after.max), (before.min, before.max));
    assert_eq!(h.save(), Written::default(), "what was loaded, and offline seconds since, aren't written again");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The version 2 format both ways, against a p1-r3 build's own test of the same name
/// (SB_HISTORY_COMPAT_DIR=… cargo test --lib history_files_both_ways -- --ignored --nocapture):
/// writes `written-by-p1-r4` (a compacted base, segEpoch 1, holding a fixed history, and a
/// segment after it), for p1-r3's loader to read as `written-by-p1-r2`; checks one p1-r3 wrote
/// (`written-by-r3`) loads here; and loads `live-p1-r3`, a copy of base files a p1-r3 dev build
/// wrote in Live, when there is one.
#[test]
#[ignore]
fn history_files_both_ways() {
    let Ok(root) = std::env::var("SB_HISTORY_COMPAT_DIR") else { return };
    let root = std::path::PathBuf::from(root);
    // A fixed history: one value per 10 s bucket of the last two hours, from the bucket number.
    let now = (now_ms() / 1000) as i64;
    let fill = |s: &mut Series| {
        for t in (now - 7200..now - 60).step_by(10) {
            s.push_buckets(t, &[(t / 10 % 97) as f32; N]);
        }
    };
    // Every bucket a file holds in those two hours has its value (the files of another run
    // were written a little earlier, so their two hours started a little earlier).
    let check = |dir: &std::path::Path| {
        let loaded = load_dir(dir, metric_names);
        let s = loaded.get("node:cell1").expect("node:cell1 loads");
        let r = q(s, &["cpu", "smClock"], now - 7200, now - 60, 10_000, now);
        assert_eq!(r.tier, "10s");
        let got: Vec<(u64, f32)> = r.ts.iter().zip(&r.series["cpu"]).filter_map(|(t, v)| Some((*t, (*v)?))).collect();
        assert!(got.iter().all(|(t, v)| *v == (*t as i64 / 1000 / 10 % 97) as f32), "every value is its bucket's");
        assert!(got.len() >= 600, "{} buckets", got.len());
        eprintln!("loaded {} buckets from {}", got.len(), dir.display());
    };
    let mine = root.join("written-by-p1-r4");
    let mut series = BTreeMap::new();
    let mut s = Series::new();
    fill(&mut s);
    series.insert("node:cell1".to_string(), s);
    save_dir(&mine, &mut series, &OsSync, true);
    // Then a minute save's segment: the last 50 s, which an older build doesn't read.
    let s = series.get_mut("node:cell1").unwrap();
    for t in (now - 50..now).step_by(10) {
        s.push_buckets(t, &[555.0; N]);
    }
    let headed = std::sync::Mutex::new(std::collections::HashSet::new());
    let mut a = super::store::Appender::new(&mine, &OsSync, &headed, now).unwrap();
    a.append("node:cell1", &s.metrics.clone(), s.epoch, &super::store::delta_record(&s.take_delta().unwrap(), now_ms())).unwrap();
    check(&mine);
    let tail = q(&load_dir(&mine, metric_names)["node:cell1"], &["cpu"], now - 50, now, 100, now);
    assert_eq!(tail.series["cpu"].iter().flatten().copied().collect::<Vec<f32>>(), vec![555.0; 5], "this build reads its segment");
    let theirs = root.join("written-by-r3");
    if theirs.exists() {
        check(&theirs);
    }
    let real = root.join("live-p1-r3");
    if real.exists() {
        let names: Vec<String> = std::fs::read_dir(&real).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".sbh")).collect();
        let loaded = load_dir(&real, metric_names);
        assert_eq!(loaded.len(), names.len(), "every base file p1-r3 wrote loads");
        assert!(!std::fs::read_dir(&real).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".corrupt-")));
        for (e, s) in &loaded {
            assert!(!s.buckets(now).is_empty(), "{e} has buckets");
            assert_eq!(s.epoch, 0);
        }
        eprintln!("loaded {} entities p1-r3 wrote in Live from {}", loaded.len(), real.display());
    }
}

/// Bytes written and time per save on a 4-node fixture day, and how long `record` waits while a
/// save runs (the report's before/after numbers; the P1.38 soak measures the real ones).
/// cargo test --lib history_save_numbers -- --ignored --nocapture --test-threads=1
#[test]
#[ignore]
fn history_save_numbers_on_a_four_node_day() {
    let dir = history_dir("numbers");
    let h = std::sync::Arc::new(History::new(Some(dir.clone())));
    let now = (now_ms() / 1000) as i64;
    let snap = live_snapshot(0);
    let eps: Vec<crate::health::EndpointInfo> = (0..2)
        .map(|i| crate::health::EndpointInfo {
            id: format!("cell{}-vllm-8000", i + 1),
            health: "ready".into(),
            metrics: [("genTps".to_string(), 40.0), ("ttftP95".to_string(), 800.0), ("kvPct".to_string(), 30.0)].into(),
            ..Default::default()
        })
        .collect();
    let span = |h: &History, from: i64, to: i64| {
        let mut s = snap.clone();
        for t in from..to {
            s.ts_ms = t as u64 * 1000;
            h.record(&s);
            h.record_entities(&s, &eps);
        }
    };
    let live = dir.join("live");
    let measure = |label: &str, full: bool| {
        let before = files(&live);
        let t = std::time::Instant::now();
        let w = if full { h.save_final() } else { h.save() };
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let after = files(&live);
        let rewritten = after.keys().filter(|k| before.get(*k).map(|b| b.0) != after.get(*k).map(|a| a.0)).count();
        let bytes = growth(&before, &after);
        eprintln!("NUMBERS {label}: {:.1} ms, {} appended, {} bases written, {} of {} files new or replaced, {} bytes ({:.2} MB)", ms, w.appended.len(), w.compacted.len(), rewritten, after.len(), bytes, bytes as f64 / 1e6);
    };
    span(&h, now - 86_400 - 180, now - 180);
    measure("a day, then the first save (Quit: every base written)", true);
    span(&h, now - 180, now - 120);
    measure("minute save, every node reporting", false);
    let mut quiet = snap.clone();
    quiet.nodes[3].status = crate::model::LinkStatus::Offline;
    for c in quiet.nodes[2].containers.iter_mut() {
        c.state = "exited".into();
    }
    {
        let mut s = quiet.clone();
        for t in now - 120..now - 60 {
            s.ts_ms = t as u64 * 1000;
            h.record(&s);
            h.record_entities(&s, &eps);
        }
    }
    measure("minute save, one node offline and one node's containers stopped", false);
    // How long publish's path waits while a minute save runs (one record every 20 ms; publish
    // runs once a second, so this is 50 times the real rate).
    span(&h, now - 60, now - 30);
    let saver = {
        let h = h.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let _ = h.save();
            t.elapsed()
        })
    };
    let mut worst = std::time::Duration::ZERO;
    let mut s = snap.clone();
    let mut t = now - 30;
    while !saver.is_finished() {
        s.ts_ms = t as u64 * 1000;
        let at = std::time::Instant::now();
        h.record(&s);
        h.record_entities(&s, &eps);
        worst = worst.max(at.elapsed());
        t = (t + 1).min(now - 1);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let took = saver.join().unwrap();
    eprintln!("NUMBERS record() during a {:.1} ms save: longest wait {:.2} ms", took.as_secs_f64() * 1000.0, worst.as_secs_f64() * 1000.0);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- R4-LIVE: minute saves append, compactions rewrite bases (decision (a))

/// Every bucket each entity of `h`'s current source holds (`Series::buckets`), entities with none left out.
fn dump(h: &History, now_s: i64) -> BTreeMap<String, Vec<(usize, i64, Vec<u32>)>> {
    h.inner.lock().unwrap().series.iter().map(|(e, s)| (e.clone(), s.buckets(now_s))).filter(|(_, b)| !b.is_empty()).collect()
}

/// What the files in `dir` load as, the same way.
fn dump_dir(dir: &std::path::Path, now_s: i64) -> BTreeMap<String, Vec<(usize, i64, Vec<u32>)>> {
    super::store::load(dir, metric_names, now_s).series.iter().map(|(e, s)| (e.clone(), s.buckets(now_s))).filter(|(_, b)| !b.is_empty()).collect()
}

/// Two ready vLLM endpoints, so every kind of entity reports (round 3's fixture).
fn two_endpoints() -> Vec<crate::health::EndpointInfo> {
    (0..2)
        .map(|i| crate::health::EndpointInfo {
            id: format!("cell{}-vllm-8000", i + 1),
            health: "ready".into(),
            metrics: [("genTps".to_string(), 40.0), ("ttftP95".to_string(), 800.0), ("kvPct".to_string(), 30.0)].into(),
            ..Default::default()
        })
        .collect()
}

/// One reading of `snap` at second `t`, through publish's path.
fn record_at(h: &History, snap: &mut crate::model::ClusterSnapshot, eps: &[crate::health::EndpointInfo], t: i64) {
    snap.ts_ms = t as u64 * 1000;
    h.record(snap);
    h.record_entities(snap, eps);
}

/// Each file in a folder: (inode, length).
fn files(dir: &std::path::Path) -> std::collections::HashMap<String, (u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::read_dir(dir).map(|r| r.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).map(|e| {
        let m = e.metadata().unwrap();
        (e.file_name().to_string_lossy().into_owned(), (m.ino(), m.len()))
    }).collect()).unwrap_or_default()
}

/// The bytes written between two listings of a folder: new or replaced files whole, grown files by their growth.
fn growth(before: &std::collections::HashMap<String, (u64, u64)>, after: &std::collections::HashMap<String, (u64, u64)>) -> u64 {
    after.iter().map(|(name, (ino, len))| match before.get(name) {
        Some((i, l)) if i == ino => len.saturating_sub(*l),
        _ => *len,
    }).sum()
}

#[test]
fn a_day_of_minute_saves_writes_well_under_a_gigabyte() {
    let dir = history_dir("day");
    let live = dir.join("live");
    let h = History::new(Some(dir.clone()));
    // Syncs counted, not waited for: the bytes are what's measured.
    let syncs = std::sync::Arc::new(Syncs::default());
    h.set_syncer(syncs.clone());
    let mut snap = live_snapshot(0);
    let eps = two_endpoints();
    let end = (now_ms() / 1000) as i64 / 60 * 60 - 60;
    let start = end - 86_400;
    // Round 3's scale: a day of every entity already recorded and in base files. Round 3's
    // minute save rewrote all of this every minute.
    for t in (start - 86_400..start).step_by(10) {
        record_at(&h, &mut snap, &eps, t);
    }
    let whole = h.save_at(true, start);
    let entities = whole.compacted.len();
    for kind in ["node:", "endpoint:", "container:", "port:", "net:"] {
        assert!(whole.compacted.iter().any(|e| e.starts_with(kind)), "{kind} reports: {:?}", whole.compacted);
    }
    // The last compaction an hour before the second minute save (the first one, right after
    // the whole save, would find no segments yet), so the day has 24 that compact something.
    h.saving.lock().unwrap().compacted_s = Some(start + 83 - COMPACT_EVERY_S);

    // The day: a reading every 5 s from every entity, a minute save 23 s into each minute (so
    // a 10 s bucket and a minute bucket straddle every save, as they do at a real timer's offset).
    let (mut counted, mut measured, mut compactions, mut compacted, mut most) = (0u64, 0u64, 0, 0, 0u64);
    let mut t = start;
    for m in 0..1440 {
        let at = start + 23 + 60 * m;
        while t < at {
            record_at(&h, &mut snap, &eps, t);
            t += 5;
        }
        let before = files(&live);
        let w = h.save_at(false, at);
        assert!(w.failed.is_empty(), "{w:?}");
        assert_eq!(w.bytes, growth(&before, &files(&live)), "minute {m}: the save's own count is what the folder grew by");
        counted += w.bytes;
        measured += growth(&before, &files(&live));
        most = most.max(w.bytes);
        if !w.compacted.is_empty() {
            compactions += 1;
            compacted += w.compacted.len();
        }
    }
    while t < end {
        record_at(&h, &mut snap, &eps, t);
        t += 5;
    }
    let before = files(&live);
    let last = h.save_at(true, end);
    counted += last.bytes;
    measured += growth(&before, &files(&live));
    assert_eq!(counted, measured);
    assert_eq!(compactions, 24, "one compaction an hour");
    assert!(compacted >= entities, "every entity compacted about once a day: {compacted} compactions of {entities} entities");
    let mb = counted as f64 / 1e6;
    eprintln!(
        "NUMBERS a day of minute saves: {entities} entities; a whole save {:.1} MB (round 3 wrote that every minute: {:.1} GB a day); \
         now 1440 minute saves + {compactions} hourly compactions ({compacted} bases) + 1 final save = {mb:.1} MB a day; the biggest single save {:.2} MB; \
         final save {:.1} MB",
        whole.bytes as f64 / 1e6,
        whole.bytes as f64 * 1440.0 / 1e9,
        most as f64 / 1e6,
        last.bytes as f64 / 1e6
    );
    assert!(counted < 1_000_000_000, "{mb:.1} MB a day");
    // After the final save only base files are left, and they hold exactly what memory holds.
    assert!(files(&live).keys().all(|n| n.ends_with(".sbh")), "{:?}", files(&live).keys().filter(|n| !n.ends_with(".sbh")).collect::<Vec<_>>());
    assert_eq!(dump_dir(&live, end), dump(&h, end));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Minute `m` after `start` recorded from every entity of `snap`, and saved 7 s into the next.
fn saved_minute(h: &History, snap: &mut crate::model::ClusterSnapshot, start: i64, m: i64) -> Written {
    for t in start + 60 * m..start + 60 * (m + 1) {
        record_at(h, snap, &[], t);
    }
    h.save_at(false, start + 60 * (m + 1) + 7)
}

#[test]
fn a_truncated_or_damaged_last_record_loads_without_it() {
    let dir = history_dir("torn");
    let live = dir.join("live");
    let h = History::new(Some(dir.clone()));
    let mut snap = live_snapshot(0);
    let start = clear_of_midnight((now_ms() / 1000) as i64 / 60 * 60 - 600);
    let node = format!("node:{}", snap.nodes[0].id);
    let seg = live.join(segment_name(&node, 0, &utc_day(start + 60)));
    saved_minute(&h, &mut snap, start, 0);
    saved_minute(&h, &mut snap, start, 1);
    let at = start + 300;
    let two = dump_dir(&live, at);
    let len_two = std::fs::metadata(&seg).unwrap().len() as usize;
    saved_minute(&h, &mut snap, start, 2);
    let all = dump(&h, at);
    assert_eq!(dump_dir(&live, at), all, "three appends load as the history");
    let whole = std::fs::read(&seg).unwrap();
    assert!(whole.len() > len_two);

    // A save cut off midway: the last record is missing its end.
    std::fs::write(&seg, &whole[..whole.len() - 37]).unwrap();
    let loaded = dump_dir(&live, at);
    assert_eq!(loaded[&node], two[&node], "the node loads as it was before the cut record");
    assert!(loaded.iter().filter(|(e, _)| **e != node).all(|(e, b)| all.get(e) == Some(b)), "every other entity loads whole");
    assert_eq!(std::fs::read(&seg).unwrap(), whole[..len_two].to_vec(), "the damaged tail was cut off the file, nothing more");

    // A bad checksum in the last record: that record is left out, and the file stays as it is,
    // since every record in it still frames.
    let mut bad = whole.clone();
    let n = bad.len();
    bad[n - 5] ^= 0x40;
    std::fs::write(&seg, &bad).unwrap();
    assert_eq!(dump_dir(&live, at)[&node], two[&node]);
    assert_eq!(std::fs::read(&seg).unwrap(), bad, "nothing was cut off");

    // Every record frames, so what's appended next loads too (from a new run, which starts its
    // records with a header again).
    let h = History::new(Some(dir.clone()));
    let mut s2 = snap.clone();
    s2.ts_ms = (start + 200) as u64 * 1000;
    h.record(&s2);
    for t in start + 180..start + 240 {
        record_at(&h, &mut snap, &[], t);
    }
    h.save_at(false, start + 247);
    let again = dump_dir(&live, at);
    assert_eq!(again, dump(&h, at), "the new run's appends load after the cut");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The segment files in `dir`: (name, length).
fn segment_files(dir: &std::path::Path) -> BTreeMap<String, u64> {
    files(dir).into_iter().filter(|(n, _)| n.ends_with(".sbs")).map(|(n, (_, len))| (n, len)).collect()
}

/// The names of `entity`'s segments of `epoch` in `dir`.
fn segments_named(dir: &std::path::Path, entity: &str, epoch: u32) -> Vec<String> {
    let prefix = format!("{}.e{epoch}.", file_name(entity).trim_end_matches(".sbh"));
    segment_files(dir).into_keys().filter(|n| n.starts_with(&prefix)).collect()
}

#[test]
fn a_compaction_writes_bases_and_retires_their_segments() {
    let dir = history_dir("compact");
    let live = dir.join("live");
    let h = History::new(Some(dir.clone()));
    let mut snap = live_snapshot(0);
    let eps = two_endpoints();
    let start = (now_ms() / 1000) as i64 / 60 * 60 - 7200;
    saved_minute(&h, &mut snap, start, 0);
    let first: Vec<String> = h.inner.lock().unwrap().series.keys().cloned().collect();
    // The first node's containers stop; two endpoints start reporting half an hour in.
    let stopped: Vec<String> = snap.nodes[0].containers.iter().filter(|c| c.state == "running").map(|c| format!("container:{}/{}", snap.nodes[0].id, c.name)).collect();
    assert!(!stopped.is_empty());
    for c in snap.nodes[0].containers.iter_mut() {
        c.state = "exited".into();
    }
    for m in 1..60 {
        if m < 30 {
            saved_minute(&h, &mut snap, start, m);
        } else {
            for t in start + 60 * m..start + 60 * (m + 1) {
                record_at(&h, &mut snap, &eps, t);
            }
            h.save_at(false, start + 60 * (m + 1) + 7);
        }
    }
    assert!(files(&live).keys().all(|n| n.ends_with(".sbs")), "an hour of minute saves writes segments only");
    assert_eq!(dump_dir(&live, start + 3600), dump(&h, start + 3600), "the segments load as the history");

    // An hour after the first save: the hourly compaction takes the oldest twenty-fourth.
    let since: BTreeMap<String, u64> = h.inner.lock().unwrap().series.iter().filter_map(|(e, s)| Some((e.clone(), s.segments_since_ms?))).collect();
    let w = saved_minute(&h, &mut snap, start, 60);
    assert_eq!(w.compacted.len(), since.len().div_ceil(COMPACT_SHARE), "{:?}", w.compacted);
    let newest_taken = w.compacted.iter().map(|e| since[e]).max().unwrap();
    assert!(since.iter().filter(|(e, _)| !w.compacted.contains(e)).all(|(_, s)| *s >= newest_taken), "oldest first");
    assert!(w.compacted.iter().all(|e| !e.starts_with("endpoint:")), "the endpoints' segments are half an hour younger");
    let at = start + 3667;
    for e in &w.compacted {
        assert!(live.join(file_name(e)).exists(), "{e} has a base");
        assert_eq!(h.inner.lock().unwrap().series[e].epoch, 1, "a new base is the next epoch");
        assert!(!segments_named(&live, e, 0).is_empty(), "{e}'s old segment stays until the next save's barrier");
    }
    assert_eq!(dump_dir(&live, at), dump(&h, at), "bases, their retired segments (skipped) and the other segments load as the history");
    let w2 = saved_minute(&h, &mut snap, start, 61);
    assert!(w2.compacted.is_empty(), "at most once an hour");
    for e in &w.compacted {
        assert!(segments_named(&live, e, 0).is_empty(), "{e}'s retired segment is gone after the next barrier");
        // Still reporting: its buckets go to a segment of the new epoch.
        if !stopped.contains(e) {
            assert!(w2.appended.contains(e) && !segments_named(&live, e, 1).is_empty(), "{e}");
        }
    }
    assert_eq!(dump_dir(&live, start + 3727), dump(&h, start + 3727));

    // Quit: everything with segments gets a base and every segment goes.
    let last = h.save_at(true, start + 3740);
    assert!(last.compacted.len() >= first.len() - w.compacted.len(), "{:?}", last.compacted);
    assert!(segment_files(&live).is_empty(), "{:?}", segment_files(&live));
    assert_eq!(dump_dir(&live, start + 3740), dump(&h, start + 3740));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_compaction_cut_off_midway_loads_the_same_history() {
    let dir = history_dir("cut-compaction");
    let live = dir.join("live");
    let h = History::new(Some(dir.clone()));
    let mut snap = live_snapshot(0);
    // Two minutes past a quarter hour, so the quarter-hour bucket open at the last minute save
    // gets more readings after it.
    let start = (now_ms() / 1000) as i64 / 900 * 900 - 7200 + 120;
    for m in 0..30 {
        saved_minute(&h, &mut snap, start, m);
    }
    // A crash before the compaction's renames: the folder as it was, plus the temporary files.
    let copy = |name: &str| {
        let to = history_dir(name);
        std::fs::create_dir_all(&to).unwrap();
        for e in std::fs::read_dir(&live).unwrap().flatten() {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
        to
    };
    let before = copy("cut-before");
    std::fs::write(before.join(format!(".{}.tmp-1-0000abcd", file_name("node:cell1"))), b"half a base").unwrap();
    let at = start + 1800 + 7;
    assert_eq!(dump_dir(&before, at), dump(&h, at), "the old files and a stray temporary file load as the history");

    // A crash after the renames, before the retired segments were deleted: every entity has a
    // base of epoch 1 and its epoch 0 segments are still there. Those are skipped, so a bucket
    // the base holds whole isn't put back to the half it was when the last segment took it
    // (20 s of readings came after the last minute save).
    for t in start + 1800..start + 1820 {
        record_at(&h, &mut snap, &[], t);
    }
    let at = start + 1820;
    let w = h.save_at(true, at);
    assert!(!w.compacted.is_empty());
    let after = history_dir("cut-after");
    std::fs::create_dir_all(&after).unwrap();
    for e in std::fs::read_dir(&live).unwrap().flatten() {
        std::fs::copy(e.path(), after.join(e.file_name())).unwrap();
    }
    for e in std::fs::read_dir(&before).unwrap().flatten() {
        if e.file_name().to_string_lossy().ends_with(".sbs") {
            std::fs::copy(e.path(), after.join(e.file_name())).unwrap();
        }
    }
    assert!(!segment_files(&after).is_empty());
    let loaded = super::store::load(&after, metric_names, at);
    assert_eq!(loaded.obsolete.len(), segment_files(&after).len(), "every left-behind segment is known as retired");
    assert_eq!(dump_dir(&after, at), dump(&h, at));
    for d in [&dir, &before, &after] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn version_2_files_load_unchanged_and_bases_stay_readable_by_p1_r3() {
    let dir = history_dir("v2");
    std::fs::create_dir_all(&dir).unwrap();
    let now = (now_ms() / 1000) as i64;
    let mut s = Series::new();
    for t in now - 7200..now - 60 {
        s.push(t, &reading((t % 37) as f32));
    }
    // Epoch 0 is p1-r3's own format, byte for byte: no segEpoch in the header.
    let bytes = s.encode("node:cell1", now);
    let header = std::str::from_utf8(&bytes[..bytes.iter().position(|&b| b == b'\n').unwrap()]).unwrap().to_string();
    assert!(!header.contains("segEpoch"), "{header}");
    std::fs::write(dir.join(file_name("node:cell1")), &bytes).unwrap();
    let loaded = load_dir(&dir, metric_names);
    assert_eq!(loaded["node:cell1"].buckets(now), s.buckets(now), "a version 2 file loads as it was");
    assert!(!loaded["node:cell1"].changed(), "and isn't written again");
    // A base this build compacts carries segEpoch, which p1-r3's header (no such field, unknown
    // fields ignored) reads past.
    let mut series = BTreeMap::from([("node:cell1".to_string(), s)]);
    series.get_mut("node:cell1").unwrap().push(now - 30, &reading(1.0));
    save_dir(&dir, &mut series, &OsSync, false);
    let bytes = std::fs::read(dir.join(file_name("node:cell1"))).unwrap();
    let line = &bytes[..bytes.iter().position(|&b| b == b'\n').unwrap()];
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    struct P1R3Header {
        format: String,
        version: u32,
        entity: Option<String>,
        node: Option<String>,
        metrics: Vec<String>,
        saved_at_ms: u64,
        tiers: Vec<serde_json::Value>,
    }
    let h: P1R3Header = serde_json::from_slice(line).unwrap();
    assert_eq!((h.format.as_str(), h.version), ("sparkbox-history", 2));
    assert!(std::str::from_utf8(line).unwrap().contains("\"segEpoch\":1"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_budget_counts_segments_and_a_dropped_bucket_never_comes_back() {
    let dir = history_dir("budget");
    let live = dir.join("live");
    let start = (now_ms() / 1000) as i64 / 60 * 60 - 4 * 3600;
    let mut snap = live_snapshot(0);
    // Four hours of minute saves with no budget to speak of.
    let h = History::with_budget(Some(dir.clone()), usize::MAX);
    for m in 0..240 {
        saved_minute(&h, &mut snap, start, m);
    }
    h.save_at(true, start + 240 * 60 + 8);
    for m in 241..250 {
        saved_minute(&h, &mut snap, start, m);
    }
    let on_disk = super::store::dir_bytes(&live);
    let segments: u64 = segment_files(&live).values().sum();
    assert!(segments > 0 && on_disk > segments, "bases and segments both count: {on_disk} bytes, {segments} in segments");
    let bases = on_disk - segments;
    drop(h);
    // The same history under a budget its bases fit and its segments push over: the next save
    // compacts everything (nothing dropped), and the one after deletes the retired segments.
    let h = History::with_budget(Some(dir.clone()), (bases + segments / 2) as usize);
    let at = start + 250 * 60 + 8;
    let mut s = snap.clone();
    s.ts_ms = at as u64 * 1000;
    h.record(&s);
    let w = h.save_at(false, at);
    assert!(w.compacted.len() > 20, "{w:?}");
    h.save_at(false, at + 60);
    assert!(segment_files(&live).is_empty());
    assert!(super::store::dir_bytes(&live) <= (bases + segments / 2), "under the budget once the segments are gone");
    // A budget the bases don't fit: the oldest buckets go, from the files too. A segment that
    // still held them (written before the drop) doesn't bring them back.
    let oldest_before = dump(&h, at).values().flatten().map(|b| b.1 * TIERS[b.0].step_s).min().unwrap();
    let small = super::store::dir_bytes(&live) as usize / 2;
    let h2 = History::with_budget(Some(dir.clone()), small);
    h2.record(&s);
    for m in 251..253 {
        saved_minute(&h2, &mut snap, start, m);
    }
    let after = dump(&h2, at + 180);
    let oldest_after = after.values().flatten().map(|b| b.1 * TIERS[b.0].step_s).min().unwrap();
    assert!(oldest_after > oldest_before, "the oldest data went");
    h2.save_at(false, at + 240);
    assert!(super::store::dir_bytes(&live) as usize <= small, "{} > {small}", super::store::dir_bytes(&live));
    assert_eq!(dump_dir(&live, at + 240), dump(&h2, at + 240), "what loads is what's kept, nothing older");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saves_compactions_and_restarts_interleaved_keep_every_bucket() {
    let dir = history_dir("interleaved");
    let live = dir.join("live");
    let mut h = History::new(Some(dir.clone()));
    let mut snap = live_snapshot(0);
    let eps = two_endpoints();
    let start = (now_ms() / 1000) as i64 / 60 * 60 - 6 * 3600;
    let mut rng = fastrand::Rng::with_seed(7);
    let mut t = start;
    let mut next_save = start + 60 + rng.i64(0..60);
    let mut checks = 0;
    while t < start + 6 * 3600 - 120 {
        // Readings with gaps (a node offline now and then), saves about a minute apart.
        snap.nodes[1].status = if rng.u8(..) < 30 { crate::model::LinkStatus::Offline } else { crate::model::LinkStatus::Online };
        record_at(&h, &mut snap, &eps, t);
        t += 1 + rng.i64(0..4);
        if t >= next_save {
            let full = rng.u8(..) < 6;
            let w = h.save_at(full, t);
            assert!(w.failed.is_empty());
            next_save = t + 45 + rng.i64(0..30);
            // Now and then: what's on disk is what's in memory, and a restart picks it all up.
            if full || rng.u8(..) < 25 {
                assert_eq!(dump_dir(&live, t), dump(&h, t), "at {} s", t - start);
                checks += 1;
                if rng.bool() {
                    drop(h);
                    h = History::new(Some(dir.clone()));
                    let mut s = snap.clone();
                    s.ts_ms = t as u64 * 1000;
                    s.nodes.iter_mut().for_each(|n| n.status = crate::model::LinkStatus::Offline);
                    h.record(&s);
                    assert_eq!(dump(&h, t), dump_dir(&live, t), "a restart loads it");
                }
            }
        }
    }
    assert!(checks > 20, "{checks} checks");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn switching_source_appends_the_one_left_and_loads_it_back() {
    let dir = history_dir("switch");
    let h = History::new(Some(dir.clone()));
    let mut snap = live_snapshot(0);
    let start = (now_ms() / 1000) as i64 / 60 * 60 - 1200;
    saved_minute(&h, &mut snap, start, 0);
    for t in start + 60..start + 90 {
        record_at(&h, &mut snap, &[], t);
    }
    let live_before = dump(&h, start + 95);
    // To Demo: the 30 s not saved yet are appended to Live's segments, Demo's history loads.
    let mut demo = DemoCluster::new(&Inventory::example());
    let mut d = demo.snapshot();
    d.ts_ms = (start + 95) as u64 * 1000;
    h.record(&d);
    assert_eq!(dump_dir(&dir.join("live"), start + 95), live_before, "nothing of Live's lost on the way out");
    // And back to Live: the same history.
    let mut s = snap.clone();
    s.ts_ms = (start + 96) as u64 * 1000;
    s.nodes.iter_mut().for_each(|n| n.status = crate::model::LinkStatus::Offline);
    h.record(&s);
    assert_eq!(dump(&h, start + 95), live_before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_segment_from_a_build_with_other_metrics_takes_this_builds_appends() {
    let dir = history_dir("metrics");
    let live = dir.join("live");
    let now = clear_of_midnight((now_ms() / 1000) as i64 / 60 * 60 - 600);
    let snap = live_snapshot(0);
    let node = format!("node:{}", snap.nodes[0].id);
    // An earlier build that kept 15 of the 16 node series appended a minute today.
    let older: Vec<String> = METRICS[..N - 1].iter().map(|m| m.to_string()).collect();
    let mut s = Series::with_metrics(older.clone());
    for t in now..now + 60 {
        s.push(t, &[2.0; N - 1]);
    }
    let headed = std::sync::Mutex::new(std::collections::HashSet::new());
    let mut a = super::store::Appender::new(&live, &OsSync, &headed, now + 60).unwrap();
    a.append(&node, &older, 0, &super::store::delta_record(&s.take_delta().unwrap(), 0)).unwrap();
    // This build appends to the same file: its own header first, so its 16-wide buckets read right.
    let h = History::new(Some(dir.clone()));
    let mut snap = snap.clone();
    for t in now + 60..now + 120 {
        record_at(&h, &mut snap, &[], t);
    }
    h.save_at(false, now + 127);
    assert_eq!(segments_named(&live, &node, 0).len(), 1, "one file for the day");
    let loaded = super::store::load(&live, metric_names, now + 130).series;
    let got = &loaded[&node];
    let q = |m: &str, from: i64, to: i64| { let k = got.index_of(m).unwrap(); let r = got.query(&[m.to_string()], &[k], from as u64 * 1000, to as u64 * 1000, 100, (now + 130) as u64 * 1000); r.series[m].iter().flatten().count() };
    assert!(q("cpu", now, now + 60) >= 6, "the earlier build's minute");
    assert_eq!(q("smClock", now, now + 60), 0, "a series it didn't keep stays a gap");
    assert!(q("smClock", now + 60, now + 120) >= 6, "this build's minute, every series");
    assert_eq!(got.buckets(now + 130).iter().filter(|b| b.1 * TIERS[b.0].step_s >= now + 60).count(), dump(&h, now + 130)[&node].iter().filter(|b| b.1 * TIERS[b.0].step_s >= now + 60).count());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- P1 round 5: what the Live verifier found in round 4

#[test]
fn a_segment_named_like_one_in_the_other_folder_gets_its_own_header() {
    let dir = history_dir("headed");
    let (demo_dir, live) = (dir.join("demo"), dir.join("live"));
    let at = clear_of_midnight((now_ms() / 1000) as i64 / 60 * 60 - 600);
    let mut snap = live_snapshot(0);
    let node = format!("node:{}", snap.nodes[0].id);
    // An earlier run, on a build that kept 15 of the 16 node series, left today's Live segment.
    let older: Vec<String> = METRICS[..N - 1].iter().map(|m| m.to_string()).collect();
    let mut s = Series::with_metrics(older.clone());
    for t in at - 120..at - 60 {
        s.push(t, &[2.0; N - 1]);
    }
    let headed = std::sync::Mutex::new(std::collections::HashSet::new());
    super::store::Appender::new(&live, &OsSync, &headed, at).unwrap().append(&node, &older, 0, &super::store::delta_record(&s.take_delta().unwrap(), 0)).unwrap();
    let seg = live.join(segment_name(&node, 0, &utc_day(at)));
    assert_eq!(segment_name(&node, 0, &utc_day(at)), seg.file_name().unwrap().to_string_lossy());

    // This run appends under Demo first: a segment of that same name, in Demo's folder.
    let h = History::new(Some(dir.clone()));
    let mut demo = snap.clone();
    demo.demo = true;
    demo.ts_ms = (at - 50) as u64 * 1000;
    h.record(&demo);
    demo.ts_ms += 1000;
    h.record(&demo);
    h.save_at(false, at - 45);
    assert!(demo_dir.join(segment_name(&node, 0, &utc_day(at))).exists());
    let demo_before = dump(&h, at);

    // Then Live, and the same name in its folder: this run hasn't written a header there.
    for t in at - 40..at - 20 {
        record_at(&h, &mut snap, &[], t);
    }
    h.save_at(false, at - 15);
    let bytes = std::fs::read(&seg).unwrap();
    let headers = bytes.windows(b"sparkbox-history-segment".len()).filter(|w| *w == b"sparkbox-history-segment").count();
    assert_eq!(headers, 2, "the earlier run's header and this run's");
    assert_eq!(dump_dir(&live, at), dump(&h, at), "Live's folder reads back whole");
    assert_eq!(std::fs::read(&seg).unwrap(), bytes, "and loading it cut nothing off");
    assert_eq!(dump_dir(&demo_dir, at), demo_before, "so does Demo's");
    let _ = std::fs::remove_dir_all(&dir);
}

/// This thread's CPU time: what a query costs, however busy the machine is.
fn thread_cpu() -> std::time::Duration {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) }, 0);
    std::time::Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

/// Ten minutes of cpu at 7.0, ending now.
fn ten_minutes() -> (std::sync::Arc<History>, u64) {
    let h = std::sync::Arc::new(History::new(None));
    let now = now_ms();
    for i in 0..600 {
        h.record_entity("node:cell1", "cpu", (now - i * 1000) as i64, 7.0).unwrap();
    }
    (h, now)
}

#[test]
fn a_query_of_all_time_answers_at_once_with_what_is_kept() {
    let (h, now) = ten_minutes();
    let (tx, rx) = std::sync::mpsc::channel();
    // On a thread of its own, so a query that hangs fails this test instead of holding it up.
    std::thread::spawn(move || {
        let every: Vec<String> = METRICS.iter().map(|m| m.to_string()).collect();
        let started = thread_cpu();
        // Every series in a few huge groups, then one series in as many groups as a query gets.
        let few = h.query("node:cell1", &every, 0, u64::MAX, 2);
        let many = h.query("node:cell1", &["cpu".into()], 0, u64::MAX, usize::MAX);
        let _ = tx.send((few, many, thread_cpu() - started));
    });
    let (few, many, cost) = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the query came back");
    assert!(cost < std::time::Duration::from_millis(100), "{cost:?} of CPU");
    let (few, many) = (few.unwrap(), many.unwrap());
    assert!(few.ts.len() <= 4, "{}", few.ts.len());
    assert!(many.ts.len() <= 20_002, "{}", many.ts.len());
    for r in [&few, &many] {
        let got: Vec<f32> = r.series["cpu"].iter().flatten().copied().collect();
        assert!(!got.is_empty() && got.iter().all(|v| *v == 7.0), "{got:?}");
        assert!(*r.ts.last().unwrap() <= now + 3_600_000, "the range ends within the hour");
    }
}

#[test]
fn a_query_that_runs_past_now_keeps_what_it_holds() {
    let (h, now) = ten_minutes();
    let cpu = ["cpu".to_string()];
    // The 1 s ring's range, the coarse tiers' range, and a clock a month ahead.
    for (from, to) in [(now - 30_000, now + 3_600_000), (now - 300_000, now + 86_400_000), (now - 300_000, now + 31 * 86_400_000), (now - 900_000, u64::MAX)] {
        let r = h.query("node:cell1", &cpu, from, to, 1000).unwrap();
        let got: Vec<f32> = r.series["cpu"].iter().flatten().copied().collect();
        assert!(!got.is_empty() && got.iter().all(|v| *v == 7.0), "{from}..{to}: {got:?}");
    }
    assert_eq!(h.query("node:cell1", &cpu, now - 30_000, now + 3_600_000, 4000).unwrap().tier, "1s");
    // A range that starts past the hour has nothing to ask for.
    assert!(h.query("node:cell1", &cpu, u64::MAX - 1, u64::MAX, 100).unwrap_err().contains("after its start"));
    // At least two points, as ever.
    assert!(h.query("node:cell1", &cpu, now - 300_000, now, 0).unwrap().ts.len() >= 2);
}

#[test]
fn a_switch_during_a_compaction_keeps_every_reading() {
    let dir = history_dir("switch-compaction");
    let h = std::sync::Arc::new(History::new(Some(dir.clone())));
    let mut snap = live_snapshot(0);
    let start = (now_ms() / 1000) as i64 / 60 * 60 - 1200;
    saved_minute(&h, &mut snap, start, 0);
    // Quit's save, held in its first file: that entity's copy is taken, its base not in place.
    let (writing_tx, writing_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    h.set_syncer(std::sync::Arc::new(Held { writing: std::sync::Mutex::new(Some(writing_tx)), release: std::sync::Mutex::new(release_rx) }));
    let at = start + 90;
    let saver = {
        let h = h.clone();
        std::thread::spawn(move || h.save_at(true, at))
    };
    writing_rx.recv_timeout(std::time::Duration::from_secs(20)).expect("the save is writing its first file");
    // Readings go on arriving, those of the copied entity after its copy.
    for t in start + 60..at {
        record_at(&h, &mut snap, &[], t);
    }
    let live_before = dump(&h, at);
    // The switch to Demo comes while that file is still being written.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let switcher = {
        let h = h.clone();
        let mut d = DemoCluster::new(&Inventory::example()).snapshot();
        d.ts_ms = (at + 1) as u64 * 1000;
        std::thread::spawn(move || {
            h.record(&d);
            let _ = done_tx.send(());
        })
    };
    let went_ahead = done_rx.recv_timeout(std::time::Duration::from_millis(500)).is_ok();
    release_tx.send(()).unwrap();
    let written = saver.join().unwrap();
    switcher.join().unwrap();
    assert!(!written.compacted.is_empty() && written.failed.is_empty(), "{written:?}");
    // A reload brings back every reading from before the switch.
    let h = History::new(Some(dir.clone()));
    let mut s = snap.clone();
    s.ts_ms = (at + 2) as u64 * 1000;
    s.nodes.iter_mut().for_each(|n| n.status = crate::model::LinkStatus::Offline);
    h.record(&s);
    assert_eq!(dump(&h, at), live_before);
    assert!(!went_ahead, "the switch waited for the save");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reading_newer_than_the_final_saves_clock_survives_it() {
    let dir = history_dir("past-clock");
    let live = dir.join("live");
    let h = History::new(Some(dir.clone()));
    let mut snap = live_snapshot(0);
    // Five seconds before a quarter hour, so the next reading is in a later bucket of every tier.
    let clock = (now_ms() / 1000) as i64 / 900 * 900 - 905;
    for t in clock - 30..=clock {
        record_at(&h, &mut snap, &[], t);
    }
    // The save reads its clock, and a reading lands before it takes its copies.
    record_at(&h, &mut snap, &[], clock + 10);
    let later = clock + 11;
    let all = dump(&h, later);
    assert!(all.values().flatten().any(|b| b.0 == 2 && b.1 * 900 > clock), "the reading is past the clock in the coarsest tier too");
    let w = h.save_at(true, clock);
    assert!(!w.compacted.is_empty() && w.failed.is_empty(), "{w:?}");
    assert!(segment_files(&live).is_empty(), "a final save leaves only bases: nothing else holds that reading");
    assert_eq!(dump_dir(&live, later), all);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bucket_past_the_clock_doesnt_stretch_a_base_for_it() {
    // A node agent whose clock is a year ahead: a dense file out to that bucket would be gigabytes.
    let mut s = Series::new();
    s.push(T0, &reading(1.0));
    let alone = s.encoded_len("node:x", T0);
    s.push_buckets(T0 + 365 * 86_400 + 15, &reading(2.0));
    assert_eq!(s.encoded_len("node:x", T0), alone);
    assert_eq!(s.encoded_len("node:x", T0), s.encode("node:x", T0).len());
    // A bucket within a lap of the clock is kept, gap and all.
    let mut s = Series::new();
    s.push(T0, &reading(1.0));
    s.push_buckets(T0 + 3600, &reading(2.0));
    let bytes = s.encode("node:x", T0);
    let (_, back) = Series::decode(&bytes, T0, metric_names).unwrap();
    assert_eq!(back.buckets(T0 + 3600), s.buckets(T0 + 3600));
}

#[test]
fn a_flipped_byte_mid_segment_loses_that_record_and_no_other() {
    let dir = history_dir("flip");
    let live = dir.join("live");
    let h = History::new(Some(dir.clone()));
    let mut snap = live_snapshot(0);
    let start = clear_of_midnight((now_ms() / 1000) as i64 / 60 * 60 - 600);
    let node = format!("node:{}", snap.nodes[0].id);
    let seg = live.join(segment_name(&node, 0, &utc_day(start)));
    let len = || std::fs::metadata(&seg).unwrap().len() as usize;
    saved_minute(&h, &mut snap, start, 0);
    let one = len();
    saved_minute(&h, &mut snap, start, 1);
    let two = len();
    saved_minute(&h, &mut snap, start, 2);
    let at = start + 300;
    let whole = std::fs::read(&seg).unwrap();
    assert!(whole.len() > two);
    // What's left without the second minute's record.
    std::fs::write(&seg, [&whole[..one], &whole[two..]].concat()).unwrap();
    let without = dump_dir(&live, at);
    assert_ne!(without[&node], dump(&h, at)[&node], "that record held readings");

    let mut bad = whole.clone();
    bad[(one + two) / 2] ^= 0x40;
    std::fs::write(&seg, &bad).unwrap();
    assert_eq!(dump_dir(&live, at), without);
    assert_eq!(std::fs::read(&seg).unwrap(), bad, "the records after it stay in the file");

    // A tail that can't be framed is the one thing cut off.
    let tail = [&bad[..], &[0u8; 64]].concat();
    std::fs::write(&seg, &tail).unwrap();
    assert_eq!(dump_dir(&live, at), without);
    assert_eq!(std::fs::read(&seg).unwrap(), bad);

    // A record this build can't read isn't damage: it's skipped, and nothing is cut.
    let foreign = {
        let payload = b"Xa later kind";
        let mut r = (payload.len() as u32).to_le_bytes().to_vec();
        r.extend_from_slice(&super::store::crc32(payload).to_le_bytes());
        r.extend_from_slice(payload);
        r
    };
    let with_foreign = [&whole[..two], &foreign[..], &whole[two..]].concat();
    std::fs::write(&seg, &with_foreign).unwrap();
    assert_eq!(dump_dir(&live, at), dump(&h, at));
    assert_eq!(std::fs::read(&seg).unwrap(), with_foreign);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn going_over_budget_because_of_segments_compacts_once() {
    let dir = history_dir("over-once");
    let live = dir.join("live");
    let start = (now_ms() / 1000) as i64 / 60 * 60 - 4 * 3600;
    let mut snap = live_snapshot(0);
    let h = History::with_budget(Some(dir.clone()), usize::MAX);
    for m in 0..60 {
        saved_minute(&h, &mut snap, start, m);
    }
    h.save_at(true, start + 60 * 60 + 8);
    for m in 61..70 {
        saved_minute(&h, &mut snap, start, m);
    }
    let on_disk = super::store::dir_bytes(&live);
    let segments: u64 = segment_files(&live).values().sum();
    drop(h);
    // Its bases fit this budget and its segments push it over.
    let h = History::with_budget(Some(dir.clone()), (on_disk - segments / 2) as usize);
    let w = saved_minute(&h, &mut snap, start, 70);
    assert!(w.compacted.len() > 20, "the first save over budget compacts: {w:?}");
    assert!(!segment_files(&live).is_empty(), "its retired segments are still on disk until the next barrier");
    // The next minute's readings are appended; the retired segments don't count against it.
    let next = saved_minute(&h, &mut snap, start, 71);
    assert!(next.compacted.is_empty(), "compacted again: {:?}", next.compacted);
    assert!(next.appended.len() > 20, "{next:?}");
    assert_eq!(dump_dir(&live, start + 72 * 60 + 8), dump(&h, start + 72 * 60 + 8));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn segments_found_retired_at_load_dont_count_against_the_budget() {
    let dir = history_dir("retired-at-load");
    let live = dir.join("live");
    let start = (now_ms() / 1000) as i64 / 60 * 60 - 3 * 3600;
    let mut snap = live_snapshot(0);
    let h = History::with_budget(Some(dir.clone()), usize::MAX);
    for m in 0..30 {
        saved_minute(&h, &mut snap, start, m);
    }
    let segments: Vec<(std::path::PathBuf, Vec<u8>)> = segment_files(&live).into_keys().map(|n| live.join(n)).map(|p| { let b = std::fs::read(&p).unwrap(); (p, b) }).collect();
    assert!(!segments.is_empty());
    h.save_at(true, start + 30 * 60 + 8);
    assert!(segment_files(&live).is_empty());
    let bases = super::store::dir_bytes(&live);
    let retired: u64 = segments.iter().map(|(_, b)| b.len() as u64).sum();
    // A crash after the compaction's renames, before its retired segments were deleted: only
    // the next run's load finds them.
    for (path, bytes) in &segments {
        std::fs::write(path, bytes).unwrap();
    }
    drop(h);
    // Its bases fit this budget and the retired segments push it over.
    let h = History::with_budget(Some(dir.clone()), (bases + retired / 2) as usize);
    let w = saved_minute(&h, &mut snap, start, 31);
    assert!(w.compacted.is_empty(), "compacted because of retired segments: {:?}", w.compacted);
    assert!(w.appended.len() > 20, "{w:?}");
    assert!(segments.iter().all(|(path, _)| !path.exists()), "they go behind this save's barrier");
    assert_eq!(dump_dir(&live, start + 32 * 60 + 8), dump(&h, start + 32 * 60 + 8));
    let _ = std::fs::remove_dir_all(&dir);
}
