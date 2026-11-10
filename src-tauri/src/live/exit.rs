//! The one exit every Live command leaves through (P1.1, P1.2, P1.8, P1.41). `run`, `probe`,
//! `run_cancellable`, `stream_cmd`, `exec_io`, `pty`, `direct_tcpip`, sudo, `sparkjob`,
//! `sparkjob_root` and `container_action` each ask `Exit::admit` first, and it fails closed:
//!
//! 1. Look-only mode lets only reads through. Inference on a node (`as_use`), the lease and
//!    every change are refused, background work included.
//! 2. Reads pass. A tunnel's forward (`direct_tcpip`) is a `shell` change: a page on a node can
//!    do what a shell does, so the debug build's fence must name the node and allow shells
//!    (tunnels.rs tags each forward too). Inference passes without a fence: it changes no node.
//!    SparkBox's own bookkeeping (the lease) passes when not look-only.
//! 3. A change needs, in this order: in the debug build, an open fence for this build that names
//!    the node, the change's class and, when the fence lists them, its unit, container or package
//!    (SPARKBOX_ADMIN_NOAUTH counts for nothing without one; Cancel, exactly as
//!    `updates::cancel_command` writes it and tagged apt-one, needs only the class: stopping apt
//!    only reduces what installs; a fence naming `model-load` also admits `model-load-one`, never
//!    the other way, `FenceClass::wider`); the node answering as the machine it was pinned as; no
//!    container it names that is the other SparkBox's (whatever its class, a typed Fleet
//!    command's included), no unload of the other SparkBox's containers and no load labelled as
//!    one of them; and the node's controller lease, unless the other SparkBox holds it.
//!
//! The release build asks no fence (`BUILD_NEEDS_FENCE`). What stands there instead is each
//! command's gate, Admin mode confirmed with Touch ID or the Mac password. The exit doesn't ask
//! Admin again, so a download, a load or an upgrade that outlives the unlock isn't refused part-way
//! when Admin relocks, and its lease is renewed for as long as it runs (`Exit::renewals`). Only the
//! fence check is skipped: look-only, the identity pin, the other build's containers, the lease
//! and the activity log stand in both builds, and the fence class still names the change on its
//! log line. The debug build (agents, tests) always needs a fence, opened by the owner in the
//! release build. Only the compile decides which is which. Nothing at run time can move it, no
//! setting, environment variable, file or IPC call, and SPARKBOX_ADMIN_NOAUTH gains nothing. At
//! build time a debug-profile compile with debug assertions forced off would be the release build
//! in all but name, so it doesn't compile (the `compile_error!`s below `BUILD_NEEDS_FENCE`).
//!
//! Not every release change came through a gate. SparkBox's own background work goes out with
//! Admin locked, and only look-only holds it: Keep running's restart of a load that went down
//! (models/watch.rs `supervise`, one back-off after a relock held it), the stop of a copy's file
//! server (models/mod.rs `stop_share`), and the acknowledgements and maintenance ends owed to the
//! agents (agent/mod.rs `start`). Two others wait for Admin mode instead. A push of the alert
//! settings copies the owner's secrets as root (`agent::auto_push_due`). The removal of a staging
//! folder a copy left behind deletes on the node long after the copy's gate, and its sweep reaches
//! other idle staging folders there (files/mod.rs `retry_leftovers`).
//!
//! A call site's tag (`as_class`) names the class a command is checked against when its shape is
//! that class, a read or SparkBox's books; a command whose shape is `other` stays `other` under
//! any tag but the few literals `fence_class::tag_names` lists (Cancel, the PID-guarded signal,
//! Doctor's sshd check), so a bug in a tagged builder can't run under the class the owner opened.
//!
//! A typed (Fleet) command is `fleet`; a typed power verb *alone* is `power-one`, however a
//! shell, `sudo`, `su -c`, `env -S`, `eval`, `exec` or `find -exec` wraps it (`sh -c -e reboot`,
//! `bash -ic reboot`); one that also changes the node some other way (`touch x; reboot`) stays a
//! Fleet command (`classify::power_alone`), and so does one hidden in a substitution, a variable
//! or syntax SparkBox doesn't follow (destructive, never power-one). Typed Wi-Fi on or off, as the
//! whole command (`nmcli r wifi off`, `sudo -n nmcli radio wi on`), is `wifi`.
//!
//! Each refusal is one "Refused" line in the activity log, and an admitted change's line names
//! the fence class that let it through. A refused Files change's line has the whole command, as
//! the admitted one has, and says what it would have done ("Save notes.txt: …"). Background
//! checks (`probe`) may only read. Look-only turning on lets go of the leases here, whether or
//! not Admin was unlocked.
//!
//! A Wake-on-LAN packet leaves from this Mac or another node, not over the woken node's SSH:
//! `admit_from_mac` holds it in look-only and, in the debug build, checks the fence for that node
//! (`power-one`); the peer that relays it runs it as `as_use` (it changes nothing there).
//!
//! What SparkBox trusts (the fence key, a node's identity pin, a node's changed host key)
//! changes only after a real Touch ID: a scripted unlock never stands in (`trust_needs_touch_id`).
//!
//! The helper refresh into ~/.sparkbox (`install`, `ensure_file`) stays outside the exit: it's
//! the one write look-only allows, copying SparkBox's own helper scripts by checksum. Look-only
//! then runs only their reading verbs: sparkjob.py's other verbs change things, and they're gated.
//! launch.py isn't one of those helpers. `Live::refresh_file` copies it the same way, but a load
//! only sends it once this exit has admitted that rank's `deploy-files`, so the exit decides.
//!
//! In tests the exit is permissive (no fence, lease or pin, look-only off) unless a test opts
//! in with `Live::enforce_writes` and `Live::set_write_policy`. A test is a debug build, so an
//! enforced exit asks for a fence whichever instance its rules name; `Live::walk_release_path`
//! walks the release build's path instead. That seam exists only in test builds. A build given
//! `--cfg test` by hand can't be told from a test at compile time, so it compiles, and `run` (lib.rs)
//! stops it before it does anything.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use super::fence::{self, Record, State};
use super::fence_class::{self, class_of, Ctx, FenceClass, Item, Shape};
use super::{identity_pin, lease, sha256_hex, Live, SPARKJOB};
use crate::admin::{self, AdminGate};
use crate::errors::{ErrorKind, SbError};
use crate::live::audit;
use crate::ops::classify::{self, escalates, Kind};
use crate::AppState;

/// Whether this build's changes need a fence: the debug build's do, the release build's don't (see
/// the module notes). A constant the compile sets, so nothing at run time can move it.
pub const BUILD_NEEDS_FENCE: bool = cfg!(debug_assertions);

// The flag above and the profile cargo built must agree (build.rs). Without this, debug assertions
// switched off at compile time would give a target/debug build no fence, and the release identity
// with the owner's config folder and vault.
#[cfg(all(sparkbox_debug_profile, not(debug_assertions)))]
compile_error!("A debug-profile SparkBox needs debug assertions: without them it takes the release identity and needs no fence. Unset CARGO_PROFILE_*_DEBUG_ASSERTIONS and RUSTFLAGS.");
#[cfg(all(not(sparkbox_debug_profile), debug_assertions))]
compile_error!("A release-profile SparkBox can't have debug assertions: it would take the test build's identity and need a fence. Unset CARGO_PROFILE_*_DEBUG_ASSERTIONS and RUSTFLAGS.");

