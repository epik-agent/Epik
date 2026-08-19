//! Work lands on a feature branch through machinery, never through an
//! Agent: the build merges; the Agent never touches the feature branch.
//!
//! [`Branch::establish`] cuts the feature branch at its base and pushes
//! it to the [`Forge`] — a branch already standing is used as it stands
//! — and keeps one workspace of it for merging, provisioned by
//! [`adopt`](crate::build::adopt), distinct from the per-issue
//! workspaces. [`Branch::merge`] then brings issue branches in one at a
//! time: `--no-ff` under a lock, the repository's own
//! [`Check`](crate::check::Check) run on the result while the lock is
//! still held — after the merge, not before, so it catches both an
//! Agent that lied and two siblings that build alone and not together —
//! and the branch pushed on green. A conflict aborts the merge, leaves
//! the issue branch intact, and reports the conflicting paths in git's
//! own reckoning; a red check resets the branch to the exact commit it
//! stood on and reports the check's output. Every commit on a feature
//! branch is green.
//!
//! A build handed no check merges on observation alone, and the
//! [`Outcome`] says so.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use crate::build::{self, Workspace, positional};
use crate::check::{self, Check};
use crate::forge::{self, Forge};
use crate::git::plumbing;

/// The feature branch as a build holds it: the workspace kept for
/// merging, the forge its pushes write to, the check that judges every
/// merge — and the lock that makes merges one at a time.
#[derive(Debug)]
pub struct Branch<F: Forge> {
    workspace: Workspace,
    forge: F,
    check: Option<Check>,
    lock: Mutex<()>,
}

/// Where one merge got to. Conflicts and red checks are outcomes, not
/// faults: the branch has been put back where it stood, and the caller
/// has an issue to fail with these words.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Merged, judged, pushed. `checked` is false when there was no
    /// check to run — the record must say the branch is unchecked.
    Merged { commit: String, checked: bool },
    /// The merge conflicted: it was aborted, the feature branch never
    /// moved, and the issue branch stands intact. The paths git could
    /// not settle.
    Conflict { paths: Vec<String> },
    /// The merged result failed the check: the feature branch is back
    /// on the commit it stood on and the remote is untouched. The
    /// check's own output.
    Red { output: String },
}

impl<F: Forge> Branch<F> {
    /// Establishes the feature branch `name` in `repository`: created
    /// at `base` and pushed to `forge` when new; a branch already
    /// standing is used as it stands, and not moved. Keeps one
    /// workspace of the branch for merging.
    ///
    /// # Errors
    ///
    /// Words for the model: the repository is not one, the base does
    /// not name a commit, the branch could not be pushed, or git failed
    /// along the way.
    pub fn establish(
        repository: &str,
        name: &str,
        base: &str,
        forge: F,
        check: Option<Check>,
    ) -> Result<Self, String> {
        let name = positional("branch", name)?;
        let standing = plumbing(&[
            "-C",
            repository,
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("refs/heads/{name}"),
        ])
        .is_ok();
        if !standing {
            let base = positional("base", base)?;
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
            plumbing(&["-C", repository, "branch", "--", name, &base_commit])?;
            forge::push(Path::new(repository), name, &forge)?;
        }
        let workspace = build::adopt(repository, name)?;
        Ok(Self {
            workspace,
            forge,
            check,
            lock: Mutex::new(()),
        })
    }

