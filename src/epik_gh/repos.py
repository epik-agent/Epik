"""Repository tools for epik-gh."""
from __future__ import annotations

from typing import Any

from .runner import run_gh

_REPO_FIELDS = [
    "name",
    "fullName",
    "description",
    "isPrivate",
    "isArchived",
    "isFork",
    "defaultBranchRef",
    "url",
    "sshUrl",
    "createdAt",
    "updatedAt",
    "stargazerCount",
    "forkCount",
    "primaryLanguage",
    "owner",
]


def repo_get(repo: str) -> dict[str, Any]:
    """Get metadata for a repository.

    Args:
        repo: Repository in owner/name format.

    Returns:
        Repository metadata object.
    """
    _, data, _ = run_gh("repo", "view", repo, json_fields=_REPO_FIELDS)
    return data  # type: ignore[return-value]


def repo_default_branch(repo: str) -> str:
    """Get the default branch name for a repository.

    Args:
        repo: Repository in owner/name format.

    Returns:
        The default branch name (e.g. 'main' or 'master').
    """
    _, data, _ = run_gh("repo", "view", repo, json_fields=["defaultBranchRef"])
    result: dict[str, Any] = data if isinstance(data, dict) else {}
    ref = result.get("defaultBranchRef") or {}
    return str(ref.get("name", "main"))


def register(server: Any) -> None:
    """Register all repository tools with the MCP server."""

    @server.tool()
    def tool_repo_get(repo: str) -> dict[str, Any]:
        """Get metadata for a repository.

        Args:
            repo: Repository in owner/name format.
        """
        return repo_get(repo)

    tool_repo_get.__name__ = "repo_get"

    @server.tool()
    def tool_repo_default_branch(repo: str) -> str:
        """Get the default branch name for a repository.

        Args:
            repo: Repository in owner/name format.
        """
        return repo_default_branch(repo)

    tool_repo_default_branch.__name__ = "repo_default_branch"
