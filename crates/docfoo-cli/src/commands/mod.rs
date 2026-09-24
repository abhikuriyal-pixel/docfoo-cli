//! Command dispatch. Stage 1.1 wires `version` and leaves the rest as clear
//! "not implemented yet" errors so the CLI surface is stable from day one.

use crate::cli::{Cli, Command};
use crate::error::{CliError, Result};
use crate::output::OutputFormat;
use crate::workspace::Workspace;

mod version;

pub fn dispatch(cli: &Cli, format: OutputFormat, workspace: &Workspace) -> Result<()> {
    match &cli.command {
        Command::Version(args) => version::run(format, workspace, args),
        Command::Kg(_) => Err(CliError::NotImplemented(
            "kg is not implemented yet — the model sidecar is not built (Stages 1.2–1.3)"
                .to_string(),
        )),
        Command::Scan(_) => Err(CliError::NotImplemented(
            "scan is not implemented yet (Stage 4)".to_string(),
        )),
        Command::Resources(_) => Err(CliError::NotImplemented(
            "resources is not implemented yet (Stage 2)".to_string(),
        )),
        Command::Notes(_) => Err(CliError::NotImplemented(
            "notes is not implemented yet (Stage 2)".to_string(),
        )),
        Command::Backup(_) => Err(CliError::NotImplemented(
            "backup is not implemented yet (Stage 3)".to_string(),
        )),
        Command::Restore(_) => Err(CliError::NotImplemented(
            "restore is not implemented yet (Stage 3)".to_string(),
        )),
        Command::Collections(_) => Err(CliError::NotImplemented(
            "collections is not implemented yet (Stage 3)".to_string(),
        )),
        Command::Model(_) => Err(CliError::NotImplemented(
            "model is not implemented yet (Stage 1.2)".to_string(),
        )),
        Command::Auth(_) => Err(CliError::NotImplemented(
            "auth is not implemented yet (Stage 1.2)".to_string(),
        )),
        Command::Setup(_) => Err(CliError::NotImplemented(
            "setup is not implemented yet (Stage 4)".to_string(),
        )),
        Command::Update(_) => Err(CliError::NotImplemented(
            "update is not implemented yet (Stage 5)".to_string(),
        )),
    }
}
