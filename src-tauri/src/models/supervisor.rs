//! Keep running for SparkBox's multi-node loads. Docker's restart policy can bring one
//! container back, but a multi-node load only recovers when it is restarted in order (the
//! head stops, the workers stop, the workers start, the head starts last), so SparkBox does
//! that itself: when the load goes down because something in it exited or stopped
//! answering, it restarts after 30 s, then 2, 5 and 10 minutes. After five restarts in
//! half an hour, or five in a row that never got back to Ready however long each took,
//! it gives up and says so, rather than looping forever. A node that is offline isn't
//! something a restart fixes: it waits for the node instead. Nor is a failed start: a
//! load that never became Ready, or whose last start got a verdict (the wrong answer,
//! its time limit), stays Failed with its reason and its containers (`failed_start`). Nor is
//! a load still starting or compiling, this restart's included: stopping it would only throw
//! the compile away (`restartable`).
//!
//! Look-only holds it ("Held · look-only"): nothing is scheduled, and when look-only ends the
//! back-off starts again from then, so no retry fires at the time it had before. A retry
//! scheduled before a relock (not an idle one: AdminGate::hold_generation) is dropped when it
//! comes due ("Held · locked"), and the back-off starts again from then. With Live data it acts
//! only on loads whose containers carry this SparkBox's `sparkbox.instance` label; the card of
//! any other load says Keep running won't restart it (`not_ours`). A failed restart rolled its
//! containers back, so that one goes by its own record instead.

use super::endpoints::{Endpoint, Health, SupervisorState, SupervisorView};
use super::DeploymentState;
use crate::admin::{Hold, HoldView};
use crate::model::ClusterSnapshot;

/// Waits before each restart, in seconds; the last one repeats.
pub const BACKOFF_S: [u64; 4] = [30, 120, 300, 600];
/// It gives up after this many restarts within `WINDOW_MS`.
pub const GIVE_UP_AFTER: usize = 5;
pub const WINDOW_MS: u64 = 30 * 60_000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Supervisor {
    /// When it restarted the load (unix ms), within the window.
    pub restarts: Vec<u64>,
    /// Restarts since it was last Ready. Only Ready clears them: a load that fails more slowly
    /// than the window still runs out of tries.
    pub unproven: usize,
    pub next_retry_ms: Option<u64>,
    pub gave_up: bool,
    /// The gate's hold generation when `next_retry_ms` was set.
    pub scheduled_gen: u64,
    /// Why it isn't acting: look-only is on, or a relock dropped the last retry (kept until
    /// the next one fires).
    pub held: Option<Hold>,
    /// Why it never restarts this load, which the card says instead of a bare "Keep running":
    /// with Live data it isn't this SparkBox's (`not_ours`), or the load failed to start.
    pub refused: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Wait,
    /// Restart now; this is attempt n (1-based).
    Restart(usize),
}

/// Keep running brings back a load that ran. This one is Failed and never became Ready, or its
/// last start got a verdict (the wrong answer, its time limit): another start would most likely
/// end the same way, 90 minutes later for the limit, and would replace the containers kept for
/// their logs.
fn failed_start(e: &Endpoint) -> bool {
    e.deployment.as_ref().is_some_and(|d| d.0 == DeploymentState::Failed) && !e.recoverable
}

/// Whether a restart could help: it's down, no node it needs is offline or silent, and it isn't
/// a failed start, one still running (`Models::follow` decides that one, at its limit) or one
/// whose log says it's compiling, whatever its state says (a container restarted by hand).
pub fn restartable(e: &Endpoint, online: impl Fn(&str) -> bool) -> bool {
    let starting = e.deployment.as_ref().is_some_and(|d| d.0 == DeploymentState::Starting);
    e.health == Health::Down && !starting && !e.compiling && !failed_start(e) && e.expected_nodes.iter().all(|n| online(n))
}

