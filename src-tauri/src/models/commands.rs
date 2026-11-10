//! Tauri commands for endpoints and load planning (the older Models commands
//! still live in lib.rs). Also the endpoint round that runs every 3 s.

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};

use super::endpoints::{Endpoint, ManagedBy};
use super::plan::{DeploySpec, PreflightSet};
use super::postmortem::ClusterDeployment;
use super::{DeployPlan, Models};
use crate::{admin, is_live, models_ctx, AppState};

pub const ENDPOINTS_EVENT: &str = "endpoints://update";
/// The deployments on the cluster changed (one appeared, failed, or its postmortem was read).
pub const DEPLOYMENTS_EVENT: &str = "deployments://changed";

#[tauri::command]
pub async fn model_image_jobs(app: AppHandle, state: State<'_, AppState>, image: String, nodes: Vec<String>, source: Option<String>, estimated_bytes: u64) -> Result<Vec<super::Job>, String> {
    admin::require(&app)?;
    state.models.image_jobs(&models_ctx(&state), &image, &nodes, source.as_deref(), estimated_bytes).await
}

#[tauri::command]
pub async fn model_recipe_import(state: State<'_, AppState>, node: String, folder: String) -> Result<serde_json::Value, String> {
    let ctx = models_ctx(&state);
    if !ctx.node_ids().contains(&node) || folder.len() > 4096 || folder.chars().any(char::is_control) {
        return Err("Choose an inventory node and a script folder.".into());
    }
    let mut draft = if ctx.live {
        state.models.sparkjob(&node, &format!("recipe-import {}", super::q(&folder)), None, Duration::from_secs(30)).await?
    } else {
        serde_json::json!({"recipe": super::recipe::by_id(super::recipe::GENERIC), "modelPath": "/home/sparkuser/models/Qwen/Qwen3-1.7B", "servedName": "demo-import", "tensorParallel": 1, "pipelineParallel": 1, "warnings": ["Demo fixture; no script was read."]})
    };
    let mut recipe: super::recipe::Recipe = serde_json::from_value(draft["recipe"].clone()).map_err(|_| "The script could not be translated into a recipe.")?;
    recipe.id = format!("imported-{}", &crate::live::sha256_hex(serde_json::to_string(&recipe).map_err(|e| e.to_string())?.as_bytes())[..12]);
    if recipe.matches.is_empty() { recipe.matches.push("imported".into()); }
    super::recipe::validate(&recipe)?;
    draft["recipe"] = serde_json::to_value(recipe).map_err(|e| e.to_string())?;
    Ok(draft)
}

/// Every 3 s: one endpoint round, pushed to the UI.
pub fn start_watch(app: &AppHandle) {
    if let Some(tasks) = app.try_state::<Arc<crate::tasks::Tasks>>() {
        app.state::<AppState>().models.set_tasks(tasks.inner().clone());
    }
    if let Some(health) = app.try_state::<Arc<crate::health::Health>>() {
        app.state::<AppState>().models.set_health(health.inner().clone());
    }
    if let Some(gate) = app.try_state::<Arc<admin::AdminGate>>() {
        app.state::<AppState>().models.set_gate(gate.inner().clone());
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(super::metrics::SCRAPE_SECS));
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            every.tick().await;
            round(&handle, false).await;
        }
    });
}

async fn round(app: &AppHandle, force: bool) -> Vec<Endpoint> {
    let state = app.state::<AppState>();
    let ctx = models_ctx(&state);
    let models: Arc<Models> = state.models.clone();
    models.set_stall_after(stall_after_s(app));
    let list = models.endpoints_tick(&ctx, force).await;
    if let Some(history) = app.try_state::<Arc<crate::history::History>>() {
        record_canaries(&models, &history);
    }
    let _ = app.emit(ENDPOINTS_EVENT, &list);
    // Reads run independently of endpoint polling; Cache::reading keeps them single-flight.
    let reading = models.clone();
    let read_ctx = ctx.clone();
    tauri::async_runtime::spawn(async move { reading.read_postmortems(&read_ctx).await; });
    let deployments = models.cluster_deployments(&ctx);
    if models.deployments_changed(&ctx, &deployments) {
        let _ = app.emit(DEPLOYMENTS_EVENT, &deployments);
    }
    list
}

