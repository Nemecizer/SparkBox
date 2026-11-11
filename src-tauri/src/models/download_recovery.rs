//! Resume an owned, stopped download from its node's non-secret pinned plan.
//! The node revalidates that plan and the partial marker under its existing folder lock.
use super::*;

fn chain_job(job: &Value) -> bool {
    // Older source records lost their participant metadata, but kept the reserved chain ID.
    job["id"].as_str().is_some_and(|id| id.starts_with("dc-")) || !job["chain"].is_null()
}

pub(super) fn summary(job: &Value, instance: &str) -> Option<DownloadResume> {
    if chain_job(job) || job["kind"] != "download" || !matches!(job["state"].as_str(), Some("failed" | "cancelled")) || job["instance"] != instance {
        return None;
    }
    let plan = &job["downloadPlan"];
    let text = |key: &str| plan[key].as_str().filter(|s| !s.is_empty()).map(String::from);
    let resume = DownloadResume { repo: text("repo")?, revision: text("revision")?, dest: text("dest")?, plan_hash: job["planHash"].as_str()?.into() };
    let hex = |s: &str, len| s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    if !hex(&resume.revision, 40) || !hex(&resume.plan_hash, 64) || job["dest"] != resume.dest
        || !plan["files"].as_array().is_some_and(|f| !f.is_empty()) || text("image").is_none() {
        return None;
    }
    Some(resume)
}

pub(super) fn checked_spec(job: &Value, expected: &DownloadResume, instance: &str) -> Result<Value, String> {
    if chain_job(job) {
        return Err("A download-copy chain cannot use ordinary Resume. Review Jobs on its nodes and cancel remaining owned jobs before a separately reviewed recovery; existing files and records are preserved.".into());
    }
    if summary(job, instance).as_ref() != Some(expected) {
        return Err("This download is no longer resumable with the plan you reviewed. Refresh Jobs; older jobs need a newly reviewed download from Hugging Face.".into());
    }
    // Whitelist again at the app boundary: never replay credentials or arbitrary saved fields.
    let saved = &job["downloadPlan"];
    let mut spec = json!({});
    for key in ["repo", "revision", "dest", "image", "files", "include"] {
        spec[key] = saved[key].clone();
    }
    spec["instance"] = json!(instance);
    spec["resumeOf"] = job["id"].clone();
    spec["resumeHash"] = json!(expected.plan_hash);
    spec["label"] = json!(format!("Resume {}", expected.repo));
    Ok(spec)
}

