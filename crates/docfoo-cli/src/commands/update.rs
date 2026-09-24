//! `docfoo update` — check for and install a newer release.

use serde_json::json;

use crate::cli::UpdateArgs;
use crate::error::Result;
use crate::output::{self, OutputFormat};
use crate::update;
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &UpdateArgs) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let workspace_display = workspace.root.display().to_string();
    let release = update::check()?;

    let Some(release) = release else {
        return output::success(
            format,
            "update",
            &workspace_display,
            json!({
                "current": current,
                "latest": null,
                "updateAvailable": false,
                "message": format!("no releases found for {}", update::repo()),
            }),
        );
    };

    if !release.is_newer_than(current) {
        return output::success(
            format,
            "update",
            &workspace_display,
            json!({
                "current": current,
                "latest": release.version,
                "updateAvailable": false,
                "message": format!("docfoo {current} is up to date"),
            }),
        );
    }

    if args.check {
        return output::success(
            format,
            "update",
            &workspace_display,
            json!({
                "current": current,
                "latest": release.version,
                "updateAvailable": true,
                "asset": release.asset_name,
                "message": format!("docfoo {} is available (current {current})", release.version),
            }),
        );
    }

    let install_dir = update::self_update(&release)?;
    output::success(
        format,
        "update",
        &workspace_display,
        json!({
            "current": current,
            "latest": release.version,
            "updateAvailable": false,
            "installedTo": install_dir.display().to_string(),
            "message": format!("updated to docfoo {}", release.version),
        }),
    )
}
