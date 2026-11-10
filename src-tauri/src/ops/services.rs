//! systemd services: which actions SparkBox allows, what `systemctl status` and the journal
//! say (parsed for the Services status pane), and what an action will break — the copy the
//! confirmation shows, worked out from the unit, the node's containers and whether you're
//! connected from outside (VPN).

use serde::Serialize;
use serde_json::Value;

use crate::errors::SbError;
use crate::model::{Listener, NodeSnapshot};

pub const ACTIONS: &[&str] = &["start", "stop", "restart", "reload", "enable", "disable", "reset-failed", "mask", "unmask", "daemon-reload"];
/// Actions that act on no unit: `systemctl daemon-reload` reads the unit files again (P1.17).
pub const NO_UNIT: &[&str] = &["daemon-reload"];
/// The actions a `service` fence covers, as the exit reads their shape (fence_class.rs
/// `sudo_shape`). mask and unmask write a link under /etc/systemd/system and reset-failed isn't
/// a service change there either: their shape is `other`, which only an `any` fence admits.
pub const SERVICE_CLASS: &[&str] = &["start", "stop", "restart", "reload", "enable", "disable", "daemon-reload"];

/// Services that keep the node reachable or serving; changing them deserves a second look.
pub const CRITICAL: &[&str] = &[
    "ssh.service",
    "ssh.socket",
    "sshd.service",
    "docker.service",
    "docker.socket",
    "containerd.service",
    "systemd-networkd.service",
    "NetworkManager.service",
    "systemd-resolved.service",
    "nvidia-persistenced.service",
    "dgx-dashboard.service",
];

pub fn check_action(unit: &str, action: &str) -> Result<(), SbError> {
    if !ACTIONS.contains(&action) {
        return Err(SbError::invalid(format!("unsupported action {action}")));
    }
    if NO_UNIT.contains(&action) {
        return if unit.is_empty() { Ok(()) } else { Err(SbError::invalid(format!("{action} acts on no service; leave the service out."))) };
    }
    if !super::unit_ok(unit) {
        return Err(SbError::invalid("That service name has characters SparkBox won't pass to a shell."));
    }
    Ok(())
}

/// "Restart", "Reset failed state of"…
pub fn verb(action: &str) -> &'static str {
    match action {
        "start" => "Start",
        "stop" => "Stop",
        "restart" => "Restart",
        "reload" => "Reload",
        "enable" => "Enable",
        "disable" => "Disable",
        "reset-failed" => "Clear the failed state of",
        "mask" => "Mask",
        "unmask" => "Unmask",
        "daemon-reload" => "Reload the unit files",
        _ => "Change",
    }
}

/// What the action is, for a title: "Restart ollama.service on cell1", "Reload the unit files on cell1".
pub fn action_title(action: &str, unit: &str, on: &str) -> String {
    if NO_UNIT.contains(&action) {
        format!("{} on {on}", verb(action))
    } else {
        format!("{} {unit} on {on}", verb(action))
    }
}

/// What runs on the node (through sudo): `systemctl <action> -- '<unit>'`, or the action alone.
pub fn action_command(unit: &str, action: &str) -> String {
    if NO_UNIT.contains(&action) {
        format!("systemctl {action}")
    } else {
        format!("systemctl {action} -- {}", crate::live::sh_quote(unit))
    }
}

