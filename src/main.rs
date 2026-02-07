mod cli;
mod config;
mod package;
mod platform;
mod storage;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, TimeZone};
use clap::Parser;
use cli::{Cli, Commands};
use console::style;
use serde::Serialize;
use std::process::Command;

use package::scan_all_binaries;
use platform::{Daemon, DaemonManager, Monitor, ProcessMonitor};
use storage::Database;

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Start => cmd_start(),
        Commands::Stop => cmd_stop(),
        Commands::Status { json } => cmd_status(json),
        Commands::Stats { json } => cmd_stats(json),
        Commands::Report {
            dust,
            low,
            stale,
            source,
            limit,
            all,
            json,
            export,
        } => cmd_report(dust, low, stale, source, limit, all, json, export),
        Commands::Clean { dry_run } => cmd_clean(dry_run),
        Commands::Config { edit } => cmd_config(edit),
        Commands::Daemon => cmd_daemon(),
    };

    if let Err(e) = result {
        eprintln!("{} {}", style("error:").red().bold(), e);
        std::process::exit(1);
    }
}

/// Start the daemon (returns true if started, false if already running)
fn start_daemon(silent: bool) -> Result<bool> {
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

    if !silent {
        println!("{} Starting dustbin daemon...", style("●").green());
        println!();
        println!(
            "  {} {}",
            style("Note:").cyan().bold(),
            Daemon::setup_instructions().lines().next().unwrap_or("")
        );
        for line in Daemon::setup_instructions().lines().skip(1) {
            println!("  {}", line);
        }
        println!();
    }

    let db = Database::open()?;
    if db.get_tracking_since()?.is_none() {
        let now = chrono::Utc::now().timestamp();
        db.set_tracking_since(now)?;
    }

    let exe_path = std::env::current_exe()?;
    let exe_str = exe_path.to_string_lossy();

    Daemon::start_daemon(&exe_str)?;

    if !silent {
        println!("{} Daemon started successfully", style("✓").green().bold());
        println!("  Run {} to check status", style("dustbin status").cyan());
    }
    Ok(true)
}

fn cmd_start() -> Result<()> {
    start_daemon(false)?;
    Ok(())
}

fn cmd_stop() -> Result<()> {
    if !Daemon::is_daemon_running() {
        println!("{} Daemon is not running", style("●").yellow());
        return Ok(());
    }

    println!("{} Stopping dustbin daemon...", style("●").yellow());

    Daemon::stop_daemon()?;
    println!("{} Daemon stopped", style("✓").green().bold());

    Ok(())
}

/// Sync binaries from PATH to database (runs silently)
fn sync_binaries(db: &Database) -> Result<()> {
    let binaries = scan_all_binaries()?;

    // Set tracking start if not already set
    if db.get_tracking_since()?.is_none() {
        let now = chrono::Utc::now().timestamp();
        db.set_tracking_since(now)?;
    }

    for (bin_path, pkg_name, source) in &binaries {
        db.register_binary(bin_path, pkg_name, source)?;
    }

    Ok(())
}

#[derive(Serialize)]
struct StatusJson {
    daemon_running: bool,
    tracking_since: Option<String>,
    tracking_days: i64,
    binaries_tracked: i64,
    dusty_count: i64,
}

