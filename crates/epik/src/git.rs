//! Epik's private clones, and the worktrees hung off them.
//!
//! Per repository, one bare clone under `<repos>/<owner>/<name>/`, created
//! lazily and brought up to date at the start of every `FeatureRun`. Deltas
//! happen only in worktrees under `<work>/<owner>/<name>/issue-<n>/`, which
//! share the clone's object store; the user's own checkout is never touched,
//! and integration never happens locally — branches are pushed and merged at
//! GitHub. That buys the invariant the tests demonstrate: everything under
//! both roots is a cache of GitHub, and deleting it all loses nothing but
//! time.
//!
//! Every verb spawns the `git` binary. The in-process alternatives (`git2`,
//! `gix`) are weakest exactly at worktrees and rebase, and every coding
//! agent already requires the binary, so harness and agent share one git.
//! Command lines and stderr stay inside this module; callers see types.
//!
//! The lifecycle is code rather than convention: [`fetch`](Git::fetch)
//! clones lazily, [`checkout`](Git::checkout) hangs a worktree off the
//! clone, and [`conclude`](Worktree::conclude) applies the retention policy
//! — a succeeded run's worktree and branch are deleted, a blocked or failed
//! one is kept for forensics until [`dismiss`](Worktree::dismiss)ed.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::config;
use crate::github::Repo;

/// Where the bare clones live under Epik's home.
const REPOS: &str = "repos";

/// Where the worktrees live under Epik's home.
const WORK: &str = "work";

/// The git verbs, rooted: one value says where the cache lives, and tests
/// root it in a temp directory instead of the real `~/.epik`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Git {
    repos: PathBuf,
    work: PathBuf,
}

impl Git {
    /// The real cache: `~/.epik/repos` and `~/.epik/work`.
    ///
    /// # Errors
    ///
    /// Returns an error when Epik's home is not discoverable.
    pub fn new() -> Result<Self> {
        let home = config::home()?;
        Ok(Self::rooted(home.join(REPOS), home.join(WORK)))
    }

    /// A cache rooted where the caller says — how tests stay out of
    /// `~/.epik`.
    #[must_use]
    pub fn rooted(repos: impl Into<PathBuf>, work: impl Into<PathBuf>) -> Self {
        Self {
            repos: repos.into(),
            work: work.into(),
        }
    }

    /// Where `repo`'s bare clone is, whether or not it exists yet.
    #[must_use]
    pub fn bare(&self, repo: &Repo) -> PathBuf {
        self.repos.join(&repo.owner).join(&repo.name)
    }

    /// Where `issue`'s worktree is, whether or not it exists yet.
    #[must_use]
    pub fn worktree(&self, repo: &Repo, issue: u64) -> PathBuf {
        self.work
            .join(&repo.owner)
            .join(&repo.name)
            .join(format!("issue-{issue}"))
    }

    /// Brings the cache's picture of `repo` up to date from `url` — the
    /// `FeatureRun`-start verb, and the lazy clone: the first call makes the
    /// bare clone, every call fetches.
    ///
    /// A bare clone carries no fetch refspec, so the mapping is stated here:
    /// GitHub's branches land under `refs/remotes/origin/*`, pruned to match,
    /// which is what [`checkout`](Self::checkout) cuts from — while the issue
    /// branches under `refs/heads` are Epik's own, which no fetch may touch.
    ///
    /// # Errors
    ///
    /// Returns an error when the clone or fetch could not be conducted: no
    /// runnable `git`, an unmakeable cache directory, or git refusing — an
    /// unreachable `url`, chiefly.
    pub fn fetch(&self, repo: &Repo, url: &str) -> Result<(), Error> {
        let bare = self.bare(repo);
        if !bare.is_dir() {
            // A clone that fails removes its target itself, so an existing
            // directory is a clone that succeeded.
            if let Some(parent) = bare.parent() {
                made(parent)?;
            }
            run(
                Command::new("git")
                    .args(["clone", "--bare"])
                    .arg(url)
                    .arg(&bare),
                "clone",
            )?;
        }
        run(
            Command::new("git").current_dir(&bare).args([
                "fetch",
                "--prune",
                "origin",
                "+refs/heads/*:refs/remotes/origin/*",
            ]),
            "fetch",
        )
    }

