//! `epic_status`: each sub-issue of an epic, with its linked pull request,
//! that pull request's CI, its blockers inside the epic, and its level in
//! the dependency graph.
//!
//! Issues and pull requests are named by URL, since sub-issues may be in
//! other repositories. GitHub names are case-insensitive, so a URL's
//! repository is lowercased and URLs compare as strings.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::{Args, Context, Reply, integer, string};
use crate::graph::Graph;
use crate::mcp::gh::Gh;

/// The most `gh` calls made at once, which keeps a large epic under
/// GitHub's secondary rate limits.
const PARALLEL: usize = 8;

/// The URL of issue `number` in `repo`.
pub fn issue_url(repo: &str, number: u64) -> String {
    format!("https://github.com/{}/issues/{number}", repo.to_lowercase())
}

/// The URL of pull request `number` in `repo`.
pub fn pull_url(repo: &str, number: u64) -> String {
    format!("https://github.com/{}/pull/{number}", repo.to_lowercase())
}

/// The `owner/name` of the repository a REST issue or pull request is in.
fn repo_of(item: &Value) -> String {
    let url = item["repository_url"].as_str().unwrap_or_default();
    url.split_once("/repos/")
        .map(|(_, repo)| repo.to_owned())
        .unwrap_or_default()
}

fn number_of(item: &Value) -> u64 {
    item["number"].as_u64().unwrap_or_default()
}

