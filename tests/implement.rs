// Tests are entitled to panic. The allow-unwrap-in-tests clippy setting only
// covers #[test] functions, not the helpers here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use epik::implementation::{Feature, Implementable, Issue};
use epik::repository::{Branch, Endpoint, Repository, Url};
use epik::tree::Tree;

const OUTPUT_FILE: &str = "output.txt";

/// This test's choice of issue implementation: "implementing" an issue
/// appends its description to the output file and commits the result to the
/// destination branch.
#[derive(Debug)]
struct AppendToOutput(Issue);

impl Implementable for AppendToOutput {
    fn implement(&self, _source: &Endpoint, dest: &Endpoint) -> Result<()> {
        let Url::Local(path) = dest.url() else {
            panic!("test endpoints are always local");
        };
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
        Ok(())
    }
}

/// Creates an empty disposable git repository with `main` checked out and
/// returns an endpoint pointing at it. The tempdir is returned so it stays
/// alive for the duration of the test.
fn disposable_repo() -> (tempfile::TempDir, Endpoint) {
    let dir = tempfile::tempdir().unwrap();
    let git = git2::Repository::init(dir.path()).unwrap();
    git.set_head("refs/heads/main").unwrap();
    let endpoint = Endpoint::new(Url::local(dir.path()), Branch::new("main"));
    (dir, endpoint)
}

#[test]
fn implementing_a_feature_commits_issues_in_bfs_order() {
    let (dir, endpoint) = disposable_repo();

    // Red is the parent of Green and Blue.
    let feature = Feature {
        repository: Repository::new(Url::local(dir.path())),
        issues: Tree {
            value: AppendToOutput(Issue::new(1, "Red")),
            children: vec![
                Tree::new(AppendToOutput(Issue::new(2, "Green"))),
                Tree::new(AppendToOutput(Issue::new(3, "Blue"))),
            ],
        },
        reviewer: None,
    };

    feature.implement(&endpoint, &endpoint).unwrap();

    let output = std::fs::read_to_string(dir.path().join(OUTPUT_FILE)).unwrap();
    assert_eq!(output, "Red\nGreen\nBlue\n");

    // The output file is checked in: nothing left dirty or untracked, and
    // each issue produced its own commit on main.
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