    /// Hangs a fresh worktree for `issue` off `repo`'s clone, on a new
    /// `branch` cut from `origin/<base>` — the fetched picture of GitHub,
    /// which is why a checkout follows a [`fetch`](Self::fetch).
    ///
    /// # Errors
    ///
    /// Returns an error when the worktree could not be conducted into being:
    /// no runnable `git`, an unmakeable cache directory, or git refusing — a
    /// `branch` that already exists, chiefly.
    pub fn checkout(
        &self,
        repo: &Repo,
        issue: u64,
        branch: &str,
        base: &str,
    ) -> Result<Worktree, Error> {
        let clone = self.bare(repo);
        let path = self.worktree(repo, issue);
        if let Some(parent) = path.parent() {
            made(parent)?;
        }
        run(
            Command::new("git")
                .current_dir(&clone)
                .args(["worktree", "add", "-b", branch])
                .arg(&path)
                .arg(format!("origin/{base}")),
            "worktree add",
        )?;
        Ok(Worktree {
            path,
            branch: branch.to_owned(),
            bare: clone,
        })
    }
}

/// A checked-out worktree: the agent's whole world on disk, and the value
/// the rest of the lifecycle happens to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Worktree {
    path: PathBuf,
    branch: String,
    /// The bare clone this worktree hangs off, where its removal is done.
    bare: PathBuf,
}

impl Worktree {
    /// Where the agent works: what `Task::worktree` is set to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The branch this worktree carries.
    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// The retention policy, applied: a succeeded run's worktree and branch
    /// have nothing left to say and are deleted; a blocked or failed run's
    /// worktree is forensic evidence, handed back to be kept until the user
    /// [`dismiss`](Self::dismiss)es it.
    ///
    /// # Errors
    ///
    /// Returns an error when a deletion git would have to perform could not
    /// be conducted, or was refused.
    pub fn conclude(self, outcome: Outcome) -> Result<Option<Self>, Error> {
        match outcome {
            Outcome::Succeeded => self.dismiss().map(|()| None),
            Outcome::Blocked | Outcome::Failed => Ok(Some(self)),
        }
    }

    /// Deletes the worktree and its branch, however the run went: cleanup on
    /// success, the end of forensics on a retained failure. Forced, because
    /// the whole point is discarding whatever state the run left.
    ///
    /// # Errors
    ///
    /// Returns an error when either deletion could not be conducted, or was
    /// refused.
    pub fn dismiss(self) -> Result<(), Error> {
        run(
            Command::new("git")
                .current_dir(&self.bare)
                .args(["worktree", "remove", "--force"])
                .arg(&self.path),
            "worktree remove",
        )?;
        run(
            Command::new("git")
                .current_dir(&self.bare)
                .args(["branch", "-D", &self.branch]),
            "branch -D",
        )
    }
}

/// How the harness judged a finished run: everything retention turns on.
///
/// The mapping from an agent's stop is the harness's call — done is judged
/// out-of-band through GitHub, so `Succeeded` is GitHub's verdict, never the
/// agent's belief.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The work landed; the worktree and branch have nothing left to say.
    Succeeded,
    /// The agent could not proceed; the worktree shows where it stood.
    Blocked,
    /// The run came to grief; the worktree is the wreckage to examine.
    Failed,
}

