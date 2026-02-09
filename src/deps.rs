//! Dynamic library dependency analysis and orphan detection

use crate::platform::{Analyzer, DylibAnalyzer};
use crate::storage::Database;
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Full dependency analysis result
#[derive(Debug, Serialize)]
pub struct DepsReport {
    pub orphan_packages: Vec<OrphanPackage>,
    pub total_freeable_bytes: u64,
    pub binaries_analyzed: usize,
    pub total_lib_packages: usize,
}

/// A library package only used by dusty binaries
#[derive(Debug, Serialize)]
pub struct OrphanPackage {
    pub manager: String,
    pub package_name: String,
    pub size_bytes: Option<u64>,
    pub used_by_dusty: Vec<String>,
}

/// Result of analyzing a single binary's deps (for --binary mode)
#[derive(Debug, Serialize)]
pub struct SingleBinaryDeps {
    pub binary_path: String,
    pub libs: Vec<ResolvedLib>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedLib {
    pub lib_path: String,
    pub package_name: Option<String>,
    pub manager: Option<String>,
}

/// Run the full dependency analysis pipeline
pub fn analyze_deps(
    db: &Database,
    refresh: bool,
    progress_callback: Option<&dyn Fn(usize, usize)>,
) -> Result<DepsReport> {
    if refresh {
        db.clear_all_deps()?;
    }

    let binaries = db.get_all_binaries()?;
    let total = binaries.len();

    // Phase 1: Analyze each binary's dylib dependencies
    for (i, binary) in binaries.iter().enumerate() {
        if let Some(cb) = &progress_callback {
            cb(i, total);
        }

        if !refresh && !needs_reanalysis(db, &binary.path)? {
            continue;
        }

        match Analyzer::analyze_binary(&binary.path) {
            Ok(analysis) => {
                let lib_paths: Vec<String> = analysis.libs.iter().map(|l| l.path.clone()).collect();
                db.store_dylib_deps(&binary.path, &lib_paths)?;
                let mtime = get_file_mtime(&binary.path);
                db.mark_deps_analyzed(&binary.path, mtime)?;
            }
            Err(_) => {
                db.store_dylib_deps(&binary.path, &[])?;
                db.mark_deps_analyzed(&binary.path, None)?;
            }
        }
    }

    // Phase 2: Resolve unresolved library paths to packages
    let unresolved = db.get_unresolved_libs()?;
    if !unresolved.is_empty() {
        let resolved = Analyzer::resolve_lib_packages(&unresolved)?;
        for info in &resolved {
            db.store_lib_package(&info.lib_path, &info.manager, &info.package_name)?;
        }
    }

    // Phase 3: Build orphan report
    let binary_counts: Vec<(String, i64)> =
        binaries.iter().map(|b| (b.path.clone(), b.count)).collect();
    build_orphan_report(db, &binary_counts)
}

/// Analyze a single binary and resolve its deps
pub fn analyze_single_binary(db: &Database, binary_path: &str) -> Result<SingleBinaryDeps> {
    let analysis = Analyzer::analyze_binary(binary_path)?;
    let lib_paths: Vec<String> = analysis.libs.iter().map(|l| l.path.clone()).collect();

    // Store in DB for caching
    db.store_dylib_deps(binary_path, &lib_paths)?;
    db.mark_deps_analyzed(binary_path, get_file_mtime(binary_path))?;

    // Resolve any new libs
    let unresolved = db.get_unresolved_libs()?;
    if !unresolved.is_empty() {
        let resolved = Analyzer::resolve_lib_packages(&unresolved)?;
        for info in &resolved {
            db.store_lib_package(&info.lib_path, &info.manager, &info.package_name)?;
        }
    }

    // Build result with resolved package info
    let all_lib_pkgs = db.get_all_lib_packages()?;
    let lib_pkg_map: HashMap<String, (String, String)> = all_lib_pkgs
        .into_iter()
        .map(|(lib_path, manager, pkg)| (lib_path, (manager, pkg)))
        .collect();

    let libs = lib_paths
        .iter()
        .map(|lib_path| {
            let resolved = lib_pkg_map.get(lib_path);
            ResolvedLib {
                lib_path: lib_path.clone(),
                package_name: resolved.map(|(_, pkg)| pkg.clone()),
                manager: resolved.map(|(mgr, _)| mgr.clone()),
            }
        })
        .collect();

    Ok(SingleBinaryDeps {
        binary_path: binary_path.to_string(),
        libs,
    })
}

fn needs_reanalysis(db: &Database, binary_path: &str) -> Result<bool> {
    if let Some((_analyzed_at, cached_mtime)) = db.get_deps_analyzed_at(binary_path)? {
        let current_mtime = get_file_mtime(binary_path);
        Ok(current_mtime != cached_mtime)
    } else {
        Ok(true)
    }
}

fn get_file_mtime(path: &str) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

fn build_orphan_report(
    db: &Database,
    binaries: &[(String, i64)],
) -> Result<DepsReport> {
    let dusty_paths: HashSet<&str> = binaries
        .iter()
        .filter(|(_, count)| *count == 0)
        .map(|(path, _)| path.as_str())
        .collect();

    let active_paths: HashSet<&str> = binaries
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(path, _)| path.as_str())
        .collect();

    // Build: (manager, package_name) -> set of binary_paths that use it
    let all_lib_packages = db.get_all_lib_packages()?;
    let mut pkg_to_users: HashMap<(String, String), HashSet<String>> = HashMap::new();

    for (lib_path, manager, pkg_name) in &all_lib_packages {
        let users = db.get_binaries_using_lib(lib_path)?;
        pkg_to_users
            .entry((manager.clone(), pkg_name.clone()))
            .or_default()
            .extend(users);
    }

    let total_lib_packages = pkg_to_users.len();

    // Find orphans: packages where ALL users are dusty
    let mut orphans = Vec::new();
    let mut total_freeable = 0u64;

    for ((manager, pkg_name), users) in &pkg_to_users {
        let has_active_user = users.iter().any(|u| active_paths.contains(u.as_str()));
        if has_active_user {
            continue;
        }

        let dusty_users: Vec<String> = users
            .iter()
            .filter(|u| dusty_paths.contains(u.as_str()))
            .cloned()
            .collect();

        if dusty_users.is_empty() {
            continue;
        }

        let size = Analyzer::get_package_size(manager, pkg_name).unwrap_or(None);
        if let Some(s) = size {
            total_freeable += s;
        }

        orphans.push(OrphanPackage {
            manager: manager.clone(),
            package_name: pkg_name.clone(),
            size_bytes: size,
            used_by_dusty: dusty_users,
        });
    }

    // Sort by size descending
    orphans.sort_by(|a, b| b.size_bytes.unwrap_or(0).cmp(&a.size_bytes.unwrap_or(0)));

    Ok(DepsReport {
        orphan_packages: orphans,
        total_freeable_bytes: total_freeable,
        binaries_analyzed: binaries.len(),
        total_lib_packages,
    })
}
