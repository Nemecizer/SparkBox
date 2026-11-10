//! Keeping macOS out of the way of SparkBox's own work (P1.12).
//!
//! App Nap. While the data source is Live, SparkBox holds an NSProcessInfo activity
//! (NSActivityUserInitiatedAllowingIdleSystemSleep), so macOS doesn't nap the app and coalesce
//! the 1 s collector loop while the window is hidden in the menu bar. The Mac can still
//! idle-sleep.
//!
//! Sleep. `keep_awake("Restart cell2")` holds an IOPMAssertion (PreventUserIdleSystemSleep)
//! until the guard it returns is dropped, so the Mac doesn't idle-sleep halfway through an
//! orchestration and cut its SSH connections. Each guard is its own assertion, named
//! "SparkBox: <what>" (`pmset -g assertions` lists it). While one is held the status bar says
//! "Keeping this Mac awake while <what> runs": `awake://status` goes out on every change and
//! every 5 s while one is held, so a window that loads mid-run catches up.
//!
//! This round the orchestrations that exist hold one through their tasks (`follow_task`): a
//! restart or shutdown, and a Fleet run that changes nodes one at a time or may run for more
//! than a minute. A load's start sequence, "Update all nodes", the node maintenance engine (P6)
//! and password rotation call `keep_awake` themselves when they're built. macOS drops every
//! assertion SparkBox holds when it quits. No node command, no Tauri command.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::tasks::Task;

pub const STATUS_EVENT: &str = "awake://status";
const HEARTBEAT: Duration = Duration::from_secs(5);
/// A Fleet run whose time limit is longer than this may outlast the Mac's idle timer.
const LONG_FLEET_S: u64 = 60;

/// Every sleep assertion held, oldest first (the status bar's line).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AwakeStatus {
    pub holds: Vec<Hold>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hold {
    /// "Restart cell2", "Fleet: apt-get -y upgrade (as root)".
    pub what: String,
    pub since_ms: u64,
    /// The task it belongs to, when it's one (the status bar opens it).
    pub task: Option<String>,
}

/// What macOS is asked for (a fake in tests).
trait Backend: Send + Sync {
    /// Opts out of App Nap (true) or back in (false).
    fn nap_opt_out(&self, on: bool);
    /// Takes a PreventUserIdleSystemSleep assertion named `name`; its id lets it go.
    fn prevent_sleep(&self, name: &str) -> Result<u32, String>;
    fn allow_sleep(&self, id: u32);
}

type EmitFn = Box<dyn Fn(&AwakeStatus) + Send + Sync>;

pub struct Awake {
    backend: Box<dyn Backend>,
    inner: Mutex<Inner>,
    /// The guards held for running tasks, by task id. Locked before `inner`, never inside it.
    tasks: Mutex<HashMap<String, Guard>>,
    emit: Mutex<Option<EmitFn>>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    /// (guard id, what it's for, the assertion macOS gave).
    holds: Vec<(u64, Hold, u32)>,
    nap_opted_out: bool,
}

