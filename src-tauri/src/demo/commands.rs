//! Tauri commands for the simulator: the scenario picker (Settings › Demo scenarios).
//! Scenarios change only the simulated world, never a real node, so they don't need Admin mode.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, State};

use super::{DemoCluster, ScenarioInfo};

#[tauri::command]
pub fn demo_scenarios_list(demo: State<'_, Arc<Mutex<DemoCluster>>>) -> Vec<ScenarioInfo> {
    demo.lock().unwrap().scenarios()
}

/// Plays a scenario (None or "default" for the everyday world); returns the one now playing.
/// Entering or leaving the as-found world takes the Demo nodes' files with it.
#[tauri::command]
pub fn demo_scenario_set(app: AppHandle, demo: State<'_, Arc<Mutex<DemoCluster>>>, id: Option<String>) -> Result<Option<String>, String> {
    let (now, world) = {
        let mut d = demo.lock().unwrap();
        d.set_scenario(id.as_deref())?;
        (d.scenario(), d.world())
    };
    if let Some(state) = app.try_state::<crate::AppState>() {
        state.files.demo_fs().lock().unwrap().set_world(world);
    }
    crate::publish(&app, false);
    Ok(now)
}
