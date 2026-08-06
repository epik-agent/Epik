//! The GitHub verbs as tools: the client's surface, worded for a model.
//!
//! [`register`] puts every verb of [`GitHub`] into a [`Registry`]. Each
//! entry's description and schema are prompt engineering, not documentation:
//! a model reads nothing else when it decides whether to call, which verb to
//! call, and what to put in the arguments — so the words name concrete
//! situations ("is this branch green", "what is blocking this issue") and
//! every parameter says exactly what belongs in it.
//!
//! The registry builds with no token. The verbs that need one — every write,
//! and the issue graph, which is GraphQL — refuse at call time with the
//! client's typed [`Error::TokenAbsent`](super::Error::TokenAbsent), which
//! rides the registry's `Failed` error into the chat loop, where it is
//! rendered into the tool result the model reads and the refusal event the
//! window shows. Nothing here panics for want of a token, and nothing here
//! asks for confirmation: whether a mutation deserves ceremony is the
//! window's question, never the registry's.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::github::{GitHub, Merge, Repo};
use crate::tools::{Registry, Tool};

/// Registers the GitHub verbs into `registry`, all backed by `github`.
///
/// # Panics
///
/// Panics when `registry` already holds a tool by one of these names, as
/// [`Registry::register`] does.
pub fn register(registry: &mut Registry, github: GitHub) {
    let github = Arc::new(github);

    let on = Arc::clone(&github);
    registry.register(issue_graph(), move |args: OnIssue| {
        on.issue_graph(&args.at.into_repo(), args.number)
    });

    let on = Arc::clone(&github);
    registry.register(create_issue(), move |args: CreateIssue| {
        on.create_issue(&args.at.into_repo(), &args.title, &args.body)
    });

    let on = Arc::clone(&github);
    registry.register(edit_issue(), move |args: EditIssue| {
        on.edit_issue(
            &args.at.into_repo(),
            args.number,
            args.title.as_deref(),
            args.body.as_deref(),
        )
    });

    let on = Arc::clone(&github);
    registry.register(comment(), move |args: OnIssueBody| {
        let repo = args.at.into_repo();
        on.comment(&repo, args.number, &args.body)?;
        Ok::<_, super::Error>(format!("Commented on {repo}#{}.", args.number))
    });

    let on = Arc::clone(&github);
    registry.register(close_issue(), move |args: OnIssue| {
        on.close_issue(&args.at.into_repo(), args.number)
    });

    let on = Arc::clone(&github);
    registry.register(open_pull(), move |args: OpenPull| {
        on.open_pull(
            &args.at.into_repo(),
            &args.title,
            &args.head,
            &args.base,
            &args.body,
        )
    });

    let on = Arc::clone(&github);
    registry.register(merge_pull(), move |args: MergePull| {
        let repo = args.at.into_repo();
        on.merge_pull(&repo, args.number, args.method)?;
        Ok::<_, super::Error>(format!("Merged {repo}#{}.", args.number))
    });

    let on = Arc::clone(&github);
    registry.register(check_conclusions(), move |args: OnRef| {
        on.check_conclusions(&args.at.into_repo(), &args.git_ref)
    });

    let on = github;
    registry.register(default_branch(), move |args: At| {
        on.default_branch(&args.into_repo())
    });
}

// ----- the faces: one constructor per verb, holding all the wording -----

fn issue_graph() -> Tool {
    Tool::new(
        "github_issue_graph",
        "Read one GitHub issue together with its relationship edges: its title, body, and \
         open/closed state, the sub-issues it is decomposed into, and the issues it is \
         blocked by. The first call to make for any question about what an issue says, what \
         its children are, or what stands in its way. Needs a GitHub token, even for a \
         public repository.",
        schema(
            &[number("The issue number, without the leading #.")],
            &["number"],
        ),
    )
}

fn create_issue() -> Tool {
    Tool::new(
        "github_create_issue",
        "Open a new GitHub issue. Answers with the created issue, including the number \
         GitHub assigned it.",
        schema(
            &[
                (
                    "title",
                    json!({
                        "type": "string",
                        "description": "The issue's title: one line, no trailing period.",
                    }),
                ),
                (
                    "body",
                    json!({
                        "type": "string",
                        "description": "The issue's description, as markdown. Omit only \
                                        when the title says everything.",
                    }),
                ),
            ],
            &["title"],
        ),
    )
}