fn cmd_status(json: bool) -> Result<()> {
    let db = Database::open()?;

    // Auto-sync binaries
    sync_binaries(&db)?;

    // Auto-start daemon if not running
    let just_started = start_daemon(true)?;
    let running = Daemon::is_daemon_running();
    let _binaries = db.get_all_binaries()?;
    let dusty_count = db.get_dusty_count()?;
    let binary_count = db.get_binary_count()?;

    let (tracking_since, days) = if let Some(since) = db.get_tracking_since()? {
        let dt: DateTime<Local> = Local.timestamp_opt(since, 0).unwrap();
        let now = Local::now();
        let duration = now.signed_duration_since(dt);
        (Some(dt.format("%Y-%m-%d").to_string()), duration.num_days())
    } else {
        (None, 0)
    };

    if json {
        let status = StatusJson {
            daemon_running: running,
            tracking_since,
            tracking_days: days,
            binaries_tracked: binary_count,
            dusty_count,
        };
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    // Pretty output
    println!();

    if just_started {
        println!("  {} Daemon started automatically", style("●").green());
    } else if running {
        println!("  {} Daemon is running", style("●").green());
    } else {
        println!("  {} Daemon is not running", style("●").red());
    }

    if let Some(ref since) = tracking_since {
        println!(
            "  {} Tracking since {} ({} days)",
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

    println!();
    Ok(())
}

#[derive(Serialize)]
struct BinaryJson {
    path: String,
    short_path: String,
    count: i64,
    last_used: Option<String>,
    status: String, // "active", "low", "dusty"
    source: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn cmd_report(
    dust: bool,
    low: Option<u32>,
    stale: Option<u32>,
    source: Option<String>,
    limit: usize,
    all: bool,
    json: bool,
    export: bool,
) -> Result<()> {
    let db = Database::open()?;
    let config = crate::config::Config::load()?;

    // Auto-sync binaries
    sync_binaries(&db)?;

    // Auto-start daemon if not running
    start_daemon(true)?;

    let binaries = db.get_all_binaries()?;

    if binaries.is_empty() {
        if json {
            println!("[]");
        } else {
            println!();
            println!("  {} No binaries found in PATH.", style("●").yellow());
            println!();
        }
        return Ok(());
    }

    let now = chrono::Utc::now().timestamp();

    // Apply filters
    let filtered: Vec<_> = binaries
        .into_iter()
        .filter(|b| {
            // Check ignore list
            let binary_name = std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if config.should_ignore_binary(binary_name) {
                return false;
            }

            // Filter by usage
            let usage_match = if dust {
                b.count == 0
            } else if let Some(threshold) = low {
                b.count < threshold as i64
            } else {
                true
            };

            // Filter by staleness (not used in N days)
            let stale_match = match stale {
                Some(days) => {
                    let threshold = now - (days as i64 * 24 * 60 * 60);
                    b.last_seen.map(|ts| ts < threshold).unwrap_or(true)
                }
                None => true,
            };

            // Filter by source
            let source_match = match &source {
                Some(s) => b.source.as_ref().map(|bs| bs == s).unwrap_or(false),
                None => true,
            };

            usage_match && stale_match && source_match
        })
        .collect();

    if filtered.is_empty() {
        if json {
            println!("[]");
        } else {
            println!();
            if dust {
                println!("  {} No dusty binaries found!", style("✓").green().bold());
            } else {
                println!("  {} No matching binaries found.", style("●").yellow());
            }
            println!();
        }
        return Ok(());
    }

    // Calculate totals before limiting
    let total_count = filtered.len();
    let total_active = filtered.iter().filter(|b| b.count >= 5).count();
    let total_low = filtered
        .iter()
        .filter(|b| b.count > 0 && b.count < 5)
        .count();
    let total_dusty = filtered.iter().filter(|b| b.count == 0).count();

    // Apply limit (0 or --all means no limit)
    let effective_limit = if all { 0 } else { limit };
    let limited: Vec<_> = if effective_limit > 0 {
        filtered.into_iter().take(effective_limit).collect()
    } else {
        filtered
    };

    // Build output data
    let rows: Vec<BinaryJson> = limited
        .iter()
        .map(|b| {
            let last_used = b.last_seen.map(|ts| {
                let dt: DateTime<Local> = Local.timestamp_opt(ts, 0).unwrap();
                dt.format("%Y-%m-%d %H:%M").to_string()
            });

            let status = if b.count == 0 {
                "dusty"
            } else if b.count < 5 {
                "low"
            } else {
                "active"
            };

            BinaryJson {
                path: b.path.clone(),
                short_path: shorten_path(&b.path),
                count: b.count,
                last_used,
                status: status.to_string(),
                source: b.source.clone(),
            }
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }

    // Export mode: output uninstall commands
    if export {
        export_uninstall_commands(&rows);
        return Ok(());
    }

    // Pretty table output with colors
    println!();

    // Header
    println!(
        "  {:<40} {:>10} {:>8} {:>16}",
        style("Binary").bold().underlined(),
        style("Source").bold().underlined(),
        style("Count").bold().underlined(),
        style("Last Used").bold().underlined()
    );
    println!();

    for row in &rows {
        let count_styled = match row.status.as_str() {
            "dusty" => style(format!("{:>8}", row.count)).red(),
            "low" => style(format!("{:>8}", row.count)).yellow(),
            _ => style(format!("{:>8}", row.count)).green(),
        };

        let path_styled = match row.status.as_str() {
            "dusty" => style(format!("{:<40}", truncate_str(&row.short_path, 40))).red(),
            "low" => style(format!("{:<40}", truncate_str(&row.short_path, 40))).yellow(),
            _ => style(format!("{:<40}", truncate_str(&row.short_path, 40))).white(),
        };

        let source_str = row.source.as_deref().unwrap_or("-");
        let last_used = row.last_used.as_deref().unwrap_or("never");

        println!(
            "  {} {:>10} {} {:>16}",
            path_styled, source_str, count_styled, last_used
        );
    }

    println!();

    // Summary line
    print!("  ");
    if total_active > 0 {
        print!("{} active  ", style(format!("{}", total_active)).green());
    }
    if total_low > 0 {
        print!("{} low  ", style(format!("{}", total_low)).yellow());
    }
    if total_dusty > 0 {
        print!("{} dusty", style(format!("{}", total_dusty)).red());
    }
    println!();

    // Show if results were limited
    if effective_limit > 0 && rows.len() < total_count {
        println!(
            "  {} Showing {} of {} (use {} for all)",
            style("◦").dim(),
            rows.len(),
            total_count,
            style("--all").cyan()
        );
    }

    println!();

    Ok(())
}

/// Export uninstall commands for the given binaries
fn export_uninstall_commands(rows: &[BinaryJson]) {
    use std::collections::HashMap;

    // Group by source
    let mut by_source: HashMap<&str, Vec<&str>> = HashMap::new();
    for row in rows {
        if let Some(ref source) = row.source {
            let pkg_name = std::path::Path::new(&row.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&row.path);
            by_source.entry(source.as_str()).or_default().push(pkg_name);
        }
    }

    println!("# Uninstall commands for {} binaries", rows.len());
    println!();

    for (source, pkgs) in by_source {
        let cmd = match source {
            "homebrew" => Some(format!("brew uninstall {}", pkgs.join(" "))),
            "cargo" => Some(format!("cargo uninstall {}", pkgs.join(" "))),
            "npm" => Some(format!("npm uninstall -g {}", pkgs.join(" "))),
            "pip" => Some(format!("pip uninstall {}", pkgs.join(" "))),
            "go" => Some(
                pkgs.iter()
                    .map(|p| format!("rm ~/go/bin/{}", p))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            "apt" => Some(format!("sudo apt remove {}", pkgs.join(" "))),
            "dnf" => Some(format!("sudo dnf remove {}", pkgs.join(" "))),
            "pacman" => Some(format!("sudo pacman -R {}", pkgs.join(" "))),
            "snap" => Some(format!("sudo snap remove {}", pkgs.join(" "))),
            _ => None,
        };

        if let Some(cmd) = cmd {
            println!("# {} ({} packages)", source, pkgs.len());
            println!("{}", cmd);
            println!();
        }
    }
}

#[derive(Serialize)]
struct StatsJson {
    tracking_days: i64,
    total_binaries: i64,
    active: i64,
    low: i64,
    dusty: i64,
    by_source: std::collections::HashMap<String, i64>,
}

fn cmd_stats(json: bool) -> Result<()> {
    let db = Database::open()?;

    // Auto-sync binaries
    sync_binaries(&db)?;

    let binaries = db.get_all_binaries()?;
    let tracking_since = db.get_tracking_since()?;

    let days = if let Some(since) = tracking_since {
        let now = chrono::Utc::now().timestamp();
        (now - since) / (24 * 60 * 60)
    } else {
        0
    };

    let total = binaries.len() as i64;
    let active = binaries.iter().filter(|b| b.count >= 5).count() as i64;
    let low = binaries
        .iter()
        .filter(|b| b.count > 0 && b.count < 5)
        .count() as i64;
    let dusty = binaries.iter().filter(|b| b.count == 0).count() as i64;

    // Count by source
    let mut by_source: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for b in &binaries {
        let source = b.source.clone().unwrap_or_else(|| "other".to_string());
        *by_source.entry(source).or_insert(0) += 1;
    }

    if json {
        let stats = StatsJson {
            tracking_days: days,
            total_binaries: total,
            active,
            low,
            dusty,
            by_source,
        };
        println!("{}", serde_json::to_string_pretty(&stats)?);
        return Ok(());
    }

    // Pretty output
    println!();
    println!("  {}  dustbin", style("📦").dim());
    println!("  {}", style("─".repeat(40)).dim());
    println!();

    // Tracking info
    if days > 0 {
        println!(
            "  Tracking for {} days",
            style(format!("{}", days)).cyan().bold()
        );
    } else {
        println!("  Tracking started {}", style("today").cyan().bold());
    }
    println!();

    // Usage bar
    let bar_width: usize = 30;
    let active_width = if total > 0 {
        active as usize * bar_width / total as usize
    } else {
        0
    };
    let low_width = if total > 0 {
        low as usize * bar_width / total as usize
    } else {
        0
    };
    let dusty_width = bar_width
        .saturating_sub(active_width)
        .saturating_sub(low_width);

    let bar = format!(
        "{}{}{}",
        style("█".repeat(active_width)).green(),
        style("█".repeat(low_width)).yellow(),
        style("█".repeat(dusty_width)).red()
    );

    println!("  {} binaries", style(format!("{}", total)).bold());
    println!("  {}", bar);
    println!();

    // Legend with counts
    println!("  {} {:>5}  active (5+ uses)", style("■").green(), active);
    println!("  {} {:>5}  low (1-4 uses)", style("■").yellow(), low);
    println!("  {} {:>5}  dusty (never used)", style("■").red(), dusty);
    println!();

    // Sort sources by count
    let mut sources: Vec<_> = by_source.iter().collect();
    sources.sort_by(|a, b| b.1.cmp(a.1));

    println!("  {}", style("By source").dim());
    println!("  {}", style("─".repeat(25)).dim());

    let max_count = sources.first().map(|(_, c)| **c).unwrap_or(1);
    let source_bar_width = 15;

    for (source, count) in sources.iter().take(8) {
        let bar_len = (**count * source_bar_width / max_count) as usize;
        let bar_len = bar_len.max(1); // At least 1 char
        println!(
            "  {:>10}  {} {}",
            source,
            style("▪".repeat(bar_len)).cyan(),
            style(count).dim()
        );
    }

    if sources.len() > 8 {
        let others: i64 = sources.iter().skip(8).map(|(_, c)| **c).sum();
        println!(
            "  {:>10}  {} {}",
            "others",
            style("▪").dim(),
            style(others).dim()
        );
    }

    println!();

    Ok(())
}

fn cmd_clean(dry_run: bool) -> Result<()> {
    use dialoguer::{MultiSelect, theme::ColorfulTheme};
    use std::collections::HashMap;

    let db = Database::open()?;

    // Auto-sync first
    sync_binaries(&db)?;

    let binaries = db.get_all_binaries()?;

    // Group dusty binaries by source
    let mut by_source: HashMap<String, Vec<_>> = HashMap::new();
    for b in binaries.into_iter().filter(|b| b.count == 0) {
        let source = b.source.clone().unwrap_or_else(|| "other".to_string());
        by_source.entry(source).or_default().push(b);
    }

    if by_source.is_empty() {
        println!();
        println!(
            "  {} No dusty binaries to clean!",
            style("✓").green().bold()
        );
        println!();
        return Ok(());
    }

    let total_dusty: usize = by_source.values().map(|v| v.len()).sum();
    println!();
    println!("  Found {} dusty binaries:", style(total_dusty).yellow());
    println!();

    // Process each source that we can uninstall
    for (source, bins) in &by_source {
        let uninstall_cmd = match source.as_str() {
            "homebrew" => Some("brew uninstall"),
            "cargo" => Some("cargo uninstall"),
            "npm" => Some("npm uninstall -g"),
            "pip" => Some("pip uninstall"),
            "apt" => Some("sudo apt remove"),
            "dnf" => Some("sudo dnf remove"),
            "pacman" => Some("sudo pacman -R"),
            "snap" => Some("sudo snap remove"),
            _ => None,
        };

        if uninstall_cmd.is_none() {
            continue;
        }

        println!(
            "  {} {} ({} packages):",
            style("●").cyan(),
            source,
            bins.len()
        );

        // Get package names
        let pkg_names: Vec<String> = bins
            .iter()
            .map(|b| {
                std::path::Path::new(&b.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            })
            .collect();

        if dry_run {
            for name in &pkg_names {
                println!("    {} {}", style("•").red(), name);
            }
            continue;
        }

        // Interactive selection
        let selections = MultiSelect::with_theme(&ColorfulTheme::default())
            .with_prompt("Select packages to remove (space to toggle, enter to confirm)")
            .items(&pkg_names)
            .interact_opt()?;

        if let Some(indices) = selections {
            if indices.is_empty() {
                println!("    {} Skipped", style("◦").dim());
                continue;
            }

            let selected: Vec<&str> = indices.iter().map(|&i| pkg_names[i].as_str()).collect();
            let cmd = format!("{} {}", uninstall_cmd.unwrap(), selected.join(" "));

            println!("    Running: {}", style(&cmd).cyan());

            let status = Command::new("sh")
                .args(["-c", &cmd])
                .status()
                .context("Failed to run uninstall command")?;

            if status.success() {
                println!(
                    "    {} Removed {} packages",
                    style("✓").green(),
                    selected.len()
                );
            } else {
                println!("    {} Some packages failed to remove", style("✗").red());
            }
        }
    }

    // Show sources we can't auto-uninstall
    let manual_sources: Vec<_> = by_source
        .iter()
        .filter(|(s, _)| {
            !matches!(
                s.as_str(),
                "homebrew" | "cargo" | "npm" | "pip" | "apt" | "dnf" | "pacman" | "snap"
            )
        })
        .collect();

    if !manual_sources.is_empty() {
        println!();
        println!("  {} Manual cleanup needed:", style("●").yellow());
        for (source, bins) in manual_sources {
            println!("    {} ({} binaries) - remove manually", source, bins.len());
        }
    }

    println!();

    if dry_run {
        println!(
            "  {} Dry run - no packages were removed",
            style("●").yellow()
        );
        println!(
            "  Run {} to interactively remove them",
            style("dustbin clean").cyan()
        );
        println!();
    }

    Ok(())
}

fn cmd_config(edit: bool) -> Result<()> {
    use crate::config::Config;

    // Load config (auto-creates if not exists)
    let _config = Config::load()?;
    let path = Config::config_path()?;

    if edit {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        Command::new(&editor)
            .arg(&path)
            .status()
            .context(format!("Failed to open editor: {}", editor))?;
        return Ok(());
    }

    // Default: show config
    println!();
    println!("  {} {}", style("Config:").bold(), path.display());
    println!();

    let content = std::fs::read_to_string(&path)?;
    for line in content.lines() {
        print!("    ");
        print_toml_line(line);
        println!();
    }
    println!();

    Ok(())
}

fn print_toml_line(line: &str) {
    let trimmed = line.trim();

    // Comment
    if trimmed.starts_with('#') {
        print!("{}", style(line).dim());
        return;
    }

    // Section header [foo]
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        print!("{}", style(line).cyan().bold());
        return;
    }

    // Key = value
    if let Some(eq_pos) = line.find('=') {
        let (key, rest) = line.split_at(eq_pos);
        let value = &rest[1..]; // skip the '='
        print!("{}", style(key).green());
        print!("{}", style("=").dim());
        print_toml_value(value);
        return;
    }

    // Fallback
    print!("{}", line);
}

fn print_toml_value(value: &str) {
    let trimmed = value.trim();

    // String value
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        print!("{}", style(value).yellow());
    }
    // Boolean or number
    else if trimmed == "true" || trimmed == "false" || trimmed.parse::<f64>().is_ok() {
        print!("{}", style(value).magenta());
    }
    // Array start
    else if trimmed.starts_with('[') {
        print!("{}", style(value).yellow());
    }
    // Other
    else {
        print!("{}", style(value).yellow());
    }
}

fn cmd_daemon() -> Result<()> {
    let db = Database::open()?;
    let mut monitor = Monitor::new();

    eprintln!("dustbin daemon starting...");

    let rx = monitor.start()?;

    eprintln!("dustbin daemon running, listening for exec events...");

    for path in rx {
        if should_skip_path(&path) {
            continue;
        }

        if let Err(e) = db.record_exec(&path) {
            eprintln!("Failed to record exec for {}: {}", path, e);
        }
    }

    Ok(())
}

fn shorten_path(path: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.display().to_string())
        .unwrap_or_default();

    path.replace("/opt/homebrew/bin/", "brew:")
        .replace("/opt/homebrew/Cellar/", "brew:")
        .replace("/usr/local/bin/", "/usr/local/")
        .replace("/usr/bin/", "/usr/")
        .replace(&format!("{}/.cargo/bin/", home), "cargo:")
        .replace(&format!("{}/", home), "~/")
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("...{}", &s[s.len() - (max_len - 3)..])
    }
}

fn should_skip_path(path: &str) -> bool {
    let skip_prefixes = ["/usr/libexec/", "/System/", "/Library/Apple/", "/usr/sbin/"];

    let skip_exact = ["/bin/sh", "/bin/bash", "/bin/zsh", "/usr/bin/env"];

    skip_prefixes.iter().any(|p| path.starts_with(p)) || skip_exact.contains(&path)
}