/// Each test token's time goes into its endpoint's history (canaryMs), next to the TTFT p95 the
/// health engine records from the endpoint list. Returns how many the history kept.
pub fn record_canaries(models: &Models, history: &crate::history::History) -> usize {
    let mut kept = 0;
    for (id, at_ms, ms) in models.take_new_canaries() {
        // Refused only by a history that doesn't keep canaryMs; the card still shows the token.
        if history.record_entity(&format!("endpoint:{id}"), "canaryMs", at_ms as i64, ms as f32).is_ok() {
            kept += 1;
        }
    }
    kept
}

/// How long requests may run with no tokens before an endpoint is Stalled: the endpoint-stalled
/// rule's wait when the rules have one (0: endpoints::STALL_AFTER_S).
fn stall_after_s(app: &AppHandle) -> u64 {
    app.try_state::<Arc<crate::health::Health>>()
        .and_then(|h| h.rules().rules.iter().find(|r| r.id == "endpoint-stalled" && r.for_s > 0).map(|r| r.for_s))
        .unwrap_or(0)
}

#[tauri::command]
pub fn endpoints_list(state: State<AppState>) -> Vec<Endpoint> {
    state.models.endpoints()
}

/// Probes every endpoint now instead of waiting for the next round.
#[tauri::command]
pub async fn endpoints_refresh(app: AppHandle) -> Result<Vec<Endpoint>, String> {
    Ok(round(&app, true).await)
}

/// The memory plan and port check for a load, exactly as `deploy` will apply them.
#[tauri::command]
pub async fn deploy_plan(state: State<'_, AppState>, spec: DeploySpec) -> Result<DeployPlan, String> {
    let ctx = models_ctx(&state);
    state.models.plan(&ctx, &spec).await
}

/// "Before it starts" on the Load page (P2.7): the chosen nodes read in parallel and judged as one
/// set, the same read `deploy` makes. Nothing changes and nothing is pinged. Demo reads nothing
/// (`verdict: "demo"`).
#[tauri::command]
pub async fn model_preflight(state: State<'_, AppState>, spec: DeploySpec) -> Result<PreflightSet, String> {
    let ctx = models_ctx(&state);
    state.models.preflight(&ctx, &spec).await
}

/// The load recipes and what each has been verified on (Settings › Models).
#[tauri::command]
pub fn recipes_list(state: State<AppState>) -> Vec<super::recipe::RecipeView> {
    state.models.recipes()
}

/// Removes what earlier loads left on `nodes` (P2.9): each id's containers, launch files and key.
/// An id that isn't this build's, or that's a running download, is refused with nothing removed.
/// Returns the task's id.
#[tauri::command]
pub async fn deployment_clean_leftovers(app: AppHandle, state: State<'_, AppState>, nodes: Vec<String>, ids: Vec<String>) -> Result<String, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.clean_leftovers(&ctx, nodes, ids).await
}

/// Stops the containers behind an endpoint SparkBox didn't start (the UI names them first).
/// SparkBox's own loads are unloaded instead, which also removes their workers.
#[tauri::command]
pub async fn endpoint_stop(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    admin::require(&app)?;
    let e = state.models.endpoints().into_iter().find(|e| e.id == id).ok_or("That endpoint isn't there any more.")?;
    if e.managed_by == ManagedBy::Sparkbox {
        return Err("SparkBox started this one: unload it instead, so its workers stop too.".into());
    }
    if e.container_ids.is_empty() {
        return Err(format!("{} on {} isn't a container (it runs as a service or a plain process). Stop it in Services or Workloads.", e.engine.label(), e.head_node));
    }
    for cid in &e.container_ids {
        if is_live(&state) {
            state.live.container_action(&e.head_node, cid, "stop").await?;
        } else {
            state.demo.lock().unwrap().container_action(&e.head_node, cid, "stop")?;
            state.live.audit.record_simulated(&e.head_node, &format!("Stop {} ({})", e.container_names.join(", "), e.engine.label()), &format!("docker stop {cid}"));
        }
    }
    crate::publish(&app, false);
    round(&app, true).await;
    Ok(())
}

