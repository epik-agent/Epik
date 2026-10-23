"""Unit tests for repos.py."""

from __future__ import annotations

from unittest.mock import patch

from epik_mcp.repos import _REPO_FIELDS, repo_default_branch, repo_get

REPO = "owner/repo"

# Field names accepted by `gh repo view --json`, per `gh repo view --json` with no
# value. Guards against typos like `fullName`, which gh rejects outright.
GH_REPO_VIEW_FIELDS = {
    "archivedAt",
    "assignableUsers",
    "codeOfConduct",
    "contactLinks",
    "createdAt",
    "defaultBranchRef",
    "deleteBranchOnMerge",
    "description",
    "diskUsage",
    "forkCount",
    "fundingLinks",
    "hasDiscussionsEnabled",
    "hasIssuesEnabled",
    "hasProjectsEnabled",
    "hasWikiEnabled",
    "homepageUrl",
    "id",
    "isArchived",
    "isBlankIssuesEnabled",
    "isEmpty",
    "isFork",
    "isInOrganization",
    "isMirror",
    "isPrivate",
    "isSecurityPolicyEnabled",
    "isTemplate",
    "isUserConfigurationRepository",
    "issueTemplates",
    "issues",
    "labels",
    "languages",
    "latestRelease",
    "licenseInfo",
    "mentionableUsers",
    "mergeCommitAllowed",
    "milestones",
    "mirrorUrl",
    "name",
    "nameWithOwner",
    "openGraphImageUrl",
    "owner",
    "parent",
    "primaryLanguage",
    "projects",
    "projectsV2",
    "pullRequestTemplates",
    "pullRequests",
    "pushedAt",
    "rebaseMergeAllowed",
    "repositoryTopics",
    "securityPolicyUrl",
    "squashMergeAllowed",
    "sshUrl",
    "stargazerCount",
    "templateRepository",
    "updatedAt",
    "url",
    "usesCustomOpenGraphImage",
    "viewerCanAdminister",
    "viewerDefaultCommitEmail",
    "viewerDefaultMergeMethod",
    "viewerHasStarred",
    "viewerPermission",
    "viewerPossibleCommitEmails",
    "viewerSubscription",
    "visibility",
    "watchers",
}


def test_repo_fields_are_valid_gh_fields():
    assert set(_REPO_FIELDS) <= GH_REPO_VIEW_FIELDS


def test_repo_fields_use_name_with_owner():
    assert "nameWithOwner" in _REPO_FIELDS
    assert "fullName" not in _REPO_FIELDS


def _mock_run(return_value):
    return patch("epik_mcp.repos.run_gh", return_value=(True, return_value, ""))


def test_repo_get_happy_path():
    repo_data = {
        "name": "repo",
        "nameWithOwner": "owner/repo",
        "isPrivate": False,
        "url": "https://github.com/owner/repo",
    }
    with _mock_run(repo_data):
        result = repo_get(REPO)
    assert result == repo_data


def test_repo_get_passes_repo_arg():
    with patch("epik_mcp.repos.run_gh", return_value=(True, {}, "")) as mock:
        repo_get(REPO)
    args = mock.call_args[0]
    assert REPO in args


def test_repo_default_branch_happy_path_main():
    with _mock_run({"defaultBranchRef": {"name": "main"}}):
        result = repo_default_branch(REPO)
    assert result == "main"


def test_repo_default_branch_happy_path_master():
    with _mock_run({"defaultBranchRef": {"name": "master"}}):
        result = repo_default_branch(REPO)
    assert result == "master"


def test_repo_default_branch_missing_ref_returns_main():
    with _mock_run({}):
        result = repo_default_branch(REPO)
    assert result == "main"
