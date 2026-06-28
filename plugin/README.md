# epik (plugin)

The Epik plugin: manager-mode **feature** development on GitHub. Converge on a design in CoWork, author the feature's issue graph, and launch autonomous builds on Claude Code on the web — without leaving the thinking.

A *feature* is a unit of code implemented in one or more stories (issues). See `../design-history/0001-epik-architecture.md` for the architecture and rationale.

## What's here

```
epik-plugin/
  .claude-plugin/
    plugin.json         # plugin manifest
    marketplace.json    # single-entry marketplace (this repo installs itself)
  commands/
    feature.md          # orchestrate a feature (Agent Teams, dependency order)
    issue.md            # implement one issue end to end
  hooks/
    hooks.json          # SessionStart Theory/Practice nudge
    session-start.sh
  .mcp.json             # declares the epik-gh MCP (does not contain it)
```

## Design in one paragraph

The plugin is **policy**; the MCPs are **mechanism**. `epik-gh` (separate repo) is the GitHub mechanism — a curated `gh` wrapper for authoring the issue graph and reading status. A second, future **Claude-API MCP** (separate repo) will own non-GitHub calls such as the routines `/fire` launch. The plugin declares those servers via `.mcp.json`; it never vendors their source. Installing the plugin brings the declared servers along, including into Claude Code on the web.

## Install

### Prerequisites

- **Claude Code up to date** — type `/plugin` and confirm the command exists. If it doesn't, update Claude Code.
- **`uv` installed** — the plugin launches `epik-gh` with `uvx`. See https://docs.astral.sh/uv/.
- **`gh` installed and authenticated** — run `gh auth login`, verify with `gh auth status`. `epik-gh` delegates all GitHub auth to `gh`.
- **`epik-gh` repo reachable** — `.mcp.json` fetches it from `github.com/wpm/epik-gh`. If that repo is private, make sure git can authenticate to it.

### Step 0 — remove the old standalone epik-gh

If you previously wired `epik-gh` into the desktop app by hand, remove it first. The plugin registers an MCP server *also* named `epik-gh`, and two same-named servers in one client collide (duplicate `mcp__epik-gh__*` tools).

- Edit `~/Library/Application Support/Claude/claude_desktop_config.json` and delete the `epik-gh` block under `mcpServers`.
- Optional cleanup of the old binary: `uv tool uninstall epik-gh`.

That manual entry is a plain MCP registration, not a plugin, so `/plugin uninstall` does **not** apply to it.

### Option A — quick local test (no marketplace)

Fastest way to try it; loads the plugin for one session only:

```bash
claude --plugin-dir /Users/mcneill/Projects/Epik/epik-plugin
```

Iterate with `/reload-plugins` after edits. No marketplace or install step needed.

### Option B — install from the local marketplace (persistent)

This repo is its own single-entry marketplace.

1. Add the marketplace — point at the plugin directory (it holds `.claude-plugin/marketplace.json`):

   ```
   /plugin marketplace add /Users/mcneill/Projects/Epik/epik-plugin
   ```

2. Install the plugin. The form is `plugin-name@marketplace-name`; here both are `epik`:

   ```
   /plugin install epik@epik
   ```

3. If it doesn't show up immediately, run `/reload-plugins`.

After you commit later changes to the plugin, run `/plugin marketplace update epik` then `/reload-plugins` (or bump `version` in `plugin.json`) to pick them up.

### Verify

- `/help` lists the commands, namespaced: **`/epik:feature`** and **`/epik:issue`**.
- The `epik-gh` tools (`mcp__epik-gh__*`) are available — try a read, e.g. ask for the repo's open issues.
- A fresh session prints the Theory/Practice "which mode are you in" nudge from the SessionStart hook.

### Cloud sessions (Claude Code on the web)

A local-path marketplace isn't reachable from a cloud VM. To use Epik there, **push `epik-plugin` to GitHub** and declare it as a marketplace/plugin in the *project repo's* `.claude/settings.json`; the plugin and its MCP declaration then load at session start. The session's setup script must also `apt install -y gh` and provide a `GH_TOKEN`, since `gh` isn't pre-installed in the cloud.

## Open refinements (agreed, not yet applied)

- **`issue` worktree base.** Step 1 should base the worktree on the HEAD of the *target* branch, not the default branch, so a dependent issue sees its blocker's merged code.
- **Parallel-merge race.** When `feature` runs issues in parallel and each merges into the shared feature branch, the "base branch needs to be refreshed" case must explicitly cover *another issue merged ahead of me*: refresh, re-run CI, then merge.
- **Convergence gate.** The feature-launch tool (in the future Claude-API MCP) is where the "is this theory ready to delegate?" check belongs — it can refuse, it isn't a prompt.
- **Forcing-function hooks.** "Never touch the default branch" and the "work started" project-status update want to be hooks (deterministic), not prose in the command.
- **Second MCP.** `feature_launch` (routines `/fire`) and `feature_status` belong in the Claude-API MCP, not in `epik-gh`.

## Status

Skeleton (v0.1.0). Manifest/marketplace/hook formats are starting points and may need adjustment against the current plugin schema.
