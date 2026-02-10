use anyhow::Result;
use console::style;
use std::process::Command;

use crate::defaults;
use crate::storage::Database;

pub fn cmd_restore(name: String) -> Result<()> {
    let db = Database::open()?;
    let matches = db.get_trash_by_name(&name)?;

    if matches.is_empty() {
        println!();
        println!(
            "  {} No trashed package found matching {}",
            style("●").yellow(),
            style(&name).bold()
        );
        println!(
            "  {} Use {} to see trashed items",
            style("◦").dim(),
            style("dusty trash").cyan()
        );
        println!();
        return Ok(());
    }

    // Restore the most recent match
    let item = &matches[0];

    println!();
    match item.method.as_str() {
        "moved" => {
            let trash_path = item.trash_path.as_deref().unwrap_or("");
            let original = &item.original_path;

            if trash_path.is_empty() || !std::path::Path::new(trash_path).exists() {
                println!(
                    "  {} Trash directory no longer exists: {}",
                    style("●").red(),
                    trash_path
                );
                println!();
                return Ok(());
            }

            if std::path::Path::new(original).exists() {
                println!(
                    "  {} Original path already exists: {}",
                    style("●").red(),
                    original
                );
                println!("  {} Trash location: {}", style("◦").dim(), trash_path);
                println!();
                return Ok(());
            }

            // Try rename first
            if std::fs::rename(trash_path, original).is_ok() {
                db.delete_trash(item.id)?;
                println!(
                    "  {} Restored {} → {}",
                    style("●").green(),
                    style(&item.package_name).bold(),
                    original
                );
                println!();
                return Ok(());
            }

            // Fallback: sudo mv
            let status = Command::new(defaults::SUDO)
                .args(["mv", trash_path, original])
                .status();

            if status.map(|s| s.success()).unwrap_or(false) {
                db.delete_trash(item.id)?;
                println!(
                    "  {} Restored {} → {}",
                    style("●").green(),
                    style(&item.package_name).bold(),
                    original
                );
            } else {
                println!(
                    "  {} Failed to restore {} from {}",
                    style("●").red(),
                    item.package_name,
                    trash_path
                );
            }
        }
        "package_manager" => {
            if let Some(ref cmd) = item.restore_cmd {
                println!(
                    "  {} {} was uninstalled via package manager",
                    style("●").yellow(),
                    style(&item.package_name).bold()
                );
                println!("  {} To reinstall, run:", style("◦").dim());
                println!();
                println!("    {}", style(cmd).cyan());
                println!();

                // Remove from trash since user has the info
                db.delete_trash(item.id)?;
            } else {
                println!(
                    "  {} {} was uninstalled but no reinstall command is known",
                    style("●").yellow(),
                    style(&item.package_name).bold()
                );
                println!("  {} Source: {}", style("◦").dim(), &item.source);
                db.delete_trash(item.id)?;
            }
        }
        _ => {
            println!(
                "  {} Unknown trash method: {}",
                style("●").red(),
                item.method
            );
        }
    }
    println!();

    Ok(())
}
