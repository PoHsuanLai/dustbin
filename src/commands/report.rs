use anyhow::Result;
use chrono::{DateTime, Local};
use console::style;
use serde::Serialize;
use std::collections::HashMap;

use crate::config;
use crate::storage::{BinaryRecord, Database};
use crate::ui::{print_with_pager, terminal_fit};
use crate::utils::{local_datetime, start_daemon, sync_binaries};

#[derive(Serialize)]
struct PackageJson {
    package_name: String,
    source: String,
    binaries: usize,
    total_uses: i64,
    last_used: Option<String>,
    status: String,
}

/// Aggregate binaries into packages
struct PackageInfo {
    package_name: String,
    source: String,
    binaries: usize,
    total_uses: i64,
    last_seen: Option<i64>,
}

fn aggregate_packages(binaries: &[BinaryRecord]) -> Vec<PackageInfo> {
    let mut map: HashMap<(String, String), (usize, i64, Option<i64>)> = HashMap::new();

    for b in binaries {
        let pkg = b.package_name.clone().unwrap_or_else(|| {
            std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        let source = b.source.clone().unwrap_or_else(|| "other".to_string());

        let entry = map.entry((pkg, source)).or_insert((0, 0, None));
        entry.0 += 1;
        entry.1 += b.count;
        entry.2 = match (entry.2, b.last_seen) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
    }

    let mut packages: Vec<PackageInfo> = map
        .into_iter()
        .map(|((pkg, source), (bins, uses, last))| PackageInfo {
            package_name: pkg,
            source,
            binaries: bins,
            total_uses: uses,
            last_seen: last,
        })
        .collect();

    // Sort: active first (by uses desc), then dusty (by binary count desc)
    packages.sort_by(|a, b| {
        b.total_uses
            .cmp(&a.total_uses)
            .then(b.binaries.cmp(&a.binaries))
            .then(a.package_name.cmp(&b.package_name))
    });

    packages
}

pub fn cmd_report(
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

    sync_binaries(&db)?;
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

    // Filter binaries before aggregation
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

            match &source {
                Some(s) => b.source.as_ref().map(|bs| bs == s).unwrap_or(false),
                None => true,
            }
        })
        .collect();

    // Aggregate into packages
    let packages = aggregate_packages(&filtered);

    // Apply usage filters at the package level
    let filtered_pkgs: Vec<_> = packages
        .into_iter()
        .filter(|p| {
            let usage_match = if dust {
                p.total_uses == 0
            } else if let Some(threshold) = low {
                p.total_uses < threshold as i64
            } else {
                true
            };

            let stale_match = match stale {
                Some(days) => {
                    let threshold = now - (days as i64 * 24 * 60 * 60);
                    p.last_seen.map(|ts| ts < threshold).unwrap_or(true)
                }
                None => true,
            };

            usage_match && stale_match
        })
        .collect();

    if filtered_pkgs.is_empty() {
        if json {
            println!("[]");
        } else {
            println!();
            if dust {
                println!("  {} No dusty packages found!", style("●").green().bold());
            } else {
                println!("  {} No matching packages found.", style("●").yellow());
            }
            println!();
        }
        return Ok(());
    }

    let total_count = filtered_pkgs.len();
    let total_active = filtered_pkgs.iter().filter(|p| p.total_uses >= 5).count();
    let total_low = filtered_pkgs
        .iter()
        .filter(|p| p.total_uses > 0 && p.total_uses < 5)
        .count();
    let total_dusty = filtered_pkgs.iter().filter(|p| p.total_uses == 0).count();

    // Default mode: hide dusty unless --dust, --all, --low, --stale, or --source
    let has_explicit_filter = dust || low.is_some() || stale.is_some() || source.is_some();
    let display: Vec<_> = if all || has_explicit_filter {
        filtered_pkgs
    } else {
        filtered_pkgs
            .into_iter()
            .filter(|p| p.total_uses > 0)
            .collect()
    };

    // Terminal height limit
    let effective_limit = if all { 0 } else { terminal_fit(8) };
    let limited: Vec<_> = if effective_limit > 0 && display.len() > effective_limit {
        display.into_iter().take(effective_limit).collect()
    } else {
        display
    };
    let display_count = limited.len();

    // Build output rows
    let rows: Vec<PackageJson> = limited
        .iter()
        .map(|p| {
            let last_used = p.last_seen.map(|ts| {
                let dt: DateTime<Local> = local_datetime(ts);
                dt.format("%Y-%m-%d %H:%M").to_string()
            });

            let status = if p.total_uses == 0 {
                "dusty"
            } else if p.total_uses < 5 {
                "low"
            } else {
                "active"
            };

            PackageJson {
                package_name: p.package_name.clone(),
                source: p.source.clone(),
                binaries: p.binaries,
                total_uses: p.total_uses,
                last_used,
                status: status.to_string(),
            }
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }

    if export {
        export_uninstall_commands(&rows);
        return Ok(());
    }

    let use_pager = all && console::Term::stdout().is_term();
    let output = format_report_table(
        &rows,
        total_active,
        total_low,
        total_dusty,
        all,
        has_explicit_filter,
        effective_limit,
        display_count,
        total_count,
    );

    if use_pager {
        print_with_pager(&output);
    } else {
        print!("{}", output);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn format_report_table(
    rows: &[PackageJson],
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
    let is_term = console::Term::stdout().is_term();
    let mut out = String::new();

    macro_rules! s {
        ($expr:expr) => {
            if is_term {
                $expr.force_styling(true)
            } else {
                $expr
            }
        };
    }

    writeln!(out).unwrap();
    writeln!(
        out,
        "  {:<30} {:>10} {:>8} {:>8} {:>16}",
        s!(style("Package").bold().underlined()),
        s!(style("Source").bold().underlined()),
        s!(style("Bins").bold().underlined()),
        s!(style("Uses").bold().underlined()),
        s!(style("Last Used").bold().underlined())
    )
    .unwrap();
    writeln!(out).unwrap();

    for row in rows {
        let uses_styled = match row.status.as_str() {
            "dusty" => s!(style(format!("{:>8}", row.total_uses)).red()),
            "low" => s!(style(format!("{:>8}", row.total_uses)).yellow()),
            _ => s!(style(format!("{:>8}", row.total_uses))),
        };

        let name_display = if row.package_name.len() > 30 {
            format!("{}...", &row.package_name[..27])
        } else {
            row.package_name.clone()
        };

        let name_styled = match row.status.as_str() {
            "dusty" => s!(style(format!("{:<30}", name_display)).red()),
            "low" => s!(style(format!("{:<30}", name_display)).yellow()),
            _ => s!(style(format!("{:<30}", name_display))),
        };

        let last_used = row.last_used.as_deref().unwrap_or("never");

        writeln!(
            out,
            "  {} {:>10} {:>8} {} {:>16}",
            name_styled, row.source, row.binaries, uses_styled, last_used
        )
        .unwrap();
    }

    writeln!(out).unwrap();

    // Summary
    write!(out, "  ").unwrap();
    if total_active > 0 {
        write!(
            out,
            "{} active  ",
            s!(style(format!("{}", total_active)).green())
        )
        .unwrap();
    }
    if total_low > 0 {
        write!(
            out,
            "{} low  ",
            s!(style(format!("{}", total_low)).yellow())
        )
        .unwrap();
    }
    if total_dusty > 0 {
        write!(out, "{} dusty", s!(style(format!("{}", total_dusty)).red())).unwrap();
    }
    writeln!(out).unwrap();

    if !all && !has_explicit_filter && total_dusty > 0 {
        writeln!(
            out,
            "  {} {} dusty packages hidden (use {} or {})",
            s!(style("◦").dim()),
            total_dusty,
            s!(style("--dust").cyan()),
            s!(style("--all").cyan())
        )
        .unwrap();
    }

    if effective_limit > 0 && display_count < total_count && (all || has_explicit_filter) {
        writeln!(
            out,
            "  {} {} more (use {} to show all)",
            s!(style("◦").dim()),
            total_count - display_count,
            s!(style("--all").cyan())
        )
        .unwrap();
    }

    writeln!(out).unwrap();
    out
}

/// Export uninstall commands for the given packages
fn export_uninstall_commands(rows: &[PackageJson]) {
    let config = config::Config::load().unwrap_or_default();

    // Group package names by source
    let mut by_source: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        by_source
            .entry(row.source.clone())
            .or_default()
            .push(row.package_name.clone());
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
                println!(
                    "# {} ({} packages, no uninstall command)",
                    source,
                    pkgs.len()
                );
                for pkg in &pkgs {
                    println!("# rm -rf <install_root>/{}", pkg);
                }
                println!();
            }
        }
    }
}
