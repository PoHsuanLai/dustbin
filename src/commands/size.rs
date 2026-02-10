use anyhow::Result;
use console::style;
use serde::Serialize;
use std::collections::HashMap;
use std::process::Command;

use crate::config;
use crate::storage::{self, Database};
use crate::ui::{Spinner, format_bytes, print_with_pager, terminal_fit, truncate_str};
use crate::utils::{detect_install_roots, start_daemon, sync_binaries};

pub fn cmd_size(dust: bool, source_filter: Option<String>, json: bool) -> Result<()> {
    let db = Database::open()?;
    let config = config::Config::load()?;
    sync_binaries(&db)?;
    start_daemon(true)?;

    let binaries = db.get_all_binaries()?;

    if binaries.is_empty() {
        if json {
            println!("[]");
        } else {
            println!();
            println!("  {} No binaries found.", style("●").yellow());
            println!();
        }
        return Ok(());
    }

    // Group by (source, package_name)
    let mut groups: HashMap<(String, String), Vec<&storage::BinaryRecord>> = HashMap::new();
    for b in &binaries {
        let binary_name = std::path::Path::new(&b.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if config.should_ignore_binary(binary_name) {
            continue;
        }
        let source = b.source.clone().unwrap_or_else(|| "other".to_string());
        let pkg = b.package_name.clone().unwrap_or_else(|| {
            std::path::Path::new(&b.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

        if let Some(ref sf) = source_filter {
            if &source != sf {
                continue;
            }
        }

        groups.entry((source, pkg)).or_default().push(b);
    }

    if dust {
        groups.retain(|_, bins| bins.iter().all(|b| b.count == 0));
    }

    #[derive(Serialize)]
    struct SizeEntry {
        source: String,
        package_name: String,
        size_bytes: Option<u64>,
        size_display: String,
        binary_count: usize,
        status: String,
    }

    // Batch-compute sizes: collect all install roots, run one `du -sk` call
    let spinner = Spinner::new();
    spinner.message("Calculating sizes");
    let size_map = batch_dir_sizes(&groups);
    spinner.finish();

    let mut entries: Vec<SizeEntry> = Vec::new();

    for ((source, pkg), bins) in &groups {
        let key = (source.clone(), pkg.clone());
        let size = size_map.get(&key).copied().flatten();

        let has_active = bins.iter().any(|b| b.count > 0);
        let has_dusty = bins.iter().any(|b| b.count == 0);
        let status = if has_active && has_dusty {
            "mixed"
        } else if has_active {
            "active"
        } else {
            "dusty"
        };

        entries.push(SizeEntry {
            source: source.clone(),
            package_name: pkg.clone(),
            size_bytes: size,
            size_display: size.map(format_bytes).unwrap_or_else(|| "?".to_string()),
            binary_count: bins.len(),
            status: status.to_string(),
        });
    }

    entries.sort_by(|a, b| b.size_bytes.unwrap_or(0).cmp(&a.size_bytes.unwrap_or(0)));

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    // Human output
    let use_pager = entries.len() > terminal_fit(6) && console::Term::stdout().is_term();
    let mut out = String::new();
    use std::fmt::Write;

    writeln!(out).unwrap();
    writeln!(
        out,
        "  {:<30} {:<12} {:>10} {:>6} {:>8}",
        style("Package").bold().underlined(),
        style("Source").bold().underlined(),
        style("Size").bold().underlined(),
        style("Bins").bold().underlined(),
        style("Status").bold().underlined(),
    )
    .unwrap();

    for entry in &entries {
        let status_styled = match entry.status.as_str() {
            "dusty" => style(&entry.status).red().to_string(),
            "mixed" => style(&entry.status).yellow().to_string(),
            _ => style(&entry.status).green().to_string(),
        };

        writeln!(
            out,
            "  {:<30} {:<12} {:>10} {:>6} {:>8}",
            truncate_str(&entry.package_name, 30),
            &entry.source,
            &entry.size_display,
            entry.binary_count,
            status_styled,
        )
        .unwrap();
    }

    let total_bytes: u64 = entries.iter().filter_map(|e| e.size_bytes).sum();
    let dusty_bytes: u64 = entries
        .iter()
        .filter(|e| e.status == "dusty")
        .filter_map(|e| e.size_bytes)
        .sum();

    writeln!(out).unwrap();
    writeln!(
        out,
        "  {} {} packages, {} total",
        style("●").green(),
        entries.len(),
        style(format_bytes(total_bytes)).bold(),
    )
    .unwrap();
    if dusty_bytes > 0 {
        writeln!(
            out,
            "  {} {} reclaimable (dusty packages)",
            style("●").red(),
            style(format_bytes(dusty_bytes)).red().bold(),
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    if use_pager {
        print_with_pager(&out);
    } else {
        print!("{}", out);
    }

    Ok(())
}

/// Batch-compute sizes for all package groups using a single `du -sk` call.
/// Returns a map from (source, package_name) to Option<u64> bytes.
fn batch_dir_sizes(
    groups: &HashMap<(String, String), Vec<&storage::BinaryRecord>>,
) -> HashMap<(String, String), Option<u64>> {
    let mut result: HashMap<(String, String), Option<u64>> = HashMap::new();

    let home = dirs::home_dir()
        .map(|h| h.display().to_string())
        .unwrap_or_default();

    // Homebrew Cellar paths can be looked up directly per package
    let cellar_prefixes = ["/opt/homebrew/Cellar", "/usr/local/Cellar"];

    // Collect du targets: either Cellar paths (for homebrew) or install roots (for /opt/*)
    let mut du_path_to_key: HashMap<String, (String, String)> = HashMap::new();
    let mut binary_sum_keys: Vec<(String, String)> = Vec::new();

    for ((source, pkg), bins) in groups {
        let key = (source.clone(), pkg.clone());

        // For homebrew: use Cellar path directly (fast, per-package)
        if source == "homebrew" {
            let mut found_cellar = false;
            for prefix in &cellar_prefixes {
                let cellar_path = format!("{}/{}", prefix, pkg);
                if std::path::Path::new(&cellar_path).exists() {
                    du_path_to_key.insert(cellar_path, key.clone());
                    found_cellar = true;
                    break;
                }
            }
            if !found_cellar {
                binary_sum_keys.push(key);
            }
            continue;
        }

        // For non-homebrew: detect install root
        let paths: Vec<&str> = bins.iter().map(|b| b.path.as_str()).collect();
        let roots = detect_install_roots(&paths);
        if let Some(root) = roots.into_iter().next() {
            // Skip home dir roots (too broad), skip /opt/homebrew (covered above)
            if !root.starts_with(&home) && root != "/opt/homebrew" {
                du_path_to_key.insert(root, key);
            } else {
                binary_sum_keys.push(key);
            }
        } else {
            binary_sum_keys.push(key);
        }
    }

    // Single `du -sk` call for all collected paths
    if !du_path_to_key.is_empty() {
        let du_paths: Vec<&str> = du_path_to_key.keys().map(|s| s.as_str()).collect();
        let output = Command::new("du").arg("-sk").args(&du_paths).output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let mut parts = line.split_whitespace();
                if let (Some(size_str), Some(path)) = (parts.next(), parts.next()) {
                    if let Ok(kb) = size_str.parse::<u64>() {
                        if let Some(key) = du_path_to_key.get(path) {
                            result.insert(key.clone(), Some(kb * 1024));
                        }
                    }
                }
            }
        }

        // Fill in any paths that du didn't report
        for key in du_path_to_key.values() {
            result.entry(key.clone()).or_insert(None);
        }
    }

    // For remaining packages, sum individual binary file sizes
    for key in &binary_sum_keys {
        if let Some(bins) = groups.get(key) {
            let mut total = 0u64;
            let mut found_any = false;
            for b in bins {
                if let Ok(meta) = std::fs::metadata(&b.path) {
                    total += meta.len();
                    found_any = true;
                }
            }
            result.insert(key.clone(), if found_any { Some(total) } else { None });
        }
    }

    result
}
