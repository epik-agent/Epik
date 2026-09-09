//! Bare git, as tools the model can call.
//!
//! Thin wrappers over the user's own `git` binary: one verb per tool, an
//! allowlist and nothing else. Every invocation is argv straight into
//! [`Command`](std::process::Command) — never a shell, never a string spliced into
//! a command line — and every tool takes `directory`, an absolute path to
//! the repository, with no resolution, no defaults, and no state held
//! anywhere. Whatever git says — a nonexistent directory, not a
//! repository, a conflict — comes back in git's own words as an ordinary
//! result the model reads.
//!
//! Deliberately absent: reset, clean, rebase, stash (stateful tools for
//! humans in IDEs), every `--force`, and any run-arbitrary-git escape
//! hatch — that would be shell access with extra steps. The allowlist is
//! the interim safety story; a permission layer comes later. Network
//! verbs ride the user's own ambient credentials — credential helpers,
//! SSH config; Epik injects nothing, and commits are made under the
//! user's own git identity — with one exception: `git_init` makes a
//! repository's empty root commit under the Epik persona's own identity,
//! by per-invocation config, so a repository is never commitless.

use serde_json::{Value, json};

use super::{Tool, arg};
use crate::git::{PERSONA_EMAIL, PERSONA_NAME, execute, plumbing};

/// What every argument named `directory` must say.
const DIRECTORY: &str = "The absolute path of the repository's directory.";

/// Whether `directory` is itself a git repository — bare, or a working
/// tree with its `.git` — as opposed to merely lying inside one.
fn is_repository(directory: &str) -> bool {
    let Ok(git_dir) = plumbing(&["-C", directory, "rev-parse", "--absolute-git-dir"]) else {
        return false;
    };
    let git_dir = std::path::Path::new(git_dir.trim());
    let here = std::path::Path::new(directory);
    let same = |a: &std::path::Path, b: &std::path::Path| match (a.canonicalize(), b.canonicalize())
    {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    same(git_dir, here) || same(git_dir, &here.join(".git"))
}

/// A bare repository at `directory` on branch `main`, holding one empty
/// root commit — the guarantee that a repository is never commitless,
/// so every later branch has somewhere to start. Plumbing all the way:
/// `mktree` on nothing, `commit-tree`, `update-ref`, the commit authored
/// as the persona by per-invocation config that touches nothing the
/// repository keeps.
fn init(directory: &str) -> Result<Value, String> {
    if is_repository(directory) {
        return Ok(json!({
            "ok": false,
            "output": format!("{directory} already holds a git repository"),
        }));
    }
    let init = execute(&["init", "--bare", "--initial-branch=main", "--", directory])?;
    if !init["ok"].as_bool().unwrap_or(false) {
        return Ok(init);
    }
    let root = || -> Result<String, String> {
        let tree = plumbing(&["-C", directory, "mktree"])?;
        let commit = plumbing(&[
            "-C",
            directory,
            "-c",
            &format!("user.name={PERSONA_NAME}"),
            "-c",
            &format!("user.email={PERSONA_EMAIL}"),
            "-c",
            "commit.gpgsign=false",
            "commit-tree",
            tree.trim(),
            "-m",
            "Initial commit",
        ])?;
        let commit = commit.trim().to_owned();
        plumbing(&["-C", directory, "update-ref", "refs/heads/main", &commit])?;
        Ok(commit)
    };
    Ok(match root() {
        Ok(commit) => json!({
            "ok": true,
            "output": format!(
                "Initialized empty bare repository in {directory}: branch main at root commit {}",
                &commit[..commit.len().min(7)]
            ),
        }),
        Err(words) => json!({ "ok": false, "output": words }),
    })
}

/// [`execute`], inside the repository at `directory` via `git -C`.
fn git(directory: &str, args: &[&str]) -> Result<Value, String> {
    let directory = absolute(directory)?;
    let mut argv = vec!["-C", directory];
    argv.extend_from_slice(args);
    execute(&argv)
}

/// The `directory` argument, which must be an absolute path — there is no
/// working directory to resolve against, and no default to fall back on.
fn absolute(directory: &str) -> Result<&str, String> {
    if std::path::Path::new(directory).is_absolute() {
        Ok(directory)
    } else {
        Err(format!(
            "directory must be an absolute path, not {directory:?}"
        ))
    }
}

/// A user-supplied positional value — a branch, a ref, a path, a URL.
/// One that starts with `-` would reach git as a flag, which is how an
/// allowlist gets talked around; it is refused in words instead.
fn positional<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
    let value = arg::string(arguments, name)?;
    if value.starts_with('-') {
        return Err(format!("the {name} argument may not start with a dash"));
    }
    Ok(value)
}

