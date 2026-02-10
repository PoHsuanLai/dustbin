//! Centralized default values for source definitions and path display mappings.
//!
//! Edit these tables to add new package managers or change display shorthands.
//! Detection paths use `~` as a placeholder for the user's home directory.

/// A source candidate for auto-detection during config generation.
pub struct SourceCandidate {
    /// Source name (e.g., "homebrew", "cargo")
    pub name: &'static str,
    /// Paths to check for existence (first match wins). `~` expands to $HOME.
    pub detect_paths: &'static [&'static str],
    /// Uninstall command prefix, if the source has a package manager.
    pub uninstall_cmd: Option<&'static str>,
}

/// All known source candidates, checked in order during config generation.
/// Only sources whose detection paths exist on the system are included in config.
pub const SOURCE_CANDIDATES: &[SourceCandidate] = &[
    // macOS
    SourceCandidate {
        name: "homebrew",
        detect_paths: &["/opt/homebrew", "/usr/local/Homebrew"],
        uninstall_cmd: Some("brew uninstall"),
    },
    // Linux package managers
    SourceCandidate {
        name: "apt",
        detect_paths: &["/var/lib/dpkg"],
        uninstall_cmd: Some("sudo apt remove -y"),
    },
    SourceCandidate {
        name: "dnf",
        detect_paths: &["/var/lib/dnf"],
        uninstall_cmd: Some("sudo dnf remove -y"),
    },
    SourceCandidate {
        name: "pacman",
        detect_paths: &["/var/lib/pacman"],
        uninstall_cmd: Some("sudo pacman -R --noconfirm"),
    },
    SourceCandidate {
        name: "zypper",
        detect_paths: &["/var/lib/zypp"],
        uninstall_cmd: Some("sudo zypper remove -y"),
    },
    SourceCandidate {
        name: "apk",
        detect_paths: &["/etc/apk"],
        uninstall_cmd: Some("sudo apk del"),
    },
    // Universal formats
    SourceCandidate {
        name: "snap",
        detect_paths: &["/snap/bin"],
        uninstall_cmd: Some("sudo snap remove"),
    },
    SourceCandidate {
        name: "flatpak",
        detect_paths: &["/var/lib/flatpak"],
        uninstall_cmd: Some("flatpak uninstall"),
    },
    // Language package managers
    SourceCandidate {
        name: "cargo",
        detect_paths: &["~/.cargo/bin"],
        uninstall_cmd: Some("cargo uninstall"),
    },
    SourceCandidate {
        name: "npm",
        detect_paths: &["~/.npm", "~/.nvm"],
        uninstall_cmd: Some("npm uninstall -g"),
    },
    SourceCandidate {
        name: "go",
        detect_paths: &["~/go/bin"],
        uninstall_cmd: None,
    },
    SourceCandidate {
        name: "pip",
        detect_paths: &["~/.local/bin"],
        uninstall_cmd: Some("pip uninstall -y"),
    },
    SourceCandidate {
        name: "pyenv",
        detect_paths: &["~/.pyenv"],
        uninstall_cmd: None,
    },
    SourceCandidate {
        name: "nix",
        detect_paths: &["~/.nix-profile"],
        uninstall_cmd: Some("nix-env --uninstall"),
    },
    SourceCandidate {
        name: "bun",
        detect_paths: &["~/.bun"],
        uninstall_cmd: Some("bun remove -g"),
    },
    SourceCandidate {
        name: "deno",
        detect_paths: &["~/.deno"],
        uninstall_cmd: None,
    },
    SourceCandidate {
        name: "linuxbrew",
        detect_paths: &["~/.linuxbrew"],
        uninstall_cmd: Some("brew uninstall"),
    },
    // General
    SourceCandidate {
        name: "opt",
        detect_paths: &["/opt"],
        uninstall_cmd: None,
    },
    SourceCandidate {
        name: "local",
        detect_paths: &["/usr/local/bin"],
        uninstall_cmd: None,
    },
];

/// Extra path patterns added without existence checks (e.g., Cellar matching).
/// Format: (source_name, path_pattern, requires_source) — only added if
/// `requires_source` is already present in the detected sources.
#[cfg(target_os = "macos")]
pub const EXTRA_PATH_PATTERNS: &[(&str, &str, &str)] = &[("homebrew", "Cellar", "homebrew")];

#[cfg(target_os = "linux")]
pub const EXTRA_PATH_PATTERNS: &[(&str, &str, &str)] = &[];

/// Path prefix replacements for display shortening, applied in order.
/// Format: (prefix_to_match, replacement)
/// `~` in the prefix is expanded to $HOME at runtime.
pub const PATH_SHORTHANDS: &[(&str, &str)] = &[
    ("/opt/homebrew/bin/", "brew:"),
    ("/opt/homebrew/Cellar/", "brew:"),
    ("/usr/local/bin/", "/usr/local/"),
    ("/usr/bin/", "/usr/"),
    ("~/.cargo/bin/", "cargo:"),
    ("~/", "~/"),
];

/// Shell execution
pub const SHELL: &str = "sh";
pub const SHELL_CMD_FLAG: &str = "-c";

/// Privilege escalation and file removal
pub const SUDO: &str = "sudo";
pub const RM: &str = "rm";
pub const RM_RECURSIVE_FLAGS: &[&str] = &["-rf"];

/// Install root detection anchors (~ expanded to $HOME at runtime)
pub const INSTALL_ROOT_ANCHORS: &[&str] = &["/opt/", "/usr/local/", "~/"];

/// Trash directory name (under data_local_dir/dusty/)
pub const TRASH_DIR: &str = "trash";

/// Derive an install command from an uninstall command.
/// Returns None if no mapping is known.
pub fn install_cmd_from_uninstall(uninstall_cmd: &str) -> Option<String> {
    let mappings: &[(&str, &str)] = &[
        ("brew uninstall", "brew install"),
        ("sudo apt remove -y", "sudo apt install -y"),
        ("sudo dnf remove -y", "sudo dnf install -y"),
        ("sudo pacman -R --noconfirm", "sudo pacman -S --noconfirm"),
        ("sudo zypper remove -y", "sudo zypper install -y"),
        ("sudo apk del", "sudo apk add"),
        ("sudo snap remove", "sudo snap install"),
        ("flatpak uninstall", "flatpak install"),
        ("cargo uninstall", "cargo install"),
        ("npm uninstall -g", "npm install -g"),
        ("pip uninstall -y", "pip install"),
        ("nix-env --uninstall", "nix-env --install"),
        ("bun remove -g", "bun add -g"),
    ];

    for (uninstall, install) in mappings {
        if uninstall_cmd == *uninstall {
            return Some(install.to_string());
        }
    }
    None
}

/// Autoremove commands to clean up orphaned dependencies, keyed by source name.
pub fn autoremove_hint(source: &str) -> Option<&'static str> {
    match source {
        "homebrew" | "linuxbrew" => Some("brew autoremove"),
        "apt" => Some("sudo apt autoremove"),
        "dnf" => Some("sudo dnf autoremove"),
        "pacman" => Some("sudo pacman -Qdtq | sudo pacman -Rns -"),
        "zypper" => Some("sudo zypper packages --unneeded"),
        "flatpak" => Some("flatpak uninstall --unused"),
        _ => None,
    }
}

/// Editor and pager defaults
pub const DEFAULT_EDITOR: &str = "vim";
pub const DEFAULT_PAGER: &str = "less";
pub const PAGER_COLOR_FLAG: &str = "-R";
