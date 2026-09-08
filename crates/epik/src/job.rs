//! A job: one coding Agent, launched in a private worktree, and the
//! record of how it went. A feature build is many jobs, one per issue;
//! a build ordered on its own is exactly one. The word is the unit's
//! name throughout — a job is ordered, provisioned, launched, and read.
//!
//! An [`Order`] is the job as it was asked for, exactly, wherever the
//! asking came from. [`provision`] turns it into a [`Workspace`]: a
//! fresh worktree of the repository on a new branch at the base, under
//! the system temp dir, never a checkout of the repository itself, with
//! the Epik persona's identity set per-worktree so every commit the
//! Agent makes is attributed to the persona — the attribution invariant
//! lives here, in provisioning, and per-worktree config never touches
//! the identity the repository's own config keeps. (Enabling
//! `extensions.worktreeConfig` does touch the repository once, and a bare
//! repository's `core.bare` moves into its main-tree `config.worktree`
//! as git requires; that is plumbing, not identity.) [`adopt`] is its
//! sibling for a branch that already stands: the feature workspace a
//! feature build keeps for itself, distinct from the jobs' own.
//!
//! [`launch`] takes a started [`Agent`] in the workspace — `ClaudeCode`
//! in production, an inline shell script in tests, the same asymmetry as
//! CLI-in-prod/git2-in-tests — and one drainer thread folds its events
//! into the shared [`Record`]: stdout lines interpreted through
//! [`claude_code::interpret`] into narration, then the exit, and at the
//! end the commit *observation* — did the branch advance past the base,
//! is the worktree clean. Epik observes; the Agent commits. A clean
//! worktree is removed; a dirty one is left where it is and its path
//! noted. Remediation is deliberately unbuilt.
//!
//! Nothing here reports anywhere. The record is the sink; whoever
//! ordered the job reads it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::agent::claude_code::{self, Update};
use crate::agent::{Agent, Event, Exit};
use crate::git::{PERSONA_EMAIL, PERSONA_NAME, base_commit, plumbing};
use crate::temp;

/// A job as ordered: the prompt, the repository — a git URL, of which
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
const PREAMBLE: &str = "You are working in a git worktree that has been prepared for you, \
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
pub(crate) fn positional<'a>(name: &str, value: &'a str) -> Result<&'a str, String> {
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
    let git_dir = locate(repository)?;
    let branch = valid_branch(repository, &order.branch)?;

    let base = match &order.base {
        Some(base) => positional("base", base)?.to_owned(),
        None => plumbing(&["-C", repository, "symbolic-ref", "--short", "HEAD"])
            .map_err(|words| format!("the repository has no default branch: {words}"))?
            .trim()
            .to_owned(),
    };
    let base_commit = base_commit(repository, &base)?;

    enable_worktree_config(repository, &git_dir)?;
    let directory = furnish(
        repository,
        Checkout::Create {
            branch,
            at: &base_commit,
        },
    )?;
    Ok(Workspace {
        repository: repository.to_owned(),
        branch: branch.to_owned(),
        base_commit,
        directory,
    })
}

/// [`provision`]'s sibling for a branch that already exists: a fresh
/// worktree of `repository` with `branch` checked out as it stands —
/// the feature workspace a build keeps for itself, distinct from the
/// per-issue workspaces — under the same persona identity.
/// `base_commit` is the tip the branch stood on when adopted.
///
/// # Errors
///
/// Words for the model: the repository is not one, the branch does not
/// exist (this function creates nothing), the branch is already checked
/// out somewhere (git's own refusal), or git failed along the way.
pub(crate) fn adopt(repository: &str, branch: &str) -> Result<Workspace, String> {
    let git_dir = locate(repository)?;
    let branch = valid_branch(repository, branch)?;
    let tip = plumbing(&[
        "-C",
        repository,
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("refs/heads/{branch}^{{commit}}"),
    ])
    .map_err(|words| format!("the branch {branch:?} does not exist: {words}"))?
    .trim()
    .to_owned();

    enable_worktree_config(repository, &git_dir)?;
    let directory = furnish(repository, Checkout::Standing { branch })?;
    Ok(Workspace {
        repository: repository.to_owned(),
        branch: branch.to_owned(),
        base_commit: tip,
        directory,
    })
}

/// What both provisioners insist on before touching anything: an
/// absolute path that is a git repository. Reads only; the answer is
/// the repository's git dir, which the config work needs later. Shared
/// with `feature::tools`, so `start_feature` refuses a relative path in
/// exactly `start_build`'s words.
pub(crate) fn locate(repository: &str) -> Result<String, String> {
    if !Path::new(repository).is_absolute() {
        return Err(format!(
            "the repository must be an absolute local path for now, not {repository:?}"
        ));
    }
    let git_dir = plumbing(&["-C", repository, "rev-parse", "--absolute-git-dir"])
        .map_err(|words| format!("{repository} is not a git repository: {words}"))?;
    Ok(git_dir.trim().to_owned())
}

