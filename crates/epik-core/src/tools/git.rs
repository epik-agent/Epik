//! The git vocabulary as tools the model can call.
//!
//! [`all`] is one [`Tool`] per [`Command`], named `git_` and the verb.
//! Every handler is the same three steps: read the arguments into the
//! command, where every check already is; run the verb; and write what
//! it read back as JSON. A repository arrives as its absolute path and
//! goes through [`Repository::open`], so a command runs only in a
//! repository that exists. Whatever is refused — by the command, by
//! git — comes back in words as an Err result the model reads.
//!
//! [`Command`]: git::commands::Command

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use super::schema::{boolean, integer, object, string, strings};
use super::{Tool, arg};
use crate::git::commands::{
    AbortMerge, Add, AddWorktree, Branch, Clone, Commit, DeleteBranch, Diff, Fetch, Hash, Init,
    ListRemotes, ListWorktrees, Log, Merge, Pull, Push, Remote, RemoveWorktree, Reset, RevParse,
    SetConfig, Show, Status, Switch,
};
use crate::git::url::Url;
use crate::git::{self, Repository};

/// One tool per git command, in the commands' order.
#[must_use]
pub fn all() -> Vec<Tool> {
    vec![
        tool(
            "git_init",
            "Makes a new bare repository at path, on branch main with no commits yet, and answers with the repository.",
            object(
                &[(
                    "path",
                    string(
                        "The absolute path to make the repository at, which does not exist yet.",
                    ),
                )],
                &["path"],
            ),
            |arguments| {
                let repository = git::init(Init::new(path(arguments, "path")?)?)?;
                Ok(made(&repository))
            },
        ),
        tool(
            "git_clone",
            "Clones from a URL or local path into a directory that does not exist yet, checked out at branch when one is named, and answers with the repository.",
            object(
                &[
                    (
                        "from",
                        string("What to clone: a URL, or the absolute path of a local repository."),
                    ),
                    (
                        "into",
                        string("The absolute path to clone into, which does not exist yet."),
                    ),
                    (
                        "branch",
                        string("The branch to check out; the remote's default when absent."),
                    ),
                ],
                &["from", "into"],
            ),
            |arguments| {
                let from: Url = text(arguments, "from")?.parse()?;
                let clone =
                    Clone::new(from, path(arguments, "into")?, owned(arguments, "branch")?)?;
                Ok(made(&git::clone(clone)?))
            },
        ),
        tool(
            "git_set_config",
            "Sets key to value in the repository's own config, such as the user.name and user.email its commits are made under.",
            within(
                &[
                    ("key", string("The config key, such as user.name.")),
                    ("value", string("The value to set.")),
                ],
                &["key", "value"],
            ),
            |arguments| {
                let key = text(arguments, "key")?.to_owned();
                let value = text(arguments, "value")?.to_owned();
                git::set_config(SetConfig::new(repository(arguments)?, key, value)?)?;
                Ok(done())
            },
        ),
        tool(
            "git_branch",
            "Makes a branch, starting at the commit named by at or at HEAD.",
            within(
                &[
                    ("name", string("The branch to make.")),
                    (
                        "at",
                        string("The full hash of the commit to start at; HEAD when absent."),
                    ),
                ],
                &["name"],
            ),
            |arguments| {
                let name = text(arguments, "name")?.to_owned();
                let at = optional(arguments, "at")?.map(Hash::new).transpose()?;
                git::branch(Branch::new(repository(arguments)?, name, at)?)?;
                Ok(done())
            },
        ),
        tool(
            "git_switch",
            "Checks out a branch that exists.",
            within(
                &[("branch", string("The branch to check out."))],
                &["branch"],
            ),
            |arguments| {
                let branch = text(arguments, "branch")?.to_owned();
                git::switch(Switch::new(repository(arguments)?, branch)?)?;
                Ok(done())
            },
        ),
        tool(
            "git_add_worktree",
            "Checks out a branch that exists as a new worktree at path, sharing the repository's history, and answers with the worktree as a repository.",
            within(
                &[
                    (
                        "path",
                        string("The absolute path of the worktree, which does not exist yet."),
                    ),
                    ("branch", string("The branch to check out there.")),
                ],
                &["path", "branch"],
            ),
            |arguments| {
                let path = path(arguments, "path")?;
                let branch = text(arguments, "branch")?.to_owned();
                let worktree =
                    git::add_worktree(AddWorktree::new(repository(arguments)?, path, branch)?)?;
                Ok(made(&worktree))
            },
        ),
        tool(
            "git_list_worktrees",
            "Every worktree of the repository, the main one first, with the commit and branch each has checked out.",
            within(&[], &[]),
            |arguments| {
                let worktrees = git::list_worktrees(ListWorktrees::new(repository(arguments)?))?;
                Ok(json!({
                    "worktrees": worktrees.iter().map(|worktree| json!({
                        "path": worktree.path.to_string_lossy(),
                        "head": worktree.head.as_ref().map(ToString::to_string),
                        "branch": worktree.branch,
                        "bare": worktree.bare,
                    })).collect::<Vec<_>>(),
                }))
            },
        ),
        tool(
            "git_add",
            "Stages paths, relative to the repository, for the next commit.",
            within(
                &[(
                    "paths",
                    strings("The paths to stage, relative to the repository; at least one."),
                )],
                &["paths"],
            ),
            |arguments| {
                git::add(Add::new(repository(arguments)?, paths(arguments)?)?)?;
                Ok(done())
            },
        ),
        tool(
            "git_status",
            "Every changed path in the repository, with its state in the index and in the working tree as git's one-letter codes.",
            within(&[], &[]),
            |arguments| {
                let changes = git::status(Status::new(repository(arguments)?))?;
                Ok(json!({
                    "changes": changes.iter().map(|change| json!({
                        "staged": change.staged.to_string(),
                        "unstaged": change.unstaged.to_string(),
                        "path": change.path.to_string_lossy(),
                        "from": change.from.as_ref().map(|from| from.to_string_lossy()),
                    })).collect::<Vec<_>>(),
                }))
            },
        ),
        tool(
            "git_diff",
            "The patch of the working tree against the index, or across revisions such as main..feature, narrowed to one path when given.",
            within(
                &[
                    (
                        "revisions",
                        string(
                            "A revision or range to diff, such as HEAD~1 or main..feature; the working tree against the index when absent.",
                        ),
                    ),
                    (
                        "path",
                        string("One path, relative to the repository, to narrow the patch to."),
                    ),
                ],
                &[],
            ),
            |arguments| {
                let revisions = owned(arguments, "revisions")?;
                let path = optional(arguments, "path")?.map(PathBuf::from);
                let patch = git::diff(Diff::new(repository(arguments)?, revisions, path)?)?;
                Ok(said(patch))
            },
        ),
        tool(
            "git_commit",
            "Commits what is staged, under a message, as the identity the repository's config gives.",
            within(&[("message", string("The commit message."))], &["message"]),
            |arguments| {
                let message = text(arguments, "message")?.to_owned();
                git::commit(Commit::new(repository(arguments)?, message)?)?;
                Ok(done())
            },
        ),
        tool(
            "git_rev_parse",
            "The full hash of the commit a revision names, such as HEAD, a branch, or a tag.",
            within(
                &[("revision", string("The revision to resolve."))],
                &["revision"],
            ),
            |arguments| {
                let revision = text(arguments, "revision")?.to_owned();
                let hash = git::rev_parse(RevParse::new(repository(arguments)?, revision)?)?;
                Ok(json!({ "hash": hash.to_string() }))
            },
        ),
        tool(
            "git_log",
            "The commits behind a revision, newest first: hash, author, date, and subject, at most count of them.",
            within(
                &[
                    (
                        "revision",
                        string("The revision to read the history behind; HEAD when absent."),
                    ),
                    (
                        "count",
                        integer("The most commits to answer with; all of them when absent."),
                    ),
                ],
                &[],
            ),
            |arguments| {
                let revision = owned(arguments, "revision")?;
                let count = count(arguments)?;
                let entries = git::log(Log::new(repository(arguments)?, revision, count)?)?;
                Ok(json!({
                    "entries": entries.iter().map(|entry| json!({
                        "hash": entry.hash.to_string(),
                        "author": entry.author,
                        "date": entry.date.to_rfc3339(),
                        "subject": entry.subject,
                    })).collect::<Vec<_>>(),
                }))
            },
        ),
        tool(
            "git_show",
            "One commit, message and patch, by revision: a hash, branch, or tag.",
            within(
                &[("revision", string("The revision to show."))],
                &["revision"],
            ),
            |arguments| {
                let revision = text(arguments, "revision")?.to_owned();
                Ok(said(git::show(Show::new(
                    repository(arguments)?,
                    revision,
                )?)?))
            },
        ),
        tool(
            "git_fetch",
            "Fetches from a remote, using the user's own git credentials.",
            within(&[("remote", string(REMOTE))], &["remote"]),
            |arguments| {
                git::fetch(Fetch::new(repository(arguments)?, remote(arguments)?))?;
                Ok(done())
            },
        ),
        tool(
            "git_pull",
            "Brings the checked-out branch up to its upstream, fast-forward only, using the user's own git credentials.",
            within(&[], &[]),
            |arguments| {
                git::pull(Pull::new(repository(arguments)?))?;
                Ok(done())
            },
        ),
        tool(
            "git_merge",
            "Merges a branch into the checked-out branch as a merge commit; a conflict fails and leaves the repository mid-merge for git_abort_merge.",
            within(
                &[("branch", string("The branch to merge in."))],
                &["branch"],
            ),
            |arguments| {
                let branch = text(arguments, "branch")?.to_owned();
                git::merge(Merge::new(repository(arguments)?, branch)?)?;
                Ok(done())
            },
        ),
        tool(
            "git_abort_merge",
            "Puts the repository back as it was before a merge that conflicted.",
            within(&[], &[]),
            |arguments| {
                git::abort_merge(AbortMerge::new(repository(arguments)?))?;
                Ok(done())
            },
        ),
        tool(
            "git_reset",
            "Moves the checked-out branch to a commit and makes the index and working tree match, discarding everything uncommitted.",
            within(
                &[("to", string("The full hash of the commit to move to."))],
                &["to"],
            ),
            |arguments| {
                let to = Hash::new(text(arguments, "to")?)?;
                git::reset(Reset::new(repository(arguments)?, to))?;
                Ok(done())
            },
        ),
        tool(
            "git_push",
            "Pushes a branch to a remote under the same name, using the user's own git credentials.",
            within(
                &[
                    ("remote", string(REMOTE)),
                    ("branch", string("The branch to push.")),
                ],
                &["remote", "branch"],
            ),
            |arguments| {
                let branch = text(arguments, "branch")?.to_owned();
                git::push(Push::new(
                    repository(arguments)?,
                    remote(arguments)?,
                    branch,
                )?)?;
                Ok(done())
            },
        ),
        tool(
            "git_list_remotes",
            "Every remote configured in the repository, with where it fetches from and pushes to.",
            within(&[], &[]),
            |arguments| {
                let remotes = git::list_remotes(ListRemotes::new(repository(arguments)?))?;
                Ok(json!({
                    "remotes": remotes.iter().map(|remote| json!({
                        "name": remote.name,
                        "fetch": remote.fetch.to_string(),
                        "push": remote.push.to_string(),
                    })).collect::<Vec<_>>(),
                }))
            },
        ),
        tool(
            "git_delete_branch",
            "Deletes a branch, merged or not.",
            within(&[("name", string("The branch to delete."))], &["name"]),
            |arguments| {
                let name = text(arguments, "name")?.to_owned();
                git::delete_branch(DeleteBranch::new(repository(arguments)?, name)?)?;
                Ok(done())
            },
        ),
        tool(
            "git_remove_worktree",
            "Removes a worktree of the repository; with force true, even one holding uncommitted work.",
            within(
                &[
                    (
                        "path",
                        string("The absolute path of the worktree to remove."),
                    ),
                    (
                        "force",
                        boolean(
                            "Whether to remove a worktree that holds uncommitted work; false when absent.",
                        ),
                    ),
                ],
                &["path"],
            ),
            |arguments| {
                let path = path(arguments, "path")?;
                let force = arg::flag(arguments, "force").map_err(anyhow::Error::msg)?;
                git::remove_worktree(RemoveWorktree::new(repository(arguments)?, path, force)?)?;
                Ok(done())
            },
        ),
    ]
}

