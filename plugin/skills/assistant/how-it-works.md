# How Epik works

Read this when the user asks how Epik works, how builds run, where a build's records are, or for help with a problem. Answer from it in your own words, and only as much as the question needs.

## Setup

Epik is the Epik app, which runs builds on the user's machine and serves Epik's tools, and a plugin for Claude Code, which the user talks to Epik in: in the terminal or in Claude Desktop's Code tab.

When the user opens the app, its **Set up Epik** checklist walks them through:

- **The GitHub CLI (`gh`):** uses the one on their computer, or installs a private copy for Epik.
- **GitHub sign-in:** signs `gh` in, in their browser, with the `project` scope Epik needs, and lets `git` push with it. That is how Epik acts on GitHub as the user.
- **Claude Code:** runs Anthropic's installer if it isn't installed.
- **Claude sign-in:** signs Claude Code in, in their browser.
- **Codex:** downloads OpenAI's Codex with the adapter Epik runs it through.
- **Codex sign-in:** signs Codex in, in their browser, with ChatGPT or an API key.
- **Node:** uses Node 26 or later from their computer, or installs a private copy. It runs the app's coding agents.
- **The Epik plugin:** adds Epik's marketplace to Claude Code and installs the plugin for the user.

The Claude Code and Codex items are needed only while a role uses that agent (see **Roles**): with the defaults, that is Claude Code alone, and an agent no role uses shows as not needed. Claude Code is still where the user plans with Epik, so they keep it to talk to you.

Each step waits until the user presses its button, and says what it will do first. Private copies go in `~/.epik/tools/`, checked against their published checksums; nothing is installed system-wide, and nothing asks for an administrator's password.

The checklist opens by itself until setup is complete, and whenever something a build needs has gone missing. **Check setup…** in Epik's tray menu opens it any time. Epik also checks before every build, and a build that would fail for want of a tool or a sign-in fails at once and says which.

## Planning

The user creates the GitHub repository themselves. In a Claude Code session, you help them settle a design and write it up as an epic issue with sub-issues, using `gh` and Epik's own tools.

## Builds

To build, the user asks you to build an issue or an epic. You hand the build to the Epik app, which starts if it isn't running. The app clones the repository under `~/.epik/builds/` and runs one coding agent per issue, with the agent and model the user chose for coding (see **Roles**). Each agent is told the issue, Epik's rules and the procedure for its work in its prompt, so it loads no plugin. An issue with sub-issues is built as an epic, its sub-issues in dependency order. The machine must stay awake until the build finishes, unless it builds on another machine (below). GitHub is where the results land: branches, pull requests, CI runs, and closed issues.

Builds never merge into the default branch. A single issue's build stops at an open pull request into it. For an epic, the app merges each issue's pull request into the epic branch, then opens the epic's pull request into the default branch, which waits for the user. The pull request is where the user decides what merges. With review on, an agent reviews that pull request first (see **Review**).

## Roles

Epik's work comes in kinds, called **roles**, and the user chooses an agent and a model for each in the app's **Settings…**, under **Agents**:

- **Design:** planning with you, in this conversation. It runs in whatever client the user talks to Epik in, not in the app, so Settings doesn't choose it.
- **Coding:** implementing one issue, up to a pull request with passing checks. Claude Code with its default model, unless the user changes it.
- **Review:** reading the build's pull request against its issue and Epik's rules when the build is done, before it reaches the user. **Off** by default, since it adds an agent's time and cost to every run.

Epik can run two agents:

- **Claude Code**, Anthropic's. It runs unattended in its auto mode, where a model approves each risky action, with no sandbox around the rest.
- **Codex**, OpenAI's, which suits a user with no Anthropic account. It runs unattended in its `agent` mode: its work runs in a sandbox with no network, and Codex's own reviewer approves what leaves it, such as `git push` and `gh`. Those approvals happen out of Epik's sight, but are in the agent's log as "Guardian Review" tool calls. The app runs the Codex that its adapter, `@agentclientprotocol/codex-acp`, brings, at a version Epik pins, and shares its sign-in in `~/.codex` with any Codex the user runs themselves.

