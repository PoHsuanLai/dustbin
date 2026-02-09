//! macOS-specific implementation using eslogger and launchd

use super::{
    DaemonManager, DylibAnalysis, DylibAnalyzer, DylibDep, LibPackageInfo, ProcessMonitor,
};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// Event from eslogger exec - we extract the target executable path
#[derive(Debug, Deserialize)]
struct EsloggerEvent {
    event: Event,
}

#[derive(Debug, Deserialize)]
struct Event {
    exec: Option<ExecInfo>,
}

#[derive(Debug, Deserialize)]
struct ExecInfo {
    target: TargetProcess,
}

#[derive(Debug, Deserialize)]
struct TargetProcess {
    executable: Executable,
}

#[derive(Debug, Deserialize)]
struct Executable {
    path: String,
}

impl EsloggerEvent {
    fn executable_path(&self) -> Option<&str> {
        self.event
            .exec
            .as_ref()
            .map(|e| e.target.executable.path.as_str())
    }
}

/// macOS process monitor using eslogger
pub struct Monitor {
    child: Option<Child>,
}

impl ProcessMonitor for Monitor {
    fn new() -> Self {
        Self { child: None }
    }

    fn start(&mut self) -> Result<Receiver<String>> {
        let mut child = Command::new("sudo")
            .args(["eslogger", "exec"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn eslogger. Make sure you have Full Disk Access enabled.")?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;

        self.child = Some(child);

        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(event) = serde_json::from_str::<EsloggerEvent>(&line)
                    && let Some(path) = event.executable_path()
                {
                    let _ = tx.send(path.to_string());
                }
            }
        });

        Ok(rx)
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            child.kill().ok();
            child.wait().ok();
        }
        Ok(())
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop().ok();
    }
}

/// macOS daemon manager using launchd
pub struct Daemon;

impl Daemon {
    const LABEL: &'static str = "com.dusty.daemon";

    fn plist_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("Library/LaunchAgents/com.dusty.daemon.plist")
    }

    fn generate_plist(exe_path: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/dusty.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/dusty.err</string>
</dict>
</plist>
"#,
            Self::LABEL,
            exe_path
        )
    }
}

impl DaemonManager for Daemon {
    fn check_available() -> bool {
        Command::new("which")
            .arg("eslogger")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn is_daemon_running() -> bool {
        Command::new("launchctl")
            .args(["list", Self::LABEL])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn start_daemon(exe_path: &str) -> Result<()> {
        let plist_path = Self::plist_path();
        let plist_content = Self::generate_plist(exe_path);

        if let Some(parent) = plist_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&plist_path, plist_content)?;

        let status = Command::new("launchctl")
            .args(["load", "-w", plist_path.to_str().unwrap()])
            .status()
            .context("Failed to load launchd job")?;

        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("Failed to start daemon via launchctl")
        }
    }

    fn stop_daemon() -> Result<()> {
        let plist_path = Self::plist_path();

        if !plist_path.exists() {
            return Ok(());
        }

        let status = Command::new("launchctl")
            .args(["unload", plist_path.to_str().unwrap()])
            .status()
            .context("Failed to unload launchd job")?;

        if status.success() {
            fs::remove_file(&plist_path).ok();
            Ok(())
        } else {
            anyhow::bail!("Failed to stop daemon")
        }
    }

    fn setup_instructions() -> &'static str {
        "eslogger requires Full Disk Access.\n\
         Go to System Settings → Privacy & Security → Full Disk Access\n\
         and add your terminal app (Terminal, iTerm, Warp, etc.)"
    }
}

/// macOS dynamic library analyzer using otool
pub struct Analyzer;

impl DylibAnalyzer for Analyzer {
    fn analyze_binary(binary_path: &str) -> Result<DylibAnalysis> {
        let output = Command::new("otool").args(["-L", binary_path]).output();

        let output = match output {
            Ok(o) => o,
            Err(_) => {
                return Ok(DylibAnalysis { libs: vec![] });
            }
        };

        if !output.status.success() {
            return Ok(DylibAnalysis { libs: vec![] });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let libs = stdout
            .lines()
            .skip(1)
            .filter_map(|line| {
                let trimmed = line.trim();
                let path = trimmed.split(" (compatibility").next()?.trim();
                if path.is_empty() {
                    return None;
                }
                // Skip system libraries
                if path.starts_with("/usr/lib/")
                    || path.starts_with("/System/")
                    || path.starts_with("/Library/Apple/")
                {
                    return None;
                }
                // Skip @rpath entries for v1
                if path.starts_with('@') {
                    return None;
                }
                Some(DylibDep {
                    path: path.to_string(),
                })
            })
            .collect();

        Ok(DylibAnalysis { libs })
    }

    fn resolve_lib_packages(lib_paths: &[String]) -> Result<Vec<LibPackageInfo>> {
        let mut results = Vec::new();
        for lib_path in lib_paths {
            if let Some(pkg) = extract_homebrew_package(lib_path) {
                results.push(LibPackageInfo {
                    lib_path: lib_path.clone(),
                    manager: "homebrew".to_string(),
                    package_name: pkg,
                });
            }
        }
        Ok(results)
    }

    fn get_package_size(_manager: &str, package_name: &str) -> Result<Option<u64>> {
        // Try common Homebrew Cellar locations
        for prefix in &["/opt/homebrew/Cellar", "/usr/local/Cellar"] {
            let cellar_path = format!("{}/{}", prefix, package_name);
            let output = Command::new("du").args(["-sk", &cellar_path]).output();
            if let Ok(output) = output
                && output.status.success()
            {
                let line = String::from_utf8_lossy(&output.stdout);
                if let Some(size_str) = line.split_whitespace().next()
                    && let Ok(kb) = size_str.parse::<u64>()
                {
                    return Ok(Some(kb * 1024));
                }
            }
        }
        Ok(None)
    }
}

/// Extract Homebrew package name from a library path
fn extract_homebrew_package(path: &str) -> Option<String> {
    for prefix in &[
        "/opt/homebrew/opt/",
        "/opt/homebrew/Cellar/",
        "/usr/local/opt/",
        "/usr/local/Cellar/",
    ] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return rest.split('/').next().map(|s| s.to_string());
        }
    }
    None
}
