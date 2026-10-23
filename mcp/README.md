# EpikMCP

EpikMCP is the single MCP for Epik: GitHub access via the
[`gh` CLI](https://cli.github.com/). Read and write issues, relationships,
projects, labels and repos, plus read-only access to pull requests and CI runs.

An MCP client (such as Claude) calls these tools directly. Tools are registered
under the `mcp__epik-mcp__*` prefix.

## Module and tool layout

### plan / GitHub (via `gh`)

All plan tools run through the `gh` CLI, so they operate with whatever GitHub
account `gh auth login` has authenticated.

- **Issues** (`issues`)
  - `issue_list` — list issues in a repository
  - `issue_get` — get a single issue
  - `issue_create` — create an issue
  - `issue_edit` — edit an issue
  - `issue_close` — close an issue
  - `issue_reopen` — reopen an issue
  - `issue_comment` — comment on an issue
- **Relationships** (`relationships`)
  - `issue_set_blocked_by` — mark an issue as blocked by another
  - `issue_remove_blocked_by` — remove a blocked-by relationship
  - `issue_list_relationships` — list an issue's relationships
  - `issue_add_sub_issue` — add a sub-issue
  - `issue_remove_sub_issue` — remove a sub-issue
- **Projects V2** (`projects`)
  - `project_list_items` — list project items
  - `project_get_item` — get a single project item
  - `project_set_status` — set an item's status
  - `project_invalidate_cache` — invalidate the cached project IDs
- **Labels** (`labels`)
  - `label_list` — list labels
  - `label_create` — create a label
  - `label_delete` — delete a label
- **Repositories** (`repos`)
  - `repo_get` — get repository metadata
  - `repo_default_branch` — get the default branch
- **Pull requests** (`prs`, read-only)
  - `pr_list` — list pull requests
  - `pr_get` — get a single pull request
- **CI / Actions runs** (`runs`, read-only)
  - `run_list` — list workflow runs
  - `run_get` — get a single run
  - `run_logs` — fetch run logs
- **Raw passthrough** (`raw`)
  - `gh_raw` — run a raw `gh` subcommand
- **Feature status** (`feature_status`)
  - `feature_status` — aggregate the plan-side status of a feature

## Prerequisites

- Python 3.11+
- [`uv`](https://docs.astral.sh/uv/) (recommended) or pip
- [`gh` CLI](https://cli.github.com/) installed and authenticated (`gh auth login`)

## Installation

### Run directly with uvx

```bash
uvx --from git+https://github.com/epik-agent/Epik.git#subdirectory=mcp epik-mcp
```

### Install as a uv tool

```bash
uv tool install git+https://github.com/epik-agent/Epik.git#subdirectory=mcp
```

Or clone and install locally:

```bash
git clone https://github.com/epik-agent/Epik.git
cd Epik/mcp
uv tool install .
```

### With pip

```bash
pip install git+https://github.com/epik-agent/Epik.git#subdirectory=mcp
```

## Configuring as a CoWork / Claude MCP server

Add the following to your Claude MCP config (for example,
`~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "epik-mcp": {
      "command": "uvx",
      "args": [
        "--from",
        "git+https://github.com/epik-agent/Epik.git#subdirectory=mcp",
        "epik-mcp"
      ]
    }
  }
}
```

If you installed the `epik-mcp` command as a uv tool instead, you can point the
config directly at the binary:

```json
{
  "mcpServers": {
    "epik-mcp": {
      "command": "/Users/YOUR_USERNAME/.local/bin/epik-mcp"
    }
  }
}
```

Confirm the binary path with:

```bash
which epik-mcp
```

## Authentication

Both modules authenticate through the `gh` CLI. Before using the tools, make
sure you are logged in:

```bash
gh auth login
```

To verify:

```bash
gh auth status
```