/// Restarts an endpoint SparkBox didn't start, in order, as a task that verifies it answers.
/// Returns the task's id.
#[tauri::command]
pub async fn endpoint_restart(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<String, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    let models: Arc<Models> = state.models.clone();
    if !models.endpoints().iter().any(|e| e.id == id && e.managed_by == ManagedBy::External && e.members.iter().any(|m| m.head)) {
        return Err("Only a server that runs in containers SparkBox didn't start can be restarted here.".into());
    }
    models.restart_external(&ctx, &id).await
}

/// Stops listing an endpoint that is down or stopped and that SparkBox didn't start (an
/// exited head, or one that vanished). It's listed again if it starts.
#[tauri::command]
pub async fn endpoint_forget(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    admin::require(&app)?;
    let name = state.models.forget_endpoint(&id)?;
    state.live.audit.record_app(&format!("Forgot endpoint {name}"), &id);
    let _ = app.emit(ENDPOINTS_EVENT, &state.models.endpoints());
    Ok(())
}

// ---- Hugging Face

/// A search row's served-weights size from the repo's files (the detail's own reading), kept a day.
#[tauri::command]
pub async fn hf_size(state: State<'_, AppState>, repo: String) -> Result<super::hf::Sizing, String> {
    state.models.hf_size(&repo).await
}

// ---- Ollama

#[tauri::command]
pub async fn ollama_list(state: State<'_, AppState>, endpoint: String) -> Result<super::ollama::OllamaView, String> {
    let ctx = models_ctx(&state);
    state.models.ollama_list(&ctx, &endpoint).await
}

/// Starts pulling a model as a task; returns the task's id.
#[tauri::command]
pub async fn ollama_pull(app: AppHandle, state: State<'_, AppState>, endpoint: String, name: String) -> Result<String, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    let tasks = app.state::<Arc<crate::tasks::Tasks>>().inner().clone();
    state.models.ollama_pull(&ctx, &endpoint, name.trim(), tasks).await
}

#[tauri::command]
pub async fn ollama_delete(app: AppHandle, state: State<'_, AppState>, endpoint: String, name: String) -> Result<(), String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    state.models.ollama_delete(&ctx, &endpoint, &name).await?;
    round(&app, true).await;
    Ok(())
}

// ---- Benchmarks

/// Starts a benchmark sweep as a task; returns the task's id.
#[tauri::command]
pub async fn bench_run(app: AppHandle, state: State<'_, AppState>, endpoint: String, model: String, spec: super::bench::BenchSpec) -> Result<String, String> {
    crate::admin::require(&app)?;
    let ctx = models_ctx(&state);
    let tasks = app.state::<Arc<crate::tasks::Tasks>>().inner().clone();
    state.models.run_benchmark(&ctx, &endpoint, &model, spec, tasks).await
}

/// Saves a benchmark to Downloads as CSV or JSON; returns the file's path.
#[tauri::command]
pub fn bench_export(app: AppHandle, state: State<AppState>, at: u64, format: String) -> Result<String, String> {
    let dir = app.path().download_dir().map_err(|e| e.to_string())?;
    state.models.export_benchmark(at, &format, &dir)
}

/// Starts a SparkBox load again with the same settings. Returns the task's id.
#[tauri::command]
pub async fn deployment_restart(app: AppHandle, state: State<'_, AppState>, id: String, drop_caches_once: Option<bool>) -> Result<String, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    if state.models.deployments(&ctx).iter().any(|d| d.spec.id == id && d.recovery == Some(super::recovery::Recovery::Maintenance)) {
        return Err("This load is paused for rolling maintenance. Resume or restore it from Software Updates.".into());
    }
    state.models.restart_with_cache_consent(&ctx, &id, drop_caches_once.unwrap_or(false)).await
}

