//! Models end to end against fake Sparks: real SSH servers (russh) that run the
//! real sparkjob.py in a temporary home, with docker, curl and anything that
//! needs a GB10 answered from a script. The AI analysis runs against the fake
//! Anthropic/OpenAI upstream.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::*;
use crate::demo::DemoCluster;
use crate::fake_node::{fake_node, real_shell, temp, Script};
use crate::fake_upstream;
use crate::vpn::Vpn;

/// A small model folder the way sparkjob.py's scan recognises one.
fn model_folder(home: &Path, repo: &str, manifest: bool) -> String {
    let dir = home.join("models").join(repo);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), "{\"architectures\": [\"Qwen3ForCausalLM\"]}").unwrap();
    std::fs::write(dir.join("model.safetensors"), vec![7u8; 4096]).unwrap();
    if manifest {
        std::fs::write(
            dir.join(".sparkbox.json"),
            json!({ "repo": repo, "revision": "abc123", "files": [], "verifiedAt": 1 }).to_string(),
        )
        .unwrap();
    }
    dir.display().to_string()
}

struct Cluster {
    models: Arc<Models>,
    live: Arc<Live>,
    llm: Arc<Llm>,
    inventory: Inventory,
    dir: PathBuf,
}

impl Cluster {
    fn ctx(&self) -> Ctx {
        Ctx {
            live: true,
            settings: ModelSettings { model_root: "~/models".into(), ..Default::default() },
            inventory: self.inventory.clone(),
            snapshot: Some(self.live.snapshot(&self.inventory)),
        }
    }
}

async fn cluster(name: &str, ports: &[(&str, u16)]) -> Cluster {
    let dir = temp(&format!("models-{name}-app"));
    let (live, inventory) = crate::fake_node::connect(&dir, ports).await;
    let vpn = Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {});
    let llm = Llm::new(dir.join("providers.json"), vpn);
    let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
    Cluster { models, live, llm, inventory, dir }
}

fn check_reply(n: u8) -> String {
    json!({
        "home": "/home/sparkuser", "rootPath": "/home/sparkuser/models", "rootExists": true, "freeBytes": 2_000_000_000_000u64,
        "docker": true, "rsync": true, "python": "3.12.3", "imagePresent": true, "rayImagePresent": true, "vllmVersion": "",
        "fabric": [
            { "iface": "enP2p1s0f1np1", "ip": format!("192.168.101.{n}"), "prefix": 24, "speedMbps": 200000, "rdma": "roceP2p1s0f1" },
            { "iface": "enp1s0f1np1", "ip": format!("192.168.100.{n}"), "prefix": 24, "speedMbps": 200000, "rdma": "rocep1s0f1" },
        ],
    })
    .to_string()
}

// ---- sparkjob.py over SSH

#[tokio::test]
async fn howto_prerequisites_only_read_quick_facts_and_preserve_errors() {
    let home = temp("howto-check-home");
    let (node, port) = fake_node(&home, real_shell()).await;
    let c = cluster("howto-check", &[("cell1", port)]).await;
    let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
    gate.set_look_only(true).unwrap(); c.live.set_write_policy(gate);
    let checks = c.models.howto_checks(&c.ctx(), &["cell1".into()]).await.unwrap();
    assert_eq!(checks.len(), 1); assert!(checks[0].error.is_none(), "{:?}", checks[0].error);
    assert!(checks[0].gpu_in_container.is_none());
    let ran = node.ran("sparkjob.py check"); assert_eq!(ran.len(), 1); assert!(ran[0].ends_with(" quick"));
    assert!(node.ran("docker run").is_empty());
    c.live.stop();
    let failed = c.models.howto_checks(&c.ctx(), &["cell1".into()]).await.unwrap();
    assert!(failed[0].error.is_some()); assert!(!failed[0].docker);
}

#[tokio::test]
async fn the_library_merges_copies_of_the_same_folder_across_nodes() {
    let home = temp("library-home");
    let key = model_folder(&home, "Qwen/Qwen3.8-27B-FP8", true);
    model_folder(&home, "zai-org/GLM-test", false);
    let (a, pa) = fake_node(&home, real_shell()).await;
    let (b, pb) = fake_node(&home, real_shell()).await;
    let c = cluster("library", &[("cell1", pa), ("cell2", pb)]).await;

    let lib = c.models.library(&c.ctx()).await.unwrap();
    assert_eq!(lib.len(), 2, "{lib:?}");
    let qwen = lib.iter().find(|e| e.key == key).unwrap();
    assert_eq!(qwen.repo, "Qwen/Qwen3.8-27B-FP8");
    assert_eq!(qwen.revision, "abc123");
    assert_eq!(qwen.copies.keys().collect::<Vec<_>>(), ["cell1", "cell2"]);
    assert!(qwen.copies["cell1"].verified);
    assert!(qwen.size_bytes >= 4096);
    let glm = lib.iter().find(|e| e.repo == "zai-org/GLM-test").unwrap();
    assert!(!glm.copies["cell1"].verified, "no manifest yet: found, not verified");

    // sparkjob.py was copied, then reused while its checksum matches.
    let installed = std::fs::read_to_string(home.join(".sparkbox/sparkjob.py")).unwrap();
    assert_eq!(installed, SPARKJOB);
    let uploads = || {
        [&a, &b].iter().map(|n| n.log.lock().unwrap().iter().filter(|cmd| cmd.contains("cat > ~/.sparkbox/sparkjob.py")).count()).sum::<usize>()
    };
    let first = uploads();
    assert!(first >= 1);
    c.models.library(&c.ctx()).await.unwrap();
    assert_eq!(uploads(), first);
    c.live.stop();
}

#[tokio::test]
async fn check_reports_readiness_and_delete_stays_inside_the_model_folder() {
    let home = temp("check-home");
    let key = model_folder(&home, "Qwen/Qwen3.8-27B-FP8", true);
    let (_a, pa) = fake_node(&home, real_shell()).await;
    let c = cluster("check", &[("cell1", pa)]).await;

    // A Mac has no docker or ConnectX-7; the check still answers with what it found.
    let checks = c.models.check(&c.ctx()).await;
    assert_eq!(checks.len(), 1);
    let n = &checks[0];
    assert!(n.error.is_none(), "{:?}", n.error);
    assert_eq!(n.home, home.display().to_string());
    assert!(n.root_exists);
    assert!(n.free_bytes > 0);
    assert!(n.fabric.is_empty());

    c.models.delete(&c.ctx(), &key, &["cell1".into()]).await.unwrap();
    assert!(!Path::new(&key).exists());
    let err = c.models.delete(&c.ctx(), "/etc", &["cell1".into()]).await.unwrap_err();
    assert!(err.contains("isn't in the library"), "{err}");
    c.live.stop();
}

#[tokio::test]
async fn a_refused_full_check_falls_back_to_the_quick_facts() {
    let home = temp("check-held-home");
    model_folder(&home, "Qwen/Qwen3.8-27B-FP8", true);
    let (node, pa) = fake_node(&home, real_shell()).await;
    let c = cluster("check-held", &[("cell1", pa)]).await;
    // The exit refuses the full check (it starts a container) in look-only; the quick one reads.
    let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
    gate.set_look_only(true).unwrap();
    c.live.set_write_policy(gate.clone());

    let checks = c.models.check(&c.ctx()).await;
    let n = &checks[0];
    assert!(n.error.is_none(), "{:?}", n.error);
    assert_eq!(n.home, home.display().to_string());
    assert!(n.root_exists);
    assert_eq!(n.vllm_version, "");
    let ran: Vec<String> = node.log.lock().unwrap().iter().filter(|cmd| cmd.contains("sparkjob.py check")).cloned().collect();
    assert_eq!(ran.len(), 1, "only the quick check reached the node: {ran:?}");
    assert!(ran[0].ends_with(" quick"), "{ran:?}");
    let refused: Vec<_> = c.live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect();
    assert_eq!(refused.len(), 1, "{refused:?}");
    // The refused line names the whole command (R3-SAFE writes sparkjob's with its runner).
    assert!(refused[0].command.contains("sparkjob.py check ") && !refused[0].command.ends_with("quick"), "{refused:?}");

    // Some other failure is reported as it was, not hidden behind the quick facts.
    gate.set_look_only(false).unwrap();
    c.live.stop();
    let checks = c.models.check(&c.ctx()).await;
    assert!(checks[0].error.as_deref().is_some_and(|e| e.contains("isn't connected")), "{:?}", checks[0].error);
}