/// How a command leaves for the node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecKind {
    Run,
    Probe,
    Cancellable,
    Stream,
    ExecIo,
    Pty,
    DirectTcpip,
    Sudo,
    SudoInput,
    Sparkjob,
    SparkjobRoot,
    ContainerAction,
}

/// What a command does, as the exit judges it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Read,
    /// Inference on a node (an engine's /v1 endpoints): changes no node, held in look-only.
    Use,
    /// SparkBox's own bookkeeping on the node (the controller lease).
    Bookkeeping,
    Change(FenceClass),
}

/// Look-only and Admin mode, as the exit sees them: the app's gate, or a test's.
pub trait WritePolicy: Send + Sync {
    fn look_only(&self) -> bool;
    fn admin_unlocked(&self) -> bool;
}

impl WritePolicy for AdminGate {
    fn look_only(&self) -> bool {
        AdminGate::look_only(self)
    }

    fn admin_unlocked(&self) -> bool {
        self.state().unlocked
    }
}

/// Before the app's gate is handed over (`start`): the global look-only switch, never unlocked.
struct SystemPolicy;

impl WritePolicy for SystemPolicy {
    fn look_only(&self) -> bool {
        admin::look_only()
    }

    fn admin_unlocked(&self) -> bool {
        false
    }
}

/// Tests' default: nothing held.
#[cfg(test)]
struct OpenPolicy;

#[cfg(test)]
impl WritePolicy for OpenPolicy {
    fn look_only(&self) -> bool {
        false
    }

    fn admin_unlocked(&self) -> bool {
        true
    }
}

/// What a call site tagged a command with (`as_use`, `as_class`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Use,
    /// A call site's own class: the one the fence is asked to allow.
    Class(FenceClass),
}

tokio::task_local! {
    static TAG: Tag;
}

/// Runs `fut` as inference HTTP on a node (an engine's readiness, metrics and test token), or
/// as a relay for a change already admitted for another node (a Wake-on-LAN packet sent from a
/// peer): `Use`, whatever the command's shape. It changes nothing on the node it runs on.
pub async fn as_use<F: Future>(fut: F) -> F::Output {
    TAG.scope(Tag::Use, fut).await
}

/// Runs `fut` as a change of class `c`, tagged where the call is made: the exit judges a command
/// whose shape is `c`, a read or SparkBox's books `Change(c)`, with the items the shape names when
/// its class is `c` (none otherwise), and checks the fence, the identity pin, the other
/// SparkBox's containers and the lease for it as for any change. A shape of another class, or
/// `other` (but for `fence_class::tag_names`' literals: a PID-guarded signal is `ProcessTest`,
/// though its shape is several commands), keeps its own class.
pub async fn as_class<F: Future>(c: FenceClass, fut: F) -> F::Output {
    TAG.scope(Tag::Class(c), fut).await
}

/// What the exit let through: the fence class on its activity-log line, for a fenced change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Admitted {
    pub fence: Option<String>,
}

/// One `Exit::admit`, as tests see it (`Live::admits`): what was asked, the call site's tag,
/// whether it was typed, the verdict and whether it was let through.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub struct Admit {
    pub node: String,
    pub kind: ExecKind,
    pub cmd: String,
    pub tag: Option<Tag>,
    pub typed: bool,
    pub verdict: Verdict,
    pub admitted: bool,
}

/// What the exit enforces besides look-only: the fence, the lease and the identity pin.
#[derive(Clone, Debug)]
pub struct Enforce {
    /// release | dev: whose fences and leases are this build's.
    pub instance: String,
    /// The shared fence folder (SparkBox Fence/), which only a build that needs a fence checks.
    pub fence_dir: PathBuf,
    /// This build's own config folder (fence-trust.json, fence-failed.json, node_pins.json);
    /// None: next to the known hosts.
    pub config_dir: Option<PathBuf>,
    /// SPARKBOX_ADMIN_NOAUTH=1 in a debug build: refused unless a fence is open.
    pub noauth: bool,
    /// On the lease: the build tag and this Mac's name (None: read when first needed).
    pub tag: String,
    pub mac: Option<String>,
}

/// SPARKBOX_ADMIN_NOAUTH=1 in a debug build that isn't look-only: Admin opens without Touch ID
/// (the gate's own rule).
pub fn scripted_unlock() -> bool {
    cfg!(debug_assertions) && !admin::look_only_forced() && std::env::var("SPARKBOX_ADMIN_NOAUTH").is_ok_and(|v| v == "1")
}

/// What SparkBox trusts (the fence key, a node's identity pin, a node's changed host key) moves
/// only after a real Touch ID or password; a scripted unlock (`scripted`) confirms nothing.
pub fn trust_needs_touch_id(scripted: bool) -> Result<(), SbError> {
    if scripted {
        return Err(SbError::invalid("Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts."));
    }
    Ok(())
}

impl Enforce {
    /// The app's: this build, the shared fence folder, its own config folder.
    pub fn app() -> Enforce {
        Enforce {
            instance: crate::identity::instance_name().into(),
            fence_dir: fence::default_dir(),
            config_dir: None,
            noauth: scripted_unlock(),
            tag: lease::build_tag().into(),
            mac: None,
        }
    }

    fn who(&self) -> lease::Who {
        lease::Who { instance: self.instance.clone(), tag: self.tag.clone(), mac: self.mac.clone().unwrap_or_else(|| lease::mac_name().into()) }
    }
}

/// The exit's state (see the module notes).
pub struct Exit {
    policy: RwLock<Arc<dyn WritePolicy>>,
    enforce: RwLock<Option<Enforce>>,
    fence: fence::Verifier,
    pub(crate) leases: lease::Leases,
    pub(crate) pins: identity_pin::Pins,
    /// Background refusals already written, by node and command (one line per 10 minutes).
    quiet: Mutex<HashMap<(String, String), Instant>>,
    /// Every admit, in order (tests).
    #[cfg(test)]
    trace: Mutex<Vec<Admit>>,
    /// Tests only: walk the release build's path (`Live::walk_release_path`).
    #[cfg(test)]
    release_path: AtomicBool,
}

impl Default for Exit {
    /// Tests: permissive. The app: everything enforced, failing closed, with the global
    /// look-only switch until `start` hands over the gate.
    fn default() -> Exit {
        #[cfg(test)]
        let (policy, enforce): (Arc<dyn WritePolicy>, Option<Enforce>) = (Arc::new(OpenPolicy), None);
        #[cfg(not(test))]
        let (policy, enforce): (Arc<dyn WritePolicy>, Option<Enforce>) = (Arc::new(SystemPolicy), Some(Enforce::app()));
        Exit::with(policy, enforce)
    }
}

