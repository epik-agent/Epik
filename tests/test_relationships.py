"""Unit tests for relationships.py."""

from __future__ import annotations

import json
from unittest.mock import patch

import pytest

from epik_gh.errors import ValidationError
from epik_gh.relationships import (
    issue_add_sub_issue,
    issue_list_relationships,
    issue_remove_blocked_by,
    issue_remove_sub_issue,
    issue_set_blocked_by,
)

REPO = "owner/repo"


def _node_id_raw(node_id: str) -> str:
    return json.dumps({"data": {"repository": {"issue": {"id": node_id}}}})


def _has_sub_issues_header(call_args: tuple) -> bool:
    """Return True if a call's positional args carry the sub_issues feature header."""
    joined = " ".join(str(a) for a in call_args)
    return "GraphQL-Features: sub_issues" in joined


# --------------------------------------------------------------------------- #
# set blocked_by
# --------------------------------------------------------------------------- #


def test_issue_set_blocked_by_happy_path():
    side_effects = [
        # resolve numeric REST id of the blocking issue
        (True, {"id": 987654}, ""),
        # POST the dependency
        (True, "", ""),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects) as mock:
        result = issue_set_blocked_by(REPO, 1, 2)

    assert result["issue"] == 1
    assert result["blocked_by"] == 2
    assert result["relationship"] == "blocked_by"
    assert result["repo"] == REPO
    assert result["blocked_by_repo"] == REPO

    # First call resolves the blocking issue's REST id.
    first_args = mock.call_args_list[0][0]
    assert "repos/owner/repo/issues/2" in first_args

    # Second call POSTs to the blocked_by dependencies endpoint with the REST id.
    second = mock.call_args_list[1]
    assert "repos/owner/repo/issues/1/dependencies/blocked_by" in second[0]
    assert "POST" in second[0]
    body = json.loads(second[1]["input_data"])
    assert body == {"issue_id": 987654}


def test_issue_set_blocked_by_cross_repo():
    other_repo = "other/repo"
    side_effects = [
        (True, {"id": 555}, ""),
        (True, "", ""),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects) as mock:
        result = issue_set_blocked_by(REPO, 1, 2, blocked_by_repo=other_repo)

    assert result["blocked_by_repo"] == other_repo

    # REST id lookup must target the *other* repo.
    first_args = mock.call_args_list[0][0]
    assert "repos/other/repo/issues/2" in first_args

    # The POST still targets the blocked issue in REPO.
    second_args = mock.call_args_list[1][0]
    assert "repos/owner/repo/issues/1/dependencies/blocked_by" in second_args


def test_issue_set_blocked_by_invalid_repo_format():
    with (
        pytest.raises(ValidationError, match="owner/name"),
        patch("epik_gh.relationships.run_gh", return_value=(True, {"id": 1}, "")),
    ):
        issue_set_blocked_by("badrepo", 1, 2)


# --------------------------------------------------------------------------- #
# remove blocked_by
# --------------------------------------------------------------------------- #


def test_issue_remove_blocked_by_happy_path():
    side_effects = [
        (True, {"id": 987654}, ""),
        (True, "", ""),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects) as mock:
        result = issue_remove_blocked_by(REPO, 1, 2)

    assert result["issue"] == 1
    assert result["removed_blocked_by"] == 2
    assert result["repo"] == REPO
    assert result["blocked_by_repo"] == REPO

    # DELETE against the dependency path keyed by the numeric REST id.
    second = mock.call_args_list[1][0]
    assert "repos/owner/repo/issues/1/dependencies/blocked_by/987654" in second
    assert "DELETE" in second


def test_issue_remove_blocked_by_cross_repo():
    other_repo = "other/repo"
    side_effects = [
        (True, {"id": 42}, ""),
        (True, "", ""),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects) as mock:
        result = issue_remove_blocked_by(REPO, 1, 2, blocked_by_repo=other_repo)

    assert result["blocked_by_repo"] == other_repo

    first_args = mock.call_args_list[0][0]
    assert "repos/other/repo/issues/2" in first_args

    second_args = mock.call_args_list[1][0]
    assert "repos/owner/repo/issues/1/dependencies/blocked_by/42" in second_args


# --------------------------------------------------------------------------- #
# list relationships (parent + sub_issues)
# --------------------------------------------------------------------------- #


def test_issue_list_relationships_parent_and_sub_issues():
    gql_data = json.dumps(
        {
            "data": {
                "repository": {
                    "issue": {
                        "parent": {
                            "number": 5,
                            "title": "Parent",
                            "url": "https://example/5",
                        },
                        "subIssues": {
                            "nodes": [
                                {
                                    "number": 11,
                                    "title": "Child",
                                    "url": "https://example/11",
                                    "state": "OPEN",
                                }
                            ]
                        },
                    }
                }
            }
        }
    )
    with patch(
        "epik_gh.relationships.run_gh", return_value=(True, gql_data, "")
    ) as mock:
        result = issue_list_relationships(REPO, 1)

    assert result["issue_number"] == 1
    assert result["repo"] == REPO
    assert result["parent"]["number"] == 5
    assert len(result["sub_issues"]) == 1
    assert result["sub_issues"][0]["number"] == 11
    assert result["sub_issues"][0]["state"] == "OPEN"

    # GraphQL call must carry the sub_issues feature header and variables as flags.
    args = mock.call_args[0]
    assert _has_sub_issues_header(args)
    assert not any("variables=" in str(a) for a in args)
    assert any("owner=owner" in str(a) for a in args)
    assert any("name=repo" in str(a) for a in args)
    assert any("number=1" in str(a) for a in args)


def test_issue_list_relationships_no_parent():
    gql_data = json.dumps(
        {
            "data": {
                "repository": {"issue": {"parent": None, "subIssues": {"nodes": []}}}
            }
        }
    )
    with patch("epik_gh.relationships.run_gh", return_value=(True, gql_data, "")):
        result = issue_list_relationships(REPO, 7)

    assert result["parent"] is None
    assert result["sub_issues"] == []


# --------------------------------------------------------------------------- #
# add / remove sub-issue
# --------------------------------------------------------------------------- #


def test_issue_add_sub_issue_happy_path():
    mutation_response = json.dumps(
        {
            "data": {
                "addSubIssue": {
                    "issue": {"number": 10},
                    "subIssue": {"number": 20},
                }
            }
        }
    )
    side_effects = [
        (True, _node_id_raw("parent_id"), ""),
        (True, _node_id_raw("sub_id"), ""),
        (True, mutation_response, ""),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects) as mock:
        result = issue_add_sub_issue(REPO, 10, 20)

    assert result["parent"] == 10
    assert result["sub_issue"] == 20

    # The mutation call (3rd) must include the sub_issues feature header.
    mutation_args = mock.call_args_list[2][0]
    assert _has_sub_issues_header(mutation_args)


def test_issue_remove_sub_issue_happy_path():
    mutation_response = json.dumps(
        {
            "data": {
                "removeSubIssue": {
                    "issue": {"number": 10},
                    "subIssue": {"number": 20},
                }
            }
        }
    )
    side_effects = [
        (True, _node_id_raw("parent_id"), ""),
        (True, _node_id_raw("sub_id"), ""),
        (True, mutation_response, ""),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects) as mock:
        result = issue_remove_sub_issue(REPO, 10, 20)

    assert result["parent"] == 10
    assert result["removed_sub_issue"] == 20

    mutation_args = mock.call_args_list[2][0]
    assert _has_sub_issues_header(mutation_args)
