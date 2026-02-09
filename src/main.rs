mod cli;
mod config;
mod defaults;
mod deps;
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
            all,
            json,
            export,
        } => cmd_report(dust, low, stale, source, all, json, export),
        Commands::Clean {
            dry_run,
            stale,
            source,
        } => cmd_clean(dry_run, stale, source),
        Commands::Config { edit } => cmd_config(edit),
        Commands::Dupes { name, expand, json } => cmd_dupes(name, expand, json),
        Commands::Deps {
            orphans,
            binary,
            refresh,
            json,
        } => cmd_deps(orphans, binary, refresh, json),
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
        println!("{} Starting dusty daemon...", style("●").green());
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
        println!("{} Daemon started successfully", style("●").green().bold());
        println!("  Run {} to check status", style("dusty status").cyan());
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

    println!("{} Stopping dusty daemon...", style("●").yellow());

    Daemon::stop_daemon()?;
    println!("{} Daemon stopped", style("●").green().bold());

    Ok(())
}

/// Sync binaries from PATH to database (runs silently)
fn sync_binaries(db: &Database) -> Result<()> {
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
        let pkg_name = package::get_package_name(bin_path, default_name);
        (source, pkg_name)
    })?;

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
    package_name: Option<String>,
}

fn cmd_report(
    dust: bool,
    low: Option<u32>,
    stale: Option<u32>,
    source: Option<String>,
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

    // Apply explicit filters (source, stale, ignore list)
    let filtered: Vec<_> = binaries
        .into_iter()
        .filter(|b| {
            let binary_name = std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if config.should_ignore_binary(binary_name) {
                return false;
            }

            // Filter by explicit usage flags
            let usage_match = if dust {
                b.count == 0
            } else if let Some(threshold) = low {
                b.count < threshold as i64
            } else {
                true
            };

            let stale_match = match stale {
                Some(days) => {
                    let threshold = now - (days as i64 * 24 * 60 * 60);
                    b.last_seen.map(|ts| ts < threshold).unwrap_or(true)
                }
                None => true,
            };

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
                println!("  {} No dusty binaries found!", style("●").green().bold());
            } else {
                println!("  {} No matching binaries found.", style("●").yellow());
            }
            println!();
        }
        return Ok(());
    }

    // Calculate totals across all filtered results
    let total_count = filtered.len();
    let total_active = filtered.iter().filter(|b| b.count >= 5).count();
    let total_low = filtered
        .iter()
        .filter(|b| b.count > 0 && b.count < 5)
        .count();
    let total_dusty = filtered.iter().filter(|b| b.count == 0).count();

    // Default mode: hide dusty unless --dust, --all, --low, --stale, or --source was used
    let has_explicit_filter = dust || low.is_some() || stale.is_some() || source.is_some();
    let display: Vec<_> = if all || has_explicit_filter {
        filtered
    } else {
        // Only show active + low (count > 0)
        filtered.into_iter().filter(|b| b.count > 0).collect()
    };

    // Apply terminal height limit
    let effective_limit = if all { 0 } else { terminal_fit(8) };
    let limited: Vec<_> = if effective_limit > 0 && display.len() > effective_limit {
        display.into_iter().take(effective_limit).collect()
    } else {
        display
    };
    let display_count = limited.len();

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
                package_name: b.package_name.clone(),
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

    // Use pager for --all when in a terminal
    let use_pager = all && console::Term::stdout().is_term();

    if use_pager {
        let output = format_report_table(&rows, total_active, total_low, total_dusty, all, has_explicit_filter, effective_limit, display_count, total_count);
        print_with_pager(&output);
    } else {
        print!("{}", format_report_table(&rows, total_active, total_low, total_dusty, all, has_explicit_filter, effective_limit, display_count, total_count));
    }

    Ok(())
}

