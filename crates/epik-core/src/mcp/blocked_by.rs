//! `issue_set_blocked_by`, `issue_remove_blocked_by` and
//! `issue_list_blocked_by`: blocked-by edges, through GitHub's
//! issue-dependencies REST endpoints.

use serde_json::{Value, json};

use super::epic_status::issue_url;
use super::gh::{Gh, strings};
use super::{Args, Context, Reply, integer, string};

/// The REST path of the edges from issue `number` in `repo` to its blockers.
fn edges(repo: &str, number: i64) -> String {
    format!("repos/{repo}/issues/{number}/dependencies/blocked_by")
}

/// The ID of issue `number` in `repo`, which the edge endpoints take in
/// place of its number.
fn issue_id(gh: &Gh, repo: &str, number: i64) -> Result<Value, String> {
    let issue: Value = gh.get(&format!("repos/{repo}/issues/{number}"))?;
    Ok(issue["id"].clone())
}

/// One edge, as a tool's arguments name it: the blocked repository and
/// issue, and the blocking repository and issue.
struct Edge<'a> {
    repo: &'a str,
    number: i64,
    blocker_repo: &'a str,
    blocker: i64,
}

impl<'a> Edge<'a> {
    fn of(args: &'a Args) -> Self {
        let repo = string(args, "repo").unwrap_or_default();
        Self {
            repo,
            number: integer(args, "issue_number").unwrap_or_default(),
            blocker_repo: string(args, "blocked_by_repo").unwrap_or(repo),
            blocker: integer(args, "blocked_by_number").unwrap_or_default(),
        }
    }

    fn blocker(&self) -> Value {
        json!({"repo": self.blocker_repo, "number": self.blocker})
    }
}

pub fn set(context: &Context, args: &Args) -> Result<Reply, String> {
    let edge = Edge::of(args);
    let id = issue_id(&context.gh, edge.blocker_repo, edge.blocker)?;
    context.gh.text(&strings([
        "api",
        "--method",
        "POST",
        &edges(edge.repo, edge.number),
        "-F",
        &format!("issue_id={id}"),
    ]))?;
    Ok(Reply::Json(json!({
        "repo": edge.repo,
        "issue_number": edge.number,
        "blocked_by": edge.blocker(),
    })))
}

pub fn remove(context: &Context, args: &Args) -> Result<Reply, String> {
    let edge = Edge::of(args);
    let id = issue_id(&context.gh, edge.blocker_repo, edge.blocker)?;
    context.gh.text(&strings([
        "api",
        "--method",
        "DELETE",
        &format!("{}/{id}", edges(edge.repo, edge.number)),
    ]))?;
    Ok(Reply::Json(json!({
        "repo": edge.repo,
        "issue_number": edge.number,
        "removed_blocked_by": edge.blocker(),
    })))
}

pub fn list(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let number = integer(args, "issue_number").unwrap_or_default();
    let blockers = context.gh.list(&edges(repo, number), None)?;
    Ok(Reply::Json(
        blockers
            .iter()
            .map(|issue| {
                let repository = repo_of(issue);
                json!({
                    "url": issue_url(&repository, issue["number"].as_u64().unwrap_or_default()),
                    "number": issue["number"],
                    "title": issue["title"],
                    "state": issue["state"],
                    "repository": repository,
                })
            })
            .collect(),
    ))
}

