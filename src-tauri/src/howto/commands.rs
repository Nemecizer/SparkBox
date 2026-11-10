use super::*;
use crate::{admin, models_ctx, AppState};
use tauri::{AppHandle, State};

fn scope(state: &AppState, demo: bool) -> Result<(), String> {
    if demo == crate::is_live(state) { Err("The data source changed. Reopen How To before continuing.".into()) } else { Ok(()) }
}
#[tauri::command]
pub fn howto_document(path: String) -> Result<Document, String> {
    documents().get(&path).cloned().ok_or_else(|| "That document is not bundled. Open its pinned source link instead.".into())
}
#[tauri::command]
pub fn howto_saved(state: State<AppState>, store: State<HowTo>, demo: bool) -> Result<BTreeMap<String, serde_json::Value>, String> {
    scope(&state, demo)?;
    Ok(store.saved(demo)?.into_iter().map(|(id, g)| {
        let title = g.sources.last().and_then(|s| s.document.text.lines().find_map(|l| l.strip_prefix("# "))).unwrap_or(&id).to_owned();
        (id, serde_json::json!({"favorite": g.favorite, "completed": g.completed.len(), "editions": g.editions.len(), "title": title}))
    }).collect())
}
#[tauri::command]
pub fn howto_read(state: State<AppState>, store: State<HowTo>, demo: bool, id: String) -> Result<LocalGuide, String> { scope(&state, demo)?; store.read(demo, &id) }
#[tauri::command]
pub fn howto_save(state: State<AppState>, store: State<HowTo>, demo: bool, request: Save) -> Result<LocalGuide, String> { scope(&state, demo)?; store.save(demo, request) }
#[tauri::command]
pub fn howto_last_check(state: State<AppState>, store: State<HowTo>, demo: bool) -> Result<Option<source::Check>, String> { scope(&state, demo)?; store.last_check(demo) }
#[tauri::command]
pub async fn howto_check_source(state: State<'_, AppState>, store: State<'_, HowTo>, demo: bool) -> Result<source::Check, String> {
    scope(&state, demo)?;
    let _one = store.network.try_lock().map_err(|_| "A source request is already running. Wait for it to finish.")?;
    let result = if demo { source::demo_check() } else { source::Client::new()?.check().await? };
    scope(&state, demo)?; store.keep_check(demo, result.clone())?; Ok(result)
}
#[tauri::command]
pub async fn howto_fetch_source(state: State<'_, AppState>, store: State<'_, HowTo>, demo: bool, id: String, revision: String) -> Result<LocalGuide, String> {
    scope(&state, demo)?;
    let _one = store.network.try_lock().map_err(|_| "A source request is already running. Wait for it to finish.")?;
    let check = store.last_check(demo)?.filter(|c| c.revision == revision && c.demo == demo).ok_or("The checked revision changed. Check NVIDIA again before fetching.")?;
    let fetched = if demo { source::demo_fetch(&id)? } else { source::Client::new()?.fetch(&check, &id).await? };
    scope(&state, demo)?; store.keep_source(demo, &id, fetched)
}

/// Existing quick check: reads only, never starts the GPU test container.
#[tauri::command]
pub async fn howto_check_nodes(state: State<'_, AppState>, demo: bool, id: String, nodes: Vec<String>) -> Result<Vec<crate::models::NodeCheck>, String> {
    scope(&state, demo)?;
    if !guide(&id).is_some_and(|g| g.spark) { return Err("This guide has no Spark prerequisite check.".into()); }
    let ctx = models_ctx(&state);
    validate_nodes(&ctx.node_ids(), &nodes)?;
    state.models.howto_checks(&ctx, &nodes).await
}
fn validate_nodes(known: &[String], nodes: &[String]) -> Result<(), String> {
    if nodes.is_empty() || nodes.len() > 16 || nodes.iter().any(|n| !known.contains(n)) || nodes.iter().collect::<std::collections::BTreeSet<_>>().len() != nodes.len() { return Err("Choose distinct nodes from the current inventory.".into()); }
    Ok(())
}
pub(super) fn image_for(id: &str, image: &str) -> Result<(), String> {
    if !guide(id).is_some_and(|g| g.spark && g.images.iter().any(|i| i == image)) { return Err("That image is not a reviewed action for this Spark guide. Source updates and AI text cannot add execution actions.".into()); }
    Ok(())
}
/// A narrow wrapper over existing owned image jobs. No installer, shell, model load or cleanup.
#[tauri::command]
pub async fn howto_prepare_image(app: AppHandle, state: State<'_, AppState>, demo: bool, id: String, image: String, nodes: Vec<String>, estimated_bytes: u64) -> Result<Vec<crate::models::Job>, String> {
    admin::require(&app)?;
    scope(&state, demo)?; image_for(&id, &image)?;
    let ctx = models_ctx(&state); validate_nodes(&ctx.node_ids(), &nodes)?;
    crate::ops::protected_updates::check_load(&state.live, demo, &nodes)?;
    // Existing sparkjob writes retain fresh authentication, lease, identity pin, fence and audit.
    state.models.image_jobs(&ctx, &image, &nodes, None, estimated_bytes).await
}
