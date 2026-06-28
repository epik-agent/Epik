"""Feature status aggregator for epik-gh.

Aggregates the status of a feature's sub-issues into a single structured
payload the caller can render as a table. Assembled from the existing read
tools (relationships, issue read, PR list, run list, dependency API).

Returns STRUCTURED data only -- the caller is responsible for formatting.
"""

from __future__ import annotations

import json
from typing import Any

from mcp.server.fastmcp import FastMCP

from .errors import EpikGhError
from .issues import issue_get
from .prs import pr_list
from .relationships import issue_list_relationships
from .runner import run_gh, split_repo
from .runs import run_list


def _linked_pr(
    repo: str, issue_number: int, prs: list[dict[str, Any]]
) -> dict[str, Any] | None:
    """Find the PR linked to an issue by branch naming convention.

    The project's branch naming convention is ``<issue#>-<slug>``, so a PR is
    considered linked when its head branch starts with ``"{number}-"`` (or
    otherwise contains the issue number as a path-style prefix).

    Returns the matched PR dict (number/state/url/headRefName) or None.
    """
    prefix = f"{issue_number}-"
    for pr in prs:
        head = pr.get("headRefName") or ""
        # Convention match `<issue#>-<slug>`, or a namespaced branch such as
        # `feat/12-foo` where the convention prefix follows a path separator.
        if head.startswith(prefix) or f"/{prefix}" in head:
            return pr
    return None


def _latest_ci_conclusion(repo: str, head_ref: str) -> str | None:
    """Return the latest CI run conclusion (or status) for a branch.

    Falls back to the run ``status`` when the run has not concluded yet.
    Returns None when there are no runs.
    """
    runs = run_list(repo, branch=head_ref, limit=1)
    if not runs:
        return None
    latest = runs[0]
    conclusion = latest.get("conclusion")
    if conclusion:
        return str(conclusion)
    status = latest.get("status")
    return str(status) if status else None


def _blocked_by_numbers(repo: str, issue_number: int) -> list[int]:
    """Read an issue's blocked-by dependencies via the GitHub API.

    Uses ``gh api repos/{owner}/{repo}/issues/{n}/dependencies/blocked_by``.
    Returns the list of blocking issue numbers (may be empty).
    """
    owner, name = split_repo(repo)
    path = f"repos/{owner}/{name}/issues/{issue_number}/dependencies/blocked_by"
    _, data, _ = run_gh("api", path)
    parsed = json.loads(data) if isinstance(data, str) and data else data
    if not isinstance(parsed, list):
        return []
    numbers: list[int] = []
    for entry in parsed:
        num = entry.get("number")
        if isinstance(num, int):
            numbers.append(num)
    return numbers


def _compute_positions(
    blocked_by_map: dict[int, list[int]],
) -> dict[int, int]:
    """Compute a topological level (position) per issue.

    position 0 = no in-feature prerequisites; each level is +1 over the deepest
    prerequisite. Cycles / unresolved edges are resolved defensively so the
    function always terminates.
    """
    positions: dict[int, int] = {}

    def resolve(number: int, seen: frozenset[int]) -> int:
        if number in positions:
            return positions[number]
        if number in seen:
            # Cycle guard: treat as a root to avoid infinite recursion.
            return 0
        deps = blocked_by_map.get(number, [])
        if not deps:
            level = 0
        else:
            level = 1 + max(resolve(dep, seen | {number}) for dep in deps)
        positions[number] = level
        return level

    for number in blocked_by_map:
        resolve(number, frozenset())
    return positions


