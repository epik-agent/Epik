"""Feature launch tool (build module, Anthropic side).

Starts a remote feature build by firing a saved Claude Code routine via the
routines API. This module is intentionally self-contained: it does NOT use the
``gh`` runner and keeps its auth/state separate from the plan module. HTTP is
done with the Python stdlib ``urllib`` so no new dependency is added.

Configuration (environment):
    EPIK_ROUTINE_ID:    The Claude Code routine id whose fire endpoint is called.
    EPIK_ROUTINE_TOKEN: The routine API bearer token (Authorization header).

Both must be set; otherwise a ``ValidationError`` is raised.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from typing import Any

from mcp.server.fastmcp import FastMCP

from .errors import EpikMcpError, ValidationError

#: Beta header value required by the routines fire endpoint.
ROUTINE_BETA: str = "experimental-cc-routine-2026-04-01"

#: Anthropic API version header value.
ANTHROPIC_VERSION: str = "2023-06-01"

#: Base URL for the routines API.
ROUTINES_BASE_URL: str = "https://api.anthropic.com/v1/claude_code/routines"

#: Timeout (seconds) for the fire request.
_REQUEST_TIMEOUT: int = 60

#: Maximum number of error-body characters to include in raised errors.
_MAX_ERROR_BODY: int = 2000

_ENV_ROUTINE_ID = "EPIK_ROUTINE_ID"
_ENV_ROUTINE_TOKEN = "EPIK_ROUTINE_TOKEN"


def _load_config() -> tuple[str, str]:
    """Read routine id and token from the environment.

    Returns:
        A ``(routine_id, routine_token)`` tuple.

    Raises:
        ValidationError: If either environment variable is missing or empty.
    """
    routine_id = os.environ.get(_ENV_ROUTINE_ID, "").strip()
    routine_token = os.environ.get(_ENV_ROUTINE_TOKEN, "").strip()
    if not routine_id:
        raise ValidationError(
            f"{_ENV_ROUTINE_ID} is not set. Set it to the Claude Code routine id "
            "to fire (see README build module section)."
        )
    if not routine_token:
        raise ValidationError(
            f"{_ENV_ROUTINE_TOKEN} is not set. Set it to the routine API bearer "
            "token (see README build module section)."
        )
    return routine_id, routine_token


def feature_launch(
    feature_issue_number: int,
    base_branch: str,
    target_branch: str,
) -> dict[str, Any]:
    """Start a remote feature build by firing the configured routine.

    POSTs to the routine's fire endpoint with a JSON body carrying a ``text``
    instruction. The instruction format is::

        Build feature #<n> from base branch '<base>' targeting '<target>'.

    Args:
        feature_issue_number: The feature issue number to build.
        base_branch: The branch the build starts from.
        target_branch: The branch the build targets.

    Returns:
        A dict with the started session URL and context::

            {"session_url": <str>, "routine_id": <str>, "feature": <int>}

    Raises:
        ValidationError: If required config env vars are missing.
        EpikMcpError: If the request fails (non-2xx, connection error) or the
            response is missing ``claude_code_session_url``.
    """
    routine_id, routine_token = _load_config()

    text = (
        f"Build feature #{feature_issue_number} from base branch "
        f"'{base_branch}' targeting '{target_branch}'."
    )
    body = json.dumps({"text": text}).encode("utf-8")
    url = f"{ROUTINES_BASE_URL}/{routine_id}/fire"
    request = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={
            "Authorization": f"Bearer {routine_token}",
            "anthropic-beta": ROUTINE_BETA,
            "anthropic-version": ANTHROPIC_VERSION,
            "content-type": "application/json",
        },
    )

    try:
        with urllib.request.urlopen(request, timeout=_REQUEST_TIMEOUT) as response:
            raw = response.read()
    except urllib.error.HTTPError as exc:
        detail = ""
        try:
            detail = exc.read().decode("utf-8", errors="replace")
        except Exception:
            detail = ""
        if len(detail) > _MAX_ERROR_BODY:
            detail = detail[:_MAX_ERROR_BODY] + "... (truncated)"
        raise EpikMcpError(
            f"Routine fire failed with HTTP {exc.code} for routine "
            f"{routine_id!r}: {detail}"
        ) from exc
    except urllib.error.URLError as exc:
        raise EpikMcpError(
            f"Could not reach the routines API for routine {routine_id!r}: {exc.reason}"
        ) from exc

    try:
        data: dict[str, Any] = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError) as exc:
        raise EpikMcpError(
            f"Routine fire returned a non-JSON response for routine {routine_id!r}."
        ) from exc

    session_url = data.get("claude_code_session_url")
    if not session_url:
        raise EpikMcpError(
            "Routine fire response did not include 'claude_code_session_url' "
            f"for routine {routine_id!r}."
        )

    return {
        "session_url": session_url,
        "routine_id": routine_id,
        "feature": feature_issue_number,
    }


def register(server: FastMCP) -> None:
    """Register the feature_launch tool with the MCP server."""

    @server.tool(name="feature_launch")
    def tool_feature_launch(
        feature_issue_number: int,
        base_branch: str,
        target_branch: str,
    ) -> dict[str, Any]:
        """Start a remote feature build via the configured Claude Code routine.

        Args:
            feature_issue_number: The feature issue number to build.
            base_branch: The branch the build starts from.
            target_branch: The branch the build targets.
        """
        return feature_launch(feature_issue_number, base_branch, target_branch)
