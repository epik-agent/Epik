//! `run_list`, `run_get` and `run_logs`: a repository's GitHub Actions runs,
//! through `gh run`.

use super::gh::strings;
use super::{Args, Context, Reply, boolean, integer, nonempty, string};

/// The fields `run_list` gives for each run.
const RUN_FIELDS: &str = "databaseId,name,workflowName,headBranch,status,conclusion,url";

/// The most runs `run_list` gives when it isn't told.
const LIMIT: i64 = 20;

pub fn list(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let limit = integer(args, "limit").unwrap_or(LIMIT).to_string();
    let mut command = strings(["run", "list", "--repo", repo, "--limit", &limit]);
    for (name, flag) in [
        ("workflow", "--workflow"),
        ("branch", "--branch"),
        ("status", "--status"),
    ] {
        if let Some(value) = nonempty(args, name) {
            command.extend(strings([flag, value]));
        }
    }
    command.extend(strings(["--json", RUN_FIELDS]));
    Ok(Reply::Json(context.gh.json(&command)?))
}

pub fn get(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let id = integer(args, "run_id").unwrap_or_default().to_string();
    let fields = format!("{RUN_FIELDS},event,headSha,createdAt,updatedAt");
    Ok(Reply::Json(context.gh.json(&strings([
        "run", "view", &id, "--repo", repo, "--json", &fields,
    ]))?))
}

pub fn logs(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let id = integer(args, "run_id").unwrap_or_default().to_string();
    let mut command = strings(["run", "view", &id, "--repo", repo]);
    if let Some(job) = integer(args, "job_id").filter(|job| *job != 0) {
        command.extend(["--job".to_owned(), job.to_string()]);
    }
    command.push(
        if boolean(args, "failed_only") {
            "--log-failed"
        } else {
            "--log"
        }
        .to_owned(),
    );
    Ok(Reply::Text(context.gh.text(&command)?))
}

#[cfg(test)]
mod tests {
    use crate::mcp::tests::{call, call_json, expect_gh, failing_gh};
    use serde_json::{Value, json};

    const LIST_FIELDS: &str = "databaseId,name,workflowName,headBranch,status,conclusion,url";

    fn run() -> Value {
        json!({"databaseId": 42, "name": "CI", "status": "completed", "conclusion": "success"})
    }

    #[test]
    fn run_list_lists_the_latest_20_runs_by_default() {
        let gh = expect_gh(
            &format!("run list --repo o/r --limit 20 --json {LIST_FIELDS}"),
            &json!([run()]).to_string(),
        );

        let runs = call_json(gh, "run_list", json!({"repo": "o/r"}));

        assert_eq!(runs.unwrap(), json!([run()]));
    }

    #[test]
    fn run_list_filters_by_workflow_branch_and_status() {
        let gh = expect_gh(
            &format!(
                "run list --repo o/r --limit 5 --workflow ci.yml --branch main --status failure --json {LIST_FIELDS}"
            ),
            "[]",
        );

        let runs = call_json(
            gh,
            "run_list",
            json!({"repo": "o/r", "workflow": "ci.yml", "branch": "main", "status": "failure", "limit": 5}),
        );

        assert_eq!(runs.unwrap(), json!([]));
    }

    #[test]
    fn run_list_fails_when_there_is_no_such_workflow() {
        let error = call_json(
            failing_gh("could not find any workflows named nope"),
            "run_list",
            json!({"repo": "o/r", "workflow": "nope"}),
        )
        .unwrap_err();

        assert!(error.contains("nope"), "{error}");
    }

    #[test]
    fn run_get_returns_one_run_in_detail() {
        let gh = expect_gh(
            &format!(
                "run view 42 --repo o/r --json {LIST_FIELDS},event,headSha,createdAt,updatedAt"
            ),
            &run().to_string(),
        );

        let got = call_json(gh, "run_get", json!({"repo": "o/r", "run_id": 42}));

        assert_eq!(got.unwrap(), run());
    }

    #[test]
    fn run_get_fails_when_there_is_no_such_run() {
        let error = call_json(
            failing_gh("HTTP 404: Not Found"),
            "run_get",
            json!({"repo": "o/r", "run_id": 1}),
        )
        .unwrap_err();

        assert!(error.contains("404"), "{error}");
    }

    #[test]
    fn run_logs_returns_the_runs_logs() {
        let gh = expect_gh("run view 42 --repo o/r --log", "build\tok\n");

        let logs = call(gh, "run_logs", json!({"repo": "o/r", "run_id": 42}));

        assert_eq!(logs.unwrap(), "build\tok\n");
    }

    #[test]
    fn run_logs_returns_one_jobs_logs() {
        let gh = expect_gh("run view 42 --repo o/r --job 7 --log", "job 7\n");

        let logs = call(
            gh,
            "run_logs",
            json!({"repo": "o/r", "run_id": 42, "job_id": 7}),
        );

        assert_eq!(logs.unwrap(), "job 7\n");
    }

    #[test]
    fn run_logs_returns_only_the_failed_steps_logs() {
        let gh = expect_gh("run view 42 --repo o/r --log-failed", "failed\n");

        let logs = call(
            gh,
            "run_logs",
            json!({"repo": "o/r", "run_id": 42, "failed_only": true}),
        );

        assert_eq!(logs.unwrap(), "failed\n");
    }

    #[test]
    fn run_logs_fails_while_the_run_is_in_progress() {
        let gh =
            failing_gh("run 42 is still in progress; logs will be available when it is complete");

        let error = call(gh, "run_logs", json!({"repo": "o/r", "run_id": 42})).unwrap_err();

        assert!(error.contains("in progress"), "{error}");
    }
}
