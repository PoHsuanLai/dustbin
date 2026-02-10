use anyhow::Result;
use console::style;
use serde::Serialize;
use std::collections::HashMap;

use crate::storage::Database;
use crate::utils::sync_binaries;

#[derive(Serialize)]
struct StatsJson {
    tracking_days: i64,
    total_packages: usize,
    total_binaries: usize,
    active: usize,
    low: usize,
    dusty: usize,
    by_source: HashMap<String, usize>,
}

pub fn cmd_stats(json: bool) -> Result<()> {
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

    let total_binaries = binaries.len();

    // Aggregate into packages
    let mut pkg_map: HashMap<(String, String), (i64, Option<i64>)> = HashMap::new();
    for b in &binaries {
        let pkg = b.package_name.clone().unwrap_or_else(|| {
            std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        let source = b.source.clone().unwrap_or_else(|| "other".to_string());
        let entry = pkg_map.entry((pkg, source)).or_insert((0, None));
        entry.0 += b.count;
    }

    let total_packages = pkg_map.len();
    let active = pkg_map.values().filter(|(uses, _)| *uses >= 5).count();
    let low = pkg_map
        .values()
        .filter(|(uses, _)| *uses > 0 && *uses < 5)
        .count();
    let dusty = pkg_map.values().filter(|(uses, _)| *uses == 0).count();

    // Count packages by source
    let mut by_source: HashMap<String, usize> = HashMap::new();
    for (_, source) in pkg_map.keys() {
        *by_source.entry(source.clone()).or_insert(0) += 1;
    }

    if json {
        let stats = StatsJson {
            tracking_days: days,
            total_packages,
            total_binaries,
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
    println!("  dusty");
    println!("  {}", style("─".repeat(40)).dim());
    println!();

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
    let total = total_packages;
    let active_width = if total > 0 {
        active * bar_width / total
    } else {
        0
    };
    let low_width = if total > 0 {
        low * bar_width / total
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

    println!(
        "  {} packages ({} binaries)",
        style(format!("{}", total_packages)).bold(),
        total_binaries
    );
    println!("  {}", bar);
    println!();

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
        let bar_len = (**count * source_bar_width / max_count).max(1);
        println!(
            "  {:>10}  {} {}",
            source,
            style("▪".repeat(bar_len)).cyan(),
            style(count).dim()
        );
    }

    if sources.len() > 8 {
        let others: usize = sources.iter().skip(8).map(|(_, c)| **c).sum();
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
