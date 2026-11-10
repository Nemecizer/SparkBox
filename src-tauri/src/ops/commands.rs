//! Tauri commands for node operations that are new in this round (Fleet streaming, bulk
//! containers, service rollouts, updates, maintenance mode). Each one that changes a node checks
//! Admin mode first and runs as a task; the long ones return the task's id at once.

use serde::Serialize;
use tauri::{AppHandle, State};

use super::containers::Target;
use super::fleet::{FleetResult, FleetSpec, Strategy};
use super::services::{self, Impact, NodeCtx, ServiceDetail};
use super::updates::{HoldPreset, NodeUpdates, SimulatedUpgrade, UpgradeArgs};
use crate::errors::{ErrorKind, SbError};
use crate::health::maintenance;
use crate::model::{Listener, MaintInfo, NodeSnapshot};
use crate::vpn::VpnState;
use crate::{admin, is_live, AppState};

// ---- Fleet Command

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn fleet_start(
    app: AppHandle,
    state: State<'_, AppState>,
    nodes: Vec<String>,
    command: String,
    sudo: bool,
    timeout_s: u64,
    strategy: Option<Strategy>,
    stop_on_failure: Option<bool>,
    title: Option<String>,
    cuts_access: Option<bool>,
) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    let spec = FleetSpec {
        nodes,
        command,
        sudo,
        timeout_s,
        strategy: strategy.unwrap_or_default(),
        stop_on_failure: stop_on_failure.unwrap_or(false),
        title,
        cuts_access: cuts_access.unwrap_or(false),
    };
    state.ops.fleet_start(demo, spec)
}

/// Stops a run. Cancelling needs Admin mode like starting it did.
#[tauri::command]
pub fn fleet_cancel(app: AppHandle, state: State<'_, AppState>, run_id: String) -> Result<(), String> {
    admin::require(&app)?;
    state.ops.tasks().cancel(&run_id)
}

