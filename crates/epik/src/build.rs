//! Building from chat: provisioning a private worktree, launching a
//! coding Agent in it, and keeping the run record the persona reports
//! from.
//!
//! An [`Order`] is the build as the persona asked for it — the tool's
//! arguments, exactly. [`provision`] turns it into a [`Workspace`]: a
//! fresh worktree of the repository on a new branch at the base, under
//! the system temp dir, never a checkout of the repository itself, with
//! the Epik persona's identity set per-worktree so every commit the
//! Agent makes is attributed to the persona — the attribution invariant
//! lives here, in provisioning, and per-worktree config never touches
//! the identity the repository's own config keeps. (Enabling
//! `extensions.worktreeConfig` does touch the repository once, and a bare
//! repository's `core.bare` moves into its main-tree `config.worktree`
//! as git requires; that is plumbing, not identity.)
//!
//! [`launch`] runs any [`Agent`] in the workspace through the runner —
//! `ClaudeCode` in production, an inline shell script in tests, the same
//! asymmetry as CLI-in-prod/git2-in-tests — and one drainer thread folds
//! its events into the shared [`Record`]: stdout lines interpreted
//! through [`claude_code::interpret`] into narration, and at the end the
//! commit *observation* — did the branch advance past the base, is the
//! worktree clean. The harness observes; the Agent commits. A clean
//! worktree is removed; a dirty one is left where it is and its path
//! noted. Remediation is deliberately unbuilt.
//!
//! Nothing here emits on the chat channel. The record is the sink; the
//! persona's tools read it.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, PoisonError};

use crate::agent::claude_code::{self, Update};
use crate::agent::{Agent, Event, Exit, Handle};
use crate::git::{PERSONA_EMAIL, PERSONA_NAME, plumbing};

/// A build as ordered: the prompt, the repository — a git URL, of which
/// a local path is one — the branch to build on, and the base to start
/// it from, the repository's default branch when unnamed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Order {
    pub prompt: String,
    pub repository: String,
    pub branch: String,
    pub base: Option<String>,
}

/// A provisioned place to build: the repository, the branch checked out
/// in `directory`, and the commit the branch started from — the mark the
/// commit observation measures against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    pub repository: String,
    pub branch: String,
    pub base_commit: String,
    pub directory: PathBuf,
}

/// The standing preface every build prompt opens with. What the Agent
/// needs to know about where it is and what it must not do; the branch
/// is named in the line [`Workspace::brief`] adds beneath it.
pub const PREAMBLE: &str = "You are working in a git worktree that has been prepared for you, \
on the branch named below, which is already checked out. Implement the instructions that \
follow. Commit your work as you go, with clear commit messages, so that the branch tells the \
story of what you built. Do not push, do not create or switch branches, and do not touch git \
configuration — the identity and branch are set for you.";

impl Workspace {
    /// The prompt the Agent receives: the preamble, the branch, then the
    /// order's own instructions.
    #[must_use]
    pub fn brief(&self, prompt: &str) -> String {
        format!("{PREAMBLE}\n\nBranch: {}\n\n{prompt}", self.branch)
    }
}

/// A user-supplied name that will reach git as a positional argument:
/// one that starts with a dash would be read as a flag.
fn positional<'a>(name: &str, value: &'a str) -> Result<&'a str, String> {
    if value.starts_with('-') {
        return Err(format!("the {name} may not start with a dash: {value:?}"));
    }
    Ok(value)
}