impl Exit {
    fn with(policy: Arc<dyn WritePolicy>, enforce: Option<Enforce>) -> Exit {
        Exit {
            policy: RwLock::new(policy),
            enforce: RwLock::new(enforce),
            fence: fence::Verifier::default(),
            leases: lease::Leases::default(),
            pins: identity_pin::Pins::default(),
            quiet: Mutex::new(HashMap::new()),
            #[cfg(test)]
            trace: Mutex::new(Vec::new()),
            #[cfg(test)]
            release_path: AtomicBool::new(false),
        }
    }

    /// An exit that enforces `e`, with the global look-only switch (the app's default, in tests).
    #[cfg(test)]
    pub fn enforcing(e: Enforce) -> Exit {
        Exit::with(Arc::new(SystemPolicy), Some(e))
    }

    fn policy(&self) -> Arc<dyn WritePolicy> {
        self.policy.read().unwrap().clone()
    }

    fn rules(&self) -> Option<Enforce> {
        self.enforce.read().unwrap().clone()
    }

    /// Whether a change here needs a fence: what the compile decided (`BUILD_NEEDS_FENCE`).
    fn needs_fence(&self) -> bool {
        #[cfg(test)]
        if self.release_path.load(Ordering::SeqCst) {
            return false;
        }
        BUILD_NEEDS_FENCE
    }

    /// Decides whether `cmd` may leave for `node` (see the module notes).
    pub async fn admit(&self, live: &Live, node: &str, kind: ExecKind, cmd: &str) -> Result<Admitted, SbError> {
        let cached = live.homes.lock().unwrap().get(node).cloned();
        let home = match cached {
            Some(h) => Some(h),
            // Paths under ~/sparkbox-test arrive as the node's real home: ask it once.
            None if cmd.contains("sparkbox-test") => node_home(live, node).await,
            None => None,
        };
        let containers = live.nodes.lock().unwrap().get(node).and_then(|s| s.frame.as_ref().map(|f| f.containers.clone())).unwrap_or_default();
        let root_lib = live.root_lib();
        let ctx = Ctx { containers: &containers, home: home.as_deref(), root_lib: &root_lib, sparkjob_sha: sparkjob_sha() };
        let (verdict, items) = verdict_of(kind, cmd, &ctx);
        let background = kind == ExecKind::Probe || verdict == Verdict::Use;
        // Containers the command stops, removes or restarts, whatever class let it through.
        let named = fence_class::named_containers(kind, cmd, &ctx);
        let whole = whole_change(verdict, kind, cmd);
        // A load's unload removes what its label filter finds: whose containers those are. A
        // load's container is labelled for one build: which.
        let unloads = fence_class::unload_instance(kind, cmd);
        let loads = fence_class::load_instance(kind, cmd);

        let result = async {
            if loads.is_some() { crate::ops::protected_updates::check_load(live, false, &[node.into()]).map_err(SbError::invalid)?; }
            // Background checks only read (inference on a node, tagged where it's asked, aside).
            if kind == ExecKind::Probe && !matches!(verdict, Verdict::Read | Verdict::Use) {
                // A debug build stops here, so a writing probe is found while it's being built.
                if cfg!(all(debug_assertions, not(test))) {
                    panic!("background checks only read: {cmd}");
                }
                return Err(self.refuse(live, node, cmd, SbError::invalid("Background checks only read; SparkBox didn't run this one."), false));
            }
            let policy = self.policy();
            if policy.look_only() && verdict != Verdict::Read {
                let e = SbError::new(ErrorKind::LookOnly, admin::LOOK_ONLY);
                return Err(if background { self.refuse(live, node, cmd, e, true) } else { self.refuse_change(live, node, kind, cmd, e) });
            }
            let class = match verdict {
                Verdict::Read | Verdict::Use | Verdict::Bookkeeping => return Ok(Admitted::default()),
                Verdict::Change(c) => c,
            };
            let Some(rules) = self.rules() else { return Ok(Admitted::default()) };
            match self.admit_change(live, node, Change { class, items: &items, whole, named: &named, unloads: unloads.as_deref(), loads: loads.as_deref() }, &containers, &rules).await {
                // Look-only can turn on while admit_change awaits the identity pin and the lease
                // claim (both over SSH): a change admitted before it came on is refused once it
                // has, so nothing runs on the node after look-only is on. The lease claim that
                // reached the node in that window is dropped here (the node's 45 s TTL frees it),
                // so `held()` stays empty even though the relock's hold hook ran before the claim.
                Ok(()) if self.policy().look_only() => {
                    self.leases.drop_local(node);
                    Err(self.refuse_change(live, node, kind, cmd, SbError::new(ErrorKind::LookOnly, admin::LOOK_ONLY)))
                }
                Ok(()) => Ok(Admitted { fence: Some(class.id().to_string()) }),
                Err(e) => Err(self.refuse_change(live, node, kind, cmd, e)),
            }
        }
        .await;
        #[cfg(test)]
        self.trace.lock().unwrap().push(Admit { node: node.into(), kind, cmd: cmd.into(), tag: TAG.try_with(|t| *t).ok(), typed: audit::in_typed(), verdict, admitted: result.is_ok() });
        result
    }

    /// Checks (a) to (e) for `change` on `node`.
    async fn admit_change(&self, live: &Live, node: &str, change: Change<'_>, containers: &[crate::model::Container], rules: &Enforce) -> Result<(), SbError> {
        let config = config_dir(live, rules);
        let now = now_ms();
        // (a) and (b), where this build needs a fence: scripted unlocks never stand in for the
        // owner's fence; the fence. The release build has no second check here: its gate is the
        // command's, and its background work has none (the module notes say which that is).
        if self.needs_fence() {
            self.fence_admits(&config, rules, node, &change, now)?;
        }
        // (c) The machine answering is the one pinned as this node.
        self.pins.check(live, &config, node, now).await?;
        // (d) The other SparkBox's containers are its own (matched by name, id, a short id prefix
        // or a leading `/name`; see `names_container`), and so are those an unload's label filter
        // finds.
        if let Some(owner) = change.unloads.filter(|i| *i != rules.instance) {
            return Err(SbError::new(ErrorKind::OtherController, format!("This unload on {node} would remove the other SparkBox's ({owner}) containers.")).on(node));
        }
        // A load labelled for the other build would be its container: it'd unload it as its own.
        if let Some(owner) = change.loads.filter(|i| *i != rules.instance) {
            return Err(SbError::new(ErrorKind::OtherController, format!("This load on {node} would start a container labeled as the other SparkBox's ({owner}).")).on(node));
        }
        for item in change.named {
            let found = item.iter().find_map(|a| fence_class::names_container(containers, a));
            if let Some(owner) = found.and_then(|c| c.labels.get("sparkbox.instance")).filter(|o| **o != rules.instance) {
                let name = found.map(|c| c.name.clone()).unwrap_or_default();
                return Err(SbError::new(ErrorKind::OtherController, format!("{name} on {node} is managed by the other SparkBox ({owner}).")).on(node));
            }
        }
        // (e) The controller lease.
        self.leases.claim(live, node, &rules.who()).await
    }