/// Orders URLs by repository, then numerically by number: runs of digits
/// compare as numbers.
fn by_url(a: &str, b: &str) -> Ordering {
    fn chunks(text: &str) -> Vec<(bool, &str)> {
        let mut chunks = Vec::new();
        let mut start = 0;
        let bytes = text.as_bytes();
        for i in 1..=bytes.len() {
            if i == bytes.len() || bytes[i].is_ascii_digit() != bytes[start].is_ascii_digit() {
                chunks.push((bytes[start].is_ascii_digit(), &text[start..i]));
                start = i;
            }
        }
        chunks
    }
    let (a, b) = (chunks(a), chunks(b));
    for (x, y) in a.iter().zip(&b) {
        let order = match (x, y) {
            ((true, x), (true, y)) => x
                .trim_start_matches('0')
                .len()
                .cmp(&y.trim_start_matches('0').len())
                .then_with(|| x.trim_start_matches('0').cmp(y.trim_start_matches('0'))),
            ((_, x), (_, y)) => x.cmp(y),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// The pull request linked to issue `number` in `repo`, from the issue's
/// timeline: among the pull requests in `repo` that cross-reference it,
/// open and merged ones rank above closed ones; then those whose title or
/// body closes the issue; then an open one; then the most recently merged.
pub fn linked_pr(timeline: &[Value], repo: &str, number: u64) -> Option<Value> {
    let closing = regex_lite::Regex::new(&format!(
        r"(?i)\b(close[sd]?|fix(e[sd])?|resolve[sd]?):?\s+({})?#{number}\b",
        regex_lite::escape(repo)
    ))
    .expect("the closing pattern compiles");
    let merged_at = |pr: &Value| -> Option<i64> {
        let at = pr["pull_request"]["merged_at"].as_str()?;
        chrono::DateTime::parse_from_rfc3339(at)
            .ok()
            .map(|at| at.timestamp_millis())
    };
    let rank = |pr: &Value| {
        let state = pr["state"].as_str().unwrap_or_default();
        let text = format!(
            "{}\n{}",
            pr["title"].as_str().unwrap_or_default(),
            pr["body"].as_str().unwrap_or_default()
        );
        (
            i64::from(state == "closed" && merged_at(pr).is_none()),
            i64::from(!closing.is_match(&text)),
            i64::from(state != "open"),
            -merged_at(pr).unwrap_or(0),
        )
    };
    let pr = timeline
        .iter()
        .filter(|event| event["event"] == "cross-referenced")
        .map(|event| &event["source"]["issue"])
        .filter(|source| {
            !source["pull_request"].is_null()
                && source["repository"]["full_name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(repo))
        })
        .min_by_key(|source| rank(source))?;
    let state = if merged_at(pr).is_some() {
        "merged"
    } else {
        pr["state"].as_str().unwrap_or_default()
    };
    let number = number_of(pr);
    Some(json!({"number": number, "state": state, "url": pull_url(repo, number)}))
}

/// The combined result of `check_runs`: failure if any failed, pending if
/// any is still running, success if all passed, null with none.
pub fn ci(check_runs: &[Value]) -> Value {
    if check_runs.is_empty() {
        return Value::Null;
    }
    let passed = ["success", "neutral", "skipped"];
    let done: Vec<&Value> = check_runs
        .iter()
        .filter(|run| run["status"] == "completed")
        .collect();
    if done
        .iter()
        .any(|run| !passed.contains(&run["conclusion"].as_str().unwrap_or_default()))
    {
        json!("failure")
    } else if done.len() < check_runs.len() {
        json!("pending")
    } else {
        json!("success")
    }
}

/// One sub-issue's row, whose blockers count only when in `epic`, the
/// URLs of the epic's sub-issues. A read that fails leaves its fields
/// empty and is listed in `errors`.
fn row(gh: &Gh, issue: &Value, epic: &BTreeSet<String>) -> Value {
    let repo = repo_of(issue);
    let number = number_of(issue);
    let mut errors = Vec::new();
    let attempt = |result: Result<Vec<Value>, String>, errors: &mut Vec<String>| {
        result.unwrap_or_else(|error| {
            errors.push(error);
            Vec::new()
        })
    };
    let timeline = attempt(
        gh.list(&format!("repos/{repo}/issues/{number}/timeline"), None),
        &mut errors,
    );
    let blocking = if issue["issue_dependencies_summary"]["total_blocked_by"]
        .as_u64()
        .unwrap_or(0)
        > 0
    {
        attempt(
            gh.list(
                &format!("repos/{repo}/issues/{number}/dependencies/blocked_by"),
                None,
            ),
            &mut errors,
        )
    } else {
        Vec::new()
    };
    let pr = linked_pr(&timeline, &repo, number);
    let ci = match &pr {
        Some(pr) => {
            let path = format!(
                "repos/{repo}/commits/refs/pull/{}/head/check-runs",
                pr["number"]
            );
            match gh.list(&path, Some("check_runs")) {
                Ok(runs) => ci(&runs),
                Err(error) => {
                    errors.push(error);
                    Value::Null
                }
            }
        }
        None => Value::Null,
    };
    let blocked_by: Vec<String> = blocking
        .iter()
        .map(|blocker| issue_url(&repo_of(blocker), number_of(blocker)))
        .filter(|url| epic.contains(url))
        .collect();
    let mut row = json!({
        "url": issue_url(&repo, number),
        "number": number,
        "title": issue["title"],
        "state": issue["state"],
        "linked_pr": pr,
        "ci": ci,
        "blocked_by": blocked_by,
    });
    if !errors.is_empty() {
        row["degraded"] = json!(true);
        row["errors"] = json!(errors);
    }
    row
}

pub fn run(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let number = integer(args, "epic_issue_number").unwrap_or_default();
    let gh = &context.gh;
    let sub_issues = gh.list(&format!("repos/{repo}/issues/{number}/sub_issues"), None)?;
    let epic: BTreeSet<String> = sub_issues
        .iter()
        .map(|issue| issue_url(&repo_of(issue), number_of(issue)))
        .collect();
    let mut rows = Vec::with_capacity(sub_issues.len());
    for chunk in sub_issues.chunks(PARALLEL) {
        let chunk_rows: Vec<Value> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|issue| scope.spawn(|| row(gh, issue, &epic)))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("a row's thread finishes"))
                .collect()
        });
        rows.extend(chunk_rows);
    }
    let (positions, cycles) = levels(&rows);
    for row in &mut rows {
        let url = row["url"].as_str().unwrap_or_default().to_owned();
        row["position"] = json!(positions.get(&url).copied().flatten());
    }
    Ok(Reply::Json(json!({
        "epic": issue_url(repo, number as u64),
        "issues": rows,
        "cycles": cycles,
    })))
}