#[tauri::command]
pub fn fleet_results(state: State<'_, AppState>, run_id: String) -> Option<Vec<FleetResult>> {
    state.ops.fleet_results(&run_id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandClass {
    /// read | change | destructive
    pub class: &'static str,
    /// Some part of it runs through sudo, doas or pkexec (beyond the "as root" switch).
    pub escalates: bool,
    /// Why it's destructive, for the confirmation's banner.
    pub reason: Option<String>,
    /// Destructive, and SparkBox may not get back in (stops SSH, the network, the firewall…).
    pub cuts_access: bool,
    /// Restarts or switches off the node: on several nodes at once that cuts access too.
    pub power: Option<super::classify::Power>,
    /// Runs with the node's root through Docker (Docker is root-equivalent): Fleet asks for the
    /// typed count, runs a canary and one node at a time (P1.42).
    pub root: bool,
}

/// Fleet's confirmation tier for the exact command it's about to run: does it only read,
/// change things, or could it destroy data, take nodes down or cut SparkBox's access?
#[tauri::command]
pub fn ops_classify(command: String) -> CommandClass {
    let c = command.trim();
    let v = super::classify::verdict(c);
    CommandClass { class: v.kind.as_str(), escalates: super::classify::escalates(c), reason: v.reason, cuts_access: v.cuts_access, power: v.power, root: v.root }
}

// ---- Services

#[tauri::command]
pub async fn services_apply(app: AppHandle, state: State<'_, AppState>, nodes: Vec<String>, unit: String, action: String) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.services_apply(demo, nodes, unit, action)
}

#[tauri::command]
pub async fn service_detail(state: State<'_, AppState>, node: String, unit: String) -> Result<ServiceDetail, String> {
    let demo = !is_live(&state);
    state.ops.service_detail(demo, &node, &unit).await
}

/// What a service action will break, for the confirmation.
#[tauri::command]
pub async fn service_impact(state: State<'_, AppState>, nodes: Vec<String>, unit: String, action: String) -> Result<Impact, SbError> {
    services::check_action(&unit, &action)?;
    let snap = state.latest.read().unwrap().clone();
    let remote = state.vpn.view().await.status.state == VpnState::Connected;
    let ctx: Vec<NodeCtx> = nodes
        .iter()
        .map(|id| {
            let view = snap.as_ref().and_then(|s| s.nodes.iter().find(|n| &n.id == id));
            match (view, &snap) {
                (Some(n), Some(s)) => node_ctx(&s.nodes, n),
                _ => NodeCtx { name: id.clone(), ..Default::default() },
            }
        })
        .collect();
    Ok(services::service_impact(&unit, &action, &ctx, remote))
}

fn node_ctx(cluster: &[NodeSnapshot], n: &NodeSnapshot) -> NodeCtx {
    NodeCtx {
        name: n.name.clone(),
        containers: n.containers.iter().filter(|c| c.state == "running").map(|c| c.name.clone()).collect(),
        serving: services::serving(cluster, n),
    }
}

// ---- Containers

#[tauri::command]
pub async fn containers_bulk(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<Target>,
    action: String,
    remove_volumes: Option<bool>,
    title: Option<String>,
) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.containers_bulk(demo, items, action, remove_volumes.unwrap_or(false), title)
}

// ---- Processes and ports

/// Every listening port with its owner, read with sudo (shows root's ports too).
#[tauri::command]
pub async fn ports_reveal(app: AppHandle, state: State<'_, AppState>, node: String) -> Result<Vec<Listener>, SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.reveal_ports(demo, &node).await
}

// ---- Updates

#[tauri::command]
pub async fn updates_status(state: State<'_, AppState>, node: String) -> Result<NodeUpdates, SbError> {
    let demo = !is_live(&state);
    state.ops.updates_status(demo, &node).await
}

#[tauri::command]
pub async fn updates_simulate(state: State<'_, AppState>, node: String, packages: Option<Vec<String>>) -> Result<SimulatedUpgrade, SbError> {
    let demo = !is_live(&state);
    state.ops.updates_simulate(demo, &node, &packages.unwrap_or_default()).await
}

#[tauri::command]
pub async fn updates_hold(app: AppHandle, state: State<'_, AppState>, node: String, packages: Vec<String>, hold: bool) -> Result<(), SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.updates_hold(demo, &node, &packages, hold).await
}

#[tauri::command]
pub async fn updates_refresh(app: AppHandle, state: State<'_, AppState>, nodes: Vec<String>) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    if is_live(&state) {
        return super::update_check::refresh_all(&state.ops, nodes);
    }
    state.ops.updates_refresh(true, nodes)
}

/// `opts`: the download limit for the reattach test, "security only" (the backend picks the
/// pending security fixes that aren't GPU-stack or held, and `packages` is ignored), and the plan
/// the dry run showed (`plan`: name and version of each package), which the upgrade installs
/// exactly or refuses; with Live data it's required.
#[tauri::command]
pub async fn updates_upgrade(app: AppHandle, state: State<'_, AppState>, node: String, packages: Option<Vec<String>>, opts: Option<UpgradeArgs>) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.updates_upgrade_opts(demo, node, packages.unwrap_or_default(), opts.unwrap_or_default())
}

/// "Pin the GPU stack" (`hold`) or "Release the GPU-stack pin" on one node (P1.27). With
/// `preview` it only reads which packages that is (dpkg-query -W, apt-mark showhold), for the
/// confirm; it still needs Admin, as the pin does. Live only.
#[tauri::command]
pub async fn updates_hold_preset(app: AppHandle, state: State<'_, AppState>, node: String, hold: bool, preview: Option<bool>) -> Result<HoldPreset, SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    state.ops.updates_hold_preset(demo, &node, hold, preview.unwrap_or(false)).await
}

/// Upgrades running (or just finished) on each node: phase, "n of m", the newest log lines and
/// whether Cancel can still stop apt. Read-only.
#[tauri::command]
pub fn updates_progress(state: State<'_, AppState>) -> Vec<super::updates::UpgradeRun> {
    state.ops.runs()
}

