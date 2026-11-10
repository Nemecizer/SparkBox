//! Container actions, one or many at once. A bulk action is one task with a step per
//! container, run in the order given (the UI sorts a deployment's containers so workers stop
//! before their head and start after it). Every docker command goes through `Live::run`, so
//! it's in the activity log.
//!
//! Containers › Run test container starts SparkBox's test container on one node (C7): busybox,
//! named sparkbox-test-busybox and labelled sparkbox.test=1 and this build's sparkbox.instance,
//! as the cluster user (docker pulls busybox:1.36 if the node hasn't got it), under the fence
//! class container-test. Rollback: Stop, then Remove, in Containers (`docker stop`, `docker rm`;
//! or by the label sparkbox.test=1).

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tauri::{AppHandle, State};

use super::Ops;
use crate::errors::{ErrorKind, SbError};
use crate::live::audit::{self, OpMeta};
use crate::live::exit::as_class;
use crate::live::fence_class::FenceClass;
use crate::tasks::TaskSpec;
use crate::{admin, is_live, AppState};

pub const ACTIONS: &[&str] = &["start", "stop", "restart", "kill", "pause", "unpause", "remove"];

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub node: String,
    pub id: String,
    /// For the task's steps and the log; the id is what's acted on.
    #[serde(default)]
    pub name: String,
}

pub fn id_ok(id: &str) -> bool {
    (12..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// The docker command for an action.
pub fn docker_command(action: &str, id: &str, remove_volumes: bool) -> Result<String, SbError> {
    if !ACTIONS.contains(&action) {
        return Err(SbError::invalid(format!("unsupported container action {action}")));
    }
    if !id_ok(id) {
        return Err(SbError::invalid("bad container id"));
    }
    Ok(match action {
        "remove" if remove_volumes => format!("docker rm -v {id}"),
        "remove" => format!("docker rm {id}"),
        a => format!("docker {a} {id}"),
    })
}

/// SparkBox's test container: one per node and build.
pub const TEST_CONTAINER: &str = "sparkbox-test-busybox";

/// What Run test container sends (C7), for `instance` ("dev" or "release").
pub fn test_container_command(instance: &str) -> String {
    format!("docker run -d --name {TEST_CONTAINER} --label sparkbox.test=1 --label sparkbox.instance={instance} busybox:1.36 sleep 3600")
}

pub fn verb(action: &str) -> &'static str {
    match action {
        "start" => "Start",
        "stop" => "Stop",
        "restart" => "Restart",
        "kill" => "Force-stop",
        "pause" => "Pause",
        "unpause" => "Resume",
        "remove" => "Remove",
        _ => "Change",
    }
}

fn past(action: &str) -> &'static str {
    match action {
        "start" => "started",
        "stop" => "stopped",
        "restart" => "restarted",
        "kill" => "force-stopped",
        "pause" => "paused",
        "unpause" => "resumed",
        "remove" => "removed",
        _ => "changed",
    }
}

impl Ops {
    /// Name and state of a container from the snapshot.
    fn container_view(&self, node: &str, id: &str) -> Option<(String, String)> {
        let n = self.node_view(node)?;
        n.containers.into_iter().find(|c| c.id == id || id.starts_with(&c.id) || c.id.starts_with(id)).map(|c| (c.name, c.state))
    }

    /// One action on one container, inside the caller's task and op.
    async fn container_step(&self, demo: bool, node: &str, id: &str, action: &str, remove_volumes: bool, title: &str) -> Result<(), SbError> {
        let cmd = docker_command(action, id, remove_volumes)?;
        if action == "remove" {
            if let Some((name, state)) = self.container_view(node, id) {
                if !matches!(state.as_str(), "exited" | "created" | "dead") {
                    return Err(SbError::invalid(format!("{name} is {state}; stop it before removing it.")));
                }
            }
        }
        if demo {
            let result = self.demo.lock().unwrap().container_action(node, id, action);
            if let Err(e) = result {
                return Err(self.simulated_failed(node, title, &cmd, &e));
            }
            self.simulated(node, title, &cmd);
            return Ok(());
        }
        let (code, _, err) = self.exec(node, &cmd, false, Duration::from_secs(90)).await?;
        if code == Some(0) {
            return Ok(());
        }
        // Not in the docker group: try again as root.
        if err.contains("permission denied") && err.contains("docker") {
            let (code, _, err) = self.exec(node, &cmd, true, Duration::from_secs(90)).await?;
            if code == Some(0) {
                return Ok(());
            }
            return Err(SbError::failed(format!("docker {action} failed on {node}: {}", err.trim())).on(node));
        }
        let kind = if err.contains("No such container") { ErrorKind::Gone } else { ErrorKind::Failed };
        Err(SbError::new(kind, format!("docker {action} failed on {node}: {}", err.trim())).on(node).with_detail(err))
    }

