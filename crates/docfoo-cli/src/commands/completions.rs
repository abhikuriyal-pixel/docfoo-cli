//! `docfoo completions <shell>` — print a shell completion script.

use std::io;

use clap::CommandFactory;
use clap_complete::{generate, Shell};

use crate::error::Result;

pub fn run(shell: Shell) -> Result<()> {
    let mut command = crate::cli::Cli::command();
    generate(shell, &mut command, "docfoo", &mut io::stdout());
    Ok(())
}