fn edit_issue() -> Tool {
    Tool::new(
        "github_edit_issue",
        "Rewrite an existing GitHub issue's title, body, or both. Provide at least one of \
         the two; a field left out keeps its current text. The body replaces the whole \
         body, so include everything the issue should say, not just the change.",
        schema(
            &[
                number("The number of the issue to edit, without the leading #."),
                (
                    "title",
                    json!({
                        "type": "string",
                        "description": "The issue's new title. Omit to keep the current one.",
                    }),
                ),
                (
                    "body",
                    json!({
                        "type": "string",
                        "description": "The issue's new body, as markdown, replacing the \
                                        old body entirely. Omit to keep the current one.",
                    }),
                ),
            ],
            &["number"],
        ),
    )
}

fn comment() -> Tool {
    Tool::new(
        "github_comment",
        "Add a comment to a GitHub issue or pull request — pull requests take comments \
         under their own number here. The comment posts as the authenticated user, \
         verbatim.",
        schema(
            &[
                number("The issue or pull request number, without the leading #."),
                (
                    "body",
                    json!({
                        "type": "string",
                        "description": "The comment, as markdown, exactly as it should \
                                        appear.",
                    }),
                ),
            ],
            &["number", "body"],
        ),
    )
}

fn close_issue() -> Tool {
    Tool::new(
        "github_close_issue",
        "Close a GitHub issue. Answers with the issue as it now stands. To explain why, \
         comment first with github_comment; closing itself says nothing.",
        schema(
            &[number(
                "The number of the issue to close, without the leading #.",
            )],
            &["number"],
        ),
    )
}

fn open_pull() -> Tool {
    Tool::new(
        "github_open_pull",
        "Open a GitHub pull request proposing to merge one branch into another. Both \
         branches must already exist on GitHub. Answers with the created pull request, \
         including its number.",
        schema(
            &[
                (
                    "title",
                    json!({
                        "type": "string",
                        "description": "The pull request's title: one line, no trailing \
                                        period.",
                    }),
                ),
                (
                    "head",
                    json!({
                        "type": "string",
                        "description": "The branch carrying the changes, e.g. \
                                        \"issue-42-fix\".",
                    }),
                ),
                (
                    "base",
                    json!({
                        "type": "string",
                        "description": "The branch the changes merge into — usually the \
                                        default branch; read it with \
                                        github_default_branch when unsure.",
                    }),
                ),
                (
                    "body",
                    json!({
                        "type": "string",
                        "description": "The pull request's description, as markdown: what \
                                        changed and why.",
                    }),
                ),
            ],
            &["title", "head", "base"],
        ),
    )
}

fn merge_pull() -> Tool {
    Tool::new(
        "github_merge_pull",
        "Merge a GitHub pull request. Fails with GitHub's own reason when it is not \
         mergeable — failing checks, conflicts, or branch protection — so check \
         github_check_conclusions on its head branch first when merging depends on green.",
        schema(
            &[
                number("The number of the pull request to merge, without the leading #."),
                (
                    "method",
                    json!({
                        "type": "string",
                        "enum": ["merge", "squash", "rebase"],
                        "description": "How to merge: \"squash\" collapses the branch into \
                                        one commit, \"merge\" keeps its commits under a \
                                        merge commit, \"rebase\" replays them with no merge \
                                        commit. Follow the repository's convention; this \
                                        project squashes.",
                    }),
                ),
            ],
            &["number", "method"],
        ),
    )
}

fn check_conclusions() -> Tool {
    Tool::new(
        "github_check_conclusions",
        "Read every CI check's verdict on a branch, tag, or commit. A conclusion of \
         \"success\" is green, anything else is not, and a null conclusion means the check \
         is still running. The way to answer whether a branch or pull request is passing.",
        schema(
            &[(
                "ref",
                json!({
                    "type": "string",
                    "description": "What to read the checks of: a branch name, a tag, or a \
                                    full commit SHA.",
                }),
            )],
            &["ref"],
        ),
    )
}

fn default_branch() -> Tool {
    Tool::new(
        "github_default_branch",
        "Read a GitHub repository's default branch name — the branch pull requests merge \
         into unless told otherwise, and the one to read CI conclusions from for the \
         repository's mainline.",
        schema(&[], &[]),
    )
}