fn format_report_table(
    rows: &[BinaryJson],
    total_active: usize,
    total_low: usize,
    total_dusty: usize,
    all: bool,
    has_explicit_filter: bool,
    effective_limit: usize,
    display_count: usize,
    total_count: usize,
) -> String {
    use std::fmt::Write;
    // Force ANSI colors when writing to a buffer (needed for pager)
    let is_term = console::Term::stdout().is_term();
    let mut out = String::new();

    macro_rules! s {
        ($expr:expr) => {
            if is_term { $expr.force_styling(true) } else { $expr }
        }
    }

    writeln!(out).unwrap();
    writeln!(
        out,
        "  {:<40} {:>10} {:>8} {:>16}",
        s!(style("Binary").bold().underlined()),
        s!(style("Source").bold().underlined()),
        s!(style("Count").bold().underlined()),
        s!(style("Last Used").bold().underlined())
    ).unwrap();
    writeln!(out).unwrap();

    for row in rows {
        let count_styled = match row.status.as_str() {
            "dusty" => s!(style(format!("{:>8}", row.count)).red()),
            "low" => s!(style(format!("{:>8}", row.count)).yellow()),
            _ => s!(style(format!("{:>8}", row.count)).green()),
        };

        let path_styled = match row.status.as_str() {
            "dusty" => s!(style(format!("{:<40}", truncate_str(&row.short_path, 40))).red()),
            "low" => s!(style(format!("{:<40}", truncate_str(&row.short_path, 40))).yellow()),
            _ => s!(style(format!("{:<40}", truncate_str(&row.short_path, 40))).white()),
        };

        let source_str = row.source.as_deref().unwrap_or("-");
        let last_used = row.last_used.as_deref().unwrap_or("never");

        writeln!(
            out,
            "  {} {:>10} {} {:>16}",
            path_styled, source_str, count_styled, last_used
        ).unwrap();
    }

    writeln!(out).unwrap();

    // Summary line
    write!(out, "  ").unwrap();
    if total_active > 0 {
        write!(out, "{} active  ", s!(style(format!("{}", total_active)).green())).unwrap();
    }
    if total_low > 0 {
        write!(out, "{} low  ", s!(style(format!("{}", total_low)).yellow())).unwrap();
    }
    if total_dusty > 0 {
        write!(out, "{} dusty", s!(style(format!("{}", total_dusty)).red())).unwrap();
    }
    writeln!(out).unwrap();

    if !all && !has_explicit_filter && total_dusty > 0 {
        writeln!(
            out,
            "  {} {} dusty binaries hidden (use {} or {})",
            s!(style("◦").dim()),
            total_dusty,
            s!(style("--dust").cyan()),
            s!(style("--all").cyan())
        ).unwrap();
    }

    if effective_limit > 0 && display_count < total_count && (all || has_explicit_filter) {
        writeln!(
            out,
            "  {} {} more (use {} to show all)",
            s!(style("◦").dim()),
            total_count - display_count,
            s!(style("--all").cyan())
        ).unwrap();
    }

    writeln!(out).unwrap();
    out
}

