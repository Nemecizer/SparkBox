//! Tauri commands for the node agents. Installing, removing, pushing settings and the
//! test page from every node act on nodes (node scope); an install or a push that copies
//! one of the owner's secrets to the nodes (a channel secret, an endpoint key, the metrics
//! token) needs Mac scope as well, like saving that secret did. The metrics settings hold a
//! token (Mac scope).

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use super::{Agents, AgentSettings, Apply, EndToEnd, Layout, PushReason, WatchState};
use crate::live::audit::{self, OpMeta};
use crate::tasks::{TaskSpec, Tasks};

fn nodes_of(app: &AppHandle, ids: &[String]) -> Result<Vec<crate::inventory::NodeConfig>, String> {
    let state = app.state::<crate::AppState>();
    let inv = state.inventory.lock().unwrap();
    if ids.is_empty() {
        return Err("Pick at least one node.".into());
    }
    ids.iter().map(|id| inv.nodes.iter().find(|n| &n.id == id).cloned().ok_or_else(|| format!("No node called \"{id}\"."))).collect()
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Every node's agent, who sends alerts right now, and what 24/7 rests on.
#[tauri::command]
pub fn agent_status(app: AppHandle) -> WatchState {
    super::current(&app)
}

#[derive(Clone, Copy, PartialEq)]
enum Job {
    Install,
    Remove,
    Push(PushReason),
}

fn run_job(app: &AppHandle, job: Job, ids: Vec<String>) -> Result<String, String> {
    let nodes = nodes_of(app, &ids)?;
    let state = app.state::<crate::AppState>();
    let demo = !crate::is_live(&state);
    let (kind, title) = match job {
        Job::Install => ("agent-install", format!("Install the always-on agent on {} node{}", nodes.len(), plural(nodes.len()))),
        Job::Remove => ("agent-remove", format!("Remove the always-on agent from {} node{}", nodes.len(), plural(nodes.len()))),
        Job::Push(_) => ("agent-config", format!("Push alert settings to {} node{}", nodes.len(), plural(nodes.len()))),
    };
    let tasks = app.state::<Arc<Tasks>>().inner().clone();
    let task = tasks.start(TaskSpec::new(kind, title.clone(), &ids));
    let id = task.id().to_string();
    let live = state.live.clone();
    let inv = state.inventory.lock().unwrap().clone();
    if let Job::Push(_) = job {
        app.state::<Arc<Agents>>().set_push_task(Some(id.clone()));
    }
    let app = app.clone();
    let head_nodes = ids.join(", ");
    tauri::async_runtime::spawn(audit::with_op_id(id.clone(), OpMeta { title: title.clone(), source: "ui".into() }, async move {
        let head = live.audit.begin_op(&head_nodes, &title, "", demo);
        let layout = Layout::system();
        let agents = app.state::<Arc<Agents>>().inner().clone();
        let mut done = Vec::new();
        let mut failed = Vec::new();
        for node in &nodes {
            let result = if demo {
                simulate(&app, job, node, &task).await
            } else {
                match job {
                    Job::Install => {
                        let bundle = super::bundle_for(&app, &inv, node, true);
                        let fingerprint = super::bundle_for(&app, &inv, node, false).hash();
                        let r = super::install_node(&live, node, &layout, &bundle.files(Apply::Restart, true), &task).await;
                        if r.is_ok() {
                            let now = crate::history::now_ms();
                            agents.update(&node.id, |rec| {
                                rec.installed = true;
                                rec.expected_hash = Some(bundle.hash());
                                rec.fingerprint = Some(fingerprint);
                                rec.pushed_parts = Some(bundle.parts());
                                rec.pushed_at_ms = Some(now);
                                rec.push_error = None;
                            });
                        }
                        r
                    }
                    Job::Push(_) => {
                        task.step(format!("Push the settings to {}", node.name));
                        let bundle = super::bundle_for(&app, &inv, node, true);
                        let fingerprint = super::bundle_for(&app, &inv, node, false).hash();
                        let r = super::push_one(&live, &agents, node, &layout, &bundle, &fingerprint, crate::history::now_ms()).await;
                        match &r {
                            Ok(a) => task.step_ok(Some(if *a == Apply::Reload { "the agent reloaded them".into() } else { "the agent restarted with them".into() })),
                            Err(e) => task.step_failed(e.clone()),
                        }
                        r.map(|_| format!("{} updated", node.name))
                    }
                    Job::Remove => {
                        task.step(format!("Remove it from {}", node.name));
                        let r = super::uninstall_node(&live, node, &layout).await.map(|_| format!("removed from {}", node.name));
                        match &r {
                            Ok(_) => {
                                task.step_ok(None);
                                agents.update(&node.id, |rec| *rec = super::NodeRecord::default());
                            }
                            Err(e) => task.step_failed(e.clone()),
                        }
                        r
                    }
                }
            };
            match result {
                Ok(_) => done.push(node.name.clone()),
                Err(e) => failed.push(if e.starts_with(&node.name) { e } else { format!("{}: {e}", node.name) }),
            }
            if !demo {
                let st = super::read_status(&live, node, &layout).await;
                agents.set_live(st);
            }
            super::emit(&app);
        }
        let summary = match (done.len(), failed.len()) {
            (_, 0) => match job {
                Job::Install => format!("Running on {}", done.join(", ")),
                Job::Remove => format!("Removed from {}", done.join(", ")),
                Job::Push(_) => format!("Pushed to {} of {}", done.len(), nodes.len()),
            },
            (0, _) if !matches!(job, Job::Push(_)) => String::new(),
            _ => match job {
                Job::Push(_) => format!("Pushed to {} of {}", done.len(), nodes.len()),
                _ => format!("Done on {}", done.join(", ")),
            },
        };
        if failed.is_empty() {
            live.audit.end_op(&head, true, Some(summary.clone()), None);
            task.finish_ok(summary);
        } else {
            // "Pushed to 3 of 4 · Failed on cell3: sudo: a password is required"
            let error = failed.iter().map(|f| format!("Failed on {f}")).collect::<Vec<_>>().join(" · ");
            live.audit.end_op(&head, false, None, Some(error.clone()));
            task.finish_err(if summary.is_empty() { error } else { format!("{summary} · {error}") });
        }
        if let Job::Push(_) = job {
            app.state::<Arc<Agents>>().set_push_task(None);
        }
    }));
    Ok(id)
}

/// Demo: the same steps, simulated, and noted in the activity log as what would have run.
async fn simulate(app: &AppHandle, job: Job, node: &crate::inventory::NodeConfig, task: &crate::tasks::TaskHandle) -> Result<String, String> {
    let audit = &app.state::<crate::AppState>().live.audit;
    let steps: &[(&str, &str)] = match job {
        Job::Install => &[
            ("Install the agent on {}", "sudo sh -c 'umask 077; w=$(mktemp -d /root/sparkbox-agent.XXXXXXXX); cat > \"$w/stream\"; … sha256sum -c sums && useradd --system sparkbox-agent && install -o root -g root -m 0644 … /opt/sparkbox-agent/ && systemctl enable sparkbox-agent && systemctl restart sparkbox-agent'"),
            ("Check it's running on {}", "systemctl is-active sparkbox-agent && sparkbox_agent.py status"),
        ],
        Job::Remove => &[("Remove it from {}", "sudo systemctl disable --now sparkbox-agent && rm -rf /opt/sparkbox-agent /etc/sparkbox-agent /var/lib/sparkbox-agent")],
        Job::Push(_) => &[("Push the settings to {}", "sudo sh -c '… sha256sum -c sums && install -o root -g root -m 0600 … /etc/sparkbox-agent/notify.json && systemctl restart sparkbox-agent'")],
    };
    for (label, command) in steps {
        let label = label.replace("{}", &node.name);
        task.step(label.clone());
        tokio::time::sleep(Duration::from_millis(700)).await;
        let push = matches!(job, Job::Push(_)).then(|| app.state::<Arc<Agents>>().inner().as_ref());
        if let Err(e) = simulate_step(audit, push, &node.id, &label, command, crate::history::now_ms()) {
            task.step_failed(e.clone());
            return Err(e);
        }
        task.step_ok(None);
    }
    let agents = app.state::<Arc<Agents>>();
    let mut sim = agents.demo.lock().unwrap();
    match job {
        Job::Install => sim.install(&node.id, crate::history::now_ms()),
        Job::Remove => sim.remove(&node.id),
        Job::Push(_) => {}
    }
    Ok(node.name.clone())
}

/// One simulated step, noted in the activity log as what would have run. A push (`push`: the
/// Demo agents) can fail as the scenario says; then the node's own row fails too, so the
/// operation reads "3/4 ok · 1 failed", never a failed push over four ok rows.
pub(super) fn simulate_step(audit: &crate::live::audit::Log, push: Option<&Agents>, node: &str, label: &str, command: &str, now: u64) -> Result<(), String> {
    let row = audit.record_simulated(node, label, command);
    let Some(agents) = push else { return Ok(()) };
    let r = agents.demo.lock().unwrap().push(node, now);
    if let Err(e) = &r {
        audit.update(&row, |x| {
            x.state = "failed".into();
            x.code = Some(1);
            x.error = Some(e.clone());
        });
    }
    r
}

/// Installs or upgrades the agent: one sudo step that checks every file (SHA-256) as root,
/// installs it root-owned, creates the sparkbox-agent user, starts it; then verifies. Mac
/// scope when it copies the owner's secrets (`push_copies_secrets`), node scope otherwise.
#[tauri::command]
pub fn agent_install(app: AppHandle, nodes: Vec<String>) -> Result<String, String> {
    if super::push_copies_secrets(&app, &nodes) { crate::admin::require_mac(&app)?; } else { crate::admin::require(&app)?; }
    run_job(&app, Job::Install, nodes)
}

/// Stops the agent and removes it, its settings and its history from the nodes.
#[tauri::command]
pub fn agent_uninstall(app: AppHandle, nodes: Vec<String>) -> Result<String, String> {
    crate::admin::require(&app)?;
    run_job(&app, Job::Remove, nodes)
}

/// The settings push `config_changed` starts (rules, channels, quiet hours or silences changed).
pub fn push(app: &AppHandle, nodes: Vec<String>, reason: PushReason) -> Result<String, String> {
    run_job(app, Job::Push(reason), nodes)
}

/// "Push again": the current settings to `nodes` (every node with an agent when empty). One
/// push at a time: while one runs, this joins it. The state it returns names the push task
/// (`pushTask`), whose end is the one outcome to show. Mac scope when the push copies the
/// owner's secrets, node scope otherwise.
#[tauri::command]
pub fn agent_push_config(app: AppHandle, nodes: Option<Vec<String>>) -> Result<WatchState, String> {
    if super::push_copies_secrets(&app, nodes.as_deref().unwrap_or(&[])) { crate::admin::require_mac(&app)?; } else { crate::admin::require(&app)?; }
    let mut ws = super::current(&app);
    let agents = app.state::<Arc<Agents>>().inner().clone();
    if let Some(id) = agents.running_push(&app.state::<Arc<Tasks>>()) {
        ws.push_task = Some(id);
        return Ok(ws);
    }
    let installed: Vec<String> = ws.agents.iter().filter(|a| a.installed).map(|a| a.node.clone()).collect();
    let nodes: Vec<String> = match nodes.filter(|n| !n.is_empty()) {
        Some(list) => list.into_iter().filter(|n| installed.contains(n)).collect(),
        None => installed,
    };
    if nodes.is_empty() {
        return Err("No node has the agent installed yet.".into());
    }
    ws.push_task = Some(push(&app, nodes, PushReason::Requested)?);
    Ok(ws)
}

/// Each node's agent sends one test through each phone channel (`test-notify --all`), at
/// most once a minute. The results are kept per node and in Deliveries.
#[tauri::command]
pub async fn agent_test_page(app: AppHandle) -> Result<EndToEnd, String> {
    crate::admin::require(&app)?;
    let agents = app.state::<Arc<Agents>>().inner().clone();
    let now = crate::history::now_ms();
    if !agents.may_test_page(now) {
        return Err("A test page went out less than a minute ago. Wait a moment and try again.".into());
    }
    let state = app.state::<crate::AppState>();
    let demo = !crate::is_live(&state);
    let inv = state.inventory.lock().unwrap().clone();
    let live = state.live.clone();
    let notifier = app.state::<Arc<crate::notify::Notifier>>().inner().clone();
    let settings = notifier.settings();
    let mut channels: Vec<crate::delivery::Channel> = settings.paging_channels().filter(|c| !c.mac_only).cloned().collect();
    if demo {
        channels.push(crate::notify::demo_channel());
        let runs = agents.demo.lock().unwrap().test_page(&channels, now);
        for (node, run) in &runs {
            live.audit.record_simulated(node, &format!("Send a test page from {node}"), "sparkbox_agent.py test-notify --all --json");
            let name = inv.nodes.iter().find(|n| &n.id == node).map(|n| n.name.clone()).unwrap_or_else(|| node.clone());
            notifier.deliveries.record_all(super::test_rows(node, &name, run, &channels, true));
        }
        super::emit(&app);
        return Ok(super::current(&app).end_to_end);
    }
    if channels.is_empty() {
        return Err("Add a phone channel first: the agents have nothing to send a test to.".into());
    }
    let ws = super::current(&app);
    let nodes: Vec<crate::inventory::NodeConfig> = inv.nodes.iter().filter(|n| ws.agents.iter().any(|a| a.node == n.id && a.installed)).cloned().collect();
    if nodes.is_empty() {
        return Err("No node has the agent installed yet.".into());
    }
    let head = live.audit.begin_op(&nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>().join(", "), "Send a test page from every node", "", false);
    let layout = Layout::system();
    let runs = futures_util::future::join_all(nodes.iter().map(|n| super::test_page_node(&live, &agents, Some(&notifier), n, &layout, &channels))).await;
    let failed = runs.iter().flat_map(|r| r.results.values()).filter(|c| !c.ok).count();
    let total: usize = runs.iter().map(|r| r.results.len()).sum();
    live.audit.end_op(&head, failed == 0, Some(format!("{} of {total} delivered", total - failed)), (failed > 0).then(|| format!("{failed} of {total} failed")));
    super::emit(&app);
    Ok(super::current(&app).end_to_end)
}

#[derive(Serialize)]
pub struct Backfilled {
    pub points: usize,
    pub alerts: usize,
}

/// Merges what the agents recorded (history and alerts) into this Mac's store now.
#[tauri::command]
pub async fn agent_backfill_now(app: AppHandle) -> Result<Backfilled, String> {
    let state = app.state::<crate::AppState>();
    if !crate::is_live(&state) {
        // Demo history is the charts' own; the simulated agents only add their alert log.
        let snap = state.latest.read().unwrap().clone().ok_or("No data yet.")?;
        let agents = app.state::<Arc<Agents>>();
        let alerts = agents.demo.lock().unwrap().seed_away_once(&app, &snap);
        return Ok(Backfilled { points: 0, alerts });
    }
    let inv = state.inventory.lock().unwrap().clone();
    let live = state.live.clone();
    let agents = app.state::<Arc<Agents>>().inner().clone();
    let installed: Vec<String> = agents.statuses(false, &inv, crate::history::now_ms()).into_iter().filter(|s| s.installed).map(|s| s.node).collect();
    let (mut points, mut alerts) = (0, 0);
    for node in inv.nodes.iter().filter(|n| installed.contains(&n.id)) {
        let (p, a) = super::backfill_node(&app, &live, node, &Layout::system(), agents.session_start(), true).await?;
        points += p;
        alerts += a;
    }
    Ok(Backfilled { points, alerts })
}

#[tauri::command]
pub fn agent_settings_get(agents: State<'_, Arc<Agents>>) -> AgentSettings {
    agents.settings()
}

/// "Expose metrics for Prometheus": its port and optional bearer token (`token`: None
/// keeps it, "" removes it). Pushed to the agents.
#[tauri::command]
pub fn agent_settings_set(app: AppHandle, agents: State<'_, Arc<Agents>>, settings: AgentSettings, token: Option<String>) -> Result<AgentSettings, String> {
    crate::admin::require_mac(&app)?;
    let before = agents.settings();
    let mut settings = settings;
    settings.has_token = before.has_token;
    // Replacing or removing the bearer token is a Keychain write: it confirms on its own.
    if token.is_some() {
        crate::admin::require_mac_confirmed(&app, "agent_settings_set")?;
    }
    match token.as_deref() {
        Some("") => {
            crate::secrets::clear(super::METRICS_TOKEN)?;
            settings.has_token = false;
        }
        Some(t) => {
            crate::secrets::set(super::METRICS_TOKEN, t)?;
            settings.has_token = true;
        }
        None => {}
    }
    let saved = agents.save_settings(settings)?;
    if let Some(state) = app.try_state::<crate::AppState>() {
        let word = |s: &AgentSettings| if s.metrics_enabled { format!("on, port {}", s.metrics_port) } else { "off".into() };
        let mut what = format!("Prometheus metrics: {} → {}", word(&before), word(&saved));
        match token.as_deref() {
            Some("") => what.push_str(" · token removed"),
            Some(_) => what.push_str(" · token replaced"),
            None => {}
        }
        state.live.audit.record_app("Agent settings changed", &what);
    }
    super::config_changed(&app);
    Ok(saved)
}
