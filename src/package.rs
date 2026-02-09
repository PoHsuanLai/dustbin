use crate::config::Config;
use anyhow::Result;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Scan all directories in PATH and return all executable binaries
/// Returns: Vec<(binary_path, binary_name, source, resolved_path)>
/// resolved_path is Some if the binary is a symlink pointing elsewhere
pub type BinaryScanResult = (String, String, String, Option<String>);

pub fn scan_all_binaries() -> Result<Vec<BinaryScanResult>> {
    let config = Config::load()?;
    let mut all_binaries = Vec::new();
    let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Get directories to scan from config
    let scan_dirs = config.get_scan_dirs();

    for dir in scan_dirs {
        // Expand ~ to home directory
        let dir_path = expand_tilde(&dir);

        if !dir_path.exists() || !dir_path.is_dir() {
            continue;
        }

        // Determine the source based on path (from config)
        let source = config.categorize_path(&dir_path.to_string_lossy());

        if let Ok(entries) = fs::read_dir(&dir_path) {
            for entry in entries.flatten() {
                let bin_path = entry.path();

                // Must be a file or symlink
                if !bin_path.is_file() && !bin_path.is_symlink() {
                    continue;
                }

                // Check if executable
                if !is_executable(&bin_path) {
                    continue;
                }

                let bin_path_str = bin_path.to_string_lossy().to_string();

                // Skip duplicates (same binary in multiple PATH entries)
                if seen_paths.contains(&bin_path_str) {
                    continue;
                }
                seen_paths.insert(bin_path_str.clone());

                // Get binary name
                let bin_name = bin_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();

                if bin_name.is_empty() || bin_name.starts_with('.') {
                    continue;
                }

                // Try to get package name (for homebrew, resolve symlink)
                let pkg_name = get_package_name(&bin_path, &bin_name);

                // If it's a symlink, resolve to get the real path
                // (eslogger reports resolved paths, so we need this mapping)
                let resolved = fs::canonicalize(&bin_path)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
                    .filter(|resolved| resolved != &bin_path_str);

                all_binaries.push((bin_path_str, pkg_name, source.clone(), resolved));
            }
        }
    }

    Ok(all_binaries)
}

/// Expand ~ to home directory
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}

/// Check if a file is executable
fn is_executable(path: &Path) -> bool {
    if let Ok(metadata) = fs::metadata(path) {
        let permissions = metadata.permissions();
        // Check if any execute bit is set
        permissions.mode() & 0o111 != 0
    } else {
        false
    }
}

/// Try to determine package name from binary path.
/// Checks Homebrew Cellar symlinks, then install root anchors, then falls back to binary name.
pub fn get_package_name(bin_path: &Path, default_name: &str) -> String {
    // For Homebrew, resolve symlink to get package name
    if let Ok(resolved) = fs::read_link(bin_path) {
        let resolved_str = resolved.to_string_lossy();

        // Look for Cellar/<package>/ pattern
        if let Some(cellar_idx) = resolved_str.find("Cellar/") {
            let after_cellar = &resolved_str[cellar_idx + 7..];
            if let Some(slash_idx) = after_cellar.find('/') {
                return after_cellar[..slash_idx].to_string();
            }
        }
    }

    // For downloaded software in well-known anchors (e.g. /opt/oss-cad-suite/bin/yosys),
    // use the install root directory name as the package name.
    let path_str = bin_path.to_string_lossy();
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    for anchor in crate::defaults::INSTALL_ROOT_ANCHORS {
        let expanded = anchor.replace('~', &home);
        if path_str.starts_with(&expanded) {
            let rest = &path_str[expanded.len()..];
            // Take the first component after the anchor as the package name
            // e.g. /opt/ + "oss-cad-suite/bin/yosys" → "oss-cad-suite"
            if let Some(slash_idx) = rest.find('/') {
                let root_name = &rest[..slash_idx];
                if !root_name.is_empty() {
                    return root_name.to_string();
                }
            }
        }
    }

    default_name.to_string()
}
