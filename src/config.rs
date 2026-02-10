use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDef {
    /// Name of the source (e.g., "homebrew", "cargo")
    pub name: String,
    /// Path pattern to match (if path contains this string, it's this source)
    pub path: String,
    /// Uninstall command (e.g., "brew uninstall", "cargo uninstall")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uninstall_cmd: Option<String>,
    /// Command that lists installed packages (one per line to stdout)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_cmd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    /// Use $PATH to discover binaries (default: true)
    #[serde(default = "default_true")]
    pub path: bool,

    /// Additional directories to scan (beyond PATH)
    #[serde(default)]
    pub extra_dirs: Vec<String>,

    /// Directories to skip (even if in PATH)
    #[serde(default = "default_skip_dirs")]
    pub skip_dirs: Vec<String>,

    /// Binary prefixes to ignore when tracking
    #[serde(default = "default_skip_prefixes")]
    pub skip_prefixes: Vec<String>,

    /// Binaries to ignore in reports (patterns, e.g. "python*-config")
    #[serde(default)]
    pub ignore_binaries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Scanning configuration
    #[serde(default)]
    pub scan: ScanConfig,

    /// Source definitions for categorizing binaries
    #[serde(default = "default_sources")]
    pub sources: Vec<SourceDef>,
}

fn default_true() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn default_skip_dirs() -> Vec<String> {
    vec![
        "/usr/bin".to_string(),
        "/usr/sbin".to_string(),
        "/bin".to_string(),
        "/sbin".to_string(),
        "/System".to_string(),
        "/Library/Apple".to_string(),
    ]
}

#[cfg(target_os = "linux")]
fn default_skip_dirs() -> Vec<String> {
    vec![
        "/usr/bin".to_string(),
        "/usr/sbin".to_string(),
        "/bin".to_string(),
        "/sbin".to_string(),
    ]
}

#[cfg(target_os = "macos")]
fn default_skip_prefixes() -> Vec<String> {
    vec![
        "/usr/libexec/".to_string(),
        "/System/".to_string(),
        "/Library/Apple/".to_string(),
    ]
}

#[cfg(target_os = "linux")]
fn default_skip_prefixes() -> Vec<String> {
    vec!["/usr/libexec/".to_string(), "/usr/lib/".to_string()]
}

fn default_sources() -> Vec<SourceDef> {
    vec![]
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            path: true,
            extra_dirs: vec![],
            skip_dirs: default_skip_dirs(),
            skip_prefixes: default_skip_prefixes(),
            ignore_binaries: vec![],
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scan: ScanConfig::default(),
            sources: Self::default_sources_list(),
        }
    }
}

impl Config {
    /// Default sources list - used when creating new config file
    /// Scan system and return only sources that exist
    pub fn default_sources_list() -> Vec<SourceDef> {
        use crate::defaults::{EXTRA_PATH_PATTERNS, SOURCE_CANDIDATES};

        let home = dirs::home_dir().unwrap_or_default();
        let home_str = home.to_str().unwrap_or("");
        let mut sources = Vec::new();

        for candidate in SOURCE_CANDIDATES {
            for detect_path in candidate.detect_paths {
                let expanded = detect_path.replace('~', home_str);
                if std::path::Path::new(&expanded).exists() {
                    let pattern = expanded.replace(home_str, "~");
                    sources.push(SourceDef {
                        name: candidate.name.to_string(),
                        path: pattern,
                        uninstall_cmd: candidate.uninstall_cmd.map(|s| s.to_string()),
                        list_cmd: None,
                    });
                    break;
                }
            }
        }

        for &(name, pattern, requires) in EXTRA_PATH_PATTERNS {
            if sources.iter().any(|s| s.name == requires) {
                sources.push(SourceDef {
                    name: name.to_string(),
                    path: pattern.to_string(),
                    uninstall_cmd: None,
                    list_cmd: None,
                });
            }
        }

        sources
    }

    /// Get the uninstall command for a source from config.
    pub fn get_uninstall_cmd(&self, source_name: &str) -> Option<String> {
        self.sources
            .iter()
            .find(|s| s.name == source_name)
            .and_then(|s| s.uninstall_cmd.clone())
    }

