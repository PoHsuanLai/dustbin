//! macOS-specific implementation using eslogger and launchd

use super::{DaemonManager, ProcessMonitor};
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
    const LABEL: &'static str = "com.dustbin.daemon";

    fn plist_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("Library/LaunchAgents/com.dustbin.daemon.plist")
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
    <string>/tmp/dustbin.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/dustbin.err</string>
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
