//! Epik's private clones, and the worktrees hung off them.
//!
//! Per repository, one bare clone under `<repos>/<owner>/<name>/`, created
//! lazily and brought up to date at the start of every `FeatureRun`. Deltas
//! happen only in worktrees under `<work>/<owner>/<name>/issue-<n>/`, which
//! share the clone's object store; the user's own checkout is never touched,
//! and integration never happens locally — branches are pushed and merged at
//! GitHub. That buys the invariant the tests demonstrate: everything under
//! both roots is a cache of GitHub, and deleting any of it — all of it, one
//! root, one worktree — loses nothing but time.
//!
//! Every verb spawns the `git` binary. The in-process alternatives (`git2`,
//! `gix`) are weakest exactly at worktrees and rebase, and every coding
//! agent already requires the binary, so harness and agent share one git.
//! Command lines and stderr stay inside this module; callers see types. The
//! spawns are hermetic and headless: the user's own git configuration goes
//! unread — Epik's git is Epik's — and neither git nor ssh may prompt, so
//! a wanted credential refuses rather than hangs.
//!
//! The lifecycle is code rather than convention: [`fetch`](Git::fetch)
//! clones lazily, [`checkout`](Git::checkout) hangs a worktree off the
//! clone, and [`conclude`](Worktree::conclude) applies the retention policy
//! — a succeeded run's worktree and branch are deleted, a blocked or failed
//! one is kept for forensics until [`dismiss`](Worktree::dismiss)ed.

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use anyhow::Result;

use crate::config;
use crate::github::Repo;

/// Where the bare clones live under Epik's home.
const REPOS: &str = "repos";

/// Where the worktrees live under Epik's home.
const WORK: &str = "work";

/// One registry writer at a time, process-wide: `worktree add`, `remove`,
/// and `prune` all rewrite a clone's worktree registry, and git gives them
/// no lock of their own — a concurrent add reads a sibling mid-birth, and a
/// concurrent prune could reap it. Held around every registry-touching
/// verb; a poisoned lock is taken over, since the registry belongs to git,
/// not to the panicked thread.
static REGISTRY: Mutex<()> = Mutex::new(());

