# Epik

_You say it. We make it._

Epik is a software-design partner that turns a conversation into working code.
Talk through what you want to build; Epik writes the plan up as GitHub issues,
then starts builds that write the code, commit it, and open a pull request for
you to review. There is nothing to babysit: the work runs on GitHub, and you
check in whenever you like.

## Layout

- [`mcp/`](mcp/README.md) — **EpikMCP**, the GitHub mechanism: an MCP server
  that authors the issue graph, reads status, and carries Epik's prompts.
- [`plugin/`](plugin/README.md) — the **epik** Claude Code plugin, the policy
  layer: skills, hooks, and the declaration of the EpikMCP server.
- [`website/`](website/README.md) — the static Epik website, deployed to
  GitHub Pages on every push to `main` that passes CI.

## Installing and uninstalling

Installing Epik is all that has to happen before Epik can speak. Everything
after that — checking your GitHub sign-in, setting a repository up, configuring
builds — Epik does itself, in conversation.

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

To update later, install the newer `.mcpb` the same way.

(No release published yet? Until the first one lands, the bundle can be built
from this repository — see [`mcp/mcpb/`](mcp/mcpb/README.md).)

### Install in Claude Code

You need a GitHub account, [Claude Code](https://claude.com/claude-code)
(current version), the [`gh` CLI](https://cli.github.com/) signed in
(`gh auth login`), and [`uv`](https://docs.astral.sh/uv/) — its `uvx` command
runs the EpikMCP server for you (plus `git`, which you almost certainly
already have). Then, inside a Claude Code session:

```
/plugin marketplace add epik-agent/Epik
/plugin install epik@epik
/reload-plugins
```

(`epik-agent/Epik` is this repository; the doubled `epik@epik` is
`plugin@marketplace`, both named in [`.claude-plugin/marketplace.json`](.claude-plugin/marketplace.json).)

To update later: `/plugin marketplace update epik` then `/reload-plugins`.

### Last step: summon Epik

You know Epik is installed because you can summon it.

| Surface | Summon Epik |
|---|---|
| Claude Desktop / Cowork | Click the **Summon Epik** prompt bubble above the message box, then send |
| Claude Code (CLI, desktop app, web, IDE) | `/epik:summon` |

Epik introduces itself: *"Hello, I'm Epik."* If it didn't say hello, you
aren't talking to Epik.

From there, everything is dialogue. One command is worth knowing:
`/epik:init-repository` sets a repository up for Epik, or repairs one that has
drifted — safe to run any time, and running it on a healthy project is how you
check. (In Claude Desktop and Cowork it is the **Set up a repository for Epik**
prompt.)

### Uninstalling

| Surface | Uninstall |
|---|---|
| Claude Desktop / Cowork | Settings → Extensions → Epik → uninstall |
| Claude Code | `/plugin uninstall epik@epik`, then `/plugin marketplace remove epik` |

`~/.epik-mcp/` is only a cache; delete it whenever you like and Epik will
rebuild it. Uninstalling never touches your own settings in `~/.epik/`.

**Uninstalling Epik from a machine does not remove Epik from a project.** A
repository Epik has set up keeps its settings, its build workflow, and its
secrets, and it will offer to install Epik for the next person who clones it
and trusts the folder. That is intended: what got set up is the project, not
your laptop.

## How it works

**The work happens on GitHub, not on your computer.** When you launch a build,
Epik starts a GitHub Action — a program GitHub runs on its own computers.
Nothing runs on your machine, and nothing depends on your chat staying open.
Your laptop can go to sleep, run out of battery, or be closed for the night;
the build carries on without you.

**GitHub is the dashboard.** There is no separate Epik app, website, or status
page to check. Everything Epik does shows up on GitHub as it happens:

- progress comments on the issue,
- commits on a branch,
- a pull request when the work is ready for a look,
- and the Action's live log, for anyone who wants the play-by-play.

Watching Epik work *means* looking at GitHub — from any browser, phone
included. When you ask Epik for a status in chat, it is reading that same
GitHub state back to you; the issue and the pull request are always the source
of truth.

**There is nothing to reconnect to.** Because the build was never attached to a
session of yours, there is no lost connection to recover. To catch up after
being away, open the issue or the pull request. The one thing Epik always
leaves for a human is the review: the pull request is where you decide what
merges.

While a build runs, you can close the chat, start another conversation, follow
along on your phone, or ignore it entirely until the pull request arrives.

## Troubleshooting the install

Epik handles problems that arise once it can speak. Only failures *of the
install itself* need this section:

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
