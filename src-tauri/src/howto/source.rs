//! Public, anonymous, owner-requested reads from one fixed NVIDIA repository.
//! Pin the commit before reading a tree or file. Never follow redirects or execute assets.
use super::*;
use futures_util::StreamExt;
use sha1::{Digest, Sha1};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub id: String, pub status: String, pub tree_sha: String, pub readme_blob: String,
    pub published: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Check { pub revision: String, pub checked_at_ms: u64, pub demo: bool, pub changes: Vec<Change>, pub total: usize }
#[derive(Deserialize)]
struct Tree { truncated: bool, tree: Vec<Entry> }
#[derive(Deserialize)]
struct Entry { path: String, sha: String, #[serde(rename = "type")] kind: String }

pub fn sha(value: &str) -> bool { value.len() == 40 && value.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) }
pub fn blob(bytes: &[u8]) -> String {
    let mut hash = Sha1::new(); hash.update(format!("blob {}\0", bytes.len())); hash.update(bytes); hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn compare(revision: &str, tree: Tree, readme: &str) -> Result<Check, String> {
    if !sha(revision) || tree.truncated || tree.tree.len() > 10_000 { return Err("NVIDIA returned an incomplete or oversized source tree. The library was left unchanged.".into()); }
    let mut found = BTreeMap::new();
    for e in &tree.tree {
        if !sha(&e.sha) || e.path.len() > 2048 { return Err("The source tree contains invalid metadata.".into()); }
        let parts: Vec<_> = e.path.split('/').collect();
        if parts.len() == 3 && parts[0] == "nvidia" && parts[2] == "README.md" && e.kind == "blob" && valid_id(parts[1]) {
            let dir = format!("nvidia/{}", parts[1]);
            let parent = tree.tree.iter().find(|t| t.path == dir && t.kind == "tree").ok_or("A playbook folder is missing from the source tree.")?;
            let id = parts[1].to_owned();
            let published = readme.contains(&format!("nvidia/{id})")) || readme.contains(&format!("nvidia/{id}/"));
            let status = match guide(&id) { None => "added", Some(g) if g.tree_sha != parent.sha || g.published != published => "changed", _ => "unchanged" };
            if found.insert(id.clone(), Change { id, status: status.into(), tree_sha: parent.sha.clone(), readme_blob: e.sha.clone(), published }).is_some() { return Err("The source tree repeats a playbook.".into()); }
        }
    }
    if found.is_empty() || found.len() > 500 { return Err("The source tree did not contain a complete, bounded playbook list.".into()); }
    let total = found.len();
    for g in &catalog().guides { found.entry(g.id.clone()).or_insert_with(|| Change { id: g.id.clone(), status: "removed".into(), tree_sha: String::new(), readme_blob: String::new(), published: false }); }
    Ok(Check { revision: revision.into(), checked_at_ms: now(), demo: false, changes: found.into_values().collect(), total })
}
pub struct Client { client: reqwest::Client, api: String, raw: String }
impl Client {
    pub fn new() -> Result<Self, String> { Self::at("https://api.github.com/repos/NVIDIA/dgx-spark-playbooks", "https://raw.githubusercontent.com/NVIDIA/dgx-spark-playbooks") }
    fn at(api: &str, raw: &str) -> Result<Self, String> {
        Ok(Self { client: reqwest::Client::builder().timeout(Duration::from_secs(45)).connect_timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).user_agent("SparkBox-HowTo/1").build().map_err(|e| e.to_string())?, api: api.into(), raw: raw.into() })
    }
    async fn read(&self, url: &str, limit: usize) -> Result<Vec<u8>, String> {
        let res = self.client.get(url).send().await.map_err(|_| "The NVIDIA source check could not connect. Saved guides are still available offline.")?;
        if !res.status().is_success() { return Err(format!("The NVIDIA source request returned HTTP {}. No local edition changed; try again later if GitHub is rate-limiting requests.", res.status().as_u16())); }
        if res.content_length().is_some_and(|n| n > limit as u64) { return Err("The source response exceeds the download limit.".into()); }
        let mut bytes = Vec::new(); let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "The source download ended early. Nothing was replaced.")?;
            if bytes.len() + chunk.len() > limit { return Err("The source response exceeds the download limit.".into()); }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub async fn check(&self) -> Result<Check, String> {
        let head: serde_json::Value = serde_json::from_slice(&self.read(&format!("{}/commits/main", self.api), 256 * 1024).await?).map_err(|_| "GitHub returned an invalid revision.")?;
        let rev = head["sha"].as_str().filter(|s| sha(s)).ok_or("GitHub returned an invalid revision.")?;
        let tree: Tree = serde_json::from_slice(&self.read(&format!("{}/git/trees/{rev}?recursive=1", self.api), 8 * 1024 * 1024).await?).map_err(|_| "GitHub returned an invalid source tree.")?;
        if tree.truncated { return Err("GitHub returned a truncated tree. No comparison was saved.".into()); }
        let readme = self.read(&format!("{}/{rev}/README.md", self.raw), MAX_TEXT).await?;
        let expected = tree.tree.iter().find(|e| e.path == "README.md" && e.kind == "blob").ok_or("The source index is missing.")?;
        if blob(&readme) != expected.sha { return Err("The source index does not match the pinned revision.".into()); }
        compare(rev, tree, std::str::from_utf8(&readme).map_err(|_| "The source index is not UTF-8 text.")?)
    }
    pub async fn fetch(&self, check: &Check, id: &str) -> Result<Fetched, String> {
        if !sha(&check.revision) || !valid_id(id) || check.demo { return Err("Run a real source check before fetching this guide.".into()); }
        let entry = check.changes.iter().find(|c| c.id == id && c.status != "removed" && sha(&c.readme_blob) && sha(&c.tree_sha)).ok_or("That guide is not available at the checked revision.")?;
        let path = format!("nvidia/{id}/README.md");
        let bytes = self.read(&format!("{}/{}/{path}", self.raw, check.revision), MAX_TEXT).await?;
        if blob(&bytes) != entry.readme_blob { return Err("The downloaded guide does not match its pinned Git blob. Nothing was saved.".into()); }
        let text = String::from_utf8(bytes).map_err(|_| "The guide is not UTF-8 text.")?;
        if text.trim().is_empty() { return Err("The downloaded guide is empty.".into()); }
        Ok(Fetched { revision: check.revision.clone(), tree_sha: entry.tree_sha.clone(), at_ms: now(), document: Document { path, git_blob: entry.readme_blob.clone(), sha256: crate::live::sha256_hex(text.as_bytes()), text } })
    }
}

