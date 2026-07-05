"""GitHub Projects V2 tools for epik-mcp.

Provides a high-level interface to GitHub Projects V2 via GraphQL.
The lookup chain (project node ID → field ID → option ID) is hidden
from callers — project_set_status(owner, number, issue_number, status)
is a single call. Stable IDs are cached in ~/.epik-mcp/cache.json.
"""

from __future__ import annotations

import json
from typing import Any

from mcp.server.fastmcp import FastMCP

from . import cache as _cache
from .errors import NotFoundError, ValidationError
from .runner import run_gh, split_repo

# ---------------------------------------------------------------------------
# Internal GraphQL helpers
# ---------------------------------------------------------------------------


def _gql(query: str, variables: dict[str, Any] | None = None) -> dict[str, Any]:
    """Execute a GraphQL query/mutation and return the 'data' portion."""
    args = ["api", "graphql", "-f", f"query={query}"]
    if variables:
        args.extend(["-f", f"variables={json.dumps(variables)}"])
    _, raw, _ = run_gh(*args)
    result = json.loads(raw)
    if "errors" in result:
        msgs = "; ".join(e.get("message", str(e)) for e in result["errors"])
        raise NotFoundError(f"GraphQL error: {msgs}")
    return result.get("data", {})  # type: ignore[return-value]


def _fetch_project_ids(project_owner: str, project_number: int) -> dict[str, Any]:
    """Query GitHub for the project node ID, status field ID, and option IDs.

    Returns a dict shaped::

        {
            "project_id": "<node id>",
            "fields": {
                "<field_name>": {
                    "id": "<field node id>",
                    "options": {"<option_name>": "<option id>", ...}
                }
            }
        }
    """
    query = """
    query($login: String!, $number: Int!) {
      user(login: $login) {
        projectV2(number: $number) {
          id
          fields(first: 20) {
            nodes {
              ... on ProjectV2SingleSelectField {
                id
                name
                options { id name }
              }
            }
          }
        }
      }
    }
    """
    data = _gql(query, {"login": project_owner, "number": project_number})
    project = data.get("user", {}).get("projectV2")

    if project is None:
        # Try org
        org_query = query.replace("user(login:", "organization(login:")
        data = _gql(org_query, {"login": project_owner, "number": project_number})
        project = data.get("organization", {}).get("projectV2")

    if not project:
        raise NotFoundError(
            f"Project #{project_number} not found for owner {project_owner!r}"
        )

    project_id: str = project["id"]
    fields: dict[str, Any] = {}
    for node in project.get("fields", {}).get("nodes", []):
        if not node:
            continue
        fname: str = node.get("name", "")
        fid: str = node.get("id", "")
        options: dict[str, str] = {
            opt["name"]: opt["id"] for opt in node.get("options", [])
        }
        if fname:
            fields[fname] = {"id": fid, "options": options}

    ids = {"project_id": project_id, "fields": fields}
    _cache.set_project_ids(project_owner, project_number, ids)
    return ids


def _get_project_ids(project_owner: str, project_number: int) -> dict[str, Any]:
    """Return project IDs from cache or by fetching from GitHub."""
    cached = _cache.get_project_ids(project_owner, project_number)
    if cached is not None:
        return cached
    return _fetch_project_ids(project_owner, project_number)


def _find_item_id(project_id: str, repo: str, issue_number: int) -> str | None:
    """Find the project item ID for a given issue (returns None if not in project)."""
    query = """
    query($projectId: ID!, $cursor: String) {
      node(id: $projectId) {
        ... on ProjectV2 {
          items(first: 100, after: $cursor) {
            pageInfo { hasNextPage endCursor }
            nodes {
              id
              content {
                ... on Issue {
                  number
                  repository { nameWithOwner }
                }
              }
            }
          }
        }
      }
    }
    """
    cursor = None
    owner_repo = repo.lower()
    while True:
        variables: dict[str, Any] = {"projectId": project_id}
        if cursor:
            variables["cursor"] = cursor
        data = _gql(query, variables)
        items_data = data.get("node", {}).get("items", {})
        for node in items_data.get("nodes", []):
            content = node.get("content") or {}
            repo_name = (
                (content.get("repository") or {}).get("nameWithOwner", "").lower()
            )
            if content.get("number") == issue_number and repo_name == owner_repo:
                return str(node["id"])
        page_info = items_data.get("pageInfo", {})
        if not page_info.get("hasNextPage"):
            break
        cursor = page_info.get("endCursor")
    return None


