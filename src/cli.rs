use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dustbin")]
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

        /// Number of items to show (default: 20, use 0 for all)
        #[arg(long, short, default_value = "20")]
        limit: usize,

        /// Show all items (same as --limit 0)
        #[arg(long)]
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
    },

    /// Show or edit configuration
    Config {
        /// Open config file in editor
        #[arg(long)]
        edit: bool,
    },

    /// Run the daemon (internal use)
    #[command(hide = true)]
    Daemon,
}
