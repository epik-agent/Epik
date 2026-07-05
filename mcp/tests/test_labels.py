"""Unit tests for labels.py."""

from __future__ import annotations

from unittest.mock import patch

import pytest

from epik_mcp.errors import ValidationError
from epik_mcp.labels import label_create, label_delete, label_list

REPO = "owner/repo"


def _mock_run(return_value):
    return patch("epik_mcp.labels.run_gh", return_value=(True, return_value, ""))


def test_label_list_happy_path():
    items = [{"name": "bug", "color": "d73a4a", "description": "Something is broken"}]
    with _mock_run(items):
        result = label_list(REPO)
    assert result == items


def test_label_list_passes_repo():
    with patch("epik_mcp.labels.run_gh", return_value=(True, [], "")) as mock:
        label_list(REPO)
    args = mock.call_args[0]
    assert "--repo" in args
    assert REPO in args


def test_label_create_happy_path():
    with _mock_run(""):
        result = label_create(REPO, "wip", "f9d0c4")
    assert result == {"name": "wip", "color": "f9d0c4", "description": ""}


def test_label_create_empty_name_raises_validation_error():
    with pytest.raises(ValidationError, match="name"):
        label_create(REPO, "", "ff0000")


def test_label_create_empty_color_raises_validation_error():
    with pytest.raises(ValidationError, match="color"):
        label_create(REPO, "bug", "")


def test_label_create_passes_force_flag():
    with patch("epik_mcp.labels.run_gh", return_value=(True, {}, "")) as mock:
        label_create(REPO, "bug", "ff0000", force=True)
    args = mock.call_args[0]
    assert "--force" in args


def test_label_create_no_force_flag_when_false():
    with patch("epik_mcp.labels.run_gh", return_value=(True, {}, "")) as mock:
        label_create(REPO, "bug", "ff0000", force=False)
    args = mock.call_args[0]
    assert "--force" not in args


def test_label_create_passes_description():
    with patch("epik_mcp.labels.run_gh", return_value=(True, {}, "")) as mock:
        label_create(REPO, "bug", "ff0000", description="A bug label")
    args = mock.call_args[0]
    assert "--description" in args


def test_label_delete_happy_path():
    with _mock_run(""):
        result = label_delete(REPO, "wip")
    assert result["deleted"] == "wip"
    assert result["repo"] == REPO


def test_label_delete_confirm_false_raises_validation_error():
    with pytest.raises(ValidationError, match="confirm"):
        label_delete(REPO, "wip", confirm=False)


def test_label_delete_passes_yes_flag():
    with patch("epik_mcp.labels.run_gh", return_value=(True, "", "")) as mock:
        label_delete(REPO, "bug")
    args = mock.call_args[0]
    assert "--yes" in args
