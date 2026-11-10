//! Admin mode: the padlock in the top bar, enforced here rather than in the UI, so a
//! locked app can't change anything even if a command is called directly.
//!
//! There are two scopes:
//! - **Node** (`admin::require`): anything that changes a node, a switch or a model. It needs
//!   Admin mode unlocked. A Demo unlock needs nothing; a Live unlock always asks for Touch ID or
//!   the Mac password (LocalAuthentication).
//! - **Mac** (`admin::require_mac`): anything that changes state on this Mac — the Keychain,
//!   the inventory, the data source, the VPN, the API server, providers, alert delivery, files
//!   in the Mac pane. It needs Admin mode unlocked *and* the owner confirmed with Touch ID or
//!   the password during this unlock, so a no-password Demo unlock never covers it.
//!
//! Every registered command starts with one of the `require…` calls or is listed in
//! `READ_ONLY` (or `CONDITIONAL`) with its reason; `every_command_is_gated_or_listed` parses
//! lib.rs's handler list and fails the build otherwise.
//!
//! Mac-scope commands are listed in `MAC_SCOPE` and must start with `require_mac`,
//! `require_mac_live`, `require_mac_confirmed` or `require_for_place`.
//!
//! The gate stays open while it's used: each privileged call pushes the deadline back by the
//! chosen idle time (5 minutes through 24 hours). It relocks when that runs out, when the Mac sleeps or its
//! screen locks, when the main window is closed to the menu bar, and when SparkBox quits.
//! The Mac scope has its own clock: it lasts 5 minutes from the confirmation and nothing
//! extends it. Keychain writes confirm every time (`require_mac_confirmed`).
//!
//! Relock hooks (`on_relock`) get the `RelockReason`. SSH tunnels close on every relock. Open
//! shells keep taking keystrokes through an idle relock and when the window closes to the
//! menu bar; they stop when the owner locks, the screen locks, the Mac sleeps or SparkBox
//! quits (and after their own idle time, term/mod.rs).
//!
//! Gate refusals are `SbError`s: commands that return `SbError` get the object, and commands
//! that return `String` get it as JSON (`SbError::to_json`), so every locked call has one shape.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::errors::{ErrorKind, SbError};

pub const LOCKED: &str = "Admin mode is locked. Unlock it in the top bar to make changes.";
pub const NEEDS_AUTH: &str = "Confirm it's you with Touch ID or your Mac password to change settings on this Mac.";
pub const STATE_EVENT: &str = "admin://state";
pub const NEEDS_CONFIRM: &str = "Confirm it's you with Touch ID or your Mac password: this changes a saved password or key.";
/// What every refusal says in look-only mode.
pub const LOOK_ONLY: &str = "SparkBox is in look-only mode: it starts no change on the cluster or this Mac, apart from keeping its helper scripts up to date.";
const IDLE_CHOICES: [u32; 7] = [5, 15, 60, 120, 240, 480, 1440];
const RETENTION_CHOICES: [u32; 3] = [30, 90, 365];
/// Lock idle shells after: 15 min, 60 min, 4 h or never (0).
pub const SHELL_IDLE_CHOICES: [u32; 4] = [15, 60, 240, 0];
/// `admin://state` goes out at most this often while privileged calls keep extending the deadline.
const EMIT_EVERY_MS: u64 = 1000;
/// The Mac scope lasts this long after Touch ID or the password, however it's used.
pub const MAC_SCOPE_MS: u64 = 5 * 60_000;
/// A confirmation asked for one Keychain write is good for this long, and once.
const FRESH_MS: u64 = 60_000;
/// What SparkBox trusts moves only after a real Touch ID or password: SPARKBOX_ADMIN_NOAUTH
/// confirms none of these (each command refuses it on its own too).
pub const NOAUTH_NEVER: &[&str] = &["fence_open", "fence_trust_key", "node_identity_repin", "known_hosts_trust", "drop_cache_grant_set"];

/// Whether `key` is one of NOAUTH_NEVER, also in the form `<key>:<detail>` (fence_confirm's
/// `fence_open:<digest of the spec>`).
fn never_without_auth(key: &str) -> bool {
    NOAUTH_NEVER.contains(&key.split(':').next().unwrap_or(key))
}
/// The warnings before something locks come this long before it does.
pub const WARN_MS: u64 = 60_000;

/// Debug builds only: SPARKBOX_IDLE_SCALE=60 makes every lock timer 60 times shorter (the
/// Admin idle time, the Mac scope, idle shells), so the warnings can be screenshotted. The
/// words still name the real times.
pub fn idle_scale() -> u64 {
    static SCALE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| {
        if !cfg!(debug_assertions) {
            return 1;
        }
        std::env::var("SPARKBOX_IDLE_SCALE").ok().and_then(|v| v.parse::<u64>().ok()).filter(|n| (1..=3600).contains(n)).unwrap_or(1)
    })
}

/// SPARKBOX_LOOK_ONLY=1, in any build: look-only for this run, whatever Settings say. The
/// Settings switch can't turn it off.
pub fn look_only_forced() -> bool {
    static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| std::env::var("SPARKBOX_LOOK_ONLY").is_ok_and(|v| v == "1"))
}

/// The app's gate is in look-only mode (Settings › Security › Look only); mirrored here by
/// `AdminGate::system` so code without the gate at hand can ask `look_only()`.
static LOOK_ONLY_SETTING: AtomicBool = AtomicBool::new(false);

/// Look-only mode: SparkBox starts no change on the cluster or this Mac. Every command outside
/// `READ_ONLY` is refused before it runs (`look_only_refusal`), every gate refuses, and
/// unlocking refuses before any Touch ID prompt. On with SPARKBOX_LOOK_ONLY=1 or the Settings
/// switch.
pub fn look_only() -> bool {
    look_only_forced() || LOOK_ONLY_SETTING.load(Ordering::SeqCst)
}

/// A duration as the timers use it (shortened by `idle_scale` in debug builds).
pub fn scaled(ms: u64) -> u64 {
    (ms / idle_scale()).max(1000)
}

/// Why Admin mode relocked. Tunnels close for every reason; open shells stop taking
/// keystrokes only when the owner or macOS locks (`ends_shells`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RelockReason {
    /// No privileged action for the idle time.
    Idle,
    /// Locked on purpose: the top bar, the menu bar.
    Explicit,
    /// macOS locked the screen.
    ScreenLock,
    /// The Mac went to sleep.
    Sleep,
    /// SparkBox quit.
    Quit,
    /// The main window was closed to the menu bar.
    WindowClosed,
    /// The data source moved to Live: a Demo unlock never asked who you are.
    LiveData,
}

impl RelockReason {
    /// Whether open shells stop taking keystrokes. An idle relock or closing the window to the
    /// menu bar leaves them working: the owner is still at them.
    pub fn ends_shells(self) -> bool {
        !matches!(self, RelockReason::Idle | RelockReason::WindowClosed)
    }

    /// Whether deferred work scheduled before it is cancelled (`AdminGate::hold_generation`): a
    /// queued restart, a Keep running retry. An idle relock or closing the window doesn't.
    pub fn holds(self) -> bool {
        !matches!(self, RelockReason::Idle | RelockReason::WindowClosed)
    }
}

/// Why deferred work must not act now (`HoldView::held_since`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Hold {
    /// Look-only mode is on, or came on since the work was scheduled.
    LookOnly,
    /// Admin mode relocked since: on purpose, the screen locked, the Mac slept, SparkBox quit or
    /// the data moved to Live.
    Locked,
}

impl Hold {
    /// What a held Keep running says.
    pub fn label(self) -> &'static str {
        match self {
            Hold::LookOnly => "Held · look-only",
            Hold::Locked => "Held · locked",
        }
    }
}

/// The gate's hold state at one moment, for deferred work to compare with the generation it
/// was scheduled under. `HoldView::global` is the fallback before a gate is handed over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldView {
    pub look_only: bool,
    pub generation: u64,
    last: Hold,
}

impl HoldView {
    /// The app-wide look-only flag, for code that hasn't been given the gate (yet).
    pub fn global() -> HoldView {
        HoldView { look_only: look_only(), generation: 0, last: Hold::LookOnly }
    }

    /// Why work scheduled at `generation` must not act now: look-only is on, or something
    /// moved the generation since (look-only, or a relock other than an idle one).
    pub fn held_since(&self, generation: u64) -> Option<Hold> {
        if self.look_only {
            Some(Hold::LookOnly)
        } else if self.generation != generation {
            Some(self.last)
        } else {
            None
        }
    }
}

/// What turning look-only on did to the tasks that were running (`Tasks::hold_non_read`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldTasks {
    /// Cancelled before their next step.
    pub stopped: usize,
    /// They can't be stopped part-way: the write exit refuses their next change in look-only
    /// mode (reads after it still run).
    pub stopping: usize,
    /// What they started runs on the node by itself (an upgrade, a load's containers): it keeps
    /// running, and stopping it is a change look-only refuses (`Task::on_node`).
    pub on_node: usize,
}

