---
name: epic
description: Implement an epic — a dependency-ordered set of GitHub issues — on its own branch using Agent Teams
argument-hint: [epic issue number or GitHub URL] [GitHub epic branch]
disable-model-invocation: true
---

Use Agent Teams to implement the epic issue or all of the issues it indicates.
The epic issue may specify a set of issues, either as child issues or linked issues described in the text.
Do all work on the epic branch. Never touch the default branch.

1. If it does not already exist, create the epic branch on GitHub for the current repo.
    - If you must create the epic branch, base it on the HEAD of the current repo's default branch.
    - Stop if the current repo is not clear from context.
2. Implement the issues in dependency order.
    - For a single issue, the order does not matter.
    - An epic with multiple issues may specify their dependency order in issue text or by using the GitHub
      Relationships fields.
    - Use Agent Teams to implement issues in parallel as much as the dependency order allows.
3. Implement each individual issue using the /issue command. The issue's target branch is the epic branch from step 1.
4. After all of its issues have been implemented, create a pull request for the epic issue that will close the
   epic issue.

While epic development is underway, print a status update every few minutes in the form of a chart showing progress
on each issue.

The epic is complete when all issues have been implemented in the epic branch and that branch is ready for review.