    pub async fn container_action(&self, demo: bool, node: &str, id: &str, action: &str, remove_volumes: bool) -> Result<(), SbError> {
        docker_command(action, id, remove_volumes)?;
        let name = self.container_view(node, id).map(|(n, _)| n).unwrap_or_else(|| id.chars().take(12).collect());
        let title = format!("{} {name} on {}", verb(action), self.name_of(node));
        let t = title.clone();
        self.one("container", title, node, async move {
            self.container_step(demo, node, id, action, remove_volumes, &t).await?;
            Ok(((), format!("{name} {}", past(action))))
        })
        .await
    }

    /// One action on several containers as one task (a step each, in the given order).
    /// Keeps going past a failure and reports it at the end. Returns the task's id.
    pub fn containers_bulk(self: &Arc<Self>, demo: bool, items: Vec<Target>, action: String, remove_volumes: bool, title: Option<String>) -> Result<String, SbError> {
        if items.is_empty() {
            return Err(SbError::invalid("Select at least one container."));
        }
        for it in &items {
            docker_command(&action, &it.id, remove_volumes)?;
        }
        let mut nodes: Vec<String> = Vec::new();
        for it in &items {
            if !nodes.contains(&it.node) {
                nodes.push(it.node.clone());
            }
        }
        let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
            format!("{} {} container{}", verb(&action), items.len(), if items.len() == 1 { "" } else { "s" })
        });
        let task = self.tasks.start(TaskSpec::new("container", title.clone(), &nodes).cancellable());
        task.set_retry("containers_bulk", json!({ "items": items, "action": action, "removeVolumes": remove_volumes, "title": title }));
        let id = task.id().to_string();
        let me = self.clone();
        tauri::async_runtime::spawn(audit::with_op_id(id.clone(), OpMeta { title: title.clone(), source: "ui".into() }, async move {
            let head = me.live.audit.begin_op(&nodes.join(", "), &title, "", demo);
            let total = items.len();
            let (mut ok, mut failed) = (0, Vec::new());
            let mut done: Vec<(String, String)> = Vec::new();
            let mut names: Vec<(String, String)> = Vec::new();
            for (i, it) in items.iter().enumerate() {
                let name = if it.name.is_empty() { it.id.chars().take(12).collect() } else { it.name.clone() };
                let node_name = me.name_of(&it.node);
                if task.is_cancelled() {
                    for rest in &items[i..] {
                        task.step_skipped(format!("{} on {}", rest.name, me.name_of(&rest.node)), "Stopped before this one");
                    }
                    break;
                }
                task.step(format!("{} {name} on {node_name}", verb(&action)));
                match me.container_step(demo, &it.node, &it.id, &action, remove_volumes, &format!("{} {name} on {node_name}", verb(&action))).await {
                    Ok(()) => {
                        ok += 1;
                        done.push((it.node.clone(), it.id.clone()));
                        names.push((it.node.clone(), name.clone()));
                        task.step_ok(None);
                    }
                    Err(e) => {
                        task.step_failed(e.message.clone());
                        failed.push(format!("{name}: {}", e.message));
                    }
                }
                task.progress((i + 1) as f32 / total as f32, None);
            }
            let mut summary = format!("{ok} of {total} {}", past(&action));
            // Restarting or starting what serves a model isn't done until the model answers.
            let mut warning = None;
            if matches!(action.as_str(), "restart" | "start") && !done.is_empty() && !task.is_cancelled() {
                let what = if action == "restart" { "Restarted" } else { "Started" };
                match me.verify(&task, crate::models::verify::Scope::Containers(done), what).await {
                    Ok(Some(ready)) => {
                        // Verified: a crash-loop alert on these containers is over now, not
                        // once its window has aged out.
                        if let Some(p) = me.planned() {
                            for (node, name) in &names {
                                p.resolved("crash-loop", &format!("crash-loop:container:{node}:{name}"), &format!("Verified: {ready}"));
                            }
                        }
                        summary = format!("{summary}. {ready}");
                    }
                    Ok(None) => {}
                    Err(w) => warning = Some(w),
                }
            }
            if task.is_cancelled() {
                me.end_head(&head, &Ok(summary.clone()));
                task.finish_cancelled(summary);
            } else if let (Some(w), true) = (&warning, failed.is_empty()) {
                me.live.audit.end(&head, audit::Ending { stdout_tail: Some(summary.clone()), error: Some(w.clone()), ..Default::default() });
                task.finish_warn(summary, w.clone());
            } else if failed.is_empty() {
                me.end_head(&head, &Ok(summary.clone()));
                task.finish_ok(summary);
            } else {
                let error = format!("{summary}. {}", failed.join("; "));
                me.end_head(&head, &Err(SbError::failed(error.clone())));
                task.finish_err(error);
            }
        }));
        Ok(id)
    }
}

