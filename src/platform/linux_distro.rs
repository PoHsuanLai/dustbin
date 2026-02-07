//! Linux distribution detection and configuration

use std::fs;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum Distro {
    Debian,   // Debian, Ubuntu, Mint, Pop!_OS, etc.
    Fedora,   // Fedora, RHEL, CentOS, Rocky, Alma
    Arch,     // Arch, Manjaro, EndeavourOS
    OpenSUSE, // openSUSE Leap/Tumbleweed
    Alpine,   // Alpine Linux
    Void,     // Void Linux
    NixOS,    // NixOS
    Gentoo,   // Gentoo
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Yum,
    Pacman,
    Zypper,
    Apk,
    Xbps,
    Nix,
    Portage,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InitSystem {
    Systemd,
    OpenRC,
    Runit,
    SysV,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct LinuxInfo {
    pub distro: Distro,
    pub package_manager: PackageManager,
    pub init_system: InitSystem,
}

impl LinuxInfo {
    pub fn detect() -> Self {
        let distro = detect_distro();
        let package_manager = detect_package_manager(&distro);
        let init_system = detect_init_system();

        Self {
            distro,
            package_manager,
            init_system,
        }
    }

    /// Get the install command for fatrace
    pub fn fatrace_install_cmd(&self) -> Option<Vec<&'static str>> {
        match self.package_manager {
            PackageManager::Apt => Some(vec!["sudo", "apt", "install", "-y", "fatrace"]),
            PackageManager::Dnf => Some(vec!["sudo", "dnf", "install", "-y", "fatrace"]),
            PackageManager::Yum => Some(vec!["sudo", "yum", "install", "-y", "fatrace"]),
            PackageManager::Pacman => Some(vec!["sudo", "pacman", "-S", "--noconfirm", "fatrace"]),
            PackageManager::Zypper => Some(vec!["sudo", "zypper", "install", "-y", "fatrace"]),
            PackageManager::Apk => None, // fatrace not in Alpine repos
            PackageManager::Xbps => Some(vec!["sudo", "xbps-install", "-y", "fatrace"]),
            PackageManager::Nix => Some(vec!["nix-env", "-iA", "nixpkgs.fatrace"]),
            PackageManager::Portage => Some(vec!["sudo", "emerge", "fatrace"]),
            PackageManager::Unknown => None,
        }
    }
}

fn detect_distro() -> Distro {
    // Try /etc/os-release first (most reliable)
    if let Ok(content) = fs::read_to_string("/etc/os-release") {
        let id = content
            .lines()
            .find(|l| l.starts_with("ID="))
            .map(|l| l.trim_start_matches("ID=").trim_matches('"').to_lowercase());

        let id_like = content
            .lines()
            .find(|l| l.starts_with("ID_LIKE="))
            .map(|l| {
                l.trim_start_matches("ID_LIKE=")
                    .trim_matches('"')
                    .to_lowercase()
            });

        if let Some(id) = id {
            match id.as_str() {
                "debian" | "ubuntu" | "linuxmint" | "pop" | "elementary" | "zorin" | "kali" => {
                    return Distro::Debian;
                }
                "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => return Distro::Fedora,
                "arch" | "manjaro" | "endeavouros" | "garuda" => return Distro::Arch,
                "opensuse" | "opensuse-leap" | "opensuse-tumbleweed" | "sles" => {
                    return Distro::OpenSUSE;
                }
                "alpine" => return Distro::Alpine,
                "void" => return Distro::Void,
                "nixos" => return Distro::NixOS,
                "gentoo" => return Distro::Gentoo,
                _ => {}
            }

            // Check ID_LIKE for derivatives
            if let Some(like) = id_like {
                if like.contains("debian") || like.contains("ubuntu") {
                    return Distro::Debian;
                }
                if like.contains("fedora") || like.contains("rhel") {
                    return Distro::Fedora;
                }
                if like.contains("arch") {
                    return Distro::Arch;
                }
                if like.contains("suse") {
                    return Distro::OpenSUSE;
                }
            }
        }
    }

    Distro::Unknown
}

fn detect_package_manager(distro: &Distro) -> PackageManager {
    // First check based on distro
    match distro {
        Distro::Debian => return PackageManager::Apt,
        Distro::Fedora => {
            // Fedora uses dnf, older RHEL/CentOS use yum
            if command_exists("dnf") {
                return PackageManager::Dnf;
            }
            return PackageManager::Yum;
        }
        Distro::Arch => return PackageManager::Pacman,
        Distro::OpenSUSE => return PackageManager::Zypper,
        Distro::Alpine => return PackageManager::Apk,
        Distro::Void => return PackageManager::Xbps,
        Distro::NixOS => return PackageManager::Nix,
        Distro::Gentoo => return PackageManager::Portage,
        Distro::Unknown => {}
    }

    // Fallback: detect by available commands
    if command_exists("apt") {
        PackageManager::Apt
    } else if command_exists("dnf") {
        PackageManager::Dnf
    } else if command_exists("yum") {
        PackageManager::Yum
    } else if command_exists("pacman") {
        PackageManager::Pacman
    } else if command_exists("zypper") {
        PackageManager::Zypper
    } else if command_exists("apk") {
        PackageManager::Apk
    } else if command_exists("xbps-install") {
        PackageManager::Xbps
    } else if command_exists("nix-env") {
        PackageManager::Nix
    } else if command_exists("emerge") {
        PackageManager::Portage
    } else {
        PackageManager::Unknown
    }
}

fn detect_init_system() -> InitSystem {
    // Check for systemd (most common)
    if fs::metadata("/run/systemd/system").is_ok() {
        return InitSystem::Systemd;
    }

    // Check for OpenRC
    if command_exists("rc-service")
        || fs::metadata("/etc/init.d").is_ok() && command_exists("openrc")
    {
        return InitSystem::OpenRC;
    }

    // Check for runit
    if fs::metadata("/run/runit").is_ok() || command_exists("sv") {
        return InitSystem::Runit;
    }

    // Check for SysV init
    if fs::metadata("/etc/init.d").is_ok() {
        return InitSystem::SysV;
    }

    InitSystem::Unknown
}

fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
