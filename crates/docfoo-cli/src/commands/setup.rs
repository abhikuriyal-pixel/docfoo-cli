//! `docfoo setup` — provision scan's native dependencies.

use serde_json::json;

use crate::cli::SetupArgs;
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::setup;
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &SetupArgs) -> Result<()> {
    let workspace_display = workspace.root.display().to_string();

    if args.check {
        let status = setup::check(workspace);
        let message = if status.ready() {
            "scan dependencies are ready".to_string()
        } else {
            format!("missing: {}", status.missing().join(", "))
        };
        let mut data = status.to_json();
        data["message"] = json!(message);
        return output::success(format, "setup", &workspace_display, data);
    }

    let status = setup::provision(
        workspace,
        args.from.as_deref(),
        args.force,
        args.layout_model_url.as_deref(),
    )?;
    if !status.ready() {
        return Err(CliError::Message(format!(
            "setup is incomplete — missing {}. On Linux the shared libraries download automatically; the layout model needs `--from <DocFoo/models>` or `--layout-model-url <url>`.",
            status.missing().join(", ")
        )));
    }
    let mut data = status.to_json();
    data["message"] = json!("scan dependencies are ready");
    output::success(format, "setup", &workspace_display, data)
}
