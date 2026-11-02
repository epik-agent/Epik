# epik (plugin)

The Epik plugin: manager-mode **epic** development on GitHub. Settle a design in conversation, author the epic's issue graph, and build it with Claude Code.

An *epic* is a dependency-ordered set of issues: an epic issue with sub-issues, implemented together on an epic branch.

## What's here

```
plugin/
  .claude-plugin/
    plugin.json         # plugin manifest
  skills/
    assistant/
      SKILL.md          # epik:assistant — how Epik plans; loads itself
      rules.md          # the rules that bind every change to the code
    issue/
      SKILL.md          # /epik:issue — implement one issue end to end
      procedure.md      # the per-issue procedure
    review/
      SKILL.md          # /epik:review — review one pull request
      procedure.md      # reviewing a pull request, and addressing a review
  .mcp.json             # runs the epik server: the Epik app's `epik-app mcp`
```

## Design in one paragraph

The skills are **policy**; `gh` is **mechanism**. The plugin is skills and configuration, with no code of its own. Its tools come from the Epik app (`crates/epik-app`), which must be installed: `.mcp.json` runs `epik-app mcp` from `~/.epik/bin/epik-app`, the link the app keeps to itself, and that server runs every GitHub operation through the `gh` CLI. `build_start` asks the running app to build, starting it if needed, and `build_status` and `build_list` ask it about runs, over the local socket in [`docs/app-protocol.md`](../docs/app-protocol.md).

## Install

For normal installation — download Epik and follow its setup checklist, which installs this plugin — see the [root README](../README.md#installing-and-uninstalling). The sections below only cover development setups.

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

The tools are the Epik app's, so trying a change to them means building the app (`cargo build -p epik-app`) and running it once, which points `~/.epik/bin/epik-app` at the build.

## Usage

- **The assistant skill (`epik:assistant`) loads itself.** It is not a command you run. The server's instructions point every session at it, so any conversation with the plugin enabled knows it is Epik. It is the exception to the explicit-invocation rule because it only shapes how Claude plans and writes. `issue` and `review` take actions (branches, pull requests, reviews), so they stay explicit.

The Epik app builds epics: it runs each issue's agent, merges each issue's pull request into the epic branch, and opens the epic's pull request. Each agent runs:

- **`/epik:issue [issue number or GitHub URL] [target branch]`** — implement a single issue: work in a git worktree, write tests first and get them passing, open a pull request into the target branch whose body says `Closes #<issue number>`, and drive it until every check passes. It never merges the pull request or closes the issue; it removes its worktree and leaves the branch for the open pull request.

To review a pull request by hand:

- **`/epik:review [pull request number or GitHub URL] [round]`** — read the issue the pull request closes and the pull request from GitHub, judge the change against them and Epik's rules, and post one GitHub review with a comment on each line to change and a verdict marker as its body's last line. It never pushes, merges, closes or edits.