    /// The nodes whose lease this build renews this round: every node while Admin mode is
    /// unlocked, and this build's fence's while it's open. Where no fence stands behind a change
    /// (the release build) it's also the nodes work runs on (`working`), whatever Admin mode's
    /// state: a download, a load or an upgrade makes a change and then only reads, and the other
    /// build mustn't act on that node meanwhile. In the debug build a lease rests on Admin or the
    /// fence alone, so a closed fence lets go of it. None in look-only, which never writes the lease.
    fn renewals(&self, view: &FenceView, inventory: &[String], working: &[String]) -> Vec<String> {
        let policy = self.policy();
        if policy.look_only() {
            return Vec::new();
        }
        let mut nodes = if policy.admin_unlocked() {
            inventory.to_vec()
        } else if view.state == State::Valid && view.mine {
            view.record.as_ref().map(|r| r.nodes.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        if !self.needs_fence() {
            nodes.extend_from_slice(working);
        }
        nodes.sort();
        nodes.dedup();
        nodes
    }

    /// (a) and (b) for `change` on `node`.
    fn fence_admits(&self, config: &Path, rules: &Enforce, node: &str, change: &Change, now: u64) -> Result<(), SbError> {
        let checked = self.fence.check(&rules.fence_dir, config, now);
        let open = checked.state == State::Valid && checked.record.as_ref().is_some_and(|r| r.instance == rules.instance);
        if rules.noauth && !open {
            return Err(fenced("SPARKBOX_ADMIN_NOAUTH needs an open fence in Live.", node));
        }
        fence_allows(&checked, rules, node, change)
    }

    /// A change sent from this Mac rather than over the node's SSH (`what`: a Wake-on-LAN packet):
    /// held in look-only and, where this build needs a fence, checked against it for `node` and
    /// `class`. The node is off, so there's no identity pin or lease to ask; a peer that relays it
    /// runs it as `as_use`.
    pub fn admit_from_mac(&self, live: &Live, node: &str, class: FenceClass, what: &str) -> Result<Admitted, SbError> {
        if self.policy().look_only() {
            return Err(self.refuse(live, node, what, SbError::new(ErrorKind::LookOnly, admin::LOOK_ONLY), false));
        }
        let Some(rules) = self.rules() else { return Ok(Admitted::default()) };
        let change = Change { class, items: &[], whole: true, named: &[], unloads: None, loads: None };
        let fenced = if self.needs_fence() { self.fence_admits(&config_dir(live, &rules), &rules, node, &change, now_ms()) } else { Ok(()) };
        match fenced {
            Ok(()) => Ok(Admitted { fence: Some(class.id().to_string()) }),
            Err(e) => Err(self.refuse(live, node, what, e, false)),
        }
    }

    /// Writes the refusal's activity-log line and returns the error. A background refusal is
    /// written once per node and command every 10 minutes, as a probe line.
    fn refuse(&self, live: &Live, node: &str, cmd: &str, e: SbError, background: bool) -> SbError {
        if background {
            let key = (node.to_string(), cmd.to_string());
            let mut quiet = self.quiet.lock().unwrap();
            quiet.retain(|_, at| at.elapsed() < Duration::from_secs(600));
            if quiet.insert(key, Instant::now()).is_none() {
                live.audit.record(audit::Entry {
                    node: node.into(),
                    kind: "op".into(),
                    command: cmd.into(),
                    title: "Refused".into(),
                    error: Some(e.message.clone()),
                    state: "failed".into(),
                    class: Kind::Probe.as_str().into(),
                    ..Default::default()
                });
            }
        } else {
            live.audit.record_failed(node, "Refused", cmd, &e.message);
        }
        e.on(node)
    }

    /// A refused change's line. A Files change is written with the whole command, as its
    /// admitted line has it (`python3 ~/.sparkbox/sparkjob.py commit …`, root's checked copy for
    /// a sudo save), and its error starts with what it would have done ("Save notes.txt: The
    /// fence doesn't allow file changes."). The error returned is the exit's own.
    fn refuse_change(&self, live: &Live, node: &str, kind: ExecKind, cmd: &str, e: SbError) -> SbError {
        let (row, words) = match kind {
            ExecKind::Sparkjob => (format!("{}{cmd}", fence_class::SPARKJOB), fence_class::files_words(cmd)),
            ExecKind::SparkjobRoot => (super::root_sparkjob_command(&live.root_lib(), sparkjob_sha(), cmd), fence_class::files_words(cmd)),
            _ => match cmd.strip_prefix(fence_class::SPARKJOB) {
                Some(args) => (cmd.to_string(), fence_class::files_words(args)),
                None => (cmd.to_string(), fence_class::file_op_words(cmd)),
            },
        };
        let error = match words {
            Some(w) => format!("{w}: {}", e.message),
            None => e.message.clone(),
        };
        live.audit.record_failed(node, "Refused", &row, &error);
        e.on(node)
    }

    /// The fence's state for this build (Settings › Fence, the banner).
    pub fn fence_view(&self, live: &Live) -> FenceView {
        let rules = self.rules().unwrap_or_else(Enforce::app);
        let config = config_dir(live, &rules);
        let now = now_ms();
        let checked = self.fence.check(&rules.fence_dir, &config, now);
        let trusted = fence::load_trust(&config);
        let key = fence::load_key_file(&rules.fence_dir);
        let remaining_ms = match (&checked.state, &checked.record) {
            (State::Valid, Some(r)) => r.expires_ms.saturating_sub(now),
            _ => 0,
        };
        FenceView {
            state: checked.state,
            mine: checked.record.as_ref().is_some_and(|r| r.instance == rules.instance),
            summary: checked.record.as_ref().map(|r| r.summary(now)),
            record: checked.record,
            remaining_ms,
            key_kind: key.as_ref().map(|k| k.kind.clone()).unwrap_or_else(|| KEY_KIND.into()),
            key_id: key.map(|k| k.key_id),
            trusted_key_id: trusted.map(|t| t.key_id),
            instance: rules.instance.clone(),
            can_open: rules.instance == "release",
            can_trust: rules.instance == "dev" && crate::identity::SEPARATE_DEV_IDENTITY,
            why: checked.why,
        }
    }
}

/// The key kind this build makes: a software P-256 key in a Keychain item of its own that only
/// release-build code reads (the Secure Enclave key wasn't built this round).
pub const KEY_KIND: &str = "keychain";

/// What `fence_state` returns.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FenceView {
    pub state: State,
    pub record: Option<Record>,
    /// The fence is for this build.
    pub mine: bool,
    /// "cell2 · 6 classes · 2 h 41 min"
    pub summary: Option<String>,
    pub remaining_ms: u64,
    /// keychain | secureEnclave
    pub key_kind: String,
    /// The key in fence-key.pub, and the one this build trusts.
    pub key_id: Option<String>,
    pub trusted_key_id: Option<String>,
    /// release | dev
    pub instance: String,
    /// Only the release build opens fences.
    pub can_open: bool,
    /// The dev build trusts the release build's key by hand, once it has its own config folder.
    pub can_trust: bool,
    pub why: Option<String>,
}

fn fenced(message: &str, node: &str) -> SbError {
    SbError::new(ErrorKind::Fenced, message).on(node)
}

/// A change as the fence sees it: its class, the items it acts on, whether it acts on no item by
/// nature (`systemctl daemon-reload`), and the containers it names, the instance an unload's
/// label filter names and the instance a load labels its container for (check (d)).
struct Change<'a> {
    class: FenceClass,
    items: &'a [Item],
    whole: bool,
    named: &'a [Item],
    unloads: Option<&'a str>,
    loads: Option<&'a str>,
}

