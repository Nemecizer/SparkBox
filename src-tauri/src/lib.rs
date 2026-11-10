#[cfg(test)]
mod distribution_tests;
#[cfg(all(debug_assertions, target_os = "macos"))]
mod devshot;
mod demo;
#[cfg(test)]
mod fake_node;
#[cfg(test)]
mod fake_upstream;
mod files;
mod firewalla;
mod inventory;
mod versions;
mod live;
mod llm;
mod model;
mod models;
mod ops;
mod prefs;
mod secrets;
mod server;
mod term;
mod tray;
mod tunnels;
mod vpn;
// ── P1 design ──
// ── P2 shell ──
mod menu;
// ── P3 ops ──
mod admin;
mod errors;
mod logging;
mod tasks;
// ── P4 world ──
mod onboard;
mod discovery;
mod probe;
mod routeros;
mod rdp_probe;
mod hardware;
mod howto;
mod util;
// ── P5 monitoring ──
mod health;
mod history;
mod notify;
// ── P6 term-files ──
// ── P7 ai ──
// ── r2-design ──
// ── r2-shell ──
// ── r2-safety ──
// ── r2-fabric ──
mod fabric;
// ── r2-monitor ──
mod agent;
mod delivery;
// ── r2-live ──
mod wol;
// ── r2-ai ──
// ── r3-paging ──
// ── r3-agent ──
// ── r3-incidents ──
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
// ── p1r1-w ──
mod evidence;
mod identity;
// ── p1r2-mac ──
mod awake;
// ── p1r3-foot ──
mod footprint;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use tauri::{ipc::Channel, AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tokio::sync::oneshot;

use demo::DemoCluster;
use files::{Files, Listing, SaveError, Saved, TextFile};
use inventory::Inventory;
use live::Live;
use llm::chat::{ChatCompletion, ChatEvent, ChatRequest};
use llm::history::{Conversation, ConversationSummary, History};
use llm::{Llm, Provider, ProviderView};
use errors::SbError;
use ops::{FleetResult, Ops, Service};
use model::ClusterSnapshot;
use models::advisor::AnalysisResult;
use models::hf::SearchHit;
use models::plan::DeploySpec;
use models::{Benchmark, Ctx, Deployment, Job, LibraryEntry, ModelDetails, Models, NodeCheck, PreviewStep};
use prefs::{DataSource, Prefs};
use server::{LogEntry, SelfTestResult, Server, ServerSettings, ServerView};
use term::{Emit, Target, TermEvent, Terminals};
use tray::Tray;
use tunnels::Tunnels;
use vpn::{Forward, Vpn, VpnMode, VpnView};

const SNAPSHOT_EVENT: &str = "cluster://snapshot";

pub(crate) struct AppState {
    inventory_path: PathBuf,
    inventory: Mutex<Inventory>,
    prefs_path: PathBuf,
    prefs: Mutex<Prefs>,
    known_hosts_path: PathBuf,
    demo: Arc<Mutex<DemoCluster>>,
    live: Arc<Live>,
    latest: RwLock<Option<ClusterSnapshot>>,
    vpn: Arc<Vpn>,
    llm: Arc<Llm>,
    history: History,
    server: Arc<Server>,
    models: Arc<Models>,
    terminals: Arc<Terminals>,
    files: Arc<Files>,
    ops: Arc<Ops>,
    tunnels: Arc<Tunnels>,
    /// Stop signals for replies still streaming, by request id.
    chats: Mutex<HashMap<String, oneshot::Sender<()>>>,
    // ── P1 design ──
    // ── P2 shell ──
    // ── P3 ops ──
    // ── P4 world ──
    // ── P5 monitoring ──
    // ── P6 term-files ──
    // ── P7 ai ──
    // ── r2-design ──
    // ── r2-shell ──
    // ── r2-safety ──
    // ── r2-fabric ──
    // ── r2-monitor ──
    // ── r2-live ──
    // ── r2-ai ──
    // ── r3-paging ──
    // ── r3-agent ──
    // ── r3-incidents ──
    // ── r3-serving ──
    // ── r3-charts ──
    // ── r3-desk ──
    // ── r3-safety ──
    // ── r4-nodes ──
    // ── r4-serving ──
    // ── r4-alerts ──
    // ── r4-agent ──
    // ── r4-safety ──
    /// SPARKBOX_DATA_SOURCE for this run (debug builds): wins over prefs.json, which keeps its value.
    source_override: Option<DataSource>,
    // ── r4-demo ──
    // ── r4-craft ──
}

/// Builds a fresh snapshot, sends it to the UI and keeps it for `get_snapshot`.
pub(crate) fn publish(app: &AppHandle, advance: bool) {
    let state = app.state::<AppState>();
    let snap = if is_live(&state) {
        let inventory = state.inventory.lock().unwrap().clone();
        state.live.snapshot(&inventory)
    } else {
        let mut demo = state.demo.lock().unwrap();
        if advance {
            demo.advance();
        }
        demo.snapshot()
    };
    // ── P4 world ──
    let mut snap = routeros::overlay(app, snap);
    // ── r2-fabric ──
    fabric::on_publish(app, &mut snap);
    // ── p1r3-foot ── the windows get each node's busiest processes; `latest` keeps every one
    // (put back even if the emit panics).
    footprint::publish_with(
        &mut snap,
        |s| {
            // ── R2-OPS ── power states SparkBox declared (P1.14), before the alerting sees the snapshot.
            state.ops.power_overlay(s);
            // ── p1r3-check ── each node's pending updates (P1.20), before the alert rules read them.
            ops::update_check::overlay(s);
            // ── P5 monitoring ──
            health::on_publish(app, s);
            s.versions = Some(versions::fleet(&s.nodes));
        },
        |s| {
            let _ = app.emit(SNAPSHOT_EVENT, s);
        },
    );
    *state.latest.write().unwrap() = Some(snap);
}

pub(crate) fn is_live(state: &AppState) -> bool {
    data_source(state) == DataSource::Live
}

/// The data source in use: SPARKBOX_DATA_SOURCE for this run when set, else prefs.json's.
pub(crate) fn data_source(state: &AppState) -> DataSource {
    state.source_override.unwrap_or_else(|| state.prefs.lock().unwrap().data_source)
}

/// Debug builds only: SPARKBOX_DATA_SOURCE=live|demo picks the data source for this run without
/// saving prefs and without asking for Touch ID (the look-only harness, README "Live, look only").
fn data_source_override() -> Option<DataSource> {
    if !cfg!(debug_assertions) {
        return None;
    }
    match std::env::var("SPARKBOX_DATA_SOURCE").ok()?.as_str() {
        "live" => Some(DataSource::Live),
        "demo" => Some(DataSource::Demo),
        _ => None,
    }
}

/// `prefs` as the UI should see them: with the data source actually in use.
fn effective_prefs(state: &AppState, mut prefs: Prefs) -> Prefs {
    if let Some(source) = state.source_override {
        prefs.data_source = source;
    }
    prefs
}

const SOURCE_PINNED: &str = "SPARKBOX_DATA_SOURCE sets the data source for this run. Quit and start SparkBox without it to switch.";

/// Starts or stops the SSH connections to match the chosen data source.
fn apply_data_source(app: &AppHandle) {
    let state = app.state::<AppState>();
    // New activity log rows carry the source they were recorded under.
    state.live.audit.set_demo(!is_live(&state));
    // App Nap is held off while Live, so the 1 s collector loop isn't coalesced (P1.12).
    awake::set_live(is_live(&state));
    if is_live(&state) {
        let nodes = state.inventory.lock().unwrap().nodes.clone();
        state.live.start(&nodes);
        // ── R4-UPD ── upgrades left running or lost are followed again (one follower per node).
        state.ops.reattach_upgrades();
    } else {
        state.live.stop();
    }
}

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> Option<ClusterSnapshot> {
    state.latest.read().unwrap().clone()
}

#[tauri::command]
fn get_inventory(state: State<AppState>) -> Inventory {
    state.inventory.lock().unwrap().clone()
}

#[tauri::command]
fn save_inventory(app: AppHandle, state: State<AppState>, inventory: Inventory) -> Result<(), String> {
    admin::require_mac(&app)?;
    let reconnect = inventory.requires_reconnect(&state.inventory.lock().unwrap());
    let what = inventory_change(&state.inventory.lock().unwrap(), &inventory);
    inventory.save(&state.inventory_path)?;
    if reconnect {
        let mut demo = state.demo.lock().unwrap();
        let scenario = demo.scenario();
        *demo = DemoCluster::new(&inventory);
        let _ = demo.set_scenario(scenario.as_deref());
    }
    *state.inventory.lock().unwrap() = inventory;
    state.live.audit.record_app("Inventory saved", &what);
    if reconnect {
        apply_data_source(&app);
    }
    publish(&app, false);
    Ok(())
}

/// "4 nodes, 1 switch · added cell5 · removed gx10-0000 · cell2 192.168.50.42 → .43" for the log.
fn inventory_change(old: &Inventory, new: &Inventory) -> String {
    let mut parts = vec![format!("{} nodes, {} switches", new.nodes.len(), new.switches.len())];
    for n in &new.nodes {
        match old.nodes.iter().find(|o| o.id == n.id) {
            None => parts.push(format!("added {}", n.name)),
            Some(o) => {
                let addr = |x: &inventory::NodeConfig| format!("{}:{}", x.ip.as_deref().unwrap_or(&x.host), x.port);
                if addr(o) != addr(n) {
                    parts.push(format!("{} {} → {}", n.name, addr(o), addr(n)));
                }
                if o.name != n.name {
                    parts.push(format!("renamed {} → {}", o.name, n.name));
                }
                if o.fabric != n.fabric {
                    parts.push(format!("{} fabric and role expectations updated", n.name));
                }
            }
        }
    }
    parts.extend(old.nodes.iter().filter(|o| !new.nodes.iter().any(|n| n.id == o.id)).map(|o| format!("removed {}", o.name)));
    parts.extend(new.switches.iter().filter(|s| !old.switches.iter().any(|o| o.id == s.id)).map(|s| format!("added {}", s.name)));
    parts.extend(old.switches.iter().filter(|o| !new.switches.iter().any(|s| s.id == o.id)).map(|o| format!("removed {}", o.name)));
    parts.join(" · ")
}

#[tauri::command]
fn prefs_get(state: State<AppState>) -> Prefs {
    effective_prefs(&state, state.prefs.lock().unwrap().clone())
}

/// After new prefs are saved: the model settings' change goes in the activity log, a new data
/// source takes effect, and the window gets a fresh snapshot.
fn prefs_saved(app: &AppHandle, state: &AppState, old: &Prefs, new: &Prefs) {
    if !same_models(old, new) {
        let (a, b) = (&old.models, &new.models);
        let mut changes = Vec::new();
        if a.model_root != b.model_root {
            changes.push(format!("Model folder {} → {}", a.model_root, b.model_root));
        }
        if a.image != b.image {
            changes.push(format!("vLLM image {} → {}", a.image, b.image));
        }
        if a.advisor_provider != b.advisor_provider || a.advisor_model != b.advisor_model {
            changes.push(format!("Advisor {}/{} → {}/{}", a.advisor_provider, a.advisor_model, b.advisor_provider, b.advisor_model));
        }
        state.live.audit.record_app("Model settings changed", &changes.join(" · "));
    }
    if old.hardware_auto_lookup != new.hardware_auto_lookup {
        state.live.audit.record_app("Automatic hardware lookup changed", if new.hardware_auto_lookup { "Enabled: uses the configured AI advisor and public manufacturer pages" } else { "Disabled" });
    }
    if old.data_source != new.data_source {
        data_source_changed(app, state, new.data_source);
    }
    publish(app, false);
}

/// Changes some preferences, under one lock: the window sends only what it changes, so two
/// windows saving at once don't undo each other. A patch
/// that touches the data source or the model settings needs Mac scope, decided from the
/// patch itself.
#[tauri::command]
fn prefs_patch(app: AppHandle, state: State<AppState>, patch: serde_json::Value) -> Result<Prefs, String> {
    if Prefs::patch_needs_mac(&patch) {
        admin::require_mac(&app)?;
    }
    if state.source_override.is_some() && patch.get("dataSource").is_some() {
        return Err(SOURCE_PINNED.into());
    }
    let (old, new) = {
        let mut prefs = state.prefs.lock().unwrap();
        let new = prefs.merged(&patch)?;
        new.save(&state.prefs_path)?;
        let old = std::mem::replace(&mut *prefs, new.clone());
        (old, new)
    };
    prefs_saved(&app, &state, &old, &new);
    Ok(effective_prefs(&state, new))
}

fn same_models(a: &Prefs, b: &Prefs) -> bool {
    serde_json::to_value(&a.models).ok() == serde_json::to_value(&b.models).ok()
}

/// Switches between the simulated cluster and the real one.
#[tauri::command]
fn data_source_set(app: AppHandle, state: State<AppState>, source: DataSource) -> Result<Prefs, String> {
    admin::require_mac(&app)?;
    if state.source_override.is_some() {
        return Err(SOURCE_PINNED.into());
    }
    let mut prefs = state.prefs.lock().unwrap().clone();
    if prefs.data_source == source {
        return Ok(prefs);
    }
    prefs.data_source = source;
    prefs.save(&state.prefs_path)?;
    *state.prefs.lock().unwrap() = prefs.clone();
    data_source_changed(&app, &state, source);
    publish(&app, false);
    Ok(prefs)
}

fn data_source_changed(app: &AppHandle, state: &AppState, source: DataSource) {
    let live = source == DataSource::Live;
    state.live.audit.record_app("Data source changed", if live { "Demo → Live" } else { "Live → Demo" });
    let gate = app.state::<Arc<admin::AdminGate>>();
    let started = gate.state().unlocked_at_ms;
    if gate.set_live(live) {
        let since = started.map(|ms| format!(" · unlocked at {}", live::audit::iso_time(ms))).unwrap_or_default();
        state.live.audit.record_app("Admin mode locked", &format!("Live data asks for Touch ID or the Mac password{since}"));
    }
    admin::emit(app);
    apply_data_source(app);
}

// ---- Processes, services, power, Fleet Command

#[tauri::command]
async fn process_signal(
    app: AppHandle,
    state: State<'_, AppState>,
    node: String,
    pid: u32,
    signal: String,
    start_time: Option<u64>,
) -> Result<(), SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.signal(demo, &node, pid, &signal, start_time).await?;
    publish(&app, false);
    Ok(())
}

