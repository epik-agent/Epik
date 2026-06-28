"""Issue relationship tools for epik-gh.

These tools manage GitHub issue relationships:

* Blocked-by dependencies use the GitHub REST issue-dependencies API
  (``repos/{owner}/{repo}/issues/{number}/dependencies/blocked_by``).
* Sub-issue hierarchy reads/writes use the GraphQL ``sub_issues`` surface, which
  requires the ``GraphQL-Features: sub_issues`` request header.
"""

from __future__ import annotations

import json
from typing import Any

from mcp.server.fastmcp import FastMCP

from .errors import ValidationError
from .runner import run_gh, split_repo


def _gql(
    query: str,
    *,
    features: str | None = None,
    **variables: str | int,
) -> dict[str, Any]:
    """Run a GraphQL query/mutation, passing variables as individual -F flags.

    Args:
        query: The GraphQL query or mutation document.
        features: Optional value for the ``GraphQL-Features`` header. When given,
            ``-H "GraphQL-Features: <features>"`` is appended to the call (this is
            required for the sub-issues surface, e.g. ``features="sub_issues"``).
        **variables: GraphQL variables. Integers are passed with ``-F`` (so gh
            coerces them to numbers); strings are passed with ``-f``.

    Returns:
        The parsed JSON response from gh.
    """
    args: list[str] = ["api", "graphql", "-f", f"query={query}"]
    if features:
        args.extend(["-H", f"GraphQL-Features: {features}"])
    for key, value in variables.items():
        # -F coerces integers; -f keeps strings
        flag = "-F" if isinstance(value, int) else "-f"
        args.extend([flag, f"{key}={value}"])
    _, data, _ = run_gh(*args)
    return json.loads(data)


def _issue_node_id(repo: str, issue_number: int) -> str:
    """Look up the GraphQL node ID for an issue."""
    owner, name = split_repo(repo)
    query = """
    query($owner: String!, $name: String!, $number: Int!) {
      repository(owner: $owner, name: $name) {
        issue(number: $number) {
          id
        }
      }
    }
    """
    result = _gql(query, owner=owner, name=name, number=issue_number)
    node_id = result.get("data", {}).get("repository", {}).get("issue", {}).get("id")
    if not node_id:
        raise ValidationError(f"Issue #{issue_number} not found in {repo}")
    return str(node_id)


def _issue_rest_id(repo: str, issue_number: int) -> int:
    """Look up the numeric REST id for an issue.

    This is the ``id`` field from ``GET repos/{owner}/{repo}/issues/{number}``,
    which is globally unique (NOT the per-repo issue number and NOT the GraphQL
    node id). It is the identifier accepted by the issue-dependencies API.
    """
    owner, name = split_repo(repo)
    _, data, _ = run_gh(
        "api",
        f"repos/{owner}/{name}/issues/{issue_number}",
        json_fields=["id"],
    )
    result: dict[str, Any] = data if isinstance(data, dict) else {}
    rest_id = result.get("id")
    if rest_id is None:
        raise ValidationError(f"Issue #{issue_number} not found in {repo}")
    return int(rest_id)


def issue_set_blocked_by(
    repo: str,
    issue_number: int,
    blocked_by_number: int,
    blocked_by_repo: str | None = None,
) -> dict[str, Any]:
    """Mark an issue as blocked by another issue.

    Uses the GitHub REST issue-dependencies API. The blocking issue's numeric
    REST id is resolved first (which is globally unique, so this also handles the
    cross-repo case).

    Args:
        repo: Repository in owner/name format for the blocked issue.
        issue_number: The issue that is being blocked.
        blocked_by_number: The issue that is doing the blocking.
        blocked_by_repo: Repository for the blocking issue (defaults to repo).

    Returns:
        Dict confirming the relationship was created.
    """
    owner, name = split_repo(repo)
    blocking_repo = blocked_by_repo or repo
    blocking_id = _issue_rest_id(blocking_repo, blocked_by_number)
    payload = json.dumps({"issue_id": blocking_id})
    run_gh(
        "api",
        f"repos/{owner}/{name}/issues/{issue_number}/dependencies/blocked_by",
        "-X",
        "POST",
        "--input",
        "-",
        input_data=payload,
    )
    return {
        "issue": issue_number,
        "blocked_by": blocked_by_number,
        "relationship": "blocked_by",
        "repo": repo,
        "blocked_by_repo": blocking_repo,
    }