impl HeldTasks {
    /// "2 tasks stopped before their next step · 1 task's next change is refused · 1 task's work
    /// keeps running on its node", or None for none.
    pub fn words(&self) -> Option<String> {
        let n = |k: usize| if k == 1 { "1 task".to_string() } else { format!("{k} tasks") };
        let its = |k: usize| if k == 1 { "its" } else { "their" };
        let owns = |k: usize| if k == 1 { "1 task's".to_string() } else { format!("{k} tasks'") };
        let mut parts = Vec::new();
        if self.stopped > 0 {
            parts.push(format!("{} stopped before {} next step", n(self.stopped), its(self.stopped)));
        }
        if self.stopping > 0 {
            parts.push(format!("{} next {} refused", owns(self.stopping), if self.stopping == 1 { "change is" } else { "changes are" }));
        }
        if self.on_node > 0 {
            parts.push(format!("{} work keeps running on {} node{}", owns(self.on_node), its(self.on_node), if self.on_node == 1 { "" } else { "s" }));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

impl std::ops::AddAssign for HeldTasks {
    fn add_assign(&mut self, o: HeldTasks) {
        self.stopped += o.stopped;
        self.stopping += o.stopping;
        self.on_node += o.on_node;
    }
}

/// What `look_only_set` answers: the state, and what turning it on did to running tasks.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookOnlyView {
    #[serde(flatten)]
    pub state: AdminState,
    pub held: HeldTasks,
}

/// What every held task's message starts with (`Tasks::hold_non_read`).
pub const LOOK_ONLY_HOLD: &str = "Look-only turned on";

/// Registered commands that change nothing a lock should protect, each with the reason. Every
/// other command must start with `admin::require…` (checked by `every_command_is_gated_or_listed`).
/// Add your package's entries under its marker. Only the gate test reads these lists.
#[cfg_attr(not(test), allow(dead_code))]
pub const READ_ONLY: &[(&str, &str)] = &[
    ("howto_document", "reads bundled NVIDIA documentation; text never defines actions"),
    ("howto_saved", "lists the owner's local How To notes and editions"),
    ("howto_read", "reads one local guide with retained editions"),
    ("howto_save", "saves the owner's notes, checklist and reviewed text locally, like conversation_save; changes no cluster or execution setting"),
    ("howto_last_check", "reads the last explicitly requested NVIDIA source comparison"),
    ("howto_check_source", "reads a fixed public NVIDIA repository at the owner's request; no credentials or node changes"),
    ("howto_fetch_source", "caches hash-verified NVIDIA text for review; never applies it or runs its commands"),
    ("howto_check_nodes", "reads existing quick prerequisite facts from selected trusted inventory nodes; never starts a container"),
    ("discovery_networks", "lists local interface metadata or a Demo fixture; does not probe"),
    ("discovery_status", "reads the in-memory survey and discards stale source results"),
    ("discovery_cancel", "stops an existing survey or verification; starts no work"),
    ("discovery_propose", "builds an unsaved inventory draft and topology from verified survey results"),
    ("hardware_profiles", "reads cached hardware metadata only; automatic AI lookup is a separate explicit Mac preference"),
    ("updates_rolling_preview", "reads pending routine packages and dry runs each selected node; nothing is installed"),
    ("updates_rolling_status", "reads the saved rolling maintenance checkpoint"),
    // ── r2-design ──
    // ── r2-shell ──
    ("menu_set_windows", "updates the app menu's Window list"),
    ("menu_set_state", "updates the app menu's checkmarks and enabled items"),
    ("menu_close_main", "closes the main window, as ⌘W does"),
    ("menu_minimize_main", "minimizes the main window, as ⌘M does"),
    // ── r2-safety ──
    // The gate itself: asking for, extending or giving up Admin mode can't change anything else.
    ("admin_state", "reports whether Admin mode is unlocked"),
    ("admin_unlock", "the unlock itself: Touch ID or the password in Live, nothing in Demo"),
    // The Switch X's panel: the labels and the cloud's last answer, from memory.
    ("firewalla_view", "reports the Switch X's labels and the cloud's last answer"),
    ("firewalla_msp_settings", "reports the saved MSP domain"),
    ("firewalla_msp_test", "asks Firewalla's cloud, GETs only; nothing changes here or on the switch"),
    ("admin_authenticate", "the Touch ID / password prompt that opens the Mac scope"),
    ("fence_confirm", "the Touch ID prompt for opening a fence, worded from its spec; changes nothing (fence_open checks it)"),
    ("admin_extend", "pushes the relock back while unlocked; never unlocks"),
    ("admin_lock", "locks Admin mode; locking only takes rights away"),
    ("security_get", "returns the auto-lock and retention settings"),
    ("autostart_get", "says whether SparkBox opens at login"),
    // Reads of the app's own state.
    ("get_snapshot", "returns the latest cluster snapshot"),
    ("get_inventory", "returns the saved inventory, which holds no secrets"),
    ("credential_status", "says which Keychain entries exist, never their values"),
    ("prefs_get", "returns the preferences"),
    ("persistence_warnings", "lists settings files that couldn't be read"),
    ("tasks_list", "lists the last 200 tasks"),
    ("task_get", "returns one task"),
    ("activity_log", "returns the activity log in memory"),
    ("activity_log_path", "returns where the activity log file is"),
    ("activity_log_since", "returns activity log entries newer than a time"),
    ("activity_export", "creates a new file in Downloads; never overwrites"),
    ("diagnostics_export", "creates a new file in Downloads; never overwrites"),
    ("log_ui_error", "appends a UI error to SparkBox's own log file"),
    ("dev_report", "debug builds only: appends a line to the dev script's output file"),
    ("doctor_run", "runs read-only checks on the nodes and creates a new file in Downloads"),
    // Nodes, read only.
    ("files_backups", "lists the editor's backups of a file"),
    ("files_du", "measures folder sizes (du)"),
    ("files_preflight", "works out what a transfer would do; copies nothing"),
    ("services_list", "lists systemd units (systemctl list-units)"),
    ("service_status", "systemctl status of one unit"),
    ("service_detail", "systemctl status and the journal of one unit"),
    ("service_impact", "works out what a service action would break; runs nothing"),
    ("process_alive", "checks a process is still there (reads /proc)"),
    ("updates_status", "reads pending packages, holds and the restart flag (apt list, read-only)"),
    ("updates_simulate", "apt-get --simulate: shows what an upgrade would do"),
    ("updates_system_status", "local system-update checkpoint"),
    ("ops_classify", "classifies a command's text; runs nothing"),
    ("fleet_results", "returns a finished Fleet run's output"),
    ("container_inspect", "docker inspect, with secret environment values masked"),
    // Terminals: opening a shell is gated (CONDITIONAL); these only drive an open one.
    ("terminal_input", "keystrokes go only to shells terminal_open authorized under node scope; a relock revokes them"),
    ("terminal_resize", "resizes an open terminal's window"),
    ("terminal_close", "closes an open terminal; closing only ends access"),
    ("terminal_ack", "flow control: xterm has drawn more of the output"),
    ("terminal_save_text", "creates a new file in ~/Downloads (never replaces one), the one folder that needs no Mac scope"),
    ("tunnel_close", "closes an SSH tunnel; closing only ends access"),
    // The VPN, Demo and the switch, read only.
    ("vpn_get", "returns the VPN's settings and state, never its keys"),
    ("vpn_reconnect", "restores the configured tunnel; changes no settings"),
    ("demo_scenarios_list", "lists the Demo scenarios"),
    ("demo_scenario_set", "changes only the simulator"),
    ("switch_test", "tests the switch's REST login; changes nothing"),
    ("switch_probe_history", "returns the switch's recent probe results"),
    // ── r2-fabric ──
    ("fabric_state", "returns the fabric monitor's last assessment"),
    ("fabric_doctor_report", "returns the Fabric doctor's last report"),
    ("fabric_baselines", "returns the saved per-pair bandwidth baselines"),
    ("switch_tables", "returns the switch's last bridge host and neighbor tables"),
    // ── r2-monitor ──
    ("health_get", "reads the health engine's current alerts"),
    ("alerts_history", "reads the alert log on this Mac"),
    ("history_query", "reads metric history from this Mac's store"),
    ("notify_settings_get", "reads the notification settings"),
    ("notify_take_pending", "reports which window the latest notification pointed at"),
    ("notify_channels_get", "lists the alert channels (their secrets stay in the Keychain)"),
    ("notify_deliveries", "reads the delivery log"),
    ("rules_get", "reads the alert rules in force"),
    ("rules_defaults", "returns the shipped alert rules without saving anything"),
    ("rules_preview", "judges a candidate rule set against the current data without saving it"),
    ("kernel_events", "reads the kernel events in the last snapshot"),
    ("agent_status", "reads each node's agent state and heartbeat"),
    ("agent_settings_get", "reads the agents' port and metrics settings"),
    ("agent_backfill_now", "copies what the agents recorded into this Mac's history; changes nothing on a node"),
    // ── r2-live ──
    ("node_wol_status", "reads a node's LAN MAC, its Wake-on setting and the routes a wake could take"),
    ("files_leftovers", "lists staging folders transfers couldn't remove yet"),
    ("updates_progress", "reports the phase, package count and log lines of running upgrades"),
    ("login_settings_get", "reads Settings › Cluster login"),
    ("terminal_broadcast", "notes in the activity log that broadcast typing started or stopped; sends nothing to a node"),
    ("login_methods", "reports how each node logged in and which keys SparkBox would offer, without connecting"),
    // ── r2-ai ──
    ("hf_search", "searches Hugging Face from this Mac"),
    ("hf_model", "reads a model's files, config and fit from Hugging Face"),
    ("hf_refs", "reads branch and tag names and commits from Hugging Face; no node or repository writes"),
    ("models_analyze", "asks the advisor model about a Hugging Face model; changes nothing"),
    ("library_list", "lists the model folders on the nodes"),
    ("library_report", "reads model folders with per-node scan results and in-memory last-known observations; starts no jobs"),
    ("model_download_tools", "reads executable fingerprints, managed-tool receipts and local image IDs; executes no hf command and installs nothing"),
    ("model_chain_review", "reads the inspected manifest, empty destinations, rsync availability, disk capacity and fabric addresses; starts no jobs or listener"),
    ("model_download_capacity", "compares reviewed file sizes with destination filesystem capacity on chosen inventory nodes; creates no model folder, installs no tool and starts no job"),
    ("model_disk_preflight", "reads file sizes and destination filesystem capacity; starts no job, creates no model folder and installs no download tool"),
    ("jobs_list", "lists download and copy jobs and their progress"),
    ("deploy_preview", "shows the commands a load would run, without running them"),
    ("deploy_plan", "computes a load's memory plan and port check"),
    ("model_preflight", "reads each chosen node's memory, earlyoom, image, Docker store, model folder, ports, /dev/shm, caches, containers, Ollama and fabric facts before a load; no ping"),
    ("recipes_list", "lists the load recipes and what each has been verified on"),
    ("deployment_history_list", "reads inert local recovery copies, or Demo fixtures; never restores loads"),
    ("deployment_history_file", "parses bytes from a user-selected local report in memory; no paths, saves or node actions"),
    ("deployments_list", "lists SparkBox's loads and their state"),
    ("benchmarks_list", "lists saved benchmark results"),
    ("bench_export", "saves a benchmark result as a new file in Downloads"),
    ("endpoints_list", "lists the inference endpoints from the last round"),
    ("endpoints_refresh", "probes the inference endpoints now"),
    ("ollama_list", "lists an Ollama server's models"),
    ("providers_list", "lists AI providers (their keys stay in the Keychain)"),
    ("providers_refresh", "asks a provider for its model list and remembers it"),
    ("chat_send", "sends a chat to the model the user picked; changes no node, secret or setting"),
    ("chat_stop", "stops a reply that is streaming"),
    ("conversations_list", "lists the chat history on this Mac"),
    ("conversation_get", "reads one conversation from the chat history"),
    ("conversation_save", "saves the user's own conversation in the chat history on this Mac"),
    ("conversation_delete", "deletes one of the user's conversations from the chat history on this Mac"),
    ("server_get", "reads the local API server's settings and state"),
    ("server_self_test", "sends test requests through the local API server"),
    ("server_logs", "reads the local API server's request log"),
    // ── r3-paging ──
    // ── r3-agent ──
    // ── r3-incidents ──
    // ── r3-serving ──
    ("hf_size", "reads a Hugging Face repo's file sizes from this Mac"),
    // ── r3-charts ──
    // ── r3-desk ──
    // ── r3-safety ──
    ("tunnels_list", "lists the open SSH tunnels to node pages, for Settings › Security & privacy"),
    // ── r4-nodes ──
    ("services_poll", "reads every service's state with systemctl show and list-unit-files, no root"),
    // ── r4-serving ──
    ("library_detail", "reads a model folder's config.json and size (cat, du) on a node that has it"),
    ("cluster_deployments_list", "groups the snapshot's containers into deployments; a failed one's postmortem reads docker inspect, docker logs --tail 600 and its launch script"),
    // ── r4-alerts ──
    // ── r4-agent ──
    // ── r4-safety ──
    ("look_only_set", "the gate itself: on only takes rights away; off asks for Touch ID and never undoes SPARKBOX_LOOK_ONLY"),
    ("images_list", "docker image ls and docker system df on each node (both read only), or the simulated containers' in Demo"),
    // ── r4-demo ──
    // ── r4-craft ──
    // ── p1r1-w ──
    ("evidence_capture", "reads the nodes; writes new files only under ~/Library/Logs/SparkBox/evidence, like the app's own logs"),
    ("evidence_list", "lists the evidence folders under ~/Library/Logs/SparkBox/evidence"),
    // ── p1r1-x ──
    ("fence_state", "reads the fence file, its key file and the key this build trusts"),
    ("lease_state", "reads each node's ~/.sparkbox/controller.json and the node's clock (cat, date)"),
    ("node_identity", "reads the identity pins and what each connected node says it is"),
    ("fence_close", "only takes rights away: deletes the open fence and gives the leases back; allowed in look-only"),
    // ── p1r1-h ──
    ("reach_state", "reads the cluster-path monitor's verdict and the Mac-side probes behind it (default route, gateway ping or TCP answer, VPN state); changes nothing"),
    // ── p1r1-c ──
    ("boots_get", "reads the boots SparkBox knows of"),
    ("boots_refresh", "reads the node's journal (journalctl, as the cluster user)"),
    // ── p1r3 ──
    ("updates_summary", "reads the cached update status of each node: apt, DGX OS, firmware, Ollama, snaps"),
    ("drop_cache_grant_get", "reads the current instance's saved permission; never creates or uses it"),
    ("earlyoom_preview", "reads /etc/default/earlyoom and prepares an exact-name edit without evaluating it"),
    ("model_recipe_import", "reads config.env and inner.sh as text, omits secret options, validates a draft recipe; never runs the scripts"),
    ("node_prereboot_check", "reads which kernel boots next and whether it has an initrd and a matching NVIDIA module"),
    ("processes_get", "lists a node's processes from its latest report"),
];

/// Commands whose gate applies to some calls only (`if … { admin::require…(&app)?; }` as the
/// first statement), each with the reason the other calls are safe.
#[cfg_attr(not(test), allow(dead_code))]
pub const CONDITIONAL: &[(&str, &str)] = &[
    // ── r2-design ──
    // ── r2-shell ──
    // ── r2-safety ──
    ("prefs_patch", "the menu bar and screenshot-safe switches are UI-only; a patch touching the data source or model settings needs Mac scope"),
    ("task_cancel", "tasks marked read_only (Fleet runs of read-only commands, checks, benchmarks) stop without Admin mode; every other cancel needs node scope, like starting it did"),
    ("terminal_open", "log followers only read (docker logs -f, journalctl -f); a shell needs node scope"),
    // ── r2-fabric ──
    // ── r2-monitor ──
    // ── r2-live ──
    ("files_transfer", "a copy from a node into ~/Downloads under new names arrives quarantined without execute bits, like a browser download; every other placement on this Mac needs Mac scope, on a node node scope"),
    ("files_list", "listing a folder only reads; ~/.ssh, the Keychains, ~/.gnupg, ~/.aws, ~/.config/gh and ~/.kube on this Mac need Mac scope"),
    ("files_read", "reading a file only reads; one in ~/.ssh, the Keychains, ~/.gnupg, ~/.aws, ~/.config/gh or ~/.kube on this Mac needs Mac scope"),
    // ── r2-ai ──
    // ── r3-paging ──
    ("notify_channel_test", "a test from this Mac sends one message to a channel the owner set up, at most once per 10 s per channel; from a node's agent (via) it runs a command there, so it needs node scope"),
    // ── r3-agent ──
    // ── r3-incidents ──
    // ── r3-serving ──
    // ── r3-charts ──
    // ── r3-desk ──
    // ── r3-safety ──
    // ── r4-nodes ──
    // ── r4-serving ──
    // ── r4-alerts ──
    ("alert_silence", "one alert (its rule and entity) or one incident for an hour or a day is node scope, like an acknowledgement; a rule or an entity everywhere, until it clears (at most 7 days), or a week or longer needs Mac scope with Live data (require_mac_live) and a reason, which the command refuses without"),
    // ── r4-agent ──
    ("agent_install", "node scope always; Mac scope as well when it copies one of the owner's secrets to the nodes (a channel secret, an endpoint API key, the metrics token), like saving that secret did"),
    ("agent_push_config", "node scope always; Mac scope as well when the push copies one of the owner's secrets to the nodes (a channel secret, an endpoint API key, the metrics token)"),
    // ── r4-safety ──
    // ── r4-demo ──
    // ── r4-craft ──
];

/// Commands that change state on this Mac (the Keychain, the inventory, alert channels and
/// settings, rules, silences, the data source, the VPN, the API server, preferences, login
/// items). Each must start with `require_mac`, `require_for_place` or `require_mac_live`, so a
/// node-scope unlock never covers it (checked by `mac_scope_commands_start_with_a_mac_gate`).
/// One line per entry with what it changes; a command listed here is never in `READ_ONLY`.
#[cfg_attr(not(test), allow(dead_code))]
pub const MAC_SCOPE: &[(&str, &str)] = &[
    ("discovery_start", "starts an explicit bounded local LAN survey without credentials"),
    ("discovery_verify", "logs in to one selected survey candidate after pinned host-key trust"),
    // ── r3-paging ──
    ("notify_channels_set", "where alerts are sent, and the channels' secrets in the Keychain"),
    ("notify_settings_set", "when alerts are sent: categories, critical only, quiet hours and their zone, repeats"),
    ("agent_settings_set", "the agents' Prometheus port and its bearer token in the Keychain"),
    // ── r3-agent ──
    // ── r3-incidents ──
    ("alert_silence", "silences an alert, an incident or a rule (alert-silences.*.json, pushed to the agents); wide, until-it-clears and week-long silences need Mac scope"),
    // ── r3-serving ──
    // ── r3-charts ──
    // ── r3-desk ──
    // ── r3-safety ──
    // ── r4-nodes ──
    // ── r4-serving ──
    // ── r4-alerts ──
    // ── r4-agent ──
    // ── r4-safety ──
    // ── r4-demo ──
    // ── r4-craft ──
    // The Keychain (each write confirms on its own: require_mac_confirmed).
    ("set_credential", "saves the cluster password, a switch password, the HF token or the Firewalla MSP token in the Keychain"),
    ("clear_credential", "removes a password or token from the Keychain"),
    ("providers_set_key", "saves a provider's API key in the Keychain"),
    ("providers_clear_key", "removes a provider's API key from the Keychain"),
    ("login_passphrase_set", "saves or removes an SSH key's passphrase in the Keychain"),
    // The inventory, cluster login and host keys.
    ("save_inventory", "changes which nodes and switches SparkBox talks to"),
    ("firewalla_label_set", "says what's plugged into a Switch X port (firewalla_ports.json); the switch is never told"),
    ("firewalla_msp_set_domain", "changes where the saved Firewalla MSP token is sent"),
    ("login_settings_set", "changes how SparkBox logs in to the nodes (user, key, agent, jump host)"),
    ("known_hosts_trust", "pins a node's SSH host key"),
    ("known_hosts_forget", "forgets every pinned SSH host key"),
    ("node_probe", "tests a node's connection settings and can pin its host key"),
    ("switch_trust", "pins a switch's certificate"),
    // The data source, preferences, security settings and login items.
    ("data_source_set", "switches between Demo and Live data"),
    ("prefs_patch", "the data source and model settings, field by field (the menu bar switches don't need it)"),
    ("security_set", "changes auto-lock, idle-shell lock and activity log retention"),
    ("autostart_set", "adds or removes SparkBox's login item"),
    // The VPN.
    ("vpn_import", "imports a VPN profile and its private key"),
    ("vpn_set_endpoint", "changes the VPN endpoint"),
    ("vpn_set_mode", "changes when the VPN connects"),
    ("vpn_set_forwards", "changes the VPN's port forwards"),
    ("vpn_remove", "removes the VPN profile"),
    ("vpn_override", "turns the VPN on or off"),
    // AI providers and the local API server.
    ("providers_upsert", "adds or changes a model provider (a new address drops its saved key)"),
    ("providers_remove", "removes a model provider"),
    ("server_save", "changes the local API server's settings"),
    ("server_start", "starts the local API server on this Mac"),
    ("server_stop", "stops the local API server"),
    ("server_create_key", "creates an API server key"),
    ("server_revoke_key", "revokes an API server key"),
    // Files in the Mac pane (node scope for a node's files).
    ("files_write", "saves a file on this Mac (node scope when it's on a node)"),
    ("files_mkdir", "creates a folder on this Mac (node scope on a node)"),
    ("files_new_file", "creates a file on this Mac (node scope on a node)"),
    ("files_rename", "renames a file on this Mac (node scope on a node)"),
    ("files_delete", "deletes files on this Mac (node scope on a node)"),
    ("files_chmod", "changes a file's permissions on this Mac (node scope on a node)"),
    ("files_trash", "moves files to this Mac's Trash (node scope on a node)"),
    ("files_untrash", "puts files back from this Mac's Trash (node scope on a node)"),
    ("files_transfer", "places copied files on this Mac (node scope when the copy lands on a node)"),
    ("files_leftover_remove", "removes a staging folder a copy left on this Mac (node scope on a node)"),
    // r2 commands of other packages that already check the Mac scope.
    ("rules_set", "changes the alert rules saved on this Mac"),
    ("fabric_cabling_accept", "saves the accepted fabric cabling on this Mac"),
    ("fabric_baselines_reset", "clears the fabric bandwidth baselines saved on this Mac"),
    // ── p1r1-x ── (each starts with require_mac_confirmed: a fresh Touch ID every time)
    ("fence_open", "signs and writes the fence (SparkBox Fence/), with the signing key from the Keychain vault"),
    ("drop_cache_grant_set", "changes a narrowly scoped standing permission in the Keychain; real fresh authentication every time"),
    ("fence_trust_key", "pins the fence key this build trusts (fence-trust.json)"),
    ("node_identity_repin", "accepts a different machine as a node in node_pins.json"),
];

/// `READ_ONLY` commands that still don't run in look-only mode, each with why: they send
/// something (a chat, a request to a model, a login to the switch), run checks on the nodes,
/// or create a file on this Mac. `look_only_refusal` refuses them with the rest.
pub const LOOK_ONLY_HOLDS: &[(&str, &str)] = &[
    ("doctor_run", "runs checks on every node and writes a report into Downloads"),
    ("switch_test", "logs in to the switch"),
    ("firewalla_msp_test", "sends the saved token to Firewalla's cloud"),
    ("vpn_reconnect", "brings the VPN tunnel up again"),
    ("chat_send", "sends a chat to a model, which may be on the cluster"),
    ("server_self_test", "sends test requests through the API server to a model"),
    ("models_analyze", "asks the advisor model, which may be on the cluster"),
    ("providers_refresh", "asks a provider's server for its model list"),
    ("conversation_save", "writes the chat history on this Mac"),
    ("conversation_delete", "deletes from the chat history on this Mac"),
    ("activity_export", "creates a file in Downloads"),
    ("diagnostics_export", "creates a file in Downloads"),
    ("terminal_save_text", "creates a file in Downloads"),
    ("bench_export", "creates a file in Downloads"),
];

/// `CONDITIONAL` commands whose ungated calls are reads and still run in look-only mode; their
/// gated calls are refused by the gate itself. Every other `CONDITIONAL` command is refused.
pub const LOOK_ONLY_CONDITIONAL: &[(&str, &str)] = &[
    ("files_list", "lists a folder; a key folder on this Mac asks for the Mac scope, which look-only refuses"),
    ("files_read", "reads a file; a key folder on this Mac asks for the Mac scope, which look-only refuses"),
    ("terminal_open", "log followers only read; a shell asks for node scope, which look-only refuses"),
    ("task_cancel", "stops a read-only task; any other cancel asks for node scope, which look-only refuses"),
];

/// In look-only mode, the refusal for `command` before it runs: everything but `READ_ONLY`
/// (less `LOOK_ONLY_HOLDS`) and `LOOK_ONLY_CONDITIONAL`. Called by lib.rs's invoke handler for
/// every command, so a command that forgot its gate is still refused.
pub fn look_only_refusal(command: &str) -> Option<SbError> {
    look_only().then(|| refusal_in_look_only(command)).flatten()
}

fn refusal_in_look_only(command: &str) -> Option<SbError> {
    let listed = |list: &[(&str, &str)]| list.iter().any(|(n, _)| *n == command);
    let passes = (listed(READ_ONLY) && !listed(LOOK_ONLY_HOLDS)) || listed(LOOK_ONLY_CONDITIONAL);
    (!passes).then(|| SbError::new(ErrorKind::LookOnly, LOOK_ONLY))
}

/// Wraps lib.rs's `generate_handler![…]`: look-only refusals first, then the command.
pub fn look_only_guard<R: tauri::Runtime>(
    handler: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        if let Some(refusal) = look_only_refusal(invoke.message.command()) {
            invoke.resolver.reject(refusal);
            return true;
        }
        handler(invoke)
    }
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
}

/// Proves the person at the Mac is its owner. Blocks until they answer.
pub trait Authenticator: Send + Sync {
    fn authenticate(&self, reason: &str) -> Result<(), String>;
}

/// Touch ID, or the Mac's login password when there's no Touch ID (deviceOwnerAuthentication).
pub struct MacAuthenticator;

impl Authenticator for MacAuthenticator {
    fn authenticate(&self, reason: &str) -> Result<(), String> {
        use block2::RcBlock;
        use objc2::runtime::Bool;
        use objc2_foundation::{NSError, NSString};
        use objc2_local_authentication::{LAContext, LAError, LAPolicy};

        let policy = LAPolicy::DeviceOwnerAuthentication;
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let tx = Mutex::new(Some(tx));
        // SAFETY: plain LocalAuthentication calls; the context lives until the reply arrives.
        let context = unsafe { LAContext::new() };
        if let Err(e) = unsafe { context.canEvaluatePolicy_error(policy) } {
            return Err(format!("This Mac can't confirm it's you: {}", e.localizedDescription()));
        }
        let reply = RcBlock::new(move |ok: Bool, error: *mut NSError| {
            let result = if ok.as_bool() {
                Ok(())
            } else {
                // SAFETY: the framework passes either null or a valid NSError.
                match unsafe { error.as_ref() } {
                    Some(e) if [LAError::UserCancel.0, LAError::SystemCancel.0, LAError::AppCancel.0].contains(&e.code()) => {
                        Err("Cancelled.".to_string())
                    }
                    Some(e) if e.code() == LAError::AuthenticationFailed.0 => Err("That wasn't the right password or fingerprint.".to_string()),
                    Some(e) => Err(e.localizedDescription().to_string()),
                    None => Err("macOS didn't confirm it's you.".to_string()),
                }
            };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(result);
            }
        });
        let reason = NSString::from_str(reason);
        unsafe { context.evaluatePolicy_localizedReason_reply(policy, &reason, &reply) };
        let result = rx.recv_timeout(Duration::from_secs(180)).unwrap_or_else(|_| Err("No answer from macOS; try again.".into()));
        drop(context);
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Changes a node, a switch or a model.
    Node,
    /// Changes state on this Mac (Keychain, inventory, VPN, server, providers, Mac files).
    Mac,
}

/// What macOS shows after "SparkBox is trying to …", by command (or scope). The UI names the
/// command; it never supplies the words, so it can't make the prompt say something else.
const PROMPTS: &[(&str, &str)] = &[
    ("node", "unlock Admin mode"),
    ("mac", "change settings on this Mac"),
    ("node_power", "restart, shut down or wake a node"),
    ("fleet_start", "run a command on your nodes"),
    ("terminal_open", "open a shell on a node"),
    ("terminal_authorize", "keep typing in a locked shell"),
    ("login_passphrase_set", "save an SSH key's passphrase in the Keychain"),
    ("save_inventory", "change the cluster's nodes and switches"),
    ("data_source_set", "switch between Demo and Live data"),
    ("prefs_set", "change the data source or model settings"),
    ("prefs_patch", "change the data source or model settings"),
    ("security_set", "change the security settings"),
    ("drop_cache_grant_set", "grant or revoke conditional page-cache drops for Keep running on the current inventory nodes"),
    ("autostart_set", "change whether SparkBox opens at login"),
    ("vpn_set_endpoint", "change the VPN endpoint"),
    ("vpn_set_mode", "change when the VPN connects"),
    ("vpn_set_forwards", "change the VPN's port forwards"),
    ("vpn_import", "import a VPN profile"),
    ("vpn_remove", "remove the VPN profile"),
    ("vpn_override", "turn the VPN on or off"),
    ("providers_upsert", "change a model provider"),
    ("providers_remove", "remove a model provider"),
    ("providers_set_key", "save a provider's API key"),
    ("providers_clear_key", "remove a provider's API key"),
    ("server_save", "change the API server's settings"),
    ("server_start", "start the API server"),
    ("server_stop", "stop the API server"),
    ("server_create_key", "create an API server key"),
    ("server_revoke_key", "revoke an API server key"),
    ("firewalla_label_set", "change what SparkBox says is plugged into a Switch X port"),
    ("firewalla_msp_set_domain", "change where the Firewalla MSP token is sent"),
    ("set_credential", "save a password in the Keychain"),
    ("clear_credential", "remove a password from the Keychain"),
    ("known_hosts_trust", "trust a node's SSH host key"),
    ("known_hosts_forget", "forget the saved SSH host keys"),
    ("switch_trust", "trust a switch's certificate"),
    ("node_probe", "test a node's connection settings"),
    ("discovery_start", "survey the selected local /24 for SSH and web services"),
    ("discovery_verify", "identify a selected device through trusted SSH without changing it"),
    ("notify_channels_set", "change where alerts are sent"),
    ("agent_settings_set", "change the node agents' metrics settings"),
    ("notify_settings_set", "change alert notifications"),
    ("rules_set", "change the alert rules"),
    ("files_mac", "change files on this Mac"),
    ("container_reveal", "show a secret in a container's settings"),
    // ── p1r1 ──
    ("fence_open", "open a fence that allows changes on the cluster"),
    ("fence_trust_key", "trust this Mac's fence key in this build"),
    ("node_identity_repin", "accept a different machine as one of your nodes"),
    ("process_reveal", "show a process's full command line"),
    ("agent_push_config", "push the alert settings to the node agents"),
    ("agent_install", "install the SparkBox agent on your nodes"),
    // ── R2-OPS ──
    ("maintenance_begin", "put a node into maintenance"),
    ("maintenance_end", "end a node's maintenance"),
    // ── R2-CONN ── (lib/admin.ts ACTION_WORDS says the same in the in-app sheet)
    ("tunnel_open", "open a web page on a node"),
    // ── p1r3 ──
    ("updates_hold_preset", "pin or release the GPU stack on a node"),
    ("container_test_run", "start a test container on a node"),
    ("files_new_file", "create a file"),
    // ── p1r4 ──
    ("deployment_forget", "forget a load (unloads it first if its containers are still there)"),
    // ── p2r1 ──
    ("deployment_clean_leftovers", "remove containers an earlier load left on a node"),
];

/// The prompt for `key` (a command name), or the scope's own words for anything unknown.
pub fn prompt_for(key: Option<&str>, scope: Scope) -> &'static str {
    let fallback = match scope {
        Scope::Node => "node",
        Scope::Mac => "mac",
    };
    let find = |k: &str| PROMPTS.iter().find(|(name, _)| *name == k).map(|(_, text)| *text);
    key.and_then(find).or_else(|| find(fallback)).unwrap_or("unlock Admin mode")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SecuritySettings {
    /// Relock after this many minutes without a privileged action: one of IDLE_CHOICES.
    pub idle_minutes: u32,
    /// Activity log files whose newest entry is older than this are deleted: 30, 90 or 365 days.
    pub retention_days: u32,
    /// An open shell locks after this many minutes with nothing typed: 15, 60, 240, or 0 (never).
    pub shell_idle_minutes: u32,
    /// Look-only mode, kept across launches (Settings › Security). Turning it on needs
    /// nothing; turning it off asks for Touch ID or the password (`look_only_set`).
    pub look_only: bool,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        SecuritySettings { idle_minutes: 15, retention_days: 90, shell_idle_minutes: 60, look_only: false }
    }
}