#[tokio::test]
async fn sharing_serves_from_the_source_over_the_fabric_and_stops_when_copies_finish() {
    let home = temp("share-home");
    let key = model_folder(&home, "Qwen/Qwen3.8-27B-FP8", true);
    let pulls_done = Arc::new(Mutex::new(false));
    let started: Arc<Mutex<Vec<String>>> = Arc::default();
    let (done, ids) = (pulls_done.clone(), started.clone());
    let script: Script = Arc::new(move |cmd, _| {
        if cmd.contains("sparkjob.py check") {
            return Some((check_reply(1), 0));
        }
        if cmd.contains("sparkjob.py start") {
            let id = cmd.split_whitespace().last().unwrap().to_string();
            ids.lock().unwrap().push(id.clone());
            return Some((json!({ "id": id, "state": "running", "step": "starting" }).to_string(), 0));
        }
        if cmd.contains("sparkjob.py list") {
            let state = if *done.lock().unwrap() { "done" } else { "running" };
            let jobs: Vec<Value> = ids.lock().unwrap().iter().map(|id| json!({ "id": id, "state": state })).collect();
            return Some((Value::from(jobs).to_string(), 0));
        }
        None
    });
    // Separate homes: only cell1 has the model.
    let (src, p1) = fake_node(&home, script.clone()).await;
    let other = temp("share-home2");
    let (dst, p2) = fake_node(&other, script).await;
    let c = cluster("share", &[("cell1", p1), ("cell2", p2)]).await;

    let jobs = c.models.share(&c.ctx(), &key, &["cell1".into(), "cell2".into()]).await.unwrap();
    assert_eq!(jobs.len(), 1, "the source isn't copied to itself");
    assert_eq!(jobs[0].node, "cell2");

    let serve = src.specs.lock().unwrap()[0].clone();
    assert_eq!(serve["address"], "192.168.100.1", "NVIDIA's first fabric subnet");
    assert_eq!(serve["allow"], "192.168.100.0/24");
    assert_eq!(serve["path"], key);
    let port = serve["port"].as_u64().unwrap();
    assert!((8730..8930).contains(&port));
    let password = serve["password"].as_str().unwrap();
    assert_eq!(password.len(), 32);

    let pull = dst.specs.lock().unwrap()[0].clone();
    for spec in [&serve, &pull] {
        assert_eq!(spec["exclude"], json!(["original/", "metal/", "*.pth"]), "checkpoints vLLM doesn't serve stay behind");
    }
    assert_eq!(pull["src"], format!("rsync://sparkbox@192.168.100.1:{port}/model"));
    assert_eq!(pull["dest"], key, "same absolute path on every node");
    assert_eq!(pull["password"], password);
    let files: Vec<&str> = pull["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert!(files.contains(&"model.safetensors") && files.contains(&"config.json"), "{files:?}");
    let serve_id = src.log.lock().unwrap().iter().find(|c| c.contains("start serve")).unwrap().split_whitespace().last().unwrap().to_string();

    // The watcher checks every 5 s and stops the server once the copies are done.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(!src.log.lock().unwrap().iter().any(|c| c.contains("cancel")), "still copying");
    *pulls_done.lock().unwrap() = true;
    let mut cancelled = false;
    for _ in 0..80 {
        if src.log.lock().unwrap().iter().any(|c| c.contains(&format!("cancel {serve_id}"))) {
            cancelled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(cancelled, "the temporary file server was stopped");
    c.live.stop();
}

/// The download spec for gpt-oss-120b, run through the real sparkjob.py's download command:
/// only the 60.8 GiB that vLLM serves is fetched, not the repo's 182 GiB.
#[test]
fn a_download_fetches_only_the_served_files() {
    let info: Value = serde_json::from_str(include_str!("fixtures/hf/gpt-oss-120b.model.json")).unwrap();
    let index: Value = serde_json::from_str(include_str!("fixtures/hf/gpt-oss-120b.index.json")).unwrap();
    let config: Value = serde_json::from_str(include_str!("fixtures/hf/gpt-oss-120b.config.json")).unwrap();
    let model = hf::parse_model(&info, config, Some(&index), "");
    let spec = download_spec(&model, "/home/sparkuser/models/openai/gpt-oss-120b", "nvcr.io/nvidia/vllm:26.05-py3", "hf_secret".into());
    let total: u64 = spec["files"].as_array().unwrap().iter().filter_map(|f| f["size"].as_u64()).sum();
    assert_eq!(format!("{:.1}", total as f64 / 1024f64.powi(3)), "60.8");

    let dir = temp("sparkjob-download-cmd");
    std::fs::write(dir.join("sparkjob.py"), SPARKJOB).unwrap();
    let code = "import importlib.util, json, sys\n\
                s = importlib.util.spec_from_file_location('sparkjob', sys.argv[1]); m = importlib.util.module_from_spec(s); s.loader.exec_module(m)\n\
                spec = json.loads(sys.stdin.read())\n\
                print(json.dumps({'cmd': m.download_cmd('dl-1', spec, '/dest'), 'verify': [f['path'] for f in m.served_files(dict(spec, exclude=[]))]}))";
    let mut child = std::process::Command::new("python3")
        .args(["-I", "-c", code, &dir.join("sparkjob.py").display().to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(spec.to_string().as_bytes()).unwrap();
    let out: Value = serde_json::from_slice(&child.wait_with_output().unwrap().stdout).unwrap();
    let cmd: Vec<&str> = out["cmd"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let includes: Vec<&str> = cmd.windows(2).filter(|w| w[0] == "--include").map(|w| w[1]).collect();
    assert_eq!(includes.len(), spec["files"].as_array().unwrap().len(), "one --include per served file");
    assert_eq!(includes.iter().filter(|f| f.ends_with(".safetensors")).count(), 15);
    assert!(!includes.iter().any(|f| f.starts_with("original/") || f.starts_with("metal/")), "{includes:?}");
    assert!(!cmd.iter().any(|a| a.contains("hf_secret")), "the token isn't in the command");
    let instance = format!("sparkbox.instance={}", crate::identity::instance_name());
    assert_eq!(cmd[..9], ["docker", "run", "--rm", "--name", "sparkbox-dl-dl-1", "--label", &instance, "--label", "sparkbox.deployment=dl-1"]);
    assert_eq!(out["verify"].as_array().unwrap().len(), includes.len(), "and only those are verified");
}

// ---- Loading and unloading

/// What a serving vLLM answers SparkBox's readiness requests with (models/readiness.rs, C-R):
/// its model list, a one-token test, 42 to the sanity prompt, and its /openapi.json.
fn engine_answers(cmd: &str, served: &str) -> Option<(String, u32)> {
    if !cmd.starts_with("curl ") {
        return None;
    }
    if cmd.contains("/v1/models") && !cmd.contains(" -H @-") {
        return Some(("{\"error\":\"Unauthorized\"}\n401".into(), 0));
    }
    if cmd.contains("/v1/models") {
        return Some((format!("{{\"object\":\"list\",\"data\":[{{\"id\":\"{served}\",\"object\":\"model\",\"max_model_len\":32768}}]}}\n200"), 0));
    }
    if cmd.contains("/v1/completions") {
        return Some((format!("{}\n200 0.084 0\n", include_str!("fixtures/probe/completion.json").trim()), 0));
    }
    if cmd.contains("/v1/chat/completions") && cmd.contains("17+25") {
        return Some(("{\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"42\"}}]}\n200".into(), 0));
    }
    if cmd.contains("/openapi.json") {
        return Some(("{\"paths\":{\"/v1/chat/completions\":{},\"/v1/completions\":{},\"/v1/models\":{}}}\n200".into(), 0));
    }
    None
}

/// The pre-flight (C-P2) as an idle GB10 answers it: memory to spare, the image there, the
/// folder present (`model`), the port free, /dev/shm empty, no container, no Ollama.
fn preflight_reply(model: bool) -> String {
    format!(
        "--- mem\nMemTotal:       127598748 kB\nMemFree:        119861600 kB\nMemAvailable:   122601504 kB\nCached:          3196204 kB\n--- earlyoom\ninactive\ncat: /etc/default/earlyoom: No such file or directory\n--- image\nsha256:{}\n--- store\n{}\n--- model\n{}--- port\n--- shm\ntotal 0\ndrwxrwxrwt  2 root root   40 Oct 29 09:33 .\ndrwxr-xr-x 20 root root 4600 Oct 29 09:33 ..\n--- caches\nls: cannot access '/home/sparkuser/.cache/vllm': No such file or directory\nls: cannot access '/home/sparkuser/.cache/flashinfer': No such file or directory\nls: cannot access '/home/sparkuser/.triton': No such file or directory\n--- containers\n--- ray\n--- ollama\n",
        "5a0f8b914da56ea2ae2dbe569e4219a14605ce798e7eff12ae6db63328adc4f1",
        crate::models::fake_vllm::OVERLAY2,
        if model { "present\n" } else { "" }
    )
}

/// What a two-node load looks like from the nodes' side.
fn vllm_script(probes: Arc<Mutex<Vec<&'static str>>>, n: u8) -> Script {
    // Like docker: a container this instance started is listed until it's removed.
    let running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    Arc::new(move |cmd, _| {
        use std::sync::atomic::Ordering::SeqCst;
        if cmd.contains("sparkjob.py check") {
            return Some((check_reply(n), 0));
        }
        if cmd.starts_with("echo '--- mem'") {
            return Some((preflight_reply(true), 0));
        }
        if let Some(a) = engine_answers(cmd, "qwen3.8-27b") {
            return Some(a);
        }
        // The jumbo path: never this Mac's own ping.
        if cmd.starts_with("ping ") {
            return Some(("half1 ok\nhalf2 ok\n".into(), 0));
        }
        if cmd.contains("docker run -d") {
            running.store(true, SeqCst);
            return Some((String::new(), 0));
        }
        if cmd.starts_with("test -f") || cmd.contains("docker image") {
            return Some((String::new(), 0));
        }
        if cmd.contains("docker ps -aq") {
            running.store(false, SeqCst);
            return Some(("ok\n".into(), 0));
        }
        if cmd.starts_with("docker ps -a --filter label=sparkbox.deployment=") {
            let listed = if running.load(SeqCst) { format!("0123456789ab {}\n", crate::identity::instance_name()) } else { String::new() };
            return Some((listed, 0));
        }
        // Only the head is probed; a worker just says how it is.
        if cmd.contains("docker inspect") && n != 1 {
            return Some(("running 0\n".into(), 0));
        }
        if cmd.contains("docker inspect") {
            let mut p = probes.lock().unwrap();
            let next = if p.len() > 1 { p.remove(0) } else { p[0] };
            return Some((next.to_string(), 0));
        }
        None
    })
}

/// The fake nodes' collector reports a vllm-head container publishing port 8000 and holding
/// 88 GB of GPU memory on every node, so loads use 8001 and override the memory guardrail.
fn spec(nodes: &[&str], tp: u64, pp: u64) -> DeploySpec {
    DeploySpec {
        id: "qwen".into(),
        served_name: "qwen3.8-27b".into(),
        model_path: "/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8".into(),
        recipe: "generic".into(),
        nodes: nodes.iter().map(|n| n.to_string()).collect(),
        tensor_parallel: tp,
        pipeline_parallel: pp,
        max_model_len: Some(32768),
        gpu_memory_utilization: 0.8,
        port: 8001,
        image: "nvcr.io/nvidia/vllm:26.05-py3".into(),
        override_memory: true,
        ..Default::default()
    }
}

async fn wait_state(c: &Cluster, id: &str, want: &str) -> Deployment {
    for _ in 0..150 {
        let d = c.models.deployments(&c.ctx()).into_iter().find(|d| d.spec.id == id).unwrap();
        if d.state == want {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{id} never reached {want}: {:?}", c.models.deployments(&c.ctx()));
}

#[tokio::test]
async fn a_crash_while_loading_reports_the_last_log_lines() {
    let probes = Arc::new(Mutex::new(vec![
        "running 0\n000\nLoading safetensors checkpoint shards:  40% Completed\n",
        "exited 1\n000\nLoading safetensors checkpoint shards:  60% Completed\ntorch.OutOfMemoryError: CUDA out of memory\n",
    ]));
    let (_n, p1) = fake_node(&temp("crash-1"), vllm_script(probes, 1)).await;
    let c = cluster("crash", &[("cell1", p1)]).await;

    c.models.deploy(&c.ctx(), spec(&["cell1"], 1, 1)).await.unwrap();
    let d = wait_state(&c, "qwen", "failed").await;
    assert!(d.message.contains("exit 1"), "{}", d.message);
    assert!(d.message.contains("CUDA out of memory"), "{}", d.message);
    assert!(c.llm.get("vllm-qwen").await.is_none());
    c.live.stop();
}

#[tokio::test]
async fn loading_stops_early_when_a_node_lacks_the_model() {
    let script: Script = Arc::new(|cmd, _| {
        if cmd.contains("sparkjob.py check") {
            return Some((check_reply(1), 0));
        }
        if cmd.starts_with("echo '--- mem'") {
            return Some((preflight_reply(false), 0));
        }
        if cmd.starts_with("test -f") {
            return Some((String::new(), 1));
        }
        None
    });
    let (n, p1) = fake_node(&temp("missing-1"), script).await;
    let c = cluster("missing", &[("cell1", p1)]).await;
    // One node: the pre-flight finds it missing before anything is saved or started.
    let e = c.models.deploy(&c.ctx(), DeploySpec { id: "missing-one".into(), ..spec(&["cell1"], 1, 1) }).await.unwrap_err();
    assert_eq!(e, "cell1 doesn't have /home/sparkuser/models/Qwen/Qwen3.8-27B-FP8 yet. Copy it to that node first (Library › Copy to nodes…).");
    assert!(c.models.deployments(&c.ctx()).is_empty(), "nothing saved");
    // Several nodes: the same read on each, as one set.
    let second: Script = Arc::new(|cmd: &str, _: &[u8]| {
        if cmd.contains("sparkjob.py check") {
            return Some((check_reply(2), 0));
        }
        cmd.starts_with("echo '--- mem'").then(|| (preflight_reply(true), 0))
    });
    let (_m, p2) = fake_node(&temp("missing-2"), second).await;
    let c2 = cluster("missing-two", &[("cell1", p1), ("cell2", p2)]).await;
    let e = c2.models.deploy(&c2.ctx(), DeploySpec { id: "missing-two".into(), ..spec(&["cell1", "cell2"], 2, 1) }).await.unwrap_err();
    assert!(e.contains("cell1 doesn't have"), "{e}");
    assert!(c2.models.deployments(&c2.ctx()).is_empty(), "nothing saved");
    assert!(!n.log.lock().unwrap().iter().any(|c| c.contains("docker run")), "nothing started");
    c.live.stop();
    c2.live.stop();
}

// ---- The AI feasibility check

fn facts() -> Facts {
    let arch = fit::ArchFacts { layers: 45, kv_layers: 11, heads: 64, kv_heads: 64, head_dim: 128, mla_dim: Some(512), ..Default::default() };
    Facts {
        model: json!({ "id": "nvidia/GLM-5.3-Flash-NVFP4", "weightBytes": 204_400_000_000u64 }),
        kv_cache_bytes_per_token: fit::kv_bytes_per_token(&arch),
        fit_table: fit::fit_table(204_400_000_000, &arch, fit::DEFAULT_NODE_MEMORY, 4),
        architecture: arch,
        nodes: vec![],
        engine: "nvcr.io/nvidia/vllm:26.05-py3".into(),
        already_loaded: vec![],
        demo_cluster: true,
    }
}

async fn advisor_llm(name: &str) -> (Arc<Llm>, fake_upstream::Seen) {
    let (upstream, seen) = fake_upstream::start().await;
    let dir = temp(&format!("advisor-{name}"));
    let vpn = Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let llm = Llm::new(dir.join("providers.json"), vpn);
    for (id, kind, base) in [
        ("advisor-claude", ProviderKind::Anthropic, upstream.clone()),
        ("advisor-openai", ProviderKind::Openai, format!("{upstream}/v1")),
    ] {
        llm.upsert(Provider { id: id.into(), name: id.into(), kind, base_url: base, ..Default::default() }).await.unwrap();
        secrets::set(&format!("llm:{id}"), "sk-test").unwrap();
    }
    (llm, seen)
}

#[tokio::test]
async fn the_analysis_with_claude_asks_for_structured_output_and_parses_it() {
    let (llm, seen) = advisor_llm("claude").await;
    let r = llm.analyze("advisor-claude", "claude-opus-5-5", &facts(), "GLM card").await.unwrap();
    assert_eq!(r.analysis.recommended.tensor_parallel_size, 4);
    assert_eq!(r.analysis.verdict, "good");
    assert_eq!(r.model, "claude-opus-5-5");

    let (path, headers, sent) = seen.last();
    assert_eq!(path, "messages");
    assert_eq!(sent["output_config"]["format"]["type"], "json_schema");
    assert_eq!(sent["output_config"]["format"]["schema"]["additionalProperties"], false);
    assert_eq!(sent["output_config"]["effort"], "high");
    assert_eq!(sent["thinking"]["type"], "adaptive");
    assert_eq!(sent["fallbacks"], "default");
    assert_eq!(headers["anthropic-beta"], crate::llm::chat::FALLBACK_BETA);
    assert_eq!(headers["anthropic-version"], crate::llm::ANTHROPIC_VERSION);
    assert!(sent.get("temperature").is_none());
    let text = sent["messages"][0]["content"].as_str().unwrap();
    assert!(text.contains("nvidia/GLM-5.3-Flash-NVFP4") && text.contains("fitTable") && text.contains("GLM card"));
}

#[tokio::test]
async fn the_analysis_with_openai_uses_a_strict_json_schema() {
    let (llm, seen) = advisor_llm("openai").await;
    let r = llm.analyze("advisor-openai", "gpt-test", &facts(), "").await.unwrap();
    assert_eq!(r.analysis.alternatives[0].nodes, 3);
    let (path, headers, sent) = seen.last();
    assert_eq!(path, "responses");
    assert_eq!(headers["authorization"], "Bearer sk-test");
    assert_eq!(sent["text"]["format"]["strict"], true);
    assert_eq!(sent["store"], false);
}

#[tokio::test]
async fn the_analysis_needs_a_key_and_a_real_provider() {
    let (llm, _) = advisor_llm("nokey").await;
    llm.upsert(Provider {
        id: "advisor-keyless".into(),
        name: "Keyless".into(),
        kind: ProviderKind::Anthropic,
        base_url: "http://127.0.0.1:9".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    let err = llm.analyze("advisor-keyless", "claude-opus-5-5", &facts(), "").await.unwrap_err();
    assert!(err.contains("API key"), "{err}");
    assert!(llm.analyze("nope", "x", &facts(), "").await.unwrap_err().contains("Unknown provider"));
}

// ---- Demo mode

/// A Demo setup with the simulated cluster, as the app runs it.
fn demo_app(name: &str) -> (Arc<Models>, Arc<Llm>, Arc<Live>, Arc<Mutex<DemoCluster>>, PathBuf) {
    let dir = temp(name);
    let vpn = Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
    let live = Live::new(vpn.clone(), dir.join("known_hosts.json"), || Ok(None));
    let llm = Llm::new(dir.join("providers.json"), vpn);
    let cluster = Arc::new(Mutex::new(DemoCluster::new(&Inventory::example())));
    let models = Models::new(dir.clone(), live.clone(), llm.clone(), Some(cluster.clone()));
    (models, llm, live, cluster, dir)
}

fn demo_ctx(cluster: &Arc<Mutex<DemoCluster>>) -> Ctx {
    Ctx { live: false, settings: ModelSettings::default(), inventory: Inventory::example(), snapshot: Some(cluster.lock().unwrap().snapshot()) }
}

#[tokio::test]
async fn deployment_history_unknown_update_cannot_erase_a_newer_file() {
    let (models, _, _, _, dir) = demo_app("history-unknown-update");
    let saved = vec![Deployment { spec: spec(&["cell2"], 1, 1), state: DeploymentState::Stopped,
        unload_report: Some(UnloadReport { ok: false, summary: "Keep this warning".into(), ..Default::default() }), ..Default::default() }];
    let path = dir.join("deployments.json");
    crate::util::save_json(&path, &saved).unwrap();
    let before = std::fs::read(&path).unwrap();
    models.update("missing", |_| panic!("no such deployment"));
    assert_eq!(std::fs::read(&path).unwrap(), before, "a stale empty list must not erase history");
}

#[tokio::test]
async fn deployment_history_demo_forget_cannot_remove_a_live_record_with_the_same_id() {
    let (models, _, _, cluster, dir) = demo_app("history-demo-forget");
    let real = Deployment { spec: spec(&["cell2"], 1, 1), state: DeploymentState::Stopped,
        unload_report: Some(UnloadReport { summary: "Keep this live report".into(), ..Default::default() }), ..Default::default() };
    models.deployments.lock().unwrap().push(real.clone());
    models.update_saved("qwen", |_| {}).unwrap();
    let before = std::fs::read(dir.join("deployments.json")).unwrap();
    models.demo.lock().unwrap().claim(Deployment { demo: true,
        spec: DeploySpec { model_path: "offline-history-fixture".into(), ..real.spec.clone() }, ..real }).unwrap();
    models.forget_checked(&demo_ctx(&cluster), "qwen").await.unwrap();
    assert!(models.deployments(&demo_ctx(&cluster)).is_empty());
    assert_eq!(models.deployments.lock().unwrap().len(), 1);
    assert_eq!(std::fs::read(dir.join("deployments.json")).unwrap(), before);
    assert!(!models.live.audit.entries().iter().any(|e| e.title == "Forgot deployment note"));
    models.forget("qwen").unwrap();
    assert!(models.live.audit.entries().iter().any(|e| e.title == "Forgot deployment note" && e.command.contains("qwen; recovery copy retained")));
}

/// The plan the Load page gets carries the sized share and the request count (the review's R20
/// and R21): what plan.rs works out from the plan's own needs and nodes, not None.
#[tokio::test]
async fn the_plan_sent_to_the_page_carries_the_sized_share_and_the_request_count() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-plan-sized");
    let ctx = demo_ctx(&cluster);
    let lib = models.library(&ctx).await.unwrap();
    let qwen = lib.iter().find(|e| e.repo == "Qwen/Qwen3.8-27B-FP8").unwrap();
    let s = DeploySpec { model_path: qwen.key.clone(), max_model_len: Some(32768), ..spec(&["gx10-0000"], 1, 1) };
    let p = models.plan(&ctx, &s).await.unwrap();
    assert_eq!(p.sized_share, plan::sized_share(&s, &p.needs, &p.nodes));
    assert_eq!(p.full_requests, plan::full_requests(&s, &p.nodes));
    assert_eq!(p.sized_share, Some(0.4), "next to Ollama: {:?}", p.nodes);
    assert!(p.full_requests.is_some_and(|n| n > 4), "{:?}", p.full_requests);
    let at = models.plan(&ctx, &DeploySpec { gpu_memory_utilization: 0.4, ..s.clone() }).await.unwrap();
    assert_eq!((at.sized_share, at.level), (Some(0.4), plan::Level::Green));
    assert!(at.full_requests.is_some_and(|n| (4..10).contains(&n)), "{:?}", at.full_requests);
}

#[tokio::test]
async fn inventory_warnings_reach_the_plan_without_replacing_observed_addresses() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-fabric-plan");
    let mut ctx = demo_ctx(&cluster);
    let mut s = spec(&["cell1", "cell2"], 2, 1);
    s.listen = "fabric".into();
    let before = models.plan(&ctx, &s).await.unwrap();
    ctx.inventory.nodes[0].fabric = Some(crate::inventory::fabric::Expectations {
        links: vec![crate::inventory::fabric::Link { iface: "enp1s0f1np1".into(), ip: "10.99.99.99".into(), rdma: "invented".into() }], head_candidate: Some(false),
    });
    let after = models.plan(&ctx, &s).await.unwrap();
    assert!(before.inventory_warnings.is_empty());
    assert!(after.inventory_warnings.iter().any(|w| w.contains("10.99.99.99")));
    assert!(after.inventory_warnings.iter().any(|w| w.contains("Worker preferred")));
    assert_eq!((after.host, after.url, after.level), (before.host, before.url, before.level));
    assert_eq!(s.nodes, ["cell1", "cell2"], "role metadata never reorders the selected head");
}

#[tokio::test]
async fn demo_mode_simulates_loads_and_benchmarks_without_the_cluster() {
    let (models, _llm, _live, cluster, dir) = demo_app("demo-app");
    let ctx = demo_ctx(&cluster);

    // The same folders every demo view shows.
    let lib = models.library(&ctx).await.unwrap();
    assert_eq!(lib.iter().map(|e| e.repo.as_str()).collect::<Vec<_>>(), ["meta-llama/Llama-3.3-70B-Instruct", "Qwen/Qwen3.8-27B-FP8"]);
    let qwen = lib[1].clone();
    let jobs = models.share(&ctx, &qwen.key, &["cell3".into(), "gx10-0000".into()]).await.unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(models.jobs(&ctx).await.unwrap().len(), 2);
    models.cancel_job(&ctx, "cell3", &jobs[0].id).await.unwrap();
    assert!(models.jobs(&ctx).await.unwrap().iter().any(|j| j.state == "cancelled"));

    let checks = models.check(&ctx).await;
    assert_eq!(checks.len(), 4);
    assert_eq!(checks[2].fabric[0].ip, "192.168.100.3");

    // gx10-0000 doesn't have the copy yet. (0.4 is the share that keeps it under 90% next to Ollama.)
    let s = DeploySpec { gpu_memory_utilization: 0.4, override_memory: false, ..spec(&["gx10-0000"], 1, 1) };
    let err = models.deploy(&ctx, s.clone()).await.unwrap_err();
    assert!(err.contains("doesn't have"), "{err}");
    models.demo.lock().unwrap().fast_forward(60.0);
    // Next to Ollama, vLLM's default 0.8 share can't be reserved, and 0.6 would take the node past 95%.
    let err = models.deploy(&ctx, DeploySpec { gpu_memory_utilization: 0.8, ..s.clone() }).await.unwrap_err();
    assert!(err.contains("Won't start: vLLM reserves"), "{err}");
    let err = models.deploy(&ctx, DeploySpec { gpu_memory_utilization: 0.6, ..s.clone() }).await.unwrap_err();
    assert!(err.contains("Won't start safely"), "{err}");

    let d = models.deploy(&ctx, s).await.unwrap();
    assert!(d.demo);
    let ip = Inventory::example().nodes.iter().find(|n| n.id == "gx10-0000").and_then(|n| n.ip.clone()).unwrap();
    assert_eq!(d.endpoint, format!("http://{ip}:8001/v1"));
    models.unload(&ctx, "qwen").await.unwrap();
    assert_eq!(models.deployments(&ctx)[0].state, DeploymentState::Stopped);
    models.forget_checked(&ctx, "qwen").await.unwrap();
    assert!(models.deployments(&ctx).is_empty());
    assert!(!dir.join("deployments.json").exists(), "demo loads aren't saved");
}

#[tokio::test]
async fn a_demo_load_shows_up_everywhere() {
    let (models, llm, live, cluster, _dir) = demo_app("demo-everywhere");
    let lib = models.library(&demo_ctx(&cluster)).await.unwrap();
    models.share(&demo_ctx(&cluster), &lib[1].key, &["gx10-0000".into()]).await.unwrap();
    models.demo.lock().unwrap().fast_forward(60.0);
    let s = DeploySpec { gpu_memory_utilization: 0.4, override_memory: false, ..spec(&["gx10-0000"], 1, 1) };
    models.deploy(&demo_ctx(&cluster), s).await.unwrap();
    // 28.8 GiB at 1.6 GB/s on its one node, then 30 s of graph capture: under a minute.
    models.demo.lock().unwrap().fast_forward(45.0);
    assert_eq!(models.deployments(&demo_ctx(&cluster))[0].state, DeploymentState::Starting);
    models.demo.lock().unwrap().fast_forward(10.0);
    assert_eq!(models.deployments(&demo_ctx(&cluster))[0].state, DeploymentState::Ready);

    // One round: the load's container joins the simulated cluster…
    models.endpoints_tick(&demo_ctx(&cluster), false).await;
    let snap = cluster.lock().unwrap().snapshot();
    let gx = snap.nodes.iter().find(|n| n.id == "gx10-0000").unwrap();
    let c = gx.containers.iter().find(|c| c.labels.get("sparkbox.deployment").map(String::as_str) == Some("qwen")).expect("container added");
    assert_eq!(c.name, "sparkbox-qwen");
    assert!(c.gpu_mem > 50_000_000_000, "its 0.4 share of memory: {}", c.gpu_mem);
    assert!(gx.listeners.iter().any(|l| l.port == 8001), "listening on its port");

    // …so the next round finds it there, managed, ready and with metrics; Chat and the Server can use it.
    let eps = models.endpoints_tick(&demo_ctx(&cluster), false).await;
    let e = eps.iter().find(|e| e.deployment_id.as_deref() == Some("qwen")).unwrap();
    assert_eq!((e.id.as_str(), e.managed_by, e.health), ("gx10-0000-vllm-8001", endpoints::ManagedBy::Sparkbox, endpoints::Health::Ready));
    assert_eq!(e.container_ids, [c.id.clone()]);
    assert!(e.metrics.is_some());
    assert_eq!(eps.iter().filter(|e| e.deployment_id.is_some()).count(), 1, "one endpoint, not two");
    let p = llm.get("vllm-qwen").await.expect("its provider");
    assert_eq!((p.kind, p.models.as_slice()), (ProviderKind::Demo, &["qwen3.8-27b".to_string()][..]));
    assert!(live.audit.entries().iter().any(|e| e.title.contains("Load qwen3.8-27b")), "in the Activity log");

    // Restarting takes it through loading again, then it's back.
    models.restart(&demo_ctx(&cluster), "qwen").await.unwrap();
    // The restart runs on the app's runtime: give it a few seconds while the whole suite runs at once.
    for _ in 0..500 {
        if models.deployments(&demo_ctx(&cluster))[0].state == DeploymentState::Starting {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(models.deployments(&demo_ctx(&cluster))[0].state, DeploymentState::Starting);
    models.endpoints_tick(&demo_ctx(&cluster), false).await;
    let snap = cluster.lock().unwrap().snapshot();
    assert!(!snap.nodes.iter().flat_map(|n| &n.containers).any(|c| c.labels.contains_key("sparkbox.deployment")), "down while it restarts");
    models.demo.lock().unwrap().fast_forward(70.0);
    models.endpoints_tick(&demo_ctx(&cluster), false).await;
    let snap = cluster.lock().unwrap().snapshot();
    assert!(snap.nodes.iter().flat_map(|n| &n.containers).any(|c| c.labels.get("sparkbox.deployment").map(String::as_str) == Some("qwen")));

    // Unloading takes the container away again.
    models.unload(&demo_ctx(&cluster), "qwen").await.unwrap();
    models.endpoints_tick(&demo_ctx(&cluster), false).await;
    let snap = cluster.lock().unwrap().snapshot();
    assert!(!snap.nodes.iter().flat_map(|n| &n.containers).any(|c| c.labels.contains_key("sparkbox.deployment")));
    assert!(llm.get("vllm-qwen").await.is_none());
    assert!(live.audit.entries().iter().any(|e| e.title.contains("Unload qwen3.8-27b")));
}

#[tokio::test]
async fn the_demo_cluster_lists_llama_over_three_nodes_and_ollama() {
    let (models, llm, _live, cluster, _dir) = demo_app("demo-endpoints");
    models.endpoints_tick(&demo_ctx(&cluster), false).await;
    cluster.lock().unwrap().advance();
    let eps = models.endpoints_tick(&demo_ctx(&cluster), false).await;
    let v = eps.iter().find(|e| e.id == "cell1-vllm-8000").expect("Llama on the Ray head");
    assert_eq!(v.models, ["llama-3.3-70b"]);
    assert_eq!(v.nodes, ["cell1", "cell2", "cell3"]);
    assert_eq!(v.parallel, Some(endpoints::Parallel { tp: 1, pp: 3 }));
    assert_eq!(v.managed_by, endpoints::ManagedBy::External);
    let m = v.metrics.as_ref().expect("metrics");
    assert_eq!(m.history.ts.len(), 1);
    assert!(m.gen_tps < 16.0 * 2.0, "PP=3 bf16 70B: well under 2 tok/s a stream ({})", m.gen_tps);
    let o = eps.iter().find(|e| e.id == "gx10-0000-ollama-11434").expect("Ollama");
    assert_eq!(o.models, ["gpt-oss:20b", "qwen3:32b", "llama3.2:3b", "nomic-embed-text:latest"], "everything installed");
    assert_eq!(o.ollama.len(), 2, "two of them in memory");
    assert_eq!(o.on_disk, o.models, "what's in memory is on its disk too, with every other pulled model");
    assert!(o.on_disk_bytes.iter().all(|b| *b > 0));
    let p = llm.views().await.into_iter().find(|p| p.provider.id == o.id).unwrap();
    assert!(!p.visible.contains(&"nomic-embed-text:latest".to_string()), "embeddings aren't offered in Chat");
    assert_eq!(o.metrics_source.as_deref(), Some("sparkbox"));
    // Both answer in Chat, at the speed the charts show.
    let p = llm.get("cell1-vllm-8000").await.unwrap();
    assert_eq!(p.kind, ProviderKind::Demo);
    let speed = llm.demo_speed("cell1-vllm-8000").unwrap();
    assert!((1.0..2.0).contains(&speed.tps), "{speed:?}");
    assert!(llm.get("gx10-0000-ollama-11434").await.is_some());
}

fn find<'a>(eps: &'a [Endpoint], id: &str) -> &'a Endpoint {
    eps.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("{id} isn't listed: {:?}", eps.iter().map(|e| &e.id).collect::<Vec<_>>()))
}

const LLAMA: &str = "cell1-vllm-8000";

/// Advances the simulated cluster `secs` seconds, then runs one endpoint round.
async fn demo_round(models: &Arc<Models>, cluster: &Arc<Mutex<DemoCluster>>, secs: u32) -> Vec<Endpoint> {
    for _ in 0..secs {
        cluster.lock().unwrap().advance();
    }
    models.endpoints_tick(&demo_ctx(cluster), false).await
}

/// Runs rounds in the background (a simulated second each, plus `ff` seconds of Demo load time),
/// as the app's 3-second loop does, recording what each round said about `id`.
fn ticker(models: &Arc<Models>, cluster: &Arc<Mutex<DemoCluster>>, id: &str, ff: f64) -> (Arc<std::sync::atomic::AtomicBool>, Arc<Mutex<Vec<(endpoints::Health, Option<u64>)>>>) {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (m, c, s, seen_, id) = (models.clone(), cluster.clone(), stop.clone(), seen.clone(), id.to_string());
    tokio::spawn(async move {
        while !s.load(std::sync::atomic::Ordering::Relaxed) {
            c.lock().unwrap().advance();
            m.demo.lock().unwrap().fast_forward(ff);
            let eps = m.endpoints_tick(&demo_ctx(&c), false).await;
            if let Some(e) = eps.iter().find(|e| e.id == id || e.deployment_id.as_deref() == Some(id.as_str())) {
                seen_.lock().unwrap().push((e.health, e.loading_pct));
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    });
    (stop, seen)
}

#[tokio::test]
async fn a_pipeline_stage_going_offline_is_unknown_with_its_reason_then_recovers() {
    let (models, llm, _live, cluster, _dir) = demo_app("demo-node-offline");
    let eps = demo_round(&models, &cluster, 1).await;
    assert_eq!(find(&eps, LLAMA).health, endpoints::Health::Ready);
    let last_ok = find(&eps, LLAMA).last_ok_ms;
    assert!(llm.get(LLAMA).await.is_some());

    cluster.lock().unwrap().set_scenario(Some("node-offline")).unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let eps = demo_round(&models, &cluster, 5).await;
    let v = find(&eps, LLAMA);
    assert_eq!(v.health, endpoints::Health::Unknown);
    assert_eq!(v.health_reason.as_deref(), Some("Pipeline stage 3 of 3 on cell3 is offline"));
    assert_eq!((v.missing_nodes.as_slice(), v.nodes.as_slice()), (&["cell3".to_string()][..], &["cell1".to_string(), "cell2".into(), "cell3".into()][..]));
    // No current metrics can be claimed while a rank is unobservable.
    assert!(v.metrics.is_none());
    assert_eq!(v.last_ok_ms, last_ok, "the last successful check stays historical while disconnected");
    assert!(llm.get(LLAMA).await.is_none());
    // Its GPUs go quiet with it.
    let eps = demo_round(&models, &cluster, 12).await;
    assert_eq!(find(&eps, LLAMA).health, endpoints::Health::Unknown);
    assert_eq!(find(&eps, LLAMA).last_ok_ms, last_ok);
    let snap = cluster.lock().unwrap().snapshot();
    let util = |id: &str| snap.nodes.iter().find(|n| n.id == id).unwrap().gpu.util_pct;
    assert!(util("cell1") < 10.0 && util("cell2") < 10.0, "cell1 {} cell2 {}", util("cell1"), util("cell2"));

    // cell3 is back: Ready again (its fabric stayed up, so the pipeline kept its weights) once
    // tokens come out again, which the next scrape shows.
    demo_round(&models, &cluster, 60).await;
    let eps = demo_round(&models, &cluster, 1).await;
    let v = find(&eps, LLAMA);
    assert_eq!((v.health, v.health_reason.as_deref()), (endpoints::Health::Ready, None), "{v:?}");
    assert!(v.missing_nodes.is_empty());
}

#[tokio::test]
async fn serving_gpus_read_their_own_trace_so_their_history_joins_at_now() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-gpu-load");
    let mut serving = 0;
    for _ in 0..12 {
        let eps = demo_round(&models, &cluster, 3).await;
        let running = find(&eps, LLAMA).metrics.as_ref().unwrap().running;
        let (snap, at) = {
            let mut c = cluster.lock().unwrap();
            (c.snapshot(), c.sim_ms())
        };
        if running < 1.0 {
            continue;
        }
        serving += 1;
        // While the engine serves as usual, every stage reads exactly the curve the Demo history
        // is synthesized from (demo::trace), not a level of the simulator's own: no step at launch.
        for id in ["cell1", "cell2", "cell3"] {
            let got = snap.nodes.iter().find(|n| n.id == id).unwrap().gpu.util_pct as f64;
            let want = crate::demo::trace::util_at(id, at) * 100.0;
            assert!((got - want).abs() <= 2.5, "{id} reads {got} where its trace says {want}");
        }
    }
    assert!(serving >= 4, "llama served in only {serving} of 12 rounds");
}

#[tokio::test]
async fn losing_a_ray_worker_takes_the_head_down_and_it_stays_listed() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-cascade");
    demo_round(&models, &cluster, 1).await;
    cluster.lock().unwrap().container_action("cell2", "vllm-worker", "stop").unwrap();
    let eps = demo_round(&models, &cluster, 4).await;
    let v = find(&eps, LLAMA);
    assert_eq!(v.health, endpoints::Health::Down, "{v:?}");
    let why = v.health_reason.clone().unwrap();
    assert!(why.starts_with("Head exited (code 1)") && why.ends_with(watch::RAY_DIED), "{why}");
    let snap = cluster.lock().unwrap().snapshot();
    let head = snap.nodes[0].containers.iter().find(|c| c.name == "vllm-head").unwrap();
    assert_eq!((head.state.as_str(), head.exit_code), ("exited", 1), "the simulated head really exited");
    // Still listed after many rounds (it doesn't vanish), until it's forgotten.
    let eps = demo_round(&models, &cluster, 30).await;
    assert_eq!(find(&eps, LLAMA).health, endpoints::Health::Down);
    models.forget_endpoint(LLAMA).unwrap();
    let eps = demo_round(&models, &cluster, 1).await;
    assert!(!eps.iter().any(|e| e.id == LLAMA), "forgotten");
}

#[tokio::test]
async fn a_crash_loop_is_down_and_never_loading() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-crash-loop");
    demo_round(&models, &cluster, 1).await;
    cluster.lock().unwrap().set_scenario(Some("crash-loop")).unwrap();
    let mut reasons = Vec::new();
    for _ in 0..40 {
        let eps = demo_round(&models, &cluster, 3).await;
        let snap = cluster.lock().unwrap().snapshot();
        let worker = snap.nodes.iter().find(|n| n.id == "cell2").unwrap().containers.iter().find(|c| c.name == "vllm-worker").unwrap().state.clone();
        let v = find(&eps, LLAMA);
        if worker == "restarting" {
            assert_eq!(v.health, endpoints::Health::Down, "restarting is never Loading: {v:?}");
        }
        reasons.push(v.health_reason.clone().unwrap_or_default());
    }
    assert!(reasons.iter().any(|r| r.starts_with("Crash loop on cell2: ") && r.contains("restarts in 10 min · last exit 1")), "{reasons:?}");
}

#[tokio::test]
async fn restarting_the_external_pipeline_in_rays_order_goes_loading_then_ready() {
    let (models, _llm, live, cluster, _dir) = demo_app("demo-ext-restart");
    let tasks = crate::tasks::Tasks::new(None);
    models.set_tasks(tasks.clone());
    demo_round(&models, &cluster, 1).await;
    let (stop, seen) = ticker(&models, &cluster, LLAMA, 0.0);
    let id = models.restart_external(&demo_ctx(&cluster), LLAMA).await.unwrap();
    // A second click while it runs starts nothing.
    let again = models.restart_external(&demo_ctx(&cluster), LLAMA).await.unwrap_err();
    assert_eq!(again, "llama-3.3-70b is already restarting: Tasks follows it.");
    let t = finished(&tasks, &id).await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(t.title, "Restart llama-3.3-70b in order");
    let steps: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(steps, ["Stop the workers on cell2, cell3", "Stop the head on cell1", "Start the head on cell1", "Start the workers on cell2, cell3 (they join the head)", "Verify llama-3.3-70b answers"]);
    assert_eq!(t.state, crate::tasks::TaskState::Ok, "{t:?}");
    assert!(t.summary.as_deref().unwrap().starts_with("Verified: llama-3.3-70b answered after"), "{:?}", t.summary);
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().any(|(h, pct)| *h == endpoints::Health::Loading && pct.is_some()), "Loading with a percentage on the way: {seen:?}");
    assert_eq!(seen.last().unwrap().0, endpoints::Health::Ready);
    // The 131 GiB pipeline reads ~47 GB a node at 1.6 GB/s, then captures CUDA graphs: about a minute.
    let took = t.summary.as_deref().unwrap().rsplit("after ").next().unwrap().to_string();
    let after_s: u64 = match took.split_once("m ") {
        Some((m, s)) => m.parse::<u64>().unwrap() * 60 + s.trim_end_matches('s').parse::<u64>().unwrap(),
        None => took.trim_end_matches('s').parse().unwrap(),
    };
    assert!((50..=90).contains(&after_s), "{:?}", t.summary);

    // The activity log tells it as one operation: one head line, each step stamped as it ran, then Verify.
    let audit = live.audit.entries();
    let mine: Vec<&crate::live::audit::Entry> = audit.iter().rev().filter(|e| e.op_id.as_deref() == Some(id.as_str())).collect();
    let heads: Vec<&&crate::live::audit::Entry> = mine.iter().filter(|e| e.kind == "op").collect();
    assert_eq!(heads.len(), 1, "one head line: {mine:#?}");
    assert_eq!((heads[0].title.as_str(), heads[0].state.as_str()), ("Restart llama-3.3-70b in order", "ok"));
    assert!(heads[0].stdout_tail.as_deref().unwrap_or("").starts_with("Verified: llama-3.3-70b answered after"), "{:?}", heads[0]);
    let order: Vec<&str> = mine.iter().filter(|e| e.command.starts_with("docker ")).map(|e| e.command.as_str()).collect();
    assert_eq!(order, ["docker stop vllm-worker", "docker stop vllm-worker", "docker stop vllm-head", "docker start vllm-head", "docker start vllm-worker", "docker start vllm-worker"]);
    let stamps: Vec<u64> = mine.iter().filter(|e| e.command.starts_with("docker ")).map(|e| e.ts_ms).collect();
    assert!(stamps.windows(2).all(|w| w[1] >= w[0] + 1_000), "each step stamped when it ran, not all in one millisecond: {stamps:?}");
    let verify = mine.iter().find(|e| e.title == "Verify llama-3.3-70b answers").expect("a Verify step");
    assert_eq!(verify.state, "ok");
    assert!(verify.stdout_tail.as_deref().unwrap_or("").starts_with("llama-3.3-70b answered after"));
    assert_eq!(audit.iter().filter(|e| e.command.starts_with("docker ") && e.op_id.as_deref() != Some(id.as_str())).count(), 0, "no loose rows");
    assert_eq!(tasks.list().iter().filter(|t| t.title.starts_with("Restart llama")).count(), 1, "one restart");
}

/// Waits for a task to finish (the rounds run beside it).
async fn finished(tasks: &Arc<crate::tasks::Tasks>, id: &str) -> crate::tasks::Task {
    for _ in 0..6000 {
        if let Some(t) = tasks.get(id).filter(|t| t.finished()) {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{id} never finished: {:?}", tasks.get(id));
}

#[tokio::test]
async fn a_restart_after_a_crash_loop_starts_the_count_over() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-restart-after-loop");
    let tasks = crate::tasks::Tasks::new(None);
    models.set_tasks(tasks.clone());
    demo_round(&models, &cluster, 1).await;
    cluster.lock().unwrap().set_scenario(Some("crash-loop")).unwrap();
    for _ in 0..25 {
        demo_round(&models, &cluster, 3).await;
    }
    cluster.lock().unwrap().set_scenario(None).unwrap();
    let eps = demo_round(&models, &cluster, 1).await;
    assert!(find(&eps, LLAMA).health_reason.as_deref().unwrap_or("").starts_with("Crash loop on cell2"), "still counted as a loop just after it");
    // Restarting it by hand fixes what the loop was about: the old restarts don't fail the verify.
    let (stop, _) = ticker(&models, &cluster, LLAMA, 0.0);
    let id = models.restart_external(&demo_ctx(&cluster), LLAMA).await.unwrap();
    let t = finished(&tasks, &id).await;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(t.state, crate::tasks::TaskState::Ok, "{t:?}");
}

/// A copy of Qwen on `nodes` (the demo library has it on cell1), ready to load.
async fn qwen_everywhere(models: &Arc<Models>, cluster: &Arc<Mutex<DemoCluster>>, nodes: &[&str]) {
    let lib = models.library(&demo_ctx(cluster)).await.unwrap();
    let targets: Vec<String> = nodes.iter().filter(|n| **n != "cell1").map(|n| n.to_string()).collect();
    if !targets.is_empty() {
        models.share(&demo_ctx(cluster), &lib[1].key, &targets).await.unwrap();
        models.demo.lock().unwrap().fast_forward(60.0);
        models.library(&demo_ctx(cluster)).await.unwrap();
    }
}

#[tokio::test]
async fn a_sparkbox_restart_is_a_task_that_ends_when_it_answers() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-ordered-restart");
    let tasks = crate::tasks::Tasks::new(None);
    models.set_tasks(tasks.clone());
    qwen_everywhere(&models, &cluster, &["cell1", "cell2"]).await;
    let s = DeploySpec { id: "qwen2".into(), gpu_memory_utilization: 0.5, override_memory: true, ..spec(&["cell1", "cell2"], 2, 1) };
    models.deploy(&demo_ctx(&cluster), s).await.unwrap();
    models.demo.lock().unwrap().fast_forward(80.0);
    demo_round(&models, &cluster, 1).await;
    let eps = demo_round(&models, &cluster, 1).await;
    let e = eps.iter().find(|e| e.deployment_id.as_deref() == Some("qwen2")).unwrap();
    assert_eq!(e.health, endpoints::Health::Ready, "{:?} {:?}", e.health_reason, e.members);

    let (stop, seen) = ticker(&models, &cluster, "qwen2", 3.0);
    models.restart_ordered(&demo_ctx(&cluster), "qwen2", "Restart", false).await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let t = tasks.list().remove(0);
    let steps: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(steps, ["Stop the head on cell1", "Stop the workers on cell2", "Start the workers on cell2", "Start the head on cell1", "Verify qwen3.8-27b answers"]);
    assert!(t.summary.as_deref().unwrap().starts_with("Verified: qwen3.8-27b answered after"), "{t:?}");
    assert_eq!(t.title, "Restart qwen3.8-27b in order");
    assert!(t.retry.as_ref().is_some_and(|r| r.command == "deployment_restart"), "Retry goes back through the command (and its confirmation)");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().any(|(h, p)| *h == endpoints::Health::Loading && p.is_some()), "{seen:?}");
}

#[tokio::test]
async fn an_sglang_load_runs_one_rank_per_node_in_the_demo() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-sglang");
    qwen_everywhere(&models, &cluster, &["cell1", "cell2"]).await;
    let s = DeploySpec {
        id: "sg".into(),
        engine: "sglang".into(),
        image: "lmsysorg/sglang:spark".into(),
        require_key: false,
        gpu_memory_utilization: 0.5,
        override_memory: true,
        ..spec(&["cell1", "cell2"], 2, 1)
    };
    assert!(models.deploy(&demo_ctx(&cluster), DeploySpec { require_key: true, ..s.clone() }).await.unwrap_err().contains("command line"));
    models.deploy(&demo_ctx(&cluster), s).await.unwrap();
    models.demo.lock().unwrap().fast_forward(80.0);
    demo_round(&models, &cluster, 1).await;
    let eps = demo_round(&models, &cluster, 1).await;
    let e = eps.iter().find(|e| e.deployment_id.as_deref() == Some("sg")).unwrap();
    assert_eq!((e.id.as_str(), e.engine, e.health), ("cell1-sglang-8001", endpoints::Engine::Sglang, endpoints::Health::Ready), "{:?}", e.health_reason);
    assert_eq!(e.nodes, ["cell1", "cell2"]);
    let snap = cluster.lock().unwrap().snapshot();
    let rank1 = snap.nodes.iter().find(|n| n.id == "cell2").unwrap().containers.iter().find(|c| c.name == "sparkbox-sg-cell2").unwrap();
    assert!(rank1.command.contains("--node-rank 1 --dist-init-addr 192.168.100.1:5000"), "{}", rank1.command);
}

#[tokio::test]
async fn keep_running_restarts_a_multi_node_load_that_lost_a_worker() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-supervisor");
    let tasks = crate::tasks::Tasks::new(None);
    models.set_tasks(tasks.clone());
    qwen_everywhere(&models, &cluster, &["cell1", "cell2"]).await;
    let s = DeploySpec { id: "qwen3".into(), keep_running: true, gpu_memory_utilization: 0.5, override_memory: true, ..spec(&["cell1", "cell2"], 2, 1) };
    models.deploy(&demo_ctx(&cluster), s).await.unwrap();
    models.demo.lock().unwrap().fast_forward(80.0);
    demo_round(&models, &cluster, 1).await;
    let eps = demo_round(&models, &cluster, 1).await;
    let e = eps.iter().find(|e| e.deployment_id.as_deref() == Some("qwen3")).unwrap();
    assert_eq!(e.health, endpoints::Health::Ready);
    assert_eq!(e.supervisor.as_ref().map(|s| s.state), Some(endpoints::SupervisorState::Watching));

    // The worker container is stopped: Ray takes the head down with it.
    cluster.lock().unwrap().container_action("cell2", "sparkbox-qwen3-cell2", "stop").unwrap();
    demo_round(&models, &cluster, 1).await;
    let eps = demo_round(&models, &cluster, 1).await;
    let e = eps.iter().find(|e| e.deployment_id.as_deref() == Some("qwen3")).unwrap();
    assert_eq!(e.health, endpoints::Health::Down);
    assert!(e.health_reason.as_deref().unwrap().starts_with("Head exited (code 1): pipeline stage 2 of 2 on cell2 stopped"), "{:?}", e.health_reason);
    let sup = e.supervisor.clone().unwrap();
    assert_eq!(sup.state, endpoints::SupervisorState::Restarting);
    let wait = sup.next_retry_ms.unwrap().saturating_sub(now() * 1000);
    assert!((25_000..=31_000).contains(&wait), "the first retry in 30 s: {wait}");

    // When it's due, the supervisor restarts it, as a task of its own.
    models.eps.lock().unwrap().supervisors.get_mut("qwen3").unwrap().next_retry_ms = Some(0);
    demo_round(&models, &cluster, 1).await;
    for _ in 0..100 {
        if !tasks.list().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let t = tasks.list().remove(0);
    assert_eq!((t.title.as_str(), t.source.as_str()), ("Keep running: restart qwen3.8-27b in order (1 of 5)", "system"));
    let eps = demo_round(&models, &cluster, 1).await;
    let e = eps.iter().find(|e| e.deployment_id.as_deref() == Some("qwen3")).unwrap();
    assert_eq!(e.supervisor.as_ref().unwrap().restarts, 1, "the card shows the restart count");
}

/// Two fake nodes with a Keep running load on them, the real watcher and rounds over them, and
/// each collector reporting that load's containers, labelled as this build labels them.
struct Kept {
    c: Cluster,
    head: crate::fake_node::Node,
    probes: Arc<Mutex<Vec<&'static str>>>,
}

impl Kept {
    async fn new(name: &str, id: &str, probes: Vec<&'static str>, answers: Option<&'static str>) -> Kept {
        let probes = Arc::new(Mutex::new(probes));
        let wrong = |script: Script| -> Script {
            Arc::new(move |cmd, stdin| match answers {
                Some(a) if cmd.contains("17+25") => Some((format!("{{\"choices\":[{{\"index\":0,\"message\":{{\"role\":\"assistant\",\"content\":\"{a}\"}},\"finish_reason\":\"stop\"}}]}}\n200"), 0)),
                _ => script(cmd, stdin),
            })
        };
        let (head, p1) = fake_node(&temp(&format!("{name}-1")), wrong(vllm_script(probes.clone(), 1))).await;
        let (worker, p2) = fake_node(&temp(&format!("{name}-2")), vllm_script(probes.clone(), 2)).await;
        let labels = |role: &str, rank: u8| {
            let mut l = json!({ "sparkbox.deployment": id, "sparkbox.instance": crate::identity::instance_name(), "sparkbox.group": id, "sparkbox.role": role, "sparkbox.rank": rank.to_string(),
                "sparkbox.model": "qwen3.8-27b", "sparkbox.path": "/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8", "sparkbox.recipe": "generic", "sparkbox.parallel": "2x1", "sparkbox.image": "nvcr.io/nvidia/vllm:26.05-py3" });
            if rank == 0 {
                l["sparkbox.port"] = json!("8001");
                l["sparkbox.listen"] = json!("127.0.0.1");
            }
            l
        };
        let show = |node: &crate::fake_node::Node, c: Value| {
            let mut v: Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
            v["containers"] = json!([c]);
            *node.knobs.frame.lock().unwrap() = Some(v.to_string());
            node.knobs.frame_every_ms.store(100, std::sync::atomic::Ordering::SeqCst);
        };
        let container = |name: String, role: &str, rank: u8| {
            json!({ "id": format!("{role}0123456789"), "name": name, "image": "nvcr.io/nvidia/vllm:26.05-py3", "state": "running", "startedAt": 1_790_000_000u64,
                "finishedAt": 0, "exitCode": 0, "cpuPct": 1.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "python3 /run/sparkbox/launch.py --key", "labels": labels(role, rank) })
        };
        show(&head, container(format!("sparkbox-{id}-r0"), "head", 0));
        show(&worker, container(format!("sparkbox-{id}-r1"), "worker", 1));
        let c = cluster(name, &[("cell1", p1), ("cell2", p2)]).await;
        c.models.set_tasks(crate::tasks::Tasks::new(None));
        // Its own gate: the app-wide look-only flag is other tests' to set.
        c.models.set_gate(crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None));
        c.models.deploy(&c.ctx(), DeploySpec { id: id.into(), keep_running: true, ..spec(&["cell1", "cell2"], 2, 1) }).await.unwrap();
        Kept { c, head, probes }
    }

    /// Rounds until the card says what `want` wants; the time a retry needs is the test's to skip.
    async fn until_card(&self, id: &str, want: impl Fn(&endpoints::SupervisorView) -> bool) -> Endpoint {
        for _ in 0..100 {
            let eps = self.c.models.endpoints_tick(&self.c.ctx(), true).await;
            if let Some(e) = eps.into_iter().find(|e| e.deployment_id.as_deref() == Some(id) && e.supervisor.as_ref().is_some_and(&want)) {
                return e;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{id}'s card never got there: {:?}", self.c.models.endpoints().iter().find(|e| e.deployment_id.as_deref() == Some(id)).map(|e| (&e.health, &e.health_reason, &e.supervisor)));
    }

    /// Rounds until the last restart's Verify step is over (it has seen the load stay Down).
    async fn restart_over(&self) {
        for _ in 0..300 {
            if self.c.models.restarting.lock().unwrap().is_empty() {
                return;
            }
            self.c.models.endpoints_tick(&self.c.ctx(), true).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the restart never ended");
    }

    /// A failed start stays as it is: rounds find nothing to do, and nothing is stopped or started.
    async fn left_alone(&self, id: &str, runs: usize) {
        let refused = |s: &endpoints::SupervisorView| s.held.as_deref() == Some("It failed to start: won't restart it");
        let e = self.until_card(id, refused).await;
        assert_eq!(e.health, endpoints::Health::Down);
        let stops = self.head.ran("docker stop").len();
        for _ in 0..3 {
            self.c.models.endpoints_tick(&self.c.ctx(), true).await;
            let s = self.c.models.eps.lock().unwrap().supervisors.get(id).cloned().unwrap();
            assert_eq!((s.next_retry_ms, s.gave_up), (None, false), "nothing scheduled");
        }
        assert_eq!(self.head.ran("docker run -d").len(), runs, "nothing started");
        assert_eq!(self.head.ran("docker stop").len(), stops, "nothing stopped");
    }
}

/// A start is left as it failed when it never became Ready, and whatever ran before it when it
/// got a verdict: the time limit, or the wrong answer.
#[tokio::test]
async fn keep_running_leaves_a_load_that_failed_at_its_limit() {
    let k = Kept::new("kept-limit", "kept-limit", vec!["running 0\n000\nLoading\n"], None).await;
    k.c.models.set_ready_within(Duration::from_secs(1));
    k.c.models.update("kept-limit", |d| d.recoverable = true);
    let d = wait_state(&k.c, "kept-limit", "failed").await;
    assert_eq!((d.message.as_str(), d.recoverable), ("vLLM didn't become ready within 1 s.", false));
    k.left_alone("kept-limit", 1).await;
    k.c.live.stop();
}

#[tokio::test]
async fn keep_running_leaves_a_load_that_gave_the_wrong_answer() {
    let k = Kept::new("kept-wrong", "kept-wrong", vec!["running 0\n200\nINFO Application startup complete.\n"], Some("4 2 2 2 2 2 2 2")).await;
    k.c.models.update("kept-wrong", |d| d.recoverable = true);
    let d = wait_state(&k.c, "kept-wrong", "failed").await;
    assert!(d.message.starts_with("It answers, but wrongly") && !d.recoverable, "{}", d.message);
    k.left_alone("kept-wrong", 1).await;
    k.c.live.stop();
}

/// A load an older SparkBox saved as Ready never said it ran; failing Ready says so.
#[tokio::test]
async fn a_load_saved_ready_counts_as_having_run_when_it_fails() {
    let (_n, p1) = fake_node(&temp("fail-saved"), real_shell()).await;
    let c = cluster("fail-saved", &[("cell1", p1)]).await;
    let spec = DeploySpec { id: "fs".into(), nodes: vec!["cell1".into()], ..Default::default() };
    let mut saved = serde_json::to_value(Deployment { spec, state: DeploymentState::Ready, ..Default::default() }).unwrap();
    saved.as_object_mut().unwrap().remove("recoverable");
    c.models.deployments.lock().unwrap().push(serde_json::from_value(saved).unwrap());
    c.models.fail("fs", "vLLM stopped on cell1 (exit 137).".into());
    assert!(c.models.deployments.lock().unwrap().iter().any(|d| d.spec.id == "fs" && d.state == DeploymentState::Failed && d.recoverable));
    c.live.stop();
}

#[tokio::test]
async fn keep_running_leaves_a_first_start_that_stopped_while_loading() {
    let k = Kept::new("kept-first", "kept-first", vec!["exited 137\n000\nKilled\n"], None).await;
    let d = wait_state(&k.c, "kept-first", "failed").await;
    assert!(d.message.starts_with("vLLM stopped on cell1 (exit 137)") && !d.recoverable, "{}", d.message);
    k.left_alone("kept-first", 1).await;
    k.c.live.stop();
}

/// The other side of those: a load that ran and crashed is what Keep running is for. `follow` sees
/// the crash and fails it, Keep running restarts it, and a restart that crashes is tried again.
/// A restart judged at its limit ends it there, though the load once ran. A real watcher's poll
/// of a Ready load and two real Verify steps: this takes about 75 s.
#[tokio::test]
async fn keep_running_restarts_a_load_that_ran_until_a_start_gets_a_verdict() {
    let up = "running 0\n200\nINFO Application startup complete.\n";
    let crash = "exited 137\n000\nKilled\n";
    let k = Kept::new("kept-ran", "kept-ran", vec![up], None).await;
    let d = wait_state(&k.c, "kept-ran", "ready").await;
    assert!(d.recoverable);

    *k.probes.lock().unwrap() = vec![crash];
    let d = until_load_for(&k.c, "kept-ran", 30, |d| d.state == DeploymentState::Failed).await;
    assert!(d.message.starts_with("vLLM stopped on cell1 (exit 137)") && d.recoverable, "{}", d.message);
    k.until_card("kept-ran", |s| s.state == endpoints::SupervisorState::Restarting && s.held.is_none()).await;
    assert_eq!(k.head.ran("docker run -d").len(), 1, "it waits first");

    // Its first restart stops while loading: tried again.
    let due = || k.c.models.eps.lock().unwrap().supervisors.get_mut("kept-ran").unwrap().next_retry_ms = Some(0);
    due();
    k.until_card("kept-ran", |s| s.restarts == 1).await;
    until_load_for(&k.c, "kept-ran", 30, |d| d.state == DeploymentState::Failed && d.generation == 1).await;
    assert_eq!(k.head.ran("docker run -d").len(), 2);
    k.until_card("kept-ran", |s| s.state == endpoints::SupervisorState::Restarting).await;
    k.restart_over().await;

    // The next one reaches its limit: a verdict, so no third.
    *k.probes.lock().unwrap() = vec!["running 0\n000\nLoading\n"];
    k.c.models.set_ready_within(Duration::from_secs(1));
    due();
    k.until_card("kept-ran", |s| s.restarts == 2).await;
    let d = until_load_for(&k.c, "kept-ran", 30, |d| d.state == DeploymentState::Failed && d.generation == 2).await;
    assert_eq!((d.message.as_str(), d.recoverable), ("vLLM didn't become ready within 1 s.", false));
    k.left_alone("kept-ran", 3).await;
    k.restart_over().await;

    // By hand it's still the owner's to try: that clears Keep running's count, and Ready its verdict.
    *k.probes.lock().unwrap() = vec![up];
    k.c.models.restart(&k.c.ctx(), "kept-ran").await.unwrap();
    let d = until_load_for(&k.c, "kept-ran", 30, |d| d.state == DeploymentState::Ready && d.generation == 3).await;
    assert!(d.recoverable);
    k.c.live.stop();
}

#[tokio::test]
async fn the_stall_scenario_is_down_after_60_s_then_recovers() {
    let (models, llm, _live, cluster, _dir) = demo_app("demo-stall");
    demo_round(&models, &cluster, 1).await;
    let eps = demo_round(&models, &cluster, 1).await;
    assert_eq!(find(&eps, LLAMA).health, endpoints::Health::Ready);
    cluster.lock().unwrap().set_scenario(Some("endpoint-stall")).unwrap();
    // Rounds 15 s apart (the test ages what SparkBox remembers instead of waiting).
    for _ in 0..4 {
        let eps = demo_round(&models, &cluster, 1).await;
        assert_eq!(find(&eps, LLAMA).health, endpoints::Health::Ready, "{:?}", find(&eps, LLAMA).health_reason);
        models.age(15_000);
    }
    let eps = demo_round(&models, &cluster, 1).await;
    let v = find(&eps, LLAMA);
    assert_eq!(v.health, endpoints::Health::Down);
    let why = v.health_reason.clone().unwrap();
    assert!(why.starts_with("Stalled: ") && why.contains("requests running, no tokens for 6"), "{why}");
    assert!(v.metrics.as_ref().is_some_and(|m| m.running >= 3.0), "the engine still holds its requests");
    assert!(llm.get(LLAMA).await.is_none(), "not offered in Chat");
    assert_eq!(find(&eps, "gx10-0000-ollama-11434").health, endpoints::Health::Ready, "Ollama is fine");
    // Its test token hangs too.
    models.age(20_000);
    let v = find(&demo_round(&models, &cluster, 1).await, LLAMA).clone();
    assert_eq!(v.canary.as_ref().and_then(|c| c.error.as_deref()), Some("no reply in 30 s"));

    // Over: tokens flow again, and that live traffic is the proof it's Ready.
    cluster.lock().unwrap().set_scenario(None).unwrap();
    demo_round(&models, &cluster, 1).await;
    demo_round(&models, &cluster, 1).await;
    let v = find(&demo_round(&models, &cluster, 1).await, LLAMA).clone();
    assert_eq!((v.health, v.health_reason.as_deref()), (endpoints::Health::Ready, None), "{v:?}");
}

#[tokio::test]
async fn the_demo_canary_answers_then_keeps_its_last_result_while_a_stage_is_unavailable() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-canary");
    demo_round(&models, &cluster, 1).await;
    let eps = demo_round(&models, &cluster, 1).await;
    // A full pass through the 141 GB pipeline: about 0.6 s for one token.
    let c = find(&eps, LLAMA).canary.clone().expect("tested on its first answer");
    assert!(c.ok && c.latency_ms.is_some_and(|ms| (450..=900).contains(&ms)), "{c:?}");
    let o = find(&eps, "gx10-0000-ollama-11434").canary.clone().unwrap();
    assert!(o.ok && o.latency_ms.is_some_and(|ms| ms < 400), "{o:?}");
    cluster.lock().unwrap().set_scenario(Some("node-offline")).unwrap();
    models.age(61_000);
    let eps = demo_round(&models, &cluster, 5).await;
    let v = find(&eps, LLAMA);
    assert_eq!(v.health_reason.as_deref(), Some("Pipeline stage 3 of 3 on cell3 is offline"));
    assert_eq!(v.health, endpoints::Health::Unknown);
    let last = v.canary.as_ref().unwrap();
    assert!(last.ok && last.error.is_none(), "unavailable ranks supply no new canary verdict");
    assert_eq!(last.at_ms, c.at_ms - 61_000, "only the test's aging changed its old timestamp");
    assert!(v.metrics.is_none());
}

#[tokio::test]
async fn the_demo_engine_log_says_what_the_card_says() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-engine-log");
    // What endpoints_list showed after each round, by the simulated second of its scrape.
    let mut cards: Vec<(u64, crate::models::metrics::EndpointMetrics)> = Vec::new();
    for _ in 0..12 {
        let eps = demo_round(&models, &cluster, 1).await;
        let m = find(&eps, LLAMA).metrics.clone().unwrap();
        cards.push((m.updated_ms / 1000, m));
    }
    let lines = models.engine_log("cell1", "vllm-head");
    assert!(lines.len() >= 2, "a stats line every 10 s of simulated time: {lines:?}");
    // The simulated terminal reads the same lines (from whichever Models wrote last).
    assert!(!watch::demo_engine_log("cell1", "vllm-head", 0, u64::MAX).is_empty());
    let num = |line: &str, label: &str| -> f64 { line.split(label).nth(1).unwrap().trim_start().split(|c: char| c != '.' && !c.is_ascii_digit()).next().unwrap().parse().unwrap() };
    for (at, line) in &lines {
        let (_, m) = cards.iter().find(|(t, _)| t == at).unwrap_or_else(|| panic!("no card for the line at {at}: {line}"));
        // The same simulated engine fed /metrics and the log: running, waiting and KV agree to the rounding.
        assert_eq!(num(line, "Running:"), m.running.round(), "{line}");
        assert_eq!(num(line, "Waiting:"), m.waiting.round(), "{line}");
        assert!((num(line, "GPU KV cache usage:") - m.kv_pct).abs() <= 0.05, "{line} vs {}", m.kv_pct);
        // Throughput is vLLM's 10 s average of the scrapes the card showed one by one.
        let window: Vec<f64> = m.history.ts.iter().zip(&m.history.gen_tps).filter(|(t, _)| **t + 10 > *at).map(|(_, v)| *v).collect();
        let avg = window.iter().sum::<f64>() / window.len().max(1) as f64;
        assert!((num(line, "Avg generation throughput:") - avg).abs() <= 0.05, "{line} vs {avg}");
        assert!((num(line, "Prefix cache hit rate:") - 46.0).abs() < 3.0, "{line}");
        assert!(line.contains("[loggers.py:123] Engine 000: Avg prompt throughput:"), "{line}");
    }
}

#[tokio::test]
async fn serving_history_is_recorded() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-serving-history");
    let history = crate::history::History::new(None);
    for _ in 0..30 {
        let eps = demo_round(&models, &cluster, 1).await;
        let mut snap = cluster.lock().unwrap().snapshot();
        snap.ts_ms = eps.iter().find(|e| e.id == LLAMA).and_then(|e| e.metrics.as_ref()).map(|m| m.updated_ms).unwrap_or(snap.ts_ms);
        // What health::on_publish does with every snapshot.
        history.record_entities(&snap, &crate::health::endpoint_infos(&eps));
    }
    let to = now_ms() + 3_600_000;
    let q = history.query(&format!("endpoint:{LLAMA}"), &["genTps".into(), "ttftP95".into(), "kvPct".into()], to - 7_200_000, to, 720).unwrap();
    let got = |m: &str| q.series[m].iter().filter(|v| v.is_some()).count();
    assert!(got("genTps") > 10 && got("kvPct") > 10, "{:?}", q.series);
    assert!(got("ttftP95") > 0, "TTFT p95 once requests finished: {:?}", q.series["ttftP95"]);
    // Each test token is handed to the history as canaryMs, once.
    let tokens = models.take_new_canaries();
    assert!(tokens.iter().any(|(id, _, ms)| id == LLAMA && *ms > 400), "{tokens:?}");
    assert!(models.take_new_canaries().is_empty(), "not handed over twice");
    let (id, at, ms) = tokens.iter().find(|(id, ..)| id == LLAMA).cloned().unwrap();
    history.record_entity(&format!("endpoint:{id}"), "canaryMs", at as i64, ms as f32).expect("the store keeps canaryMs");
    let q = history.query(&format!("endpoint:{id}"), &["canaryMs".into()], at - 60_000, at + 60_000, 120).unwrap();
    assert!(q.series["canaryMs"].iter().flatten().any(|v| (*v - ms as f32).abs() < 1.0), "{:?}", q.series);
}

#[tokio::test]
async fn serving_scenarios_fail_what_they_name() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-model-scenarios");
    qwen_everywhere(&models, &cluster, &["cell1", "cell2", "gx10-0000"]).await;

    // model-oom: killed at 80% of the weights.
    cluster.lock().unwrap().set_scenario(Some("model-oom")).unwrap();
    models.deploy(&demo_ctx(&cluster), DeploySpec { id: "oom".into(), override_memory: true, ..spec(&["gx10-0000"], 1, 1) }).await.unwrap();
    demo_round(&models, &cluster, 1).await;
    models.demo.lock().unwrap().fast_forward(22.0);
    let eps = demo_round(&models, &cluster, 1).await;
    let e = eps.iter().find(|e| e.deployment_id.as_deref() == Some("oom")).unwrap();
    assert_eq!((e.health, e.health_reason.as_deref()), (endpoints::Health::Down, Some("Out of memory while loading (exit 137)")));
    models.unload(&demo_ctx(&cluster), "oom").await.unwrap();

    // ray-join-timeout: the last node never joins.
    cluster.lock().unwrap().set_scenario(Some("ray-join-timeout")).unwrap();
    models.deploy(&demo_ctx(&cluster), DeploySpec { id: "join".into(), override_memory: true, ..spec(&["cell1", "cell2"], 2, 1) }).await.unwrap();
    demo_round(&models, &cluster, 1).await;
    models.demo.lock().unwrap().fast_forward(60.0);
    let d = models.deployments(&demo_ctx(&cluster)).into_iter().find(|d| d.spec.id == "join").unwrap();
    assert_eq!(d.stage, Some(Stage::Joining { have: 1, need: 2 }), "still waiting for cell2");
    models.demo.lock().unwrap().fast_forward(70.0);
    let eps = demo_round(&models, &cluster, 1).await;
    let e = eps.iter().find(|e| e.deployment_id.as_deref() == Some("join")).unwrap();
    assert_eq!((e.health, e.health_reason.as_deref()), (endpoints::Health::Down, Some("Ray worker on cell2 didn't join in 120 s")));

    // copy-disk-full: the copy stops at 92%.
    cluster.lock().unwrap().set_scenario(Some("copy-disk-full")).unwrap();
    demo_round(&models, &cluster, 1).await;
    let lib = models.library(&demo_ctx(&cluster)).await.unwrap();
    let job = models.share(&demo_ctx(&cluster), &lib[1].key, &["cell3".into()]).await.unwrap().remove(0);
    models.demo.lock().unwrap().fast_forward(60.0);
    let j = models.jobs(&demo_ctx(&cluster)).await.unwrap().into_iter().find(|j| j.id == job.id).unwrap();
    assert_eq!((j.state.as_str(), j.message.as_str()), ("failed", "No space left on device"));
    assert_eq!(j.bytes_done, (j.bytes_total as f64 * 0.92) as u64);
    assert!(!models.library(&demo_ctx(&cluster)).await.unwrap()[1].copies.contains_key("cell3"), "no copy on cell3");

    // gated-401: Hugging Face refuses the download.
    cluster.lock().unwrap().set_scenario(Some("gated-401")).unwrap();
    demo_round(&models, &cluster, 1).await;
    let info = json!({ "id": "meta-llama/Llama-3.3-70B-Instruct", "sha": "abc", "gated": "manual",
        "siblings": [{ "rfilename": "config.json", "size": 900 }, { "rfilename": "model-00001-of-00001.safetensors", "size": 141_000_000_000u64 }] });
    let m = hf::parse_model(&info, Value::Null, None, "");
    let job = models.demo.lock().unwrap().download("cell1", &m, "~/models/meta-llama/Llama-3.3-70B-Instruct-x");
    models.demo.lock().unwrap().fast_forward(3.0);
    let j = models.jobs(&demo_ctx(&cluster)).await.unwrap().into_iter().find(|j| j.id == job.id).unwrap();
    assert_eq!((j.state.as_str(), j.message.as_str()), ("failed", "This model is gated: accept its license on huggingface.co and add a token"));
}

#[tokio::test]
async fn the_slow_endpoint_scenario_builds_a_queue() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-slow");
    cluster.lock().unwrap().set_scenario(Some("endpoint-slow")).unwrap();
    let mut eps = Vec::new();
    for _ in 0..80 {
        eps = demo_round(&models, &cluster, 3).await;
    }
    let m = find(&eps, LLAMA).metrics.clone().unwrap();
    assert!(m.waiting >= 2.0, "a queue: {}", m.waiting);
    assert!(m.ttft_p95_ms.unwrap() >= 2_000.0, "TTFT p95 climbs to seconds: {:?}", m.ttft_p95_ms);
}

