# Epik

_You say it. We make it._

Epik is a software-design partner that turns a conversation into working code.
Talk through what you want to build; Epik writes the plan up as GitHub issues,
then Claude Code builds them on your machine: it writes the code, commits it,
and opens a pull request for you to review.

## Layout

- [`mcp/`](mcp/README.md) — **EpikMCP**, the GitHub mechanism: an MCP server
  that authors the issue graph and reads status.
- [`plugin/`](plugin/README.md) — the **epik** Claude Code plugin, the policy
  layer: skills and the declaration of the EpikMCP server.
- [`website/`](website/README.md) — the static Epik website, deployed to
  GitHub Pages on every push to `main` that passes CI.

## Installing and uninstalling

### Install in Claude Desktop or Cowork

Epik installs as a one-click extension. There is no config file to edit and no
Python or command-line tooling to set up.

You need a GitHub account, a current version of
[Claude Desktop](https://claude.com/download), and the
[`gh` CLI](https://cli.github.com/) signed in (`gh auth login`) — that is how
Epik acts on GitHub as you.

1. Download the `.mcpb` file from the
   [latest release](https://github.com/epik-agent/Epik/releases/latest).
2. Double-click it, or drag it onto the Claude Desktop window. The extension
   installs itself.
3. Start a **new** conversation.

Once Epik is installed, Claude Desktop and Cowork ask you to approve Epik's
actions as it works. That is intended — Epik never acts without your say-so.
If a chat ever seems hung, it usually isn't: look for an approval prompt
waiting on your screen.

To update later, install the newer `.mcpb` the same way.

(No release published yet? Until the first one lands, the bundle can be built
from this repository — see [`mcp/mcpb/`](mcp/mcpb/README.md).)

### Install in Claude Code

You need a GitHub account, [Claude Code](https://claude.com/claude-code)
(current version), the [`gh` CLI](https://cli.github.com/) signed in
(`gh auth login`), and [`uv`](https://docs.astral.sh/uv/) — its `uvx` command
runs the EpikMCP server for you (plus `git`, which you almost certainly
already have). Then, inside a Claude Code session, enter the following
commands one at a time:

```
/plugin marketplace add epik-agent/Epik
```

```
/plugin install epik@epik
```

The install command opens a details view asking you to confirm an
installation scope; confirm it, then reload:

```
/reload-plugins
```

(`epik-agent/Epik` is this repository; the doubled `epik@epik` is
`plugin@marketplace`, both named in [`.claude-plugin/marketplace.json`](.claude-plugin/marketplace.json).)

As Epik works, Claude Code asks you to approve its actions. That is intended —
Epik never acts without your say-so. If the chat seems hung, it usually isn't:
look for an approval prompt waiting on your screen.

To update later, again one command at a time:

```
/plugin marketplace update epik
```

```
/reload-plugins
```

### Uninstalling

| Surface | Uninstall |
|---|---|
| Claude Desktop / Cowork | Settings → Extensions → Epik → uninstall |
| Claude Code | `/plugin uninstall epik@epik`, then `/plugin marketplace remove epik` |

`~/.epik-mcp/` is only a cache; delete it whenever you like and Epik will
rebuild it.

## How it works

You create the GitHub repository yourself. In conversation (Claude Desktop,
Cowork, or Claude Code), Epik helps you settle a design and writes it up as an
epic issue with sub-issues, using the EpikMCP tools.

To build, run Claude Code in a local clone and invoke `/epik:epic` for an
epic or `/epik:issue` for a single issue. The code is written on your
machine; GitHub is where the results land — branches, pull requests, CI runs,
and closed issues. The pull request is where you decide what merges.

## Troubleshooting the install

**Claude Code: `/plugin marketplace add` fails with `JSON Parse error`**
Claude Code's own marketplace registry is corrupt (often a stray trailing
comma). Check `python3 -m json.tool ~/.claude/plugins/known_marketplaces.json`
and fix the syntax it points at. If it says the name `epik` is already in
use, `/plugin marketplace remove epik` and add it again.

**Claude Desktop: Epik never appears after installing the extension**
Check Settings → Extensions to confirm the extension is enabled, then start a
fresh conversation — extensions are picked up when a conversation begins. If it
still doesn't appear, the desktop app's logs are in `~/Library/Logs/Claude/` on
macOS.
