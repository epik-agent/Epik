# EpikMCP

EpikMCP is the single MCP for Epik. It exposes its functionality through two
internal modules:

- **plan** — GitHub access via the [`gh` CLI](https://cli.github.com/). Read and
  write issues, relationships, projects, labels and repos, plus read-only access
  to pull requests and CI runs.
- **build** — launch feature builds via Anthropic. Fires a saved Claude Code
  routine to start a remote feature build.

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

### build / Anthropic

- **Feature launch** (`feature_launch`)
  - `feature_launch` — start a remote feature build

See [Build module: `feature_launch`](#build-module-feature_launch) below for the
one-time routine setup and required environment variables.

## Prerequisites

- Python 3.11+
- [`uv`](https://docs.astral.sh/uv/) (recommended) or pip
- [`gh` CLI](https://cli.github.com/) installed and authenticated (`gh auth login`)
  for the plan module

## Installation

### Run directly with uvx

```bash
uvx --from git+https://github.com/wpm/EpikMCP.git epik-mcp
```

### Install as a uv tool

```bash
uv tool install git+https://github.com/wpm/EpikMCP.git
```

Or clone and install locally:

```bash
git clone https://github.com/wpm/EpikMCP.git
cd EpikMCP
uv tool install .
```

### With pip

```bash
pip install git+https://github.com/wpm/EpikMCP.git
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
        "git+https://github.com/wpm/EpikMCP.git",
        "epik-mcp"
      ],
      "env": {
        "EPIK_ROUTINE_ID": "<your-routine-id>",
        "EPIK_ROUTINE_TOKEN": "<your-routine-token>"
      }
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
      "command": "/Users/YOUR_USERNAME/.local/bin/epik-mcp",
      "env": {
        "EPIK_ROUTINE_ID": "<your-routine-id>",
        "EPIK_ROUTINE_TOKEN": "<your-routine-token>"
      }
    }
  }
}
```

Confirm the binary path with:

```bash
which epik-mcp
```

The build-module env vars are only needed if you intend to use `feature_launch`.

## Authentication

EpikMCP keeps the two modules' authentication separate:

- **plan module** delegates all authentication to the `gh` CLI. Before using the
  plan tools, make sure you are logged in:

  ```bash
  gh auth login
  ```

  To verify:

  ```bash
  gh auth status
  ```

- **build module** uses the routine environment variables (`EPIK_ROUTINE_ID` and
  `EPIK_ROUTINE_TOKEN`) described below. It does not use the `gh` CLI.

## Build module: `feature_launch`

The build module is the Anthropic-side half of Epik. It keeps its auth and state
**separate** from the `gh` CLI used by the plan module. The `feature_launch`
tool starts a remote feature build by firing a saved
[Claude Code routine](https://claude.ai/code) via the routines API.

When called with a feature issue number plus a base and target branch, it POSTs
to the routine's fire endpoint and returns the URL of the cloud session it
started.

### One-time routine setup

1. In the Claude Code web UI, create a routine (for example, a "feature runner")
   that builds a feature when given a feature command.
2. Add an **API trigger** to the routine. Its prompt should be the feature-command
   body (the instruction your build follows). `feature_launch` sends the feature
   issue number and branches as the trigger's `text`.
3. Enable **"Allow unrestricted branch pushes"** for the repository so the routine
   can push the build branch.
4. Copy the routine's **id** and its **API token** — you'll set these as env vars.

### Required environment variables

These are read on every call and must be set in the environment the MCP server
runs in:

| Variable | Description |
| --- | --- |
| `EPIK_ROUTINE_ID` | The Claude Code routine id whose fire endpoint is called. |
| `EPIK_ROUTINE_TOKEN` | The routine API bearer token. |

If either is missing or empty, `feature_launch` raises a clear validation error
naming the missing variable. Non-2xx responses and connection failures also raise
clear errors.
