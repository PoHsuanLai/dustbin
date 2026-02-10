# Configuration

```bash
dusty config        # show current config
dusty config --edit # open in $EDITOR
```

Config location:
- macOS: `~/Library/Application Support/dusty/config.toml`
- Linux: `~/.config/dusty/config.toml`

## Full example

```toml
[scan]
path = true                              # scan $PATH directories
extra_dirs = ["/opt/custom/bin"]         # additional directories to scan
skip_dirs = ["/usr/bin", "/bin"]         # directories to ignore
skip_prefixes = ["/usr/libexec/"]        # path prefixes to ignore
ignore_binaries = ["python*-config"]     # binary names to hide in reports

[[sources]]
name = "homebrew"
path = "/opt/homebrew"                   # path pattern to match
uninstall_cmd = "brew uninstall"         # used by dusty clean
list_cmd = "brew list --formula -1"      # used by dusty inventory (optional)
```

## Scan options

| Key | Default | Description |
|-----|---------|-------------|
| `path` | `true` | Scan directories from `$PATH` |
| `extra_dirs` | `[]` | Additional directories to scan beyond PATH |
| `skip_dirs` | system dirs | Directories to skip even if in PATH |
| `skip_prefixes` | system prefixes | Path prefixes to ignore when tracking |
| `ignore_binaries` | `[]` | Binary name patterns to hide in reports (supports `*` glob) |

## Sources

Each `[[sources]]` entry tells dusty how to categorize binaries by path:

| Key | Required | Description |
|-----|----------|-------------|
| `name` | yes | Source name (e.g., `"homebrew"`, `"cargo"`) |
| `path` | yes | Path pattern — if a binary's path contains this string, it belongs to this source |
| `uninstall_cmd` | no | Command used by `dusty clean` to uninstall packages |
| `list_cmd` | no | Command used by `dusty inventory` to list installed packages (see [Inventory](inventory.md)) |

Sources are auto-detected on first run. Edit the config to add custom sources, ignore noisy binaries, or configure `list_cmd` for language package managers.

## Shell completions

```bash
# Bash
dusty completions --shell bash > ~/.local/share/bash-completion/completions/dusty

# Zsh
dusty completions --shell zsh > ~/.zfunc/_dusty

# Fish
dusty completions --shell fish > ~/.config/fish/completions/dusty.fish
```
