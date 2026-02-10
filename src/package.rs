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

                // Refine source: npm globals under homebrew's node should be "npm"
                let refined_source = if let Ok(link_target) = fs::read_link(&bin_path) {
                    let target_str = link_target.to_string_lossy();
                    if target_str.contains("node_modules/") {
                        "npm".to_string()
                    } else if target_str.contains("Caskroom/") {
                        "cask".to_string()
                    } else {
                        source.clone()
                    }
                } else {
                    source.clone()
                };

                // If it's a symlink, resolve to get the real path
                // (eslogger reports resolved paths, so we need this mapping)
                let resolved = fs::canonicalize(&bin_path)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
                    .filter(|resolved| resolved != &bin_path_str);

                all_binaries.push((bin_path_str, pkg_name, refined_source, resolved));
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

/// Extract package name from a Cellar path (e.g. ".../Cellar/python@3.13/3.13.11_1/..." → "python@3.13")
fn extract_cellar_package(path: &str) -> Option<String> {
    let after_cellar = path.split("Cellar/").nth(1)?;
    let pkg = after_cellar.split('/').next()?;
    if pkg.is_empty() {
        return None;
    }
    Some(pkg.to_string())
}

/// Try to determine package name from binary path.
/// Checks Homebrew Cellar symlinks, then install root anchors, then falls back to binary name.
pub fn get_package_name(bin_path: &Path, default_name: &str) -> String {
    // For Homebrew, resolve symlink to get package name
    if let Ok(resolved) = fs::read_link(bin_path) {
        let resolved_str = resolved.to_string_lossy();

        if let Some(pkg) = extract_cellar_package(&resolved_str) {
            return pkg;
        }
    }

    // Also check the path itself — daemon-recorded paths are already resolved
    let path_str = bin_path.to_string_lossy();
    if let Some(pkg) = extract_cellar_package(&path_str) {
        return pkg;
    }

    // For downloaded software in well-known anchors (e.g. /opt/oss-cad-suite/bin/yosys),
    // use the install root directory name as the package name.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_cellar_package() {
        assert_eq!(
            extract_cellar_package("/opt/homebrew/Cellar/python@3.13/3.13.11_1/libexec/bin/pip"),
            Some("python@3.13".to_string())
        );
        assert_eq!(
            extract_cellar_package("/opt/homebrew/Cellar/git/2.44.0/bin/git"),
            Some("git".to_string())
        );
        assert_eq!(extract_cellar_package("/opt/homebrew/bin/git"), None);
        assert_eq!(extract_cellar_package("/usr/bin/ls"), None);
        assert_eq!(extract_cellar_package("Cellar/"), None);
        assert_eq!(extract_cellar_package(""), None);
    }

    #[test]
    fn test_get_package_name_cellar_path() {
        // Non-symlink Cellar path (daemon-recorded)
        let path = Path::new("/opt/homebrew/Cellar/python@3.13/3.13.11_1/libexec/bin/pip");
        assert_eq!(get_package_name(path, "pip"), "python@3.13");
    }

    #[test]
    fn test_get_package_name_install_root() {
        let path = Path::new("/opt/oss-cad-suite/bin/yosys");
        assert_eq!(get_package_name(path, "yosys"), "oss-cad-suite");
    }

    #[test]
    fn test_get_package_name_fallback() {
        let path = Path::new("/some/random/path/mytool");
        assert_eq!(get_package_name(path, "mytool"), "mytool");
    }

    #[test]
    fn test_expand_tilde() {
        assert_eq!(expand_tilde("/usr/bin"), PathBuf::from("/usr/bin"));
        // ~ should expand to something (home dir)
        let expanded = expand_tilde("~/test");
        assert!(expanded.to_string_lossy().ends_with("/test"));
        assert!(!expanded.to_string_lossy().starts_with("~"));
    }
}
