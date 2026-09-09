//! Work lands on a feature branch through machinery, never through an
//! Agent: the build merges; the Agent never touches the feature branch.
//!
//! [`Branch::establish`] cuts the feature branch at its base and pushes
//! it to the [`Forge`] — a branch already standing is used as it stands
//! — and keeps one workspace of it for merging, provisioned by
//! [`adopt`](job::adopt), distinct from the per-issue
//! workspaces. [`Branch::merge`] then brings issue branches in one at a
//! time: `--no-ff` under a lock, the repository's own
//! [`Check`] run on the result while the lock is
//! still held — after the merge, not before, so it catches both an
//! Agent that lied and two siblings that build alone and not together —
//! and the branch pushed on green. A conflict aborts the merge, leaves
//! the issue branch intact, and reports the conflicting paths in git's
//! own reckoning; a red check — and a failed push, the same posture —
//! resets the branch to the exact commit it stood on, so an error
//! always means nothing landed. Every commit on a feature branch is
//! green.
//!
//! [`Branch::tip`] reads the branch's settled tip under the same lock,
//! which is where an issue branch is cut from at dispatch: a merge
//! mid-judgement, whose commit may yet be reset away, can never be the
//! answer.
//!
//! A build handed no check merges on observation alone, and the
//! [`Outcome`] says so.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use super::check::{self, Check};
use crate::forge::{self, Forge};
use crate::git::{base_commit, plumbing};
use crate::job::{self, Workspace, positional};

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
pub(super) enum Outcome {
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
    pub(super) fn establish(
        repository: &str,
        name: &str,
        base: &str,
        forge: F,
        check: Option<Check>,
    ) -> Result<Self, String> {
        let name = positional("branch", name)?;
        // Absence and failure are different answers: `for-each-ref`
        // exits zero either way and simply lists nothing for a branch
        // that is not there, so a repository that cannot be read blames
        // itself — never the base — and never sends the build down the
        // create path onto a name that exists.
        let ref_name = format!("refs/heads/{name}");
        let standing = plumbing(&[
            "-C",
            repository,
            "for-each-ref",
            "--format=%(refname)",
            &ref_name,
        ])
        .map_err(|words| format!("could not read the branches of {repository}: {words}"))?
        .lines()
        .any(|line| line.trim() == ref_name);
        if !standing {
            let base = positional("base", base)?;
            let base_commit = base_commit(repository, base)?;
            plumbing(&["-C", repository, "branch", "--", name, &base_commit])?;
            // A branch created but never pushed must not read as
            // standing on retry — unwind it, so the retry re-creates
            // and re-pushes instead of silently skipping the remote.
            if let Err(words) = forge::push(Path::new(repository), name, &forge) {
                let _ = plumbing(&["-C", repository, "branch", "-D", name]);
                return Err(words);
            }
        }
        let workspace = job::adopt(repository, name)?;
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
    pub(super) fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The check that judges every merge; `None` is an unchecked branch.
    #[must_use]
    pub(super) const fn check(&self) -> Option<&Check> {
        self.check.as_ref()
    }

    /// The feature branch's tip as it stands settled — read under the
    /// merge lock, so a merge mid-judgement, whose commit a red check
    /// or a failed push may yet reset away, can never be the answer.
    /// Every commit this returns is permanent: it is where an issue
    /// branch is cut from at dispatch.
    ///
    /// # Errors
    ///
    /// Git failing to read the ref, in its own words.
    pub(super) fn tip(&self) -> Result<String, String> {
        let _serialized = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let directory = self
            .workspace
            .directory
            .to_str()
            .ok_or("the workspace path is not valid unicode")?;
        Ok(plumbing(&[
            "-C",
            directory,
            "rev-parse",
            &format!("refs/heads/{}", self.workspace.branch),
        ])?
        .trim()
        .to_owned())
    }

    /// Merges `branch` into the feature branch: `--no-ff`, one at a
    /// time, the check run on the result under the same lock, the
    /// branch pushed when it holds. A conflict or a red check comes
    /// back as an [`Outcome`] with the branch already put right.
    ///
    /// Each merge is self-contained: it opens, still under the lock, by
    /// restoring the workspace to a clean checkout of the feature
    /// branch tip — a standing half-merge whose abort once failed, or
    /// tracked files a green check dirtied, cannot reach it.
    ///
    /// # Errors
    ///
    /// Git failing for reasons that are not a conflict — and the push
    /// failing, in which case the merge is unwound: the feature branch
    /// is back on its pre-merge commit and the issue branch stands
    /// intact, so an error always means nothing landed and a retry can
    /// merge again.
    pub(super) fn merge(&self, branch: &str) -> Result<Outcome, String> {
        let branch = positional("branch", branch)?;
        let _serialized = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let directory = self
            .workspace
            .directory
            .to_str()
            .ok_or("the workspace path is not valid unicode")?;

        // The opening restore. "There is no merge to abort" is the
        // ordinary answer, not a fault; the reset is what must hold.
        let _ = plumbing(&["-C", directory, "merge", "--abort"]);
        plumbing(&["-C", directory, "reset", "--hard", "HEAD"])?;

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
            // Git's own report of what it could not settle — read
            // before the abort erases it, judged after the abort has
            // been attempted, so the report can never skip the abort.
            let probed = plumbing(&["-C", directory, "diff", "--name-only", "--diff-filter=U"]);
            let _ = plumbing(&["-C", directory, "merge", "--abort"]);
            let paths: Vec<String> = probed
                .map_err(|probe| {
                    format!(
                        "the merge failed ({words}) and the conflict could not be read: {probe}"
                    )
                })?
                .lines()
                .map(str::to_owned)
                .collect();
            if paths.is_empty() {
                return Err(words);
            }
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

        // A merge the remote never saw must not stand: unwind it, so an
        // error always means nothing landed — the same posture as
        // reset-on-red — and the caller's record never contradicts what
        // the next push publishes.
        if let Err(words) = forge::push(
            &self.workspace.directory,
            &self.workspace.branch,
            &self.forge,
        ) {
            plumbing(&["-C", directory, "reset", "--hard", &before]).map_err(|reset| {
                format!("the push failed ({words}) and the merge could not be unwound: {reset}")
            })?;
            return Err(words);
        }
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
    use crate::job::{Order, provision};

    use crate::testing::{Local, Scratch, seeded};

    fn git(args: &[&str]) -> String {
        plumbing(args).unwrap()
    }

    /// A seeded repository with the feature branch established over it,
    /// `check` in force: the work and remote paths, and the branch. How
    /// most merge tests open.
    fn established(scratch: &Scratch, check: Option<Check>) -> (String, String, Branch<Local>) {
        let (work, remote) = seeded(scratch);
        let forge = Local(remote.clone());
        let branch = Branch::establish(&work, "feature/wumpus", "main", forge, check).unwrap();
        (work, remote, branch)
    }

    /// The commit at the tip of `branch`, as git2 reads it.
    fn commit_of<'r>(repo: &'r git2::Repository, branch: &str) -> git2::Commit<'r> {
        repo.find_branch(branch, git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap()
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
        job::remove_worktree(workspace);
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
        let (work, remote, branch) = established(&scratch, None);

        let base = tip(&work, "main");
        assert_eq!(tip(&work, "feature/wumpus"), base);
        assert_eq!(branch.workspace().base_commit, base);
        assert!(branch.workspace().directory.is_dir());

        // The remote already holds the branch, before any merge.
        let verified = git2::Repository::open(&remote).unwrap();
        assert_eq!(
            commit_of(&verified, "feature/wumpus").id().to_string(),
            base
        );

        tidy(branch.workspace());
    }

    #[test]
    fn a_branch_already_standing_is_used_as_it_stands() {
        let scratch = Scratch::new("standing");
        let (work, remote) = seeded(&scratch);
        // The branch predates the build, one commit behind main.
        let old = git(&["-C", &work, "rev-parse", "HEAD"]).trim().to_owned();
        std::fs::write(Path::new(&work).join("later.txt"), "later\n").unwrap();
        git(&["-C", &work, "add", "later.txt"]);
        git(&["-C", &work, "commit", "-m", "later"]);
        git(&["-C", &work, "branch", "feature/wumpus", &old]);

        let forge = Local(remote.clone());
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

    /// A create-path push that fails unwinds the local branch it just
    /// made — a retry must re-create and re-push, not read the orphan
    /// as standing and leave the remote without it forever.
    #[test]
    fn a_failed_push_unwinds_the_created_branch_so_a_retry_reaches_the_remote() {
        let scratch = Scratch::new("unwind");
        let (work, remote) = seeded(&scratch);

        let gone = Local(scratch.join("gone.git"));
        let error = Branch::establish(&work, "feature/wumpus", "main", gone, None).unwrap_err();
        assert!(error.contains("gone.git"), "{error}");
        let listed = git(&["-C", &work, "for-each-ref", "refs/heads/feature/wumpus"]);
        assert_eq!(listed.trim(), "", "the created branch was unwound");

        // The retry, against a forge that answers, creates and pushes.
        let forge = Local(remote.clone());
        let branch = Branch::establish(&work, "feature/wumpus", "main", forge, None).unwrap();
        let verified = git2::Repository::open(&remote).unwrap();
        assert!(
            verified
                .find_branch("feature/wumpus", git2::BranchType::Local)
                .is_ok()
        );

        tidy(branch.workspace());
    }

    #[test]
    fn two_siblings_from_the_same_tip_both_merge_when_their_edits_do_not_overlap() {
        let scratch = Scratch::new("siblings");
        let (work, remote, branch) = established(&scratch, None);

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
        let head = commit_of(&verified, "feature/wumpus");
        assert_eq!(head.parent_count(), 2);
        assert_eq!(head.parent(0).unwrap().id().to_string(), commit);
        assert_eq!(head.parent(0).unwrap().parent_count(), 2);
        let tree = head.tree().unwrap();
        assert!(tree.get_name("one.txt").is_some());
        assert!(tree.get_name("two.txt").is_some());

        // The remote kept pace, one push per merge.
        let mirrored = git2::Repository::open(&remote).unwrap();
        assert_eq!(commit_of(&mirrored, "feature/wumpus").id(), head.id());

        tidy(&first);
        tidy(&second);
        tidy(branch.workspace());
    }

    #[test]
    fn the_second_overlapping_sibling_is_refused_with_its_conflicting_paths_named() {
        let scratch = Scratch::new("conflict");
        let (work, _, branch) = established(&scratch, None);

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

        // The conflict poisoned nothing: the next sibling still lands.
        let third = agent(&work, "issue-3", "three.txt", "three\n");
        assert!(matches!(
            branch.merge("issue-3").unwrap(),
            Outcome::Merged { .. }
        ));

        tidy(&first);
        tidy(&second);
        tidy(&third);
        tidy(branch.workspace());
    }

    /// A green check that mutates tracked files — a formatter, a test
    /// refreshing a lockfile — must not wedge the workspace: each merge
    /// opens by restoring a clean checkout, so the next one still lands.
    #[test]
    fn a_check_that_dirties_the_workspace_on_green_does_not_reach_the_next_merge() {
        let scratch = Scratch::new("dirty");
        let check = Check {
            command: "echo dirt >> hello.txt".to_owned(),
        };
        let (work, remote, branch) = established(&scratch, Some(check));

        let first = agent(&work, "issue-1", "one.txt", "one\n");
        let second = agent(&work, "issue-2", "two.txt", "two\n");

        assert!(matches!(
            branch.merge("issue-1").unwrap(),
            Outcome::Merged { checked: true, .. }
        ));
        // The green check left hello.txt dirty in the sole workspace;
        // without the opening restore this merge refuses in git's words.
        let Outcome::Merged { commit, .. } = branch.merge("issue-2").unwrap() else {
            panic!("check-dirt from a green run must not reach the next merge");
        };
        assert_eq!(tip(&work, "feature/wumpus"), commit);
        assert_eq!(tip(&remote, "feature/wumpus"), commit);

        // The dirt itself was never committed: what landed is history's
        // hello.txt, not the check's scribbles.
        let verified = git2::Repository::open(&work).unwrap();
        let tree = commit_of(&verified, "feature/wumpus").tree().unwrap();
        let hello = tree.get_name("hello.txt").unwrap();
        let blob = verified.find_blob(hello.id()).unwrap();
        assert_eq!(blob.content(), b"hello\n");

        tidy(&first);
        tidy(&second);
        tidy(branch.workspace());
    }

    #[test]
    fn a_red_check_resets_the_branch_and_leaves_the_remote_unchanged() {
        let scratch = Scratch::new("red");
        // The repository's own idea of green: the poison file is absent.
        let check = Check {
            command: "test ! -f poison.txt || { echo the wumpus got in; exit 1; }".to_owned(),
        };
        let (work, remote, branch) = established(&scratch, Some(check));
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
        let check = Check {
            command: "test -f hello.txt".to_owned(),
        };
        let (work, remote, branch) = established(&scratch, Some(check));

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

    /// A push the remote refuses unwinds the merge — Err means nothing
    /// landed — and the same issue branch merges again once the remote
    /// answers: the retry the reset invites actually works.
    #[test]
    fn a_failed_push_unwinds_the_merge_so_a_retry_can_land_it() {
        let scratch = Scratch::new("pushless");
        let (work, _) = seeded(&scratch);
        // A standing branch, so establishing pushes nothing; the forge
        // does not exist yet.
        git(&["-C", &work, "branch", "feature/wumpus", "HEAD"]);
        let gone = scratch.join("gone.git");
        let branch =
            Branch::establish(&work, "feature/wumpus", "main", Local(gone.clone()), None).unwrap();
        let before = tip(&work, "feature/wumpus");

        let issue = agent(&work, "issue-1", "one.txt", "one\n");
        let error = branch.merge("issue-1").unwrap_err();
        assert!(error.contains("gone.git"), "{error}");
        assert_eq!(
            tip(&work, "feature/wumpus"),
            before,
            "the merge was unwound: nothing landed"
        );

        // The remote comes up; the same branch merges and lands.
        git(&["init", "--bare", "--initial-branch=main", &gone]);
        let Outcome::Merged { commit, .. } = branch.merge("issue-1").unwrap() else {
            panic!("the retry lands");
        };
        assert_eq!(tip(&work, "feature/wumpus"), commit);
        assert_eq!(tip(&gone, "feature/wumpus"), commit);

        tidy(&issue);
        tidy(branch.workspace());
    }

    /// The tip is read under the merge lock: while a red check holds a
    /// doomed merge commit on the ref, a concurrent tip read waits it
    /// out and answers with the settled commit — never the one about to
    /// be reset away. This is the cut point dispatch uses.
    #[test]
    fn the_tip_never_answers_with_a_commit_a_red_check_is_about_to_reset() {
        let scratch = Scratch::new("tip");
        let sync = scratch.join("sync");
        std::fs::create_dir_all(&sync).unwrap();
        // The check announces itself, holds until released, then goes
        // red — keeping the doomed merge commit on the ref meanwhile.
        let check = Check {
            command: format!(
                "touch '{sync}/checking'; i=0; until [ -e '{sync}/release' ]; do \
                 i=$((i+1)); if [ \"$i\" -gt 600 ]; then exit 1; fi; sleep 0.05; done; exit 1"
            ),
        };
        let (work, _, branch) = established(&scratch, Some(check));
        let before = tip(&work, "feature/wumpus");
        let issue = agent(&work, "issue-1", "one.txt", "one\n");

        std::thread::scope(|scope| {
            let merging = scope.spawn(|| branch.merge("issue-1"));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !Path::new(&sync).join("checking").exists() {
                assert!(std::time::Instant::now() < deadline, "the check never ran");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let asked = scope.spawn(|| branch.tip());
            std::fs::write(Path::new(&sync).join("release"), "").unwrap();
            assert!(matches!(
                merging.join().unwrap().unwrap(),
                Outcome::Red { .. }
            ));
            assert_eq!(
                asked.join().unwrap().unwrap(),
                before,
                "the settled tip, not the doomed merge commit"
            );
        });

        tidy(&issue);
        tidy(branch.workspace());
    }

    /// The lock, exercised: two threads merging through one Branch both
    /// land, because merges queue rather than trample.
    #[test]
    fn concurrent_merges_are_serialized_and_both_land() {
        let scratch = Scratch::new("threads");
        let (work, _, branch) = established(&scratch, None);

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
        let tree = commit_of(&verified, "feature/wumpus").tree().unwrap();
        assert!(tree.get_name("one.txt").is_some());
        assert!(tree.get_name("two.txt").is_some());

        tidy(&first);
        tidy(&second);
        tidy(branch.workspace());
    }
}
