//! Read-only suggestions for an explicitly chosen set of potential load nodes.
use super::*;
use futures_util::{stream, StreamExt};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityReading {
    node: String,
    check: Option<DiskCheck>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCapacity {
    readings: Vec<CapacityReading>,
    suggested: Option<String>,
}

impl DownloadCapacity {
    fn new(mut readings: Vec<CapacityReading>) -> Self {
        // Stable IDs resolve ties, independent of inventory order or probe completion order.
        readings.sort_by(|a, b| a.node.cmp(&b.node));
        let suggested = readings.iter().filter_map(|r| r.check.as_ref().filter(|c| c.ok))
            .max_by(|a, b| (a.available_bytes - a.required_bytes).cmp(&(b.available_bytes - b.required_bytes))
                .then_with(|| b.node.cmp(&a.node))).map(|c| c.node.clone());
        Self { readings, suggested }
    }
}

impl Models {
    pub async fn download_capacity(&self, ctx: &Ctx, selection: &hf::DownloadSelection, nodes: &[String]) -> Result<DownloadCapacity, String> {
        // Reject foreign or duplicate identities before metadata or SSH access.
        Self::capacity_nodes(ctx, nodes)?;
        let model = self.reviewed_model(selection).await?;
        self.capacity_for_model(ctx, selection, nodes, &model).await
    }

    fn capacity_nodes(ctx: &Ctx, nodes: &[String]) -> Result<(), String> {
        let inventory = ctx.node_ids();
        if nodes.is_empty() || nodes.iter().any(|n| !inventory.contains(n))
            || nodes.iter().collect::<std::collections::HashSet<_>>().len() != nodes.len() {
            return Err("Choose distinct inventory nodes to compare destination capacity.".into());
        }
        Ok(())
    }

    async fn capacity_for_model(&self, ctx: &Ctx, selection: &hf::DownloadSelection, nodes: &[String], model: &HfModel) -> Result<DownloadCapacity, String> {
        Self::capacity_nodes(ctx, nodes)?;
        selection.check(model)?;
        let bytes = disk::file_bytes(&json!(model.files))?;
        let dest = format!("{}/{}", ctx.settings.model_root.trim_end_matches('/'), model.id);
        let readings = stream::iter(nodes.iter().cloned().map(|node| {
            let dest = dest.clone();
            async move {
                match self.destination_disk(ctx, &node, &dest, bytes).await {
                    Ok(check) => CapacityReading { node: node.clone(), check: Some(check), error: None },
                    Err(error) => CapacityReading { node: node.clone(), check: None, error: Some(error) },
                }
            }
        })).buffer_unordered(4).collect().await;
        Ok(DownloadCapacity::new(readings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{self, fake_node, temp, Script};
    use crate::vpn::Vpn;

    fn reading(node: &str, available: u64) -> CapacityReading {
        let mut check = disk::demo_check(node, "/models/fixture/model", 100);
        check.available_bytes = available;
        check.ok = available >= check.required_bytes;
        CapacityReading { node: node.into(), check: Some(check), error: None }
    }

    #[test]
    fn ranking_uses_remaining_reserve_and_stable_ids_and_never_unknown_or_short_space() {
        let need = disk::demo_check("fixture", "/models/fixture/model", 100).required_bytes;
        let unknown = || CapacityReading { node: "unknown".into(), check: None, error: Some("Unreadable".into()) };
        let result = DownloadCapacity::new(vec![reading("b", need + 10), unknown(), reading("a", need + 10), reading("short", need - 1)]);
        assert_eq!(result.suggested.as_deref(), Some("a"));
        assert_eq!(DownloadCapacity::new(vec![reading("a", need), reading("b", need + 1)]).suggested.as_deref(), Some("b"));
        assert!(DownloadCapacity::new(vec![reading("short", need - 1), unknown()]).suggested.is_none());
        assert!(DownloadCapacity::new(vec![]).suggested.is_none());
        // Fullness warnings remain visible and don't erase sufficient capacity.
        let mut warned = reading("warned", need);
        warned.check.as_mut().unwrap().warning = Some("Nearly full".into());
        assert_eq!(DownloadCapacity::new(vec![warned]).suggested.as_deref(), Some("warned"));
    }

    #[tokio::test]
    async fn compares_only_requested_destinations_and_keeps_failures_without_starting_jobs() {
        let mut peers = Vec::new(); let mut ports = Vec::new();
        for index in 0..3 {
            let script: Script = Arc::new(move |cmd, _| {
                if !cmd.contains("sparkjob.py disk-preflight") { return None; }
                if index == 2 { return Some(("unreadable filesystem".into(), 1)); }
                let mut check: Value = serde_json::from_str(&disk::fixture_read(100).0).unwrap();
                check["availableBytes"] = json!(if index == 0 { 1 } else { 2_000 * 1024_u64.pow(3) });
                check["ok"] = json!(index != 0);
                Some((check.to_string(), 0))
            });
            let (peer, port) = fake_node(&temp(&format!("capacity-node-{index}")), script).await;
            peers.push(peer); ports.push(port);
        }
        let base = temp("capacity-app");
        let (live, inventory) = fake_node::connect(&base, &[("cell1", ports[0]), ("cell2", ports[1]), ("cell3", ports[2])]).await;
        let llm = Llm::new(base.join("providers.json"), Vpn::new(base.join("vpn.json"), base.join("no-helper"), |_| {}));
        let models = Models::new(base, live.clone(), llm, None);
        let mut ctx = Ctx { live: true, settings: Default::default(), snapshot: Some(live.snapshot(&inventory)), inventory };
        let model = hf::parse_model(&json!({"id":"fixture/model","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":100}]}), json!({}), None, "");
        let selection = hf::DownloadSelection { repo: model.id.clone(), revision: model.revision.clone(), files: model.files.clone() };
        let selected = vec!["cell3".into(), "cell2".into(), "cell1".into()];
        let result = models.capacity_for_model(&ctx, &selection, &selected, &model).await.unwrap();
        assert_eq!(result.suggested.as_deref(), Some("cell2"));
        assert!(!result.readings[0].check.as_ref().unwrap().ok);
        assert!(result.readings[2].error.is_some());
        let previous = peers[1].ran("sparkjob.py disk-preflight").len();
        let result = models.capacity_for_model(&ctx, &selection, &["cell1".into()], &model).await.unwrap();
        assert!(result.suggested.is_none());
        assert_eq!(peers[1].ran("sparkjob.py disk-preflight").len(), previous, "an excluded node is not probed");
        ctx.snapshot.as_mut().unwrap().nodes.iter_mut().find(|n| n.id == "cell2").unwrap().status = LinkStatus::Offline;
        let result = models.capacity_for_model(&ctx, &selection, &["cell2".into()], &model).await.unwrap();
        assert!(result.readings[0].error.as_ref().unwrap().contains("not connected"));
        assert!(result.suggested.is_none());
        assert_eq!(peers[1].ran("sparkjob.py disk-preflight").len(), previous);
        let mut changed = selection.clone(); changed.files[0].size = Some(101);
        assert!(models.capacity_for_model(&ctx, &changed, &selected, &model).await.unwrap_err().contains("no longer matches"));
        // Invalid identities refuse before the public entry point can contact the Hub.
        for ids in [vec![], vec!["foreign".into()], vec!["cell1".into(), "cell1".into()]] {
            assert!(models.download_capacity(&ctx, &selection, &ids).await.unwrap_err().contains("distinct inventory nodes"));
        }
        assert!(peers.iter().all(|n| n.ran("sparkjob.py start").is_empty() && n.ran("download-tools").is_empty()));
        live.stop();
    }
}