/// (b): the fence is valid, this build's, names the node and the class, and its `only` list
/// (when it has one for the class) names every item. A change that acts on no item by nature
/// (`systemctl daemon-reload`, a wake) needs only the class.
fn fence_allows(checked: &fence::Checked, rules: &Enforce, node: &str, change: &Change) -> Result<(), SbError> {
    let (class, items) = (change.class, change.items);
    let record = match (&checked.state, &checked.record) {
        (State::None, _) => return Err(fenced("No fence is open, so changes on the cluster are refused. This test build needs one: open it in the release SparkBox (Settings › Fence).", node)),
        (State::Untrusted, _) => return Err(fenced("This build doesn't trust the fence key yet, so changes on the cluster are refused.", node)),
        (State::Failed, _) => return Err(fenced("The fence record failed its check: open a new one.", node)),
        (State::Expired, Some(r)) => return Err(fenced(&format!("The fence expired at {}.", fence::clock(r.expires_ms)), node)),
        (State::Valid, Some(r)) => r,
        _ => return Err(fenced("No fence is open, so changes on the cluster are refused.", node)),
    };
    if record.instance != rules.instance {
        return Err(fenced(&format!("This fence is for the {} build.", record.instance), node));
    }
    if !record.nodes.iter().any(|n| n == node) {
        return Err(fenced(&format!("{node} isn't in the open fence."), node));
    }
    let any = record.classes.contains(&FenceClass::Any) && record.instance == "release";
    let wider = class.wider().filter(|w| record.classes.contains(w));
    if !any && !record.classes.contains(&class) && wider.is_none() {
        return Err(fenced(&format!("The fence doesn't allow {}.", class.words()), node));
    }
    let only = record.only.get(&class).or_else(|| wider.and_then(|w| record.only.get(&w)));
    if let Some(only) = only.filter(|o| !o.is_empty()).filter(|_| !record.classes.contains(&FenceClass::Any)) {
        let missing = items.iter().find(|aliases| !aliases.iter().any(|a| only.iter().any(|o| o == a || fence_class::same_id(o, a))));
        if (items.is_empty() && !change.whole) || missing.is_some() {
            let what = missing.and_then(|m| m.first()).map(|m| format!(" to {m}")).unwrap_or_default();
            return Err(fenced(&format!("The fence doesn't allow {}{what}: it names only {}.", class.words(), only.join(", ")), node));
        }
    }
    Ok(())
}

/// Whether a change needs only its class, whatever a fence's `only` list names (`Change::whole`).
fn whole_change(verdict: Verdict, kind: ExecKind, cmd: &str) -> bool {
    // Cancel (C-X), tagged apt-one where Updates sends it: stopping apt only reduces what
    // installs, so a fence that allows apt-one on the node admits it whatever its `only` list
    // names. Exactly that text, as root, and only as apt-one.
    let cancel = verdict == Verdict::Change(FenceClass::AptOne) && kind == ExecKind::Sudo && fence_class::upgrade_cancel_unit(cmd).is_some();
    fence_class::acts_on_no_item(kind, cmd) || cancel
}

/// The verdict, in the order the module notes give: a call-site class, inference, a typed
/// (Fleet) command, then the command's shape. Also the items the change acts on.
fn verdict_of(kind: ExecKind, cmd: &str, ctx: &Ctx) -> (Verdict, Vec<Item>) {
    let shape = class_of(kind, cmd, ctx);
    match TAG.try_with(|t| *t).ok() {
        Some(Tag::Class(c)) => return tagged(shape, kind, cmd, c),
        Some(Tag::Use) => return (Verdict::Use, Vec::new()),
        None => {}
    }
    if audit::in_typed() && kind != ExecKind::DirectTcpip {
        let v = classify::verdict(cmd);
        let read = matches!(kind, ExecKind::Run | ExecKind::Cancellable | ExecKind::Stream | ExecKind::Probe) && !escalates(cmd) && matches!(v.kind, Kind::Read | Kind::Probe);
        // A typed power verb alone needs `power-one`, not just `fleet`: Fleet runs it one node at a
        // time behind the canary, and power has its own fence class (P1 r2). A command that only
        // *contains* a power verb (`touch x; reboot`) stays a Fleet command, so its other parts
        // can't ride under `power-one` with no `fleet` (P1 r3). So does Wi-Fi on or off, typed as
        // the whole command (`nmcli radio wifi off`): `wifi` (P1 r3).
        let class = if classify::power_alone(cmd).is_some() {
            FenceClass::PowerOne
        } else if fence_class::typed_wifi(cmd) {
            FenceClass::Wifi
        } else {
            FenceClass::Fleet
        };
        return (if read { Verdict::Read } else { Verdict::Change(class) }, Vec::new());
    }
    from_shape(shape)
}

/// The verdict a call-site tag `c` gives a command of `kind` and `shape`: `Change(c)`, with the
/// shape's items when the shape's class is `c` (none otherwise). A tag can't relabel a
/// root-running command (Sudo/SudoInput/SparkjobRoot) to a non-sudo class, nor a command whose
/// shape already names a different, specific change class: the shape decides there (P1 r2). Nor
/// an `other`, but for the literals `fence_class::tag_names` lists (P1 r4): a builder's mistake
/// under the tag stays `other`, which only an `any` fence admits.
fn tagged(shape: Shape, kind: ExecKind, cmd: &str, c: FenceClass) -> (Verdict, Vec<Item>) {
    let sudo_kind = matches!(kind, ExecKind::Sudo | ExecKind::SudoInput | ExecKind::SparkjobRoot);
    let other = matches!(&shape, Shape::Change(FenceClass::Other, _)) && !fence_class::tag_names(kind, cmd, c);
    let shape_wins = other || (sudo_kind && !is_sudo_class(c)) || matches!(&shape, Shape::Change(k, _) if *k != c && *k != FenceClass::Other);
    if shape_wins {
        return from_shape(shape);
    }
    let items = match shape {
        Shape::Change(k, items) if k == c => items,
        _ => Vec::new(),
    };
    (Verdict::Change(c), items)
}

/// The verdict a shape gives directly (no tag, not typed).
fn from_shape(shape: Shape) -> (Verdict, Vec<Item>) {
    match shape {
        Shape::Read => (Verdict::Read, Vec::new()),
        Shape::Bookkeeping => (Verdict::Bookkeeping, Vec::new()),
        Shape::Change(c, items) => (Verdict::Change(c), items),
    }
}

/// The fence classes a root-running command (sudo and friends) may carry: a call-site tag on
/// Sudo/SudoInput/SparkjobRoot is honoured only for one of these.
fn is_sudo_class(c: FenceClass) -> bool {
    use FenceClass::*;
    matches!(c, SudoProbe | RootHelper | Service | PowerOne | AptRefresh | AptOne | AptHold | DropCaches | Wifi | RdpRepair)
}

