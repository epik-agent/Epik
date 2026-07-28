"""Unit tests for prompts.py and the vendored resources behind them."""

from __future__ import annotations

from pathlib import Path

import pytest

from epik_mcp.prompts import register, setup_epik, summon_epik
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


def test_setup_epik_is_the_doctor():
    text = setup_epik()
    assert "detect" in text and "report" in text
    assert "EPIK_BUILD_GH_TOKEN" in text


async def test_register_exposes_both_prompts():
    from mcp.server.mcpserver import MCPServer

    server = MCPServer("test")
    register(server)
    names = {p.name for p in await server.list_prompts()}
    assert {"summon-epik", "setup-epik"} <= names


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
