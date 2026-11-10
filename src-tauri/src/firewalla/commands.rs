//! Tauri commands for the Switch X's panel. The panel's labels and the cloud's settings change
//! files on this Mac, so they're Mac-scope Admin actions; nothing here can reach the switch.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::labels::{self, PortLabel};
use super::{Firewalla, MspTestResult, PortsView};
use crate::inventory::{SwitchConfig, SwitchKind};
use crate::{admin, AppState};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn firewalla_switch(state: &AppState, switch_id: &str) -> Result<SwitchConfig, String> {
    let inventory = state.inventory.lock().unwrap();
    let sw = inventory.switches.iter().find(|s| s.id == switch_id).ok_or_else(|| format!("{switch_id} isn't in the inventory."))?;
    if sw.kind != SwitchKind::Firewalla {
        return Err(format!("{} has no port panel.", sw.name));
    }
    Ok(sw.clone())
}

/// The labels and the cloud's last answer. In Demo, samples: the real file and the cloud stay out of it.
#[tauri::command]
pub fn firewalla_view(app: AppHandle, switch_id: String) -> Result<PortsView, String> {
    let state = app.state::<AppState>();
    let cfg = firewalla_switch(&state, &switch_id)?;
    let fw = app.state::<Arc<Firewalla>>();
    let live = crate::is_live(&state);
    Ok(PortsView { labels: fw.labels_of(&cfg.id, live), msp: fw.status_for(live, admin::look_only(), now_ms()) })
}

/// Says what's plugged into a port, or (None) takes the label off. The switch is never told.
#[tauri::command]
pub fn firewalla_label_set(app: AppHandle, switch_id: String, port: u32, label: Option<PortLabel>) -> Result<(), String> {
    admin::require_mac(&app)?;
    let state = app.state::<AppState>();
    let cfg = firewalla_switch(&state, &switch_id)?;
    let nodes: Vec<String> = state.inventory.lock().unwrap().nodes.iter().map(|n| n.id.clone()).collect();
    let live = crate::is_live(&state);
    let saved = app.state::<Arc<Firewalla>>().set_label(&cfg.id, port, label.as_ref(), &nodes, now_ms(), live)?;
    let (title, what) = (format!("Labeled port {port} of {}", cfg.name), labels::words(saved.as_ref()));
    if live {
        state.live.audit.record_app(&title, &what);
    } else {
        state.live.audit.record_simulated("switch", &title, &what);
    }
    Ok(())
}

/// What Settings shows. The token isn't here: Settings asks `credential_status` whether it's saved.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MspSettings {
    pub domain: Option<String>,
}

#[tauri::command]
pub fn firewalla_msp_settings(app: AppHandle) -> MspSettings {
    MspSettings { domain: app.state::<Arc<Firewalla>>().domain() }
}

/// Saves the MSP domain: where the saved token goes, so it confirms every time, as the token does.
#[tauri::command]
pub fn firewalla_msp_set_domain(app: AppHandle, domain: String) -> Result<MspSettings, String> {
    admin::require_mac_confirmed(&app, "firewalla_msp_set_domain")?;
    let saved = app.state::<Arc<Firewalla>>().set_domain(&domain, now_ms())?;
    app.state::<AppState>().live.audit.record_app("Saved the Firewalla MSP domain", saved.as_deref().unwrap_or("none"));
    Ok(MspSettings { domain: saved })
}

/// Tries the saved domain and token against the cloud, GETs only. In Demo nothing is sent, and
/// look-only mode refuses it before it runs (`LOOK_ONLY_HOLDS`).
#[tauri::command]
pub async fn firewalla_msp_test(app: AppHandle) -> Result<MspTestResult, String> {
    let (live, want) = {
        let state = app.state::<AppState>();
        (crate::is_live(&state), super::wanted(&state))
    };
    let fw = app.state::<Arc<Firewalla>>().inner().clone();
    Ok(fw.test_connection(live, want, now_ms()).await)
}
