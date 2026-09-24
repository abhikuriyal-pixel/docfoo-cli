//! `model-selection.json` — the same four model slots the desktop app uses.
//!
//! Port of `src-tauri/src/config/mod.rs` model preferences: unknown keys are
//! preserved, only the known slots are written, empty values remove a slot,
//! and `scanConcurrency` is clamped to 1..=20.

use std::path::Path;

use serde_json::{Map, Value};

use crate::error::{CliError, Result};
use crate::util::atomic_write;
use crate::workspace::Workspace;

pub const MODEL_PREFERENCES_FILE: &str = "model-selection.json";
pub const SCAN_CONCURRENCY_KEY: &str = "scanConcurrency";
pub const MIN_SCAN_CONCURRENCY: u64 = 1;
pub const MAX_SCAN_CONCURRENCY: u64 = 20;

/// `(slot, json key)` pairs. The slot names are the CLI's `model --get/--set`
/// vocabulary; the keys are the app's on-disk fields.
pub const MODEL_SLOTS: [(&str, &str); 4] = [
    ("chat", "chatModelKey"),
    ("scan", "scanModelKey"),
    ("scan-analysis", "scanAnalysisModelKey"),
    ("kg", "kgModelKey"),
];

pub fn slot_key(slot: &str) -> Result<&'static str> {
    MODEL_SLOTS
        .iter()
        .find(|(name, _)| *name == slot)
        .map(|(_, key)| *key)
        .ok_or_else(|| {
            CliError::Usage(format!(
                "unknown model slot \"{slot}\" (expected chat, scan, scan-analysis or kg)"
            ))
        })
}

#[derive(Debug, Clone, Default)]
pub struct ModelPreferences {
    values: Map<String, Value>,
}

impl ModelPreferences {
    pub fn from_value(value: Value) -> Self {
        match value {
            Value::Object(map) => Self { values: map },
            _ => Self::default(),
        }
    }

    pub fn load(workspace: &Workspace) -> Self {
        let path = workspace.model_selection_path();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .map(Self::from_value)
            .unwrap_or_default()
    }

    pub fn get(&self, slot: &str) -> Result<Option<&str>> {
        let key = slot_key(slot)?;
        Ok(self.values.get(key).and_then(Value::as_str))
    }

    /// Set a slot; an empty value removes it (same as the app's settings UI).
    pub fn set(&mut self, slot: &str, value: &str) -> Result<()> {
        let key = slot_key(slot)?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            self.values.remove(key);
        } else {
            self.values
                .insert(key.to_string(), Value::String(trimmed.to_string()));
        }
        Ok(())
    }

    pub fn scan_concurrency(&self) -> Option<u64> {
        self.values.get(SCAN_CONCURRENCY_KEY).and_then(Value::as_u64)
    }

    /// Apply a partial preferences object, mirroring the app's
    /// `set_model_preferences` merge (unknown keys are left untouched).
    pub fn apply(&mut self, preferences: &Value) {
        let Some(incoming) = preferences.as_object() else {
            return;
        };
        for (_, key) in MODEL_SLOTS {
            if let Some(value) = incoming.get(key).and_then(Value::as_str) {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    self.values.remove(key);
                } else {
                    self.values
                        .insert(key.to_string(), Value::String(trimmed.to_string()));
                }
            }
        }
        if let Some(value) = incoming
            .get(SCAN_CONCURRENCY_KEY)
            .and_then(Value::as_u64)
            .filter(|value| (MIN_SCAN_CONCURRENCY..=MAX_SCAN_CONCURRENCY).contains(value))
        {
            self.values
                .insert(SCAN_CONCURRENCY_KEY.to_string(), Value::from(value));
        }
    }

    pub fn as_value(&self) -> Value {
        Value::Object(self.values.clone())
    }

    /// Atomic write; the caller is responsible for `ensure_scaffold` when the
    /// workspace may not exist yet.
    pub fn save(&self, workspace: &Workspace) -> Result<()> {
        let path = workspace.model_selection_path();
        write_preferences(&path, &self.as_value())
    }
}

fn write_preferences(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workspace(dir: &Path) -> Workspace {
        Workspace {
            root: dir.to_path_buf(),
            agent_dir: dir.join(".agent"),
            models_dir: dir.join("models"),
        }
    }

    #[test]
    fn set_get_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(temp.path());
        let mut prefs = ModelPreferences::default();
        prefs.set("kg", "openrouter/inception/mercury-2.5").unwrap();
        prefs.save(&workspace).unwrap();

        let loaded = ModelPreferences::load(&workspace);
        assert_eq!(
            loaded.get("kg").unwrap(),
            Some("openrouter/inception/mercury-2.5")
        );
        assert_eq!(loaded.get("chat").unwrap(), None);
    }

    #[test]
    fn empty_value_removes_slot() {
        let mut prefs = ModelPreferences::default();
        prefs.set("scan", "provider/model").unwrap();
        prefs.set("scan", "  ").unwrap();
        assert_eq!(prefs.get("scan").unwrap(), None);
    }

    #[test]
    fn unknown_slot_is_a_usage_error() {
        let prefs = ModelPreferences::default();
        let error = prefs.get("nope").unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("unknown model slot"));
    }

    #[test]
    fn apply_preserves_unknown_keys_and_rejects_bad_concurrency() {
        let mut prefs = ModelPreferences::from_value(json!({
            "kgModelKey": "old/kg",
            "somethingElse": 7
        }));
        prefs.apply(&json!({
            "kgModelKey": "new/kg",
            "chatModelKey": "chat/key",
            "scanConcurrency": 99
        }));
        let value = prefs.as_value();
        assert_eq!(value["kgModelKey"], "new/kg");
        assert_eq!(value["chatModelKey"], "chat/key");
        assert_eq!(value["somethingElse"], 7);
        assert!(value.get("scanConcurrency").is_none());
    }

    #[test]
    fn apply_accepts_valid_concurrency() {
        let mut prefs = ModelPreferences::default();
        prefs.apply(&json!({ "scanConcurrency": 4 }));
        assert_eq!(prefs.scan_concurrency(), Some(4));
    }

    #[test]
    fn load_missing_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(temp.path());
        assert_eq!(ModelPreferences::load(&workspace).as_value(), json!({}));
    }
}
