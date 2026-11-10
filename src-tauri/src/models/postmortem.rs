//! Deployments found on the cluster, whoever started them, and why a failed one failed.
//!
//! Containers on different nodes are one deployment when they share SparkBox's
//! `sparkbox.deployment` label or a Compose project, when they run the same image with the
//! same command apart from a trailing rank (`/bin/bash /glm53/inner.sh 0` … `3`), when their
//! environments name the same head (NNODES plus HEAD_IP or MASTER_ADDR plus MASTER_PORT), or
//! when Ray workers point at a head. All but the label also need them to have started within
//! 120 s of each other: the label is the deployment, however long its ranks took to start.
//! A grouping SparkBox works out (anything but its own label) is `inferred`, and says how.
//! A load's Command is only `python3 /run/sparkbox/launch.py`, so its labels say what it serves.
//!
//! A failed deployment (none running, an exit other than 0) gets a postmortem, read once per
//! container and exit and only when it's first shown: `docker inspect` of its head, `docker
//! logs --tail 600` of the head, and, when the likely cause names a launch option and the head
//! runs a script from a host folder, that script read through sparkjob.py. All three only read.
//! The log's wrapped lines are joined, the decisive lines picked (the first root-cause exception
//! before "See root cause above", with two lines of context, never traceback frames), and
//! matched against the signatures in fixtures/postmortem-cases.json.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{now_ms, Ctx, Models};
use crate::live::sh_quote;
use crate::model::{ClusterSnapshot, Container, NodeSnapshot};

/// Containers whose starts are further apart than this aren't one deployment.
pub const START_WINDOW_S: u64 = 120;
/// How much of the head's log a postmortem reads.
pub const LOG_TAIL: usize = 600;
/// The GPU every node has (DGX Spark): what the signatures say, and the compute capability.
const GPU: &str = "GB10";
const COMPUTE: &str = "12.1";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentMember {
    pub node: String,
    /// Container name.
    pub container: String,
    pub id: String,
    pub rank: Option<u32>,
    /// "head" or "worker".
    pub role: String,
    pub state: String,
    pub exit_code: i32,
    pub oom: bool,
    pub started_ms: u64,
    pub finished_ms: u64,
    /// Last fresh observation, never advanced by a cached/offline frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

impl DeploymentMember {
    fn running(&self) -> bool {
        matches!(self.state.as_str(), "running" | "restarting" | "paused")
    }
}

/// Why a failed deployment failed, as far as its log tells.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Postmortem {
    /// "No attention kernel for a bf16 KV cache on GB10" (hedged, at most 60 characters), when a
    /// signature matched.
    pub title: Option<String>,
    /// The signature's id ("sparse-mla-kv-dtype").
    pub cause: Option<String>,
    pub explanation: Option<String>,
    pub fix: Option<String>,
    /// The decisive lines in log order: up to two lines of context, then the root-cause exception
    /// last. Without the log's prefixes.
    pub lines: Vec<String>,
    /// The decisive line as the log has it, for Show log to find.
    pub log_line: Option<String>,
    /// "~/glm53/inner.sh line 22: --kv-cache-dtype bfloat16".
    pub script_line: Option<String>,
    #[serde(default)]
    pub script_path: Option<String>,
    #[serde(default)]
    pub script_line_number: Option<u64>,
    /// Unix ms the log was read.
    pub read_ms: u64,
    /// The member whose log was read (the head).
    pub log_node: String,
    pub log_container: String,
    /// What the log says it ran with.
    pub vllm: Option<String>,
    pub tp: Option<u64>,
    pub kv_dtype: Option<String>,
    /// Why there's nothing to show ("Couldn't read the log: …").
    pub error: Option<String>,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterDeployment {
    /// The shared container name ("glm53"), the label or project, else the image's short name.
    pub id: String,
    /// What it runs: the model folder's last part ("GLM-5.3-Flash-NVFP4"), else the served name, else the id.
    pub label: String,
    /// The name clients use (SERVED_NAME, --served-model-name), when known.
    pub served: Option<String>,
    /// vllm | sglang | other.
    pub engine: String,
    pub image: String,
    /// The model path, when known ("/models/GLM-5.3-Flash-NVFP4").
    pub model: Option<String>,
    /// Its nodes, in rank order.
    pub nodes: Vec<String>,
    pub members: Vec<DeploymentMember>,
    /// running | failed | stopped | partial | unknown
    pub state: String,
    /// SparkBox worked the grouping out (anything but its own label).
    pub inferred: bool,
    /// The evidence for the grouping: "same image and command, ranks 0–3".
    pub how: String,
    /// The earliest start and the latest exit (Unix ms), and how long it ran.
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub ran_s: Option<u64>,
    /// The exit that failed it (the first non-zero one, head first).
    pub exit_code: Option<i32>,
    pub postmortem: Option<Postmortem>,
    pub demo: bool,
    /// Labels are an ownership boundary, never joined by a weaker inference key.
    #[serde(skip)]
    identity: Option<(String, String)>,
}

// ---- Grouping

fn words(cmd: &str) -> Vec<&str> {
    cmd.split_whitespace().collect()
}

/// A command's trailing rank (`inner.sh 2`), and the command without it.
fn trailing_rank(cmd: &str) -> Option<(String, u32)> {
    let w = words(cmd);
    let (last, rest) = w.split_last()?;
    let rank: u32 = last.parse().ok().filter(|r| *r < 64)?;
    (!rest.is_empty()).then(|| (rest.join(" "), rank))
}

fn flag<'a>(w: &[&'a str], name: &str) -> Option<&'a str> {
    w.iter().enumerate().find_map(|(i, x)| if *x == name { w.get(i + 1).copied() } else { x.strip_prefix(name).and_then(|r| r.strip_prefix('=')) })
}

/// A member's rank: the trailing argument, --node-rank, SparkBox's label, or NODE_RANK/RANK.
fn rank_of(c: &Container) -> Option<u32> {
    let w = words(&c.command);
    trailing_rank(&c.command)
        .map(|(_, r)| r)
        .or_else(|| flag(&w, "--node-rank").and_then(|r| r.parse().ok()))
        .or_else(|| c.labels.get("sparkbox.rank").and_then(|r| r.parse().ok()))
        .or_else(|| ["NODE_RANK", "RANK"].iter().find_map(|k| c.env_flags.get(*k).and_then(|r| r.parse().ok())))
}

/// Every address a node answers on.
fn node_ips(n: &NodeSnapshot) -> Vec<String> {
    let mut ips: Vec<String> = n.nets.iter().filter_map(|x| x.ip.clone()).collect();
    ips.push(n.host.clone());
    ips
}

fn host_of(addr: &str) -> String {
    addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr).to_string()
}

/// The head address a container's environment names (HEAD_IP, else MASTER_ADDR).
fn head_ip(c: &Container) -> Option<&str> {
    c.env_flags.get("HEAD_IP").or_else(|| c.env_flags.get("MASTER_ADDR")).map(String::as_str)
}

fn short_image(image: &str) -> String {
    let repo = image.split(['@', ':']).next().unwrap_or(image);
    repo.rsplit('/').next().unwrap_or(repo).to_string()
}

fn engine_of(c: &Container) -> &'static str {
    // Started by launch.py and by image id: neither its command nor its image says.
    if c.labels.contains_key("sparkbox.path") {
        return "vllm";
    }
    let s = format!("{} {}", c.image, c.command).to_ascii_lowercase();
    if s.contains("sglang") {
        "sglang"
    } else if s.contains("vllm") {
        "vllm"
    } else {
        "other"
    }
}

/// A short folder name from a model path: "/models/GLM-5.3-Flash-NVFP4/" → "GLM-5.3-Flash-NVFP4".
pub fn folder_name(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

struct Found<'a> {
    node: &'a NodeSnapshot,
    c: &'a Container,
}

/// Union-find over the containers, by every key they share.
fn groups(found: &[Found], keys: &[Vec<(char, String)>]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..found.len()).collect();
    fn root(p: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        p[i] = r;
        r
    }
    let mut first: HashMap<(char, String), usize> = HashMap::new();
    for (i, ks) in keys.iter().enumerate() {
        for k in ks {
            match first.get(k) {
                Some(&j) => {
                    let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                    parent[a] = b;
                }
                None => {
                    first.insert(k.clone(), i);
                }
            }
        }
    }
    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..found.len() {
        let r = root(&mut parent, i);
        by_root.entry(r).or_default().push(i);
    }
    by_root.into_values().collect()
}

/// Splits a group into runs whose starts are within START_WINDOW_S of the run's first.
fn by_start(found: &[Found], mut idx: Vec<usize>) -> Vec<Vec<usize>> {
    idx.sort_by_key(|&i| found[i].c.started_at);
    let mut out: Vec<Vec<usize>> = Vec::new();
    for i in idx {
        match out.last_mut() {
            Some(run) if found[i].c.started_at.saturating_sub(found[run[0]].c.started_at) <= START_WINDOW_S => run.push(i),
            _ => out.push(vec![i]),
        }
    }
    out
}