/// What every argument named `repository` must say.
const REPOSITORY: &str =
    "The absolute path of the repository: the root of a working tree, or a bare repository.";

/// What every argument named `remote` must say.
const REMOTE: &str = "The remote: a configured name such as origin, or a URL.";

/// A tool whose handler speaks anyhow: the error, with every context it
/// gathered, is the words the model reads.
fn tool(
    name: &str,
    description: &str,
    parameters: Value,
    handler: fn(&Value) -> Result<Value>,
) -> Tool {
    Tool::new(
        name,
        description,
        parameters,
        Box::new(move |arguments| handler(arguments).map_err(|error| format!("{error:#}"))),
    )
}

/// An [`object`] for a command that runs in a repository: `repository`
/// first, and required.
fn within(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let mut all = vec![("repository", string(REPOSITORY))];
    all.extend(properties.iter().cloned());
    let mut all_required = vec!["repository"];
    all_required.extend_from_slice(required);
    object(&all, &all_required)
}

/// A required, non-empty string.
fn text<'a>(arguments: &'a Value, name: &str) -> Result<&'a str> {
    arg::non_empty(arguments, name).map_err(anyhow::Error::msg)
}

/// A string that may be absent.
fn optional<'a>(arguments: &'a Value, name: &str) -> Result<Option<&'a str>> {
    arg::optional(arguments, name).map_err(anyhow::Error::msg)
}

