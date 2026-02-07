# dustbin

Find your dusty binaries — track which installed packages you actually use.

Ever installed a CLI tool, used it once, and forgot about it? `dustbin` tracks every binary execution on your system and shows you which packages are collecting dust.

## Features

- **Automatic tracking** — Monitors all binary executions in the background
- **Multi-source support** — Tracks Homebrew, Cargo, npm, pip, apt, and more
- **Smart reports** — Filter by source, usage count, or staleness
- **Interactive cleanup** — Select and uninstall unused packages
- **Cross-platform** — Works on macOS (eslogger) and Linux (fanotify)

## Installation

```bash
cargo install dustbin
```

## Quick Start

```bash
# Check status and start tracking
dustbin status

# View usage statistics
dustbin stats

# Show dusty (never used) binaries
dustbin report --dust

# Show packages not used in 30 days
dustbin report --stale 30

# Interactive cleanup
dustbin clean
```

## Usage

### Commands

| Command | Description |
|---------|-------------|
| `dustbin status` | Show daemon status and tracking info |
| `dustbin stats` | Show summary statistics with visual charts |
| `dustbin report` | Show detailed usage report |
| `dustbin clean` | Interactively remove unused packages |
| `dustbin config` | Show or edit configuration |
| `dustbin start` | Manually start the tracking daemon |
| `dustbin stop` | Stop the tracking daemon |

### Report Options

```bash
dustbin report              # Top 20 binaries by usage
dustbin report --dust       # Only unused binaries (count = 0)
dustbin report --low 5      # Binaries with < 5 uses
dustbin report --stale 30   # Not used in 30 days
dustbin report --source homebrew  # Filter by source
dustbin report --all        # Show all (no limit)
dustbin report --json       # JSON output for scripting
dustbin report --export     # Output uninstall commands
```

### Stats

```
  📦  dustbin
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
- macOS: `~/Library/Application Support/dustbin/config.toml`
- Linux: `~/.config/dustbin/config.toml`

```toml
scan_path = true
extra_scan_dirs = []
skip_dirs = ["/usr/bin", "/bin"]
skip_prefixes = ["/usr/libexec/"]

# Ignore these binaries in reports
ignore_binaries = ["python*-config"]

# Source detection patterns
[[sources]]
name = "homebrew"
path = "/opt/homebrew"

[[sources]]
name = "cargo"
path = ".cargo/bin"
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
