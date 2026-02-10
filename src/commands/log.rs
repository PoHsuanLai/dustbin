use anyhow::Result;

use crate::platform::{Daemon, DaemonManager};

pub fn cmd_log(lines: usize, follow: bool) -> Result<()> {
    Daemon::view_logs(lines, follow)
}
