//! The GitHub verbs as tools the model can call.
//!
//! [`TOOLS`] is one [`Spec`] per public verb of the [`GitHub`] client,
//! named `github_` and the verb — the table of everything GitHub the
//! model can do. A row's handler is a [`Verb`]: it takes the client and
//! the repository beside the arguments, because the token lives in the
//! client and never passes through the model, and every verb is about
//! one repository, settled once from the `repo` argument against the
//! host's default owner. [`all`] binds the table to a client.
//! Descriptions are written for a model: one sentence each, and the
//! verbs that need the token say so, so the persona can tell the user
//! what is missing instead of guessing.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use super::spec::{Kind, Param, Spec, optional, required};
use super::{Tool, arg};
use crate::github::{Error, GitHub, Merge, Repo};

/// What a GitHub tool does with the client, the settled repository,
/// and its parsed arguments: a JSON answer, or why not, for the model.
pub type Verb = fn(&GitHub, &Repo, &Value) -> Result<Value>;

/// The `repo` every verb takes.
const REPO: Param = required(
    "repo",
    Kind::String,
    "The repository as one owner/name string, e.g. epik-agent/Epik. A bare name is accepted when a default owner is configured.",
);

/// The `number` of the verbs about one issue.
const NUMBER: Param = required("number", Kind::Integer, "The issue number.");

/// One tool per public verb, in the client's order.
pub const TOOLS: &[Spec<Verb>] = &[
    Spec {
        name: "github_default_branch",
        description: "The default branch name of a GitHub repository.",
        parameters: &[REPO],
        handler: default_branch,
    },
    Spec {
        name: "github_issue",
        description: "One GitHub issue by number: title, body, and open/closed state.",
        parameters: &[REPO, NUMBER],
        handler: issue,
    },
    Spec {
        name: "github_create_issue",
        description: "Opens a new GitHub issue with a title and body. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("title", Kind::String, "The issue title."),
            required(
                "body",
                Kind::String,
                "The issue body, in Markdown; may be empty.",
            ),
        ],
        handler: create_issue,
    },
    Spec {
        name: "github_edit_issue",
        description: "Rewrites a GitHub issue's title, body, or both; an omitted field keeps its text. Needs the GitHub token.",
        parameters: &[
            REPO,
            NUMBER,
            optional(
                "title",
                Kind::String,
                "The new title; omit to keep the current one.",
            ),
            optional(
                "body",
                Kind::String,
                "The new body; omit to keep the current one.",
            ),
        ],
        handler: edit_issue,
    },
    Spec {
        name: "github_comment",
        description: "Adds a comment to a GitHub issue or pull request. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("number", Kind::Integer, "The issue or pull request number."),
            required("body", Kind::String, "The comment body, in Markdown."),
        ],
        handler: comment,
    },
    Spec {
        name: "github_close_issue",
        description: "Closes a GitHub issue. Needs the GitHub token.",
        parameters: &[REPO, NUMBER],
        handler: close_issue,
    },
    Spec {
        name: "github_reopen_issue",
        description: "Reopens a closed GitHub issue. Needs the GitHub token.",
        parameters: &[REPO, NUMBER],
        handler: reopen_issue,
    },
    Spec {
        name: "github_open_pull",
        description: "Opens a GitHub pull request merging branch head into branch base. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("title", Kind::String, "The pull request title."),
            required("head", Kind::String, "The branch to merge from."),
            required("base", Kind::String, "The branch to merge into."),
            required(
                "body",
                Kind::String,
                "The pull request body, in Markdown; may be empty.",
            ),
        ],
        handler: open_pull,
    },
    Spec {
        name: "github_pull",
        description: "One GitHub pull request by number: title, state, merged flag, and both branches.",
        parameters: &[
            REPO,
            required("number", Kind::Integer, "The pull request number."),
        ],
        handler: pull,
    },
    Spec {
        name: "github_pull_for",
        description: "The most recent GitHub pull request a branch produced, in any state; null when the branch never produced one.",
        parameters: &[REPO, required("head", Kind::String, "The branch name.")],
        handler: pull_for,
    },
    Spec {
        name: "github_merge_pull",
        description: "Merges a GitHub pull request by commit, squash, or rebase; an unmergeable one comes back as GitHub's own refusal. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("number", Kind::Integer, "The pull request number."),
            required(
                "method",
                Kind::OneOf(&["commit", "squash", "rebase"]),
                "How to merge.",
            ),
        ],
        handler: merge_pull,
    },
    Spec {
        name: "github_branch_sha",
        description: "The commit SHA a GitHub branch points at; null when there is no such branch.",
        parameters: &[REPO, required("branch", Kind::String, "The branch name.")],
        handler: branch_sha,
    },
    Spec {
        name: "github_compare",
        description: "How one GitHub ref stands relative to another: identical, ahead, behind, or diverged.",
        parameters: &[
            REPO,
            required("base", Kind::String, "The ref to compare against."),
            required("head", Kind::String, "The ref being compared."),
        ],
        handler: compare,
    },
    Spec {
        name: "github_create_branch",
        description: "Creates a GitHub branch at a commit SHA, entirely at the remote. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("branch", Kind::String, "The new branch's name."),
            required("sha", Kind::String, "The commit SHA to start it at."),
        ],
        handler: create_branch,
    },
    Spec {
        name: "github_check_conclusions",
        description: "Every check run's verdict on a GitHub ref — a branch, tag, or commit SHA; a run still executing has a null conclusion.",
        parameters: &[
            REPO,
            required("ref", Kind::String, "The branch name, tag, or commit SHA."),
        ],
        handler: check_conclusions,
    },
    Spec {
        name: "github_issue_graph",
        description: "A GitHub issue with its edges: the sub-issues it decomposes into, the issues blocking it, and the issues it blocks. Needs the GitHub token.",
        parameters: &[REPO, NUMBER],
        handler: issue_graph,
    },
    Spec {
        name: "github_add_sub_issue",
        description: "Makes one GitHub issue a sub-issue of another. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("parent", Kind::Integer, "The parent issue's number."),
            required(
                "child",
                Kind::Integer,
                "The issue to attach as a sub-issue.",
            ),
        ],
        handler: add_sub_issue,
    },
    Spec {
        name: "github_remove_sub_issue",
        description: "Detaches a sub-issue from its parent; both issues survive, the edge does not. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("parent", Kind::Integer, "The parent issue's number."),
            required("child", Kind::Integer, "The sub-issue to detach."),
        ],
        handler: remove_sub_issue,
    },
    Spec {
        name: "github_add_blocked_by",
        description: "Records that one GitHub issue is blocked by another. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("issue", Kind::Integer, "The blocked issue's number."),
            required("blocker", Kind::Integer, "The issue doing the blocking."),
        ],
        handler: add_blocked_by,
    },
    Spec {
        name: "github_remove_blocked_by",
        description: "Removes a blocked-by edge between two GitHub issues — what unblocking is. Needs the GitHub token.",
        parameters: &[
            REPO,
            required("issue", Kind::Integer, "The blocked issue's number."),
            required("blocker", Kind::Integer, "The blocker to remove."),
        ],
        handler: remove_blocked_by,
    },
];

