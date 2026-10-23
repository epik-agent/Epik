"""Unit tests for prs.py."""

from __future__ import annotations

from unittest.mock import patch

import pytest

from epik_mcp.errors import ValidationError
from epik_mcp.prs import pr_get, pr_list

REPO = "owner/repo"


def _mock_run(return_value):
    return patch("epik_mcp.prs.run_gh", return_value=(True, return_value, ""))


def test_pr_list_happy_path():
    items = [{"number": 1, "title": "Feature", "state": "open"}]
    with _mock_run(items):
        result = pr_list(REPO)
    assert result == items


def test_pr_list_invalid_state_raises_error():
    with pytest.raises(ValidationError, match="state"):
        pr_list(REPO, state="invalid")


def test_pr_list_passes_state():
    with patch("epik_mcp.prs.run_gh", return_value=(True, [], "")) as mock:
        pr_list(REPO, state="closed")
    args = mock.call_args[0]
    assert "closed" in args


def test_pr_list_passes_base_filter():
    with patch("epik_mcp.prs.run_gh", return_value=(True, [], "")) as mock:
        pr_list(REPO, base="main")
    args = mock.call_args[0]
    assert "--base" in args
    assert "main" in args


def test_pr_get_happy_path():
    pr = {"number": 10, "title": "My PR", "state": "open"}
    with _mock_run(pr):
        result = pr_get(REPO, 10)
    assert result == pr
