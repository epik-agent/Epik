# Design history

Theory and design documents for this project, in chronological order. Each
entry is a dated Markdown file (`YYYY-MM-DD-topic.md`) capturing a design
decision, a theory of the problem, or a revision of one. Newer entries
supersede older ones; keep the old ones — the history is the point.

Entries are not edited once written. When an entry's standing changes, the
index below changes; the document stays as it was, because a decision record
that gets quietly updated is no longer a record.

## Where each entry stands

Read this before treating any entry as binding. Epik pivoted twice — away from
the Electron era in June 2026, and away from the plugin-and-Actions era in
August 2026 — so several entries below are honest history rather than current
architecture.

| Entry | What it decided | Where it stands |
|---|---|---|
| `2026-06-27-epik-architecture` | ADR-0001: architecture and workflow | **Superseded.** Written for the Electron/NATS era. |
| `2026-06-27-consolidate-single-mcp` | ADR-0002: one component, not three repos | **Superseded** as product architecture. EpikMCP survives as an operating tool. |
| `2026-06-28-builds-on-claude-code-not-managed-agents` | ADR-0003: Claude Code is the engine | **Live.** `epik::agent::claude_code`. Codex and an open engine follow behind the same `Agent` trait. |
| `2026-07-28-presence-not-a-connector` | No hosted connector; presence in-band | **Principle live, mechanisms superseded.** Owning as little infrastructure as possible is reaffirmed by the licensed-daemon direction; the plugin, statusline and summon mechanisms belong to the plugin era. |
| `2026-07-28-no-clone-necessary` | Operating Epik needs GitHub, not your disk | **Superseded.** The desktop app builds in worktrees off a local repository; the clone is now central. |
| `2026-07-28-init-is-idempotent-convergence` | `/epik:init` converges a project, idempotently | **Mechanism superseded, contract live.** There is no init skill; the convergence contract survives as the rule that setup runs at every app startup. |
| `2026-07-29-github-app-as-credential` | A GitHub App as pure credential for automation | **Proposed, and reopened.** Predates the restart and assumed the Actions engine as the writer. Not binding until re-examined. |
| `2026-07-31-consent-at-every-granularity` | Epik never acts without the user's say-so | **Proposed, and currently contradicted by the code.** The persona commits, pushes and writes to GitHub with no confirmation, and Agents launch with `--dangerously-skip-permissions` — accepted knowingly as a dogfooding posture, never reconciled with this entry. The question card is the seam a permission layer would reuse. Owed: a decision, either way. |
| `2026-08-04-chatbot-walking-skeleton` | Chat is library infrastructure; model-agnostic over the OpenAI-compatible wire | **Live.** Rewritten since, decisions intact. |
| `2026-08-05-coding-agents-and-tools` | Agents come home; GitHub in the library; tools behind one registry | **Partly live.** GitHub-in-the-library and the one registry hold; `epik-worker` as the agents' home is superseded by the runner, and the one-registry principle bends knowingly where Claude Code brings its own tools. |
| `2026-08-17-build-from-chat` | The persona builds from typed instructions, no GitHub | **Live and built, one decision superseded.** "Claude Code commits; assume success" gives way in `epik-core` (#205): Epik commits what the worktree holds and pushes the branch, under the rule that a step whose outcome can be derived from git is never left to a model's self-report. Reconstructed 2026-08-19; the original was lost before check-in. |
| `2026-08-18-json-in-tool-cards` | One structural JSON renderer for cards and messages | **Principle live, mechanism superseded** by `2026-09-19-the-assistant-window-is-rich-chat`: the JSON logic moves to `epik-core`, and syntect's pruned set gives way to `leptos-rich-chat`'s. Reconstructed 2026-08-19; the original was lost before check-in. |
| `2026-08-19-a-feature-is-a-build-of-builds` | Features are built concurrently under a DAG | **Live and built.** `epik::feature`: the plan, the build, the merge, and the chat verbs. |
| `2026-09-19-the-assistant-window-is-rich-chat` | The window is `leptos-rich-chat`; the assistant lives in `epik-core::assistant` | **Accepted.** |
| `2026-09-20-epik-does-everything-around-the-code` | Deterministic code does everything an agent is not needed for | **Accepted, partly revised.** A red check is now fixed by the agent that caused it, resuming its session (`2026-09-24-task-machines`); which checks a build waits for is read from the repository's rulesets rather than stated by the build (`2026-09-24-the-repositorys-rulesets-are-the-builds-policy`). |
| `2026-09-23-where-a-feature-accumulates` | Whether a feature accumulates on GitHub or in the local clone | **Proposed, undecided.** States the question and the trade-offs; no decision taken. |
| `2026-09-23-how-epiks-tests-are-written` | The conventions every Epik test follows | **Accepted.** Converted from `docs/testing.md`, which was a description; as a record it binds. |
| `2026-09-24-task-machines` | Loops live inside a node as a Task Machine; an issue is one, and fixes its own red checks | **Live and built** (#284). The `Integrate` edge waits on `2026-09-23-where-a-feature-accumulates`. |
| `2026-09-24-the-repositorys-rulesets-are-the-builds-policy` | A build reads the repository's rulesets at launch and waits only for required checks; Epik never changes GitHub settings | **Live and built** (#284). Rulesets only; classic branch protection is not read. |

Three entries carry a reconstruction note in their headers:
`2026-07-29-github-app-as-credential`, `2026-08-17-build-from-chat`, and
`2026-08-18-json-in-tool-cards`. All three were delivered as chat attachments
and lost before check-in, and all three were rebuilt from session records — the
decisions are as discussed, the wording is not the original's. Three losses of
the same kind is a process fact rather than three accidents: **a design decision
that lives only in a conversation has not been recorded.** An entry belongs in
this folder before the work it describes is merged.
