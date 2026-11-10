//! Detached node-owned image jobs. No registry credentials or tar bytes pass through the Mac.
use super::*;

impl Models {
    /// How To uses only the existing quick facts, never the full container test.
    pub(crate) async fn howto_checks(&self, ctx: &Ctx, nodes: &[String]) -> Result<Vec<NodeCheck>, String> {
        if !ctx.live {
            return Ok(demo::checks(nodes, &ctx.settings, ctx.snapshot.as_ref()));
        }
        Ok(join_all(nodes.iter().map(|node| self.check_node(ctx, node, true))).await)
    }

    pub async fn image_jobs(self: &Arc<Self>, ctx: &Ctx, image: &str, targets: &[String], source: Option<&str>, estimated_bytes: u64) -> Result<Vec<Job>, String> {
        if !plan::is_image_ref(image) || image.contains('@') || image.starts_with("sha256:") {
            return Err("Choose a tagged image reference.".into());
        }
        let known = ctx.node_ids();
        if targets.is_empty() || targets.iter().any(|n| !known.contains(n)) || targets.iter().collect::<std::collections::HashSet<_>>().len() != targets.len()
            || source.is_some_and(|n| !known.iter().any(|k| k == n) || targets.iter().any(|t| t == n)) {
            return Err("Choose distinct inventory nodes; the source cannot be a target.".into());
        }
        if estimated_bytes == 0 || estimated_bytes > (1u64 << 40) { return Err("Estimate the unpacked image size (up to 1 TiB).".into()); }
        if !ctx.live {
            return Ok(targets.iter().map(|node| {
                self.live.audit.record_simulated(node, "Prepare an engine image", if source.is_some() { "verified image copy and docker load" } else { "Docker Engine image pull" });
                self.demo.lock().unwrap().image_job(node, image, source.is_some(), estimated_bytes)
            }).collect());
        }
        let group = format!("image-{}", short_id());
        let ids: Vec<_> = targets.iter().enumerate().map(|(i, _)| format!("{group}-t{i}")).collect();
        let distribution = if let Some(node) = source {
            let mut check_ctx = ctx.clone();
            check_ctx.settings.image = image.into();
            let check = self.check_node(&check_ctx, node, true).await;
            let image_id = check.versions.get("imageId").and_then(Option::as_ref).filter(|id| plan::is_image_id(id)).ok_or("The selected image has no readable identity on the source.")?;
            let fabric = check.fabric.iter().find(|f| f.ip.starts_with("192.168.100.")).or_else(|| check.fabric.first()).ok_or("The source has no fabric address.")?;
            let net = fabric.ip.rsplit_once('.').filter(|_| fabric.prefix == 24).ok_or("Distribution currently requires an IPv4 /24 fabric network.")?.0;
            let password = token();
            let port = 8730 + (u16::from_str_radix(&group[6..8], 16).unwrap_or(0) % 200);
            let serve = format!("{group}-source");
            let spec = json!({ "image": image, "imageId": image_id, "address": fabric.ip, "port": port, "allow": format!("{net}.0/24"),
                "password": password, "expected": ids, "instance": crate::identity::instance_name(), "label": format!("Export and share {image}") });
            let job = self.start_job(node, "image-save", &serve, &spec).await?;
            self.follow_job(&job, format!("Export and share {image}"));
            Some((format!("rsync://sparkbox@{}:{port}/model", fabric.ip), password, image_id.clone()))
        } else { None };
        let mut jobs = Vec::new();
        for (node, id) in targets.iter().zip(ids) {
            let mut spec = json!({ "image": image, "estimatedBytes": estimated_bytes, "instance": crate::identity::instance_name(), "label": format!("Prepare {image} on {node}") });
            let kind = if let Some((src, password, image_id)) = &distribution {
                spec["src"] = json!(src); spec["password"] = json!(password); spec["imageId"] = json!(image_id);
                "image-load"
            } else { "pull-image" };
            match self.start_job(node, kind, &id, &spec).await {
                Ok(job) => { self.follow_job(&job, format!("Prepare {image} on {node}")); jobs.push(job); }
                Err(e) => return Err(format!("Could not start the image job on {node}: {e}. {} target jobs already started; follow them in Jobs. The source closes when copies finish or after 30 minutes idle.", jobs.len())),
            }
        }
        Ok(jobs)
    }
}
