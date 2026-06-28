"""epik-gh: GitHub MCP server wrapping the gh CLI.

This package can be used as a library (import individual functions directly)
or as an MCP server (run `epik-gh` or `python -m epik_gh.server`).
"""

from .errors import (
    AuthError,
    EpikGhError,
    GhError,
    NotFoundError,
    RateLimitError,
    ValidationError,
)

# Feature launch (build module)
from .feature_launch import feature_launch

# Issues
from .issues import (
    issue_close,
    issue_comment,
    issue_create,
    issue_edit,
    issue_get,
    issue_list,
    issue_reopen,
)

# Labels
from .labels import label_create, label_delete, label_list

# Projects V2 (GraphQL)
from .projects import (
    project_get_item,
    project_invalidate_cache,
    project_list_items,
    project_set_status,
)

# Pull requests
from .prs import pr_get, pr_list

# Raw gh passthrough
from .raw import gh_raw

# Issue relationships (GraphQL)
from .relationships import (
    issue_add_sub_issue,
    issue_list_relationships,
    issue_remove_blocked_by,
    issue_remove_sub_issue,
    issue_set_blocked_by,
)

# Repos
from .repos import repo_default_branch, repo_get
from .runner import run_gh, run_gh_raw, split_repo

# CI / Actions
from .runs import run_get, run_list, run_logs

__all__ = [
    "AuthError",
    "EpikGhError",
    "GhError",
    "NotFoundError",
    "RateLimitError",
    "ValidationError",
    "feature_launch",
    "gh_raw",
    "issue_add_sub_issue",
    "issue_close",
    "issue_comment",
    "issue_create",
    "issue_edit",
    "issue_get",
    "issue_list",
    "issue_list_relationships",
    "issue_remove_blocked_by",
    "issue_remove_sub_issue",
    "issue_reopen",
    "issue_set_blocked_by",
    "label_create",
    "label_delete",
    "label_list",
    "pr_get",
    "pr_list",
    "project_get_item",
    "project_invalidate_cache",
    "project_list_items",
    "project_set_status",
    "repo_default_branch",
    "repo_get",
    "run_get",
    "run_gh",
    "run_gh_raw",
    "run_list",
    "run_logs",
    "split_repo",
]