The model is optional: left empty, a role runs the agent's default. Otherwise it is the agent's own name for a model, such as `opus` for Claude Code. Settings are kept in `~/.epik/settings.json`, and builds that start after **Save** use them. Before a build starts, the app opens a session with each role's agent, without a prompt, to check that it runs unattended and offers the model; if it doesn't, the build is refused with an error listing the models it does offer, and nothing is recorded. A build records the roles it ran with, and **Rerun** uses them again, whatever Settings say now. A build on a host runs with the roles in the user's own Settings, not the host's.

To use Codex, the user chooses it for a role in Settings, then **Check setup…**: the checklist downloads Codex and signs it in. To build without Claude Code at all, both coding and, if it is on, review must use Codex.

## Review

With review on, the review agent reviews the run's pull request once it exists and its checks pass: a single issue's pull request, or for an epic, the epic's pull request into the default branch. An epic's issues still merge into the epic branch as they finish; the epic never pauses for a review in the middle.

The reviewer reads the issue (for an epic, the epic and its sub-issues) and the pull request, and posts one GitHub review on the pull request, as the user reads any review: an inline comment on each line it questions, each the start of a thread, and a summary for concerns with no line. Because every agent acts on GitHub as the user, and GitHub won't let anyone approve or request changes on their own pull request, the review only comments. Its verdict is the last line of its summary, a marker hidden in GitHub's rendering, naming the round it answers: `<!-- epik-review: approve round=1 -->` or `<!-- epik-review: changes round=1 -->`. The app reads the verdict from GitHub, not from what the agent says.

- **Approve:** the build finishes, and the pull request waits for the user.
- **Changes:** a coding agent works on the pull request's branch. For each open thread it fixes what the thread asks, replies saying what it did and resolves the thread; a thread it thinks is wrong it answers and leaves open. It pushes, and once the checks pass the reviewer reviews again, reading the threads before the diff. After two rounds of changes the build stops anyway: the pull request waits for the user with its reviews and any threads still open, and the run needs attention.
- **No verdict:** a review that leaves no marker for its round is recorded as none, and the pull request waits for the user.

The run's page shows its reviews under its header, each with its verdict, and how many threads were left open.

## Cost

Epik records what each agent reports it spent, and keeps no price table of its own. Claude Code reports cost; Codex doesn't, so a Codex session has its time recorded and its cost shown as "not reported by Codex", never as $0.00. A total that includes unreported costs says so: "$0.92 plus some not reported by Codex" for one issue, or "$2.06 plus 1 issue not reported" for a run. The comment Epik leaves on each issue lists its agent sessions, with each one's role, round, agent, model, time and cost.

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

3. **Install the tools.** `~/.epik/bin/epik-app setup` checks what a build needs and says what to run for each item not done. `epik-app setup install gh` and `epik-app setup install node` install private copies in `~/.epik/tools/`, checked against their published checksums. If a role uses Claude Code and it's missing: `curl -fsSL https://claude.ai/install.sh | bash`. If a role uses Codex, `epik-app setup` prints the command that downloads it. The host doesn't need the Epik plugin; builds bring their own.
4. **Sign in.** `gh auth login -s project` (a one-time code to enter in the browser) and `gh auth setup-git`; where Epik installed its own `gh`, `epik-app setup` prints the full path to type. Then each agent the roles use, since a build on a host runs with the roles in the user's own Settings: `claude auth login` for Claude Code (a code to paste back), and for Codex the sign-in command `epik-app setup` prints. Codex's sign-in opens a browser, which a host reached over `ssh` doesn't have; instead, sign Codex in on their own computer and copy `~/.codex/auth.json` to the same place on the host. `epik-app setup` succeeds once everything a build needs is done.
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
- each agent session's log and its standard error: `<issue>.jsonl` and `<issue>.err` for an issue's coding, and `<issue>-review-<round>.jsonl` or `<issue>-coding-<round>.jsonl` for a review and the coding that answers it;
- `summary.json`: the build's sessions, roles, cost and duration, once it ends.

