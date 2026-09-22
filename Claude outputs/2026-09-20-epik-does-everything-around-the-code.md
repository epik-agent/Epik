# ADR: Epik does everything around the code

- **Status:** Accepted
- **Date:** 2026-09-20
- **Source:** Design conversation (Cowork), September 2026

## Context

`crates/epik-core` holds the pieces of a feature build — `plan.rs` with its
`Dag`-shaped `Plan` and `AgentSlots`, `plan/implement.rs` running one agent
as a `Task`, `agent/claude.rs`, `worktrees.rs`, `github.rs`, and
`tracker/github.rs` — and nothing yet assembles them into a build.

The behaviour being ported is written down in this repository's own
`.claude/commands/feature.md` and `issue.md`, the slash commands Bill drives
builds with by hand. A feature there becomes an Epik `Plan`; an issue becomes
a node in it. The Agent Teams instruction and the ordering steps of
`feature.md` are what a `Plan` already is.

## Decisions

### Whatever can be done with deterministic code is not done by an agent

Stated as a standing rule and recorded here because it decides most of what
follows. An agent writes code. Epik does everything around it: branch names,
commit and push, pull-request bodies, check verdicts, merges, closing issues.
A step whose outcome can be read from git or from the API is never taken on a
model's account of itself.

### The agent's session ends with an edited working tree

`issue.md` steps 4–7 — open the pull request, watch CI, merge, close the issue,
clean up — are Epik's, as `Task`s. The agent is handed an issue and a worktree
and is finished when the files are written.

Rejected: the agent doing 4–7 in one session, close to `issue.md` verbatim.
It would need a GitHub token in its environment and the authority to merge,
its "monitor the pull request for problems" is a model in a polling loop
billed per turn, and Epik's only view of an issue's progress would be GitHub
state read after the fact. Also rejected: the agent opening its own pull
request and Epik merging — the same token, for one templated body.

The cost accepted: a red check is not fixed by the agent that caused it in the
context it still holds. Remediation is a second run, which is bounded and
observable, and the triage before it — conflict, stale base, or a genuine
failure — is a deterministic read of the API.

### Epik commits the worktree

This **supersedes** "Claude Code commits; assume success" in
`2026-08-17-build-from-chat.md`. "Did the agent remember to commit" is a
self-report standing in for a fact git holds. Epik stages and commits what the
worktree contains, under an Epik identity set repo-locally when the worktree
is provisioned. The commit messages the agent would have written are given up.

### Sub-issue edges are membership; blocked-by edges are order

`IssueTracker::dependents` follows `graph.blocking` and never reads
`graph.sub_issues`, so a feature filed the way #158 was returns a DAG holding
only the feature issue. Both relationship kinds shape a plan, each meaning
what GitHub means by it, which is the containment-tree/order-DAG split of
`2026-08-19-a-feature-is-a-build-of-builds.md` arriving in `epik-core`.

Membership is the transitive `sub_issues` closure from the root; work nodes
are its leaves. Order is the `blocked_by` edges among members, pushed through
containers in both directions. A blocker outside the membership is read rather
than dropped: closed satisfies, open refuses the plan and names it. A member
already closed is not work and satisfies the edges naming it.

Rejected: reading membership out of issue text. The graph comes from
relationships recorded in the tracker, never from prose.

### A plan is one DAG with three kinds of node

`Plan<T> = Dag<T>` is homogeneous in `T`, so the
nodes travel as an enum: creating the feature branch (the single source), one
node per issue, and opening the feature's pull request (the single sink).
Creating the branch is a node for the same reason the final pull request is —
it makes the whole of `feature.md` one plan, and `execute`'s existing
semantics then supply the sequencing and the "a failure holds back only what
follows it" behaviour without anything being written.

Rejected: `Box<dyn Task<Return = R>>`. The set of node kinds in a feature
build is knowable, and an enum keeps it checked.

**One node per issue, not five.** The leaf operations — worktree, agent,
commit, push, pull request, checks, merge, close — are `Task`s composed by a
sequence. Five DAG nodes per issue would make the graph unreadable and buy
nothing, since the steps are strictly sequential; and the monitor's vocabulary
is already per-issue, with a `Merging` state that only means something if an
issue is one node.

### An empty check list is never green

`check_conclusions` returns an empty list both for a repository that runs no
checks on pull requests into non-default branches and for the seconds after a
pull request opens. A fold over zero conclusions is vacuously green, which
would merge unverified work and call it verified.

A build states whether its repository runs checks. With checks expected, zero
runs is not-yet, and the wait continues to a stated bound. With none expected,
the merge proceeds and no verdict is claimed.

Rejected: a grace period. Its verdict depends on how fast Actions registered,
which is a guess wearing deterministic clothes.

This also moves the green gate of `2026-08-19-a-feature-is-a-build-of-builds.md`:
there the check ran after the merge, in the feature workspace, under the merge
lock; here GitHub runs it before the merge, on the pull request. The contract
— every commit on the feature branch is green — is unchanged.

### Narration is emitted as it arrives

`Implement::run` collects the whole stdout iterator before returning, so a run
says nothing until it ends. `Process` already streams; the `collect` is the
batching. Each `Update` is sent onward as its line is interpreted, through a
sink `Implement` carries, on the one endpoint of
`2026-08-15`'s decision. What crosses names its node, because several agents
run at once through one endpoint. Task lifecycle crosses the same endpoint, so
a node that runs no agent is not invisible. The agent's stderr is emitted
rather than dropped — it is where a CLI's last words land.

### A task returns nothing

Once the narration leaves on the stream as it happens, nothing is left for a
task to hand back. A nonzero exit is already a task's error — which is what
holds back its dependents — so on the success path a returned exit would be
the constant `Code(0)`.

`Task::run` returns `Result<()>` and loses its associated `Return`;
`Plan::execute` returns `Result<()>` and loses its `Sum` bound; `Narration`
and its `Sum` implementation go with them. No value travels between nodes:
the feature branch's name is computed from the feature's number, and the pull
request number the checks and the merge need is threaded within one issue's
sequence.

Slots bound agents, not nodes and not processes: a slot is one agent, held
while the agent runs, so an issue waiting on its checks holds nothing, and git
and the other short-lived commands take no slot at all (#240, revising #223).

### Pull requests carry `Closes #<n>`, and issues are closed explicitly

No API sets GitHub's Development field; a closing keyword in the body is what
fills it. And a closing keyword fires only on a merge into the default branch,
so an issue whose pull request merges into a feature branch is closed by an
explicit call.

### The prompt names no slash command

An Epik-launched agent runs `--settings '{}' --strict-mcp-config` in a
worktree of a repository that is not Epik's, so neither `.claude/commands/`
nor any installed plugin is there. `issue.md`'s "Run `/simplify`" becomes
prose in the issue brief. Simplification is Epik's own instruction rather than
a dependency on an optional install, which is where opinionated craft belongs.

## Consequences

- `tracker/github.rs`'s `dependents` changes meaning, and its name follows
  what it returns. `tests/support/json_tracker.rs` and `tests/adder.rs` state
  the old meaning and move with it.
- `Worktrees::add` takes the branch to create and the commit-ish to create it
  at; an issue's branch is cut from the feature branch's tip at dispatch.
- `ClaudeCode` gains no GitHub token.
- The stream becomes the only record of a build. A run of real work is never
  conducted without a sink attached; the run log of `2026-08-08` is it.
- Filed as #208 with sub-issues #202, #203, #204, #205, #206, #207.
