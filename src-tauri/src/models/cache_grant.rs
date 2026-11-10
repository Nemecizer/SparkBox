//! Explicit, revocable permission in the Keychain. It covers only the existing conditional
//! cache-drop step of Keep running; the normal write exit still checks pins, leases and fences.
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use crate::{admin, models_ctx, secrets, AppState};

fn account() -> String { format!("permission:keep-running-drop-caches-v1:{}", crate::identity::instance_name()) }
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grant {
    pub version: u32,
    pub instance: String,
    pub nodes: Vec<String>,
    pub granted_ms: u64,
}
impl Grant {
    fn covers<'a>(&self, nodes: impl IntoIterator<Item = &'a str>) -> bool {
        let nodes: Vec<_> = nodes.into_iter().collect();
        self.version == 1 && self.instance == crate::identity::instance_name() && !nodes.is_empty()
            && nodes.iter().all(|n| self.nodes.iter().any(|g| g == n))
    }
}
fn read() -> Option<Grant> {
    secrets::get(&account()).ok().flatten().and_then(|s| serde_json::from_str(&s).ok())
}
pub(super) fn covers<'a>(nodes: impl IntoIterator<Item = &'a str>) -> bool {
    !admin::look_only() && read().is_some_and(|g| g.covers(nodes))
}

#[tauri::command]
pub fn drop_cache_grant_get() -> Option<Grant> { read().filter(|g| g.instance == crate::identity::instance_name() && g.version == 1) }

#[tauri::command]
pub fn drop_cache_grant_set(app: AppHandle, state: State<AppState>, enabled: bool) -> Result<Option<Grant>, String> {
    admin::require_mac_confirmed(&app, "drop_cache_grant_set")?;
    let ctx = models_ctx(&state);
    if !ctx.live { return Err("Standing permission can only be changed with Live data selected. Demo never grants it.".into()); }
    let grant = if enabled {
        let nodes = ctx.node_ids();
        if nodes.is_empty() { return Err("There are no inventory nodes to grant.".into()); }
        let g = Grant { version: 1, instance: crate::identity::instance_name().into(), nodes, granted_ms: super::now_ms() };
        secrets::set(&account(), &serde_json::to_string(&g).map_err(|e| e.to_string())?)?;
        Some(g)
    } else { secrets::clear(&account())?; None };
    state.models.live.audit.record(crate::live::audit::Entry {
        node: "mac".into(), kind: "permission".into(), title: if enabled { "Grant conditional page-cache drops to Keep running" } else { "Revoke Keep running's page-cache grant" }.into(),
        command: super::plan::DROP_CACHES.into(), state: "ok".into(), ..Default::default()
    });
    Ok(grant)
}

#[cfg(test)]
pub(super) fn test_grant(nodes: &[&str]) -> impl Drop {
    struct Clear; impl Drop for Clear { fn drop(&mut self) { let _ = secrets::clear(&account()); } }
    let grant = Grant { version:1, instance:crate::identity::instance_name().into(), nodes:nodes.iter().map(|s|s.to_string()).collect(), granted_ms:1 };
    secrets::set(&account(), &serde_json::to_string(&grant).unwrap()).unwrap(); Clear
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grant_is_versioned_instance_scoped_and_cannot_cover_new_nodes() {
        let mut g = Grant { version: 1, instance: crate::identity::instance_name().into(), nodes: vec!["cell1".into()], granted_ms: 1 };
        assert!(g.covers(["cell1"])); assert!(!g.covers(["cell2"])); assert!(!g.covers([]));
        g.version = 2; assert!(!g.covers(["cell1"]));
        g.version = 1; g.instance = "some-other-instance".into(); assert!(!g.covers(["cell1"]));
        assert!(serde_json::from_str::<Grant>(r#"{"enabled":true}"#).is_err());
    }
}
