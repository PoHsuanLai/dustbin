use console::style;
use std::process::Command;

use crate::defaults;

/// Animated status line on stderr (hides cursor, overwrites with \r).
pub struct Spinner {
    term: console::Term,
    tick: usize,
}

impl Spinner {
    pub fn new() -> Self {
        let term = console::Term::stderr();
        let _ = term.hide_cursor();
        Self { term, tick: 0 }
    }

    pub fn update(&mut self, label: &str, current: usize, total: usize) {
        let dots = [".", "..", "..."];
        let dot = dots[self.tick % dots.len()];
        let msg = format!(
            "  {} {}/{}{}",
            style(label).cyan(),
            style(current).bold(),
            style(total).bold(),
            dot,
        );
        let _ = self.term.write_str(&format!("\r{:<70}", msg));
        self.tick += 1;
    }

    pub fn message(&self, label: &str) {
        let _ = self.term.write_str(&format!(
            "\r{:<70}",
            format!("  {} ...", style(label).cyan())
        ));
    }

    pub fn finish(self) {
        let _ = self.term.show_cursor();
        let _ = self.term.write_str(&format!("\r{:<70}\r", ""));
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

pub fn shorten_path(path: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.display().to_string())
        .unwrap_or_default();

    for &(prefix, replacement) in defaults::PATH_SHORTHANDS {
        let expanded = prefix.replace('~', &home);
        if path.starts_with(&expanded) {
            return format!("{}{}", replacement, &path[expanded.len()..]);
        }
    }
    path.to_string()
}

pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("...{}", &s[s.len() - (max_len - 3)..])
    }
}

pub fn print_with_pager(content: &str) {
    use std::io::Write;

    if !console::Term::stdout().is_term() {
        print!("{}", content);
        return;
    }

    let pager_cmd = std::env::var("PAGER").unwrap_or_else(|_| defaults::DEFAULT_PAGER.to_string());
    let (program, base_args): (&str, Vec<&str>) = if pager_cmd.contains("less") {
        (pager_cmd.as_str(), vec![defaults::PAGER_COLOR_FLAG])
    } else {
        (pager_cmd.as_str(), vec![])
    };

    match Command::new(program)
        .args(&base_args)
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(content.as_bytes());
            }
            let _ = child.wait();
        }
        Err(_) => {
            print!("{}", content);
        }
    }
}

/// Returns how many content rows fit in the terminal, reserving `overhead` lines
/// for headers, summaries, and padding. Returns 0 if detection fails (show all).
pub fn terminal_fit(overhead: usize) -> usize {
    console::Term::stdout()
        .size_checked()
        .map(|(rows, _)| (rows as usize).saturating_sub(overhead))
        .unwrap_or(0)
}