fn registry() -> MutexGuard<'static, ()> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

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
    /// `FeatureRun`-start verb, and the lazy clone: the first call
    /// materializes the bare repository, every call fetches.
    ///
    /// GitHub's branches land under `refs/remotes/origin/*`, pruned to
    /// match, and nowhere else: `refs/heads` holds Epik's issue branches
    /// and nothing git fetched, so no pushed branch can come back after a
    /// cache wipe to collide with the rerun that wants its name.
    ///
    /// # Errors
    ///
    /// Returns an error when the clone or fetch could not be conducted: no
    /// runnable `git`, or git refusing — an unreachable `url`, chiefly.
    pub fn fetch(&self, repo: &Repo, url: &str) -> Result<(), Error> {
        let bare = self.bare(repo);
        if !bare.is_dir() {
            self.cloned(repo, url)?;
        }
        // `url` is authoritative on every call: a clone made against
        // yesterday's URL — or a bad one — is re-aimed rather than trusted,
        // so one wrong call can never poison the cache for every good one
        // after it. Read before write, so the steady state touches no
        // config lock for concurrent fetches to contend on.
        aimed(&bare, url)?;
        let fetched = || {
            run(
                Command::new("git")
                    .current_dir(&bare)
                    .args(["fetch", "--prune", "origin"]),
                "fetch",
            )
        };
        match fetched() {
            // Two runs racing the same fetch contend on ref locks; the
            // loser's refs are the winner's work, so one more look finds
            // them in place. A genuine refusal merely refuses twice.
            Err(Error::Refused { .. }) => fetched(),
            done => done,
        }
    }

    /// Materializes `repo`'s bare clone without ever leaving a half-made
    /// one at its path: the repository is shaped in a staging directory
    /// beside it and renamed into place whole, so the real path either does
    /// not exist or is entirely a repository — whatever kills the process.
    ///
    /// `init` + `remote add` rather than `clone --bare`, because a bare
    /// clone copies origin's branches into `refs/heads` — the namespace
    /// that is Epik's own. The remote's default refspec aims every fetch at
    /// `refs/remotes/origin/*`, and the first [`fetch`](Self::fetch) brings
    /// the actual objects.
    fn cloned(&self, repo: &Repo, url: &str) -> Result<(), Error> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let staging = self.repos.join(&repo.owner).join(format!(
            ".{}.cloning.{}.{}",
            repo.name,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        run(
            Command::new("git").args(["init", "--bare"]).arg(&staging),
            "init",
        )?;
        run(
            Command::new("git")
                .current_dir(&staging)
                .args(["remote", "add", "origin", url]),
            "remote add",
        )?;
        let bare = self.bare(repo);
        fs::rename(&staging, &bare).or_else(|error| {
            // Either a concurrent run won the race — its clone is this
            // clone — or the filesystem refused; the staging leftovers go
            // either way.
            let _ = fs::remove_dir_all(&staging);
            if bare.is_dir() {
                Ok(())
            } else {
                Err(Error::Cache {
                    path: bare,
                    error: error.to_string(),
                })
            }
        })
    }

    /// Hangs a fresh worktree for `issue` off `repo`'s clone, on `branch`
    /// cut from `origin/<base>` — the fetched picture of GitHub, which is
    /// why a checkout follows a [`fetch`](Self::fetch).
    ///
    /// A wipe of the work root alone undoes none of this: pruning clears
    /// the registration such a wipe leaves behind, and `-B` resets the
    /// branch it strands — stale cache, either one — while a branch alive
    /// in a living worktree still refuses, so a retained run is never
    /// clobbered.
    ///
    /// # Errors
    ///
    /// Returns an error when the worktree could not be conducted into
    /// being: no runnable `git`, no clone in the cache, or git refusing —
    /// a `branch` checked out in another worktree, chiefly.
    pub fn checkout(
        &self,
        repo: &Repo,
        issue: u64,
        branch: &str,
        base: &str,
    ) -> Result<Worktree, Error> {
        let clone = self.bare(repo);
        let path = self.worktree(repo, issue);
        let _registry = registry();
        run(
            Command::new("git")
                .current_dir(&clone)
                .args(["worktree", "prune"]),
            "worktree prune",
        )?;
        // `--no-track`: an issue branch tracks nothing — its push is the
        // harness's explicit act — and the tracking git would otherwise set
        // up is a write to the clone's one config file, which concurrent
        // checkouts must not contend on.
        run(
            Command::new("git")
                .current_dir(&clone)
                .args(["worktree", "add", "--no-track", "-B", branch])
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

    /// The worktree a previous run left at `issue`'s path, when one is
    /// there: the handle by which retained forensics are inspected and
    /// eventually dismissed. `branch` is the caller's naming, exactly as in
    /// [`checkout`](Self::checkout) — a worktree is never asked what it is.
    #[must_use]
    pub fn retained(&self, repo: &Repo, issue: u64, branch: &str) -> Option<Worktree> {
        let path = self.worktree(repo, issue);
        path.is_dir().then(|| Worktree {
            path,
            branch: branch.to_owned(),
            bare: self.bare(repo),
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

    /// The commit this worktree stands on — what binds a verified pull
    /// request to this run's work rather than a predecessor's.
    ///
    /// # Errors
    ///
    /// Returns an error when git cannot say: a worktree already wiped out
    /// from under the cache, chiefly.
    pub fn head(&self) -> Result<String, Error> {
        let output = output(
            Command::new("git")
                .current_dir(&self.path)
                .args(["rev-parse", "HEAD"]),
        )?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(Error::Refused {
                verb: "rev-parse",
                said: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }

    /// The retention policy, applied: a succeeded run's worktree and branch
    /// have nothing left to say and are deleted; a blocked or failed run's
    /// worktree is forensic evidence, handed back to be kept until the user
    /// [`dismiss`](Self::dismiss)es it.
    ///
    /// # Errors
    ///
    /// A failed deletion hands the worktree back beside the error, so
    /// nothing is lost and the dismissal can be retried.
    pub fn conclude(self, outcome: Outcome) -> Result<Option<Self>, (Self, Error)> {
        match outcome {
            Outcome::Succeeded => self.dismiss().map(|()| None),
            Outcome::Blocked | Outcome::Failed => Ok(Some(self)),
        }
    }

    /// Deletes the worktree and its branch, however the run went: cleanup
    /// on success, the end of forensics on a retained failure. Forced, and
    /// indifferent to what a crash or an external wipe already took — the
    /// whole point is discarding whatever state remains.
    ///
    /// # Errors
    ///
    /// A failed deletion hands the worktree back beside the error, so
    /// nothing is lost and the dismissal can be retried.
    pub fn dismiss(self) -> Result<(), (Self, Error)> {
        match self.removed() {
            Ok(()) => Ok(()),
            Err(error) => Err((self, error)),
        }
    }

    /// The deletion itself, idempotently: directory, registration, branch,
    /// each taken as it stands so a retry finishes what an earlier failure
    /// started.
    fn removed(&self) -> Result<(), Error> {
        let _registry = registry();
        if self.path.exists() {
            match run(
                Command::new("git")
                    .current_dir(&self.bare)
                    .args(["worktree", "remove", "--force"])
                    .arg(&self.path),
                "worktree remove",
            ) {
                // Refused: the clone does not know this directory — a wipe
                // and re-clone lost the registration — so it goes directly.
                Err(Error::Refused { .. }) => {
                    fs::remove_dir_all(&self.path).map_err(|error| Error::Cache {
                        path: self.path.clone(),
                        error: error.to_string(),
                    })?;
                }
                done => done?,
            }
        }
        // Clears whatever registration remains: the path-already-gone case,
        // and the direct deletion above.
        run(
            Command::new("git")
                .current_dir(&self.bare)
                .args(["worktree", "prune"]),
            "worktree prune",
        )?;
        if self.branched()? {
            run(
                Command::new("git")
                    .current_dir(&self.bare)
                    .args(["branch", "-D", &self.branch]),
                "branch -D",
            )?;
        }
        Ok(())
    }

    /// Whether the branch still exists — a probe, so a branch an earlier
    /// pass or a wipe already took is not a refusal.
    fn branched(&self) -> Result<bool, Error> {
        let output = output(Command::new("git").current_dir(&self.bare).args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", self.branch),
        ]))?;
        Ok(output.status.success())
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
    /// The clone a verb runs inside is not in the cache: wiped, or never
    /// fetched. A [`fetch`](Git::fetch) remakes it — nothing was lost but
    /// time.
    Missing { path: PathBuf },
    /// A filesystem step of the cache's own — landing a fresh clone at its
    /// path, clearing a discarded worktree — failed.
    Cache { path: PathBuf, error: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unstartable { error } => write!(f, "git would not start: {error}"),
            Self::Refused { verb, said } => write!(f, "git {verb} refused: {said}"),
            Self::Missing { path } => {
                write!(f, "no clone at {}: a fetch remakes it", path.display())
            }
            Self::Cache { path, error } => {
                write!(f, "shaping the cache at {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {}

/// Runs one git command to completion, translating the ways it goes wrong:
/// the clone it would run in being gone, the binary not starting, and git
/// saying no.
fn run(command: &mut Command, verb: &'static str) -> Result<(), Error> {
    let output = output(command)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Refused {
            verb,
            said: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

/// Points the clone's origin at `url` when it is not already there.
fn aimed(bare: &Path, url: &str) -> Result<(), Error> {
    let current = output(
        Command::new("git")
            .current_dir(bare)
            .args(["remote", "get-url", "origin"]),
    )?;
    if current.status.success() && String::from_utf8_lossy(&current.stdout).trim() == url {
        return Ok(());
    }
    run(
        Command::new("git")
            .current_dir(bare)
            .args(["remote", "set-url", "origin", url]),
        "remote set-url",
    )
}

/// One hermetic, headless git spawn. A missing working directory is named
/// as the cache problem it is — spawning there would misreport a broken
/// git — and no prompt can hang a harness with nobody at the keyboard: not
/// git's, and not ssh's, unless the environment already says how ssh should
/// behave. The user's own git configuration goes unread, as in
/// `epik-worker`: Epik's git is Epik's.
fn output(command: &mut Command) -> Result<Output, Error> {
    if let Some(dir) = command.get_current_dir().filter(|dir| !dir.is_dir()) {
        return Err(Error::Missing {
            path: dir.to_owned(),
        });
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    if env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes");
    }
    command.output().map_err(|error| Error::Unstartable {
        error: error.to_string(),
    })
}

/// Hermetic git scaffolding shared by this module's tests and the run
/// state machine's, so the policy — no user configuration, a fixed
/// identity — lives once.
#[cfg(test)]
// Test scaffolding is entitled to panic, and skips the API-doc ceremony a
// real surface owes.
#[allow(
    missing_debug_implementations,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::new_without_default,
    clippy::too_long_first_doc_paragraph
)]
pub mod testing {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use tempfile::TempDir;

    use super::{Git, REPOS, WORK};
    use crate::github::Repo;

    /// Runs git in `dir`, insisting it succeed: origin-side scaffolding,
    /// where the module's own verbs must not be trusted to build the world
    /// they are tested against. Hermetic like the module's own spawns, with
    /// a fixed identity so commits need no configuration at all.
    pub fn sh(dir: &Path, args: &[&str]) -> Output {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "epik")
            .env("GIT_AUTHOR_EMAIL", "epik@example.invalid")
            .env("GIT_COMMITTER_NAME", "epik")
            .env("GIT_COMMITTER_EMAIL", "epik@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Writes `file` and commits it.
    pub fn commit(dir: &Path, file: &str, text: &str) {
        fs::write(dir.join(file), text).unwrap();
        sh(dir, &["add", "."]);
        sh(dir, &["commit", "-m", file]);
    }

    pub fn rev(dir: &Path, name: &str) -> String {
        let output = sh(dir, &["rev-parse", name]);
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// A little GitHub: a local origin with one commit, and a cache rooted
    /// beside it in the same temp directory.
    pub struct World {
        pub root: TempDir,
        pub origin: PathBuf,
        pub git: Git,
        pub repo: Repo,
    }

    impl World {
        pub fn new() -> Self {
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

        pub fn url(&self) -> String {
            self.origin.display().to_string()
        }

        pub fn wipe(&self, root: &str) {
            fs::remove_dir_all(self.root.path().join(root)).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{World, commit, rev, sh};
    use super::*;

    fn has_ref(dir: &Path, name: &str) -> bool {
        Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "--verify", "--quiet", name])
            .output()
            .unwrap()
            .status
            .success()
    }

    /// Everything under `refs/heads` — which the module promises is Epik's
    /// issue branches and nothing else.
    fn heads(dir: &Path) -> String {
        let output = sh(dir, &["for-each-ref", "refs/heads"]);
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
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
        assert_eq!(
            heads(&bare),
            "",
            "origin's branches land under refs/remotes/origin, never refs/heads"
        );

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
    fn concurrent_first_fetches_agree_on_one_clone() {
        let w = World::new();

        std::thread::scope(|scope| {
            let fetches: Vec<_> = (0..2)
                .map(|_| {
                    let (git, repo, url) = (&w.git, &w.repo, w.url());
                    scope.spawn(move || git.fetch(repo, &url))
                })
                .collect();
            for fetch in fetches {
                fetch.join().unwrap().unwrap();
            }
        });

        assert_eq!(
            rev(&w.git.bare(&w.repo), "origin/main"),
            rev(&w.origin, "main"),
            "the race's loser lost nothing: the winner's clone is the same clone"
        );
    }

    #[test]
    fn the_url_argument_is_authoritative_on_every_fetch() {
        let w = World::new();
        let nowhere = w.root.path().join("no-such-origin").display().to_string();

        // The bad call lands a clone aimed nowhere, and fails...
        assert!(w.git.fetch(&w.repo, &nowhere).is_err());

        // ...but does not poison the cache: the next fetch re-aims the
        // existing clone at the URL it was actually given.
        w.git.fetch(&w.repo, &w.url()).unwrap();

        assert_eq!(
            rev(&w.git.bare(&w.repo), "origin/main"),
            rev(&w.origin, "main")
        );
    }

    #[test]
    fn a_retained_worktree_is_reconstructible_by_its_address() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();
        drop(w.git.checkout(&w.repo, 11, "issue-11", "main").unwrap());

        let handle = w.git.retained(&w.repo, 11, "issue-11").unwrap();
        handle.dismiss().unwrap();

        assert!(!w.git.worktree(&w.repo, 11).exists());
        assert_eq!(
            w.git.retained(&w.repo, 11, "issue-11"),
            None,
            "nothing retained, no handle"
        );
    }

    #[test]
    fn a_worktree_knows_the_commit_it_stands_on() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();
        let worktree = w.git.checkout(&w.repo, 12, "issue-12", "main").unwrap();

        assert_eq!(worktree.head().unwrap(), rev(&w.origin, "main"));

        commit(worktree.path(), "delta.md", "the work");

        assert_eq!(worktree.head().unwrap(), rev(worktree.path(), "HEAD"));
        assert_ne!(worktree.head().unwrap(), rev(&w.origin, "main"));
    }

    #[test]
    fn the_cache_is_nothing_but_a_cache_of_the_origin() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();
        let worktree = w.git.checkout(&w.repo, 5, "issue-5", "main").unwrap();
        commit(worktree.path(), "unpushed.md", "never left the laptop");

        // The invariant, demonstrated: delete everything under both roots...
        w.wipe(REPOS);
        w.wipe(WORK);

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
    fn a_branch_pushed_before_a_cache_wipe_can_run_again() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();
        let worktree = w.git.checkout(&w.repo, 4, "issue-4", "main").unwrap();
        commit(worktree.path(), "half.md", "blocked mid-way");
        sh(worktree.path(), &["push", "origin", "issue-4"]);

        w.wipe(REPOS);
        w.wipe(WORK);
        w.git.fetch(&w.repo, &w.url()).unwrap();

        // The name is free again: the pushed branch came back as
        // origin/issue-4, never as a refs/heads squatter refusing the rerun.
        let worktree = w.git.checkout(&w.repo, 4, "issue-4", "main").unwrap();

        assert!(has_ref(&w.git.bare(&w.repo), "origin/issue-4"));
        assert!(
            !worktree.path().join("half.md").exists(),
            "the rerun starts from the base, not from the last run's push"
        );
    }

    #[test]
    fn a_worktree_wiped_out_from_under_the_cache_heals_on_the_next_checkout() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();
        let _stranded = w.git.checkout(&w.repo, 6, "issue-6", "main").unwrap();

        // The work root goes alone: the registration and the branch stay
        // behind in the clone, both stale.
        w.wipe(WORK);
        let worktree = w.git.checkout(&w.repo, 6, "issue-6", "main").unwrap();

        assert!(worktree.path().join("README.md").is_file());
    }

    #[test]
    fn a_clone_interrupted_by_death_leaves_nothing_where_the_clone_goes() {
        let w = World::new();
        // A killed process can leave only staging corpses; the clone's real
        // path is never half-made. Fetch alongside the corpse.
        let corpse = w.root.path().join("repos/epik-agent/.Epik.cloning.0.0");
        fs::create_dir_all(&corpse).unwrap();

        w.git.fetch(&w.repo, &w.url()).unwrap();

        assert_eq!(
            rev(&w.git.bare(&w.repo), "origin/main"),
            rev(&w.origin, "main")
        );
    }

    #[test]
    fn a_failed_dismissal_hands_the_worktree_back_for_retry() {
        let w = World::new();
        w.git.fetch(&w.repo, &w.url()).unwrap();
        let worktree = w.git.checkout(&w.repo, 2, "issue-2", "main").unwrap();

        // The clone vanishes out from under the handle...
        w.wipe(REPOS);
        let (worktree, error) = worktree.dismiss().unwrap_err();
        assert!(matches!(error, Error::Missing { .. }), "{error:?}");

        // ...and once a fetch remakes it, the returned handle finishes the
        // job — the worktree the new clone never registered included.
        w.git.fetch(&w.repo, &w.url()).unwrap();
        worktree.dismiss().unwrap();

        assert!(!w.git.worktree(&w.repo, 2).exists());
    }

    #[test]
    fn a_verb_without_its_clone_names_the_cache_rather_than_git() {
        let w = World::new();

        let error = w.git.checkout(&w.repo, 1, "issue-1", "main").unwrap_err();

        let Error::Missing { path } = error else {
            panic!("a never-fetched repo is a cache miss, not a broken git: {error:?}");
        };
        assert_eq!(path, w.git.bare(&w.repo));
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
            verb: "fetch",
            said,
        } = &error
        else {
            panic!("an unreachable origin is a refused fetch, not {error:?}");
        };
        assert!(!said.is_empty(), "git says why in its own words");
    }

    #[test]
    fn a_branch_alive_in_another_worktree_is_a_refused_checkout() {
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
            "a living worktree's branch is never clobbered: {error:?}"
        );
    }
}