/// Why a verb could not be done — refusals as vocabulary, in the house rule,
/// never prose scraped from a log.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// The `git` binary would not start at all.
    Unstartable { error: String },
    /// git ran and said no. `verb` names the operation; `said` is git's own
    /// stderr, the one place its stringliness surfaces.
    Refused { verb: &'static str, said: String },
    /// The cache's own directories could not be made.
    Cache { path: PathBuf, error: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unstartable { error } => write!(f, "git would not start: {error}"),
            Self::Refused { verb, said } => write!(f, "git {verb} refused: {said}"),
            Self::Cache { path, error } => {
                write!(f, "shaping the cache at {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {}

/// Runs one git command to completion, translating the two ways it goes
/// wrong: the binary not starting, and git saying no. Headless throughout —
/// a credential prompt has nobody to answer it, so it refuses rather than
/// hangs.
fn run(command: &mut Command, verb: &'static str) -> Result<(), Error> {
    let output = command
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| Error::Unstartable {
            error: error.to_string(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Refused {
            verb,
            said: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

/// Makes the directories a verb is about to need.
fn made(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path).map_err(|error| Error::Cache {
        path: path.to_owned(),
        error: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    /// Runs git in `dir`, insisting it succeed: origin-side scaffolding,
    /// where the module's own verbs must not be trusted to build the world
    /// they are tested against.
    fn sh(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Writes `file` and commits it, with an identity and no signature so
    /// the laptop's own gitconfig cannot make the test flaky.
    fn commit(dir: &Path, file: &str, text: &str) {
        fs::write(dir.join(file), text).unwrap();
        sh(dir, &["add", "."]);
        sh(
            dir,
            &[
                "-c",
                "user.name=epik",
                "-c",
                "user.email=epik@example.invalid",
                "commit",
                "--no-gpg-sign",
                "-m",
                file,
            ],
        );
    }

    fn rev(dir: &Path, name: &str) -> String {
        let output = Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", name])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "rev-parse {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn has_ref(dir: &Path, name: &str) -> bool {
        Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "--verify", "--quiet", name])
            .output()
            .unwrap()
            .status
            .success()
    }

    /// A little GitHub: a local origin with one commit, and a cache rooted
    /// beside it in the same temp directory.
    struct World {
        root: TempDir,
        origin: PathBuf,
        git: Git,
        repo: Repo,
    }

    impl World {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            let origin = root.path().join("github");
            fs::create_dir_all(&origin).unwrap();
            sh(&origin, &["init", "-b", "main"]);
            commit(&origin, "README.md", "hello");
            let git = Git::rooted(root.path().join(REPOS), root.path().join(WORK));
            Self {
                root,
                origin,
                git,
                repo: Repo::new("epik-agent", "Epik"),
            }
        }

        fn url(&self) -> String {
            self.origin.display().to_string()
        }
    }

    #[test]
    fn the_first_fetch_clones_lazily_and_every_fetch_tracks_the_origin() {
        let w = World::new();

        w.git.fetch(&w.repo, &w.url()).unwrap();

        let bare = w.git.bare(&w.repo);
        assert!(
            bare.ends_with("repos/epik-agent/Epik"),
            "the clone lives at <repos>/<owner>/<name>: {}",
            bare.display()
        );
        assert_eq!(rev(&bare, "origin/main"), rev(&w.origin, "main"));

        commit(&w.origin, "second.md", "news");
        w.git.fetch(&w.repo, &w.url()).unwrap();

        assert_eq!(
            rev(&bare, "origin/main"),
            rev(&w.origin, "main"),
            "a second fetch finds the clone already there and brings it up to date"
        );
    }

    #[test]
    fn a_succeeded_run_leaves_no_worktree_and_no_branch() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();

        let worktree = w.git.checkout(&w.repo, 7, "issue-7-fix", "main").unwrap();

        assert_eq!(worktree.path(), w.git.worktree(&w.repo, 7));
        assert!(
            worktree.path().ends_with("work/epik-agent/Epik/issue-7"),
            "the worktree lives at <work>/<owner>/<name>/issue-<n>: {}",
            worktree.path().display()
        );
        assert!(worktree.path().join("README.md").is_file());
        assert_eq!(worktree.branch(), "issue-7-fix");
        commit(worktree.path(), "patch.rs", "the work");

        let retained = worktree.conclude(Outcome::Succeeded).unwrap();

        assert_eq!(retained, None);
        assert!(!w.git.worktree(&w.repo, 7).exists());
        assert!(
            !has_ref(&w.git.bare(&w.repo), "issue-7-fix"),
            "success deletes the branch along with the worktree"
        );
    }

    #[test]
    fn blocked_and_failed_runs_are_retained_for_forensics_until_dismissed() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();

        let blocked = w.git.checkout(&w.repo, 8, "issue-8", "main").unwrap();
        let failed = w.git.checkout(&w.repo, 9, "issue-9", "main").unwrap();

        let blocked = blocked.conclude(Outcome::Blocked).unwrap().unwrap();
        let failed = failed.conclude(Outcome::Failed).unwrap().unwrap();
        assert!(blocked.path().join("README.md").is_file());
        assert!(failed.path().join("README.md").is_file());

        blocked.dismiss().unwrap();

        assert!(!w.git.worktree(&w.repo, 8).exists());
        assert!(!has_ref(&w.git.bare(&w.repo), "issue-8"));
        assert!(
            w.git.worktree(&w.repo, 9).is_dir(),
            "dismissal is per worktree, not a purge"
        );
    }

    #[test]
    // The collect is the concurrency: every checkout is spawned before any
    // is joined, which is the property under test.
    #[allow(clippy::needless_collect)]
    fn concurrent_worktrees_hang_off_one_bare_clone() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();

        let worktrees: Vec<Worktree> = std::thread::scope(|scope| {
            let handles: Vec<_> = (1..=3u64)
                .map(|issue| {
                    let (git, repo) = (&w.git, &w.repo);
                    scope.spawn(move || {
                        let worktree = git
                            .checkout(repo, issue, &format!("issue-{issue}"), "main")
                            .unwrap();
                        commit(worktree.path(), &format!("delta-{issue}.md"), "work");
                        worktree
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });

        let bare = w.git.bare(&w.repo);
        for (issue, worktree) in (1..=3u64).zip(&worktrees) {
            assert!(worktree.path().join(format!("delta-{issue}.md")).is_file());
            assert!(has_ref(&bare, &format!("issue-{issue}")));
        }
        assert!(
            !worktrees[0].path().join("delta-2.md").exists(),
            "each worktree sees only its own branch's work"
        );
    }

    #[test]
    fn the_cache_is_nothing_but_a_cache_of_the_origin() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();
        let worktree = w.git.checkout(&w.repo, 5, "issue-5", "main").unwrap();
        commit(worktree.path(), "unpushed.md", "never left the laptop");

        // The invariant, demonstrated: delete everything under both roots...
        fs::remove_dir_all(w.root.path().join(REPOS)).unwrap();
        fs::remove_dir_all(w.root.path().join(WORK)).unwrap();

        // ...and rerunning the lifecycle reconstructs it from the origin.
        w.git.fetch(&w.repo, &w.url()).unwrap();
        let worktree = w.git.checkout(&w.repo, 5, "issue-5", "main").unwrap();

        assert_eq!(
            rev(&w.git.bare(&w.repo), "origin/main"),
            rev(&w.origin, "main")
        );
        assert!(worktree.path().join("README.md").is_file());
        assert!(
            !worktree.path().join("unpushed.md").exists(),
            "what was never pushed is gone: deleting the cache loses nothing but time"
        );
    }

    #[test]
    fn a_refusal_carries_gits_own_words() {
        let w = World::new();
        let nowhere = w.root.path().join("no-such-origin").display().to_string();

        let error = w
            .git
            .fetch(&Repo::new("nobody", "void"), &nowhere)
            .unwrap_err();

        let Error::Refused {
            verb: "clone",
            said,
        } = &error
        else {
            panic!("an unreachable origin is a refused clone, not {error:?}");
        };
        assert!(!said.is_empty(), "git says why in its own words");
    }

    #[test]
    fn a_branch_already_taken_is_a_refused_checkout() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();
        let _kept = w.git.checkout(&w.repo, 1, "issue-1", "main").unwrap();

        let error = w.git.checkout(&w.repo, 2, "issue-1", "main").unwrap_err();

        assert!(
            matches!(
                error,
                Error::Refused {
                    verb: "worktree add",
                    ..
                }
            ),
            "a retained worktree's branch is never silently clobbered: {error:?}"
        );
    }
}
