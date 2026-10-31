# Milestone 0.4.0: plan

Drafted 2026-10-30, assuming 0.3.0 has landed (milestone 1, including the
polish epic #458, now in PR #471). Nothing here is filed on GitHub.

The milestone's own description: *Remote work and agents beyond Anthropic.
Builds can run on a remote host that streams their logs back. Coding agents
and models can be mixed and matched by role (design, coding, review),
starting with OpenAI.*

## The files

| File | What it is |
|---|---|
| `plan.md` | This overview: order, decisions for Bill, what's left out |
| `adr-agents-and-roles.md` | Draft decision record, in `docs/design-history` style |
| `adr-remote-hosts.md` | Draft decision record |
| `epic-agents-and-roles.md` | Epic A: 17 issues, graph, findings still to make |
| `epic-remote-hosts.md` | Epic R: 15 issues, graph, findings still to make |
| `probe-codex.md` | The Codex probe's first attempt: what it settled, what's open, how to rerun |

Each epic file gives each issue as it would be filed: why, what, tests
first, and what's out. They are meant to be read, argued with and cut
before anything is filed.

## The shape of 0.4.0

Two epics, then an optional third:

1. **Agents and roles (A).** The procedure travels in the prompt, so builds
   stop needing the plugin. Agents become profiles of one ACP client.
   Codex is the second profile. A role is an agent and a model, set in
   Settings. Optional review before merge, with the verdict posted on the
   pull request.
2. **Remote hosts (R).** A remote host is a whole headless Epik app on
   another machine, reached through `ssh <host> epik-app stdio`. Builds run
   entirely there, so the laptop can close. The local app mirrors the
   host's records under `~/.epik/hosts/<host>/` and reads them through the
   same code as local runs.
3. **Plan with Epik in Codex (C), optional.** The design role. See below.

Each decision record lands first as its own docs pull request, as #355 and
#394 did, so the epics' issues can cite it.

## What I found that changes the plan

- **The "host" seam named in October would not deliver the goal.** It
  moved only the agents ("run this command and stream it back"), which
  leaves the orchestrator on the laptop, so closing the lid still stops
  the build. The draft moves the whole app instead and supersedes that half
  of §6.
- **Cloud sessions still can't host builds.** A program still can't start
  a new cloud session in a documented way, and in cloud sessions GitHub's
  proxy refuses GraphQL, so `gh pr` fails.
- **Builds needn't load the plugin at all.** Putting the procedure in the
  prompt (A1) is what makes Codex and remote hosts simple: no plugin on the
  host, no `plugin_dir` in requests, no `CLAUDE_PLUGIN_ROOT`.
- **Codex runs unattended with network only with its sandbox off**
  (`agent-full-access`). A sandboxed, networked configuration may work; a
  probe settles it.
- **Codex reports no cost**, not even cumulative tokens. The August
  principle "money is reported, never synthesized" says to show "not
  reported" rather than estimate.
- **An agent can't approve its own pull request.** Builds act as the user,
  and GitHub refuses approvals and change requests on one's own pull
  request. So review is a comment ending in a hidden verdict marker that the
  app reads back from GitHub.
- **Model choice is already agent-neutral.** Both adapters offer a `model`
  session config option.
- **`epik-app --headless` is already Tauri-free in what it runs.** Only
  the linking isn't, which a cargo feature fixes (R1).
- `~/.epik/config.toml` on this Mac is left over from the old Rust version.
  Nothing reads it. The plan uses `settings.json`, beside
  `repositories.json`.

## Order

**Recommendation: A1 first, then epic A, then epic R, then C if it stays.**

The order agreed on 2026-10-28 put remote hosts first. I suggest reversing
it, for three reasons:

- A has fewer unknowns and can be tried entirely on this Mac, where Codex
  is installed and signed in. R needs a host that doesn't exist yet.
- R forwards a build request that should already carry roles and no plugin
  directory. Built after A, R is smaller.
- The two epics touch the same files (`build.rs`, `ipc.rs`, `record.rs`,
  the Settings page), so building them in parallel would mean conflicts
  between epic branches.

If remote stays first, A1 moves into epic R as its root, and R9 forwards
no roles until A lands.

## Decisions for Bill

1. **Order:** A then R (recommended), or R then A as agreed.
2. **Codex's unattended mode:** the default `agent` mode, sandboxed, with
   Codex's own reviewer approving what leaves the sandbox (recommended:
   the probe built #13 this way with no prompt reaching Epik), or
   `agent-full-access` (no sandbox, the user's own access). A sandboxed
   setup with open network isn't possible through the adapter. Either
   way, Settings says which.
3. **Cost Codex doesn't report:** show "not reported" (recommended), or
   keep a price table.
4. **Review in 0.4.0:** in, off by default, verdict by marker comment,
   two rounds (recommended), or cut to keep the release small. A10–A13 cut
   cleanly: drop them, and A14 and A15 lose their review parts. A5 (one
   record per agent session) is still worth keeping.
5. **The design role:** epic C in 0.4.0, or the next release. I lean to
   next release unless Sabine, or someone else without an Anthropic
   account, is the user 0.4.0 is for.
6. **Host setup by hand, Linux-only artifacts** (recommended), or
   automated setup from the app.
7. **A host to test on:** a small cloud VM (with Tailscale SSH), a machine
   at home, or a container on this Mac for development only.

## Findings to make before filing

These are experiments, done with Bill (not filed as issues). Each needs his
go-ahead, because they download the Codex adapter, spend a little on
OpenAI's API, or need a machine:

1. The Codex probe (epic A, finding 1): build one `wpm/epik-test` issue
   through codex-acp 2.5.0. Settles decision 2 and four smaller questions.
   **Done 2026-10-31:** Codex built #13 unattended in `agent` mode and
   opened wpm/epik-test#17. Details in `probe-codex.md`.
2. Prompt against skill for Claude (epic A, finding 2).
3. A host, and the sign-ins over SSH (epic R, findings 1–2).
4. Windows' in-box `ssh -T` relaying stdio (epic R, finding 3), on the
   machine used for the `${HOME}` check.

Findings 1 and 2 finish the agents record; 3 and 4 finish the remote one.
Then the records merge, and the issues are filed with `blocked-by` links as
drawn, checked with `epic_status`.

## How the plans were checked

A fresh agent reviewed the first drafts against the code on the polish
branch. It found real holes, now fixed: the window reads runs through
Tauri commands, not the protocol, so host runs needed their own issue
(R8); `list` stops at 20 runs, which would have made the mirror delete the
rest (R3's `runs`); roles had no way to reach a host (A4); one
`IssueFinished` per issue can't hold a review's cost (A5); an unreported
cost would have been written as $0 (A6); and the release names files
without their version (R14). It also split six issues that were too large
and fixed several dependency edges.

## Epic C (optional): plan with Epik in Codex

A user with no Anthropic account can build with Codex after epic A, but
still plans in Claude Code. Codex's CLI has Agent Skills (`SKILL.md` in
`~/.agents/skills`) and MCP servers (`codex mcp add`), so Epik can live
there too:

- C1. A setup item, **Epik in Codex**, shown when a role uses Codex: writes
  Epik's assistant skill to `~/.agents/skills/epik/` (embedded in the app,
  refreshed when the app updates) and registers `~/.epik/bin/epik-app mcp`
  with `codex mcp add epik`. Uninstall removes both.
- C2. Make the assistant skill's words client-neutral where they name
  Claude Code (setup, Desktop, troubleshooting), with what differs in
  Codex.
- C3. Make the checklist's Claude Code items optional when no role uses
  Claude and Epik is set up in Codex.

Its findings: does Codex load a skill with Epik's frontmatter, call Epik's
MCP tools without prompting each time, and run `gh` in its shell under its
default sandbox? This epic is the first thing to cut.

## Left out on purpose

- A native `claude -p` agent beside ACP. The cost gap ($1.12 against
  $0.56) is one run each. Measure it with finding 2, then decide.
- Price tables. Per-repository or per-build roles. Listing the models an
  agent offers.
- Installing on a host from the app. macOS and Windows hosts. Moving a run
  between hosts. A `follow` request instead of polling.
- Concurrency limits. All ready issues still start at once, on any host.

## Outside 0.4.0, noted so they aren't lost

- The chat-support probes (Cowork, Desktop chat, Claude Design) are due
  once 0.3.0 merges. They are separate from this milestone.
- The macOS app menu and About stay deferred until Settings outgrows its
  page.

## Risks

- **Adapter churn.** Both adapters release near-daily. Pin both, as now,
  and make bumping a version a deliberate change with a probe run.
- **Codex without a sandbox** runs with the user's credentials. That is a
  real change in trust, and the reason decision 2 is Bill's.
- **Review doubles agent time**, and its value isn't measured. It is off
  by default, so its cost is opt-in.
- **Protocol across machines.** Requiring the same minor version on both
  ends is blunt but safe. Loosen it when a real version skew shows up.