// ---- Endpoints on (fake) live nodes

#[tokio::test]
async fn live_endpoints_are_probed_on_their_head_node() {
    let fixture = include_str!("fixtures/vllm_metrics.txt");
    let script: Script = Arc::new(move |cmd, _| {
        if cmd.contains("127.0.0.1:8000/v1/models") {
            assert!(cmd.contains("127.0.0.1:8000/health"), "/health with every model list: {cmd}");
            return Some(("{\"data\":[{\"id\":\"llama-3.3-70b\",\"max_model_len\":32768}]}\n200\n--sparkbox--\n\n200".into(), 0));
        }
        if cmd.contains("127.0.0.1:8000/metrics") {
            return Some((format!("{fixture}\n200"), 0));
        }
        if cmd.contains("127.0.0.1:8000/v1/completions") {
            return Some((format!("{}\n200 0.084 0\n", include_str!("fixtures/probe/completion.json").trim()), 0));
        }
        None
    });
    let (node, p1) = fake_node(&temp("eps-1"), script).await;
    let c = cluster("eps", &[("cell1", p1)]).await;
    c.models.endpoints_tick(&c.ctx(), false).await;
    let e = tested(&c, &c.ctx(), "cell1-vllm-8000", |e| e.canary.is_some()).await;
    assert_eq!(e.canary.as_ref().map(|c| (c.ok, c.latency_ms)), Some((true, Some(84))));
    let eps = c.models.endpoints_tick(&c.ctx(), false).await;
    let e = eps.iter().find(|e| e.id == "cell1-vllm-8000").expect("the collector's vllm-head");
    assert_eq!(e.models, ["llama-3.3-70b"], "from /v1/models");
    assert_eq!(e.max_context, Some(32768));
    let m = e.metrics.as_ref().unwrap();
    assert_eq!((m.running, m.waiting, m.kv_pct), (3.0, 1.0, 21.78));
    assert_eq!(e.metrics_source.as_deref(), Some("server"));
    let p = c.llm.get("cell1-vllm-8000").await.expect("registered for Chat and the Server");
    assert_eq!((p.kind, p.base_url.as_str()), (ProviderKind::OpenaiCompatible, "http://127.0.0.1:8000/v1"));
    let log = node.log.lock().unwrap().clone();
    assert_eq!(log.iter().filter(|c| c.contains("/v1/models")).count(), 1, "model lists every 15 s, not every round");
    assert!(log.iter().filter(|c| c.contains("/metrics")).count() >= 3, "metrics every round");
    assert_eq!(log.iter().filter(|c| c.contains("/v1/completions")).count(), 1, "one test token a minute");

    // A server that stops answering goes down and leaves Chat.
    c.models.endpoints_tick(&c.ctx(), true).await;
    c.live.stop();
}

