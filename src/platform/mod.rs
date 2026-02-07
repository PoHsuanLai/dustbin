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

    /// Get platform-specific setup instructions
    fn setup_instructions() -> &'static str;
}