/// Every deployment on the cluster: containers on two or more nodes grouped by the rules above
/// (SparkBox's own label groups even one), members in rank order, head first.
pub fn infer(snap: &ClusterSnapshot) -> Vec<ClusterDeployment> {
    let found: Vec<Found> = snap.nodes.iter().flat_map(|n| n.containers.iter().map(move |c| Found { node: n, c })).collect();
    // Ray workers join the head on the node at their --address.
    let ray_head = |addr: &str| snap.nodes.iter().find(|n| node_ips(n).iter().any(|ip| ip == addr)).map(|n| n.id.clone());
    let keys: Vec<Vec<(char, String)>> = found
        .iter()
        .map(|f| {
            let c = f.c;
            let mut k = Vec::new();
            if let Some(l) = c.labels.get("sparkbox.deployment") {
                return vec![('a', serde_json::to_string(&(c.labels.get("sparkbox.instance").map(String::as_str).unwrap_or(""), l)).unwrap())];
            }
            // Explicit rendezvous identity is stronger than a shared script or Compose name.
            if let (Some(n), Some(h), Some(port)) = (c.env_flags.get("NNODES"), head_ip(c), c.env_flags.get("MASTER_PORT")) {
                return vec![('c', format!("{n}|{h}|{port}"))];
            }
            if let Some(p) = &c.project {
                k.push(('p', p.clone()));
            }
            if let Some((cmd, _)) = trailing_rank(&c.command) {
                k.push(('b', format!("{}|{cmd}", c.image)));
            }
            let w = words(&c.command);
            if c.command.contains("ray start") {
                if c.command.contains("--head") {
                    k.push(('r', f.node.id.clone()));
                } else if let Some(head) = flag(&w, "--address").map(host_of).and_then(|a| ray_head(&a)) {
                    k.push(('r', head));
                }
            } else if c.command.contains("distributed-executor-backend ray") || c.command.contains("distributed-executor-backend=ray") {
                k.push(('r', f.node.id.clone()));
            }
            k
        })
        .collect();
    let mut out: Vec<ClusterDeployment> = Vec::new();
    for group in groups(&found, &keys) {
        let own = group.iter().all(|&i| found[i].c.labels.contains_key("sparkbox.deployment"));
        for run in if own { vec![group] } else { by_start(&found, group) } {
            let labelled = run.iter().all(|&i| found[i].c.labels.contains_key("sparkbox.deployment"));
            let nodes: HashSet<&str> = run.iter().map(|&i| found[i].node.id.as_str()).collect();
            if nodes.len() < 2 && !(labelled && !run.is_empty()) {
                continue;
            }
            // Repeated launch ranks with no ownership label cannot identify one run. Do not
            // merge two independently launched models just because their scripts match.
            let ranks: Vec<_> = run.iter().filter_map(|&i| rank_of(found[i].c)).collect();
            if !labelled && ranks.iter().collect::<HashSet<_>>().len() != ranks.len() { continue; }
            out.push(build(&found, &run, &keys, snap.demo));
        }
    }
    unique_ids(&mut out);
    out.sort_by(|a, b| b.started_ms.cmp(&a.started_ms));
    out
}

// Separate runs may share a name, even on the same head. Keep UI grouping keys distinct.
fn unique_ids(list: &mut [ClusterDeployment]) {
    let mut seen: HashSet<String> = HashSet::new();
    for d in list {
        if !seen.insert(d.id.clone()) {
            let base = format!("{}-{}", d.id, d.nodes.first().cloned().unwrap_or_default());
            d.id = base.clone();
            let mut suffix = 2;
            while !seen.insert(d.id.clone()) {
                d.id = format!("{base}-{suffix}");
                suffix += 1;
            }
        }
    }
}

fn build(found: &[Found], run: &[usize], keys: &[Vec<(char, String)>], demo: bool) -> ClusterDeployment {
    let cs: Vec<&Container> = run.iter().map(|&i| found[i].c).collect();
    let first = cs[0];
    // The head: rank 0, the node at HEAD_IP / MASTER_ADDR, Ray's --head, or SparkBox's role label.
    let head_addr = cs.iter().find_map(|c| head_ip(c)).map(String::from);
    let is_head = |f: &Found| {
        rank_of(f.c) == Some(0)
            || head_addr.as_ref().is_some_and(|a| node_ips(f.node).contains(a))
            || f.c.command.contains("--head")
            || f.c.labels.get("sparkbox.role").is_some_and(|r| r == "head")
            || (!f.c.command.contains("ray start") && f.c.command.contains("distributed-executor-backend"))
    };
    let mut members: Vec<DeploymentMember> = run
        .iter()
        .map(|&i| {
            let f = &found[i];
            DeploymentMember {
                node: f.node.id.clone(),
                container: f.c.name.clone(),
                id: f.c.id.clone(),
                rank: rank_of(f.c),
                role: if is_head(f) { "head".into() } else { "worker".into() },
                state: f.c.state.clone(),
                exit_code: f.c.exit_code,
                oom: f.c.oom_killed,
                started_ms: f.c.started_at * 1000,
                finished_ms: f.c.finished_at.saturating_mul(1000),
                ..Default::default()
            }
        })
        .collect();
    // One head at most: the lowest rank of those that claim it.
    let heads: Vec<usize> = (0..members.len()).filter(|&i| members[i].role == "head").collect();
    if heads.len() > 1 {
        let keep = *heads.iter().min_by_key(|&&i| members[i].rank.unwrap_or(u32::MAX)).unwrap();
        for i in heads {
            if i != keep {
                members[i].role = "worker".into();
            }
        }
    }
    members.sort_by_key(|m| (m.role != "head", m.rank.unwrap_or(u32::MAX), m.node.clone()));
    let mut nodes: Vec<String> = Vec::new();
    for m in &members {
        if !nodes.contains(&m.node) {
            nodes.push(m.node.clone());
        }
    }
    let running = members.iter().filter(|m| m.running()).count();
    let bad = members.iter().find(|m| !m.running() && m.exit_code != 0).map(|m| m.exit_code);
    let state = match (running, members.len()) {
        (r, n) if r == n => "running",
        (0, _) if bad.is_some() => "failed",
        (0, _) => "stopped",
        _ => "partial",
    };
    let same_name = cs.iter().all(|c| c.name == first.name);
    let label_key = first.labels.get("sparkbox.deployment").cloned().or_else(|| first.project.clone());
    let id = if same_name { first.name.clone() } else { label_key.clone().unwrap_or_else(|| short_image(&first.image)) };
    let all_keys: HashSet<char> = run.iter().flat_map(|&i| keys[i].iter().map(|(k, _)| *k)).collect();
    let ranks: Vec<u32> = members.iter().filter_map(|m| m.rank).collect();
    let span = match (ranks.iter().min(), ranks.iter().max()) {
        (Some(a), Some(b)) if a != b => format!(", ranks {a}–{b}"),
        _ => String::new(),
    };
    let how = if all_keys.contains(&'a') {
        "SparkBox's deployment label".to_string()
    } else if all_keys.contains(&'p') {
        format!("Compose project {}", first.project.clone().unwrap_or_default())
    } else if all_keys.contains(&'b') {
        format!("same image and command{span}, started together")
    } else if all_keys.contains(&'c') {
        "same head address and port in their environment, started together".to_string()
    } else {
        "Ray workers attached to their head".to_string()
    };
    // What it serves: SparkBox's labels, which every rank carries (a load that stopped before its
    // head started has none), else the head's command line (a script hides it until the postmortem
    // reads it).
    let head = members.first().and_then(|m| cs.iter().find(|c| c.id == m.id)).copied().unwrap_or(first);
    let label = |k: &str| cs.iter().find_map(|c| c.labels.get(k)).cloned();
    let w = words(&head.command);
    let served = label("sparkbox.model").or_else(|| flag(&w, "--served-model-name").map(String::from));
    let model = label("sparkbox.path").or_else(|| w.windows(2).find(|p| p[0].ends_with("vllm") && p[1] == "serve").and_then(|_| w.iter().skip_while(|x| **x != "serve").nth(1)).filter(|m| !m.starts_with('-')).map(|s| s.to_string())).or_else(|| flag(&w, "--model").or_else(|| flag(&w, "--model-path")).map(String::from));
    let started = members.iter().map(|m| m.started_ms).min().unwrap_or(0);
    let finished = (running == 0).then(|| members.iter().map(|m| m.finished_ms).max().unwrap_or(0)).filter(|f| *f > 0);
    ClusterDeployment {
        label: model.as_deref().map(folder_name).or_else(|| served.clone()).unwrap_or_else(|| id.clone()),
        id,
        served,
        engine: engine_of(head).into(),
        // A load runs its image by id: the name it was picked by is on its label.
        image: label("sparkbox.image").unwrap_or_else(|| head.image.clone()),
        model,
        nodes,
        state: state.into(),
        inferred: !all_keys.contains(&'a'),
        how,
        started_ms: started,
        finished_ms: finished,
        // How long the head ran (each member's own run is in its times).
        ran_s: finished.and_then(|_| members.first()).filter(|h| h.finished_ms >= h.started_ms && h.started_ms > 0).map(|h| (h.finished_ms - h.started_ms) / 1000),
        exit_code: bad,
        members,
        postmortem: None,
        demo,
        identity: first.labels.get("sparkbox.deployment").map(|d| (first.labels.get("sparkbox.instance").cloned().unwrap_or_default(), d.clone())),
    }
}

