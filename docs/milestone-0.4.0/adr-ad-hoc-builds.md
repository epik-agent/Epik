# A task is a block of work given in words, and the app builds it like an issue

- Status: Proposed (draft, 2026-10-31; for the release after 0.4.0, not in the repository)
- Date: 2026-10-31
- Builds on: [A run is whatever it takes to finish a block of work](2026-10-28-a-run-finishes-a-block-of-work.md);
  [An Epik app runs builds, one agent per issue, and records every run](2026-10-28-epik-runs-the-epic.md);
  [Agents and roles: each role is an agent and a model, and Codex is the second agent](adr-agents-and-roles.md) (under review), whose
  §1 puts the procedure in the prompt and whose §6 sends a coding agent back to a pull request;
  [Builds can run on another machine, which is a headless Epik app reached over SSH](adr-remote-hosts.md) (under review),
  whose mirror names runs by their key
- Refines: §4 ("One agent process per issue") of "An Epik app runs builds, one agent per issue, and records every run",
  which assumes every block of work is an issue

## Context

Epik's skill says of itself: "You do not write the code yourself." In
practice it does, in two situations. A conversation turns up a small
coding job, too small to be worth an issue, and the user says "just do
it", so the assistant edits the code in the conversation. Or a pull
request has review comments, and the user asks the assistant to address
them. Both are coding work done by the planner, in the planning client,
with no run, no record, no entry in the tray, and no pull request opened
the way builds open them.

That mode is good: not every change deserves an issue, and making the user
file one to get a two-line fix is ceremony for its own sake. What is wrong
is where the work happens. It should happen where every other coding job
happens: in the Epik app, as a run, by a coding agent, ending at a pull
request, visible in the tray and in the record.

The October record defined a run as "whatever it takes to finish a block
of work", and then, everywhere it got concrete, the block of work was an
issue: runs are named `owner/name#number`, a build starts by reading an
issue, a pull request is found by `Closes #n`, and a run leaves when its
issue closes. The 0.4.0 plans were written to leave this open (see the
agents record's §1, and the remote record's mirror, which treats a run's
key as an opaque string). This record says what fills the opening.

## Decision

### 1. A task is a block of work given in words

A **task** is a run whose work is stated in the request rather than in a
GitHub issue: a few sentences of what to do, in a repository, and
optionally the pull request it is about. Two kinds, to start:

- **Do this.** A change described in words, on a new branch, ending at a
  pull request into the repository's default branch (or a branch the
  user names). The pull request's body is the task's words, verbatim,
  so the statement of work is on GitHub as an issue's would be.
- **Address this pull request.** Work on an existing pull request's
  branch: address its review comments, or do what the words say there.
  No new pull request. This is the agents record's §6 round-2 header
  with the user as the reviewer.

**A task about a run belongs to that run.** When the task's pull request
is a run's pull request (an epic's into the default branch, or a single
issue's), or the task names the run, the task is a **new build of that
run**, not a run of its own: it works on the run's branch, it is
recorded with the run's other builds, its time and cost add into the
run's totals and closing comment, and the tray shows it under the epic.
"Address the comments on the epic's pull request" is a build of the
epic's run, exactly as a rerun after a failure is. Only a task with no
run behind it is a run of its own.

A task is still a statement of work that someone wrote down before an
agent read it. What it drops is the issue, not the writing. An agent
told to "fix the thing" gets a task saying "fix the thing"; the user
owns the consequences, as they do when they type it to the assistant
today.

### 2. The assistant starts tasks instead of coding

