//! A load's start, its pre-flight, its rollback and its restart on 1 to 4 fake GB10s: real SSH, the
//! real sparkjob.py and launch.py, the docker shim, and a stand-in whose ranks really wait for each
//! other (models/fake_vllm.rs). Multi-node here means that: no Spark has run any of it.

use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::admin::AdminGate;
use crate::fake_node::Node;
use crate::live::fence_class::FenceClass;
use crate::models::fake_vllm::{self, Knobs, Standin, CONTAINERD_STORE, GLM53_IMAGE};
use crate::tasks::{TaskState, Tasks};

const IDS: [&str; 4] = ["cell1", "cell2", "cell3", "cell4"];
const LAN: &str = "192.168.50.51";

struct Rank {
    node: Node,
    standin: Standin,
    /// What `sparkjob.py manifest` answers here, in place of the real listing.
    manifest: Arc<Mutex<Option<String>>>,
    /// What `sparkjob.py check` answers here, in place of both fabric halves.
    check: Arc<Mutex<Option<String>>>,
}

struct Rig {
    c: Cluster,
    ranks: Vec<Rank>,
    tasks: Arc<Tasks>,
    folder: String,
    gate: Arc<AdminGate>,
}

struct Yes;

impl crate::admin::Authenticator for Yes {
    fn authenticate(&self, _reason: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Gemma's config as the Hub has it, for what a split can take.
const GEMMA: &str = r#"{"architectures": ["Gemma3ForConditionalGeneration"], "text_config": {"num_attention_heads": 32, "num_key_value_heads": 16, "num_hidden_layers": 28, "hidden_size": 3840}}"#;

/// `a`'s command had answered before `b`'s was sent, by the app's clock. A row's start and its length
/// are each rounded to a millisecond, and measured apart: `ROUNDING` is what that leaves.
fn answered_before(a: &audit::Entry, b: &audit::Entry) -> bool {
    const ROUNDING: u64 = 5;
    a.ts_ms + a.ms <= b.ts_ms + ROUNDING
}

/// `n` fake GB10s, cell1 first, each with its own Knobs, all meeting in one rendezvous folder,
/// sharing gemma's folder at one path. SparkBox is connected, with a task list and a gate.
async fn rig(name: &str, knobs: Vec<Knobs>) -> Rig {
    let meet = temp(&format!("{name}-meet"));
    std::fs::create_dir_all(&meet).unwrap();
    let folder = temp(&format!("{name}-models")).join("google/gemma-3-27b-it");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("config.json"), GEMMA).unwrap();
    std::fs::write(folder.join("model.safetensors"), vec![7u8; 4096]).unwrap();
    let mut ranks = Vec::new();
    let mut ports = Vec::new();
    for (i, k) in knobs.into_iter().enumerate() {
        let n = i as u8 + 1;
        let manifest: Arc<Mutex<Option<String>>> = Arc::default();
        let listed = manifest.clone();
        let check: Arc<Mutex<Option<String>>> = Arc::default();
        let checked = check.clone();
        let script: Script = Arc::new(move |cmd, _| {
            if cmd.contains("sparkjob.py check") {
                return Some((checked.lock().unwrap().clone().unwrap_or_else(|| check_reply(n)), 0));
            }
            if cmd.contains("sparkjob.py manifest") {
                if let Some(m) = listed.lock().unwrap().clone() {
                    return Some((m, 0));
                }
            }
            super::tests_r4::ss(cmd)
        });
        let (node, port, standin) = fake_vllm::node_with(&temp(&format!("{name}-{n}")), Knobs { rendezvous: Some(meet.clone()), ..k }, script).await;
        ranks.push(Rank { node, standin, manifest, check });
        ports.push((IDS[i], port));
    }
    let c = cluster(name, &ports).await;
    let tasks = Tasks::new(None);
    c.models.set_tasks(tasks.clone());
    c.models.set_unload_wait(Duration::from_millis(100), Duration::from_secs(2));
    let gate = AdminGate::new(Box::new(crate::admin::SystemClock), Box::new(Yes), None);
    c.models.set_gate(gate.clone());
    c.live.set_write_policy(gate.clone());
    Rig { c, ranks, tasks, folder: folder.display().to_string(), gate }
}

async fn same(name: &str, n: usize, k: Knobs) -> Rig {
    rig(name, vec![k; n]).await
}

impl Rig {
    /// What the Load page sends for gemma on the first `n` nodes, on a free port.
    fn spec(&self, id: &str, n: usize, tp: u64, pp: u64) -> DeploySpec {
        self.on(id, &IDS[..n], tp, pp)
    }

    fn on(&self, id: &str, nodes: &[&str], tp: u64, pp: u64) -> DeploySpec {
        DeploySpec {
            id: id.into(),
            served_name: "gemma-3-27b-it".into(),
            model_path: self.folder.clone(),
            nodes: nodes.iter().map(|n| n.to_string()).collect(),
            tensor_parallel: tp,
            pipeline_parallel: pp,
            max_model_len: Some(32768),
            port: fake_vllm::free_port(),
            image_ref: GLM53_IMAGE.into(),
            override_memory: true,
            ..Default::default()
        }
    }

    async fn deploy(&self, spec: &DeploySpec) -> Deployment {
        self.c.models.deploy(&self.c.ctx(), spec.clone()).await.unwrap()
    }

    async fn ready(&self, spec: &DeploySpec) -> Deployment {
        self.deploy(spec).await;
        self.wait(&spec.id, "ready").await
    }

    fn load(&self, id: &str) -> Deployment {
        self.c.models.deployments(&self.c.ctx()).into_iter().find(|d| d.spec.id == id).unwrap()
    }

    /// Waits until the load reads `want`, for as long as a test machine under load may take.
    async fn wait(&self, id: &str, want: &str) -> Deployment {
        self.until(id, want, |d| d.state == want).await
    }