/// SparkBox's sparkjob.py's SHA-256 (what root's copy must match), worked out once.
fn sparkjob_sha() -> &'static str {
    static SHA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SHA.get_or_init(|| sha256_hex(SPARKJOB.as_bytes()))
}

/// The cluster user's $HOME on `node`, read on its control connection (outside the exit: a
/// read) and kept like `Live::home_dir` keeps it.
async fn node_home(live: &Live, node: &str) -> Option<String> {
    let session = live.session(node)?;
    let out = session.exec(&super::wire("printf %s \"$HOME\""), None, Duration::from_secs(10)).await.ok()?;
    let home = out.stdout.trim().to_string();
    if out.code != Some(0) || !home.starts_with('/') {
        return None;
    }
    live.homes.lock().unwrap().insert(node.to_string(), home.clone());
    Some(home)
}

fn config_dir(live: &Live, rules: &Enforce) -> PathBuf {
    rules.config_dir.clone().unwrap_or_else(|| live.data_path("x").parent().map(Path::to_path_buf).unwrap_or_default())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Live {
    /// Look-only and Admin mode from `policy` (the app's gate, or a test's).
    pub fn set_write_policy(&self, policy: Arc<dyn WritePolicy>) {
        *self.exit.policy.write().unwrap() = policy;
    }

    /// Enforces the fence, the lease and the identity pin with these rules (tests opt in).
    #[cfg(test)]
    pub fn enforce_writes(&self, rules: Enforce) {
        *self.exit.enforce.write().unwrap() = Some(rules);
    }

    /// Walks the release build's path at the exit, where no change needs a fence, from a test (a
    /// debug build). Compiled only into tests: the app has no way to do this.
    #[cfg(test)]
    pub fn walk_release_path(&self) {
        self.exit.release_path.store(true, Ordering::SeqCst);
    }

    /// This build's config folder for the exit's own files.
    pub fn exit_config_dir(&self) -> PathBuf {
        config_dir(self, &self.exit.rules().unwrap_or_else(Enforce::app))
    }

    fn exit_rules(&self) -> Enforce {
        self.exit.rules().unwrap_or_else(Enforce::app)
    }

    /// One round of lease renewals on the connected nodes (`Exit::renewals`). Look-only can come
    /// on between nodes, or while a claim is out: it never writes the lease, so what's left goes
    /// unasked, and the claim that was out is let go of here (the node's 45 s TTL frees it).
    pub async fn renew_leases(&self, view: &FenceView, inventory: &[String], working: &[String]) {
        let who = self.exit_rules().who();
        for n in self.exit.renewals(view, inventory, working).iter().filter(|n| self.is_connected(n)) {
            if self.exit.policy().look_only() {
                break;
            }
            let _ = self.exit.leases.claim(self, n, &who).await;
            if self.exit.policy().look_only() {
                self.exit.leases.drop_local(n);
            }
        }
    }

    /// A change sent from this Mac rather than over `node`'s SSH (see `Exit::admit_from_mac`).
    pub fn admit_from_mac(&self, node: &str, class: FenceClass, what: &str) -> Result<Admitted, SbError> {
        self.exit.admit_from_mac(self, node, class, what)
    }

    /// Forgets `node`'s identity pin (its host key was replaced); it's pinned at its next connection.
    pub fn forget_identity(&self, node: &str) -> Result<(), String> {
        self.exit.pins.clear(&self.exit_config_dir(), node)
    }

    /// Every command the exit was asked about so far, in order (tests).
    #[cfg(test)]
    pub fn admits(&self) -> Vec<Admit> {
        self.exit.trace.lock().unwrap().clone()
    }
}

/// Hands the app's gate to the exit and starts its background work: the fence ticker (an
/// event when it opens, closes or runs out), the identity pins of newly connected nodes, and
/// the lease refresher. Lets go of the leases on every relock and on quit. Called once at start.
pub fn start(app: &AppHandle) {
    let state = app.state::<AppState>();
    let live = state.live.clone();
    let gate = app.try_state::<Arc<AdminGate>>().map(|g| g.inner().clone());
    if let Some(g) = &gate {
        hand_over(g, &live);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        let mut last: Option<(State, Option<String>)> = None;
        let mut refreshed = Instant::now() - lease::REFRESH;
        loop {
            tick.tick().await;
            let state = app.state::<AppState>();
            let live = state.live.clone();
            if !crate::is_live(&state) {
                continue;
            }
            // The fence opened, closed, failed or ran out: tell the UI.
            let view = live.exit.fence_view(&live);
            let now = (view.state, view.record.as_ref().map(|r| r.id.clone()));
            if last.as_ref() != Some(&now) {
                if last.as_ref().is_some_and(|(s, _)| *s == State::Valid) && now.0 != State::Valid {
                    let send = !live.exit.policy().look_only();
                    release_all(&live, send).await;
                }
                let _ = app.emit(fence::EVENT, &view);
                last = Some(now);
            }
            // Newly connected nodes get their identity pinned.
            live.exit.pins.pin_new(&live, &live.exit_config_dir(), now_ms()).await;
            // A held lease is renewed while Admin is unlocked or this build's fence is open, and
            // in the release build while work runs on its node (`Exit::renewals`).
            if refreshed.elapsed() >= lease::REFRESH {
                refreshed = Instant::now();
                let inventory: Vec<String> = state.inventory.lock().unwrap().nodes.iter().map(|n| n.id.clone()).collect();
                let working = work_under_way(&app, &state);
                live.renew_leases(&view, &inventory, &working).await;
            }
        }
    });
}

/// The nodes work runs on now: a running task's, and a copy's file server's, which no task follows.
fn work_under_way(app: &AppHandle, state: &AppState) -> Vec<String> {
    let mut nodes = app.try_state::<Arc<crate::tasks::Tasks>>().map(|t| t.working_nodes()).unwrap_or_default();
    nodes.extend(state.models.serving_nodes());
    nodes
}

/// The gate becomes the exit's write policy, and its hooks let go of the leases: every relock
/// (on the node too only for a quit while not look-only), and look-only turning on, here only,
/// whether or not Admin was unlocked (a locked gate runs no relock hook then).
pub(crate) fn hand_over(gate: &Arc<AdminGate>, live: &Arc<Live>) {
    live.set_write_policy(gate.clone());
    let releasing = live.clone();
    gate.on_relock(move |why, _| {
        let live = releasing.clone();
        let release = async move { release_on_relock(&live, why).await };
        if why == admin::RelockReason::Quit {
            // The app is about to exit: give the release a moment on its own thread.
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                tauri::async_runtime::block_on(release);
                let _ = tx.send(());
            });
            let _ = rx.recv_timeout(Duration::from_secs(3));
        } else {
            tauri::async_runtime::spawn(release);
        }
    });
    let held = live.clone();
    gate.on_hold(move |_| {
        for n in held.exit.leases.held() {
            held.exit.leases.drop_local(&n);
        }
        admin::HeldTasks::default()
    });
}

