//! Linux-specific implementation using fanotify and systemd

#[path = "linux_distro.rs"]
mod linux_distro;
pub use linux_distro::{InitSystem, LinuxInfo, PackageManager};

use super::{DaemonManager, ProcessMonitor};
use anyhow::{Context, Result};
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// Linux process monitor using fanotify
///
/// fanotify with FAN_OPEN_EXEC can monitor all exec events system-wide.
/// Requires CAP_SYS_ADMIN capability or root.
pub struct Monitor {
    child: Option<Child>,
}

impl ProcessMonitor for Monitor {
    fn new() -> Self {
        Self { child: None }
    }

    fn start(&mut self) -> Result<Receiver<String>> {
        // Use fatrace to monitor exec events
        // fatrace is a simple CLI wrapper around fanotify
        let mut child = Command::new("sudo")
            .args(["fatrace", "-f", "O", "-t"]) // -f O = open events, -t = timestamp
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn fatrace. Install with: sudo apt install fatrace")?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;

        self.child = Some(child);

        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                // fatrace output format: "timestamp process(pid): O filename"
                // We want to extract the filename from exec events
                if let Some(path) = parse_fatrace_line(&line) {
                    let _ = tx.send(path);
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

/// Parse fatrace output line to extract executable path
fn parse_fatrace_line(line: &str) -> Option<String> {
    // Format: "timestamp process(pid): O /path/to/file"
    // We want files that are being executed (typically in bin directories)
    let parts: Vec<&str> = line.splitn(4, ' ').collect();
    if parts.len() >= 4 {
        let path = parts[3].trim();
        // Filter to only track binaries in common locations
        if is_binary_path(path) {
            return Some(path.to_string());
        }
    }
    None
}

/// Check if a path looks like an executable binary
fn is_binary_path(path: &str) -> bool {
    path.contains("/bin/")
        || path.contains("/.cargo/bin/")
        || path.contains("/.local/bin/")
        || path.contains("/go/bin/")
}

/// Linux daemon manager - supports systemd, OpenRC, and runit
pub struct Daemon;

impl Daemon {
    const SERVICE_NAME: &'static str = "dustbin";

    fn systemd_service_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("systemd/user/dustbin.service")
    }

    fn generate_systemd_service(exe_path: &str) -> String {
        format!(
            r#"[Unit]
Description=Dustbin - Track binary usage
After=default.target

[Service]
Type=simple
ExecStart={} daemon
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
"#,
            exe_path
        )
    }

    fn openrc_service_path() -> PathBuf {
        PathBuf::from("/etc/init.d/dustbin")
    }

    fn generate_openrc_service(exe_path: &str) -> String {
        format!(
            r#"#!/sbin/openrc-run

name="dustbin"
description="Dustbin - Track binary usage"
command="{}"
command_args="daemon"
command_background=true
pidfile="/run/${{RC_SVCNAME}}.pid"

depend() {{
    need localmount
    after bootmisc
}}
"#,
            exe_path
        )
    }

    fn runit_service_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(".local/sv/dustbin")
    }

    fn generate_runit_run(exe_path: &str) -> String {
        format!(
            r#"#!/bin/sh
exec {} daemon
"#,
            exe_path
        )
    }
}

impl Daemon {
    /// Try to install fatrace automatically using detected package manager
    fn install_fatrace() -> Result<()> {
        let info = LinuxInfo::detect();

        let install_cmd = info.fatrace_install_cmd().ok_or_else(|| {
            anyhow::anyhow!(
                "fatrace is not available for {:?}. You may need to build from source.",
                info.distro
            )
        })?;

        eprintln!("Installing fatrace via {:?}...", info.package_manager);

        let status = Command::new(install_cmd[0])
            .args(&install_cmd[1..])
            .status()
            .context("Failed to run package manager")?;

        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("Package manager failed to install fatrace")
        }
    }