#[tauri::command]
async fn process_renice(app: AppHandle, state: State<'_, AppState>, node: String, pid: u32, nice: i32) -> Result<(), SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.renice(demo, &node, pid, nice).await?;
    publish(&app, false);
    Ok(())
}

#[tauri::command]
async fn services_list(state: State<'_, AppState>, node: String) -> Result<Vec<Service>, String> {
    let demo = !is_live(&state);
    state.ops.services(demo, &node).await
}

#[tauri::command]
async fn service_action(app: AppHandle, state: State<'_, AppState>, node: String, unit: String, action: String) -> Result<(), SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.service_action(demo, &node, &unit, &action).await?;
    publish(&app, false);
    Ok(())
}

#[tauri::command]
async fn service_status(state: State<'_, AppState>, node: String, unit: String) -> Result<String, String> {
    let demo = !is_live(&state);
    state.ops.service_status(demo, &node, &unit).await
}

/// Restarts or shuts down a node ("reboot" | "poweroff", now or in a few minutes), or wakes a
/// switched-off one ("wake", Wake-on-LAN). Each runs as a task that follows the node through
/// and, for restart and wake, checks its models answer again.
#[tauri::command]
async fn node_power(app: AppHandle, state: State<'_, AppState>, node: String, action: String, delay_min: Option<u32>) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    if action == "wake" {
        let (handle, id) = (app.clone(), node.clone());
        return state.ops.wake(demo, &node, Box::pin(async move { wol::send(&handle, &id).await.map(|route| route.describe()) }));
    }
    state.ops.power(demo, &node, &action, delay_min.unwrap_or(0))
}

#[tauri::command]
async fn fleet_run(app: AppHandle, state: State<'_, AppState>, nodes: Vec<String>, command: String, sudo: bool, timeout_s: u64) -> Result<Vec<FleetResult>, String> {
    admin::require(&app)?;
    let demo = !is_live(&state);
    Ok(state.ops.fleet(demo, &nodes, &command, sudo, timeout_s).await)
}

/// Opens a web page on the node (the DGX Dashboard, or any port in the node's listener list but
/// SSH and DNS; `tunnels::target`) through SSH; returns its local URL, which carries the
/// tunnel's one-time token.
#[tauri::command]
async fn tunnel_open(app: AppHandle, state: State<'_, AppState>, node: String, port: u16) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    let what = tunnels::page_label(port);
    if !is_live(&state) {
        return Err(SbError::new(errors::ErrorKind::DemoOnly, "In Demo mode there's no node to connect to. Switch to Live data to open it through an SSH tunnel.").on(&node));
    }
    let opened = state.tunnels.open(&node, port).await;
    match &opened {
        Ok(t) => state.live.audit.record_app(&format!("Opened the {what} of {node}"), &format!("SSH tunnel 127.0.0.1:{} → {node} {}:{port} · fence class {}", t.local_port, t.host, t.fence_class)),
        Err(e) => state.live.audit.record_failed(&node, &format!("Open the {what} of {node}"), &format!("SSH tunnel to port {port}"), e),
    };
    Ok(opened.map_err(|e| SbError::classify(&e).on(&node))?.url())
}

/// Closes a node page's tunnel (Close in Settings › Security & privacy).
#[tauri::command]
fn tunnel_close(state: State<AppState>, node: String, port: u16) {
    if state.tunnels.close(&node, port) {
        let what = tunnels::page_label(port);
        state.live.audit.record_app(&format!("Closed the {what} tunnel to {node}"), "Closed from Settings › Security & privacy");
    }
}

/// The SSH tunnels open now, for Settings › Security & privacy.
#[tauri::command]
fn tunnels_list(state: State<AppState>) -> Vec<tunnels::TunnelView> {
    state.tunnels.list()
}

#[tauri::command]
fn activity_log(state: State<AppState>) -> Vec<live::audit::Entry> {
    // Only the data source in use: Demo rows never show in Live (nor in Endpoint History).
    state.live.audit.entries_for(!is_live(&state))
}

#[tauri::command]
fn activity_log_path(state: State<AppState>) -> Option<String> {
    state.live.audit.path().map(|p| p.display().to_string())
}

/// Entries added or changed since `ts_ms` (newest first), for the window's incremental refresh.
#[tauri::command]
fn activity_log_since(state: State<AppState>, ts_ms: u64, limit: Option<usize>) -> Vec<live::audit::Entry> {
    state.live.audit.since_for(!is_live(&state), ts_ms, limit.unwrap_or(2000).min(5000))
}

/// Saves the activity log from `from_ms` to `to_ms` (all of it when left out), read from the
/// files on disk, to Downloads as CSV or JSON and shows it in Finder.
#[tauri::command]
fn activity_export(app: AppHandle, state: State<AppState>, format: String, from_ms: Option<u64>, to_ms: Option<u64>) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt;
    let text = state.live.audit.export(&format, from_ms.unwrap_or(0), to_ms.unwrap_or(u64::MAX), Some(!is_live(&state)))?;
    let stamp = live::audit::iso_time(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0))
        .replace([':', 'T'], "-")
        .trim_end_matches('Z')
        .to_string();
    let path = app.path().download_dir().map_err(|e| e.to_string())?.join(format!("SparkBox-activity-{stamp}.{format}"));
    util::write_atomic(&path, text.as_bytes())?;
    let _ = app.opener().reveal_item_in_dir(&path);
    Ok(path.display().to_string())
}

