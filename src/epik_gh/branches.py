"""Branch tools for epik-gh."""
from __future__ import annotations

import json
from typing import Any

from .errors import ValidationError
from .runner import run_gh, split_repo


def branch_list(
    repo: str,
    prefix: str | None = None,
    limit: int = 50,
) -> list[dict[str, Any]]:
    """List branches in a repository, optionally filtered by prefix.

    Args:
        repo: Repository in owner/name format.
        prefix: If provided, only return branches whose names start with this string.
        limit: Maximum number of branches to return (default 50).

    Returns:
        List of branch objects with name and commit SHA.
    """
    owner, name = split_repo(repo)
    _, data, _ = run_gh(
        "api", f"repos/{owner}/{name}/branches",
        "-X", "GET", "-F", f"per_page={limit}",
    )
    branches: list[dict[str, Any]] = json.loads(data)
    if prefix:
        branches = [b for b in branches if b.get("name", "").startswith(prefix)]
    return [{"name": b["name"], "sha": b["commit"]["sha"]} for b in branches]


def branch_create(repo: str, branch_name: str, ref: str) -> dict[str, Any]:
    """Create a new branch from an arbitrary ref (branch, tag, or commit SHA).

    Args:
        repo: Repository in owner/name format.
        branch_name: Name for the new branch.
        ref: The ref to branch from (branch name, tag name, or full commit SHA).

    Returns:
        Dict with the new branch name and the SHA it points to.
    """
    if not branch_name:
        raise ValidationError("branch_name is required")
    if not ref:
        raise ValidationError("ref is required")
    owner, name = split_repo(repo)
    sha = _resolve_ref(owner, name, ref)
    payload = json.dumps({"ref": f"refs/heads/{branch_name}", "sha": sha})
    _, data, _ = run_gh(
        "api", f"repos/{owner}/{name}/git/refs",
        "-X", "POST",
        "--input", "-",
        input_data=payload,
    )
    result = json.loads(data)
    return {"branch": branch_name, "sha": sha, "ref": result.get("ref", "")}


def branch_delete(repo: str, branch_name: str, force: bool = False) -> dict[str, Any]:
    """Delete a branch from a repository.

    Args:
        repo: Repository in owner/name format.
        branch_name: Name of the branch to delete.
        force: Must be True to confirm deletion.

    Returns:
        Dict confirming deletion.
    """
    if not force:
        raise ValidationError("force must be True to delete a branch")
    owner, name = split_repo(repo)
    run_gh(
        "api", f"repos/{owner}/{name}/git/refs/heads/{branch_name}",
        "-X", "DELETE",
    )
    return {"deleted": branch_name, "repo": repo}


def _resolve_ref(owner: str, name: str, ref: str) -> str:
    """Resolve a ref to a commit SHA using the GitHub API."""
    if len(ref) == 40 and all(c in "0123456789abcdef" for c in ref.lower()):
        return ref
    _, data, _ = run_gh(
        "api", f"repos/{owner}/{name}/commits/{ref}",
        json_fields=["sha"],
    )
    result: dict[str, Any] = data if isinstance(data, dict) else {}
    sha = result.get("sha", "")
    if not sha:
        raise ValidationError(f"Could not resolve ref {ref!r} to a commit SHA")
    return sha


def register(server: Any) -> None:
    """Register all branch tools with the MCP server."""

    @server.tool()
    def tool_branch_list(
        repo: str,
        prefix: str | None = None,
        limit: int = 50,
    ) -> list[dict[str, Any]]:
        """List branches in a repository, optionally filtered by prefix.

        Args:
            repo: Repository in owner/name format.
            prefix: If provided, only return branches whose names start with this string.
            limit: Maximum number of branches to return (default 50).
        """
        return branch_list(repo, prefix=prefix, limit=limit)

    tool_branch_list.__name__ = "branch_list"

    @server.tool()
    def tool_branch_create(repo: str, branch_name: str, ref: str) -> dict[str, Any]:
        """Create a new branch from an arbitrary ref (branch, tag, or commit SHA).

        Args:
            repo: Repository in owner/name format.
            branch_name: Name for the new branch.
            ref: The ref to branch from (branch name, tag name, or full commit SHA).
        """
        return branch_create(repo, branch_name, ref)

    tool_branch_create.__name__ = "branch_create"

    @server.tool()
    def tool_branch_delete(repo: str, branch_name: str, force: bool = False) -> dict[str, Any]:
        """Delete a branch from a repository.

        Args:
            repo: Repository in owner/name format.
            branch_name: Name of the branch to delete.
            force: Must be True to confirm deletion.
        """
        return branch_delete(repo, branch_name, force=force)

    tool_branch_delete.__name__ = "branch_delete"
