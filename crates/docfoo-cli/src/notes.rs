//! Read-only notes — `notes.json` is a map of resource rel → note objects.
//!
//! The CLI never writes notes (Stage 2 is read-only); this module only loads,
//! filters and looks up.

use serde_json::{Map, Value};

use crate::error::{CliError, Result};
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
}
