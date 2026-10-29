# A run is whatever it takes to finish a block of work, and the library owns its lifecycle

- Status: Accepted
- Date: 2026-10-28
- Builds on: [epik-runs-the-epic](2026-10-28-epik-runs-the-epic.md)
- Supersedes: §5 ("Every run has a record") of that entry, and the
  one-window-per-run design it implies

## Context

Designing the Epik app's window raised a question the current code answers
by accident: what is a run? Today a run is one build attempt. Its id is
`<owner>-<name>-<number>-<UTC stamp>`, `RunState` folds one attempt's
events, and the tray lists attempts. Building an interrupted epic again
makes a second, unrelated run.

That isn't how a person thinks about the work. When a build is
interrupted and started again, it is the same job, not a new one. The
window should show the job, and the job should add up its own time and
cost.

The same design work settled what the app is for. It shows work while it
is happening. It is not an issue tracker and not an archive. GitHub is
both.

## Terms

These are names in the code as well as in the window.

- **Epic issue:** the issue at the root of an epic. An epic is an epic
  issue and its sub-issues, if any.
- **Epic sub-issue:** a direct sub-issue of an epic issue. Epics don't
  nest, so there is nothing below them.
- **Run:** the work of finishing one epic, from its first build until it
  leaves the window. Identified by its epic issue, `owner/name#number`.
- **Build:** one attempt at a run: a clone, an agent for each issue that
  is ready, and a record of what happened. A run has one or more
  builds. `epik_core::build` and `epik_core::builds` already use the name
  in this sense.

What the code calls a run today (`RunState`, `run_id`, `~/.epik/runs/<run-id>/`)
is a build, and is renamed. "Run" is kept for the new, longer-lived thing.
`Member` is dropped: an epic's work is its epic sub-issues, or, with none,
its epic issue. A build works on whichever of those are still open when
it starts.

## Decision

### 1. GitHub is the source of truth

The app doesn't decide what an epic is. An epic is an issue with
sub-issues, or a lone issue with no parent. There are no nested epics.
Building a sub-issue on its own is part of its parent epic. The app reads
all of this from GitHub every time it needs it, and keeps no copy that
could disagree.

GitHub is also the archive. The app keeps nothing about work that is
finished.

Pull requests are for implementing code. Epik never uses a pull request
to record anything else.

### 2. A run is whatever it takes to finish a block of work

A **run** is identified by the issue being built: `owner/name#number`. A
single issue and a multi-issue epic are both runs, and the window shows
them the same way.

A run is made of one or more **builds**, each an attempt: the thing
today's code calls a run. Building again after an interruption or a
failure adds a build to the existing run. A run's elapsed time and cost
are the sums over its builds.

A run begins when its first build starts. The app never shows epics that
haven't started building.

### 3. Every run ends in one of two ways

| Disposition | What the window shows | How it leaves the window |
|---|---|---|
| **Succeeded:** every epic sub-issue merged, and the last pull request waits for review into the default branch | The run's final state, read-only, waiting for review. No delete button. | By itself. When the epic issue closes, the library comments the run's total time and cost on the issue, then deletes the run's local files. |
| **Failed or interrupted,** including a pull request closed without merging | The run's final state, the error, and **Rerun** and **Delete** buttons, as with a failed GitHub Action | When the user deletes it or reruns it to success, or by itself when the epic issue closes |

Closing the epic issue ends the run whatever state it is in: the library
posts the closing comment and deletes the run's files. A successful run
knows when to delete itself. The app only ever shows time and cost so
far; the totals live on GitHub.

### 4. Time and cost are recorded on the issues they were spent on

- **Each epic sub-issue** gets a comment with its own time and
  cost when its pull request merges into the epic branch.
- **The epic issue** gets the closing comment: the run's total time and
  cost, then a breakdown by issue, and the number of builds. A run of
  one issue gets only this comment, since its issue is the epic issue.

Each comment names the pull request and the merge commit the money
bought. The goal this serves comes later: auditing cost line by line,
from a line through `git blame` to its commit, its pull request, its
issue, and that issue's cost. The comments carry what that audit will
need, so it can be built from GitHub alone.

Cost is known per issue, because each agent reports one total for its
session. A line's cost is therefore an estimate, its issue's cost shared
out among the lines it changed. That is good enough to start; finer
attribution, such as per commit from the agent's running cost, can come
later.

### 5. The library owns the lifecycle

`epik-core` owns everything under `~/.epik`. The app's window calls it
and never touches files itself. The library gains:

- **Runs as the unit.** Listing gathers builds under their run and folds
  them into one run state: its epic sub-issues as they now stand, time and cost
  summed, builds in order.
- **Rerun:** start a new build of an existing run.
- **Delete:** remove a run, meaning every build's clone under
  `~/.epik/builds/` and every build's record under `~/.epik/runs/`.
- **Comment:** post an issue's time-and-cost comment when its work is
  done.
- **Finish:** for a run whose epic issue has closed, post the closing
  comment, then delete. The comment is written first, because the totals
  are read from the files the delete removes. A run is deleted only once
  its comment is posted.
- **Reconcile:** check GitHub for what the runs depend on, run on a
  timer by the app: which epic issues have closed (and so finish), which
  waiting pull requests were closed without merging (and so need
  attention), and which monitored repositories still exist.

Names and signatures are left to the implementation.

### 6. The app monitors a list of monitored repositories

The app keeps a list of the repositories it monitors, the **monitored
repositories**. Epik adds a repository when it creates an epic there or
starts a build in it. The user can add and remove repositories in the
app's Settings. Removing one that has runs asks for confirmation and says
what will disappear. A
monitored repository that no longer exists on GitHub is marked with a red ✕
and offered for removal. GitHub answers "not found" for a deleted
repository and one the user can no longer reach, so the app doesn't say
which it was.

## Options considered

| Option | Verdict | Reason |
|---|---|---|
| A run is one build attempt (today) | Rejected | A rebuild becomes an unrelated run, and time and cost split across them. |
| The app lists every open epic in its monitored repositories | Rejected | Duplicates GitHub. Every lone top-level issue counts as an epic, so the list would be the whole issue tracker. |
| Keep finished runs in the app as history | Rejected | GitHub is the archive. Keeping them alive means a retention policy to design. |
| Record totals in the epic's pull request | Rejected | Pull requests are for code. |
| Totals on the epic issue only | Rejected | Cost has to reach the issue, and through it the commit, for a line-by-line audit. |
| Keep totals only in `~/.epik` | Rejected | They are lost when the run is deleted, and invisible to anyone else. |
| Confirm before deleting a failed run | Rejected | Delete removes only local files. GitHub keeps the outcome. |

## Consequences

- **The record's layout changes.** Builds are grouped by run, so a run's
  id no longer carries a timestamp. Nothing is migrated: the only runs
  under `~/.epik` so far are the author's.
- **Every built issue gets a comment from Epik.** That is visible to
  anyone watching the repository.
- **The app talks to GitHub on its own**, on a timer, not only during
  builds. That needs `gh` signed in whenever the app is running.
- **Cleanup has no policy to tune.** Successful runs delete themselves;
  failed ones stay until the user decides.
- **Rerun is the app's first control that starts work.** It starts a
  build the user already asked for, so it stays within "the app notifies
  and never asks."
- **The socket protocol** (`docs/app-protocol.md`) gains run-level reads,
  and `build` on an issue that already has a run adds a build to it.

## Open questions

- **The comments' format.** They are read by people now and by an audit
  tool later. A readable table with the same figures in a hidden,
  machine-readable block is one way to serve both.