def issue_remove_blocked_by(
    repo: str,
    issue_number: int,
    blocked_by_number: int,
    blocked_by_repo: str | None = None,
) -> dict[str, Any]:
    """Remove a blocked-by relationship between two issues.

    Uses the GitHub REST issue-dependencies API. The blocking issue's numeric
    REST id is resolved first (globally unique, so cross-repo works too).

    Args:
        repo: Repository in owner/name format for the blocked issue.
        issue_number: The issue that was being blocked.
        blocked_by_number: The issue that was doing the blocking.
        blocked_by_repo: Repository for the blocking issue (defaults to repo).

    Returns:
        Dict confirming the relationship was removed.
    """
    owner, name = split_repo(repo)
    blocking_repo = blocked_by_repo or repo
    blocking_id = _issue_rest_id(blocking_repo, blocked_by_number)
    run_gh(
        "api",
        f"repos/{owner}/{name}/issues/{issue_number}/dependencies/blocked_by/{blocking_id}",
        "-X",
        "DELETE",
    )
    return {
        "issue": issue_number,
        "removed_blocked_by": blocked_by_number,
        "repo": repo,
        "blocked_by_repo": blocking_repo,
    }


def issue_list_relationships(repo: str, issue_number: int) -> dict[str, Any]:
    """List the sub-issue hierarchy for an issue (parent and sub-issues).

    Uses the GraphQL ``sub_issues`` surface (requires the
    ``GraphQL-Features: sub_issues`` header).

    Args:
        repo: Repository in owner/name format.
        issue_number: The issue number to query.

    Returns:
        Dict with ``parent`` (or None) and ``sub_issues`` (a list).
    """
    owner, name = split_repo(repo)
    query = """
    query($owner: String!, $name: String!, $number: Int!) {
      repository(owner: $owner, name: $name) {
        issue(number: $number) {
          parent { number title url }
          subIssues(first: 50) {
            nodes { number title url state }
          }
        }
      }
    }
    """
    result = _gql(
        query,
        features="sub_issues",
        owner=owner,
        name=name,
        number=issue_number,
    )
    issue_data = result.get("data", {}).get("repository", {}).get("issue", {}) or {}
    parent = issue_data.get("parent")
    sub_issues = issue_data.get("subIssues", {}).get("nodes", [])
    return {
        "issue_number": issue_number,
        "repo": repo,
        "parent": parent,
        "sub_issues": sub_issues,
    }


def issue_add_sub_issue(
    repo: str, parent_issue_number: int, sub_issue_number: int
) -> dict[str, Any]:
    """Add a sub-issue to a parent issue.

    Uses the GraphQL ``addSubIssue`` mutation (requires the
    ``GraphQL-Features: sub_issues`` header).

    Args:
        repo: Repository in owner/name format.
        parent_issue_number: The parent issue number.
        sub_issue_number: The sub-issue number to attach.

    Returns:
        Dict confirming the sub-issue was added.
    """
    parent_id = _issue_node_id(repo, parent_issue_number)
    sub_id = _issue_node_id(repo, sub_issue_number)
    mutation = """
    mutation($parentId: ID!, $subId: ID!) {
      addSubIssue(input: {issueId: $parentId, subIssueId: $subId}) {
        issue { number }
        subIssue { number }
      }
    }
    """
    _gql(mutation, features="sub_issues", parentId=parent_id, subId=sub_id)
    return {
        "parent": parent_issue_number,
        "sub_issue": sub_issue_number,
        "repo": repo,
    }


