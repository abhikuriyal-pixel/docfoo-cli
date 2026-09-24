//! `docfoo notes` — read-only notes access.

use serde_json::json;

use crate::cli::NotesArgs;
use crate::error::{CliError, Result};
use crate::notes;
use crate::output::{self, OutputFormat};
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &NotesArgs) -> Result<()> {
    let workspace_display = workspace.root.display().to_string();

    if args.list {
        let entries = notes::list(workspace, args.resource.as_deref())?;
        let notes: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                let mut value = entry.note.clone();
                if let Some(object) = value.as_object_mut() {
                    object.insert("resource".to_string(), json!(entry.resource));
                }
                value
            })
            .collect();
        if format.is_json() {
            return output::success(
                format,
                "notes.list",
                &workspace_display,
                json!({ "total": notes.len(), "notes": notes }),
            );
        }
        if notes.is_empty() {
            println!("no notes found");
            return Ok(());
        }
        for note in &notes {
            let id = note.get("id").and_then(serde_json::Value::as_str).unwrap_or("?");
            let resource = note
                .get("resource")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let text = note
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .replace('\n', " ");
            let preview: String = text.chars().take(80).collect();
            println!("{id}  {resource}  {preview}");
        }
        return Ok(());
    }

    if let Some(id) = &args.read {
        let entry = notes::find(workspace, id)?;
        if format.is_json() {
            return output::success(
                format,
                "notes.read",
                &workspace_display,
                json!({ "resource": entry.resource, "note": entry.note }),
            );
        }
        let note = &entry.note;
        println!("id: {}", note.get("id").and_then(serde_json::Value::as_str).unwrap_or("?"));
        println!("resource: {}", entry.resource);
        if let Some(line) = note.get("sourceLine").and_then(serde_json::Value::as_u64) {
            println!("line: {line}");
        }
        if let Some(original) = note.get("originalText").and_then(serde_json::Value::as_str) {
            if !original.is_empty() {
                println!("quote: {original}");
            }
        }
        if let Some(images) = note.get("images").and_then(serde_json::Value::as_array) {
            for image in images {
                if let Some(path) = image.get("path").and_then(serde_json::Value::as_str) {
                    println!("image: {path}");
                }
            }
        }
        println!(
            "\n{}",
            note.get("text").and_then(serde_json::Value::as_str).unwrap_or("")
        );
        return Ok(());
    }

    Err(CliError::Usage(
        "notes needs --list or --read".to_string(),
    ))
}