    /// Get the list command for a source from config.
    pub fn get_list_cmd(&self, source_name: &str) -> Option<String> {
        self.sources
            .iter()
            .find(|s| s.name == source_name)
            .and_then(|s| s.list_cmd.clone())
    }

    /// Get all sources that have a list_cmd configured.
    pub fn get_sources_with_list_cmd(&self) -> Vec<&SourceDef> {
        self.sources
            .iter()
            .filter(|s| s.list_cmd.is_some())
            .collect()
    }

    /// Categorize a path to determine its source based on configured patterns
    pub fn categorize_path(&self, path: &str) -> String {
        for source in &self.sources {
            if path.contains(&source.path) {
                return source.name.clone();
            }
        }
        "other".to_string()
    }

    /// Load config from file, or create default if not exists
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            let config = Config::default();
            config.save()?;
            Ok(config)
        }
    }

    /// Save config to file
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        fs::write(&config_path, content)?;
        Ok(())
    }

    /// Get config file path
    pub fn config_path() -> Result<PathBuf> {
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;
        Ok(config_dir.join("dusty").join("config.toml"))
    }

    /// Get all directories to scan
    pub fn get_scan_dirs(&self) -> Vec<String> {
        let mut dirs = Vec::new();

        if self.scan.path
            && let Ok(path_var) = std::env::var("PATH")
        {
            for dir in path_var.split(':') {
                if !self.should_skip_dir(dir) {
                    dirs.push(dir.to_string());
                }
            }
        }

        for dir in &self.scan.extra_dirs {
            if !self.should_skip_dir(dir) {
                dirs.push(dir.clone());
            }
        }

        dirs
    }

    /// Check if a directory should be skipped
    pub fn should_skip_dir(&self, dir: &str) -> bool {
        self.scan.skip_dirs.iter().any(|skip| dir.starts_with(skip))
    }

    /// Check if a binary should be ignored in reports
    pub fn should_ignore_binary(&self, binary_name: &str) -> bool {
        for pattern in &self.scan.ignore_binaries {
            if pattern.contains('*') {
                let parts: Vec<&str> = pattern.split('*').collect();
                if parts.len() == 2 {
                    let (prefix, suffix) = (parts[0], parts[1]);
                    if binary_name.starts_with(prefix) && binary_name.ends_with(suffix) {
                        return true;
                    }
                }
            } else if binary_name == pattern {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_ignore_binary_exact() {
        let mut config = Config::default();
        config.scan.ignore_binaries = vec!["python3-config".to_string()];

        assert!(config.should_ignore_binary("python3-config"));
        assert!(!config.should_ignore_binary("python3"));
        assert!(!config.should_ignore_binary("python3-config-extra"));
    }

    #[test]
    fn test_should_ignore_binary_glob() {
        let mut config = Config::default();
        config.scan.ignore_binaries = vec!["python*-config".to_string()];

        assert!(config.should_ignore_binary("python3-config"));
        assert!(config.should_ignore_binary("python-config"));
        assert!(config.should_ignore_binary("python3.11-config"));
        assert!(!config.should_ignore_binary("python3"));
    }

    #[test]
    fn test_categorize_path() {
        let mut config = Config::default();
        config.sources = vec![
            SourceDef {
                name: "homebrew".to_string(),
                path: "/opt/homebrew".to_string(),
                uninstall_cmd: None,
                list_cmd: None,
            },
            SourceDef {
                name: "cargo".to_string(),
                path: ".cargo/bin".to_string(),
                uninstall_cmd: None,
                list_cmd: None,
            },
        ];

        assert_eq!(config.categorize_path("/opt/homebrew/bin/git"), "homebrew");
        assert_eq!(
            config.categorize_path("/Users/test/.cargo/bin/rustc"),
            "cargo"
        );
        assert_eq!(config.categorize_path("/usr/bin/ls"), "other");
    }

    #[test]
    fn test_should_skip_dir() {
        let config = Config::default();

        assert!(config.should_skip_dir("/usr/bin"));
        assert!(config.should_skip_dir("/bin"));
        assert!(!config.should_skip_dir("/opt/homebrew/bin"));
    }
}
