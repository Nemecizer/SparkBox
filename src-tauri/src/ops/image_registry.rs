//! Anonymous, read-only registry checks from the Mac. Only the four roadmap repositories are
//! queried. Unknown digests, architectures and tag ordering remain unknown, never "up to date".
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

pub const REPOS: [&str; 4] = ["nvcr.io/nvidia/vllm", "vllm/vllm-openai", "eugr/spark-vllm", "eugr/spark-vllm-b12x"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageUpdate {
    pub image: String,
    pub checked_ms: u64,
    pub tag_moved: Option<bool>,
    pub newer_tag: Option<String>,
    pub remote_digest: Option<String>,
    pub error: Option<String>,
}
impl ImageUpdate {
    pub fn available(&self) -> bool { self.tag_moved == Some(true) || self.newer_tag.is_some() }
}

pub fn installed(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let repo = v["Repository"].as_str().unwrap_or("").trim_start_matches("docker.io/");
        let tag = v["Tag"].as_str().unwrap_or("");
        if REPOS.contains(&repo) && !tag.is_empty() && tag.len() < 128 && tag.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            && !out.iter().any(|x: &(String, String)| x.0 == repo && x.1 == tag) {
            out.push((repo.into(), tag.into()));
        }
        if out.len() == 32 { break; }
    }
    out
}

fn digest(s: &str) -> Option<String> {
    s.strip_prefix("sha256:").filter(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit())).map(|_| s.to_ascii_lowercase())
}

fn newer(candidate: &str, current: &str) -> bool {
    let parts = |s: &str| {
        let (version, suffix) = s.trim_start_matches('v').split_once('-').unwrap_or((s.trim_start_matches('v'), ""));
        let (numeric, post) = version.split_once(".post").unwrap_or((version, "0"));
        Some((numeric.split('.').map(str::parse::<u64>).collect::<Result<Vec<_>, _>>().ok()?, post.parse::<u64>().ok()?, suffix.to_string()))
    };
    matches!((parts(candidate), parts(current)), (Some((a, ap, x)), Some((b, bp, y))) if x == y && a.len() == b.len() && (&a, ap) > (&b, bp))
}

fn arm64(v: &Value) -> bool {
    v["images"].as_array().is_some_and(|a| a.iter().any(|p| p["architecture"] == "arm64" && p["os"] == "linux"))
        || v["manifests"].as_array().is_some_and(|a| a.iter().any(|p| p["platform"]["architecture"] == "arm64" && p["platform"]["os"] == "linux"))
}

async fn json(reply: reqwest::Response) -> Result<Value, String> {
    if !reply.status().is_success() { return Err(format!("Registry returned HTTP {}", reply.status().as_u16())); }
    let mut reply = reply;
    let mut bytes = Vec::new();
    while let Some(chunk) = reply.chunk().await.map_err(|_| "Registry response was interrupted")? {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 { return Err("Registry response exceeded 2 MiB".into()); }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Registry returned invalid JSON".into())
}

fn compare(mut report: ImageUpdate, digests: &[String], remote: Option<String>, newer_tag: Option<String>) -> ImageUpdate {
    report.tag_moved = remote.as_ref().filter(|_| !digests.is_empty()).map(|r| !digests.contains(r));
    report.remote_digest = remote;
    report.newer_tag = newer_tag;
    report
}

/// Endpoints are injected only by local tests; production uses these fixed HTTPS authorities.
pub async fn check(repo: &str, tag: &str, local: &[String], at_ms: u64, hub: &str, ngc: &str) -> ImageUpdate {
    let report = ImageUpdate { image: format!("{repo}:{tag}"), checked_ms: at_ms, ..Default::default() };
    let result = async {
        if !REPOS.contains(&repo) || !tag.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)) { return Err("Unsupported repository or tag".into()); }
        let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).user_agent("SparkBox").build().map_err(|_| "Could not create registry client")?;
        let get = |url: String| client.get(url);
        if repo.starts_with("nvcr.io/") {
            let token = json(get(format!("{ngc}/proxy_auth?scope=repository:nvidia/vllm:pull")).send().await.map_err(|_| "NGC token request failed")?).await?;
            let token = token["token"].as_str().filter(|t| !t.is_empty() && t.len() < 16384).ok_or("NGC returned no anonymous token")?;
            let accept = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json";
            let url = format!("{ngc}/v2/nvidia/vllm/manifests/{tag}");
            let head = client.head(&url).bearer_auth(token).header("Accept", accept).send().await.map_err(|_| "NGC manifest check failed")?;
            if !head.status().is_success() { return Err(format!("NGC returned HTTP {} for this tag", head.status().as_u16())); }
            let remote = head.headers().get("docker-content-digest").and_then(|h| h.to_str().ok()).and_then(digest);
            let current = json(get(url).bearer_auth(token).header("Accept", accept).send().await.map_err(|_| "NGC architecture check failed")?).await?;
            if !arm64(&current) { return Err("NGC did not confirm a linux/arm64 manifest for this tag".into()); }
            let tags = json(get(format!("{ngc}/v2/nvidia/vllm/tags/list?n=1000")).bearer_auth(token).send().await.map_err(|_| "NGC tag list failed")?).await?;
            let mut candidates: Vec<_> = tags["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|t| newer(t, tag) && t.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))).collect();
            candidates.sort_by(|a, b| if newer(a, b) { std::cmp::Ordering::Less } else if newer(b, a) { std::cmp::Ordering::Greater } else { a.cmp(b) });
            let mut candidate = None;
            for t in candidates.into_iter().take(5) {
                let manifest = json(get(format!("{ngc}/v2/nvidia/vllm/manifests/{t}")).bearer_auth(token).header("Accept", accept).send().await.map_err(|_| "NGC architecture check failed")?).await?;
                if arm64(&manifest) { candidate = Some(t.to_string()); break; }
            }
            Ok((remote, candidate))
        } else {
            let (namespace, repository) = repo.split_once('/').ok_or("Invalid repository")?;
            let url = format!("{hub}/v2/namespaces/{namespace}/repositories/{repository}/tags");
            let tags = json(get(format!("{url}?page_size=100")).send().await.map_err(|_| "Docker Hub tag list failed")?).await?;
            let entries = tags["results"].as_array().ok_or("Docker Hub returned no tag list")?;
            let current = match entries.iter().find(|v| v["name"].as_str() == Some(tag)) {
                Some(v) => v.clone(),
                None => json(get(format!("{url}/{tag}")).send().await.map_err(|_| "Docker Hub current-tag check failed")?).await?,
            };
            if !arm64(&current) { return Err("Registry did not confirm linux/arm64 for this tag".into()); }
            let mut candidates: Vec<_> = entries.iter().filter(|v| arm64(v)).filter_map(|v| v["name"].as_str()).filter(|t| newer(t, tag)).collect();
            candidates.sort_by(|a, b| if newer(a, b) { std::cmp::Ordering::Less } else if newer(b, a) { std::cmp::Ordering::Greater } else { a.cmp(b) });
            Ok((current["digest"].as_str().and_then(digest), candidates.first().map(|s| s.to_string())))
        }
    }.await;
    match result {
        Ok((remote, next)) => compare(report, local, remote, next),
        Err(error) => ImageUpdate { error: Some(error), ..report },
    }
}

