//! Epik's git: the user's `git` binary, run as argv.
//!
//! Every git invocation in the crate comes through [`run`]: a
//! [`Command`] from [`commands`], argv straight into an [`Agent`] —
//! never a shell — under a deadline, with no terminal to prompt on.
//! Library code and a model's tool calls both arrive here, so the
//! command's construction is the one check either gets. Above `run`,
//! one verb per command, in the list's order, each returning what the
//! command reads back: a [`Repository`] from [`init`], [`clone`],
//! [`add_worktree`], and [`Repository::open`], the door a path named
//! from outside comes through; a [`Hash`](struct@Hash) from [`rev_parse`]; the typed listing
//! from [`status`], [`log`], [`list_worktrees`], and [`list_remotes`];
//! the text from [`diff`] and [`show`]; and nothing from the rest,
//! which either did what was asked or failed in git's words.

pub mod commands;
pub mod url;

use std::path::{Path, PathBuf};

#[cfg(feature = "native")]
use std::time::Duration;

#[cfg(feature = "native")]
use anyhow::{Context, Result, anyhow, bail};

#[cfg(feature = "native")]
use crate::agent::{Agent, Finished};
#[cfg(feature = "native")]
use crate::keystore::Secret;
#[cfg(feature = "native")]
use commands::{
    AbortMerge, Add, AddWorktree, Branch, Change, Clone, Command, Commit, DeleteBranch, Diff,
    Entry, Fetch, Hash, Init, ListRemotes, ListWorktrees, Log, Merge, Pull, Push, RemoteEntry,
    RemoveWorktree, Reset, RevParse, SetConfig, Show, ShowPrefix, Status, Switch, Worktree,
};

/// A repository on this machine: the directory an init, a clone, or a
/// worktree add made. The commands that need one take it, so each runs
/// in a repository that exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository(PathBuf);

