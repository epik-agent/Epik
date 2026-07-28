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
    "persona.md": "plugin/skills/design/persona.md",
    "theory-and-practice.md": "plugin/skills/design/theory-and-practice.md",
    "doctor.md": "plugin/skills/doctor/doctor.md",
}


def load(name: str) -> str:
    """Return the text of a vendored resource by filename."""
    if name not in SOURCES:
        raise KeyError(f"unknown resource: {name}")
    return (resources.files(__package__) / name).read_text(encoding="utf-8")