/// A restart that failed and rolled back: nothing of the load is left on the cluster, only its
/// record, which is this SparkBox's own and says it ran (`recoverable`).
fn rolled_back(e: &Endpoint) -> bool {
    e.members.is_empty() && e.recoverable && e.deployment.as_ref().is_some_and(|d| d.0 == DeploymentState::Failed)
}

/// Why the load isn't this SparkBox's to restart, as its card says it, or None when it is: it
/// is when every container of it in the snapshot carries `sparkbox.instance=<instance>`, or none
/// is left because its restart rolled back. One without the label, or the other SparkBox's, is
/// left alone, and so is any other load with no containers to look at.
pub fn not_ours(e: &Endpoint, snap: &ClusterSnapshot, instance: &str) -> Option<String> {
    if rolled_back(e) {
        return None;
    }
    if e.members.is_empty() {
        return Some(format!("No containers to check for sparkbox.instance={instance}: won't restart it"));
    }
    let labelled = e.members.iter().all(|m| {
        let c = snap.nodes.iter().find(|n| n.id == m.node).and_then(|n| n.containers.iter().find(|c| c.id == m.id));
        c.and_then(|c| c.labels.get("sparkbox.instance")).is_some_and(|i| i == instance)
    });
    (!labelled).then(|| format!("Not this SparkBox's load (no sparkbox.instance={instance} label): won't restart it"))
}

impl Supervisor {
    /// One round for a load (models/watch.rs `supervise`): with Live data, one that isn't this
    /// SparkBox's (`not_ours`) is never restarted, nor is a failed start, and the card gives the
    /// reason; otherwise as `tick`, restartable when it's down and its nodes are `online`.
    #[allow(clippy::too_many_arguments)]
    pub fn round(&mut self, e: &Endpoint, snap: &ClusterSnapshot, live: bool, instance: &str, online: impl Fn(&str) -> bool, hold: HoldView, now_ms: u64) -> Action {
        self.refused = if live { not_ours(e, snap, instance) } else { None }.or_else(|| failed_start(e).then(|| "It failed to start: won't restart it".to_string()));
        if e.compiling {
            // Scheduled while it was down: compiling, it's back, and a retry mustn't wait for it.
            self.next_retry_ms = None;
        }
        let can = self.refused.is_none() && restartable(e, online);
        self.tick(e.health, can, hold, now_ms)
    }

    /// One round: what to do about an endpoint that is (or isn't) `restartable`, given the
    /// gate's hold state now.
    pub fn tick(&mut self, health: Health, restartable: bool, hold: HoldView, now_ms: u64) -> Action {
        // Before the window ages them: the card's reason quotes the count it gave up at.
        if self.gave_up {
            return Action::Wait;
        }
        self.restarts.retain(|t| now_ms.saturating_sub(*t) < WINDOW_MS);
        if health == Health::Ready {
            self.unproven = 0;
        }
        if hold.look_only {
            // Nothing scheduled, so nothing fires at the old time when look-only ends.
            self.next_retry_ms = None;
            self.held = Some(Hold::LookOnly);
            return Action::Wait;
        }
        if self.held == Some(Hold::LookOnly) {
            self.held = None;
        }
        if !restartable {
            // Back (or loading, or stopped on purpose, or waiting for a node): nothing scheduled.
            if health != Health::Down {
                self.next_retry_ms = None;
                self.held = None;
            }
            return Action::Wait;
        }
        if self.tries() >= GIVE_UP_AFTER {
            self.gave_up = true;
            self.next_retry_ms = None;
            return Action::Wait;
        }
        let wait = BACKOFF_S[self.tries().min(BACKOFF_S.len() - 1)];
        match self.next_retry_ms {
            None => {
                self.next_retry_ms = Some(now_ms + wait * 1000);
                self.scheduled_gen = hold.generation;
                Action::Wait
            }
            Some(at) if now_ms >= at => {
                if let Some(why) = hold.held_since(self.scheduled_gen) {
                    // Scheduled before a relock: dropped, and the back-off starts again from now.
                    self.held = Some(why);
                    self.next_retry_ms = Some(now_ms + wait * 1000);
                    self.scheduled_gen = hold.generation;
                    return Action::Wait;
                }
                self.restarts.push(now_ms);
                self.unproven += 1;
                self.next_retry_ms = None;
                self.held = None;
                Action::Restart(self.tries())
            }
            Some(_) => Action::Wait,
        }
    }

