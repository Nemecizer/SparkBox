//! One routine upgrade at a time, with a durable, owner-resumed checkpoint.
//! Child apt jobs retain their existing detached lifecycle. Cancellation never kills dpkg.
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use super::{Ops, updates::{PlanEntry, UpgradeArgs}};
use crate::{admin, models_ctx, AppState};
use crate::models::{Ctx, Models, DeploymentState};
use crate::tasks::{TaskHandle, TaskSpec, TaskState};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodePlan {
    pub node: String, pub packages: Vec<String>, pub versions: Vec<PlanEntry>,
    pub restart: bool, pub phase: String, pub child: Option<String>, pub admitted_ms: u64,
    pub boot_before: Option<String>, pub kernel: Option<String>, pub driver: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    pub id: String, pub demo: bool, pub instance: String, pub created_ms: u64,
    pub nodes: Vec<NodePlan>, pub loads: Vec<String>, pub paused: Vec<String>,
    pub phase: String, pub message: String,
}
fn path(ops: &Ops, demo: bool) -> std::path::PathBuf { ops.live.data_path(if demo { "rolling-demo.json" } else { "rolling.json" }) }
fn save(ops: &Ops, plan: &Plan) -> Result<(), String> { crate::util::save_json(&path(ops, plan.demo), plan).map_err(|e| format!("Couldn't save the maintenance checkpoint: {e}")) }
fn read(ops: &Ops, demo: bool) -> Result<Option<Plan>, String> {
    let path = path(ops, demo);
    if !path.exists() { return Ok(None); }
    let plan: Plan = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| format!("The maintenance checkpoint needs review: {e}"))?;
    if plan.demo != demo || plan.instance != crate::identity::instance_name() { return Err("This maintenance plan belongs to another source or SparkBox instance.".into()); }
    Ok(Some(plan))
}
/// Durable restart reservations prevent a new load racing the last pre-reboot check.
pub(crate) fn check_load(live: &crate::live::Live, demo: bool, nodes: &[String]) -> Result<(), String> {
    super::protected_updates::check_load(live, demo, nodes)?;
    let path = live.data_path(if demo { "rolling-demo.json" } else { "rolling.json" });
    if !path.exists() { return Ok(()); }
    let p: Plan = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|_| "Review the unreadable maintenance plan in Software Updates.")?;
    if p.phase != "done" && p.nodes.iter().any(|n| n.restart && nodes.contains(&n.node)) {
        return Err("A rolling maintenance plan reserves these restart nodes. Finish or end it in Software Updates before loading or restarting a model.".into());
    }
    Ok(())
}
pub fn ordered(mut nodes: Vec<String>, canary: Option<&str>, dependents: impl Fn(&str) -> usize) -> Result<Vec<String>, String> {
    if nodes.is_empty() || nodes.len() > 4 || nodes.iter().collect::<std::collections::HashSet<_>>().len() != nodes.len() { return Err("Choose one to four distinct inventory nodes.".into()); }
    if canary.is_some_and(|c| !nodes.iter().any(|n| n == c)) { return Err("The canary must be a selected node.".into()); }
    let first = canary.map(String::from).or_else(|| nodes.iter().find(|n| n.as_str() == "cell2").cloned()).unwrap_or_else(|| nodes.iter().min_by_key(|n| (dependents(n), *n)).unwrap().clone());
    nodes.sort_by_key(|n| (n != &first, dependents(n), n.clone())); Ok(nodes)
}

