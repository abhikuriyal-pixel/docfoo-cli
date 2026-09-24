//! `kg-settings.json` persistence.
//!
//! Port of `src-tauri/src/kg/settings.rs`: nested `{ index, query }`, a legacy
//! flat file is migrated once, every load is clamped, and the vocabulary
//! fingerprint tracks schema changes that force a fresh rebuild.

use docfoo_kg::tunables::{IndexSettings, KgSettings};

use crate::error::Result;
use crate::util::atomic_write_json;
use crate::workspace::Workspace;

fn parse_settings(raw: &str) -> Option<KgSettings> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    if object.contains_key("index") || object.contains_key("query") {
        return serde_json::from_value(value).ok();
    }
    let index = serde_json::from_value::<IndexSettings>(serde_json::Value::Object(object.clone())).ok()?;
    let query = serde_json::from_value::<docfoo_kg::tunables::QueryTunables>(
        serde_json::Value::Object(object.clone()),
    )
    .ok()?;
    Some(KgSettings { index, query })
}

fn is_legacy(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .as_object()
                .map(|object| !object.contains_key("index") && !object.contains_key("query"))
        })
        .unwrap_or(false)
}

/// Load settings with defaults + clamping; a legacy flat file is rewritten in
/// the nested shape after the first successful load.
pub fn load_settings(workspace: &Workspace) -> KgSettings {
    let path = workspace.kg_settings_path();
    let raw = std::fs::read_to_string(&path).ok();
    let settings = raw
        .as_deref()
        .and_then(parse_settings)
        .unwrap_or_default()
        .clamped();
    if raw.as_deref().is_some_and(is_legacy) {
        let _ = save_settings(workspace, &settings);
    }
    settings
}

pub fn save_settings(workspace: &Workspace, settings: &KgSettings) -> Result<()> {
    atomic_write_json(
        &workspace.kg_settings_path(),
        &serde_json::to_value(settings)?,
    )
}

/// fnv1a-64 over `"T:<sorted types>|R:<sorted relations>"` — detects vocabulary
/// changes that require a fresh rebuild.
pub fn vocab_fingerprint(settings: &IndexSettings) -> String {
    let mut types = settings.entity_types.clone();
    types.sort();
    let mut relations = settings.relations.clone();
    relations.sort();
    let joined = format!("T:{}|R:{}", types.join(","), relations.join(","));
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in joined.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Persist the fingerprint of the vocabulary a successful build used.
pub fn save_vocab_fingerprint(workspace: &Workspace, fingerprint: &str) {
    let mut settings = load_settings(workspace);
    settings.index.vocab_fingerprint = fingerprint.to_string();
    let _ = save_settings(workspace, &settings);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use std::path::Path;

    fn workspace(root: &Path) -> Workspace {
        Workspace {
            root: root.to_path_buf(),
            agent_dir: root.join(".agent"),
            models_dir: root.join("models"),
        }
    }

    #[test]
    fn missing_file_yields_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let settings = load_settings(&workspace(temp.path()));
        assert!(!settings.index.entity_types.is_empty());
        assert_eq!(settings.index.vocab_fingerprint, "");
    }

    #[test]
    fn legacy_flat_file_is_migrated() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(temp.path());
        std::fs::write(
            workspace.kg_settings_path(),
            r#"{"entityTypes":["DEVICE","CONCEPT"],"relations":["USES"],"seedPoolK":21}"#,
        )
        .unwrap();
        let settings = load_settings(&workspace);
        assert_eq!(settings.index.entity_types, vec!["CONCEPT", "DEVICE"]);
        assert_eq!(settings.query.seed_pool_k, 21);
        // rewritten in the nested shape
        let raw = std::fs::read_to_string(workspace.kg_settings_path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(value.get("index").is_some());
        assert!(value.get("query").is_some());
    }

    #[test]
    fn vocab_fingerprint_is_stable_and_order_independent() {
        let a = IndexSettings {
            entity_types: vec!["B".into(), "A".into()],
            ..IndexSettings::default()
        };
        let b = IndexSettings {
            entity_types: vec!["A".into(), "B".into()],
            ..IndexSettings::default()
        };
        assert_eq!(vocab_fingerprint(&a), vocab_fingerprint(&b));
    }

    #[test]
    fn save_vocab_fingerprint_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(temp.path());
        save_vocab_fingerprint(&workspace, "abc123");
        assert_eq!(load_settings(&workspace).index.vocab_fingerprint, "abc123");
    }
}
