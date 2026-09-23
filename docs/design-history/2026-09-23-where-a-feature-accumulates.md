# ADR: Where a feature accumulates

- **Status:** Proposed. The decision is open; this entry states the question and
  what each answer costs.
- **Date:** 2026-09-23
- **Source:** Review of #253, the worktrees thread
  ([discussion_r4078365678](https://github.com/epik-agent/Epik/pull/253#discussion_r4078365678))

## Context

A feature build runs its issues in worktrees of one local clone and pushes each
issue's branch to GitHub as that issue finishes. Everything after the push
happens on GitHub: the pull request is opened against the pushed branch, the
checks are polled against the pushed SHA, the merge is a server-side squash, and
the issue is closed. `epik_core::plan::feature::Issue::run` is those steps in
order, and `Step` names all eight — `Worktree`, `Agent`, `Commit`, `Push`,
`PullRequest`, `Checks`, `Merge`, `Close`.

The invariant this rests on is stated in `worked`:

> The commit is a fact once the push has put it on the remote, and it is the
> commit every step after this one names.

That is the current theory: **GitHub is where a feature accumulates.** The local
clone is scratch space — somewhere to run an agent and make a commit — and the
feature branch on the remote is the only place the feature exists as a whole.
The clone is disposable enough that `LinkedWorktree::remove` deletes it when the
last worktree drops.

The alternative surfaced during the #253 review: **accumulate the feature in the
local clone, and push the result to GitHub.** Issues merge into a local feature
branch as they finish; the network sees the feature once, at the end.

The two are not variations on a push schedule. They disagree about what an issue
*is*, and the disagreement is visible in what each allows.

## The question

Is an issue independently verifiable on GitHub, or is the feature the unit that
GitHub sees?

Everything else follows. Per-issue checks, per-issue pull requests, and
per-issue failure attribution all exist because the issue's commit reaches
GitHub on its own. Move accumulation local and they have nowhere to happen; keep
them and the local accumulation buys nothing, because the objects cross the
network at the same rate either way.

## Option A: GitHub accumulates (today)

The feature branch on the remote is the feature. Each issue pushes, is checked,
and is merged into it.

**For:**

- **An issue is verifiable by itself.** CI runs on the issue's own commit, so a
  red check names the issue that caused it. Failure is localized at the moment
  it happens, not at the end of the feature.
- **The record is on GitHub.** Each issue has a pull request with its own diff,
  checks and merge commit. Someone who was not watching can reconstruct what
  happened from the repository, which is also what makes the work reviewable by
  a human who is not running Epik.
- **The monitor has something to draw.** `Step::Checks`, `Step::Merge` and
  `Step::Close` are per-issue states because they are per-issue events. The
  eight-step shape the window shows is this design.
- **The local clone is disposable.** Nothing of value lives there, so losing it
  — a crash, a cleared disk, a deleted worktree — costs a re-clone and nothing
  else.
- **It is built and working.** No migration.

**Against:**

- **Parallel issues do not see each other.** Each is cut from `origin/feature-N`
  as it stood when that issue started. Two issues running at once each work from
  a base excluding the other, and they first meet when the second one's merge is
  attempted server-side.
- **Conflicts surface as a merge refusal.** When that meeting fails, it fails as
  GitHub declining to merge — the "Pull Request is not mergeable" path — which
  is late, and which reports the collision rather than the work that caused it.
- **Churn on the remote.** A pull request per issue, and CI per issue, on a
  branch nobody intends to review at that granularity.
- **The feature branch is a public work-in-progress.** Half-built features are
  visible on the remote for as long as the build runs.

## Option B: The local clone accumulates

Issues merge into a local feature branch as they finish. GitHub sees the feature
when it is done.

**For:**

- **Integration becomes a step Epik controls.** This is the real argument. A
  clone holding the feature's accumulated state is somewhere issues can be
  rebased onto what has landed, and where a conflict is found *in the build*,
  attributable to the issue that hit it, while the worktree and the agent that
  can fix it are still there. Under Option A that same conflict is a merge
  refusal after the fact.
- **The network is used once.** Fewer, larger pushes; no per-issue pull request
  or CI churn.
- **The remote stays clean.** No half-built feature branch, no pull requests that
  exist only to be squashed.
- **A failed feature can be abandoned without trace.** Nothing was pushed.

**Against:**

- **An issue is no longer independently checked.** CI cannot run on a commit
  GitHub has not seen. Checks move to the end, where a red run names the feature
  and not the issue — losing exactly the attribution the eight-step model
  provides.
- **Failure is found late.** An issue that breaks the build is discovered after
  every issue has run, rather than when it merges. The agents that could have
  fixed it are gone.
- **The local clone becomes load-bearing.** It holds the only copy of the
  feature's accumulated work. Losing it loses the build, which makes clone
  lifetime a durability question rather than a disk-hygiene one, and sits
  badly with the current design where the clone is deleted on the last drop.
- **The monitor's vocabulary changes.** Per-issue `Checks`, `Merge` and `Close`
  no longer describe anything. What the window draws would need rethinking, and
  so would `Watch`'s per-issue states.
- **Epik takes on merging.** Server-side squash merge becomes local merge or
  rebase, so conflict resolution, merge strategy and the "what does Epik do when
  it cannot merge" question all become Epik's code rather than GitHub's
  behavior. This is the largest piece of new work in the option.
- **It contradicts a live invariant.** `worked`'s comment, and the rule from
  `2026-08-17-build-from-chat` that a step whose outcome can be derived from git
  is never left to a model's self-report, are both written against a pushed
  commit being the fact.

## A third position, not yet explored

The two are not exhaustive. An issue could be pushed and checked on GitHub *and*
integrated locally before its next sibling starts — the local clone used for
rebasing onto what has landed, with GitHub still the place each issue is checked
and merged. That keeps per-issue attribution and adds deliberate integration, at
the cost of both mechanisms existing at once.

This may be the interesting one, since the genuine win in Option B is
integration and not the push schedule, and integration does not in fact require
withholding the push. It is written down here because the framing that produced
it — "pushing to a local clone first" — makes it easy to miss.

## Consequences either way

The clone lifetime change agreed in the #253 review — from "when the last
worktree drops" to "when the feature is complete" — is independent of this
decision and worth taking regardless. Under Option A it stops a serial feature
re-cloning per issue. Under Option B it is a precondition, since the clone must
survive the whole feature by definition.

## What would decide it

- **Is per-issue CI load-bearing for Epik's product?** If a red check is meant
  to tell a user which issue went wrong, Option A is the design and Option B
  cannot be adopted without replacing that with something.
- **How often do parallel issues actually collide?** The integration argument
  scales with collision rate. It has not been measured; the current design makes
  collisions visible only as merge refusals, so the data is on GitHub and
  nobody has counted it.
- **Is Epik willing to own merging?** Option B requires it, and it is the part
  most likely to be underestimated.

## Related

- `2026-08-19-a-feature-is-a-build-of-builds` — the DAG, the per-issue steps,
  and the merge-into-feature-branch model this would revise.
- `2026-08-17-build-from-chat` — the rule that a step's outcome is derived from
  git rather than self-reported.
- #254, and the "a single issue is a degenerate case of a `Plan`" work: both
  are asking what a `Plan` may assume about GitHub, which is this question in
  another form.
