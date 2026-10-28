# epik (plugin)

The Epik plugin: manager-mode **epic** development on GitHub. Settle a design in conversation, author the epic's issue graph, and build it with Claude Code.

An *epic* is a dependency-ordered set of issues: an epic issue with sub-issues, implemented together on an epic branch.

## What's here

```
plugin/
  .claude-plugin/
    plugin.json         # plugin manifest
  server/               # the Node MCP server (see server/README.md)
  skills/
    epic/
      SKILL.md          # /epik:epic — implement an epic's issues in parallel subagents
    issue/
      SKILL.md          # /epik:issue — implement one issue end to end
      procedure.md      # the per-issue procedure, shared by both skills
  .mcp.json             # declares the epik server
```

## Design in one paragraph

The skills are **policy**; `gh` is **mechanism**. The `epik` server ships inside the plugin and runs every GitHub operation through the `gh` CLI, under the user's own Node. Installing the plugin brings the server along.

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

Changes to the server's source need a rebuilt `server/dist/index.js` (see [`server/README.md`](server/README.md)); CI fails when the bundle is stale.

## Usage

Two build skills, both invoked explicitly as slash commands namespaced under `epik` (they never auto-trigger):

- **`/epik:epic [epic issue number or GitHub URL] [epic branch]`** — implement an epic: the epic issue's sub-issues. Creates the epic branch if needed, then implements the sub-issues in the dependency order the `epic_status` tool reports (in parallel subagents where the order allows), each following `/epik:issue`'s procedure and targeting the epic branch, and finally opens a pull request for the epic.
- **`/epik:issue [issue number or GitHub URL] [target branch]`** — implement a single issue end to end: work in a git worktree, get tests passing, run `/simplify`, open a pull request linked to the issue, drive it through CI, merge into the target branch, close the issue, and clean up.