impl Repository {
    /// The repository's directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(feature = "native")]
impl Repository {
    /// The repository at `path`: the root of a working tree, main or
    /// linked, or a bare repository — as git judges it, by
    /// [`ShowPrefix`].
    ///
    /// # Errors
    ///
    /// A relative `path`; git's own words when `path` is in no
    /// repository at all; and a `path` inside a repository but below
    /// its root, which says how far below.
    pub fn open(path: PathBuf) -> Result<Self> {
        let finished = settled(Command::ShowPrefix(ShowPrefix::new(path.clone())?))?;
        let prefix = ShowPrefix::prefix(said(&finished));
        if prefix.is_empty() {
            return Ok(Self(path));
        }
        bail!(
            "{} is {prefix} below the root of its repository",
            path.display()
        );
    }
}

/// How long one git invocation may take before it is killed and
/// reported.
#[cfg(feature = "native")]
const TIMEOUT: Duration = Duration::from_secs(60);

/// Runs `command`, and reports how git ended and what it said, stdout
/// then stderr.
///
/// # Errors
///
/// Git could not be run — the binary missing — or ran past the
/// deadline and was killed. A nonzero exit is a [`Finished`], for the
/// verb above to read.
#[cfg(feature = "native")]
pub fn run(command: &Command) -> Result<Finished> {
    run_within(command, TIMEOUT)
}

/// [`run`] against a stated deadline, which is how the tests exercise
/// the kill without sitting through the real one.
#[cfg(feature = "native")]
fn run_within(command: &Command, timeout: Duration) -> Result<Finished> {
    let argv = std::iter::once("git".to_owned())
        .chain(command.argv())
        .collect();
    // No terminal is attached, so a network verb that wants credentials
    // fails in words instead of waiting for a prompt nobody can see.
    // The user's helpers and SSH config still apply.
    let env = [("GIT_TERMINAL_PROMPT".to_owned(), Secret::from("0"))];
    Agent::new(argv, ".", env, Some(timeout))?.finish()
}

/// Runs `command` and settles the exit: what git said, for the verb
/// to read, or an error carrying git's own words on a nonzero exit.
#[cfg(feature = "native")]
fn settled(command: Command) -> Result<Finished> {
    let finished = run(&command)?;
    settle(&finished)?;
    Ok(finished)
}

/// What git said on stdout, or nothing.
#[cfg(feature = "native")]
fn said(finished: &Finished) -> &str {
    finished.stdout.as_deref().unwrap_or_default()
}

/// Makes a bare repository, and hands it back.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit.
#[cfg(feature = "native")]
pub fn init(init: Init) -> Result<Repository> {
    let path = init.path().to_owned();
    settled(Command::Init(init))?;
    Ok(Repository(path))
}

/// Clones, and hands back the repository it made.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit.
#[cfg(feature = "native")]
pub fn clone(clone: Clone) -> Result<Repository> {
    clone_within(clone, TIMEOUT)
}

#[cfg(feature = "native")]
fn clone_within(clone: Clone, timeout: Duration) -> Result<Repository> {
    let into = clone.destination().to_owned();
    settle(&run_within(&Command::Clone(clone), timeout)?)?;
    Ok(Repository(into))
}

/// Sets one config key.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit.
#[cfg(feature = "native")]
pub fn set_config(config: SetConfig) -> Result<()> {
    settled(Command::SetConfig(config)).map(drop)
}

/// Makes a branch.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a branch already
/// there among them.
#[cfg(feature = "native")]
pub fn branch(branch: Branch) -> Result<()> {
    settled(Command::Branch(branch)).map(drop)
}

/// Checks a branch out.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — local changes
/// the switch would overwrite among them.
#[cfg(feature = "native")]
pub fn switch(switch: Switch) -> Result<()> {
    settled(Command::Switch(switch)).map(drop)
}

/// Adds a linked worktree, and hands back the repository it is.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a branch already
/// checked out elsewhere among them.
#[cfg(feature = "native")]
pub fn add_worktree(worktree: AddWorktree) -> Result<Repository> {
    let linked = worktree.linked().to_owned();
    settled(Command::AddWorktree(worktree))?;
    Ok(Repository(linked))
}

/// Lists the worktrees, the main one first.
///
/// # Errors
///
/// [`run`]'s, git's own words on a nonzero exit, or a listing of a
/// shape git does not produce.
#[cfg(feature = "native")]
pub fn list_worktrees(worktrees: ListWorktrees) -> Result<Vec<Worktree>> {
    ListWorktrees::worktrees(said(&settled(Command::ListWorktrees(worktrees))?))
}

/// Stages paths.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a path that is
/// not there among them.
#[cfg(feature = "native")]
pub fn add(add: Add) -> Result<()> {
    settled(Command::Add(add)).map(drop)
}

/// Reads what differs from `HEAD`; nothing for a clean tree.
///
/// # Errors
///
/// [`run`]'s, git's own words on a nonzero exit, or an entry of a
/// shape git does not produce.
#[cfg(feature = "native")]
pub fn status(status: Status) -> Result<Vec<Change>> {
    Status::changes(said(&settled(Command::Status(status))?))
}

/// Reads a patch, as git prints one.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit.
#[cfg(feature = "native")]
pub fn diff(diff: Diff) -> Result<String> {
    Ok(said(&settled(Command::Diff(diff))?).to_owned())
}

/// Commits what is staged.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — nothing staged
/// among them.
#[cfg(feature = "native")]
pub fn commit(commit: Commit) -> Result<()> {
    settled(Command::Commit(commit)).map(drop)
}

/// The commit a revision names.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a revision naming
/// nothing, or something other than a commit. Git leaves the revision
/// out of its words, so the error names it.
#[cfg(feature = "native")]
pub fn rev_parse(rev_parse: RevParse) -> Result<Hash> {
    let revision = rev_parse.revision().to_owned();
    let finished = settled(Command::RevParse(rev_parse))
        .with_context(|| format!("{revision:?} names no commit"))?;
    RevParse::hash(said(&finished))
}

/// The history `log` asks for, newest first.
///
/// # Errors
///
/// [`run`]'s, git's own words on a nonzero exit — a revision that
/// names nothing, a repository with no commits yet — or a line of
/// output that is not an entry.
#[cfg(feature = "native")]
pub fn log(log: Log) -> Result<Vec<Entry>> {
    Log::entries(said(&settled(Command::Log(log))?))
}

/// Shows a commit, as git prints one.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit.
#[cfg(feature = "native")]
pub fn show(show: Show) -> Result<String> {
    Ok(said(&settled(Command::Show(show))?).to_owned())
}

/// Fetches from a remote.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a remote that
/// cannot be reached among them.
#[cfg(feature = "native")]
pub fn fetch(fetch: Fetch) -> Result<()> {
    settled(Command::Fetch(fetch)).map(drop)
}

/// Pulls, fast-forward only.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a branch that has
/// diverged from its upstream among them.
#[cfg(feature = "native")]
pub fn pull(pull: Pull) -> Result<()> {
    settled(Command::Pull(pull)).map(drop)
}

/// Merges a branch in.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a conflict, which
/// leaves the repository mid-merge for [`abort_merge`], among them.
#[cfg(feature = "native")]
pub fn merge(merge: Merge) -> Result<()> {
    settled(Command::Merge(merge)).map(drop)
}

/// Abandons a merge that conflicted.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — no merge to abort
/// among them.
#[cfg(feature = "native")]
pub fn abort_merge(abort: AbortMerge) -> Result<()> {
    settled(Command::AbortMerge(abort)).map(drop)
}

/// Moves the checked-out branch, and the tree with it.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit.
#[cfg(feature = "native")]
pub fn reset(reset: Reset) -> Result<()> {
    settled(Command::Reset(reset)).map(drop)
}

/// Pushes a branch.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a rejected push
/// among them.
#[cfg(feature = "native")]
pub fn push(push: Push) -> Result<()> {
    settled(Command::Push(push)).map(drop)
}

/// Lists the remotes.
///
/// # Errors
///
/// [`run`]'s, git's own words on a nonzero exit, or a listing of a
/// shape git does not produce.
#[cfg(feature = "native")]
pub fn list_remotes(remotes: ListRemotes) -> Result<Vec<RemoteEntry>> {
    ListRemotes::remotes(said(&settled(Command::ListRemotes(remotes))?))
}

/// Deletes a branch, merged or not.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — a branch checked
/// out somewhere among them.
#[cfg(feature = "native")]
pub fn delete_branch(branch: DeleteBranch) -> Result<()> {
    settled(Command::DeleteBranch(branch)).map(drop)
}

/// Removes a linked worktree.
///
/// # Errors
///
/// [`run`]'s, or git's own words on a nonzero exit — uncommitted work
/// in a removal that was not forced among them.
#[cfg(feature = "native")]
pub fn remove_worktree(worktree: RemoveWorktree) -> Result<()> {
    settled(Command::RemoveWorktree(worktree)).map(drop)
}

/// A nonzero exit as an error carrying git's own words: stderr, where
/// git explains itself, then stdout.
#[cfg(feature = "native")]
fn settle(finished: &Finished) -> Result<()> {
    if finished.exit.success() {
        return Ok(());
    }
    let words: Vec<&str> = [&finished.stderr, &finished.stdout]
        .into_iter()
        .flatten()
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .collect();
    if words.is_empty() {
        Err(anyhow!("git failed without saying why"))
    } else {
        Err(anyhow!("{}", words.join("\n")))
    }
}

/// These reach what only this module has: the runner's private
/// deadline, and the one way a path becomes a [`Repository`].
#[cfg(all(test, unix, feature = "native"))]
mod tests {
    use super::url::Url;
    use super::*;
    use crate::temp::Scratch;

