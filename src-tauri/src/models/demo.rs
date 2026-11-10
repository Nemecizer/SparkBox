//! Demo mode for Models: a simulated library, jobs and loads with realistic
//! timings, so the whole flow can be clicked through without the cluster. A load
//! reads its weights at 1.5 GB/s and then spends 40 s compiling and capturing
//! CUDA graphs, so a 29 GiB model takes about a minute and a 131 GiB one about
//! two and a half. The serving scenarios fail what they name: a load runs out of
//! memory at 80% (model-oom), a Ray worker never joins (ray-join-timeout), a
//! gated download is refused (gated-401), a copy fills its disk (copy-disk-full).
//! While the as-found world plays, the library is what the nodes really hold (GLM-5.3-Flash in
//! ~/models, three models in the Hugging Face cache) and Ollama is the native one on cell1 and
//! cell3: version 0.21.0, gemma4 on cell3's disk, nothing in memory.

use super::DownloadTool;
use std::time::Instant;

use serde_json::{json, Value};

use super::hf::HfModel;
use super::metrics::OllamaLoaded;
use super::ollama::OllamaModel;
use super::plan::{DeploymentState, Stage};
use super::{Copy, DownloadResume, Deployment, FabricIf, Job, LibraryEntry, ModelSettings, NodeCheck};

/// Copies over the fabric: rsync over TCP from a temporary server, about 19 Gb/s.
const COPY_BYTES_PER_S: f64 = 2.4e9;
/// Downloads from Hugging Face on a fast home connection (Demo shortens them a lot).
const DOWNLOAD_BYTES_PER_S: f64 = 1.2e9;
/// Reading weights from the NVMe into unified memory, per node (each reads its own share).
const LOAD_BYTES_PER_S: f64 = 1.6e9;
/// torch.compile and CUDA graph capture after the weights are in.
const WARM_S: f64 = 30.0;

/// How long a simulated load takes once its nodes are up: each node's share of the weights at
/// 1.6 GB/s, then the warm-up. Llama 3.3 70B over three nodes: 29 s of weights, a minute in all.
pub fn load_seconds(weights: f64, nodes: u64) -> (f64, f64) {
    (weights / nodes.max(1) as f64 / LOAD_BYTES_PER_S, WARM_S)
}
/// How long the head waits for a Ray worker before giving up.
const JOIN_TIMEOUT_S: f64 = 120.0;

fn root() -> String {
    format!("{}/{}", crate::demo::world::home(), crate::demo::world::MODELS_DIR)
}

struct DemoJob {
    job: Job,
    started: Instant,
    seconds: f64,
    /// Library change applied when the job finishes: (key, node).
    adds_copy: Option<(String, String)>,
    applied: bool,
    /// The node a copy reads from (it sends over the fabric while the job runs).
    source: Option<String>,
}

pub struct DemoModels {
    library: Vec<LibraryEntry>,
    jobs: Vec<DemoJob>,
    deployments: Vec<(Deployment, Instant)>,
    /// What gx10-0000's Ollama has installed; the sim's loaded models stay in memory.
    ollama: Vec<OllamaModel>,
    /// The scenario playing (DemoCluster's), for the serving faults this side plays.
    scenario: Option<String>,
    /// The world the library and Ollama reflect.
    world: crate::demo::world::World,
}

