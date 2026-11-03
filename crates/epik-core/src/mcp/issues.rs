//! `issue_create`: creates an issue and adds it to its repository's linked
//! projects.

use serde::Deserialize;
use serde_json::{Value, json};

use super::gh::{Gh, strings};
use super::{Args, Context, Reply, integer, string};
use crate::github::{self, GhCli};

/// An issue as the REST API returns it.
#[derive(Deserialize)]
struct Created {
    number: u64,
    html_url: String,
    node_id: String,
}

pub fn create(context: &Context, args: &Args) -> Result<Reply, String> {
    let repo = string(args, "repo").unwrap_or_default();
    let title = string(args, "title").unwrap_or_default();
    let body = string(args, "body").unwrap_or_default();
    let path = format!("repos/{repo}/issues");
    let mut command = strings([
        "api",
        "-X",
        "POST",
        &path,
        "-f",
        &format!("title={title}"),
        "-f",
        &format!("body={body}"),
    ]);
    for label in args
        .get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        command.extend(strings(["-f", &format!("labels[]={label}")]));
    }
    if let Some(milestone) = integer(args, "milestone") {
        command.extend(strings(["-F", &format!("milestone={milestone}")]));
    }
    let created: Created = context.gh.json(&command)?;
    Ok(Reply::Json(json!({
        "number": created.number,
        "url": created.html_url,
        "project_failures": add_to_linked_projects(&context.gh, repo, &created.node_id),
    })))
}

/// Adds the issue with node ID `issue` to each of `repo`'s linked projects,
/// and says what went wrong, naming each project that failed.
fn add_to_linked_projects(gh: &Gh, repo: &str, issue: &str) -> Vec<String> {
    let gh = gh.clone();
    let github = GhCli::with(move |args| {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        gh.text(&args).map_err(github::Error::new)
    });
    let projects = match github.linked_projects(repo) {
        Ok(projects) => projects,
        Err(error) => {
            return vec![format!(
                "couldn't read the linked projects of {repo}: {error}"
            )];
        }
    };
    projects
        .iter()
        .filter_map(|project| {
            let added = github.add_to_project(project, issue);
            added
                .err()
                .map(|error| format!("{}: {error}", project.title))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::mcp::gh::{Gh, Output};
    use crate::mcp::tests::{Calls, call, call_json, scripted, server_with};
    use serde_json::{Value, json};

    const CREATED: &str = "repos/o/r/issues";

    fn created() -> Value {
        json!({"number": 5, "html_url": "https://github.com/o/r/issues/5", "node_id": "issue-5"})
    }

    fn page(nodes: Value) -> Value {
        json!([{"data": {"repository": {"projectsV2": {"nodes": nodes}}}}])
    }

    fn project(id: &str, title: &str, closed: bool) -> Value {
        json!({"id": id, "title": title, "closed": closed, "field": null})
    }

    fn added() -> Value {
        json!({"data": {"addProjectV2ItemById": {"item": {"id": "item"}}}})
    }

    fn args() -> Value {
        json!({"repo": "o/r", "title": "A title", "body": "A body"})
    }

    /// The calls whose arguments, joined by spaces, contain `needle`.
    fn calls_with(calls: &Calls, needle: &str) -> Vec<Vec<String>> {
        let calls = calls.lock().unwrap();
        let found = calls.iter().filter(|args| args.join(" ").contains(needle));
        found.cloned().collect()
    }

    #[test]
    fn with_no_linked_project_it_just_creates_the_issue() {
        let (gh, calls) = scripted(vec![("projectsV2", page(json!([]))), (CREATED, created())]);

        let reply = call_json(gh, "issue_create", args()).unwrap();

        assert_eq!(
            reply,
            json!({"number": 5, "url": "https://github.com/o/r/issues/5", "project_failures": []})
        );
        assert_eq!(
            calls.lock().unwrap()[0],
            [
                "api",
                "-X",
                "POST",
                "repos/o/r/issues",
                "-f",
                "title=A title",
                "-f",
                "body=A body",
            ]
        );
        assert_eq!(
            calls_with(&calls, "addProjectV2ItemById"),
            [] as [Vec<String>; 0]
        );
    }

    #[test]
    fn labels_and_a_milestone_go_with_the_issue() {
        let (gh, calls) = scripted(vec![("projectsV2", page(json!([]))), (CREATED, created())]);
        let mut args = args();
        args["labels"] = json!(["bug", "epic"]);
        args["milestone"] = json!(3);

        call_json(gh, "issue_create", args).unwrap();

        assert_eq!(
            calls.lock().unwrap()[0][8..],
            [
                "-f",
                "labels[]=bug",
                "-f",
                "labels[]=epic",
                "-F",
                "milestone=3",
            ]
        );
    }

    #[test]
    fn it_adds_the_issue_to_each_open_linked_project_and_skips_closed_ones() {
        let (gh, calls) = scripted(vec![
            ("addProjectV2ItemById", added()),
            (
                "projectsV2",
                page(json!([
                    project("open-1", "One", false),
                    project("closed", "Old", true),
                    project("open-2", "Two", false),
                ])),
            ),
            (CREATED, created()),
        ]);

        let reply = call_json(gh, "issue_create", args()).unwrap();

        assert_eq!(reply["project_failures"], json!([]));
        let adds: Vec<Vec<String>> = calls_with(&calls, "addProjectV2ItemById")
            .into_iter()
            .map(|add| add[add.len() - 4..].to_vec())
            .collect();
        assert_eq!(
            adds,
            [
                ["-f", "project=open-1", "-f", "content=issue-5"],
                ["-f", "project=open-2", "-f", "content=issue-5"],
            ]
        );
    }

    #[test]
    fn it_still_returns_the_issue_when_adding_it_fails_and_names_the_project() {
        let gh = Gh::with(|args| {
            let call = args.join(" ");
            let reply = |body: Value| Output {
                exit_code: 0,
                stdout: body.to_string(),
                stderr: String::new(),
            };
            Ok(if call.contains("project=bad") {
                Output {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: "forbidden\n".to_owned(),
                }
            } else if call.contains("addProjectV2ItemById") {
                reply(added())
            } else if call.contains("projectsV2") {
                reply(page(json!([
                    project("good", "Fine", false),
                    project("bad", "Roadmap", false),
                ])))
            } else {
                reply(created())
            })
        });

        let reply = call_json(gh, "issue_create", args()).unwrap();

        assert_eq!(reply["number"], 5);
        assert_eq!(
            reply["project_failures"],
            json!(["Roadmap: forbidden\n(exit code 1)"])
        );
    }

    #[test]
    fn it_still_returns_the_issue_when_the_projects_cannot_be_read() {
        // No reply for the projects' query.
        let (gh, _) = scripted(vec![(CREATED, created())]);

        let reply = call_json(gh, "issue_create", args()).unwrap();

        assert_eq!(reply["number"], 5);
        let failure = reply["project_failures"][0].as_str().unwrap();
        assert!(
            failure.starts_with("couldn't read the linked projects of o/r: "),
            "{failure}"
        );
    }

    #[test]
    fn an_issue_that_cannot_be_created_is_the_tools_error() {
        let (gh, _) = scripted(vec![]);

        let error = call(gh, "issue_create", args()).unwrap_err();

        assert!(error.contains("repos/o/r/issues"), "{error}");
    }

    #[test]
    fn a_title_is_required() {
        let (gh, _) = scripted(vec![]);
        let server = server_with(gh);

        let bad = json!({"repo": "o/r", "title": "", "body": "b"});

        assert!(server.call("issue_create", bad).is_err());
    }
}
