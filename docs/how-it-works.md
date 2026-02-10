# How It Works

```
                          ┌──────────────┐
                          │   eslogger   │  (macOS)
                          │   fatrace    │  (Linux)
                          └──────┬───────┘
                                 │ exec events
                          ┌──────▼───────┐
                          │    daemon    │  background process
                          │  (dusty start) │  matches against PATH
                          └──────┬───────┘
                                 │ binary path + count
                          ┌──────▼───────┐
                          │   SQLite DB  │  ~/.local/share/dusty/
                          └──────┬───────┘
                                 │
              ┌──────────────────┼──────────────────┐
              ▼                  ▼                  ▼
         dusty report      dusty clean        dusty size
         dusty stats       dusty trash        dusty deps
         dusty why         dusty restore      dusty dupes
```

## Daemon

A background process monitors every `exec` syscall on your system. On macOS this uses Apple's Endpoint Security framework via `eslogger`. On Linux it uses `fanotify` via `fatrace`. Only binary paths matching your configured sources are recorded — everything else is ignored.

## Database

A local SQLite database stores each binary's path, execution count, first/last seen timestamps, source (homebrew, cargo, npm, ...), and package name. The daemon writes to it; all commands read from it.

Database location: `~/.local/share/dusty/`

## Sync

When you run any command, dusty first scans your PATH directories and registers any new binaries it finds. It also prunes binaries that no longer exist on disk. This means the database stays current even if you install or remove packages between daemon runs.

## Categorization

Each binary is matched to a source by path pattern (e.g., `/opt/homebrew` → homebrew, `~/.cargo/bin` → cargo). Package names are extracted from Homebrew Cellar symlinks or install root directories.

Sources are auto-detected on first run and stored in your [config file](configuration.md).