/// Every row of [`TOOLS`] bound to `github`, each call's repository
/// settled against `owner`, the configured default a bare name
/// completes with.
#[must_use]
pub fn all(github: GitHub, owner: Option<String>) -> Vec<Tool> {
    let github = Arc::new(github);
    let owner = Arc::new(owner);
    TOOLS
        .iter()
        .map(|spec| spec.bound(&github, &owner))
        .collect()
}

impl Spec<Verb> {
    /// The tool this row describes, speaking through `github`.
    fn bound(&self, github: &Arc<GitHub>, owner: &Arc<Option<String>>) -> Tool {
        let handler = self.handler;
        let github = Arc::clone(github);
        let owner = Arc::clone(owner);
        Tool::new(
            self.name,
            self.description,
            self.schema(),
            Box::new(move |arguments| {
                let repo = Repo::settle(arg::string(arguments, "repo")?, owner.as_deref())?;
                handler(&github, &repo, arguments).map_err(|error| format!("{error:#}"))
            }),
        )
    }
}

fn default_branch(github: &GitHub, repo: &Repo, _: &Value) -> Result<Value> {
    answer(github.default_branch(repo))
}

fn issue(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.issue(repo, number(arguments, "number")?))
}

fn create_issue(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.create_issue(repo, text(arguments, "title")?, text(arguments, "body")?))
}

fn edit_issue(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.edit_issue(
        repo,
        number(arguments, "number")?,
        arguments["title"].as_str(),
        arguments["body"].as_str(),
    ))
}

fn comment(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    done(github.comment(repo, number(arguments, "number")?, text(arguments, "body")?))
}

fn close_issue(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.close_issue(repo, number(arguments, "number")?))
}

fn reopen_issue(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.reopen_issue(repo, number(arguments, "number")?))
}

fn open_pull(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.open_pull(
        repo,
        text(arguments, "title")?,
        text(arguments, "head")?,
        text(arguments, "base")?,
        text(arguments, "body")?,
    ))
}

fn pull(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.pull(repo, number(arguments, "number")?))
}

fn pull_for(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.pull_for(repo, text(arguments, "head")?))
}