/// Gives up every lease this build holds: on the node too (`lease release`) when `send`, else
/// only here (`Leases::drop_local`), and the node's own 45 s TTL frees it. Only a clean fence
/// close or a quit, while not look-only, sends; look-only turning on and a relock write nothing.
pub async fn release_all(live: &Live, send: bool) {
    let instance = live.exit_rules().instance;
    for n in live.exit.leases.held() {
        if send {
            live.exit.leases.release(live, &n, &instance).await;
        } else {
            live.exit.leases.drop_local(&n);
        }
    }
}

/// Whether a relock gives the leases back on the nodes: only a quit while not look-only. An
/// idle, explicit, screen-lock or sleep relock, and look-only turning on (an explicit relock),
/// let go of them here only.
fn relock_sends(why: admin::RelockReason, look_only: bool) -> bool {
    why == admin::RelockReason::Quit && !look_only
}

/// What a relock does with the leases (the hook `start` installs).
pub async fn release_on_relock(live: &Live, why: admin::RelockReason) {
    let send = relock_sends(why, live.exit.policy().look_only());
    release_all(live, send).await
}

// ---- Commands

fn emit_fence(app: &AppHandle, live: &Live) -> FenceView {
    let view = live.exit.fence_view(live);
    let _ = app.emit(fence::EVENT, &view);
    view
}

/// Tauri command: the fence's state for this build, the keys, and whether it can open one.
#[tauri::command]
pub fn fence_state(app: AppHandle) -> FenceView {
    let live = app.state::<AppState>().live.clone();
    live.exit.fence_view(&live)
}

/// What the Touch ID sheet says for opening `spec`, after macOS's "SparkBox is trying to":
/// "open a fence: cell2 · 3 classes (container-test, process-test, files-test) · 8 h · for the
/// dev build". Built here from the spec, never from words the UI sends.
pub fn open_reason(spec: &fence::Spec) -> String {
    let n = spec.classes.len();
    let ids: Vec<&str> = spec.classes.iter().map(|c| c.id()).collect();
    format!(
        "open a fence: {} · {n} class{} ({}) · {} · for the {} build",
        spec.nodes.join(", "),
        if n == 1 { "" } else { "es" },
        ids.join(", "),
        fence::left(spec.minutes.saturating_mul(60_000)),
        spec.instance
    )
}

/// A fence is for the test build: the release build's own changes need none (`BUILD_NEEDS_FENCE`).
fn for_the_test_build(spec: &fence::Spec) -> Result<(), SbError> {
    if spec.instance != "dev" {
        return Err(SbError::invalid("A fence is for the test build: the release build's changes need Admin mode, not a fence."));
    }
    Ok(())
}

/// Whether this build may open `spec` now (`fence_open` and the Touch ID asked for it check the
/// same): Live data, the release build, for the test build, real nodes, 8 hours at most.
fn openable(state: &AppState, spec: &fence::Spec) -> Result<(), SbError> {
    if !crate::is_live(state) {
        return Err(SbError::new(ErrorKind::DemoOnly, "Available with Live data."));
    }
    if crate::identity::instance_name() != "release" {
        return Err(SbError::invalid("Open fences in the release SparkBox."));
    }
    for_the_test_build(spec)?;
    let ids: Vec<String> = state.inventory.lock().unwrap().nodes.iter().map(|n| n.id.clone()).collect();
    fence::validate(spec, &ids).map_err(SbError::invalid)
}

/// Tauri command: the fresh Touch ID `fence_open` checks for, with the sheet naming the fence
/// (`open_reason`: its nodes, classes and hours). Asked only for a fence this build could open.
#[tauri::command]
pub async fn fence_confirm(app: AppHandle, spec: fence::Spec) -> Result<admin::AdminState, SbError> {
    openable(&app.state::<AppState>(), &spec)?;
    admin::confirm_with_reason(&app, &fence_confirm_key(&spec), open_reason(&spec)).await
}

/// The confirmation key `fence_confirm` stores and `fence_open` checks: tied to the exact spec by
/// a digest of its canonical JSON (nodes, classes, only, minutes, instance, note), so a
/// confirmation of one fence can't open another, and the generic `admin_authenticate("fence_open")`
/// (key "fence_open") can't satisfy it either (P1 r2).
fn fence_confirm_key(spec: &fence::Spec) -> String {
    let json = serde_json::to_vec(spec).unwrap_or_default();
    format!("fence_open:{}", sha256_hex(&json))
}

/// Tauri command: opens a fence (release build only), after a fresh Touch ID for it
/// (`fence_confirm`). Signs it with the key in the release build's own Keychain item, made and
/// trusted on first use.
#[tauri::command]
pub async fn fence_open(app: AppHandle, spec: fence::Spec) -> Result<FenceView, SbError> {
    admin::require_mac_confirmed(&app, &fence_confirm_key(&spec)).map_err(SbError::from)?;
    let state = app.state::<AppState>();
    openable(&state, &spec)?;
    let live = state.live.clone();
    // The Keychain blocks while macOS asks: off the async workers.
    let signer = live.clone();
    let record = tauri::async_runtime::spawn_blocking(move || open_signed(&signer, &spec)).await.map_err(|e| SbError::failed(format!("Couldn't open the fence: {e}")))??;
    live.audit.record_app("Fence opened", &format!("{} · {} · {}", record.summary(now_ms()), record.classes.iter().map(|c| c.id()).collect::<Vec<_>>().join(", "), record.instance));
    Ok(emit_fence(&app, &live))
}

/// Opening a fence with the signing key kept by `get` and `set` (the Keychain item in the
/// release build, memory in tests): the key read, or made and kept on first use; this build's
/// trust pinned to it; the record signed and written.
#[cfg(any(test, not(debug_assertions)))]
fn open_with(rules: &Enforce, config: &Path, spec: &fence::Spec, now: u64, mac: &str, get: impl Fn() -> Result<Option<String>, String>, set: impl Fn(&str) -> Result<(), String>) -> Result<Record, SbError> {
    let key = match get().map_err(SbError::failed)? {
        Some(text) => fence::key_from_text(&text).map_err(SbError::failed)?,
        None => {
            let key = fence::new_key().map_err(SbError::failed)?;
            set(&fence::key_to_text(&key)).map_err(SbError::failed)?;
            key
        }
    };
    let (key_id, public_key) = fence::public_of(&key).map_err(SbError::failed)?;
    let file = fence::KeyFile { key_id, public_key, kind: KEY_KIND.into() };
    // The release build trusts exactly the key it signs with.
    if fence::load_trust(config).is_none_or(|t| t.key_id != file.key_id) {
        fence::save_trust(config, &file).map_err(SbError::failed)?;
    }
    let record = fence::record_for(spec, &key, now, mac).map_err(SbError::failed)?;
    fence::write(&rules.fence_dir, config, &file, &record).map_err(SbError::failed)?;
    Ok(record)
}

