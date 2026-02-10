use anyhow::{Context, Result};
use console::style;
use std::process::Command;

use crate::defaults;

pub fn cmd_config(edit: bool) -> Result<()> {
    use crate::config::Config;

    // Load config (auto-creates if not exists)
    let _config = Config::load()?;
    let path = Config::config_path()?;

    if edit {
        let editor =
            std::env::var("EDITOR").unwrap_or_else(|_| defaults::DEFAULT_EDITOR.to_string());
        Command::new(&editor)
            .arg(&path)
            .status()
            .context(format!("Failed to open editor: {}", editor))?;
        return Ok(());
    }

    // Default: show config
    println!();
    println!("  {} {}", style("Config:").bold(), path.display());
    println!();

    let content = std::fs::read_to_string(&path)?;
    for line in content.lines() {
        print!("    ");
        print_toml_line(line);
        println!();
    }
    println!();

    Ok(())
}

fn print_toml_line(line: &str) {
    let trimmed = line.trim();

    // Comment
    if trimmed.starts_with('#') {
        print!("{}", style(line).dim());
        return;
    }

    // Section header [[foo]] or [foo]
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        print!("{}", style(line).cyan().bold());
        return;
    }

    // Key = value
    if let Some(eq_pos) = line.find('=') {
        let (key, rest) = line.split_at(eq_pos);
        let value = &rest[1..]; // skip the '='
        print!("{}{}", style(key).green(), style("=").dim());
        print_toml_value(value);
        return;
    }

    // Array items (strings in quotes)
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        print!("{}", style(line).yellow());
        return;
    }

    // Array brackets
    if trimmed == "[" || trimmed == "]" || trimmed == "]," {
        print!("{}", style(line).dim());
        return;
    }

    // Fallback
    print!("{}", line);
}

fn print_toml_value(value: &str) {
    let trimmed = value.trim();

    // Boolean or number
    if trimmed == "true" || trimmed == "false" || trimmed.parse::<f64>().is_ok() {
        print!("{}", style(value).cyan());
    }
    // Empty array or array start
    else if trimmed == "[]" || trimmed == "[" {
        print!("{}", style(value).dim());
    }
    // Strings and other values
    else {
        print!("{}", style(value).yellow());
    }
}
