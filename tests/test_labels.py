"""Unit tests for labels.py."""
from __future__ import annotations

import pytest
from unittest.mock import patch

from epik_gh.errors import ValidationError
from epik_gh.labels import label_create, label_delete, label_list

REPO = "owner/repo"


def _mock_run(return_value):
    return patch("epik_gh.labels.run_gh", return_value=(True, return_value, ""))


class TestLabelList:
    def test_happy_path(self):
        items = [{"name": "bug", "color": "d73a4a", "description": "Something is broken"}]
        with _mock_run(items):
            result = label_list(REPO)
        assert result == items

    def test_passes_repo(self):
        with patch("epik_gh.labels.run_gh", return_value=(True, [], "")) as mock:
            label_list(REPO)
        args = mock.call_args[0]
        assert "--repo" in args
        assert REPO in args


class TestLabelCreate:
    def test_happy_path(self):
        with _mock_run(""):
            result = label_create(REPO, "wip", "f9d0c4")
        assert result == {"name": "wip", "color": "f9d0c4", "description": ""}

    def test_empty_name_raises_validation_error(self):
        with pytest.raises(ValidationError, match="name"):
            label_create(REPO, "", "ff0000")

    def test_empty_color_raises_validation_error(self):
        with pytest.raises(ValidationError, match="color"):
            label_create(REPO, "bug", "")

    def test_passes_force_flag(self):
        with patch("epik_gh.labels.run_gh", return_value=(True, {}, "")) as mock:
            label_create(REPO, "bug", "ff0000", force=True)
        args = mock.call_args[0]
        assert "--force" in args

    def test_no_force_flag_when_false(self):
        with patch("epik_gh.labels.run_gh", return_value=(True, {}, "")) as mock:
            label_create(REPO, "bug", "ff0000", force=False)
        args = mock.call_args[0]
        assert "--force" not in args

    def test_passes_description(self):
        with patch("epik_gh.labels.run_gh", return_value=(True, {}, "")) as mock:
            label_create(REPO, "bug", "ff0000", description="A bug label")
        args = mock.call_args[0]
        assert "--description" in args


class TestLabelDelete:
    def test_happy_path(self):
        with _mock_run(""):
            result = label_delete(REPO, "wip")
        assert result["deleted"] == "wip"
        assert result["repo"] == REPO

    def test_confirm_false_raises_validation_error(self):
        with pytest.raises(ValidationError, match="confirm"):
            label_delete(REPO, "wip", confirm=False)

    def test_passes_yes_flag(self):
        with patch("epik_gh.labels.run_gh", return_value=(True, "", "")) as mock:
            label_delete(REPO, "bug")
        args = mock.call_args[0]
        assert "--yes" in args
