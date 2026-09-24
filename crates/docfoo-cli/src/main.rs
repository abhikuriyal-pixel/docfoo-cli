//! docfoo — barebones DocFoo CLI.

use clap::Parser;

use docfoo_cli::cli::Cli;
use docfoo_cli::{commands, output, workspace};

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
