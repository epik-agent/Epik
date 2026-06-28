# epik (plugin)

The Epik plugin: manager-mode **feature** development on GitHub. Converge on a design in CoWork, author the feature's issue graph, and launch autonomous builds on Claude Code on the web — without leaving the thinking.

A *feature* is a unit of code implemented in one or more stories (issues).

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
  .mcp.json             # declares the EpikMCP server (does not contain it)
```

## Design in one paragraph

The plugin is **policy**; the MCP is **mechanism**. `EpikMCP` (separate repo) is the GitHub mechanism — it authors the issue graph and reads status. The plugin declares the server via `.mcp.json`; it never vendors its source. Installing the plugin brings the declared server along, including into Claude Code on the web.

## Install

### Prerequisites

- **Claude Code up to date** — type `/plugin` and confirm the command exists. If it doesn't, update Claude Code.
- **`uv` installed** — the plugin launches `EpikMCP` with `uvx`. See https://docs.astral.sh/uv/.
- **`gh` installed and authenticated** — run `gh auth login`, verify with `gh auth status`. `EpikMCP` delegates all GitHub auth to `gh`.
- **`EpikMCP` repo reachable** — `.mcp.json` fetches it from `github.com/wpm/EpikMCP`. If that repo is private, make sure git can authenticate to it.

### Step 0 — remove any same-named manual MCP registration

If you previously wired an `EpikMCP` server into the desktop app by hand, remove it first. The plugin registers an MCP server named `EpikMCP`, and two same-named servers in one client collide (duplicate `mcp__EpikMCP__*` tools).

- Edit `~/Library/Application Support/Claude/claude_desktop_config.json` and delete the `EpikMCP` block under `mcpServers`.

A manual entry is a plain MCP registration, not a plugin, so `/plugin uninstall` does **not** apply to it.

### Option A — quick local test (no marketplace)

Fastest way to try it; loads the plugin for one session only:

```bash
claude --plugin-dir /path/to/epik-plugin
```

Iterate with `/reload-plugins` after edits. No marketplace or install step needed.

### Option B — install from the local marketplace (persistent)

This repo is its own single-entry marketplace.

1. Add the marketplace — point at the plugin directory (it holds `.claude-plugin/marketplace.json`):

   ```
   /plugin marketplace add /path/to/epik-plugin
   ```

2. Install the plugin. The form is `plugin-name@marketplace-name`; here both are `epik`:

   ```
   /plugin install epik@epik
   ```

3. If it doesn't show up immediately, run `/reload-plugins`.

After you commit later changes to the plugin, run `/plugin marketplace update epik` then `/reload-plugins` (or bump `version` in `plugin.json`) to pick them up.

### Verify

- `/help` lists the commands, namespaced: **`/epik:feature`** and **`/epik:issue`**.
- The `EpikMCP` tools (`mcp__EpikMCP__*`) are available — try a read, e.g. ask for the repo's open issues.
- A fresh session prints the Theory/Practice "which mode are you in" nudge from the SessionStart hook.

### Cloud sessions (Claude Code on the web)

A local-path marketplace isn't reachable from a cloud VM. To use Epik there, **push `epik-plugin` to GitHub** and declare it as a marketplace/plugin in the *project repo's* `.claude/settings.json`; the plugin and its MCP declaration then load at session start. The session's setup script must also `apt install -y gh` and provide a `GH_TOKEN`, since `gh` isn't pre-installed in the cloud.

## Usage

Two commands, both namespaced under `epik`:

- **`/epik:feature [feature issue number or GitHub URL] [feature branch]`** — implement a feature: the set of related issues a feature issue points to. It creates the feature branch, implements the issues in dependency order using Agent Teams (in parallel where the dependencies allow), opens a pull request per issue against the feature branch, and shepherds each through CI and review.
- **`/epik:issue [issue number or GitHub URL] [target branch]`** — implement a single issue end to end: work in a git worktree, get tests passing, open a pull request, drive it through `/review` and CI, merge into the target branch, close the issue, and clean up.

The SessionStart hook prints a Theory/Practice nudge so you stay aware of which mode you're in: manager mode (delegated, autonomous feature builds) is safe only once the design has converged; while you're still discovering the design you're in theory-building mode and shouldn't delegate a build yet.

## Status

Skeleton (v0.1.0). Manifest, marketplace, and hook formats are starting points and may need adjustment against the current plugin schema.
