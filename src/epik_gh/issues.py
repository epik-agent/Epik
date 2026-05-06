"""Issue tools for epik-gh."""
from __future__ import annotations

from typing import Any

from .errors import ValidationError
from .runner import run_gh

_ISSUE_FIELDS = [
    "number",
    "title",
    "body",
    "state",
    "labels",
    "assignees",
    "author",
    "createdAt",
    "updatedAt",
    "url",
    "comments",
]


def issue_list(
    repo: str,
    state: str = "open",
    labels: str | None = None,
    assignee: str | None = None,
    limit: int = 30,
) -> list[dict[str, Any]]:
    """List issues in a GitHub repository.

    Args:
        repo: Repository in owner/name format.
        state: Filter by state: open, closed, or all.
        labels: Comma-separated list of label names to filter by.
        assignee: Filter by assignee login.
        limit: Maximum number of issues to return (default 30).

    Returns:
        List of issue objects.
    """
    if state not in ("open", "closed", "all"):
        raise ValidationError(f"state must be open, closed, or all; got {state!r}")
    args = ["issue", "list", "--repo", repo, "--state", state, "--limit", str(limit)]
    if labels:
        args.extend(["--label", labels])
    if assignee:
        args.extend(["--assignee", assignee])
    _, data, _ = run_gh(*args, json_fields=["number", "title", "state", "labels", "assignees", "url"])
    return data  # type: ignore[return-value]


def issue_get(repo: str, issue_number: int) -> dict[str, Any]:
    """Get a single issue by number.

    Args:
        repo: Repository in owner/name format.
        issue_number: The issue number.

    Returns:
        Issue object with full details.
    """
    _, data, _ = run_gh(
        "issue", "view", str(issue_number), "--repo", repo,
        json_fields=_ISSUE_FIELDS,
    )
    return data  # type: ignore[return-value]


def issue_create(
    repo: str,
    title: str,
    body: str = "",
    labels: str | None = None,
    assignees: str | None = None,
    milestone: str | None = None,
) -> dict[str, Any]:
    """Create a new issue in a repository.

    Args:
        repo: Repository in owner/name format.
        title: Issue title.
        body: Issue body (markdown).
        labels: Comma-separated label names to apply.
        assignees: Comma-separated GitHub logins to assign.
        milestone: Milestone title or number to associate.

    Returns:
        The created issue object.
    """
    if not title:
        raise ValidationError("title is required")
    args = ["issue", "create", "--repo", repo, "--title", title, "--body", body]
    if labels:
        args.extend(["--label", labels])
    if assignees:
        args.extend(["--assignee", assignees])
    if milestone:
        args.extend(["--milestone", milestone])
    _, data, _ = run_gh(*args, json_fields=["number", "title", "url"])
    return data  # type: ignore[return-value]


def issue_edit(
    repo: str,
    issue_number: int,
    title: str | None = None,
    body: str | None = None,
    labels: str | None = None,
    assignees: str | None = None,
    milestone: str | None = None,
) -> dict[str, Any]:
    """Edit an existing issue.

    Args:
        repo: Repository in owner/name format.
        issue_number: The issue number to edit.
        title: New title (omit to leave unchanged).
        body: New body (omit to leave unchanged).
        labels: Comma-separated labels to set (replaces current labels).
        assignees: Comma-separated logins to set as assignees.
        milestone: Milestone title or number to set.

    Returns:
        The updated issue object.
    """
    args = ["issue", "edit", str(issue_number), "--repo", repo]
    if title is not None:
        args.extend(["--title", title])
    if body is not None:
        args.extend(["--body", body])
    if labels is not None:
        args.extend(["--label", labels])
    if assignees is not None:
        args.extend(["--add-assignee", assignees])
    if milestone is not None:
        args.extend(["--milestone", milestone])
    _, data, _ = run_gh(*args, json_fields=["number", "title", "url"])
    return data  # type: ignore[return-value]


def issue_close(repo: str, issue_number: int, comment: str | None = None) -> dict[str, Any]:
    """Close an open issue.

    Args:
        repo: Repository in owner/name format.
        issue_number: The issue number to close.
        comment: Optional comment to post when closing.

    Returns:
        The updated issue object.
    """
    args = ["issue", "close", str(issue_number), "--repo", repo]
    if comment:
        args.extend(["--comment", comment])
    _, data, _ = run_gh(*args, json_fields=["number", "title", "state", "url"])
    return data  # type: ignore[return-value]


