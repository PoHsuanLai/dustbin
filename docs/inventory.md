# Inventory

Dusty tracks binary usage via OS-level exec monitoring, which works for anything in PATH. But language package managers (R, pip, npm libraries, etc.) install packages that aren't standalone binaries — their usage happens inside a host process and can't be observed this way.

`dusty inventory` lets you list what's installed through these managers, and `dusty clean` can remove them.

## Setup

Add a `list_cmd` to any source in your config:

```toml
[[sources]]
name = "r"
path = "/usr/local/lib/R"
list_cmd = "Rscript -e 'cat(installed.packages()[,1], sep=\"\\n\")'"
uninstall_cmd = "Rscript -e 'remove.packages(\"%s\")'"

[[sources]]
name = "pip-user"
path = "~/.local/lib/python"
list_cmd = "pip list --format=freeze | cut -d= -f1"
uninstall_cmd = "pip uninstall -y"
```

The `list_cmd` must output **one package name per line** to stdout.

## Usage

```bash
dusty inventory              # list packages from all list_cmd sources
dusty inventory --source r   # show all R packages
dusty inventory --json       # machine-readable output
dusty clean --source r       # interactively remove R packages
```

## Uninstall command format

Two styles are supported:

- **Append** (default): `"pip uninstall -y"` — selected package names are appended to the command
- **Template**: `"Rscript -e 'remove.packages(\"%s\")'"` — if `uninstall_cmd` contains `%s`, it runs once per package with the name substituted

## Limitations

Since dusty can't observe language-level imports, inventory packages have no usage data. They won't appear in `dusty report` or `dusty stats` — use `dusty inventory` to see them.
