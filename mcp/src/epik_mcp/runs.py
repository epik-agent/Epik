"""CI / GitHub Actions run tools for epik-mcp."""

from __future__ import annotations

from typing import Any

from mcp.server.fastmcp import FastMCP

from .runner import run_gh

_RUN_FIELDS = [
    "databaseId",
    "name",
    "status",
    "conclusion",
    "workflowName",
    "headBranch",
    "headSha",
    "createdAt",
    "updatedAt",
    "url",
    "event",
]


def run_list(
    repo: str,
    workflow: str | None = None,
    branch: str | None = None,
    status: str | None = None,
    limit: int = 20,
) -> list[dict[str, Any]]:
    """List workflow runs for a repository.

    Args:
        repo: Repository in owner/name format.
        workflow: Filter by workflow name or filename.
        branch: Filter by branch name.
        status: Filter by run status (queued, in_progress, completed, failure, success).
        limit: Maximum number of runs to return (default 20).

    Returns:
        List of run objects.
    """
    args = ["run", "list", "--repo", repo, "--limit", str(limit)]
    if workflow:
        args.extend(["--workflow", workflow])
    if branch:
        args.extend(["--branch", branch])
    if status:
        args.extend(["--status", status])
    _, data, _ = run_gh(
        *args,
        json_fields=[
            "databaseId",
            "name",
            "status",
            "conclusion",
            "workflowName",
            "headBranch",
            "url",
        ],
    )
    return data  # type: ignore[return-value]


def run_get(repo: str, run_id: int) -> dict[str, Any]:
    """Get details of a single workflow run.

    Args:
        repo: Repository in owner/name format.
        run_id: The numeric run ID.

    Returns:
        Run object with full details including jobs.
    """
    _, data, _ = run_gh(
        "run",
        "view",
        str(run_id),
        "--repo",
        repo,
        json_fields=_RUN_FIELDS,
    )
    return data  # type: ignore[return-value]


def run_logs(
    repo: str,
    run_id: int,
    job_id: int | None = None,
    failed_only: bool = False,
) -> str:
    """Fetch logs for a workflow run or a specific job.

    Args:
        repo: Repository in owner/name format.
        run_id: The numeric run ID.
        job_id: If provided, fetch logs for this specific job only.
        failed_only: If True, only return logs from failed steps.

    Returns:
        Log output as a string.
    """
    base = ["run", "view", str(run_id), "--repo", repo]
    if job_id is not None:
        args = [*base, "--job", str(job_id), "--log"]
    elif failed_only:
        args = [*base, "--log-failed"]
    else:
        args = [*base, "--log"]
    _, data, _ = run_gh(*args)
    return str(data)


def register(server: FastMCP) -> None:
    """Register all CI/run tools with the MCP server."""

    @server.tool(name="run_list")
    def tool_run_list(
        repo: str,
        workflow: str | None = None,
        branch: str | None = None,
        status: str | None = None,
        limit: int = 20,
    ) -> list[dict[str, Any]]:
        """List workflow runs for a repository.

        Args:
            repo: Repository in owner/name format.
            workflow: Filter by workflow name or filename.
            branch: Filter by branch name.
            status: Filter by run status (queued, in_progress, completed, failure,
                success).
            limit: Maximum number of runs to return (default 20).
        """
        return run_list(
            repo, workflow=workflow, branch=branch, status=status, limit=limit
        )

    @server.tool(name="run_get")
    def tool_run_get(repo: str, run_id: int) -> dict[str, Any]:
        """Get details of a single workflow run.

        Args:
            repo: Repository in owner/name format.
            run_id: The numeric run ID.
        """
        return run_get(repo, run_id)

    @server.tool(name="run_logs")
    def tool_run_logs(
        repo: str,
        run_id: int,
        job_id: int | None = None,
        failed_only: bool = False,
    ) -> str:
        """Fetch logs for a workflow run or a specific job.

        Args:
            repo: Repository in owner/name format.
            run_id: The numeric run ID.
            job_id: If provided, fetch logs for this specific job only.
            failed_only: If True, only return logs from failed steps.
        """
        return run_logs(repo, run_id, job_id=job_id, failed_only=failed_only)
