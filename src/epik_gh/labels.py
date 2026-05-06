"""Label tools for epik-gh."""
from __future__ import annotations

from typing import Any

from .errors import ValidationError
from .runner import run_gh


def label_list(repo: str) -> list[dict[str, Any]]:
    """List all labels in a repository.

    Args:
        repo: Repository in owner/name format.

    Returns:
        List of label objects with name, color, and description.
    """
    _, data, _ = run_gh(
        "label", "list", "--repo", repo,
        json_fields=["name", "color", "description"],
    )
    return data  # type: ignore[return-value]


def label_create(
    repo: str,
    name: str,
    color: str,
    description: str = "",
    force: bool = False,
) -> dict[str, Any]:
    """Create a label in a repository.

    Args:
        repo: Repository in owner/name format.
        name: Label name.
        color: Hex color code without the leading '#' (e.g. 'ff0000').
        description: Optional description for the label.
        force: If True, update the label if it already exists.

    Returns:
        The created or updated label object.
    """
    if not name:
        raise ValidationError("name is required")
    if not color:
        raise ValidationError("color is required")
    args = ["label", "create", name, "--repo", repo, "--color", color]
    if description:
        args.extend(["--description", description])
    if force:
        args.append("--force")
    run_gh(*args)
    return {"name": name, "color": color, "description": description}


def label_delete(repo: str, name: str, confirm: bool = True) -> dict[str, Any]:
    """Delete a label from a repository.

    Args:
        repo: Repository in owner/name format.
        name: Label name to delete.
        confirm: Must be True to confirm deletion (safety guard).

    Returns:
        Dict confirming deletion.
    """
    if not confirm:
        raise ValidationError("confirm must be True to delete a label")
    _, data, _ = run_gh("label", "delete", name, "--repo", repo, "--yes")
    return {"deleted": name, "repo": repo}


def register(server: Any) -> None:
    """Register all label tools with the MCP server."""

    @server.tool()
    def tool_label_list(repo: str) -> list[dict[str, Any]]:
        """List all labels in a repository.

        Args:
            repo: Repository in owner/name format.
        """
        return label_list(repo)

    tool_label_list.__name__ = "label_list"

    @server.tool()
    def tool_label_create(
        repo: str,
        name: str,
        color: str,
        description: str = "",
        force: bool = False,
    ) -> dict[str, Any]:
        """Create a label in a repository.

        Args:
            repo: Repository in owner/name format.
            name: Label name.
            color: Hex color code without '#' (e.g. 'ff0000').
            description: Optional description for the label.
            force: If True, update the label if it already exists.
        """
        return label_create(repo, name, color, description=description, force=force)

    tool_label_create.__name__ = "label_create"

    @server.tool()
    def tool_label_delete(repo: str, name: str, confirm: bool = True) -> dict[str, Any]:
        """Delete a label from a repository.

        Args:
            repo: Repository in owner/name format.
            name: Label name to delete.
            confirm: Must be True to confirm deletion (safety guard).
        """
        return label_delete(repo, name, confirm=confirm)

    tool_label_delete.__name__ = "label_delete"