pub fn repo_digests(text: &str, repo: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(text).unwrap_or_default().iter().filter_map(|s| {
        let (r, d) = s.split_once('@')?;
        (r.trim_start_matches("docker.io/") == repo).then(|| digest(d)).flatten()
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn registry(status:u16, bad_json:bool) -> (String, tokio::task::JoinHandle<()>) {
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let url=format!("http://{}",listener.local_addr().unwrap());
        let handle=tokio::spawn(async move { while let Ok((mut socket,_))=listener.accept().await {
            let mut bytes=[0u8;8192]; let n=socket.read(&mut bytes).await.unwrap_or(0); let req=String::from_utf8_lossy(&bytes[..n]);
            let body=if bad_json { "not json".into() } else if req.contains("/proxy_auth?") { r#"{"token":"anonymous-fixture-token"}"#.into() }
                else if req.contains("/tags/list") { r#"{"tags":["26.03.post1-py3","26.03.post2-py3","26.04-py3"]}"#.into() }
                else if req.contains("/manifests/") { r#"{"manifests":[{"platform":{"architecture":"arm64","os":"linux"}}]}"#.into() }
                else { serde_json::json!({"results":[
                    {"name":"v0.11.0","digest":format!("sha256:{}","a".repeat(64)),"images":[{"architecture":"arm64","os":"linux"}]},
                    {"name":"v0.12.0","images":[{"architecture":"arm64","os":"linux"}]},
                    {"name":"v0.13.0","images":[{"architecture":"amd64","os":"linux"}]},
                    {"name":"latest","images":[{"architecture":"arm64","os":"linux"}]}]}).to_string() };
            let body=if req.starts_with("HEAD ") {String::new()} else {body};
            let reply=format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nDocker-Content-Digest: sha256:{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}","a".repeat(64),body.len()); let _=socket.write_all(reply.as_bytes()).await;
        }}); (url,handle)
    }

    #[tokio::test]
    async fn local_hub_and_ngc_fixtures_filter_architecture_compare_digests_and_report_errors() {
        let (url, server)=registry(200,false).await;
        let same=format!("sha256:{}","a".repeat(64)); let old=format!("sha256:{}","b".repeat(64));
        let r=check(REPOS[1],"v0.11.0", &[same.clone()],7,&url,&url).await;
        assert_eq!(r.tag_moved,Some(false)); assert_eq!(r.newer_tag.as_deref(),Some("v0.12.0")); assert_eq!(r.checked_ms,7); assert!(r.error.is_none());
        let r=check(REPOS[0],"26.03.post1-py3", &[old],8,&url,&url).await;
        assert_eq!(r.tag_moved,Some(true)); assert_eq!(r.newer_tag.as_deref(),Some("26.04-py3")); assert_eq!(r.remote_digest,Some(same));
        let r=check(REPOS[1],"v0.11.0", &[],9,&url,&url).await; assert_eq!(r.tag_moved,None);
        server.abort();
        for (status,bad,expected) in [(429,false,"HTTP 429"),(200,true,"invalid JSON")] {
            let (url,server)=registry(status,bad).await; let r=check(REPOS[1],"v0.11.0", &[],10,&url,&url).await;
            assert!(r.error.as_deref().unwrap().contains(expected)); assert_eq!(r.tag_moved,None); assert!(!r.available()); server.abort();
        }
    }
    #[test]
    fn unknown_digests_and_unordered_tags_are_not_promoted_to_updates() {
        assert!(!newer("latest", "v0.11.0")); assert!(!newer("v0.12.0-rc1", "v0.11.0"));
        assert!(newer("26.06-py3", "26.05-py3"));
        assert!(newer("26.03.post2-py3", "26.03.post1-py3"));
        let r = compare(ImageUpdate::default(), &[], Some(format!("sha256:{}", "a".repeat(64))), None);
        assert_eq!(r.tag_moved, None); assert!(!r.available());
        assert!(!arm64(&serde_json::json!({"images":[{"architecture":"amd64","os":"linux"}]})));
        assert_eq!(installed("{\"Repository\":\"private/secret\",\"Tag\":\"v1\"}"), Vec::new());
    }
}