pub fn done(action: &str) -> &'static str {
    match action {
        "start" => "Started",
        "stop" => "Stopped",
        "restart" => "Restarted",
        "reload" => "Reloaded",
        "enable" => "Enabled at boot",
        "disable" => "Disabled at boot",
        "reset-failed" => "Failed state cleared",
        "mask" => "Masked",
        "unmask" => "Unmasked",
        "daemon-reload" => "Unit files reloaded",
        _ => "Done",
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalLine {
    /// As journalctl prints it: "Oct 28 09:14:02".
    pub time: String,
    /// Unix ms when known (JSON output), for showing local time.
    pub ts_ms: Option<u64>,
    /// syslog priority: 3 err, 4 warning, 5 notice, 6 info, 7 debug.
    pub priority: u8,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDetail {
    pub unit: String,
    pub description: String,
    pub loaded: Option<String>,
    /// "active (running)", "failed (Result: exit-code)"…
    pub active: Option<String>,
    /// "Tue 2026-10-28 09:14:02 UTC; 1h 3min ago"
    pub active_since: Option<String>,
    pub main_pid: Option<u32>,
    /// Why it stopped: exit-code, signal, timeout, oom-kill…, with the status when given.
    pub result: Option<String>,
    pub memory: Option<String>,
    pub cpu: Option<String>,
    pub tasks: Option<String>,
    pub status_text: String,
    pub journal: Vec<JournalLine>,
}

/// Reads the header of `systemctl status`.
pub fn parse_status(unit: &str, text: &str, journal: Vec<JournalLine>) -> ServiceDetail {
    let mut d = ServiceDetail { unit: unit.into(), status_text: text.to_string(), journal, ..Default::default() };
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if i == 0 {
            if let Some((_, desc)) = t.split_once(" - ") {
                d.description = desc.trim().into();
            }
            continue;
        }
        let Some((key, value)) = t.split_once(": ") else { continue };
        let value = value.trim();
        match key {
            "Loaded" => d.loaded = Some(value.into()),
            "Active" => {
                let (state, since) = match value.split_once(" since ") {
                    Some((s, since)) => (s, Some(since)),
                    None => (value, None),
                };
                d.active = Some(state.trim().into());
                d.active_since = since.map(|s| s.trim().to_string());
                if let Some(r) = state.split_once("(Result: ").and_then(|(_, r)| r.split(')').next()) {
                    d.result = Some(r.into());
                }
            }
            "Main PID" => {
                d.main_pid = value.split_whitespace().next().and_then(|p| p.parse().ok());
                // "2211 (code=exited, status=1/FAILURE)"
                if let Some(status) = value.split_once("status=").map(|(_, s)| s.trim_end_matches(')')) {
                    d.result = Some(match d.result.take() {
                        Some(r) if !r.contains(status) => format!("{r} · status {status}"),
                        Some(r) => r,
                        // A oneshot that ran and exited 0 (anacron after its daily run): a clean finish.
                        None if status.starts_with("0/") => "success".into(),
                        None => format!("status {status}"),
                    });
                }
            }
            "Memory" => d.memory = Some(value.into()),
            "CPU" => d.cpu = Some(value.into()),
            "Tasks" => d.tasks = Some(value.split(" (").next().unwrap_or(value).into()),
            _ => {}
        }
    }
    d
}

/// `journalctl -o json`: one object per line.
pub fn parse_journal_json(text: &str) -> Vec<JournalLine> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| {
            let ts_ms = v["__REALTIME_TIMESTAMP"].as_str().and_then(|s| s.parse::<u64>().ok()).map(|us| us / 1000);
            let ident = v["SYSLOG_IDENTIFIER"].as_str().unwrap_or("");
            let pid = v["_PID"].as_str().map(|p| format!("[{p}]")).unwrap_or_default();
            let message = match &v["MESSAGE"] {
                Value::String(s) => s.clone(),
                // Non-UTF-8 messages come as byte arrays.
                Value::Array(bytes) => String::from_utf8_lossy(&bytes.iter().filter_map(|b| b.as_u64().map(|b| b as u8)).collect::<Vec<_>>()).into_owned(),
                _ => String::new(),
            };
            JournalLine {
                time: ts_ms.map(|ms| short_time(ms / 1000)).unwrap_or_default(),
                ts_ms,
                priority: v["PRIORITY"].as_str().and_then(|p| p.parse().ok()).unwrap_or(6),
                text: if ident.is_empty() { message } else { format!("{ident}{pid}: {message}") },
            }
        })
        .collect()
}

