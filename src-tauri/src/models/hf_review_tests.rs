//! No Hub or node connection: immutable review checks and a local HTTP fixture.
use super::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

fn sample() -> (HfModel, DownloadSelection) {
    let model = parse_model(&json!({"id":"fixture/model", "sha":"a".repeat(40), "siblings":[
        {"rfilename":"model.safetensors", "size":100, "lfs":{"sha256":"c".repeat(64)}},
        {"rfilename":"config.json", "size":10}
    ]}), json!({}), None, "");
    let selection = DownloadSelection { repo: model.id.clone(), revision: model.revision.clone(), files: model.files.clone() };
    (model, selection)
}

#[test]
fn review_compares_repo_commit_and_complete_manifest_order_independently() {
    let (model, selection) = sample();
    selection.check(&model).unwrap();
    let mut reordered = model.clone(); reordered.files.reverse();
    selection.check(&reordered).unwrap();
    for change in 0..7 {
        let mut changed = model.clone();
        match change {
            0 => changed.id = "fixture/other".into(),
            1 => changed.revision = "b".repeat(40),
            2 => { changed.files.pop(); },
            3 => changed.files.push(HfFile { path: "tokenizer.json".into(), size: Some(10), sha256: None }),
            4 => changed.files[0].size = Some(11),
            5 => changed.files[0].sha256 = Some("d".repeat(64)),
            _ => changed.files[0].path = "other.json".into(),
        }
        assert!(selection.check(&changed).unwrap_err().contains("no longer matches"), "change {change}");
    }
    let mut failed = model; failed.config_error = Some("config.json (HTTP 403)".into());
    assert!(selection.check(&failed).unwrap_err().contains("403"));
}

#[test]
fn review_rejects_missing_fields_moving_revisions_patterns_and_unknown_sizes() {
    let (_, selection) = sample();
    let mut value = serde_json::to_value(&selection).unwrap();
    value.as_object_mut().unwrap().remove("files");
    assert!(serde_json::from_value::<DownloadSelection>(value).is_err());
    for revision in ["main", "v1.0", "abc123", "", &"A".repeat(40)] {
        let mut bad = selection.clone(); bad.revision = revision.into(); assert!(bad.validate().is_err());
    }
    for path in ["*.safetensors", "foo?.bin", "a[0].bin", "../x", "a/../x", "/x", "a//x", "a\\b", "a\n.json"] {
        let mut bad = selection.clone(); bad.files[0].path = path.into(); assert!(bad.validate().is_err(), "{path}");
    }
    let mut bad = selection.clone(); bad.files[0].size = None; assert!(bad.validate().is_err());
    let mut bad = selection.clone(); bad.files.push(bad.files[0].clone()); assert!(bad.validate().is_err());
    let mut bad = selection; bad.repo = "fixture/model?token=x".into(); assert!(bad.validate().is_err());
}

#[test]
fn refs_keep_branch_and_tag_identity_and_encode_revision_as_one_segment() {
    let refs = parse_refs(&json!({"branches":[{"name":"release/test", "targetCommit":"a".repeat(40)}],
        "tags":[{"name":"release/test", "targetCommit":"b".repeat(40)}], "converts":[]})).unwrap();
    assert_eq!((refs[0].kind.as_str(), refs[1].kind.as_str()), ("branch", "tag"));
    assert_ne!(refs[0].revision, refs[1].revision);
    assert!(parse_refs(&json!({"branches":[{"name":"main", "targetCommit":"main"}],"tags":[]})).is_err());
    let url = model_url(HUB, "fixture/model", "release/test?blobs=false#x").unwrap();
    assert_eq!(url.path(), "/api/models/fixture/model/revision/release%2Ftest%3Fblobs=false%23x");
    assert_eq!(url.query(), Some("blobs=true"));
    assert!(model_url(HUB, "fixture/model", "..").is_err());
}