def _add_item_to_project(project_id: str, repo: str, issue_number: int) -> str:
    """Add an issue to a project and return the new item ID."""
    owner, name = split_repo(repo)
    query = """
    query($owner: String!, $repo: String!, $number: Int!) {
      repository(owner: $owner, name: $repo) {
        issue(number: $number) { id }
      }
    }
    """
    data = _gql(query, {"owner": owner, "repo": name, "number": issue_number})
    issue_id = data.get("repository", {}).get("issue", {}).get("id")
    if not issue_id:
        raise NotFoundError(f"Issue #{issue_number} not found in {repo}")

    mutation = """
    mutation($projectId: ID!, $contentId: ID!) {
      addProjectV2ItemById(input: {projectId: $projectId, contentId: $contentId}) {
        item { id }
      }
    }
    """
    result = _gql(mutation, {"projectId": project_id, "contentId": issue_id})
    item_id = result.get("addProjectV2ItemById", {}).get("item", {}).get("id")
    if not item_id:
        raise NotFoundError(f"Failed to add issue #{issue_number} to project")
    return str(item_id)


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def project_set_status(
    project_owner: str,
    project_number: int,
    repo: str,
    issue_number: int,
    status_name: str,
) -> dict[str, Any]:
    """Set the status of an issue in a GitHub Project V2.

    This is a high-level convenience function. It handles the full lookup chain:
    project node ID → status field ID → option ID → item ID, with caching of
    stable IDs to minimise API calls.

    Args:
        project_owner: GitHub login (user or org) that owns the project.
        project_number: The project number (visible in the project URL).
        repo: Repository in owner/name format where the issue lives.
        issue_number: The issue number to update.
        status_name: The status column name (e.g. 'In Progress', 'Done').

    Returns:
        Dict confirming the update with project, item, and field details.
    """
    ids = _get_project_ids(project_owner, project_number)
    project_id: str = ids["project_id"]

    # Find Status field
    fields: dict[str, Any] = ids.get("fields", {})
    status_field: dict[str, Any] | None = fields.get("Status")
    if not status_field:
        raise ValidationError(
            f"Project #{project_number} has no 'Status' single-select field. "
            f"Available fields: {list(fields.keys())}"
        )
    field_id: str = status_field["id"]
    options: dict[str, str] = status_field.get("options", {})
    option_id = options.get(status_name)
    if not option_id:
        available = list(options.keys())
        raise ValidationError(
            f"Status {status_name!r} not found. Available statuses: {available}"
        )

    # Get or create item
    item_id = _find_item_id(project_id, repo, issue_number)
    if item_id is None:
        item_id = _add_item_to_project(project_id, repo, issue_number)

    # Set the field value
    mutation = """
    mutation($projectId: ID!, $itemId: ID!, $fieldId: ID!, $optionId: String!) {
      updateProjectV2ItemFieldValue(input: {
        projectId: $projectId,
        itemId: $itemId,
        fieldId: $fieldId,
        value: { singleSelectOptionId: $optionId }
      }) {
        projectV2Item { id }
      }
    }
    """
    _gql(
        mutation,
        {
            "projectId": project_id,
            "itemId": item_id,
            "fieldId": field_id,
            "optionId": option_id,
        },
    )

    return {
        "project_owner": project_owner,
        "project_number": project_number,
        "issue_number": issue_number,
        "status": status_name,
        "item_id": item_id,
    }


def project_get_item(
    project_owner: str,
    project_number: int,
    repo: str,
    issue_number: int,
) -> dict[str, Any]:
    """Get the project item for an issue, including its current field values.

    Args:
        project_owner: GitHub login (user or org) that owns the project.
        project_number: The project number.
        repo: Repository in owner/name format.
        issue_number: The issue number.

    Returns:
        Project item object with field values, or a not-found indicator.
    """
    ids = _get_project_ids(project_owner, project_number)
    project_id = ids["project_id"]
    item_id = _find_item_id(project_id, repo, issue_number)
    if not item_id:
        return {"found": False, "issue_number": issue_number, "repo": repo}

    query = """
    query($itemId: ID!) {
      node(id: $itemId) {
        ... on ProjectV2Item {
          id
          fieldValues(first: 20) {
            nodes {
              ... on ProjectV2ItemFieldSingleSelectValue {
                name
                field { ... on ProjectV2SingleSelectField { name } }
              }
              ... on ProjectV2ItemFieldTextValue {
                text
                field { ... on ProjectV2Field { name } }
              }
            }
          }
        }
      }
    }
    """
    data = _gql(query, {"itemId": item_id})
    item = data.get("node", {})
    field_values: list[dict[str, Any]] = []
    for node in item.get("fieldValues", {}).get("nodes", []):
        if not node:
            continue
        field_values.append(node)

    return {
        "found": True,
        "item_id": item_id,
        "issue_number": issue_number,
        "repo": repo,
        "field_values": field_values,
    }


