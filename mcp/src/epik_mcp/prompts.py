"""MCP prompts: summon Epik and set Epik up on plugin-less surfaces.

Claude Desktop and CoWork don't load Claude Code plugins, so the persona and
doctor travel with the server instead: ``summon-epik`` serves the persona
(the same text behind the plugin's ``/epik:design``), ``setup-epik`` serves
the doctor (the same text behind ``/epik:doctor``).
"""

from __future__ import annotations

from mcp.server.mcpserver import MCPServer

from .resources import load


def summon_epik() -> str:
    """The Epik persona: engagement instructions plus the grounding philosophy."""
    return load("persona.md") + "\n\n---\n\n" + load("theory-and-practice.md")


def setup_epik() -> str:
    """The Epik doctor: detect, offer, fix, and report on Epik's setup."""
    return load("doctor.md")


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
        name="setup-epik",
        title="Set up Epik",
        description=(
            "Have Epik check and repair its own setup: gh auth, project"
            " initialization, the headless-build workflow, and build secrets."
        ),
    )
    def prompt_setup_epik() -> str:
        return setup_epik()