fn merge_pull(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    let method: Merge = serde_json::from_value(arguments["method"].clone())
        .map_err(|_| anyhow!("method must be commit, squash, or rebase"))?;
    done(github.merge_pull(repo, number(arguments, "number")?, method))
}

fn branch_sha(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.branch_sha(repo, text(arguments, "branch")?))
}

fn compare(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.compare(repo, text(arguments, "base")?, text(arguments, "head")?))
}

fn create_branch(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    done(github.create_branch(repo, text(arguments, "branch")?, text(arguments, "sha")?))
}

fn check_conclusions(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.check_conclusions(repo, text(arguments, "ref")?))
}

fn issue_graph(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    answer(github.issue_graph(repo, number(arguments, "number")?))
}

fn add_sub_issue(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    done(github.add_sub_issue(
        repo,
        number(arguments, "parent")?,
        number(arguments, "child")?,
    ))
}

fn remove_sub_issue(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    done(github.remove_sub_issue(
        repo,
        number(arguments, "parent")?,
        number(arguments, "child")?,
    ))
}

fn add_blocked_by(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    done(github.add_blocked_by(
        repo,
        number(arguments, "issue")?,
        number(arguments, "blocker")?,
    ))
}

fn remove_blocked_by(github: &GitHub, repo: &Repo, arguments: &Value) -> Result<Value> {
    done(github.remove_blocked_by(
        repo,
        number(arguments, "issue")?,
        number(arguments, "blocker")?,
    ))
}

// ----- argument reads, each refusal in words -----

fn text<'a>(arguments: &'a Value, name: &str) -> Result<&'a str> {
    arg::string(arguments, name).map_err(anyhow::Error::msg)
}

fn number(arguments: &Value, name: &str) -> Result<u64> {
    arg::number(arguments, name).map_err(anyhow::Error::msg)
}

/// A verb's answer as a tool result: the value as JSON, or the
/// error's own rendering.
fn answer<T: serde::Serialize>(result: Result<T, Error>) -> Result<Value> {
    Ok(serde_json::to_value(result?)?)
}

/// The same, for verbs whose success has nothing to say.
fn done(result: Result<(), Error>) -> Result<Value> {
    result?;
    Ok(json!({ "done": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_tools::Registry;

    /// Port 1 is reserved; a handler that reached the wire would say
    /// so rather than answer.
    fn registry(owner: Option<&str>) -> Registry {
        let mut registry = Registry::default();
        registry.extend(all(
            GitHub::at("http://127.0.0.1:1", None),
            owner.map(str::to_owned),
        ));
        registry
    }

    #[test]
    fn one_tool_per_public_verb() {
        assert_eq!(TOOLS.len(), 20);
        let names: Vec<&str> = TOOLS.iter().map(|spec| spec.name).collect();
        assert!(
            names.iter().all(|name| name.starts_with("github_")),
            "{names:?}"
        );
    }

    #[test]
    fn a_bad_repository_spelling_is_an_err_result_in_words() {
        let error = registry(None)
            .dispatch("github_issue", r#"{"repo":"not a repo","number":1}"#)
            .unwrap_err();
        assert!(error.contains("owner/name"), "{error}");
    }

    #[test]
    fn a_bare_name_settles_against_the_configured_owner() {
        // Past the spelling check and onto the wire, where port 1 refuses.
        let error = registry(Some("epik-agent"))
            .dispatch("github_issue", r#"{"repo":"Epik","number":1}"#)
            .unwrap_err();
        assert!(!error.contains("owner/name"), "{error}");
        let error = registry(None)
            .dispatch("github_issue", r#"{"repo":"Epik","number":1}"#)
            .unwrap_err();
        assert!(error.contains("Settings"), "{error}");
    }

    #[test]
    fn a_writing_verb_without_a_token_answers_with_the_settings_pointer() {
        let error = registry(None)
            .dispatch(
                "github_create_issue",
                r#"{"repo":"epik-agent/Epik","title":"t","body":""}"#,
            )
            .unwrap_err();
        assert!(error.contains("Settings (Cmd+,)"), "{error}");
    }

    #[test]
    fn a_missing_argument_is_an_err_result_in_words() {
        let error = registry(None)
            .dispatch("github_issue", r#"{"repo":"epik-agent/Epik"}"#)
            .unwrap_err();
        assert!(error.contains("number"), "{error}");
    }

    #[test]
    fn a_merge_method_outside_the_three_is_refused_in_words() {
        let error = registry(None)
            .dispatch(
                "github_merge_pull",
                r#"{"repo":"epik-agent/Epik","number":1,"method":"yolo"}"#,
            )
            .unwrap_err();
        assert_eq!(error, "method must be commit, squash, or rebase");
    }
}
