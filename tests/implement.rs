// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use anyhow::{Context, Result};
use epik::implementation::{Feature, Implementable, Issue};
use epik::logging::{Event, Log, Silent};
use epik::repository::{Branch, Endpoint, Repository, Url};
use epik::tree::Tree;
use serde::Serialize;

const OUTPUT_FILE: &str = "output.txt";

/// This test's choice of issue implementation: "implementing" an issue
/// appends its description to the output file and commits the result to the
/// destination branch.
#[derive(Debug)]
struct AppendToOutput(Issue);

impl Implementable for AppendToOutput {
    fn implement(&self, _source: &Endpoint, dest: &Endpoint, log: &mut dyn Log) -> Result<()> {
        log.emit(Event::IssueStarted { id: self.0.id });
        let Url(path) = &dest.url();
        let git = git2::Repository::open(path)
            .with_context(|| format!("opening working copy at {}", path.display()))?;
        let workdir = git.workdir().expect("test working copies are never bare");

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(workdir.join(OUTPUT_FILE))
            .with_context(|| format!("appending to {OUTPUT_FILE}"))?;
        writeln!(file, "{}", self.0.description)?;

        let mut index = git.index()?;
        index.add_path(Path::new(OUTPUT_FILE))?;
        index.write()?;
        let tree = git.find_tree(index.write_tree()?)?;
        let signature = git2::Signature::now("Epik", "epik@localhost")?;
        let branch_ref = format!("refs/heads/{}", dest.branch().as_str());
        let parent = git
            .find_reference(&branch_ref)
            .ok()
            .and_then(|r| r.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        let message = format!("Implement issue #{}: {}", self.0.id, self.0.description);
        git.commit(
            Some(&branch_ref),
            &signature,
            &signature,
            &message,
            &tree,
            &parents,
        )?;
        log.emit(Event::IssueImplemented { id: self.0.id });
        Ok(())
    }
}

/// Red is the parent of Green and Blue.
fn red_green_blue() -> Tree<Issue> {
    Tree {
        value: Issue::new(1, "Red"),
        children: vec![
            Tree::new(Issue::new(2, "Green")),
            Tree::new(Issue::new(3, "Blue")),
        ],
    }
}

fn red_green_blue_feature(dir: &tempfile::TempDir) -> Feature<AppendToOutput> {
    Feature {
        repository: Repository::new(Url(dir.path().to_owned())),
        issues: red_green_blue().map(AppendToOutput),
        reviewer: None,
    }
}

/// The event stream a red/green/blue run should produce: BFS order, one
/// started/implemented pair per issue.
const fn expected_events() -> [Event; 6] {
    [
        Event::IssueStarted { id: 1 },
        Event::IssueImplemented { id: 1 },
        Event::IssueStarted { id: 2 },
        Event::IssueImplemented { id: 2 },
        Event::IssueStarted { id: 3 },
        Event::IssueImplemented { id: 3 },
    ]
}

/// Creates an empty disposable git repository with `main` checked out and
/// returns an endpoint pointing at it. The tempdir is returned so it stays
/// alive for the duration of the test.
fn disposable_repo() -> (tempfile::TempDir, Endpoint) {
    let dir = tempfile::tempdir().unwrap();
    let git = git2::Repository::init(dir.path()).unwrap();
    git.set_head("refs/heads/main").unwrap();
    let endpoint = Endpoint::new(Url(dir.path().to_owned()), Branch::new("main"));
    (dir, endpoint)
}

/// The output file is checked in: it reads Red/Green/Blue, nothing is left
/// dirty or untracked, and each issue produced its own commit on main.
fn assert_red_green_blue_committed(dir: &tempfile::TempDir) {
    let output = std::fs::read_to_string(dir.path().join(OUTPUT_FILE)).unwrap();
    assert_eq!(output, "Red\nGreen\nBlue\n");

    let git = git2::Repository::open(dir.path()).unwrap();
    let mut status_options = git2::StatusOptions::new();
    status_options.include_untracked(true);
    let statuses = git.statuses(Some(&mut status_options)).unwrap();
    assert!(statuses.is_empty(), "working tree should be clean");

    let head = git.head().unwrap();
    assert_eq!(head.name(), Some("refs/heads/main"));
    let mut walk = git.revwalk().unwrap();
    walk.push_head().unwrap();
    assert_eq!(walk.count(), 3);
}

#[test]
fn implementing_a_feature_commits_issues_in_bfs_order() {
    let (dir, endpoint) = disposable_repo();
    let feature = red_green_blue_feature(&dir);

    feature
        .implement(&endpoint, &endpoint, &mut Silent)
        .unwrap();

    assert_red_green_blue_committed(&dir);
}

#[test]
fn implementation_events_stream_over_a_channel() {
    let (dir, endpoint) = disposable_repo();
    let feature = red_green_blue_feature(&dir);

    let (mut sender, receiver) = mpsc::channel();
    feature
        .implement(&endpoint, &endpoint, &mut sender)
        .unwrap();
    drop(sender);

    let events: Vec<Event> = receiver.iter().collect();
    assert_eq!(events, expected_events());
}

/// Mirror of the worker binary's Job: what a run needs to know, on the wire.
#[derive(Serialize)]
struct Job<'a> {
    source: &'a Endpoint,
    dest: &'a Endpoint,
    issues: Tree<Issue>,
}

#[test]
fn implementing_in_a_separate_process_streams_events() {
    let (dir, endpoint) = disposable_repo();
    let job = Job {
        source: &endpoint,
        dest: &endpoint,
        issues: red_green_blue(),
    };

    let mut worker = Command::new(env!("CARGO_BIN_EXE_epik-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(worker.stdin.take().unwrap(), &job).unwrap();
    let output = worker.wait_with_output().unwrap();
    assert!(output.status.success(), "worker exited with failure");

    let events: Vec<Event> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events, expected_events());

    assert_red_green_blue_committed(&dir);
}