// ---- Deployments on the cluster (r4-serving)

/// Every deployment on the cluster, inferred or SparkBox's, with the postmortems read so far. The
/// first call that lists a failed one reads its postmortem in the background (docker inspect and
/// `docker logs --tail 600` of its head, and its launch script: reads only), then sends
/// deployments://changed.
#[tauri::command]
pub fn cluster_deployments_list(app: AppHandle, state: State<AppState>, retry: Option<String>) -> Vec<ClusterDeployment> {
    let ctx = models_ctx(&state);
    if let Some(id) = retry { state.models.retry_postmortem(&ctx, &id); }
    let list = state.models.cluster_deployments(&ctx);
    if list.iter().any(|d| d.state == "failed" && d.postmortem.as_ref().is_none_or(|p| p.retryable)) {
        let models: Arc<Models> = state.models.clone();
        tauri::async_runtime::spawn(async move {
            if models.read_postmortems(&ctx).await {
                let ctx = models_ctx(&app.state::<AppState>());
                let list = models.cluster_deployments(&ctx);
                models.deployments_changed(&ctx, &list);
                let _ = app.emit(DEPLOYMENTS_EVENT, &list);
            }
        });
    }
    list
}

/// How long `model_restart_after` waits for its node to come back.
const BACK_WITHIN: Duration = Duration::from_secs(15 * 60);

/// After a restart of `node` that SparkBox started: a task that waits until the node has booted
/// again and reports, then restarts the endpoint in order (that restart's own task ends with
/// Verify). Returns the waiting task's id. A relock (not an idle one) or look-only before
/// the restart cancels it: nothing is restarted.
#[tauri::command]
pub async fn model_restart_after(app: AppHandle, state: State<'_, AppState>, node: String, endpoint_id: String) -> Result<String, String> {
    admin::require(&app)?;
    let models: Arc<Models> = state.models.clone();
    let e = models.endpoints().into_iter().find(|e| e.id == endpoint_id).ok_or("That endpoint isn't listed any more.")?;
    if !e.nodes.contains(&node) && e.head_node != node {
        return Err(format!("{} doesn't run on {node}.", e.label));
    }
    let restartable = e.deployment_id.is_some() || e.members.iter().any(|m| m.head);
    if !restartable {
        return Err(format!("{} isn't a container SparkBox can restart.", e.label));
    }
    let gate = app.state::<Arc<admin::AdminGate>>().inner().clone();
    let scheduled = gate.hold_generation();
    let tasks = app.state::<Arc<crate::tasks::Tasks>>().inner().clone();
    let title = format!("Restart {} in order once {node} is back", e.label);
    let task = tasks.start(crate::tasks::TaskSpec::new("model", title, &e.nodes).cancellable());
    let task_id = task.id().to_string();
    let boot_before = models_ctx(&state).snapshot.and_then(|s| s.nodes.into_iter().find(|n| n.id == node)).and_then(|n| n.boot_id);
    tauri::async_runtime::spawn(async move {
        let look = || {
            let snap = models_ctx(&app.state::<AppState>()).snapshot;
            let n = snap.as_ref().and_then(|s| s.nodes.iter().find(|n| n.id == node));
            let reporting = n.is_some_and(|n| n.status == crate::model::LinkStatus::Online && n.power_state.is_none() && !(n.stale && n.frame_age_ms.unwrap_or(0) > 10_000));
            NodeLook { reporting, boot_id: n.and_then(|n| n.boot_id.clone()) }
        };
        let restart = || async {
            let ctx = models_ctx(&app.state::<AppState>());
            match (&e.deployment_id, e.managed_by) {
                (Some(d), ManagedBy::Sparkbox) => models.restart(&ctx, d).await,
                _ => models.restart_external(&ctx, &e.id).await,
            }
        };
        let wait = Wait { every: Duration::from_secs(3), within: BACK_WITHIN };
        let target = Target { node: &node, label: &e.label, boot_before };
        wait_then_restart(&gate, scheduled, &task, target, look, wait, restart).await;
    });
    Ok(task_id)
}

