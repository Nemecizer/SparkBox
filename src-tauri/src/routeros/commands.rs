//! Tauri commands for switches: test the connection, trust a certificate, and the port
//! actions (enable or disable, comment, reset counters, identify). Each change needs
//! Admin mode and goes into the activity log; in Demo they act on the simulator.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use super::{user_of, Switches};
use crate::inventory::{SwitchConfig, SwitchKind};
use crate::probe::{self, ProbeHistory, Prober};
use crate::{admin, AppState};

/// What "Test connection" found.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchTestResult {
    pub ok: bool,
    pub board: Option<String>,
    pub version: Option<String>,
    /// SHA-256 of the switch's certificate (64 hex digits), to compare and trust.
    pub fingerprint: Option<String>,
    pub error_kind: Option<String>,
    pub message: String,
}

fn config(app: &AppHandle, switch_id: &str) -> Result<SwitchConfig, String> {
    app.state::<AppState>()
        .inventory
        .lock()
        .unwrap()
        .switches
        .iter()
        .find(|s| s.id == switch_id)
        .cloned()
        .ok_or_else(|| format!("{switch_id} isn't in the inventory."))
}

fn is_live(app: &AppHandle) -> bool {
    crate::is_live(&app.state::<AppState>())
}

fn audit(app: &AppHandle, title: &str, detail: &str) {
    app.state::<AppState>().live.audit.record_app(title, detail);
}

/// Tries the switch's REST API (or, for the Firewalla devices, a probe) and explains the result.
#[tauri::command]
pub async fn switch_test(app: AppHandle, switch_id: String) -> Result<SwitchTestResult, String> {
    let cfg = config(&app, &switch_id)?;
    if !is_live(&app) {
        let snap = app.state::<AppState>().latest.read().unwrap().clone();
        let s = snap.and_then(|s| s.switches.into_iter().find(|s| s.id == switch_id));
        return Ok(SwitchTestResult {
            ok: true,
            board: s.as_ref().and_then(|s| s.board.clone()),
            version: s.as_ref().and_then(|s| s.version.clone()),
            fingerprint: None,
            error_kind: None,
            message: "Demo data: the simulated switch answers. Switch to Live data to test the real one.".into(),
        });
    }
    if cfg.host.trim().is_empty() {
        return Ok(SwitchTestResult { error_kind: Some("not-configured".into()), message: format!("{} has no address yet.", cfg.name), ..Default::default() });
    }
    if cfg.kind != SwitchKind::RouterOs {
        let prober = app.state::<Arc<Prober>>().inner().clone();
        prober.probe(&cfg.id, cfg.host.trim()).await;
        let h = prober.history(&cfg.id);
        let last = h.samples.last().and_then(|s| s.ms);
        return Ok(match last {
            Some(ms) => SwitchTestResult { ok: true, board: Some(cfg.name.clone()), message: format!("{} answered in {ms:.1} ms.", cfg.name), ..Default::default() },
            None => SwitchTestResult { error_kind: Some("timeout".into()), message: format!("{} didn't answer on port 443 or 80.", cfg.name), ..Default::default() },
        });
    }
    let switches = app.state::<Arc<Switches>>().inner().clone();
    Ok(test_routeros(&switches, &cfg).await)
}

pub async fn test_routeros(switches: &Switches, cfg: &SwitchConfig) -> SwitchTestResult {
    let client = match switches.client(cfg).await {
        Ok(c) => c,
        Err(e) => return SwitchTestResult { error_kind: Some("auth".into()), message: e, ..Default::default() },
    };
    let via = switches.via_vpn().await;
    match client.resource().await {
        Ok(r) => SwitchTestResult {
            ok: true,
            message: format!("Connected to {} as {} (RouterOS {}).", r.board.as_deref().unwrap_or("the switch"), user_of(cfg), r.version.as_deref().unwrap_or("?")),
            board: r.board,
            version: r.version,
            fingerprint: client.seen_fingerprint(),
            error_kind: None,
        },
        Err(e) => SwitchTestResult {
            error_kind: Some(e.kind().into()),
            message: e.explain(&user_of(cfg), cfg.host.trim(), via),
            fingerprint: e.cert_to_trust().map(String::from),
            ..Default::default()
        },
    }
}

