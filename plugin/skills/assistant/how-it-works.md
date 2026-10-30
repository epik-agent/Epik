# How Epik works

Read this when the user asks how Epik works, how builds run, where a build's records are, or for help with a problem. Answer from it in your own words, and only as much as the question needs.

## Setup

Epik is the Epik app, which runs builds on the user's machine and serves Epik's tools, and a plugin for Claude Code, which the user talks to Epik in: in the terminal or in Claude Desktop's Code tab.

When the user opens the app, its **Set up Epik** checklist walks them through:

- **The GitHub CLI (`gh`):** uses the one on their computer, or installs a private copy for Epik.
- **GitHub sign-in:** signs `gh` in, in their browser, with the `project` scope Epik needs, and lets `git` push with it. That is how Epik acts on GitHub as the user.
- **Claude Code:** runs Anthropic's installer if it isn't installed.
- **Claude sign-in:** signs Claude Code in, in their browser.
- **Node:** uses Node 26 or later from their computer, or installs a private copy. It runs the app's coding agents.
- **The Epik plugin:** adds Epik's marketplace to Claude Code and installs the plugin for the user.

Each step waits until the user presses its button, and says what it will do first. Private copies go in `~/.epik/tools/`, checked against their published checksums; nothing is installed system-wide, and nothing asks for an administrator's password.

The checklist opens by itself until setup is complete, and whenever something a build needs has gone missing. **Check setup** in Epik's menu opens it any time. Epik also checks before every build, and a build that would fail for want of a tool or a sign-in fails at once and says which.

## Planning

The user creates the GitHub repository themselves. In a Claude Code session, you help them settle a design and write it up as an epic issue with sub-issues, using `gh` and Epik's own tools.

## Builds

To build, the user asks you to build an issue or an epic. You hand the build to the Epik app, which starts if it isn't running. The app clones the repository under `~/.epik/builds/` and runs one Claude Code agent per issue. An issue with sub-issues is built as an epic, its sub-issues in dependency order. The machine must stay awake until the build finishes. GitHub is where the results land: branches, pull requests, CI runs, and closed issues.

Builds never merge into the default branch. A single issue's build stops at an open pull request into it. For an epic, the app merges each issue's pull request into the epic branch, then opens the epic's pull request into the default branch, which waits for the user. The pull request is where the user decides what merges.

## The app

The app appears in the menu bar (macOS), the notification area (Windows) or the system tray (Linux). Clicking its icon shows a summary of what's building and what's waiting for the user. **Open Epik** opens its window, which lists every run and shows each one's issues live, with what each agent is doing. On Linux, where tray clicks aren't reported, its menu opens the window, and Epik opens its window when it starts, since not every desktop shows tray icons.

## Runs, builds and their records

A **run** is the work of finishing one epic or issue, and each attempt at it is a **build**. Building an epic again after a failure or an interruption adds a build to the same run, and the run's time and cost add up across its builds.

A run's record is in `~/.epik/runs/<owner>/<name>/<number>/`, with a directory per build holding:

- `events.jsonl`: what happened in the build;
- each issue's agent log, `<issue>.jsonl` and `<issue>.err`;
- `summary.json`: the build's cost and duration, once it ends.

A build's clone is temporary: the app removes it, and the worktrees its agents made, when the build ends, and removes any left over when it starts. The record stays. To say how a build is going, use `build_status`.

## Claude Desktop

Desktop's Code tab loads user-scope plugins, so once the setup checklist has installed the plugin, it has Epik too. Epik works in Desktop's **Code** tab with the **My Computer** environment selected. The Code tab's cloud environment doesn't load Epik, because cloud sessions don't have locally installed plugins. Desktop's chat doesn't run Epik either.

## Troubleshooting

**A chat seems hung.** It usually isn't. Claude asks the user to approve Epik's actions as it works; that is intended, since Epik never acts without the user's say-so. Tell the user to look for an approval prompt waiting on their screen.

**Start with Check setup.** For any install problem, have the user choose **Check setup** in Epik's menu. The checklist says what is missing or out of date, and offers to fix it.

**Installing the plugin fails with `JSON Parse error`.** Claude Code's marketplace registry is corrupt, often from a stray trailing comma. Have the user check `jq . ~/.claude/plugins/known_marketplaces.json` and fix the syntax it points at. If it says the name `epik` is already in use, they run `claude plugin marketplace remove epik` and press the checklist's button again.

**A build's agent stops at once: it "can't run in auto mode".** The Epik app runs its agents in Claude Code's auto mode, so they can work without anyone answering prompts. When auto mode isn't available, the app stops the agent rather than let it stall. Auto mode needs a recent model, chosen with `/model`. On a Team or Enterprise plan, an admin can turn auto mode off for the organization. Claude Code's [permission modes documentation](https://code.claude.com/docs/en/permission-modes) lists the models that support it.

**On Windows, Epik's tools don't appear in Claude Code.** The plugin runs Epik's tools as `${HOME}/.epik/bin/epik-app mcp`. Claude Code expands `${HOME}` on macOS; that it does on Windows is not yet checked. If it doesn't, this is a known limitation, to be fixed in a later release.
