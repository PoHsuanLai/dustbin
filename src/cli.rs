use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dusty")]
#[command(author, version, about = "Find your dusty binaries", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the tracking daemon
    Start,

    /// Stop the tracking daemon
    Stop,

    /// Show tracking status and statistics
    Status {
        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,
    },

    /// Show summary statistics
    Stats {
        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,
    },

    /// Show package usage report
    Report {
        /// Show only unused packages (count = 0)
        #[arg(long)]
        dust: bool,

        /// Show packages with fewer than N uses
        #[arg(long, value_name = "N")]
        low: Option<u32>,

        /// Show packages not used in N days (e.g., --stale 30)
        #[arg(long, value_name = "DAYS")]
        stale: Option<u32>,

        /// Filter by source (homebrew, cargo, npm, local, etc.)
        #[arg(long, short)]
        source: Option<String>,

        /// Show all items (default: fits terminal height)
        #[arg(long, short)]
        all: bool,

        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,

        /// Output uninstall commands for shell
        #[arg(long)]
        export: bool,
    },

    /// Interactively remove unused packages
    Clean {
        /// Show what would be removed without removing
        #[arg(long)]
        dry_run: bool,

        /// Include packages not used in N days
        #[arg(long, value_name = "DAYS")]
        stale: Option<u32>,

        /// Filter by source (homebrew, cargo, npm, etc.)
        #[arg(long, short)]
        source: Option<String>,

        /// Permanently delete instead of moving to trash
        #[arg(long)]
        no_trash: bool,
    },

    /// Show or edit configuration
    Config {
        /// Open config file in editor
        #[arg(long)]
        edit: bool,
    },

    /// Find duplicate binaries installed from different sources
    Dupes {
        /// Show details for a specific binary (e.g., dusty dupes rustc)
        #[arg(value_name = "NAME")]
        name: Option<String>,

        /// Show expanded details for all duplicates
        #[arg(long, short)]
        all: bool,

        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,
    },

    /// Analyze dynamic library dependencies
    Deps {
        /// Show only orphan packages (used exclusively by dusty binaries)
        #[arg(long)]
        orphans: bool,

        /// Show dependencies for a specific binary
        #[arg(long, value_name = "BINARY")]
        binary: Option<String>,

        /// Force re-analysis (ignore cache)
        #[arg(long)]
        refresh: bool,

        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,
    },

    /// Explain why a binary is installed
    Why {
        /// Binary name to look up (e.g., "yosys")
        name: String,

        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,
    },

    /// Show disk space per package
    Size {
        /// Show only unused (dusty) packages
        #[arg(long)]
        dust: bool,

        /// Filter by source (homebrew, cargo, npm, etc.)
        #[arg(long, short)]
        source: Option<String>,

        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,
    },

    /// List trashed packages
    Trash {
        /// Permanently delete a specific trashed package
        #[arg(long, value_name = "NAME")]
        drop: Option<String>,

        /// Permanently delete all trashed items
        #[arg(long)]
        empty: bool,

        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,
    },

    /// List packages from external package managers (R, pip, etc.)
    Inventory {
        /// Filter by source name
        #[arg(long, short)]
        source: Option<String>,

        /// Show full package lists
        #[arg(long, short)]
        all: bool,

        /// Output as JSON (for scripting/nushell)
        #[arg(long)]
        json: bool,
    },

    /// Restore a trashed package
    Restore {
        /// Package name to restore
        name: String,
    },

    /// Show daemon logs
    Log {
        /// Number of lines to show (default: 50)
        #[arg(long, short = 'n', default_value = "50")]
        lines: usize,

        /// Follow log output in real time
        #[arg(long, short)]
        follow: bool,
    },

    /// Generate shell completions
    #[command(hide = true)]
    Completions {
        /// Shell to generate completions for (bash, zsh, fish)
        #[arg(long, value_enum)]
        shell: clap_complete::Shell,
    },

    /// Run the daemon (internal use)
    #[command(hide = true)]
    Daemon,
}