/// Provisions the workspace for `order`: checks the repository, resolves
/// the base, adds the worktree on the new branch, and sets the persona
/// identity per-worktree.
///
/// # Errors
///
/// Words for the model: the repository is not one, the base does not
/// resolve, the branch already exists (git's own refusal from `worktree
/// add -b`), or git failed along the way.
pub fn provision(order: &Order) -> Result<Workspace, String> {
    let repository = order.repository.as_str();
    if !Path::new(repository).is_absolute() {
        return Err(format!(
            "the repository must be an absolute local path for now, not {repository:?}"
        ));
    }
    let git_dir = plumbing(&["-C", repository, "rev-parse", "--absolute-git-dir"])
        .map_err(|words| format!("{repository} is not a git repository: {words}"))?;
    let git_dir = git_dir.trim().to_owned();

    let branch = positional("branch", &order.branch)?;
    plumbing(&["-C", repository, "check-ref-format", "--branch", branch])
        .map_err(|words| format!("{branch:?} is not a valid branch name: {words}"))?;

    let base = match &order.base {
        Some(base) => positional("base", base)?.to_owned(),
        None => plumbing(&["-C", repository, "symbolic-ref", "--short", "HEAD"])
            .map_err(|words| format!("the repository has no default branch: {words}"))?
            .trim()
            .to_owned(),
    };
    let base_commit = plumbing(&[
        "-C",
        repository,
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{base}^{{commit}}"),
    ])
    .map_err(|words| format!("the base {base:?} does not name a commit: {words}"))?
    .trim()
    .to_owned();

    // Per-worktree config needs the extension on; a bare repository's
    // core.bare must then live in the main tree's own worktree config,
    // or every linked worktree would read it and believe itself bare.
    let bare = plumbing(&["-C", repository, "rev-parse", "--is-bare-repository"])?;
    plumbing(&[
        "-C",
        repository,
        "config",
        "extensions.worktreeConfig",
        "true",
    ])?;
    if bare.trim() == "true" {
        let common = format!("{git_dir}/config");
        let common_bare = plumbing(&["config", "--file", &common, "--get", "core.bare"]);
        if common_bare.is_ok_and(|value| value.trim() == "true") {
            plumbing(&[
                "-C",
                repository,
                "config",
                "--worktree",
                "core.bare",
                "true",
            ])?;
            plumbing(&["config", "--file", &common, "--unset", "core.bare"])?;
        }
    }

    let directory = std::env::temp_dir().join(format!(
        "epik-build-{}-{}-{}",
        branch.replace(['/', '\\'], "-"),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    ));
    let directory_str = directory
        .to_str()
        .ok_or("the temp dir path is not valid unicode")?
        .to_owned();
    plumbing(&[
        "-C",
        repository,
        "worktree",
        "add",
        "-b",
        branch,
        &directory_str,
        &base_commit,
    ])?;

    let identity = [("user.name", PERSONA_NAME), ("user.email", PERSONA_EMAIL)];
    for (key, value) in identity {
        plumbing(&["-C", &directory_str, "config", "--worktree", key, value])?;
    }

    Ok(Workspace {
        repository: repository.to_owned(),
        branch: branch.to_owned(),
        base_commit,
        directory,
    })
}

/// Where a run stands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Phase {
    /// The Agent is running.
    Running,
    /// The Agent is gone; this is how it ended.
    Finished(Exit),
}

/// What the branch and the worktree looked like when the Agent was
/// reaped. Observation only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commits {
    /// The branch's tip at reap.
    pub head: String,
    /// Whether the tip moved past the base commit — whether the Agent
    /// committed anything at all.
    pub advanced: bool,
    /// Whether `status --porcelain` was empty: nothing uncommitted,
    /// nothing untracked.
    pub clean: bool,
    /// Where the worktree still is, when it was left in place — dirty,
    /// or clean but unremovable. `None` means it was removed.
    pub kept: Option<PathBuf>,
}

/// The most narration updates a record retains; the oldest go first.
pub const NARRATION_CAP: usize = 1000;
/// The most stderr lines a record retains; the oldest go first.
pub const STDERR_CAP: usize = 50;

/// The run record: everything about one build, behind the shared
/// [`Run`] handle.
#[derive(Clone, Debug)]
pub struct Record {
    pub order: Order,
    pub workspace: Workspace,
    pub phase: Phase,
    /// The Agent's stdout, interpreted: session, tool uses, texts, and
    /// the terminal result — capped at [`NARRATION_CAP`].
    pub narration: Vec<Update>,
    /// The Agent's stderr, verbatim — capped at [`STDERR_CAP`]. Where a
    /// CLI that could not start says why.
    pub stderr: Vec<String>,
    /// The commit observation, taken at reap.
    pub commits: Option<Commits>,
}

/// The shared handle to a [`Record`]: cloned by whoever needs to read
/// the run — the drainer writes it, the persona's tools read it.
pub type Run = Arc<Mutex<Record>>;

impl Record {
    /// A record for a run about to start.
    #[must_use]
    pub fn new(order: Order, workspace: Workspace) -> Run {
        Arc::new(Mutex::new(Self {
            order,
            workspace,
            phase: Phase::Running,
            narration: Vec::new(),
            stderr: Vec::new(),
            commits: None,
        }))
    }

    /// The terminal result, when the Agent said one.
    #[must_use]
    pub fn result(&self) -> Option<&Update> {
        self.narration
            .iter()
            .rev()
            .find(|update| matches!(update, Update::Result { .. }))
    }

    /// Folds one event in: narration, stderr, or the end.
    fn absorb(&mut self, event: Event) {
        match event {
            Event::Stdout { line } => {
                self.narration.extend(claude_code::interpret(&line));
                if self.narration.len() > NARRATION_CAP {
                    let excess = self.narration.len() - NARRATION_CAP;
                    self.narration.drain(..excess);
                }
            }
            Event::Stderr { line } => {
                self.stderr.push(line);
                if self.stderr.len() > STDERR_CAP {
                    self.stderr.remove(0);
                }
            }
            Event::Exited(exit) => self.phase = Phase::Finished(exit),
            Event::Started { .. } | Event::Unknown => {}
        }
    }
}

