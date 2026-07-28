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

| Surface | Gesture |
|---|---|
| Claude Code (CLI, desktop app, web, IDE) | `/epik:design` |
| Claude Desktop / CoWork | **+** (attach) → **summon-epik** → send |

Epik introduces itself: *"Hello, I'm Epik."* If it didn't say hello, you
aren't talking to Epik.

From there, everything is dialogue. In particular, Epik can check and repair
its own setup — `gh` authentication, project initialization, the headless
build workflow and its secrets, plugin health — via the **doctor**:
`/epik:doctor` in Claude Code, or the **setup-epik** prompt in
Desktop/CoWork. Summon the doctor whenever something seems missing; it
diagnoses, offers fixes, and leaves you only the steps a human must do
(pasting credentials).

To make a project self-installing for collaborators — clone, trust the
folder, accept the install prompt, done — ask Epik to initialize it
(`/epik:init`). That writes the project's `.claude/settings.json` enablement
and scaffolding; the doctor will offer this too when it finds a project
uninitialized.

## Troubleshooting the bootstrap

Epik's doctor handles problems that arise once Epik can speak. Only failures
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
