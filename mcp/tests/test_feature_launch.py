"""Unit tests for feature_launch.py (build module)."""

from __future__ import annotations

import io
import json
import urllib.error
from typing import Any
from unittest.mock import patch

import pytest

from epik_mcp.errors import EpikMcpError, ValidationError
from epik_mcp.feature_launch import (
    ANTHROPIC_VERSION,
    ROUTINE_BETA,
    ROUTINES_BASE_URL,
    feature_launch,
)

ROUTINE_ID = "routine-123"
ROUTINE_TOKEN = "sk-routine-secret"
SESSION_URL = "https://claude.ai/code/session_abc"


def _set_config(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("EPIK_ROUTINE_ID", ROUTINE_ID)
    monkeypatch.setenv("EPIK_ROUTINE_TOKEN", ROUTINE_TOKEN)


class _FakeResponse:
    def __init__(self, payload: dict[str, Any]) -> None:
        self._raw = json.dumps(payload).encode("utf-8")

    def read(self) -> bytes:
        return self._raw

    def __enter__(self) -> _FakeResponse:
        return self

    def __exit__(self, *args: object) -> None:
        return None


def test_feature_launch_success(monkeypatch: pytest.MonkeyPatch) -> None:
    _set_config(monkeypatch)
    captured: dict[str, Any] = {}

    def fake_urlopen(request: Any, timeout: int = 0) -> _FakeResponse:
        captured["request"] = request
        captured["timeout"] = timeout
        return _FakeResponse({"claude_code_session_url": SESSION_URL})

    with patch("urllib.request.urlopen", side_effect=fake_urlopen):
        result = feature_launch(42, "main", "feature/x")

    assert result == {
        "session_url": SESSION_URL,
        "routine_id": ROUTINE_ID,
        "feature": 42,
    }

    request = captured["request"]
    assert request.full_url == f"{ROUTINES_BASE_URL}/{ROUTINE_ID}/fire"
    assert request.get_method() == "POST"

    # Headers (urllib capitalizes header keys).
    assert request.get_header("Authorization") == f"Bearer {ROUTINE_TOKEN}"
    assert request.get_header("Anthropic-beta") == ROUTINE_BETA
    assert request.get_header("Anthropic-version") == ANTHROPIC_VERSION
    assert request.get_header("Content-type") == "application/json"

    # Body carries the feature number and branches.
    body = json.loads(request.data.decode("utf-8"))
    assert "#42" in body["text"]
    assert "main" in body["text"]
    assert "feature/x" in body["text"]


def test_feature_launch_missing_config(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("EPIK_ROUTINE_ID", raising=False)
    monkeypatch.delenv("EPIK_ROUTINE_TOKEN", raising=False)
    with pytest.raises(ValidationError, match="EPIK_ROUTINE_ID"):
        feature_launch(1, "main", "target")


def test_feature_launch_missing_token(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("EPIK_ROUTINE_ID", ROUTINE_ID)
    monkeypatch.delenv("EPIK_ROUTINE_TOKEN", raising=False)
    with pytest.raises(ValidationError, match="EPIK_ROUTINE_TOKEN"):
        feature_launch(1, "main", "target")


def test_feature_launch_http_error(monkeypatch: pytest.MonkeyPatch) -> None:
    _set_config(monkeypatch)

    def fake_urlopen(request: Any, timeout: int = 0) -> None:
        raise urllib.error.HTTPError(
            url=request.full_url,
            code=403,
            msg="Forbidden",
            hdrs=None,  # type: ignore[arg-type]
            fp=io.BytesIO(b'{"error": "no access"}'),
        )

    with (
        patch("urllib.request.urlopen", side_effect=fake_urlopen),
        pytest.raises(EpikMcpError, match="403"),
    ):
        feature_launch(7, "main", "target")


def test_feature_launch_missing_session_url(monkeypatch: pytest.MonkeyPatch) -> None:
    _set_config(monkeypatch)

    def fake_urlopen(request: Any, timeout: int = 0) -> _FakeResponse:
        return _FakeResponse({"something_else": "value"})

    with (
        patch("urllib.request.urlopen", side_effect=fake_urlopen),
        pytest.raises(EpikMcpError, match="claude_code_session_url"),
    ):
        feature_launch(7, "main", "target")
