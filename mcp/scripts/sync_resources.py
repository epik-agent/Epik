"""Sync plugin-owned artifacts into the MCP package's vendored resources.

Run from the ``mcp/`` directory after editing any file listed in
``epik_mcp.resources.SOURCES``:

    uv run python scripts/sync_resources.py          # copy sources in
    uv run python scripts/sync_resources.py --check  # exit 1 on drift

The pytest suite runs the same comparison, so CI fails when a vendored copy
drifts from its canonical plugin source.
"""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

MCP_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = MCP_DIR.parent
RESOURCES_DIR = MCP_DIR / "src" / "epik_mcp" / "resources"

sys.path.insert(0, str(MCP_DIR / "src"))
from epik_mcp.resources import SOURCES  # noqa: E402


def main() -> int:
    check = "--check" in sys.argv[1:]
    drifted: list[str] = []
    for name, source_rel in SOURCES.items():
        source = REPO_ROOT / source_rel
        target = RESOURCES_DIR / name
        if not source.exists():
            print(f"missing canonical source: {source}", file=sys.stderr)
            return 1
        in_sync = target.exists() and target.read_text() == source.read_text()
        if in_sync:
            continue
        if check:
            drifted.append(f"{target.relative_to(MCP_DIR)} != {source_rel}")
        else:
            shutil.copyfile(source, target)
            print(f"synced {source_rel} -> {target.relative_to(MCP_DIR)}")
    if drifted:
        print("vendored resources drifted from plugin sources:", file=sys.stderr)
        for line in drifted:
            print(f"  {line}", file=sys.stderr)
        print("run: uv run python scripts/sync_resources.py", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
