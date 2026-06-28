"""Unit tests for feature_status.py."""

from __future__ import annotations

import json
from typing import Any
from unittest.mock import patch

from epik_gh.errors import GhError
from epik_gh.feature_status import feature_status

REPO = "owner/repo"


def _rel(sub_issues: list[dict[str, Any]]) -> dict[str, Any]:
    return {
        "issue_number": 100,
        "repo": REPO,
        "parent": None,
        "sub_issues": sub_issues,
    }


def test_feature_status_mixed_states():
    """A feature with sub-issues in mixed states yields a full per-issue table."""
    tracked = [
        {"number": 1, "title": "Foundation", "url": "u/1"},
        {"number": 2, "title": "Builds on 1", "url": "u/2"},
        {"number": 3, "title": "Untouched", "url": "u/3"},
    ]

    # State comes from issue_get (relationship nodes carry no state).
    issue_states = {
        1: {"state": "CLOSED"},
        2: {"state": "OPEN"},
        3: {"state": "OPEN"},
    }

    prs = [
        # #1: merged PR, CI success.
        {
            "number": 11,
            "title": "PR for 1",
            "state": "MERGED",
            "headRefName": "1-foundation",
            "baseRefName": "main",
            "url": "pr/11",
        },
        # #2: open PR, CI failure.
        {
            "number": 12,
            "title": "PR for 2",
            "state": "OPEN",
            "headRefName": "2-builds-on-1",
            "baseRefName": "main",
            "url": "pr/12",
        },
        # #3 has no PR.
    ]

    # CI runs keyed by branch.
    runs_by_branch = {
        "1-foundation": [{"conclusion": "success", "status": "completed"}],
        "2-builds-on-1": [{"conclusion": "failure", "status": "completed"}],
    }

    # Dependency edges: #2 is blocked by #1; #1 and #3 have none.
    # #2 also references #99 (not a sub-issue) which must be filtered out.
    deps_by_issue = {
        1: [],
        2: [{"number": 1}, {"number": 99}],
        3: [],
    }

    def fake_issue_get(repo: str, number: int) -> dict[str, Any]:
        return issue_states[number]

    def fake_pr_list(repo: str, **kwargs: Any) -> list[dict[str, Any]]:
        assert kwargs["state"] == "all"
        return prs

    def fake_run_list(
        repo: str, branch: str | None = None, **kwargs: Any
    ) -> list[dict[str, Any]]:
        return runs_by_branch.get(branch or "", [])

    def fake_run_gh(*args: str, **kwargs: Any) -> tuple[bool, str, str]:
        # args == ("api", "repos/owner/repo/issues/<n>/dependencies/blocked_by")
        path = args[1]
        number = int(path.split("/issues/")[1].split("/")[0])
        return (True, json.dumps(deps_by_issue[number]), "")

    with (
        patch(
            "epik_gh.feature_status.issue_list_relationships",
            return_value=_rel(tracked),
        ),
        patch("epik_gh.feature_status.issue_get", side_effect=fake_issue_get),
        patch("epik_gh.feature_status.pr_list", side_effect=fake_pr_list),
        patch("epik_gh.feature_status.run_list", side_effect=fake_run_list),
        patch("epik_gh.feature_status.run_gh", side_effect=fake_run_gh),
    ):
        result = feature_status(REPO, 100)

    assert result["feature"] == 100
    assert result["repo"] == REPO
    assert result["degraded"] is False
    assert len(result["issues"]) == 3

    by_num = {row["number"]: row for row in result["issues"]}

    # #1: closed, merged PR, CI success, root of the graph.
    assert by_num[1]["state"] == "CLOSED"
    assert by_num[1]["title"] == "Foundation"
    assert by_num[1]["linked_pr"] == {"number": 11, "state": "MERGED", "url": "pr/11"}
    assert by_num[1]["ci_conclusion"] == "success"
    assert by_num[1]["blocked_by"] == []
    assert by_num[1]["position"] == 0

    # #2: open, open PR, CI failure, blocked by #1 (out-of-feature #99 filtered).
    assert by_num[2]["state"] == "OPEN"
    assert by_num[2]["linked_pr"] == {"number": 12, "state": "OPEN", "url": "pr/12"}
    assert by_num[2]["ci_conclusion"] == "failure"
    assert by_num[2]["blocked_by"] == [1]
    assert by_num[2]["position"] == 1

    # #3: open, no PR, no CI, no deps.
    assert by_num[3]["state"] == "OPEN"
    assert by_num[3]["linked_pr"] is None
    assert by_num[3]["ci_conclusion"] is None
    assert by_num[3]["blocked_by"] == []
    assert by_num[3]["position"] == 0


def test_feature_status_degrades_when_relationships_fail():
    """If the relationship read raises, output is empty/flat and degraded=True."""
    with (
        patch(
            "epik_gh.feature_status.issue_list_relationships",
            side_effect=GhError("boom"),
        ),
        patch("epik_gh.feature_status.pr_list", return_value=[]),
    ):
        result = feature_status(REPO, 100)

    assert result["feature"] == 100
    assert result["repo"] == REPO
    assert result["degraded"] is True
    assert result["issues"] == []


def test_feature_status_degrades_when_dependencies_fail():
    """If dependency reads raise, rows keep basic state but graph data degrades."""
    tracked = [{"number": 1, "title": "A", "url": "u/1"}]

    with (
        patch(
            "epik_gh.feature_status.issue_list_relationships",
            return_value=_rel(tracked),
        ),
        patch("epik_gh.feature_status.issue_get", return_value={"state": "OPEN"}),
        patch("epik_gh.feature_status.pr_list", return_value=[]),
        patch("epik_gh.feature_status.run_list", return_value=[]),
        patch(
            "epik_gh.feature_status.run_gh", side_effect=GhError("no dependencies api")
        ),
    ):
        result = feature_status(REPO, 100)

    assert result["degraded"] is True
    row = result["issues"][0]
    assert row["number"] == 1
    assert row["state"] == "OPEN"
    assert row["blocked_by"] == []
    assert row["position"] is None
