"""GraphQL issue relationship tools for epik-gh.

These tools manage GitHub issue relationships (blocked-by, sub-issues) using
the GraphQL API via `gh api graphql`.
"""

from __future__ import annotations

import json
from typing import Any

from mcp.server.fastmcp import FastMCP

from .errors import ValidationError
from .runner import run_gh, split_repo


def _issue_node_id(repo: str, issue_number: int) -> str:
    """Look up the GraphQL node ID for an issue."""
    owner, name = split_repo(repo)
    query = """
    query($owner: String!, $repo: String!, $number: Int!) {
      repository(owner: $owner, name: $repo) {
        issue(number: $number) {
          id
        }
      }
    }
    """
    variables = json.dumps({"owner": owner, "repo": name, "number": issue_number})
    _, data, _ = run_gh(
        "api",
        "graphql",
        "-f",
        f"query={query}",
        "-f",
        f"variables={variables}",
    )
    result = json.loads(data)
    node_id = result.get("data", {}).get("repository", {}).get("issue", {}).get("id")
    if not node_id:
        raise ValidationError(f"Issue #{issue_number} not found in {repo}")
    return str(node_id)


def issue_set_blocked_by(
    repo: str,
    issue_number: int,
    blocked_by_number: int,
    blocked_by_repo: str | None = None,
) -> dict[str, Any]:
    """Mark an issue as blocked by another issue.

    Uses the GitHub GraphQL API to create a blocked-by relationship between
    two issues, optionally in different repositories.

    Args:
        repo: Repository in owner/name format for the blocked issue.
        issue_number: The issue that is being blocked.
        blocked_by_number: The issue that is doing the blocking.
        blocked_by_repo: Repository for the blocking issue (defaults to repo).

    Returns:
        Dict confirming the relationship was created.
    """
    issue_id = _issue_node_id(repo, issue_number)
    blocking_id = _issue_node_id(blocked_by_repo or repo, blocked_by_number)
    mutation = """
    mutation($issueId: ID!, $blockingId: ID!) {
      addIssueRelationship(input: {
        issueId: $issueId,
        relatedIssueId: $blockingId,
        relationshipType: BLOCKED_BY
      }) {
        issueRelationship {
          type
        }
      }
    }
    """
    variables = json.dumps({"issueId": issue_id, "blockingId": blocking_id})
    run_gh(
        "api",
        "graphql",
        "-f",
        f"query={mutation}",
        "-f",
        f"variables={variables}",
    )
    return {
        "issue": issue_number,
        "blocked_by": blocked_by_number,
        "relationship": "blocked_by",
        "repo": repo,
        "blocked_by_repo": blocked_by_repo or repo,
    }


def issue_remove_blocked_by(
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
        blocked_by_repo: Repository for the blocking issue (defaults to repo).

    Returns:
        Dict confirming the relationship was removed.
    """
    issue_id = _issue_node_id(repo, issue_number)
    blocking_id = _issue_node_id(blocked_by_repo or repo, blocked_by_number)
    mutation = """
    mutation($issueId: ID!, $blockingId: ID!) {
      removeIssueRelationship(input: {
        issueId: $issueId,
        relatedIssueId: $blockingId,
        relationshipType: BLOCKED_BY
      }) {
        clientMutationId
      }
    }
    """
    variables = json.dumps({"issueId": issue_id, "blockingId": blocking_id})
    run_gh(
        "api",
        "graphql",
        "-f",
        f"query={mutation}",
        "-f",
        f"variables={variables}",
    )
    return {
        "issue": issue_number,
        "removed_blocked_by": blocked_by_number,
        "repo": repo,
        "blocked_by_repo": blocked_by_repo or repo,
    }


def issue_list_relationships(repo: str, issue_number: int) -> dict[str, Any]:
    """List all relationships for an issue (blocked-by and sub-issues).

    Args:
        repo: Repository in owner/name format.
        issue_number: The issue number to query.

    Returns:
        Dict with 'blocked_by', 'blocking', and 'sub_issues' lists.
    """
    owner, name = split_repo(repo)
    query = """
    query($owner: String!, $repo: String!, $number: Int!) {
      repository(owner: $owner, name: $repo) {
        issue(number: $number) {
          id
          trackedInIssues(first: 25) {
            nodes { number title url }
          }
          trackedIssues(first: 25) {
            nodes { number title url }
          }
        }
      }
    }
    """
    variables = json.dumps({"owner": owner, "repo": name, "number": issue_number})
    _, data, _ = run_gh(
        "api",
        "graphql",
        "-f",
        f"query={query}",
        "-f",
        f"variables={variables}",
    )
    result = json.loads(data)
    issue_data = result.get("data", {}).get("repository", {}).get("issue", {})
    return {
        "issue_number": issue_number,
        "repo": repo,
        "tracked_in": issue_data.get("trackedInIssues", {}).get("nodes", []),
        "tracked_issues": issue_data.get("trackedIssues", {}).get("nodes", []),
    }


def issue_add_sub_issue(
    repo: str, parent_issue_number: int, sub_issue_number: int
) -> dict[str, Any]:
    """Add a sub-issue to a parent issue.

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
    variables = json.dumps({"parentId": parent_id, "subId": sub_id})
    run_gh(
        "api",
        "graphql",
        "-f",
        f"query={mutation}",
        "-f",
        f"variables={variables}",
    )
    return {
        "parent": parent_issue_number,
        "sub_issue": sub_issue_number,
        "repo": repo,
    }


def issue_remove_sub_issue(
    repo: str, parent_issue_number: int, sub_issue_number: int
) -> dict[str, Any]:
    """Remove a sub-issue from a parent issue.

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
    variables = json.dumps({"parentId": parent_id, "subId": sub_id})
    run_gh(
        "api",
        "graphql",
        "-f",
        f"query={mutation}",
        "-f",
        f"variables={variables}",
    )
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
        """Mark an issue as blocked by another issue using the GitHub GraphQL API.

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
        """List all relationships for an issue (blocked-by and sub-issues).

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