Each **agent session** is recorded with its role, round, agent, model, time and cost, and the issue's and run's time and cost are their sums.

A build's clone is temporary: the app removes it, and the worktrees its agents made, when the build ends, and removes any left over when it starts. The record stays. To say how a build is going, use `build_status`.

## Claude Desktop

Desktop's Code tab loads user-scope plugins, so once the setup checklist has installed the plugin, it has Epik too. Epik works in Desktop's **Code** tab with the **My Computer** environment selected. The Code tab's cloud environment doesn't load Epik, because cloud sessions don't have locally installed plugins. Desktop's chat doesn't run Epik either.

## Troubleshooting

**A chat seems hung.** It usually isn't. Claude asks the user to approve Epik's actions as it works; that is intended, since Epik never acts without the user's say-so. Tell the user to look for an approval prompt waiting on their screen.

**Start with Check setup.** For any install problem, have the user choose **Check setup…** in Epik's tray menu. The checklist says what is missing or out of date, and offers to fix it.

**Installing the plugin fails with `JSON Parse error`.** Claude Code's marketplace registry is corrupt, often from a stray trailing comma. Have the user check `jq . ~/.claude/plugins/known_marketplaces.json` and fix the syntax it points at. If it says the name `epik` is already in use, they run `claude plugin marketplace remove epik` and press the checklist's button again.

**A build is refused: Claude Code "can't run unattended".** The Epik app runs Claude Code in its auto mode, so it can work without anyone answering prompts. When auto mode isn't available, the app refuses the build rather than let the agent stall. Auto mode needs a recent model, chosen with `/model`. On a Team or Enterprise plan, an admin can turn auto mode off for the organization. Claude Code's [permission modes documentation](https://code.claude.com/docs/en/permission-modes) lists the models that support it.

**A build is refused: an agent "doesn't offer the model".** The model in Settings is one the role's agent doesn't offer, or doesn't offer to this account. The error lists the models it does offer, such as "Claude Code doesn't offer the model `gpt-5`; its models are: sonnet, opus": a model belongs to its agent, so a Codex model needs Codex chosen for the role. Have the user choose one from the list in **Settings…**, or clear the model to use the agent's default, and build again. For a build on a host, the check runs on the host, with the agent signed in there.

**A build is refused because Codex or its sign-in is missing.** A role uses Codex and its setup isn't done on the machine building. Settings says "Its setup isn't done" beside Codex; **Check setup…** downloads Codex and signs it in. On a host, run `~/.epik/bin/epik-app setup` there.

**The review keeps asking for changes.** After two rounds of changes the build stops, and the run needs attention: "the review still asks for changes after 2 rounds". Nothing is lost: the pull request waits with its reviews, and the threads still open are where the reviewer and the coding agent disagree, or what the coding agent couldn't fix. Have the user read those threads on GitHub and decide, as with any review: fix what is right, answer and resolve what isn't, and merge when they are satisfied. The reviewer can be wrong. If it is often wrong, a different review model, or review turned off, may serve them better.

**A cost says "not reported".** Codex doesn't report what it spends, so its sessions have a time but no cost, and totals say how much is missing rather than count it as zero. Its usage is in the user's OpenAI account.

**On Windows, Epik's tools don't appear in Claude Code.** The plugin runs Epik's tools as `${HOME}/.epik/bin/epik-app mcp`. Claude Code expands `${HOME}` on macOS; that it does on Windows is not yet checked. If it doesn't, this is a known limitation, to be fixed in a later release.
