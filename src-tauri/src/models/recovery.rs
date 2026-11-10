//! Read-only reconciliation of a load left behind when SparkBox quit.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Recovery { Checking, Waiting, Cleanup, OwnerAction, Maintenance }

pub(super) enum Reconciled { Present, Wait, Owner }

impl Models {
    /// Stop an owned load once for rolling maintenance, retaining its key and launch settings.
    /// Persist the hold before a stop is admitted so a crash cannot trigger Keep running.
    pub async fn pause_for_maintenance(self: &Arc<Self>, ctx: &Ctx, id: &str) -> Result<(), String> {
        let _slot = RestartSlot::take(&self.restarting, &format!("load:{id}"), id)?;
        let lock = self.start_lock(id);
        let _lock = lock.lock().await;
        let d = self.deployments(ctx).into_iter().find(|d| d.spec.id == id && d.state != DeploymentState::Stopped).ok_or("The load was unloaded; review the maintenance plan.")?;
        if d.recovery.is_some() && d.recovery != Some(Recovery::Maintenance) { return Err("This load needs recovery review before maintenance.".into()); }
        if !ctx.live { self.demo.lock().unwrap().pause_for_maintenance(id); return Ok(()); }
        let counts = self.load_containers(&d.spec.nodes, id).await;
        Self::all_mine(&d.spec.served_name, &counts)?;
        self.update(id, |d| { d.generation += 1; d.state = DeploymentState::Failed; d.recoverable = false; d.recovery = Some(Recovery::Maintenance); d.message = "Paused for rolling maintenance. Resume or restore it from Software Updates.".into(); });
        for (i, node) in d.spec.nodes.iter().enumerate() {
            let role = if d.spec.nodes.len() == 1 { "single" } else if i == 0 { "head" } else { "worker" };
            let out = as_class(FenceClass::ModelUnload, self.live.run(node, &plan::stop_role_command(id, role), None, Duration::from_secs(120))).await?;
            if out.code != Some(0) { return Err(format!("{node}: couldn't stop the owned {role}; maintenance is paused.")); }
        }
        Ok(())
    }

