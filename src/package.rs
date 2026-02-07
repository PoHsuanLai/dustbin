#![allow(dead_code)]

use crate::config::Config;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Scan all directories in PATH and return all executable binaries
/// Returns: Vec<(binary_path, binary_name, source)>
pub fn scan_all_binaries() -> Result<Vec<(String, String, String)>> {
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

                all_binaries.push((bin_path_str, pkg_name, source.clone()));
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

/// Try to determine package name from binary path
fn get_package_name(bin_path: &Path, default_name: &str) -> String {
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

    default_name.to_string()
}

// ============ Legacy code below (kept for compatibility) ============

pub trait PackageManager {
    fn name(&self) -> &'static str;
    fn list_packages(&self) -> Result<Vec<Package>>;
    fn binary_to_package(&self, path: &Path) -> Option<String>;
    fn uninstall(&self, package: &str) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub manager: String,
    pub binaries: Vec<String>,
}

pub struct Homebrew {
    bin_to_package: HashMap<String, String>,
}

impl Homebrew {
    pub fn new() -> Result<Self> {
        Ok(Self {
            bin_to_package: HashMap::new(),
        })
    }
}

impl PackageManager for Homebrew {
    fn name(&self) -> &'static str {
        "homebrew"
    }

    fn list_packages(&self) -> Result<Vec<Package>> {
        Ok(vec![]) // Now handled by scan_all_binaries
    }

    fn binary_to_package(&self, path: &Path) -> Option<String> {
        self.bin_to_package.get(path.to_str()?).cloned()
    }

    fn uninstall(&self, package: &str) -> Result<()> {
        let status = Command::new("brew").args(["uninstall", package]).status()?;

        if status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Failed to uninstall {}", package))
        }
    }
}

pub struct Cargo;

impl Cargo {
    pub fn new() -> Self {
        Self
    }
}

impl PackageManager for Cargo {
    fn name(&self) -> &'static str {
        "cargo"
    }

    fn list_packages(&self) -> Result<Vec<Package>> {
        Ok(vec![]) // Now handled by scan_all_binaries
    }

    fn binary_to_package(&self, path: &Path) -> Option<String> {
        let cargo_bin = dirs::home_dir()?.join(".cargo/bin");
        if !path.starts_with(&cargo_bin) {
            return None;
        }
        path.file_name()?.to_str().map(String::from)
    }

    fn uninstall(&self, package: &str) -> Result<()> {
        let status = Command::new("cargo")
            .args(["uninstall", package])
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Failed to uninstall {}", package))
        }
    }
}
