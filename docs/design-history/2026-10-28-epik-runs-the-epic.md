# An Epik app runs builds, one agent per issue, and records every run

- Status: Accepted
- Date: 2026-10-28
- Revisits: "A host or monitor application" (Deferred) in
  [local-builds](2026-10-26-local-builds.md), and that entry's
  `/epik:epic` design

## Context

Four goals for the next release share one starting point:

1. **Live feedback:** a graph of the epic whose nodes show what each issue
   is doing.
2. **Remote work:** builds that run on another machine and stream back.
3. **Audits and cleanup:** how long a build took and what it cost, kept;
   old clones and logs removed.
4. **Other coding agents:** OpenAI first, mixed with Claude.

Epik's own code orchestrating an epic has always been the plan. It was
implicit in design discussions, and the Rust implementation that preceded
the current rewrite did it (tag `still-rust`, removed from `main` in
#303). To ship 0.2.0, Epik backed off it and put the orchestration in the
`/epik:epic` skill. This entry returns to the plan.

Today `build_start` clones the repository, starts one detached
`claude -p /epik:epic …`, checks that it started in auto mode, and returns.
From then on the model is the orchestrator: the `epic` skill loops on
`epic_status` and starts a Claude Code subagent per ready issue. Epik's own
code never learns what happens next. GitHub records the outcomes, but
nothing records the process: which issue is running, for how long, at what
cost.

Each of the four goals needs that state. A live graph needs it shown, an
audit needs it kept, a remote host needs it streamed, and a mix of agents
needs it produced by something other than Claude Code subagents. And
something has to hold it while builds run: Epik's MCP server lives only as
long as the Claude Code session that started it, and builds outlive that
session.

### Findings (2026-10-28, from the 11 runs under `~/.epik`)

| Question | Finding |
|---|---|
| Do build logs record cost and duration? | **Yes.** Claude Code's stream-json `result` event has `total_cost_usd`, `duration_ms`, `duration_api_ms` and token `usage`. |
| Is there one `result` per run? | **No.** A session writes one per turn, including turns woken by subagents finishing. `total_cost_usd` is cumulative, so the last `result` is the run's total. |
| Can an epic's cost be split by issue? | **No.** Subagent events carry `parent_tool_use_id`, but `result` costs are per session, and an epic is one session. |
| What do runs cost? | Single issues: $0.31 to $1.76. The two `wpm/epik-test` epic runs: $2.78 and $3.67. |
| Does anything clean up? | **No.** 11 clones take 64 MB in `~/.epik/builds`; 3.7 MB of logs sit in `~/.epik/runs`. |
| Is the build process tied to Claude Code? | **Throughout.** The `claude` binary, `--permission-mode auto` and the init-event check, `--plugin-dir`, stream-json, and subagents for parallelism. |

### Findings (2026-10-28, one issue built through ACP)

`wpm/epik-test#13` was built by `claude-agent-acp` 0.99.0 (Claude Code
2.1.312), driven by a 100-line ACP client, with the plugin passed as
`_meta.claudeCode.options.plugins` and the target branch `acp-probe`.

| Question | Finding |
|---|---|
| Does the adapter load a plugin from a path? | **Yes.** `/epik:issue` appeared in the session's commands and ran. |
| Can it run unattended? | **Yes.** Sessions start in `auto` mode. No permission request reached the client. |
| Does it report cost? | **Yes.** `usage_update` carries a cumulative `cost` in USD: $1.14 for the issue. |
| Does the end of a prompt mean the work is done? | **No.** In the first attempt the prompt ended after 60 s while four background subagents still ran; closing the session lost the work. In the second, their hand-backs arrived inside the prompt, each marked with its origin. A client waits until the agent is quiet, not for `end_turn`. |
| Did it merge into the target branch? | **No, and ACP is not why.** Auto mode refused `gh pr merge` into `acp-probe`, then into `epic/13-exponentiation`. A control through plain `claude -p` with `build_start`'s flags was refused the same way. Merges into epic branches have succeeded only for subagents inside `/epik:epic`. A per-issue agent can't merge its own pull request. |
| What does the same issue cost each way? | $1.14 and $1.12 through ACP; $0.56 through `claude -p`. One run each, so not yet a measurement. |

### Prior work

- Tag `still-rust`: the Rust implementation that orchestrated epics in
  code, with a graph type (`epik-core/src/dag.rs`), a Claude agent module
  (`epik-core/src/agent/claude`), a monitor, and a Tauri app. Its pieces
  are the first place to look before writing new ones.
- `prototypes/epik-app-spike`: a Tauri app that drives Claude Code over
  stream-json from Rust. Its verdict was green. It was built as a chat
  window, which this entry does not need.
- `design_handoff_feature_dashboard`: a dependency graph whose nodes show
  live state (merged, running, paused, failed, blocked), and the rule that
  a pane may notify but never ask. Both carry over.

## Decision

### 1. Epik has an app, and it owns builds

Epik gains a long-lived process, the **Epik app**. It runs wherever Epik
runs today: macOS, Linux and Windows. It lives where each platform keeps
such programs: the menu bar on macOS, the notification area on Windows,
and the system tray on Linux desktops that have one. Its icon shows whether
anything is building. Its menu lists active and recent runs. Choosing a run
opens a window with that run's graph.

Building does not depend on the icon. Where there is no tray (a Linux
desktop without one, or a machine with no desktop) the app runs without
it, and the run's graph is opened from the conversation or from its files.

The app is the only thing that starts, watches or stops builds. The
conversation stays in Claude Code: the app notifies and never asks.
Everything it shows can also be asked for in the conversation.

The app keeps the machine from idle sleep while a run is active, with
each platform's own mechanism. Closing a laptop's lid still stops builds.

### 2. The MCP server asks the app to build

`build_start` no longer starts `claude`. It checks the issue as today, then
sends the request to the app over a local socket: `~/.epik/epik.sock` on
macOS and Linux, a named pipe on Windows. Only the user can open it; that
is its access control. When the app isn't
running, `build_start` launches it and tries again; when it isn't
installed, `build_start` says so and how to install it.

The app replies once the run has started, with its run id and directory.
New read tools (run status, run list) ask the app too, so the conversation
can answer "where are we?".

### 3. The app orchestrates an epic

For each run the app:

1. Clones the repository under `~/.epik/builds/<run-id>`.
2. For an epic: creates the epic branch if it is missing, and adds the
   `epic` label.
3. Reads the graph with the same logic as `epic_status`. Stops on cycles,
   on members in another repository, or on an issue with no sub-issues.
4. Starts an agent for each open issue whose blockers are all closed, then
   waits. An agent's work ends at an open pull request into the epic
   branch with its checks passing. The app then merges that pull request
   and closes the issue (GitHub closes issues only for merges into the
   default branch). Then it reads the graph again and repeats.
5. When every member is closed, opens the epic's pull request into the
   default branch.

Steps 2, 3 and 5, and the merge in step 4, are deterministic and move
into code. Auto mode refuses an issue's agent its own merge, and the
merge is orchestration in any case, so an agent never merges. The loop
in step 4 is the one the skill asked the model to run. This is the
orchestration Epik always meant to own, and 0.2.0's skill was the
stand-in.

A single issue is a run with one member. Its pull request targets the
default branch, so the app leaves it for the user to merge.

If the app quits or crashes, its agents are stopped and the run is
recorded as interrupted. The user builds again: closed issues are skipped,
because GitHub, not the app, records what is done.

### 4. One agent process per issue

Each issue gets its own `claude -p "/epik:issue <n> <branch>"`, in the
run's clone, with today's flags and today's auto-mode check. The issue
procedure is unchanged; it already works in its own worktree.

The `/epik:epic` skill is retired. `/epik:issue` stays: it is what each
agent runs, and still works by hand.

### 5. Every run has a record

Each run gets a directory, `~/.epik/runs/<run-id>/`:

- `events.jsonl` — Epik's own events, written by the app: run started,
  issue started, issue finished (with outcome, duration and cost), run
  finished or interrupted. A small vocabulary, about the run, not the
  agent.
- `<issue>.jsonl` and `<issue>.err` — each agent's raw output, as today.
- `summary.json` — written when the run ends: per issue and in total,
  duration and cost.

The app's windows read these files; so can `tail -f`, and so can an audit.
History needs nothing running. GitHub stays the record of outcomes: what
merged, what is open, what is closed. The run directory records only how
the work went.

### 6. Two seams, each with one implementation

The app reaches an agent through one module, the **agent**: start a prompt
in a directory, report when it has started unattended, and turn its output
into Epik's issue-finished event (outcome, duration, cost). It speaks the
[Agent Client Protocol](https://agentclientprotocol.com) through the
`agent-client-protocol` Rust crate. Its only agent is Claude Code, through
`claude-agent-acp`.

The app runs agents on a machine through one module, the **host**: run this
command in this directory and stream its output back. The local machine is
its only implementation.

These are named so their second implementations have somewhere to go. Each
of those, OpenAI's agent and a remote host, comes with its own entry and
its own findings. Neither is designed here.

### 7. The app is written in Rust

Epik uses Rust wherever it can. The app is Tauri and Rust, building on
`prototypes/epik-app-spike`, which already speaks stream-json and has one
tray API for all three platforms. The app is a separate program from the
plugin and shares no code with it. It needs only the dependency graph,
which it reads from GitHub with `gh` itself (`epic_status`'s graph logic is
under 50 lines), and the plugin's directory, which `build_start` sends with
each request so agents can load `/epik:issue`.

### User setup

1. Node.
2. `gh`, signed in with `gh auth login -s project`, and `gh auth setup-git`
   so builds can push.
3. Claude Code, installed and signed in.
4. The plugin, installed at user scope.
5. The Epik app.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| Keep `/epik:epic`; parse its one log for state | Rejected | Per-issue cost can't be recovered from one session. State would be inferred from model output, and every agent but Claude Code is excluded. |
| A short-lived runner per build, with no app | Rejected | No place to show live state, nothing to keep the machine awake, and no home for remote hosts or cleanup. |
| A background daemon with no interface, plus a separate viewer | Rejected | Two programs to install and keep in step, for what one tray app does. |
| A macOS menu bar app only | Rejected | Epik is cross-platform. The menu bar is the Mac face of the app, not the app. |
| Per-issue agents, but orchestrated by the model as now | Rejected | The model would still own order and state, which is what goes unrecorded. |
| The app as the conversation (the spike's chat window) | Rejected | Reopens the terminal-first decision. The conversation stays in Claude Code. |
| Design the OpenAI agent and remote host now | Deferred | Not needed to ship this. Each needs findings of its own first. |

## Consequences

- **Setup gains a fifth step:** an app built and released for three
  platforms, where the plugin alone needed only Node.
- **Per-issue cost and duration** come from each agent's last `result`.
- **The epic skill's work moves into tested code**: branch, label, graph
  loop and final pull request. The stand-in skill goes away.
- **The app is a second program to release** alongside the plugin, and the
  socket between them is a contract to version.
- **Each issue starts a fresh session.** It reloads the plugin and the
  repository's context instead of sharing the epic session's. Expected to
  cost a little more per issue; measure it on the first epic.
- **ACP appears to cost more than running Claude Code natively.** The same
  issue cost $1.12 and $1.14 through `claude-agent-acp` and $0.56 through
  `claude -p`. If that holds up, it is a reason to add a native Claude Code
  agent beside the ACP one later, behind the same agent seam, and keep ACP
  for other agents.
- **All ready issues run at once**, as with subagents today. A limit waits
  until one is needed.
- **Cleanup has a natural unit:** a run's clone and its directory, and an
  owner that is always running. The retention policy is not decided here.

## Open questions

- **How the app is distributed** on each platform, and which ones are
  signed (macOS notarization, Windows code signing).
- **Which Linux desktops show a tray icon.** GNOME needs an extension. The
  app works without one; how a Linux user finds a run's graph without it
  needs settling.
- Does ACP really cost more than `claude -p`? One run each so far. Build
  the same few issues both ways before deciding whether Claude needs its
  own agent path.
- What does a per-issue session cost against today's subagent? Compare a
  whole epic with `wpm/epik-test`'s $3.67.
- The issue procedure changes with this entry: an agent stops at a passing
  pull request and never merges, and its `/simplify` step goes.
