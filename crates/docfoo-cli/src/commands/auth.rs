//! `docfoo auth` — provider credentials.
//!
//! API keys are written to Pi's `auth.json` by the sidecar's `ModelRuntime`.
//! Status output only ever reports `configured`/`source`/`label`, never values.

use serde_json::json;

use crate::cli::AuthArgs;
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::sidecar::Sidecar;
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &AuthArgs) -> Result<()> {
    let workspace_display = workspace.root.display().to_string();
    let mut sidecar = Sidecar::locate(workspace)?;

    if args.status {
        let providers = sidecar.client()?.auth_status(args.provider.as_deref())?;
        if format.is_json() {
            return output::success(
                format,
                "auth",
                &workspace_display,
                json!({ "providers": providers }),
            );
        }
        if providers.is_empty() {
            println!("no providers matched");
            return Ok(());
        }
        for provider in providers {
            let status = match (&provider.configured, &provider.source) {
                (true, Some(source)) => format!("configured ({source})"),
                (true, None) => "configured".to_string(),
                (false, _) => "not configured".to_string(),
            };
            println!("{} ({}) — {status}", provider.id, provider.name);
        }
        return Ok(());
    }

    if let Some(provider) = &args.set {
        let key = match &args.key {
            Some(key) => key.clone(),
            None => prompt_secret(provider)?,
        };
        sidecar.client()?.auth_set(provider, &key)?;
        if format.is_json() {
            return output::success(
                format,
                "auth",
                &workspace_display,
                json!({ "provider": provider, "saved": true }),
            );
        }
        println!("stored key for {provider}");
        return Ok(());
    }

    if let Some(provider) = &args.logout {
        sidecar.client()?.auth_logout(provider)?;
        if format.is_json() {
            return output::success(
                format,
                "auth",
                &workspace_display,
                json!({ "provider": provider, "removed": true }),
            );
        }
        println!("removed credential for {provider}");
        return Ok(());
    }

    Err(CliError::Usage(
        "auth needs --status, --set or --logout".to_string(),
    ))
}

fn prompt_secret(provider: &str) -> Result<String> {
    let prompt = format!("API key for {provider}: ");
    let key = rpassword::prompt_password(prompt)
        .map_err(|error| CliError::Message(format!("could not read the API key: {error}")))?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(CliError::Usage("no API key entered".to_string()));
    }
    Ok(key)
}