impl Ops {
    async fn rolling_preview(&self, ctx: &Ctx, models: &Models, nodes: Vec<String>, canary: Option<&str>, restart: &[String]) -> Result<Plan, String> {
        let deployments = models.deployments(ctx);
        let nodes = ordered(nodes, canary, |n| deployments.iter().filter(|d| d.state.active() && d.spec.nodes.iter().any(|x| x == n)).count())?;
        if nodes.iter().any(|n| !ctx.node_ids().contains(n)) || restart.iter().any(|n| !nodes.contains(n)) { return Err("Choose inventory nodes, with restarts limited to those selected.".into()); }
        let loads: Vec<_> = deployments.iter().filter(|d| d.state.active() && d.spec.nodes.iter().any(|n| restart.contains(n))).map(|d| d.spec.id.clone()).collect();
        let external = models.endpoints().into_iter().any(|e| e.nodes.iter().any(|n| restart.contains(n)) && e.deployment_id.as_ref().is_none_or(|id| !loads.contains(id)) && !e.members.is_empty());
        if external { return Err("A selected restart affects a server SparkBox cannot restore. Stop or exclude it before making this plan.".into()); }
        let mut plan = Plan { id: format!("rolling-{}", super::now_ms()), demo: !ctx.live, instance: crate::identity::instance_name().into(), created_ms: super::now_ms(), loads, phase: "preview".into(), ..Default::default() };
        for node in nodes {
            let status = self.updates_status(!ctx.live, &node).await.map_err(|e| e.message)?;
            let packages: Vec<_> = status.pending.iter().filter(|p| !p.held && !p.sensitive).map(|p| p.name.clone()).collect();
            let versions = if packages.is_empty() { Vec::new() } else {
                let sim = self.updates_simulate(!ctx.live, &node, &packages).await.map_err(|e| e.message)?;
                if let Some(why) = sim.refused { return Err(format!("{node}: {why}")); }
                if !sim.sensitive.is_empty() || !sim.removes.is_empty() { return Err(format!("{node}: the routine plan would change the GPU stack or remove packages.")); }
                sim.shown()
            };
            plan.nodes.push(NodePlan { restart: restart.contains(&node), node, packages, versions, phase: "pending".into(), ..Default::default() });
        }
        Ok(plan)
    }

