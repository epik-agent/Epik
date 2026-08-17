//! The build machinery end to end: a bare repository from git_init, a
//! provisioned worktree, a launched Agent through the real runner —
//! located by cargo, which is why these tests live in this package —
//! and the run record's verdict, verified with git2. All deterministic:
//! the Agent is an inline `sh -c` script, never a script file (the
//! ETXTBSY race), and no LLM is anywhere near.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use epik::agent::claude_code::Update;
use epik::agent::{Agent, Task};
use epik::build::{Order, Phase, Record, Run, launch, provision};
use epik::git::{PERSONA_EMAIL, PERSONA_NAME};

fn runner() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_epik-agent"))
}

/// A scratch directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "epik-launch-{name}-{}-{}",
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
    let mut registry = epik::tools::Registry::default();
    registry.extend(epik::git::all());
    let made = registry
        .dispatch(
            "git_init",
            &serde_json::json!({ "directory": directory }).to_string(),
        )
        .unwrap();
    assert_eq!(made["ok"], true, "{made}");
    directory
}

/// A test Agent: `sh -c` of an inline script, in the workspace. It also
/// speaks one stream-json line, so the record has narration to keep.
struct Shell {
    script: String,
    cwd: String,
}

impl Agent for Shell {
    fn task(&self) -> Task {
        Task {
            argv: vec![
                "sh".to_owned(),
                "-c".to_owned(),
                format!(
                    "{}; echo '{{\"type\":\"result\",\"is_error\":false,\"result\":\"done\"}}'",
                    self.script
                ),
            ],
            env: Vec::new(),
            cwd: self.cwd.clone(),
            stdin: None,
        }
    }
}

fn order(repository: &str, branch: &str) -> Order {
    Order {
        prompt: "build it".to_owned(),
        repository: repository.to_owned(),
        branch: branch.to_owned(),
        base: None,
    }
}

/// Provisions and launches `script` in the workspace, then waits for the
/// record to settle — the commit observation is the last thing written.
fn build(repository: &str, branch: &str, script: &str) -> Run {
    let order = order(repository, branch);
    let workspace = provision(&order).unwrap();
    let agent = Shell {
        script: script.to_owned(),
        cwd: workspace.directory.to_string_lossy().into_owned(),
    };
    let run = Record::new(order, workspace);
    let handle = launch(&agent, runner(), run.clone()).unwrap();
    handle.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while run.lock().unwrap().commits.is_none() {
        assert!(Instant::now() < deadline, "the observation never landed");
        std::thread::sleep(Duration::from_millis(20));
    }
    run
}

#[test]
fn an_agent_that_commits_leaves_an_advanced_branch_and_a_removed_worktree() {
    let scratch = Scratch::new("commits");
    let repository = bare(&scratch);

    let run = build(
        &repository,
        "wumpus",
        "printf 'wumpus\\n' > game.txt && git add game.txt && git commit -q -m 'Add the game'",
    );
    let record = run.lock().unwrap();

    assert!(
        matches!(record.phase, Phase::Finished(exit) if exit.code == Some(0)),
        "{:?}",
        record.phase
    );
    assert!(
        matches!(record.result(), Some(Update::Result { ok: true, .. })),
        "{:?}",
        record.narration
    );
    let commits = record.commits.as_ref().unwrap();
    assert!(commits.advanced, "{commits:?}");
    assert!(commits.clean, "{commits:?}");
    assert_eq!(commits.kept, None, "a clean worktree is removed");
    assert!(!record.workspace.directory.exists());

    // The independent implementation: the branch has the commit, authored
    // as the persona.
    let verified = git2::Repository::open(&repository).unwrap();
    let branch = verified
        .find_branch("wumpus", git2::BranchType::Local)
        .unwrap();
    let tip = branch.get().peel_to_commit().unwrap();
    assert_eq!(tip.id().to_string(), commits.head);
    assert_eq!(tip.message().unwrap().trim(), "Add the game");
    assert_eq!(tip.author().name(), Some(PERSONA_NAME));
    assert_eq!(tip.author().email(), Some(PERSONA_EMAIL));
    assert_eq!(tip.committer().email(), Some(PERSONA_EMAIL));
    assert_eq!(
        tip.parent(0).unwrap().id().to_string(),
        record.workspace.base_commit
    );
    assert_eq!(
        verified.worktrees().unwrap().len(),
        0,
        "no worktree remains registered"
    );
}

#[test]
fn an_agent_that_writes_without_committing_leaves_a_dirty_worktree_in_place() {
    let scratch = Scratch::new("dirty");
    let repository = bare(&scratch);

    let run = build(&repository, "half-done", "printf 'wip\\n' > notes.txt");
    let record = run.lock().unwrap();

    let commits = record.commits.as_ref().unwrap();
    assert!(!commits.advanced, "{commits:?}");
    assert!(!commits.clean, "{commits:?}");
    assert_eq!(
        commits.kept.as_deref(),
        Some(record.workspace.directory.as_path())
    );
    assert!(record.workspace.directory.join("notes.txt").exists());
    assert_eq!(commits.head, record.workspace.base_commit);

    let verified = git2::Repository::open(&repository).unwrap();
    let branch = verified
        .find_branch("half-done", git2::BranchType::Local)
        .unwrap();
    assert_eq!(
        branch.get().peel_to_commit().unwrap().id().to_string(),
        record.workspace.base_commit,
        "the branch never moved"
    );

    // Tidy what the harness deliberately left.
    let directory = record.workspace.directory.to_string_lossy().into_owned();
    let _ = std::process::Command::new("git")
        .args([
            "-C",
            &repository,
            "worktree",
            "remove",
            "--force",
            "--",
            &directory,
        ])
        .status();
}

#[test]
fn a_launch_that_cannot_spawn_removes_what_it_provisioned() {
    let scratch = Scratch::new("nospawn");
    let repository = bare(&scratch);
    let order = order(&repository, "orphan");
    let workspace = provision(&order).unwrap();
    let agent = Shell {
        script: "true".to_owned(),
        cwd: workspace.directory.to_string_lossy().into_owned(),
    };
    let directory = workspace.directory.clone();
    let run = Record::new(order, workspace);

    let error = launch(&agent, Path::new("/nonexistent/epik-agent"), run).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(!directory.exists(), "the worktree was abandoned");
    let verified = git2::Repository::open(&repository).unwrap();
    assert!(
        verified
            .find_branch("orphan", git2::BranchType::Local)
            .is_err(),
        "and the branch with it"
    );
}
