"""Launcher for the Epik MCP Bundle (.mcpb).

This file exists only in the bundle, not in the epik-mcp package. It does two
things the desktop-app launch environment needs:

1. PATH hardening. Claude Desktop starts MCP servers with a minimal PATH,
   and EpikMCP shells out to the ``gh`` CLI by bare name. Append the
   directories where ``gh`` is conventionally installed so the subprocess
   calls in ``epik_mcp.runner`` resolve it.
2. Start the server via the package entrypoint.
"""

from __future__ import annotations

import os
import sys


def _harden_path() -> None:
    home = os.path.expanduser("~")
    if sys.platform == "win32":
        sep = ";"
        candidates = [
            r"C:\Program Files\GitHub CLI",
            r"C:\Program Files (x86)\GitHub CLI",
            os.path.join(home, "scoop", "shims"),
        ]
    else:
        sep = ":"
        candidates = [
            "/opt/homebrew/bin",  # macOS, Apple Silicon
            "/usr/local/bin",  # macOS Intel, Linux
            "/usr/bin",
            os.path.join(home, ".local", "bin"),
            "/home/linuxbrew/.linuxbrew/bin",
        ]
    current = os.environ.get("PATH", "").split(sep) if os.environ.get("PATH") else []
    for directory in candidates:
        if directory not in current and os.path.isdir(directory):
            current.append(directory)
    os.environ["PATH"] = sep.join(current)


def run() -> None:
    _harden_path()
    from epik_mcp.server import main

    main()


if __name__ == "__main__":
    run()