/// Debug builds only: lets the $SPARKBOX_DEV_SCRIPT JavaScript report back, one line appended to
/// "<script>.out" per call. Release builds ignore it.
#[tauri::command]
fn dev_report(text: String) {
    #[cfg(debug_assertions)]
    if let Ok(path) = std::env::var("SPARKBOX_DEV_SCRIPT") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{path}.out")) {
            let _ = writeln!(f, "{text}");
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = text;
}

// ---- Files

/// Lists a folder. A folder of keys and credentials needs Mac scope: on this Mac ~/.ssh, the
/// Keychains and the rest of mac.rs's SECRET_DIRS; on a node its secrets (~/.ssh, ~/hf_token.txt,
/// ~/glm53/api_key…, mac::is_node_secret).
#[tauri::command]
async fn files_list(app: AppHandle, state: State<'_, AppState>, place: String, path: String) -> Result<Listing, String> {
    if state.files.is_secret(&place, &path) {
        admin::require_mac(&app)?;
    }
    let demo = !is_live(&state);
    state.files.list(demo, &place, &path).await
}

/// Reads a text file for the editor; one in a key folder, on this Mac or a node, needs Mac scope.
#[tauri::command]
async fn files_read(app: AppHandle, state: State<'_, AppState>, place: String, path: String) -> Result<TextFile, String> {
    if state.files.is_secret(&place, &path) {
        admin::require_mac(&app)?;
    }
    let demo = !is_live(&state);
    state.files.read(demo, &place, &path).await
}