/// Reads the workspace back at reap: the branch tip, whether it advanced,
/// whether the tree is clean — and removes the worktree if it is.
fn observe(workspace: &Workspace) -> Commits {
    let directory = workspace.directory.to_string_lossy().into_owned();
    let head = plumbing(&["-C", &directory, "rev-parse", "HEAD"])
        .map(|head| head.trim().to_owned())
        .unwrap_or_default();
    let clean = plumbing(&["-C", &directory, "status", "--porcelain"])
        .is_ok_and(|status| status.trim().is_empty());
    let removed = clean
        && plumbing(&[
            "-C",
            &workspace.repository,
            "worktree",
            "remove",
            "--",
            &directory,
        ])
        .is_ok();
    Commits {
        advanced: !head.is_empty() && head != workspace.base_commit,
        head,
        clean,
        kept: (!removed).then(|| workspace.directory.clone()),
    }
}

/// Removes a provisioned worktree that never got its Agent, so a failed
/// launch leaves nothing behind. Best effort.
fn abandon(workspace: &Workspace) {
    let directory = workspace.directory.to_string_lossy().into_owned();
    let _ = plumbing(&[
        "-C",
        &workspace.repository,
        "worktree",
        "remove",
        "--force",
        "--",
        &directory,
    ]);
    let _ = plumbing(&[
        "-C",
        &workspace.repository,
        "branch",
        "-D",
        &workspace.branch,
    ]);
}

