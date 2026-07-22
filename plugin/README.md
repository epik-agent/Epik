# epik (plugin)

The Epik plugin: manager-mode **feature** development on GitHub. Converge on a design in CoWork, author the feature's issue graph, and launch autonomous builds on Claude Code on the web — without leaving the thinking.

A *feature* is a unit of code implemented in one or more stories (issues).

## What's here

```
plugin/
  .claude-plugin/
    plugin.json         # plugin manifest
  commands/
    feature.md          # orchestrate a feature (Agent Teams, dependency order)
    issue.md            # implement one issue end to end
  hooks/
    hooks.json          # SessionStart Theory/Practice nudge
    session-start.sh
  .mcp.json             # declares the EpikMCP server (does not contain it)
```

## Design in one paragraph

The plugin is **policy**; the MCP is **mechanism**. `EpikMCP` (`../mcp` in this repo) is the GitHub mechanism — it authors the issue graph and reads status. The plugin declares the server via `.mcp.json`; it never vendors its source. Installing the plugin brings the declared server along, including into Claude Code on the web.

## Install

For normal installation — prerequisites, the two `/plugin` commands, verification, and troubleshooting — follow the [root README](../README.md#installation). The sections below only cover development setups.

### Developing the plugin — quick local test (no marketplace)

Fastest way to try changes; loads the plugin for one session only:

```bash
claude --plugin-dir /path/to/Epik/plugin
```

Iterate with `/reload-plugins` after edits. No marketplace or install step needed.

### Developing the plugin — install from a local clone (persistent)

The repo root is a single-entry marketplace, so a local clone works exactly like the GitHub install in the root README — just add the clone's root directory instead of `epik-agent/Epik`:

```
/plugin marketplace add /path/to/Epik
/plugin install epik@epik
```

After committing later changes to the plugin, run `/plugin marketplace update epik` then `/reload-plugins` (or bump `version` in `plugin.json`) to pick them up.

Note: if an `EpikMCP` server is also registered by hand in the same client (e.g. an entry you added to a `.mcp.json` outside the plugin), the two same-named servers collide as duplicate `mcp__EpikMCP__*` tools. Remove the manual entry — `/plugin uninstall` doesn't touch plain MCP registrations.

### Cloud sessions (Claude Code on the web)

A local-path marketplace isn't reachable from a cloud VM. To use Epik there, declare `epik-agent/Epik` as a marketplace/plugin in the *project repo's* `.claude/settings.json`; the plugin and its MCP declaration then load at session start. The session's setup script must also `apt install -y gh` and provide a `GH_TOKEN`, since `gh` isn't pre-installed in the cloud.

## Usage

Two commands, both namespaced under `epik`:

- **`/epik:feature [feature issue number or GitHub URL] [feature branch]`** — implement a feature: the set of related issues a feature issue points to. It creates the feature branch, implements the issues in dependency order using Agent Teams (in parallel where the dependencies allow), opens a pull request per issue against the feature branch, and shepherds each through CI and review.
- **`/epik:issue [issue number or GitHub URL] [target branch]`** — implement a single issue end to end: work in a git worktree, get tests passing, open a pull request, drive it through `/review` and CI, merge into the target branch, close the issue, and clean up.

The SessionStart hook prints a Theory/Practice nudge so you stay aware of which mode you're in: manager mode (delegated, autonomous feature builds) is safe only once the design has converged; while you're still discovering the design you're in theory-building mode and shouldn't delegate a build yet.

## Status

Skeleton (v0.1.0). Manifest, marketplace, and hook formats are starting points and may need adjustment against the current plugin schema.
