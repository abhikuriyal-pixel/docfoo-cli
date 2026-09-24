//! `docfoo backup` — create a `docfoo-backup` v1 zip.

use serde_json::json;

use crate::backup;
use crate::cli::BackupArgs;
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::util::now_ms;
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &BackupArgs) -> Result<()> {
    workspace.ensure_scaffold()?;
    let out = match &args.out {
        Some(path) => crate::workspace::absolutize(path)?,
        None => std::env::current_dir()?.join(format!("docfoo-backup-{}.zip", now_ms())),
    };
    let stats = backup::zip::create_backup_zip(&workspace.root, &workspace.agent_dir, &out)
        .map_err(CliError::Message)?;
    let bytes = std::fs::metadata(&out).map(|meta| meta.len()).unwrap_or(0);
    let message = format!(
        "backup written to {} ({} resources, {} sessions, {} notes)",
        out.display(),
        stats.resources,
        stats.sessions,
        stats.notes
    );
    let data = json!({
        "file": out.display().to_string(),
        "bytes": bytes,
        "resources": stats.resources,
        "sessions": stats.sessions,
        "notes": stats.notes,
        "chatDirs": stats.chat_dirs,
        "createdAt": now_ms(),
        "message": message,
    });
    output::success(format, "backup", &workspace.root.display().to_string(), data)
}
