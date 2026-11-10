//! Managing an Ollama server on a node: what it has (`/api/tags`), what's in
//! memory (`/api/ps`), pulling a model (a tracked task with progress from
//! `/api/pull`'s stream) and deleting one. Everything goes through `curl` on
//! the node itself, so it works when Ollama only listens on localhost.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use super::metrics::{parse_ollama_ps, rfc3339_secs, OllamaLoaded};
use crate::live::sh_quote;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OllamaModel {
    pub name: String,
    pub size_bytes: u64,
    /// Unix seconds.
    pub modified_at: Option<u64>,
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    /// In memory right now, from /api/ps.
    pub loaded: Option<OllamaLoaded>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaView {
    pub endpoint: String,
    pub node: String,
    pub port: u16,
    pub version: Option<String>,
    pub models: Vec<OllamaModel>,
}

/// Ollama model names: `name[:tag]`, optionally `namespace/` or a registry host in front.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-:/".contains(c))
        && !name.contains("..")
}

const SEP: &str = "--sparkbox--";

/// One round trip: tags, what's loaded, and the version.
pub fn list_command(port: u16) -> String {
    let get = |path: &str| format!("curl -s -m 5 http://127.0.0.1:{port}{path}");
    format!("{}; printf '\\n{SEP}\\n'; {}; printf '\\n{SEP}\\n'; {}", get("/api/tags"), get("/api/ps"), get("/api/version"))
}

pub fn pull_command(port: u16, name: &str) -> String {
    let body = json!({ "model": name, "stream": true }).to_string();
    format!("curl -sN --fail-with-body http://127.0.0.1:{port}/api/pull -d {}", sh_quote(&body))
}

pub fn delete_command(port: u16, name: &str) -> String {
    let body = json!({ "model": name }).to_string();
    format!("curl -s -m 30 -w '\\n%{{http_code}}' -X DELETE http://127.0.0.1:{port}/api/delete -d {}", sh_quote(&body))
}

pub fn parse_list(out: &str) -> (Vec<OllamaModel>, Option<String>) {
    let sep = format!("\n{SEP}\n");
    let mut parts = out.split(&sep);
    let tags: Value = serde_json::from_str(parts.next().unwrap_or("").trim()).unwrap_or(Value::Null);
    let ps: Value = serde_json::from_str(parts.next().unwrap_or("").trim()).unwrap_or(Value::Null);
    let version: Value = serde_json::from_str(parts.next().unwrap_or("").trim()).unwrap_or(Value::Null);
    let loaded = parse_ollama_ps(&ps);
    let mut models: Vec<OllamaModel> = tags["models"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let name = m["name"].as_str().or(m["model"].as_str()).unwrap_or_default().to_string();
            OllamaModel {
                loaded: loaded.iter().find(|l| l.name == name).cloned(),
                size_bytes: m["size"].as_u64().unwrap_or(0),
                modified_at: m["modified_at"].as_str().and_then(rfc3339_secs),
                family: m["details"]["family"].as_str().map(String::from),
                parameter_size: m["details"]["parameter_size"].as_str().map(String::from),
                quantization: m["details"]["quantization_level"].as_str().map(String::from),
                name,
            }
        })
        .collect();
    models.sort_by(|a, b| b.loaded.is_some().cmp(&a.loaded.is_some()).then(a.name.cmp(&b.name)));
    (models, version["version"].as_str().map(String::from))
}

/// Progress from one line of /api/pull's stream: (status, completed, total).
pub fn parse_pull_line(line: &str) -> Option<(String, u64, u64)> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if let Some(e) = v["error"].as_str() {
        return Some((format!("error: {e}"), 0, 0));
    }
    Some((v["status"].as_str().unwrap_or("").to_string(), v["completed"].as_u64().unwrap_or(0), v["total"].as_u64().unwrap_or(0)))
}