/// What the top bar shows; sent as `admin://state` (at most once a second while in use).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminState {
    pub unlocked: bool,
    pub expires_at_ms: Option<u64>,
    pub idle_minutes: u32,
    /// Unlocking will ask for Touch ID or the password (Live data).
    pub requires_auth: bool,
    /// The owner confirmed with Touch ID or the password in the last 5 minutes: Mac-scope
    /// changes are allowed.
    pub authenticated: bool,
    /// When the Mac scope runs out (5 minutes after the confirmation; nothing extends it).
    pub mac_expires_at_ms: Option<u64>,
    /// When this unlock started (for the activity log's session row).
    pub unlocked_at_ms: Option<u64>,
    /// Why it last locked by itself ("No changes for 15 minutes"), for the notice.
    pub lock_reason: Option<String>,
    /// How long before a lock its warning shows (1 minute; less with SPARKBOX_IDLE_SCALE).
    pub warn_before_ms: u64,
    /// Look-only mode: the padlock reads "Look only" and nothing unlocks.
    pub look_only: bool,
    /// Look-only was set by SPARKBOX_LOOK_ONLY for this run, so Settings can't turn it off.
    pub look_only_forced: bool,
}

struct Gate {
    /// Locked when None.
    until_ms: Option<u64>,
    last_activity_ms: u64,
    unlocked_at_ms: u64,
    /// When LocalAuthentication last confirmed the owner during this unlock: the Mac scope lasts
    /// MAC_SCOPE_MS from it. Cleared on every relock.
    confirmed_ms: Option<u64>,
    /// A confirmation asked for one command that always confirms (a Keychain write): the
    /// command and when. Used once.
    fresh: Option<(String, u64)>,
    /// Goes up on every relock, so work that started unlocked can tell a relock happened
    /// meanwhile (a shell still connecting).
    generation: u64,
}

type Hook = Arc<dyn Fn(RelockReason, &str) + Send + Sync>;
type HoldHook = Arc<dyn Fn(&str) -> HeldTasks + Send + Sync>;

pub struct AdminGate {
    gate: Mutex<Gate>,
    settings: Mutex<SecuritySettings>,
    settings_path: Option<PathBuf>,
    live: AtomicBool,
    /// Debug builds with SPARKBOX_ADMIN_NOAUTH=1, for driving the app in scripts.
    no_auth: bool,
    /// SPARKBOX_LOOK_ONLY=1 (`look_only_forced`); tests set it directly.
    look_only_forced: bool,
    /// This is the app's own gate: its look-only switch is mirrored into `look_only()`.
    system: bool,
    clock: Arc<dyn Clock>,
    auth: Box<dyn Authenticator>,
    /// One prompt at a time; a second unlock waits for the first.
    prompting: Mutex<()>,
    lock_reason: Mutex<Option<String>>,
    /// Run after every relock with its reason: close tunnels, end shells when it says so.
    relock_hooks: Mutex<Vec<Hook>>,
    /// Goes up on every relock that holds deferred work (`RelockReason::holds`) and every time
    /// look-only turns on, whether or not Admin mode was unlocked (`hold_generation`).
    hold_generation: AtomicU64,
    /// What moved it last.
    last_hold: Mutex<Hold>,
    /// Run when look-only turns on: Tasks stops what isn't a read (`on_hold`).
    hold_hooks: Mutex<Vec<HoldHook>>,
    last_emit_ms: AtomicU64,
    emit_pending: AtomicBool,
}

impl AdminGate {
    pub fn new(clock: Box<dyn Clock>, auth: Box<dyn Authenticator>, settings_path: Option<PathBuf>) -> Arc<AdminGate> {
        let settings = settings_path.as_deref().map(crate::util::load_json_or_default::<SecuritySettings>).unwrap_or_default();
        Arc::new(AdminGate {
            gate: Mutex::new(Gate { until_ms: None, last_activity_ms: 0, unlocked_at_ms: 0, confirmed_ms: None, fresh: None, generation: 0 }),
            settings: Mutex::new(sanitize(settings)),
            settings_path,
            live: AtomicBool::new(false),
            // Never both: look-only wins, so a scripted run can't unlock against the real cluster.
            no_auth: cfg!(debug_assertions) && !look_only_forced() && std::env::var("SPARKBOX_ADMIN_NOAUTH").is_ok_and(|v| v == "1"),
            look_only_forced: false,
            system: false,
            clock: Arc::from(clock),
            auth,
            prompting: Mutex::new(()),
            lock_reason: Mutex::new(None),
            relock_hooks: Mutex::new(Vec::new()),
            hold_generation: AtomicU64::new(0),
            last_hold: Mutex::new(Hold::Locked),
            hold_hooks: Mutex::new(Vec::new()),
            last_emit_ms: AtomicU64::new(0),
            emit_pending: AtomicBool::new(false),
        })
    }

