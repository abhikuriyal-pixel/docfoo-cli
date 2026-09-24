//! `docfoo model` — get, set and list model slots.
//!
//! The slots and `model-selection.json` are shared with the desktop app; `set`
//! validates the key against the sidecar's live provider catalog before writing.

use serde_json::json;

use crate::cli::ModelArgs;
use crate::config::{slot_key, ModelPreferences};
use crate::error::{CliError, Result};
use crate::output::{self, OutputFormat};
use crate::sidecar::Sidecar;
use crate::workspace::Workspace;

pub fn run(format: OutputFormat, workspace: &Workspace, args: &ModelArgs) -> Result<()> {
    let workspace_display = workspace.root.display().to_string();

    if let Some(slot) = &args.get {
        let prefs = ModelPreferences::load(workspace);
        let key = prefs.get(slot)?;
        let data = json!({ "slot": slot, "key": key, "configured": key.is_some() });
        if format.is_json() {
            return output::success(format, "model", &workspace_display, data);
        }
        match key {
            Some(key) => println!("{slot} = {key}"),
            None => println!("{slot} = (not set)"),
        }
        return Ok(());
    }

    if let Some(set) = &args.set {
        let slot = set
            .first()
            .ok_or_else(|| CliError::Usage("model --set needs SLOT and KEY".to_string()))?;
        let key = set
            .get(1)
            .ok_or_else(|| CliError::Usage("model --set needs SLOT and KEY".to_string()))?;
        slot_key(slot)?;
        validate_model(workspace, key)?;
        workspace.ensure_scaffold()?;
        let mut prefs = ModelPreferences::load(workspace);
        prefs.set(slot, key)?;
        prefs.save(workspace)?;
        let data = json!({ "slot": slot, "key": key, "saved": true });
        if format.is_json() {
            return output::success(format, "model", &workspace_display, data);
        }
        println!("{slot} = {key}");
        return Ok(());
    }

    let mut sidecar = Sidecar::locate(workspace)?;
    let providers = sidecar.client()?.models()?;
    let filtered: Vec<_> = providers
        .iter()
        .filter(|provider| {
            args.provider
                .as_deref()
                .map_or(true, |wanted| provider.id == wanted)
        })
        .collect();

    if format.is_json() {
        return output::success(
            format,
            "model",
            &workspace_display,
            json!({ "providers": filtered }),
        );
    }

    if filtered.is_empty() {
        println!("no providers matched");
        return Ok(());
    }
    for provider in filtered {
        let auth = match (&provider.configured, &provider.source) {
            (true, Some(source)) => format!("configured ({source})"),
            (true, None) => "configured".to_string(),
            (false, _) => "not configured".to_string(),
        };
        println!(
            "{} ({}) — {auth}, {} models",
            provider.id, provider.name, provider.model_count
        );
        for model in &provider.models {
            let name = model.name.as_deref().unwrap_or(&model.id);
            println!("  {}/{} — {name}", provider.id, model.id);
        }
    }
    Ok(())
}

fn validate_model(workspace: &Workspace, key: &str) -> Result<()> {
    let (provider_id, model_id) = key.split_once('/').ok_or_else(|| {
        CliError::Usage(format!("model key \"{key}\" must look like provider/model"))
    })?;
    if provider_id.is_empty() || model_id.is_empty() {
        return Err(CliError::Usage(format!(
            "model key \"{key}\" must look like provider/model"
        )));
    }
    let mut sidecar = Sidecar::locate(workspace)?;
    let providers = sidecar.client()?.models()?;
    let provider = providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| CliError::Message(format!("unknown provider \"{provider_id}\"")))?;
    if !provider.models.iter().any(|model| model.id == model_id) {
        return Err(CliError::Message(format!(
            "unknown model \"{key}\" in provider \"{provider_id}\""
        )));
    }
    Ok(())
}