    /// Restarts that count towards giving up: those in the window, or those in a row with no Ready.
    fn tries(&self) -> usize {
        self.restarts.len().max(self.unproven)
    }

    /// A restart by hand starts over.
    pub fn reset(&mut self) {
        *self = Supervisor::default();
    }

    pub fn view(&self) -> SupervisorView {
        SupervisorView {
            enabled: true,
            state: if self.gave_up {
                SupervisorState::GaveUp
            } else if self.next_retry_ms.is_some() {
                SupervisorState::Restarting
            } else {
                SupervisorState::Watching
            },
            // Given up, the count its reason quotes; otherwise the card's "in 30 min".
            restarts: if self.gave_up { self.tries() } else { self.restarts.len() } as u32,
            next_retry_ms: self.next_retry_ms,
            held: self.refused.clone().or_else(|| self.held.map(|h| h.label().to_string())).filter(|_| !self.gave_up),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::testing::gate;
    use crate::admin::RelockReason;

    #[test]
    fn restarts_back_off_then_give_up_after_five_in_half_an_hour() {
        let (g, _, _) = gate();
        let mut s = Supervisor::default();
        let mut now = 1_000_000u64;
        let mut backoffs = Vec::new();
        loop {
            match s.tick(Health::Down, true, g.hold_view(), now) {
                // The restart fails again a little later.
                Action::Restart(_) => now += 5_000,
                Action::Wait if s.gave_up => break,
                Action::Wait => {
                    let at = s.next_retry_ms.unwrap();
                    assert_eq!(s.view().state, SupervisorState::Restarting);
                    backoffs.push((at - now) / 1000);
                    now = at;
                }
            }
        }
        assert_eq!(backoffs, [30, 120, 300, 600, 600], "30 s, 2, 5 and 10 minutes");
        assert_eq!(s.restarts.len(), GIVE_UP_AFTER);
        let v = s.view();
        assert_eq!((v.state, v.restarts, v.next_retry_ms), (SupervisorState::GaveUp, 5, None));
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), now + 60_000), Action::Wait, "gave up stays given up");
        s.reset();
        assert_eq!(s.view().state, SupervisorState::Watching);
    }

    /// Each restart loads for 90 minutes and fails at the limit. The window never holds two of
    /// them, but they're restarts in a row that never got back to Ready, and they run out.
    #[test]
    fn restarts_that_never_get_back_to_ready_give_up_however_slowly_they_fail() {
        use crate::models::endpoints::START_LIMIT_MS;
        let (g, _, _) = gate();
        let mut s = Supervisor::default();
        let mut now = 1_000_000u64;
        let (mut attempts, mut backoffs) = (Vec::new(), Vec::new());
        // Twelve hours of it, well past five restarts.
        for _ in 0..16 {
            match s.tick(Health::Down, true, g.hold_view(), now) {
                Action::Restart(n) => {
                    attempts.push(n);
                    assert_eq!(s.tick(Health::Loading, false, g.hold_view(), now + 60_000), Action::Wait);
                    now += START_LIMIT_MS;
                }
                Action::Wait if s.gave_up => break,
                Action::Wait => {
                    let at = s.next_retry_ms.unwrap();
                    backoffs.push((at - now) / 1000);
                    now = at;
                }
            }
        }
        assert_eq!(attempts, [1, 2, 3, 4, 5], "the task's \"(n of 5)\"");
        assert_eq!(backoffs, [30, 120, 300, 600, 600], "the same back-off");
        assert_eq!(s.view().state, SupervisorState::GaveUp);
        assert_eq!(s.view().restarts, 5, "\"Gave up after 5 restarts\": one is in the window");
        // Hours on, it has neither started again nor changed what it said.
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), now + 6 * 3_600_000), Action::Wait);
        assert_eq!(s.view().restarts, 5);
    }

    /// "Gave up after 5 restarts" stays 5 once the window has moved on.
    #[test]
    fn the_count_it_gave_up_at_stays() {
        let (g, _, _) = gate();
        let now = 1_000_000u64;
        let mut s = Supervisor { restarts: vec![now; GIVE_UP_AFTER], ..Default::default() };
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), now), Action::Wait);
        assert!(s.gave_up);
        s.tick(Health::Down, true, g.hold_view(), now + 2 * WINDOW_MS);
        assert_eq!(s.view().restarts, GIVE_UP_AFTER as u32);
    }

    /// A load that gets back to Ready each time has only the window against it, as ever.
    #[test]
    fn ready_between_restarts_starts_the_count_again() {
        let (g, _, _) = gate();
        let mut s = Supervisor::default();
        let mut now = 1_000_000u64;
        for _ in 0..8 {
            assert_eq!(s.tick(Health::Down, true, g.hold_view(), now), Action::Wait);
            assert_eq!(s.next_retry_ms, Some(now + 30_000), "the first wait");
            now += 30_000;
            assert_eq!(s.tick(Health::Down, true, g.hold_view(), now), Action::Restart(1));
            now += WINDOW_MS;
            assert_eq!(s.tick(Health::Ready, false, g.hold_view(), now), Action::Wait);
        }
        assert!(!s.gave_up);
        // Even while look-only holds everything else.
        g.set_look_only(true).unwrap();
        s.unproven = 4;
        s.tick(Health::Ready, false, g.hold_view(), now);
        assert_eq!(s.unproven, 0);
    }

    /// A load that ran and fell over is Keep running's. One Failed that never became Ready, or
    /// whose last start got a verdict, isn't, and its card says so; one still starting is
    /// `Models::follow`'s, even when its probes read Down.
    #[test]
    fn a_failed_start_is_left_alone_and_its_card_says_so() {
        let (g, _, _) = gate();
        let snap = ClusterSnapshot::default();
        let online = |_: &str| true;
        let load = |state, recoverable| Endpoint {
            health: Health::Down,
            expected_nodes: vec!["cell1".into(), "cell2".into()],
            deployment: Some((state, None, String::new(), 0)),
            recoverable,
            ..Default::default()
        };

        let failed = load(DeploymentState::Failed, false);
        assert!(failed_start(&failed) && !restartable(&failed, online));
        let mut s = Supervisor::default();
        for t in [0, 30_000, 31_000, 600_000, 3_600_000] {
            assert_eq!(s.round(&failed, &snap, false, "dev", online, g.hold_view(), t), Action::Wait, "never a restart");
        }
        let v = s.view();
        assert_eq!((v.state, v.next_retry_ms, v.restarts), (SupervisorState::Watching, None, 0));
        assert_eq!(v.held.as_deref(), Some("It failed to start: won't restart it"));

        let starting = load(DeploymentState::Starting, false);
        assert!(!failed_start(&starting) && !restartable(&starting, online));
        let mut s = Supervisor::default();
        assert_eq!(s.round(&starting, &snap, false, "dev", online, g.hold_view(), 0), Action::Wait);
        assert_eq!((s.next_retry_ms, s.view().held), (None, None), "no reason to give: it isn't over");

        // Ran, then failed: restarted after the first wait, and still when a restart fails.
        let ran = load(DeploymentState::Failed, true);
        assert!(!failed_start(&ran) && restartable(&ran, online));
        let mut s = Supervisor::default();
        assert_eq!(s.round(&ran, &snap, false, "dev", online, g.hold_view(), 0), Action::Wait);
        assert_eq!(s.round(&ran, &snap, false, "dev", online, g.hold_view(), 30_000), Action::Restart(1));
        assert_eq!(s.view().held, None);
        assert_eq!(s.round(&ran, &snap, false, "dev", online, g.hold_view(), 31_000), Action::Wait);
        assert_eq!(s.round(&ran, &snap, false, "dev", online, g.hold_view(), 151_000), Action::Restart(2));

        // A load nobody failed reads as before.
        assert!(restartable(&load(DeploymentState::Ready, false), online));
        assert!(!restartable(&Endpoint { health: Health::Loading, ..load(DeploymentState::Failed, true) }, online));
    }

    /// A restart that fails rolls back: every container goes, and all that's left of a load that
    /// ran is its Failed record, this SparkBox's own. With Live data Keep running tries it again
    /// on its back-off and still runs out. No other load gets that pass.
    #[test]
    fn keep_running_tries_a_failed_restart_again_once_its_containers_went() {
        use super::super::endpoints::Member;
        let (g, _, _) = gate();
        let mut snap = ClusterSnapshot::default();
        let online = |_: &str| true;
        let load = |state, recoverable| Endpoint {
            health: Health::Down,
            expected_nodes: vec!["cell1".into(), "cell2".into()],
            deployment: Some((state, None, String::new(), 0)),
            recoverable,
            ..Default::default()
        };

        let failed = load(DeploymentState::Failed, true);
        let mut s = Supervisor::default();
        let mut now = 1_000_000u64;
        let mut backoffs = Vec::new();
        for attempt in 1..=GIVE_UP_AFTER {
            assert_eq!(s.round(&failed, &snap, true, "dev", online, g.hold_view(), now), Action::Wait);
            assert_eq!(s.view().held, None, "nothing to refuse: it's waiting to try again");
            let at = s.next_retry_ms.expect("a retry is scheduled");
            backoffs.push((at - now) / 1000);
            assert_eq!(s.round(&failed, &snap, true, "dev", online, g.hold_view(), at), Action::Restart(attempt));
            now = at + 5_000;
        }
        assert_eq!(backoffs, [30, 120, 300, 600, 600], "the promised back-off, not one try");
        assert_eq!(s.round(&failed, &snap, true, "dev", online, g.hold_view(), now), Action::Wait);
        assert_eq!(s.view().state, SupervisorState::GaveUp);

        // No record, or one that isn't a failed restart: nothing says these are its own.
        let unknown = "No containers to check for sparkbox.instance=dev: won't restart it";
        for e in [load(DeploymentState::Failed, false), load(DeploymentState::Ready, true), load(DeploymentState::Starting, true), Endpoint { deployment: None, ..load(DeploymentState::Failed, true) }] {
            assert_eq!(not_ours(&e, &snap, "dev").as_deref(), Some(unknown), "{:?}", e.deployment);
        }
        // Anything left of it still answers for it.
        let mut n = crate::model::NodeSnapshot { id: "cell1".into(), ..Default::default() };
        n.containers.push(crate::model::Container { id: "c1".into(), ..Default::default() });
        snap.nodes.push(n);
        let left = Endpoint { members: vec![Member { node: "cell1".into(), id: "c1".into(), ..Default::default() }], ..load(DeploymentState::Failed, true) };
        assert_eq!(not_ours(&left, &snap, "dev").as_deref(), Some("Not this SparkBox's load (no sparkbox.instance=dev label): won't restart it"));
    }

    /// A restart throws a compile away, and the next start does it all again: a load still
    /// starting or compiling is never restarted, whatever its health reads, and that includes the
    /// load Keep running restarted itself.
    #[test]
    fn keep_running_never_restarts_a_load_thats_starting_or_compiling() {
        use crate::models::plan::Stage;
        let (g, _, _) = gate();
        let snap = ClusterSnapshot::default();
        let online = |_: &str| true;
        let compile = Stage::Compiling { what: "FlashInfer kernels".into() };
        let load = |state, stage: Option<Stage>, compiling| Endpoint {
            health: Health::Down,
            expected_nodes: vec!["cell1".into(), "cell2".into()],
            deployment: Some((state, stage, String::new(), 0)),
            recoverable: true,
            compiling,
            ..Default::default()
        };
        let hour = 3_600_000u64;

        // Down for a while, and Keep running restarts it.
        let mut s = Supervisor::default();
        let up = load(DeploymentState::Ready, None, false);
        assert_eq!(s.round(&up, &snap, false, "dev", online, g.hold_view(), 0), Action::Wait);
        assert_eq!(s.round(&up, &snap, false, "dev", online, g.hold_view(), 30_000), Action::Restart(1));

        // Its own restart, starting: 90 minutes of it, whatever reads Down, and nothing is scheduled.
        let restarted = load(DeploymentState::Starting, Some(compile.clone()), true);
        for t in (31_000..90 * 60_000).step_by(60_000) {
            assert_eq!(s.round(&restarted, &snap, false, "dev", online, g.hold_view(), t), Action::Wait, "at {t}");
            assert_eq!(s.next_retry_ms, None, "at {t}");
        }

        // A load that isn't Starting by its state, compiling by its log (restarted by hand).
        let compiling = load(DeploymentState::Ready, Some(Stage::Ready), true);
        assert!(!restartable(&compiling, online));
        let mut s = Supervisor::default();
        for t in (0..2 * hour).step_by(60_000) {
            assert_eq!(s.round(&compiling, &snap, false, "dev", online, g.hold_view(), t), Action::Wait, "at {t}");
            assert_eq!(s.next_retry_ms, None, "at {t}");
        }

        // A retry scheduled before it began compiling doesn't fire the moment the compile ends.
        let mut s = Supervisor::default();
        assert_eq!(s.round(&up, &snap, false, "dev", online, g.hold_view(), 0), Action::Wait);
        assert_eq!(s.next_retry_ms, Some(30_000));
        assert_eq!(s.round(&compiling, &snap, false, "dev", online, g.hold_view(), 10_000), Action::Wait);
        assert_eq!(s.next_retry_ms, None, "compiling, it's back");
        assert_eq!(s.round(&up, &snap, false, "dev", online, g.hold_view(), 600_000), Action::Wait, "down again: the wait starts over");
        assert_eq!(s.next_retry_ms, Some(630_000));

        // Hung past its limit (endpoints.rs stops calling it compiling): a restart may help again.
        let mut s = Supervisor::default();
        s.round(&compiling, &snap, false, "dev", online, g.hold_view(), 0);
        assert_eq!(s.round(&up, &snap, false, "dev", online, g.hold_view(), 2 * hour), Action::Wait);
        assert_eq!(s.round(&up, &snap, false, "dev", online, g.hold_view(), 2 * hour + 30_000), Action::Restart(1));
    }

    #[test]
    fn it_waits_for_offline_nodes_and_forgets_old_restarts() {
        let (g, _, _) = gate();
        let h = || g.hold_view();
        let mut s = Supervisor::default();
        assert_eq!(s.tick(Health::Down, false, h(), 0), Action::Wait, "a node is offline: a restart can't help");
        assert_eq!(s.next_retry_ms, None);
        assert_eq!(s.tick(Health::Down, true, h(), 0), Action::Wait);
        assert_eq!(s.tick(Health::Down, true, h(), 30_000), Action::Restart(1));
        // It came back: nothing scheduled while it's loading or ready.
        assert_eq!(s.tick(Health::Loading, false, h(), 60_000), Action::Wait);
        assert_eq!(s.tick(Health::Ready, false, h(), 200_000), Action::Wait);
        assert_eq!(s.view().state, SupervisorState::Watching);
        // Four restarts long ago don't count towards giving up.
        s.restarts = vec![1, 2, 3, 4];
        let later = WINDOW_MS + 10;
        assert_eq!(s.tick(Health::Down, true, h(), later), Action::Wait);
        assert_eq!(s.restarts.len(), 0);
        assert_eq!(s.next_retry_ms, Some(later + 30_000), "the first backoff again");
    }

    /// A retry scheduled before look-only never fires when it ends: look-only clears it, the
    /// card says "Held · look-only", and the back-off starts again from the moment it ended.
    #[test]
    fn a_held_supervisor_never_fires_on_release_and_its_back_off_starts_again() {
        let (g, _, _) = gate();
        let mut s = Supervisor::default();
        let t0 = 1_000_000u64;
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), t0), Action::Wait);
        assert_eq!(s.next_retry_ms, Some(t0 + 30_000), "a retry in 30 s");

        g.set_look_only(true).unwrap();
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), t0 + 10_000), Action::Wait);
        let v = s.view();
        assert_eq!((v.state, v.next_retry_ms, v.held.as_deref()), (SupervisorState::Watching, None, Some("Held · look-only")));
        // Look-only through the old time and well past it: nothing fires, nothing is scheduled.
        for t in [t0 + 30_000, t0 + 31_000, t0 + 600_000] {
            assert_eq!(s.tick(Health::Down, true, g.hold_view(), t), Action::Wait);
            assert_eq!(s.next_retry_ms, None);
        }

        g.set_look_only(false).unwrap();
        let off = t0 + 700_000;
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), off), Action::Wait, "no restart the moment it ends");
        assert_eq!(s.next_retry_ms, Some(off + 30_000), "the back-off starts again from the first wait");
        assert_eq!(s.view().held, None);
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), off + 29_000), Action::Wait);
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), off + 30_000), Action::Restart(1));
        assert!(s.restarts == [off + 30_000], "one restart, after the fresh wait");
    }

    /// A retry scheduled before an explicit relock (or a screen lock, a sleep) is dropped when it
    /// comes due and waits a full back-off from then; an idle relock leaves it alone.
    #[test]
    fn a_retry_scheduled_before_a_relock_is_dropped_but_an_idle_relock_keeps_it() {
        let (g, clock, _) = gate();
        let mut s = Supervisor::default();
        let t0 = 1_000_000u64;
        s.tick(Health::Down, true, g.hold_view(), t0);
        g.unlock(None).unwrap();
        clock.advance(16);
        assert!(g.tick(), "an idle relock");
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), t0 + 30_000), Action::Restart(1), "not held by an idle relock");

        s.tick(Health::Down, true, g.hold_view(), t0 + 40_000);
        let due = s.next_retry_ms.unwrap();
        g.lock_with(RelockReason::Explicit, "Locked from the top bar");
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), due), Action::Wait, "dropped");
        let v = s.view();
        assert_eq!(v.held.as_deref(), Some("Held · locked"));
        assert_eq!(v.next_retry_ms, Some(due + 120_000), "the back-off starts again from now");
        assert_eq!(s.tick(Health::Down, true, g.hold_view(), due + 120_000), Action::Restart(2), "scheduled after the relock: it runs");
        assert_eq!(s.view().held, None);
    }

    #[test]
    fn only_loads_labelled_with_this_instance_are_ours() {
        use super::super::endpoints::Member;
        let mut snap = ClusterSnapshot::default();
        for (node, id, label) in [("cell1", "c1", Some("dev")), ("cell2", "c2", Some("dev")), ("cell3", "c3", Some("release")), ("gx10-0000", "c4", None)] {
            let mut n = crate::model::NodeSnapshot { id: node.into(), ..Default::default() };
            let mut c = crate::model::Container { id: id.into(), ..Default::default() };
            if let Some(l) = label {
                c.labels.insert("sparkbox.instance".into(), l.into());
            }
            n.containers.push(c);
            snap.nodes.push(n);
        }
        let load = |ids: &[(&str, &str)]| Endpoint {
            members: ids.iter().map(|(n, id)| Member { node: n.to_string(), id: id.to_string(), ..Default::default() }).collect(),
            ..Default::default()
        };
        let ours = |e: &Endpoint, instance: &str| not_ours(e, &snap, instance).is_none();
        assert!(ours(&load(&[("cell1", "c1"), ("cell2", "c2")]), "dev"));
        assert!(!ours(&load(&[("cell1", "c1"), ("cell3", "c3")]), "dev"), "one container is the release app's");
        assert!(!ours(&load(&[("gx10-0000", "c4")]), "dev"), "no label: not this SparkBox's");
        assert!(!ours(&load(&[]), "dev"), "nothing to look at");
        assert!(ours(&load(&[("cell3", "c3")]), "release"));
        assert_eq!(crate::identity::instance_name(), "dev", "tests are debug builds");
    }

    /// With Live data, a down load whose containers lack this SparkBox's label is never
    /// restarted, and its card says why rather than a bare "Keep running"; one whose containers
    /// are gone says so too. Demo data has no labels to check.
    #[test]
    fn keep_running_says_it_wont_restart_a_load_that_isnt_ours() {
        use super::super::endpoints::Member;
        let (g, _, _) = gate();
        let mut snap = ClusterSnapshot::default();
        for node in ["cell1", "cell2"] {
            let mut n = crate::model::NodeSnapshot { id: node.into(), ..Default::default() };
            // Launched before P1.2: no sparkbox.instance label.
            n.containers.push(crate::model::Container { id: format!("{node}-c"), ..Default::default() });
            snap.nodes.push(n);
        }
        let down = |members: &[&str]| Endpoint {
            health: Health::Down,
            expected_nodes: vec!["cell1".into(), "cell2".into()],
            members: members.iter().map(|n| Member { node: n.to_string(), id: format!("{n}-c"), ..Default::default() }).collect(),
            ..Default::default()
        };
        let online = |_: &str| true;
        let mut s = Supervisor::default();
        for t in [0, 30_000, 31_000, 600_000, 3_600_000] {
            assert_eq!(s.round(&down(&["cell1", "cell2"]), &snap, true, "dev", online, g.hold_view(), t), Action::Wait, "never a restart");
        }
        let v = s.view();
        assert_eq!((v.state, v.next_retry_ms, v.restarts), (SupervisorState::Watching, None, 0));
        assert_eq!(v.held.as_deref(), Some("Not this SparkBox's load (no sparkbox.instance=dev label): won't restart it"));
        s.round(&down(&[]), &snap, true, "dev", online, g.hold_view(), 3_700_000);
        assert_eq!(s.view().held.as_deref(), Some("No containers to check for sparkbox.instance=dev: won't restart it"));

        // Labelled with this instance: no reason, and it restarts after the first wait.
        for n in &mut snap.nodes {
            n.containers[0].labels.insert("sparkbox.instance".into(), "dev".into());
        }
        let mut s = Supervisor::default();
        assert_eq!(s.round(&down(&["cell1", "cell2"]), &snap, true, "dev", online, g.hold_view(), 0), Action::Wait);
        assert_eq!(s.view().held, None);
        assert_eq!(s.round(&down(&["cell1", "cell2"]), &snap, true, "dev", online, g.hold_view(), 30_000), Action::Restart(1));
        // Demo: the unlabelled load is the simulator's own, and Keep running acts on it.
        let mut s = Supervisor::default();
        for n in &mut snap.nodes {
            n.containers[0].labels.clear();
        }
        s.round(&down(&["cell1", "cell2"]), &snap, false, "dev", online, g.hold_view(), 0);
        assert_eq!(s.round(&down(&["cell1", "cell2"]), &snap, false, "dev", online, g.hold_view(), 30_000), Action::Restart(1));
        assert_eq!(s.view().held, None);
    }
}