    /// The real thing: wall clock and Touch ID / password.
    pub fn system(settings_path: PathBuf) -> Arc<AdminGate> {
        let mut gate = AdminGate::new(Box::new(SystemClock), Box::new(MacAuthenticator), Some(settings_path));
        let g = Arc::get_mut(&mut gate).expect("a new gate has one owner");
        g.look_only_forced = look_only_forced();
        g.system = true;
        LOOK_ONLY_SETTING.store(g.settings.get_mut().unwrap().look_only, Ordering::SeqCst);
        gate
    }

    /// Look-only mode: SPARKBOX_LOOK_ONLY=1 or the Settings switch.
    pub fn look_only(&self) -> bool {
        self.look_only_forced || self.settings.lock().unwrap().look_only
    }

    /// Turns the Settings switch on or off. On takes effect at once and locks, holds deferred
    /// work (`hold_generation` moves) and runs the hold hooks, which stop every task that isn't a
    /// read; the answer says how many. Off asks for Touch ID or the password first (it gives
    /// rights back), and can't undo SPARKBOX_LOOK_ONLY. Blocks while macOS shows its prompt.
    pub fn set_look_only(&self, on: bool) -> Result<LookOnlyView, String> {
        let was = self.look_only();
        if !on {
            if self.look_only_forced {
                return Err("SPARKBOX_LOOK_ONLY=1 set look-only mode for this run. Quit and start SparkBox without it.".into());
            }
            if self.look_only() {
                let _one = self.prompting.lock().unwrap_or_else(|p| p.into_inner());
                if !self.no_auth {
                    self.auth.authenticate("turn off look-only mode")?;
                }
            }
        }
        let mut settings = self.settings();
        settings.look_only = on;
        if let Some(path) = &self.settings_path {
            crate::util::save_json(path, &settings)?;
        }
        *self.settings.lock().unwrap() = settings;
        if self.system {
            LOOK_ONLY_SETTING.store(on, Ordering::SeqCst);
        }
        let mut held = HeldTasks::default();
        // Already on (the environment, or a second click): nothing to lock or stop.
        if on && !was {
            self.lock_with(RelockReason::Explicit, "Look-only mode is on");
            held = self.hold_now();
        }
        Ok(LookOnlyView { state: self.state(), held })
    }

    /// Look-only just came on: deferred work scheduled before now is held, and every hold hook
    /// runs (Tasks stops what isn't a read). Returns what the hooks stopped.
    fn hold_now(&self) -> HeldTasks {
        self.bump_hold(Hold::LookOnly);
        let hooks: Vec<HoldHook> = self.hold_hooks.lock().unwrap().clone();
        let mut held = HeldTasks::default();
        for hook in hooks {
            held += hook(LOOK_ONLY_HOLD);
        }
        held
    }

    fn bump_hold(&self, why: Hold) {
        *self.last_hold.lock().unwrap() = why;
        self.hold_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Goes up every time look-only turns on and on every relock but an idle one or the window
    /// closing, unlocked or not. Deferred work records it when it's scheduled and checks it
    /// right before it acts (`hold_view().held_since(…)`).
    pub fn hold_generation(&self) -> u64 {
        self.hold_generation.load(Ordering::SeqCst)
    }

    /// Look-only and the hold generation now, for work scheduled earlier to compare against.
    pub fn hold_view(&self) -> HoldView {
        let last = *self.last_hold.lock().unwrap();
        HoldView { look_only: self.look_only(), generation: self.hold_generation(), last }
    }

    /// Called with "Look-only turned on" each time look-only turns on; returns what it stopped.
    pub fn on_hold(&self, hook: impl Fn(&str) -> HeldTasks + Send + Sync + 'static) {
        self.hold_hooks.lock().unwrap().push(Arc::new(hook));
    }

    /// The gate's clock, shared with the shells' idle timer.
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    fn idle_ms(&self) -> u64 {
        scaled(u64::from(self.settings.lock().unwrap().idle_minutes) * 60_000)
    }

    fn requires_auth(&self) -> bool {
        self.live.load(Ordering::SeqCst) && !self.no_auth
    }

    /// Whether the data is Live (Mac scope then covers what `require_mac_live` guards).
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    /// Tells the gate which data source is in use. Moving to Live relocks it, since a Demo
    /// unlock never asked who you are; returns true when that happened.
    pub fn set_live(&self, live: bool) -> bool {
        let was = self.live.swap(live, Ordering::SeqCst);
        live && !was && self.lock_with(RelockReason::LiveData, "Live data is on: unlocking now asks for Touch ID or your Mac password.")
    }

    fn mac_until(g: &Gate) -> Option<u64> {
        g.until_ms.and(g.confirmed_ms).map(|t| t + scaled(MAC_SCOPE_MS))
    }

    pub fn state(&self) -> AdminState {
        let now = self.clock.now_ms();
        let idle_minutes = self.settings.lock().unwrap().idle_minutes;
        let warn_before_ms = WARN_MS.min(self.idle_ms() / 2);
        let g = self.gate.lock().unwrap();
        let mac_until = Self::mac_until(&g).filter(|until| now < *until);
        AdminState {
            unlocked: g.until_ms.is_some(),
            expires_at_ms: g.until_ms,
            idle_minutes,
            requires_auth: self.requires_auth(),
            authenticated: mac_until.is_some(),
            mac_expires_at_ms: mac_until,
            unlocked_at_ms: g.until_ms.map(|_| g.unlocked_at_ms),
            lock_reason: self.lock_reason.lock().unwrap().clone(),
            warn_before_ms,
            look_only: self.look_only(),
            look_only_forced: self.look_only_forced,
        }
    }

    /// Goes up on every relock (see `Gate::generation`).
    pub fn generation(&self) -> u64 {
        self.gate.lock().unwrap().generation
    }

    /// Node scope: Ok while unlocked, and pushes the deadline back (being used keeps it open).
    pub fn check(&self) -> Result<(), String> {
        self.check_scope(Scope::Node)
    }

    /// Mac scope: unlocked and confirmed with Touch ID or the password in the last 5 minutes.
    pub fn check_mac(&self) -> Result<(), String> {
        self.check_scope(Scope::Mac)
    }

    pub fn check_scope(&self, scope: Scope) -> Result<(), String> {
        if self.look_only() {
            return Err(LOOK_ONLY.into());
        }
        let now = self.clock.now_ms();
        let idle = self.idle_ms();
        let mut g = self.gate.lock().unwrap();
        match g.until_ms {
            Some(until) if now < until => {
                if scope == Scope::Mac && !Self::mac_until(&g).is_some_and(|t| now < t) {
                    return Err(NEEDS_AUTH.into());
                }
                // Being used keeps Admin mode open; the Mac scope's own clock never moves.
                g.last_activity_ms = now;
                g.until_ms = Some(now + idle);
                Ok(())
            }
            Some(_) => {
                // Expired before the ticker noticed: the relock happens here.
                drop(g);
                let minutes = self.settings.lock().unwrap().idle_minutes;
                self.lock_with(RelockReason::Idle, &format!("No changes for {minutes} minutes, so SparkBox is read-only again."));
                Err(LOCKED.into())
            }
            None => Err(LOCKED.into()),
        }
    }

    /// Mac scope plus a confirmation asked for `key` itself in the last minute (Keychain
    /// writes). Each confirmation covers one call.
    pub fn check_confirmed(&self, key: &str) -> Result<(), String> {
        self.check_mac()?;
        let now = self.clock.now_ms();
        let mut g = self.gate.lock().unwrap();
        match g.fresh.take() {
            Some((k, at)) if k == key && now.saturating_sub(at) < FRESH_MS => Ok(()),
            _ => Err(NEEDS_CONFIRM.into()),
        }
    }

    /// Unlocks (node scope), asking for Touch ID or the password first when the data is Live.
    /// `key` names the command that asked (see `PROMPTS`). Blocks while macOS shows its
    /// prompt, so call it off the async workers.
    pub fn unlock(&self, key: Option<&str>) -> Result<AdminState, String> {
        // Look-only: refused before anything can ask for Touch ID.
        if self.look_only() {
            return Err(LOOK_ONLY.into());
        }
        let _one = self.prompting.lock().unwrap_or_else(|p| p.into_inner());
        if self.state().unlocked {
            self.check()?;
            return Ok(self.state());
        }
        let asked = self.requires_auth();
        if asked {
            self.auth.authenticate(&unlock_prompt(key))?;
        }
        self.open(asked);
        Ok(self.state())
    }

    /// Mac scope: confirms it's the owner (Touch ID or the password), with Demo data too, and
    /// unlocks too when locked. Asks again once the 5 minutes are up, and always when `fresh`.
    /// A confirmation also covers one call of `key` when that command confirms every time.
    pub fn authenticate(&self, key: Option<&str>, fresh: bool) -> Result<AdminState, String> {
        if self.look_only() {
            return Err(LOOK_ONLY.into());
        }
        let _one = self.prompting.lock().unwrap_or_else(|p| p.into_inner());
        if self.state().authenticated && !fresh {
            self.check_mac()?;
            return Ok(self.state());
        }
        if self.no_auth && key.is_some_and(never_without_auth) {
            return Err("Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts.".into());
        }
        if !self.no_auth {
            self.auth.authenticate(prompt_for(key, Scope::Mac))?;
        }
        let now = self.clock.now_ms();
        if self.state().unlocked {
            let idle = self.idle_ms();
            let mut g = self.gate.lock().unwrap();
            g.confirmed_ms = Some(now);
            g.last_activity_ms = now;
            g.until_ms = Some(now + idle);
        } else {
            self.open(true);
        }
        // This confirmation was asked for `key`: it also covers one call of it that confirms every time.
        self.gate.lock().unwrap().fresh = key.map(|k| (k.to_string(), now));
        Ok(self.state())
    }

    fn open(&self, authenticated: bool) {
        let now = self.clock.now_ms();
        let idle = self.idle_ms();
        let mut g = self.gate.lock().unwrap();
        let generation = g.generation;
        *g = Gate { until_ms: Some(now + idle), last_activity_ms: now, unlocked_at_ms: now, confirmed_ms: authenticated.then_some(now), fresh: None, generation };
        drop(g);
        *self.lock_reason.lock().unwrap() = None;
    }

    /// "Keep unlocked": pushes the deadline back while still unlocked. Never unlocks.
    pub fn extend(&self) -> Result<AdminState, String> {
        self.check()?;
        Ok(self.state())
    }

    /// Locks now; true when it was unlocked. Doesn't run the relock hooks (see `lock_with`).
    pub fn lock(&self) -> bool {
        let mut g = self.gate.lock().unwrap();
        g.confirmed_ms = None;
        g.fresh = None;
        g.generation += 1;
        g.until_ms.take().is_some()
    }

    /// Locks, remembers why (for the notice the UI shows) and runs the relock hooks. Every reason
    /// but an idle one or the window closing also holds deferred work, even when it was locked
    /// already (a queued restart doesn't run after the Mac slept).
    pub fn lock_with(&self, why: RelockReason, text: &str) -> bool {
        let was = self.lock();
        if why.holds() {
            self.bump_hold(Hold::Locked);
        }
        if was {
            *self.lock_reason.lock().unwrap() = Some(text.to_string());
            self.run_hooks(why, text);
        }
        was
    }

    /// macOS locked the screen (`com.apple.screenIsLocked`).
    pub fn screen_locked(&self) -> bool {
        self.lock_with(RelockReason::ScreenLock, "The screen was locked")
    }

    /// Called after every relock with its reason and its words: closes tunnels, ends shells.
    pub fn on_relock(&self, hook: impl Fn(RelockReason, &str) + Send + Sync + 'static) {
        self.relock_hooks.lock().unwrap().push(Arc::new(hook));
    }

    fn run_hooks(&self, why: RelockReason, text: &str) {
        let hooks: Vec<Hook> = self.relock_hooks.lock().unwrap().clone();
        for hook in hooks {
            hook(why, text);
        }
    }

    /// Called every second: relocks once the idle time has run out; true when it just did.
    pub fn tick(&self) -> bool {
        let now = self.clock.now_ms();
        let expired = matches!(self.gate.lock().unwrap().until_ms, Some(until) if now >= until);
        if !expired {
            return false;
        }
        let minutes = self.settings.lock().unwrap().idle_minutes;
        self.lock_with(RelockReason::Idle, &format!("No changes for {minutes} minutes, so SparkBox is read-only again."))
    }

    /// Whether a state event may go out now (at most one a second); otherwise it's left
    /// pending for the ticker.
    fn may_emit(&self) -> bool {
        let now = self.clock.now_ms();
        let last = self.last_emit_ms.load(Ordering::SeqCst);
        if now.saturating_sub(last) >= EMIT_EVERY_MS || now < last {
            self.last_emit_ms.store(now, Ordering::SeqCst);
            self.emit_pending.store(false, Ordering::SeqCst);
            true
        } else {
            self.emit_pending.store(true, Ordering::SeqCst);
            false
        }
    }

    fn take_pending_emit(&self) -> bool {
        self.emit_pending.swap(false, Ordering::SeqCst)
    }

    pub fn settings(&self) -> SecuritySettings {
        self.settings.lock().unwrap().clone()
    }

    /// Saves new settings. A shorter idle time applies to the current unlock straight away.
    pub fn set_settings(&self, settings: SecuritySettings) -> Result<SecuritySettings, String> {
        if !IDLE_CHOICES.contains(&settings.idle_minutes) {
            return Err("Auto-lock can be 5 or 15 minutes, or 1, 2, 4, 8 or 24 hours.".into());
        }
        if !RETENTION_CHOICES.contains(&settings.retention_days) {
            return Err("The activity log can be kept for 30, 90 or 365 days.".into());
        }
        if !SHELL_IDLE_CHOICES.contains(&settings.shell_idle_minutes) {
            return Err("Idle shells can lock after 15 minutes, 60 minutes, 4 hours or never.".into());
        }
        if let Some(path) = &self.settings_path {
            crate::util::save_json(path, &settings)?;
        }
        *self.settings.lock().unwrap() = settings.clone();
        let idle = self.idle_ms();
        let mut g = self.gate.lock().unwrap();
        if let Some(until) = g.until_ms {
            g.until_ms = Some(until.min(g.last_activity_ms + idle));
        }
        Ok(settings)
    }
}

fn unlock_prompt(key: Option<&str>) -> String {
    match prompt_for(key, Scope::Node) {
        "unlock Admin mode" => "unlock Admin mode".into(),
        what => format!("unlock Admin mode to {what}"),
    }
}

fn sanitize(mut s: SecuritySettings) -> SecuritySettings {
    if !SHELL_IDLE_CHOICES.contains(&s.shell_idle_minutes) {
        s.shell_idle_minutes = SecuritySettings::default().shell_idle_minutes;
    }
    if !IDLE_CHOICES.contains(&s.idle_minutes) {
        s.idle_minutes = SecuritySettings::default().idle_minutes;
    }
    if !RETENTION_CHOICES.contains(&s.retention_days) {
        s.retention_days = SecuritySettings::default().retention_days;
    }
    s
}

fn to_sb(message: String) -> SbError {
    let kind = if message == LOOK_ONLY {
        ErrorKind::LookOnly
    } else if message == NEEDS_AUTH || message == NEEDS_CONFIRM {
        ErrorKind::NeedsAuth
    } else {
        ErrorKind::Locked
    };
    SbError::new(kind, message)
}

/// The gate's answer as an `SbError` (kind `locked` or `needs_auth`), telling the top bar when
/// the deadline moved or it just relocked.
fn gate_check(app: &AppHandle, check: impl FnOnce(&AdminGate) -> Result<(), String>) -> Result<(), SbError> {
    let gate = app.try_state::<Arc<AdminGate>>().ok_or_else(|| to_sb(LOCKED.to_string()))?;
    let result = check(&gate);
    if matches!(&result, Err(e) if e == LOCKED) || gate.may_emit() {
        let _ = app.emit(STATE_EVENT, gate.state());
    }
    result.map_err(to_sb)
}

/// What a `String`-returning command's gate refuses with: the same `SbError`, as JSON.
fn as_string_refusal(result: Result<(), SbError>) -> Result<(), String> {
    result.map_err(|e| e.to_json())
}

/// Node scope: the first statement of every command that changes a node, a switch or a model.
pub fn require(app: &AppHandle) -> Result<(), String> {
    as_string_refusal(require_sb(app))
}

/// Mac scope: the first statement of every command that changes state on this Mac.
pub fn require_mac(app: &AppHandle) -> Result<(), String> {
    as_string_refusal(gate_check(app, |g| g.check_mac()))
}

/// Mac scope with Live data, node scope in Demo: for settings that only matter to the real
/// cluster's alerting (silences, rules), which a Demo session may try freely while Live ones
/// need Touch ID.
pub fn require_mac_live(app: &AppHandle) -> Result<(), String> {
    as_string_refusal(gate_check(app, |g| if g.is_live() { g.check_mac() } else { g.check() }))
}

/// Mac scope confirmed for this very command (`key`, its name) in the last minute: Keychain
/// writes and changes to where a saved secret is sent. The UI asks with
/// `admin_authenticate(action, fresh: true)` right before.
pub fn require_mac_confirmed(app: &AppHandle, key: &str) -> Result<(), String> {
    as_string_refusal(gate_check(app, |g| g.check_confirmed(key)))
}

/// Mac scope when `place` is the Mac (Files' left pane), node scope otherwise.
pub fn require_for_place(app: &AppHandle, place: &str) -> Result<(), String> {
    as_string_refusal(gate_check(app, |g| if place == crate::files::MAC { g.check_mac() } else { g.check() }))
}

/// `require` for commands that return `SbError` (kind `locked`).
pub fn require_sb(app: &AppHandle) -> Result<(), SbError> {
    gate_check(app, |g| g.check())
}

pub fn emit(app: &AppHandle) {
    if let Some(gate) = app.try_state::<Arc<AdminGate>>() {
        gate.last_emit_ms.store(gate.clock.now_ms(), Ordering::SeqCst);
        let _ = app.emit(STATE_EVENT, gate.state());
    }
}

/// Locks (if unlocked), tells the UI and notes why in the activity log.
pub fn lock_because(app: &AppHandle, why: RelockReason, text: &str) {
    let Some(gate) = app.try_state::<Arc<AdminGate>>() else { return };
    let started = gate.state().unlocked_at_ms;
    if gate.lock_with(why, text) {
        record_lock(app, started, text);
    }
    emit(app);
}

fn record(app: &AppHandle, title: &str, detail: &str) {
    if let Some(state) = app.try_state::<crate::AppState>() {
        state.live.audit.record_app(title, detail);
    }
}

/// "Admin mode locked", with when the session started so the log can fold the pair.
fn record_lock(app: &AppHandle, started_ms: Option<u64>, why: &str) {
    let detail = match started_ms {
        Some(ms) => format!("{why} · unlocked at {}", crate::live::audit::iso_time(ms)),
        None => why.to_string(),
    };
    record(app, "Admin mode locked", &detail);
}

/// Relocks when the idle time runs out, flushes held-back state events, relocks after the Mac
/// sleeps (the wall clock jumps while the monotonic one doesn't) and drops unused Keychain
/// secrets from memory. Spawned once at start.
pub fn start_ticker(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(1));
        let mut last = (SystemTime::now(), std::time::Instant::now());
        let mut mac_was = false;
        loop {
            every.tick().await;
            let now = (SystemTime::now(), std::time::Instant::now());
            let wall = now.0.duration_since(last.0).unwrap_or_default();
            let mono = now.1.duration_since(last.1);
            last = now;
            if wall > mono + Duration::from_secs(20) {
                lock_because(&app, RelockReason::Sleep, "The Mac went to sleep");
            }
            // Keychain secrets don't stay in memory unused (secrets.rs).
            crate::secrets::forget_idle();
            let Some(gate) = app.try_state::<Arc<AdminGate>>() else { continue };
            let before = gate.state();
            if gate.tick() {
                let minutes = gate.settings().idle_minutes;
                record_lock(&app, before.unlocked_at_ms, &format!("No changes for {minutes} minutes"));
                emit(&app);
            } else if gate.take_pending_emit() || before.authenticated != mac_was {
                // Also when the Mac scope's 5 minutes ran out: the UI asks again next time.
                emit(&app);
            }
            mac_was = gate.state().authenticated;
        }
    });
}

