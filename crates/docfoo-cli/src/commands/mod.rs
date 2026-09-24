//! Command dispatch. Stages land one command at a time; everything not wired
//! yet returns a clear "not implemented" error naming its stage.

use crate::cli::{Cli, Command};
use crate::error::Result;
use crate::output::OutputFormat;
use crate::workspace::Workspace;

mod auth;
mod backup;
mod collections;
mod completions;
mod kg;
mod model;
mod notes;
mod resources;
mod restore;
mod scan;
mod setup;
mod update;
mod version;

pub fn dispatch(cli: &Cli, format: OutputFormat, workspace: &Workspace) -> Result<()> {
    match &cli.command {
        Command::Version(args) => version::run(format, workspace, args),
        Command::Model(args) => model::run(format, workspace, args),
        Command::Auth(args) => auth::run(format, workspace, args),
        Command::Kg(args) => kg::run(cli, format, workspace, args),
        Command::Resources(args) => resources::run(format, workspace, args),
        Command::Notes(args) => notes::run(format, workspace, args),
        Command::Backup(args) => backup::run(format, workspace, args),
        Command::Restore(args) => restore::run(format, workspace, args),
        Command::Collections(args) => collections::run(format, workspace, args),
        Command::Scan(args) => scan::run(cli, format, workspace, args),
        Command::Setup(args) => setup::run(format, workspace, args),
        Command::Update(args) => update::run(format, workspace, args),
        Command::Completions(args) => completions::run(args.shell),
    }
}
