# ADR: What planning tells implementation

- **Status:** Proposed
- **Date:** 2026-10-20
- **Source:** Design conversation (Cowork), October 2026

## Context

The assistant plans with the user; agents implement issues in worktrees.
They are separate sessions, and an agent knows only what it is handed:
`prompt::issue_brief` is the standing preface, the branch, and the issue as
the tracker holds it. `2026-09-20-epik-does-everything-around-the-code`
already settled that an agent's session ends with an edited working tree and
that Epik does everything around it.

`2026-10-20-the-user-always-understands` makes the user's understanding the
central principle. Applied to the build rather than the code, it asks that
the user can read what an agent was told and what it changed, and find
nothing else.

## Decisions

### The issue is the only thing planning tells an agent

Everything an agent learns from planning, it learns from its issue.
Everything else it learns from the repository, which is shared ground truth
and not a channel. The user can read exactly what an agent was told, and any
issue can be run again by launching it again.

So an issue is self-contained. What the agent needs from the feature, from
its siblings, or from the conversation is written into it, with the dials
already turned. A link is not relied on; an agent may or may not follow one.
Conventions are not restated issue to issue: the first issue to need one
establishes it in code, and later ones follow the code.

Rejected: assistant context injected at launch, and a notes file beside the
worktree — each is a second channel the user cannot see. Rejected: the
target repository's `CLAUDE.md`, which belongs to the user.

### Stale issues are prevented, not handled

An issue is the current plan, not a record of a past one. When the plan
changes, the assistant updates every issue that has not started before any
of them launches.

### An agent that departs from its plan says so; one that does not says nothing

The default is no report. An agent that made a judgment call against its
issue writes a summary, and Epik posts it as a comment on the issue's pull
request — the pull request is opened after the agent exits, so the agent
could not post it if it would. Whether the departure is large enough to
report is the agent's judgment, against this test: a later issue, written
from the plan, would assume something about this work that is no longer
true — a name, an interface, a file's location, an added dependency, scope
cut or moved. The test's final wording is a matter for experiment.

### Departures reach dependents mechanically

Before an agent starts, Epik reads the deviation comments of the issues it
is blocked by and writes them into its brief. This is a deterministic step,
not an instruction to read: an agent told to go and read something does so
unreliably.

### An agent reads its own issue

An agent is given its own issue and its blockers' deviation comments, and
reads nothing else of the tracker. Adding the feature issue is an
experiment; the risk it carries is scope creep into siblings, which shows in
the diff.

### Agents have no general GitHub access

Every effect outside the worktree is Epik's. The line is the breadth of what
an agent can do, not who triggers an effect: a narrow capability Epik
implements against a target fixed at launch is consistent with this; the
user's credentials are not.

`2026-09-20` recorded that `ClaudeCode` gains no GitHub token. No token is
added, but none is withheld either: `Process::new` adds to the environment
Epik has, and an agent launched with `--dangerously-skip-permissions`
inherits whatever `gh` and `git` can reach on the user's machine. Withholding
it — `GH_TOKEN`, `GITHUB_TOKEN` and `SSH_AUTH_SOCK` removed, `GH_CONFIG_DIR`
empty, `GIT_TERMINAL_PROMPT=0` — is independent of any decision about tools.

## Open

- **The handoff.** How a summary leaves the agent: a tool is less to build
  than a file convention, but a tool means a worker MCP server, and MCP is
  undecided. A file outside the worktree is the fallback.
- **Direct or transitive.** Whether a brief carries the deviation comments
  of its direct blockers only or of the whole chain behind it. Departures
  are rare, so the whole chain is cheap.
- **The assistant as a reader.** Whether the assistant reads deviation
  comments when it updates unstarted issues, which the stale-issue rule
  needs.
- **When credentials are withheld.** Before release, or carried as debt.
- **A test bed** for comparing brief configurations — the feature-issue
  experiment, the deviation test's wording — is separate work.

## Consequences

- `prompt::issue_brief` gains its blockers' deviation comments.
- The assistant's prompt carries two duties: write self-contained issues,
  and keep unstarted ones current.
- `issue-brief.md` gains the deviation rule and its test.