    /// Waits until `f` holds of the load.
    async fn until(&self, id: &str, what: &str, f: impl Fn(&Deployment) -> bool) -> Deployment {
        for _ in 0..1200 {
            let d = self.load(id);
            if f(&d) {
                return d;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{id}: {what}: {:?}", self.load(id));
    }

    /// Restarts it by hand and waits for the task, whose Verify step reads the endpoint rounds the
    /// app runs by itself.
    async fn restarted(&self, id: &str) -> crate::tasks::Task {
        self.restarted_with_consent(id, false).await
    }

    async fn restarted_with_consent(&self, id: &str, drop_caches_once: bool) -> crate::tasks::Task {
        let task = self.c.models.restart_with_cache_consent(&self.c.ctx(), id, drop_caches_once).await.unwrap();
        for _ in 0..1200 {
            self.c.models.endpoints_tick(&self.c.ctx(), true).await;
            if let Some(t) = self.tasks.get(&task).filter(|t| t.finished()) {
                return t;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{id}'s restart never finished: {:?}", self.tasks.get(&task));
    }

    /// Unloads it and waits for the task.
    async fn unload(&self, id: &str) -> crate::tasks::Task {
        finished(&self.tasks, &self.c.models.unload(&self.c.ctx(), id).await.unwrap()).await
    }

    /// Every container and every process of every node is gone, and nothing of `id`'s on disk.
    async fn nothing_left(&self, id: &str) {
        for (i, r) in self.ranks.iter().enumerate() {
            let listed = self.c.live.run(IDS[i], "docker ps -aq --filter label=sparkbox.deployment", None, Duration::from_secs(20)).await.unwrap();
            assert_eq!((IDS[i], listed.stdout.trim()), (IDS[i], ""), "no container left");
            assert!(r.standin.running().is_empty(), "{}: {:?}", IDS[i], r.standin.running());
            assert!(r.standin.shm().is_empty(), "{}: /dev/shm: {:?}", IDS[i], r.standin.shm());
            let home = &r.node.home;
            assert!(!home.join(".sparkbox/keys").join(id).exists() && !home.join(".sparkbox/deploy").join(id).exists(), "{}: its files", IDS[i]);
        }
    }

    fn ran(&self, rank: usize, what: &str) -> Vec<String> {
        self.ranks[rank].node.ran(what)
    }

    /// Look-only, the way Settings turns it on.
    fn look_only(&self, on: bool) {
        self.gate.set_look_only(on).unwrap();
    }

    /// Everything a node saw and every row SparkBox wrote: the key is in none of it.
    fn no_key_anywhere(&self, key: &str) {
        for (i, r) in self.ranks.iter().enumerate() {
            assert!(!r.node.raw.lock().unwrap().iter().any(|l| l.contains(key)), "{}: a command carried the key", IDS[i]);
            for name in r.standin.names() {
                assert!(!r.standin.argv(&name).iter().any(|w| w.contains(key)), "{name}'s argv");
                assert!(!r.standin.log(&name).contains(key), "{name}'s log");
                assert!(!r.standin.seen(&name).to_string().contains(key), "{name}'s environment as recorded");
            }
        }
        for e in self.c.live.audit.entries() {
            let row = serde_json::to_string(&e).unwrap();
            assert!(!row.contains(key), "the activity log: {row}");
        }
        let log = std::fs::read_to_string(self.c.dir.join("ssh.log")).unwrap_or_default();
        assert!(!log.contains(key), "ssh.log");
    }
}

fn files_of(r: &Rank, id: &str) -> (Value, Value) {
    let dir = r.node.home.join(".sparkbox/deploy").join(id);
    let read = |f: &str| serde_json::from_str(&std::fs::read_to_string(dir.join(f)).unwrap()).unwrap();
    (read("argv.json"), read("env.json"))
}

// ---- The start

#[tokio::test]
async fn two_nodes_start_workers_first_and_head_last() {
    let r = same("r1-order", 2, Knobs { warmup_s: 0.3, ..Knobs::default() }).await;
    let spec = r.spec("r1-order", 2, 2, 1);
    let d = r.ready(&spec).await;
    let (head, worker) = (&r.ranks[0], &r.ranks[1]);

    // By the shim's own clock: the head's process came after the worker's existed.
    let head_at = head.standin.started_ms("sparkbox-r1-order-r0").expect("the head ran");
    let worker_at = worker.standin.started_ms("sparkbox-r1-order-r1").expect("the worker ran");
    assert!(worker_at < head_at, "worker {worker_at}, head {head_at}");
    assert_eq!((r.ran(0, "docker run -d").len(), r.ran(1, "docker run -d").len()), (1, 1));
    assert_eq!(head.standin.running(), ["sparkbox-r1-order-r0"]);
    assert_eq!(worker.standin.running(), ["sparkbox-r1-order-r1"]);

    // Exactly C-L2, the image by its id, no shell, no Ray; launch.py and the files came first.
    assert_eq!(r.ran(0, "docker run -d"), [plan::rank_command(&d.spec, 0)]);
    assert_eq!(r.ran(1, "docker run -d"), [plan::rank_command(&d.spec, 1)]);
    assert_eq!(d.spec.image_id, fake_vllm::test_image_id());
    for rank in &r.ranks {
        assert_eq!(std::fs::read_to_string(rank.node.home.join(format!(".sparkbox/{}", plan::launcher_name()))).unwrap(), plan::LAUNCH);
    }
    assert!(r.ran(1, "docker build").is_empty() && r.ran(1, "pip install").is_empty());
    assert_eq!(d.spec.docker_access, "privileged", "the recipe's first-run choice");

    // The worker has no key at all: no file, no mount, no flag.
    let worker_argv = worker.standin.argv("sparkbox-r1-order-r1");
    assert_eq!(worker_argv.last().unwrap(), "/run/sparkbox/launch.py", "{worker_argv:?}");
    assert!(!worker_argv.iter().any(|w| w.contains("/run/sparkbox/key")), "{worker_argv:?}");
    assert!(!worker.node.home.join(".sparkbox/keys/r1-order").exists());
    assert_eq!(worker.standin.seen("sparkbox-r1-order-r1")["hasKey"].as_bool(), Some(false));
    let key = secrets::get(&plan::key_account("r1-order")).unwrap().expect("its key");
    assert_eq!(std::fs::read_to_string(head.node.home.join(".sparkbox/keys/r1-order")).unwrap(), key);
    assert_eq!(std::fs::metadata(head.node.home.join(".sparkbox/keys/r1-order")).unwrap().permissions().mode() & 0o777, 0o400);
    assert_eq!(head.standin.seen("sparkbox-r1-order-r0")["hasKey"].as_bool(), Some(true));

    // Each rank's argv and env are in the activity log; the key in none of it.
    let rows = r.c.live.audit.entries();
    for title in ["Launch files for the head (rank 0) on cell1", "Launch files for the worker (rank 1) on cell2"] {
        let row = rows.iter().find(|e| e.title == title).unwrap_or_else(|| panic!("no row {title}"));
        assert!(row.command.contains("\"--nnodes\",\"2\"") && row.command.contains("NCCL_IB_HCA"), "{}", row.command);
    }
    r.no_key_anywhere(&key);

    // Ready means it answered, and it's in Chat.
    assert!(r.c.llm.get("vllm-r1-order").await.is_some());
    assert_eq!(d.readiness.as_ref().map(|x| x.ok), Some(true));
    let t = r.unload("r1-order").await;
    assert_eq!(t.state, TaskState::Ok, "{t:?}");
    r.nothing_left("r1-order").await;
    r.c.live.stop();
}

#[tokio::test]
async fn a_two_node_load_reads_ready_only_after_every_rank_joins() {
    let r = rig("r1-join", vec![Knobs::default(), Knobs { join_held: true, ..Knobs::default() }]).await;
    let spec = r.spec("r1-join", 2, 2, 1);
    r.deploy(&spec).await;
    // Both containers run; the head has no one to meet, so it can't answer.
    let d = r.until("r1-join", "both ranks counted", |d| d.stage == Some(Stage::Joining { have: 2, need: 2 })).await;
    assert_eq!(d.state, DeploymentState::Starting);
    assert_eq!(d.message, "2 of 2 nodes up");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let d = r.load("r1-join");
    assert_eq!(d.state, DeploymentState::Starting, "{}", d.message);
    assert!(r.c.llm.get("vllm-r1-join").await.is_none(), "not in Chat yet");
    assert!(!r.ranks[0].standin.log("sparkbox-r1-join-r0").contains("Loading model from scratch"));

    r.ranks[1].standin.let_join();
    let d = r.wait("r1-join", "ready").await;
    assert_eq!(d.stage, Some(Stage::Ready));
    assert!(r.ranks[0].standin.log("sparkbox-r1-join-r0").contains("world_size=2"));
    r.unload("r1-join").await;
    r.c.live.stop();
}

#[tokio::test]
async fn four_nodes_tp4_and_three_nodes_pp3_build_the_right_ranks() {
    let r = same("r1-ranks", 4, Knobs::default()).await;
    let check = |id: &str, n: usize, tp: u64, pp: u64| {
        let name = |rank: usize| plan::rank_container(id, rank, n);
        for rank in 0..n {
            let argv = r.ranks[rank].standin.argv(&name(rank));
            let (files, env) = files_of(&r.ranks[rank], id);
            let words: Vec<&str> = files.as_array().unwrap().iter().filter_map(Value::as_str).collect();
            let has = |pair: [&str; 2]| words.windows(2).any(|w| w == pair);
            assert_eq!(words[..2], ["vllm", "serve"]);
            assert!(has(["--nnodes", &n.to_string()]) && has(["--node-rank", &rank.to_string()]), "{words:?}");
            assert!(has(["--master-addr", "192.168.100.1"]) && has(["--master-port", "29501"]), "{words:?}");
            assert!(has(["--tensor-parallel-size", &tp.to_string()]), "{words:?}");
            assert_eq!(words.contains(&"--pipeline-parallel-size"), pp > 1, "{words:?}");
            assert_eq!(words.contains(&"--headless"), rank > 0, "{words:?}");
            assert_eq!(words.contains(&"--port"), rank == 0, "{words:?}");
            // Every value from the rank's own node.
            let ip = format!("192.168.100.{}", rank + 1);
            assert_eq!(env["VLLM_HOST_IP"].as_str(), Some(ip.as_str()), "{env}");
            assert_eq!(env["NCCL_IB_HCA"].as_str(), Some("rocep1s0f1,roceP2p1s0f1"), "{env}");
            assert_eq!(env["NCCL_SOCKET_IFNAME"].as_str(), Some("enp1s0f1np1"), "{env}");
            assert!(env.get("MASTER_ADDR").is_none() && env.as_object().unwrap().keys().all(|k| !k.starts_with("RAY_")), "{env}");
            let role = if rank == 0 { "head" } else { "worker" };
            assert!(argv.contains(&format!("sparkbox.role={role}")) && argv.contains(&format!("sparkbox.rank={rank}")) && argv.contains(&format!("sparkbox.parallel={tp}x{pp}")), "{argv:?}");
            assert!(argv.contains(&"--privileged".to_string()), "{argv:?}");
            let seen = r.ranks[rank].standin.seen(&name(rank));
            assert_eq!(seen["values"]["VLLM_HOST_IP"].as_str(), Some(ip.as_str()), "what the process really got: {seen}");
        }
    };
    r.ready(&r.spec("r1-tp4", 4, 4, 1)).await;
    check("r1-tp4", 4, 4, 1);
    r.unload("r1-tp4").await;

    // 3 nodes: TP=3 can't divide gemma's 32 heads, in the words the Load page shows.
    let e = r.c.models.deploy(&r.c.ctx(), r.spec("r1-tp3", 3, 3, 1)).await.unwrap_err();
    assert!(e.starts_with("Tensor parallel 3 won't work for this model: 32 heads don't divide by 3."), "{e}");
    assert!(r.ran(2, "docker run -d").len() == 1, "only the TP=4 run");

    r.ready(&r.spec("r1-pp3", 3, 1, 3)).await;
    check("r1-pp3", 3, 1, 3);
    r.unload("r1-pp3").await;
    assert!(r.ranks[3].standin.running().is_empty());

    // The offers the page gets, by the same rules.
    let p = r.c.models.plan(&r.c.ctx(), &r.spec("r1-offers", 3, 1, 3)).await.unwrap();
    let offers: Vec<(&str, bool)> = p.topologies.iter().map(|o| (o.id.as_str(), o.ok)).collect();
    assert_eq!(offers, [("tp3", false), ("pp3", true), ("replicas3", true)], "{:?}", p.topologies);
    assert_eq!(p.recipe.as_ref().map(|r| r.id.as_str()), Some("gemma-3-27b-it"));
    r.c.live.stop();
}

// ---- What a failed start leaves

#[tokio::test]
async fn a_worker_that_fails_docker_run_starts_no_head_and_rolls_back() {
    let bad = Knobs { run_fails: Some("Error response from daemon: could not select device driver \"\" with capabilities: [gpu]".into()), ..Knobs::default() };
    let r = rig("r1-noworker", vec![Knobs::default(), Knobs::default(), bad]).await;
    let spec = r.spec("r1-noworker", 3, 1, 3);
    r.deploy(&spec).await;
    let d = r.wait("r1-noworker", "failed").await;
    // Docker's reason, not the help hint that ends its stderr.
    let reason = "docker: Error response from daemon: could not select device driver \"\" with capabilities: [gpu]";
    assert_eq!(d.message, format!("cell3: docker run failed: {reason}"));
    assert!(r.ran(0, "docker run -d").is_empty(), "the head never started");
    assert!(r.ranks[0].standin.names().is_empty());
    let f = d.failure.expect("a failure record");
    assert_eq!(f.left_on, Vec::<String>::new());
    assert_eq!(f.cause, d.message);
    // All of what it wrote, in the rank's record and its saved log: its container has no log.
    let end = f.ranks.iter().find(|x| x.rank == 2).unwrap();
    assert_eq!(end.exit, Some(128), "Created, with the exit code Docker gave it");
    assert!(end.tail.contains(reason) && end.tail.contains("Run 'docker run --help' for more information"), "{}", end.tail);
    assert!(d.log_tail.contains("── cell3, worker (rank 2) ──") && d.log_tail.contains(reason), "{}", d.log_tail);
    let saved = std::fs::read_to_string(PathBuf::from(&f.log_dir).join("r2.log")).unwrap();
    assert!(saved.contains(reason), "{saved}");
    // The Created container it left went with the rollback, as it did not with the run.
    assert!(r.ranks[2].standin.names().contains(&"sparkbox-r1-noworker-r2".to_string()));
    r.nothing_left("r1-noworker").await;
    assert_eq!(r.ran(1, "ids=$(docker ps -aq").len(), 1, "the other worker, which did start, was unloaded");
    assert!(secrets::get(&plan::key_account("r1-noworker")).unwrap().is_none(), "a first start's key goes with it");
    assert!(r.c.llm.get("vllm-r1-noworker").await.is_none());
    r.c.live.stop();
}

/// A `docker run` refused before it made anything: no container to read, so what it wrote is all
/// there is, and the worker is still the rank the load's tail shows.
#[tokio::test]
async fn a_docker_run_that_makes_no_container_keeps_what_docker_said() {
    let r = same("r1-nomade", 2, Knobs::default()).await;
    let worker = &r.ranks[1];
    let shim = std::fs::read_to_string(worker.node.bin_dir.join("docker")).unwrap();
    std::fs::write(worker.node.bin_dir.join("docker-real"), &shim).unwrap();
    std::fs::set_permissions(worker.node.bin_dir.join("docker-real"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let reason = "docker: Error response from daemon: invalid mount config for type \"bind\"";
    worker.node.bin("docker", &format!("[ \"$1\" = run ] && {{ printf '%s\\n\\nRun '\\''docker run --help'\\'' for more information\\n' '{reason}' >&2; exit 125; }}\nexec docker-real \"$@\"\n"));
    r.deploy(&r.spec("r1-nomade", 2, 2, 1)).await;
    let d = r.wait("r1-nomade", "failed").await;
    assert_eq!(d.message, format!("cell2: docker run failed: {reason}"));
    let f = d.failure.clone().unwrap();
    let end = &f.ranks[1];
    assert_eq!(end.exit, None, "it never existed");
    assert!(end.tail.starts_with("docker run said:\n") && end.tail.contains(reason) && end.tail.ends_with("This rank's container was never made."), "{}", end.tail);
    assert!(d.log_tail.contains("── cell2, worker (rank 1) ──") && d.log_tail.contains(reason), "{}", d.log_tail);
    r.c.live.stop();
}

/// Session 2's step 9.
#[tokio::test]
async fn a_worker_that_dies_while_starting_fails_the_load_with_its_logs_and_leaves_nothing() {
    let dying = Knobs { rank_dies_after_s: Some(1.0), ..Knobs::default() };
    let r = rig("r1-dies", vec![Knobs { warmup_s: 300.0, ..Knobs::default() }, dying]).await;
    let spec = r.spec("r1-dies", 2, 2, 1);
    r.deploy(&spec).await;
    let key = secrets::get(&plan::key_account("r1-dies")).unwrap().expect("its key");
    let d = r.wait("r1-dies", "failed").await;
    assert!(d.message.contains("The worker (rank 1) on cell2 stopped while the load was starting (exit 1)."), "{}", d.message);
    r.nothing_left("r1-dies").await;

    let f = d.failure.clone().expect("a failure record");
    assert_eq!(f.left_on, Vec::<String>::new());
    assert_eq!(f.ranks.iter().map(|x| (x.node.as_str(), x.rank, x.role.as_str())).collect::<Vec<_>>(), [("cell1", 0, "head"), ("cell2", 1, "worker")]);
    assert_eq!(f.ranks[1].exit, Some(1));
    assert!(f.ranks[1].tail.contains("WorkerProc failed to start."), "{}", f.ranks[1].tail);
    assert!(d.log_tail.contains("── cell2, worker (rank 1) ──") && d.log_tail.contains("── cell1, head (rank 0) ──"), "{}", d.log_tail);

    // Saved on this Mac before anything was removed, the owner's alone, and with no key in them.
    let dir = PathBuf::from(&f.log_dir);
    assert!(dir.starts_with(r.c.dir.join("failed-loads")), "{dir:?}");
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    for rank in 0..2 {
        let file = dir.join(format!("r{rank}.log"));
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        let log = std::fs::read_to_string(&file).unwrap();
        assert!(log.contains("Loading") || log.contains("vLLM API server"), "{log}");
        assert!(!log.contains(&key));
    }
    assert!(std::fs::read_to_string(dir.join("r1.log")).unwrap().contains("rank 1 failed while starting"));
    r.no_key_anywhere(&key);
    assert!(secrets::get(&plan::key_account("r1-dies")).unwrap().is_none());
    // The rollback is its own operation, with the unload under model-unload.
    let rows = r.c.live.audit.entries();
    let rows: Vec<_> = rows.iter().filter(|e| e.title.starts_with("Clean up after gemma-3-27b-it")).collect();
    let unloads: Vec<_> = rows.iter().filter(|e| e.command.starts_with("ids=$(docker ps -aq")).collect();
    assert_eq!(unloads.len(), 2, "{rows:?}");
    // The head's first: the worker's was sent once it had answered.
    assert_eq!(unloads.iter().rev().map(|e| e.node.as_str()).collect::<Vec<_>>(), ["cell1", "cell2"], "{unloads:?}");
    assert!(answered_before(unloads[1], unloads[0]), "{unloads:?}");
    r.c.live.stop();
}

/// Keep running on one node has `--restart unless-stopped` from the first `docker run`, so Docker
/// brings a crash straight back and a look reads `running`: only RestartCount says it stopped.
#[tokio::test]
async fn a_first_start_that_stops_is_not_retried() {
    let r = same("r1-noretry", 1, Knobs { warmup_s: 300.0, rank_dies_after_s: Some(0.3), ..Knobs::default() }).await;
    let spec = DeploySpec { keep_running: true, ..r.spec("r1-noretry", 1, 1, 1) };
    r.deploy(&spec).await;
    let d = r.wait("r1-noretry", "failed").await;
    assert!(r.ranks[0].standin.restarts("sparkbox-r1-noretry") >= 1, "the shim's policy really started it again");
    assert!(d.message.starts_with("vLLM stopped on cell1 while it was starting, and Docker started it again (") && d.message.contains("A start that stops isn't retried. Last log lines:\n"), "{}", d.message);
    assert!(!d.recoverable, "it never ran");
    let f = d.failure.clone().expect("a failure record");
    assert!(f.left_on.is_empty(), "{f:?}");
    // The saved log, not the message: its last lines can be the next run's by the time it's read.
    let saved = std::fs::read_to_string(PathBuf::from(&f.log_dir).join("r0.log")).unwrap();
    assert!(saved.contains("rank 0 failed while starting"), "{saved}");
    assert!(secrets::get(&plan::key_account("r1-noretry")).unwrap().is_none());
    r.nothing_left("r1-noretry").await;
    for _ in 0..6 {
        r.c.models.endpoints_tick(&r.c.ctx(), true).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(r.ran(0, "docker run -d").len(), 1, "nothing tried it again");
    assert_eq!(r.load("r1-noretry").state, DeploymentState::Failed);
    assert!(r.ranks[0].standin.running().is_empty(), "and Docker's policy ended with its container");
    r.c.live.stop();
}

/// The same load, found after a quit: `watch_only` looks at the count too.
#[tokio::test]
async fn a_load_docker_keeps_restarting_is_stopped_after_a_quit() {
    let r = same("r1-loop", 1, Knobs { warmup_s: 300.0, rank_dies_after_s: Some(0.3), ..Knobs::default() }).await;
    let spec = DeploySpec { keep_running: true, require_key: false, image_id: fake_vllm::test_image_id(), ..r.spec("r1-loop", 1, 1, 1) };
    fake_vllm::launch_files(&r.c.live, "cell1", &spec, 0, &[], None).await;
    assert_eq!(r.c.live.run("cell1", &plan::rank_command(&spec, 0), None, Duration::from_secs(30)).await.unwrap().code, Some(0));
    let saved = Deployment { spec, state: DeploymentState::Starting, first_start: true, started_at: now(), ..Default::default() };
    r.c.models.deployments.lock().unwrap().push(saved);

    r.c.models.resume(&ModelSettings::default());
    let d = r.wait("r1-loop", "failed").await;
    assert!(d.message.starts_with("vLLM stopped on cell1 while it was starting, and Docker started it again ("), "{}", d.message);
    r.nothing_left("r1-loop").await;
    r.c.live.stop();
}

#[tokio::test]
async fn look_only_holds_a_rollback_and_says_unload_removes_them() {
    let r = rig("r1-held", vec![Knobs { warmup_s: 300.0, ..Knobs::default() }, Knobs { join_held: true, ..Knobs::default() }]).await;
    let spec = r.spec("r1-held", 2, 2, 1);
    r.deploy(&spec).await;
    r.until("r1-held", "both ranks running", |d| matches!(d.stage, Some(Stage::Joining { have: 2, .. }))).await;
    // Look-only comes on, then the worker dies: reads go on, the clean-up is refused.
    r.look_only(true);
    r.ranks[1].standin.kill("sparkbox-r1-held-r1");
    let d = r.wait("r1-held", "failed").await;
    assert!(d.message.contains("The worker (rank 1) on cell2 stopped while the load was starting (exit 137)."), "{}", d.message);
    assert!(d.message.ends_with("Its containers and files are still on cell1, cell2; Unload or Forget removes them."), "{}", d.message);
    let f = d.failure.clone().unwrap();
    assert_eq!(f.left_on, ["cell1", "cell2"]);
    assert!(std::fs::read_to_string(PathBuf::from(&f.log_dir).join("r1.log")).is_ok(), "its logs were read all the same");
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-held-r0"], "still there");
    assert!(r.ran(0, "ids=$(docker ps -aq").is_empty(), "nothing was sent");

    // Nothing retries while it holds; once it ends, Unload removes them.
    r.look_only(false);
    let t = r.unload("r1-held").await;
    assert_eq!(t.state, TaskState::Ok, "{t:?}");
    assert_eq!(r.load("r1-held").failure.unwrap().left_on, Vec::<String>::new());
    assert_eq!(r.ran(0, "docker run -d").len(), 1);
    r.c.live.stop();
}

#[tokio::test]
async fn a_missing_key_file_fails_the_load_with_launchers_line() {
    let r = same("r1-nokey", 1, Knobs { key_file_goes: true, ..Knobs::default() }).await;
    r.deploy(&r.spec("r1-nokey", 1, 1, 1)).await;
    let d = r.wait("r1-nokey", "failed").await;
    assert_eq!(d.message, "cell1 (exit 78): the API key file is missing or empty, so vLLM was not started.");
    let f = d.failure.unwrap();
    assert_eq!(f.ranks[0].exit, Some(plan::KEY_REFUSED));
    assert!(f.ranks[0].tail.contains("sparkbox: the API key file is missing or empty"), "{}", f.ranks[0].tail);
    r.nothing_left("r1-nokey").await;
    r.c.live.stop();
}

/// The mutant the plan asks for, as a fake: `--key` never reaches launch.py, so vLLM has no key.
/// It must fail at readiness, not serve open.
#[tokio::test]
async fn a_load_that_serves_without_its_key_fails_at_readiness() {
    let r = same("r1-open", 1, Knobs { key_flag_dropped: true, ..Knobs::default() }).await;
    r.deploy(&r.spec("r1-open", 1, 1, 1)).await;
    let d = r.wait("r1-open", "failed").await;
    assert_eq!(r.ranks[0].standin.seen("sparkbox-r1-open")["hasKey"].as_bool(), Some(false));
    assert!(d.message.starts_with("vLLM answered without its API key, so it was stopped rather than left open."), "{}", d.message);
    assert!(!d.recoverable);
    assert!(r.c.llm.get("vllm-r1-open").await.is_none(), "never offered in Chat");
    r.nothing_left("r1-open").await;
    r.c.live.stop();
}

// ---- Claims

#[tokio::test]
async fn two_loads_on_disjoint_nodes_run_together_and_unload_alone() {
    let r = same("r1-two", 4, Knobs::default()).await;
    let a = r.on("r1-a", &["cell1", "cell2"], 2, 1);
    let b = r.on("r1-b", &["cell3", "cell4"], 2, 1);
    let (da, db) = (r.ready(&a).await, r.ready(&b).await);
    assert_eq!((da.state, db.state), (DeploymentState::Ready, DeploymentState::Ready));

    // A node either holds is taken.
    let e = r.c.models.deploy(&r.c.ctx(), r.on("r1-c", &["cell2"], 1, 1)).await.unwrap_err();
    assert_eq!(e, "cell2 is already serving another model; unload it first.");

    // Unloading one leaves the other's containers alone.
    let t = r.unload("r1-a").await;
    assert_eq!(t.state, TaskState::Ok, "{t:?}");
    assert!(r.ranks[0].standin.running().is_empty() && r.ranks[1].standin.running().is_empty());
    assert_eq!(r.ranks[2].standin.running(), ["sparkbox-r1-b-r0"]);
    assert_eq!(r.ranks[3].standin.running(), ["sparkbox-r1-b-r1"]);
    assert_eq!(r.load("r1-b").state, DeploymentState::Ready);
    assert!(r.ran(2, "ids=$(docker ps -aq").is_empty(), "nothing was sent to the other load's nodes");
    fake_vllm_health(&r, "cell3", b.port).await;
    r.unload("r1-b").await;
    r.c.live.stop();
}

async fn fake_vllm_health(r: &Rig, node: &str, port: u16) {
    let asked = format!("curl -s -o /dev/null -m 3 -w '%{{http_code}}' http://127.0.0.1:{port}/health || true");
    assert_eq!(r.c.live.run(node, &asked, None, Duration::from_secs(10)).await.unwrap().stdout.trim(), "200");
}

#[tokio::test]
async fn two_deploys_racing_for_one_node_get_it_once() {
    let r = same("r1-race", 2, Knobs::default()).await;
    let (a, b) = (r.on("r1-race-a", &["cell1"], 1, 1), r.on("r1-race-b", &["cell1", "cell2"], 2, 1));
    let ctx = r.c.ctx();
    let (ra, rb) = futures_util::future::join(r.c.models.deploy(&ctx, a), r.c.models.deploy(&ctx, b)).await;
    assert!(ra.is_ok() != rb.is_ok(), "exactly one gets it: {:?} {:?}", ra.as_ref().map(|d| &d.spec.id), rb.as_ref().map(|d| &d.spec.id));
    let lost = ra.as_ref().err().or(rb.as_ref().err()).unwrap();
    assert!(lost.contains("is already serving another model"), "{lost}");
    let won = ra.ok().or(rb.ok()).unwrap();
    r.wait(&won.spec.id, "ready").await;
    assert_eq!(r.ran(0, "docker run -d").len(), 1, "one container on the node they raced for");
    r.unload(&won.spec.id).await;
    r.c.live.stop();
}

#[tokio::test]
async fn ollama_resident_refuses_with_the_reason() {
    let resident = Knobs { ollama_ps: Some(r#"{"models":[{"name":"gemma4:latest","model":"gemma4:latest","size":9556302438}]}"#.into()), ..Knobs::default() };
    let r = rig("r1-ollama", vec![Knobs::default(), resident]).await;
    let spec = r.spec("r1-ollama", 2, 2, 1);
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    let why = "cell2 has gemma4:latest in memory in Ollama, and unified memory is shared. Unload it there first.";
    assert_eq!((set.verdict.as_str(), set.problems.as_slice()), ("blocked", &[why.to_string()][..]));
    assert_eq!(set.nodes[1].ollama_resident, ["gemma4:latest"]);
    assert_eq!(r.c.models.deploy(&r.c.ctx(), spec).await.unwrap_err(), why);
    assert!(r.ran(0, "deploy-files").is_empty() && r.c.models.deployments(&r.c.ctx()).is_empty());
    r.c.live.stop();
}

/// A container of `id` on the node, as a load that's gone left it.
async fn left_behind(r: &Rig, rank: usize, spec: &DeploySpec, instance: &str) {
    let spec = DeploySpec { image_id: fake_vllm::test_image_id(), require_key: false, ..spec.clone() };
    fake_vllm::launch_files(&r.c.live, IDS[rank], &spec, 0, &[], None).await;
    let out = r.c.live.run(IDS[rank], &plan::rank_command_for(&spec, 0, instance), None, Duration::from_secs(30)).await.unwrap();
    assert_eq!(out.code, Some(0), "{}", out.stderr);
}

#[tokio::test]
async fn leftovers_block_until_removed() {
    let r = same("r1-left", 1, Knobs { load_bytes: 0, ..Knobs::default() }).await;
    let spec = r.spec("r1-left", 1, 1, 1);
    let old = r.spec("r1-old", 1, 1, 1);
    left_behind(&r, 0, &old, crate::identity::instance_name()).await;

    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    let why = "cell1 has containers left from an earlier load (sparkbox-r1-old). Remove them first.";
    assert_eq!((set.verdict.as_str(), set.problems.as_slice()), ("blocked", &[why.to_string()][..]));
    assert_eq!(set.nodes[0].leftovers.iter().map(|l| (l.name.as_str(), l.deployment.as_str(), l.ours)).collect::<Vec<_>>(), [("sparkbox-r1-old", "r1-old", true)]);
    assert!(r.c.models.deploy(&r.c.ctx(), spec.clone()).await.unwrap_err().contains(why));
    assert_eq!(r.ran(0, "docker run -d").len(), 1, "only the old one");

    let task = r.c.models.clean_leftovers(&r.c.ctx(), vec!["cell1".into()], vec!["r1-old".into()]).await.unwrap();
    let t = finished(&r.tasks, &task).await;
    assert_eq!((t.state, t.title.as_str()), (TaskState::Ok, "Remove what earlier loads left on cell1"), "{t:?}");
    assert!(r.ranks[0].standin.running().is_empty());
    assert!(!r.ranks[0].node.home.join(".sparkbox/deploy/r1-old").exists());
    assert_eq!(r.ran(0, &plan::unload_command("r1-old")).len(), 1, "C-U2, by id");
    assert_eq!(r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap().verdict, "ready");
    r.ready(&spec).await;

    // A load that's serving isn't a leftover, on the page or to remove.
    let set = r.c.models.preflight(&r.c.ctx(), &r.spec("r1-next", 1, 1, 1)).await.unwrap();
    assert!(set.nodes[0].leftovers.is_empty(), "{:?}", set.nodes[0].leftovers);
    let e = r.c.models.clean_leftovers(&r.c.ctx(), vec!["cell1".into()], vec!["r1-left".into()]).await.unwrap_err();
    assert_eq!(e, "gemma-3-27b-it is loading or serving: unload it instead.");
    // C-U2's rm -rf takes this id, so `..` is refused before any command goes out.
    for bad in ["..", ".", "-rf", "a/b", ""] {
        let e = r.c.models.clean_leftovers(&r.c.ctx(), vec!["cell1".into()], vec![bad.into()]).await.unwrap_err();
        assert_eq!(e, format!("{bad} isn't a load's id."));
    }
    r.unload("r1-left").await;
    r.c.live.stop();
}

#[tokio::test]
async fn the_other_builds_leftover_is_never_removed() {
    let r = same("r1-other", 1, Knobs { load_bytes: 0, ..Knobs::default() }).await;
    let other = r.spec("r1-theirs", 1, 1, 1);
    left_behind(&r, 0, &other, "release").await;
    assert_ne!(crate::identity::instance_name(), "release", "a test build is dev");

    // Reported, not this load's to remove, and no reason not to load beside it.
    let spec = r.spec("r1-mine", 1, 1, 1);
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!((set.verdict.as_str(), set.problems.len()), ("ready", 0), "{:?}", set.problems);
    assert_eq!(set.nodes[0].leftovers.iter().map(|l| (l.name.as_str(), l.instance.as_str(), l.ours)).collect::<Vec<_>>(), [("sparkbox-r1-theirs", "release", false)]);

    // Told to remove it by id: refused before anything goes, as an unload refuses it. C-U2's docker
    // part names this build, its `rm -rf` of the launch files and key doesn't, and they're shared.
    let home = r.ranks[0].node.home.clone();
    std::fs::create_dir_all(home.join(".sparkbox/keys")).unwrap();
    std::fs::write(home.join(".sparkbox/keys/r1-theirs"), "sbk-theirs").unwrap();
    let e = r.c.models.clean_leftovers(&r.c.ctx(), vec!["cell1".into()], vec!["r1-theirs".into()]).await.unwrap_err();
    assert_eq!(e, "r1-theirs's containers on cell1 are managed by the other SparkBox; nothing was stopped.");
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-theirs"]);
    assert!(home.join(".sparkbox/deploy/r1-theirs/argv.json").exists() && home.join(".sparkbox/keys/r1-theirs").exists(), "its files stay too");
    assert!(r.ran(0, "ids=$(docker ps -aq").is_empty(), "nothing was sent");
    r.c.live.stop();
}

#[tokio::test]
async fn an_id_nothing_of_this_build_holds_is_refused() {
    let r = same("r1-noid", 1, Knobs::default()).await;
    let home = r.ranks[0].node.home.clone();
    std::fs::create_dir_all(home.join(".sparkbox/keys")).unwrap();
    std::fs::write(home.join(".sparkbox/keys/r1-idle"), "sbk-theirs").unwrap();
    let e = r.c.models.clean_leftovers(&r.c.ctx(), vec!["cell1".into()], vec!["r1-idle".into()]).await.unwrap_err();
    assert_eq!(e, "Nothing of r1-idle from this SparkBox is on cell1; nothing was stopped.");
    assert!(home.join(".sparkbox/keys/r1-idle").exists(), "the other build's key, with no container of it left");
    r.c.live.stop();
}

/// A node that can't be asked says nothing about whose it is.
#[tokio::test]
async fn a_node_that_cant_be_asked_stops_a_clean_up() {
    let r = same("r1-unasked", 1, Knobs::default()).await;
    r.ranks[0].node.bin("docker", "echo 'Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?' >&2; exit 1\n");
    let e = r.c.models.clean_leftovers(&r.c.ctx(), vec!["cell1".into()], vec!["r1-old".into()]).await.unwrap_err();
    assert_eq!(e, "Couldn't check r1-old's containers on cell1; nothing was stopped.");
    assert!(r.ran(0, "ids=$(docker ps -aq").is_empty());
    r.c.live.stop();
}

/// Download containers carry sparkbox.deployment=<job> too, and a job's id is never a load's.
#[tokio::test]
async fn a_running_download_isnt_a_leftover_and_survives_remove_leftovers() {
    let r = same("r1-dl", 1, Knobs { load_bytes: 0, ..Knobs::default() }).await;
    let me = crate::identity::instance_name();
    let run = format!("docker run -d --rm --name sparkbox-dl-job1 --label sparkbox.instance={me} --label sparkbox.deployment=job1 --entrypoint bash {} -c 'sleep 300'", fake_vllm::test_image_id());
    assert_eq!(r.c.live.run("cell1", &run, None, Duration::from_secs(30)).await.unwrap().code, Some(0));

    // Shown as what it is, and no reason not to load beside it.
    let spec = r.spec("r1-dl", 1, 1, 1);
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!((set.verdict.as_str(), set.problems.len()), ("ready", 0), "{:?}", set.problems);
    assert!(set.nodes[0].leftovers.is_empty(), "{:?}", set.nodes[0].leftovers);
    assert_eq!(set.nodes[0].downloads.iter().map(|l| (l.name.as_str(), l.deployment.as_str(), l.status.as_str())).collect::<Vec<_>>(), [("sparkbox-dl-job1", "job1", "Up 2 minutes")]);

    // Told to remove it by id, it's refused, and nothing goes.
    let e = r.c.models.clean_leftovers(&r.c.ctx(), vec!["cell1".into()], vec!["job1".into()]).await.unwrap_err();
    assert_eq!(e, "job1 is a download on cell1, not something a load left. Stop it in Models › Jobs.");
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-dl-job1"]);
    assert!(r.ran(0, "ids=$(docker ps -aq").is_empty(), "nothing was sent");

    r.ready(&spec).await;
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-dl-job1", "sparkbox-r1-dl"], "the load ran beside it");
    r.unload("r1-dl").await;
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-dl-job1"], "and left it running");
    r.c.live.stop();
}

#[tokio::test]
async fn a_running_ray_worker_refuses_and_is_left_alone() {
    let r = same("r1-ray", 1, Knobs::default()).await;
    let run = format!("docker run -d --name vllm-worker --entrypoint bash {} -c 'ray start --address=192.168.100.1:6379 --block'", fake_vllm::test_image_id());
    assert_eq!(r.c.live.run("cell1", &run, None, Duration::from_secs(30)).await.unwrap().code, Some(0));
    let set = r.c.models.preflight(&r.c.ctx(), &r.spec("r1-ray", 1, 1, 1)).await.unwrap();
    let why = "cell1 has a Ray worker still looking for its head (vllm-worker). SparkBox didn't start it; remove it in Containers.";
    assert_eq!((set.verdict.as_str(), set.problems.as_slice()), ("blocked", &[why.to_string()][..]));
    assert_eq!(r.ranks[0].standin.running(), ["vllm-worker"]);
    r.c.live.stop();
}

#[tokio::test]
async fn different_image_ids_and_different_stores_refuse() {
    let other_id = format!("sha256:{}", "b".repeat(64));
    let r = rig("r1-image", vec![Knobs::default(), Knobs { images: vec![(GLM53_IMAGE.into(), other_id.clone())], ..Knobs::default() }]).await;
    let spec = r.spec("r1-image", 2, 2, 1);
    let e = r.c.models.deploy(&r.c.ctx(), spec.clone()).await.unwrap_err();
    assert_eq!(e, format!("{GLM53_IMAGE} isn't the same image on every node: cell1 has aaaaaaaaaaaa, cell2 has bbbbbbbbbbbb. Pull the same tag on each node."));

    // Stores differ: the ids can't be compared, and it says so instead of a false mismatch.
    r.ranks[1].standin.set_knobs(Knobs { images: vec![(GLM53_IMAGE.into(), other_id)], store: CONTAINERD_STORE.into(), ..Knobs::default() });
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!(set.problems, ["Different Docker image stores on cell1 and cell2; pull the same tag on each node."]);
    assert_eq!((set.nodes[0].store.as_deref(), set.nodes[1].store.as_deref()), (Some("overlay2"), Some("containerd")));
    assert!(r.ran(0, "docker run -d").is_empty() && r.ran(0, "deploy-files").is_empty());
    r.c.live.stop();
}

#[tokio::test]
async fn different_files_refuse() {
    let r = same("r1-files", 2, Knobs::default()).await;
    let spec = r.spec("r1-files", 2, 2, 1);
    // The real listing of the same folder: they agree.
    assert_eq!(r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap().verdict, "ready");

    // cell2's copy lost a shard and has a short one, as a copy left wrong would.
    let listing = |files: Value| json!({ "path": r.folder, "repo": "", "revision": "", "files": files }).to_string();
    *r.ranks[0].manifest.lock().unwrap() = Some(listing(json!([{ "path": "config.json", "size": 10 }, { "path": "model-1.safetensors", "size": 4096 }, { "path": "model-2.safetensors", "size": 4096 }])));
    *r.ranks[1].manifest.lock().unwrap() = Some(listing(json!([{ "path": "config.json", "size": 10 }, { "path": "model-1.safetensors", "size": 2048 }])));
    let why = "The model's files differ on cell2: 2 files differ, first model-1.safetensors.";
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!((set.verdict.as_str(), set.problems.as_slice()), ("blocked", &[why.to_string()][..]));
    assert_eq!(r.c.models.deploy(&r.c.ctx(), spec).await.unwrap_err(), why);
    assert!(r.ran(1, "deploy-files").is_empty());
    r.c.live.stop();
}

/// A node without the folder has said so: an empty listing isn't 24 files that differ, whichever
/// node it is.
#[tokio::test]
async fn a_folder_a_node_lacks_is_one_problem() {
    let listing = |files: Value, folder: &str| json!({ "path": folder, "repo": "", "revision": "", "files": files }).to_string();
    let two = json!([{ "path": "config.json", "size": 10 }, { "path": "model.safetensors", "size": 4096 }]);
    let r = rig("r1-lack", vec![Knobs::default(), Knobs::default(), Knobs { no_model: true, ..Knobs::default() }]).await;
    let spec = r.spec("r1-lack", 3, 1, 3);
    for rank in 0..2 {
        *r.ranks[rank].manifest.lock().unwrap() = Some(listing(two.clone(), &r.folder));
    }
    *r.ranks[2].manifest.lock().unwrap() = Some(listing(json!([]), &r.folder));
    let lacks = plan::not_on_node("cell3", &r.folder);
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!((set.verdict.as_str(), set.problems.as_slice()), ("blocked", &[lacks.clone()][..]));

    // The head lacks it: the others are compared with each other, and they agree.
    r.ranks[0].standin.set_knobs(Knobs { no_model: true, ..Knobs::default() });
    r.ranks[2].standin.set_knobs(Knobs::default());
    *r.ranks[0].manifest.lock().unwrap() = Some(listing(json!([]), &r.folder));
    *r.ranks[2].manifest.lock().unwrap() = Some(listing(two.clone(), &r.folder));
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!(set.problems, [plan::not_on_node("cell1", &r.folder)]);

    // Two copies still get compared.
    *r.ranks[2].manifest.lock().unwrap() = Some(listing(json!([{ "path": "config.json", "size": 10 }, { "path": "model.safetensors", "size": 2048 }]), &r.folder));
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!(set.problems, [plan::not_on_node("cell1", &r.folder), "The model's files differ on cell3: 1 files differ, first model.safetensors.".to_string()]);
    r.c.live.stop();
}

#[tokio::test]
async fn a_failed_jumbo_half_refuses() {
    let r = rig("r1-jumbo", vec![Knobs::default(), Knobs { jumbo_fails: vec!["enP2p1s0f1np1".into()], ..Knobs::default() }]).await;
    let e = r.c.models.deploy(&r.c.ctx(), r.spec("r1-jumbo", 2, 2, 1)).await.unwrap_err();
    assert_eq!(e, "The jumbo-frame path from cell2 to cell1 failed on enP2p1s0f1np1: the model's traffic can't cross the fabric.");
    // Both halves were tried, each against the head's address on its own subnet.
    let mut pings = r.ranks[1].standin.pings();
    pings.sort();
    assert_eq!(pings, ["enP2p1s0f1np1 192.168.101.1", "enp1s0f1np1 192.168.100.1"]);
    assert!(r.ranks[0].standin.pings().is_empty(), "only the workers ping");
    assert!(r.ran(1, "deploy-files").is_empty() && r.c.models.deployments(&r.c.ctx()).is_empty(), "nothing started");
    r.c.live.stop();
}

/// A worker with only the second half: the head's first half is its `--master-addr`, and it has no
/// interface there. No ping could show it, so the page and the load both refuse from the facts.
#[tokio::test]
async fn a_worker_off_the_heads_subnet_refuses_without_a_ping() {
    let r = same("r1-unpaired", 2, Knobs::default()).await;
    let spec = r.spec("r1-unpaired", 2, 2, 1);
    let mut one_half: Value = serde_json::from_str(&check_reply(2)).unwrap();
    one_half["fabric"].as_array_mut().unwrap().retain(|f| f["iface"] == "enP2p1s0f1np1");
    *r.ranks[1].check.lock().unwrap() = Some(one_half.to_string());
    let why = "The jumbo-frame path from cell2 to cell1 failed on enp1s0f1np1: the model's traffic can't cross the fabric.";
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!((set.verdict.as_str(), set.problems.as_slice()), ("blocked", &[why.to_string()][..]));
    assert_eq!(r.c.models.deploy(&r.c.ctx(), spec).await.unwrap_err(), why);
    assert!(r.ranks[1].standin.pings().is_empty() && r.ran(1, "deploy-files").is_empty(), "nothing was sent or started");
    r.c.live.stop();
}

/// A node that drops mid-read, as `ssh::collect` hands it over: what arrived, then a channel closed
/// with no exit status. The load reads the nodes again, and refuses the same way.
#[tokio::test]
async fn a_read_cut_off_before_a_section_is_never_ready() {
    for section in ["port", "containers", "ray", "ollama"] {
        let held = Knobs { ollama_ps: Some(r#"{"models":[{"name":"qwen3:27b"}]}"#.into()), ..Knobs::default() };
        let cut = Knobs { preflight_stops_before: Some(section.into()), ..held.clone() };
        let r = rig(&format!("r1-cut-{section}"), vec![held, cut]).await;
        let spec = r.spec("r1-cut", 2, 2, 1);
        r.ranks[1].node.close_without_status("echo '--- mem'");
        let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
        assert_eq!(set.verdict, "blocked", "{section}");
        assert!(set.nodes[1].cut_off && !set.nodes[0].cut_off, "{section}");
        let said = set.nodes[1].problems.join(" ");
        assert!(said.starts_with("Couldn't finish reading cell2: it stopped at ") && said.ends_with("Check again."), "{section}: {said}");
        r.ranks[1].node.close_without_status("echo '--- mem'");
        let e = r.c.models.deploy(&r.c.ctx(), spec).await.unwrap_err();
        assert!(e.contains("Couldn't finish reading cell2"), "{section}: {e}");
        assert!(r.ran(1, "deploy-files").is_empty(), "nothing started");
        r.c.live.stop();
    }
}

/// Every header came and curl was still waiting: the text is what a finished read prints, so only
/// the missing exit status tells them apart. Check again then reads the node whole.
#[tokio::test]
async fn a_read_cut_off_in_its_last_section_is_told_by_its_exit() {
    let r = same("r1-cut-last", 2, Knobs::default()).await;
    let spec = r.spec("r1-cut-last", 2, 2, 1);
    r.ranks[1].node.close_without_status("echo '--- mem'");
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!(set.verdict, "blocked");
    assert_eq!(set.problems, ["Couldn't finish reading cell2: it stopped at Ollama, and nothing from there on was checked. Check again."]);
    let again = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!((again.verdict.as_str(), again.nodes[1].cut_off), ("ready", false), "{:?}", again.problems);
    r.c.live.stop();
}

#[tokio::test]
async fn look_only_sends_no_ping() {
    let r = same("r1-noping", 2, Knobs::default()).await;
    r.look_only(true);
    let spec = r.spec("r1-noping", 2, 2, 1);
    // The Load page reads the nodes; nothing it does changes them.
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!((set.verdict.as_str(), set.image_id.clone()), ("ready", Some(fake_vllm::test_image_id())));
    assert_eq!(set.nodes[1].fabric.len(), 2, "fabric facts, from each node's own check");
    // Every read a load makes of itself passes the exit in look-only, as a read.
    for cmd in [rank_state_command("sparkbox-r1-noping-r1", Some(600)), rank_state_command("sparkbox-r1-noping-r1", None), plan::transport_command("sparkbox-r1-noping-r0"), plan::load_containers_command("r1-noping")] {
        assert!(r.c.live.run("cell1", &cmd, None, Duration::from_secs(20)).await.is_ok(), "{cmd}");
    }

    // Load is refused at the first change, in look-only's words, and never pings.
    r.deploy(&spec).await;
    let d = r.wait("r1-noping", "failed").await;
    assert_eq!(d.message, crate::admin::LOOK_ONLY);
    for rank in 0..2 {
        assert!(r.ran(rank, "ping -c").is_empty() && r.ranks[rank].standin.pings().is_empty(), "no ping");
        assert!(r.ran(rank, "deploy-files").is_empty() && r.ran(rank, "docker run").is_empty(), "no change");
        assert!(!r.ranks[rank].node.home.join(format!(".sparkbox/{}", plan::launcher_name())).exists(), "not even the launcher");
    }
    let rows: Vec<_> = r.c.live.audit.entries().into_iter().filter(|e| e.changes && e.title != "Refused" && e.kind != "app").collect();
    assert!(rows.is_empty(), "{rows:?}");
    r.c.live.stop();
}

#[tokio::test]
async fn the_page_cache_is_dropped_on_each_node_that_needs_it_with_one_consent() {
    let short = Knobs { mem_free: 43_201_000 * 1024, mem_cached: 75 << 30, ..Knobs::default() };
    let r = rig("r1-drop", vec![short.clone(), Knobs::default(), short]).await;
    let spec = r.spec("r1-drop", 3, 1, 3);
    let e = r.c.models.deploy(&r.c.ctx(), spec.clone()).await.unwrap_err();
    assert!(e.contains("cell1 has 41.2 GiB free") && e.contains("cell3 has 41.2 GiB free") && !e.contains("cell2 has"), "{e}");
    assert_eq!(r.ranks[0].standin.drops(), 0);

    r.ready(&DeploySpec { drop_caches: true, ..spec }).await;
    assert_eq!(r.ranks.iter().map(|x| x.standin.drops()).collect::<Vec<_>>(), [1, 0, 1]);
    let rows = r.c.live.audit.entries();
    let drops: Vec<_> = rows.iter().filter(|e| e.kind == "sudo" && e.command == plan::DROP_CACHES).collect();
    assert_eq!(drops.len(), 2, "{drops:?}");
    r.unload("r1-drop").await;
    r.c.live.stop();
}

// ---- Restart and unload

#[tokio::test]
async fn restart_stops_head_then_workers_and_starts_workers_then_head() {
    let r = same("r1-restart", 2, Knobs::default()).await;
    let spec = r.spec("r1-restart", 2, 2, 1);
    r.ready(&spec).await;
    let (h, w) = ("sparkbox-r1-restart-r0", "sparkbox-r1-restart-r1");
    let (first_head, first_worker) = (r.ranks[0].standin.started_ms(h).unwrap(), r.ranks[1].standin.started_ms(w).unwrap());
    let key = secrets::get(&plan::key_account("r1-restart")).unwrap().unwrap();

    let id = r.c.models.restart(&r.c.ctx(), "r1-restart").await.unwrap();
    // The containers come back in order, and it answers again.
    let d = r.until("r1-restart", "restarted and ready", |d| d.generation == 1 && d.state == DeploymentState::Ready).await;
    assert_eq!(r.load("r1-restart").failure, None);
    let (head_up, worker_up) = (r.ranks[0].standin.started_ms(h).unwrap(), r.ranks[1].standin.started_ms(w).unwrap());
    assert!(head_up > first_head && worker_up > first_worker, "both started again");
    assert!(worker_up < head_up, "workers first: worker {worker_up}, head {head_up}");
    let worker_down = r.ranks[1].standin.ended_ms(w).unwrap();
    assert!(worker_down < worker_up, "stopped before anything started");
    // Stopped by role: the head's stop had answered before the workers' was sent. (The processes
    // don't say: a worker that loses its head can end first, and the head's API takes longer.)
    let rows = r.c.live.audit.entries();
    let row = |role: &str| rows.iter().find(|e| e.command == plan::stop_role_command("r1-restart", role)).unwrap_or_else(|| panic!("no stop of the {role}"));
    assert!(answered_before(row("head"), row("worker")), "the head first: {:?} {:?}", row("head"), row("worker"));
    assert_eq!((row("head").node.as_str(), row("worker").node.as_str()), ("cell1", "cell2"));
    assert!(d.spec.image_id == fake_vllm::test_image_id());

    // The pre-flight ran again, and the launch files were written again with the same key.
    assert!(r.ran(0, "deploy-files").len() == 2 && r.ran(1, "deploy-files").len() == 2);
    assert_eq!(r.ranks[1].standin.pings().len(), 2, "the two halves at deploy, none since");
    assert_eq!(std::fs::read_to_string(r.ranks[0].node.home.join(".sparkbox/keys/r1-restart")).unwrap(), key);

    let t = (0..200).find_map(|_| r.tasks.get(&id).filter(|t| t.finished()));
    let t = match t {
        Some(t) => t,
        None => {
            for _ in 0..200 {
                r.c.models.endpoints_tick(&r.c.ctx(), true).await;
                if r.tasks.get(&id).is_some_and(|t| t.finished()) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            r.tasks.get(&id).unwrap()
        }
    };
    assert_eq!(t.title, "Restart gemma-3-27b-it in order");
    let steps: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(steps[..6], ["Check the nodes first", "Stop the head on cell1", "Stop the workers on cell2", "Check the nodes again", "Start the workers on cell2", "Start the head on cell1"], "{t:?}");
    r.unload("r1-restart").await;
    r.c.live.stop();
}

#[tokio::test]
async fn a_restart_that_fails_keeps_the_key_for_keep_running_and_cleans_up() {
    let r = same("r1-rf", 1, Knobs::default()).await;
    let spec = r.spec("r1-rf", 1, 1, 1);
    r.ready(&spec).await;
    let key = secrets::get(&plan::key_account("r1-rf")).unwrap().unwrap();
    r.ranks[0].standin.set_knobs(Knobs { rendezvous: None, warmup_s: 300.0, rank_dies_after_s: Some(0.2), ..Knobs::default() });
    r.c.models.restart(&r.c.ctx(), "r1-rf").await.unwrap();
    let d = r.until("r1-rf", "failed", |d| d.state == DeploymentState::Failed && d.generation == 1).await;
    assert!(d.recoverable, "it ran before: Keep running may try again");
    assert!(d.failure.as_ref().is_some_and(|f| f.left_on.is_empty()), "{:?}", d.failure);
    assert_eq!(secrets::get(&plan::key_account("r1-rf")).unwrap().as_deref(), Some(key.as_str()), "a restart's key stays");
    assert!(r.c.llm.get("vllm-r1-rf").await.is_none(), "not offered while it's down");
    assert!(d.failure.is_some());
    r.nothing_left("r1-rf").await;
    r.c.live.stop();
}

#[tokio::test]
async fn a_restart_refused_at_the_write_exit_leaves_the_running_load_unchanged() {
    let r = same("r1-refused", 1, Knobs::default()).await;
    let spec = r.spec("r1-refused", 1, 1, 1);
    let before = r.ready(&spec).await;
    r.look_only(true);

    let task = r.c.models.restart(&r.c.ctx(), &spec.id).await.unwrap();
    let t = finished(&r.tasks, &task).await;
    assert_eq!(t.state, TaskState::Failed, "{t:?}");
    assert!(t.error.as_deref().is_some_and(|e| e.to_lowercase().contains("look-only")), "{t:?}");
    let after = r.load(&spec.id);
    assert_eq!((after.state, after.generation, after.message, after.started_at, after.failure),
        (before.state, before.generation, before.message, before.started_at, None));
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-refused"]);
    assert!(r.ran(0, "docker stop").is_empty() && r.ran(0, "ids=$(docker ps -aq").is_empty(), "no stop or rollback was sent");
    fake_vllm_health(&r, "cell1", spec.port).await;

    r.look_only(false);
    r.unload(&spec.id).await;
    r.c.live.stop();
}

/// A load on clean memory never asked for the drop, and its weights fill the page cache as it
/// runs. Stopping it then would strand it, so the restart looks first.
#[tokio::test]
async fn a_restart_that_would_come_up_short_of_memory_stops_nothing() {
    let r = same("r1-short", 1, Knobs::default()).await;
    let spec = r.spec("r1-short", 1, 1, 1);
    let before = r.ready(&spec).await;
    assert!(!before.spec.drop_caches, "no consent: the first load didn't need the drop");
    r.ranks[0].standin.set_knobs(Knobs { mem_free: 60 << 30, mem_cached: 56 << 30, ..Knobs::default() });

    let why = "cell1 has 91.3 GiB free once this load stops; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Click Restart and choose 'Restart and allow cache drop' to permit it for this attempt (sudo; the kernel refills it). Nothing was stopped.";
    let task = r.c.models.restart(&r.c.ctx(), "r1-short").await.unwrap();
    let t = finished(&r.tasks, &task).await;
    assert_eq!((t.state, t.error.as_deref()), (TaskState::Failed, Some(why)), "{t:?}");
    assert_eq!(t.steps.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["Check the nodes first"]);

    // Exactly as it was, and it still answers.
    let d = r.load("r1-short");
    assert_eq!((d.state, d.generation, d.message.as_str(), d.failure), (DeploymentState::Ready, 0, before.message.as_str(), None));
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-short"]);
    assert!(r.ran(0, "docker stop").is_empty() && r.ran(0, "ids=$(docker ps -aq").is_empty(), "no stop was sent");
    assert_eq!(r.ranks[0].standin.drops(), 0);
    fake_vllm_health(&r, "cell1", spec.port).await;

    // Keep running gets the same answer.
    let spec = DeploySpec { keep_running: true, id: "r1-short2".into(), port: fake_vllm::free_port(), ..spec };
    r.unload("r1-short").await;
    r.ranks[0].standin.set_knobs(Knobs::default());
    r.ready(&spec).await;
    r.ranks[0].standin.set_knobs(Knobs { mem_free: 60 << 30, mem_cached: 56 << 30, ..Knobs::default() });
    assert_eq!(r.c.models.restart_ordered(&r.c.ctx(), "r1-short2", "Keep running: restart|(1 of 5)", true).await.unwrap_err(), why);
    assert_eq!(r.load("r1-short2").state, DeploymentState::Ready);
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-short2"]);
    r.unload("r1-short2").await;
    r.c.live.stop();
}

/// A manual grant reaches both preflights, drops only where needed and keeps the key, but cannot
/// turn into a stored grant, a Retry argument, or permission for Keep running.
#[tokio::test]
async fn restart_cache_consent_is_one_attempt_and_keeps_the_key_on_two_nodes() {
    let r = same("cache-once", 2, Knobs::default()).await;
    let spec = r.spec("cache-once", 2, 2, 1);
    r.ready(&spec).await;
    let key = secrets::get(&plan::key_account(&spec.id)).unwrap().unwrap();
    let short = Knobs { mem_free: 60 << 30, mem_cached: 56 << 30, ..r.ranks[0].standin.knobs() };
    r.ranks[0].standin.set_knobs(short.clone());
    fence(&r, &["cell1", "cell2"], vec![FenceClass::ModelLoad, FenceClass::ModelUnload, FenceClass::DropCaches]);

    let task = r.restarted_with_consent(&spec.id, true).await;
    assert_eq!(task.state, TaskState::Ok, "{task:?}");
    assert_eq!(task.retry.unwrap().args, json!({ "id": spec.id }), "Retry must ask again");
    let d = r.load(&spec.id);
    assert_eq!((d.state, d.generation, d.spec.drop_caches), (DeploymentState::Ready, 1, false));
    let saved: Vec<Deployment> = serde_json::from_slice(&std::fs::read(r.c.dir.join("deployments.json")).unwrap()).unwrap();
    assert!(!saved.iter().find(|d| d.spec.id == spec.id).unwrap().spec.drop_caches, "no grant on disk");
    assert_eq!(r.ranks.iter().map(|x| x.standin.drops()).collect::<Vec<_>>(), [1, 0]);
    assert_eq!(secrets::get(&plan::key_account(&spec.id)).unwrap().as_deref(), Some(key.as_str()));
    assert_eq!(std::fs::read_to_string(r.ranks[0].node.home.join(".sparkbox/keys").join(&spec.id)).unwrap(), key);
    let rows = r.c.live.audit.entries();
    let drop = rows.iter().find(|e| e.kind == "sudo" && e.command == plan::DROP_CACHES).unwrap();
    assert_eq!((drop.node.as_str(), drop.fence.as_deref()), ("cell1", Some("drop-caches")));
    let stop = rows.iter().find(|e| e.node == "cell1" && e.command == plan::stop_role_command(&spec.id, "head")).unwrap();
    assert!(answered_before(stop, drop), "drop follows the stop and fresh preflight");

    // Fill the cache again: neither a later manual restart nor Keep running inherits consent.
    r.ranks[0].standin.set_knobs(short);
    let task = r.restarted(&spec.id).await;
    assert_eq!(task.state, TaskState::Failed);
    assert!(task.error.unwrap().ends_with("Nothing was stopped."));
    r.gate.unlock(None).unwrap();
    let err = r.c.models.restart_ordered(&r.c.ctx(), &spec.id, "Keep running: restart|(1 of 5)", true).await.unwrap_err();
    assert!(err.contains("Restart and allow cache drop") && err.ends_with("Nothing was stopped."), "{err}");
    assert_eq!(r.load(&spec.id).generation, 1);
    assert_eq!(r.ranks[0].standin.drops(), 1);
    r.unload(&spec.id).await;
    r.c.live.stop();
}

#[tokio::test]
async fn restart_cache_consent_drops_nothing_when_memory_is_already_free() {
    let r = same("cache-free", 1, Knobs::default()).await;
    let spec = r.spec("cache-free", 1, 1, 1);
    r.ready(&spec).await;
    let task = r.restarted_with_consent(&spec.id, true).await;
    assert_eq!(task.state, TaskState::Ok, "{task:?}");
    assert_eq!(r.ranks[0].standin.drops(), 0);
    assert!(!r.load(&spec.id).spec.drop_caches);
    r.unload(&spec.id).await;
    r.c.live.stop();
}

#[tokio::test]
async fn restart_cache_consent_cannot_bypass_look_only_or_the_drop_fence() {
    let r = same("cache-fence", 1, Knobs::default()).await;
    let spec = r.spec("cache-fence", 1, 1, 1);
    r.ready(&spec).await;
    r.ranks[0].standin.set_knobs(Knobs { mem_free: 60 << 30, mem_cached: 56 << 30, ..Knobs::default() });
    r.look_only(true);
    let task = r.restarted_with_consent(&spec.id, true).await;
    assert_eq!(task.state, TaskState::Failed);
    assert!(task.error.unwrap().to_lowercase().contains("look-only"));
    assert_eq!((r.load(&spec.id).state, r.load(&spec.id).generation), (DeploymentState::Ready, 0));
    assert!(r.ran(0, "docker stop").is_empty());
    assert_eq!(r.ranks[0].standin.drops(), 0);

    r.look_only(false);
    fence(&r, &["cell1"], vec![FenceClass::ModelLoad, FenceClass::ModelUnload]);
    let task = r.restarted_with_consent(&spec.id, true).await;
    assert_eq!(task.state, TaskState::Failed);
    assert!(task.error.unwrap().contains("The fence doesn't allow page-cache drops"));
    assert_eq!(r.ranks[0].standin.drops(), 0);
    assert!(!r.load(&spec.id).spec.drop_caches);
    assert_eq!(r.load(&spec.id).state, DeploymentState::Failed, "the load is down after the admitted stop");
    assert!(r.load(&spec.id).message.contains("The fence doesn't allow page-cache drops"));
    assert!(r.ranks[0].standin.running().is_empty());
    assert_eq!(r.ran(0, "docker run -d").len(), 1, "no new rank was started");
    assert!(secrets::get(&plan::key_account(&spec.id)).unwrap().is_some(), "the restart key stays");
    fence(&r, &["cell1"], vec![FenceClass::ModelLoad, FenceClass::ModelUnload, FenceClass::DropCaches]);
    let retry = r.restarted(&spec.id).await;
    assert!(retry.error.unwrap().contains("Restart and allow cache drop"), "the failed grant expired too");
    assert_eq!(r.ranks[0].standin.drops(), 0);
    r.unload(&spec.id).await;
    r.c.live.stop();
}

/// The look before the stop is an estimate. When the read after it says otherwise the load is down,
/// and what it says names a control a restart has.
#[tokio::test]
async fn a_restart_short_of_memory_after_its_stop_says_where_the_tick_is() {
    let r = same("r1-after", 1, Knobs { load_bytes: 0, ..Knobs::default() }).await;
    r.ready(&r.spec("r1-after", 1, 1, 1)).await;
    r.ranks[0].standin.set_knobs(Knobs { mem_free: 43_201_000 * 1024, mem_cached: 75 << 30, load_bytes: 0, ..Knobs::default() });
    let e = r.c.models.restart_ordered(&r.c.ctx(), "r1-after", "Restart", false).await.unwrap_err();
    assert_eq!(e, "cell1 has 41.2 GiB free; vLLM at share 0.75 needs 95.3 GiB free (4 GiB of it headroom). Click Restart and choose 'Restart and allow cache drop' to permit it for this attempt (sudo; the kernel refills it).");
    let d = r.load("r1-after");
    assert_eq!((d.state, d.message.as_str()), (DeploymentState::Failed, e.as_str()));
    r.c.live.stop();
}

/// What a load holds counts as coming back only while it's there to give it: a load that failed
/// and left nothing is read as it is.
#[tokio::test]
async fn a_restart_of_a_load_with_nothing_running_counts_nothing_as_coming_back() {
    let r = same("r1-down", 1, Knobs { warmup_s: 300.0, rank_dies_after_s: Some(0.2), ..Knobs::default() }).await;
    let spec = DeploySpec { require_key: false, ..r.spec("r1-down", 1, 1, 1) };
    r.deploy(&spec).await;
    r.wait("r1-down", "failed").await;
    r.ranks[0].standin.set_knobs(Knobs { mem_free: 60 << 30, mem_cached: 56 << 30, ..Knobs::default() });
    let e = r.c.models.restart_ordered(&r.c.ctx(), "r1-down", "Restart", false).await.unwrap_err();
    assert!(e.starts_with("cell1 has 60.0 GiB free; vLLM at share 0.75") && e.ends_with("Nothing was stopped."), "{e}");
    assert_eq!(r.load("r1-down").generation, 0, "it never began");
    r.c.live.stop();
}

/// A restart with no key to give it stops at once, with its reason: nothing is stopped.
#[tokio::test]
async fn a_restart_whose_key_went_says_so_and_stops_nothing() {
    let r = same("r1-gone", 1, Knobs::default()).await;
    let before = r.ready(&r.spec("r1-gone", 1, 1, 1)).await;
    secrets::clear(&plan::key_account("r1-gone")).unwrap();
    let e = r.c.models.restart_ordered(&r.c.ctx(), "r1-gone", "Restart", false).await.unwrap_err();
    assert_eq!(e, "Its API key is gone from the Keychain; load it again instead.");
    let d = r.load("r1-gone");
    assert_eq!((d.state, d.generation, d.message), (DeploymentState::Ready, before.generation, before.message));
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-gone"], "it was never stopped");
    assert!(r.ran(0, "docker stop").is_empty() && r.ran(0, "ids=$(docker ps -aq").is_empty());
    r.unload("r1-gone").await;
    r.c.live.stop();
}

#[tokio::test]
async fn keep_running_needs_admin_to_drop_the_page_cache() {
    let r = same("r1-keep", 1, Knobs::default()).await;
    let spec = DeploySpec { drop_caches: true, keep_running: true, ..r.spec("r1-keep", 1, 1, 1) };
    r.ready(&spec).await;
    // The node's page cache has filled since; Admin is locked, as it is at night.
    r.ranks[0].standin.set_knobs(Knobs { mem_free: 43_201_000 * 1024, mem_cached: 75 << 30, ..Knobs::default() });
    assert!(!r.gate.state().unlocked);
    // Bounded, so a restart that got past this check fails here instead of waiting out 90 minutes.
    r.c.models.set_ready_within(Duration::from_secs(5));
    let e = r.c.models.restart_ordered(&r.c.ctx(), "r1-keep", "Keep running: restart|(1 of 5)", true).await.unwrap_err();
    r.c.models.set_ready_within(Duration::from_millis(endpoints::START_LIMIT_MS));
    assert_eq!(e, "cell1 needs its page cache dropped before this load can start again. Unlock Admin or explicitly grant Keep running permission in Settings › Security. Nothing was stopped.");
    assert_eq!(r.ranks[0].standin.drops(), 0);
    assert_eq!(r.load("r1-keep").state, DeploymentState::Ready, "refused before its first stop");
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-keep"]);
    assert_eq!(r.ran(0, "docker run -d").len(), 1, "it didn't start again");

    // An explicit, scoped grant allows only the conditional cache step while Admin is idle-locked.
    let grant = cache_grant::test_grant(&["cell1"]);
    let tick = { let c = r.c.models.clone(); let ctx = r.c.ctx(); tokio::spawn(async move { loop { c.endpoints_tick(&ctx, true).await; tokio::time::sleep(Duration::from_millis(30)).await; } }) };
    let result = r.c.models.restart_ordered(&r.c.ctx(), "r1-keep", "Keep running: restart|(1 of 5)", true).await;
    tick.abort(); result.unwrap(); drop(grant);
    assert!(!cache_grant::covers(["cell1"]), "revocation removes the next attempt's permission");
    r.until("r1-keep", "restarted", |d| d.generation == 1 && d.state == DeploymentState::Ready).await;
    assert_eq!(r.ranks[0].standin.drops(), 1);
    r.unload("r1-keep").await;
    r.c.live.stop();
}

#[tokio::test]
async fn unload_stops_the_head_first() {
    let r = same("r1-unload", 3, Knobs::default()).await;
    let spec = r.spec("r1-unload", 3, 1, 3);
    r.ready(&spec).await;
    let t = r.unload("r1-unload").await;
    assert_eq!(t.state, TaskState::Ok, "{t:?}");
    let rows = r.c.live.audit.entries();
    let row = |node: &str| rows.iter().find(|e| e.node == node && e.command == plan::unload_command("r1-unload")).unwrap_or_else(|| panic!("no stop on {node}"));
    assert!(answered_before(row("cell1"), row("cell2")) && answered_before(row("cell1"), row("cell3")), "the head's stop had answered before the others were sent");
    r.nothing_left("r1-unload").await;
    r.c.live.stop();
}

#[tokio::test]
async fn one_node_loads_restarts_and_unloads_on_c_l2() {
    let r = same("r1-one", 1, Knobs::default()).await;
    let spec = r.spec("r1-one", 1, 1, 1);
    let d = r.ready(&spec).await;
    assert_eq!(r.ran(0, "docker run -d"), [plan::rank_command(&d.spec, 0)]);
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-one"]);
    let argv = r.ranks[0].standin.argv("sparkbox-r1-one");
    assert!(argv.contains(&"sparkbox.role=single".to_string()) && !argv.contains(&"--privileged".to_string()), "{argv:?}");
    assert!(r.ranks[0].standin.pings().is_empty(), "one node has no jumbo path to test");
    r.c.models.restart(&r.c.ctx(), "r1-one").await.unwrap();
    r.until("r1-one", "restarted", |d| d.generation == 1 && d.state == DeploymentState::Ready).await;
    assert_eq!(r.ran(0, "docker run -d").len(), 2);
    r.unload("r1-one").await;
    r.nothing_left("r1-one").await;
    r.c.live.stop();
}

// ---- Names and addresses

#[tokio::test]
async fn renamed_node_keeps_its_load() {
    let r = same("r1-renamed", 4, Knobs::default()).await;
    // gx10-0000 as it is now: id cell4, hostname cell4-gx10, and a display name the owner can change.
    let mut ctx = r.c.ctx();
    let cell4 = ctx.inventory.nodes.iter_mut().find(|n| n.id == "cell4").unwrap();
    cell4.host = "cell4-gx10".into();
    cell4.name = "The ASUS".into();
    let spec = r.on("r1-renamed", &["cell1", "cell4"], 2, 1);
    r.c.models.deploy(&ctx, spec).await.unwrap();
    r.wait("r1-renamed", "ready").await;

    // Rank names and fabric facts: no node name in a container, a label, or a launch file.
    let name = "sparkbox-r1-renamed-r1";
    assert_eq!(r.ranks[3].standin.running(), [name]);
    let (argv, env) = files_of(&r.ranks[3], "r1-renamed");
    let everything = format!("{:?} {argv} {env}", r.ranks[3].standin.argv(name));
    for word in ["cell4", "gx10", "The ASUS"] {
        assert!(!everything.contains(word), "{word} in {everything}");
    }
    assert_eq!(env["VLLM_HOST_IP"].as_str(), Some("192.168.100.4"), "from the node's own check, not its name");

    // Renamed again while it serves: still its load, and Unload still finds it by label.
    let mut ctx = r.c.ctx();
    ctx.inventory.nodes.iter_mut().find(|n| n.id == "cell4").unwrap().name = "Spark four".into();
    let t = finished(&r.tasks, &r.c.models.unload(&ctx, "r1-renamed").await.unwrap()).await;
    assert_eq!(t.state, TaskState::Ok, "{t:?}");
    assert!(r.ranks[3].standin.running().is_empty());
    r.c.live.stop();
}

#[tokio::test]
async fn no_lan_address_in_any_multi_node_env() {
    let r = same("r1-lan", 2, Knobs { warmup_s: 30.0, ..Knobs::default() }).await;
    let mut ctx = r.c.ctx();
    ctx.inventory.nodes[0].ip = Some(LAN.into());
    r.c.models.deploy(&ctx, r.spec("r1-lan", 2, 2, 1)).await.unwrap();
    r.until("r1-lan", "both ranks running", |d| matches!(d.stage, Some(Stage::Joining { have: 2, .. }) | Some(Stage::Loading { .. }))).await;
    let (argv, env) = files_of(&r.ranks[0], "r1-lan");
    let (worker_argv, worker_env) = files_of(&r.ranks[1], "r1-lan");
    for text in [env.to_string(), worker_env.to_string(), worker_argv.to_string()] {
        assert!(!text.contains("192.168.50."), "{text}");
    }
    let words: Vec<&str> = argv.as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(words.iter().filter(|w| w.contains("192.168.50.")).copied().collect::<Vec<_>>(), [LAN], "{words:?}");
    assert!(words.windows(2).any(|w| w == ["--host", LAN]) && words.windows(2).any(|w| w == ["--master-addr", "192.168.100.1"]), "{words:?}");
    r.unload("r1-lan").await;
    r.c.live.stop();
}

#[tokio::test]
async fn transport_warns_on_a_lan_message_queue() {
    let lan = Knobs { mq_on_lan: Some(LAN.into()), ..Knobs::default() };
    let r = rig("r1-mq", vec![lan, Knobs::default()]).await;
    r.ready(&r.spec("r1-mq", 2, 2, 1)).await;
    let d = r.until("r1-mq", "the transport read", |d| d.transport.is_some()).await;
    let t = d.transport.clone().unwrap();
    assert_eq!((t.mq_ip.as_deref(), t.ib.as_slice(), t.socket), (Some(LAN), &["rocep1s0f1".to_string(), "roceP2p1s0f1".to_string()][..], false));
    let warning = "It isn't using the RDMA fabric: its message queue is on 192.168.50.51, which isn't a fabric address.";
    assert_eq!(d.transport_warning.as_deref(), Some(warning));
    assert!(d.message.ends_with(warning), "{}", d.message);
    assert_eq!(d.state, DeploymentState::Ready, "it carries on");
    let row = r.c.live.audit.entries().into_iter().find(|e| e.command == plan::transport_command("sparkbox-r1-mq-r0")).expect("the read's row");
    assert!(row.stdout_tail.is_some_and(|t| t.contains("mq_connect_ip=192.168.50.51")));
    r.unload("r1-mq").await;

    // On the fabric, with both devices: no warning.
    let r = same("r1-mq2", 2, Knobs::default()).await;
    r.ready(&r.spec("r1-mq2", 2, 2, 1)).await;
    let d = r.until("r1-mq2", "the transport read", |d| d.transport.is_some()).await;
    assert_eq!((d.transport.unwrap().mq_ip.as_deref(), d.transport_warning), (Some("192.168.100.1"), None));
    r.unload("r1-mq2").await;
    r.c.live.stop();
}

// ---- The fence

/// The debug build's exit, enforced on these nodes for these classes, signed as Settings signs it.
fn fence(r: &Rig, nodes: &[&str], classes: Vec<FenceClass>) {
    use crate::live::exit::{Enforce, KEY_KIND};
    use crate::live::fence;
    let instance = crate::identity::instance_name();
    let (dir, config) = (r.c.dir.join("SparkBox Fence"), r.c.dir.join(format!("config-{instance}")));
    std::fs::create_dir_all(&dir).unwrap();
    let key = fence::new_key().unwrap();
    let (key_id, public_key) = fence::public_of(&key).unwrap();
    let file = fence::KeyFile { key_id, public_key, kind: KEY_KIND.into() };
    fence::save_trust(&config, &file).unwrap();
    r.c.live.enforce_writes(Enforce { instance: instance.into(), fence_dir: dir.clone(), config_dir: Some(config.clone()), noauth: false, tag: "p2-r1".into(), mac: Some("Studio".into()) });
    let spec = fence::Spec { nodes: nodes.iter().map(|n| n.to_string()).collect(), classes, minutes: 180, instance: instance.into(), ..Default::default() };
    let record = fence::record_for(&spec, &key, now_ms(), "Studio").unwrap();
    fence::write(&dir, &config, &file, &record).unwrap();
}

#[tokio::test]
async fn a_several_node_load_needs_model_load_and_each_row_says_so() {
    let r = same("r1-fence", 2, Knobs::default()).await;
    let spec = r.spec("r1-fence", 2, 2, 1);
    // A fence naming only model-load-one refuses a head and a worker, before anything is written.
    fence(&r, &["cell1", "cell2"], vec![FenceClass::ModelLoadOne, FenceClass::ModelUnload]);
    r.deploy(&spec).await;
    let d = r.wait("r1-fence", "failed").await;
    assert!(d.message.contains("The fence doesn't allow"), "{}", d.message);
    assert_eq!(d.failure, None, "nothing was touched, so there's nothing to roll back");
    assert!(r.ran(0, "deploy-files").is_empty() && r.ran(1, "deploy-files").is_empty());

    // model-load: every rank's files and run, tagged, and the unload model-unload.
    fence(&r, &["cell1", "cell2"], vec![FenceClass::ModelLoad, FenceClass::ModelUnload]);
    r.ready(&DeploySpec { port: fake_vllm::free_port(), ..spec }).await;
    let rows = r.c.live.audit.entries();
    let fenced: Vec<_> = rows.iter().filter(|e| e.title != "Refused" && (e.command.contains("docker run -d") || e.command.contains("sparkjob.py deploy-files"))).collect();
    assert_eq!(fenced.len(), 4, "{fenced:?}");
    assert!(fenced.iter().all(|e| e.fence.as_deref() == Some("model-load")), "{fenced:?}");
    r.until("r1-fence", "the transport read, which needs no fence", |d| d.transport.is_some()).await;
    assert_eq!(r.unload("r1-fence").await.state, TaskState::Ok);
    let rows = r.c.live.audit.entries();
    let unloads: Vec<_> = rows.iter().filter(|e| e.command == plan::unload_command("r1-fence")).collect();
    assert_eq!(unloads.len(), 2, "{unloads:?}");
    assert!(unloads.iter().all(|e| e.fence.as_deref() == Some("model-unload")), "{unloads:?}");
    r.c.live.stop();
}

#[tokio::test]
async fn a_model_load_fence_admits_a_load_on_one_node() {
    let r = same("r1-fence1", 1, Knobs::default()).await;
    fence(&r, &["cell1"], vec![FenceClass::ModelLoad, FenceClass::ModelUnload]);
    r.ready(&r.spec("r1-fence1", 1, 1, 1)).await;
    assert_eq!(r.unload("r1-fence1").await.state, TaskState::Ok);
    r.c.live.stop();
}

// ---- Trying to break it

#[tokio::test]
async fn a_head_that_exits_while_workers_wait_fails_and_cleans_up() {
    let r = rig("r1-headdies", vec![Knobs { rank_dies_after_s: Some(0.2), ..Knobs::default() }, Knobs { join_held: true, ..Knobs::default() }]).await;
    r.deploy(&r.spec("r1-headdies", 2, 2, 1)).await;
    let d = r.wait("r1-headdies", "failed").await;
    assert!(d.message.contains("vLLM stopped on cell1 (exit 1)."), "{}", d.message);
    r.nothing_left("r1-headdies").await;
    r.c.live.stop();
}

/// Look-only comes on while the workers start: the head is never started, the clean-up is held
/// back too, and what's left is named. Nothing is said to have stopped that didn't.
#[tokio::test]
async fn look_only_between_the_workers_and_the_head_starts_no_head() {
    let r = same("r1-between", 2, Knobs::default()).await;
    let worker = &r.ranks[1];
    let shim = std::fs::read_to_string(worker.node.bin_dir.join("docker")).unwrap();
    std::fs::write(worker.node.bin_dir.join("docker-real"), &shim).unwrap();
    std::fs::set_permissions(worker.node.bin_dir.join("docker-real"), std::fs::Permissions::from_mode(0o755)).unwrap();
    worker.node.bin("docker", "[ \"$1\" = run ] && sleep 1\nexec docker-real \"$@\"\n");
    r.deploy(&r.spec("r1-between", 2, 2, 1)).await;
    for _ in 0..800 {
        if !r.ran(1, "docker run -d").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    r.look_only(true);
    let d = r.wait("r1-between", "failed").await;
    assert!(d.message.starts_with(crate::admin::LOOK_ONLY) && d.message.ends_with("Its containers and files are still on cell1, cell2; Unload or Forget removes them."), "{}", d.message);
    assert!(r.ran(0, "docker run -d").is_empty(), "the head never started");
    assert_eq!(worker.standin.running(), ["sparkbox-r1-between-r1"], "the worker it couldn't stop");
    r.look_only(false);
    assert_eq!(r.unload("r1-between").await.state, TaskState::Ok);
    r.nothing_left("r1-between").await;
    r.c.live.stop();
}

/// A rank that starts and never gets anywhere: the limit is a failure like any other, with its
/// logs kept and nothing left, and a verdict Keep running won't retry.
#[tokio::test]
async fn a_start_that_hangs_reaches_its_limit_and_rolls_back() {
    let r = rig("r1-hang", vec![Knobs::default(), Knobs { start_hangs: true, ..Knobs::default() }]).await;
    r.c.models.set_ready_within(Duration::from_secs(2));
    r.deploy(&r.spec("r1-hang", 2, 2, 1)).await;
    let d = r.wait("r1-hang", "failed").await;
    assert_eq!(d.message, "vLLM didn't become ready within 2 s.");
    assert!(!d.recoverable, "a verdict");
    let f = d.failure.unwrap();
    assert_eq!(f.ranks.iter().map(|x| x.exit).collect::<Vec<_>>(), [None, None], "both still running when it gave up");
    assert!(std::fs::read_to_string(PathBuf::from(&f.log_dir).join("r1.log")).unwrap().contains("vLLM API server version"));
    r.nothing_left("r1-hang").await;
    r.c.live.stop();
}

#[tokio::test]
async fn a_worker_docker_run_that_times_out_fails_and_starts_no_head() {
    let r = rig("r1-timeout", vec![Knobs::default(), Knobs { run_hangs: true, ..Knobs::default() }]).await;
    r.c.models.set_run_limit(Duration::from_secs(1));
    r.deploy(&r.spec("r1-timeout", 2, 2, 1)).await;
    let d = r.wait("r1-timeout", "failed").await;
    assert_eq!(d.message, "cell2: docker run didn't answer within 1 s. Check Containers on cell2.");
    assert!(r.ran(0, "docker run -d").is_empty(), "the head never started");
    assert_eq!(d.failure.map(|f| f.left_on), Some(Vec::new()), "the nodes answered, so the clean-up ran");
    assert!(!r.ranks[0].node.home.join(".sparkbox/keys/r1-timeout").exists());
    r.c.live.stop();
}

#[tokio::test]
async fn a_connection_lost_mid_start_fails_and_says_where_things_are() {
    let r = rig("r1-cut", vec![Knobs::default(), Knobs { run_hangs: true, ..Knobs::default() }]).await;
    r.deploy(&r.spec("r1-cut", 2, 2, 1)).await;
    for _ in 0..400 {
        if !r.ran(1, "docker run -d").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    r.ranks[1].node.drop_connections().await;
    let d = r.wait("r1-cut", "failed").await;
    assert!(r.ran(0, "docker run -d").is_empty(), "the head never started");
    let f = d.failure.expect("a failure record");
    assert!(d.message.contains("cell2") && !d.message.is_empty(), "{}", d.message);
    // Either the clean-up reached it after the reconnect, or the message says what's there.
    assert!(f.left_on.is_empty() || d.message.ends_with("Unload or Forget removes them."), "{:?} {}", f.left_on, d.message);
    r.c.live.stop();
}

#[tokio::test]
async fn an_unload_during_a_workers_docker_run_waits_and_removes_what_it_made() {
    let r = same("r1-unl", 2, Knobs::default()).await;
    let worker = &r.ranks[1];
    let shim = std::fs::read_to_string(worker.node.bin_dir.join("docker")).unwrap();
    std::fs::write(worker.node.bin_dir.join("docker-real"), &shim).unwrap();
    std::fs::set_permissions(worker.node.bin_dir.join("docker-real"), std::fs::Permissions::from_mode(0o755)).unwrap();
    worker.node.bin("docker", "[ \"$1\" = run ] && sleep 2\nexec docker-real \"$@\"\n");
    r.deploy(&r.spec("r1-unl", 2, 2, 1)).await;
    for _ in 0..400 {
        if !r.ran(1, "docker run -d").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let task = r.c.models.unload(&r.c.ctx(), "r1-unl").await.unwrap();
    let t = finished(&r.tasks, &task).await;
    assert_eq!(t.state, TaskState::Ok, "{t:?}");
    assert_eq!(r.load("r1-unl").state, DeploymentState::Stopped);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(r.load("r1-unl").state, DeploymentState::Stopped, "its watcher stood down");
    r.nothing_left("r1-unl").await;
    r.c.live.stop();
}

/// Session 2's step 9 again, with what a KILL leaves: the rollback reports the segment through
/// its own read, in the load's message, and can't remove it.
#[tokio::test]
async fn a_rollback_names_what_a_killed_rank_left_in_dev_shm() {
    let r = rig("r1-shm", vec![Knobs { warmup_s: 300.0, ..Knobs::default() }, Knobs { join_held: true, ..Knobs::default() }]).await;
    r.deploy(&r.spec("r1-shm", 2, 2, 1)).await;
    r.until("r1-shm", "both running", |d| matches!(d.stage, Some(Stage::Joining { have: 2, .. }))).await;
    r.ranks[1].standin.kill("sparkbox-r1-shm-r1");
    let d = r.wait("r1-shm", "failed").await;
    let left = r.ranks[1].standin.shm();
    assert_eq!(left.len(), 1, "a KILL leaves its segment");
    assert!(d.message.ends_with(&format!("New in /dev/shm since the load: cell2: {}.", left[0])), "{}", d.message);
    assert!(d.failure.is_some_and(|f| f.left_on.is_empty()));
    assert!(r.ranks[0].standin.shm().is_empty(), "the head got its TERM");
    r.c.live.stop();
}

/// C-U2 answered, but its `docker rm` failed: the container is still there, and left_on says so.
#[tokio::test]
async fn a_rollback_names_the_node_whose_container_wouldnt_go() {
    let r = rig("r1-stuck", vec![Knobs { warmup_s: 300.0, ..Knobs::default() }, Knobs { rank_dies_after_s: Some(0.5), ..Knobs::default() }]).await;
    let worker = &r.ranks[1];
    let shim = std::fs::read_to_string(worker.node.bin_dir.join("docker")).unwrap();
    std::fs::write(worker.node.bin_dir.join("docker-real"), &shim).unwrap();
    std::fs::set_permissions(worker.node.bin_dir.join("docker-real"), std::fs::Permissions::from_mode(0o755)).unwrap();
    worker.node.bin("docker", "[ \"$1\" = rm ] && { echo 'Error response from daemon: removal of container is already in progress' >&2; exit 1; }\nexec docker-real \"$@\"\n");
    r.deploy(&r.spec("r1-stuck", 2, 2, 1)).await;
    let d = r.wait("r1-stuck", "failed").await;
    assert_eq!(d.failure.as_ref().unwrap().left_on, ["cell2"]);
    assert!(d.message.ends_with("Its containers and files are still on cell2; Unload or Forget removes them."), "{}", d.message);
    r.c.live.stop();
}

/// A key in a log is hidden on every way out of it: the saved logs, the load's tail while it
/// starts, its message, its failure record and deployments.json. vLLM isn't known to print it.
#[tokio::test]
async fn a_log_that_prints_the_key_is_masked_wherever_it_goes() {
    let r = same("r1-logkey", 1, Knobs { prints_key: true, warmup_s: 300.0, rank_dies_after_s: Some(1.5), ..Knobs::default() }).await;
    r.deploy(&r.spec("r1-logkey", 1, 1, 1)).await;
    let key = secrets::get(&plan::key_account("r1-logkey")).unwrap().expect("its key");
    let name = "sparkbox-r1-logkey";
    let d = r.until("r1-logkey", "its log read", |d| d.log_tail.contains("using key")).await;
    assert!(r.ranks[0].standin.log(name).contains(&key), "the control: the log really has it");
    assert!(!d.log_tail.contains(&key) && d.log_tail.contains("{'api_key': ['‹redacted›'], "), "{}", d.log_tail);

    let d = r.wait("r1-logkey", "failed").await;
    let f = d.failure.clone().unwrap();
    assert!(d.message.contains("the key was ‹redacted›"), "{}", d.message);
    assert!(f.cause.contains("the key was ‹redacted›") && f.ranks[0].tail.contains("using key ‹redacted›"), "{f:?}");
    let saved = std::fs::read_to_string(PathBuf::from(&f.log_dir).join("r0.log")).unwrap();
    assert!(saved.contains("using key ‹redacted›"), "{saved}");
    for (what, text) in [("the message", d.message.clone()), ("the tail", d.log_tail.clone()), ("the record", format!("{f:?}")), ("the saved log", saved), ("deployments.json", std::fs::read_to_string(r.c.dir.join("deployments.json")).unwrap())] {
        assert!(!text.contains(&key), "{what}: {text}");
    }
    r.c.live.stop();
}

/// Docker's words go through the same mask: they can quote what they were given.
#[tokio::test]
async fn what_docker_run_said_is_masked_too() {
    let key = "sbk-0123456789abcdef0123456789abcdef";
    let r = same("r1-saidkey", 1, Knobs { run_fails: Some(format!("Error response from daemon: invalid {key}")), ..Knobs::default() }).await;
    r.deploy(&r.spec("r1-saidkey", 1, 1, 1)).await;
    let d = r.wait("r1-saidkey", "failed").await;
    let f = d.failure.clone().unwrap();
    assert_eq!(d.message, "cell1: docker run failed: docker: Error response from daemon: invalid ‹redacted›");
    assert_eq!(f.cause, d.message);
    let saved = std::fs::read_to_string(PathBuf::from(&f.log_dir).join("r0.log")).unwrap();
    assert!(!saved.contains(key) && !format!("{f:?}").contains(key) && !d.log_tail.contains(key), "{saved} {f:?}");
    r.c.live.stop();
}

/// An unload compares with what the node had before the last start, not the first.
#[tokio::test]
async fn an_unload_after_a_restart_checks_against_the_restarts_baseline() {
    let r = same("r1-base", 1, Knobs::default()).await;
    r.ready(&r.spec("r1-base", 1, 1, 1)).await;
    // Something else on the node took 20 GiB meanwhile: not this load's to give back.
    let busier = Knobs { mem_free: 99 << 30, mem_available: 100 << 30, ..Knobs::default() };
    r.ranks[0].standin.set_knobs(busier);
    r.c.models.restart(&r.c.ctx(), "r1-base").await.unwrap();
    let d = r.until("r1-base", "restarted", |d| d.generation == 1 && d.state == DeploymentState::Ready).await;
    assert_eq!(d.baseline.iter().map(|b| b.mem_available).collect::<Vec<_>>(), [100 << 30], "from the read after its stop");
    let t = r.unload("r1-base").await;
    let report = r.load("r1-base").unload_report.unwrap();
    assert!(report.ok, "{report:?}");
    assert!(report.summary.contains("memory back (100.0 GiB available, baseline 100.0)"), "{}", report.summary);
    assert_eq!(t.state, TaskState::Ok, "{t:?}");
    r.c.live.stop();
}

/// An Unload that gets the lock first: the start it overtook sends nothing.
#[tokio::test]
async fn an_unload_ahead_of_its_start_leaves_nothing_to_start() {
    let r = same("r1-ahead", 1, Knobs::default()).await;
    let spec = r.spec("r1-ahead", 1, 1, 1);
    let held = r.c.models.start_lock("r1-ahead").lock_owned().await;
    // In line for the lock before the load even exists, so it's ahead of the load's own start.
    let (models, ctx) = (r.c.models.clone(), r.c.ctx());
    let unload = tokio::spawn(async move { models.unload(&ctx, "r1-ahead").await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    r.deploy(&spec).await;
    assert_eq!(r.load("r1-ahead").state, DeploymentState::Starting);
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(held);

    let task = unload.await.unwrap().unwrap();
    assert_eq!(finished(&r.tasks, &task).await.state, TaskState::Ok);
    assert_eq!(r.load("r1-ahead").state, DeploymentState::Stopped);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(r.ran(0, "docker run").is_empty() && r.ran(0, "deploy-files").is_empty(), "nothing started");
    assert_eq!(r.load("r1-ahead").state, DeploymentState::Stopped);
    r.c.live.stop();
}

/// A worker's node that doesn't answer isn't a worker that stopped: the load goes on and the rank
/// counts. Docker's inspect takes the shell down with it, so the channel ends with no exit status.
#[tokio::test]
async fn a_worker_that_cant_be_asked_counts_as_running() {
    let r = rig("r1-noanswer", vec![Knobs::default(), Knobs { join_held: true, ..Knobs::default() }]).await;
    let worker = &r.ranks[1];
    let shim = std::fs::read_to_string(worker.node.bin_dir.join("docker")).unwrap();
    std::fs::write(worker.node.bin_dir.join("docker-real"), &shim).unwrap();
    std::fs::set_permissions(worker.node.bin_dir.join("docker-real"), std::fs::Permissions::from_mode(0o755)).unwrap();
    worker.node.bin("docker", "[ \"$1\" = inspect ] && kill -KILL $PPID\nexec docker-real \"$@\"\n");
    r.deploy(&r.spec("r1-noanswer", 2, 2, 1)).await;
    let d = r.until("r1-noanswer", "both ranks counted", |d| d.stage == Some(Stage::Joining { have: 2, need: 2 })).await;
    assert_eq!(d.state, DeploymentState::Starting);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let d = r.load("r1-noanswer");
    assert_eq!((d.state, d.failure), (DeploymentState::Starting, None), "{}", d.message);

    // It really is there: once it answers and joins, the load is Ready.
    worker.node.bin("docker", "exec docker-real \"$@\"\n");
    worker.standin.let_join();
    r.wait("r1-noanswer", "ready").await;
    r.unload("r1-noanswer").await;
    r.c.live.stop();
}

/// launch.py is read for every load, whatever this connection checked before, and a copy is a row
/// of the log. A legacy build writes a separate filename; corrupted bytes at our own path are repaired.
#[tokio::test]
async fn launch_py_is_checked_at_every_start_and_a_copy_has_its_row() {
    let r = same("r1-launch", 1, Knobs::default()).await;
    r.ready(&r.spec("r1-launch", 1, 1, 1)).await;
    let copies = || r.c.live.audit.entries().into_iter().filter(|e| e.title == "Copy launch.py to cell1").collect::<Vec<_>>();
    let rows = copies();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].changes && rows[0].command.starts_with(&format!("~/.sparkbox/{} sha256 ", plan::launcher_name())), "{:?}", rows[0]);
    assert!(rows[0].op_id.is_some(), "under the load's operation");

    // An older build can replace its shared launch.py without changing this load's launcher.
    let legacy = r.ranks[0].node.home.join(".sparkbox/launch.py");
    std::fs::write(&legacy, "import sys; sys.exit(3)\n").unwrap();
    assert_eq!(r.restarted("r1-launch").await.state, TaskState::Ok);
    assert_eq!(copies().len(), 1);
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "import sys; sys.exit(3)\n");

    // Corruption at this build's path: put back before this restart's run.
    let file = r.ranks[0].node.home.join(format!(".sparkbox/{}", plan::launcher_name()));
    std::fs::write(&file, "import sys; sys.exit(3)\n").unwrap();
    assert_eq!(r.restarted("r1-launch").await.state, TaskState::Ok);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), plan::LAUNCH);
    assert_eq!(copies().len(), 2);
    r.unload("r1-launch").await;
    r.c.live.stop();
}

/// Recipes are vLLM's. SGLang's load has neither a recipe nor an image id to verify.
#[tokio::test]
async fn an_sglang_load_verifies_no_recipe() {
    let answers = "running 0\n200\nINFO Application startup complete.\n";
    let engine = vllm_script(Arc::new(Mutex::new(vec![answers])), 1);
    // SGLang has no key: its model list answers without one.
    let served = "{\"object\":\"list\",\"data\":[{\"id\":\"qwen3.8-27b\",\"object\":\"model\",\"max_model_len\":32768}]}\n200";
    let script: Script = Arc::new(move |cmd, stdin| if cmd.starts_with("curl ") && cmd.contains("/v1/models") { Some((served.to_string(), 0)) } else { engine(cmd, stdin) });
    let (_node, port) = crate::fake_node::fake_node(&temp("r1-sglang-1"), script).await;
    let c = cluster("r1-sglang", &[("cell1", port)]).await;
    let s = DeploySpec { id: "r1-sglang".into(), engine: "sglang".into(), image: "lmsysorg/sglang:spark".into(), require_key: false, ..spec(&["cell1"], 1, 1) };
    c.models.deploy(&c.ctx(), s).await.unwrap();
    let d = wait_state(&c, "r1-sglang", "ready").await;
    assert_eq!(d.stage, Some(Stage::Ready));
    assert!(c.models.recipes().iter().all(|x| x.verified_on.is_empty()), "{:?}", c.models.recipes());
    assert!(!c.dir.join("recipes-verified.json").exists());
    c.live.stop();
}

/// A reload under the same id starts over, and the first load's watcher, asleep through it all,
/// wakes while the new container isn't there yet: it stands down, and doesn't fail the new load.
#[tokio::test]
async fn a_reload_under_the_same_id_isnt_failed_by_the_old_watcher() {
    let r = same("r1-reload", 1, Knobs::default()).await;
    let spec = r.spec("r1-reload", 1, 1, 1);
    r.ready(&spec).await;
    r.unload("r1-reload").await;
    let node = &r.ranks[0].node;
    let shim = std::fs::read_to_string(node.bin_dir.join("docker")).unwrap();
    std::fs::write(node.bin_dir.join("docker-real"), &shim).unwrap();
    std::fs::set_permissions(node.bin_dir.join("docker-real"), std::fs::Permissions::from_mode(0o755)).unwrap();
    // Longer than the serving watcher's sleep: it wakes with nothing on the node.
    node.bin("docker", "[ \"$1\" = run ] && sleep 3\nexec docker-real \"$@\"\n");
    r.deploy(&spec).await;
    let d = r.wait("r1-reload", "ready").await;
    assert_eq!((d.generation, d.failure), (1, None));
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-reload"]);
    r.unload("r1-reload").await;
    r.c.live.stop();
}

/// Quit and start again with a load still starting: the watcher finds it on its rank names, and
/// a failure then is rolled back like any other.
#[tokio::test]
async fn quit_and_resume_with_a_starting_load_follows_it_on_rank_names() {
    let r = same("r1-resume", 2, Knobs { warmup_s: 1.5, ..Knobs::default() }).await;
    let spec = r.spec("r1-resume", 2, 2, 1);
    r.deploy(&spec).await;
    r.until("r1-resume", "both up", |d| matches!(d.stage, Some(Stage::Joining { .. }) | Some(Stage::Loading { .. }))).await;
    let saved = std::fs::read_to_string(r.c.dir.join("deployments.json")).unwrap();
    assert_eq!(serde_json::from_str::<Vec<Deployment>>(&saved).unwrap()[0].state, DeploymentState::Starting);
    let ports: Vec<(&str, u16)> = r.c.inventory.nodes.iter().map(|n| (IDS[if n.id == "cell1" { 0 } else { 1 }], n.port)).collect();
    r.c.live.stop();

    let dir = temp("r1-resume-again");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("deployments.json"), &saved).unwrap();
    let (live, inventory) = crate::fake_node::connect(&dir, &ports).await;
    let llm = Llm::new(dir.join("providers.json"), Vpn::new(dir.join("vpn2.json"), dir.join("no-helper"), |_| {}));
    let models = Models::new(dir.clone(), live.clone(), llm.clone(), None);
    let again = Cluster { models, live, llm, inventory, dir };
    again.models.resume(&ModelSettings::default());
    again.models.resume(&ModelSettings::default());
    assert_eq!(again.models.resuming.lock().unwrap().len(), 1, "one observer per saved generation");
    let found = || again.models.deployments(&again.ctx()).into_iter().find(|d| d.spec.id == "r1-resume");
    for _ in 0..1200 {
        if found().is_some_and(|d| d.state == DeploymentState::Ready) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let d = found().unwrap();
    assert_eq!((d.state, d.stage), (DeploymentState::Ready, Some(Stage::Ready)), "{}", d.message);
    assert!(again.llm.get("vllm-r1-resume").await.is_some());
    assert_eq!(r.ranks[0].standin.running(), ["sparkbox-r1-resume-r0"]);
    again.live.stop();
}

/// SparkBox quit between the workers and the head: the next start finds a head that never came,
/// and offers cleanup without launching a duplicate head or stopping workers by itself.
#[tokio::test]
async fn a_quit_between_the_workers_and_the_head_can_resume_only_the_missing_head() {
    let r = same("r1-quit", 2, Knobs::default()).await;
    let spec = DeploySpec { require_key: false, image: GLM53_IMAGE.into(), host:r.c.ctx().lan_address("cell1"), image_id: fake_vllm::test_image_id(), ..r.spec("r1-quit", 2, 2, 1) };
    let net = |id: &str, n: u8| plan::NodeNet { node_id: id.into(), iface: "enp1s0f1np1".into(), ip: format!("192.168.100.{n}"), hcas: vec!["rocep1s0f1".into()] };
    let nets = [net("cell1", 1), net("cell2", 2)];
    fake_vllm::launch_files(&r.c.live, "cell2", &spec, 1, &nets, None).await;
    assert_eq!(r.c.live.run("cell2", &plan::rank_command(&spec, 1), None, Duration::from_secs(30)).await.unwrap().code, Some(0));
    assert_eq!(r.ranks[1].standin.running(), ["sparkbox-r1-quit-r1"]);
    let saved = Deployment { spec, state: DeploymentState::Starting, first_start: true, started_at: now(), ..Default::default() };
    r.c.models.deployments.lock().unwrap().push(saved);

    r.c.models.resume(&ModelSettings::default());
    let d = r.wait("r1-quit", "failed").await;
    assert_eq!(d.recovery, Some(recovery::Recovery::Cleanup));
    assert!(d.message.contains("no owned container on cell1"), "{}", d.message);
    assert_eq!(r.ranks[1].standin.running(), ["sparkbox-r1-quit-r1"], "no automatic cleanup");
    assert!(r.ran(0, "docker run -d").is_empty(), "no duplicate head launch");
    let worker_started = r.ranks[1].standin.started_ms("sparkbox-r1-quit-r1");
    r.c.models.resume_partial(&r.c.ctx(), "r1-quit", false).await.unwrap();
    r.wait("r1-quit", "ready").await;
    assert_eq!(r.ranks[1].standin.started_ms("sparkbox-r1-quit-r1"), worker_started);
    assert_eq!(r.ran(0, "docker run -d").len(), 1);
    assert_eq!(r.ran(1, "docker run -d").len(), 1, "the original worker is retained");
    let task = r.unload("r1-quit").await;
    assert_eq!(task.state, TaskState::Ok);
    r.nothing_left("r1-quit").await;
    assert!(d.failure.is_none(), "reconciliation reports missing membership without claiming it ran a rollback");
    r.c.live.stop();
}

#[tokio::test]
async fn maintenance_pause_holds_claims_keeps_the_key_and_restores_the_whole_load() {
    let r = same("p2-maintenance", 2, Knobs::default()).await;
    let spec = r.spec("p2-maintenance", 2, 2, 1);
    r.ready(&spec).await;
    let key = secrets::get(&plan::key_account(&spec.id)).unwrap(); assert!(key.is_some());
    r.c.models.pause_for_maintenance(&r.c.ctx(), &spec.id).await.unwrap();
    assert_eq!(r.load(&spec.id).recovery, Some(recovery::Recovery::Maintenance));
    assert!(r.ranks.iter().all(|n| n.standin.running().is_empty()));
    assert!(claim(&r.c.models.deployments(&r.c.ctx()), &r.spec("another",1,1,1)).is_err());
    assert!(secrets::get(&plan::key_account(&spec.id)).unwrap() == key);
    // An app restart's observer must leave the durable maintenance hold alone.
    r.c.models.resume(&ModelSettings::default());
    assert_eq!(r.load(&spec.id).recovery, Some(recovery::Recovery::Maintenance));
    let (models, ctx, id) = (r.c.models.clone(), r.c.ctx(), spec.id.clone());
    let restore = tokio::spawn(async move { models.restore_after_maintenance(&ctx, &id).await });
    for _ in 0..1200 { if restore.is_finished() { break; } r.c.models.endpoints_tick(&r.c.ctx(), true).await; tokio::time::sleep(Duration::from_millis(50)).await; }
    assert!(restore.is_finished(), "restore should finish once the load answers");
    restore.await.unwrap().unwrap(); assert_eq!(r.load(&spec.id).state,DeploymentState::Ready);
    assert!(secrets::get(&plan::key_account(&spec.id)).unwrap() == key);
    r.unload(&spec.id).await; r.c.live.stop();
}

// ---- What's kept

#[tokio::test]
async fn checks_preflights_and_verified_recipes_survive_a_restart() {
    let r = same("r1-kept", 2, Knobs::default()).await;
    let spec = r.spec("r1-kept", 2, 2, 1);
    assert!(r.c.models.recipes().iter().all(|x| x.verified_on.is_empty()), "nothing is verified yet");
    let set = r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    assert!(set.checked_at_ms > 0);
    let d = r.ready(&spec).await;

    let saved: BTreeMap<String, plan::PreflightSet> = load(&r.c.dir.join("preflight.json"));
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved.values().next().unwrap().nodes.len(), 2);
    let checks: Vec<NodeCheck> = load(&r.c.dir.join("checks.json"));
    assert_eq!(checks.iter().map(|c| (c.node.as_str(), c.fabric.len())).collect::<Vec<_>>(), [("cell1", 2), ("cell2", 2)]);
    assert!(checks.iter().all(|c| c.checked_at_ms > 0));

    // Only passing readiness marks a recipe verified, with what it passed on.
    let view = || r.c.models.recipes().into_iter().find(|x| x.id == "gemma-3-27b-it").unwrap();
    r.until("r1-kept", "verified", |_| !view().verified_on.is_empty()).await;
    let view = view();
    assert_eq!(view.verified_on.iter().map(|v| (v.image_id.as_str(), v.topology.as_str(), v.node_count)).collect::<Vec<_>>(), [(d.spec.image_id.as_str(), "tp2", 2)]);
    let p = r.c.models.plan(&r.c.ctx(), &spec).await.unwrap();
    assert_eq!(p.recipe.map(|x| x.verified_on.len()), Some(1));
    r.unload("r1-kept").await;

    // A new SparkBox has them, and the preview its fabric addresses with no check of its own.
    let models = Models::new(r.c.dir.clone(), r.c.live.clone(), r.c.llm.clone(), None);
    assert_eq!(models.recipes().into_iter().find(|x| x.id == "gemma-3-27b-it").unwrap().verified_on.len(), 1);
    let steps = models.preview(&r.c.ctx(), &spec).unwrap();
    let files = steps.iter().find(|s| s.title == "Write the launch files for the worker (rank 1)").expect("the worker's files");
    assert!(files.command.contains("192.168.100.1") && files.command.contains("192.168.100.2"), "{}", files.command);
    r.c.live.stop();
}

#[tokio::test]
async fn the_preview_shows_each_rank_workers_first_with_the_key_masked_and_no_ray() {
    let r = same("r1-preview", 3, Knobs::default()).await;
    let spec = r.spec("r1-preview", 3, 1, 3);
    r.c.models.preflight(&r.c.ctx(), &spec).await.unwrap();
    let steps = r.c.models.preview(&r.c.ctx(), &spec).unwrap();
    let titles: Vec<&str> = steps.iter().map(|s| s.title.as_str()).filter(|t| t.starts_with("Start ") || t.starts_with("Write ")).collect();
    assert_eq!(
        titles,
        ["Write the launch files for the worker (rank 1)", "Start the worker (rank 1)", "Write the launch files for the worker (rank 2)", "Start the worker (rank 2)", "Write the launch files for the head (rank 0)", "Start the head (rank 0)"]
    );
    let head = steps.iter().find(|s| s.title == "Write the launch files for the head (rank 0)").unwrap();
    assert!(head.command.contains("sbk-••••••••"), "{}", head.command);
    let worker = steps.iter().find(|s| s.title == "Write the launch files for the worker (rank 1)").unwrap();
    assert!(!worker.command.contains("sbk-"), "{}", worker.command);
    assert_eq!(steps.iter().filter(|s| s.title.starts_with("Test the jumbo-frame path")).count(), 2);
    let joined = steps.iter().map(|s| format!("{} {}", s.title, s.command)).collect::<Vec<_>>().join("\n");
    assert!(!joined.contains("Ray ") && !joined.contains("ray-") && !joined.contains("--distributed-executor-backend"), "{joined}");
    r.c.live.stop();
}

#[tokio::test]
async fn demo_preflights_say_demo_and_new_functions_need_live_data() {
    let (models, _llm, live, cluster, _dir) = demo_app("r1-demo");
    qwen_everywhere(&models, &cluster, &["cell1", "cell2"]).await;
    let ctx = demo_ctx(&cluster);
    let spec = DeploySpec { gpu_memory_utilization: 0.5, ..spec(&["cell1", "cell2"], 2, 1) };
    let before = live.audit.entries().len();
    let set = models.preflight(&ctx, &spec).await.unwrap();
    assert_eq!((set.verdict.as_str(), set.nodes.len()), ("demo", 2));
    assert!(set.nodes.iter().all(|p| p.demo));
    assert_eq!(live.audit.entries().len(), before, "Demo read nothing");
    assert_eq!(models.clean_leftovers(&ctx, vec!["cell1".into()], vec!["x".into()]).await.unwrap_err(), "Available with Live data");
    let steps = models.preview(&ctx, &spec).unwrap();
    assert!(steps.iter().any(|s| s.title == "Start the worker (rank 1)"), "{steps:?}");
    let d = models.deploy(&ctx, spec).await.expect("a Demo load still simulates");
    assert!(d.demo);
}

/// Keep running after a restart that failed, on real stand-in ranks. Rollback took every
/// container, so all that's left is the load's own Failed record: it tries again on its back-off,
/// with Live data, and that restart brings the load back.
#[tokio::test]
async fn keep_running_tries_again_after_a_restart_that_left_no_containers() {
    let r = same("r1-retry", 2, Knobs::default()).await;
    let spec = DeploySpec { keep_running: true, ..r.spec("r1-retry", 2, 2, 1) };
    r.ready(&spec).await;

    // The next start of the head has nowhere to meet its worker: it exits, and the restart fails.
    let good = r.ranks[0].standin.knobs();
    r.ranks[0].standin.set_knobs(Knobs { rendezvous: None, ..good.clone() });
    r.c.models.restart(&r.c.ctx(), "r1-retry").await.unwrap();
    let d = r.until("r1-retry", "failed", |d| d.state == DeploymentState::Failed && d.generation == 1).await;
    assert!(d.recoverable && d.failure.as_ref().is_some_and(|f| f.left_on.is_empty()), "{:?}", d.failure);
    r.nothing_left("r1-retry").await;
    assert_eq!(r.ran(0, "docker run -d").len(), 2);

    // Its containers are gone, and the card says it will try again.
    let mut scheduled = None;
    for _ in 0..100 {
        let eps = r.c.models.endpoints_tick(&r.c.ctx(), true).await;
        let e = eps.into_iter().find(|e| e.deployment_id.as_deref() == Some("r1-retry")).expect("listed");
        assert!(e.members.is_empty() && e.health == endpoints::Health::Down, "{:?} {:?}", e.health, e.health_reason);
        let s = e.supervisor.clone().expect("Keep running is on");
        assert_eq!(s.held, None, "{s:?}");
        if s.next_retry_ms.is_some() {
            scheduled = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let s = scheduled.expect("a retry is scheduled");
    assert_eq!((s.state, s.restarts), (endpoints::SupervisorState::Restarting, 0));
    assert_eq!(r.ran(0, "docker run -d").len(), 2, "it waits first");

    // The failed restart's Verify step ends, as it does well inside the 30 s.
    for _ in 0..300 {
        if r.c.models.restarting.lock().unwrap().is_empty() {
            break;
        }
        r.c.models.endpoints_tick(&r.c.ctx(), true).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(r.c.models.restarting.lock().unwrap().is_empty(), "the restart never ended");

    // The cause is fixed, and the retry comes due (the 30 s are the test's to skip).
    r.ranks[0].standin.set_knobs(good);
    r.c.models.eps.lock().unwrap().supervisors.get_mut("r1-retry").unwrap().next_retry_ms = Some(0);
    r.c.models.endpoints_tick(&r.c.ctx(), true).await;
    let d = r.until("r1-retry", "ready again", |d| d.state == DeploymentState::Ready && d.generation == 2).await;
    assert!(d.recoverable);
    assert_eq!((r.ran(0, "docker run -d").len(), r.ran(1, "docker run -d").len()), (3, 3));
    let tasks = r.tasks.list();
    let t = tasks.iter().find(|t| t.title == "Keep running: restart gemma-3-27b-it in order (1 of 5)").unwrap_or_else(|| panic!("{tasks:?}"));
    assert_eq!(t.source, "system");
    r.c.live.stop();
}
