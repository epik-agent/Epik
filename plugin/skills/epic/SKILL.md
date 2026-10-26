---
name: epic
description: Implement an epic — a dependency-ordered set of GitHub issues — on its own branch
argument-hint: [epic issue number or GitHub URL] [GitHub epic branch]
disable-model-invocation: true
---

Implement all of the epic issue's sub-issues. They are the epic's only members, and the `epic_status` tool is the
only source of their dependency order: never work out membership or order from issue text.
Do all work on the epic branch. Never touch the default branch.

1. Make sure the epic issue has the `epic` label.
    - If the repository has no label named `epic`, create it:
      `gh label create epic --color 5319E7 --description "A dependency-ordered set of issues"`.
      Never change an existing `epic` label's color or description.
    - Add the label to the epic issue if it does not already have it.
2. If it does not already exist, create the epic branch on GitHub for the current repo.
    - If you must create the epic branch, base it on the HEAD of the current repo's default branch.
    - Stop if the current repo is not clear from context.
3. Implement the sub-issues in dependency order.
    1. Call `epic_status` on the epic issue.
    2. If it reports `cycles`, stop and report them.
    3. If the epic has no sub-issues, stop and say it is not an epic.
    4. If any sub-issue is in another repository, stop and report it.
    5. Start a subagent (step 4) for every open sub-issue whose `blocked_by` issues are all closed and that is not
       already running.
    6. Each time a subagent finishes, call `epic_status` again and repeat from 5.
    7. The sub-issues are implemented when every one of them is closed.
4. Implement each individual issue in its own subagent. Give the subagent the issue, its target branch (the epic
   branch from step 2), and the instruction to follow the procedure in
   `${CLAUDE_PLUGIN_ROOT}/skills/issue/procedure.md`.
5. After all of its issues have been implemented, create a pull request for the epic issue that will close the
   epic issue.

While epic development is underway, print a status update every few minutes in the form of a chart showing progress
on each issue.

The epic is complete when all issues have been implemented in the epic branch and that branch is ready for review.
