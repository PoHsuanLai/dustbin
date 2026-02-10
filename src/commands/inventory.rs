use anyhow::Result;
use console::style;
use std::process::Command;

use crate::config::Config;
use crate::defaults;
use crate::ui::truncate_str;

pub fn cmd_inventory(source_filter: Option<String>, all: bool, json: bool) -> Result<()> {
    let config = Config::load()?;
    let list_sources = config.get_sources_with_list_cmd();

    if list_sources.is_empty() {
        if json {
            println!("[]");
        } else {
            println!();
            println!(
                "  {} No sources with {} configured",
                style("●").yellow(),
                style("list_cmd").cyan()
            );
            println!();
            println!(
                "  {} Add to config ({}):",
                style("◦").dim(),
                style("dusty config --edit").cyan()
            );
            println!();
            println!("    {}", style("[[sources]]").cyan());
            println!("    {} \"pip-user\"", style("name =").green());
            println!("    {} \"~/.local/lib/python\"", style("path =").green());
            println!(
                "    {} \"pip list --format=freeze | cut -d= -f1\"",
                style("list_cmd =").green()
            );
            println!(
                "    {} \"pip uninstall -y\"",
                style("uninstall_cmd =").green()
            );
            println!();
        }
        return Ok(());
    }

    // Filter to specific source if requested
    let sources: Vec<_> = if let Some(ref filter) = source_filter {
        list_sources
            .into_iter()
            .filter(|s| &s.name == filter)
            .collect()
    } else {
        list_sources
    };

    if sources.is_empty() {
        if json {
            println!("[]");
        } else {
            println!();
            println!(
                "  {} No source {} has a {} configured",
                style("●").yellow(),
                style(source_filter.as_deref().unwrap_or("?")).bold(),
                style("list_cmd").cyan()
            );
            println!();
        }
        return Ok(());
    }

    // Run list_cmd for each source
    let mut results: Vec<(&str, Vec<String>)> = Vec::new();

    for source in &sources {
        let cmd = source.list_cmd.as_deref().unwrap();
        let output = Command::new(defaults::SHELL)
            .args([defaults::SHELL_CMD_FLAG, cmd])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let packages: Vec<String> = stdout
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                results.push((&source.name, packages));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !json {
                    println!();
                    println!(
                        "  {} {} list_cmd failed: {}",
                        style("●").red(),
                        style(&source.name).bold(),
                        stderr.trim()
                    );
                }
            }
            Err(e) => {
                if !json {
                    println!();
                    println!(
                        "  {} {} list_cmd error: {}",
                        style("●").red(),
                        style(&source.name).bold(),
                        e
                    );
                }
            }
        }
    }

    if json {
        #[derive(serde::Serialize)]
        struct InventoryJson {
            source: String,
            packages: Vec<String>,
            count: usize,
        }

        let rows: Vec<InventoryJson> = results
            .iter()
            .map(|(source, pkgs)| InventoryJson {
                source: source.to_string(),
                packages: pkgs.clone(),
                count: pkgs.len(),
            })
            .collect();

        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if results.is_empty() {
        println!();
        println!("  {} No packages found", style("●").yellow());
        println!();
        return Ok(());
    }

    let total_packages: usize = results.iter().map(|(_, pkgs)| pkgs.len()).sum();
    let show_expanded = all || source_filter.is_some();

    println!();
    for (source, packages) in &results {
        println!(
            "  {} ({} packages)",
            style(source).bold(),
            style(packages.len()).cyan()
        );

        if packages.is_empty() {
            println!("    {}", style("(none)").dim());
        } else if show_expanded {
            // Full list: one per line
            for pkg in packages {
                println!("    {} {}", style("◦").dim(), pkg);
            }
        } else {
            // Compact: comma-separated, truncated
            let summary = packages.join(", ");
            let truncated = truncate_str(&summary, 70);
            println!("    {}", style(truncated).dim());
        }
        println!();
    }

    println!(
        "  {} {} sources, {} packages",
        style("●").green(),
        results.len(),
        style(total_packages).bold()
    );
    println!(
        "  {} Usage tracking not available — use {} for tracked binaries",
        style("◦").dim(),
        style("dusty report").cyan()
    );
    if !show_expanded && total_packages > 0 {
        println!(
            "  {} Use {} or {} to see all",
            style("◦").dim(),
            style("--all").cyan(),
            style("--source <name>").cyan()
        );
    }
    println!();

    Ok(())
}