/// The editor's Read with sudo…, for a file only root can read: node scope, since it runs a
/// command as root on the node (the password goes to sudo on stdin).
#[tauri::command]
async fn files_read_sudo(app: AppHandle, state: State<'_, AppState>, place: String, path: String) -> Result<TextFile, String> {
    admin::require(&app)?;
    let demo = !is_live(&state);
    state.files.read_sudo(demo, &place, &path).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn files_write(
    app: AppHandle,
    state: State<'_, AppState>,
    place: String,
    path: String,
    content: String,
    sudo: bool,
    expect_sha256: Option<String>,
) -> Result<Saved, SaveError> {
    admin::require_for_place(&app, &place)?;
    let demo = !is_live(&state);
    state.files.write(demo, &place, &path, &content, sudo, expect_sha256.as_deref()).await
}

#[tauri::command]
async fn files_mkdir(app: AppHandle, state: State<'_, AppState>, place: String, dir: String, name: String) -> Result<(), String> {
    admin::require_for_place(&app, &place)?;
    let demo = !is_live(&state);
    state.files.mkdir(demo, &place, &dir, &name).await
}

#[tauri::command]
async fn files_rename(app: AppHandle, state: State<'_, AppState>, place: String, path: String, new_name: String) -> Result<(), String> {
    admin::require_for_place(&app, &place)?;
    let demo = !is_live(&state);
    state.files.rename(demo, &place, &path, &new_name).await
}

#[tauri::command]
async fn files_delete(app: AppHandle, state: State<'_, AppState>, place: String, paths: Vec<String>) -> Result<(), SbError> {
    admin::require_for_place(&app, &place)?;
    let demo = !is_live(&state);
    Ok(state.files.delete(demo, &place, &paths).await?)
}

#[tauri::command]
async fn files_chmod(app: AppHandle, state: State<'_, AppState>, place: String, path: String, mode: u32) -> Result<(), String> {
    admin::require_for_place(&app, &place)?;
    let demo = !is_live(&state);
    state.files.chmod(demo, &place, &path, mode).await
}

/// Stops a transfer. Cancelling needs Admin mode like starting one did (node scope; the
/// destination keeps what it had, since a copy is only put in place once verified).
#[tauri::command]
fn files_cancel(app: AppHandle, state: State<AppState>, id: String) -> Result<(), String> {
    admin::require(&app)?;
    state.files.cancel(&id);
    Ok(())
}

// ---- Terminals

#[tauri::command]
async fn terminal_open(
    app: AppHandle,
    state: State<'_, AppState>,
    node: String,
    target: Target,
    cols: u32,
    rows: u32,
    on_event: Channel<TermEvent>,
) -> Result<String, String> {
    if target.is_shell() {
        admin::require(&app)?;
    }
    let demo = !is_live(&state);
    // A shell opens unauthorised and is authorised once it's there, only if Admin mode didn't
    // relock meanwhile (a slow connection mustn't outlast a lock).
    let gate = app.state::<Arc<admin::AdminGate>>().inner().clone();
    let target = named_target(&state, &node, target);
    let emit: Emit = Box::new(move |e| {
        let _ = on_event.send(e);
    });
    state.terminals.open_gated(&gate, demo, &node, target, cols, rows, emit).await
}

/// The same target with a container id from the Terminal menu replaced by the container's
/// name, so the tab, the activity log and docker all say "vllm-head".
fn named_target(state: &AppState, node: &str, target: Target) -> Target {
    let name = |id: &str| {
        let latest = state.latest.read().unwrap();
        let n = latest.as_ref()?.nodes.iter().find(|n| n.id == node)?;
        n.containers.iter().find(|c| c.id == id || (id.len() >= 12 && c.id.starts_with(id))).map(|c| c.name.clone()).filter(|n| !n.is_empty())
    };
    match target {
        Target::ContainerShell { container } => Target::ContainerShell { container: name(&container).unwrap_or(container) },
        Target::ContainerLogs { container, since } => Target::ContainerLogs { container: name(&container).unwrap_or(container), since },
        other => other,
    }
}

#[tauri::command]
fn terminal_input(state: State<AppState>, id: String, data: String) -> Result<(), String> {
    // Typing into a shell is a change too: a locked shell (the owner locked, or nobody typed for
    // its idle time) refuses keystrokes here, not only in the UI. No per-keystroke gate check
    // or event: a keystroke stores its time in the session's atomic.
    if state.terminals.is_shell(&id) && !state.terminals.shell_authorized(&id) {
        return Err(errors::SbError::new(errors::ErrorKind::Locked, term::LOCKED_INPUT).to_json());
    }
    state.terminals.input(&id, data.into_bytes())
}

#[tauri::command]
fn terminal_resize(state: State<AppState>, id: String, cols: u32, rows: u32) -> Result<(), String> {
    state.terminals.resize(&id, cols, rows)
}

#[tauri::command]
fn terminal_close(state: State<AppState>, id: String) {
    state.terminals.close(&id)
}

/// xterm has drawn `bytes` more of a terminal's output (flow control, see term/mod.rs).
#[tauri::command]
fn terminal_ack(state: State<AppState>, id: String, bytes: u64) {
    state.terminals.ack(&id, bytes)
}

// ---- Models

/// What the Models calls need from the app's state, taken without holding locks across awaits.
fn models_ctx(state: &AppState) -> Ctx {
    Ctx {
        live: is_live(state),
        settings: state.prefs.lock().unwrap().models.clone(),
        inventory: state.inventory.lock().unwrap().clone(),
        snapshot: state.latest.read().unwrap().clone(),
    }
}

#[tauri::command]
async fn hf_search(state: State<'_, AppState>, query: String) -> Result<Vec<SearchHit>, String> {
    state.models.search(&query).await
}

#[tauri::command]
async fn hf_model(state: State<'_, AppState>, repo: String) -> Result<ModelDetails, String> {
    let ctx = models_ctx(&state);
    state.models.details(&ctx, &repo).await
}

#[tauri::command]
async fn models_analyze(state: State<'_, AppState>, repo: String) -> Result<AnalysisResult, String> {
    let ctx = models_ctx(&state);
    state.models.analyze(&ctx, &repo).await
}

#[tauri::command]
async fn library_list(state: State<'_, AppState>) -> Result<Vec<LibraryEntry>, String> {
    let ctx = models_ctx(&state);
    state.models.library(&ctx).await
}

#[tauri::command]
async fn jobs_list(state: State<'_, AppState>) -> Result<Vec<Job>, String> {
    let ctx = models_ctx(&state);
    state.models.jobs(&ctx).await
}

#[tauri::command]
async fn job_cancel(app: AppHandle, state: State<'_, AppState>, node: String, id: String) -> Result<(), String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.cancel_job(&ctx, &node, &id).await
}

#[tauri::command]
async fn job_resume_download(app: AppHandle, state: State<'_, AppState>, node: String, id: String, expected: models::DownloadResume, review: models::DownloadToolReview) -> Result<Job, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.resume_download(&ctx, &node, &id, &expected, &review).await
}

#[tauri::command]
async fn model_disk_preflight(state: State<'_, AppState>, request: models::DiskRequest) -> Result<Vec<models::DiskCheck>, String> {
    state.models.disk_preflight(&models_ctx(&state), &request).await
}

#[tauri::command]
async fn model_download_tools(state: State<'_, AppState>, node: String) -> Result<models::DownloadTools, String> {
    state.models.download_tools(&models_ctx(&state), &node).await
}

#[tauri::command]
async fn model_download(app: AppHandle, state: State<'_, AppState>, repo: String, review: models::DownloadToolReview) -> Result<Job, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.download(&ctx, &repo, &review).await
}

#[tauri::command]
async fn model_share(app: AppHandle, state: State<'_, AppState>, key: String, targets: Vec<String>) -> Result<Vec<Job>, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.share(&ctx, &key, &targets).await
}

#[tauri::command]
async fn model_delete(app: AppHandle, state: State<'_, AppState>, key: String, nodes: Vec<String>) -> Result<(), String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.delete(&ctx, &key, &nodes).await
}

/// Models › Check nodes. A change, not a read: it starts a short-lived container on each node
/// to try the GPU and the image.
#[tauri::command]
async fn cluster_check(app: AppHandle, state: State<'_, AppState>, image: Option<String>, nodes: Option<Vec<String>>) -> Result<Vec<NodeCheck>, String> {
    admin::require(&app)?;
    let mut ctx = models_ctx(&state);
    if let Some(image) = image {
        if !models::plan::is_image_ref(&image) { return Err("Invalid image reference.".into()); }
        ctx.settings.image = image;
    }
    if let Some(nodes) = nodes {
        if nodes.is_empty() || nodes.iter().any(|id| !ctx.inventory.nodes.iter().any(|n| n.id == *id)) { return Err("Pick known nodes to check.".into()); }
        ctx.inventory.nodes.retain(|n| nodes.contains(&n.id));
    }
    Ok(state.models.check(&ctx).await)
}

#[tauri::command]
fn deploy_preview(state: State<AppState>, spec: DeploySpec) -> Result<Vec<PreviewStep>, String> {
    state.models.preview(&models_ctx(&state), &spec)
}

#[tauri::command]
async fn deploy(app: AppHandle, state: State<'_, AppState>, spec: DeploySpec) -> Result<Deployment, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.deploy(&ctx, spec).await
}

#[tauri::command]
fn deployments_list(state: State<AppState>) -> Vec<Deployment> {
    state.models.deployments(&models_ctx(&state))
}

/// Local evidence only: never routes archived records into Models' active list.
#[tauri::command]
async fn deployment_history_list(state: State<'_, AppState>) -> Result<models::history_view::View, String> {
    let models = state.models.clone();
    let live = is_live(&state);
    tauri::async_runtime::spawn_blocking(move || models.history_view(live)).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn deployment_history_file(state: State<'_, AppState>, name: String, bytes: Vec<u8>) -> Result<models::history_view::Source, String> {
    if !is_live(&state) { return Err("Demo shows samples only. Choose Live to view a local report file.".into()); }
    tauri::async_runtime::spawn_blocking(move || models::history_view::preview(name, &bytes)).await.map_err(|e| e.to_string())?
}

/// Unloads a load as a task (stop, then check what came back); returns the task's id.
#[tauri::command]
async fn deployment_unload(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<String, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.unload(&ctx, &id).await
}

#[tauri::command]
async fn deployment_resume(app: AppHandle, state: State<'_, AppState>, id: String, drop_caches_once: Option<bool>) -> Result<(), String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    live::audit::with_op_id(format!("resume-{id}"), live::audit::OpMeta { title: format!("Resume interrupted load {id}"), source: "ui".into() },
        state.models.resume_partial(&ctx, &id, drop_caches_once.unwrap_or(false))).await
}

/// Forgets a failed or unloaded load; one whose containers are still there is unloaded first
/// (the unload's task id comes back).
#[tauri::command]
async fn deployment_forget(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<Option<String>, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.forget_checked(&ctx, &id).await
}

#[tauri::command]
fn benchmarks_list(state: State<AppState>) -> Vec<Benchmark> {
    state.models.benchmarks()
}

/// Collects diagnostics from every node into a text file in Downloads; returns its path.
#[tauri::command]
async fn doctor_run(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    // The inventory holds the real nodes in Demo too: the doctor logs in to them only with Live data.
    if !is_live(&state) {
        return Err("Available with Live data: the doctor reads the real nodes.".into());
    }
    let nodes = state.inventory.lock().unwrap().nodes.clone();
    let source = data_source(&state);
    let vpn = state.vpn.view().await.status;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let header = format!(
        "SparkBox {} · unix time {stamp}\nData source: {source:?}\nVPN: {:?}{}\nNodes: {}",
        app.package_info().version,
        vpn.state,
        vpn.message.map(|m| format!(" ({m})")).unwrap_or_default(),
        nodes.iter().map(|n| format!("{} {}:{}", n.name, n.ip.as_deref().unwrap_or(&n.host), n.port)).collect::<Vec<_>>().join(", "),
    );
    // The sudo-only line (sshd -T, P1.13) only while unlocked and not look-only; it still goes
    // through the write exit as a sudo check, so the test build needs a fence that allows those too.
    let gate = app.state::<Arc<admin::AdminGate>>();
    let sudo = !gate.look_only() && gate.state().unlocked;
    let report = state.live.doctor_with(&nodes, &header, sudo).await;
    let path = app.path().download_dir().map_err(|e| e.to_string())?.join(format!("SparkBox-doctor-{stamp}.txt"));
    std::fs::write(&path, report).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Forgets every pinned SSH host key (after reinstalling a node).
#[tauri::command]
fn known_hosts_forget(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    admin::require_mac(&app)?;
    live::forget_host_keys(&state.known_hosts_path)?;
    state.live.audit.record_app("Forgot the saved SSH host keys", "Each node waits for you to trust it again, unless ~/.ssh/known_hosts already holds that exact key");
    Ok(())
}

#[tauri::command]
async fn credential_status(accounts: Vec<String>) -> Result<HashMap<String, bool>, String> {
    // The Keychain can block (a first read asks macOS): off the async workers.
    tauri::async_runtime::spawn_blocking(move || accounts.into_iter().map(|a| secrets::get(&a).map(|s| (a, s.is_some()))).collect())
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_credential(app: AppHandle, state: State<'_, AppState>, account: String, secret: String) -> Result<(), String> {
    admin::require_mac_confirmed(&app, "set_credential")?;
    let label = credential_label(&state, &account)?;
    let key = account.clone();
    tauri::async_runtime::spawn_blocking(move || secrets::set(&key, &secret)).await.map_err(|e| e.to_string())??;
    if account == "ssh" {
        // A new password gets a fresh chance with sudo.
        state.live.reset_sudo_breaker();
    }
    if account == firewalla::TOKEN_ACCOUNT {
        app.state::<Arc<firewalla::Firewalla>>().token_changed(true, firewalla::commands::now_ms());
    }
    state.live.audit.record_app("Password saved", &label);
    Ok(())
}

#[tauri::command]
async fn clear_credential(app: AppHandle, state: State<'_, AppState>, account: String) -> Result<(), String> {
    admin::require_mac_confirmed(&app, "clear_credential")?;
    let label = credential_label(&state, &account)?;
    let key = account.clone();
    tauri::async_runtime::spawn_blocking(move || secrets::clear(&key)).await.map_err(|e| e.to_string())??;
    if account == firewalla::TOKEN_ACCOUNT {
        app.state::<Arc<firewalla::Firewalla>>().token_changed(false, firewalla::commands::now_ms());
    }
    state.live.audit.record_app("Password removed", &label);
    Ok(())
}

/// The Keychain entries the UI may change: the cluster password, the Hugging Face token, the
/// Firewalla MSP token and one RouterOS password per switch in the inventory. Returns the name
/// the log shows.
fn credential_label(state: &AppState, account: &str) -> Result<String, String> {
    let label = match account.strip_prefix("routeros:") {
        Some(id) => {
            let inventory = state.inventory.lock().unwrap();
            let switch = inventory.switches.iter().find(|s| s.id == id).ok_or_else(|| format!("There's no switch “{id}” in the inventory."))?;
            format!("RouterOS password for {}", switch.name)
        }
        None => named_credential(account).ok_or_else(|| format!("SparkBox doesn't keep a password called “{account}”."))?.into(),
    };
    Ok(label)
}

/// The Keychain entries with fixed names, as the log shows them.
fn named_credential(account: &str) -> Option<&'static str> {
    match account {
        "ssh" => Some("Cluster password (SSH and sudo)"),
        "hf" => Some("Hugging Face token"),
        firewalla::TOKEN_ACCOUNT => Some("Firewalla MSP token"),
        _ => None,
    }
}

/// `docker inspect` with the values of secret-looking environment variables masked.
#[tauri::command]
async fn container_inspect(state: State<'_, AppState>, node_id: String, container_id: String) -> Result<serde_json::Value, String> {
    let mut value = inspect_raw(&state, &node_id, &container_id).await?;
    ops::containers::mask_env(&mut value);
    Ok(value)
}

async fn inspect_raw(state: &AppState, node_id: &str, container_id: &str) -> Result<serde_json::Value, String> {
    if is_live(state) {
        state.live.container_inspect(node_id, container_id).await
    } else {
        state.demo.lock().unwrap().container_inspect(node_id, container_id)
    }
}

/// One masked environment value, in full: needs node scope and is logged ("Revealed HF_TOKEN
/// of vllm-head").
#[tauri::command]
async fn container_reveal(app: AppHandle, state: State<'_, AppState>, node_id: String, container_id: String, name: String) -> Result<String, String> {
    admin::require(&app)?;
    let value = inspect_raw(&state, &node_id, &container_id).await?;
    let found = ops::containers::env_value(&value, &name).ok_or_else(|| format!("{name} isn't set in this container."))?;
    let container = ops::containers::inspect_name(&value).unwrap_or(container_id.clone());
    state.live.audit.record_app(&format!("Revealed {name} of {container}"), &format!("on {node_id}"));
    Ok(found)
}

#[tauri::command]
async fn container_action(
    app: AppHandle,
    state: State<'_, AppState>,
    node_id: String,
    container_id: String,
    action: String,
    remove_volumes: Option<bool>,
) -> Result<(), SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.container_action(demo, &node_id, &container_id, &action, remove_volumes.unwrap_or(false)).await?;
    publish(&app, false);
    Ok(())
}

#[tauri::command]
async fn vpn_get(state: State<'_, AppState>) -> Result<VpnView, String> {
    Ok(state.vpn.view().await)
}

#[tauri::command]
async fn vpn_import(app: AppHandle, state: State<'_, AppState>, conf_text: String, endpoint_host: String) -> Result<VpnView, String> {
    admin::require_mac_confirmed(&app, "vpn_import")?;
    let view = state.vpn.import(&conf_text, &endpoint_host).await?;
    state.live.audit.record_app("VPN profile imported", &format!("Endpoint {endpoint_host}"));
    Ok(view)
}

#[tauri::command]
async fn vpn_set_endpoint(app: AppHandle, state: State<'_, AppState>, host: String) -> Result<VpnView, String> {
    admin::require_mac(&app)?;
    let old = state.vpn.view().await;
    let view = state.vpn.set_endpoint(&host).await?;
    state.live.audit.record_app("VPN endpoint changed", &format!("{} → {}", json_field(&old, "/settings/profile/endpointHost"), json_field(&view, "/settings/profile/endpointHost")));
    Ok(view)
}

/// `home_pin` also sets "Direct at home" (P1.7), under the same gate and prompt ("change when
/// the VPN connects"); the VPN window sends it with the mode unchanged.
#[tauri::command]
async fn vpn_set_mode(app: AppHandle, state: State<'_, AppState>, mode: VpnMode, home_pin: Option<bool>) -> Result<VpnView, String> {
    admin::require_mac(&app)?;
    let old = state.vpn.view().await;
    if let Some(on) = home_pin {
        let view = state.vpn.set_home_pin(on).await?;
        let word = |on: bool| if on { "on" } else { "off" };
        state.live.audit.record_app("VPN Direct at home changed", &format!("{} → {}", word(old.settings.home_pin), word(on)));
        if mode == old.settings.mode {
            return Ok(view);
        }
    }
    let view = state.vpn.set_mode(mode).await?;
    state.live.audit.record_app("VPN mode changed", &format!("{} → {}", json_field(&old, "/settings/mode"), json_field(&view, "/settings/mode")));
    Ok(view)
}

#[tauri::command]
async fn vpn_set_forwards(app: AppHandle, state: State<'_, AppState>, forwards: Vec<Forward>) -> Result<VpnView, String> {
    admin::require_mac(&app)?;
    let n = forwards.len();
    let view = state.vpn.set_forwards(forwards).await?;
    state.live.audit.record_app("VPN port forwards changed", &format!("{n} forward{}", if n == 1 { "" } else { "s" }));
    Ok(view)
}

/// A field of a view (a JSON pointer) as plain text for the log's "old → new" ("—" when missing).
fn json_field(view: &impl serde::Serialize, pointer: &str) -> String {
    match serde_json::to_value(view).ok().and_then(|v| v.pointer(pointer).cloned()) {
        Some(serde_json::Value::String(s)) if !s.is_empty() => s,
        Some(serde_json::Value::Null) | None => "—".into(),
        Some(v) => v.to_string(),
    }
}

#[tauri::command]
async fn vpn_reconnect(state: State<'_, AppState>) -> Result<VpnView, String> {
    Ok(state.vpn.reconnect().await)
}

#[tauri::command]
async fn vpn_remove(app: AppHandle, state: State<'_, AppState>) -> Result<VpnView, String> {
    admin::require_mac_confirmed(&app, "vpn_remove")?;
    let view = state.vpn.remove().await?;
    state.live.audit.record_app("VPN profile removed", "");
    Ok(view)
}

#[tauri::command]
async fn providers_list(state: State<'_, AppState>) -> Result<Vec<ProviderView>, String> {
    Ok(state.llm.views().await)
}

#[tauri::command]
async fn providers_upsert(app: AppHandle, state: State<'_, AppState>, provider: Provider) -> Result<Vec<ProviderView>, String> {
    admin::require_mac(&app)?;
    let old = state.llm.views().await.into_iter().find(|v| json_field(v, "/id") == provider.id);
    let (name, url) = (json_field(&provider, "/name"), json_field(&provider, "/baseUrl"));
    // https unless it's this Mac, a cluster node or a ticked home address; a new address drops the old key.
    let hosts: Vec<String> = state.inventory.lock().unwrap().nodes.iter().flat_map(|n| [n.ip.clone().unwrap_or_default(), n.host.clone()]).filter(|h| !h.is_empty()).collect();
    let saved = state.llm.upsert_checked(provider, &hosts).await?;
    let detail = match old {
        Some(o) => format!("{} {} → {} {}", json_field(&o, "/name"), json_field(&o, "/baseUrl"), name, url),
        None => format!("{name} {url}"),
    };
    let detail = if saved.key_cleared { format!("{detail} (its saved key was removed)") } else { detail };
    state.live.audit.record_app("Provider saved", &detail);
    Ok(state.llm.views().await)
}

#[tauri::command]
async fn providers_remove(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<Vec<ProviderView>, String> {
    admin::require_mac(&app)?;
    state.llm.remove(&id).await?;
    state.live.audit.record_app("Provider removed", &id);
    Ok(state.llm.views().await)
}

#[tauri::command]
async fn providers_set_key(app: AppHandle, state: State<'_, AppState>, id: String, key: String) -> Result<Vec<ProviderView>, String> {
    admin::require_mac_confirmed(&app, "providers_set_key")?;
    let account = format!("llm:{id}");
    let key = key.trim().to_string();
    tauri::async_runtime::spawn_blocking(move || secrets::set(&account, &key)).await.map_err(|e| e.to_string())??;
    state.live.audit.record_app("Provider API key saved", &format!("{id} (in the Keychain)"));
    Ok(state.llm.views().await)
}

#[tauri::command]
async fn providers_clear_key(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<Vec<ProviderView>, String> {
    admin::require_mac_confirmed(&app, "providers_clear_key")?;
    let account = format!("llm:{id}");
    tauri::async_runtime::spawn_blocking(move || secrets::clear(&account)).await.map_err(|e| e.to_string())??;
    state.live.audit.record_app("Provider API key removed", &id);
    Ok(state.llm.views().await)
}

#[tauri::command]
async fn providers_refresh(state: State<'_, AppState>, id: String) -> Result<Vec<String>, String> {
    state.llm.refresh_models(&id).await
}

#[tauri::command]
/// Looks for model servers on the nodes and saves each one found as a provider.
async fn providers_scan(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<Provider>, String> {
    admin::require(&app)?;
    let nodes: Vec<(String, String)> = {
        let inventory = state.inventory.lock().unwrap();
        inventory.nodes.iter().filter_map(|n| Some((n.name.clone(), n.ip.clone()?))).collect()
    };
    let found = state.llm.scan(nodes).await?;
    if !found.is_empty() {
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        state.live.audit.record_app("Providers found on the cluster", &names.join(", "));
    }
    Ok(found)
}

#[tauri::command]
async fn chat_send(state: State<'_, AppState>, request: ChatRequest, on_event: Channel<ChatEvent>) -> Result<ChatCompletion, String> {
    let id = request.request_id.clone();
    let (tx, rx) = oneshot::channel();
    state.chats.lock().unwrap().insert(id.clone(), tx);
    let emit = move |e: ChatEvent| {
        let _ = on_event.send(e);
    };
    let completion = state.llm.chat_completion(request, &emit, rx).await;
    state.chats.lock().unwrap().remove(&id);
    Ok(completion)
}

#[tauri::command]
fn chat_stop(state: State<AppState>, request_id: String) {
    if let Some(tx) = state.chats.lock().unwrap().remove(&request_id) {
        let _ = tx.send(());
    }
}

#[tauri::command]
fn conversations_list(state: State<AppState>) -> Vec<ConversationSummary> {
    state.history.list()
}

#[tauri::command]
fn conversation_get(state: State<AppState>, id: String) -> Result<Conversation, String> {
    state.history.get(&id)
}

#[tauri::command]
fn conversation_save(state: State<AppState>, conversation: Conversation) -> Result<(), String> {
    state.history.save(&conversation)
}

#[tauri::command]
fn conversation_delete(state: State<AppState>, id: String) -> Result<(), String> {
    state.history.delete(&id)
}

#[tauri::command]
async fn server_get(state: State<'_, AppState>) -> Result<ServerView, String> {
    Ok(state.server.view().await)
}

/// Tells every window (status bar included) whether the server is running.
fn announce(app: &AppHandle, view: &ServerView) {
    let _ = app.emit("server://status", view);
}

#[tauri::command]
async fn server_save(app: AppHandle, state: State<'_, AppState>, settings: ServerSettings) -> Result<ServerView, String> {
    admin::require_mac(&app)?;
    let old = serde_json::to_value(state.server.view().await).ok().and_then(|v| v.get("settings").cloned());
    let new = serde_json::to_value(&settings).ok();
    let view = state.server.save_settings(settings).await?;
    state.live.audit.record_app("API server settings changed", &settings_diff(old.as_ref(), new.as_ref()));
    announce(&app, &view);
    Ok(view)
}

/// "port 8484 → 8585 · requireKey true → false" for the top-level fields that changed.
fn settings_diff(old: Option<&serde_json::Value>, new: Option<&serde_json::Value>) -> String {
    let (Some(serde_json::Value::Object(a)), Some(serde_json::Value::Object(b))) = (old, new) else { return String::new() };
    let show = |v: Option<&serde_json::Value>| match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "—".into(),
    };
    b.iter().filter(|(k, v)| a.get(*k) != Some(*v)).map(|(k, v)| format!("{k} {} → {}", show(a.get(k)), show(Some(v)))).collect::<Vec<_>>().join(" · ")
}

#[tauri::command]
async fn server_start(app: AppHandle, state: State<'_, AppState>) -> Result<ServerView, String> {
    admin::require_mac(&app)?;
    let view = state.server.start().await;
    match &view {
        Ok(_) => state.live.audit.record_app("API server started", ""),
        Err(e) => state.live.audit.record_failed("mac", "Start the API server", "", e),
    };
    let view = view?;
    announce(&app, &view);
    Ok(view)
}

#[tauri::command]
async fn server_stop(app: AppHandle, state: State<'_, AppState>) -> Result<ServerView, String> {
    admin::require_mac(&app)?;
    let view = state.server.stop().await;
    state.live.audit.record_app("API server stopped", "");
    announce(&app, &view);
    Ok(view)
}

#[tauri::command]
async fn server_create_key(app: AppHandle, state: State<'_, AppState>, name: String) -> Result<String, String> {
    admin::require_mac_confirmed(&app, "server_create_key")?;
    let key = state.server.create_key(&name).await?;
    state.live.audit.record_app("Server API key created", &name);
    Ok(key)
}

#[tauri::command]
async fn server_revoke_key(app: AppHandle, state: State<'_, AppState>, name: String) -> Result<ServerView, String> {
    admin::require_mac_confirmed(&app, "server_revoke_key")?;
    state.server.revoke_key(&name).await?;
    state.live.audit.record_app("Server API key revoked", &name);
    Ok(state.server.view().await)
}

#[tauri::command]
async fn server_self_test(state: State<'_, AppState>, model: String) -> Result<Vec<SelfTestResult>, String> {
    state.server.self_test(&model).await
}

#[tauri::command]
async fn server_logs(state: State<'_, AppState>) -> Result<Vec<LogEntry>, String> {
    Ok(state.server.logs().await)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Under cfg(test) the exit is permissive: no fence, identity pin or lease (live/exit.rs). No
    // test starts the app, so a build compiled that way by hand stops here.
    if cfg!(test) {
        panic!("A SparkBox compiled with cfg(test) has no write exit.");
    }
    // The identifier names the config and data folders: com.nemecj.sparkbox, or .dev (identity.rs).
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = identity::identifier().to_string();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // ── P2 shell ──
        .plugin(menu::window_state_plugin())
        // ── P5 monitoring ──
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        // Closing the window keeps SparkBox running in the menu bar (unless turned off in Settings).
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                if app.state::<AppState>().prefs.lock().unwrap().keep_in_menu_bar {
                    api.prevent_close();
                    tray::hide_main(app);
                }
                admin::lock_because(app, admin::RelockReason::WindowClosed, "The window was closed");
            }
        })
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            // A dev build with its own identity starts from the release app's cluster (never its secrets).
            let imported = identity::import_from_release_once(&config_dir).unwrap_or_else(|e| {
                eprintln!("couldn't import the cluster from the release app: {e}");
                None
            });
            let inventory_path = config_dir.join("inventory.json");
            let inventory = Inventory::load(&inventory_path);
            let demo = Arc::new(Mutex::new(DemoCluster::new(&inventory)));
            let emitter = app.handle().clone();
            let vpn = Vpn::new(config_dir.join("vpn.json"), vpn::helper_path(), move |status| {
                let _ = emitter.emit("vpn://status", status);
            });
            let llm = Llm::new(config_dir.join("providers.json"), vpn.clone());
            let logger = app.handle().clone();
            let server = Server::new(config_dir.join("server.json"), llm.clone(), move |entry| {
                let _ = logger.emit("server://log", entry);
            });
            let history = History::new(app.path().app_data_dir()?.join("chats"));
            let prefs_path = config_dir.join("prefs.json");
            let known_hosts_path = config_dir.join("known_hosts.json");
            let live = Live::new(vpn.clone(), known_hosts_path.clone(), || secrets::get("ssh"));
            logging::init(&app.path().home_dir()?);
            let admin_gate = admin::AdminGate::system(config_dir.join("security.json"));
            let tasks = tasks::Tasks::new(Some(app.path().app_data_dir()?.join("tasks.jsonl")));
            live.audit.set_retention_days(admin_gate.settings().retention_days);
            live.audit.set_path(app.path().app_data_dir()?.join("activity.jsonl"));
            if let Some(files) = &imported {
                live.audit.record_app("Imported the cluster from the release app", &files.join(", "));
            }
            let prefs = Prefs::load(&prefs_path);
            let models = Models::new(config_dir.clone(), live.clone(), llm.clone(), Some(demo.clone()));
            let files = Files::new(live.clone(), app.path().app_data_dir()?.join("backups"));
            // ── p1r1-l ──
            // Look-only pauses the Files leftover sweep and stops running tasks that aren't reads.
            files.set_gate(admin_gate.clone());
            {
                let tasks = tasks.clone();
                admin_gate.on_hold(move |why| tasks.hold_non_read(why));
            }
            {
                // Look-only holds inference: a running API server stops, as Chat doesn't send
                // (and any model request it gets meanwhile is answered 503: server/mod.rs).
                let (server, live, handle) = (server.clone(), live.clone(), app.handle().clone());
                admin_gate.on_hold(move |why| {
                    let (server, live, handle) = (server.clone(), live.clone(), handle.clone());
                    let why = why.to_string();
                    tauri::async_runtime::spawn(async move {
                        if let Some(view) = server.stop_for_hold().await {
                            live.audit.record_app("API server stopped", &why);
                            announce(&handle, &view);
                        }
                    });
                    admin::HeldTasks::default()
                });
            }
            let terminals = Terminals::with_demo(live.clone(), demo.clone(), files.demo_fs());
            let ops = Ops::new(live.clone(), demo.clone(), tasks.clone());
            let tunnels = Tunnels::new(live.clone());
            models.resume(&prefs.models);
            // Build your package's state here, under your marker (pass demo.clone() / live.clone()).
            // ── P1 design ──
            // ── P2 shell ──
            // ── P3 ops ──
            app.manage(admin_gate.clone());
            app.manage(tasks.clone());
            // ── P4 world ──
            app.manage(demo.clone());
            app.manage(routeros::Switches::new(config_dir.join("switch_certs.json"), vpn.clone(), |id| secrets::get(&format!("routeros:{id}"))));
            app.manage(probe::Prober::new(vpn.clone()));
            app.manage(hardware::Hardware::new(config_dir.join("hardware-profiles.json")));
            app.manage(discovery::Discovery::default());
            app.manage(howto::HowTo::new(app.path().app_data_dir()?.join("howto")));
            app.manage(firewalla::Firewalla::new(config_dir.join("firewalla_ports.json"), config_dir.join("firewalla_msp.json"), || secrets::get(firewalla::TOKEN_ACCOUNT), admin::look_only));
            // ── P5 monitoring ──
            let data_dir = app.path().app_data_dir()?;
            app.manage(Arc::new(history::History::new(Some(data_dir.join("history")))));
            app.manage(Arc::new(health::Health::new(Some(data_dir.clone()))));
            // ── p1r1-c ──
            health::boots::start(app.handle());
            app.manage(notify::Notifier::with_log(Some(config_dir.join("notify.json")), Some(data_dir.join("deliveries.jsonl"))));
            // ── P6 term-files ──
            {
                // Demo: the Files footer's free space comes from the simulated cluster's disks.
                let handle = app.handle().clone();
                files.set_demo_space(Box::new(move |node| {
                    let state = handle.try_state::<AppState>()?;
                    let latest = state.latest.read().unwrap();
                    let disk = latest.as_ref()?.nodes.iter().find(|n| n.id == node)?.disks.iter().find(|d| d.mount == "/")?.clone();
                    Some((disk.total.saturating_sub(disk.used), disk.total))
                }));
            }
            // ── P7 ai ──
            // ── r2-design ──
            // ── r2-shell ──
            // ── r2-safety ──
            // ── r2-fabric ──
            app.manage(Arc::new(fabric::FabricMonitor::new(Some(config_dir.join("fabric_cabling.json")))));
            app.manage(fabric::commands::Doctor::new(Some(config_dir.join("fabric_baselines.json"))));
            // ── r2-monitor ──
            app.manage(agent::Agents::new(Some(config_dir.join("agents.json"))));
            // ── r2-live ──
            // ── r2-ai ──
            // ── r3-paging ──
            // ── r3-agent ──
            // ── r3-incidents ──
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
            app.manage(AppState {
                prefs: Mutex::new(prefs),
                prefs_path,
                known_hosts_path,
                live,
                demo: demo.clone(),
                inventory: Mutex::new(inventory),
                inventory_path,
                latest: RwLock::new(None),
                vpn: vpn.clone(),
                llm,
                history,
                server: server.clone(),
                models,
                terminals,
                files,
                ops,
                tunnels,
                chats: Mutex::new(HashMap::new()),
                // ── P1 design ──
                // ── P2 shell ──
                // ── P3 ops ──
                // ── P4 world ──
                // ── P5 monitoring ──
                // ── P6 term-files ──
                // ── P7 ai ──
                // ── r2-design ──
                // ── r2-shell ──
                // ── r2-safety ──
                // ── r2-fabric ──
                // ── r2-monitor ──
                // ── r2-live ──
                // ── r2-ai ──
                // ── r3-paging ──
                // ── r3-agent ──
                // ── r3-incidents ──
                // ── r3-serving ──
                // ── r3-charts ──
                // ── r3-desk ──
                // ── r3-safety ──
                // ── r4-nodes ──
                // ── r4-serving ──
                // ── r4-alerts ──
                // ── r4-agent ──
                // ── r4-safety ──
                source_override: data_source_override(),
                // ── r4-demo ──
                // ── r4-craft ──
            });
            // After AppState is managed: spawn tickers and manage extra state here, under your marker.
            // ── P1 design ──
            // ── P2 shell ──
            menu::install(app)?;
            // ── P3 ops ──
            admin_gate.set_live(is_live(&app.state::<AppState>()));
            admin::start_ticker(app.handle());
            let emitter = app.handle().clone();
            tasks.on_log(move |delta| {
                let _ = emitter.emit(tasks::LOG_EVENT, delta);
            });
            tasks.start_background();
            let emitter = app.handle().clone();
            tasks.on_update(move |t| {
                let _ = emitter.emit(tasks::UPDATE_EVENT, t);
                // A power run or a long Fleet run keeps this Mac awake while it runs (P1.12).
                awake::follow_task(t);
                if t.state == tasks::TaskState::Failed {
                    logging::write("task", &format!("{} failed: {}", t.title, t.error.as_deref().unwrap_or("")));
                }
                // A long task that ends while you're in another app gets a notification.
                let long = t.finished_ms.is_some_and(|f| f.saturating_sub(t.started_ms) > 10_000);
                let away = || emitter.get_webview_window("main").and_then(|w| w.is_focused().ok()).map(|f| !f).unwrap_or(true);
                if t.finished() && long && away() {
                    let body = match t.state {
                        tasks::TaskState::Ok => t.summary.clone().unwrap_or_else(|| "Done".into()),
                        tasks::TaskState::Cancelled => "Stopped".into(),
                        _ => t.error.clone().unwrap_or_else(|| "Failed".into()),
                    };
                    notify::post(&emitter, &t.title, &body, Some(("tasks", serde_json::json!({}))));
                }
            });
            let emitter = app.handle().clone();
            app.state::<AppState>().live.audit.on_record(move |e| {
                let _ = emitter.emit("activity://entry", e);
            });
            let emitter = app.handle().clone();
            let state = app.state::<AppState>();
            state.ops.set_emitter(move |event, payload| {
                let _ = emitter.emit(event, payload);
            });
            let reader = app.handle().clone();
            state.ops.set_snapshot_source(move || reader.state::<AppState>().latest.read().unwrap().clone());
            // ── P4 world ──
            routeros::start(app.handle());
            hardware::start(app.handle());
            probe::start(app.handle());
            firewalla::start(app.handle());
            // ── p1r1-h ──
            health::reach::start(app.handle());
            // ── P5 monitoring ──
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut every = tokio::time::interval(Duration::from_secs(history::SAVE_EVERY_S));
                every.tick().await;
                loop {
                    every.tick().await;
                    let store = handle.state::<Arc<history::History>>().inner().clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || store.save()).await;
                }
            });
            // ── P6 term-files ──
            // ── P7 ai ──
            models::commands::start_watch(app.handle());
            // ── r2-design ──
            // ── r2-shell ──
            // ── r2-safety ──
            {
                // Every relock closes the SSH tunnels. Shells stop taking keystrokes only when the
                // owner or macOS locks (not when Admin mode idles out), and after their own idle time.
                let state = app.state::<AppState>();
                let (terminals, tunnels) = (state.terminals.clone(), state.tunnels.clone());
                admin_gate.on_relock(move |why, text| {
                    terminals.on_relock(why, text);
                    tunnels.close_all();
                });
                state.terminals.set_clock(admin_gate.clock());
                state.terminals.set_idle_minutes(admin_gate.settings().shell_idle_minutes);
                state.terminals.start_idle_timer();
                admin::watch_screen_lock(app.handle());
                state.tunnels.start_idle_reaper();
                // Restarts, wakes and container restarts end by checking the models answer again.
                state.ops.set_verifier(Arc::new(ops::ModelsVerifier(state.models.clone())));
            }
            // ── r2-fabric ──
            // ── r2-monitor ──
            agent::start(app.handle());
            // ── p1r1-x ──
            // The write exit reads the same AdminGate (look-only, Admin) as the background work.
            live::exit::start(app.handle());
            // ── p1r3-check ── the update check's poller and its last results (P1.20).
            ops::update_check::start(app.handle());
            notify::install_click_handler(app.handle());
            // ── r2-live ──
            // Upgrades started before SparkBox last quit are followed again once their node is back.
            app.state::<AppState>().ops.reattach_upgrades();
            // ── r2-ai ──
            let announcer = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Look-only holds inference too: the API server doesn't start, as Chat doesn't send.
                if crate::server::autostart_now(server.autostart().await, admin::look_only()) {
                    match server.start().await {
                        Ok(view) => announce(&announcer, &view),
                        Err(e) => eprintln!("server didn't start: {e}"),
                    }
                }
            });
            // ── r3-paging ──
            // ── r3-agent ──
            // ── r3-incidents ──
            // ── r3-serving ──
            // ── r3-charts ──
            // Charts open full: fills become `history://filled`; Live pulls each agent's last hour first.
            history::backfill::announce_fills(app.handle());
            history::backfill::start(app.handle());
            // ── r3-desk ──
            // ── r3-safety ──
            // Restarts, shutdowns and wakes are planned work for the alerting and the agents.
            app.state::<AppState>().ops.set_planned(Arc::new(ops::AppPlanned(app.handle().clone())));
            // Reachability is tested against every Spark's SSH port and the Gold, by IP:
            // .local names don't resolve across a VPN.
            let handle = app.handle().clone();
            vpn.start(move || vpn::targets(&handle.state::<AppState>().inventory.lock().unwrap()));
            // ── p1r2-mac ──
            // "Keeping this Mac awake while … runs" reaches the status bar (`awake://status`).
            awake::start(app.handle());
            // ── r4-nodes ──
            // ── r4-serving ──
            // ── r4-alerts ──
            // ── r4-agent ──
            // ── r4-safety ──
            {
                let state = app.state::<AppState>();
                // Filed with the source this run uses (apply_data_source below sets it again).
                state.live.audit.set_demo(!is_live(&state));
                if let Some(source) = state.source_override {
                    let which = if source == DataSource::Live { "Live" } else { "Demo" };
                    state.live.audit.record_app("Data source for this run", &format!("{which} (SPARKBOX_DATA_SOURCE) · prefs.json unchanged"));
                }
                if admin::look_only() {
                    let why = if admin::look_only_forced() { "SPARKBOX_LOOK_ONLY=1" } else { "Settings › Security" };
                    state.live.audit.record_app("Look-only mode on", &format!("{why} · starts no change on the cluster or this Mac"));
                }
            }
            // ── r4-demo ──
            {
                // Tasks and chats carry the data source they ran under; their lists show the current one's.
                let handle = app.handle().clone();
                tasks.set_source(move || handle.try_state::<AppState>().is_none_or(|s| !is_live(&s)));
                let handle = app.handle().clone();
                app.state::<AppState>().history.set_source(move || handle.try_state::<AppState>().is_none_or(|s| !is_live(&s)));
                // ── R2-CHATMODELS ── With Live data the demo model isn't offered (P1.28).
                let handle = app.handle().clone();
                app.state::<AppState>().llm.set_source(move || handle.try_state::<AppState>().is_none_or(|s| !is_live(&s)));
                // Tasks saved before that are placed once by their activity-log rows, else Demo.
                let entries = app.state::<AppState>().live.audit.entries();
                tasks.classify_legacy(|op| {
                    let rows: Vec<_> = entries.iter().filter(|e| e.op_id.as_deref() == Some(op)).collect();
                    rows.iter()
                        .find_map(|e| e.demo.map(|d| !d))
                        .or_else(|| rows.iter().any(|e| e.simulated).then_some(false))
                        .or_else(|| rows.iter().any(|e| matches!(e.kind.as_str(), "command" | "sudo" | "terminal" | "stream") && e.node != "mac").then_some(true))
                });
            }
            // ── r4-craft ──
            // SIGTERM and SIGINT save history the way Quit does: a restart keeps its minutes.
            history::save_on_signals(app.handle());
            apply_data_source(app.handle());
            publish(app.handle(), false);

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut every = tokio::time::interval(Duration::from_secs(1));
                loop {
                    every.tick().await;
                    publish(&handle, true);
                }
            });

            let tray = Tray::build(app.handle(), |app| {
                // The same gated, logged commands as the Server window.
                let app = app.clone();
                tauri::async_runtime::spawn(tray::from_menu_bar("Start or stop the API server", async move {
                    let running = app.state::<AppState>().server.view().await.running;
                    let result = if running { server_stop(app.clone(), app.state()).await } else { server_start(app.clone(), app.state()).await };
                    if let Err(e) = result {
                        tray::refused(&app, if running { "stop the API server" } else { "start the API server" }, &e);
                    }
                }));
            })?;
            app.manage(tray);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut every = tokio::time::interval(Duration::from_secs(2));
                loop {
                    every.tick().await;
                    let state = handle.state::<AppState>();
                    let Some(snap) = state.latest.read().unwrap().clone() else { continue };
                    let deployments = state.models.deployments(&models_ctx(&state));
                    let vpn = state.vpn.view().await.status;
                    let server = state.server.view().await;
                    if let Err(e) = handle.state::<Tray>().update(&handle, &snap, &deployments, &vpn, &server) {
                        eprintln!("menu bar update failed: {e}");
                    }
                }
            });

            // Debug builds only: run the JavaScript in $SPARKBOX_DEV_SCRIPT in the window each time
            // that file changes, so the UI can be driven for screenshots without touching the screen.
            #[cfg(debug_assertions)]
            if let Ok(path) = std::env::var("SPARKBOX_DEV_SCRIPT") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(w) = handle.get_webview_window("main") {
                        devshot::keep_painting(&w);
                    }
                    let mut last = String::new();
                    loop {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        // A picture for Help: the file names where it goes.
                        let shot = format!("{path}.shot");
                        if let Ok(out) = std::fs::read_to_string(&shot) {
                            let _ = std::fs::remove_file(&shot);
                            if let Some(w) = handle.get_webview_window("main") {
                                devshot::snapshot(&w, out.trim().to_string());
                            }
                        }
                        let Ok(js) = std::fs::read_to_string(&path) else { continue };
                        if js != last {
                            last = js.clone();
                            if let Some(w) = handle.get_webview_window("main") {
                                let _ = w.eval(&js);
                            }
                        }
                    }
                });
            }

            // Launched at login: start quietly in the menu bar.
            if std::env::args().any(|a| a == "--hidden") {
                tray::hide_main(app.handle());
            }
            Ok(())
        })
        // Look-only mode refuses everything but reads before the command runs.
        .invoke_handler(admin::look_only_guard(tauri::generate_handler![
            howto::commands::howto_document,
            howto::commands::howto_saved,
            howto::commands::howto_read,
            howto::commands::howto_save,
            howto::commands::howto_last_check,
            howto::commands::howto_check_source,
            howto::commands::howto_fetch_source,
            howto::commands::howto_check_nodes,
            howto::commands::howto_prepare_image,
            get_snapshot,
            get_inventory,
            save_inventory,
            credential_status,
            set_credential,
            clear_credential,
            container_action,
            container_inspect,
            vpn_get,
            vpn_import,
            vpn_set_endpoint,
            vpn_set_mode,
            vpn_set_forwards,
            vpn_reconnect,
            vpn_remove,
            providers_list,
            providers_upsert,
            providers_remove,
            providers_set_key,
            providers_clear_key,
            providers_refresh,
            providers_scan,
            chat_send,
            chat_stop,
            conversations_list,
            conversation_get,
            conversation_save,
            conversation_delete,
            server_get,
            server_save,
            server_start,
            server_stop,
            server_create_key,
            server_revoke_key,
            server_logs,
            server_self_test,
            prefs_get,
            doctor_run,
            known_hosts_forget,
            hf_search,
            hf_model,
            models_analyze,
            library_list,
            jobs_list,
            job_cancel,
            job_resume_download,
            model_download,
            model_download_tools,
            model_disk_preflight,
            model_share,
            model_delete,
            cluster_check,
            deploy_preview,
            deploy,
            deployments_list,
            deployment_history_list,
            deployment_history_file,
            deployment_unload,
            deployment_resume,
            deployment_forget,
            benchmarks_list,
            terminal_open,
            terminal_input,
            terminal_resize,
            terminal_close,
            files_list,
            files_read,
            files_write,
            files_mkdir,
            files_rename,
            files_delete,
            files_chmod,
            files_cancel,
            process_signal,
            process_renice,
            services_list,
            service_action,
            service_status,
            node_power,
            fleet_run,
            tunnel_open,
            activity_log,
            activity_log_path,
            dev_report,
            // ── P1 design ──
            // ── P2 shell ──
            menu::menu_set_windows,
            menu::menu_set_state,
            menu::menu_close_main,
            menu::menu_minimize_main,
            // ── P3 ops ──
            admin::admin_state,
            admin::admin_unlock,
            admin::admin_lock,
            admin::security_get,
            admin::security_set,
            tasks::tasks_list,
            tasks::task_get,
            tasks::task_cancel,
            activity_log_since,
            activity_export,
            ops::commands::fleet_start,
            ops::commands::fleet_cancel,
            ops::commands::fleet_results,
            ops::commands::ops_classify,
            ops::commands::services_apply,
            ops::commands::service_detail,
            ops::commands::service_impact,
            ops::commands::containers_bulk,
            ops::commands::ports_reveal,
            ops::commands::updates_status,
            ops::update_check::updates_summary,
            ops::commands::updates_simulate,
            ops::commands::updates_hold,
            ops::commands::updates_refresh,
            ops::commands::updates_upgrade,
            ops::rdp::rdp_check,
            ops::rdp::rdp_repair,
            ops::commands::updates_system_preview,
            ops::commands::updates_system_start,
            ops::commands::updates_system_status,
            ops::commands::updates_system_verify,
            ops::commands::updates_system_discard_review,
            ops::rolling::updates_rolling_preview,
            ops::rolling::updates_rolling_status,
            ops::rolling::updates_rolling_start,
            ops::rolling::updates_rolling_resume,
            ops::commands::updates_hold_preset,
            ops::commands::process_alive,
            // ── R2-OPS ── maintenance mode (P1.16)
            ops::commands::maintenance_begin,
            ops::commands::maintenance_end,
            logging::log_ui_error,
            logging::diagnostics_export,
            // ── P4 world ──
            demo::commands::demo_scenarios_list,
            demo::commands::demo_scenario_set,
            util::persistence_warnings,
            firewalla::commands::firewalla_view,
            firewalla::commands::firewalla_label_set,
            firewalla::commands::firewalla_msp_settings,
            firewalla::commands::firewalla_msp_set_domain,
            firewalla::commands::firewalla_msp_test,
            hardware::hardware_profiles,
            discovery::discovery_networks,
            discovery::discovery_status,
            discovery::discovery_start,
            discovery::discovery_cancel,
            discovery::discovery_verify,
            discovery::discovery_propose,
            routeros::commands::switch_test,
            routeros::commands::switch_port_set,
            routeros::commands::switch_trust,
            routeros::commands::switch_port_comment,
            routeros::commands::switch_port_reset,
            routeros::commands::switch_port_blink,
            routeros::commands::switch_probe_history,
            vpn::commands::vpn_override,
            onboard::node_probe,
            onboard::known_hosts_trust,
            // ── P5 monitoring ──
            health::commands::health_get,
            health::commands::alerts_history,
            health::commands::alert_ack,
            history::commands::history_query,
            notify::commands::notify_settings_get,
            notify::commands::notify_settings_set,
            notify::commands::notify_take_pending,
            // ── P6 term-files ──
            terminal_ack,
            term::terminal_save_text,
            files::commands::files_trash,
            files::commands::files_new_file,
            files::commands::files_untrash,
            files::commands::files_backups,
            files::commands::files_du,
            files::commands::files_preflight,
            files::commands::files_transfer,
            // ── P7 ai ──
            models::commands::endpoints_list,
            models::commands::endpoints_refresh,
            models::commands::deploy_plan,
            models::commands::model_preflight,
            models::commands::endpoint_stop,
            models::commands::ollama_list,
            models::commands::ollama_pull,
            models::commands::ollama_delete,
            models::commands::bench_run,
            models::commands::bench_export,
            models::commands::deployment_restart,
            models::commands::recipes_list,
            models::commands::deployment_clean_leftovers,
            // ── r2-design ──
            // ── r2-shell ──
            // ── r2-safety ──
            admin::admin_authenticate,
            admin::admin_extend,
            admin::autostart_get,
            admin::autostart_set,
            data_source_set,
            tunnel_close,
            container_reveal,
            // ── r2-fabric ──
            fabric::commands::fabric_state,
            fabric::commands::fabric_cabling_accept,
            fabric::commands::fabric_doctor_start,
            fabric::commands::fabric_doctor_report,
            fabric::commands::fabric_baselines,
            fabric::commands::fabric_baselines_reset,
            fabric::commands::fabric_perftest_install,
            routeros::commands::switch_port_keep_disabled,
            routeros::commands::switch_port_bounce,
            routeros::commands::switch_tables,
            // ── r2-monitor ──
            health::commands::rules_get,
            health::commands::rules_defaults,
            health::commands::rules_set,
            health::commands::rules_preview,
            health::commands::kernel_events,
            notify::commands::notify_channels_get,
            notify::commands::notify_channels_set,
            notify::commands::notify_channel_test,
            notify::commands::notify_deliveries,
            agent::commands::agent_status,
            agent::commands::agent_install,
            agent::commands::agent_uninstall,
            agent::commands::agent_push_config,
            agent::commands::agent_backfill_now,
            agent::commands::agent_settings_get,
            agent::commands::agent_settings_set,
            // ── r2-live ──
            term::terminal_authorize,
            term::terminal_broadcast,
            wol::node_wol_status,
            files::commands::files_leftovers,
            files::commands::files_leftover_remove,
            ops::commands::updates_progress,
            ops::commands::updates_cancel,
            live::sshconfig::login_settings_get,
            live::sshconfig::login_settings_set,
            live::sshconfig::login_methods,
            live::sshconfig::login_passphrase_set,
            live::sshconfig::login_setup_key,
            // ── r2-ai ──
            models::commands::endpoint_forget,
            models::commands::endpoint_restart,
            // ── r3-paging ──
            agent::commands::agent_test_page,
            // ── r3-agent ──
            // ── r3-incidents ──
            health::commands::alert_silence,
            health::commands::alert_unsilence,
            // ── r3-serving ──
            models::commands::hf_size,
            // ── r3-charts ──
            // ── r3-desk ──
            // ── r3-safety ──
            tunnels_list,
            prefs_patch,
            // ── r4-nodes ──
            ops::services::services_poll,
            // ── r4-serving ──
            models::commands::cluster_deployments_list,
            models::commands::model_restart_after,
            models::commands::model_image_jobs,
            models::commands::model_recipe_import,
            models::cache_grant::drop_cache_grant_get,
            models::cache_grant::drop_cache_grant_set,
            models::earlyoom::earlyoom_preview,
            models::earlyoom::earlyoom_apply,
            models::commands::library_detail,
            // ── r4-alerts ──
            // ── r4-agent ──
            // ── r4-safety ──
            admin::look_only_set,
            files_read_sudo,
            ops::commands::images_list,
            // ── r4-demo ──
            // ── r4-craft ──
            // ── p1r1-w ──
            evidence::evidence_capture,
            evidence::evidence_list,
            // ── p1r1-x ──
            live::exit::fence_state,
            live::exit::fence_open,
            live::exit::fence_confirm,
            live::exit::fence_close,
            live::exit::fence_trust_key,
            live::exit::lease_state,
            live::exit::node_identity,
            live::exit::node_identity_repin,
            // ── p1r1-k ──
            ops::commands::process_reveal,
            // ── p1r1-h ──
            health::reach::reach_state,
            // ── p1r1-c ──
            health::boots::commands::boots_get,
            health::boots::commands::boots_refresh,
            // ── p1r3 ──
            ops::prereboot::node_prereboot_check,
            footprint::processes_get,
            ops::containers::container_test_run,
        ]))
        .build(context)
        .expect("error while building SparkBox")
        .run(|app, event| {
            // Clicking the Dock icon brings the window back.
            if let RunEvent::Reopen { .. } = event {
                tray::show_main(app, None);
            }
            // ── P5 monitoring ──
            if let RunEvent::Exit = event {
                if let Some(store) = app.try_state::<Arc<history::History>>() {
                    store.save_final();
                }
            }
            if let RunEvent::Exit = event {
                admin::lock_because(app, admin::RelockReason::Quit, "SparkBox quit");
                // ── r2-safety ──
                if let Some(tasks) = app.try_state::<Arc<tasks::Tasks>>() {
                    tasks.save_now();
                }
                if let Some(state) = app.try_state::<AppState>() {
                    state.tunnels.close_all();
                }
            }
        });
}