/// Stops an upgrade while apt is still downloading; refused once dpkg is installing.
#[tauri::command]
pub fn updates_cancel(app: AppHandle, state: State<'_, AppState>, node: String) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    state.ops.updates_cancel(&node)
}

/// Is the process still there (the same one: its start time must match)? Read-only, not logged.
#[tauri::command]
pub async fn process_alive(state: State<'_, AppState>, node: String, pid: u32, start_time: Option<u64>) -> Result<bool, String> {
    if !is_live(&state) {
        let snap = state.latest.read().unwrap().clone();
        let n = snap.and_then(|s| s.nodes.into_iter().find(|n| n.id == node)).ok_or("unknown node")?;
        return Ok(n.processes.iter().any(|p| p.pid == pid && (start_time.is_none() || p.start_time == start_time)));
    }
    // Fresh each time: Workloads asks every 500 ms whether a stopped process has gone.
    let out = state.live.probe_fresh(&node, &format!("sed 's/^.*) //' /proc/{pid}/stat 2>/dev/null | cut -d' ' -f20"), std::time::Duration::from_secs(10)).await?;
    let found = out.stdout.trim();
    Ok(!found.is_empty() && start_time.is_none_or(|s| found == s.to_string()))
}

/// One process's full command line, secrets shown: Admin (node scope), then a read
/// (`cat /proc/<pid>/cmdline`), not logged. Like `container_reveal` (P1.46). Code-gated.
#[tauri::command]
pub async fn process_reveal(app: AppHandle, state: State<'_, AppState>, node: String, pid: u32) -> Result<String, String> {
    admin::require(&app)?;
    if !is_live(&state) {
        let snap = state.latest.read().unwrap().clone();
        let p = snap
            .and_then(|s| s.nodes.into_iter().find(|n| n.id == node))
            .and_then(|n| n.processes.into_iter().find(|p| p.pid == pid))
            .ok_or("That process isn't in the latest snapshot.")?;
        return Ok(p.cmd);
    }
    let out = state.live.probe(&node, &format!("cat /proc/{pid}/cmdline | tr '\\0' ' '"), std::time::Duration::from_secs(10)).await?;
    let cmd = out.stdout.trim().to_string();
    if cmd.is_empty() {
        Err(format!("Process {pid} is gone or has no command line."))
    } else {
        Ok(cmd)
    }
}

// ---- Images (read only)

/// Every node's Docker images, with what uses them and Docker's disk totals: `docker image ls`
/// and `docker system df` on each node (both reads), or the simulated containers' in Demo.
#[tauri::command]
pub async fn images_list(state: State<'_, AppState>) -> Result<Vec<super::images::NodeImages>, String> {
    let demo = !is_live(&state);
    Ok(state.ops.images(demo).await)
}

// ---- Maintenance mode (P1.16)

/// The node's display name, for one of your nodes.
fn inventory_name(state: &AppState, node: &str) -> Result<String, SbError> {
    let inventory = state.inventory.lock().unwrap();
    inventory.nodes.iter().find(|n| n.id == node).map(|n| n.name.clone()).ok_or_else(|| SbError::invalid(format!("{node} isn't one of your nodes.")))
}

/// An activity row about a node that ran nothing on it (class "app").
fn maintenance_row(state: &AppState, node: &str, title: &str, detail: &str) {
    state.live.audit.record(crate::live::audit::Entry { node: node.into(), kind: "app".into(), command: detail.into(), title: title.into(), state: "ok".into(), ..Default::default() });
}