/// Merge current observations with expected membership. An unavailable node cannot prove a
/// rank stopped or disappeared. A fresh report can: missing containers then read "missing".
/// External groups are restored from saved observations; owned loads use their persisted spec too.
fn retain_members(snap: &ClusterSnapshot, previous: &[ClusterDeployment], owned: &[super::Deployment]) -> Vec<ClusterDeployment> {
    let reporting = |id: &str| snap.nodes.iter().find(|n| n.id == id).is_some_and(super::endpoints::reporting);
    let same = |a: &ClusterDeployment, b: &ClusterDeployment| {
        (a.identity.is_some() && a.identity == b.identity)
            || a.members.iter().any(|m| !m.id.is_empty() && b.members.iter().any(|n| m.node == n.node && m.id == n.id))
    };
    let mut list = infer(snap);
    for old in previous {
        if let Some(d) = list.iter_mut().find(|d| same(d, old)) {
            d.id = old.id.clone();
            for m in &old.members {
                if !d.members.iter().any(|n| n.node == m.node && n.rank == m.rank) {
                    d.members.push(m.clone());
                }
            }
            // The saved order keeps a missing middle rank in its original place.
            let mut nodes = old.nodes.clone();
            nodes.extend(d.nodes.iter().filter(|n| !old.nodes.contains(n)).cloned());
            d.nodes = nodes;
        } else if old.nodes.iter().any(|id| !reporting(id)) || old.members.iter().any(|m| {
            snap.nodes.iter().find(|n| n.id == m.node).is_some_and(|n| n.containers.iter().any(|c| c.id == m.id))
        }) {
            // infer() normally requires at least two observed nodes for an external group.
            list.push(old.clone());
        }
    }
    for load in owned.iter().filter(|d| d.state != super::DeploymentState::Stopped) {
        let spec = &load.spec;
        let identity = Some((crate::identity::instance_name().to_string(), spec.id.clone()));
        let at = list.iter().position(|d| d.identity == identity).unwrap_or_else(|| {
            list.push(ClusterDeployment {
                id: spec.id.clone(), label: spec.served_name.clone(), served: Some(spec.served_name.clone()),
                engine: spec.engine.clone(), image: spec.image_ref.clone(), model: Some(spec.model_path.clone()),
                how: "SparkBox's saved load".into(), started_ms: load.started_at.saturating_mul(1000),
                demo: load.demo, identity, ..Default::default()
            });
            list.len() - 1
        });
        let d = &mut list[at];
        d.nodes = spec.nodes.clone();
        d.members.retain(|m| spec.nodes.contains(&m.node));
        for (rank, node) in spec.nodes.iter().enumerate() {
            if !d.members.iter().any(|m| &m.node == node) {
                d.members.push(DeploymentMember {
                    node: node.clone(), rank: Some(rank as u32), role: if rank == 0 { "head" } else { "worker" }.into(),
                    container: super::plan::rank_container(&spec.id, rank, spec.nodes.len()), ..Default::default()
                });
            }
        }
    }
    for d in &mut list {
        for m in &mut d.members {
            let old = previous.iter().flat_map(|o| &o.members).find(|x| x.node == m.node && x.id == m.id);
            // Last observation may already be in this member if the whole group was recalled.
            m.last_seen_ms = old.and_then(|o| o.last_seen_ms).or(m.last_seen_ms);
            let node = snap.nodes.iter().find(|n| n.id == m.node);
            if node.is_none_or(|n| !super::endpoints::reporting(n)) {
                m.state = "unknown".into();
                m.unavailable_reason = Some(if node.is_some_and(|n| n.status == crate::model::LinkStatus::Online) { "No current report" } else { "Node unavailable" }.into());
            } else if let Some(c) = node.and_then(|n| n.containers.iter().find(|c| c.id == m.id)) {
                m.state = c.state.clone(); m.exit_code = c.exit_code; m.oom = c.oom_killed;
                m.started_ms = c.started_at.saturating_mul(1000); m.finished_ms = c.finished_at.saturating_mul(1000);
                m.last_seen_ms = Some(snap.ts_ms); m.unavailable_reason = None;
            } else {
                m.state = "missing".into(); m.unavailable_reason = None;
                m.exit_code = 0; m.oom = false; m.finished_ms = 0;
            }
        }
        d.members.sort_by_key(|m| (m.role != "head", m.rank.unwrap_or(u32::MAX), m.node.clone()));
        let running = d.members.iter().filter(|m| m.running()).count();
        let unknown = d.members.iter().any(|m| m.state == "unknown");
        let missing = d.members.iter().any(|m| m.state == "missing");
        d.exit_code = d.members.iter().find(|m| !m.running() && m.state != "unknown" && m.exit_code != 0).map(|m| m.exit_code);
        d.state = if unknown { "unknown" } else if missing { "partial" } else if running == d.members.len() { "running" }
            else if running > 0 { "partial" } else if d.exit_code.is_some() { "failed" } else { "stopped" }.into();
        if unknown || missing || running > 0 {
            d.finished_ms = None; d.ran_s = None; d.postmortem = None;
        } else {
            d.finished_ms = d.members.iter().map(|m| m.finished_ms).max().filter(|ms| *ms > 0);
            d.ran_s = d.members.first().filter(|h| h.started_ms > 0 && h.finished_ms >= h.started_ms).map(|h| (h.finished_ms - h.started_ms) / 1000);
        }
    }
    unique_ids(&mut list);
    list.sort_by_key(|d| std::cmp::Reverse(d.started_ms));
    list
}

// ---- Reading the log

/// Joins lines a terminal wrapped at a fixed width (a log copied from a terminal): when many lines
/// are exactly the longest width, a line of that width continues on the next. A log straight
/// from `docker logs` has none, and comes back as it is.
pub fn join_wrapped(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut widths: HashMap<usize, usize> = HashMap::new();
    for l in &lines {
        *widths.entry(l.chars().count()).or_default() += 1;
    }
    let max = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let wrapped = max >= 60 && widths.get(&max).copied().unwrap_or(0) * 10 >= lines.len().max(1);
    let mut out: Vec<String> = Vec::new();
    let mut last_width = 0;
    for l in lines {
        if wrapped && last_width == max && !out.is_empty() {
            out.last_mut().unwrap().push_str(l);
        } else {
            out.push(l.to_string());
        }
        last_width = l.chars().count();
    }
    out
}

/// A log line without vLLM's prefixes: "(Worker_TP0 pid=251) ERROR 10-26 22:21:14 [file.py:944] " and "[rank0]:".
pub fn strip_prefix(line: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^(\([A-Za-z0-9_]+ pid=\d+\)\s*)?((INFO|WARNING|ERROR|DEBUG|CRITICAL)\s+\d{2}-\d{2} \d{2}:\d{2}:\d{2}(\.\d+)?\s+\[[^\]]+\]\s?)?").unwrap()
    });
    re.replace(line, "").trim_end().to_string()
}

fn is_frame(msg: &str) -> bool {
    let t = msg.trim_start();
    t.starts_with("File \"") || t.starts_with("Traceback (most recent call last)") || (!t.is_empty() && t.chars().all(|c| c == '^' || c == '~' || c == ' '))
}

/// The root-cause exception: (line index, the message) for the first exception line before the
/// first "See root cause" / "See stack trace for root cause" line (else in the whole log).
pub fn decisive(lines: &[String]) -> Option<(usize, String)> {
    static EXC: OnceLock<Regex> = OnceLock::new();
    let exc = EXC.get_or_init(|| {
        Regex::new(r"^([\w.]+\.)?(ValueError|RuntimeError|AssertionError|OutOfMemoryError|OSError|FileNotFoundError|TimeoutError|ImportError|KeyError|TypeError|NotImplementedError|DistBackendError|DistNetworkError|GatedRepoError|LocalEntryNotFoundError|HFValidationError)\b.*:|CUDA error|NCCL error|^([\w.]+\.)?\w*Error: ").unwrap()
    });
    let generic = |m: &str| m.contains("See root cause above") || m.contains("See stack trace for root cause") || m.contains("Engine core initialization failed");
    let end = lines.iter().position(|l| generic(&strip_prefix(l))).unwrap_or(lines.len());
    let pick = |range: &[String], offset: usize| {
        range.iter().enumerate().find_map(|(i, l)| {
            let m = strip_prefix(l);
            (!is_frame(&m) && !generic(&m) && exc.is_match(m.trim_start())).then(|| (offset + i, m.trim_start().to_string()))
        })
    };
    pick(&lines[..end], 0).or_else(|| pick(lines, 0))
}

/// Up to two lines before the decisive one, skipping traceback frames, carets and blank lines.
fn context(lines: &[String], at: usize) -> Vec<String> {
    let mut ctx: Vec<String> = lines[..at].iter().rev().map(|l| strip_prefix(l).trim().to_string()).filter(|m| !m.is_empty() && !is_frame(m)).take(2).collect();
    ctx.reverse();
    ctx
}

