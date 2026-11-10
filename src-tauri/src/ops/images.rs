//! Docker images on each node, read only (the Images window): `docker image ls` and `docker
//! system df` as JSON lines, joined with the snapshot's containers for which ones use each
//! image and when one last ran. Nothing here changes a node. In Demo each simulated node
//! answers `docker image ls` itself (demo.rs `images`), read by the same parser.

use std::time::Duration;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Ops;
use crate::live::audit;
use crate::model::{ClusterSnapshot, NodeSnapshot};

/// What runs on a node: both are reads (the classifier and the gate test agree).
pub const LIST_IMAGES: &str = "docker image ls --format '{{json .}}'";
pub const DISK_USAGE: &str = "docker system df --format '{{json .}}'";
/// Between the two outputs, so one SSH exec reads both.
const SPLIT: &str = "--- sparkbox df";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub node: String,
    /// The short id, as `docker image ls` prints it.
    pub id: String,
    /// "nvcr.io/nvidia/vllm"; "<none>" for a dangling image.
    pub repository: String,
    pub tag: String,
    /// Bytes on disk (shared layers counted in each image, as Docker reports it).
    pub size: u64,
    pub created_ms: Option<u64>,
    /// No tag: left behind by a pull or a build.
    pub dangling: bool,
    /// The containers made from it, running or not.
    pub used_by: Vec<String>,
    /// How many of those are running now.
    pub running: u32,
    /// When one of its containers last ran: now while one runs, else the latest exit. None
    /// when no container uses it.
    pub last_used_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeImages {
    pub node: String,
    pub images: Vec<Image>,
    /// Docker's own total for images (system df), else the sum of the images.
    pub total: u64,
    /// What Docker says could be freed (images no container uses).
    pub reclaimable: Option<u64>,
    /// Why this node's images couldn't be read (offline, docker refused).
    pub error: Option<String>,
}

/// "21.4GB", "512MB", "1.2kB", "0B" (Docker's decimal units; binary ones are read too).
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mult = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.0,
        "MiB" => 1024f64.powi(2),
        "GiB" => 1024f64.powi(3),
        "TiB" => 1024f64.powi(4),
        _ => return None,
    };
    Some((n * mult).round() as u64)
}

