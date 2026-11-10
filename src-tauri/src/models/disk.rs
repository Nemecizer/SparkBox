//! Destination filesystem admission, independent of the dashboard's cached root-disk reading.
use super::*;
const GIB: u64 = 1024 * 1024 * 1024;

#[cfg(test)]
pub(super) fn fixture_read(bytes: u64) -> (String, u32) {
    (json!(demo_check("fixture", "/models/fixture", bytes)).to_string(), 0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskCheck {
    pub node: String,
    pub dest: String,
    pub probe: String,
    pub transfer_bytes: u64,
    pub available_bytes: u64,
    pub capacity_bytes: u64,
    pub required_bytes: u64,
    pub used_percent: f64,
    pub projected_used_percent: f64,
    pub ok: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DiskRequest {
    Download { node: String, selection: hf::DownloadSelection },
    Resume { node: String, id: String, expected: DownloadResume },
    Copy { key: String, targets: Vec<String> },
}

pub(super) fn file_bytes(files: &Value) -> Result<u64, String> {
    let files = files.as_array().filter(|f| !f.is_empty()).ok_or("The transfer has no known file sizes. Refresh its details.")?;
    files.iter().try_fold(0_u64, |sum, file| {
        file["size"].as_u64().and_then(|n| sum.checked_add(n)).filter(|n| *n < (1_u64 << 53))
            .ok_or_else(|| "The transfer has unknown or invalid file sizes. Refresh its details before starting.".into())
    })
}

impl DiskCheck {
    pub(super) fn require(&self) -> Result<(), String> {
        if self.ok { return Ok(()); }
        Err(format!("Not enough disk space on {} at {}: {:.2} GiB available; {:.2} GiB required (full transfer plus 5% and 20 GiB).",
            self.node, self.dest, self.available_bytes as f64 / GIB as f64, self.required_bytes as f64 / GIB as f64))
    }
}

pub(super) fn demo_check(node: &str, dest: &str, bytes: u64) -> DiskCheck {
    let required = (bytes * 105).div_ceil(100) + 20 * GIB;
    let projected = (50.0 + 100.0 * bytes as f64 / (4_000 * GIB) as f64).min(100.0);
    DiskCheck { node: node.into(), dest: dest.into(), probe: "/home/demo".into(), transfer_bytes: bytes,
        available_bytes: 2_000 * GIB, capacity_bytes: 4_000 * GIB, required_bytes: required,
        used_percent: 50.0, projected_used_percent: projected,
        ok: 2_000 * GIB >= required, warning: (projected > 85.0).then(|| format!("Demo: this disk could be {projected:.1}% full after the transfer.")) }
}

impl Models {
    pub(super) async fn destination_disk(&self, ctx: &Ctx, node: &str, dest: &str, bytes: u64) -> Result<DiskCheck, String> {
        if !ctx.node_ids().iter().any(|id| id == node) { return Err("Choose an inventory node.".into()); }
        if bytes > (1_u64 << 53) - 1 { return Err("The transfer size is invalid.".into()); }
        if !ctx.live { return Ok(demo_check(node, dest, bytes)); }
        if !ctx.online().iter().any(|id| id == node) { return Err(format!("{node} is not connected. Reconnect it before checking disk space.")); }
        let raw = self.sparkjob(node, "disk-preflight", Some(json!({"dest":dest,"bytes":bytes,"node":node}).to_string().as_bytes()), Duration::from_secs(20)).await?;
        let mut check: DiskCheck = serde_json::from_value(raw).map_err(|_| format!("Cannot read destination disk space on {node}."))?;
        // The trusted inventory determines routing and display; malformed readings never pass.
        check.node = node.into();
        let need = bytes.checked_mul(105).and_then(|n| n.checked_add(99)).map(|n| n / 100 + 20 * GIB).ok_or("The required disk space is too large.")?;
        if check.transfer_bytes != bytes || check.required_bytes != need || check.capacity_bytes == 0
            || check.available_bytes > check.capacity_bytes || check.ok != (check.available_bytes >= need)
            || !check.used_percent.is_finite() || !check.projected_used_percent.is_finite() {
            return Err(format!("Invalid destination disk reading on {node}; nothing was started."));
        }
        Ok(check)
    }

    /// A review reads only; every start repeats admission against the same reviewed file list.
    pub async fn disk_preflight(&self, ctx: &Ctx, request: &DiskRequest) -> Result<Vec<DiskCheck>, String> {
        match request {
            DiskRequest::Download { node, selection } => {
                // Check identity/connection before any Hugging Face request.
                if !ctx.node_ids().contains(node) || (ctx.live && !ctx.online().contains(node)) { return Err("Choose a connected inventory node.".into()); }
                // Native Demo already reads Hugging Face metadata for details/downloads;
                // simulate storage capacity, not the model's transfer size.
                let model = self.reviewed_model(selection).await?;
                let bytes = file_bytes(&json!(model.files))?;
                Ok(vec![self.destination_disk(ctx, node, &format!("{}/{}", ctx.settings.model_root.trim_end_matches('/'), model.id), bytes).await?])
            }
            DiskRequest::Resume { node, id, expected } => {
                if !ctx.node_ids().contains(node) || (ctx.live && !ctx.online().contains(node)) { return Err("Choose a connected inventory node.".into()); }
                let bytes = if ctx.live {
                    let list = self.sparkjob(node, "list --look", None, Duration::from_secs(20)).await?;
                    let old = list.as_array().and_then(|rows| rows.iter().find(|j| j["id"] == *id)).ok_or("Refresh Jobs; the saved download was not found.")?;
                    let spec = download_recovery::checked_spec(old, expected, crate::identity::instance_name())?;
                    file_bytes(&spec["files"])?
                } else {
                    self.demo.lock().unwrap().jobs().into_iter()
                        .find(|j| j.node == *node && j.id == *id && matches!(j.state.as_str(), "failed" | "cancelled") && j.resume.as_ref() == Some(expected))
                        .ok_or("Refresh Jobs and select the stopped Demo download.")?.bytes_total
                };
                Ok(vec![self.destination_disk(ctx, node, &expected.dest, bytes).await?])
            }
            DiskRequest::Copy { key, targets } => {
                if targets.is_empty() || targets.iter().any(|n| !ctx.node_ids().contains(n)) { return Err("Choose inventory nodes to copy to.".into()); }
                let entry = self.library(ctx).await?.into_iter().find(|e| e.key == *key).ok_or("Refresh Library; the model was not found.")?;
                let bytes = if ctx.live {
                    let source = copy_source(ctx, &entry)?;
                    let manifest = self.sparkjob(&source, &format!("manifest {}", q(key)), None, Duration::from_secs(60)).await?;
                    file_bytes(&manifest["files"])?
                } else { entry.size_bytes };
                let mut checks = Vec::new();
                for node in targets { checks.push(self.destination_disk(ctx, node, key, bytes).await?); }
                Ok(checks)
            }
        }
    }
}

pub(super) fn copy_source(ctx: &Ctx, entry: &LibraryEntry) -> Result<String, String> {
    ctx.node_ids().into_iter().find(|n| entry.copies.get(n).is_some_and(|c| c.verified))
        .or_else(|| entry.copies.iter().find(|(_, c)| c.partial.is_none()).map(|(n, _)| n.clone()))
        .ok_or_else(|| "No node has a finished copy of that model.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{self, fake_node, temp, Script};
    use crate::vpn::Vpn;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn native_demo_reviews_the_actual_copy_and_saved_download_sizes_without_starting_jobs() {
        let base = temp("disk-demo");
        let vpn = Vpn::new(base.join("vpn.json"), base.join("no-helper"), |_| {});
        let llm = Llm::new(base.join("providers.json"), vpn.clone());
        let live = crate::live::Live::new(vpn, base.join("known_hosts.json"), || panic!("Demo storage checks must not read credentials"));
        let models = Models::new(base, live, llm, None);
        let ctx = Ctx { live: false, settings: Default::default(), inventory: crate::inventory::Inventory::example(), snapshot: None };
        let node = ctx.node_ids()[0].clone();
        let entry = models.library(&ctx).await.unwrap().remove(0);
        let checks = models.disk_preflight(&ctx, &DiskRequest::Copy { key: entry.key, targets: vec![node.clone()] }).await.unwrap();
        assert_eq!(checks[0].transfer_bytes, entry.size_bytes);
        let model = hf::parse_model(&json!({"id":"fixture/disk","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":123}]}), json!({}), None, "");
        let job = models.demo.lock().unwrap().download(&node, &model, "/models/fixture/disk");
        models.demo.lock().unwrap().cancel(&job.id);
        let before = models.demo.lock().unwrap().jobs().len();
        let mut expected = job.resume.unwrap();
        let checks = models.disk_preflight(&ctx, &DiskRequest::Resume { node: node.clone(), id: job.id.clone(), expected: expected.clone() }).await.unwrap();
        assert_eq!(checks[0].transfer_bytes, 123);
        assert_eq!(checks[0].required_bytes, (123_u64 * 105).div_ceil(100) + 20 * GIB);
        expected.plan_hash = "changed".into();
        assert!(models.disk_preflight(&ctx, &DiskRequest::Resume { node, id: job.id, expected }).await.is_err());
        assert_eq!(models.demo.lock().unwrap().jobs().len(), before);
    }

    #[test]
    fn unknown_sizes_and_overflow_refuse_without_discounting_partial_files() {
        assert_eq!(file_bytes(&json!([{"size":0},{"size":123}])).unwrap(), 123);
        for files in [json!([]), json!([{}]), json!([{"size":null}]), json!([{"size":true}]), json!([{"size":-1}]), json!([{"size":u64::MAX}])] {
            assert!(file_bytes(&files).is_err(), "{files}");
        }
        let check = demo_check("stable-node", "/models/model", 1);
        assert_eq!(check.required_bytes, 20 * GIB + 2);
        assert!(demo_check("stable-node", "/models/model", 1_700 * GIB).warning.is_some());
        assert!(demo_check("stable-node", "/models/model", 2_000 * GIB).require().is_err());
    }

    #[tokio::test]
    async fn download_rechecks_space_after_review_and_refuses_malformed_or_offline_readings() {
        let available = Arc::new(AtomicU64::new(2_000 * GIB));
        let space = available.clone();
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py disk-preflight") {
                let mut check: Value = serde_json::from_str(&fixture_read(100).0).unwrap();
                check["availableBytes"] = json!(space.load(Ordering::SeqCst));
                check["ok"] = json!(space.load(Ordering::SeqCst) >= check["requiredBytes"].as_u64().unwrap());
                return Some((check.to_string(), 0));
            }
            if cmd.contains("sparkjob.py download-tools") { return Some((json!(download_tools::demo_tools("cell2")).to_string(), 0)); }
            if cmd.contains("sparkjob.py list") { return Some(("[]".into(), 0)); }
            if cmd.contains("sparkjob.py start download") { return Some((json!({"id":"fixture-download","kind":"download","state":"running"}).to_string(),0)); }
            None
        });
        let (node, port) = fake_node(&temp("disk-review-home"), script).await;
        let base = temp("disk-review-app");
        let (live, inventory) = fake_node::connect(&base, &[("cell2", port)]).await;
        let llm = Llm::new(base.join("providers.json"), Vpn::new(base.join("vpn.json"),base.join("no-helper"), |_| {}));
        let models = Models::new(base,live.clone(),llm,None);
        let mut ctx = Ctx { live:true, settings:Default::default(), snapshot:Some(live.snapshot(&inventory)), inventory };
        models.destination_disk(&ctx,"cell2","~/models/org/model",100).await.unwrap().require().unwrap();
        assert!(node.ran("sparkjob.py start").is_empty());
        available.store(1,Ordering::SeqCst);
        let review = DownloadToolReview { node:"cell2".into(),tool:download_tools::demo_tools("cell2").options[0].clone(),install_consent:false };
        let model = hf::parse_model(&json!({"id":"org/model","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":100}]}),json!({}),None,"");
        assert!(models.download_model(&ctx,&model,&review).await.unwrap_err().contains("Not enough disk space on cell2"));
        assert!(node.ran("sparkjob.py start").is_empty());
        available.store(u64::MAX,Ordering::SeqCst);
        assert!(models.destination_disk(&ctx,"cell2","~/models/org/model",100).await.unwrap_err().contains("Invalid destination disk"));
        ctx.snapshot.as_mut().unwrap().nodes[0].status = LinkStatus::Offline;
        let reads = node.ran("sparkjob.py disk-preflight").len();
        assert!(models.destination_disk(&ctx,"cell2","~/models/org/model",100).await.unwrap_err().contains("not connected"));
        assert_eq!(node.ran("sparkjob.py disk-preflight").len(),reads);
        live.stop();
    }

    #[tokio::test]
    async fn every_copy_destination_is_checked_before_any_source_or_pull_job_starts() {
        let mut nodes = Vec::new(); let mut ports = Vec::new();
        let key = "/models/org/model";
        for index in 0..3 {
            let script: Script = Arc::new(move |cmd,_| {
                if cmd.contains("sparkjob.py scan") {
                    return Some((if index == 0 { json!([{"path":key,"repo":"org/model","sizeBytes":12,"verified":true,"format":"safetensors"}]) } else { json!([]) }.to_string(),0));
                }
                if cmd.contains("sparkjob.py manifest") { return Some((json!({"files":[{"path":"model.safetensors","size":12}]}).to_string(),0)); }
                if cmd.contains("sparkjob.py disk-preflight") {
                    let (text,code) = fixture_read(12);
                    if index != 2 { return Some((text,code)); }
                    let mut check:Value=serde_json::from_str(&text).unwrap();check["availableBytes"]=json!(1);check["ok"]=json!(false);
                    return Some((check.to_string(),0));
                }
                None
            });
            let (node,port)=fake_node(&temp(&format!("disk-copy-home-{index}")),script).await;
            nodes.push(node);ports.push(port);
        }
        let base=temp("disk-copy-app");
        let (live,inventory)=fake_node::connect(&base,&[("cell1",ports[0]),("cell2",ports[1]),("cell3",ports[2])]).await;
        let llm=Llm::new(base.join("providers.json"),Vpn::new(base.join("vpn.json"),base.join("no-helper"), |_| {}));
        let models=Models::new(base,live.clone(),llm,None);
        let ctx=Ctx {live:true,settings:Default::default(),snapshot:Some(live.snapshot(&inventory)),inventory};
        let targets=vec!["cell2".into(),"cell3".into()];
        let checks=models.disk_preflight(&ctx,&DiskRequest::Copy {key:key.into(),targets:targets.clone()}).await.unwrap();
        assert!(checks[0].ok);assert!(!checks[1].ok);
        assert!(models.share(&ctx,key,&targets).await.unwrap_err().contains("Not enough disk space on cell3"));
        assert!(nodes.iter().all(|node| node.ran("sparkjob.py start").is_empty()),"no source server or pull starts when any target is short");
        live.stop();
    }
}
