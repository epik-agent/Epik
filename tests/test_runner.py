"""Unit tests for runner.py."""
from __future__ import annotations

import json
import subprocess
from unittest.mock import MagicMock, patch

import pytest

from epik_gh.errors import AuthError, GhError, NotFoundError, RateLimitError
from epik_gh.runner import run_gh


def _make_result(stdout: str = "", stderr: str = "", returncode: int = 0) -> MagicMock:
    result = MagicMock()
    result.stdout = stdout.encode()
    result.stderr = stderr.encode()
    result.returncode = returncode
    return result


class TestRunGhHappyPath:
    def test_returns_raw_stdout_without_json_fields(self):
        with patch("subprocess.run", return_value=_make_result(stdout="hello")) as mock_run:
            ok, data, err = run_gh("issue", "list")
        assert ok is True
        assert data == "hello"
        assert err == ""

    def test_parses_json_when_json_fields_provided(self):
        payload = json.dumps([{"number": 1, "title": "Test"}])
        with patch("subprocess.run", return_value=_make_result(stdout=payload)):
            ok, data, err = run_gh("issue", "list", json_fields=["number", "title"])
        assert ok is True
        assert data == [{"number": 1, "title": "Test"}]
        assert err == ""

    def test_appends_json_flag_to_command(self):
        payload = json.dumps({"number": 42})
        with patch("subprocess.run", return_value=_make_result(stdout=payload)) as mock_run:
            run_gh("issue", "view", "42", json_fields=["number"])
        call_args = mock_run.call_args[0][0]
        assert "--json" in call_args
        assert "number" in call_args

    def test_empty_json_output_returns_empty_dict(self):
        with patch("subprocess.run", return_value=_make_result(stdout="", returncode=0)):
            ok, data, err = run_gh("something", json_fields=["x"])
        assert ok is True
        assert data == {}

    def test_passes_input_data_to_stdin(self):
        with patch("subprocess.run", return_value=_make_result(stdout="ok")) as mock_run:
            run_gh("api", "something", input_data='{"key": "val"}')
        call_kwargs = mock_run.call_args[1]
        assert call_kwargs["input"] == b'{"key": "val"}'


class TestRunGhErrorClassification:
    def test_raises_auth_error_on_auth_login_in_stderr(self):
        with patch(
            "subprocess.run",
            return_value=_make_result(stderr="run gh auth login first", returncode=1),
        ):
            with pytest.raises(AuthError):
                run_gh("issue", "list")

    def test_raises_auth_error_on_authentication_in_stderr(self):
        with patch(
            "subprocess.run",
            return_value=_make_result(stderr="Authentication required", returncode=1),
        ):
            with pytest.raises(AuthError):
                run_gh("issue", "list")

    def test_raises_rate_limit_error_on_rate_limit_message(self):
        with patch(
            "subprocess.run",
            return_value=_make_result(stderr="API rate limit exceeded", returncode=1),
        ):
            with pytest.raises(RateLimitError):
                run_gh("issue", "list")

    def test_raises_not_found_error_on_not_found(self):
        with patch(
            "subprocess.run",
            return_value=_make_result(stderr="not found", returncode=1),
        ):
            with pytest.raises(NotFoundError):
                run_gh("issue", "view", "999")

    def test_raises_not_found_error_on_404_in_stderr(self):
        with patch(
            "subprocess.run",
            return_value=_make_result(stderr="HTTP 404: Not Found", returncode=1),
        ):
            with pytest.raises(NotFoundError):
                run_gh("issue", "view", "999")

    def test_raises_gh_error_on_generic_failure(self):
        with patch(
            "subprocess.run",
            return_value=_make_result(stderr="Something went wrong", returncode=1),
        ):
            with pytest.raises(GhError) as exc_info:
                run_gh("some", "command")
        assert exc_info.value.exit_code == 1
        assert "Something went wrong" in str(exc_info.value)

    def test_raises_gh_error_when_gh_not_found(self):
        with patch("subprocess.run", side_effect=FileNotFoundError):
            with pytest.raises(GhError, match="gh CLI not found"):
                run_gh("issue", "list")

    def test_raises_gh_error_on_timeout(self):
        with patch("subprocess.run", side_effect=subprocess.TimeoutExpired(cmd="gh", timeout=60)):
            with pytest.raises(GhError, match="timed out"):
                run_gh("issue", "list")

    def test_raises_gh_error_on_bad_json(self):
        with patch(
            "subprocess.run",
            return_value=_make_result(stdout="not json", returncode=0),
        ):
            with pytest.raises(GhError, match="parse gh JSON"):
                run_gh("issue", "list", json_fields=["number"])
