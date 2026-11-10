//! Tauri commands for the Files window beyond the original files_* set in lib.rs: the Trash,
//! New file, batch delete, previous versions, folder sizes and queued transfers. Everything that changes
//! a file needs Admin mode first: Mac scope on this Mac, node scope on a node.

use std::path::Path;

use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use super::mac::{self, Landing};
use super::transfer::{Op, Policy};
use super::{Backup, Emit, Leftover, Preflight, TransferEvent, TransferSpec, Trashed, Usage, MAC};
use crate::{admin, is_live, AppState};

/// Whether du or backups refuse `paths` on `place`: one is a keys-and-credentials path or inside
/// one (P1.46). A folder that only holds one, like ~/.cache/huggingface, can be measured.
pub fn measure_refused(place: &str, paths: &[String], home: &Path) -> bool {
    paths.iter().any(|p| mac::is_secret_at(place, p, home))
}

/// Whether a copy's preflight refuses `spec`: its folder is a secret path or inside one, or an
/// item is one or (on this Mac) holds one, since the preflight reads the items whole (P1.46).
/// The folder itself may hold a secret: copying ~/Documents out of ~ reads no key.
pub fn preflight_refused(spec: &TransferSpec, home: &Path) -> bool {
    let dir = spec.dir.trim_end_matches('/');
    mac::is_secret_at(&spec.from, &spec.dir, home) || spec.names.iter().any(|n| mac::secret_read(&spec.from, &format!("{dir}/{n}"), home))
}

#[tauri::command]
pub async fn files_trash(app: AppHandle, state: State<'_, AppState>, place: String, paths: Vec<String>) -> Result<Vec<Trashed>, String> {
    admin::require_for_place(&app, &place)?;
    let demo = !is_live(&state);
    state.files.trash(demo, &place, &paths).await
}

#[tauri::command]
pub async fn files_untrash(app: AppHandle, state: State<'_, AppState>, place: String, items: Vec<Trashed>) -> Result<(), String> {
    admin::require_for_place(&app, &place)?;
    let demo = !is_live(&state);
    state.files.untrash(demo, &place, &items).await
}

/// New file: an empty file called `name` in `dir`, never over anything already there. Mac scope
/// on this Mac, node scope on a node, as New folder; a Demo node says it needs Live data. Returns
/// the new file's path, for the editor.
#[tauri::command]
pub async fn files_new_file(app: AppHandle, state: State<'_, AppState>, place: String, dir: String, name: String) -> Result<String, String> {
    admin::require_for_place(&app, &place)?;
    let demo = !is_live(&state);
    if demo && place != MAC {
        return Err(super::LIVE_ONLY.into());
    }
    state.files.new_file(demo, &place, &dir, &name).await.map_err(|e| new_file_error(e, &name))
}

/// New file's error in words: when sparkjob.py found no folder to make it in (`{"error": "The
/// folder <dir> doesn't exist."}`, after the node's name), "Can't create notes.txt: the folder
/// ~/sparkbox-test/x doesn't exist."; any other error as it is.
pub fn new_file_error(e: String, name: &str) -> String {
    let reply = e.split_once(": ").map_or(e.as_str(), |(_, r)| r);
    let missing = serde_json::from_str::<serde_json::Value>(reply).ok().and_then(|v| Some(v["error"].as_str()?.strip_prefix("The folder ")?.strip_suffix(" doesn't exist.")?.to_string()));
    match missing {
        Some(dir) => format!("Can't create {name}: the folder {dir} doesn't exist."),
        None => e,
    }
}

#[tauri::command]
pub async fn files_backups(state: State<'_, AppState>, place: String, path: String) -> Result<Vec<Backup>, String> {
    if measure_refused(&place, std::slice::from_ref(&path), &state.files.mac_home()) {
        return Err(mac::SECRET_PATH.to_string());
    }
    let demo = !is_live(&state);
    state.files.backups(demo, &place, &path).await
}

#[tauri::command]
pub async fn files_du(state: State<'_, AppState>, place: String, paths: Vec<String>) -> Result<Vec<Usage>, String> {
    if measure_refused(&place, &paths, &state.files.mac_home()) {
        return Err(mac::SECRET_PATH.to_string());
    }
    let demo = !is_live(&state);
    state.files.du(demo, &place, &paths).await
}

#[tauri::command]
pub async fn files_preflight(state: State<'_, AppState>, spec: TransferSpec) -> Result<Preflight, String> {
    // The source is read whole to measure it: refuse one that is or holds keys and credentials.
    if preflight_refused(&spec, &state.files.mac_home()) {
        return Err(mac::SECRET_PATH.to_string());
    }
    let demo = !is_live(&state);
    state.files.preflight(demo, &spec).await
}

/// A copy from a node into ~/Downloads under new names arrives like a browser download
/// (quarantined, without execute bits) and needs no scope. Anything else changes something:
/// a folder on the Mac (it lands there, or replaces or merges), a node (it's the destination,
/// or a move takes the originals away) or the Mac's originals (a move from it).
pub fn transfer_changes(spec: &TransferSpec, home: &Path) -> bool {
    let new_names = spec.policies.values().all(|p| matches!(p, Policy::Skip | Policy::KeepBoth));
    let download = spec.to == MAC && spec.from != MAC && spec.op == Op::Copy && new_names && mac::landing(home, &spec.to_dir, &spec.names).ok() == Some(Landing::Downloads);
    !download
}

/// The place whose scope a transfer needs: the Mac whenever it lands there, takes files away
/// from it or reads its key folders; otherwise the node it lands on. Every item is checked:
/// one in ~/.ssh and the like, or one holding such a folder (copying ~, ~/Library, ~/.config
/// or /Users), is a Mac-scope read. Mac scope covers node scope too.
pub fn gate_place(spec: &TransferSpec, home: &Path) -> String {
    let secret = || mac::is_secret_path(home, &spec.dir) || spec.names.iter().any(|n| mac::holds_secrets(home, &format!("{}/{n}", spec.dir.trim_end_matches('/'))));
    let mac = spec.to == MAC || (spec.from == MAC && (spec.op == Op::Move || secret()));
    if mac { MAC.to_string() } else { spec.to.clone() }
}

#[tauri::command]
pub async fn files_transfer(app: AppHandle, state: State<'_, AppState>, spec: TransferSpec, on_event: Channel<TransferEvent>) -> Result<String, String> {
    if transfer_changes(&spec, &state.files.mac_home()) {
        admin::require_for_place(&app, &gate_place(&spec, &state.files.mac_home()))?;
    }
    let demo = !is_live(&state);
    let emit: Emit = Box::new(move |e| {
        let _ = on_event.send(e);
    });
    state.files.transfer(demo, spec, emit).await
}

/// Staging folders that transfers couldn't remove yet (the Files window offers Remove).
#[tauri::command]
pub fn files_leftovers(state: State<'_, AppState>) -> Vec<Leftover> {
    state.files.leftovers()
}

#[tauri::command]
pub async fn files_leftover_remove(app: AppHandle, state: State<'_, AppState>, place: String, path: String) -> Result<(), String> {
    admin::require_for_place(&app, &place)?;
    state.files.remove_leftover(&place, &path).await
}
