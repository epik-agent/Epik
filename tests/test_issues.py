"""Unit tests for issues.py."""
from __future__ import annotations

import pytest
from unittest.mock import patch

from epik_gh.errors import ValidationError
from epik_gh.issues import (
    issue_close,
    issue_comment,
    issue_create,
    issue_edit,
    issue_get,
    issue_list,
    issue_reopen,
)

REPO = "owner/repo"


def _mock_run(return_value):
    """Patch run_gh to return a fixed (ok, data, error) tuple."""
    return patch("epik_gh.issues.run_gh", return_value=(True, return_value, ""))


class TestIssueList:
    def test_happy_path(self):
        items = [{"number": 1, "title": "Bug", "state": "open"}]
        with _mock_run(items):
            result = issue_list(REPO)
        assert result == items

    def test_passes_state_filter(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, [], "")) as mock:
            issue_list(REPO, state="closed")
        args = mock.call_args[0]
        assert "--state" in args
        assert "closed" in args

    def test_passes_label_filter(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, [], "")) as mock:
            issue_list(REPO, labels="bug,enhancement")
        args = mock.call_args[0]
        assert "--label" in args
        assert "bug,enhancement" in args

    def test_invalid_state_raises_validation_error(self):
        with pytest.raises(ValidationError, match="state"):
            issue_list(REPO, state="invalid")

    def test_passes_limit(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, [], "")) as mock:
            issue_list(REPO, limit=10)
        args = mock.call_args[0]
        assert "10" in args


class TestIssueGet:
    def test_happy_path(self):
        issue = {"number": 42, "title": "Test issue", "state": "open"}
        with _mock_run(issue):
            result = issue_get(REPO, 42)
        assert result == issue

    def test_passes_correct_args(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, {}, "")) as mock:
            issue_get(REPO, 99)
        args = mock.call_args[0]
        assert "99" in args
        assert "--repo" in args
        assert REPO in args


class TestIssueCreate:
    def test_happy_path(self):
        created = {"number": 5, "title": "New bug", "url": "https://github.com/..."}
        with _mock_run(created):
            result = issue_create(REPO, "New bug")
        assert result == created

    def test_empty_title_raises_validation_error(self):
        with pytest.raises(ValidationError, match="title"):
            issue_create(REPO, "")

    def test_passes_labels(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, {}, "")) as mock:
            issue_create(REPO, "Title", labels="bug")
        args = mock.call_args[0]
        assert "--label" in args
        assert "bug" in args

    def test_passes_body(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, {}, "")) as mock:
            issue_create(REPO, "Title", body="Description text")
        args = mock.call_args[0]
        assert "--body" in args
        assert "Description text" in args


class TestIssueEdit:
    def test_happy_path(self):
        updated = {"number": 1, "title": "Updated", "url": "https://..."}
        with _mock_run(updated):
            result = issue_edit(REPO, 1, title="Updated")
        assert result == updated

    def test_passes_title(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, {}, "")) as mock:
            issue_edit(REPO, 1, title="New Title")
        args = mock.call_args[0]
        assert "--title" in args
        assert "New Title" in args

    def test_no_optional_args_when_not_provided(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, {}, "")) as mock:
            issue_edit(REPO, 1)
        args = mock.call_args[0]
        assert "--title" not in args
        assert "--body" not in args


class TestIssueClose:
    def test_happy_path(self):
        closed = {"number": 3, "state": "closed"}
        with _mock_run(closed):
            result = issue_close(REPO, 3)
        assert result == closed

    def test_passes_comment(self):
        with patch("epik_gh.issues.run_gh", return_value=(True, {}, "")) as mock:
            issue_close(REPO, 3, comment="Closing this")
        args = mock.call_args[0]
        assert "--comment" in args


class TestIssueReopen:
    def test_happy_path(self):
        reopened = {"number": 3, "state": "open"}
        with _mock_run(reopened):
            result = issue_reopen(REPO, 3)
        assert result == reopened


class TestIssueComment:
    def test_happy_path(self):
        result_data = {"url": "https://github.com/.../comments/1"}
        with _mock_run(result_data):
            result = issue_comment(REPO, 1, "A comment")
        assert result == result_data

    def test_empty_body_raises_validation_error(self):
        with pytest.raises(ValidationError, match="body"):
            issue_comment(REPO, 1, "")