/// Rough download sizes (Q4) for models the Demo can pull, by name.
pub fn demo_size(name: &str) -> u64 {
    match name.split(':').next().unwrap_or(name) {
        "llama3.2" => 2_019_393_189,
        "qwen3" if name.ends_with(":32b") => 20_201_253_588,
        "qwen3" => 5_225_387_923,
        "gemma3" => 17_396_936_941,
        "gpt-oss" if name.ends_with(":120b") => 65_290_185_889,
        "gpt-oss" => 13_793_437_013,
        "nomic-embed-text" => 274_302_450,
        "mistral-small3.2" => 15_035_496_448,
        _ => 4_920_753_328,
    }
}

/// How fast the Demo pulls (a good home connection).
pub const DEMO_PULL_BYTES_PER_S: f64 = 110e6;
/// Longest a real pull may take.
pub const PULL_LIMIT: Duration = Duration::from_secs(6 * 3600);

// ---- Running it

use std::sync::Arc;

use super::endpoints::Engine;
use super::{now, Ctx, Models};
use crate::live::{CancelToken, StreamKind};
use crate::tasks::{TaskSpec, Tasks};

impl Models {
    fn ollama_endpoint(&self, id: &str) -> Result<super::endpoints::Endpoint, String> {
        self.endpoints()
            .into_iter()
            .find(|e| e.id == id && e.engine == Engine::Ollama)
            .ok_or_else(|| "That Ollama server isn't running any more.".to_string())
    }

    pub async fn ollama_list(&self, ctx: &Ctx, id: &str) -> Result<OllamaView, String> {
        let e = self.ollama_endpoint(id)?;
        let (models, version) = if ctx.live {
            parse_list(&self.poll(&e.head_node, &list_command(e.port), Duration::from_secs(20)).await?)
        } else if e.members.is_empty() {
            // A service Ollama, as found on cell1 and cell3: its tags, nothing in memory.
            let tags = super::watch::demo_service_tags(&e.head_node);
            let models = tags
                .into_iter()
                .map(|(name, size_bytes)| OllamaModel {
                    family: name.split(':').next().map(|f| f.trim_end_matches(char::is_numeric).to_string()),
                    parameter_size: Some("8.0B".into()),
                    quantization: Some("Q4_K_M".into()),
                    // Pulled Apr 18, 2026.
                    modified_at: Some(1_776_528_000),
                    name,
                    size_bytes,
                    loaded: None,
                })
                .collect();
            (models, Some("0.21.0".to_string()))
        } else {
            (self.demo.lock().unwrap().ollama_models(), Some("0.12.6".to_string()))
        };
        Ok(OllamaView { endpoint: e.id, node: e.head_node, port: e.port, version, models })
    }

