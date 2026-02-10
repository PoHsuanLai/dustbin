use anyhow::Result;
use chrono::{DateTime, Local};
use console::style;
use std::collections::HashMap;
use std::process::Command;

use crate::defaults;
use crate::storage::Database;
use crate::ui::{Spinner, format_bytes};
use crate::utils::local_datetime;

pub fn cmd_trash(drop: Option<String>, empty: bool, json: bool) -> Result<()> {
    let db = Database::open()?;
    let items = db.list_trash()?;

    // Drop a specific package from trash
    if let Some(ref name) = drop {
        let matches = db.get_trash_by_name(name)?;
        if matches.is_empty() {
            println!();
            println!(
                "  {} No trashed package found matching {}",
                style("●").yellow(),
                style(name).bold()
            );
            println!();
            return Ok(());
        }

        println!();
        let mut removed = 0;
        for item in &matches {
            if let Some(ref tp) = item.trash_path {
                let path = std::path::Path::new(tp);
                if path.exists() {
                    println!("  Running: {}", style(format!("rm -rf {}", tp)).cyan());
                    if std::fs::remove_dir_all(path).is_ok() {
                        removed += 1;
                    } else {
                        eprintln!("  {} Failed to remove {}", style("●").red(), tp);
                    }
                }
            }
            db.delete_trash(item.id)?;
        }

        println!(
            "  {} Permanently deleted {} ({} directories removed)",
            style("●").green(),
            style(name).bold(),
            removed
        );
        println!();
        return Ok(());
    }

    if empty {
        if items.is_empty() {
            println!();
            println!("  {} Trash is already empty", style("●").green().bold());
            println!();
            return Ok(());
        }

        // Delete files for "moved" items
        let trash_dir = dirs::data_local_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find local data directory"))?
            .join("dusty")
            .join(defaults::TRASH_DIR);

        println!();
        let mut removed = 0;
        for item in &items {
            if let Some(ref tp) = item.trash_path {
                let path = std::path::Path::new(tp);
                if path.exists() {
                    println!("  Running: {}", style(format!("rm -rf {}", tp)).cyan());
                    if std::fs::remove_dir_all(path).is_ok() {
                        removed += 1;
                    } else {
                        eprintln!("  {} Failed to remove {}", style("●").red(), tp);
                    }
                }
            }
        }

        // Remove the trash directory itself if empty
        if trash_dir.exists() {
            std::fs::remove_dir(&trash_dir).ok();
        }

        db.clear_all_trash()?;

        println!();
        println!(
            "  {} Emptied trash ({} items, {} directories removed)",
            style("●").green(),
            items.len(),
            removed
        );
        println!();
        return Ok(());
    }

    if items.is_empty() {
        if json {
            println!("[]");
        } else {
            println!();
            println!("  {} Trash is empty", style("●").green().bold());
            println!();
        }
        return Ok(());
    }

    // Compute sizes with spinner
    let spinner = Spinner::new();
    spinner.message("Calculating sizes");
    let sizes = batch_trash_sizes(&items);
    spinner.finish();

    if json {
        #[derive(serde::Serialize)]
        struct TrashJson {
            id: i64,
            package_name: String,
            source: String,
            method: String,
            original_path: String,
            trash_path: Option<String>,
            size_bytes: Option<u64>,
            deleted_at: String,
            restore_cmd: Option<String>,
        }

        let rows: Vec<TrashJson> = items
            .iter()
            .map(|item| {
                let dt: DateTime<Local> = local_datetime(item.deleted_at);
                TrashJson {
                    id: item.id,
                    package_name: item.package_name.clone(),
                    source: item.source.clone(),
                    method: item.method.clone(),
                    original_path: item.original_path.clone(),
                    trash_path: item.trash_path.clone(),
                    size_bytes: item
                        .trash_path
                        .as_ref()
                        .and_then(|tp| sizes.get(tp.as_str()).copied()),
                    deleted_at: dt.format("%Y-%m-%d %H:%M").to_string(),
                    restore_cmd: item.restore_cmd.clone(),
                }
            })
            .collect();

        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    println!();
    println!(
        "  {:<25} {:<12} {:>10} {:<18} {}",
        style("Package").bold().underlined(),
        style("Source").bold().underlined(),
        style("Size").bold().underlined(),
        style("Method").bold().underlined(),
        style("Deleted").bold().underlined(),
    );
    println!();

    let mut total_size: u64 = 0;

    for item in &items {
        let dt: DateTime<Local> = local_datetime(item.deleted_at);
        let date_str = dt.format("%Y-%m-%d %H:%M").to_string();

        let method_str = match item.method.as_str() {
            "moved" => style("moved to trash".to_string()).yellow(),
            "package_manager" => style("uninstalled".to_string()).dim(),
            _ => style(item.method.clone()).dim(),
        };

        let size_str = item
            .trash_path
            .as_ref()
            .and_then(|tp| sizes.get(tp.as_str()).copied())
            .map(|bytes| {
                total_size += bytes;
                format_bytes(bytes)
            })
            .unwrap_or_else(|| "-".to_string());

        println!(
            "  {:<25} {:<12} {:>10} {:<18} {}",
            style(&item.package_name).bold(),
            style(&item.source).dim(),
            size_str,
            method_str,
            style(date_str).dim(),
        );
    }

    let moved_count = items.iter().filter(|i| i.method == "moved").count();
    let pkg_count = items
        .iter()
        .filter(|i| i.method == "package_manager")
        .count();

    println!();
    if moved_count > 0 {
        let size_summary = if total_size > 0 {
            format!(" ({})", format_bytes(total_size))
        } else {
            String::new()
        };
        println!(
            "  {} {} moved to trash{}",
            style("●").yellow(),
            moved_count,
            size_summary
        );
    }
    if pkg_count > 0 {
        println!(
            "  {} {} uninstalled via package manager",
            style("◦").dim(),
            pkg_count
        );
    }
    println!(
        "  {} Use {} to restore, {} or {} to permanently delete",
        style("◦").dim(),
        style("dusty restore <name>").cyan(),
        style("dusty trash --drop <name>").cyan(),
        style("--empty").cyan()
    );
    println!();

    Ok(())
}

/// Compute sizes for moved trash items using `du -sk` per path.
/// Runs each path individually to handle spaces in paths correctly.
fn batch_trash_sizes(items: &[crate::storage::TrashRecord]) -> HashMap<String, u64> {
    let mut result = HashMap::new();

    for item in items {
        if item.method != "moved" {
            continue;
        }
        let Some(ref tp) = item.trash_path else {
            continue;
        };
        if !std::path::Path::new(tp).exists() {
            continue;
        }

        // Run du -sk on a single path — no whitespace parsing issues
        let output = Command::new("du").args(["-sk", tp]).output();
        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(size_str) = stdout.split_whitespace().next() {
                if let Ok(kb) = size_str.parse::<u64>() {
                    result.insert(tp.clone(), kb * 1024);
                }
            }
        }
    }

    result
}
