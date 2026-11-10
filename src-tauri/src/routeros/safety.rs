//! Port actions that can't leave SparkBox cut off from the switch.
//!
//! - A disable first leaves a `/system/scheduler` entry on the switch that re-enables
//!   the port 60 s later (and removes itself); only then is the port disabled. If the
//!   revert can't be scheduled, nothing is disabled. "Keep disabled" removes the entry,
//!   and only once the switch still answers.
//! - A bounce is one `/system/script` on the switch: down, 3 s, up, remove itself. It
//!   finishes even when the port carries SparkBox's own connection.
//! - On a port that carries SparkBox's management path both need the exact phrase
//!   typed ("disable 1-1"), and such a disable can never be kept.

use super::client::ApiError;
use super::{user_of, Switches};
use crate::inventory::SwitchConfig;

/// Seconds after a disable that the switch re-enables the port by itself, unless kept.
pub const REVERT_S: u32 = 60;

/// Port names reach RouterOS scripts: letters, digits and - _ . / only.
pub fn valid_port(name: &str) -> bool {
    !name.is_empty() && name.len() <= 48 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

/// "disable 1-1": what has to be typed to touch a management port.
pub fn phrase(verb: &str, port: &str) -> String {
    format!("{verb} {}", crate::fabric::short_port(port))
}

pub fn revert_name(port: &str) -> String {
    format!("sparkbox-revert-{port}")
}

pub fn bounce_name(port: &str) -> String {
    format!("sparkbox-bounce-{port}")
}

/// The scheduler entry a disable leaves on the switch: re-enable, then remove itself.
pub fn revert_script(port: &str) -> String {
    format!("/interface ethernet enable [find name=\"{port}\"]; /system scheduler remove [find name=\"{}\"]", revert_name(port))
}

/// The bounce, as one script on the switch.
pub fn bounce_script(port: &str) -> String {
    format!(
        "/interface ethernet disable [find name=\"{port}\"]; :delay 3s; /interface ethernet enable [find name=\"{port}\"]; /system script remove [find name=\"{}\"]",
        bounce_name(port)
    )
}

/// Refuses to touch the management path unless the exact phrase was typed.
pub fn check_phrase(port: &str, management: Option<&str>, verb: &str, confirm: Option<&str>) -> Result<(), String> {
    let Some(why) = management else { return Ok(()) };
    let want = phrase(verb, port);
    if confirm.map(str::trim) == Some(want.as_str()) {
        return Ok(());
    }
    Err(format!("{port} carries SparkBox's connection to the switch. {why} To {verb} it anyway, type \"{want}\"; the switch undoes it by itself."))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn explainer(cfg: &SwitchConfig, via: bool) -> impl Fn(ApiError) -> String + '_ {
    move |e: ApiError| e.explain(&user_of(cfg), cfg.host.trim(), via)
}

/// Leaves the revert on the switch, then disables. Returns when the switch will re-enable it.
pub async fn disable(switches: &Switches, cfg: &SwitchConfig, port: &str) -> Result<u64, String> {
    if !valid_port(port) {
        return Err(format!("{port:?} isn't a port name."));
    }
    let client = switches.client(cfg).await?;
    let explain = explainer(cfg, switches.via_vpn().await);
    let id = switches.port_id(cfg, port).await?;
    let name = revert_name(port);
    let _ = client.unschedule(&name).await;
    client.schedule_once(&name, REVERT_S, &revert_script(port)).await.map_err(|e| format!("Couldn't leave a revert on the switch, so {port} wasn't disabled: {}", explain(e)))?;
    if let Err(e) = client.set_disabled(&id, true).await {
        let _ = client.unschedule(&name).await;
        return Err(format!("Couldn't disable {port}: {}", explain(e)));
    }
    let at = now_ms() + REVERT_S as u64 * 1000;
    switches.set_revert(&cfg.id, port, Some(at));
    Ok(at)
}

/// Enables a port and drops any revert still waiting for it.
pub async fn enable(switches: &Switches, cfg: &SwitchConfig, port: &str) -> Result<(), String> {
    if !valid_port(port) {
        return Err(format!("{port:?} isn't a port name."));
    }
    let client = switches.client(cfg).await?;
    let explain = explainer(cfg, switches.via_vpn().await);
    let id = switches.port_id(cfg, port).await?;
    client.set_disabled(&id, false).await.map_err(|e| format!("Couldn't enable {port}: {}", explain(e)))?;
    let _ = client.unschedule(&revert_name(port)).await;
    switches.set_revert(&cfg.id, port, None);
    Ok(())
}

/// Removes the revert, once the switch still answers (otherwise the revert is what brings it back).
pub async fn keep_disabled(switches: &Switches, cfg: &SwitchConfig, port: &str) -> Result<(), String> {
    let client = switches.client(cfg).await?;
    let explain = explainer(cfg, switches.via_vpn().await);
    client.resource().await.map_err(|e| format!("{} doesn't answer, so the revert stays: {}", cfg.name, explain(e)))?;
    client.unschedule(&revert_name(port)).await.map_err(&explain)?;
    switches.set_revert(&cfg.id, port, None);
    Ok(())
}

/// Runs the bounce script on the switch. On the management path the reply may never
/// come back; the script still finishes there, so that isn't an error.
pub async fn bounce(switches: &Switches, cfg: &SwitchConfig, port: &str, management: bool) -> Result<(), String> {
    if !valid_port(port) {
        return Err(format!("{port:?} isn't a port name."));
    }
    let client = switches.client(cfg).await?;
    let explain = explainer(cfg, switches.via_vpn().await);
    match client.run_script(&bounce_name(port), &bounce_script(port)).await {
        Ok(()) => Ok(()),
        Err(ApiError::Timeout | ApiError::Unreachable(_)) if management => Ok(()),
        Err(e) => Err(format!("Couldn't bounce {port}: {}", explain(e))),
    }
}