def issue_remove_sub_issue(
    repo: str, parent_issue_number: int, sub_issue_number: int
) -> dict[str, Any]:
    """Remove a sub-issue from a parent issue.

    Uses the GraphQL ``removeSubIssue`` mutation (requires the
    ``GraphQL-Features: sub_issues`` header).

    Args:
        repo: Repository in owner/name format.
        parent_issue_number: The parent issue number.
        sub_issue_number: The sub-issue number to detach.

    Returns:
        Dict confirming the sub-issue was removed.
    """
    parent_id = _issue_node_id(repo, parent_issue_number)
    sub_id = _issue_node_id(repo, sub_issue_number)
    mutation = """
    mutation($parentId: ID!, $subId: ID!) {
      removeSubIssue(input: {issueId: $parentId, subIssueId: $subId}) {
        issue { number }
        subIssue { number }
      }
    }
    """
    _gql(mutation, features="sub_issues", parentId=parent_id, subId=sub_id)
    return {
        "parent": parent_issue_number,
        "removed_sub_issue": sub_issue_number,
        "repo": repo,
    }


def register(server: FastMCP) -> None:
    """Register all issue relationship tools with the MCP server."""

    @server.tool()
    def tool_issue_set_blocked_by(
        repo: str,
        issue_number: int,
        blocked_by_number: int,
        blocked_by_repo: str | None = None,
    ) -> dict[str, Any]:
        """Mark an issue as blocked by another issue.

        Args:
            repo: Repository in owner/name format for the blocked issue.
            issue_number: The issue that is being blocked.
            blocked_by_number: The issue that is doing the blocking.
            blocked_by_repo: Repository for the blocking issue in owner/name format.
                Defaults to repo (same-repo relationship).
        """
        return issue_set_blocked_by(
            repo, issue_number, blocked_by_number, blocked_by_repo=blocked_by_repo
        )

    tool_issue_set_blocked_by.__name__ = "issue_set_blocked_by"

    @server.tool()
    def tool_issue_remove_blocked_by(
        repo: str,
        issue_number: int,
        blocked_by_number: int,
        blocked_by_repo: str | None = None,
    ) -> dict[str, Any]:
        """Remove a blocked-by relationship between two issues.

        Args:
            repo: Repository in owner/name format for the blocked issue.
            issue_number: The issue that was being blocked.
            blocked_by_number: The issue that was doing the blocking.
            blocked_by_repo: Repository for the blocking issue in owner/name format.
                Defaults to repo (same-repo relationship).
        """
        return issue_remove_blocked_by(
            repo, issue_number, blocked_by_number, blocked_by_repo=blocked_by_repo
        )

    tool_issue_remove_blocked_by.__name__ = "issue_remove_blocked_by"

    @server.tool()
    def tool_issue_list_relationships(repo: str, issue_number: int) -> dict[str, Any]:
        """List the sub-issue hierarchy for an issue (parent and sub-issues).

        Args:
            repo: Repository in owner/name format.
            issue_number: The issue number to query.
        """
        return issue_list_relationships(repo, issue_number)

    tool_issue_list_relationships.__name__ = "issue_list_relationships"

    @server.tool()
    def tool_issue_add_sub_issue(
        repo: str, parent_issue_number: int, sub_issue_number: int
    ) -> dict[str, Any]:
        """Add a sub-issue to a parent issue using the GitHub GraphQL API.

        Args:
            repo: Repository in owner/name format.
            parent_issue_number: The parent issue number.
            sub_issue_number: The sub-issue number to attach.
        """
        return issue_add_sub_issue(repo, parent_issue_number, sub_issue_number)

    tool_issue_add_sub_issue.__name__ = "issue_add_sub_issue"

    @server.tool()
    def tool_issue_remove_sub_issue(
        repo: str, parent_issue_number: int, sub_issue_number: int
    ) -> dict[str, Any]:
        """Remove a sub-issue from a parent issue using the GitHub GraphQL API.

        Args:
            repo: Repository in owner/name format.
            parent_issue_number: The parent issue number.
            sub_issue_number: The sub-issue number to detach.
        """
        return issue_remove_sub_issue(repo, parent_issue_number, sub_issue_number)

    tool_issue_remove_sub_issue.__name__ = "issue_remove_sub_issue"