/// The first ERROR line, for a log with no exception (engine-init's quote).
fn first_error(lines: &[String]) -> Option<(usize, String)> {
    lines.iter().enumerate().find_map(|(i, l)| {
        let m = strip_prefix(l);
        (l.contains(" ERROR ") && !m.trim().is_empty() && !is_frame(&m)).then(|| (i, m.trim().to_string()))
    })
}

/// What the log says the engine ran with: its version ("0.28.1rc1"), tensor parallel and KV dtype.
pub fn run_facts(lines: &[String]) -> (Option<String>, Option<u64>, Option<String>) {
    static VER: OnceLock<Regex> = OnceLock::new();
    static TP: OnceLock<Regex> = OnceLock::new();
    static KV: OnceLock<Regex> = OnceLock::new();
    let ver = VER.get_or_init(|| Regex::new(r"(?:version |engine \(v)(\d+\.\d+\.\d+[A-Za-z0-9]*)").unwrap());
    let tp = TP.get_or_init(|| Regex::new(r"tensor_parallel_size['=:\s]+(\d+)").unwrap());
    let kv = KV.get_or_init(|| Regex::new(r"kv_cache_dtype['=:\s]+'?([A-Za-z0-9_]+)").unwrap());
    let find = |re: &Regex| lines.iter().find_map(|l| re.captures(l).map(|c| c[1].to_string()));
    (find(ver), find(tp).and_then(|t| t.parse().ok()), find(kv))
}

/// The model path the log names ('model': '/models/X', or vLLM's banner "model   /models/X").
fn log_model(lines: &[String]) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"'model(?:_tag)?': '([^']+)'|\bmodel\s{2,}(/\S+)").unwrap());
    lines.iter().find_map(|l| re.captures(l).and_then(|c| c.get(1).or(c.get(2)).map(|m| m.as_str().to_string())))
}

// ---- Signatures

#[derive(Clone, Debug, Deserialize)]
pub struct Case {
    pub id: String,
    pub all: Vec<String>,
    pub title: String,
    pub explanation: String,
    pub fix: String,
    #[serde(default)]
    pub flag: String,
    /// A log excerpt the case must match, and its filled title (the tests read them).
    #[serde(default)]
    #[cfg_attr(not(test), allow(dead_code))]
    pub sample: String,
    #[serde(default)]
    #[cfg_attr(not(test), allow(dead_code))]
    pub expect: String,
}

#[derive(Deserialize)]
struct Cases {
    cases: Vec<Case>,
}

pub fn cases() -> &'static [(Case, Vec<Regex>)] {
    static CASES: OnceLock<Vec<(Case, Vec<Regex>)>> = OnceLock::new();
    CASES.get_or_init(|| {
        let parsed: Cases = serde_json::from_str(include_str!("fixtures/postmortem-cases.json")).expect("postmortem-cases.json");
        parsed
            .cases
            .into_iter()
            .map(|c| {
                let res = c.all.iter().map(|p| RegexBuilder::new(p).case_insensitive(true).build().expect("signature regex")).collect();
                (c, res)
            })
            .collect()
    })
}

/// The first case whose patterns all match `text`.
pub fn match_case(text: &str) -> Option<&'static Case> {
    cases().iter().find(|(_, res)| res.iter().all(|r| r.is_match(text))).map(|(c, _)| c)
}

/// "bfloat16" → "bf16", "float16" → "fp16".
fn short_dtype(d: &str) -> String {
    match d {
        "bfloat16" => "bf16".into(),
        "float16" | "half" => "fp16".into(),
        "float32" => "fp32".into(),
        other => other.into(),
    }
}

/// A model folder's family: "GLM-5.3-Flash-NVFP4" → "GLM-5.3-Flash".
fn family(name: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| RegexBuilder::new(r"[-_](NVFP4|FP4|FP8|MXFP4|AWQ|GPTQ|INT4|INT8|W4A16|W8A8|BF16|FP16|GGUF)([-_].*)?$").case_insensitive(true).build().unwrap());
    re.replace(name, "").to_string()
}

/// Fills a case's {placeholders}.
fn fill(text: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = text.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// The host path of the script a container runs (`/bin/bash /glm53/inner.sh 0` with
/// /home/sparkuser/glm53 mounted at /glm53), and how to show it ("~/glm53/inner.sh").
pub fn script_on_host(command: &str, mounts: &[(String, String)]) -> Option<(String, String)> {
    let w = words(command);
    let first = *w.first()?;
    let shell = ["bash", "sh", "/bin/bash", "/bin/sh", "/usr/bin/bash", "/usr/bin/env"].contains(&first);
    let script = if shell { w.iter().skip(1).find(|x| !x.starts_with('-') && **x != "bash")? } else { &first };
    if !script.ends_with(".sh") || !script.starts_with('/') {
        return None;
    }
    let (src, dst) = mounts.iter().filter(|(_, d)| script.starts_with(&format!("{}/", d.trim_end_matches('/')))).max_by_key(|(_, d)| d.len())?;
    let host = format!("{}{}", src.trim_end_matches('/'), &script[dst.trim_end_matches('/').len()..]);
    let shown = match host.split('/').collect::<Vec<_>>().as_slice() {
        ["", "home", _user, rest @ ..] => format!("~/{}", rest.join("/")),
        ["", "root", rest @ ..] => format!("~/{}", rest.join("/")),
        _ => host.clone(),
    };
    Some((host, shown))
}

/// "~/glm53/inner.sh line 22: --kv-cache-dtype bfloat16": the line of the script that sets `flag`.
pub fn script_line(shown: &str, text: &str, flag: &str) -> Option<String> {
    if flag.is_empty() {
        return None;
    }
    text.lines().enumerate().find(|(_, l)| {
        let t = l.trim_start();
        !t.starts_with('#') && (t.contains(&format!("{flag} ")) || t.contains(&format!("{flag}=")) || t.ends_with(flag))
    }).map(|(i, l)| format!("{shown} line {}: {}", i + 1, l.trim().trim_end_matches('\\').trim_end()))
}

/// Mounts from `docker inspect`: (source on the node, destination in the container).
pub fn inspect_mounts(v: &Value) -> Vec<(String, String)> {
    v["Mounts"].as_array().into_iter().flatten().filter_map(|m| Some((m["Source"].as_str()?.to_string(), m["Destination"].as_str()?.to_string()))).collect()
}

/// An environment variable from `docker inspect`.
pub fn inspect_env(v: &Value, name: &str) -> Option<String> {
    v["Config"]["Env"].as_array()?.iter().filter_map(|e| e.as_str()).find_map(|e| e.strip_prefix(&format!("{name}=")).map(String::from))
}

/// The postmortem of one log: decisive lines, run facts, the likely cause. `script` is the head's
/// script (where it's shown, its text) when one could be read.
pub fn read_log(d: &ClusterDeployment, log: &str, script: Option<(&str, &str)>) -> Postmortem {
    let lines = join_wrapped(log);
    let (vllm, tp, kv) = run_facts(&lines);
    let vllm = vllm.map(|v| v.split(".dev").next().unwrap_or(&v).split('+').next().unwrap_or(&v).to_string());
    let found = decisive(&lines).or_else(|| first_error(&lines));
    let (lines_out, log_line, decisive_text) = match &found {
        Some((at, msg)) => {
            let mut v = context(&lines, *at);
            v.push(msg.clone());
            (v, Some(lines[*at].clone()), msg.clone())
        }
        None => (Vec::new(), None, String::new()),
    };
    // The decisive lines say it best; the whole log is the fallback.
    let case = match_case(&decisive_text).or_else(|| match_case(&lines.join("\n")));
    let model = d.model.clone().or_else(|| log_model(&lines));
    let kv_long = static_kv(&decisive_text).or_else(|| kv.clone()).unwrap_or_else(|| "bfloat16".into());
    let mut vars: BTreeMap<&str, String> = BTreeMap::new();
    vars.insert("gpu", GPU.into());
    vars.insert("cc", COMPUTE.into());
    vars.insert("kv", short_dtype(&kv_long));
    vars.insert("kv_long", kv_long.clone());
    vars.insert("vllm_build", match &vllm {
        Some(v) => format!("this vLLM build ({v})"),
        None => "this vLLM build".into(),
    });
    vars.insert("model", model.as_deref().map(|m| family(&folder_name(m))).unwrap_or_else(|| "This model".into()));
    let script_line = case.and_then(|c| script.and_then(|(shown, text)| script_line(shown, text, &c.flag)));
    // The decisive log lines and the script line are shown as-is, so a secret echoed there
    // (`vllm serve … --api-key KEY`) is masked first, like the activity log (P1.46).
    use crate::live::audit::redact;
    Postmortem {
        title: case.map(|c| fill(&c.title, &vars)),
        cause: case.map(|c| c.id.clone()),
        explanation: case.map(|c| fill(&c.explanation, &vars)),
        fix: case.map(|c| fill(&c.fix, &vars)),
        lines: lines_out.iter().map(|l| redact(l)).collect(),
        log_line: log_line.as_deref().map(redact),
        script_line: script_line.as_deref().map(redact),
        read_ms: now_ms(),
        vllm,
        tp: tp.or_else(|| (d.members.len() > 1).then_some(d.members.len() as u64)),
        kv_dtype: kv.map(|k| short_dtype(&k)),
        error: found.is_none().then(|| "Its log names no error.".to_string()),
        ..Default::default()
    }
}

/// The KV dtype a decisive line names (kv_cache_dtype=bfloat16).
fn static_kv(line: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"kv_cache_dtype=([A-Za-z0-9_]+)").unwrap()).captures(line).map(|c| c[1].to_string())
}

