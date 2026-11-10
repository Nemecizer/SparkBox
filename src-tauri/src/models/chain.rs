//! Explicitly reviewed, node-side download → verify → copy jobs. No startup dispatcher.
use super::*;
use std::net::Ipv4Addr;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChainMember {
    pub node: String,
    pub id: String,
    pub dest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadChain {
    pub id: String,
    pub source: String,
    pub repo: String,
    pub revision: String,
    pub expires_at: u64,
    pub members: Vec<ChainMember>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChainNode {
    pub node: String,
    pub dest: String,
    pub address: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChainReview {
    pub selection: hf::DownloadSelection,
    pub source: String,
    pub targets: Vec<String>,
    pub nodes: Vec<ChainNode>,
    pub checks: Vec<DiskCheck>,
    pub port: u16,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainCancel {
    pub node: String,
    pub id: String,
    pub error: Option<String>,
}

fn chosen(ctx: &Ctx, source: &str, targets: &[String]) -> Result<Vec<String>, String> {
    let mut nodes = vec![source.to_string()];
    nodes.extend_from_slice(targets);
    if targets.is_empty()
        || targets.len() > 16
        || nodes.iter().collect::<std::collections::HashSet<_>>().len() != nodes.len()
        || nodes
            .iter()
            .any(|n| !ctx.node_ids().contains(n) || (ctx.live && !ctx.online().contains(n)))
    {
        return Err("Choose a connected download node and 1–16 distinct connected copy targets from this inventory.".into());
    }
    Ok(nodes)
}
fn route(facts: &[(String, Vec<FabricIf>)]) -> Result<Vec<String>, String> {
    for source in &facts[0].1 {
        let Ok(ip) = source.ip.parse::<Ipv4Addr>() else {
            continue;
        };
        if !(1..=30).contains(&source.prefix) || ip.is_loopback() || ip.is_unspecified() {
            continue;
        }
        let mask = u32::MAX << (32 - source.prefix);
        let mut addresses = vec![source.ip.clone()];
        for (_, fabric) in &facts[1..] {
            if let Some(peer) = fabric.iter().find(|f| {
                f.prefix == source.prefix
                    && f.ip.parse::<Ipv4Addr>().is_ok_and(|v| {
                        !v.is_loopback()
                            && !v.is_unspecified()
                            && u32::from(v) & mask == u32::from(ip) & mask
                            && v != ip
                    })
            }) {
                addresses.push(peer.ip.clone());
            }
        }
        if addresses.len() == facts.len()
            && addresses
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == addresses.len()
        {
            return Ok(addresses);
        }
    }
    Err("The chosen nodes need distinct IPv4 fabric addresses on one common subnet. No LAN fallback or firewall change was made.".into())
}
impl DownloadChain {
    fn valid(&self) -> bool {
        self.id.starts_with("dc-")
            && self.id.len() <= 40
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && (2..=17).contains(&self.members.len())
            && self.members[0].node == self.source
            && self
                .members
                .iter()
                .enumerate()
                .all(|(i, m)| m.id == format!("{}-{i}", self.id))
            && self
                .members
                .iter()
                .map(|m| &m.node)
                .collect::<std::collections::HashSet<_>>()
                .len()
                == self.members.len()
    }
}
impl Models {
    pub async fn chain_review(
        &self,
        ctx: &Ctx,
        selection: &hf::DownloadSelection,
        source: &str,
        targets: &[String],
    ) -> Result<ChainReview, String> {
        chosen(ctx, source, targets)?;
        let model = self.reviewed_model(selection).await?;
        self.chain_review_model(ctx, selection, source, targets, &model)
            .await
    }
    async fn chain_review_model(
        &self,
        ctx: &Ctx,
        selection: &hf::DownloadSelection,
        source: &str,
        targets: &[String],
        model: &HfModel,
    ) -> Result<ChainReview, String> {
        let ids = chosen(ctx, source, targets)?;
        selection.check(model)?;
        if model.mlx
            || model.weights_format == "none"
            || selection
                .files
                .iter()
                .any(|f| f.path.split('/').any(|p| p.starts_with('.')))
        {
            return Err("This chain needs served weight files and no hidden paths. Review a supported model.".into());
        }
        let bytes = disk::file_bytes(&json!(selection.files))?;
        let dest = format!(
            "{}/{}",
            ctx.settings.model_root.trim_end_matches('/'),
            selection.repo
        );
        let mut checks = Vec::new();
        let mut facts = Vec::new();
        for (index, node) in ids.iter().enumerate() {
            if ctx.live {
                let raw = self
                    .sparkjob(
                        node,
                        "chain-preflight",
                        Some(
                            json!({"dest":dest,"bytes":bytes,"node":node})
                                .to_string()
                                .as_bytes(),
                        ),
                        Duration::from_secs(30),
                    )
                    .await?;
                if raw["rsync"] != true {
                    return Err(format!("{node} needs rsync installed before a chain can start. Nothing was installed."));
                }
                let check = self.destination_disk(ctx, node, &dest, bytes).await?;
                check.require()?;
                if raw["dest"] != check.dest {
                    return Err(
                        "The destination changed during review. Review the chain again.".into(),
                    );
                }
                facts.push((
                    node.clone(),
                    serde_json::from_value(raw["fabric"].clone())
                        .map_err(|_| "Invalid fabric facts.")?,
                ));
                checks.push(check);
            } else {
                if self
                    .demo
                    .lock()
                    .unwrap()
                    .library()
                    .iter()
                    .any(|e| e.repo == selection.repo && e.copies.contains_key(node))
                {
                    return Err(format!("{node} already has files for this model. A chain needs empty destinations."));
                }
                checks.push(disk::demo_check(
                    node,
                    &dest.replace("~/", "/home/demo/"),
                    bytes,
                ));
                facts.push((
                    node.clone(),
                    vec![FabricIf {
                        ip: format!("192.0.2.{}", index + 1),
                        prefix: 24,
                        ..Default::default()
                    }],
                ));
            }
        }
        let addresses = route(&facts)?;
        let nodes = ids
            .iter()
            .zip(addresses)
            .zip(&checks)
            .map(|((node, address), c)| ChainNode {
                node: node.clone(),
                dest: c.dest.clone(),
                address,
            })
            .collect();
        Ok(ChainReview {
            selection: selection.clone(),
            source: source.into(),
            targets: targets.to_vec(),
            nodes,
            checks,
            port: 8730 + fastrand::u16(0..200),
        })
    }
    pub async fn chain_start(
        self: &Arc<Self>,
        ctx: &Ctx,
        expected: &ChainReview,
        tool: &DownloadToolReview,
    ) -> Result<Vec<Job>, String> {
        chosen(ctx, &expected.source, &expected.targets)?;
        if tool.node != expected.source || !(8730..8930).contains(&expected.port) {
            return Err("Review the chain's source, port and tool again.".into());
        }
        self.check_download_tool(ctx, &expected.source, tool)
            .await?;
        let model = self.reviewed_model(&expected.selection).await?;
        self.chain_start_model(ctx, expected, tool, &model).await
    }
    async fn chain_start_model(
        self: &Arc<Self>,
        ctx: &Ctx,
        expected: &ChainReview,
        tool: &DownloadToolReview,
        model: &HfModel,
    ) -> Result<Vec<Job>, String> {
        let fresh = self
            .chain_review_model(
                ctx,
                &expected.selection,
                &expected.source,
                &expected.targets,
                model,
            )
            .await?;
        if fresh.nodes != expected.nodes {
            return Err("The reviewed destination or fabric route changed. Review the chain again; nothing was started.".into());
        }
        self.check_download_tool(ctx, &expected.source, tool)
            .await?;
        let id = format!("dc-{}", short_id());
        let chain = DownloadChain {
            id: id.clone(),
            source: expected.source.clone(),
            repo: model.id.clone(),
            revision: model.revision.clone(),
            expires_at: now() + 24 * 3600,
            members: expected
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| ChainMember {
                    node: n.node.clone(),
                    id: format!("{id}-{i}"),
                    dest: n.dest.clone(),
                })
                .collect(),
        };
        if !ctx.live {
            return Ok(self.demo.lock().unwrap().chain(model, &chain, &tool.tool));
        }
        let password = token();
        let mut jobs = Vec::new();
        // Targets wait on-node. The source is started LAST, so it never downloads for an unarmed set.
        let order = (1..chain.members.len()).chain(std::iter::once(0));
        for i in order {
            let member = &chain.members[i];
            let mut spec = download_spec(model, &member.dest, &ctx.settings.image, String::new());
            spec["chain"] = json!(chain);
            spec["password"] = json!(password);
            spec["protocol"] = json!(2);
            spec["exclude"] = json!([]);
            spec["label"] = json!(if i == 0 {
                format!("Download, verify and share {}", model.id)
            } else {
                format!("Wait, then copy {}", model.id)
            });
            if i == 0 {
                spec["token"] = json!(Self::hf_token().unwrap_or_default());
                spec["tool"] = json!(tool.tool);
                spec["installConsent"] = json!(tool.install_consent);
                spec["address"] = json!(expected.nodes[0].address);
                spec["port"] = json!(expected.port);
                spec["allow"] = json!(expected.nodes[1..]
                    .iter()
                    .map(|n| n.address.as_str())
                    .collect::<Vec<_>>()
                    .join(" "));
            } else {
                spec["src"] = json!(format!(
                    "rsync://sparkbox@{}:{}/model",
                    expected.nodes[0].address, expected.port
                ));
            }
            match self
                .start_job(
                    &member.node,
                    if i == 0 { "download" } else { "pull" },
                    &member.id,
                    &spec,
                )
                .await
            {
                Ok(job) => {
                    self.follow_job(&job, job.label.clone());
                    jobs.push(job);
                }
                Err(e) => {
                    let cleanup = self.cancel_chain_members(ctx, &chain).await;
                    let remaining = cleanup
                        .iter()
                        .filter(|r| r.error.is_some())
                        .map(|r| r.node.as_str())
                        .collect::<Vec<_>>();
                    return Err(format!("Chain setup stopped on {}: {e}. Cleanup was attempted through the normal safeguards. Reopen Jobs and review chain {}. Cleanup needing review: {}. No replacement chain was started.",member.node,chain.id,if remaining.is_empty() { "none reported".into() } else { remaining.join(", ") }));
                }
            }
        }
        Ok(jobs)
    }
    async fn cancel_chain_members(&self, ctx: &Ctx, chain: &DownloadChain) -> Vec<ChainCancel> {
        let mut results = Vec::new();
        for m in &chain.members {
            let outcome = async {
                if !ctx.node_ids().contains(&m.node) || !ctx.online().contains(&m.node) { return Err("Node is disconnected or no longer in inventory; cancel its owned chain job after reconnecting.".into()); }
                let list = self.sparkjob(&m.node,"list --look",None,Duration::from_secs(20)).await?;
                let rows = list.as_array().ok_or("Cannot read valid job records; cancellation was not confirmed.")?;
                let row = rows.iter().find(|j| j["id"] == m.id).ok_or("No job record was found; review this node after reconnecting before starting another chain.")?;
                if row["instance"] != crate::identity::instance_name() || row["chain"] != json!(chain) { return Err("Job ownership or chain plan changed; nothing on this node was canceled.".into()); }
                if row["orphan"] == true { return Err("This stopped job reports leftover processes. Its failure record was preserved; review a new download or Library copy to reclaim the folder through the normal safeguards.".into()); }
                if !matches!(row["state"].as_str(), Some("running" | "done" | "failed" | "cancelled")) { return Err("The job state is unknown; cancellation was not confirmed.".into()); }
                if row["state"] == "running" {
                    let stopped = self.sparkjob(&m.node,&format!("cancel {}",m.id),None,Duration::from_secs(30)).await?;
                    if let Some(message) = stopped["message"].as_str().filter(|m|m.starts_with("Canceled,")) { return Err(message.into()); }
                }
                Ok::<(),String>(())
            }.await;
            results.push(ChainCancel {
                node: m.node.clone(),
                id: m.id.clone(),
                error: outcome.err(),
            });
        }
        results
    }
    pub async fn chain_cancel(
        &self,
        ctx: &Ctx,
        node: &str,
        id: &str,
    ) -> Result<Vec<ChainCancel>, String> {
        if !ctx.node_ids().contains(&node.to_string()) {
            return Err("Choose an inventory node.".into());
        }
        if !ctx.live {
            return self.demo.lock().unwrap().cancel_chain(node, id);
        }
        let list = self
            .sparkjob(node, "list --look", None, Duration::from_secs(20))
            .await?;
        let row = list
            .as_array()
            .and_then(|r| r.iter().find(|j| j["id"] == id))
            .ok_or("Refresh Jobs; the chain job was not found.")?;
        let chain: DownloadChain = serde_json::from_value(row["chain"].clone())
            .map_err(|_| "This job has no valid chain plan.")?;
        if !chain.valid()
            || row["instance"] != crate::identity::instance_name()
            || !chain.members.iter().any(|m| m.node == node && m.id == id)
        {
            return Err("This chain belongs to another instance or its plan changed.".into());
        }
        Ok(self.cancel_chain_members(ctx, &chain).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{self, fake_node, temp, Script};
    use crate::live::exit::{Enforce, WritePolicy};
    use crate::vpn::Vpn;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn routes_and_saved_plans_are_exact() {
        let f = |ip: &str| {
            vec![FabricIf {
                ip: ip.into(),
                prefix: 24,
                ..Default::default()
            }]
        };
        assert_eq!(
            route(&[("a".into(), f("192.0.2.1")), ("b".into(), f("192.0.2.2"))]).unwrap(),
            vec!["192.0.2.1", "192.0.2.2"]
        );
        for peer in ["192.0.2.1", "198.51.100.1", "127.0.0.1"] {
            assert!(route(&[("a".into(), f("192.0.2.1")), ("b".into(), f(peer))]).is_err());
        }
        let plan = json!({"id":"dc-test","source":"a","repo":"fixture/model","revision":"a".repeat(40),"expiresAt":1,
            "members":[{"node":"a","id":"dc-test-0","dest":"/models/a"},{"node":"b","id":"dc-test-1","dest":"/models/a"}]});
        assert!(serde_json::from_value::<DownloadChain>(plan.clone())
            .unwrap()
            .valid());
        let mut bad = plan.clone();
        bad["members"][1]["id"] = json!("foreign");
        assert!(!serde_json::from_value::<DownloadChain>(bad)
            .unwrap()
            .valid());
        let mut bad = plan;
        bad["password"] = json!("never stored");
        assert!(serde_json::from_value::<DownloadChain>(bad).is_err());
    }

    #[tokio::test]
    async fn review_source_last_rollback_and_guarded_group_cancel() {
        let mode = Arc::new(AtomicUsize::new(0));
        let order: Arc<Mutex<Vec<String>>> = Arc::default();
        let rows: Arc<Mutex<Vec<Vec<Value>>>> = Arc::new(Mutex::new(vec![vec![], vec![]]));
        type Specs = Arc<Mutex<Vec<Value>>>;
        let histories: Arc<Mutex<Vec<Specs>>> = Arc::default();
        let mut peers = Vec::new();
        let mut ports = Vec::new();
        for i in 0..2 {
            let (mode, order, rows, history_read) =
                (mode.clone(), order.clone(), rows.clone(), histories.clone());
            let script: Script = Arc::new(move |cmd, _| {
                if cmd.contains("sparkjob.py chain-preflight") {
                    return Some((json!({"dest":"/models/fixture","rsync":mode.load(Ordering::SeqCst)!=5,
                    "fabric":[{"ip":format!("192.0.2.{}",i+1+if mode.load(Ordering::SeqCst)==1 {10}else{0}),"prefix":24}]}).to_string(),0));
                }
                if cmd.contains("sparkjob.py disk-preflight") {
                    return Some(disk::fixture_read(100));
                }
                if cmd.contains("sparkjob.py download-tools") {
                    return Some((json!(download_tools::demo_tools("ignored")).to_string(), 0));
                }
                if cmd.contains("sparkjob.py list") {
                    if mode.load(Ordering::SeqCst) == 6 {
                        return Some(("{}".into(), 0));
                    }
                    if mode.load(Ordering::SeqCst) == 2 && rows.lock().unwrap()[i].is_empty() {
                        if let Some(spec) = history_read.lock().unwrap()[i].lock().unwrap().last() {
                            rows.lock().unwrap()[i].push(json!({"id":spec["chain"]["members"][i]["id"],"state":"running","instance":spec["instance"],"chain":spec["chain"]}));
                        }
                    }
                    return Some((json!(rows.lock().unwrap()[i]).to_string(), 0));
                }
                if cmd.contains("sparkjob.py start") {
                    let row = json!({"id":cmd.split_whitespace().last().unwrap(),"kind":if i==0 {"download"}else{"pull"},"state":"running"});
                    order.lock().unwrap().push(format!("cell{}", i + 1));
                    if mode.load(Ordering::SeqCst) == 2 && i == 0 {
                        return Some(("fixture lost start acknowledgment".into(), 1));
                    }
                    return Some((row.to_string(), 0));
                }
                if cmd.contains("sparkjob.py cancel") {
                    for row in &mut rows.lock().unwrap()[i] {
                        if row["id"] == cmd.split_whitespace().last().unwrap() {
                            row["state"] = json!("cancelled");
                        }
                    }
                    return Some((json!({"message":"Canceled"}).to_string(), 0));
                }
                None
            });
            let (node, port) = fake_node(&temp(&format!("chain-node-{i}")), script).await;
            histories.lock().unwrap().push(node.specs.clone());
            peers.push(node);
            ports.push(port);
        }
        let base = temp("chain-app");
        let (live, inventory) =
            fake_node::connect(&base, &[("cell1", ports[0]), ("cell2", ports[1])]).await;
        let llm = Llm::new(
            base.join("providers.json"),
            Vpn::new(base.join("vpn.json"), base.join("no-helper"), |_| {}),
        );
        let models = Models::new(base.clone(), live.clone(), llm, None);
        let mut ctx = Ctx {
            live: true,
            settings: Default::default(),
            snapshot: Some(live.snapshot(&inventory)),
            inventory,
        };
        let model = hf::parse_model(
            &json!({"id":"fixture/model","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":100}]}),
            json!({}),
            None,
            "",
        );
        let selection = hf::DownloadSelection {
            repo: model.id.clone(),
            revision: model.revision.clone(),
            files: model.files.clone(),
        };
        let expected = models
            .chain_review_model(&ctx, &selection, "cell1", &["cell2".into()], &model)
            .await
            .unwrap();
        assert!(order.lock().unwrap().is_empty());
        let tool = DownloadToolReview {
            node: "cell1".into(),
            tool: download_tools::demo_tools("cell1").options[0].clone(),
            install_consent: false,
        };
        mode.store(1, Ordering::SeqCst);
        assert!(models
            .chain_start_model(&ctx, &expected, &tool, &model)
            .await
            .unwrap_err()
            .contains("route changed"));
        mode.store(5, Ordering::SeqCst);
        assert!(models
            .chain_start_model(&ctx, &expected, &tool, &model)
            .await
            .unwrap_err()
            .contains("rsync"));
        mode.store(0, Ordering::SeqCst);
        let mut changed = model.clone();
        changed.files[0].size = Some(101);
        assert!(models
            .chain_start_model(&ctx, &expected, &tool, &changed)
            .await
            .is_err());
        assert!(order.lock().unwrap().is_empty());
        models
            .chain_start_model(&ctx, &expected, &tool, &model)
            .await
            .unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["cell2", "cell1"]);
        let specs: Vec<Value> = peers
            .iter()
            .map(|p| {
                serde_json::from_slice(
                    &p.stdin
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(cmd, _)| cmd.contains("sparkjob.py start"))
                        .unwrap()
                        .1,
                )
                .unwrap()
            })
            .collect();
        assert_eq!(specs[0]["chain"], specs[1]["chain"]);
        assert_eq!(specs[0]["files"], json!(selection.files));
        assert_eq!(specs[0]["allow"], "192.0.2.2");
        assert_eq!(specs[0]["password"], specs[1]["password"]);
        let chain: DownloadChain = serde_json::from_value(specs[0]["chain"].clone()).unwrap();
        for i in 0..2 {
            rows.lock().unwrap()[i] = vec![
                json!({"id":chain.members[i].id,"state":"running","instance":crate::identity::instance_name(),"chain":chain}),
            ];
        }
        rows.lock().unwrap()[1][0]["instance"] = json!("foreign");
        let result = models
            .chain_cancel(&ctx, "cell1", &chain.members[0].id)
            .await
            .unwrap();
        assert!(result[0].error.is_none());
        assert!(result[1].error.as_ref().unwrap().contains("ownership"));
        assert!(peers[1].ran("sparkjob.py cancel").is_empty());
        rows.lock().unwrap()[1][0]["instance"] = json!(crate::identity::instance_name());
        rows.lock().unwrap()[1][0]["chain"]["revision"] = json!("b".repeat(40));
        assert!(models
            .chain_cancel(&ctx, "cell1", &chain.members[0].id)
            .await
            .unwrap()[1]
            .error
            .is_some());
        rows.lock().unwrap()[1][0]["chain"] = json!(chain);
        assert!(models
            .chain_cancel(&ctx, "cell1", &chain.members[0].id)
            .await
            .unwrap()
            .iter()
            .all(|r| r.error.is_none()));
        rows.lock().unwrap()[0][0]["state"] = json!("failed");
        rows.lock().unwrap()[0][0]["message"] = json!("Original verification failure");
        let before = peers[0].ran("sparkjob.py cancel").len();
        assert!(models
            .cancel_chain_members(&ctx, &chain)
            .await
            .iter()
            .all(|r| r.error.is_none()));
        assert_eq!(peers[0].ran("sparkjob.py cancel").len(), before);
        assert_eq!(
            rows.lock().unwrap()[0][0]["message"],
            "Original verification failure"
        );
        rows.lock().unwrap()[0][0]["orphan"] = json!(true);
        assert!(models.cancel_chain_members(&ctx, &chain).await[0]
            .error
            .as_ref()
            .unwrap()
            .contains("leftover processes"));
        assert_eq!(peers[0].ran("sparkjob.py cancel").len(), before);
        mode.store(6, Ordering::SeqCst);
        assert!(models
            .cancel_chain_members(&ctx, &chain)
            .await
            .iter()
            .all(|r| r.error.is_some()));
        for node in &mut *rows.lock().unwrap() {
            node.clear();
        }
        mode.store(2, Ordering::SeqCst);
        assert!(models
            .chain_start_model(&ctx, &expected, &tool, &model)
            .await
            .unwrap_err()
            .contains("Chain setup stopped"));
        assert_eq!(
            order.lock().unwrap().len(),
            4,
            "one source dispatch, no automatic retries"
        );
        assert!(
            rows.lock()
                .unwrap()
                .iter()
                .flatten()
                .all(|r| r["state"] == "cancelled"),
            "uncertain source and armed target both canceled"
        );
        mode.store(0, Ordering::SeqCst);
        live.enforce_writes(Enforce {
            instance: crate::identity::instance_name().into(),
            fence_dir: base.join("Fence"),
            config_dir: Some(base.join("config")),
            noauth: false,
            tag: "chain-test".into(),
            mac: Some("fixture Mac".into()),
        });
        assert!(models
            .chain_start_model(&ctx, &expected, &tool, &model)
            .await
            .is_err());
        struct ReadOnly;
        impl WritePolicy for ReadOnly {
            fn look_only(&self) -> bool {
                true
            }
            fn admin_unlocked(&self) -> bool {
                false
            }
        }
        live.set_write_policy(Arc::new(ReadOnly));
        models
            .chain_review_model(&ctx, &selection, "cell1", &["cell2".into()], &model)
            .await
            .unwrap();
        assert!(models
            .chain_start_model(&ctx, &expected, &tool, &model)
            .await
            .is_err());
        assert_eq!(
            order.lock().unwrap().len(),
            4,
            "guarded starts never reach SSH"
        );
        ctx.snapshot.as_mut().unwrap().nodes[1].status = LinkStatus::Offline;
        for targets in [
            vec!["cell2".into()],
            vec!["cell1".into()],
            vec!["foreign".into()],
            vec![],
        ] {
            assert!(models
                .chain_review(&ctx, &selection, "cell1", &targets)
                .await
                .is_err());
        }
        live.stop();
    }
}
