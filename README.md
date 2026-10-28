# Epik

_You say it. We make it._

Epik is a software-design partner that turns a conversation into working code.
Talk through what you want to build; Epik writes the plan up as GitHub issues,
then Claude Code builds them on your machine: it writes the code, commits it,
and opens a pull request for you to review.

## Layout

- [`plugin/`](plugin/README.md) — the **epik** plugin: skills, and the Node
  server that provides the GitHub operations the standard GitHub connector
  lacks.
- [`website/`](website/README.md) — the static Epik website, deployed to
  GitHub Pages on every push to `main` that passes CI.

## Installing and uninstalling

Epik is one plugin, installed the same way in Claude Desktop and Claude Code:
from this repository's marketplace.

You need:

- A GitHub account.
- [Node](https://nodejs.org/) 26 or later, which runs Epik's server.
- The [`gh` CLI](https://cli.github.com/), signed in with `gh auth login`.
  That is how Epik acts on GitHub as you.
- For Claude Desktop: a current version of
  [Claude Desktop](https://claude.com/download), running, with the GitHub
  connector connected.
- [Claude Code](https://claude.com/claude-code), installed and signed in
  with your claude.ai account, on the machine that runs Epik. Epik runs every
  build with it, including builds you ask for in Claude Desktop.

### Install

In Claude Desktop, add the marketplace `epik-agent/Epik` in the app's plugin
settings, install the `epik` plugin from it, and start a **new**
conversation.

In Claude Code, enter the following commands one at a time:

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

Once Epik is installed, Claude asks you to approve Epik's actions as it works.
That is intended — Epik never acts without your say-so. If a chat ever seems
hung, it usually isn't: look for an approval prompt waiting on your screen.

To update later in Claude Code, again one command at a time:

```
/plugin marketplace update epik
```

```
/reload-plugins
```

### Uninstalling

In Claude Desktop, uninstall the `epik` plugin and remove the `epik`
marketplace in the app's plugin settings.

In Claude Code, `/plugin uninstall epik@epik`, then
`/plugin marketplace remove epik`.

## How it works

You create the GitHub repository yourself. In conversation (Claude Desktop,
Cowork, or Claude Code), Epik helps you settle a design and writes it up as an
epic issue with sub-issues, using the GitHub connector and Epik's own tools.

To build, ask Epik to build an issue or an epic. Epik starts Claude Code on
your machine, in a fresh clone under `~/.epik/builds/`, and tells you where
the build's log is; follow it with `tail -f`. An issue with sub-issues is
built as an epic, its sub-issues in dependency order. Keep the machine awake
until the build finishes. GitHub is where the results land — branches, pull
requests, CI runs, and closed issues. The pull request is where you decide
what merges.

## Troubleshooting the install

**Claude Code: `/plugin marketplace add` fails with `JSON Parse error`**
Claude Code's own marketplace registry is corrupt (often a stray trailing
comma). Check `jq . ~/.claude/plugins/known_marketplaces.json` and fix the
syntax it points at. If it says the name `epik` is already in use,
`/plugin marketplace remove epik` and add it again.

**Claude Desktop: Epik's tools never appear after installing the plugin**
Check that `node --version` reports 26 or later, then start a fresh
conversation — plugins are picked up when a conversation begins. If the tools
still don't appear, the desktop app's logs are in `~/Library/Logs/Claude/` on
macOS.