impl Ops {
    /// Starts the test container on `node` (C7) as one titled task: as the cluster user, under
    /// container-test, never with sudo. Returns its id. One already there (by its name, running or
    /// not) is left alone: "remove it first".
    pub async fn container_test_run(&self, demo: bool, node: &str) -> Result<String, SbError> {
        if demo {
            return Err(SbError::new(ErrorKind::DemoOnly, crate::files::LIVE_ONLY).on(node));
        }
        let name = self.name_of(node);
        let cmd = test_container_command(crate::identity::instance_name());
        self.one("container", format!("Run the test container on {name}"), node, async {
            // It may pull busybox:1.36 first.
            let (code, out, err) = as_class(FenceClass::ContainerTest, self.exec(node, &cmd, false, Duration::from_secs(300))).await?;
            if code == Some(0) {
                return Ok((out.trim().to_string(), format!("{TEST_CONTAINER} started")));
            }
            if err.contains("is already in use") {
                return Err(SbError::invalid(format!("A test container is already on {name}; remove it first.")).on(node));
            }
            Err(SbError::failed(format!("docker run failed on {name}: {}", err.trim())).on(node).with_detail(err))
        })
        .await
    }
}

/// Containers › Run test container (Live, one node): node scope, as every container action.
#[tauri::command]
pub async fn container_test_run(app: AppHandle, state: State<'_, AppState>, node: String) -> Result<String, SbError> {
    admin::require_sb(&app)?;
    let demo = !is_live(&state);
    if demo {
        return Err(SbError::new(ErrorKind::DemoOnly, crate::files::LIVE_ONLY).on(&node));
    }
    state.ops.container_test_run(demo, &node).await
}

// ---- Inspect: secrets masked before they leave the backend

/// Environment variable names whose values are secrets: API_KEY, HF_TOKEN, DB_PASSWORD… but not
/// MTP_TOKENS (a count), MAX_NUM_BATCHED_TOKENS or a KEY_* setting.
pub fn secret_name(name: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::RegexBuilder::new(r"api[_-]?key|secret|passw|(^|_)pass$|token$|hf_token|bearer|credential|private[_-]?key|access[_-]?key").case_insensitive(true).build().unwrap()
    })
    .is_match(name)
}

/// A value that can't be a secret whatever its name: empty, a number, or a yes/no word. Anything
/// else under a secret name stays hidden, short ones and paths too (they fail closed).
fn plainly_not_secret(value: &str) -> bool {
    let v = value.trim();
    v.is_empty() || v.chars().all(|c| c.is_ascii_digit() || c == '.') || ["true", "false", "yes", "no", "none", "null", "auto", "on", "off"].contains(&v.to_ascii_lowercase().as_str())
}

