"""Raw gh passthrough tool for epik-mcp.

Exposes a single ``gh_raw`` tool that runs an arbitrary gh command, covering
operations not handled by the dedicated tools. Non-zero exits are surfaced as a
structured result rather than raised.
"""

from __future__ import annotations

from typing import Any

from mcp.server.fastmcp import FastMCP

from .runner import run_gh_raw


def gh_raw(args: list[str]) -> dict[str, Any]:
    """Run an arbitrary gh command and return its result without raising.

    This is an escape hatch for gh operations that do not have a dedicated tool
    (e.g. ``["release", "list", "--repo", "wpm/EpikMCP"]``). Pass the arguments
    as a list to avoid shell-quoting bugs; do not pass a single command string.

    Args:
        args: The list of arguments passed to gh, excluding the leading "gh".

    Returns:
        A dict with keys: ok (bool), exit_code (int), stdout (str), stderr (str).
    """
    exit_code, stdout, stderr = run_gh_raw(*args)
    return {
        "ok": exit_code == 0,
        "exit_code": exit_code,
        "stdout": stdout,
        "stderr": stderr,
    }


def register(server: FastMCP) -> None:
    """Register the raw passthrough tool with the MCP server."""

    @server.tool(name="gh_raw")
    def tool_gh_raw(args: list[str]) -> dict[str, Any]:
        """Run an arbitrary gh command and return its result without raising.

        Escape hatch for gh operations without a dedicated tool (e.g.
        ["release", "list", "--repo", "wpm/EpikMCP"]). Pass arguments as a list
        to avoid shell-quoting bugs; do not pass a single command string.

        Args:
            args: The list of arguments passed to gh, excluding the leading "gh".
        """
        return gh_raw(args)
