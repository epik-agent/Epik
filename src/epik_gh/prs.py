"""Pull request tools for epik-gh."""

from __future__ import annotations

from typing import Any

from mcp.server.fastmcp import FastMCP

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
        "pr",
        "view",
        str(pr_number),
        "--repo",
        repo,
        json_fields=_PR_FIELDS,
    )
    return data  # type: ignore[return-value]


def register(server: FastMCP) -> None:
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
        return pr_list(
            repo, state=state, base=base, head=head, assignee=assignee, limit=limit
        )

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