impl Models {
    pub async fn resume_download(self: &Arc<Self>, ctx: &Ctx, node: &str, id: &str, expected: &DownloadResume, review: &DownloadToolReview) -> Result<Job, String> {
        if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || !ctx.node_ids().iter().any(|n| n == node) {
            return Err("Choose a saved download on an inventory node.".into());
        }
        if id.starts_with("dc-") {
            return Err("A download-copy chain cannot use ordinary Resume. Review its remaining jobs before a separately reviewed recovery.".into());
        }
        self.check_download_tool(ctx, node, review).await?;
        if !ctx.live {
            let mut demo = self.demo.lock().unwrap();
            let mut job = demo.resume_download(node, id, expected)?;
            demo.note_download_tool(&mut job, &review.tool);
            self.live.audit.record_simulated(node, "Resume a download", &format!("python3 ~/.sparkbox/sparkjob.py start download {}", job.id));
            return Ok(job);
        }
        if !ctx.online().iter().any(|n| n == node) {
            return Err(format!("{node} is not connected. Reconnect it, then refresh Jobs."));
        }
        let list = self.list_jobs(node).await?;
        let old = list.as_array().and_then(|rows| rows.iter().find(|j| j["id"] == id)).ok_or("The saved download was not found. Refresh Jobs.")?;
        let mut spec = checked_spec(old, expected, crate::identity::instance_name())?;
        self.destination_disk(ctx, node, &expected.dest, disk::file_bytes(&spec["files"])?).await?.require()?;
        // Only stdin carries the fresh Keychain token; the helper never saves it in the plan.
        spec["token"] = json!(Self::hf_token().unwrap_or_default());
        spec["tool"] = json!(review.tool);
        spec["installConsent"] = json!(review.install_consent);
        let job = self.start_job(node, "download", &format!("dl-{}", short_id()), &spec).await?;
        self.follow_job(&job, format!("Resume {} on {node}", expected.repo));
        Ok(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn saved() -> Value {
        json!({"id":"dl-old","kind":"download","state":"cancelled","instance":"mine","dest":"/home/test/models/org/model",
            "planHash":"c".repeat(64),"downloadPlan":{"repo":"org/model","revision":"a".repeat(40),"dest":"/home/test/models/org/model",
            "image":"vllm:test","files":[{"path":"model.safetensors","size":123,"sha256":"b".repeat(64)}],"include":[],"token":"must-not-replay","password":"must-not-replay"}})
    }
    #[test]
    fn chain_rows_cannot_fall_back_to_download_only_resume() {
        let row = json!({"kind":"download","state":"failed","instance":"mine","chain":{"id":"dc-fixture"},
            "dest":"/models/a","planHash":"c".repeat(64),"downloadPlan":{"repo":"fixture/model","revision":"a".repeat(40),"dest":"/models/a","files":[{"path":"a","size":1}],"image":"fixture"}});
        assert!(summary(&row,"mine").is_none());
        let mut ordinary=row;ordinary.as_object_mut().unwrap().remove("chain");
        assert!(summary(&ordinary,"mine").is_some());
    }

    #[test]
    fn legacy_chain_id_without_participants_is_not_an_ordinary_resume() {
        let ordinary = saved();
        let expected = summary(&ordinary, "mine").unwrap();
        let mut legacy = ordinary.clone();
        legacy["id"] = json!("dc-fixture-0");
        let before = legacy.clone();
        assert!(summary(&legacy, "mine").is_none());
        assert!(checked_spec(&legacy, &expected, "mine").unwrap_err().contains("chain"));
        assert_eq!(legacy, before, "no participant plan is invented or written");
        legacy["chain"] = Value::Null;
        assert!(summary(&legacy, "mine").is_none());
        assert!(summary(&ordinary, "mine").is_some());
        assert!(checked_spec(&ordinary, &expected, "mine").is_ok());
    }

    #[test]
    fn resume_pins_the_original_plan_without_replaying_secrets() {
        let job = saved();
        let view = summary(&job, "mine").unwrap();
        let spec = checked_spec(&job, &view, "mine").unwrap();
        assert_eq!(spec["revision"], job["downloadPlan"]["revision"]);
        assert_eq!(spec["files"], job["downloadPlan"]["files"]);
        assert_eq!(spec["resumeOf"], "dl-old");
        assert!(!spec.to_string().contains("must-not-replay"));
    }
    #[test]
    fn changed_foreign_running_finished_and_legacy_jobs_cannot_resume() {
        let job = saved();
        let view = summary(&job, "mine").unwrap();
        for (key, value) in [("state", json!("running")), ("state", json!("done")), ("instance", json!("other")),
            ("downloadPlan", Value::Null), ("planHash", json!("d".repeat(64))), ("dest", json!("/elsewhere"))] {
            let mut changed = job.clone(); changed[key] = value;
            assert!(checked_spec(&changed, &view, "mine").is_err(), "{key}");
        }
        let mut changed = job.clone(); changed["downloadPlan"]["revision"] = json!("b".repeat(40));
        assert!(checked_spec(&changed, &view, "mine").is_err());
    }

    #[tokio::test]
    async fn resume_rereads_the_node_and_uses_the_existing_guarded_start() {
        use crate::fake_node::{self, fake_node, temp, Script};
        use crate::live::exit::{Enforce, WritePolicy};
        use crate::vpn::Vpn;
        struct LookOnly;
        impl WritePolicy for LookOnly {
            fn look_only(&self) -> bool { true }
            fn admin_unlocked(&self) -> bool { true }
        }
        let instance = crate::identity::instance_name();
        let mut row = saved(); row["instance"] = json!(instance);
        let expected = summary(&row, instance).unwrap();
        let response = row.clone();
        let review = DownloadToolReview { node: "cell1".into(), tool: download_tools::demo_tools("cell1").options[0].clone(), install_consent: false };
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py disk-preflight") { return Some(disk::fixture_read(123)); }
            if cmd.contains("sparkjob.py download-tools") { return Some((json!(download_tools::demo_tools("cell1")).to_string(), 0)); }
            if cmd.contains("sparkjob.py list") { return Some((json!([response]).to_string(), 0)); }
            if cmd.contains("sparkjob.py start download") {
                return Some((json!({"id":cmd.split_whitespace().last().unwrap(),"kind":"download","state":"running"}).to_string(), 0));
            }
            None
        });
        let (node, port) = fake_node(&temp("resume-home"), script).await;
        let base = temp("resume-app");
        let (live, inventory) = fake_node::connect(&base, &[("cell1", port)]).await;
        let llm = Llm::new(base.join("providers.json"), Vpn::new(base.join("vpn.json"), base.join("no-helper"), |_| {}));
        let m = Models::new(base.clone(), live.clone(), llm, None);
        let ctx = Ctx { live: true, settings: Default::default(), snapshot: Some(live.snapshot(&inventory)), inventory };
        let mut stale = expected.clone(); stale.plan_hash = "d".repeat(64);
        assert!(m.resume_download(&ctx, "cell1", "dl-old", &stale, &review).await.is_err());
        assert!(m.resume_download(&ctx, "missing", "dl-old", &expected, &review).await.is_err());
        assert!(node.ran("sparkjob.py start").is_empty());
        let job = m.resume_download(&ctx, "cell1", "dl-old", &expected, &review).await.unwrap();
        assert_ne!(job.id, "dl-old");
        assert_eq!(job.node, "cell1");
        let spec: Value = {
            let inputs = node.stdin.lock().unwrap();
            let (_, bytes) = inputs.iter().find(|(cmd, _)| cmd.contains("sparkjob.py start download")).unwrap();
            serde_json::from_slice(bytes).unwrap()
        };
        assert_eq!(spec["revision"], expected.revision);
        assert_eq!(spec["resumeOf"], "dl-old");
        assert!(!spec.to_string().contains("must-not-replay"));
        live.enforce_writes(Enforce { instance: instance.into(), fence_dir: base.join("Fence"), config_dir: Some(base.join("config")),
            noauth: false, tag: "resume-test".into(), mac: Some("test Mac".into()) });
        let refusal = m.resume_download(&ctx, "cell1", "dl-old", &expected, &review).await.unwrap_err();
        assert!(refusal.contains("fence"), "{refusal}");
        live.set_write_policy(Arc::new(LookOnly));
        assert!(m.resume_download(&ctx, "cell1", "dl-old", &expected, &review).await.is_err());
        assert_eq!(node.ran("sparkjob.py start").len(), 1, "neither refusal sends a new start");
        live.stop();
    }

    #[test]
    fn demo_resume_keeps_old_attempt_and_refuses_a_second_claim() {
        let mut demo = demo::DemoModels::new();
        let model = hf::parse_model(&json!({"id":"org/model","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":100}]}), json!({}), None, "");
        let old = demo.download("cell1", &model, "~/models/org/model");
        demo.cancel(&old.id);
        let expected = old.resume.unwrap();
        assert!(demo.resume_download("cell2", &old.id, &expected).is_err());
        let next = demo.resume_download("cell1", &old.id, &expected).unwrap();
        assert_ne!(next.id, old.id);
        assert!(demo.resume_download("cell1", &old.id, &expected).is_err());
        let rows = demo.jobs();
        assert_eq!(rows.iter().find(|j| j.id == old.id).unwrap().state, "cancelled");
        assert_eq!(rows.iter().find(|j| j.id == next.id).unwrap().state, "running");
    }
}
