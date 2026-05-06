"""Pull request tools for epik-gh."""
from __future__ import annotations

from typing import Any

from .errors import ValidationError
from .runner import run_gh

_PR_FIELDS = [
    "number",
    "title",
    "body",
    "state",
    "labels",
    "assignees",
    "author",
    "baseRefName",
    "headRefName",
    "createdAt",
    "updatedAt",
    "url",
    "mergeable",
    "reviewDecision",
    "isDraft",
]


def pr_list(
    repo: str,
    state: str = "open",
    base: str | None = None,
    head: str | None = None,
    assignee: str | None = None,
    limit: int = 30,
) -> list[dict[str, Any]]:
    """List pull requests in a repository.

    Args:
        repo: Repository in owner/name format.
        state: Filter by state: open, closed, or merged.
        base: Filter by base branch name.
        head: Filter by head branch name.
        assignee: Filter by assignee login.
        limit: Maximum number of PRs to return (default 30).

    Returns:
        List of pull request objects.
    """
    if state not in ("open", "closed", "merged"):
        raise ValidationError(f"state must be open, closed, or merged; got {state!r}")
    args = ["pr", "list", "--repo", repo, "--state", state, "--limit", str(limit)]
    if base:
        args.extend(["--base", base])
    if head:
        args.extend(["--head", head])
    if assignee:
        args.extend(["--assignee", assignee])
    _, data, _ = run_gh(
        *args,
        json_fields=["number", "title", "state", "headRefName", "baseRefName", "url"],
    )
    return data  # type: ignore[return-value]


def pr_get(repo: str, pr_number: int) -> dict[str, Any]:
    """Get a single pull request by number.

    Args:
        repo: Repository in owner/name format.
        pr_number: The pull request number.

    Returns:
        Pull request object with full details.
    """
    _, data, _ = run_gh(
        "pr", "view", str(pr_number), "--repo", repo,
        json_fields=_PR_FIELDS,
    )
    return data  # type: ignore[return-value]


def pr_create(
    repo: str,
    title: str,
    body: str = "",
    base: str | None = None,
    head: str | None = None,
    draft: bool = False,
    labels: str | None = None,
    assignees: str | None = None,
) -> dict[str, Any]:
    """Create a new pull request.

    Args:
        repo: Repository in owner/name format.
        title: PR title.
        body: PR body (markdown).
        base: Base branch name (defaults to repository default branch).
        head: Head branch name (defaults to current branch).
        draft: Create as a draft PR.
        labels: Comma-separated label names to apply.
        assignees: Comma-separated GitHub logins to assign.

    Returns:
        The created pull request object.
    """
    if not title:
        raise ValidationError("title is required")
    args = ["pr", "create", "--repo", repo, "--title", title, "--body", body]
    if base:
        args.extend(["--base", base])
    if head:
        args.extend(["--head", head])
    if draft:
        args.append("--draft")
    if labels:
        args.extend(["--label", labels])
    if assignees:
        args.extend(["--assignee", assignees])
    _, data, _ = run_gh(*args, json_fields=["number", "title", "url"])
    return data  # type: ignore[return-value]


def pr_edit(
    repo: str,
    pr_number: int,
    title: str | None = None,
    body: str | None = None,
    base: str | None = None,
    labels: str | None = None,
    assignees: str | None = None,
) -> dict[str, Any]:
    """Edit an existing pull request.

    Args:
        repo: Repository in owner/name format.
        pr_number: The pull request number.
        title: New title (omit to leave unchanged).
        body: New body (omit to leave unchanged).
        base: New base branch.
        labels: Comma-separated labels to set.
        assignees: Comma-separated logins to set as assignees.

    Returns:
        The updated pull request object.
    """
    args = ["pr", "edit", str(pr_number), "--repo", repo]
    if title is not None:
        args.extend(["--title", title])
    if body is not None:
        args.extend(["--body", body])
    if base is not None:
        args.extend(["--base", base])
    if labels is not None:
        args.extend(["--label", labels])
    if assignees is not None:
        args.extend(["--add-assignee", assignees])
    _, data, _ = run_gh(*args, json_fields=["number", "title", "url"])
    return data  # type: ignore[return-value]