/// The release build's signing key: its own Keychain item (fence::KEYCHAIN_SERVICE), outside the
/// vault a debug build shares and loads whole until the dev build has its own identity.
#[cfg(not(debug_assertions))]
fn open_signed(live: &Live, spec: &fence::Spec) -> Result<Record, SbError> {
    let rules = live.exit_rules();
    let entry = || keyring::Entry::new(fence::KEYCHAIN_SERVICE, fence::KEYCHAIN_ACCOUNT).map_err(|e| e.to_string());
    let get = || match entry()?.get_password() {
        Ok(text) => Ok(Some(text)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Couldn't read the fence key from the Keychain: {e}")),
    };
    let set = |text: &str| entry()?.set_password(text).map_err(|e| format!("Couldn't keep the fence key in the Keychain: {e}"));
    open_with(&rules, &config_dir(live, &rules), spec, now_ms(), lease::mac_name(), get, set)
}

/// Debug builds never hold the signing key.
#[cfg(debug_assertions)]
fn open_signed(_live: &Live, _spec: &fence::Spec) -> Result<Record, SbError> {
    Err(SbError::invalid("Open fences in the release SparkBox."))
}

/// Tauri command: closes the fence (deletes fence.json; the key stays, and the close is noted so
/// the file put back stays closed) and gives the leases back (on the nodes only when not
/// look-only). Always allowed, look-only included: it only takes rights away.
#[tauri::command]
pub async fn fence_close(app: AppHandle) -> Result<FenceView, SbError> {
    let live = app.state::<AppState>().live.clone();
    close_fence(&live).await?;
    Ok(emit_fence(&app, &live))
}

/// `fence_close`'s work: the fence closed, and the leases given back, on the nodes too unless
/// look-only is on.
async fn close_fence(live: &Live) -> Result<(), SbError> {
    let rules = live.exit_rules();
    let config = config_dir(live, &rules);
    let had = fence::Verifier::default().check(&rules.fence_dir, &config, now_ms()).record;
    fence::close(&rules.fence_dir, &config, now_ms()).map_err(SbError::failed)?;
    if let Some(r) = had {
        live.audit.record_app("Fence closed", &r.id);
    }
    let send = !live.exit.policy().look_only();
    release_all(live, send).await;
    Ok(())
}

/// Tauri command: the debug build trusts the fence key the release build made (a fresh Touch
/// ID, never a scripted unlock). Until the dev build has its own config folder, the release
/// build's pin covers it, and this is refused.
#[tauri::command]
pub fn fence_trust_key(app: AppHandle) -> Result<FenceView, SbError> {
    admin::require_mac_confirmed(&app, "fence_trust_key").map_err(SbError::from)?;
    trust_needs_touch_id(scripted_unlock())?;
    if crate::identity::instance_name() != "dev" {
        return Err(SbError::invalid("The release build trusts its own key when it makes it."));
    }
    shared_config_refusal(crate::identity::SEPARATE_DEV_IDENTITY)?;
    let live = app.state::<AppState>().live.clone();
    let key = trust_key_file(&live.exit_rules(), &live.exit_config_dir())?;
    live.audit.record_app("Trusted the fence key", &key.key_id);
    Ok(emit_fence(&app, &live))
}

/// While the dev build shares the release build's config folder, the release build's own pin is
/// the dev build's too: trusting by hand would only rewrite the release build's trust anchor.
fn shared_config_refusal(separate: bool) -> Result<(), SbError> {
    if !separate {
        return Err(SbError::invalid("This build shares the release build's settings, so it already trusts the key the release build pinned."));
    }
    Ok(())
}

/// Pins the key in fence-key.pub as the one `config`'s build trusts.
fn trust_key_file(rules: &Enforce, config: &Path) -> Result<fence::KeyFile, SbError> {
    let key = fence::load_key_file(&rules.fence_dir).ok_or_else(|| SbError::invalid("There's no fence key yet: open a fence in the release SparkBox first."))?;
    fence::save_trust(config, &key).map_err(SbError::failed)?;
    Ok(key)
}

/// Tauri command: who holds each connected node's controller lease (read from the nodes).
#[tauri::command]
pub async fn lease_state(app: AppHandle) -> Vec<lease::LeaseView> {
    let state = app.state::<AppState>();
    let nodes: Vec<String> = state.inventory.lock().unwrap().nodes.iter().map(|n| n.id.clone()).collect();
    if !crate::is_live(&state) {
        return nodes.into_iter().map(|node| lease::LeaseView { node, ..Default::default() }).collect();
    }
    let live = state.live.clone();
    let mine = live.exit_rules().instance;
    let mut out = Vec::new();
    for node in nodes {
        out.push(match live.probe(&node, lease::SHOW, Duration::from_secs(10)).await {
            Ok(o) => lease::parse_show(&node, &o.stdout, &mine),
            Err(e) => lease::LeaseView { node, error: Some(SbError::classify(&e).message), ..Default::default() },
        });
    }
    out
}

/// Tauri command: each node's identity pin and what it says now.
#[tauri::command]
pub async fn node_identity(app: AppHandle) -> Vec<identity_pin::IdentityView> {
    let state = app.state::<AppState>();
    let nodes: Vec<(String, String)> = state.inventory.lock().unwrap().nodes.iter().map(|n| (n.id.clone(), n.name.clone())).collect();
    let live = state.live.clone();
    live.exit.pins.view(&live, &live.exit_config_dir(), &nodes).await
}

/// Tauri command: accepts the machine answering as `node` now as that node (a fresh Touch ID,
/// never a scripted unlock, and the node's name typed), after it was moved or rebuilt.
#[tauri::command]
pub async fn node_identity_repin(app: AppHandle, node: String, typed_name: String) -> Result<identity_pin::Pin, SbError> {
    admin::require_mac_confirmed(&app, "node_identity_repin").map_err(SbError::from)?;
    let state = app.state::<AppState>();
    let name = state.inventory.lock().unwrap().nodes.iter().find(|n| n.id == node).map(|n| n.name.clone());
    let live = state.live.clone();
    repin(&live, &node, name, &typed_name, scripted_unlock()).await
}

/// `node_identity_repin` once Admin confirmed it: never on a scripted unlock (`scripted`), and
/// only with the node's name (`name`, None: not one of yours) typed.
async fn repin(live: &Live, node: &str, name: Option<String>, typed_name: &str, scripted: bool) -> Result<identity_pin::Pin, SbError> {
    trust_needs_touch_id(scripted)?;
    let name = name.ok_or_else(|| SbError::invalid(format!("{node} isn't one of your nodes.")))?;
    if typed_name.trim() != name {
        return Err(SbError::invalid(format!("Type {name} to re-pin it.")));
    }
    let pin = live.exit.pins.repin(live, &live.exit_config_dir(), node, now_ms()).await.map_err(|e| SbError::failed(e).on(node))?;
    live.audit.record_app(&format!("Re-pinned {name}'s identity"), &format!("{}{}", pin.hostname, pin.lan_mac.as_ref().map(|m| format!(" · {m}")).unwrap_or_default()));
    Ok(pin)
}

#[cfg(test)]
#[path = "exit_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "exit_trace_tests.rs"]
mod trace_tests;

#[cfg(test)]
#[path = "exit_release_tests.rs"]
mod release_tests;