/// "2026-04-20 10:11:12 +0000 UTC" (docker image ls CreatedAt) → ms since the epoch.
pub fn parse_created(s: &str) -> Option<u64> {
    let mut parts = s.split_whitespace();
    let (date, time, zone) = (parts.next()?, parts.next()?, parts.next().unwrap_or("+0000"));
    let d: Vec<i64> = date.split('-').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let t: Vec<i64> = time.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let z = zone.trim_start_matches(['+', '-']);
    let offset = if z.len() == 4 { sign * (z[..2].parse::<i64>().ok()? * 3600 + z[2..].parse::<i64>().ok()? * 60) } else { 0 };
    let days = days_from_civil(d[0], d[1] as u32, d[2] as u32);
    let secs = days * 86_400 + t[0] * 3600 + t[1] * 60 + t[2] - offset;
    (secs >= 0).then_some(secs as u64 * 1000)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm, the inverse of audit's civil_from_days).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Reads `docker image ls --format '{{json .}}'`, then (after SPLIT) `docker system df`'s
/// JSON lines: the images, and Docker's total and reclaimable bytes for them.
pub fn parse(node: &str, out: &str) -> (Vec<Image>, Option<(u64, u64)>) {
    let (ls, df) = out.split_once(SPLIT).unwrap_or((out, ""));
    let images = ls
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .map(|v| {
            let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            let (repository, tag) = (s("Repository"), s("Tag"));
            Image {
                node: node.to_string(),
                id: s("ID").trim_start_matches("sha256:").chars().take(12).collect(),
                dangling: repository == "<none>" || tag == "<none>",
                size: parse_size(&s("Size")).or_else(|| parse_size(&s("VirtualSize"))).unwrap_or(0),
                created_ms: parse_created(&s("CreatedAt")),
                repository,
                tag,
                ..Default::default()
            }
        })
        .collect();
    let totals = df
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .find(|v| v["Type"].as_str() == Some("Images"))
        .and_then(|v| {
            let size = parse_size(v["Size"].as_str()?)?;
            let reclaim = v["Reclaimable"].as_str().and_then(|r| parse_size(r.split_whitespace().next()?)).unwrap_or(0);
            Some((size, reclaim))
        });
    (images, totals)
}

/// Whether container image reference `r` ("repo:tag", "repo", an id) names `img`.
fn names(img: &Image, r: &str) -> bool {
    let r = r.trim_start_matches("sha256:");
    if !img.id.is_empty() && r.len() >= 6 && (img.id.starts_with(r) || r.starts_with(&img.id)) {
        return true;
    }
    if img.dangling {
        return false;
    }
    let (repo, tag) = match r.rsplit_once(':').filter(|(_, t)| !t.contains('/')) {
        Some((repo, tag)) => (repo, tag),
        None => (r, "latest"),
    };
    let short = |x: &str| x.trim_start_matches("docker.io/").trim_start_matches("library/").to_string();
    short(repo) == short(&img.repository) && tag == img.tag
}

/// Fills in each image's containers, running count and last use from the node's snapshot.
pub fn join_containers(images: &mut [Image], node: &NodeSnapshot, now_ms: u64) {
    for img in images.iter_mut() {
        let using: Vec<&crate::model::Container> = node.containers.iter().filter(|c| names(img, &c.image)).collect();
        for c in using {
            img.used_by.push(c.name.clone());
            let at = if c.state == "running" {
                img.running += 1;
                now_ms
            } else {
                c.finished_at.max(c.started_at) * 1000
            };
            img.last_used_ms = Some(img.last_used_ms.map_or(at, |t| t.max(at)));
        }
    }
}

/// Demo: the simulated node's own `docker image ls` lines, read as a real node's are; Docker's
/// total is the images' sum, and what it could free is the images no container uses.
pub fn demo_node_images(demo: &crate::demo::DemoCluster, n: &NodeSnapshot, now_ms: u64) -> NodeImages {
    let mut out = NodeImages { node: n.id.clone(), ..Default::default() };
    match demo.images(&n.id) {
        Err(e) => out.error = Some(e),
        Ok(list) => {
            let lines: Vec<String> = list.iter().filter_map(|i| serde_json::to_string(i).ok()).collect();
            let (mut images, _) = parse(&n.id, &lines.join("\n"));
            join_containers(&mut images, n, now_ms);
            out.total = images.iter().map(|i| i.size).sum();
            out.reclaimable = Some(images.iter().filter(|i| i.used_by.is_empty()).map(|i| i.size).sum());
            out.images = images;
        }
    }
    out
}

impl Ops {
    /// Every node's images (or why they couldn't be read), in inventory order. Read only.
    pub async fn images(&self, demo: bool) -> Vec<NodeImages> {
        let Some(snap) = self.cluster() else { return Vec::new() };
        let now = snap.ts_ms;
        if demo {
            return snap.nodes.iter().map(|n| demo_node_images(&self.demo.lock().unwrap(), n, now)).collect();
        }
        join_all(snap.nodes.iter().map(|n| self.node_images(&snap, n))).await
    }

    async fn node_images(&self, snap: &ClusterSnapshot, n: &NodeSnapshot) -> NodeImages {
        let mut out = NodeImages { node: n.id.clone(), ..Default::default() };
        if n.status != crate::model::LinkStatus::Online {
            out.error = Some(format!("{} isn't connected.", n.name));
            return out;
        }
        let cmd = format!("{LIST_IMAGES}; echo '{SPLIT}'; {DISK_USAGE}");
        let reply = audit::as_probe(self.live.run(&n.id, &cmd, None, Duration::from_secs(30))).await;
        match reply {
            Err(e) => out.error = Some(e),
            Ok(o) if o.code != Some(0) && !o.stdout.contains('{') => {
                let why = o.stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("docker didn't answer").trim().to_string();
                out.error = Some(if why.contains("permission denied") { format!("{}: the cluster user can't use Docker ({why}).", n.name) } else { format!("{}: {why}", n.name) });
            }
            Ok(o) => {
                let (mut images, totals) = parse(&n.id, &o.stdout);
                join_containers(&mut images, n, snap.ts_ms);
                out.total = totals.map_or_else(|| images.iter().map(|i| i.size).sum(), |t| t.0);
                out.reclaimable = totals.map(|t| t.1);
                out.images = images;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // In Docker 29's format, with sizes and dates for the test (the containers are the real ones
    // from the Live tour; the Live check lists six images on gx10-0000).
    const GX10: &str = concat!(
        r#"{"Containers":"N/A","CreatedAt":"2026-10-21 03:12:40 +0000 UTC","CreatedSince":"8 days ago","Digest":"<none>","ID":"4f1c2a9e7b3d","Repository":"vllm/vllm-openai","SharedSize":"N/A","Size":"19.6GB","Tag":"glm53-flash-arm64-cu130","UniqueSize":"N/A","VirtualSize":"19.6GB"}"#,
        "\n",
        r#"{"Containers":"N/A","CreatedAt":"2026-03-18 22:01:09 +0000 UTC","CreatedSince":"7 months ago","Digest":"<none>","ID":"9a7e5d0c11f2","Repository":"nvcr.io/nvidia/vllm","SharedSize":"N/A","Size":"21.4GB","Tag":"26.03.post1-py3","UniqueSize":"N/A","VirtualSize":"21.4GB"}"#,
        "\n",
        r#"{"Containers":"N/A","CreatedAt":"2026-02-02 17:40:55 +0000 UTC","CreatedSince":"8 months ago","Digest":"<none>","ID":"c03b8e6f4a21","Repository":"nvcr.io/nvidia/cuda","SharedSize":"N/A","Size":"8.91GB","Tag":"13.0.1-devel-ubuntu24.04","UniqueSize":"N/A","VirtualSize":"8.91GB"}"#,
        "\n",
        r#"{"Containers":"N/A","CreatedAt":"2026-04-25 09:30:00 +0200 CEST","CreatedSince":"6 months ago","Digest":"<none>","ID":"77e0aa31c9d4","Repository":"<none>","SharedSize":"N/A","Size":"3.1GB","Tag":"<none>","UniqueSize":"N/A","VirtualSize":"3.1GB"}"#,
        "\n--- sparkbox df\n",
        r#"{"Active":"3","Reclaimable":"3.1GB (6%)","Size":"53.01GB","TotalCount":"4","Type":"Images"}"#,
        "\n",
        r#"{"Active":"0","Reclaimable":"0B","Size":"0B","TotalCount":"5","Type":"Containers"}"#,
        "\n",
    );

    #[test]
    fn docker_sizes_and_times_read_as_docker_writes_them() {
        assert_eq!(parse_size("21.4GB"), Some(21_400_000_000));
        assert_eq!(parse_size("512MB"), Some(512_000_000));
        assert_eq!(parse_size("1.2kB"), Some(1200));
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("2GiB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("N/A"), None);
        assert_eq!(parse_created("1970-01-02 00:00:00 +0000 UTC"), Some(86_400_000));
        assert_eq!(parse_created("2026-04-25 09:30:00 +0200 CEST"), parse_created("2026-04-25 07:30:00 +0000 UTC"));
        assert_eq!(audit::iso_time(parse_created("2026-10-29 08:15:00 +0000 UTC").unwrap()), "2026-10-29T08:15:00Z");
    }

    #[test]
    fn gx10s_images_with_their_containers_and_last_use() {
        let (mut images, totals) = parse("gx10-0000", GX10);
        assert_eq!(images.len(), 4);
        assert_eq!(totals, Some((53_010_000_000, 3_100_000_000)));
        assert!(images[3].dangling && !images[0].dangling);
        // The real gx10-0000 at the tour: glm53 exited on Oct 26, the others six months ago.
        let node = gx10_at_the_tour();
        let now = parse_created("2026-10-29 08:15:00 +0000 UTC").unwrap();
        join_containers(&mut images, &node, now);
        assert_eq!(images[0].used_by, vec!["glm53"]);
        assert_eq!(images[2].used_by.len(), 3, "the three cuda containers");
        let days = |i: &Image| i.last_used_ms.map(|t| (now - t) / 86_400_000);
        assert_eq!(days(&images[1]), Some(186), "vigilant_hofstadter exited on Apr 26");
        assert_eq!(days(&images[2]), Some(186));
        assert_eq!(images[3].last_used_ms, None, "no container uses the dangling one");
    }

    /// gx10-0000's containers as the Live snapshot had them (live-capture/snapshot-tour-1.json).
    pub(crate) fn gx10_at_the_tour() -> NodeSnapshot {
        let c = |name: &str, image: &str, started_at: u64, finished_at: u64| crate::model::Container {
            id: format!("{name:0<12}").chars().take(12).collect(),
            name: name.into(),
            image: image.into(),
            state: "exited".into(),
            started_at,
            finished_at,
            ..Default::default()
        };
        NodeSnapshot {
            id: "gx10-0000".into(),
            name: "gx10-0000".into(),
            containers: vec![
                c("glm53", "vllm/vllm-openai:glm53-flash-arm64-cu130", 1793053224, 1793053279),
                c("vigilant_hofstadter", "nvcr.io/nvidia/vllm:26.03.post1-py3", 1777186384, 1777186771),
                c("adoring_northcutt", "nvcr.io/nvidia/cuda:13.0.1-devel-ubuntu24.04", 1777185292, 1777185292),
                c("jovial_elgamal", "nvcr.io/nvidia/cuda:13.0.1-devel-ubuntu24.04", 1777185282, 1777185282),
                c("gracious_babbage", "nvcr.io/nvidia/cuda:13.0.1-devel-ubuntu24.04", 1777185261, 1777185262),
            ],
            ..Default::default()
        }
    }

    /// Demo's as-found world answers `docker image ls` per node; the window reads it as it reads
    /// a real node, and gx10-0000's April images show as unused for about 186 days.
    #[test]
    fn demo_images_come_from_the_simulated_nodes() {
        let mut demo = crate::demo::DemoCluster::as_found();
        let snap = demo.snapshot();
        let now = snap.ts_ms;
        let gx = snap.nodes.iter().find(|n| n.id == "gx10-0000").unwrap();
        let got = demo_node_images(&demo, gx, now);
        assert_eq!(got.error, None);
        let refs: Vec<String> = got.images.iter().map(|i| format!("{}:{}", i.repository, i.tag)).collect();
        assert_eq!(refs, demo.images("gx10-0000").unwrap().iter().map(|i| format!("{}:{}", i.repository, i.tag)).collect::<Vec<_>>());
        assert!(got.images.iter().all(|i| i.size > 100_000_000 && i.created_ms.is_some() && !i.used_by.is_empty()), "{:#?}", got.images);
        let old: Vec<&Image> = got.images.iter().filter(|i| i.last_used_ms.is_some_and(|t| now - t > 180 * 86_400_000)).collect();
        assert_eq!(old.len(), 2, "the NGC vLLM and CUDA images, unused since April");
        assert_eq!(got.total, got.images.iter().map(|i| i.size).sum::<u64>());
    }

    #[test]
    fn listing_images_only_reads() {
        use super::super::classify::{classify, Kind};
        for cmd in [LIST_IMAGES, DISK_USAGE, &format!("{LIST_IMAGES}; echo '{SPLIT}'; {DISK_USAGE}")] {
            assert_eq!(classify(cmd), Kind::Read, "{cmd}");
        }
    }
}