/// [`optional`], owned.
fn owned(arguments: &Value, name: &str) -> Result<Option<String>> {
    Ok(optional(arguments, name)?.map(str::to_owned))
}

/// A required path, as given: the command decides whether it must be
/// absolute.
fn path(arguments: &Value, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(text(arguments, name)?))
}

/// The `repository` argument, opened.
fn repository(arguments: &Value) -> Result<Repository> {
    Repository::open(path(arguments, "repository")?)
}

/// The `remote` argument: a URL when it is spelled like one — a colon
/// or a slash in it — and a configured name otherwise.
fn remote(arguments: &Value) -> Result<Remote> {
    let remote = text(arguments, "remote")?;
    if remote.contains([':', '/']) {
        Ok(Remote::from(remote.parse::<Url>()?))
    } else {
        Remote::named(remote.to_owned())
    }
}

/// The `paths` argument: an array of strings.
fn paths(arguments: &Value) -> Result<Vec<PathBuf>> {
    arguments["paths"]
        .as_array()
        .ok_or_else(|| anyhow!("the paths argument must be an array of strings"))?
        .iter()
        .map(|path| {
            path.as_str()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow!("every path must be a string"))
        })
        .collect()
}

/// The `count` argument: a whole number that may be absent.
fn count(arguments: &Value) -> Result<Option<usize>> {
    match &arguments["count"] {
        Value::Null => Ok(None),
        value => {
            let count = value
                .as_u64()
                .ok_or_else(|| anyhow!("the count argument must be a whole number"))?;
            Ok(Some(usize::try_from(count)?))
        }
    }
}