/// The `owner/name` of the repository a REST issue is in.
fn repo_of(item: &Value) -> String {
    let url = item["repository_url"].as_str().unwrap_or_default();
    url.split_once("/repos/")
        .map(|(_, repo)| repo.to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tests::{call_json, scripted};
    use serde_json::json;

    const EDGES: &str = "repos/o/r/issues/12/dependencies/blocked_by";

    fn calls(calls: &crate::mcp::tests::Calls) -> Vec<String> {
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|args| args.join(" "))
            .collect()
    }

    #[test]
    fn setting_an_edge_looks_up_the_blocking_issue_id_then_posts_it() {
        let post = format!("api --method POST {EDGES} -F issue_id=1010");
        let (gh, made) = scripted(vec![
            ("api repos/o/r/issues/10", json!({"id": 1010, "number": 10})),
            (&post, json!({"number": 12})),
        ]);

        let edge = call_json(
            gh,
            "issue_set_blocked_by",
            json!({"repo": "o/r", "issue_number": 12, "blocked_by_number": 10}),
        );

        assert_eq!(
            edge.unwrap(),
            json!({"repo": "o/r", "issue_number": 12, "blocked_by": {"repo": "o/r", "number": 10}})
        );
        assert_eq!(calls(&made), ["api repos/o/r/issues/10", post.as_str()]);
    }

    #[test]
    fn a_blocking_issue_in_another_repository_is_looked_up_there() {
        let (gh, _) = scripted(vec![
            ("api repos/x/y/issues/3", json!({"id": 33, "number": 3})),
            (
                &format!("api --method POST {EDGES} -F issue_id=33"),
                json!({"number": 12}),
            ),
        ]);

        let edge = call_json(
            gh,
            "issue_set_blocked_by",
            json!({"repo": "o/r", "issue_number": 12, "blocked_by_number": 3, "blocked_by_repo": "x/y"}),
        );

        assert_eq!(
            edge.unwrap()["blocked_by"],
            json!({"repo": "x/y", "number": 3})
        );
    }

    #[test]
    fn removing_an_edge_looks_up_the_blocking_issue_id_then_deletes_it() {
        let delete = format!("api --method DELETE {EDGES}/1010");
        let (gh, made) = scripted(vec![
            ("api repos/o/r/issues/10", json!({"id": 1010, "number": 10})),
            (&delete, json!({"number": 12})),
        ]);

        let edge = call_json(
            gh,
            "issue_remove_blocked_by",
            json!({"repo": "o/r", "issue_number": 12, "blocked_by_number": 10}),
        );

        assert_eq!(
            edge.unwrap(),
            json!({"repo": "o/r", "issue_number": 12, "removed_blocked_by": {"repo": "o/r", "number": 10}})
        );
        assert_eq!(calls(&made), ["api repos/o/r/issues/10", delete.as_str()]);
    }

    #[test]
    fn setting_an_edge_on_a_missing_issue_fails_with_githubs_message() {
        let (gh, _) = scripted(vec![]);

        let error = call_json(
            gh,
            "issue_set_blocked_by",
            json!({"repo": "o/r", "issue_number": 12, "blocked_by_number": 99}),
        )
        .unwrap_err();

        assert!(
            error.contains("no reply for: gh api repos/o/r/issues/99"),
            "{error}"
        );
    }

    #[test]
    fn listing_gives_every_blocking_issue_across_all_pages() {
        let (gh, _) = scripted(vec![(
            &format!("api --paginate --slurp {EDGES}?per_page=100"),
            json!([
                [{"number": 10, "title": "Ten", "state": "open", "repository_url": "https://api.github.com/repos/o/r", "id": 1}],
                [{"number": 3, "title": "Three", "state": "closed", "repository_url": "https://api.github.com/repos/x/y", "id": 2}],
            ]),
        )]);

        let blockers = call_json(
            gh,
            "issue_list_blocked_by",
            json!({"repo": "o/r", "issue_number": 12}),
        );

        assert_eq!(
            blockers.unwrap(),
            json!([
                {"url": "https://github.com/o/r/issues/10", "number": 10, "title": "Ten", "state": "open", "repository": "o/r"},
                {"url": "https://github.com/x/y/issues/3", "number": 3, "title": "Three", "state": "closed", "repository": "x/y"},
            ])
        );
    }

    #[test]
    fn a_blockers_url_lowercases_its_repository() {
        let (gh, _) = scripted(vec![(
            "dependencies/blocked_by",
            json!([[{"number": 305, "title": "T", "state": "open", "repository_url": "https://api.github.com/repos/Epik-Agent/Epik"}]]),
        )]);

        let blockers = call_json(
            gh,
            "issue_list_blocked_by",
            json!({"repo": "o/r", "issue_number": 12}),
        )
        .unwrap();

        assert_eq!(
            blockers[0]["url"],
            "https://github.com/epik-agent/epik/issues/305"
        );
        assert_eq!(blockers[0]["repository"], "Epik-Agent/Epik");
    }

    #[test]
    fn repo_of_reads_owner_and_name_from_a_rest_issue() {
        assert_eq!(
            repo_of(&json!({"repository_url": "https://api.github.com/repos/other/lib"})),
            "other/lib"
        );
    }
}