/// Keeps the Mac awake until dropped.
#[must_use = "the Mac stays awake only while the guard is held"]
pub struct Guard {
    id: u64,
    awake: Arc<Awake>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.awake.release(self.id);
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Awake {
    fn new(backend: Box<dyn Backend>) -> Arc<Awake> {
        Arc::new(Awake { backend, inner: Mutex::new(Inner::default()), tasks: Mutex::new(HashMap::new()), emit: Mutex::new(None) })
    }

    /// Holds a sleep assertion for `what` until the guard drops. When macOS refuses one, the
    /// guard holds nothing and the status bar doesn't claim it (the refusal goes to the log).
    fn hold(self: &Arc<Self>, what: &str, task: Option<String>) -> Guard {
        let mut inner = self.inner.lock().unwrap();
        inner.next += 1;
        let id = inner.next;
        match self.backend.prevent_sleep(&format!("SparkBox: {what}")) {
            Ok(assertion) => {
                inner.holds.push((id, Hold { what: what.to_string(), since_ms: now_ms(), task }, assertion));
                let status = status_of(&inner);
                drop(inner);
                self.send(&status);
            }
            Err(e) => crate::logging::write("awake", &format!("couldn't keep the Mac awake for {what}: {e}")),
        }
        Guard { id, awake: self.clone() }
    }

    fn release(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        let Some(at) = inner.holds.iter().position(|(i, _, _)| *i == id) else { return };
        let (_, _, assertion) = inner.holds.remove(at);
        self.backend.allow_sleep(assertion);
        let status = status_of(&inner);
        drop(inner);
        self.send(&status);
    }

    /// Holds the Mac awake while `t` runs, when it's one of this round's orchestrations (Live only).
    fn follow(self: &Arc<Self>, t: &Task) {
        let wanted = !t.finished() && !t.demo && orchestration(t);
        let mut tasks = self.tasks.lock().unwrap();
        if wanted {
            if !tasks.contains_key(&t.id) {
                let guard = self.hold(&t.title, Some(t.id.clone()));
                tasks.insert(t.id.clone(), guard);
            }
        } else {
            let gone = tasks.remove(&t.id);
            drop(tasks);
            drop(gone);
        }
    }

    fn set_live(&self, live: bool) {
        let mut inner = self.inner.lock().unwrap();
        if inner.nap_opted_out != live {
            self.backend.nap_opt_out(live);
            inner.nap_opted_out = live;
        }
    }

    pub fn status(&self) -> AwakeStatus {
        status_of(&self.inner.lock().unwrap())
    }

    fn send(&self, status: &AwakeStatus) {
        if let Some(emit) = self.emit.lock().unwrap().as_ref() {
            emit(status);
        }
    }
}

fn status_of(inner: &Inner) -> AwakeStatus {
    AwakeStatus { holds: inner.holds.iter().map(|(_, h, _)| h.clone()).collect() }
}

/// Whether a task is one of the orchestrations that keep the Mac awake this round: a restart or
/// shutdown (followed until the node is back or off), or a Fleet run that changes nodes one at a
/// time (canary, one by one, or one that may cut SparkBox's access) or may run over a minute.
fn orchestration(t: &Task) -> bool {
    match t.kind.as_str() {
        "power" => true,
        "fleet" if !t.read_only => {
            let arg = |k: &str| t.retry.as_ref().and_then(|r| r.args.get(k));
            let one_at_a_time = matches!(arg("strategy").and_then(|v| v.as_str()), Some("oneByOne" | "canary")) || arg("cutsAccess").and_then(|v| v.as_bool()) == Some(true);
            let long = arg("timeoutS").and_then(|v| v.as_u64()).is_some_and(|s| s > LONG_FLEET_S);
            one_at_a_time || long
        }
        _ => false,
    }
}

static AWAKE: OnceLock<Arc<Awake>> = OnceLock::new();

fn global() -> &'static Arc<Awake> {
    AWAKE.get_or_init(|| Awake::new(Box::new(mac::MacBackend::default())))
}

/// Keeps this Mac from idle-sleeping until the guard drops; the status bar says "Keeping this
/// Mac awake while `what` runs" meanwhile.
#[allow(dead_code)] // The hook for a load's start sequence, "Update all nodes", the P6 engine and password rotation (P1.12).
pub fn keep_awake(what: &str) -> Guard {
    global().hold(what, None)
}

/// Every task update (lib.rs's `tasks.on_update`): a power run or a long Fleet run holds the
/// Mac awake from its first update to its last.
pub fn follow_task(t: &Task) {
    global().follow(t);
}

/// The data source changed: App Nap is held off while it's Live.
pub fn set_live(live: bool) {
    global().set_live(live);
}