/// How a node looks to a restart waiting for it.
pub(crate) struct NodeLook {
    /// Online, not in a power window, and its frames fresh.
    pub reporting: bool,
    pub boot_id: Option<String>,
}

/// What a queued restart waits for: `node` to boot again (its boot id moves off `boot_before`),
/// then `label` restarts.
pub(crate) struct Target<'a> {
    pub node: &'a str,
    pub label: &'a str,
    pub boot_before: Option<String>,
}

/// How often `wait_then_restart` looks, and for how long at most.
pub(crate) struct Wait {
    pub every: Duration,
    pub within: Duration,
}

/// The waiting half of `model_restart_after`: looks at `node` (`look`) until it has booted again
/// and reports, then checks the gate one last time and runs `restart` once. Something that held
/// work since it was scheduled (`scheduled`, the gate's hold generation then) cancels it with a
/// message and restarts nothing: look-only, or a relock other than an idle one.
pub(crate) async fn wait_then_restart<F, Fut>(gate: &admin::AdminGate, scheduled: u64, task: &crate::tasks::TaskHandle, target: Target<'_>, look: impl Fn() -> NodeLook, wait: Wait, restart: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let Target { node, label, boot_before } = target;
    task.step(format!("Waiting for {node} to come back"));
    let started = std::time::Instant::now();
    let mut went_away = false;
    loop {
        if task.is_cancelled() {
            return task.finish_err("Canceled before the restart.");
        }
        if started.elapsed() > wait.within {
            return task.finish_err(format!("{node} didn't come back within {} minutes, so {label} wasn't restarted.", wait.within.as_secs() / 60));
        }
        let n = look();
        went_away |= !n.reporting;
        // Back means booted again: a new boot id, or gone and returned when the node has none.
        let rebooted = match (&boot_before, &n.boot_id) {
            (Some(before), Some(now)) => before != now,
            _ => went_away,
        };
        if n.reporting && rebooted {
            break;
        }
        tokio::time::sleep(wait.every).await;
    }
    task.step_ok(Some(format!("{node} is back")));
    // Right before acting: a relock or look-only since it was queued means the owner isn't
    // standing behind this restart any more.
    if let Some(hold) = gate.hold_view().held_since(scheduled) {
        let why = match hold {
            admin::Hold::Locked => "Locked before the restart; nothing was restarted",
            admin::Hold::LookOnly => "Look-only turned on before the restart; nothing was restarted",
        };
        task.step_skipped(format!("Restart {label} in order"), why);
        return task.finish_cancelled(why);
    }
    task.step(format!("Restart {label} in order"));
    match restart().await {
        Ok(_) => task.finish_ok(format!("{node} is back; {label} is restarting in order")),
        Err(why) => task.finish_err(why),
    }
}