    async fn child_done(&self, id: &str) -> Result<(), String> {
        // Child work already admitted is followed to an outcome even if the parent is cancelled.
        // No new action follows a cancellation; apt/dpkg is never killed here.
        let until = tokio::time::Instant::now() + Duration::from_secs(4 * 60 * 60);
        loop {
            let task = self.tasks.get(id).ok_or("The child task is no longer in history; review it before resuming.")?;
            match task.state {
                TaskState::Ok => return task.warning.map_or(Ok(()), Err),
                TaskState::Failed | TaskState::Cancelled => return Err(task.error.or(task.summary).unwrap_or("The child operation did not complete.".into())),
                TaskState::Running => {}
            }
            if tokio::time::Instant::now() >= until { return Err("The child task is still running after four hours; inspect it before resuming.".into()); }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn rolling_run(self: &Arc<Self>, ctx: &Ctx, models: &Arc<Models>, plan: &mut Plan, task: &TaskHandle, restore_only: bool) -> Result<(), String> {
        let boundary = || if task.is_cancelled() { Err("Stopped between steps. An admitted package job is left to finish; resume or restore stopped models explicitly.".to_string()) } else { Ok(()) };
        if restore_only {
            // Never abandon a running installer or reboot and overlap it with a restore/new plan.
            for n in &plan.nodes {
                if matches!(n.phase.as_str(), "admitting" | "installing") {
                    let run = self.runs().into_iter().find(|r| r.node == n.node && r.simulated == plan.demo && r.started_ms >= n.admitted_ms && r.packages == n.packages);
                    let id = run.map(|r| r.task_id).or_else(|| n.child.clone()).ok_or("Upgrade admission is unresolved. Review its node job before ending the plan.")?;
                    let child = self.tasks.get(&id).ok_or("The saved child task is absent. Review its node job before ending the plan.")?;
                    if child.state == TaskState::Running { return Err("A package job is still running. Wait for it to finish before ending the plan.".into()); }
                }
                if n.phase == "rebooting" {
                    if n.child.as_ref().and_then(|id| self.tasks.get(id)).is_some_and(|t| t.state == TaskState::Running) { return Err("A restart task is still running; wait for its outcome before ending the plan.".into()); }
                    let current = self.node_view(&n.node).ok_or("The restart node is unavailable.")?;
                    if current.stale || current.status != crate::model::LinkStatus::Online { return Err("Wait for fresh online reports from every restart node before restoring models.".into()); }
                }
            }
        }
        if !restore_only {
            for i in 0..plan.nodes.len() {
                boundary()?;
                let n = plan.nodes[i].clone();
                if n.phase == "pending" {
                    if n.versions.is_empty() { plan.nodes[i].phase = "upgraded".into(); save(self, plan)?; continue; }
                    // Persist intent first: a crash between admission and saving the returned id
                    // is reconciled against the existing upgrade run, never repeated blindly.
                    plan.nodes[i].phase = "admitting".into(); plan.nodes[i].admitted_ms = super::now_ms(); save(self, plan)?;
                    let id = self.updates_upgrade_opts(plan.demo, n.node.clone(), n.packages.clone(), UpgradeArgs { plan: Some(n.versions), ..Default::default() }).map_err(|e| e.message)?;
                    plan.nodes[i].child = Some(id); plan.nodes[i].phase = "installing".into(); save(self, plan)?;
                }
                if matches!(plan.nodes[i].phase.as_str(), "admitting" | "installing") {
                    let n = &plan.nodes[i];
                    // Reattached node jobs get a new task id. Bind only to this saved run.
                    let run = self.runs().into_iter().find(|r| r.node == n.node && r.simulated == plan.demo && r.started_ms >= n.admitted_ms && r.packages == n.packages);
                    let child = run.as_ref().map(|r| r.task_id.clone()).or_else(|| n.child.clone()).ok_or("Upgrade admission was interrupted. Inspect Updates history before replacing this plan; nothing is repeated automatically.")?;
                    task.step(format!("Routine updates on {}", self.name_of(&n.node)));
                    self.child_done(&child).await?;
                    task.step_ok(None); plan.nodes[i].phase = "upgraded".into(); plan.nodes[i].child = None; save(self, plan)?;
                }
            }
            // Upgrade every selected node in place first. Only the explicitly selected restart
            // nodes take models down, and a spanning load stays paused until its last member.
            for i in 0..plan.nodes.len() {
                boundary()?;
                if !plan.nodes[i].restart || plan.nodes[i].phase == "done" { plan.nodes[i].phase = "done".into(); save(self, plan)?; continue; }
                let node = plan.nodes[i].node.clone();
                if plan.nodes[i].phase == "upgraded" {
                    let current = models.deployments(ctx);
                    let affected: Vec<_> = current.iter().filter(|d| d.state != DeploymentState::Stopped && d.spec.nodes.contains(&node)).map(|d| d.spec.id.clone()).collect();
                    if affected.iter().any(|id| !plan.loads.contains(id)) { return Err("A new load appeared on a restart node. Make a fresh maintenance preview.".into()); }
                    let external = models.endpoints().into_iter().any(|e| e.nodes.contains(&node) && e.deployment_id.as_ref().is_none_or(|id| !plan.loads.contains(id)) && !e.members.is_empty());
                    if external { return Err("An external server appeared on a restart node; stop and review.".into()); }
                    for id in affected {
                        boundary()?;
                        if !plan.paused.contains(&id) { plan.paused.push(id.clone()); save(self, plan)?; }
                        // Repeat the ownership-checked stop if admission was interrupted. Empty
                        // label selections are no-ops; ranks stopped earlier are never restarted.
                        models.pause_for_maintenance(ctx, &id).await?;
                    }
                    boundary()?;
                    let before = self.node_view(&node).ok_or("The restart node is unavailable.")?;
                    plan.nodes[i].boot_before = before.boot_id;
                    plan.nodes[i].driver = Some(before.system.driver);
                    if ctx.live {
                        let checked = super::prereboot::check(&self.live, &node).await.map_err(|e| e.message)?;
                        if !checked.ok { return Err(checked.blocking.join("; ")); }
                        plan.nodes[i].kernel = checked.kernel;
                    }
                    plan.nodes[i].phase = "rebooting".into(); save(self, plan)?;
                    let child = self.power_with_models(plan.demo, &node, "reboot", 0, false).map_err(|e| e.message)?;
                    plan.nodes[i].child = Some(child.clone()); save(self, plan)?;
                    self.child_done(&child).await?;
                } else if plan.nodes[i].phase == "rebooting" {
                    let n = &plan.nodes[i];
                    let current = self.node_view(&node).ok_or("The rebooted node is unavailable; resume when it returns.")?;
                    if current.stale || current.status != crate::model::LinkStatus::Online || n.boot_before.is_none() || current.boot_id.is_none() || n.boot_before == current.boot_id {
                        return Err("The saved reboot has no fresh new-boot evidence. Review the power task; no second reboot was sent.".into());
                    }
                    self.postboot.lock().unwrap().insert(node.clone(), super::postboot::Expected { boot: n.boot_before.clone(), kernel: n.kernel.clone(), driver: n.driver.clone(), peers: self.cluster().unwrap_or_default().nodes.iter().filter(|p| p.id != node && p.status == crate::model::LinkStatus::Online && !p.stale && !p.system.driver.is_empty()).map(|p| (p.id.clone(), p.system.driver.clone())).collect() });
                    if let Some(warning) = self.checks_after_restart(plan.demo, task, &node).await { return Err(warning); }
                } else { return Err(format!("Unexpected saved phase for {node}: {}", plan.nodes[i].phase)); }
                plan.nodes[i].phase = "done".into(); plan.nodes[i].child = None; save(self, plan)?;
                self.restore_ready(ctx, models, plan, task, false).await?;
            }
        }
        boundary()?;
        self.restore_ready(ctx, models, plan, task, true).await?;
        plan.phase = "done".into(); plan.message = if restore_only { "Stopped models restored; remaining package/restart steps were abandoned." } else { "Routine updates and the selected restarts completed." }.into(); save(self, plan)
    }

    async fn restore_ready(&self, ctx: &Ctx, models: &Arc<Models>, plan: &mut Plan, task: &TaskHandle, all: bool) -> Result<(), String> {
        for id in plan.paused.clone() {
            if task.is_cancelled() { return Err("Stopped before restoring the next model. Resume or restore it explicitly.".into()); }
            let d = models.deployments(ctx).into_iter().find(|d| d.spec.id == id);
            if d.as_ref().is_none_or(|d| d.state == DeploymentState::Stopped) {
                task.step_skipped(format!("Restore {id}"), "The owner unloaded or forgot it; it will not be relaunched.");
                plan.paused.retain(|p| p != &id); save(self, plan)?; continue;
            }
            let d = d.unwrap();
            if !all && plan.nodes.iter().any(|n| n.restart && n.phase != "done" && d.spec.nodes.contains(&n.node)) { continue; }
            task.step(format!("Restore {} in order", d.spec.served_name));
            models.restore_after_maintenance(ctx, &id).await?;
            task.step_ok(None); plan.paused.retain(|p| p != &id); save(self, plan)?;
        }
        Ok(())
    }

    fn start_rolling(self: &Arc<Self>, ctx: Ctx, models: Arc<Models>, mut plan: Plan, restore_only: bool) -> Result<String, String> {
        if self.rolling_busy.swap(true, Ordering::SeqCst) { return Err("Rolling maintenance is already running.".into()); }
        if plan.nodes.iter().any(|n| !ctx.node_ids().contains(&n.node)) || plan.demo == ctx.live { self.rolling_busy.store(false, Ordering::SeqCst); return Err("The source or inventory changed; review the saved plan.".into()); }
        plan.phase = "running".into(); plan.message.clear();
        if let Err(e) = save(self, &plan) { self.rolling_busy.store(false, Ordering::SeqCst); return Err(e); }
        let nodes = plan.nodes.iter().map(|n| n.node.clone()).collect::<Vec<_>>();
        let task = self.tasks.start(TaskSpec::new("updates", "Update nodes one by one", &nodes).cancellable());
        let id = task.id().to_string(); let me = self.clone();
        tauri::async_runtime::spawn(async move {
            struct Release(Arc<Ops>); impl Drop for Release { fn drop(&mut self) { self.0.rolling_busy.store(false, Ordering::SeqCst); } }
            let _release = Release(me.clone());
            let result = me.rolling_run(&ctx, &models, &mut plan, &task, restore_only).await;
            match result {
                Ok(()) => task.finish_ok(plan.message.clone()),
                Err(e) => { plan.phase = "paused".into(); plan.message = crate::live::audit::redact(&e); let persisted = save(&me, &plan); let message = persisted.err().map(|s| format!("{}; {s}", plan.message)).unwrap_or(plan.message); if task.is_cancelled() { task.finish_cancelled(message); } else { task.finish_err(message); } }
            }
        });
        Ok(id)
    }
}

#[tauri::command]
pub async fn updates_rolling_preview(state: State<'_, AppState>, nodes: Vec<String>, canary: Option<String>, restart: Vec<String>) -> Result<Plan, String> {
    state.ops.rolling_preview(&models_ctx(&state), &state.models, nodes, canary.as_deref(), &restart).await
}
#[tauri::command]
pub fn updates_rolling_status(state: State<'_, AppState>) -> Result<Option<Plan>, String> {
    let mut p = read(&state.ops, !crate::is_live(&state))?;
    if !state.ops.rolling_busy.load(Ordering::SeqCst) { if let Some(p) = &mut p { if p.phase == "running" { p.phase = "paused".into(); p.message = "SparkBox stopped following this plan. Resume explicitly to reconcile the current node job.".into(); } } }
    Ok(p)
}
#[tauri::command]
pub async fn updates_rolling_start(app: AppHandle, state: State<'_, AppState>, plan: Plan) -> Result<String, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    if read(&state.ops, !ctx.live)?.is_some_and(|p| p.phase != "done") { return Err("Resume or restore the existing maintenance plan first.".into()); }
    // Never trust saved/UI package plans without a fresh dry run. Changes require a new preview.
    let fresh = state.ops.rolling_preview(&ctx, &state.models, plan.nodes.iter().map(|n| n.node.clone()).collect(), plan.nodes.first().map(|n| n.node.as_str()), &plan.nodes.iter().filter(|n| n.restart).map(|n| n.node.clone()).collect::<Vec<_>>()).await?;
    if fresh.loads != plan.loads || fresh.nodes.iter().zip(&plan.nodes).any(|(a,b)| a.packages != b.packages || a.versions != b.versions) { return Err("The package plan or affected models changed; preview again.".into()); }
    state.ops.start_rolling(ctx, state.models.clone(), fresh, false)
}
#[tauri::command]
pub fn updates_rolling_resume(app: AppHandle, state: State<'_, AppState>, restore_only: bool) -> Result<String, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state); let plan = read(&state.ops, !ctx.live)?.ok_or("There is no saved maintenance plan.")?;
    if plan.phase == "done" { return Err("The plan is already finished.".into()); }
    state.ops.start_rolling(ctx, state.models.clone(), plan, restore_only)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn canary_order_is_stable_and_invalid_selections_are_refused() {
        assert_eq!(ordered(vec!["cell1".into(), "cell2".into(), "cell3".into()], None, |_|0).unwrap(), ["cell2","cell1","cell3"]);
        assert_eq!(ordered(vec!["a".into(), "b".into()], None, |n|usize::from(n=="a")).unwrap(), ["b","a"]);
        assert!(ordered(vec!["a".into(),"a".into()], None, |_|0).is_err());
        assert!(ordered(vec!["a".into()], Some("b"), |_|0).is_err());
    }
    #[test] fn saved_plan_keeps_child_identity_phases_and_paused_loads() {
        let p = Plan { id:"test".into(), phase:"paused".into(), paused:vec!["tp2".into()], nodes:vec![NodePlan { node:"cell2".into(), phase:"installing".into(), child:Some("apt-1".into()), admitted_ms:1, ..Default::default() }], ..Default::default() };
        let round:Plan=serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(round.nodes[0].child.as_deref(),Some("apt-1")); assert_eq!(round.paused,["tp2"]); assert_eq!(round.phase,"paused");
    }
    fn rig(name: &str) -> (Arc<Ops>, Arc<Models>, Ctx) {
        use std::sync::Mutex;
        let dir = crate::fake_node::temp(name);
        let inv = crate::inventory::Inventory::example();
        let cluster = Arc::new(Mutex::new(crate::demo::DemoCluster::new(&inv)));
        let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_|{});
        let live = crate::live::Live::new(vpn.clone(), dir.join("known_hosts.json"), ||Ok(None));
        let models = Models::new(dir.clone(), live.clone(), crate::llm::Llm::new(dir.join("providers.json"), vpn), Some(cluster.clone()));
        let ops = Ops::with_poll(live, cluster.clone(), crate::tasks::Tasks::new(None), Duration::from_millis(1));
        let src = cluster.clone(); ops.set_snapshot_source(move || { let mut c = src.lock().unwrap(); c.advance(); Some(c.snapshot()) });
        let ctx = Ctx { live:false, settings:Default::default(), inventory:inv, snapshot:Some(cluster.lock().unwrap().snapshot()) };
        (ops, models, ctx)
    }