/// journalctl's default text ("Oct 28 09:14:02 cell1 ident[pid]: message"); the priority
/// isn't printed, so it's read from the words.
pub fn parse_journal_text(text: &str) -> Vec<JournalLine> {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("-- "))
        .map(|l| {
            let mut parts = l.splitn(5, ' ');
            let (m, d, t) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let _host = parts.next();
            let rest = parts.next().unwrap_or("").to_string();
            let looks_dated = m.len() == 3 && t.matches(':').count() == 2;
            let (time, body) = if looks_dated { (format!("{m} {d} {t}"), rest) } else { (String::new(), l.to_string()) };
            JournalLine { priority: guess_priority(&body), time, ts_ms: None, text: body }
        })
        .collect()
}

fn guess_priority(text: &str) -> u8 {
    let t = text.to_lowercase();
    if ["failed", "failure", "error", "cannot", "can't", "permission denied", "fatal", "segfault", "oom", "killed"].iter().any(|w| t.contains(w)) {
        3
    } else if ["warn", "deprecated", "retry", "timeout", "timed out"].iter().any(|w| t.contains(w)) {
        4
    } else {
        6
    }
}

fn short_time(unix: u64) -> String {
    let (_, m, d, h, mi, s, _) = crate::demo::ops::civil(unix);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {d:02} {h:02}:{mi:02}:{s:02}", MONTHS[(m as usize).saturating_sub(1) % 12])
}

// ---- What each unit is, for the Services window (one batched read, no root)

/// A unit as the Services window shows it: list-units' fields plus what tells an idle unit
/// from a stopped one (its type, the timer that starts it, its last result, a skipped condition).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitFacts {
    pub unit: String,
    pub description: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub enabled: String,
    /// simple | exec | forking | oneshot | dbus | notify | notify-reload | idle
    pub kind: String,
    /// The units that start it ("anacron.timer"), when it's started by something else.
    pub triggered_by: Vec<String>,
    /// success | exit-code | signal | timeout…
    pub result: String,
    /// False when its start conditions weren't met (ConditionResult=no): skipped, not stopped.
    pub condition: Option<bool>,
}

/// Every loaded service's facts in one command (`systemctl show` takes a pattern), read every
/// 10 s while Services is open.
pub const SHOW_UNITS: &str = "systemctl show --no-pager --all \
-p Id,Description,LoadState,ActiveState,SubState,UnitFileState,Type,TriggeredBy,Result,ConditionResult '*.service'";
/// Installed services, loaded or not: read when Services opens and every 5 minutes.
pub const LIST_FILES: &str = "systemctl list-unit-files --type=service --no-pager --output=json";

/// `systemctl show -p …` for many units: blocks of Key=Value separated by blank lines.
pub fn parse_show(text: &str) -> Vec<UnitFacts> {
    text.split("\n\n")
        .filter_map(|block| {
            let mut f = UnitFacts::default();
            for line in block.lines() {
                let Some((k, v)) = line.split_once('=') else { continue };
                let v = v.trim().to_string();
                match k.trim() {
                    "Id" => f.unit = v,
                    "Description" => f.description = v,
                    "LoadState" => f.load = v,
                    "ActiveState" => f.active = v,
                    "SubState" => f.sub = v,
                    "UnitFileState" => f.enabled = v,
                    "Type" => f.kind = v,
                    "TriggeredBy" => f.triggered_by = v.split_whitespace().map(String::from).collect(),
                    "Result" => f.result = v,
                    "ConditionResult" => f.condition = match v.as_str() { "yes" => Some(true), "no" => Some(false), _ => None },
                    _ => {}
                }
            }
            (!f.unit.is_empty() && f.load != "not-found").then_some(f)
        })
        .collect()
}

