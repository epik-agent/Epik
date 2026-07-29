"""Unit tests for prompts.py and the vendored resources behind them."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from epik_mcp.prompts import init_epik, register, summon_epik
from epik_mcp.resources import SOURCES, load

MCP_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = MCP_DIR.parent
RESOURCES_DIR = MCP_DIR / "src" / "epik_mcp" / "resources"


def test_load_known_resources():
    for name in SOURCES:
        text = load(name)
        assert text.strip(), f"vendored resource {name} is empty"


def test_load_unknown_resource_raises():
    with pytest.raises(KeyError):
        load("nonexistent.md")


def test_summon_epik_combines_persona_and_philosophy():
    text = summon_epik()
    assert "Hello, I'm Epik" in text
    assert "Programming as Theory Building" in text
    # Persona precedes philosophy.
    assert text.index("Hello, I'm Epik") < text.index("Programming as Theory Building")


def test_init_epik_is_the_convergence_dialogue():
    text = init_epik()
    assert "idempotent convergence" in text
    # The former doctor's behaviour is retained in full.
    assert "Detect everything before changing anything" in text
    assert "EPIK_BUILD_GH_TOKEN" in text


def test_init_epik_carries_the_project_spec():
    """Creation and repair share one spec, so the prompt must contain all of it."""
    text = init_epik()
    for item in (
        ".claude/settings.json",
        "CLAUDE.md",
        ".claude/loop.md",
        "docs/design-history/",
        "epik-build.yml",
        "~/.epik/config.json",
    ):
        assert item in text, f"init spec is missing {item}"


async def test_register_exposes_the_prompts():
    from mcp.server.mcpserver import MCPServer

    server = MCPServer("test")
    register(server)
    names = {p.name for p in await server.list_prompts()}
    assert {"summon-epik", "init-epik", "setup-epik"} <= names


async def test_prompt_descriptions_carry_no_internal_nomenclature():
    """Internal nomenclature grounds Epik's answers but never faces the user."""
    from mcp.server.mcpserver import MCPServer

    server = MCPServer("test")
    register(server)
    for prompt in await server.list_prompts():
        user_facing = f"{prompt.title or ''} {prompt.description or ''}"
        assert not re.search(r"theory.and.practice", user_facing, re.IGNORECASE), (
            f"{prompt.name} exposes internal nomenclature: {user_facing!r}"
        )


async def test_setup_epik_is_an_alias_of_init_epik():
    """Plugin-less surfaces were told to reach for setup-epik; keep it working."""
    from mcp.server.mcpserver import MCPServer

    server = MCPServer("test")
    register(server)
    rendered = {
        name: await server.get_prompt(name) for name in ("init-epik", "setup-epik")
    }
    texts = {
        name: "".join(
            message.content.text
            for message in result.messages
            if hasattr(message.content, "text")
        )
        for name, result in rendered.items()
    }
    assert texts["setup-epik"] == texts["init-epik"] == init_epik()


@pytest.mark.skipif(
    not (REPO_ROOT / "plugin").is_dir(),
    reason="canonical plugin sources not present (installed package)",
)
def test_vendored_resources_match_plugin_sources():
    """CI drift gate: vendored copies must equal their canonical plugin files."""
    for name, source_rel in SOURCES.items():
        source = REPO_ROOT / source_rel
        vendored = RESOURCES_DIR / name
        assert source.exists(), f"canonical source missing: {source_rel}"
        assert vendored.exists(), f"vendored copy missing: {name}"
        assert vendored.read_text(encoding="utf-8") == source.read_text(
            encoding="utf-8"
        ), (
            f"{name} drifted from {source_rel}; "
            "run: uv run python scripts/sync_resources.py"
        )
