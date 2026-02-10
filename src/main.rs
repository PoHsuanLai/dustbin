mod cli;
mod commands;
mod config;
mod defaults;
mod deps;
mod package;
mod platform;
mod storage;
mod ui;
mod utils;

use clap::Parser;
use cli::{Cli, Commands};
use console::style;

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Start => commands::cmd_start(),
        Commands::Stop => commands::cmd_stop(),
        Commands::Status { json } => commands::cmd_status(json),
        Commands::Stats { json } => commands::cmd_stats(json),
        Commands::Report {
            dust,
            low,
            stale,
            source,
            all,
            json,
            export,
        } => commands::cmd_report(dust, low, stale, source, all, json, export),
        Commands::Clean {
            dry_run,
            stale,
            source,
            no_trash,
        } => commands::cmd_clean(dry_run, stale, source, no_trash),
        Commands::Config { edit } => commands::cmd_config(edit),
        Commands::Dupes { name, all, json } => commands::cmd_dupes(name, all, json),
        Commands::Trash { drop, empty, json } => commands::cmd_trash(drop, empty, json),
        Commands::Restore { name } => commands::cmd_restore(name),
        Commands::Inventory { source, all, json } => commands::cmd_inventory(source, all, json),
        Commands::Deps {
            orphans,
            binary,
            refresh,
            json,
        } => commands::cmd_deps(orphans, binary, refresh, json),
        Commands::Why { name, json } => commands::cmd_why(name, json),
        Commands::Size { dust, source, json } => commands::cmd_size(dust, source, json),
        Commands::Log { lines, follow } => commands::cmd_log(lines, follow),
        Commands::Completions { shell } => commands::cmd_completions(shell),
        Commands::Daemon => commands::cmd_daemon(),
    };

    if let Err(e) = result {
        eprintln!("{} {}", style("error:").red().bold(), e);
        std::process::exit(1);
    }
}
