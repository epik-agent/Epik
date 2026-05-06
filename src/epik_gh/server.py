"""MCP server entrypoint for epik-gh.

Registers all tool modules and starts the server.
"""
from __future__ import annotations

import traceback
from typing import Any

from mcp.server.fastmcp import FastMCP

from . import branches, issues, labels, prs, projects, relationships, repos, runs
from .errors import AuthError, EpikGhError, GhError, NotFoundError, RateLimitError, ValidationError

mcp = FastMCP(
    "epik-gh",
    description="GitHub MCP server wrapping the gh CLI. Provides read/write GitHub access via gh subcommands and the GitHub API.",
)


def _register_all() -> None:
    """Call register(mcp) for every tool module."""
    for module in (issues, prs, runs, labels, branches, repos, relationships, projects):
        module.register(mcp)


_register_all()


def main() -> None:
    """Run the epik-gh MCP server over stdio."""
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
