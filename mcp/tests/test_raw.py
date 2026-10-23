"""Unit tests for raw.py and the run_gh_raw runner helper."""

from __future__ import annotations

import subprocess
from unittest.mock import MagicMock, patch

from epik_mcp.raw import gh_raw
from epik_mcp.runner import run_gh_raw


def _make_result(stdout: str = "", stderr: str = "", returncode: int = 0) -> MagicMock:
    result = MagicMock()
    result.stdout = stdout.encode()
    result.stderr = stderr.encode()
    result.returncode = returncode
    return result


def test_gh_raw_success():
    with patch("epik_mcp.raw.run_gh_raw", return_value=(0, "v1.0.0\tLatest", "")):
        result = gh_raw(["release", "list"])
    assert result == {
        "ok": True,
        "exit_code": 0,
        "stdout": "v1.0.0\tLatest",
        "stderr": "",
    }


def test_gh_raw_failure_does_not_raise():
    with patch(
        "epik_mcp.raw.run_gh_raw",
        return_value=(1, "", "could not find any releases"),
    ):
        result = gh_raw(["release", "list", "--repo", "owner/missing"])
    assert result["ok"] is False
    assert result["exit_code"] == 1
    assert result["stderr"] == "could not find any releases"


def test_run_gh_raw_success():
    with patch("subprocess.run", return_value=_make_result(stdout="hello")) as mock_run:
        exit_code, stdout, stderr = run_gh_raw("release", "list")
    assert exit_code == 0
    assert stdout == "hello"
    assert stderr == ""
    assert mock_run.call_args[0][0] == ["gh", "release", "list"]


def test_run_gh_raw_nonzero_exit_does_not_raise():
    with patch(
        "subprocess.run",
        return_value=_make_result(stderr="boom", returncode=2),
    ):
        exit_code, stdout, stderr = run_gh_raw("bogus", "command")
    assert exit_code == 2
    assert stdout == ""
    assert stderr == "boom"


def test_run_gh_raw_gh_not_found():
    with patch("subprocess.run", side_effect=FileNotFoundError):
        exit_code, _, stderr = run_gh_raw("release", "list")
    assert exit_code != 0
    assert "gh CLI not found" in stderr


def test_run_gh_raw_timeout():
    with patch(
        "subprocess.run",
        side_effect=subprocess.TimeoutExpired(cmd="gh", timeout=60),
    ):
        exit_code, _, stderr = run_gh_raw("release", "list")
    assert exit_code != 0
    assert "timed out" in stderr