/// Services Ubuntu starts from a timer: what Demo knows without `systemctl show`.
const TIMER_STARTED: &[&str] = &[
    "anacron.service", "apt-daily.service", "apt-daily-upgrade.service", "dpkg-db-backup.service", "e2scrub_all.service", "fstrim.service",
    "fwupd-refresh.service", "logrotate.service", "man-db.service", "motd-news.service", "sysstat-collect.service", "sysstat-summary.service",
    "update-notifier-download.service", "update-notifier-motd.service", "systemd-tmpfiles-clean.service",
];

/// A Demo unit with the facts `systemctl show` would add, guessed as Ubuntu ships them.
pub fn demo_facts(u: &crate::demo::ops::DemoUnit) -> UnitFacts {
    let timer = TIMER_STARTED.contains(&u.unit.as_str());
    UnitFacts {
        unit: u.unit.clone(),
        description: u.description.clone(),
        load: u.load.clone(),
        active: u.active.clone(),
        sub: u.sub.clone(),
        enabled: u.enabled.clone(),
        kind: if timer || u.sub == "exited" { "oneshot" } else { "simple" }.into(),
        triggered_by: if timer { vec![u.unit.replace(".service", ".timer")] } else { Vec::new() },
        result: if u.active == "failed" { "exit-code" } else { "success" }.into(),
        condition: Some(true),
    }
}

/// One poll of a node's services: every loaded unit's facts, and the installed unit files when asked.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesPoll {
    pub units: Vec<UnitFacts>,
    /// (unit file, its state), when `files` was asked for.
    pub files: Option<Vec<(String, String)>>,
}

/// The Services window's read: one `systemctl show` for every loaded unit, plus list-unit-files
/// when `files` (on open and every 5 minutes). Read-only; SparkBox's own polling, so it isn't
/// in the activity log.
#[tauri::command]
pub async fn services_poll(app: tauri::AppHandle, node: String, files: bool) -> Result<ServicesPoll, String> {
    use std::time::Duration;
    use tauri::Manager;
    let state = app.state::<crate::AppState>();
    if !crate::is_live(&state) {
        let units = state.demo.lock().unwrap().services(&node)?;
        let list = files.then(|| units.iter().map(|u| (u.unit.clone(), u.enabled.clone())).collect());
        return Ok(ServicesPoll { units: units.iter().map(demo_facts).collect(), files: list });
    }
    let out = state.live.probe(&node, SHOW_UNITS, Duration::from_secs(30)).await?;
    if out.code != Some(0) {
        return Err(out.failure("systemctl show", &node).unwrap_or_else(|| "systemctl show failed".into()));
    }
    let files = if files {
        let f = state.live.probe(&node, LIST_FILES, Duration::from_secs(30)).await?;
        let list: Vec<Value> = serde_json::from_str(&f.stdout).unwrap_or_default();
        Some(list.iter().filter_map(|x| Some((x["unit_file"].as_str()?.to_string(), x["state"].as_str()?.to_string()))).collect())
    } else {
        None
    };
    Ok(ServicesPoll { units: parse_show(&out.stdout), files })
}

// ---- Listening ports with their owners (sudo ss -tlnpH)

/// `ss -tlnpH`: "LISTEN 0 4096 0.0.0.0:8000 0.0.0.0:* users:(("python3",pid=2211,fd=7))".
pub fn parse_ss(text: &str) -> Vec<Listener> {
    let mut out: Vec<Listener> = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let local = cols[3];
        let Some((addr, port)) = local.rsplit_once(':') else { continue };
        let Ok(port) = port.parse::<u16>() else { continue };
        let users = cols.get(5).copied().unwrap_or("");
        let process = users.split("((\"").nth(1).and_then(|s| s.split('"').next()).map(String::from);
        let pid = users.split("pid=").nth(1).and_then(|s| s.split([',', ')']).next()).and_then(|p| p.parse().ok());
        let addr = addr.trim_matches(['[', ']']).to_string();
        if out.iter().any(|l| l.port == port && l.addr == addr) {
            continue;
        }
        out.push(Listener { port, addr, pid, process, ..Default::default() });
    }
    out.sort_by_key(|l| l.port);
    out
}