/// The answer of a command that reads nothing back: it did what was
/// asked.
fn done() -> Value {
    json!({ "ok": true })
}

/// The answer of a command that made a repository: where it is.
fn made(repository: &Repository) -> Value {
    json!({ "repository": repository.path().to_string_lossy() })
}

/// The answer of a command that reads text back.
fn said(output: String) -> Value {
    json!({ "output": output })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::temp::Scratch;
    use crate::tools::Registry;

    fn registry() -> Registry {
        let mut registry = Registry::default();
        registry.extend(all());
        registry
    }

    fn call(registry: &Registry, name: &str, arguments: Value) -> Result<Value, String> {
        registry.dispatch(name, &arguments.to_string())
    }

    fn ok(registry: &Registry, name: &str, arguments: Value) -> Value {
        call(registry, name, arguments).unwrap()
    }

    fn refused(registry: &Registry, name: &str, arguments: Value) -> String {
        call(registry, name, arguments).unwrap_err()
    }

    /// A bare origin under `scratch` and a clone of it, `work`, with an
    /// identity set and one commit of `hello.txt` on `main`, pushed —
    /// all through the tools. The two repositories' paths.
    fn seeded(registry: &Registry, scratch: &Scratch) -> (String, String) {
        let origin = ok(
            registry,
            "git_init",
            json!({ "path": scratch.join("origin.git") }),
        );
        let origin = origin["repository"].as_str().unwrap().to_owned();
        let work = ok(
            registry,
            "git_clone",
            json!({ "from": origin, "into": scratch.join("work") }),
        );
        let work = work["repository"].as_str().unwrap().to_owned();
        for (key, value) in [
            ("user.name", "Ada"),
            ("user.email", "ada@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            ok(
                registry,
                "git_set_config",
                json!({ "repository": work, "key": key, "value": value }),
            );
        }
        committed(registry, &work, "hello.txt", "hello\n", "the first commit");
        ok(
            registry,
            "git_push",
            json!({ "repository": work, "remote": "origin", "branch": "main" }),
        );
        (origin, work)
    }

    /// `content` written to `name` in `repository`, staged, and
    /// committed under `subject`, through the tools: the commit's hash.
    fn committed(
        registry: &Registry,
        repository: &str,
        name: &str,
        content: &str,
        subject: &str,
    ) -> String {
        std::fs::write(std::path::Path::new(repository).join(name), content).unwrap();
        ok(
            registry,
            "git_add",
            json!({ "repository": repository, "paths": [name] }),
        );
        ok(
            registry,
            "git_commit",
            json!({ "repository": repository, "message": subject }),
        );
        head(registry, repository)
    }

    fn head(registry: &Registry, repository: &str) -> String {
        ok(
            registry,
            "git_rev_parse",
            json!({ "repository": repository, "revision": "HEAD" }),
        )["hash"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn subjects(registry: &Registry, repository: &str) -> Vec<String> {
        ok(registry, "git_log", json!({ "repository": repository }))["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["subject"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn every_command_has_a_tool_with_a_schema_and_a_sentence() {
        let wire = serde_json::to_value(registry().to_wire()).unwrap();
        let names: Vec<&str> = wire
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "git_init",
                "git_clone",
                "git_set_config",
                "git_branch",
                "git_switch",
                "git_add_worktree",
                "git_list_worktrees",
                "git_add",
                "git_status",
                "git_diff",
                "git_commit",
                "git_rev_parse",
                "git_log",
                "git_show",
                "git_fetch",
                "git_pull",
                "git_merge",
                "git_abort_merge",
                "git_reset",
                "git_push",
                "git_list_remotes",
                "git_delete_branch",
                "git_remove_worktree",
            ]
        );
        for tool in wire.as_array().unwrap() {
            let function = &tool["function"];
            let name = function["name"].as_str().unwrap();
            assert_eq!(function["parameters"]["type"], "object", "{name}");
            assert!(function["parameters"]["properties"].is_object(), "{name}");
            assert!(function["parameters"]["required"].is_array(), "{name}");
            assert!(
                function["description"].as_str().unwrap().ends_with('.'),
                "{name} says what it does"
            );
            if !["git_init", "git_clone"].contains(&name) {
                assert_eq!(
                    function["parameters"]["required"][0], "repository",
                    "{name} runs in a repository"
                );
            }
        }
    }

    /// A repository made, cloned, configured, written to, staged,
    /// committed, and read back — status, log, show, diff, rev-parse —
    /// through nothing but tool calls.
    #[test]
    fn a_repository_is_made_worked_in_and_read_back_through_the_tools() {
        let registry = registry();
        let scratch = Scratch::new("tools-read-back");
        let (_, work) = seeded(&registry, &scratch);

        assert_eq!(subjects(&registry, &work), ["the first commit"]);
        let first = head(&registry, &work);
        assert_eq!(first.len(), 40);
        let shown = ok(
            &registry,
            "git_show",
            json!({ "repository": work, "revision": "HEAD" }),
        );
        assert!(shown["output"].as_str().unwrap().contains("+hello"));

        std::fs::write(
            std::path::Path::new(&work).join("hello.txt"),
            "hello again\n",
        )
        .unwrap();
        let patch = ok(&registry, "git_diff", json!({ "repository": work }));
        assert!(patch["output"].as_str().unwrap().contains("+hello again"));
        ok(
            &registry,
            "git_add",
            json!({ "repository": work, "paths": ["hello.txt"] }),
        );
        let status = ok(&registry, "git_status", json!({ "repository": work }));
        assert_eq!(
            status["changes"],
            json!([{ "staged": "M", "unstaged": " ", "path": "hello.txt", "from": null }])
        );
        ok(
            &registry,
            "git_commit",
            json!({ "repository": work, "message": "the second commit" }),
        );

        let log = ok(
            &registry,
            "git_log",
            json!({ "repository": work, "count": 1 }),
        );
        assert_eq!(log["entries"].as_array().unwrap().len(), 1);
        assert_eq!(log["entries"][0]["subject"], "the second commit");
        assert_eq!(log["entries"][0]["author"], "Ada");
        assert!(log["entries"][0]["date"].as_str().unwrap().contains('T'));
        let range = ok(
            &registry,
            "git_diff",
            json!({ "repository": work, "revisions": format!("{first}..HEAD"), "path": "hello.txt" }),
        );
        assert!(range["output"].as_str().unwrap().contains("-hello\n"));
    }

    /// Branches made, switched to, merged, reset away, and deleted; a
    /// worktree added, listed, and removed.
    #[test]
    fn branches_and_worktrees_come_and_go_through_the_tools() {
        let registry = registry();
        let scratch = Scratch::new("tools-branches");
        let (_, work) = seeded(&registry, &scratch);
        let first = head(&registry, &work);

        ok(
            &registry,
            "git_branch",
            json!({ "repository": work, "name": "side", "at": first }),
        );
        ok(
            &registry,
            "git_switch",
            json!({ "repository": work, "branch": "side" }),
        );
        committed(&registry, &work, "side.txt", "side\n", "on the side");
        ok(
            &registry,
            "git_switch",
            json!({ "repository": work, "branch": "main" }),
        );
        ok(
            &registry,
            "git_merge",
            json!({ "repository": work, "branch": "side" }),
        );
        let mut merged = subjects(&registry, &work);
        assert_eq!(merged.remove(0), "Merge branch 'side'");
        merged.sort();
        assert_eq!(
            merged,
            ["on the side", "the first commit"],
            "the parents, in either order"
        );
        let nothing_to_abort = refused(&registry, "git_abort_merge", json!({ "repository": work }));
        assert!(
            nothing_to_abort.contains("no merge to abort"),
            "{nothing_to_abort}"
        );

        ok(
            &registry,
            "git_reset",
            json!({ "repository": work, "to": first }),
        );
        assert_eq!(subjects(&registry, &work), ["the first commit"]);
        assert!(!std::path::Path::new(&work).join("side.txt").exists());

        let linked = scratch.join("linked");
        let added = ok(
            &registry,
            "git_add_worktree",
            json!({ "repository": work, "path": linked, "branch": "side" }),
        );
        assert_eq!(added["repository"], linked);
        let listed = ok(
            &registry,
            "git_list_worktrees",
            json!({ "repository": work }),
        );
        let worktrees = listed["worktrees"].as_array().unwrap();
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[1]["branch"], "side");
        assert_eq!(worktrees[1]["bare"], false);
        assert_eq!(head(&registry, &linked), worktrees[1]["head"]);
        ok(
            &registry,
            "git_remove_worktree",
            json!({ "repository": work, "path": linked }),
        );
        assert_eq!(
            ok(
                &registry,
                "git_list_worktrees",
                json!({ "repository": work })
            )["worktrees"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        ok(
            &registry,
            "git_delete_branch",
            json!({ "repository": work, "name": "side" }),
        );
        let gone = refused(
            &registry,
            "git_switch",
            json!({ "repository": work, "branch": "side" }),
        );
        assert!(gone.contains("side"), "{gone}");
    }

    /// Two clones of one origin, one pushing and the other fetching and
    /// pulling, and the remotes read back.
    #[test]
    fn remotes_are_pushed_to_fetched_from_pulled_from_and_listed_through_the_tools() {
        let registry = registry();
        let scratch = Scratch::new("tools-remotes");
        let (origin, work) = seeded(&registry, &scratch);
        let other = ok(
            &registry,
            "git_clone",
            json!({ "from": origin, "into": scratch.join("other"), "branch": "main" }),
        )["repository"]
            .as_str()
            .unwrap()
            .to_owned();

        committed(&registry, &work, "more.txt", "more\n", "the second commit");
        ok(
            &registry,
            "git_push",
            json!({ "repository": work, "remote": "origin", "branch": "main" }),
        );
        ok(
            &registry,
            "git_fetch",
            json!({ "repository": other, "remote": origin }),
        );
        assert_eq!(subjects(&registry, &other), ["the first commit"]);
        ok(&registry, "git_pull", json!({ "repository": other }));
        assert_eq!(
            subjects(&registry, &other),
            ["the second commit", "the first commit"]
        );

        let remotes = ok(
            &registry,
            "git_list_remotes",
            json!({ "repository": other }),
        );
        assert_eq!(
            remotes["remotes"],
            json!([{ "name": "origin", "fetch": origin, "push": origin }])
        );
    }

    /// What is refused is refused in words: the command's checks, the
    /// door into a repository, and git's own.
    #[test]
    fn refusals_come_back_as_words_for_the_model() {
        let registry = registry();
        let scratch = Scratch::new("tools-refusals");
        let (_, work) = seeded(&registry, &scratch);

        let relative = refused(&registry, "git_status", json!({ "repository": "work" }));
        assert_eq!(relative, "the repository must be an absolute path: work");
        std::fs::create_dir(std::path::Path::new(&work).join("src")).unwrap();
        let below = refused(
            &registry,
            "git_status",
            json!({ "repository": format!("{work}/src") }),
        );
        assert!(
            below.ends_with("is src/ below the root of its repository"),
            "{below}"
        );
        let outside = refused(
            &registry,
            "git_status",
            json!({ "repository": scratch.path() }),
        );
        assert!(outside.contains("not a git repository"), "{outside}");
        let flag = refused(
            &registry,
            "git_switch",
            json!({ "repository": work, "branch": "--orphan" }),
        );
        assert_eq!(flag, "a branch may not begin with a dash: \"--orphan\"");
        let nothing = refused(
            &registry,
            "git_add",
            json!({ "repository": work, "paths": [] }),
        );
        assert_eq!(nothing, "an add needs at least one path");
        let blank = refused(
            &registry,
            "git_commit",
            json!({ "repository": work, "message": " " }),
        );
        assert_eq!(blank, "the message argument must be a non-empty string");
        let not_a_hash = refused(
            &registry,
            "git_reset",
            json!({ "repository": work, "to": "main" }),
        );
        assert_eq!(not_a_hash, "not a commit hash: \"main\"");
        let missing = refused(
            &registry,
            "git_show",
            json!({ "repository": work, "revision": "nonesuch" }),
        );
        assert!(missing.contains("nonesuch"), "{missing}");
        let force = refused(
            &registry,
            "git_remove_worktree",
            json!({ "repository": work, "path": scratch.join("linked"), "force": "yes" }),
        );
        assert_eq!(force, "the force argument must be true or false");
    }
}
