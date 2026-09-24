# ADR: The repository's rulesets are the build's policy

- **Status:** Accepted
- **Date:** 2026-09-24
- **Source:** Design conversation (Cowork), September 2026

## Context

A feature build merges each issue's pull request into `feature-N` once
GitHub's checks allow it. Whether any checks are coming is stated by the
build, not read: `CHECKS` in the backend's `build.rs` is a constant with
`expected: true`, per `2026-09-20-epik-does-everything-around-the-code`'s "a
build states whether its repository runs checks". A repository that runs no
checks on pull requests into `feature-*` — no CI at all, or the common
`on: pull_request: branches: [main]` — therefore waits the full hour at every
issue's `Checks` step and fails there.

The fact the build needs is not whether checks will *run* but which checks
must *pass* before `feature-N` accepts a merge. GitHub holds that as a
repository setting, the required status checks of a ruleset, and enforces it
on every merge, including Epik's.

## Decisions

### GitHub's settings are the user's

Epik reads repository settings and never writes them: no rulesets, no branch
protection, no Actions settings, created or changed, silently or on offer.
When a setting stands in the way of a build, Epik says which one and why, and
the user decides.

### The policy is read at launch, from rulesets

Before a build creates anything, it reads
`GET /repos/{owner}/{repo}/rules/branches/{branch}` for `feature-N` and for
the issue branches. The endpoint returns the active rules — from repository
and organization rulesets alike, excluding those in `evaluate` or `disabled`
enforcement — that would apply to a branch of that name, whether or not the
branch exists. It needs only Metadata read.

What it returns, read into a **policy**, is what the build obeys:

| Rule | What the build does |
|---|---|
| `required_status_checks` on `feature-N` | The checks it waits for: each `context`, from its `integration_id` when one is given |
| `strict_required_status_checks_policy` | Recorded: an issue's branch must be current with `feature-N` to merge |
| `pull_request`, `allowed_merge_methods` | The merge uses an allowed method: squash when allowed, else merge, else rebase |
| `required_linear_history` | Excludes merge from the methods above |
| `pull_request`, `required_approving_review_count > 0` | Refused at launch: Epik cannot approve its own pull requests |
| `merge_queue` | Refused at launch: Epik does not drive a merge queue |
| `required_signatures` on the issue branches | Refused at launch: Epik's commits are unsigned |
| `creation` or `update` on either branch | Refused at launch: Epik could not push |

A refusal is recorded as the run's `Abandoned`, naming each rule and the
ruleset it comes from, before any agent runs. A rule type this list does not
name is reported on the stream and otherwise passed over: GitHub enforces it
regardless, and a failure it causes is GitHub's to state at the step it
blocks.

### What is waited for is what is required

The `Checks` step waits until every required check has concluded on the
pushed commit, matched by name and — where the rule names one — by the app
that reports it. Every required check green is `Green`; any red is `Red`, the
Task Machine's back-edge (`2026-09-24-task-machines`). No required checks is
`Unclaimed`: the merge proceeds and claims no verdict, as it does today.

A check that ran but is not required never blocks a merge. Its conclusion is
reported on the stream, red included.

This **revises** two lines of `2026-09-20-epik-does-everything-around-the-code`.
"A build states whether its repository runs checks" gives way to a policy the
repository states. And "with checks expected, zero runs is not-yet" narrows:
a *required* check that has not reported is not-yet, and the wait runs to its
bound, where the failure names the checks still missing — the same state
GitHub shows as "Expected — Waiting for status to be reported". The rule that
an empty check list is never green is untouched; it now applies to the
required list.

The `settles` rule — a green list read the same way twice before it is the
verdict — was a guard against checks still arriving. With the checks named in
advance, the wait ends when the last one concludes, and `settles` goes.

### Rulesets only; classic branch protection is not read

Classic branch protection's endpoints need Administration read, a wider
permission than a build otherwise asks for, and their behaviour for a branch
not yet created is undocumented. Epik does not read them.

The cost is bounded. Epik merges only into `feature-N` and pushes only
`feature-N` and `issue-N`; the feature's own pull request into the default
branch is left for the user, so classic protection on the default branch —
where it is almost always set — touches nothing Epik does. A classic rule on
`feature-*` or `issue-*` is still enforced by GitHub, so what it forbids fails
loudly at `Push` or `Merge` rather than landing. Those two refusals say that a
classic branch protection rule may be the cause.

### Strict repositories need the `Integrate` edge

Under `strict_required_status_checks_policy`, an issue whose sibling merged
first is behind `feature-N` and GitHub will not merge it. That is the
`Integrate` edge `2026-09-24-task-machines` names and does not build. Until it
is built, a strict repository builds serial features and fails the later of
two parallel issues at `Merge`. A build with parallel issues on a strict
repository says so at launch rather than refusing: the serial part of it
still lands.

## Rejected

- **Reading `.github/workflows`.** It answers which workflows will run, not
  which must pass, and it would make Epik a partial interpreter of GitHub's
  trigger and filter-pattern grammar — a YAML parser, a matcher, and a
  fallback for everything the model of the grammar missed.
- **Watching for runs to appear.** A grace period, rejected already in
  `2026-09-20` as a guess wearing deterministic clothes.
- **An Epik configuration line naming the checks.** Right on the day it is
  written, stale without anyone noticing, and a second place for a policy the
  repository already holds.
- **Epik creating a `feature-*` ruleset.** GitHub's settings are the user's.

## Consequences

- `github`: a client call for a branch's rules; a check run carries the id of
  the app that reported it.
- `plan::github`: `CheckWait` loses `expected` and `settles`; `await_checks`
  takes the required list.
- The backend's `CHECKS` constant keeps only the cadence and the bound. The
  launch reads the policy and records a refusal as `Abandoned`.
- `monitor`: whether an issue's merge was checked is the issue's, from its
  verdict, rather than the build's from `Started`.
- The merge takes its method from the policy rather than `Merge::Squash`.
- Independent of the issue brief and of `2026-09-24-task-machines`'s loop,
  and meeting them only at `Checks`: until it is built, a live build needs a
  repository whose checks run on pull requests into `feature-*`.
