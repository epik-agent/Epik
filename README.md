# epik-gh

A GitHub MCP server that wraps the [`gh` CLI](https://cli.github.com/). It exposes GitHub operations as tools that an MCP client (such as Claude) can call directly.

## What it does

epik-gh gives an MCP client read/write access to GitHub through a focused set of tools:

- **Issues** — list, get, create, edit, close, reopen, comment
- **Pull requests** — list, get, create, edit, close, merge, review, comment
- **Branches** — list, create, delete
- **Labels** — list, create, delete
- **Repositories** — get metadata, get default branch
- **CI / Actions** — list runs, get run details, fetch run logs
- **Issue relationships** — set/remove blocked-by, add/remove sub-issues, list relationships
- **Projects V2** — list items, get item, set status, invalidate cache

All operations go through the `gh` CLI, so they run with whatever GitHub account `gh auth login` has authenticated.

## Prerequisites

- Python 3.11+
- [`uv`](https://docs.astral.sh/uv/) (recommended) or pip
- [`gh` CLI](https://cli.github.com/) installed and authenticated (`gh auth login`)

## Installation

### With uv (recommended)

```bash
uv tool install git+https://github.com/YOUR_ORG/epik-gh.git
```

Or clone and install locally:

```bash
git clone https://github.com/YOUR_ORG/epik-gh.git
cd epik-gh
uv tool install .
```

### With pip

```bash
pip install git+https://github.com/YOUR_ORG/epik-gh.git
```

## Configuring as an MCP server

### Claude Desktop

Add the following to your `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "epik-gh": {
      "command": "epik-gh"
    }
  }
}
```

If you installed with `uv tool`, the full path is typically `~/.local/bin/epik-gh`. You can confirm it with `which epik-gh`.

### Claude Code

```bash
claude mcp add epik-gh epik-gh
```

Or with the full path:

```bash
claude mcp add epik-gh ~/.local/bin/epik-gh
```

### Other MCP clients

Run the server over stdio:

```bash
epik-gh
```

Point your MCP client at that command. The server speaks the MCP stdio transport.

## Authentication

epik-gh delegates all authentication to the `gh` CLI. Before using the server, make sure you are logged in:

```bash
gh auth login
```

To verify:

```bash
gh auth status
```
