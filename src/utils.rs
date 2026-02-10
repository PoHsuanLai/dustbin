use anyhow::Result;
use chrono::{DateTime, Local, TimeZone};
use console::style;
use std::collections::BTreeSet;

use crate::config;
use crate::defaults;
use crate::package::scan_all_binaries;
use crate::platform::{Daemon, DaemonManager};
use crate::storage::Database;

/// Convert a Unix timestamp to a local DateTime, handling invalid values gracefully.
pub fn local_datetime(ts: i64) -> DateTime<Local> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).single().unwrap())
}

/// Start the daemon (returns true if started, false if already running).
/// When `silent` is true, skip starting (it requires sudo and a tty).
pub fn start_daemon(silent: bool) -> Result<bool> {
    if !Daemon::check_available() {
        if !silent {
            anyhow::bail!(
                "Required monitoring tool not found.\n{}",
                Daemon::setup_instructions()
            );
        }
        return Ok(false);
    }

    if Daemon::is_daemon_running() {
        if !silent {
            println!("{} Daemon is already running", style("●").yellow());
        }
        return Ok(false);
    }

    // Silent mode: don't try to start (needs sudo with tty)
    if silent {
        return Ok(false);
    }

    println!("{} Starting dusty daemon...", style("●").green());
    println!();
    println!(
        "  {} {}",
        style("Note:").yellow().bold(),
        Daemon::setup_instructions().lines().next().unwrap_or("")
    );
    for line in Daemon::setup_instructions().lines().skip(1) {
        println!("  {}", line);
    }
    println!();

    let db = Database::open()?;
    if db.get_tracking_since()?.is_none() {
        let now = chrono::Utc::now().timestamp();
        db.set_tracking_since(now)?;
    }

    let exe_path = std::env::current_exe()?;
    let exe_str = exe_path.to_string_lossy();

    Daemon::start_daemon(&exe_str)?;

    println!("{} Daemon started successfully", style("●").green().bold());
    println!("  Run {} to check status", style("dusty status").cyan());
    Ok(true)
}

/// Sync binaries from PATH to database (runs silently)
pub fn sync_binaries(db: &Database) -> Result<()> {
    let config = config::Config::load()?;
    let binaries = scan_all_binaries()?;

    // Set tracking start if not already set
    if db.get_tracking_since()?.is_none() {
        let now = chrono::Utc::now().timestamp();
        db.set_tracking_since(now)?;
    }

    for (bin_path, pkg_name, source, resolved) in &binaries {
        db.register_binary(bin_path, pkg_name, source)?;

        // If the binary is a symlink, register the resolved path as an alias
        // so that exec events from eslogger (which reports resolved paths)
        // get credited to the canonical symlink path
        if let Some(resolved_path) = resolved {
            db.register_alias(resolved_path, bin_path)?;
        }
    }

    // Remove binaries that no longer exist on disk
    db.prune_missing()?;

    // Backfill source + package_name for binaries discovered by the daemon
    db.backfill_uncategorized(|path| {
        let source = config.categorize_path(path);
        let bin_path = std::path::Path::new(path);
        let default_name = bin_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let pkg_name = crate::package::get_package_name(bin_path, default_name);
        (source, pkg_name)
    })?;

    Ok(())
}

/// Detect install root directories from a set of binary paths.
/// e.g. ["/opt/anaconda3/bin/python", "/opt/anaconda3/bin/conda"] -> ["/opt/anaconda3"]
/// Walks up from each binary path to find a reasonable root (one level below
/// a well-known parent like /opt, /usr/local, or $HOME).
pub fn detect_install_roots(paths: &[&str]) -> Vec<String> {
    let home = dirs::home_dir()
        .map(|h| h.display().to_string())
        .unwrap_or_default();

    // Well-known parent dirs -- one level below these is the install root
    let anchors: Vec<String> = defaults::INSTALL_ROOT_ANCHORS
        .iter()
        .map(|a| a.replace('~', &home))
        .collect();

    let mut roots = BTreeSet::new();
    for path in paths {
        // Try to match an anchor
        for anchor in &anchors {
            if path.starts_with(anchor.as_str()) {
                // Take the first component after the anchor
                let rest = &path[anchor.len()..];
                if let Some(first_component) = rest.split('/').next()
                    && !first_component.is_empty()
                {
                    roots.insert(format!("{}{}", anchor, first_component));
                    break;
                }
            }
        }
    }

    roots.into_iter().collect()
}
