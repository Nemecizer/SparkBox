//! Tauri commands for the Fabric window: its state, accepting the current cabling,
//! and the doctor (run as a cancellable task), with perftest installs and baselines.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};

use super::doctor::{self, Baselines, BoxFut, DoctorReport, DoctorRequest, Out, Runner};
use super::{FabricMonitor, FabricState};
use crate::demo::DemoCluster;
use crate::live::{CancelToken, Live};
use crate::tasks::{TaskHandle, TaskSpec, Tasks};
use crate::{admin, AppState};

pub const DOCTOR_EVENT: &str = "fabric://doctor";

/// The doctor's last report and the saved baselines.
pub struct Doctor {
    path: Option<PathBuf>,
    report: Mutex<Option<DoctorReport>>,
    baselines: Mutex<Baselines>,
    running: Mutex<Option<String>>,
}

impl Doctor {
    pub fn new(path: Option<PathBuf>) -> Arc<Doctor> {
        let baselines = path.as_deref().map(crate::util::load_json_or_default::<Baselines>).unwrap_or_default();
        Arc::new(Doctor { path, report: Mutex::new(None), baselines: Mutex::new(baselines), running: Mutex::new(None) })
    }
}

/// SSH, each command in the activity log; the perftest client stops on cancel.
pub struct LiveRunner {
    pub live: Arc<Live>,
    pub cancel: CancelToken,
}

impl Runner for LiveRunner {
    fn run<'a>(&'a self, node: &'a str, cmd: String, limit: Duration, stoppable: bool) -> BoxFut<'a, Result<Out, String>> {
        Box::pin(async move {
            let out = if stoppable { self.live.run_cancellable(node, &cmd, None, limit, self.cancel.clone()).await } else { self.live.run(node, &cmd, None, limit).await }?;
            Ok(Out { code: out.code, stdout: out.stdout, stderr: out.stderr })
        })
    }
}

/// The simulator answers with real tool output; each command is logged as simulated.
pub struct DemoRunner {
    pub demo: Arc<Mutex<DemoCluster>>,
    /// For its activity log.
    pub live: Arc<Live>,
    pub title: String,
}

impl Runner for DemoRunner {
    fn run<'a>(&'a self, node: &'a str, cmd: String, _limit: Duration, _stoppable: bool) -> BoxFut<'a, Result<Out, String>> {
        Box::pin(async move {
            let (code, stdout, stderr) = self.demo.lock().unwrap().fleet_reply(node, &cmd);
            self.live.audit.record_simulated(node, &self.title, &cmd);
            Ok(Out { code: Some(code), stdout, stderr })
        })
    }

    fn pause<'a>(&'a self, what: &'a str) -> BoxFut<'a, ()> {
        // Roughly how long the real thing takes, shortened: a 5 s perftest pair shows for a second.
        let ms = match what {
            "pair" => 900,
            "server" => 250,
            "ping" => 90,
            _ => 300,
        };
        Box::pin(tokio::time::sleep(Duration::from_millis(ms)))
    }
}

struct TaskProgress {
    app: AppHandle,
    task: TaskHandle,
}

impl doctor::Progress for TaskProgress {
    fn update(&self, report: &DoctorReport) {
        let total = report.bandwidth.len() + report.ping.len() + report.mtu.len();
        if total > 0 {
            let done = report.bandwidth.iter().filter(|c| c.status != doctor::Status::Running).count() + report.ping.len() + report.mtu.len();
            self.task.progress(done as f32 / (total as f32 + 1.0), report.current.clone());
        }
        if let Some(d) = self.app.try_state::<Arc<Doctor>>() {
            *d.report.lock().unwrap() = Some(report.clone());
        }
        let _ = self.app.emit(DOCTOR_EVENT, report);
    }

    fn cancelled(&self) -> bool {
        self.task.is_cancelled()
    }

    fn step(&self, label: &str) {
        self.task.step(label);
    }
}

/// The fabric's latest assessment (null until the first snapshot).
#[tauri::command]
pub fn fabric_state(mon: State<'_, Arc<FabricMonitor>>) -> Option<FabricState> {
    mon.state()
}

