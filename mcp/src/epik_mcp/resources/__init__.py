"""Vendored plugin-owned artifacts served by EpikMCP prompts.

The canonical copies live in the plugin (the policy layer); the MCP package
vendors them so prompts work offline on surfaces that don't load plugins.
``SOURCES`` maps each vendored file to its repo-relative canonical path —
``scripts/sync_resources.py`` copies them in, and a test fails when the
copies drift.
"""

from __future__ import annotations

from importlib import resources

# Vendored filename -> canonical path, relative to the repository root.
SOURCES = {
    "persona.md": "plugin/skills/summon/persona.md",
    "theory-and-practice.md": "plugin/skills/summon/theory-and-practice.md",
    "init.md": "plugin/skills/init-repository/init.md",
}


def load(name: str) -> str:
    """Return the text of a vendored resource by filename."""
    if name not in SOURCES:
        raise KeyError(f"unknown resource: {name}")
    return (resources.files(__package__) / name).read_text(encoding="utf-8")
