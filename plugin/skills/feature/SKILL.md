---
name: feature
description: Launch a headless feature build on GitHub Actions and monitor it from chat
argument-hint: [feature issue number or GitHub URL] [GitHub feature branch]
disable-model-invocation: true
---

Launch a headless build of the feature issue and monitor it from here. The
build itself runs on GitHub Actions — do NOT implement the issues in this
session.

A feature issue may specify a set of issues, either as child issues or linked
issues described in the text. All build work happens on the feature branch;
the default branch is never touched.

1. Resolve the launch parameters:
   - Repo: the current repo, in `owner/name` form. Stop if the repo is not
     clear from context.
   - Feature issue number: from the argument (number or GitHub URL).
   - Base branch: the repo's default branch (use `repo_default_branch`).
   - Feature branch: the second argument if given, otherwise
     `feature-<issue number>`.
2. Sanity-check readiness before launching:
   - The feature issue exists and is open (`issue_get`).
   - It has sub-issues or clearly enumerated linked issues
     (`issue_list_relationships` / the issue text). If the issue graph looks
     unfinished, say so and stop — converge on the plan first.
3. Launch the build with the EpikMCP `feature_launch` tool, passing the repo,
   feature issue number, base branch, and feature branch. This dispatches the
   `epik-build.yml` workflow on GitHub Actions (the repo must have that
   workflow and an `ANTHROPIC_API_KEY` secret configured).
4. Confirm the dispatch: report the feature, branch, and how to watch it
   (`run_list` with workflow `epik-build.yml`, and `feature_status`).
5. Hand off to monitoring by invoking `/loop` with this goal:

   > Monitor the headless build of feature issue #<n> in <owner/name>. Each
   > iteration: call `feature_status` for the feature and `run_list`
   > (workflow `epik-build.yml`, plus recent CI runs on the feature and issue
   > branches), and print a compact status table of sub-issues, their PRs,
   > and CI state. Stay quiet otherwise. Interrupt me only on needs-me
   > events: the build run fails or stalls, CI on a PR fails repeatedly, the
   > build posts a comment asking for a human decision, or the feature
   > completes (all sub-issues closed and merged into the feature branch).
   > Stop looping once the feature completes or the build run ends in
   > failure.

The feature is complete when all issues have been implemented in the feature
branch and that branch is ready for review.