    fn is_fatrace_available() -> bool {
        Command::new("which")
            .arg("fatrace")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl DaemonManager for Daemon {
    fn check_available() -> bool {
        if Self::is_fatrace_available() {
            return true;
        }

        // Try to auto-install
        eprintln!("fatrace not found. Attempting to install...");
        if Self::install_fatrace().is_ok() && Self::is_fatrace_available() {
            eprintln!("fatrace installed successfully.");
            return true;
        }

        false
    }

    fn is_daemon_running() -> bool {
        let info = LinuxInfo::detect();

        match info.init_system {
            InitSystem::Systemd => Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", Self::SERVICE_NAME])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            InitSystem::OpenRC => Command::new("rc-service")
                .args([Self::SERVICE_NAME, "status"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            InitSystem::Runit => Command::new("sv")
                .args(["status", Self::SERVICE_NAME])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            _ => false,
        }
    }

    fn start_daemon(exe_path: &str) -> Result<()> {
        let info = LinuxInfo::detect();

        match info.init_system {
            InitSystem::Systemd => {
                let service_path = Self::systemd_service_path();
                let service_content = Self::generate_systemd_service(exe_path);

                if let Some(parent) = service_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&service_path, service_content)?;

                Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status()
                    .context("Failed to reload systemd")?;

                let status = Command::new("systemctl")
                    .args(["--user", "enable", "--now", Self::SERVICE_NAME])
                    .status()
                    .context("Failed to enable/start service")?;

                if status.success() {
                    Ok(())
                } else {
                    anyhow::bail!("Failed to start daemon via systemd")
                }
            }
            InitSystem::OpenRC => {
                let service_path = Self::openrc_service_path();
                let service_content = Self::generate_openrc_service(exe_path);

                // OpenRC requires root to install services
                fs::write(&service_path, &service_content).or_else(|_| {
                    // Try with sudo
                    let tmp = "/tmp/dustbin-openrc";
                    fs::write(tmp, &service_content)?;
                    Command::new("sudo")
                        .args(["mv", tmp, service_path.to_str().unwrap()])
                        .status()?;
                    Command::new("sudo")
                        .args(["chmod", "+x", service_path.to_str().unwrap()])
                        .status()?;
                    Ok::<(), anyhow::Error>(())
                })?;

                let status = Command::new("sudo")
                    .args(["rc-service", Self::SERVICE_NAME, "start"])
                    .status()
                    .context("Failed to start OpenRC service")?;

                if status.success() {
                    Command::new("sudo")
                        .args(["rc-update", "add", Self::SERVICE_NAME, "default"])
                        .status()
                        .ok();
                    Ok(())
                } else {
                    anyhow::bail!("Failed to start daemon via OpenRC")
                }
            }
            InitSystem::Runit => {
                let service_dir = Self::runit_service_dir();
                fs::create_dir_all(&service_dir)?;

                let run_script = service_dir.join("run");
                fs::write(&run_script, Self::generate_runit_run(exe_path))?;

                // Make executable
                Command::new("chmod")
                    .args(["+x", run_script.to_str().unwrap()])
                    .status()?;

                // Link to user services (varies by distro)
                let user_sv = dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("~"))
                    .join(".local/service");
                fs::create_dir_all(&user_sv)?;

                let link_path = user_sv.join(Self::SERVICE_NAME);
                if !link_path.exists() {
                    std::os::unix::fs::symlink(&service_dir, &link_path)?;
                }

                Ok(())
            }
            _ => {
                anyhow::bail!(
                    "Unsupported init system. Please start the daemon manually: {} daemon",
                    exe_path
                )
            }
        }
    }

    fn stop_daemon() -> Result<()> {
        let info = LinuxInfo::detect();

        match info.init_system {
            InitSystem::Systemd => {
                Command::new("systemctl")
                    .args(["--user", "disable", "--now", Self::SERVICE_NAME])
                    .status()
                    .ok();

                let service_path = Self::systemd_service_path();
                if service_path.exists() {
                    fs::remove_file(&service_path).ok();
                }

                Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status()
                    .ok();
            }
            InitSystem::OpenRC => {
                Command::new("sudo")
                    .args(["rc-service", Self::SERVICE_NAME, "stop"])
                    .status()
                    .ok();
                Command::new("sudo")
                    .args(["rc-update", "del", Self::SERVICE_NAME])
                    .status()
                    .ok();
                Command::new("sudo")
                    .args(["rm", "-f", Self::openrc_service_path().to_str().unwrap()])
                    .status()
                    .ok();
            }
            InitSystem::Runit => {
                let user_sv = dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("~"))
                    .join(".local/service")
                    .join(Self::SERVICE_NAME);

                if user_sv.exists() {
                    fs::remove_file(&user_sv).ok();
                }

                let service_dir = Self::runit_service_dir();
                if service_dir.exists() {
                    fs::remove_dir_all(&service_dir).ok();
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn setup_instructions() -> &'static str {
        let info = LinuxInfo::detect();
        match info.package_manager {
            PackageManager::Apt => {
                "fatrace requires root privileges.\nInstall with: sudo apt install fatrace"
            }
            PackageManager::Dnf => {
                "fatrace requires root privileges.\nInstall with: sudo dnf install fatrace"
            }
            PackageManager::Pacman => {
                "fatrace requires root privileges.\nInstall with: sudo pacman -S fatrace"
            }
            PackageManager::Zypper => {
                "fatrace requires root privileges.\nInstall with: sudo zypper install fatrace"
            }
            PackageManager::Apk => {
                "fatrace is not available in Alpine repos.\nYou may need to build from source."
            }
            _ => "fatrace requires root privileges.\nInstall using your system's package manager.",
        }
    }
}
