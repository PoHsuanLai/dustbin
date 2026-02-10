use anyhow::Result;
use console::style;

use crate::platform::{Daemon, DaemonManager};
use crate::utils;

pub fn cmd_start() -> Result<()> {
    utils::start_daemon(false)?;
    Ok(())
}

pub fn cmd_stop() -> Result<()> {
    if !Daemon::is_daemon_running() {
        println!("{} Daemon is not running", style("●").red());
        return Ok(());
    }

    println!("{} Stopping dusty daemon...", style("●").red());

    Daemon::stop_daemon()?;
    println!("{} Daemon stopped", style("●").green().bold());

    Ok(())
}