    #[tokio::test]
    async fn demo_routine_plan_runs_canary_first_and_records_every_child_before_advancing() {
        let (ops, models, ctx) = rig("rolling-demo-all");
        let mut p = ops.rolling_preview(&ctx, &models, vec!["cell1".into(),"cell2".into()], None, &[]).await.unwrap();
        assert_eq!(p.nodes[0].node,"cell2"); assert!(p.nodes.iter().all(|n| n.versions.iter().all(|v| !super::super::updates::sensitive(&v.name))));
        let task = ops.tasks.start(TaskSpec::new("updates", "fixture parent", &[]).cancellable());
        ops.rolling_run(&ctx, &models, &mut p, &task, false).await.unwrap();
        assert_eq!(p.phase,"done"); assert!(p.nodes.iter().all(|n| n.phase == "done"));
        let runs = ops.runs(); assert_eq!(runs.len(),2);
        let first = ops.tasks.get(&runs.iter().find(|r|r.node=="cell2").unwrap().task_id).unwrap();
        let second = ops.tasks.get(&runs.iter().find(|r|r.node=="cell1").unwrap().task_id).unwrap();
        assert!(first.finished()); assert!(first.started_ms <= second.started_ms);
        assert_eq!(read(&ops, true).unwrap().unwrap().phase,"done");
    }