/// Relocks when macOS locks the screen: listens for the `com.apple.screenIsLocked`
/// distributed notification. Called once at start, on the main thread's run loop.
pub fn watch_screen_lock(app: &AppHandle) {
    use block2::RcBlock;
    use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSString};

    let handle = app.clone();
    let block = RcBlock::new(move |_note: std::ptr::NonNull<NSNotification>| {
        let app = handle.clone();
        // Leave the notification callback quickly; the hooks may close sockets.
        tauri::async_runtime::spawn(async move {
            let Some(gate) = app.try_state::<Arc<AdminGate>>() else { return };
            let started = gate.state().unlocked_at_ms;
            if gate.screen_locked() {
                record_lock(&app, started, "The screen was locked");
            }
            emit(&app);
        });
    });
    let name = NSString::from_str("com.apple.screenIsLocked");
    // SAFETY: plain Foundation calls; the observer token is kept for the app's lifetime.
    unsafe {
        let center = NSDistributedNotificationCenter::defaultCenter();
        let token = center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block);
        std::mem::forget(token);
    }
}

// ---- Commands

#[tauri::command]
pub fn admin_state(gate: State<'_, Arc<AdminGate>>) -> AdminState {
    gate.state()
}

/// Unlocks Admin mode. `reason` names the command that asked (it picks the prompt's words from
/// `PROMPTS`; free text is ignored). `scope: "mac"` confirms it's you even with Demo data.
#[tauri::command]
pub async fn admin_unlock(app: AppHandle, reason: Option<String>, scope: Option<Scope>) -> Result<AdminState, SbError> {
    if scope == Some(Scope::Mac) {
        return admin_authenticate(app, reason, None).await;
    }
    let gate = app.state::<Arc<AdminGate>>().inner().clone();
    let was = gate.state();
    let asked = was.requires_auth;
    let key = reason.clone();
    let result = tauri::async_runtime::spawn_blocking(move || gate.unlock(key.as_deref()))
        .await
        .map_err(|e| SbError::failed(e.to_string()))?;
    match result {
        Ok(state) => {
            if !was.unlocked {
                let how = if asked { "Confirmed with Touch ID or the Mac password" } else { "Demo data: no password needed" };
                record(&app, "Admin mode unlocked", &format!("{how} · {}", prompt_for(reason.as_deref(), Scope::Node)));
            }
            emit(&app);
            Ok(state)
        }
        Err(e) => Err(refused(e)),
    }
}

/// Confirms it's the owner (Touch ID or the Mac password) so Mac-scope changes are allowed
/// for the next 5 minutes, unlocking too when locked. `action` names the command that asked,
/// for the prompt's words. `fresh` asks even within the 5 minutes, for a command that
/// confirms every time (`require_mac_confirmed`); that confirmation covers one call of it.
/// Every confirmation is in the activity log.
#[tauri::command]
pub async fn admin_authenticate(app: AppHandle, action: Option<String>, fresh: Option<bool>) -> Result<AdminState, SbError> {
    let gate = app.state::<Arc<AdminGate>>().inner().clone();
    let was = gate.state();
    let key = action.clone();
    let fresh = fresh.unwrap_or(false);
    let result = tauri::async_runtime::spawn_blocking(move || gate.authenticate(key.as_deref(), fresh))
        .await
        .map_err(|e| SbError::failed(e.to_string()))?;
    match result {
        Ok(state) => {
            if !was.authenticated || fresh {
                let what = prompt_for(action.as_deref(), Scope::Mac);
                let title = if was.unlocked { "Admin mode: confirmed it's you" } else { "Admin mode unlocked" };
                let lasts = if fresh { "for this change only" } else { "Mac changes allowed for 5 minutes" };
                record(&app, title, &format!("Confirmed with Touch ID or the Mac password · to {what} · {lasts}"));
            }
            emit(&app);
            Ok(state)
        }
        Err(e) => Err(refused(e)),
    }
}

fn refused(e: String) -> SbError {
    if e == LOOK_ONLY {
        SbError::new(ErrorKind::LookOnly, LOOK_ONLY)
    } else if e == "Cancelled." {
        SbError::new(ErrorKind::Cancelled, "Admin mode stays locked.")
    } else {
        SbError::new(ErrorKind::AuthRejected, format!("Admin mode stays locked. {e}"))
    }
}

/// "Keep unlocked" in the relock warning: extends only while still unlocked, never unlocks,
/// and isn't logged as an unlock.
#[tauri::command]
pub fn admin_extend(app: AppHandle) -> Result<AdminState, SbError> {
    let gate = app.state::<Arc<AdminGate>>();
    let state = gate.extend().map_err(to_sb)?;
    emit(&app);
    Ok(state)
}

#[tauri::command]
pub fn admin_lock(app: AppHandle) -> AdminState {
    lock_because(&app, RelockReason::Explicit, "Locked from the top bar");
    app.state::<Arc<AdminGate>>().state()
}

/// Settings › Security › Look only. On is immediate and locks Admin mode; off asks for Touch
/// ID or the Mac password, and can't undo SPARKBOX_LOOK_ONLY=1.
#[tauri::command]
pub async fn look_only_set(app: AppHandle, on: bool) -> Result<LookOnlyView, SbError> {
    if !on {
        // Off gives rights back, so it asks who you are (inside set_look_only).
        let gate = app.state::<Arc<AdminGate>>().inner().clone();
        let view = tauri::async_runtime::spawn_blocking(move || gate.set_look_only(false))
            .await
            .map_err(|e| SbError::failed(e.to_string()))?
            .map_err(|e| if e == "Cancelled." { SbError::new(ErrorKind::Cancelled, "Look-only mode stays on.") } else { SbError::failed(e) })?;
        record(&app, "Look-only mode off", "Confirmed with Touch ID or the Mac password");
        emit(&app);
        return Ok(view);
    }
    let gate = app.state::<Arc<AdminGate>>();
    let was = gate.look_only();
    let view = gate.set_look_only(true).map_err(SbError::failed)?;
    if !was {
        let mut detail = "Starts no change on the cluster or this Mac".to_string();
        if let Some(words) = view.held.words() {
            detail.push_str(&format!(" · {words}"));
        }
        record(&app, "Look-only mode on", &detail);
    }
    emit(&app);
    Ok(view)
}

#[tauri::command]
pub fn security_get(gate: State<'_, Arc<AdminGate>>) -> SecuritySettings {
    gate.settings()
}

#[tauri::command]
pub fn security_set(app: AppHandle, settings: SecuritySettings) -> Result<SecuritySettings, String> {
    crate::admin::require_mac(&app)?;
    let gate = app.state::<Arc<AdminGate>>();
    let old = gate.settings();
    // Look-only has its own switch (look_only_set): this one never changes it.
    let saved = gate.set_settings(SecuritySettings { look_only: old.look_only, ..settings })?;
    let mut changes = Vec::new();
    if old.idle_minutes != saved.idle_minutes {
        changes.push(format!("Auto-lock {} → {} min", old.idle_minutes, saved.idle_minutes));
    }
    if old.retention_days != saved.retention_days {
        changes.push(format!("Keep the activity log {} → {} days", old.retention_days, saved.retention_days));
    }
    if old.shell_idle_minutes != saved.shell_idle_minutes {
        let words = |m: u32| if m == 0 { "never".to_string() } else if m % 60 == 0 { format!("{} h", m / 60) } else { format!("{m} min") };
        changes.push(format!("Lock idle shells after {} → {}", words(old.shell_idle_minutes), words(saved.shell_idle_minutes)));
    }
    if let Some(state) = app.try_state::<crate::AppState>() {
        state.terminals.set_idle_minutes(saved.shell_idle_minutes);
        state.live.audit.set_retention_days(saved.retention_days);
        if !changes.is_empty() {
            state.live.audit.record_app("Security settings changed", &changes.join(" · "));
        }
    }
    emit(&app);
    Ok(saved)
}