def project_list_items(
    project_owner: str,
    project_number: int,
    status_filter: str | None = None,
) -> list[dict[str, Any]]:
    """List items in a GitHub Project V2, optionally filtered by status.

    Args:
        project_owner: GitHub login (user or org) that owns the project.
        project_number: The project number.
        status_filter: If provided, only return items with this status name.

    Returns:
        List of project items with their issue number, repo, and status.
    """
    ids = _get_project_ids(project_owner, project_number)
    project_id = ids["project_id"]

    query = """
    query($projectId: ID!, $cursor: String) {
      node(id: $projectId) {
        ... on ProjectV2 {
          items(first: 100, after: $cursor) {
            pageInfo { hasNextPage endCursor }
            nodes {
              id
              content {
                ... on Issue {
                  number
                  title
                  url
                  repository { nameWithOwner }
                }
              }
              fieldValues(first: 10) {
                nodes {
                  ... on ProjectV2ItemFieldSingleSelectValue {
                    name
                    field { ... on ProjectV2SingleSelectField { name } }
                  }
                }
              }
            }
          }
        }
      }
    }
    """
    results: list[dict[str, Any]] = []
    cursor = None
    while True:
        variables: dict[str, Any] = {"projectId": project_id}
        if cursor:
            variables["cursor"] = cursor
        data = _gql(query, variables)
        items_data = data.get("node", {}).get("items", {})
        for node in items_data.get("nodes", []):
            if not node:
                continue
            content = node.get("content") or {}
            if not content:
                continue
            status: str | None = None
            for fv in node.get("fieldValues", {}).get("nodes", []):
                if not fv:
                    continue
                field_info = fv.get("field") or {}
                if field_info.get("name") == "Status":
                    status = fv.get("name")
                    break
            item = {
                "item_id": node["id"],
                "issue_number": content.get("number"),
                "title": content.get("title"),
                "url": content.get("url"),
                "repo": (content.get("repository") or {}).get("nameWithOwner"),
                "status": status,
            }
            if status_filter is None or status == status_filter:
                results.append(item)
        page_info = items_data.get("pageInfo", {})
        if not page_info.get("hasNextPage"):
            break
        cursor = page_info.get("endCursor")
    return results


def project_invalidate_cache(
    project_owner: str,
    project_number: int | None = None,
) -> dict[str, Any]:
    """Invalidate the cached project IDs so they are re-fetched on next use.

    Use this when project columns or fields have changed and the cached IDs are stale.

    Args:
        project_owner: GitHub login (user or org) that owns the project.
        project_number: If provided, only invalidate this project. If omitted,
            all cached project IDs for this owner are cleared (full cache wipe).

    Returns:
        Dict confirming what was invalidated.
    """
    if project_number is not None:
        _cache.invalidate_project(project_owner, project_number)
        return {"invalidated": f"{project_owner}/{project_number}"}
    _cache.invalidate_all()
    return {"invalidated": "all"}


def register(server: FastMCP) -> None:
    """Register all Projects V2 tools with the MCP server."""

    @server.tool(name="project_set_status")
    def tool_project_set_status(
        project_owner: str,
        project_number: int,
        repo: str,
        issue_number: int,
        status_name: str,
    ) -> dict[str, Any]:
        """Set the status of an issue in a GitHub Project V2.

        Handles the full lookup chain (project ID, field ID, option ID, item ID)
        internally. Stable IDs are cached for subsequent calls.

        Args:
            project_owner: GitHub login (user or org) that owns the project.
            project_number: The project number (visible in the project URL).
            repo: Repository in owner/name format where the issue lives.
            issue_number: The issue number to update.
            status_name: The status column name (e.g. 'In Progress', 'Done').
        """
        return project_set_status(
            project_owner, project_number, repo, issue_number, status_name
        )

    @server.tool(name="project_get_item")
    def tool_project_get_item(
        project_owner: str,
        project_number: int,
        repo: str,
        issue_number: int,
    ) -> dict[str, Any]:
        """Get the project item for an issue, including its current field values.

        Args:
            project_owner: GitHub login (user or org) that owns the project.
            project_number: The project number.
            repo: Repository in owner/name format.
            issue_number: The issue number.
        """
        return project_get_item(project_owner, project_number, repo, issue_number)

    @server.tool(name="project_list_items")
    def tool_project_list_items(
        project_owner: str,
        project_number: int,
        status_filter: str | None = None,
    ) -> list[dict[str, Any]]:
        """List items in a GitHub Project V2, optionally filtered by status.

        Args:
            project_owner: GitHub login (user or org) that owns the project.
            project_number: The project number.
            status_filter: If provided, only return items with this status name.
        """
        return project_list_items(
            project_owner, project_number, status_filter=status_filter
        )

    @server.tool(name="project_invalidate_cache")
    def tool_project_invalidate_cache(
        project_owner: str,
        project_number: int | None = None,
    ) -> dict[str, Any]:
        """Invalidate cached project IDs so they are re-fetched on next use.

        Use when project columns or fields have changed and cached IDs are stale.

        Args:
            project_owner: GitHub login (user or org) that owns the project.
            project_number: If provided, only invalidate this project.
                If omitted, all cached data is cleared.
        """
        return project_invalidate_cache(project_owner, project_number)
