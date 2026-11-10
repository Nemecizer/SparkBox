//! Read-only tool inventory and a per-attempt reviewed choice. Installing is never a fallback.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadTool {
    pub kind: String,
    pub path: String,
    pub fingerprint: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadToolReview {
    pub node: String,
    pub tool: DownloadTool,
    pub install_consent: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DownloadTools {
    pub node: String,
    pub options: Vec<DownloadTool>,
    pub notes: Vec<String>,
}

pub(super) fn demo_tools(node: &str) -> DownloadTools {
    DownloadTools { node: node.into(), options: vec![
        DownloadTool { kind: "host".into(), path: "/home/demo/.local/bin/hf".into(), fingerprint: "a".repeat(64), label: "Existing hf on this node".into() },
        DownloadTool { kind: "install".into(), path: "/home/demo/.sparkbox/venv".into(), fingerprint: "uv-0.13.7-hf-2.4.0-xet-1.8.0".into(), label: "Install SparkBox hf 2.4.0 + hf-xet 1.8.0".into() },
        DownloadTool { kind: "image".into(), path: format!("sha256:{}", "b".repeat(64)), fingerprint: format!("sha256:{}", "b".repeat(64)), label: "vllm:demo".into() },
    ], notes: vec!["Demo: these tool choices and any installation are simulated.".into()] }
}

impl Models {
    pub async fn download_tools(&self, ctx: &Ctx, node: &str) -> Result<DownloadTools, String> {
        if !ctx.node_ids().iter().any(|n| n == node) {
            return Err("Choose an inventory node.".into());
        }
        if !ctx.live { return Ok(demo_tools(node)); }
        if !ctx.online().iter().any(|n| n == node) {
            return Err(format!("{node} is not connected. Reconnect it before choosing a download tool."));
        }
        let result = self.sparkjob(node, "download-tools", None, Duration::from_secs(20)).await?;
        let mut result: DownloadTools = serde_json::from_value(result).map_err(|e| e.to_string())?;
        result.node = node.into();
        Ok(result)
    }

    pub(super) async fn check_download_tool(&self, ctx: &Ctx, node: &str, review: &DownloadToolReview) -> Result<(), String> {
        if review.node != node || (review.tool.kind == "install" && !review.install_consent) {
            return Err("Review the download tool and confirm any installation for this node and attempt.".into());
        }
        let current = self.download_tools(ctx, node).await?;
        if !current.options.contains(&review.tool) {
            return Err("The download tool changed. Review the tool choice again; no fallback was started.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{self, fake_node, temp, Script};
    use crate::live::exit::{Enforce, WritePolicy};
    use crate::vpn::Vpn;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn discovery_and_review_refuse_stale_unconsented_wrong_node_and_fenced_writes() {
        struct LookOnly;
        impl WritePolicy for LookOnly {
            fn look_only(&self) -> bool { true }
            fn admin_unlocked(&self) -> bool { true }
        }
        let changed = Arc::new(AtomicBool::new(false));
        let changed_in_node = changed.clone();
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py disk-preflight") { return Some(disk::fixture_read(100)); }
            if cmd.contains("sparkjob.py download-tools") {
                let mut tools = demo_tools("untrusted node label");
                if changed_in_node.load(Ordering::SeqCst) { tools.options[0].fingerprint = "c".repeat(64); }
                return Some((json!(tools).to_string(), 0));
            }
            if cmd.contains("sparkjob.py list") { return Some(("[]".into(), 0)); }
            if cmd.contains("sparkjob.py start download") {
                return Some((json!({"id":cmd.split_whitespace().last().unwrap(),"kind":"download","state":"running"}).to_string(), 0));
            }
            None
        });
        let (node, port) = fake_node(&temp("tools-home"), script).await;
        let base = temp("tools-app");
        let (live, inventory) = fake_node::connect(&base, &[("cell1", port)]).await;
        let llm = Llm::new(base.join("providers.json"), Vpn::new(base.join("vpn.json"), base.join("no-helper"), |_| {}));
        let models = Models::new(base.clone(), live.clone(), llm, None);
        let ctx = Ctx { live: true, settings: Default::default(), snapshot: Some(live.snapshot(&inventory)), inventory };
        let tools = models.download_tools(&ctx, "cell1").await.unwrap();
        assert_eq!(tools.node, "cell1", "node labels from the helper are never authority");
        let mut review = DownloadToolReview { node: "cell1".into(), tool: tools.options[0].clone(), install_consent: false };
        models.check_download_tool(&ctx, "cell1", &review).await.unwrap();
        assert!(node.ran("sparkjob.py start").is_empty());
        changed.store(true, Ordering::SeqCst);
        assert!(models.check_download_tool(&ctx, "cell1", &review).await.unwrap_err().contains("changed"));
        changed.store(false, Ordering::SeqCst);
        assert!(models.check_download_tool(&ctx, "cell2", &review).await.is_err());
        review.tool = tools.options[1].clone();
        assert!(models.check_download_tool(&ctx, "cell1", &review).await.is_err());
        assert!(node.ran("sparkjob.py start").is_empty());
        review.install_consent = true;
        let model = hf::parse_model(&json!({"id":"org/model","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":100}]}),json!({}),None,"");
        let job = models.download_model(&ctx, &model, &review).await.unwrap();
        assert_eq!(job.node, "cell1");
        let inputs = node.stdin.lock().unwrap().clone();
        let spec: Value = serde_json::from_slice(&inputs.iter().find(|(cmd, _)| cmd.contains("sparkjob.py start download")).unwrap().1).unwrap();
        assert_eq!(spec["tool"], json!(review.tool));
        assert_eq!(spec["installConsent"], true);
        live.enforce_writes(Enforce { instance: crate::identity::instance_name().into(), fence_dir: base.join("Fence"), config_dir: Some(base.join("config")), noauth: false, tag: "tool-review-test".into(), mac: Some("test Mac".into()) });
        assert!(models.download_model(&ctx, &model, &review).await.unwrap_err().contains("fence"));
        live.set_write_policy(Arc::new(LookOnly));
        models.download_tools(&ctx, "cell1").await.unwrap();
        assert!(models.download_model(&ctx, &model, &review).await.is_err());
        assert_eq!(node.ran("sparkjob.py start").len(), 1);
        live.stop();
    }

    #[test]
    fn review_rejects_unexpected_fields_and_missing_consent() {
        let tools = demo_tools("cell1");
        let review = json!({"node":"cell1","tool":tools.options[0],"installConsent":false});
        assert!(serde_json::from_value::<DownloadToolReview>(review.clone()).is_ok());
        let mut extra = review.clone(); extra["tool"]["command"] = json!("arbitrary");
        assert!(serde_json::from_value::<DownloadToolReview>(extra).is_err());
        let mut missing = review; missing.as_object_mut().unwrap().remove("installConsent");
        assert!(serde_json::from_value::<DownloadToolReview>(missing).is_err());
    }

    #[tokio::test]
    async fn download_target_is_the_reviewed_id_not_inventory_order_or_display_name() {
        let busy = Arc::new(AtomicBool::new(false));
        let busy_node = busy.clone();
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py disk-preflight") { return Some(disk::fixture_read(100)); }
            if cmd.contains("sparkjob.py download-tools") { return Some((json!(demo_tools("ignored")).to_string(), 0)); }
            if cmd.contains("sparkjob.py list") {
                let rows = if busy_node.load(Ordering::SeqCst) { json!([{"kind":"download","state":"running","dest":"~/models/org/model"}]) } else { json!([]) };
                return Some((rows.to_string(), 0));
            }
            if cmd.contains("sparkjob.py start download") { return Some((json!({"id":cmd.split_whitespace().last().unwrap(),"kind":"download","state":"running"}).to_string(), 0)); }
            None
        });
        let (first, p1) = fake_node(&temp("target-first"), script.clone()).await;
        let (chosen, p2) = fake_node(&temp("target-chosen"), script).await;
        let base = temp("target-app");
        let (live, mut inventory) = fake_node::connect(&base, &[("cell1", p1), ("stable-four", p2)]).await;
        inventory.nodes[1].name = "cell4".into();
        let llm = Llm::new(base.join("providers.json"), Vpn::new(base.join("vpn.json"), base.join("no-helper"), |_| {}));
        let models = Models::new(base, live.clone(), llm, None);
        let mut ctx = Ctx { live: true, settings: Default::default(), snapshot: Some(live.snapshot(&inventory)), inventory };
        // The first node need not be available. Its folder/job state must never be consulted.
        ctx.snapshot.as_mut().unwrap().nodes.iter_mut().find(|n| n.id == "cell1").unwrap().status = LinkStatus::Offline;
        let mut review = DownloadToolReview { node: "stable-four".into(), tool: demo_tools("stable-four").options[0].clone(), install_consent: false };
        let model = hf::parse_model(&json!({"id":"org/model","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":100}]}),json!({}),None,"");
        let job = models.download_model(&ctx, &model, &review).await.unwrap();
        assert_eq!(job.node, "stable-four");
        assert!(first.ran("sparkjob.py").is_empty(), "unselected node receives no tool, job or start command");
        assert_eq!(chosen.ran("sparkjob.py start download").len(), 1);
        ctx.inventory.nodes.reverse();
        assert_eq!(models.download_model(&ctx, &model, &review).await.unwrap().node, "stable-four");
        busy.store(true, Ordering::SeqCst);
        assert!(models.download_model(&ctx, &model, &review).await.unwrap_err().contains("already running on stable-four"));
        busy.store(false, Ordering::SeqCst);
        review.node = "cell4".into();
        assert!(models.download_model(&ctx, &model, &review).await.unwrap_err().contains("inventory node"));
        review.node = "stable-four".into();
        ctx.snapshot.as_mut().unwrap().nodes.iter_mut().find(|n| n.id == "stable-four").unwrap().status = LinkStatus::Offline;
        assert!(models.download_model(&ctx, &model, &review).await.unwrap_err().contains("not connected"));
        assert_eq!(chosen.ran("sparkjob.py start download").len(), 2, "no replacement target or start after refusal");
        assert!(first.ran("sparkjob.py").is_empty());
        live.stop();
    }

    #[tokio::test]
    async fn demo_download_records_the_selected_node() {
        let base = temp("target-demo");
        let vpn = Vpn::new(base.join("vpn.json"), base.join("no-helper"), |_| {});
        let llm = Llm::new(base.join("providers.json"), vpn.clone());
        let live = crate::live::Live::new(vpn, base.join("known_hosts.json"), || panic!("Demo must not read a credential"));
        let models = Models::new(base.clone(), live, llm, None);
        let ctx = Ctx { live: false, settings: Default::default(), inventory: crate::inventory::Inventory::example(), snapshot: None };
        let node = ctx.node_ids().last().unwrap().clone();
        let review = DownloadToolReview { node: node.clone(), tool: demo_tools(&node).options[0].clone(), install_consent: false };
        let model = hf::parse_model(&json!({"id":"org/model","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":100}]}),json!({}),None,"");
        assert_eq!(models.download_model(&ctx, &model, &review).await.unwrap().node, node);
    }
}
