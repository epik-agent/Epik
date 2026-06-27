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

## Install (sketch)

This repo is its own marketplace. Add it as a plugin marketplace in Claude Code, then install `epik`. The `.mcp.json` runs `epik-gh` via `uvx` from its repo; `gh` must be installed and authenticated (`gh auth login`), and in cloud sessions a setup script must install `gh` and provide a token.

## Open refinements (agreed, not yet applied)

- **`issue` worktree base.** Step 1 should base the worktree on the HEAD of the *target* branch, not the default branch, so a dependent issue sees its blocker's merged code.
- **Parallel-merge race.** When `feature` runs issues in parallel and each merges into the shared feature branch, the "base branch needs to be refreshed" case must explicitly cover *another issue merged ahead of me*: refresh, re-run CI, then merge.
- **Convergence gate.** The feature-launch tool (in the future Claude-API MCP) is where the "is this theory ready to delegate?" check belongs — it can refuse, it isn't a prompt.
- **Forcing-function hooks.** "Never touch the default branch" and the "work started" project-status update want to be hooks (deterministic), not prose in the command.
- **Second MCP.** `feature_launch` (routines `/fire`) and `feature_status` belong in the Claude-API MCP, not in `epik-gh`.

## Status

Skeleton (v0.1.0). Manifest/marketplace/hook formats are starting points and may need adjustment against the current plugin schema.