pub fn demo_check() -> Check {
    Check { revision: "dddddddddddddddddddddddddddddddddddddddd".into(), checked_at_ms: now(), demo: true, total: catalog().guides.len(), changes: catalog().guides.iter().map(|g| Change { id: g.id.clone(), status: if g.id == "playbook-vllm" { "changed" } else { "unchanged" }.into(), tree_sha: g.tree_sha.clone(), readme_blob: documents()[&g.readme].git_blob.clone(), published: g.published }).collect() }
}
pub fn demo_fetch(id: &str) -> Result<Fetched, String> {
    let g = guide(id).ok_or("Choose a bundled Demo guide.")?;
    let text = format!("{}\n\n## Demo source update\n\nThis is a simulated documentation revision. No NVIDIA request or node operation was made.\n", documents()[&g.readme].text);
    Ok(Fetched { revision: demo_check().revision, tree_sha: g.tree_sha.clone(), at_ms: now(), document: Document { path: g.readme.clone(), git_blob: blob(text.as_bytes()), sha256: crate::live::sha256_hex(text.as_bytes()), text } })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_index_and_complete_tree_compare_without_false_changes() {
        let mut entries = Vec::new();
        for g in &catalog().guides {
            entries.push(Entry { path: format!("nvidia/{}", g.id), sha: g.tree_sha.clone(), kind: "tree".into() });
            entries.push(Entry { path: g.readme.clone(), sha: documents()[&g.readme].git_blob.clone(), kind: "blob".into() });
        }
        let checked = compare(REVISION, Tree { truncated: false, tree: entries }, &documents()["README.md"].text).unwrap();
        assert_eq!(checked.total, 136);
        assert_eq!(checked.changes.iter().filter(|c| c.published).count(), 69);
        assert!(checked.changes.iter().all(|c| c.status == "unchanged"));
    }
    #[test]
    fn blob_identity_and_bounded_complete_tree() {
        assert_eq!(blob(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
        let make = |truncated| Tree { truncated, tree: vec![Entry { path: "nvidia/new-guide".into(), sha: "a".repeat(40), kind: "tree".into() }, Entry { path: "nvidia/new-guide/README.md".into(), sha: "b".repeat(40), kind: "blob".into() }] };
        let c = compare(REVISION, make(false), "[New](nvidia/new-guide)").unwrap();
        assert_eq!(c.total, 1); assert_eq!(c.changes.iter().filter(|c| c.status == "removed").count(), 136);
        assert!(c.changes.iter().any(|c| c.id == "new-guide" && c.published && c.status == "added"));
        assert!(compare(REVISION, make(true), "").is_err());
        assert!(compare("../../main", make(false), "").is_err());
    }
    #[tokio::test]
    async fn rejects_unchecked_fetch_before_network() {
        let c = Client::new().unwrap(); let checked = demo_check();
        assert!(c.fetch(&checked, "playbook-vllm").await.is_err());
        let mut checked = checked; checked.demo = false;
        assert!(c.fetch(&checked, "../../secret").await.is_err());
        assert!(c.fetch(&checked, "unknown").await.is_err());
    }
    #[tokio::test]
    async fn pinned_fetch_checks_hash_and_never_follows_redirects_or_sends_credentials() {
        use axum::{body::Body, http::{Request, StatusCode}, response::IntoResponse, routing::get, Router};
        use std::sync::{Arc, Mutex};
        let text = "# A new NVIDIA guide\n\nReview before use.\n";
        let root = "[New](nvidia/new-guide)\n";
        let tree = serde_json::json!({"truncated": false, "tree": [
            {"path": "README.md", "sha": blob(root.as_bytes()), "type": "blob"},
            {"path": "nvidia/new-guide", "sha": "a".repeat(40), "type": "tree"},
            {"path": "nvidia/new-guide/README.md", "sha": blob(text.as_bytes()), "type": "blob"}
        ]});
        let seen = Arc::new(Mutex::new(Vec::new())); let requests = seen.clone();
        let app = Router::new().fallback(get(move |req: Request<Body>| {
            let tree = tree.clone(); let requests = requests.clone();
            async move {
                assert!(req.headers().get("authorization").is_none() && req.headers().get("cookie").is_none());
                let path = req.uri().path().to_string(); requests.lock().unwrap().push(path.clone());
                if path == "/api/commits/main" { return axum::Json(serde_json::json!({"sha": REVISION})).into_response(); }
                if path == format!("/api/git/trees/{REVISION}") { return axum::Json(tree).into_response(); }
                if path == format!("/raw/{REVISION}/README.md") { return root.into_response(); }
                if path == format!("/raw/{REVISION}/nvidia/new-guide/README.md") { return text.into_response(); }
                if path == "/redirect" { return (StatusCode::FOUND, [("location", "/must-not-follow")], "").into_response(); }
                if path == "/large" { return "x".repeat(100).into_response(); }
                (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let c = Client::at(&format!("http://{addr}/api"), &format!("http://{addr}/raw")).unwrap();
        let check = c.check().await.unwrap(); let fetched = c.fetch(&check, "new-guide").await.unwrap();
        assert_eq!(fetched.document.text, text); assert_eq!(fetched.revision, REVISION);
        let mut changed = check; changed.changes.iter_mut().find(|g| g.id == "new-guide").unwrap().readme_blob = "0".repeat(40);
        assert!(c.fetch(&changed, "new-guide").await.unwrap_err().contains("Git blob"));
        assert!(c.read(&format!("http://{addr}/redirect"), 1024).await.unwrap_err().contains("302"));
        assert!(c.read(&format!("http://{addr}/limited"), 1024).await.unwrap_err().contains("429"));
        assert!(c.read(&format!("http://{addr}/large"), 8).await.is_err());
        assert!(!seen.lock().unwrap().iter().any(|p| p == "/must-not-follow"));
        assert!(seen.lock().unwrap().iter().filter(|p| p.starts_with("/raw/")).all(|p| p.contains(REVISION)));
        server.abort();
    }
}
