"""MCP prompts: summon Epik and set a repository up for Epik on plugin-less surfaces.

Claude Desktop and Cowork don't load Claude Code plugins, so the persona and
the setup spec travel with the server instead: ``summon-epik`` serves the
persona (the same text behind the plugin's ``/epik:summon``), ``init-repository``
serves the setup dialogue (the same text behind ``/epik:init-repository``).

Prompt names are already namespaced by the server, so they echo the commands
exactly; the branding lives in the server name and in each prompt's title.
"""

from __future__ import annotations

from mcp.server.mcpserver import MCPServer

from .resources import load

INIT_DESCRIPTION = (
    "Set this repository up for Epik, or repair one that has drifted: create"
    " the repository, convert an existing one, or fix what is missing (gh auth,"
    " project enablement, the headless-build workflow, build secrets). Safe to"
    " run any time."
)


def summon_epik() -> str:
    """The Epik persona: engagement instructions plus the grounding philosophy."""
    return load("persona.md") + "\n\n---\n\n" + load("theory-and-practice.md")


def init_repository() -> str:
    """The Epik setup spec: detect, offer, fix, report — until the project matches."""
    return load("init.md")


def register(server: MCPServer) -> None:
    """Register all prompts with the MCP server."""

    @server.prompt(
        name="summon-epik",
        title="Summon Epik",
        description="Summon Epik, your software-design partner.",
    )
    def prompt_summon_epik() -> str:
        return summon_epik()

    @server.prompt(
        name="init-repository",
        title="Set up a repository for Epik",
        description=INIT_DESCRIPTION,
    )
    def prompt_init_repository() -> str:
        return init_repository()
