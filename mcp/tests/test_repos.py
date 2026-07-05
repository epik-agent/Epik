"""Unit tests for repos.py."""

from __future__ import annotations

from unittest.mock import patch

from epik_mcp.repos import repo_default_branch, repo_get

REPO = "owner/repo"


def _mock_run(return_value):
    return patch("epik_mcp.repos.run_gh", return_value=(True, return_value, ""))


def test_repo_get_happy_path():
    repo_data = {
        "name": "repo",
        "fullName": "owner/repo",
        "isPrivate": False,
        "url": "https://github.com/owner/repo",
    }
    with _mock_run(repo_data):
        result = repo_get(REPO)
    assert result == repo_data


def test_repo_get_passes_repo_arg():
    with patch("epik_mcp.repos.run_gh", return_value=(True, {}, "")) as mock:
        repo_get(REPO)
    args = mock.call_args[0]
    assert REPO in args


def test_repo_default_branch_happy_path_main():
    with _mock_run({"defaultBranchRef": {"name": "main"}}):
        result = repo_default_branch(REPO)
    assert result == "main"


def test_repo_default_branch_happy_path_master():
    with _mock_run({"defaultBranchRef": {"name": "master"}}):
        result = repo_default_branch(REPO)
    assert result == "master"


def test_repo_default_branch_missing_ref_returns_main():
    with _mock_run({}):
        result = repo_default_branch(REPO)
    assert result == "main"