/// Runs endpoint rounds until `want` holds for `id` (a test token runs beside the rounds).
async fn tested(c: &Cluster, ctx: &Ctx, id: &str, want: impl Fn(&Endpoint) -> bool) -> Endpoint {
    for _ in 0..200 {
        let eps = c.models.endpoints_tick(ctx, false).await;
        if let Some(e) = eps.iter().find(|e| e.id == id).filter(|e| want(e)) {
            return e.clone();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{id} never got there: {:?}", c.models.endpoints().iter().find(|e| e.id == id));
}

/// A GB10's vLLM on loopback: its model list and /health, /metrics (the fixture with the token
/// counter and running requests set by the test) and test tokens (`reply`: body, status line).
fn engine_script(gen: Arc<std::sync::atomic::AtomicU64>, step: u64, running: f64, reply: Arc<Mutex<String>>) -> Script {
    use std::sync::atomic::Ordering;
    let fixture = include_str!("fixtures/vllm_metrics.txt");
    Arc::new(move |cmd, _| {
        if cmd.contains("/v1/models") {
            return Some(("{\"data\":[{\"id\":\"llama-3.3-70b\"}]}\n200\n--sparkbox--\n\n200".into(), 0));
        }
        if cmd.contains("/metrics") {
            let n = gen.fetch_add(step, Ordering::SeqCst);
            let text = fixture
                .replace("vllm:generation_tokens_total{engine=\"0\",model_name=\"meta-llama/Llama-3.3-70B-Instruct\"} 184211.0", &format!("vllm:generation_tokens_total{{engine=\"0\",model_name=\"meta-llama/Llama-3.3-70B-Instruct\"}} {n}.0"))
                .replace("vllm:num_requests_running{engine=\"0\",model_name=\"meta-llama/Llama-3.3-70B-Instruct\"} 3.0", &format!("vllm:num_requests_running{{engine=\"0\",model_name=\"meta-llama/Llama-3.3-70B-Instruct\"}} {running:.1}"))
                // Nothing running, nothing waiting: an idle engine, whose flat counter means nothing.
                .replace("vllm:num_requests_waiting{engine=\"0\",model_name=\"meta-llama/Llama-3.3-70B-Instruct\"} 1.0", &format!("vllm:num_requests_waiting{{engine=\"0\",model_name=\"meta-llama/Llama-3.3-70B-Instruct\"}} {}", if running > 0.0 { "1.0" } else { "0.0" }));
            return Some((format!("{text}\n200"), 0));
        }
        if cmd.contains("/v1/completions") || cmd.contains("/v1/chat/completions") {
            return Some((reply.lock().unwrap().clone(), 0));
        }
        None
    })
}

fn completions(node: &crate::fake_node::Node) -> usize {
    node.log.lock().unwrap().iter().filter(|c| c.contains("/v1/completions")).count()
}

#[tokio::test]
async fn canary_fails_twice_is_down() {
    let dead = format!("{}\n500 0.21 0\n", include_str!("fixtures/probe/engine_dead.json").trim());
    let reply = Arc::new(Mutex::new(dead));
    let gen = Arc::new(std::sync::atomic::AtomicU64::new(184_211));
    let (node, p1) = fake_node(&temp("canary-fails"), engine_script(gen, 0, 0.0, reply.clone())).await;
    let c = cluster("canary-fails", &[("cell1", p1)]).await;
    let id = "cell1-vllm-8000";
    c.models.endpoints_tick(&c.ctx(), true).await;
    let e = tested(&c, &c.ctx(), id, |e| e.canary.is_some()).await;
    assert_eq!(e.health, endpoints::Health::Loading, "its model list answers, but one failed token isn't an outage yet");
    assert_eq!(e.health_reason.as_deref(), Some("Answers, but no token yet (HTTP 500: EngineCore encountered an issue. See stack trace (above) for the root cause.)"));
    assert!(c.llm.get(id).await.is_none(), "not in Chat until it generates");

    // The second try, 15 s later, fails too: Down, in the engine's own words.
    c.models.age(16_000);
    let e = tested(&c, &c.ctx(), id, |e| e.health == endpoints::Health::Down).await;
    assert_eq!(e.health_reason.as_deref(), Some("Didn't generate: HTTP 500: EngineCore encountered an issue. See stack trace (above) for the root cause. (2 tries)"));
    assert_eq!(completions(&node), 2);

    // It recovers: the next test token comes back and it's Ready, in Chat again.
    *reply.lock().unwrap() = format!("{}\n200 0.612 0\n", include_str!("fixtures/probe/completion.json").trim());
    c.models.age(16_000);
    let e = tested(&c, &c.ctx(), id, |e| e.health == endpoints::Health::Ready).await;
    assert_eq!(e.canary.as_ref().and_then(|c| c.latency_ms), Some(612));
    assert!(c.llm.get(id).await.is_some());
    c.live.stop();
}

#[tokio::test]
async fn live_traffic_is_proof() {
    let ok = format!("{}\n200 0.09 0\n", include_str!("fixtures/probe/completion.json").trim());
    let gen = Arc::new(std::sync::atomic::AtomicU64::new(184_211));
    let (node, p1) = fake_node(&temp("canary-passive"), engine_script(gen, 30, 2.0, Arc::new(Mutex::new(ok)))).await;
    let c = cluster("canary-passive", &[("cell1", p1)]).await;
    let id = "cell1-vllm-8000";
    c.models.endpoints_tick(&c.ctx(), true).await;
    tested(&c, &c.ctx(), id, |e| e.health == endpoints::Health::Ready).await;
    // A first round's test token runs beside the round: it's counted once it's over, however
    // long a loaded machine takes.
    while c.models.eps.lock().unwrap().tracks.get(id).is_some_and(|t| t.canary_running) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let first = completions(&node);
    assert!(first <= 1, "at most the first round tests: {first}");
    // Minutes pass with tokens rising and /health fine: that is the proof, no test tokens.
    for _ in 0..4 {
        c.models.age(61_000);
        let before = now_ms();
        c.models.endpoints_tick(&c.ctx(), false).await;
        let e = c.models.endpoints_tick(&c.ctx(), false).await.into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.health, endpoints::Health::Ready, "{:?}", e.health_reason);
        assert!(e.passive_ok_ms.is_some_and(|p| p >= before), "live traffic just proved it");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(completions(&node), first, "no test token while live traffic proves it");
    c.live.stop();
}

#[tokio::test]
async fn stall_is_down_after_60s() {
    let ok = format!("{}\n200 0.09 0\n", include_str!("fixtures/probe/completion.json").trim());
    // Three requests running, and the token counter never moves.
    let gen = Arc::new(std::sync::atomic::AtomicU64::new(184_211));
    let (_node, p1) = fake_node(&temp("canary-stall"), engine_script(gen, 0, 3.0, Arc::new(Mutex::new(ok)))).await;
    let c = cluster("canary-stall", &[("cell1", p1)]).await;
    let id = "cell1-vllm-8000";
    c.models.endpoints_tick(&c.ctx(), true).await;
    tested(&c, &c.ctx(), id, |e| e.health == endpoints::Health::Ready).await;
    for _ in 0..3 {
        c.models.age(15_000);
        let e = c.models.endpoints_tick(&c.ctx(), false).await.into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.health, endpoints::Health::Ready, "under 60 s flat: {:?}", e.stalled_s);
    }
    c.models.age(15_000);
    let e = c.models.endpoints_tick(&c.ctx(), false).await.into_iter().find(|e| e.id == id).unwrap();
    assert_eq!(e.health, endpoints::Health::Down);
    let why = e.health_reason.clone().unwrap();
    assert!(why.starts_with("Stalled: 3 requests running, no tokens for 6"), "{why}");
    assert!(e.stalled_s.is_some_and(|s| s >= 60), "{:?}", e.stalled_s);
    assert!(c.llm.get(id).await.is_none());
    c.live.stop();
}

#[tokio::test]
async fn ollama_canary_keeps_residency() {
    let expires = Arc::new(std::sync::atomic::AtomicU64::new(now() + 212));
    let ex = expires.clone();
    let script: Script = Arc::new(move |cmd, _| {
        if cmd.contains("/api/tags") {
            let at = ex.load(std::sync::atomic::Ordering::SeqCst) as i64;
            // Ollama's own format: a local time with its offset, nanoseconds.
            let local = |unix: i64| {
                let (y, mo, d, h, mi, sec, _) = crate::demo::ops::civil((unix - 4 * 3600) as u64);
                format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}.123456789-04:00")
            };
            // gpt-oss:20b has the most time left; qwen3:32b unloads 100 s sooner.
            let ps = include_str!("fixtures/probe/ollama_ps.json").replace("EXPIRES_GPT", &local(at)).replace("EXPIRES_QWEN", &local(at - 100));
            return Some((format!("{}\n200\n--sparkbox--\n{}\n200", include_str!("fixtures/probe/ollama_tags.json").trim(), ps.trim()), 0));
        }
        if cmd.contains("/api/generate") {
            return Some((format!("{}\n200 0.152 0\n", include_str!("fixtures/probe/ollama_generate.json").trim()), 0));
        }
        None
    });
    let (node, p1) = fake_node(&temp("canary-ollama"), script).await;
    let c = cluster("canary-ollama", &[("gx10-0000", p1)]).await;
    let mut ctx = c.ctx();
    let snap = ctx.snapshot.as_mut().unwrap();
    snap.nodes[0].containers.clear();
    snap.nodes[0].processes = vec![crate::model::Process { pid: 812, cmd: "/usr/local/bin/ollama serve".into(), ports: vec![11434], unit: Some("ollama.service".into()), ..Default::default() }];
    let id = "gx10-0000-ollama-11434";
    c.models.endpoints_tick(&ctx, true).await;
    let e = tested(&c, &ctx, id, |e| e.canary.is_some()).await;
    assert_eq!((e.health, e.canary.as_ref().and_then(|c| c.latency_ms)), (endpoints::Health::Ready, Some(152)));
    let sent = node.log.lock().unwrap().iter().find(|c| c.contains("/api/generate")).cloned().unwrap();
    // Only a model in memory, and only for as long as it had left: residency doesn't change.
    assert!(sent.contains(r#""model":"gpt-oss:20b""#) && sent.contains(r#""num_predict":1"#) && sent.contains(r#""stream":false"#), "{sent}");
    let kept: u64 = sent.split(r#""keep_alive":""#).nth(1).and_then(|r| r.split('s').next()).and_then(|n| n.parse().ok()).unwrap();
    assert!((209..=212).contains(&kept), "{kept} s of the 212 it had left: {sent}");

    // Both about to unload (under 30 s left): nothing to test, so it's Idle (neutral, never Down).
    expires.store(now() + 20, std::sync::atomic::Ordering::SeqCst);
    c.models.age(61_000);
    let e = tested(&c, &ctx, id, |e| e.health == endpoints::Health::Idle).await;
    assert_eq!(e.health_reason.as_deref(), Some("Nothing in memory"));
    assert_eq!(node.log.lock().unwrap().iter().filter(|c| c.contains("/api/generate")).count(), 1, "no test that would keep it loaded");
    c.live.stop();
}

#[tokio::test]
async fn key_never_on_command_line() {
    // A real vLLM-shaped server on this Mac, and a fake node whose shell runs the real curl.
    let (base, seen) = fake_upstream::start().await;
    let port: u16 = base.rsplit(':').next().unwrap().parse().unwrap();
    let (node, p1) = fake_node(&temp("canary-key"), real_shell()).await;
    let c = cluster("canary-key", &[("cell1", p1)]).await;
    let mut ctx = c.ctx();
    let head = &mut ctx.snapshot.as_mut().unwrap().nodes[0].containers[0];
    head.command = format!("vllm serve /models/meta-llama/Llama-3.3-70B-Instruct --served-model-name llama-3.3-70b --port {port}");
    head.ports.clear();
    let id = format!("cell1-vllm-{port}");
    let key = "sbk-canary-7f3e9a1c5b2d4e6f8a0b";
    crate::secrets::set(&format!("llm:{id}"), key).unwrap();
    c.models.endpoints_tick(&ctx, true).await;
    let e = tested(&c, &ctx, &id, |e| e.canary.is_some()).await;
    assert!(e.canary.as_ref().is_some_and(|c| c.ok), "{:?}", e.canary);
    let e = tested(&c, &ctx, &id, |e| e.health == endpoints::Health::Ready).await;
    assert!(e.health_reason.is_none());

    // The key reached the server as a header…
    let (path, headers, body) = seen.first("completions").expect("a test token");
    assert_eq!(path, "completions");
    assert_eq!(headers.get("authorization").and_then(|v| v.to_str().ok()), Some(format!("Bearer {key}").as_str()));
    assert_eq!((body["max_tokens"].as_u64(), body["prompt"].as_str(), body["model"].as_str()), (Some(1), Some("Hi"), Some("llama-3.3-70b")));
    // …through stdin: no command the node ran, and nothing in the activity log, has it.
    let raw = node.raw.lock().unwrap().clone();
    assert!(raw.iter().any(|c| c.contains("-H @-") && c.contains("/v1/completions")), "{raw:?}");
    assert!(!raw.iter().chain(node.log.lock().unwrap().iter()).any(|c| c.contains(key)), "the key is on a command line");
    assert!(!c.live.audit.entries().iter().any(|e| e.command.contains(key) || e.title.contains(key) || e.stdout_tail.as_deref().unwrap_or("").contains(key)));
    crate::secrets::clear(&format!("llm:{id}")).unwrap();
    c.live.stop();
}

#[tokio::test]
async fn gpu_fault_downs_member_endpoints() {
    let (models, llm, _live, cluster, _dir) = demo_app("demo-gpu-xid");
    demo_round(&models, &cluster, 1).await;
    cluster.lock().unwrap().set_scenario(Some("gpu-xid")).unwrap();
    // What health::on_publish stamps on gx10-0000 for the latched Xid 79.
    let fault = crate::model::GpuFault { code: Some(79), text: "GPU has fallen off the bus (Xid 79)".into(), since_ms: now_ms(), source: "xid".into() };
    let ctx = || {
        let mut ctx = demo_ctx(&cluster);
        if let Some(n) = ctx.snapshot.as_mut().unwrap().nodes.iter_mut().find(|n| n.id == "gx10-0000") {
            n.gpu_fault = Some(fault.clone());
        }
        ctx
    };
    let mut health = crate::health::Engine::default();
    let mut fired = false;
    let mut eps = Vec::new();
    // Two minutes of rounds, 3 s apart, through the rule engine as on_publish runs it.
    let start = now_ms();
    for i in 0..40u64 {
        cluster.lock().unwrap().advance();
        let ctx = ctx();
        eps = models.endpoints_tick(&ctx, false).await;
        let mut snap = ctx.snapshot.unwrap();
        snap.ts_ms = start + i * 3_000;
        let events = health.step_with(&snap, &crate::health::endpoint_infos(&eps), snap.ts_ms);
        fired |= events.iter().any(|e| e.alert.rule == "endpoint-down" && e.alert.entity.id == "gx10-0000-ollama-11434");
    }
    let o = find(&eps, "gx10-0000-ollama-11434");
    assert_eq!((o.health, o.health_reason.as_deref()), (endpoints::Health::Down, Some("GPU fault on gx10-0000: Xid 79 (fell off the bus)")));
    // Its test token fails the way a CUDA call on a lost GPU does, and it left Chat.
    assert_eq!(o.canary.as_ref().and_then(|c| c.error.as_deref()), Some("HTTP 500: GPU has fallen off the bus"));
    assert!(llm.get("gx10-0000-ollama-11434").await.is_none());
    // Llama on cell1–cell3 serves on: 1 model, 3 of 4 GPUs serving.
    assert_eq!(find(&eps, LLAMA).health, endpoints::Health::Ready);
    let serving: std::collections::BTreeSet<&String> = eps.iter().filter(|e| e.health == endpoints::Health::Ready).flat_map(|e| &e.nodes).collect();
    assert_eq!(serving.len(), 3, "{serving:?}");
    assert!(fired, "endpoint-down fired for Ollama: {:?}", health.view(start + 120_000, None).alerts.iter().map(|a| &a.rule).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_load_that_fails_with_a_refusal_keeps_its_words_not_its_json() {
    // The restart path handed the exit's error straight to `fail`, and the alert printed it:
    // qwen3-1.7b is down: {"kind":"fenced","message":"No fence is open, so changes…
    let (_n, p1) = fake_node(&temp("fail-json"), real_shell()).await;
    let c = cluster("fail-json", &[("cell1", p1)]).await;
    let spec = DeploySpec { id: "fj".into(), nodes: vec!["cell1".into()], ..Default::default() };
    c.models.deployments.lock().unwrap().push(Deployment { spec, state: DeploymentState::Starting, ..Default::default() });
    let refused = crate::errors::SbError::new(crate::errors::ErrorKind::Fenced, "No fence is open, so changes on the cluster are refused.").on("cell1");
    c.models.fail("fj", refused.to_json());
    let saved = c.models.deployments.lock().unwrap().iter().find(|d| d.spec.id == "fj").unwrap().clone();
    assert_eq!((saved.state, saved.message.as_str()), (DeploymentState::Failed, "No fence is open, so changes on the cluster are refused."));
    c.live.stop();
}

/// A refusal on a two-node load comes back through `format!("{n}: {e}")` in plain words.
/// It belongs to the restart task, not to the still-running load or a false Down alert.
#[tokio::test]
async fn a_refused_stop_keeps_its_words_in_the_result_and_preserves_the_load() {
    let answers = "running 0\n200\nINFO Application startup complete.\n";
    let (_a, p1) = fake_node(&temp("worker-json-a"), vllm_script(Arc::new(Mutex::new(vec![answers])), 1)).await;
    let (_b, p2) = fake_node(&temp("worker-json-b"), vllm_script(Arc::new(Mutex::new(vec![answers])), 2)).await;
    let c = cluster("worker-json", &[("cell1", p1), ("cell2", p2)]).await;
    let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
    gate.set_look_only(true).unwrap();
    c.live.set_write_policy(gate.clone());
    let spec = DeploySpec { id: "two".into(), require_key: false, ..spec(&["cell1", "cell2"], 2, 1) };
    c.models.deployments.lock().unwrap().push(Deployment { spec, state: DeploymentState::Ready, message: "Serving".into(), ..Default::default() });
    let result = c.models.restart_ordered(&c.ctx(), "two", "Restart", false).await;
    let saved = c.models.deployments.lock().unwrap().iter().find(|d| d.spec.id == "two").unwrap().clone();
    gate.set_look_only(false).unwrap();
    c.live.stop();
    let e = result.expect_err("look-only refuses the restart");
    assert!(e.starts_with("cell1: ") && !e.contains('{'), "what the task and the log show: {e}");
    assert_eq!((saved.state, saved.generation, saved.message.as_str()), (DeploymentState::Ready, 0, "Serving"));
}

/// A restart of containers SparkBox didn't load: its task line goes through a path of its own.
#[tokio::test]
async fn a_refused_container_stop_keeps_its_words_in_an_outside_restart() {
    let (_a, p1) = fake_node(&temp("outside-json"), real_shell()).await;
    let c = cluster("outside-json", &[("cell1", p1)]).await;
    let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
    gate.set_look_only(true).unwrap();
    c.live.set_write_policy(gate.clone());
    let head = endpoints::Member { node: "cell1".into(), id: "abcdefabcdef".into(), name: "vllm".into(), head: true, ..Default::default() };
    let e = Endpoint { id: "cell1-vllm-8000".into(), nodes: vec!["cell1".into()], members: vec![head.clone()], ..Default::default() };
    let result = c.models.restart_external_in(&c.ctx(), e, head, None, "Restart".into()).await;
    gate.set_look_only(false).unwrap();
    c.live.stop();
    let why = result.expect_err("look-only refuses it");
    assert!(why.starts_with("vllm on cell1: ") && why.contains("look-only") && !why.contains('{'), "the refusal itself, in words: {why}");
}

#[tokio::test]
async fn a_dead_endpoint_is_marked_down_after_two_failed_probes() {
    // curl fails to connect: the probe's `|| printf '\n000'` answers for it.
    let (_n, p1) = fake_node(&temp("eps-dead"), real_shell()).await;
    let c = cluster("eps-dead", &[("cell1", p1)]).await;
    let eps = c.models.endpoints_tick(&c.ctx(), true).await;
    let e = eps.iter().find(|e| e.id == "cell1-vllm-8000").unwrap();
    assert_eq!((e.health, e.health_reason.as_deref()), (endpoints::Health::Loading, Some("Not answering yet (connection refused)")), "one miss isn't an outage");
    let eps = c.models.endpoints_tick(&c.ctx(), true).await;
    let e = eps.iter().find(|e| e.id == "cell1-vllm-8000").unwrap();
    assert_eq!((e.health, e.health_reason.as_deref()), (endpoints::Health::Down, Some("No answer from 127.0.0.1:8000 on cell1 (over SSH)")));
    assert_eq!(e.message, e.health_reason, "the alert says why");
    assert!(c.llm.get("cell1-vllm-8000").await.is_none());
    c.live.stop();
}

#[tokio::test]
async fn a_loading_endpoint_shows_progress_from_its_log_and_an_exit_quotes_the_error() {
    let logs = Arc::new(Mutex::new("INFO Loading safetensors checkpoint shards:  42% Completed | 13/30\n".to_string()));
    let l = logs.clone();
    let script: Script = Arc::new(move |cmd, _| cmd.starts_with("docker logs --tail").then(|| (l.lock().unwrap().clone(), 0)));
    let (node, p1) = fake_node(&temp("eps-loading"), script).await;
    let c = cluster("eps-loading", &[("cell1", p1)]).await;
    // Its collector says the API port isn't listening yet (only Ray's is).
    let mut ctx = c.ctx();
    let snap = ctx.snapshot.as_mut().unwrap();
    snap.nodes[0].listeners = vec![crate::model::Listener { port: 6379, addr: "0.0.0.0".into(), pid: None, process: None, ..Default::default() }];
    let eps = c.models.endpoints_tick(&ctx, true).await;
    let e = eps.iter().find(|e| e.id == "cell1-vllm-8000").unwrap();
    assert_eq!((e.health, e.loading_pct), (endpoints::Health::Loading, Some(42)));
    assert!(!node.log.lock().unwrap().iter().any(|c| c.contains("/v1/models")), "no probe while it isn't listening: a refused one would count as down");

    // It exits: still listed, Down, with the line from its log that says why.
    *logs.lock().unwrap() = "INFO Loading safetensors checkpoint shards:  60% Completed\n\
        ERROR 10-28 12:04:10 [core.py:812] torch.OutOfMemoryError: CUDA out of memory. Tried to allocate 2.00 GiB\n\
        INFO 10-28 12:04:11 [launcher.py:80] Shutting down.\n".into();
    let head = &mut ctx.snapshot.as_mut().unwrap().nodes[0].containers[0];
    head.state = "exited".into();
    head.exit_code = 1;
    head.finished_at = now() - 5;
    let eps = c.models.endpoints_tick(&ctx, false).await;
    let e = eps.iter().find(|e| e.id == "cell1-vllm-8000").unwrap();
    assert_eq!(e.health, endpoints::Health::Down);
    assert_eq!(e.health_reason.as_deref(), Some("Head exited (code 1) just now · torch.OutOfMemoryError: CUDA out of memory. Tried to allocate 2.00 GiB"));
    // The log is read once per exit, not every round.
    c.models.endpoints_tick(&ctx, false).await;
    assert_eq!(node.log.lock().unwrap().iter().filter(|c| c.starts_with("docker logs --tail 30")).count(), 1);
    c.live.stop();
}

// ---- Ollama

#[tokio::test]
async fn ollama_is_managed_over_curl_on_its_node() {
    let script: Script = Arc::new(|cmd, _| {
        if cmd.contains("/api/tags") {
            let tags = r#"{"models":[{"name":"qwen3:8b","size":5225387923,"details":{"family":"qwen3","parameter_size":"8.2B","quantization_level":"Q4_K_M"}}]}"#;
            return Some((format!("{tags}\n--sparkbox--\n{{\"models\":[]}}\n--sparkbox--\n{{\"version\":\"0.12.6\"}}"), 0));
        }
        if cmd.contains("/api/delete") {
            return Some(("\n200".into(), 0));
        }
        if cmd.contains("/api/pull") {
            return Some(("{\"status\":\"pulling manifest\"}\n{\"status\":\"pulling 6a07\",\"total\":100,\"completed\":100}\n{\"status\":\"success\"}\n".into(), 0));
        }
        None
    });
    let (node, p1) = fake_node(&temp("ollama-1"), script).await;
    let c = cluster("ollama", &[("gx10-0000", p1)]).await;
    // A node running Ollama as a service (as its installer sets it up), not in a container.
    let mut ctx = c.ctx();
    let snap = ctx.snapshot.as_mut().unwrap();
    snap.nodes[0].containers.clear();
    snap.nodes[0].processes = vec![crate::model::Process { pid: 812, cmd: "/usr/local/bin/ollama serve".into(), ports: vec![11434], unit: Some("ollama.service".into()), ..Default::default() }];
    c.models.endpoints_tick(&ctx, true).await;
    let id = "gx10-0000-ollama-11434";
    let view = c.models.ollama_list(&ctx, id).await.unwrap();
    assert_eq!(view.version.as_deref(), Some("0.12.6"));
    assert_eq!(view.models[0].name, "qwen3:8b");

    let tasks = crate::tasks::Tasks::new(None);
    let task = c.models.ollama_pull(&ctx, id, "gemma3:27b", tasks.clone()).await.unwrap();
    for _ in 0..100 {
        if tasks.get(&task).is_some_and(|t| t.finished()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let t = tasks.get(&task).unwrap();
    assert_eq!(t.state, crate::tasks::TaskState::Ok, "{t:?}");
    assert!(node.log.lock().unwrap().iter().any(|cmd| cmd.contains(r#"/api/pull -d '{"model":"gemma3:27b","stream":true}'"#)));
    assert!(c.models.ollama_pull(&ctx, id, "x; rm -rf /", tasks).await.is_err());

    c.models.ollama_delete(&ctx, id, "qwen3:8b").await.unwrap();
    assert!(node.log.lock().unwrap().iter().any(|cmd| cmd.contains("-X DELETE http://127.0.0.1:11434/api/delete")));
    // Deleting is a change: it's in the Activity log. Listing isn't.
    let audit = c.live.audit.entries();
    assert!(audit.iter().any(|e| e.command.contains("/api/delete")));
    assert!(!audit.iter().any(|e| e.command.contains("/api/tags")));
    c.live.stop();
}

#[tokio::test]
async fn demo_ollama_pulls_and_deletes() {
    let (models, _llm, live, cluster, _dir) = demo_app("demo-ollama");
    models.endpoints_tick(&demo_ctx(&cluster), false).await;
    let ctx = demo_ctx(&cluster);
    let id = "gx10-0000-ollama-11434";
    let before = models.ollama_list(&ctx, id).await.unwrap();
    assert!(before.models[0].loaded.is_some(), "loaded models first");
    assert!(models.ollama_delete(&ctx, id, "qwen3:32b").await.unwrap_err().contains("in memory"));
    models.ollama_delete(&ctx, id, "llama3.2:3b").await.unwrap();
    assert!(!models.ollama_list(&ctx, id).await.unwrap().models.iter().any(|m| m.name == "llama3.2:3b"));
    assert!(live.audit.entries().iter().any(|e| e.title.contains("Delete llama3.2:3b")));
    models.demo.lock().unwrap().ollama_add("qwen3:8b", 5_225_387_923, 0);
    assert!(models.ollama_list(&ctx, id).await.unwrap().models.iter().any(|m| m.name == "qwen3:8b"));
}

// ---- Benchmarks

#[tokio::test]
async fn a_sweep_measures_what_the_server_streams() {
    let (upstream, seen) = fake_upstream::start().await;
    let client = reqwest::Client::new();
    let spec = bench::BenchSpec { profile: "custom".into(), input_tokens: 64, output_tokens: 20, concurrency: vec![1, 4], requests: 0 };
    let base = format!("{upstream}/v1");
    let one = bench::run_level(&client, &base, Some("sk-x"), "m", &spec, 1, 1).await;
    assert_eq!((one.requests, one.completed, one.failed), (4, 4, 0));
    let ttft = fake_upstream::TTFT_MS as f64;
    assert!(one.ttft_ms.p50 >= ttft && one.ttft_ms.p50 < ttft + 200.0, "{:?}", one.ttft_ms);
    assert!(one.itl_ms.p50 >= fake_upstream::ITL_MS as f64 * 0.8 && one.itl_ms.p50 < 50.0, "{:?}", one.itl_ms);
    assert!(one.tpot_ms.p50 > 0.0 && one.output_tps > 0.0);
    let (path, headers, sent) = seen.last();
    assert_eq!(path, "completions");
    assert_eq!(sent["prompt"].as_array().unwrap().len(), 64, "a prompt of exactly 64 tokens");
    assert_eq!((sent["min_tokens"].as_u64(), sent["ignore_eos"].as_bool()), (Some(20), Some(true)));
    assert_eq!(headers["authorization"], "Bearer sk-x");

    let four = bench::run_level(&client, &base, None, "m", &spec, 4, 2).await;
    assert_eq!(four.completed, 8);
    assert!(four.output_tps > one.output_tps * 1.5, "four at once streams more in total: {} vs {}", four.output_tps, one.output_tps);
}

#[tokio::test]
async fn a_demo_sweep_runs_as_a_task_and_is_kept() {
    let (models, _llm, live, cluster, dir) = demo_app("demo-bench");
    models.endpoints_tick(&demo_ctx(&cluster), false).await;
    let tasks = crate::tasks::Tasks::new(None);
    let spec = bench::BenchSpec { concurrency: vec![1, 2], ..bench::BenchSpec::profile("chat").unwrap() };
    let err = models.run_benchmark(&demo_ctx(&cluster), "gx10-0000-ollama-11434", "gpt-oss:20b", spec.clone(), tasks.clone()).await.unwrap_err();
    assert!(err.contains("Ollama"), "{err}");
    let id = models.run_benchmark(&demo_ctx(&cluster), "cell1-vllm-8000", "llama-3.3-70b", spec, tasks.clone()).await.unwrap();
    for _ in 0..60 {
        if tasks.get(&id).is_some_and(|t| t.finished()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(tasks.get(&id).unwrap().state, crate::tasks::TaskState::Ok);
    let b = models.benchmarks().into_iter().last().unwrap();
    assert_eq!(b.levels.len(), 2);
    assert!((1.4..1.8).contains(&b.levels[0].output_tps), "one stream of a bf16 70B over PP 3: {}", b.levels[0].output_tps);
    assert!(b.external, "the cluster's own Llama, not SparkBox's");
    assert!(live.audit.entries().iter().any(|e| e.title.contains("Benchmark llama-3.3-70b")));
    let csv = models.export_benchmark(b.at, "csv", &dir).unwrap();
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with("model,nodes,tp,pp,profile") && text.lines().count() == 3, "{text}");
    let json = models.export_benchmark(b.at, "json", &dir).unwrap();
    assert!(std::fs::read_to_string(json).unwrap().contains("\"tpotMs\""));
}

// ---- Ollama as found on cell1 and cell3 (r4-serving)

/// A fake cell3 set up like the real one: ollama.service active as user `ollama`, bound to
/// 127.0.0.1:11434, gemma4 on disk, nothing in memory (`ps`, settable). Test tokens are counted.
fn ollama_service_script(ps: Arc<Mutex<String>>) -> Script {
    Arc::new(move |cmd, _| {
        if cmd.contains("/api/tags") {
            let tags = r#"{"models":[{"name":"gemma4:latest","model":"gemma4:latest","modified_at":"2026-04-18T10:12:44.123456789-04:00","size":9556302029,"digest":"c6eb396dbd59","details":{"format":"gguf","family":"gemma4","parameter_size":"8.0B","quantization_level":"Q4_K_M"}}]}"#;
            let unit = if cmd.contains("systemctl show -p ActiveState --value ollama.service") { "\n--sparkbox--\nactive\n" } else { "" };
            return Some((format!("{tags}\n200\n--sparkbox--\n{}\n200{unit}", ps.lock().unwrap()), 0));
        }
        if cmd.contains("/api/generate") {
            return Some((format!("{}\n200 0.152 0\n", include_str!("fixtures/probe/ollama_generate.json").trim()), 0));
        }
        None
    })
}

/// cell3's snapshot as the collector sends it: the listener with no owner (ss without root), and
/// the ollama process only when `sampled` (it's idle, so most samples leave it out).
fn as_found(ctx: &mut Ctx, node: &str, sampled: bool, listening: bool) {
    let n = ctx.snapshot.as_mut().unwrap().nodes.iter_mut().find(|n| n.id == node).unwrap();
    n.containers.clear();
    n.processes = if sampled {
        vec![crate::model::Process { pid: 2175, name: "ollama".into(), user: "ollama".into(), cmd: "/usr/local/bin/ollama serve".into(), unit: Some("ollama.service".into()), ..Default::default() }]
    } else {
        vec![crate::model::Process { pid: 1, name: "systemd".into(), user: "root".into(), cmd: "/sbin/init splash".into(), ..Default::default() }]
    };
    n.listeners = if listening {
        vec![
            crate::model::Listener { port: 22, addr: "0.0.0.0".into(), ..Default::default() },
            crate::model::Listener { port: 11434, addr: "127.0.0.1".into(), pid: None, process: None, ..Default::default() },
        ]
    } else {
        vec![crate::model::Listener { port: 22, addr: "0.0.0.0".into(), ..Default::default() }]
    };
}

fn generates(node: &crate::fake_node::Node) -> usize {
    node.log.lock().unwrap().iter().filter(|c| c.contains("/api/generate")).count()
}

#[tokio::test]
async fn an_idle_loopback_ollama_is_never_down_across_sample_gaps() {
    let ps = Arc::new(Mutex::new(r#"{"models":[]}"#.to_string()));
    let (node, p1) = fake_node(&temp("ollama-gaps"), ollama_service_script(ps)).await;
    let c = cluster("ollama-gaps", &[("cell3", p1)]).await;
    let id = "cell3-ollama-11434";
    let mut seen = Vec::new();
    // 40 polls, 3 s apart. The first 20 as the real cell3: the listener always, the process in
    // one sample of four. Then the worst sampler: neither, in three of four.
    for i in 0..40 {
        let mut ctx = c.ctx();
        as_found(&mut ctx, "cell3", i % 4 == 0, i < 20 || i % 4 == 0);
        let eps = c.models.endpoints_tick(&ctx, i == 0).await;
        let e = eps.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("poll {i}: Ollama on cell3 isn't listed"));
        assert_ne!(e.health, endpoints::Health::Down, "poll {i}: {:?}", e.health_reason);
        assert_ne!(e.health, endpoints::Health::Gone, "poll {i}: a sample gap is no evidence");
        seen.push(e.health);
        c.models.age(3_000);
    }
    assert!(seen.iter().all(|h| *h == endpoints::Health::Idle), "{seen:?}");
    let e = c.models.endpoints().into_iter().find(|e| e.id == id).unwrap();
    assert_eq!(e.health_reason.as_deref(), Some("Nothing in memory"));
    assert_eq!((e.label.as_str(), e.reach.as_str()), ("Ollama on cell3", "node-only"));
    assert_eq!((e.on_disk.clone(), e.on_disk_bytes.clone()), (vec!["gemma4:latest".to_string()], vec![9_556_302_029]));
    assert_eq!(generates(&node), 0, "nothing in memory: never a test token");

    // Gone takes every source missing for three rounds and 30 s, and reads neutral.
    let mut ctx = c.ctx();
    as_found(&mut ctx, "cell3", false, false);
    c.models.endpoints_tick(&ctx, false).await;
    {
        let mut st = c.models.eps.lock().unwrap();
        let t = st.tracks.get_mut(id).unwrap();
        (t.missed, t.missed_since_ms, t.seen_ms) = (5, Some(now_ms() - 31_000), now_ms() - 31_000);
    }
    let e = c.models.endpoints_tick(&ctx, false).await.into_iter().find(|e| e.id == id).unwrap();
    assert_eq!(e.health, endpoints::Health::Gone);
    assert!(e.health_reason.as_deref().is_some_and(|r| r.starts_with("Not found since ")), "{:?}", e.health_reason);
    c.live.stop();
}

#[tokio::test]
async fn a_local_bind_is_probed_through_the_node() {
    let ps = Arc::new(Mutex::new(r#"{"models":[]}"#.to_string()));
    let (node, p1) = fake_node(&temp("ollama-local"), ollama_service_script(ps)).await;
    let c = cluster("ollama-local", &[("cell3", p1)]).await;
    let id = "cell3-ollama-11434";
    let mut ctx = c.ctx();
    as_found(&mut ctx, "cell3", false, true);
    let e = c.models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).expect("found from its listener alone");
    // Asked on cell3 itself, over SSH, at 127.0.0.1: the model list, what's in memory, and the unit.
    let probe = node.log.lock().unwrap().iter().find(|c| c.contains("/api/tags")).cloned().expect("probed on the node");
    assert!(probe.contains("http://127.0.0.1:11434/api/tags") && probe.contains("http://127.0.0.1:11434/api/ps"), "{probe}");
    assert!(probe.contains("systemctl show -p ActiveState --value ollama.service"), "{probe}");
    assert_eq!(e.url, "http://127.0.0.1:11434/v1", "a loopback server's URL is the node's own");
    assert_eq!(e.reach, "node-only");
    assert!(c.llm.get(id).await.is_none(), "not a provider: nothing on this Mac asks it");

    // A node where nothing answers on 11434 (curl is refused, so the probe's own fallbacks
    // print 000) and the unit is inactive: Down needs two failed probes on that path, and says
    // which path. (Not the real curl: this Mac may run an Ollama of its own on 11434.)
    let refused: Script = Arc::new(|cmd, _| cmd.contains("/api/tags").then(|| ("\n000\n--sparkbox--\n\n000\n--sparkbox--\ninactive\n".to_string(), 0)));
    let (_n2, p2) = fake_node(&temp("ollama-local-2"), refused).await;
    let c2 = cluster("ollama-local-2", &[("cell3", p2)]).await;
    let mut ctx2 = c2.ctx();
    as_found(&mut ctx2, "cell3", true, true);
    let e = c2.models.endpoints_tick(&ctx2, true).await.into_iter().find(|e| e.id == id).unwrap();
    assert_ne!(e.health, endpoints::Health::Down, "one miss isn't an outage");
    let e = c2.models.endpoints_tick(&ctx2, true).await.into_iter().find(|e| e.id == id).unwrap();
    assert_eq!((e.health, e.health_reason.as_deref()), (endpoints::Health::Down, Some("No answer from 127.0.0.1:11434 on cell3 (over SSH)")));
    c.live.stop();
    c2.live.stop();
}

#[tokio::test]
async fn ollama_with_nothing_in_memory_is_idle_and_never_tested() {
    let ps = Arc::new(Mutex::new(r#"{"models":[]}"#.to_string()));
    let (node, p1) = fake_node(&temp("ollama-idle"), ollama_service_script(ps.clone())).await;
    let c = cluster("ollama-idle", &[("cell1", p1)]).await;
    let id = "cell1-ollama-11434";
    let mut ctx = c.ctx();
    as_found(&mut ctx, "cell1", true, true);
    let e = c.models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == id).unwrap();
    assert_eq!(e.health, endpoints::Health::Idle, "Idle from the first answer, never Checking that it answers…: {:?}", e.health_reason);
    // Minutes pass: still idle, and no test token (one would load a model).
    for _ in 0..5 {
        c.models.age(61_000);
        let e = c.models.endpoints_tick(&ctx, false).await.into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.health, endpoints::Health::Idle);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(generates(&node), 0);

    // Something is loaded (by someone else): Ready once a token comes back, tested with what it has left.
    let at = now() + 240;
    let (y, mo, d, h, mi, s, _) = crate::demo::ops::civil(at);
    *ps.lock().unwrap() = format!(r#"{{"models":[{{"name":"gemma4:latest","model":"gemma4:latest","size":9556302029,"size_vram":9556302029,"expires_at":"{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z"}}]}}"#);
    c.models.age(61_000);
    let e = tested(&c, &ctx, id, |e| e.health == endpoints::Health::Ready).await;
    assert!(e.canary.as_ref().is_some_and(|c| c.ok));
    assert_eq!(generates(&node), 1);
    let sent = node.log.lock().unwrap().iter().find(|c| c.contains("/api/generate")).cloned().unwrap();
    assert!(sent.contains(r#""model":"gemma4:latest""#), "only what's in memory: {sent}");

    // Unloaded again: idle, and the tests stop.
    *ps.lock().unwrap() = r#"{"models":[]}"#.to_string();
    for _ in 0..3 {
        c.models.age(61_000);
        c.models.endpoints_tick(&ctx, false).await;
    }
    let e = c.models.endpoints().into_iter().find(|e| e.id == id).unwrap();
    assert_eq!(e.health, endpoints::Health::Idle);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(generates(&node), 1, "no test once nothing is in memory");
    c.live.stop();
}

// ---- The GLM-5.3 postmortem on a fake cell1 (r4-serving)

#[tokio::test]
async fn logs_are_read_once_per_container() {
    let home = temp("pm-cell1");
    std::fs::create_dir_all(home.join("glm53")).unwrap();
    std::fs::write(home.join("glm53/inner.sh"), include_str!("fixtures/logs/glm53-inner.sh")).unwrap();
    let glm = home.join("glm53").display().to_string();
    let inspect = json!([{
        "Id": "7da4d848a41aefe02d3543e6",
        "Name": "/glm53",
        "Config": { "Env": ["SERVED_NAME=glm-5.3-flash", "MODEL_LOCAL_NAME=GLM-5.3-Flash-NVFP4", "API_KEY=sk-not-for-tests", "NNODES=4", "HEAD_IP=192.168.100.1", "MASTER_PORT=29501", "MTP_TOKENS=5"] },
        "Mounts": [{ "Type": "bind", "Source": glm, "Destination": "/glm53", "RW": false }],
    }])
    .to_string();
    let script: Script = Arc::new(move |cmd, _| {
        if cmd.starts_with("docker inspect --type container 7da4d848a41a") {
            return Some((inspect.clone(), 0));
        }
        if cmd == "docker logs --tail 600 7da4d848a41a 2>&1" {
            return Some((include_str!("fixtures/logs/glm53-rank0.log").into(), 0));
        }
        None
    });
    let (node, p1) = fake_node(&home, script).await;
    let c = cluster("pm", &[("cell1", p1)]).await;
    // The tour's snapshot: glm53 on all four nodes, cell1's rank 0 the head.
    let mut ctx = c.ctx();
    ctx.snapshot = Some(serde_json::from_str(include_str!("fixtures/snapshot-tour-2.json")).unwrap());
    let d = c.models.cluster_deployments(&ctx).into_iter().find(|d| d.id == "glm53").unwrap();
    assert!(d.postmortem.is_none(), "nothing is read until it's shown");
    assert_eq!(c.models.postmortem_reads(), 0);
    let before = node.log.lock().unwrap().len();
    for _ in 0..3 {
        c.models.read_postmortems(&ctx).await;
    }
    assert_eq!(c.models.postmortem_reads(), 1);
    assert_eq!(node.log.lock().unwrap().iter().filter(|c| c.contains("docker logs")).count(), 1, "one log read per container and exit");
    let d = c.models.cluster_deployments(&ctx).into_iter().find(|d| d.id == "glm53").unwrap();
    assert_eq!(d.label, "GLM-5.3-Flash-NVFP4", "from the head's environment");
    assert_eq!(d.served.as_deref(), Some("glm-5.3-flash"));
    let pm = d.postmortem.expect("read");
    assert_eq!(pm.title.as_deref(), Some("No attention kernel for a bf16 KV cache on GB10"));
    assert_eq!(pm.script_line.as_deref(), Some(format!("{glm}/inner.sh line 22: --kv-cache-dtype bfloat16").as_str()));
    // Only reads ran on the node: inspect, logs and sparkjob.py read (after SparkBox's own
    // helper is checked, and put in ~/.sparkbox when its checksum differs).
    let ran: Vec<String> = node.log.lock().unwrap()[before..].to_vec();
    for cmd in &ran {
        assert!(cmd.starts_with("docker inspect --type container ") || cmd == "docker logs --tail 600 7da4d848a41a 2>&1" || cmd.contains("sparkjob.py read ") || cmd.contains(".sparkbox"), "{cmd}");
    }
    assert!(ran.iter().any(|c| c.contains("sparkjob.py read") && c.contains("inner.sh")), "{ran:?}");
    // A new exit of the same container is a new postmortem.
    let snap = ctx.snapshot.as_mut().unwrap();
    for c in snap.nodes.iter_mut().flat_map(|n| n.containers.iter_mut()).filter(|c| c.name == "glm53") {
        c.finished_at += 3600;
        c.started_at += 3590;
    }
    c.models.read_postmortems(&ctx).await;
    assert_eq!(c.models.postmortem_reads(), 2);
    c.live.stop();
}

// ---- The as-found world (DemoCluster::as_found), as the Live tour saw the cluster (r4-serving)

#[tokio::test]
async fn postmortem_transient_reads_back_off_are_single_flight_and_retry_manually() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0)); let reads = calls.clone();
    let (node, port) = fake_node(&temp("p2-pm-retry-home"), Arc::new(move |cmd, _| {
        if cmd.starts_with("docker inspect") { return Some(("[]".into(), 0)); }
        if cmd.starts_with("docker logs") {
            let n = reads.fetch_add(1, Ordering::SeqCst);
            return Some((if n < 4 { "Docker is temporarily busy" } else { include_str!("fixtures/logs/glm53-rank0.log") }.into(), if n < 4 {1} else {0}));
        } None
    })).await;
    let c = cluster("p2-pm-retry", &[("cell1", port)]).await;
    let mut ctx = c.ctx(); ctx.snapshot = Some(serde_json::from_str(include_str!("fixtures/snapshot-tour-2.json")).unwrap());
    for delay in [30_000,120_000,600_000,u64::MAX] {
        let before = now_ms();
        let (a,b) = tokio::join!(c.models.read_postmortems(&ctx), c.models.read_postmortems(&ctx)); assert!(a || b);
        assert!(!c.models.read_postmortems(&ctx).await);
        let mut cache = c.models.postmortems.lock().unwrap();
        let (_, due) = cache.retries.values_mut().next().unwrap();
        if delay == u64::MAX { assert_eq!(*due, delay); } else { assert!(*due >= before + delay && *due <= now_ms() + delay); *due = 0; }
    }
    assert_eq!(calls.load(Ordering::SeqCst),4);
    c.models.retry_postmortem(&ctx,"glm53"); assert!(c.models.read_postmortems(&ctx).await);
    assert_eq!(calls.load(Ordering::SeqCst),5); assert!(!c.models.read_postmortems(&ctx).await);
    let d = c.models.cluster_deployments(&ctx).into_iter().find(|d|d.id=="glm53").unwrap(); assert!(!d.postmortem.unwrap().retryable);
    assert_eq!(node.log.lock().unwrap().iter().filter(|s|s.starts_with("docker logs")).count(),5);
    c.live.stop();
}

#[tokio::test]
async fn as_found_the_ollamas_rest_and_glm53_failed_for_its_real_reason() {
    let (models, llm, _live, cluster, _dir) = demo_app("demo-as-found");
    *cluster.lock().unwrap() = DemoCluster::as_found();
    // Two minutes of rounds: Ollama on cell1 and cell3 stays idle and node-only in every one.
    for i in 0..40 {
        let eps = demo_round(&models, &cluster, 3).await;
        for id in ["cell1-ollama-11434", "cell3-ollama-11434"] {
            let e = find(&eps, id);
            assert_eq!(e.health, endpoints::Health::Idle, "round {i}, {id}: {:?}", e.health_reason);
            assert_eq!(e.reach, "node-only");
            assert!(e.canary.is_none(), "nothing in memory: never a test token");
            assert!(e.ollama.is_empty(), "round {i}, {id}: nothing in memory, not the everyday world's models: {:?}", e.ollama);
        }
        assert!(find(&eps, "cell1-ollama-11434").models.is_empty(), "round {i}: cell1's Ollama has no models");
        assert_eq!(find(&eps, "cell3-ollama-11434").models, ["gemma4:latest"], "round {i}");
        assert!(eps.iter().all(|e| e.health != endpoints::Health::Down), "round {i}: {:?}", eps.iter().map(|e| (&e.id, e.health)).collect::<Vec<_>>());
    }
    let eps = models.endpoints();
    assert_eq!((find(&eps, "cell1-ollama-11434").label.as_str(), find(&eps, "cell3-ollama-11434").label.as_str()), ("Ollama on cell1", "Ollama on cell3"));
    assert_eq!(find(&eps, "cell3-ollama-11434").on_disk, ["gemma4:latest"]);
    assert!(find(&eps, "cell1-ollama-11434").on_disk.is_empty());
    assert!(llm.get("cell3-ollama-11434").await.is_none(), "bound to 127.0.0.1: Chat on this Mac can't use it");

    // The 4-node GLM-5.3 run of Oct 26 is one failed deployment, and its postmortem is the real one.
    let ctx = demo_ctx(&cluster);
    let list = models.cluster_deployments(&ctx);
    assert_eq!(list.iter().map(|d| (d.id.as_str(), d.state.as_str(), d.nodes.len())).collect::<Vec<_>>(), [("glm53", "failed", 4)]);
    assert!(models.read_postmortems(&ctx).await);
    let d = models.cluster_deployments(&ctx).remove(0);
    let pm = d.postmortem.expect("read from the Demo's copy of the real log");
    assert_eq!(pm.title.as_deref(), Some("No attention kernel for a bf16 KV cache on GB10"));
    assert_eq!(pm.script_line.as_deref(), Some("~/glm53/inner.sh line 22: --kv-cache-dtype bfloat16"));
    assert_eq!((d.exit_code, d.ran_s), (Some(1), Some(54)));
}

#[tokio::test]
async fn leaving_the_everyday_world_its_servers_go_quietly() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-world-switch");
    for _ in 0..3 {
        demo_round(&models, &cluster, 3).await;
    }
    assert_eq!(find(&models.endpoints(), LLAMA).health, endpoints::Health::Ready);
    // The as-found world has no Llama and no Ollama on gx10-0000: nothing answers for them now.
    cluster.lock().unwrap().set_scenario(Some("as-found")).unwrap();
    let mut seen = Vec::new();
    for _ in 0..6 {
        models.age(10_000);
        let eps = demo_round(&models, &cluster, 3).await;
        seen.push((find(&eps, LLAMA).health, find(&eps, "gx10-0000-ollama-11434").health));
    }
    assert!(seen.iter().all(|(a, b)| *a != endpoints::Health::Down && *b != endpoints::Health::Down), "{seen:?}");
    assert_eq!(seen.last(), Some(&(endpoints::Health::Gone, endpoints::Health::Gone)), "{seen:?}");
    let e = find(&models.endpoints(), LLAMA).clone();
    assert!(e.health_reason.as_deref().is_some_and(|r| r.starts_with("Not found since ")), "{:?}", e.health_reason);
    // Their simulated load is gone from the nodes too: the as-found GPUs are idle.
    for _ in 0..30 {
        cluster.lock().unwrap().advance();
    }
    let snap = cluster.lock().unwrap().snapshot();
    for n in &snap.nodes {
        assert!(n.gpu.util_pct < 5.0, "{} GPU at {}%", n.id, n.gpu.util_pct);
    }
}

#[tokio::test]
async fn the_as_found_library_detail_reads_glm53s_real_config() {
    let (models, _llm, _live, cluster, _dir) = demo_app("demo-lib-detail");
    *cluster.lock().unwrap() = DemoCluster::as_found();
    demo_round(&models, &cluster, 1).await;
    let ctx = demo_ctx(&cluster);
    let d = models.library_detail(&ctx, "/home/sparkuser/models/GLM-5.3-Flash-NVFP4").await;
    assert_eq!(d.architectures, ["Glm5NextForConditionalGeneration"]);
    assert_eq!(d.quantization.as_deref(), Some("nvfp4"));
    assert!(d.arch.sparse_mla, "{:?}", d.arch);
    assert!(d.arch.layers > 0 && d.weight_bytes > 180 * (1u64 << 30), "{} layers, {} bytes", d.arch.layers, d.weight_bytes);
}

// ---- p1r1 look-only: test tokens are held, not failed

/// Look-only that never asks for Touch ID; turning it off answers yes.
struct Yes;

impl crate::admin::Authenticator for Yes {
    fn authenticate(&self, _reason: &str) -> Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
async fn look_only_holds_test_tokens_and_a_healthy_model_stays_ready() {
    let ok = format!("{}\n200 0.084 0\n", include_str!("fixtures/probe/completion.json").trim());
    let gen = Arc::new(std::sync::atomic::AtomicU64::new(184_211));
    let (node, p1) = fake_node(&temp("canary-look-only"), engine_script(gen, 0, 0.0, Arc::new(Mutex::new(ok)))).await;
    let c = cluster("canary-look-only", &[("cell1", p1)]).await;
    let id = "cell1-vllm-8000";
    // A real test token first: Ready, proven.
    c.models.endpoints_tick(&c.ctx(), true).await;
    tested(&c, &c.ctx(), id, |e| e.health == endpoints::Health::Ready && e.health_note.is_none()).await;
    assert_eq!(completions(&node), 1);

    // Look-only on, the way Settings turns it on: the watch and the exit both see it.
    let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
    gate.set_look_only(true).unwrap();
    c.models.set_gate(gate.clone());
    c.live.set_write_policy(gate.clone());
    // Five minutes of rounds: the proof runs out, no test token goes, and it stays Ready.
    for round in 0..20 {
        c.models.age(16_000);
        let e = c.models.endpoints_tick(&c.ctx(), round % 4 == 0).await.into_iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.health, endpoints::Health::Ready, "round {round}: {:?}", e.health_reason);
        if round >= 8 {
            assert_eq!(e.health_note.as_deref(), Some("Not tested: look-only mode holds test tokens"), "round {round}");
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(completions(&node), 1, "no test token while look-only holds them");
    let refused: Vec<_> = c.live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect();
    assert!(refused.is_empty(), "nothing was even asked of the exit: {refused:?}");

    // Look-only off: the next test token goes out and proves it again.
    gate.set_look_only(false).unwrap();
    // (The next round with a fresh model list sends it.)
    c.models.age(16_000);
    let e = tested(&c, &c.ctx(), id, |e| e.health == endpoints::Health::Ready && e.health_note.is_none() && e.canary.as_ref().is_some_and(|k| k.ok)).await;
    assert_eq!(e.health_reason, None);
    assert_eq!(completions(&node), 2);
    c.live.stop();
}

#[tokio::test]
async fn a_test_token_the_exit_holds_is_not_a_failure() {
    // The race: the round asked for a token, then look-only came on before it left. The exit
    // refuses it; the endpoint is held, never Loading or Down for it.
    let ok = format!("{}\n200 0.084 0\n", include_str!("fixtures/probe/completion.json").trim());
    let gen = Arc::new(std::sync::atomic::AtomicU64::new(184_211));
    let (node, p1) = fake_node(&temp("canary-held-race"), engine_script(gen, 0, 0.0, Arc::new(Mutex::new(ok)))).await;
    let c = cluster("canary-held-race", &[("cell1", p1)]).await;
    let id = "cell1-vllm-8000";
    // Only the exit is look-only: the watch still asks for test tokens.
    let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
    gate.set_look_only(true).unwrap();
    c.live.set_write_policy(gate.clone());
    c.models.endpoints_tick(&c.ctx(), true).await;
    for round in 0..6 {
        c.models.age(16_000);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let e = c.models.endpoints_tick(&c.ctx(), false).await.into_iter().find(|e| e.id == id).unwrap();
        if round > 0 {
            assert_eq!((e.health, e.health_note.as_deref()), (endpoints::Health::Ready, Some("Not tested: look-only mode holds test tokens")), "round {round}: {:?}", e.health_reason);
        }
        assert!(e.canary.is_none(), "a held token isn't recorded as a result: {:?}", e.canary);
    }
    assert_eq!(completions(&node), 0, "the exit refused every one");
    let refused: Vec<_> = c.live.audit.entries().into_iter().filter(|e| e.title == "Refused").collect();
    assert_eq!(refused.len(), 1, "one background line for the same command in 10 minutes: {refused:?}");
    c.live.stop();
}

// ---- P1 r1 fix (P1.2): unload and pre-clean touch only this instance's containers

/// A docker stand-in on the node's PATH, enough for a load's containers: `ps -a` with label
/// filters (ANDed, as docker does) and the unload's `--format`, `ps -aq`, `stop -t 30` and `rm`.
/// The containers are lines "id instance deployment" in ~/containers ("-" for no instance
/// label); none of them runs, so `stop` has nothing to do.
const DOCKER_SHIM: &str = r#"db="$HOME/containers"
touch "$db"
case "$1" in
ps)
  shift; quiet=0; fmt=""; inst=""; dep=""
  while [ $# -gt 0 ]; do
    case "$1" in
      -a) ;;
      -aq|-q) quiet=1 ;;
      --filter) shift
        case "$1" in
          label=sparkbox.instance=*) inst="${1#label=sparkbox.instance=}" ;;
          label=sparkbox.deployment=*) dep="${1#label=sparkbox.deployment=}" ;;
          *) echo "unsupported filter: $1" >&2; exit 2 ;;
        esac ;;
      --format) shift; fmt="$1"
        [ "$fmt" = '{{.ID}} {{.Label "sparkbox.instance"}}' ] || { echo "unsupported format: $fmt" >&2; exit 2; } ;;
      *) echo "unsupported: $1" >&2; exit 2 ;;
    esac
    shift
  done
  while read -r id i d; do
    [ -n "$dep" ] && [ "$d" != "$dep" ] && continue
    [ -n "$inst" ] && [ "$i" != "$inst" ] && continue
    [ "$i" = "-" ] && i=""
    if [ "$quiet" = 1 ]; then echo "$id"; else echo "$id $i"; fi
  done < "$db"
  ;;
stop)
  shift; [ "$1" = "-t" ] && shift 2
  for id in "$@"; do echo "$id"; done
  ;;
rm)
  shift; [ "$1" = "-f" ] && shift
  for id in "$@"; do grep -v "^$id " "$db" > "$db.new"; mv "$db.new" "$db"; done
  ;;
*) echo "unsupported: $*" >&2; exit 2 ;;
esac
"#;

/// The reads an unload makes once the containers are gone, as an idle Ubuntu node answers them
/// (a Mac has no /proc, ss or /dev/shm).
fn after_unload_reads() -> Script {
    Arc::new(|cmd, _| match cmd {
        "grep MemAvailable /proc/meminfo" => Some(("MemAvailable:   122601504 kB\n".into(), 0)),
        "ls -la /dev/shm" => Some(("total 0\ndrwxrwxrwt  2 root root   40 Oct 29 09:33 .\ndrwxr-xr-x 20 root root 4600 Oct 29 09:33 ..\n".into(), 0)),
        c if c.starts_with("ss -Hltn 'sport = :") => Some((String::new(), 0)),
        _ => None,
    })
}

/// A Ready load saved before SparkBox starts, so `unload` finds it.
async fn saved_load(name: &str, id: &str, containers: &str) -> (Cluster, crate::fake_node::Node) {
    let (node, port) = fake_node(&temp(&format!("{name}-node")), after_unload_reads()).await;
    node.bin("docker", DOCKER_SHIM);
    std::fs::write(node.home.join("containers"), containers).unwrap();
    let dir = temp(&format!("models-{name}-app"));
    let d = Deployment {
        spec: DeploySpec { id: id.into(), ..spec(&["cell1"], 1, 1) },
        endpoint: "http://127.0.0.1:8001/v1".into(),
        state: DeploymentState::Ready,
        stage: None,
        message: String::new(),
        log_tail: String::new(),
        started_at: 1,
        provider_id: String::new(),
        demo: false,
        generation: 0,
        ..Default::default()
    };
    std::fs::write(dir.join("deployments.json"), serde_json::to_string(&vec![d]).unwrap()).unwrap();
    let (live, inventory) = crate::fake_node::connect(&dir, &[("cell1", port)]).await;
    let vpn = Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {});
    let llm = Llm::new(dir.join("providers.json"), vpn);
    let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
    (Cluster { models, live, llm, inventory, dir }, node)
}

fn left(node: &crate::fake_node::Node) -> String {
    std::fs::read_to_string(node.home.join("containers")).unwrap()
}

fn state_of(c: &Cluster, id: &str) -> (DeploymentState, String) {
    let d = c.models.deployments(&c.ctx()).into_iter().find(|d| d.spec.id == id).unwrap();
    (d.state, d.message)
}

/// Waits up to `secs` for the load `id` to satisfy `done`.
async fn until_load_for(c: &Cluster, id: &str, secs: u64, done: impl Fn(&Deployment) -> bool) -> Deployment {
    for _ in 0..secs * 10 {
        if let Some(d) = c.models.deployments(&c.ctx()).into_iter().find(|d| d.spec.id == id).filter(|d| done(d)) {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{id}: {:?}", c.models.deployments(&c.ctx()));
}

/// Waits until the load `id` satisfies `done` (an unload runs as a task beside the test).
async fn until_load(c: &Cluster, id: &str, done: impl Fn(&Deployment) -> bool) -> Deployment {
    for _ in 0..300 {
        if let Some(d) = c.models.deployments(&c.ctx()).into_iter().find(|d| d.spec.id == id).filter(|d| done(d)) {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{id}: {:?}", c.models.deployments(&c.ctx()));
}

#[tokio::test]
async fn unload_and_pre_clean_remove_only_this_instances_containers() {
    let mine = crate::identity::instance_name();
    let other = if mine == "dev" { "release" } else { "dev" };

    // The pre-clean (and unload) command against a docker that ANDs label filters: of a dev and
    // a release container of the same load, only this instance's goes.
    let (c, node) = saved_load("unload-preclean", "u-pre", &format!("aaa111 {mine} u-pre\nbbb222 {other} u-pre\nccc333 {mine} another\n")).await;
    c.live.run("cell1", &plan::unload_command("u-pre"), None, Duration::from_secs(10)).await.unwrap();
    assert_eq!(left(&node), format!("bbb222 {other} u-pre\nccc333 {mine} another\n"), "the other instance's container and another load's stay");
    c.live.stop();

    // Unload of a load the other SparkBox has containers of: refused before anything is removed.
    let (c, node) = saved_load("unload-other", "u-other", &format!("aaa111 {mine} u-other\nbbb222 {other} u-other\n")).await;
    let e = c.models.unload(&c.ctx(), "u-other").await.unwrap_err();
    assert_eq!(e, "qwen3.8-27b's containers on cell1 are managed by the other SparkBox; nothing was stopped.");
    assert_eq!(left(&node).lines().count(), 2, "nothing removed");
    assert_eq!(state_of(&c, "u-other").0, DeploymentState::Ready, "still Ready: it may still be serving");
    c.live.stop();

    // One started before instance labels: whose it is can't be told, so it's left too.
    let (c, node) = saved_load("unload-unlabelled", "u-old", "aaa111 - u-old\n").await;
    let e = c.models.unload(&c.ctx(), "u-old").await.unwrap_err();
    assert!(e.contains("have no SparkBox instance label") && e.ends_with("nothing was stopped. Remove them in Containers."), "{e}");
    assert_eq!(left(&node), "aaa111 - u-old\n");
    c.live.stop();

    // Only this instance's: stopped, removed, confirmed gone, then Stopped with what came back
    // (a load saved before baselines were recorded says so).
    let (c, node) = saved_load("unload-mine", "u-mine", &format!("aaa111 {mine} u-mine\nddd444 {mine} u-mine\n")).await;
    c.models.unload(&c.ctx(), "u-mine").await.unwrap();
    let d = until_load(&c, "u-mine", |d| d.unload_report.is_some()).await;
    assert_eq!(left(&node), "");
    assert_eq!(d.state, DeploymentState::Stopped);
    assert_eq!(d.message, "Unloaded from cell1 · memory: no baseline recorded (116.9 GiB available) · port 8001 closed · /dev/shm: no baseline recorded");
    let log = node.log.lock().unwrap().clone();
    let listed = log.iter().filter(|l| l.starts_with("docker ps -a --filter label=sparkbox.deployment=u-mine --format")).count();
    assert_eq!(listed, 2, "listed before removing, and again to confirm: {log:?}");
    c.live.stop();

    // Nothing of it running: said so, not "Unloaded", and marked stopped.
    let (c, _node) = saved_load("unload-none", "u-none", "").await;
    c.models.unload(&c.ctx(), "u-none").await.unwrap();
    let d = until_load(&c, "u-none", |d| d.unload_report.is_some()).await;
    assert_eq!(d.state, DeploymentState::Stopped);
    assert!(d.message.starts_with("No containers of qwen3.8-27b from this SparkBox were running; it's marked stopped · Unloaded from cell1 · "), "{}", d.message);
    c.live.stop();

    // A container that won't go: not marked stopped.
    let (c, node) = saved_load("unload-stuck", "u-stuck", &format!("aaa111 {mine} u-stuck\n")).await;
    node.bin("docker", &DOCKER_SHIM.replace("rm)\n  shift;", "rm)\n  exit 1; shift;"));
    c.models.unload(&c.ctx(), "u-stuck").await.unwrap();
    let d = until_load(&c, "u-stuck", |d| !d.message.is_empty()).await;
    assert_eq!(d.message, "Its containers on cell1 are still there; it isn't marked stopped.");
    assert_eq!(state_of(&c, "u-stuck").0, DeploymentState::Ready);
    c.live.stop();
}

// ---- An idle Ollama asked once a minute (P1.47): one busy or unanswered ask is no evidence

const IDLE_OLLAMA: &str = "cell3-ollama-11434";

/// One round as the app runs it on cell3 as found: its idle Ollama, and the snapshot it saw.
async fn idle_round(c: &Cluster) -> (Endpoint, crate::model::ClusterSnapshot) {
    let mut ctx = c.ctx();
    as_found(&mut ctx, "cell3", true, true);
    let e = c.models.endpoints_tick(&ctx, false).await.into_iter().find(|e| e.id == IDLE_OLLAMA).unwrap();
    (e, ctx.snapshot.unwrap())
}

/// cell3's idle Ollama, answered at the first round and then left alone for 57 s of 3 s rounds
/// (the idle schedule asks nothing meanwhile). The clock then stands at the round that crosses
/// the minute, 1 s late, past Live's 2 s reuse of the last read: that round asks.
async fn idle_ollama_a_minute_on(name: &str, script: Script) -> (crate::fake_node::Node, Cluster) {
    let (node, p1) = fake_node(&temp(name), script).await;
    let c = cluster(name, &[("cell3", p1)]).await;
    let mut ctx = c.ctx();
    as_found(&mut ctx, "cell3", true, true);
    let e = c.models.endpoints_tick(&ctx, true).await.into_iter().find(|e| e.id == IDLE_OLLAMA).unwrap();
    assert_eq!(e.health, endpoints::Health::Idle, "{:?}", e.health_reason);
    let asked = || node.log.lock().unwrap().iter().filter(|c| c.contains("/api/tags")).count();
    let before = asked();
    for _ in 0..19 {
        c.models.age(3_000);
        let (e, _) = idle_round(&c).await;
        assert_eq!(e.health, endpoints::Health::Idle, "{:?}", e.health_reason);
    }
    assert_eq!(asked(), before, "not asked within the minute");
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    c.models.age(4_000);
    (node, c)
}

fn tags_asked(node: &crate::fake_node::Node) -> usize {
    node.log.lock().unwrap().iter().filter(|c| c.contains("/api/tags")).count()
}

/// cell3's Ollama, whose probe gets no answer from curl while `fail` is set (Ollama restarting).
fn ollama_that_fails(fail: Arc<std::sync::atomic::AtomicBool>) -> Script {
    let inner = ollama_service_script(Arc::new(Mutex::new(r#"{"models":[]}"#.to_string())));
    Arc::new(move |cmd, stdin| {
        if fail.load(std::sync::atomic::Ordering::SeqCst) && cmd.contains("/api/tags") {
            return Some(("\n000\n--sparkbox--\n\n000\n--sparkbox--\nactive\n".to_string(), 0));
        }
        inner(cmd, stdin)
    })
}

/// The minute's ask finds every SSH channel busy: no evidence about the model. It stays Idle with
/// the note, is asked again on the 15 s schedule, and is back to once a minute once it answers.
#[tokio::test]
async fn an_idle_ollama_whose_ask_finds_the_channels_busy_stays_idle_and_is_asked_again_soon() {
    use std::sync::atomic::Ordering;
    let ps = Arc::new(Mutex::new(r#"{"models":[]}"#.to_string()));
    let (node, c) = idle_ollama_a_minute_on("idle-ollama-busy", ollama_service_script(ps)).await;
    let before = tags_asked(&node);
    node.knobs.refuse_next.store(100_000, Ordering::SeqCst);
    let (e, _) = idle_round(&c).await;
    node.knobs.refuse_next.store(0, Ordering::SeqCst);
    assert_eq!(tags_asked(&node), before, "the busy ask never reached the node");
    assert_eq!(e.health, endpoints::Health::Idle, "{:?}", e.health_reason);
    let note = e.health_note.clone().unwrap_or_default();
    assert!(note.starts_with("Not checked since ") && note.ends_with(": SSH channels busy on cell3"), "{note}");
    let mut seen = Vec::new();
    for _ in 0..15 {
        c.models.age(3_000);
        let (e, _) = idle_round(&c).await;
        seen.push((e.health, e.health_reason.clone()));
    }
    assert!(seen.iter().all(|(h, _)| *h == endpoints::Health::Idle), "{seen:?}");
    assert_eq!(tags_asked(&node) - before, 1, "asked again within 15 s, then once a minute");
    let (e, _) = idle_round(&c).await;
    assert_eq!(e.health_note, None, "answered since, and at rest no test is owed");
    c.live.stop();
}

/// The minute's ask gets no answer once (Ollama restarting): one failed ask isn't Down. It's
/// asked again within 15 s, answers, and stays Idle throughout.
#[tokio::test]
async fn an_idle_ollama_that_misses_one_ask_stays_idle_and_is_asked_again_soon() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fail = Arc::new(AtomicBool::new(false));
    let (node, c) = idle_ollama_a_minute_on("idle-ollama-miss", ollama_that_fails(fail.clone())).await;
    let before = tags_asked(&node);
    fail.store(true, Ordering::SeqCst);
    let (e, _) = idle_round(&c).await;
    fail.store(false, Ordering::SeqCst);
    assert_eq!(tags_asked(&node), before + 1, "asked");
    assert_eq!(e.health, endpoints::Health::Idle, "one failed ask: {:?}", e.health_reason);
    // Past Live's 2 s reuse of that read, as the app's rounds are.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    let mut seen = Vec::new();
    for _ in 0..15 {
        c.models.age(3_000);
        let (e, _) = idle_round(&c).await;
        seen.push((e.health, e.health_reason.clone()));
    }
    assert!(seen.iter().all(|(h, _)| *h == endpoints::Health::Idle), "{seen:?}");
    assert_eq!(tags_asked(&node) - before, 2, "the failed ask, and one more within 15 s");
    c.live.stop();
}

/// The rule engine as on_publish runs it, fed every round: the idle Ollama's endpoint-down events.
struct Pager {
    health: crate::health::Engine,
    start: u64,
    fired: Vec<String>,
}

impl Pager {
    fn new() -> Pager {
        Pager { health: crate::health::Engine::default(), start: now_ms(), fired: Vec::new() }
    }

    fn step(&mut self, e: &Endpoint, snap: &crate::model::ClusterSnapshot, t_ms: u64) {
        let mut snap = snap.clone();
        snap.ts_ms = self.start + t_ms;
        for ev in self.health.step_with(&snap, &crate::health::endpoint_infos(std::slice::from_ref(e)), snap.ts_ms) {
            if ev.alert.rule == "endpoint-down" && ev.alert.entity.id == IDLE_OLLAMA {
                self.fired.push(format!("t={}s {:?} {:?} {:?}", t_ms / 1000, ev.kind, ev.alert.severity, ev.alert.message));
            }
        }
    }
}

/// One busy ask, then a minute of rounds through the rule engine: "Model down" never fires.
#[tokio::test]
async fn one_busy_ask_of_an_idle_ollama_pages_nobody() {
    use std::sync::atomic::Ordering;
    let ps = Arc::new(Mutex::new(r#"{"models":[]}"#.to_string()));
    let (node, c) = idle_ollama_a_minute_on("idle-ollama-page", ollama_service_script(ps)).await;
    let mut pager = Pager::new();
    let mut t = 61_000;
    node.knobs.refuse_next.store(100_000, Ordering::SeqCst);
    let (e, snap) = idle_round(&c).await;
    node.knobs.refuse_next.store(0, Ordering::SeqCst);
    pager.step(&e, &snap, t);
    for _ in 0..30 {
        c.models.age(3_000);
        t += 3_000;
        let (e, snap) = idle_round(&c).await;
        assert_ne!(e.health, endpoints::Health::Down, "t={}s: {:?}", t / 1000, e.health_reason);
        pager.step(&e, &snap, t);
    }
    assert!(pager.fired.is_empty(), "{:?}", pager.fired);
    c.live.stop();
}

/// An idle Ollama that stops answering is still found out: its minute's ask fails, the ask 15 s
/// later fails too, and it's Down then (within IDLE_READY_WITHIN_MS of its last answer), named by
/// what was asked; "Model down" fires once that has lasted the rule's 30 s.
#[tokio::test]
async fn an_idle_ollama_that_stops_answering_is_down_after_two_asks_and_pages() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fail = Arc::new(AtomicBool::new(false));
    let (node, c) = idle_ollama_a_minute_on("idle-ollama-dead", ollama_that_fails(fail.clone())).await;
    let before = tags_asked(&node);
    fail.store(true, Ordering::SeqCst);
    let mut pager = Pager::new();
    let mut t = 61_000;
    let (e, snap) = idle_round(&c).await;
    assert_eq!(e.health, endpoints::Health::Idle, "one failed ask: {:?}", e.health_reason);
    pager.step(&e, &snap, t);
    // Past Live's 2 s reuse of that read, as the app's rounds are.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    let mut down_at = None;
    while t <= 150_000 {
        c.models.age(3_000);
        t += 3_000;
        let (e, snap) = idle_round(&c).await;
        pager.step(&e, &snap, t);
        if e.health == endpoints::Health::Down && down_at.is_none() {
            down_at = Some(t);
            assert_eq!(e.health_reason.as_deref(), Some("No answer from 127.0.0.1:11434 on cell3 (over SSH)"));
            assert_eq!(tags_asked(&node) - before, 2, "two asks, both unanswered");
        }
    }
    let down_at = down_at.expect("Down");
    assert!(down_at <= endpoints::IDLE_READY_WITHIN_MS, "Down {} s after its last answer", down_at / 1000);
    assert!(!pager.fired.is_empty(), "Model down fires");
    c.live.stop();
}

// ---- P1 r4 (R4-LOAD): one node without Ray, the pre-flight and its page-cache drop, Ready once
// it answers, and an unload that stops cleanly and checks what came back. cell2 is a fake GB10
// whose docker is models/fake_vllm.rs's shim running the vLLM stand-in.

mod r4_load {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::errors::{ErrorKind, SbError};
    use crate::live::exit::{Enforce, WritePolicy, KEY_KIND};
    use crate::live::fence::{self, Spec as FenceSpec};
    use crate::live::fence_class::FenceClass::{self, *};
    use crate::models::fake_vllm::{self, Knobs};
    use crate::tasks::{TaskState, Tasks};

    const GIB: u64 = 1 << 30;
    /// The "P1 Session B · cell2" preset, as src/lib/fence.ts had it.
    const SESSION_B_CELL2: [FenceClass; 10] = [PowerOne, AptRefresh, AptOne, AptHold, Shell, ContainerTest, Fleet, ModelLoadOne, ModelUnload, DropCaches];
    /// The "P1 Session B · all nodes" preset (service only for nginx.service).
    const SESSION_B_ALL: [FenceClass; 5] = [AptRefresh, AptOne, AptHold, Service, Shell];

    /// Fences signed the way Settings › Fence signs one, for this build, with the exit enforcing them.
    struct Fences {
        dir: PathBuf,
        config: PathBuf,
        key: p256::ecdsa::SigningKey,
        file: fence::KeyFile,
    }

    impl Fences {
        fn enforce(base: &Path, live: &Live) -> Fences {
            let instance = crate::identity::instance_name();
            let dir = base.join("SparkBox Fence");
            std::fs::create_dir_all(&dir).unwrap();
            let config = base.join(format!("config-{instance}"));
            let key = fence::new_key().unwrap();
            let (key_id, public_key) = fence::public_of(&key).unwrap();
            let file = fence::KeyFile { key_id, public_key, kind: KEY_KIND.into() };
            fence::save_trust(&config, &file).unwrap();
            live.enforce_writes(Enforce { instance: instance.into(), fence_dir: dir.clone(), config_dir: Some(config.clone()), noauth: false, tag: "p1-r4-load".into(), mac: Some("Studio".into()) });
            Fences { dir, config, key, file }
        }

        /// Opens a fence on cell2 for `classes` (the newer record is the open one).
        fn open(&self, classes: &[FenceClass], note: &str) {
            std::thread::sleep(Duration::from_millis(3));
            let only = if classes.contains(&Service) { BTreeMap::from([(Service, vec!["nginx.service".to_string()])]) } else { BTreeMap::new() };
            let spec = FenceSpec { nodes: vec!["cell2".into()], classes: classes.to_vec(), only, minutes: 180, instance: crate::identity::instance_name().into(), note: note.into() };
            let r = fence::record_for(&spec, &self.key, now_ms(), "Studio").unwrap();
            fence::write(&self.dir, &self.config, &self.file, &r).unwrap();
        }
    }

    /// Look-only as the gate says it, Admin open otherwise.
    struct Held(AtomicBool);

    impl WritePolicy for Held {
        fn look_only(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }

        fn admin_unlocked(&self) -> bool {
            true
        }
    }

    /// cell2 as a GB10 with the glm53 image and gemma-3-27b-it's folder, docker answered by the
    /// vLLM stand-in, and SparkBox connected to it with a task list.
    struct Gb10 {
        c: Cluster,
        node: crate::fake_node::Node,
        standin: fake_vllm::Standin,
        folder: String,
        tasks: Arc<Tasks>,
        port: u16,
    }

    /// `ss -Hltn 'sport = :N'` as Ubuntu answers it (a Mac has no ss): a LISTEN line when
    /// something takes the port on this Mac's loopback (the stand-in's API), nothing otherwise.
    pub(super) fn ss(cmd: &str) -> Option<(String, u32)> {
        let port: u16 = cmd.strip_prefix("ss -Hltn 'sport = :")?.split('\'').next()?.parse().ok()?;
        let open = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(200)).is_ok();
        Some((if open { format!("LISTEN 0      2048       127.0.0.1:{port}        0.0.0.0:*\n") } else { String::new() }, 0))
    }

    /// The quick check's answer for node `n`, and `ss`; the stand-in answers the rest.
    fn gb10_script(n: u8) -> Script {
        Arc::new(move |cmd, _| if cmd.contains("sparkjob.py check") { Some((check_reply(n), 0)) } else { ss(cmd) })
    }

    async fn gb10(name: &str, knobs: Knobs) -> Gb10 {
        let home = temp(&format!("{name}-home"));
        let folder = model_folder(&home, "google/gemma-3-27b-it", true);
        let (node, port, standin) = fake_vllm::node_with(&home, knobs, gb10_script(2)).await;
        let c = cluster(name, &[("cell2", port)]).await;
        let tasks = Tasks::new(None);
        c.models.set_tasks(tasks.clone());
        Gb10 { c, node, standin, folder, tasks, port }
    }

    /// gemma-3-27b-it on cell2 with the glm53 image, on a port nothing on this Mac uses. The fake
    /// node's frame says vllm-head holds 88 GB, so the memory plan's guardrail is overridden.
    fn gemma(id: &str, folder: &str) -> DeploySpec {
        DeploySpec {
            id: id.into(),
            served_name: "gemma-3-27b-it".into(),
            model_path: folder.into(),
            nodes: vec!["cell2".into()],
            max_model_len: Some(32768),
            port: fake_vllm::free_port(),
            image_ref: fake_vllm::GLM53_IMAGE.into(),
            override_memory: true,
            ..Default::default()
        }
    }

    fn gib(b: u64) -> String {
        format!("{:.1}", b as f64 / GIB as f64)
    }

    /// Loads `spec` and waits until it's Ready.
    async fn ready(g: &Gb10, spec: &DeploySpec) -> Deployment {
        g.c.models.deploy(&g.c.ctx(), spec.clone()).await.unwrap();
        wait_state(&g.c, &spec.id, "ready").await
    }

    fn sudo_sent(node: &crate::fake_node::Node) -> Vec<String> {
        node.ran("sudo ")
    }

    #[tokio::test]
    async fn one_node_starts_its_image_by_id_is_ready_once_it_answers_and_unloads_cleanly_under_the_cell2_fence() {
        let g = gb10("r4-full", Knobs::default()).await;
        let c = &g.c;
        let fences = Fences::enforce(&c.dir, &c.live);
        let spec = gemma("r4-full", &g.folder);

        // "P1 Session B · all nodes" has neither model-load-one nor model-unload: the exit refuses
        // the load in its own words, and nothing that changes cell2 reaches it.
        fences.open(&SESSION_B_ALL, "P1 Session B · all nodes");
        c.models.deploy(&c.ctx(), spec.clone()).await.unwrap();
        let d = wait_state(c, "r4-full", "failed").await;
        assert!(d.message.contains("The fence doesn't allow model loads"), "{}", d.message);
        assert!(g.node.ran("docker run").is_empty() && g.node.ran("docker stop").is_empty() && g.node.ran("docker rm").is_empty() && g.node.ran("deploy-files").is_empty());
        assert!(g.standin.running().is_empty());

        // "P1 Session B · cell2": the pre-flight (a read) gives the image's id and the baseline,
        // C-L2 starts it under model-load-one, and it's Ready only once it answers.
        fences.open(&SESSION_B_CELL2, "P1 Session B · cell2");
        let d = c.models.deploy(&c.ctx(), DeploySpec { image_id: format!("sha256:{}", "b".repeat(64)), ..spec.clone() }).await.unwrap();
        assert_eq!(d.spec.image_id, fake_vllm::test_image_id(), "the id the node gave, never the UI's");
        assert_eq!((d.spec.image.as_str(), d.spec.gpu_memory_utilization), (fake_vllm::GLM53_IMAGE, 0.75));
        assert_eq!(d.baseline.len(), 1);
        assert_eq!((d.baseline[0].node.as_str(), d.baseline[0].mem_available, d.baseline[0].shm.len()), ("cell2", Knobs::default().mem_available, 0));
        let d = wait_state(c, "r4-full", "ready").await;
        let r = d.readiness.clone().expect("a readiness verdict");
        assert!(r.ok && r.health && r.models_listed && r.token == Some(true), "{r:?}");
        assert!(r.sanity.as_ref().is_some_and(|s| s.ok && s.answer.contains("42")), "the first run of a recipe and image asks 17+25: {r:?}");
        // Its native APIs are read once it's Ready.
        let d = until_load(c, "r4-full", |d| d.protocols.is_some()).await;
        assert_eq!(d.protocols.as_ref().map(|p| (p.messages, p.count_tokens, p.responses, p.image_id.clone())), Some((true, true, true, fake_vllm::test_image_id())));
        let p = c.llm.get("vllm-r4-full").await.expect("in Chat and the Server once Ready");
        let key = p.api_key().unwrap().expect("its key");

        // The run is C-L2 for this load: the image by its id, no Ray layer, no build, pull or pip.
        assert_eq!(g.node.ran("docker run -d"), vec![plan::rank_command(&d.spec, 0)]);
        for never in ["docker build", "docker pull", "pip install", "--shm-size"] {
            assert!(g.node.ran(never).is_empty(), "{never}");
        }
        assert!(!g.node.ran("docker run -d")[0].contains("vllm-ray"), "the image itself, not a Ray layer");
        let argv = g.standin.argv("sparkbox-r4-full");
        assert!(argv.contains(&fake_vllm::test_image_id()), "{argv:?}");
        assert!(argv.windows(2).any(|w| w[0] == "--label" && w[1] == format!("sparkbox.image={}", fake_vllm::GLM53_IMAGE)), "{argv:?}");
        assert!(!argv.contains(&"-e".to_string()), "no -e: {argv:?}");
        let seen = g.standin.seen("sparkbox-r4-full");
        assert_eq!((seen["values"]["HF_HUB_OFFLINE"].as_str(), seen["hasKey"].as_bool()), (Some("1"), Some(true)), "{seen}");
        // sparkjob.py wrote the key file 0400 with the key in it; no command carried the key.
        let file = g.node.home.join(".sparkbox/keys/r4-full");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), key);
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o400);
        assert!(!g.node.raw.lock().unwrap().iter().any(|l| l.contains(&key)));
        assert!(!c.live.audit.entries().iter().any(|e| e.command.contains(&key)));
        // sparkjob.py, as the cluster user, made the cache folders (Docker would have, as root).
        assert!(g.node.home.join(".cache/vllm").is_dir() && g.node.home.join(".cache/flashinfer").is_dir() && g.node.home.join(".triton").is_dir());

        // Unload: a task that stops (TERM, so vLLM cleans up), removes, then checks.
        let task = c.models.unload(&c.ctx(), "r4-full").await.unwrap();
        let t = finished(&g.tasks, &task).await;
        assert_eq!(t.title, "Unload gemma-3-27b-it from cell2");
        assert_eq!((t.state, t.warning.as_deref()), (TaskState::Ok, None), "{t:?}");
        assert_eq!(t.steps.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["Stopping (docker stop -t 30, then docker rm)", "Checking memory, port and /dev/shm"]);
        let d = until_load(c, "r4-full", |d| d.unload_report.is_some()).await;
        let base = Knobs::default().mem_available;
        let summary = format!("Unloaded from cell2 · memory back ({} GiB available, baseline {}) · port {} closed · /dev/shm: nothing new", gib(base), gib(base), spec.port);
        assert_eq!(d.unload_report.as_ref().map(|r| (r.ok, r.summary.clone())), Some((true, summary.clone())));
        assert_eq!(t.summary.as_deref(), Some(summary.as_str()));
        assert_eq!(d.state, DeploymentState::Stopped);
        assert!(g.standin.running().is_empty() && g.standin.shm().is_empty(), "TERM let vLLM remove its shared memory");
        assert!(!file.exists() && !g.node.home.join(".sparkbox/deploy/r4-full").exists(), "the key file and the launch files went with it");
        assert!(c.llm.get("vllm-r4-full").await.is_none() && secrets::get(&plan::key_account("r4-full")).unwrap().is_none());
        assert!(!g.node.log.lock().unwrap().iter().any(|l| l.contains("docker rm -f") || l.contains("rm -f $ids")), "never docker rm -f");

        // Every row's class: the pre-flight and the checks read, the launch files and the drop-free
        // load model-load-one, the unload model-unload, the test token inference.
        let rows: Vec<audit::Entry> = c.live.audit.entries().into_iter().filter(|e| e.title != "Refused").collect();
        let of = |pre: &str| rows.iter().filter(|e| e.command.starts_with(pre) || e.command.contains(&format!("\n{pre}"))).collect::<Vec<_>>();
        let preflights = of("echo '--- mem'");
        assert!(preflights.len() >= 2 && preflights.iter().all(|e| !e.changes && e.fence.is_none()), "{preflights:?}");
        let loads: Vec<_> = rows.iter().filter(|e| e.command.contains("docker run -d") || e.command.contains("sparkjob.py deploy-files")).collect();
        assert_eq!(loads.iter().map(|e| e.fence.as_deref()).collect::<Vec<_>>(), [Some("model-load-one"), Some("model-load-one")], "{loads:?}");
        let unloads = of("ids=$(docker ps -aq");
        assert!(unloads.len() == 1 && unloads.iter().all(|e| e.fence.as_deref() == Some("model-unload")), "the unload: {unloads:?}");
        let tokens: Vec<_> = rows.iter().filter(|e| e.command.contains("/v1/completions")).collect();
        assert!(!tokens.is_empty() && tokens.iter().all(|e| !e.changes && e.fence.is_none()), "{tokens:?}");
        for e in rows.iter().filter(|e| e.changes && (e.kind == "sudo" || e.command.contains("docker "))) {
            assert!(e.fence.is_some(), "every change on cell2 went through the fence: {e:?}");
        }
        let after = rows.iter().filter(|e| ["grep MemAvailable /proc/meminfo", "ls -la /dev/shm"].contains(&e.command.as_str())).collect::<Vec<_>>();
        assert!(after.len() >= 2 && after.iter().all(|e| !e.changes && e.fence.is_none()), "{after:?}");
        c.live.stop();
    }

    #[tokio::test]
    async fn the_page_cache_is_dropped_only_when_vllm_would_refuse_and_only_with_consent() {
        // 41.2 GiB free and 116.9 GiB available: most of the rest is page cache.
        let short = Knobs { mem_free: 43_201_000 * 1024, mem_cached: 75 * GIB, ..Knobs::default() };
        let g = gb10("r4-drop", short).await;
        let c = &g.c;
        let spec = gemma("r4-drop", &g.folder);

        // No consent: refused with the figures; nothing went through sudo, nothing was saved.
        let e = c.models.deploy(&c.ctx(), spec.clone()).await.unwrap_err();
        assert_eq!(e, "cell2 has 41.2 GiB free; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Tick 'Drop the page cache first' to let SparkBox free it (sudo; the kernel refills it).");
        assert!(sudo_sent(&g.node).is_empty() && g.standin.drops() == 0);
        assert!(c.models.deployments(&c.ctx()).is_empty());

        // Look-only: refused at the exit before any sudo, in look-only's words.
        let held = Arc::new(Held(AtomicBool::new(true)));
        c.live.set_write_policy(held.clone());
        let e = c.models.deploy(&c.ctx(), DeploySpec { drop_caches: true, ..spec.clone() }).await.unwrap_err();
        assert_eq!(SbError::classify(&e).kind, ErrorKind::LookOnly, "{e}");
        assert!(sudo_sent(&g.node).is_empty() && g.standin.drops() == 0);
        held.0.store(false, Ordering::SeqCst);

        // A fence without drop-caches refuses it: nothing sent.
        let fences = Fences::enforce(&c.dir, &c.live);
        fences.open(&[ModelLoadOne, ModelUnload], "load and unload only");
        let e = c.models.deploy(&c.ctx(), DeploySpec { drop_caches: true, ..spec.clone() }).await.unwrap_err();
        assert!(e.contains("The fence doesn't allow page-cache drops"), "{e}");
        assert!(sudo_sent(&g.node).is_empty() && g.standin.drops() == 0);

        // The cell2 fence admits it: dropped once (drop-caches, its own operation), read again,
        // then the load goes on from the new figures.
        fences.open(&SESSION_B_CELL2, "P1 Session B · cell2");
        let d = c.models.deploy(&c.ctx(), DeploySpec { drop_caches: true, ..spec.clone() }).await.unwrap();
        assert_eq!(g.standin.drops(), 1);
        assert_eq!(sudo_sent(&g.node), ["sudo -k -S -p '' sync; echo 3 > /proc/sys/vm/drop_caches"]);
        let stdin = g.node.stdin.lock().unwrap().iter().find(|(cmd, _)| cmd.contains("drop_caches")).map(|(_, i)| String::from_utf8_lossy(i).into_owned()).unwrap();
        assert_eq!(stdin, "pw\n", "the password on sudo's stdin, nowhere else");
        let row = c.live.audit.entries().into_iter().find(|e| e.kind == "sudo" && e.command.contains("drop_caches")).unwrap();
        assert_eq!((row.fence.as_deref(), row.title.as_str(), row.escalated), (Some("drop-caches"), "Drop the page cache on cell2 (vLLM counts it as used)", true));
        assert_eq!(d.baseline[0].mem_available, Knobs::default().mem_available);
        wait_state(c, "r4-drop", "ready").await;
        let t = finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-drop").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Ok);
        c.live.stop();

        // Above the line: no drop, even with consent.
        let g = gb10("r4-nodrop", Knobs::default()).await;
        ready(&g, &DeploySpec { drop_caches: true, ..gemma("r4-nodrop", &g.folder) }).await;
        assert!(sudo_sent(&g.node).is_empty() && g.standin.drops() == 0);
        let t = finished(&g.tasks, &g.c.models.unload(&g.c.ctx(), "r4-nodrop").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Ok);
        g.c.live.stop();

        // Still short after the drop (little is page cache): refused with the new figures.
        let tight = Knobs { mem_free: 30 * GIB, mem_available: 41 * GIB, mem_cached: 11 * GIB, ..Knobs::default() };
        let g = gb10("r4-short", tight).await;
        let e = g.c.models.deploy(&g.c.ctx(), DeploySpec { drop_caches: true, ..gemma("r4-short", &g.folder) }).await.unwrap_err();
        assert_eq!(e, "cell2 has 40.5 GiB free after dropping the page cache; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Lower the share or free memory.");
        assert_eq!(g.standin.drops(), 1);
        assert!(g.node.ran("docker run").is_empty() && g.c.models.deployments(&g.c.ctx()).is_empty());
        g.c.live.stop();
    }

    #[tokio::test]
    async fn a_missing_image_or_folder_a_taken_port_or_an_unknown_node_is_refused_before_anything_changes() {
        let g = gb10("r4-refuse", Knobs { images: vec![], ..Knobs::default() }).await;
        let c = &g.c;
        let spec = gemma("r4-refuse", &g.folder);
        let e = c.models.deploy(&c.ctx(), spec.clone()).await.unwrap_err();
        assert_eq!(e, "cell2 doesn't have vllm/vllm-openai:glm53-flash-arm64-cu130. Pull it first (Fleet: docker pull vllm/vllm-openai:glm53-flash-arm64-cu130), or pick an image that's there.");
        g.standin.set_knobs(Knobs::default());
        // (Not under /home: a Mac's automounter answers for it.)
        let elsewhere = format!("{}/elsewhere/gemma-3-27b-it", g.node.home.display());
        let e = c.models.deploy(&c.ctx(), DeploySpec { model_path: elsewhere.clone(), ..spec.clone() }).await.unwrap_err();
        assert_eq!(e, format!("cell2 doesn't have {elsewhere} yet. Copy it to that node first (Library › Copy to nodes…)."));
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let e = c.models.deploy(&c.ctx(), DeploySpec { port, ..spec.clone() }).await.unwrap_err();
        assert_eq!(e, format!("Port {port} is already in use on cell2 (listening on 127.0.0.1:{port}). Pick another port."));
        let e = c.models.deploy(&c.ctx(), DeploySpec { nodes: vec!["cell9".into()], ..spec.clone() }).await.unwrap_err();
        assert_eq!(e, "Unknown node cell9");
        let e = c.models.deploy(&c.ctx(), DeploySpec { image_ref: "vllm/vllm-openai:x;reboot".into(), ..spec.clone() }).await.unwrap_err();
        assert!(e.contains("letters, digits and . _ / : @ -"), "{e}");
        // Nothing was saved, started or cleared on the node.
        assert!(c.models.deployments(&c.ctx()).is_empty());
        for never in ["docker run", "docker stop", "docker rm", "ids=$(", "sudo "] {
            assert!(g.node.ran(never).is_empty(), "{never}");
        }
        c.live.stop();
    }

    #[tokio::test]
    async fn the_preflight_shows_the_head_nodes_figures_and_demo_reads_nothing() {
        let knobs = Knobs { earlyoom: Some("EARLYOOM_ARGS=\"-r 3600 -n --avoid '(^|/)(init|systemd|Xorg|sshd)$'\"".into()), ..Knobs::default() };
        let g = gb10("r4-preflight", knobs.clone()).await;
        let c = &g.c;
        let spec = gemma("r4-preflight", &g.folder);
        let set = c.models.preflight(&c.ctx(), &spec).await.unwrap();
        assert_eq!((set.nodes.len(), set.verdict.as_str(), set.image_id.clone()), (1, "ready", Some(fake_vllm::test_image_id())));
        let p = &set.nodes[0];
        assert!(!p.demo);
        assert_eq!(p.mem.as_ref().map(|m| (m.total / 1024 * 1024, m.available / 1024 * 1024)), Some((knobs.mem_total, knobs.mem_available)));
        assert_eq!(gib(p.need_free), "95.3");
        assert!(!p.drop_needed);
        assert_eq!(p.earlyoom.as_ref().map(|e| (e.active, e.config.clone())), Some((true, knobs.earlyoom.clone())));
        assert_eq!((p.image_ref.as_str(), p.image_id.clone()), (fake_vllm::GLM53_IMAGE, Some(fake_vllm::test_image_id())));
        assert!(p.model_present && p.port_free && p.shm.is_empty() && p.problems.is_empty(), "{p:?}");
        assert_eq!(p.caches.iter().map(|c| c.exists).collect::<Vec<_>>(), [false, false, false]);
        // One read with its activity line; nothing changed.
        assert_eq!(g.node.ran("echo '--- mem'").len(), 1);
        let row = c.live.audit.entries().into_iter().find(|e| e.command.starts_with("echo '--- mem'")).unwrap();
        assert!(!row.changes && row.fence.is_none(), "{row:?}");
        for never in ["docker run", "ids=$(", "sudo "] {
            assert!(g.node.ran(never).is_empty(), "{never}");
        }
        // The preview then shows C-L with the id that read gave.
        let steps = c.models.preview(&c.ctx(), &spec).unwrap();
        let start = steps.iter().find(|s| s.title == "Start vLLM").unwrap();
        assert!(start.command.contains(&format!("--entrypoint python3 {} /run/sparkbox/launch.py --key", fake_vllm::test_image_id())), "{}", start.command);
        let files = steps.iter().find(|s| s.title == "Write vLLM's launch files").expect("the launch files");
        assert!(files.command.contains("sbk-••••••••"), "the key masked: {}", files.command);
        assert!(steps.iter().all(|s| !s.command.contains("vllm-ray") && !s.command.contains("docker build") && !s.command.contains("--distributed-executor-backend")));
        c.live.stop();

        // Demo: nothing is read, and the page says so.
        let (models, _llm, live, cluster, _dir) = demo_app("r4-preflight-demo");
        let before = live.audit.entries().len();
        let p = models.preflight(&demo_ctx(&cluster), &DeploySpec { nodes: vec!["cell2".into()], ..gemma("r4-demo", "/home/sparkuser/models/google/gemma-3-27b-it") }).await.unwrap();
        assert_eq!((p.nodes.len(), p.verdict.as_str()), (1, "demo"));
        assert!(p.nodes[0].demo && p.nodes[0].mem.is_none() && p.problems.is_empty());
        assert_eq!(live.audit.entries().len(), before, "Demo recorded nothing");
        assert!(serde_json::to_value(&p.nodes[0]).unwrap()["demo"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn a_wrong_answer_fails_the_load_and_a_passed_recipe_isnt_asked_again() {
        let g = gb10("r4-sanity", Knobs { garbage: true, ..Knobs::default() }).await;
        let c = &g.c;
        // 17+25 comes back as "4 2 2 2…": Failed in readiness's words, not in Chat, its log saved
        // and its container removed (C-X), and the verdict kept so the next load asks again.
        let spec = gemma("r4-sanity", &g.folder);
        c.models.deploy(&c.ctx(), spec.clone()).await.unwrap();
        let d = wait_state(c, "r4-sanity", "failed").await;
        assert_eq!(d.message, readiness::wrong_line("4 2 2 2 2 2 2 2"));
        assert!(c.llm.get("vllm-r4-sanity").await.is_none(), "not registered");
        assert!(g.standin.running().is_empty(), "{:?}", g.standin.running());
        let log = d.failure.as_ref().map(|f| PathBuf::from(&f.log_dir).join("r0.log")).expect("a failure record");
        assert!(std::fs::read_to_string(log).unwrap().contains("Application startup complete."));
        let recipe = readiness::recipe_key(&d.spec);
        let kept: BTreeMap<String, readiness::Sanity> = serde_json::from_str(&std::fs::read_to_string(c.dir.join("sanity.json")).unwrap()).unwrap();
        assert!(kept.get(&recipe).is_some_and(|s| !s.ok && s.answer == "4 2 2 2 2 2 2 2"), "{kept:?}");
        assert!(readiness::SanityStore::open(&c.dir).needs(&recipe));

        // A good image: asked again (the last verdict failed), passes, kept.
        g.standin.set_knobs(Knobs::default());
        let asked = || g.node.ran("17+25").len();
        assert_eq!(asked(), 2, "a reply cut off with no 42 is asked a second time before it's judged");
        let d = ready(&g, &DeploySpec { port: fake_vllm::free_port(), ..spec.clone() }).await;
        assert_eq!(asked(), 3);
        assert!(d.readiness.unwrap().sanity.is_some_and(|s| s.ok));
        finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-sanity").await.unwrap()).await;
        // The same recipe and image again: not asked.
        let d = ready(&g, &DeploySpec { port: fake_vllm::free_port(), ..spec.clone() }).await;
        assert_eq!(asked(), 3, "a passed verdict skips the prompt");
        assert!(d.readiness.as_ref().is_some_and(|r| r.ok && r.sanity.is_none()));
        finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-sanity").await.unwrap()).await;
        c.live.stop();
    }

    #[tokio::test]
    async fn up_but_not_yet_answering_stays_starting_with_the_reason_and_is_asked_again() {
        let g = gb10("r4-held", Knobs { warmup_s: 1.0, ..Knobs::default() }).await;
        let c = &g.c;
        let held = Arc::new(Held(AtomicBool::new(false)));
        c.live.set_write_policy(held.clone());
        c.models.deploy(&c.ctx(), gemma("r4-held", &g.folder)).await.unwrap();
        for _ in 0..100 {
            if !g.standin.running().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Look-only from here: /health and the model list answer, the test token is held.
        held.0.store(true, Ordering::SeqCst);
        // The same message prefix also covers a warmup HTTP 503. Wait for the held-token
        // verdict itself, instead of racing that earlier (valid) readiness result.
        let d = until_load(c, "r4-held", |d| d.readiness.as_ref().is_some_and(|r| r.why.as_deref() == Some(readiness::HELD))).await;
        assert_eq!(d.state, DeploymentState::Starting);
        assert_eq!(d.message, format!("Up; checking it answers… ({})", readiness::HELD));
        assert!(d.readiness.as_ref().is_some_and(|r| r.health && r.models_listed && r.token.is_none() && !r.ok), "{:?}", d.readiness);
        assert!(c.llm.get("vllm-r4-held").await.is_none(), "not in Chat while it isn't Ready");
        // Asked again at the next poll once look-only ends: Ready.
        held.0.store(false, Ordering::SeqCst);
        let d = wait_state(c, "r4-held", "ready").await;
        assert!(d.readiness.is_some_and(|r| r.ok && r.token == Some(true)));
        held.0.store(false, Ordering::SeqCst);
        finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-held").await.unwrap()).await;
        c.live.stop();
    }

    /// Waits until the stand-in of container `name` runs on its own: its first log line comes
    /// after it left the docker run's process group (setsid), as a container is the daemon's,
    /// not the SSH session's, and a dropped connection can't take it down.
    async fn up(g: &Gb10, name: &str) {
        let run_prefix = format!("docker run -d --name {name} ");
        crate::fake_node::wait_for("the detached, acknowledged model start", || {
            // And its `docker run` has answered: a test that cuts the connection now is a quit, not
            // a run that failed, which would roll the load back and take its key.
            let answered = g.c.live.audit.entries().iter().any(|e| e.command.starts_with(&run_prefix) && e.state == "ok");
            (answered && g.standin.log(name).contains("vLLM API server version")).then_some(())
        }, || format!("{}\nDeployments: {:?}\nAudit: {:?}", g.standin.diagnostic(name), g.c.models.deployments(&g.c.ctx()), g.c.live.audit.entries())).await;
    }

    #[tokio::test]
    async fn time_look_only_holds_the_test_token_doesnt_count_toward_the_ready_limit() {
        let g = gb10("r4-held-long", Knobs::default()).await;
        let c = &g.c;
        let held = Arc::new(Held(AtomicBool::new(false)));
        c.live.set_write_policy(held.clone());
        // The 90-minute limit, as 2 s.
        c.models.set_ready_within(Duration::from_secs(2));
        c.models.deploy(&c.ctx(), gemma("r4-held-long", &g.folder)).await.unwrap();
        up(&g, "sparkbox-r4-held-long").await;
        held.0.store(true, Ordering::SeqCst);
        until_load(c, "r4-held-long", |d| d.message.starts_with("Up; checking it answers")).await;
        // Well past the limit, held all along: still starting, in look-only's words.
        tokio::time::sleep(Duration::from_secs(8)).await;
        assert_eq!(state_of(c, "r4-held-long"), (DeploymentState::Starting, format!("Up; checking it answers… ({})", readiness::HELD)));
        // Asked again once look-only ends: Ready, and it stays Ready though that poll came past
        // the limit.
        held.0.store(false, Ordering::SeqCst);
        wait_state(c, "r4-held-long", "ready").await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(state_of(c, "r4-held-long").0, DeploymentState::Ready);
        finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-held-long").await.unwrap()).await;

        // Not held: one that doesn't answer within the limit fails, in the limit's words.
        g.standin.set_knobs(Knobs { warmup_s: 60.0, ..Knobs::default() });
        c.models.deploy(&c.ctx(), gemma("r4-slow", &g.folder)).await.unwrap();
        let d = wait_state(c, "r4-slow", "failed").await;
        assert_eq!(d.message, "vLLM didn't become ready within 2 s.");
        finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-slow").await.unwrap()).await;
        c.live.stop();
    }

    #[tokio::test]
    async fn after_a_restart_mid_load_sparkbox_still_waits_for_it_to_answer() {
        // Starting when SparkBox quit; the image answers 17+25 wrongly, though /health is 200.
        let g = gb10("r4-resume", Knobs { garbage: true, warmup_s: 2.0, ..Knobs::default() }).await;
        let spec = gemma("r4-resume", &g.folder);
        g.c.models.deploy(&g.c.ctx(), spec.clone()).await.unwrap();
        up(&g, "sparkbox-r4-resume").await;
        let saved = std::fs::read_to_string(g.c.dir.join("deployments.json")).unwrap();
        g.c.live.stop();
        let was: Vec<Deployment> = serde_json::from_str(&saved).unwrap();
        assert_eq!(was[0].state, DeploymentState::Starting);

        // The next start: the same saved load, watched again.
        let dir = temp("r4-resume-again");
        std::fs::write(dir.join("deployments.json"), &saved).unwrap();
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell2", g.port)]).await;
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        let again = Cluster { models, live, llm, inventory, dir };
        again.models.resume(&ModelSettings::default());
        let d = wait_state(&again, "r4-resume", "failed").await;
        assert!(d.message.starts_with("It answers, but wrongly: asked 17+25"), "{}", d.message);
        assert!(d.readiness.as_ref().is_some_and(|r| r.health && !r.ok), "/health alone didn't make it Ready: {:?}", d.readiness);
        assert!(again.llm.get("vllm-r4-resume").await.is_none());
        assert!(g.standin.running().is_empty() && d.failure.as_ref().is_some_and(|f| f.left_on.is_empty()), "cleaned up after, as any failed start: {:?}", d.failure);
        again.live.stop();
    }

    /// The next start: a new SparkBox with `g`'s saved load, which began `ago` seconds back.
    async fn relaunch(name: &str, g: &Gb10, ago: u64) -> Cluster {
        let mut saved: Vec<Deployment> = serde_json::from_str(&std::fs::read_to_string(g.c.dir.join("deployments.json")).unwrap()).unwrap();
        g.c.live.stop();
        saved[0].started_at = now() - ago;
        let dir = temp(name);
        std::fs::write(dir.join("deployments.json"), serde_json::to_string(&saved).unwrap()).unwrap();
        let (live, inventory) = crate::fake_node::connect(&dir, &[("cell2", g.port)]).await;
        let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
        let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
        Cluster { models, live, llm, inventory, dir }
    }

    /// A load still starting when SparkBox quit, past its 90 minutes at the next start.
    async fn left_starting(name: &str, knobs: Knobs) -> (Gb10, DeploySpec) {
        let g = gb10(name, knobs).await;
        let spec = gemma(name, &g.folder);
        g.c.models.deploy(&g.c.ctx(), spec.clone()).await.unwrap();
        up(&g, &format!("sparkbox-{name}")).await;
        (g, spec)
    }

    const PAST_THE_LIMIT: u64 = 91 * 60;

    #[tokio::test]
    async fn after_a_restart_a_load_keeps_its_limit() {
        // Never answers: failed at the first look, in `follow`'s words, since the 90 minutes count
        // from when it began. A limit counted from the relaunch would leave it Starting here.
        let (g, _) = left_starting("r5-limit", Knobs { warmup_s: 600.0, ..Knobs::default() }).await;
        let again = relaunch("r5-limit-again", &g, PAST_THE_LIMIT).await;
        again.models.update("r5-limit", |d| d.recoverable = true);
        again.models.resume(&ModelSettings::default());
        let d = wait_state(&again, "r5-limit", "failed").await;
        assert_eq!((d.message.as_str(), d.recoverable), ("vLLM didn't become ready within 90 minutes.", false), "a verdict, as in `follow`");
        again.live.stop();

        // Inside its limit it's left to go on.
        let (g, _) = left_starting("r5-limit-ok", Knobs { warmup_s: 2.0, ..Knobs::default() }).await;
        let again = relaunch("r5-limit-ok-again", &g, 89 * 60).await;
        again.models.resume(&ModelSettings::default());
        wait_state(&again, "r5-limit-ok", "ready").await;
        again.live.stop();

        // A node that stays unreachable cannot prove that a saved load failed.
        let (g, _) = left_starting("r5-limit-lost", Knobs { warmup_s: 600.0, ..Knobs::default() }).await;
        let again = relaunch("r5-limit-lost-again", &g, PAST_THE_LIMIT).await;
        again.models.set_lost_grace(Duration::from_millis(300));
        again.live.stop();
        again.models.resume(&ModelSettings::default());
        let d = until_load(&again, "r5-limit-lost", |d| d.recovery == Some(recovery::Recovery::Waiting)).await;
        assert_eq!(d.state, DeploymentState::Starting);
        assert!(d.failure.is_none(), "no cleanup based on an unavailable node");
        assert!(d.message.contains("nothing is relaunched or cleaned up"));
    }

    #[tokio::test]
    async fn past_the_limit_a_first_look_that_cant_reach_the_node_fails_nothing() {
        // The model came up while SparkBox was closed, and it's reopened before the VPN is up. A
        // Failed load isn't watched again, so one failed look would end it for good.
        let (g, _) = left_starting("r5-blip", Knobs::default()).await;
        let again = relaunch("r5-blip-again", &g, PAST_THE_LIMIT).await;
        g.node.hang();
        g.node.drop_connections().await;
        again.models.resume(&ModelSettings::default());
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(state_of(&again, "r5-blip").0, DeploymentState::Starting);
        g.node.thaw();
        wait_state(&again, "r5-blip", "ready").await;
        assert_eq!(g.standin.running().len(), 1, "it was serving all along");
        again.live.stop();
    }

    /// Once `follow` has had its first answer, the node stops answering, as if it went away.
    async fn away(g: &Gb10, id: &str) {
        until_load(&g.c, id, |d| !d.log_tail.is_empty()).await;
        g.node.hang();
        g.node.drop_connections().await;
    }

    #[tokio::test]
    async fn a_node_away_mid_load_ends_nothing() {
        // Longer than the seven failed looks, 21 s, that used to read "Lost contact" for good.
        let g = gb10("r5-away", Knobs { warmup_s: 30.0, ..Knobs::default() }).await;
        let c = &g.c;
        c.models.deploy(&c.ctx(), gemma("r5-away", &g.folder)).await.unwrap();
        up(&g, "sparkbox-r5-away").await;
        away(&g, "r5-away").await;
        tokio::time::sleep(Duration::from_secs(24)).await;
        assert_ne!(state_of(c, "r5-away").0, DeploymentState::Failed);
        g.node.thaw();
        until_load_for(c, "r5-away", 40, |d| d.state == DeploymentState::Ready).await;
        c.live.stop();
    }

    #[tokio::test]
    async fn a_node_that_stays_away_mid_load_reaches_the_limit() {
        let g = gb10("r5-gone", Knobs { warmup_s: 600.0, ..Knobs::default() }).await;
        let c = &g.c;
        c.models.set_ready_within(Duration::from_secs(9));
        c.models.set_lost_grace(Duration::from_millis(100));
        c.models.deploy(&c.ctx(), gemma("r5-gone", &g.folder)).await.unwrap();
        c.models.update("r5-gone", |d| d.recoverable = true);
        up(&g, "sparkbox-r5-gone").await;
        away(&g, "r5-gone").await;
        let d = wait_state(c, "r5-gone", "failed").await;
        assert_eq!((d.message.as_str(), d.recoverable), ("vLLM didn't become ready within 9 s. Its containers and files are still on cell2; Unload or Forget removes them.", false), "a verdict, as at any limit");
        g.node.thaw();
        c.live.stop();
    }

    #[tokio::test]
    async fn mid_load_a_node_out_of_reach_at_the_limit_gets_its_grace() {
        // Past the limit while it can't be asked, then it's back and answers: Ready.
        let g = gb10("r5-grace", Knobs { warmup_s: 9.0, ..Knobs::default() }).await;
        let c = &g.c;
        c.models.set_ready_within(Duration::from_secs(7));
        c.models.deploy(&c.ctx(), gemma("r5-grace", &g.folder)).await.unwrap();
        up(&g, "sparkbox-r5-grace").await;
        away(&g, "r5-grace").await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_ne!(state_of(c, "r5-grace").0, DeploymentState::Failed);
        g.node.thaw();
        until_load_for(c, "r5-grace", 40, |d| d.state == DeploymentState::Ready).await;
        c.live.stop();
    }

    /// A `gb10` whose next poll, once the flag is set, gets nothing: the connection goes before the
    /// command says anything, so its channel closes with no output and no exit status.
    async fn gb10_with_a_cut(name: &str, knobs: Knobs) -> (Gb10, Arc<AtomicBool>) {
        let cut = Arc::new(AtomicBool::new(false));
        let slot: Arc<Mutex<Option<crate::fake_node::Node>>> = Arc::default();
        let (flag, node_slot, base) = (cut.clone(), slot.clone(), gb10_script(2));
        let script: Script = Arc::new(move |cmd, stdin| {
            if cmd.contains("State.Status") && flag.swap(false, Ordering::SeqCst) {
                node_slot.lock().unwrap().as_ref().unwrap().close_without_status("State.Status");
                return Some((String::new(), 0));
            }
            base(cmd, stdin)
        });
        let home = temp(&format!("{name}-home"));
        let folder = model_folder(&home, "google/gemma-3-27b-it", true);
        let (node, port, standin) = fake_vllm::node_with(&home, knobs, script).await;
        *slot.lock().unwrap() = Some(node.clone());
        let c = cluster(name, &[("cell2", port)]).await;
        let tasks = Tasks::new(None);
        c.models.set_tasks(tasks.clone());
        (Gb10 { c, node, standin, folder, tasks, port }, cut)
    }

    #[tokio::test]
    async fn a_poll_with_no_reply_is_a_failed_look_not_a_stopped_load() {
        let (g, cut) = gb10_with_a_cut("r5-empty", Knobs { warmup_s: 6.0, ..Knobs::default() }).await;
        let c = &g.c;
        c.models.deploy(&c.ctx(), gemma("r5-empty", &g.folder)).await.unwrap();
        until_load(c, "r5-empty", |d| !d.log_tail.is_empty()).await;
        cut.store(true, Ordering::SeqCst);
        wait_state(c, "r5-empty", "ready").await;
        assert!(!cut.load(Ordering::SeqCst), "the empty reply came");
        c.live.stop();
    }

    #[tokio::test]
    async fn after_a_restart_a_poll_with_no_reply_is_a_failed_look_too() {
        let (g, cut) = gb10_with_a_cut("r5-empty-again", Knobs::default()).await;
        g.c.models.deploy(&g.c.ctx(), gemma("r5-empty-again", &g.folder)).await.unwrap();
        up(&g, "sparkbox-r5-empty-again").await;
        let again = relaunch("r5-empty-again-2", &g, 60).await;
        cut.store(true, Ordering::SeqCst);
        again.models.resume(&ModelSettings::default());
        wait_state(&again, "r5-empty-again", "ready").await;
        assert!(!cut.load(Ordering::SeqCst), "the empty reply came");
        again.live.stop();
    }

    #[tokio::test]
    async fn past_the_limit_a_load_that_answers_or_is_held_isnt_failed() {
        // It answers at this look: Ready, however late (as in `follow`).
        let (g, _) = left_starting("r5-late", Knobs::default()).await;
        let again = relaunch("r5-late-again", &g, PAST_THE_LIMIT).await;
        again.models.resume(&ModelSettings::default());
        wait_state(&again, "r5-late", "ready").await;
        again.live.stop();

        // Look-only holds the test token: that isn't the load's doing, and it's asked again after.
        let (g, _) = left_starting("r5-late-held", Knobs::default()).await;
        let again = relaunch("r5-late-held-again", &g, PAST_THE_LIMIT).await;
        let held = Arc::new(Held(AtomicBool::new(true)));
        again.live.set_write_policy(held.clone());
        again.models.resume(&ModelSettings::default());
        until_load(&again, "r5-late-held", |d| d.message.starts_with("Up; checking it answers")).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(state_of(&again, "r5-late-held"), (DeploymentState::Starting, format!("Up; checking it answers… ({})", readiness::HELD)));
        held.0.store(false, Ordering::SeqCst);
        wait_state(&again, "r5-late-held", "ready").await;
        again.live.stop();
    }

    /// Starts a container of load `id` on `node` the way docker would have one (this build's labels).
    async fn container(c: &Cluster, node: &str, id: &str, name: &str) {
        let cmd = format!(
            "docker run -d --name {name} --label sparkbox.instance={} --label sparkbox.deployment={id} --label sparkbox.role=worker {}",
            crate::identity::instance_name(),
            fake_vllm::GLM53_IMAGE
        );
        let out = c.live.run(node, &cmd, None, Duration::from_secs(20)).await.unwrap();
        assert_eq!(out.code, Some(0), "{}", out.stderr);
    }

    #[tokio::test]
    async fn an_unload_finds_its_containers_on_every_node_and_reports_what_didnt_come_back() {
        // cell2 serves; cell3 holds a container of the same load (it isn't in the load's nodes).
        let g = gb10("r4-unload", Knobs::default()).await;
        let home3 = temp("r4-unload-cell3");
        let (_n3, p3, standin3) = fake_vllm::node_with(&home3, Knobs::default(), gb10_script(3)).await;
        let c = cluster("r4-unload-both", &[("cell2", g.port), ("cell3", p3)]).await;
        c.models.set_tasks(g.tasks.clone());
        c.models.set_unload_wait(Duration::from_millis(100), Duration::from_secs(1));
        let spec = gemma("r4-unload", &g.folder);
        c.models.deploy(&c.ctx(), spec.clone()).await.unwrap();
        wait_state(&c, "r4-unload", "ready").await;
        container(&c, "cell3", "r4-unload", "sparkbox-r4-unload-cell3").await;
        assert_eq!(standin3.running(), ["sparkbox-r4-unload-cell3"]);
        // vLLM is killed outright (an OOM kill): its /dev/shm segment stays behind. And the node's
        // memory won't come back (something else took it meanwhile).
        let leftover = g.standin.shm();
        assert_eq!(leftover.len(), 1);
        g.standin.kill("sparkbox-r4-unload");
        g.standin.set_knobs(Knobs { mem_available: Knobs::default().mem_available - 10 * GIB, ..Knobs::default() });
        let started = std::time::Instant::now();
        let task = c.models.unload(&c.ctx(), "r4-unload").await.unwrap();
        let t = finished(&g.tasks, &task).await;
        assert!(started.elapsed() < Duration::from_secs(8), "the 1 s wait is the test's: {:?}", started.elapsed());
        assert_eq!(t.title, "Unload gemma-3-27b-it from cell2, cell3");
        assert_eq!(t.state, TaskState::Ok, "Stopped stands: the containers are gone");
        assert_eq!(t.warning.as_deref(), Some("Unloaded, but something didn't come back; see the report."));
        assert!(standin3.running().is_empty() && g.standin.running().is_empty());
        let d = until_load(&c, "r4-unload", |d| d.unload_report.is_some()).await;
        assert_eq!(d.state, DeploymentState::Stopped);
        let r = d.unload_report.unwrap();
        assert!(!r.ok);
        let base = Knobs::default().mem_available;
        let now = base - 10 * GIB;
        assert!(r.summary.starts_with(&format!(
            "Unloaded from cell2, cell3 · cell2: memory not back after 1 s ({} GiB available, baseline {}), port {} closed, /dev/shm: new since the load: {} (",
            gib(now),
            gib(base),
            spec.port,
            leftover[0]
        )), "{}", r.summary);
        assert!(r.summary.contains("); removing them needs sudo and your OK (not done) · cell3: memory: no baseline recorded"), "{}", r.summary);
        assert_eq!(r.nodes.iter().map(|n| (n.node.as_str(), n.stopped, n.mem_back, n.new_shm.clone())).collect::<Vec<_>>(), [("cell2", true, false, leftover.clone()), ("cell3", true, false, vec![])]);
        assert_eq!(g.standin.shm(), leftover, "reported, not removed (that needs sudo and your OK)");
        c.live.stop();
        g.c.live.stop();
    }

    #[tokio::test]
    async fn a_container_that_wont_stop_or_a_node_that_drops_is_never_marked_stopped_nor_rm_f() {
        let g = gb10("r4-stuck", Knobs::default()).await;
        let c = &g.c;
        let spec = gemma("r4-stuck", &g.folder);
        ready(&g, &spec).await;
        // docker can't stop it (TERM ignored and the daemon gives up): rm refuses a running
        // container, the check finds it still there, and SparkBox never forces it.
        let shim = std::fs::read_to_string(g.node.bin_dir.join("docker")).unwrap();
        std::fs::write(g.node.bin_dir.join("docker-real"), &shim).unwrap();
        std::fs::set_permissions(g.node.bin_dir.join("docker-real"), std::fs::Permissions::from_mode(0o755)).unwrap();
        g.node.bin("docker", "[ \"$1\" = stop ] && { echo 'Error response from daemon: cannot stop container: tried to kill container, but did not receive an exit event' >&2; exit 1; }\nexec docker-real \"$@\"\n");
        let t = finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-stuck").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Failed);
        let e = t.error.unwrap();
        assert!(e.starts_with("Its containers on cell2 are still there (docker said: cell2: Error response from daemon: cannot remove container"), "{e}");
        assert!(e.ends_with("it isn't marked stopped."), "{e}");
        assert_eq!(state_of(c, "r4-stuck").0, DeploymentState::Ready);
        assert_eq!(g.standin.running(), ["sparkbox-r4-stuck"]);
        assert!(!g.node.log.lock().unwrap().iter().any(|l| l.contains("rm -f $ids") || l.contains("docker rm -f")));

        // The connection drops as the stop ends (no exit status comes back): SparkBox can't tell
        // from the stop what happened, so it asks docker again, and only then marks it stopped.
        g.node.bin("docker", &shim);
        g.node.close_without_status("docker stop -t 30");
        let t = finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-stuck").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Ok, "{t:?}");
        let d = until_load(c, "r4-stuck", |d| d.unload_report.is_some()).await;
        assert_eq!(d.state, DeploymentState::Stopped);
        assert!(g.standin.running().is_empty());
        let asked = g.node.ran("docker ps -a --filter label=sparkbox.deployment=r4-stuck --format");
        assert!(asked.len() >= 4, "asked before and after each unload: {asked:?}");
        c.live.stop();

        // A node that's gone before the unload: nothing is stopped and it stays as it was.
        let g = gb10("r4-gone", Knobs::default()).await;
        let c = &g.c;
        ready(&g, &gemma("r4-gone", &g.folder)).await;
        g.node.set_login_password(None);
        g.node.drop_connections().await;
        for _ in 0..100 {
            if !c.live.is_connected("cell2") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let e = c.models.unload(&c.ctx(), "r4-gone").await.unwrap_err();
        assert_eq!(e, "Couldn't check gemma-3-27b-it's containers on cell2; nothing was stopped.");
        assert_eq!(state_of(c, "r4-gone").0, DeploymentState::Ready);
        assert_eq!(g.standin.running(), ["sparkbox-r4-gone"]);
        c.live.stop();
    }

    #[tokio::test]
    async fn deployment_history_save_failure_warns_after_cleanup_and_blocks_a_new_load() {
        let g = gb10("history-unload-conflict", Knobs::default()).await;
        let c = &g.c;
        ready(&g, &gemma("history-unload-conflict", &g.folder)).await;
        let path = c.dir.join("deployments.json");
        std::fs::write(&path, "[]").unwrap(); // A competing writer, entirely inside the fixture.
        let t = finished(&g.tasks, &c.models.unload(&c.ctx(), "history-unload-conflict").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Ok, "{t:?}");
        assert!(t.warning.as_deref().is_some_and(|w| w.contains("Couldn't save deployment history")), "{t:?}");
        assert!(g.standin.running().is_empty(), "storage failure must not prevent safe cleanup");
        let d = until_load(c, "history-unload-conflict", |d| d.unload_report.is_some()).await;
        assert!(d.unload_report.as_ref().unwrap().ok, "health result stays honest: {d:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
        assert!(std::fs::read_dir(c.dir.join("deployment-history")).unwrap().any(|f| {
            let copy: Vec<Deployment> = serde_json::from_slice(&std::fs::read(f.unwrap().path()).unwrap()).unwrap();
            copy.iter().any(|saved| saved.unload_report == d.unload_report)
        }));
        let launches = g.node.ran("docker run -d").len();
        let error = c.models.deploy(&c.ctx(), gemma("history-new-load", &g.folder)).await.unwrap_err();
        assert!(error.contains("Couldn't save deployment history"), "{error}");
        assert_eq!(g.node.ran("docker run -d").len(), launches);
        assert!(!c.models.deployments(&c.ctx()).iter().any(|d| d.spec.id == "history-new-load"));
        c.live.stop();
    }

    #[tokio::test]
    async fn forget_unloads_a_failed_load_whose_container_is_still_there_first() {
        let g = gb10("r4-forget", Knobs::default()).await;
        let c = &g.c;
        // A failed load whose container is still there: what a rollback look-only held back leaves.
        let spec = DeploySpec { image_id: fake_vllm::test_image_id(), ..gemma("r4-forget", &g.folder) };
        fake_vllm::launch_files(&c.live, "cell2", &spec, 0, &[], None).await;
        assert_eq!(c.live.run("cell2", &plan::rank_command(&DeploySpec { require_key: false, ..spec.clone() }, 0), None, Duration::from_secs(60)).await.unwrap().code, Some(0));
        c.models.deployments.lock().unwrap().push(Deployment { spec, state: DeploymentState::Failed, ..Default::default() });
        for _ in 0..100 {
            if !g.standin.running().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(g.standin.running(), ["sparkbox-r4-forget"]);
        // Forget: its container is still there, so it's unloaded first, then forgotten.
        let task = c.models.forget_checked(&c.ctx(), "r4-forget").await.unwrap().expect("an unload first");
        let t = finished(&g.tasks, &task).await;
        assert_eq!((t.title.as_str(), t.state), ("Unload gemma-3-27b-it from cell2", TaskState::Ok));
        assert!(g.standin.running().is_empty());
        until_gone(c, "r4-forget").await;

        // Nothing left of it: forgotten at once. The node drops right as the container starts, and
        // the failed start's rollback removes what docker made.
        g.node.close_without_status("docker run -d");
        c.models.deploy(&c.ctx(), gemma("r4-forget2", &g.folder)).await.unwrap();
        wait_state(c, "r4-forget2", "failed").await;
        assert!(g.standin.running().is_empty());
        assert_eq!(c.models.forget_checked(&c.ctx(), "r4-forget2").await.unwrap(), None);
        assert!(!c.models.deployments(&c.ctx()).iter().any(|d| d.spec.id == "r4-forget2"));
        // A load that's serving is unloaded, not forgotten.
        ready(&g, &gemma("r4-forget3", &g.folder)).await;
        assert_eq!(c.models.forget_checked(&c.ctx(), "r4-forget3").await.unwrap_err(), "gemma-3-27b-it is loading or serving: unload it instead.");
        finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-forget3").await.unwrap()).await;
        c.live.stop();

        // Demo: forgotten as before.
        let (models, _llm, _live, cluster, _dir) = demo_app("r4-forget-demo");
        assert_eq!(models.forget_checked(&demo_ctx(&cluster), "none").await.unwrap(), None);
    }

    /// Stops the next gate between its register and its update; the first value says it's there,
    /// the second lets it go on.
    fn hold_the_gate(c: &Cluster) -> (tokio::sync::oneshot::Receiver<()>, tokio::sync::oneshot::Sender<()>) {
        let (reached, at) = tokio::sync::oneshot::channel();
        let (go, wait) = tokio::sync::oneshot::channel();
        *c.models.after_register.lock().unwrap() = Some((reached, wait));
        (at, go)
    }

    #[tokio::test]
    async fn an_unload_that_lands_while_the_gate_registers_stands() {
        let g = gb10("r9a", Knobs::default()).await;
        let c = &g.c;
        let (at, go) = hold_the_gate(c);
        c.models.deploy(&c.ctx(), gemma("r9a", &g.folder)).await.unwrap();
        at.await.unwrap();
        assert!(c.llm.get("vllm-r9a").await.is_some(), "register ran");
        let t = finished(&g.tasks, &c.models.unload(&c.ctx(), "r9a").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Ok, "{t:?}");
        go.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(state_of(c, "r9a").0, DeploymentState::Stopped, "the unload stands");
        assert!(c.llm.get("vllm-r9a").await.is_none(), "no provider left");
        c.live.stop();

        // Its update came before register's: the unload found no provider to remove, so what
        // register added is the gate's to take away.
        let g = gb10("r9a-early", Knobs::default()).await;
        let c = &g.c;
        let (at, go) = hold_the_gate(c);
        c.models.deploy(&c.ctx(), gemma("r9a-early", &g.folder)).await.unwrap();
        at.await.unwrap();
        c.models.update("r9a-early", |d| {
            d.state = DeploymentState::Stopped;
            d.message = "Unloaded".into();
        });
        go.send(()).unwrap();
        for _ in 0..100 {
            if c.llm.get("vllm-r9a-early").await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(c.llm.get("vllm-r9a-early").await.is_none(), "no provider left");
        let d = c.models.deployments(&c.ctx()).remove(0);
        assert_eq!((d.state, d.message.as_str(), d.provider_id.as_str()), (DeploymentState::Stopped, "Unloaded", ""));
        c.live.stop();
    }

    #[tokio::test]
    async fn a_restart_that_became_ready_keeps_the_provider_when_the_old_gate_wakes() {
        let g = gb10("r9a-restart", Knobs::default()).await;
        let c = &g.c;
        let (at, go) = hold_the_gate(c);
        c.models.deploy(&c.ctx(), gemma("r9a-restart", &g.folder)).await.unwrap();
        at.await.unwrap();
        // A newer start of this id got there first: the older gate leaves it and its provider be.
        c.models.update("r9a-restart", |d| {
            d.generation += 1;
            d.state = DeploymentState::Ready;
            d.message = "Serving".into();
        });
        go.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(c.llm.get("vllm-r9a-restart").await.is_some());
        assert_eq!(state_of(c, "r9a-restart"), (DeploymentState::Ready, "Serving".into()));
        c.live.stop();
    }

    /// `llm.remove` clears `llm:vllm-<id>`, which is also the load's own key: a restart still
    /// starting needs it for its test token, or it can never read Ready.
    #[tokio::test]
    async fn a_restart_still_starting_keeps_its_key_when_the_old_gate_wakes() {
        let g = gb10("r9a-key", Knobs::default()).await;
        let c = &g.c;
        let (at, go) = hold_the_gate(c);
        c.models.deploy(&c.ctx(), gemma("r9a-key", &g.folder)).await.unwrap();
        at.await.unwrap();
        let account = plan::key_account("r9a-key");
        assert!(secrets::get(&account).unwrap().is_some(), "the load has a key");
        c.models.update("r9a-key", |d| d.generation += 1);
        go.send(()).unwrap();
        for _ in 0..200 {
            if c.llm.get("vllm-r9a-key").await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(c.llm.get("vllm-r9a-key").await.is_none(), "the older gate's provider goes");
        assert!(secrets::get(&account).unwrap().is_some(), "the newer start's key stays");
        let _ = secrets::clear(&account);
        c.live.stop();
    }

    #[tokio::test]
    async fn a_link_drop_during_docker_run_says_what_to_check() {
        let g = gb10("r9d", Knobs::default()).await;
        let c = &g.c;
        g.node.close_without_status("docker run -d");
        c.models.deploy(&c.ctx(), gemma("r9d", &g.folder)).await.unwrap();
        let d = wait_state(c, "r9d", "failed").await;
        assert_eq!(d.message, "cell2: docker run didn't finish: the connection dropped before it answered. Check Containers on cell2, then unload or try again.");
        c.live.stop();
    }

    async fn until_gone(c: &Cluster, id: &str) {
        for _ in 0..200 {
            if !c.models.deployments(&c.ctx()).iter().any(|d| d.spec.id == id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{id} wasn't forgotten: {:?}", c.models.deployments(&c.ctx()));
    }

    /// cell2's `docker run` takes `secs` before it makes the container (a busy daemon); the rest
    /// of docker answers at once.
    fn slow_run(g: &Gb10, secs: u32) {
        let shim = std::fs::read_to_string(g.node.bin_dir.join("docker")).unwrap();
        std::fs::write(g.node.bin_dir.join("docker-real"), &shim).unwrap();
        std::fs::set_permissions(g.node.bin_dir.join("docker-real"), std::fs::Permissions::from_mode(0o755)).unwrap();
        g.node.bin("docker", &format!("[ \"$1\" = run ] && sleep {secs}\nexec docker-real \"$@\"\n"));
    }

    /// Waits until cell2 has been sent its `n`th `docker run`; when the test saw it.
    async fn run_sent(g: &Gb10, n: usize) -> std::time::Instant {
        for _ in 0..400 {
            if g.node.ran("docker run -d").len() >= n {
                return std::time::Instant::now();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("docker run #{n} never came: {:?}", g.node.ran("docker run"));
    }

    /// The load's containers on cell2 as docker lists them, stopped ones too (a read).
    async fn containers_of(c: &Cluster, id: &str) -> String {
        let out = c.live.run("cell2", &format!("docker ps -aq --filter label=sparkbox.deployment={id}"), None, Duration::from_secs(10)).await.unwrap();
        out.stdout.trim().to_string()
    }

    #[tokio::test]
    async fn an_unload_pressed_during_the_docker_run_waits_for_it_and_removes_what_it_made() {
        let g = gb10("r4-race", Knobs::default()).await;
        let c = &g.c;
        // docker run takes 3 s to make the container; Unload is pressed meanwhile.
        slow_run(&g, 3);
        c.models.deploy(&c.ctx(), gemma("r4-race", &g.folder)).await.unwrap();
        let sent = run_sent(&g, 1).await;
        let task = c.models.unload(&c.ctx(), "r4-race").await.unwrap();
        assert!(sent.elapsed() >= Duration::from_millis(2900), "the unload waited for the run: {:?}", sent.elapsed());
        let t = finished(&g.tasks, &task).await;
        assert_eq!(t.state, TaskState::Ok, "{t:?}");
        // It found the container the run made, and stopped it.
        let d = until_load(c, "r4-race", |d| d.unload_report.is_some()).await;
        assert!(d.message.starts_with("Unloaded from cell2 · "), "{}", d.message);
        // Past the run and the watcher's first look: nothing of it on cell2, and it stays Stopped.
        tokio::time::sleep_until((sent + Duration::from_secs(8)).into()).await;
        assert_eq!(containers_of(c, "r4-race").await, "", "no container of it left");
        assert!(g.standin.running().is_empty());
        assert!(!g.node.home.join(".sparkbox/keys/r4-race").exists());
        assert_eq!(state_of(c, "r4-race"), (DeploymentState::Stopped, d.message));
        c.live.stop();
    }

    #[tokio::test]
    async fn an_unload_pressed_during_a_restarts_docker_run_waits_for_it_too() {
        let g = gb10("r4-race2", Knobs::default()).await;
        let c = &g.c;
        ready(&g, &gemma("r4-race2", &g.folder)).await;
        slow_run(&g, 3);
        c.models.restart(&c.ctx(), "r4-race2").await.unwrap();
        let sent = run_sent(&g, 2).await;
        let t = finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-race2").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Ok, "{t:?}");
        let d = until_load(c, "r4-race2", |d| d.unload_report.is_some()).await;
        assert!(d.message.starts_with("Unloaded from cell2 · "), "{}", d.message);
        tokio::time::sleep_until((sent + Duration::from_secs(8)).into()).await;
        assert_eq!(containers_of(c, "r4-race2").await, "", "no container of it left");
        assert!(g.standin.running().is_empty());
        assert_eq!(state_of(c, "r4-race2").0, DeploymentState::Stopped);
        c.live.stop();
    }

    #[tokio::test]
    async fn an_unloaded_load_stays_stopped_when_its_watcher_looks_again() {
        let g = gb10("r4-stays", Knobs::default()).await;
        let c = &g.c;
        ready(&g, &gemma("r4-stays", &g.folder)).await;
        let t = finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-stays").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Ok);
        let d = until_load(c, "r4-stays", |d| d.unload_report.is_some()).await;
        // A Ready load's watcher looks every 20 s: past its next look, the unload's word stands.
        tokio::time::sleep(Duration::from_secs(23)).await;
        assert_eq!(state_of(c, "r4-stays"), (DeploymentState::Stopped, d.message));
        c.live.stop();
    }

    #[tokio::test]
    async fn edit_and_reload_counts_the_loads_own_port_and_memory_as_coming_back() {
        let g = gb10("r4-reload", Knobs::default()).await;
        let c = &g.c;
        let spec = gemma("r4-reload", &g.folder);
        ready(&g, &spec).await;
        // The snapshot lists vLLM's listener on the port, and keeps listing it after the unload
        // (the collector reads listeners every 5 s).
        let mut frame: Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
        frame["listeners"] = json!([{ "port": spec.port, "addr": "127.0.0.1", "pid": null, "process": null }]);
        *g.node.knobs.frame.lock().unwrap() = Some(frame.to_string());
        let listed = || c.ctx().snapshot.unwrap().nodes.iter().any(|n| n.id == "cell2" && n.listeners.iter().any(|l| l.port == spec.port));
        for _ in 0..100 {
            if listed() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(listed());

        // The Load page in Edit & reload: the same id and port, read while the load still serves.
        let p = c.models.preflight(&c.ctx(), &spec).await.unwrap().nodes.remove(0);
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        assert!(!p.drop_needed, "its share comes back at the unload: {p:?}");
        let m = p.mem.clone().unwrap();
        assert!(m.free < p.need_free && m.free + p.freed >= p.need_free, "{p:?}");
        assert_eq!(p.freed, (m.total as f64 * 0.75) as u64);
        assert_eq!(p.replacing.as_deref(), Some("gemma-3-27b-it"));
        assert_eq!(p.port_owner.as_deref(), Some("gemma-3-27b-it, the load being replaced; free once it's unloaded"));
        assert_eq!(c.models.plan(&c.ctx(), &spec).await.unwrap().port_taken_by, None);
        // Another load on that port is still told it's taken, and short of memory.
        let other = DeploySpec { id: "r4-other".into(), ..spec.clone() };
        let p = c.models.preflight(&c.ctx(), &other).await.unwrap().nodes.remove(0);
        assert_eq!(p.problems, [format!("Port {0} is already in use on cell2 (listening on 127.0.0.1:{0}). Pick another port.", spec.port)]);
        assert!(p.drop_needed && p.replacing.is_none() && p.freed == 0);
        assert!(c.models.plan(&c.ctx(), &other).await.unwrap().port_taken_by.is_some());

        // Reload as the page runs it: unload, then load with the new settings. The node's own
        // read says the port is free; the snapshot's stale listener doesn't refuse it.
        let t = finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-reload").await.unwrap()).await;
        assert_eq!(t.state, TaskState::Ok);
        let d = c.models.deploy(&c.ctx(), DeploySpec { max_model_len: Some(16384), ..spec.clone() }).await.unwrap();
        assert_eq!(d.spec.max_model_len, Some(16384));
        wait_state(c, "r4-reload", "ready").await;
        finished(&g.tasks, &c.models.unload(&c.ctx(), "r4-reload").await.unwrap()).await;
        c.live.stop();
    }
}

// ---- The Hugging Face token (C-T), job secrets (C-S) and partial folders (C-P)

const TEST_TOKEN: &str = "hf_testtoken_0123456789abcdefSECRET";

/// docker on a fake node, as Docker behaves towards a signal. `run` records its argv, its
/// environment and what Docker would keep of the container (the -e values, -v mounts and labels),
/// then stays running until told to finish, as hf does while it downloads, and writes the repo's two
/// files into /dest. It ignores SIGTERM, as hf does as a container's PID 1, and the container is the
/// daemon's: it outlives a kill of the CLI, and only `docker stop` ends it. `ps` answers from the
/// record, by label; `inspect` answers from it too, as the real one answers from the container's
/// config. A `run` while a container is still up leaves `second-writer`, and every call is in `calls`.
fn token_tool_review() -> DownloadToolReview {
    DownloadToolReview { node: "cell1".into(), install_consent: false,
        tool: DownloadTool { kind: "image".into(), path: format!("sha256:{}", "b".repeat(64)), fingerprint: format!("sha256:{}", "b".repeat(64)), label: "vllm:test".into() } }
}

const TOKEN_DOCKER: &str = r#"#!/usr/bin/env python3
import json, os, signal, sys, time
state = os.path.join(os.environ["HOME"], "docker-shim")
os.makedirs(state, exist_ok=True)
a = sys.argv[1:]
record = os.path.join(state, "run.json")
stopped = os.path.join(state, "stopped")
with open(os.path.join(state, "calls"), "a") as f:
    f.write(json.dumps(a) + "\n")
if a[:2] == ["image", "inspect"]:
    print("sha256:" + "b" * 64)
    sys.exit(0)
if a[:2] == ["image", "ls"]:
    print(json.dumps({"ID":"sha256:" + "b" * 64, "Repository":"vllm", "Tag":"test"}))
    sys.exit(0)
if a[:1] == ["run"] and a[-1:] == ["version"]:
    print("hf 2.4.0")
    sys.exit(0)
if a[:1] == ["inspect"]:
    run = json.load(open(record))
    print(json.dumps([{"Config": {"Env": run["env"], "Cmd": run["argv"]}, "HostConfig": {"Binds": run["binds"]}}]))
    sys.exit(0)
if a[:1] == ["ps"]:
    wanted = [x[len("label="):] for i, x in enumerate(a) if a[i - 1] == "--filter"]
    if os.path.exists(record) and not os.path.exists(stopped):
        run = json.load(open(record))
        labels = [run["argv"][i + 1] for i, x in enumerate(run["argv"]) if x == "--label"]
        if all(w in labels for w in wanted):
            print(run["id"])
    sys.exit(0)
if a[:1] == ["stop"]:
    if os.path.exists(os.path.join(state, "stuck")):
        print("Error response from daemon: cannot stop container", file=sys.stderr)
        sys.exit(1)
    open(stopped, "w").close()
    sys.exit(0)
if a[:1] != ["run"]:
    sys.exit(0)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
if os.path.exists(record) and not os.path.exists(stopped):
    open(os.path.join(state, "second-writer"), "w").close()
if os.path.exists(stopped):
    os.unlink(stopped)
flags = [a[i + 1] for i, x in enumerate(a) if x == "-e"]
env = [f if "=" in f else "%s=%s" % (f, os.environ.get(f, "")) for f in flags]
binds = [a[i + 1] for i, x in enumerate(a) if x == "-v"]
json.dump({"id": "c0ffee%06d" % os.getpid(), "argv": a, "env": env, "binds": binds, "environ": dict(os.environ), "pid": os.getpid()}, open(record + ".tmp", "w"))
os.replace(record + ".tmp", record)
finish = os.path.join(state, "finish")
deadline = time.time() + 60
while not os.path.exists(finish):
    if os.path.exists(stopped):
        sys.exit(137)
    if time.time() > deadline:
        sys.exit(1)
    time.sleep(0.05)
os.unlink(finish)
os.unlink(record)
dest = [b.split(":")[0] for b in binds if b.split(":")[1] == "/dest"][0]
open(os.path.join(dest, "config.json"), "w").write("{}")
open(os.path.join(dest, "model.safetensors"), "w").write("w" * 64)
"#;

/// A small repo as Hugging Face's API lists it.
fn small_repo(repo: &str) -> HfModel {
    let info = json!({ "id": repo, "sha": "abc123", "siblings": [
        { "rfilename": "config.json", "size": 2 },
        { "rfilename": "model.safetensors", "size": 64 },
    ] });
    hf::parse_model(&info, json!({}), None, "")
}

/// One fake node with the docker above, its `run` as a job runs it.
struct TokenNode {
    node: crate::fake_node::Node,
    home: PathBuf,
    c: Cluster,
}

impl TokenNode {
    async fn new(name: &str) -> TokenNode {
        let home = temp(&format!("{name}-home"));
        let (node, port) = fake_node(&home, real_shell()).await;
        node.bin("docker", TOKEN_DOCKER);
        let c = cluster(name, &[("cell1", port)]).await;
        TokenNode { node, home, c }
    }

    fn shim(&self, name: &str) -> PathBuf {
        self.home.join("docker-shim").join(name)
    }

    /// What `docker run` recorded, once it ran.
    async fn run(&self) -> Value {
        crate::fake_node::wait_for("the download container's run record", || {
            std::fs::read_to_string(self.shim("run.json")).ok().map(|text| serde_json::from_str(&text).unwrap())
        }, || format!("Commands: {:?}\nDocker calls: {:?}", self.node.log.lock().unwrap(), self.calls("run"))).await
    }

    /// What `docker run` recorded once it's a newer run than `old`.
    async fn run_after(&self, old: &Value) -> Value {
        crate::fake_node::wait_for("a newer download container's run record", || {
            let text = std::fs::read_to_string(self.shim("run.json")).ok()?;
            let run: Value = serde_json::from_str(&text).unwrap();
            (run["id"] != old["id"]).then_some(run)
        }, || format!("Previous: {}\nCommands: {:?}", old["id"], self.node.log.lock().unwrap())).await
    }

    fn inspect(&self) -> String {
        let out = std::process::Command::new(self.node.bin_dir.join("docker")).arg("inspect").arg("x").env("HOME", &self.home).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The docker calls of one verb, each as its argv.
    fn calls(&self, verb: &str) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(self.shim("calls")).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str::<Vec<String>>(l).unwrap()).filter(|c| c.first().is_some_and(|v| v == verb)).collect()
    }

    /// Whether a container is up: it's the daemon's, so this is true after its CLI has gone.
    fn container_up(&self) -> bool {
        self.shim("run.json").exists() && !self.shim("stopped").exists()
    }

    fn status(&self, id: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.home.join(format!(".sparkbox/jobs/{id}.json"))).unwrap()).unwrap()
    }

    async fn until_state(&self, id: &str, state: &str) {
        for _ in 0..400 {
            if self.status(id)["state"] == state {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{id} never reached {state}: {:?}", self.status(id));
    }

    fn token_folder(&self, id: &str) -> PathBuf {
        self.home.join(".sparkbox/job-secrets").join(id)
    }

    /// Lets the running `docker run` end, and forgets its record for the next one.
    fn finish(&self) {
        std::fs::write(self.shim("finish"), "").unwrap();
    }

    fn forget_run(&self) {
        let _ = std::fs::remove_file(self.shim("run.json"));
    }
}

/// Look-only changes nothing on a node, even by the upkeep `list` does: on the real helper over a
/// real shell, a dead job reads failed in both, and only with look-only off is its status rewritten
/// and its token folder removed.
#[tokio::test]
async fn in_look_only_listing_jobs_changes_nothing_on_the_node() {
    let t = TokenNode::new("look-list").await;
    let ctx = t.c.ctx();
    let gate = crate::admin::AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
    gate.set_look_only(true).unwrap();
    t.c.live.set_write_policy(gate.clone());
    t.c.models.set_gate(gate.clone());
    // A job a reboot or a SIGKILL left: it says running, it has no process, and its token's there.
    let id = "job-dead-look";
    let jobs = t.home.join(".sparkbox/jobs");
    std::fs::create_dir_all(&jobs).unwrap();
    let status = jobs.join(format!("{id}.json"));
    let saved = json!({ "id": id, "kind": "download", "state": "running", "pid": 999_999_999u64, "startedAt": 1 }).to_string();
    std::fs::write(&status, &saved).unwrap();
    let folder = t.token_folder(id);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("hf_token"), TEST_TOKEN).unwrap();

    let listed = t.c.models.jobs(&ctx).await.unwrap();
    assert_eq!(listed.iter().find(|j| j.id == id).unwrap().state, "failed", "it still reads as dead");
    assert_eq!(std::fs::read_to_string(&status).unwrap(), saved, "look-only rewrote a job's status");
    assert!(folder.join("hf_token").exists(), "look-only removed a job's token");
    assert!(t.node.ran("sparkjob.py list --look").len() == 1, "{:?}", t.node.ran("sparkjob.py list"));

    gate.set_look_only(false).unwrap();
    let listed = t.c.models.jobs(&ctx).await.unwrap();
    assert_eq!(listed.iter().find(|j| j.id == id).unwrap().state, "failed");
    assert_eq!(t.status(id)["state"], "failed", "with look-only off the status is written");
    assert!(!folder.exists(), "with look-only off the token goes");
    t.c.live.stop();
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn kill_9(pid: u64) {
    assert!(std::process::Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap().success());
}

#[tokio::test]
async fn a_download_token_is_in_no_argv_env_or_inspect() {
    let t = TokenNode::new("token").await;
    let ctx = t.c.ctx();
    secrets::set(HF_ACCOUNT, TEST_TOKEN).unwrap();

    // While it runs: nowhere Docker or the job keeps things, only in the mounted file.
    let job = t.c.models.download_model(&ctx, &small_repo("org/tiny"), &token_tool_review()).await.unwrap();
    let run = t.run().await;
    let instance = crate::identity::instance_name();
    let words: Vec<String> = run["argv"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_string()).collect();
    let name = format!("sparkbox-dl-{}", job.id);
    assert_eq!(
        words[..8],
        ["run", "--rm", "--name", &name, "--label", &format!("sparkbox.instance={instance}"), "--label", &format!("sparkbox.deployment={}", job.id)],
        "C-T's command"
    );
    let log = std::fs::read_to_string(t.home.join(format!(".sparkbox/jobs/{}.log", job.id))).unwrap_or_default();
    let status = t.status(&job.id).to_string();
    for (what, text) in [("argv", run["argv"].to_string()), ("environment", run["environ"].to_string()), ("run", run.to_string()), ("docker inspect", t.inspect()), ("job status", status), ("job log", log)] {
        assert!(!text.contains(TEST_TOKEN), "the token is in the {what}: {text}");
    }
    assert!(run["environ"].get("HF_TOKEN").is_none(), "{:?}", run["environ"]);
    let env: Vec<&str> = run["env"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
    assert!(env.contains(&"HF_TOKEN_PATH=/run/sparkbox/hf_token"), "{env:?}");
    assert!(!env.iter().any(|e| e.starts_with("HF_TOKEN=")), "{env:?}");
    let folder = t.token_folder(&job.id);
    let file = folder.join("hf_token");
    assert!(run["binds"].as_array().unwrap().iter().any(|b| *b == format!("{}:/run/sparkbox/hf_token:ro", file.display())), "{:?}", run["binds"]);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), TEST_TOKEN);
    assert_eq!((mode(&file), mode(&folder)), (0o400, 0o700));
    // SparkBox's own records: the token reaches the node on stdin alone.
    assert!(!t.node.raw.lock().unwrap().iter().any(|c| c.contains(TEST_TOKEN)), "on a command line");
    assert!(!format!("{:?}", t.c.live.audit.entries()).contains(TEST_TOKEN), "in the activity log");
    assert_eq!(t.node.specs.lock().unwrap().last().unwrap()["token"], TEST_TOKEN);

    // A second download of that folder is refused while it runs (list's dest).
    let e = t.c.models.download_model(&ctx, &small_repo("org/tiny"), &token_tool_review()).await.unwrap_err();
    assert_eq!(e, "A download of org/tiny is already running on cell1. Follow it in Jobs.");
    assert_eq!(t.status(&job.id)["dest"], t.home.join("models/org/tiny").display().to_string());

    // A normal end.
    t.finish();
    t.until_state(&job.id, "done").await;
    assert!(!folder.exists(), "the token outlived the job");

    // Cancel: the container stops, found by both labels, the folder goes, and the Library says the
    // download stopped partway.
    let job = t.c.models.download_model(&ctx, &small_repo("org/tiny2"), &token_tool_review()).await.unwrap();
    let run = t.run().await;
    let folder = t.token_folder(&job.id);
    assert!(folder.join("hf_token").exists());
    t.c.models.cancel_job(&ctx, "cell1", &job.id).await.unwrap();
    assert!(!folder.exists(), "the token outlived the cancel");
    assert_eq!(t.status(&job.id)["state"], "cancelled");
    assert!(!t.container_up(), "Cancel leaves no container writing");
    let wanted = ["ps", "-q", "--filter", &format!("label=sparkbox.deployment={}", job.id), "--filter", &format!("label=sparkbox.instance={instance}")];
    assert!(t.calls("ps").iter().any(|c| *c == wanted), "{:?}", t.calls("ps"));
    assert_eq!(t.calls("stop"), [["stop", "-t", "5", run["id"].as_str().unwrap()]]);
    let lib = t.c.models.library(&ctx).await.unwrap();
    let copy = &lib.iter().find(|e| e.repo == "org/tiny2").expect("listed though no weights finished").copies["cell1"];
    assert_eq!((copy.partial.as_deref(), copy.verified, copy.check.as_str()), (Some("stopped"), false, ""));

    // SIGKILL: no finally runs, and the next `list` takes the folder. It stops nothing: the
    // container is still writing, and the job's message says it may be.
    t.forget_run();
    let job = t.c.models.download_model(&ctx, &small_repo("org/tiny3"), &token_tool_review()).await.unwrap();
    let run = t.run().await;
    let folder = t.token_folder(&job.id);
    kill_9(t.status(&job.id)["pid"].as_u64().unwrap());
    kill_9(run["pid"].as_u64().unwrap());
    assert!(folder.join("hf_token").exists(), "nothing ran after the SIGKILL");
    let stops = t.calls("stop").len();
    let jobs = t.c.models.jobs(&ctx).await.unwrap();
    assert!(!folder.exists(), "list left a dead job's token");
    let dead = jobs.iter().find(|j| j.id == job.id).unwrap();
    assert_eq!(dead.state, "failed");
    assert_eq!(dead.message, "The job stopped unexpectedly (node restarted?). Its container may still be running: Resume download stops it first.");
    assert_eq!(t.calls("stop").len(), stops, "list is a read");
    assert!(t.container_up(), "the container outlives its CLI");

    // Resume, through the same download(): the old container stops first, found through the
    // marker (the new job has a new id), so there's one writer.
    let retry = t.c.models.download_model(&ctx, &small_repo("org/tiny3"), &token_tool_review()).await.unwrap();
    assert_ne!(retry.id, job.id);
    let second = t.run_after(&run).await;
    assert!(!t.shim("second-writer").exists(), "two containers wrote to one folder");
    assert_eq!(t.calls("stop").last().unwrap(), &["stop", "-t", "5", run["id"].as_str().unwrap()]);
    t.finish();
    t.until_state(&retry.id, "done").await;
    assert_ne!(second["id"], run["id"]);

    // No token: neither the variable nor the mount.
    secrets::clear(HF_ACCOUNT).unwrap();
    let job = t.c.models.download_model(&ctx, &small_repo("org/tiny4"), &token_tool_review()).await.unwrap();
    let run = t.run().await;
    assert!(!run["argv"].to_string().contains("HF_TOKEN"), "{:?}", run["argv"]);
    assert_eq!(run["binds"].as_array().unwrap().len(), 1, "{:?}", run["binds"]);
    assert!(!t.token_folder(&job.id).exists());
    t.finish();
    t.until_state(&job.id, "done").await;
    t.c.live.stop();
}

#[tokio::test]
async fn cancel_says_so_when_the_container_is_still_running() {
    let t = TokenNode::new("cancel-stuck").await;
    let ctx = t.c.ctx();
    let job = t.c.models.download_model(&ctx, &small_repo("org/stuck"), &token_tool_review()).await.unwrap();
    t.run().await;
    std::fs::write(t.shim("stuck"), "").unwrap();
    let e = t.c.models.cancel_job(&ctx, "cell1", &job.id).await.unwrap_err();
    assert_eq!(e, "Canceled, but its container may still be running: Resume download stops it first.");
    assert_eq!((t.status(&job.id)["state"].as_str(), t.status(&job.id)["message"].as_str()), (Some("cancelled"), Some(e.as_str())));
    assert!(t.container_up(), "the words are true");

    // One that stops is a plain Cancel.
    std::fs::remove_file(t.shim("stuck")).unwrap();
    let job = t.c.models.download_model(&ctx, &small_repo("org/unstuck"), &token_tool_review()).await.unwrap();
    t.c.models.cancel_job(&ctx, "cell1", &job.id).await.unwrap();
    assert_eq!(t.status(&job.id)["message"], "Canceled");
    t.c.live.stop();
}

/// A scan's line for a folder on one node, as sparkjob.py prints it.
fn scan_line(key: &str, verified: bool, partial: Option<&str>) -> Value {
    json!({ "path": key, "root": "/home/sparkuser/models", "repo": "Qwen/Qwen3-8B", "revision": "abc123", "sizeBytes": 4096, "verified": verified,
            "partial": partial, "checked": if verified { "sha256" } else { "" }, "format": "safetensors" })
}

/// Fake nodes answering `share`'s calls: scan with `lines` (one a node), check, manifest, and start
/// recorded in `specs`.
async fn share_cluster(name: &str, lines: Vec<Vec<Value>>) -> (Cluster, Vec<crate::fake_node::Node>) {
    let mut nodes = Vec::new();
    let mut ports = Vec::new();
    for (i, line) in lines.into_iter().enumerate() {
        let n = i as u8 + 1;
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py disk-preflight") { return Some(disk::fixture_read(2)); }
            if cmd.contains("sparkjob.py scan") {
                return Some((Value::from(line.clone()).to_string(), 0));
            }
            if cmd.contains("sparkjob.py manifest") {
                return Some((json!({ "path": "x", "repo": "Qwen/Qwen3-8B", "revision": "abc123", "files": [{ "path": "config.json", "size": 2 }] }).to_string(), 0));
            }
            if cmd.contains("sparkjob.py check") {
                return Some((check_reply(n), 0));
            }
            if cmd.contains("sparkjob.py start") {
                let id = cmd.split_whitespace().last().unwrap().to_string();
                return Some((json!({ "id": id, "state": "running", "step": "starting" }).to_string(), 0));
            }
            if cmd.contains("sparkjob.py list") {
                return Some(("[]".into(), 0));
            }
            if cmd.contains("sparkjob.py delete") {
                return Some(("{}".into(), 0));
            }
            None
        });
        let (node, port) = fake_node(&temp(&format!("{name}-{n}")), script).await;
        nodes.push(node);
        ports.push(port);
    }
    let names = ["cell1", "cell2", "cell3"];
    let inventory: Vec<(&str, u16)> = ports.iter().enumerate().map(|(i, p)| (names[i], *p)).collect();
    (cluster(name, &inventory).await, nodes)
}

const SHARE_KEY: &str = "/home/sparkuser/models/Qwen/Qwen3-8B";

#[tokio::test]
async fn share_never_copies_from_a_partial_folder() {
    // cell1 sorts first and is partial; cell2's copy was found on disk: it's the source.
    let (c, nodes) = share_cluster("share-partial", vec![vec![scan_line(SHARE_KEY, false, Some("stopped"))], vec![scan_line(SHARE_KEY, false, None)], vec![]]).await;
    let jobs = c.models.share(&c.ctx(), SHARE_KEY, &["cell3".into()]).await.unwrap();
    assert_eq!(jobs[0].node, "cell3");
    assert_eq!(nodes[1].specs.lock().unwrap()[0]["path"], SHARE_KEY, "the file server runs on the finished copy");
    assert!(nodes[0].ran("start serve").is_empty(), "never on the partial one");
    let pull = nodes[2].specs.lock().unwrap()[0].clone();
    assert_eq!((pull["repo"].as_str(), pull["revision"].as_str()), (Some("Qwen/Qwen3-8B"), Some("abc123")));

    // Only partial copies: nothing starts.
    let (c, nodes) = share_cluster("share-none", vec![vec![scan_line(SHARE_KEY, false, Some("stopped"))], vec![scan_line(SHARE_KEY, false, Some("downloading"))], vec![]]).await;
    assert_eq!(c.models.share(&c.ctx(), SHARE_KEY, &["cell3".into()]).await.unwrap_err(), "No node has a finished copy of that model.");
    assert!(nodes.iter().all(|n| n.ran("sparkjob.py start").is_empty()));
}

#[tokio::test]
async fn delete_touches_no_node_while_a_job_is_writing_to_a_picked_one() {
    for running in ["copying", "downloading"] {
        let (c, nodes) = share_cluster(&format!("delete-running-{running}"), vec![vec![scan_line(SHARE_KEY, true, None)], vec![scan_line(SHARE_KEY, false, Some(running))]]).await;
        let e = c.models.delete(&c.ctx(), SHARE_KEY, &["cell1".into(), "cell2".into()]).await.unwrap_err();
        assert_eq!(e, "Still running on cell2. Follow it in Jobs.");
        assert!(nodes.iter().all(|n| n.ran("sparkjob.py delete").is_empty()), "cell1's finished copy stays too");
        // A node that wasn't picked doesn't hold up the others.
        c.models.delete(&c.ctx(), SHARE_KEY, &["cell1".into()]).await.unwrap();
        assert_eq!(nodes[0].ran("sparkjob.py delete").len(), 1);
    }
    // A copy that has stopped is the owner's to delete.
    let (c, nodes) = share_cluster("delete-stopped", vec![vec![scan_line(SHARE_KEY, false, Some("stopped"))]]).await;
    c.models.delete(&c.ctx(), SHARE_KEY, &["cell1".into()]).await.unwrap();
    assert_eq!(nodes[0].ran("sparkjob.py delete").len(), 1);
}

#[tokio::test]
async fn share_refuses_a_target_that_is_still_copying() {
    for running in ["copying", "downloading"] {
        let (c, nodes) = share_cluster(&format!("share-running-{running}"), vec![vec![scan_line(SHARE_KEY, true, None)], vec![scan_line(SHARE_KEY, false, Some(running))]]).await;
        let e = c.models.share(&c.ctx(), SHARE_KEY, &["cell2".into()]).await.unwrap_err();
        assert_eq!(e, "A copy to cell2 is already running. Follow it in Jobs.");
        assert!(nodes.iter().all(|n| n.ran("sparkjob.py start").is_empty()), "nothing started");
    }
}

#[tokio::test]
async fn share_retries_a_target_that_stopped_partway() {
    let (c, nodes) = share_cluster("share-retry", vec![vec![scan_line(SHARE_KEY, true, None)], vec![scan_line(SHARE_KEY, false, Some("stopped"))]]).await;
    let jobs = c.models.share(&c.ctx(), SHARE_KEY, &["cell2".into()]).await.unwrap();
    assert_eq!(jobs[0].node, "cell2");
    assert_eq!(nodes[1].specs.lock().unwrap()[0]["dest"], SHARE_KEY);
}

#[tokio::test]
async fn share_refuses_a_hugging_face_cache_folder() {
    let key = "/home/sparkuser/.cache/huggingface/hub/models--google--gemma-3-27b-it/snapshots/005ad34";
    assert!(is_hf_cache(key));
    assert!(!is_hf_cache(SHARE_KEY) && !is_hf_cache("/home/sparkuser/models/models--org--name") && !is_hf_cache("/home/sparkuser/models/snapshots/x"));
    let (c, nodes) = share_cluster("share-cache", vec![vec![scan_line(key, false, None)], vec![]]).await;
    assert_eq!(c.models.share(&c.ctx(), key, &["cell2".into()]).await.unwrap_err(), "Copying a Hugging Face cache model isn't supported yet.");
    assert!(nodes.iter().all(|n| n.ran("sparkjob.py start").is_empty()), "nothing started");
}

#[tokio::test]
async fn the_library_passes_on_what_scan_says_about_each_copy() {
    let home = temp("library-partial");
    model_folder(&home, "org/stopped", false);
    std::fs::write(home.join("models/org/stopped/.sparkbox.partial"), "{\"job\": \"gone\", \"kind\": \"pull\", \"at\": 1}").unwrap();
    let copied = model_folder(&home, "org/copied", false);
    std::fs::write(Path::new(&copied).join(".sparkbox.json"), json!({ "repo": "org/copied", "revision": "r", "files": [], "checked": "sizes", "verifiedAt": 1 }).to_string()).unwrap();
    model_folder(&home, "org/older", true);
    let (_a, port) = fake_node(&home, real_shell()).await;
    let c = cluster("library-partial", &[("cell1", port)]).await;
    let lib = c.models.library(&c.ctx()).await.unwrap();
    let copy = |repo: &str| lib.iter().find(|e| e.repo == repo).unwrap().copies["cell1"].clone();
    assert_eq!((copy("org/stopped").verified, copy("org/stopped").partial.as_deref(), copy("org/stopped").check.as_str()), (false, Some("stopped"), ""));
    assert_eq!((copy("org/copied").verified, copy("org/copied").partial.as_deref(), copy("org/copied").check.as_str()), (true, None, "sizes"));
    assert_eq!((copy("org/older").verified, copy("org/older").partial.as_deref(), copy("org/older").check.as_str()), (true, None, "sha256"));
    // partial isn't sent to the UI when there's nothing to say.
    let wire = serde_json::to_value(copy("org/older")).unwrap();
    assert!(wire.get("partial").is_none(), "{wire}");
    assert_eq!(wire["check"], "sha256");
    c.live.stop();
}

#[tokio::test]
async fn a_folder_left_partway_reads_as_missing_on_a_real_shell() {
    let home = temp("preflight-partial");
    let folder = model_folder(&home, "org/m", true);
    let (_a, port) = fake_node(&home, real_shell()).await;
    let c = cluster("preflight-partial", &[("cell1", port)]).await;
    let spec = DeploySpec { model_path: folder.clone(), image_ref: "x".into(), ..spec(&["cell1"], 1, 1) };
    let read = || async {
        let out = c.live.run("cell1", &plan::preflight_script(&spec), None, Duration::from_secs(15)).await.unwrap();
        plan::parse_preflight("cell1", &spec, &out.stdout)
    };
    assert!(read().await.model_present, "a finished folder is present");
    std::fs::write(Path::new(&folder).join(".sparkbox.partial"), "{}").unwrap();
    let p = read().await;
    assert!(!p.model_present, "{p:?}");
    assert!(p.problems.contains(&format!("cell1 doesn't have {folder} yet. Copy it to that node first (Library › Copy to nodes…).")), "{:?}", p.problems);
    c.live.stop();
}

#[tokio::test]
async fn a_several_node_load_refuses_a_folder_left_partway() {
    use crate::models::fake_vllm::{self, Knobs};
    let home = temp("multi-partial");
    let folder = model_folder(&home, "org/m", true);
    std::fs::write(Path::new(&folder).join(".sparkbox.partial"), "{}").unwrap();
    let script = |n: u8| -> Script { Arc::new(move |cmd: &str, _: &[u8]| cmd.contains("sparkjob.py check").then(|| (check_reply(n), 0))) };
    let (a, p1, _s1) = fake_vllm::node_with(&home, Knobs::default(), script(1)).await;
    let (_b, p2, _s2) = fake_vllm::node_with(&temp("multi-partial-2"), Knobs::default(), script(2)).await;
    let c = cluster("multi-partial", &[("cell1", p1), ("cell2", p2)]).await;
    let spec = DeploySpec { id: "multi".into(), model_path: folder.clone(), image_ref: fake_vllm::GLM53_IMAGE.into(), ..spec(&["cell1", "cell2"], 2, 1) };
    let e = c.models.deploy(&c.ctx(), spec).await.unwrap_err();
    assert!(e.starts_with(&format!("cell1 doesn't have {folder} yet. Copy it to that node first (Library › Copy to nodes…).")), "{e}");
    assert!(a.ran("docker run").is_empty(), "nothing started");
    assert!(c.models.deployments(&c.ctx()).is_empty(), "nothing saved");
    c.live.stop();
}

#[test]
fn docker_run_says_what_it_can() {
    assert_eq!(docker_run_failed("cell2", "docker: Error response from daemon: no such image\n"), "cell2: docker run failed: docker: Error response from daemon: no such image");
    assert_eq!(docker_run_failed("cell2", " \n"), "cell2: docker run didn't finish: the connection dropped before it answered. Check Containers on cell2, then unload or try again.");
}

#[test]
fn dockers_reason_is_the_line_it_wrote_before_its_help_hint() {
    let hint = "\n\nRun 'docker run --help' for more information\n";
    let real = format!("docker: Error response from daemon: could not select device driver \"\" with capabilities: [gpu]{hint}");
    assert_eq!(docker_said(&real), "docker: Error response from daemon: could not select device driver \"\" with capabilities: [gpu]");
    let pulled = format!("Unable to find image 'x:y' locally\ndocker: Error response from daemon: pull access denied for x{hint}");
    assert_eq!(docker_said(&pulled), "docker: Error response from daemon: pull access denied for x");
    assert_eq!(docker_said("unknown flag: --nope\nSee 'docker run --help'.\n"), "unknown flag: --nope");
    assert_eq!(docker_said("some other last line\n"), "some other last line");
    assert_eq!(docker_said("Run 'docker run --help' for more information\n"), "Run 'docker run --help' for more information", "a hint is all there is");
    assert_eq!(docker_run_failed("cell3", ""), "cell3: docker run didn't finish: the connection dropped before it answered. Check Containers on cell3, then unload or try again.");
}

#[test]
fn a_ranks_state_reads_as_docker_writes_it() {
    assert_eq!(rank_state("running 0"), (true, None));
    assert_eq!(rank_state("restarting 1"), (true, None));
    assert_eq!(rank_state("created 0"), (true, None), "made, and not started yet");
    assert_eq!(rank_state("created 128"), (false, Some(128)), "a `docker run` that couldn't start it");
    assert_eq!(rank_state("exited 137"), (false, Some(137)));
    assert_eq!(rank_state("missing 0"), (false, None));
    assert_eq!(rank_state(""), (false, None));
    assert_eq!((restart_count("running 0 3"), restart_count("restarting 1 1"), restart_count("running 0"), restart_count("missing 0")), (3, 1, 0, 0));
}

#[test]
fn a_tilde_folder_matches_sparkjobs_absolute_dest() {
    assert!(same_folder("/home/sparkuser/models/org/m", "~/models/org/m"));
    assert!(same_folder("/data/models/org/m", "/data/models/org/m"));
    assert!(!same_folder("/home/sparkuser/models/org/m2", "~/models/org/m"));
    assert!(!same_folder("", "~/models/org/m"));
}

#[path = "release_exit_tests.rs"]
mod release_exit_tests;

use r4_load as tests_r4;
#[path = "run_tests.rs"]
mod run_tests;

#[tokio::test]
async fn version_preflight_reads_the_selected_image_and_drops_declarations_after_a_tag_moves() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use crate::models::fake_vllm::{self, Knobs};
    let moved = Arc::new(AtomicBool::new(false));
    let script = |n: u8| -> Script {
        let moved = moved.clone();
        Arc::new(move |cmd, _| {
            if !cmd.contains("sparkjob.py check") { return None; }
            let mut reply: Value = serde_json::from_str(&check_reply(n)).unwrap();
            reply["versions"] = json!({"driver": if n == 1 { "580.1" } else { "580.2" }, "nccl": "2.27.3", "imageId": if moved.load(Ordering::SeqCst) { format!("sha256:{}", "b".repeat(64)) } else { fake_vllm::test_image_id() }});
            Some((reply.to_string(), 0))
        })
    };
    let home = temp("version-preflight");
    let folder = model_folder(&home, "org/m", true);
    let (a, pa, _) = fake_vllm::node_with(&home, Knobs::default(), script(1)).await;
    let (b, pb, _) = fake_vllm::node_with(&temp("version-preflight-b"), Knobs::default(), script(2)).await;
    let c = cluster("version-preflight", &[("cell1", pa), ("cell2", pb)]).await;
    let spec = DeploySpec { model_path: folder, image_ref: fake_vllm::GLM53_IMAGE.into(), ..spec(&["cell1", "cell2"], 2, 1) };
    let settings = ModelSettings { image: "different:saved-default".into(), ..Default::default() };
    let first = c.models.preflight_set(&spec, &settings, None).await;
    let report = first.versions.unwrap();
    let row = |key: &str| report.rows.iter().find(|r| r.key == key).unwrap();
    assert_eq!(row("driver").state, crate::versions::State::Mismatched);
    assert_eq!(row("nccl").state, crate::versions::State::Matching);
    assert_eq!(row("imageId").state, crate::versions::State::Matching);
    for node in [&a, &b] {
        let reads = node.ran("sparkjob.py check");
        assert!(!reads.is_empty());
        assert!(reads.iter().all(|cmd| cmd.contains(fake_vllm::GLM53_IMAGE) && cmd.ends_with(" quick") && !cmd.contains("different:saved-default")), "{reads:?}");
        for mutation in ["docker run", "docker stop", "docker rm", "sudo "] { assert!(node.ran(mutation).is_empty()); }
    }
    moved.store(true, Ordering::SeqCst);
    let next = c.models.preflight_set(&spec, &settings, None).await;
    let report = next.versions.unwrap();
    assert_eq!(report.rows.iter().find(|r| r.key == "nccl").unwrap().state, crate::versions::State::Unknown);
    assert!(next.nodes.iter().all(|n| !n.versions.contains_key("nccl")));
    c.live.stop();
}