    #[test]
    fn open_takes_a_root_and_refuses_everything_else() {
        let scratch = Scratch::new("open");
        let origin = init(Init::new(scratch.join("origin.git").into()).unwrap()).unwrap();
        let work = clone(
            Clone::new(
                Url::Local(origin.path().to_owned()),
                scratch.join("work").into(),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir(work.path().join("src")).unwrap();

        assert_eq!(Repository::open(origin.path().to_owned()).unwrap(), origin);
        assert_eq!(Repository::open(work.path().to_owned()).unwrap(), work);
        let below = Repository::open(work.path().join("src")).unwrap_err();
        assert!(
            below
                .to_string()
                .ends_with("is src/ below the root of its repository"),
            "{below}"
        );
        let outside = Repository::open(scratch.path().into()).unwrap_err();
        assert!(
            outside.to_string().contains("not a git repository"),
            "{outside}"
        );
        let relative = Repository::open("work".into()).unwrap_err();
        assert_eq!(
            relative.to_string(),
            "the repository must be an absolute path: work"
        );
    }

    /// The deadline, exercised on a short fuse rather than the real 60
    /// seconds: a clone from a loopback listener that never answers
    /// blocks git forever, so the Agent has to kill it and say so. The
    /// kernel completes the connection on the listener's behalf, so git
    /// gets as far as waiting for a reply.
    #[test]
    fn the_deadline_kills_a_clone_that_will_not_finish() {
        let scratch = Scratch::new("clone-deadline");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let from = format!("git://{}/hung", listener.local_addr().unwrap());

        let error = clone_within(
            Clone::new(from.parse().unwrap(), scratch.join("clone").into(), None).unwrap(),
            Duration::from_millis(500),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("killed"), "{error}");
    }
}