def pr_close(repo: str, pr_number: int, comment: str | None = None) -> dict[str, Any]:
    """Close a pull request without merging.

    Args:
        repo: Repository in owner/name format.
        pr_number: The pull request number to close.
        comment: Optional comment to post when closing.

    Returns:
        The updated pull request object.
    """
    args = ["pr", "close", str(pr_number), "--repo", repo]
    if comment:
        args.extend(["--comment", comment])
    _, data, _ = run_gh(*args, json_fields=["number", "title", "state", "url"])
    return data  # type: ignore[return-value]


def pr_merge(
    repo: str,
    pr_number: int,
    method: str = "merge",
    auto: bool = False,
    delete_branch: bool = False,
) -> dict[str, Any]:
    """Merge a pull request.

    Args:
        repo: Repository in owner/name format.
        pr_number: The pull request number to merge.
        method: Merge method: merge, squash, or rebase.
        auto: Enable auto-merge (merge when requirements are met).
        delete_branch: Delete the head branch after merging.

    Returns:
        The merged pull request object.
    """
    if method not in ("merge", "squash", "rebase"):
        raise ValidationError(f"method must be merge, squash, or rebase; got {method!r}")
    args = ["pr", "merge", str(pr_number), "--repo", repo, f"--{method}"]
    if auto:
        args.append("--auto")
    if delete_branch:
        args.append("--delete-branch")
    _, data, _ = run_gh(*args, json_fields=["number", "title", "state", "url"])
    return data  # type: ignore[return-value]


def pr_review(
    repo: str,
    pr_number: int,
    action: str,
    body: str = "",
) -> dict[str, Any]:
    """Submit a review on a pull request.

    Args:
        repo: Repository in owner/name format.
        pr_number: The pull request number to review.
        action: Review action: approve, comment, or request-changes.
        body: Review comment body (required for comment and request-changes).

    Returns:
        Dict confirming the review was submitted.
    """
    if action not in ("approve", "comment", "request-changes"):
        raise ValidationError(
            f"action must be approve, comment, or request-changes; got {action!r}"
        )
    if action in ("comment", "request-changes") and not body:
        raise ValidationError(f"body is required for action={action!r}")
    args = ["pr", "review", str(pr_number), "--repo", repo, f"--{action}"]
    if body:
        args.extend(["--body", body])
    _, data, _ = run_gh(*args)
    return {"status": "review submitted", "action": action, "pr_number": pr_number}


def pr_comment(repo: str, pr_number: int, body: str) -> dict[str, Any]:
    """Post a comment on a pull request.

    Args:
        repo: Repository in owner/name format.
        pr_number: The pull request number to comment on.
        body: Comment body text (markdown).

    Returns:
        Dict with the comment URL.
    """
    if not body:
        raise ValidationError("body is required")
    _, data, _ = run_gh(
        "pr", "comment", str(pr_number), "--repo", repo, "--body", body,
        json_fields=["url"],
    )
    return data  # type: ignore[return-value]


