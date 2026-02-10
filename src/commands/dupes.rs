use anyhow::Result;
use chrono::{DateTime, Local};
use console::style;
use std::collections::{HashMap, HashSet};

use crate::storage::{self, Database};
use crate::ui::{print_with_pager, shorten_path, terminal_fit, truncate_str};
use crate::utils::{local_datetime, sync_binaries};

pub fn cmd_dupes(name: Option<String>, all: bool, json: bool) -> Result<()> {
    let db = Database::open()?;
    sync_binaries(&db)?;

    let binaries = db.get_all_binaries()?;
    let alias_paths = db.get_all_alias_paths()?;

    // Deduplicate by path and filter out alias paths
    let mut seen_paths = HashSet::new();
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
            let mut sources = HashSet::new();
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
                            let dt: DateTime<Local> = local_datetime(ts);
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
        let matching: Vec<_> = dupes.iter().filter(|(n, _)| n == filter_name).collect();

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
    if all {
        use std::fmt::Write;
        let is_term = console::Term::stdout().is_term();
        let mut out = String::new();
        writeln!(out).unwrap();
        for (name, copies) in &dupes {
            write_dupe_expanded(&mut out, name, copies, is_term);
        }

        macro_rules! s {
            ($expr:expr) => {
                if is_term {
                    $expr.force_styling(true)
                } else {
                    $expr
                }
            };
        }

        writeln!(
            out,
            "  {} {} duplicate binaries ({} redundant copies)",
            s!(style("●").yellow()),
            s!(style(total_groups).yellow()),
            s!(style(total_redundant).yellow())
        )
        .unwrap();
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
        let with_active = dupes
            .iter()
            .filter(|(_, c)| c.iter().any(|b| b.count > 0))
            .count();
        println!(
            "  {} {} more ({} with active winner)",
            style("◦").dim(),
            dupes.len() - limit,
            with_active.saturating_sub(
                shown
                    .iter()
                    .filter(|(_, c)| c.iter().any(|b| b.count > 0))
                    .count()
            )
        );
    }

    println!(
        "  {} {} duplicate binaries ({} redundant copies)",
        style("●").yellow(),
        style(total_groups).yellow(),
        style(total_redundant).yellow()
    );
    println!(
        "  {} Use {} to show all or {} to inspect one",
        style("◦").dim(),
        style("--all").cyan(),
        style("dusty dupes <name>").cyan()
    );
    println!();

    Ok(())
}

/// Write expanded detail view for one duplicate group to a buffer.
/// `force_colors` should be true when output is destined for a pager.
fn write_dupe_expanded(
    out: &mut String,
    name: &str,
    copies: &[storage::BinaryRecord],
    force_colors: bool,
) {
    use std::fmt::Write;

    macro_rules! s {
        ($expr:expr) => {
            if force_colors {
                $expr.force_styling(true)
            } else {
                $expr
            }
        };
    }

    writeln!(out, "  {}", s!(style(name).bold())).unwrap();

    for (i, c) in copies.iter().enumerate() {
        let source_str = c.source.as_deref().unwrap_or("-");
        let last_used = c
            .last_seen
            .map(|ts| {
                let dt: DateTime<Local> = local_datetime(ts);
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
            )
            .unwrap();
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
            )
            .unwrap();
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
