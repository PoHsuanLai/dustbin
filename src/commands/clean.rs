use anyhow::{Context, Result};
use console::style;
use std::collections::HashMap;
use std::process::Command;

use crate::config;
use crate::defaults;
use crate::storage::{self, Database};
use crate::ui::{print_with_pager, terminal_fit};
use crate::utils::{detect_install_roots, sync_binaries};

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
            if let Some(sf) = source_filter
                && b.source.as_deref() != Some(sf)
            {
                return false;
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

    result.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.package_name.cmp(&b.package_name))
    });
    result
}

pub fn cmd_clean(
    dry_run: bool,
    stale: Option<u32>,
    source_filter: Option<String>,
    no_trash: bool,
) -> Result<()> {
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
            println!("  {} No packages to clean!", style("●").green().bold());
            println!();
            return Ok(());
        }

        // Count by source
        let mut by_source: HashMap<&str, usize> = HashMap::new();
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
                style("◦").dim(),
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
        // If source has a list_cmd, use that instead of DB
        if let Some(ref sf) = source_filter {
            if let Some(list_cmd) = config.get_list_cmd(sf) {
                return clean_from_list_cmd(sf, &list_cmd, &config, dry_run, &theme);
            }
        }

        println!();
        println!("  {} No packages to clean!", style("●").green().bold());
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

    // Dry run mode -- same format as interactive items, with pager
    if dry_run {
        use std::fmt::Write;

        let is_term = console::Term::stdout().is_term();
        macro_rules! s {
            ($expr:expr) => {
                if is_term {
                    $expr.force_styling(true)
                } else {
                    $expr
                }
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
        writeln!(
            buf,
            "  {} Dry run -- no changes made",
            s!(style("●").yellow())
        )
        .ok();
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
    let mut by_source: HashMap<String, Vec<&PackageGroup>> = HashMap::new();
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
                    // Record trash receipts for package manager removals
                    let install_cmd = defaults::install_cmd_from_uninstall(&cmd);
                    for pkg_name in &pkg_names {
                        let restore = install_cmd
                            .as_ref()
                            .map(|ic| format!("{} {}", ic, pkg_name));
                        db.record_trash(
                            pkg_name,
                            None,
                            source.as_str(),
                            pkg_name,
                            "package_manager",
                            restore.as_deref(),
                        )
                        .ok();
                    }

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
                // No package manager -- detect install root directories
                let all_paths: Vec<&str> = pkgs
                    .iter()
                    .flat_map(|g| g.binaries.iter().map(|b| b.path.as_str()))
                    .collect();

                let roots = detect_install_roots(&all_paths);

                if roots.is_empty() {
                    continue;
                }

                let action = if no_trash { "remove" } else { "trash" };
                println!();
                println!(
                    "  {} {} (no package manager -- {} directories):",
                    style("●").yellow(),
                    style(source).yellow().bold(),
                    action
                );
                for root in &roots {
                    println!("    {} {}", style("◦").dim(), root);
                }

                let prompt = if no_trash {
                    format!(
                        "Permanently remove {} directories? (may require sudo)",
                        roots.len()
                    )
                } else {
                    format!("Move {} directories to trash?", roots.len())
                };

                let confirm = Confirm::with_theme(&theme)
                    .with_prompt(prompt)
                    .default(false)
                    .interact()?;

                if confirm {
                    // Derive a single package_name for the group
                    let pkg_name = pkgs
                        .first()
                        .map(|g| g.package_name.as_str())
                        .unwrap_or("unknown");

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

                        if no_trash {
                            // Permanent deletion (old behavior)
                            println!("  Running: {}", style(format!("rm -rf {}", root)).cyan());
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
                        } else {
                            // Move to trash
                            match move_to_trash(root, &db, source, pkg_name) {
                                Ok(trash_path) => {
                                    println!(
                                        "  {} Trashed {} → {}",
                                        style("●").green(),
                                        root,
                                        style(&trash_path).dim()
                                    );
                                    total_removed += 1;
                                }
                                Err(e) => {
                                    println!(
                                        "  {} Failed to trash {}: {}",
                                        style("●").red(),
                                        root,
                                        e
                                    );
                                    total_failed += 1;
                                }
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
        if !no_trash {
            println!(
                "  {} Use {} to see trashed items, {} to undo",
                style("◦").dim(),
                style("dusty trash").cyan(),
                style("dusty restore <name>").cyan()
            );
        }

        // Show autoremove hints for sources that were cleaned
        let mut shown = std::collections::HashSet::new();
        for source in by_source.keys() {
            if let Some(hint) = defaults::autoremove_hint(source) {
                if shown.insert(hint) {
                    println!(
                        "  {} Run {} to remove orphaned dependencies",
                        style("◦").dim(),
                        style(hint).cyan()
                    );
                }
            }
        }
    }
    println!();

    Ok(())
}

/// Move a directory to the trash instead of deleting it.
/// Returns the trash path on success.
fn move_to_trash(
    root: &str,
    db: &storage::Database,
    source: &str,
    package_name: &str,
) -> Result<String> {
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find local data directory"))?;
    let trash_dir = data_dir.join("dusty").join(defaults::TRASH_DIR);
    std::fs::create_dir_all(&trash_dir)?;

    let dir_name = std::path::Path::new(root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let timestamp = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S");
    let dest = trash_dir.join(format!("{}_{}", timestamp, dir_name));
    let dest_str = dest.to_string_lossy().to_string();

    // Try rename (fast if same filesystem)
    if std::fs::rename(root, &dest).is_ok() {
        db.record_trash(root, Some(&dest_str), source, package_name, "moved", None)?;
        return Ok(dest_str);
    }

    // Cross-filesystem or permission issue: try sudo mv
    let status = Command::new(defaults::SUDO)
        .args(["mv", root, &dest_str])
        .status()
        .context("Failed to run sudo mv")?;

    if status.success() {
        db.record_trash(root, Some(&dest_str), source, package_name, "moved", None)?;
        Ok(dest_str)
    } else {
        anyhow::bail!("Failed to move {} to trash", root)
    }
}

/// Clean packages from a source that uses list_cmd (e.g., R, pip).
/// Runs list_cmd to get installed packages, shows MultiSelect, then uninstalls.
fn clean_from_list_cmd(
    source: &str,
    list_cmd: &str,
    config: &config::Config,
    dry_run: bool,
    theme: &dialoguer::theme::ColorfulTheme,
) -> Result<()> {
    use dialoguer::MultiSelect;

    println!();
    println!(
        "  {} Querying {} packages...",
        style("◦").dim(),
        style(source).bold()
    );

    let output = Command::new(defaults::SHELL)
        .args([defaults::SHELL_CMD_FLAG, list_cmd])
        .output()
        .context("Failed to run list_cmd")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("  {} list_cmd failed: {}", style("●").red(), stderr.trim());
        println!();
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let packages: Vec<String> = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if packages.is_empty() {
        println!("  {} No packages found", style("●").green().bold());
        println!();
        return Ok(());
    }

    println!(
        "  Found {} packages from {}",
        style(packages.len()).yellow(),
        style(source).bold()
    );

    if dry_run {
        println!();
        for pkg in &packages {
            println!("  {} {}", style("◦").dim(), pkg);
        }
        println!();
        println!("  {} Dry run -- no changes made", style("●").yellow());
        println!();
        return Ok(());
    }

    println!(
        "  {}",
        style("↑/↓ navigate, ←/→ page, Space toggle, a all, Enter confirm, Esc cancel").dim()
    );
    println!();

    let item_refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();

    let selections = MultiSelect::with_theme(theme)
        .with_prompt("Select packages to remove")
        .items(&item_refs)
        .max_length(crate::ui::terminal_fit(10).max(10))
        .interact_opt()?;

    let indices = match selections {
        Some(indices) if !indices.is_empty() => indices,
        _ => {
            println!("  {} Nothing selected", style("◦").dim());
            println!();
            return Ok(());
        }
    };

    let uninstall_cmd = config.get_uninstall_cmd(source);

    let Some(cmd) = uninstall_cmd else {
        println!(
            "  {} No uninstall_cmd configured for {}",
            style("●").red(),
            source
        );
        println!();
        return Ok(());
    };

    let selected: Vec<&str> = indices.iter().map(|&i| packages[i].as_str()).collect();
    let has_template = cmd.contains("%s");

    let mut total_removed = 0;
    let mut total_failed = 0;

    if has_template {
        // Template mode: one invocation per package (%s replaced with name)
        for &pkg in &selected {
            let safe = pkg
                .chars()
                .all(|c| c.is_alphanumeric() || "-_.@+".contains(c));
            if !safe {
                eprintln!(
                    "  {} Skipping '{}' (unsafe characters)",
                    style("●").red(),
                    pkg
                );
                continue;
            }

            let full_cmd = cmd.replace("%s", pkg);
            println!("  Running: {}", style(&full_cmd).cyan());

            let status = Command::new(defaults::SHELL)
                .args([defaults::SHELL_CMD_FLAG, &full_cmd])
                .status()
                .context("Failed to run uninstall command")?;

            if status.success() {
                total_removed += 1;
            } else {
                total_failed += 1;
            }
        }
    } else {
        // Append mode: batch uninstall
        let safe_pkgs: Vec<&str> = selected
            .iter()
            .copied()
            .filter(|pkg| {
                let safe = pkg
                    .chars()
                    .all(|c| c.is_alphanumeric() || "-_.@+".contains(c));
                if !safe {
                    eprintln!(
                        "  {} Skipping '{}' (unsafe characters)",
                        style("●").red(),
                        pkg
                    );
                }
                safe
            })
            .collect();

        if !safe_pkgs.is_empty() {
            let full_cmd = format!("{} {}", cmd, safe_pkgs.join(" "));
            println!("  Running: {}", style(&full_cmd).cyan());

            let status = Command::new(defaults::SHELL)
                .args([defaults::SHELL_CMD_FLAG, &full_cmd])
                .status()
                .context("Failed to run uninstall command")?;

            if status.success() {
                total_removed += safe_pkgs.len();
            } else {
                total_failed += safe_pkgs.len();
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