def register(server: Any) -> None:
    """Register all pull request tools with the MCP server."""

    @server.tool()
    def tool_pr_list(
        repo: str,
        state: str = "open",
        base: str | None = None,
        head: str | None = None,
        assignee: str | None = None,
        limit: int = 30,
    ) -> list[dict[str, Any]]:
        """List pull requests in a repository.

        Args:
            repo: Repository in owner/name format.
            state: Filter by state: open, closed, or merged.
            base: Filter by base branch name.
            head: Filter by head branch name.
            assignee: Filter by assignee login.
            limit: Maximum number of PRs to return (default 30).
        """
        return pr_list(repo, state=state, base=base, head=head, assignee=assignee, limit=limit)

    tool_pr_list.__name__ = "pr_list"

    @server.tool()
    def tool_pr_get(repo: str, pr_number: int) -> dict[str, Any]:
        """Get a single pull request by number.

        Args:
            repo: Repository in owner/name format.
            pr_number: The pull request number.
        """
        return pr_get(repo, pr_number)

    tool_pr_get.__name__ = "pr_get"

    @server.tool()
    def tool_pr_create(
        repo: str,
        title: str,
        body: str = "",
        base: str | None = None,
        head: str | None = None,
        draft: bool = False,
        labels: str | None = None,
        assignees: str | None = None,
    ) -> dict[str, Any]:
        """Create a new pull request.

        Args:
            repo: Repository in owner/name format.
            title: PR title.
            body: PR body (markdown).
            base: Base branch name (defaults to repository default branch).
            head: Head branch name (defaults to current branch).
            draft: Create as a draft PR.
            labels: Comma-separated label names to apply.
            assignees: Comma-separated GitHub logins to assign.
        """
        return pr_create(repo, title, body=body, base=base, head=head, draft=draft, labels=labels, assignees=assignees)

    tool_pr_create.__name__ = "pr_create"

    @server.tool()
    def tool_pr_edit(
        repo: str,
        pr_number: int,
        title: str | None = None,
        body: str | None = None,
        base: str | None = None,
        labels: str | None = None,
        assignees: str | None = None,
    ) -> dict[str, Any]:
        """Edit an existing pull request.

        Args:
            repo: Repository in owner/name format.
            pr_number: The pull request number.
            title: New title (omit to leave unchanged).
            body: New body (omit to leave unchanged).
            base: New base branch.
            labels: Comma-separated labels to set.
            assignees: Comma-separated logins to set as assignees.
        """
        return pr_edit(repo, pr_number, title=title, body=body, base=base, labels=labels, assignees=assignees)

    tool_pr_edit.__name__ = "pr_edit"

    @server.tool()
    def tool_pr_close(repo: str, pr_number: int, comment: str | None = None) -> dict[str, Any]:
        """Close a pull request without merging.

        Args:
            repo: Repository in owner/name format.
            pr_number: The pull request number to close.
            comment: Optional comment to post when closing.
        """
        return pr_close(repo, pr_number, comment=comment)

    tool_pr_close.__name__ = "pr_close"

    @server.tool()
    def tool_pr_merge(
        repo: str,
        pr_number: int,
        method: str = "merge",
        auto: bool = False,
        delete_branch: bool = False,
    ) -> dict[str, Any]:
        """Merge a pull request.

        Args:
            repo: Repository in owner/name format.
            pr_number: The pull request number to merge.
            method: Merge method: merge, squash, or rebase.
            auto: Enable auto-merge (merge when requirements are met).
            delete_branch: Delete the head branch after merging.
        """
        return pr_merge(repo, pr_number, method=method, auto=auto, delete_branch=delete_branch)

    tool_pr_merge.__name__ = "pr_merge"

    @server.tool()
    def tool_pr_review(
        repo: str,
        pr_number: int,
        action: str,
        body: str = "",
    ) -> dict[str, Any]:
        """Submit a review on a pull request.

        Args:
            repo: Repository in owner/name format.
            pr_number: The pull request number to review.
            action: Review action: approve, comment, or request-changes.
            body: Review comment body (required for comment and request-changes).
        """
        return pr_review(repo, pr_number, action, body=body)

    tool_pr_review.__name__ = "pr_review"

    @server.tool()
    def tool_pr_comment(repo: str, pr_number: int, body: str) -> dict[str, Any]:
        """Post a comment on a pull request.

        Args:
            repo: Repository in owner/name format.
            pr_number: The pull request number to comment on.
            body: Comment body text (markdown).
        """
        return pr_comment(repo, pr_number, body)

    tool_pr_comment.__name__ = "pr_comment"