    /// Pulls a model as a tracked task; returns the task's id.
    pub async fn ollama_pull(self: &Arc<Self>, ctx: &Ctx, id: &str, name: &str, tasks: Arc<Tasks>) -> Result<String, String> {
        if !valid_name(name) {
            return Err(format!("“{name}” isn't an Ollama model name like qwen3:8b."));
        }
        let e = self.ollama_endpoint(id)?;
        // Ollama does the download; look-only leaves its stream open, so the pull keeps running there.
        let task = tasks.start(TaskSpec::new("ollama-pull", format!("Pull {name} on {}", e.head_node), std::slice::from_ref(&e.head_node)).cancellable().on_node());
        task.set_retry("ollama_pull", json!({ "endpoint": id, "name": name }));
        let task_id = task.id().to_string();
        let me = self.clone();
        let name = name.to_string();
        if !ctx.live {
            tauri::async_runtime::spawn(async move {
                let size = demo_size(&name);
                let secs = (size as f64 / DEMO_PULL_BYTES_PER_S).clamp(6.0, 45.0);
                task.step(format!("pulling {name}"));
                let started = std::time::Instant::now();
                loop {
                    if task.is_cancelled() {
                        return task.finish_cancelled("Stopped; the partial download stays in Ollama's cache.");
                    }
                    let frac = (started.elapsed().as_secs_f64() / secs).min(1.0);
                    task.progress_rate(frac as f32, Some(format!("{name}: {:.1} of {:.1} GB", frac * size as f64 / 1e9, size as f64 / 1e9)), Some(size as f64 / secs), Some(secs * (1.0 - frac)));
                    if frac >= 1.0 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                task.step("verifying sha256 digest");
                tokio::time::sleep(Duration::from_millis(600)).await;
                me.demo.lock().unwrap().ollama_add(&name, size, now());
                me.live.audit.record_simulated(&e.head_node, &format!("Pull {name} into Ollama"), &pull_command(e.port, &name));
                task.finish_ok(format!("{name} is ready on {}", e.head_node));
            });
            return Ok(task_id);
        }
        tauri::async_runtime::spawn(async move {
            let cancel = CancelToken::new();
            let stop = {
                let (cancel, wait) = (cancel.clone(), task.cancelled());
                tokio::spawn(async move {
                    wait.await;
                    cancel.cancel();
                })
            };
            task.step(format!("pulling {name}"));
            let mut buf = String::new();
            let mut failure: Option<String> = None;
            let t = task.clone();
            let result = me
                .live
                .stream_cmd(&e.head_node, &pull_command(e.port, &name), PULL_LIMIT, cancel, |kind, data| {
                    if kind != StreamKind::Stdout {
                        return;
                    }
                    buf.push_str(&String::from_utf8_lossy(data));
                    while let Some(i) = buf.find('\n') {
                        let line: String = buf.drain(..=i).collect();
                        match parse_pull_line(&line) {
                            Some((status, _, _)) if status.starts_with("error: ") => failure = Some(status[7..].to_string()),
                            Some((status, done, total)) if total > 0 => {
                                t.progress(done as f32 / total as f32, Some(format!("{status}: {:.1} of {:.1} GB", done as f64 / 1e9, total as f64 / 1e9)))
                            }
                            Some((status, _, _)) if !status.is_empty() => t.step(status),
                            _ => {}
                        }
                    }
                })
                .await;
            stop.abort();
            match (result, failure) {
                (_, Some(f)) => task.finish_err(format!("Ollama couldn't pull {name}: {f}")),
                (Ok(o), None) if o.cancelled => task.finish_cancelled("Stopped; the partial download stays in Ollama's cache."),
                (Ok(o), None) if o.code == Some(0) => task.finish_ok(format!("{name} is ready on {}", e.head_node)),
                (Ok(o), None) => task.finish_err(format!("The pull stopped (exit {}).", o.code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()))),
                (Err(err), None) => task.finish_err(err),
            }
        });
        Ok(task_id)
    }

