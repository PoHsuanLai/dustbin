# Trash

`dusty clean` moves directories to trash instead of permanently deleting them.

## Workflow

```bash
dusty clean --source opt    # moves to ~/.local/share/dusty/trash/
dusty trash                 # see what's in trash (with sizes)
dusty restore anaconda3     # move it back
dusty trash --drop anaconda3  # permanently delete one
dusty trash --empty         # permanently delete all
dusty clean --no-trash      # skip trash, delete immediately
```

## How it works

When you remove a package through `dusty clean`:

- **Package manager sources** (brew, cargo, etc.) — dusty runs the uninstall command directly. The package manager handles deletion, but dusty records what was removed and the reinstall command so `dusty restore <name>` can tell you how to get it back.

- **Unmanaged sources** (standalone installs like anaconda, opt directories) — dusty moves the directory to `~/.local/share/dusty/trash/` instead of deleting it. `dusty restore <name>` moves it back to the original location.

## Commands

### `dusty trash`

Lists everything in trash with sizes, source, deletion method, and date.

```
  Package                   Source       Size       Method             Deleted
  anaconda3                 opt        11.5 GB     moved to trash     2025-06-15 14:30
  httpie                    homebrew        -       uninstalled        2025-06-14 09:12
```

Flags:
- `--drop <name>` — permanently delete a specific trashed package
- `--empty` — permanently delete everything in trash
- `--json` — machine-readable output

### `dusty restore <name>`

- For **moved** packages: moves the directory back to its original location
- For **uninstalled** packages: shows the reinstall command (e.g., `brew install httpie`)