struct HubFixture { url: String, seen: Arc<Mutex<Vec<String>>>, task: tokio::task::JoinHandle<()> }
impl Drop for HubFixture { fn drop(&mut self) { self.task.abort(); } }

async fn hub(config_status: u16, metadata: Value, index: Option<(u16, Value)>) -> HubFixture {
    use axum::{extract::OriginalUri, http::StatusCode, routing::get, Router};
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = seen.clone();
    let app = Router::new().fallback(get(move |OriginalUri(uri): OriginalUri| {
        let (metadata, requests, index) = (metadata.clone(), requests.clone(), index.clone());
        async move {
            requests.lock().unwrap().push(uri.to_string());
            let path = uri.path();
            if path.starts_with("/api/models/") { return (StatusCode::OK, metadata.to_string()); }
            if path.ends_with("config.json") { return (StatusCode::from_u16(config_status).unwrap(), "{\"num_hidden_layers\":1}".into()); }
            if path.ends_with("model.safetensors.index.json") {
                let (status, body) = index.unwrap(); return (StatusCode::from_u16(status).unwrap(), body.to_string());
            }
            (StatusCode::NOT_FOUND, String::new()) // README is optional.
        }
    }));
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", socket.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(socket, app).await.unwrap(); });
    HubFixture { url, seen, task }
}

fn metadata() -> Value { json!({"id":"fixture/model","sha":"a".repeat(40),"siblings":[{"rfilename":"model.safetensors","size":100}]}) }

#[tokio::test]
async fn detail_resolves_branch_once_and_reads_config_and_card_at_commit() {
    let fixture = hub(200, metadata(), None).await;
    let model = model_from_hub(&reqwest::Client::new(), None, "fixture/model", "release/test", &fixture.url).await.unwrap();
    assert_eq!(model.revision, "a".repeat(40)); assert!(model.config_error.is_none());
    assert_eq!(fixture.seen.lock().unwrap().as_slice(), [
        "/api/models/fixture/model/revision/release%2Ftest?blobs=true".into(),
        format!("/fixture/model/resolve/{}/config.json", "a".repeat(40)),
        format!("/fixture/model/resolve/{}/README.md", "a".repeat(40)),
    ]);
}

#[tokio::test]
async fn config_errors_remain_visible_and_never_admit_a_download() {
    for status in [401, 403, 404, 429, 500] {
        let fixture = hub(status, metadata(), None).await;
        let model = model_from_hub(&reqwest::Client::new(), None, "fixture/model", "main", &fixture.url).await.unwrap();
        assert!(model.config_error.as_ref().unwrap().contains(&format!("HTTP {status}")));
        let review = DownloadSelection { repo: model.id.clone(), revision: model.revision.clone(), files: model.files.clone() };
        assert!(review.check(&model).is_err());
    }
}

#[tokio::test]
async fn missing_or_substituted_commits_and_unreadable_index_have_no_fallback() {
    let mut missing = metadata(); missing.as_object_mut().unwrap().remove("sha");
    let fixture = hub(200, missing, None).await;
    assert!(model_from_hub(&reqwest::Client::new(), None, "fixture/model", "main", &fixture.url).await.unwrap_err().contains("full commit"));
    assert_eq!(fixture.seen.lock().unwrap().len(), 1);
    let fixture = hub(200, metadata(), None).await;
    assert!(model_from_hub(&reqwest::Client::new(), None, "fixture/model", &"b".repeat(40), &fixture.url).await.unwrap_err().contains("different commit"));
    let mut indexed = metadata(); indexed["siblings"].as_array_mut().unwrap().push(json!({"rfilename":"model.safetensors.index.json","size":10}));
    for response in [(403, json!({})), (200, json!({})), (200, json!({"weight_map":{"x":"missing.safetensors"}}))] {
        let fixture = hub(200, indexed.clone(), Some(response)).await;
        assert!(model_from_hub(&reqwest::Client::new(), None, "fixture/model", "main", &fixture.url).await.is_err());
    }
}
