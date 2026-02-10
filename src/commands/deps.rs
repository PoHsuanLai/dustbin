use anyhow::Result;
use console::style;
use std::cell::RefCell;

use crate::deps;
use crate::storage::Database;
use crate::ui::{Spinner, format_bytes, shorten_path, truncate_str};
use crate::utils::sync_binaries;

pub fn cmd_deps(
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
            style("●").green(),
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
            println!("  {:<50} {}", style(lib_name).dim(), pkg_display);
        }

        println!();
        return Ok(());
    }

    // Full analysis mode
    let spinner = RefCell::new(Spinner::new());
    let report = deps::analyze_deps(
        &db,
        refresh,
        Some(&|current, total| {
            spinner
                .borrow_mut()
                .update("Analyzing dependencies", current, total);
        }),
    )?;
    spinner.into_inner().finish();

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
        style(format_bytes(report.total_freeable_bytes))
            .green()
            .bold()
    );
    println!();

    Ok(())
}
