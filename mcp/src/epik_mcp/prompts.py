"""MCP prompts: summon Epik and converge a project on Epik on plugin-less surfaces.

Claude Desktop and CoWork don't load Claude Code plugins, so the persona and
the init spec travel with the server instead: ``summon-epik`` serves the
persona (the same text behind the plugin's ``/epik:design``), ``init-epik``
serves the convergence dialogue (the same text behind ``/epik:init``).

``setup-epik`` is retained as an alias of ``init-epik``: it is the name these
surfaces were told to reach for, and "set up" reads more naturally than "init"
where there is no ``/epik:init`` command to echo.
"""

from __future__ import annotations

from mcp.server.mcpserver import MCPServer

from .resources import load

INIT_DESCRIPTION = (
    "Converge this project on the correct Epik shape: create the repository,"
    " convert an existing one, or repair drift (gh auth, project enablement,"
    " the headless-build workflow, build secrets). Safe to run repeatedly."
)


def summon_epik() -> str:
    """The Epik persona: engagement instructions plus the grounding philosophy."""
    return load("persona.md") + "\n\n---\n\n" + load("theory-and-practice.md")


def init_epik() -> str:
    """The Epik init spec: detect, offer, fix, report — until the project converges."""
    return load("init.md")


def register(server: MCPServer) -> None:
    """Register all prompts with the MCP server."""

    @server.prompt(
        name="summon-epik",
        title="Summon Epik",
        description=(
            "Summon Epik, a software-design partner grounded in the"
            " Theory-and-Practice philosophy."
        ),
    )
    def prompt_summon_epik() -> str:
        return summon_epik()

    @server.prompt(
        name="init-epik",
        title="Initialize an Epik project",
        description=INIT_DESCRIPTION,
    )
    def prompt_init_epik() -> str:
        return init_epik()

    @server.prompt(
        name="setup-epik",
        title="Set up Epik",
        description=f"Alias of init-epik. {INIT_DESCRIPTION}",
    )
    def prompt_setup_epik() -> str:
        return init_epik()