/// Masks `docker inspect` output in place: the value of every secret-looking variable in an
/// `Env` list, and anything that looks like a secret in the other strings (`--token …` in Cmd).
pub fn mask_env(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                match (k.as_str(), v) {
                    ("Env", Value::Array(items)) => {
                        for item in items.iter_mut() {
                            if let Value::String(s) = item {
                                *s = match s.split_once('=') {
                                    Some((name, value)) if secret_name(name) && !plainly_not_secret(value) => format!("{name}={}", audit::REDACTED),
                                    Some((name, value)) if secret_name(name) => format!("{name}={value}"),
                                    _ => audit::redact(s),
                                };
                            }
                        }
                    }
                    (_, v) => mask_env(v),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_env),
        Value::String(s) => *s = audit::redact(s),
        _ => {}
    }
}

/// The value of `name` in the first `Env` list of an inspect result, unmasked.
pub fn env_value(value: &serde_json::Value, name: &str) -> Option<String> {
    use serde_json::Value;
    match value {
        Value::Object(map) => map.iter().find_map(|(k, v)| match (k.as_str(), v) {
            ("Env", Value::Array(items)) => items.iter().filter_map(Value::as_str).find_map(|s| s.split_once('=').filter(|(n, _)| *n == name).map(|(_, v)| v.to_string())),
            (_, v) => env_value(v, name),
        }),
        Value::Array(items) => items.iter().find_map(|v| env_value(v, name)),
        _ => None,
    }
}

