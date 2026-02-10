//! macOS-specific implementation using eslogger and launchd

use super::{
    DaemonManager, DylibAnalysis, DylibAnalyzer, DylibDep, LibPackageInfo, ProcessMonitor,
};
use anyhow::{Context, Result};
use chrono::Local;
use serde::Deserialize;
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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

/// Spawn eslogger to monitor exec events
fn spawn_eslogger() -> Result<Child> {
    Command::new("eslogger")
        .arg("exec")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn eslogger. Make sure you have Full Disk Access enabled.")
}

fn timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// macOS process monitor using eslogger with automatic restart on crash
pub struct Monitor {
    stop_flag: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    parse_errors: Arc<AtomicU64>,
}

impl Monitor {
    /// Get and reset the parse error count since last call
    pub fn take_parse_errors(&self) -> u64 {
        self.parse_errors.swap(0, Ordering::Relaxed)
    }
}

impl ProcessMonitor for Monitor {
    fn new() -> Self {
        Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
            parse_errors: Arc::new(AtomicU64::new(0)),
        }
    }

    fn start(&mut self) -> Result<Receiver<String>> {
        let (tx, rx) = mpsc::channel();
        let stop_flag = self.stop_flag.clone();
        let child_holder = self.child.clone();
        let parse_errors = self.parse_errors.clone();

        thread::spawn(move || {
            let mut backoff = Duration::from_secs(2);

            while !stop_flag.load(Ordering::Relaxed) {
                match spawn_eslogger() {
                    Ok(mut child) => {
                        let pid = child.id();
                        println!("[{}] eslogger started (pid: {})", timestamp(), pid);

                        let stdout = child.stdout.take().unwrap();

                        // Log eslogger stderr for diagnostics
                        if let Some(stderr) = child.stderr.take() {
                            thread::spawn(move || {
                                let reader = std::io::BufReader::new(stderr);
                                for line in reader.lines().map_while(Result::ok) {
                                    eprintln!("[eslogger stderr] {}", line);
                                }
                            });
                        }

                        *child_holder.lock().unwrap() = Some(child);
                        backoff = Duration::from_secs(2); // reset on successful spawn

                        let reader = std::io::BufReader::new(stdout);
                        for line in reader.lines().map_while(Result::ok) {
                            if stop_flag.load(Ordering::Relaxed) {
                                break;
                            }
                            match serde_json::from_str::<EsloggerEvent>(&line) {
                                Ok(event) => {
                                    if let Some(path) = event.executable_path() {
                                        if tx.send(path.to_string()).is_err() {
                                            return; // receiver dropped
                                        }
                                    }
                                }
                                Err(_) => {
                                    parse_errors.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }

                        // stdout closed — eslogger exited
                        if !stop_flag.load(Ordering::Relaxed) {
                            // Reap the child to get exit status
                            let status = child_holder
                                .lock()
                                .unwrap()
                                .as_mut()
                                .and_then(|c| c.wait().ok());
                            let status_str = status
                                .map(|s| format!("{}", s))
                                .unwrap_or_else(|| "unknown".into());
                            println!(
                                "[{}] eslogger exited (status: {}), restarting in {}s",
                                timestamp(),
                                status_str,
                                backoff.as_secs()
                            );
                        }
                    }
                    Err(e) => {
                        println!(
                            "[{}] eslogger spawn failed: {}, retrying in {}s",
                            timestamp(),
                            e,
                            backoff.as_secs()
                        );
                    }
                }

                if !stop_flag.load(Ordering::Relaxed) {
                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        });

        Ok(rx)
    }

    fn stop(&mut self) -> Result<()> {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(ref mut child) = *self.child.lock().unwrap() {
            // Try SIGTERM first for graceful shutdown
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            // Wait up to 5 seconds
            for _ in 0..50 {
                if let Ok(Some(_)) = child.try_wait() {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(100));
            }
            // Fall back to SIGKILL
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
        // System-level LaunchDaemon (runs as root, required for eslogger)
        PathBuf::from("/Library/LaunchDaemons/com.dusty.daemon.plist")
    }

    fn log_dir() -> PathBuf {
        PathBuf::from("/var/log/dusty")
    }

    fn generate_plist(exe_path: &str) -> String {
        let log_dir = Self::log_dir();
        let log_path = log_dir.join("dusty.log");
        let err_path = log_dir.join("dusty.err");
        // Set HOME so dirs::data_local_dir() / dirs::config_dir() resolve
        // to the real user's paths, not /var/root/ (daemon runs as root)
        let user_home = dirs::home_dir()
            .expect("Could not determine home directory")
            .to_string_lossy()
            .to_string();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
            label = Self::LABEL,
            exe = exe_path,
            home = user_home,
            log = log_path.to_string_lossy(),
            err = err_path.to_string_lossy(),
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

    fn check_permissions() -> bool {
        // Check the daemon's stderr log for FDA errors
        let err_path = Self::log_dir().join("dusty.err");
        let Ok(content) = std::fs::read_to_string(&err_path) else {
            return true; // No err file, assume OK
        };
        // If the last eslogger error mentions NOT_PERMITTED, FDA is missing
        !content.contains("NOT_PERMITTED") && !content.contains("Not permitted")
    }

    fn is_daemon_running() -> bool {
        // Check if the plist exists and the process is alive (no sudo needed)
        if !Self::plist_path().exists() {
            return false;
        }
        // Check if any dusty daemon process is running
        Command::new("pgrep")
            .args(["-f", "dusty daemon"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn start_daemon(exe_path: &str) -> Result<()> {
        let plist_path = Self::plist_path();
        let plist_content = Self::generate_plist(exe_path);
        let log_dir = Self::log_dir();

        // Create log dir (needs sudo since it's /var/log/)
        Command::new("sudo")
            .args(["mkdir", "-p", &log_dir.to_string_lossy()])
            .status()
            .ok();

        // Write plist to a temp file then sudo mv (can't write to /Library/LaunchDaemons/ directly)
        let tmp = std::env::temp_dir().join("com.dusty.daemon.plist");
        fs::write(&tmp, plist_content)?;

        Command::new("sudo")
            .args(["cp", &tmp.to_string_lossy(), &plist_path.to_string_lossy()])
            .status()
            .context("Failed to install plist")?;
        fs::remove_file(&tmp).ok();

        let status = Command::new("sudo")
            .args(["launchctl", "load", "-w", &*plist_path.to_string_lossy()])
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

        let status = Command::new("sudo")
            .args(["launchctl", "unload", &*plist_path.to_string_lossy()])
            .status()
            .context("Failed to unload launchd job")?;

        if status.success() {
            Command::new("sudo")
                .args(["rm", "-f", &*plist_path.to_string_lossy()])
                .status()
                .ok();
            Ok(())
        } else {
            anyhow::bail!("Failed to stop daemon")
        }
    }

    fn setup_instructions() -> &'static str {
        "One-time setup: grant Full Disk Access to /usr/bin/eslogger\n\
         System Settings > Privacy & Security > Full Disk Access > add /usr/bin/eslogger\n\
         (Press Cmd+Shift+G in the file picker to type the path)\n\
         Then start the daemon with: sudo dusty start"
    }

    fn log_hint() -> String {
        Self::log_dir().display().to_string()
    }

    fn view_logs(lines: usize, follow: bool) -> Result<()> {
        let log_file = Self::log_dir().join("dusty.log");

        if !log_file.exists() {
            anyhow::bail!(
                "No log file found at {}. Is the daemon running?",
                log_file.display()
            );
        }

        let mut cmd = Command::new("tail");
        cmd.arg("-n").arg(lines.to_string());
        if follow {
            cmd.arg("-f");
        }
        cmd.arg(&log_file);
        cmd.status().context("Failed to run tail")?;
        Ok(())
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
