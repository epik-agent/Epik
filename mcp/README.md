# EpikMCP

EpikMCP is the single MCP for Epik. It exposes its functionality through two
internal modules:

- **plan** — GitHub access via the [`gh` CLI](https://cli.github.com/). Read and
  write issues, relationships, projects, labels and repos, plus read-only access
  to pull requests and CI runs.
- **build** — launch headless feature builds on GitHub Actions. Dispatches the
  repository's `epik-build.yml` workflow, which runs a headless Claude Code
  feature build.

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

### build / GitHub Actions

- **Feature launch** (`feature_launch`)
  - `feature_launch` — dispatch the `epik-build.yml` workflow to start a
    headless feature build

See [Build module: `feature_launch`](#build-module-feature_launch) below for
the repository setup the workflow requires.

### Prompts

Besides tools, the server exposes two MCP prompts, so surfaces that don't
load Claude Code plugins (Claude Desktop, CoWork) can still summon Epik —
they appear in the chat's **+** (attach) menu:

- **`summon-epik`** — the Epik persona: engagement instructions plus the
  Theory-and-Practice philosophy. Sending it yields "Hello, I'm Epik."
- **`setup-epik`** — the Epik doctor: a detect → offer → fix → report
  dialogue over Epik's own setup (`gh` auth, project initialization, the
  headless-build workflow, build secrets).

The prompt texts are owned by the plugin (`plugin/skills/design/`,
`plugin/skills/doctor/`) and vendored into this package under
`src/epik_mcp/resources/`. After editing the canonical files, run
`uv run python scripts/sync_resources.py` from `mcp/`; CI (and
`--check`) fail while the vendored copies drift.

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

The build module additionally requires the target repository to be set up for
headless builds (see below); the workflow it dispatches uses repository
secrets, not local environment variables.

## Build module: `feature_launch`

The build module launches headless feature builds on GitHub Actions. The
`feature_launch` tool runs

```bash
gh workflow run epik-build.yml --repo <owner/name> \
  --field feature_issue_number=<n> \
  --field base_branch=<base> \
  --field target_branch=<feature-branch>
```

which dispatches the repository's `.github/workflows/epik-build.yml` workflow.
That workflow runs a headless Claude Code session that builds the feature on
the given feature branch and reports progress through GitHub (per-issue pull
requests, issue comments, and the run itself). An optional `ref` argument
selects the git ref to run the workflow from (default: the repository default
branch; the ref must contain the workflow file).

`feature_launch` returns a dispatch receipt. Follow the build with the
read-only run tools (`run_list` / `run_get` / `run_logs` filtered to workflow
`epik-build.yml`) and the `feature_status` aggregator.

### Repository setup

1. The target repository must contain the `epik-build.yml` workflow (this
   repo's [`.github/workflows/epik-build.yml`](../.github/workflows/epik-build.yml)
   is the reference implementation).
2. The **`ANTHROPIC_API_KEY` repository secret must be configured** — the
   headless Claude Code session authenticates with it.
3. Optional: an `EPIK_BUILD_GH_TOKEN` secret (a PAT with repo scope). Pull
   requests opened with the default `github.token` do not trigger downstream
   workflows, so without this the build's per-issue PRs won't run CI.
4. The `gh` account used by the MCP server needs permission to dispatch
   workflows in the target repository (`actions: write`).

Dispatch failures (missing workflow, no permission) surface as clear gh
errors; malformed arguments raise validation errors before any call is made.