/// Pins the switch's certificate after the user compared the fingerprint, then tests again.
#[tauri::command]
pub async fn switch_trust(app: AppHandle, switch_id: String, fingerprint: String) -> Result<SwitchTestResult, String> {
    admin::require_mac(&app)?;
    let cfg = config(&app, &switch_id)?;
    let switches = app.state::<Arc<Switches>>().inner().clone();
    switches.trust(cfg.host.trim(), &fingerprint)?;
    audit(&app, &format!("Trusted the certificate of {}", cfg.name), &format!("{} SHA-256 {}", cfg.host.trim(), fingerprint));
    Ok(test_routeros(&switches, &cfg).await)
}

/// A change to one port of a RouterOS switch.
enum PortAction<'a> {
    Comment(&'a str),
    Reset,
    Blink,
}

async fn live_port_action(app: &AppHandle, cfg: &SwitchConfig, port: &str, action: PortAction<'_>) -> Result<(), String> {
    if cfg.kind != SwitchKind::RouterOs {
        return Err(format!("{} is managed in the Firewalla app.", cfg.name));
    }
    let switches = app.state::<Arc<Switches>>().inner().clone();
    let client = switches.client(cfg).await?;
    let via = switches.via_vpn().await;
    let id = switches.port_id(cfg, port).await?;
    let (what, result) = match action {
        PortAction::Comment(c) => ("rename", client.set_comment(&id, c).await),
        PortAction::Reset => ("reset the counters of", client.reset_counters(port).await),
        PortAction::Blink => ("blink", client.blink(port).await),
    };
    result.map_err(|e| format!("Couldn't {what} {port}: {}", e.explain(&user_of(cfg), cfg.host.trim(), via)))
}

use super::safety::{self, bounce_script, check_phrase, revert_name, revert_script, valid_port, REVERT_S};

/// What a disable, enable or bounce did, for the window to follow up.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortChange {
    pub port: String,
    pub disabled: bool,
    /// When the switch will re-enable it (Unix ms), if a revert is scheduled.
    pub revert_at_ms: Option<u64>,
    /// "Keep disabled" is offered (never on the management path).
    pub keep_allowed: bool,
    pub message: String,
}

/// The port from the latest snapshot, and why it's on the management path (if it is).
fn port_info(app: &AppHandle, switch_id: &str, port: &str) -> Result<(String, Option<String>), String> {
    if !valid_port(port) {
        return Err(format!("{port:?} isn't a port name."));
    }
    let snap = app.state::<AppState>().latest.read().unwrap().clone();
    let sw = snap.and_then(|s| s.switches.into_iter().find(|s| s.id == switch_id)).ok_or_else(|| format!("{switch_id} isn't in the inventory."))?;
    let p = sw.ports.iter().find(|p| p.name == port).ok_or_else(|| format!("{} has no port {port}.", sw.name))?;
    Ok((sw.name.clone(), p.management.clone()))
}

fn routeros_config(app: &AppHandle, switch_id: &str) -> Result<SwitchConfig, String> {
    let cfg = config(app, switch_id)?;
    if cfg.kind != SwitchKind::RouterOs {
        return Err(format!("{} is managed in the Firewalla app.", cfg.name));
    }
    Ok(cfg)
}

/// What the activity log shows for a disable: the revert left behind, then the disable.
fn disable_command(port: &str) -> String {
    format!(
        "/system scheduler add name={} start-date=<in {REVERT_S} s> start-time=<in {REVERT_S} s> interval=0s on-event=\"{}\"; /interface ethernet set {port} disabled=yes",
        revert_name(port),
        revert_script(port)
    )
}

/// Enables or disables a switch port (Demo or Live). A disable reverts on the switch
/// after 60 s unless "Keep disabled" follows; on the management path it needs
/// `confirm` = "disable 1-1" and can't be kept.
#[tauri::command]
pub async fn switch_port_set(app: AppHandle, switch_id: String, port: String, disabled: bool, confirm: Option<String>) -> Result<PortChange, String> {
    admin::require(&app)?;
    guarded_set(&app, &switch_id, &port, disabled, confirm.as_deref()).await
}