/// Turns per-worktree config on — and moves a bare repository's
/// `core.bare` into the main tree's own worktree config, or every
/// linked worktree would read it and believe itself bare. The one place
/// provisioning writes the repository's shared config, so it runs after
/// everything is validated, skips when the extension is already on —
/// the steady state — and holds a process-wide lock for the write:
/// concurrent provisions must not race each other onto git's
/// `config.lock`.
fn enable_worktree_config(repository: &str, git_dir: &str) -> Result<(), String> {
    static WRITE: Mutex<()> = Mutex::new(());
    let _serialized = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    let enabled = plumbing(&[
        "-C",
        repository,
        "config",
        "--get",
        "extensions.worktreeConfig",
    ]);
    if enabled.is_ok_and(|value| value.trim() == "true") {
        return Ok(());
    }
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
    Ok(())
}

/// A branch name fit to reach git as a positional: no leading dash, and
/// well-formed by git's own judgement.
fn valid_branch<'a>(repository: &str, branch: &'a str) -> Result<&'a str, String> {
    let branch = positional("branch", branch)?;
    plumbing(&["-C", repository, "check-ref-format", "--branch", branch])
        .map_err(|words| format!("{branch:?} is not a valid branch name: {words}"))?;
    Ok(branch)
}

/// How a workspace gets its branch: each provisioner is one variant,
/// so neither can reach the other's git invocation.
enum Checkout<'a> {
    /// Create `branch` at the commit `at` — `worktree add -b`.
    Create { branch: &'a str, at: &'a str },
    /// Check out `branch` as it already stands.
    Standing { branch: &'a str },
}

impl Checkout<'_> {
    const fn branch(&self) -> &str {
        match self {
            Self::Create { branch, .. } | Self::Standing { branch } => branch,
        }
    }
}

/// The worktree itself: a fresh directory under the system temp dir,
/// `worktree add` as `checkout` says — and the persona identity set
/// per-worktree, so every commit made inside is attributed to the
/// persona without touching the identity the repository's own config
/// keeps.
fn furnish(repository: &str, checkout: Checkout) -> Result<PathBuf, String> {
    let directory = temp::unique(&format!(
        "epik-build-{}",
        checkout.branch().replace(['/', '\\'], "-")
    ));
    let directory_str = directory
        .to_str()
        .ok_or("the temp dir path is not valid unicode")?
        .to_owned();
    let mut args = vec!["-C", repository, "worktree", "add"];
    match checkout {
        Checkout::Create { branch, at } => args.extend(["-b", branch, &directory_str, at]),
        Checkout::Standing { branch } => args.extend([directory_str.as_str(), branch]),
    }
    plumbing(&args)?;

    let identity = [("user.name", PERSONA_NAME), ("user.email", PERSONA_EMAIL)];
    for (key, value) in identity {
        plumbing(&["-C", &directory_str, "config", "--worktree", key, value])?;
    }
    Ok(directory)
}

/// Where a run stands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Phase {
    /// The Agent is running.
    Running,
    /// The Agent is gone; this is how it ended.
    Finished(Exit),
    /// The Agent is gone, but how it ended could not be read; the words
    /// say why.
    Lost(String),
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
const NARRATION_CAP: usize = 1000;
/// The most stderr lines a record retains; the oldest go first.
const STDERR_CAP: usize = 50;

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
/// the run — the drainer writes it, whoever ordered the job reads it.
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

    /// Folds one event in: narration or stderr.
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

/// Removes a workspace's worktree, force and all. Best effort: this is
/// cleanup, and a refusal has nobody to tell.
pub(crate) fn remove_worktree(workspace: &Workspace) {
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
}

/// Removes a provisioned worktree whose Agent never started, so a
/// failed start — or a factory that never produced one — leaves nothing
/// behind. Best effort.
pub fn abandon(workspace: &Workspace) {
    remove_worktree(workspace);
    let _ = plumbing(&[
        "-C",
        &workspace.repository,
        "branch",
        "-D",
        &workspace.branch,
    ]);
}