// ---- Demo: the as-found world's glm53, from the real log

/// What a Demo container's log and inspect say, for containers the simulator has none for: the
/// owner's real GLM-5.3 head (Oct 26).
fn demo_log(c: &DeploymentMember) -> Option<&'static str> {
    (c.container == "glm53").then_some(include_str!("fixtures/logs/glm53-rank0.log"))
}

fn demo_script(host: &str) -> Option<&'static str> {
    host.ends_with("/glm53/inner.sh").then_some(include_str!("fixtures/logs/glm53-inner.sh"))
}

/// The real glm53 mounts (text-inspect-glm53-cell1), when the Demo's inspect has none.
fn demo_mounts(c: &DeploymentMember) -> Vec<(String, String)> {
    if c.container != "glm53" {
        return Vec::new();
    }
    [("/home/sparkuser/glm53", "/glm53"), ("/home/sparkuser/models", "/models")].iter().map(|(s, d)| (s.to_string(), d.to_string())).collect()
}

// ---- The cache, and reading postmortems from the nodes

/// Postmortems by (node, container id, finished at): one read per container and exit.
#[derive(Default)]
pub struct Cache {
    pub done: HashMap<(String, String, u64), Postmortem>,
    /// What SERVED_NAME and the model folder the head's environment named.
    pub named: HashMap<String, (Option<String>, Option<String>)>,
    pub reading: HashSet<(String, String, u64)>,
    /// How many logs have been read (tests: once per container).
    pub reads: u64,
    pub retries: HashMap<(String, String, u64), (usize, u64)>,
    /// The data source these are for: switching clears them.
    pub live: Option<bool>,
    /// The ids and states last announced, for deployments://changed.
    pub announced: String,
    /// Saved Live observations; Demo stays in memory. Owned loads also have their DeploySpec.
    pub observed: Vec<ClusterDeployment>,
}

// Keep the ownership key in the on-disk envelope, not in the UI's serialized view.
#[derive(Serialize, Deserialize)]
struct SavedObservation {
    deployment: ClusterDeployment,
    identity: Option<(String, String)>,
}

fn read_observed(path: &std::path::Path) -> Vec<ClusterDeployment> {
    let saved: Vec<SavedObservation> = super::load(&path.to_path_buf());
    saved.into_iter().take(256).filter(|r| !r.deployment.demo).map(|r| {
        let mut d = r.deployment;
        d.identity = r.identity;
        d.postmortem = None;
        d
    }).collect()
}

fn save_observed(path: &std::path::Path, list: &[ClusterDeployment]) {
    let saved: Vec<SavedObservation> = list.iter().filter(|d| !d.demo).take(256).map(|d| {
        let mut deployment = d.clone();
        deployment.postmortem = None; // no logs, scripts, commands, keys or environments in this cache
        SavedObservation { identity: d.identity.clone(), deployment }
    }).collect();
    super::save(&path.to_path_buf(), &saved);
}

fn key(m: &DeploymentMember) -> (String, String, u64) {
    (m.node.clone(), m.id.clone(), m.finished_ms)
}