The MCP server gains `task_start`: repository, the task's words, an
optional pull request to address, an optional target branch, an optional
run the task belongs to (inferred from the pull request when that is a
run's), and (after the remote record) an optional host. A task that
belongs to a run runs on the run's host. The assistant's skill is amended:
when the user asks for code to be changed, Epik starts a task; it does
not edit the code itself. The user sees the task in the tray and its
pull request on GitHub, and reviews or merges it as they would an
issue's.

### 3. A task on its own is a run, named by its first build

The protocol gains a `task` request beside `build`, replied to with the
same `started`. A task that belongs to a run is a build of that run and
needs no key of its own; `started` names the run. A task on its own is a
run whose key is `owner/name@<build id>`, the id of its first build,
which is already unique and sorts by time. It is a run in every sense
the October record gives the word: it has builds, and rerunning it after
a failure adds one; it is listed, shown, rerun, deleted and mirrored as
any run is; and it is the third kind of run beside an epic's and a
single issue's. Its record
lives at `runs/<owner>/<name>/@<build id>/`, in the layout every run has.
Its title, wherever runs are shown, is the first line of its words.

Everything that takes a run takes a task: `status`, `list`, `rerun`,
`delete`, the runs table, the run page, the live stream, `build_status`,
the mirror on a remote host. The one place the key's form matters is
`run.rs`, which parses both.

### 4. A task's build is a single issue's build with a different header

A build gains a `work` field saying what it did: the run's issue or epic
(every build today), or a task's words; `Kind` gains `Task` for a run
that is only a task. The build's prompt is the agents record's §1 prompt
with a header for the kind of task: for **do this**, the repository, the
words, the target branch, and that the pull request's body is to be the
words; for **address this pull request**, the pull request, its branch,
the words, and that no new pull request is opened. The rules and the
issue procedure follow unchanged. The roles apply: the coding role codes,
and the review role, when on, reviews the pull request against the
task's words.

The build succeeds at an open pull request, as a single issue's does,
and leaves it for the user. Its pull request is found by branch (a
task's branch is `task/<build id>`), not by a `Closes` line, since there
is nothing to close.

### 5. A task on its own leaves when its pull request does

A task that belongs to a run leaves with the run, as its other builds do.
Reconcile, which today finishes a run when its epic issue closes, finishes
a task run when its pull request merges or closes. The closing comment, with
the run's time and cost, goes on the pull request. A task whose pull
request was closed unmerged needs attention, as an issue's does.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| Keep doing quick jobs in the assistant | Rejected | Coding by the planner, unrecorded, is the thing the skill says Epik doesn't do. |
| File an issue behind the scenes and build that | Rejected | Issues are for planned work; a tracker full of "fix typo" issues the user never wrote buries the ones they did. (It would be the smallest implementation.) |
| Extend `build_start` with a `task` argument | Rejected | Two tools whose arguments barely overlap; a task has words and no number, an issue has a number and no words. |
| Name a task by its pull request number | Rejected | A new task has no pull request until its agent opens one, so the run would be nameless while it most needs a name. A task addressing a pull request could use its number, but then two key forms would mean two things. |
| Let a task push to the default branch | Rejected | Every change arrives as a pull request, from an agent or from Epik. The one-line fix is where the habit matters most. |
| Several tasks on one pull request at once | Out | While a task on a pull request is building, another on the same one is refused, as a second build of a run is. |
| Every task is a run of its own, even one about an epic | Rejected | The user asked (2026-10-31) that work on an epic be tracked as part of the epic. A separate run would split the epic's cost, record and tray entry in two. |

## Consequences

- **The assistant stops writing code.** The skill's sentence becomes
  true. A quick job costs a run and a pull request, which is the point.
- **Work on an epic after its build stays with the epic.** Fixes to the
  epic's pull request, whether asked for by a reviewer (the agents
  record's §6) or by the user (a task), are builds of the epic's run,
  and the epic's totals, record and tray entry include them. Nothing
  about an epic is ever tracked somewhere else.
- **Every run has a key, and not every key has a number.** `run.rs`
  parses two forms; everything else is unchanged, including the remote
  mirror.
- **A task's record is as full as an issue's**: prompt, agent sessions,
  cost, pull request, closing comment. The tray shows it building.
- **The procedure is unchanged.** What differs travels in the header, as
  the agents record arranged for the review round.
- **A new task's pull request body is its words.** The statement of work
  is on GitHub even though no issue is. "Address this pull request" adds
  nothing to the body; the comments it answers are already there.

## Open questions

- Should the assistant ask before starting a task, or start it on the
  user's word? The user said "do it"; a confirmation seems like the
  ceremony the task exists to avoid. Start it, and say that it started.
- Should a task be able to grow into an issue, when the agent finds it
  larger than its words? Not now: the agent builds what it was told, and
  the user decides from the pull request.
- Is `owner/name@<build id>` readable enough in the tray and in
  conversation, where issues are `#123`? The title carries the meaning;
  the key is for the record.
