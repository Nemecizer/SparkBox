//! SparkBox's own weight on the Mac: the process lists stay out of the 1 s broadcast.
//!
//! A node reports up to 200 processes a second; sent to every window each second, that's the
//! bulk of the snapshot (about 250-300 KB of JSON a second). The snapshot the windows get keeps,
//! per node, the 15 busiest by CPU and every process that owns a listening port (Workloads'
//! port owners, knownPorts.ts and the VPN window name them from it), and says how many there
//! were (`process_count`). Workloads asks for the whole list (`processes_get`) every 5 s while
//! it's open, for each node whose list was cut. Everything in Rust keeps reading the full list from the latest snapshot
//! (the PID guard, process lookups by name, endpoint discovery): `publish` takes the lists out
//! for the emit only and puts them back (`publish_with`: `trim_for_ui`, the emit, `restore`),
//! copying nothing but the kept rows.

use crate::model::{ClusterSnapshot, Process};

/// The busiest processes per node the 1 s snapshot keeps (besides the listeners' owners).
pub const TOP: usize = 15;

/// The full lists `trim_for_ui` took out, by node index, for `restore`.
pub struct Trimmed(Vec<(usize, Vec<Process>)>);

/// Cuts each node's process list to the 15 busiest by CPU plus every process with a listening
/// port (or named as a listener's pid), in the order the node sent them, and sets
/// `process_count` to how many it sent. Returns the full lists for `restore`.
pub fn trim_for_ui(snap: &mut ClusterSnapshot) -> Trimmed {
    let mut held = Vec::new();
    for (i, n) in snap.nodes.iter_mut().enumerate() {
        n.process_count = Some(n.processes.len() as u32);
        let keep = kept(&n.processes, &n.listeners.iter().filter_map(|l| l.pid).collect::<Vec<u32>>());
        if keep.len() == n.processes.len() {
            continue;
        }
        let full = std::mem::take(&mut n.processes);
        n.processes = keep.into_iter().map(|k| full[k].clone()).collect();
        held.push((i, full));
    }
    Trimmed(held)
}

/// Puts back the lists `trim_for_ui` took out.
pub fn restore(snap: &mut ClusterSnapshot, trimmed: Trimmed) {
    for (i, full) in trimmed.0 {
        if let Some(n) = snap.nodes.get_mut(i) {
            n.processes = full;
        }
    }
}

/// What `publish` does with each snapshot, without the app: `overlays` (power states, update
/// facts, the alerting) on the whole snapshot, then the windows' copy cut to each node's busiest
/// processes and listeners' owners for `emit`, then the full lists back for everything in Rust
/// (`state.latest`). The lists go back even if `emit` panics; an overlay that panics does so
/// before anything was cut.
pub fn publish_with(snap: &mut ClusterSnapshot, overlays: impl FnOnce(&mut ClusterSnapshot), emit: impl FnOnce(&ClusterSnapshot)) {
    overlays(snap);
    let held = Restoring { trimmed: Some(trim_for_ui(snap)), snap };
    emit(held.snap);
}

/// Puts the full lists back when dropped, however `publish_with` leaves.
struct Restoring<'a> {
    snap: &'a mut ClusterSnapshot,
    trimmed: Option<Trimmed>,
}

impl Drop for Restoring<'_> {
    fn drop(&mut self) {
        if let Some(t) = self.trimmed.take() {
            restore(self.snap, t);
        }
    }
}

/// Which processes (by index, in order) the 1 s snapshot keeps.
fn kept(procs: &[Process], listener_pids: &[u32]) -> Vec<usize> {
    let mut busiest: Vec<usize> = (0..procs.len()).collect();
    busiest.sort_by(|a, b| procs[*b].cpu_pct.total_cmp(&procs[*a].cpu_pct).then(a.cmp(b)));
    busiest.truncate(TOP);
    (0..procs.len()).filter(|i| busiest.contains(i) || !procs[*i].ports.is_empty() || listener_pids.contains(&procs[*i].pid)).collect()
}

/// A node's whole process list, from `snap` (the latest snapshot, never trimmed).
pub fn processes_of(snap: Option<&ClusterSnapshot>, node: &str) -> Result<Vec<Process>, String> {
    let Some(snap) = snap else { return Ok(Vec::new()) };
    snap.nodes.iter().find(|n| n.id == node).map(|n| n.processes.clone()).ok_or_else(|| format!("There's no node \"{node}\" in the inventory."))
}

