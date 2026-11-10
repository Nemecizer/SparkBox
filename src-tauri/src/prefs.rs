//! App-wide preferences, saved as prefs.json in the config folder.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::models::ModelSettings;
use crate::util;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DataSource {
    /// The built-in simulator.
    #[default]
    Demo,
    /// SSH to the real Sparks.
    Live,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Closing the window leaves SparkBox running in the menu bar.
    pub keep_in_menu_bar: bool,
    pub data_source: DataSource,
    /// Model folder, vLLM image and the AI that checks whether a model fits.
    pub models: ModelSettings,
    /// Masks public IP addresses and the home folder wherever they're shown, for screenshots.
    pub screenshot_safe: bool,
    /// Explicit opt-in to paid, read-only manufacturer lookups for observed models.
    pub hardware_auto_lookup: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { keep_in_menu_bar: true, data_source: DataSource::Demo, models: ModelSettings::default(), screenshot_safe: false, hardware_auto_lookup: false }
    }
}

impl Prefs {
    /// Loads prefs.json; one that can't be read is set aside (and reported) and defaults are used.
    pub fn load(path: &Path) -> Prefs {
        util::load_json_or_default(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        util::save_json(path, self)
    }

    /// These prefs with `patch`'s fields laid over them (a `{field: value}` object; nested
    /// objects merge). Unknown fields are refused, so a typo doesn't pass silently.
    pub fn merged(&self, patch: &serde_json::Value) -> Result<Prefs, String> {
        let serde_json::Value::Object(fields) = patch else { return Err("Preferences change as {field: value}.".into()) };
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        for (k, val) in fields {
            match v.get_mut(k) {
                Some(serde_json::Value::Object(cur)) if val.is_object() => {
                    for (kk, vv) in val.as_object().unwrap() {
                        cur.insert(kk.clone(), vv.clone());
                    }
                }
                Some(slot) => *slot = val.clone(),
                None => return Err(format!("There's no preference called “{k}”.")),
            }
        }
        serde_json::from_value(v).map_err(|e| format!("That preference doesn't take this value: {e}"))
    }

    /// Whether a patch touches settings that are Mac scope (the data source, the model settings).
    pub fn patch_needs_mac(patch: &serde_json::Value) -> bool {
        patch.as_object().is_some_and(|f| f.contains_key("dataSource") || f.contains_key("models") || f.contains_key("hardwareAutoLookup"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_changes_only_its_fields_and_says_its_scope() {
        let old = Prefs { screenshot_safe: true, ..Default::default() };
        let menu = serde_json::json!({ "keepInMenuBar": false });
        let p = old.merged(&menu).unwrap();
        assert!(!p.keep_in_menu_bar && p.screenshot_safe, "the other window's change survives");
        assert!(!Prefs::patch_needs_mac(&menu));
        let image = serde_json::json!({ "models": { "image": "nvcr.io/nvidia/vllm:99.0" } });
        let p = old.merged(&image).unwrap();
        assert_eq!(p.models.image, "nvcr.io/nvidia/vllm:99.0");
        assert_eq!(p.models.model_root, Prefs::default().models.model_root, "nested fields merge");
        assert!(Prefs::patch_needs_mac(&image) && Prefs::patch_needs_mac(&serde_json::json!({ "dataSource": "live" })));
        assert!(old.merged(&serde_json::json!({ "dataSorce": "live" })).unwrap_err().contains("dataSorce"));
        assert!(old.merged(&serde_json::json!({ "dataSource": "cloud" })).is_err());
    }

    #[test]
    fn prefs_save_atomically_and_a_broken_file_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("sparkbox-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let p = Prefs { data_source: DataSource::Live, ..Default::default() };
        p.save(&path).unwrap();
        assert_eq!(Prefs::load(&path).data_source, DataSource::Live);

        std::fs::write(&path, "{\"dataSource\": \"li").unwrap();
        assert_eq!(Prefs::load(&path).data_source, DataSource::Demo, "defaults, not a crash");
        let w = util::warning_for(&path).expect("reported for the banner");
        assert!(std::path::Path::new(&w.backup).exists() && w.backup.contains("prefs.json.corrupt-"));
        util::clear_warning(&path);
    }
}