/// Each row's level in the graph of its `blocked_by`, and the cycles, each
/// cycle and the list in URL order.
fn levels(rows: &[Value]) -> (BTreeMap<String, Option<usize>>, Vec<Vec<String>>) {
    let mut urls: Vec<String> = rows
        .iter()
        .map(|row| row["url"].as_str().unwrap_or_default().to_owned())
        .collect();
    urls.sort_by(|a, b| by_url(a, b));
    urls.dedup();
    let index = |url: &str| urls.iter().position(|u| u == url).map(|at| at as u64);
    let graph = Graph::new(rows.iter().filter_map(|row| {
        let at = index(row["url"].as_str()?)?;
        let blockers: Vec<u64> = row["blocked_by"]
            .as_array()?
            .iter()
            .filter_map(|url| index(url.as_str()?))
            .collect();
        Some((at, blockers))
    }));
    let url = |at: u64| urls[at as usize].clone();
    let positions = graph
        .levels()
        .into_iter()
        .map(|(at, level)| (url(at), level))
        .collect();
    let cycles = graph
        .cycles()
        .into_iter()
        .map(|cycle| cycle.into_iter().map(url).collect())
        .collect();
    (positions, cycles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tests::{server, text};

    fn issue(number: u64, title: &str, state: &str, blocked_by: u64, repo: &str) -> Value {
        json!({
            "number": number,
            "title": title,
            "state": state,
            "repository_url": format!("https://api.github.com/repos/{repo}"),
            "issue_dependencies_summary": {"total_blocked_by": blocked_by},
        })
    }

    fn open(number: u64, title: &str) -> Value {
        issue(number, title, "open", 0, "o/r")
    }

    #[derive(Default)]
    struct Ref<'a> {
        state: Option<&'a str>,
        merged_at: Option<&'a str>,
        body: &'a str,
        repo: Option<&'a str>,
        not_pr: bool,
    }

    fn reference(number: u64, r: Ref) -> Value {
        let repo = r.repo.unwrap_or("o/r");
        let mut source = json!({
            "number": number,
            "title": format!("PR {number}"),
            "body": r.body,
            "state": r.state.unwrap_or("closed"),
            "html_url": format!("https://github.com/{repo}/pull/{number}"),
            "repository": {"full_name": repo},
        });
        if !r.not_pr {
            source["pull_request"] = json!({"merged_at": r.merged_at});
        }
        json!({"event": "cross-referenced", "source": {"type": "issue", "issue": source}})
    }

    fn merged(at: &str) -> Ref<'_> {
        Ref {
            merged_at: Some(at),
            ..Ref::default()
        }
    }

    fn open_pr() -> Ref<'static> {
        Ref {
            state: Some("open"),
            ..Ref::default()
        }
    }

    /// The `gh api` arguments that fetch every page of `path`.
    fn pages(path: &str) -> String {
        let separator = if path.contains('?') { '&' } else { '?' };
        format!("api --paginate --slurp {path}{separator}per_page=100")
    }

    fn status(replies: Vec<(String, Value)>) -> Value {
        let replies: Vec<(&str, Value)> = replies
            .iter()
            .map(|(needle, body)| (needle.as_str(), body.clone()))
            .collect();
        let result = server(replies)
            .call(
                "epic_status",
                json!({"repo": "o/r", "epic_issue_number": 9}),
            )
            .unwrap();
        let (text, error) = text(&result);
        assert!(!error, "{text}");
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn the_linked_pr_is_the_open_one_among_those_referencing_the_issue() {
        let timeline = [
            reference(16, merged("2026-06-28T21:44:46Z")),
            reference(17, open_pr()),
            json!({"event": "labeled"}),
        ];
        assert_eq!(
            linked_pr(&timeline, "o/r", 10),
            Some(json!({"number": 17, "state": "open", "url": "https://github.com/o/r/pull/17"}))
        );
    }

    #[test]
    fn with_none_open_the_linked_pr_is_the_most_recently_merged() {
        let timeline = [
            reference(16, merged("2026-06-28T21:44:46Z")),
            reference(19, merged("2026-06-29T00:00:00Z")),
            reference(18, Ref::default()),
        ];
        assert_eq!(
            linked_pr(&timeline, "o/r", 10),
            Some(json!({"number": 19, "state": "merged", "url": "https://github.com/o/r/pull/19"}))
        );
    }

    #[test]
    fn a_pr_that_closes_the_issue_outranks_one_that_only_mentions_it() {
        let timeline = [
            reference(
                16,
                Ref {
                    body: "Closes #10.",
                    ..merged("2026-06-28T21:44:46Z")
                },
            ),
            reference(
                22,
                Ref {
                    body: "Rolls up #10 and #11.",
                    ..merged("2026-06-28T22:31:35Z")
                },
            ),
            reference(
                23,
                Ref {
                    body: "Fixes #100.",
                    ..open_pr()
                },
            ),
        ];
        assert_eq!(linked_pr(&timeline, "o/r", 10).unwrap()["number"], 16);
    }

    #[test]
    fn issues_and_prs_in_other_repositories_are_never_the_linked_pr() {
        let timeline = [
            reference(
                12,
                Ref {
                    not_pr: true,
                    ..open_pr()
                },
            ),
            reference(
                5,
                Ref {
                    repo: Some("x/y"),
                    ..open_pr()
                },
            ),
        ];
        assert_eq!(linked_pr(&timeline, "o/r", 10), None);
    }

    #[test]
    fn the_repository_match_ignores_case() {
        let timeline = [reference(
            17,
            Ref {
                repo: Some("O/R"),
                ..open_pr()
            },
        )];
        assert_eq!(
            linked_pr(&timeline, "o/r", 10).unwrap()["url"],
            "https://github.com/o/r/pull/17"
        );
    }

    #[test]
    fn a_closed_unmerged_pr_ranks_below_every_open_or_merged_one() {
        let abandoned = || {
            reference(
                16,
                Ref {
                    body: "Closes #10.",
                    ..Ref::default()
                },
            )
        };
        let open = reference(
            17,
            Ref {
                body: "Part of #10.",
                ..open_pr()
            },
        );
        assert_eq!(
            linked_pr(&[abandoned(), open], "o/r", 10).unwrap()["number"],
            17
        );
        let merged_pr = reference(18, merged("2026-06-28T21:44:46Z"));
        assert_eq!(
            linked_pr(&[abandoned(), merged_pr], "o/r", 10).unwrap()["number"],
            18
        );
        assert_eq!(linked_pr(&[abandoned()], "o/r", 10).unwrap()["number"], 16);
    }

    #[test]
    fn ci_combines_the_check_runs_on_a_commit() {
        let run = |status: &str, conclusion: Option<&str>| json!({"status": status, "conclusion": conclusion});
        let success = Some("success");
        assert_eq!(
            ci(&[
                run("completed", success),
                run("completed", Some("skipped")),
                run("completed", Some("neutral"))
            ]),
            "success"
        );
        assert_eq!(
            ci(&[
                run("completed", success),
                run("in_progress", None),
                run("queued", None)
            ]),
            "pending"
        );
        assert_eq!(
            ci(&[run("in_progress", None), run("completed", Some("failure"))]),
            "failure"
        );
        assert_eq!(ci(&[run("completed", Some("cancelled"))]), "failure");
        assert_eq!(ci(&[]), Value::Null);
    }

    #[test]
    fn it_reports_each_sub_issue_with_its_pr_ci_in_epic_blockers_and_level() {
        let reply = status(vec![
            (
                pages("repos/o/r/issues/9/sub_issues"),
                json!([[
                    issue(10, "Ten", "closed", 0, "o/r"),
                    issue(11, "Eleven", "open", 2, "o/r"),
                    issue(12, "Twelve", "open", 1, "o/r")
                ]]),
            ),
            (
                pages("repos/o/r/issues/11/dependencies/blocked_by"),
                json!([[open(10, "Ten"), open(99, "Elsewhere")]]),
            ),
            (
                pages("repos/o/r/issues/12/dependencies/blocked_by"),
                json!([[open(11, "Eleven")]]),
            ),
            (
                pages("repos/o/r/issues/10/timeline"),
                json!([[reference(16, merged("2026-06-28T21:44:46Z"))]]),
            ),
            (
                pages("repos/o/r/issues/11/timeline"),
                json!([[reference(20, open_pr())]]),
            ),
            (pages("repos/o/r/issues/12/timeline"), json!([[]])),
            (
                pages("repos/o/r/commits/refs/pull/16/head/check-runs"),
                json!([{"check_runs": [{"status": "completed", "conclusion": "success"}]}]),
            ),
            (
                pages("repos/o/r/commits/refs/pull/20/head/check-runs"),
                json!([{"check_runs": [{"status": "queued", "conclusion": null}]}]),
            ),
        ]);

        assert_eq!(
            reply,
            json!({
                "epic": "https://github.com/o/r/issues/9",
                "issues": [
                    {
                        "url": "https://github.com/o/r/issues/10",
                        "number": 10,
                        "title": "Ten",
                        "state": "closed",
                        "linked_pr": {"number": 16, "state": "merged", "url": "https://github.com/o/r/pull/16"},
                        "ci": "success",
                        "blocked_by": [],
                        "position": 0,
                    },
                    {
                        "url": "https://github.com/o/r/issues/11",
                        "number": 11,
                        "title": "Eleven",
                        "state": "open",
                        "linked_pr": {"number": 20, "state": "open", "url": "https://github.com/o/r/pull/20"},
                        "ci": "pending",
                        "blocked_by": ["https://github.com/o/r/issues/10"],
                        "position": 1,
                    },
                    {
                        "url": "https://github.com/o/r/issues/12",
                        "number": 12,
                        "title": "Twelve",
                        "state": "open",
                        "linked_pr": null,
                        "ci": null,
                        "blocked_by": ["https://github.com/o/r/issues/11"],
                        "position": 2,
                    },
                ],
                "cycles": [],
            })
        );
    }

    #[test]
    fn it_levels_around_a_cycle_and_lists_it() {
        let reply = status(vec![
            (
                pages("repos/o/r/issues/9/sub_issues"),
                json!([[
                    issue(1, "One", "open", 1, "o/r"),
                    issue(2, "Two", "open", 1, "o/r"),
                    open(3, "Three")
                ]]),
            ),
            (
                pages("repos/o/r/issues/1/dependencies/blocked_by"),
                json!([[open(2, "Two")]]),
            ),
            (
                pages("repos/o/r/issues/2/dependencies/blocked_by"),
                json!([[open(1, "One")]]),
            ),
            (pages("repos/o/r/issues/1/timeline"), json!([[]])),
            (pages("repos/o/r/issues/2/timeline"), json!([[]])),
            (pages("repos/o/r/issues/3/timeline"), json!([[]])),
        ]);

        let positions: Vec<(u64, Value)> = reply["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (row["number"].as_u64().unwrap(), row["position"].clone()))
            .collect();
        assert_eq!(
            positions,
            [(1, Value::Null), (2, Value::Null), (3, json!(0))]
        );
        assert_eq!(
            reply["cycles"],
            json!([[
                "https://github.com/o/r/issues/1",
                "https://github.com/o/r/issues/2"
            ]])
        );
    }

    #[test]
    fn a_sub_issue_in_another_repository_is_read_from_that_repository() {
        let reply = status(vec![
            (
                pages("repos/o/r/issues/9/sub_issues"),
                json!([[issue(3, "Elsewhere", "open", 0, "other/lib")]]),
            ),
            (
                pages("repos/other/lib/issues/3/timeline"),
                json!([[reference(
                    5,
                    Ref {
                        repo: Some("other/lib"),
                        ..open_pr()
                    }
                )]]),
            ),
            (
                pages("repos/other/lib/commits/refs/pull/5/head/check-runs"),
                json!([{"check_runs": []}]),
            ),
        ]);

        let row = &reply["issues"][0];
        assert_eq!(row["url"], "https://github.com/other/lib/issues/3");
        assert_eq!(
            row["linked_pr"],
            json!({"number": 5, "state": "open", "url": "https://github.com/other/lib/pull/5"})
        );
    }

    #[test]
    fn a_blocker_in_another_repository_with_a_sub_issues_number_is_outside_the_epic() {
        let reply = status(vec![
            (
                pages("repos/o/r/issues/9/sub_issues"),
                json!([[open(7, "Seven"), issue(12, "Twelve", "open", 1, "o/r")]]),
            ),
            (
                pages("repos/o/r/issues/12/dependencies/blocked_by"),
                json!([[issue(7, "Library seven", "open", 0, "other/lib")]]),
            ),
            (pages("repos/o/r/issues/7/timeline"), json!([[]])),
            (pages("repos/o/r/issues/12/timeline"), json!([[]])),
        ]);

        for row in reply["issues"].as_array().unwrap() {
            assert_eq!(row["blocked_by"], json!([]));
            assert_eq!(row["position"], 0);
        }
    }

    #[test]
    fn a_failed_read_degrades_its_row_and_leaves_the_others_whole() {
        let reply = status(vec![
            (
                pages("repos/o/r/issues/9/sub_issues"),
                json!([[open(10, "Ten"), open(11, "Eleven")]]),
            ),
            (pages("repos/o/r/issues/10/timeline"), json!([[]])),
        ]);

        let rows = reply["issues"].as_array().unwrap();
        assert!(rows[0].get("degraded").is_none());
        assert_eq!(rows[1]["degraded"], true);
        let error = rows[1]["errors"][0].as_str().unwrap();
        assert!(error.contains("no reply for: gh api"), "{error}");
        assert!(error.contains("issues/11/timeline"), "{error}");
        assert_eq!(rows[1]["linked_pr"], Value::Null);
    }

    #[test]
    fn urls_order_by_repository_then_number() {
        let mut urls = vec![
            "https://github.com/o/r/issues/10",
            "https://github.com/o/r/issues/9",
            "https://github.com/a/b/issues/100",
        ];
        urls.sort_by(|a, b| by_url(a, b));
        assert_eq!(
            urls,
            [
                "https://github.com/a/b/issues/100",
                "https://github.com/o/r/issues/9",
                "https://github.com/o/r/issues/10",
            ]
        );
    }
}
