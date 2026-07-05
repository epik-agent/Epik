"""MCP server entrypoint for EpikMCP.

EpikMCP is the single MCP for Epik with two internal modules: a plan module
(GitHub access via the gh CLI) and a build module (launch feature builds via
Anthropic). Registers all tool modules and starts the server.
"""

from __future__ import annotations

from mcp.server.fastmcp import FastMCP

# Import each module's ``register`` directly. The package ``__init__``
# re-exports the ``feature_launch`` and ``feature_status`` *functions*, which
# would otherwise shadow the same-named submodules under ``from . import ...``.
from .feature_launch import register as register_feature_launch
from .feature_status import register as register_feature_status
from .issues import register as register_issues
from .labels import register as register_labels
from .projects import register as register_projects
from .prs import register as register_prs
from .raw import register as register_raw
from .relationships import register as register_relationships
from .repos import register as register_repos
from .runs import register as register_runs

mcp = FastMCP(
    "epik-mcp",
    instructions=(
        "EpikMCP is the single MCP for Epik, with two internal modules. The plan"
        " module wraps the gh CLI for read/write GitHub access (issues,"
        " relationships, projects, labels, repos, read-only PRs and runs). The"
        " build module launches feature builds via an Anthropic routine."
    ),
)


def _register_all() -> None:
    """Call register(mcp) for every tool module."""
    for register in (
        register_issues,
        register_prs,
        register_runs,
        register_labels,
        register_repos,
        register_relationships,
        register_projects,
        register_raw,
        register_feature_launch,
        register_feature_status,
    ):
        register(mcp)


_register_all()


def main() -> None:
    """Run the EpikMCP server over stdio."""
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()
