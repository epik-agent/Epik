"""Assemble and pack the Epik MCP Bundle (.mcpb) for Claude Desktop.

Stages the epik-mcp source together with the bundle glue in this directory,
strips development dependencies, re-locks, and packs with the MCPB CLI.

Requirements: ``uv`` and ``npx`` (Node) on PATH. Network access for
``uv lock`` and the first ``npx @anthropic-ai/mcpb`` run.

Usage::

    python mcp/mcpb/build.py

Output: ``mcp/mcpb/dist/epik-mcp-<version>.mcpb`` where ``<version>`` is the
extension version in ``manifest.json`` (the extension is versioned by the
manifest, independently of the ``epik-mcp`` package version).
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent  # mcp/mcpb
MCP = HERE.parent  # mcp

BUNDLE_FILES = ["manifest.json", "main.py", ".mcpbignore", "icon.png"]


def _strip_dev_dependencies(pyproject: str) -> str:
    """Remove the [dependency-groups] section so only runtime deps install."""
    lines = pyproject.splitlines(keepends=True)
    out: list[str] = []
    skipping = False
    for line in lines:
        if line.rstrip() == "[dependency-groups]":
            skipping = True
            continue
        if skipping and line.startswith("["):
            skipping = False
        if not skipping:
            out.append(line)
    return "".join(out)


def main() -> int:
    version = json.loads((HERE / "manifest.json").read_text())["version"]
    dist = HERE / "dist"
    dist.mkdir(exist_ok=True)
    out = dist / f"epik-mcp-{version}.mcpb"

    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp) / "epik-mcpb"
        stage.mkdir()
        shutil.copytree(MCP / "src", stage / "src")
        shutil.copy(MCP / "README.md", stage / "README.md")
        (stage / "pyproject.toml").write_text(
            _strip_dev_dependencies((MCP / "pyproject.toml").read_text())
        )
        for name in BUNDLE_FILES:
            shutil.copy(HERE / name, stage / name)

        subprocess.run(["uv", "lock"], cwd=stage, check=True)
        subprocess.run(
            ["npx", "--yes", "@anthropic-ai/mcpb", "pack", str(stage), str(out)],
            check=True,
        )

    print(f"\nPacked {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