impl Models {
    fn pm_cache(&self, live: bool) -> std::sync::MutexGuard<'_, Cache> {
        let mut c = self.postmortems.lock().unwrap();
        if c.live != Some(live) {
            *c = Cache { live: Some(live), observed: if live { read_observed(&self.observed_path) } else { Vec::new() }, ..Default::default() };
        }
        c
    }

    /// Every deployment on the cluster, with the postmortems read so far (for `cluster_deployments_list`).
    pub fn cluster_deployments(&self, ctx: &Ctx) -> Vec<ClusterDeployment> {
        let snap = ctx.snapshot.clone().unwrap_or_default();
        let deployments = self.deployments(ctx);
        let mut cache = self.pm_cache(ctx.live);
        let mut list = retain_members(&snap, &cache.observed, &deployments);
        if ctx.live && cache.observed != list { save_observed(&self.observed_path, &list); }
        cache.observed = list.clone();
        for d in list.iter_mut() {
            let Some(head) = d.members.first() else { continue };
            if let Some((served, model)) = cache.named.get(&head.id) {
                if d.served.is_none() {
                    d.served = served.clone();
                }
                if d.model.is_none() {
                    d.model = model.clone();
                }
                d.label = d.model.as_deref().map(folder_name).or_else(|| d.served.clone()).unwrap_or_else(|| d.id.clone());
            }
            if d.state == "failed" {
                d.postmortem = cache.done.get(&key(head)).cloned();
            }
        }
        list
    }

    /// Whether the deployments' ids or states changed since the last call (deployments://changed).
    pub fn deployments_changed(&self, ctx: &Ctx, list: &[ClusterDeployment]) -> bool {
        let sig = list.iter().map(|d| format!("{}:{}:{:?}:{:?}:{:?}", d.id, d.state, d.postmortem.as_ref().map(|p| p.read_ms), d.nodes,
            d.members.iter().map(|m| (&m.node, &m.id, &m.state, &m.unavailable_reason)).collect::<Vec<_>>())).collect::<Vec<_>>().join(",");
        let mut cache = self.pm_cache(ctx.live);
        let changed = cache.announced != sig;
        cache.announced = sig;
        changed
    }

    /// Reads the postmortem of every failed deployment that has none yet: once per head container
    /// and exit. Returns whether any was read.
    pub async fn read_postmortems(&self, ctx: &Ctx) -> bool {
        let mut todo: Vec<ClusterDeployment> = Vec::new();
        {
            let list = self.cluster_deployments(ctx);
            let mut cache = self.pm_cache(ctx.live);
            for d in list.into_iter().filter(|d| d.state == "failed") {
                let Some(head) = d.members.first() else { continue };
                let k = key(head);
                if ctx.live && !ctx.snapshot.as_ref().is_some_and(|s| s.nodes.iter().any(|n| n.id == head.node && n.status == crate::model::LinkStatus::Online && !n.stale)) { continue; }
                if cache.done.get(&k).is_some_and(|p| !p.retryable)
                    || cache.retries.get(&k).is_some_and(|(_, due)| now_ms() < *due)
                    || !cache.reading.insert(k) { continue; }
                todo.push(d);
            }
        }
        let mut any = false;
        for d in todo {
            let head = d.members[0].clone();
            let pm = self.postmortem_of(ctx, &d, &head).await;
            let mut cache = self.postmortems.lock().unwrap();
            if cache.live != Some(ctx.live) { return any; }
            cache.reading.remove(&key(&head));
            cache.reads += 1;
            if pm.retryable {
                let attempt = cache.retries.get(&key(&head)).map_or(0, |(attempt, _)| *attempt);
                let delay = [30_000u64, 120_000, 600_000].get(attempt).copied();
                cache.retries.insert(key(&head), (attempt + 1, delay.map_or(u64::MAX, |d| now_ms().saturating_add(d))));
            } else { cache.retries.remove(&key(&head)); }
            cache.done.insert(key(&head), pm);
            any = true;
        }
        any
    }

    pub fn retry_postmortem(&self, ctx: &Ctx, id: &str) {
        let list = self.cluster_deployments(ctx);
        let Some(head) = list.iter().find(|d| d.id == id && d.state == "failed").and_then(|d| d.members.first()) else { return };
        let mut cache = self.pm_cache(ctx.live);
        if !cache.reading.contains(&key(head)) {
            cache.done.remove(&key(head));
            cache.retries.remove(&key(head));
        }
    }

    async fn postmortem_of(&self, ctx: &Ctx, d: &ClusterDeployment, head: &DeploymentMember) -> Postmortem {
        let mut d = d.clone();
        let base = |error: String| Postmortem { log_node: head.node.clone(), log_container: head.container.clone(), read_ms: now_ms(), error: Some(crate::live::audit::redact(&error)), ..Default::default() };
        if !crate::ops::containers::id_ok(&head.id) {
            return base("Its container id isn't one SparkBox can read.".into());
        }
        // docker inspect: the served name, the model folder and the mounts.
        let inspect = if ctx.live { self.live.container_inspect(&head.node, &head.id).await.ok() } else { self.cluster.as_ref().and_then(|c| c.lock().unwrap().container_inspect(&head.node, &head.id).ok()) };
        let served = inspect.as_ref().and_then(|v| inspect_env(v, "SERVED_NAME"));
        let model = inspect.as_ref().and_then(|v| inspect_env(v, "MODEL_LOCAL_NAME")).map(|m| format!("/models/{m}"));
        let mut mounts = inspect.as_ref().map(inspect_mounts).unwrap_or_default();
        if !ctx.live && mounts.is_empty() {
            mounts = demo_mounts(head);
        }
        {
            let mut cache = self.postmortems.lock().unwrap();
            if cache.live == Some(ctx.live) && (served.is_some() || model.is_some()) {
                cache.named.insert(head.id.clone(), (served.clone(), model.clone()));
            }
        }
        d.served = d.served.or(served);
        d.model = d.model.or(model);
        // The log, once.
        let cmd = format!("docker logs --tail {LOG_TAIL} {} 2>&1", head.id);
        let log = if ctx.live {
            match self.live.run(&head.node, &cmd, None, Duration::from_secs(30)).await {
                Ok(o) if o.code == Some(0) => Ok(o.stdout),
                Ok(o) => Err(format!("docker logs said: {}", o.stdout.lines().last().unwrap_or("").trim())),
                Err(e) => Err(e),
            }
        } else {
            demo_log(head).map(String::from).ok_or_else(|| "The Demo has no log for this container.".to_string())
        };
        let log = match log {
            Ok(l) => l,
            Err(e) => {
                let mut pm = base(format!("Couldn't read its log: {e}"));
                pm.retryable = ctx.live && !e.contains("No such container") && !e.contains("No such object");
                return pm;
            },
        };
        // The script it runs, when the likely cause names an option it sets.
        let rough = read_log(&d, &log, None);
        let flag = rough.cause.as_deref().and_then(|id| cases().iter().find(|(c, _)| c.id == id)).map(|(c, _)| c.flag.clone()).unwrap_or_default();
        let container = self.container_command(ctx, &head.node, &head.id);
        let script = match (flag.is_empty(), container.as_deref().and_then(|cmd| script_on_host(cmd, &mounts))) {
            (false, Some((host, shown))) => {
                let text = if ctx.live {
                    self.live.sparkjob(&head.node, &format!("read {}", sh_quote(&host)), None, Duration::from_secs(20)).await.ok().and_then(|v| v["content"].as_str().map(String::from))
                } else {
                    demo_script(&host).map(String::from)
                };
                text.map(|t| (shown, t, host))
            }
            _ => None,
        };
        let mut pm = read_log(&d, &log, script.as_ref().map(|(s, t, _)| (s.as_str(), t.as_str())));
        pm.script_path = script.as_ref().map(|(_, _, host)| host.clone());
        pm.script_line_number = pm.script_line.as_deref().and_then(|l| l.rsplit_once(" line ")).and_then(|(_, tail)| tail.split(':').next()?.parse().ok());
        pm.log_node = head.node.clone();
        pm.log_container = head.container.clone();
        pm
    }

    fn container_command(&self, ctx: &Ctx, node: &str, id: &str) -> Option<String> {
        ctx.snapshot.as_ref()?.nodes.iter().find(|n| n.id == node)?.containers.iter().find(|c| c.id == id).map(|c| c.command.clone())
    }

    #[cfg(test)]
    pub(crate) fn postmortem_reads(&self) -> u64 {
        self.postmortems.lock().unwrap().reads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = include_str!("fixtures/logs/glm53-rank0.log");
    const SCRIPT: &str = include_str!("fixtures/logs/glm53-inner.sh");

    /// The real cluster on the tour (snapshot-tour-2.json, Oct 29): glm53 ×4, workbench-proxy and
    /// gx10's four old containers.
    pub(crate) fn tour() -> ClusterSnapshot {
        serde_json::from_str(include_str!("fixtures/snapshot-tour-2.json")).expect("snapshot-tour-2.json")
    }

    fn glm53() -> ClusterDeployment {
        infer(&tour()).into_iter().find(|d| d.id == "glm53").expect("glm53")
    }

    #[test]
    fn live_observations_survive_demo_switches_and_a_new_models_instance() {
        let dir = crate::fake_node::temp("observed-source");
        let make = || {
            let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("none"), |_| {});
            let live = crate::live::Live::new(vpn.clone(), dir.join("known"), || Ok(None));
            let llm = crate::llm::Llm::new(dir.join("providers.json"), vpn);
            Models::new(dir.clone(), live, llm, None)
        };
        let models = make();
        let mut snap = tour();
        for n in &mut snap.nodes { n.status = crate::model::LinkStatus::Online; n.stale = false; }
        let mut ctx = Ctx { live: true, snapshot: Some(snap.clone()), settings: Default::default(), inventory: Default::default() };
        let before = models.cluster_deployments(&ctx);
        let saved = std::fs::read(&models.observed_path).unwrap();
        ctx.live = false; snap.demo = true; ctx.snapshot = Some(snap);
        models.cluster_deployments(&ctx);
        assert_eq!(std::fs::read(&models.observed_path).unwrap(), saved);
        ctx.live = true; ctx.snapshot = None;
        for models in [models, make()] {
            let away = models.cluster_deployments(&ctx);
            assert_eq!(away[0].nodes, before[0].nodes);
            assert_eq!(away[0].state, "unknown");
            assert_eq!(away[0].members[0].last_seen_ms, before[0].members[0].last_seen_ms);
        }
    }

    #[test]
    fn external_members_survive_a_cold_start_and_reconcile_with_fresh_reports() {
        let dir = crate::fake_node::temp("observed-restart");
        let path = dir.join("deployments-observed.json");
        let mut snap = tour();
        for n in &mut snap.nodes { n.status = crate::model::LinkStatus::Online; n.stale = false; }
        let before = retain_members(&snap, &[], &[]);
        save_observed(&path, &before);
        let restored = read_observed(&path);
        assert_eq!(restored, before);
        let absent = retain_members(&ClusterSnapshot::default(), &restored, &[]);
        assert_eq!(absent[0].members.len(), 4);
        assert_eq!(absent[0].state, "unknown");
        assert_eq!(absent[0].members[0].last_seen_ms, before[0].members[0].last_seen_ms);
        snap.nodes[1].containers.clear();
        let back = retain_members(&snap, &absent, &[]);
        assert_eq!(back[0].members.iter().find(|m| m.node == "cell2").unwrap().state, "missing");
        assert_eq!(back[0].state, "partial");
        save_observed(&path, &back);
        assert_eq!(read_observed(&path), back);
        std::fs::write(&path, b"{partial").unwrap();
        assert!(read_observed(&path).is_empty(), "a damaged optional cache cannot prevent startup");
    }

    #[test]
    fn saved_observations_keep_ownership_separate_and_exclude_demo_and_logs() {
        let path = crate::fake_node::temp("observed-ownership").join("observed.json");
        let mut d = glm53();
        d.identity = Some(("release".into(), "load-a".into()));
        d.postmortem = Some(Postmortem { log_line: Some("must-not-save-this-log".into()), ..Default::default() });
        let mut demo = d.clone(); demo.demo = true;
        save_observed(&path, &[d.clone(), demo]);
        let restored = read_observed(&path);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].identity, d.identity);
        assert!(restored[0].postmortem.is_none());
        assert!(!std::fs::read_to_string(path).unwrap().contains("must-not-save-this-log"));
        assert!(serde_json::to_value(&restored[0]).unwrap().get("identity").is_none());
    }

    #[test]
    fn offline_members_keep_their_ranks_without_becoming_current_failures() {
        let mut snap = tour();
        for n in &mut snap.nodes { n.status = crate::model::LinkStatus::Online; n.stale = false; }
        let before = retain_members(&snap, &[], &[]);
        let d = before.iter().find(|d| d.id == "glm53").unwrap();
        assert_eq!(d.state, "failed", "fresh exits still count");
        let at = snap.ts_ms;
        snap.ts_ms += 60_000;
        for n in snap.nodes.iter_mut().skip(1) { n.status = crate::model::LinkStatus::Offline; n.containers.clear(); }
        let away = retain_members(&snap, &before, &[]);
        let d = away.iter().find(|d| d.id == "glm53").unwrap();
        assert_eq!(d.state, "unknown");
        assert_eq!(d.nodes, ["cell1", "cell2", "cell3", "gx10-0000"]);
        assert_eq!(d.members.len(), 4);
        assert!(d.members.iter().skip(1).all(|m| m.state == "unknown" && m.last_seen_ms == Some(at)));
        assert!(d.finished_ms.is_none() && d.postmortem.is_none());
        // A completely absent snapshot is not evidence of deletion, even after a day away.
        snap.nodes.clear(); snap.ts_ms += 86_400_000;
        let absent = retain_members(&snap, &away, &[]);
        assert_eq!(absent[0].nodes.len(), 4);
        assert!(absent[0].members.iter().all(|m| m.state == "unknown"));
        // Fresh observations, rather than cached exits, recover the actual state.
        let mut back = tour();
        for n in &mut back.nodes { n.status = crate::model::LinkStatus::Online; n.stale = false; }
        let recovered = retain_members(&back, &absent, &[]);
        assert_eq!(recovered[0].state, "failed");
        back.nodes[1].containers.clear();
        let removed = retain_members(&back, &recovered, &[]);
        assert_eq!(removed[0].nodes.len(), 4);
        assert_eq!(removed[0].members.iter().find(|m| m.node == "cell2").unwrap().state, "missing");
        for n in &mut back.nodes { n.containers.clear(); }
        assert!(retain_members(&back, &removed, &[]).is_empty(), "fresh confirmed removal retires an external group");
    }

    #[test]
    fn a_saved_load_keeps_expected_nodes_on_a_cold_disconnected_start() {
        let load = super::super::Deployment {
            spec: super::super::plan::DeploySpec { id: "saved".into(), nodes: vec!["cell1".into(), "cell2".into(), "cell3".into()], served_name: "model".into(), ..Default::default() },
            state: super::super::DeploymentState::Ready, ..Default::default()
        };
        let list = retain_members(&ClusterSnapshot::default(), &[], &[load.clone()]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].nodes, ["cell1", "cell2", "cell3"]);
        assert_eq!(list[0].state, "unknown");
        assert!(list[0].members.iter().all(|m| m.last_seen_ms.is_none() && m.state == "unknown"));
        let mut changed = load;
        changed.spec.nodes.pop();
        let updated = retain_members(&ClusterSnapshot::default(), &list, &[changed]);
        assert_eq!(updated[0].nodes.len(), 2);
        assert_eq!(updated[0].members.len(), 2, "the saved specification, not old membership, is authoritative");
    }

    #[test]
    fn explicit_rendezvous_ports_separate_identical_external_scripts() {
        let mut snap = tour();
        for (i, n) in snap.nodes.iter_mut().enumerate() {
            for c in n.containers.iter_mut().filter(|c| c.name == "glm53") {
                c.env_flags.insert("NNODES".into(), "2".into());
                c.env_flags.insert("HEAD_IP".into(), "192.168.100.1".into());
                c.env_flags.insert("MASTER_PORT".into(), if i < 2 { "29500" } else { "29501" }.into());
            }
        }
        let list = infer(&snap);
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|d| d.nodes.len() == 2));
        for n in &mut snap.nodes { n.status = crate::model::LinkStatus::Online; n.stale = false; }
        let before = retain_members(&snap, &[], &[]);
        for n in snap.nodes.iter_mut().take(2) { n.status = crate::model::LinkStatus::Offline; n.containers.clear(); }
        let away = retain_members(&snap, &before, &[]);
        assert_eq!(away.len(), 2);
        assert_ne!(away[0].id, away[1].id, "an absent head must not merge distinct UI groups");
        for old in &before {
            assert!(away.iter().any(|d| d.id == old.id && d.nodes == old.nodes));
        }
    }

    #[test]
    fn shared_commands_and_projects_never_bridge_owned_deployments() {
        let mut ranks = c_l2("tp4");
        for (i, (_, c)) in ranks.iter_mut().enumerate() {
            c.labels.insert("sparkbox.instance".into(), if i < 2 { "dev" } else { "release" }.into());
            c.project = Some("same-project".into());
            c.command = format!("bash /m/inner.sh {i}");
        }
        let list = infer(&cluster_of(ranks));
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|d| d.nodes.len() == 2));
    }

    #[test]
    fn the_real_containers_group_into_one_failed_glm53() {
        let all = infer(&tour());
        assert_eq!(all.len(), 1, "{:?}", all.iter().map(|d| &d.id).collect::<Vec<_>>());
        let d = &all[0];
        assert_eq!((d.id.as_str(), d.state.as_str(), d.engine.as_str()), ("glm53", "failed", "vllm"));
        assert_eq!(d.nodes, ["cell1", "cell2", "cell3", "gx10-0000"]);
        assert_eq!(d.members.iter().map(|m| (m.node.as_str(), m.rank, m.role.as_str())).collect::<Vec<_>>(), [
            ("cell1", Some(0), "head"),
            ("cell2", Some(1), "worker"),
            ("cell3", Some(2), "worker"),
            ("gx10-0000", Some(3), "worker"),
        ]);
        assert!(d.inferred);
        assert_eq!(d.how, "same image and command, ranks 0–3, started together");
        assert_eq!((d.exit_code, d.ran_s), (Some(1), Some(54)), "the head ran 54 s");
        let names: Vec<&str> = d.members.iter().map(|m| m.container.as_str()).collect();
        assert!(!names.contains(&"workbench-proxy") && !names.iter().any(|n| n.contains("hofstadter") || n.contains("babbage")));
    }

    #[test]
    fn starts_further_apart_than_two_minutes_are_two_runs() {
        let mut snap = tour();
        for n in snap.nodes.iter_mut().filter(|n| n.id == "gx10-0000") {
            for c in n.containers.iter_mut().filter(|c| c.name == "glm53") {
                c.started_at += 600;
            }
        }
        let d = infer(&snap).into_iter().find(|d| d.id == "glm53").unwrap();
        assert_eq!(d.nodes, ["cell1", "cell2", "cell3"], "gx10's rank started 10 minutes later: not this run");
    }

    /// A load's ranks as C-L2 leaves them, by rank: (node, container).
    fn c_l2(load: &str) -> Vec<(String, Container)> {
        let all: Value = serde_json::from_str(include_str!("fixtures/loads-c-l2.json")).unwrap();
        all[load].as_array().unwrap().iter().map(|r| (r["node"].as_str().unwrap().to_string(), serde_json::from_value(r["container"].clone()).unwrap())).collect()
    }

    fn cluster_of(ranks: Vec<(String, Container)>) -> ClusterSnapshot {
        let mut nodes: Vec<NodeSnapshot> = ["cell1", "cell2", "cell3", "gx10-0000"].iter().map(|n| NodeSnapshot { id: n.to_string(), ..Default::default() }).collect();
        for (node, c) in ranks {
            nodes.iter_mut().find(|n| n.id == node).unwrap().containers.push(c);
        }
        ClusterSnapshot { nodes, ..Default::default() }
    }

    fn stopped(c: &mut Container, code: i32) {
        c.state = "exited".into();
        c.exit_code = code;
        c.finished_at = c.started_at + 40;
    }

    /// C-X: a start that failed while look-only refused the rollback leaves its ranks where they
    /// stopped. Their Command is only launch.py and their image an id, so the labels say it all.
    #[test]
    fn a_load_look_only_held_back_is_one_deployment_with_its_head_by_role() {
        let mut ranks = c_l2("tp4");
        stopped(&mut ranks[0].1, 78);
        stopped(&mut ranks[2].1, 1);
        let list = infer(&cluster_of(ranks.clone()));
        assert_eq!(list.len(), 1, "{list:?}");
        let d = &list[0];
        assert_eq!((d.id.as_str(), d.state.as_str(), d.engine.as_str()), ("m4tp4", "partial", "vllm"));
        assert_eq!((d.inferred, d.how.as_str()), (false, "SparkBox's deployment label"));
        assert_eq!((d.served.as_deref(), d.model.as_deref()), (Some("gemma-3-27b-it"), Some("/home/sparkuser/models/google/gemma-3-27b-it")));
        assert_eq!((d.label.as_str(), d.image.as_str()), ("gemma-3-27b-it", "vllm/vllm-openai:glm53-flash-arm64-cu130"));
        assert_eq!(d.nodes, ["cell3", "cell1", "gx10-0000", "cell2"], "by rank, not the cluster's order");
        assert_eq!(d.members.iter().map(|m| (m.rank, m.role.as_str())).collect::<Vec<_>>(), [(Some(0), "head"), (Some(1), "worker"), (Some(2), "worker"), (Some(3), "worker")]);
        assert_eq!(d.exit_code, Some(78), "the head's exit");

        // All stopped: failed, and the postmortem reads the head.
        for (_, c) in ranks.iter_mut() {
            stopped(c, 1);
        }
        ranks[0].1.exit_code = 78;
        let d = infer(&cluster_of(ranks.clone())).remove(0);
        assert_eq!((d.state.as_str(), d.members[0].container.as_str()), ("failed", "sparkbox-m4tp4-r0"));

        // It stopped before its head started: still one deployment that says what it was.
        ranks.remove(0);
        let d = infer(&cluster_of(ranks)).remove(0);
        assert_eq!((d.id.as_str(), d.state.as_str(), d.served.as_deref()), ("m4tp4", "failed", Some("gemma-3-27b-it")));
        assert_eq!(d.members.iter().map(|m| m.rank).collect::<Vec<_>>(), [Some(1), Some(2), Some(3)]);
    }

    /// The label is the deployment: however long a rank took to start, and even a worker
    /// restarted by hand, it isn't a second run.
    #[test]
    fn a_labelled_load_is_one_deployment_however_far_apart_its_ranks_started() {
        let mut ranks = c_l2("tp2");
        ranks[0].1.started_at += 600;
        let d = infer(&cluster_of(ranks));
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].nodes, ["cell1", "cell3"]);
    }

    #[test]
    fn the_same_head_in_the_environment_groups_containers() {
        let mut snap = tour();
        for n in snap.nodes.iter_mut() {
            for c in n.containers.iter_mut().filter(|c| c.name == "glm53") {
                // Different commands, the same NNODES / HEAD_IP / MASTER_PORT.
                c.command = format!("/start-{}.sh", n.id);
                c.env_flags.insert("NNODES".into(), "4".into());
                c.env_flags.insert("HEAD_IP".into(), "192.168.100.1".into());
                c.env_flags.insert("MASTER_PORT".into(), "29501".into());
            }
        }
        let d = infer(&snap).into_iter().find(|d| d.id == "glm53").unwrap();
        assert_eq!(d.nodes.len(), 4);
        assert_eq!(d.members[0].node, "cell1", "the node at HEAD_IP is the head");
        assert_eq!(d.how, "same head address and port in their environment, started together");
    }

    #[test]
    fn wrapped_lines_are_joined() {
        let raw = include_str!("fixtures/logs/glm53-rank0.log");
        let wrapped: String = raw
            .lines()
            .flat_map(|l| {
                let chars: Vec<char> = l.chars().collect();
                if chars.is_empty() {
                    vec![String::new()]
                } else {
                    chars.chunks(131).map(|c| c.iter().collect::<String>()).collect()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(join_wrapped(&wrapped), raw.lines().map(String::from).collect::<Vec<_>>());
        assert_eq!(join_wrapped(raw), raw.lines().map(String::from).collect::<Vec<_>>(), "a docker log stays as it is");
    }

    #[test]
    fn postmortem_reads_the_real_glm53_failure() {
        let d = ClusterDeployment { model: Some("/models/GLM-5.3-Flash-NVFP4".into()), ..glm53() };
        let pm = read_log(&d, LOG, Some(("~/glm53/inner.sh", SCRIPT)));
        assert_eq!(pm.title.as_deref(), Some("No attention kernel for a bf16 KV cache on GB10"));
        assert_eq!(pm.cause.as_deref(), Some("sparse-mla-kv-dtype"));
        assert_eq!(
            pm.explanation.as_deref(),
            Some("GLM-5.3-Flash uses sparse MLA attention. On GB10 (compute 12.1) this vLLM build (0.28.1rc1) has one kernel for it, FlashInfer's SM120 sparse MLA, and that kernel doesn't accept a bfloat16 KV cache; Triton MLA has no sparse support.")
        );
        assert_eq!(pm.fix.as_deref(), Some("Check the model's recipe for a compatible checkpoint, image and KV cache type. Changing the KV cache type alone may not fix this. SparkBox doesn't change your launch script."));
        let last = pm.lines.last().unwrap();
        assert!(last.starts_with("ValueError: No valid attention backend found for cuda with AttentionSelectorConfig(head_size=512, dtype=torch.bfloat16, kv_cache_dtype=bfloat16,"), "{last}");
        assert!(last.ends_with("FLASHINFER_MLA_SPARSE_SM120: [kv_cache_dtype not supported, kv_cache_dtype not supported]}."), "{last}");
        assert_eq!(pm.lines.len(), 3);
        assert_eq!(pm.lines[..2], ["attention_cls = current_platform.get_attn_backend_cls(".to_string(), "raise ValueError(".to_string()]);
        assert!(pm.lines.iter().all(|l| !l.starts_with("File \"") && !l.contains("See root cause")), "{:?}", pm.lines);
        assert_eq!(pm.script_line.as_deref(), Some("~/glm53/inner.sh line 22: --kv-cache-dtype bfloat16"));
        assert_eq!((pm.vllm.as_deref(), pm.tp, pm.kv_dtype.as_deref()), (Some("0.28.1rc1"), Some(4), Some("bf16")));
        assert!(pm.log_line.as_deref().unwrap().starts_with("(Worker_TP0 pid=251) ERROR 10-26 22:21:14"));
    }

    #[test]
    fn every_title_is_short_and_hedged_or_the_given_one() {
        for (c, _) in cases() {
            assert!(c.title.chars().count() <= 60 && c.expect.chars().count() <= 60, "{}", c.id);
        }
    }

    fn signature(id: &str) {
        let (c, _) = cases().iter().find(|(c, _)| c.id == id).unwrap_or_else(|| panic!("no case {id}"));
        let log = format!("(APIServer pid=1) INFO 10-27 09:00:00 [api_utils.py:347] version 0.28.1rc1\n{}\n(APIServer pid=1) RuntimeError: Engine core initialization failed. See root cause above. Failed core proc(s): {{}}\n", c.sample);
        let d = ClusterDeployment { id: "x".into(), ..Default::default() };
        let pm = read_log(&d, &log, None);
        assert_eq!(pm.cause.as_deref(), Some(id), "{:?}", pm.lines);
        assert_eq!(pm.title.as_deref(), Some(c.expect.as_str()));
        assert!(pm.fix.as_deref().is_some_and(|f| f.contains("SparkBox doesn't change your launch script") || id == "engine-init"));
    }

    #[test]
    fn signature_sparse_mla_kv_dtype() {
        signature("sparse-mla-kv-dtype");
    }
    #[test]
    fn signature_cuda_oom() {
        signature("cuda-oom");
    }
    #[test]
    fn signature_nccl() {
        signature("nccl");
    }
    #[test]
    fn signature_port_in_use() {
        signature("port-in-use");
    }
    #[test]
    fn signature_hf_gated() {
        signature("hf-gated");
    }
    #[test]
    fn signature_disk_full() {
        signature("disk-full");
    }
    #[test]
    fn signature_hf_offline_missing() {
        signature("hf-offline-missing");
    }
    #[test]
    fn signature_ray_join() {
        signature("ray-join");
    }
    #[test]
    fn signature_quant_unsupported() {
        signature("quant-unsupported");
    }
    #[test]
    fn signature_engine_init_quotes_the_first_error() {
        let log = "(EngineCore pid=178) ERROR 10-26 22:21:17 [core.py:1366] EngineCore failed to start.\n(EngineCore pid=178) ERROR 10-26 22:21:17 [core.py:1366] Exception: something nobody has a signature for\n(APIServer pid=1) RuntimeError: Engine core initialization failed. See root cause above. Failed core proc(s): {}\n";
        let pm = read_log(&ClusterDeployment::default(), log, None);
        assert_eq!(pm.cause.as_deref(), Some("engine-init"));
        assert_eq!(pm.title.as_deref(), Some("vLLM's engine didn't start; the first error is quoted"));
        assert_eq!(pm.lines.last().map(String::as_str), Some("EngineCore failed to start."));
    }

    /// P1.46: a key echoed on the decisive line or the lines before it (`--api-key KEY`,
    /// `HF_TOKEN=…`) is masked in the lines the card shows and in the log line it quotes.
    #[test]
    fn secrets_on_the_decisive_lines_are_masked() {
        let key = "sbk-0123456789abcdef0123456789";
        let hf = "hf_AbCdEf0123456789AbCdEf0123456789ab";
        let start: String = (0..8).map(|i| format!("(APIServer pid=1) INFO 10-27 09:00:0{i} [api_utils.py:347] step {i}\n")).collect();
        let log = format!(
            "{start}(APIServer pid=1) INFO 10-27 09:00:08 [launcher.py:12] started: vllm serve /models/x --api-key {key}\n\
             (APIServer pid=1) INFO 10-27 09:00:09 [env.py:3] HF_TOKEN={hf}\n\
             (APIServer pid=1) ValueError: refusing --api-key {key} with HF_TOKEN={hf}\n\
             (APIServer pid=1) RuntimeError: Engine core initialization failed. See root cause above. Failed core proc(s): {{}}\n"
        );
        let pm = read_log(&ClusterDeployment::default(), &log, None);
        assert_eq!(pm.lines.len(), 3, "{:?}", pm.lines);
        assert!(pm.lines[2].starts_with("ValueError: refusing --api-key ‹redacted›"), "{:?}", pm.lines);
        let quoted = pm.log_line.clone().unwrap();
        assert!(quoted.starts_with("(APIServer pid=1) ValueError: refusing --api-key ‹redacted›"), "{quoted}");
        for text in pm.lines.iter().chain([&quoted]) {
            assert!(!text.contains(key) && !text.contains(hf), "a secret got through: {text}");
        }
        assert!(pm.lines[1].contains("HF_TOKEN=‹redacted›"), "{:?}", pm.lines);
    }

    #[test]
    fn scripts_are_found_through_their_mount() {
        let mounts = vec![("/home/sparkuser/glm53".to_string(), "/glm53".to_string()), ("/home/sparkuser/models".to_string(), "/models".to_string())];
        assert_eq!(script_on_host("/bin/bash /glm53/inner.sh 0", &mounts), Some(("/home/sparkuser/glm53/inner.sh".into(), "~/glm53/inner.sh".into())));
        assert_eq!(script_on_host("/glm53/inner.sh 2", &mounts).map(|s| s.1), Some("~/glm53/inner.sh".into()));
        assert_eq!(script_on_host("vllm serve /models/x", &mounts), None);
        assert_eq!(script_on_host("/bin/bash /opt/run.sh", &mounts), None, "not from a host folder");
        assert_eq!(script_line("~/glm53/inner.sh", SCRIPT, "--port").as_deref(), Some("~/glm53/inner.sh line 18: --port \"$PORT\""));
    }
}
