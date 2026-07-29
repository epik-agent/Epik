# Epik

_You say it. We make it._

Manager-mode feature development on GitHub: converge on a design, author the
feature's issue graph, and launch autonomous builds on Claude Code on the web.

## Layout

- [`mcp/`](mcp/README.md) — **EpikMCP**, the GitHub mechanism: an MCP server
  that authors the issue graph, reads status, and carries Epik's prompts.
- [`plugin/`](plugin/README.md) — the **epik** Claude Code plugin, the policy
  layer: skills, hooks, and the declaration of the EpikMCP server.

## Installation

These sections cover only what must happen before Epik can speak. Everything
after that — checking `gh` auth, initializing a project, configuring headless
builds — Epik does itself, in conversation (see [Summoning Epik](#summoning-epik)).

### Claude Code

You need [Claude Code](https://claude.com/claude-code) (current version), the
[`gh` CLI](https://cli.github.com/) logged in (`gh auth login`), and
[`uv`](https://docs.astral.sh/uv/) (its `uvx` command runs the EpikMCP server
for you). Then, inside a Claude Code session:

```
/plugin marketplace add epik-agent/Epik
/plugin install epik@epik
/reload-plugins
```

(`epik-agent/Epik` is this repository; the doubled `epik@epik` is
`plugin@marketplace`, both named in [`.claude-plugin/marketplace.json`](.claude-plugin/marketplace.json).)

To update later: `/plugin marketplace update epik` then `/reload-plugins`.
To uninstall: `/plugin uninstall epik@epik` then `/plugin marketplace remove epik`.

### Claude Desktop / CoWork

Claude Desktop (including CoWork sessions) doesn't load Claude Code plugins;
it gets Epik from the EpikMCP server directly. Add the server to the desktop
app's config file — `~/Library/Application Support/Claude/claude_desktop_config.json`
on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows — using
the absolute path to `uvx` (`which uvx`), and keeping any other servers
already in the block:

```json
{
  "mcpServers": {
    "EpikMCP": {
      "command": "/path/to/uvx",
      "args": [
        "--from",
        "git+https://github.com/epik-agent/Epik.git#subdirectory=mcp",
        "epik-mcp"
      ],
      "env": {
        "PATH": "/opt/homebrew/bin:/path/to/uvx-directory:/usr/bin:/bin:/usr/sbin:/sbin"
      }
    }
  }
}
```

Absolute paths matter because the desktop app launches servers with a minimal
`PATH`; `env.PATH` must include the directories containing `uvx` and `gh`.
Quit and relaunch the desktop app, then start a **new** conversation.

## Summoning Epik

| Surface | Summon Epik | Converge a project |
|---|---|---|
| Claude Code (CLI, desktop app, web, IDE) | `/epik:summon` | `/epik:init` |
| Claude Desktop / CoWork | **+** (attach) → **summon-epik** → send | **+** (attach) → **init-epik** → send |

Epik introduces itself: *"Hello, I'm Epik."* If it didn't say hello, you
aren't talking to Epik.

From there, everything is dialogue.

### `/epik:init` — idempotent convergence

One command covers setting a project up and putting it right, because they are
the same thing: **`/epik:init` converges this project on the correct Epik
shape, and is safe to run any number of times.** What it does depends on what
it finds — no repository yet, a repository that isn't an Epik project, an Epik
project that has drifted, or one that's already correct (in which case it says
so and stops). A greenfield project is simply a maximally broken setup.

It covers `gh` authentication, the project's `.claude/settings.json`
enablement and scaffolding, the headless build workflow and its secrets,
repository conventions, and plugin health. The dialogue is detect → offer →
fix → report: it never changes anything you haven't agreed to, and it leaves
you only the steps a human must do (pasting credentials). Run it whenever
something seems missing — running it on a healthy project is how you check.

Creating a repository is **repo-first**: Epik builds it through the GitHub API
and never needs a clone, because Epik needs GitHub, not your hard drive. What
it produces is self-installing — a collaborator clones, trusts the folder,
accepts the install prompt, and is enrolled. That clone + trust is the
enrollment ritual for joining a project as a terminal builder; operating Epik
never requires one.

On Desktop/CoWork the same dialogue is the **init-epik** prompt (also served
under its older name, **setup-epik**).

## Troubleshooting the bootstrap

`/epik:init` handles problems that arise once Epik can speak. Only failures
*of the bootstrap itself* need this section:

**`/plugin marketplace add` fails with `JSON Parse error`**
Claude Code's own marketplace registry is corrupt (often a stray trailing
comma). Check `python3 -m json.tool ~/.claude/plugins/known_marketplaces.json`
and fix the syntax it points at. If it says the name `epik` is already in
use, `/plugin marketplace remove epik` and add it again.

**Desktop/CoWork: the EpikMCP server never appears**
Almost always a path problem: use the absolute `uvx` path in `command`, set
`env.PATH` as shown above, relaunch the app, and start a fresh conversation.
The desktop app's MCP logs are in `~/Library/Logs/Claude/` on macOS.
