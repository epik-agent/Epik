"""Subprocess wrapper for all gh CLI invocations.

This is the only module that knows about subprocesses. Every tool calls into it.
"""

from __future__ import annotations

import json
import subprocess
from typing import Any

from .errors import AuthError, GhError, NotFoundError, RateLimitError, ValidationError


def split_repo(repo: str) -> tuple[str, str]:
    """Split 'owner/name' into (owner, name), raising ValidationError on bad format."""
    parts = repo.split("/", 1)
    if len(parts) != 2 or not parts[0] or not parts[1]:
        raise ValidationError(f"repo must be in owner/name format; got {repo!r}")
    return parts[0], parts[1]


def run_gh(
    *args: str,
    json_fields: list[str] | None = None,
    input_data: str | None = None,
) -> tuple[bool, Any, str]:
    """Run a gh command and return (ok, data, error_message).

    Args:
        *args: Positional arguments to gh. The first arg is typically a subcommand,
            e.g. "issue", "list", "--repo", "owner/repo".
        json_fields: If provided, appends --json field1,field2,... to the command
            and parses stdout as JSON.
        input_data: If provided, passed to the subprocess on stdin.

    Returns:
        (ok, data, error_message) where data is parsed JSON when json_fields is
        given, or raw stdout string otherwise. error_message is empty on success.

    Raises:
        AuthError: gh is not authenticated.
        NotFoundError: The resource was not found or permission was denied.
        RateLimitError: GitHub rate limit exceeded.
        GhError: Any other non-zero exit from gh.
    """
    cmd: list[str] = ["gh", *args]
    if json_fields:
        cmd.extend(["--json", ",".join(json_fields)])

    stdin_input = input_data.encode() if input_data is not None else None

    try:
        result = subprocess.run(
            cmd,
            input=stdin_input,
            capture_output=True,
            timeout=60,
        )
    except FileNotFoundError as err:
        raise GhError(
            "gh CLI not found. Install it from https://cli.github.com/",
            stderr="",
            exit_code=-1,
        ) from err
    except subprocess.TimeoutExpired as err:
        raise GhError(
            "gh command timed out after 60 seconds", stderr="", exit_code=-1
        ) from err

    stdout = result.stdout.decode(errors="replace").strip()
    stderr = result.stderr.decode(errors="replace").strip()
    exit_code = result.returncode

    if exit_code == 0:
        if json_fields:
            try:
                data = json.loads(stdout) if stdout else {}
            except json.JSONDecodeError as exc:
                raise GhError(
                    f"Failed to parse gh JSON output: {exc}",
                    stderr=stderr,
                    exit_code=exit_code,
                ) from exc
        else:
            data = stdout
        return True, data, ""

    # --- error classification ---
    stderr_lower = stderr.lower()

    # Auth errors
    if (
        "gh auth login" in stderr_lower
        or "authentication" in stderr_lower
        or "not logged in" in stderr_lower
        or "401" in stderr
    ):
        raise AuthError()

    # Rate limit
    if "rate limit" in stderr_lower or exit_code == 429:
        raise RateLimitError()

    # Not found / permission denied
    if exit_code == 1 and (
        "not found" in stderr_lower
        or "could not resolve" in stderr_lower
        or "404" in stderr
        or "no such" in stderr_lower
        or "does not exist" in stderr_lower
    ):
        raise NotFoundError(stderr or f"Resource not found (exit {exit_code})")

    raise GhError(
        stderr or f"gh exited with code {exit_code}",
        stderr=stderr,
        exit_code=exit_code,
    )


def run_gh_raw(*args: str) -> tuple[int, str, str]:
    """Run an arbitrary gh command without raising on failure.

    Unlike :func:`run_gh`, this never raises or classifies errors. It returns the
    raw exit code, stdout, and stderr so callers can surface failures cleanly.
    FileNotFoundError and timeouts are mapped to a non-zero exit code with a
    message in stderr.

    Args:
        *args: Positional arguments passed directly to gh, e.g.
            "release", "list", "--repo", "owner/repo".

    Returns:
        (exit_code, stdout, stderr). exit_code is 0 on success.
    """
    cmd: list[str] = ["gh", *args]

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            timeout=60,
        )
    except FileNotFoundError:
        return (
            -1,
            "",
            "gh CLI not found. Install it from https://cli.github.com/",
        )
    except subprocess.TimeoutExpired:
        return (-1, "", "gh command timed out after 60 seconds")

    stdout = result.stdout.decode(errors="replace").strip()
    stderr = result.stderr.decode(errors="replace").strip()
    return (result.returncode, stdout, stderr)