def issue_reopen(repo: str, issue_number: int, comment: str | None = None) -> dict[str, Any]:
    """Reopen a closed issue.

    Args:
        repo: Repository in owner/name format.
        issue_number: The issue number to reopen.
        comment: Optional comment to post when reopening.

    Returns:
        The updated issue object.
    """
    args = ["issue", "reopen", str(issue_number), "--repo", repo]
    if comment:
        args.extend(["--comment", comment])
    _, data, _ = run_gh(*args, json_fields=["number", "title", "state", "url"])
    return data  # type: ignore[return-value]


def issue_comment(repo: str, issue_number: int, body: str) -> dict[str, Any]:
    """Post a comment on an issue.

    Args:
        repo: Repository in owner/name format.
        issue_number: The issue number to comment on.
        body: Comment body text (markdown).

    Returns:
        Dict with the comment URL.
    """
    if not body:
        raise ValidationError("body is required")
    _, data, _ = run_gh(
        "issue", "comment", str(issue_number), "--repo", repo, "--body", body,
        json_fields=["url"],
    )
    return data  # type: ignore[return-value]


def register(server: Any) -> None:
    """Register all issue tools with the MCP server."""

    @server.tool()
    def tool_issue_list(
        repo: str,
        state: str = "open",
        labels: str | None = None,
        assignee: str | None = None,
        limit: int = 30,
    ) -> list[dict[str, Any]]:
        """List issues in a GitHub repository.

        Args:
            repo: Repository in owner/name format.
            state: Filter by state: open, closed, or all.
            labels: Comma-separated list of label names to filter by.
            assignee: Filter by assignee login.
            limit: Maximum number of issues to return (default 30).
        """
        return issue_list(repo, state=state, labels=labels, assignee=assignee, limit=limit)

    tool_issue_list.__name__ = "issue_list"

    @server.tool()
    def tool_issue_get(repo: str, issue_number: int) -> dict[str, Any]:
        """Get a single issue by number.

        Args:
            repo: Repository in owner/name format.
            issue_number: The issue number.
        """
        return issue_get(repo, issue_number)

    tool_issue_get.__name__ = "issue_get"

    @server.tool()
    def tool_issue_create(
        repo: str,
        title: str,
        body: str = "",
        labels: str | None = None,
        assignees: str | None = None,
        milestone: str | None = None,
    ) -> dict[str, Any]:
        """Create a new issue in a repository.

        Args:
            repo: Repository in owner/name format.
            title: Issue title.
            body: Issue body (markdown).
            labels: Comma-separated label names to apply.
            assignees: Comma-separated GitHub logins to assign.
            milestone: Milestone title or number to associate.
        """
        return issue_create(repo, title, body=body, labels=labels, assignees=assignees, milestone=milestone)

    tool_issue_create.__name__ = "issue_create"

    @server.tool()
    def tool_issue_edit(
        repo: str,
        issue_number: int,
        title: str | None = None,
        body: str | None = None,
        labels: str | None = None,
        assignees: str | None = None,
        milestone: str | None = None,
    ) -> dict[str, Any]:
        """Edit an existing issue.

        Args:
            repo: Repository in owner/name format.
            issue_number: The issue number to edit.
            title: New title (omit to leave unchanged).
            body: New body (omit to leave unchanged).
            labels: Comma-separated labels to set.
            assignees: Comma-separated logins to set as assignees.
            milestone: Milestone title or number to set.
        """
        return issue_edit(repo, issue_number, title=title, body=body, labels=labels, assignees=assignees, milestone=milestone)

    tool_issue_edit.__name__ = "issue_edit"

    @server.tool()
    def tool_issue_close(repo: str, issue_number: int, comment: str | None = None) -> dict[str, Any]:
        """Close an open issue.

        Args:
            repo: Repository in owner/name format.
            issue_number: The issue number to close.
            comment: Optional comment to post when closing.
        """
        return issue_close(repo, issue_number, comment=comment)

    tool_issue_close.__name__ = "issue_close"

    @server.tool()
    def tool_issue_reopen(repo: str, issue_number: int, comment: str | None = None) -> dict[str, Any]:
        """Reopen a closed issue.

        Args:
            repo: Repository in owner/name format.
            issue_number: The issue number to reopen.
            comment: Optional comment to post when reopening.
        """
        return issue_reopen(repo, issue_number, comment=comment)

    tool_issue_reopen.__name__ = "issue_reopen"

    @server.tool()
    def tool_issue_comment(repo: str, issue_number: int, body: str) -> dict[str, Any]:
        """Post a comment on an issue.

        Args:
            repo: Repository in owner/name format.
            issue_number: The issue number to comment on.
            body: Comment body text (markdown).
        """
        return issue_comment(repo, issue_number, body)

    tool_issue_comment.__name__ = "issue_comment"