    pub async fn restore_after_maintenance(self: &Arc<Self>, ctx: &Ctx, id: &str) -> Result<(), String> {
        let d = self.deployments(ctx).into_iter().find(|d| d.spec.id == id).ok_or("The saved load no longer exists.")?;
        if d.state == DeploymentState::Stopped { return Err("The owner unloaded this model; it will not be restarted by maintenance.".into()); }
        if d.state == DeploymentState::Ready && d.recovery.is_none() { return Ok(()); }
        if d.recovery != Some(Recovery::Maintenance) { return Err("The load changed outside maintenance; review it before resuming.".into()); }
        let result = self.restart_ordered(ctx, id, "Restore", false).await;
        if let Err(e) = &result {
            self.update(id, |d| { if d.state != DeploymentState::Stopped { d.state = DeploymentState::Failed; d.recoverable = false; d.recovery = Some(Recovery::Maintenance); d.message = format!("Maintenance restore needs review: {}", audit::redact(e)); } });
        }
        result
    }
    /// Explicit owner action. Existing ranks must still be running with the saved immutable
    /// image and labels; only absent ranks get files or docker run. A failed resume leaves the
    /// evidence in place for another review instead of rolling back ranks it did not start.
    pub async fn resume_partial(self: &Arc<Self>, ctx: &Ctx, id: &str, drop_caches_once: bool) -> Result<(), String> {
        let slot = RestartSlot::take(&self.restarting, &format!("load:{id}"), id)?;
        let lock = self.start_lock(id);
        let starting = lock.lock().await;
        let mut d = self.deployments(ctx).into_iter().find(|d| d.spec.id == id).ok_or("Unknown saved load.")?;
        if !d.first_start || d.readiness.as_ref().is_some_and(|r| r.ok) || d.recovery != Some(Recovery::Cleanup) || !plan::is_native(&d.spec) {
            return Err("Resume is only for an interrupted first startup. Use Restart in order for a load that already served.".into());
        }
        plan::validate(&d.spec)?;
        if d.spec.nodes.iter().any(|n| !ctx.node_ids().contains(n)) { return Err("The saved load names a node outside this inventory.".into()); }
        if !ctx.live {
            self.demo.lock().unwrap().restart(id).ok_or("Unknown Demo load.")?;
            return Ok(());
        }
        if !plan::is_image_id(&d.spec.image_id) { return Err("The saved image identity is missing; clean up and load again.".into()); }
        let key = if d.spec.require_key { secrets::get(&plan::key_account(id))? } else { None };
        if d.spec.require_key && key.is_none() { return Err("The saved API key is missing; clean up and load again.".into()); }
        let before = self.load_containers(&d.spec.nodes, id).await;
        Self::all_mine(&d.spec.served_name, &before)?;
        let mut missing = Vec::new();
        for (rank, (node, counts)) in before.iter().enumerate() {
            let mine = &counts.as_ref().ok_or("A node did not report ownership.")?.mine;
            if mine.is_empty() { missing.push(rank); continue; }
            if mine.len() != 1 { return Err(format!("{node} has more than one owned rank; review and clean up first.")); }
            let name = plan::rank_container(id, rank, d.spec.nodes.len());
            let cmd = format!("docker inspect -f '{{{{.Id}}}} {{{{.State.Status}}}} {{{{.Image}}}} {{{{index .Config.Labels \"sparkbox.instance\"}}}} {{{{index .Config.Labels \"sparkbox.deployment\"}}}}' {name}");
            let out = self.live.run(node, &cmd, None, Duration::from_secs(20)).await?;
            validate_resume_rank(&out.stdout, out.code, &mine[0], &d.spec.image_id, id).map_err(|e| format!("{node}: {e}"))?;
        }
        // Check all nodes while counting the existing ranks as this deployment's own. Cache
        // drops are restricted to missing ranks; an existing rank is never disturbed.
        d.spec.drop_caches |= drop_caches_once;
        let mut set = self.preflight_set(&d.spec, &ctx.settings, Some(&d)).await;
        set.nodes.iter_mut().enumerate().filter(|(i, _)| !missing.contains(i)).for_each(|(_, p)| p.drop_needed = false);
        set = plan::PreflightSet::judge(&d.spec, set.nodes, set.checked_at_ms);
        set_blocked(&set, &d.spec, Starting::Deploy)?;
        if set.image_id.as_deref() != Some(&d.spec.image_id) { return Err("The image tag no longer names the saved image on every node. Clean up and load again.".into()); }
        for &rank in &missing {
            if set.nodes[rank].drop_needed { self.drop_page_cache(&d.spec.nodes[rank], id).await?; }
        }
        if set.nodes.iter().any(|p| p.drop_needed) {
            let mut fresh = self.preflight_set(&d.spec, &ctx.settings, Some(&d)).await;
            fresh.nodes.iter_mut().enumerate().filter(|(i, _)| !missing.contains(i)).for_each(|(_, p)| p.drop_needed = false);
            let fresh = plan::PreflightSet::judge(&d.spec, fresh.nodes, fresh.checked_at_ms);
            set_blocked(&fresh, &d.spec, Starting::Deploy)?;
            if fresh.image_id.as_deref() != Some(&d.spec.image_id) { return Err("The image changed during the memory check. Nothing else was started.".into()); }
            if missing.iter().any(|&r| fresh.nodes[r].drop_needed) { return Err("A missing rank still lacks free memory after the cache drop.".into()); }
            set = fresh;
        }
        let nets: Vec<NodeNet> = set.nodes.iter().filter_map(|p| Self::net_of(&p.node, &p.fabric)).collect();
        if d.spec.nodes.len() > 1 { self.jumbo_path(&set).await?; }
        let generation = d.generation + 1;
        self.update(id, |d| { d.generation = generation; d.state = DeploymentState::Starting; d.stage = Some(Stage::Starting); d.recovery = None; d.message = "Resuming missing ranks…".into(); });
        let class = if d.spec.nodes.len() == 1 { FenceClass::ModelLoadOne } else { FenceClass::ModelLoad };
        let result = async {
            for rank in missing.iter().copied().filter(|r| *r != 0).chain(missing.contains(&0).then_some(0)) {
                if !self.current(id, generation) { return Err("Resume was canceled.".into()); }
                self.write_rank_files(&d.spec, rank, &nets, key.as_deref(), class).await?;
                self.start_rank(&d.spec, rank, class).await.map_err(|(e, _)| e)?;
            }
            Ok::<(), String>(())
        }.await;
        drop(starting);
        if let Err(e) = result {
            self.recovery_owner(&d.spec, generation, Recovery::Cleanup, &format!("Resume stopped: {}. Review the remaining ranks; they were preserved.", audit::redact(&e)));
            return Err(e);
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let _slot = slot;
            let head = plan::rank_container(&d.spec.id, 0, d.spec.nodes.len());
            if let Err(e) = me.clone().follow(&d.spec, &head, generation).await {
                me.recovery_owner(&d.spec, generation, Recovery::Cleanup, &audit::redact(&e));
            }
        });
        Ok(())
    }
    pub(super) fn recovery_wait(&self, id: &str, generation: u64, message: &str) {
        self.update(id, |d| {
            if d.generation == generation && d.state.active() {
                d.recovery = Some(Recovery::Waiting);
                d.message = message.into();
            }
        });
    }

    pub(super) async fn reconcile_load(&self, spec: &DeploySpec, generation: u64) -> Reconciled {
        if spec.nodes.is_empty() { return Reconciled::Owner; }
        let before = self.load_containers(&spec.nodes, &spec.id).await;
        if !self.current(&spec.id, generation) { return Reconciled::Owner; }
        let unavailable: Vec<&str> = before.iter().filter(|(_, c)| c.is_none()).map(|(n, _)| n.as_str()).collect();
        if !unavailable.is_empty() {
            self.recovery_wait(&spec.id, generation, &format!("Waiting for fresh reports from {}. Observation resumes when they return; nothing is relaunched or cleaned up while they are unreachable.", unavailable.join(", ")));
            return Reconciled::Wait;
        }
        let conflict = before.iter().any(|(_, c)| c.as_ref().is_some_and(|c| !c.others.is_empty() || !c.unlabelled.is_empty()));
        if conflict {
            self.recovery_owner(spec, generation, Recovery::OwnerAction, "Ownership changed or cannot be confirmed. Review the containers and their owner before restarting or removing this load.");
            return Reconciled::Owner;
        }
        let missing: Vec<&str> = before.iter().filter(|(_, c)| c.as_ref().is_some_and(|c| c.mine.is_empty())).map(|(n, _)| n.as_str()).collect();
        if !missing.is_empty() {
            self.recovery_owner(spec, generation, Recovery::Cleanup, &format!("The interrupted load has no owned container on {}. Review its remaining containers. Resume can start missing ranks of a first startup; Restart in order replaces all ranks, and Unload cleans up. SparkBox has not launched replacements.", missing.join(", ")));
            return Reconciled::Owner;
        }
        // A head answering /health cannot prove its distributed workers are still running.
        let names = containers_of(spec);
        for (node, name) in spec.nodes.iter().zip(names).skip(1) {
            let cmd = rank_state_command(&name, None);
            let result = audit::as_probe(self.live.run(node, &cmd, None, Duration::from_secs(20))).await;
            if !self.current(&spec.id, generation) { return Reconciled::Owner; }
            match result {
                Ok(o) if o.code == Some(0) && o.stdout.starts_with("running") => {}
                Ok(o) if o.code == Some(0) && !o.stdout.trim().is_empty() => {
                    self.recovery_owner(spec, generation, Recovery::Cleanup, &format!("The interrupted load's worker on {node} is no longer running. Review its logs, then choose Unload before loading again. SparkBox has not launched replacements."));
                    return Reconciled::Owner;
                }
                _ => {
                    self.recovery_wait(&spec.id, generation, &format!("Waiting for the worker on {node} to report its state. Nothing is relaunched or cleaned up while it is unreachable."));
                    return Reconciled::Wait;
                }
            }
        }
        Reconciled::Present
    }

    pub(super) fn recovery_owner(&self, spec: &DeploySpec, generation: u64, recovery: Recovery, message: &str) {
        self.update(&spec.id, |d| {
            if d.generation == generation && d.state.active() {
                d.state = DeploymentState::Failed;
                d.recoverable = false;
                d.recovery = Some(recovery);
                d.message = message.into();
            }
        });
    }
}

