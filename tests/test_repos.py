"""Unit tests for repos.py."""
from __future__ import annotations

from unittest.mock import patch

from epik_gh.repos import repo_default_branch, repo_get

REPO = "owner/repo"


def _mock_run(return_value):
    return patch("epik_gh.repos.run_gh", return_value=(True, return_value, ""))


class TestRepoGet:
    def test_happy_path(self):
        repo_data = {
            "name": "repo",
            "fullName": "owner/repo",
            "isPrivate": False,
            "url": "https://github.com/owner/repo",
        }
        with _mock_run(repo_data):
            result = repo_get(REPO)
        assert result == repo_data

    def test_passes_repo_arg(self):
        with patch("epik_gh.repos.run_gh", return_value=(True, {}, "")) as mock:
            repo_get(REPO)
        args = mock.call_args[0]
        assert REPO in args


class TestRepoDefaultBranch:
    def test_happy_path_main(self):
        with _mock_run({"defaultBranchRef": {"name": "main"}}):
            result = repo_default_branch(REPO)
        assert result == "main"

    def test_happy_path_master(self):
        with _mock_run({"defaultBranchRef": {"name": "master"}}):
            result = repo_default_branch(REPO)
        assert result == "master"

    def test_missing_ref_returns_main(self):
        with _mock_run({}):
            result = repo_default_branch(REPO)
        assert result == "main"