/// Launches `agent` under the runner at `runner` for the run `record`
/// describes, and returns at once. One drainer thread folds the events
/// into the record; when the Agent exits it takes the commit observation
/// and removes the worktree if clean. The [`Handle`] is the run's life:
/// dropping it kills the Agent, so the caller keeps it.
///
/// # Errors
///
/// The spawn itself failing — the runner binary missing, chiefly. The
/// provisioned worktree is removed on the way out.
pub fn launch(agent: &impl Agent, runner: &Path, record: Run) -> io::Result<Handle> {
    let (events_in, events) = channel();
    let handle = match crate::agent::run(agent, runner, events_in) {
        Ok(handle) => handle,
        Err(error) => {
            let workspace = record
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .workspace
                .clone();
            abandon(&workspace);
            return Err(error);
        }
    };
    std::thread::spawn(move || {
        for event in events {
            let exited = matches!(event, Event::Exited(_));
            let workspace = {
                let mut record = record.lock().unwrap_or_else(PoisonError::into_inner);
                record.absorb(event);
                exited.then(|| record.workspace.clone())
            };
            // Observed outside the lock: git takes its time.
            if let Some(workspace) = workspace {
                let commits = observe(&workspace);
                record
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .commits = Some(commits);
            }
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Provisioning against a git_init-made bare repository, verified with
    // git2 — the independent implementation. Launching needs the runner
    // binary, which only the epik-agent package's tests can locate; those
    // live in crates/epik-agent/tests/build.rs.

    /// A scratch directory that cleans up after itself.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "epik-build-test-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A bare repository with its empty root commit, made by git_init.
    fn bare(scratch: &Scratch) -> String {
        let directory = scratch.join("repo.git");
        let mut registry = crate::tools::Registry::default();
        registry.extend(crate::git::all());
        let made = registry
            .dispatch(
                "git_init",
                &serde_json::json!({ "directory": directory }).to_string(),
            )
            .unwrap();
        assert_eq!(made["ok"], true, "{made}");
        directory
    }

    fn order(repository: &str, branch: &str) -> Order {
        Order {
            prompt: "build it".to_owned(),
            repository: repository.to_owned(),
            branch: branch.to_owned(),
            base: None,
        }
    }

    /// Removes a provisioned worktree so the scratch dir's drop is enough.
    fn tidy(workspace: &Workspace) {
        abandon(workspace);
    }

    #[test]
    fn provisioning_makes_a_worktree_on_the_new_branch_at_the_default_base() {
        let scratch = Scratch::new("provision");
        let repository = bare(&scratch);

        let workspace = provision(&order(&repository, "feature/wumpus")).unwrap();

        assert_eq!(workspace.repository, repository);
        assert_eq!(workspace.branch, "feature/wumpus");
        assert!(workspace.directory.is_dir(), "{workspace:?}");

        let verified = git2::Repository::open(&repository).unwrap();
        let main = verified.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(workspace.base_commit, main.id().to_string());
        let branch = verified
            .find_branch("feature/wumpus", git2::BranchType::Local)
            .unwrap();
        assert_eq!(
            branch.get().peel_to_commit().unwrap().id(),
            main.id(),
            "the branch starts at the base"
        );

        let checked_out = git2::Repository::open(&workspace.directory).unwrap();
        assert!(!checked_out.is_bare(), "the worktree is a working tree");
        assert_eq!(
            checked_out.head().unwrap().shorthand(),
            Some("feature/wumpus")
        );
        let config = checked_out.config().unwrap();
        assert_eq!(config.get_string("user.name").unwrap(), PERSONA_NAME);
        assert_eq!(config.get_string("user.email").unwrap(), PERSONA_EMAIL);

        // The repository's own config carries no persona identity.
        let repository_config = std::fs::read_to_string(format!("{repository}/config")).unwrap();
        assert!(
            !repository_config.contains(PERSONA_EMAIL),
            "{repository_config}"
        );

        tidy(&workspace);
    }

    #[test]
    fn a_branch_that_already_exists_is_refused_in_gits_words() {
        let scratch = Scratch::new("collision");
        let repository = bare(&scratch);
        let first = provision(&order(&repository, "taken")).unwrap();

        let error = provision(&order(&repository, "taken")).unwrap_err();
        assert!(error.contains("taken"), "{error}");
        assert!(error.contains("already exists"), "{error}");

        tidy(&first);
    }

    #[test]
    fn a_named_base_is_where_the_branch_starts() {
        let scratch = Scratch::new("base");
        let repository = bare(&scratch);
        let first = provision(&order(&repository, "first")).unwrap();
        let mut second = order(&repository, "second");
        second.base = Some("first".to_owned());

        let workspace = provision(&second).unwrap();
        assert_eq!(workspace.base_commit, first.base_commit);

        tidy(&workspace);
        tidy(&first);
    }

    #[test]
    fn what_is_not_a_repository_is_refused_in_words() {
        let scratch = Scratch::new("plain");
        let error = provision(&order(&scratch.join("nowhere"), "b")).unwrap_err();
        assert!(error.contains("not a git repository"), "{error}");

        let error = provision(&order("relative/path", "b")).unwrap_err();
        assert!(error.contains("absolute"), "{error}");
    }

    #[test]
    fn a_bad_branch_or_base_is_refused_before_git_sees_a_flag() {
        let scratch = Scratch::new("names");
        let repository = bare(&scratch);

        let error = provision(&order(&repository, "--force")).unwrap_err();
        assert!(error.contains("dash"), "{error}");
        let error = provision(&order(&repository, "bad..name")).unwrap_err();
        assert!(error.contains("not a valid branch name"), "{error}");

        let mut missing = order(&repository, "fine");
        missing.base = Some("nonesuch".to_owned());
        let error = provision(&missing).unwrap_err();
        assert!(error.contains("nonesuch"), "{error}");
    }

    #[test]
    fn the_brief_leads_with_the_preamble_and_names_the_branch() {
        let workspace = Workspace {
            repository: "/r".to_owned(),
            branch: "wumpus".to_owned(),
            base_commit: "abc".to_owned(),
            directory: PathBuf::from("/w"),
        };
        let brief = workspace.brief("Write hunt the wumpus.");
        assert!(brief.starts_with(PREAMBLE));
        assert!(brief.contains("Branch: wumpus"));
        assert!(brief.ends_with("Write hunt the wumpus."));
    }

    #[test]
    fn a_record_folds_narration_and_the_end_and_caps_what_it_keeps() {
        let run = Record::new(
            order("/r", "b"),
            Workspace {
                repository: "/r".to_owned(),
                branch: "b".to_owned(),
                base_commit: "abc".to_owned(),
                directory: PathBuf::from("/w"),
            },
        );
        let mut record = run.lock().unwrap();
        record.absorb(Event::Started { pid: 1 });
        record.absorb(Event::Stdout {
            line: r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#
                .to_owned(),
        });
        record.absorb(Event::Stdout {
            line: "not json".to_owned(),
        });
        record.absorb(Event::Stderr {
            line: "grumble".to_owned(),
        });
        assert_eq!(
            record.narration,
            [Update::Text {
                text: "hi".to_owned()
            }]
        );
        assert_eq!(record.stderr, ["grumble"]);
        assert_eq!(record.phase, Phase::Running);
        assert!(record.result().is_none());

        for _ in 0..(NARRATION_CAP + 5) {
            record.absorb(Event::Stdout {
                line: r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write"}]}}"#
                    .to_owned(),
            });
        }
        assert_eq!(record.narration.len(), NARRATION_CAP);
        assert!(
            matches!(record.narration[0], Update::ToolUse { .. }),
            "the oldest went first"
        );

        record.absorb(Event::Stdout {
            line: r#"{"type":"result","is_error":false,"result":"done","total_cost_usd":0.5}"#
                .to_owned(),
        });
        record.absorb(Event::Exited(Exit {
            code: Some(0),
            signal: None,
        }));
        assert!(matches!(
            record.result(),
            Some(Update::Result { ok: true, .. })
        ));
        assert_eq!(
            record.phase,
            Phase::Finished(Exit {
                code: Some(0),
                signal: None
            })
        );
    }
}
