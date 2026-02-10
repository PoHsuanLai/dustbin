# dusty

[![CI](https://github.com/PoHsuanLai/dusty/actions/workflows/ci.yml/badge.svg)](https://github.com/PoHsuanLai/dusty/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/dusty.svg)](https://crates.io/crates/dusty)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Track which installed packages you actually use. Find the rest, and clean them up.

```
❯ dusty stats

  2478 binaries tracked across 4 sources

  ■    89  active (5+ uses)
  ■   113  low (1-4 uses)
  ■  2276  dusty (never used)
```

<!-- TODO: add demo video here -->

## Install

```bash
cargo install dusty
```

## Setup

```bash
dusty status
```

This starts the background daemon and tells you if anything else is needed.

**macOS** — Grant Full Disk Access to `/usr/bin/eslogger`:

> System Settings > Privacy & Security > Full Disk Access > add `/usr/bin/eslogger`
> (press Cmd+Shift+G in the file picker to type the path)

Then: `sudo dusty start`

**Linux** — Install `fatrace` and run as root:

```bash
sudo apt install fatrace   # or dnf, pacman, etc.
sudo dusty start
```

## Usage

```bash
dusty report --dust          # what's collecting dust?
dusty report --stale 30      # not used in 30 days
dusty clean --source homebrew # interactive cleanup
dusty size --dust             # how much space can I reclaim?
```

### Commands

| Command | Description |
|---------|-------------|
| `dusty status` | Daemon status and tracking info |
| `dusty stats` | Summary with visual charts |
| `dusty report` | Usage report (filter by `--dust`, `--stale`, `--source`, `--low`) |
| `dusty clean` | Interactively remove unused packages |
| `dusty size` | Disk space per package |
| `dusty why <name>` | Explain why a binary is installed |
| `dusty dupes` | Find duplicate binaries across sources |
| `dusty deps` | Analyze dynamic library dependencies |
| `dusty trash` | List trashed packages (`--drop <name>`, `--empty`) |
| `dusty restore <name>` | Restore a trashed package |
| `dusty inventory` | List packages from external managers (R, pip, etc.) |
| `dusty config` | Show or edit configuration |
| `dusty log` | Show daemon logs (`-n`, `--follow`) |

Most commands support `--json` for scripting and `--all` to bypass terminal height limits.

## Documentation

- [How It Works](docs/how-it-works.md) — architecture, daemon, database
- [Configuration](docs/configuration.md) — config file reference, sources, scan options
- [Trash](docs/trash.md) — safe deletion, restore, permanent cleanup
- [Inventory](docs/inventory.md) — tracking language packages (R, pip, etc.)

## Requirements

- **macOS**: 13.0+ (Ventura), Full Disk Access for `/usr/bin/eslogger`
- **Linux**: Kernel 2.6.37+, `fatrace`, root privileges
- **Rust**: 1.85+ (for building from source)

## License

MIT