/// Every process `node` reported in its latest frame (Workloads asks every 5 s while it's open).
#[tauri::command]
pub fn processes_get(app: tauri::AppHandle, node: String) -> Result<Vec<Process>, String> {
    use tauri::Manager;
    let state = app.state::<crate::AppState>();
    let latest = state.latest.read().unwrap();
    processes_of(latest.as_ref(), &node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Listener, NodeSnapshot};

    fn proc(pid: u32, cpu: f32, ports: &[u16]) -> Process {
        Process { pid, name: format!("p{pid}"), cpu_pct: cpu, ports: ports.to_vec(), ..Default::default() }
    }

    /// cell1 runs 40 processes, its busiest at pid 40; an idle nginx owns 8080 and an idle
    /// sshd shows as a listener's pid only. cell2 runs 5.
    fn snapshot() -> ClusterSnapshot {
        let mut a = NodeSnapshot { id: "cell1".into(), ..Default::default() };
        a.processes = (1..=40).map(|p| proc(p, p as f32, &[])).collect();
        a.processes.push(proc(100, 0.0, &[8080]));
        a.processes.push(proc(101, 0.0, &[]));
        a.listeners = vec![Listener { port: 22, addr: "0.0.0.0".into(), pid: Some(101), ..Default::default() }];
        let mut b = NodeSnapshot { id: "cell2".into(), ..Default::default() };
        b.processes = (1..=5).map(|p| proc(p, 1.0, &[])).collect();
        ClusterSnapshot { nodes: vec![a, b], ..Default::default() }
    }

    #[test]
    fn the_broadcast_keeps_the_15_busiest_and_every_listeners_owner() {
        let mut snap = snapshot();
        let trimmed = trim_for_ui(&mut snap);
        let pids: Vec<u32> = snap.nodes[0].processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids, (26..=40).chain([100, 101]).collect::<Vec<u32>>(), "the busiest 15, then the port's owner and the listener's pid, in the node's order");
        assert_eq!(snap.nodes[0].process_count, Some(42));
        assert_eq!((snap.nodes[1].processes.len(), snap.nodes[1].process_count), (5, Some(5)), "a short list is sent whole");
        restore(&mut snap, trimmed);
        let back: Vec<u32> = snap.nodes[0].processes.iter().map(|p| p.pid).collect();
        assert_eq!(back, snapshot().nodes[0].processes.iter().map(|p| p.pid).collect::<Vec<u32>>(), "everything back, in order");
    }

    #[test]
    fn the_whole_list_is_there_for_whoever_asks() {
        let snap = snapshot();
        assert_eq!(processes_of(Some(&snap), "cell1").unwrap().len(), 42);
        assert!(processes_of(Some(&snap), "cell9").unwrap_err().contains("cell9"));
        assert!(processes_of(None, "cell1").unwrap().is_empty(), "no snapshot yet");
    }

    #[test]
    fn what_rust_reads_keeps_every_process() {
        // publish(): trim, emit, restore; state.latest gets the restored snapshot. The PID
        // guard and process lookups read that one.
        let mut snap = snapshot();
        let trimmed = trim_for_ui(&mut snap);
        let emitted = serde_json::to_value(&snap).unwrap();
        restore(&mut snap, trimmed);
        assert_eq!(emitted["nodes"][0]["processes"].as_array().unwrap().len(), 17);
        assert_eq!(emitted["nodes"][0]["processCount"], 42);
        assert!(snap.nodes[0].processes.iter().any(|p| p.pid == 1), "an idle process is still found by pid");
    }

    /// Publish's overlays, standing in: one marks the snapshot, one reads the whole process list.
    fn overlays(seen: &mut usize) -> impl FnOnce(&mut ClusterSnapshot) + '_ {
        move |s: &mut ClusterSnapshot| {
            *seen = s.nodes[0].processes.len();
            s.nodes[0].name = "cell1 (overlaid)".into();
            s.nodes[1].stale = true;
        }
    }

    #[test]
    fn publish_with_emits_the_overlaid_snapshot_cut_and_keeps_every_process() {
        let mut snap = snapshot();
        let mut seen_by_overlays = 0;
        let mut emitted = None;
        publish_with(&mut snap, overlays(&mut seen_by_overlays), |s| emitted = Some(serde_json::to_value(s).unwrap()));
        let emitted = emitted.expect("emitted once");
        assert_eq!(seen_by_overlays, 42, "the overlays (the alerting among them) see every process");
        // The windows get the overlays' work, and per node at most the 15 busiest plus the listeners' owners.
        assert_eq!((emitted["nodes"][0]["name"].as_str(), emitted["nodes"][1]["stale"].as_bool()), (Some("cell1 (overlaid)"), Some(true)));
        let pids: Vec<u64> = emitted["nodes"][0]["processes"].as_array().unwrap().iter().map(|p| p["pid"].as_u64().unwrap()).collect();
        assert_eq!(pids, (26..=40).chain([100, 101]).collect::<Vec<u64>>());
        assert!(pids.len() <= TOP + 2);
        assert_eq!((emitted["nodes"][0]["processCount"].as_u64(), emitted["nodes"][1]["processCount"].as_u64()), (Some(42), Some(5)));
        assert_eq!(emitted["nodes"][1]["processes"].as_array().unwrap().len(), 5);
        // Afterwards (what goes into state.latest): the overlays kept, every process back, in order.
        assert_eq!(snap.nodes[0].name, "cell1 (overlaid)");
        assert_eq!(snap.nodes[0].processes.iter().map(|p| p.pid).collect::<Vec<_>>(), snapshot().nodes[0].processes.iter().map(|p| p.pid).collect::<Vec<_>>());
        assert_eq!(snap.nodes[1].processes.len(), 5);
    }

    #[test]
    fn a_panic_in_publish_with_never_loses_the_processes() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        // An overlay that panics: nothing was cut yet.
        let mut snap = snapshot();
        let r = catch_unwind(AssertUnwindSafe(|| publish_with(&mut snap, |_| panic!("an overlay failed"), |_| unreachable!("no emit after a failed overlay"))));
        assert!(r.is_err());
        assert_eq!(snap.nodes[0].processes.len(), 42);
        // An emit that panics: the cut lists go back on the way out.
        let r = catch_unwind(AssertUnwindSafe(|| {
            publish_with(&mut snap, |_| {}, |s| {
                assert_eq!(s.nodes[0].processes.len(), 17, "cut for the windows");
                panic!("the emit failed")
            })
        }));
        assert!(r.is_err());
        assert_eq!(snap.nodes[0].processes.iter().map(|p| p.pid).collect::<Vec<_>>(), snapshot().nodes[0].processes.iter().map(|p| p.pid).collect::<Vec<_>>(), "every process back");
    }
}
