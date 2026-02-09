//! Linux-specific implementation using fanotify and systemd

#[path = "linux_distro.rs"]
mod linux_distro;
pub use linux_distro::{InitSystem, LinuxInfo, PackageManager};

use super::{DaemonManager, DylibAnalysis, DylibAnalyzer, DylibDep, LibPackageInfo, ProcessMonitor};
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
    const SERVICE_NAME: &'static str = "dusty";

    fn systemd_service_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("systemd/user/dusty.service")
    }

    fn generate_systemd_service(exe_path: &str) -> String {
        format!(
            r#"[Unit]
Description=Dusty - Track binary usage
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
        PathBuf::from("/etc/init.d/dusty")
    }

    fn generate_openrc_service(exe_path: &str) -> String {
        format!(
            r#"#!/sbin/openrc-run

name="dusty"
description="Dusty - Track binary usage"
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
            .join(".local/sv/dusty")
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
                    let tmp = "/tmp/dusty-openrc";
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

/// Linux dynamic library analyzer using ldd
pub struct Analyzer;

/// Core glibc libraries to skip (always present, not interesting for dependency analysis)
const SKIP_LIB_PREFIXES: &[&str] = &[
    "linux-vdso.so",
    "linux-gate.so",
    "ld-linux",
    "libpthread.so",
    "libdl.so",
    "librt.so",
    "libm.so",
    "libc.so",
];

impl DylibAnalyzer for Analyzer {
    fn analyze_binary(binary_path: &str) -> Result<DylibAnalysis> {
        let output = Command::new("ldd").arg(binary_path).output();

        let output = match output {
            Ok(o) => o,
            Err(_) => {
                return Ok(DylibAnalysis {

                    libs: vec![],
                });
            }
        };

        if !output.status.success() {
            // "not a dynamic executable" or similar
            return Ok(DylibAnalysis {
                binary_path: binary_path.to_string(),
                libs: vec![],
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let libs = stdout
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                // Lines with => contain resolved paths: "libfoo.so => /usr/lib/libfoo.so (0x...)"
                if let Some(arrow_pos) = trimmed.find("=>") {
                    let after_arrow = trimmed[arrow_pos + 2..].trim();
                    let path = after_arrow.split(" (").next()?.trim();
                    if path.is_empty() || path == "not found" {
                        return None;
                    }
                    let lib_name = trimmed.split("=>").next()?.trim();
                    if SKIP_LIB_PREFIXES.iter().any(|p| lib_name.starts_with(p)) {
                        return None;
                    }
                    Some(DylibDep {
                        path: path.to_string(),
                    })
                } else {
                    // Lines without => are either the loader or vdso, skip
                    None
                }
            })
            .collect();

        Ok(DylibAnalysis { libs })
    }

    fn resolve_lib_packages(lib_paths: &[String]) -> Result<Vec<LibPackageInfo>> {
        let info = LinuxInfo::detect();
        match info.package_manager {
            PackageManager::Apt => resolve_via_dpkg(lib_paths),
            PackageManager::Dnf | PackageManager::Yum | PackageManager::Zypper => {
                resolve_via_rpm(lib_paths)
            }
            PackageManager::Pacman => resolve_via_pacman(lib_paths),
            _ => Ok(vec![]),
        }
    }

    fn get_package_size(_manager: &str, package_name: &str) -> Result<Option<u64>> {
        let info = LinuxInfo::detect();
        match info.package_manager {
            PackageManager::Apt => {
                // dpkg-query returns size in KB
                let output = Command::new("dpkg-query")
                    .args(["-W", "-f=${Installed-Size}", package_name])
                    .output();
                if let Ok(output) = output {
                    if output.status.success() {
                        let size_str = String::from_utf8_lossy(&output.stdout);
                        if let Ok(kb) = size_str.trim().parse::<u64>() {
                            return Ok(Some(kb * 1024));
                        }
                    }
                }
            }
            PackageManager::Pacman => {
                let output = Command::new("pacman")
                    .args(["-Qi", package_name])
                    .output();
                if let Ok(output) = output {
                    if output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        if let Some(size) = parse_pacman_size(&stdout) {
                            return Ok(Some(size));
                        }
                    }
                }
            }
            PackageManager::Dnf | PackageManager::Yum | PackageManager::Zypper => {
                let output = Command::new("rpm")
                    .args(["-qi", package_name])
                    .output();
                if let Ok(output) = output {
                    if output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        if let Some(size) = parse_rpm_size(&stdout) {
                            return Ok(Some(size));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }
}

/// Resolve library paths via dpkg -S (Debian/Ubuntu)
fn resolve_via_dpkg(lib_paths: &[String]) -> Result<Vec<LibPackageInfo>> {
    let mut results = Vec::new();
    for chunk in lib_paths.chunks(50) {
        let args: Vec<&str> = std::iter::once("-S")
            .chain(chunk.iter().map(|s| s.as_str()))
            .collect();
        let output = Command::new("dpkg").args(&args).output();
        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Output: "package:arch: /path/to/file"
            for line in stdout.lines() {
                if let Some((pkg_part, path_part)) = line.split_once(": ") {
                    let package_name = pkg_part.split(':').next().unwrap_or(pkg_part).trim();
                    let lib_path = path_part.trim();
                    results.push(LibPackageInfo {
                        lib_path: lib_path.to_string(),
                        manager: "apt".to_string(),
                        package_name: package_name.to_string(),
                    });
                }
            }
        }
    }
    Ok(results)
}

/// Resolve library paths via rpm -qf (Fedora/RHEL/SUSE)
fn resolve_via_rpm(lib_paths: &[String]) -> Result<Vec<LibPackageInfo>> {
    let mut results = Vec::new();
    for chunk in lib_paths.chunks(50) {
        let mut args = vec!["-qf", "--queryformat", "%{NAME}\\n"];
        for path in chunk {
            args.push(path.as_str());
        }
        let output = Command::new("rpm").args(&args).output();
        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for (pkg_name, lib_path) in stdout.lines().zip(chunk.iter()) {
                    let pkg_name = pkg_name.trim();
                    if !pkg_name.is_empty() && !pkg_name.starts_with("file ") {
                        results.push(LibPackageInfo {
                            lib_path: lib_path.clone(),
                            manager: "rpm".to_string(),
                            package_name: pkg_name.to_string(),
                        });
                    }
                }
            }
        }
    }
    Ok(results)
}

/// Resolve library paths via pacman -Qo (Arch)
fn resolve_via_pacman(lib_paths: &[String]) -> Result<Vec<LibPackageInfo>> {
    let mut results = Vec::new();
    for lib_path in lib_paths {
        let output = Command::new("pacman")
            .args(["-Qo", lib_path.as_str()])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Output: "/usr/lib/libfoo.so is owned by package-name 1.2.3-4"
                if let Some(pkg) = stdout
                    .split("is owned by ")
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next())
                {
                    results.push(LibPackageInfo {
                        lib_path: lib_path.clone(),
                        manager: "pacman".to_string(),
                        package_name: pkg.to_string(),
                    });
                }
            }
        }
    }
    Ok(results)
}

/// Parse "Installed Size" from pacman -Qi output
fn parse_pacman_size(output: &str) -> Option<u64> {
    for line in output.lines() {
        if line.starts_with("Installed Size") {
            let value = line.split(':').nth(1)?.trim();
            return parse_human_size(value);
        }
    }
    None
}

/// Parse "Size" from rpm -qi output
fn parse_rpm_size(output: &str) -> Option<u64> {
    for line in output.lines() {
        if line.starts_with("Size") {
            let value = line.split(':').nth(1)?.trim();
            // rpm reports size in bytes directly
            return value.parse::<u64>().ok();
        }
    }
    None
}

/// Parse human-readable size strings like "1.2 MiB", "340 KiB"
fn parse_human_size(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }
    let num: f64 = parts[0].parse().ok()?;
    let multiplier = match parts[1] {
        "B" => 1.0,
        "KiB" | "KB" => 1024.0,
        "MiB" | "MB" => 1024.0 * 1024.0,
        "GiB" | "GB" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((num * multiplier) as u64)
}