fn directory(arguments: &Value) -> Result<&str, String> {
    arg::string(arguments, "directory")
}

fn schema(extra: &[(&str, Value)], required_extra: &[&str]) -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "directory".to_owned(),
        json!({ "type": "string", "description": DIRECTORY }),
    );
    for (name, schema) in extra {
        properties.insert((*name).to_owned(), schema.clone());
    }
    let mut required = vec!["directory"];
    required.extend_from_slice(required_extra);
    json!({ "type": "object", "properties": properties, "required": required })
}

/// The allowlist: every git verb the model may speak, one tool each.
#[must_use]
pub fn all() -> Vec<Tool> {
    vec![
        Tool::new(
            "git_status",
            "The working-tree status of a git repository, in git's own words.",
            schema(&[], &[]),
            Box::new(|arguments| git(directory(arguments)?, &["status"])),
        ),
        Tool::new(
            "git_log",
            "The most recent commits — hash, author, date, and subject, one per line — newest first, of HEAD or of a named ref such as a branch.",
            schema(
                &[
                    (
                        "count",
                        json!({
                            "type": "integer",
                            "description": "How many commits to show; 20 when omitted.",
                        }),
                    ),
                    (
                        "ref",
                        json!({
                            "type": "string",
                            "description": "The branch, tag, or commit whose history to show; HEAD when omitted.",
                        }),
                    ),
                ],
                &[],
            ),
            Box::new(|arguments| {
                let count = arguments["count"].as_u64().unwrap_or(20).to_string();
                let mut args = vec![
                    "log",
                    "-n",
                    &count,
                    "--date=short",
                    "--pretty=format:%h\t%an\t%ad\t%s",
                ];
                if arguments["ref"].is_string() {
                    args.push(positional(arguments, "ref")?);
                }
                git(directory(arguments)?, &args)
            }),
        ),
        Tool::new(
            "git_diff",
            "The diff of the working tree, or of a ref or ref-range like main..feature, optionally narrowed to one path.",
            schema(
                &[
                    (
                        "range",
                        json!({
                            "type": "string",
                            "description": "A ref or ref-range to diff; the working tree against the index when omitted.",
                        }),
                    ),
                    (
                        "path",
                        json!({
                            "type": "string",
                            "description": "A repository-relative path to narrow the diff to.",
                        }),
                    ),
                ],
                &[],
            ),
            Box::new(|arguments| {
                let mut args = vec!["diff"];
                if arguments["range"].is_string() {
                    args.push(positional(arguments, "range")?);
                }
                if arguments["path"].is_string() {
                    args.push("--");
                    args.push(positional(arguments, "path")?);
                }
                git(directory(arguments)?, &args)
            }),
        ),
        Tool::new(
            "git_show",
            "One commit — message and diff — by ref: a hash, branch, or tag.",
            schema(
                &[(
                    "ref",
                    json!({ "type": "string", "description": "The commit to show." }),
                )],
                &["ref"],
            ),
            Box::new(|arguments| {
                git(
                    directory(arguments)?,
                    &["show", positional(arguments, "ref")?],
                )
            }),
        ),
        Tool::new(
            "git_branch_list",
            "Every local branch, the current one starred.",
            schema(&[], &[]),
            Box::new(|arguments| git(directory(arguments)?, &["branch", "--list"])),
        ),
        Tool::new(
            "git_current_branch",
            "The name of the branch currently checked out.",
            schema(&[], &[]),
            Box::new(|arguments| {
                git(
                    directory(arguments)?,
                    &["rev-parse", "--abbrev-ref", "HEAD"],
                )
            }),
        ),
        Tool::new(
            "git_add",
            "Stages files for the next commit, by repository-relative path.",
            schema(
                &[(
                    "paths",
                    json!({
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Repository-relative paths of the files to stage.",
                    }),
                )],
                &["paths"],
            ),
            Box::new(|arguments| {
                let paths = arguments["paths"]
                    .as_array()
                    .ok_or("the paths argument must be an array of strings")?;
                let mut args = vec!["add", "--"];
                for path in paths {
                    args.push(
                        path.as_str()
                            .ok_or("the paths argument must be an array of strings")?,
                    );
                }
                git(directory(arguments)?, &args)
            }),
        ),
        Tool::new(
            "git_commit",
            "Commits whatever is staged, with a message, under the user's own git identity.",
            schema(
                &[(
                    "message",
                    json!({ "type": "string", "description": "The commit message." }),
                )],
                &["message"],
            ),
            Box::new(|arguments| {
                git(
                    directory(arguments)?,
                    &["commit", "-m", arg::string(arguments, "message")?],
                )
            }),
        ),
        Tool::new(
            "git_checkout",
            "Checks out a branch; with create true, makes the branch first.",
            schema(
                &[
                    (
                        "branch",
                        json!({ "type": "string", "description": "The branch name." }),
                    ),
                    (
                        "create",
                        json!({
                            "type": "boolean",
                            "description": "Create the branch before checking it out; false when omitted.",
                        }),
                    ),
                ],
                &["branch"],
            ),
            Box::new(|arguments| {
                let branch = positional(arguments, "branch")?;
                let args: &[&str] = if arguments["create"].as_bool().unwrap_or(false) {
                    &["checkout", "-b", branch]
                } else {
                    &["checkout", branch]
                };
                git(directory(arguments)?, args)
            }),
        ),
        Tool::new(
            "git_fetch",
            "Fetches from the remote, using the user's own git credentials.",
            schema(&[], &[]),
            Box::new(|arguments| git(directory(arguments)?, &["fetch"])),
        ),
        Tool::new(
            "git_pull",
            "Pulls the current branch from its remote, using the user's own git credentials.",
            schema(&[], &[]),
            Box::new(|arguments| git(directory(arguments)?, &["pull"])),
        ),
        Tool::new(
            "git_push",
            "Pushes the current branch, using the user's own git credentials; set_upstream names a branch to push to origin with upstream tracking.",
            schema(
                &[(
                    "set_upstream",
                    json!({
                        "type": "string",
                        "description": "A branch to push to origin with --set-upstream; a plain push when omitted.",
                    }),
                )],
                &[],
            ),
            Box::new(|arguments| {
                let args: Vec<&str> = if arguments["set_upstream"].is_string() {
                    vec![
                        "push",
                        "--set-upstream",
                        "origin",
                        positional(arguments, "set_upstream")?,
                    ]
                } else {
                    vec!["push"]
                };
                git(directory(arguments)?, &args)
            }),
        ),
        Tool::new(
            "git_clone",
            "Clones a repository from a URL into directory, which must be an absolute path that does not exist yet; uses the user's own git credentials.",
            schema(
                &[(
                    "url",
                    json!({ "type": "string", "description": "The repository URL or local path to clone from." }),
                )],
                &["url"],
            ),
            Box::new(|arguments| {
                let destination = absolute(directory(arguments)?)?;
                execute(&["clone", "--", positional(arguments, "url")?, destination])
            }),
        ),
        Tool::new(
            "git_remote_list",
            "Every configured remote and its URLs.",
            schema(&[], &[]),
            Box::new(|arguments| git(directory(arguments)?, &["remote", "-v"])),
        ),
        Tool::new(
            "git_init",
            "Creates a new bare git repository at directory — an absolute path that must not already hold a repository — on branch main with an empty root commit, so it is ready to be built in or cloned. Use this to give a project a home when the user has named a location that has no repository yet.",
            schema(&[], &[]),
            Box::new(|arguments| init(absolute(directory(arguments)?)?)),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // The git tools run the real CLI against scratch repositories in
    // tempdirs; git2 is the independent implementation that verifies what
    // the CLI did — the project's established cross-check pattern.

    use crate::testing::Scratch;

    /// The allowlist as a registry, which is also how the backend holds it.
    fn registry() -> crate::tools::Registry {
        let mut registry = crate::tools::Registry::default();
        registry.extend(all());
        registry
    }

    /// Runs the named tool through the registry and unwraps the
    /// `{ ok, output }` shape.
    fn call(name: &str, arguments: Value) -> (bool, String) {
        let value = registry()
            .dispatch(name, &arguments.to_string())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        (
            value["ok"].as_bool().unwrap(),
            value["output"].as_str().unwrap().to_owned(),
        )
    }

    fn ok(name: &str, arguments: Value) -> String {
        let (ok, output) = call(name, arguments);
        assert!(ok, "{name}: {output}");
        output
    }

    /// A repository with one committed file, made entirely through the
    /// tools under test — plus the identity config the tools deliberately
    /// do not manage.
    fn seeded(name: &str) -> Scratch {
        let scratch = Scratch::new(name);
        let dir = scratch.path();
        assert!(
            execute(&["-C", dir, "init", "--initial-branch=main"]).unwrap()["ok"]
                .as_bool()
                .unwrap()
        );
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            assert!(
                execute(&["-C", dir, "config", key, value]).unwrap()["ok"]
                    .as_bool()
                    .unwrap()
            );
        }
        std::fs::write(scratch.0.join("hello.txt"), "hello\n").unwrap();
        ok(
            "git_add",
            json!({ "directory": dir, "paths": ["hello.txt"] }),
        );
        ok(
            "git_commit",
            json!({ "directory": dir, "message": "the first commit" }),
        );
        scratch
    }

    #[test]
    fn status_log_show_and_branches_read_a_seeded_repository() {
        let repo = seeded("read");
        let dir = repo.path();

        let status = ok("git_status", json!({ "directory": dir }));
        assert!(status.contains("working tree clean"), "{status}");

        let log = ok("git_log", json!({ "directory": dir }));
        assert!(log.contains("the first commit"), "{log}");
        assert!(log.contains("Test"), "the author rides along: {log}");

        let show = ok("git_show", json!({ "directory": dir, "ref": "HEAD" }));
        assert!(show.contains("hello"), "{show}");

        let branches = ok("git_branch_list", json!({ "directory": dir }));
        assert!(branches.contains("main"), "{branches}");

        let current = ok("git_current_branch", json!({ "directory": dir }));
        assert_eq!(current.trim(), "main");

        // The independent implementation agrees the commit exists.
        let verified = git2::Repository::open(dir).unwrap();
        let head = verified.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.message().unwrap().trim(), "the first commit");
        assert_eq!(head.author().name(), Some("Test"));
    }

    #[test]
    fn add_commit_and_diff_advance_the_repository() {
        let repo = seeded("write");
        let dir = repo.path();
        std::fs::write(repo.0.join("hello.txt"), "hello again\n").unwrap();

        let diff = ok("git_diff", json!({ "directory": dir }));
        assert!(diff.contains("+hello again"), "{diff}");

        ok(
            "git_add",
            json!({ "directory": dir, "paths": ["hello.txt"] }),
        );
        ok(
            "git_commit",
            json!({ "directory": dir, "message": "the second commit" }),
        );

        let ranged = ok(
            "git_diff",
            json!({ "directory": dir, "range": "HEAD~1..HEAD", "path": "hello.txt" }),
        );
        assert!(ranged.contains("+hello again"), "{ranged}");

        let verified = git2::Repository::open(dir).unwrap();
        let head = verified.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.message().unwrap().trim(), "the second commit");
        assert_eq!(head.parent_count(), 1);
    }

    #[test]
    fn checkout_with_create_makes_and_switches_to_the_branch() {
        let repo = seeded("branch");
        let dir = repo.path();

        ok(
            "git_checkout",
            json!({ "directory": dir, "branch": "feature", "create": true }),
        );
        assert_eq!(
            ok("git_current_branch", json!({ "directory": dir })).trim(),
            "feature"
        );

        ok(
            "git_checkout",
            json!({ "directory": dir, "branch": "main" }),
        );
        assert_eq!(
            ok("git_current_branch", json!({ "directory": dir })).trim(),
            "main"
        );

        let verified = git2::Repository::open(dir).unwrap();
        assert!(
            verified
                .find_branch("feature", git2::BranchType::Local)
                .is_ok()
        );
    }

    #[test]
    fn clone_push_fetch_and_pull_speak_to_a_local_bare_remote() {
        let source = seeded("source");
        let bare = Scratch::new("bare");
        assert!(
            execute(&["init", "--bare", "--initial-branch=main", bare.path()]).unwrap()["ok"]
                .as_bool()
                .unwrap()
        );
        ok("git_remote_list", json!({ "directory": source.path() }));
        assert!(
            execute(&["-C", source.path(), "remote", "add", "origin", bare.path()]).unwrap()["ok"]
                .as_bool()
                .unwrap()
        );
        ok(
            "git_push",
            json!({ "directory": source.path(), "set_upstream": "main" }),
        );

        // Clone from the bare remote into a second working copy.
        let clones = Scratch::new("clones");
        let copy = clones.join("copy");
        ok(
            "git_clone",
            json!({ "directory": copy, "url": bare.path() }),
        );
        let remotes = ok("git_remote_list", json!({ "directory": copy }));
        assert!(remotes.contains("origin"), "{remotes}");

        // Advance the source, push, then fetch and pull from the copy.
        std::fs::write(source.0.join("more.txt"), "more\n").unwrap();
        ok(
            "git_add",
            json!({ "directory": source.path(), "paths": ["more.txt"] }),
        );
        ok(
            "git_commit",
            json!({ "directory": source.path(), "message": "more" }),
        );
        ok("git_push", json!({ "directory": source.path() }));
        ok("git_fetch", json!({ "directory": copy }));
        ok("git_pull", json!({ "directory": copy }));

        let log = ok("git_log", json!({ "directory": copy }));
        assert!(log.contains("more"), "{log}");

        let verified = git2::Repository::open(std::path::Path::new(&copy)).unwrap();
        let head = verified.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.message().unwrap().trim(), "more");
    }

    #[test]
    fn init_makes_a_bare_repository_with_one_empty_root_commit_on_main() {
        let scratch = Scratch::new("init");
        let dir = scratch.join("wumpus.git");

        let output = ok("git_init", json!({ "directory": dir }));
        assert!(output.contains("main"), "{output}");

        // The independent implementation: bare, on main, one commit with
        // no parents, whose tree is empty, authored as the persona.
        let verified = git2::Repository::open(&dir).unwrap();
        assert!(verified.is_bare());
        let head = verified.head().unwrap();
        assert_eq!(head.shorthand(), Some("main"));
        let commit = head.peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 0);
        assert_eq!(commit.tree().unwrap().len(), 0);
        assert_eq!(commit.author().name(), Some(PERSONA_NAME));
        assert_eq!(commit.author().email(), Some(PERSONA_EMAIL));

        // Which is exactly enough for the git tools to read.
        let log = ok("git_log", json!({ "directory": dir }));
        assert!(log.contains("Initial commit"), "{log}");
    }

    /// A bare repository's HEAD is one branch; the others are reached by
    /// name — which is how the persona reads a build branch's history.
    #[test]
    fn log_reads_a_named_branch_of_a_bare_repository() {
        let scratch = Scratch::new("branchlog");
        let dir = scratch.join("r.git");
        ok("git_init", json!({ "directory": dir }));
        assert!(
            execute(&["-C", &dir, "branch", "other", "main"]).unwrap()["ok"]
                .as_bool()
                .unwrap()
        );
        let log = ok(
            "git_log",
            json!({ "directory": dir, "ref": "other", "count": 5 }),
        );
        assert!(log.contains("Initial commit"), "{log}");
        let (ok, output) = call("git_log", json!({ "directory": dir, "ref": "nonesuch" }));
        assert!(!ok, "{output}");
    }

    #[test]
    fn init_refuses_a_directory_that_already_holds_a_repository() {
        let repo = seeded("reinit");
        let (ok, output) = call("git_init", json!({ "directory": repo.path() }));
        assert!(!ok);
        assert!(output.contains("already holds"), "{output}");

        let bare = Scratch::new("rebare");
        let dir = bare.join("r.git");
        self::ok("git_init", json!({ "directory": dir }));
        let (ok, output) = call("git_init", json!({ "directory": dir }));
        assert!(!ok, "a bare repository counts too: {output}");
    }

    #[test]
    fn a_nonexistent_directory_fails_in_gits_own_words() {
        let (ok, output) = call(
            "git_status",
            json!({ "directory": "/nonexistent/epik-test" }),
        );
        assert!(!ok);
        assert!(output.contains("/nonexistent/epik-test"), "{output}");
    }

    #[test]
    fn a_directory_that_is_no_repository_fails_in_gits_own_words() {
        let scratch = Scratch::new("plain");
        let (ok, output) = call("git_status", json!({ "directory": scratch.path() }));
        assert!(!ok);
        assert!(
            output.to_lowercase().contains("not a git repository"),
            "{output}"
        );
    }

    #[test]
    fn a_relative_directory_is_refused_in_words() {
        let error = registry()
            .dispatch("git_status", r#"{"directory":"repo"}"#)
            .unwrap_err();
        assert!(error.contains("absolute path"), "{error}");
    }

    /// A model that passes `--force` as a branch name is refused before
    /// git ever sees it — the allowlist cannot be talked around through
    /// argument values.
    #[test]
    fn a_positional_that_smuggles_a_flag_is_refused() {
        let repo = seeded("flags");
        let error = registry()
            .dispatch(
                "git_checkout",
                &json!({ "directory": repo.path(), "branch": "--force" }).to_string(),
            )
            .unwrap_err();
        assert!(error.contains("dash"), "{error}");
    }

    /// The whole rig at once: the scripted model asks for git_status, the
    /// loop dispatches it against a real scratch repository, and the tool
    /// result reaches the second request as a role:"tool" message.
    #[cfg(feature = "testing")]
    #[test]
    fn a_scripted_git_status_turn_reaches_the_model_as_a_tool_message() {
        use crate::chat::{Client, Role, TranscriptItem};
        use crate::testing::model::{Fragment, Scripted, Turn};

        let repo = seeded("loop");
        let arguments = json!({ "directory": repo.path() }).to_string();
        let model = Scripted::spawn(vec![
            Turn::ToolCalls(vec![vec![Fragment::open(
                0,
                "call_1",
                "git_status",
                &arguments,
            )]]),
            Turn::text(&["clean"]),
        ]);
        let client = Client::new(model.base_url(), "scripted".to_owned(), None);

        let text = crate::tools::run(
            &client,
            "system",
            &[TranscriptItem::Message {
                role: Role::User,
                text: "status?".to_owned(),
            }],
            &registry(),
            |_| {},
            |_, _| {},
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(text, "clean");
        let second = &model.requests()[1];
        let answer = second["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(answer["role"], "tool");
        assert_eq!(answer["tool_call_id"], "call_1");
        let content: Value = serde_json::from_str(answer["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["ok"], true);
        assert!(
            content["output"]
                .as_str()
                .unwrap()
                .contains("working tree clean"),
            "{content}"
        );
    }
}
