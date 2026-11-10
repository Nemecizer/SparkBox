//! Tauri commands for the VPN console beyond the settings ones in lib.rs.

use tauri::{AppHandle, Manager};

use super::VpnView;
use crate::{admin, AppState};

/// Connect or disconnect now regardless of the mode ("connect" | "disconnect"), or follow
/// the mode again (null). Not saved: the next launch follows the mode.
#[tauri::command]
pub async fn vpn_override(app: AppHandle, action: Option<String>) -> Result<VpnView, String> {
    admin::require_mac(&app)?;
    let connect = match action.as_deref() {
        Some("connect") => Some(true),
        Some("disconnect") => Some(false),
        None | Some("auto") => None,
        Some(other) => return Err(format!("unknown VPN action {other}")),
    };
    let vpn = app.state::<AppState>().vpn.clone();
    let view = vpn.set_override(connect).await;
    let what = match connect {
        Some(true) => "VPN: connect now",
        Some(false) => "VPN: disconnect now",
        None => "VPN: follow the mode",
    };
    app.state::<AppState>().live.audit.record_app(what, "");
    Ok(view)
}