/// The container's name ("vllm-head") from an inspect result.
pub fn inspect_name(value: &serde_json::Value) -> Option<String> {
    let v = value.as_array().and_then(|a| a.first()).unwrap_or(value);
    v.get("Name").and_then(|n| n.as_str()).map(|n| n.trim_start_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_node::{self, fake_node, temp, Script};
    use crate::live::fence_class::FenceClass::{ContainerTest, FilesTest, ProcessTest};
    use crate::live::Live;

    const ID: &str = "5b0c7e1f2a3d4c5b6a7980f1e2d3c4b5a69788776655443322110ffeeddccbba";

    /// Docker on cell2 as it answers the test container's commands: a second `docker run` of the
    /// name is refused as Docker refuses it (exit 125, on stderr).
    fn docker() -> Script {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        Arc::new(move |cmd: &str, _: &[u8]| {
            let w: Vec<&str> = cmd.split_whitespace().collect();
            match w.as_slice() {
                ["docker", "run", ..] if ran.swap(true, std::sync::atomic::Ordering::SeqCst) => Some((format!(
                    "!stderr:docker: Error response from daemon: Conflict. The container name \"/{TEST_CONTAINER}\" is already in use by container \"{ID}\". You have to remove (or rename) that container to be able to reuse that name.\nSee 'docker run --help'.\n"
                ), 125)),
                ["docker", "run", ..] => Some((format!("{ID}\n"), 0)),
                ["docker", "stop" | "rm", .., last] => Some((format!("{last}\n"), 0)),
                _ => None,
            }
        })
    }

    fn ops_for(live: &Arc<Live>, inv: &crate::inventory::Inventory) -> Arc<Ops> {
        let demo = Arc::new(std::sync::Mutex::new(crate::demo::DemoCluster::new(&crate::inventory::Inventory::example())));
        let ops = Ops::new(live.clone(), demo, crate::tasks::Tasks::new(None));
        let (l, i) = (live.clone(), inv.clone());
        ops.set_snapshot_source(move || Some(l.snapshot(&i)));
        ops
    }

    /// Containers › Run test container (T2's "Run sparkbox-test-busybox"): C7 as the cluster user,
    /// with this build's instance label, admitted as container-test under the T2 preset (cell2:
    /// test containers, process signals, files); refused without container-test and in look-only,
    /// with nothing reaching the node. A second one is "remove it first", and the one it started
    /// stops and goes under container-test too. Demo says it needs Live data.
    #[tokio::test]
    async fn the_test_container_runs_under_the_t2_preset_and_nowhere_else() {
        let (node, port) = fake_node(&temp("test-container-home"), docker()).await;
        let base = temp("test-container-app");
        let (live, inv) = fake_node::connect(&base, &[("cell2", port)]).await;
        let ops = ops_for(&live, &inv);
        let message = |e: SbError| crate::errors::SbError::classify(&e.message).message;
        let runs = || node.ran("docker run");
        let c7 = format!("docker run -d --name sparkbox-test-busybox --label sparkbox.test=1 --label sparkbox.instance={} busybox:1.36 sleep 3600", crate::identity::instance_name());
        assert_eq!(test_container_command("dev"), "docker run -d --name sparkbox-test-busybox --label sparkbox.test=1 --label sparkbox.instance=dev busybox:1.36 sleep 3600");

        crate::live::tests::open_fence(&live, &base, &["cell2"], &[ProcessTest, FilesTest]);
        assert_eq!(message(ops.container_test_run(false, "cell2").await.unwrap_err()), "The fence doesn't allow test containers.");
        assert!(runs().is_empty());

        crate::live::tests::open_fence(&live, &base, &["cell2"], &[ContainerTest, ProcessTest, FilesTest]);
        assert_eq!(ops.container_test_run(false, "cell2").await.unwrap(), ID);
        assert_eq!(runs(), [c7.as_str()]);
        let row = live.audit.entries().into_iter().find(|e| e.command == c7).expect("its row");
        assert_eq!((row.fence.as_deref(), row.escalated, row.title.as_str()), (Some("container-test"), false, "Run the test container on cell2"));
        assert!(node.ran("sudo").is_empty(), "as the cluster user");
        let e = ops.container_test_run(false, "cell2").await.unwrap_err();
        assert_eq!(e.message, "A test container is already on cell2; remove it first.");

        // It shows on cell2: Stop and Remove (Containers) are container-test changes too.
        let mut frame: serde_json::Value = serde_json::from_str(&crate::live::tests::frame()).unwrap();
        frame["containers"] = json!([{ "id": ID, "name": TEST_CONTAINER, "image": "busybox:1.36", "state": "exited", "startedAt": 1, "finishedAt": 2, "exitCode": 0,
            "cpuPct": 0.0, "mem": 0, "gpuMem": 0, "ports": [], "project": null, "command": "sleep 3600",
            "labels": { "sparkbox.test": "1", "sparkbox.instance": crate::identity::instance_name() } }]);
        *node.knobs.frame.lock().unwrap() = Some(frame.to_string());
        for _ in 0..200 {
            if live.snapshot(&inv).nodes.iter().any(|n| n.containers.iter().any(|c| c.name == TEST_CONTAINER)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        for action in ["stop", "remove"] {
            ops.container_action(false, "cell2", ID, action, false).await.unwrap();
            let cmd = docker_command(action, ID, false).unwrap();
            let row = live.audit.entries().into_iter().find(|e| e.command == cmd).expect("its row");
            assert_eq!(row.fence.as_deref(), Some("container-test"), "{action}");
        }

        // Look-only: refused before any fence is asked.
        live.set_write_policy(crate::live::tests::TestPolicy::look_only());
        assert!(message(ops.container_test_run(false, "cell2").await.unwrap_err()).contains("look-only mode"));
        assert_eq!(runs().len(), 2, "nothing more reached the node");
        let e = ops.container_test_run(true, "cell2").await.unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::DemoOnly, "Available with Live data"));
        live.stop();
    }

    #[test]
    fn only_secret_names_with_secret_looking_values_are_masked() {
        // The real glm53 environment (text-inspect-glm53-cell1): MTP_TOKENS is a count.
        let mut v = serde_json::json!([{ "Config": { "Env": [
            "MTP_TOKENS=5", "API_KEY=sk-glm53-0123456789", "MAX_NUM_SEQS=8", "MOE_BACKEND=", "SERVED_NAME=glm-5.3-flash",
            "HF_TOKEN=hf_AbCdEf0123456789", "MAX_NUM_BATCHED_TOKENS=8192", "DB_PASS=x", "SECRET_COUNT=3", "KEY_FILE_MODE=600",
        ] } }]);
        mask_env(&mut v);
        let env: Vec<&str> = v[0]["Config"]["Env"].as_array().unwrap().iter().map(|e| e.as_str().unwrap()).collect();
        assert_eq!(env, [
            "MTP_TOKENS=5", "API_KEY=‹redacted›", "MAX_NUM_SEQS=8", "MOE_BACKEND=", "SERVED_NAME=glm-5.3-flash",
            "HF_TOKEN=‹redacted›", "MAX_NUM_BATCHED_TOKENS=8192", "DB_PASS=‹redacted›", "SECRET_COUNT=3", "KEY_FILE_MODE=600",
        ]);
    }
}