    /// The workspace the branch is held through — where the check runs,
    /// and the mark the branch stood on when established.
    #[must_use]
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Merges `branch` into the feature branch: `--no-ff`, one at a
    /// time, the check run on the result under the same lock, the
    /// branch pushed when it holds. A conflict or a red check comes
    /// back as an [`Outcome`] with the branch already put right.
    ///
    /// # Errors
    ///
    /// Git failing for reasons that are not a conflict — and the push
    /// failing, in which case the merge stands locally and the remote
    /// is behind.
    pub fn merge(&self, branch: &str) -> Result<Outcome, String> {
        let branch = positional("branch", branch)?;
        let _serialized = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let directory = self
            .workspace
            .directory
            .to_str()
            .ok_or("the workspace path is not valid unicode")?;

        let before = plumbing(&["-C", directory, "rev-parse", "HEAD"])?
            .trim()
            .to_owned();
        let merged = plumbing(&[
            "-C",
            directory,
            "-c",
            "commit.gpgsign=false",
            "merge",
            "--no-ff",
            "--no-edit",
            branch,
        ]);
        if let Err(words) = merged {
            // Git's own report of what it could not settle; empty means
            // the merge failed for some other reason, which is a fault.
            let paths: Vec<String> =
                plumbing(&["-C", directory, "diff", "--name-only", "--diff-filter=U"])
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_owned)
                    .collect();
            if paths.is_empty() {
                return Err(words);
            }
            plumbing(&["-C", directory, "merge", "--abort"])?;
            return Ok(Outcome::Conflict { paths });
        }

        if let Some(check) = &self.check {
            let verdict = check::run(check, &self.workspace.directory);
            if !verdict.green {
                plumbing(&["-C", directory, "reset", "--hard", &before])?;
                return Ok(Outcome::Red {
                    output: verdict.output,
                });
            }
        }

