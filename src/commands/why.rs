use anyhow::Result;
use console::style;
use serde::Serialize;

use crate::config;
use crate::storage::Database;
use crate::ui::shorten_path;
use crate::utils::{detect_install_roots, local_datetime, sync_binaries};

pub fn cmd_why(name: String, json: bool) -> Result<()> {
    let db = Database::open()?;
    let config = config::Config::load()?;
    sync_binaries(&db)?;

    let binaries = db.get_all_binaries()?;

    // Try matching by binary name first, then fall back to package name
    let mut matches: Vec<&crate::storage::BinaryRecord> = binaries
        .iter()
        .filter(|b| {
            std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                == Some(name.as_str())
        })
        .collect();

    let matched_by_package = if matches.is_empty() {
        matches = binaries
            .iter()
            .filter(|b| b.package_name.as_deref() == Some(name.as_str()))
            .collect();
        !matches.is_empty()
    } else {
        false
    };

    if matches.is_empty() {
        if json {
            #[derive(Serialize)]
            struct Empty {
                name: String,
                matches: Vec<()>,
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&Empty {
                    name: name.clone(),
                    matches: vec![],
                })?
            );
        } else {
            println!();
            println!(
                "  {} No binary or package named '{}' found",
                style("◦").dim(),
                style(&name).bold()
            );
            println!();
        }
        return Ok(());
    }

    // When matched by package name, show a single package summary
    if matched_by_package {
        return show_package_summary(&name, &matches, &config, &binaries, json);
    }

    // Binary-level matches: show each match with its package context
    #[derive(Serialize)]
    struct WhyJson {
        name: String,
        matches: Vec<WhyMatch>,
    }

    #[derive(Serialize)]
    struct WhyMatch {
        path: String,
        source: Option<String>,
        package_name: Option<String>,
        count: i64,
        last_used: Option<String>,
        first_seen: Option<String>,
        install_root: Option<String>,
        siblings: Vec<String>,
        sibling_count: usize,
        uninstall_cmd: Option<String>,
    }

    let mut why_matches: Vec<WhyMatch> = Vec::new();

    for m in &matches {
        let install_root = detect_install_roots(&[m.path.as_str()]).into_iter().next();

        let siblings: Vec<String> = if let (Some(src), Some(pkg)) = (&m.source, &m.package_name) {
            binaries
                .iter()
                .filter(|b| {
                    b.path != m.path
                        && b.source.as_ref() == Some(src)
                        && b.package_name.as_ref() == Some(pkg)
                })
                .map(|b| {
                    std::path::Path::new(&b.path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?")
                        .to_string()
                })
                .collect()
        } else {
            vec![]
        };

        let sibling_count = siblings.len();

        let uninstall_cmd = m
            .source
            .as_ref()
            .and_then(|s| config.get_uninstall_cmd(s))
            .map(|cmd| {
                let pkg = m.package_name.as_deref().unwrap_or(&name);
                format!("{} {}", cmd, pkg)
            });

        let last_used = m
            .last_seen
            .map(|ts| local_datetime(ts).format("%Y-%m-%d %H:%M").to_string());

        let first_seen = m
            .first_seen
            .map(|ts| local_datetime(ts).format("%Y-%m-%d %H:%M").to_string());

        why_matches.push(WhyMatch {
            path: m.path.clone(),
            source: m.source.clone(),
            package_name: m.package_name.clone(),
            count: m.count,
            last_used,
            first_seen,
            install_root,
            siblings,
            sibling_count,
            uninstall_cmd,
        });
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&WhyJson {
                name: name.clone(),
                matches: why_matches,
            })?
        );
        return Ok(());
    }

    println!();
    for wm in &why_matches {
        println!("  {}", style(&name).bold());
        println!();
        println!("    {}  {}", style("Path:").dim(), shorten_path(&wm.path));
        if let Some(ref src) = wm.source {
            println!("    {}  {}", style("Source:").dim(), src);
        }
        if let Some(ref pkg) = wm.package_name {
            println!("    {}  {}", style("Package:").dim(), pkg);
        }
        if let Some(ref root) = wm.install_root {
            println!("    {}  {}", style("Root:").dim(), root);
        }

        let count_styled = if wm.count == 0 {
            style(format!("{} (dusty)", wm.count)).red()
        } else if wm.count < 5 {
            style(format!("{} (low)", wm.count)).yellow()
        } else {
            style(format!("{} (active)", wm.count)).green()
        };
        println!("    {}  {}", style("Uses:").dim(), count_styled);

        if let Some(ref last) = wm.last_used {
            println!("    {}  {}", style("Last used:").dim(), last);
        }
        if let Some(ref first) = wm.first_seen {
            println!("    {}  {}", style("Tracked since:").dim(), first);
        }

        if wm.sibling_count > 0 {
            let display = if wm.sibling_count <= 5 {
                wm.siblings.join(", ")
            } else {
                format!("{} other binaries", wm.sibling_count)
            };
            println!("    {}  {}", style("Also in package:").dim(), display);
        }

        if let Some(ref cmd) = wm.uninstall_cmd {
            println!("    {}  {}", style("Uninstall:").dim(), style(cmd).cyan());
        }
        println!();
    }

    Ok(())
}