/// Arguments naming a repository — the pair every verb starts with.
#[derive(Deserialize)]
struct At {
    owner: String,
    repo: String,
}

impl At {
    fn into_repo(self) -> Repo {
        Repo::new(self.owner, self.repo)
    }
}

#[derive(Deserialize)]
struct OnIssue {
    #[serde(flatten)]
    at: At,
    number: u64,
}

#[derive(Deserialize)]
struct OnIssueBody {
    #[serde(flatten)]
    at: At,
    number: u64,
    body: String,
}

#[derive(Deserialize)]
struct CreateIssue {
    #[serde(flatten)]
    at: At,
    title: String,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct EditIssue {
    #[serde(flatten)]
    at: At,
    number: u64,
    title: Option<String>,
    body: Option<String>,
}

#[derive(Deserialize)]
struct OpenPull {
    #[serde(flatten)]
    at: At,
    title: String,
    head: String,
    base: String,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct MergePull {
    #[serde(flatten)]
    at: At,
    number: u64,
    method: Merge,
}

#[derive(Deserialize)]
struct OnRef {
    #[serde(flatten)]
    at: At,
    #[serde(rename = "ref")]
    git_ref: String,
}

/// The property every issue-shaped verb shares, under one wording.
fn number(description: &str) -> (&str, Value) {
    (
        "number",
        json!({ "type": "integer", "description": description }),
    )
}

/// An argument schema: the `owner`/`repo` pair every verb takes, `extra`
/// properties after it, and `required` naming which of the extras are.
fn schema(extra: &[(&str, Value)], required: &[&str]) -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "owner".to_owned(),
        json!({
            "type": "string",
            "description": "The repository owner: a user or organization login, e.g. \
                            \"epik-agent\" in epik-agent/Epik.",
        }),
    );
    properties.insert(
        "repo".to_owned(),
        json!({
            "type": "string",
            "description": "The repository name without its owner, e.g. \"Epik\" in \
                            epik-agent/Epik.",
        }),
    );
    for (name, property) in extra {
        properties.insert((*name).to_owned(), property.clone());
    }
    let mut require = vec!["owner", "repo"];
    require.extend(required);
    json!({ "type": "object", "properties": properties, "required": require })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::super::Error;
    use super::*;
    use crate::chat::{Conversation, Scripted, StopToken, ToolCall};
    use crate::event::ChatEvent;
    use crate::keystore::GITHUB_OVERRIDE_ENV;

    /// Captured payloads from the real API, shared with the client's own
    /// tests; provenance in `fixtures/README.md`.
    const REPO: &str = include_str!("fixtures/repo.json");
    const GRAPH_PARENT: &str = include_str!("fixtures/graph_parent.json");

    /// A registry over a client that is told a token — or not — and where
    /// GitHub is. No test here reads the environment: the token is always
    /// stated, so a real `EPIK_GITHUB_TOKEN` in the shell changes nothing.
    fn registry(api: &str, token: Option<&str>) -> Registry {
        let mut registry = Registry::new();
        register(&mut registry, GitHub::at(api, token.map(str::to_owned)));
        registry
    }

    /// A registry whose client can reach nothing: port 1 is reserved, so a
    /// verb that got past its own preconditions would fail as `Wire`, never
    /// as `TokenAbsent`.
    fn unreachable(token: Option<&str>) -> Registry {
        registry("http://127.0.0.1:1", token)
    }