        forge::push(
            &self.workspace.directory,
            &self.workspace.branch,
            &self.forge,
        )?;
        let commit = plumbing(&["-C", directory, "rev-parse", "HEAD"])?
            .trim()
            .to_owned();
        Ok(Outcome::Merged {
            commit,
            checked: self.check.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{Order, provision};
    use crate::forge::Credentials;

    /// A forge for tests: a bare directory, no credentials.
    struct Local(String);

    impl Forge for Local {
        fn remote(&self) -> String {
            self.0.clone()
        }

        fn credentials(&self) -> Option<Credentials> {
            None
        }
    }

    /// A scratch directory that cleans up after itself.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "epik-merge-{name}-{}-{}",
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

    fn git(args: &[&str]) -> String {
        plumbing(args).unwrap()
    }

    /// A working repository with one commit on main, and a bare remote
    /// for the forge.
    fn seeded(scratch: &Scratch) -> (String, Local) {
        let work = scratch.join("work");
        let remote = scratch.join("remote.git");
        git(&["init", "--initial-branch=main", &work]);
        git(&["-C", &work, "config", "user.name", "Test"]);
        git(&["-C", &work, "config", "user.email", "test@example.com"]);
        git(&["-C", &work, "config", "commit.gpgsign", "false"]);
        std::fs::write(std::path::Path::new(&work).join("hello.txt"), "hello\n").unwrap();
        git(&["-C", &work, "add", "hello.txt"]);
        git(&["-C", &work, "commit", "-m", "the first commit"]);
        git(&["init", "--bare", "--initial-branch=main", &remote]);
        (work, Local(remote))
    }

    /// The scripted Agent: cuts an issue branch from the feature tip,
    /// writes `content` at `path`, and commits it — everything a real
    /// Agent's landing looks like, none of the model.
    fn agent(repository: &str, branch: &str, path: &str, content: &str) -> Workspace {
        let workspace = provision(&Order {
            prompt: "scripted".to_owned(),
            repository: repository.to_owned(),
            branch: branch.to_owned(),
            base: Some("feature/wumpus".to_owned()),
        })
        .unwrap();
        let directory = workspace.directory.to_str().unwrap().to_owned();
        std::fs::write(workspace.directory.join(path), content).unwrap();
        git(&["-C", &directory, "add", path]);
        git(&[
            "-C",
            &directory,
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            &format!("write {path}"),
        ]);
        workspace
    }

    /// Removes a worktree so the scratch dir's drop is enough.
    fn tidy(workspace: &Workspace) {
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

    fn tip(repository: &str, branch: &str) -> String {
        git(&[
            "-C",
            repository,
            "rev-parse",
            &format!("refs/heads/{branch}"),
        ])
        .trim()
        .to_owned()
    }

    #[test]
    fn establishing_cuts_the_branch_at_the_base_and_pushes_it() {
        let scratch = Scratch::new("establish");
        let (work, forge) = seeded(&scratch);
        let remote = forge.0.clone();

        let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();

        let base = tip(&work, "main");
        assert_eq!(tip(&work, "feature/wumpus"), base);
        assert_eq!(branch.workspace().base_commit, base);
        assert!(branch.workspace().directory.is_dir());

        // The remote already holds the branch, before any merge.
        let verified = git2::Repository::open(&remote).unwrap();
        let pushed = verified
            .find_branch("feature/wumpus", git2::BranchType::Local)
            .unwrap();
        assert_eq!(
            pushed.get().peel_to_commit().unwrap().id().to_string(),
            base
        );

        tidy(branch.workspace());
    }

    #[test]
    fn a_branch_already_standing_is_used_as_it_stands() {
        let scratch = Scratch::new("standing");
        let (work, forge) = seeded(&scratch);
        let remote = forge.0.clone();
        // The branch predates the build, one commit behind main.
        let old = git(&["-C", &work, "rev-parse", "HEAD"]).trim().to_owned();
        std::fs::write(std::path::Path::new(&work).join("later.txt"), "later\n").unwrap();
        git(&["-C", &work, "add", "later.txt"]);
        git(&["-C", &work, "commit", "-m", "later"]);
        git(&["-C", &work, "branch", "feature/wumpus", &old]);

        let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();

        assert_eq!(branch.workspace().base_commit, old, "not moved to main");
        let verified = git2::Repository::open(&remote).unwrap();
        assert!(
            verified
                .find_branch("feature/wumpus", git2::BranchType::Local)
                .is_err(),
            "a standing branch is not pushed; merges will push it"
        );

        tidy(branch.workspace());
    }

    #[test]
    fn two_siblings_from_the_same_tip_both_merge_when_their_edits_do_not_overlap() {
        let scratch = Scratch::new("siblings");
        let (work, forge) = seeded(&scratch);
        let remote = forge.0.clone();
        let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();

        // Both Agents cut from the same tip.
        let first = agent(&work, "issue-1", "one.txt", "one\n");
        let second = agent(&work, "issue-2", "two.txt", "two\n");

        let Outcome::Merged { commit, checked } = branch.merge("issue-1").unwrap() else {
            panic!("the first sibling merges");
        };
        assert!(!checked, "no check was handed in");
        assert!(matches!(
            branch.merge("issue-2").unwrap(),
            Outcome::Merged { .. }
        ));

        // Both edits are on the branch, each behind a --no-ff merge
        // commit — even the first, which was fast-forwardable.
        let verified = git2::Repository::open(&work).unwrap();
        let head = verified
            .find_branch("feature/wumpus", git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap();
        assert_eq!(head.parent_count(), 2);
        assert_eq!(head.parent(0).unwrap().id().to_string(), commit);
        assert_eq!(head.parent(0).unwrap().parent_count(), 2);
        let tree = head.tree().unwrap();
        assert!(tree.get_name("one.txt").is_some());
        assert!(tree.get_name("two.txt").is_some());

        // The remote kept pace, one push per merge.
        let mirrored = git2::Repository::open(&remote).unwrap();
        let far = mirrored
            .find_branch("feature/wumpus", git2::BranchType::Local)
            .unwrap();
        assert_eq!(far.get().peel_to_commit().unwrap().id(), head.id());

        tidy(&first);
        tidy(&second);
        tidy(branch.workspace());
    }

    #[test]
    fn the_second_overlapping_sibling_is_refused_with_its_conflicting_paths_named() {
        let scratch = Scratch::new("conflict");
        let (work, forge) = seeded(&scratch);
        let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();

        let first = agent(&work, "issue-1", "hello.txt", "first words\n");
        let second = agent(&work, "issue-2", "hello.txt", "second words\n");

        assert!(matches!(
            branch.merge("issue-1").unwrap(),
            Outcome::Merged { .. }
        ));
        let after_first = tip(&work, "feature/wumpus");
        let issue_tip = tip(&work, "issue-2");

        let Outcome::Conflict { paths } = branch.merge("issue-2").unwrap() else {
            panic!("overlapping edits conflict");
        };
        assert_eq!(paths, ["hello.txt"]);

        // Aborted: the feature branch never moved, its workspace is
        // clean, and the issue branch stands intact for the report.
        assert_eq!(tip(&work, "feature/wumpus"), after_first);
        assert_eq!(tip(&work, "issue-2"), issue_tip);
        let directory = branch.workspace().directory.to_str().unwrap().to_owned();
        assert_eq!(git(&["-C", &directory, "status", "--porcelain"]).trim(), "");

        tidy(&first);
        tidy(&second);
        tidy(branch.workspace());
    }

    #[test]
    fn a_red_check_resets_the_branch_and_leaves_the_remote_unchanged() {
        let scratch = Scratch::new("red");
        let (work, forge) = seeded(&scratch);
        let remote = forge.0.clone();
        // The repository's own idea of green: the poison file is absent.
        let check = Check {
            command: "test ! -f poison.txt || { echo the wumpus got in; exit 1; }".to_owned(),
        };
        let branch =
            Branch::establish(&work, "feature/wumpus", "main", forge, Some(check)).unwrap();
        let before = tip(&work, "feature/wumpus");
        let far_before = tip(&remote, "feature/wumpus");

        let issue = agent(&work, "issue-1", "poison.txt", "red\n");
        let Outcome::Red { output } = branch.merge("issue-1").unwrap() else {
            panic!("the poisoned merge is judged red");
        };
        assert!(output.contains("the wumpus got in"), "{output}");

        // The exact pre-merge commit, locally and at the remote.
        assert_eq!(tip(&work, "feature/wumpus"), before);
        assert_eq!(tip(&remote, "feature/wumpus"), far_before);

        tidy(&issue);
        tidy(branch.workspace());
    }

    #[test]
    fn a_green_check_keeps_the_merge_and_advances_the_remote() {
        let scratch = Scratch::new("green");
        let (work, forge) = seeded(&scratch);
        let remote = forge.0.clone();
        let check = Check {
            command: "test -f hello.txt".to_owned(),
        };
        let branch =
            Branch::establish(&work, "feature/wumpus", "main", forge, Some(check)).unwrap();

        let issue = agent(&work, "issue-1", "one.txt", "one\n");
        let Outcome::Merged { commit, checked } = branch.merge("issue-1").unwrap() else {
            panic!("a green result lands");
        };
        assert!(checked);
        assert_eq!(tip(&work, "feature/wumpus"), commit);
        assert_eq!(tip(&remote, "feature/wumpus"), commit);

        tidy(&issue);
        tidy(branch.workspace());
    }

    /// The lock, exercised: two threads merging through one Branch both
    /// land, because merges queue rather than trample.
    #[test]
    fn concurrent_merges_are_serialized_and_both_land() {
        let scratch = Scratch::new("threads");
        let (work, forge) = seeded(&scratch);
        let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();

        let first = agent(&work, "issue-1", "one.txt", "one\n");
        let second = agent(&work, "issue-2", "two.txt", "two\n");

        std::thread::scope(|scope| {
            let merges = [
                scope.spawn(|| branch.merge("issue-1")),
                scope.spawn(|| branch.merge("issue-2")),
            ];
            for merge in merges {
                assert!(matches!(
                    merge.join().unwrap().unwrap(),
                    Outcome::Merged { .. }
                ));
            }
        });

        let verified = git2::Repository::open(&work).unwrap();
        let tree = verified
            .find_branch("feature/wumpus", git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        assert!(tree.get_name("one.txt").is_some());
        assert!(tree.get_name("two.txt").is_some());

        tidy(&first);
        tidy(&second);
        tidy(branch.workspace());
    }
}
