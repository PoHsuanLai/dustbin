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
        expand: bool,

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

    /// Run the daemon (internal use)
    #[command(hide = true)]
    Daemon,
}
