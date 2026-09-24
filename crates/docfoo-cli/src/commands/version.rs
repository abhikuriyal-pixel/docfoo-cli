use serde_json::json;

use crate::cli::VersionArgs;
use crate::error::Result;
use crate::output::{self, OutputFormat};
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &VersionArgs) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let workspace_display = workspace.root.display().to_string();

    if format.is_json() {
        let data = json!({
            "cliVersion": version,
            "sidecarVersion": null,
            "workspace": workspace_display,
            "agentDir": workspace.agent_dir.display().to_string(),
            "modelsDir": workspace.models_dir.display().to_string(),
            "platform": platform,
        });
        return output::success(format, "version", &workspace_display, data);
    }

    if args.verbose {
        println!("docfoo {version}");
        println!("sidecar: not built (Stage 1.2)");
        println!("workspace: {workspace_display}");
        println!("agent dir: {}", workspace.agent_dir.display());
        println!("models dir: {}", workspace.models_dir.display());
        println!("platform: {platform}");
    } else {
        println!("docfoo {version}");
    }
    Ok(())
}
