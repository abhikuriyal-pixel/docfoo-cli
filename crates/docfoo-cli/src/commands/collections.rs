//! `docfoo collections` — list and download community collections.

use serde_json::json;

use crate::cli::{CollectionKind, CollectionsArgs};
use crate::collections::{client, zip_util, CollectionItem};
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &CollectionsArgs) -> Result<()> {
    let workspace_display = workspace.root.display().to_string();
    let base = crate::collections::server_url();

    if args.list {
        let items = client::list(&base)?;
        if format.is_json() {
            return output::success(
                format,
                "collections.list",
                &workspace_display,
                json!({ "total": items.len(), "items": items }),
            );
        }
        if items.is_empty() {
            println!("no collections available");
            return Ok(());
        }
        for item in &items {
            println!(
                "{}  {}  {}  {}  {} downloads",
                item.id,
                item.kind,
                item.name,
                human_bytes(item.size),
                item.downloads
            );
        }
        return Ok(());
    }

    if let Some(needle) = &args.info {
        let items = client::list(&base)?;
        let item = find_item(&items, needle, args.kind)?;
        if format.is_json() {
            return output::success(
                format,
                "collections.info",
                &workspace_display,
                json!({ "item": item }),
            );
        }
        println!("id: {}", item.id);
        println!("type: {}", item.kind);
        println!("name: {}", item.name);
        if !item.rel.is_empty() {
            println!("rel: {}", item.rel);
        }
        println!("size: {}", human_bytes(item.size));
        println!("files: {}", item.files);
        println!("downloads: {}", item.downloads);
        if !item.description.is_empty() {
            println!("description: {}", item.description);
        }
        return Ok(());
    }

    if let Some(needle) = &args.download {
        if args.name.is_some() && args.kind == Some(CollectionKind::Kg) {
            return Err(CliError::Usage(
                "--name only applies to resource collections".to_string(),
            ));
        }
        let items = client::list(&base)?;
        let item = find_item(&items, needle, args.kind)?;
        std::fs::create_dir_all(workspace.tmp_dir())?;
        let dest = workspace.tmp_dir().join("collections-item.zip");
        let mut last_reported = 0u64;
        client::download(&base, &item.id, &dest, &mut |received, total| {
            if received.saturating_sub(last_reported) >= 8 * 1024 * 1024 || received == total {
                last_reported = received;
                if total > 0 {
                    eprintln!("downloading {received}/{total} bytes");
                } else {
                    eprintln!("downloading {received} bytes");
                }
            }
        })?;
        let installed = zip_util::extract(
            &dest,
            &workspace.root,
            &item.kind,
            args.force,
            args.name.as_deref(),
        )
        .map_err(CliError::Message)?;
        let _ = std::fs::remove_file(&dest);
        let message = format!("installed {} collection \"{}\"", item.kind, installed);
        if format.is_json() {
            return output::success(
                format,
                "collections.download",
                &workspace_display,
                json!({
                    "id": item.id,
                    "kind": item.kind,
                    "name": installed,
                    "message": message,
                }),
            );
        }
        println!("{message}");
        return Ok(());
    }

    Err(CliError::Usage(
        "collections needs --list, --info or --download".to_string(),
    ))
}

fn find_item(
    items: &[CollectionItem],
    needle: &str,
    kind: Option<CollectionKind>,
) -> Result<CollectionItem> {
    let mut candidates: Vec<&CollectionItem> = items
        .iter()
        .filter(|item| item.id == needle || item.name.eq_ignore_ascii_case(needle))
        .collect();
    if let Some(kind) = kind {
        let wanted = match kind {
            CollectionKind::Resource => "resource",
            CollectionKind::Kg => "kg",
        };
        candidates.retain(|item| item.kind == wanted);
    }
    match candidates.len() {
        0 => Err(CliError::NotFound(format!(
            "no collection matches \"{needle}\""
        ))),
        1 => Ok(candidates[0].clone()),
        _ => Err(CliError::Message(format!(
            "\"{needle}\" matches {} collections — use the id",
            candidates.len()
        ))),
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