fn validate_resume_rank(text: &str, code: Option<u32>, owned: &str, image: &str, deployment: &str) -> Result<(), String> {
    let words: Vec<_> = text.split_whitespace().collect();
    if code != Some(0) || words.len() != 5 || owned.len() < 12 || !owned.bytes().all(|c| c.is_ascii_hexdigit()) || !words[0].starts_with(owned) || words[1] != "running" || words[2] != image
        || words[3] != crate::identity::instance_name() || words[4] != deployment {
        return Err("The existing rank is stopped or its identity differs from the saved load. Review it, then use Restart in order or clean up.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{fake_node, temp};
    use crate::vpn::Vpn;
    use std::sync::atomic::{AtomicU8, Ordering};

    #[test]
    fn resume_requires_the_running_owned_rank_and_exact_saved_image() {
        let id = "abcdef0123456789"; let image = format!("sha256:{}", "a".repeat(64));
        let good = format!("{id} running {image} {} saved", crate::identity::instance_name());
        assert!(validate_resume_rank(&good, Some(0), &id[..12], &image, "saved").is_ok());
        for bad in [good.replace("running", "exited"), good.replace(&image, "sha256:other"), good.replace(crate::identity::instance_name(), "another-owner"), good.replace("saved", "another-load"), good.replace(id, "1111111111111111"), format!("{good} extra")] {
            assert!(validate_resume_rank(&bad, Some(0), &id[..12], &image, "saved").is_err());
        }
        assert!(validate_resume_rank(&good, Some(1), &id[..12], &image, "saved").is_err());
        assert!(validate_resume_rank(&good, Some(0), "abc", &image, "saved").is_err());
    }

    #[tokio::test]
    async fn recovery_checks_every_rank_and_never_launches_or_cleans_up_on_missing_evidence() {
        let mode = Arc::new(AtomicU8::new(0));
        let mut peers = Vec::new();
        for rank in 0..2 {
            let mode = mode.clone();
            let peer = fake_node(&temp("recovery-peer"), Arc::new(move |cmd, _| {
                if cmd.starts_with("docker ps -a --filter label=sparkbox.deployment=") {
                    return Some(match (rank, mode.load(Ordering::SeqCst)) {
                        (1, 1) => (String::new(), 0), // fresh report: worker absent
                        (1, 2) => ("abc other-owner\n".into(), 0),
                        (1, 3) => ("Docker unavailable".into(), 1),
                        _ => (format!("abc {}\n", crate::identity::instance_name()), 0),
                    });
                }
                if cmd.contains("docker inspect") {
                    return Some((if mode.load(Ordering::SeqCst) == 4 { "exited 1" } else { "running 0" }.into(), 0));
                }
                None
            })).await;
            peers.push(peer);
        }
        let dir = temp("recovery-saved");
        let (live, _) = crate::fake_node::connect(&dir, &[("cell1", peers[0].1), ("cell2", peers[1].1)]).await;
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {}));
        let spec = DeploySpec { id: "recover".into(), nodes: vec!["cell1".into(), "cell2".into()], served_name: "saved".into(), ..Default::default() };
        let saved = Deployment { spec: spec.clone(), state: DeploymentState::Ready, recoverable: true, started_at: now() - 7 * 86400, ..Default::default() };
        save(&dir.join("deployments.json"), &vec![saved.clone()]);
        let models = Models::new(dir.clone(), live.clone(), llm, None);
        assert_eq!(models.deployments.lock().unwrap()[0].state, DeploymentState::Ready, "the saved record is historical until resume checks it");
        assert!(claim(&models.deployments.lock().unwrap(), &spec).is_err(), "no duplicate launch from stale saved Ready");
        mode.store(3, Ordering::SeqCst);
        assert!(matches!(models.reconcile_load(&spec, 0).await, Reconciled::Wait));
        assert_eq!(models.deployments.lock().unwrap()[0].state, DeploymentState::Ready);
        mode.store(0, Ordering::SeqCst);
        assert!(matches!(models.reconcile_load(&spec, 0).await, Reconciled::Present), "reconnect after a week can resume observation");
        for (value, reason) in [(1, Recovery::Cleanup), (2, Recovery::OwnerAction), (4, Recovery::Cleanup)] {
            *models.deployments.lock().unwrap() = vec![saved.clone()];
            mode.store(value, Ordering::SeqCst);
            assert!(matches!(models.reconcile_load(&spec, 0).await, Reconciled::Owner));
            let d = models.deployments.lock().unwrap()[0].clone();
            assert_eq!((d.state, d.recovery, d.recoverable), (DeploymentState::Failed, Some(reason), false));
            assert!(claim(&[d], &spec).is_err(), "review is required before reusing the nodes");
        }
        let mut newer = saved.clone(); newer.generation = 7;
        *models.deployments.lock().unwrap() = vec![newer];
        let before = peers[0].0.ran("docker ps -a").len();
        models.clone().watch_only(&spec, &ModelSettings::default(), 0).await.unwrap();
        assert_eq!(peers[0].0.ran("docker ps -a").len(), before, "old observer cannot adopt a new launch generation");
        assert_eq!(models.deployments.lock().unwrap()[0].state, DeploymentState::Ready);
        for (node, _) in &peers {
            assert!(node.ran("docker run").is_empty());
            assert!(node.ran("docker stop").is_empty());
            assert!(node.ran("docker rm").is_empty());
        }
        live.stop();
    }
}