/// Sends `awake://status` to the window on every change and every 5 s while something is held.
pub fn start(app: &AppHandle) {
    let emitter = app.clone();
    *global().emit.lock().unwrap() = Some(Box::new(move |s: &AwakeStatus| {
        let _ = emitter.emit(STATUS_EVENT, s);
    }));
    tauri::async_runtime::spawn(async {
        let mut every = tokio::time::interval(HEARTBEAT);
        loop {
            every.tick().await;
            let status = global().status();
            if !status.holds.is_empty() {
                global().send(&status);
            }
        }
    });
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::sync::Mutex;

    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        // IOKit/pwr_mgt/IOPMLib.h. The CFStringRef arguments are NSStrings (toll-free bridged).
        fn IOPMAssertionCreateWithName(assertion_type: *const c_void, level: u32, name: *const c_void, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }
    /// kIOPMAssertionLevelOn.
    const LEVEL_ON: u32 = 255;

    /// The token beginActivityWithOptions:reason: returns.
    struct Activity(Retained<ProtocolObject<dyn NSObjectProtocol>>);
    // SAFETY: an opaque token that's only handed back to endActivity:; NSProcessInfo is
    // thread-safe, and the token is only touched under MacBackend's mutex.
    unsafe impl Send for Activity {}

    #[derive(Default)]
    pub struct MacBackend {
        activity: Mutex<Option<Activity>>,
    }

    impl super::Backend for MacBackend {
        fn nap_opt_out(&self, on: bool) {
            let mut held = self.activity.lock().unwrap();
            objc2::rc::autoreleasepool(|_| {
                let info = NSProcessInfo::processInfo();
                if on && held.is_none() {
                    let reason = NSString::from_str("SparkBox reads the cluster every second (Live data)");
                    *held = Some(Activity(info.beginActivityWithOptions_reason(NSActivityOptions::UserInitiatedAllowingIdleSystemSleep, &reason)));
                } else if !on {
                    if let Some(a) = held.take() {
                        // SAFETY: the token beginActivityWithOptions:reason: gave this process.
                        unsafe { info.endActivity(&a.0) };
                    }
                }
            });
        }

        fn prevent_sleep(&self, name: &str) -> Result<u32, String> {
            objc2::rc::autoreleasepool(|_| {
                let kind = NSString::from_str("PreventUserIdleSystemSleep");
                let name = NSString::from_str(name);
                let mut id = 0u32;
                // SAFETY: both strings outlive the call (IOKit copies them); `id` is written once.
                let r = unsafe { IOPMAssertionCreateWithName(Retained::as_ptr(&kind).cast(), LEVEL_ON, Retained::as_ptr(&name).cast(), &mut id) };
                if r == 0 {
                    Ok(id)
                } else {
                    Err(format!("IOPMAssertionCreateWithName returned {r:#x}"))
                }
            })
        }

        fn allow_sleep(&self, id: u32) {
            // SAFETY: an id IOPMAssertionCreateWithName gave this process; each is released once.
            unsafe { IOPMAssertionRelease(id) };
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod mac {
    #[derive(Default)]
    pub struct MacBackend;

    impl super::Backend for MacBackend {
        fn nap_opt_out(&self, _on: bool) {}
        fn prevent_sleep(&self, _name: &str) -> Result<u32, String> {
            Err("no sleep assertions on this system".into())
        }
        fn allow_sleep(&self, _id: u32) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{Retry, TaskState};

    /// Records what macOS would have been asked, and can refuse.
    #[derive(Default)]
    struct Fake {
        held: Arc<Mutex<Vec<(u32, String)>>>,
        naps: Arc<Mutex<Vec<bool>>>,
        refuse: bool,
    }

    impl Backend for Fake {
        fn nap_opt_out(&self, on: bool) {
            self.naps.lock().unwrap().push(on);
        }
        fn prevent_sleep(&self, name: &str) -> Result<u32, String> {
            if self.refuse {
                return Err("refused".into());
            }
            let mut held = self.held.lock().unwrap();
            let id = held.iter().map(|(i, _)| *i).max().unwrap_or(0) + 1;
            held.push((id, name.to_string()));
            Ok(id)
        }
        fn allow_sleep(&self, id: u32) {
            self.held.lock().unwrap().retain(|(i, _)| *i != id);
        }
    }

    /// What the fake backend holds (assertion id, name) and what the window was sent.
    type Held = Arc<Mutex<Vec<(u32, String)>>>;
    type Sent = Arc<Mutex<Vec<AwakeStatus>>>;

    fn fake() -> (Arc<Awake>, Held, Sent) {
        let backend = Fake::default();
        let held = backend.held.clone();
        let awake = Awake::new(Box::new(backend));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let s = sent.clone();
        *awake.emit.lock().unwrap() = Some(Box::new(move |st: &AwakeStatus| s.lock().unwrap().push(st.clone())));
        (awake, held, sent)
    }

    fn names(held: &Mutex<Vec<(u32, String)>>) -> Vec<String> {
        held.lock().unwrap().iter().map(|(_, n)| n.clone()).collect()
    }

    fn whats(s: &AwakeStatus) -> Vec<&str> {
        s.holds.iter().map(|h| h.what.as_str()).collect()
    }

    #[test]
    fn a_guard_holds_an_assertion_until_it_drops() {
        let (awake, held, sent) = fake();
        let restart = awake.hold("Restart cell2", None);
        assert_eq!(names(&held), ["SparkBox: Restart cell2"]);
        let fleet = awake.hold("Fleet: apt-get -y upgrade (as root)", None);
        assert_eq!(names(&held).len(), 2, "one assertion per guard");
        assert_eq!(whats(&awake.status()), ["Restart cell2", "Fleet: apt-get -y upgrade (as root)"], "oldest first");
        drop(restart);
        assert_eq!(names(&held), ["SparkBox: Fleet: apt-get -y upgrade (as root)"]);
        assert_eq!(whats(&awake.status()), ["Fleet: apt-get -y upgrade (as root)"]);
        drop(fleet);
        assert!(held.lock().unwrap().is_empty(), "every assertion released");
        assert!(awake.status().holds.is_empty());
        // The window heard each change, ending with nothing held.
        let sent = sent.lock().unwrap();
        assert_eq!(sent.iter().map(|s| s.holds.len()).collect::<Vec<_>>(), [1, 2, 1, 0]);
        let json = serde_json::to_value(&sent[0]).unwrap();
        assert_eq!(json["holds"][0]["what"], "Restart cell2");
        assert!(json["holds"][0]["sinceMs"].as_u64().unwrap() > 0 && json["holds"][0].get("task").is_some(), "{json}");
    }

    #[test]
    fn a_refused_assertion_isnt_claimed() {
        let backend = Fake { refuse: true, ..Default::default() };
        let awake = Awake::new(Box::new(backend));
        let guard = awake.hold("Restart cell2", None);
        assert!(awake.status().holds.is_empty(), "the status bar says nothing it isn't doing");
        drop(guard);
        assert!(awake.status().holds.is_empty());
    }

    fn task(kind: &str, title: &str, args: serde_json::Value) -> Task {
        Task { id: format!("t-{title}"), kind: kind.into(), title: title.into(), retry: Some(Retry { command: "x".into(), args }), ..Default::default() }
    }

    #[test]
    fn power_runs_and_long_fleet_runs_keep_the_mac_awake_while_they_run() {
        let (awake, held, _) = fake();
        let fleet = |title: &str, args: serde_json::Value| task("fleet", title, args);
        let mut restart = task("power", "Restart cell2", serde_json::json!({ "node": "cell2", "action": "reboot" }));
        let one_by_one = fleet("Fleet: sudo reboot", serde_json::json!({ "strategy": "oneByOne", "timeoutS": 60, "cutsAccess": true }));
        let long = fleet("Fleet: apt-get -y upgrade", serde_json::json!({ "strategy": "parallel", "timeoutS": 1800 }));
        let short = fleet("Fleet: systemctl restart earlyoom", serde_json::json!({ "strategy": "parallel", "timeoutS": 60 }));
        let read = Task { read_only: true, ..fleet("Fleet: journalctl -f", serde_json::json!({ "strategy": "oneByOne", "timeoutS": 3600 })) };
        let demo = Task { demo: true, ..task("power", "Shut down cell3", serde_json::json!({})) };
        let other = task("model", "Load llama-3.3-70b", serde_json::json!({}));
        for t in [&restart, &one_by_one, &long, &short, &read, &demo, &other] {
            awake.follow(t);
        }
        assert_eq!(whats(&awake.status()), ["Restart cell2", "Fleet: sudo reboot", "Fleet: apt-get -y upgrade"]);
        assert_eq!(awake.status().holds[0].task.as_deref(), Some("t-Restart cell2"));
        // Updates while it runs (steps, progress) don't take a second assertion.
        awake.follow(&restart);
        assert_eq!(names(&held).len(), 3);
        // Its last update lets go.
        restart.state = TaskState::Ok;
        restart.finished_ms = Some(1);
        awake.follow(&restart);
        assert_eq!(whats(&awake.status()), ["Fleet: sudo reboot", "Fleet: apt-get -y upgrade"]);
        for mut t in [one_by_one, long] {
            t.state = TaskState::Failed;
            t.finished_ms = Some(1);
            awake.follow(&t);
        }
        assert!(held.lock().unwrap().is_empty() && awake.status().holds.is_empty());
    }

    #[test]
    fn app_nap_is_held_off_only_while_live() {
        let backend = Fake::default();
        let naps = backend.naps.clone();
        let awake = Awake::new(Box::new(backend));
        awake.set_live(false);
        awake.set_live(true);
        awake.set_live(true);
        awake.set_live(false);
        assert_eq!(*naps.lock().unwrap(), [true, false], "asked once each way");
    }

    /// The real IOKit call, read back with `pmset -g assertions` (read-only). Owner-observable
    /// only: that the Mac then really skips idle sleep.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_lists_the_assertion_while_its_guard_is_held() {
        let awake = Awake::new(Box::new(mac::MacBackend::default()));
        let what = format!("unit test {}", std::process::id());
        let listed = || {
            let out = std::process::Command::new("/usr/bin/pmset").args(["-g", "assertions"]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).contains(&format!("SparkBox: {what}"))
        };
        let guard = awake.hold(&what, None);
        assert_eq!(awake.status().holds.len(), 1, "macOS granted it");
        assert!(listed(), "pmset lists it while held");
        drop(guard);
        let gone = (0..20).any(|_| {
            if !listed() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
            false
        });
        assert!(gone, "released on drop");
        // The App Nap activity begins and ends without complaint.
        awake.set_live(true);
        awake.set_live(false);
    }
}
