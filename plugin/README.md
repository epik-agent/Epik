# epik (plugin)

The Epik plugin: manager-mode **feature** development on GitHub. Settle a design in conversation, author the feature's issue graph, and build it locally with Claude Code.

An *epic* is a dependency-ordered set of issues: an epic issue with sub-issues, implemented together on an epic branch.

## What's here

```
plugin/
  .claude-plugin/
    plugin.json         # plugin manifest
  skills/
    epic/
      SKILL.md          # /epik:epic — implement an epic's issues with Agent Teams
    issue/
      SKILL.md          # /epik:issue — implement one issue end to end
  .mcp.json             # declares the EpikMCP server (does not contain it)
```

## Design in one paragraph

The plugin is **policy**; the MCP is **mechanism**. `EpikMCP` (`../mcp` in this repo) is the GitHub mechanism — it authors the issue graph and reads status. The plugin declares the server via `.mcp.json`; it never vendors its source. Installing the plugin brings the declared server along.

## Install

For normal installation — prerequisites, the two `/plugin` commands, verification, and troubleshooting — follow the [root README](../README.md#installing-and-uninstalling). The sections below only cover development setups.

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
```

```
/plugin install epik@epik
```

After committing later changes to the plugin, run `/plugin marketplace update epik` then `/reload-plugins` to pick them up. `plugin.json` sets no `version`, so each commit counts as a new version.

Note: if an `EpikMCP` server is also registered by hand in the same client (e.g. an entry you added to a `.mcp.json` outside the plugin), the two same-named servers collide as duplicate `mcp__EpikMCP__*` tools. Remove the manual entry — `/plugin uninstall` doesn't touch plain MCP registrations.

## Usage

Two build skills, both invoked explicitly as slash commands namespaced under `epik` (they never auto-trigger):

- **`/epik:epic [epic issue number or GitHub URL] [epic branch]`** — implement an epic: the dependency-ordered set of issues an epic issue points to. Creates the epic branch if needed, then uses Agent Teams to implement the issues in dependency order (in parallel where the order allows), each via `/epik:issue` targeting the epic branch, and finally opens a pull request for the epic.
- **`/epik:issue [issue number or GitHub URL] [target branch]`** — implement a single issue end to end: work in a git worktree, get tests passing, run `/simplify`, open a pull request linked to the issue, drive it through CI, merge into the target branch, close the issue, and clean up.
