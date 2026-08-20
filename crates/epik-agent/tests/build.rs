//! The build machinery end to end: a bare repository from git_init, a
//! provisioned worktree, a launched Agent through the real runner —
//! located by cargo, which is why these tests live in this package —
//! and the run record's verdict, verified with git2. All deterministic:
//! the Agent is an inline `sh -c` script, never a script file (the
//! ETXTBSY race), and no LLM is anywhere near.

mod common;

use std::path::Path;

use common::{Scratch, Shell, runner};
use epik::agent::claude_code::Update;
use epik::build::{Order, Phase, Record, Run, launch, provision};
use epik::git::{PERSONA_EMAIL, PERSONA_NAME};

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

fn order(repository: &str, branch: &str) -> Order {
    Order {
        prompt: "build it".to_owned(),
        repository: repository.to_owned(),
        branch: branch.to_owned(),
        base: None,
    }
}

/// Provisions and launches `script` in the workspace, then waits for the
/// record to settle: the commit observation is the last thing the
/// drainer writes, and joining the drainer is the bounded wait for it.
/// The script gets one stream-json line appended, so the record has
/// narration to keep.
fn build(repository: &str, branch: &str, script: &str) -> Run {
    let order = order(repository, branch);
    let workspace = provision(&order).unwrap();
    let agent = Shell {
        script: format!(
            "{script}; echo '{{\"type\":\"result\",\"is_error\":false,\"result\":\"done\"}}'"
        ),
        cwd: workspace.directory.to_string_lossy().into_owned(),
    };
    let run = Record::new(order, workspace);
    let (handle, observer) = launch(&agent, runner(), run.clone()).unwrap();
    handle.wait().unwrap();
    observer.join().unwrap();
    assert!(run.lock().unwrap().commits.is_some());
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
