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

The checklist opens by itself until setup is complete, and whenever something a build needs has gone missing. **Check setup…** in Epik's tray menu opens it any time. Epik also checks before every build, and a build that would fail for want of a tool or a sign-in fails at once and says which.

## Planning

The user creates the GitHub repository themselves. In a Claude Code session, you help them settle a design and write it up as an epic issue with sub-issues, using `gh` and Epik's own tools.

## Builds

To build, the user asks you to build an issue or an epic. You hand the build to the Epik app, which starts if it isn't running. The app clones the repository under `~/.epik/builds/` and runs one Claude Code agent per issue. An issue with sub-issues is built as an epic, its sub-issues in dependency order. The machine must stay awake until the build finishes, unless it builds on another machine (below). GitHub is where the results land: branches, pull requests, CI runs, and closed issues.

Builds never merge into the default branch. A single issue's build stops at an open pull request into it. For an epic, the app merges each issue's pull request into the epic branch, then opens the epic's pull request into the default branch, which waits for the user. The pull request is where the user decides what merges.

## Building on another machine

A build on the user's computer stops when they close the lid. To build while it's closed, they build on a **host**: another machine they reach with `ssh`, such as a server, a cloud VM or a desktop at home, running the Epik app without its windows. The build runs entirely there, and the app on their computer shows it as it goes and catches up when the lid opens. Hosts run Linux, on x86-64 or ARM64. Epik reaches a host by running `ssh <host>`, with the host named as the user would type it (`devbox`, `me@build.example.com`), so their `~/.ssh/config`, keys and jump hosts work. Epik holds no keys or passwords and opens no ports.

Setting up a host is done by hand:

1. **From their computer, `ssh <host>` once by hand.** Epik never answers a password or a host-key question, so the host key must already be known and the login must not ask for a password (`ssh-copy-id <host>` sets up a key). Everything after this is done on the host, over `ssh`.
2. **Install Epik.** Download the app and its checksum, check it, and put the binary at `~/.epik/bin/epik-app`. Use `linux-arm64` in place of `linux-x64` on ARM64. The host must run the same minor version as their computer; for a particular version, use `releases/download/v0.4.0/` in place of `releases/latest/download/`.

   ```sh
   curl -fsSLO https://github.com/epik-agent/Epik/releases/latest/download/epik-app-linux-x64.tar.gz
   curl -fsSLO https://github.com/epik-agent/Epik/releases/latest/download/epik-app-linux-x64.tar.gz.sha256
   sha256sum -c epik-app-linux-x64.tar.gz.sha256
   mkdir -p ~/.epik/bin
   tar -xzf epik-app-linux-x64.tar.gz -C ~/.epik/bin
   ~/.epik/bin/epik-app --version
   ```

3. **Install the tools.** `~/.epik/bin/epik-app setup` checks what a build needs and says what to run for each item not done. `epik-app setup install gh` and `epik-app setup install node` install private copies in `~/.epik/tools/`, checked against their published checksums. Builds run Claude Code, so if it's missing: `curl -fsSL https://claude.ai/install.sh | bash`. The host doesn't need the Epik plugin; builds bring their own.
4. **Sign in.** `gh auth login -s project` (a one-time code to enter in the browser) and `gh auth setup-git`; where Epik installed its own `gh`, `epik-app setup` prints the full path to type. Then each agent the roles use, since a build on a host runs with the roles in the user's own Settings: `claude auth login` for Claude Code (a code to paste back). `epik-app setup` succeeds once everything a build needs is done.
5. **Run it as a `systemd --user` service.** Save this as `~/.config/systemd/user/epik.service`:

   ```ini
   [Unit]
   Description=Epik, building on this machine
   After=network-online.target
   Wants=network-online.target

   [Service]
   ExecStart=%h/.epik/bin/epik-app --headless
   Restart=on-failure
   RestartSec=5

   [Install]
   WantedBy=default.target
   ```

   Then `systemctl --user daemon-reload`, `systemctl --user enable --now epik`, and `loginctl enable-linger "$USER"` so it keeps running after they log out. If `enable-linger` asks for a password, an administrator runs `sudo loginctl enable-linger <user>`.
6. **Add the host in Settings.** In the app on their computer, **Settings…** has a **Hosts** section: add the host as typed after `ssh`, and it says whether the host is reachable and whether its setup is complete. One host is the default for builds, **This computer** unless the user picks another, and the user can name one for a build: "build #12 on devbox".

Wherever a run appears, it says which host it builds on. **Rerun** and **Delete** go to the host, which owns the run; the user's computer keeps a copy of its record under `~/.epik/hosts/<host>/runs/`.

When a host isn't working, Settings says why:

- **Unreachable:** it shows `ssh`'s own message, and when the host was last heard from. Check that `ssh <host>` works from their computer without a password or a host-key question, then that the service is running on the host: `systemctl --user status epik` and `journalctl --user -u epik`.
- **A version mismatch:** "devbox runs Epik 0.3.2; install 0.4.x there". They download that release as in step 2, replace `~/.epik/bin/epik-app`, and run `systemctl --user restart epik`.
- **Missing setup:** it lists what is missing on the host, with the command to run there over `ssh`. `~/.epik/bin/epik-app setup` on the host shows everything.

## The app

The app appears in the menu bar (macOS), the notification area (Windows) or the system tray (Linux). Clicking its icon opens Epik's tray menu: whether Epik is running, each active run with its progress, and **Open Dashboard**, **Restart**, **Settings…**, **Check setup…**, **Check for updates…** (which opens Epik's releases on GitHub) and **Quit Epik**. **Open Dashboard** opens its window, which lists every run and shows each one's issues live, with what each agent is doing. On Linux, where tray clicks aren't reported, the tray has a native menu of the same items, without the runs, and Epik opens its window when it starts, since not every desktop shows tray icons.

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

**Start with Check setup.** For any install problem, have the user choose **Check setup…** in Epik's tray menu. The checklist says what is missing or out of date, and offers to fix it.

**Installing the plugin fails with `JSON Parse error`.** Claude Code's marketplace registry is corrupt, often from a stray trailing comma. Have the user check `jq . ~/.claude/plugins/known_marketplaces.json` and fix the syntax it points at. If it says the name `epik` is already in use, they run `claude plugin marketplace remove epik` and press the checklist's button again.

**A build's agent stops at once: it "can't run in auto mode".** The Epik app runs its agents in Claude Code's auto mode, so they can work without anyone answering prompts. When auto mode isn't available, the app stops the agent rather than let it stall. Auto mode needs a recent model, chosen with `/model`. On a Team or Enterprise plan, an admin can turn auto mode off for the organization. Claude Code's [permission modes documentation](https://code.claude.com/docs/en/permission-modes) lists the models that support it.

**On Windows, Epik's tools don't appear in Claude Code.** The plugin runs Epik's tools as `${HOME}/.epik/bin/epik-app mcp`. Claude Code expands `${HOME}` on macOS; that it does on Windows is not yet checked. If it doesn't, this is a known limitation, to be fixed in a later release.
