# Agents and roles: each role is an agent and a model, and Codex is the second agent

- Status: Proposed (draft for Bill, 2026-10-30; not in the repository)
- Date: 2026-10-30
- Builds on: [An Epik app runs builds, one agent per issue, and records every run](2026-10-28-epik-runs-the-epic.md) §6 (the
  agent seam), and [A run is whatever it takes to finish a block of work](2026-10-28-a-run-finishes-a-block-of-work.md)
- Supersedes: §4 ("One agent process per issue") of "An Epik app runs builds, one agent per issue, and records every run",
  where it says each agent loads the plugin to run `/epik:issue`

## Context

The app reaches coding agents through one module, the **agent**, which
speaks ACP and has one implementation: Claude Code through
`claude-agent-acp`. The October record "An Epik app runs builds, one agent per issue, and records every run" named the seam so that OpenAI's
agent had somewhere to go, and left its design to an entry with findings
of its own. This is that entry.

Two reasons to do it now. Some users have no Anthropic account (Sabine is
one), and letting the user pick a model for each kind of work is a reason
to use Epik. The kinds of work are **roles**:

- **Design**: planning with Epik in the conversation. The user's chat
  client runs it, not the app.
- **Coding**: implementing one issue, up to a pull request with passing
  checks. Today's only role.
- **Review**: reading an issue's pull request against the issue and Epik's
  rules before it merges. New.

### Findings (2026-10-30, from documentation, the adapters' source, and this Mac)

| Question | Finding |
|---|---|
| Is there an ACP adapter for Codex? | **Yes:** `@agentclientprotocol/codex-acp` 2.5.0 (Apache-2.0, 2026-10-29), an npm package like Claude's adapter. It runs `codex app-server` and translates. Zed's `@zed-industries/codex-acp` was archived in July in its favor. The Codex CLI doesn't speak ACP itself. Releases are near-daily, so Epik pins one. |
| How is Codex signed in? | `~/.codex/auth.json`, by `codex login` (ChatGPT in the browser, or an API key). `codex login status` checks it. The adapter also reports `_auth/status_update` after `initialize`. |
| Can Codex run unattended, with network for `git push` and `gh`? | **Either with Codex's automatic reviewer, or with no sandbox.** Its modes are `read-only`, `workspace-write` (asks, no network), `agent` (the default: sandboxed, no network, and Codex's own reviewer answers approvals) and `agent-full-access` (never asks, no sandbox). `CODEX_CONFIG` can't open the sandbox's network: each turn sends its mode's policy (adapter source, 2026-10-31). In the probe the reviewer approved every push, `gh` call and `gh pr create`, and no permission request reached the client. |
| Which Codex runs? | **The adapter's own.** codex-acp 2.5.0 depends on `@openai/codex` 0.165 and runs it unless `CODEX_PATH` is set. It shares `~/.codex`, and with it the sign-in. |
| Does Codex report cost? | **No.** Its `usage_update` has only the context window's size and use. The prompt response's ACP `usage` has the turn's tokens (probe: 222k, mostly cached), but no money. ChatGPT plans are metered in credits, where a dollar figure means little anyway. |
| How is a model chosen? | Both adapters offer an ACP session **config option** `model` (Claude's 0.99.0 and Codex's 2.5.0), set with `session/set_config_option`. Neither needs anything agent-specific. |
| How does Codex take a procedure? | Not as a Claude Code plugin. It has Agent Skills and AGENTS.md, and the adapter accepts an appended system prompt on `session/new`. The plain prompt works for every agent. |
| What do Codex's tool calls look like? | Plain ACP: `kind` (`execute`, `edit`, `read`, …), `title`, `rawInput.command`, diffs as `content` of type `diff`. No equivalent of Claude's `_meta.claudeCode.toolName`, which `activity` and `stream` read today. |
| Can a build's agent approve or request changes on the pull request? | **No.** Every agent acts as the user through `gh`, and GitHub refuses a review that approves or requests changes on one's own pull request. A review that only comments is allowed. |
| Prior work | The August record "Builds come home: coding agents under `epik-worker`…" (tag `still-rust`) held that **money is reported, never synthesized**: a price table in Epik goes stale. |

## Decision

### 1. The app gives every agent its procedure in the prompt

An agent's prompt is a short header naming the repository, the issue and
the branch, then Epik's rules (`plugin/skills/assistant/rules.md`), then
the procedure for its role (`plugin/skills/<role>/procedure.md`), all
embedded in the app at build time. No agent loads Epik's plugin. Builds no
longer need a plugin directory, so `plugin_dir` leaves the build request
and the record. The `/epik:issue` skill stays, for building by hand, and
reads the same file.

### 2. An agent is a profile, not a module

One ACP client runs every agent. What differs is data, an **agent
profile**: the command that starts its adapter, the session mode that runs
unattended (`auto` for Claude Code), the extra environment, whether it
reports cost, and how long it must be quiet after its prompt returns before
the session is closed. Claude Code needs that quiet period, 300 seconds
today, because its background subagents can outlive the prompt; Codex
returns only when its work is done, so its period is short. Claude Code and
Codex are two profiles. A third ACP agent is a third profile.

### 3. A role is an agent and a model

Settings holds, in `~/.epik/settings.json`:

- **Coding**: an agent and, optionally, a model. Claude Code, with the
  agent's default model, unless the user changes it.
- **Review**: off, or an agent and, optionally, a model. Off by default.

A model is set through the session's `model` config option. Before a build
starts, the app opens a session for each role without a prompt, to check
that the agent runs unattended and offers the model. A model it doesn't
offer refuses the build, as a missing setup item does: an error naming the
models the agent offers, and no record. A build records the roles it ran
with, and a rerun uses them again. Each **agent session** an issue takes (coding, review, coding again) is
recorded with its role, round, agent, model, time and cost, and the issue's
time and cost are their sums.

The design role isn't the app's: it is whichever client the user plans in.
Planning with Epik in Codex is its own, later piece of work (see the plan).

### 4. Codex is the second agent

The Codex profile runs `@agentclientprotocol/codex-acp`, pinned, through
`npx`, with the Codex the adapter brings. It runs in the **`agent` mode**, where Codex's
own reviewer answers approvals inside a sandbox with no network of its
own; the probe built an issue this way. **[Bill to confirm, against
`agent-full-access`.]** The
checklist gains **Codex** and **Codex sign-in**, needed only while a role
uses Codex. Claude Code and its sign-in are needed to build only while a
role uses Claude Code.

### 5. Cost is what the agent reports

An agent that reports cost has it recorded, summed and commented on the
issue, as today. One that doesn't, like Codex, has its time recorded, and
its cost shown as "not reported by Codex". Epik keeps no price table.

### 6. Review comes before merge

With review on, an issue's pull request is reviewed once its checks pass:

1. The review agent reads the issue and the pull request, judges the change
   against the issue and Epik's rules, and posts its review on the pull
   request as a comment that ends with Epik's verdict marker, naming the
   round it answers: `<!-- epik-review: approve round=1 -->` or
   `<!-- epik-review: changes round=1 -->`.
2. The app reads the verdict from GitHub, not from the agent's words: the
   review whose marker names the current round. The round, not a
   timestamp, ties a verdict to an attempt, so the clocks of GitHub and
   the building machine never have to agree.
3. On **approve**, the build goes on as today: it merges into the epic
   branch, or leaves a single issue's pull request for the user.
4. On **changes**, the coding agent runs again on the same pull request,
   told to address the review, then the checks and the review run again.
   After two rounds of changes, the issue fails and the run needs
   attention. The pull request stays open with its reviews.
5. A review that posts no verdict fails the issue.

The procedure the review agent follows lives beside the issue procedure,
in `plugin/skills/review/procedure.md`.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| A Rust module per agent | Rejected | The two adapters differ only in data. |
| A native `claude -p` path beside ACP | Deferred | The one cost comparison (ACP $1.12 against $0.56) still has a single run on each side. Measure it before deciding. |
| Load Epik's plugin into Claude, a skill into Codex | Rejected | Two ways to carry one procedure, and a plugin on every host. |
| A price table to estimate Codex's cost | Rejected | Goes stale, and means nothing on ChatGPT plans. |
| The review agent approves or requests changes on GitHub | Impossible | GitHub refuses either on one's own pull request. |
| The app reads the verdict from the agent's last message | Rejected | The verdict should be where the user can read it, in the pull request. |
| Review on by default | Rejected | It roughly doubles each issue's agent time. The user turns it on. |
| Per-repository or per-build roles | Deferred | One setting until someone needs more. |
| Roles for agents not reached over ACP | Rejected | ACP is the seam. |
| Claude Code's own sandbox, through the adapter's `sandbox` option | Deferred | It works, but on macOS `gh` can't verify the sandbox proxy's certificate, and with the defaults Claude leaves the sandbox whenever a command fails inside it, with auto mode's classifier approving. One build cost $0.26 against $0.17 and ran every network command outside the sandbox anyway. Revisit when `gh` trusts the proxy (perhaps on Linux) and a strict configuration has built an issue. |

## Consequences

- **Builds no longer depend on the plugin.** A remote host needs no Epik
  plugin, and `build_start` no longer needs `CLAUDE_PLUGIN_ROOT`.
- **A user can build without Claude Code**, once coding (and review, if
  on) use Codex. Planning still happens in Claude Code until Epik works in
  Codex's own client.
- **Codex's cost isn't known**, so a run that used Codex shows a partial
  total, and its comment says which issues' costs are missing.
- **In `agent` mode Codex is the more cautious agent**: ordinary work
  runs in a sandbox and a model reviews what leaves it, where Claude's
  auto mode has the model review risky actions with no sandbox around
  the rest. The approvals happen out of Epik's sight, but are in the
  record as "Guardian Review" tool calls. Were `agent-full-access` chosen
  instead, Codex would run with no sandbox and no reviewer, and Settings
  would say so.
- **Review adds a step, and cost, to every issue**, and a comment per round
  to every pull request.
- **An unreported cost is never a zero.** The audit block on GitHub
  records it as null, and totals count the issues without one.
- **Uninstalling works without Claude Code**, skipping the plugin steps.
- **Activity and the live stream read standard ACP fields first**, so any
  ACP agent shows its commands, reads and edits.

## Open questions

- Which unattended setup Codex should run in (see §4). Settled by a probe.
- Is two rounds the right review limit? Start with two; tune from use.