/// Takes the cabling every switch shows now as the baseline. In Live it's saved to
/// fabric_cabling.json on this Mac; in Demo it holds until the scenario changes.
#[tauri::command]
pub fn fabric_cabling_accept(app: AppHandle) -> Result<usize, String> {
    admin::require_mac(&app)?;
    let state = app.state::<AppState>();
    let live = crate::is_live(&state);
    let scenario = state.demo.lock().unwrap().scenario();
    let n = app.state::<Arc<FabricMonitor>>().accept_cabling(!live, scenario)?;
    state.live.audit.record_app("Fabric cabling accepted", &format!("{n} fabric interfaces{}", if live { "" } else { " (Demo)" }));
    crate::publish(&app, false);
    Ok(n)
}

/// The doctor's last report (null before the first run).
#[tauri::command]
pub fn fabric_doctor_report(doctor: State<'_, Arc<Doctor>>) -> Option<DoctorReport> {
    doctor.report.lock().unwrap().clone()
}

/// Saved per-pair baselines, "cell1>cell2/1" → Gb/s.
#[tauri::command]
pub fn fabric_baselines(doctor: State<'_, Arc<Doctor>>) -> Baselines {
    doctor.baselines.lock().unwrap().clone()
}

/// Forgets the baselines; the next all-pass run sets new ones.
#[tauri::command]
pub fn fabric_baselines_reset(app: AppHandle) -> Result<(), String> {
    admin::require_mac(&app)?;
    let doctor = app.state::<Arc<Doctor>>();
    *doctor.baselines.lock().unwrap() = Baselines::new();
    if let Some(p) = &doctor.path {
        crate::util::save_json(p, &Baselines::new())?;
    }
    app.state::<AppState>().live.audit.record_app("Fabric baselines cleared", "");
    Ok(())
}

/// Starts "Test fabric" as a task; returns its id. Cancel goes through the task.
#[tauri::command]
pub async fn fabric_doctor_start(app: AppHandle, request: DoctorRequest) -> Result<String, String> {
    admin::require(&app)?;
    let state = app.state::<AppState>();
    let snap = state.latest.read().unwrap().clone().ok_or("No snapshot yet")?;
    if request.nodes.len() < 2 && (request.checks.ping || request.checks.bandwidth || request.checks.latency || request.checks.nccl) {
        return Err("Choose at least two nodes: the tests run between pairs.".into());
    }
    for n in &request.nodes {
        if !snap.nodes.iter().any(|x| &x.id == n) {
            return Err(format!("{n} isn't in the inventory."));
        }
    }
    if !(request.checks.mtu || request.checks.ping || request.checks.bandwidth || request.checks.latency || request.checks.nccl) {
        return Err("Choose at least one check.".into());
    }
    let doctor = app.state::<Arc<Doctor>>().inner().clone();
    {
        let mut running = doctor.running.lock().unwrap();
        if let Some(id) = running.as_ref() {
            return Err(format!("The fabric test is already running (task {id})."));
        }
        let tasks = app.state::<Arc<Tasks>>().inner().clone();
        let task = tasks.start(TaskSpec::new("fabric", "Test fabric", &request.nodes).cancellable());
        *running = Some(task.id().to_string());
        let live = crate::is_live(&state);
        let live_c = state.live.clone();
        let what = describe(&request);
        let op = live_c.audit.begin_op(&request.nodes.join(","), "Test fabric", &what, !live);
        let cancel = CancelToken::new();
        let runner: Box<dyn Runner> = if live {
            Box::new(LiveRunner { live: state.live.clone(), cancel: cancel.clone() })
        } else {
            Box::new(DemoRunner { demo: state.demo.clone(), live: live_c.clone(), title: "Test fabric".into() })
        };
        let stop = task.cancelled();
        tauri::async_runtime::spawn(async move {
            stop.await;
            cancel.cancel();
        });
        let app2 = app.clone();
        let id = task.id().to_string();
        let baselines = doctor.baselines.lock().unwrap().clone();
        let doctor = doctor.clone();
        let expected = request.expected_gbps.filter(|g| *g > 0.0).unwrap_or(doctor::DEFAULT_EXPECTED_GBPS);
        let report = Mutex::new(DoctorReport {
            task_id: Some(id.clone()),
            started_ms: snap.ts_ms,
            demo: !live,
            nodes: request.nodes.clone(),
            checks: request.checks.clone(),
            expected_gbps: expected,
            ..Default::default()
        });
        tauri::async_runtime::spawn(async move {
            let progress = TaskProgress { app: app2.clone(), task: task.clone() };
            doctor::Progress::update(&progress, &report.lock().unwrap());
            doctor::run(runner.as_ref(), &snap, &request, &baselines, &report, &progress).await;
            let mut r = report.lock().unwrap().clone();
            if let Some(saved) = doctor::maybe_save_baselines(doctor.path.as_ref(), &baselines, &r) {
                *doctor.baselines.lock().unwrap() = saved;
                r.baseline_saved = true;
            }
            *doctor.report.lock().unwrap() = Some(r.clone());
            *doctor.running.lock().unwrap() = None;
            let _ = app2.emit(DOCTOR_EVENT, &r);
            let measured = r.bandwidth.iter().filter(|c| c.gbps.is_some()).count();
            let summary = if r.findings.is_empty() {
                format!("Everything passed ({measured} pairs measured)")
            } else {
                format!("{} finding{}: {}", r.findings.len(), if r.findings.len() == 1 { "" } else { "s" }, r.findings[0])
            };
            let audit = &live_c.audit;
            if r.cancelled {
                audit.end_op(&op, false, None, Some("Stopped".into()));
                task.finish_cancelled("Stopped; any perftest server was stopped too");
            } else {
                audit.end_op(&op, r.findings.is_empty(), Some(summary.clone()), (!r.findings.is_empty()).then(|| summary.clone()));
                task.finish_ok(summary);
            }
        });
        Ok(id)
    }
}

