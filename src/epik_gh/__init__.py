"""epik-gh: GitHub MCP server wrapping the gh CLI.

This package can be used as a library (import individual functions directly)
or as an MCP server (run `epik-gh` or `python -m epik_gh.server`).
"""
from .errors import AuthError, EpikGhError, GhError, NotFoundError, RateLimitError, ValidationError
from .runner import run_gh, split_repo

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

# Pull requests
from .prs import (
    pr_close,
    pr_comment,
    pr_create,
    pr_edit,
    pr_get,
    pr_list,
    pr_merge,
    pr_review,
)

# CI / Actions
from .runs import run_get, run_list, run_logs

# Labels
from .labels import label_create, label_delete, label_list

# Branches
from .branches import branch_create, branch_delete, branch_list

# Repos
from .repos import repo_default_branch, repo_get

# Issue relationships (GraphQL)
from .relationships import (
    issue_add_sub_issue,
    issue_list_relationships,
    issue_remove_blocked_by,
    issue_remove_sub_issue,
    issue_set_blocked_by,
)

# Projects V2 (GraphQL)
from .projects import (
    project_get_item,
    project_invalidate_cache,
    project_list_items,
    project_set_status,
)

__all__ = [
    # Errors
    "EpikGhError",
    "AuthError",
    "NotFoundError",
    "RateLimitError",
    "ValidationError",
    "GhError",
    # Runner
    "run_gh",
    "split_repo",
    # Issues
    "issue_list",
    "issue_get",
    "issue_create",
    "issue_edit",
    "issue_close",
    "issue_reopen",
    "issue_comment",
    # PRs
    "pr_list",
    "pr_get",
    "pr_create",
    "pr_edit",
    "pr_close",
    "pr_merge",
    "pr_review",
    "pr_comment",
    # Runs
    "run_list",
    "run_get",
    "run_logs",
    # Labels
    "label_list",
    "label_create",
    "label_delete",
    # Branches
    "branch_list",
    "branch_create",
    "branch_delete",
    # Repos
    "repo_get",
    "repo_default_branch",
    # Relationships
    "issue_set_blocked_by",
    "issue_remove_blocked_by",
    "issue_list_relationships",
    "issue_add_sub_issue",
    "issue_remove_sub_issue",
    # Projects
    "project_set_status",
    "project_get_item",
    "project_list_items",
    "project_invalidate_cache",
]
