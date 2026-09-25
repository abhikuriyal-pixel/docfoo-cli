//! Notes — `notes.json` is a map of resource rel → note objects.
//!
//! `docfoo notes` reads them; the resource browser (`resources --vis`) writes
//! them (create, edit, delete) in the desktop app's schema, so a note jotted
//! in the browser appears in the desktop app and vice versa. Writes are
//! whole-file read-modify-write with an atomic rename, and every other
//! resource's notes and unknown fields are preserved.

use serde_json::{Map, Value};

use crate::error::{CliError, Result};
use crate::resources::read;
use crate::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct NoteWithResource {
    pub resource: String,
    pub note: Value,
}

pub fn load(workspace: &Workspace) -> Result<Map<String, Value>> {
    let path = workspace.notes_path();
    if !path.is_file() {
        return Ok(Map::new());
    }
    let raw = std::fs::read_to_string(&path)?;
    let value: Value = serde_json::from_str(&raw)?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(CliError::Message(
            "notes.json is not a JSON object".to_string(),
        )),
    }
}

/// Every note, optionally filtered to one resource rel path.
pub fn list(workspace: &Workspace, resource: Option<&str>) -> Result<Vec<NoteWithResource>> {
    let notes = load(workspace)?;
    let mut out = Vec::new();
    for (rel, value) in notes {
        if let Some(filter) = resource {
            if rel != filter {
                continue;
            }
        }
        if let Some(array) = value.as_array() {
            for note in array {
                out.push(NoteWithResource {
                    resource: rel.clone(),
                    note: note.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// One note by its id, wherever it lives.
pub fn find(workspace: &Workspace, id: &str) -> Result<NoteWithResource> {
    for entry in list(workspace, None)? {
        if entry.note.get("id").and_then(Value::as_str) == Some(id) {
            return Ok(entry);
        }
    }
    Err(CliError::NotFound(format!("note not found: {id}")))
}

/// Create or replace one note for a resource, keyed by note `id`.
///
/// The stored object follows the desktop schema — `id`, `type`, `text`,
/// `originalText`, `filePath`, `created`, `anchor`, `sourceLine` — so the
/// desktop app re-anchors and lists it exactly like one of its own.
pub fn upsert(workspace: &Workspace, rel: &str, note: &Value) -> Result<Vec<Value>> {
    let rel = resolve_resource(workspace, rel)?;
    let object = note
        .as_object()
        .ok_or_else(|| CliError::Message("note must be a JSON object".to_string()))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CliError::Message("note id must not be empty".to_string()))?
        .to_string();
    if !object.get("text").map(Value::is_string).unwrap_or(false) {
        return Err(CliError::Message("note text must be a string".to_string()));
    }

    let mut map = load(workspace)?;
    let mut notes = map
        .get(&rel)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let created = notes
        .iter()
        .find(|candidate| candidate.get("id").and_then(Value::as_str) == Some(id.as_str()))
        .and_then(|candidate| candidate.get("created").and_then(Value::as_i64))
        .or_else(|| object.get("created").and_then(Value::as_i64))
        .unwrap_or_else(now_millis);

    let mut stored = object.clone();
    stored.insert("id".to_string(), Value::String(id.clone()));
    stored.insert(
        "type".to_string(),
        Value::String(
            object
                .get("type")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("NOTE")
                .to_string(),
        ),
    );
    stored.insert("filePath".to_string(), Value::String(rel.clone()));
    stored.insert("created".to_string(), Value::from(created));
    if !stored.contains_key("originalText") {
        stored.insert("originalText".to_string(), Value::String(String::new()));
    }

    match notes
        .iter_mut()
        .find(|candidate| candidate.get("id").and_then(Value::as_str) == Some(id.as_str()))
    {
        Some(slot) => *slot = Value::Object(stored),
        None => notes.push(Value::Object(stored)),
    }
    map.insert(rel, Value::Array(notes.clone()));
    save(workspace, &map)?;
    Ok(notes)
}

/// Remove one note by id; returns the resource's remaining notes.
pub fn delete(workspace: &Workspace, rel: &str, id: &str) -> Result<Vec<Value>> {
    let rel = resolve_resource(workspace, rel)?;
    let mut map = load(workspace)?;
    let mut notes = map
        .get(&rel)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let before = notes.len();
    notes.retain(|candidate| candidate.get("id").and_then(Value::as_str) != Some(id));
    if notes.len() == before {
        return Err(CliError::NotFound(format!("note not found: {id}")));
    }
    map.insert(rel, Value::Array(notes.clone()));
    save(workspace, &map)?;
    Ok(notes)
}

/// A note may only point at a real file inside `resources/`.
fn resolve_resource(workspace: &Workspace, rel: &str) -> Result<String> {
    let (normalized, path) = read::resolve_rel(&workspace.resources_dir(), rel)?;
    if !path.is_file() {
        return Err(CliError::NotFound(format!(
            "resource not found: {normalized}"
        )));
    }
    Ok(normalized)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Whole-file write with an atomic rename so a crash cannot truncate notes.
fn save(workspace: &Workspace, map: &Map<String, Value>) -> Result<()> {
    let path = workspace.notes_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!("{}\n", serde_json::to_string_pretty(map)?);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
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
    fn lists_and_filters_notes() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(temp.path());
        std::fs::write(
            workspace.notes_path(),
            r#"{
                "a/content.md": [{"id":"n1","text":"first"}],
                "b/content.md": [{"id":"n2","text":"second"}]
            }"#,
        )
        .unwrap();
        assert_eq!(list(&workspace, None).unwrap().len(), 2);
        let filtered = list(&workspace, Some("b/content.md")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].note["id"], "n2");
        assert_eq!(find(&workspace, "n1").unwrap().resource, "a/content.md");
        assert!(find(&workspace, "missing").is_err());
    }

    #[test]
    fn missing_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        assert!(list(&workspace(temp.path()), None).unwrap().is_empty());
    }

    #[test]
    fn upsert_create_edit_and_delete_roundtrip() {
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace(temp.path());
        std::fs::create_dir_all(workspace.resources_dir().join("Book")).unwrap();
        std::fs::write(workspace.resources_dir().join("Book/content.md"), "# hi").unwrap();
        std::fs::write(
            workspace.notes_path(),
            r#"{"Other/content.md":[{"id":"x","text":"keep"}]}"#,
        )
        .unwrap();

        let note = json!({
            "id": "ann-1",
            "text": "first",
            "originalText": "hi",
            "sourceLine": 1,
            "anchor": { "startLine": 0, "startCol": 0, "endLine": 0, "endCol": 2 }
        });
        let notes = upsert(&workspace, "Book/content.md", &note).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0]["type"], "NOTE");
        assert_eq!(notes[0]["filePath"], "Book/content.md");
        let created = notes[0]["created"].clone();
        assert!(created.as_i64().unwrap() > 0);

        // Editing keeps the id and original created stamp, and never touches
        // another resource's notes.
        let edited = upsert(
            &workspace,
            "Book/content.md",
            &json!({"id":"ann-1","text":"second","originalText":"hi","sourceLine":1}),
        )
        .unwrap();
        assert_eq!(edited.len(), 1);
        assert_eq!(edited[0]["text"], "second");
        assert_eq!(edited[0]["created"], created);
        let map = load(&workspace).unwrap();
        assert_eq!(map["Other/content.md"][0]["text"], "keep");

        let left = delete(&workspace, "Book/content.md", "ann-1").unwrap();
        assert!(left.is_empty());
        assert!(delete(&workspace, "Book/content.md", "ann-1").is_err());
        assert!(upsert(&workspace, "Missing/content.md", &note).is_err());
        assert!(upsert(&workspace, "../escape.md", &note).is_err());
        assert!(upsert(&workspace, "Book/content.md", &json!({"id":"","text":"x"})).is_err());
        assert!(upsert(&workspace, "Book/content.md", &json!({"id":"ann-2"})).is_err());
    }
}
