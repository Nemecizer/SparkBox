//! Offline playbook text and local editions. Text is never an executable recipe.
//! Only the compiled catalog can supply native actions or image references.
pub mod commands;
mod source;
#[cfg(test)]
mod tests;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::{Mutex, OnceLock}};

const MAX_FILE: u64 = 32 * 1024 * 1024;
pub const MAX_TEXT: usize = 256 * 1024;
pub const REVISION: &str = "30883150541743d580c9c347700c6ac99fc396d2";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Guide {
    pub id: String, pub title: String, pub published: bool, pub example: bool,
    pub platforms: Vec<String>, pub spark: bool, pub category: String, pub purpose: String,
    pub steps: Vec<String>, pub success: String, pub attention: String, pub actions: Vec<String>,
    pub images: Vec<String>, pub readme: String, pub tree_sha: String, pub documents: Vec<String>,
    pub source_steps: Vec<String>, pub hardware: String, pub prerequisites: String, pub time_risk: String,
}
#[derive(Deserialize)]
pub struct Catalog { pub guides: Vec<Guide> }
pub fn catalog() -> &'static Catalog {
    static DATA: OnceLock<Catalog> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("../../../resources/playbooks/catalog.json")).expect("validated playbook catalog"))
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document { pub path: String, pub git_blob: String, pub sha256: String, pub text: String }
pub fn documents() -> &'static BTreeMap<String, Document> {
    static DATA: OnceLock<BTreeMap<String, Document>> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(include_str!("../../../resources/playbooks/documents.json")).expect("validated playbook documents"))
}
pub fn guide(id: &str) -> Option<&'static Guide> { catalog().guides.iter().find(|g| g.id == id) }
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
pub fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalGuide {
    pub version: u64, pub favorite: bool, pub notes: String, pub completed: Vec<usize>,
    pub editions: Vec<Edition>, pub sources: Vec<Fetched>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edition {
    pub text: String, pub author: String, pub source_revision: String, pub source_sha256: String,
    pub at_ms: u64, pub sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft { pub text: String, pub author: String, pub source_revision: String, pub source_sha256: String }
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fetched {
    pub revision: String, pub tree_sha: String, pub document: Document, pub at_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Save {
    pub id: String, pub expected: u64, pub favorite: bool, pub notes: String,
    pub completed: Vec<usize>, pub edition: Option<Draft>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Disk {
    schema: u32, guides: BTreeMap<String, LocalGuide>, check: Option<source::Check>,
}
impl Default for Disk { fn default() -> Self { Self { schema: 1, guides: BTreeMap::new(), check: None } } }

pub struct HowTo {
    root: PathBuf,
    gate: Mutex<()>,
    pub network: tokio::sync::Mutex<()>,
}
impl HowTo {
    pub fn new(root: PathBuf) -> Self { Self { root, gate: Mutex::new(()), network: tokio::sync::Mutex::new(()) } }
    fn path(&self, demo: bool) -> PathBuf { self.root.join(if demo { "demo.json" } else { "live.json" }) }
    fn load(&self, demo: bool) -> Result<Disk, String> {
        for p in [&self.root, &self.path(demo)] {
            if fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) { return Err("The How To storage location is a link. Its contents were left untouched.".into()); }
        }
        let path = self.path(demo);
        let m = match fs::metadata(&path) { Ok(m) => m, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Disk::default()), Err(e) => return Err(e.to_string()) };
        if !m.is_file() || m.len() > MAX_FILE { return Err("The How To library file is too large or is not a regular file. It was left untouched.".into()); }
        let disk: Disk = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?).map_err(|_| "The local How To library could not be read. It was preserved; restore a backup before saving.")?;
        if disk.schema != 1 || disk.guides.len() > 500 { return Err("This How To library needs a compatible SparkBox version. It was left untouched.".into()); }
        Ok(disk)
    }
    fn write(&self, demo: bool, disk: &Disk) -> Result<(), String> {
        if disk.guides.len() > 500 { return Err("The local library has reached its 500-guide limit. Nothing was replaced.".into()); }
        let bytes = serde_json::to_vec(disk).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_FILE { return Err("The local library has reached its 32 MiB limit. Nothing was replaced; keep a backup before reducing it.".into()); }
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?; }
        crate::util::write_atomic(&self.path(demo), &bytes)?;
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(self.path(demo), fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?; }
        Ok(())
    }
    pub fn read(&self, demo: bool, id: &str) -> Result<LocalGuide, String> {
        let _lock = self.gate.lock().unwrap();
        let d = self.load(demo)?;
        if !valid_id(id) || (guide(id).is_none() && !d.guides.contains_key(id)) { return Err("Choose a guide from the library.".into()); }
        Ok(d.guides.get(id).cloned().unwrap_or_default())
    }
    pub fn saved(&self, demo: bool) -> Result<BTreeMap<String, LocalGuide>, String> {
        let _lock = self.gate.lock().unwrap();
        Ok(self.load(demo)?.guides)
    }
    pub fn save(&self, demo: bool, req: Save) -> Result<LocalGuide, String> {
        let _lock = self.gate.lock().unwrap();
        let mut disk = self.load(demo)?;
        if !valid_id(&req.id) || (guide(&req.id).is_none() && !disk.guides.contains_key(&req.id)) { return Err("Choose a bundled guide or fetch its NVIDIA source first.".into()); }
        let state = disk.guides.entry(req.id.clone()).or_default();
        if req.expected != state.version { return Err("This guide changed in another window. Reload it before saving; your draft has not replaced anything.".into()); }
        let count = guide(&req.id).map(|g| g.steps.len()).unwrap_or(0);
        if req.notes.len() > 20_000 || req.completed.len() > count || req.completed.iter().any(|i| *i >= count) || req.completed.iter().collect::<std::collections::BTreeSet<_>>().len() != req.completed.len() { return Err("Notes or checklist values exceed the guide's limits.".into()); }
        if let Some(e) = req.edition {
            if e.text.trim().is_empty() || e.text.len() > MAX_TEXT || e.author.is_empty() || e.author.len() > 200 { return Err("A local edition needs text (up to 256 KiB) and a short author label.".into()); }
            let bundled = guide(&req.id).and_then(|g| documents().get(&g.readme));
            let provenance = (e.source_revision == REVISION && bundled.is_some_and(|d| d.sha256 == e.source_sha256))
                || state.sources.iter().any(|s| s.revision == e.source_revision && s.document.sha256 == e.source_sha256);
            if !provenance { return Err("The source for this draft is not in the local library. Fetch it again before saving.".into()); }
            if state.editions.len() >= 50 { return Err("This guide already has 50 saved editions. Nothing was removed; export a backup before reorganizing the library.".into()); }
            state.editions.push(Edition { sha256: crate::live::sha256_hex(e.text.as_bytes()), text: e.text, author: e.author, source_revision: e.source_revision, source_sha256: e.source_sha256, at_ms: now() });
        }
        state.favorite = req.favorite; state.notes = req.notes; state.completed = req.completed;
        state.version = state.version.checked_add(1).ok_or("The guide's version limit was reached.")?;
        let result = state.clone();
        self.write(demo, &disk)?;
        Ok(result)
    }
    pub fn last_check(&self, demo: bool) -> Result<Option<source::Check>, String> { let _lock = self.gate.lock().unwrap(); Ok(self.load(demo)?.check) }
    pub fn keep_check(&self, demo: bool, check: source::Check) -> Result<(), String> {
        let _lock = self.gate.lock().unwrap(); let mut disk = self.load(demo)?;
        disk.check = Some(check); self.write(demo, &disk)
    }
    pub fn keep_source(&self, demo: bool, id: &str, fetched: Fetched) -> Result<LocalGuide, String> {
        let _lock = self.gate.lock().unwrap(); let mut disk = self.load(demo)?;
        if !valid_id(id) || fetched.document.path != format!("nvidia/{id}/README.md") { return Err("Invalid source path.".into()); }
        let local = disk.guides.entry(id.into()).or_default();
        if !local.sources.iter().any(|s| s.revision == fetched.revision) {
            if local.sources.len() >= 20 { return Err("This guide has 20 retained source revisions. Nothing was removed; keep a backup before reorganizing the library.".into()); }
            local.sources.push(fetched);
        }
        // Source fetching does not edit notes, checklists, local editions or their version.
        let result = local.clone(); self.write(demo, &disk)?; Ok(result)
    }
}
