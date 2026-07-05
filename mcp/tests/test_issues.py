"""Unit tests for issues.py."""

from __future__ import annotations

from unittest.mock import patch

import pytest

from epik_mcp.errors import ValidationError
from epik_mcp.issues import (
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
    return patch("epik_mcp.issues.run_gh", return_value=(True, return_value, ""))


def test_issue_list_happy_path():
    items = [{"number": 1, "title": "Bug", "state": "open"}]
    with _mock_run(items):
        result = issue_list(REPO)
    assert result == items


def test_issue_list_passes_state_filter():
    with patch("epik_mcp.issues.run_gh", return_value=(True, [], "")) as mock:
        issue_list(REPO, state="closed")
    args = mock.call_args[0]
    assert "--state" in args
    assert "closed" in args


def test_issue_list_passes_label_filter():
    with patch("epik_mcp.issues.run_gh", return_value=(True, [], "")) as mock:
        issue_list(REPO, labels="bug,enhancement")
    args = mock.call_args[0]
    assert "--label" in args
    assert "bug,enhancement" in args


def test_issue_list_invalid_state_raises_validation_error():
    with pytest.raises(ValidationError, match="state"):
        issue_list(REPO, state="invalid")


def test_issue_list_passes_limit():
    with patch("epik_mcp.issues.run_gh", return_value=(True, [], "")) as mock:
        issue_list(REPO, limit=10)
    args = mock.call_args[0]
    assert "10" in args


def test_issue_get_happy_path():
    issue = {"number": 42, "title": "Test issue", "state": "open"}
    with _mock_run(issue):
        result = issue_get(REPO, 42)
    assert result == issue


def test_issue_get_passes_correct_args():
    with patch("epik_mcp.issues.run_gh", return_value=(True, {}, "")) as mock:
        issue_get(REPO, 99)
    args = mock.call_args[0]
    assert "99" in args
    assert "--repo" in args
    assert REPO in args


def test_issue_create_happy_path():
    url = "https://github.com/owner/repo/issues/5"
    with _mock_run(url):
        result = issue_create(REPO, "New bug")
    assert result == {"url": url}


def test_issue_create_empty_title_raises_validation_error():
    with pytest.raises(ValidationError, match="title"):
        issue_create(REPO, "")


def test_issue_create_passes_labels():
    with patch("epik_mcp.issues.run_gh", return_value=(True, {}, "")) as mock:
        issue_create(REPO, "Title", labels="bug")
    args = mock.call_args[0]
    assert "--label" in args
    assert "bug" in args


def test_issue_create_passes_body():
    with patch("epik_mcp.issues.run_gh", return_value=(True, {}, "")) as mock:
        issue_create(REPO, "Title", body="Description text")
    args = mock.call_args[0]
    assert "--body" in args
    assert "Description text" in args


def test_issue_edit_happy_path():
    url = "https://github.com/owner/repo/issues/1"
    with _mock_run(url):
        result = issue_edit(REPO, 1, title="Updated")
    assert result == {"url": url}


def test_issue_edit_passes_title():
    with patch("epik_mcp.issues.run_gh", return_value=(True, {}, "")) as mock:
        issue_edit(REPO, 1, title="New Title")
    args = mock.call_args[0]
    assert "--title" in args
    assert "New Title" in args


def test_issue_edit_no_optional_args_when_not_provided():
    with patch("epik_mcp.issues.run_gh", return_value=(True, {}, "")) as mock:
        issue_edit(REPO, 1)
    args = mock.call_args[0]
    assert "--title" not in args
    assert "--body" not in args


def test_issue_close_happy_path():
    with _mock_run(""):
        result = issue_close(REPO, 3)
    assert result == {"number": 3, "state": "closed"}


def test_issue_close_passes_comment():
    with patch("epik_mcp.issues.run_gh", return_value=(True, {}, "")) as mock:
        issue_close(REPO, 3, comment="Closing this")
    args = mock.call_args[0]
    assert "--comment" in args


def test_issue_reopen_happy_path():
    with _mock_run(""):
        result = issue_reopen(REPO, 3)
    assert result == {"number": 3, "state": "open"}


def test_issue_comment_happy_path():
    url = "https://github.com/owner/repo/issues/1#issuecomment-1"
    with _mock_run(url):
        result = issue_comment(REPO, 1, "A comment")
    assert result == {"url": url}


def test_issue_comment_empty_body_raises_validation_error():
    with pytest.raises(ValidationError, match="body"):
        issue_comment(REPO, 1, "")
