"""Unit tests for feature_launch.py (build module)."""

from __future__ import annotations

from unittest.mock import patch

import pytest

from epik_mcp.errors import GhError, ValidationError
from epik_mcp.feature_launch import BUILD_WORKFLOW, feature_launch

REPO = "owner/repo"


def _mock_run(return_value=""):
    return patch(
        "epik_mcp.feature_launch.run_gh", return_value=(True, return_value, "")
    )


def test_feature_launch_dispatches_workflow():
    with _mock_run() as mock:
        result = feature_launch(REPO, 42, "main", "feature-42")

    args = mock.call_args[0]
    assert args[:3] == ("workflow", "run", BUILD_WORKFLOW)
    assert "--repo" in args
    assert REPO in args
    assert "--field" in args
    assert "feature_issue_number=42" in args
    assert "base_branch=main" in args
    assert "target_branch=feature-42" in args

    assert result["dispatched"] is True
    assert result["repo"] == REPO
    assert result["workflow"] == BUILD_WORKFLOW
    assert result["feature"] == 42
    assert result["base_branch"] == "main"
    assert result["target_branch"] == "feature-42"
    assert result["ref"] is None


def test_feature_launch_omits_ref_by_default():
    with _mock_run() as mock:
        feature_launch(REPO, 1, "main", "feature-1")
    args = mock.call_args[0]
    assert "--ref" not in args


def test_feature_launch_passes_ref():
    with _mock_run() as mock:
        result = feature_launch(REPO, 1, "main", "feature-1", ref="develop")
    args = mock.call_args[0]
    assert "--ref" in args
    assert "develop" in args
    assert result["ref"] == "develop"


def test_feature_launch_rejects_bad_repo():
    with pytest.raises(ValidationError, match="owner/name"):
        feature_launch("not-a-repo", 1, "main", "feature-1")


def test_feature_launch_rejects_nonpositive_issue_number():
    with pytest.raises(ValidationError, match="feature_issue_number"):
        feature_launch(REPO, 0, "main", "feature-1")


def test_feature_launch_rejects_empty_base_branch():
    with pytest.raises(ValidationError, match="base_branch"):
        feature_launch(REPO, 1, "  ", "feature-1")


def test_feature_launch_rejects_empty_target_branch():
    with pytest.raises(ValidationError, match="target_branch"):
        feature_launch(REPO, 1, "main", "")


def test_feature_launch_propagates_gh_error():
    with (
        patch(
            "epik_mcp.feature_launch.run_gh",
            side_effect=GhError("could not create workflow dispatch event"),
        ),
        pytest.raises(GhError, match="workflow dispatch"),
    ):
        feature_launch(REPO, 7, "main", "feature-7")
