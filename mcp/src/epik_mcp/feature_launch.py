"""Feature launch tool (build module).

Starts a headless feature build by dispatching the repository's Epik build
workflow on GitHub Actions (``.github/workflows/epik-build.yml``) via
``gh workflow run``. Like the plan module, it authenticates through the gh
CLI; the build itself needs an ``ANTHROPIC_API_KEY`` secret configured in the
target repository (see the README build module section).

After dispatch, progress is observable with the read-only run tools
(``run_list`` / ``run_get`` / ``run_logs``) filtered to the build workflow.
"""

from __future__ import annotations

from typing import Any

from mcp.server.fastmcp import FastMCP

from .errors import ValidationError
from .runner import run_gh, split_repo

#: Workflow file name the tool dispatches. The target repository must contain
#: this workflow on the ref the dispatch runs against.
BUILD_WORKFLOW: str = "epik-build.yml"


def feature_launch(
    repo: str,
    feature_issue_number: int,
    base_branch: str,
    target_branch: str,
    ref: str | None = None,
) -> dict[str, Any]:
    """Start a headless feature build by dispatching the Epik build workflow.

    Runs ``gh workflow run epik-build.yml`` on ``repo`` with the feature issue
    number and branches as workflow inputs. The workflow runs a headless
    Claude Code session that builds the feature on ``target_branch``.

    Args:
        repo: Repository in owner/name format.
        feature_issue_number: The feature issue number to build.
        base_branch: The branch the feature branch is created from.
        target_branch: The feature branch the build works on.
        ref: Git ref to run the workflow from (the ref must contain
            ``epik-build.yml``). Defaults to the repository default branch.

    Returns:
        A dict describing the dispatch::

            {
                "dispatched": True,
                "repo": <str>,
                "workflow": "epik-build.yml",
                "feature": <int>,
                "base_branch": <str>,
                "target_branch": <str>,
                "ref": <str or None>,
                "watch": <hint for following the run>,
            }

    Raises:
        ValidationError: If arguments are malformed.
        EpikMcpError: If the gh dispatch fails (workflow missing, no
            permission, etc.).
    """
    split_repo(repo)
    if feature_issue_number < 1:
        raise ValidationError(
            f"feature_issue_number must be positive; got {feature_issue_number}"
        )
    if not base_branch.strip():
        raise ValidationError("base_branch must be a non-empty branch name")
    if not target_branch.strip():
        raise ValidationError("target_branch must be a non-empty branch name")

    args = [
        "workflow",
        "run",
        BUILD_WORKFLOW,
        "--repo",
        repo,
        "--field",
        f"feature_issue_number={feature_issue_number}",
        "--field",
        f"base_branch={base_branch}",
        "--field",
        f"target_branch={target_branch}",
    ]
    if ref:
        args.extend(["--ref", ref])
    run_gh(*args)

    return {
        "dispatched": True,
        "repo": repo,
        "workflow": BUILD_WORKFLOW,
        "feature": feature_issue_number,
        "base_branch": base_branch,
        "target_branch": target_branch,
        "ref": ref,
        "watch": (
            f"Follow the build with run_list(repo={repo!r}, "
            f"workflow={BUILD_WORKFLOW!r}) and feature_status."
        ),
    }


def register(server: FastMCP) -> None:
    """Register the feature_launch tool with the MCP server."""

    @server.tool(name="feature_launch")
    def tool_feature_launch(
        repo: str,
        feature_issue_number: int,
        base_branch: str,
        target_branch: str,
        ref: str | None = None,
    ) -> dict[str, Any]:
        """Start a headless feature build on GitHub Actions.

        Dispatches the repository's epik-build.yml workflow with the feature
        issue number and branches as inputs. Watch progress with run_list /
        run_get / run_logs (workflow "epik-build.yml") and feature_status.

        Args:
            repo: Repository in owner/name format.
            feature_issue_number: The feature issue number to build.
            base_branch: The branch the feature branch is created from.
            target_branch: The feature branch the build works on.
            ref: Git ref to run the workflow from (must contain
                epik-build.yml). Defaults to the repository default branch.
        """
        return feature_launch(
            repo,
            feature_issue_number,
            base_branch,
            target_branch,
            ref=ref,
        )
