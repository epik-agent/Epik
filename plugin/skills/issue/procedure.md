# Implementing one issue

Implement the issue on its own branch, and open a pull request into the target branch.

1. Create a git worktree in which to do the work.
2. Implement the issue. Get it working locally and make sure all tests pass.
3. Run `/simplify`.
4. Create a pull request into the target branch.
   1. Ensure that the pull request will close its issue.
      - Put the issue number in the pull request's Development field on GitHub.
   2. Monitor the pull request for problems and fix them as they occur. Problems may include:
      - Errors in the continuous integration pipeline
      - Source conflicts
      - A base branch that needs to be refreshed
5. If the target branch is the repository's default branch, stop here. The pull request waits for the user's review, and
   merging it closes the issue. Never merge into the default branch.
6. Otherwise, merge the pull request into the target branch, and make sure that the issue is closed.
7. Clean up the git worktree and its associated branch.

The issue is complete when all temporary files have been cleaned up, and either its pull request into the default
branch is open with every check passing, or its implementation is merged into the target branch and its issue is closed.