/// Show a package-level summary when the user looked up a package name
fn show_package_summary(
    name: &str,
    matches: &[&crate::storage::BinaryRecord],
    config: &config::Config,
    _all_binaries: &[crate::storage::BinaryRecord],
    json: bool,
) -> Result<()> {
    let total_bins = matches.len();
    let total_uses: i64 = matches.iter().map(|b| b.count).sum();
    let used_bins = matches.iter().filter(|b| b.count > 0).count();
    let source = matches
        .first()
        .and_then(|b| b.source.as_deref())
        .unwrap_or("other");
    let install_root = detect_install_roots(&[matches[0].path.as_str()])
        .into_iter()
        .next();

    let last_seen = matches.iter().filter_map(|b| b.last_seen).max();

    let uninstall_cmd = config
        .get_uninstall_cmd(source)
        .map(|cmd| format!("{} {}", cmd, name));

    // Top used binaries
    let mut by_use: Vec<_> = matches.iter().collect();
    by_use.sort_by(|a, b| b.count.cmp(&a.count));

    if json {
        #[derive(Serialize)]
        struct PkgJson {
            package_name: String,
            source: String,
            binaries: usize,
            used_binaries: usize,
            total_uses: i64,
            last_used: Option<String>,
            install_root: Option<String>,
            uninstall_cmd: Option<String>,
            top_binaries: Vec<BinEntry>,
        }
        #[derive(Serialize)]
        struct BinEntry {
            name: String,
            uses: i64,
        }

        let top: Vec<BinEntry> = by_use
            .iter()
            .take(10)
            .map(|b| BinEntry {
                name: std::path::Path::new(&b.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string(),
                uses: b.count,
            })
            .collect();

        println!(
            "{}",
            serde_json::to_string_pretty(&PkgJson {
                package_name: name.to_string(),
                source: source.to_string(),
                binaries: total_bins,
                used_binaries: used_bins,
                total_uses,
                last_used: last_seen
                    .map(|ts| local_datetime(ts).format("%Y-%m-%d %H:%M").to_string()),
                install_root,
                uninstall_cmd,
                top_binaries: top,
            })?
        );
        return Ok(());
    }

    println!();
    println!("  {}", style(name).bold());
    println!();
    println!("    {}  {}", style("Source:").dim(), source);
    if let Some(ref root) = install_root {
        println!("    {}  {}", style("Root:").dim(), root);
    }
    println!(
        "    {}  {} ({} used)",
        style("Binaries:").dim(),
        total_bins,
        used_bins
    );

    let status = if total_uses == 0 {
        style(format!("{} (dusty)", total_uses)).red()
    } else if total_uses < 5 {
        style(format!("{} (low)", total_uses)).yellow()
    } else {
        style(format!("{} (active)", total_uses)).green()
    };
    println!("    {}  {}", style("Total uses:").dim(), status);

    if let Some(ts) = last_seen {
        println!(
            "    {}  {}",
            style("Last used:").dim(),
            local_datetime(ts).format("%Y-%m-%d %H:%M")
        );
    }

    // Show top binaries
    let show_count = 10.min(by_use.len());
    if used_bins > 0 {
        println!();
        println!("    {}", style("Top binaries:").dim());
        for b in by_use.iter().take(show_count) {
            let bin_name = std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            if b.count > 0 {
                println!("      {:>6}  {}", style(b.count).green(), bin_name);
            }
        }
    }

    if total_bins > show_count {
        println!(
            "      {}  {} more binaries",
            style("...").dim(),
            total_bins - show_count
        );
    }

    if let Some(ref cmd) = uninstall_cmd {
        println!();
        println!("    {}  {}", style("Uninstall:").dim(), style(cmd).cyan());
    }
    println!();

    Ok(())
}
