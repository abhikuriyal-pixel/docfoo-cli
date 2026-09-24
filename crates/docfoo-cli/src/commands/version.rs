use serde_json::json;

use crate::cli::VersionArgs;
use crate::error::Result;
use crate::output::{self, OutputFormat};
use crate::sidecar::SidecarLaunch;
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &VersionArgs) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let workspace_display = workspace.root.display().to_string();
    let sidecar_path = SidecarLaunch::discover().ok().map(|launch| match launch {
        SidecarLaunch::Binary(path) => path.display().to_string(),
        SidecarLaunch::Ts { script, .. } => format!("{} (bun)", script.display()),
    });

    if format.is_json() {
        let data = json!({
            "cliVersion": version,
            "sidecarVersion": null,
            "sidecarPath": sidecar_path,
            "workspace": workspace_display,
            "agentDir": workspace.agent_dir.display().to_string(),
            "modelsDir": workspace.models_dir.display().to_string(),
            "platform": platform,
        });
        return output::success(format, "version", &workspace_display, data);
    }

    if args.verbose {
        println!("docfoo {version}");
        match &sidecar_path {
            Some(path) => println!("sidecar: {path}"),
            None => println!("sidecar: not found (run `docfoo setup` or build sidecar/)"),
        }
        println!("workspace: {workspace_display}");
        println!("agent dir: {}", workspace.agent_dir.display());
        println!("models dir: {}", workspace.models_dir.display());
        println!("platform: {platform}");
    } else {
        println!("docfoo {version}");
    }
    Ok(())
}
