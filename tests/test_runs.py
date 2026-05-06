"""Unit tests for runs.py."""
from __future__ import annotations

from unittest.mock import patch

from epik_gh.runs import run_get, run_list, run_logs

REPO = "owner/repo"


def _mock_run(return_value):
    return patch("epik_gh.runs.run_gh", return_value=(True, return_value, ""))


class TestRunList:
    def test_happy_path(self):
        items = [
            {"databaseId": 1, "name": "CI", "status": "completed", "conclusion": "success"}
        ]
        with _mock_run(items):
            result = run_list(REPO)
        assert result == items

    def test_passes_workflow_filter(self):
        with patch("epik_gh.runs.run_gh", return_value=(True, [], "")) as mock:
            run_list(REPO, workflow="ci.yml")
        args = mock.call_args[0]
        assert "--workflow" in args
        assert "ci.yml" in args

    def test_passes_branch_filter(self):
        with patch("epik_gh.runs.run_gh", return_value=(True, [], "")) as mock:
            run_list(REPO, branch="main")
        args = mock.call_args[0]
        assert "--branch" in args
        assert "main" in args

    def test_passes_status_filter(self):
        with patch("epik_gh.runs.run_gh", return_value=(True, [], "")) as mock:
            run_list(REPO, status="failure")
        args = mock.call_args[0]
        assert "--status" in args
        assert "failure" in args

    def test_passes_limit(self):
        with patch("epik_gh.runs.run_gh", return_value=(True, [], "")) as mock:
            run_list(REPO, limit=5)
        args = mock.call_args[0]
        assert "5" in args


class TestRunGet:
    def test_happy_path(self):
        run = {"databaseId": 99, "name": "CI", "status": "completed"}
        with _mock_run(run):
            result = run_get(REPO, 99)
        assert result == run

    def test_passes_run_id(self):
        with patch("epik_gh.runs.run_gh", return_value=(True, {}, "")) as mock:
            run_get(REPO, 12345)
        args = mock.call_args[0]
        assert "12345" in args


class TestRunLogs:
    def test_full_logs(self):
        with _mock_run("Step 1\nStep 2\nSuccess"):
            result = run_logs(REPO, 42)
        assert "Step 1" in result

    def test_failed_only_logs(self):
        with patch("epik_gh.runs.run_gh", return_value=(True, "Error in step 3", "")) as mock:
            result = run_logs(REPO, 42, failed_only=True)
        args = mock.call_args[0]
        assert "--log-failed" in args

    def test_job_specific_logs(self):
        with patch("epik_gh.runs.run_gh", return_value=(True, "job output", "")) as mock:
            result = run_logs(REPO, 42, job_id=7)
        args = mock.call_args[0]
        assert "--job" in args
        assert "7" in args
        assert "--log" in args