/// The Library's detail pane for one folder: its config's architecture, quantisation and
/// attention layout (cat config.json and du on a node holding it, read once).
#[tauri::command]
pub async fn library_detail(state: State<'_, AppState>, key: String) -> Result<super::LibraryDetail, String> {
    if !key.starts_with('/') || key.contains("..") {
        return Err("That isn't a model folder.".into());
    }
    let ctx = models_ctx(&state);
    Ok(state.models.library_detail(&ctx, &key).await)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::admin::{testing, RelockReason};
    use crate::tasks::{TaskSpec, TaskState, Tasks};

    /// A restart queued behind cell3's reboot: the node goes away, and while it's gone the gate
    /// relocks for `why`; then it's back with a new boot id. Returns the task and how many
    /// times the restart ran.
    async fn queued_restart_with_a_relock(why: RelockReason) -> (crate::tasks::Task, usize) {
        let (gate, clock, _) = testing::gate();
        gate.unlock(None).unwrap();
        let scheduled = gate.hold_generation();
        let tasks = Tasks::new(None);
        let task = tasks.start(TaskSpec::new("model", "Restart llama-3.3-70b in order once cell3 is back", &["cell3".into()]).cancellable());
        let looks = Mutex::new(0);
        let look = || {
            let mut n = looks.lock().unwrap();
            *n += 1;
            if *n == 1 {
                // While cell3 reboots: an idle relock, or the owner locks, the screen locks, the Mac sleeps.
                if why == RelockReason::Idle {
                    clock.advance(16);
                    assert!(gate.tick());
                } else {
                    gate.lock_with(why, "relocked while waiting");
                }
                return NodeLook { reporting: false, boot_id: None };
            }
            NodeLook { reporting: true, boot_id: Some("boot-2".into()) }
        };
        let restarted = AtomicUsize::new(0);
        let restart = || async {
            restarted.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>("t-restart".to_string())
        };
        let wait = Wait { every: Duration::from_millis(5), within: Duration::from_secs(5) };
        let target = Target { node: "cell3", label: "llama-3.3-70b", boot_before: Some("boot-1".into()) };
        wait_then_restart(&gate, scheduled, &task, target, look, wait, restart).await;
        (tasks.get(task.id()).unwrap(), restarted.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn a_queued_restart_is_cancelled_by_a_relock_and_restarts_nothing() {
        for why in [RelockReason::Explicit, RelockReason::ScreenLock, RelockReason::Sleep, RelockReason::Quit] {
            let (t, restarted) = queued_restart_with_a_relock(why).await;
            assert_eq!(restarted, 0, "{why:?}: nothing was restarted");
            assert_eq!(t.state, TaskState::Cancelled, "{why:?}");
            assert_eq!(t.summary.as_deref(), Some("Locked before the restart; nothing was restarted"), "{why:?}");
            assert_eq!(t.steps.last().map(|s| s.state), Some(crate::tasks::StepState::Skipped));
        }
    }

    #[tokio::test]
    async fn an_idle_relock_doesnt_cancel_a_queued_restart() {
        let (t, restarted) = queued_restart_with_a_relock(RelockReason::Idle).await;
        assert_eq!(restarted, 1);
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Ok, Some("cell3 is back; llama-3.3-70b is restarting in order")));
    }

    #[tokio::test]
    async fn look_only_cancels_a_queued_restart_before_it_acts() {
        let (gate, _, _) = testing::gate();
        let scheduled = gate.hold_generation();
        let tasks = Tasks::new(None);
        let hook = tasks.clone();
        gate.on_hold(move |why| hook.hold_non_read(why));
        let task = tasks.start(TaskSpec::new("model", "Restart llama-3.3-70b in order once cell3 is back", &["cell3".into()]).cancellable());
        let looks = Mutex::new(0);
        let look = || {
            let mut n = looks.lock().unwrap();
            *n += 1;
            if *n == 1 {
                gate.set_look_only(true).unwrap();
                return NodeLook { reporting: false, boot_id: None };
            }
            NodeLook { reporting: true, boot_id: Some("boot-2".into()) }
        };
        let restarted = AtomicUsize::new(0);
        let restart = || async {
            restarted.fetch_add(1, Ordering::SeqCst);
            Ok::<_, String>(String::new())
        };
        let wait = Wait { every: Duration::from_millis(5), within: Duration::from_secs(5) };
        let target = Target { node: "cell3", label: "llama-3.3-70b", boot_before: Some("boot-1".into()) };
        wait_then_restart(&gate, scheduled, &task, target, look, wait, restart).await;
        let t = tasks.get(task.id()).unwrap();
        assert_eq!(restarted.load(Ordering::SeqCst), 0);
        assert_eq!((t.state, t.error.as_deref()), (TaskState::Cancelled, Some("Look-only turned on: stopped before its next step")));
    }
}
