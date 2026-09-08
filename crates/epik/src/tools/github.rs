//! The GitHub verbs as tools the model can call.
//!
//! [`all`] turns one [`GitHub`] client into one [`Tool`] per public verb,
//! plus `feature_plan`, the [`Tracker`] seam's read of a whole feature.
//! Descriptions are written for a model: one sentence each, and the verbs
//! that need the token say so, so the persona can tell the user what is
//! missing instead of guessing. Arguments arrive as JSON; a repository is
//! one `owner/name` string settled through [`Repo::parse`], and a bad
//! spelling is an Err result in plain words, never a panic.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use super::{Tool, arg};
use crate::feature::{Feature, IssueId, Plan};
use crate::github::{Error, GitHub, Merge, Repo};
use crate::tracker::Tracker;
use crate::tracker::github::GitHubTracker;

/// One tool per public GitHub verb — plus `feature_plan`, the
/// [`Tracker`] seam's read of a whole feature — all speaking through
/// `github`. Every verb takes a repository, settled once here against
/// `owner`, the configured default a bare name completes with.
#[must_use]
pub fn all(github: GitHub, owner: Option<String>) -> Vec<Tool> {
    let github = Arc::new(github);
    let owner = Arc::new(owner);
    let gh = |f: fn(&GitHub, &Repo, &Value) -> Result<Value, String>| {
        let github = Arc::clone(&github);
        let owner = Arc::clone(&owner);
        Box::new(move |arguments: &Value| {
            let repo = Repo::settle(arg::string(arguments, "repo")?, owner.as_deref())?;
            f(&github, &repo, arguments)
        }) as crate::tools::Handler
    };
    vec![
        Tool::new(
            "github_default_branch",
            "The default branch name of a GitHub repository.",
            schema(&[repo_arg()], &["repo"]),
            gh(|github, repo, _| answer(github.default_branch(repo))),
        ),
        Tool::new(
            "github_issue",
            "One GitHub issue by number: title, body, and open/closed state.",
            issue_schema(),
            gh(|github, repo, arguments| {
                answer(github.issue(repo, arg::number(arguments, "number")?))
            }),
        ),
        Tool::new(
            "github_create_issue",
            "Opens a new GitHub issue with a title and body. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    string_arg("title", "The issue title."),
                    string_arg("body", "The issue body, in Markdown; may be empty."),
                ],
                &["repo", "title", "body"],
            ),
            gh(|github, repo, arguments| {
                answer(github.create_issue(
                    repo,
                    arg::string(arguments, "title")?,
                    arg::string(arguments, "body")?,
                ))
            }),
        ),
        Tool::new(
            "github_edit_issue",
            "Rewrites a GitHub issue's title, body, or both; an omitted field keeps its text. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    number_arg("number", "The issue number."),
                    string_arg("title", "The new title; omit to keep the current one."),
                    string_arg("body", "The new body; omit to keep the current one."),
                ],
                &["repo", "number"],
            ),
            gh(|github, repo, arguments| {
                answer(github.edit_issue(
                    repo,
                    arg::number(arguments, "number")?,
                    arguments["title"].as_str(),
                    arguments["body"].as_str(),
                ))
            }),
        ),
        Tool::new(
            "github_comment",
            "Adds a comment to a GitHub issue or pull request. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    number_arg("number", "The issue or pull request number."),
                    string_arg("body", "The comment body, in Markdown."),
                ],
                &["repo", "number", "body"],
            ),
            gh(|github, repo, arguments| {
                done(github.comment(
                    repo,
                    arg::number(arguments, "number")?,
                    arg::string(arguments, "body")?,
                ))
            }),
        ),
        Tool::new(
            "github_close_issue",
            "Closes a GitHub issue. Needs the GitHub token.",
            issue_schema(),
            gh(|github, repo, arguments| {
                answer(github.close_issue(repo, arg::number(arguments, "number")?))
            }),
        ),
        Tool::new(
            "github_open_pull",
            "Opens a GitHub pull request merging branch head into branch base. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    string_arg("title", "The pull request title."),
                    string_arg("head", "The branch to merge from."),
                    string_arg("base", "The branch to merge into."),
                    string_arg("body", "The pull request body, in Markdown; may be empty."),
                ],
                &["repo", "title", "head", "base", "body"],
            ),
            gh(|github, repo, arguments| {
                answer(github.open_pull(
                    repo,
                    arg::string(arguments, "title")?,
                    arg::string(arguments, "head")?,
                    arg::string(arguments, "base")?,
                    arg::string(arguments, "body")?,
                ))
            }),
        ),
        Tool::new(
            "github_pull",
            "One GitHub pull request by number: title, state, merged flag, and both branches.",
            schema(
                &[repo_arg(), number_arg("number", "The pull request number.")],
                &["repo", "number"],
            ),
            gh(|github, repo, arguments| {
                answer(github.pull(repo, arg::number(arguments, "number")?))
            }),
        ),
        Tool::new(
            "github_pull_for",
            "The most recent GitHub pull request a branch produced, in any state; null when the branch never produced one.",
            schema(
                &[repo_arg(), string_arg("head", "The branch name.")],
                &["repo", "head"],
            ),
            gh(|github, repo, arguments| {
                answer(github.pull_for(repo, arg::string(arguments, "head")?))
            }),
        ),
        Tool::new(
            "github_merge_pull",
            "Merges a GitHub pull request by commit, squash, or rebase; an unmergeable one comes back as GitHub's own refusal. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    number_arg("number", "The pull request number."),
                    (
                        "method",
                        json!({
                            "type": "string",
                            "enum": ["commit", "squash", "rebase"],
                            "description": "How to merge.",
                        }),
                    ),
                ],
                &["repo", "number", "method"],
            ),
            gh(|github, repo, arguments| {
                let method: Merge = serde_json::from_value(arguments["method"].clone())
                    .map_err(|_| "method must be commit, squash, or rebase".to_owned())?;
                done(github.merge_pull(repo, arg::number(arguments, "number")?, method))
            }),
        ),
        Tool::new(
            "github_branch_sha",
            "The commit SHA a GitHub branch points at; null when there is no such branch.",
            schema(
                &[repo_arg(), string_arg("branch", "The branch name.")],
                &["repo", "branch"],
            ),
            gh(|github, repo, arguments| {
                answer(github.branch_sha(repo, arg::string(arguments, "branch")?))
            }),
        ),
        Tool::new(
            "github_compare",
            "How one GitHub ref stands relative to another: identical, ahead, behind, or diverged.",
            schema(
                &[
                    repo_arg(),
                    string_arg("base", "The ref to compare against."),
                    string_arg("head", "The ref being compared."),
                ],
                &["repo", "base", "head"],
            ),
            gh(|github, repo, arguments| {
                answer(github.compare(
                    repo,
                    arg::string(arguments, "base")?,
                    arg::string(arguments, "head")?,
                ))
            }),
        ),
        Tool::new(
            "github_create_branch",
            "Creates a GitHub branch at a commit SHA, entirely at the remote. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    string_arg("branch", "The new branch's name."),
                    string_arg("sha", "The commit SHA to start it at."),
                ],
                &["repo", "branch", "sha"],
            ),
            gh(|github, repo, arguments| {
                done(github.create_branch(
                    repo,
                    arg::string(arguments, "branch")?,
                    arg::string(arguments, "sha")?,
                ))
            }),
        ),
        Tool::new(
            "github_check_conclusions",
            "Every check run's verdict on a GitHub ref — a branch, tag, or commit SHA; a run still executing has a null conclusion.",
            schema(
                &[
                    repo_arg(),
                    string_arg("ref", "The branch name, tag, or commit SHA."),
                ],
                &["repo", "ref"],
            ),
            gh(|github, repo, arguments| {
                answer(github.check_conclusions(repo, arg::string(arguments, "ref")?))
            }),
        ),
        Tool::new(
            "github_issue_graph",
            "A GitHub issue with its edges: the sub-issues it decomposes into and the issues blocking it. Needs the GitHub token.",
            issue_schema(),
            gh(|github, repo, arguments| {
                answer(github.issue_graph(repo, arg::number(arguments, "number")?))
            }),
        ),
        Tool::new(
            "feature_plan",
            "A feature's whole plan in one call: the sub-issue tree rooted at the feature issue, \
             the blocked-by edges over it, any blockers outside the tree, the issues ready to \
             start, and any problems with the plan's shape. Titles and state only, no bodies. \
             Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    number_arg("feature", "The feature issue's number."),
                ],
                &["repo", "feature"],
            ),
            gh(|github, repo, arguments| {
                /// The whole [`Plan`] flattened into the answer — a field
                /// added to it later reaches the model unasked — plus the
                /// two readings taken of it.
                #[derive(serde::Serialize)]
                struct Shown<'a> {
                    #[serde(flatten)]
                    plan: &'a Plan,
                    ready: Vec<&'a crate::feature::Issue>,
                    problems: Vec<crate::feature::Problem>,
                }
                let tracker = GitHubTracker::new(github, repo.clone());
                let plan =
                    tracker.plan(&Feature(IssueId::from(arg::number(arguments, "feature")?)))?;
                let none = BTreeSet::new();
                answer(Ok(Shown {
                    ready: plan.ready(&none),
                    problems: plan.problems(),
                    plan: &plan,
                }))
            }),
        ),
        Tool::new(
            "github_add_sub_issue",
            "Makes one GitHub issue a sub-issue of another. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    number_arg("parent", "The parent issue's number."),
                    number_arg("child", "The issue to attach as a sub-issue."),
                ],
                &["repo", "parent", "child"],
            ),
            gh(|github, repo, arguments| {
                done(github.add_sub_issue(
                    repo,
                    arg::number(arguments, "parent")?,
                    arg::number(arguments, "child")?,
                ))
            }),
        ),
        Tool::new(
            "github_remove_sub_issue",
            "Detaches a sub-issue from its parent; both issues survive, the edge does not. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    number_arg("parent", "The parent issue's number."),
                    number_arg("child", "The sub-issue to detach."),
                ],
                &["repo", "parent", "child"],
            ),
            gh(|github, repo, arguments| {
                done(github.remove_sub_issue(
                    repo,
                    arg::number(arguments, "parent")?,
                    arg::number(arguments, "child")?,
                ))
            }),
        ),
        Tool::new(
            "github_add_blocked_by",
            "Records that one GitHub issue is blocked by another. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    number_arg("issue", "The blocked issue's number."),
                    number_arg("blocker", "The issue doing the blocking."),
                ],
                &["repo", "issue", "blocker"],
            ),
            gh(|github, repo, arguments| {
                done(github.add_blocked_by(
                    repo,
                    arg::number(arguments, "issue")?,
                    arg::number(arguments, "blocker")?,
                ))
            }),
        ),
        Tool::new(
            "github_remove_blocked_by",
            "Removes a blocked-by edge between two GitHub issues — what unblocking is. Needs the GitHub token.",
            schema(
                &[
                    repo_arg(),
                    number_arg("issue", "The blocked issue's number."),
                    number_arg("blocker", "The blocker to remove."),
                ],
                &["repo", "issue", "blocker"],
            ),
            gh(|github, repo, arguments| {
                done(github.remove_blocked_by(
                    repo,
                    arg::number(arguments, "issue")?,
                    arg::number(arguments, "blocker")?,
                ))
            }),
        ),
    ]
}

