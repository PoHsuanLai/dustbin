//! Platform-specific implementations for process monitoring and daemon management

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

use anyhow::Result;
use std::sync::mpsc::Receiver;

/// Trait for platform-specific process monitoring
pub trait ProcessMonitor {
    fn new() -> Self;
    fn start(&mut self) -> Result<Receiver<String>>;
    fn stop(&mut self) -> Result<()>;
}

/// Trait for platform-specific daemon management
pub trait DaemonManager {
    /// Check if the monitoring tool is available on this system
    fn check_available() -> bool;

    /// Check if daemon is currently running
    fn is_daemon_running() -> bool;

    /// Install and start the daemon
    fn start_daemon(exe_path: &str) -> Result<()>;

    /// Stop and uninstall the daemon
    fn stop_daemon() -> Result<()>;

    /// Check if monitoring has required permissions (e.g. Full Disk Access on macOS)
    fn check_permissions() -> bool;

    /// Get platform-specific setup instructions
    fn setup_instructions() -> &'static str;

    /// Get the log path or command for viewing daemon logs
    fn log_hint() -> String;

    /// View daemon logs (tail/follow)
    fn view_logs(lines: usize, follow: bool) -> Result<()>;
}

/// A dynamic library dependency of a binary
#[derive(Debug, Clone)]
pub struct DylibDep {
    pub path: String,
}

/// Result of analyzing a binary's dynamic library dependencies
#[derive(Debug)]
pub struct DylibAnalysis {
    pub libs: Vec<DylibDep>,
}

/// A library file resolved to its owning package
#[derive(Debug, Clone)]
pub struct LibPackageInfo {
    pub lib_path: String,
    pub manager: String,
    pub package_name: String,
}

/// Trait for platform-specific dynamic library analysis
pub trait DylibAnalyzer {
    /// Analyze a binary's dynamic library dependencies
    fn analyze_binary(binary_path: &str) -> Result<DylibAnalysis>;

    /// Resolve library paths to their owning packages (batch)
    fn resolve_lib_packages(lib_paths: &[String]) -> Result<Vec<LibPackageInfo>>;

    /// Get installed size of a package in bytes
    fn get_package_size(manager: &str, package_name: &str) -> Result<Option<u64>>;
}
