# dusty

[![CI](https://github.com/PoHsuanLai/dusty/actions/workflows/ci.yml/badge.svg)](https://github.com/PoHsuanLai/dusty/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/dusty.svg)](https://crates.io/crates/dusty)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Find your dusty binaries — track which installed packages you actually use.

Ever installed a CLI tool, used it once, and forgot about it? `dusty` tracks every binary execution on your system and shows you which packages are collecting dust.

## Features

- **Automatic tracking** — Monitors all binary executions in the background
- **Multi-source support** — Tracks Homebrew, Cargo, npm, pip, apt, and more
- **Smart reports** — Filter by source, usage count, or staleness
- **Interactive cleanup** — Select and uninstall unused packages
- **Cross-platform** — Works on macOS (eslogger) and Linux (fanotify)

## Installation

```bash
cargo install dusty
```

## Quick Start

```bash
# Check status and start tracking
dusty status

# View usage statistics
dusty stats

# Show dusty (never used) binaries
dusty report --dust

# Show packages not used in 30 days
dusty report --stale 30

# Interactive cleanup
dusty clean
```

## Usage

### Commands

| Command | Description |
|---------|-------------|
| `dusty status` | Show daemon status and tracking info |
| `dusty stats` | Show summary statistics with visual charts |
| `dusty report` | Show detailed usage report |
| `dusty clean` | Interactively remove unused packages |
| `dusty dupes` | Find duplicate binaries across sources |
| `dusty deps` | Analyze dynamic library dependencies |
| `dusty config` | Show or edit configuration |
| `dusty start` | Manually start the tracking daemon |
| `dusty stop` | Stop the tracking daemon |

### Report Options

```bash
dusty report              # Binaries by usage (fits terminal)
dusty report --dust       # Only unused binaries (count = 0)
dusty report --low 5      # Binaries with < 5 uses
dusty report --stale 30   # Not used in 30 days
dusty report --source homebrew  # Filter by source
dusty report --all        # Show all (no limit)
dusty report --json       # JSON output for scripting
dusty report --export     # Output uninstall commands
```

### Stats

```
  📦  dusty
  ────────────────────────────────────────

  Tracking for 45 days

  4435 binaries
  ██████████████████████████████

  ■    29  active (5+ uses)
  ■    23  low (1-4 uses)
  ■  4383  dusty (never used)

  By source
  ─────────────────────────
    homebrew  ▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪ 3190
         apt  ▪▪▪▪ 1004
       cargo  ▪ 43
```

## Configuration

Config file location:
- macOS: `~/Library/Application Support/dusty/config.toml`
- Linux: `~/.config/dusty/config.toml`

```toml
[scan]
path = true
extra_dirs = []
skip_dirs = ["/usr/bin", "/bin"]
skip_prefixes = ["/usr/libexec/"]
ignore_binaries = ["python*-config"]

[[sources]]
name = "homebrew"
path = "/opt/homebrew"
uninstall_cmd = "brew uninstall"

[[sources]]
name = "cargo"
path = ".cargo/bin"
uninstall_cmd = "cargo uninstall"
```

## How It Works

### macOS
Uses `eslogger` (Endpoint Security) to monitor process executions. Requires:
- macOS 13 (Ventura) or later
- Full Disk Access for your terminal app

### Linux
Uses `fanotify` via `fatrace` to monitor file access. Requires:
- `fatrace` package (`sudo apt install fatrace`)
- Root privileges for monitoring

The daemon runs as a launchd service (macOS) or systemd user service (Linux).

## Requirements

- **macOS**: 13.0+ (Ventura)
- **Linux**: Kernel 2.6.37+ with fanotify support
- **Rust**: 1.85+ (for building from source)

## License

MIT
