//! `docfoo restore <FILE>` — restore a `docfoo-backup` v1 zip.

use std::io::Write;

use serde_json::json;

use crate::backup;
use crate::cli::RestoreArgs;
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &RestoreArgs) -> Result<()> {
    if !args.file.is_file() {
        return Err(CliError::NotFound(format!(
            "backup file not found: {}",
            args.file.display()
        )));
    }
    if !args.yes {
        eprint!(
            "Restore {} and replace the current workspace? [y/N] ",
            args.file.display()
        );
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            return Err(CliError::Message("restore cancelled".to_string()));
        }
    }
    workspace.ensure_scaffold()?;
    let stats = backup::zip::restore_from_zip(&args.file, &workspace.root, &workspace.agent_dir)
        .map_err(CliError::Message)?;
    let message = format!(
        "restored {} ({} resources, {} sessions, {} notes)",
        args.file.display(),
        stats.resources,
        stats.sessions,
        stats.notes
    );
    let data = json!({
        "file": args.file.display().to_string(),
        "resources": stats.resources,
        "sessions": stats.sessions,
        "notes": stats.notes,
        "chatDirs": stats.chat_dirs,
        "message": message,
    });
    output::success(format, "restore", &workspace.root.display().to_string(), data)
}