def _issue_row(
    repo: str,
    sub: dict[str, Any],
    prs: list[dict[str, Any]],
) -> dict[str, Any]:
    """Build a per-sub-issue row (without graph position)."""
    number = sub["number"]
    state = sub.get("state")
    if state is None:
        # Relationship nodes don't carry state; fetch it.
        state = issue_get(repo, number).get("state")

    pr = _linked_pr(repo, number, prs)
    linked_pr: dict[str, Any] | None = None
    ci_conclusion: str | None = None
    if pr is not None:
        linked_pr = {
            "number": pr.get("number"),
            "state": pr.get("state"),
            "url": pr.get("url"),
        }
        head_ref = pr.get("headRefName")
        if head_ref:
            ci_conclusion = _latest_ci_conclusion(repo, head_ref)

    return {
        "number": number,
        "title": sub.get("title"),
        "state": state,
        "linked_pr": linked_pr,
        "ci_conclusion": ci_conclusion,
        "blocked_by": [],
        "position": None,
    }


def feature_status(
    repo: str,
    feature_issue_number: int,
    pr_limit: int = 100,
) -> dict[str, Any]:
    """Aggregate the status of a feature's sub-issues into a structured payload.

    For each sub-issue of ``feature_issue_number`` returns its number, title,
    state, linked PR (and PR state), latest CI run conclusion, in-feature
    blocked-by edges, and position in the dependency graph.

    Degrades gracefully: if the relationship/sub-issue/dependency reads fail,
    each issue's basic state is still returned with ``blocked_by: []`` and
    ``position: null``, and ``degraded`` is set to True.

    Args:
        repo: Repository in owner/name format.
        feature_issue_number: The parent (feature) issue number.
        pr_limit: Maximum number of PRs to scan when matching linked PRs.

    Returns:
        Dict with keys ``feature``, ``repo``, ``degraded`` and ``issues``
        (a list of per-sub-issue dicts).
    """
    degraded = False

    # Sub-issues come from the relationship query's sub_issues hierarchy.
    try:
        rel = issue_list_relationships(repo, feature_issue_number)
        sub_issues = rel.get("sub_issues", [])
    except EpikGhError:
        degraded = True
        sub_issues = []

    # PR list is needed regardless of degraded state for linked-PR matching.
    try:
        prs = pr_list(repo, state="all", limit=pr_limit)
    except EpikGhError:
        prs = []

    rows = [_issue_row(repo, sub, prs) for sub in sub_issues]

    if degraded:
        return {
            "feature": feature_issue_number,
            "repo": repo,
            "degraded": True,
            "issues": rows,
        }

    # Dependency graph positions. Failure here degrades only the graph data.
    sub_numbers = {row["number"] for row in rows}
    blocked_by_map: dict[int, list[int]] = {}
    try:
        for row in rows:
            edges = _blocked_by_numbers(repo, row["number"])
            # Keep only edges pointing to other sub-issues in this feature.
            in_feature = [n for n in edges if n in sub_numbers and n != row["number"]]
            blocked_by_map[row["number"]] = in_feature
            row["blocked_by"] = in_feature
    except EpikGhError:
        degraded = True
        for row in rows:
            row["blocked_by"] = []
            row["position"] = None
    else:
        positions = _compute_positions(blocked_by_map)
        for row in rows:
            row["position"] = positions.get(row["number"], 0)

    return {
        "feature": feature_issue_number,
        "repo": repo,
        "degraded": degraded,
        "issues": rows,
    }


def register(server: FastMCP) -> None:
    """Register the feature_status tool with the MCP server."""

    @server.tool()
    def tool_feature_status(
        repo: str,
        feature_issue_number: int,
        pr_limit: int = 100,
    ) -> dict[str, Any]:
        """Aggregate a feature's sub-issue status into a structured payload.

        Returns, per sub-issue: number, title, state, linked PR and its state,
        latest CI run conclusion, in-feature blocked-by edges, and position in
        the dependency graph. Degrades to flat per-issue state when relationship
        data is unavailable.

        Args:
            repo: Repository in owner/name format.
            feature_issue_number: The parent (feature) issue number.
            pr_limit: Maximum number of PRs to scan when matching linked PRs.
        """
        return feature_status(repo, feature_issue_number, pr_limit=pr_limit)

    tool_feature_status.__name__ = "feature_status"
