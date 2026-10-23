"""Start an MCP server over stdio and check that it is the current EpikMCP.

Usage: python3 scripts/smoke_mcp.py COMMAND [ARG ...]

Sends initialize and tools/list, prints the tools the server offers, and exits
non-zero if the server fails to start, lacks a tool EpikMCP must have, or still
offers a tool the current EpikMCP has removed. Standard library only.
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading

REQUIRED = {"gh_raw", "issue_create", "issue_set_blocked_by", "run_list", "run_logs"}
REMOVED = {"feature_launch"}
TIMEOUT_SECONDS = 120


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    proc = subprocess.Popen(
        argv,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    timer = threading.Timer(TIMEOUT_SECONDS, proc.kill)
    timer.start()
    try:
        tools = _list_tools(proc)
    except RuntimeError as error:
        proc.kill()
        stderr = proc.stderr.read() if proc.stderr else ""
        print(f"FAIL: {error}", file=sys.stderr)
        if stderr.strip():
            print(stderr.strip()[-2000:], file=sys.stderr)
        return 1
    finally:
        timer.cancel()
        if proc.stdin:
            proc.stdin.close()
        proc.terminate()

    names = sorted(tool["name"] for tool in tools)
    print(f"{len(names)} tools: {', '.join(names)}")
    missing = REQUIRED - set(names)
    stale = REMOVED & set(names)
    if missing:
        print(f"FAIL: missing tools: {', '.join(sorted(missing))}", file=sys.stderr)
    if stale:
        print(
            f"FAIL: old server; still offers {', '.join(sorted(stale))}",
            file=sys.stderr,
        )
    if missing or stale:
        return 1
    print("OK: this is the current EpikMCP")
    return 0


def _list_tools(proc: subprocess.Popen[str]) -> list[dict]:
    _send(
        proc,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "epik-smoke", "version": "0"},
            },
        },
    )
    _read_result(proc, 1)
    _send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    _send(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    return _read_result(proc, 2)["tools"]


def _send(proc: subprocess.Popen[str], message: dict) -> None:
    assert proc.stdin is not None
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()


def _read_result(proc: subprocess.Popen[str], request_id: int) -> dict:
    assert proc.stdout is not None
    for line in proc.stdout:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        if message.get("id") != request_id:
            continue
        if "error" in message:
            raise RuntimeError(f"server error: {message['error']}")
        return message["result"]
    raise RuntimeError("server exited before answering")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
