use anyhow::Result;
use chrono::{DateTime, Local};
use console::style;
use serde::Serialize;

use crate::config;
use crate::platform::{Daemon, DaemonManager};
use crate::storage::Database;
use crate::utils::{local_datetime, start_daemon, sync_binaries};

#[derive(Serialize)]
struct StatusJson {
    daemon_running: bool,
    daemon_healthy: bool,
    has_permissions: bool,
    first_scan: Option<String>,
    first_scan_days: i64,
    binaries_tracked: i64,
    dusty_count: i64,
    db_path: Option<String>,
    config_path: Option<String>,
    log_path: Option<String>,
}

pub fn cmd_status(json: bool) -> Result<()> {
    let db = Database::open()?;

    // Auto-sync binaries
    sync_binaries(&db)?;

    // Auto-start daemon if not running
    let just_started = start_daemon(true)?;
    let running = Daemon::is_daemon_running();
    let healthy = running && is_daemon_healthy();
    let dusty_count = db.get_dusty_count()?;
    let binary_count = db.get_binary_count()?;

    let (first_scan, days) = if let Some(since) = db.get_tracking_since()? {
        let dt: DateTime<Local> = local_datetime(since);
        let now = Local::now();
        let duration = now.signed_duration_since(dt);
        (Some(dt.format("%Y-%m-%d").to_string()), duration.num_days())
    } else {
        (None, 0)
    };

    if json {
        let status = StatusJson {
            daemon_running: running,
            daemon_healthy: healthy,
            has_permissions: Daemon::check_permissions(),
            first_scan,
            first_scan_days: days,
            binaries_tracked: binary_count,
            dusty_count,
            db_path: Database::db_path().ok().map(|p| p.display().to_string()),
            config_path: config::Config::config_path()
                .ok()
                .map(|p| p.display().to_string()),
            log_path: Some(Daemon::log_hint()),
        };
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    // Pretty output
    println!();

    if just_started {
        println!("  {} Daemon started automatically", style("●").green());
    } else if running {
        if healthy {
            println!("  {} Daemon is running", style("●").green());
        } else {
            println!(
                "  {} Daemon is running but may be crash-looping",
                style("●").yellow()
            );
            println!("    Check logs: {}", style("dusty log").cyan());
        }
    } else {
        println!("  {} Daemon is not running", style("●").red());
        println!("    Start with: {}", style("dusty start").cyan());
    }

    // Permissions check
    if running && !healthy && !Daemon::check_permissions() {
        println!();
        println!(
            "  {} Full Disk Access required for eslogger",
            style("!").red().bold()
        );
        println!("    System Settings > Privacy & Security > Full Disk Access");
        println!(
            "    Add {} (Cmd+Shift+G to type path)",
            style("/usr/bin/eslogger").cyan()
        );
        println!(
            "    Then restart: {}",
            style("sudo dusty stop && sudo dusty start").cyan()
        );
        // Open the FDA settings page
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .spawn();
        println!("    {}", style("Opening System Settings...").dim());
    }

    if let Some(ref since) = first_scan {
        println!(
            "  {} First scan {} ({} days ago)",
            style("◦").dim(),
            since,
            days
        );
    }

    println!("  {} {} binaries tracked", style("◦").dim(), binary_count);

    if dusty_count > 0 {
        println!(
            "  {} {} dusty binaries (never used)",
            style("◦").dim(),
            style(dusty_count).yellow()
        );
    }

    // Show paths for debugging
    println!();
    if let Ok(db_path) = Database::db_path() {
        println!(
            "  {} {}",
            style("Database:").dim(),
            style(db_path.display()).dim()
        );
    }
    if let Ok(config_path) = config::Config::config_path() {
        println!(
            "  {} {}",
            style("Config:").dim(),
            style(config_path.display()).dim()
        );
    }
    println!(
        "  {} {}",
        style("Logs:").dim(),
        style(Daemon::log_hint()).dim()
    );

    println!();
    Ok(())
}

/// Check if daemon is healthy by looking for "shutting down" in recent log lines.
/// If the last log line is a shutdown message, the daemon is crash-looping.
fn is_daemon_healthy() -> bool {
    let log_path = std::path::PathBuf::from(Daemon::log_hint()).join("dusty.log");
    let Ok(content) = std::fs::read_to_string(&log_path) else {
        return true; // No log file yet, assume OK
    };
    // Check the last non-empty line
    let last_line = content.lines().rev().find(|l| !l.is_empty());
    match last_line {
        Some(line) => !line.contains("shutting down"),
        None => true,
    }
}