/// Whether SparkBox opens (in the menu bar) at login.
#[tauri::command]
pub fn autostart_get(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn autostart_set(app: AppHandle, enabled: bool) -> Result<bool, String> {
    crate::admin::require_mac(&app)?;
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let was = manager.is_enabled().unwrap_or(false);
    if enabled { manager.enable() } else { manager.disable() }.map_err(|e| format!("macOS didn't change the login item: {e}"))?;
    let now = manager.is_enabled().map_err(|e| e.to_string())?;
    if was != now {
        record(&app, "Open at login changed", &format!("{} → {}", if was { "On" } else { "Off" }, if now { "On" } else { "Off" }));
    }
    Ok(now)
}

/// A gate for tests anywhere in the crate: a clock the test moves and an authenticator that
/// says yes until told to refuse. Background code takes the gate, so its tests toggle
/// look-only and relock on one of these instead of the app-wide switch.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{AdminGate, Authenticator, Clock};

    #[derive(Clone, Default)]
    pub struct FakeClock(pub Arc<AtomicU64>);
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    impl FakeClock {
        pub fn advance(&self, minutes: u64) {
            self.0.fetch_add(minutes * 60_000, Ordering::SeqCst);
        }
        pub fn advance_ms(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
    }

    #[derive(Clone, Default)]
    pub struct FakeAuth {
        pub asked: Arc<AtomicUsize>,
        pub refuse: Arc<AtomicBool>,
        pub reasons: Arc<Mutex<Vec<String>>>,
    }
    impl Authenticator for FakeAuth {
        fn authenticate(&self, reason: &str) -> Result<(), String> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.reasons.lock().unwrap().push(reason.into());
            if self.refuse.load(Ordering::SeqCst) {
                Err("Cancelled.".into())
            } else {
                Ok(())
            }
        }
    }

    pub fn gate() -> (Arc<AdminGate>, FakeClock, FakeAuth) {
        let clock = FakeClock(Arc::new(AtomicU64::new(1_000_000)));
        let auth = FakeAuth::default();
        let mut g = AdminGate::new(Box::new(clock.clone()), Box::new(auth.clone()), None);
        Arc::get_mut(&mut g).unwrap().no_auth = false;
        (g, clock, auth)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::testing::{gate, FakeAuth, FakeClock};
    use super::*;

    #[test]
    fn locked_until_unlocked_then_relocks_when_idle() {
        let (g, clock, _) = gate();
        assert_eq!(g.check().unwrap_err(), LOCKED);
        let s = g.unlock(None).unwrap();
        assert!(s.unlocked);
        assert_eq!(s.expires_at_ms, Some(1_000_000 + 15 * 60_000));
        assert!(g.check().is_ok());

        // Using it keeps it open: 10 minutes, a change, 10 more minutes.
        clock.advance(10);
        assert!(g.check().is_ok());
        clock.advance(10);
        assert!(!g.tick(), "20 minutes since unlock but only 10 idle");
        assert!(g.check().is_ok());

        clock.advance(16);
        assert!(g.tick(), "idle for 16 minutes relocks");
        assert!(!g.state().unlocked);
        assert_eq!(g.check().unwrap_err(), LOCKED);
    }

    #[test]
    fn expiry_is_enforced_even_before_the_ticker_runs() {
        let (g, clock, _) = gate();
        g.unlock(None).unwrap();
        clock.advance(15);
        assert_eq!(g.check().unwrap_err(), LOCKED);
        assert!(!g.state().unlocked, "and it's locked now, not just refused");
    }

    #[test]
    fn demo_unlocks_without_asking_and_live_asks() {
        let (g, _, auth) = gate();
        g.unlock(None).unwrap();
        assert_eq!(auth.asked.load(Ordering::SeqCst), 0, "Demo never prompts for node scope");
        assert!(!g.state().requires_auth);
        assert!(!g.state().authenticated);

        assert!(g.set_live(true), "switching to Live relocks a Demo unlock");
        assert!(g.state().requires_auth);
        assert!(g.check().is_err());
        g.unlock(Some("node_power")).unwrap();
        assert_eq!(auth.asked.load(Ordering::SeqCst), 1);
        assert_eq!(auth.reasons.lock().unwrap()[0], "unlock Admin mode to restart, shut down or wake a node");
        assert!(g.state().authenticated, "a Live unlock confirmed it's you");

        // Already unlocked: no second prompt.
        g.unlock(None).unwrap();
        assert_eq!(auth.asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_prompt_comes_from_the_table_never_from_the_ui() {
        let (g, _, auth) = gate();
        g.set_live(true);
        g.unlock(Some("Transfer all your money")).unwrap();
        assert_eq!(auth.reasons.lock().unwrap()[0], "unlock Admin mode", "unknown keys get the scope's own words");
        g.lock();
        g.authenticate(Some("vpn_set_endpoint"), false).unwrap();
        assert_eq!(auth.reasons.lock().unwrap()[1], "change the VPN endpoint");
        assert_eq!(prompt_for(Some("nonsense"), Scope::Mac), "change settings on this Mac");
        assert_eq!(prompt_for(Some("agent_push_config"), Scope::Node), "push the alert settings to the node agents");
        assert_eq!(unlock_prompt(Some("tunnel_open")), "unlock Admin mode to open a web page on a node");
    }

    #[test]
    fn after_a_demo_unlock_node_commands_pass_and_mac_commands_need_auth() {
        let (g, _, auth) = gate();
        g.unlock(None).unwrap();
        assert!(g.check().is_ok(), "node scope");
        assert_eq!(g.check_mac().unwrap_err(), NEEDS_AUTH, "Mac scope after a no-password Demo unlock");
        assert_eq!(to_sb(NEEDS_AUTH.into()).kind, ErrorKind::NeedsAuth);
        assert_eq!(to_sb(LOCKED.into()).kind, ErrorKind::Locked);
        assert_eq!(auth.asked.load(Ordering::SeqCst), 0);

        // Confirming it's you (admin_authenticate) opens both scopes.
        let s = g.authenticate(Some("save_inventory"), false).unwrap();
        assert!(s.unlocked && s.authenticated);
        assert_eq!(auth.asked.load(Ordering::SeqCst), 1, "Demo or not, Mac scope asks");
        assert_eq!(auth.reasons.lock().unwrap()[0], "change the cluster's nodes and switches");
        assert!(g.check().is_ok());
        assert!(g.check_mac().is_ok());

        // A relock forgets it: the next Demo unlock covers nodes only again.
        g.lock();
        assert_eq!(g.check_mac().unwrap_err(), LOCKED);
        g.unlock(None).unwrap();
        assert_eq!(g.check_mac().unwrap_err(), NEEDS_AUTH);
    }

    #[test]
    fn mac_scope_when_locked_unlocks_with_authentication_and_a_refusal_changes_nothing() {
        let (g, _, auth) = gate();
        auth.refuse.store(true, Ordering::SeqCst);
        assert_eq!(g.authenticate(None, false).unwrap_err(), "Cancelled.");
        assert!(!g.state().unlocked);
        g.unlock(None).unwrap();
        assert!(g.authenticate(None, false).is_err());
        assert!(g.state().unlocked && !g.state().authenticated, "still the Demo unlock, nothing more");
        auth.refuse.store(false, Ordering::SeqCst);
        g.lock();
        let s = g.authenticate(None, false).unwrap();
        assert!(s.unlocked && s.authenticated);
    }

    #[test]
    fn extend_never_re_unlocks() {
        let (g, clock, auth) = gate();
        assert_eq!(g.extend().unwrap_err(), LOCKED, "locked stays locked");
        g.unlock(None).unwrap();
        clock.advance(10);
        let s = g.extend().unwrap();
        assert_eq!(s.expires_at_ms, Some(clock.now_ms() + 15 * 60_000));
        clock.advance(15);
        assert!(g.extend().is_err(), "expired: Keep unlocked comes too late");
        assert!(!g.state().unlocked);
        assert_eq!(auth.asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn screen_lock_relocks_and_revokes_shells() {
        let (g, _, _) = gate();
        let revoked = Arc::new(Mutex::new(Vec::<String>::new()));
        let r = revoked.clone();
        g.on_relock(move |_, why| r.lock().unwrap().push(why.to_string()));
        assert!(!g.screen_locked(), "nothing to do while locked");
        assert!(revoked.lock().unwrap().is_empty());
        g.authenticate(None, false).unwrap();
        assert!(g.screen_locked());
        assert!(!g.state().unlocked && !g.state().authenticated);
        assert_eq!(*revoked.lock().unwrap(), ["The screen was locked"]);
        assert_eq!(g.state().lock_reason.as_deref(), Some("The screen was locked"));
    }

    #[test]
    fn every_relock_runs_the_hooks() {
        let (g, clock, _) = gate();
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        g.on_relock(move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        g.unlock(None).unwrap();
        clock.advance(16);
        assert!(g.tick(), "idle");
        g.unlock(None).unwrap();
        g.lock_with(RelockReason::Explicit, "Locked from the menu bar");
        g.unlock(None).unwrap();
        g.set_live(true);
        g.unlock(None).unwrap();
        clock.advance(16);
        assert!(g.check().is_err(), "expired when checked");
        assert_eq!(count.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn state_events_go_out_at_most_once_a_second() {
        let (g, clock, _) = gate();
        assert!(g.may_emit());
        let mut sent = 0;
        for _ in 0..50 {
            clock.advance_ms(30);
            if g.may_emit() {
                sent += 1;
            }
        }
        // 1.5 s of calls every 30 ms: one more event after the first second, the rest pending.
        assert_eq!(sent, 1);
        assert!(g.take_pending_emit(), "the ticker flushes the last one");
        assert!(!g.take_pending_emit());
    }

    #[test]
    fn a_refused_prompt_keeps_it_locked() {
        let (g, _, auth) = gate();
        g.set_live(true);
        auth.refuse.store(true, Ordering::SeqCst);
        assert_eq!(g.unlock(None).unwrap_err(), "Cancelled.");
        assert!(g.check().is_err());
    }

    #[test]
    fn settings_are_validated_and_a_shorter_idle_applies_now() {
        let (g, clock, _) = gate();
        g.unlock(None).unwrap();
        assert!(g.set_settings(SecuritySettings { idle_minutes: 7, retention_days: 90, shell_idle_minutes: 60, look_only: false }).is_err());
        assert!(g.set_settings(SecuritySettings { idle_minutes: 15, retention_days: 10, shell_idle_minutes: 60, look_only: false }).is_err());
        g.set_settings(SecuritySettings { idle_minutes: 5, retention_days: 30, shell_idle_minutes: 60, look_only: false }).unwrap();
        assert!(g.set_settings(SecuritySettings { idle_minutes: 5, retention_days: 30, shell_idle_minutes: 30, look_only: false }).is_err(), "idle shells: 15, 60, 240 or never");
        assert_eq!(g.state().idle_minutes, 5);
        clock.advance(6);
        assert!(g.tick());
    }

    #[test]
    fn lock_reports_whether_it_was_open() {
        let (g, _, _) = gate();
        assert!(!g.lock());
        g.unlock(None).unwrap();
        assert!(g.lock());
        assert!(!g.state().unlocked);
    }

    #[test]
    fn all_admin_durations_persist_but_an_unlock_never_does() {
        let path = std::env::temp_dir().join(format!("sparkbox-security-{}-{}.json", std::process::id(), fastrand::u64(..)));
        let (g, clock, auth) = gate();
        assert_eq!(g.settings().idle_minutes, 15, "existing default stays unchanged");
        let saved = AdminGate::new(Box::new(clock.clone()), Box::new(auth.clone()), Some(path.clone()));
        for minutes in [5, 15, 60, 120, 240, 480, 1440] {
            saved.set_settings(SecuritySettings { idle_minutes: minutes, ..SecuritySettings::default() }).unwrap();
            saved.unlock(None).unwrap();
            let reopened = AdminGate::new(Box::new(clock.clone()), Box::new(auth.clone()), Some(path.clone()));
            assert_eq!(reopened.settings().idle_minutes, minutes);
            assert!(!reopened.state().unlocked, "reopening always starts locked");
        }
        for minutes in [0, 7, 30, 1441, u32::MAX] {
            let invalid = SecuritySettings { idle_minutes: minutes, ..SecuritySettings::default() };
            assert!(saved.set_settings(invalid.clone()).is_err());
            assert_eq!(sanitize(invalid).idle_minutes, 15);
            assert_eq!(saved.settings().idle_minutes, 1440, "invalid choice cannot replace the last saved choice");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_full_day_still_expires_at_the_exact_idle_boundary() {
        for use_ticker in [false, true] {
            let (g, clock, _) = gate();
            g.set_settings(SecuritySettings { idle_minutes: 1440, ..g.settings() }).unwrap();
            g.unlock(None).unwrap();
            assert_eq!(g.state().expires_at_ms, Some(clock.now_ms() + 86_400_000));
            clock.advance_ms(86_399_999);
            assert!(!g.tick());
            assert!(g.state().unlocked);
            clock.advance_ms(1);
            if use_ticker { assert!(g.tick()); } else { assert_eq!(g.check().unwrap_err(), LOCKED); }
            assert!(!g.state().unlocked);
        }
    }

    #[test]
    fn a_longer_choice_waits_for_activity_and_never_extends_mac_confirmation() {
        let (g, clock, _) = gate();
        g.authenticate(Some("security_set"), false).unwrap();
        let before = g.state();
        g.set_settings(SecuritySettings { idle_minutes: 1440, ..g.settings() }).unwrap();
        assert_eq!(g.state().expires_at_ms, before.expires_at_ms);
        clock.advance(1);
        g.check().unwrap();
        assert_eq!(g.state().expires_at_ms, Some(clock.now_ms() + 86_400_000));
        assert_eq!(g.state().mac_expires_at_ms, before.mac_expires_at_ms);
        clock.advance(4);
        assert_eq!(g.check_mac().unwrap_err(), NEEDS_AUTH);
        assert!(g.state().unlocked, "the node session remains open");
        g.set_settings(SecuritySettings { idle_minutes: 5, ..g.settings() }).unwrap();
        clock.advance(1);
        assert!(g.tick(), "shortening still uses the last activity, not the settings change");
    }

    #[test]
    fn a_full_day_does_not_bypass_relock_or_look_only() {
        let (g, _, _) = gate();
        g.set_live(true);
        g.set_settings(SecuritySettings { idle_minutes: 1440, ..g.settings() }).unwrap();
        for why in [RelockReason::Explicit, RelockReason::ScreenLock, RelockReason::Sleep, RelockReason::Quit, RelockReason::WindowClosed] {
            g.authenticate(Some("security_set"), false).unwrap();
            assert!(g.lock_with(why, "test"));
            assert_eq!(g.check().unwrap_err(), LOCKED);
            assert!(g.check_mac().is_err());
        }
        g.unlock(None).unwrap();
        g.set_look_only(true).unwrap();
        assert_eq!(g.check().unwrap_err(), LOOK_ONLY);
        assert_eq!(g.unlock(None).unwrap_err(), LOOK_ONLY);
    }

    #[test]
    fn the_mac_scope_lasts_five_minutes_and_node_actions_dont_extend_it() {
        let (g, clock, auth) = gate();
        g.authenticate(Some("save_inventory"), false).unwrap();
        assert!(g.check_mac().is_ok());
        let until = g.state().mac_expires_at_ms.unwrap();
        assert_eq!(until, clock.now_ms() + MAC_SCOPE_MS);
        // Four minutes of node changes and one Mac change keep Admin mode open, not the Mac scope.
        for _ in 0..4 {
            clock.advance(1);
            g.check().unwrap();
        }
        g.check_mac().unwrap();
        assert_eq!(g.state().mac_expires_at_ms, Some(until), "nothing moves the Mac scope's clock");
        clock.advance(1);
        assert_eq!(g.check_mac().unwrap_err(), NEEDS_AUTH, "5 minutes after Touch ID");
        assert!(g.check().is_ok(), "node scope is still open");
        assert!(!g.state().authenticated && g.state().unlocked);
        // Confirming again asks again.
        g.authenticate(Some("save_inventory"), false).unwrap();
        assert_eq!(auth.asked.load(Ordering::SeqCst), 2);
        assert!(g.check_mac().is_ok());
    }

    #[test]
    fn keychain_writes_confirm_every_time_and_each_confirmation_covers_one_call() {
        let (g, clock, auth) = gate();
        g.authenticate(Some("save_inventory"), false).unwrap();
        assert_eq!(g.check_confirmed("set_credential").unwrap_err(), NEEDS_CONFIRM, "the Mac scope alone isn't enough");
        assert_eq!(to_sb(NEEDS_CONFIRM.into()).kind, ErrorKind::NeedsAuth);
        g.authenticate(Some("set_credential"), true).unwrap();
        assert_eq!(auth.asked.load(Ordering::SeqCst), 2, "asked again within the 5 minutes");
        assert_eq!(auth.reasons.lock().unwrap()[1], "save a password in the Keychain");
        assert!(g.check_confirmed("set_credential").is_ok());
        assert!(g.check_confirmed("set_credential").is_err(), "used once");
        g.authenticate(Some("clear_credential"), true).unwrap();
        assert!(g.check_confirmed("set_credential").is_err(), "a confirmation for one command doesn't cover another");
        g.authenticate(Some("set_credential"), true).unwrap();
        clock.advance(2);
        assert!(g.check_confirmed("set_credential").is_err(), "and it doesn't wait around");
    }

    #[test]
    fn a_scripted_unlock_confirms_nothing_sparkbox_trusts() {
        let clock = FakeClock(Arc::new(AtomicU64::new(1_000_000)));
        let auth = FakeAuth::default();
        let mut g = AdminGate::new(Box::new(clock), Box::new(auth.clone()), None);
        Arc::get_mut(&mut g).unwrap().no_auth = true;
        g.set_live(true);
        // SPARKBOX_ADMIN_NOAUTH still unlocks and confirms the rest, without asking…
        g.authenticate(Some("set_credential"), true).unwrap();
        assert!(g.check_confirmed("set_credential").is_ok());
        // …but never what SparkBox trusts.
        for key in NOAUTH_NEVER {
            let e = g.authenticate(Some(key), true).unwrap_err();
            assert_eq!(e, "Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts.", "{key}");
            assert_eq!(g.check_confirmed(key).unwrap_err(), NEEDS_CONFIRM, "{key}");
        }
        assert_eq!(auth.asked.load(Ordering::SeqCst), 0, "no prompt either way");
    }

    #[test]
    fn relocks_say_why_and_only_some_end_shells() {
        let (g, clock, _) = gate();
        let seen = Arc::new(Mutex::new(Vec::<RelockReason>::new()));
        let s = seen.clone();
        g.on_relock(move |why, _| s.lock().unwrap().push(why));
        let gen = g.generation();
        g.unlock(None).unwrap();
        clock.advance(16);
        assert!(g.tick());
        g.unlock(None).unwrap();
        g.lock_with(RelockReason::WindowClosed, "The window was closed");
        g.unlock(None).unwrap();
        g.screen_locked();
        assert_eq!(*seen.lock().unwrap(), [RelockReason::Idle, RelockReason::WindowClosed, RelockReason::ScreenLock]);
        assert_eq!(g.generation(), gen + 3, "every relock moves the generation");
        assert!(!RelockReason::Idle.ends_shells() && !RelockReason::WindowClosed.ends_shells());
        for why in [RelockReason::Explicit, RelockReason::ScreenLock, RelockReason::Sleep, RelockReason::Quit, RelockReason::LiveData] {
            assert!(why.ends_shells(), "{why:?}");
        }
    }

    /// Deferred work compares the hold generation it was scheduled under: an idle relock or the
    /// window closing leaves it alone, every other relock (locked already or not) and look-only
    /// coming on move it, and turning look-only on runs the hold hooks once.
    #[test]
    fn the_hold_generation_moves_on_every_relock_but_an_idle_one_and_when_look_only_turns_on() {
        let (g, clock, _) = gate();
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let c = calls.clone();
        g.on_hold(move |why| {
            c.lock().unwrap().push(why.to_string());
            HeldTasks { stopped: 2, stopping: 1, on_node: 1 }
        });
        let start = g.hold_generation();
        g.unlock(None).unwrap();
        clock.advance(16);
        assert!(g.tick(), "idle relock");
        g.unlock(None).unwrap();
        g.lock_with(RelockReason::WindowClosed, "The window was closed");
        assert_eq!(g.hold_generation(), start, "idle and window-closed relocks hold nothing");
        assert_eq!(g.hold_view().held_since(start), None);

        for why in [RelockReason::Explicit, RelockReason::ScreenLock, RelockReason::Sleep, RelockReason::Quit, RelockReason::LiveData] {
            let before = g.hold_generation();
            assert!(!g.lock_with(why, "already locked"), "{why:?}: nothing was unlocked");
            assert_eq!(g.hold_generation(), before + 1, "{why:?} holds deferred work even when it was locked already");
            assert_eq!(g.hold_view().held_since(before), Some(Hold::Locked));
        }
        assert!(calls.lock().unwrap().is_empty(), "relocks don't stop running tasks");

        let before = g.hold_generation();
        let v = g.set_look_only(true).unwrap();
        assert!(v.state.look_only);
        assert_eq!(v.held, HeldTasks { stopped: 2, stopping: 1, on_node: 1 }, "the hooks' counts come back for the toast");
        assert_eq!(*calls.lock().unwrap(), [LOOK_ONLY_HOLD]);
        assert_eq!(g.hold_view().held_since(before), Some(Hold::LookOnly));
        assert_eq!(g.hold_view().held_since(g.hold_generation()), Some(Hold::LookOnly), "and while it's on, everything is held");
        assert_eq!(g.set_look_only(true).unwrap().held, HeldTasks::default(), "already on: nothing new to stop");
        assert_eq!(calls.lock().unwrap().len(), 1);

        g.set_look_only(false).unwrap();
        let now = g.hold_generation();
        assert_eq!(g.hold_view().held_since(now), None, "work scheduled after look-only ended runs");
        assert_eq!(g.hold_view().held_since(before), Some(Hold::LookOnly), "work from before it stays held");
        assert_eq!((Hold::LookOnly.label(), Hold::Locked.label()), ("Held · look-only", "Held · locked"));
        assert_eq!(HeldTasks { stopped: 1, stopping: 2, on_node: 0 }.words().as_deref(), Some("1 task stopped before its next step · 2 tasks' next changes are refused"));
        assert_eq!(
            HeldTasks { stopped: 0, stopping: 1, on_node: 2 }.words().as_deref(),
            Some("1 task's next change is refused · 2 tasks' work keeps running on their nodes")
        );
        assert_eq!(HeldTasks::default().words(), None);
    }

    /// One refusal, one shape: an `SbError` for commands that return it (service_action,
    /// node_power, task_cancel, files_delete, tunnel_open), and the same error as JSON for those
    /// that return strings (endpoint_forget, and every other plain gate). errors.ts reads both.
    #[test]
    fn gate_refusals_have_one_shape_whatever_the_command_returns() {
        let (g, _, _) = gate();
        let as_object = g.check().map_err(to_sb);
        let as_string = as_string_refusal(g.check().map_err(to_sb));
        let object = serde_json::to_value(as_object.clone().unwrap_err()).unwrap();
        let string: serde_json::Value = serde_json::from_str(&as_string.clone().unwrap_err()).unwrap();
        assert_eq!(object, string);
        assert_eq!(object, serde_json::json!({ "kind": "locked", "message": LOCKED }));
        assert_eq!(SbError::from(as_string.unwrap_err()), as_object.unwrap_err(), "`?` from a String gate into an SbError command keeps it");
        g.unlock(None).unwrap();
        let mac: serde_json::Value = serde_json::from_str(&as_string_refusal(g.check_mac().map_err(to_sb)).unwrap_err()).unwrap();
        assert_eq!(mac["kind"], "needs_auth");
    }

    // ---- Look-only mode (r4 F0)

    fn look_only_gate() -> (Arc<AdminGate>, FakeClock, FakeAuth) {
        let (mut g, clock, auth) = gate();
        Arc::get_mut(&mut g).unwrap().look_only_forced = true;
        g.set_live(true);
        (g, clock, auth)
    }

    #[test]
    fn look_only_refuses_every_gate_and_unlocking_never_prompts() {
        let (g, _, auth) = look_only_gate();
        assert!(g.state().look_only && g.state().look_only_forced, "the padlock can say Look only");
        for refusal in [g.check(), g.check_mac(), g.check_confirmed("set_credential")] {
            assert_eq!(refusal.unwrap_err(), LOOK_ONLY);
        }
        assert_eq!(g.unlock(Some("node_power")).unwrap_err(), LOOK_ONLY);
        assert_eq!(g.authenticate(Some("save_inventory"), true).unwrap_err(), LOOK_ONLY);
        assert_eq!(auth.asked.load(Ordering::SeqCst), 0, "no Touch ID or password prompt, ever");
        assert!(!g.state().unlocked);
        let e = to_sb(g.check().unwrap_err());
        assert_eq!(serde_json::to_value(&e).unwrap(), serde_json::json!({ "kind": "lookOnly", "message": LOOK_ONLY }));
        assert_eq!(refused(LOOK_ONLY.into()).kind, ErrorKind::LookOnly, "admin_unlock refuses with the same kind");
        assert_eq!(g.set_look_only(false).unwrap_err(), "SPARKBOX_LOOK_ONLY=1 set look-only mode for this run. Quit and start SparkBox without it.");
        assert!(g.state().look_only, "the Settings switch can't undo the environment");
        assert_eq!(auth.asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_look_only_switch_turns_on_at_once_and_off_with_touch_id() {
        let (g, _, auth) = gate();
        g.unlock(None).unwrap();
        let s = g.set_look_only(true).unwrap().state;
        assert!(s.look_only && !s.unlocked, "on is immediate and locks");
        assert_eq!(auth.asked.load(Ordering::SeqCst), 0, "turning it on asks nothing");
        assert_eq!(g.unlock(None).unwrap_err(), LOOK_ONLY);
        auth.refuse.store(true, Ordering::SeqCst);
        assert_eq!(g.set_look_only(false).unwrap_err(), "Cancelled.");
        assert!(g.state().look_only, "a cancelled prompt keeps it on");
        auth.refuse.store(false, Ordering::SeqCst);
        assert!(!g.set_look_only(false).unwrap().state.look_only);
        assert_eq!(auth.asked.load(Ordering::SeqCst), 2, "off asks every time");
        assert_eq!(auth.reasons.lock().unwrap()[1], "turn off look-only mode");
        assert!(g.unlock(None).is_ok());
        let mut settings = g.settings();
        settings.idle_minutes = 5;
        assert!(!g.set_settings(settings).unwrap().look_only);
    }

    /// With look-only on, every registered command outside READ_ONLY is refused before it runs,
    /// and every READ_ONLY command passes except LOOK_ONLY_HOLDS (each with its reason).
    #[test]
    fn in_look_only_only_reads_get_through() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let report = audit_commands(&Sources::from_dir(&src), READ_ONLY, CONDITIONAL, MAC_SCOPE);
        assert!(report.registered.len() > 100);
        let listed = |list: &[(&str, &str)], n: &str| list.iter().any(|(x, _)| *x == n);
        let (mut refused_n, mut passed_n) = (0, 0);
        for name in &report.registered {
            let refusal = refusal_in_look_only(name);
            let expect_pass = (listed(READ_ONLY, name) && !listed(LOOK_ONLY_HOLDS, name)) || listed(LOOK_ONLY_CONDITIONAL, name);
            assert_eq!(refusal.is_none(), expect_pass, "{name}");
            if let Some(e) = refusal {
                assert_eq!((e.kind, e.message.as_str()), (ErrorKind::LookOnly, LOOK_ONLY));
                refused_n += 1;
            } else {
                passed_n += 1;
            }
            if !listed(READ_ONLY, name) && !listed(LOOK_ONLY_CONDITIONAL, name) {
                assert!(refusal_in_look_only(name).is_some(), "{name} changes something and must be refused");
            }
        }
        assert!(refused_n > 100 && passed_n > 80, "refused {refused_n}, passed {passed_n}");
        for (n, _) in LOOK_ONLY_HOLDS {
            assert!(listed(READ_ONLY, n), "{n} is held in look-only, so it must be a READ_ONLY command");
        }
        for (n, _) in LOOK_ONLY_CONDITIONAL {
            assert!(listed(CONDITIONAL, n), "{n} passes look-only on its ungated calls, so it must be CONDITIONAL");
        }
        for must in ["save_inventory", "fleet_start", "node_power", "files_transfer", "prefs_patch", "data_source_set", "notify_channel_test", "firewalla_msp_test", "firewalla_label_set", "firewalla_msp_set_domain", "admin_extend"] {
            if report.registered.iter().any(|r| r == must) && must != "admin_extend" {
                assert!(refusal_in_look_only(must).is_some(), "{must}");
            }
        }
        for read in ["get_snapshot", "activity_log", "files_list", "files_read", "admin_state", "admin_unlock", "look_only_set", "container_inspect"] {
            assert!(refusal_in_look_only(read).is_none(), "{read}");
        }
        assert!(refusal_in_look_only("not_a_command").is_some(), "anything unknown is refused");
    }

    // ---- The deny-by-default gate test (G1)

    use gate_audit::{audit_commands, Sources};

    /// Every command registered in lib.rs's `generate_handler![…]` either starts with an
    /// `admin::require…(…)?` call or is listed in READ_ONLY (or CONDITIONAL) with a reason;
    /// every list entry is registered, and every `#[tauri::command]` fn is registered.
    #[test]
    fn every_command_is_gated_or_listed() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = Sources::from_dir(&src);
        let report = audit_commands(&sources, READ_ONLY, CONDITIONAL, MAC_SCOPE);
        assert!(report.registered.len() > 100, "parsed the handler list ({} commands)", report.registered.len());
        assert!(report.errors.is_empty(), "{} problem(s):\n{}", report.errors.len(), report.errors.join("\n"));
    }

    /// The parser isn't fooled: an ungated command, a gate whose result is ignored, a gate
    /// after other work, a stale list entry and an unregistered command are all caught.
    #[test]
    fn the_gate_audit_catches_ungated_commands() {
        let attr = ["#[tauri::", "command]"].concat();
        let lib = format!(
            "mod x;\n{attr}\nfn read_thing() -> u32 {{ 1 }}\n{attr}\nfn wipe(app: AppHandle) -> Result<(), String> {{\n    // a comment first is fine\n    do_wipe();\n    admin::require(&app)?;\n    Ok(())\n}}\n\
             {attr}\nasync fn gated(app: AppHandle) -> Result<(), String> {{\n    /* note */ admin::require_mac(&app)?;\n    Ok(())\n}}\n\
             {attr}\nfn ignored(app: AppHandle) {{\n    let _ = admin::require(&app);\n}}\n\
             {attr}\nfn sometimes(app: AppHandle, shell: bool) -> Result<(), String> {{\n    if shell {{\n        admin::require(&app)?;\n    }}\n    Ok(())\n}}\n\
             {attr}\nfn orphan() {{}}\n\
             fn run() {{ tauri::Builder::default().invoke_handler(tauri::generate_handler![\n    read_thing,\n    wipe, // ── r2-x ──\n    gated,\n    ignored,\n    sometimes,\n    x::nested,\n    x::commands::inline_open,\n    // not_registered,\n]); }}\n"
        );
        let module = format!(
            "{attr}\npub fn nested<R>(app: AppHandle<R>) -> Result<(), String>\nwhere R: Runtime {{\n    crate::admin::require_for_place(&app, \"mac\")?;\n    Ok(())\n}}\n\
             pub mod commands {{\n    {attr}\n    pub fn inline_open(app: AppHandle) {{\n        do_it();\n    }}\n}}\n"
        );
        let sources = Sources::from_files(vec![("lib.rs".into(), lib), ("x.rs".into(), module)]);
        let report = audit_commands(&sources, &[("read_thing", "returns a number"), ("stale_one", "was removed long ago")], &[("short", "x")], &[("nested", "changes a file on this Mac"), ("gated", "changes a setting on this Mac")]);
        let errors = report.errors.join("\n");
        assert_eq!(report.registered, ["read_thing", "wipe", "gated", "ignored", "sometimes", "nested", "inline_open"], "comments stripped, paths resolved");
        assert!(report.errors.iter().any(|e| e.starts_with("inline_open (x.rs) must start with admin::require")), "an inline `mod commands {{}}` is read too");
        assert!(errors.contains("wipe (lib.rs) must start with admin::require"), "{errors}");
        assert!(errors.contains("ignored (lib.rs) must start with admin::require"), "a discarded result isn't a gate: {errors}");
        assert!(errors.contains("sometimes (lib.rs) has a conditional gate"), "{errors}");
        assert!(errors.contains("stale_one is listed in READ_ONLY but isn't registered"), "{errors}");
        assert!(errors.contains("short is listed in CONDITIONAL but isn't registered"), "{errors}");
        assert!(errors.contains("orphan (lib.rs) is a #[tauri::command] but isn't registered"), "{errors}");
        assert!(!errors.contains("gated (lib.rs)") && !errors.contains("read_thing (") && !errors.contains("nested ("), "{errors}");
        assert_eq!(report.errors.len(), 7, "{errors}");

        // A reason too short to mean anything is refused.
        let report = audit_commands(&sources, &[("read_thing", "fine")], &[], &[("nested", "changes a file on this Mac"), ("gated", "changes a setting on this Mac")]);
        assert!(report.errors.iter().any(|e| e.contains("read_thing needs a reason")), "{:?}", report.errors);
    }

    /// The negative fixture for scope: a Mac-scope command behind the node gate, a Mac gate
    /// missing from MAC_SCOPE, a Mac-scope command also called read-only, and duplicates.
    #[test]
    fn the_gate_audit_checks_scope_and_duplicates() {
        let attr = ["#[tauri::", "command]"].concat();
        let lib = format!(
            "{attr}\nfn keychain_set(app: AppHandle) -> Result<(), String> {{\n    admin::require(&app)?;\n    Ok(())\n}}\n\
             {attr}\nfn vpn_edit(app: AppHandle) -> Result<(), String> {{\n    admin::require_mac(&app)?;\n    Ok(())\n}}\n\
             {attr}\nfn silence_add(app: AppHandle) -> Result<(), String> {{\n    admin::require_mac_live(&app)?;\n    Ok(())\n}}\n\
             {attr}\nfn key_save(app: AppHandle) -> Result<(), String> {{\n    admin::require_mac_confirmed(&app, \"key_save\")?;\n    Ok(())\n}}\n\
             {attr}\nfn prefs_put(app: AppHandle, big: bool) -> Result<(), String> {{\n    if big {{\n        admin::require_mac(&app)?;\n    }}\n    Ok(())\n}}\n\
             {attr}\nfn peek() -> u32 {{ 1 }}\n\
             fn run() {{ tauri::Builder::default().invoke_handler(tauri::generate_handler![keychain_set, vpn_edit, silence_add, key_save, prefs_put, peek]); }}\n"
        );
        let sources = Sources::from_files(vec![("lib.rs".into(), lib)]);
        let report = audit_commands(
            &sources,
            &[("peek", "returns a number"), ("peek", "returns a number again"), ("key_save", "says whether a key is saved")],
            &[("prefs_put", "small preferences are UI-only")],
            &[("keychain_set", "writes the Keychain"), ("silence_add", "adds a silence on this Mac"), ("key_save", "saves a key in the Keychain"), ("prefs_put", "the data source on this Mac")],
        );
        let errors = report.errors.join("\n");
        assert!(errors.contains("keychain_set (lib.rs) is in MAC_SCOPE but its gate is require;"), "{errors}");
        assert!(errors.contains("vpn_edit (lib.rs) checks the Mac scope (require_mac); list it in MAC_SCOPE"), "{errors}");
        assert!(errors.contains("peek is listed twice in READ_ONLY"), "{errors}");
        assert!(errors.contains("key_save is in both MAC_SCOPE and READ_ONLY"), "{errors}");
        assert!(!errors.contains("silence_add (") && !errors.contains("prefs_put ("), "require_mac_live and a conditional Mac gate count: {errors}");
        assert_eq!(report.errors.len(), 5, "{errors}");
    }
}

/// Parses lib.rs's `generate_handler![…]`, finds each command's fn and reads its first
/// statement. Used by the deny-by-default test above.
#[cfg(test)]
mod gate_audit {
    use std::collections::BTreeMap;
    use std::path::Path;

    pub struct Sources {
        /// Path relative to src/ ("lib.rs", "ops/commands.rs") → text.
        files: BTreeMap<String, String>,
    }

    impl Sources {
        pub fn from_dir(dir: &Path) -> Sources {
            fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
                for entry in std::fs::read_dir(dir).unwrap().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, base, out);
                    } else if path.extension().is_some_and(|e| e == "rs") {
                        let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                        out.insert(rel, std::fs::read_to_string(&path).unwrap_or_default());
                    }
                }
            }
            let mut files = BTreeMap::new();
            walk(dir, dir, &mut files);
            Sources { files }
        }

        pub fn from_files(files: Vec<(String, String)>) -> Sources {
            Sources { files: files.into_iter().collect() }
        }

        /// The file a module path lives in: `ops::commands` → ops/commands.rs or ops/commands/mod.rs,
        /// or its parent's file when it's an inline `mod commands { … }` there.
        fn module(&self, path: &[&str]) -> Option<(&str, &str)> {
            if path.is_empty() {
                return self.files.get_key_value("lib.rs").map(|(k, v)| (k.as_str(), v.as_str()));
            }
            let joined = path.join("/");
            [format!("{joined}.rs"), format!("{joined}/mod.rs")]
                .into_iter()
                .find_map(|f| self.files.get_key_value(&f).map(|(k, v)| (k.as_str(), v.as_str())))
                .or_else(|| {
                    let (last, parent) = path.split_last()?;
                    let (file, text) = self.module(parent)?;
                    text.contains(&format!("mod {last} {{")).then_some((file, text))
                })
        }
    }

    pub struct Report {
        pub registered: Vec<String>,
        pub errors: Vec<String>,
    }

    /// Removes // and /* */ comments, leaving string literals alone.
    pub fn strip_comments(src: &str) -> String {
        let b = src.as_bytes();
        let mut out = String::with_capacity(src.len());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'"' => {
                    let start = i;
                    i += 1;
                    while i < b.len() && b[i] != b'"' {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                    i = (i + 1).min(b.len());
                    out.push_str(&src[start..i]);
                }
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    i += 2;
                    while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                        i += 1;
                    }
                    i += 2;
                    out.push(' ');
                }
                _ => {
                    let ch = src[i..].chars().next().unwrap();
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        out
    }

    /// The paths inside `generate_handler![…]`.
    fn handler_paths(lib: &str) -> Vec<String> {
        let lib = strip_comments(lib);
        let Some(at) = lib.find("generate_handler![") else { return Vec::new() };
        let rest = &lib[at + "generate_handler![".len()..];
        let end = rest.find(']').unwrap_or(rest.len());
        rest[..end].split(',').map(|p| p.split_whitespace().collect::<String>()).filter(|p| !p.is_empty()).collect()
    }

    /// Where `#[tauri::command]` fns start in `src` (comments stripped): (name, offset of "fn").
    fn commands_in(src: &str) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(i) = src[from..].find("#[tauri::command]") {
            let at = from + i;
            from = at + 1;
            // Only an attribute on its own line (not the text inside a string).
            let line_start = src[..at].rfind('\n').map_or(0, |n| n + 1);
            if !src[line_start..at].trim().is_empty() {
                continue;
            }
            // Skip further attributes and qualifiers up to `fn name`.
            let after = &src[at..];
            let Some(f) = after.find("fn ") else { continue };
            let name: String = after[f + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            out.push((name, at + f));
        }
        out
    }

    /// The statements a fn body starts with: (first, the first inside it when it's an `if`).
    fn first_statement(src: &str, fn_at: usize) -> Option<(String, Option<String>)> {
        let b = src.as_bytes();
        let mut depth = 0i32;
        let mut i = fn_at;
        // The body is the first `{` outside the parameter list and generics.
        while i < b.len() {
            match b[i] {
                b'(' | b'<' => depth += 1,
                b')' | b'>' if !(b[i] == b'>' && i > 0 && b[i - 1] == b'-') => depth -= 1,
                b'{' if depth <= 0 => break,
                _ => {}
            }
            i += 1;
        }
        let body = src.get(i + 1..)?;
        let stmt = statement(body.trim_start());
        let inner = stmt.strip_prefix("if ").and_then(|_| {
            let open = stmt.find('{')?;
            Some(statement(stmt[open + 1..].trim_start()))
        });
        Some((stmt, inner))
    }

    /// One statement: up to `;` at depth 0, or a whole `{ … }` block for an `if`.
    fn statement(s: &str) -> String {
        let mut depth = 0i32;
        let mut in_str = false;
        let mut prev = ' ';
        for (i, c) in s.char_indices() {
            if in_str {
                if c == '"' && prev != '\\' {
                    in_str = false;
                }
                prev = c;
                continue;
            }
            match c {
                '"' => in_str = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' => depth -= 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 && s.starts_with("if ") {
                        return s[..=i].to_string();
                    }
                    if depth < 0 {
                        return s[..i].trim().to_string();
                    }
                }
                ';' if depth == 0 => return s[..=i].to_string(),
                _ => {}
            }
            prev = c;
        }
        s.trim().to_string()
    }

    /// `admin::require(&app)?;`, `crate::admin::require_mac(&app)?;`, `admin::require_for_place(&app, &place)?;`
    fn is_gate(stmt: &str) -> bool {
        let s: String = stmt.split_whitespace().collect();
        let s = s.strip_prefix("crate::").unwrap_or(&s);
        let Some(rest) = s.strip_prefix("admin::require") else { return false };
        let name_end = rest.find('(').unwrap_or(0);
        rest[..name_end].chars().all(|c| c.is_ascii_lowercase() || c == '_') && rest.ends_with(")?;")
    }

    /// The name of the gate a statement calls (`require`, `require_mac`, …), when it is one.
    fn gate_name(stmt: &str) -> Option<String> {
        if !is_gate(stmt) {
            return None;
        }
        let s: String = stmt.split_whitespace().collect();
        let s = s.strip_prefix("crate::").unwrap_or(&s);
        let rest = s.strip_prefix("admin::")?;
        Some(rest[..rest.find('(')?].to_string())
    }

    /// Gates that check the Mac scope (with Live data, at least).
    const MAC_GATES: [&str; 4] = ["require_mac", "require_mac_live", "require_mac_confirmed", "require_for_place"];

    pub fn audit_commands(sources: &Sources, read_only: &[(&str, &str)], conditional: &[(&str, &str)], mac_scope: &[(&str, &str)]) -> Report {
        let mut errors = Vec::new();
        let lib = sources.module(&[]).map(|(_, t)| t).unwrap_or("");
        let paths = handler_paths(lib);
        let mut registered = Vec::new();
        for path in &paths {
            let parts: Vec<&str> = path.split("::").collect();
            let (name, module) = parts.split_last().unwrap();
            registered.push(name.to_string());
            let Some((file, text)) = sources.module(module) else {
                errors.push(format!("{path}: no file for module {}", module.join("::")));
                continue;
            };
            let text = strip_comments(text);
            let Some(&(_, at)) = commands_in(&text).iter().find(|(n, _)| n == name) else {
                errors.push(format!("{path}: no #[tauri::command] fn {name} in {file}"));
                continue;
            };
            let Some((first, inner)) = first_statement(&text, at) else {
                errors.push(format!("{path}: couldn't read the body of {name} in {file}"));
                continue;
            };
            let listed_ro = read_only.iter().any(|(n, _)| n == name);
            let listed_cond = conditional.iter().any(|(n, _)| n == name);
            let listed_mac = mac_scope.iter().any(|(n, _)| n == name);
            let gate = if is_gate(&first) {
                if listed_ro || listed_cond {
                    errors.push(format!("{name} ({file}) is gated but also listed as read-only or conditional; remove it from the list"));
                }
                gate_name(&first)
            } else if inner.as_deref().is_some_and(is_gate) {
                if !listed_cond {
                    errors.push(format!("{name} ({file}) has a conditional gate; list it in CONDITIONAL with the reason, or gate every call"));
                }
                inner.as_deref().and_then(gate_name)
            } else {
                if !listed_ro {
                    errors.push(format!("{name} ({file}) must start with admin::require…(&app)? or be listed in READ_ONLY with a reason; starts with: {first}"));
                }
                None
            };
            let mac_gate = gate.as_deref().is_some_and(|g| MAC_GATES.contains(&g));
            if listed_mac && !mac_gate {
                errors.push(format!("{name} ({file}) is in MAC_SCOPE but its gate is {}; Mac-scope commands start with require_mac, require_mac_live, require_mac_confirmed or require_for_place", gate.as_deref().unwrap_or("missing")));
            }
            // (A conditional Mac gate, like reading ~/.ssh, may stay out: it guards a read.)
            if mac_gate && !listed_mac && !listed_cond {
                errors.push(format!("{name} ({file}) checks the Mac scope ({}); list it in MAC_SCOPE with what it changes on this Mac", gate.as_deref().unwrap_or("")));
            }
        }
        for (list, entries) in [("READ_ONLY", read_only), ("CONDITIONAL", conditional), ("MAC_SCOPE", mac_scope)] {
            let mut seen: Vec<&str> = Vec::new();
            for (name, reason) in entries {
                if seen.contains(name) {
                    errors.push(format!("{name} is listed twice in {list}; keep one entry"));
                }
                seen.push(name);
                if !registered.iter().any(|r| r == name) {
                    errors.push(format!("{name} is listed in {list} but isn't registered in generate_handler!"));
                } else if reason.trim().chars().count() < 12 {
                    errors.push(format!("{name} needs a reason in {list} (at least 12 characters)"));
                }
            }
        }
        for (name, _) in mac_scope {
            if read_only.iter().any(|(n, _)| n == name) {
                errors.push(format!("{name} is in both MAC_SCOPE and READ_ONLY; a command that changes this Mac isn't read-only"));
            }
        }
        for (file, text) in &sources.files {
            for (name, _) in commands_in(&strip_comments(text)) {
                if !registered.contains(&name) {
                    errors.push(format!("{name} ({file}) is a #[tauri::command] but isn't registered; register it or delete it"));
                }
            }
        }
        Report { registered, errors }
    }
}

// ---- R2-SAFE ----

impl AdminGate {
    /// `authenticate(Some(key), fresh: true)` with macOS's words for this one change built by the
    /// command that asks, from what it will do (fence_open: the fence's nodes, classes and hours),
    /// never sent by the UI. It covers one call of `key`, which `require_mac_confirmed` checks.
    pub fn confirm_with_words(&self, key: &str, words: &str) -> Result<AdminState, String> {
        if self.look_only() {
            return Err(LOOK_ONLY.into());
        }
        let _one = self.prompting.lock().unwrap_or_else(|p| p.into_inner());
        if self.no_auth && never_without_auth(key) {
            return Err("Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts.".into());
        }
        if !self.no_auth {
            self.auth.authenticate(words)?;
        }
        let now = self.clock.now_ms();
        if self.state().unlocked {
            let idle = self.idle_ms();
            let mut g = self.gate.lock().unwrap();
            g.confirmed_ms = Some(now);
            g.last_activity_ms = now;
            g.until_ms = Some(now + idle);
        } else {
            self.open(true);
        }
        self.gate.lock().unwrap().fresh = Some((key.to_string(), now));
        Ok(self.state())
    }
}

/// `admin_authenticate(action: key, fresh: true)` with `words` as what macOS says SparkBox is
/// trying to do (`AdminGate::confirm_with_words`), in the activity log the same way.
pub async fn confirm_with_reason(app: &AppHandle, key: &str, words: String) -> Result<AdminState, SbError> {
    let gate = app.state::<Arc<AdminGate>>().inner().clone();
    let was = gate.state();
    let (k, w) = (key.to_string(), words.clone());
    let result = tauri::async_runtime::spawn_blocking(move || gate.confirm_with_words(&k, &w)).await.map_err(|e| SbError::failed(e.to_string()))?;
    match result {
        Ok(state) => {
            let title = if was.unlocked { "Admin mode: confirmed it's you" } else { "Admin mode unlocked" };
            record(app, title, &format!("Confirmed with Touch ID or the Mac password · to {words} · for this change only"));
            emit(app);
            Ok(state)
        }
        Err(e) => Err(refused(e)),
    }
}

#[cfg(test)]
mod r2_safe_tests {
    use super::testing::{FakeAuth, FakeClock};
    use super::*;

    /// SPARKBOX_ADMIN_NOAUTH confirms a fence for nothing, with its words or without (`confirm_with_words`
    /// keeps `authenticate`'s NOAUTH_NEVER rule), and asks nothing.
    #[test]
    fn a_scripted_unlock_confirms_no_fence_with_its_words_either() {
        let auth = FakeAuth::default();
        let mut g = AdminGate::new(Box::new(FakeClock(Arc::new(AtomicU64::new(1_000_000)))), Box::new(auth.clone()), None);
        Arc::get_mut(&mut g).unwrap().no_auth = true;
        g.set_live(true);
        let e = g.confirm_with_words("fence_open", "open a fence: cell2 · 1 class (service) · 3 h · for the release build").unwrap_err();
        assert_eq!(e, "Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts.");
        assert!(g.check_confirmed("fence_open").is_err(), "nothing was confirmed");
        // Unlocked by NOAUTH (another command's confirmation), still nothing for fence_open.
        g.authenticate(Some("set_credential"), true).unwrap();
        assert!(g.confirm_with_words("fence_open", "open a fence: cell2 · 1 class (service) · 3 h · for the release build").is_err());
        assert_eq!(g.check_confirmed("fence_open").unwrap_err(), NEEDS_CONFIRM);
        // fence_confirm's key is `fence_open:<digest of the spec>`: NOAUTH confirms that for nothing too.
        let key = "fence_open:6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b";
        let e = g.confirm_with_words(key, "open a fence: cell2 · 1 class (service) · 3 h · for the release build").unwrap_err();
        assert_eq!(e, "Needs Touch ID: SPARKBOX_ADMIN_NOAUTH can't change what SparkBox trusts.");
        assert_eq!(g.authenticate(Some(key), true).unwrap_err(), e);
        assert_eq!(g.check_confirmed(key).unwrap_err(), NEEDS_CONFIRM);
        assert_eq!(auth.asked.load(Ordering::SeqCst), 0, "no prompt either way");
    }
}
