---
description: Implement a feature — a set of related GitHub issues — on its own branch using Agent Teams
argument-hint: [feature issue number or GitHub URL] [GitHub feature branch]
---

Use Agent Teams to implement the feature issue or all of the issues it indicates.
The feature issue may specify a set of issues, either as child issues or linked issues described in the text.
Do all work on the feature branch. Never touch the default branch.

1. If it does not already exist, create the feature branch on GitHub for the current repo.
    - If you must create the feature branch, base it on the HEAD of the current repo's default branch.
    - Stop if the current repo is not clear from context.
2. Implement the issues in dependency order.
    - For a single issue, the order does not matter.
    - A feature with multiple issues may specify their dependency order in issue text or by using the GitHub Relationships fields.
    - Use Agent Teams to implement issues in parallel as much as the dependency order allows.
3. Implement each individual issue on its own issue branch and create a pull request for it that targets the feature branch.
4. Monitor the pull request for problems and fix them as they occur. Problems may include:
   - Errors in the continuous integration pipeline
   - Source conflicts
   - A base branch that needs to be refreshed
5. When the pull request is clear to commit, run `/review`, writing review comments to the GitHub issue, addressing all comments, and fixing further continuous integration problems as necessary.
6. When all review comments have been addressed, merge the pull request branch into the feature branch.
7. While feature development is underway, print a status update every few minutes in the form of a chart showing progress on each issue.
8. Ensure that individual feature issues are closed once their corresponding pull requests have been merged.

The feature is complete when all issues have been implemented in the feature branch and that branch is ready for review.