/// Attaches the started `agent` to the run `record` describes, and
/// returns at once. One drainer thread owns the Agent for the run's
/// life: it folds the events into the record, records the exit, and
/// then takes the commit observation and removes the worktree if clean.
/// The drainer's own [`JoinHandle`](std::thread::JoinHandle) comes back:
/// joining it is how a caller waits for the commit observation — the
/// last thing the drainer writes — bounded by the work itself rather
/// than by polling; dropping it detaches the drainer, which finishes on
/// its own.
///
/// `on_exit` runs on the drainer thread the moment the run's end is
/// recorded — the [`Phase::Finished`] transition, before the commit
/// observation — so a host can return an Agent slot the instant the
/// Agent is gone, even when the observation outlives the run.
pub fn launch(
    mut agent: Agent,
    record: Run,
    on_exit: impl FnOnce() + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for event in agent.events() {
            record
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .absorb(event);
        }
        let phase = match agent.wait() {
            Ok(exit) => Phase::Finished(exit),
            Err(error) => Phase::Lost(format!("{error:#}")),
        };
        let workspace = {
            let mut record = record.lock().unwrap_or_else(PoisonError::into_inner);
            record.phase = phase;
            record.workspace.clone()
        };
        on_exit();
        // Observed outside the lock: git takes its time.
        let commits = observe(&workspace);
        record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .commits = Some(commits);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Provisioning against a git_init-made bare repository, verified with
    // git2 — the independent implementation; launching against a
    // scripted Agent in the provisioned worktree.

    use crate::testing::Scratch;

    /// A bare repository with its empty root commit, made by git_init.
    fn bare(scratch: &Scratch) -> String {
        let directory = scratch.join("repo.git");
        let mut registry = crate::tools::Registry::default();
        registry.extend(crate::tools::git::all());
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

    /// Two provisions racing onto a fresh repository — whose shared
    /// config has never been written — both land: the config write is
    /// serialized and skipped once made, so neither loses to git's own
    /// `config.lock`.
    #[test]
    fn concurrent_provisions_against_a_fresh_repository_both_land() {
        let scratch = Scratch::new("race");
        let repository = bare(&scratch);

        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| provision(&order(&repository, "race/one")));
            let second = scope.spawn(|| provision(&order(&repository, "race/two")));
            (first.join().unwrap(), second.join().unwrap())
        });
        let first = first.unwrap();
        let second = second.unwrap();
        assert_ne!(first.directory, second.directory);

        tidy(&first);
        tidy(&second);
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
    fn a_record_folds_narration_and_stderr_and_caps_what_it_keeps() {
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
        assert!(matches!(
            record.result(),
            Some(Update::Result { ok: true, .. })
        ));
        assert_eq!(
            record.phase,
            Phase::Running,
            "the exit is the drainer's to record, after the stream"
        );
    }

    /// `script` under `sh -c` in the workspace's directory.
    fn agent_in(workspace: &Workspace, script: &str) -> Agent {
        Agent::new(
            vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            workspace.directory.to_string_lossy().into_owned(),
            [],
            None,
        )
        .expect("sh starts")
    }

    #[test]
    fn a_launched_agent_is_recorded_observed_and_its_clean_worktree_removed() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let scratch = Scratch::new("launch");
        let repository = bare(&scratch);
        let order = order(&repository, "launch/clean");
        let workspace = provision(&order).unwrap();
        let directory = workspace.directory.clone();
        let agent = agent_in(
            &workspace,
            concat!(
                r#"echo '{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}'; "#,
                "echo grumble >&2; ",
                "echo hello > hello.txt && git add hello.txt && ",
                "git -c commit.gpgsign=false commit -qm hello"
            ),
        );
        let run = Record::new(order, workspace);
        let exited = Arc::new(AtomicBool::new(false));
        let observer = launch(agent, Arc::clone(&run), {
            let exited = Arc::clone(&exited);
            move || exited.store(true, Ordering::SeqCst)
        });
        observer.join().unwrap();

        let record = run.lock().unwrap();
        assert_eq!(record.phase, Phase::Finished(Exit::Code(0)));
        assert!(exited.load(Ordering::SeqCst), "on_exit ran");
        assert_eq!(
            record.narration,
            [Update::Text {
                text: "hi".to_owned()
            }]
        );
        assert_eq!(record.stderr, ["grumble"]);
        let commits = record.commits.as_ref().expect("the observation is taken");
        assert!(commits.advanced, "{commits:?}");
        assert!(commits.clean, "{commits:?}");
        assert_eq!(commits.kept, None);
        assert!(!directory.exists(), "a clean worktree is removed");
    }

    #[test]
    fn a_launched_agent_that_commits_nothing_and_leaves_a_mess_keeps_its_worktree() {
        let scratch = Scratch::new("launch-dirty");
        let repository = bare(&scratch);
        let order = order(&repository, "launch/dirty");
        let workspace = provision(&order).unwrap();
        let directory = workspace.directory.clone();
        let agent = agent_in(&workspace, "echo half-done > mess.txt; exit 7");
        let run = Record::new(order, workspace.clone());
        launch(agent, Arc::clone(&run), || {}).join().unwrap();

        let record = run.lock().unwrap();
        assert_eq!(record.phase, Phase::Finished(Exit::Code(7)));
        let commits = record.commits.as_ref().expect("the observation is taken");
        assert!(!commits.advanced, "{commits:?}");
        assert!(!commits.clean, "{commits:?}");
        assert_eq!(commits.kept.as_deref(), Some(directory.as_path()));
        assert!(directory.exists(), "a dirty worktree is left where it is");
        drop(record);
        tidy(&workspace);
    }
}