/// What `sudo ss` would add in Demo: owners of the ports the cluster user can't see.
pub fn demo_reveal(node: &NodeSnapshot) -> Vec<Listener> {
    node.listeners
        .iter()
        .map(|l| {
            let mut l = l.clone();
            if l.process.is_none() {
                let (name, pid) = match l.port {
                    22 => ("sshd", 1201),
                    53 => ("systemd-resolve", 812),
                    111 => ("rpcbind", 790),
                    631 => ("cupsd", 1530),
                    11000 => ("dgx-dashboard", 1688),
                    _ => ("root process", 0),
                };
                l.process = Some(name.into());
                l.pid = (pid > 0).then_some(pid);
            }
            l
        })
        .collect()
}

// ---- What an action will break

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Impact {
    /// low: no dialog · medium: confirm · high: type the name to confirm.
    pub tier: &'static str,
    pub title: String,
    pub lines: Vec<String>,
    pub require_typing: Option<String>,
    pub blocked: Option<String>,
    pub confirm_label: String,
}

/// One node an action will touch, as the impact needs it.
#[derive(Clone, Debug, Default)]
pub struct NodeCtx {
    pub name: String,
    /// Running containers' names.
    pub containers: Vec<String>,
    /// Models served from this node, e.g. "Llama-3.3-70B (cell1–cell3)".
    pub serving: Vec<String>,
}

pub fn service_impact(unit: &str, action: &str, nodes: &[NodeCtx], remote: bool) -> Impact {
    let short = unit.trim_end_matches(".service");
    let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
    let on = list(&names);
    let one = nodes.len() == 1;
    let typing = if one { names[0].to_string() } else { short.to_string() };
    let critical = CRITICAL.contains(&unit);
    let ssh = matches!(unit, "ssh.service" | "ssh.socket" | "sshd.service");
    let docker = matches!(unit, "docker.service" | "docker.socket" | "containerd.service");
    let network = matches!(unit, "systemd-networkd.service" | "NetworkManager.service");
    let mut im = Impact {
        tier: "low",
        title: format!("{}?", action_title(action, short, &on)),
        confirm_label: verb(action).split(' ').next().unwrap_or("OK").to_string(),
        ..Default::default()
    };
    let high = |im: &mut Impact| {
        im.tier = "high";
        im.require_typing = Some(typing.clone());
    };
    match action {
        "stop" if ssh => {
            for n in &names {
                im.lines.push(format!("SparkBox and new SSH sessions can't reach {n} until it's started from a console."));
            }
            if remote {
                im.lines.push("You're away (VPN connected), so there's no console to start it from.".into());
            }
            high(&mut im);
        }
        "disable" if ssh => {
            im.lines.push(if remote {
                "After the next reboot nobody can SSH in — you're remote.".into()
            } else {
                format!("After the next reboot nobody can SSH in to {on} until it's enabled from a console.")
            });
            high(&mut im);
        }
        "mask" if ssh => {
            im.lines.push(format!("ssh can't be started on {on} by anything, even after a reboot, until it's unmasked from a console."));
            high(&mut im);
        }
        "restart" if ssh => {
            im.lines.push("Open sessions stay connected; new logins wait a moment while it restarts.".into());
            im.tier = "medium";
        }
        "stop" | "restart" if docker => {
            for n in nodes {
                if n.containers.is_empty() {
                    im.lines.push(format!("{} has no running containers.", n.name));
                } else {
                    im.lines.push(format!(
                        "{} running container{} on {} {}: {}.",
                        n.containers.len(),
                        if n.containers.len() == 1 { "" } else { "s" },
                        n.name,
                        if action == "stop" { "stop" } else { "restart" },
                        n.containers.join(", ")
                    ));
                }
                for s in &n.serving {
                    im.lines.push(format!("{s} is unavailable until its containers are back."));
                }
            }
            if action == "restart" {
                im.lines.push("Containers with a restart policy come back by themselves; vLLM containers (restart: no) stay stopped.".into());
            }
            if nodes.iter().any(|n| !n.serving.is_empty()) {
                high(&mut im);
            } else {
                im.tier = "medium";
            }
        }
        "stop" | "restart" | "disable" if network => {
            im.lines.push(format!("{on}'s network settings are torn down; SparkBox (and you) will most likely lose the connection."));
            high(&mut im);
        }
        "stop" if unit == "nvidia-persistenced.service" => {
            im.lines.push("The GPU driver unloads whenever the GPU is idle, so the next CUDA program starts several seconds slower. Running models keep going.".into());
            im.tier = "medium";
        }
        "stop" if unit == "dgx-dashboard.service" => {
            im.lines.push(format!("The DGX Dashboard on {on} stops answering until it's started again."));
            im.tier = "medium";
        }
        "mask" | "unmask" => {
            im.lines.push(if action == "mask" {
                format!("{short} can't be started on {on} by anything — not by hand, not by another service — until it's unmasked.")
            } else {
                format!("{short} can be started again on {on}; it doesn't start by itself.")
            });
            high(&mut im);
        }
        "stop" => {
            im.lines.push(format!("{short} stops on {on}, along with anything that needs it."));
            im.tier = "medium";
        }
        "restart" => {
            im.lines.push(format!("{short} is briefly unavailable on {on} while it restarts."));
            im.tier = "medium";
        }
        "disable" => {
            im.lines.push("It won't start at boot any more (it keeps running until stopped).".into());
            im.tier = "medium";
        }
        "daemon-reload" => {
            im.lines.push(format!("systemd on {on} reads its unit files again; no service starts, stops or restarts."));
        }
        _ => {}
    }
    if critical && im.tier == "low" && matches!(action, "stop" | "restart" | "disable") {
        im.tier = "medium";
    }
    if !one && im.tier == "medium" {
        im.lines.push(format!("Runs on one node at a time and stops at the first failure: {on}."));
    }
    im
}

