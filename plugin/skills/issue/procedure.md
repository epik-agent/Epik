# Implementing one issue

Implement the issue on its own branch, and open a pull request into the target branch.

1. Create a git worktree in which to do the work.
2. Implement the issue, test first. Get it working locally and make sure all tests pass.
3. Create a pull request into the target branch. Its body must contain the line `Closes #<issue number>`, whatever the
   target branch: Epik finds the pull request by that line.
4. Monitor the pull request for problems and fix them as they occur, until every check passes. Wait on its checks with
   `gh pr checks <pull request> --watch --fail-fast` rather than polling. Problems may include:
   - Errors in the continuous integration pipeline
   - Source conflicts
   - A base branch that needs to be refreshed
5. Never merge the pull request, and never close the issue. Epik merges a pull request into an epic branch itself; a
   pull request into the default branch waits for the user's review.
6. Remove the git worktree. Leave its branch: the open pull request needs it.

The issue is complete when its pull request is open with every check passing and its worktree has been removed.
