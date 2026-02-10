use anyhow::Result;
use chrono::Local;

use crate::config;
use crate::platform::{Monitor, ProcessMonitor};
use crate::storage::Database;

pub fn cmd_daemon() -> Result<()> {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};

    let db = Database::open()?;
    let config = config::Config::load()?;
    let mut monitor = Monitor::new();

    let source_names: Vec<&str> = config.sources.iter().map(|s| s.name.as_str()).collect();
    println!(
        "[{}] dusty daemon starting (db: {}, sources: {})",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        Database::db_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into()),
        source_names.join(", "),
    );

    let rx = monitor.start()?;

    println!(
        "[{}] listening for exec events",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
    );

    let heartbeat = Duration::from_secs(3600);
    let mut last_heartbeat = Instant::now();
    let mut period_recorded: u64 = 0;
    let mut period_skipped: u64 = 0;
    let mut total_recorded: u64 = 0;

    loop {
        match rx.recv_timeout(heartbeat) {
            Ok(path) => {
                if should_skip_path(&path, &config) {
                    period_skipped += 1;
                    continue;
                }
                let source = config.categorize_path(&path);
                if let Err(e) = db.record_exec(&path, Some(&source)) {
                    eprintln!(
                        "[{}] error recording {}: {}",
                        Local::now().format("%H:%M:%S"),
                        path,
                        e
                    );
                }
                period_recorded += 1;
                total_recorded += 1;
            }
            Err(RecvTimeoutError::Disconnected) => {
                println!(
                    "[{}] monitor disconnected, shutting down (total recorded: {})",
                    Local::now().format("%Y-%m-%d %H:%M:%S"),
                    total_recorded,
                );
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        if last_heartbeat.elapsed() >= heartbeat {
            #[cfg(target_os = "macos")]
            let parse_errors = monitor.take_parse_errors();
            #[cfg(not(target_os = "macos"))]
            let parse_errors = 0u64;

            println!(
                "[{}] heartbeat: {} recorded, {} skipped, {} parse errors this hour (total: {})",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                period_recorded,
                period_skipped,
                parse_errors,
                total_recorded,
            );
            period_recorded = 0;
            period_skipped = 0;
            last_heartbeat = Instant::now();
        }
    }

    Ok(())
}

fn should_skip_path(path: &str, config: &config::Config) -> bool {
    let skip_exact = ["/bin/sh", "/bin/bash", "/bin/zsh", "/usr/bin/env"];

    if skip_exact.contains(&path) {
        return true;
    }

    config
        .scan
        .skip_prefixes
        .iter()
        .any(|p| path.starts_with(p.as_str()) || path.contains(p.as_str()))
}