    pub async fn ollama_delete(&self, ctx: &Ctx, id: &str, name: &str) -> Result<(), String> {
        if !valid_name(name) {
            return Err("bad model name".into());
        }
        let e = self.ollama_endpoint(id)?;
        if !ctx.live {
            self.demo.lock().unwrap().ollama_delete(name)?;
            self.live.audit.record_simulated(&e.head_node, &format!("Delete {name} from Ollama"), &delete_command(e.port, name));
            return Ok(());
        }
        let out = self.live.run(&e.head_node, &delete_command(e.port, name), None, Duration::from_secs(40)).await?;
        let (body, code) = out.stdout.trim_end().rsplit_once('\n').map(|(b, c)| (b.to_string(), c.trim().to_string())).unwrap_or_default();
        match code.as_str() {
            "200" => Ok(()),
            "404" => Err(format!("{name} isn't on {} any more.", e.head_node)),
            _ => Err(format!("Ollama refused to delete {name}: {}", serde_json::from_str::<Value>(&body).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or(body))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_and_quoted() {
        for ok in ["qwen3:32b", "gpt-oss:20b", "hf.co/unsloth/Qwen3-8B-GGUF:Q4_K_M", "library/llama3.2"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-rm", "a b", "x;rm -rf /", "x'$(id)'", "../../etc"] {
            assert!(!valid_name(bad), "{bad}");
        }
        let cmd = pull_command(11434, "qwen3:8b");
        assert_eq!(cmd, "curl -sN --fail-with-body http://127.0.0.1:11434/api/pull -d '{\"model\":\"qwen3:8b\",\"stream\":true}'");
        assert!(delete_command(11434, "qwen3:8b").contains("-X DELETE http://127.0.0.1:11434/api/delete -d '{\"model\":\"qwen3:8b\"}'"));
    }

    #[test]
    fn the_list_joins_tags_and_whats_loaded() {
        let out = format!(
            "{}\n{SEP}\n{}\n{SEP}\n{}",
            r#"{"models":[{"name":"qwen3:32b","size":20201253588,"modified_at":"2026-10-01T09:00:00Z","details":{"family":"qwen3","parameter_size":"32.8B","quantization_level":"Q4_K_M"}},
                {"name":"nomic-embed-text:latest","size":274302450,"details":{"family":"nomic-bert","parameter_size":"137M","quantization_level":"F16"}}]}"#,
            r#"{"models":[{"name":"qwen3:32b","size":29960000000,"size_vram":29960000000,"expires_at":"2026-10-28T14:20:00Z"}]}"#,
            r#"{"version":"0.12.6"}"#
        );
        let (models, version) = parse_list(&out);
        assert_eq!(version.as_deref(), Some("0.12.6"));
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "qwen3:32b", "loaded first");
        assert_eq!(models[0].loaded.as_ref().unwrap().vram_bytes, 29_960_000_000);
        assert_eq!(models[0].quantization.as_deref(), Some("Q4_K_M"));
        assert!(models[1].loaded.is_none());
        assert_eq!(parse_pull_line(r#"{"status":"pulling 6a0746a1ec1a","digest":"sha256:x","total":1000,"completed":250}"#), Some(("pulling 6a0746a1ec1a".into(), 250, 1000)));
        assert_eq!(parse_pull_line(r#"{"error":"pull model manifest: file does not exist"}"#).unwrap().0, "error: pull model manifest: file does not exist");
    }

    #[tokio::test]
    async fn turning_look_only_on_leaves_a_pull_running_on_its_node_and_says_so() {
        use std::sync::Mutex;
        let dir = crate::fake_node::temp("ollama-hold");
        let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
        let live = crate::live::Live::new(vpn.clone(), dir.join("known_hosts.json"), || Ok(None));
        let llm = crate::llm::Llm::new(dir.join("providers.json"), vpn);
        let cluster = Arc::new(Mutex::new(crate::demo::DemoCluster::new(&crate::inventory::Inventory::example())));
        let models = Models::new(dir.clone(), live, llm, Some(cluster.clone()));
        let ctx = Ctx { live: false, settings: Default::default(), inventory: Default::default(), snapshot: Some(cluster.lock().unwrap().snapshot()) };
        let eps = models.endpoints_tick(&ctx, false).await;
        let id = eps.iter().find(|e| e.engine == Engine::Ollama).expect("the demo cluster's Ollama").id.clone();
        let tasks = Tasks::new(None);
        let task = models.ollama_pull(&ctx, &id, "qwen3:32b", tasks.clone()).await.unwrap();
        for _ in 0..100 {
            if tasks.get(&task).is_some_and(|t| t.progress.is_some()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let held = tasks.hold_non_read("Look-only turned on");
        assert_eq!((held.stopped, held.on_node), (0, 1), "Ollama does the download, so it isn't claimed stopped");
        let t = tasks.get(&task).unwrap();
        assert!(t.on_node && !t.cancel_requested && !t.finished(), "{t:?}");
        assert!(t.held.as_deref().is_some_and(|h| h.contains("keeps running")), "{:?}", t.held);
    }
}