/// Export uninstall commands for the given binaries
fn export_uninstall_commands(rows: &[BinaryJson]) {
    use std::collections::HashMap;

    let config = config::Config::load().unwrap_or_default();

    // Group by (source, package_name), collecting binary paths
    let mut by_package: HashMap<(String, String), Vec<String>> = HashMap::new();
    for row in rows {
        let source = row.source.clone().unwrap_or_else(|| "other".to_string());
        let pkg = row.package_name.clone().unwrap_or_else(|| {
            std::path::Path::new(&row.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&row.path)
                .to_string()
        });
        by_package
            .entry((source, pkg))
            .or_default()
            .push(row.path.clone());
    }

    // Group package names by source, deduplicated
    let mut by_source: HashMap<String, Vec<String>> = HashMap::new();
    for (source, pkg_name) in by_package.keys() {
        by_source
            .entry(source.clone())
            .or_default()
            .push(pkg_name.clone());
    }
    for pkgs in by_source.values_mut() {
        pkgs.sort();
        pkgs.dedup();
    }

    let total_pkgs: usize = by_source.values().map(|v| v.len()).sum();
    println!("# Uninstall commands for {} packages", total_pkgs);
    println!();

    let mut sources: Vec<_> = by_source.into_iter().collect();
    sources.sort_by(|a, b| a.0.cmp(&b.0));

    for (source, pkgs) in sources {
        match config.get_uninstall_cmd(&source) {
            Some(cmd) => {
                println!("# {} ({} packages)", source, pkgs.len());
                println!("{} {}", cmd, pkgs.join(" "));
                println!();
            }
            None => {
                // No uninstall command — emit rm for each binary path
                let paths: Vec<&str> = by_package
                    .iter()
                    .filter(|((s, _), _)| s == &source)
                    .flat_map(|(_, paths)| paths.iter().map(|p| p.as_str()))
                    .collect();
                if !paths.is_empty() {
                    println!("# {} ({} files)", source, paths.len());
                    for path in paths {
                        println!("rm {}", path);
                    }
                    println!();
                }
            }
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
    println!("  {}  dusty", style("📦").dim());
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

/// A group of binaries belonging to the same (source, package) pair
struct PackageGroup {
    source: String,
    package_name: String,
    binaries: Vec<storage::BinaryRecord>,
}

impl PackageGroup {
    fn is_mixed(&self) -> bool {
        let has_active = self.binaries.iter().any(|b| b.count > 0);
        let has_dusty = self.binaries.iter().any(|b| b.count == 0);
        has_active && has_dusty
    }

    fn binary_names(&self) -> Vec<String> {
        self.binaries
            .iter()
            .map(|b| {
                std::path::Path::new(&b.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            })
            .collect()
    }

    /// Short summary: list names if <= 5, otherwise just show count
    fn binary_summary(&self) -> String {
        let count = self.binaries.len();
        if count <= 5 {
            self.binary_names().join(", ")
        } else {
            format!("{} binaries", count)
        }
    }

    fn active_binary_summary(&self) -> Vec<String> {
        self.binaries
            .iter()
            .filter(|b| b.count > 0)
            .map(|b| {
                let name = std::path::Path::new(&b.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?");
                format!("{} ({}x)", name, b.count)
            })
            .collect()
    }
}

fn build_package_groups(
    binaries: Vec<storage::BinaryRecord>,
    stale: Option<u32>,
    source_filter: Option<&str>,
    config: &config::Config,
) -> Vec<PackageGroup> {
    use std::collections::HashMap;

    let now = chrono::Utc::now().timestamp();

    let filtered: Vec<_> = binaries
        .into_iter()
        .filter(|b| {
            let binary_name = std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            if config.should_ignore_binary(binary_name) {
                return false;
            }

            // Source filter
            if let Some(sf) = source_filter {
                if b.source.as_ref().map(|s| s.as_str()) != Some(sf) {
                    return false;
                }
            }

            // Include if dusty
            if b.count == 0 {
                return true;
            }

            // Include if stale
            if let Some(days) = stale {
                let threshold = now - (days as i64 * 24 * 60 * 60);
                if b.last_seen.map(|ts| ts < threshold).unwrap_or(true) {
                    return true;
                }
            }

            false
        })
        .collect();

    // Group by (source, package_name)
    let mut groups: HashMap<(String, String), Vec<storage::BinaryRecord>> = HashMap::new();
    for b in filtered {
        let source = b.source.clone().unwrap_or_else(|| "other".to_string());
        let pkg = b.package_name.clone().unwrap_or_else(|| {
            std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        groups.entry((source, pkg)).or_default().push(b);
    }

    let mut result: Vec<PackageGroup> = groups
        .into_iter()
        .map(|((source, pkg), bins)| PackageGroup {
            source,
            package_name: pkg,
            binaries: bins,
        })
        .collect();

    result.sort_by(|a, b| a.source.cmp(&b.source).then(a.package_name.cmp(&b.package_name)));
    result
}

fn cmd_clean(dry_run: bool, stale: Option<u32>, source_filter: Option<String>) -> Result<()> {
    use dialoguer::{Confirm, MultiSelect, theme::ColorfulTheme};

    let theme = ColorfulTheme {
        checked_item_prefix: style("● ".to_string()).green(),
        unchecked_item_prefix: style("◦ ".to_string()).dim(),
        success_prefix: style("● ".to_string()).green(),
        ..ColorfulTheme::default()
    };

    let has_filter = stale.is_some() || source_filter.is_some();

    let db = Database::open()?;
    let config = config::Config::load()?;
    sync_binaries(&db)?;

    let binaries = db.get_all_binaries()?;

    // Without a filter, show a summary and ask user to narrow down
    if !has_filter && !dry_run {
        let all_groups = build_package_groups(binaries, None, None, &config);
        if all_groups.is_empty() {
            println!();
            println!(
                "  {} No packages to clean!",
                style("●").green().bold()
            );
            println!();
            return Ok(());
        }

        // Count by source
        let mut by_source: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for g in &all_groups {
            *by_source.entry(g.source.as_str()).or_default() += 1;
        }
        let mut sources: Vec<_> = by_source.into_iter().collect();
        sources.sort_by(|a, b| b.1.cmp(&a.1));

        println!();
        println!(
            "  {} dusty packages found across {} sources:",
            style(all_groups.len()).yellow(),
            style(sources.len()).cyan()
        );
        println!();
        for (source, count) in &sources {
            println!(
                "    {} {:>4}  {}",
                style("●").dim(),
                style(count).bold(),
                source
            );
        }
        println!();
        println!("  Narrow down with a filter:");
        println!(
            "    {}    clean one source",
            style("dusty clean --source homebrew").cyan()
        );
        println!(
            "    {}  clean stale packages",
            style("dusty clean --stale 30").cyan()
        );
        println!(
            "    {}          preview first",
            style("dusty clean --dry-run").cyan()
        );
        println!();
        return Ok(());
    }

    let groups = build_package_groups(binaries, stale, source_filter.as_deref(), &config);

    if groups.is_empty() {
        println!();
        println!(
            "  {} No packages to clean!",
            style("●").green().bold()
        );
        println!();
        return Ok(());
    }

    let total_packages = groups.len();
    let total_binaries: usize = groups.iter().map(|g| g.binaries.len()).sum();
    let mixed_count = groups.iter().filter(|g| g.is_mixed()).count();

    println!();
    println!(
        "  Found {} packages ({} binaries) to review",
        style(total_packages).yellow(),
        style(total_binaries).cyan()
    );

    if mixed_count > 0 {
        println!(
            "  {} {} packages have both active and unused binaries",
            style("!").yellow(),
            mixed_count
        );
    }

    // Dry run mode — same format as interactive items, with pager
    if dry_run {
        use std::fmt::Write;

        let is_term = console::Term::stdout().is_term();
        macro_rules! s {
            ($expr:expr) => {
                if is_term { $expr.force_styling(true) } else { $expr }
            };
        }

        let mut buf = String::new();
        writeln!(buf).ok();
        for group in &groups {
            let bins = group.binary_summary();
            let mixed = if group.is_mixed() {
                format!(" {}", s!(style("!").yellow()))
            } else {
                String::new()
            };
            writeln!(
                buf,
                "  {} {} {} {}{}",
                s!(style("◦").dim()),
                s!(style(&group.package_name).bold()),
                s!(style(format!("({})", group.source)).dim()),
                s!(style(format!("[{}]", bins)).dim()),
                mixed
            )
            .ok();
        }
        writeln!(buf).ok();
        writeln!(buf, "  {} Dry run -- no changes made", s!(style("●").yellow())).ok();
        writeln!(buf).ok();

        let fits = terminal_fit(8);
        if groups.len() > fits {
            print_with_pager(&buf);
        } else {
            print!("{}", buf);
        }
        return Ok(());
    }

    // Build selection items
    let items: Vec<String> = groups
        .iter()
        .map(|g| {
            let bins = g.binary_summary();
            let mixed = if g.is_mixed() {
                format!(" {}", style("!").yellow().force_styling(true))
            } else {
                String::new()
            };
            format!(
                "{} {} {}{}",
                style(&g.package_name).bold().force_styling(true),
                style(format!("({})", g.source)).dim().force_styling(true),
                style(format!("[{}]", bins)).dim().force_styling(true),
                mixed
            )
        })
        .collect();

    let item_refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();

    // Pre-select fully-dusty packages

    // Warn about mixed packages
    for group in &groups {
        if group.is_mixed() {
            println!(
                "  {} {} has active binaries: {}",
                style("!").yellow(),
                style(&group.package_name).bold(),
                group.active_binary_summary().join(", ")
            );
        }
    }

    if mixed_count > 0 {
        println!();
    }

    println!(
        "  {}",
        style("↑/↓ navigate, ←/→ page, Space toggle, a all, Enter confirm, Esc cancel").dim()
    );
    println!();

    let selections = MultiSelect::with_theme(&theme)
        .with_prompt("Select packages to remove")
        .items(&item_refs)
        .max_length(terminal_fit(10).max(10))
        .interact_opt()?;

    let indices = match selections {
        Some(indices) if !indices.is_empty() => indices,
        _ => {
            println!("  {} Nothing selected", style("◦").dim());
            println!();
            return Ok(());
        }
    };

    // Extra confirmation for mixed packages
    let selected_mixed: Vec<&PackageGroup> = indices
        .iter()
        .map(|&i| &groups[i])
        .filter(|g| g.is_mixed())
        .collect();

    if !selected_mixed.is_empty() {
        println!();
        println!(
            "  {} {} selected packages have active binaries that will also be removed:",
            style("!").yellow().bold(),
            selected_mixed.len()
        );
        for g in &selected_mixed {
            println!(
                "    {} {} -> active: {}",
                style("•").yellow(),
                g.package_name,
                g.active_binary_summary().join(", ")
            );
        }

        let confirm = Confirm::with_theme(&theme)
            .with_prompt("Continue with these mixed packages?")
            .default(false)
            .interact()?;

        if !confirm {
            println!("  {} Cancelled", style("◦").dim());
            println!();
            return Ok(());
        }
    }

    // Group selected packages by source for batch uninstall
    let mut by_source: std::collections::HashMap<String, Vec<&PackageGroup>> =
        std::collections::HashMap::new();
    for &i in &indices {
        by_source
            .entry(groups[i].source.clone())
            .or_default()
            .push(&groups[i]);
    }

    let mut total_removed = 0;
    let mut total_failed = 0;

    for (source, pkgs) in &by_source {
        let uninstall_cmd = config.get_uninstall_cmd(source);

        match uninstall_cmd {
            Some(cmd) => {
                // Use package names (not binary names)
                // Reject names with shell metacharacters to prevent injection
                let pkg_names: Vec<&str> = pkgs
                    .iter()
                    .map(|g| g.package_name.as_str())
                    .filter(|name| {
                        let safe = name
                            .chars()
                            .all(|c| c.is_alphanumeric() || "-_.@+".contains(c));
                        if !safe {
                            eprintln!(
                                "  {} Skipping '{}' (unsafe characters in name)",
                                style("●").red(),
                                name
                            );
                        }
                        safe
                    })
                    .collect();

                if pkg_names.is_empty() {
                    continue;
                }

                let full_cmd = format!("{} {}", cmd, pkg_names.join(" "));
                println!();
                println!("  Running: {}", style(&full_cmd).cyan());

                let status = Command::new(defaults::SHELL)
                    .args([defaults::SHELL_CMD_FLAG, &full_cmd])
                    .status()
                    .context("Failed to run uninstall command")?;

                if status.success() {
                    println!(
                        "  {} Removed {} packages",
                        style("●").green(),
                        pkg_names.len()
                    );
                    total_removed += pkg_names.len();
                } else {
                    println!("  {} Some packages failed to remove", style("●").red());
                    total_failed += pkg_names.len();
                }
            }
            None => {
                // No package manager — detect install root directories
                // e.g. /opt/anaconda3/bin/python → /opt/anaconda3/
                let all_paths: Vec<&str> = pkgs
                    .iter()
                    .flat_map(|g| g.binaries.iter().map(|b| b.path.as_str()))
                    .collect();

                let roots = detect_install_roots(&all_paths);

                if roots.is_empty() {
                    continue;
                }

                println!();
                println!(
                    "  {} {} (no package manager — remove directories):",
                    style("●").cyan(),
                    style(source).cyan().bold()
                );
                for root in &roots {
                    println!("    {} {}", style("◦").dim(), root);
                }

                let confirm = Confirm::with_theme(&theme)
                    .with_prompt(format!(
                        "Remove {} directories? (may require sudo)",
                        roots.len()
                    ))
                    .default(false)
                    .interact()?;

                if confirm {
                    for root in &roots {
                        // Safety: refuse to delete paths that are too short
                        // (must have at least 3 components like /opt/something)
                        let components = std::path::Path::new(root).components().count();
                        if components < 3 {
                            println!(
                                "  {} Refusing to delete {} (path too short)",
                                style("●").red(),
                                root
                            );
                            total_failed += 1;
                            continue;
                        }

                        println!(
                            "  Running: {}",
                            style(format!("rm -rf {}", root)).cyan()
                        );
                        if std::fs::remove_dir_all(root).is_ok() {
                            println!("  {} Removed {}", style("●").green(), root);
                            total_removed += 1;
                        } else {
                            println!(
                                "  Running: {}",
                                style(format!("sudo rm -rf {}", root)).cyan()
                            );
                            let status = Command::new(defaults::SUDO)
                                .arg(defaults::RM)
                                .args(defaults::RM_RECURSIVE_FLAGS)
                                .arg(root.as_str())
                                .status();
                            if status.map(|s| s.success()).unwrap_or(false) {
                                println!("  {} Removed {}", style("●").green(), root);
                                total_removed += 1;
                            } else {
                                println!("  {} Failed to remove {}", style("●").red(), root);
                                total_failed += 1;
                            }
                        }
                    }
                } else {
                    println!("  {} Skipped", style("◦").dim());
                }
            }
        }
    }

    println!();
    if total_removed > 0 || total_failed > 0 {
        println!(
            "  {} Removed {}, failed {}",
            style("Summary:").bold(),
            style(total_removed).green(),
            style(total_failed).red()
        );
    }
    println!();

    Ok(())
}

fn cmd_config(edit: bool) -> Result<()> {
    use crate::config::Config;

    // Load config (auto-creates if not exists)
    let _config = Config::load()?;
    let path = Config::config_path()?;

    if edit {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| defaults::DEFAULT_EDITOR.to_string());
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

    // Section header [[foo]] or [foo]
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        print!("{}", style(line).cyan().bold());
        return;
    }

    // Key = value
    if let Some(eq_pos) = line.find('=') {
        let (key, rest) = line.split_at(eq_pos);
        let value = &rest[1..]; // skip the '='
        print!("{}{}", style(key).green(), style("=").dim());
        print_toml_value(value);
        return;
    }

    // Array items (strings in quotes)
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        print!("{}", style(line).yellow());
        return;
    }

    // Array brackets
    if trimmed == "[" || trimmed == "]" || trimmed == "]," {
        print!("{}", style(line).dim());
        return;
    }

    // Fallback
    print!("{}", line);
}

fn print_toml_value(value: &str) {
    let trimmed = value.trim();

    // Boolean
    if trimmed == "true" || trimmed == "false" {
        print!("{}", style(value).magenta());
    }
    // Number
    else if trimmed.parse::<f64>().is_ok() {
        print!("{}", style(value).magenta());
    }
    // Empty array
    else if trimmed == "[]" {
        print!("{}", style(value).dim());
    }
    // Array start
    else if trimmed == "[" {
        print!("{}", style(value).dim());
    }
    // Strings and other values
    else {
        print!("{}", style(value).yellow());
    }
}

fn cmd_dupes(name: Option<String>, expand: bool, json: bool) -> Result<()> {
    use std::collections::HashMap;

    let db = Database::open()?;
    sync_binaries(&db)?;

    let binaries = db.get_all_binaries()?;
    let alias_paths = db.get_all_alias_paths()?;

    // Deduplicate by path (LEFT JOIN can produce multiple rows per binary
    // if it has multiple package entries) and filter out alias paths
    let mut seen_paths = std::collections::HashSet::new();
    let mut by_name: HashMap<String, Vec<_>> = HashMap::new();
    for b in binaries {
        if alias_paths.contains(&b.path) {
            continue;
        }
        if !seen_paths.insert(b.path.clone()) {
            continue;
        }
        let bin_name = std::path::Path::new(&b.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if !bin_name.is_empty() {
            by_name.entry(bin_name).or_default().push(b);
        }
    }

    // Keep only groups with 2+ entries from different sources
    let mut dupes: Vec<(String, Vec<_>)> = by_name
        .into_iter()
        .filter(|(_, copies)| {
            if copies.len() < 2 {
                return false;
            }
            let mut sources = std::collections::HashSet::new();
            for c in copies {
                sources.insert(c.source.as_deref().unwrap_or("unknown"));
            }
            sources.len() > 1
        })
        .collect();

    // Sort: groups with an active winner first, then by name
    dupes.sort_by(|a, b| {
        let a_has_active = a.1.iter().any(|c| c.count > 0);
        let b_has_active = b.1.iter().any(|c| c.count > 0);
        b_has_active.cmp(&a_has_active).then(a.0.cmp(&b.0))
    });

    // Sort copies within each group by count desc
    for (_, copies) in &mut dupes {
        copies.sort_by(|a, b| b.count.cmp(&a.count));
    }

    if json {
        #[derive(serde::Serialize)]
        struct DupeGroup {
            name: String,
            copies: Vec<DupeCopy>,
        }
        #[derive(serde::Serialize)]
        struct DupeCopy {
            path: String,
            source: Option<String>,
            count: i64,
            last_used: Option<String>,
        }

        let groups: Vec<DupeGroup> = dupes
            .iter()
            .map(|(name, copies)| DupeGroup {
                name: name.clone(),
                copies: copies
                    .iter()
                    .map(|c| DupeCopy {
                        path: c.path.clone(),
                        source: c.source.clone(),
                        count: c.count,
                        last_used: c.last_seen.map(|ts| {
                            let dt: DateTime<Local> = Local.timestamp_opt(ts, 0).unwrap();
                            dt.format("%Y-%m-%d %H:%M").to_string()
                        }),
                    })
                    .collect(),
            })
            .collect();

        println!("{}", serde_json::to_string_pretty(&groups)?);
        return Ok(());
    }

    if dupes.is_empty() {
        println!();
        println!(
            "  {} No duplicate binaries found",
            style("●").green().bold()
        );
        println!();
        return Ok(());
    }

    // Detail mode: show expanded view for a specific binary
    if let Some(ref filter_name) = name {
        let matching: Vec<_> = dupes
            .iter()
            .filter(|(n, _)| n == filter_name)
            .collect();

        if matching.is_empty() {
            println!();
            println!(
                "  {} No duplicates found for {}",
                style("◦").dim(),
                style(filter_name).bold()
            );
            println!();
            return Ok(());
        }

        println!();
        for (name, copies) in matching {
            print_dupe_expanded(name, copies);
        }
        return Ok(());
    }

    let total_groups = dupes.len();
    let total_redundant: usize = dupes.iter().map(|(_, c)| c.len() - 1).sum();

    // Expanded mode: show full details for all groups (with pager)
    if expand {
        use std::fmt::Write;
        let is_term = console::Term::stdout().is_term();
        let mut out = String::new();
        writeln!(out).unwrap();
        for (name, copies) in &dupes {
            write_dupe_expanded(&mut out, name, copies, is_term);
        }

        macro_rules! s {
            ($expr:expr) => {
                if is_term { $expr.force_styling(true) } else { $expr }
            }
        }

        writeln!(
            out,
            "  {} {} duplicate binaries ({} redundant copies)",
            s!(style("●").yellow()),
            s!(style(total_groups).yellow()),
            s!(style(total_redundant).yellow())
        ).unwrap();
        writeln!(out).unwrap();

        if is_term {
            print_with_pager(&out);
        } else {
            print!("{}", out);
        }
        return Ok(());
    }

    // Compact mode (default): one line per group, fits terminal
    let limit = terminal_fit(6); // header(2) + summary(3) + padding(1)

    println!();
    println!(
        "  {:<20} {:>7} {}",
        style("Binary").bold().underlined(),
        style("Copies").bold().underlined(),
        style("Sources").bold().underlined()
    );
    println!();

    let shown = if limit > 0 && dupes.len() > limit {
        &dupes[..limit]
    } else {
        &dupes
    };

    for (name, copies) in shown {
        let winner = copies.iter().find(|c| c.count > 0);
        let sources: Vec<&str> = copies
            .iter()
            .map(|c| c.source.as_deref().unwrap_or("-"))
            .collect();

        let summary = if let Some(w) = winner {
            let others: Vec<&str> = sources
                .iter()
                .filter(|&&s| s != w.source.as_deref().unwrap_or("-"))
                .copied()
                .collect();
            format!(
                "{} ({} uses) vs {}",
                w.source.as_deref().unwrap_or("-"),
                w.count,
                others.join(", ")
            )
        } else {
            format!("{} (all unused)", sources.join(", "))
        };

        let name_styled = if winner.is_some() {
            style(format!("{:<20}", truncate_str(name, 20)))
        } else {
            style(format!("{:<20}", truncate_str(name, 20))).dim()
        };

        println!(
            "  {} {:>7} {}",
            name_styled,
            style(copies.len()).dim(),
            style(summary).dim()
        );
    }

    println!();

    if limit > 0 && dupes.len() > limit {
        let with_active = dupes.iter().filter(|(_, c)| c.iter().any(|b| b.count > 0)).count();
        println!(
            "  {} {} more ({} with active winner)",
            style("◦").dim(),
            dupes.len() - limit,
            with_active.saturating_sub(shown.iter().filter(|(_, c)| c.iter().any(|b| b.count > 0)).count())
        );
    }

    println!(
        "  {} {} duplicate binaries ({} redundant copies)",
        style("●").yellow(),
        style(total_groups).yellow(),
        style(total_redundant).yellow()
    );
    println!(
        "  {} Use {} to expand or {} to inspect one",
        style("◦").dim(),
        style("--expand").cyan(),
        style("dusty dupes <name>").cyan()
    );
    println!();

    Ok(())
}

/// Write expanded detail view for one duplicate group to a buffer.
/// `force_colors` should be true when output is destined for a pager.
fn write_dupe_expanded(out: &mut String, name: &str, copies: &[storage::BinaryRecord], force_colors: bool) {
    use std::fmt::Write;

    macro_rules! s {
        ($expr:expr) => {
            if force_colors { $expr.force_styling(true) } else { $expr }
        }
    }

    writeln!(out, "  {}", s!(style(name).bold())).unwrap();

    for (i, c) in copies.iter().enumerate() {
        let source_str = c.source.as_deref().unwrap_or("-");
        let last_used = c
            .last_seen
            .map(|ts| {
                let dt: DateTime<Local> = Local.timestamp_opt(ts, 0).unwrap();
                dt.format("%Y-%m-%d").to_string()
            })
            .unwrap_or_else(|| "never".to_string());

        let is_winner = i == 0 && c.count > 0;

        if is_winner {
            writeln!(
                out,
                "    {} {:<40} {:>10} {:>8} {:>12}",
                s!(style("●").green()),
                shorten_path(&c.path),
                source_str,
                s!(style(c.count).green()),
                last_used
            ).unwrap();
        } else {
            let count_styled = if c.count == 0 {
                s!(style(format!("{}", c.count)).red())
            } else {
                s!(style(format!("{}", c.count)).yellow())
            };
            writeln!(
                out,
                "    {} {:<40} {:>10} {:>8} {:>12}",
                s!(style("◦").dim()),
                s!(style(shorten_path(&c.path)).dim()),
                s!(style(source_str).dim()),
                count_styled,
                s!(style(&last_used).dim())
            ).unwrap();
        }
    }
    writeln!(out).unwrap();
}

/// Print expanded detail view directly (for single-binary detail mode)
fn print_dupe_expanded(name: &str, copies: &[storage::BinaryRecord]) {
    let mut out = String::new();
    write_dupe_expanded(&mut out, name, copies, false);
    print!("{}", out);
}

fn cmd_deps(
    orphans_only: bool,
    binary: Option<String>,
    refresh: bool,
    json: bool,
) -> Result<()> {
    let db = Database::open()?;
    sync_binaries(&db)?;

    // Single binary mode
    if let Some(binary_path) = binary {
        let result = deps::analyze_single_binary(&db, &binary_path)?;

        if json {
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }

        println!();

        if result.libs.is_empty() {
            println!(
                "  {} {} has no dynamic library dependencies",
                style("◦").dim(),
                shorten_path(&result.binary_path)
            );
            println!();
            return Ok(());
        }

        println!(
            "  {} {} ({} dependencies)",
            style("●").cyan(),
            shorten_path(&result.binary_path),
            result.libs.len()
        );
        println!();

        println!(
            "  {:<50} {}",
            style("Library").bold().underlined(),
            style("Package").bold().underlined()
        );
        println!();

        for lib in &result.libs {
            let pkg_display = match (&lib.package_name, &lib.manager) {
                (Some(pkg), Some(mgr)) => format!("{} ({})", pkg, mgr),
                _ => "-".to_string(),
            };
            let lib_name = std::path::Path::new(&lib.lib_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&lib.lib_path);
            println!(
                "  {:<50} {}",
                style(lib_name).dim(),
                pkg_display
            );
        }

        println!();
        return Ok(());
    }

    // Full analysis mode
    let term = console::Term::stderr();
    let dots = [".", "..", "..."];
    let _ = term.hide_cursor();
    let report = deps::analyze_deps(&db, refresh, Some(&|current, total| {
        let dot = dots[current % dots.len()];
        let msg = format!(
            "  {} {}/{}{}",
            style("Analyzing dependencies").cyan(),
            style(current).bold(),
            style(total).bold(),
            dot,
        );
        let _ = term.write_str(&format!("\r{:<70}", msg));
    }))?;
    let _ = term.show_cursor();
    let _ = term.write_str(&format!("\r{:<70}\r", ""));

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!();

    if !orphans_only {
        println!(
            "  {} {} binaries analyzed",
            style("◦").dim(),
            report.binaries_analyzed
        );
        println!(
            "  {} {} library packages found",
            style("◦").dim(),
            report.total_lib_packages
        );
    }

    if report.orphan_packages.is_empty() {
        println!();
        println!(
            "  {} No orphan library packages found",
            style("●").green().bold()
        );
        println!();
        return Ok(());
    }

    println!(
        "  {} {} orphan packages (only used by dusty binaries)",
        style("●").yellow(),
        style(report.orphan_packages.len()).yellow()
    );
    println!();

    // Table
    println!(
        "  {:<40} {:>10} {:>16}",
        style("Package").bold().underlined(),
        style("Size").bold().underlined(),
        style("Used By").bold().underlined()
    );
    println!();

    for orphan in &report.orphan_packages {
        let size_str = orphan
            .size_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "?".to_string());
        let users: Vec<String> = orphan
            .used_by_dusty
            .iter()
            .map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(p)
                    .to_string()
            })
            .collect();
        let users_str = users.join(", ");

        println!(
            "  {:<40} {:>10} {:>16}",
            style(truncate_str(&orphan.package_name, 40)).red(),
            style(&size_str).dim(),
            style(truncate_str(&users_str, 16)).dim()
        );
    }

    println!();
    println!(
        "  {} Total freeable: {}",
        style("●").green(),
        style(format_bytes(report.total_freeable_bytes)).green().bold()
    );
    println!();

    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn cmd_daemon() -> Result<()> {
    let db = Database::open()?;
    let config = config::Config::load()?;
    let mut monitor = Monitor::new();

    eprintln!("dusty daemon starting...");

    let rx = monitor.start()?;

    eprintln!("dusty daemon running, listening for exec events...");

    for path in rx {
        if should_skip_path(&path) {
            continue;
        }

        let source = config.categorize_path(&path);
        if let Err(e) = db.record_exec(&path, Some(&source)) {
            eprintln!("Failed to record exec for {}: {}", path, e);
        }
    }

    Ok(())
}

/// Pipe content through a pager ($PAGER or less -R) if stdout is a terminal.
/// Falls back to printing directly if pager isn't available or stdout is piped.
fn print_with_pager(content: &str) {
    use std::io::Write;

    if !console::Term::stdout().is_term() {
        print!("{}", content);
        return;
    }

    let pager_cmd = std::env::var("PAGER").unwrap_or_else(|_| defaults::DEFAULT_PAGER.to_string());
    let (program, base_args): (&str, Vec<&str>) = if pager_cmd.contains("less") {
        (pager_cmd.as_str(), vec![defaults::PAGER_COLOR_FLAG]) // -R for ANSI color passthrough
    } else {
        (pager_cmd.as_str(), vec![])
    };

    match Command::new(program)
        .args(&base_args)
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(content.as_bytes());
            }
            let _ = child.wait();
        }
        Err(_) => {
            // Pager not available, print directly
            print!("{}", content);
        }
    }
}

/// Returns how many content rows fit in the terminal, reserving `overhead` lines
/// for headers, summaries, and padding. Returns 0 if detection fails (show all).
fn terminal_fit(overhead: usize) -> usize {
    console::Term::stdout()
        .size_checked()
        .map(|(rows, _)| (rows as usize).saturating_sub(overhead))
        .unwrap_or(0)
}

/// Detect install root directories from a set of binary paths.
/// e.g. ["/opt/anaconda3/bin/python", "/opt/anaconda3/bin/conda"] → ["/opt/anaconda3"]
/// Walks up from each binary path to find a reasonable root (one level below
/// a well-known parent like /opt, /usr/local, or $HOME).
fn detect_install_roots(paths: &[&str]) -> Vec<String> {
    use std::collections::BTreeSet;

    let home = dirs::home_dir()
        .map(|h| h.display().to_string())
        .unwrap_or_default();

    // Well-known parent dirs — one level below these is the install root
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
                if let Some(first_component) = rest.split('/').next() {
                    if !first_component.is_empty() {
                        roots.insert(format!("{}{}", anchor, first_component));
                        break;
                    }
                }
            }
        }
    }

    roots.into_iter().collect()
}

fn shorten_path(path: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.display().to_string())
        .unwrap_or_default();

    for &(prefix, replacement) in defaults::PATH_SHORTHANDS {
        let expanded = prefix.replace('~', &home);
        if path.starts_with(&expanded) {
            return format!("{}{}", replacement, &path[expanded.len()..]);
        }
    }
    path.to_string()
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
