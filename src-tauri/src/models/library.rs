//! Library observations are for display only. Mutations keep using `library`, which
//! performs a fresh, strict scan and never reads this in-memory cache.
use super::*;
use crate::inventory::NodeConfig;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryNodeStatus {
    pub node: String,
    /// ready, offline or error. A failed scan never means an empty disk.
    pub state: String,
    pub checked_at_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryReport {
    pub entries: Vec<LibraryEntry>,
    pub nodes: Vec<LibraryNodeStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryProgress {
    pub step: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct Binding {
    config: NodeConfig,
    root: String,
    generation: Option<u64>,
}

#[derive(Clone)]
struct Observation {
    binding: Binding,
    entries: Vec<LibraryEntry>,
    at_ms: u64,
}

#[derive(Default)]
pub(super) struct Cache {
    ticket: u64,
    nodes: BTreeMap<String, Observation>,
}

impl Cache {
    fn begin(&mut self, bindings: &BTreeMap<String, Binding>) -> (u64, BTreeMap<String, Observation>) {
        self.ticket += 1;
        self.nodes.retain(|node, old| bindings.get(node) == Some(&old.binding));
        (self.ticket, self.nodes.clone())
    }

    fn finish(&mut self, ticket: u64, observations: BTreeMap<String, Observation>) {
        // A slower, older request cannot put removed/replaced nodes back in the cache.
        if self.ticket == ticket { self.nodes = observations; }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanItem {
    path: String,
    root: String,
    repo: String,
    #[serde(default)]
    revision: String,
    format: String,
    size_bytes: u64,
    verified: bool,
    #[serde(default)]
    partial: Option<String>,
    #[serde(default)]
    checked: String,
    #[serde(default)]
    progress: Option<LibraryProgress>,
    #[serde(default)]
    missing_cache_files: u64,
}

/// Reject malformed/truncated replies, including duplicate paths, rather than
/// replacing the last observation with an apparently successful empty scan.
fn parse(node: &str, value: Value) -> Result<Vec<LibraryEntry>, String> {
    let items: Vec<ScanItem> = serde_json::from_value(value).map_err(|_| "Couldn't read the model scan response. Try Find models again.")?;
    let mut paths = std::collections::HashSet::new();
    items.into_iter().map(|s| {
        if !s.path.starts_with('/') || !s.root.starts_with('/') || s.repo.is_empty() || !paths.insert(s.path.clone())
            || !matches!(s.partial.as_deref(), None | Some("stopped" | "downloading" | "copying"))
            || s.partial.is_some() && s.verified
            || s.missing_cache_files > 0 && s.partial.is_none() {
            return Err("The model scan returned inconsistent folder information. Try Find models again.".into());
        }
        let progress = s.progress.filter(|p| s.partial.is_some() && matches!(p.step.as_str(), "downloading" | "copying" | "verifying" | "waiting"));
        let copy = Copy { path: s.path.clone(), size_bytes: s.size_bytes, verified: s.verified, partial: s.partial, check: s.checked, progress, missing_cache_files: s.missing_cache_files };
        Ok(LibraryEntry { key: s.path, in_use: None, repo: s.repo, revision: s.revision, format: s.format,
            size_bytes: s.size_bytes, root: s.root, copies: BTreeMap::from([(node.into(), copy)]) })
    }).collect()
}

fn merge(lists: impl IntoIterator<Item = Vec<LibraryEntry>>) -> Vec<LibraryEntry> {
    let mut entries: BTreeMap<String, LibraryEntry> = BTreeMap::new();
    for item in lists.into_iter().flatten() {
        match entries.get_mut(&item.key) {
            None => { entries.insert(item.key.clone(), item); }
            Some(e) => {
                e.size_bytes = e.size_bytes.max(item.size_bytes);
                if e.revision.is_empty() { e.revision = item.revision; }
                e.copies.extend(item.copies);
            }
        }
    }
    entries.into_values().collect()
}

impl Models {
    fn library_in_use(&self, entries: &mut [LibraryEntry]) {
        let serving = endpoints::in_use(&self.endpoints());
        for e in entries { e.in_use = serving.get(&e.key).or_else(|| serving.get(&e.repo)).cloned(); }
    }

    async fn scan_library_node(&self, ctx: &Ctx, node: &str) -> Result<Vec<LibraryEntry>, String> {
        let cmd = format!("scan {} {}", q(&ctx.settings.model_root), q("~/.cache/huggingface/hub"));
        parse(node, self.sparkjob(node, &cmd, None, Duration::from_secs(120)).await?)
    }

    /// Strict, fresh action input. Never uses observations, even immediately after a report.
    pub async fn library(&self, ctx: &Ctx) -> Result<Vec<LibraryEntry>, String> {
        let mut entries = if ctx.live {
            let nodes = ctx.online();
            let scans = join_all(nodes.iter().filter(|id| ctx.inventory.nodes.iter().any(|n| n.id == **id))
                .map(|node| self.scan_library_node(ctx, node))).await;
            merge(scans.into_iter().collect::<Result<Vec<_>, _>>()?)
        } else { self.demo.lock().unwrap().library() };
        self.library_in_use(&mut entries);
        Ok(entries)
    }

    /// Best-effort read-only display. Cache lives only until this app quits and is
    /// bound to the node configuration, Live generation and model-root setting.
    pub async fn library_report(&self, ctx: &Ctx) -> LibraryReport {
        if !ctx.live {
            self.library_observations.lock().unwrap().begin(&BTreeMap::new());
            let mut entries = self.demo.lock().unwrap().library_observations();
            self.library_in_use(&mut entries);
            return LibraryReport { entries, nodes: ctx.node_ids().into_iter().map(|node| LibraryNodeStatus {
                node, state: "ready".into(), checked_at_ms: Some(now_ms()), error: None,
            }).collect() };
        }
        let bindings: BTreeMap<_, _> = ctx.inventory.nodes.iter().map(|config| (config.id.clone(), Binding {
            config: config.clone(), root: ctx.settings.model_root.clone(), generation: self.live.library_generation(config),
        })).collect();
        let (ticket, mut observations) = self.library_observations.lock().unwrap().begin(&bindings);
        let online = ctx.online();
        let scans = join_all(ctx.inventory.nodes.iter().map(|config| async {
            if !online.contains(&config.id) { return None; }
            let result = self.scan_library_node(ctx, &config.id).await;
            Some(result)
        })).await;
        let mut nodes = Vec::new();
        for (config, scan) in ctx.inventory.nodes.iter().zip(scans) {
            let binding = &bindings[&config.id];
            let scan = if self.live.library_generation(config) != binding.generation || binding.generation.is_none() && online.contains(&config.id) {
                observations.remove(&config.id);
                Some(Err("The node connection settings changed. Find models again.".into()))
            } else { scan };
            let (state, error) = match scan {
                None => ("offline", None),
                Some(Err(e)) => ("error", Some(e)),
                Some(Ok(entries)) => {
                    observations.insert(config.id.clone(), Observation { binding: binding.clone(), entries, at_ms: now_ms() });
                    ("ready", None)
                }
            };
            nodes.push(LibraryNodeStatus { node: config.id.clone(), state: state.into(), error,
                checked_at_ms: observations.get(&config.id).map(|o| o.at_ms) });
        }
        // Fresh metadata wins when another node only has a last-known copy at the same path.
        let lists = nodes.iter().filter(|n| n.state == "ready").chain(nodes.iter().filter(|n| n.state != "ready"))
            .filter_map(|n| observations.get(&n.node).map(|o| o.entries.clone()));
        let mut entries = merge(lists);
        self.library_observations.lock().unwrap().finish(ticket, observations);
        self.library_in_use(&mut entries);
        LibraryReport { entries, nodes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Binding {
        Binding { config: Inventory::example().nodes.remove(0), root: "~/models".into(), generation: Some(1) }
    }
    fn item() -> Value {
        json!({"path":"/home/demo/models/org/tiny", "root":"/home/demo/models", "repo":"org/tiny",
            "format":"safetensors", "sizeBytes":12, "verified":true})
    }
    #[test]
    fn library_scan_rejects_malformed_and_inconsistent_results() {
        for bad in [Value::Null, json!({}), json!([{}]), json!([item(),item()])] {
            assert!(parse("cell1", bad).is_err());
        }
        let mut partial = item(); partial["partial"] = json!("copying");
        assert!(parse("cell1", json!([partial])).is_err(), "unfinished cannot be verified");
        assert!(parse("cell1", json!([])).unwrap().is_empty(), "valid empty scan clears old observations");
        assert!(parse("cell1", json!([item()])).unwrap()[0].copies["cell1"].verified);
    }
    #[test]
    fn missing_cache_files_require_an_incomplete_copy_and_cannot_be_sources() {
        let mut missing = item(); missing["missingCacheFiles"] = json!(3);
        assert!(parse("cell1", json!([missing.clone()])).is_err());
        missing["verified"] = json!(false);
        assert!(parse("cell1", json!([missing.clone()])).is_err());
        missing["partial"] = json!("stopped");
        let entry = parse("cell1", json!([missing.clone()])).unwrap().remove(0);
        assert_eq!(entry.copies["cell1"].missing_cache_files, 3);
        let ctx = Ctx { live: false, settings: Default::default(), inventory: Inventory::example(), snapshot: None };
        assert!(super::super::disk::copy_source(&ctx, &entry).is_err());
        missing["missingCacheFiles"] = json!(-1);
        assert!(parse("cell1", json!([missing])).is_err());
    }
    #[test]
    fn library_observations_are_bound_to_config_root_and_generation() {
        let b = binding();
        for changed in [Binding { root:"~/elsewhere".into(), ..b.clone() }, Binding { generation:Some(2), ..b.clone() },
            Binding { config:NodeConfig { user:"different".into(), ..b.config.clone() }, ..b.clone() },
            Binding { config:NodeConfig { port:2222, ..b.config.clone() }, ..b.clone() }] {
            let mut cache = Cache::default();
            cache.nodes.insert("cell1".into(), Observation { binding:b.clone(), entries:vec![], at_ms:1 });
            let (_, old) = cache.begin(&BTreeMap::from([("cell1".into(), changed)]));
            assert!(old.is_empty());
        }
    }
    #[test]
    fn library_older_request_cannot_restore_removed_nodes_or_demo_data() {
        let b = binding();
        let mut cache = Cache::default();
        let (old, _) = cache.begin(&BTreeMap::from([("cell1".into(), b.clone())]));
        let (new, _) = cache.begin(&BTreeMap::new());
        cache.finish(new, BTreeMap::new());
        cache.finish(old, BTreeMap::from([("cell1".into(), Observation { binding:b, entries:vec![], at_ms:1 })]));
        assert!(cache.nodes.is_empty());
    }
}