/// Puts a node in maintenance mode for `minutes` (None: until it's ended): its alerts are
/// Planned and SparkBox pages nobody about them (the node agents aren't told). Nothing runs on
/// the node, so the write exit has nothing to admit, but it changes how the node is watched:
/// Admin (node scope), Mac scope too for "until I end it", and look-only refuses it.
#[tauri::command]
pub fn maintenance_begin(app: AppHandle, state: State<'_, AppState>, node: String, minutes: Option<u32>) -> Result<MaintInfo, SbError> {
    admin::require_sb(&app)?;
    if !is_live(&state) {
        return Err(SbError::new(ErrorKind::DemoOnly, "Available with Live data."));
    }
    let name = inventory_name(&state, &node)?;
    if minutes.is_some_and(|m| m == 0 || m > maintenance::MANUAL_MAX_MINUTES) {
        return Err(SbError::invalid("Pick up to 24 hours, or until you end it."));
    }
    let fence_nodes = || {
        let view = crate::live::exit::fence_state(app.clone());
        (view.state == crate::live::fence::State::Valid && view.mine).then(|| view.record.map(|r| r.nodes).unwrap_or_default())
    };
    maintenance_allowed(&name, minutes, crate::identity::instance_name(), || admin::require_mac(&app).map_err(SbError::from), fence_nodes, &node)?;
    let until = minutes.map(|m| crate::history::now_ms() + u64::from(m) * 60_000);
    let info = maintenance::begin_manual(&app, &node, until).map_err(|e| SbError::invalid(format!("{name}: {e}")).on(&node))?;
    let agents = state.latest.read().unwrap().as_ref().is_some_and(|s| s.nodes.iter().any(|n| n.agent.is_some()));
    maintenance_row(&state, &node, &maintenance::manual_title(&name, minutes), &maintenance_detail(&name, until, agents));
    Ok(info)
}

/// Beyond node scope. "Until I end it" mutes the node's pages with no end, so it needs Mac scope
/// (Touch ID in the last 5 minutes), as an "until it clears" silence does (P1.45). A debug
/// build quiets only a node its own open fence names (`fence_nodes`: that fence's nodes, None
/// with none open), so the agents' build can't silence the others' pages.
fn maintenance_allowed(
    name: &str,
    minutes: Option<u32>,
    instance: &str,
    mac: impl FnOnce() -> Result<(), SbError>,
    fence_nodes: impl FnOnce() -> Option<Vec<String>>,
    node: &str,
) -> Result<(), SbError> {
    if minutes.is_none() {
        mac()?;
    }
    if instance == "dev" && !fence_nodes().is_some_and(|nodes| nodes.iter().any(|n| n == node)) {
        return Err(SbError::new(ErrorKind::Fenced, format!("{name} isn't in this build's open fence, so this build can't put it in maintenance.")).on(node));
    }
    Ok(())
}

/// The activity row's words. With node agents installed, only SparkBox's own pages stop: the
/// agents aren't told about maintenance mode and page on their own.
fn maintenance_detail(name: &str, until: Option<u64>, agents: bool) -> String {
    let until = until.map(|u| format!("until {}", maintenance::local_hhmm(u))).unwrap_or_else(|| "until you end it".into());
    if agents {
        format!("Alerts about {name} are planned {until} and SparkBox pages nobody about them; the node agents aren't told and may still page. Nothing on the node changes.")
    } else {
        format!("Alerts about {name} are planned {until} and page nobody. Nothing on the node changes.")
    }
}

