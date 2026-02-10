use anyhow::Result;
use clap::CommandFactory;

use crate::cli::Cli;

pub fn cmd_completions(shell: clap_complete::Shell) -> Result<()> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "dusty", &mut std::io::stdout());
    Ok(())
}
