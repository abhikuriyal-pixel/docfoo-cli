//! docfoo — barebones DocFoo CLI.

// Stage 1.1 ships the full command surface and the config/workspace plumbing
// ahead of their consumers. Remove this allow once Stages 1.2–2 land.
#![allow(dead_code)]

mod cli;
mod commands;
mod config;
mod error;
mod output;
mod util;
mod workspace;

use clap::Parser;

use cli::Cli;

fn main() {
    let cli = Cli::parse();
    let format = cli.output_format();
    let command = cli.command.name();

    let workspace = match workspace::resolve_from_env(cli.workspace.as_deref()) {
        Ok(workspace) => workspace,
        Err(error) => {
            output::error(format, command, "<unresolved>", &error);
            std::process::exit(error.exit_code());
        }
    };

    if let Err(error) = commands::dispatch(&cli, format, &workspace) {
        output::error(format, command, &workspace.root.display().to_string(), &error);
        std::process::exit(error.exit_code());
    }
}