    #[tokio::test]
    async fn saved_child_failure_cancel_and_restore_never_repeat_admitted_work() {
        let (ops, models, ctx) = rig("rolling-boundaries");
        let child = ops.tasks.start(TaskSpec::new("updates", "admitted apt", &["cell2".into()]));
        let mut p = Plan { id:"saved".into(), demo:true, instance:crate::identity::instance_name().into(), phase:"paused".into(), nodes:vec![
            NodePlan {node:"cell2".into(),phase:"installing".into(),child:Some(child.id().into()),restart:true,..Default::default()},
            NodePlan {node:"cell1".into(),phase:"pending".into(),..Default::default()}], ..Default::default() };
        save(&ops,&p).unwrap();
        assert!(check_load(&ops.live,true,&["cell2".into()]).is_err());
        assert!(check_load(&ops.live,false,&["cell2".into()]).is_ok(),"sources stay separate");
        let task = ops.tasks.start(TaskSpec::new("updates","parent",&[]).cancellable());
        assert!(ops.rolling_run(&ctx,&models,&mut p,&task,true).await.unwrap_err().contains("still running"));
        child.finish_err("fixture apt failure");
        p = read(&ops,true).unwrap().unwrap();
        assert!(ops.rolling_run(&ctx,&models,&mut p,&task,false).await.unwrap_err().contains("fixture apt failure"));
        assert_eq!(p.nodes[1].phase,"pending"); assert_eq!(ops.tasks.list().len(),2,"no replacement child");
        ops.tasks.cancel(task.id()).unwrap();
        assert!(ops.rolling_run(&ctx,&models,&mut p,&task,true).await.unwrap_err().contains("Stopped between steps"));
        let restore = ops.tasks.start(TaskSpec::new("updates","restore",&[]).cancellable());
        p.paused.push("owner-already-forgot".into());
        ops.rolling_run(&ctx,&models,&mut p,&restore,true).await.unwrap();
        assert_eq!(p.phase,"done"); assert!(p.paused.is_empty());
        assert!(check_load(&ops.live,true,&["cell2".into()]).is_ok());
    }

}
