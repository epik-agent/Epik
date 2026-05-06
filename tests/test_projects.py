"""Unit tests for projects.py."""
from __future__ import annotations

import json
from unittest.mock import MagicMock, call, patch

import pytest

from epik_gh.errors import NotFoundError, ValidationError
from epik_gh.projects import (
    project_get_item,
    project_invalidate_cache,
    project_list_items,
    project_set_status,
)

OWNER = "testowner"
PROJECT_NUMBER = 1
REPO = "testowner/repo"
ISSUE_NUMBER = 42

# Fake IDs returned from caching
FAKE_PROJECT_IDS = {
    "project_id": "PVT_project123",
    "fields": {
        "Status": {
            "id": "PVTF_field456",
            "options": {
                "Todo": "opt_todo",
                "In Progress": "opt_inprogress",
                "Done": "opt_done",
            },
        }
    },
}


class TestProjectSetStatus:
    def test_happy_path(self):
        with (
            patch("epik_gh.projects._get_project_ids", return_value=FAKE_PROJECT_IDS),
            patch("epik_gh.projects._find_item_id", return_value="PVTI_item789"),
            patch("epik_gh.projects._gql", return_value={"updateProjectV2ItemFieldValue": {"projectV2Item": {"id": "PVTI_item789"}}}),
        ):
            result = project_set_status(OWNER, PROJECT_NUMBER, REPO, ISSUE_NUMBER, "In Progress")

        assert result["status"] == "In Progress"
        assert result["issue_number"] == ISSUE_NUMBER
        assert result["item_id"] == "PVTI_item789"

    def test_adds_item_when_not_in_project(self):
        with (
            patch("epik_gh.projects._get_project_ids", return_value=FAKE_PROJECT_IDS),
            patch("epik_gh.projects._find_item_id", return_value=None),
            patch("epik_gh.projects._add_item_to_project", return_value="PVTI_newitem"),
            patch("epik_gh.projects._gql", return_value={}),
        ):
            result = project_set_status(OWNER, PROJECT_NUMBER, REPO, ISSUE_NUMBER, "Todo")

        assert result["item_id"] == "PVTI_newitem"

    def test_invalid_status_raises_validation_error(self):
        with (
            patch("epik_gh.projects._get_project_ids", return_value=FAKE_PROJECT_IDS),
        ):
            with pytest.raises(ValidationError, match="not found"):
                project_set_status(OWNER, PROJECT_NUMBER, REPO, ISSUE_NUMBER, "Nonexistent Status")

    def test_missing_status_field_raises_validation_error(self):
        ids_without_status = {
            "project_id": "PVT_project123",
            "fields": {"Priority": {"id": "fid", "options": {"High": "opt1"}}},
        }
        with (
            patch("epik_gh.projects._get_project_ids", return_value=ids_without_status),
        ):
            with pytest.raises(ValidationError, match="Status"):
                project_set_status(OWNER, PROJECT_NUMBER, REPO, ISSUE_NUMBER, "Todo")


class TestProjectGetItem:
    def test_item_found(self):
        with (
            patch("epik_gh.projects._get_project_ids", return_value=FAKE_PROJECT_IDS),
            patch("epik_gh.projects._find_item_id", return_value="PVTI_item789"),
            patch("epik_gh.projects._gql", return_value={
                "node": {
                    "id": "PVTI_item789",
                    "fieldValues": {
                        "nodes": [
                            {"name": "In Progress", "field": {"name": "Status"}}
                        ]
                    }
                }
            }),
        ):
            result = project_get_item(OWNER, PROJECT_NUMBER, REPO, ISSUE_NUMBER)

        assert result["found"] is True
        assert result["item_id"] == "PVTI_item789"

    def test_item_not_found(self):
        with (
            patch("epik_gh.projects._get_project_ids", return_value=FAKE_PROJECT_IDS),
            patch("epik_gh.projects._find_item_id", return_value=None),
        ):
            result = project_get_item(OWNER, PROJECT_NUMBER, REPO, ISSUE_NUMBER)

        assert result["found"] is False


class TestProjectListItems:
    def test_happy_path(self):
        gql_response = {
            "node": {
                "items": {
                    "pageInfo": {"hasNextPage": False, "endCursor": None},
                    "nodes": [
                        {
                            "id": "PVTI_1",
                            "content": {
                                "number": 1,
                                "title": "First issue",
                                "url": "https://github.com/...",
                                "repository": {"nameWithOwner": "owner/repo"},
                            },
                            "fieldValues": {
                                "nodes": [
                                    {"name": "In Progress", "field": {"name": "Status"}}
                                ]
                            },
                        }
                    ],
                }
            }
        }
        with (
            patch("epik_gh.projects._get_project_ids", return_value=FAKE_PROJECT_IDS),
            patch("epik_gh.projects._gql", return_value=gql_response),
        ):
            result = project_list_items(OWNER, PROJECT_NUMBER)

        assert len(result) == 1
        assert result[0]["issue_number"] == 1
        assert result[0]["status"] == "In Progress"

    def test_status_filter(self):
        gql_response = {
            "node": {
                "items": {
                    "pageInfo": {"hasNextPage": False, "endCursor": None},
                    "nodes": [
                        {
                            "id": "PVTI_1",
                            "content": {
                                "number": 1,
                                "title": "Issue 1",
                                "url": "https://...",
                                "repository": {"nameWithOwner": "owner/repo"},
                            },
                            "fieldValues": {"nodes": [{"name": "Todo", "field": {"name": "Status"}}]},
                        },
                        {
                            "id": "PVTI_2",
                            "content": {
                                "number": 2,
                                "title": "Issue 2",
                                "url": "https://...",
                                "repository": {"nameWithOwner": "owner/repo"},
                            },
                            "fieldValues": {"nodes": [{"name": "Done", "field": {"name": "Status"}}]},
                        },
                    ],
                }
            }
        }
        with (
            patch("epik_gh.projects._get_project_ids", return_value=FAKE_PROJECT_IDS),
            patch("epik_gh.projects._gql", return_value=gql_response),
        ):
            result = project_list_items(OWNER, PROJECT_NUMBER, status_filter="Todo")

        assert len(result) == 1
        assert result[0]["issue_number"] == 1


class TestProjectInvalidateCache:
    def test_invalidate_specific_project(self):
        with patch("epik_gh.projects._cache.invalidate_project") as mock_invalidate:
            result = project_invalidate_cache(OWNER, PROJECT_NUMBER)
        mock_invalidate.assert_called_once_with(OWNER, PROJECT_NUMBER)
        assert "invalidated" in result

    def test_invalidate_all(self):
        with patch("epik_gh.projects._cache.invalidate_all") as mock_invalidate:
            result = project_invalidate_cache(OWNER)
        mock_invalidate.assert_called_once()
        assert result["invalidated"] == "all"
