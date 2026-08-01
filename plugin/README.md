# epik (plugin)

The Epik plugin: manager-mode **feature** development on GitHub. Settle a design in conversation, author the feature's issue graph, and launch headless builds on GitHub Actions — without leaving the thinking.

A *feature* is a unit of code implemented in one or more stories (issues).

## What's here

```
plugin/
  .claude-plugin/
    plugin.json         # plugin manifest
  skills/
    summon/
      SKILL.md          # /epik:summon — summon Epik as a design partner
      persona.md        # canonical persona (vendored into EpikMCP as summon-epik)
      theory-and-practice.md
    feature/
      SKILL.md          # /epik:feature — launch a headless feature build and monitor it
    init-repository/
      SKILL.md          # /epik:init-repository — set a repository up for Epik
      init.md           # canonical spec of a correct Epik project
                        # (vendored into EpikMCP as init-repository)
    issue/
      SKILL.md          # /epik:issue — implement one issue end to end
  hooks/
    hooks.json          # SessionStart pointer to /epik:summon
    session-start.sh
  templates/
    loop.md             # default /loop body: build monitoring (written to
                        # projects as .claude/loop.md by epik:init-repository)
    settings.json       # .claude/settings.json template that epik:init-repository writes
                        # into projects
  .mcp.json             # declares the EpikMCP server (does not contain it)
```

## Design in one paragraph

The plugin is **policy**; the MCP is **mechanism**. `EpikMCP` (`../mcp` in this repo) is the GitHub mechanism — it authors the issue graph and reads status. The plugin declares the server via `.mcp.json`; it never vendors its source. Installing the plugin brings the declared server along, including into Claude Code on the web.

One nuance: the persona and setup texts are policy, owned here (`skills/summon/persona.md`, `theory-and-practice.md`, `skills/init-repository/init.md`), but the MCP *serves* them as the `summon-epik` and `init-repository` prompts so plugin-less surfaces (Claude Desktop, Cowork) can summon Epik too. The server vendors copies (`mcp/scripts/sync_resources.py` syncs; CI fails on drift) — ownership stays with the plugin.

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

After committing later changes to the plugin, run `/plugin marketplace update epik` then `/reload-plugins` (or bump `version` in `plugin.json`) to pick them up.

Note: if an `EpikMCP` server is also registered by hand in the same client (e.g. an entry you added to a `.mcp.json` outside the plugin), the two same-named servers collide as duplicate `mcp__EpikMCP__*` tools. Remove the manual entry — `/plugin uninstall` doesn't touch plain MCP registrations.

### Cloud sessions (Claude Code on the web)

A local-path marketplace isn't reachable from a cloud VM. To use Epik there, declare `epik-agent/Epik` as a marketplace/plugin in the *project repo's* `.claude/settings.json` (the exact JSON is [`templates/settings.json`](templates/settings.json)); the plugin and its MCP declaration then load at session start. The session's setup script must also `apt install -y gh` and provide a `GH_TOKEN`, since `gh` isn't pre-installed in the cloud.

## Usage

Two build skills, both invoked explicitly as slash commands namespaced under `epik` (they never auto-trigger):

- **`/epik:feature [feature issue number or GitHub URL] [feature branch]`** — launch a headless build of a feature: the set of related issues a feature issue points to. It sanity-checks the issue graph, calls the EpikMCP `feature_launch` tool to dispatch the repo's `epik-build.yml` GitHub Actions workflow (which runs the build on the feature branch — the repo needs that workflow plus an `ANTHROPIC_API_KEY` secret), then hands off to `/loop` to monitor via `feature_status` / `run_list`, interrupting only on needs-me events.
- **`/epik:issue [issue number or GitHub URL] [target branch]`** — implement a single issue end to end: work in a git worktree, get tests passing, open a pull request, drive it through `/review` and CI, merge into the target branch, close the issue, and clean up.

Plus two explicitly invoked skills:

- **`/epik:summon`** — summon Epik, your software-design partner. Epik introduces itself: *"Hello, I'm Epik."*
- **`/epik:init-repository`** — set this repository up for Epik, or repair one that has drifted: safe to run any time, doing only what the project's current state calls for. No repository yet → create it. A repository that isn't an Epik project → offer conversion. An Epik project that has drifted → offer the diff. Already correct → say so and stop. The spec it applies covers `gh` auth, `.claude/settings.json` (per-project enablement, merged into any existing settings — see [`templates/settings.json`](templates/settings.json)), a `CLAUDE.md` stub, `.claude/loop.md` (the build-monitoring `/loop` default), a `docs/design-history/` scaffold, the headless-build workflow, build secrets, repository conventions from `~/.epik/config.json`, and plugin health. Detect → offer → fix → report; nothing changes without your agreement to that specific change, and only credential pastes are left to the human. Creation is repo-first — the repository is built through the GitHub API, no clone involved — and changes to a repository that already exists arrive as a pull request. (The same dialogue is the `init-repository` MCP prompt in Desktop/Cowork.)

  [`skills/init-repository/init.md`](skills/init-repository/init.md) is the single home of that spec: creation applies all of it, repair applies the diff, so the two can't drift apart. It absorbed the former `/epik:doctor` skill, whose behaviour it keeps in full — see [the ADR](../docs/design-history/2026-07-28-init-is-idempotent-convergence.md).

The SessionStart hook prints a one-line pointer to `/epik:summon` — presence without invocation. It stays silent in CI and headless build sessions so the persona never bleeds into builders.

## Status

Skeleton (v0.1.0). Manifest, marketplace, and hook formats are starting points and may need adjustment against the current plugin schema.
