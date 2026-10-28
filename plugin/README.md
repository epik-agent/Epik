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
    assistant/
      SKILL.md          # epik:assistant — how Epik plans; loads itself
      rules.md          # the rules that bind every change to the code
    issue/
      SKILL.md          # /epik:issue — implement one issue end to end
      procedure.md      # the per-issue procedure
  .mcp.json             # declares the epik server
```

## Design in one paragraph

The skills are **policy**; `gh` is **mechanism**. The `epik` server ships inside the plugin and runs every GitHub operation through the `gh` CLI, under the user's own Node. Installing the plugin brings the server along. Builds are handed to the Epik app (`crates/epik-app`), which is installed separately: `build_start` asks it to build, and `build_status` and `build_list` ask it about builds, over the local socket in [`docs/app-protocol.md`](../docs/app-protocol.md).

## Install

For normal installation — prerequisites, the Epik app, the two `/plugin` commands, verification, and troubleshooting — follow the [root README](../README.md#installing-and-uninstalling). The sections below only cover development setups.

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

- **The assistant skill (`epik:assistant`) loads itself.** It is not a command you run. The server's instructions point every session at it, so any conversation with the plugin enabled knows it is Epik. It is the exception to the explicit-invocation rule because it only shapes how Claude plans and writes. `issue` takes actions (branches, pull requests), so it stays explicit.

The Epik app builds epics: it runs each issue's agent, merges each issue's pull request into the epic branch, and opens the epic's pull request. Each agent runs:

- **`/epik:issue [issue number or GitHub URL] [target branch]`** — implement a single issue: work in a git worktree, write tests first and get them passing, open a pull request into the target branch whose body says `Closes #<issue number>`, and drive it until every check passes. It never merges the pull request or closes the issue; it removes its worktree and leaves the branch for the open pull request.