/// Ends a node's maintenance mode: its alerts count again.
#[tauri::command]
pub fn maintenance_end(app: AppHandle, state: State<'_, AppState>, node: String) -> Result<(), SbError> {
    admin::require_sb(&app)?;
    if !is_live(&state) {
        return Err(SbError::new(ErrorKind::DemoOnly, "Available with Live data."));
    }
    let name = inventory_name(&state, &node)?;
    maintenance::end_manual(&app, &node).map_err(|e| SbError::invalid(format!("{name}: {e}")).on(&node))?;
    maintenance_row(&state, &node, &format!("Maintenance ended on {name}"), &format!("Alerts about {name} count again."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::testing::gate;

    #[test]
    fn until_i_end_it_needs_mac_scope_and_a_debug_build_quiets_only_its_fence() {
        let (g, _, _) = gate();
        // Admin mode open for nodes, without Touch ID (a Demo-style unlock).
        g.unlock(None).unwrap();
        let mac = || g.check_mac().map_err(SbError::from);
        let cell2 = || Some(vec!["cell2".to_string()]);
        // For an hour: node scope is enough.
        assert!(maintenance_allowed("cell2", Some(60), "release", mac, || None, "cell2").is_ok());
        // Until I end it: Mac scope first, as an "until it clears" silence.
        let e = maintenance_allowed("cell2", None, "release", mac, || None, "cell2").unwrap_err();
        assert_eq!(e.kind, ErrorKind::NeedsAuth);
        g.authenticate(Some("maintenance_begin"), false).unwrap();
        assert!(maintenance_allowed("cell2", None, "release", mac, || None, "cell2").is_ok());

        // The debug build: only a node its own open fence names, timed or not.
        let e = maintenance_allowed("cell1", Some(60), "dev", mac, cell2, "cell1").unwrap_err();
        assert_eq!((e.kind, e.message.as_str(), e.node.as_deref()), (ErrorKind::Fenced, "cell1 isn't in this build's open fence, so this build can't put it in maintenance.", Some("cell1")));
        assert_eq!(maintenance_allowed("cell3", None, "dev", mac, cell2, "cell3").unwrap_err().kind, ErrorKind::Fenced);
        assert!(maintenance_allowed("cell2", Some(60), "dev", mac, cell2, "cell2").is_ok());
        assert_eq!(maintenance_allowed("cell2", Some(60), "dev", mac, || None, "cell2").unwrap_err().kind, ErrorKind::Fenced, "no fence open");
        // Mac scope still comes first in the debug build.
        g.lock();
        g.unlock(None).unwrap();
        assert_eq!(maintenance_allowed("cell2", None, "dev", mac, cell2, "cell2").unwrap_err().kind, ErrorKind::NeedsAuth);
    }

    #[test]
    fn the_activity_row_says_the_agents_arent_told_when_there_are_agents() {
        assert_eq!(maintenance_detail("cell2", None, false), "Alerts about cell2 are planned until you end it and page nobody. Nothing on the node changes.");
        assert_eq!(
            maintenance_detail("cell2", None, true),
            "Alerts about cell2 are planned until you end it and SparkBox pages nobody about them; the node agents aren't told and may still page. Nothing on the node changes."
        );
        assert!(maintenance_detail("cell2", Some(1_800_000_000_000), false).starts_with("Alerts about cell2 are planned until "));
    }
}

#[tauri::command]
pub async fn updates_system_preview(app: AppHandle, state: State<'_, AppState>, node: String) -> Result<SimulatedUpgrade, SbError> {
    admin::require_sb(&app)?;
    state.ops.system_preview(!is_live(&state), &node).await
}
#[tauri::command]
pub async fn updates_system_status(state: State<'_, AppState>) -> Result<Option<super::protected_updates::Record>, String> {
    state.ops.system_status(!is_live(&state))
}
#[tauri::command]
pub async fn updates_system_start(app: AppHandle, state: State<'_, AppState>, node: String, plan: Vec<super::updates::PlanEntry>) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    let ctx = crate::models_ctx(&state);
    if !ctx.node_ids().contains(&node) { return Err(SbError::invalid("Choose an inventory node.")); }
    if state.models.deployments(&ctx).iter().any(|d| d.state.active() && d.spec.nodes.contains(&node)) || state.models.endpoints().iter().any(|e| e.nodes.contains(&node) && !e.members.is_empty()) {
        return Err(SbError::invalid("Unload models on this node before installing system updates."));
    }
    state.ops.system_start(!is_live(&state), node, plan)
}
#[tauri::command]
pub async fn updates_system_verify(app: AppHandle, state: State<'_, AppState>) -> Result<super::protected_updates::Record, String> {
    admin::require(&app)?;
    state.ops.system_verify(!is_live(&state)).await
}

#[tauri::command]
pub async fn updates_system_discard_review(app: AppHandle, state: State<'_, AppState>) -> Result<super::protected_updates::Record, String> {
    admin::require(&app)?;
    state.ops.system_discard_review(!is_live(&state))
}