/// The guarded disable/enable behind `switch_port_set`.
pub async fn guarded_set(app: &AppHandle, switch_id: &str, port: &str, disabled: bool, confirm: Option<&str>) -> Result<PortChange, String> {
    let (sw_name, mgmt) = port_info(app, switch_id, port)?;
    if disabled {
        check_phrase(port, mgmt.as_deref(), "disable", confirm)?;
    }
    let title = format!("{} {port} on {sw_name}", if disabled { "Disabled" } else { "Enabled" });
    let command = if disabled { disable_command(port) } else { format!("/interface ethernet set {port} disabled=no") };
    let at = if is_live(app) {
        let cfg = routeros_config(app, switch_id)?;
        let switches = app.state::<Arc<Switches>>().inner().clone();
        let result = if disabled { safety::disable(&switches, &cfg, port).await.map(Some) } else { safety::enable(&switches, &cfg, port).await.map(|_| None) };
        match result {
            Ok(at) => {
                audit(app, &title, &command);
                at
            }
            Err(e) => {
                app.state::<AppState>().live.audit.record_failed("switch", &title, &command, &e);
                return Err(e);
            }
        }
    } else {
        let at = app.state::<AppState>().demo.lock().unwrap().set_port_disabled_for(switch_id, port, disabled, if disabled { REVERT_S } else { 0 })?;
        app.state::<AppState>().live.audit.record_simulated("switch", &title, &command);
        at
    };
    crate::publish(app, false);
    let short = crate::fabric::short_port(port);
    Ok(PortChange {
        port: port.to_string(),
        disabled,
        revert_at_ms: at,
        keep_allowed: disabled && mgmt.is_none(),
        message: match (disabled, mgmt.is_some()) {
            (true, true) => format!("{short} disabled; the switch re-enables it in {REVERT_S} s"),
            (true, false) => format!("{short} disabled; it comes back in {REVERT_S} s unless you keep it disabled"),
            _ => format!("{short} enabled"),
        },
    })
}

/// Cancels the revert of a disabled port, once SparkBox can still reach the switch.
/// Never for a management port.
#[tauri::command]
pub async fn switch_port_keep_disabled(app: AppHandle, switch_id: String, port: String) -> Result<PortChange, String> {
    admin::require(&app)?;
    let (sw_name, mgmt) = port_info(&app, &switch_id, &port)?;
    if mgmt.is_some() {
        return Err(format!("{port} carries SparkBox's connection to the switch, so it can't stay disabled."));
    }
    if is_live(&app) {
        let cfg = routeros_config(&app, &switch_id)?;
        let switches = app.state::<Arc<Switches>>().inner().clone();
        safety::keep_disabled(&switches, &cfg, &port).await?;
        audit(&app, &format!("Kept {port} disabled on {}", cfg.name), &format!("/system scheduler remove [find name={}]", revert_name(&port)));
    } else {
        app.state::<AppState>().demo.lock().unwrap().keep_port_disabled(&switch_id, &port)?;
        app.state::<AppState>().live.audit.record_simulated("switch", &format!("Kept {port} disabled on {sw_name}"), &format!("/system scheduler remove [find name={}]", revert_name(&port)));
    }
    crate::publish(&app, false);
    Ok(PortChange { port: port.clone(), disabled: true, revert_at_ms: None, keep_allowed: false, message: format!("{} stays disabled", crate::fabric::short_port(&port)) })
}