    /// Serves exactly one HTTP exchange: reads the request, answers with
    /// `status` and `body`, and hands back the request line and request body
    /// it saw — which is how a test asserts the arguments really reached the
    /// wire.
    fn serving(status: u16, body: &'static str) -> (String, thread::JoinHandle<(String, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if let Some(value) = header.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if header.trim().is_empty() {
                    break;
                }
            }
            let mut sent = vec![0; length];
            reader.read_exact(&mut sent).unwrap();
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 {status} Fine\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            (
                request_line.trim().to_owned(),
                String::from_utf8(sent).unwrap(),
            )
        });
        (api, handle)
    }

    #[test]
    fn the_registry_lists_every_github_verb() {
        let names: Vec<_> = unreachable(None)
            .tools()
            .map(|tool| tool.name.clone())
            .collect();
        assert_eq!(
            names,
            [
                "github_issue_graph",
                "github_create_issue",
                "github_edit_issue",
                "github_comment",
                "github_close_issue",
                "github_open_pull",
                "github_merge_pull",
                "github_check_conclusions",
                "github_default_branch",
            ]
        );
    }

    #[test]
    fn every_entry_is_worded_for_a_model_deciding_whether_to_call() {
        // The registry entry is the model's whole briefing, so this is a
        // gate on the deliverable: no empty descriptions, no undescribed
        // parameters, no required name that is not a parameter.
        for tool in unreachable(None).tools() {
            assert!(
                tool.description.len() > 40,
                "{}: a one-word description briefs nobody",
                tool.name
            );
            assert_eq!(tool.schema["type"], "object", "{}", tool.name);
            let properties = tool.schema["properties"].as_object().unwrap();
            for (parameter, property) in properties {
                assert!(
                    !property["description"].as_str().unwrap_or("").is_empty(),
                    "{}.{parameter} says nothing about what belongs in it",
                    tool.name
                );
            }
            for required in tool.schema["required"].as_array().unwrap() {
                let required = required.as_str().unwrap();
                assert!(
                    properties.contains_key(required),
                    "{} requires {required}, which its properties never name",
                    tool.name
                );
            }
            assert!(properties.contains_key("owner") && properties.contains_key("repo"));
        }
    }

    #[test]
    fn a_read_runs_through_the_real_client_to_the_stated_repository() {
        let (api, server) = serving(200, REPO);
        let mut registry = registry(&api, None);

        let answer = registry
            .call(
                "github_default_branch",
                r#"{"owner": "epik-agent", "repo": "Epik"}"#,
            )
            .unwrap();

        assert_eq!(answer, json!("main"));
        let (request_line, _) = server.join().unwrap();
        assert_eq!(
            request_line, "GET /repos/epik-agent/Epik HTTP/1.1",
            "the arguments name the endpoint, verbatim"
        );
    }

    #[test]
    fn the_issue_graph_answers_in_json_a_model_can_fold_over() {
        let (api, server) = serving(200, GRAPH_PARENT);
        let mut registry = registry(&api, Some("ghp-test"));

        let answer = registry
            .call(
                "github_issue_graph",
                r#"{"owner": "epik-agent", "repo": "Epik", "number": 102}"#,
            )
            .unwrap();

        assert_eq!(answer["issue"]["number"], 102);
        assert_eq!(answer["issue"]["state"], "open");
        let children: Vec<_> = answer["sub_issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|edge| edge["number"].as_u64().unwrap())
            .collect();
        assert_eq!(children, [105, 106]);
        assert_eq!(answer["blocked_by"], json!([]));
        let (request_line, _) = server.join().unwrap();
        assert_eq!(request_line, "POST /graphql HTTP/1.1");
    }

    #[test]
    fn a_comment_lands_where_the_arguments_say_and_confirms_in_words() {
        let (api, server) = serving(201, "{}");
        let mut registry = registry(&api, Some("ghp-test"));

        let answer = registry
            .call(
                "github_comment",
                r#"{"owner": "epik-agent", "repo": "Epik", "number": 115, "body": "On it."}"#,
            )
            .unwrap();

        assert_eq!(answer, json!("Commented on epik-agent/Epik#115."));
        let (request_line, sent) = server.join().unwrap();
        assert_eq!(
            request_line,
            "POST /repos/epik-agent/Epik/issues/115/comments HTTP/1.1"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&sent).unwrap(),
            json!({ "body": "On it." })
        );
    }

    #[test]
    fn a_merge_names_its_method_on_the_wire() {
        let (api, server) = serving(200, r#"{"merged": true}"#);
        let mut registry = registry(&api, Some("ghp-test"));

        let answer = registry
            .call(
                "github_merge_pull",
                r#"{"owner": "epik-agent", "repo": "Epik", "number": 9, "method": "squash"}"#,
            )
            .unwrap();

        assert_eq!(answer, json!("Merged epik-agent/Epik#9."));
        let (request_line, sent) = server.join().unwrap();
        assert_eq!(
            request_line,
            "PUT /repos/epik-agent/Epik/pulls/9/merge HTTP/1.1"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&sent).unwrap(),
            json!({ "merge_method": "squash" })
        );
    }

    #[test]
    fn every_verb_that_needs_a_token_refuses_typed_with_no_wire_touched() {
        let mut registry = unreachable(None);
        let needy = [
            ("github_issue_graph", r#"{"number": 1}"#),
            ("github_create_issue", r#"{"title": "t"}"#),
            ("github_edit_issue", r#"{"number": 1, "title": "t"}"#),
            ("github_comment", r#"{"number": 1, "body": "b"}"#),
            ("github_close_issue", r#"{"number": 1}"#),
            (
                "github_open_pull",
                r#"{"title": "t", "head": "h", "base": "b"}"#,
            ),
            ("github_merge_pull", r#"{"number": 1, "method": "squash"}"#),
        ];
        for (name, extra) in needy {
            let arguments = format!(
                r#"{{"owner": "o", "repo": "r", {}"#,
                extra.trim_start_matches('{')
            );
            let error = registry.call(name, &arguments).unwrap_err();
            let crate::tools::Error::Failed { error, .. } = error else {
                panic!("{name}: a missing token should be the tool's own failure, got {error}");
            };
            assert_eq!(
                error.downcast_ref::<Error>(),
                Some(&Error::TokenAbsent),
                "{name}: the keystore's refusal survives the registry typed"
            );
        }
    }

    #[test]
    fn the_tokenless_refusal_reaches_a_transcript_as_the_tool_result() {
        // The vertical slice in miniature: a model asks for a comment, the
        // registry has no token, and the refusal — with its remedy — is what
        // the model reads back and what the window is told.
        let mut registry = unreachable(None);
        let mut chat = Conversation::new(
            "You are Epik.",
            Scripted::saying(["Commenting."])
                .asking([ToolCall::new(
                    "call-1",
                    "github_comment",
                    r#"{"owner": "epik-agent", "repo": "Epik", "number": 115, "body": "hi"}"#,
                )])
                .then_saying(["There is no GitHub token to comment with."]),
        );
        let mut events: Vec<ChatEvent> = Vec::new();

        chat.send_with_tools(
            "Comment on #115",
            &mut registry,
            &mut events,
            &StopToken::new(),
        )
        .expect("a refused tool call is not the turn's failure");

        let result = chat.model().seen()[1]
            .iter()
            .find_map(|message| match message {
                crate::chat::Message::Tool { content, .. } => Some(content.clone()),
                _ => None,
            })
            .expect("the refusal goes back to the model as a tool result");
        assert!(result.contains("no GitHub token"), "{result}");
        assert!(
            result.contains(GITHUB_OVERRIDE_ENV),
            "the refusal says how to supply one: {result}"
        );
        assert!(
            events.iter().any(
                |event| matches!(event, ChatEvent::ToolCallRefused { error, .. }
                    if error.contains("no GitHub token"))
            ),
            "and the window hears the same words: {events:?}"
        );
    }

    #[test]
    fn reads_that_need_no_token_get_past_the_precondition_without_one() {
        // Tokenless, the read verbs go for the wire and fail there — proof
        // the precondition is per-verb, not a gate on the whole registry.
        let mut registry = unreachable(None);
        for (name, arguments) in [
            ("github_default_branch", r#"{"owner": "o", "repo": "r"}"#),
            (
                "github_check_conclusions",
                r#"{"owner": "o", "repo": "r", "ref": "main"}"#,
            ),
        ] {
            let error = registry.call(name, arguments).unwrap_err();
            let crate::tools::Error::Failed { error, .. } = error else {
                panic!("{name}: expected the wire's failure, got {error}");
            };
            assert!(
                matches!(error.downcast_ref::<Error>(), Some(Error::Wire(_))),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn arguments_that_do_not_fit_a_verb_refuse_as_arguments() {
        let mut registry = unreachable(None);
        let error = registry
            .call("github_issue_graph", r#"{"owner": "o", "repo": "r"}"#)
            .unwrap_err();
        assert!(
            matches!(&error, crate::tools::Error::Arguments { name, .. }
                if name == "github_issue_graph"),
            "a missing number is worded for the model to try again: {error}"
        );
    }
}
