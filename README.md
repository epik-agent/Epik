# Epik

_You say it. We make it._

Epik is a software-design partner that turns a conversation into working code.
Talk through what you want to build; Epik writes the plan up as GitHub issues,
then Claude Code builds them on your machine: it writes the code, commits it,
and opens a pull request for you to review.

## Layout

- [`plugin/`](plugin/README.md) — the **epik** plugin: skills, and the Node
  server that runs Epik's GitHub operations through the `gh` CLI.
- [`website/`](website/README.md) — the static Epik website, deployed to
  GitHub Pages on every push to `main` that passes CI.

## Installing and uninstalling

Epik is one plugin for the Claude Code terminal, installed from this
repository's marketplace.

You need:

- A GitHub account.
- [Node](https://nodejs.org/) 26 or later, which runs Epik's server.
- The [`gh` CLI](https://cli.github.com/), signed in with
  `gh auth login -s project`, with `gh auth setup-git` run so builds can push.
  That is how Epik acts on GitHub as you.
- [Claude Code](https://claude.com/claude-code), installed and signed in
  with your claude.ai account, on the machine that runs Epik. You talk to Epik
  in it, and Epik runs every build with it.

### Install

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

To update later, again one command at a time:

```
/plugin marketplace update epik
```

```
/reload-plugins
```

### Uninstalling

Run `/plugin uninstall epik@epik`, then
`/plugin marketplace remove epik`.

## How it works

You create the GitHub repository yourself. In a Claude Code session, Epik
helps you settle a design and writes it up as an epic issue with sub-issues,
using `gh` and Epik's own tools.

To build, ask Epik to build an issue or an epic. Epik starts Claude Code on
your machine, in a fresh clone under `~/.epik/builds/`, and tells you where
the build's log is; follow it with `tail -f`. An issue with sub-issues is
built as an epic, its sub-issues in dependency order. Keep the machine awake
until the build finishes. GitHub is where the results land — branches, pull
requests, CI runs, and closed issues.

Builds never merge into the default branch. A single issue's build stops at
an open pull request into it. An epic's issues merge into the epic branch,
whose pull request into the default branch waits for you. The pull request is
where you decide what merges.

## Troubleshooting the install

**`/plugin marketplace add` fails with `JSON Parse error`**
Claude Code's marketplace registry is corrupt (often a stray trailing
comma). Check `jq . ~/.claude/plugins/known_marketplaces.json` and fix the
syntax it points at. If it says the name `epik` is already in use,
`/plugin marketplace remove epik` and add it again.