fn ollama_model(name: &str, size: u64, family: &str, params: &str, quant: &str, modified: u64) -> OllamaModel {
    OllamaModel {
        name: name.into(),
        size_bytes: size,
        modified_at: Some(modified),
        family: Some(family.into()),
        parameter_size: Some(params.into()),
        quantization: Some(quant.into()),
        loaded: None,
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A library entry for a model folder, as the scan finds it on each node that has a copy.
fn entry(f: &crate::demo::world::ModelFolder) -> LibraryEntry {
    let home = crate::demo::world::home();
    let key = f.path(&home);
    LibraryEntry {
        key: key.clone(),
        in_use: None,
        repo: f.repo.into(),
        revision: f.hf_revision.unwrap_or("").into(),
        format: "safetensors".into(),
        size_bytes: f.bytes,
        root: f.root(&home),
        copies: f.nodes.iter().map(|n| (n.to_string(), Copy { path: key.clone(), size_bytes: f.bytes, verified: true, ..Default::default() })).collect(),
    }
}

/// gx10-0000's Ollama in the everyday world: four models, two of them kept in memory.
fn everyday_ollama() -> Vec<OllamaModel> {
    let t = now();
    vec![
        ollama_model("gpt-oss:20b", 13_793_437_013, "gptoss", "20.9B", "MXFP4", t - 12 * 86_400),
        ollama_model("qwen3:32b", 20_201_253_588, "qwen3", "32.8B", "Q4_K_M", t - 9 * 86_400),
        ollama_model("llama3.2:3b", 2_019_393_189, "llama", "3.2B", "Q4_K_M", t - 40 * 86_400),
        ollama_model("nomic-embed-text:latest", 274_302_450, "nomic-bert", "137M", "F16", t - 40 * 86_400),
    ]
}

/// cell3's Ollama as found: gemma4 pulled on Apr 18, 8.9 GiB on disk (cell1's has none).
fn found_ollama(node: &str) -> Vec<OllamaModel> {
    if node != "cell3" {
        return Vec::new();
    }
    let pulled = crate::demo::world::found(1_776_513_600, now());
    vec![ollama_model("gemma4:latest", 9_556_302_438, "gemma4", "8.0B", "Q4_K_M", pulled)]
}

impl DemoModels {
    /// The same model folders every demo view shows (demo/world.rs).
    pub fn new() -> Self {
        DemoModels {
            library: crate::demo::world::MODEL_FOLDERS.iter().map(entry).collect(),
            jobs: Vec::new(),
            deployments: Vec::new(),
            scenario: None,
            ollama: everyday_ollama(),
            world: crate::demo::world::World::Everyday,
        }
    }

    /// Follows the simulator's scenario; entering or leaving the as-found world swaps the
    /// library and Ollama for that world's (what was downloaded or pulled in the other goes).
    pub fn set_scenario(&mut self, id: Option<String>) {
        use crate::demo::world::World;
        let world = if id.as_deref() == Some("as-found") { World::AsFound } else { World::Everyday };
        if world != self.world {
            self.world = world;
            self.library = world.folders().iter().map(entry).collect();
            self.ollama = if world == World::AsFound { Vec::new() } else { everyday_ollama() };
            self.jobs.clear();
        }
        self.scenario = id;
    }

    /// One node's Ollama: (installed models, version). The everyday world's runs on gx10-0000
    /// in Docker (0.12.6); as found, cell1 and cell3 run it natively (0.21.0), nothing loaded.
    /// For `Models::ollama_list` in Demo, which knows the endpoint's node.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn ollama_view(&self, node: &str) -> (Vec<OllamaModel>, String) {
        if self.world == crate::demo::world::World::AsFound {
            return (found_ollama(node), crate::demo::world::OLLAMA_VERSION.into());
        }
        (self.ollama_models(), "0.12.6".into())
    }

    fn playing(&self, id: &str) -> bool {
        self.scenario.as_deref() == Some(id)
    }

    /// A load's head died (the cluster side saw a worker go): Down with the reason, and its log says why.
    pub fn fail(&mut self, id: &str, why: &str) {
        if let Some((d, _)) = self.deployments.iter_mut().find(|(d, _)| d.spec.id == id && d.state.active()) {
            d.recoverable |= d.state == DeploymentState::Ready;
            d.state = DeploymentState::Failed;
            d.message = why.into();
            d.log_tail = format!("{}\n{}", d.log_tail, why.rsplit(" · ").next().unwrap_or(why)).trim().to_string();
        }
    }

    /// Installed models, with the ones the simulated server keeps in memory (the everyday
    /// world's gx10-0000; as found, cell3's, which is the one with a model: use `ollama_view`).
    pub fn ollama_models(&self) -> Vec<OllamaModel> {
        if self.world == crate::demo::world::World::AsFound {
            return found_ollama("cell3");
        }
        let expires = now() + 240;
        let mut out: Vec<OllamaModel> = self
            .ollama
            .iter()
            .map(|m| {
                let loaded = super::sim::OLLAMA_MODELS.iter().find(|(n, _, _)| *n == m.name).map(|(n, vram, _)| OllamaLoaded {
                    name: n.to_string(),
                    size_bytes: *vram,
                    vram_bytes: *vram,
                    expires_at: Some(expires),
                    context_length: Some(8192),
                });
                OllamaModel { loaded, ..m.clone() }
            })
            .collect();
        out.sort_by(|a, b| b.loaded.is_some().cmp(&a.loaded.is_some()).then(a.name.cmp(&b.name)));
        out
    }

    pub fn ollama_add(&mut self, name: &str, size: u64, at: u64) {
        let family = name.split(':').next().unwrap_or(name).trim_end_matches(char::is_numeric).trim_end_matches('.');
        self.ollama.retain(|m| m.name != name);
        self.ollama.push(ollama_model(name, size, family, "", "Q4_K_M", at));
    }

    pub fn ollama_delete(&mut self, name: &str) -> Result<(), String> {
        if super::sim::OLLAMA_MODELS.iter().any(|(n, _, _)| *n == name) {
            return Err(format!("{name} is in memory right now; Ollama unloads it after 5 idle minutes. Delete it then."));
        }
        let before = self.ollama.len();
        self.ollama.retain(|m| m.name != name);
        if self.ollama.len() == before {
            return Err(format!("{name} isn't installed."));
        }
        Ok(())
    }

    /// Running copies as (source, target, bytes per second), for the simulated fabric.
    pub fn fabric_flows(&mut self) -> Vec<(String, String, f64)> {
        self.tick();
        self.jobs
            .iter()
            .filter(|j| j.job.state == "running" && j.job.kind == "pull")
            .filter_map(|j| Some((j.source.clone()?, j.job.node.clone(), j.job.rate_bps)))
            .collect()
    }

    /// Adds a vLLM log line to a load's tail (the metrics lines match the Endpoints charts).
    pub fn append_log(&mut self, id: &str, line: &str, keep: usize) {
        if let Some((d, _)) = self.deployments.iter_mut().find(|(d, _)| d.spec.id == id) {
            let mut lines: Vec<&str> = d.log_tail.lines().collect();
            lines.push(line);
            let start = lines.len().saturating_sub(keep);
            d.log_tail = lines[start..].join("\n");
        }
    }

    pub fn library(&mut self) -> Vec<LibraryEntry> {
        self.tick();
        self.library.clone()
    }

    pub fn jobs(&mut self) -> Vec<Job> {
        self.tick();
        let mut jobs: Vec<Job> = self.jobs.iter().map(|j| j.job.clone()).collect();
        jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        jobs
    }

    fn tick(&mut self) {
        let gated = self.playing("gated-401");
        let disk_full = self.playing("copy-disk-full");
        for j in &mut self.jobs {
            if j.job.state != "running" {
                continue;
            }
            let p = (j.started.elapsed().as_secs_f64() / j.seconds).min(1.0);
            // gated-401: Hugging Face refuses the first file; copy-disk-full: the target fills at 92%.
            let failure = match j.job.kind.as_str() {
                "download" if gated && j.started.elapsed().as_secs_f64() > 2.0 => {
                    Some("This model is gated: accept its license on huggingface.co and add a token")
                }
                "pull" if disk_full && p >= 0.92 => Some("No space left on device"),
                _ => None,
            };
            if let Some(why) = failure {
                j.job.bytes_done = if j.job.kind == "pull" { (j.job.bytes_total as f64 * 0.92) as u64 } else { 0 };
                j.job.state = "failed".into();
                j.job.step = "failed".into();
                j.job.message = why.into();
                j.job.rate_bps = 0.0;
                j.applied = true;
                continue;
            }
            j.job.bytes_done = (j.job.bytes_total as f64 * p) as u64;
            j.job.rate_bps = j.job.bytes_total as f64 / j.seconds;
            j.job.updated_at = now();
            if (0.9..1.0).contains(&p) {
                j.job.step = "verifying".into();
            }
            if p >= 1.0 {
                j.job.state = "done".into();
                j.job.step = "done".into();
            }
        }
        let finished: Vec<_> = self
            .jobs
            .iter_mut()
            .filter(|j| j.job.state == "done" && !j.applied)
            .filter_map(|j| {
                j.applied = true;
                j.adds_copy.clone().map(|(key, node)| (key, node, if j.job.kind == "pull" { "source-sha256" } else { "sha256" }))
            })
            .collect();
        for (key, node, check) in finished {
            if let Some(e) = self.library.iter_mut().find(|e| e.key == key) {
                e.copies.insert(node, Copy { path: key.clone(), size_bytes: e.size_bytes, verified: true, check: check.into(), ..Default::default() });
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_job(&mut self, node: &str, kind: &str, label: String, bytes: u64, seconds: f64, adds: Option<(String, String)>, source: Option<String>) -> Job {
        let job = Job {
            id: format!("demo-{kind}-{}", self.jobs.len() + 1),
            node: node.into(),
            kind: kind.into(),
            state: "running".into(),
            step: if kind == "download" { "downloading".into() } else { "copying".into() },
            message: String::new(),
            label,
            bytes_done: 0,
            bytes_total: bytes,
            rate_bps: 0.0,
            started_at: now(),
            updated_at: now(),
            ..Default::default()
        };
        self.jobs.push(DemoJob { job: job.clone(), started: Instant::now(), seconds, adds_copy: adds, applied: false, source });
        job
    }

    pub fn note_download_tool(&mut self, job: &mut Job, tool: &DownloadTool) {
        job.tool_used = tool.label.clone();
        job.tool_version = "Demo".into();
        job.message = if tool.kind == "install" { "Demo: approved tool installation and download are simulated.".into() } else { "Demo: download is simulated.".into() };
        if let Some(saved) = self.jobs.iter_mut().find(|j| j.job.id == job.id) { saved.job = job.clone(); }
    }

    pub fn image_job(&mut self, node: &str, image: &str, copy: bool, bytes: u64) -> Job {
        self.add_job(node, if copy { "image-load" } else { "pull-image" }, format!("Simulate preparing {image}"), bytes, 12.0, None, None)
    }

    /// Downloads the served files only (hf.rs), so gpt-oss-120b is 60.8 GiB, not its repo's 182.
    pub fn download(&mut self, node: &str, model: &HfModel, dest: &str) -> Job {
        let key = dest.replacen('~', &crate::demo::world::home(), 1);
        let gated = self.playing("gated-401");
        if !self.library.iter().any(|e| e.key == key) && !gated {
            self.library.push(LibraryEntry {
                key: key.clone(),
                in_use: None,
                repo: model.id.clone(),
                revision: "demo".into(),
                format: "safetensors".into(),
                size_bytes: model.download_bytes,
                root: root(),
                copies: Default::default(),
            });
        }
        let seconds = (model.download_bytes as f64 / DOWNLOAD_BYTES_PER_S).clamp(8.0, 120.0);
        let adds = (!gated).then(|| (key, node.to_string()));
        let mut job = self.add_job(node, "download", format!("Download {}", model.id), model.download_bytes, seconds, adds, None);
        job.resume = Some(DownloadResume { repo: model.id.clone(), revision: model.revision.clone(), dest: dest.into(), plan_hash: format!("demo-{}", job.id) });
        self.jobs.last_mut().unwrap().job = job.clone();
        job
    }

    pub fn resume_download(&mut self, node: &str, id: &str, expected: &DownloadResume) -> Result<Job, String> {
        self.tick();
        let index = self.jobs.iter().position(|j| j.job.id == id && j.job.node == node).ok_or("This Demo download was not found.")?;
        let old = &self.jobs[index];
        if old.job.kind != "download" || !matches!(old.job.state.as_str(), "failed" | "cancelled") || old.job.resume.as_ref() != Some(expected)
            || self.jobs[index + 1..].iter().any(|j| j.job.node == node && j.job.resume.as_ref().is_some_and(|r| r.dest == expected.dest)) {
            return Err("Refresh Jobs and select the latest stopped Demo download.".into());
        }
        let (bytes, seconds, adds) = (old.job.bytes_total, old.seconds, old.adds_copy.clone());
        let mut job = self.add_job(node, "download", format!("Resume {}", expected.repo), bytes, seconds, adds, None);
        job.resume = Some(DownloadResume { plan_hash: format!("demo-{}", job.id), ..expected.clone() });
        self.jobs.last_mut().unwrap().job = job.clone();
        Ok(job)
    }

    pub fn share(&mut self, key: &str, source: &str, targets: &[String]) -> Vec<Job> {
        let (repo, size) = self.library.iter().find(|e| e.key == key).map(|e| (e.repo.clone(), e.size_bytes)).unwrap_or_default();
        let seconds = (size as f64 / COPY_BYTES_PER_S).max(4.0);
        targets
            .iter()
            .map(|t| self.add_job(t, "pull", format!("Copy {repo} to {t}"), size, seconds, Some((key.into(), t.clone())), Some(source.into())))
            .collect()
    }

    pub fn delete(&mut self, key: &str, nodes: &[String]) {
        if let Some(e) = self.library.iter_mut().find(|e| e.key == key) {
            for n in nodes {
                e.copies.remove(n);
            }
        }
        self.library.retain(|e| !e.copies.is_empty() || e.key != key);
    }

    pub fn cancel(&mut self, id: &str) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.job.id == id && j.job.state == "running") {
            j.job.state = "cancelled".into();
            j.job.message = "Canceled".into();
            j.applied = true;
        }
    }

    /// Adds a load unless another active one holds its nodes or port (checked and inserted under one lock).
    pub fn claim(&mut self, d: Deployment) -> Result<(), String> {
        self.tick();
        if let Some(e) = self.library.iter().find(|e| e.key == d.spec.model_path) {
            if let Some(n) = d.spec.nodes.iter().find(|n| !e.copies.contains_key(*n)) {
                return Err(super::plan::not_on_node(n, &d.spec.model_path));
            }
        }
        let all: Vec<Deployment> = self.deployments.iter().map(|(d, _)| d.clone()).collect();
        super::claim(&all, &d.spec)?;
        self.deployments.retain(|(x, _)| x.spec.id != d.spec.id);
        self.deployments.push((d, Instant::now()));
        Ok(())
    }

    pub fn deployments(&mut self) -> Vec<Deployment> {
        let oom = self.playing("model-oom");
        let no_join = self.playing("ray-join-timeout");
        for (d, started) in &mut self.deployments {
            if d.state != DeploymentState::Starting {
                continue;
            }
            let t = started.elapsed().as_secs_f64();
            let need = d.spec.nodes.len() as u64;
            let weights = model_info(&d.spec.model_path)["bytes"].as_u64().filter(|b| *b > 0).unwrap_or(30_923_764_531) as f64;
            let prep = 3.0;
            let join = if need > 1 { 4.0 } else { 0.0 };
            let loaded = prep + join + load_seconds(weights, need).0;
            let (stage, msg) = if t < prep {
                (Stage::Starting, "Preparing the vLLM image on each node…".to_string())
            } else if need > 1 && (t < prep + join || no_join) {
                // ray-join-timeout: the last node never registers with the head.
                let have = (1 + ((t - prep) / join * need as f64) as u64).min(if no_join { need - 1 } else { need });
                if no_join && t >= prep + JOIN_TIMEOUT_S {
                    let node = d.spec.nodes.last().cloned().unwrap_or_default();
                    d.state = DeploymentState::Failed;
                    d.message = format!("Ray worker on {node} didn't join in {JOIN_TIMEOUT_S:.0} s");
                    d.log_tail = format!("sparkbox: {have} of {need} GPUs joined\n{}", d.message);
                    continue;
                }
                (Stage::Joining { have, need }, format!("{have} of {need} nodes up"))
            } else if t < loaded {
                let p = (((t - prep - join) / (loaded - prep - join)) * 100.0) as u64;
                // model-oom: the kernel kills it as the weights fill unified memory.
                if oom && p >= 80 {
                    d.state = DeploymentState::Failed;
                    d.message = "Out of memory while loading (exit 137)".into();
                    d.log_tail = "Loading safetensors checkpoint shards:  80% Completed\ntorch.OutOfMemoryError: CUDA out of memory.\nKilled".into();
                    continue;
                }
                (Stage::Loading { percent: p }, format!("Loading weights {p}%"))
            } else if t < loaded + WARM_S {
                (Stage::Warming, "Capturing CUDA graphs".to_string())
            } else {
                d.state = DeploymentState::Ready;
                d.provider_id = format!("vllm-{}", d.spec.id);
                (Stage::Ready, format!("Serving at {} (simulated)", d.endpoint))
            };
            if let Stage::Loading { percent } = &stage {
                d.log_tail = format!("Loading safetensors checkpoint shards: {percent:>3}% Completed");
            }
            d.stage = Some(stage);
            d.message = msg;
        }
        self.deployments.iter().map(|(d, _)| d.clone()).collect()
    }

    /// Starts a load again from the beginning (image, joining, weights, warm-up).
    pub fn pause_for_maintenance(&mut self, id: &str) {
        if let Some((d, _)) = self.deployments.iter_mut().find(|(d, _)| d.spec.id == id) {
            d.state = DeploymentState::Failed; d.recoverable = false; d.recovery = Some(super::recovery::Recovery::Maintenance);
            d.message = "Paused for simulated maintenance".into(); d.generation += 1;
        }
    }

    pub fn restart(&mut self, id: &str) -> Option<Deployment> {
        let (d, started) = self.deployments.iter_mut().find(|(d, _)| d.spec.id == id && d.state != DeploymentState::Stopped)?;
        d.state = DeploymentState::Starting;
        d.stage = Some(Stage::Starting);
        d.message = "Restarting…".into();
        d.recovery = None;
        d.log_tail.clear();
        d.generation += 1;
        d.started_at = now();
        *started = Instant::now();
        Some(d.clone())
    }

    pub fn unload(&mut self, id: &str) -> Option<Deployment> {
        let (d, _) = self.deployments.iter_mut().find(|(d, _)| d.spec.id == id)?;
        d.state = DeploymentState::Stopped;
        d.message = "Unloaded".into();
        Some(d.clone())
    }

    pub fn forget(&mut self, id: &str) {
        self.deployments.retain(|(d, _)| !(d.spec.id == id && !d.state.active()));
    }

    /// Tests: as if `secs` had passed for every job and load.
    #[cfg(test)]
    pub fn fast_forward(&mut self, secs: f64) {
        let by = std::time::Duration::from_secs_f64(secs);
        for j in &mut self.jobs {
            j.started = j.started.checked_sub(by).unwrap_or(j.started);
        }
        for (_, started) in &mut self.deployments {
            *started = started.checked_sub(by).unwrap_or(*started);
        }
    }
}

/// Prepare cluster's facts for the simulated nodes. Free disk comes from the simulated
/// root disk when there's a snapshot, so it agrees with the node's Disks tab.
pub fn checks(nodes: &[String], s: &ModelSettings, snapshot: Option<&crate::model::ClusterSnapshot>) -> Vec<NodeCheck> {
    let root_free = |id: &str| {
        let node = snapshot?.nodes.iter().find(|n| n.id == id)?;
        let root = node.disks.iter().find(|d| d.mount == "/")?;
        // As df reports it, like sparkjob.py's f_bavail on a real node (Demo's disks send no avail: total − used).
        Some(root.free())
    };
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| NodeCheck {
            node: n.clone(),
            error: None,
            home: crate::demo::world::home(),
            root_path: s.model_root.replacen('~', &crate::demo::world::home(), 1),
            root_exists: true,
            free_bytes: root_free(n).unwrap_or(if i == 3 { 140_000_000_000 } else { 2_400_000_000_000 }),
            docker: true,
            rsync: true,
            python: "3.12.3".into(),
            image_present: true,
            vllm_version: "0.11.1+nv26.05".into(),
            fabric: crate::demo::world::FABRIC
                .iter()
                .enumerate()
                .map(|(half, (iface, _, rdma))| FabricIf {
                    iface: iface.to_string(),
                    ip: crate::demo::world::fabric_ip(i, half),
                    prefix: crate::demo::world::FABRIC_PREFIX as u32,
                    speed_mbps: crate::demo::world::FABRIC_SPEED_MBPS as i64,
                    rdma: rdma.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
        .collect()
}

/// config.json and size of the demo library's models, for the memory plan and the fit math.
pub fn model_info(path: &str) -> Value {
    let repo = super::endpoints::repo_of(path).unwrap_or_default();
    match repo.as_str() {
        "Qwen/Qwen3.8-27B-FP8" => json!({ "bytes": 30_923_764_531u64, "config": {
            "architectures": ["Qwen3_8ForConditionalGeneration"], "model_type": "qwen3_8",
            "text_config": {
                "num_hidden_layers": 64, "hidden_size": 5120, "num_attention_heads": 24, "num_key_value_heads": 4,
                "head_dim": 256, "max_position_embeddings": 262144, "full_attention_interval": 4,
                "linear_num_value_heads": 48, "linear_num_key_heads": 16, "linear_key_head_dim": 128,
                "linear_value_head_dim": 128, "linear_conv_kernel_dim": 4
            },
            "vision_config": { "depth": 27, "hidden_size": 1152 },
            "quantization_config": { "quant_method": "fp8" }
        }}),
        "meta-llama/Llama-3.3-70B-Instruct" => json!({ "bytes": 141_107_412_992u64, "config": {
            "architectures": ["LlamaForCausalLM"], "model_type": "llama",
            "num_hidden_layers": 80, "hidden_size": 8192, "num_attention_heads": 64, "num_key_value_heads": 8,
            "max_position_embeddings": 131072
        }}),
        _ => json!({ "bytes": 0, "config": {} }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClusterSnapshot, DiskStats, NodeSnapshot};

    #[test]
    fn prepare_reports_the_simulated_root_disk() {
        let disk = |mount: &str, total: u64, used: u64| DiskStats { mount: mount.into(), total, used, ..Default::default() };
        let snap = ClusterSnapshot {
            nodes: vec![NodeSnapshot { id: "cell1".into(), disks: vec![disk("/boot/efi", 512, 6), disk("/", 4_000, 1_200)], ..Default::default() }],
            ..Default::default()
        };
        let nodes = vec!["cell1".to_string(), "cell2".to_string()];
        let checks = checks(&nodes, &ModelSettings::default(), Some(&snap));
        assert_eq!(checks[0].free_bytes, 2_800);
        // A node the snapshot doesn't know keeps the stand-in figure.
        assert_eq!(checks[1].free_bytes, 2_400_000_000_000);
    }

    #[test]
    fn the_as_found_library_and_ollama_are_what_the_nodes_hold() {
        let mut m = DemoModels::new();
        m.set_scenario(Some("as-found".into()));
        let lib = m.library();
        let row = |repo: &str| lib.iter().find(|e| e.repo == repo).unwrap_or_else(|| panic!("{repo}"));
        let glm = row("GLM-5.3-Flash-NVFP4");
        assert_eq!((glm.key.as_str(), glm.copies.len()), ("/home/sparkuser/models/GLM-5.3-Flash-NVFP4", 4));
        assert!((glm.size_bytes as f64 / 1024f64.powi(3) - 181.0).abs() < 0.5, "181 GiB");
        let w4 = row("AIAgens/GLM-5.3-Flash-UNCENSORED-W4A16");
        assert_eq!(w4.root, "/home/sparkuser/.cache/huggingface/hub");
        assert!(w4.key.contains("/models--AIAgens--GLM-5.3-Flash-UNCENSORED-W4A16/snapshots/"));
        assert_eq!(row("google/gemma-3-27b-it").copies.keys().cloned().collect::<Vec<_>>(), ["cell1", "cell2", "cell3"]);
        assert_eq!(row("Qwen/Qwen3-1.7B").copies.keys().cloned().collect::<Vec<_>>(), ["cell1"]);
        assert_eq!(lib.len(), 4);
        // Native Ollama: 0.21.0, gemma4 on cell3's disk and nothing in memory; none on cell1.
        let (cell3, version) = m.ollama_view("cell3");
        assert_eq!(version, "0.21.0");
        assert_eq!(cell3.iter().map(|o| (o.name.as_str(), o.parameter_size.as_deref(), o.quantization.as_deref(), o.loaded.is_none())).collect::<Vec<_>>(), [("gemma4:latest", Some("8.0B"), Some("Q4_K_M"), true)]);
        assert!((cell3[0].size_bytes as f64 / 1024f64.powi(3) - 8.9).abs() < 0.05);
        assert!(m.ollama_view("cell1").0.is_empty());
        // Back to everyday serving: gx10-0000's Ollama and the served Llama again.
        m.set_scenario(None);
        assert!(m.library().iter().any(|e| e.repo == "meta-llama/Llama-3.3-70B-Instruct"));
        assert_eq!(m.ollama_view("gx10-0000").1, "0.12.6");
    }
}