// ----- argument plumbing -----

fn repo_arg() -> (&'static str, Value) {
    (
        "repo",
        json!({
            "type": "string",
            "description": "The repository as one owner/name string, e.g. epik-agent/Epik. A bare name is accepted when a default owner is configured.",
        }),
    )
}

fn string_arg(name: &'static str, description: &str) -> (&'static str, Value) {
    (
        name,
        json!({ "type": "string", "description": description }),
    )
}

fn number_arg(name: &'static str, description: &str) -> (&'static str, Value) {
    (
        name,
        json!({ "type": "integer", "description": description }),
    )
}

/// The arguments every verb about one issue takes: the repository and
/// the issue's number.
fn issue_schema() -> Value {
    schema(
        &[repo_arg(), number_arg("number", "The issue number.")],
        &["repo", "number"],
    )
}

fn schema(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let properties: serde_json::Map<String, Value> = properties
        .iter()
        .map(|(name, schema)| ((*name).to_owned(), schema.clone()))
        .collect();
    json!({ "type": "object", "properties": properties, "required": required })
}

/// A verb's answer as a tool result: the value as JSON, or the error's
/// own rendering.
fn answer<T: serde::Serialize>(result: Result<T, Error>) -> Result<Value, String> {
    match result {
        Ok(value) => serde_json::to_value(value).map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// The same, for verbs whose success has nothing to say.
fn done(result: Result<(), Error>) -> Result<Value, String> {
    answer(result.map(|()| json!({ "done": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> crate::tools::Registry {
        // Port 1 is reserved; a handler that reached the wire would say
        // so rather than answer.
        let mut registry = crate::tools::Registry::default();
        registry.extend(all(GitHub::at("http://127.0.0.1:1", None), None));
        registry
    }

    #[test]
    fn one_tool_per_public_verb_plus_the_trackers_plan() {
        assert_eq!(all(GitHub::at("http://127.0.0.1:1", None), None).len(), 20);
    }

    #[test]
    fn the_plan_tool_without_a_token_answers_with_the_settings_pointer() {
        let error = registry()
            .dispatch(
                "feature_plan",
                r#"{"repo":"epik-agent/Epik","feature":100}"#,
            )
            .unwrap_err();
        assert!(error.contains("Settings (Cmd+,)"), "{error}");
    }

    #[test]
    fn a_bad_repository_spelling_is_an_err_result_in_words() {
        let error = registry()
            .dispatch("github_issue", r#"{"repo":"not a repo","number":1}"#)
            .unwrap_err();
        assert!(error.contains("owner/name"), "{error}");
    }

    #[test]
    fn a_bare_name_settles_against_the_configured_owner() {
        let mut with_owner = crate::tools::Registry::default();
        with_owner.extend(all(
            GitHub::at("http://127.0.0.1:1", None),
            Some("epik-agent".to_owned()),
        ));
        // Past the spelling check and onto the wire, where port 1 refuses.
        let error = with_owner
            .dispatch("github_issue", r#"{"repo":"Epik","number":1}"#)
            .unwrap_err();
        assert!(!error.contains("owner/name"), "{error}");
        let error = registry()
            .dispatch("github_issue", r#"{"repo":"Epik","number":1}"#)
            .unwrap_err();
        assert!(error.contains("Settings"), "{error}");
    }

    #[test]
    fn a_writing_verb_without_a_token_answers_with_the_settings_pointer() {
        let error = registry()
            .dispatch(
                "github_create_issue",
                r#"{"repo":"epik-agent/Epik","title":"t","body":""}"#,
            )
            .unwrap_err();
        assert!(error.contains("Settings (Cmd+,)"), "{error}");
    }

    #[test]
    fn a_missing_argument_is_an_err_result_in_words() {
        let error = registry()
            .dispatch("github_issue", r#"{"repo":"epik-agent/Epik"}"#)
            .unwrap_err();
        assert!(error.contains("number"), "{error}");
    }
}