/// "cell1", "cell1 and cell2", "cell1, cell2 and cell3"
pub fn list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [a] => a.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The models a node helps serve: "Llama-3.3-70B-Instruct (llama-70b on cell1, cell2, cell3)".
/// A container belongs to a deployment by its `sparkbox.deployment` label or Compose project;
/// the model comes from the `sparkbox.model` label or a container's command (a Ray worker
/// has none of its own, so the group's head supplies it).
pub fn serving(cluster: &[NodeSnapshot], node: &NodeSnapshot) -> Vec<String> {
    let group_of = |c: &crate::model::Container| c.labels.get("sparkbox.deployment").cloned().or_else(|| c.project.clone());
    let model_of = |c: &crate::model::Container| c.labels.get("sparkbox.model").cloned().or_else(|| model_arg(&c.command));
    let mut out: Vec<String> = Vec::new();
    for c in node.containers.iter().filter(|c| c.state == "running") {
        let (model, members) = match group_of(c) {
            Some(g) => {
                let members: Vec<(&str, &crate::model::Container)> =
                    cluster.iter().flat_map(|n| n.containers.iter().map(move |c| (n.name.as_str(), c))).filter(|(_, m)| group_of(m).as_deref() == Some(g.as_str())).collect();
                let model = members.iter().find_map(|(_, m)| model_of(m));
                let mut nodes: Vec<&str> = members.iter().map(|(n, _)| *n).collect();
                nodes.dedup();
                (model, if nodes.len() > 1 { format!("{g} on {}", nodes.join(", ")) } else { c.name.clone() })
            }
            None => (model_of(c), c.name.clone()),
        };
        let Some(model) = model else { continue };
        let short = model.rsplit('/').next().unwrap_or(&model).to_string();
        let line = format!("{short} ({members})");
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

/// The model from "vllm serve <model>", "--model <model>" and the like; the served name
/// (often lowercased) only when nothing better is there.
fn model_arg(command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let after = |flag: &str| words.iter().position(|w| *w == flag).and_then(|i| words.get(i + 1)).filter(|w| !w.starts_with('-')).map(|s| s.to_string());
    after("--model")
        .or_else(|| after("--model-path"))
        .or_else(|| words.iter().find_map(|w| w.strip_prefix("--model=")).map(String::from))
        .or_else(|| after("serve"))
        .or_else(|| after("--served-model-name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_blocks_parse_into_facts() {
        let text = "Id=anacron.service\nDescription=Run anacron jobs\nLoadState=loaded\nActiveState=inactive\nSubState=dead\nUnitFileState=enabled\n\
Type=oneshot\nTriggeredBy=anacron.timer\nResult=success\nConditionResult=yes\n\nId=gone.service\nLoadState=not-found\n\n\
Id=ollama.service\nDescription=Ollama Service\nLoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nType=simple\n\
TriggeredBy=\nResult=success\nConditionResult=yes\n";
        let units = parse_show(text);
        assert_eq!(units.len(), 2, "a unit that isn't there is left out");
        assert_eq!((units[0].kind.as_str(), units[0].triggered_by.clone(), units[0].condition), ("oneshot", vec!["anacron.timer".to_string()], Some(true)));
        assert_eq!((units[1].unit.as_str(), units[1].active.as_str(), units[1].triggered_by.len()), ("ollama.service", "active", 0));
    }

    #[test]
    fn a_clean_exit_reads_as_success() {
        let anacron = "○ anacron.service - Run anacron jobs\n     Loaded: loaded (/usr/lib/systemd/system/anacron.service; enabled; preset: enabled)\n\
     Active: inactive (dead) since Thu 2026-10-29 09:33:33 EDT; 4min ago\n   Main PID: 1591 (code=exited, status=0/SUCCESS)\n";
        assert_eq!(parse_status("anacron.service", anacron, vec![]).result.as_deref(), Some("success"));
        let failed = "× x.service - X\n     Active: failed (Result: exit-code) since Thu 2026-10-29 09:33:33 EDT; 4min ago\n   Main PID: 2211 (code=exited, status=1/FAILURE)\n";
        assert_eq!(parse_status("x.service", failed, vec![]).result.as_deref(), Some("exit-code · status 1/FAILURE"));
        let killed = "○ y.service - Y\n     Active: inactive (dead)\n   Main PID: 7 (code=killed, signal=TERM)\n";
        assert_eq!(parse_status("y.service", killed, vec![]).result, None);
    }

    #[tokio::test]
    async fn one_batched_show_reads_a_real_sparks_units() {
        use crate::fake_node::{connect_sparks, fake_spark, temp};
        let (node, port) = fake_spark(&temp("svc-gx10"), "gx10-0000").await;
        let (live, _) = connect_sparks(&temp("svc-gx10-app"), &[("gx10-0000", port)]).await;
        let out = live.probe("gx10-0000", SHOW_UNITS, std::time::Duration::from_secs(20)).await.unwrap();
        let units = parse_show(&out.stdout);
        assert_eq!(units.len(), 88, "every service on gx10-0000, from one command");
        let anacron = units.iter().find(|u| u.unit == "anacron.service").unwrap();
        assert_eq!((anacron.active.as_str(), anacron.kind.as_str(), anacron.triggered_by.clone()), ("inactive", "oneshot", vec!["anacron.timer".to_string()]));
        let nd = units.iter().find(|u| u.unit == "networkd-dispatcher.service").unwrap();
        assert_eq!((nd.active.as_str(), nd.condition), ("inactive", Some(false)), "skipped by its condition, not stopped");
        assert_eq!(node.ran("systemctl show").len(), 1);
        live.stop();
    }
}

