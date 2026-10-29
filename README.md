# Epik

_You say it. We make it._

Epik is a software-design partner that turns a conversation into working code.
Talk through what you want to build; Epik writes the plan up as GitHub issues,
then Claude Code builds them on your machine: it writes the code, commits it,
and opens a pull request for you to review.

## Layout

- [`plugin/`](plugin/README.md) — the **epik** plugin: skills and
  configuration. Its tools come from the Epik app.
- [`crates/`](crates) — the **Epik app** (`epik-app`), Epik's only program: it
  runs builds and, as `epik-app mcp`, serves Epik's MCP tools, running its
  GitHub operations through the `gh` CLI. `epik-core` is the library it is
  built on. [`docs/app-protocol.md`](docs/app-protocol.md) is how the MCP
  server talks to the running app.
- [`website/`](website/README.md) — the static Epik website, deployed to
  GitHub Pages on every push to `main` that passes CI.

## Installing and uninstalling

Epik is a plugin for Claude Code, used in the terminal or in Claude
Desktop's Code tab, installed from this repository's marketplace, and the
Epik app, which runs builds on your machine.

You need:

- A GitHub account.
- [Node](https://nodejs.org/) 26 or later, which runs the app's coding
  agents.
- [Rust](https://www.rust-lang.org/), stable, installed with
  [rustup](https://rustup.rs/), which builds the Epik app.
- The [`gh` CLI](https://cli.github.com/), signed in with
  `gh auth login -s project`, with `gh auth setup-git` run so builds can push.
  Already signed in to `gh`? Run `gh auth refresh -s project` instead of
  logging in again. That is how Epik acts on GitHub as you.
- [Claude Code](https://claude.com/claude-code), installed and signed in
  with your claude.ai account, on the machine that runs Epik. You talk to Epik
  in it, and Epik runs every build with it.

### Install the Epik app

In a terminal:

```
cargo install --git https://github.com/epik-agent/Epik epik-app
```

That puts `epik-app` in `~/.cargo/bin`, which rustup adds to your `PATH`.
You don't need to start it: Epik starts it the first time you build. Run it
once, though (`epik-app --version` will do): each time it runs it links
itself at `~/.epik/bin/epik-app`, which is where the plugin finds Epik's
tools. On
Linux, the app needs the system packages Tauri needs; see Tauri's
[Linux prerequisites](https://v2.tauri.app/start/prerequisites/#linux).
Where there is no tray, such as a server without a desktop, the app runs
without one and builds the same.

To update the app later, run the same command again.

### Install the plugin

In Claude Code, enter the following commands one at a time:

```
/plugin marketplace add epik-agent/Epik
```

```
/plugin install epik@epik
```

The install command opens a details view asking you to choose an
installation scope. Choose **user**, so Epik loads in every folder you start
Claude Code from, then reload:

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

### Claude Desktop

Install Epik as above, in a Claude Code terminal. Desktop's Code tab loads
user-scope plugins, so it then has Epik too. Use Epik in Desktop's **Code**
tab with the **My Computer** environment selected. The Code tab's cloud
environment doesn't load Epik, because cloud sessions don't have locally
installed plugins. Desktop's chat doesn't run Epik either.

### Uninstalling

Run `/plugin uninstall epik@epik`, then
`/plugin marketplace remove epik`. Quit the Epik app from its menu, then run
`cargo uninstall epik-app`.

## How it works

You create the GitHub repository yourself. In a Claude Code session, Epik
helps you settle a design and writes it up as an epic issue with sub-issues,
using `gh` and Epik's own tools.

To build, ask Epik to build an issue or an epic. Epik hands the build to the
Epik app, starting the app if it isn't running. The app clones the
repository under `~/.epik/builds/` and runs one Claude Code agent per issue.
An issue with sub-issues is built as an epic, its sub-issues in dependency
order. Keep the machine awake until the build finishes. GitHub is where the
results land — branches, pull requests, CI runs, and closed issues.

The app appears in the menu bar (macOS), the notification area (Windows) or
the system tray (Linux). Click its icon for a summary of what's building and
what's waiting for you; **Open Epik** opens its window, which lists every run
and shows each one's issues live, with what each agent is doing. On Linux,
where tray clicks aren't reported, its menu opens the window. A **run** is the work of finishing one
epic or issue, and each attempt at it is a **build**: building an epic
again after a failure or an interruption adds a build to the same run, and
the run's time and cost add up across its builds. A run's record is in
`~/.epik/runs/<owner>/<name>/<number>/`, with a directory per build holding
`events.jsonl`, what happened in the build; each issue's agent log,
`<issue>.jsonl` and `<issue>.err`; and `summary.json`, with the build's cost
and duration, once it ends. You can also ask Epik how a build is going. A
build's clone is temporary: the app removes it, and the worktrees its
agents made, when the build ends, and removes any left over when it starts.
The record stays.

Builds never merge into the default branch. A single issue's build stops at
an open pull request into it. For an epic, the app merges each issue's pull
request into the epic branch, then opens the epic's pull request into the
default branch, which waits for you. The pull request is where you decide
what merges.

To learn more about how Epik works, ask Epik itself. Start Claude Code in any
folder and say, "Hello, Epik."

## Troubleshooting the install

**`/plugin marketplace add` fails with `JSON Parse error`**
Claude Code's marketplace registry is corrupt (often a stray trailing
comma). Check `jq . ~/.claude/plugins/known_marketplaces.json` and fix the
syntax it points at. If it says the name `epik` is already in use,
`/plugin marketplace remove epik` and add it again.

**A build's agent stops at once: it "can't run in auto mode"**
The Epik app runs its agents in Claude Code's auto mode, so they can work
without you answering prompts. When auto mode isn't available, the app stops
the agent rather than let it stall.
Auto mode needs a recent model, so choose one with `/model`. On a Team or
Enterprise plan, an admin can turn auto mode off for the organization. Claude
Code's [permission modes documentation](https://code.claude.com/docs/en/permission-modes)
lists the models that support it.

**On Windows, Epik's tools don't appear in Claude Code**
The plugin runs Epik's tools as `${HOME}/.epik/bin/epik-app mcp`. Claude
Code expands `${HOME}` on macOS; that it does on Windows is not yet
checked. If it doesn't, this is a known limitation, to be fixed in a later
release.