/// Bounces a port with one script on the switch (down 3 s, then up), so the port comes
/// back even if SparkBox loses its connection. The management path needs `confirm` = "bounce 1-1".
#[tauri::command]
pub async fn switch_port_bounce(app: AppHandle, switch_id: String, port: String, confirm: Option<String>) -> Result<PortChange, String> {
    admin::require(&app)?;
    let (sw_name, mgmt) = port_info(&app, &switch_id, &port)?;
    check_phrase(&port, mgmt.as_deref(), "bounce", confirm.as_deref())?;
    let script = bounce_script(&port);
    if is_live(&app) {
        let cfg = routeros_config(&app, &switch_id)?;
        let switches = app.state::<Arc<Switches>>().inner().clone();
        let title = format!("Bounced {port} on {}", cfg.name);
        match safety::bounce(&switches, &cfg, &port, mgmt.is_some()).await {
            Ok(()) => audit(&app, &title, &script),
            Err(e) => {
                app.state::<AppState>().live.audit.record_failed("switch", &title, &script, &e);
                return Err(e);
            }
        }
    } else {
        app.state::<AppState>().demo.lock().unwrap().bounce_port(&switch_id, &port)?;
        app.state::<AppState>().live.audit.record_simulated("switch", &format!("Bounced {port} on {sw_name}"), &script);
    }
    crate::publish(&app, false);
    Ok(PortChange { port: port.clone(), disabled: false, revert_at_ms: None, keep_allowed: false, message: format!("{} bounced; the switch brings it back in 3 s", crate::fabric::short_port(&port)) })
}

/// Who the switch sees behind its ports (bridge hosts and LLDP/MNDP neighbors).
#[tauri::command]
pub fn switch_tables(app: AppHandle, switch_id: String) -> Result<super::mapping::PortTables, String> {
    if is_live(&app) {
        return Ok(app.state::<Arc<Switches>>().tables(&switch_id).unwrap_or_default());
    }
    Ok(app.state::<AppState>().demo.lock().unwrap().switch_tables(&switch_id))
}

#[tauri::command]
pub async fn switch_port_comment(app: AppHandle, switch_id: String, port: String, comment: String) -> Result<(), String> {
    admin::require(&app)?;
    let cfg = config(&app, &switch_id)?;
    let comment: String = comment.chars().filter(|c| !c.is_control()).take(120).collect();
    if is_live(&app) {
        live_port_action(&app, &cfg, &port, PortAction::Comment(&comment)).await?;
    } else {
        app.state::<AppState>().demo.lock().unwrap().set_port_comment(&switch_id, &port, &comment)?;
    }
    audit(&app, &format!("Changed the comment on {port} ({})", cfg.name), &format!("comment=\"{comment}\""));
    crate::publish(&app, false);
    Ok(())
}

#[tauri::command]
pub async fn switch_port_reset(app: AppHandle, switch_id: String, port: String) -> Result<(), String> {
    admin::require(&app)?;
    let cfg = config(&app, &switch_id)?;
    if is_live(&app) {
        live_port_action(&app, &cfg, &port, PortAction::Reset).await?;
    } else {
        app.state::<AppState>().demo.lock().unwrap().reset_port_counters(&switch_id, &port)?;
    }
    audit(&app, &format!("Reset the counters on {port} ({})", cfg.name), "/interface reset-counters");
    crate::publish(&app, false);
    Ok(())
}

/// Blinks the port's LED for 10 seconds so it can be found in the rack.
#[tauri::command]
pub async fn switch_port_blink(app: AppHandle, switch_id: String, port: String) -> Result<(), String> {
    admin::require(&app)?;
    let cfg = config(&app, &switch_id)?;
    if is_live(&app) {
        live_port_action(&app, &cfg, &port, PortAction::Blink).await?;
    }
    audit(&app, &format!("Blinked {port} on {}", cfg.name), "/interface ethernet blink duration=10s");
    Ok(())
}

/// Latency, loss, uptime and the last outage of a probed device (the Firewalla Switch X and Gold).
#[tauri::command]
pub fn switch_probe_history(app: AppHandle, prober: State<'_, Arc<Prober>>, switch_id: String) -> Result<ProbeHistory, String> {
    if is_live(&app) {
        return Ok(prober.history(&switch_id));
    }
    let snap = app.state::<AppState>().latest.read().unwrap().clone();
    let sw = snap.and_then(|s| s.switches.into_iter().find(|s| s.id == switch_id)).ok_or_else(|| format!("{switch_id} isn't in the inventory."))?;
    Ok(probe::demo_history(&sw, sw_now()))
}

fn sw_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