fn describe(r: &DoctorRequest) -> String {
    let mut c = Vec::new();
    if r.checks.mtu {
        c.push("MTU audit");
    }
    if r.checks.ping {
        c.push("jumbo ping");
    }
    if r.checks.bandwidth {
        c.push("ib_write_bw");
    }
    if r.checks.latency {
        c.push("ib_send_lat");
    }
    if r.checks.nccl {
        c.push("all_reduce_perf");
    }
    format!("{} on {}", c.join(", "), r.nodes.join(", "))
}

/// Installs perftest (`sudo apt-get install -y perftest`) on these nodes, as a task.
#[tauri::command]
pub async fn fabric_perftest_install(app: AppHandle, nodes: Vec<String>) -> Result<String, String> {
    admin::require(&app)?;
    let state = app.state::<AppState>();
    let snap = state.latest.read().unwrap().clone().ok_or("No snapshot yet")?;
    if nodes.is_empty() || nodes.iter().any(|n| !snap.nodes.iter().any(|x| &x.id == n)) {
        return Err("Choose nodes from the inventory.".into());
    }
    let tasks = app.state::<Arc<Tasks>>().inner().clone();
    let task = tasks.start(TaskSpec::new("fabric", "Install perftest", &nodes));
    let id = task.id().to_string();
    let live = crate::is_live(&state);
    let (live_c, demo) = (state.live.clone(), state.demo.clone());
    let name = |id: &str| snap.nodes.iter().find(|n| n.id == id).map(|n| n.name.clone()).unwrap_or_else(|| id.to_string());
    let names: Vec<(String, String)> = nodes.iter().map(|n| (n.clone(), name(n))).collect();
    tauri::async_runtime::spawn(async move {
        let mut failed = Vec::new();
        for (node, label) in &names {
            task.step(format!("Install perftest on {label}"));
            let result = if live {
                match live_c.sudo(node, doctor::INSTALL_CMD, Duration::from_secs(600)).await {
                    Ok(o) if o.code == Some(0) => {
                        // Verify: the tools are there now.
                        match live_c.run(node, doctor::PREREQ_CMD, None, Duration::from_secs(10)).await {
                            Ok(v) if v.stdout.lines().count() >= 2 => Ok(()),
                            _ => Err("apt-get finished but ib_write_bw still isn't on the PATH".to_string()),
                        }
                    }
                    Ok(o) => Err(crate::live::audit::tail(&o.stderr).unwrap_or_else(|| format!("apt-get exited with {:?}", o.code))),
                    Err(e) => Err(e),
                }
            } else {
                let mut d = demo.lock().unwrap();
                d.sudo_check(node).and_then(|_| d.install_perftest(node)).map(|_| {
                    live_c.audit.record_simulated(node, "Install perftest", &format!("sudo {}", doctor::INSTALL_CMD));
                })
            };
            match result {
                Ok(()) => task.step_ok(Some("ib_write_bw and ib_send_lat are installed".into())),
                Err(e) => {
                    task.step_failed(e.clone());
                    failed.push(format!("{label}: {e}"));
                }
            }
        }
        if failed.is_empty() {
            task.finish_ok(format!("perftest installed on {}", names.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join(", ")));
        } else {
            task.finish_err(failed.join("; "));
        }
    });
    Ok(id)
}
