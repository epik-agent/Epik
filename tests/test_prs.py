"""Unit tests for prs.py."""
from __future__ import annotations

import pytest
from unittest.mock import patch

from epik_gh.errors import ValidationError
from epik_gh.prs import (
    pr_close,
    pr_comment,
    pr_create,
    pr_edit,
    pr_get,
    pr_list,
    pr_merge,
    pr_review,
)

REPO = "owner/repo"


def _mock_run(return_value):
    return patch("epik_gh.prs.run_gh", return_value=(True, return_value, ""))


class TestPrList:
    def test_happy_path(self):
        items = [{"number": 1, "title": "Feature", "state": "open"}]
        with _mock_run(items):
            result = pr_list(REPO)
        assert result == items

    def test_invalid_state_raises_error(self):
        with pytest.raises(ValidationError, match="state"):
            pr_list(REPO, state="invalid")

    def test_passes_state(self):
        with patch("epik_gh.prs.run_gh", return_value=(True, [], "")) as mock:
            pr_list(REPO, state="closed")
        args = mock.call_args[0]
        assert "closed" in args

    def test_passes_base_filter(self):
        with patch("epik_gh.prs.run_gh", return_value=(True, [], "")) as mock:
            pr_list(REPO, base="main")
        args = mock.call_args[0]
        assert "--base" in args
        assert "main" in args


class TestPrGet:
    def test_happy_path(self):
        pr = {"number": 10, "title": "My PR", "state": "open"}
        with _mock_run(pr):
            result = pr_get(REPO, 10)
        assert result == pr


class TestPrCreate:
    def test_happy_path(self):
        created = {"number": 7, "title": "My PR", "url": "https://..."}
        with _mock_run(created):
            result = pr_create(REPO, "My PR")
        assert result == created

    def test_empty_title_raises_validation_error(self):
        with pytest.raises(ValidationError, match="title"):
            pr_create(REPO, "")

    def test_passes_draft_flag(self):
        with patch("epik_gh.prs.run_gh", return_value=(True, {}, "")) as mock:
            pr_create(REPO, "Draft PR", draft=True)
        args = mock.call_args[0]
        assert "--draft" in args

    def test_no_draft_flag_when_false(self):
        with patch("epik_gh.prs.run_gh", return_value=(True, {}, "")) as mock:
            pr_create(REPO, "Normal PR", draft=False)
        args = mock.call_args[0]
        assert "--draft" not in args


class TestPrEdit:
    def test_happy_path(self):
        updated = {"number": 1, "title": "Updated PR"}
        with _mock_run(updated):
            result = pr_edit(REPO, 1, title="Updated PR")
        assert result == updated


class TestPrClose:
    def test_happy_path(self):
        closed = {"number": 2, "state": "closed"}
        with _mock_run(closed):
            result = pr_close(REPO, 2)
        assert result == closed


class TestPrMerge:
    def test_merge_method(self):
        merged = {"number": 5, "state": "merged"}
        with _mock_run(merged):
            result = pr_merge(REPO, 5, method="merge")
        assert result == merged

    def test_squash_method(self):
        merged = {"number": 5, "state": "merged"}
        with _mock_run(merged):
            result = pr_merge(REPO, 5, method="squash")
        assert result == merged

    def test_rebase_method(self):
        merged = {"number": 5, "state": "merged"}
        with _mock_run(merged):
            result = pr_merge(REPO, 5, method="rebase")
        assert result == merged

    def test_invalid_method_raises_validation_error(self):
        with pytest.raises(ValidationError, match="method"):
            pr_merge(REPO, 5, method="fast-forward")

    def test_passes_method_flag(self):
        with patch("epik_gh.prs.run_gh", return_value=(True, {}, "")) as mock:
            pr_merge(REPO, 5, method="squash")
        args = mock.call_args[0]
        assert "--squash" in args

    def test_passes_delete_branch_flag(self):
        with patch("epik_gh.prs.run_gh", return_value=(True, {}, "")) as mock:
            pr_merge(REPO, 5, delete_branch=True)
        args = mock.call_args[0]
        assert "--delete-branch" in args


class TestPrReview:
    def test_approve(self):
        with _mock_run(""):
            result = pr_review(REPO, 1, "approve")
        assert result["action"] == "approve"

    def test_comment_requires_body(self):
        with pytest.raises(ValidationError, match="body"):
            pr_review(REPO, 1, "comment", body="")

    def test_request_changes_requires_body(self):
        with pytest.raises(ValidationError, match="body"):
            pr_review(REPO, 1, "request-changes", body="")

    def test_invalid_action_raises_validation_error(self):
        with pytest.raises(ValidationError, match="action"):
            pr_review(REPO, 1, "invalid-action")

    def test_comment_with_body_succeeds(self):
        with _mock_run(""):
            result = pr_review(REPO, 1, "comment", body="Looks good")
        assert result["action"] == "comment"


class TestPrComment:
    def test_happy_path(self):
        result_data = {"url": "https://github.com/.../comments/1"}
        with _mock_run(result_data):
            result = pr_comment(REPO, 1, "Nice work!")
        assert result == result_data

    def test_empty_body_raises_validation_error(self):
        with pytest.raises(ValidationError, match="body"):
            pr_comment(REPO, 1, "")
