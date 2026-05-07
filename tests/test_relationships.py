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


def _node_id_response(node_id: str) -> dict:
    return {"data": {"repository": {"issue": {"id": node_id}}}}


def _gql_response(data: dict) -> str:
    return json.dumps({"data": data})


def test_issue_set_blocked_by_happy_path():
    side_effects = [
        (True, json.dumps(_node_id_response("issue_id_1")), ""),
        (True, json.dumps(_node_id_response("issue_id_2")), ""),
        (
            True,
            json.dumps(
                {
                    "data": {
                        "addIssueRelationship": {
                            "issueRelationship": {"type": "BLOCKED_BY"}
                        }
                    }
                }
            ),
            "",
        ),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects):
        result = issue_set_blocked_by(REPO, 1, 2)

    assert result["issue"] == 1
    assert result["blocked_by"] == 2
    assert result["relationship"] == "blocked_by"


def test_issue_set_blocked_by_cross_repo():
    other_repo = "other/repo"
    side_effects = [
        (True, json.dumps(_node_id_response("issue_id_1")), ""),
        (True, json.dumps(_node_id_response("issue_id_2")), ""),
        (
            True,
            json.dumps(
                {
                    "data": {
                        "addIssueRelationship": {
                            "issueRelationship": {"type": "BLOCKED_BY"}
                        }
                    }
                }
            ),
            "",
        ),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects) as mock:
        result = issue_set_blocked_by(REPO, 1, 2, blocked_by_repo=other_repo)

    assert result["blocked_by_repo"] == other_repo
    # Second node-id lookup must use other_repo, not REPO
    second_call_args = mock.call_args_list[1][0]
    assert "other" in " ".join(second_call_args)


def test_issue_set_blocked_by_invalid_repo_format():
    with (
        pytest.raises(ValidationError, match="owner/name"),
        patch("epik_gh.relationships.run_gh", return_value=(True, "{}", "")),
    ):
        issue_set_blocked_by("badrepo", 1, 2)


def test_issue_remove_blocked_by_happy_path():
    side_effects = [
        (True, json.dumps(_node_id_response("issue_id_1")), ""),
        (True, json.dumps(_node_id_response("issue_id_2")), ""),
        (
            True,
            json.dumps(
                {"data": {"removeIssueRelationship": {"clientMutationId": None}}}
            ),
            "",
        ),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects):
        result = issue_remove_blocked_by(REPO, 1, 2)

    assert result["issue"] == 1
    assert result["removed_blocked_by"] == 2


def test_issue_remove_blocked_by_cross_repo():
    other_repo = "other/repo"
    side_effects = [
        (True, json.dumps(_node_id_response("issue_id_1")), ""),
        (True, json.dumps(_node_id_response("issue_id_2")), ""),
        (
            True,
            json.dumps(
                {"data": {"removeIssueRelationship": {"clientMutationId": None}}}
            ),
            "",
        ),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects) as mock:
        result = issue_remove_blocked_by(REPO, 1, 2, blocked_by_repo=other_repo)

    assert result["blocked_by_repo"] == other_repo
    second_call_args = mock.call_args_list[1][0]
    assert "other" in " ".join(second_call_args)


def test_issue_list_relationships_happy_path():
    gql_data = json.dumps(
        {
            "data": {
                "repository": {
                    "issue": {
                        "id": "issue_id_1",
                        "trackedInIssues": {
                            "nodes": [
                                {
                                    "number": 5,
                                    "title": "Parent",
                                    "url": "https://...",
                                }
                            ]
                        },
                        "trackedIssues": {"nodes": []},
                    }
                }
            }
        }
    )
    with patch("epik_gh.relationships.run_gh", return_value=(True, gql_data, "")):
        result = issue_list_relationships(REPO, 1)

    assert result["issue_number"] == 1
    assert len(result["tracked_in"]) == 1
    assert result["tracked_in"][0]["number"] == 5


def test_issue_add_sub_issue_happy_path():
    side_effects = [
        (True, json.dumps(_node_id_response("parent_id")), ""),
        (True, json.dumps(_node_id_response("sub_id")), ""),
        (
            True,
            json.dumps(
                {
                    "data": {
                        "addSubIssue": {
                            "issue": {"number": 10},
                            "subIssue": {"number": 20},
                        }
                    }
                }
            ),
            "",
        ),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects):
        result = issue_add_sub_issue(REPO, 10, 20)

    assert result["parent"] == 10
    assert result["sub_issue"] == 20


def test_issue_remove_sub_issue_happy_path():
    side_effects = [
        (True, json.dumps(_node_id_response("parent_id")), ""),
        (True, json.dumps(_node_id_response("sub_id")), ""),
        (
            True,
            json.dumps(
                {
                    "data": {
                        "removeSubIssue": {
                            "issue": {"number": 10},
                            "subIssue": {"number": 20},
                        }
                    }
                }
            ),
            "",
        ),
    ]
    with patch("epik_gh.relationships.run_gh", side_effect=side_effects):
        result = issue_remove_sub_issue(REPO, 10, 20)

    assert result["parent"] == 10
    assert result["removed_sub_issue"] == 20
